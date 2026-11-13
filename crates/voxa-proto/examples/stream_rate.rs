//! Measures the frame rate of a screen share as a viewer receives it:
//! `cargo run -p voxa-proto --example stream_rate -- 127.0.0.1:9988 <seconds>`
//! Waits for someone on the server to start sharing, watches it and counts frames.

use std::time::{Duration, Instant};

use tokio::net::TcpStream;
use tokio::time::timeout;
use voxa_proto::identity::KeyPair;
use voxa_proto::transport;
use voxa_proto::*;

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());
    let secs: u64 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(8);
    let kp = KeyPair::generate();
    let s = TcpStream::connect(&addr).await.unwrap();
    let (r, w) = s.into_split();
    let sec = transport::connect(r, w, &kp.private).await.unwrap();
    let (mut r, mut w) = (sec.reader, sec.writer);
    w.send(&C2S::Hello {
        protocol: PROTOCOL_VERSION,
        version: APP_VERSION.into(),
        platform: "test".into(),
        nickname: "FrameCounter".into(),
        server_password: None,
        default_channel: None,
        default_channel_password: None,
    })
    .await
    .unwrap();
    let S2C::Welcome { bulk_token, clients, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    // Bulk connection for the frames.
    let s = TcpStream::connect(&addr).await.unwrap();
    let (br, bw) = s.into_split();
    let bsec = transport::connect(br, bw, &kp.private).await.unwrap();
    let (mut br, mut bw) = (bsec.reader, bsec.writer);
    bw.send(&C2S::AttachBulk(bulk_token)).await.unwrap();
    assert!(matches!(br.recv::<BulkS2C>().await.unwrap(), BulkS2C::Attached));
    // Find (or wait for) a streamer.
    let mut streamer = clients.iter().find(|c| c.streaming.is_some()).map(|c| (c.id, c.streaming.clone().unwrap()));
    let deadline = Instant::now() + Duration::from_secs(90);
    while streamer.is_none() && Instant::now() < deadline {
        if let Ok(Ok(S2C::ClientUpdated(c) | S2C::ClientJoined(c))) = timeout(Duration::from_secs(1), r.recv::<S2C>()).await {
            if let Some(info) = c.streaming.clone() {
                streamer = Some((c.id, info));
            }
        }
    }
    let (id, info) = streamer.expect("nobody started sharing");
    println!("watching client {id}: {}x{} at {} fps requested", info.width, info.height, info.fps);
    w.send(&C2S::WatchStream { streamer: id, watch: true }).await.unwrap();
    // Keep the control connection alive while counting.
    tokio::spawn(async move {
        loop {
            if r.recv::<S2C>().await.is_err() {
                break;
            }
        }
    });
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            if w.send(&C2S::Ping(0)).await.is_err() {
                break;
            }
        }
    });
    // Skip the first second (start-up), then count.
    let warmup = Instant::now() + Duration::from_secs(1);
    let mut start = None;
    let (mut frames, mut keys, mut bytes) = (0u64, 0u64, 0usize);
    let mut per_second = Vec::new();
    let mut this_second = (Instant::now(), 0u32);
    let end_after = Duration::from_secs(secs);
    loop {
        let Ok(Ok(m)) = timeout(Duration::from_secs(3), br.recv::<BulkS2C>()).await else { break };
        let BulkS2C::Frame { from, frame } = m else { continue };
        if from != id || Instant::now() < warmup {
            continue;
        }
        let t0 = *start.get_or_insert_with(Instant::now);
        frames += 1;
        keys += frame.keyframe as u64;
        bytes += frame.tiles.iter().map(|t| t.jpeg.len()).sum::<usize>();
        this_second.1 += 1;
        if this_second.0.elapsed() >= Duration::from_secs(1) {
            per_second.push(this_second.1);
            this_second = (Instant::now(), 0);
        }
        if t0.elapsed() >= end_after {
            break;
        }
    }
    let el = start.map(|s| s.elapsed().as_secs_f64()).unwrap_or(1.0);
    println!(
        "received {frames} frames in {el:.1} s = {:.1} fps ({keys} keyframes, {:.1} Mbit/s); per second: {per_second:?}",
        frames as f64 / el,
        bytes as f64 * 8.0 / el / 1e6
    );
}
