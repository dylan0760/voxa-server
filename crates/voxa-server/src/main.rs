//! Voxa Server - free and open source voice, chat and screen sharing server.

#[cfg(windows)]
mod acl;
mod config;
mod files;
mod net;
mod secrets;
mod state;
mod store;
mod voice;
#[cfg(windows)]
mod service;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use tracing::{error, info};
use voxa_proto::identity::KeyPair;

use crate::config::Config;
use crate::state::Server;

const HELP: &str = "Voxa Server

USAGE:
    voxa-server [OPTIONS]

OPTIONS:
    --data <DIR>     Data directory (default: %ProgramData%\\Voxa Server)
    --service        Run as a Windows service (used by the service manager)
    --check-update   Check GitHub for a newer Voxa Server release
    --update         Download and install the newest release (run as administrator)
    --clear-bans     Remove every ban (stop the server first; run as administrator)
    --version        Print version
    --help           Print this help

Network ports (default):
    TCP 9987  control connection, encrypted chat, file transfer and screen sharing
    UDP 9987  encrypted voice

Configuration is read from server.toml in the data directory.";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h" || a == "/?") {
        println!("{HELP}");
        return;
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("voxa-server {}", voxa_proto::APP_VERSION);
        return;
    }
    if args.iter().any(|a| a == "--check-update" || a == "--update") {
        std::process::exit(cli_update(args.iter().any(|a| a == "--update")));
    }
    let data_dir = args
        .iter()
        .position(|a| a == "--data")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(config::default_data_dir);

    if args.iter().any(|a| a == "--clear-bans") {
        std::process::exit(match clear_bans(&data_dir) {
            Ok(n) => {
                println!("Removed {n} ban(s).");
                0
            }
            Err(e) => {
                eprintln!("Could not clear the bans: {e:#}\nStop Voxa Server first and run this as administrator.");
                1
            }
        });
    }

    if args.iter().any(|a| a == "--service") {
        #[cfg(windows)]
        {
            service::DATA_DIR.set(data_dir).ok();
            if let Err(e) = service::run() {
                eprintln!("service error: {e}");
                std::process::exit(1);
            }
            return;
        }
        #[cfg(not(windows))]
        {
            eprintln!("--service is only supported on Windows");
            std::process::exit(2);
        }
    }

    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let result = rt.block_on(async {
        run(data_dir, true, async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
    });
    if let Err(e) = result {
        eprintln!("fatal: {e:#}");
        error!("fatal: {e:#}");
        std::process::exit(1);
    }
}

/// GitHub repository of the Voxa client (override with VOXA_CLIENT_REPO at build time).
const CLIENT_REPO: &str = match option_env!("VOXA_CLIENT_REPO") {
    Some(r) => r,
    None => "dylan0760/voxa-client",
};

/// Raises the required client version when a newer Voxa client is published (one
/// whose installer is attached, so clients can actually update). Checks every 30 minutes;
/// without internet the known version stays.
async fn watch_client_releases(server: Arc<Server>) {
    loop {
        let r = tokio::task::spawn_blocking(|| {
            voxa_proto::update::latest_version(CLIENT_REPO, |n| n.starts_with("Voxa-") && !n.starts_with("Voxa-Server") && n.ends_with("-x64.msi"))
        })
        .await;
        match r {
            Ok(Ok(latest)) => {
                let mut min = server.min_client.lock();
                if voxa_proto::update::is_newer(&latest, &min) {
                    info!("Voxa {latest} was released: clients must now be Voxa {latest} or newer");
                    *min = latest;
                }
            }
            Ok(Err(e)) => tracing::debug!("client release check: {e}"),
            Err(_) => {}
        }
        tokio::time::sleep(Duration::from_secs(30 * 60)).await;
    }
}

