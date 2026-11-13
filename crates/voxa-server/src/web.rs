//! The web admin page: the server's channels, people and settings in a browser, looking
//! like the Voxa client.
//!
//! * Plain HTTP on `web_bind:web_port`; put a reverse proxy (e.g. Nginx Proxy Manager) in
//!   front of it for HTTPS. Behind a proxy on the local network the visitor's address is
//!   taken from `X-Real-IP` / `X-Forwarded-For`.
//! * Sign-in proves ownership of a Voxa identity without sending it: the page computes an
//!   X25519 key agreement with a one-time server key and returns an HMAC over the
//!   challenge. The identity's private key never leaves the browser.
//! * A signed-in page is an ordinary session of that identity (shown with platform "Web",
//!   no voice): every action goes through the same handler and permission checks as the
//!   desktop client. Updates are streamed to the page with Server-Sent Events, actions
//!   are POSTed as JSON.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine;
use parking_lot::Mutex;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, Notify};
use tokio::time::timeout;
use tracing::{debug, info, warn};
use voxa_proto::identity::uid_from_public;
use voxa_proto::*;

use crate::state::{Out, Outbox, Server, MIN_CLIENT_VERSION, OUTBOX_SIZE};

const INDEX: &str = include_str!("web/index.html");
const STYLE: &str = include_str!("web/style.css");
const CRYPTO_JS: &str = include_str!("web/crypto.js");
const APP_JS: &str = include_str!("web/app.js");
const ICON: &str = include_str!("web/icon.svg");

const COOKIE: &str = "voxa_web";
/// A sign-in lasts this long; then the page asks for the identity again.
const LOGIN_LIFETIME: Duration = Duration::from_secs(12 * 3600);
const CHALLENGE_LIFETIME: Duration = Duration::from_secs(120);
const MAX_CHALLENGES: usize = 2000;
const MAX_HEAD: usize = 16 * 1024;
const MAX_BODY: usize = 256 * 1024;
const MAX_CONNECTIONS: usize = 512;
/// Failed sign-ins per address before it has to wait (`FAIL_WINDOW`).
const MAX_FAILURES: u32 = 10;
const FAIL_WINDOW: Duration = Duration::from_secs(600);

struct Challenge {
    private: [u8; 32],
    public: Vec<u8>,
    nonce: [u8; 32],
    created: Instant,
}

struct Login {
    uid: String,
    nickname: String,
    server_password: Option<String>,
    created: Instant,
    /// The live session of the open page, if any: (client id, its close signal).
    client: Option<(ClientId, Arc<Notify>)>,
}

#[derive(Default)]
struct Web {
    challenges: Mutex<HashMap<String, Challenge>>,
    logins: Mutex<HashMap<String, Login>>,
    failures: Mutex<HashMap<IpAddr, (u32, Instant)>>,
}

struct Request {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Request {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    fn cookie(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .filter(|(k, _)| k == "cookie")
            .flat_map(|(_, v)| v.split(';'))
            .filter_map(|p| p.trim().split_once('='))
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.to_string())
    }

    /// Requests from the page itself carry this header; other sites cannot add it
    /// without a CORS preflight, which is never answered.
    fn sent_by_page(&self) -> bool {
        self.header("x-voxa") == Some("1")
    }
}

pub async fn run(server: Arc<Server>, listener: TcpListener) {
    let web = Arc::new(Web::default());
    let slots = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(x) => x,
            Err(_) => {
                tokio::time::sleep(Duration::from_millis(100)).await;
                continue;
            }
        };
        let Ok(permit) = slots.clone().try_acquire_owned() else { continue };
        let (server, web) = (server.clone(), web.clone());
        tokio::spawn(async move {
            let _permit = permit;
            if let Err(e) = connection(server, web, stream, peer).await {
                debug!("web: {peer}: {e}");
            }
        });
    }
}

