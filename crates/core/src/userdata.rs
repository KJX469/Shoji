//! Small JSON stores for things the user accumulates: favourites, remembered per-site choices,
//! and the open-tabs session used to recover after a crash. Each is a single file written
//! atomically (temp file + rename), so a crash mid-write never corrupts it.

use crate::tabs::TabGroup;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/* ------------------------------------------------------------ favourites */

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Favourite {
    pub url: String,
    pub title: String,
}

/// Cap so a runaway script can't grow the file without bound.
const MAX_FAVOURITES: usize = 500;

fn same_page(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

#[derive(Debug, Default)]
pub struct Favourites {
    path: PathBuf,
    items: Vec<Favourite>,
}

impl Favourites {
    pub fn load(path: PathBuf) -> Self {
        let items: Vec<Favourite> = read_json(&path);
        Self { path, items }
    }

    pub fn items(&self) -> &[Favourite] {
        &self.items
    }

    pub fn contains(&self, url: &str) -> bool {
        self.items.iter().any(|f| same_page(&f.url, url))
    }

    /// Only web pages can be favourites (not internal pages or script URLs).
    pub fn add(&mut self, url: &str, title: &str) -> bool {
        let ok = url.starts_with("https://") || url.starts_with("http://");
        if !ok || self.contains(url) || self.items.len() >= MAX_FAVOURITES {
            return false;
        }
        let title: String = title.trim().chars().take(120).collect();
        self.items.push(Favourite {
            url: url.to_string(),
            title: if title.is_empty() { url.to_string() } else { title },
        });
        self.save();
        true
    }

    pub fn remove(&mut self, url: &str) -> bool {
        let before = self.items.len();
        self.items.retain(|f| !same_page(&f.url, url));
        let changed = self.items.len() != before;
        if changed {
            self.save();
        }
        changed
    }

    /// Add if missing, remove if present. Returns true when it is now a favourite.
    pub fn toggle(&mut self, url: &str, title: &str) -> bool {
        if self.contains(url) {
            self.remove(url);
            false
        } else {
            self.add(url, title)
        }
    }

    fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(&self.items) {
            let _ = write_atomic(&self.path, &json);
        }
    }
}

/* ------------------------------------------------------------- site data */

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
struct SiteFile {
    /// host -> zoom factor
    zoom: BTreeMap<String, f64>,
    /// host -> permission kind -> "allow" | "block"
    permissions: BTreeMap<String, BTreeMap<String, String>>,
}

/// Remembered per-site zoom and permission decisions.
#[derive(Debug, Default)]
pub struct SiteData {
    path: PathBuf,
    data: SiteFile,
}

impl SiteData {
    pub fn load(path: PathBuf) -> Self {
        let data = read_json(&path);
        Self { path, data }
    }

    pub fn zoom_for(&self, host: &str) -> Option<f64> {
        self.data.zoom.get(host).copied()
    }

    /// `None` forgets the host's zoom (it falls back to the default zoom).
    pub fn set_zoom(&mut self, host: &str, zoom: Option<f64>) {
        if host.is_empty() {
            return;
        }
        match zoom {
            Some(z) => {
                self.data.zoom.insert(host.to_string(), z.clamp(0.25, 3.0));
            }
            None => {
                self.data.zoom.remove(host);
            }
        }
        self.save();
    }

    /// The remembered decision for `kind` on `host`: `Some(true)` allow, `Some(false)` block.
    pub fn permission(&self, host: &str, kind: &str) -> Option<bool> {
        match self.data.permissions.get(host)?.get(kind)?.as_str() {
            "allow" => Some(true),
            "block" => Some(false),
            _ => None,
        }
    }

    pub fn set_permission(&mut self, host: &str, kind: &str, allow: bool) {
        if host.is_empty() {
            return;
        }
        self.data
            .permissions
            .entry(host.to_string())
            .or_default()
            .insert(kind.to_string(), if allow { "allow" } else { "block" }.to_string());
        self.save();
    }

    /// Number of remembered choices (zoom levels and permissions), for Settings.
    pub fn count(&self) -> usize {
        self.data.zoom.len() + self.data.permissions.values().map(|p| p.len()).sum::<usize>()
    }

    pub fn clear(&mut self) {
        self.data = SiteFile::default();
        self.save();
    }

    fn save(&self) {
        if let Ok(json) = serde_json::to_string_pretty(&self.data) {
            let _ = write_atomic(&self.path, &json);
        }
    }
}

/* --------------------------------------------------------------- session */

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTab {
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub group: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionWindow {
    pub tabs: Vec<SessionTab>,
    /// Index into `tabs` of the tab that was showing
    pub active: usize,
    /// This window's tab groups (ids are per window)
    #[serde(default)]
    pub groups: Vec<TabGroup>,
}

/// The tabs open when Shoji last ran. `clean_exit` is false while it is running and only set
/// true when the user quits normally, so a crash or a force-close leaves it false.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub clean_exit: bool,
    pub windows: Vec<SessionWindow>,
}

impl Default for Session {
    fn default() -> Self {
        Self { clean_exit: true, windows: Vec::new() }
    }
}

impl Session {
    pub fn load(path: &Path) -> Self {
        read_json_or(path, Session::default)
    }

    pub fn save(&self, path: &Path) {
        if let Ok(json) = serde_json::to_string(self) {
            let _ = write_atomic(path, &json);
        }
    }

    pub fn tab_count(&self) -> usize {
        self.windows.iter().map(|w| w.tabs.len()).sum()
    }

    /// Whether there is anything worth restoring: at least one real page.
    pub fn has_pages(&self) -> bool {
        self.windows
            .iter()
            .flat_map(|w| &w.tabs)
            .any(|t| t.url.starts_with("http://") || t.url.starts_with("https://"))
    }
}

fn read_json_or<T: for<'de> Deserialize<'de>>(path: &Path, default: impl FnOnce() -> T) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(default)
}
