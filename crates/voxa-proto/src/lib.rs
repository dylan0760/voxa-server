//! Voxa shared protocol crate.
//!
//! Everything the client and the server need to agree on lives here: data models,
//! control/bulk messages, the Noise based secure stream used for TCP traffic and the
//! encrypted UDP voice packet format.

pub mod identity;
pub mod lossless;
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
/// are the quality (0..=10), the top bits select the codec (0x80 Opus Music, servers
/// 1.10.0+; 0x40 Lossless, servers 1.11.0+). That keeps the wire format unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VoiceCodec {
    /// Mono, tuned for speech (low bitrates, packet loss protection).
    OpusVoice,
    /// Stereo, full band, high bitrate: for music.
    OpusMusic,
    /// 16-bit stereo PCM, bit exact (compressed like FLAC, ~0.8-1.5 Mbit/s).
    Lossless,
}

pub const CODEC_MUSIC_BIT: u8 = 0x80;
pub const CODEC_LOSSLESS_BIT: u8 = 0x40;

impl VoiceCodec {
    pub const ALL: [VoiceCodec; 3] = [VoiceCodec::OpusVoice, VoiceCodec::OpusMusic, VoiceCodec::Lossless];

    pub fn name(self) -> &'static str {
        match self {
            VoiceCodec::OpusVoice => "Opus Voice",
            VoiceCodec::OpusMusic => "Opus Music",
            VoiceCodec::Lossless => "Lossless",
        }
    }

    pub fn stereo(self) -> bool {
        self != VoiceCodec::OpusVoice
    }
}

/// The codec stored in a channel's `codec_quality`.
pub fn codec_of(codec_quality: u8) -> VoiceCodec {
    if codec_quality & CODEC_LOSSLESS_BIT != 0 {
        VoiceCodec::Lossless
    } else if codec_quality & CODEC_MUSIC_BIT != 0 {
        VoiceCodec::OpusMusic
    } else {
        VoiceCodec::OpusVoice
    }
}

/// The quality (0..=10) stored in a channel's `codec_quality`.
pub fn quality_of(codec_quality: u8) -> u8 {
    (codec_quality & !(CODEC_MUSIC_BIT | CODEC_LOSSLESS_BIT)).min(10)
}

/// Combines a codec and a quality into a channel's `codec_quality`.
pub fn pack_codec(codec: VoiceCodec, quality: u8) -> u8 {
    quality.min(10)
        | match codec {
            VoiceCodec::OpusVoice => 0,
            VoiceCodec::OpusMusic => CODEC_MUSIC_BIT,
            VoiceCodec::Lossless => CODEC_LOSSLESS_BIT,
        }
}

/// Opus bitrate for a channel's `codec_quality` (codec and quality).
pub fn channel_bitrate(codec_quality: u8) -> i32 {
    match codec_of(codec_quality) {
        VoiceCodec::OpusVoice => quality_to_bitrate(codec_quality),
        // Stereo music: 32 kbit/s .. 192 kbit/s (Opus is transparent from ~128 kbit/s).
        VoiceCodec::OpusMusic => 32_000 + quality_of(codec_quality) as i32 * 16_000,
        // Uncompressed size; the lossless compression usually saves 30-50% on music.
        VoiceCodec::Lossless => SAMPLE_RATE as i32 * 2 * 16,
    }
}

/// How a separator's text is laid out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpacerAlign {
    Left,
    Center,
    Right,
    /// The text repeated across the whole width (lines like `───` or `- - -`).
    Repeat,
}

/// A channel separator ("spacer", like TeamSpeak): a channel whose name starts with
/// `[spacer]`, `[lspacer]`, `[cspacer]`, `[rspacer]` or `[*spacer]`, optionally with a
/// number (`[cspacer2]`) so several can share a parent. Nobody can join one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spacer {
    pub align: SpacerAlign,
    pub number: Option<u32>,
    pub text: String,
}

impl Spacer {
    /// The channel name for this separator.
    pub fn name(&self) -> String {
        let tag = match self.align {
            SpacerAlign::Left => "",
            SpacerAlign::Center => "c",
            SpacerAlign::Right => "r",
            SpacerAlign::Repeat => "*",
        };
        let n = self.number.map(|n| n.to_string()).unwrap_or_default();
        format!("[{tag}spacer{n}]{}", self.text)
    }
}

/// The separator described by a channel name, or None for a normal channel.
pub fn spacer(name: &str) -> Option<Spacer> {
    let rest = name.strip_prefix('[')?;
    let close = rest.find(']')?;
    let (tag, text) = (&rest[..close], &rest[close + 1..]);
    let (align, tag) = match tag.as_bytes().first()? {
        b'c' => (SpacerAlign::Center, &tag[1..]),
        b'l' => (SpacerAlign::Left, &tag[1..]),
        b'r' => (SpacerAlign::Right, &tag[1..]),
        b'*' => (SpacerAlign::Repeat, &tag[1..]),
        _ => (SpacerAlign::Left, tag),
    };
    let digits = tag.strip_prefix("spacer")?;
    if digits.len() > 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number = if digits.is_empty() { None } else { Some(digits.parse().ok()?) };
    Some(Spacer { align, number, text: text.to_string() })
}

/// True if nobody can join this channel (it is a separator).
pub fn is_spacer(name: &str) -> bool {
    spacer(name).is_some()
}

#[cfg(test)]
mod spacer_tests {
    use super::*;

    #[test]
    fn teamspeak_style_spacers() {
        let s = spacer("[cspacer]Games").unwrap();
        assert_eq!((s.align, s.number, s.text.as_str()), (SpacerAlign::Center, None, "Games"));
        assert_eq!(spacer("[*spacer3]-").unwrap().align, SpacerAlign::Repeat);
        assert_eq!(spacer("[*spacer3]-").unwrap().number, Some(3));
        assert_eq!(spacer("[spacer]").unwrap().align, SpacerAlign::Left);
        assert_eq!(spacer("[lspacer]x").unwrap().align, SpacerAlign::Left);
        assert_eq!(spacer("[rspacer12]Right").unwrap().align, SpacerAlign::Right);
        for normal in ["Games", "[Games]", "[cspacerx]a", "[xspacer]a", "spacer", "[cspacer", "[cspacer1234567]a", "[]", "["] {
            assert!(spacer(normal).is_none(), "{normal}");
        }
        for n in ["[cspacer]Games", "[*spacer3]═", "[spacer]", "[rspacer7]Right"] {
            assert_eq!(spacer(n).unwrap().name(), n);
        }
        // Multi-byte text right after the tag.
        assert_eq!(spacer("[cspacer]ゲーム").unwrap().text, "ゲーム");
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
        assert_eq!(codec_of(pack_codec(VoiceCodec::Lossless, 3)), VoiceCodec::Lossless);
    }
}
