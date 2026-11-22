//! The server's own GIF library (1.28.0+): every `.gif` in the `gifs` folder of the data
//! folder (sub-folders are categories, file names are what people search for), plus GIFs
//! people with "Manage server GIFs" add from Voxa. Shown in the picker's 📚 Server tab.
//!
//! The folder is read again at most every 30 seconds when someone looks, so GIFs dropped
//! in appear by themselves. Small previews are made on first use and kept in
//! `gif-previews`. Everything here blocks (files): call it from a blocking thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use voxa_proto::{GifInfo, MAX_CHAT_IMAGE, MAX_GIF_QUERY};

/// GIFs per page, GIFs in the library at most, and how deep categories go.
const PAGE: usize = 48;
const MAX_GIFS: usize = 50_000;
/// GIFs added from Voxa stop once the library holds this much.
const MAX_LIBRARY_BYTES: u64 = 5 * 1024 * 1024 * 1024;
const MAX_DEPTH: usize = 3;
const RESCAN: Duration = Duration::from_secs(30);
/// GIFs up to this size are their own preview; bigger ones get a small copy.
const SMALL: u64 = 400 * 1024;
const PREVIEW_SIDE: u32 = 160;
const PREVIEW_FRAMES: usize = 80;
/// Where GIFs added from Voxa go when no category is given.
pub const DEFAULT_CATEGORY: &str = "Added in Voxa";

#[derive(Clone, Debug)]
struct LibGif {
    info: GifInfo,
    /// Path inside the gifs folder, with `/`.
    rel: String,
    category: String,
    /// Lower-case name and category, for searching.
    words: String,
}

#[derive(Default)]
struct Index {
    gifs: Vec<LibGif>,
    by_id: HashMap<String, usize>,
    categories: Vec<String>,
    bytes: u64,
    scanned: Option<Instant>,
}

pub struct Library {
    dir: PathBuf,
    previews: PathBuf,
    index: Mutex<Index>,
}

