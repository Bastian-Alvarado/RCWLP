//! The host's protocol logic: discovery, pairing, sessions and virtual controllers. It reaches
//! the outside world only through [`Platform`], so tests can drive it without a network, a
//! clock or the ViGEmBus driver.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use crate::crypto::{self, HostKey};
use crate::protocol::{self, Commitment, Id, Input, Nonce, PadKind, PadState, PubKey, Reader, Writer, kind};
use crate::store::{PairedClient, Store};

/// This host's version, shown at startup and sent in DISCOVER_REPLY.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Release all buttons if a phone goes quiet, so nothing stays held down.
pub const NEUTRAL_AFTER: Duration = Duration::from_secs(1);
/// Unplug the virtual controllers after this long without input.
pub const DROP_AFTER: Duration = Duration::from_secs(15);
/// Unplug one controller after its slot has been missing from input for this long.
pub const SLOT_GONE_AFTER: Duration = Duration::from_secs(2);
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
    /// Plugs in a virtual controller. `on_feedback` may be called from any thread.
    fn plug(&self, kind: PadKind, on_feedback: FeedbackFn) -> Result<Box<dyn VirtualPad>, String>;
}

/// What a game sends back to a controller.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Feedback {
    pub large: u8,
    pub small: u8,
    /// DualShock 4 lightbar colour, black for Xbox pads.
    pub lightbar: (u8, u8, u8),
    /// The Xbox player number when XInput changes it, 1-based.
    pub player: Option<u8>,
}

pub type FeedbackFn = Box<dyn FnMut(Feedback) + Send>;

/// A virtual controller. Dropping it unplugs it.
pub trait VirtualPad {
    fn kind(&self) -> PadKind;
    /// 1-based XInput player number, 0 when unknown or not applicable.
    fn player(&self) -> u8;
    /// `battery` is the phone's, in INPUT's encoding.
    fn update(&mut self, state: &PadState, battery: u8);
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
    /// The controllers connected right now, described. Sent whenever that changes.
    Connected(Vec<String>),
}

pub type OutFn = Box<dyn FnMut(Out) + Send>;

/// (client, slot, feedback) sent from a driver notification thread.
type FeedbackEvent = (Id, u8, Feedback);

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

/// One virtual controller of a phone.
struct Slot {
    pad: Box<dyn VirtualPad>,
    feedback: Feedback,
    last_seen: Instant,
    /// The player number last shown to the user, to announce when XInput renumbers.
    announced_player: u8,
}

impl Slot {
    fn player(&self) -> u8 {
        self.feedback.player.unwrap_or_else(|| self.pad.player())
    }
}

struct Conn {
    name: String,
    /// The phone's controllers by slot.
    slots: BTreeMap<u8, Slot>,
    /// The session the phone is sending input on.
    active: Option<Session>,
    /// A newer session that only replaces `active` once the phone proves it holds the key,
    /// so a replayed CONNECT can't kick a phone that's in use.
    pending: Option<Session>,
    last_input: Instant,
    neutral: bool,
    last_status: Instant,
    /// The phone's round trip in ms, as it last reported it.
    rtt_ms: Option<u16>,
    battery: u8,
}

impl Conn {
    fn new(name: String, now: Instant) -> Conn {
        Conn {
            name,
            slots: BTreeMap::new(),
            active: None,
            pending: None,
            last_input: now,
            neutral: true,
            last_status: now,
            rtt_ms: None,
            battery: protocol::BATTERY_UNKNOWN,
        }
    }