async fn connection(server: Arc<Server>, web: Arc<Web>, mut s: TcpStream, peer: SocketAddr) -> std::io::Result<()> {
    let _ = s.set_nodelay(true);
    let Ok(Some(req)) = timeout(Duration::from_secs(15), read_request(&mut s)).await else {
        return respond(&mut s, "400 Bad Request", "text/plain", &[], b"Bad request").await;
    };
    let ip = client_ip(peer, &req);
    let secure = req.header("x-forwarded-proto").map(|p| p.eq_ignore_ascii_case("https")).unwrap_or(false);
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") | ("GET", "/index.html") => static_file(&mut s, "text/html; charset=utf-8", INDEX).await,
        ("GET", "/style.css") => static_file(&mut s, "text/css; charset=utf-8", STYLE).await,
        ("GET", "/crypto.js") => static_file(&mut s, "text/javascript; charset=utf-8", CRYPTO_JS).await,
        ("GET", "/app.js") => static_file(&mut s, "text/javascript; charset=utf-8", APP_JS).await,
        ("GET", "/icon.svg") | ("GET", "/favicon.ico") => static_file(&mut s, "image/svg+xml", ICON).await,
        ("GET", "/api/challenge") => challenge(&server, &web, &mut s).await,
        ("GET", "/api/session") => match signed_in(&web, &req) {
            Some((_, uid, nickname, _)) => json(&mut s, "200 OK", &[], &serde_json::json!({ "signed_in": true, "uid": uid, "nickname": nickname })).await,
            None => json(&mut s, "200 OK", &[], &serde_json::json!({ "signed_in": false })).await,
        },
        ("POST", "/api/login") if req.sent_by_page() => login(&server, &web, &mut s, &req, ip, secure).await,
        ("POST", "/api/logout") if req.sent_by_page() => logout(&server, &web, &mut s, &req, secure).await,
        ("GET", "/api/events") => events(&server, &web, &mut s, &req, ip).await,
        ("POST", "/api/send") if req.sent_by_page() => send(&server, &web, &mut s, &req).await,
        ("GET", p) if p.starts_with("/api/image/") => image(&server, &web, &mut s, &req, &p["/api/image/".len()..]).await,
        _ => respond(&mut s, "404 Not Found", "text/plain", &[], b"Not found").await,
    }
}

// ------------------------------------------------------------------ sign-in

async fn challenge(server: &Server, web: &Web, s: &mut TcpStream) -> std::io::Result<()> {
    let private: [u8; 32] = crate::secrets::random_bytes();
    let Some(public) = x25519_public(&private) else {
        return json(s, "500 Internal Server Error", &[], &serde_json::json!({ "error": "No key agreement available" })).await;
    };
    let nonce: [u8; 32] = crate::secrets::random_bytes();
    let id = hex(&crate::secrets::random_bytes::<16>());
    let stored = {
        let mut ch = web.challenges.lock();
        ch.retain(|_, c| c.created.elapsed() < CHALLENGE_LIFETIME);
        let room = ch.len() < MAX_CHALLENGES;
        if room {
            ch.insert(id.clone(), Challenge { private, public: public.clone(), nonce, created: Instant::now() });
        }
        room
    };
    if !stored {
        return json(s, "503 Service Unavailable", &[], &serde_json::json!({ "error": "Too many sign-ins at once, try again in a minute" })).await;
    }
    let (name, has_password) = {
        let st = server.state.lock();
        (st.settings.name.clone(), st.settings.password_hash.is_some())
    };
    let b64 = base64::engine::general_purpose::STANDARD;
    let body = serde_json::json!({
        "id": id,
        "server_key": b64.encode(&public),
        "nonce": b64.encode(nonce),
        "name": name,
        "fingerprint": server.fingerprint,
        "has_password": has_password,
        "version": APP_VERSION,
    });
    json(s, "200 OK", &[], &body).await
}

