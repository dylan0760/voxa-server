use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Instant;

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use base64::Engine;
use parking_lot::Mutex;
use tokio::sync::{mpsc, Notify};
use tracing::{info, warn};
use voxa_proto::identity::KeyPair;
use voxa_proto::voice::{ReplayWindow, VoiceCipher};
use voxa_proto::*;

use crate::config::Config;
use crate::store::*;

pub enum Out {
    Msg(S2C),
    Close,
}

/// Messages waiting for one client at most. A client that stops reading its connection
/// is disconnected instead of filling the server's memory.
pub const OUTBOX_SIZE: usize = 4096;

/// The sending side of a client's control connection.
#[derive(Clone)]
pub struct Outbox {
    pub tx: mpsc::Sender<Out>,
    pub close: Arc<Notify>,
}

impl Outbox {
    pub fn send(&self, o: Out) {
        if let Err(mpsc::error::TrySendError::Full(_)) = self.tx.try_send(o) {
            self.close.notify_one();
        }
    }
}

pub struct Session {
    pub info: ClientInfo,
    pub tx: Outbox,
    pub close: Arc<Notify>,
    pub ip: IpAddr,
    pub voice: Arc<VoiceCipher>,
    pub voice_addr: Option<SocketAddr>,
    pub replay: ReplayWindow,
    pub s2c_seq: u64,
    pub bulk_token: [u8; 32],
    pub bulk_tx: Option<mpsc::Sender<BulkS2C>>,
    /// Screen share frames queued on the bulk connection (counted down when sent).
    pub bulk_frames: Arc<AtomicUsize>,
    /// Clients currently watching this client's screen share.
    pub viewers: HashSet<ClientId>,
    pub last_keyframe_req: Option<Instant>,
    /// Per-user permissions granted on top of the user's groups.
    pub extra_perms: u64,
    /// Chat rate limit: (tokens left, last refill).
    pub chat_budget: (f32, Instant),
    /// Signed in through the web admin page (no voice; may run next to the same
    /// identity's desktop client).
    pub web: bool,
    /// Hardware tokens the client reported (for hardware bans).
    pub hardware: Vec<String>,
    /// Voxa 1.21.0+ must report its hardware by then.
    pub hardware_due: Option<Instant>,
    /// Online / away / busy / invisible (1.17.0+).
    pub presence: Presence,
    /// The camera, while it is on (1.21.0+): pictures go to everyone in the channel.
    pub camera: Option<CameraInfo>,
    pub last_camera_key_req: Option<Instant>,
    /// Channels whose calendar this client has open (it gets every change).
    pub calendar_subs: HashSet<ChannelId>,
}

/// Chat messages, images and pokes: bursts of this many, then `CHAT_RATE` per second.
const CHAT_BURST: f32 = 10.0;
const CHAT_RATE: f32 = 2.0;

impl Session {
    /// Takes one token from the chat budget; false when the client sends too fast.
    pub fn spend_chat(&mut self) -> bool {
        let (tokens, last) = &mut self.chat_budget;
        *tokens = (*tokens + last.elapsed().as_secs_f32() * CHAT_RATE).min(CHAT_BURST);
        *last = Instant::now();
        if *tokens >= 1.0 {
            *tokens -= 1.0;
            true
        } else {
            false
        }
    }

    /// Feature detection for messages added after 1.2 (old clients cannot decode them).
    pub fn client_at_least(&self, min: &str) -> bool {
        !voxa_proto::update::is_newer(min, &self.info.version)
    }

    /// Invisible and not in a channel: shown to others as offline.
    pub fn hidden(&self) -> bool {
        self.presence == Presence::Invisible && self.info.channel == NO_CHANNEL
    }
}

pub struct ChannelRec {
    pub info: ChannelInfo,
    pub password_hash: Option<String>,
}

pub struct State {
    pub settings: StoredSettings,
    pub channels: BTreeMap<ChannelId, ChannelRec>,
    pub groups: BTreeMap<GroupId, GroupInfo>,
    pub sessions: HashMap<ClientId, Session>,
    /// File browser limits (0 = unlimited); editable by admins from the client.
    pub file_limits: FileLimits,
    /// Server-wide options (auto-away), editable by admins.
    pub options: ServerOptions,
}

pub struct Server {
    pub state: Mutex<State>,
    pub store: Store,
    pub cfg: Config,
    pub data_dir: PathBuf,
    pub identity: KeyPair,
    pub fingerprint: String,
    pub started: u64,
    pub pw: Mutex<PasswordWork>,
    /// Bytes promised to running uploads, per channel (see net.rs `Reservation`).
    pub reserved: Mutex<HashMap<ChannelId, u64>>,
    /// Oldest client version allowed to connect (`require_latest_client`).
    pub min_client: Mutex<String>,
    /// Argon2 runs at most this many at once, so password floods cannot use every core.
    argon_slots: tokio::sync::Semaphore,
}

pub type Res<T = ()> = Result<T, String>;

/// Creates (id 0) or changes an event of a calendar. Returns its id.
fn save_event(cal: &mut ChannelCalendar, event: CalendarEvent, uid: &str, name: &str) -> Res<EventId> {
    let title = clean_line(&event.title, 100);
    if title.is_empty() {
        return Err("An event needs a title".into());
    }
    let description = clean_text(&event.description, 2000);
    let duration_min = if event.duration_min == 0 { 60 } else { event.duration_min.min(7 * 24 * 60) };
    let max_people = match event.max_people {
        Some(n) if !(1..=1000).contains(&n) => return Err("The number of people must be between 1 and 1000".into()),
        m => m,
    };
    let now = now_unix();
    if event.start > now + 2 * 366 * 86_400 {
        return Err("Events can be arranged up to two years ahead".into());
    }
    // Calendar 0 (or one that is gone): the first one, made if there is none.
    let calendar = match cal.calendars.iter().find(|c| c.id == event.calendar) {
        Some(c) => c.id,
        None => match cal.calendars.first() {
            Some(c) => c.id,
            None => {
                cal.next_id += 1;
                cal.calendars.push(CalendarInfo { id: cal.next_id, name: "General".into() });
                cal.next_id
            }
        },
    };
    if event.id == 0 {
        if event.start + 86_400 < now {
            return Err("Events can be arranged from today up to two years ahead".into());
        }
        if cal.events.len() >= MAX_EVENTS {
            return Err(format!("A calendar can have at most {MAX_EVENTS} events"));
        }
        cal.next_id += 1;
        cal.events.push(CalendarEvent {
            id: cal.next_id,
            calendar,
            title,
            description,
            start: event.start,
            duration_min,
            max_people,
            created_by: name.to_string(),
            created_uid: uid.to_string(),
            rsvps: Vec::new(),
        });
        Ok(cal.next_id)
    } else {
        let e = cal.events.iter_mut().find(|e| e.id == event.id).ok_or("Event not found")?;
        e.calendar = calendar;
        e.title = title;
        e.description = description;
        e.start = event.start;
        e.duration_min = duration_min;
        e.max_people = max_people;
        Ok(e.id)
    }
}

fn db<T>(r: anyhow::Result<T>) -> Res<T> {
    r.map_err(|e| {
        warn!("database error: {e:#}");
        format!("database error: {e}")
    })
}

pub fn hash_password(pw: &str) -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default().hash_password(pw.as_bytes(), &salt).map(|h| h.to_string()).unwrap_or_default()
}

pub fn verify_password(pw: &str, hash: &str) -> bool {
    PasswordHash::new(hash).map(|h| Argon2::default().verify_password(pw.as_bytes(), &h).is_ok()).unwrap_or(false)
}

/// Wrong passwords per address within `FAILURE_WINDOW` before further tries are refused.
const MAX_FAILURES: u32 = 10;
const FAILURE_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);
/// How long a verified password stays known (only its hash is kept).
const VERIFIED_TTL: std::time::Duration = std::time::Duration::from_secs(600);

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    for p in parts {
        h.update((p.len() as u64).to_le_bytes());
        h.update(p);
    }
    h.finalize().into()
}

/// Argon2 is slow on purpose, so it never runs while the server state is locked:
/// passwords are verified / hashed before a message is handled, and the handler only
/// looks the result up here.
#[derive(Default)]
pub struct PasswordWork {
    /// SHA-256 of (stored hash, password) pairs that were verified as correct.
    verified: HashMap<[u8; 32], Instant>,
    /// Hashes made ahead for passwords about to be set, by SHA-256 of the password.
    hashed: HashMap<[u8; 32], (String, Instant)>,
    /// Wrong passwords per address: (count, start of the window).
    failures: HashMap<IpAddr, (u32, Instant)>,
}

impl PasswordWork {
    fn is_verified(&self, hash: &str, pw: &str) -> bool {
        self.verified.get(&sha256(&[hash.as_bytes(), pw.as_bytes()])).map(|t| t.elapsed() < VERIFIED_TTL).unwrap_or(false)
    }

    fn check_failures(&self, ip: IpAddr) -> Res {
        match self.failures.get(&ip) {
            Some((n, since)) if *n >= MAX_FAILURES && since.elapsed() < FAILURE_WINDOW => {
                Err("Too many wrong passwords. Please wait a minute and try again.".into())
            }
            _ => Ok(()),
        }
    }

    fn record(&mut self, ip: IpAddr, hash: &str, pw: &str, ok: bool) {
        let now = Instant::now();
        if ok {
            if self.verified.len() > 4096 {
                self.verified.retain(|_, t| t.elapsed() < VERIFIED_TTL);
            }
            self.verified.insert(sha256(&[hash.as_bytes(), pw.as_bytes()]), now);
        } else {
            if self.failures.len() > 10_000 {
                self.failures.retain(|_, (_, t)| t.elapsed() < FAILURE_WINDOW);
            }
            let e = self.failures.entry(ip).or_insert((0, now));
            if e.1.elapsed() >= FAILURE_WINDOW {
                *e = (0, now);
            }
            e.0 += 1;
        }
    }
}

/// Largest screen share frame relayed, and how many may wait for one viewer.
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
const MAX_QUEUED_FRAMES: usize = 8;
/// A camera picture (at most 720p) is far smaller than a shared screen.
const MAX_CAMERA_FRAME_BYTES: usize = 2 * 1024 * 1024;

/// The Voxa client released together with this server: the oldest one it accepts when
/// `require_latest_client` is on (newer releases found on GitHub raise it).
pub const MIN_CLIENT_VERSION: &str = "1.31.0";

/// Offline direct messages: how many may wait per person, and for how many days.
pub const MAX_OFFLINE_WAITING: usize = 100;
pub const OFFLINE_DAYS: u64 = 30;
/// Answer to messages and images for the Server tab, which shows server events only.
pub const SERVER_TAB_READ_ONLY: &str = "The Server tab only shows what happens on the server: chat in a channel, or with direct or group messages";
/// Group chats one person can be in.
pub const MAX_GROUP_CHATS_PER_USER: usize = 100;

/// Most channels a server holds, and how deeply they can be nested.
pub const MAX_CHANNELS: usize = 2000;
pub const MAX_CHANNEL_DEPTH: usize = 16;

pub const GROUP_ADMIN: GroupId = 1;
pub const GROUP_NORMAL: GroupId = 2;
pub const GROUP_GUEST: GroupId = 3;
pub const GROUP_CHANNEL_ADMIN: GroupId = 4;
pub const GROUP_CHANNEL_OPERATOR: GroupId = 5;
pub const GROUP_CHANNEL_GUEST: GroupId = 6;

fn default_groups() -> Vec<GroupInfo> {
    use perm::*;
    let normal = CHANNEL_CREATE_TEMPORARY
        | CLIENT_POKE
        | PRIVATE_MESSAGE
        | SERVER_CHAT
        | CHANNEL_CHAT
        | CHAT_HISTORY
        | FILE_BROWSE
        | FILE_DOWNLOAD
        | SCREEN_SHARE
        | CHANNEL_COMMANDER
        | CHANNEL_JOIN
        | CHAT_EDIT_OWN
        | CALENDAR_VIEW
        | CALENDAR_RSVP;
    // Join power mirrors TeamSpeak: Admin 75, Normal 10, Guest 0. A channel with
    // "needed join power" 10 is therefore members-only, 75 is admins-only.
    let g = |id, kind, name: &str, perms, talk_power, sort, show_name| GroupInfo {
        id,
        kind,
        name: name.into(),
        perms,
        talk_power,
        join_power: match id {
            GROUP_ADMIN => 75,
            GROUP_NORMAL => 10,
            GROUP_CHANNEL_ADMIN => 50,
            GROUP_CHANNEL_OPERATOR => 35,
            _ => 0,
        },
        sort,
        builtin: true,
        show_name,
    };
    vec![
        g(GROUP_ADMIN, GroupKind::Server, "Server Admin", ALL, 75, 10, true),
        g(GROUP_NORMAL, GroupKind::Server, "Normal", normal, 0, 20, false),
        g(GROUP_GUEST, GroupKind::Server, "Guest", CHANNEL_JOIN | SERVER_CHAT | CHANNEL_CHAT | CHAT_HISTORY | FILE_BROWSE | CALENDAR_VIEW, 0, 30, false),
        g(
            GROUP_CHANNEL_ADMIN,
            GroupKind::Channel,
            "Channel Admin",
            CHANNEL_EDIT
                | CHANNEL_DELETE
                | CHANNEL_JOIN_IGNORE_PASSWORD
                | CLIENT_KICK_CHANNEL
                | TALK_POWER_GRANT
                | GROUP_ASSIGN_CHANNEL
                | FILE_BROWSE
                | FILE_UPLOAD
                | FILE_DOWNLOAD
                | FILE_DELETE
                | CHAT_DELETE
                | CHAT_PIN
                | CALENDAR_EVENTS
                | CALENDAR_MANAGE,
            50,
            10,
            true,
        ),
        g(
            GROUP_CHANNEL_OPERATOR,
            GroupKind::Channel,
            "Operator",
            CLIENT_KICK_CHANNEL | TALK_POWER_GRANT | FILE_UPLOAD | CHAT_DELETE,
            35,
            20,
            true,
        ),
        g(GROUP_CHANNEL_GUEST, GroupKind::Channel, "Guest", 0, 0, 30, false),
    ]
}

impl State {
    pub fn send(&self, id: ClientId, msg: S2C) {
        if let Some(s) = self.sessions.get(&id) {
            s.tx.send(Out::Msg(msg));
        }
    }

    pub fn broadcast(&self, msg: S2C) {
        for s in self.sessions.values() {
            s.tx.send(Out::Msg(msg.clone()));
        }
    }

    pub fn broadcast_channel(&self, ch: ChannelId, msg: S2C) {
        for s in self.sessions.values().filter(|s| s.info.channel == ch) {
            s.tx.send(Out::Msg(msg.clone()));
        }
    }

    fn broadcast_client(&self, id: ClientId) {
        if let Some(s) = self.sessions.get(&id) {
            self.broadcast_about(id, S2C::ClientUpdated(s.info.clone()));
        }
    }

    /// Administrators see invisible people (to moderate); everyone sees themselves.
    fn sees(&self, viewer: &Session, subject: &Session) -> bool {
        viewer.info.id == subject.info.id || !subject.hidden() || self.server_perms(viewer) & perm::SERVER_EDIT != 0
    }

    /// Sends a message about someone to everyone who can see them.
    fn broadcast_about(&self, subject: ClientId, msg: S2C) {
        let Some(sub) = self.sessions.get(&subject) else { return };
        for s in self.sessions.values().filter(|s| self.sees(s, sub)) {
            s.tx.send(Out::Msg(msg.clone()));
        }
    }

    /// After someone's visibility may have changed (`was_hidden` before): people who can
    /// now see them get them as joined, people who no longer can get them as gone.
    /// `avatar` is their picture, for people who never got it while they were hidden.
    fn announce_visibility(&self, id: ClientId, was_hidden: bool, avatar: Option<AvatarMeta>) {
        let Some(sub) = self.sessions.get(&id) else { return };
        let now_hidden = sub.hidden();
        if was_hidden == now_hidden {
            return;
        }
        for v in self.sessions.values().filter(|v| v.info.id != id && self.server_perms(v) & perm::SERVER_EDIT == 0) {
            if now_hidden {
                v.tx.send(Out::Msg(S2C::ClientLeft { id, reason: LeaveReason::Disconnected(String::new()) }));
            } else {
                v.tx.send(Out::Msg(S2C::ClientJoined(sub.info.clone())));
                if let Some(meta) = avatar.as_ref().filter(|_| v.client_at_least("1.20.0")) {
                    v.tx.send(Out::Msg(S2C::AvatarInfo(vec![meta.clone()])));
                }
            }
        }
    }

    /// Someone's presence for the clients that know presences and can see them.
    fn broadcast_presence(&self, id: ClientId) {
        let Some(sub) = self.sessions.get(&id) else { return };
        for v in self.sessions.values().filter(|v| self.sees(v, sub) && v.client_at_least("1.22.0")) {
            v.tx.send(Out::Msg(S2C::ClientPresence { id, presence: sub.presence }));
        }
    }

    pub fn default_channel(&self) -> ChannelId {
        self.channels
            .values()
            .find(|c| c.info.is_default)
            .or_else(|| self.channels.values().next())
            .map(|c| c.info.id)
            .unwrap_or(0)
    }

    fn spend_chat(&mut self, id: ClientId) -> Res {
        let s = self.sessions.get_mut(&id).ok_or("Client not found")?;
        if s.spend_chat() {
            Ok(())
        } else {
            Err("You are sending messages too fast. Please wait a moment.".into())
        }
    }

    fn session(&self, id: ClientId) -> Res<&Session> {
        self.sessions.get(&id).ok_or_else(|| "Client not found".to_string())
    }

    fn group_perms(&self, g: GroupId) -> u64 {
        self.groups.get(&g).map(|g| g.perms).unwrap_or(0)
    }

    /// Server-wide permissions: all server groups plus the user's own permissions.
    pub fn server_perms(&self, s: &Session) -> u64 {
        s.info.server_groups.iter().fold(s.extra_perms, |acc, g| acc | self.group_perms(*g))
    }

    /// Sends a message only to clients new enough to understand it.
    fn send_if_supported(&self, id: ClientId, min: &str, msg: S2C) {
        if let Some(s) = self.sessions.get(&id) {
            if s.client_at_least(min) {
                s.tx.send(Out::Msg(msg));
            }
        }
    }

    pub fn talk_power(&self, s: &Session) -> i32 {
        s.info
            .server_groups
            .iter()
            .chain(std::iter::once(&s.info.channel_group))
            .filter_map(|g| self.groups.get(g))
            .map(|g| g.talk_power)
            .max()
            .unwrap_or(0)
    }

    /// Highest join power of the given server groups plus a channel group.
    pub fn join_power(&self, server_groups: &[GroupId], channel_group: GroupId) -> i32 {
        server_groups
            .iter()
            .chain(std::iter::once(&channel_group))
            .filter_map(|g| self.groups.get(g))
            .map(|g| g.join_power)
            .max()
            .unwrap_or(0)
    }

