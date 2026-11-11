//! End-to-end smoke test against a running server: `cargo run --example smoke -- 127.0.0.1:9987`

use std::time::Duration;

use tokio::net::{TcpStream, UdpSocket};
use voxa_proto::identity::KeyPair;
use voxa_proto::transport::{self, SecureReader, SecureWriter};
use voxa_proto::voice::{self, Direction, Kind, VoiceCipher};
use voxa_proto::*;

type R = tokio::net::tcp::OwnedReadHalf;
type W = tokio::net::tcp::OwnedWriteHalf;

async fn login(addr: &str, kp: &KeyPair, nick: &str) -> (SecureReader<R>, SecureWriter<W>, S2C) {
    let s = TcpStream::connect(addr).await.unwrap();
    let (r, w) = s.into_split();
    let sec = transport::connect(r, w, &kp.private).await.unwrap();
    let (mut r, mut w) = (sec.reader, sec.writer);
    w.send(&C2S::Hello {
        protocol: PROTOCOL_VERSION,
        version: "smoke".into(),
        platform: "test".into(),
        nickname: nick.into(),
        server_password: None,
        default_channel: None,
        default_channel_password: None,
    })
    .await
    .unwrap();
    let welcome: S2C = r.recv().await.unwrap();
    (r, w, welcome)
}

