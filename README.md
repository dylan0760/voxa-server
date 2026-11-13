# Voxa Server

Voxa is free, open source voice chat with persistent encrypted text chat and screen
sharing. It is written in Rust for Windows. There are no license keys, accounts,
registrations or fees.

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
* opens **TCP 9987** and **UDP 9987** in Windows Firewall;
* installs **Voxa Server Manager**, which starts in the system tray at sign-in.

**When updating, update the Voxa clients first.** By default the server only accepts the
newest Voxa client (see [Client version](#client-version)).

## Ports
| Port | Protocol | Used for |
|------|----------|----------|
| **9987** | **TCP** | Control connection, encrypted chat and history, file transfers, screen sharing |
| **9987** | **UDP** | Encrypted voice |

If people connect from the internet, forward **TCP and UDP 9987** on your router to the
server PC. To use another port, change `port` in `server.toml`.

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
* **Updates**: checks GitHub and installs new versions;
* **What's new**, with the change log of every version.

Closing the window (✕) keeps the manager in the system tray. The server itself always
runs as a service, even when the manager is closed.

## Updates
* **Manager:** it checks this repository's releases at startup and every 12 hours. You
  can also click **Check for updates** in the window or the tray menu.
  * Updates are only installed from github.com, and only if the MSI matches the SHA-256
    checksum GitHub publishes.
  * The installer stops the service, upgrades it, starts it again and reopens the
    manager.
* **Without a desktop:** run `voxa-server.exe --check-update` to check, or
  `voxa-server.exe --update` as administrator to download and install.

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
* **Groups and moderation:** server and channel groups with a permission editor,
  privilege keys, bans, kick, move, poke and channel commander.
  * Moderators can only kick, move or ban people who have no permissions the moderator
    lacks.
  * Nobody can hand out permissions they don't have themselves.
* **Personal permissions:** grant individual permissions to a single user on top of
  their groups (client: right-click user ▸ **Permissions…**).
* **Channel access control:**
  * The **Join / switch channels** permission controls whether a group can change
    channels at all.
  * Each group has a **join power**: Admin 75, Normal 10, Guest 0 by default.
  * Each channel has a **needed join power**: 0 lets anyone in, 10 makes it
    members-only, 75 admins-only.
  * **Join channels regardless of join power** lets moderators bypass the check.
  * A channel group assigned in a specific channel also counts, so you can let
    individual people into one restricted channel.
* **Persistent encrypted chat:** channel and server chat is stored on the server
  (XChaCha20-Poly1305), with configurable retention.
  * Delete single messages: your own always, anyone's with *Delete anyone's chat
    messages*.
  * *Clear entire chat history* wipes a whole channel or server chat.
  * **Pinned messages**.
* **Images in chat:**
  * They are stored encrypted next to the chat history, with the same permissions as
    sending text there.
  * Only people who can see that chat can open them: everyone for server chat, channel
    members for a channel, and the two participants for a private chat.
  * PNG, JPEG, GIF, WebP and BMP, up to 10 MB each.
* **Server-relayed screen sharing** with sound, up to 60 fps. Only viewers in the same
  channel receive it.
* **File storage per channel:** a **max file size** and **storage per channel**, both
  unlimited if you like.
  * Admins set them in the client: click the server name ▸ **Edit Virtual Server** ▸
    **File transfers**.
  * `max_upload_mb` / `channel_quota_mb` in `server.toml` are only the initial defaults
    (0 = unlimited).

## Security
* **Connections:** Noise `XX_25519_ChaChaPoly_BLAKE2s` with mutual authentication.
  Voice uses ChaCha20-Poly1305 with a per-session key and replay protection.
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
* **Limits against overload:**
  * at most 64 connections per address;
  * small messages until a client has logged in;
  * a chat rate limit;
  * at most 2000 channels, nested at most 16 levels deep;
  * bounded queues;
  * the upload quota cannot be bypassed by uploading in parallel.
* **Voice addresses:** voice is only sent to the address a client is connected from.
* **Text:** names, topics and messages are cleaned of control characters and invisible
  direction-changing characters.

## Data & configuration
Everything lives in `%ProgramData%\Voxa Server`: `server.toml`, `voxa.db`,
`identity.key`, `chat.key`, `files\`, `logs\` and `status.json`.
* Back up the whole folder. The `.key` files only work on the same machine.
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
  `smoke`, `moderation_test`, `transfer_test`, `image_test` and `clone_test`.
* The client repository has more: `security_test`, `feature_test`, `spacer_test` and
  others. Build those from the client repository: they report the client's version,
  which the server's version check expects.

## Releasing an update
1. Bump `version` in `Cargo.toml` and commit.
2. Push a matching tag: `git tag v1.2.0` then `git push origin v1.2.0`.
3. GitHub Actions builds the MSI and publishes the release. Installed managers offer the
   update.

Release the matching Voxa client first. The server's `MIN_CLIENT_VERSION` (in
`state.rs`) is the oldest client it accepts.

## Protocol crate
`crates/voxa-proto` is shared with [voxa-client](https://github.com/dylan0760/voxa-client).
Keep both copies identical.
* **Incompatible changes:** bump `PROTOCOL_VERSION`.
* **New message types:** append them at the end, and only send them to peers whose
  version supports them.

## License
Dual-licensed under MIT or Apache-2.0, at your option.
