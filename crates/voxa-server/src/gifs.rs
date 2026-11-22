//! GIF search (1.27.0+): the server asks the GIF service set in `server.toml` (KLIPY or
//! GIPHY) on behalf of its people, so their searches and addresses never reach that
//! service. Results get short ids; previews and GIFs are only fetched for ids the server
//! handed out itself (a client can never make the server fetch an address of its choice).
//!
//! Everything here blocks (HTTP): call it from a blocking thread.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use voxa_proto::{GifInfo, MAX_CHAT_IMAGE, MAX_GIF_QUERY};

use crate::config::Config;

/// Results per page.
const PAGE: usize = 24;
/// Search answers are reused this long (also spares the service's request limit).
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
const MAX_CACHED_SEARCHES: usize = 500;
/// Result ids the server remembers (previews and sends must name one of them).
const MAX_ITEMS: usize = 5_000;
/// Previews kept in memory, and the largest preview accepted.
const MAX_PREVIEW_BYTES: usize = 2 * 1024 * 1024;
const PREVIEW_CACHE_BYTES: usize = 64 * 1024 * 1024;
/// Calls to the GIF service per minute for the whole server (free keys allow few).
const SERVICE_CALLS_PER_MIN: usize = 60;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Provider {
    /// KLIPY: Tenor's v2 API (Tenor itself closed its API in June 2026).
    Klipy,
    Giphy,
}

impl Provider {
    pub fn from_config(cfg: &Config) -> Option<Provider> {
        if cfg.gif_api_key.trim().is_empty() {
            return None;
        }
        match cfg.gif_provider.trim().to_ascii_lowercase().as_str() {
            "klipy" | "" => Some(Provider::Klipy),
            "giphy" => Some(Provider::Giphy),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Provider::Klipy => "KLIPY",
            Provider::Giphy => "GIPHY",
        }
    }
}

/// A result as the server keeps it: where its files are.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub info: GifInfo,
    pub gif_url: String,
    pub preview_url: String,
}

struct Cached {
    at: Instant,
    items: Vec<GifInfo>,
    next: Option<String>,
}

pub struct Gifs {
    provider: Option<Provider>,
    key: String,
    filter: String,
    /// Base address of the API (tests point it at a local server).
    base: String,
    searches: Mutex<HashMap<(String, String), Cached>>,
    items: Mutex<(HashMap<String, Item>, VecDeque<String>)>,
    previews: Mutex<(HashMap<String, Arc<Vec<u8>>>, VecDeque<String>, usize)>,
    calls: Mutex<VecDeque<Instant>>,
    next_id: Mutex<u64>,
}

impl Gifs {
    pub fn new(cfg: &Config) -> Gifs {
        let provider = Provider::from_config(cfg);
        let base = if cfg.gif_api_base.trim().is_empty() {
            match provider {
                Some(Provider::Giphy) => "https://api.giphy.com".to_string(),
                _ => "https://api.klipy.com".to_string(),
            }
        } else {
            cfg.gif_api_base.trim().trim_end_matches('/').to_string()
        };
        Gifs {
            provider,
            key: cfg.gif_api_key.trim().to_string(),
            filter: cfg.gif_content_filter.trim().to_ascii_lowercase(),
            base,
            searches: Mutex::new(HashMap::new()),
            items: Mutex::new((HashMap::new(), VecDeque::new())),
            previews: Mutex::new((HashMap::new(), VecDeque::new(), 0)),
            calls: Mutex::new(VecDeque::new()),
            next_id: Mutex::new(1),
        }
    }

    pub fn provider(&self) -> Option<Provider> {
        self.provider
    }

    /// One call to the GIF service, if the server-wide limit allows it.
    fn spend_call(&self) -> Result<(), String> {
        let mut c = self.calls.lock();
        while c.front().is_some_and(|t| t.elapsed() > Duration::from_secs(60)) {
            c.pop_front();
        }
        if c.len() >= SERVICE_CALLS_PER_MIN {
            return Err("GIF search is busy on this server: try again in a minute".into());
        }
        c.push_back(Instant::now());
        Ok(())
    }

