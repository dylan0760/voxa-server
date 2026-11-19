//! Nickname uniqueness against a running server (1.26.0+):
//!   cargo run -p voxa-proto --example names_test -- 127.0.0.1:9987
//! Use a server with a fresh data directory (names stay reserved between runs).

use std::time::Duration;

use tokio::net::TcpStream;
use voxa_proto::identity::KeyPair;
use voxa_proto::transport::{self, SecureReader, SecureWriter};
use voxa_proto::*;

type R = tokio::net::tcp::OwnedReadHalf;
type W = tokio::net::tcp::OwnedWriteHalf;

async fn login(addr: &str, kp: &KeyPair, nick: &str, platform: &str) -> (SecureReader<R>, SecureWriter<W>, ClientInfo) {
    let s = TcpStream::connect(addr).await.unwrap();
    let (r, w) = s.into_split();
    let sec = transport::connect(r, w, &kp.private).await.unwrap();
    let (mut r, mut w) = (sec.reader, sec.writer);
    w.send(&C2S::Hello {
        protocol: PROTOCOL_VERSION,
        version: APP_VERSION.into(),
        platform: platform.into(),
        nickname: nick.into(),
        server_password: None,
        default_channel: None,
        default_channel_password: None,
    })
    .await
    .unwrap();
    let me = match r.recv::<S2C>().await.unwrap() {
        S2C::Welcome { your_id, clients, .. } => clients.into_iter().find(|c| c.id == your_id).unwrap(),
        other => panic!("expected welcome, got {other:?}"),
    };
    (r, w, me)
}

async fn until<T>(r: &mut SecureReader<R>, mut f: impl FnMut(S2C) -> Option<T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(t) = f(r.recv::<S2C>().await.unwrap()) {
                return t;
            }
        }
    })
    .await
    .expect("timed out waiting for message")
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or_else(|| "127.0.0.1:9987".into());
    let (dylan, other, third) = (KeyPair::generate(), KeyPair::generate(), KeyPair::generate());

    // The first identity to use a name owns it; the platform is reported as sent.
    let (_r1, w1, me1) = login(&addr, &dylan, "Dylan", "Android").await;
    assert_eq!(me1.nickname, "Dylan");
    assert_eq!(me1.platform, "Android");
    println!("ok: Dylan joined on Android");

    // An app cannot claim to be the web admin page.
    let (_rx, wx, mex) = login(&addr, &third, "Webby", "Web").await;
    assert_eq!(mex.platform, "Unknown", "platform 'Web' is reserved for web sessions");
    drop(wx);
    println!("ok: 'Web' platform refused for an app");

    // A look-alike while Dylan is online gets a numbered name and a notice.
    let (mut r2, mut w2, me2) = login(&addr, &other, "DyIan", "Windows").await;
    assert_ne!(me2.nickname.to_lowercase(), "dylan");
    assert!(me2.nickname.starts_with("DyIan"), "{}", me2.nickname);
    until(&mut r2, |m| matches!(m, S2C::Notice(t) if t.contains("belongs to someone else")).then_some(())).await;
    println!("ok: look-alike joined as {}", me2.nickname);

    // Dylan goes offline: the name stays reserved.
    drop(w1);
    tokio::time::sleep(Duration::from_millis(500)).await;
    w2.send(&C2S::UpdateSelf(SelfUpdate { nickname: Some("dylan".into()), ..Default::default() })).await.unwrap();
    until(&mut r2, |m| matches!(m, S2C::Error(t) if t.contains("belongs to someone else")).then_some(())).await;
    println!("ok: rename to an offline member's name refused");

    // A free name works, and the old numbered name is released.
    w2.send(&C2S::UpdateSelf(SelfUpdate { nickname: Some("Sam".into()), ..Default::default() })).await.unwrap();
    until(&mut r2, |m| matches!(m, S2C::ClientUpdated(c) if c.nickname == "Sam").then_some(())).await;
    println!("ok: renamed to Sam");

    // Dylan comes back and keeps the name.
    let (_r3, _w3, me3) = login(&addr, &dylan, "Dylan", "Android").await;
    assert_eq!(me3.nickname, "Dylan");
    println!("ok: Dylan kept the name\nnames_test passed");
}