#[derive(Deserialize)]
struct LoginBody {
    id: String,
    public: String,
    proof: String,
    nickname: String,
    #[serde(default)]
    password: Option<String>,
}

/// The proof the page computes: HMAC-SHA256(X25519(identity, server key), context).
fn expected_proof(shared: &[u8; 32], server_key: &[u8], nonce: &[u8], public: &[u8]) -> [u8; 32] {
    hmac_sha256(shared, &[b"voxa-web-login-v1", server_key, nonce, public])
}

async fn login(server: &Server, web: &Web, s: &mut TcpStream, req: &Request, ip: IpAddr, secure: bool) -> std::io::Result<()> {
    let refuse = |msg: &str| serde_json::json!({ "error": msg });
    let blocked = {
        let mut f = web.failures.lock();
        f.retain(|_, (_, at)| at.elapsed() < FAIL_WINDOW);
        f.get(&ip).map(|(n, _)| *n >= MAX_FAILURES).unwrap_or(false)
    };
    if blocked {
        return json(s, "429 Too Many Requests", &[], &refuse("Too many failed sign-ins. Wait a few minutes.")).await;
    }
    let Ok(body) = serde_json::from_slice::<LoginBody>(&req.body) else {
        return json(s, "400 Bad Request", &[], &refuse("Bad request")).await;
    };
    let fail = |web: &Web| {
        let mut f = web.failures.lock();
        let e = f.entry(ip).or_insert((0, Instant::now()));
        e.0 += 1;
        e.1 = Instant::now();
    };
    let taken = web.challenges.lock().remove(&body.id);
    let Some(ch) = taken.filter(|c| c.created.elapsed() < CHALLENGE_LIFETIME) else {
        return json(s, "400 Bad Request", &[], &refuse("The sign-in expired, try again")).await;
    };
    let b64 = base64::engine::general_purpose::STANDARD;
    let public = b64.decode(body.public.trim()).ok().filter(|p| p.len() == 32);
    let proof = b64.decode(body.proof.trim()).ok().filter(|p| p.len() == 32);
    let ok = match (&public, &proof) {
        (Some(public), Some(proof)) => match x25519(&ch.private, public) {
            Some(shared) => ct_eq(&expected_proof(&shared, &ch.public, &ch.nonce, public), proof),
            None => false,
        },
        _ => false,
    };
    let Some(public) = public.filter(|_| ok) else {
        fail(web);
        info!("web: failed sign-in from {ip}");
        return json(s, "403 Forbidden", &[], &refuse("That identity key could not be verified")).await;
    };
    let uid = uid_from_public(&public);
    let password = body.password.filter(|p| !p.is_empty());
    let has_password = server.state.lock().settings.password_hash.is_some();
    if has_password {
        let checked = match &password {
            Some(p) => server.check_login_passwords(ip, Some(p), None, None).await,
            None => Err("This server has a password".to_string()),
        };
        if let Err(e) = checked {
            fail(web);
            return json(s, "403 Forbidden", &[], &refuse(&e)).await;
        }
    }
    let nickname: String = body.nickname.trim().chars().take(30).collect();
    let token = hex(&crate::secrets::random_bytes::<32>());
    {
        let mut logins = web.logins.lock();
        logins.retain(|_, l| l.created.elapsed() < LOGIN_LIFETIME);
        logins.insert(
            token.clone(),
            Login { uid: uid.clone(), nickname: if nickname.is_empty() { "Web admin".into() } else { nickname }, server_password: password, created: Instant::now(), client: None },
        );
    }
    info!("web: {uid} signed in from {ip}");
    let cookie = format!(
        "{COOKIE}={token}; Path=/; HttpOnly; SameSite=Strict; Max-Age={}{}",
        LOGIN_LIFETIME.as_secs(),
        if secure { "; Secure" } else { "" }
    );
    json(s, "200 OK", &[("Set-Cookie", cookie)], &serde_json::json!({ "uid": uid })).await
}

