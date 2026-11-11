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
            w.commit()?;
        }
        let cipher = XChaCha20Poly1305::new_from_slice(master_key).map_err(|_| anyhow::anyhow!("bad master key"))?;
        Ok(Store { db, cipher })
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
