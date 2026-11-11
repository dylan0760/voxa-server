use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
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

pub struct Session {
    pub info: ClientInfo,
    pub tx: mpsc::UnboundedSender<Out>,
    pub close: Arc<Notify>,
    pub ip: IpAddr,
    pub voice: Arc<VoiceCipher>,
    pub voice_addr: Option<SocketAddr>,
    pub replay: ReplayWindow,
    pub s2c_seq: u64,
    pub bulk_token: [u8; 32],
    pub bulk_tx: Option<mpsc::Sender<BulkS2C>>,
    /// Clients currently watching this client's screen share.
    pub viewers: HashSet<ClientId>,
    pub last_keyframe_req: Option<Instant>,
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
}

pub struct Server {
    pub state: Mutex<State>,
    pub store: Store,
    pub cfg: Config,
    pub data_dir: PathBuf,
    pub identity: KeyPair,
    pub fingerprint: String,
    pub started: u64,
}

pub type Res<T = ()> = Result<T, String>;

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
        | CHANNEL_COMMANDER;
    let g = |id, kind, name: &str, perms, talk_power, sort, show_name| GroupInfo {
        id,
        kind,
        name: name.into(),
        perms,
        talk_power,
        sort,
        builtin: true,
        show_name,
    };
    vec![
        g(GROUP_ADMIN, GroupKind::Server, "Server Admin", ALL, 75, 10, true),
        g(GROUP_NORMAL, GroupKind::Server, "Normal", normal, 0, 20, false),
        g(GROUP_GUEST, GroupKind::Server, "Guest", SERVER_CHAT | CHANNEL_CHAT | CHAT_HISTORY | FILE_BROWSE, 0, 30, false),
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
                | CHAT_DELETE,
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
            let _ = s.tx.send(Out::Msg(msg));
        }
    }

    pub fn broadcast(&self, msg: S2C) {
        for s in self.sessions.values() {
            let _ = s.tx.send(Out::Msg(msg.clone()));
        }
    }

    pub fn broadcast_channel(&self, ch: ChannelId, msg: S2C) {
        for s in self.sessions.values().filter(|s| s.info.channel == ch) {
            let _ = s.tx.send(Out::Msg(msg.clone()));
        }
    }

    fn broadcast_client(&self, id: ClientId) {
        if let Some(s) = self.sessions.get(&id) {
            self.broadcast(S2C::ClientUpdated(s.info.clone()));
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

    fn session(&self, id: ClientId) -> Res<&Session> {
        self.sessions.get(&id).ok_or_else(|| "Client not found".to_string())
    }

    fn group_perms(&self, g: GroupId) -> u64 {
        self.groups.get(&g).map(|g| g.perms).unwrap_or(0)
    }

    pub fn server_perms(&self, s: &Session) -> u64 {
        s.info.server_groups.iter().fold(0, |acc, g| acc | self.group_perms(*g))
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

    pub fn can_talk(&self, s: &Session) -> bool {
        if s.info.input_muted {
            return false;
        }
        let needed = self.channels.get(&s.info.channel).map(|c| c.info.needed_talk_power).unwrap_or(0);
        s.info.is_talker || self.talk_power(s) >= needed
    }

    fn children(&self, ch: ChannelId) -> Vec<ChannelId> {
        self.channels.values().filter(|c| c.info.parent == Some(ch)).map(|c| c.info.id).collect()
    }

    /// The channel and all of its descendants, deepest first.
    fn subtree(&self, ch: ChannelId) -> Vec<ChannelId> {
        let mut out = Vec::new();
        for c in self.children(ch) {
            out.extend(self.subtree(c));
        }
        out.push(ch);
        out
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
                voice_encrypted: true,
            };
            store.save_channel(&StoredChannel { info: info.clone(), password_hash: None })?;
            channels.insert(id, ChannelRec { info, password_hash: None });
        }
        let srv = Server {
            state: Mutex::new(State { settings, channels, groups, sessions: HashMap::new() }),
            store,
            cfg,
            data_dir,
            identity,
            fingerprint,
            started: now_unix(),
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
        info!("------------------------------------------------------------------");
        info!(" Server Admin privilege key: {key}");
        info!(" (also saved to {})", path.display());
        info!("------------------------------------------------------------------");
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

    // ------------------------------------------------------------------ login

    #[allow(clippy::too_many_arguments)]
    pub fn login(
        &self,
        uid: String,
        ip: IpAddr,
        hello: C2S,
        tx: mpsc::UnboundedSender<Out>,
        close: Arc<Notify>,
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
        let mut st = self.state.lock();
        if let Some(ban) = self.find_ban(Some(&uid), Some(ip)) {
            let until = ban.expires.map(|e| format!(" (expires in {} min)", (e.saturating_sub(now_unix())) / 60 + 1)).unwrap_or_default();
            return Err(format!("You are banned from this server: {}{until}", ban.reason));
        }
        if let Some(h) = &st.settings.password_hash {
            if !server_password.as_deref().map(|p| verify_password(p, h)).unwrap_or(false) {
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
        if st.sessions.len() as u32 >= st.settings.max_clients && group_perms & perm::IGNORE_SERVER_MAXCLIENTS == 0 {
            return Err("The server is full".into());
        }
        if st.sessions.values().any(|s| s.info.uid == uid) {
            // Same identity connected twice: drop the stale session (TeamSpeak-like reconnect).
            if let Some(old) = st.sessions.values().find(|s| s.info.uid == uid).map(|s| s.info.id) {
                drop(st);
                self.kick_session(old, LeaveReason::Disconnected("Reconnected".into()));
                st = self.state.lock();
            }
        }
        let nickname = self.unique_nickname(&st, &sanitize_nick(&nickname), None);
        let id = (1..=u16::MAX).find(|i| !st.sessions.contains_key(i)).ok_or("No free client slots")?;

        // Default channel, optionally by path.
        let mut channel = st.default_channel();
        if let Some(path) = default_channel.filter(|p| !p.trim().is_empty()) {
            if let Some(ch) = find_channel_by_path(&st, &path) {
                let c = &st.channels[&ch];
                let pw_ok = match &c.password_hash {
                    None => true,
                    Some(h) => default_channel_password.as_deref().map(|p| verify_password(p, h)).unwrap_or(false)
                        || group_perms & perm::CHANNEL_JOIN_IGNORE_PASSWORD != 0,
                };
                let full = c.info.max_clients.map(|m| st.clients_in(ch) as u32 >= m).unwrap_or(false)
                    && group_perms & perm::CHANNEL_JOIN_IGNORE_MAXCLIENTS == 0;
                if pw_ok && !full {
                    channel = ch;
                }
            }
        }
        let channel_group = self.channel_group_for(&st, &uid, channel);

        record.nickname = nickname.clone();
        record.last_seen = now_unix();
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
        let welcome = S2C::Welcome {
            your_id: id,
            your_uid: uid.clone(),
            server: st.server_info(self),
            channels: st.channels.values().map(|c| c.info.clone()).collect(),
            clients: st.sessions.values().map(|s| s.info.clone()).chain(std::iter::once(info.clone())).collect(),
            groups: st.groups.values().cloned().collect(),
            voice: VoiceSetup { key: voice_key, udp_port: self.cfg.port },
            bulk_token,
        };
        let _ = tx.send(Out::Msg(welcome));
        st.broadcast(S2C::ClientJoined(info.clone()));
        info!("client {} ({}) connected from {ip} as id {id}", info.nickname, uid);
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
                viewers: HashSet::new(),
                last_keyframe_req: None,
            },
        );
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

    pub fn attach_bulk(&self, token: &[u8; 32], tx: mpsc::Sender<BulkS2C>) -> Option<ClientId> {
        let mut st = self.state.lock();
        let s = st.sessions.values_mut().find(|s| &s.bulk_token == token)?;
        s.bulk_tx = Some(tx);
        Some(s.info.id)
    }

    pub fn detach_bulk(&self, id: ClientId) {
        let mut st = self.state.lock();
        if let Some(s) = st.sessions.get_mut(&id) {
            s.bulk_tx = None;
        }
    }

    // ------------------------------------------------------------------ disconnect

    /// Removes a session and tells everyone. Safe to call more than once.
    pub fn remove_session(&self, id: ClientId, reason: LeaveReason) {
        let mut st = self.state.lock();
        let Some(s) = st.sessions.remove(&id) else { return };
        info!("client {} (id {id}) left: {reason:?}", s.info.nickname);
        for other in st.sessions.values_mut() {
            other.viewers.remove(&id);
        }
        if let Ok(Some(mut rec)) = self.store.get_client(&s.info.uid) {
            rec.last_seen = now_unix();
            let _ = self.store.save_client(&rec);
        }
        st.broadcast(S2C::ClientLeft { id, reason });
        self.cleanup_temporary(&mut st, s.info.channel);
    }

    /// Sends a final message, closes the connection and removes the session.
    pub fn kick_session(&self, id: ClientId, reason: LeaveReason) {
        {
            let st = self.state.lock();
            if let Some(s) = st.sessions.get(&id) {
                let _ = s.tx.send(Out::Msg(S2C::Kicked(reason.clone())));
                let _ = s.tx.send(Out::Close);
                s.close.notify_one();
            }
        }
        self.remove_session(id, reason);
    }

    fn cleanup_temporary(&self, st: &mut State, ch: ChannelId) {
        let Some(c) = st.channels.get(&ch) else { return };
        if c.info.kind != ChannelKind::Temporary || st.clients_in(ch) > 0 || !st.children(ch).is_empty() {
            return;
        }
        let parent = c.info.parent;
        st.channels.remove(&ch);
        let _ = self.store.delete_channel(ch);
        crate::files::delete_channel_files(&self.data_dir, ch);
        st.broadcast(S2C::ChannelDeleted(ch));
        if let Some(p) = parent {
            self.cleanup_temporary(st, p);
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
        match msg {
            C2S::Hello { .. } | C2S::AttachBulk(_) => Err("Unexpected message".into()),
            C2S::Ping(t) => {
                st.send(me, S2C::Pong(t));
                Ok(())
            }
            C2S::Disconnect(_) => Ok(()),
            C2S::UpdateSelf(u) => self.update_self(st, me, u),
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
                let s = st.session(me)?;
                self.need(st.server_perms(s), perm::CLIENT_POKE)?;
                let name = s.info.nickname.clone();
                st.session(target)?;
                st.send(target, S2C::Poked { from: me, from_name: name, message: truncate(&message, 500) });
                Ok(())
            }
            C2S::Kick { target, from_server, reason } => {
                let s = st.session(me)?;
                let t = st.session(target)?;
                let by = s.info.nickname.clone();
                self.check_outranks(st, s, t)?;
                let reason = truncate(&reason, 200);
                if from_server {
                    self.need(st.server_perms(s), perm::CLIENT_KICK_SERVER)?;
                    kicks.push((target, LeaveReason::Kicked { by, reason }));
                } else {
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
                if !st.channels.contains_key(&channel) {
                    return Err("Channel not found".into());
                }
                let by = s.info.nickname.clone();
                self.move_client(st, target, channel, Some(by));
                Ok(())
            }
            C2S::Ban { target, duration_secs, reason } => self.ban(st, me, target, duration_secs, reason, kicks),
            C2S::BanList => {
                self.need(st.server_perms(st.session(me)?), perm::BAN_LIST)?;
                st.send(me, S2C::BanList(db(self.store.bans())?));
                Ok(())
            }
            C2S::Unban(id) => {
                self.need(st.server_perms(st.session(me)?), perm::BAN_LIST)?;
                db(self.store.remove_ban(id))?;
                st.send(me, S2C::BanList(db(self.store.bans())?));
                Ok(())
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
                t.info.description = truncate(&description, 200);
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
                self.need(st.server_perms(st.session(me)?), perm::GROUP_MANAGE)?;
                let name = truncate(name.trim(), 40);
                if name.is_empty() {
                    return Err("Group name required".into());
                }
                let (perms, tp) = copy_from.and_then(|g| st.groups.get(&g)).map(|g| (g.perms, g.talk_power)).unwrap_or((0, 0));
                let g = GroupInfo {
                    id: db(self.store.next_id("next_group"))?,
                    kind,
                    name,
                    perms,
                    talk_power: tp,
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
                g.name = truncate(g.name.trim(), 40);
                if g.name.is_empty() {
                    return Err("Group name required".into());
                }
                if old.id == GROUP_ADMIN {
                    g.perms = perm::ALL;
                }
                // Nobody can hand out permissions they do not have themselves.
                let added = g.perms & !old.perms;
                if added & !mine != 0 {
                    return Err("You cannot grant permissions you do not have".into());
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
                let online: HashSet<&str> = st.sessions.values().map(|s| s.info.uid.as_str()).collect();
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
                        description: truncate(&description, 100),
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
                self.need(self.perms_in(st, s, s.info.channel), perm::SCREEN_SHARE)?;
                info.title = truncate(&info.title, 80);
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
            C2S::WatchStream { streamer, watch } => {
                let my_channel = st.session(me)?.info.channel;
                let t = st.sessions.get_mut(&streamer).ok_or("Client not found")?;
                if watch {
                    if t.info.streaming.is_none() {
                        return Err("This client is not sharing their screen".into());
                    }
                    if t.info.channel != my_channel {
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
    fn check_outranks(&self, st: &State, actor: &Session, target: &Session) -> Res {
        let a = st.server_perms(actor);
        let t = st.server_perms(target);
        if t & perm::SERVER_EDIT != 0 && a & perm::SERVER_EDIT == 0 {
            return Err("You cannot do that to a server administrator".into());
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
            s.info.description = truncate(&d, 200);
        }
        if let Some(a) = u.away {
            s.info.away = a.map(|m| truncate(&m, 80));
        }
        if let Some(m) = u.input_muted {
            s.info.input_muted = m;
        }
        if let Some(m) = u.output_muted {
            s.info.output_muted = m;
        }
        if let Some(r) = u.talk_request {
            s.info.talk_request = r.map(|m| truncate(&m, 80));
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
        let p = self.perms_in(st, s, ch);
        if let Some(h) = &c.password_hash {
            if p & perm::CHANNEL_JOIN_IGNORE_PASSWORD == 0 && !password.map(|pw| verify_password(pw, h)).unwrap_or(false) {
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
        let cg = self.channel_group_for(st, &uid, ch);
        let s = st.sessions.get_mut(&id).unwrap();
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
        st.broadcast(S2C::ClientMoved { id, channel: ch, by });
        st.broadcast_client(id);
        self.cleanup_temporary(st, old);
    }

    fn validate_channel_edit(&self, st: &State, id: Option<ChannelId>, e: &mut ChannelEdit) -> Res {
        e.name = truncate(e.name.trim(), 40);
        if e.name.is_empty() {
            return Err("Channel name required".into());
        }
        if e.name.contains('/') {
            return Err("Channel names cannot contain '/'".into());
        }
        e.topic = truncate(&e.topic, 255);
        e.description = truncate(&e.description, 8192);
        e.codec_quality = e.codec_quality.min(10);
        if let Some(p) = e.parent {
            if !st.channels.contains_key(&p) {
                return Err("Parent channel not found".into());
            }
            if let Some(id) = id {
                if st.subtree(id).contains(&p) {
                    return Err("A channel cannot be moved below itself".into());
                }
            }
        }
        let dup = st
            .channels
            .values()
            .any(|c| Some(c.info.id) != id && c.info.parent == e.parent && c.info.name.eq_ignore_ascii_case(&e.name));
        if dup {
            return Err("A channel with this name already exists".into());
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
        self.validate_channel_edit(st, None, &mut e)?;
        let uid = s.info.uid.clone();
        let id = db(self.store.next_id("next_channel"))?;
        if e.sort == 0 {
            e.sort = st.channels.values().filter(|c| c.info.parent == e.parent).map(|c| c.info.sort).max().unwrap_or(0) + 1;
        }
        let password_hash = match &e.password {
            PasswordChange::Set(pw) if !pw.is_empty() => Some(hash_password(pw)),
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
            voice_encrypted: e.voice_encrypted,
        };
        let rec = ChannelRec { info: info.clone(), password_hash };
        if rec.info.kind == ChannelKind::Permanent {
            self.persist_channel(&rec)?;
        }
        st.channels.insert(id, rec);
        st.broadcast(S2C::ChannelCreated(info));
        // The creator becomes channel admin and is moved into the new channel.
        let admin = st.settings.channel_admin_group;
        db(self.store.set_channel_group(&uid, id, Some(admin)))?;
        self.move_client(st, me, id, None);
        Ok(())
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
            PasswordChange::Set(pw) => c.password_hash = Some(hash_password(pw)),
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
            st.broadcast(S2C::ClientMoved { id: c, channel: def, by: Some(by.clone()) });
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
        let s = st.session(me)?;
        let from_uid = s.info.uid.clone();
        let from_name = s.info.nickname.clone();
        let ts = now_unix();
        match target {
            TextTarget::Server => {
                self.need(st.server_perms(s), perm::SERVER_CHAT)?;
                let stored = StoredMessage { from_uid: from_uid.clone(), from_name: from_name.clone(), ts, text: text.clone() };
                let id = db(self.store.append_chat(SERVER_CHAT_CHANNEL, &stored))?;
                st.broadcast(S2C::Text(ChatMessage { id, target, from: Some(me), from_uid, from_name, ts, text }));
            }
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
        }
        Ok(())
    }

    fn history_channel(&self, st: &State, s: &Session, target: TextTarget) -> Res<ChannelId> {
        match target {
            TextTarget::Server => {
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
            TextTarget::Private(_) => Err("Private conversations are not stored".into()),
        }
    }

    fn fetch_history(&self, st: &mut State, me: ClientId, target: TextTarget, before: Option<MessageId>, limit: u32) -> Res {
        let s = st.session(me)?;
        let ch = self.history_channel(st, s, target)?;
        let limit = limit.clamp(1, 200) as usize;
        let rows = db(self.store.history(ch, before, limit))?;
        let online: HashMap<&str, ClientId> = st.sessions.values().map(|s| (s.info.uid.as_str(), s.info.id)).collect();
        let complete = rows.len() < limit;
        let messages = rows
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
        st.send(me, S2C::History { target, messages, complete });
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
        match target {
            TextTarget::Channel(c) => st.broadcast_channel(c, S2C::MessageDeleted { target, id }),
            _ => st.broadcast(S2C::MessageDeleted { target, id }),
        }
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
        let reason = truncate(&reason, 200);
        let (uid, ip, name) = match &target {
            BanTarget::Client(id) => {
                let t = st.session(*id)?;
                self.check_outranks(st, s, t)?;
                (Some(t.info.uid.clone()), Some(t.ip.to_string()), Some(t.info.nickname.clone()))
            }
            BanTarget::Uid(u) => (Some(u.clone()), None, None),
            BanTarget::Ip(i) => {
                let ip: IpAddr = i.trim().parse().map_err(|_| "Invalid IP address".to_string())?;
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
        let s = &mut st.settings;
        s.name = truncate(e.name.trim(), 64);
        if s.name.is_empty() {
            s.name = "Voxa Server".into();
        }
        s.welcome_message = truncate(&e.welcome_message, 4000);
        s.host_message = truncate(&e.host_message, 1000);
        s.max_clients = e.max_clients.clamp(1, 1024);
        s.default_server_group = e.default_server_group;
        s.default_channel_group = e.default_channel_group;
        s.channel_admin_group = e.channel_admin_group;
        s.chat_retention_days = e.chat_retention_days;
        match e.password {
            PasswordChange::Keep => {}
            PasswordChange::Clear => s.password_hash = None,
            PasswordChange::Set(p) if p.is_empty() => s.password_hash = None,
            PasswordChange::Set(p) => s.password_hash = Some(hash_password(&p)),
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
    }

    // ------------------------------------------------------------------ screen share

    pub fn relay_frame(&self, from: ClientId, frame: StreamFrame) {
        let mut st = self.state.lock();
        let Some(s) = st.sessions.get(&from) else { return };
        if s.info.streaming.is_none() {
            return;
        }
        let viewers: Vec<ClientId> = s.viewers.iter().copied().collect();
        let mut dropped = false;
        for v in viewers {
            if let Some(b) = st.sessions.get(&v).and_then(|v| v.bulk_tx.as_ref()) {
                if b.try_send(BulkS2C::Frame { from, frame: frame.clone() }).is_err() {
                    dropped = true;
                }
            }
        }
        if dropped {
            self.request_keyframe_locked(&mut st, from);
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

fn new_privilege_key() -> String {
    let b: [u8; 24] = crate::secrets::random_bytes();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

pub fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn sanitize_nick(n: &str) -> String {
    let n: String = n.chars().filter(|c| !c.is_control()).collect();
    let n = truncate(n.trim(), 30);
    if n.chars().count() < 1 {
        "Voxa User".into()
    } else {
        n
    }
}