    pub fn can_talk(&self, s: &Session) -> bool {
        // Nobody to talk to outside a channel.
        if s.info.input_muted || s.info.channel == NO_CHANNEL {
            return false;
        }
        let needed = self.channels.get(&s.info.channel).map(|c| c.info.needed_talk_power).unwrap_or(0);
        s.info.is_talker || self.talk_power(s) >= needed
    }

    fn children(&self, ch: ChannelId) -> Vec<ChannelId> {
        self.channels.values().filter(|c| c.info.parent == Some(ch)).map(|c| c.info.id).collect()
    }

    /// The channel and all of its descendants, deepest first (every channel comes
    /// before its parent). A loop, not recursion: trees can be deep.
    fn subtree(&self, ch: ChannelId) -> Vec<ChannelId> {
        let mut kids: HashMap<ChannelId, Vec<ChannelId>> = HashMap::new();
        for c in self.channels.values() {
            if let Some(p) = c.info.parent {
                kids.entry(p).or_default().push(c.info.id);
            }
        }
        let mut order = vec![ch];
        let mut seen = HashSet::from([ch]);
        let mut i = 0;
        while i < order.len() {
            for &k in kids.get(&order[i]).map(Vec::as_slice).unwrap_or_default() {
                if seen.insert(k) {
                    order.push(k);
                }
            }
            i += 1;
        }
        order.reverse();
        order
    }

    /// Nesting level of a channel: 1 for a top level channel.
    fn depth(&self, ch: ChannelId) -> usize {
        let mut d = 0;
        let mut cur = Some(ch);
        while let Some(c) = cur {
            d += 1;
            if d > MAX_CHANNEL_DEPTH * 4 {
                break;
            }
            cur = self.channels.get(&c).and_then(|c| c.info.parent);
        }
        d
    }

    /// Levels in the tree below and including `ch` (1 = no sub-channels).
    fn height(&self, ch: ChannelId) -> usize {
        let base = self.depth(ch);
        self.subtree(ch).iter().map(|c| self.depth(*c) + 1 - base).max().unwrap_or(1)
    }

    fn clients_in(&self, ch: ChannelId) -> usize {
        self.sessions.values().filter(|s| s.info.channel == ch).count()
    }

    pub fn server_info(&self, srv: &Server) -> ServerInfo {
        let s = &self.settings;
        ServerInfo {
            name: s.name.clone(),
            welcome_message: s.welcome_message.clone(),
            host_message: s.host_message.clone(),
            max_clients: s.max_clients,
            has_password: s.password_hash.is_some(),
            default_server_group: s.default_server_group,
            default_channel_group: s.default_channel_group,
            channel_admin_group: s.channel_admin_group,
            chat_retention_days: s.chat_retention_days,
            version: APP_VERSION.into(),
            platform: std::env::consts::OS.into(),
            created: s.created,
            started: srv.started,
            fingerprint: srv.fingerprint.clone(),
        }
    }
}

impl Server {
    pub fn new(cfg: Config, data_dir: PathBuf, store: Store, identity: KeyPair) -> anyhow::Result<Server> {
        let fingerprint = identity::fingerprint(&identity.public);
        let settings = match store.load_settings()? {
            Some(s) => s,
            None => Self::bootstrap(&store, &data_dir)?,
        };
        let mut groups = BTreeMap::new();
        for g in store.load_groups()? {
            groups.insert(g.id, g);
        }
        // The Server Admin group always holds every permission, including ones added
        // in newer versions.
        if let Some(g) = groups.get_mut(&GROUP_ADMIN) {
            if g.perms != perm::ALL {
                g.perms = perm::ALL;
                store.save_group(g)?;
            }
        }
        let mut channels = BTreeMap::new();
        for c in store.load_channels()? {
            channels.insert(c.info.id, ChannelRec { info: c.info, password_hash: c.password_hash });
        }
        if channels.is_empty() {
            let id = store.next_id("next_channel")?;
            let info = ChannelInfo {
                id,
                parent: None,
                sort: 0,
                name: "Default Channel".into(),
                topic: String::new(),
                description: "This is the default channel.".into(),
                has_password: false,
                max_clients: None,
                kind: ChannelKind::Permanent,
                is_default: true,
                codec_quality: 6,
                needed_talk_power: 0,
                needed_join_power: 0,
                voice_encrypted: true,
            };
            store.save_channel(&StoredChannel { info: info.clone(), password_hash: None })?;
            channels.insert(id, ChannelRec { info, password_hash: None });
        }
        let srv = Server {
            state: Mutex::new(State {
                settings,
                channels,
                groups,
                sessions: HashMap::new(),
                // Values saved from the client win; server.toml only provides the default.
                options: store.server_options()?.unwrap_or_default(),
            file_limits: store.load_file_limits()?.unwrap_or(FileLimits {
                    max_upload_mb: cfg.max_upload_mb,
                    channel_quota_mb: cfg.channel_quota_mb,
                }),
            }),
            store,
            cfg,
            data_dir,
            identity,
            fingerprint,
            started: now_unix(),
            pw: Mutex::new(PasswordWork::default()),
            reserved: Mutex::new(HashMap::new()),
            min_client: Mutex::new(MIN_CLIENT_VERSION.to_string()),
            argon_slots: tokio::sync::Semaphore::new(2),
        };
        srv.ensure_admin_key()?;
        Ok(srv)
    }

    fn bootstrap(store: &Store, _dir: &std::path::Path) -> anyhow::Result<StoredSettings> {
        info!("first start: creating default server configuration");
        for g in default_groups() {
            store.save_group(&g)?;
        }
        // Reserve the built-in group ids.
        while store.next_id("next_group")? < 100 {}
        let s = StoredSettings {
            name: "Voxa Server".into(),
            welcome_message: "Welcome to Voxa - free, open source voice chat.".into(),
            host_message: String::new(),
            max_clients: 64,
            password_hash: None,
            default_server_group: GROUP_NORMAL,
            default_channel_group: GROUP_CHANNEL_GUEST,
            channel_admin_group: GROUP_CHANNEL_ADMIN,
            chat_retention_days: 0,
            created: now_unix(),
        };
        store.save_settings(&s)?;
        Ok(s)
    }

    /// As long as nobody holds the Server Admin group, keep an unused admin privilege
    /// key around and write it to `privilege_key.txt` in the data directory.
    fn ensure_admin_key(&self) -> anyhow::Result<()> {
        let path = self.data_dir.join("privilege_key.txt");
        let has_admin = self.store.all_clients()?.iter().any(|c| c.server_groups.contains(&GROUP_ADMIN));
        if has_admin {
            let _ = std::fs::remove_file(&path);
            return Ok(());
        }
        let key = match self.store.privkey_for_group(GROUP_ADMIN)? {
            Some(k) => k,
            None => {
                let k = new_privilege_key();
                self.store.add_privkey(
                    &k,
                    &PrivKey { group: GROUP_ADMIN, channel: None, description: "Initial admin key".into(), created: now_unix() },
                )?;
                k
            }
        };
        let text = format!(
            "Voxa Server - Server Admin privilege key\r\n\r\n{key}\r\n\r\nConnect with the Voxa client and use Permissions > Use Privilege Key.\r\nThis file is removed once an administrator exists.\r\n"
        );
        std::fs::write(&path, text)?;
        // The key itself is not logged: logs are kept (and shared) long after it is used.
        info!("no Server Admin yet: the admin privilege key is in {}", path.display());
        Ok(())
    }

    fn need(&self, have: u64, bit: u64) -> Res {
        if have & bit == bit {
            Ok(())
        } else {
            let name = perm::NAMES.iter().find(|(b, _)| *b == bit).map(|(_, n)| *n).unwrap_or("permission");
            Err(format!("Insufficient permissions ({name})"))
        }
    }

    fn channel_group_for(&self, st: &State, uid: &str, ch: ChannelId) -> GroupId {
        match self.store.channel_group(uid, ch) {
            Ok(Some(g)) if st.groups.contains_key(&g) => g,
            _ => st.settings.default_channel_group,
        }
    }

    /// Effective permissions of a session for an action scoped to channel `ch`.
    fn perms_in(&self, st: &State, s: &Session, ch: ChannelId) -> u64 {
        let cg = if s.info.channel == ch { s.info.channel_group } else { self.channel_group_for(st, &s.info.uid, ch) };
        st.server_perms(s) | st.group_perms(cg)
    }

    /// The client version a Hello must have, or an `UPDATE_REQUIRED` refusal.
    pub fn check_client_version(&self, version: &str) -> Res {
        if !self.cfg.require_latest_client {
            return Ok(());
        }
        let min = self.min_client.lock().clone();
        if voxa_proto::update::is_newer(&min, version) {
            return Err(format!(
                "{}This server needs Voxa {min} or newer (you have {}). Update Voxa, then connect again.",
                voxa_proto::UPDATE_REQUIRED,
                if version.trim().is_empty() { "an unknown version" } else { version }
            ));
        }
        Ok(())
    }

    // ------------------------------------------------------------------ passwords

