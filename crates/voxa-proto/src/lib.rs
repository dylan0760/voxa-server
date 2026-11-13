//! Voxa shared protocol crate.
//!
//! Everything the client and the server need to agree on lives here: data models,
//! control/bulk messages, the Noise based secure stream used for TCP traffic and the
//! encrypted UDP voice packet format.

pub mod identity;
pub mod model;
pub mod msg;
pub mod traffic;
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

/// Maps a channel codec quality (0..=10) to an Opus Voice bitrate.
pub fn quality_to_bitrate(q: u8) -> i32 {
    let q = quality_of(q) as i32;
    8_000 + q * 8_800 // 8 kbit/s .. 96 kbit/s
}

/// The codec a channel uses.
///
/// Stored in `ChannelInfo::codec_quality` / `ChannelEdit::codec_quality`: the low bits
/// are the quality (0..=10), the top bit selects Opus Music. That keeps the wire format
/// unchanged; only servers 1.10.0+ keep the bit (older ones clamp the value to 10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceCodec {
    /// Mono, tuned for speech (low bitrates, packet loss protection).
    OpusVoice,
    /// Stereo, full band, high bitrate: for music.
    OpusMusic,
}

pub const CODEC_MUSIC_BIT: u8 = 0x80;

impl VoiceCodec {
    pub const ALL: [VoiceCodec; 2] = [VoiceCodec::OpusVoice, VoiceCodec::OpusMusic];

    pub fn name(self) -> &'static str {
        match self {
            VoiceCodec::OpusVoice => "Opus Voice",
            VoiceCodec::OpusMusic => "Opus Music",
        }
    }

    pub fn stereo(self) -> bool {
        self == VoiceCodec::OpusMusic
    }
}

/// The codec stored in a channel's `codec_quality`.
pub fn codec_of(codec_quality: u8) -> VoiceCodec {
    if codec_quality & CODEC_MUSIC_BIT != 0 {
        VoiceCodec::OpusMusic
    } else {
        VoiceCodec::OpusVoice
    }
}

/// The quality (0..=10) stored in a channel's `codec_quality`.
pub fn quality_of(codec_quality: u8) -> u8 {
    (codec_quality & !CODEC_MUSIC_BIT).min(10)
}

/// Combines a codec and a quality into a channel's `codec_quality`.
pub fn pack_codec(codec: VoiceCodec, quality: u8) -> u8 {
    quality.min(10) | if codec == VoiceCodec::OpusMusic { CODEC_MUSIC_BIT } else { 0 }
}

/// Opus bitrate for a channel's `codec_quality` (codec and quality).
pub fn channel_bitrate(codec_quality: u8) -> i32 {
    match codec_of(codec_quality) {
        VoiceCodec::OpusVoice => quality_to_bitrate(codec_quality),
        // Stereo music: 32 kbit/s .. 192 kbit/s (Opus is transparent from ~128 kbit/s).
        VoiceCodec::OpusMusic => 32_000 + quality_of(codec_quality) as i32 * 16_000,
    }
}

/// Start of the refusal message for clients that are older than the server requires;
/// newer clients offer to update when they see it.
pub const UPDATE_REQUIRED: &str = "Update required: ";

#[cfg(test)]
mod codec_tests {
    use super::*;

    #[test]
    fn codec_packing() {
        for codec in VoiceCodec::ALL {
            for q in 0..=10 {
                let packed = pack_codec(codec, q);
                assert_eq!((codec_of(packed), quality_of(packed)), (codec, q));
            }
        }
        // Old values are Opus Voice; the voice bitrates do not change.
        assert_eq!(codec_of(6), VoiceCodec::OpusVoice);
        assert_eq!(channel_bitrate(6), quality_to_bitrate(6));
        assert_eq!(channel_bitrate(pack_codec(VoiceCodec::OpusMusic, 10)), 192_000);
        assert_eq!(channel_bitrate(pack_codec(VoiceCodec::OpusMusic, 6)), 128_000);
    }
}
