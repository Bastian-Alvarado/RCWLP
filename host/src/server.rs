//! The host's protocol logic: discovery, pairing, sessions and virtual controllers. It reaches
//! the outside world only through [`Platform`], so tests can drive it without a network, a
//! clock or the ViGEmBus driver.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use crate::crypto::{self, HostKey};
use crate::protocol::{self, Commitment, Id, Nonce, PadKind, PadState, PubKey, Reader, Writer, kind};
use crate::store::{PairedClient, Store};

/// This host's version, shown at startup and sent in DISCOVER_REPLY.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Release all buttons if a phone goes quiet, so nothing stays held down.
pub const NEUTRAL_AFTER: Duration = Duration::from_secs(1);
/// Unplug the virtual controller after this long without input.
pub const DROP_AFTER: Duration = Duration::from_secs(15);
/// Status packets double as the host heartbeat and resend lost rumble changes.
pub const STATUS_EVERY: Duration = Duration::from_millis(250);
pub const PAIR_TIMEOUT: Duration = Duration::from_secs(60);
/// A phone that starts pairing over may replace its own unanswered request, but no sooner than
/// this, so nobody can cycle through requests hoping for one whose code matches the phone's.
pub const PAIR_REPLACE_AFTER: Duration = Duration::from_secs(3);

const REJECT_NOT_PAIRED: u8 = 1;

/// What the server needs from the machine it runs on.
pub trait Platform {
    fn now(&self) -> Instant;
    fn send(&self, data: &[u8], to: SocketAddr) -> std::io::Result<()>;
    /// Plugs in a virtual controller. `on_rumble(large, small)` may be called from any thread.
    fn plug(&self, kind: PadKind, on_rumble: RumbleFn) -> Result<Box<dyn VirtualPad>, String>;
}

pub type RumbleFn = Box<dyn FnMut(u8, u8) + Send>;

/// A virtual controller. Dropping it unplugs it.
pub trait VirtualPad {
    fn kind(&self) -> PadKind;
    /// 1-based XInput player number, 0 when unknown or not applicable.
    fn player(&self) -> u8;
    fn update(&mut self, state: &PadState);
}

/// What the server tells whoever shows it to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    /// Something happened: a phone paired, connected, timed out.
    Event(String),
    /// The answer to a command.
    Reply(String),
    /// Why a packet was ignored. Only sent with `verbose`.
    Debug(String),
    /// A pairing request is waiting for its code (the phone's name), or none is any more.
    Pairing(Option<String>),
    /// The phones connected right now, described. Sent whenever that changes.
    Connected(Vec<String>),
}

pub type OutFn = Box<dyn FnMut(Out) + Send>;

/// (client, large motor, small motor) sent from a driver notification thread.
type RumbleEvent = (Id, u8, u8);

/// Why a packet was ignored, for `--verbose`.
type Handled = Result<(), String>;

struct Session {
    kind: PadKind,
    key: [u8; 32],
    cnonce: Nonce,
    hnonce: Nonce,
    addr: SocketAddr,
    last_seq: u32,
    out_seq: u32,
    created: Instant,
}

struct Conn {
    name: String,
    pad: Option<Box<dyn VirtualPad>>,
    /// The session the phone is sending input on.
    active: Option<Session>,
    /// A newer session that only replaces `active` once the phone proves it holds the key,
    /// so a replayed CONNECT can't kick a phone that's in use.
    pending: Option<Session>,
    rumble: (u8, u8),
    last_input: Instant,
    neutral: bool,
    last_status: Instant,
}

impl Conn {
    fn new(name: String, now: Instant) -> Conn {
        Conn {
            name,
            pad: None,
            active: None,
            pending: None,
            rumble: (0, 0),
            last_input: now,
            neutral: true,
            last_status: now,
        }
    }
}

pub(crate) struct PairRequest {
    pub(crate) client_id: Id,
    /// The phone's public key stays hidden behind this until it has seen ours.
    pub(crate) commitment: Commitment,
    pub(crate) name: String,
    pub(crate) addr: SocketAddr,
    pub(crate) host_key: HostKey,
    /// Set once the phone reveals the public key it committed to.
    pub(crate) pair_key: Option<[u8; 32]>,
    pub(crate) created: Instant,
    pub(crate) decision: Option<bool>,
}

