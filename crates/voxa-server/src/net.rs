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
    let id = match server.login(uid, addr.ip(), hello, outbox, close.clone(), false) {
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
                // GIF searches and sends wait on the GIF service: in their own task (1.27.0+).
                Ok(Ok(msg @ (C2S::SearchGifs { .. } | C2S::SendGif { .. } | C2S::ServerGifs { .. } | C2S::RemoveServerGif(_)))) => {
                    bad = 0;
                    let (srv,lease)=(server.clone(),close.clone());
                    tokio::spawn(async move { let _lease=lease; srv.gif_message(id,msg).await; });
                }
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

pub(crate) struct Upload {
    _slot: crate::state::TransferSlot,
    file: Option<tokio::fs::File>,
    temp: PathBuf,
    target: PathBuf,
    remaining: u64,
    relative: String,
    overwrite: bool,
    _space: Reservation,
}

impl Drop for Upload {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = std::fs::remove_file(&self.temp);
    }
}

impl Upload {
    pub(crate) async fn chunk(&mut self, server: &Server, me: ClientId, data: &[u8]) -> Result<(), String> {
        if data.len() > FILE_CHUNK || data.len() as u64 > self.remaining {
            return Err("File larger than announced".into());
        }
        let rate = server.state.lock().sessions.get(&me).map(|s| s.upload_rate.clone()).ok_or("Disconnected")?;
        let active = || server.state.lock().sessions.contains_key(&me);
        let paced = tokio::join!(server.upload_rate.pace_while(data.len(), active), rate.pace_while(data.len(), active));
        if paced != (true, true) {
            return Err("Disconnected".into());
        }
        server.file_perm(me, self._space.channel, perm::FILE_UPLOAD)?;
        self.file.as_mut().ok_or("Upload closed")?.write_all(data).await.map_err(|e| e.to_string())?;
        self.remaining -= data.len() as u64;
        Ok(())
    }
    pub(crate) async fn finish(mut self, server: &Server, me: ClientId) -> Result<(), String> {
        if self.remaining != 0 {
            return Err("Upload incomplete".into());
        }
        let mut file = self.file.take().ok_or("Upload closed")?;
        file.flush().await.map_err(|e| e.to_string())?;
        drop(file);
        let ch = self._space.channel;
        server.file_perm(me, ch, perm::FILE_UPLOAD)?;
        let dst = crate::files::resolve(&server.data_dir, ch, &self.relative)?;
        if dst != self.target || crate::files::is_link(&self.temp) {
            return Err("Invalid upload path".into());
        }
        if self.overwrite && server.file_perm(me, ch, perm::FILE_DELETE).is_ok() {
            // Replace atomically; a failed rename leaves the existing file intact.
            tokio::fs::rename(&self.temp, &dst).await.map_err(|e| e.to_string())
        } else {
            // A file could have appeared since UploadBegin. Never replace it
            // without delete permission, even with two uploads racing to finish.
            publish_new(&self.temp, &dst).map_err(|e| e.to_string())
        }
    }
}

