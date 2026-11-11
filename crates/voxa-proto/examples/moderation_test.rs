//! Chat moderation test: `cargo run -p voxa-proto --example moderation_test -- 127.0.0.1:9988 <data-dir>`
//! Message deletion (live + history), per-user permissions and clearing a chat.

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
    let S2C::Welcome { your_id, your_uid, channels, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    let channel = channels.iter().find(|c| c.is_default).unwrap().id;
    User { r, w, id: your_id, uid: your_uid, channel }
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

async fn history(u: &mut User) -> Vec<String> {
    u.w.send(&C2S::FetchHistory { target: TextTarget::Channel(u.channel), before: None, limit: 100 }).await.unwrap();
    until(u, |m| if let S2C::History { messages, .. } = m { Some(messages.into_iter().map(|m| m.text).collect()) } else { None }).await
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
    let target = TextTarget::Channel(admin.channel);

    // 1. Admin deletes Liam's "yes" -> gone live for Liam and from history.
    let _games = say(&mut admin, "Games tonight, War Dogs. 🎮").await;
    let yes = say(&mut liam, "yes").await;
    admin.w.send(&C2S::DeleteMessage { target, id: yes }).await.unwrap();
    until(&mut liam, |m| matches!(m, S2C::MessageDeleted { id, .. } if id == yes).then_some(())).await;
    assert!(!history(&mut liam).await.contains(&"yes".to_string()));
    println!("1. admin deleted Liam's message: removed live and from history");

    // 2. Liam (Normal) may not delete the admin's message.
    let rules = say(&mut admin, "Server rules: be nice").await;
    liam.w.send(&C2S::DeleteMessage { target, id: rules }).await.unwrap();
    let err = until(&mut liam, |m| if let S2C::Error(e) = m { Some(e) } else { None }).await;
    println!("2. Liam without permission: \"{err}\"");

    // 3. Grant Liam the delete permission personally -> it works.
    admin.w.send(&C2S::SetClientPerms { uid: liam.uid.clone(), perms: perm::CHAT_DELETE }).await.unwrap();
    let p = until(&mut liam, |m| if let S2C::YourPerms(p) = m { Some(p) } else { None }).await;
    assert_eq!(p, perm::CHAT_DELETE);
    liam.w.send(&C2S::DeleteMessage { target, id: rules }).await.unwrap();
    until(&mut admin, |m| matches!(m, S2C::MessageDeleted { id, .. } if id == rules).then_some(())).await;
    println!("3. after granting \"Delete anyone's chat messages\" to Liam, he deleted the admin's message");

    // 4. Clearing the chat needs its own permission.
    liam.w.send(&C2S::ClearChat(target)).await.unwrap();
    let err = until(&mut liam, |m| if let S2C::Error(e) = m { Some(e) } else { None }).await;
    println!("4. Liam clearing without permission: \"{err}\"");
    admin.w.send(&C2S::SetClientPerms { uid: liam.uid.clone(), perms: perm::CHAT_DELETE | perm::CHAT_CLEAR }).await.unwrap();
    until(&mut liam, |m| matches!(m, S2C::YourPerms(p) if p & perm::CHAT_CLEAR != 0).then_some(())).await;
    liam.w.send(&C2S::ClearChat(target)).await.unwrap();
    let by = until(&mut admin, |m| if let S2C::ChatCleared { by, .. } = m { Some(by) } else { None }).await;
    assert!(history(&mut admin).await.is_empty());
    println!("5. after granting \"Clear entire chat history\", {by} cleared the chat; history is empty");

    // 5. Permissions persist and can be read back.
    admin.w.send(&C2S::GetClientPerms(liam.uid.clone())).await.unwrap();
    let back = until(&mut admin, |m| if let S2C::ClientPerms { perms, .. } = m { Some(perms) } else { None }).await;
    assert_eq!(back, perm::CHAT_DELETE | perm::CHAT_CLEAR);
    let _ = (admin.id, liam.id);
    println!("MODERATION TEST PASSED");
}
