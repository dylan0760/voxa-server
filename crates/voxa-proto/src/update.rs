//! Self-updater backed by GitHub Releases.
//!
//! 1. `check` asks the GitHub API for the latest release of a repository and picks the
//!    MSI asset whose name matches the given prefix.
//! 2. `download` fetches it from github.com over HTTPS (Windows' native TLS) into a new
//!    private folder and verifies the SHA-256 digest GitHub publishes for the asset
//!    (an installer without a digest is refused).
//! 3. `install` launches `msiexec` with a progress-only UI; the installer stops the
//!    running app/service, upgrades it and relaunches it (`AUTOLAUNCH=1`).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub name: String,
    pub notes: String,
    pub page_url: String,
    pub asset_name: String,
    pub asset_url: String,
    pub size: u64,
    /// Lower-case hex SHA-256 if GitHub provides an asset digest.
    pub sha256: Option<String>,
}

fn agent() -> ureq::Agent {
    let tls = ureq::tls::TlsConfig::builder().provider(ureq::tls::TlsProvider::NativeTls).build();
    let cfg = ureq::Agent::config_builder()
        .tls_config(tls)
        .timeout_global(Some(Duration::from_secs(600)))
        .user_agent(format!("Voxa-Updater/{}", crate::APP_VERSION))
        .build();
    ureq::Agent::new_with_config(cfg)
}

fn parse_version(v: &str) -> Vec<u64> {
    v.trim()
        .trim_start_matches(['v', 'V'])
        .split(['-', '+'])
        .next()
        .unwrap_or("")
        .split('.')
        .map(|p| p.parse().unwrap_or(0))
        .collect()
}

/// True when `candidate` is a newer version than `current` (semantic versioning).
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (mut a, mut b) = (parse_version(candidate), parse_version(current));
    let n = a.len().max(b.len());
    a.resize(n, 0);
    b.resize(n, 0);
    a > b
}

/// Returns the latest release if it is newer than `current`, `None` when up to date.
/// `asset_matches` selects the MSI file for this product.
pub fn check(repo: &str, current: &str, asset_matches: impl Fn(&str) -> bool) -> Result<Option<Release>, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let mut resp = agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => "No releases have been published yet.".to_string(),
            e => format!("Could not reach GitHub: {e}"),
        })?;
    let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("Unexpected response from GitHub: {e}"))?;
    let tag = json["tag_name"].as_str().unwrap_or_default().to_string();
    if !is_newer(&tag, current) {
        return Ok(None);
    }
    let asset = json["assets"]
        .as_array()
        .and_then(|a| a.iter().find(|x| x["name"].as_str().map(&asset_matches).unwrap_or(false)))
        .ok_or_else(|| format!("Release {tag} has no installer for this product yet."))?;
    Ok(Some(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        name: json["name"].as_str().unwrap_or(&tag).to_string(),
        notes: json["body"].as_str().unwrap_or_default().to_string(),
        page_url: json["html_url"].as_str().unwrap_or_default().to_string(),
        asset_name: asset["name"].as_str().unwrap_or_default().to_string(),
        asset_url: asset["browser_download_url"].as_str().unwrap_or_default().to_string(),
        size: asset["size"].as_u64().unwrap_or(0),
        sha256: asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(|s| s.to_lowercase()),
    }))
}

/// Version of the newest published release that has an installer matching
/// `asset_matches` with a checksum (so it can actually be installed by the updater).
pub fn latest_version(repo: &str, asset_matches: impl Fn(&str) -> bool) -> Result<String, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases/latest");
    let mut resp = agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("Could not reach GitHub: {e}"))?;
    let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let tag = json["tag_name"].as_str().ok_or("Unexpected answer from GitHub")?;
    let installable = json["assets"].as_array().map(|a| {
        a.iter().any(|x| x["name"].as_str().map(&asset_matches).unwrap_or(false) && x["digest"].as_str().map(|d| d.starts_with("sha256:")).unwrap_or(false))
    });
    if installable != Some(true) {
        return Err(format!("Release {tag} has no installer yet"));
    }
    Ok(tag.trim_start_matches(['v', 'V']).to_string())
}