async fn logout(server: &Server, web: &Web, s: &mut TcpStream, req: &Request, secure: bool) -> std::io::Result<()> {
    if let Some(l) = req.cookie(COOKIE).and_then(|t| web.logins.lock().remove(&t)) {
        if let Some((id, close)) = l.client {
            close.notify_one();
            end_session(server, id, &close, "Signed out");
        }
    }
    let cookie = format!("{COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{}", if secure { "; Secure" } else { "" });
    json(s, "200 OK", &[("Set-Cookie", cookie)], &serde_json::json!({})).await
}

/// The signed-in login of a request: (token, uid, nickname, server password).
fn signed_in(web: &Web, req: &Request) -> Option<(String, String, String, Option<String>)> {
    let token = req.cookie(COOKIE)?;
    let logins = web.logins.lock();
    let l = logins.get(&token).filter(|l| l.created.elapsed() < LOGIN_LIFETIME)?;
    Some((token.clone(), l.uid.clone(), l.nickname.clone(), l.server_password.clone()))
}

/// Removes a page's session, unless its client id was already given to someone else.
fn end_session(server: &Server, id: ClientId, close: &Arc<Notify>, reason: &str) {
    let ours = server.state.lock().sessions.get(&id).map(|s| Arc::ptr_eq(&s.close, close)).unwrap_or(false);
    if ours {
        server.remove_session(id, LeaveReason::Disconnected(reason.into()));
    }
}

// ------------------------------------------------------------------ live session

/// The page's live connection: joins the server as this identity and streams every
/// message to the page until it is closed.
async fn events(server: &Server, web: &Web, s: &mut TcpStream, req: &Request, ip: IpAddr) -> std::io::Result<()> {
    let Some((token, uid, nickname, password)) = signed_in(web, req) else {
        return json(s, "401 Unauthorized", &[], &serde_json::json!({ "error": "Not signed in" })).await;
    };
    s.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-store\r\nX-Accel-Buffering: no\r\nX-Content-Type-Options: nosniff\r\nConnection: keep-alive\r\n\r\n",
    )
    .await?;
    // One live page per sign-in: an older tab is closed.
    let old = web.logins.lock().get_mut(&token).and_then(|l| l.client.take());
    if let Some((id, close)) = old {
        close.notify_one();
        end_session(server, id, &close, "Opened in another tab");
    }
    if let Some(p) = &password {
        let _ = server.check_login_passwords(ip, Some(p), None, None).await;
    }
    let (tx, mut rx) = mpsc::channel::<Out>(OUTBOX_SIZE);
    let close = Arc::new(Notify::new());
    let hello = C2S::Hello {
        protocol: PROTOCOL_VERSION,
        // Web pages understand every message of this server version.
        version: MIN_CLIENT_VERSION.into(),
        platform: "Web".into(),
        nickname,
        server_password: password,
        default_channel: None,
        default_channel_password: None,
    };
    let id = match server.login(uid, ip, hello, Outbox { tx, close: close.clone() }, close.clone(), true) {
        Ok(id) => id,
        Err(e) => {
            let _ = s.write_all(event(&S2C::Refused(e)).as_bytes()).await;
            return Ok(());
        }
    };
    match web.logins.lock().get_mut(&token) {
        Some(l) => l.client = Some((id, close.clone())),
        None => close.notify_one(),
    }
    let mut ping = tokio::time::interval(Duration::from_secs(20));
    let reason = loop {
        tokio::select! {
            o = rx.recv() => match o {
                Some(Out::Msg(m)) => {
                    if s.write_all(event(&m).as_bytes()).await.is_err() {
                        break "Closed the web page";
                    }
                }
                Some(Out::Close) | None => break "Disconnected",
            },
            _ = close.notified() => break "Disconnected",
            _ = ping.tick() => {
                if s.write_all(b": ping\n\n").await.is_err() {
                    break "Closed the web page";
                }
            }
        }
    };
    // Whatever was queued last (e.g. why the session was kicked).
    while let Ok(Out::Msg(m)) = rx.try_recv() {
        let _ = s.write_all(event(&m).as_bytes()).await;
    }
    if let Some(l) = web.logins.lock().get_mut(&token) {
        if l.client.as_ref().map(|(_, c)| Arc::ptr_eq(c, &close)).unwrap_or(false) {
            l.client = None;
        }
    }
    end_session(server, id, &close, reason);
    Ok(())
}

