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

/// `%LOCALAPPDATA%\Shoji`: the engine profile (logins, cache, history) and anything else that
/// stays on this machine.
pub fn local_app_directory() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|l| PathBuf::from(l).join(crate::APP_NAME))
}

/// `%LOCALAPPDATA%\Programs\Shoji`: where the installer puts the browser and its uninstaller.
pub fn install_directory() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(|l| PathBuf::from(l).join("Programs").join(crate::APP_NAME))
}

/// Every folder any version of Shoji keeps anything in: the program, settings and notes
/// (`%APPDATA%\shoji`, including downloaded updates and Byobu's lists) and the engine profile
/// (`%LOCALAPPDATA%\Shoji`). Uninstalling deletes these whole, so whatever a later version
/// stores must live inside one of them; the tests hold every path the app uses to that.
pub fn footprint_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = install_directory().into_iter().collect();
    if let Some(appdata) = std::env::var_os("APPDATA") {
        dirs.push(PathBuf::from(appdata).join(crate::DATA_DIR_NAME));
    }
    dirs.extend(local_app_directory());
    dirs
}

/// Prefixes of the scratch files and folders Shoji may leave in `%TEMP%` (an import copy whose
/// clean-up was cut short, the setup window's engine folder, a temporary uninstaller).
pub const TEMP_PREFIXES: &[&str] = &["shoji-import-", "shoji-setup-ui-", "shoji-uninstall-"];

/// Shoji's leftovers in the temp folder (see [`TEMP_PREFIXES`]).
pub fn temp_leftovers() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            TEMP_PREFIXES.iter().any(|p| name.starts_with(p))
        })
        .map(|e| e.path())
        .collect()
}

/// Whether `path` lies inside `dir` (compared without regard to case, as Windows does).
pub fn is_within(path: &Path, dir: &Path) -> bool {
    let norm = |p: &Path| {
        p.to_string_lossy()
            .replace('/', "\\")
            .trim_end_matches('\\')
            .to_ascii_lowercase()
    };
    let (p, d) = (norm(path), norm(dir));
    !d.is_empty() && (p == d || p.starts_with(&format!("{d}\\")))
}

/// A batch script that deletes `dirs` recursively and `files`, trying again every second for
/// up to a minute while something (the process that started it, or the engine's helper
/// processes) still holds them, then deletes itself. Returns `None` when there is nothing safe
/// to delete. Paths that cmd could misread (`"` or `%`) or that have no parent (drive roots)
/// are skipped rather than risk deleting the wrong thing.
pub fn purge_script(dirs: &[PathBuf], files: &[PathBuf]) -> Option<String> {
    fn usable(p: &Path) -> bool {
        let s = p.to_string_lossy();
        p.parent().is_some()
            && p.file_name().is_some()
            && !s.contains('"')
            && !s.contains('%')
            && !s.contains(['\r', '\n'])
            && !s.ends_with(['\\', '/'])
    }
    let shown = |list: &[PathBuf]| -> Vec<String> {
        list.iter()
            .filter(|p| usable(p))
            .map(|p| p.display().to_string())
            .collect()
    };
    let (dirs, files) = (shown(dirs), shown(files));
    if dirs.is_empty() && files.is_empty() {
        return None;
    }
    let mut lines: Vec<String> = vec!["@echo off".into(), "set tries=0".into(), ":again".into()];
    lines.extend(dirs.iter().map(|d| format!("rmdir /s /q \"{d}\" 2>nul")));
    lines.extend(files.iter().map(|f| format!("del /f /q \"{f}\" 2>nul")));
    lines.push("set /a tries+=1".into());
    lines.extend(
        dirs.iter()
            .chain(&files)
            .map(|p| format!("if exist \"{p}\" goto wait")),
    );
    lines.push("goto done".into());
    lines.push(":wait".into());
    lines.push("if %tries% geq 60 goto done".into());
    lines.push("ping -n 2 127.0.0.1 >nul".into());
    lines.push("goto again".into());
    lines.push(":done".into());
    // Delete this script too (the `goto` makes cmd let go of it first).
    lines.push("(goto) 2>nul & del /f /q \"%~f0\"".into());
    Some(lines.join("\r\n") + "\r\n")
}

/// Start [`purge_script`] in the background, detached, so it outlives the caller. Returns
/// false when there was nothing to do or it could not be started.
#[cfg(windows)]
pub fn start_purge(dirs: &[PathBuf], files: &[PathBuf]) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Some(script) = purge_script(dirs, files) else {
        return false;
    };
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    let temp = std::env::temp_dir();
    let path = temp.join(format!("shoji-cleanup-{}-{stamp}.cmd", std::process::id()));
    if std::fs::write(&path, script).is_err() {
        return false;
    }
    std::process::Command::new("cmd")
        .arg("/c")
        .arg(&path)
        // Run from outside every folder it deletes, so its working directory holds none.
        .current_dir(&temp)
        .env_remove("WEBVIEW2_USER_DATA_FOLDER")
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .is_ok()
}

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
