//! Channel cloning test: `cargo run -p voxa-proto --example clone_test -- 127.0.0.1:9988 <data-dir>`
//! Settings, password, channel groups and sub-channels are copied; permissions apply.

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
    groups: Vec<GroupInfo>,
    channels: Vec<ChannelInfo>,
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
    let S2C::Welcome { your_id, your_uid, groups, channels, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    User { r, w, id: your_id, uid: your_uid, groups, channels }
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

async fn created(u: &mut User, name: &str) -> ChannelInfo {
    let name = name.to_string();
    until(u, |m| match m {
        S2C::ChannelCreated(c) if c.name == name => Some(c),
        _ => None,
    })
    .await
}

fn edit(name: &str, parent: Option<ChannelId>) -> ChannelEdit {
    ChannelEdit {
        parent,
        sort: 0,
        name: name.into(),
        topic: "Friday game night".into(),
        description: "Bring snacks".into(),
        max_clients: Some(12),
        kind: ChannelKind::Permanent,
        is_default: false,
        codec_quality: 9,
        needed_talk_power: 15,
        needed_join_power: 0,
        voice_encrypted: true,
        password: PasswordChange::Set("pw".into()),
    }
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());
    let dir = std::env::args().nth(2).unwrap_or_default();
    let mut admin = login(&addr, "Dylan").await;
    let key = std::fs::read_to_string(std::path::Path::new(&dir).join("privilege_key.txt"))
        .ok()
        .and_then(|t| t.lines().map(str::trim).find(|l| l.len() >= 20 && !l.contains(' ')).map(String::from))
        .expect("privilege key");
    admin.w.send(&C2S::UsePrivilegeKey(key)).await.unwrap();
    until(&mut admin, |m| matches!(m, S2C::Notice(_)).then_some(())).await;
    let mut liam = login(&addr, "Liam").await;
    let chan_admin = liam.groups.iter().find(|g| g.kind == GroupKind::Channel && g.name.contains("Admin")).expect("channel admin group").id;
    let default_ch = liam.channels.iter().find(|c| c.is_default).unwrap().id;

    // Original: "Games" (password, topic, limits, talk power) with sub-channel "Lobby",
    // and a channel below it so we can see the clone is inserted right after "Games".
    admin.w.send(&C2S::CreateChannel(edit("Games", None))).await.unwrap();
    let games = created(&mut admin, "Games").await;
    admin.w.send(&C2S::CreateChannel(ChannelEdit { password: PasswordChange::Keep, ..edit("Lobby", Some(games.id)) })).await.unwrap();
    let lobby = created(&mut admin, "Lobby").await;
    admin.w.send(&C2S::CreateChannel(ChannelEdit { password: PasswordChange::Keep, ..edit("Music", None) })).await.unwrap();
    let music = created(&mut admin, "Music").await;
    admin.w.send(&C2S::SetChannelGroup { uid: liam.uid.clone(), channel: games.id, group: chan_admin }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 1. A normal user may not clone.
    liam.w.send(&C2S::CloneChannel { source: games.id, name: "Nope".into(), with_children: false }).await.unwrap();
    let err = until(&mut liam, |m| if let S2C::Error(e) = m { Some(e) } else { None }).await;
    println!("1. normal user refused: \"{err}\"");

    // 2. Admin clones with sub-channels.
    admin.w.send(&C2S::CloneChannel { source: games.id, name: "Games 2".into(), with_children: true }).await.unwrap();
    // "Music" moves down one place first, then the copies are announced.
    let moved = until(&mut liam, |m| match m {
        S2C::ChannelEdited(c) if c.id == music.id => Some(c.sort),
        _ => None,
    })
    .await;
    let copy = created(&mut liam, "Games 2").await;
    let sub = created(&mut liam, "Lobby").await;
    assert_eq!((copy.topic.as_str(), copy.description.as_str()), ("Friday game night", "Bring snacks"));
    assert_eq!((copy.max_clients, copy.codec_quality, copy.needed_talk_power, copy.has_password, copy.kind), (Some(12), 9, 15, true, ChannelKind::Permanent));
    assert_eq!((copy.parent, copy.sort), (None, games.sort + 1));
    assert_eq!(sub.parent, Some(copy.id));
    assert_ne!(sub.id, lobby.id);
    assert_eq!(moved, music.sort + 1);
    println!("2. \"Games 2\" copied with topic, description, limits, quality, talk power and password; sub-channel copied; placed right below \"Games\"");

    // 3. The password is copied (Mia has no channel group, so it applies to her).
    let mut mia = login(&addr, "Mia").await;
    mia.w.send(&C2S::SwitchChannel { channel: copy.id, password: None }).await.unwrap();
    let err = until(&mut mia, |m| if let S2C::Error(e) = m { Some(e) } else { None }).await;
    println!("3a. Mia joining the copy without password: \"{err}\"");
    mia.w.send(&C2S::SwitchChannel { channel: copy.id, password: Some("pw".into()) }).await.unwrap();
    let mid = mia.id;
    until(&mut mia, |m| matches!(m, S2C::ClientMoved { id, channel, .. } if id == mid && channel == copy.id).then_some(())).await;
    println!("3b. with password \"pw\" she gets in");

    // 4. Liam's channel group (Channel Admin) was copied: he is admin in the copy too
    //    (which also lets him in without the password).
    liam.w.send(&C2S::SwitchChannel { channel: copy.id, password: None }).await.unwrap();
    let lid = liam.id;
    let cg = until(&mut liam, |m| match m {
        S2C::ClientUpdated(c) if c.id == lid && c.channel == copy.id => Some(c.channel_group),
        _ => None,
    })
    .await;
    assert_eq!(cg, chan_admin);
    println!("4. Liam is Channel Admin in the copy too");

    // 5. Names stay unique.
    admin.w.send(&C2S::CloneChannel { source: games.id, name: "games 2".into(), with_children: false }).await.unwrap();
    let err = until(&mut admin, |m| if let S2C::Error(e) = m { Some(e) } else { None }).await;
    println!("5. duplicate name refused: \"{err}\"");
    let _ = default_ch;
    println!("CLONE TEST PASSED");
}