/// One Server-Sent Event. Secrets meant for the voice and bulk connections are left out.
fn event(m: &S2C) -> String {
    let json = match m {
        S2C::Welcome { your_id, your_uid, server, channels, clients, groups, .. } => serde_json::json!({ "Welcome": {
            "your_id": your_id, "your_uid": your_uid, "server": server, "channels": channels, "clients": clients, "groups": groups,
        } })
        .to_string(),
        other => serde_json::to_string(other).unwrap_or_else(|_| "{}".into()),
    };
    format!("data: {json}\n\n")
}

async fn send(server: &Server, web: &Web, s: &mut TcpStream, req: &Request) -> std::io::Result<()> {
    let Some(id) = live_client(web, req) else {
        return json(s, "409 Conflict", &[], &serde_json::json!({ "error": "Not connected" })).await;
    };
    let msg = match parse_c2s(&req.body) {
        Ok(m) => m,
        Err(e) => return json(s, "400 Bad Request", &[], &serde_json::json!({ "error": format!("Bad message: {e}") })).await,
    };
    // The page has no voice, bulk connection or screen sharing.
    if matches!(msg, C2S::Hello { .. } | C2S::AttachBulk(_) | C2S::Disconnect(_) | C2S::StartStream(_) | C2S::StopStream | C2S::WatchStream { .. }) {
        return json(s, "400 Bad Request", &[], &serde_json::json!({ "error": "Not available on the web page" })).await;
    }
    if let Err(e) = server.prepare(id, &msg).await {
        server.state.lock().send(id, S2C::Error(e));
    } else {
        server.handle(id, msg);
    }
    respond(s, "204 No Content", "text/plain", &[], b"").await
}

/// A message from the page. `UpdateSelf` is read by hand: in JSON a missing field means
/// "unchanged" and an explicit `null` means "clear" (e.g. back from away), which serde
/// cannot tell apart for `Option<Option<_>>`.
fn parse_c2s(body: &[u8]) -> Result<C2S, serde_json::Error> {
    let v: serde_json::Value = serde_json::from_slice(body)?;
    if let Some(u) = v.get("UpdateSelf").and_then(|u| u.as_object()) {
        let text = |k: &str| u.get(k).and_then(|x| x.as_str()).map(String::from);
        let clearable = |k: &str| u.get(k).map(|x| x.as_str().map(String::from));
        let flag = |k: &str| u.get(k).and_then(|x| x.as_bool());
        return Ok(C2S::UpdateSelf(SelfUpdate {
            nickname: text("nickname"),
            description: text("description"),
            away: clearable("away"),
            input_muted: flag("input_muted"),
            output_muted: flag("output_muted"),
            talk_request: clearable("talk_request"),
            commander: flag("commander"),
        }));
    }
    serde_json::from_value(v)
}

fn live_client(web: &Web, req: &Request) -> Option<ClientId> {
    let token = req.cookie(COOKIE)?;
    let logins = web.logins.lock();
    logins.get(&token).filter(|l| l.created.elapsed() < LOGIN_LIFETIME)?.client.as_ref().map(|(id, _)| *id)
}