fn fnv(s: &str) -> u64 {
    s.bytes().fold(0xcbf2_9ce4_8422_2325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

/// A GIF's size from its header (logical screen width and height).
fn gif_size(head: &[u8]) -> Option<(u32, u32)> {
    if head.len() < 10 || !(head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a")) {
        return None;
    }
    Some((u16::from_le_bytes([head[6], head[7]]) as u32, u16::from_le_bytes([head[8], head[9]]) as u32))
}

/// Folder and file names people chose: letters, digits, spaces and `-_.()&'`, no dots at
/// the ends (no `..`), at most `max` characters.
pub fn clean_name(s: &str, max: usize) -> String {
    let s: String = s.chars().filter(|c| c.is_alphanumeric() || " -_.()&'".contains(*c)).take(max).collect();
    let s = s.trim().trim_matches('.').trim().to_string();
    // Windows device names (also with an extension) cannot be files or folders.
    let base = s.split('.').next().unwrap_or_default().trim().to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&base.as_str())
        || (base.len() == 4 && (base.starts_with("COM") || base.starts_with("LPT")) && base.as_bytes()[3].is_ascii_digit());
    if reserved { format!("_{s}") } else { s }
}

/// "happy-cat_dance" -> "happy cat dance".
fn display_name(stem: &str) -> String {
    stem.replace(['-', '_'], " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Library {
    pub fn new(data_dir: &Path) -> Library {
        let dir = data_dir.join("gifs");
        let _ = std::fs::create_dir_all(&dir);
        Library { previews: data_dir.join("gif-previews"), dir, index: Mutex::new(Index::default()) }
    }

    fn rescan_if_old(&self) {
        let old = self.index.lock().scanned.is_none_or(|t| t.elapsed() > RESCAN);
        if old {
            self.rescan();
        }
    }

    /// Reads the folder again (links and junctions are skipped: nothing outside it).
    pub fn rescan(&self) {
        let mut found = Vec::new();
        let mut bytes = 0u64;
        walk(&self.dir, &self.dir, 0, &mut found, &mut bytes);
        found.sort_by(|a, b| (a.category.to_lowercase(), a.info.title.to_lowercase()).cmp(&(b.category.to_lowercase(), b.info.title.to_lowercase())));
        let mut by_id = HashMap::new();
        let mut cats: Vec<String> = Vec::new();
        for (i, g) in found.iter().enumerate() {
            by_id.insert(g.info.id.clone(), i);
            if !g.category.is_empty() && !cats.contains(&g.category) {
                cats.push(g.category.clone());
            }
        }
        *self.index.lock() = Index { gifs: found, by_id, categories: cats, bytes, scanned: Some(Instant::now()) };
    }

    /// A page of GIFs matching every word of `query` (in their name or category).
    pub fn search(&self, query: &str, next: Option<&str>) -> (Vec<GifInfo>, Vec<String>, Option<String>, u32) {
        self.rescan_if_old();
        let q: String = query.chars().filter(|c| !c.is_control()).take(MAX_GIF_QUERY).collect::<String>().to_lowercase();
        let words: Vec<&str> = q.split_whitespace().collect();
        let ix = self.index.lock();
        let hits: Vec<&LibGif> = ix.gifs.iter().filter(|g| words.iter().all(|w| g.words.contains(w))).collect();
        let from: usize = next.and_then(|n| n.parse().ok()).unwrap_or(0).min(hits.len());
        let page: Vec<GifInfo> = hits[from..].iter().take(PAGE).map(|g| g.info.clone()).collect();
        let more = (from + PAGE < hits.len()).then(|| (from + PAGE).to_string());
        (page, ix.categories.clone(), more, hits.len() as u32)
    }

    fn path_of(&self, id: &str) -> Result<(PathBuf, LibGif), String> {
        let ix = self.index.lock();
        let g = ix.by_id.get(id).map(|i| ix.gifs[*i].clone()).ok_or("That GIF is no longer in the server's library")?;
        Ok((self.dir.join(&g.rel), g))
    }

    /// The GIF for posting: (bytes, file name, width, height).
    pub fn full(&self, id: &str) -> Result<(Vec<u8>, String, u32, u32), String> {
        let (p, g) = self.path_of(id)?;
        if crate::files::is_link(&p) {
            return Err("That GIF is no longer in the server's library".into());
        }
        let data = std::fs::read(&p).map_err(|_| "That GIF is no longer in the server's library".to_string())?;
        if data.len() > MAX_CHAT_IMAGE || gif_size(&data).is_none() {
            return Err("That file is not a GIF Voxa can post".into());
        }
        Ok((data, format!("{}.gif", g.info.title), g.info.width, g.info.height))
    }

    /// A small preview: small GIFs are their own; bigger ones get a scaled-down copy, made
    /// once and kept in `gif-previews`.
    pub fn preview(&self, id: &str) -> Result<Vec<u8>, String> {
        let (p, _) = self.path_of(id)?;
        let meta = std::fs::metadata(&p).map_err(|e| e.to_string())?;
        if crate::files::is_link(&p) || meta.len() > MAX_CHAT_IMAGE as u64 {
            return Err("not available".into());
        }
        if meta.len() <= SMALL {
            return std::fs::read(&p).map_err(|e| e.to_string());
        }
        let cached = self.previews.join(format!("{id}.gif"));
        if let (Ok(c), Ok(src)) = (std::fs::metadata(&cached), meta.modified()) {
            if c.modified().is_ok_and(|t| t >= src) {
                if let Ok(d) = std::fs::read(&cached) {
                    return Ok(d);
                }
            }
        }
        let data = std::fs::read(&p).map_err(|e| e.to_string())?;
        let small = shrink(&data)?;
        let _ = std::fs::create_dir_all(&self.previews);
        let _ = std::fs::write(&cached, &small);
        Ok(small)
    }

    /// Adds a GIF (from Voxa) to a category folder. Returns its name.
    pub fn add(&self, category: &str, name: &str, data: &[u8]) -> Result<String, String> {
        if data.len() > MAX_CHAT_IMAGE {
            return Err(format!("GIFs can be at most {} MB", MAX_CHAT_IMAGE / (1024 * 1024)));
        }
        let (w, h) = gif_size(data).ok_or("Only GIF files can be added")?;
        if w == 0 || h == 0 {
            return Err("That GIF has no picture".into());
        }
        let mut category = clean_name(category, 40);
        if category.is_empty() {
            category = DEFAULT_CATEGORY.into();
        }
        let stem = Path::new(name).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let mut stem = clean_name(&stem, 60);
        if stem.is_empty() {
            stem = "GIF".into();
        }
        let folder = self.dir.join(&category);
        if crate::files::is_link(&folder) {
            return Err("That category cannot be used".into());
        }
        std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
        {
            let ix = self.index.lock();
            if ix.gifs.len() >= MAX_GIFS || ix.bytes + data.len() as u64 > MAX_LIBRARY_BYTES {
                return Err("The server's GIF library is full (5 GB or 50 000 GIFs)".into());
            }
        }
        // Never over an existing file: "name", "name 2", "name 3"...
        let mut path = folder.join(format!("{stem}.gif"));
        let mut n = 2;
        while path.exists() && n < 10_000 {
            path = folder.join(format!("{stem} {n}.gif"));
            n += 1;
        }
        std::fs::write(&path, data).map_err(|e| e.to_string())?;
        self.rescan();
        Ok(display_name(&path.file_stem().unwrap_or_default().to_string_lossy()))
    }

    /// Removes a GIF from the library (its file and preview). Returns its name.
    pub fn remove(&self, id: &str) -> Result<String, String> {
        let (p, g) = self.path_of(id)?;
        if crate::files::is_link(&p) || !p.starts_with(&self.dir) {
            return Err("That GIF cannot be removed".into());
        }
        std::fs::remove_file(&p).map_err(|e| format!("Could not remove it: {e}"))?;
        let _ = std::fs::remove_file(self.previews.join(format!("{id}.gif")));
        self.rescan();
        Ok(g.info.title)
    }
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<LibGif>, bytes: &mut u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if out.len() >= MAX_GIFS {
            return;
        }
        let p = e.path();
        if crate::files::is_link(&p) {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_dir() {
            if depth < MAX_DEPTH {
                walk(root, &p, depth + 1, out, bytes);
            }
            continue;
        }
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("gif")) {
            continue;
        }
        let Ok(meta) = e.metadata() else { continue };
        if meta.len() == 0 || meta.len() > MAX_CHAT_IMAGE as u64 {
            continue;
        }
        let mut head = [0u8; 10];
        let ok = std::fs::File::open(&p).and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head)).is_ok();
        let Some((w, h)) = ok.then(|| gif_size(&head)).flatten() else { continue };
        let Ok(rel) = p.strip_prefix(root) else { continue };
        *bytes += meta.len();
        let rel = rel.to_string_lossy().replace('\\', "/");
        let category = rel.rsplit_once('/').map(|(c, _)| c.replace('/', " / ")).unwrap_or_default();
        let title = display_name(&p.file_stem().unwrap_or_default().to_string_lossy());
        let words = format!("{} {}", title.to_lowercase(), category.to_lowercase());
        out.push(LibGif { info: GifInfo { id: format!("s{:016x}", fnv(&rel.to_lowercase())), title, width: w, height: h }, rel, category, words });
    }
}

