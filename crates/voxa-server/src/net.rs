use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::AsyncWriteExt;
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify};
use tokio::time::timeout;
use tracing::{debug, info};
use voxa_proto::traffic::{self, Cat, TRAFFIC};
use voxa_proto::identity::uid_from_public;
use voxa_proto::transport::{self, SecureReader, SecureWriter};
use voxa_proto::*;

use crate::state::{Out, Server};

/// Open TCP connections at most (every client uses two).
const MAX_CONNECTIONS: usize = 4096;
/// Open TCP connections per remote address (the local machine is not limited). Every
/// client uses two, and several people can share one address behind a router.
const MAX_PER_IP: u32 = 64;
/// Largest first message (Hello / AttachBulk) before a client is logged in.
const MAX_FIRST_MESSAGE: usize = 64 * 1024;
/// Largest message on a logged-in control connection (chat text is at most 8000
/// characters; files and images go over the bulk connection).
const MAX_CONTROL_MESSAGE: usize = 1024 * 1024;
/// Undecodable messages in a row before a control connection is dropped.
const MAX_BAD_MESSAGES: u32 = 20;

/// Counts open connections per address; a slot is freed when its guard is dropped.
#[derive(Default)]
struct PerIp(parking_lot::Mutex<HashMap<std::net::IpAddr, u32>>);

struct IpSlot(Arc<PerIp>, std::net::IpAddr);

impl PerIp {
    fn take(self: &Arc<Self>, ip: std::net::IpAddr) -> Option<IpSlot> {
        if ip.is_loopback() {
            return Some(IpSlot(self.clone(), ip));
        }
        let mut m = self.0.lock();
        let n = m.entry(ip).or_insert(0);
        if *n >= MAX_PER_IP {
            return None;
        }
        *n += 1;
        Some(IpSlot(self.clone(), ip))
    }
}

impl Drop for IpSlot {
    fn drop(&mut self) {
        if self.1.is_loopback() {
            return;
        }
        let mut m = (self.0).0.lock();
        if let Some(n) = m.get_mut(&self.1) {
            *n -= 1;
            if *n == 0 {
                m.remove(&self.1);
            }
        }
    }
}

pub async fn run(server: Arc<Server>, listener: TcpListener) {
    let total = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    let per_ip = Arc::new(PerIp::default());
    let mut last_warn: Option<std::time::Instant> = None;
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let ip = addr.ip().to_canonical();
                let (Ok(permit), Some(slot)) = (total.clone().try_acquire_owned(), per_ip.take(ip)) else {
                    // Dropping the stream closes it at once.
                    if last_warn.map(|t| t.elapsed() > Duration::from_secs(60)).unwrap_or(true) {
                        tracing::warn!("too many connections (from {ip}); refusing new ones");
                        last_warn = Some(std::time::Instant::now());
                    }
                    continue;
                };
                let server = server.clone();
                tokio::spawn(async move {
                    let _held = (permit, slot);
                    if let Err(e) = handle(server, stream, addr).await {
                        debug!("connection {addr} ended: {e:#}");
                    }
                });
            }
            Err(e) => {
                tracing::warn!("accept failed: {e}");
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        }
    }
}

async fn handle(server: Arc<Server>, stream: TcpStream, addr: SocketAddr) -> anyhow::Result<()> {
    stream.set_nodelay(true)?;
    let (r, w) = stream.into_split();
    let sec = timeout(Duration::from_secs(10), transport::accept(r, w, &server.identity.private)).await??;
    let uid = uid_from_public(&sec.remote_static);
    let mut reader = sec.reader;
    let mut writer = sec.writer;
    reader.set_max_message(MAX_FIRST_MESSAGE);
    let first: C2S = timeout(Duration::from_secs(10), reader.recv()).await??;
    TRAFFIC.received(Cat::Chat, reader.last_size);
    match first {
        C2S::AttachBulk(token) => {
            reader.set_max_message(transport::MAX_MESSAGE);
            bulk(server, reader, writer, uid, token).await
        }
        C2S::Hello { ref version, ref server_password, ref default_channel, ref default_channel_password, .. } => {
            if let Err(e) = server.check_client_version(version) {
                info!("refused connection from {addr}: client {} is too old", state_trunc(version));
                writer.send(&S2C::Refused(e)).await?;
                return Ok(());
            }
            // Password checks are slow on purpose; they run before the state is locked.
            let checked = server
                .check_login_passwords(addr.ip(), server_password.as_deref(), default_channel.as_deref(), default_channel_password.as_deref())
                .await;
            if let Err(e) = checked {
                info!("refused connection from {addr}: {e}");
                writer.send(&S2C::Refused(e)).await?;
                return Ok(());
            }
            reader.set_max_message(MAX_CONTROL_MESSAGE);
            control(server, reader, writer, uid, addr, first).await
        }
        _ => {
            writer.send(&S2C::Refused("Expected hello".into())).await?;
            Ok(())
        }
    }
}