pub struct Server<P: Platform> {
    platform: P,
    host_name: String,
    store: Store,
    conns: HashMap<Id, Conn>,
    pairing: Option<PairRequest>,
    rumble_tx: Sender<RumbleEvent>,
    rumble_rx: Receiver<RumbleEvent>,
    out: OutFn,
    verbose: bool,
    /// Addresses already reported as running another protocol version, so the log isn't flooded.
    warned_versions: HashSet<IpAddr>,
    /// Addresses that sent a current-version DISCOVER. Current apps also send an old-version
    /// one so they can recognize old hosts, which mustn't count as an outdated app.
    current_apps: HashSet<IpAddr>,
    last_connected: Vec<String>,
}

impl<P: Platform> Server<P> {
    pub fn new(platform: P, store: Store, host_name: String, out: OutFn, verbose: bool) -> Server<P> {
        let (rumble_tx, rumble_rx) = mpsc::channel();
        Server {
            platform,
            host_name,
            store,
            conns: HashMap::new(),
            pairing: None,
            rumble_tx,
            rumble_rx,
            out,
            verbose,
            warned_versions: HashSet::new(),
            current_apps: HashSet::new(),
            last_connected: Vec::new(),
        }
    }

    pub fn host_name(&self) -> &str {
        &self.host_name
    }

    pub fn paired_count(&self) -> usize {
        self.store.clients.len()
    }

    pub fn handle(&mut self, buf: &[u8], from: SocketAddr) {
        let Some((version, k)) = Reader::new(buf).header_any() else {
            return self.debug(|| format!("{from}: ignored a packet that isn't RCWLP"));
        };
        // Discovery works across versions, so a mismatched phone can tell its user to update.
        let result = if k == kind::DISCOVER {
            self.on_discover(from, version)
        } else if version != protocol::VERSION {
            self.warn_version(from, version);
            Err(format!("it's protocol v{version}"))
        } else {
            match k {
                kind::PAIR_HELLO => self.on_pair_hello(buf, from),
                kind::PAIR_REVEAL => self.on_pair_reveal(buf, from),
                kind::CONNECT => self.on_connect(buf, from),
                kind::INPUT => self.on_input(buf, from),
                kind::DISCONNECT => self.on_disconnect(buf),
                _ => Err("unknown kind".into()),
            }
        };
        if let Err(why) = result {
            self.debug(|| format!("{from}: ignored {}: {why}", kind_name(k)));
        }
    }

    fn on_discover(&mut self, from: SocketAddr, version: u8) -> Handled {
        if version == protocol::VERSION {
            self.current_apps.insert(from.ip());
        } else {
            self.warn_version(from, version);
        }
        let reply = Writer::new(kind::DISCOVER_REPLY)
            .bytes(&self.store.host_id)
            .string(&self.host_name)
            .string(VERSION)
            .finish();
        self.send(&reply, from);
        Ok(())
    }

    fn warn_version(&mut self, from: SocketAddr, version: u8) {
        if !self.current_apps.contains(&from.ip()) && self.warned_versions.insert(from.ip()) {
            let which = if version < protocol::VERSION { "an older" } else { "a newer" };
            self.event(format!(
                "The phone at {} runs {which} RCWLP app (protocol v{version}, this host is v{}). \
                 Update both to the same version.",
                from.ip(),
                protocol::VERSION
            ));
        }
    }

