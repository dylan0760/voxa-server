use serde::{Deserialize, Serialize};

pub type ClientId = u16;
pub type ChannelId = u64;
pub type GroupId = u64;
pub type MessageId = u64;

/// Channel id used for the server-wide chat in history storage.
pub const SERVER_CHAT_CHANNEL: ChannelId = 0;

/// Permission bits. Effective permissions are the union of all server groups and the
/// channel group of the client in its current channel.
pub mod perm {
    pub const SERVER_EDIT: u64 = 1 << 0;
    pub const CHANNEL_CREATE_PERMANENT: u64 = 1 << 1;
    pub const CHANNEL_CREATE_SEMI_PERMANENT: u64 = 1 << 2;
    pub const CHANNEL_CREATE_TEMPORARY: u64 = 1 << 3;
    pub const CHANNEL_EDIT: u64 = 1 << 4;
    pub const CHANNEL_DELETE: u64 = 1 << 5;
    pub const CHANNEL_JOIN_IGNORE_PASSWORD: u64 = 1 << 6;
    pub const CHANNEL_JOIN_IGNORE_MAXCLIENTS: u64 = 1 << 7;
    pub const CLIENT_MOVE: u64 = 1 << 8;
    pub const CLIENT_KICK_CHANNEL: u64 = 1 << 9;
    pub const CLIENT_KICK_SERVER: u64 = 1 << 10;
    pub const CLIENT_BAN: u64 = 1 << 11;
    pub const BAN_LIST: u64 = 1 << 12;
    pub const CLIENT_POKE: u64 = 1 << 13;
    pub const PRIVATE_MESSAGE: u64 = 1 << 14;
    pub const SERVER_CHAT: u64 = 1 << 15;
    pub const CHANNEL_CHAT: u64 = 1 << 16;
    pub const CHAT_DELETE: u64 = 1 << 17;
    pub const GROUP_MANAGE: u64 = 1 << 18;
    pub const GROUP_ASSIGN_SERVER: u64 = 1 << 19;
    pub const GROUP_ASSIGN_CHANNEL: u64 = 1 << 20;
    pub const TALK_POWER_GRANT: u64 = 1 << 21;
    pub const PRIVILEGE_KEY_CREATE: u64 = 1 << 22;
    pub const FILE_BROWSE: u64 = 1 << 23;
    pub const FILE_UPLOAD: u64 = 1 << 24;
    pub const FILE_DOWNLOAD: u64 = 1 << 25;
    pub const FILE_DELETE: u64 = 1 << 26;
    pub const SCREEN_SHARE: u64 = 1 << 27;
    pub const CHANNEL_COMMANDER: u64 = 1 << 28;
    pub const CHAT_HISTORY: u64 = 1 << 29;
    pub const CLIENT_DESCRIPTION_EDIT: u64 = 1 << 30;
    pub const IGNORE_SERVER_MAXCLIENTS: u64 = 1 << 31;
    /// Allowed to join / switch to channels at all.
    pub const CHANNEL_JOIN: u64 = 1 << 32;
    /// Join channels even when the group's join power is below the channel's needed join power.
    pub const CHANNEL_JOIN_IGNORE_POWER: u64 = 1 << 33;
    /// Wipe an entire channel / server chat, live and from the stored history.
    pub const CHAT_CLEAR: u64 = 1 << 34;

    pub const ALL: u64 = (1 << 35) - 1;

