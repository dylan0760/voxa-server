//! Lossless audio codec for the "Lossless" channel codec: 16-bit stereo PCM, bit exact,
//! compressed the way FLAC does it (stereo decorrelation, fixed polynomial prediction,
//! Rice coded residuals) but for tiny packets (5 ms) so every packet fits a single UDP
//! datagram. A packet that would not get smaller is stored verbatim.
//!
//! Packet: `[mode u8][frames u16 BE][payload]`
//! * mode 0..=3: compressed, stereo layout left/right, left/side, side/right, mid/side;
//!   per channel `[order:3 bits][rice k:5 bits]`, `order` warm-up samples (18 bit
//!   two's complement) and the Rice coded residuals (zig-zag, unary quotient + k bits).
//! * mode 255: verbatim interleaved little endian i16.
//!
//! The decoder treats its input as untrusted: it never panics and never allocates more
//! than the frame count allows.

/// Most frames (stereo sample pairs) in one packet.
pub const MAX_FRAMES: usize = 4096;
const VERBATIM: u8 = 255;
const WARMUP_BITS: u32 = 18;
/// `order` value of a channel whose samples are all the same (e.g. silence).
const CONSTANT: usize = 7;

struct BitWriter {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl BitWriter {
    fn new(out: Vec<u8>) -> Self {
        BitWriter { out, acc: 0, n: 0 }
    }

    fn put(&mut self, value: u64, bits: u32) {
        debug_assert!(bits <= 32);
        if bits == 0 {
            return;
        }
        self.acc = (self.acc << bits) | (value & ((1u64 << bits) - 1));
        self.n += bits;
        while self.n >= 8 {
            self.n -= 8;
            self.out.push((self.acc >> self.n) as u8);
        }
    }

    fn unary(&mut self, q: u32) {
        let mut q = q;
        while q >= 32 {
            self.put(0, 32);
            q -= 32;
        }
        self.put(1, q + 1);
    }

    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push((self.acc << (8 - self.n)) as u8);
        }
        self.out
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    bit: u32,
}

impl<'a> BitReader<'a> {
    fn bit(&mut self) -> Option<u32> {
        let byte = *self.data.get(self.pos)?;
        let b = (byte >> (7 - self.bit)) & 1;
        self.bit += 1;
        if self.bit == 8 {
            self.bit = 0;
            self.pos += 1;
        }
        Some(b as u32)
    }

    fn get(&mut self, bits: u32) -> Option<u64> {
        let mut v = 0u64;
        for _ in 0..bits {
            v = (v << 1) | self.bit()? as u64;
        }
        Some(v)
    }

    /// Unary count of zeros before a one (bounded by the remaining input).
    fn unary(&mut self) -> Option<u32> {
        let mut q = 0u32;
        while self.bit()? == 0 {
            q += 1;
        }
        Some(q)
    }
}

fn zigzag(v: i64) -> u64 {
    ((v << 1) ^ (v >> 63)) as u64
}

fn unzigzag(u: u64) -> i64 {
    ((u >> 1) as i64) ^ -((u & 1) as i64)
}

/// Prediction of sample `i` from the previous ones with a fixed polynomial of `order`.
fn predict(x: &[i64], i: usize, order: usize) -> i64 {
    match order {
        0 => 0,
        1 => x[i - 1],
        2 => 2 * x[i - 1] - x[i - 2],
        3 => 3 * x[i - 1] - 3 * x[i - 2] + x[i - 3],
        _ => 4 * x[i - 1] - 6 * x[i - 2] + 4 * x[i - 3] - x[i - 4],
    }
}

/// Same as `predict` on untrusted data: wrapping arithmetic, never overflows.
fn predict_wrapping(x: &[i64], i: usize, order: usize) -> i64 {
    let w = |k: usize| x[i - k];
    match order {
        0 => 0,
        1 => w(1),
        2 => w(1).wrapping_mul(2).wrapping_sub(w(2)),
        3 => w(1).wrapping_mul(3).wrapping_sub(w(2).wrapping_mul(3)).wrapping_add(w(3)),
        _ => w(1).wrapping_mul(4).wrapping_sub(w(2).wrapping_mul(6)).wrapping_add(w(3).wrapping_mul(4)).wrapping_sub(w(4)),
    }
}

/// Rice parameter and coded size (bits) for residuals.
fn best_rice(res: &[u64]) -> (u32, u64) {
    let mut best = (0u32, u64::MAX);
    for k in 0..=24u32 {
        let bits: u64 = res.iter().map(|u| (u >> k) + 1 + k as u64).sum();
        if bits < best.1 {
            best = (k, bits);
        }
    }
    best
}