    fn on_pair_hello(&mut self, buf: &[u8], addr: SocketAddr) -> Handled {
        let mut r = Reader::new(buf);
        r.header();
        let client_id: Id = r.array().ok_or("too short")?;
        let commitment: Commitment = r.array().ok_or("too short")?;
        let name = r.string().ok_or("too short")?;
        let now = self.platform.now();

        let mut replacing = false;
        if let Some(p) = &mut self.pairing {
            // The phone retries its hello until it gets an answer; repeat ours.
            if p.client_id == client_id && p.commitment == commitment {
                p.addr = addr;
                let reply = pair_reply(p, &self.store.host_id);
                self.send(&reply, addr);
                return Ok(());
            }
            let age = now - p.created;
            if p.decision.is_none() && age < PAIR_TIMEOUT {
                // A new key from the same phone means it started over (say, after Cancel).
                if p.client_id != client_id {
                    return Err(format!("busy pairing \"{}\"", p.name));
                }
                if age < PAIR_REPLACE_AFTER {
                    return Err("the phone restarted pairing too soon".into());
                }
                replacing = true;
            }
        }

        let p = PairRequest {
            client_id,
            commitment,
            name,
            addr,
            host_key: HostKey::random(),
            pair_key: None,
            created: now,
            decision: None,
        };
        let name = p.name.clone();
        let reply = pair_reply(&p, &self.store.host_id);
        self.pairing = Some(p);
        if replacing {
            self.event(format!("\"{name}\" started pairing again, so it shows a new code now."));
        } else {
            self.event(format!("Pairing request from \"{name}\" at {}.", addr.ip()));
        }
        self.event("  To accept, type the 6-digit code shown on the phone. Type n to reject.".into());
        (self.out)(Out::Pairing(Some(name)));
        self.send(&reply, addr);
        Ok(())
    }

    /// The phone's second step: the public key behind its hello's commitment.
    fn on_pair_reveal(&mut self, buf: &[u8], addr: SocketAddr) -> Handled {
        let mut r = Reader::new(buf);
        r.header();
        let client_id: Id = r.array().ok_or("too short")?;
        let client_pub: PubKey = r.array().ok_or("too short")?;

        let host_id = self.store.host_id;
        let p = self.pairing.as_mut().filter(|p| p.client_id == client_id).ok_or("no pairing request from it")?;
        if crypto::commitment(&client_pub) != p.commitment {
            return Err("the key doesn't match its commitment".into());
        }
        if p.pair_key.is_none() {
            p.pair_key = Some(p.host_key.pair_key(&client_id, &host_id, &client_pub).ok_or("invalid public key")?);
        }
        p.addr = addr;
        // The phone keeps revealing until it hears the result.
        if p.decision.is_some() {
            let reply = pair_reply(p, &host_id);
            self.send(&reply, addr);
        }
        Ok(())
    }

    fn accept(&mut self, code: u32) {
        let now = self.platform.now();
        let Some(p) = waiting(&mut self.pairing, now) else {
            return self.reply("No pairing request is waiting.");
        };
        let Some(pair_key) = p.pair_key else {
            return self.reply("The phone hasn't finished its part yet. Type the code again in a moment.");
        };
        if crypto::pair_code(&pair_key) != code {
            let msg = format!("That isn't the code for \"{}\". Check the phone and type it again, or n to reject.", p.name);
            return self.reply(&msg);
        }
        p.decision = Some(true);
        let (client_id, name, addr) = (p.client_id, p.name.clone(), p.addr);
        let reply = pair_reply(p, &self.store.host_id);
        self.store.upsert(PairedClient { id: client_id, name: name.clone(), key: pair_key });
        // Any live connection was keyed with the old pairing.
        self.conns.remove(&client_id);
        self.event(format!("Paired with \"{name}\"."));
        (self.out)(Out::Pairing(None));
        self.send(&reply, addr);
        self.report_connected();
    }

    fn reject(&mut self) {
        let now = self.platform.now();
        let Some(p) = waiting(&mut self.pairing, now) else {
            return self.reply("No pairing request is waiting.");
        };
        p.decision = Some(false);
        let (name, addr) = (p.name.clone(), p.addr);
        let reply = pair_reply(p, &self.store.host_id);
        self.event(format!("Rejected \"{name}\"."));
        (self.out)(Out::Pairing(None));
        self.send(&reply, addr);
    }