    pub const NAMES: &[(u64, &str)] = &[
        (SERVER_EDIT, "Edit virtual server settings"),
        (CHANNEL_CREATE_PERMANENT, "Create permanent channels"),
        (CHANNEL_CREATE_SEMI_PERMANENT, "Create semi-permanent channels"),
        (CHANNEL_CREATE_TEMPORARY, "Create temporary channels"),
        (CHANNEL_EDIT, "Edit channels"),
        (CHANNEL_DELETE, "Delete channels"),
        (CHANNEL_JOIN, "Join / switch channels"),
        (CHANNEL_JOIN_IGNORE_POWER, "Join channels regardless of join power"),
        (CHANNEL_JOIN_IGNORE_PASSWORD, "Join channels without password"),
        (CHANNEL_JOIN_IGNORE_MAXCLIENTS, "Join full channels"),
        (CLIENT_MOVE, "Move clients"),
        (CLIENT_KICK_CHANNEL, "Kick clients from channel"),
        (CLIENT_KICK_SERVER, "Kick clients from server"),
        (CLIENT_BAN, "Ban clients"),
        (BAN_LIST, "View and edit ban list"),
        (CLIENT_POKE, "Poke clients"),
        (PRIVATE_MESSAGE, "Send private messages"),
        (SERVER_CHAT, "Send server chat messages"),
        (CHANNEL_CHAT, "Send channel chat messages"),
        (CHAT_HISTORY, "Read chat history"),
        (CHAT_DELETE, "Delete anyone's chat messages (incl. history)"),
        (CHAT_CLEAR, "Clear entire chat history"),
        (GROUP_MANAGE, "Create, edit and delete groups"),
        (GROUP_ASSIGN_SERVER, "Assign server groups"),
        (GROUP_ASSIGN_CHANNEL, "Assign channel groups"),
        (TALK_POWER_GRANT, "Grant / revoke talk power"),
        (PRIVILEGE_KEY_CREATE, "Create privilege keys"),
        (FILE_BROWSE, "Browse channel files"),
        (FILE_UPLOAD, "Upload files"),
        (FILE_DOWNLOAD, "Download files"),
        (FILE_DELETE, "Delete files"),
        (SCREEN_SHARE, "Share screen / applications"),
        (CHANNEL_COMMANDER, "Use channel commander"),
        (CLIENT_DESCRIPTION_EDIT, "Edit client descriptions"),
        (IGNORE_SERVER_MAXCLIENTS, "Join when server is full"),
    ];
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChannelKind {
    Permanent,
    SemiPermanent,
    Temporary,
}

impl ChannelKind {
    pub fn label(self) -> &'static str {
        match self {
            ChannelKind::Permanent => "Permanent",
            ChannelKind::SemiPermanent => "Semi-Permanent",
            ChannelKind::Temporary => "Temporary",
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChannelInfo {
    pub id: ChannelId,
    pub parent: Option<ChannelId>,
    pub sort: i64,
    pub name: String,
    pub topic: String,
    pub description: String,
    pub has_password: bool,
    pub max_clients: Option<u32>,
    pub kind: ChannelKind,
    pub is_default: bool,
    pub codec_quality: u8,
    pub needed_talk_power: i32,
    /// Minimum group join power required to enter (0 = anyone with Join permission).
    pub needed_join_power: i32,
    pub voice_encrypted: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum PasswordChange {
    Keep,
    Clear,
    Set(String),
}

/// Fields used to create or edit a channel.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChannelEdit {
    pub parent: Option<ChannelId>,
    pub sort: i64,
    pub name: String,
    pub topic: String,
    pub description: String,
    pub max_clients: Option<u32>,
    pub kind: ChannelKind,
    pub is_default: bool,
    pub codec_quality: u8,
    pub needed_talk_power: i32,
    pub needed_join_power: i32,
    pub voice_encrypted: bool,
    pub password: PasswordChange,
}

impl ChannelEdit {
    pub fn from_info(c: &ChannelInfo) -> ChannelEdit {
        ChannelEdit {
            parent: c.parent,
            sort: c.sort,
            name: c.name.clone(),
            topic: c.topic.clone(),
            description: c.description.clone(),
            max_clients: c.max_clients,
            kind: c.kind,
            is_default: c.is_default,
            codec_quality: c.codec_quality,
            needed_talk_power: c.needed_talk_power,
            needed_join_power: c.needed_join_power,
            voice_encrypted: c.voice_encrypted,
            password: PasswordChange::Keep,
        }
    }

    pub fn new_default(parent: Option<ChannelId>) -> ChannelEdit {
        ChannelEdit {
            parent,
            sort: 0,
            name: String::new(),
            topic: String::new(),
            description: String::new(),
            max_clients: None,
            kind: ChannelKind::Permanent,
            is_default: false,
            codec_quality: 6,
            needed_talk_power: 0,
            needed_join_power: 0,
            voice_encrypted: true,
            password: PasswordChange::Keep,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamKind {
    Screen,
    Window,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct StreamInfo {
    pub kind: StreamKind,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub fps: u8,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientInfo {
    pub id: ClientId,
    pub uid: String,
    pub nickname: String,
    pub description: String,
    pub channel: ChannelId,
    pub server_groups: Vec<GroupId>,
    pub channel_group: GroupId,
    pub away: Option<String>,
    pub input_muted: bool,
    pub output_muted: bool,
    /// Extra talk power granted by a moderator ("talker" flag).
    pub is_talker: bool,
    pub talk_request: Option<String>,
    pub commander: bool,
    pub streaming: Option<StreamInfo>,
    pub platform: String,
    pub version: String,
    pub connected_at: u64,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GroupKind {
    Server,
    Channel,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GroupInfo {
    pub id: GroupId,
    pub kind: GroupKind,
    pub name: String,
    pub perms: u64,
    pub talk_power: i32,
    /// Compared against a channel's needed join power.
    pub join_power: i32,
    pub sort: i32,
    /// Built-in groups cannot be deleted.
    pub builtin: bool,
    /// Show the group name next to member nicknames.
    pub show_name: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ServerInfo {
    pub name: String,
    pub welcome_message: String,
    pub host_message: String,
    pub max_clients: u32,
    pub has_password: bool,
    pub default_server_group: GroupId,
    pub default_channel_group: GroupId,
    pub channel_admin_group: GroupId,
    pub chat_retention_days: u32,
    pub version: String,
    pub platform: String,
    pub created: u64,
    pub started: u64,
    pub fingerprint: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ServerEdit {
    pub name: String,
    pub welcome_message: String,
    pub host_message: String,
    pub max_clients: u32,
    pub password: PasswordChange,
    pub default_server_group: GroupId,
    pub default_channel_group: GroupId,
    pub channel_admin_group: GroupId,
    pub chat_retention_days: u32,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextTarget {
    Server,
    Channel(ChannelId),
    /// Private conversation; the id is the *other* participant.
    Private(ClientId),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChatMessage {
    pub id: MessageId,
    pub target: TextTarget,
    pub from: Option<ClientId>,
    pub from_uid: String,
    pub from_name: String,
    pub ts: u64,
    pub text: String,
}

/// Largest image that can be posted to a chat.
pub const MAX_CHAT_IMAGE: usize = 10 * 1024 * 1024;

const IMAGE_TAG: &str = "[voxa-image:";

/// An image posted to a chat. It travels inside the message text as a tag, so clients
/// that don't know images still show "📷 name" instead of nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageRef {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub name: String,
}

impl ImageRef {
    pub fn to_text(&self) -> String {
        format!("📷 {}\n{IMAGE_TAG}{}:{}x{}]", self.name, self.id, self.width, self.height)
    }

    pub fn parse(text: &str) -> Option<ImageRef> {
        let (head, tail) = text.rsplit_once('\n')?;
        let inner = tail.strip_prefix(IMAGE_TAG)?.strip_suffix(']')?;
        let (id, dims) = inner.split_once(':')?;
        let (w, h) = dims.split_once('x')?;
        let name = head.strip_prefix("📷 ")?.to_string();
        Some(ImageRef { id: id.parse().ok()?, width: w.parse().ok()?, height: h.parse().ok()?, name })
    }
}

/// Recognised image formats (by their first bytes): returns the file extension.
pub fn image_kind(data: &[u8]) -> Option<&'static str> {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if data.starts_with(b"GIF87a") || data.starts_with(b"GIF89a") {
        Some("gif")
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        Some("webp")
    } else if data.starts_with(b"BM") {
        Some("bmp")
    } else {
        None
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct BanEntry {
    pub id: u64,
    pub uid: Option<String>,
    pub ip: Option<String>,
    pub name: Option<String>,
    pub reason: String,
    pub created: u64,
    pub expires: Option<u64>,
    pub by: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum BanTarget {
    Client(ClientId),
    Uid(String),
    Ip(String),
}

/// File browser limits. `0` means unlimited.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct FileLimits {
    /// Largest single file that can be uploaded, in megabytes.
    pub max_upload_mb: u64,
    /// Total storage per channel, in megabytes.
    pub channel_quota_mb: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct GroupMember {
    pub uid: String,
    pub name: String,
    pub online: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum LeaveReason {
    Disconnected(String),
    Kicked { by: String, reason: String },
    Banned { by: String, reason: String },
    TimedOut,
}

/// One rectangular piece of a screen share frame, JPEG encoded.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Tile {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    pub jpeg: Vec<u8>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct StreamFrame {
    pub width: u32,
    pub height: u32,
    pub keyframe: bool,
    pub tiles: Vec<Tile>,
}

#[cfg(test)]
mod image_tests {
    use super::*;

    #[test]
    fn image_ref_round_trip() {
        let r = ImageRef { id: 42, width: 1920, height: 1080, name: "cover art.jpg".into() };
        assert_eq!(ImageRef::parse(&r.to_text()), Some(r));
        assert_eq!(ImageRef::parse("hello"), None);
        assert_eq!(ImageRef::parse("📷 x\n[voxa-image:abc:1x1]"), None);
    }
}
