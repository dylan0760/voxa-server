//! Backups: everything this server needs in one password-protected file.
//!
//! * The running server makes them (on a daily / weekly schedule, or "Back up now" from
//!   Voxa Server Manager), so nobody is disconnected: the database is copied in one read
//!   transaction ([`Store::snapshot_to`]).
//! * A backup holds `server.toml`, the database (settings, channels, groups, permissions,
//!   bans, people, chat history, images, avatars), the server's identity and chat keys,
//!   the initial admin key and every uploaded channel file.
//! * Restoring (`--restore`, from the manager) stops the service, keeps the current data in
//!   `before-restore`, puts the backup in place and starts the service again.
//!
//! File format (`.voxabackup`): a 56-byte header (magic, format, Argon2id salt and
//! parameters, nonce prefix), then the content in chunks of at most 1 MiB, each sealed
//! with XChaCha20-Poly1305 under a key derived from the password. The nonce counts the
//! chunks and the last chunk is marked, so chunks can't be reordered, dropped or cut off.
//! The content is a list of entries (name, size, bytes) ending with a zero tag.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::state::Server;

pub const EXT: &str = "voxabackup";
const MAGIC: &[u8; 8] = b"VOXABAK1";
const FORMAT: u32 = 1;
const HEADER_LEN: usize = 56;
const CHUNK: usize = 1 << 20;
/// Backup files are named `Voxa Server backup 2026-11-15 030000.voxabackup`.
const NAME_PREFIX: &str = "Voxa Server backup ";
/// Argon2id: 64 MiB, 3 passes.
const KDF_MEMORY_KIB: u32 = 64 * 1024;
const KDF_PASSES: u32 = 3;
pub const MIN_PASSWORD: usize = 8;

/// Service control code (128-255 are for services) for "back up now".
pub const CONTROL_BACKUP_NOW: u32 = 130;
/// Woken by the service control above.
pub static BACKUP_NOW: tokio::sync::Notify = tokio::sync::Notify::const_new();

const SETTINGS_FILE: &str = "backup.json";
const SECRETS_FILE: &str = "backup.key";
const STATUS_FILE: &str = "backup-status.json";

// ------------------------------------------------------------------ settings & status

/// Backup settings (`backup.json`, readable by Voxa Server Manager). The passwords are
/// kept apart in `backup.key`, readable by SYSTEM and Administrators only.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct BackupSettings {
    /// Automatic backups on.
    pub enabled: bool,
    /// Where backups go: a folder on this PC or a network share (`\\nas\backups\voxa`).
    pub folder: String,
    /// Weekly on `weekday` (0 = Monday), otherwise daily.
    pub weekly: bool,
    pub weekday: u8,
    /// Local hour of the day (0-23).
    pub hour: u8,
    /// How many backups to keep in the folder (0 = all).
    pub keep: u32,
    /// Account for the network share (empty: the service's own access).
    pub share_user: String,
    /// A backup password is saved (the password itself is in `backup.key`).
    pub password_set: bool,
    /// When automatic backups were turned on (the schedule counts from there).
    pub since: u64,
}

impl Default for BackupSettings {
    fn default() -> Self {
        BackupSettings { enabled: false, folder: String::new(), weekly: false, weekday: 6, hour: 3, keep: 7, share_user: String::new(), password_set: false, since: 0 }
    }
}

#[derive(Serialize, Deserialize, Default)]
struct Secrets {
    password: String,
    share_password: String,
}

/// What the last backups did (`backup-status.json`, shown by Voxa Server Manager).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct BackupStatus {
    pub running: bool,
    pub last_attempt: u64,
    pub last_ok: u64,
    pub last_file: String,
    pub last_size: u64,
    /// Error of the last attempt (empty when it worked).
    pub last_error: String,
    /// Next automatic backup (0 = none planned).
    pub next: u64,
}

