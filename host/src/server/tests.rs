//! Drives the server through a fake platform: packets are recorded instead of sent, virtual pads
//! are recorded instead of plugged in, and the clock only moves when a test says so.

use std::cell::{Cell, RefCell};
use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use p256::SecretKey;
use p256::ecdh::diffie_hellman;
use rand_core::OsRng;

use super::*;

#[derive(Default)]
struct FakePadState {
    updates: Vec<PadState>,
    rumble: Option<RumbleFn>,
    unplugged: bool,
}

struct FakePad {
    kind: PadKind,
    state: Rc<RefCell<FakePadState>>,
}

impl VirtualPad for FakePad {
    fn kind(&self) -> PadKind {
        self.kind
    }
    fn player(&self) -> u8 {
        1
    }
    fn update(&mut self, state: &PadState) {
        self.state.borrow_mut().updates.push(*state);
    }
}

impl Drop for FakePad {
    fn drop(&mut self) {
        self.state.borrow_mut().unplugged = true;
    }
}

#[derive(Default)]
struct World {
    sent: Vec<(Vec<u8>, SocketAddr)>,
    pads: Vec<(PadKind, Rc<RefCell<FakePadState>>)>,
    plug_fails: bool,
}

struct FakePlatform {
    world: Rc<RefCell<World>>,
    clock: Rc<Cell<Instant>>,
}

impl Platform for FakePlatform {
    fn now(&self) -> Instant {
        self.clock.get()
    }
    fn send(&self, data: &[u8], to: SocketAddr) -> std::io::Result<()> {
        self.world.borrow_mut().sent.push((data.to_vec(), to));
        Ok(())
    }
    fn plug(&self, kind: PadKind, on_rumble: RumbleFn) -> Result<Box<dyn VirtualPad>, String> {
        let mut world = self.world.borrow_mut();
        if world.plug_fails {
            return Err("no driver".into());
        }
        let state = Rc::new(RefCell::new(FakePadState { rumble: Some(on_rumble), ..Default::default() }));
        world.pads.push((kind, state.clone()));
        Ok(Box::new(FakePad { kind, state }))
    }
}

const HOST_ID: Id = [9; 16];

struct Rig {
    server: Server<FakePlatform>,
    world: Rc<RefCell<World>>,
    clock: Rc<Cell<Instant>>,
    out: Arc<Mutex<Vec<Out>>>,
}

impl Rig {
    fn new() -> Rig {
        let world = Rc::new(RefCell::new(World::default()));
        let clock = Rc::new(Cell::new(Instant::now()));
        let out = Arc::new(Mutex::new(Vec::new()));
        let sink = out.clone();
        let platform = FakePlatform { world: world.clone(), clock: clock.clone() };
        let server = Server::new(
            platform,
            Store::in_memory(HOST_ID),
            "TEST-PC".into(),
            Box::new(move |o| sink.lock().unwrap().push(o)),
            true,
        );
        Rig { server, world, clock, out }
    }

    fn advance(&mut self, by: Duration) {
        self.clock.set(self.clock.get() + by);
        self.server.tick();
    }

    /// Feeds a packet in and returns everything the server sent in response.
    fn feed(&mut self, packet: &[u8], from: SocketAddr) -> Vec<(Vec<u8>, SocketAddr)> {
        self.server.handle(packet, from);
        self.take_sent()
    }

    fn take_sent(&mut self) -> Vec<(Vec<u8>, SocketAddr)> {
        std::mem::take(&mut self.world.borrow_mut().sent)
    }

    fn command(&mut self, line: &str) {
        assert!(self.server.command(line));
    }

    fn take_out(&mut self) -> Vec<Out> {
        std::mem::take(&mut self.out.lock().unwrap())
    }

    fn pad(&self, i: usize) -> Rc<RefCell<FakePadState>> {
        self.world.borrow().pads[i].1.clone()
    }

    fn pad_count(&self) -> usize {
        self.world.borrow().pads.len()
    }

    /// Pairs `phone` through the real flow.
    fn pair(&mut self, phone: &mut Phone) {
        let challenge = self.feed(&phone.hello(), phone.addr);
        let code = phone.on_challenge(&challenge[0].0);
        self.feed(&phone.reveal(), phone.addr);
        self.command(&format!("{code:06}"));
        let result = self.take_sent();
        assert!(phone.result_ok(&result[0].0), "pairing failed");
    }

    /// Connects a paired phone and sends its first input, which makes the session active.
    fn connect(&mut self, phone: &mut Phone, kind: PadKind) {
        let welcome = self.feed(&phone.connect(kind), phone.addr);
        phone.on_welcome(&welcome[0].0);
        self.feed(&phone.input(PadState::default()), phone.addr);
    }
}

