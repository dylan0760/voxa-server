//! Per-channel file storage (`<data>\files\channel_<id>\...`).

use std::path::{Path, PathBuf};

use voxa_proto::{ChannelId, FileEntry};

pub fn channel_root(data: &Path, ch: ChannelId) -> PathBuf {
    data.join("files").join(format!("channel_{ch}"))
}

/// Resolves a client supplied relative path safely inside the channel root.
pub fn resolve(data: &Path, ch: ChannelId, rel: &str) -> Result<PathBuf, String> {
    let mut p = channel_root(data, ch);
    for part in rel.split(['/', '\\']).filter(|s| !s.is_empty()) {
        if part == "." || part == ".." || part.contains(':') || part.chars().any(|c| c.is_control() || "<>\"|?*".contains(c)) {
            return Err("Invalid path".into());
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return Err("Invalid file name".into());
        }
        p.push(part);
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

pub fn dir_size(p: &Path) -> u64 {
    let Ok(rd) = std::fs::read_dir(p) else { return 0 };
    rd.flatten()
        .map(|e| match e.metadata() {
            Ok(m) if m.is_dir() => dir_size(&e.path()),
            Ok(m) => m.len(),
            Err(_) => 0,
        })
        .sum()
}
