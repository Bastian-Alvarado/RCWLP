//! Pairing key agreement (P-256 ECDH) and packet authentication (HMAC-SHA256).
//! These are all available in stock Android (javax.crypto), so the phone needs no extra libraries.

use hmac::{Hmac, Mac};
use p256::ecdh::diffie_hellman;
use p256::elliptic_curve::sec1::ToEncodedPoint;
use p256::{PublicKey, SecretKey};
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};

use crate::protocol::{Commitment, Id, MAC_LEN, Nonce, PubKey};

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

/// The host's key pair for one pairing attempt. It's created when the phone's hello arrives
/// and kept until the phone reveals the public key it committed to.
pub struct HostKey {
    secret: SecretKey,
    pub public: PubKey,
}

impl HostKey {
    pub fn random() -> HostKey {
        HostKey::from_secret(SecretKey::random(&mut OsRng))
    }

    pub fn from_secret(secret: SecretKey) -> HostKey {
        let public = encode(&secret.public_key());
        HostKey { secret, public }
    }

    /// Runs the host half of the key agreement. Returns None if the phone's public key is invalid.
    pub fn pair_key(&self, client_id: &Id, host_id: &Id, client_pub: &PubKey) -> Option<[u8; 32]> {
        let peer = PublicKey::from_sec1_bytes(client_pub).ok()?;
        let shared = diffie_hellman(self.secret.to_nonzero_scalar(), peer.as_affine());
        Some(derive_pair_key(shared.raw_secret_bytes(), client_id, host_id, client_pub, &self.public))
    }
}

/// Uncompressed SEC1 point: 0x04 || x || y.
pub fn encode(key: &PublicKey) -> PubKey {
    key.to_encoded_point(false).as_bytes().try_into().expect("uncompressed P-256 points are 65 bytes")
}

pub fn derive_pair_key(shared: &[u8], client_id: &Id, host_id: &Id, client_pub: &PubKey, host_pub: &PubKey) -> [u8; 32] {
    hmac(shared, &[b"rcwlp-pair-v1", client_id, host_id, client_pub, host_pub])
}

/// What the phone sends in its hello instead of its public key. Because the phone is bound to
/// its key before it sees ours, a man in the middle can't search for a key pair that makes
/// both sides show the same code.
pub fn commitment(client_pub: &PubKey) -> Commitment {
    Sha256::new().chain_update(b"rcwlp-commit").chain_update(client_pub).finalize().into()
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
        // Simulate the phone with its own key.
        let phone = SecretKey::random(&mut OsRng);
        let phone_pub = encode(&phone.public_key());
        let (cid, hid) = ([1u8; 16], [2u8; 16]);
        let host = HostKey::random();
        let host_key = host.pair_key(&cid, &hid, &phone_pub).unwrap();

        let host_pk = PublicKey::from_sec1_bytes(&host.public).unwrap();
        let shared = diffie_hellman(phone.to_nonzero_scalar(), host_pk.as_affine());
        let phone_key = derive_pair_key(shared.raw_secret_bytes(), &cid, &hid, &phone_pub, &host.public);
        assert_eq!(phone_key, host_key);
        assert!(pair_code(&phone_key) < 1_000_000);
    }

    #[test]
    fn rejects_invalid_phone_key() {
        let mut bad = [0u8; 65];
        bad[0] = 4;
        assert!(HostKey::random().pair_key(&[1; 16], &[2; 16], &bad).is_none());
    }

    #[test]
    fn commitment_binds_the_key() {
        let a = encode(&SecretKey::random(&mut OsRng).public_key());
        let b = encode(&SecretKey::random(&mut OsRng).public_key());
        assert_eq!(commitment(&a), commitment(&a));
        assert_ne!(commitment(&a), commitment(&b));
    }
}