fn kind_of(packet: &[u8]) -> u8 {
    Reader::new(packet).header().expect("valid header")
}

/// Plays the phone's side of the protocol, using the host's own crypto helpers.
struct Phone {
    id: Id,
    name: String,
    secret: SecretKey,
    public: PubKey,
    addr: SocketAddr,
    pair_key: Option<[u8; 32]>,
    cnonce: Nonce,
    session_key: Option<[u8; 32]>,
    seq: u32,
}

impl Phone {
    fn new(n: u8) -> Phone {
        let secret = SecretKey::random(&mut OsRng);
        Phone {
            id: [n; 16],
            name: format!("Phone {n}"),
            public: crypto::encode(&secret.public_key()),
            secret,
            addr: format!("192.168.1.{n}:5000").parse().unwrap(),
            pair_key: None,
            cnonce: [0; 16],
            session_key: None,
            seq: 0,
        }
    }

    /// A fresh key, as when the user cancels and pairs again.
    fn new_key(&mut self) {
        self.secret = SecretKey::random(&mut OsRng);
        self.public = crypto::encode(&self.secret.public_key());
        self.pair_key = None;
    }

    fn hello(&self) -> Vec<u8> {
        Writer::new(kind::PAIR_HELLO).bytes(&self.id).bytes(&crypto::commitment(&self.public)).string(&self.name).finish()
    }

    fn reveal(&self) -> Vec<u8> {
        Writer::new(kind::PAIR_REVEAL).bytes(&self.id).bytes(&self.public).finish()
    }

    /// Returns the code the phone would show.
    fn on_challenge(&mut self, packet: &[u8]) -> u32 {
        let mut r = Reader::new(packet);
        assert_eq!(r.header(), Some(kind::PAIR_CHALLENGE));
        let host_id: Id = r.array().unwrap();
        let host_pub: PubKey = r.array().unwrap();
        let peer = p256::PublicKey::from_sec1_bytes(&host_pub).unwrap();
        let shared = diffie_hellman(self.secret.to_nonzero_scalar(), peer.as_affine());
        let key = crypto::derive_pair_key(shared.raw_secret_bytes(), &self.id, &host_id, &self.public, &host_pub);
        self.pair_key = Some(key);
        crypto::pair_code(&key)
    }

    fn result_ok(&self, packet: &[u8]) -> bool {
        let mut r = Reader::new(packet);
        r.header() == Some(kind::PAIR_RESULT)
            && r.u8() == Some(1)
            && r.array::<16>() == Some(crypto::pair_ok_mac(&self.pair_key.unwrap()))
    }

    fn connect(&mut self, kind: PadKind) -> Vec<u8> {
        self.cnonce = crypto::random();
        self.session_key = None;
        let pad = match kind {
            PadKind::Xbox360 => 0,
            PadKind::DualShock4 => 1,
        };
        Writer::new(kind::CONNECT).bytes(&self.id).bytes(&self.cnonce).u8(pad).sign(&self.pair_key.unwrap())
    }

    fn on_welcome(&mut self, packet: &[u8]) {
        let (body, mac) = protocol::split_mac(packet).unwrap();
        let mut r = Reader::new(body);
        assert_eq!(r.header(), Some(kind::WELCOME));
        let hnonce: Nonce = r.array().unwrap();
        let key = crypto::session_key(&self.pair_key.unwrap(), &self.cnonce, &hnonce);
        assert!(crypto::verify16(&key, body, mac), "bad WELCOME mac");
        self.session_key = Some(key);
        self.seq = 0;
    }

    fn input(&mut self, s: PadState) -> Vec<u8> {
        self.seq += 1;
        let mut w = Writer::new(kind::INPUT)
            .bytes(&self.id)
            .u32(self.seq)
            .bytes(&s.buttons.to_be_bytes())
            .u8(s.left_trigger)
            .u8(s.right_trigger);
        for v in [s.thumb_lx, s.thumb_ly, s.thumb_rx, s.thumb_ry].iter().chain(&s.gyro).chain(&s.accel) {
            w = w.bytes(&v.to_be_bytes());
        }
        w.sign(&self.session_key.unwrap())
    }

    fn disconnect(&mut self) -> Vec<u8> {
        Writer::new(kind::DISCONNECT).bytes(&self.id).u32(self.seq + 1).sign(&self.session_key.unwrap())
    }

