//! Server options (auto-away) and hardware bans against a running server (1.16.0+):
//! `cargo run -p voxa-proto --example hwban_test -- 127.0.0.1:9988 <data-dir>`
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
    id: ClientId,
    options: Option<ServerOptions>,
}

async fn login(addr: &str, kp: &KeyPair, nick: &str) -> Result<User, String> {
    let s = TcpStream::connect(addr).await.unwrap();
    let (r, w) = s.into_split();
    let sec = transport::connect(r, w, &kp.private).await.unwrap();
    let (mut r, mut w) = (sec.reader, sec.writer);
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
    let id = match r.recv::<S2C>().await.unwrap() {
        S2C::Welcome { your_id, .. } => your_id,
        S2C::Refused(e) => return Err(e),
        _ => return Err("unexpected".into()),
    };
    let mut u = User { r, w, id, options: None };
    while let Ok(Ok(m)) = timeout(Duration::from_millis(300), u.r.recv::<S2C>()).await {
        if let S2C::ServerOptions(o) = m {
            u.options = Some(o);
        }
    }
    Ok(u)
}

async fn wait<T>(u: &mut User, secs: u64, mut f: impl FnMut(S2C) -> Option<T>) -> Option<T> {
    timeout(Duration::from_secs(secs), async {
        loop {
            match u.r.recv::<S2C>().await {
                Ok(m) => {
                    if let Some(t) = f(m) {
                        return Some(t);
                    }
                }
                Err(_) => return None,
            }
        }
    })
    .await
    .ok()
    .flatten()
}

async fn error(u: &mut User) -> String {
    wait(u, 5, |m| if let S2C::Error(e) = m { Some(e) } else { None }).await.expect("no error")
}

