use std::path::{Path, PathBuf};

/// Resolves the storage root for settings, notes and generated icons: `%APPDATA%\Shoji`,
/// or a `shoji-data` folder next to the executable when `%APPDATA%` is unavailable.
pub fn resolve_data_directory(exe_dir: &Path) -> PathBuf {
    match std::env::var_os("APPDATA") {
        Some(appdata) => PathBuf::from(appdata).join(crate::DATA_DIR_NAME),
        None => exe_dir.join("shoji-data"),
    }
}

/// Resolves the browser engine profile folder (cookies, logins, cache, history).
///
/// Like other browsers, it lives under `%LOCALAPPDATA%\Shoji\User Data`: the profile holds
/// machine-bound encrypted secrets and large caches, which should not roam. Settings and notes
/// stay in `data_dir`. A profile left in the old roaming location is moved there once.
pub fn resolve_profile_directory(data_dir: &Path) -> PathBuf {
    let legacy = data_dir.join(LEGACY_PROFILE_DIR);
    match std::env::var_os("LOCALAPPDATA") {
        Some(local) => migrate_profile(
            &legacy,
            PathBuf::from(local).join(crate::APP_NAME).join("User Data"),
        ),
        None => legacy,
    }
}

const LEGACY_PROFILE_DIR: &str = "webview2_data";

/// Move a profile from the old roaming location to `target` once. Returns the folder to use:
/// `target` normally, or `legacy` if the move was not possible (e.g. another Shoji is still
/// using it), so existing logins are never lost or split.
pub fn migrate_profile(legacy: &Path, target: PathBuf) -> PathBuf {
    if target.exists() || !legacy.is_dir() {
        return target;
    }
    if let Some(parent) = target.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return legacy.to_path_buf();
        }
    }
    match std::fs::rename(legacy, &target) {
        Ok(()) => target,
        Err(_) => legacy.to_path_buf(),
    }
}

/// Helper to parse a dotted version string (e.g. "153.0.4234.48") into numerical components for semver sorting.
pub fn parse_version_tuple(v: &str) -> Option<Vec<u64>> {
    let parts: Vec<u64> = v.split('.').filter_map(|p| p.parse::<u64>().ok()).collect();
    if parts.len() >= 2 {
        Some(parts)
    } else {
        None
    }
}

/// Inspect the host Windows system for WebView2 Runtime installation without requiring COM initialization.
/// Evaluates system and per-user paths, sorting multiple installed directories to reliably return the latest active version.
pub fn detect_webview2_runtime() -> Option<String> {
    let mut candidates: Vec<String> = Vec::new();

    let mut search_paths: Vec<PathBuf> = Vec::new();
    if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
        search_paths.push(PathBuf::from(pf86).join(r"Microsoft\EdgeWebView\Application"));
    }
    if let Ok(pf) = std::env::var("ProgramFiles") {
        search_paths.push(PathBuf::from(pf).join(r"Microsoft\EdgeWebView\Application"));
    }
    let sys_drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
    search_paths.push(PathBuf::from(format!(
        r"{}\Program Files (x86)\Microsoft\EdgeWebView\Application",
        sys_drive
    )));
    search_paths.push(PathBuf::from(format!(
        r"{}\Program Files\Microsoft\EdgeWebView\Application",
        sys_drive
    )));

    for path in &search_paths {
        if path.exists() {
            if let Ok(entries) = std::fs::read_dir(path) {
                for entry in entries.flatten() {
                    if let Ok(file_type) = entry.file_type() {
                        if file_type.is_dir() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if parse_version_tuple(&name).is_some() {
                                candidates.push(name);
                            }
                        }
                    }
                }
            }
        }
    }

    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        let user_path = PathBuf::from(local_appdata).join(r"Microsoft\EdgeWebView\Application");
        if user_path.exists() {
            if let Ok(entries) = std::fs::read_dir(user_path) {
                for entry in entries.flatten() {
                    if let Ok(file_type) = entry.file_type() {
                        if file_type.is_dir() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if parse_version_tuple(&name).is_some() {
                                candidates.push(name);
                            }
                        }
                    }
                }
            }
        }
    }

    candidates.sort_by(|a, b| {
        let pa = parse_version_tuple(a).unwrap_or_default();
        let pb = parse_version_tuple(b).unwrap_or_default();
        pa.cmp(&pb)
    });

    candidates.pop()
}

/// Verify process elevation status to enforce non-elevated running invariant.
#[cfg(windows)]
pub fn is_process_elevated() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{
        GetTokenInformation, TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // SAFETY: We query the current process token with TOKEN_QUERY.
    // The HANDLE is closed cleanly before returning.
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION::default();
        let mut return_length = 0u32;
        let success = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut return_length,
        );

        let _ = CloseHandle(token);

        if success.is_ok() {
            elevation.TokenIsElevated != 0
        } else {
            false
        }
    }
}

/// Fallback for non-Windows platforms (always false).
#[cfg(not(windows))]
pub fn is_process_elevated() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_version_tuple_sorting() {
        let v1 = "153.0.4234.32";
        let v2 = "153.0.4234.48";
        let v3 = "154.0.100.1";
        let v_invalid = "SetupMetrics";

        assert_eq!(parse_version_tuple(v_invalid), None);
        assert!(parse_version_tuple(v1).is_some());

        let mut list = vec![v1.to_string(), v3.to_string(), v2.to_string()];
        list.sort_by(|a, b| {
            let pa = parse_version_tuple(a).unwrap_or_default();
            let pb = parse_version_tuple(b).unwrap_or_default();
            pa.cmp(&pb)
        });

        assert_eq!(list, vec![v1.to_string(), v2.to_string(), v3.to_string()]);
        assert_eq!(list.pop(), Some("154.0.100.1".to_string()));
    }
}
