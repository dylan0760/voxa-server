//! GIF search through the server (servers 1.27.0+), against a fake GIF service this test
//! runs on 127.0.0.1:9979 (the server's `gif_api_base`): the server announces the service,
//! searches for its people (caching answers), only posts GIFs it handed out itself, and
//! limits how fast people search.
//! `cargo run -p voxa-proto --example gif_test -- 127.0.0.1:9988 <server data dir>`
//! The server needs `gif_api_key = "test-key"` and `gif_api_base = "http://127.0.0.1:9979"`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
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
    welcome: Vec<S2C>,
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
    let S2C::Welcome { your_id, clients, .. } = r.recv::<S2C>().await.unwrap() else { panic!("refused") };
    let channel = clients.iter().find(|c| c.id == your_id).map(|c| c.channel).unwrap();
    w.send(&C2S::Hardware(vec![hardware_token("x", nick)])).await.unwrap();
    let mut welcome = Vec::new();
    while let Ok(Ok(m)) = timeout(Duration::from_millis(400), r.recv::<S2C>()).await {
        welcome.push(m);
    }
    User { r, w, channel, welcome }
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
    wait(u, 20, |m| if let S2C::Error(e) = m { Some(e) } else { None }).await.expect("no error")
}

/// A tiny HTTP server answering like KLIPY's (Tenor v2) API.
async fn fake_service(hits: Arc<AtomicUsize>, last: Arc<parking_lot_free::Last>) {
    let l = TcpListener::bind("127.0.0.1:9979").await.expect("port 9979");
    loop {
        let Ok((mut s, _)) = l.accept().await else { continue };
        let (hits, last) = (hits.clone(), last.clone());
        tokio::spawn(async move {
            let mut buf = vec![0u8; 8192];
            let n = s.read(&mut buf).await.unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.split_whitespace().nth(1).unwrap_or_default().to_string();
            hits.fetch_add(1, Ordering::SeqCst);
            last.set(path.clone());
            let body = if path.contains("pos=p2") {
                r#"{"results":[{"id":"c","content_description":"Third","media_formats":{"gif":{"url":"https://gif.invalid/c.gif","dims":[100,100]},"tinygif":{"url":"https://gif.invalid/c-s.gif","dims":[50,50]}}}],"next":""}"#.to_string()
            } else {
                r#"{"results":[{"id":"a","content_description":"Cat dance","media_formats":{"gif":{"url":"https://gif.invalid/a.gif","dims":[480,270]},"tinygif":{"url":"https://gif.invalid/a-s.gif","dims":[220,124]}}},{"id":"b","content_description":"Bad","media_formats":{"gif":{"url":"https://127.0.0.1/b.gif","dims":[1,1]},"tinygif":{"url":"https://127.0.0.1/b.gif","dims":[1,1]}}}],"next":"p2"}"#.to_string()
            };
            let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = s.write_all(resp.as_bytes()).await;
        });
    }
}

mod parking_lot_free {
    use std::sync::Mutex;
    #[derive(Default)]
    pub struct Last(Mutex<String>);
    impl Last {
        pub fn set(&self, s: String) {
            *self.0.lock().unwrap() = s;
        }
        pub fn get(&self) -> String {
            self.0.lock().unwrap().clone()
        }
    }
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9988".into());
    let hits = Arc::new(AtomicUsize::new(0));
    let last = Arc::new(parking_lot_free::Last::default());
    tokio::spawn(fake_service(hits.clone(), last.clone()));
    tokio::time::sleep(Duration::from_millis(200)).await;

    let mut u = login(&addr, "Gif fan").await;
    let provider = u.welcome.iter().find_map(|m| if let S2C::GifSearch(p) = m { Some(p.clone()) } else { None }).expect("no GifSearch after Welcome");
    assert_eq!(provider.as_deref(), Some("KLIPY"));
    println!("1. the server announces its GIF service: {provider:?}");

