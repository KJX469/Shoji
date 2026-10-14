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
const MAX_FAVOURITES: usize = 5000;

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
            title: if title.is_empty() {
                url.to_string()
            } else {
                title
            },
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

    /// Add many at once (an import), saving once. Returns how many were new.
    pub fn add_many(&mut self, list: &[Favourite]) -> usize {
        let before = self.items.len();
        for f in list {
            let ok = f.url.starts_with("https://") || f.url.starts_with("http://");
            if !ok || self.contains(&f.url) || self.items.len() >= MAX_FAVOURITES {
                continue;
            }
            let title: String = f.title.trim().chars().take(120).collect();
            self.items.push(Favourite {
                url: f.url.clone(),
                title: if title.is_empty() {
                    f.url.clone()
                } else {
                    title
                },
            });
        }
        let added = self.items.len() - before;
        if added > 0 {
            self.save();
        }
        added
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

/* --------------------------------------------------------------- history */

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub url: String,
    pub title: String,
    pub visits: u32,
    /// Unix milliseconds
    pub last_visit: u64,
}

/// Cap so the file (read at startup) stays small.
pub const MAX_HISTORY: usize = 50_000;

/// Pages the user visited, newest first once saved. Feeds address-bar suggestions and the
/// history list. Visits are batched in memory and written by [`History::flush`].
#[derive(Debug, Default)]
pub struct History {
    path: PathBuf,
    items: Vec<HistoryEntry>,
    dirty: bool,
}

impl History {
    pub fn load(path: PathBuf) -> Self {
        let items: Vec<HistoryEntry> = read_json(&path);
        Self {
            path,
            items,
            dirty: false,
        }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn items(&self) -> &[HistoryEntry] {
        &self.items
    }

    /// Note a visit. Only web pages count.
    pub fn record(&mut self, url: &str, title: &str, now_ms: u64) {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return;
        }
        let title: String = title.trim().chars().take(120).collect();
        if let Some(e) = self.items.iter_mut().find(|e| e.url == url) {
            e.visits = e.visits.saturating_add(1);
            e.last_visit = now_ms;
            if !title.is_empty() {
                e.title = title;
            }
        } else {
            self.items.push(HistoryEntry {
                url: url.to_string(),
                title,
                visits: 1,
                last_visit: now_ms,
            });
        }
        self.dirty = true;
    }

    /// Fill in the title of a page just visited, without counting another visit.
    pub fn set_title(&mut self, url: &str, title: &str) {
        let title: String = title.trim().chars().take(120).collect();
        if title.is_empty() {
            return;
        }
        if let Some(e) = self.items.iter_mut().rev().take(50).find(|e| e.url == url) {
            if e.title != title {
                e.title = title;
                self.dirty = true;
            }
        }
    }

    /// Merge an import: known pages keep the larger visit count and later time. Returns how many
    /// pages were new.
    pub fn merge(&mut self, list: Vec<HistoryEntry>) -> usize {
        let mut known: std::collections::HashMap<String, usize> = self
            .items
            .iter()
            .enumerate()
            .map(|(i, e)| (e.url.clone(), i))
            .collect();
        let mut added = 0;
        for h in list {
            match known.get(&h.url) {
                Some(&i) => {
                    let e = &mut self.items[i];
                    e.visits = e.visits.max(h.visits);
                    e.last_visit = e.last_visit.max(h.last_visit);
                    if e.title.is_empty() {
                        e.title = h.title;
                    }
                }
                None => {
                    known.insert(h.url.clone(), self.items.len());
                    self.items.push(h);
                    added += 1;
                }
            }
        }
        self.dirty = true;
        self.flush();
        added
    }