    /// Returns (large motor, small motor, player) from a STATUS signed with the session key.
    fn read_status(&self, packet: &[u8]) -> (u8, u8, u8) {
        let (body, mac) = protocol::split_mac(packet).unwrap();
        assert!(crypto::verify16(&self.session_key.unwrap(), body, mac), "bad STATUS mac");
        let mut r = Reader::new(body);
        assert_eq!(r.header(), Some(kind::STATUS));
        r.u32().unwrap();
        (r.u8().unwrap(), r.u8().unwrap(), r.u8().unwrap())
    }
}

fn pressed_a() -> PadState {
    PadState { buttons: 0x1000, ..Default::default() }
}

#[test]
fn discovery_answers_every_version_and_warns_once_about_old_apps() {
    let mut rig = Rig::new();
    let phone: SocketAddr = "192.168.1.50:4000".parse().unwrap();

    let sent = rig.feed(&Writer::new(kind::DISCOVER).finish(), phone);
    let mut r = Reader::new(&sent[0].0);
    assert_eq!(r.header(), Some(kind::DISCOVER_REPLY));
    assert_eq!(r.array::<16>(), Some(HOST_ID));
    assert_eq!(r.string().as_deref(), Some("TEST-PC"));
    assert_eq!(r.string().as_deref(), Some(VERSION));
    assert!(!rig.take_out().iter().any(|o| matches!(o, Out::Event(_))));

    let old_app: SocketAddr = "192.168.1.60:4000".parse().unwrap();
    let old = Writer::with_version(2, kind::DISCOVER).finish();
    assert_eq!(rig.feed(&old, old_app).len(), 1, "old apps get an answer too, so they can say what's wrong");
    let out = rig.take_out();
    assert!(out.iter().any(|o| matches!(o, Out::Event(e) if e.contains("older RCWLP app (protocol v2"))), "{out:?}");
    rig.feed(&old, old_app);
    assert!(!rig.take_out().iter().any(|o| matches!(o, Out::Event(_))), "warned only once per address");
}

#[test]
fn a_current_app_asking_in_both_versions_isnt_called_outdated() {
    let mut rig = Rig::new();
    let phone: SocketAddr = "192.168.1.51:4000".parse().unwrap();
    // What the app sends: its own version first, then v2 to recognize old hosts.
    rig.feed(&Writer::new(kind::DISCOVER).finish(), phone);
    rig.feed(&Writer::with_version(2, kind::DISCOVER).finish(), phone);
    assert!(!rig.take_out().iter().any(|o| matches!(o, Out::Event(_))));
}

#[test]
fn other_versions_are_ignored_with_a_reason() {
    let mut rig = Rig::new();
    let packet = Writer::with_version(2, kind::CONNECT).bytes(&[0; 33]).finish();
    assert!(rig.feed(&packet, "192.168.1.2:1".parse().unwrap()).is_empty());
    let out = rig.take_out();
    assert!(out.iter().any(|o| matches!(o, Out::Debug(d) if d.contains("ignored CONNECT: it's protocol v2"))), "{out:?}");
}

#[test]
fn pairs_only_with_the_code_the_phone_shows() {
    let mut rig = Rig::new();
    let mut phone = Phone::new(1);

    let sent = rig.feed(&phone.hello(), phone.addr);
    assert_eq!(kind_of(&sent[0].0), kind::PAIR_CHALLENGE);
    assert!(rig.take_out().contains(&Out::Pairing(Some("Phone 1".into()))));
    let code = phone.on_challenge(&sent[0].0);

    rig.command(&format!("{code:06}"));
    assert!(rig.take_out().iter().any(|o| matches!(o, Out::Reply(r) if r.contains("hasn't finished"))));

    assert!(rig.feed(&phone.reveal(), phone.addr).is_empty(), "no result until the code is typed");
    rig.command("y");
    rig.command(&format!("{:06}", (code + 1) % 1_000_000));
    assert!(rig.take_sent().is_empty());
    assert_eq!(rig.server.paired_count(), 0);

    let typed = format!("{:03} {:03}", code / 1000, code % 1000);
    rig.command(&typed);
    let sent = rig.take_sent();
    assert!(phone.result_ok(&sent[0].0));
    assert_eq!(rig.server.paired_count(), 1);
    assert!(rig.take_out().contains(&Out::Pairing(None)));

    // A late reveal or hello from the same attempt gets the result again.
    assert!(phone.result_ok(&rig.feed(&phone.reveal(), phone.addr)[0].0));
    assert!(phone.result_ok(&rig.feed(&phone.hello(), phone.addr)[0].0));
}

