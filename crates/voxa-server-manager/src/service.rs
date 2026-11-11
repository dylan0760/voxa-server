//! Control of the `VoxaServer` Windows service.

use std::time::{Duration, Instant};

use windows_service::service::{ServiceAccess, ServiceState};
use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

pub const NAME: &str = "VoxaServer";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Svc {
    NotInstalled,
    Stopped,
    Starting,
    Running,
    Stopping,
    Unknown,
}

impl Svc {
    pub fn label(self) -> &'static str {
        match self {
            Svc::NotInstalled => "Not installed",
            Svc::Stopped => "Stopped",
            Svc::Starting => "Starting...",
            Svc::Running => "Running",
            Svc::Stopping => "Stopping...",
            Svc::Unknown => "Unknown",
        }
    }
}

fn describe(e: windows_service::Error) -> String {
    let code = match &e {
        windows_service::Error::Winapi(io) => io.raw_os_error(),
        _ => None,
    };
    match code {
        Some(5) => "Permission denied. Run the manager as administrator, or reinstall Voxa Server.".into(),
        Some(1060) => "The VoxaServer service is not installed.".into(),
        Some(1056) => "The service is already running.".into(),
        Some(1062) => "The service is not running.".into(),
        _ => format!("Service control failed: {e}"),
    }
}

fn open(access: ServiceAccess) -> Result<windows_service::service::Service, windows_service::Error> {
    let m = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    m.open_service(NAME, access)
}

pub fn query() -> Svc {
    match open(ServiceAccess::QUERY_STATUS) {
        Ok(s) => match s.query_status() {
            Ok(st) => match st.current_state {
                ServiceState::Running => Svc::Running,
                ServiceState::Stopped => Svc::Stopped,
                ServiceState::StartPending => Svc::Starting,
                ServiceState::StopPending => Svc::Stopping,
                _ => Svc::Unknown,
            },
            Err(_) => Svc::Unknown,
        },
        Err(windows_service::Error::Winapi(io)) if io.raw_os_error() == Some(1060) => Svc::NotInstalled,
        Err(_) => Svc::Unknown,
    }
}

pub fn start() -> Result<(), String> {
    let s = open(ServiceAccess::START | ServiceAccess::QUERY_STATUS).map_err(describe)?;
    s.start::<&str>(&[]).map_err(describe)
}

pub fn stop() -> Result<(), String> {
    let s = open(ServiceAccess::STOP | ServiceAccess::QUERY_STATUS).map_err(describe)?;
    s.stop().map(|_| ()).map_err(describe)
}

pub fn restart() -> Result<(), String> {
    if query() == Svc::Running {
        stop()?;
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    while query() != Svc::Stopped {
        if Instant::now() > deadline {
            return Err("Timed out waiting for the service to stop".into());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    start()
}
