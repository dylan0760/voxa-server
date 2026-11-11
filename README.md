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

## Ports
| Port | Protocol | Used for |
|------|----------|----------|
| **9987** | **TCP** | Control connection, encrypted chat and history, file transfers, screen sharing |
| **9987** | **UDP** | Encrypted voice |

If people connect from the internet, forward **TCP and UDP 9987** on your router to the
server PC. To use another port, change `port` in `server.toml`.

## Using Voxa Server Manager
Open it from the Start menu (**Voxa Server Manager**) or from its tray icon. It shows:
* service status, clients online, channels, uptime and fingerprint;
* **Start / Stop / Restart** buttons (no administrator rights needed);
* the **Server Admin privilege key**. In the client, click the server name ▸
  **Use Privilege Key** and paste it to become admin;
* buttons for the data folder, `server.toml` and the log, plus a live log view;
* **Updates**: checks GitHub and installs new versions.

Closing the window (✕) keeps the manager in the system tray. The server itself always
runs as a service, even when the manager is closed.

## Updates
* **Manager:** it checks this repository's releases at startup and every 12 hours.
  You can also click **Check for updates** in the window or the tray menu. When you
  install an update, the manager downloads the new MSI (verified against GitHub's
  SHA-256 digest). The installer stops the service, upgrades it, starts it again and
  reopens the manager.
* **Without a desktop:** run `voxa-server.exe --check-update` to check, or
  `voxa-server.exe --update` as administrator to download and install.

## Features
* Channels: permanent, semi-permanent and temporary; sub-channels, passwords, max
  clients, topic and description, codec quality, moderated channels (talk power).
* Server and channel groups with a permission editor, privilege keys, bans, kick, move,
  poke, channel commander.
* **Chat moderation:**
  * Delete single messages: your own always, anyone's with *Delete anyone's chat
    messages*.
  * Clear a whole channel or server chat with *Clear entire chat history*.
  * Both remove the messages for everyone and from the encrypted history, and every
    deletion is written to the server log.
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
  (XChaCha20-Poly1305). The master key is protected by Windows DPAPI. Retention is
  configurable.
* **Server-relayed screen sharing.** The server forwards changed screen tiles to
  viewers in the same channel.
* Per-channel file storage with a **max file size** and **storage per channel**, both
  unlimited if you like. Admins set them in the client: click the server name ▸
  **Edit Virtual Server** ▸ **File transfers**. `max_upload_mb` / `channel_quota_mb` in
  `server.toml` are only the initial defaults (0 = unlimited).

## Data & configuration
Everything lives in `%ProgramData%\Voxa Server`: `server.toml`, `voxa.db`,
`identity.key`, `chat.key`, `files\`, `logs\` and `status.json`. Back up the whole
folder. The `.key` files only work on the same machine. Uninstalling keeps this folder.

Run the server in a console with `voxa-server.exe --data <dir>` (stop the service first).

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

End-to-end protocol test against a running server:
`cargo run -p voxa-proto --example smoke -- 127.0.0.1:9987`.

## Releasing an update
1. Bump `version` in `Cargo.toml` and commit.
2. Push a matching tag: `git tag v1.2.0` then `git push origin v1.2.0`.
3. GitHub Actions builds the MSI and publishes the release. Installed managers offer the
   update.

## Protocol crate
`crates/voxa-proto` is shared with [voxa-client](https://github.com/dylan0760/voxa-client).
Keep both copies identical, and bump `PROTOCOL_VERSION` for incompatible changes.

## License
Dual-licensed under MIT or Apache-2.0, at your option.
