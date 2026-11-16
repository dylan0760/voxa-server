//! Video in channels (servers 1.21.0+): cameras on and off, pictures relayed to the
//! channel only, full pictures on request, and the camera going off with the channel.
//! `cargo run -p voxa-proto --example video_test -- 127.0.0.1:9988`
//! Must be built from the client repository (it reports the client version).

use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::timeout;
use voxa_proto::identity::KeyPair;
use voxa_proto::transport::{self, SecureReader, SecureWriter};
use voxa_proto::*;

type R = tokio::net::tcp::OwnedReadHalf;
type W = tokio::net::tcp::OwnedWriteHalf;

struct User {
    r: SecureReader<R>,
    w: SecureWriter<W>,
    br: SecureReader<R>,
    bw: SecureWriter<W>,
    id: ClientId,
    first: Vec<S2C>,
}

async fn open(addr: &str, kp: &KeyPair) -> (SecureReader<R>, SecureWriter<W>) {
    let s = TcpStream::connect(addr).await.unwrap();
    s.set_nodelay(true).unwrap();
    let (r, w) = s.into_split();
    let sec = transport::connect(r, w, &kp.private).await.unwrap();
    (sec.reader, sec.writer)
}

async fn login(addr: &str, nick: &str) -> User {
    let kp = KeyPair::generate();
    let (mut r, mut w) = open(addr, &kp).await;
    w.send(&C2S::Hello {
        protocol: PROTOCOL_VERSION,
        version: APP_VERSION.into(),
        platform: "test".into(),
        nickname: nick.into(),
        server_password: None,
        default_channel: None,
        default_channel_password: None,
    })
    .await
    .unwrap();
    let S2C::Welcome { your_id, bulk_token, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    w.send(&C2S::Hardware(vec![hardware_token("x", nick)])).await.unwrap();
    let mut first = Vec::new();
    while let Ok(Ok(m)) = timeout(Duration::from_millis(400), r.recv::<S2C>()).await {
        first.push(m);
    }
    let (mut br, mut bw) = open(addr, &kp).await;
    bw.send(&C2S::AttachBulk(bulk_token)).await.unwrap();
    assert!(matches!(br.recv::<BulkS2C>().await.unwrap(), BulkS2C::Attached));
    User { r, w, br, bw, id: your_id, first }
}

async fn wait<T>(u: &mut User, secs: u64, mut f: impl FnMut(S2C) -> Option<T>) -> Option<T> {
    timeout(Duration::from_secs(secs), async {
        loop {
            if let Some(t) = f(u.r.recv::<S2C>().await.unwrap()) {
                return t;
            }
        }
    })
    .await
    .ok()
}

async fn bulk_wait<T>(u: &mut User, secs: u64, mut f: impl FnMut(BulkS2C) -> Option<T>) -> Option<T> {
    timeout(Duration::from_secs(secs), async {
        loop {
            if let Some(t) = f(u.br.recv::<BulkS2C>().await.unwrap()) {
                return t;
            }
        }
    })
    .await
    .ok()
}

async fn error(u: &mut User) -> String {
    wait(u, 5, |m| if let S2C::Error(e) = m { Some(e) } else { None }).await.expect("no error")
}

fn frame(n: u8) -> StreamFrame {
    StreamFrame { width: 640, height: 360, keyframe: true, tiles: vec![Tile { x: 0, y: 0, w: 128, h: 128, jpeg: vec![n; 64] }] }
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());
    let mut alice = login(&addr, "Alice").await;
    let mut bob = login(&addr, "Bob").await;
    assert!(bob.first.iter().any(|m| matches!(m, S2C::Cameras(c) if c.is_empty())), "no camera list after Welcome");
    println!("1. the camera list comes after Welcome");

    // 2. Pictures too large are refused.
    alice.w.send(&C2S::StartCamera(CameraInfo { width: 1920, height: 1080, fps: 30 })).await.unwrap();
    println!("2. a 1080p camera: \"{}\"", error(&mut alice).await);

    // 3. On: everyone hears about it, pictures reach the channel.
    let cam = CameraInfo { width: 640, height: 360, fps: 15 };
    alice.w.send(&C2S::StartCamera(cam)).await.unwrap();
    let aid = alice.id;
    assert!(wait(&mut bob, 5, |m| matches!(m, S2C::CameraChanged { client, camera: Some(c) } if client == aid && c == cam).then_some(())).await.is_some());
    alice.bw.send(&BulkC2S::CameraFrame(frame(7))).await.unwrap();
    let got = bulk_wait(&mut bob, 5, |m| match m {
        BulkS2C::CameraFrame { from, frame } if from == aid => Some(frame.tiles[0].jpeg[0]),
        _ => None,
    })
    .await;
    assert_eq!(got, Some(7), "Bob did not get Alice's picture");
    println!("3. Alice's camera is on and Bob sees her pictures");

    // 4. A full picture on request.
    bob.bw.send(&BulkC2S::RequestCameraKeyframe(aid)).await.unwrap();
    assert!(bulk_wait(&mut alice, 5, |m| matches!(m, BulkS2C::CameraKeyframeRequested).then_some(())).await.is_some());
    println!("4. Bob asked for a full picture and Alice was told");

    // 5. Without a camera on, pictures go nowhere.
    bob.bw.send(&BulkC2S::CameraFrame(frame(9))).await.unwrap();
    let leaked = bulk_wait(&mut alice, 2, |m| matches!(m, BulkS2C::CameraFrame { .. }).then_some(())).await;
    assert!(leaked.is_none(), "a picture without a camera was relayed");
    println!("5. pictures from someone whose camera is off are dropped");

    // 6. Oversized pictures are dropped.
    let mut big = frame(3);
    big.width = 4000;
    alice.bw.send(&BulkC2S::CameraFrame(big)).await.unwrap();
    let leaked = bulk_wait(&mut bob, 2, |m| matches!(m, BulkS2C::CameraFrame { .. }).then_some(())).await;
    assert!(leaked.is_none(), "a picture larger than the camera was relayed");
    println!("6. a picture larger than the camera is dropped");

    // 7. Leaving the channel turns the camera off; then pictures stop.
    alice.w.send(&C2S::SwitchChannel { channel: NO_CHANNEL, password: None }).await.unwrap();
    assert!(wait(&mut bob, 5, |m| matches!(m, S2C::CameraChanged { client, camera: None } if client == aid).then_some(())).await.is_some());
    alice.bw.send(&BulkC2S::CameraFrame(frame(5))).await.unwrap();
    let leaked = bulk_wait(&mut bob, 2, |m| matches!(m, BulkS2C::CameraFrame { .. }).then_some(())).await;
    assert!(leaked.is_none(), "pictures were relayed after leaving the channel");
    println!("7. leaving the channel turned Alice's camera off");

    // 8. No camera outside a channel.
    alice.w.send(&C2S::StartCamera(cam)).await.unwrap();
    println!("8. a camera outside a channel: \"{}\"", error(&mut alice).await);

    // 9. Stopping explicitly.
    bob.w.send(&C2S::StartCamera(cam)).await.unwrap();
    let bid = bob.id;
    assert!(wait(&mut alice, 5, |m| matches!(m, S2C::CameraChanged { client, camera: Some(_) } if client == bid).then_some(())).await.is_some());
    bob.w.send(&C2S::StopCamera).await.unwrap();
    assert!(wait(&mut alice, 5, |m| matches!(m, S2C::CameraChanged { client, camera: None } if client == bid).then_some(())).await.is_some());
    println!("9. Bob turned his camera on and off");
    println!("VIDEO TEST PASSED");
}
