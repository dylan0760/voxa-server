//! Client version requirement and the Opus Music codec against a running server
//! (with `require_latest_client = true`):
//! `cargo run -p voxa-proto --example feature_test -- 127.0.0.1:9988`
//! Must be built from the client repository (it reports the client version).

use std::net::UdpSocket;
use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::timeout;
use voxa_proto::identity::KeyPair;
use voxa_proto::transport::{self, SecureReader, SecureWriter};
use voxa_proto::voice::{self, Direction, Kind, VoiceCipher};
use voxa_proto::*;

type R = tokio::net::tcp::OwnedReadHalf;
type W = tokio::net::tcp::OwnedWriteHalf;

struct User {
    r: SecureReader<R>,
    w: SecureWriter<W>,
    id: ClientId,
    voice: VoiceSetup,
}

async fn hello(addr: &str, nick: &str, version: &str) -> Result<User, String> {
    let kp = KeyPair::generate();
    let s = TcpStream::connect(addr).await.unwrap();
    let (r, w) = s.into_split();
    let sec = transport::connect(r, w, &kp.private).await.unwrap();
    let (mut r, mut w) = (sec.reader, sec.writer);
    w.send(&C2S::Hello {
        protocol: PROTOCOL_VERSION,
        version: version.into(),
        platform: "test".into(),
        nickname: nick.into(),
        server_password: None,
        default_channel: None,
        default_channel_password: None,
    })
    .await
    .unwrap();
    match r.recv::<S2C>().await.map_err(|e| e.to_string())? {
        S2C::Welcome { your_id, voice, .. } => Ok(User { r, w, id: your_id, voice }),
        S2C::Refused(e) => Err(e),
        m => Err(format!("unexpected {m:?}")),
    }
}

async fn until<T>(u: &mut User, mut f: impl FnMut(S2C) -> Option<T>) -> T {
    timeout(Duration::from_secs(10), async {
        loop {
            if let Some(t) = f(u.r.recv::<S2C>().await.unwrap()) {
                return t;
            }
        }
    })
    .await
    .expect("timed out")
}

fn udp(addr: &str, u: &User) -> (UdpSocket, VoiceCipher) {
    let host = addr.rsplit_once(':').unwrap().0;
    let sock = UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.connect(format!("{host}:{}", u.voice.udp_port)).unwrap();
    sock.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
    (sock, VoiceCipher::new(&u.voice.key))
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());

    // 1. Version requirement.
    let err = hello(&addr, "Old", "1.14.0").await.err().expect("an old client was let in");
    assert!(err.starts_with(UPDATE_REQUIRED), "{err}");
    println!("1. Voxa 1.14.0 refused: \"{}\"", err.trim_start_matches(UPDATE_REQUIRED));
    let mut a = hello(&addr, "Alice", APP_VERSION).await.expect("the current client was refused");
    let mut b = hello(&addr, "Bob", APP_VERSION).await.expect("the current client was refused");
    println!("1. Voxa {APP_VERSION} accepted");

    // 2. A channel with Opus Music keeps its codec (create and edit).
    let mut edit = ChannelEdit::new_default(None);
    edit.name = "Music".into();
    edit.kind = ChannelKind::Temporary;
    edit.codec_quality = pack_codec(VoiceCodec::OpusMusic, 8);
    a.w.send(&C2S::CreateChannel(edit.clone())).await.unwrap();
    let ch = until(&mut a, |m| if let S2C::ChannelCreated(c) = m { Some(c) } else { None }).await;
    assert_eq!((codec_of(ch.codec_quality), quality_of(ch.codec_quality)), (VoiceCodec::OpusMusic, 8));
    println!("2. channel created with {} quality 8 ({} kbit/s)", codec_of(ch.codec_quality).name(), channel_bitrate(ch.codec_quality) / 1000);
    edit.codec_quality = pack_codec(VoiceCodec::OpusMusic, 10);
    a.w.send(&C2S::EditChannel { id: ch.id, edit }).await.unwrap();
    let q = until(&mut a, |m| match m {
        S2C::ChannelEdited(c) if c.id == ch.id => Some(c.codec_quality),
        _ => None,
    })
    .await;
    assert_eq!((codec_of(q), quality_of(q)), (VoiceCodec::OpusMusic, 10));
    println!("2. edited to quality 10: still {}", codec_of(q).name());

    // 3. Stereo voice packets reach the other listener with the stereo flag.
    let bid = b.id;
    b.w.send(&C2S::SwitchChannel { channel: ch.id, password: None }).await.unwrap();
    until(&mut b, |m| matches!(m, S2C::ClientMoved { id, channel, .. } if id == bid && channel == ch.id).then_some(())).await;
    let (sa, ca) = udp(&addr, &a);
    let (sb, cb) = udp(&addr, &b);
    // Both register their UDP address with a ping.
    for (s, c, id) in [(&sa, &ca, a.id), (&sb, &cb, b.id)] {
        s.send(&c.seal(Direction::ToServer, Kind::Ping, id, 1, &0u64.to_be_bytes())).unwrap();
        let mut buf = [0u8; 2048];
        let n = s.recv(&mut buf).expect("pong");
        assert_eq!(c.open(Direction::ToClient, &buf[..n]).unwrap().kind, Kind::Pong);
    }
    let fake_opus = vec![0xABu8; 480]; // the size of a 192 kbit/s, 20 ms frame
    let payload = voice::encode_voice(1, voice::FLAG_STEREO, &fake_opus);
    sa.send(&ca.seal(Direction::ToServer, Kind::Voice, a.id, 2, &payload)).unwrap();
    let mut buf = [0u8; 2048];
    let n = sb.recv(&mut buf).expect("voice did not arrive");
    let pkt = cb.open(Direction::ToClient, &buf[..n]).unwrap();
    let (_, flags, opus) = voice::decode_voice(&pkt.payload).unwrap();
    assert_eq!((pkt.kind, pkt.client, flags, opus.len()), (Kind::Voice, a.id, voice::FLAG_STEREO, 480));
    println!("3. a 480 byte stereo music packet from Alice reached Bob with the stereo flag");
    println!("FEATURE TEST PASSED");
}