#[test]
fn rejecting_tells_the_phone() {
    let mut rig = Rig::new();
    let mut phone = Phone::new(1);
    let sent = rig.feed(&phone.hello(), phone.addr);
    phone.on_challenge(&sent[0].0);
    rig.command("n");
    let sent = rig.take_sent();
    let mut r = Reader::new(&sent[0].0);
    assert_eq!(r.header(), Some(kind::PAIR_RESULT));
    assert_eq!(r.u8(), Some(0));
    assert_eq!(rig.server.paired_count(), 0);
}

#[test]
fn a_reveal_that_doesnt_match_the_commitment_is_ignored() {
    let mut rig = Rig::new();
    let mut phone = Phone::new(1);
    let sent = rig.feed(&phone.hello(), phone.addr);
    let code = phone.on_challenge(&sent[0].0);
    // Someone else's key, sent under this phone's id.
    let mut impostor = Phone::new(1);
    impostor.new_key();
    rig.feed(&impostor.reveal(), phone.addr);
    rig.command(&format!("{code:06}"));
    assert!(rig.take_out().iter().any(|o| matches!(o, Out::Reply(r) if r.contains("hasn't finished"))));
}

#[test]
fn the_same_phone_can_start_over_but_not_instantly() {
    let mut rig = Rig::new();
    let mut phone = Phone::new(1);
    let first = rig.feed(&phone.hello(), phone.addr);

    phone.new_key();
    rig.advance(Duration::from_secs(1));
    assert!(rig.feed(&phone.hello(), phone.addr).is_empty(), "too soon");

    rig.advance(Duration::from_secs(3));
    let second = rig.feed(&phone.hello(), phone.addr);
    assert_eq!(kind_of(&second[0].0), kind::PAIR_CHALLENGE);
    assert_ne!(first[0].0, second[0].0, "a new host key for the new attempt");
    assert!(rig.take_out().iter().any(|o| matches!(o, Out::Event(e) if e.contains("started pairing again"))));

    let code = phone.on_challenge(&second[0].0);
    rig.feed(&phone.reveal(), phone.addr);
    rig.command(&format!("{code:06}"));
    assert_eq!(rig.server.paired_count(), 1);
}

#[test]
fn another_phone_waits_until_the_request_is_done() {
    let mut rig = Rig::new();
    let a = Phone::new(1);
    let mut b = Phone::new(2);
    rig.feed(&a.hello(), a.addr);
    rig.advance(Duration::from_secs(10));
    assert!(rig.feed(&b.hello(), b.addr).is_empty());

    rig.advance(PAIR_TIMEOUT);
    assert!(rig.take_out().iter().any(|o| matches!(o, Out::Event(e) if e.contains("expired"))));
    let sent = rig.feed(&b.hello(), b.addr);
    b.on_challenge(&sent[0].0);
}

#[test]
fn unpaired_phones_are_rejected() {
    let mut rig = Rig::new();
    let mut phone = Phone::new(1);
    phone.pair_key = Some([1; 32]);
    let sent = rig.feed(&phone.connect(PadKind::Xbox360), phone.addr);
    assert_eq!(kind_of(&sent[0].0), kind::CONNECT_REJECT);
}

#[test]
fn connects_streams_and_reports_status() {
    let mut rig = Rig::new();
    let mut phone = Phone::new(1);
    rig.pair(&mut phone);
    rig.take_out();

    let welcome = rig.feed(&phone.connect(PadKind::Xbox360), phone.addr);
    phone.on_welcome(&welcome[0].0);
    assert_eq!(rig.pad_count(), 0, "no pad until the phone proves the session with input");

    let sent = rig.feed(&phone.input(pressed_a()), phone.addr);
    assert_eq!(rig.pad_count(), 1);
    assert_eq!(rig.pad(0).borrow().updates, vec![pressed_a()]);
    assert_eq!(phone.read_status(&sent[0].0), (0, 0, 1));
    let out = rig.take_out();
    assert!(out.contains(&Out::Connected(vec!["Phone 1 (Xbox 360 controller, player 1)".into()])), "{out:?}");

    // Heartbeat.
    rig.clock.set(rig.clock.get() + STATUS_EVERY);
    rig.server.tick();
    assert_eq!(phone.read_status(&rig.take_sent()[0].0), (0, 0, 1));
}

#[test]
fn replayed_input_is_dropped() {
    let mut rig = Rig::new();
    let mut phone = Phone::new(1);
    rig.pair(&mut phone);
    rig.connect(&mut phone, PadKind::Xbox360);
    let packet = phone.input(pressed_a());
    rig.feed(&packet, phone.addr);
    rig.feed(&packet, phone.addr);
    assert_eq!(rig.pad(0).borrow().updates.len(), 2, "the first input plus one copy of the replayed one");
}

