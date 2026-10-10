//! Pretends to be the Android app, to test the host without a phone.
//!
//!   cargo run --example fake_phone -- [host ip] [--wiggle] [--ds4]
//!
//! Pairs (type the code it prints into the host window), connects, and sends input for 10
//! seconds: a neutral pad by default, or a slowly circling left stick (and, with `--ds4`, a slow
//! yaw rotation on the gyro) with `--wiggle`. `--ds4` asks for a PS4 controller instead of Xbox.
//!
//! `--discover` only asks who's there, the way the app does (in this version and in v2), and
//! prints each host with the protocol version it answered in. Use a broadcast address like
//! 192.168.1.255 to ask the whole network.

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use p256::ecdh::diffie_hellman;
use p256::{PublicKey, SecretKey};
use rcwlp_host::protocol::{self, Id, PubKey, Reader, Writer, kind};
use rcwlp_host::crypto;
use rand_core::OsRng;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let wiggle = args.iter().any(|a| a == "--wiggle");
    let ds4 = args.iter().any(|a| a == "--ds4");
    let ip = args.iter().find(|a| !a.starts_with("--")).map_or("127.0.0.1", String::as_str);
    let host: SocketAddr = format!("{ip}:{}", std::env::var("RCWLP_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(protocol::DEFAULT_PORT)).parse().expect("host ip");
    let socket = UdpSocket::bind("0.0.0.0:0").unwrap();
    socket.set_read_timeout(Some(Duration::from_millis(200))).unwrap();
    let mut buf = [0u8; 1500];

    if args.iter().any(|a| a == "--discover") {
        return discover(&socket, host);
    }

    // Discover.
    let host_id: Id = loop {
        socket.send_to(&Writer::new(kind::DISCOVER).finish(), host).unwrap();
        if let Ok((n, _)) = socket.recv_from(&mut buf) {
            let mut r = Reader::new(&buf[..n]);
            if r.header() == Some(kind::DISCOVER_REPLY) {
                let id = r.array().unwrap();
                println!("Found host \"{}\"", r.string().unwrap());
                break id;
            }
        }
    };

    // Pair: commit to our key in the hello, reveal it once the host has sent its own.
    let client_id: Id = crypto::random();
    let secret = SecretKey::random(&mut OsRng);
    let client_pub = crypto::encode(&secret.public_key());
    let hello = Writer::new(kind::PAIR_HELLO)
        .bytes(&client_id)
        .bytes(&crypto::commitment(&client_pub))
        .string("fake_phone")
        .finish();
    let reveal = Writer::new(kind::PAIR_REVEAL).bytes(&client_id).bytes(&client_pub).finish();
    let mut pair_key = None;
    let deadline = Instant::now() + Duration::from_secs(90);
    let pair_key = loop {
        assert!(Instant::now() < deadline, "pairing timed out");
        socket.send_to(if pair_key.is_none() { &hello } else { &reveal }, host).unwrap();
        let Ok((n, _)) = socket.recv_from(&mut buf) else { continue };
        let mut r = Reader::new(&buf[..n]);
        match r.header() {
            Some(kind::PAIR_CHALLENGE) if pair_key.is_none() => {
                let hid: Id = r.array().unwrap();
                assert_eq!(hid, host_id, "challenge from a different host");
                let host_pub: PubKey = r.array().unwrap();
                let peer = PublicKey::from_sec1_bytes(&host_pub).unwrap();
                let shared = diffie_hellman(secret.to_nonzero_scalar(), peer.as_affine());
                let key = crypto::derive_pair_key(shared.raw_secret_bytes(), &client_id, &hid, &client_pub, &host_pub);
                let code = crypto::pair_code(&key);
                println!("Code: {:03} {:03}  (type it in the host window)", code / 1000, code % 1000);
                pair_key = Some(key);
                continue; // Reveal right away.
            }
            Some(kind::PAIR_RESULT) => {
                assert_eq!(r.u8(), Some(1), "host rejected pairing");
                let key = pair_key.expect("result before challenge");
                assert_eq!(r.array::<16>().unwrap(), crypto::pair_ok_mac(&key), "bad pair-ok mac");
                println!("Paired");
                break key;
            }
            _ => {}
        }
        std::thread::sleep(Duration::from_millis(500));
    };

    // Connect.
    let cnonce = crypto::random();
    let connect = Writer::new(kind::CONNECT).bytes(&client_id).bytes(&cnonce).u8(ds4 as u8).sign(&pair_key);
    let session_key = loop {
        socket.send_to(&connect, host).unwrap();
        let Ok((n, _)) = socket.recv_from(&mut buf) else { continue };
        let Some((body, mac)) = protocol::split_mac(&buf[..n]) else { continue };
        let mut r = Reader::new(body);
        if r.header() != Some(kind::WELCOME) {
            continue;
        }
        let hnonce = r.array().unwrap();
        let key = crypto::session_key(&pair_key, &cnonce, &hnonce);
        assert!(crypto::verify16(&key, body, mac), "bad WELCOME mac");
        println!("Welcomed");
        break key;
    };

    // Stream input and watch STATUS.
    socket.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
    let start = Instant::now();
    let mut seq = 0u32;
    let mut statuses = 0;
    let mut player = 0;
    while start.elapsed() < Duration::from_secs(10) {
        seq += 1;
        let (lx, ly) = if wiggle {
            let t = start.elapsed().as_secs_f32();
            ((t.cos() * 20000.0) as i16, (t.sin() * 20000.0) as i16)
        } else {
            (0, 0)
        };
        let mut w = Writer::new(kind::INPUT).bytes(&client_id).u32(seq);
        w = w.bytes(&0u16.to_be_bytes()).u8(0).u8(0);
        // Gyro in DS4 units (16 per deg/s): 45 deg/s of yaw while wiggling. Accel: 1 g up, at rest.
        let yaw = if wiggle { 45 * 16 } else { 0 };
        for v in [lx, ly, 0, 0, 0, yaw, 0, 0, 8192, 0] {
            w = w.bytes(&v.to_be_bytes());
        }
        socket.send_to(&w.sign(&session_key), host).unwrap();

        if let Ok((n, _)) = socket.recv_from(&mut buf)
            && let Some((body, mac)) = protocol::split_mac(&buf[..n])
            && crypto::verify16(&session_key, body, mac)
        {
            let mut r = Reader::new(body);
            if r.header() == Some(kind::STATUS) {
                let (_seq, _large, _small) = (r.u32(), r.u8(), r.u8());
                player = r.u8().unwrap();
                statuses += 1;
            }
        }
    }
    let disconnect = Writer::new(kind::DISCONNECT).bytes(&client_id).u32(seq + 1).sign(&session_key);
    socket.send_to(&disconnect, host).unwrap();
    println!("Sent {seq} input packets, got {statuses} verified STATUS packets, player {player}");
    assert!(statuses > 0, "no STATUS from host");
}

/// Asks like the app does and prints every answer for two seconds.
fn discover(socket: &UdpSocket, to: SocketAddr) {
    socket.set_broadcast(true).unwrap();
    for version in [protocol::VERSION, 2] {
        socket.send_to(&Writer::with_version(version, kind::DISCOVER).finish(), to).unwrap();
    }
    let mut buf = [0u8; 1500];
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut seen = std::collections::HashSet::new();
    while Instant::now() < deadline {
        let Ok((n, from)) = socket.recv_from(&mut buf) else { continue };
        let mut r = Reader::new(&buf[..n]);
        let Some((version, kind::DISCOVER_REPLY)) = r.header_any() else { continue };
        let id: Id = r.array().unwrap_or_default();
        let name = r.string().unwrap_or_default();
        let host_version = r.string().unwrap_or_else(|| "0.1.x or older".into());
        if seen.insert((from, version)) {
            let note = if version == protocol::VERSION { "compatible" } else { "different protocol" };
            println!("{from}  \"{name}\"  host {host_version}  protocol v{version} ({note})  id {}", hex(&id));
        }
    }
    if seen.is_empty() {
        println!("No answer from {to}.");
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}
