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

    // --- added in 1.3.0 (appended to stay wire compatible with 1.2.x) ---
    /// Ask for the file transfer limits (answered with `S2C::FileLimits`).
    GetFileLimits,
    /// Change the file transfer limits (requires "Edit virtual server").
    SetFileLimits(FileLimits),

    // --- added in 1.4.0 ---
    /// Delete every message of a channel / server chat (requires "Clear entire chat history").
    ClearChat(TextTarget),
    /// Per-user permissions granted on top of the user's groups (requires "Manage groups").
    GetClientPerms(String),
    SetClientPerms { uid: String, perms: u64 },

    // --- added in 1.7.0; only sent to servers that are 1.7.0 or newer ---
    /// Copies a channel under a new name: all settings, the password and everyone's
    /// channel group (e.g. channel admins). With `with_children` the sub-channels too.
    CloneChannel { source: ChannelId, name: String, with_children: bool },

    // --- added in 1.8.0; only sent to servers that are 1.8.0 or newer ---
    /// Pins or unpins a stored message (requires "Pin messages").
    PinMessage { target: TextTarget, id: MessageId, pin: bool },
    /// Asks for the pinned messages of a chat (answered with `S2C::Pins`).
    GetPins(TextTarget),

    // --- added in 1.13.0; only sent to servers that are 1.13.0 or newer ---
    /// Everyone who has been on the server (answered with `S2C::People`).
    GetPeople,
    /// A direct message to someone who is offline: the server keeps it (encrypted) and
    /// delivers it when they come online.
    SendOffline { to_uid: String, text: String },

    // --- added in 1.15.0; only sent to servers that are 1.15.0 or newer ---
    /// Changes the text of one of your own stored messages ("Edit own chat messages").
    EditMessage { target: TextTarget, id: MessageId, text: String },
    /// Your picture and colour. `image` is a square PNG or JPEG of at most
    /// `MAX_AVATAR_BYTES` (None: no picture, the initials are shown).
    SetAvatar { color: Option<u32>, image: Option<Vec<u8>> },
    /// Asks for someone's picture (answered with `AvatarInfo` and `AvatarImage`).
    GetAvatar(String),
    /// Removes someone's picture, e.g. an offensive one (requires "Edit client descriptions").
    ClearAvatar(String),

    // --- added in 1.16.0; only sent to servers that are 1.16.0 or newer ---
    /// Server-wide options such as auto-away (requires "Edit virtual server").
    SetServerOptions(ServerOptions),
    /// This PC's hardware tokens, sent right after Welcome (see `hardware_token`). Used
    /// for hardware bans; the raw MAC addresses / machine id never leave the PC.
    Hardware(Vec<String>),
    /// Bans someone who is online by any combination of identity, address and hardware.
    BanWith { target: ClientId, uid: bool, ip: bool, hardware: bool, duration_secs: Option<u64>, reason: String },

    // --- added in 1.17.0; only sent to servers that are 1.17.0 or newer ---
    /// Online, busy or invisible (away goes through `UpdateSelf`, as before).
    SetPresence(Presence),

    // --- added in 1.19.0; only sent to servers that are 1.19.0 or newer ---
    /// Starts a group chat with you and these people (uids). Messages go with
    /// `SendText { target: TextTarget::Group(id) }`.
    CreateGroupChat { name: String, members: Vec<String> },
    /// Adds people to a group chat you are in.
    InviteToGroupChat { id: GroupChatId, uids: Vec<String> },
    RenameGroupChat { id: GroupChatId, name: String },
    LeaveGroupChat(GroupChatId),

    // --- added in 1.21.0; only sent to servers that are 1.21.0 or newer ---
    /// Turns your camera on (video in your channel, like Discord); the pictures go with
    /// `BulkC2S::CameraFrame`. Turned off by `StopCamera`, leaving the channel or
    /// disconnecting.
    StartCamera(CameraInfo),
    StopCamera,
    /// A channel's calendar (answered with `S2C::Calendar`, and again after every change
    /// while you may see it).
    GetCalendar(ChannelId),
    CreateCalendar { channel: ChannelId, name: String },
    RenameCalendar { channel: ChannelId, id: CalendarId, name: String },
    /// Deletes a calendar with its events.
    DeleteCalendar { channel: ChannelId, id: CalendarId },
    /// Creates (id 0) or changes an event; calendar 0 means the channel's first calendar.
    SaveEvent { channel: ChannelId, event: CalendarEvent },
    DeleteEvent { channel: ChannelId, id: EventId },
    /// Going, maybe, can't, or None to take your answer back.
    AnswerEvent { channel: ChannelId, id: EventId, answer: Option<RsvpAnswer> },

    // --- added in 1.23.0; only sent to servers that are 1.23.0 or newer ---
    /// Forgets someone who is offline: their record, server groups, personal
    /// permissions, channel groups, picture and waiting messages (they start fresh if they
    /// come back). Needs "Create, edit and delete groups", and no permission they have
    /// that you lack. Answered with an updated `People` list.
    ForgetClient(String),
    /// Creates (id 0) or changes an event of the server calendar (`SERVER_CALENDAR`),
    /// with the channel to meet in (None: no channel).
    SaveServerEvent { event: CalendarEvent, meet: Option<ChannelId> },

    // --- added in 1.25.1; only sent to servers that are 1.25.1 or newer ---
    /// Which server groups do not see a channel (needs "Edit channels").
    SetChannelHidden { channel: ChannelId, groups: Vec<GroupId> },
    /// Who must sign in (needs "Edit virtual server").
    SetAccountPolicy(AccountPolicy),
    /// Makes an account for your identity on this server (and signs you in).
    CreateAccount { username: String, password: String },
    /// Signs in. `code`: from the authenticator app, or a recovery code. `remember`: the
    /// server answers with `AccountToken` so this device signs in by itself next time.
    SignIn { username: String, password: String, code: Option<String>, remember: bool },
    /// Signs in with a token from `AccountToken` (a remembered device).
    ResumeAccount(String),
    /// Signs out (and forgets this device's token).
    SignOut(Option<String>),
    ChangePassword { old: String, new: String },
    /// Starts adding an authenticator app (answered with `MfaSetup`).
    StartMfa,
    /// Finishes it with a code from the app (answered with `RecoveryCodes`).
    ConfirmMfa(String),
    DisableMfa { password: String, code: String },
    /// New recovery codes (the old ones stop working).
    NewRecoveryCodes { password: String },
    /// A new password with a recovery code.
    ResetPassword { username: String, code: String, password: String },
    /// An administrator resets someone's password (needs "Edit virtual server", and
    /// nobody with permissions you lack). The server answers with `AccountReset` holding a
    /// temporary password, which they must change after signing in. `remove_mfa`: also
    /// turns off their authenticator app (e.g. a lost phone).
    ResetAccount { uid: String, remove_mfa: bool },
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

    // --- added in 1.3.0; only sent to clients that asked with GetFileLimits ---
    FileLimits(FileLimits),

    // --- added in 1.4.0; only sent to clients that are 1.4.0 or newer ---
    ChatCleared { target: TextTarget, by: String },
    ClientPerms { uid: String, perms: u64 },
    /// Your own per-user permissions (in addition to your groups).
    YourPerms(u64),

    // --- added in 1.8.0; only sent to clients that are 1.10.0 or newer ---
    /// Pinned messages of a chat, most recently pinned first.
    Pins { target: TextTarget, pins: Vec<PinnedMessage> },
    /// Someone pinned / unpinned a message (fetch the pins again to update).
    PinChanged { target: TextTarget, id: MessageId, pinned: bool, by: String },

    // --- added in 1.13.0; only sent to clients that are 1.18.0 or newer ---
    /// Everyone who has been on the server (answer to `GetPeople`).
    People(Vec<Person>),
    /// Direct messages that arrived while you were offline (sent once after Welcome).
    OfflineMessages(Vec<OfflineMessage>),
    /// An offline message was accepted for delivery.
    OfflineQueued { to_uid: String, ts: u64 },

    // --- added in 1.15.0; only sent to clients that are 1.20.0 or newer ---
    /// A stored message was edited by its author.
    MessageEdited { target: TextTarget, id: MessageId, text: String, edited_at: u64 },
    /// Which messages of the `History` just sent were edited, and when.
    EditedMessages { target: TextTarget, edits: Vec<(MessageId, u64)> },
    /// Pictures / colours of people: after Welcome (everyone online), when someone joins
    /// or changes theirs, with the People list, and in answer to `GetAvatar`.
    AvatarInfo(Vec<AvatarMeta>),
    /// A picture asked for with `GetAvatar`.
    AvatarImage { uid: String, image: u64, data: Vec<u8> },

    // --- added in 1.16.0; only sent to clients that are 1.21.0 or newer ---
    /// Server-wide options (after Welcome and when they change).
    ServerOptions(ServerOptions),
    /// Which bans of the `BanList` just sent also ban hardware.
    BannedHardware(Vec<u64>),

    // --- added in 1.17.0; only sent to clients that are 1.22.0 or newer ---
    /// Someone's presence (Online or Busy; Invisible only to administrators, who can see
    /// invisible people). Sent after Welcome for everyone not Online, and on changes.
    ClientPresence { id: ClientId, presence: Presence },

    // --- added in 1.19.0; only sent to clients that are 1.27.0 or newer ---
    /// The group chats you are in (after Welcome, before any waiting group messages).
    GroupChats(Vec<GroupChatInfo>),
    /// A group chat you are in was created or changed (members, name).
    GroupChatUpdated(GroupChatInfo),
    /// You are no longer in this group chat (you left it).
    GroupChatLeft(GroupChatId),

    // --- added in 1.21.0; only sent to clients that are 1.29.0 or newer ---
    /// Whose cameras are on (sent after Welcome).
    Cameras(Vec<(ClientId, CameraInfo)>),
    /// Someone turned their camera on (Some) or off (None).
    CameraChanged { client: ClientId, camera: Option<CameraInfo> },
    /// A channel's calendars and events (after `GetCalendar` and every change).
    Calendar { channel: ChannelId, calendars: Vec<CalendarInfo>, events: Vec<CalendarEvent> },

    // --- added in 1.23.0; only sent to clients that are 1.31.0 or newer ---
    /// The server calendar (after `GetCalendar(SERVER_CALENDAR)` and every change): its
    /// calendars, events, and the channel each event meets in.
    ServerCalendar { calendars: Vec<CalendarInfo>, events: Vec<CalendarEvent>, meet: Vec<(EventId, ChannelId)> },

    // --- added in 1.25.1; only sent to clients that are 1.33.1 or newer ---
    /// The server's account rules (after Welcome and when they change).
    AccountPolicy(AccountPolicy),
    /// Your account here (after Welcome and every change).
    AccountState(AccountState),
    /// A token that signs this device in next time (`ResumeAccount`).
    AccountToken(String),
    /// Adding an authenticator app: the secret (base32) and an `otpauth://` link (QR code).
    MfaSetup { secret: String, uri: String },
    /// One-time recovery codes: shown once, keep them safe.
    RecoveryCodes(Vec<String>),
    /// Which groups do not see which channels (to people who may edit channels).
    HiddenChannels(Vec<(ChannelId, Vec<GroupId>)>),
    /// Channels you now see (parents first) or no longer see (sub-channels first), after a
    /// change of groups, hidden lists, your channel or signing in. Not created or deleted.
    ChannelsVisible { shown: Vec<ChannelInfo>, hidden: Vec<ChannelId> },
    /// To the administrator who reset someone's password: the temporary password to give
    /// them (shown once; they must change it after signing in).
    AccountReset { uid: String, username: String, password: String },
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

    // --- added in 1.5.0; only sent to servers that are 1.5.0 or newer ---
    /// Posts an image to a chat; it is stored encrypted like the chat text. Answered
    /// with `ChatImageSent` or `TransferError` carrying the same id.
    ChatImage { id: u32, target: TextTarget, name: String, width: u32, height: u32, data: Vec<u8> },
    /// Fetches a chat image (answered with `Image` or `ImageError`).
    GetImage(u64),

    // --- added in 1.6.0; only sent to servers that are 1.6.0 or newer ---
    /// 20 ms of the shared screen / application's sound: one stereo 48 kHz Opus packet.
    StreamAudio(Vec<u8>),

    // --- added in 1.21.0; only sent to servers that are 1.21.0 or newer ---
    /// A picture of your camera (same encoding as screen sharing).
    CameraFrame(StreamFrame),
    /// Asks the camera of this person for a full picture (e.g. after joining).
    RequestCameraKeyframe(ClientId),
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

    // --- added in 1.5.0; only sent in answer to the 1.5.0 requests above ---
    ChatImageSent { id: u32 },
    Image { image: u64, data: Vec<u8> },
    ImageError { image: u64, message: String },

    // --- added in 1.6.0; only sent to clients that are 1.8.0 or newer ---
    /// Sound of a stream being watched (see `BulkC2S::StreamAudio`).
    StreamAudio { from: ClientId, data: Vec<u8> },

    // --- added in 1.21.0; only sent to clients that are 1.29.0 or newer ---
    /// A camera picture of someone in your channel.
    CameraFrame { from: ClientId, frame: StreamFrame },
    /// Someone needs a full picture of your camera.
    CameraKeyframeRequested,
}

pub const FILE_CHUNK: usize = 256 * 1024;