/// Best predictor for one channel: (order, k, residuals, size in bits).
fn analyse(x: &[i64]) -> (usize, u32, Vec<u64>, u64) {
    if x.windows(2).all(|w| w[0] == w[1]) {
        return (CONSTANT, 0, Vec::new(), 8 + WARMUP_BITS as u64);
    }
    let mut best: Option<(usize, u32, Vec<u64>, u64)> = None;
    for order in 0..=4usize.min(x.len()) {
        let res: Vec<u64> = (order..x.len()).map(|i| zigzag(x[i] - predict(x, i, order))).collect();
        let (k, bits) = best_rice(&res);
        let total = 8 + order as u64 * WARMUP_BITS as u64 + bits;
        if best.as_ref().map(|b| total < b.3).unwrap_or(true) {
            best = Some((order, k, res, total));
        }
    }
    best.unwrap_or((0, 0, Vec::new(), 8))
}

/// Encodes interleaved stereo 16-bit samples (`pcm.len()` = 2 × frames).
pub fn encode(pcm: &[i16]) -> Vec<u8> {
    let frames = (pcm.len() / 2).min(MAX_FRAMES);
    let l: Vec<i64> = (0..frames).map(|i| pcm[2 * i] as i64).collect();
    let r: Vec<i64> = (0..frames).map(|i| pcm[2 * i + 1] as i64).collect();
    let side: Vec<i64> = l.iter().zip(&r).map(|(a, b)| a - b).collect();
    let mid: Vec<i64> = l.iter().zip(&r).map(|(a, b)| (a + b) >> 1).collect();
    let (al, ar, as_, am) = (analyse(&l), analyse(&r), analyse(&side), analyse(&mid));
    // Stereo layouts like FLAC: independent, left/side, side/right, mid/side.
    let layouts = [(&al, &ar), (&al, &as_), (&as_, &ar), (&am, &as_)];
    let (mode, (a, b)) = layouts.iter().enumerate().min_by_key(|(_, (a, b))| a.3 + b.3).map(|(m, p)| (m as u8, *p)).unwrap();
    let verbatim_bits = frames as u64 * 32;
    let mut head = Vec::with_capacity(3 + frames * 4);
    if a.3 + b.3 >= verbatim_bits {
        head.push(VERBATIM);
        head.extend_from_slice(&(frames as u16).to_be_bytes());
        for s in &pcm[..frames * 2] {
            head.extend_from_slice(&s.to_le_bytes());
        }
        return head;
    }
    head.push(mode);
    head.extend_from_slice(&(frames as u16).to_be_bytes());
    let mut w = BitWriter::new(head);
    let channels = [(a, if mode == 3 { &mid } else if mode == 2 { &side } else { &l }), (b, if mode == 0 { &r } else if mode == 2 { &r } else { &side })];
    for ((order, k, res, _), x) in channels {
        w.put(*order as u64, 3);
        w.put(*k as u64, 5);
        for s in x.iter().take(if *order == CONSTANT { 1 } else { *order }) {
            w.put(*s as u64, WARMUP_BITS);
        }
        for u in res {
            w.unary((u >> k) as u32);
            w.put(*u, *k);
        }
    }
    w.finish()
}

