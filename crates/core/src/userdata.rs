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

    /// One page of the history list, most recently visited first, filtered by `query` (every
    /// word must appear in the title or address). Returns the page and how many entries match.
    pub fn page(&self, query: &str, offset: usize, limit: usize) -> (Vec<&HistoryEntry>, usize) {
        let words: Vec<String> = query
            .to_lowercase()
            .split_whitespace()
            .map(String::from)
            .collect();
        let mut hits: Vec<&HistoryEntry> = self
            .items
            .iter()
            .filter(|e| {
                words.is_empty() || {
                    let hay = format!("{} {}", e.title, e.url).to_lowercase();
                    words.iter().all(|w| hay.contains(w))
                }
            })
            .collect();
        hits.sort_by_key(|e| std::cmp::Reverse(e.last_visit));
        let total = hits.len();
        let page = hits.into_iter().skip(offset).take(limit).collect();
        (page, total)
    }

    /// Forget these pages. Returns how many were removed.
    pub fn remove(&mut self, urls: &[String]) -> usize {
        let before = self.items.len();
        let gone: std::collections::HashSet<&str> = urls.iter().map(String::as_str).collect();
        self.items.retain(|e| !gone.contains(e.url.as_str()));
        let removed = before - self.items.len();
        if removed > 0 {
            self.dirty = true;
            self.flush();
        }
        removed
    }

    /// Forget everything visited at or after `since_ms` (Unix milliseconds). Returns how many
    /// were removed.
    pub fn clear_since(&mut self, since_ms: u64) -> usize {
        let before = self.items.len();
        self.items.retain(|e| e.last_visit < since_ms);
        let removed = before - self.items.len();
        if removed > 0 {
            self.dirty = true;
            self.flush();
        }
        removed
    }

    /// Write to disk if anything changed, keeping the newest [`MAX_HISTORY`].
    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        if self.items.len() > MAX_HISTORY {
            self.items.sort_by_key(|e| std::cmp::Reverse(e.last_visit));
            self.items.truncate(MAX_HISTORY);
        }
        if let Ok(json) = serde_json::to_string(&self.items) {
            let _ = write_atomic(&self.path, &json);
        }
    }
}

/* ------------------------------------------------------------- downloads */

/// One file the user downloaded.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadEntry {
    /// Stable id for the list (not the engine's per-session download id)
    pub key: u64,
    pub name: String,
    /// Full path the file was saved to
    pub path: String,
    pub bytes: u64,
    /// "downloading", "done" or "stopped"
    pub state: String,
    /// Unix milliseconds the download began
    pub started: u64,
    /// The host the file came from, when known
    #[serde(default)]
    pub source: String,
}

/// Cap on how many downloads are remembered.
pub const MAX_DOWNLOADS: usize = 500;

/// The list of downloads, newest first, kept across restarts.
#[derive(Debug, Default)]
pub struct DownloadLog {
    path: PathBuf,
    items: Vec<DownloadEntry>,
    dirty: bool,
}

impl DownloadLog {
    pub fn load(path: PathBuf) -> Self {
        let mut items: Vec<DownloadEntry> = read_json(&path);
        let mut dirty = false;
        // A download still marked running was cut off by a crash or quit.
        for e in &mut items {
            if e.state == "downloading" {
                e.state = "stopped".into();
                dirty = true;
            }
        }
        Self { path, items, dirty }
    }

    pub fn items(&self) -> &[DownloadEntry] {
        &self.items
    }

    pub fn get(&self, key: u64) -> Option<&DownloadEntry> {
        self.items.iter().find(|e| e.key == key)
    }

    /// Note a download that has just begun; returns its key.
    pub fn start(&mut self, name: &str, path: &str, source: &str, now_ms: u64) -> u64 {
        let key = self.items.iter().map(|e| e.key).max().unwrap_or(0) + 1;
        self.items.insert(
            0,
            DownloadEntry {
                key,
                name: name.chars().take(200).collect(),
                path: path.to_string(),
                bytes: 0,
                state: "downloading".into(),
                started: now_ms,
                source: source.chars().take(120).collect(),
            },
        );
        self.items.truncate(MAX_DOWNLOADS);
        self.dirty = true;
        key
    }

    /// Record how a download ended ("done" or "stopped") and its final size.
    pub fn finish(&mut self, key: u64, state: &str, bytes: u64) {
        if let Some(e) = self.items.iter_mut().find(|e| e.key == key) {
            e.state = state.to_string();
            if bytes > 0 {
                e.bytes = bytes;
            }
            self.dirty = true;
        }
    }

