//! File transfer stress test against a running server:
//! `cargo run -p voxa-proto --example transfer_test -- 127.0.0.1:9988 <data-dir> 256 [--legacy]`
//!
//! Becomes admin with the privilege key from the data dir, uploads a throttled file (so
//! it spans the server's periodic housekeeping ticks), downloads it again and verifies
//! every byte. Unless `--legacy` is given it also tests the 1.3.0 file size limits.

use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::time::timeout;
use voxa_proto::identity::KeyPair;
use voxa_proto::transport::{self, SecureReader, SecureWriter};
use voxa_proto::*;

type R = tokio::net::tcp::OwnedReadHalf;
type W = tokio::net::tcp::OwnedWriteHalf;

async fn open(addr: &str, kp: &KeyPair) -> (SecureReader<R>, SecureWriter<W>) {
    let s = TcpStream::connect(addr).await.unwrap();
    s.set_nodelay(true).unwrap();
    let (r, w) = s.into_split();
    let sec = transport::connect(r, w, &kp.private).await.unwrap();
    (sec.reader, sec.writer)
}

fn byte_at(i: u64) -> u8 {
    (i % 251) as u8 ^ ((i >> 16) as u8)
}

async fn until<T>(r: &mut SecureReader<R>, secs: u64, mut f: impl FnMut(S2C) -> Option<T>) -> Option<T> {
    timeout(Duration::from_secs(secs), async {
        loop {
            if let Some(t) = f(r.recv::<S2C>().await.unwrap()) {
                return t;
            }
        }
    })
    .await
    .ok()
}