    /// Trending (empty query) or search results, a page at a time.
    pub fn search(&self, query: &str, next: Option<&str>) -> Result<(Vec<GifInfo>, Option<String>), String> {
        let provider = self.provider.ok_or("This server has no GIF search")?;
        let query: String = query.chars().filter(|c| !c.is_control()).take(MAX_GIF_QUERY).collect::<String>().trim().to_lowercase();
        let next = next.unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric() || "._-:".contains(*c)).take(100).collect::<String>();
        let key = (query.clone(), next.clone());
        {
            let mut s = self.searches.lock();
            if let Some(c) = s.get(&key).filter(|c| c.at.elapsed() < CACHE_TTL) {
                return Ok((c.items.clone(), c.next.clone()));
            }
            s.retain(|_, c| c.at.elapsed() < CACHE_TTL);
        }
        self.spend_call()?;
        let url = self.search_url(provider, &query, &next);
        let body = get(&url, 4 * 1024 * 1024).map_err(|e| {
            tracing::warn!("GIF search failed: {e}");
            "The GIF service did not answer: try again later".to_string()
        })?;
        let text = String::from_utf8_lossy(&body);
        let (found, next_page) = match provider {
            Provider::Klipy => parse_tenor(&text),
            Provider::Giphy => parse_giphy(&text),
        }?;
        let mut infos = Vec::new();
        {
            let mut items = self.items.lock();
            let mut id_ctr = self.next_id.lock();
            for mut it in found {
                let id = format!("g{}", *id_ctr);
                *id_ctr += 1;
                it.info.id = id.clone();
                infos.push(it.info.clone());
                items.0.insert(id.clone(), it);
                items.1.push_back(id);
                while items.1.len() > MAX_ITEMS {
                    if let Some(old) = items.1.pop_front() {
                        items.0.remove(&old);
                    }
                }
            }
        }
        let mut s = self.searches.lock();
        if s.len() >= MAX_CACHED_SEARCHES {
            s.clear();
        }
        s.insert(key, Cached { at: Instant::now(), items: infos.clone(), next: next_page.clone() });
        Ok((infos, next_page))
    }

    fn search_url(&self, p: Provider, query: &str, next: &str) -> String {
        let q = enc(query);
        let key = enc(&self.key);
        match p {
            Provider::Klipy => {
                let filter = if ["off", "low", "medium", "high"].contains(&self.filter.as_str()) { self.filter.as_str() } else { "medium" };
                let path = if query.is_empty() { "featured" } else { "search" };
                let pos = if next.is_empty() { String::new() } else { format!("&pos={}", enc(next)) };
                format!("{}/v2/{path}?key={key}&client_key=voxa&q={q}&limit={PAGE}&media_filter=gif,tinygif&contentfilter={filter}{pos}", self.base)
            }
            Provider::Giphy => {
                let rating = if ["g", "pg", "pg-13", "r"].contains(&self.filter.as_str()) { self.filter.as_str() } else { "pg-13" };
                let offset: u32 = next.parse().unwrap_or(0).min(4_999);
                if query.is_empty() {
                    format!("{}/v1/gifs/trending?api_key={key}&limit={PAGE}&offset={offset}&rating={rating}", self.base)
                } else {
                    format!("{}/v1/gifs/search?api_key={key}&q={q}&limit={PAGE}&offset={offset}&rating={rating}&lang=en", self.base)
                }
            }
        }
    }

    fn item(&self, id: &str) -> Result<Item, String> {
        self.items.lock().0.get(id).cloned().ok_or_else(|| "That GIF is no longer available: search again".to_string())
    }

    /// The small animated preview of a result.
    pub fn preview(&self, id: &str) -> Result<Arc<Vec<u8>>, String> {
        if let Some(p) = self.previews.lock().0.get(id) {
            return Ok(p.clone());
        }
        let it = self.item(id)?;
        let data = Arc::new(get_gif(&it.preview_url, MAX_PREVIEW_BYTES)?);
        let mut p = self.previews.lock();
        p.2 += data.len();
        p.0.insert(id.to_string(), data.clone());
        p.1.push_back(id.to_string());
        while p.2 > PREVIEW_CACHE_BYTES {
            let Some(old) = p.1.pop_front() else { break };
            if let Some(d) = p.0.remove(&old) {
                p.2 -= d.len();
            }
        }
        Ok(data)
    }

    /// The full GIF of a result, for posting: (bytes, file name, width, height).
    pub fn full(&self, id: &str) -> Result<(Vec<u8>, String, u32, u32), String> {
        let it = self.item(id)?;
        let data = get_gif(&it.gif_url, MAX_CHAT_IMAGE)?;
        let mut name: String = it.info.title.chars().filter(|c| c.is_alphanumeric() || " -_".contains(*c)).take(60).collect::<String>().trim().to_string();
        if name.is_empty() {
            name = "GIF".into();
        }
        Ok((data, format!("{name}.gif"), it.info.width, it.info.height))
    }
}

