use serde::{Deserialize, Serialize};

use crate::model::*;

/// Client to server messages on the control connection.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum C2S {
    Hello {
        protocol: u32,
        version: String,
        platform: String,
        nickname: String,
        server_password: Option<String>,
        /// Channel path such as "Lobby/Games" to join on connect.
        default_channel: Option<String>,
        default_channel_password: Option<String>,
    },
    /// First message on a secondary connection: turns it into the bulk connection of
    /// the session that received this token in its Welcome.
    AttachBulk([u8; 32]),
    Ping(u64),
    Disconnect(String),

    // --- self ---
    UpdateSelf(SelfUpdate),

    // --- channels ---
    SwitchChannel { channel: ChannelId, password: Option<String> },
    CreateChannel(ChannelEdit),
    EditChannel { id: ChannelId, edit: ChannelEdit },
    DeleteChannel { id: ChannelId, force: bool },

    // --- chat ---
    SendText { target: TextTarget, text: String },
    FetchHistory { target: TextTarget, before: Option<MessageId>, limit: u32 },
    DeleteMessage { target: TextTarget, id: MessageId },

    // --- moderation ---
    Poke { target: ClientId, message: String },
    Kick { target: ClientId, from_server: bool, reason: String },
    MoveClient { target: ClientId, channel: ChannelId },
    Ban { target: BanTarget, duration_secs: Option<u64>, reason: String },
    BanList,
    Unban(u64),
    SetTalker { target: ClientId, talker: bool },
    EditClientDescription { target: ClientId, description: String },

    // --- server & groups ---
    EditServer(ServerEdit),
    CreateGroup { kind: GroupKind, name: String, copy_from: Option<GroupId> },
    EditGroup(GroupInfo),
    DeleteGroup(GroupId),
    GroupMembers(GroupId),
    SetServerGroup { uid: String, group: GroupId, add: bool },
    SetChannelGroup { uid: String, channel: ChannelId, group: GroupId },
    CreatePrivilegeKey { group: GroupId, channel: Option<ChannelId>, description: String },
    UsePrivilegeKey(String),

    // --- files ---
    FileList { channel: ChannelId, path: String },
    FileDelete { channel: ChannelId, path: String },
    FileMkdir { channel: ChannelId, path: String },

    // --- screen sharing ---
    StartStream(StreamInfo),
    StopStream,
    WatchStream { streamer: ClientId, watch: bool },
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct SelfUpdate {
    pub nickname: Option<String>,
    pub description: Option<String>,
    pub away: Option<Option<String>>,
    pub input_muted: Option<bool>,
    pub output_muted: Option<bool>,
    pub talk_request: Option<Option<String>>,
    pub commander: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct VoiceSetup {
    pub key: [u8; 32],
    pub udp_port: u16,
}

/// Server to client messages on the control connection.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum S2C {
    Welcome {
        your_id: ClientId,
        your_uid: String,
        server: ServerInfo,
        channels: Vec<ChannelInfo>,
        clients: Vec<ClientInfo>,
        groups: Vec<GroupInfo>,
        voice: VoiceSetup,
        bulk_token: [u8; 32],
    },
    /// Connection refused before welcome (bad password, banned, full...).
    Refused(String),
    Error(String),
    Notice(String),
    Pong(u64),

    ServerUpdated(ServerInfo),
    ChannelCreated(ChannelInfo),
    ChannelEdited(ChannelInfo),
    ChannelDeleted(ChannelId),

    ClientJoined(ClientInfo),
    ClientLeft { id: ClientId, reason: LeaveReason },
    ClientMoved { id: ClientId, channel: ChannelId, by: Option<String> },
    ClientUpdated(ClientInfo),

    Text(ChatMessage),
    History { target: TextTarget, messages: Vec<ChatMessage>, complete: bool },
    MessageDeleted { target: TextTarget, id: MessageId },

    Poked { from: ClientId, from_name: String, message: String },
    /// Sent right before the server closes the connection.
    Kicked(LeaveReason),

    GroupsUpdated(Vec<GroupInfo>),
    GroupMembers { group: GroupId, members: Vec<GroupMember> },
    BanList(Vec<BanEntry>),
    PrivilegeKeyCreated(String),
    FileList { channel: ChannelId, path: String, entries: Vec<FileEntry> },
}

/// Messages on the secondary "bulk" connection used for screen sharing and file
/// transfers, so large payloads never delay control traffic.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum BulkC2S {
    Frame(StreamFrame),
    RequestKeyframe(ClientId),
    UploadBegin { id: u32, channel: ChannelId, path: String, size: u64, overwrite: bool },
    UploadChunk { id: u32, data: Vec<u8> },
    UploadEnd { id: u32 },
    Download { id: u32, channel: ChannelId, path: String },
    Cancel { id: u32 },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub enum BulkS2C {
    Attached,
    Frame { from: ClientId, frame: StreamFrame },
    KeyframeRequested,
    UploadReady { id: u32 },
    UploadDone { id: u32 },
    DownloadBegin { id: u32, size: u64 },
    DownloadChunk { id: u32, data: Vec<u8> },
    DownloadEnd { id: u32 },
    TransferError { id: u32, message: String },
}

pub const FILE_CHUNK: usize = 256 * 1024;