    fn on_connect(&mut self, buf: &[u8], addr: SocketAddr) -> Handled {
        let (body, mac) = protocol::split_mac(buf).ok_or("too short")?;
        let mut r = Reader::new(body);
        r.header();
        let client_id: Id = r.array().ok_or("too short")?;
        let cnonce: Nonce = r.array().ok_or("too short")?;
        let pad_kind = PadKind::from_wire(r.u8().ok_or("too short")?).ok_or("unknown controller type")?;
        let now = self.platform.now();

        let Some(paired) = self.store.find(&client_id).filter(|c| crypto::verify16(&c.key, body, mac)) else {
            let reject = Writer::new(kind::CONNECT_REJECT).u8(REJECT_NOT_PAIRED).finish();
            self.send(&reject, addr);
            return Err("not paired (or paired with an older key); sent CONNECT_REJECT".into());
        };
        let pair_key = paired.key;
        let name = paired.name.clone();
        let conn = self.conns.entry(client_id).or_insert_with(|| Conn::new(name, now));

        // A retransmitted CONNECT gets the same WELCOME back.
        if let Some(s) = conn.active.iter_mut().chain(conn.pending.iter_mut()).find(|s| s.cnonce == cnonce) {
            s.addr = addr;
            let welcome = welcome_packet(&s.hnonce, &s.key);
            self.send(&welcome, addr);
            return Ok(());
        }

        let hnonce = crypto::random();
        let session = Session {
            kind: pad_kind,
            key: crypto::session_key(&pair_key, &cnonce, &hnonce),
            cnonce,
            hnonce,
            addr,
            last_seq: 0,
            out_seq: 0,
            created: now,
        };
        let welcome = welcome_packet(&session.hnonce, &session.key);
        conn.pending = Some(session);
        self.send(&welcome, addr);
        Ok(())
    }

    fn on_input(&mut self, buf: &[u8], addr: SocketAddr) -> Handled {
        let (body, mac) = protocol::split_mac(buf).ok_or("too short")?;
        let mut r = Reader::new(body);
        r.header();
        let client_id: Id = r.array().ok_or("too short")?;
        let seq = r.u32().ok_or("too short")?;
        let state = PadState::read(&mut r).ok_or("too short")?;
        let now = self.platform.now();

        let conn = self.conns.get_mut(&client_id).ok_or("no session for that phone")?;
        let promoted = if conn.active.as_ref().is_some_and(|s| crypto::verify16(&s.key, body, mac)) {
            false
        } else if conn.pending.as_ref().is_some_and(|s| crypto::verify16(&s.key, body, mac)) {
            conn.active = conn.pending.take();
            true
        } else {
            return Err("bad signature".into());
        };

        let session = conn.active.as_mut().ok_or("no session")?;
        if seq <= session.last_seq {
            return Err(format!("replayed or reordered (seq {seq}, last {})", session.last_seq));
        }
        session.last_seq = seq;
        session.addr = addr;
        let pad_kind = session.kind;

        // The phone switched controller type since its last session: swap the virtual pad.
        if conn.pad.as_ref().is_some_and(|p| p.kind() != pad_kind) {
            conn.pad = None;
        }
        let mut message = None;
        if conn.pad.is_none() {
            let rumble = self.rumble_tx.clone();
            let on_rumble: RumbleFn = Box::new(move |large, small| {
                let _ = rumble.send((client_id, large, small));
            });
            match self.platform.plug(pad_kind, on_rumble) {
                Ok(pad) => {
                    message = Some(format!("\"{}\" connected as {}.", conn.name, describe(pad.as_ref())));
                    conn.pad = Some(pad);
                }
                Err(e) => {
                    let msg = format!("Could not create a virtual controller for \"{}\": {e}", conn.name);
                    self.conns.remove(&client_id);
                    self.event(msg);
                    return Ok(());
                }
            }
        } else if promoted {
            message = Some(format!("\"{}\" reconnected.", conn.name));
        }

        if let Some(pad) = conn.pad.as_mut() {
            pad.update(&state);
        }
        conn.last_input = now;
        conn.neutral = false;
        if promoted {
            let packet = status_for(conn, now);
            if let Some((packet, to)) = packet {
                self.send(&packet, to);
            }
        }
        if let Some(m) = message {
            self.event(m);
            self.report_connected();
        }
        Ok(())
    }

