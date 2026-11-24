//! "View as" (servers 1.30.0+): an administrator previews the server as Guest. The server
//! really treats them as Guest (hidden channels disappear, admin actions are refused)
//! until they stop. Members cannot use it, and nobody can view as more than they have.
//! `cargo run -p voxa-proto --example view_as_test -- 127.0.0.1:9988 <server data dir>`

use std::time::Duration;

use tokio::net::TcpStream;
use tokio::time::timeout;
use voxa_proto::identity::KeyPair;
use voxa_proto::transport::{self, SecureReader, SecureWriter};
use voxa_proto::*;

type R = tokio::net::tcp::OwnedReadHalf;
type W = tokio::net::tcp::OwnedWriteHalf;

const ADMIN: GroupId = 1;
const GUEST: GroupId = 3;

struct User {
    r: SecureReader<R>,
    w: SecureWriter<W>,
    id: ClientId,
    uid: String,
}

async fn login(addr: &str, nick: &str) -> User {
    let kp = KeyPair::generate();
    let uid = voxa_proto::identity::uid_from_public(&kp.public);
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
    let S2C::Welcome { your_id, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    w.send(&C2S::Hardware(vec![hardware_token("x", nick)])).await.unwrap();
    while let Ok(Ok(_)) = timeout(Duration::from_millis(400), r.recv::<S2C>()).await {}
    User { r, w, id: your_id, uid }
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

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());
    let dir = std::env::args().nth(2).unwrap_or_default();
    let mut admin = login(&addr, "Admin").await;
    let key = std::fs::read_to_string(std::path::Path::new(&dir).join("privilege_key.txt"))
        .ok()
        .and_then(|t| t.lines().map(str::trim).find(|l| l.len() >= 20 && !l.contains(' ')).map(String::from))
        .expect("privilege key");
    admin.w.send(&C2S::UsePrivilegeKey(key)).await.unwrap();
    wait(&mut admin, 5, |m| matches!(m, S2C::Notice(_)).then_some(())).await;

    // A channel Guests do not see.
    let mut e = ChannelEdit::new_default(None);
    e.name = "Admins only".into();
    admin.w.send(&C2S::CreateChannel(e)).await.unwrap();
    let aid = admin.id;
    let secret = wait(&mut admin, 5, |m| match m {
        S2C::ClientMoved { id, channel, .. } if id == aid => Some(channel),
        _ => None,
    })
    .await
    .expect("no channel");
    // Back to the default channel, so the preview does not start inside it.
    admin.w.send(&C2S::SwitchChannel { channel: 1, password: None }).await.unwrap();
    admin.w.send(&C2S::SetChannelHidden { channel: secret, groups: vec![GUEST] }).await.unwrap();
    wait(&mut admin, 5, |m| matches!(m, S2C::HiddenChannels(_)).then_some(())).await.expect("not hidden");

    // A member cannot preview.
    let mut member = login(&addr, "Member").await;
    member.w.send(&C2S::ViewAs(Some(vec![GUEST]))).await.unwrap();
    println!("1. a member using View as: \"{}\"", error(&mut member).await);

    // The admin views as Guest: the hidden channel disappears, admin actions are refused.
    admin.w.send(&C2S::ViewAs(Some(vec![GUEST]))).await.unwrap();
    let (mut told, mut gone) = (false, false);
    wait(&mut admin, 5, |m| {
        match m {
            S2C::ViewingAs(Some(g)) if g == vec![GUEST] => told = true,
            S2C::ChannelsVisible { hidden, .. } if hidden.contains(&secret) => gone = true,
            _ => {}
        }
        (told && gone).then_some(())
    })
    .await
    .expect("not viewing as Guest");
    println!("2. viewing as Guest: told, and the channel hidden from Guests is gone");
    let mut e = ChannelEdit::new_default(None);
    e.name = "Not allowed".into();
    admin.w.send(&C2S::CreateChannel(e)).await.unwrap();
    println!("2. creating a permanent channel as Guest: \"{}\"", error(&mut admin).await);
    admin.w.send(&C2S::SwitchChannel { channel: secret, password: None }).await.unwrap();
    println!("2. joining the hidden channel as Guest: \"{}\"", error(&mut admin).await);

    // Stop: everything is back.
    admin.w.send(&C2S::ViewAs(None)).await.unwrap();
    let (mut told, mut back) = (false, false);
    wait(&mut admin, 5, |m| {
        match m {
            S2C::ViewingAs(None) => told = true,
            S2C::ChannelsVisible { shown, .. } if shown.iter().any(|c| c.id == secret) => back = true,
            _ => {}
        }
        (told && back).then_some(())
    })
    .await
    .expect("not back");
    admin.w.send(&C2S::SwitchChannel { channel: secret, password: None }).await.unwrap();
    wait(&mut admin, 5, |m| matches!(m, S2C::ClientMoved { id, channel, .. } if id == aid && channel == secret).then_some(())).await.expect("could not join after stopping");
    println!("3. stopped: the channel is back and can be joined");

    // An admin group member can view as Admin (not more than they have); a group made
    // for the test with every permission is no more than an admin has either.
    admin.w.send(&C2S::ViewAs(Some(vec![ADMIN, GUEST]))).await.unwrap();
    wait(&mut admin, 5, |m| matches!(m, S2C::ViewingAs(Some(_))).then_some(())).await.expect("could not view as Admin + Guest");
    admin.w.send(&C2S::ViewAs(Some(vec![]))).await.unwrap();
    println!("4. viewing as no group at all: \"{}\"", error(&mut admin).await);
    admin.w.send(&C2S::ViewAs(Some(vec![9999]))).await.unwrap();
    println!("4. viewing as a group that does not exist: \"{}\"", error(&mut admin).await);

    // Security checks (1.30.1+): talk/join power escalation, hidden channel bypass, and
    // deleting a group that someone is viewing as.
    admin.w.send(&C2S::ViewAs(None)).await.unwrap();
    wait(&mut admin, 5, |m| matches!(m, S2C::ViewingAs(None)).then_some(())).await.expect("stop");
    // Create a Mod group (GROUP_MANAGE, low talk/join power) and a VIP group (fewer perms, high talk/join power).
    admin.w.send(&C2S::CreateGroup { kind: GroupKind::Server, name: "Mod".into(), copy_from: Some(GUEST) }).await.unwrap();
    let mut mod_g = wait(&mut admin, 5, |m| match m {
        S2C::GroupsUpdated(gs) => gs.into_iter().find(|g| g.name == "Mod"),
        _ => None,
    })
    .await
    .expect("Mod group");
    mod_g.perms |= perm::CHANNEL_VIEW | perm::CHANNEL_JOIN | perm::GROUP_MANAGE;
    mod_g.talk_power = 20;
    mod_g.join_power = 20;
    admin.w.send(&C2S::EditGroup(mod_g.clone())).await.unwrap();
    wait(&mut admin, 5, |m| matches!(m, S2C::GroupsUpdated(_)).then_some(())).await;

    admin.w.send(&C2S::CreateGroup { kind: GroupKind::Server, name: "VIP".into(), copy_from: None }).await.unwrap();
    let mut vip_g = wait(&mut admin, 5, |m| match m {
        S2C::GroupsUpdated(gs) => gs.into_iter().find(|g| g.name == "VIP"),
        _ => None,
    })
    .await
    .expect("VIP group");
    vip_g.perms = perm::CHANNEL_VIEW | perm::CHANNEL_JOIN;
    vip_g.talk_power = 90;
    vip_g.join_power = 90;
    admin.w.send(&C2S::EditGroup(vip_g.clone())).await.unwrap();
    wait(&mut admin, 5, |m| matches!(m, S2C::GroupsUpdated(_)).then_some(())).await;

    // Hide `secret` from Mod (not from Guest!), and assign `member` to Mod.
    admin.w.send(&C2S::SetChannelHidden { channel: secret, groups: vec![mod_g.id] }).await.unwrap();
    wait(&mut admin, 5, |m| matches!(m, S2C::HiddenChannels(_)).then_some(())).await;
    admin.w.send(&C2S::SetServerGroup { uid: member.uid.clone(), group: mod_g.id, add: true }).await.unwrap();
    while let Ok(Ok(_)) = timeout(Duration::from_millis(300), member.r.recv::<S2C>()).await {}

    // Mod cannot view as VIP (higher talk/join power).
    member.w.send(&C2S::ViewAs(Some(vec![vip_g.id]))).await.unwrap();
    println!("5. mod viewing as higher talk/join power VIP: \"{}\"", error(&mut member).await);

    // Mod views as Guest: must NOT see `secret` (which is hidden from Mod!).
    member.w.send(&C2S::ViewAs(Some(vec![GUEST]))).await.unwrap();
    let mut leaked = false;
    let saw_va = wait(&mut member, 5, |m| {
        if let S2C::ChannelsVisible { shown, .. } = &m {
            if shown.iter().any(|c| c.id == secret) {
                leaked = true;
            }
        }
        matches!(m, S2C::ViewingAs(Some(_))).then_some(())
    })
    .await;
    assert!(saw_va.is_some(), "mod could not view as Guest");
    assert!(!leaked, "ViewAs leaked a channel hidden from the viewer's own group!");
    member.w.send(&C2S::SwitchChannel { channel: secret, password: None }).await.unwrap();
    println!("6. mod viewing as Guest trying hidden channel: \"{}\"", error(&mut member).await);

    // Deleting a group someone is viewing as stops their preview cleanly.
    admin.w.send(&C2S::CreateGroup { kind: GroupKind::Server, name: "Temp".into(), copy_from: Some(GUEST) }).await.unwrap();
    let temp_g = wait(&mut admin, 5, |m| match m {
        S2C::GroupsUpdated(gs) => gs.into_iter().find(|g| g.name == "Temp"),
        _ => None,
    })
    .await
    .expect("Temp group");
    member.w.send(&C2S::ViewAs(Some(vec![temp_g.id]))).await.unwrap();
    wait(&mut member, 5, |m| matches!(m, S2C::ViewingAs(Some(v)) if v == vec![temp_g.id]).then_some(())).await.expect("mod view as Temp");
    admin.w.send(&C2S::DeleteGroup(temp_g.id)).await.unwrap();
    wait(&mut member, 5, |m| matches!(m, S2C::ViewingAs(None)).then_some(())).await.expect("deleting group did not cancel ViewAs");
    println!("7. deleting the viewed-as group cancelled ViewAs automatically");

    // Demoting `member` (removing Mod) cancels their ViewAs(Guest) automatically.
    member.w.send(&C2S::ViewAs(Some(vec![GUEST]))).await.unwrap();
    wait(&mut member, 5, |m| matches!(m, S2C::ViewingAs(Some(_))).then_some(())).await.expect("mod view as Guest again");
    admin.w.send(&C2S::SetServerGroup { uid: member.uid.clone(), group: mod_g.id, add: false }).await.unwrap();
    wait(&mut member, 5, |m| matches!(m, S2C::ViewingAs(None)).then_some(())).await.expect("demoting did not cancel ViewAs");
    println!("8. demoting a member cancelled their ViewAs automatically");

    // Disconnecting ends it: a new connection is the admin again.
    drop(admin);
    println!("VIEW AS TEST PASSED");
}
