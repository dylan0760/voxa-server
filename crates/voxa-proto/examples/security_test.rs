//! Security checks against a running server:
//! `cargo run -p voxa-proto --example security_test -- 127.0.0.1:9988 <data-dir>`
//! Use a fresh data directory: the test sets a server password at the end.

use std::time::{Duration, Instant};

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
    id: ClientId,
    kp: KeyPair,
    token: [u8; 32],
    server: ServerInfo,
    channels: Vec<ChannelInfo>,
}

async fn secure(addr: &str, kp: &KeyPair) -> (SecureReader<R>, SecureWriter<W>) {
    let s = TcpStream::connect(addr).await.unwrap();
    let (r, w) = s.into_split();
    let sec = transport::connect(r, w, &kp.private).await.unwrap();
    (sec.reader, sec.writer)
}

fn hello(nick: &str, password: Option<&str>) -> C2S {
    C2S::Hello {
        protocol: PROTOCOL_VERSION,
        version: APP_VERSION.into(),
        platform: "test".into(),
        nickname: nick.into(),
        server_password: password.map(String::from),
        default_channel: None,
        default_channel_password: None,
    }
}

async fn try_login(addr: &str, nick: &str, password: Option<&str>) -> Result<User, String> {
    let kp = KeyPair::generate();
    let (mut r, mut w) = secure(addr, &kp).await;
    w.send(&hello(nick, password)).await.unwrap();
    match r.recv::<S2C>().await.map_err(|e| e.to_string())? {
        S2C::Welcome { your_id, bulk_token, server, channels, .. } => {
            Ok(User { r, w, id: your_id, kp, token: bulk_token, server, channels })
        }
        S2C::Refused(e) => Err(e),
        other => Err(format!("unexpected {other:?}")),
    }
}

async fn login(addr: &str, nick: &str) -> User {
    try_login(addr, nick, None).await.expect("login")
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

/// Sends AttachBulk with `token` from identity `kp`; true if the server attached it.
async fn attach(addr: &str, kp: &KeyPair, token: [u8; 32]) -> bool {
    let (mut r, mut w) = secure(addr, kp).await;
    w.send(&C2S::AttachBulk(token)).await.unwrap();
    matches!(timeout(Duration::from_secs(3), r.recv::<BulkS2C>()).await, Ok(Ok(BulkS2C::Attached)))
}

fn temp_channel(name: &str, parent: Option<ChannelId>) -> ChannelEdit {
    ChannelEdit {
        parent,
        sort: 0,
        name: name.into(),
        topic: String::new(),
        description: String::new(),
        max_clients: None,
        kind: ChannelKind::Temporary,
        is_default: false,
        codec_quality: 6,
        needed_talk_power: 0,
        needed_join_power: 0,
        voice_encrypted: true,
        password: PasswordChange::Keep,
    }
}

async fn ping_ms(u: &mut User) -> u128 {
    let t = Instant::now();
    u.w.send(&C2S::Ping(42)).await.unwrap();
    until(u, |m| matches!(m, S2C::Pong(42)).then_some(())).await;
    t.elapsed().as_millis()
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());
    let dir = std::env::args().nth(2).unwrap_or_default();

    // 1. The bulk token only works for the identity it was issued to.
    let mut mia = login(&addr, "Mia").await;
    let thief = KeyPair::generate();
    assert!(!attach(&addr, &thief, mia.token).await, "a stolen bulk token was accepted");
    assert!(attach(&addr, &mia.kp, mia.token).await, "the owner could not attach");
    println!("1. bulk token: refused for another identity, accepted for its owner");

    // 2. Oversized first message: dropped before login, the server stays up.
    let kp = KeyPair::generate();
    let (mut r, mut w) = secure(&addr, &kp).await;
    let _ = w.send(&hello(&"x".repeat(200_000), None)).await;
    let res = timeout(Duration::from_secs(5), r.recv::<S2C>()).await.expect("no answer");
    assert!(res.is_err(), "a 200 KB hello was accepted: {res:?}");
    login(&addr, "StillUp").await;
    println!("2. 200 KB hello before login: connection dropped, server still accepts logins");

    // 3. Messages the server does not know are skipped, not fatal.
    mia.w.send(&(250u32, 1u8, 2u8)).await.unwrap();
    ping_ms(&mut mia).await;
    println!("3. unknown message type skipped, connection stays up");

    // 4. Channels cannot be nested deeper than 16 levels.
    let mut parent = None;
    let mut depth = 0;
    let err = loop {
        mia.w.send(&C2S::CreateChannel(temp_channel(&format!("Level {}", depth + 1), parent))).await.unwrap();
        let name = format!("Level {}", depth + 1);
        let got = until(&mut mia, |m| match m {
            S2C::ChannelCreated(c) if c.name == name => Some(Ok(c.id)),
            S2C::Error(e) => Some(Err(e)),
            _ => None,
        })
        .await;
        match got {
            Ok(id) => {
                depth += 1;
                parent = Some(id);
                assert!(depth <= 16, "nesting is not limited");
            }
            Err(e) => break e,
        }
    };
    assert_eq!(depth, 16);
    println!("4. nesting stops at {depth} levels: \"{err}\"");

    // 5. Wrong passwords: checked without blocking the server, then rate limited.
    let mut admin = login(&addr, "Admin").await;
    let key = std::fs::read_to_string(std::path::Path::new(&dir).join("privilege_key.txt"))
        .ok()
        .and_then(|t| t.lines().map(str::trim).find(|l| l.len() >= 20 && !l.contains(' ')).map(String::from))
        .expect("privilege key");
    admin.w.send(&C2S::UsePrivilegeKey(key)).await.unwrap();
    until(&mut admin, |m| matches!(m, S2C::Notice(_)).then_some(())).await;
    let s = admin.server.clone();
    admin
        .w
        .send(&C2S::EditServer(ServerEdit {
            name: s.name,
            welcome_message: s.welcome_message,
            host_message: s.host_message,
            max_clients: s.max_clients,
            password: PasswordChange::Set("correct horse".into()),
            default_server_group: s.default_server_group,
            default_channel_group: s.default_channel_group,
            channel_admin_group: s.channel_admin_group,
            chat_retention_days: s.chat_retention_days,
        }))
        .await
        .unwrap();
    until(&mut admin, |m| matches!(m, S2C::ServerUpdated(ref i) if i.has_password).then_some(())).await;
    assert!(try_login(&addr, "Right", Some("correct horse")).await.is_ok(), "the right password was refused");
    let flood: Vec<_> = (0..8).map(|i| {
        let addr = addr.clone();
        tokio::spawn(async move { try_login(&addr, &format!("Guess{i}"), Some("wrong")).await.err().unwrap_or_default() })
    }).collect();
    let mut worst = 0;
    for _ in 0..10 {
        worst = worst.max(ping_ms(&mut admin).await);
    }
    for f in flood {
        let e = f.await.unwrap();
        assert!(e.contains("password"), "unexpected answer: {e}");
    }
    println!("5a. 8 wrong passwords at once; meanwhile the server answered pings in at most {worst} ms");
    assert!(worst < 250, "the server stalled during password checks ({worst} ms)");
    let mut last = String::new();
    for i in 0..4 {
        last = try_login(&addr, &format!("More{i}"), Some("wrong")).await.err().unwrap_or_default();
    }
    assert!(last.contains("Too many"), "wrong passwords are not rate limited: {last}");
    println!("5b. after 10 wrong passwords: \"{last}\"");
    let _ = (mia.id, mia.channels.len(), admin.channels.len());
    println!("SECURITY TEST PASSED");
}