    /// Runs Argon2 on a blocking thread, a few at a time.
    async fn argon<T: Send + 'static>(&self, f: impl FnOnce() -> T + Send + 'static) -> Res<T> {
        let _slot = self.argon_slots.acquire().await.map_err(|_| "Server is shutting down".to_string())?;
        tokio::task::spawn_blocking(f).await.map_err(|e| e.to_string())
    }

    /// Verifies a password before the state is locked; the handler then finds the
    /// result with `pw_verified`. Wrong passwords count against the address.
    async fn verify_ahead(&self, ip: IpAddr, pw: &str, hash: &str) -> Res {
        {
            let w = self.pw.lock();
            if w.is_verified(hash, pw) {
                return Ok(());
            }
            w.check_failures(ip)?;
        }
        let (p, h) = (pw.to_string(), hash.to_string());
        let ok = self.argon(move || verify_password(&p, &h)).await?;
        self.pw.lock().record(ip, hash, pw, ok);
        if !ok {
            warn!("wrong password from {ip}");
        }
        Ok(())
    }

    /// Hashes a password that is about to be set, before the state is locked.
    async fn hash_ahead(&self, change: &PasswordChange) -> Res {
        if let PasswordChange::Set(pw) = change {
            if !pw.is_empty() {
                let p = pw.clone();
                let hash = self.argon(move || hash_password(&p)).await?;
                let mut w = self.pw.lock();
                if w.hashed.len() > 256 {
                    w.hashed.retain(|_, (_, t)| t.elapsed() < VERIFIED_TTL);
                }
                w.hashed.insert(sha256(&[pw.as_bytes()]), (hash, Instant::now()));
            }
        }
        Ok(())
    }

    /// True if `pw` matches `hash`. Only looks up `verify_ahead`'s result (cheap).
    fn pw_verified(&self, pw: &str, hash: &str) -> bool {
        self.pw.lock().is_verified(hash, pw)
    }

    /// The hash of a new password, made ahead by `hash_ahead` (computed here only if
    /// that did not happen).
    fn pw_hash(&self, pw: &str) -> String {
        let ready = self.pw.lock().hashed.remove(&sha256(&[pw.as_bytes()]));
        ready.map(|(h, _)| h).unwrap_or_else(|| hash_password(pw))
    }

    /// Checks the passwords of a Hello before the login takes the state lock.
    pub async fn check_login_passwords(
        &self,
        ip: IpAddr,
        server_password: Option<&str>,
        default_channel: Option<&str>,
        channel_password: Option<&str>,
    ) -> Res {
        let (server_hash, channel_hash) = {
            let st = self.state.lock();
            let ch = default_channel.map(join::parse).filter(|(no, _, _)| !no).and_then(|(_, _, p)| find_channel_by_path(&st, p));
            (st.settings.password_hash.clone(), ch.and_then(|c| st.channels.get(&c)).and_then(|c| c.password_hash.clone()))
        };
        if let Some(h) = &server_hash {
            self.pw.lock().check_failures(ip)?;
            if let Some(p) = server_password {
                self.verify_ahead(ip, p, h).await?;
            }
        }
        if let (Some(h), Some(p)) = (&channel_hash, channel_password) {
            self.verify_ahead(ip, p, h).await?;
        }
        Ok(())
    }

    /// Does the slow password work of a message before it is handled under the lock.
    pub async fn prepare(&self, me: ClientId, msg: &C2S) -> Res {
        match msg {
            C2S::SwitchChannel { channel, password: Some(pw) } => {
                let (ip, hash) = {
                    let st = self.state.lock();
                    let Some(s) = st.sessions.get(&me) else { return Ok(()) };
                    (s.ip, st.channels.get(channel).and_then(|c| c.password_hash.clone()))
                };
                if let Some(h) = hash {
                    self.verify_ahead(ip, pw, &h).await?;
                }
                Ok(())
            }
            C2S::CreateChannel(e) | C2S::EditChannel { edit: e, .. } => self.hash_ahead(&e.password).await,
            C2S::EditServer(e) => self.hash_ahead(&e.password).await,
            _ => Ok(()),
        }
    }

    // ------------------------------------------------------------------ login

    #[allow(clippy::too_many_arguments)]
    pub fn login(
        &self,
        uid: String,
        ip: IpAddr,
        hello: C2S,
        tx: Outbox,
        close: Arc<Notify>,
        web: bool,
    ) -> Res<ClientId> {
        let C2S::Hello {
            protocol,
            version,
            platform,
            nickname,
            server_password,
            default_channel,
            default_channel_password,
        } = hello
        else {
            return Err("Expected hello".into());
        };
        if protocol != PROTOCOL_VERSION {
            return Err(format!(
                "Protocol mismatch (server {}, client {protocol}). Please update your client.",
                PROTOCOL_VERSION
            ));
        }
        // The ban list is read before the state is locked (database work).
        let ban = self.find_ban(Some(&uid), Some(ip));
        let mut st = self.state.lock();
        if let Some(h) = &st.settings.password_hash {
            if !server_password.as_deref().map(|p| self.pw_verified(p, h)).unwrap_or(false) {
                return Err("Invalid server password".into());
            }
        }
        let mut record = db(self.store.get_client(&uid))?.unwrap_or_else(|| ClientRecord {
            uid: uid.clone(),
            nickname: String::new(),
            description: String::new(),
            server_groups: vec![st.settings.default_server_group],
            first_seen: now_unix(),
            last_seen: 0,
            connections: 0,
            last_ip: String::new(),
        });
        record.server_groups.retain(|g| st.groups.get(g).map(|g| g.kind == GroupKind::Server).unwrap_or(false));
        if record.server_groups.is_empty() {
            record.server_groups.push(st.settings.default_server_group);
        }
        let group_perms = record.server_groups.iter().fold(0, |a, g| a | st.group_perms(*g));
        // Server administrators cannot be locked out by a ban (e.g. on a shared address).
        if let Some(ban) = ban.filter(|_| group_perms & perm::SERVER_EDIT == 0) {
            let until = ban.expires.map(|e| format!(" (expires in {} min)", (e.saturating_sub(now_unix())) / 60 + 1)).unwrap_or_default();
            return Err(format!("You are banned from this server: {}{until}", ban.reason));
        }
        if st.sessions.len() as u32 >= st.settings.max_clients && group_perms & perm::IGNORE_SERVER_MAXCLIENTS == 0 {
            return Err("The server is full".into());
        }
        // Same identity connected twice: drop the stale session (TeamSpeak-like reconnect).
        // The web page and the desktop client are separate sessions and do not replace each other.
        if st.sessions.values().any(|s| s.info.uid == uid && s.web == web) {
            if let Some(old) = st.sessions.values().find(|s| s.info.uid == uid && s.web == web).map(|s| s.info.id) {
                drop(st);
                self.kick_session(old, LeaveReason::Disconnected("Reconnected".into()));
                st = self.state.lock();
            }
        }
        let nickname = self.unique_nickname(&st, &sanitize_nick(&nickname), None);
        let id = (1..=u16::MAX).find(|i| !st.sessions.contains_key(i)).ok_or("No free client slots")?;

        // Join options (stay out of channels, invisible), then the channel path.
        let (no_channel, invisible, path) = default_channel.as_deref().map(join::parse).map(|(a, b, p)| (a, b, p.to_string())).unwrap_or((false, false, String::new()));
        let presence = if invisible { Presence::Invisible } else { Presence::Online };
        // Default channel, optionally by path.
        let mut channel = if no_channel { NO_CHANNEL } else { st.default_channel() };
        if let Some(path) = Some(path).filter(|p| !no_channel && !p.trim().is_empty()) {
            if let Some(ch) = find_channel_by_path(&st, &path) {
                let c = &st.channels[&ch];
                let pw_ok = match &c.password_hash {
                    None => true,
                    Some(h) => default_channel_password.as_deref().map(|p| self.pw_verified(p, h)).unwrap_or(false)
                        || group_perms & perm::CHANNEL_JOIN_IGNORE_PASSWORD != 0,
                };
                let full = c.info.max_clients.map(|m| st.clients_in(ch) as u32 >= m).unwrap_or(false)
                    && group_perms & perm::CHANNEL_JOIN_IGNORE_MAXCLIENTS == 0;
                let cg = self.channel_group_for(&st, &uid, ch);
                let perms_there = group_perms | st.group_perms(cg);
                let may_join = perms_there & perm::CHANNEL_JOIN != 0
                    && (perms_there & perm::CHANNEL_JOIN_IGNORE_POWER != 0
                        || st.join_power(&record.server_groups, cg) >= c.info.needed_join_power);
                if pw_ok && !full && may_join && !voxa_proto::is_spacer(&c.info.name) {
                    channel = ch;
                }
            }
        }
        let channel_group = self.channel_group_for(&st, &uid, channel);

        record.nickname = nickname.clone();
        // "Last seen" is shown to everyone (People): invisible people keep the old time.
        if !(invisible && no_channel) {
            record.last_seen = now_unix();
        }
        record.connections += 1;
        record.last_ip = ip.to_string();
        db(self.store.save_client(&record))?;

        let info = ClientInfo {
            id,
            uid: uid.clone(),
            nickname,
            description: record.description.clone(),
            channel,
            server_groups: record.server_groups.clone(),
            channel_group,
            away: None,
            input_muted: false,
            output_muted: false,
            is_talker: false,
            talk_request: None,
            commander: false,
            streaming: None,
            platform: platform.chars().take(32).collect(),
            version: version.chars().take(32).collect(),
            connected_at: now_unix(),
        };
        let voice_key: [u8; 32] = crate::secrets::random_bytes();
        let bulk_token: [u8; 32] = crate::secrets::random_bytes();
        let viewer_admin = (group_perms | self.store.client_perms(&uid).unwrap_or(0)) & perm::SERVER_EDIT != 0;
        let welcome = S2C::Welcome {
            your_id: id,
            your_uid: uid.clone(),
            server: st.server_info(self),
            channels: st.channels.values().map(|c| c.info.clone()).collect(),
            // Invisible people are left out, except for administrators.
            clients: st
                .sessions
                .values()
                .filter(|s| !s.hidden() || viewer_admin)
                .map(|s| s.info.clone())
                .chain(std::iter::once(info.clone()))
                .collect(),
            groups: st.groups.values().cloned().collect(),
            voice: VoiceSetup { key: voice_key, udp_port: self.cfg.port },
            bulk_token,
        };
        tx.send(Out::Msg(welcome));
        let extra_perms = self.store.client_perms(&uid).unwrap_or(0);
        if !voxa_proto::update::is_newer("1.4.0", &info.version) {
            tx.send(Out::Msg(S2C::YourPerms(extra_perms)));
        }
        // Direct messages that arrived while this person was offline.
        if !voxa_proto::update::is_newer("1.18.0", &info.version) {
            match self.store.take_offline(&uid) {
                Ok(msgs) if !msgs.is_empty() => {
                    info!("delivering {} offline message(s) to {uid}", msgs.len());
                    tx.send(Out::Msg(S2C::OfflineMessages(msgs)));
                }
                Err(e) => warn!("could not read offline messages: {e:#}"),
                _ => {}
            }
        }
        // Whose cameras are on (Voxa 1.29.0+).
        if !voxa_proto::update::is_newer("1.29.0", &info.version) {
            let cams: Vec<(ClientId, CameraInfo)> = st.sessions.values().filter_map(|x| x.camera.map(|c| (x.info.id, c))).collect();
            tx.send(Out::Msg(S2C::Cameras(cams)));
        }
        // Group chats, then the group messages that arrived while this person was offline.
        if !voxa_proto::update::is_newer("1.27.0", &info.version) {
            let chats = self.store.group_chats_of(&uid).unwrap_or_default();
            let infos: Vec<GroupChatInfo> = chats.iter().map(|(id, rec)| self.group_info(&st, *id, rec)).collect();
            tx.send(Out::Msg(S2C::GroupChats(infos)));
            match self.store.take_group_queue(&uid) {
                Ok(msgs) => {
                    for m in msgs.into_iter().filter(|m| chats.iter().any(|(id, _)| *id == m.group)) {
                        let target = TextTarget::Group(m.group);
                        tx.send(Out::Msg(S2C::Text(ChatMessage { id: 0, target, from: None, from_uid: m.from_uid, from_name: m.from_name, ts: m.ts, text: m.text })));
                    }
                }
                Err(e) => warn!("could not read waiting group messages: {e:#}"),
            }
        }
        // Presences of the people the newcomer can see (busy, and invisible for administrators).
        if !voxa_proto::update::is_newer("1.22.0", &info.version) {
            for s in st.sessions.values().filter(|s| s.presence != Presence::Online && (!s.hidden() || viewer_admin)) {
                tx.send(Out::Msg(S2C::ClientPresence { id: s.info.id, presence: s.presence }));
            }
        }
        // Server options and (for Voxa 1.21.0+) a deadline to report the hardware.
        let new_client = !voxa_proto::update::is_newer("1.21.0", &info.version);
        if new_client {
            tx.send(Out::Msg(S2C::ServerOptions(st.options)));
        }
        let hardware_due = (new_client && !web).then(|| Instant::now() + std::time::Duration::from_secs(20));
        // Pictures: everyone online for the newcomer, the newcomer's for everyone else.
        if !voxa_proto::update::is_newer("1.20.0", &info.version) {
            let metas = st
                .sessions
                .values()
                .filter(|s| !s.hidden() || viewer_admin)
                .map(|s| s.info.uid.as_str())
                .chain(std::iter::once(uid.as_str()))
                .filter_map(|u| self.avatar_meta(u))
                .collect::<Vec<_>>();
            tx.send(Out::Msg(S2C::AvatarInfo(metas)));
        }
        let avatar = self.avatar_meta(&uid);
        info!("client {} ({}) connected from {ip} as id {id}{}", info.nickname, uid, if invisible { " (invisible)" } else { "" });
        st.sessions.insert(
            id,
            Session {
                info,
                tx,
                close,
                ip,
                voice: Arc::new(VoiceCipher::new(&voice_key)),
                voice_addr: None,
                replay: ReplayWindow::default(),
                s2c_seq: 0,
                bulk_token,
                bulk_tx: None,
                bulk_frames: Arc::new(AtomicUsize::new(0)),
                camera: None,
                last_camera_key_req: None,
                calendar_subs: HashSet::new(),
                viewers: HashSet::new(),
                last_keyframe_req: None,
                extra_perms,
                chat_budget: (CHAT_BURST, Instant::now()),
                web,
                hardware: Vec::new(),
                hardware_due,
                presence,
            },
        );
        // Everyone who can see the newcomer is told (invisible people stay unseen).
        let joined = st.sessions[&id].info.clone();
        let sub = &st.sessions[&id];
        for s in st.sessions.values().filter(|s| s.info.id != id && st.sees(s, sub)) {
            s.tx.send(Out::Msg(S2C::ClientJoined(joined.clone())));
            if presence != Presence::Online && s.client_at_least("1.22.0") {
                s.tx.send(Out::Msg(S2C::ClientPresence { id, presence }));
            }
            if let Some(meta) = &avatar {
                if s.client_at_least("1.20.0") {
                    s.tx.send(Out::Msg(S2C::AvatarInfo(vec![meta.clone()])));
                }
            }
        }
        Ok(id)
    }

    fn unique_nickname(&self, st: &State, nick: &str, except: Option<ClientId>) -> String {
        let taken = |n: &str| {
            st.sessions.values().any(|s| Some(s.info.id) != except && s.info.nickname.eq_ignore_ascii_case(n))
        };
        if !taken(nick) {
            return nick.to_string();
        }
        (1..).map(|i| format!("{nick}{i}")).find(|n| !taken(n)).unwrap()
    }

    fn find_ban(&self, uid: Option<&str>, ip: Option<IpAddr>) -> Option<BanEntry> {
        let now = now_unix();
        let bans = self.store.bans().ok()?;
        for b in bans {
            if b.expires.map(|e| e <= now).unwrap_or(false) {
                let _ = self.store.remove_ban(b.id);
                continue;
            }
            let uid_hit = matches!((&b.uid, uid), (Some(a), Some(b2)) if a == b2);
            let ip_hit = matches!((&b.ip, ip), (Some(a), Some(i)) if *a == i.to_string());
            if uid_hit || ip_hit {
                return Some(b);
            }
        }
        None
    }

    /// Attaches a bulk connection to the session that received `token`. The connection
    /// must come from the same identity (`uid` from its handshake) as the session, so a
    /// token that leaked is useless to anyone else.
    pub fn attach_bulk(&self, token: &[u8; 32], uid: &str, tx: mpsc::Sender<BulkS2C>, frames: Arc<AtomicUsize>) -> Option<ClientId> {
        let mut st = self.state.lock();
        let mut found = None;
        // Compare every token in constant time (no early exit that leaks timing).
        for s in st.sessions.values() {
            if ct_eq(&s.bulk_token, token) {
                found = Some(s.info.id);
            }
        }
        let s = st.sessions.get_mut(&found?)?;
        if s.info.uid != uid {
            warn!("bulk connection for client {} came from another identity ({uid}); refused", s.info.id);
            return None;
        }
        s.bulk_tx = Some(tx);
        s.bulk_frames = frames;
        Some(s.info.id)
    }

    /// Detaches a bulk connection, unless the session already has a newer one.
    pub fn detach_bulk(&self, id: ClientId, tx: &mpsc::Sender<BulkS2C>) {
        let mut st = self.state.lock();
        if let Some(s) = st.sessions.get_mut(&id) {
            if s.bulk_tx.as_ref().map(|b| b.same_channel(tx)).unwrap_or(false) {
                s.bulk_tx = None;
            }
        }
    }

    // ------------------------------------------------------------------ disconnect

    /// Removes a session and tells everyone. Safe to call more than once.
    pub fn remove_session(&self, id: ClientId, reason: LeaveReason) {
        let mut st = self.state.lock();
        // Only people who could see them are told they left.
        let told: Vec<ClientId> = match st.sessions.get(&id) {
            Some(sub) => st.sessions.values().filter(|v| v.info.id != id && st.sees(v, sub)).map(|v| v.info.id).collect(),
            None => return,
        };
        let Some(s) = st.sessions.remove(&id) else { return };
        info!("client {} (id {id}) left: {reason:?}", s.info.nickname);
        for other in st.sessions.values_mut() {
            other.viewers.remove(&id);
        }
        // Someone leaving while invisible keeps the "last seen" time they were last visible.
        if !s.hidden() {
            self.touch_last_seen(&s.info.uid);
        }
        for v in told {
            st.send(v, S2C::ClientLeft { id, reason: reason.clone() });
        }
        self.cleanup_temporary(&mut st, s.info.channel);
    }

    /// Sends a final message, closes the connection and removes the session.
    pub fn kick_session(&self, id: ClientId, reason: LeaveReason) {
        {
            let st = self.state.lock();
            if let Some(s) = st.sessions.get(&id) {
                s.tx.send(Out::Msg(S2C::Kicked(reason.clone())));
                s.tx.send(Out::Close);
                s.close.notify_one();
            }
        }
        self.remove_session(id, reason);
    }

    /// Removes an empty temporary channel, then its parent if that became empty too.
    fn cleanup_temporary(&self, st: &mut State, ch: ChannelId) {
        let mut next = Some(ch);
        while let Some(ch) = next.take() {
            let Some(c) = st.channels.get(&ch) else { return };
            if c.info.kind != ChannelKind::Temporary || st.clients_in(ch) > 0 || !st.children(ch).is_empty() {
                return;
            }
            next = c.info.parent;
            st.channels.remove(&ch);
            let _ = self.store.delete_channel(ch);
            crate::files::delete_channel_files(&self.data_dir, ch);
            st.broadcast(S2C::ChannelDeleted(ch));
        }
    }

    // ------------------------------------------------------------------ dispatch

    pub fn handle(&self, me: ClientId, msg: C2S) {
        let mut st = self.state.lock();
        if !st.sessions.contains_key(&me) {
            return;
        }
        let mut kicks = Vec::new();
        if let Err(e) = self.dispatch(&mut st, me, msg, &mut kicks) {
            st.send(me, S2C::Error(e));
        }
        drop(st);
        for (id, reason) in kicks {
            self.kick_session(id, reason);
        }
    }

    fn dispatch(&self, st: &mut State, me: ClientId, msg: C2S, kicks: &mut Vec<(ClientId, LeaveReason)>) -> Res {
        // "Channel 0" means "not in a channel": it is not a real channel (and 0 is the
        // server chat's storage slot), so nothing may be read from or written to it.
        if names_no_channel(&msg) {
            return Err("You are not in a channel: join one first".into());
        }
        // Invisible people look offline: acting on them must look the same as on nobody.
        if let Some(t) = names_client(&msg) {
            if let (Some(a), Some(b)) = (st.sessions.get(&me), st.sessions.get(&t)) {
                if !st.sees(a, b) {
                    return Err("Client not found".into());
                }
            }
        }
        match msg {
            C2S::Hello { .. } | C2S::AttachBulk(_) => Err("Unexpected message".into()),
            C2S::Ping(t) => {
                st.send(me, S2C::Pong(t));
                Ok(())
            }
            C2S::Disconnect(_) => Ok(()),
            C2S::UpdateSelf(u) => self.update_self(st, me, u),
            // Channel "0": leave your channel and stay on the server.
            C2S::SwitchChannel { channel: NO_CHANNEL, .. } => {
                self.move_client(st, me, NO_CHANNEL, None);
                Ok(())
            }
            C2S::SwitchChannel { channel, password } => {
                self.check_join(st, me, channel, password.as_deref())?;
                self.move_client(st, me, channel, None);
                Ok(())
            }
            C2S::CreateChannel(edit) => self.create_channel(st, me, edit),
            C2S::EditChannel { id, edit } => self.edit_channel(st, me, id, edit),
            C2S::DeleteChannel { id, force } => self.delete_channel(st, me, id, force),
            C2S::SendText { target, text } => self.send_text(st, me, target, text),
            C2S::FetchHistory { target, before, limit } => self.fetch_history(st, me, target, before, limit),
            C2S::DeleteMessage { target, id } => self.delete_message(st, me, target, id),
            C2S::Poke { target, message } => {
                st.spend_chat(me)?;
                let s = st.session(me)?;
                self.need(st.server_perms(s), perm::CLIENT_POKE)?;
                let name = s.info.nickname.clone();
                st.session(target)?;
                st.send(target, S2C::Poked { from: me, from_name: name, message: clean_text(&message, 500) });
                Ok(())
            }
            C2S::Kick { target, from_server, reason } => {
                let s = st.session(me)?;
                let t = st.session(target)?;
                let by = s.info.nickname.clone();
                self.check_outranks(st, s, t)?;
                let reason = clean_line(&reason, 200);
                if from_server {
                    self.need(st.server_perms(s), perm::CLIENT_KICK_SERVER)?;
                    kicks.push((target, LeaveReason::Kicked { by, reason }));
                } else {
                    if t.info.channel == NO_CHANNEL {
                        return Err("This person is not in a channel".into());
                    }
                    self.need(self.perms_in(st, s, t.info.channel), perm::CLIENT_KICK_CHANNEL)?;
                    let def = st.default_channel();
                    if t.info.channel == def {
                        return Err("Client is already in the default channel".into());
                    }
                    st.send(target, S2C::Notice(format!("You were kicked from the channel by {by}: {reason}")));
                    self.move_client(st, target, def, Some(by));
                }
                Ok(())
            }
            C2S::MoveClient { target, channel } => {
                let s = st.session(me)?;
                self.need(st.server_perms(s), perm::CLIENT_MOVE)?;
                let t = st.session(target)?;
                self.check_outranks(st, s, t)?;
                let c = st.channels.get(&channel).ok_or("Channel not found")?;
                if voxa_proto::is_spacer(&c.info.name) {
                    return Err("Nobody can be moved into a separator".into());
                }
                let by = s.info.nickname.clone();
                self.move_client(st, target, channel, Some(by));
                Ok(())
            }
            C2S::Ban { target, duration_secs, reason } => self.ban(st, me, target, duration_secs, reason, kicks),
            C2S::BanList => {
                self.need(st.server_perms(st.session(me)?), perm::BAN_LIST)?;
                self.send_ban_list(st, me)
            }
            C2S::Unban(id) => {
                self.need(st.server_perms(st.session(me)?), perm::BAN_LIST)?;
                db(self.store.remove_ban(id))?;
                self.send_ban_list(st, me)
            }
            C2S::SetTalker { target, talker } => {
                let s = st.session(me)?;
                let t = st.session(target)?;
                self.need(self.perms_in(st, s, t.info.channel), perm::TALK_POWER_GRANT)?;
                let t = st.sessions.get_mut(&target).unwrap();
                t.info.is_talker = talker;
                t.info.talk_request = None;
                st.broadcast_client(target);
                Ok(())
            }
            C2S::EditClientDescription { target, description } => {
                let s = st.session(me)?;
                if target != me {
                    self.need(st.server_perms(s), perm::CLIENT_DESCRIPTION_EDIT)?;
                }
                let t = st.sessions.get_mut(&target).ok_or("Client not found")?;
                t.info.description = clean_text(&description, 200);
                let uid = t.info.uid.clone();
                let d = t.info.description.clone();
                if let Ok(Some(mut r)) = self.store.get_client(&uid) {
                    r.description = d;
                    let _ = self.store.save_client(&r);
                }
                st.broadcast_client(target);
                Ok(())
            }
            C2S::EditServer(e) => self.edit_server(st, me, e),
            C2S::CreateGroup { kind, name, copy_from } => {
                let mine = st.server_perms(st.session(me)?);
                self.need(mine, perm::GROUP_MANAGE)?;
                if copy_from.and_then(|g| st.groups.get(&g)).map(|g| g.perms & !mine != 0).unwrap_or(false) {
                    return Err("You cannot copy a group with permissions you do not have".into());
                }
                let name = clean_line(&name, 40);
                if name.is_empty() {
                    return Err("Group name required".into());
                }
                // Without a template, new server groups start like the default group so their
                // members can still join channels and chat.
                let template = copy_from.or((kind == GroupKind::Server).then_some(st.settings.default_server_group));
                let (perms, tp, jp) =
                    template.and_then(|g| st.groups.get(&g)).map(|g| (g.perms, g.talk_power, g.join_power)).unwrap_or((0, 0, 0));
                let g = GroupInfo {
                    id: db(self.store.next_id("next_group"))?,
                    kind,
                    name,
                    perms,
                    talk_power: tp,
                    join_power: jp,
                    sort: 50,
                    builtin: false,
                    show_name: false,
                };
                db(self.store.save_group(&g))?;
                st.groups.insert(g.id, g);
                st.broadcast(S2C::GroupsUpdated(st.groups.values().cloned().collect()));
                Ok(())
            }
            C2S::EditGroup(mut g) => {
                let s = st.session(me)?;
                let mine = st.server_perms(s);
                self.need(mine, perm::GROUP_MANAGE)?;
                let old = st.groups.get(&g.id).ok_or("Group not found")?.clone();
                g.kind = old.kind;
                g.builtin = old.builtin;
                g.name = clean_line(&g.name, 40);
                if g.name.is_empty() {
                    return Err("Group name required".into());
                }
                if old.id == GROUP_ADMIN {
                    g.perms = perm::ALL;
                }
                // Groups ranked above me (holding permissions I lack) are not mine to change,
                // and nobody can hand out permissions they do not have themselves.
                if old.perms & !mine != 0 {
                    return Err("You cannot edit a group that has permissions you do not have".into());
                }
                if g.perms & !mine != 0 {
                    return Err("You cannot grant permissions you do not have".into());
                }
                // Talk / join power can be raised only up to my own.
                if mine != perm::ALL {
                    let my_talk = st.talk_power(s);
                    let my_join = st.join_power(&s.info.server_groups, s.info.channel_group);
                    if (g.talk_power > old.talk_power && g.talk_power > my_talk) || (g.join_power > old.join_power && g.join_power > my_join) {
                        return Err("You cannot give a group more talk or join power than you have".into());
                    }
                }
                db(self.store.save_group(&g))?;
                st.groups.insert(g.id, g);
                st.broadcast(S2C::GroupsUpdated(st.groups.values().cloned().collect()));
                Ok(())
            }
            C2S::DeleteGroup(id) => self.delete_group(st, me, id),
            C2S::GroupMembers(g) => {
                let s = st.session(me)?;
                let p = st.server_perms(s);
                if p & (perm::GROUP_MANAGE | perm::GROUP_ASSIGN_SERVER) == 0 {
                    return Err("Insufficient permissions".into());
                }
                let group = st.groups.get(&g).ok_or("Group not found")?;
                // Invisible people count as offline, unless the asker may see them.
                let online: HashSet<&str> = st.sessions.values().filter(|x| st.sees(s, x)).map(|x| x.info.uid.as_str()).collect();
                let members = match group.kind {
                    GroupKind::Server => db(self.store.all_clients())?
                        .into_iter()
                        .filter(|c| c.server_groups.contains(&g))
                        .map(|c| GroupMember { online: online.contains(c.uid.as_str()), uid: c.uid, name: c.nickname })
                        .collect(),
                    GroupKind::Channel => {
                        let mut v = Vec::new();
                        for (uid, ch) in db(self.store.channel_group_members(g))? {
                            let nick = self.store.get_client(&uid).ok().flatten().map(|c| c.nickname).unwrap_or_default();
                            let cname = st.channels.get(&ch).map(|c| c.info.name.clone()).unwrap_or_default();
                            v.push(GroupMember { online: online.contains(uid.as_str()), uid, name: format!("{nick} ({cname})") });
                        }
                        v
                    }
                };
                st.send(me, S2C::GroupMembers { group: g, members });
                Ok(())
            }
            C2S::SetServerGroup { uid, group, add } => self.set_server_group(st, me, &uid, group, add),
            C2S::SetChannelGroup { uid, channel, group } => self.set_channel_group(st, me, &uid, channel, group),
            C2S::CreatePrivilegeKey { group, channel, description } => {
                let s = st.session(me)?;
                let mine = st.server_perms(s);
                self.need(mine, perm::PRIVILEGE_KEY_CREATE)?;
                let g = st.groups.get(&group).ok_or("Group not found")?;
                if g.perms & !mine != 0 {
                    return Err("You cannot create keys for groups with more permissions than you".into());
                }
                if g.kind == GroupKind::Channel && channel.map(|c| !st.channels.contains_key(&c)).unwrap_or(true) {
                    return Err("A channel is required for channel group keys".into());
                }
                let key = new_privilege_key();
                db(self.store.add_privkey(
                    &key,
                    &PrivKey {
                        group,
                        channel: if g.kind == GroupKind::Channel { channel } else { None },
                        description: clean_line(&description, 100),
                        created: now_unix(),
                    },
                ))?;
                st.send(me, S2C::PrivilegeKeyCreated(key));
                Ok(())
            }
            C2S::UsePrivilegeKey(key) => {
                let pk = db(self.store.take_privkey(key.trim()))?.ok_or("Invalid privilege key")?;
                let g = st.groups.get(&pk.group).cloned().ok_or("The group of this key no longer exists")?;
                let uid = st.session(me)?.info.uid.clone();
                match (g.kind, pk.channel) {
                    (GroupKind::Server, _) => self.apply_server_group(st, &uid, g.id, true)?,
                    (GroupKind::Channel, Some(ch)) => self.apply_channel_group(st, &uid, ch, g.id)?,
                    _ => return Err("Invalid privilege key".into()),
                }
                st.send(me, S2C::Notice(format!("Privilege key used. You are now a member of \"{}\".", g.name)));
                if g.id == GROUP_ADMIN {
                    let _ = std::fs::remove_file(self.data_dir.join("privilege_key.txt"));
                }
                Ok(())
            }
            C2S::FileList { channel, path } => {
                let s = st.session(me)?;
                self.need(self.perms_in(st, s, channel), perm::FILE_BROWSE)?;
                if !st.channels.contains_key(&channel) {
                    return Err("Channel not found".into());
                }
                let entries = crate::files::list(&self.data_dir, channel, &path)?;
                st.send(me, S2C::FileList { channel, path, entries });
                Ok(())
            }
            C2S::FileDelete { channel, path } => {
                let s = st.session(me)?;
                self.need(self.perms_in(st, s, channel), perm::FILE_DELETE)?;
                crate::files::delete(&self.data_dir, channel, &path)?;
                Ok(())
            }
            C2S::FileMkdir { channel, path } => {
                let s = st.session(me)?;
                self.need(self.perms_in(st, s, channel), perm::FILE_UPLOAD)?;
                crate::files::mkdir(&self.data_dir, channel, &path)?;
                Ok(())
            }
            C2S::StartStream(mut info) => {
                let s = st.session(me)?;
                if s.info.channel == NO_CHANNEL {
                    return Err("Join a channel to share your screen".into());
                }
                self.need(self.perms_in(st, s, s.info.channel), perm::SCREEN_SHARE)?;
                info.title = clean_line(&info.title, 80);
                info.fps = info.fps.clamp(1, 60);
                let s = st.sessions.get_mut(&me).unwrap();
                s.info.streaming = Some(info);
                s.viewers.clear();
                st.broadcast_client(me);
                Ok(())
            }
            C2S::StopStream => {
                let s = st.sessions.get_mut(&me).ok_or("Client not found")?;
                s.info.streaming = None;
                s.viewers.clear();
                st.broadcast_client(me);
                Ok(())
            }
            C2S::StartCamera(info) => {
                let s = st.session(me)?;
                if s.web {
                    return Err("The web page cannot turn on a camera".into());
                }
                if s.info.channel == NO_CHANNEL {
                    return Err("Join a channel to turn on your camera".into());
                }
                self.need(self.perms_in(st, s, s.info.channel), perm::SCREEN_SHARE)?;
                if !info.valid() {
                    return Err("Camera pictures can be at most 1280 x 720 (or 720 x 1280) at 30 fps".into());
                }
                st.sessions.get_mut(&me).unwrap().camera = Some(info);
                info!("{} turned their camera on ({}x{} at {} fps)", st.sessions[&me].info.nickname, info.width, info.height, info.fps);
                self.camera_changed(st, me);
                Ok(())
            }
            C2S::StopCamera => {
                if st.sessions.get_mut(&me).and_then(|s| s.camera.take()).is_some() {
                    self.camera_changed(st, me);
                }
                Ok(())
            }
            C2S::GetCalendar(ch) if ch == SERVER_CALENDAR => {
                let s = st.session(me)?;
                if !s.client_at_least("1.31.0") {
                    return Err("Update Voxa to see the server calendar".into());
                }
                self.need(st.server_perms(s), perm::CALENDAR_VIEW)?;
                st.sessions.get_mut(&me).unwrap().calendar_subs.insert(ch);
                let msg = self.server_calendar_msg(st)?;
                st.send(me, msg);
                Ok(())
            }
            C2S::GetCalendar(ch) => {
                let s = st.session(me)?;
                if !st.channels.contains_key(&ch) {
                    return Err("Channel not found".into());
                }
                self.need(self.perms_in(st, s, ch), perm::CALENDAR_VIEW)?;
                // Not (or no longer) in the channel: nothing to send, and no error either (a
                // request made just before switching channels arrives after the switch).
                if self.calendar_reach(st, s, ch).is_err() {
                    return Ok(());
                }
                let cal = db(self.store.calendar(ch))?;
                st.sessions.get_mut(&me).unwrap().calendar_subs.insert(ch);
                st.send(me, S2C::Calendar { channel: ch, calendars: cal.calendars, events: cal.events });
                Ok(())
            }
            C2S::CreateCalendar { channel, name } => self.calendar_edit(st, me, channel, perm::CALENDAR_MANAGE, |cal, _, _| {
                let name = clean_line(&name, 40);
                if name.is_empty() {
                    return Err("A calendar needs a name".into());
                }
                if cal.calendars.len() >= MAX_CALENDARS {
                    return Err(format!("A channel can have at most {MAX_CALENDARS} calendars"));
                }
                cal.next_id += 1;
                cal.calendars.push(CalendarInfo { id: cal.next_id, name });
                Ok(())
            }),
            C2S::RenameCalendar { channel, id, name } => self.calendar_edit(st, me, channel, perm::CALENDAR_MANAGE, |cal, _, _| {
                let name = clean_line(&name, 40);
                if name.is_empty() {
                    return Err("A calendar needs a name".into());
                }
                cal.calendars.iter_mut().find(|c| c.id == id).ok_or("Calendar not found")?.name = name;
                Ok(())
            }),
            C2S::DeleteCalendar { channel, id } => self.calendar_edit(st, me, channel, perm::CALENDAR_MANAGE, |cal, _, _| {
                let before = cal.calendars.len();
                cal.calendars.retain(|c| c.id != id);
                if cal.calendars.len() == before {
                    return Err("Calendar not found".into());
                }
                cal.events.retain(|e| e.calendar != id);
                Ok(())
            }),
            C2S::SaveEvent { channel, event } => self.calendar_edit(st, me, channel, perm::CALENDAR_EVENTS, |cal, uid, name| save_event(cal, event, uid, name).map(|_| ())),
            C2S::SaveServerEvent { event, meet } => {
                // The channel to meet in must exist (and not be a separator).
                if let Some(m) = meet {
                    match st.channels.get(&m) {
                        Some(c) if m != NO_CHANNEL && voxa_proto::spacer(&c.info.name).is_none() => {}
                        _ => return Err("That channel does not exist".into()),
                    }
                }
                let store = &self.store;
                self.calendar_edit(st, me, SERVER_CALENDAR, perm::CALENDAR_EVENTS, move |cal, uid, name| {
                    let id = save_event(cal, event, uid, name)?;
                    let mut places = db(store.calendar_meet())?;
                    // Also drops places of events that are gone (deleted or long over).
                    places.retain(|(e, _)| *e != id && cal.events.iter().any(|x| x.id == *e));
                    if let Some(m) = meet {
                        places.push((id, m));
                    }
                    db(store.save_calendar_meet(&places))
                })
            }
            C2S::DeleteEvent { channel, id } => self.calendar_edit(st, me, channel, perm::CALENDAR_EVENTS, |cal, _, _| {
                let before = cal.events.len();
                cal.events.retain(|e| e.id != id);
                if cal.events.len() == before {
                    return Err("Event not found".into());
                }
                Ok(())
            }),
            C2S::AnswerEvent { channel, id, answer } => self.calendar_edit(st, me, channel, perm::CALENDAR_RSVP, |cal, uid, name| {
                let e = cal.events.iter_mut().find(|e| e.id == id).ok_or("Event not found")?;
                if e.start + e.duration_min as u64 * 60 < now_unix() {
                    return Err("This event is over".into());
                }
                e.rsvps.retain(|r| r.uid != uid);
                if let Some(answer) = answer {
                    if answer == RsvpAnswer::Going && e.max_people.is_some_and(|m| e.going() >= m as usize) {
                        return Err("This event is full".into());
                    }
                    e.rsvps.push(Rsvp { uid: uid.to_string(), name: name.to_string(), answer });
                }
                Ok(())
            }),
            C2S::ForgetClient(uid) => self.forget_client(st, me, uid),
            C2S::ClearChat(target) => {
                let s = st.session(me)?;
                let ch = match target {
                    TextTarget::Server => {
                        self.need(st.server_perms(s), perm::CHAT_CLEAR)?;
                        SERVER_CHAT_CHANNEL
                    }
                    TextTarget::Channel(ch) => {
                        if !st.channels.contains_key(&ch) {
                            return Err("Channel not found".into());
                        }
                        self.need(self.perms_in(st, s, ch), perm::CHAT_CLEAR)?;
                        ch
                    }
                    TextTarget::Private(_) | TextTarget::Group(_) => return Err("Private conversations are not stored".into()),
                };
                let by = s.info.nickname.clone();
                let n = db(self.store.clear_chat(ch))?;
                let _ = self.store.clear_pins(ch);
                let imgs = self.store.delete_images_where(|m| m.ch == Some(ch)).unwrap_or(0);
                info!("{by} cleared the chat history of {target:?} ({n} messages, {imgs} images)");
                let ids: Vec<ClientId> = st
                    .sessions
                    .values()
                    .filter(|x| matches!(target, TextTarget::Server) || x.info.channel == ch)
                    .map(|x| x.info.id)
                    .collect();
                for id in ids {
                    st.send_if_supported(id, "1.4.0", S2C::ChatCleared { target, by: by.clone() });
                }
                Ok(())
            }
            C2S::GetClientPerms(uid) => {
                self.need(st.server_perms(st.session(me)?), perm::GROUP_MANAGE)?;
                let perms = db(self.store.client_perms(&uid))?;
                st.send(me, S2C::ClientPerms { uid, perms });
                Ok(())
            }
            C2S::SetClientPerms { uid, perms } => {
                let s = st.session(me)?;
                let mine = st.server_perms(s);
                self.need(mine, perm::GROUP_MANAGE)?;
                let perms = perms & perm::ALL;
                let old = db(self.store.client_perms(&uid))?;
                // Nobody can hand out (or take away) permissions they do not have themselves.
                if (perms ^ old) & !mine != 0 {
                    return Err("You cannot change permissions you do not have".into());
                }
                if db(self.store.get_client(&uid))?.is_none() {
                    return Err("Unknown client".into());
                }
                db(self.store.set_client_perms(&uid, perms))?;
                let by = s.info.nickname.clone();
                let affected: Vec<ClientId> = st.sessions.values().filter(|x| x.info.uid == uid).map(|x| x.info.id).collect();
                for id in affected {
                    st.sessions.get_mut(&id).unwrap().extra_perms = perms;
                    st.send_if_supported(id, "1.4.0", S2C::YourPerms(perms));
                    st.send(id, S2C::Notice(format!("{by} changed your permissions")));
                }
                info!("{by} set personal permissions of {uid} to {perms:#x}");
                st.send(me, S2C::ClientPerms { uid, perms });
                Ok(())
            }
            C2S::CloneChannel { source, name, with_children } => self.clone_channel(st, me, source, name, with_children),
            C2S::PinMessage { target, id, pin } => self.pin_message(st, me, target, id, pin),
            C2S::GetPins(target) => self.get_pins(st, me, target),
            C2S::GetPeople => {
                let s = st.session(me)?;
                if s.client_at_least("1.18.0") {
                    let clients = db(self.store.all_clients())?;
                    let avatars: Vec<AvatarMeta> =
                        if s.client_at_least("1.20.0") { clients.iter().filter_map(|c| self.avatar_meta(&c.uid)).collect() } else { Vec::new() };
                    let people = clients.into_iter().map(|c| Person { uid: c.uid, nickname: c.nickname, last_seen: c.last_seen }).collect();
                    st.send(me, S2C::People(people));
                    if !avatars.is_empty() {
                        st.send(me, S2C::AvatarInfo(avatars));
                    }
                }
                Ok(())
            }
            C2S::SendOffline { to_uid, text } => self.send_offline(st, me, to_uid, text),
            C2S::EditMessage { target, id, text } => self.edit_message(st, me, target, id, text),
            C2S::SetAvatar { color, image } => self.set_avatar(st, me, color, image),
            C2S::GetAvatar(uid) => {
                if uid.len() > 64 {
                    return Err("Unknown user".into());
                }
                if let Some(a) = db(self.store.avatar(&uid))? {
                    st.send_if_supported(me, "1.20.0", S2C::AvatarInfo(vec![AvatarMeta { uid: uid.clone(), color: a.color, image: a.hash }]));
                    if !a.image.is_empty() {
                        st.send_if_supported(me, "1.20.0", S2C::AvatarImage { uid, image: a.hash, data: a.image });
                    }
                }
                Ok(())
            }
            C2S::ClearAvatar(uid) => self.clear_avatar(st, me, uid),
            C2S::SetServerOptions(o) => {
                let s = st.session(me)?;
                self.need(st.server_perms(s), perm::SERVER_EDIT)?;
                let o = ServerOptions { auto_away_minutes: o.auto_away_minutes.min(MAX_AUTO_AWAY_MINUTES) };
                db(self.store.save_server_options(&o))?;
                info!("{} set auto-away to {} min", s.info.nickname, o.auto_away_minutes);
                st.options = o;
                let ids: Vec<ClientId> = st.sessions.keys().copied().collect();
                for c in ids {
                    st.send_if_supported(c, "1.21.0", S2C::ServerOptions(o));
                }
                Ok(())
            }
            C2S::Hardware(tokens) => self.set_hardware(st, me, tokens, kicks),
            C2S::SetPresence(p) => self.set_presence(st, me, p),
            C2S::CreateGroupChat { name, members } => self.create_group_chat(st, me, name, members),
            C2S::InviteToGroupChat { id, uids } => self.invite_to_group_chat(st, me, id, uids),
            C2S::RenameGroupChat { id, name } => self.rename_group_chat(st, me, id, name),
            C2S::LeaveGroupChat(id) => self.leave_group_chat(st, me, id),
            C2S::BanWith { target, uid, ip, hardware, duration_secs, reason } => self.ban_with(st, me, target, [uid, ip, hardware], duration_secs, reason, kicks),
            C2S::GetFileLimits => {
                st.send(me, S2C::FileLimits(st.file_limits));
                Ok(())
            }
            C2S::SetFileLimits(l) => {
                self.need(st.server_perms(st.session(me)?), perm::SERVER_EDIT)?;
                db(self.store.save_file_limits(&l))?;
                st.file_limits = l;
                info!("file limits changed: max upload {} MB, channel quota {} MB (0 = unlimited)", l.max_upload_mb, l.channel_quota_mb);
                st.send(me, S2C::FileLimits(l));
                Ok(())
            }
            C2S::WatchStream { streamer, watch } => {
                let my_channel = st.session(me)?.info.channel;
                let t = st.sessions.get_mut(&streamer).ok_or("Client not found")?;
                if watch {
                    if t.info.streaming.is_none() {
                        return Err("This client is not sharing their screen".into());
                    }
                    if t.info.channel != my_channel || my_channel == NO_CHANNEL {
                        return Err("You can only watch streams in your own channel".into());
                    }
                    t.viewers.insert(me);
                    if let Some(b) = &t.bulk_tx {
                        let _ = b.try_send(BulkS2C::KeyframeRequested);
                    }
                } else {
                    t.viewers.remove(&me);
                }
                Ok(())
            }
        }
    }

    /// Prevents lower ranked clients from acting on server administrators.
    /// Kicks, bans and moves only work on people who rank at most as high as the actor:
    /// nobody with server-wide permissions the actor lacks (e.g. a moderator cannot ban
    /// an administrator, even with "edit server" rights).
    fn check_outranks(&self, st: &State, actor: &Session, target: &Session) -> Res {
        Self::outranks(st.server_perms(actor), st.server_perms(target))
    }

    fn outranks(actor: u64, target: u64) -> Res {
        if target & !actor != 0 {
            return Err("You cannot do that to someone with permissions you do not have".into());
        }
        Ok(())
    }

    // ------------------------------------------------------------------ self

    fn update_self(&self, st: &mut State, me: ClientId, u: SelfUpdate) -> Res {
        if let Some(n) = &u.nickname {
            let n = sanitize_nick(n);
            if n.is_empty() {
                return Err("Invalid nickname".into());
            }
            let taken = st.sessions.values().any(|s| s.info.id != me && s.info.nickname.eq_ignore_ascii_case(&n));
            if taken {
                return Err("Nickname is already in use".into());
            }
        }
        if u.commander == Some(true) {
            let s = st.session(me)?;
            self.need(self.perms_in(st, s, s.info.channel), perm::CHANNEL_COMMANDER)?;
        }
        let s = st.sessions.get_mut(&me).ok_or("Client not found")?;
        if let Some(n) = u.nickname {
            s.info.nickname = sanitize_nick(&n);
            if let Ok(Some(mut r)) = self.store.get_client(&s.info.uid) {
                r.nickname = s.info.nickname.clone();
                let _ = self.store.save_client(&r);
            }
        }
        if let Some(d) = u.description {
            s.info.description = clean_text(&d, 200);
        }
        if let Some(a) = u.away {
            s.info.away = a.map(|m| clean_line(&m, 80));
        }
        if let Some(m) = u.input_muted {
            s.info.input_muted = m;
        }
        if let Some(m) = u.output_muted {
            s.info.output_muted = m;
        }
        if let Some(r) = u.talk_request {
            s.info.talk_request = r.map(|m| clean_line(&m, 80));
        }
        if let Some(c) = u.commander {
            s.info.commander = c;
        }
        st.broadcast_client(me);
        Ok(())
    }

    // ------------------------------------------------------------------ channels

    fn check_join(&self, st: &State, me: ClientId, ch: ChannelId, password: Option<&str>) -> Res {
        let s = st.session(me)?;
        let c = st.channels.get(&ch).ok_or("Channel not found")?;
        if s.info.channel == ch {
            return Ok(());
        }
        if voxa_proto::is_spacer(&c.info.name) {
            return Err("This is a separator, not a channel: nobody can join it".into());
        }
        let p = self.perms_in(st, s, ch);
        if p & perm::CHANNEL_JOIN == 0 {
            return Err("Your group is not allowed to join channels".into());
        }
        if c.info.needed_join_power > 0 && p & perm::CHANNEL_JOIN_IGNORE_POWER == 0 {
            let cg = self.channel_group_for(st, &s.info.uid, ch);
            let have = st.join_power(&s.info.server_groups, cg);
            if have < c.info.needed_join_power {
                return Err(format!(
                    "Your group cannot join \"{}\" (needs join power {}, you have {have})",
                    c.info.name, c.info.needed_join_power
                ));
            }
        }
        if let Some(h) = &c.password_hash {
            if p & perm::CHANNEL_JOIN_IGNORE_PASSWORD == 0 && !password.map(|pw| self.pw_verified(pw, h)).unwrap_or(false) {
                return Err("Invalid channel password".into());
            }
        }
        if let Some(max) = c.info.max_clients {
            if st.clients_in(ch) as u32 >= max && p & perm::CHANNEL_JOIN_IGNORE_MAXCLIENTS == 0 {
                return Err("Channel is full".into());
            }
        }
        Ok(())
    }

    pub fn move_client(&self, st: &mut State, id: ClientId, ch: ChannelId, by: Option<String>) {
        let Some(s) = st.sessions.get(&id) else { return };
        let old = s.info.channel;
        if old == ch {
            return;
        }
        let uid = s.info.uid.clone();
        let was_hidden = s.hidden();
        let cg = self.channel_group_for(st, &uid, ch);
        let s = st.sessions.get_mut(&id).unwrap();
        if ch == NO_CHANNEL {
            // Nothing to share outside a channel.
            s.info.streaming = None;
        }
        // A camera is for the channel you are in: it goes off when you leave it.
        let camera_off = s.camera.take().is_some();
        s.info.channel = ch;
        s.info.channel_group = cg;
        s.info.is_talker = false;
        s.info.talk_request = None;
        s.viewers.clear();
        for other in st.sessions.values_mut() {
            if other.info.channel != ch {
                other.viewers.remove(&id);
            }
        }
        // Invisible people become visible in a channel and vanish again when they leave it.
        let now_hidden = st.sessions[&id].hidden();
        if was_hidden == now_hidden {
            st.broadcast_about(id, S2C::ClientMoved { id, channel: ch, by });
        } else {
            if now_hidden {
                self.touch_last_seen(&uid);
            }
            st.announce_visibility(id, was_hidden, self.avatar_meta(&uid));
            // Administrators (and the person) saw them all along.
            for v in st.sessions.values().filter(|v| v.info.id == id || st.server_perms(v) & perm::SERVER_EDIT != 0) {
                v.tx.send(Out::Msg(S2C::ClientMoved { id, channel: ch, by: by.clone() }));
            }
        }
        st.broadcast_client(id);
        if camera_off {
            self.camera_changed(st, id);
        }
        self.cleanup_temporary(st, old);
    }

    fn validate_channel_edit(&self, st: &State, id: Option<ChannelId>, e: &mut ChannelEdit) -> Res {
        e.name = clean_line(&e.name, 40);
        if e.name.is_empty() {
            return Err("Channel name required".into());
        }
        if e.name.contains('/') {
            return Err("Channel names cannot contain '/'".into());
        }
        e.topic = clean_line(&e.topic, 255);
        e.description = clean_text(&e.description, 8192);
        // Quality 0..=10, plus the codec bits (Opus Music, Lossless).
        e.codec_quality = voxa_proto::pack_codec(voxa_proto::codec_of(e.codec_quality), voxa_proto::quality_of(e.codec_quality));
        if let Some(p) = e.parent {
            if !st.channels.contains_key(&p) {
                return Err("Parent channel not found".into());
            }
            if let Some(id) = id {
                if st.subtree(id).contains(&p) {
                    return Err("A channel cannot be moved below itself".into());
                }
            }
            let height = id.map(|id| st.height(id)).unwrap_or(1);
            if st.depth(p) + height > MAX_CHANNEL_DEPTH {
                return Err(format!("Channels can be nested at most {MAX_CHANNEL_DEPTH} levels deep"));
            }
        }
        let dup = st
            .channels
            .values()
            .any(|c| Some(c.info.id) != id && c.info.parent == e.parent && c.info.name.eq_ignore_ascii_case(&e.name));
        if dup {
            return Err("A channel with this name already exists".into());
        }
        if voxa_proto::is_spacer(&e.name) {
            // Separators are only decoration: never default, never temporary (a temporary
            // channel only lives while someone is inside), no password.
            if e.is_default {
                return Err("A separator cannot be the default channel".into());
            }
            if e.kind == ChannelKind::Temporary {
                return Err("Separators must be permanent or semi-permanent".into());
            }
            e.password = PasswordChange::Clear;
            if let Some(id) = id {
                if st.clients_in(id) > 0 {
                    return Err("Move everyone out of the channel before turning it into a separator".into());
                }
            }
        }
        if e.is_default && e.kind != ChannelKind::Permanent {
            return Err("The default channel must be permanent".into());
        }
        if e.is_default && e.password != PasswordChange::Clear && matches!(e.password, PasswordChange::Set(_)) {
            return Err("The default channel cannot have a password".into());
        }
        Ok(())
    }

    fn create_perm_for(kind: ChannelKind) -> u64 {
        match kind {
            ChannelKind::Permanent => perm::CHANNEL_CREATE_PERMANENT,
            ChannelKind::SemiPermanent => perm::CHANNEL_CREATE_SEMI_PERMANENT,
            ChannelKind::Temporary => perm::CHANNEL_CREATE_TEMPORARY,
        }
    }

    fn persist_channel(&self, c: &ChannelRec) -> Res {
        if c.info.kind == ChannelKind::Permanent {
            db(self.store.save_channel(&StoredChannel { info: c.info.clone(), password_hash: c.password_hash.clone() }))
        } else {
            db(self.store.delete_channel_record_only(c.info.id))
        }
    }

    fn create_channel(&self, st: &mut State, me: ClientId, mut e: ChannelEdit) -> Res {
        let s = st.session(me)?;
        let p = st.server_perms(s);
        self.need(p, Self::create_perm_for(e.kind))?;
        if e.is_default {
            self.need(p, perm::SERVER_EDIT)?;
        }
        if st.channels.len() >= MAX_CHANNELS {
            return Err(format!("This server has the maximum of {MAX_CHANNELS} channels"));
        }
        self.validate_channel_edit(st, None, &mut e)?;
        let uid = s.info.uid.clone();
        let id = db(self.store.next_id("next_channel"))?;
        if e.sort == 0 {
            e.sort = st.channels.values().filter(|c| c.info.parent == e.parent).map(|c| c.info.sort).max().unwrap_or(0) + 1;
        }
        let password_hash = match &e.password {
            PasswordChange::Set(pw) if !pw.is_empty() => Some(self.pw_hash(pw)),
            _ => None,
        };
        if e.is_default {
            self.clear_default(st)?;
        }
        let info = ChannelInfo {
            id,
            parent: e.parent,
            sort: e.sort,
            name: e.name,
            topic: e.topic,
            description: e.description,
            has_password: password_hash.is_some(),
            max_clients: e.max_clients,
            kind: e.kind,
            is_default: e.is_default,
            codec_quality: e.codec_quality,
            needed_talk_power: e.needed_talk_power,
            needed_join_power: e.needed_join_power.max(0),
            voice_encrypted: e.voice_encrypted,
        };
        let rec = ChannelRec { info: info.clone(), password_hash };
        if rec.info.kind == ChannelKind::Permanent {
            self.persist_channel(&rec)?;
        }
        st.channels.insert(id, rec);
        st.broadcast(S2C::ChannelCreated(info));
        // The creator becomes channel admin and is moved into the new channel (not into a
        // separator, which nobody can join).
        let admin = st.settings.channel_admin_group;
        db(self.store.set_channel_group(&uid, id, Some(admin)))?;
        if !st.channels.get(&id).map(|c| voxa_proto::is_spacer(&c.info.name)).unwrap_or(false) {
            self.move_client(st, me, id, None);
        }
        Ok(())
    }

    /// Copies a channel under a new name, right below the original: all settings, the
    /// password and everyone's channel group (channel admins etc.). Optionally with all
    /// sub-channels (which keep their names). Chat history and files are not copied.
    fn clone_channel(&self, st: &mut State, me: ClientId, source: ChannelId, name: String, with_children: bool) -> Res {
        let src = st.channels.get(&source).ok_or("Channel not found")?;
        let s = st.session(me)?;
        let p = st.server_perms(s);
        let ids = if with_children { st.subtree(source) } else { vec![source] };
        for id in &ids {
            if let Some(c) = st.channels.get(id) {
                self.need(p, Self::create_perm_for(c.info.kind))?;
            }
        }
        let by = s.info.nickname.clone();
        let (parent, src_sort, kind, src_name) = (src.info.parent, src.info.sort, src.info.kind, src.info.name.clone());
        let mut e = ChannelEdit::from_info(&src.info);
        e.name = name;
        e.is_default = false;
        e.password = PasswordChange::Keep;
        if st.channels.len() + ids.len() > MAX_CHANNELS {
            return Err(format!("This server has the maximum of {MAX_CHANNELS} channels"));
        }
        let height = if with_children { st.height(source) } else { 1 };
        if parent.map(|p| st.depth(p)).unwrap_or(0) + height > MAX_CHANNEL_DEPTH {
            return Err(format!("Channels can be nested at most {MAX_CHANNEL_DEPTH} levels deep"));
        }
        self.validate_channel_edit(st, None, &mut e)?;
        // Make room directly below the original.
        let later: Vec<ChannelId> = st.channels.values().filter(|c| c.info.parent == parent && c.info.sort > src_sort).map(|c| c.info.id).collect();
        for id in later {
            let c = st.channels.get_mut(&id).unwrap();
            c.info.sort += 1;
            let rec = ChannelRec { info: c.info.clone(), password_hash: c.password_hash.clone() };
            self.persist_channel(&rec)?;
            st.broadcast(S2C::ChannelEdited(rec.info));
        }
        let new_id = self.copy_channel(st, source, parent, e.name.clone(), src_sort + 1)?;
        let mut count = 1;
        if with_children {
            count += self.copy_children(st, source, new_id)?;
        }
        info!("{by} cloned channel \"{src_name}\" as \"{}\" ({count} channel(s))", e.name);
        // A temporary channel only lives while someone is in it.
        if kind == ChannelKind::Temporary {
            self.move_client(st, me, new_id, None);
        }
        // Copied temporary sub-channels are empty, so they go away again at once.
        for ch in st.subtree(new_id) {
            self.cleanup_temporary(st, ch);
        }
        Ok(())
    }

    fn copy_channel(&self, st: &mut State, source: ChannelId, parent: Option<ChannelId>, name: String, sort: i64) -> Res<ChannelId> {
        let src = st.channels.get(&source).ok_or("Channel not found")?;
        let id = db(self.store.next_id("next_channel"))?;
        let info = ChannelInfo { id, parent, sort, name, is_default: false, ..src.info.clone() };
        let rec = ChannelRec { info: info.clone(), password_hash: src.password_hash.clone() };
        if info.kind == ChannelKind::Permanent {
            self.persist_channel(&rec)?;
        }
        st.channels.insert(id, rec);
        st.broadcast(S2C::ChannelCreated(info));
        for (uid, g) in db(self.store.channel_group_assignments(source))? {
            db(self.store.set_channel_group(&uid, id, Some(g)))?;
        }
        Ok(id)
    }

    /// Copies the sub-channels of `from` below `to`, recursively. Returns how many.
    fn copy_children(&self, st: &mut State, from: ChannelId, to: ChannelId) -> Res<usize> {
        let mut kids: Vec<(i64, ChannelId, String)> =
            st.channels.values().filter(|c| c.info.parent == Some(from)).map(|c| (c.info.sort, c.info.id, c.info.name.clone())).collect();
        kids.sort();
        let mut n = 0;
        for (sort, kid, name) in kids {
            let copy = self.copy_channel(st, kid, Some(to), name, sort)?;
            n += 1 + self.copy_children(st, kid, copy)?;
        }
        Ok(n)
    }

    fn clear_default(&self, st: &mut State) -> Res {
        let ids: Vec<ChannelId> = st.channels.values().filter(|c| c.info.is_default).map(|c| c.info.id).collect();
        for id in ids {
            let c = st.channels.get_mut(&id).unwrap();
            c.info.is_default = false;
            let info = c.info.clone();
            let rec = ChannelRec { info: info.clone(), password_hash: c.password_hash.clone() };
            self.persist_channel(&rec)?;
            st.broadcast(S2C::ChannelEdited(info));
        }
        Ok(())
    }

    fn edit_channel(&self, st: &mut State, me: ClientId, id: ChannelId, mut e: ChannelEdit) -> Res {
        let s = st.session(me)?;
        let p = self.perms_in(st, s, id);
        let sp = st.server_perms(s);
        self.need(p, perm::CHANNEL_EDIT)?;
        let old = st.channels.get(&id).ok_or("Channel not found")?.info.clone();
        if e.kind != old.kind {
            self.need(sp, Self::create_perm_for(e.kind))?;
        }
        if e.is_default != old.is_default {
            self.need(sp, perm::SERVER_EDIT)?;
            if !e.is_default {
                return Err("Choose another default channel instead".into());
            }
        }
        if old.is_default && matches!(e.password, PasswordChange::Set(ref p) if !p.is_empty()) {
            return Err("The default channel cannot have a password".into());
        }
        self.validate_channel_edit(st, Some(id), &mut e)?;
        if e.is_default && !old.is_default {
            self.clear_default(st)?;
        }
        let c = st.channels.get_mut(&id).unwrap();
        match &e.password {
            PasswordChange::Keep => {}
            PasswordChange::Clear => c.password_hash = None,
            PasswordChange::Set(pw) if pw.is_empty() => c.password_hash = None,
            PasswordChange::Set(pw) => c.password_hash = Some(self.pw_hash(pw)),
        }
        c.info = ChannelInfo {
            id,
            parent: e.parent,
            sort: e.sort,
            name: e.name,
            topic: e.topic,
            description: e.description,
            has_password: c.password_hash.is_some(),
            max_clients: e.max_clients,
            kind: e.kind,
            is_default: e.is_default,
            codec_quality: e.codec_quality,
            needed_talk_power: e.needed_talk_power,
            needed_join_power: e.needed_join_power.max(0),
            voice_encrypted: e.voice_encrypted,
        };
        let info = c.info.clone();
        let rec = ChannelRec { info: info.clone(), password_hash: c.password_hash.clone() };
        self.persist_channel(&rec)?;
        st.broadcast(S2C::ChannelEdited(info));
        Ok(())
    }

    fn delete_channel(&self, st: &mut State, me: ClientId, id: ChannelId, force: bool) -> Res {
        let s = st.session(me)?;
        self.need(self.perms_in(st, s, id), perm::CHANNEL_DELETE)?;
        let c = st.channels.get(&id).ok_or("Channel not found")?;
        if c.info.is_default {
            return Err("The default channel cannot be deleted".into());
        }
        let tree = st.subtree(id);
        if tree.iter().any(|c| st.channels.get(c).map(|c| c.info.is_default).unwrap_or(false)) {
            return Err("This channel contains the default channel, which cannot be deleted".into());
        }
        let occupants: Vec<ClientId> =
            st.sessions.values().filter(|s| tree.contains(&s.info.channel)).map(|s| s.info.id).collect();
        if !occupants.is_empty() && !force {
            return Err("The channel is not empty".into());
        }
        let def = st.default_channel();
        let by = st.session(me)?.info.nickname.clone();
        for c in occupants {
            // Temporary-channel cleanup is suppressed by removing those channels below.
            let uid = st.sessions[&c].info.uid.clone();
            let cg = self.channel_group_for(st, &uid, def);
            let s = st.sessions.get_mut(&c).unwrap();
            s.info.channel = def;
            s.info.channel_group = cg;
            s.info.is_talker = false;
            st.broadcast_about(c, S2C::ClientMoved { id: c, channel: def, by: Some(by.clone()) });
            st.broadcast_client(c);
        }
        for ch in tree {
            st.channels.remove(&ch);
            db(self.store.delete_channel(ch))?;
            crate::files::delete_channel_files(&self.data_dir, ch);
            st.broadcast(S2C::ChannelDeleted(ch));
        }
        Ok(())
    }

    // ------------------------------------------------------------------ chat

    fn send_text(&self, st: &mut State, me: ClientId, target: TextTarget, text: String) -> Res {
        let text = truncate(text.trim_end(), 8000);
        if text.trim().is_empty() {
            return Ok(());
        }
        st.spend_chat(me)?;
        let s = st.session(me)?;
        let from_uid = s.info.uid.clone();
        let from_name = s.info.nickname.clone();
        let ts = now_unix();
        match target {
            // The Server tab only shows what happens on the server (1.19.0+).
            TextTarget::Server => return Err(SERVER_TAB_READ_ONLY.into()),
            TextTarget::Channel(ch) => {
                if s.info.channel != ch {
                    return Err("You can only write to the channel you are in".into());
                }
                self.need(self.perms_in(st, s, ch), perm::CHANNEL_CHAT)?;
                let stored = StoredMessage { from_uid: from_uid.clone(), from_name: from_name.clone(), ts, text: text.clone() };
                let id = db(self.store.append_chat(ch, &stored))?;
                st.broadcast_channel(ch, S2C::Text(ChatMessage { id, target, from: Some(me), from_uid, from_name, ts, text }));
            }
            TextTarget::Private(other) => {
                self.need(st.server_perms(s), perm::PRIVATE_MESSAGE)?;
                st.session(other)?;
                let m = ChatMessage { id: 0, target: TextTarget::Private(me), from: Some(me), from_uid, from_name, ts, text };
                st.send(other, S2C::Text(m.clone()));
                st.send(me, S2C::Text(ChatMessage { target: TextTarget::Private(other), ..m }));
            }
            TextTarget::Group(id) => {
                self.need(st.server_perms(s), perm::PRIVATE_MESSAGE)?;
                let rec = self.member_group(id, &from_uid)?;
                let m = ChatMessage { id: 0, target, from: Some(me), from_uid, from_name, ts, text };
                self.deliver_group(st, id, &rec, m);
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------ group chats

    /// A group chat `uid` is in (anything else looks like it does not exist).
    fn member_group(&self, id: GroupChatId, uid: &str) -> Res<crate::store::GroupChatRec> {
        db(self.store.group_chat(id))?.filter(|r| r.members.iter().any(|m| m == uid)).ok_or_else(|| "Group chat not found".into())
    }

    /// The nickname of a uid: online, else as last seen.
    fn nickname_of(&self, st: &State, uid: &str) -> String {
        st.sessions
            .values()
            .find(|s| s.info.uid == uid)
            .map(|s| s.info.nickname.clone())
            .or_else(|| self.store.get_client(uid).ok().flatten().map(|c| c.nickname))
            .unwrap_or_default()
    }

    fn group_info(&self, st: &State, id: GroupChatId, rec: &crate::store::GroupChatRec) -> GroupChatInfo {
        GroupChatInfo {
            id,
            name: rec.name.clone(),
            owner: rec.owner.clone(),
            members: rec.members.iter().map(|u| (u.clone(), self.nickname_of(st, u))).collect(),
            created: rec.created,
        }
    }

    /// Sends a message to every session of `uids` that understands group chats.
    fn send_to_uids(st: &State, uids: &[String], msg: &S2C) {
        for s in st.sessions.values().filter(|s| uids.contains(&s.info.uid) && s.client_at_least("1.27.0")) {
            s.tx.send(Out::Msg(msg.clone()));
        }
    }

    /// A group message to every member: live to those online (the sender's sessions too, so
    /// it shows there), kept for the others until they connect.
    fn deliver_group(&self, st: &State, id: GroupChatId, rec: &crate::store::GroupChatRec, m: ChatMessage) {
        let msg = S2C::Text(m.clone());
        Self::send_to_uids(st, &rec.members, &msg);
        for uid in rec.members.iter().filter(|u| **u != m.from_uid) {
            let online = st.sessions.values().any(|s| &s.info.uid == uid && s.client_at_least("1.27.0"));
            if online || self.store.group_queue_count(uid).unwrap_or(usize::MAX) >= MAX_OFFLINE_WAITING {
                continue;
            }
            let q = crate::store::QueuedGroupMessage { group: id, from_uid: m.from_uid.clone(), from_name: m.from_name.clone(), ts: m.ts, text: m.text.clone() };
            if let Err(e) = self.store.queue_group_message(uid, &q) {
                warn!("could not keep a group message for {uid}: {e:#}");
            }
        }
    }

    /// Checks people to add to a group chat: known to the server, not yet in it.
    fn new_group_members(&self, rec_members: &[String], uids: Vec<String>) -> Res<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        for u in uids {
            if u.len() > 64 || rec_members.contains(&u) || out.contains(&u) {
                continue;
            }
            if db(self.store.get_client(&u))?.is_none() {
                return Err("Unknown user".into());
            }
            out.push(u);
        }
        if rec_members.len() + out.len() > MAX_GROUP_CHAT_MEMBERS {
            return Err(format!("A group chat can have at most {MAX_GROUP_CHAT_MEMBERS} people").into());
        }
        Ok(out)
    }

    fn create_group_chat(&self, st: &mut State, me: ClientId, name: String, members: Vec<String>) -> Res {
        let s = st.session(me)?;
        self.need(st.server_perms(s), perm::PRIVATE_MESSAGE)?;
        let my_uid = s.info.uid.clone();
        if db(self.store.group_chats_of(&my_uid))?.len() >= MAX_GROUP_CHATS_PER_USER {
            return Err(format!("You can be in at most {MAX_GROUP_CHATS_PER_USER} group chats").into());
        }
        let others = self.new_group_members(std::slice::from_ref(&my_uid), members.into_iter().take(MAX_GROUP_CHAT_MEMBERS * 2).collect())?;
        if others.is_empty() {
            return Err("Add at least one other person".into());
        }
        st.spend_chat(me)?;
        let mut name = clean_line(&name, 50);
        if name.trim().is_empty() {
            name = "Group chat".into();
        }
        let id = db(self.store.new_group_chat_id())?;
        let rec = crate::store::GroupChatRec { name, owner: my_uid.clone(), members: std::iter::once(my_uid).chain(others).collect(), created: now_unix() };
        db(self.store.save_group_chat(id, &rec))?;
        info!("group chat {id} \"{}\" created with {} people", rec.name, rec.members.len());
        let info = self.group_info(st, id, &rec);
        Self::send_to_uids(st, &rec.members, &S2C::GroupChatUpdated(info));
        Ok(())
    }

    fn invite_to_group_chat(&self, st: &mut State, me: ClientId, id: GroupChatId, uids: Vec<String>) -> Res {
        let s = st.session(me)?;
        self.need(st.server_perms(s), perm::PRIVATE_MESSAGE)?;
        let mut rec = self.member_group(id, &s.info.uid)?;
        let add = self.new_group_members(&rec.members, uids.into_iter().take(MAX_GROUP_CHAT_MEMBERS * 2).collect())?;
        if add.is_empty() {
            return Ok(());
        }
        st.spend_chat(me)?;
        rec.members.extend(add);
        db(self.store.save_group_chat(id, &rec))?;
        let info = self.group_info(st, id, &rec);
        Self::send_to_uids(st, &rec.members, &S2C::GroupChatUpdated(info));
        Ok(())
    }

    fn rename_group_chat(&self, st: &mut State, me: ClientId, id: GroupChatId, name: String) -> Res {
        let s = st.session(me)?;
        let mut rec = self.member_group(id, &s.info.uid)?;
        let name = clean_line(&name, 50);
        if name.trim().is_empty() || name == rec.name {
            return Ok(());
        }
        st.spend_chat(me)?;
        rec.name = name;
        db(self.store.save_group_chat(id, &rec))?;
        let info = self.group_info(st, id, &rec);
        Self::send_to_uids(st, &rec.members, &S2C::GroupChatUpdated(info));
        Ok(())
    }

    fn leave_group_chat(&self, st: &mut State, me: ClientId, id: GroupChatId) -> Res {
        let s = st.session(me)?;
        let my_uid = s.info.uid.clone();
        let mut rec = self.member_group(id, &my_uid)?;
        rec.members.retain(|m| *m != my_uid);
        if rec.members.is_empty() {
            db(self.store.delete_group_chat(id))?;
        } else {
            db(self.store.save_group_chat(id, &rec))?;
            let info = self.group_info(st, id, &rec);
            Self::send_to_uids(st, &rec.members, &S2C::GroupChatUpdated(info));
        }
        Self::send_to_uids(st, std::slice::from_ref(&my_uid), &S2C::GroupChatLeft(id));
        Ok(())
    }

    fn history_channel(&self, st: &State, s: &Session, target: TextTarget) -> Res<ChannelId> {
        match target {
            TextTarget::Server => {
                // The Server tab is only for people who may see the server's events.
                self.need(st.server_perms(s), perm::SERVER_EVENTS)?;
                self.need(st.server_perms(s), perm::CHAT_HISTORY)?;
                Ok(SERVER_CHAT_CHANNEL)
            }
            TextTarget::Channel(ch) => {
                if s.info.channel != ch && st.server_perms(s) & perm::CHANNEL_EDIT == 0 {
                    return Err("Join the channel to read its history".into());
                }
                self.need(self.perms_in(st, s, ch), perm::CHAT_HISTORY)?;
                Ok(ch)
            }
            TextTarget::Private(_) | TextTarget::Group(_) => Err("Private conversations are not stored".into()),
        }
    }

    fn fetch_history(&self, st: &mut State, me: ClientId, target: TextTarget, before: Option<MessageId>, limit: u32) -> Res {
        let s = st.session(me)?;
        let ch = self.history_channel(st, s, target)?;
        let limit = limit.clamp(1, 200) as usize;
        let rows = db(self.store.history(ch, before, limit))?;
        let online: HashMap<&str, ClientId> = st.sessions.values().filter(|x| st.sees(s, x)).map(|x| (x.info.uid.as_str(), x.info.id)).collect();
        let complete = rows.len() < limit;
        let messages: Vec<ChatMessage> = rows
            .into_iter()
            .map(|(id, m)| ChatMessage {
                id,
                target,
                from: online.get(m.from_uid.as_str()).copied(),
                from_uid: m.from_uid,
                from_name: m.from_name,
                ts: m.ts,
                text: m.text,
            })
            .collect();
        let ids: Vec<MessageId> = messages.iter().map(|m: &ChatMessage| m.id).collect();
        st.send(me, S2C::History { target, messages, complete });
        if st.session(me)?.client_at_least("1.20.0") {
            let edits = db(self.store.edits(ch, &ids))?;
            if !edits.is_empty() {
                st.send(me, S2C::EditedMessages { target, edits });
            }
        }
        Ok(())
    }

    fn delete_message(&self, st: &mut State, me: ClientId, target: TextTarget, id: MessageId) -> Res {
        let s = st.session(me)?;
        let ch = self.history_channel(st, s, target)?;
        let m = db(self.store.get_chat(ch, id))?.ok_or("Message not found")?;
        if m.from_uid != s.info.uid {
            let p = if ch == SERVER_CHAT_CHANNEL { st.server_perms(s) } else { self.perms_in(st, s, ch) };
            self.need(p, perm::CHAT_DELETE)?;
        }
        db(self.store.delete_chat(ch, id))?;
        let _ = self.store.set_pin(ch, id, None);
        // An image message takes its image with it (only the image really posted with
        // this message, not one a typed tag merely points to).
        if let Some(img) = ImageRef::parse(&m.text) {
            if let Ok(Some(meta)) = self.store.image_meta(img.id) {
                if meta.ch == Some(ch) && meta.msg == id {
                    let _ = self.store.delete_images_where(|x| x.ch == Some(ch) && x.msg == id);
                }
            }
        }
        info!("{} deleted a chat message by {} in {target:?}", s.info.nickname, m.from_name);
        match target {
            TextTarget::Channel(c) => st.broadcast_channel(c, S2C::MessageDeleted { target, id }),
            _ => st.broadcast(S2C::MessageDeleted { target, id }),
        }
        Ok(())
    }

    /// Changes the text of one of your own stored messages ("Edit own chat messages").
    fn edit_message(&self, st: &mut State, me: ClientId, target: TextTarget, id: MessageId, text: String) -> Res {
        let text = truncate(text.trim_end(), 8000);
        if text.trim().is_empty() {
            return Err("A message cannot be empty: delete it instead".into());
        }
        let s = st.session(me)?;
        let ch = match target {
            // The Server tab is read-only (1.19.0+), old messages included.
            TextTarget::Server => return Err(SERVER_TAB_READ_ONLY.into()),
            TextTarget::Channel(c) => {
                if s.info.channel != c {
                    return Err("You can only edit messages in the channel you are in".into());
                }
                c
            }
            TextTarget::Private(_) | TextTarget::Group(_) => return Err("Direct messages are not stored on the server".into()),
        };
        let p = if ch == SERVER_CHAT_CHANNEL { st.server_perms(s) } else { self.perms_in(st, s, ch) };
        self.need(p, perm::CHAT_EDIT_OWN)?;
        let old = db(self.store.get_chat(ch, id))?.ok_or("Message not found")?;
        if old.from_uid != s.info.uid {
            return Err("You can only edit your own messages".into());
        }
        if ImageRef::parse(&old.text).is_some() || ImageRef::parse(&text).is_some() {
            return Err("Images cannot be edited".into());
        }
        if old.text == text {
            return Ok(());
        }
        let nick = s.info.nickname.clone();
        st.spend_chat(me)?;
        let at = now_unix();
        db(self.store.edit_chat(ch, id, &StoredMessage { text: text.clone(), ..old }, at))?;
        info!("{nick} edited their message {id} in {target:?}");
        let msg = S2C::MessageEdited { target, id, text, edited_at: at };
        let ids: Vec<ClientId> =
            st.sessions.values().filter(|x| matches!(target, TextTarget::Server) || x.info.channel == ch).map(|x| x.info.id).collect();
        for c in ids {
            st.send_if_supported(c, "1.20.0", msg.clone());
        }
        Ok(())
    }

    // ------------------------------------------------------------------ avatars

    /// Someone was just seen (or just went invisible, which looks like leaving).
    fn touch_last_seen(&self, uid: &str) {
        if let Ok(Some(mut rec)) = self.store.get_client(uid) {
            rec.last_seen = now_unix();
            let _ = self.store.save_client(&rec);
        }
    }

    fn avatar_meta(&self, uid: &str) -> Option<AvatarMeta> {
        let a = self.store.avatar(uid).ok()??;
        Some(AvatarMeta { uid: uid.to_string(), color: a.color, image: a.hash })
    }

    fn broadcast_avatar(&self, st: &State, meta: AvatarMeta) {
        // A change by someone invisible would give away that they are online.
        let owners: Vec<&Session> = st.sessions.values().filter(|s| s.info.uid == meta.uid).collect();
        for v in st.sessions.values() {
            if (owners.is_empty() || owners.iter().any(|o| st.sees(v, o))) && v.client_at_least("1.20.0") {
                v.tx.send(Out::Msg(S2C::AvatarInfo(vec![meta.clone()])));
            }
        }
    }

    /// Sets your picture and colour. Pictures must be small PNG / JPEG files.
    fn set_avatar(&self, st: &mut State, me: ClientId, color: Option<u32>, image: Option<Vec<u8>>) -> Res {
        if color.is_some_and(|c| c > 0xFF_FFFF) {
            return Err("Invalid colour".into());
        }
        let image = image.unwrap_or_default();
        if !image.is_empty() {
            check_avatar_image(&image)?;
        }
        let uid = st.session(me)?.info.uid.clone();
        let old = db(self.store.avatar(&uid))?.unwrap_or_default();
        let hash = avatar_hash(&image);
        if hash != 0 && hash == old.removed {
            return Err("A moderator removed this picture, so it cannot be used here".into());
        }
        st.spend_chat(me)?;
        let rec = AvatarRecord { color, image, hash, removed: old.removed };
        db(self.store.save_avatar(&uid, &rec))?;
        self.broadcast_avatar(st, AvatarMeta { uid, color, image: hash });
        Ok(())
    }

    /// Removes someone's picture (their colour stays). It cannot be set again.
    fn clear_avatar(&self, st: &mut State, me: ClientId, uid: String) -> Res {
        let s = st.session(me)?;
        if s.info.uid != uid {
            self.need(st.server_perms(s), perm::CLIENT_DESCRIPTION_EDIT)?;
            Self::outranks(st.server_perms(s), self.perms_of_uid(st, &uid))?;
        }
        let Some(mut rec) = db(self.store.avatar(&uid))? else { return Ok(()) };
        if rec.image.is_empty() {
            return Ok(());
        }
        if s.info.uid != uid {
            rec.removed = rec.hash;
            info!("{} removed the picture of {uid}", s.info.nickname);
        }
        rec.image.clear();
        rec.hash = 0;
        db(self.store.save_avatar(&uid, &rec))?;
        self.broadcast_avatar(st, AvatarMeta { uid, color: rec.color, image: 0 });
        Ok(())
    }

    /// Server-wide permissions of an identity, online or not.
    fn perms_of_uid(&self, st: &State, uid: &str) -> u64 {
        if let Some(s) = st.sessions.values().find(|s| s.info.uid == uid) {
            return st.server_perms(s);
        }
        let groups = self.store.get_client(uid).ok().flatten().map(|c| c.server_groups).unwrap_or_default();
        groups.iter().fold(self.store.client_perms(uid).unwrap_or(0), |acc, g| acc | st.group_perms(*g))
    }

    /// Pins / unpins a stored message ("Pin messages" permission, at most 50 per chat).
    fn pin_message(&self, st: &mut State, me: ClientId, target: TextTarget, id: MessageId, pin: bool) -> Res {
        let s = st.session(me)?;
        let ch = self.history_channel(st, s, target)?;
        let p = if ch == SERVER_CHAT_CHANNEL { st.server_perms(s) } else { self.perms_in(st, s, ch) };
        self.need(p, perm::CHAT_PIN)?;
        let by = s.info.nickname.clone();
        let pins = db(self.store.pins(ch))?;
        let is_pinned = pins.iter().any(|(x, _)| *x == id);
        if pin == is_pinned {
            return Ok(());
        }
        if pin {
            db(self.store.get_chat(ch, id))?.ok_or("Message not found")?;
            if pins.len() >= 50 {
                return Err("This chat already has 50 pinned messages - unpin one first".into());
            }
            db(self.store.set_pin(ch, id, Some(&PinMeta { by: by.clone(), at: now_unix() })))?;
        } else {
            db(self.store.set_pin(ch, id, None))?;
        }
        info!("{by} {} message {id} in {target:?}", if pin { "pinned" } else { "unpinned" });
        let ids: Vec<ClientId> = st
            .sessions
            .values()
            .filter(|x| match target {
                // Server tab pins only for people who see the Server tab (1.20.0+).
                TextTarget::Server => st.server_perms(x) & perm::SERVER_EVENTS != 0,
                _ => x.info.channel == ch,
            })
            .map(|x| x.info.id)
            .collect();
        for c in ids {
            st.send_if_supported(c, "1.10.0", S2C::PinChanged { target, id, pinned: pin, by: by.clone() });
        }
        Ok(())
    }

    /// Sends the pinned messages of a chat, most recently pinned first. Pins of messages
    /// that no longer exist (e.g. removed by chat retention) are dropped.
    fn get_pins(&self, st: &mut State, me: ClientId, target: TextTarget) -> Res {
        let s = st.session(me)?;
        let ch = self.history_channel(st, s, target)?;
        let mut pins = Vec::new();
        {
            let online: HashMap<&str, ClientId> = st.sessions.values().filter(|x| st.sees(s, x)).map(|x| (x.info.uid.as_str(), x.info.id)).collect();
            for (id, meta) in db(self.store.pins(ch))? {
                match db(self.store.get_chat(ch, id))? {
                    Some(m) => pins.push(PinnedMessage {
                        message: ChatMessage {
                            id,
                            target,
                            from: online.get(m.from_uid.as_str()).copied(),
                            from_uid: m.from_uid,
                            from_name: m.from_name,
                            ts: m.ts,
                            text: m.text,
                        },
                        pinned_by: meta.by,
                        pinned_at: meta.at,
                    }),
                    None => {
                        let _ = self.store.set_pin(ch, id, None);
                    }
                }
            }
        }
        pins.sort_by(|a, b| b.pinned_at.cmp(&a.pinned_at).then(b.message.id.cmp(&a.message.id)));
        st.send(me, S2C::Pins { target, pins });
        Ok(())
    }

    // ------------------------------------------------------------------ moderation

    fn ban(
        &self,
        st: &mut State,
        me: ClientId,
        target: BanTarget,
        duration: Option<u64>,
        reason: String,
        kicks: &mut Vec<(ClientId, LeaveReason)>,
    ) -> Res {
        let s = st.session(me)?;
        self.need(st.server_perms(s), perm::CLIENT_BAN)?;
        let by = s.info.nickname.clone();
        let reason = clean_line(&reason, 200);
        let (uid, ip, name) = match &target {
            BanTarget::Client(id) => {
                let t = st.session(*id)?;
                self.check_outranks(st, s, t)?;
                (Some(t.info.uid.clone()), Some(t.ip.to_string()), Some(t.info.nickname.clone()))
            }
            BanTarget::Uid(u) => {
                // Same rule as banning someone who is online: administrators can only be
                // banned by administrators.
                if let Some(t) = st.sessions.values().find(|x| x.info.uid == *u) {
                    self.check_outranks(st, s, t)?;
                } else if let Some(rec) = db(self.store.get_client(u))? {
                    let theirs = rec.server_groups.iter().fold(self.store.client_perms(u).unwrap_or(0), |a, g| a | st.group_perms(*g));
                    Self::outranks(st.server_perms(s), theirs)?;
                }
                (Some(u.clone()), None, None)
            }
            BanTarget::Ip(i) => {
                let ip: IpAddr = i.trim().parse().map_err(|_| "Invalid IP address".to_string())?;
                for t in st.sessions.values().filter(|x| x.ip.to_canonical() == ip.to_canonical()) {
                    self.check_outranks(st, s, t)?;
                }
                (None, Some(ip.to_string()), None)
            }
        };
        if uid.as_deref() == Some(s.info.uid.as_str()) {
            return Err("You cannot ban yourself".into());
        }
        db(self.store.add_ban(BanEntry {
            id: 0,
            uid: uid.clone(),
            ip: ip.clone(),
            name,
            reason: reason.clone(),
            created: now_unix(),
            expires: duration.filter(|d| *d > 0).map(|d| now_unix() + d),
            by: by.clone(),
        }))?;
        for t in st.sessions.values() {
            let hit = uid.as_deref() == Some(t.info.uid.as_str()) || ip.as_deref() == Some(t.ip.to_string().as_str());
            if hit && t.info.id != me {
                kicks.push((t.info.id, LeaveReason::Banned { by: by.clone(), reason: reason.clone() }));
            }
        }
        Ok(())
    }

    /// Online, away, busy or invisible.
    fn set_presence(&self, st: &mut State, me: ClientId, p: Presence) -> Res {
        let s = st.sessions.get_mut(&me).ok_or("Client not found")?;
        // Away may also have been set with UpdateSelf (idle), so the away field is
        // checked too: "Online" must clear it even when the presence did not change.
        let away_ok = (p == Presence::Away) == s.info.away.is_some();
        if s.presence == p && away_ok {
            return Ok(());
        }
        let changed = s.presence != p;
        // Each change is announced to everyone (and invisible ones as joins / leaves), so
        // changes share the chat rate limit.
        st.spend_chat(me)?;
        let s = st.sessions.get_mut(&me).ok_or("Client not found")?;
        let was_hidden = s.hidden();
        s.presence = p;
        match p {
            Presence::Away => {
                if s.info.away.is_none() {
                    s.info.away = Some(String::new());
                }
            }
            _ => s.info.away = None,
        }
        let uid = s.info.uid.clone();
        if !was_hidden && s.hidden() {
            self.touch_last_seen(&uid);
        }
        st.announce_visibility(me, was_hidden, self.avatar_meta(&uid));
        st.broadcast_client(me);
        if changed {
            st.broadcast_presence(me);
        }
        Ok(())
    }

    fn send_ban_list(&self, st: &State, me: ClientId) -> Res {
        st.send(me, S2C::BanList(db(self.store.bans())?));
        let hw: Vec<u64> = db(self.store.hardware_bans())?.into_iter().map(|(id, _)| id).collect();
        st.send_if_supported(me, "1.21.0", S2C::BannedHardware(hw));
        Ok(())
    }

    /// An active ban that includes one of these hardware tokens.
    fn find_hardware_ban(&self, tokens: &[String]) -> Option<BanEntry> {
        if tokens.is_empty() {
            return None;
        }
        let now = now_unix();
        let bans = self.store.bans().ok()?;
        for (id, banned) in self.store.hardware_bans().ok()? {
            match bans.iter().find(|b| b.id == id) {
                Some(b) if b.expires.map(|e| e > now).unwrap_or(true) => {
                    if banned.iter().any(|t| tokens.contains(t)) {
                        return Some(b.clone());
                    }
                }
                // The ban is gone or expired: so is its hardware part.
                _ => {
                    let _ = self.store.remove_ban(id);
                }
            }
        }
        None
    }

    /// The hardware tokens a client reports after connecting; banned hardware is turned away.
    fn set_hardware(&self, st: &mut State, me: ClientId, tokens: Vec<String>, kicks: &mut Vec<(ClientId, LeaveReason)>) -> Res {
        let mut tokens: Vec<String> = tokens.into_iter().filter(|t| valid_hardware_token(t)).take(8).collect();
        tokens.sort();
        tokens.dedup();
        // Nothing usable does not count as reporting (the deadline stays).
        if tokens.is_empty() {
            return Err("Invalid hardware report".into());
        }
        let ban = self.find_hardware_ban(&tokens);
        let s = st.sessions.get_mut(&me).ok_or("Client not found")?;
        if !s.hardware.is_empty() {
            return Ok(()); // reported once per connection
        }
        s.hardware = tokens;
        s.hardware_due = None;
        let s = st.session(me)?;
        // Like address bans, administrators cannot be locked out this way.
        if let Some(b) = ban.filter(|_| st.server_perms(s) & perm::SERVER_EDIT == 0) {
            info!("turned away {} ({}): banned hardware", s.info.nickname, s.info.uid);
            kicks.push((me, LeaveReason::Banned { by: b.by, reason: b.reason }));
        }
        Ok(())
    }

    /// Disconnects Voxa 1.21.0+ clients that never reported their hardware (a modified
    /// client trying to dodge hardware bans).
    pub fn enforce_hardware(&self) {
        let late: Vec<ClientId> = {
            let st = self.state.lock();
            st.sessions.values().filter(|s| s.hardware_due.map(|d| Instant::now() > d).unwrap_or(false)).map(|s| s.info.id).collect()
        };
        for id in late {
            info!("client {id} did not report its hardware; disconnecting");
            self.kick_session(id, LeaveReason::Disconnected("Your Voxa did not identify itself. Please update or reinstall Voxa.".into()));
        }
    }

    /// Bans someone who is online by their unique ID, IP address and / or hardware.
    #[allow(clippy::too_many_arguments)]
    fn ban_with(&self, st: &mut State, me: ClientId, target: ClientId, what: [bool; 3], duration: Option<u64>, reason: String, kicks: &mut Vec<(ClientId, LeaveReason)>) -> Res {
        let [by_uid, by_ip, by_hw] = what;
        if !(by_uid || by_ip || by_hw) {
            return Err("Choose what to ban: the unique ID, the IP address and / or the hardware".into());
        }
        let s = st.session(me)?;
        self.need(st.server_perms(s), perm::CLIENT_BAN)?;
        let t = st.session(target)?;
        self.check_outranks(st, s, t)?;
        if t.info.uid == s.info.uid {
            return Err("You cannot ban yourself".into());
        }
        if by_hw && t.hardware.is_empty() {
            return Err("This person's hardware is not known (web page or an older Voxa): ban the unique ID or IP address instead".into());
        }
        let by = s.info.nickname.clone();
        let reason = clean_line(&reason, 200);
        let (uid, ip, hw) = (t.info.uid.clone(), t.ip.to_string(), t.hardware.clone());
        let entry = db(self.store.add_ban(BanEntry {
            id: 0,
            uid: by_uid.then(|| uid.clone()),
            ip: by_ip.then(|| ip.clone()),
            name: Some(t.info.nickname.clone()),
            reason: reason.clone(),
            created: now_unix(),
            expires: duration.filter(|d| *d > 0).map(|d| now_unix() + d),
            by: by.clone(),
        }))?;
        if by_hw {
            db(self.store.add_hardware_ban(entry.id, &hw))?;
        }
        info!("{by} banned {} (unique ID: {by_uid}, IP: {by_ip}, hardware: {by_hw})", t.info.nickname);
        for x in st.sessions.values() {
            let hit = (by_uid && x.info.uid == uid) || (by_ip && x.ip.to_string() == ip) || (by_hw && x.hardware.iter().any(|h| hw.contains(h)));
            if hit && x.info.id != me {
                kicks.push((x.info.id, LeaveReason::Banned { by: by.clone(), reason: reason.clone() }));
            }
        }
        Ok(())
    }

    // ------------------------------------------------------------------ server & groups

    fn edit_server(&self, st: &mut State, me: ClientId, e: ServerEdit) -> Res {
        self.need(st.server_perms(st.session(me)?), perm::SERVER_EDIT)?;
        let kind_ok = |g: GroupId, k: GroupKind| st.groups.get(&g).map(|g| g.kind == k).unwrap_or(false);
        if !kind_ok(e.default_server_group, GroupKind::Server)
            || !kind_ok(e.default_channel_group, GroupKind::Channel)
            || !kind_ok(e.channel_admin_group, GroupKind::Channel)
        {
            return Err("Invalid default group selection".into());
        }
        // A default group is handed to everyone, so it must not hold more than the person
        // choosing it (otherwise every new user, e.g. a second identity, gets it).
        let mine = st.server_perms(st.session(me)?);
        let cur = &st.settings;
        for (new, old) in [
            (e.default_server_group, cur.default_server_group),
            (e.default_channel_group, cur.default_channel_group),
            (e.channel_admin_group, cur.channel_admin_group),
        ] {
            if new != old && st.groups.get(&new).map(|g| g.perms & !mine != 0).unwrap_or(true) {
                return Err("You cannot make a group with permissions you do not have the default".into());
            }
        }
        let s = &mut st.settings;
        s.name = clean_line(&e.name, 64);
        if s.name.is_empty() {
            s.name = "Voxa Server".into();
        }
        s.welcome_message = clean_text(&e.welcome_message, 4000);
        s.host_message = clean_text(&e.host_message, 1000);
        s.max_clients = e.max_clients.clamp(1, 1024);
        s.default_server_group = e.default_server_group;
        s.default_channel_group = e.default_channel_group;
        s.channel_admin_group = e.channel_admin_group;
        s.chat_retention_days = e.chat_retention_days;
        match e.password {
            PasswordChange::Keep => {}
            PasswordChange::Clear => s.password_hash = None,
            PasswordChange::Set(p) if p.is_empty() => s.password_hash = None,
            PasswordChange::Set(p) => s.password_hash = Some(self.pw_hash(&p)),
        }
        db(self.store.save_settings(&st.settings))?;
        st.broadcast(S2C::ServerUpdated(st.server_info(self)));
        Ok(())
    }

    fn delete_group(&self, st: &mut State, me: ClientId, id: GroupId) -> Res {
        self.need(st.server_perms(st.session(me)?), perm::GROUP_MANAGE)?;
        let g = st.groups.get(&id).ok_or("Group not found")?;
        if g.builtin {
            return Err("Built-in groups cannot be deleted".into());
        }
        let s = &st.settings;
        if [s.default_server_group, s.default_channel_group, s.channel_admin_group].contains(&id) {
            return Err("This group is used as a default group".into());
        }
        db(self.store.delete_group(id))?;
        st.groups.remove(&id);
        let def_s = st.settings.default_server_group;
        let def_c = st.settings.default_channel_group;
        let mut changed = Vec::new();
        for s in st.sessions.values_mut() {
            let before = s.info.clone();
            s.info.server_groups.retain(|g| *g != id);
            if s.info.server_groups.is_empty() {
                s.info.server_groups.push(def_s);
            }
            if s.info.channel_group == id {
                s.info.channel_group = def_c;
            }
            if before != s.info {
                changed.push(s.info.id);
            }
        }
        if let Ok(all) = self.store.all_clients() {
            for mut r in all.into_iter().filter(|r| r.server_groups.contains(&id)) {
                r.server_groups.retain(|g| *g != id);
                if r.server_groups.is_empty() {
                    r.server_groups.push(def_s);
                }
                let _ = self.store.save_client(&r);
            }
        }
        st.broadcast(S2C::GroupsUpdated(st.groups.values().cloned().collect()));
        for c in changed {
            st.broadcast_client(c);
        }
        Ok(())
    }

    fn set_server_group(&self, st: &mut State, me: ClientId, uid: &str, group: GroupId, add: bool) -> Res {
        let s = st.session(me)?;
        let mine = st.server_perms(s);
        self.need(mine, perm::GROUP_ASSIGN_SERVER)?;
        let g = st.groups.get(&group).ok_or("Group not found")?;
        if g.kind != GroupKind::Server {
            return Err("Not a server group".into());
        }
        if g.perms & !mine != 0 {
            return Err("You cannot assign groups with more permissions than you have".into());
        }
        self.apply_server_group(st, uid, group, add)
    }

    fn apply_server_group(&self, st: &mut State, uid: &str, group: GroupId, add: bool) -> Res {
        let mut rec = db(self.store.get_client(uid))?.ok_or("Unknown client")?;
        if add {
            if !rec.server_groups.contains(&group) {
                rec.server_groups.push(group);
            }
            // Joining a real group replaces the implicit default group.
            if group != st.settings.default_server_group {
                let def = st.settings.default_server_group;
                rec.server_groups.retain(|g| *g != def);
            }
        } else {
            rec.server_groups.retain(|g| *g != group);
        }
        if rec.server_groups.is_empty() {
            rec.server_groups.push(st.settings.default_server_group);
        }
        db(self.store.save_client(&rec))?;
        let ids: Vec<ClientId> = st.sessions.values().filter(|s| s.info.uid == uid).map(|s| s.info.id).collect();
        for id in ids {
            st.sessions.get_mut(&id).unwrap().info.server_groups = rec.server_groups.clone();
            st.broadcast_client(id);
        }
        Ok(())
    }

    fn set_channel_group(&self, st: &mut State, me: ClientId, uid: &str, ch: ChannelId, group: GroupId) -> Res {
        let s = st.session(me)?;
        let mine = self.perms_in(st, s, ch);
        self.need(mine, perm::GROUP_ASSIGN_CHANNEL)?;
        let g = st.groups.get(&group).ok_or("Group not found")?;
        if g.kind != GroupKind::Channel {
            return Err("Not a channel group".into());
        }
        if g.perms & !mine != 0 {
            return Err("You cannot assign groups with more permissions than you have".into());
        }
        self.apply_channel_group(st, uid, ch, group)
    }

    fn apply_channel_group(&self, st: &mut State, uid: &str, ch: ChannelId, group: GroupId) -> Res {
        if !st.channels.contains_key(&ch) {
            return Err("Channel not found".into());
        }
        let stored = if group == st.settings.default_channel_group { None } else { Some(group) };
        db(self.store.set_channel_group(uid, ch, stored))?;
        let ids: Vec<ClientId> =
            st.sessions.values().filter(|s| s.info.uid == uid && s.info.channel == ch).map(|s| s.info.id).collect();
        for id in ids {
            st.sessions.get_mut(&id).unwrap().info.channel_group = group;
            st.broadcast_client(id);
        }
        Ok(())
    }

    // ------------------------------------------------------------------ offline messages

    /// Keeps a direct message for someone who is offline until they come back.
    fn send_offline(&self, st: &mut State, me: ClientId, to_uid: String, text: String) -> Res {
        let text = clean_text(text.trim_end(), 8000);
        if text.trim().is_empty() {
            return Ok(());
        }
        let s = st.session(me)?;
        self.need(st.server_perms(s), perm::PRIVATE_MESSAGE)?;
        if s.info.uid == to_uid {
            return Err("You cannot message yourself".into());
        }
        let online: Vec<ClientId> = st.sessions.values().filter(|x| x.info.uid == to_uid).map(|x| x.info.id).collect();
        if !online.is_empty() {
            // Someone invisible looks offline: hand the message over directly, and answer
            // as if it was kept for later (so the sender does not learn they are online).
            if online.iter().all(|t| !st.sees(s, &st.sessions[t])) {
                let (from_uid, from_name) = (s.info.uid.clone(), s.info.nickname.clone());
                st.spend_chat(me)?;
                let ts = now_unix();
                let m = ChatMessage { id: 0, target: TextTarget::Private(me), from: Some(me), from_uid, from_name, ts, text };
                for t in online {
                    st.send(t, S2C::Text(m.clone()));
                }
                st.send(me, S2C::OfflineQueued { to_uid, ts });
                return Ok(());
            }
            return Err("This person is online now: send the message again".into());
        }
        let (from_uid, from_name) = (s.info.uid.clone(), s.info.nickname.clone());
        let to = db(self.store.get_client(&to_uid))?.ok_or("Unknown user")?;
        if db(self.store.offline_count(&to_uid))? >= MAX_OFFLINE_WAITING {
            return Err(format!("{} already has {MAX_OFFLINE_WAITING} messages waiting", to.nickname));
        }
        st.spend_chat(me)?;
        let ts = now_unix();
        db(self.store.add_offline(&to_uid, &OfflineMessage { from_uid, from_name: from_name.clone(), ts, text }))?;
        info!("{from_name} left an offline message for {} ({to_uid})", to.nickname);
        st.send_if_supported(me, "1.18.0", S2C::OfflineQueued { to_uid, ts });
        Ok(())
    }

    /// Undelivered offline messages are kept for `OFFLINE_DAYS`.
    pub fn purge_offline(&self) {
        let cutoff = now_unix().saturating_sub(OFFLINE_DAYS * 86_400);
        match self.store.purge_offline_before(cutoff) {
            Ok(n) if n > 0 => info!("removed {n} undelivered offline message(s) older than {OFFLINE_DAYS} days"),
            Err(e) => warn!("offline message cleanup failed: {e:#}"),
            _ => {}
        }
        match self.store.purge_group_queue_before(cutoff) {
            Ok(n) if n > 0 => info!("removed {n} undelivered group chat message(s)"),
            Err(e) => warn!("group message cleanup failed: {e:#}"),
            _ => {}
        }
    }

    // ------------------------------------------------------------------ periodic

    pub fn purge_chat(&self) {
        let days = self.state.lock().settings.chat_retention_days;
        if days == 0 {
            return;
        }
        let cutoff = now_unix().saturating_sub(days as u64 * 86_400);
        match self.store.purge_chat_before(cutoff) {
            Ok(n) if n > 0 => info!("chat retention: removed {n} old messages"),
            Err(e) => warn!("chat retention failed: {e:#}"),
            _ => {}
        }
        match self.store.delete_images_where(|m| m.ts < cutoff) {
            Ok(n) if n > 0 => info!("chat retention: removed {n} old images"),
            Err(e) => warn!("chat image retention failed: {e:#}"),
            _ => {}
        }
    }

    // ------------------------------------------------------------------ chat images

    /// Posts an image to a chat (bulk connection, clients 1.5.0+). Same permissions as
    /// sending text there; the image is stored encrypted next to the message.
    pub fn post_image(&self, me: ClientId, target: TextTarget, name: &str, width: u32, height: u32, data: Vec<u8>) -> Res {
        if data.is_empty() || data.len() > MAX_CHAT_IMAGE {
            return Err(format!("Images can be at most {} MB", MAX_CHAT_IMAGE / (1024 * 1024)));
        }
        let ext = image_kind(&data).ok_or("Not a supported image (PNG, JPEG, GIF, WebP or BMP)")?;
        let name: String = name.chars().filter(|c| !c.is_control()).collect();
        let mut name = truncate(name.trim(), 100);
        if name.is_empty() {
            name = format!("image.{ext}");
        }
        if target == TextTarget::Server {
            return Err(SERVER_TAB_READ_ONLY.into());
        }
        let mut guard = self.state.lock();
        let st = &mut *guard;
        st.spend_chat(me)?;
        let s = st.session(me)?;
        let from_uid = s.info.uid.clone();
        let from_name = s.info.nickname.clone();
        let ts = now_unix();
        let (width, height) = (width.min(100_000), height.min(100_000));
        let size = data.len();
        match target {
            TextTarget::Server | TextTarget::Channel(_) => {
                let ch = match target {
                    TextTarget::Channel(ch) => {
                        if s.info.channel != ch || ch == NO_CHANNEL {
                            return Err("You can only post to the channel you are in".into());
                        }
                        self.need(self.perms_in(st, s, ch), perm::CHANNEL_CHAT)?;
                        ch
                    }
                    _ => {
                        self.need(st.server_perms(s), perm::SERVER_CHAT)?;
                        SERVER_CHAT_CHANNEL
                    }
                };
                let image = db(self.store.new_image_id())?;
                let text = ImageRef { id: image, width, height, name: name.clone() }.to_text();
                let stored = StoredMessage { from_uid: from_uid.clone(), from_name: from_name.clone(), ts, text: text.clone() };
                let id = db(self.store.append_chat(ch, &stored))?;
                let meta = ImageMeta { ch: Some(ch), msg: id, uids: Vec::new(), ts, name: name.clone() };
                if let Err(e) = db(self.store.put_image(image, &meta, &data)) {
                    let _ = self.store.delete_chat(ch, id);
                    return Err(e);
                }
                let m = ChatMessage { id, target, from: Some(me), from_uid, from_name: from_name.clone(), ts, text };
                if ch == SERVER_CHAT_CHANNEL {
                    st.broadcast(S2C::Text(m));
                } else {
                    st.broadcast_channel(ch, S2C::Text(m));
                }
            }
            TextTarget::Private(other) => {
                self.need(st.server_perms(s), perm::PRIVATE_MESSAGE)?;
                let other_uid = st.session(other)?.info.uid.clone();
                let image = db(self.store.new_image_id())?;
                let text = ImageRef { id: image, width, height, name: name.clone() }.to_text();
                let meta = ImageMeta { ch: None, msg: 0, uids: vec![from_uid.clone(), other_uid], ts, name: name.clone() };
                db(self.store.put_image(image, &meta, &data))?;
                let m = ChatMessage { id: 0, target: TextTarget::Private(me), from: Some(me), from_uid, from_name: from_name.clone(), ts, text };
                st.send(other, S2C::Text(m.clone()));
                st.send(me, S2C::Text(ChatMessage { target: TextTarget::Private(other), ..m }));
            }
            TextTarget::Group(id) => {
                // Like a private image: only the chat's members (at the time) may open it.
                self.need(st.server_perms(s), perm::PRIVATE_MESSAGE)?;
                let rec = self.member_group(id, &from_uid)?;
                let image = db(self.store.new_image_id())?;
                let text = ImageRef { id: image, width, height, name: name.clone() }.to_text();
                let meta = ImageMeta { ch: None, msg: 0, uids: rec.members.clone(), ts, name: name.clone() };
                db(self.store.put_image(image, &meta, &data))?;
                let m = ChatMessage { id: 0, target, from: Some(me), from_uid, from_name: from_name.clone(), ts, text };
                self.deliver_group(st, id, &rec, m);
            }
        }
        info!("{from_name} posted image \"{name}\" ({} KB) to {target:?}", size / 1024);
        Ok(())
    }

    /// Returns a chat image if `me` may see it: server chat images to everyone,
    /// channel images to members of that channel, private ones to the two participants.
    pub fn chat_image(&self, me: ClientId, image: u64) -> Res<Vec<u8>> {
        let meta = db(self.store.image_meta(image))?.ok_or("Image not found (it may have been deleted)")?;
        {
            let st = self.state.lock();
            let s = st.session(me)?;
            match meta.ch {
                // Older Server tab pictures: like its history, only with "See server events".
                Some(SERVER_CHAT_CHANNEL) => self.need(st.server_perms(s), perm::SERVER_EVENTS)?,
                Some(ch) => {
                    if s.info.channel != ch && st.server_perms(s) & perm::CHANNEL_EDIT == 0 {
                        return Err("Join the channel to see its images".into());
                    }
                }
                None => {
                    if !meta.uids.contains(&s.info.uid) {
                        return Err("This image belongs to someone else's conversation".into());
                    }
                }
            }
        }
        db(self.store.image_data(image))?.ok_or_else(|| "Image not found (it may have been deleted)".to_string())
    }

    // ------------------------------------------------------------------ screen share

    pub fn relay_frame(&self, from: ClientId, frame: StreamFrame) {
        let mut st = self.state.lock();
        let Some(s) = st.sessions.get(&from) else { return };
        if s.info.streaming.is_none() {
            return;
        }
        // Frames are copied for every viewer: refuse oversized ones, and keep only a few
        // queued per viewer (a slow viewer skips frames and gets a fresh keyframe).
        if frame.tiles.iter().map(|t| t.jpeg.len()).sum::<usize>() > MAX_FRAME_BYTES {
            return;
        }
        let viewers: Vec<ClientId> = s.viewers.iter().copied().collect();
        let mut dropped = false;
        for v in viewers {
            let Some(viewer) = st.sessions.get(&v) else { continue };
            if let Some(b) = &viewer.bulk_tx {
                let queued = &viewer.bulk_frames;
                // Counted before sending, so the writer can never count below zero.
                if queued.fetch_add(1, Ordering::Relaxed) >= MAX_QUEUED_FRAMES || b.try_send(BulkS2C::Frame { from, frame: frame.clone() }).is_err() {
                    queued.fetch_sub(1, Ordering::Relaxed);
                    dropped = true;
                }
            }
        }
        if dropped {
            self.request_keyframe_locked(&mut st, from);
        }
    }

    /// Forwards a stream's sound to its viewers (clients 1.8.0+ only: older ones cannot
    /// decode the message). Dropped for a viewer whose connection is backed up.
    pub fn relay_audio(&self, from: ClientId, data: Vec<u8>) {
        if data.len() > 4000 {
            return;
        }
        let st = self.state.lock();
        let Some(s) = st.sessions.get(&from) else { return };
        if s.info.streaming.is_none() {
            return;
        }
        for v in &s.viewers {
            if let Some(viewer) = st.sessions.get(v).filter(|x| x.client_at_least("1.8.0")) {
                if let Some(b) = &viewer.bulk_tx {
                    let _ = b.try_send(BulkS2C::StreamAudio { from, data: data.clone() });
                }
            }
        }
    }

    /// A channel's calendar is for the people in it (and those who may edit the channel),
    /// so the calendar of a password or members-only channel stays inside it.
    fn calendar_reach(&self, st: &State, s: &Session, ch: ChannelId) -> Res {
        if s.info.channel == ch || self.perms_in(st, s, ch) & perm::CHANNEL_EDIT != 0 {
            Ok(())
        } else {
            Err("Join the channel to see its calendar".into())
        }
    }

    /// Changes a channel's calendar (needs `bit` there, and seeing it), saves it and sends
    /// it to everyone who has it open. Events that ended over 30 days ago are dropped.
    /// SERVER_CALENDAR is the server calendar: the permissions of the server groups (and
    /// personal ones) count, wherever one is.
    fn calendar_edit(&self, st: &mut State, me: ClientId, ch: ChannelId, bit: u64, f: impl FnOnce(&mut ChannelCalendar, &str, &str) -> Res) -> Res {
        let s = st.session(me)?;
        let server = ch == SERVER_CALENDAR;
        if server && !s.client_at_least("1.31.0") {
            return Err("Update Voxa to use the server calendar".into());
        }
        if !server && !st.channels.contains_key(&ch) {
            return Err("Channel not found".into());
        }
        let p = if server { st.server_perms(s) } else { self.perms_in(st, s, ch) };
        self.need(p, perm::CALENDAR_VIEW)?;
        self.need(p, bit)?;
        if !server {
            self.calendar_reach(st, s, ch)?;
        }
        let (uid, name) = (s.info.uid.clone(), s.info.nickname.clone());
        st.spend_chat(me)?;
        let mut cal = db(self.store.calendar(ch))?;
        f(&mut cal, &uid, &name)?;
        let now = now_unix();
        cal.events.retain(|e| e.start + e.duration_min as u64 * 60 + 30 * 86_400 > now);
        cal.events.sort_by_key(|e| e.start);
        db(self.store.save_calendar(ch, &cal))?;
        if server {
            let to: Vec<ClientId> = st.sessions.values().filter(|x| x.calendar_subs.contains(&ch) && st.server_perms(x) & perm::CALENDAR_VIEW != 0).map(|x| x.info.id).collect();
            let msg = self.server_calendar_msg(st)?;
            for c in to {
                st.send(c, msg.clone());
            }
            return Ok(());
        }
        let to: Vec<ClientId> = st.sessions.values().filter(|x| x.calendar_subs.contains(&ch) && self.perms_in(st, x, ch) & perm::CALENDAR_VIEW != 0 && self.calendar_reach(st, x, ch).is_ok()).map(|x| x.info.id).collect();
        for c in to {
            st.send(c, S2C::Calendar { channel: ch, calendars: cal.calendars.clone(), events: cal.events.clone() });
        }
        Ok(())
    }

    /// The server calendar for a client: meeting places of events that still exist, in
    /// channels that still exist.
    fn server_calendar_msg(&self, st: &State) -> Res<S2C> {
        let cal = db(self.store.calendar(SERVER_CALENDAR))?;
        let meet = db(self.store.calendar_meet())?.into_iter().filter(|(e, ch)| cal.events.iter().any(|x| x.id == *e) && st.channels.contains_key(ch)).collect();
        Ok(S2C::ServerCalendar { calendars: cal.calendars, events: cal.events, meet })
    }

    /// Forgets someone who is offline (see C2S::ForgetClient).
    fn forget_client(&self, st: &mut State, me: ClientId, uid: String) -> Res {
        let s = st.session(me)?;
        let mine = st.server_perms(s);
        self.need(mine, perm::GROUP_MANAGE)?;
        if uid == s.info.uid {
            return Err("You cannot remove yourself".into());
        }
        if st.sessions.values().any(|x| x.info.uid == uid) {
            return Err("They are online: they can only be removed while offline (kick them first)".into());
        }
        // Nobody can take away permissions (groups) they do not have themselves.
        Self::outranks(mine, self.perms_of_uid(st, &uid))?;
        let by = s.info.nickname.clone();
        let name = db(self.store.get_client(&uid))?.map(|c| c.nickname);
        if !db(self.store.delete_client(&uid))? {
            return Err("Unknown user".into());
        }
        info!("{by} removed {} ({uid}) from the server", name.as_deref().unwrap_or("?"));
        st.send(me, S2C::Notice(format!("Removed {} from the server", name.as_deref().unwrap_or("them"))));
        let people = db(self.store.all_clients())?.into_iter().map(|c| Person { uid: c.uid, nickname: c.nickname, last_seen: c.last_seen }).collect();
        st.send(me, S2C::People(people));
        Ok(())
    }

    /// Tells everyone (Voxa 1.29.0+) that `id` turned their camera on or off.
    fn camera_changed(&self, st: &mut State, id: ClientId) {
        let camera = st.sessions.get(&id).and_then(|s| s.camera);
        let to: Vec<ClientId> = st.sessions.values().filter(|x| x.client_at_least("1.29.0")).map(|x| x.info.id).collect();
        for c in to {
            st.send(c, S2C::CameraChanged { client: id, camera });
        }
    }

    /// Forwards a camera picture to everyone else in the sender's channel (Voxa 1.29.0+),
    /// like screen share frames: oversized ones are refused, and a slow receiver skips
    /// pictures and the camera is asked for a full one.
    pub fn relay_camera(&self, from: ClientId, frame: StreamFrame) {
        let mut st = self.state.lock();
        let Some(s) = st.sessions.get(&from) else { return };
        let Some(cam) = s.camera else { return };
        let ch = s.info.channel;
        if ch == NO_CHANNEL || frame.width > cam.width.max(cam.height) || frame.height > cam.width.max(cam.height) {
            return;
        }
        if frame.tiles.iter().map(|t| t.jpeg.len()).sum::<usize>() > MAX_CAMERA_FRAME_BYTES {
            return;
        }
        let to: Vec<ClientId> = st.sessions.values().filter(|x| x.info.id != from && x.info.channel == ch && !x.web && x.client_at_least("1.29.0")).map(|x| x.info.id).collect();
        let mut dropped = false;
        for v in to {
            let Some(viewer) = st.sessions.get(&v) else { continue };
            if let Some(b) = &viewer.bulk_tx {
                let queued = &viewer.bulk_frames;
                if queued.fetch_add(1, Ordering::Relaxed) >= MAX_QUEUED_FRAMES || b.try_send(BulkS2C::CameraFrame { from, frame: frame.clone() }).is_err() {
                    queued.fetch_sub(1, Ordering::Relaxed);
                    dropped = true;
                }
            }
        }
        if dropped {
            self.camera_keyframe_locked(&mut st, from);
        }
    }

    /// Someone in the same channel needs a full picture of this camera.
    pub fn request_camera_keyframe(&self, me: ClientId, camera_of: ClientId) {
        let mut st = self.state.lock();
        let same = match (st.sessions.get(&me), st.sessions.get(&camera_of)) {
            (Some(a), Some(b)) => a.info.channel == b.info.channel && b.camera.is_some(),
            _ => false,
        };
        if same {
            self.camera_keyframe_locked(&mut st, camera_of);
        }
    }

    fn camera_keyframe_locked(&self, st: &mut State, id: ClientId) {
        if let Some(s) = st.sessions.get_mut(&id) {
            if s.last_camera_key_req.map(|t| t.elapsed().as_millis() > 1000).unwrap_or(true) {
                s.last_camera_key_req = Some(Instant::now());
                if let Some(b) = &s.bulk_tx {
                    let _ = b.try_send(BulkS2C::CameraKeyframeRequested);
                }
            }
        }
    }

    pub fn request_keyframe(&self, streamer: ClientId) {
        let mut st = self.state.lock();
        self.request_keyframe_locked(&mut st, streamer);
    }

    fn request_keyframe_locked(&self, st: &mut State, streamer: ClientId) {
        if let Some(s) = st.sessions.get_mut(&streamer) {
            let due = s.last_keyframe_req.map(|t| t.elapsed().as_millis() > 1000).unwrap_or(true);
            if due {
                s.last_keyframe_req = Some(Instant::now());
                if let Some(b) = &s.bulk_tx {
                    let _ = b.try_send(BulkS2C::KeyframeRequested);
                }
            }
        }
    }

    /// Checks whether `id` may upload to / download from a channel.
    pub fn file_perm(&self, id: ClientId, ch: ChannelId, bit: u64) -> Res {
        let st = self.state.lock();
        let s = st.session(id)?;
        if !st.channels.contains_key(&ch) {
            return Err("Channel not found".into());
        }
        self.need(self.perms_in(&st, s, ch), bit)
    }
}

fn find_channel_by_path(st: &State, path: &str) -> Option<ChannelId> {
    let mut parent: Option<ChannelId> = None;
    let mut found = None;
    for part in path.split('/').map(str::trim).filter(|p| !p.is_empty()) {
        let c = st.channels.values().find(|c| c.info.parent == parent && c.info.name.eq_ignore_ascii_case(part))?;
        parent = Some(c.info.id);
        found = Some(c.info.id);
    }
    found
}

/// Whether a message refers to "channel 0" (not in a channel) where a real channel is needed.
fn names_no_channel(msg: &C2S) -> bool {
    let t = |x: &TextTarget| matches!(x, TextTarget::Channel(NO_CHANNEL));
    match msg {
        C2S::SendText { target, .. }
        | C2S::FetchHistory { target, .. }
        | C2S::DeleteMessage { target, .. }
        | C2S::PinMessage { target, .. }
        | C2S::EditMessage { target, .. } => t(target),
        C2S::GetPins(target) | C2S::ClearChat(target) => t(target),
        C2S::FileList { channel, .. } | C2S::FileDelete { channel, .. } | C2S::FileMkdir { channel, .. } => *channel == NO_CHANNEL,
        C2S::MoveClient { channel, .. } | C2S::SetChannelGroup { channel, .. } => *channel == NO_CHANNEL,
        C2S::CreatePrivilegeKey { channel: Some(c), .. } => *c == NO_CHANNEL,
        C2S::EditChannel { id, .. } | C2S::DeleteChannel { id, .. } => *id == NO_CHANNEL,
        C2S::CloneChannel { source, .. } => *source == NO_CHANNEL,
        _ => false,
    }
}

/// The person a message acts on, if any.
fn names_client(msg: &C2S) -> Option<ClientId> {
    match msg {
        C2S::Poke { target, .. }
        | C2S::Kick { target, .. }
        | C2S::MoveClient { target, .. }
        | C2S::SetTalker { target, .. }
        | C2S::EditClientDescription { target, .. }
        | C2S::BanWith { target, .. } => Some(*target),
        C2S::Ban { target: BanTarget::Client(t), .. } => Some(*t),
        C2S::WatchStream { streamer, .. } => Some(*streamer),
        C2S::SendText { target: TextTarget::Private(t), .. } => Some(*t),
        _ => None,
    }
}

/// A picture someone wants as their avatar: a small square-ish PNG or JPEG.
fn check_avatar_image(data: &[u8]) -> Res {
    if data.len() > MAX_AVATAR_BYTES {
        return Err(format!("The picture is too big (at most {} KB)", MAX_AVATAR_BYTES / 1024));
    }
    if !matches!(image_kind(data), Some("png") | Some("jpg")) {
        return Err("The picture must be a PNG or JPEG".into());
    }
    match image_dims(data) {
        Some((w, h)) if (16..=MAX_AVATAR_DIM).contains(&w) && (16..=MAX_AVATAR_DIM).contains(&h) => Ok(()),
        _ => Err(format!("The picture must be between 16 and {MAX_AVATAR_DIM} pixels wide and high")),
    }
}

fn new_privilege_key() -> String {
    let b: [u8; 24] = crate::secrets::random_bytes();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// Constant-time comparison of two secrets.
fn ct_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    let diff = a.iter().zip(b).fold(0u8, |d, (x, y)| d | (x ^ y));
    std::hint::black_box(diff) == 0
}

pub fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

/// Characters that are invisible or change the direction of text: they let names look
/// like other names (or hide parts of them).
fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200B}' | '\u{200C}' | '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2064}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}' | '\u{00AD}')
}

/// One line of user text (names, topics, reasons): no control or invisible characters,
/// trimmed, at most `max` characters.
pub fn clean_line(s: &str, max: usize) -> String {
    let s: String = s.chars().map(|c| if c == '\t' { ' ' } else { c }).filter(|c| !c.is_control() && !is_invisible(*c)).collect();
    truncate(s.trim(), max)
}

/// Multi-line user text (descriptions, welcome messages): like `clean_line`, but keeps
/// line breaks and tabs.
pub fn clean_text(s: &str, max: usize) -> String {
    let s: String = s.replace("\r\n", "\n").chars().filter(|c| (*c == '\n' || *c == '\t' || !c.is_control()) && !is_invisible(*c)).collect();
    truncate(&s, max)
}

fn sanitize_nick(n: &str) -> String {
    let n = clean_line(n, 30);
    if n.chars().count() < 1 {
        "Voxa User".into()
    } else {
        n
    }
}
