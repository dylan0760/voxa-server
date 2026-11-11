//! Long term secrets (server identity key, chat encryption master key).
//!
//! On Windows the key files are wrapped with DPAPI (machine scope), so copying the data
//! directory to another machine does not expose the chat history encryption key.

use std::path::Path;

use rand::RngCore;

#[cfg(windows)]
mod dpapi {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_LOCAL_MACHINE, CRYPTPROTECT_UI_FORBIDDEN,
        CRYPT_INTEGER_BLOB,
    };

    pub fn protect(data: &[u8]) -> Option<Vec<u8>> {
        unsafe {
            let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
            let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
            let ok = CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_LOCAL_MACHINE | CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            );
            if ok == 0 {
                return None;
            }
            let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(out.pbData as _);
            Some(v)
        }
    }

    pub fn unprotect(data: &[u8]) -> Option<Vec<u8>> {
        unsafe {
            let input = CRYPT_INTEGER_BLOB { cbData: data.len() as u32, pbData: data.as_ptr() as *mut u8 };
            let mut out = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };
            let ok = CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut out,
            );
            if ok == 0 {
                return None;
            }
            let v = std::slice::from_raw_parts(out.pbData, out.cbData as usize).to_vec();
            LocalFree(out.pbData as _);
            Some(v)
        }
    }
}

const MAGIC_DPAPI: &[u8] = b"VXD1";
const MAGIC_PLAIN: &[u8] = b"VXP1";

fn wrap(secret: &[u8]) -> Vec<u8> {
    #[cfg(windows)]
    if let Some(p) = dpapi::protect(secret) {
        return [MAGIC_DPAPI, &p].concat();
    }
    [MAGIC_PLAIN, secret].concat()
}

fn unwrap(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    if let Some(rest) = data.strip_prefix(MAGIC_PLAIN) {
        return Ok(rest.to_vec());
    }
    #[cfg(windows)]
    if let Some(rest) = data.strip_prefix(MAGIC_DPAPI) {
        return dpapi::unprotect(rest)
            .ok_or_else(|| anyhow::anyhow!("unable to decrypt key file (was it created on another machine?)"));
    }
    anyhow::bail!("unrecognised key file format")
}

/// Loads a secret from `path`, or creates it with `create` on first start.
pub fn load_or_create(path: &Path, create: impl FnOnce() -> Vec<u8>) -> anyhow::Result<Vec<u8>> {
    if path.exists() {
        unwrap(&std::fs::read(path)?)
    } else {
        let secret = create();
        std::fs::write(path, wrap(&secret))?;
        Ok(secret)
    }
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut b = [0u8; N];
    rand::thread_rng().fill_bytes(&mut b);
    b
}