fn enc(s: &str) -> String {
    s.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

/// Only https addresses on a named host (no IP addresses, no localhost): the GIF
/// service's files. (The base address of the API itself may be http only in tests.)
pub fn safe_media_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host);
    let name = host.split(':').next().unwrap_or_default().to_ascii_lowercase();
    !name.is_empty()
        && name.contains('.')
        && !name.ends_with(".localhost")
        && name != "localhost"
        && !name.starts_with('[')
        && name.parse::<std::net::Ipv4Addr>().is_err()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

fn agent() -> ureq::Agent {
    let cfg = ureq::Agent::config_builder()
        .tls_config(voxa_proto::update::tls_config(true))
        .timeout_global(Some(Duration::from_secs(15)))
        .max_redirects(3)
        .user_agent(format!("Voxa-Server/{}", env!("CARGO_PKG_VERSION")))
        .build();
    ureq::Agent::new_with_config(cfg)
}

fn get(url: &str, limit: usize) -> Result<Vec<u8>, String> {
    let resp = agent().get(url).call().map_err(|e| e.to_string())?;
    let mut data = Vec::new();
    use std::io::Read;
    resp.into_body().into_with_config().limit(limit as u64 + 1).reader().take(limit as u64 + 1).read_to_end(&mut data).map_err(|e| e.to_string())?;
    if data.len() > limit {
        return Err("too large".into());
    }
    Ok(data)
}

/// Downloads a GIF from an address a search result gave, refusing anything else.
fn get_gif(url: &str, limit: usize) -> Result<Vec<u8>, String> {
    if !safe_media_url(url) {
        return Err("The GIF service gave an address Voxa does not load".into());
    }
    let data = get(url, limit).map_err(|e| {
        if e == "too large" {
            format!("That GIF is larger than {} MB", limit / (1024 * 1024))
        } else {
            tracing::warn!("GIF download failed: {e}");
            "The GIF could not be downloaded: try another one".into()
        }
    })?;
    if !data.starts_with(b"GIF87a") && !data.starts_with(b"GIF89a") {
        return Err("The GIF service did not send a GIF".into());
    }
    Ok(data)
}

fn clean_title(s: &str) -> String {
    s.chars().filter(|c| !c.is_control()).take(120).collect::<String>().trim().to_string()
}

/// Tenor v2 / KLIPY: `{ results: [{ id, content_description, title, media_formats:
/// { gif: { url, dims }, tinygif: { url, dims } } }], next }`.
pub fn parse_tenor(body: &str) -> Result<(Vec<Item>, Option<String>), String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|_| "The GIF service sent an answer Voxa cannot read".to_string())?;
    let mut out = Vec::new();
    for r in v["results"].as_array().map(|a| a.as_slice()).unwrap_or_default().iter().take(PAGE * 2) {
        let m = &r["media_formats"];
        let (Some(gif), Some(prev)) = (m["gif"]["url"].as_str(), m["tinygif"]["url"].as_str().or(m["nanogif"]["url"].as_str())) else { continue };
        let dims = |f: &serde_json::Value| (f["dims"][0].as_u64().unwrap_or(0) as u32, f["dims"][1].as_u64().unwrap_or(0) as u32);
        let (w, h) = dims(&m["gif"]);
        let title = clean_title(r["content_description"].as_str().filter(|t| !t.is_empty()).or(r["title"].as_str()).unwrap_or("GIF"));
        out.push(Item { info: GifInfo { id: String::new(), title, width: w.min(10_000), height: h.min(10_000) }, gif_url: gif.into(), preview_url: prev.into() });
    }
    let next = v["next"].as_str().filter(|n| !n.is_empty() && *n != "0").map(String::from);
    Ok((out, next))
}

