//! Windows service integration (service name `VoxaServer`).

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use tokio::sync::Notify;
use windows_service::service::{
    ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus, ServiceType,
};
use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
use windows_service::{define_windows_service, service_dispatcher};

pub const SERVICE_NAME: &str = "VoxaServer";
pub static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

define_windows_service!(ffi_service_main, service_main);

pub fn run() -> windows_service::Result<()> {
    service_dispatcher::start(SERVICE_NAME, ffi_service_main)
}

fn service_main(_args: Vec<OsString>) {
    let _ = run_service();
}

fn status(state: ServiceState, code: u32) -> ServiceStatus {
    ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code: ServiceExitCode::Win32(code),
        checkpoint: 0,
        wait_hint: Duration::from_secs(10),
        process_id: None,
    }
}

fn run_service() -> windows_service::Result<()> {
    let stop = Arc::new(Notify::new());
    let stop2 = stop.clone();
    let handle = service_control_handler::register(SERVICE_NAME, move |ev| match ev {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            stop2.notify_one();
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    handle.set_service_status(status(ServiceState::Running, 0))?;
    let dir = DATA_DIR.get().cloned().unwrap_or_else(crate::config::default_data_dir);
    let code = match tokio::runtime::Runtime::new() {
        Ok(rt) => match rt.block_on(crate::run(dir, false, async move { stop.notified().await })) {
            Ok(()) => 0,
            Err(e) => {
                tracing::error!("service failed: {e:#}");
                1
            }
        },
        Err(_) => 1,
    };
    handle.set_service_status(status(ServiceState::Stopped, code))?;
    Ok(())
}
