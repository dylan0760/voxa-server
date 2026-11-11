//! Voxa shared protocol crate.
//!
//! Everything the client and the server need to agree on lives here: data models,
//! control/bulk messages, the Noise based secure stream used for TCP traffic and the
//! encrypted UDP voice packet format.

pub mod identity;
pub mod model;
pub mod msg;
pub mod transport;
#[cfg(feature = "update")]
pub mod update;
pub mod voice;

pub use model::*;
pub use msg::*;

/// Default port for both the TCP control connection and the UDP voice socket.
pub const DEFAULT_PORT: u16 = 9987;
/// Bumped whenever the wire format changes incompatibly.
pub const PROTOCOL_VERSION: u32 = 2;
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Opus is always run at 48 kHz mono with 20 ms frames.
pub const SAMPLE_RATE: u32 = 48_000;
pub const FRAME_SAMPLES: usize = 960;

pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Maps a channel codec quality (0..=10) to an Opus bitrate.
pub fn quality_to_bitrate(q: u8) -> i32 {
    let q = q.min(10) as i32;
    8_000 + q * 8_800 // 8 kbit/s .. 96 kbit/s
}