    /// How a slot's controller appears in messages: the phone's name, numbered from the second on.
    fn label(&self, slot: u8) -> String {
        if slot == 0 { self.name.clone() } else { format!("{} #{}", self.name, slot + 1) }
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
    feedback_tx: Sender<FeedbackEvent>,
    feedback_rx: Receiver<FeedbackEvent>,
    out: OutFn,
    verbose: bool,
    /// Addresses already reported as running another protocol version, so the log isn't flooded.
    warned_versions: HashSet<IpAddr>,
    /// Addresses that sent a current-version DISCOVER. Current apps also send an old-version
    /// one so they can recognize old hosts, which mustn't count as an outdated app.
    current_apps: HashSet<IpAddr>,
    /// Keys of phones that just unpaired themselves, so a repeated UNPAIR still gets an answer.
    recently_unpaired: HashMap<Id, ([u8; 32], Instant)>,
    last_connected: Vec<String>,
}

impl<P: Platform> Server<P> {
    pub fn new(platform: P, store: Store, host_name: String, out: OutFn, verbose: bool) -> Server<P> {
        let (feedback_tx, feedback_rx) = mpsc::channel();
        Server {
            platform,
            host_name,
            store,
            conns: HashMap::new(),
            pairing: None,
            feedback_tx,
            feedback_rx,
            out,
            verbose,
            warned_versions: HashSet::new(),
            current_apps: HashSet::new(),
            recently_unpaired: HashMap::new(),
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
                kind::UNPAIR => self.on_unpair(buf, from),
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
        let input = Input::read(&mut r).ok_or("malformed controller list")?;
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

        // The phone switched controller type since its last session: replace its pads.
        if conn.slots.values().any(|s| s.pad.kind() != pad_kind) {
            conn.slots.clear();
        }
        conn.last_input = now;
        conn.neutral = false;
        conn.rtt_ms = (input.rtt_ms != protocol::RTT_UNKNOWN).then_some(input.rtt_ms);
        conn.battery = input.battery;

        let mut events = Vec::new();
        for (slot, state) in &input.pads {
            if !conn.slots.contains_key(slot) {
                let feedback = self.feedback_tx.clone();
                let slot_no = *slot;
                let on_feedback: FeedbackFn = Box::new(move |f| {
                    let _ = feedback.send((client_id, slot_no, f));
                });
                match self.platform.plug(pad_kind, on_feedback) {
                    Ok(pad) => {
                        events.push(format!("\"{}\" connected as {}.", conn.label(*slot), describe(pad.kind(), pad.player())));
                        let announced_player = pad.player();
                        conn.slots.insert(*slot, Slot { pad, feedback: Feedback::default(), last_seen: now, announced_player });
                    }
                    Err(e) => {
                        let msg = format!("Could not create a virtual controller for \"{}\": {e}", conn.label(*slot));
                        self.conns.remove(&client_id);
                        self.event(msg);
                        self.report_connected();
                        return Ok(());
                    }
                }
            }
            if let Some(s) = conn.slots.get_mut(slot) {
                s.last_seen = now;
                s.pad.update(state, input.battery);
            }
        }
        if promoted && events.is_empty() {
            events.push(format!("\"{}\" reconnected.", conn.name));
        }
        if promoted && let Some((packet, to)) = status_for(conn, now) {
            self.send(&packet, to);
        }
        let changed = !events.is_empty();
        for e in events {
            self.event(e);
        }
        if changed {
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

    /// The phone forgot this PC and asks to be forgotten too.
    fn on_unpair(&mut self, buf: &[u8], from: SocketAddr) -> Handled {
        let (body, mac) = protocol::split_mac(buf).ok_or("too short")?;
        let mut r = Reader::new(body);
        r.header();
        let client_id: Id = r.array().ok_or("too short")?;

        let (key, name) = match self.store.find(&client_id) {
            Some(c) => (c.key, Some(c.name.clone())),
            // Already gone: the phone repeats UNPAIR until it hears back, so answer again.
            None => (self.recently_unpaired.get(&client_id).ok_or("not paired")?.0, None),
        };
        if !crypto::verify16(&key, body, mac) {
            return Err("bad signature".into());
        }
        let reply = unpaired_packet(&client_id, &key);
        if let Some(name) = name {
            self.store.remove_id(&client_id);
            self.conns.remove(&client_id);
            self.recently_unpaired.insert(client_id, (key, self.platform.now()));
            self.event(format!("\"{name}\" unpaired itself from this PC."));
            self.report_connected();
        }
        self.send(&reply, from);
        Ok(())
    }

    /// Sends what games told the controllers (rumble, lightbar) since the last call.
    pub fn drain_feedback(&mut self) {
        let now = self.platform.now();
        let mut players_changed = false;
        while let Ok((client_id, slot, mut f)) = self.feedback_rx.try_recv() {
            let Some(conn) = self.conns.get_mut(&client_id) else { continue };
            let Some(s) = conn.slots.get_mut(&slot) else { continue };
            match f.player {
                Some(p) => players_changed |= s.feedback.player != Some(p),
                None => f.player = s.feedback.player,
            }
            s.feedback = f;
            if let Some((packet, to)) = status_for(conn, now) {
                self.send(&packet, to);
            }
        }
        if players_changed {
            self.report_connected();
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
        self.recently_unpaired.retain(|_, (_, at)| now - *at < PAIR_TIMEOUT);

        let mut gone = Vec::new();
        let mut detached = Vec::new();
        let mut renumbered = Vec::new();
        let mut statuses = Vec::new();
        for (id, conn) in &mut self.conns {
            let idle = now.saturating_duration_since(conn.last_input);
            if conn.active.is_some() {
                if idle > DROP_AFTER {
                    gone.push(*id);
                    continue;
                }
                if idle > NEUTRAL_AFTER && !conn.neutral {
                    for s in conn.slots.values_mut() {
                        s.pad.update(&PadState::default(), conn.battery);
                    }
                    conn.neutral = true;
                }
                // A controller the phone stopped reporting while it kept sending input was detached.
                let last_input = conn.last_input;
                let missing: Vec<u8> = conn
                    .slots
                    .iter()
                    .filter(|(_, s)| last_input.saturating_duration_since(s.last_seen) > SLOT_GONE_AFTER)
                    .map(|(slot, _)| *slot)
                    .collect();
                for slot in missing {
                    conn.slots.remove(&slot);
                    detached.push(conn.label(slot));
                }
                let name = conn.name.clone();
                for (slot, s) in conn.slots.iter_mut() {
                    let player = s.player();
                    if s.pad.kind() == PadKind::Xbox360 && player != s.announced_player && player > 0 {
                        s.announced_player = player;
                        let label = if *slot == 0 { name.clone() } else { format!("{name} #{}", slot + 1) };
                        renumbered.push((label, player));
                    }
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
        let mut changed = !detached.is_empty() || !renumbered.is_empty();
        for label in detached {
            self.event(format!("\"{label}\" was detached from the phone."));
        }
        for (label, player) in renumbered {
            self.event(format!("\"{label}\" is now player {player}."));
        }
        for id in gone {
            if let Some(conn) = self.conns.remove(&id)
                && !conn.slots.is_empty()
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
                let state = match self.conns.get(&c.id).filter(|conn| !conn.slots.is_empty()) {
                    Some(conn) => {
                        let pads: Vec<String> = conn
                            .slots
                            .iter()
                            .map(|(slot, s)| {
                                let number = if *slot == 0 { String::new() } else { format!("#{} ", slot + 1) };
                                format!("{number}{}", describe(s.pad.kind(), s.player()))
                            })
                            .collect();
                        let mut extra = String::new();
                        if let Some(ms) = conn.rtt_ms {
                            extra += &format!(" · {ms} ms");
                        }
                        if let Some(b) = battery_text(conn.battery) {
                            extra += &format!(" · phone battery {b}");
                        }
                        format!("connected: {}{extra}", pads.join("; "))
                    }
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
            .flat_map(|c| c.slots.iter().map(|(slot, s)| format!("{} ({})", c.label(*slot), describe(s.pad.kind(), s.player()))))
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
        kind::UNPAIR => "UNPAIR".into(),
        _ => format!("packet {k:#04x}"),
    }
}

pub fn describe(kind: PadKind, player: u8) -> String {
    match kind {
        PadKind::Xbox360 if player > 0 => format!("Xbox 360 controller, player {player}"),
        PadKind::Xbox360 => "Xbox 360 controller".into(),
        PadKind::DualShock4 => "PS4 controller with gyro".into(),
    }
}

/// "80%" or "80%, charging" from INPUT's battery byte, None if the phone didn't say.
fn battery_text(battery: u8) -> Option<String> {
    let percent = battery & 0x7F;
    if percent > 100 {
        return None;
    }
    Some(if battery & 0x80 != 0 { format!("{percent}%, charging") } else { format!("{percent}%") })
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

pub(crate) fn unpaired_packet(client_id: &Id, pair_key: &[u8; 32]) -> Vec<u8> {
    Writer::new(kind::UNPAIRED).bytes(client_id).sign(pair_key)
}

pub(crate) fn welcome_packet(hnonce: &Nonce, key: &[u8; 32]) -> Vec<u8> {
    Writer::new(kind::WELCOME).bytes(hnonce).sign(key)
}

/// `pads` holds (slot, feedback, player) per controller.
pub(crate) fn status_packet(seq: u32, ack_seq: u32, ack_age_ms: u16, pads: &[(u8, Feedback, u8)], key: &[u8; 32]) -> Vec<u8> {
    let mut w = Writer::new(kind::STATUS).u32(seq).u32(ack_seq).u16(ack_age_ms).u8(pads.len() as u8);
    for (slot, f, player) in pads {
        let (r, g, b) = f.lightbar;
        w = w.u8(*slot).u8(f.large).u8(f.small).u8(*player).u8(r).u8(g).u8(b);
    }
    w.sign(key)
}

/// The next STATUS for a connection and where to send it, or None without an active session.
fn status_for(conn: &mut Conn, now: Instant) -> Option<(Vec<u8>, SocketAddr)> {
    conn.last_status = now;
    let s = conn.active.as_mut()?;
    s.out_seq += 1;
    let age = now.saturating_duration_since(conn.last_input).as_millis().min(u16::MAX as u128) as u16;
    let pads: Vec<(u8, Feedback, u8)> = conn.slots.iter().map(|(slot, p)| (*slot, p.feedback, p.player())).collect();
    Some((status_packet(s.out_seq, s.last_seq, age, &pads, &s.key), s.addr))
}

#[cfg(test)]
mod tests;
