//! Traffic statistics: bytes sent and received since the program started, split into
//! voice, file transfers, screen sharing and chat / everything else. Sizes are what the
//! messages take on the wire (encrypted, with Voxa's framing), without TCP/IP headers.

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::msg::{BulkC2S, BulkS2C};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cat {
    Voice = 0,
    Files = 1,
    Screen = 2,
    Chat = 3,
}

pub struct Traffic {
    received: [AtomicU64; 4],
    sent: [AtomicU64; 4],
}

/// Counters of this process.
pub static TRAFFIC: Traffic = Traffic::new();

impl Traffic {
    pub const fn new() -> Traffic {
        Traffic {
            received: [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)],
            sent: [AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0)],
        }
    }

    pub fn received(&self, cat: Cat, bytes: usize) {
        self.received[cat as usize].fetch_add(bytes as u64, Ordering::Relaxed);
    }

    pub fn sent(&self, cat: Cat, bytes: usize) {
        self.sent[cat as usize].fetch_add(bytes as u64, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> TrafficStats {
        let r = |c: Cat| self.received[c as usize].load(Ordering::Relaxed);
        let s = |c: Cat| self.sent[c as usize].load(Ordering::Relaxed);
        TrafficStats {
            voice_in: r(Cat::Voice),
            voice_out: s(Cat::Voice),
            files_in: r(Cat::Files),
            files_out: s(Cat::Files),
            screen_in: r(Cat::Screen),
            screen_out: s(Cat::Screen),
            chat_in: r(Cat::Chat),
            chat_out: s(Cat::Chat),
        }
    }
}

impl Default for Traffic {
    fn default() -> Self {
        Traffic::new()
    }
}

/// Byte totals per category ("in" = received, "out" = sent).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(default)]
pub struct TrafficStats {
    pub voice_in: u64,
    pub voice_out: u64,
    pub files_in: u64,
    pub files_out: u64,
    pub screen_in: u64,
    pub screen_out: u64,
    pub chat_in: u64,
    pub chat_out: u64,
}

impl TrafficStats {
    pub fn total_in(&self) -> u64 {
        self.voice_in + self.files_in + self.screen_in + self.chat_in
    }

    pub fn total_out(&self) -> u64 {
        self.voice_out + self.files_out + self.screen_out + self.chat_out
    }

    /// Rows for display: (label, received, sent).
    pub fn rows(&self) -> [(&'static str, u64, u64); 4] {
        [
            ("Voice", self.voice_in, self.voice_out),
            ("File transfers", self.files_in, self.files_out),
            ("Screen sharing", self.screen_in, self.screen_out),
            ("Chat & other", self.chat_in, self.chat_out),
        ]
    }
}

/// Category of a message on the bulk (files / screen share) connection.
pub fn bulk_c2s(m: &BulkC2S) -> Cat {
    match m {
        BulkC2S::Frame(_) | BulkC2S::RequestKeyframe(_) | BulkC2S::StreamAudio(_) => Cat::Screen,
        BulkC2S::UploadBegin { .. }
        | BulkC2S::UploadChunk { .. }
        | BulkC2S::UploadEnd { .. }
        | BulkC2S::Download { .. }
        | BulkC2S::Cancel { .. }
        | BulkC2S::ChatImage { .. }
        | BulkC2S::GetImage(_) => Cat::Files,
    }
}

pub fn bulk_s2c(m: &BulkS2C) -> Cat {
    match m {
        BulkS2C::Frame { .. } | BulkS2C::KeyframeRequested | BulkS2C::StreamAudio { .. } => Cat::Screen,
        BulkS2C::Attached => Cat::Chat,
        _ => Cat::Files,
    }
}

/// "12.3 MB" style sizes.
pub fn fmt_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = b as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_and_totals() {
        let t = Traffic::new();
        t.received(Cat::Voice, 100);
        t.sent(Cat::Voice, 50);
        t.received(Cat::Files, 1000);
        t.sent(Cat::Chat, 7);
        let s = t.snapshot();
        assert_eq!((s.voice_in, s.voice_out, s.files_in, s.chat_out), (100, 50, 1000, 7));
        assert_eq!((s.total_in(), s.total_out()), (1100, 57));
        assert_eq!(fmt_bytes(1536), "1.5 KB");
        assert_eq!(fmt_bytes(12), "12 B");
    }
}
