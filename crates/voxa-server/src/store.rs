//! Persistent storage (redb). Chat messages are encrypted at rest with
//! XChaCha20-Poly1305 under the server master key; each record is bound to its
//! (channel, message id) position through the AEAD associated data.

use std::path::Path;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use voxa_proto::*;

const META: TableDefinition<&str, &[u8]> = TableDefinition::new("meta");
const CHANNELS: TableDefinition<u64, &[u8]> = TableDefinition::new("channels");
const GROUPS: TableDefinition<u64, &[u8]> = TableDefinition::new("groups");
const CLIENTS: TableDefinition<&str, &[u8]> = TableDefinition::new("clients");
const CHANNEL_GROUPS: TableDefinition<(&str, u64), u64> = TableDefinition::new("channel_groups");
const BANS: TableDefinition<u64, &[u8]> = TableDefinition::new("bans");
const PRIVKEYS: TableDefinition<&str, &[u8]> = TableDefinition::new("privilege_keys");
const CHAT: TableDefinition<(u64, u64), &[u8]> = TableDefinition::new("chat");
/// Per-user permission bits granted on top of the user's groups.
const CLIENT_PERMS: TableDefinition<&str, u64> = TableDefinition::new("client_perms");
/// Chat images (1.5.0+): small sealed metadata and the sealed image bytes, kept apart
/// so clean-ups never have to decrypt whole images.
const IMAGE_META: TableDefinition<u64, &[u8]> = TableDefinition::new("image_meta");
const IMAGE_DATA: TableDefinition<u64, &[u8]> = TableDefinition::new("image_data");

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredSettings {
    pub name: String,
    pub welcome_message: String,
    pub host_message: String,
    pub max_clients: u32,
    pub password_hash: Option<String>,
    pub default_server_group: GroupId,
    pub default_channel_group: GroupId,
    pub channel_admin_group: GroupId,
    pub chat_retention_days: u32,
    pub created: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredChannel {
    pub info: ChannelInfo,
    pub password_hash: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ClientRecord {
    pub uid: String,
    pub nickname: String,
    pub description: String,
    pub server_groups: Vec<GroupId>,
    pub first_seen: u64,
    pub last_seen: u64,
    pub connections: u64,
    pub last_ip: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct PrivKey {
    pub group: GroupId,
    pub channel: Option<ChannelId>,
    pub description: String,
    pub created: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StoredMessage {
    pub from_uid: String,
    pub from_name: String,
    pub ts: u64,
    pub text: String,
}

/// Where a chat image was posted and who may see it.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ImageMeta {
    /// Stored chat it belongs to (`SERVER_CHAT_CHANNEL` for the server chat), or None
    /// for a private conversation.
    pub ch: Option<ChannelId>,
    /// Message carrying the image (0 for private conversations).
    pub msg: MessageId,
    /// Participants of a private conversation.
    pub uids: Vec<String>,
    pub ts: u64,
    pub name: String,
}

pub struct Store {
    db: Database,
    cipher: XChaCha20Poly1305,
}

fn enc<T: Serialize>(v: &T) -> Vec<u8> {
    bincode::serialize(v).expect("serialize")
}

fn dec<T: DeserializeOwned>(b: &[u8]) -> Option<T> {
    bincode::deserialize(b).ok()
}

type R<T> = anyhow::Result<T>;

impl Store {
    pub fn open(path: &Path, master_key: &[u8]) -> R<Store> {
        let db = Database::create(path)?;
        {
            let w = db.begin_write()?;
            w.open_table(META)?;
            w.open_table(CHANNELS)?;
            w.open_table(GROUPS)?;
            w.open_table(CLIENTS)?;
            w.open_table(CHANNEL_GROUPS)?;
            w.open_table(BANS)?;
            w.open_table(PRIVKEYS)?;
            w.open_table(CHAT)?;
            w.open_table(CLIENT_PERMS)?;
            w.open_table(IMAGE_META)?;
            w.open_table(IMAGE_DATA)?;
            w.commit()?;
        }
        let cipher = XChaCha20Poly1305::new_from_slice(master_key).map_err(|_| anyhow::anyhow!("bad master key"))?;
        let store = Store { db, cipher };
        store.migrate()?;
        Ok(store)
    }

    /// Upgrades records written by older versions. Schema 1 (Voxa 1.0/1.1) had no
    /// join power on groups/channels and no "join channels" permission.
    fn migrate(&self) -> R<()> {
        const SCHEMA: u64 = 2;
        let current = {
            let r = self.db.begin_read()?;
            let t = r.open_table(META)?;
            let has_settings = t.get("settings")?.is_some();
            match t.get("schema")? {
                Some(v) => u64::from_le_bytes(v.value().try_into().unwrap_or([0; 8])),
                // A fresh database is created directly in the current schema.
                None if !has_settings => SCHEMA,
                None => 1,
            }
        };
        if current < 2 {
            #[derive(Deserialize)]
            struct GroupV1 {
                id: GroupId,
                kind: GroupKind,
                name: String,
                perms: u64,
                talk_power: i32,
                sort: i32,
                builtin: bool,
                show_name: bool,
            }
            #[derive(Deserialize)]
            struct ChannelInfoV1 {
                id: ChannelId,
                parent: Option<ChannelId>,
                sort: i64,
                name: String,
                topic: String,
                description: String,
                has_password: bool,
                max_clients: Option<u32>,
                kind: ChannelKind,
                is_default: bool,
                codec_quality: u8,
                needed_talk_power: i32,
                voice_encrypted: bool,
            }
            #[derive(Deserialize)]
            struct StoredChannelV1 {
                info: ChannelInfoV1,
                password_hash: Option<String>,
            }
            let w = self.db.begin_write()?;
            {
                let mut t = w.open_table(GROUPS)?;
                let rows: Vec<(u64, Vec<u8>)> = t.iter()?.filter_map(|r| r.ok()).map(|(k, v)| (k.value(), v.value().to_vec())).collect();
                for (k, v) in rows {
                    let Some(g) = dec::<GroupV1>(&v) else { continue };
                    let is_admin = g.perms & ((1u64 << 32) - 1) == (1u64 << 32) - 1;
                    let join_power = if is_admin { 75 } else { g.talk_power.max(if g.kind == GroupKind::Server && g.id == 2 { 10 } else { 0 }) };
                    // Everyone could join channels before; keep it that way.
                    let perms = if is_admin { perm::ALL } else if g.kind == GroupKind::Server { g.perms | perm::CHANNEL_JOIN } else { g.perms };
                    let ng = GroupInfo {
                        id: g.id,
                        kind: g.kind,
                        name: g.name,
                        perms,
                        talk_power: g.talk_power,
                        join_power,
                        sort: g.sort,
                        builtin: g.builtin,
                        show_name: g.show_name,
                    };
                    t.insert(k, enc(&ng).as_slice())?;
                }
                let mut t = w.open_table(CHANNELS)?;
                let rows: Vec<(u64, Vec<u8>)> = t.iter()?.filter_map(|r| r.ok()).map(|(k, v)| (k.value(), v.value().to_vec())).collect();
                for (k, v) in rows {
                    let Some(c) = dec::<StoredChannelV1>(&v) else { continue };
                    let i = c.info;
                    let nc = StoredChannel {
                        info: ChannelInfo {
                            id: i.id,
                            parent: i.parent,
                            sort: i.sort,
                            name: i.name,
                            topic: i.topic,
                            description: i.description,
                            has_password: i.has_password,
                            max_clients: i.max_clients,
                            kind: i.kind,
                            is_default: i.is_default,
                            codec_quality: i.codec_quality,
                            needed_talk_power: i.needed_talk_power,
                            needed_join_power: 0,
                            voice_encrypted: i.voice_encrypted,
                        },
                        password_hash: c.password_hash,
                    };
                    t.insert(k, enc(&nc).as_slice())?;
                }
            }
            w.open_table(META)?.insert("schema", SCHEMA.to_le_bytes().as_slice())?;
            w.commit()?;
            tracing::info!("database migrated to schema {SCHEMA}");
        } else if current == SCHEMA {
            let w = self.db.begin_write()?;
            w.open_table(META)?.insert("schema", SCHEMA.to_le_bytes().as_slice())?;
            w.commit()?;
        }
        Ok(())
    }

    // ---------- meta / settings ----------

    pub fn load_settings(&self) -> R<Option<StoredSettings>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(META)?;
        Ok(t.get("settings")?.and_then(|v| dec(v.value())))
    }

    pub fn save_settings(&self, s: &StoredSettings) -> R<()> {
        let w = self.db.begin_write()?;
        w.open_table(META)?.insert("settings", enc(s).as_slice())?;
        w.commit()?;
        Ok(())
    }

    /// File browser limits set by an administrator (None = never set, use server.toml).
    pub fn load_file_limits(&self) -> R<Option<FileLimits>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(META)?;
        Ok(t.get("file_limits")?.and_then(|v| dec(v.value())))
    }

    pub fn save_file_limits(&self, l: &FileLimits) -> R<()> {
        let w = self.db.begin_write()?;
        w.open_table(META)?.insert("file_limits", enc(l).as_slice())?;
        w.commit()?;
        Ok(())
    }

    pub fn next_id(&self, name: &str) -> R<u64> {
        let w = self.db.begin_write()?;
        let id = {
            let mut t = w.open_table(META)?;
            let cur = t
                .get(name)?
                .map(|v| u64::from_le_bytes(v.value().try_into().unwrap_or([0; 8])))
                .unwrap_or(1);
            t.insert(name, (cur + 1).to_le_bytes().as_slice())?;
            cur
        };
        w.commit()?;
        Ok(id)
    }

    // ---------- generic helpers ----------

    fn all_u64<T: DeserializeOwned>(&self, def: TableDefinition<u64, &[u8]>) -> R<Vec<T>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(def)?;
        let mut out = Vec::new();
        for row in t.iter()? {
            let (_, v) = row?;
            if let Some(x) = dec(v.value()) {
                out.push(x);
            }
        }
        Ok(out)
    }

    fn put_u64<T: Serialize>(&self, def: TableDefinition<u64, &[u8]>, k: u64, v: &T) -> R<()> {
        let w = self.db.begin_write()?;
        w.open_table(def)?.insert(k, enc(v).as_slice())?;
        w.commit()?;
        Ok(())
    }

    fn del_u64(&self, def: TableDefinition<u64, &[u8]>, k: u64) -> R<()> {
        let w = self.db.begin_write()?;
        w.open_table(def)?.remove(k)?;
        w.commit()?;
        Ok(())
    }

    // ---------- channels ----------

    pub fn load_channels(&self) -> R<Vec<StoredChannel>> {
        self.all_u64(CHANNELS)
    }

    pub fn save_channel(&self, c: &StoredChannel) -> R<()> {
        self.put_u64(CHANNELS, c.info.id, c)
    }

    /// Removes only the persisted channel definition (channel became non-permanent).
    pub fn delete_channel_record_only(&self, id: ChannelId) -> R<()> {
        self.del_u64(CHANNELS, id)
    }

    pub fn delete_channel(&self, id: ChannelId) -> R<()> {
        self.del_u64(CHANNELS, id)?;
        let w = self.db.begin_write()?;
        {
            let mut t = w.open_table(CHANNEL_GROUPS)?;
            let keys: Vec<(String, u64)> = t
                .iter()?
                .filter_map(|r| r.ok())
                .map(|(k, _)| {
                    let (u, c) = k.value();
                    (u.to_string(), c)
                })
                .filter(|(_, c)| *c == id)
                .collect();
            for (u, c) in keys {
                t.remove((u.as_str(), c))?;
            }
            let mut chat = w.open_table(CHAT)?;
            let keys: Vec<(u64, u64)> =
                chat.range((id, 0)..=(id, u64::MAX))?.filter_map(|r| r.ok()).map(|(k, _)| k.value()).collect();
            for k in keys {
                chat.remove(k)?;
            }
        }
        w.commit()?;
        self.delete_images_where(|m| m.ch == Some(id))?;
        Ok(())
    }

    // ---------- groups ----------

    pub fn load_groups(&self) -> R<Vec<GroupInfo>> {
        self.all_u64(GROUPS)
    }

    pub fn save_group(&self, g: &GroupInfo) -> R<()> {
        self.put_u64(GROUPS, g.id, g)
    }

    pub fn delete_group(&self, id: GroupId) -> R<()> {
        self.del_u64(GROUPS, id)
    }

    // ---------- clients ----------

    pub fn get_client(&self, uid: &str) -> R<Option<ClientRecord>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(CLIENTS)?;
        Ok(t.get(uid)?.and_then(|v| dec(v.value())))
    }

    pub fn save_client(&self, c: &ClientRecord) -> R<()> {
        let w = self.db.begin_write()?;
        w.open_table(CLIENTS)?.insert(c.uid.as_str(), enc(c).as_slice())?;
        w.commit()?;
        Ok(())
    }

    pub fn all_clients(&self) -> R<Vec<ClientRecord>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(CLIENTS)?;
        let mut out = Vec::new();
        for row in t.iter()? {
            let (_, v) = row?;
            if let Some(x) = dec(v.value()) {
                out.push(x);
            }
        }
        Ok(out)
    }

    pub fn channel_group(&self, uid: &str, ch: ChannelId) -> R<Option<GroupId>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(CHANNEL_GROUPS)?;
        Ok(t.get((uid, ch))?.map(|v| v.value()))
    }

    pub fn set_channel_group(&self, uid: &str, ch: ChannelId, g: Option<GroupId>) -> R<()> {
        let w = self.db.begin_write()?;
        {
            let mut t = w.open_table(CHANNEL_GROUPS)?;
            match g {
                Some(g) => {
                    t.insert((uid, ch), g)?;
                }
                None => {
                    t.remove((uid, ch))?;
                }
            }
        }
        w.commit()?;
        Ok(())
    }

    pub fn channel_group_members(&self, g: GroupId) -> R<Vec<(String, ChannelId)>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(CHANNEL_GROUPS)?;
        let mut out = Vec::new();
        for row in t.iter()? {
            let (k, v) = row?;
            if v.value() == g {
                let (u, c) = k.value();
                out.push((u.to_string(), c));
            }
        }
        Ok(out)
    }

    // ---------- bans ----------

    pub fn bans(&self) -> R<Vec<BanEntry>> {
        self.all_u64(BANS)
    }

    pub fn add_ban(&self, mut b: BanEntry) -> R<BanEntry> {
        b.id = self.next_id("next_ban")?;
        self.put_u64(BANS, b.id, &b)?;
        Ok(b)
    }

    pub fn remove_ban(&self, id: u64) -> R<()> {
        self.del_u64(BANS, id)
    }

    // ---------- privilege keys ----------

    pub fn add_privkey(&self, key: &str, p: &PrivKey) -> R<()> {
        let w = self.db.begin_write()?;
        w.open_table(PRIVKEYS)?.insert(key, enc(p).as_slice())?;
        w.commit()?;
        Ok(())
    }

    pub fn take_privkey(&self, key: &str) -> R<Option<PrivKey>> {
        let w = self.db.begin_write()?;
        let p = {
            let mut t = w.open_table(PRIVKEYS)?;
            let v = t.remove(key)?.and_then(|v| dec::<PrivKey>(v.value()));
            v
        };
        w.commit()?;
        Ok(p)
    }

    pub fn privkey_for_group(&self, g: GroupId) -> R<Option<String>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(PRIVKEYS)?;
        for row in t.iter()? {
            let (k, v) = row?;
            if let Some(p) = dec::<PrivKey>(v.value()) {
                if p.group == g && p.channel.is_none() {
                    return Ok(Some(k.value().to_string()));
                }
            }
        }
        Ok(None)
    }

    // ---------- chat (encrypted at rest) ----------

    fn seal_raw(&self, aad: &[u8], pt: &[u8]) -> Vec<u8> {
        let nonce: [u8; 24] = crate::secrets::random_bytes();
        let ct = self.cipher.encrypt(XNonce::from_slice(&nonce), Payload { msg: pt, aad }).expect("encrypt");
        [nonce.as_slice(), &ct].concat()
    }

    fn unseal_raw(&self, aad: &[u8], data: &[u8]) -> Option<Vec<u8>> {
        if data.len() < 24 {
            return None;
        }
        self.cipher.decrypt(XNonce::from_slice(&data[..24]), Payload { msg: &data[24..], aad }).ok()
    }

    fn image_aad(kind: &[u8], id: u64) -> Vec<u8> {
        [b"voxa-image-".as_slice(), kind, &id.to_be_bytes()].concat()
    }

    pub fn new_image_id(&self) -> R<u64> {
        self.next_id("next_image")
    }

    pub fn put_image(&self, id: u64, meta: &ImageMeta, data: &[u8]) -> R<()> {
        let m = self.seal_raw(&Self::image_aad(b"meta", id), &enc(meta));
        let d = self.seal_raw(&Self::image_aad(b"data", id), data);
        let w = self.db.begin_write()?;
        w.open_table(IMAGE_META)?.insert(id, m.as_slice())?;
        w.open_table(IMAGE_DATA)?.insert(id, d.as_slice())?;
        w.commit()?;
        Ok(())
    }

    pub fn image_meta(&self, id: u64) -> R<Option<ImageMeta>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(IMAGE_META)?;
        Ok(t.get(id)?.and_then(|v| self.unseal_raw(&Self::image_aad(b"meta", id), v.value())).and_then(|p| dec(&p)))
    }

    pub fn image_data(&self, id: u64) -> R<Option<Vec<u8>>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(IMAGE_DATA)?;
        Ok(t.get(id)?.and_then(|v| self.unseal_raw(&Self::image_aad(b"data", id), v.value())))
    }

    /// Deletes every image whose metadata matches `doomed`. Returns the count.
    pub fn delete_images_where(&self, doomed: impl Fn(&ImageMeta) -> bool) -> R<usize> {
        let ids: Vec<u64> = {
            let r = self.db.begin_read()?;
            let t = r.open_table(IMAGE_META)?;
            let mut v = Vec::new();
            for row in t.iter()? {
                let (k, val) = row?;
                let id = k.value();
                match self.unseal_raw(&Self::image_aad(b"meta", id), val.value()).and_then(|p| dec::<ImageMeta>(&p)) {
                    Some(m) if doomed(&m) => v.push(id),
                    // Unreadable metadata: nobody could ever be allowed to see it.
                    None => v.push(id),
                    _ => {}
                }
            }
            v
        };
        if ids.is_empty() {
            return Ok(0);
        }
        let w = self.db.begin_write()?;
        {
            let mut m = w.open_table(IMAGE_META)?;
            let mut d = w.open_table(IMAGE_DATA)?;
            for id in &ids {
                m.remove(*id)?;
                d.remove(*id)?;
            }
        }
        w.commit()?;
        Ok(ids.len())
    }

    fn seal(&self, ch: u64, id: u64, m: &StoredMessage) -> Vec<u8> {
        let nonce: [u8; 24] = crate::secrets::random_bytes();
        let mut aad = [0u8; 16];
        aad[..8].copy_from_slice(&ch.to_be_bytes());
        aad[8..].copy_from_slice(&id.to_be_bytes());
        let ct = self
            .cipher
            .encrypt(XNonce::from_slice(&nonce), Payload { msg: &enc(m), aad: &aad })
            .expect("encrypt");
        [nonce.as_slice(), &ct].concat()
    }

    fn unseal(&self, ch: u64, id: u64, data: &[u8]) -> Option<StoredMessage> {
        if data.len() < 24 {
            return None;
        }
        let mut aad = [0u8; 16];
        aad[..8].copy_from_slice(&ch.to_be_bytes());
        aad[8..].copy_from_slice(&id.to_be_bytes());
        let pt = self
            .cipher
            .decrypt(XNonce::from_slice(&data[..24]), Payload { msg: &data[24..], aad: &aad })
            .ok()?;
        dec(&pt)
    }

    pub fn append_chat(&self, ch: ChannelId, m: &StoredMessage) -> R<MessageId> {
        let id = self.next_id("next_message")?;
        let blob = self.seal(ch, id, m);
        let w = self.db.begin_write()?;
        w.open_table(CHAT)?.insert((ch, id), blob.as_slice())?;
        w.commit()?;
        Ok(id)
    }

    /// Returns up to `limit` messages older than `before`, oldest first.
    pub fn history(&self, ch: ChannelId, before: Option<MessageId>, limit: usize) -> R<Vec<(MessageId, StoredMessage)>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(CHAT)?;
        let end = before.unwrap_or(u64::MAX);
        let mut out = Vec::new();
        for row in t.range((ch, 0)..(ch, end))?.rev() {
            let (k, v) = row?;
            let (_, id) = k.value();
            if let Some(m) = self.unseal(ch, id, v.value()) {
                out.push((id, m));
            }
            if out.len() >= limit {
                break;
            }
        }
        out.reverse();
        Ok(out)
    }

    pub fn get_chat(&self, ch: ChannelId, id: MessageId) -> R<Option<StoredMessage>> {
        let r = self.db.begin_read()?;
        let t = r.open_table(CHAT)?;
        Ok(t.get((ch, id))?.and_then(|v| self.unseal(ch, id, v.value())))
    }

    /// Deletes every stored message of a channel (or the server chat). Returns the count.
    pub fn clear_chat(&self, ch: ChannelId) -> R<usize> {
        let w = self.db.begin_write()?;
        let n = {
            let mut t = w.open_table(CHAT)?;
            let keys: Vec<(u64, u64)> = t.range((ch, 0)..=(ch, u64::MAX))?.filter_map(|r| r.ok()).map(|(k, _)| k.value()).collect();
            for k in &keys {
                t.remove(*k)?;
            }
            keys.len()
        };
        w.commit()?;
        Ok(n)
    }

    pub fn client_perms(&self, uid: &str) -> R<u64> {
        let r = self.db.begin_read()?;
        let t = r.open_table(CLIENT_PERMS)?;
        Ok(t.get(uid)?.map(|v| v.value()).unwrap_or(0))
    }

    pub fn set_client_perms(&self, uid: &str, perms: u64) -> R<()> {
        let w = self.db.begin_write()?;
        {
            let mut t = w.open_table(CLIENT_PERMS)?;
            if perms == 0 {
                t.remove(uid)?;
            } else {
                t.insert(uid, perms)?;
            }
        }
        w.commit()?;
        Ok(())
    }

    pub fn delete_chat(&self, ch: ChannelId, id: MessageId) -> R<()> {
        let w = self.db.begin_write()?;
        w.open_table(CHAT)?.remove((ch, id))?;
        w.commit()?;
        Ok(())
    }

    /// Deletes all messages older than `cutoff` (unix seconds). Returns the count.
    pub fn purge_chat_before(&self, cutoff: u64) -> R<usize> {
        let doomed: Vec<(u64, u64)> = {
            let r = self.db.begin_read()?;
            let t = r.open_table(CHAT)?;
            let mut v = Vec::new();
            for row in t.iter()? {
                let (k, val) = row?;
                let (ch, id) = k.value();
                if let Some(m) = self.unseal(ch, id, val.value()) {
                    if m.ts < cutoff {
                        v.push((ch, id));
                    }
                }
            }
            v
        };
        if doomed.is_empty() {
            return Ok(0);
        }
        let w = self.db.begin_write()?;
        {
            let mut t = w.open_table(CHAT)?;
            for k in &doomed {
                t.remove(*k)?;
            }
        }
        w.commit()?;
        Ok(doomed.len())
    }
}
