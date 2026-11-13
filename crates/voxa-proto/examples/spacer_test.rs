//! Channel separators against a running server:
//! `cargo run -p voxa-proto --example spacer_test -- 127.0.0.1:9988 <data-dir>`
//! Nobody can get into a separator, by any route.

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
    channel: ChannelId,
    default: ChannelId,
}

async fn login(addr: &str, nick: &str, default_channel: Option<&str>) -> User {
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
        default_channel: default_channel.map(String::from),
        default_channel_password: None,
    })
    .await
    .unwrap();
    let S2C::Welcome { your_id, clients, channels, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    let channel = clients.iter().find(|c| c.id == your_id).unwrap().channel;
    let default = channels.iter().find(|c| c.is_default).unwrap().id;
    User { r, w, id: your_id, channel, default }
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

async fn error(u: &mut User) -> String {
    until(u, |m| if let S2C::Error(e) = m { Some(e) } else { None }).await
}

fn edit(name: &str, kind: ChannelKind) -> ChannelEdit {
    ChannelEdit { name: name.into(), kind, ..ChannelEdit::new_default(None) }
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());
    let dir = std::env::args().nth(2).unwrap_or_default();
    let mut admin = login(&addr, "Admin", None).await;
    let key = std::fs::read_to_string(std::path::Path::new(&dir).join("privilege_key.txt"))
        .ok()
        .and_then(|t| t.lines().map(str::trim).find(|l| l.len() >= 20 && !l.contains(' ')).map(String::from))
        .expect("privilege key");
    admin.w.send(&C2S::UsePrivilegeKey(key)).await.unwrap();
    until(&mut admin, |m| matches!(m, S2C::Notice(_)).then_some(())).await;

    // 1. Creating a separator does not move its creator into it.
    admin.w.send(&C2S::CreateChannel(edit("[cspacer]Games", ChannelKind::Permanent))).await.unwrap();
    let sep = until(&mut admin, |m| if let S2C::ChannelCreated(c) = m { Some(c) } else { None }).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    admin.w.send(&C2S::Ping(1)).await.unwrap();
    let aid = admin.id;
    let moved = timeout(Duration::from_millis(500), until(&mut admin, |m| matches!(m, S2C::ClientMoved { id, .. } if id == aid).then_some(()))).await;
    assert!(moved.is_err(), "the creator was moved into the separator");
    println!("1. created \"{}\": shown as a separator, the creator stays where he was", sep.name);

    // 2. Line separators side by side need numbers; temporary ones are refused.
    admin.w.send(&C2S::CreateChannel(edit("[*spacer]─", ChannelKind::Permanent))).await.unwrap();
    until(&mut admin, |m| matches!(m, S2C::ChannelCreated(ref c) if c.name == "[*spacer]─").then_some(())).await;
    admin.w.send(&C2S::CreateChannel(edit("[*spacer]─", ChannelKind::Permanent))).await.unwrap();
    println!("2. a second identical separator: \"{}\"", error(&mut admin).await);
    admin.w.send(&C2S::CreateChannel(edit("[*spacer1]─", ChannelKind::Permanent))).await.unwrap();
    until(&mut admin, |m| matches!(m, S2C::ChannelCreated(ref c) if c.name == "[*spacer1]─").then_some(())).await;
    println!("2. with a number in the tag it is accepted");
    admin.w.send(&C2S::CreateChannel(edit("[spacer]x", ChannelKind::Temporary))).await.unwrap();
    println!("2. temporary separator: \"{}\"", error(&mut admin).await);

    // 3. Nobody can join or be moved in.
    let mut bob = login(&addr, "Bob", None).await;
    bob.w.send(&C2S::SwitchChannel { channel: sep.id, password: None }).await.unwrap();
    let e = error(&mut bob).await;
    assert!(e.contains("separator"), "{e}");
    println!("3. Bob joining it: \"{e}\"");
    admin.w.send(&C2S::MoveClient { target: bob.id, channel: sep.id }).await.unwrap();
    let e = error(&mut admin).await;
    assert!(e.contains("separator"), "{e}");
    println!("3. the admin moving Bob into it: \"{e}\"");

    // 4. A connect link whose default channel is the separator lands in the default channel.
    let carl = login(&addr, "Carl", Some("[cspacer]Games")).await;
    assert_eq!(carl.channel, carl.default);
    println!("4. connecting with the separator as default channel: placed in the default channel");

    // 5. An occupied channel cannot be turned into a separator.
    admin.w.send(&C2S::CreateChannel(edit("Lounge", ChannelKind::Permanent))).await.unwrap();
    let lounge = until(&mut admin, |m| if let S2C::ChannelCreated(c) = m { Some(c) } else { None }).await;
    until(&mut admin, |m| matches!(m, S2C::ClientMoved { id, channel, .. } if id == aid && channel == lounge.id).then_some(())).await;
    let mut e5 = ChannelEdit::from_info(&lounge);
    e5.name = "[cspacer]Lounge".into();
    admin.w.send(&C2S::EditChannel { id: lounge.id, edit: e5 }).await.unwrap();
    println!("5. turning the occupied \"Lounge\" into a separator: \"{}\"", error(&mut admin).await);
    let _ = (bob.channel, bob.default);
    println!("SPACER TEST PASSED");
}
