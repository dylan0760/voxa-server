use base64::Engine;
use sha2::{Digest, Sha256};

/// Noise parameters used for every TCP connection. XX gives mutual authentication:
/// the server proves its long term key (pinned by clients, trust-on-first-use) and the
/// client proves its identity key, whose hash is the client's unique id.
pub const NOISE_PARAMS: &str = "Noise_XX_25519_ChaChaPoly_BLAKE2s";
pub const NOISE_PROLOGUE: &[u8] = b"VOXA/1";

#[derive(Clone)]
pub struct KeyPair {
    pub private: Vec<u8>,
    pub public: Vec<u8>,
}

impl KeyPair {
    pub fn generate() -> KeyPair {
        let builder = snow::Builder::new(NOISE_PARAMS.parse().expect("noise params"));
        let kp = builder.generate_keypair().expect("keypair");
        KeyPair { private: kp.private.to_vec(), public: kp.public.to_vec() }
    }

    /// Export as a single base64 string (private key only; the public key is derived).
    pub fn export(&self) -> String {
        base64::engine::general_purpose::STANDARD.encode(&self.private)
    }

    pub fn import(s: &str) -> Option<KeyPair> {
        let private = base64::engine::general_purpose::STANDARD.decode(s.trim()).ok()?;
        if private.len() != 32 {
            return None;
        }
        let public = public_from_private(&private)?;
        Some(KeyPair { private, public })
    }
}

/// Derives an X25519 public key from a private key by running a throwaway handshake
/// builder; snow does not expose scalar multiplication directly.
fn public_from_private(private: &[u8]) -> Option<Vec<u8>> {
    use snow::resolvers::{CryptoResolver, DefaultResolver};
    let mut dh = DefaultResolver.resolve_dh(&snow::params::DHChoice::Curve25519)?;
    dh.set(private);
    Some(dh.pubkey().to_vec())
}

/// X25519 key agreement; None if a key is malformed or the result is all zeros (a
/// low-order public key).
pub fn agree(private: &[u8], public: &[u8]) -> Option<[u8; 32]> {
    use snow::resolvers::{CryptoResolver, DefaultResolver};
    if private.len() != 32 || public.len() != 32 {
        return None;
    }
    let mut dh = DefaultResolver.resolve_dh(&snow::params::DHChoice::Curve25519)?;
    dh.set(private);
    let mut out = [0u8; 32];
    dh.dh(public, &mut out).ok()?;
    (out != [0u8; 32]).then_some(out)
}

/// The public key of a private key.
pub fn public_key(private: &[u8]) -> Option<Vec<u8>> {
    (private.len() == 32).then(|| public_from_private(private)).flatten()
}

pub fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    for p in parts {
        inner.update(p);
    }
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

/// Proof of owning an identity for the web admin page: HMAC-SHA256 keyed with the X25519
/// agreement between the identity and the server's one-time key, over the challenge and
/// the page's origin (so it only works on that web address).
pub fn web_login_proof(shared: &[u8; 32], server_key: &[u8], nonce: &[u8], public: &[u8], origin: &str) -> [u8; 32] {
    hmac_sha256(shared, &[b"voxa-web-login-v2", server_key, nonce, public, origin.as_bytes()])
}

/// Unique, stable client id derived from the identity public key.
pub fn uid_from_public(public: &[u8]) -> String {
    let hash = Sha256::digest(public);
    base64::engine::general_purpose::STANDARD.encode(&hash[..20])
}

/// Human readable fingerprint (used to pin server keys).
pub fn fingerprint(public: &[u8]) -> String {
    let hash = Sha256::digest(public);
    hash[..16]
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .chunks(2)
        .map(|c| c.concat())
        .collect::<Vec<_>>()
        .join(":")
}
