//! RCWLP host: turns paired Android phones (with a controller attached) into virtual
//! Xbox 360 controllers on this PC. No video, no audio, just input.

mod crypto;
mod pad;
mod protocol;
mod store;

use std::collections::HashMap;
use std::io::{self, BufRead};
use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crypto::HostKey;
use pad::Pad;
use protocol::{Commitment, Id, Nonce, PadKind, PadState, PubKey, Reader, Writer, kind};
use store::{PairedClient, Store};

#[cfg(test)]
mod vectors;

/// Upper bound on how long the loop sleeps waiting for a packet.
const TICK: Duration = Duration::from_millis(20);
/// Release all buttons if a phone goes quiet, so nothing stays held down.
const NEUTRAL_AFTER: Duration = Duration::from_secs(1);
/// Unplug the virtual controller after this long without input.
const DROP_AFTER: Duration = Duration::from_secs(15);
/// Status packets double as the host heartbeat and resend lost rumble changes.
const STATUS_EVERY: Duration = Duration::from_millis(250);
const PAIR_TIMEOUT: Duration = Duration::from_secs(60);
/// A phone that starts pairing over may replace its own unanswered request, but no sooner than
/// this, so nobody can cycle through requests hoping for one whose code matches the phone's.
const PAIR_REPLACE_AFTER: Duration = Duration::from_secs(3);

const REJECT_NOT_PAIRED: u8 = 1;

/// (client, large motor, small motor) sent from a driver notification thread.
type RumbleEvent = (Id, u8, u8);

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

impl Session {
    fn welcome(&self) -> Vec<u8> {
        welcome_packet(&self.hnonce, &self.key)
    }
}

