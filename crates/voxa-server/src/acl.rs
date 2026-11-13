//! Locks down the permissions of the data folder when running as the Windows service.
//!
//! The folder lives in ProgramData, where every local user may create files and folders
//! by default. The service runs as SYSTEM, so a folder link planted there by a normal
//! user could make the service write or delete files anywhere. After this runs:
//! SYSTEM and Administrators have full control, Users can only read (Voxa Server Manager
//! shows the status, log and initial admin key without administrator rights), and the
//! database, key files and uploaded files are readable by SYSTEM and Administrators only.

use std::path::Path;
use std::process::Command;

use tracing::{info, warn};

const SYSTEM: &str = "*S-1-5-18";
const ADMINS: &str = "*S-1-5-32-544";
const USERS: &str = "*S-1-5-32-545";

/// Files and folders that only SYSTEM and Administrators may read.
const PRIVATE: &[&str] = &["identity.key", "chat.key", "voxa.db", "files"];

/// Marker for the one-time cleanup of objects created before the lockdown.
const MARKER: &str = ".permissions-v1";

fn icacls(args: &[&std::ffi::OsStr]) -> bool {
    let exe = std::env::var_os("SystemRoot")
        .map(|r| Path::new(&r).join("System32").join("icacls.exe"))
        .unwrap_or_else(|| "C:\\Windows\\System32\\icacls.exe".into());
    match Command::new(exe).args(args).output() {
        Ok(o) if o.status.success() => true,
        Ok(o) => {
            warn!("icacls {:?} failed: {}", args, String::from_utf8_lossy(&o.stdout).trim());
            false
        }
        Err(e) => {
            warn!("icacls could not be started: {e}");
            false
        }
    }
}

fn os(s: &str) -> &std::ffi::OsStr {
    s.as_ref()
}

pub fn lock_down(dir: &Path) {
    let first_time = !dir.join(MARKER).exists();
    let d = dir.as_os_str();
    if first_time {
        // Objects a user created earlier belong to that user, who could change their
        // permissions back. Hand them to Administrators and make everything inherit.
        icacls(&[d, os("/setowner"), os(ADMINS), os("/T"), os("/C"), os("/Q")]);
        let all = dir.join("*");
        icacls(&[all.as_os_str(), os("/reset"), os("/T"), os("/C"), os("/Q")]);
    }
    let full = |who: &str| format!("{who}:(OI)(CI)F");
    let ok = icacls(&[
        d,
        os("/inheritance:r"),
        os("/grant:r"),
        os(&full(SYSTEM)),
        os(&full(ADMINS)),
        os(&format!("{USERS}:(OI)(CI)RX")),
        os("/C"),
        os("/Q"),
    ]);
    std::fs::create_dir_all(dir.join("files")).ok();
    for name in PRIVATE {
        let p = dir.join(name);
        if !p.exists() {
            continue;
        }
        let (sys, adm) = if p.is_dir() { (full(SYSTEM), full(ADMINS)) } else { (format!("{SYSTEM}:F"), format!("{ADMINS}:F")) };
        icacls(&[p.as_os_str(), os("/inheritance:r"), os("/grant:r"), os(&sys), os(&adm), os("/C"), os("/Q")]);
    }
    if ok {
        if first_time {
            let _ = std::fs::write(dir.join(MARKER), "Permissions of this folder are managed by Voxa Server.\r\n");
            info!("data folder permissions secured (users: read only; keys, database and files: administrators only)");
        }
    } else {
        warn!("could not secure the permissions of {}", dir.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn acl_of(p: &Path) -> String {
        let o = Command::new("icacls").arg(p).output().unwrap();
        String::from_utf8_lossy(&o.stdout).to_string()
    }

    /// Run with `--ignored`: changes permissions of a temporary folder inside ProgramData
    /// (which grants users write access, like the real data folder).
    #[test]
    #[ignore]
    fn lock_down_removes_user_write_access() {
        let base = Path::new(&std::env::var("ProgramData").unwrap()).join(format!("voxa-acl-test-{}", std::process::id()));
        std::fs::create_dir_all(base.join("logs")).unwrap();
        std::fs::create_dir_all(base.join("files").join("channel_1")).unwrap();
        std::fs::write(base.join("identity.key"), b"x").unwrap();
        std::fs::write(base.join("status.json"), b"{}").unwrap();
        let before = acl_of(&base);
        println!("before:\n{before}");
        assert!(before.contains("WD") || before.contains("AD"), "test folder does not start out user-writable");
        lock_down(&base);
        let root = acl_of(&base);
        let key = acl_of(&base.join("identity.key"));
        let files = acl_of(&base.join("files").join("channel_1"));
        let status = acl_of(&base.join("status.json"));
        println!("root:\n{root}\nkey:\n{key}\nfiles/channel_1:\n{files}\nstatus:\n{status}");
        // Restore access for cleanup (this user still owns the objects when not elevated).
        let me = std::env::var("USERNAME").unwrap();
        Command::new("icacls").arg(&base).args(["/grant", &format!("{me}:(OI)(CI)F"), "/T", "/C", "/Q"]).output().unwrap();
        Command::new("icacls").arg(base.join("identity.key")).args(["/grant", &format!("{me}:F")]).output().unwrap();
        Command::new("icacls").arg(base.join("files")).args(["/grant", &format!("{me}:(OI)(CI)F"), "/T"]).output().unwrap();
        std::fs::remove_dir_all(&base).unwrap();
        let users_lines = |s: &str| s.lines().filter(|l| l.contains("BUILTIN\\Users")).map(str::to_string).collect::<Vec<_>>();
        assert!(users_lines(&root).iter().all(|l| !l.contains("WD") && !l.contains("AD") && !l.contains("(F)") && !l.contains("(M)")), "users can still write: {root}");
        assert!(users_lines(&key).is_empty(), "users can read the key: {key}");
        assert!(users_lines(&files).is_empty(), "users can read uploaded files: {files}");
        assert!(!users_lines(&status).is_empty(), "users cannot read status.json: {status}");
    }
}