pub fn load_settings(dir: &Path) -> BackupSettings {
    fs::read(dir.join(SETTINGS_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn load_secrets(dir: &Path) -> Option<Secrets> {
    let wrapped = fs::read(dir.join(SECRETS_FILE)).ok()?;
    serde_json::from_slice(&crate::secrets::unwrap(&wrapped).ok()?).ok()
}

fn load_status(dir: &Path) -> BackupStatus {
    fs::read(dir.join(STATUS_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_status(dir: &Path, s: &BackupStatus) {
    if let Ok(j) = serde_json::to_vec_pretty(s) {
        let _ = fs::write(dir.join(STATUS_FILE), j);
    }
}

/// Saves new settings (`--backup-settings`, run as administrator). `password` /
/// `share_password` None keep the saved ones.
pub fn save_settings(dir: &Path, mut s: BackupSettings, password: Option<String>, share_password: Option<String>, lock: bool) -> anyhow::Result<()> {
    let folder = s.folder.trim().trim_end_matches(['\\', '/']).to_string();
    if s.enabled || !folder.is_empty() {
        let p = Path::new(&folder);
        if !(p.is_absolute() || folder.starts_with("\\\\")) {
            bail!("Choose a full folder path, like D:\\Backups or \\\\nas\\backups");
        }
        if folder.contains(['"', '*', '?', '<', '>', '|']) {
            bail!("The folder name contains characters Windows does not allow");
        }
    }
    if s.hour > 23 || s.weekday > 6 || s.keep > 1000 {
        bail!("Invalid schedule");
    }
    let old = load_secrets(dir).unwrap_or_default();
    let secrets = Secrets {
        password: password.unwrap_or(old.password),
        share_password: if s.share_user.trim().is_empty() { String::new() } else { share_password.unwrap_or(old.share_password) },
    };
    if (s.enabled || !secrets.password.is_empty()) && secrets.password.chars().count() < MIN_PASSWORD {
        bail!("The backup password must be at least {MIN_PASSWORD} characters");
    }
    let before = load_settings(dir);
    s.folder = folder;
    s.share_user = s.share_user.trim().to_string();
    s.password_set = !secrets.password.is_empty();
    // Turning automatic backups on (or changing the schedule) starts counting from now.
    s.since = if s.enabled && (!before.enabled || before.weekly != s.weekly || before.weekday != s.weekday || before.hour != s.hour) {
        voxa_proto::now_unix()
    } else {
        before.since
    };
    let key_path = dir.join(SECRETS_FILE);
    fs::write(&key_path, crate::secrets::wrap(&serde_json::to_vec(&secrets)?))?;
    #[cfg(windows)]
    if lock {
        crate::acl::lock_private(&key_path);
    }
    let _ = lock;
    fs::write(dir.join(SETTINGS_FILE), serde_json::to_vec_pretty(&s)?)?;
    Ok(())
}

// ------------------------------------------------------------------ sealed stream

fn derive_key(password: &str, salt: &[u8], memory_kib: u32, passes: u32, lanes: u32) -> anyhow::Result<[u8; 32]> {
    // Limits for files from elsewhere: at most 1 GiB and 16 passes / lanes.
    if memory_kib > 1 << 20 || passes == 0 || passes > 16 || lanes == 0 || lanes > 16 {
        bail!("This backup uses settings Voxa does not accept");
    }
    let params = argon2::Params::new(memory_kib, passes, lanes, Some(32)).map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut key = [0u8; 32];
    argon2::Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(key)
}

fn nonce(prefix: &[u8; 16], counter: u64) -> XNonce {
    let mut n = [0u8; 24];
    n[..16].copy_from_slice(prefix);
    n[16..].copy_from_slice(&counter.to_le_bytes());
    XNonce::from(n)
}

/// Encrypts everything written to it into sealed chunks.
struct SealedWriter<W: Write> {
    out: W,
    cipher: XChaCha20Poly1305,
    header: Vec<u8>,
    prefix: [u8; 16],
    counter: u64,
    buf: Vec<u8>,
}

impl<W: Write> SealedWriter<W> {
    fn new(mut out: W, password: &str) -> anyhow::Result<Self> {
        let salt: [u8; 16] = crate::secrets::random_bytes();
        let prefix: [u8; 16] = crate::secrets::random_bytes();
        let mut header = Vec::with_capacity(HEADER_LEN);
        header.extend_from_slice(MAGIC);
        header.extend_from_slice(&FORMAT.to_le_bytes());
        header.extend_from_slice(&salt);
        for v in [KDF_MEMORY_KIB, KDF_PASSES, 1] {
            header.extend_from_slice(&v.to_le_bytes());
        }
        header.extend_from_slice(&prefix);
        debug_assert_eq!(header.len(), HEADER_LEN);
        let key = derive_key(password, &salt, KDF_MEMORY_KIB, KDF_PASSES, 1)?;
        out.write_all(&header)?;
        let cipher = XChaCha20Poly1305::new_from_slice(&key).expect("32-byte key");
        Ok(SealedWriter { out, cipher, header, prefix, counter: 0, buf: Vec::with_capacity(CHUNK) })
    }

    fn seal(&mut self, last: bool) -> io::Result<()> {
        let mut aad = self.header.clone();
        aad.push(last as u8);
        let ct = self
            .cipher
            .encrypt(&nonce(&self.prefix, self.counter), Payload { msg: &self.buf, aad: &aad })
            .map_err(|_| io::Error::other("encryption failed"))?;
        self.out.write_all(&(ct.len() as u32).to_le_bytes())?;
        self.out.write_all(&ct)?;
        self.counter += 1;
        self.buf.clear();
        Ok(())
    }

    fn finish(mut self) -> io::Result<W> {
        self.seal(true)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

impl<W: Write> Write for SealedWriter<W> {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        let n = data.len().min(CHUNK - self.buf.len());
        self.buf.extend_from_slice(&data[..n]);
        if self.buf.len() == CHUNK {
            self.seal(false)?;
        }
        Ok(n)
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Reads and checks what [`SealedWriter`] wrote.
struct SealedReader<R: Read> {
    inp: R,
    cipher: XChaCha20Poly1305,
    header: Vec<u8>,
    prefix: [u8; 16],
    counter: u64,
    plain: Vec<u8>,
    pos: usize,
    done: bool,
}

impl<R: Read> SealedReader<R> {
    fn new(mut inp: R, password: &str) -> anyhow::Result<Self> {
        let mut header = vec![0u8; HEADER_LEN];
        inp.read_exact(&mut header).map_err(|_| anyhow::anyhow!("This is not a Voxa Server backup"))?;
        if &header[..8] != MAGIC {
            bail!("This is not a Voxa Server backup");
        }
        let u32_at = |i: usize| u32::from_le_bytes(header[i..i + 4].try_into().unwrap());
        if u32_at(8) != FORMAT {
            bail!("This backup was made by a newer Voxa Server: update first");
        }
        let salt = header[12..28].to_vec();
        let key = derive_key(password, &salt, u32_at(28), u32_at(32), u32_at(36))?;
        let prefix: [u8; 16] = header[40..56].try_into().unwrap();
        let cipher = XChaCha20Poly1305::new_from_slice(&key).expect("32-byte key");
        let mut r = SealedReader { inp, cipher, header, prefix, counter: 0, plain: Vec::new(), pos: 0, done: false };
        // The first chunk tells a wrong password early.
        r.next_chunk().map_err(|_| anyhow::anyhow!("Wrong password, or the backup is damaged"))?;
        Ok(r)
    }

    fn next_chunk(&mut self) -> io::Result<()> {
        let mut len = [0u8; 4];
        self.inp.read_exact(&mut len).map_err(|_| io::Error::other("The backup is incomplete (cut off)"))?;
        let len = u32::from_le_bytes(len) as usize;
        if len > CHUNK + 16 {
            return Err(io::Error::other("The backup is damaged"));
        }
        let mut ct = vec![0u8; len];
        self.inp.read_exact(&mut ct).map_err(|_| io::Error::other("The backup is incomplete (cut off)"))?;
        let n = nonce(&self.prefix, self.counter);
        let open = |last: bool| {
            let mut aad = self.header.clone();
            aad.push(last as u8);
            self.cipher.decrypt(&n, Payload { msg: &ct, aad: &aad })
        };
        let (plain, last) = match open(false) {
            Ok(p) => (p, false),
            Err(_) => (open(true).map_err(|_| io::Error::other("Wrong password, or the backup is damaged"))?, true),
        };
        if last {
            // Nothing may follow the last chunk.
            let mut extra = [0u8; 1];
            if self.inp.read(&mut extra)? != 0 {
                return Err(io::Error::other("The backup is damaged (data after the end)"));
            }
            self.done = true;
        }
        self.counter += 1;
        self.plain = plain;
        self.pos = 0;
        Ok(())
    }
}

impl<R: Read> Read for SealedReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        while self.pos == self.plain.len() {
            if self.done {
                return Ok(0);
            }
            self.next_chunk()?;
        }
        let n = out.len().min(self.plain.len() - self.pos);
        out[..n].copy_from_slice(&self.plain[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

// ------------------------------------------------------------------ entries

fn put_entry(w: &mut impl Write, name: &str, size: u64, data: &mut impl Read) -> io::Result<()> {
    w.write_all(&[1])?;
    w.write_all(&(name.len() as u16).to_le_bytes())?;
    w.write_all(name.as_bytes())?;
    w.write_all(&size.to_le_bytes())?;
    let copied = io::copy(&mut data.by_ref().take(size), w)?;
    if copied != size {
        return Err(io::Error::other(format!("{name} changed while it was being backed up")));
    }
    Ok(())
}

fn put_bytes(w: &mut impl Write, name: &str, data: &[u8]) -> io::Result<()> {
    put_entry(w, name, data.len() as u64, &mut &data[..])
}

fn put_file(w: &mut impl Write, name: &str, path: &Path) -> io::Result<()> {
    let f = File::open(path)?;
    let size = f.metadata()?.len();
    put_entry(w, name, size, &mut BufReader::new(f))
}

/// Channel files, without following links or junctions.
fn put_dir(w: &mut impl Write, prefix: &str, dir: &Path) -> io::Result<u64> {
    let mut n = 0;
    let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let Some(name) = e.file_name().to_str().map(String::from) else {
            warn!("backup: skipped a file whose name is not valid Unicode in {}", dir.display());
            continue;
        };
        let meta = fs::symlink_metadata(e.path())?;
        let rel = format!("{prefix}/{name}");
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            n += put_dir(w, &rel, &e.path())?;
        } else if meta.is_file() {
            put_file(w, &rel, &e.path())?;
            n += 1;
        }
    }
    Ok(n)
}

/// Names a backup may contain: fixed files, and channel files below `files/`.
fn allowed_name(name: &str) -> bool {
    match name {
        "manifest.json" | "server.toml" | "voxa.db" | "keys/identity" | "keys/chat" | "privilege_key.txt" => true,
        _ => name.strip_prefix("files/").is_some_and(|rest| {
            !rest.is_empty()
                && rest.split('/').all(|c| {
                    !c.is_empty() && c != "." && c != ".." && c.len() <= 255 && !c.ends_with(['.', ' ']) && !c.contains(['\\', ':', '\0', '<', '>', '"', '|', '?', '*'])
                })
        }),
    }
}

#[derive(Serialize, Deserialize)]
struct Manifest {
    format: u32,
    created: u64,
    server_version: String,
    fingerprint: String,
    channel_files: u64,
}

// ------------------------------------------------------------------ making a backup

#[cfg(windows)]
mod share {
    use windows_sys::Win32::NetworkManagement::WNet::{WNetAddConnection2W, WNetCancelConnection2W, NETRESOURCEW, RESOURCETYPE_DISK};

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// `\\server\share` of a UNC path.
    pub fn root(folder: &str) -> Option<String> {
        let rest = folder.strip_prefix("\\\\")?;
        let mut parts = rest.split('\\').filter(|p| !p.is_empty());
        Some(format!("\\\\{}\\{}", parts.next()?, parts.next()?))
    }

    /// Signs in to the share with the saved account. Returns the share to disconnect.
    pub fn connect(folder: &str, user: &str, password: &str) -> Result<Option<String>, String> {
        let Some(remote) = root(folder).filter(|_| !user.is_empty()) else { return Ok(None) };
        let mut r = wide(&remote);
        let mut nr: NETRESOURCEW = unsafe { std::mem::zeroed() };
        nr.dwType = RESOURCETYPE_DISK;
        nr.lpRemoteName = r.as_mut_ptr();
        let (p, u) = (wide(password), wide(user));
        match unsafe { WNetAddConnection2W(&nr, p.as_ptr(), u.as_ptr(), 0) } {
            0 => Ok(Some(remote)),
            // Already connected (e.g. with the same account): use that connection.
            1219 => Ok(None),
            86 | 1326 => Err(format!("The network share {remote} refused the user name or password")),
            code => Err(format!("Could not connect to the network share {remote} (Windows error {code})")),
        }
    }

    pub fn disconnect(remote: &str) {
        let r = wide(remote);
        unsafe {
            WNetCancelConnection2W(r.as_ptr(), 0, 1);
        }
    }
}

/// A local-time name for a new backup file.
fn file_name(now: u64) -> String {
    use chrono::TimeZone;
    let t = chrono::Local.timestamp_opt(now as i64, 0).single().unwrap_or_else(chrono::Local::now);
    format!("{NAME_PREFIX}{}.{EXT}", t.format("%Y-%m-%d %H%M%S"))
}

/// Makes a backup into the configured folder. Returns the file and its size.
pub fn create(server: &Server, lock: bool) -> anyhow::Result<(PathBuf, u64)> {
    let dir = &server.data_dir;
    let s = load_settings(dir);
    let secrets = load_secrets(dir).filter(|x| !x.password.is_empty());
    let (Some(secrets), false) = (secrets, s.folder.is_empty()) else {
        bail!("Choose a backup folder and password first (Voxa Server Manager ▸ Backups ▸ Settings)");
    };
    // The database copy is private while it exists.
    let temp = dir.join("backup-temp");
    let _ = fs::remove_dir_all(&temp);
    fs::create_dir_all(&temp)?;
    #[cfg(windows)]
    if lock {
        crate::acl::lock_private(&temp);
    }
    let _ = lock;
    let result = (|| -> anyhow::Result<(PathBuf, u64)> {
        let db_copy = temp.join("voxa.db");
        server.store.snapshot_to(&db_copy).context("copying the database")?;
        let identity = crate::secrets::unwrap(&fs::read(dir.join("identity.key"))?)?;
        let chat = crate::secrets::unwrap(&fs::read(dir.join("chat.key"))?)?;

        #[cfg(windows)]
        let connected = share::connect(&s.folder, &s.share_user, &secrets.share_password).map_err(|e| anyhow::anyhow!(e))?;
        let write = || -> anyhow::Result<(PathBuf, u64)> {
            let folder = Path::new(&s.folder);
            fs::create_dir_all(folder).with_context(|| format!("Cannot use the folder {}", s.folder))?;
            let now = voxa_proto::now_unix();
            let path = folder.join(file_name(now));
            let partial = path.with_extension(format!("{EXT}.partial"));
            let file = File::create(&partial).with_context(|| format!("Cannot write to {}", s.folder))?;
            let written = (|| -> anyhow::Result<()> {
                let mut w = SealedWriter::new(BufWriter::new(file), &secrets.password)?;
                let toml = dir.join("server.toml");
                if toml.exists() {
                    put_file(&mut w, "server.toml", &toml)?;
                }
                put_bytes(&mut w, "keys/identity", &identity)?;
                put_bytes(&mut w, "keys/chat", &chat)?;
                put_file(&mut w, "voxa.db", &db_copy)?;
                let pk = dir.join("privilege_key.txt");
                if pk.exists() {
                    put_file(&mut w, "privilege_key.txt", &pk)?;
                }
                let files = dir.join("files");
                let channel_files = if files.is_dir() { put_dir(&mut w, "files", &files)? } else { 0 };
                let manifest = Manifest {
                    format: FORMAT,
                    created: now,
                    server_version: voxa_proto::APP_VERSION.into(),
                    fingerprint: server.fingerprint.clone(),
                    channel_files,
                };
                put_bytes(&mut w, "manifest.json", &serde_json::to_vec_pretty(&manifest)?)?;
                w.write_all(&[0])?;
                let out = w.finish()?;
                out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
                Ok(())
            })();
            if let Err(e) = written {
                let _ = fs::remove_file(&partial);
                return Err(e);
            }
            fs::rename(&partial, &path)?;
            let size = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            prune(folder, s.keep);
            Ok((path, size))
        };
        let r = write();
        #[cfg(windows)]
        if let Some(remote) = connected {
            share::disconnect(&remote);
        }
        r
    })();
    let _ = fs::remove_dir_all(&temp);
    result
}

/// Deletes the oldest backups beyond `keep` (only files named like Voxa's backups).
fn prune(folder: &Path, keep: u32) {
    let Ok(rd) = fs::read_dir(folder) else { return };
    let mut ours: Vec<String> = rd
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().to_str().map(String::from))
        .filter(|n| n.starts_with(NAME_PREFIX))
        .collect();
    // Leftovers of interrupted backups.
    for n in ours.iter().filter(|n| n.ends_with(".partial")) {
        let p = folder.join(n);
        let old = fs::metadata(&p).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|e| e > Duration::from_secs(86400));
        if old {
            let _ = fs::remove_file(p);
        }
    }
    ours.retain(|n| n.ends_with(&format!(".{EXT}")));
    if keep == 0 || ours.len() <= keep as usize {
        return;
    }
    ours.sort();
    for n in &ours[..ours.len() - keep as usize] {
        match fs::remove_file(folder.join(n)) {
            Ok(()) => info!("backup: removed the old backup {n}"),
            Err(e) => warn!("backup: could not remove the old backup {n}: {e}"),
        }
    }
}

// ------------------------------------------------------------------ schedule

/// The first scheduled time after `after` (local time).
pub fn next_after(s: &BackupSettings, after: u64) -> u64 {
    use chrono::{Datelike, Duration as D, Local, TimeZone};
    let Some(base) = Local.timestamp_opt(after as i64, 0).single() else { return after + 86400 };
    let mut day = base.date_naive();
    for _ in 0..16 {
        let ok_day = !s.weekly || day.weekday().num_days_from_monday() == s.weekday as u32;
        if ok_day {
            if let Some(t) = day.and_hms_opt(s.hour as u32, 0, 0).and_then(|t| Local.from_local_datetime(&t).earliest()) {
                if t.timestamp() > after as i64 {
                    return t.timestamp() as u64;
                }
            }
        }
        day += D::days(1);
    }
    after + 86400
}

/// When the next automatic backup is due (0 = not planned).
fn next_due(s: &BackupSettings, st: &BackupStatus) -> u64 {
    if !s.enabled || !s.password_set || s.folder.is_empty() {
        return 0;
    }
    let due = next_after(s, s.since.max(st.last_ok));
    // A failed attempt is retried an hour later.
    if st.last_attempt > st.last_ok && st.last_attempt >= due {
        return st.last_attempt + 3600;
    }
    due
}

/// Makes backups when they are due, or when asked to (service control 130).
pub async fn scheduler(server: Arc<Server>, lock: bool) {
    let dir = server.data_dir.clone();
    let mut st = load_status(&dir);
    st.running = false;
    let mut saved = BackupStatus { next: u64::MAX, ..Default::default() };
    loop {
        let s = load_settings(&dir);
        st.next = next_due(&s, &st);
        // Only rewritten when something changed (antivirus rescans rewritten files).
        if st != saved {
            save_status(&dir, &st);
            saved = st.clone();
        }
        let now = voxa_proto::now_unix();
        // `backup-now` in the data folder (only administrators can create it) also asks.
        let asked = fs::remove_file(dir.join("backup-now")).is_ok();
        let due = asked || (st.next != 0 && now >= st.next);
        if !due {
            tokio::select! {
                _ = BACKUP_NOW.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(5)) => continue,
            }
        }
        st.running = true;
        st.last_attempt = voxa_proto::now_unix();
        save_status(&dir, &st);
        saved = st.clone();
        info!("backup: starting");
        let srv = server.clone();
        let r = tokio::task::spawn_blocking(move || create(&srv, lock)).await;
        st.running = false;
        match r {
            Ok(Ok((path, size))) => {
                info!("backup: saved {} ({} bytes)", path.display(), size);
                st.last_ok = voxa_proto::now_unix();
                st.last_file = path.display().to_string();
                st.last_size = size;
                st.last_error.clear();
            }
            Ok(Err(e)) => {
                warn!("backup failed: {e:#}");
                st.last_error = format!("{e:#}");
            }
            Err(e) => st.last_error = format!("The backup stopped unexpectedly: {e}"),
        }
    }
}

// ------------------------------------------------------------------ restoring

#[cfg(windows)]
mod svc {
    use std::time::{Duration, Instant};
    use windows_service::service::{ServiceAccess, ServiceState};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    fn open() -> Option<windows_service::service::Service> {
        let m = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT).ok()?;
        m.open_service(crate::service::SERVICE_NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::START | ServiceAccess::STOP).ok()
    }

    /// Stops the service if it runs. Returns whether it was running.
    pub fn stop() -> anyhow::Result<bool> {
        let Some(s) = open() else { return Ok(false) };
        if s.query_status()?.current_state == ServiceState::Stopped {
            return Ok(false);
        }
        let _ = s.stop();
        let deadline = Instant::now() + Duration::from_secs(60);
        while s.query_status()?.current_state != ServiceState::Stopped {
            if Instant::now() > deadline {
                anyhow::bail!("The Voxa Server service did not stop");
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        // The process may still be letting go of the database.
        std::thread::sleep(Duration::from_millis(500));
        Ok(true)
    }

    pub fn start() {
        if let Some(s) = open() {
            let _ = s.start::<&str>(&[]);
        }
    }
}

/// What can be restored, in place of what.
const RESTORED: &[&str] = &["server.toml", "voxa.db", "identity.key", "chat.key", "privilege_key.txt", "files"];

/// Restores a backup into `dir` (run as administrator). The current data is moved to
/// `before-restore` first. `service`: stop the Windows service first and start it again.
pub fn restore(dir: &Path, file: &Path, password: &str, service: bool, lock: bool) -> anyhow::Result<String> {
    // 1. Unpack and check everything before touching the server.
    let staging = dir.join("restore-temp");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)?;
    #[cfg(windows)]
    if lock {
        crate::acl::lock_private(&staging);
    }
    let unpacked = unpack(file, password, &staging);
    let manifest = match unpacked {
        Ok(m) => m,
        Err(e) => {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
    };

    // 2. Stop the server, keep the current data, put the backup in place.
    #[cfg(windows)]
    let was_running = if service { svc::stop()? } else { false };
    #[cfg(not(windows))]
    let was_running = {
        let _ = service;
        false
    };
    let before = dir.join("before-restore");
    let _ = fs::remove_dir_all(&before);
    fs::create_dir_all(&before)?;
    let mut moved = Vec::new();
    let mut placed = || -> anyhow::Result<()> {
        for name in RESTORED {
            let p = dir.join(name);
            if p.exists() {
                fs::rename(&p, before.join(name)).with_context(|| format!("moving {name} aside (is the server still running?)"))?;
                moved.push(*name);
            }
        }
        let keys = staging.join("keys");
        fs::write(dir.join("identity.key"), crate::secrets::wrap(&fs::read(keys.join("identity"))?))?;
        fs::write(dir.join("chat.key"), crate::secrets::wrap(&fs::read(keys.join("chat"))?))?;
        fs::rename(staging.join("voxa.db"), dir.join("voxa.db"))?;
        for name in ["server.toml", "privilege_key.txt", "files"] {
            if staging.join(name).exists() {
                fs::rename(staging.join(name), dir.join(name))?;
            }
        }
        fs::create_dir_all(dir.join("files"))?;
        Ok(())
    };
    let r = placed();
    if let Err(e) = r {
        // Put the previous data back.
        for name in RESTORED {
            if moved.contains(name) {
                let p = dir.join(name);
                if p.is_dir() {
                    let _ = fs::remove_dir_all(&p);
                } else {
                    let _ = fs::remove_file(&p);
                }
                let _ = fs::rename(before.join(name), &p);
            }
        }
        let _ = fs::remove_dir_all(&staging);
        #[cfg(windows)]
        if was_running {
            svc::start();
        }
        return Err(e.context("restoring failed; the previous data was put back"));
    }
    let _ = fs::remove_dir_all(&staging);
    #[cfg(windows)]
    if lock {
        crate::acl::lock_down(dir);
        crate::acl::lock_private(&before);
    }
    #[cfg(windows)]
    if was_running {
        svc::start();
    }
    let _ = lock;
    let when = {
        use chrono::TimeZone;
        chrono::Local.timestamp_opt(manifest.created as i64, 0).single().map(|t| t.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
    };
    let n = manifest.channel_files;
    Ok(format!(
        "Restored the backup from {when} (Voxa Server {}, {n} channel file{}). The previous data is in {}.",
        manifest.server_version,
        if n == 1 { "" } else { "s" },
        before.display()
    ))
}

/// Decrypts a backup into `staging` and checks it can be used.
fn unpack(file: &Path, password: &str, staging: &Path) -> anyhow::Result<Manifest> {
    let f = File::open(file).with_context(|| format!("Cannot open {}", file.display()))?;
    let mut r = SealedReader::new(BufReader::new(f), password)?;
    let mut manifest: Option<Manifest> = None;
    loop {
        let mut tag = [0u8; 1];
        r.read_exact(&mut tag)?;
        match tag[0] {
            0 => break,
            1 => {}
            _ => bail!("The backup is damaged"),
        }
        let mut len = [0u8; 2];
        r.read_exact(&mut len)?;
        let mut name = vec![0u8; u16::from_le_bytes(len) as usize];
        r.read_exact(&mut name)?;
        let name = String::from_utf8(name).map_err(|_| anyhow::anyhow!("The backup is damaged"))?;
        if !allowed_name(&name) {
            bail!("The backup contains an unexpected entry: {name}");
        }
        let mut size = [0u8; 8];
        r.read_exact(&mut size)?;
        let size = u64::from_le_bytes(size);
        let target = staging.join(name.replace('/', "\\"));
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        if name == "manifest.json" {
            let mut b = Vec::new();
            (&mut r).take(size.min(1 << 16)).read_to_end(&mut b)?;
            manifest = Some(serde_json::from_slice(&b).map_err(|_| anyhow::anyhow!("The backup is damaged"))?);
            continue;
        }
        let mut out = BufWriter::new(File::create(&target)?);
        let n = io::copy(&mut (&mut r).take(size), &mut out)?;
        if n != size {
            bail!("The backup is incomplete");
        }
        out.flush()?;
    }
    // Everything after the end tag must be the end of the stream.
    let mut extra = [0u8; 1];
    if r.read(&mut extra)? != 0 {
        bail!("The backup is damaged");
    }
    let manifest = manifest.ok_or_else(|| anyhow::anyhow!("The backup is damaged (no manifest)"))?;
    if manifest.format != FORMAT {
        bail!("This backup was made by a newer Voxa Server: update first");
    }
    let keys = staging.join("keys");
    let identity = fs::read(keys.join("identity")).map_err(|_| anyhow::anyhow!("The backup has no server key"))?;
    let chat = fs::read(keys.join("chat")).map_err(|_| anyhow::anyhow!("The backup has no chat key"))?;
    if voxa_proto::identity::KeyPair::import(&base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &identity)).is_none() || chat.len() != 32 {
        bail!("The backup's keys are damaged");
    }
    if !staging.join("voxa.db").exists() {
        bail!("The backup has no database");
    }
    // The database must open with its chat key (this also upgrades an older one).
    drop(crate::store::Store::open(&staging.join("voxa.db"), &chat).context("The backup's database cannot be opened")?);
    if let Ok(t) = fs::read_to_string(staging.join("server.toml")) {
        toml::from_str::<crate::config::Config>(&t).context("The backup's server.toml is invalid")?;
    }
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sealed_stream_round_trip_and_tamper() {
        let mut data = vec![0u8; CHUNK * 2 + 123];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        let mut w = SealedWriter::new(Vec::new(), "correct horse").unwrap();
        w.write_all(&data).unwrap();
        let file = w.finish().unwrap();
        let mut back = Vec::new();
        SealedReader::new(&file[..], "correct horse").unwrap().read_to_end(&mut back).unwrap();
        assert_eq!(back, data);
        // Wrong password.
        assert!(SealedReader::new(&file[..], "wrong horse!").is_err());
        // Cut off after the first chunk: the end marker is missing.
        let first = HEADER_LEN + 4 + CHUNK + 16;
        let mut r = SealedReader::new(&file[..first], "correct horse").unwrap();
        assert!(r.read_to_end(&mut Vec::new()).is_err());
        // A flipped bit in the second chunk.
        let mut bad = file.clone();
        bad[first + 100] ^= 1;
        let mut r = SealedReader::new(&bad[..], "correct horse").unwrap();
        assert!(r.read_to_end(&mut Vec::new()).is_err());
        // Extra bytes after the end.
        let mut long = file.clone();
        long.push(0);
        let mut r = SealedReader::new(&long[..], "correct horse").unwrap();
        assert!(r.read_to_end(&mut Vec::new()).is_err());
    }

    #[test]
    fn entry_names_cannot_escape() {
        for ok in ["voxa.db", "keys/chat", "files/channel_1/a.txt", "files/channel_2/sub dir/b.png"] {
            assert!(allowed_name(ok), "{ok}");
        }
        for bad in ["../x", "files/../voxa.db", "files/", "files//a", "files/a/..", "C:/x", "files/C:x", "files/a\\..\\b", "identity.key", "files/a.", "files/nul\0"] {
            assert!(!allowed_name(bad), "{bad}");
        }
    }

    #[test]
    fn schedule() {
        use chrono::{Datelike, Local, TimeZone, Timelike};
        let s = BackupSettings { enabled: true, hour: 3, ..Default::default() };
        let base = Local.with_ymd_and_hms(2026, 11, 15, 10, 0, 0).unwrap().timestamp() as u64;
        let next = Local.timestamp_opt(next_after(&s, base) as i64, 0).unwrap();
        assert_eq!((next.day(), next.hour()), (16, 3));
        let weekly = BackupSettings { weekly: true, weekday: 0, ..s.clone() };
        let next = Local.timestamp_opt(next_after(&weekly, base) as i64, 0).unwrap();
        assert_eq!((next.weekday().num_days_from_monday(), next.hour()), (0, 3));
        assert!(next.timestamp() as u64 > base && next.timestamp() as u64 <= base + 7 * 86400);
        // A failed attempt is retried an hour later.
        let st = BackupStatus { last_attempt: next_after(&s, base) + 5, last_ok: 0, ..Default::default() };
        let s2 = BackupSettings { password_set: true, folder: "D:\\b".into(), since: base, ..s };
        assert_eq!(next_due(&s2, &st), st.last_attempt + 3600);
    }
}
