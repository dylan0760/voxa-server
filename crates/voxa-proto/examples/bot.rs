//! Demo bot for UI testing: joins a server, creates a couple of channels, chats and
//! "talks" (sends voice packets) for a while. `cargo run --example bot -- 127.0.0.1:9987 Alice 60`

use std::time::Duration;

use tokio::net::{TcpStream, UdpSocket};
use voxa_proto::identity::KeyPair;
use voxa_proto::transport;
use voxa_proto::voice::{self, Direction, Kind, VoiceCipher};
use voxa_proto::*;

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9987".into());
    let nick = std::env::args().nth(2).unwrap_or("Alice".into());
    let secs: u64 = std::env::args().nth(3).and_then(|s| s.parse().ok()).unwrap_or(60);
    let kp = KeyPair::generate();
    let s = TcpStream::connect(&addr).await.unwrap();
    let (r, w) = s.into_split();
    let sec = transport::connect(r, w, &kp.private).await.unwrap();
    let (mut r, mut w) = (sec.reader, sec.writer);
    w.send(&C2S::Hello {
        protocol: PROTOCOL_VERSION,
        version: APP_VERSION.into(),
        platform: "Windows".into(),
        nickname: nick.clone(),
        server_password: None,
        default_channel: None,
        default_channel_password: None,
    })
    .await
    .unwrap();
    let S2C::Welcome { your_id, voice: v, channels, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    let ch = channels.iter().find(|c| c.is_default).unwrap().id;
    tokio::spawn(async move { while r.recv::<S2C>().await.is_ok() {} });
    for (name, topic) in [("Gaming", "Squad up here"), ("Music", "Chill tunes")] {
        let mut e = ChannelEdit::new_default(None);
        e.name = name.into();
        e.topic = topic.into();
        e.kind = ChannelKind::Temporary;
        let _ = w.send(&C2S::CreateChannel(e)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let _ = w.send(&C2S::SwitchChannel { channel: ch, password: None }).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    for t in ["Hey everyone! 👋", "Anyone up for a game tonight? 🎮🔥", "Chat history is now stored encrypted on the server, nice. 🔒😎", "🎉🥳👍"] {
        let _ = w.send(&C2S::SendText { target: TextTarget::Channel(ch), text: t.into() }).await;
        tokio::time::sleep(Duration::from_millis(400)).await;
    }
    let host = addr.rsplit_once(':').unwrap().0.to_string();
    let u = UdpSocket::bind("0.0.0.0:0").await.unwrap();
    u.connect(format!("{host}:{}", v.udp_port)).await.unwrap();
    let c = VoiceCipher::new(&v.key);
    let mut seq = 1u64;
    let start = std::time::Instant::now();
    while start.elapsed().as_secs() < secs {
        // Talk for 2 s, pause 1 s.
        let talking = start.elapsed().as_millis() % 3000 < 2000;
        if talking {
            seq += 1;
            let _ = u.send(&c.seal(Direction::ToServer, Kind::Voice, your_id, seq, &voice::encode_voice(seq as u16, 0, &[0xF8, 0xFF, 0xFE]))).await;
        }
        if seq % 100 == 0 {
            let _ = w.send(&C2S::Ping(0)).await;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
        if !talking {
            seq += 1;
        }
    }
}