/// `--clear-bans`: removes every ban (for an administrator who locked themselves out).
fn clear_bans(data_dir: &Path) -> anyhow::Result<usize> {
    let master = secrets::load_or_create(&data_dir.join("chat.key"), || secrets::random_bytes::<32>().to_vec())?;
    let store = store::Store::open(&data_dir.join("voxa.db"), &master)?;
    let bans = store.bans()?;
    for b in &bans {
        store.remove_ban(b.id)?;
    }
    Ok(bans.len())
}

/// GitHub repository the server updates from (override with VOXA_UPDATE_REPO at build time).
const UPDATE_REPO: &str = match option_env!("VOXA_UPDATE_REPO") {
    Some(r) => r,
    None => "dylan0760/voxa-server",
};

/// `--check-update` reports whether a newer release exists; `--update` installs it.
fn cli_update(install: bool) -> i32 {
    use voxa_proto::update;
    println!("Voxa Server {} - checking github.com/{UPDATE_REPO} for updates...", voxa_proto::APP_VERSION);
    let rel = match update::check(UPDATE_REPO, voxa_proto::APP_VERSION, |n| n.starts_with("Voxa-Server-") && n.ends_with("-x64.msi")) {
        Ok(Some(r)) => r,
        Ok(None) => {
            println!("You are running the latest version.");
            return 0;
        }
        Err(e) => {
            eprintln!("Update check failed: {e}");
            return 1;
        }
    };
    println!("Version {} is available: {}", rel.version, rel.page_url);
    if !install {
        println!("Run `voxa-server.exe --update` (as administrator) to install it.");
        return 0;
    }
    println!("Downloading {} ...", rel.asset_name);
    let path = match update::download(&rel, |done, total| {
        if total > 0 {
            print!("\r  {:>3}%", done * 100 / total);
            let _ = std::io::stdout().flush();
        }
    }) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("\n{e}");
            return 1;
        }
    };
    println!("\nStarting the installer (the service restarts automatically)...");
    match update::install(&path) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

#[derive(Clone)]
struct LogWriter {
    file: Arc<parking_lot::Mutex<std::fs::File>>,
    console: bool,
}

impl Write for LogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self.console {
            let _ = std::io::stdout().write_all(buf);
        }
        self.file.lock().write_all(buf)?;
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.lock().flush()
    }
}

fn init_logging(dir: &Path, level: &str, console: bool) -> anyhow::Result<()> {
    let logs = dir.join("logs");
    std::fs::create_dir_all(&logs)?;
    let path = logs.join("server.log");
    // Simple rotation: keep one previous log once it grows beyond 10 MB.
    if std::fs::metadata(&path).map(|m| m.len() > 10 * 1024 * 1024).unwrap_or(false) {
        let _ = std::fs::rename(&path, logs.join("server.old.log"));
    }
    let file = std::fs::OpenOptions::new().create(true).append(true).open(&path)?;
    let w = LogWriter { file: Arc::new(parking_lot::Mutex::new(file)), console };
    let level: tracing::Level = level.parse().unwrap_or(tracing::Level::INFO);
    let _ = tracing_subscriber::fmt().with_max_level(level).with_ansi(false).with_writer(move || w.clone()).try_init();
    Ok(())
}

