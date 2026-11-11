//! Single-instance support: a second launch signals the running instance (which shows
//! its window, e.g. from the tray) and exits.

#[cfg(windows)]
mod imp {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
    use windows::Win32::System::Threading::{CreateEventW, SetEvent, WaitForSingleObject, INFINITE};

    pub struct Instance(HANDLE);
    unsafe impl Send for Instance {}

    /// Returns `Some` for the first instance. For later instances the first one is
    /// signalled and `None` is returned.
    pub fn acquire(name: &str) -> Option<Instance> {
        let wide: Vec<u16> = format!("Local\\{name}").encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            let h = CreateEventW(None, false, false, PCWSTR(wide.as_ptr())).ok()?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                let _ = SetEvent(h);
                return None;
            }
            Some(Instance(h))
        }
    }

    /// Calls `on_signal` (on a background thread) whenever another instance starts.
    pub fn listen(inst: Instance, on_signal: impl Fn() + Send + 'static) {
        std::thread::spawn(move || loop {
            let inst = &inst;
            unsafe {
                WaitForSingleObject(inst.0, INFINITE);
            }
            on_signal();
        });
    }
}

#[cfg(windows)]
pub use imp::{acquire, listen, Instance};
