//! Editing your own messages and avatars against a running server (1.15.0+):
//! `cargo run -p voxa-proto --example edit_avatar_test -- 127.0.0.1:9988 <data-dir>`
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
    channel: ChannelId,
}

async fn login(addr: &str, kp: &KeyPair, nick: &str) -> (User, Vec<S2C>) {
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
    let S2C::Welcome { your_id, your_uid, clients, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    let channel = clients.iter().find(|c| c.id == your_id).unwrap().channel;
    let mut u = User { r, w, uid: your_uid, channel };
    // What arrives right after the welcome (avatars etc.).
    let mut early = Vec::new();
    while let Ok(Ok(m)) = timeout(Duration::from_millis(400), u.r.recv::<S2C>()).await {
        early.push(m);
    }
    (u, early)
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

async fn say(u: &mut User, text: &str) -> MessageId {
    let target = TextTarget::Channel(u.channel);
    u.w.send(&C2S::SendText { target, text: text.into() }).await.unwrap();
    wait(u, 5, |m| match m {
        S2C::Text(t) if t.text == text => Some(t.id),
        _ => None,
    })
    .await
    .expect("message not echoed")
}

/// A PNG header of the given size (the server only reads the header).
fn png(w: u32, h: u32, filler: u8) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    v.extend_from_slice(&w.to_be_bytes());
    v.extend_from_slice(&h.to_be_bytes());
    v.extend(std::iter::repeat_n(filler, 200));
    v
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());
    let dir = std::env::args().nth(2).unwrap_or_default();
    let (admin_key, alice_key, bob_key) = (KeyPair::generate(), KeyPair::generate(), KeyPair::generate());
    let (mut admin, _) = login(&addr, &admin_key, "Admin").await;
    let key = std::fs::read_to_string(std::path::Path::new(&dir).join("privilege_key.txt"))
        .ok()
        .and_then(|t| t.lines().map(str::trim).find(|l| l.len() >= 20 && !l.contains(' ')).map(String::from))
        .expect("privilege key");
    admin.w.send(&C2S::UsePrivilegeKey(key)).await.unwrap();
    wait(&mut admin, 5, |m| matches!(m, S2C::Notice(_)).then_some(())).await;
    let (mut alice, _) = login(&addr, &alice_key, "Alice").await;
    let (mut bob, _) = login(&addr, &bob_key, "Bob").await;
    let target = TextTarget::Channel(alice.channel);

    // 1. Alice (Normal group) edits her own message; Bob sees the change live.
    let id = say(&mut alice, "helo wrold").await;
    alice.w.send(&C2S::EditMessage { target, id, text: "hello world".into() }).await.unwrap();
    let edited = wait(&mut bob, 5, |m| match m {
        S2C::MessageEdited { id: i, text, edited_at, .. } if i == id => Some((text, edited_at)),
        _ => None,
    })
    .await
    .expect("Bob did not see the edit");
    assert_eq!(edited.0, "hello world");
    println!("1. Alice corrected her message; Bob saw \"{}\"", edited.0);

    // 2. History has the new text, and says which messages were edited.
    bob.w.send(&C2S::FetchHistory { target, before: None, limit: 50 }).await.unwrap();
    let text = wait(&mut bob, 5, |m| match m {
        S2C::History { messages, .. } => messages.into_iter().find(|x| x.id == id).map(|x| x.text),
        _ => None,
    })
    .await;
    assert_eq!(text.as_deref(), Some("hello world"));
    let marks = wait(&mut bob, 5, |m| if let S2C::EditedMessages { edits, .. } = m { Some(edits) } else { None }).await.expect("no edit marks");
    assert!(marks.iter().any(|(i, at)| *i == id && *at == edited.1));
    println!("2. history shows the edited text and marks it as edited");

    // 3. Refusals: someone else's message, empty text, images, direct messages.
    bob.w.send(&C2S::EditMessage { target, id, text: "hacked".into() }).await.unwrap();
    println!("3. Bob editing Alice's message: \"{}\"", error(&mut bob).await);
    admin.w.send(&C2S::EditMessage { target, id, text: "admin rewrite".into() }).await.unwrap();
    println!("3. an admin editing Alice's message: \"{}\"", error(&mut admin).await);
    alice.w.send(&C2S::EditMessage { target, id, text: "   ".into() }).await.unwrap();
    println!("3. empty text: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::EditMessage { target, id, text: "📷 x\n[voxa-image:1:10x10]".into() }).await.unwrap();
    println!("3. turning it into an image: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::EditMessage { target: TextTarget::Private(0), id, text: "x".into() }).await.unwrap();
    println!("3. a direct message: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::EditMessage { target: TextTarget::Server, id, text: "x".into() }).await.unwrap();
    println!("3. the wrong chat: \"{}\"", error(&mut alice).await);
    let stored = {
        bob.w.send(&C2S::FetchHistory { target, before: None, limit: 50 }).await.unwrap();
        wait(&mut bob, 5, |m| match m {
            S2C::History { messages, .. } => messages.into_iter().find(|x| x.id == id).map(|x| x.text),
            _ => None,
        })
        .await
    };
    assert_eq!(stored.as_deref(), Some("hello world"), "a refused edit changed the message");

    // 4. Without the permission (taken from Normal), editing is refused.
    // The group list comes with the welcome; fetch it again from a fresh login.
    let s = TcpStream::connect(&addr).await.unwrap();
    let (r, w) = s.into_split();
    let sec = transport::connect(r, w, &KeyPair::generate().private).await.unwrap();
    let (mut r2, mut w2) = (sec.reader, sec.writer);
    w2.send(&C2S::Hello { protocol: PROTOCOL_VERSION, version: APP_VERSION.into(), platform: "test".into(), nickname: "Probe3".into(), server_password: None, default_channel: None, default_channel_password: None }).await.unwrap();
    let S2C::Welcome { groups, .. } = r2.recv::<S2C>().await.unwrap() else { panic!() };
    let mut normal = groups.into_iter().find(|g| g.id == 2).unwrap();
    assert!(normal.perms & perm::CHAT_EDIT_OWN != 0, "Normal should be allowed to edit by default");
    normal.perms &= !perm::CHAT_EDIT_OWN;
    admin.w.send(&C2S::EditGroup(normal.clone())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    alice.w.send(&C2S::EditMessage { target, id, text: "again".into() }).await.unwrap();
    println!("4. without the permission: \"{}\"", error(&mut alice).await);
    normal.perms |= perm::CHAT_EDIT_OWN;
    admin.w.send(&C2S::EditGroup(normal)).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;

    // 5. Avatars: Alice sets a picture and colour; everyone is told; the picture can be fetched.
    let pic = png(128, 128, 7);
    alice.w.send(&C2S::SetAvatar { color: Some(0x3BA55D), image: Some(pic.clone()) }).await.unwrap();
    let meta = wait(&mut bob, 5, |m| match m {
        S2C::AvatarInfo(v) => v.into_iter().find(|x| x.uid == alice.uid),
        _ => None,
    })
    .await
    .expect("Bob was not told about Alice's avatar");
    assert_eq!((meta.color, meta.image), (Some(0x3BA55D), avatar_hash(&pic)));
    bob.w.send(&C2S::GetAvatar(alice.uid.clone())).await.unwrap();
    let data = wait(&mut bob, 5, |m| match m {
        S2C::AvatarImage { uid, data, image } if uid == alice.uid && image == meta.image => Some(data),
        _ => None,
    })
    .await
    .expect("no picture");
    assert_eq!(data, pic);
    println!("5. Alice's picture ({} bytes) and colour reached Bob", data.len());
    // A newcomer gets everyone's avatars right after the welcome.
    let (_carol, early) = login(&addr, &KeyPair::generate(), "Carol").await;
    assert!(early.iter().any(|m| matches!(m, S2C::AvatarInfo(v) if v.iter().any(|x| x.uid == alice.uid))));
    println!("5. a newcomer was told about Alice's avatar on joining");

    // 6. Refused pictures.
    for (what, img, color) in [
        ("a GIF", b"GIF89a\x80\0\x80\0".to_vec(), None),
        ("too large", png(128, 128, 1).into_iter().chain(std::iter::repeat_n(0u8, MAX_AVATAR_BYTES)).collect(), None),
        ("a 100000 px picture", png(100_000, 100_000, 1), None),
        ("a 4 px picture", png(4, 4, 1), None),
        ("a broken header", b"\x89PNG\r\n\x1a\n\0\0\0\rIHDX".to_vec(), None),
    ] {
        alice.w.send(&C2S::SetAvatar { color, image: Some(img) }).await.unwrap();
        println!("6. {what}: \"{}\"", error(&mut alice).await);
    }
    alice.w.send(&C2S::SetAvatar { color: Some(0x1_000_000), image: None }).await.unwrap();
    println!("6. an invalid colour: \"{}\"", error(&mut alice).await);

    // 7. Only moderators remove pictures, and a removed one cannot be set again.
    bob.w.send(&C2S::ClearAvatar(alice.uid.clone())).await.unwrap();
    println!("7. Bob removing Alice's picture: \"{}\"", error(&mut bob).await);
    alice.w.send(&C2S::ClearAvatar(admin.uid.clone())).await.unwrap();
    let e = wait(&mut alice, 2, |m| if let S2C::Error(e) = m { Some(e) } else { None }).await;
    println!("7. Alice removing the admin's (absent) picture: {:?}", e);
    admin.w.send(&C2S::ClearAvatar(alice.uid.clone())).await.unwrap();
    let gone = wait(&mut bob, 5, |m| match m {
        S2C::AvatarInfo(v) => v.into_iter().find(|x| x.uid == alice.uid && x.image == 0),
        _ => None,
    })
    .await
    .expect("removal not announced");
    assert_eq!(gone.color, Some(0x3BA55D), "the colour stays");
    alice.w.send(&C2S::SetAvatar { color: Some(0x3BA55D), image: Some(pic.clone()) }).await.unwrap();
    println!("7. setting the removed picture again: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::SetAvatar { color: Some(0x3BA55D), image: Some(png(96, 96, 9)) }).await.unwrap();
    wait(&mut bob, 5, |m| match m {
        S2C::AvatarInfo(v) => v.into_iter().find(|x| x.uid == alice.uid && x.image != 0),
        _ => None,
    })
    .await
    .expect("a different picture should be accepted");
    println!("7. a different picture is accepted");
    println!("EDIT AVATAR TEST PASSED");
}
