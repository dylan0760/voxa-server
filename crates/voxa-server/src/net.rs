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
use voxa_proto::identity::uid_from_public;
use voxa_proto::transport::{self, SecureReader, SecureWriter};
use voxa_proto::*;

use crate::state::{Out, Server};

pub async fn run(server: Arc<Server>, listener: TcpListener) {
    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                let server = server.clone();
                tokio::spawn(async move {
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
    let first: C2S = timeout(Duration::from_secs(15), reader.recv()).await??;
    match first {
        C2S::AttachBulk(token) => bulk(server, reader, writer, token).await,
        hello @ C2S::Hello { .. } => control(server, reader, writer, uid, addr, hello).await,
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
    let (tx, mut rx) = mpsc::unbounded_channel::<Out>();
    let close = Arc::new(Notify::new());
    let id = match server.login(uid, addr.ip(), hello, tx, close.clone()) {
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
                }
                Out::Close => break,
            }
        }
    });
    let reason = loop {
        tokio::select! {
            r = timeout(Duration::from_secs(45), reader.recv::<C2S>()) => match r {
                Ok(Ok(C2S::Disconnect(msg))) => break LeaveReason::Disconnected(state_trunc(&msg)),
                Ok(Ok(msg)) => server.handle(id, msg),
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
    crate::state::truncate(s, 120)
}

struct Upload {
    file: tokio::fs::File,
    temp: PathBuf,
    target: PathBuf,
    remaining: u64,
}

async fn bulk(
    server: Arc<Server>,
    mut reader: SecureReader<OwnedReadHalf>,
    mut writer: SecureWriter<OwnedWriteHalf>,
    token: [u8; 32],
) -> anyhow::Result<()> {
    let (btx, mut brx) = mpsc::channel::<BulkS2C>(48);
    let Some(id) = server.attach_bulk(&token, btx.clone()) else {
        return Ok(());
    };
    writer.send(&BulkS2C::Attached).await?;
    tokio::spawn(async move {
        while let Some(m) = brx.recv().await {
            if writer.send(&m).await.is_err() {
                break;
            }
        }
    });
    let mut uploads: HashMap<u32, Upload> = HashMap::new();
    let mut downloads: HashMap<u32, tokio::task::AbortHandle> = HashMap::new();
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        let msg = tokio::select! {
            m = reader.recv::<BulkC2S>() => match m { Ok(m) => m, Err(_) => break },
            _ = tick.tick() => {
                if !server.state.lock().sessions.contains_key(&id) { break; }
                continue;
            }
        };
        match msg {
            BulkC2S::Frame(f) => server.relay_frame(id, f),
            BulkC2S::RequestKeyframe(streamer) => server.request_keyframe(streamer),
            BulkC2S::UploadBegin { id: tid, channel, path, size, overwrite } => {
                match begin_upload(&server, id, channel, &path, size, overwrite).await {
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
                let (srv, tx) = (server.clone(), btx.clone());
                tokio::spawn(async move {
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
                let (srv, tx) = (server.clone(), btx.clone());
                tokio::spawn(async move {
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
    server.detach_bulk(id);
    Ok(())
}

async fn begin_upload(
    server: &Server,
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
    if limits.channel_quota_mb > 0 {
        let root = crate::files::channel_root(&server.data_dir, channel);
        let used = tokio::task::spawn_blocking(move || crate::files::dir_size(&root)).await.unwrap_or(0);
        if used.saturating_add(size) > limits.channel_quota_mb.saturating_mul(MB) {
            return Err(format!("Channel file quota exceeded ({} MB per channel)", limits.channel_quota_mb));
        }
    }
    let target = crate::files::resolve(&server.data_dir, channel, path)?;
    if target == crate::files::channel_root(&server.data_dir, channel) {
        return Err("Invalid file name".into());
    }
    if target.exists() && !overwrite {
        return Err("A file with this name already exists".into());
    }
    if let Some(parent) = target.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(|e| e.to_string())?;
    }
    let mut temp = target.clone().into_os_string();
    temp.push(".voxa-part");
    let temp = PathBuf::from(temp);
    let file = tokio::fs::File::create(&temp).await.map_err(|e| format!("Cannot create file: {e}"))?;
    Ok(Upload { file, temp, target, remaining: size })
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
