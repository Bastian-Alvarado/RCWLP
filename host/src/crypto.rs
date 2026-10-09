//! Pairing key agreement (P-256 ECDH) and packet authentication (HMAC-SHA256).
//! These are all available in stock Android (javax.crypto), so the phone needs no extra libraries.

use hmac::{Hmac, Mac};
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, ecdh::EphemeralSecret};
use rand_core::{OsRng, RngCore};
use sha2::Sha256;

use crate::protocol::{Id, MAC_LEN, Nonce, PubKey};

type HmacSha256 = Hmac<Sha256>;

pub fn hmac(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut m = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    for p in parts {
        m.update(p);
    }
    m.finalize().into_bytes().into()
}

pub fn mac16(key: &[u8], data: &[u8]) -> [u8; MAC_LEN] {
    hmac(key, &[data])[..MAC_LEN].try_into().unwrap()
}

/// Constant-time check of a truncated MAC.
pub fn verify16(key: &[u8], data: &[u8], mac: &[u8]) -> bool {
    let mut m = HmacSha256::new_from_slice(key).expect("HMAC accepts any key length");
    m.update(data);
    m.verify_truncated_left(mac).is_ok()
}

pub fn random<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    OsRng.fill_bytes(&mut b);
    b
}

pub struct PairKeys {
    pub host_pub: PubKey,
    pub pair_key: [u8; 32],
    pub code: u32,
}

/// Runs the host half of pairing. Returns None if the phone's public key is invalid.
pub fn pair(client_id: &Id, host_id: &Id, client_pub: &PubKey) -> Option<PairKeys> {
    let peer = PublicKey::from_sec1_bytes(client_pub).ok()?;
    let secret = EphemeralSecret::random(&mut OsRng);
    let host_pub: PubKey = secret.public_key().to_encoded_point(false).as_bytes().try_into().ok()?;
    let shared = secret.diffie_hellman(&peer);
    let pair_key = hmac(
        shared.raw_secret_bytes(),
        &[b"rcwlp-pair-v1", client_id, host_id, client_pub, &host_pub],
    );
    Some(PairKeys { host_pub, pair_key, code: pair_code(&pair_key) })
}

/// Six-digit code both sides display so the user can confirm they derived the same key.
pub fn pair_code(pair_key: &[u8; 32]) -> u32 {
    let h = hmac(pair_key, &[b"rcwlp-code"]);
    u32::from_be_bytes(h[..4].try_into().unwrap()) % 1_000_000
}

pub fn pair_ok_mac(pair_key: &[u8; 32]) -> [u8; MAC_LEN] {
    mac16(pair_key, b"rcwlp-pair-ok")
}

pub fn session_key(pair_key: &[u8; 32], cnonce: &Nonce, hnonce: &Nonce) -> [u8; 32] {
    hmac(pair_key, &[b"rcwlp-session", cnonce, hnonce])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_roundtrip() {
        let key = [7u8; 32];
        let mac = mac16(&key, b"hello");
        assert!(verify16(&key, b"hello", &mac));
        assert!(!verify16(&key, b"hellp", &mac));
        assert!(!verify16(&[8u8; 32], b"hello", &mac));
    }

    #[test]
    fn both_sides_derive_same_key() {
        // Simulate the phone with its own ephemeral key.
        let phone = EphemeralSecret::random(&mut OsRng);
        let phone_pub: PubKey = phone.public_key().to_encoded_point(false).as_bytes().try_into().unwrap();
        let (cid, hid) = ([1u8; 16], [2u8; 16]);
        let host = pair(&cid, &hid, &phone_pub).unwrap();

        let host_pk = PublicKey::from_sec1_bytes(&host.host_pub).unwrap();
        let shared = phone.diffie_hellman(&host_pk);
        let phone_key = hmac(
            shared.raw_secret_bytes(),
            &[b"rcwlp-pair-v1", &cid, &hid, &phone_pub, &host.host_pub],
        );
        assert_eq!(phone_key, host.pair_key);
        assert_eq!(pair_code(&phone_key), host.code);
        assert!(host.code < 1_000_000);
    }
}