async fn image(server: &Server, web: &Web, s: &mut TcpStream, req: &Request, id: &str) -> std::io::Result<()> {
    let (Some(me), Ok(image)) = (live_client(web, req), id.parse::<u64>()) else {
        return respond(s, "404 Not Found", "text/plain", &[], b"Not found").await;
    };
    match server.chat_image(me, image) {
        Ok(data) => {
            let ctype = match image_kind(&data) {
                Some("png") => "image/png",
                Some("jpg") => "image/jpeg",
                Some("gif") => "image/gif",
                Some("webp") => "image/webp",
                Some("bmp") => "image/bmp",
                _ => return respond(s, "415 Unsupported Media Type", "text/plain", &[], b"Not an image").await,
            };
            respond(s, "200 OK", ctype, &[("Cache-Control", "private, max-age=3600".into())], &data).await
        }
        Err(e) => respond(s, "403 Forbidden", "text/plain; charset=utf-8", &[], e.as_bytes()).await,
    }
}

// ------------------------------------------------------------------ HTTP

async fn read_request(s: &mut TcpStream) -> Option<Request> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > MAX_HEAD {
            return None;
        }
        let n = s.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = std::str::from_utf8(&buf[..head_end]).ok()?;
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let method = first.next()?.to_string();
    let target = first.next()?;
    let path = target.split('?').next().unwrap_or("/").to_string();
    let headers: Vec<(String, String)> =
        lines.filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_ascii_lowercase(), v.trim().to_string())).collect();
    if headers.iter().any(|(k, _)| k == "transfer-encoding") {
        return None;
    }
    let len: usize = headers.iter().find(|(k, _)| k == "content-length").map(|(_, v)| v.parse().ok()).unwrap_or(Some(0))?;
    if len > MAX_BODY {
        return None;
    }
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = s.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(len);
    Some(Request { method, path, headers, body })
}

/// The visitor's address. Behind a reverse proxy on this PC or the local network the
/// proxy's `X-Real-IP` / `X-Forwarded-For` is used (bans and logs need the real address).
fn client_ip(peer: SocketAddr, req: &Request) -> IpAddr {
    let p = peer.ip();
    let local = match p {
        IpAddr::V4(a) => a.is_loopback() || a.is_private() || a.is_link_local(),
        IpAddr::V6(a) => a.is_loopback() || (a.segments()[0] & 0xfe00) == 0xfc00 || a.to_ipv4_mapped().map(|v| v.is_private() || v.is_loopback()).unwrap_or(false),
    };
    if !local {
        return p;
    }
    let forwarded = req.header("x-real-ip").or_else(|| req.header("x-forwarded-for").and_then(|v| v.split(',').next()));
    forwarded.and_then(|v| v.trim().parse().ok()).unwrap_or(p)
}

const SECURITY_HEADERS: &str = "X-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\n\
Content-Security-Policy: default-src 'self'; img-src 'self' data: blob:; style-src 'self'; script-src 'self'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'\r\n";

async fn respond(s: &mut TcpStream, status: &str, ctype: &str, extra: &[(&str, String)], body: &[u8]) -> std::io::Result<()> {
    let mut head = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n{SECURITY_HEADERS}", body.len());
    if !extra.iter().any(|(k, _)| k.eq_ignore_ascii_case("cache-control")) {
        head.push_str("Cache-Control: no-store\r\n");
    }
    for (k, v) in extra {
        head.push_str(&format!("{k}: {v}\r\n"));
    }
    head.push_str("\r\n");
    s.write_all(head.as_bytes()).await?;
    s.write_all(body).await?;
    s.flush().await
}

async fn static_file(s: &mut TcpStream, ctype: &str, body: &str) -> std::io::Result<()> {
    // Revalidated every time so an updated server never serves an old page.
    respond(s, "200 OK", ctype, &[("Cache-Control", "no-cache".into())], body.as_bytes()).await
}

async fn json(s: &mut TcpStream, status: &str, extra: &[(&str, String)], v: &serde_json::Value) -> std::io::Result<()> {
    respond(s, status, "application/json", extra, v.to_string().as_bytes()).await
}

