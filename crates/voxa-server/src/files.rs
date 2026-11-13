//! Per-channel file storage (`<data>\files\channel_<id>\...`).

use std::path::{Path, PathBuf};

use voxa_proto::{ChannelId, FileEntry};

pub fn channel_root(data: &Path, ch: ChannelId) -> PathBuf {
    data.join("files").join(format!("channel_{ch}"))
}

/// True for names Windows treats as devices (CON, NUL, COM1, "aux.txt", ...).
fn is_device_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or(part).trim_end().to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem[3..].chars().all(|c| c.is_ascii_digit() || "¹²³".contains(c)))
}

/// True if the path exists and is a symbolic link, junction or other reparse point.
fn is_link(p: &Path) -> bool {
    let Ok(m) = std::fs::symlink_metadata(p) else { return false };
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        if m.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return true;
        }
    }
    m.file_type().is_symlink()
}

/// Resolves a client supplied relative path safely inside the channel root. Links and
/// junctions on the way are refused, so nothing outside the data folder can be reached.
pub fn resolve(data: &Path, ch: ChannelId, rel: &str) -> Result<PathBuf, String> {
    let files = data.join("files");
    let mut p = channel_root(data, ch);
    if is_link(&files) || is_link(&p) {
        return Err("Invalid path".into());
    }
    for part in rel.split(['/', '\\']).filter(|s| !s.is_empty()) {
        if part == "." || part == ".." || part.contains(':') || part.chars().any(|c| c.is_control() || "<>\"|?*".contains(c)) {
            return Err("Invalid path".into());
        }
        if part.ends_with('.') || part.ends_with(' ') || is_device_name(part) || part.ends_with(".voxa-part") {
            return Err("Invalid file name".into());
        }
        p.push(part);
        if is_link(&p) {
            return Err("Invalid path".into());
        }
    }
    Ok(p)
}

fn unix(t: std::io::Result<std::time::SystemTime>) -> u64 {
    t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()).unwrap_or(0)
}

pub fn list(data: &Path, ch: ChannelId, rel: &str) -> Result<Vec<FileEntry>, String> {
    let dir = resolve(data, ch, rel)?;
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(&dir) else { return Ok(out) };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if name.ends_with(".voxa-part") {
            continue;
        }
        if let Ok(m) = e.metadata() {
            out.push(FileEntry { name, is_dir: m.is_dir(), size: if m.is_dir() { 0 } else { m.len() }, modified: unix(m.modified()) });
        }
    }
    out.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    Ok(out)
}

pub fn delete(data: &Path, ch: ChannelId, rel: &str) -> Result<(), String> {
    let p = resolve(data, ch, rel)?;
    if p == channel_root(data, ch) {
        return Err("Invalid path".into());
    }
    let r = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
    r.map_err(|e| format!("Delete failed: {e}"))
}

pub fn mkdir(data: &Path, ch: ChannelId, rel: &str) -> Result<(), String> {
    let p = resolve(data, ch, rel)?;
    std::fs::create_dir_all(&p).map_err(|e| format!("Could not create folder: {e}"))
}

pub fn delete_channel_files(data: &Path, ch: ChannelId) {
    let _ = std::fs::remove_dir_all(channel_root(data, ch));
}

/// Total size of the files in a folder tree (links are not followed).
pub fn dir_size(p: &Path) -> u64 {
    let mut total = 0;
    let mut dirs = vec![p.to_path_buf()];
    while let Some(d) = dirs.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            // Unfinished uploads are counted through their reservation instead.
            if is_link(&path) || e.file_name().to_string_lossy().ends_with(".voxa-part") {
                continue;
            }
            match e.metadata() {
                Ok(m) if m.is_dir() => dirs.push(path),
                Ok(m) => total += m.len(),
                Err(_) => {}
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_refuses_unsafe_names() {
        let data = Path::new(r"C:\data");
        for bad in ["..", "a/../b", "C:x", "x:stream", "con", "NUL.txt", "com1", "lpt9.log", "x.", "x ", "a|b", "f.voxa-part"] {
            assert!(resolve(data, 1, bad).is_err(), "{bad:?}");
        }
        for ok in ["a.txt", "dir/sub/file", "console.log", "com10", r"\\server\share\x"] {
            assert!(resolve(data, 1, ok).unwrap().starts_with(r"C:\data\files\channel_1"), "{ok:?}");
        }
    }

    /// A junction in the channel folder (made by someone with access to the data
    /// folder) must not let uploads or deletes reach the place it points to.
    #[cfg(windows)]
    #[test]
    fn resolve_refuses_junctions() {
        let base = std::env::temp_dir().join(format!("voxa-junction-test-{}", std::process::id()));
        let target = base.join("outside");
        let root = channel_root(&base, 7);
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let link = root.join("link");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&target)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if made {
            assert!(resolve(&base, 7, "link/evil.txt").is_err());
            assert!(resolve(&base, 7, "link").is_err());
            assert!(resolve(&base, 7, "fine.txt").is_ok());
            let _ = std::fs::remove_dir(&link);
        }
        let _ = std::fs::remove_dir_all(&base);
        assert!(made, "could not create a junction for the test");
    }
}