/// Local TCP proxy that limits client->server bandwidth (~4 MB/s in small writes), so
/// messages arrive slowly like on a real network link.
async fn slow_proxy(upstream: String) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        loop {
            let Ok((client, _)) = listener.accept().await else { break };
            let upstream = upstream.clone();
            tokio::spawn(async move {
                let Ok(server) = TcpStream::connect(&upstream).await else { return };
                let (mut cr, mut cw) = client.into_split();
                let (mut sr, mut sw) = server.into_split();
                tokio::spawn(async move {
                    let _ = tokio::io::copy(&mut sr, &mut cw).await;
                });
                let mut buf = vec![0u8; 8192];
                loop {
                    let n = match cr.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    if sw.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                    tokio::time::sleep(Duration::from_millis(2)).await;
                }
            });
        }
    });
    addr
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut addr = args.get(1).cloned().unwrap_or("127.0.0.1:9988".into());
    if args.iter().any(|a| a == "--slow") {
        addr = slow_proxy(addr).await;
        println!("using bandwidth-limited proxy {addr}");
    }
    let data_dir = args.get(2).cloned().unwrap_or_default();
    let size_mb: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(256);
    let legacy = args.iter().any(|a| a == "--legacy");
    let size = size_mb * 1024 * 1024;

    let kp = KeyPair::generate();
    let (mut r, mut w) = open(&addr, &kp).await;
    w.send(&C2S::Hello {
        protocol: PROTOCOL_VERSION,
        version: "transfer-test".into(),
        platform: "test".into(),
        nickname: "Tester".into(),
        server_password: None,
        default_channel: None,
        default_channel_password: None,
    })
    .await
    .unwrap();
    let S2C::Welcome { channels, bulk_token, server, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    println!("server version {}", server.version);
    let ch = channels.iter().find(|c| c.is_default).unwrap().id;

    // Become Server Admin (uploads need the "Upload files" permission).
    let key = std::fs::read_to_string(std::path::Path::new(&data_dir).join("privilege_key.txt"))
        .ok()
        .and_then(|t| t.lines().map(str::trim).find(|l| l.len() >= 20 && !l.contains(' ')).map(String::from))
        .expect("privilege_key.txt");
    w.send(&C2S::UsePrivilegeKey(key)).await.unwrap();
    until(&mut r, 5, |m| matches!(m, S2C::Notice(_)).then_some(())).await.expect("privilege key");

    let (mut br, mut bw) = open(&addr, &kp).await;
    bw.send(&C2S::AttachBulk(bulk_token)).await.unwrap();
    assert!(matches!(br.recv::<BulkS2C>().await.unwrap(), BulkS2C::Attached));

    if !legacy {
        w.send(&C2S::GetFileLimits).await.unwrap();
        let l = until(&mut r, 5, |m| if let S2C::FileLimits(l) = m { Some(l) } else { None }).await.expect("limits");
        println!("current limits: {l:?}");
        w.send(&C2S::SetFileLimits(FileLimits { max_upload_mb: 1, channel_quota_mb: 0 })).await.unwrap();
        until(&mut r, 5, |m| matches!(m, S2C::FileLimits(l) if l.max_upload_mb == 1).then_some(())).await.expect("set limit");
        bw.send(&BulkC2S::UploadBegin { id: 9, channel: ch, path: "too-big.bin".into(), size: 2 * 1024 * 1024, overwrite: true }).await.unwrap();
        match timeout(Duration::from_secs(5), br.recv::<BulkS2C>()).await.unwrap().unwrap() {
            BulkS2C::TransferError { message, .. } if message.contains("too large") => println!("limit enforced: \"{message}\""),
            other => panic!("expected size rejection, got {other:?}"),
        }
        w.send(&C2S::SetFileLimits(FileLimits { max_upload_mb: 0, channel_quota_mb: 0 })).await.unwrap();
        until(&mut r, 5, |m| matches!(m, S2C::FileLimits(l) if l.max_upload_mb == 0).then_some(())).await.expect("unlimited");
        println!("limits set to unlimited (0)");
    }

    // Keep the control connection alive and drained from here on.
    tokio::spawn(async move {
        let mut t = tokio::time::interval(Duration::from_secs(5));
        loop {
            tokio::select! {
                m = r.recv::<S2C>() => { if m.is_err() { break; } }
                _ = t.tick() => { if w.send(&C2S::Ping(0)).await.is_err() { break; } }
            }
        }
    });

    // Throttled upload: ~16 s in total so it spans several server ticks.
    let chunks = size.div_ceil(FILE_CHUNK as u64);
    let pause = Duration::from_micros((16_000_000 / chunks.max(1)).min(20_000));
    bw.send(&BulkC2S::UploadBegin { id: 1, channel: ch, path: "stress.bin".into(), size, overwrite: true }).await.unwrap();
    match timeout(Duration::from_secs(10), br.recv::<BulkS2C>()).await.unwrap().unwrap() {
        BulkS2C::UploadReady { id: 1 } => {}
        other => panic!("upload refused: {other:?}"),
    }
    let start = Instant::now();
    let mut off = 0u64;
    while off < size {
        let n = (size - off).min(FILE_CHUNK as u64);
        let data: Vec<u8> = (off..off + n).map(byte_at).collect();
        if let Err(e) = bw.send(&BulkC2S::UploadChunk { id: 1, data }).await {
            println!("UPLOAD FAILED after {} MB: connection broke ({e})", off / 1024 / 1024);
            std::process::exit(2);
        }
        off += n;
        tokio::time::sleep(pause).await;
    }
    let _ = bw.send(&BulkC2S::UploadEnd { id: 1 }).await;
    match timeout(Duration::from_secs(60), br.recv::<BulkS2C>()).await {
        Ok(Ok(BulkS2C::UploadDone { id: 1 })) => println!("upload of {size_mb} MB finished in {:.1}s", start.elapsed().as_secs_f32()),
        Ok(Ok(other)) => {
            println!("UPLOAD FAILED: {other:?}");
            std::process::exit(2);
        }
        Ok(Err(e)) => {
            println!("UPLOAD FAILED: bulk connection broke ({e})");
            std::process::exit(2);
        }
        Err(_) => {
            println!("UPLOAD HUNG: no reply 60 s after the last chunk");
            std::process::exit(2);
        }
    }

    // Download and verify.
    bw.send(&BulkC2S::Download { id: 2, channel: ch, path: "stress.bin".into() }).await.unwrap();
    let mut got = 0u64;
    let mut ok = true;
    loop {
        match timeout(Duration::from_secs(30), br.recv::<BulkS2C>()).await {
            Ok(Ok(BulkS2C::DownloadBegin { size: s, .. })) => assert_eq!(s, size),
            Ok(Ok(BulkS2C::DownloadChunk { data, .. })) => {
                for (i, b) in data.iter().enumerate() {
                    if *b != byte_at(got + i as u64) {
                        ok = false;
                    }
                }
                got += data.len() as u64;
            }
            Ok(Ok(BulkS2C::DownloadEnd { .. })) => break,
            Ok(Ok(_)) => {}
            Ok(Err(e)) => panic!("download broke: {e}"),
            Err(_) => panic!("download hung at {got} bytes"),
        }
    }
    assert_eq!(got, size, "download size");
    assert!(ok, "downloaded data does not match");
    println!("download verified: {size_mb} MB byte-for-byte identical");
    println!("TRANSFER TEST PASSED");
}
