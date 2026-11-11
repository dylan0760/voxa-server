use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(default)]
pub struct Config {
    /// Address to bind TCP (control, bulk) and UDP (voice) sockets to.
    pub bind: String,
    pub port: u16,
    /// Default maximum size of a single uploaded file in megabytes (0 = unlimited).
    /// Only used until an administrator changes it in the client (Edit Virtual Server).
    pub max_upload_mb: u64,
    /// Default maximum total size of all files per channel in megabytes (0 = unlimited).
    /// Only used until an administrator changes it in the client.
    pub channel_quota_mb: u64,
    /// Log level: error, warn, info, debug, trace.
    pub log_level: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            bind: "0.0.0.0".into(),
            port: voxa_proto::DEFAULT_PORT,
            max_upload_mb: 2048,
            channel_quota_mb: 0,
            log_level: "info".into(),
        }
    }
}

impl Config {
    pub fn load_or_create(dir: &Path) -> anyhow::Result<Config> {
        let path = dir.join("server.toml");
        if path.exists() {
            let text = std::fs::read_to_string(&path)?;
            Ok(toml::from_str(&text)?)
        } else {
            let cfg = Config::default();
            let text = format!(
                "# Voxa server configuration. Restart the server after editing.\n{}",
                toml::to_string_pretty(&cfg)?
            );
            std::fs::write(&path, text)?;
            Ok(cfg)
        }
    }
}

/// `%ProgramData%\Voxa Server` on Windows, `./voxa-data` elsewhere. Can be overridden
/// with `--data <dir>` or the `VOXA_DATA` environment variable.
pub fn default_data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("VOXA_DATA") {
        return PathBuf::from(d);
    }
    if cfg!(windows) {
        let base = std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into());
        PathBuf::from(base).join("Voxa Server")
    } else {
        PathBuf::from("voxa-data")
    }
}