/// GIPHY: `{ data: [{ id, title, images: { original: { url, width, height },
/// fixed_width_small: { url } } }], pagination: { offset, count, total_count } }`.
pub fn parse_giphy(body: &str) -> Result<(Vec<Item>, Option<String>), String> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|_| "The GIF service sent an answer Voxa cannot read".to_string())?;
    let mut out = Vec::new();
    for r in v["data"].as_array().map(|a| a.as_slice()).unwrap_or_default().iter().take(PAGE * 2) {
        let im = &r["images"];
        let Some(gif) = im["original"]["url"].as_str() else { continue };
        let prev = im["fixed_width_small"]["url"].as_str().or(im["fixed_width_downsampled"]["url"].as_str()).unwrap_or(gif);
        let num = |x: &serde_json::Value| x.as_str().and_then(|s| s.parse::<u32>().ok()).or(x.as_u64().map(|n| n as u32)).unwrap_or(0);
        let title = clean_title(r["title"].as_str().filter(|t| !t.is_empty()).unwrap_or("GIF"));
        out.push(Item {
            info: GifInfo { id: String::new(), title, width: num(&im["original"]["width"]).min(10_000), height: num(&im["original"]["height"]).min(10_000) },
            gif_url: gif.into(),
            preview_url: prev.into(),
        });
    }
    let p = &v["pagination"];
    let (off, count, total) = (p["offset"].as_u64().unwrap_or(0), p["count"].as_u64().unwrap_or(0), p["total_count"].as_u64().unwrap_or(0));
    let next = (count > 0 && off + count < total && off + count < 5_000).then(|| (off + count).to_string());
    Ok((out, next))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tenor_format() {
        let body = r#"{"results":[{"id":"1","content_description":"Cat\u0007 dance","media_formats":{"gif":{"url":"https://static.klipy.com/a.gif","dims":[480,270]},"tinygif":{"url":"https://static.klipy.com/a-s.gif","dims":[220,124]}}},{"id":"2","media_formats":{"mp4":{"url":"x"}}}],"next":"abc"}"#;
        let (items, next) = parse_tenor(body).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!((items[0].info.title.as_str(), items[0].info.width, items[0].info.height), ("Cat dance", 480, 270));
        assert_eq!(items[0].preview_url, "https://static.klipy.com/a-s.gif");
        assert_eq!(next.as_deref(), Some("abc"));
        assert!(parse_tenor("not json").is_err());
    }

    #[test]
    fn giphy_format() {
        let body = r#"{"data":[{"id":"x","title":"Hello","images":{"original":{"url":"https://media.giphy.com/m/x/giphy.gif","width":"500","height":"281"},"fixed_width_small":{"url":"https://media.giphy.com/m/x/100w.gif"}}}],"pagination":{"offset":0,"count":1,"total_count":40}}"#;
        let (items, next) = parse_giphy(body).unwrap();
        assert_eq!((items[0].info.width, items[0].info.height, next.as_deref()), (500, 281, Some("1")));
    }

    #[test]
    fn media_urls() {
        assert!(safe_media_url("https://media.giphy.com/media/x/giphy.gif"));
        assert!(!safe_media_url("http://media.giphy.com/x.gif"));
        assert!(!safe_media_url("https://127.0.0.1/x.gif"));
        assert!(!safe_media_url("https://localhost/x.gif"));
        assert!(!safe_media_url("https://[::1]/x.gif"));
        assert!(!safe_media_url("https://user@10.0.0.1/x.gif"));
        assert!(!safe_media_url("https://intranet/x.gif"));
        assert!(!safe_media_url("file:///c:/x.gif"));
    }
}