/// Release notes of one specific version (`/releases/tags/v<version>`), used to show
/// "What's new" after an update. Returns `(title, notes, page url)`.
pub fn release_notes(repo: &str, version: &str) -> Result<(String, String, String), String> {
    let tag = format!("v{}", version.trim_start_matches(['v', 'V']));
    let url = format!("https://api.github.com/repos/{repo}/releases/tags/{tag}");
    let mut resp = agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| match e {
            ureq::Error::StatusCode(404) => format!("No release notes published for {tag}"),
            e => format!("Could not reach GitHub: {e}"),
        })?;
    let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok((
        json["name"].as_str().unwrap_or(&tag).to_string(),
        json["body"].as_str().unwrap_or_default().to_string(),
        json["html_url"].as_str().unwrap_or_default().to_string(),
    ))
}

/// One published release, for the change log history.
#[derive(Clone, Debug, PartialEq)]
pub struct ReleaseNote {
    /// Without the leading "v".
    pub version: String,
    pub title: String,
    pub notes: String,
    pub url: String,
    /// "2026-11-13" (empty if unknown).
    pub date: String,
}

/// The newest `limit` published releases (no drafts / pre-releases), newest first.
pub fn release_history(repo: &str, limit: usize) -> Result<Vec<ReleaseNote>, String> {
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page={}", limit.clamp(1, 100));
    let mut resp = agent()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("Could not reach GitHub: {e}"))?;
    let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let mut out: Vec<ReleaseNote> = json
        .as_array()
        .ok_or("Unexpected answer from GitHub")?
        .iter()
        .filter(|r| !r["draft"].as_bool().unwrap_or(false) && !r["prerelease"].as_bool().unwrap_or(false))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?;
            let version = tag.trim_start_matches(['v', 'V']).to_string();
            Some(ReleaseNote {
                title: r["name"].as_str().filter(|s| !s.is_empty()).unwrap_or(tag).to_string(),
                notes: r["body"].as_str().unwrap_or_default().to_string(),
                url: r["html_url"].as_str().unwrap_or_default().to_string(),
                date: r["published_at"].as_str().unwrap_or_default().chars().take(10).collect(),
                version,
            })
        })
        .collect();
    // Newest version first (by number, not by publishing time).
    out.sort_by(|a, b| {
        let (x, y) = (parse_version(&a.version), parse_version(&b.version));
        y.cmp(&x)
    });
    Ok(out)
}

/// Downloads the release installer, reporting `(downloaded, total)` bytes.
pub fn download(rel: &Release, progress: impl Fn(u64, u64)) -> Result<PathBuf, String> {
    if !rel.asset_url.starts_with("https://github.com/") {
        return Err("Refusing to download an update from outside github.com".into());
    }
    let Some(expected) = rel.sha256.as_deref().filter(|d| d.len() == 64 && d.chars().all(|c| c.is_ascii_hexdigit())) else {
        return Err("This release has no checksum, so it cannot be verified. Download it from the release page instead.".into());
    };
    // A new folder with a random name: nothing can be planted there beforehand.
    let tag = format!("{:016x}", rand::random::<u64>());
    let dir = std::env::temp_dir().join(format!("voxa-update-{tag}"));
    std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
    let safe_name: String = rel.asset_name.chars().filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c)).collect();
    let path = dir.join(if safe_name.ends_with(".msi") { safe_name } else { "voxa-update.msi".into() });
    let resp = agent().get(&rel.asset_url).call().map_err(|e| format!("Download failed: {e}"))?;
    let mut reader = resp.into_body().into_with_config().limit(1024 * 1024 * 1024).reader();
    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("Download failed: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        hasher.update(&buf[..n]);
        done += n as u64;
        progress(done, rel.size.max(done));
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    if rel.size > 0 && done != rel.size {
        let _ = std::fs::remove_file(&path);
        return Err("Download incomplete".into());
    }
    let got: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if !got.eq_ignore_ascii_case(expected) {
        let _ = std::fs::remove_file(&path);
        return Err("The downloaded installer failed its integrity check".into());
    }
    Ok(path)
}

/// Starts the installer. The caller should exit afterwards so files can be replaced.
pub fn install(msi: &Path) -> Result<(), String> {
    // The system's own msiexec, not whatever a search of the current folder / PATH finds.
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| "C:\\Windows".into());
    std::process::Command::new(system.join("System32").join("msiexec.exe"))
        .arg("/i")
        .arg(msi)
        .arg("/passive")
        .arg("AUTOLAUNCH=1")
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Could not start the installer: {e}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn versions() {
        assert!(super::is_newer("v1.2.0", "1.1.9"));
        assert!(super::is_newer("1.10.0", "1.9.0"));
        assert!(!super::is_newer("v1.1.0", "1.1.0"));
        assert!(!super::is_newer("1.0.9", "1.1.0"));
        assert!(super::is_newer("2", "1.9.9"));
    }
}
