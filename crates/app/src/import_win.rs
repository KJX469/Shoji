//! The Windows half of importing: unlocking values another browser protected for this Windows
//! user, finding the browsers on this PC, and running a read on a worker thread.
//!
//! Only values the signed-in user can legitimately unlock are read: Chromium's per-user key
//! (protected with DPAPI) and the AES-GCM "v10" values it encrypts with that key. Newer Chrome,
//! Edge and Brave builds also seal some values to the browser itself ("v20", app-bound); those are
//! left alone and reported as not movable.

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use shoji_core::import::{catalog, Decryptor, Imported, Kinds};
use std::path::{Path, PathBuf};
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPT_INTEGER_BLOB,
};

/// Reverse DPAPI protection made for the current Windows user.
pub(crate) fn dpapi_unprotect(data: &[u8]) -> Option<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&input, None, None, None, None, 0, &mut output).ok()?;
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData as _)));
        Some(bytes)
    }
}

/// Protect `data` for the current Windows user, with no prompts.
pub(crate) fn dpapi_protect(data: &[u8]) -> Option<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        // CRYPTPROTECT_UI_FORBIDDEN: never show a dialog.
        CryptProtectData(&input, None, None, None, None, 0x1, &mut output).ok()?;
        let bytes = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        let _ = LocalFree(Some(HLOCAL(output.pbData as _)));
        Some(bytes)
    }
}

#[derive(Default)]
pub struct WinDecryptor {
    key: Option<LessSafeKey>,
}

impl Decryptor for WinDecryptor {
    fn load_key(&mut self, local_state: &Path) -> bool {
        self.key = None;
        let Ok(text) = std::fs::read_to_string(local_state) else {
            return false;
        };
        let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) else {
            return false;
        };
        let Some(b64) = json["os_crypt"]["encrypted_key"].as_str() else {
            return false;
        };
        use base64::Engine;
        let Ok(raw) = base64::engine::general_purpose::STANDARD.decode(b64) else {
            return false;
        };
        let Some(wrapped) = raw.strip_prefix(b"DPAPI") else {
            return false;
        };
        let Some(key) = dpapi_unprotect(wrapped) else {
            return false;
        };
        self.key = UnboundKey::new(&AES_256_GCM, &key)
            .ok()
            .map(LessSafeKey::new);
        self.key.is_some()
    }

    fn decrypt(&self, blob: &[u8]) -> Option<Vec<u8>> {
        if let Some(rest) = blob
            .strip_prefix(b"v10")
            .or_else(|| blob.strip_prefix(b"v11"))
        {
            // nonce (12) + ciphertext + tag (16)
            if rest.len() < 12 + 16 {
                return None;
            }
            let (nonce, sealed) = rest.split_at(12);
            let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
            let mut buf = sealed.to_vec();
            let plain = self
                .key
                .as_ref()?
                .open_in_place(nonce, Aad::empty(), &mut buf)
                .ok()?;
            return Some(plain.to_vec());
        }
        if blob.len() > 3 && blob[0] == b'v' && blob[1].is_ascii_digit() && blob[2].is_ascii_digit()
        {
            // v20 and any later sealed format: not readable without the browser itself.
            return None;
        }
        // Very old Chromium: the value itself is DPAPI-protected.
        dpapi_unprotect(blob)
    }
}

fn env_dir(name: &str) -> PathBuf {
    std::env::var_os(name)
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// Every browser Shoji knows, with what was found on this PC.
pub fn detect() -> Vec<catalog::Detected> {
    catalog::detect(
        &env_dir("LOCALAPPDATA"),
        &env_dir("APPDATA"),
        &env_dir("USERPROFILE"),
    )
}

/// Where to start the folder picker for `source`.
pub fn picker_start(source: &catalog::Source) -> PathBuf {
    let base = match source.family {
        catalog::Family::Firefox => env_dir("APPDATA"),
        _ => env_dir("LOCALAPPDATA"),
    };
    if base.as_os_str().is_empty() {
        env_dir("USERPROFILE")
    } else {
        base
    }
}

/// Where the profiles for this request are: the picked path, else the installed location.
pub fn profiles_for(source: &catalog::Source, path: Option<&str>) -> Vec<catalog::Profile> {
    match path {
        Some(p) if !p.is_empty() => catalog::profiles_at(source, Path::new(p)),
        _ => catalog::data_dir(
            source,
            &env_dir("LOCALAPPDATA"),
            &env_dir("APPDATA"),
            &env_dir("USERPROFILE"),
        )
        .map(|d| catalog::profiles(source, &d))
        .unwrap_or_default(),
    }
}

/// Read one profile. Blocking: run it on a worker thread.
pub fn read(
    source: &'static catalog::Source,
    profile: &catalog::Profile,
    kinds: Kinds,
    progress: &mut dyn FnMut(&str),
) -> Imported {
    shoji_core::import::read_profile(
        source,
        profile,
        kinds,
        &mut WinDecryptor::default(),
        progress,
    )
}