async fn until<T>(r: &mut SecureReader<R>, mut f: impl FnMut(S2C) -> Option<T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let m: S2C = r.recv().await.unwrap();
            if let Some(t) = f(m) {
                return t;
            }
        }
    })
    .await
    .expect("timed out waiting for message")
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9987".into());
    let ka = KeyPair::generate();
    let kb = KeyPair::generate();
    let (mut ra, mut wa, wel_a) = login(&addr, &ka, "Alice").await;
    let S2C::Welcome { your_id: ida, voice: va, channels, bulk_token: ta, .. } = wel_a else { panic!("A refused: {wel_a:?}") };
    let ch = channels.iter().find(|c| c.is_default).unwrap().id;
    println!("A logged in as {ida}, default channel {ch}");
    let (mut rb, mut wb, wel_b) = login(&addr, &kb, "Bob").await;
    let S2C::Welcome { your_id: idb, voice: vb, bulk_token: tb, .. } = wel_b else { panic!("B refused") };
    println!("B logged in as {idb}");
    until(&mut ra, |m| matches!(m, S2C::ClientJoined(ref c) if c.id == idb).then_some(())).await;

    // Channel chat, live + persisted.
    let text = format!("hello from alice {}", now_unix());
    wa.send(&C2S::SendText { target: TextTarget::Channel(ch), text: text.clone() }).await.unwrap();
    let got = until(&mut rb, |m| if let S2C::Text(t) = m { Some(t) } else { None }).await;
    assert_eq!(got.text, text);
    println!("B received channel message id {}", got.id);
    wb.send(&C2S::FetchHistory { target: TextTarget::Channel(ch), before: None, limit: 10 }).await.unwrap();
    let hist = until(&mut rb, |m| if let S2C::History { messages, .. } = m { Some(messages) } else { None }).await;
    assert!(hist.iter().any(|m| m.text == text));
    println!("history contains the message ({} messages)", hist.len());

    // Voice relay over UDP.
    let host = addr.rsplit_once(':').unwrap().0.to_string();
    let ua = UdpSocket::bind("0.0.0.0:0").await.unwrap();
    ua.connect(format!("{host}:{}", va.udp_port)).await.unwrap();
    let ub = UdpSocket::bind("0.0.0.0:0").await.unwrap();
    ub.connect(format!("{host}:{}", vb.udp_port)).await.unwrap();
    let ca = VoiceCipher::new(&va.key);
    let cb = VoiceCipher::new(&vb.key);
    ub.send(&cb.seal(Direction::ToServer, Kind::Ping, idb, 1, &1u64.to_be_bytes())).await.unwrap();
    let mut buf = [0u8; 2048];
    let n = tokio::time::timeout(Duration::from_secs(3), ub.recv(&mut buf)).await.expect("no pong").unwrap();
    assert_eq!(cb.open(Direction::ToClient, &buf[..n]).unwrap().kind, Kind::Pong);
    println!("UDP ping/pong ok");
    ua.send(&ca.seal(Direction::ToServer, Kind::Voice, ida, 1, &voice::encode_voice(1, 0, b"opusdata"))).await.unwrap();
    let n = tokio::time::timeout(Duration::from_secs(3), ub.recv(&mut buf)).await.expect("no voice relayed").unwrap();
    let p = cb.open(Direction::ToClient, &buf[..n]).unwrap();
    assert_eq!(p.client, ida);
    assert_eq!(voice::decode_voice(&p.payload).unwrap().2, b"opusdata");
    println!("encrypted voice relayed A -> B");

    // Screen share relay through the bulk connection.
    async fn bulk(addr: &str, kp: &KeyPair, token: [u8; 32]) -> (SecureReader<R>, SecureWriter<W>) {
        let s = TcpStream::connect(addr).await.unwrap();
        let (r, w) = s.into_split();
        let sec = transport::connect(r, w, &kp.private).await.unwrap();
        let (mut r, mut w) = (sec.reader, sec.writer);
        w.send(&C2S::AttachBulk(token)).await.unwrap();
        let a: BulkS2C = r.recv().await.unwrap();
        assert!(matches!(a, BulkS2C::Attached));
        (r, w)
    }
    let (mut bra, mut bwa) = bulk(&addr, &ka, ta).await;
    let (mut brb, _bwb) = bulk(&addr, &kb, tb).await;
    wa.send(&C2S::StartStream(StreamInfo { kind: StreamKind::Screen, title: "test".into(), width: 4, height: 4, fps: 5 })).await.unwrap();
    until(&mut rb, |m| matches!(m, S2C::ClientUpdated(ref c) if c.id == ida && c.streaming.is_some()).then_some(())).await;
    wb.send(&C2S::WatchStream { streamer: ida, watch: true }).await.unwrap();
    let kf: BulkS2C = tokio::time::timeout(Duration::from_secs(3), bra.recv()).await.unwrap().unwrap();
    assert!(matches!(kf, BulkS2C::KeyframeRequested));
    bwa.send(&BulkC2S::Frame(StreamFrame { width: 4, height: 4, keyframe: true, tiles: vec![Tile { x: 0, y: 0, w: 4, h: 4, jpeg: vec![1, 2, 3] }] }))
        .await
        .unwrap();
    let f: BulkS2C = tokio::time::timeout(Duration::from_secs(3), brb.recv()).await.unwrap().unwrap();
    assert!(matches!(f, BulkS2C::Frame { from, .. } if from == ida));
    println!("screen share frame relayed A -> server -> B");

    // Channel creation (Normal users may create temporary channels).
    let mut e = ChannelEdit::new_default(None);
    e.name = format!("Temp {}", now_unix());
    e.kind = ChannelKind::Temporary;
    wb.send(&C2S::CreateChannel(e)).await.unwrap();
    let created = until(&mut ra, |m| if let S2C::ChannelCreated(c) = m { Some(c) } else { None }).await;
    until(&mut ra, |m| matches!(m, S2C::ClientMoved { id, channel, .. } if id == idb && channel == created.id).then_some(())).await;
    println!("temporary channel created and creator moved in");

    // Join power: A creates a channel needing join power 50; B (Normal = 10) is refused.
    let mut e = ChannelEdit::new_default(None);
    e.name = format!("VIP {}", now_unix());
    e.kind = ChannelKind::Temporary;
    e.needed_join_power = 50;
    wa.send(&C2S::CreateChannel(e)).await.unwrap();
    let vip = until(&mut ra, |m| if let S2C::ChannelCreated(c) = m { Some(c) } else { None }).await;
    assert_eq!(vip.needed_join_power, 50);
    wb.send(&C2S::SwitchChannel { channel: vip.id, password: None }).await.unwrap();
    let err = until(&mut rb, |m| if let S2C::Error(e) = m { Some(e) } else { None }).await;
    assert!(err.contains("join power"), "unexpected: {err}");
    println!("join power enforced: \"{err}\"");
    wa.send(&C2S::SwitchChannel { channel: ch, password: None }).await.unwrap();
    wb.send(&C2S::Disconnect("bye".into())).await.unwrap();
    until(&mut ra, |m| matches!(m, S2C::ChannelDeleted(id) if id == created.id).then_some(())).await;
    println!("temporary channel removed after it became empty");
    println!("ALL SMOKE TESTS PASSED");
}