    /// Take a download off the list (the file itself is untouched).
    pub fn remove(&mut self, key: u64) -> bool {
        let before = self.items.len();
        self.items.retain(|e| e.key != key);
        self.dirty |= self.items.len() != before;
        self.items.len() != before
    }

    /// Forget downloads begun at or after `since_ms`.
    pub fn clear_since(&mut self, since_ms: u64) {
        let before = self.items.len();
        self.items.retain(|e| e.started < since_ms);
        self.dirty |= self.items.len() != before;
    }

    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        if let Ok(json) = serde_json::to_string(&self.items) {
            let _ = write_atomic(&self.path, &json);
        }
    }
}

/* ---------------------------------------------------------------- cranes */

/// How many paper cranes have been folded. Each closed tab and each saved file folds one;
/// a thousand (senbazuru) is a wish. Purely a keepsake: it counts, it never reports.
#[derive(Debug, Default)]
pub struct Cranes {
    path: PathBuf,
    count: u32,
    dirty: bool,
}

/// Cranes in a full string-of-strings: 25 strings of 40.
pub const THOUSAND: u32 = 1000;

impl Cranes {
    pub fn load(path: PathBuf) -> Self {
        #[derive(Deserialize, Default)]
        struct File {
            cranes: u32,
        }
        let f: File = read_json(&path);
        Self {
            path,
            count: f.cranes,
            dirty: false,
        }
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    /// Fold `n` more. Returns true when this crossed a thousand.
    pub fn fold(&mut self, n: u32) -> bool {
        let before = self.count;
        self.count = self.count.saturating_add(n);
        self.dirty = true;
        before / THOUSAND != self.count / THOUSAND
    }

    pub fn flush(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let json = format!("{{\"cranes\":{}}}", self.count);
        let _ = write_atomic(&self.path, &json);
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

#[cfg(test)]
mod crane_tests {
    use super::*;

    #[test]
    fn counts_persists_and_notices_a_thousand() {
        let p = std::env::temp_dir().join(format!("shoji_cranes_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut c = Cranes::load(p.clone());
        assert_eq!(c.count(), 0);
        assert!(!c.fold(998));
        assert!(!c.fold(1));
        assert!(c.fold(1), "the thousandth crane is noticed once");
        assert!(!c.fold(1));
        c.flush();
        assert_eq!(Cranes::load(p.clone()).count(), 1001);
        let _ = std::fs::remove_file(&p);
    }
}

#[cfg(test)]
mod download_tests {
    use super::*;

    fn path(n: &str) -> PathBuf {
        std::env::temp_dir().join(format!("shoji_dl_{}_{n}.json", std::process::id()))
    }

    #[test]
    fn logs_finishes_and_recovers() {
        let p = path("a");
        let _ = std::fs::remove_file(&p);
        let mut log = DownloadLog::load(p.clone());
        let a = log.start("a.zip", "C:\\d\\a.zip", "example.com", 10);
        let b = log.start("b.pdf", "C:\\d\\b.pdf", "", 20);
        assert_ne!(a, b);
        assert_eq!(log.items()[0].key, b, "newest first");
        log.finish(a, "done", 4096);
        log.flush();
        let again = DownloadLog::load(p.clone());
        assert_eq!(
            again.get(a).map(|e| (e.state.as_str(), e.bytes)),
            Some(("done", 4096))
        );
        assert_eq!(again.get(b).map(|e| e.state.as_str()), Some("stopped"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn removes_and_clears_by_time() {
        let mut log = DownloadLog::load(path("b"));
        let old = log.start("old", "p", "", 100);
        let new = log.start("new", "p", "", 900);
        log.clear_since(500);
        assert!(log.get(new).is_none() && log.get(old).is_some());
        assert!(log.remove(old) && !log.remove(old));
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

    #[test]
    fn pages_removes_and_clears_by_time() {
        let mut h = History::load(std::env::temp_dir().join("shoji_hist_pages_unused.json"));
        for (i, u) in ["https://a.test/", "https://b.test/", "https://c.test/"]
            .iter()
            .enumerate()
        {
            h.record(u, &format!("Page {i}"), 100 * (i as u64 + 1));
        }
        let (page, total) = h.page("", 0, 2);
        assert_eq!(total, 3);
        assert_eq!(page[0].url, "https://c.test/");
        assert_eq!(page.len(), 2);
        assert_eq!(h.page("page 1", 0, 10).1, 1);
        assert_eq!(h.clear_since(200), 2);
        assert_eq!(h.len(), 1);
        assert_eq!(h.remove(&["https://a.test/".to_string()]), 1);
        assert!(h.is_empty());
    }

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
