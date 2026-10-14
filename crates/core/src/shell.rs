//! Windows shell integration shared by the browser and its installer: shortcut locations,
//! creating shortcuts, and re-pointing their icons when the user picks a new crane colour.
//!
//! Everything here talks to the shell through COM directly (`IShellLinkW`), instead of
//! spawning PowerShell, so it costs milliseconds rather than a process launch.

use std::path::{Path, PathBuf};
use windows::core::{Interface, HSTRING};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, IPersistFile, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED, STGM_READWRITE,
};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, IShellLinkW, SHChangeNotify, SHGetKnownFolderPath, ShellLink,
    KF_FLAG_DEFAULT, SHCNE_ASSOCCHANGED, SHCNF_IDLIST,
};

fn shortcut_name() -> String {
    format!("{}.lnk", crate::APP_NAME)
}

/// `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Shoji.lnk`
pub fn start_menu_shortcut() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(appdata)
            .join(r"Microsoft\Windows\Start Menu\Programs")
            .join(shortcut_name()),
    )
}

/// The user's Desktop (resolved through the shell, so OneDrive-redirected desktops work).
pub fn desktop_shortcut() -> Option<PathBuf> {
    unsafe {
        let pwstr = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None).ok()?;
        let path = pwstr.to_string().ok();
        CoTaskMemFree(Some(pwstr.0 as _));
        Some(PathBuf::from(path?).join(shortcut_name()))
    }
}

/// The shortcut Windows creates when the app is pinned to the taskbar.
pub fn pinned_taskbar_shortcut() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(
        PathBuf::from(appdata)
            .join(r"Microsoft\Internet Explorer\Quick Launch\User Pinned\TaskBar")
            .join(shortcut_name()),
    )
}

/// Ensure COM is initialized on the calling thread (harmless if it already is).
pub fn init_com() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
}

/// Create (or overwrite) a shortcut to `target`, optionally with a custom icon file.
pub fn write_shortcut(lnk: &Path, target: &Path, icon: Option<&Path>) -> windows::core::Result<()> {
    if let Some(parent) = lnk.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(target.as_os_str()))?;
        if let Some(dir) = target.parent() {
            link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        }
        link.SetDescription(&HSTRING::from(crate::APP_NAME))?;
        link.SetIconLocation(&HSTRING::from(icon.unwrap_or(target).as_os_str()), 0)?;
        link.cast::<IPersistFile>()?
            .Save(&HSTRING::from(lnk.as_os_str()), true)
    }
}

/// Point an existing shortcut at a new icon file, but only if it launches `target`.
/// Returns `Ok(false)` when the shortcut is missing or belongs to another copy of the app.
pub fn set_shortcut_icon(lnk: &Path, target: &Path, icon: &Path) -> windows::core::Result<bool> {
    if !lnk.exists() {
        return Ok(false);
    }
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let file = link.cast::<IPersistFile>()?;
        file.Load(&HSTRING::from(lnk.as_os_str()), STGM_READWRITE)?;
        let mut buf = [0u16; 1024];
        link.GetPath(&mut buf, std::ptr::null_mut(), 0)?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let current = String::from_utf16_lossy(&buf[..len]);
        if !current.eq_ignore_ascii_case(&target.to_string_lossy()) {
            return Ok(false);
        }
        link.SetIconLocation(&HSTRING::from(icon.as_os_str()), 0)?;
        file.Save(&HSTRING::from(lnk.as_os_str()), true)?;
        Ok(true)
    }
}