    /// Entries matching every word of `query` in title or address: most recent first.
    pub fn search(&self, query: &str, limit: usize) -> Vec<&HistoryEntry> {
        let words: Vec<String> = query
            .to_lowercase()
            .split_whitespace()
            .map(String::from)
            .collect();
        let mut hits: Vec<&HistoryEntry> = self
            .items
            .iter()
            .filter(|e| {
                let hay = format!("{} {}", e.title, e.url).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .collect();
        hits.sort_by(|a, b| {
            (b.visits.min(20), b.last_visit).cmp(&(a.visits.min(20), a.last_visit))
        });
        hits.truncate(limit);
        hits
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.dirty = true;
        self.flush();
    }

    /// Write to disk if anything changed, keeping the newest [`MAX_HISTORY`].
    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        if self.items.len() > MAX_HISTORY {
            self.items.sort_by(|a, b| b.last_visit.cmp(&a.last_visit));
            self.items.truncate(MAX_HISTORY);
        }
        if let Ok(json) = serde_json::to_string(&self.items) {
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
            .insert(
                kind.to_string(),
                if allow { "allow" } else { "block" }.to_string(),
            );
        self.save();
    }

    /// Number of remembered choices (zoom levels and permissions), for Settings.
    pub fn count(&self) -> usize {
        self.data.zoom.len()
            + self
                .data
                .permissions
                .values()
                .map(|p| p.len())
                .sum::<usize>()
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

const fn default_session_split_ratio() -> f64 {
    crate::tabs::DEFAULT_SPLIT_RATIO
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSplit {
    /// Indices into this workspace's tabs of the two split panes.
    pub panes: [usize; 2],
    #[serde(default)]
    pub orientation: crate::tabs::SplitOrientation,
    #[serde(default = "default_session_split_ratio")]
    pub ratio: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionWorkspace {
    pub id: u32,
    pub name: String,
    pub color: String,
    /// Index into this workspace's tabs of the tab that was active in this workspace.
    #[serde(default)]
    pub active: usize,
    #[serde(default)]
    pub split: Option<SessionSplit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTab {
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub group: Option<u32>,
    #[serde(default = "crate::tabs::default_workspace_id")]
    pub workspace: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionWindow {
    pub tabs: Vec<SessionTab>,
    /// Index into `tabs` of the tab that was showing
    pub active: usize,
    /// This window's tab groups (ids are per window)
    #[serde(default)]
    pub groups: Vec<TabGroup>,
    /// This window's workspaces in user order (empty in pre-workspace session files)
    #[serde(default)]
    pub workspaces: Vec<SessionWorkspace>,
    /// Active workspace id in this window (0 in pre-workspace session files)
    #[serde(default)]
    pub active_workspace: u32,
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
        Self {
            clean_exit: true,
            windows: Vec::new(),
        }
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

    /// Build a session from window `TabManager`s if persistence is allowed by `privacy`.
    /// Returns `None` in private mode (`ephemeral_default`) or when `restore_session == "never"`.
    pub fn from_managers(
        managers: &[&crate::tabs::TabManager],
        clean_exit: bool,
        privacy: &crate::settings::PrivacySettings,
    ) -> Option<Self> {
        if privacy.ephemeral_default || privacy.restore_session == "never" {
            return None;
        }
        let windows = managers
            .iter()
            .filter_map(|m| m.to_session_window())
            .collect();
        Some(Self {
            clean_exit,
            windows,
        })
    }

    /// Save session to `path` only when allowed by `privacy`. In private mode, removes any existing
    /// session file at `path` and returns `false`.
    pub fn save_respecting_privacy(
        &self,
        path: &Path,
        privacy: &crate::settings::PrivacySettings,
    ) -> bool {
        if privacy.ephemeral_default || privacy.restore_session == "never" {
            let _ = std::fs::remove_file(path);
            return false;
        }
        self.save(path);
        true
    }

    /// Load session from `path` respecting `privacy`. In private mode, removes any stale session
    /// file and returns an empty default session.
    pub fn load_respecting_privacy(
        path: &Path,
        privacy: &crate::settings::PrivacySettings,
    ) -> Self {
        if privacy.ephemeral_default || privacy.restore_session == "never" {
            let _ = std::fs::remove_file(path);
            return Self::default();
        }
        Self::load(path)
    }

    pub fn tab_count(&self) -> usize {
        self.windows.iter().map(|w| w.tabs.len()).sum()
    }

    /// Whether there is anything worth restoring: at least one real page or custom workspace.
    pub fn has_pages(&self) -> bool {
        self.windows.iter().any(|w| {
            w.tabs
                .iter()
                .any(|t| t.url.starts_with("http://") || t.url.starts_with("https://"))
                || w.workspaces.len() > 1
                || w.workspaces
                    .first()
                    .is_some_and(|ws| ws.name != "Main" || ws.color != "blue")
        })
    }
}

fn read_json_or<T: for<'de> Deserialize<'de>>(path: &Path, default: impl FnOnce() -> T) -> T {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(default)
}

#[cfg(test)]
mod history_tests {
    use super::*;

    fn path(n: &str) -> PathBuf {
        std::env::temp_dir().join(format!("shoji_hist_{}_{n}.json", std::process::id()))
    }

    #[test]
    fn records_merges_and_searches() {
        let p = path("a");
        let _ = std::fs::remove_file(&p);
        let mut h = History::load(p.clone());
        h.record("https://rust-lang.org/", "Rust", 10);
        h.record("https://rust-lang.org/", "", 20);
        h.record("shoji://newtab", "New tab", 30);
        assert_eq!(h.len(), 1);
        assert_eq!((h.items()[0].visits, h.items()[0].last_visit), (2, 20));
        let added = h.merge(vec![
            HistoryEntry {
                url: "https://rust-lang.org/".into(),
                title: "x".into(),
                visits: 9,
                last_visit: 5,
            },
            HistoryEntry {
                url: "https://example.com/".into(),
                title: "Example".into(),
                visits: 1,
                last_visit: 50,
            },
        ]);
        assert_eq!(added, 1);
        assert_eq!(h.items()[0].visits, 9);
        assert_eq!(h.items()[0].last_visit, 20);
        assert_eq!(h.search("rust lang", 5).len(), 1);
        assert!(h.search("nothing", 5).is_empty());
        // merge() saved it
        assert_eq!(History::load(p.clone()).len(), 2);
        h.clear();
        assert!(History::load(p.clone()).is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn keeps_only_the_newest_when_over_the_cap() {
        let p = path("b");
        let mut h = History::load(p.clone());
        let list: Vec<_> = (0..MAX_HISTORY as u64 + 10)
            .map(|i| HistoryEntry {
                url: format!("https://x.test/{i}"),
                title: String::new(),
                visits: 1,
                last_visit: i,
            })
            .collect();
        h.merge(list);
        assert_eq!(h.len(), MAX_HISTORY);
        assert!(h.items().iter().all(|e| e.last_visit >= 10));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn favourites_add_many_skips_duplicates_and_junk() {
        let p = path("fav");
        let _ = std::fs::remove_file(&p);
        let mut f = Favourites::load(p.clone());
        let n = f.add_many(&[
            Favourite {
                url: "https://a.com".into(),
                title: "A".into(),
            },
            Favourite {
                url: "https://a.com/".into(),
                title: "dup".into(),
            },
            Favourite {
                url: "ftp://x".into(),
                title: "no".into(),
            },
            Favourite {
                url: "http://b.com".into(),
                title: "".into(),
            },
        ]);
        assert_eq!(n, 2);
        assert_eq!(f.items()[1].title, "http://b.com");
        let _ = std::fs::remove_file(&p);
    }
}