/// Logs in and reports hardware; returns why the server sent us away, if it did.
async fn with_hardware(addr: &str, nick: &str, tokens: &[String]) -> (User, Option<String>) {
    let mut u = login(addr, &KeyPair::generate(), nick).await.unwrap();
    u.w.send(&C2S::Hardware(tokens.to_vec())).await.unwrap();
    let kicked = wait(&mut u, 2, |m| match m {
        S2C::Kicked(LeaveReason::Banned { reason, .. }) => Some(format!("banned: {reason}")),
        S2C::Kicked(r) => Some(format!("{r:?}")),
        _ => None,
    })
    .await;
    (u, kicked)
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());
    let dir = std::env::args().nth(2).unwrap_or_default();
    let mut admin = login(&addr, &KeyPair::generate(), "Admin").await.unwrap();
    let key = std::fs::read_to_string(std::path::Path::new(&dir).join("privilege_key.txt"))
        .ok()
        .and_then(|t| t.lines().map(str::trim).find(|l| l.len() >= 20 && !l.contains(' ')).map(String::from))
        .expect("privilege key");
    admin.w.send(&C2S::UsePrivilegeKey(key)).await.unwrap();
    wait(&mut admin, 5, |m| matches!(m, S2C::Notice(_)).then_some(())).await;
    admin.w.send(&C2S::Hardware(vec![hardware_token("server", "admin-pc")])).await.unwrap();

    // 1. Auto-away: 10 minutes by default; admins change it, at most 30; others cannot.
    let mut bob = login(&addr, &KeyPair::generate(), "Bob").await.unwrap();
    bob.w.send(&C2S::Hardware(vec![hardware_token("server", "bob-pc")])).await.unwrap();
    assert_eq!(bob.options.map(|o| o.auto_away_minutes), Some(DEFAULT_AUTO_AWAY_MINUTES));
    admin.w.send(&C2S::SetServerOptions(ServerOptions { auto_away_minutes: 45 })).await.unwrap();
    let got = wait(&mut bob, 5, |m| if let S2C::ServerOptions(o) = m { Some(o.auto_away_minutes) } else { None }).await;
    assert_eq!(got, Some(MAX_AUTO_AWAY_MINUTES), "not clamped to 30 / not announced");
    println!("1. auto-away is 10 min by default; an admin asking for 45 got {} (the maximum)", got.unwrap());
    bob.w.send(&C2S::SetServerOptions(ServerOptions { auto_away_minutes: 1 })).await.unwrap();
    println!("1. a normal user changing it: \"{}\"", error(&mut bob).await);
    let fresh = login(&addr, &KeyPair::generate(), "Late").await.unwrap();
    assert_eq!(fresh.options.map(|o| o.auto_away_minutes), Some(30), "newcomers get the current setting");

    // 2. Ban Alice by hardware only; a new identity on the same PC is turned away.
    let pc = vec![hardware_token("server", "mac:00:1a:2b:3c:4d:5e"), hardware_token("server", "machine:alice")];
    let (alice, kicked) = with_hardware(&addr, "Alice", &pc).await;
    assert!(kicked.is_none());
    admin.w.send(&C2S::BanWith { target: alice.id, uid: false, ip: false, hardware: false, duration_secs: None, reason: "x".into() }).await.unwrap();
    println!("2. a ban with nothing chosen: \"{}\"", error(&mut admin).await);
    admin.w.send(&C2S::BanWith { target: alice.id, uid: false, ip: false, hardware: true, duration_secs: None, reason: "cheating".into() }).await.unwrap();
    let mut alice = alice;
    let k = wait(&mut alice, 5, |m| if let S2C::Kicked(LeaveReason::Banned { reason, .. }) = m { Some(reason) } else { None }).await;
    assert_eq!(k.as_deref(), Some("cheating"));
    println!("2. Alice was banned by hardware only");
    let (_a2, kicked) = with_hardware(&addr, "Alice2", &pc).await;
    assert!(kicked.as_deref().unwrap_or("").starts_with("banned"), "a new identity on the banned PC got in: {kicked:?}");
    println!("2. a brand new identity on Alice's PC: {}", kicked.unwrap());
    // One matching token is enough (e.g. a new network card but the same Windows).
    let (_a3, kicked) = with_hardware(&addr, "Alice3", &[pc[1].clone(), hardware_token("server", "mac:new-card")]).await;
    assert!(kicked.is_some(), "one shared token should be enough");
    println!("2. the same PC with a new network card is also turned away");
    let (_other, kicked) = with_hardware(&addr, "Someone", &[hardware_token("server", "machine:other-pc")]).await;
    assert!(kicked.is_none(), "an unrelated PC was turned away");
    println!("2. a different PC gets in");

    // 3. The ban list says which bans include hardware; unbanning lets the PC back in.
    admin.w.send(&C2S::BanList).await.unwrap();
    let bans = wait(&mut admin, 5, |m| if let S2C::BanList(b) = m { Some(b) } else { None }).await.unwrap();
    let hw = wait(&mut admin, 5, |m| if let S2C::BannedHardware(v) = m { Some(v) } else { None }).await.unwrap();
    let ban = bans.iter().find(|b| b.name.as_deref() == Some("Alice")).expect("ban listed");
    assert!(ban.uid.is_none() && ban.ip.is_none() && hw.contains(&ban.id));
    println!("3. the ban list shows Alice's ban as hardware only");
    admin.w.send(&C2S::Unban(ban.id)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    let (_back, kicked) = with_hardware(&addr, "AliceBack", &pc).await;
    assert!(kicked.is_none(), "still banned after unbanning");
    println!("3. after unbanning, the PC gets in again");

    // 4. All three at once, and refusals.
    let (carl, _) = with_hardware(&addr, "Carl", &[hardware_token("server", "machine:carl")]).await;
    let mut bob2 = login(&addr, &KeyPair::generate(), "Bob2").await.unwrap();
    bob2.w.send(&C2S::BanWith { target: carl.id, uid: true, ip: true, hardware: true, duration_secs: Some(60), reason: "x".into() }).await.unwrap();
    println!("4. a normal user banning: \"{}\"", error(&mut bob2).await);
    admin.w.send(&C2S::BanWith { target: admin.id, uid: true, ip: false, hardware: false, duration_secs: None, reason: "x".into() }).await.unwrap();
    println!("4. banning yourself: \"{}\"", error(&mut admin).await);
    // Someone who never reported hardware cannot be hardware-banned.
    bob2.w.send(&C2S::Ping(1)).await.unwrap();
    admin.w.send(&C2S::BanWith { target: bob2.id, uid: false, ip: false, hardware: true, duration_secs: None, reason: "x".into() }).await.unwrap();
    println!("4. hardware ban without known hardware: \"{}\"", error(&mut admin).await);
    let mut carl = carl;
    admin.w.send(&C2S::BanWith { target: carl.id, uid: true, ip: false, hardware: true, duration_secs: Some(600), reason: "all".into() }).await.unwrap();
    assert!(wait(&mut carl, 5, |m| matches!(m, S2C::Kicked(LeaveReason::Banned { .. })).then_some(())).await.is_some());
    admin.w.send(&C2S::BanList).await.unwrap();
    let bans = wait(&mut admin, 5, |m| if let S2C::BanList(b) = m { Some(b) } else { None }).await.unwrap();
    let b = bans.iter().find(|b| b.name.as_deref() == Some("Carl")).unwrap();
    assert!(b.uid.is_some() && b.ip.is_none() && b.expires.is_some());
    println!("4. Carl banned by unique ID and hardware for 10 minutes");

    // 5. Invalid reports do not count, and Voxa 1.21.0+ must report within 20 seconds.
    let mut sly = login(&addr, &KeyPair::generate(), "Sly").await.unwrap();
    sly.w.send(&C2S::Hardware(vec!["not-a-token".into(), "ABCDEF".into()])).await.unwrap();
    println!("5. a report without valid tokens: \"{}\"", error(&mut sly).await);
    let gone = wait(&mut sly, 30, |m| if let S2C::Kicked(r) = m { Some(r) } else { None }).await;
    assert!(matches!(gone, Some(LeaveReason::Disconnected(_))), "a client that never reported hardware stayed: {gone:?}");
    println!("5. a client that never reported its hardware was disconnected: {gone:?}");
    // Bob did report and is still here.
    bob.w.send(&C2S::Ping(7)).await.unwrap();
    assert!(wait(&mut bob, 5, |m| matches!(m, S2C::Pong(7)).then_some(())).await.is_some());
    println!("HWBAN TEST PASSED");
}