async fn control(
    server: Arc<Server>,
    mut reader: SecureReader<OwnedReadHalf>,
    mut writer: SecureWriter<OwnedWriteHalf>,
    uid: String,
    addr: SocketAddr,
    hello: C2S,
) -> anyhow::Result<()> {
    let (tx, mut rx) = mpsc::channel::<Out>(crate::state::OUTBOX_SIZE);
    let close = Arc::new(Notify::new());
    let outbox = crate::state::Outbox { tx, close: close.clone() };
    let id = match server.login(uid, addr.ip(), hello, outbox, close.clone()) {
        Ok(id) => id,
        Err(e) => {
            info!("refused connection from {addr}: {e}");
            writer.send(&S2C::Refused(e)).await?;
            return Ok(());
        }
    };
    tokio::spawn(async move {
        while let Some(o) = rx.recv().await {
            match o {
                Out::Msg(m) => {
                    if writer.send(&m).await.is_err() {
                        break;
                    }
                    TRAFFIC.sent(Cat::Chat, writer.last_size);
                }
                Out::Close => break,
            }
        }
    });
    let mut bad = 0u32;
    let reason = loop {
        tokio::select! {
            r = timeout(Duration::from_secs(45), reader.recv::<C2S>()) => match r.inspect(|m| if m.is_ok() { TRAFFIC.received(Cat::Chat, reader.last_size) }) {
                Ok(Ok(C2S::Disconnect(msg))) => break LeaveReason::Disconnected(state_trunc(&msg)),
                Ok(Ok(msg)) => {
                    bad = 0;
                    // Password checks are slow on purpose; they run before the state is locked.
                    if let Err(e) = server.prepare(id, &msg).await {
                        server.state.lock().send(id, S2C::Error(e));
                        continue;
                    }
                    server.handle(id, msg)
                }
                // A message this server does not know (e.g. from a newer client) is skipped.
                Ok(Err(transport::TransportError::Codec(e))) if bad < MAX_BAD_MESSAGES => {
                    bad += 1;
                    debug!("client {id}: skipped an unreadable message: {e}");
                }
                Ok(Err(_)) => break LeaveReason::Disconnected("Connection lost".into()),
                Err(_) => break LeaveReason::TimedOut,
            },
            _ = close.notified() => break LeaveReason::Disconnected(String::new()),
        }
    };
    server.remove_session(id, reason);
    Ok(())
}

fn state_trunc(s: &str) -> String {
    crate::state::clean_line(s, 120)
}

/// Uploads one bulk connection may run at the same time.
const MAX_UPLOADS: usize = 8;
/// Chat images one bulk connection may have in work (posted or fetched) at once.
const MAX_IMAGE_JOBS: usize = 4;

struct Upload {
    file: tokio::fs::File,
    temp: PathBuf,
    target: PathBuf,
    remaining: u64,
    _space: Reservation,
}

/// Disk space promised to a running upload, so parallel uploads cannot exceed the
/// channel quota together. Released when the upload ends in any way.
struct Reservation {
    server: Arc<Server>,
    channel: ChannelId,
    bytes: u64,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        let mut r = self.server.reserved.lock();
        if let Some(n) = r.get_mut(&self.channel) {
            *n = n.saturating_sub(self.bytes);
            if *n == 0 {
                r.remove(&self.channel);
            }
        }
    }
}

