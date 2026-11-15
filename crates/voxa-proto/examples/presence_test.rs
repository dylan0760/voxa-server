//! Being on the server without a channel, presences and invisibility (servers 1.17.0+):
//! `cargo run -p voxa-proto --example presence_test -- 127.0.0.1:9988 <data-dir>`
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
    uid: String,
    channel: ChannelId,
    clients: Vec<ClientInfo>,
}

async fn login(addr: &str, nick: &str, join: &str) -> User {
    let kp = KeyPair::generate();
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
        default_channel: Some(join.into()),
        default_channel_password: None,
    })
    .await
    .unwrap();
    let S2C::Welcome { your_id, your_uid, clients, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    let channel = clients.iter().find(|c| c.id == your_id).unwrap().channel;
    w.send(&C2S::Hardware(vec![hardware_token("x", nick)])).await.unwrap();
    let mut u = User { r, w, id: your_id, uid: your_uid, channel, clients };
    while timeout(Duration::from_millis(300), u.r.recv::<S2C>()).await.is_ok() {}
    u
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

async fn error(u: &mut User) -> String {
    wait(u, 5, |m| if let S2C::Error(e) = m { Some(e) } else { None }).await.expect("no error")
}

/// Nothing about `id` arrives within `secs`.
async fn silent_about(u: &mut User, id: ClientId, secs: u64) -> bool {
    wait(u, secs, |m| match m {
        S2C::ClientJoined(c) | S2C::ClientUpdated(c) if c.id == id => Some(()),
        S2C::ClientMoved { id: i, .. } | S2C::ClientLeft { id: i, .. } | S2C::ClientPresence { id: i, .. } if i == id => Some(()),
        _ => None,
    })
    .await
    .is_none()
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());
    let dir = std::env::args().nth(2).unwrap_or_default();
    let no_channel = join::build(true, false, "");
    let mut admin = login(&addr, "Admin", "").await;
    let key = std::fs::read_to_string(std::path::Path::new(&dir).join("privilege_key.txt"))
        .ok()
        .and_then(|t| t.lines().map(str::trim).find(|l| l.len() >= 20 && !l.contains(' ')).map(String::from))
        .expect("privilege key");
    admin.w.send(&C2S::UsePrivilegeKey(key)).await.unwrap();
    wait(&mut admin, 5, |m| matches!(m, S2C::Notice(_)).then_some(())).await;
    let mut bob = login(&addr, "Bob", "").await;
    let lobby = bob.channel;

    // 1. Alice joins the server without joining a channel; Bob sees her online.
    let mut alice = login(&addr, "Alice", &no_channel).await;
    assert_eq!(alice.channel, NO_CHANNEL);
    let seen = wait(&mut bob, 5, |m| if let S2C::ClientJoined(c) = m { (c.id == alice.id).then_some(c.channel) } else { None }).await;
    assert_eq!(seen, Some(NO_CHANNEL));
    println!("1. Alice is on the server without a channel, and Bob sees her online");

    // 2. Outside a channel: no channel chat, history, files, streams or voice.
    for (what, msg) in [
        ("channel chat", C2S::SendText { target: TextTarget::Channel(NO_CHANNEL), text: "hi".into() }),
        ("channel history (the server chat's slot)", C2S::FetchHistory { target: TextTarget::Channel(NO_CHANNEL), before: None, limit: 50 }),
        ("clearing it", C2S::ClearChat(TextTarget::Channel(NO_CHANNEL))),
        ("files", C2S::FileList { channel: NO_CHANNEL, path: String::new() }),
        ("screen sharing", C2S::StartStream(StreamInfo { kind: StreamKind::Screen, title: "x".into(), width: 640, height: 480, fps: 5 })),
    ] {
        alice.w.send(&msg).await.unwrap();
        println!("2. {what}: \"{}\"", error(&mut alice).await);
    }
    admin.w.send(&C2S::FetchHistory { target: TextTarget::Channel(NO_CHANNEL), before: None, limit: 50 }).await.unwrap();
    println!("2. even an admin reading \"channel 0\": \"{}\"", error(&mut admin).await);
    admin.w.send(&C2S::MoveClient { target: bob.id, channel: NO_CHANNEL }).await.unwrap();
    println!("2. moving someone to \"channel 0\": \"{}\"", error(&mut admin).await);
    // The Server tab only shows server events (1.19.0+); direct messages work without a channel.
    alice.w.send(&C2S::SendText { target: TextTarget::Server, text: "hello from the lobby".into() }).await.unwrap();
    println!("2. writing in the Server tab: \"{}\"", error(&mut alice).await);
    // Only "See server events" (admins by default) opens the Server tab's history.
    alice.w.send(&C2S::FetchHistory { target: TextTarget::Server, before: None, limit: 50 }).await.unwrap();
    let e = error(&mut alice).await;
    assert!(e.contains("See server events"), "{e}");
    println!("2. a normal user reading the Server tab: \"{e}\"");
    admin.w.send(&C2S::FetchHistory { target: TextTarget::Server, before: None, limit: 50 }).await.unwrap();
    assert!(wait(&mut admin, 5, |m| matches!(m, S2C::History { target: TextTarget::Server, .. }).then_some(())).await.is_some());
    println!("2. an admin reading the Server tab: ok");
    let bob_id = bob.id;
    alice.w.send(&C2S::SendText { target: TextTarget::Private(bob_id), text: "hello from the lobby".into() }).await.unwrap();
    assert!(wait(&mut bob, 5, |m| matches!(m, S2C::Text(t) if t.text == "hello from the lobby").then_some(())).await.is_some());
    println!("2. direct messages work without a channel");

    // 3. Joining and leaving a channel.
    alice.w.send(&C2S::SwitchChannel { channel: lobby, password: None }).await.unwrap();
    assert!(wait(&mut bob, 5, |m| matches!(m, S2C::ClientMoved { id, channel, .. } if id == alice.id && channel == lobby).then_some(())).await.is_some());
    alice.w.send(&C2S::SwitchChannel { channel: NO_CHANNEL, password: None }).await.unwrap();
    assert!(wait(&mut bob, 5, |m| matches!(m, S2C::ClientMoved { id, channel, .. } if id == alice.id && channel == NO_CHANNEL).then_some(())).await.is_some());
    println!("3. Alice joined the channel and left it again (still on the server)");

    // 4. Presences: busy, away, online.
    alice.w.send(&C2S::SetPresence(Presence::Busy)).await.unwrap();
    let p = wait(&mut bob, 5, |m| if let S2C::ClientPresence { id, presence } = m { (id == alice.id).then_some(presence) } else { None }).await;
    assert_eq!(p, Some(Presence::Busy));
    alice.w.send(&C2S::SetPresence(Presence::Away)).await.unwrap();
    let away = wait(&mut bob, 5, |m| if let S2C::ClientUpdated(c) = m { (c.id == alice.id && c.away.is_some()).then_some(()) } else { None }).await;
    assert!(away.is_some());
    alice.w.send(&C2S::SetPresence(Presence::Online)).await.unwrap();
    let back = wait(&mut bob, 5, |m| if let S2C::ClientUpdated(c) = m { (c.id == alice.id && c.away.is_none()).then_some(()) } else { None }).await;
    assert!(back.is_some());
    println!("4. Bob saw Alice go busy, away and back online");
    // Idle away (UpdateSelf) is cleared by choosing Online.
    alice.w.send(&C2S::UpdateSelf(SelfUpdate { away: Some(Some("idle".into())), ..Default::default() })).await.unwrap();
    assert!(wait(&mut bob, 5, |m| if let S2C::ClientUpdated(c) = m { (c.id == alice.id && c.away.is_some()).then_some(()) } else { None }).await.is_some());
    alice.w.send(&C2S::SetPresence(Presence::Online)).await.unwrap();
    assert!(wait(&mut bob, 5, |m| if let S2C::ClientUpdated(c) = m { (c.id == alice.id && c.away.is_none()).then_some(()) } else { None }).await.is_some());
    println!("4. choosing Online clears an idle away");

    // 5. Carol connects invisible: Bob does not see her, the admin does.
    let mut carol = login(&addr, "Carol", &join::build(true, true, "")).await;
    assert!(silent_about(&mut bob, carol.id, 2).await, "Bob was told about invisible Carol");
    assert!(admin.clients.iter().all(|c| c.id != carol.id));
    let admin_saw = wait(&mut admin, 3, |m| if let S2C::ClientJoined(c) = m { (c.id == carol.id).then_some(()) } else { None }).await;
    assert!(admin_saw.is_some(), "the admin should see invisible people");
    println!("5. Carol is invisible: Bob sees nothing, the admin sees her");
    let dave = login(&addr, "Dave", "").await;
    assert!(dave.clients.iter().all(|c| c.id != carol.id), "a newcomer got invisible Carol in the welcome");
    println!("5. a newcomer's client list leaves Carol out");

    // 6. A message to invisible Carol: Bob is told it was kept for later; Carol gets it.
    bob.w.send(&C2S::SendOffline { to_uid: carol.uid.clone(), text: "are you around?".into() }).await.unwrap();
    assert!(wait(&mut bob, 5, |m| matches!(m, S2C::OfflineQueued { .. }).then_some(())).await.is_some());
    assert!(wait(&mut carol, 5, |m| matches!(m, S2C::Text(t) if t.text == "are you around?").then_some(())).await.is_some());
    println!("6. Bob's message reached invisible Carol, and Bob only saw \"kept for later\"");
    bob.w.send(&C2S::Poke { target: carol.id, message: "boo".into() }).await.unwrap();
    assert_eq!(error(&mut bob).await, "Client not found");
    println!("6. poking invisible Carol looks like poking nobody");

    // 7. In a channel Carol is visible; out of it she vanishes again.
    carol.w.send(&C2S::SwitchChannel { channel: lobby, password: None }).await.unwrap();
    assert!(wait(&mut bob, 5, |m| if let S2C::ClientJoined(c) = m { (c.id == carol.id && c.channel == lobby).then_some(()) } else { None }).await.is_some());
    carol.w.send(&C2S::SwitchChannel { channel: NO_CHANNEL, password: None }).await.unwrap();
    assert!(wait(&mut bob, 5, |m| matches!(m, S2C::ClientLeft { id, .. } if id == carol.id).then_some(())).await.is_some());
    println!("7. Carol showed up while in the channel and vanished when she left it");
    // Going online makes her visible; invisible again hides her.
    carol.w.send(&C2S::SetPresence(Presence::Online)).await.unwrap();
    assert!(wait(&mut bob, 5, |m| matches!(m, S2C::ClientJoined(c) if c.id == carol.id).then_some(())).await.is_some());
    carol.w.send(&C2S::SetPresence(Presence::Invisible)).await.unwrap();
    assert!(wait(&mut bob, 5, |m| matches!(m, S2C::ClientLeft { id, .. } if id == carol.id).then_some(())).await.is_some());
    println!("7. going online / invisible shows / hides her");

    // 8. When invisible Carol disconnects, only the admin is told.
    carol.w.send(&C2S::Disconnect("bye".into())).await.unwrap();
    drop(carol);
    let admin_told = wait(&mut admin, 3, |m| matches!(m, S2C::ClientLeft { .. }).then_some(())).await;
    assert!(admin_told.is_some());
    assert!(silent_about(&mut bob, 999, 1).await);
    println!("8. invisible Carol left quietly (only the admin was told)");
    let _ = (dave, alice.uid);
    println!("PRESENCE TEST PASSED");
}
