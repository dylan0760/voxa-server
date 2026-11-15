//! Backups in Voxa Server Manager. The server makes the backups itself (on its schedule or
//! "Back up now"); the manager shows `backup.json` / `backup-status.json` and changes the
//! settings or restores through `voxa-server.exe` run as administrator (a UAC prompt).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Mirrors the server's `backup.json`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    pub folder: String,
    pub weekly: bool,
    pub weekday: u8,
    pub hour: u8,
    pub keep: u32,
    pub share_user: String,
    pub password_set: bool,
    pub since: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { enabled: false, folder: String::new(), weekly: false, weekday: 6, hour: 3, keep: 7, share_user: String::new(), password_set: false, since: 0 }
    }
}

/// Mirrors the server's `backup-status.json`.
#[derive(Deserialize, Clone, Debug, Default)]
#[serde(default)]
pub struct Status {
    pub running: bool,
    pub last_attempt: u64,
    pub last_ok: u64,
    pub last_file: String,
    pub last_size: u64,
    pub last_error: String,
    pub next: u64,
}

pub fn read(dir: &Path) -> (Settings, Status) {
    let load = |name: &str| std::fs::read(dir.join(name)).ok();
    (
        load("backup.json").and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default(),
        load("backup-status.json").and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default(),
    )
}

pub const WEEKDAYS: [&str; 7] = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

pub fn describe_schedule(s: &Settings) -> String {
    let when = if s.weekly { format!("every {} at {:02}:00", WEEKDAYS[(s.weekday as usize).min(6)], s.hour) } else { format!("every day at {:02}:00", s.hour) };
    let keep = if s.keep == 0 { "keeping all".to_string() } else { format!("keeping the last {}", s.keep) };
    format!("{when}, {keep}")
}

pub fn fmt_time(t: u64) -> String {
    use chrono::TimeZone;
    chrono::Local.timestamp_opt(t as i64, 0).single().map(|t| t.format("%Y-%m-%d %H:%M").to_string()).unwrap_or_default()
}

pub fn fmt_size(b: u64) -> String {
    match b {
        b if b >= 1 << 30 => format!("{:.1} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b => format!("{} KB", b.div_ceil(1024)),
    }
}

fn server_exe() -> PathBuf {
    std::env::current_exe().map(|p| p.with_file_name("voxa-server.exe")).unwrap_or_else(|_| "voxa-server.exe".into())
}

/// Runs `voxa-server.exe --data <dir> <flag> <job file>` as administrator. The job (which
/// holds passwords) goes through a file in this user's temp folder, never the command
/// line; the server deletes it at once and answers in `<job>.result`.
pub fn run_job(data_dir: &Path, flag: &str, job: serde_json::Value) -> Result<String, String> {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let path = std::env::temp_dir().join(format!("voxa-job-{}-{nanos}.json", std::process::id()));
    let result_path = PathBuf::from(format!("{}.result", path.display()));
    std::fs::write(&path, job.to_string()).map_err(|e| format!("Could not prepare the request: {e}"))?;
    let args = format!("--data \"{}\" {flag} \"{}\"", data_dir.display(), path.display());
    let ran = run_elevated(&server_exe(), &args);
    let _ = std::fs::remove_file(&path);
    let answer = std::fs::read(&result_path).ok();
    let _ = std::fs::remove_file(&result_path);
    ran?;
    #[derive(Deserialize)]
    struct Answer {
        ok: bool,
        message: String,
    }
    match answer.and_then(|b| serde_json::from_slice::<Answer>(&b).ok()) {
        Some(a) if a.ok => Ok(a.message),
        Some(a) => Err(a.message),
        None => Err("Voxa Server did not answer (is it installed next to the manager?)".into()),
    }
}

/// Asks the running server for a backup now. Without the right to send it the service
/// control (servers installed before 1.18.0), it goes through an administrator prompt.
pub fn back_up_now(data_dir: &Path) -> Result<(), String> {
    match crate::service::backup_now() {
        Ok(()) => Ok(()),
        Err(_) => run_elevated(&server_exe(), &format!("--data \"{}\" --backup-now", data_dir.display())).map(|_| ()),
    }
}

#[cfg(windows)]
fn run_elevated(exe: &Path, args: &str) -> Result<u32, String> {
    use windows::core::{HSTRING, PCWSTR};
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let (verb, file, params) = (HSTRING::from("runas"), HSTRING::from(exe.as_os_str()), HSTRING::from(args));
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    if let Err(e) = unsafe { ShellExecuteExW(&mut info) } {
        // ERROR_CANCELLED: the administrator prompt was declined.
        return Err(if e.code().0 as u32 == 0x800704C7 { "Cancelled: administrator rights are needed for this.".into() } else { format!("Could not start Voxa Server: {}", e.message()) });
    }
    let mut code = 0u32;
    unsafe {
        WaitForSingleObject(info.hProcess, INFINITE);
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
    }
    Ok(code)
}

#[cfg(not(windows))]
fn run_elevated(_: &Path, _: &str) -> Result<u32, String> {
    Err("Only on Windows".into())
}