    u.w.send(&C2S::SearchGifs { query: "Cat  Dance\u{7}".into(), next: None }).await.unwrap();
    let (q, items, next) = wait(&mut u, 20, |m| if let S2C::Gifs { query, items, next } = m { Some((query, items, next)) } else { None }).await.expect("no results");
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].title, "Cat dance");
    assert_eq!((items[0].width, items[0].height), (480, 270));
    assert_eq!(next.as_deref(), Some("p2"));
    let path = last.get();
    assert!(path.starts_with("/v2/search?key=test-key&client_key=voxa&q=cat%20%20dance&limit=24"), "{path}");
    assert!(path.contains("contentfilter=medium") && path.contains("media_filter=gif,tinygif"), "{path}");
    assert!(!items[0].id.contains("gif.invalid"), "the server must not hand out the service's addresses");
    println!("2. search \"{q}\": {} results, asked the service {path}", items.len());

    let before = hits.load(Ordering::SeqCst);
    u.w.send(&C2S::SearchGifs { query: "cat  dance".into(), next: None }).await.unwrap();
    wait(&mut u, 20, |m| matches!(m, S2C::Gifs { .. }).then_some(())).await.expect("no cached results");
    assert_eq!(hits.load(Ordering::SeqCst), before, "the second search should come from the cache");
    u.w.send(&C2S::SearchGifs { query: "cat  dance".into(), next: next.clone() }).await.unwrap();
    let more = wait(&mut u, 20, |m| if let S2C::Gifs { items, next, .. } = m { Some((items, next)) } else { None }).await.expect("no next page");
    assert_eq!((more.0.len(), more.1), (1, None));
    println!("3. the same search again came from the cache; the next page has 1 more");

    u.w.send(&C2S::SendGif { target: TextTarget::Channel(u.channel), id: "g99999".into() }).await.unwrap();
    println!("4. posting a GIF the server never handed out: \"{}\"", error(&mut u).await);
    let mut other = login(&addr, "Someone else").await;
    other.w.send(&C2S::SendGif { target: TextTarget::Channel(other.channel), id: items[0].id.clone() }).await.unwrap();
    println!("4. posting a GIF from someone else's search: \"{}\"", error(&mut other).await);
    u.w.send(&C2S::SendGif { target: TextTarget::Channel(u.channel), id: items[1].id.clone() }).await.unwrap();
    println!("4. a result pointing at an IP address: \"{}\"", error(&mut u).await);
    u.w.send(&C2S::SendGif { target: TextTarget::Channel(u.channel), id: items[0].id.clone() }).await.unwrap();
    println!("4. a GIF that cannot be downloaded: \"{}\"", error(&mut u).await);
    u.w.send(&C2S::SendGif { target: TextTarget::Server, id: items[0].id.clone() }).await.unwrap();
    println!("4. posting to the Server tab: \"{}\"", error(&mut u).await);

    // Searching faster than the limit.
    for i in 0..12 {
        u.w.send(&C2S::SearchGifs { query: format!("x{i}"), next: None }).await.unwrap();
    }
    let slow = wait(&mut u, 20, |m| if let S2C::Error(e) = m { e.contains("too fast").then_some(e) } else { None }).await.expect("no rate limit");
    println!("5. searching too fast: \"{slow}\"");

    // The server's own library (1.28.0+): GIFs dropped into the data folder's gifs folder.
    let dir = std::path::PathBuf::from(std::env::args().nth(2).expect("server data dir"));
    let gif = |frames: usize| {
        let mut buf = vec![];
        buf.extend_from_slice(b"GIF89a");
        buf.extend_from_slice(&[4, 0, 3, 0, 0, 0, 0]);
        // Image descriptor + 1 tiny LZW block per frame, then the trailer.
        for _ in 0..frames {
            buf.extend_from_slice(&[0x2C, 0, 0, 0, 0, 4, 0, 3, 0, 0x80, 0, 0, 0, 255, 255, 255, 2, 2, 0x44, 0x01, 0]);
        }
        buf.push(0x3B);
        buf
    };
    std::fs::create_dir_all(dir.join("gifs/Reactions")).unwrap();
    std::fs::write(dir.join("gifs/Reactions/thumbs-up.gif"), gif(2)).unwrap();
    std::fs::write(dir.join("gifs/Reactions/happy_dance.gif"), gif(2)).unwrap();
    std::fs::write(dir.join("gifs/wave.gif"), gif(1)).unwrap();
    std::fs::write(dir.join("gifs/not-a-gif.gif"), b"hello").unwrap();
    let mut lib = login(&addr, "Library fan").await;
    lib.w.send(&C2S::ServerGifs { query: String::new(), next: None }).await.unwrap();
    let (items, cats, total) = wait(&mut lib, 20, |m| if let S2C::ServerGifs { items, categories, total, .. } = m { Some((items, categories, total)) } else { None }).await.expect("no library");
    assert_eq!((items.len(), total), (3, 3), "{items:?}");
    assert_eq!(cats, vec!["Reactions".to_string()]);
    println!("6. the server's library: {total} GIFs ({}), categories {cats:?}", items.iter().map(|i| i.title.as_str()).collect::<Vec<_>>().join(", "));
    lib.w.send(&C2S::ServerGifs { query: "Dance REACTIONS".into(), next: None }).await.unwrap();
    let hits = wait(&mut lib, 20, |m| if let S2C::ServerGifs { items, .. } = m { Some(items) } else { None }).await.expect("no search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title, "happy dance");
    lib.w.send(&C2S::SendGif { target: TextTarget::Channel(lib.channel), id: hits[0].id.clone() }).await.unwrap();
    let posted = wait(&mut lib, 20, |m| if let S2C::Text(t) = m { ImageRef::parse(&t.text) } else { None }).await.expect("the library GIF was not posted");
    assert_eq!(posted.name, "happy dance.gif");
    println!("7. searching \"dance reactions\" found \"{}\", posted as {}", hits[0].title, posted.name);
    lib.w.send(&C2S::RemoveServerGif(hits[0].id.clone())).await.unwrap();
    println!("8. removing a GIF without \"Manage server GIFs\": \"{}\"", error(&mut lib).await);
    assert!(dir.join("gifs/Reactions/happy_dance.gif").exists());
    let key = std::fs::read_to_string(dir.join("privilege_key.txt"))
        .ok()
        .and_then(|t| t.lines().map(str::trim).find(|l| l.len() >= 20 && !l.contains(' ')).map(String::from))
        .expect("privilege key");
    lib.w.send(&C2S::UsePrivilegeKey(key)).await.unwrap();
    wait(&mut lib, 5, |m| matches!(m, S2C::Notice(_)).then_some(())).await;
    lib.w.send(&C2S::RemoveServerGif(hits[0].id.clone())).await.unwrap();
    let n = wait(&mut lib, 20, |m| if let S2C::Notice(n) = m { Some(n) } else { None }).await.expect("not removed");
    assert!(!dir.join("gifs/Reactions/happy_dance.gif").exists());
    println!("8. an admin removed it: \"{n}\"");
    println!("GIF TEST PASSED");
}
