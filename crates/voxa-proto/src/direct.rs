//! Bounded, independently decodable JPEGs for private live video. A voice/video
//! note is a sequence of JPEGs at 8 fps with optional mono 16-bit 48 kHz PCM.
//! Fixed-width little-endian lengths make the format usable by the web page too.
pub const MAX_FRAME: usize = 240 * 1024;
pub const MAX_CLIP: usize = 8 * 1024 * 1024;
pub const FPS: usize = 8;
pub const MAX_FRAMES: usize = 15 * FPS;
pub const MAGIC: &[u8; 4] = b"VXC1";
pub fn frame_ok(jpeg: &[u8]) -> bool {
    jpeg.len() <= MAX_FRAME
        && crate::image_kind(jpeg) == Some("jpg")
        && crate::image_dims(jpeg).is_some_and(|(w, h)| (1..=1280).contains(&w) && (1..=1280).contains(&h))
}

pub struct Clip<'a> {
    pub frames: Vec<&'a [u8]>,
    pub pcm: &'a [u8],
}

pub fn clip(data: &[u8]) -> Option<Clip<'_>> {
    if data.len() > MAX_CLIP || !data.starts_with(MAGIC) {
        return None;
    }
    let mut rest = &data[4..];
    let mut number = || -> Option<usize> {
        let n = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?);
        rest = &rest[4..];
        Some(n as usize)
    };
    let count = number()?;
    let audio = number()?;
    if !(1..=MAX_FRAMES).contains(&count) || audio % 2 != 0 || audio > count * 2 * 48_000 / FPS {
        return None;
    }
    let mut frames = Vec::with_capacity(count);
    for _ in 0..count {
        let n = u32::from_le_bytes(rest.get(..4)?.try_into().ok()?) as usize;
        let (jpeg, tail) = rest.get(4..)?.split_at_checked(n)?;
        if !frame_ok(jpeg) {
            return None;
        }
        frames.push(jpeg);
        rest = tail;
    }
    (rest.len() == audio).then_some(Clip { frames, pcm: rest })
}

pub fn clip_id(text: &str) -> Option<u64> {
    text.strip_prefix("🎥 Video message\n[voxa-clip:")?.strip_suffix(']')?.parse().ok()
}