// ------------------------------------------------------------------ crypto

fn dh() -> Option<Box<dyn snow::types::Dh>> {
    use snow::resolvers::{CryptoResolver, DefaultResolver};
    DefaultResolver.resolve_dh(&snow::params::DHChoice::Curve25519)
}

fn x25519_public(private: &[u8; 32]) -> Option<Vec<u8>> {
    let mut d = dh()?;
    d.set(private);
    Some(d.pubkey().to_vec())
}

/// X25519 key agreement; None for keys that give an all-zero (contributory-less) result.
fn x25519(private: &[u8; 32], public: &[u8]) -> Option<[u8; 32]> {
    let mut d = dh()?;
    d.set(private);
    let mut out = [0u8; 32];
    d.dh(public, &mut out).ok()?;
    (out != [0u8; 32]).then_some(out)
}

fn hmac_sha256(key: &[u8], parts: &[&[u8]]) -> [u8; 32] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..32].copy_from_slice(&Sha256::digest(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Sha256::new();
    inner.update(k.map(|b| b ^ 0x36));
    for p in parts {
        inner.update(p);
    }
    let mut outer = Sha256::new();
    outer.update(k.map(|b| b ^ 0x5c));
    outer.update(inner.finalize());
    outer.finalize().into()
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Starts the web admin page if it is enabled in server.toml.
pub async fn start(server: Arc<Server>) {
    let cfg = &server.cfg;
    if !cfg.web_enabled || cfg.web_port == 0 {
        info!("web admin page is off (web_enabled = false in server.toml)");
        return;
    }
    let bind = format!("{}:{}", cfg.web_bind, cfg.web_port);
    match TcpListener::bind(&bind).await {
        Ok(l) => {
            info!("web admin page on http://{bind}/ (put an HTTPS reverse proxy in front of it for access over the internet)");
            run(server, l).await;
        }
        Err(e) => warn!("web admin page could not listen on {bind}: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HMAC-SHA256 test vector (RFC 4231, case 2).
    #[test]
    fn hmac_matches_rfc4231() {
        let mac = hmac_sha256(b"Jefe", &[b"what do ya want ", b"for nothing?"]);
        assert_eq!(hex(&mac), "5bdcc146bf60754e6a042426089575c75a003f089d2739839dec58b964ec3843");
    }

    /// Both sides of the sign-in agree, and a wrong identity gives a different proof.
    #[test]
    fn key_agreement_and_proof() {
        let server: [u8; 32] = crate::secrets::random_bytes();
        let server_pub = x25519_public(&server).unwrap();
        let id = voxa_proto::identity::KeyPair::generate();
        let id_priv: [u8; 32] = id.private.clone().try_into().unwrap();
        let a = x25519(&server, &id.public).unwrap();
        let b = x25519(&id_priv, &server_pub).unwrap();
        assert_eq!(a, b);
        let nonce = [7u8; 32];
        let other = voxa_proto::identity::KeyPair::generate();
        let other_priv: [u8; 32] = other.private.clone().try_into().unwrap();
        let wrong = x25519(&other_priv, &server_pub).unwrap();
        assert_ne!(expected_proof(&a, &server_pub, &nonce, &id.public), expected_proof(&wrong, &server_pub, &nonce, &id.public));
        // A low-order public key is refused.
        assert!(x25519(&server, &[0u8; 32]).is_none());
    }

    /// RFC 7748 section 6.1 test vector (the JavaScript side is checked against the same).
    #[test]
    fn x25519_rfc7748() {
        let unhex = |s: &str| (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect::<Vec<u8>>();
        let alice: [u8; 32] = unhex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a").try_into().unwrap();
        let bob_pub = unhex("de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f");
        assert_eq!(hex(&x25519_public(&alice).unwrap()), "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a");
        assert_eq!(hex(&x25519(&alice, &bob_pub).unwrap()), "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742");
    }
}