/// A small copy of a GIF for the picker (at most PREVIEW_SIDE, PREVIEW_FRAMES frames).
fn shrink(data: &[u8]) -> Result<Vec<u8>, String> {
    use image::AnimationDecoder;
    let mut dec = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(data)).map_err(|e| e.to_string())?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8_192);
    limits.max_image_height = Some(8_192);
    limits.max_alloc = Some(256 * 1024 * 1024);
    image::ImageDecoder::set_limits(&mut dec, limits).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    {
        let mut enc = image::codecs::gif::GifEncoder::new_with_speed(&mut out, 20);
        enc.set_repeat(image::codecs::gif::Repeat::Infinite).map_err(|e| e.to_string())?;
        // Frames dropped to stay under the limit are made up for by the next frame's delay.
        let mut n = 0;
        for f in dec.into_frames() {
            let f = f.map_err(|e| e.to_string())?;
            let delay = f.delay();
            let img = image::DynamicImage::ImageRgba8(f.into_buffer()).resize(PREVIEW_SIDE, PREVIEW_SIDE, image::imageops::FilterType::Triangle).to_rgba8();
            enc.encode_frame(image::Frame::from_parts(img, 0, 0, delay)).map_err(|e| e.to_string())?;
            n += 1;
            if n >= PREVIEW_FRAMES {
                break;
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gif(w: u32, h: u32, frames: usize) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut enc = image::codecs::gif::GifEncoder::new(&mut buf);
            for i in 0..frames {
                let img = image::RgbaImage::from_pixel(w, h, image::Rgba([(i * 40) as u8, 100, 200, 255]));
                enc.encode_frame(image::Frame::from_parts(img, 0, 0, image::Delay::from_numer_denom_ms(80, 1))).unwrap();
            }
        }
        buf
    }

    #[test]
    fn library_folder() {
        let d = std::env::temp_dir().join(format!("voxa-giflib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let lib = Library::new(&d);
        std::fs::create_dir_all(d.join("gifs/Reactions")).unwrap();
        std::fs::write(d.join("gifs/Reactions/happy-cat_dance.gif"), gif(40, 30, 3)).unwrap();
        std::fs::write(d.join("gifs/wave.gif"), gif(20, 20, 2)).unwrap();
        std::fs::write(d.join("gifs/notes.txt"), "hi").unwrap();
        std::fs::write(d.join("gifs/fake.gif"), "not a gif").unwrap();
        let (all, cats, next, total) = lib.search("", None);
        assert_eq!((all.len(), total, next), (2, 2, None));
        assert_eq!(cats, vec!["Reactions".to_string()]);
        let (hits, ..) = lib.search("CAT reactions", None);
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].title.as_str(), hits[0].width, hits[0].height), ("happy cat dance", 40, 30));
        let (data, name, ..) = lib.full(&hits[0].id).unwrap();
        assert_eq!(name, "happy cat dance.gif");
        assert_eq!(lib.preview(&hits[0].id).unwrap(), data, "small GIFs are their own preview");
        assert!(lib.full("s0000000000000000").is_err());

        // Adding: names cleaned, never over another file, only GIFs.
        // Only the file name counts, and "../" goes nowhere.
        assert_eq!(lib.add("../Memes", "..\\evil/../zap.gif", &gif(10, 10, 2)).unwrap(), "zap");
        assert!(d.join("gifs/Memes/zap.gif").exists());
        assert_eq!(lib.add("Memes", "zap", &gif(10, 10, 2)).unwrap(), "zap 2");
        assert!(lib.add("Memes", "a", b"PNG...").is_err());
        let (hits, ..) = lib.search("zap", None);
        assert_eq!(hits.len(), 2);
        assert_eq!(lib.remove(&hits[0].id).unwrap(), "zap");
        assert_eq!(lib.search("zap", None).0.len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn big_gifs_get_a_small_preview() {
        let big = gif(600, 400, 3);
        let small = shrink(&big).unwrap();
        let (w, h) = gif_size(&small).unwrap();
        assert!(w <= PREVIEW_SIDE && h <= PREVIEW_SIDE, "{w}x{h}");
    }

    #[test]
    fn names() {
        assert_eq!(clean_name("../..\\x", 40), "x");
        assert_eq!(clean_name("  Cats & Dogs (2) ", 40), "Cats & Dogs (2)");
        assert_eq!(clean_name("a:b*c?<d>|", 40), "abcd");
        assert_eq!(clean_name("con", 40), "_con");
        assert_eq!(clean_name("COM1.gif", 40), "_COM1.gif");
        assert_eq!(clean_name("Console", 40), "Console");
    }
}
