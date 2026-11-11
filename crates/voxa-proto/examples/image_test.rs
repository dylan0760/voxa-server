//! Chat image test: `cargo run -p voxa-proto --example image_test -- 127.0.0.1:9988 <data-dir>`
//! Posting, receiving, fetching, access rules and clean-up of chat images (1.5.0+).

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
    br: SecureReader<R>,
    bw: SecureWriter<W>,
    id: ClientId,
    channel: ChannelId,
}

async fn open(addr: &str, kp: &KeyPair) -> (SecureReader<R>, SecureWriter<W>) {
    let s = TcpStream::connect(addr).await.unwrap();
    let (r, w) = s.into_split();
    let sec = transport::connect(r, w, &kp.private).await.unwrap();
    (sec.reader, sec.writer)
}

async fn login(addr: &str, nick: &str) -> User {
    let kp = KeyPair::generate();
    let (mut r, mut w) = open(addr, &kp).await;
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
    let S2C::Welcome { your_id, channels, bulk_token, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    let channel = channels.iter().find(|c| c.is_default).unwrap().id;
    let (mut br, mut bw) = open(addr, &kp).await;
    bw.send(&C2S::AttachBulk(bulk_token)).await.unwrap();
    assert!(matches!(br.recv::<BulkS2C>().await.unwrap(), BulkS2C::Attached));
    User { r, w, br, bw, id: your_id, channel }
}

async fn until<T>(u: &mut User, mut f: impl FnMut(S2C) -> Option<T>) -> T {
    timeout(Duration::from_secs(10), async {
        loop {
            if let Some(t) = f(u.r.recv::<S2C>().await.unwrap()) {
                return t;
            }
        }
    })
    .await
    .expect("timed out")
}

async fn bulk_until<T>(u: &mut User, mut f: impl FnMut(BulkS2C) -> Option<T>) -> T {
    timeout(Duration::from_secs(10), async {
        loop {
            if let Some(t) = f(u.br.recv::<BulkS2C>().await.unwrap()) {
                return t;
            }
        }
    })
    .await
    .expect("timed out")
}

/// Bytes that pass the server's format check (PNG signature + payload).
fn fake_png(len: usize, seed: u8) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
    v.extend((0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)));
    v
}

async fn post(u: &mut User, tid: u32, target: TextTarget, name: &str, data: Vec<u8>) -> Result<(), String> {
    u.bw.send(&BulkC2S::ChatImage { id: tid, target, name: name.into(), width: 640, height: 480, data }).await.unwrap();
    bulk_until(u, |m| match m {
        BulkS2C::ChatImageSent { id } if id == tid => Some(Ok(())),
        BulkS2C::TransferError { id, message } if id == tid => Some(Err(message)),
        _ => None,
    })
    .await
}

async fn fetch(u: &mut User, image: u64) -> Result<Vec<u8>, String> {
    u.bw.send(&BulkC2S::GetImage(image)).await.unwrap();
    bulk_until(u, |m| match m {
        BulkS2C::Image { image: i, data } if i == image => Some(Ok(data)),
        BulkS2C::ImageError { image: i, message } if i == image => Some(Err(message)),
        _ => None,
    })
    .await
}