    fn on_disconnect(&mut self, buf: &[u8]) -> Handled {
        let (body, mac) = protocol::split_mac(buf).ok_or("too short")?;
        let mut r = Reader::new(body);
        r.header();
        let client_id: Id = r.array().ok_or("too short")?;
        let seq = r.u32().ok_or("too short")?;

        let session = self.conns.get(&client_id).and_then(|c| c.active.as_ref()).ok_or("no session for that phone")?;
        if !crypto::verify16(&session.key, body, mac) {
            return Err("bad signature".into());
        }
        if seq <= session.last_seq {
            return Err("replayed".into());
        }
        if let Some(conn) = self.conns.remove(&client_id) {
            self.event(format!("\"{}\" disconnected.", conn.name));
            self.report_connected();
        }
        Ok(())
    }

    /// Sends rumble that games asked for since the last call.
    pub fn drain_rumble(&mut self) {
        let now = self.platform.now();
        while let Ok((client_id, large, small)) = self.rumble_rx.try_recv() {
            if let Some(conn) = self.conns.get_mut(&client_id) {
                conn.rumble = (large, small);
                if let Some((packet, to)) = status_for(conn, now) {
                    self.send(&packet, to);
                }
            }
        }
    }

    /// Timeouts and heartbeats. Call at least every few tens of milliseconds.
    pub fn tick(&mut self) {
        let now = self.platform.now();

        if let Some(p) = &self.pairing
            && now - p.created > PAIR_TIMEOUT
        {
            let expired = p.decision.is_none().then(|| p.name.clone());
            self.pairing = None;
            if let Some(name) = expired {
                self.event(format!("Pairing request from \"{name}\" expired."));
                (self.out)(Out::Pairing(None));
            }
        }

        let mut gone = Vec::new();
        let mut statuses = Vec::new();
        for (id, conn) in &mut self.conns {
            let idle = now - conn.last_input;
            if conn.active.is_some() {
                if idle > DROP_AFTER {
                    gone.push(*id);
                    continue;
                }
                if idle > NEUTRAL_AFTER && !conn.neutral {
                    if let Some(pad) = &mut conn.pad {
                        pad.update(&PadState::default());
                    }
                    conn.neutral = true;
                }
                if now - conn.last_status >= STATUS_EVERY {
                    statuses.extend(status_for(conn, now));
                }
            } else if conn.pending.as_ref().is_none_or(|s| now - s.created > DROP_AFTER) {
                gone.push(*id);
            }
        }
        for (packet, to) in statuses {
            self.send(&packet, to);
        }
        let mut changed = false;
        for id in gone {
            if let Some(conn) = self.conns.remove(&id)
                && conn.pad.is_some()
            {
                self.event(format!("\"{}\" timed out.", conn.name));
                changed = true;
            }
        }
        if changed {
            self.report_connected();
        }
    }

    /// Runs one console command. Returns false when the user asked to quit.
    pub fn command(&mut self, line: &str) -> bool {
        let line = line.trim();
        if let Some(code) = parse_code(line) {
            self.accept(code);
            return true;
        }
        let mut words = line.split_whitespace();
        match (words.next(), words.next()) {
            (None, _) => {}
            // Typing the code makes sure it was compared, which "y" never did.
            (Some("y" | "yes"), None) => self.reply("Type the 6-digit code shown on the phone to accept it."),
            (Some("n" | "no"), None) => self.reject(),
            (Some("list"), None) => self.list(),
            (Some("forget"), Some(n)) => match n.parse::<usize>().ok().and_then(|n| n.checked_sub(1)) {
                Some(i) => match self.store.remove(i) {
                    Some(c) => {
                        self.conns.remove(&c.id);
                        self.reply(&format!("Forgot \"{}\".", c.name));
                        self.report_connected();
                    }
                    None => self.reply(&format!("No phone #{n}. Type list to see them.")),
                },
                None => self.reply("Usage: forget <number from list>"),
            },
            (Some("quit" | "exit"), None) => return false,
            _ => self.reply(HELP),
        }
        true
    }

