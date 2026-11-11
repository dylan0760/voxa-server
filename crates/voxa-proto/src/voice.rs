//! Encrypted UDP voice packets.
//!
//! `[kind u8][client id u16][seq u64][ChaCha20-Poly1305 ciphertext]`, where the header is
//! authenticated as associated data and the nonce is derived from the direction and the
//! sequence number. Every client gets a fresh random 256 bit key per session over the
//! already encrypted control connection.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};

use crate::model::ClientId;

pub const HEADER: usize = 11;
pub const MAX_PACKET: usize = 1400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Voice = 1,
    Ping = 2,
    Pong = 3,
}

impl Kind {
    fn from_u8(v: u8) -> Option<Kind> {
        match v {
            1 => Some(Kind::Voice),
            2 => Some(Kind::Ping),
            3 => Some(Kind::Pong),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    ToServer = 0,
    ToClient = 1,
}

pub struct VoiceCipher {
    aead: ChaCha20Poly1305,
}

pub struct Packet {
    pub kind: Kind,
    pub client: ClientId,
    pub seq: u64,
    pub payload: Vec<u8>,
}

fn nonce(dir: Direction, seq: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[0] = dir as u8;
    n[4..].copy_from_slice(&seq.to_be_bytes());
    n
}

/// Reads the unauthenticated client id from a packet header so the receiver can look
/// up the right key before decrypting.
pub fn peek_client(buf: &[u8]) -> Option<ClientId> {
    if buf.len() < HEADER {
        return None;
    }
    Some(u16::from_be_bytes([buf[1], buf[2]]))
}

impl VoiceCipher {
    pub fn new(key: &[u8; 32]) -> Self {
        VoiceCipher { aead: ChaCha20Poly1305::new(Key::from_slice(key)) }
    }

    pub fn seal(&self, dir: Direction, kind: Kind, client: ClientId, seq: u64, payload: &[u8]) -> Vec<u8> {
        let mut header = [0u8; HEADER];
        header[0] = kind as u8;
        header[1..3].copy_from_slice(&client.to_be_bytes());
        header[3..11].copy_from_slice(&seq.to_be_bytes());
        let n = nonce(dir, seq);
        let ct = self
            .aead
            .encrypt(Nonce::from_slice(&n), Payload { msg: payload, aad: &header })
            .expect("encrypt");
        let mut out = Vec::with_capacity(HEADER + ct.len());
        out.extend_from_slice(&header);
        out.extend_from_slice(&ct);
        out
    }

    pub fn open(&self, dir: Direction, buf: &[u8]) -> Option<Packet> {
        if buf.len() < HEADER + 16 || buf.len() > MAX_PACKET + 64 {
            return None;
        }
        let kind = Kind::from_u8(buf[0])?;
        let client = u16::from_be_bytes([buf[1], buf[2]]);
        let seq = u64::from_be_bytes(buf[3..11].try_into().ok()?);
        let n = nonce(dir, seq);
        let payload = self
            .aead
            .decrypt(Nonce::from_slice(&n), Payload { msg: &buf[HEADER..], aad: &buf[..HEADER] })
            .ok()?;
        Some(Packet { kind, client, seq, payload })
    }
}

/// Sliding window replay protection (64 packets).
#[derive(Default, Clone)]
pub struct ReplayWindow {
    highest: u64,
    mask: u64,
    any: bool,
}

impl ReplayWindow {
    pub fn accept(&mut self, seq: u64) -> bool {
        if !self.any {
            self.any = true;
            self.highest = seq;
            self.mask = 1;
            return true;
        }
        if seq > self.highest {
            let shift = seq - self.highest;
            self.mask = if shift >= 64 { 1 } else { (self.mask << shift) | 1 };
            self.highest = seq;
            true
        } else {
            let diff = self.highest - seq;
            if diff >= 64 {
                return false;
            }
            let bit = 1u64 << diff;
            if self.mask & bit != 0 {
                false
            } else {
                self.mask |= bit;
                true
            }
        }
    }
}

/// Voice payload: `[frame seq u16][flags u8][opus data]`. An empty opus payload with
/// `FLAG_END` marks the end of a transmission.
pub const FLAG_END: u8 = 1;

pub fn encode_voice(frame_seq: u16, flags: u8, opus: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(3 + opus.len());
    v.extend_from_slice(&frame_seq.to_be_bytes());
    v.push(flags);
    v.extend_from_slice(opus);
    v
}

pub fn decode_voice(p: &[u8]) -> Option<(u16, u8, &[u8])> {
    if p.len() < 3 {
        return None;
    }
    Some((u16::from_be_bytes([p[0], p[1]]), p[2], &p[3..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let c = VoiceCipher::new(&[7u8; 32]);
        let p = c.seal(Direction::ToServer, Kind::Voice, 5, 42, b"hello");
        let o = c.open(Direction::ToServer, &p).unwrap();
        assert_eq!(o.payload, b"hello");
        assert_eq!(o.client, 5);
        assert!(c.open(Direction::ToClient, &p).is_none());
    }

    #[test]
    fn replay() {
        let mut w = ReplayWindow::default();
        assert!(w.accept(10));
        assert!(!w.accept(10));
        assert!(w.accept(12));
        assert!(w.accept(11));
        assert!(!w.accept(11));
        assert!(w.accept(200));
        assert!(!w.accept(100));
    }
}
