//! Is Shoji the default browser, and sending the user to Windows' own page to make it so.
//! Windows 10 and 11 do not let an app change the default silently, so this opens Settings →
//! Default apps on Shoji and the page checks again when the user returns.

use windows::core::{w, HSTRING};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::Shell::{
    ApplicationAssociationRegistration, IApplicationAssociationRegistration, ShellExecuteW,
    AL_EFFECTIVE, AT_URLPROTOCOL,
};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Whether `https` links currently open in Shoji.
pub fn is_default() -> bool {
    shoji_core::shell::init_com();
    unsafe {
        let Ok(reg) = CoCreateInstance::<_, IApplicationAssociationRegistration>(
            &ApplicationAssociationRegistration,
            None,
            CLSCTX_INPROC_SERVER,
        ) else {
            return false;
        };
        let Ok(prog_id) = reg.QueryCurrentDefault(w!("https"), AT_URLPROTOCOL, AL_EFFECTIVE) else {
            return false;
        };
        let current = prog_id.to_string().unwrap_or_default();
        CoTaskMemFree(Some(prog_id.0 as _));
        current == shoji_core::shell::URL_PROG_ID
    }
}

/// Open Windows' default apps page, scrolled to Shoji.
pub fn open_settings() {
    let uri = format!(
        "ms-settings:defaultapps?registeredAppUser={}",
        shoji_core::shell::REGISTERED_APP
    );
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(uri),
            None,
            None,
            SW_SHOWNORMAL,
        );
    }
}