async fn bulk(
    server: Arc<Server>,
    mut reader: SecureReader<OwnedReadHalf>,
    mut writer: SecureWriter<OwnedWriteHalf>,
    uid: String,
    token: [u8; 32],
) -> anyhow::Result<()> {
    let (btx, mut brx) = mpsc::channel::<BulkS2C>(48);
    let frames = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let Some(id) = server.attach_bulk(&token, &uid, btx.clone(), frames.clone()) else {
        return Ok(());
    };
    writer.send(&BulkS2C::Attached).await?;
    TRAFFIC.sent(Cat::Chat, writer.last_size);
    tokio::spawn(async move {
        while let Some(m) = brx.recv().await {
            if matches!(m, BulkS2C::Frame { .. }) {
                frames.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
            }
            if writer.send(&m).await.is_err() {
                break;
            }
            TRAFFIC.sent(traffic::bulk_s2c(&m), writer.last_size);
        }
    });
    let mut uploads: HashMap<u32, Upload> = HashMap::new();
    let image_jobs = Arc::new(tokio::sync::Semaphore::new(MAX_IMAGE_JOBS));
    let mut downloads: HashMap<u32, tokio::task::AbortHandle> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        let msg = tokio::select! {
            m = reader.recv::<BulkC2S>() => match m {
                Ok(m) => {
                    TRAFFIC.received(traffic::bulk_c2s(&m), reader.last_size);
                    m
                }
                Err(_) => break,
            },
            _ = tick.tick() => {
                if !server.state.lock().sessions.contains_key(&id) { break; }
                continue;
            }
        };
        match msg {
            BulkC2S::Frame(f) => server.relay_frame(id, f),
            BulkC2S::RequestKeyframe(streamer) => server.request_keyframe(streamer),
            BulkC2S::StreamAudio(data) => server.relay_audio(id, data),
            BulkC2S::UploadBegin { id: tid, channel, path, size, overwrite } => {
                let res = if uploads.len() >= MAX_UPLOADS {
                    Err(format!("At most {MAX_UPLOADS} uploads can run at the same time"))
                } else {
                    begin_upload(&server, id, channel, &path, size, overwrite).await
                };
                match res {
                    Ok(u) => {
                        uploads.insert(tid, u);
                        let _ = btx.send(BulkS2C::UploadReady { id: tid }).await;
                    }
                    Err(e) => {
                        let _ = btx.send(BulkS2C::TransferError { id: tid, message: e }).await;
                    }
                }
            }
            BulkC2S::UploadChunk { id: tid, data } => {
                let mut failed = None;
                if let Some(u) = uploads.get_mut(&tid) {
                    if data.len() as u64 > u.remaining {
                        failed = Some("File larger than announced".to_string());
                    } else if let Err(e) = u.file.write_all(&data).await {
                        failed = Some(format!("Write failed: {e}"));
                    } else {
                        u.remaining -= data.len() as u64;
                    }
                }
                if let Some(msg) = failed {
                    if let Some(u) = uploads.remove(&tid) {
                        drop(u.file);
                        let _ = tokio::fs::remove_file(&u.temp).await;
                    }
                    let _ = btx.send(BulkS2C::TransferError { id: tid, message: msg }).await;
                }
            }
            BulkC2S::UploadEnd { id: tid } => {
                if let Some(mut u) = uploads.remove(&tid) {
                    let _ = u.file.flush().await;
                    drop(u.file);
                    let res = if u.remaining != 0 {
                        Err("Upload incomplete".to_string())
                    } else {
                        let _ = tokio::fs::remove_file(&u.target).await;
                        tokio::fs::rename(&u.temp, &u.target).await.map_err(|e| e.to_string())
                    };
                    match res {
                        Ok(()) => {
                            let _ = btx.send(BulkS2C::UploadDone { id: tid }).await;
                        }
                        Err(e) => {
                            let _ = tokio::fs::remove_file(&u.temp).await;
                            let _ = btx.send(BulkS2C::TransferError { id: tid, message: e }).await;
                        }
                    }
                }
            }
            BulkC2S::Download { id: tid, channel, path } => {
                if let Err(e) = server.file_perm(id, channel, perm::FILE_DOWNLOAD) {
                    let _ = btx.send(BulkS2C::TransferError { id: tid, message: e }).await;
                    continue;
                }
                let p = match crate::files::resolve(&server.data_dir, channel, &path) {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = btx.send(BulkS2C::TransferError { id: tid, message: e }).await;
                        continue;
                    }
                };
                let tx = btx.clone();
                let h = tokio::spawn(async move {
                    if let Err(e) = send_file(&tx, tid, p).await {
                        let _ = tx.send(BulkS2C::TransferError { id: tid, message: e }).await;
                    }
                });
                downloads.insert(tid, h.abort_handle());
            }
            BulkC2S::Cancel { id: tid } => {
                if let Some(u) = uploads.remove(&tid) {
                    drop(u.file);
                    let _ = tokio::fs::remove_file(&u.temp).await;
                }
                if let Some(h) = downloads.remove(&tid) {
                    h.abort();
                }
            }
            BulkC2S::ChatImage { id: tid, target, name, width, height, data } => {
                // Waiting for a slot here also stops reading more data from this client.
                let Ok(slot) = image_jobs.clone().acquire_owned().await else { break };
                let (srv, tx) = (server.clone(), btx.clone());
                tokio::spawn(async move {
                    let _slot = slot;
                    let r = tokio::task::spawn_blocking(move || srv.post_image(id, target, &name, width, height, data))
                        .await
                        .unwrap_or_else(|e| Err(e.to_string()));
                    let _ = tx
                        .send(match r {
                            Ok(()) => BulkS2C::ChatImageSent { id: tid },
                            Err(message) => BulkS2C::TransferError { id: tid, message },
                        })
                        .await;
                });
            }
            BulkC2S::GetImage(image) => {
                let Ok(slot) = image_jobs.clone().acquire_owned().await else { break };
                let (srv, tx) = (server.clone(), btx.clone());
                tokio::spawn(async move {
                    // Held until the image is queued for sending, so only a few decrypted
                    // images can wait in memory per connection.
                    let _slot = slot;
                    let r = tokio::task::spawn_blocking(move || srv.chat_image(id, image)).await.unwrap_or_else(|e| Err(e.to_string()));
                    let _ = tx
                        .send(match r {
                            Ok(data) => BulkS2C::Image { image, data },
                            Err(message) => BulkS2C::ImageError { image, message },
                        })
                        .await;
                });
            }
        }
        downloads.retain(|_, h| !h.is_finished());
    }
    for (_, u) in uploads {
        drop(u.file);
        let _ = tokio::fs::remove_file(&u.temp).await;
    }
    for (_, h) in downloads {
        h.abort();
    }
    server.detach_bulk(id, &btx);
    Ok(())
}