#[test]
fn a_replayed_connect_cant_kick_a_phone_in_use() {
    let mut rig = Rig::new();
    let mut phone = Phone::new(1);
    rig.pair(&mut phone);
    let old_connect = phone.connect(PadKind::Xbox360);
    rig.feed(&old_connect, phone.addr);
    // The phone reconnects (say, after a Wi-Fi blip), then someone replays its old CONNECT.
    rig.connect(&mut phone, PadKind::Xbox360);
    let attacker: SocketAddr = "192.168.1.66:6666".parse().unwrap();
    rig.feed(&old_connect, attacker);

    rig.feed(&phone.input(pressed_a()), phone.addr);
    assert_eq!(rig.pad(0).borrow().updates.last(), Some(&pressed_a()), "the real session still works");
    assert_eq!(rig.pad_count(), 1);
}

#[test]
fn goes_neutral_then_times_out() {
    let mut rig = Rig::new();
    let mut phone = Phone::new(1);
    rig.pair(&mut phone);
    rig.connect(&mut phone, PadKind::Xbox360);
    rig.feed(&phone.input(pressed_a()), phone.addr);

    rig.advance(NEUTRAL_AFTER + Duration::from_millis(10));
    assert_eq!(rig.pad(0).borrow().updates.last(), Some(&PadState::default()), "buttons released");
    assert!(!rig.pad(0).borrow().unplugged);

    rig.advance(DROP_AFTER);
    assert!(rig.pad(0).borrow().unplugged);
    let out = rig.take_out();
    assert!(out.iter().any(|o| matches!(o, Out::Event(e) if e.contains("timed out"))), "{out:?}");
    assert!(out.contains(&Out::Connected(vec![])));
}

#[test]
fn rumble_reaches_the_phone() {
    let mut rig = Rig::new();
    let mut phone = Phone::new(1);
    rig.pair(&mut phone);
    rig.connect(&mut phone, PadKind::Xbox360);
    rig.take_sent();

    (rig.pad(0).borrow_mut().rumble.as_mut().unwrap())(200, 50);
    rig.server.drain_rumble();
    assert_eq!(phone.read_status(&rig.take_sent()[0].0), (200, 50, 1));
}

#[test]
fn switching_controller_type_replaces_the_pad() {
    let mut rig = Rig::new();
    let mut phone = Phone::new(1);
    rig.pair(&mut phone);
    rig.connect(&mut phone, PadKind::Xbox360);
    rig.connect(&mut phone, PadKind::DualShock4);
    assert_eq!(rig.pad_count(), 2);
    assert!(rig.pad(0).borrow().unplugged);
    assert_eq!(rig.world.borrow().pads[1].0, PadKind::DualShock4);
}

#[test]
fn disconnect_unplugs() {
    let mut rig = Rig::new();
    let mut phone = Phone::new(1);
    rig.pair(&mut phone);
    rig.connect(&mut phone, PadKind::Xbox360);
    rig.feed(&phone.disconnect(), phone.addr);
    assert!(rig.pad(0).borrow().unplugged);
}

#[test]
fn forget_unpairs_and_unplugs() {
    let mut rig = Rig::new();
    let mut phone = Phone::new(1);
    rig.pair(&mut phone);
    rig.connect(&mut phone, PadKind::Xbox360);
    rig.command("forget 1");
    assert!(rig.pad(0).borrow().unplugged);
    assert_eq!(rig.server.paired_count(), 0);
    let sent = rig.feed(&phone.connect(PadKind::Xbox360), phone.addr);
    assert_eq!(kind_of(&sent[0].0), kind::CONNECT_REJECT);
}

#[test]
fn a_missing_driver_is_reported() {
    let mut rig = Rig::new();
    let mut phone = Phone::new(1);
    rig.pair(&mut phone);
    rig.world.borrow_mut().plug_fails = true;
    rig.take_out();
    rig.connect(&mut phone, PadKind::Xbox360);
    let out = rig.take_out();
    assert!(out.iter().any(|o| matches!(o, Out::Event(e) if e.contains("Could not create a virtual controller"))), "{out:?}");
}

#[test]
fn reads_typed_codes() {
    assert_eq!(parse_code("123456"), Some(123456));
    assert_eq!(parse_code("012 345"), Some(12345));
    assert_eq!(parse_code(" 98 76 54 "), Some(987654));
    for bad in ["", "12345", "1234567", "12345a", "forget 1", "y"] {
        assert_eq!(parse_code(bad), None, "{bad:?}");
    }
}
