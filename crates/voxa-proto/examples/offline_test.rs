//! People list and offline direct messages against a running server:
//! `cargo run -p voxa-proto --example offline_test -- 127.0.0.1:9988 <data-dir>`
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
}

async fn login(addr: &str, kp: &KeyPair, nick: &str) -> User {
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
    let S2C::Welcome { your_uid, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    User { r, w, uid: your_uid }
}

/// The next message matching `f`, or None after `secs`.
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
    // Two phases: "send" leaves the messages (then the server is stopped and voxa.db is
    // checked for the text, which needs the server closed), "receive" delivers them.
    let phase = std::env::args().nth(3).unwrap_or_default();
    let keys = std::path::Path::new(&dir).join("offline_test_keys.txt");
    if phase == "receive" {
        let saved = std::fs::read_to_string(&keys).expect("run the send phase first");
        let mut parts = saved.lines();
        let alice_key = KeyPair::import(parts.next().unwrap()).unwrap();
        let bob_key = KeyPair::import(parts.next().unwrap()).unwrap();
        receive(&addr, &alice_key, &bob_key).await;
        return;
    }
    let secret = "meet me at 8, the code is swordfish";
    let (alice_key, bob_key) = (KeyPair::generate(), KeyPair::generate());
    std::fs::write(&keys, format!("{}\n{}\n", alice_key.export(), bob_key.export())).unwrap();

    // Bob visits once, then leaves.
    let mut bob = login(&addr, &bob_key, "Bob").await;
    let bob_uid = bob.uid.clone();
    bob.w.send(&C2S::Disconnect("bye".into())).await.unwrap();
    drop(bob);
    tokio::time::sleep(Duration::from_millis(500)).await;

    // 1. Alice sees Bob in the People list (offline, with a last-seen time).
    let mut alice = login(&addr, &alice_key, "Alice").await;
    alice.w.send(&C2S::GetPeople).await.unwrap();
    let people = wait(&mut alice, 5, |m| if let S2C::People(p) = m { Some(p) } else { None }).await.expect("no people list");
    let b = people.iter().find(|p| p.uid == bob_uid).expect("Bob is not listed");
    assert!(b.last_seen > 0 && b.nickname == "Bob");
    println!("1. People list: {} people, Bob last seen {}s ago", people.len(), now_unix().saturating_sub(b.last_seen));

    // 2. Offline messages are accepted for Bob; refusals for nonsense.
    alice.w.send(&C2S::SendOffline { to_uid: bob_uid.clone(), text: secret.into() }).await.unwrap();
    let queued = wait(&mut alice, 5, |m| match m {
        S2C::OfflineQueued { to_uid, .. } => Some(to_uid),
        S2C::Error(e) => panic!("refused: {e}"),
        _ => None,
    })
    .await;
    assert_eq!(queued.as_deref(), Some(bob_uid.as_str()));
    alice.w.send(&C2S::SendOffline { to_uid: bob_uid.clone(), text: "second one".into() }).await.unwrap();
    wait(&mut alice, 5, |m| matches!(m, S2C::OfflineQueued { .. }).then_some(())).await.expect("second not queued");
    println!("2. two messages left for Bob");
    alice.w.send(&C2S::SendOffline { to_uid: "nobody".into(), text: "hi".into() }).await.unwrap();
    println!("2. to an unknown person: \"{}\"", error(&mut alice).await);
    alice.w.send(&C2S::SendOffline { to_uid: alice.uid.clone(), text: "hi".into() }).await.unwrap();
    println!("2. to herself: \"{}\"", error(&mut alice).await);

    println!("SEND PHASE PASSED");
}

async fn receive(addr: &str, alice_key: &KeyPair, bob_key: &KeyPair) {
    let addr = addr.to_string();
    let mut alice = login(&addr, alice_key, "Alice").await;
    let bob_uid = voxa_proto::identity::uid_from_public(&bob_key.public);
    let secret = "meet me at 8, the code is swordfish";
    // 4. Bob comes back and gets both, in order; they are then gone from the server.
    let mut bob = login(&addr, bob_key, "Bob").await;
    let msgs = wait(&mut bob, 5, |m| if let S2C::OfflineMessages(v) = m { Some(v) } else { None }).await.expect("nothing delivered");
    assert_eq!(msgs.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), vec![secret, "second one"]);
    assert!(msgs.iter().all(|m| m.from_uid == alice.uid && m.from_name == "Alice"));
    println!("4. Bob got {} messages from Alice on login", msgs.len());
    // While Bob is online, offline messages to him are refused (send normally instead).
    alice.w.send(&C2S::SendOffline { to_uid: bob_uid.clone(), text: "hi".into() }).await.unwrap();
    println!("4. while Bob is online: \"{}\"", error(&mut alice).await);
    bob.w.send(&C2S::Disconnect("bye".into())).await.unwrap();
    drop(bob);
    tokio::time::sleep(Duration::from_millis(500)).await;
    let mut bob = login(&addr, &bob_key, "Bob").await;
    let again = wait(&mut bob, 2, |m| matches!(m, S2C::OfflineMessages(_)).then_some(())).await;
    assert!(again.is_none(), "delivered twice");
    println!("4. on the next login nothing is delivered again");
    println!("OFFLINE TEST PASSED");
}
