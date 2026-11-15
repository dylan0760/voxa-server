//! Group chats (servers 1.19.0+): creating, messages live and for people who are offline,
//! members only, inviting, renaming and leaving.
//! `cargo run -p voxa-proto --example group_test -- 127.0.0.1:9988`
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
    uid: String,
    /// What arrived while logging in (GroupChats, waiting messages...).
    first: Vec<S2C>,
}

async fn login(addr: &str, kp: &KeyPair, nick: &str, version: &str) -> User {
    try_login(addr, kp, nick, version).await.expect("refused")
}

/// None when the server turns the client away (e.g. too old with require_latest_client).
async fn try_login(addr: &str, kp: &KeyPair, nick: &str, version: &str) -> Option<User> {
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
    let S2C::Welcome { your_uid, .. } = r.recv::<S2C>().await.ok()? else { return None };
    w.send(&C2S::Hardware(vec![hardware_token("x", nick)])).await.unwrap();
    let mut first = Vec::new();
    while let Ok(Ok(m)) = timeout(Duration::from_millis(400), r.recv::<S2C>()).await {
        first.push(m);
    }
    Some(User { r, w, uid: your_uid, first })
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

/// The text of a group message for `gid`, if one arrives within `secs`.
async fn group_text(u: &mut User, gid: GroupChatId, secs: u64) -> Option<String> {
    wait(u, secs, |m| match m {
        S2C::Text(t) if t.target == TextTarget::Group(gid) => Some(t.text),
        _ => None,
    })
    .await
}

async fn quiet(u: &mut User, secs: u64) -> bool {
    wait(u, secs, |m| matches!(m, S2C::Text(_) | S2C::GroupChatUpdated(_) | S2C::GroupChats(_)).then_some(())).await.is_none()
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());
    let (ka, kb, kc, kd, ke) = (KeyPair::generate(), KeyPair::generate(), KeyPair::generate(), KeyPair::generate(), KeyPair::generate());
    let mut alice = login(&addr, &ka, "Alice", APP_VERSION).await;
    let mut bob = login(&addr, &kb, "Bob", APP_VERSION).await;
    // Carol visits once so the server knows her, then goes offline.
    let carol = login(&addr, &kc, "Carol", APP_VERSION).await;
    let carol_uid = carol.uid.clone();
    drop(carol);
    let mut dave = login(&addr, &kd, "Dave", APP_VERSION).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // 1. Alice starts a group chat with Bob and (offline) Carol.
    alice.w.send(&C2S::CreateGroupChat { name: "Raid night".into(), members: vec![bob.uid.clone(), carol_uid.clone()] }).await.unwrap();
    let info = wait(&mut bob, 5, |m| if let S2C::GroupChatUpdated(i) = m { Some(i) } else { None }).await.expect("Bob was not told about the group chat");
    assert_eq!((info.name.as_str(), info.members.len()), ("Raid night", 3));
    let gid = info.id;
    assert!(wait(&mut alice, 5, |m| matches!(m, S2C::GroupChatUpdated(ref i) if i.id == gid).then_some(())).await.is_some());
    assert!(quiet(&mut dave, 1).await, "Dave (not in it) heard about the group chat");
    println!("1. Alice started \"Raid night\" with Bob and Carol ({} people)", info.members.len());

    // 2. A message reaches Bob (and Alice's own session), and is kept for Carol.
    alice.w.send(&C2S::SendText { target: TextTarget::Group(gid), text: "Tonight at 8?".into() }).await.unwrap();
    assert_eq!(group_text(&mut bob, gid, 5).await.as_deref(), Some("Tonight at 8?"));
    assert_eq!(group_text(&mut alice, gid, 5).await.as_deref(), Some("Tonight at 8?"));
    assert!(quiet(&mut dave, 1).await, "Dave got a message of a group chat he is not in");
    println!("2. Bob got Alice's message live; Dave (not a member) got nothing");

    // 3. Outsiders can't use it; unknown people can't be added.
    dave.w.send(&C2S::SendText { target: TextTarget::Group(gid), text: "let me in".into() }).await.unwrap();
    println!("3. Dave writing to it: \"{}\"", error(&mut dave).await);
    dave.w.send(&C2S::InviteToGroupChat { id: gid, uids: vec![dave.uid.clone()] }).await.unwrap();
    println!("3. Dave adding himself: \"{}\"", error(&mut dave).await);
    alice.w.send(&C2S::InviteToGroupChat { id: gid, uids: vec!["nobody-at-all".into()] }).await.unwrap();
    println!("3. adding someone unknown: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::CreateGroupChat { name: "Alone".into(), members: vec![] }).await.unwrap();
    println!("3. a group chat with nobody else: \"{}\"", error(&mut alice).await);

    // 4. Carol comes online: she gets the group chat, then the waiting message.
    let mut carol = login(&addr, &kc, "Carol", APP_VERSION).await;
    let chats = carol.first.iter().find_map(|m| if let S2C::GroupChats(c) = m { Some(c.clone()) } else { None }).expect("no GroupChats");
    assert!(chats.iter().any(|c| c.id == gid));
    let waiting: Vec<String> = carol.first.iter().filter_map(|m| if let S2C::Text(t) = m { (t.target == TextTarget::Group(gid)).then(|| t.text.clone()) } else { None }).collect();
    assert_eq!(waiting, vec!["Tonight at 8?".to_string()]);
    println!("4. Carol came online and got the group chat and the message kept for her");
    // Delivered once only.
    drop(carol);
    let carol = login(&addr, &kc, "Carol", APP_VERSION).await;
    assert!(!carol.first.iter().any(|m| matches!(m, S2C::Text(t) if t.target == TextTarget::Group(gid))), "the waiting message came twice");
    println!("4. ...and only once");

    // 5. Bob adds Dave; renames; Bob leaves.
    bob.w.send(&C2S::InviteToGroupChat { id: gid, uids: vec![dave.uid.clone()] }).await.unwrap();
    let i = wait(&mut dave, 5, |m| if let S2C::GroupChatUpdated(i) = m { Some(i) } else { None }).await.expect("Dave was not added");
    assert_eq!(i.members.len(), 4);
    dave.w.send(&C2S::SendText { target: TextTarget::Group(gid), text: "Thanks for the invite".into() }).await.unwrap();
    assert_eq!(group_text(&mut alice, gid, 5).await.as_deref(), Some("Thanks for the invite"));
    println!("5. Bob added Dave, and Dave's message reached Alice");
    dave.w.send(&C2S::RenameGroupChat { id: gid, name: "Raid night (Friday)".into() }).await.unwrap();
    let renamed = wait(&mut alice, 5, |m| if let S2C::GroupChatUpdated(i) = m { Some(i.name) } else { None }).await;
    assert_eq!(renamed.as_deref(), Some("Raid night (Friday)"));
    bob.w.send(&C2S::LeaveGroupChat(gid)).await.unwrap();
    assert!(wait(&mut bob, 5, |m| matches!(m, S2C::GroupChatLeft(id) if id == gid).then_some(())).await.is_some());
    let after = wait(&mut alice, 5, |m| if let S2C::GroupChatUpdated(i) = m { Some(i.members.len()) } else { None }).await;
    assert_eq!(after, Some(3));
    bob.w.send(&C2S::SendText { target: TextTarget::Group(gid), text: "still here?".into() }).await.unwrap();
    println!("5. renamed; Bob left (Alice was told) and can't write anymore: \"{}\"", error(&mut bob).await);

    // 6. Clients older than 1.27.0 never get group messages (they could not read them).
    match try_login(&addr, &ke, "Eve", "1.26.0").await {
        Some(mut old) => {
            let eve_uid = old.uid.clone();
            alice.w.send(&C2S::InviteToGroupChat { id: gid, uids: vec![eve_uid] }).await.unwrap();
            alice.w.send(&C2S::SendText { target: TextTarget::Group(gid), text: "Eve, update Voxa!".into() }).await.unwrap();
            assert!(quiet(&mut old, 2).await, "an old client got group chat messages");
            println!("6. an old Voxa got nothing (its messages wait until it is updated)");
        }
        None => println!("6. (this server only lets in the newest Voxa, so old clients never see group chats)"),
    }
    let _ = (carol.uid, dave.uid);
    println!("GROUP TEST PASSED");
}