/// Decodes a packet into interleaved stereo 16-bit samples. None if it is malformed.
pub fn decode(data: &[u8]) -> Option<Vec<i16>> {
    let mode = *data.first()?;
    let frames = u16::from_be_bytes([*data.get(1)?, *data.get(2)?]) as usize;
    if frames == 0 || frames > MAX_FRAMES {
        return None;
    }
    let body = &data[3..];
    if mode == VERBATIM {
        if body.len() < frames * 4 {
            return None;
        }
        return Some(body[..frames * 4].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect());
    }
    if mode > 3 {
        return None;
    }
    let mut rd = BitReader { data: body, pos: 0, bit: 0 };
    let mut ch = [Vec::with_capacity(frames), Vec::with_capacity(frames)];
    for x in ch.iter_mut() {
        let order = rd.get(3)? as usize;
        let k = rd.get(5)? as u32;
        if order == CONSTANT {
            let v = rd.get(WARMUP_BITS)?;
            x.resize(frames, ((v << (64 - WARMUP_BITS)) as i64) >> (64 - WARMUP_BITS));
            continue;
        }
        if order > 4 || order > frames || k > 24 {
            return None;
        }
        for _ in 0..order {
            let v = rd.get(WARMUP_BITS)?;
            // Sign-extend the 18 bit value.
            x.push(((v << (64 - WARMUP_BITS)) as i64) >> (64 - WARMUP_BITS));
        }
        for i in order..frames {
            let q = rd.unary()? as u64;
            let u = (q << k) | rd.get(k)?;
            let v = predict_wrapping(x, i, order).wrapping_add(unzigzag(u));
            // Real samples (and sides) fit 18 bits; anything else is a broken packet.
            if !(-(1 << 17)..(1 << 17)).contains(&v) {
                return None;
            }
            x.push(v);
        }
    }
    let [a, b] = ch;
    let mut out = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let (l, r) = match mode {
            0 => (a[i], b[i]),
            1 => (a[i], a[i] - b[i]),
            2 => (a[i] + b[i], b[i]),
            _ => {
                // mid/side: mid = (l + r) >> 1 lost one bit; side's lowest bit restores it.
                let (mid, side) = (a[i], b[i]);
                let sum = (mid << 1) | (side & 1);
                ((sum + side) >> 1, (sum - side) >> 1)
            }
        };
        if !(i16::MIN as i64..=i16::MAX as i64).contains(&l) || !(i16::MIN as i64..=i16::MAX as i64).contains(&r) {
            return None;
        }
        out.push(l as i16);
        out.push(r as i16);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(name: &str, pcm: &[i16]) -> usize {
        let enc = encode(pcm);
        let dec = decode(&enc).unwrap_or_else(|| panic!("{name}: did not decode"));
        assert_eq!(dec, pcm, "{name}: not bit exact");
        assert!(enc.len() <= 3 + pcm.len() * 2, "{name}: bigger than verbatim");
        println!("{name}: {} -> {} bytes ({:.0}%)", pcm.len() * 2, enc.len(), enc.len() as f64 * 100.0 / (pcm.len() * 2) as f64);
        enc.len()
    }

    fn rng(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    #[test]
    fn bit_exact_for_all_kinds_of_sound() {
        let n = 240;
        let mut seed = 0x1234_5678_9abc_def1u64;
        let music: Vec<i16> = (0..n)
            .flat_map(|i| {
                let t = i as f64 / 48_000.0;
                let s = (t * 440.0 * std::f64::consts::TAU).sin() * 9000.0 + (t * 1320.0 * std::f64::consts::TAU).sin() * 3000.0;
                [s as i16, (s * 0.8 + (t * 220.0 * std::f64::consts::TAU).sin() * 2000.0) as i16]
            })
            .collect();
        let noise: Vec<i16> = (0..n * 2).map(|_| rng(&mut seed) as i16).collect();
        let quiet_noise: Vec<i16> = (0..n * 2).map(|_| (rng(&mut seed) % 64) as i16 - 32).collect();
        let extremes: Vec<i16> = (0..n).flat_map(|i| if i % 2 == 0 { [i16::MAX, i16::MIN] } else { [i16::MIN, i16::MAX] }).collect();
        let square: Vec<i16> = (0..n).flat_map(|i| if (i / 20) % 2 == 0 { [20000, -20000] } else { [-20000, 20000] }).collect();
        let silence = vec![0i16; n * 2];
        let mono: Vec<i16> = music.chunks_exact(2).flat_map(|p| [p[0], p[0]]).collect();
        let music_len = roundtrip("music", &music);
        assert!(music_len < n * 4 * 7 / 10, "music should compress: {music_len}");
        roundtrip("white noise", &noise);
        roundtrip("quiet noise", &quiet_noise);
        roundtrip("full scale extremes", &extremes);
        roundtrip("square wave", &square);
        assert!(roundtrip("silence", &silence) < 20);
        assert!(roundtrip("mono as stereo", &mono) < music_len);
        roundtrip("one frame", &[123, -456]);
        roundtrip("4096 frames", &(0..MAX_FRAMES * 2).map(|i| ((i * 37) % 2000) as i16).collect::<Vec<_>>());
    }

    /// A 5 ms packet always fits one UDP datagram (with the voice headers).
    #[test]
    fn packets_fit_a_datagram() {
        let mut seed = 7u64;
        let worst: Vec<i16> = (0..240 * 2).map(|_| rng(&mut seed) as i16).collect();
        assert!(encode(&worst).len() + 3 + crate::voice::HEADER + 16 <= crate::voice::MAX_PACKET);
    }

    /// Garbage never panics and never yields more samples than declared.
    #[test]
    fn hostile_packets_are_refused_safely() {
        let mut seed = 99u64;
        for i in 0..20_000 {
            let len = (rng(&mut seed) % 1200) as usize;
            let mut p: Vec<u8> = (0..len).map(|_| rng(&mut seed) as u8).collect();
            if i % 2 == 0 && p.len() >= 3 {
                p[0] = (rng(&mut seed) % 4) as u8;
            }
            if let Some(out) = decode(&p) {
                assert!(out.len() <= MAX_FRAMES * 2);
            }
        }
        // Truncated real packets fail cleanly.
        let real = encode(&(0..480).map(|i| (i * 31 % 4000) as i16).collect::<Vec<_>>());
        for cut in 0..real.len() {
            let _ = decode(&real[..cut]);
        }
    }
}