struct Conn {
    name: String,
    pad: Option<Pad>,
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
    fn new(name: String) -> Conn {
        let now = Instant::now();
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

struct PairRequest {
    client_id: Id,
    /// The phone's public key stays hidden behind this until it has seen ours.
    commitment: Commitment,
    name: String,
    addr: SocketAddr,
    host_key: HostKey,
    /// Set once the phone reveals the public key it committed to.
    pair_key: Option<[u8; 32]>,
    created: Instant,
    decision: Option<bool>,
}

struct Server {
    socket: UdpSocket,
    host_name: String,
    store: Store,
    conns: HashMap<Id, Conn>,
    pairing: Option<PairRequest>,
    rumble_tx: Sender<RumbleEvent>,
    rumble_rx: Receiver<RumbleEvent>,
    /// Rumble arrives on driver threads, which poke the socket through this so the loop
    /// handles it right away instead of after its receive timeout.
    waker: Arc<UdpSocket>,
}

fn main() {
    let port = parse_port();
    if let Err(e) = pad::check_driver() {
        eprintln!("Cannot reach the ViGEmBus driver ({e}).");
        eprintln!("Install it from https://github.com/nefarius/ViGEmBus/releases and try again.");
        std::process::exit(1);
    }
    let socket = UdpSocket::bind(("0.0.0.0", port)).unwrap_or_else(|e| {
        eprintln!("Cannot listen on UDP port {port}: {e}");
        std::process::exit(1);
    });
    socket.set_read_timeout(Some(TICK)).expect("set socket timeout");
    let waker = UdpSocket::bind(("127.0.0.1", 0))
        .and_then(|w| w.connect(("127.0.0.1", port)).map(|()| w))
        .expect("create loopback socket");

    let (rumble_tx, rumble_rx) = mpsc::channel();
    let mut server = Server {
        socket,
        host_name: std::env::var("COMPUTERNAME").unwrap_or_else(|_| "PC".into()),
        store: Store::load(),
        conns: HashMap::new(),
        pairing: None,
        rumble_tx,
        rumble_rx,
        waker: Arc::new(waker),
    };
    let console = spawn_console();

    println!(
        "RCWLP host \"{}\" listening on UDP port {port}. {} paired phone(s).",
        server.host_name,
        server.store.clients.len()
    );
    print_help();

    let mut buf = [0u8; 1500];
    loop {
        match server.socket.recv_from(&mut buf) {
            Ok((n, addr)) => server.handle(&buf[..n], addr),
            Err(e) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            // Windows surfaces ICMP "port unreachable" from an earlier send this way; harmless.
            Err(e) if e.kind() == io::ErrorKind::ConnectionReset => {}
            Err(e) => eprintln!("Receive error: {e}"),
        }
        while let Ok(event) = server.rumble_rx.try_recv() {
            server.on_rumble(event);
        }
        while let Ok(line) = console.try_recv() {
            if !server.command(line.trim()) {
                return;
            }
        }
        server.tick();
    }
}

impl Server {
    fn handle(&mut self, buf: &[u8], addr: SocketAddr) {
        let Some(k) = Reader::new(buf).header() else { return };
        let _ = match k {
            kind::DISCOVER => self.on_discover(addr),
            kind::PAIR_HELLO => self.on_pair_hello(buf, addr),
            kind::PAIR_REVEAL => self.on_pair_reveal(buf, addr),
            kind::CONNECT => self.on_connect(buf, addr),
            kind::INPUT => self.on_input(buf, addr),
            kind::DISCONNECT => self.on_disconnect(buf),
            _ => None,
        };
    }

    fn on_discover(&mut self, addr: SocketAddr) -> Option<()> {
        let reply = Writer::new(kind::DISCOVER_REPLY)
            .bytes(&self.store.host_id)
            .string(&self.host_name)
            .finish();
        send(&self.socket, &reply, addr);
        Some(())
    }

    fn on_pair_hello(&mut self, buf: &[u8], addr: SocketAddr) -> Option<()> {
        let mut r = Reader::new(buf);
        r.header()?;
        let client_id: Id = r.array()?;
        let commitment: Commitment = r.array()?;
        let name = r.string()?;

        let mut replacing = false;
        if let Some(p) = &mut self.pairing {
            // The phone retries its hello until it gets an answer; repeat ours.
            if p.client_id == client_id && p.commitment == commitment {
                p.addr = addr;
                send(&self.socket, &pair_reply(p, &self.store.host_id), addr);
                return Some(());
            }
            if p.decision.is_none() && p.created.elapsed() < PAIR_TIMEOUT {
                // A new key from the same phone means it started over (say, after Cancel).
                if p.client_id != client_id || p.created.elapsed() < PAIR_REPLACE_AFTER {
                    return None; // Busy with another phone, or too soon.
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
            created: Instant::now(),
            decision: None,
        };
        println!();
        if replacing {
            println!("\"{}\" started pairing again, so it shows a new code now.", p.name);
        } else {
            println!("Pairing request from \"{}\" at {}.", p.name, addr.ip());
        }
        println!("  To accept, type the 6-digit code shown on the phone. Type n to reject.");
        send(&self.socket, &pair_reply(&p, &self.store.host_id), addr);
        self.pairing = Some(p);
        Some(())
    }

    /// The phone's second step: the public key behind its hello's commitment.
    fn on_pair_reveal(&mut self, buf: &[u8], addr: SocketAddr) -> Option<()> {
        let mut r = Reader::new(buf);
        r.header()?;
        let client_id: Id = r.array()?;
        let client_pub: PubKey = r.array()?;

        let p = self.pairing.as_mut().filter(|p| p.client_id == client_id)?;
        if crypto::commitment(&client_pub) != p.commitment {
            return None;
        }
        if p.pair_key.is_none() {
            p.pair_key = Some(p.host_key.pair_key(&client_id, &self.store.host_id, &client_pub)?);
        }
        p.addr = addr;
        // The phone keeps revealing until it hears the result.
        if p.decision.is_some() {
            send(&self.socket, &pair_reply(p, &self.store.host_id), addr);
        }
        Some(())
    }

    fn accept(&mut self, code: u32) {
        let Some(p) = waiting(&mut self.pairing) else {
            println!("No pairing request is waiting.");
            return;
        };
        let Some(pair_key) = p.pair_key else {
            println!("The phone hasn't finished its part yet. Type the code again in a moment.");
            return;
        };
        if crypto::pair_code(&pair_key) != code {
            println!("That isn't the code for \"{}\". Check the phone and type it again, or n to reject.", p.name);
            return;
        }
        p.decision = Some(true);
        self.store.upsert(PairedClient { id: p.client_id, name: p.name.clone(), key: pair_key });
        // Any live connection was keyed with the old pairing.
        self.conns.remove(&p.client_id);
        println!("Paired with \"{}\".", p.name);
        send(&self.socket, &pair_reply(p, &self.store.host_id), p.addr);
    }

    fn reject(&mut self) {
        let Some(p) = waiting(&mut self.pairing) else {
            println!("No pairing request is waiting.");
            return;
        };
        p.decision = Some(false);
        println!("Rejected \"{}\".", p.name);
        send(&self.socket, &pair_reply(p, &self.store.host_id), p.addr);
    }

    fn on_connect(&mut self, buf: &[u8], addr: SocketAddr) -> Option<()> {
        let (body, mac) = protocol::split_mac(buf)?;
        let mut r = Reader::new(body);
        r.header()?;
        let client_id: Id = r.array()?;
        let cnonce: Nonce = r.array()?;
        let pad_kind = PadKind::from_wire(r.u8()?)?;

        let Some(paired) = self.store.find(&client_id).filter(|c| crypto::verify16(&c.key, body, mac)) else {
            let reject = Writer::new(kind::CONNECT_REJECT).u8(REJECT_NOT_PAIRED).finish();
            send(&self.socket, &reject, addr);
            return None;
        };
        let pair_key = paired.key;
        let name = paired.name.clone();
        let conn = self.conns.entry(client_id).or_insert_with(|| Conn::new(name));

        // A retransmitted CONNECT gets the same WELCOME back.
        if let Some(s) = conn.active.iter_mut().chain(conn.pending.iter_mut()).find(|s| s.cnonce == cnonce) {
            s.addr = addr;
            send(&self.socket, &s.welcome(), addr);
            return Some(());
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
            created: Instant::now(),
        };
        send(&self.socket, &session.welcome(), addr);
        conn.pending = Some(session);
        Some(())
    }

    fn on_input(&mut self, buf: &[u8], addr: SocketAddr) -> Option<()> {
        let (body, mac) = protocol::split_mac(buf)?;
        let mut r = Reader::new(body);
        r.header()?;
        let client_id: Id = r.array()?;
        let seq = r.u32()?;
        let state = PadState::read(&mut r)?;

        let conn = self.conns.get_mut(&client_id)?;
        let promoted = if conn.active.as_ref().is_some_and(|s| crypto::verify16(&s.key, body, mac)) {
            false
        } else if conn.pending.as_ref().is_some_and(|s| crypto::verify16(&s.key, body, mac)) {
            conn.active = conn.pending.take();
            true
        } else {
            return None;
        };

        let session = conn.active.as_mut()?;
        if seq <= session.last_seq {
            return None; // Replayed or reordered.
        }
        session.last_seq = seq;
        session.addr = addr;
        let pad_kind = session.kind;

        // The phone switched controller type since its last session: swap the virtual pad.
        if conn.pad.as_ref().is_some_and(|p| p.kind != pad_kind) {
            conn.pad = None;
        }
        if conn.pad.is_none() {
            let (rumble, waker) = (self.rumble_tx.clone(), self.waker.clone());
            let on_rumble = move |large, small| {
                if rumble.send((client_id, large, small)).is_ok() {
                    let _ = waker.send(&[]);
                }
            };
            match Pad::plug(pad_kind, on_rumble) {
                Ok(pad) => {
                    println!("\"{}\" connected as {}.", conn.name, describe(&pad));
                    conn.pad = Some(pad);
                }
                Err(e) => {
                    eprintln!("Could not create a virtual controller for \"{}\": {e}", conn.name);
                    self.conns.remove(&client_id);
                    return None;
                }
            }
        } else if promoted {
            println!("\"{}\" reconnected.", conn.name);
        }

        conn.pad.as_mut()?.update(&state);
        conn.last_input = Instant::now();
        conn.neutral = false;
        if promoted {
            send_status(&self.socket, conn);
        }
        Some(())
    }

    fn on_disconnect(&mut self, buf: &[u8]) -> Option<()> {
        let (body, mac) = protocol::split_mac(buf)?;
        let mut r = Reader::new(body);
        r.header()?;
        let client_id: Id = r.array()?;
        let seq = r.u32()?;

        let session = self.conns.get(&client_id)?.active.as_ref()?;
        if !crypto::verify16(&session.key, body, mac) || seq <= session.last_seq {
            return None;
        }
        let conn = self.conns.remove(&client_id)?;
        println!("\"{}\" disconnected.", conn.name);
        Some(())
    }

    fn on_rumble(&mut self, (client_id, large, small): RumbleEvent) {
        if let Some(conn) = self.conns.get_mut(&client_id) {
            conn.rumble = (large, small);
            send_status(&self.socket, conn);
        }
    }

    fn tick(&mut self) {
        let now = Instant::now();

        if let Some(p) = &self.pairing
            && now - p.created > PAIR_TIMEOUT
        {
            if p.decision.is_none() {
                println!("Pairing request from \"{}\" expired.", p.name);
            }
            self.pairing = None;
        }

        let mut gone = Vec::new();
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
                    send_status(&self.socket, conn);
                }
            } else if conn.pending.as_ref().is_none_or(|s| now - s.created > DROP_AFTER) {
                gone.push(*id);
            }
        }
        for id in gone {
            if let Some(conn) = self.conns.remove(&id)
                && conn.pad.is_some()
            {
                println!("\"{}\" timed out.", conn.name);
            }
        }
    }

    /// Returns false when the user asked to quit.
    fn command(&mut self, line: &str) -> bool {
        if let Some(code) = parse_code(line) {
            self.accept(code);
            return true;
        }
        let mut words = line.split_whitespace();
        match (words.next(), words.next()) {
            (None, _) => {}
            // Typing the code makes sure it was compared, which "y" never did.
            (Some("y" | "yes"), None) => println!("Type the 6-digit code shown on the phone to accept it."),
            (Some("n" | "no"), None) => self.reject(),
            (Some("list"), None) => self.list(),
            (Some("forget"), Some(n)) => match n.parse::<usize>().ok().and_then(|n| n.checked_sub(1)) {
                Some(i) => match self.store.remove(i) {
                    Some(c) => {
                        self.conns.remove(&c.id);
                        println!("Forgot \"{}\".", c.name);
                    }
                    None => println!("No phone #{n}. Type list to see them."),
                },
                None => println!("Usage: forget <number from list>"),
            },
            (Some("quit" | "exit"), None) => return false,
            _ => print_help(),
        }
        true
    }

    fn list(&self) {
        if self.store.clients.is_empty() {
            println!("No paired phones. Open the app on your phone and pick this PC to pair.");
        }
        for (i, c) in self.store.clients.iter().enumerate() {
            let state = match self.conns.get(&c.id).and_then(|conn| conn.pad.as_ref()) {
                Some(pad) => format!("connected as {}", describe(pad)),
                None => "not connected".into(),
            };
            println!("  {}. {} ({state})", i + 1, c.name);
        }
    }
}

fn describe(pad: &Pad) -> String {
    match pad.kind {
        PadKind::Xbox360 if pad.player > 0 => format!("Xbox 360 controller, player {}", pad.player),
        PadKind::Xbox360 => "Xbox 360 controller".into(),
        PadKind::DualShock4 => "PS4 controller with gyro".into(),
    }
}

/// The pairing request still waiting for a decision, if it hasn't expired.
fn waiting(pairing: &mut Option<PairRequest>) -> Option<&mut PairRequest> {
    pairing.as_mut().filter(|p| p.decision.is_none() && p.created.elapsed() <= PAIR_TIMEOUT)
}

/// Reads a typed pairing code: "123456" or "123 456", as the phone shows it.
fn parse_code(line: &str) -> Option<u32> {
    let digits: String = line.split_whitespace().collect();
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn pair_reply(p: &PairRequest, host_id: &Id) -> Vec<u8> {
    match (p.decision, p.pair_key) {
        (None, _) => Writer::new(kind::PAIR_CHALLENGE).bytes(host_id).bytes(&p.host_key.public).finish(),
        (Some(true), Some(key)) => Writer::new(kind::PAIR_RESULT).u8(1).bytes(&crypto::pair_ok_mac(&key)).finish(),
        _ => Writer::new(kind::PAIR_RESULT).u8(0).bytes(&[0; protocol::MAC_LEN]).finish(),
    }
}

fn welcome_packet(hnonce: &Nonce, key: &[u8; 32]) -> Vec<u8> {
    Writer::new(kind::WELCOME).bytes(hnonce).sign(key)
}

fn status_packet(seq: u32, (large, small): (u8, u8), player: u8, key: &[u8; 32]) -> Vec<u8> {
    Writer::new(kind::STATUS).u32(seq).u8(large).u8(small).u8(player).sign(key)
}

fn send_status(socket: &UdpSocket, conn: &mut Conn) {
    conn.last_status = Instant::now();
    let Some(s) = conn.active.as_mut() else { return };
    s.out_seq += 1;
    let player = conn.pad.as_ref().map_or(0, |p| p.player);
    send(socket, &status_packet(s.out_seq, conn.rumble, player, &s.key), s.addr);
}

fn send(socket: &UdpSocket, data: &[u8], addr: SocketAddr) {
    if let Err(e) = socket.send_to(data, addr) {
        eprintln!("Send to {addr} failed: {e}");
    }
}

fn spawn_console() -> Receiver<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

fn print_help() {
    println!("Commands: list | forget <n> | quit   (to pair, type the code the phone shows, or n to reject)");
}

fn parse_port() -> u16 {
    let args: Vec<String> = std::env::args().collect();
    match args.iter().position(|a| a == "--port") {
        Some(i) => args.get(i + 1).and_then(|p| p.parse().ok()).unwrap_or_else(|| {
            eprintln!("Usage: rcwlp-host [--port <udp port>]");
            std::process::exit(2);
        }),
        None => protocol::DEFAULT_PORT,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_typed_codes() {
        assert_eq!(parse_code("123456"), Some(123456));
        assert_eq!(parse_code("012 345"), Some(12345));
        assert_eq!(parse_code(" 98 76 54 "), Some(987654));
        for bad in ["", "12345", "1234567", "12345a", "forget 1", "y"] {
            assert_eq!(parse_code(bad), None, "{bad:?}");
        }
    }
}
