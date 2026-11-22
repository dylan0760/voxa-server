# Voxa Server

Voxa is free, open source voice chat with persistent encrypted text chat and screen
sharing. It is written in Rust for Windows. There are no license keys, registrations or
fees. Servers can optionally ask for per-server accounts (username, password and an
authenticator app) on top of each identity.

This repository contains the **server**:
* `voxa-server.exe`: runs as the Windows service **VoxaServer**.
* **Voxa Server Manager**: a small tray app to monitor and control the service.

The desktop client lives at [dylan0760/voxa-client](https://github.com/dylan0760/voxa-client).

Voxa is an independent clean-room implementation. It contains no TeamSpeak code or
assets and does not use the TeamSpeak protocol.

## Download & install
Get `Voxa-Server-<version>-x64.msi` from the
[latest release](https://github.com/dylan0760/voxa-server/releases/latest) and run it.
The MSI is not code-signed. If Windows SmartScreen warns you, choose
**More info → Run anyway**.

The installer:
* installs the **VoxaServer** service (automatic start, restarts on failure);
* opens **TCP 9987** and **UDP 9987** in Windows Firewall, and **TCP 9980** (the
  [web admin page](#web-admin-page)) for the local network only;
* installs **Voxa Server Manager**, which starts in the system tray at sign-in (each user
  can turn that off in the manager's **Startup** section).

**When updating, update the Voxa clients first.** By default the server only accepts the
newest Voxa client (see [Client version](#client-version)).

## Ports
| Port | Protocol | Used for |
|------|----------|----------|
| **9987** | **TCP** | Control connection, encrypted chat and history, file transfers, screen sharing |
| **9987** | **UDP** | Encrypted voice |
| 9980 | TCP | [Web admin page](#web-admin-page) (plain HTTP; put an HTTPS proxy in front of it) |

If people connect from the internet, forward **TCP and UDP 9987** on your router to the
server PC. To use another port, change `port` in `server.toml`. Don't forward 9980
directly: publish the web admin page through an HTTPS reverse proxy instead.

**Upload bandwidth:** the server sends each listener their own copy.
* Voice: up to about 0.1 Mbit/s per listener.
* Opus Music: up to about 0.2 Mbit/s per listener.
* Lossless: about 0.8–1.5 Mbit/s per listener.
* Screen shares: depend on the content, typically a few Mbit/s per viewer.

## Using Voxa Server Manager
Open it from the Start menu (**Voxa Server Manager**) or from its tray icon. It shows:
* service status, clients online, channels, uptime, fingerprint and traffic;
* **Start / Stop / Restart** buttons (no administrator rights needed);
* the **Server Admin privilege key**. In the client, click the server name ▸
  **Use Privilege Key** and paste it to become admin;
* buttons for the data folder, `server.toml` and the log, plus a live log view;
* **🌐 Open Web Admin**, which opens the [web admin page](#web-admin-page);
* **Backups**: settings, **Back up now**, the last and next backup, and **Restore** (see
  [Backups](#backups));
* **Startup**: whether the manager opens in the system tray when you sign in to Windows
  (on by default, per user);
* **Updates**: checks GitHub and installs new versions;
* **What's new**, with the change log of every version.

Everything below the Start / Stop / Restart header scrolls, so a small window still
reaches every section. Closing the window (✕) keeps the manager in the system tray. The
server itself always runs as a service that starts with Windows, even before anyone
signs in and when the manager is closed.

## Backups
A backup is **one password-protected file** (`Voxa Server backup 2026-11-15 030000.voxabackup`)
with everything the server needs: `server.toml`, the settings, channels, groups,
permissions, bans, people, chat history, chat images, avatars, the server's keys (so
clients still recognise it and the history stays readable), the initial admin key,
every uploaded channel file and the GIF library (`gifs`, 1.28.0+).

* **Set it up:** Voxa Server Manager ▸ **Backups ▸ ⚙ Settings…** (asks for administrator
  rights):
  * the **folder**: a folder on this PC or a network share, e.g. `\\nas\backups\voxa`;
  * for a network share, an optional **account** that may write there (the server runs as
    a Windows service, which usually can't sign in to a NAS by itself);
  * the **password** (at least 8 characters). It is needed to restore, so keep it safe:
    without it a backup can't be opened;
  * **automatic** backups **every day** or **every week** on a chosen day, at a chosen hour;
  * how many backups to **keep** (older ones are deleted; 0 keeps all).
* **Back up now:** in the manager (no administrator rights needed), or
  `voxa-server.exe --backup-now` as administrator.
* **Made by the running server**, so nobody is disconnected: the database is copied in one
  read transaction. If the PC was off at backup time, the backup is made as soon as the
  server runs again; a failed backup is retried an hour later. The manager shows the last
  backup, any error and the next one.
* **Restore:** **Backups ▸ ♻ Restore…**, pick the file and enter its password (asks for
  administrator rights). The backup is unpacked and checked first; then the service is
  stopped, the current data is moved to `before-restore` in the data folder, the backup is
  put in place and the service starts again. Everyone is disconnected for a moment. A
  backup can be restored on another PC (move a server). Without the manager:
  `voxa-server.exe --restore <file>` as administrator.
* **Encryption:** the password is stretched with Argon2id (64 MiB); the content is sealed
  with XChaCha20-Poly1305 in 1 MiB chunks that can't be reordered, removed or cut off.
  A wrong password or a damaged file is refused without touching the server.
* **Where the settings live:** `backup.json` (schedule and folder) and `backup-status.json`
  can be read by every user of the PC; the backup and share passwords are in `backup.key`,
  readable by SYSTEM and Administrators only.

## Updates
* **Manager:** it checks this repository's releases at startup and every 12 hours. You
  can also click **Check for updates** in the window or the tray menu.
  * Updates are only installed from github.com, and only if the MSI matches the SHA-256
    checksum GitHub publishes.
  * The installer stops the service, upgrades it, starts it again and reopens the
    manager.
* **Without a desktop:** run `voxa-server.exe --check-update` to check, or
  `voxa-server.exe --update` as administrator to download and install.

## Web admin page
Manage the server from a browser: the channel tree, chat, people and every admin dialog,
with the same layout and the same themes as the Voxa client: 🎨 at the bottom left opens
a picker with a preview of every theme, the solid ones (including Mustard, Slate and
Obsidian) and the two-tone ones where one colour blends into another.

* **Open it:** `http://<server>:9980/`, or **🌐 Open Web Admin** in Voxa Server Manager.
* **Sign in** with a Voxa identity. You get exactly the rights that identity has: an
  admin identity can do everything, a normal one only what normal users can.
  * **🎧 Sign in with Voxa** (Voxa 1.20.0+ on this PC): Voxa pops up, shows the web
    address and the server (recognised by its fingerprint), and asks which identity to
    use. Confirm, and the browser is signed in. The key never leaves Voxa.
  * **Or paste a key:** in Voxa, click your name at the bottom left ▸ **Identities**,
    export the identity, and paste it. The key never leaves the browser.
  * Either way only a proof is sent: an X25519 key agreement with a one-time server key,
    then HMAC-SHA256 over the challenge **and the page's web address**. A proof made for
    a look-alike site is useless on the real one.
  * **Remember the key in this browser** is off by default; a sign-in lasts 12 hours.
* **Join channel:** choose the channel the page joins when signing in (a list on
  servers without a password, otherwise type a path like `Lobby/Games`), with its
  password if it has one, or **No channel** to just be on the server.
* **Status:** click your name at the bottom left for Online, Away, Do not disturb or
  Invisible (remembered in the browser). Do not disturb mutes the page's sounds.
* **Leave Channel** (stay on the server): in the status menu, your channel's menu or its
  info panel.
* **Remembered for you:** the theme, nickname and channel are saved per identity on the
  server, so they follow you to other browsers, PCs and web addresses.
* **What you can do:**
  * create, edit, clone, reorder and delete channels and separators; join channels;
  * drag people onto channels to move them; poke, kick, ban, talk power, server and
    channel groups, personal permissions, descriptions;
  * edit the virtual server and the file limits, groups and permissions, the ban list,
    and privilege keys;
  * set the calendar permissions in the permission editor (the calendars themselves are
    in Voxa);
  * remove someone who is offline from the server: right-click them in 👥 People ▸
    **Remove from Server…** (1.23.0);
  * use the 🌴 Vice City theme (1.23.0), like Voxa's, or **Your picture** (1.24.0): a
    picture of yours behind the see-through panels, kept in that browser only (shrunk
    to 1920 pixels and stored as JPEG in the browser's storage, never sent to the
    server). Its **Choose picture** and **Remove picture** buttons follow the theme
    (1.26.1; before, they stayed light grey and were hard to read on dark themes);
  * find a permission with the 🔍 search above the permission list (1.22.0): every word
    you type must appear in the permission's name, in any order; the ticks of hidden
    permissions are kept;
  * read the Server tab (server events with *See server events*, otherwise the welcome
    message) and read and write channel chat (with history, pins and images), edit your
    own messages, and send direct messages, including to people who are offline;
  * see everyone's avatars, and remove an offensive avatar picture;
  * ban by unique ID, IP address and / or hardware;
  * **server statistics** for administrators: traffic since the server started (voice,
    file transfers, screen sharing, chat), refreshed every few seconds. They use the
    same live connection as the rest of the page, so no WebSocket setting is needed in
    the reverse proxy.
* **Auto away** after the server's idle time without input on the page, and join /
  leave sounds (🔔 / 🔕 at the bottom left).
* **How it appears:** a signed-in page is an ordinary session of that identity, shown
  with a **WEB** badge and no voice. It can run alongside the same identity's desktop
  client without replacing it.

### HTTPS with a reverse proxy (e.g. Nginx Proxy Manager)
The server only speaks plain HTTP. For access over the internet, put an HTTPS reverse
proxy in front of it:
1. In Nginx Proxy Manager, add a **Proxy Host** for your domain (e.g. `voxa-admin.example.com`).
2. Scheme `http`, forward hostname/IP = the Voxa server PC, forward port `9980`.
3. On the **SSL** tab, request a Let's Encrypt certificate and enable **Force SSL**.
4. Leave **Block common exploits** on. No custom configuration is needed: the live
   updates use Server-Sent Events, which the server marks as unbuffered.

Behind a proxy on the same PC or local network, the visitor's address comes from
`X-Real-IP` / `X-Forwarded-For` (bans, logs and sign-in rate limits use it), and the
sign-in cookie is marked `Secure` when the proxy reports HTTPS. Other proxies (Caddy,
Traefik, IIS…) work the same way.

The proxy must pass the original `Host` header (Nginx Proxy Manager does). If sign-in
says *"This sign-in was made for a different web address"*, the proxy rewrites it.

To change the port or address, set `web_port` / `web_bind` in `server.toml`. To turn
the page off, set `web_enabled = false`.

**Which web addresses may sign in** (1.26.0): a look-alike site could otherwise relay a
sign-in that you approve in Voxa. So the page only accepts sign-ins made for its own
address:
* If `web_allowed_origins` lists addresses, exactly those, e.g.
  `web_allowed_origins = ["https://voxa-admin.example.com", "http://192.168.1.35:9980"]`.
* If it is empty (the default), the address the **first server administrator** signs in
  from is remembered in `web_origin.txt` in the data folder, and only that address works
  from then on. Until an administrator has signed in, any address works.
* Using the page from a second address (e.g. on the local network and through your
  domain)? List both in `web_allowed_origins`. Moved the page to a new address? Delete
  `web_origin.txt` and sign in again as an administrator. A refused address shows
  *"This web address is not allowed for this server's web page"*.

Restart the server after editing `server.toml`.

## Client version
With `require_latest_client = true` (the default in `server.toml`), the server only
accepts clients that are at least the newest Voxa it knows:
* the Voxa version released together with the server;
* or a newer Voxa published on GitHub since. The server checks every 30 minutes, and
  only counts a release once its installer can be downloaded.

Older clients are refused with a message to update. Voxa 1.15.0 and newer then open
their update window by themselves. To allow older clients, set
`require_latest_client = false` and restart the server.

## Features
* **Channels:**
  * Permanent, semi-permanent and temporary channels, with sub-channels, passwords, max
    clients, topic and description, and moderation (talk power).
  * Drag & drop ordering, and **Clone Channel**.
* **Separators:** channels named with a TeamSpeak-style spacer tag are separators.
  * The tags are `[cspacer]Text`, `[lspacer]`, `[rspacer]` and `[*spacer]─`, optionally
    numbered (`[*spacer2]─`).
  * Nobody can get into one: joining, being moved in, a connect link naming it, and the
    creator being moved in are all refused.
  * They can't be temporary, the default channel or password protected.
* **Codecs per channel:**
  * **Opus Voice** (mono speech);
  * **Opus Music** (stereo, up to 192 kbit/s);
  * **Lossless** (16-bit 48 kHz stereo, bit exact). The server only forwards lossless
    audio to clients that can play it.
* **Unique names** (1.26.0): each nickname belongs to one identity on the server, also
  while its owner is offline, so nobody can take or imitate someone else's name.
  * Names are compared ignoring case, accents, spaces and separators (`_ - .`), and
    look-alike characters count as the same letter: Cyrillic and Greek letters that look
    Latin, `I`/`l`/`1`/`|`, `0`/`O` and full-width letters. "DyIan", "Dylаn" (Cyrillic
    а) and "dy_lan" all clash with "Dylan".
  * Joining with a name that belongs to someone else gives you a numbered name
    ("Dylan2") and a message explaining why; renaming to it is refused.
  * A name is released when its owner changes name, or when an administrator removes
    them from the server (People → Remove).
  * Your own sessions (Voxa and the web page) can share your name.
  * On a server upgraded from an older version where two identities already had the
    same name, the one seen most recently keeps it.
* **Groups and moderation:** server and channel groups with a permission editor,
  privilege keys, bans, kick, move, poke and channel commander.
  * Moderators can only kick, move or ban people who have no permissions the moderator
    lacks.
  * Nobody can hand out permissions they don't have themselves.
* **Personal permissions:** grant individual permissions to a single user on top of
  their groups (client: right-click user ▸ **Permissions…**).
* **Permission search:** Voxa 1.30.0 and the web admin page (1.22.0) have a search box
  above every permission list.
* **Channel access control:**
  * The **Join / switch channels** permission controls whether a group can change
    channels at all.
  * Each group has a **join power**: Admin 75, Normal 10, Guest 0 by default.
  * Each channel has a **needed join power**: 0 lets anyone in, 10 makes it
    members-only, 75 admins-only.
  * **Join channels regardless of join power** lets moderators bypass the check.
  * A channel group assigned in a specific channel also counts, so you can let
    individual people into one restricted channel.
* **The Server tab** (1.19.0) shows only what happens on the server (who connects and
  leaves); nobody can write there or edit old messages. Older history stays readable.
  * **See server events** (1.20.0) decides who sees it. Only the **Server Admin** group
    has it by default; give it to other groups or single people in the permission
    editor. Everyone else sees the server's **welcome message** in that tab.
  * The server enforces it: without the permission, the tab's history, pins and pictures
    are refused (older Voxa versions included).
  * *Send server chat messages* is no longer used.
* **Persistent encrypted chat:** channel chat is stored on the server
  (XChaCha20-Poly1305), with configurable retention.
  * Delete single messages: your own always, anyone's with *Delete anyone's chat
    messages*.
  * *Clear entire chat history* wipes a whole channel or server chat.
  * **Pinned messages**.
  * **Edit your own messages** with the *Edit own chat messages* permission (on for the
    Normal and Server Admin groups; existing servers get it added to Normal when they
    update). Nobody can edit someone else's message, and images can't be edited. Edited
    messages are marked "(edited)" for everyone.
* **Auto away:** people are set *Away (idle)* after a number of minutes without input
  (10 by default). Admins change it under **Edit Virtual Server ▸ Auto away after**:
  0 turns it off, at most 30. Each person can also turn it off for themselves.
* **Hardware bans:** ban someone by **unique ID**, **IP address** and / or **hardware**
  (any combination). A hardware ban turns away any identity from that PC.
  * Voxa 1.21.0 reports scrambled hardware tokens right after connecting: hashes of its
    network adapters' MAC addresses and the Windows machine ID, mixed with this server's
    fingerprint. The server never sees the addresses themselves, and the tokens mean
    nothing on other servers. One matching token is enough, so a new network card does
    not get around a ban.
  * A client that doesn't report within 20 seconds is disconnected.
  * Limits: the web admin page has no hardware to check, and a determined person can fake
    hardware identifiers. Combine hardware with the unique ID / IP address for best effect.
* **Avatars:** people can pick a colour for their initials or upload a picture (Voxa
  1.20.0). Pictures are checked: PNG or JPEG, at most 96 KB and 16–256 pixels per side.
  Moderators with *Edit client descriptions* can remove a picture, and the same picture
  can't be set again.
* **On the server without a channel** (Voxa 1.22.0): like Discord, people can be
  connected without being in a channel. They show as online (in the People list, not the
  channel tree), can use direct and group messages, and can't talk, use channel
  chat or files, or share or watch screens until they join a channel.
* **Status** (Voxa 1.22.0): Online, Away, Do not disturb or Invisible.
  * Invisible people look offline while they are not in a channel. They are left out of
    everyone else's client list, avatars, chat-history sender ids and group member lists.
    Pokes, kicks and messages aimed at their session get "Client not found", and their
    "last seen" stays at the time they were last visible.
  * A direct message to an invisible person is delivered at once, but the sender is told
    it was "kept for later".
  * Administrators (*Edit virtual server*) see invisible people, to moderate.
  * Status changes share the chat rate limit, so toggling Invisible can't flood everyone
    with joins and leaves.
* **People list:** everyone who has been on the server, with when they were last seen.
  Everyone can see it.
* **Group chats** (1.19.0, Voxa 1.27.0): like Steam's, chats with several people.
  * Any member can add people, rename the chat or leave it; at most 50 people per chat
    and 100 chats per person. When the last person leaves, the chat is deleted.
  * Only members can send to it, add people or see it; people must be known to the
    server. Messages share the chat rate limit.
  * Like direct messages, the messages are **not stored** on the server: they go
    straight to the members who are online, and are kept (encrypted) only for members
    who are offline, until they connect (at most 100 waiting per person, 30 days).
    Voxa older than 1.27.0 gets them once it is updated.
  * Pictures in a group chat can only be opened by its members.
  * The members of each chat are stored encrypted in the database (and in backups).
* **Offline messages:** people can message someone who is offline.
  * The server keeps the message encrypted, delivers it when the person comes online,
    and then deletes it.
  * Undelivered messages are removed after 30 days; at most 100 wait per person.
  * Direct messages themselves are never stored on the server: Voxa keeps them
    encrypted on each user's PC.
* **Images in chat:**
  * They are stored encrypted next to the chat history, with the same permissions as
    sending text there.
  * Only people who can see that chat can open them: people with *See server events* for older Server tab pictures, channel
    members for a channel, and the two participants for a private chat.
  * PNG, JPEG, GIF, WebP and BMP, up to 10 MB each. Animated GIFs play in Voxa 1.35.0+
    and on the web page.
* **GIF search** (1.27.0, Voxa 1.35.0): the GIF button in Voxa searches GIFs through the
  server, like WhatsApp's picker.
  * Set it up in `server.toml`: `gif_provider = "klipy"` (or `"giphy"`) and
    `gif_api_key = "<your key>"`, then restart. Without a key there is no search; people
    still have their ⭐ favourites, recent GIFs and pasted GIFs.
  * **Which service:** Tenor closed its API on 30 June 2026.
    * [KLIPY](https://klipy.com/developers), from Tenor's founders, has a free key.
      Test keys allow 100 searches an hour; ask for production access in their Partner
      Panel. KLIPY's terms ask integrations that search through a server (as Voxa
      does) to get their approval first: email developers@klipy.com.
    * [GIPHY](https://developers.giphy.com) free beta keys allow 100 calls an hour;
      production use is paid.
  * `gif_content_filter`: KLIPY `off` / `low` / `medium` (default) / `high`; GIPHY `g`,
    `pg`, `pg-13` or `r`.
  * The server searches on people's behalf: their searches and addresses never reach the
    GIF service. Answers are cached for 10 minutes, and the server makes at most 60 calls
    a minute to the service. Picking a GIF makes the server download it and post it like
    a picture (stored encrypted, plays for everyone, and doesn't depend on the service
    keeping it).
* **The server's own GIF library** (1.28.0, Voxa 1.36.0): GIFs you collect, shown to
  everyone under **📚 Server** in Voxa's GIF picker. It needs no GIF service or key.
  * **Add a whole heap at once:** copy `.gif` files into
    `%ProgramData%\Voxa Server\gifs`. Sub-folders become categories (up to 3 levels),
    and file names are what people search for (`happy-cat_dance.gif` is found as "happy
    cat dance"). New files show up by themselves within 30 seconds; no restart needed.
  * **From Voxa:** people with the new **Manage server GIFs** permission (every group
    with *Edit virtual server* gets it on update) can add GIF files (**⬆ Add GIFs…** in
    the 📚 Server tab, with an optional category), add any GIF from the chat (**📚 Add to
    server GIFs** in its full-size view) and remove GIFs (right-click). Added GIFs go into
    a category folder in `gifs` (default "Added in Voxa"); a name already taken gets a
    number.
  * **From GIPHY:** Voxa 1.36.1's picker has a 🌐 GIPHY button. Copy a GIF's link there
    (a GIPHY page or its `media.giphy.com/…/giphy.gif` address), paste it into the
    📚 Server tab's link box, give it a name if you like, and press Add. The admin's Voxa
    downloads it and adds it to the library.
  * Limits: GIF files up to 10 MB, 50 000 GIFs, and adding from Voxa stops at 5 GB.
    Small previews for the picker are made the first time they are needed and kept in
    `gif-previews`.
* **Server-relayed screen sharing** with sound, up to 60 fps. Only viewers in the same
  channel receive it.
* **Video calls** (1.21.0, Voxa 1.29.0 on Windows and Android): people turn their camera
  on in a channel and everyone in that channel sees it, like Discord or Teams.
  * The server relays camera pictures to the others in the channel only, at most 720p
    (landscape or portrait) and 30 fps; bigger pictures, or pictures from someone whose
    camera is off, are dropped.
  * A camera needs *Share screen / applications and camera* in the channel, and goes off
    when its owner leaves the channel. The web page can't turn one on.
  * A slow receiver skips pictures instead of slowing everyone down.
  * **Calls from group chats and direct messages** use what is already there: Voxa makes
    a temporary channel with a random password and posts it to the chat. It disappears
    when everyone has left. Starting one needs *Create temporary channels*.
* **Channel calendar** (1.21.0, Voxa 1.29.0): each channel can have calendars (tabs) where
  people arrange things, e.g. a game on Monday at 8 pm for 8 players, and answer going,
  maybe or can't.
  * Four permissions, checked in the channel (so channel groups count):
    * *See the channel calendar*: Normal and Guest by default;
    * *Answer calendar invitations*: Normal by default;
    * *Create and edit calendar events*: Channel Admin by default;
    * *Create, rename and delete calendars*: Channel Admin by default.

    Server Admins have all of them. Existing servers get these defaults when they
    update.
  * A calendar is only shown to the people in its channel (and those who may edit the
    channel), so a password or members-only channel keeps its calendar to itself.
  * Events with a limit refuse more people going once full. At most 20 calendars and 300
    events per channel; events that ended over 30 days ago are removed. Changes share the
    chat rate limit.
  * Calendars are stored encrypted in the database, are part of backups, and go when
    their channel is deleted.
* **Server calendar** (1.23.0, Voxa 1.31.0): one calendar for the whole server, in a tab
  people see whatever channel they are in (or none).
  * Same calendars, events, answers and limits as a channel's, and events can name a
    **channel to meet in** (e.g. a game night in *Racing*); Voxa offers a Join button.
    The channel must exist; when it is deleted, its events simply have no place any more.
  * The same four calendar permissions, taken from the server groups (and personal
    permissions) instead of a channel: by default everyone sees it, Normal members
    answer, and Server Admins arrange events and manage the calendars. Give *Create and
    edit calendar events* to a server group to let its members arrange game nights.
  * Stored encrypted like the channel calendars (meeting places too), and in backups.
* **Remove people** (1.23.0, Voxa 1.31.0 and the web page): administrators can make the
  server forget someone who is offline. Their record (nickname, server groups, first /
  last seen), personal permissions, channel groups, picture, web page preferences,
  waiting messages and their place in group chats are deleted; their chat messages stay.
  They start fresh if they come back (ban them to keep them out); a picture a moderator
  removed stays blocked.
  * Needs *Create, edit and delete groups*, and only works on people without permissions
    you lack (so a moderator can't remove an admin). Not yourself, and not while they
    are online.
* **File storage per channel:** a **max file size** and **storage per channel**, both
  unlimited if you like.
  * Admins set them in the client: click the server name ▸ **Edit Virtual Server** ▸
    **File transfers**.
  * `max_upload_mb` / `channel_quota_mb` in `server.toml` are only the initial defaults
    (0 = unlimited).
* **Accounts** (1.25.1, Voxa 1.33.1 and the web page): a username and password on top of
  each identity, so a copied identity key alone isn't enough. Accounts are per server.
  * **Edit Virtual Server → Accounts & security:** **Off** (the default), **Optional**,
    **Required for staff** (everyone whose groups give more than the default server group)
    or **Required for everyone**, plus **Staff must also use an authenticator app**.
  * Until someone the rules apply to signs in, they get only **Guest** permissions, talk
    power and join power, and see only what Guests see. The server refuses a change that
    would lock out the admin making it.
  * **MFA:** authenticator apps (TOTP, RFC 6238: Microsoft / Google Authenticator,
    Aegis, 2FAS, Yubico Authenticator with a YubiKey), with 10 one-time recovery codes.
  * **Remembered devices:** a token that signs a device in for 30 days (at most 10 per
    account). Changing or resetting the password, or turning MFA on, signs them all out.
  * **Forgotten passwords:** a recovery code, or an administrator resets it.
  * **Administrator resets** (*Edit virtual server*): right-click someone (or someone
    offline in People) ▸ **Reset Account Password…**. The server makes a temporary
    password, shows it to that administrator once, signs the account out everywhere and,
    if ticked, turns off its authenticator app. After signing in with it, they must choose
    a new password; until then they only get Guest permissions where accounts are
    required. Not for people with permissions you lack, and not yourself. There is no
    email: the server never sends any.
  * Removing someone from the server deletes their account too.
* **Hidden channels** (1.25.1, Voxa 1.33.1 and the web page): **Edit Channel → Hidden
  from** picks server groups that don't see a channel. For them the channel, its
  sub-channels, the people in it, its chat, pictures, files and calendar don't exist.
  Everything about it is left out of what they are sent (also as a meeting place in the
  server calendar, 1.26.1, and as a member's channel in a channel group's member list,
  1.26.2), and asking for it by number gets "Channel not found" (or
  "Image not found" for its pictures).
  * A new permission, **See channels**: a group without it only sees the channel it is
    in. Every server group gets it when the server updates, so nothing changes until you
    change it.
  * Administrators (*Edit virtual server*) always see everything. The default channel
    can't be hidden.
  * You can't hide a channel from a group you are in, or from a group with permissions
    you lack. The web page's public channel list (for "join channel") only shows channels
    Guests can see.
  * When groups, hidden lists, channels or sign-ins change, everyone is told what
    appeared or disappeared for them right away.

## Security
* **Connections:** Noise `XX_25519_ChaChaPoly_BLAKE2s` with mutual authentication,
  for both of a client's connections (the second one carries files, pictures, screen
  sharing and video).
* **Voice** is always encrypted: every UDP packet with ChaCha20-Poly1305 under a fresh
  random 256-bit key per connection (sent over the encrypted connection), with replay
  protection, whatever a channel's *Encrypt voice* setting says. The server decrypts and
  re-encrypts for each listener (it relays, like Discord and Teams); nothing crosses the
  network in the clear.
* **Data folder:** the service locks down the permissions of its data folder when it
  starts.
  * Normal users can only read it.
  * The database, keys and uploaded files are readable by administrators only.
  * Links and junctions inside the files folder are refused.
* **Keys:** the server key and the chat key are protected with Windows DPAPI. The
  initial admin privilege key is only written to `privilege_key.txt` (never to the log),
  and that file is removed once an administrator exists.
* **Passwords:** hashed with Argon2id, and checked without blocking the server. After 10
  wrong passwords from one address in a minute, further tries are refused for a minute.
* **Accounts** (1.25.1):
  * Account records are stored encrypted, each bound to its identity. Recovery codes,
    device tokens are kept only as SHA-256 hashes and compared in
    constant time.
  * Ten wrong passwords or codes in a row lock an account for 15 minutes. Only the
    identity that owns an account can try to sign in to it. Without an account, the
    password is checked against a dummy hash, so the answer takes just as long.
  * Recovery-code resets can be tried by anyone for any username, so wrong codes are
    rate limited but never lock the account (strangers can't lock its owner out).
  * An administrator's reset makes a random 75-bit temporary password, shown only to
    them (never logged). The account can't get its permissions back until the owner
    chooses a new password, so the administrator doesn't keep a working password.
  * Authenticator codes work once. A device is only remembered after a full sign-in
    (with MFA if the account has it).
  * Changing an account (password, MFA, new recovery codes) needs a full sign-in
    on that connection, with the authenticator app if the account has one. A stolen
    password alone can't make new recovery codes to get around MFA.
  * Messages carrying passwords are never logged.
* **GIF search** (1.27.0):
  * Searches and downloads run in their own tasks, never holding up the server, with at
    most 6 at once. Each person may search or post 8 GIFs in a burst, then 1 a second.
  * Results get the server's own ids. People only get the GIF service's addresses
    through the server, and can only preview or post results of their own searches (so
    nobody can make the server fetch an address, or see what others searched for).
  * Only https GIF addresses on named hosts are downloaded (no IP addresses or
    localhost), only real GIF files are accepted (10 MB per GIF, 2 MB per preview), and
    posted GIFs go through the same permission checks as pictures.
* **GIF library** (1.28.0): only GIF files are read or accepted. Links and junctions in
  the `gifs` folder are skipped, so nothing outside it is ever shown. Names given in Voxa
  are cleaned: no folder separators or `..`, and Windows device names like `CON` are
  changed. Files are never overwritten. Adding and removing need *Manage server GIFs*,
  and adding is rate limited.
* **Limits against overload:**
  * at most 64 connections per address;
  * small messages until a client has logged in;
  * a chat rate limit;
  * at most 2000 channels, nested at most 16 levels deep;
  * bounded queues;
  * the upload quota cannot be bypassed by uploading in parallel.
* **Voice addresses:** voice is only sent to the address a client is connected from.
* **Hardware tokens:** only valid tokens count; a client that sends none (or junk) is
  disconnected; tokens are bound to the server's verified fingerprint, so another server
  posing as this one cannot obtain matching tokens. Administrators cannot be locked out by
  a hardware ban (like address bans).
* **Web admin page:**
  * Sign-in by proof of identity (the key stays in the browser or in Voxa), bound to the
    page's web address so phishing sites can't reuse it. The address must be one of
    `web_allowed_origins`, or (when that is empty) the one the first administrator
    signed in from (1.26.0; see [Web admin page](#web-admin-page)). The request's own
    `Host` header is not trusted for this, because whoever sends it chooses it.
  * One-time challenges expire after 2 minutes; 10 failed sign-ins from one address
    lock it out for 10 minutes.
  * Signing in with Voxa always needs a click in Voxa, which shows the web address.
  * The session cookie is `HttpOnly` and `SameSite=Strict`; every action also needs a
    header other sites can't send (no cross-site requests).
  * A strict Content Security Policy (no inline or third-party scripts), no framing, and
    all user text is shown as text, never as HTML.
  * Every action goes through the same permission checks as the desktop client.
  * Server statistics are only given to administrators (*Edit virtual server*).
* **No channel ("channel 0"):** it is not a real channel, and the server chat's storage
  slot shares its number, so every message that names channel 0 where a channel is needed
  (chat, history, pins, files, moving people, channel groups, keys) is refused, even for
  administrators.
* **Invisible people:** see [Features](#features). Nothing the server sends to people who
  can't see them reveals that they are connected.
* **Backups:** only an administrator can change where backups go or restore one (both ask
  for administrator rights); passwords travel to `voxa-server.exe` in a file in your own
  temp folder, never on a command line, and that file is deleted at once. Normal users may
  only ask for a backup into the folder an administrator chose. Temporary copies of the
  database and keys are readable by SYSTEM and Administrators only, and a restore only
  accepts the files it expects (no paths outside the data folder).
* **Text:** names, topics and messages are cleaned of control characters and invisible
  direction-changing characters.
* **Names can't be imitated** (1.26.0): see *Unique names* in [Features](#features).
* **Files** (1.26.0):
  * Paths are limited to 1024 characters, 32 folder levels and 255 characters per name,
    checked before the disk is touched (a huge path used to keep the server busy).
  * Each upload writes to its own randomly named partial file, and an upload number
    that is already running is refused, so partial files can't pile up outside the
    channel quota or overwrite each other.
* **Rank** (1.26.0): granting talk power, editing someone's description, removing them
  from a server group and changing their personal permissions need at least their rank,
  like kicks, moves and bans.
* **Hidden channels** (1.26.0): who has a camera on, deleted channels, and the answer to
  creating or moving a channel under a hidden parent no longer reveal hidden channels or
  the people in them.
* **Platform:** the platform shown for a person (Windows, Android) is what their app
  reports, cleaned and at most 32 characters. Only the server marks web admin page
  sessions as "Web": an app that claims to be "Web" is shown as "Unknown", so it can't
  pass itself off as (or hide among) web sessions.
* **Video and calendars:** see [Features](#features); every limit and permission is
  checked by the server, not just by Voxa.

## Data & configuration
Everything lives in `%ProgramData%\Voxa Server`: `server.toml`, `voxa.db`,
`identity.key`, `chat.key`, `files\`, `gifs\` (the GIF library), `gif-previews\`, `logs\`, `status.json`, `web_origin.txt` (the web
page's remembered address), and the backup settings
(`backup.json`, `backup.key`, `backup-status.json`).
* Use [Backups](#backups) rather than copying the folder: the `.key` files only work on
  the same machine, and copying `voxa.db` while the server runs can catch it half-written.
  A backup works on any PC.
* Uninstalling keeps this folder.

`server.toml` settings:

| Setting | Default | Meaning |
|---|---|---|
| `bind` | `0.0.0.0` | Address to listen on |
| `port` | `9987` | TCP and UDP port |
| `max_upload_mb` | `2048` | Initial max file size (0 = unlimited) |
| `channel_quota_mb` | `0` | Initial storage per channel (0 = unlimited) |
| `log_level` | `info` | `error`, `warn`, `info`, `debug` or `trace` |
| `require_latest_client` | `true` | Only accept the newest Voxa client |
| `web_enabled` | `true` | Serve the [web admin page](#web-admin-page) |
| `web_bind` | `0.0.0.0` | Address the web admin page listens on |
| `web_port` | `9980` | TCP port of the web admin page |
| `web_allowed_origins` | `[]` | Web addresses the page may be signed in from (empty = the first administrator's address, kept in `web_origin.txt`) |
| `gif_provider` | `klipy` | GIF search service: `klipy` or `giphy` |
| `gif_api_key` | `""` | That service's API key (empty = no GIF search) |
| `gif_content_filter` | `medium` | KLIPY `off`/`low`/`medium`/`high`; GIPHY `g`/`pg`/`pg-13`/`r` |

Restart the server after editing.

Command line:
* `voxa-server.exe --data <dir>`: run in a console (stop the service first).
* `voxa-server.exe --clear-bans`: remove every ban, e.g. if an administrator locked
  themselves out (stop the server first; run as administrator).
* `voxa-server.exe --check-update` / `--update`: see [Updates](#updates).

## Building
Requirements: Rust (rustup) and PowerShell.

```powershell
.\build.ps1
```

The script installs the `stable-x86_64-pc-windows-gnullvm` toolchain if needed and
downloads portable LLVM-MinGW and WiX 3.14 into `%LOCALAPPDATA%\voxa-build-tools`. It
writes the installer to `dist\`.

For development:
* `cargo run -p voxa-server -- --data .\devdata`
* `cargo run -p voxa-server-manager`

Tests:
* `cargo test -p voxa-server -p voxa-proto` runs the unit tests.
* End-to-end tests run against a running server, with
  `cargo run --release -p voxa-proto --example <name> -- 127.0.0.1:9987 <data dir>`:
  `smoke`, `moderation_test`, `transfer_test`, `image_test`, `clone_test` and
  `names_test` (unique names and the platform field) and `web_origin_test` (the web
  page's address pin; needs the web page on, and a third argument with its address,
  e.g. `127.0.0.1:9980`).
* The client repository has more: `security_test`, `feature_test`, `spacer_test` and
  others. Build those from the client repository: they report the client's version,
  which the server's version check expects.
* Start each test with a **fresh data folder**: the tests claim the one-time admin
  privilege key, and names stay reserved between runs. `feature_test` also needs
  `require_latest_client = true` (the others are easiest with `false`).

## Releasing an update
1. Bump `version` in `Cargo.toml` and commit.
2. Push a matching tag: `git tag v1.2.0` then `git push origin v1.2.0`.
3. GitHub Actions builds the MSI and publishes the release. Installed managers offer the
   update.

Release the matching Voxa client first. The server's `MIN_CLIENT_VERSION` (in
`state.rs`) is the oldest client it accepts.

### Test builds
New work goes to the **`test` branch** first. A test build is published as a GitHub
**pre-release** named `vX.Y.Z (test)`, made from the `test` branch. Pre-releases are not
"latest", so installed servers and Voxa Server Manager do not offer them as updates;
install them by hand from the release page. Once tested, merge `test` into `main` and
publish the normal release.

## Protocol crate
`crates/voxa-proto` is shared with [voxa-client](https://github.com/dylan0760/voxa-client).
Keep both copies identical.
* **Incompatible changes:** bump `PROTOCOL_VERSION`.
* **New message types:** append them at the end, and only send them to peers whose
  version supports them.

## License
Dual-licensed under MIT or Apache-2.0, at your option.
