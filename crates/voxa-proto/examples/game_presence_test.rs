//! Game status against a running server (1.31.0+): someone who reports a game is shown
//! as playing it to the others, and stops when it is cleared.
//! `cargo run -p voxa-proto --example game_presence_test -- 127.0.0.1:9988 [--hold 20]`
//! `--hold` keeps the player connected (playing) for that many seconds, so another
//! client or the web page can be checked meanwhile.

use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::{timeout, Instant};
use voxa_proto::identity::KeyPair;
use voxa_proto::transport::{self, SecureReader, SecureWriter};
use voxa_proto::*;

type R = tokio::net::tcp::OwnedReadHalf;
type W = tokio::net::tcp::OwnedWriteHalf;

struct User {
    r: SecureReader<R>,
    w: SecureWriter<W>,
    id: ClientId,
}

async fn hello(addr: &str, nick: &str) -> User {
    let kp = KeyPair::generate();
    let s = TcpStream::connect(addr).await.unwrap();
    let (r, w) = s.into_split();
    let sec = transport::connect(r, w, &kp.private).await.unwrap();
    let (mut r, mut w) = (sec.reader, sec.writer);
    w.send(&C2S::Hello {
        protocol: PROTOCOL_VERSION,
        version: APP_VERSION.into(),
        platform: "Windows".into(),
        nickname: nick.into(),
        server_password: None,
        default_channel: None,
        default_channel_password: None,
    })
    .await
    .unwrap();
    let id = match r.recv::<S2C>().await.unwrap() {
        S2C::Welcome { your_id, .. } => your_id,
        m => panic!("{nick} was not let in: {m:?}"),
    };
    // Like Voxa: unidentified connections are disconnected after a few seconds.
    w.send(&C2S::Hardware(vec![hardware_token("x", nick)])).await.unwrap();
    User { r, w, id }
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

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    let addr = args.get(1).cloned().unwrap_or("127.0.0.1:9988".into());
    let hold = args.iter().position(|a| a == "--hold").and_then(|i| args.get(i + 1)).and_then(|s| s.parse().ok()).unwrap_or(0u64);

    let mut friend = hello(&addr, "Friend").await;
    let mut gamer = hello(&addr, "Gamer").await;
    let gid = gamer.id;
    gamer.w.send(&C2S::SetGame(Some("Fixture Quest".into()))).await.unwrap();
    until(&mut friend, |m| matches!(m, S2C::Playing { client, game: Some(ref g) } if client == gid && g == "Fixture Quest").then_some(())).await;
    println!("1. Friend sees Gamer playing \"Fixture Quest\"");

    // Someone joining later is told what the people already there are playing.
    let mut late = hello(&addr, "Latecomer").await;
    until(&mut late, |m| matches!(m, S2C::Playing { client, game: Some(_) } if client == gid).then_some(())).await;
    println!("2. a client connecting later is told too");

    if hold > 0 {
        println!("READY");
        let end = Instant::now() + Duration::from_secs(hold);
        // Keep every connection read and alive while something else looks.
        let mut ping = tokio::time::interval(Duration::from_secs(5));
        while Instant::now() < end {
            let _ = timeout(Duration::from_millis(100), gamer.r.recv::<S2C>()).await;
            let _ = timeout(Duration::from_millis(100), friend.r.recv::<S2C>()).await;
            let _ = timeout(Duration::from_millis(100), late.r.recv::<S2C>()).await;
            if timeout(Duration::ZERO, ping.tick()).await.is_ok() {
                for u in [&mut gamer, &mut friend, &mut late] {
                    u.w.send(&C2S::Ping(0)).await.unwrap();
                }
            }
        }
    }

    gamer.w.send(&C2S::SetGame(None)).await.unwrap();
    until(&mut friend, |m| matches!(m, S2C::Playing { client, game: None } if client == gid).then_some(())).await;
    println!("3. clearing the game reaches the others");
    println!("GAME PRESENCE TEST PASSED");
}
