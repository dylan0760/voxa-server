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