pub async fn run(data_dir: PathBuf, console: bool, shutdown: impl std::future::Future<Output = ()>) -> anyhow::Result<()> {
    std::fs::create_dir_all(&data_dir)?;
    let cfg = Config::load_or_create(&data_dir)?;
    init_logging(&data_dir, &cfg.log_level, console)?;
    info!("Voxa Server {} starting, data directory {}", voxa_proto::APP_VERSION, data_dir.display());

    let id_private = secrets::load_or_create(&data_dir.join("identity.key"), || KeyPair::generate().private)?;
    let identity = KeyPair::import(&base64_std(&id_private)).ok_or_else(|| anyhow::anyhow!("invalid identity key"))?;
    let master = secrets::load_or_create(&data_dir.join("chat.key"), || secrets::random_bytes::<32>().to_vec())?;
    let store = store::Store::open(&data_dir.join("voxa.db"), &master)?;
    let server = Arc::new(Server::new(cfg.clone(), data_dir.clone(), store, identity)?);
    info!("server identity fingerprint: {}", server.fingerprint);
    // As the Windows service (SYSTEM) the data folder must not be writable by users.
    #[cfg(windows)]
    if !console {
        let dir = data_dir.clone();
        let _ = tokio::task::spawn_blocking(move || acl::lock_down(&dir)).await;
    }

    let bind = format!("{}:{}", cfg.bind, cfg.port);
    let tcp = tokio::net::TcpListener::bind(&bind).await?;
    let udp = tokio::net::UdpSocket::bind(&bind).await?;
    info!("listening on TCP {bind} (control, chat, files, screen share) and UDP {bind} (voice)");

    tokio::spawn(net::run(server.clone(), tcp));
    if cfg.require_latest_client {
        info!("clients must be Voxa {} or newer (require_latest_client = true in server.toml)", server.min_client.lock());
        tokio::spawn(watch_client_releases(server.clone()));
    }
    tokio::spawn(voice::run(server.clone(), udp));
    let purge = server.clone();
    tokio::spawn(async move {
        let mut t = tokio::time::interval(Duration::from_secs(3600));
        loop {
            t.tick().await;
            let s = purge.clone();
            let _ = tokio::task::spawn_blocking(move || {
                s.purge_chat();
                s.purge_offline();
            })
            .await;
        }
    });

    // Status snapshot for the Voxa Server Manager (tray app).
    let status_srv = server.clone();
    let status_path = data_dir.join("status.json");
    let port = cfg.port;
    tokio::spawn(async move {
        // Written only when something changed, plus a heartbeat once a minute. Rewriting
        // the file every few seconds makes antivirus software rescan it each time.
        let mut t = tokio::time::interval(Duration::from_secs(3));
        // Traffic totals change while people talk / transfer, so the file is then
        // refreshed every 3 s; while idle nothing changes and nothing is written.
        let mut last: Option<(usize, usize, voxa_proto::traffic::TrafficStats)> = None;
        let mut last_write = std::time::Instant::now();
        loop {
            t.tick().await;
            let (online, channels) = {
                let st = status_srv.state.lock();
                (st.sessions.len(), st.channels.len())
            };
            let tr = voxa_proto::traffic::TRAFFIC.snapshot();
            if last == Some((online, channels, tr)) && last_write.elapsed() < Duration::from_secs(60) {
                continue;
            }
            last = Some((online, channels, tr));
            last_write = std::time::Instant::now();
            let json = format!(
                "{{\"running\":true,\"version\":\"{}\",\"port\":{},\"online\":{},\"channels\":{},\"started\":{},\"updated\":{},\"fingerprint\":\"{}\",\"traffic\":{{\"voice_in\":{},\"voice_out\":{},\"files_in\":{},\"files_out\":{},\"screen_in\":{},\"screen_out\":{},\"chat_in\":{},\"chat_out\":{}}}}}",
                voxa_proto::APP_VERSION,
                port,
                online,
                channels,
                status_srv.started,
                voxa_proto::now_unix(),
                status_srv.fingerprint,
                tr.voice_in,
                tr.voice_out,
                tr.files_in,
                tr.files_out,
                tr.screen_in,
                tr.screen_out,
                tr.chat_in,
                tr.chat_out
            );
            let _ = std::fs::write(&status_path, json);
        }
    });

    shutdown.await;
    info!("shutting down");
    let _ = std::fs::remove_file(data_dir.join("status.json"));
    let ids: Vec<u16> = server.state.lock().sessions.keys().copied().collect();
    for id in ids {
        server.kick_session(id, voxa_proto::LeaveReason::Disconnected("Server shutting down".into()));
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    Ok(())
}

fn base64_std(b: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(b)
}
