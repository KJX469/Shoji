//! Windows side of the password manager: the key protector (DPAPI) and a clipboard that keeps
//! copied secrets out of Windows' clipboard history and clears them after a short while.

use crate::import_win::{dpapi_protect, dpapi_unprotect};
use shoji_core::vault::KeyProtector;
use windows::core::w;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, OpenClipboard,
    RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};

/// Seals the vault's data key to the signed-in Windows user.
pub struct WinProtector;

impl KeyProtector for WinProtector {
    fn protect(&self, data: &[u8]) -> Option<Vec<u8>> {
        dpapi_protect(data)
    }
    fn unprotect(&self, data: &[u8]) -> Option<Vec<u8>> {
        dpapi_unprotect(data)
    }
}

const CF_UNICODETEXT: u32 = 13;
/// How long a copied password stays on the clipboard.
const CLEAR_AFTER: std::time::Duration = std::time::Duration::from_secs(30);

unsafe fn put(format: u32, bytes: &[u8]) -> bool {
    let Ok(mem) = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1)) else {
        return false;
    };
    let dst = GlobalLock(mem);
    if dst.is_null() {
        let _ = GlobalFree(Some(mem));
        return false;
    }
    std::ptr::copy_nonoverlapping(bytes.as_ptr(), dst as *mut u8, bytes.len());
    let _ = GlobalUnlock(mem);
    if SetClipboardData(format, Some(HANDLE(mem.0))).is_err() {
        let _ = GlobalFree(Some(HGLOBAL(mem.0)));
        return false;
    }
    true
}

/// Put `text` on the clipboard, marked so clipboard history and cloud sync skip it, and clear it
/// after 30 seconds unless something else has been copied since.
pub fn copy_secret(text: &str) -> bool {
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let wide: Vec<u8> = text
            .encode_utf16()
            .chain(Some(0))
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let ok = put(CF_UNICODETEXT, &wide);
        // Windows' own marker for "don't record this": a DWORD of 0.
        let exclude = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
        if exclude != 0 {
            let _ = put(exclude, &0u32.to_le_bytes());
        }
        let _ = CloseClipboard();
        if ok {
            let seq = GetClipboardSequenceNumber();
            std::thread::spawn(move || {
                std::thread::sleep(CLEAR_AFTER);
                if GetClipboardSequenceNumber() == seq && OpenClipboard(None).is_ok() {
                    let _ = EmptyClipboard();
                    let _ = CloseClipboard();
                }
            });
        }
        ok
    }
}

/// Put plain text (a page address) on the clipboard.
pub fn copy_text(text: &str) -> bool {
    unsafe {
        if OpenClipboard(None).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        let wide: Vec<u8> = text
            .encode_utf16()
            .chain(Some(0))
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let ok = put(CF_UNICODETEXT, &wide);
        let _ = CloseClipboard();
        ok
    }
}

/// The text on the clipboard, if there is any.
pub fn paste_text() -> Option<String> {
    unsafe {
        OpenClipboard(None).ok()?;
        let text = (|| {
            let handle = GetClipboardData(CF_UNICODETEXT).ok()?;
            let mem = HGLOBAL(handle.0);
            let ptr = GlobalLock(mem) as *const u16;
            if ptr.is_null() {
                return None;
            }
            let mut len = 0;
            while *ptr.add(len) != 0 && len < 1_000_000 {
                len += 1;
            }
            let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
            let _ = GlobalUnlock(mem);
            Some(text)
        })();
        let _ = CloseClipboard();
        text
    }
}
