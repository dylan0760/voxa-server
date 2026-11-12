//! Pinned messages test: `cargo run -p voxa-proto --example pin_test -- 127.0.0.1:9988 <data-dir> [--check-migration]`

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
    channel: ChannelId,
    groups: Vec<GroupInfo>,
}

async fn login(addr: &str, nick: &str) -> User {
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
        default_channel: None,
        default_channel_password: None,
    })
    .await
    .unwrap();
    let S2C::Welcome { channels, groups, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    let channel = channels.iter().find(|c| c.is_default).unwrap().id;
    User { r, w, channel, groups }
}

async fn until<T>(u: &mut User, mut f: impl FnMut(S2C) -> Option<T>) -> T {
    timeout(Duration::from_secs(5), async {
        loop {
            if let Some(t) = f(u.r.recv::<S2C>().await.unwrap()) {
                return t;
            }
        }
    })
    .await
    .expect("timed out")
}

async fn say(u: &mut User, text: &str) -> MessageId {
    let target = TextTarget::Channel(u.channel);
    u.w.send(&C2S::SendText { target, text: text.into() }).await.unwrap();
    until(u, |m| match m {
        S2C::Text(t) if t.text == text => Some(t.id),
        _ => None,
    })
    .await
}

async fn pins(u: &mut User) -> Vec<PinnedMessage> {
    let target = TextTarget::Channel(u.channel);
    u.w.send(&C2S::GetPins(target)).await.unwrap();
    until(u, |m| match m {
        S2C::Pins { target: t, pins } if t == target => Some(pins),
        _ => None,
    })
    .await
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());
    let dir = std::env::args().nth(2).unwrap_or_default();
    if std::env::args().nth(3).as_deref() == Some("--check-migration") {
        let u = login(&addr, "Checker").await;
        let g = u.groups.iter().find(|g| g.kind == GroupKind::Channel && g.name == "Channel Admin").expect("Channel Admin");
        assert!(g.perms & perm::CHAT_PIN != 0, "Channel Admin cannot pin after the upgrade");
        println!("MIGRATION OK: Channel Admin has \"Pin messages\"");
        return;
    }
    let mut admin = login(&addr, "Dylan").await;
    let key = std::fs::read_to_string(std::path::Path::new(&dir).join("privilege_key.txt"))
        .ok()
        .and_then(|t| t.lines().map(str::trim).find(|l| l.len() >= 20 && !l.contains(' ')).map(String::from))
        .expect("privilege key");
    admin.w.send(&C2S::UsePrivilegeKey(key)).await.unwrap();
    until(&mut admin, |m| matches!(m, S2C::Notice(_)).then_some(())).await;
    // `--demo`: post and pin two messages, then stay online (to look at it in a client).
    if std::env::args().nth(3).as_deref() == Some("--demo") {
        let target = TextTarget::Channel(admin.channel);
        let rules = say(&mut admin, "**Server rules**\n- be nice\n- no spam\n- voice chat after 8 pm").await;
        say(&mut admin, "Anyone up for games tonight?").await;
        let ip = say(&mut admin, "Game server IP: `192.168.1.50:27015`").await;
        for id in [rules, ip] {
            admin.w.send(&C2S::PinMessage { target, id, pin: true }).await.unwrap();
            tokio::time::sleep(Duration::from_millis(1100)).await;
        }
        println!("pinned 2 messages");
        tokio::time::sleep(Duration::from_secs(90)).await;
        return;
    }
    let mut liam = login(&addr, "Liam").await;
    let target = TextTarget::Channel(admin.channel);

    let rules = say(&mut admin, "Server rules: be nice, no spam").await;
    let other = say(&mut admin, "Games tonight at 8").await;

    // 1. Normal users may not pin.
    liam.w.send(&C2S::PinMessage { target, id: rules, pin: true }).await.unwrap();
    let err = until(&mut liam, |m| if let S2C::Error(e) = m { Some(e) } else { None }).await;
    println!("1. Liam without permission: \"{err}\"");

    // 2. Admin pins; everyone in the channel is told, and the pin is listed.
    admin.w.send(&C2S::PinMessage { target, id: rules, pin: true }).await.unwrap();
    let by = until(&mut liam, |m| match m {
        S2C::PinChanged { id, pinned: true, by, .. } if id == rules => Some(by),
        _ => None,
    })
    .await;
    let p = pins(&mut liam).await;
    assert_eq!(p.len(), 1);
    assert_eq!((p[0].message.text.as_str(), p[0].pinned_by.as_str()), ("Server rules: be nice, no spam", "Dylan"));
    println!("2. {by} pinned the rules; Liam was notified and sees 1 pin");

    // 3. Pinning twice changes nothing; a second message is listed first (newest pin).
    admin.w.send(&C2S::PinMessage { target, id: rules, pin: true }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(1100)).await;
    admin.w.send(&C2S::PinMessage { target, id: other, pin: true }).await.unwrap();
    until(&mut liam, |m| matches!(m, S2C::PinChanged { id, .. } if id == other).then_some(())).await;
    let p = pins(&mut liam).await;
    assert_eq!(p.iter().map(|x| x.message.id).collect::<Vec<_>>(), vec![other, rules]);
    println!("3. no duplicate pins; newest pin listed first");

    // 4. Unpin.
    admin.w.send(&C2S::PinMessage { target, id: other, pin: false }).await.unwrap();
    until(&mut liam, |m| matches!(m, S2C::PinChanged { id, pinned: false, .. } if id == other).then_some(())).await;
    assert_eq!(pins(&mut liam).await.len(), 1);
    println!("4. unpinned again");

    // 5. Deleting a pinned message removes its pin.
    admin.w.send(&C2S::DeleteMessage { target, id: rules }).await.unwrap();
    until(&mut liam, |m| matches!(m, S2C::MessageDeleted { id, .. } if id == rules).then_some(())).await;
    assert!(pins(&mut liam).await.is_empty());
    println!("5. deleting the pinned message removed the pin");

    // 6. Clearing the chat removes all pins.
    let note = say(&mut admin, "Note to self").await;
    admin.w.send(&C2S::PinMessage { target, id: note, pin: true }).await.unwrap();
    until(&mut liam, |m| matches!(m, S2C::PinChanged { id, .. } if id == note).then_some(())).await;
    admin.w.send(&C2S::ClearChat(target)).await.unwrap();
    until(&mut liam, |m| matches!(m, S2C::ChatCleared { .. }).then_some(())).await;
    assert!(pins(&mut liam).await.is_empty());
    println!("6. clearing the chat removed all pins");
    println!("PIN TEST PASSED");
}
