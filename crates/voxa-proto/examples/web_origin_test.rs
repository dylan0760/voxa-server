//! The web page's address pin (Voxa Server 1.26.0+) against a running server with the web
//! page on and `web_allowed_origins` empty:
//!   cargo run -p voxa-proto --example web_origin_test -- 127.0.0.1:9987 <data dir> 127.0.0.1:9980
//! A look-alike site's proof (made for its own address, sent with its own Host header)
//! must be refused once an administrator has signed in from the real address.

use std::time::Duration;

use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use voxa_proto::identity::{self, KeyPair};
use voxa_proto::transport;
use voxa_proto::*;

/// One HTTP/1.1 request; returns (status, body).
async fn http(web: &str, method: &str, path: &str, host: &str, body: &str) -> (u16, String) {
    let mut s = TcpStream::connect(web).await.unwrap();
    let req = format!(
        "{method} {path} HTTP/1.1\r\nHost: {host}\r\nX-Voxa: 1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    s.write_all(req.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text.split(' ').nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

fn field(json: &str, key: &str) -> String {
    let pat = format!("\"{key}\":\"");
    let start = json.find(&pat).map(|i| i + pat.len()).unwrap_or_else(|| panic!("no {key} in {json}"));
    json[start..].split('"').next().unwrap().to_string()
}

/// Signs `kp` in to the web page at `web`, claiming the page is `origin` (Host header too).
async fn web_login(web: &str, kp: &KeyPair, origin: &str) -> (u16, String) {
    let (_, ch) = http(web, "GET", "/api/challenge", origin.split("://").nth(1).unwrap(), "").await;
    let b64 = base64::engine::general_purpose::STANDARD;
    let server_key = b64.decode(field(&ch, "server_key")).unwrap();
    let nonce = b64.decode(field(&ch, "nonce")).unwrap();
    let shared = identity::agree(&kp.private, &server_key).unwrap();
    let proof = identity::web_login_proof(&shared, &server_key, &nonce, &kp.public, origin);
    let body = format!(
        r#"{{"id":"{}","public":"{}","proof":"{}","nickname":"Web","origin":"{}"}}"#,
        field(&ch, "id"),
        b64.encode(&kp.public),
        b64.encode(proof),
        origin
    );
    http(web, "POST", "/api/login", origin.split("://").nth(1).unwrap(), &body).await
}

#[tokio::main]
async fn main() {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:9987".into());
    let dir = std::env::args().nth(2).unwrap_or_default();
    let web = std::env::args().nth(3).unwrap_or("127.0.0.1:9980".into());
    let real = format!("http://{web}");
    let evil = "http://voxa-admin.evil.example";
    let pin = std::path::Path::new(&dir).join("web_origin.txt");

    // An administrator identity (privilege key over the normal connection).
    let admin = KeyPair::generate();
    let s = TcpStream::connect(&addr).await.unwrap();
    let (r, w) = s.into_split();
    let sec = transport::connect(r, w, &admin.private).await.unwrap();
    let (mut r, mut w) = (sec.reader, sec.writer);
    w.send(&C2S::Hello { protocol: PROTOCOL_VERSION, version: APP_VERSION.into(), platform: "test".into(), nickname: "Admin".into(), server_password: None, default_channel: None, default_channel_password: None }).await.unwrap();
    let _welcome: S2C = r.recv().await.unwrap();
    let key = std::fs::read_to_string(std::path::Path::new(&dir).join("privilege_key.txt"))
        .ok()
        .and_then(|t| t.lines().map(str::trim).find(|l| l.len() >= 20 && !l.contains(' ')).map(String::from))
        .expect("privilege key");
    w.send(&C2S::UsePrivilegeKey(key)).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let S2C::Notice(_) = r.recv::<S2C>().await.unwrap() {
                break;
            }
        }
    })
    .await
    .expect("privilege key accepted");

    // 1. Before any administrator has used the web page, sign-ins work and fix nothing.
    let guest = KeyPair::generate();
    let (st, body) = web_login(&web, &guest, evil).await;
    assert_eq!(st, 200, "{body}");
    assert!(!pin.exists(), "a non-administrator must not fix the address");
    println!("1. before the first administrator sign-in: any address works, nothing remembered");

    // 2. The administrator signs in from the real address, which is remembered.
    let (st, body) = web_login(&web, &admin, &real).await;
    assert_eq!(st, 200, "{body}");
    assert_eq!(std::fs::read_to_string(&pin).unwrap().trim(), real);
    println!("2. administrator signed in from {real}: address remembered");

    // 3. A look-alike site's proof (its own address and Host header) is refused now,
    //    also for the administrator's identity.
    for kp in [&guest, &admin] {
        let (st, body) = web_login(&web, kp, evil).await;
        assert_eq!(st, 403, "{body}");
        assert!(body.contains("not allowed"), "{body}");
    }
    println!("3. look-alike address refused");

    // 4. The real address keeps working for everyone.
    let (st, body) = web_login(&web, &guest, &real).await;
    assert_eq!(st, 200, "{body}");
    println!("4. real address still works\nweb_origin_test passed");
}