#[cfg(windows)]
fn publish_new(temp: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    let wide = |p: &std::path::Path| p.as_os_str().encode_wide().chain(std::iter::once(0)).collect::<Vec<_>>();
    let (from, to) = (wide(temp), wide(dst));
    // No REPLACE_EXISTING flag: creation must fail atomically if dst exists.
    if unsafe { windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}
#[cfg(not(windows))]
fn publish_new(temp: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::hard_link(temp, dst)
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
    let Some((id, lease)) = server.attach_bulk(&token, &uid, btx.clone(), frames.clone()) else {
        return Ok(());
    };
    writer.send(&BulkS2C::Attached).await?;
    TRAFFIC.sent(Cat::Chat, writer.last_size);
    tokio::spawn(async move {
        while let Some(m) = brx.recv().await {
            if matches!(m, BulkS2C::Frame { .. } | BulkS2C::CameraFrame { .. }) {
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
        if !server.state.lock().sessions.get(&id).and_then(|s| s.bulk_tx.as_ref()).is_some_and(|tx| tx.same_channel(&btx)) {
            break;
        }
        match msg {
            BulkC2S::DirectFrame { to, stream, jpeg } => {
                let _ = server.direct_frame(id, to, stream, jpeg);
            }
            BulkC2S::VideoMessage { id: tid, uid, data } => {
                let Ok(slot) = image_jobs.clone().acquire_owned().await else { break };
                let (srv, tx, lease) = (server.clone(), btx.clone(), lease.clone());
                tokio::spawn(async move {
                    let _slot = slot;
                    let result = tokio::task::spawn_blocking(move || {
                        let _lease = lease;
                        srv.video_message(id, &uid, &data)
                    })
                    .await;
                    let reply = match result {
                        Ok(Ok(())) => BulkS2C::ChatImageSent { id: tid },
                        Ok(Err(message)) => BulkS2C::TransferError { id: tid, message },
                        Err(_) => BulkS2C::TransferError { id: tid, message: "Video message failed".into() },
                    };
                    let _ = tx.send(reply).await;
                });
            }
            BulkC2S::Frame(f) => server.relay_frame(id, f),
            BulkC2S::RequestKeyframe(streamer) => server.request_keyframe(streamer),
            BulkC2S::StreamAudio(data) => server.relay_audio(id, data),
            BulkC2S::CameraFrame(f) => server.relay_camera(id, f),
            BulkC2S::RequestCameraKeyframe(of) => server.request_camera_keyframe(id, of),
            BulkC2S::UploadBegin { id: tid, channel, path, size, overwrite } => {
                let res = if uploads.len() >= MAX_UPLOADS {
                    Err(format!("At most {MAX_UPLOADS} uploads can run at the same time"))
                } else if uploads.contains_key(&tid) {
                    // Replacing a running upload would leave its partial file behind,
                    // hidden from listings and outside the quota.
                    Err("That upload is already running".to_string())
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
                    failed = u.chunk(&server, id, &data).await.err();
                }
                if let Some(msg) = failed {
                    uploads.remove(&tid);
                    let _ = btx.send(BulkS2C::TransferError { id: tid, message: msg }).await;
                }
            }
            BulkC2S::UploadEnd { id: tid } => {
                if let Some(u) = uploads.remove(&tid) {
                    let res = u.finish(&server, id).await;
                    match res {
                        Ok(()) => {
                            let _ = btx.send(BulkS2C::UploadDone { id: tid }).await;
                        }
                        Err(e) => {
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
                if downloads.contains_key(&tid) || downloads.len() >= 64 {
                    let _ = tx.send(BulkS2C::TransferError { id: tid, message: "Too many or duplicate downloads".into() }).await;
                    continue;
                }
                let slot = match server.transfer_slot(id, false) {
                    Ok(s) => s,
                    Err(message) => {
                        let _ = tx.send(BulkS2C::TransferError { id: tid, message }).await;
                        continue;
                    }
                };
                let local = server.state.lock().sessions.get(&id).map(|s| s.download_rate.clone());
                let Some(local) = local else { break };
                let srv = server.clone();
                let lease = lease.clone();
                let h = tokio::spawn(async move {
                    let _slot = slot;
                    let _lease = lease;
                    if let Err(e) = send_file(&tx, tid, p, &srv, id, channel, &local).await {
                        let _ = tx.send(BulkS2C::TransferError { id: tid, message: e }).await;
                    }
                });
                downloads.insert(tid, h.abort_handle());
            }
            BulkC2S::Cancel { id: tid } => {
                uploads.remove(&tid);
                if let Some(h) = downloads.remove(&tid) {
                    h.abort();
                }
            }
            BulkC2S::ChatImage { id: tid, target, name, width, height, data } => {
                let job_lease = lease.clone();
                // Waiting for a slot here also stops reading more data from this client.
                let Ok(slot) = image_jobs.clone().acquire_owned().await else { break };
                let (srv, tx) = (server.clone(), btx.clone());
                tokio::spawn(async move {
                    let _slot = slot;
                    let r = tokio::task::spawn_blocking(move || {let _lease=job_lease;srv.post_image(id, target, &name, width, height, data)})
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
            m @ (BulkC2S::AddServerGif { .. } | BulkC2S::AddServerGifTagged { .. }) => {
                let job_lease = lease.clone();
                let (tid, category, name, tags, data) = match m {
                    BulkC2S::AddServerGifTagged { id, category, name, tags, data } => (id, category, name, tags, data),
                    BulkC2S::AddServerGif { id, category, name, data } => (id, category, name, Vec::new(), data),
                    _ => unreachable!(),
                };
                let Ok(slot) = image_jobs.clone().acquire_owned().await else { break };
                let (srv, tx) = (server.clone(), btx.clone());
                tokio::spawn(async move {
                    let _slot = slot;
                    let r = tokio::task::spawn_blocking(move || {
                        let _lease = job_lease;
                        srv.add_server_gif(id, &category, &name, &tags, &data)
                    })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                    let _ = tx.send(BulkS2C::ServerGifAdded { id: tid, error: r.err() }).await;
                });
            }
            BulkC2S::GifPreview(gif) => {
                let job_lease = lease.clone();
                let Ok(slot) = image_jobs.clone().acquire_owned().await else { break };
                let (srv, tx) = (server.clone(), btx.clone());
                tokio::spawn(async move {
                    let _slot = slot;
                    let g = gif.chars().take(40).collect::<String>();
                    let data = tokio::task::spawn_blocking({
                        let g = g.clone();
                        move || {
                            let _lease = job_lease;
                            srv.gif_preview(id, &g)
                        }
                    })
                    .await
                    .unwrap_or_default();
                    let _ = tx.send(BulkS2C::GifPreview { id: g, data }).await;
                });
            }
            BulkC2S::GetImage(image) => {
                let job_lease = lease.clone();
                let Ok(slot) = image_jobs.clone().acquire_owned().await else { break };
                let (srv, tx) = (server.clone(), btx.clone());
                tokio::spawn(async move {
                    // Held until the image is queued for sending, so only a few decrypted
                    // images can wait in memory per connection.
                    let _slot = slot;
                    let r = tokio::task::spawn_blocking(move || {
                        let _lease = job_lease;
                        srv.chat_image(id, image)
                    })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
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
    drop(uploads);
    for (_, h) in downloads {
        h.abort();
    }
    server.detach_bulk(id, &btx);
    Ok(())
}

pub(crate) async fn begin_upload(
    server: &Arc<Server>,
    me: ClientId,
    channel: ChannelId,
    path: &str,
    size: u64,
    overwrite: bool,
) -> Result<Upload, String> {
    server.file_perm(me, channel, perm::FILE_UPLOAD)?;
    let slot = server.transfer_slot(me, true)?;
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
        let entry = r.entry(channel).or_insert(0);
        *entry = entry.checked_add(size).ok_or("Too much space reserved")?;
        Reservation { server: server.clone(), channel, bytes: size }
    };
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    }
    // A temp file of its own per upload, so two uploads of the same file can't write
    // into each other. `create_new` never reuses or follows an existing file.
    let tag: [u8; 6] = crate::secrets::random_bytes();
    let mut temp = target.clone().into_os_string();
    temp.push(format!(".{}.voxa-part", tag.iter().map(|b| format!("{b:02x}")).collect::<String>()));
    let temp = PathBuf::from(temp);
    let file = tokio::fs::OpenOptions::new().write(true).create_new(true).open(&temp).await.map_err(|e| format!("Cannot create file: {e}"))?;
    Ok(Upload { file: Some(file), temp, target, remaining: size, _space: space, _slot: slot, relative: path.into(), overwrite })
}

async fn send_file(
    tx: &mpsc::Sender<BulkS2C>,
    id: u32,
    path: PathBuf,
    server: &Server,
    me: ClientId,
    channel: ChannelId,
    client: &voxa_proto::transfers::Rate,
) -> Result<(), String> {
    use tokio::io::AsyncReadExt;
    let global = &server.download_rate;
    let mut f = tokio::fs::File::open(&path).await.map_err(|_| "File not found".to_string())?;
    let size = f.metadata().await.map_err(|e| e.to_string())?.len();
    tx.send(BulkS2C::DownloadBegin { id, size }).await.map_err(|_| "closed")?;
    let mut buf = vec![0u8; FILE_CHUNK];
    loop {
        let chunk = global.chunk().min(client.chunk());
        let n = f.read(&mut buf[..chunk]).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        tokio::join!(global.pace(n), client.pace(n));
        server.file_perm(me, channel, perm::FILE_DOWNLOAD)?;
        tx.send(BulkS2C::DownloadChunk { id, data: buf[..n].to_vec() }).await.map_err(|_| "closed")?;
    }
    tx.send(BulkS2C::DownloadEnd { id }).await.map_err(|_| "closed")?;
    Ok(())
}
