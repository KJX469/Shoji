//! Keeps the crane icon in step with the chosen colour outside the window itself:
//! writes a tinted `.ico` into the data folder and re-points Shoji's Start menu, desktop
//! and pinned-taskbar shortcuts at it. Runs on a background thread; takes milliseconds.

use shoji_brand::{ico, Accent, ICO_SIZES};
use std::path::{Path, PathBuf};

fn icon_path(data_dir: &Path, accent: Accent) -> PathBuf {
    data_dir
        .join("icons")
        .join(format!("shoji-{}.ico", accent.id()))
}

#[cfg(windows)]
pub fn retint_shortcuts(data_dir: PathBuf, accent: Accent) {
    std::thread::spawn(move || {
        let Ok(exe) = std::env::current_exe() else {
            return;
        };
        let shortcuts: Vec<PathBuf> = shoji_core::shell::all_shortcuts()
            .into_iter()
            .filter(|p| p.exists())
            .collect();
        if shortcuts.is_empty() {
            return; // no shortcuts to update
        }
        // White is the icon embedded in the executable itself.
        let icon = if accent == Accent::default() {
            exe.clone()
        } else {
            let path = icon_path(&data_dir, accent);
            // Written again whenever the icon itself changed (a new version draws it
            // differently), so shortcuts never keep an old one.
            if write_if_changed(&path, &ico(accent, &ICO_SIZES)).is_err() {
                return;
            }
            path
        };
        shoji_core::shell::init_com();
        let mut changed = false;
        for lnk in &shortcuts {
            changed |= shoji_core::shell::set_shortcut_icon(lnk, &exe, &icon).unwrap_or(false);
        }
        if changed {
            shoji_core::shell::notify_icons_changed();
        }
        // Keep the data folder lean: drop icons for colours no longer in use.
        if let Ok(entries) = std::fs::read_dir(data_dir.join("icons")) {
            for e in entries.flatten() {
                if e.path() != icon {
                    let _ = std::fs::remove_file(e.path());
                }
            }
        }
    });
}

#[cfg(not(windows))]
pub fn retint_shortcuts(_data_dir: PathBuf, _accent: Accent) {}

/// Write `bytes` to `path` unless it already holds exactly them. Returns whether it wrote.
fn write_if_changed(path: &Path, bytes: &[u8]) -> std::io::Result<bool> {
    if std::fs::read(path).is_ok_and(|old| old == bytes) {
        return Ok(false);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, bytes)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icon_left_by_an_older_version_is_replaced() {
        let dir = std::env::temp_dir().join(format!("shoji_icons_{}", std::process::id()));
        let path = icon_path(&dir, Accent::Gold);
        let now = ico(Accent::Gold, &[16, 32]);
        // An older version's drawing of the same colour.
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"old icon").unwrap();
        assert!(write_if_changed(&path, &now).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), now);
        // Unchanged: left alone.
        assert!(!write_if_changed(&path, &now).unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
