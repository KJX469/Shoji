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
            if !path.exists() {
                let _ = std::fs::create_dir_all(path.parent().unwrap_or(&data_dir));
                if std::fs::write(&path, ico(accent, &ICO_SIZES)).is_err() {
                    return;
                }
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