/// Every shortcut location Shoji may own.
pub fn all_shortcuts() -> Vec<PathBuf> {
    [
        start_menu_shortcut(),
        desktop_shortcut(),
        pinned_taskbar_shortcut(),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// Ask Explorer to refresh cached icons.
pub fn notify_icons_changed() {
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

/* ------------------------------------------------- registration as a web browser */

/// ProgID Windows records when Shoji is the default for http and https.
pub const URL_PROG_ID: &str = "ShojiURL";
/// The name under which Shoji appears in Windows' default apps list.
pub const REGISTERED_APP: &str = "Shoji";
const CLIENT_KEY: &str = r"Software\Clients\StartMenuInternet\Shoji";
const REGISTERED_APPS_KEY: &str = r"Software\RegisteredApplications";

fn wide_bytes(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain(Some(0))
        .flat_map(|u| u.to_le_bytes())
        .collect()
}

/// Set `name` (empty for the key's default value) under `HKCU\key`, creating the key.
fn set_value(key: &str, name: &str, value: &str) -> windows::core::Result<()> {
    unsafe {
        let mut hkey = HKEY::default();
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            None,
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_WRITE,
            None,
            &mut hkey,
            None,
        )
        .ok()?;
        let result = RegSetValueExW(
            hkey,
            &HSTRING::from(name),
            None,
            REG_SZ,
            Some(&wide_bytes(value)),
        )
        .ok();
        let _ = RegCloseKey(hkey);
        result
    }
}

/// Whether `exe` is the installed copy (`%LOCALAPPDATA%\Programs\Shoji`). Only that copy
/// registers itself, so a build run from a source folder never becomes the default.
pub fn is_installed_copy(exe: &Path) -> bool {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else {
        return false;
    };
    let dir = PathBuf::from(local).join("Programs").join(crate::APP_NAME);
    exe.parent().is_some_and(|p| {
        p.to_string_lossy()
            .eq_ignore_ascii_case(&dir.to_string_lossy())
    })
}

/// Make Shoji appear in Windows' list of web browsers (Settings → Default apps), for the current
/// user only. Safe to run again: it rewrites the same values. Windows decides what is the default;
/// this only makes Shoji a choice.
pub fn register_as_browser(exe: &Path) -> windows::core::Result<()> {
    let exe = exe.to_string_lossy();
    let open = format!("\"{exe}\" \"%1\"");
    let icon = format!("{exe},0");
    let caps = format!(r"{CLIENT_KEY}\Capabilities");

    let class = format!(r"Software\Classes\{URL_PROG_ID}");
    set_value(&class, "", "Shoji URL")?;
    set_value(&class, "URL Protocol", "")?;
    set_value(&format!(r"{class}\DefaultIcon"), "", &icon)?;
    set_value(&format!(r"{class}\shell\open\command"), "", &open)?;

    set_value(CLIENT_KEY, "", crate::APP_NAME)?;
    set_value(&format!(r"{CLIENT_KEY}\DefaultIcon"), "", &icon)?;
    set_value(
        &format!(r"{CLIENT_KEY}\shell\open\command"),
        "",
        &format!("\"{exe}\""),
    )?;
    set_value(&caps, "ApplicationName", crate::APP_NAME)?;
    set_value(
        &caps,
        "ApplicationDescription",
        "A paper-thin, privacy-first browser",
    )?;
    set_value(&caps, "ApplicationIcon", &icon)?;
    for scheme in ["http", "https"] {
        set_value(&format!(r"{caps}\URLAssociations"), scheme, URL_PROG_ID)?;
    }
    set_value(REGISTERED_APPS_KEY, REGISTERED_APP, &caps)
}

/// Undo [`register_as_browser`] (uninstall).
pub fn unregister_browser() {
    use windows::Win32::System::Registry::RegDeleteKeyValueW;
    unsafe {
        for key in [
            format!(r"Software\Classes\{URL_PROG_ID}"),
            CLIENT_KEY.to_string(),
        ] {
            let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key));
        }
        let _ = RegDeleteKeyValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(REGISTERED_APPS_KEY),
            &HSTRING::from(REGISTERED_APP),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icon_location(lnk: &Path) -> String {
        unsafe {
            let link: IShellLinkW =
                CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            link.cast::<IPersistFile>()
                .unwrap()
                .Load(&HSTRING::from(lnk.as_os_str()), STGM_READWRITE)
                .unwrap();
            let mut buf = [0u16; 512];
            let mut index = 0;
            link.GetIconLocation(&mut buf, &mut index).unwrap();
            String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)])
        }
    }

    #[test]
    fn shortcuts_are_created_and_retinted_only_for_our_target() {
        init_com();
        let dir = std::env::temp_dir().join(format!("shoji_shell_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let target = std::env::current_exe().unwrap();
        let lnk = dir.join("Shoji.lnk");
        let icon = dir.join("shoji-gold.ico");
        std::fs::write(&icon, b"x").unwrap();

        write_shortcut(&lnk, &target, None).unwrap();
        assert!(lnk.exists());
        assert!(set_shortcut_icon(&lnk, &target, &icon).unwrap());
        assert!(icon_location(&lnk).eq_ignore_ascii_case(&icon.to_string_lossy()));

        // A shortcut that launches some other program is left alone.
        let other = dir.join("Other.lnk");
        write_shortcut(&other, Path::new(r"C:\Windows\notepad.exe"), None).unwrap();
        assert!(!set_shortcut_icon(&other, &target, &icon).unwrap());
        // Missing shortcuts are skipped.
        assert!(!set_shortcut_icon(&dir.join("none.lnk"), &target, &icon).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