async fn begin_upload(
    server: &Arc<Server>,
    me: ClientId,
    channel: ChannelId,
    path: &str,
    size: u64,
    overwrite: bool,
) -> Result<Upload, String> {
    server.file_perm(me, channel, perm::FILE_UPLOAD)?;
    let limits = server.state.lock().file_limits;
    const MB: u64 = 1024 * 1024;
    if limits.max_upload_mb > 0 && size > limits.max_upload_mb.saturating_mul(MB) {
        return Err(format!("File too large (server limit {} MB)", limits.max_upload_mb));
    }
    let target = crate::files::resolve(&server.data_dir, channel, path)?;
    if target == crate::files::channel_root(&server.data_dir, channel) {
        return Err("Invalid file name".into());
    }
    if target.exists() {
        if !overwrite {
            return Err("A file with this name already exists".into());
        }
        // Replacing a file destroys it, so it needs the right to delete files too.
        server.file_perm(me, channel, perm::FILE_DELETE)?;
    }
    let used = if limits.channel_quota_mb > 0 {
        let root = crate::files::channel_root(&server.data_dir, channel);
        tokio::task::spawn_blocking(move || crate::files::dir_size(&root)).await.unwrap_or(0)
    } else {
        0
    };
    let space = {
        let mut r = server.reserved.lock();
        let pending = r.get(&channel).copied().unwrap_or(0);
        if limits.channel_quota_mb > 0 && used.saturating_add(pending).saturating_add(size) > limits.channel_quota_mb.saturating_mul(MB) {
            return Err(format!("Channel file quota exceeded ({} MB per channel)", limits.channel_quota_mb));
        }
        *r.entry(channel).or_insert(0) += size;
        Reservation { server: server.clone(), channel, bytes: size }
    };
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    }
    let mut temp = target.clone().into_os_string();
    temp.push(".voxa-part");
    let temp = PathBuf::from(temp);
    let file = tokio::fs::File::create(&temp).await.map_err(|e| format!("Cannot create file: {e}"))?;
    Ok(Upload { file, temp, target, remaining: size, _space: space })
}

async fn send_file(tx: &mpsc::Sender<BulkS2C>, id: u32, path: PathBuf) -> Result<(), String> {
    use tokio::io::AsyncReadExt;
    let mut f = tokio::fs::File::open(&path).await.map_err(|_| "File not found".to_string())?;
    let size = f.metadata().await.map_err(|e| e.to_string())?.len();
    tx.send(BulkS2C::DownloadBegin { id, size }).await.map_err(|_| "closed")?;
    let mut buf = vec![0u8; FILE_CHUNK];
    loop {
        let n = f.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        tx.send(BulkS2C::DownloadChunk { id, data: buf[..n].to_vec() }).await.map_err(|_| "closed")?;
    }
    tx.send(BulkS2C::DownloadEnd { id }).await.map_err(|_| "closed")?;
    Ok(())
}