async fn image_msg(u: &mut User) -> (ChatMessage, ImageRef) {
    until(u, |m| match m {
        S2C::Text(t) => ImageRef::parse(&t.text).map(|r| (t, r)),
        _ => None,
    })
    .await
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());
    let dir = std::env::args().nth(2).unwrap_or_default();
    // `--post <file>`: "Liam" posts a real image to the default channel and stays online
    // for a while (to look at it in a client).
    if std::env::args().nth(3).as_deref() == Some("--post") {
        let file = std::env::args().nth(4).expect("image file");
        let data = std::fs::read(&file).unwrap();
        let mut liam = login(&addr, "Liam").await;
        let chan = TextTarget::Channel(liam.channel);
        liam.w.send(&C2S::SendText { target: chan, text: "Check out this picture:".into() }).await.unwrap();
        let name = std::path::Path::new(&file).file_name().unwrap().to_string_lossy().to_string();
        post(&mut liam, 1, chan, &name, data).await.expect("post");
        println!("posted {name}");
        tokio::time::sleep(Duration::from_secs(60)).await;
        return;
    }
    let mut dylan = login(&addr, "Dylan").await;
    let key = std::fs::read_to_string(std::path::Path::new(&dir).join("privilege_key.txt"))
        .ok()
        .and_then(|t| t.lines().map(str::trim).find(|l| l.len() >= 20 && !l.contains(' ')).map(String::from))
        .expect("privilege key");
    dylan.w.send(&C2S::UsePrivilegeKey(key)).await.unwrap();
    until(&mut dylan, |m| matches!(m, S2C::Notice(_)).then_some(())).await;
    let mut liam = login(&addr, "Liam").await;
    let mut mia = login(&addr, "Mia").await;
    let chan = TextTarget::Channel(dylan.channel);

    // 1. Post a 3 MB image to the channel; everyone in it gets the image message.
    let big = fake_png(3 * 1024 * 1024, 7);
    post(&mut dylan, 1, chan, "cover.png", big.clone()).await.expect("post");
    let (msg, img) = image_msg(&mut liam).await;
    assert_eq!((img.name.as_str(), img.width, img.height), ("cover.png", 640, 480));
    println!("1. posted a 3 MB image; Liam got message {} -> image {} \"{}\"", msg.id, img.id, img.name);

    // 2. Liam downloads it: byte-for-byte identical.
    assert_eq!(fetch(&mut liam, img.id).await.expect("fetch"), big);
    println!("2. Liam fetched it: identical ({} bytes)", big.len());

    // 3. It is part of the stored (encrypted) history.
    liam.w.send(&C2S::FetchHistory { target: chan, before: None, limit: 50 }).await.unwrap();
    let hist = until(&mut liam, |m| if let S2C::History { messages, .. } = m { Some(messages) } else { None }).await;
    assert!(hist.iter().any(|m| ImageRef::parse(&m.text).map(|r| r.id) == Some(img.id)));
    println!("3. the image message is in the stored chat history");

    // 4. Non-images and oversized images are refused.
    let e = post(&mut dylan, 2, chan, "evil.exe", b"MZ\x90\x00 not an image".to_vec()).await.unwrap_err();
    println!("4a. non-image refused: \"{e}\"");
    let e = post(&mut dylan, 3, chan, "huge.png", fake_png(MAX_CHAT_IMAGE + 10, 1)).await.unwrap_err();
    println!("4b. oversized refused: \"{e}\"");

    // 5. Private image: only the two participants can fetch it.
    let secret = fake_png(50_000, 3);
    post(&mut dylan, 4, TextTarget::Private(liam.id), "secret.png", secret.clone()).await.expect("private post");
    let (pm, pimg) = image_msg(&mut liam).await;
    assert_eq!(pm.target, TextTarget::Private(dylan.id));
    assert_eq!(fetch(&mut liam, pimg.id).await.expect("liam private"), secret);
    let e = fetch(&mut mia, pimg.id).await.unwrap_err();
    println!("5. private image: Liam can see it, Mia gets \"{e}\"");

    // 6. A typed tag pointing at someone else's image neither grants access nor
    //    deletes the image when that fake message is deleted.
    post(&mut dylan, 5, chan, "second.png", fake_png(1000, 9)).await.expect("post 2");
    let (_, second) = image_msg(&mut mia).await;
    let fake = ImageRef { id: second.id, width: 1, height: 1, name: "fake".into() }.to_text();
    mia.w.send(&C2S::SendText { target: chan, text: fake.clone() }).await.unwrap();
    let fake_id = until(&mut mia, |m| match m {
        S2C::Text(t) if t.text == fake => Some(t.id),
        _ => None,
    })
    .await;
    mia.w.send(&C2S::DeleteMessage { target: chan, id: fake_id }).await.unwrap();
    until(&mut mia, |m| matches!(m, S2C::MessageDeleted { id, .. } if id == fake_id).then_some(())).await;
    assert!(fetch(&mut liam, second.id).await.is_ok());
    println!("6. deleting a message with a copied image tag left the real image alone");

    // 7. Deleting the real message deletes its image.
    dylan.w.send(&C2S::DeleteMessage { target: chan, id: msg.id }).await.unwrap();
    until(&mut liam, |m| matches!(m, S2C::MessageDeleted { id, .. } if id == msg.id).then_some(())).await;
    let e = fetch(&mut liam, img.id).await.unwrap_err();
    println!("7. message deleted -> image gone: \"{e}\"");

    // 8. Clearing the chat removes its images too.
    dylan.w.send(&C2S::ClearChat(chan)).await.unwrap();
    until(&mut liam, |m| matches!(m, S2C::ChatCleared { .. }).then_some(())).await;
    assert!(fetch(&mut liam, second.id).await.is_err());
    println!("8. chat cleared -> its images are gone");
    let _ = mia.id;
    println!("IMAGE TEST PASSED");
}