    fn list(&mut self) {
        if self.store.clients.is_empty() {
            return self.reply("No paired phones. Open the app on your phone and pick this PC to pair.");
        }
        let lines: Vec<String> = self
            .store
            .clients
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let state = match self.conns.get(&c.id).and_then(|conn| conn.pad.as_deref()) {
                    Some(pad) => format!("connected as {}", describe(pad)),
                    None => "not connected".into(),
                };
                format!("  {}. {} ({state})", i + 1, c.name)
            })
            .collect();
        for line in lines {
            self.reply(&line);
        }
    }

    fn report_connected(&mut self) {
        let mut now: Vec<String> = self
            .conns
            .values()
            .filter_map(|c| c.pad.as_deref().map(|pad| format!("{} ({})", c.name, describe(pad))))
            .collect();
        now.sort();
        if now != self.last_connected {
            self.last_connected = now.clone();
            (self.out)(Out::Connected(now));
        }
    }

    fn send(&mut self, data: &[u8], to: SocketAddr) {
        if let Err(e) = self.platform.send(data, to) {
            self.debug(|| format!("Send to {to} failed: {e}"));
        }
    }

    pub fn event(&mut self, msg: String) {
        (self.out)(Out::Event(msg));
    }

    fn reply(&mut self, msg: &str) {
        (self.out)(Out::Reply(msg.into()));
    }

    fn debug(&mut self, msg: impl FnOnce() -> String) {
        if self.verbose {
            (self.out)(Out::Debug(msg()));
        }
    }
}

pub const HELP: &str = "Commands: list | forget <n> | quit   (to pair, type the code the phone shows, or n to reject)";

fn kind_name(k: u8) -> String {
    match k {
        kind::DISCOVER => "DISCOVER".into(),
        kind::PAIR_HELLO => "PAIR_HELLO".into(),
        kind::PAIR_REVEAL => "PAIR_REVEAL".into(),
        kind::CONNECT => "CONNECT".into(),
        kind::INPUT => "INPUT".into(),
        kind::DISCONNECT => "DISCONNECT".into(),
        _ => format!("packet {k:#04x}"),
    }
}

pub fn describe(pad: &dyn VirtualPad) -> String {
    match pad.kind() {
        PadKind::Xbox360 if pad.player() > 0 => format!("Xbox 360 controller, player {}", pad.player()),
        PadKind::Xbox360 => "Xbox 360 controller".into(),
        PadKind::DualShock4 => "PS4 controller with gyro".into(),
    }
}

/// The pairing request still waiting for a decision, if it hasn't expired.
fn waiting(pairing: &mut Option<PairRequest>, now: Instant) -> Option<&mut PairRequest> {
    pairing.as_mut().filter(|p| p.decision.is_none() && now - p.created <= PAIR_TIMEOUT)
}

/// Reads a typed pairing code: "123456" or "123 456", as the phone shows it.
pub fn parse_code(line: &str) -> Option<u32> {
    let digits: String = line.split_whitespace().collect();
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

pub(crate) fn pair_reply(p: &PairRequest, host_id: &Id) -> Vec<u8> {
    match (p.decision, p.pair_key) {
        (None, _) => Writer::new(kind::PAIR_CHALLENGE).bytes(host_id).bytes(&p.host_key.public).finish(),
        (Some(true), Some(key)) => Writer::new(kind::PAIR_RESULT).u8(1).bytes(&crypto::pair_ok_mac(&key)).finish(),
        _ => Writer::new(kind::PAIR_RESULT).u8(0).bytes(&[0; protocol::MAC_LEN]).finish(),
    }
}

pub(crate) fn welcome_packet(hnonce: &Nonce, key: &[u8; 32]) -> Vec<u8> {
    Writer::new(kind::WELCOME).bytes(hnonce).sign(key)
}

pub(crate) fn status_packet(seq: u32, (large, small): (u8, u8), player: u8, key: &[u8; 32]) -> Vec<u8> {
    Writer::new(kind::STATUS).u32(seq).u8(large).u8(small).u8(player).sign(key)
}

/// The next STATUS for a connection and where to send it, or None without an active session.
fn status_for(conn: &mut Conn, now: Instant) -> Option<(Vec<u8>, SocketAddr)> {
    conn.last_status = now;
    let s = conn.active.as_mut()?;
    s.out_seq += 1;
    let player = conn.pad.as_ref().map_or(0, |p| p.player());
    Some((status_packet(s.out_seq, conn.rumble, player, &s.key), s.addr))
}

#[cfg(test)]
mod tests;
