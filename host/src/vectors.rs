//! Checks the host against protocol-vectors.txt at the repo root. The Android app's unit tests
//! check the same file, so if both pass, the two sides agree on every key and packet byte.

use std::collections::HashMap;
use std::time::Instant;

use p256::ecdh::diffie_hellman;
use p256::{PublicKey, SecretKey};

use crate::crypto::{self, HostKey};
use crate::protocol::{self, Id, Nonce, PadState, PubKey, Reader, kind};
use crate::server::{PairRequest, pair_reply, status_packet, welcome_packet};

struct Vectors(HashMap<String, String>);

impl Vectors {
    fn load() -> Vectors {
        let lines = include_str!("../../protocol-vectors.txt").lines();
        Vectors(
            lines
                .filter_map(|line| {
                    let (key, value) = line.split('#').next()?.split_once('=')?;
                    Some((key.trim().to_string(), value.trim().to_string()))
                })
                .collect(),
        )
    }

    fn str(&self, key: &str) -> &str {
        self.0.get(key).unwrap_or_else(|| panic!("protocol-vectors.txt has no {key}"))
    }

    fn bytes(&self, key: &str) -> Vec<u8> {
        let s = self.str(key);
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    fn array<const N: usize>(&self, key: &str) -> [u8; N] {
        self.bytes(key).try_into().unwrap_or_else(|_| panic!("{key} isn't {N} bytes"))
    }

    fn int(&self, key: &str) -> i64 {
        self.str(key).parse().unwrap()
    }

    fn ints<const N: usize>(&self, key: &str) -> [i16; N] {
        let v: Vec<i16> = self.str(key).split_whitespace().map(|n| n.parse().unwrap()).collect();
        v.try_into().unwrap()
    }

    fn secret(&self, key: &str) -> SecretKey {
        SecretKey::from_slice(&self.bytes(key)).unwrap()
    }

    fn pad_state(&self) -> PadState {
        PadState {
            buttons: self.int("buttons") as u16,
            left_trigger: self.int("left_trigger") as u8,
            right_trigger: self.int("right_trigger") as u8,
            thumb_lx: self.int("lx") as i16,
            thumb_ly: self.int("ly") as i16,
            thumb_rx: self.int("rx") as i16,
            thumb_ry: self.int("ry") as i16,
            gyro: self.ints("gyro"),
            accel: self.ints("accel"),
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

/// Asserts with hex output, so a mismatch shows both values readably.
fn check(v: &Vectors, key: &str, actual: &[u8]) {
    assert_eq!(hex(actual), v.str(key), "{key}");
}

#[test]
fn keys() {
    let v = Vectors::load();
    let (client_id, host_id): (Id, Id) = (v.array("client_id"), v.array("host_id"));
    let host = HostKey::from_secret(v.secret("host_private"));
    let client = v.secret("client_private");
    let client_pub = crypto::encode(&client.public_key());
    check(&v, "host_public", &host.public);
    check(&v, "client_public", &client_pub);

    let pair_key = host.pair_key(&client_id, &host_id, &client_pub).unwrap();
    check(&v, "pair_key", &pair_key);
    // The phone's half of the key agreement lands on the same key.
    let host_pub = PublicKey::from_sec1_bytes(&host.public).unwrap();
    let shared = diffie_hellman(client.to_nonzero_scalar(), host_pub.as_affine());
    let phone_key = crypto::derive_pair_key(shared.raw_secret_bytes(), &client_id, &host_id, &client_pub, &host.public);
    assert_eq!(phone_key, pair_key);

    check(&v, "commitment", &crypto::commitment(&client_pub));
    assert_eq!(crypto::pair_code(&pair_key) as i64, v.int("pair_code"));
    check(&v, "pair_ok_mac", &crypto::pair_ok_mac(&pair_key));
    check(&v, "session_key", &crypto::session_key(&pair_key, &v.array("cnonce"), &v.array("hnonce")));
}

/// Parses the phone's packets the way the handlers in main.rs do.
#[test]
fn reads_phone_packets() {
    let v = Vectors::load();
    let client_id: Id = v.array("client_id");
    let pair_key: [u8; 32] = v.array("pair_key");
    let session_key: [u8; 32] = v.array("session_key");

    let hello = v.bytes("pair_hello");
    let mut r = Reader::new(&hello);
    assert_eq!(r.header(), Some(kind::PAIR_HELLO));
    assert_eq!(r.array::<16>(), Some(client_id));
    assert_eq!(r.array::<32>(), Some(v.array("commitment")));
    assert_eq!(r.string().as_deref(), Some(v.str("name")));

    let reveal = v.bytes("pair_reveal");
    let mut r = Reader::new(&reveal);
    assert_eq!(r.header(), Some(kind::PAIR_REVEAL));
    assert_eq!(r.array::<16>(), Some(client_id));
    let client_pub: PubKey = r.array().unwrap();
    assert_eq!(crypto::commitment(&client_pub), v.array::<32>("commitment"));

    let connect = v.bytes("connect");
    let (body, mac) = protocol::split_mac(&connect).unwrap();
    assert!(crypto::verify16(&pair_key, body, mac));
    let mut r = Reader::new(body);
    assert_eq!(r.header(), Some(kind::CONNECT));
    assert_eq!(r.array::<16>(), Some(client_id));
    assert_eq!(r.array::<16>(), Some(v.array("cnonce")));
    assert_eq!(r.u8(), Some(v.int("pad_type") as u8));

    let input = v.bytes("input");
    let (body, mac) = protocol::split_mac(&input).unwrap();
    assert!(crypto::verify16(&session_key, body, mac));
    let mut r = Reader::new(body);
    assert_eq!(r.header(), Some(kind::INPUT));
    assert_eq!(r.array::<16>(), Some(client_id));
    assert_eq!(r.u32(), Some(v.int("seq") as u32));
    assert_eq!(PadState::read(&mut r), Some(v.pad_state()));
}

/// Builds the host's packets with the same functions main.rs sends them with.
#[test]
fn writes_host_packets() {
    let v = Vectors::load();
    let host_id: Id = v.array("host_id");
    let pair_key: [u8; 32] = v.array("pair_key");
    let session_key: [u8; 32] = v.array("session_key");
    let mut request = PairRequest {
        client_id: v.array("client_id"),
        commitment: v.array("commitment"),
        name: v.str("name").into(),
        addr: "127.0.0.1:1".parse().unwrap(),
        host_key: HostKey::from_secret(v.secret("host_private")),
        pair_key: None,
        created: Instant::now(),
        decision: None,
    };
    check(&v, "pair_challenge", &pair_reply(&request, &host_id));
    request.pair_key = Some(pair_key);
    request.decision = Some(true);
    check(&v, "pair_result", &pair_reply(&request, &host_id));

    let hnonce: Nonce = v.array("hnonce");
    check(&v, "welcome", &welcome_packet(&hnonce, &session_key));
    let rumble = (v.int("large_motor") as u8, v.int("small_motor") as u8);
    check(&v, "status", &status_packet(v.int("status_seq") as u32, rumble, v.int("player") as u8, &session_key));
}
