//! Small JSON stores for things the user accumulates: favourites, remembered per-site choices,
//! and the open-tabs session used to recover after a crash. Each is a single file written
//! atomically (temp file + rename), so a crash mid-write never corrupts it.

use crate::tabs::TabGroup;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub fn write_atomic(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, content)?;
    std::fs::rename(&tmp, path)
}

/// Keep a copy of a file that exists but can't be understood (damaged, cut short, or from a
/// newer version) next to it as `<name>.bad`, because the next save would overwrite it with
/// defaults. A failed copy must never stop the app from starting.
pub fn preserve_unreadable(path: &Path) {
    let mut bad = path.as_os_str().to_owned();
    bad.push(".bad");
    let _ = std::fs::copy(path, PathBuf::from(bad));
}

/// File text without a leading byte-order mark.
pub fn read_text(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    Some(match text.strip_prefix('\u{feff}') {
        Some(rest) => rest.to_string(),
        None => text,
    })
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(path: &Path) -> T {
    let Some(text) = read_text(path) else {
        return T::default();
    };
    match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => {
            if !text.trim().is_empty() {
                preserve_unreadable(path);
            }
            T::default()
        }
    }
}

/* ------------------------------------------------------------ favourites */

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Favourite {
    pub url: String,
    pub title: String,
    /// Folder it is filed in ("" for the top level; nested folders are joined with " / ")
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub folder: String,
}

impl Favourite {
    pub fn new(url: &str, title: &str) -> Self {
        Self {
            url: url.to_string(),
            title: title.to_string(),
            folder: String::new(),
        }
    }
}

/// Folder paths are names joined with " / ". Names are trimmed, capped and kept free of control
/// characters; a "/" typed in a name starts a new level.
pub fn clean_folder(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_control())
        .collect::<String>()
        .split('/')
        .map(|p| p.trim().chars().take(60).collect::<String>())
        .filter(|p| !p.is_empty())
        .take(8)
        .collect::<Vec<_>>()
        .join(" / ")
}

/// `"A / B"` -> `"A"`; a top-level folder's parent is `""`.
pub fn folder_parent(folder: &str) -> &str {
    folder.rsplit_once(" / ").map_or("", |(p, _)| p)
}

/// `"A / B"` -> `"B"`.
pub fn folder_leaf(folder: &str) -> &str {
    folder.rsplit_once(" / ").map_or(folder, |(_, l)| l)
}

/// Whether `folder` is `inside` or somewhere below it.
pub fn folder_within(folder: &str, inside: &str) -> bool {
    folder == inside || folder.starts_with(&format!("{inside} / "))
}

/// Cap so a runaway script can't grow the file without bound.
const MAX_FAVOURITES: usize = 5000;

pub fn same_page(a: &str, b: &str) -> bool {
    a.trim_end_matches('/') == b.trim_end_matches('/')
}

/// Cap on folders kept without any bookmark in them.
const MAX_EMPTY_FOLDERS: usize = 500;

#[derive(Debug, Default)]
pub struct Favourites {
    path: PathBuf,
    items: Vec<Favourite>,
    /// Folders that hold nothing yet (a folder with bookmarks in it needs no entry here); kept in
    /// a file of their own beside the bookmarks so older versions still read the bookmarks.
    empty: Vec<String>,
}

impl Favourites {
    pub fn load(path: PathBuf) -> Self {
        let items: Vec<Favourite> = read_json(&path);
        let empty: Vec<String> = read_json(&Self::folders_path(&path));
        Self { path, items, empty }
    }

    fn folders_path(path: &Path) -> PathBuf {
        path.with_file_name("favourite-folders.json")
    }

    pub fn items(&self) -> &[Favourite] {
        &self.items
    }

    /// Every folder, including empty ones and the folders above nested ones, sorted by name.
    pub fn all_folders(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for f in self
            .items
            .iter()
            .map(|f| f.folder.as_str())
            .chain(self.empty.iter().map(String::as_str))
            .filter(|f| !f.is_empty())
        {
            let mut path = String::new();
            for part in f.split(" / ") {
                if !path.is_empty() {
                    path.push_str(" / ");
                }
                path.push_str(part);
                out.push(path.clone());
            }
        }
        out.sort_by_key(|f| f.to_lowercase());
        out.dedup();
        out
    }

    /// Make a folder (and the ones above it). Returns its cleaned path, or `None` for a blank name.
    pub fn add_folder(&mut self, name: &str) -> Option<String> {
        let name = clean_folder(name);
        if name.is_empty() {
            return None;
        }
        if !self.all_folders().contains(&name) && self.empty.len() < MAX_EMPTY_FOLDERS {
            self.empty.push(name.clone());
            self.save();
        }
        Some(name)
    }

    /// Remove a folder and every folder below it. With `keep_items` its bookmarks move up to the
    /// folder's parent; otherwise they are deleted. Returns how many bookmarks were affected.
    pub fn delete_folder(&mut self, name: &str, keep_items: bool) -> usize {
        let parent = folder_parent(name).to_string();
        let mut n = 0;
        if keep_items {
            for f in self
                .items
                .iter_mut()
                .filter(|f| folder_within(&f.folder, name))
            {
                // Items directly in it go to its parent; deeper ones lose just the removed level.
                f.folder = if f.folder == name {
                    parent.clone()
                } else {
                    let rest = f.folder[name.len() + 3..].to_string();
                    if parent.is_empty() {
                        rest
                    } else {
                        format!("{parent} / {rest}")
                    }
                };
                n += 1;
            }
        } else {
            let before = self.items.len();
            self.items.retain(|f| !folder_within(&f.folder, name));
            n = before - self.items.len();
        }
        self.empty.retain(|f| !folder_within(f, name));
        self.save();
        n
    }

    /// Move a folder under `new_parent` ("" for the top level). Returns its new path, or `None`
    /// when that would put it inside itself.
    pub fn move_folder(&mut self, from: &str, new_parent: &str) -> Option<String> {
        let new_parent = clean_folder(new_parent);
        if from.is_empty() || folder_within(&new_parent, from) {
            return None;
        }
        let leaf = folder_leaf(from);
        let to = if new_parent.is_empty() {
            leaf.to_string()
        } else {
            format!("{new_parent} / {leaf}")
        };
        if to == from {
            return Some(to);
        }
        // Keep the folder in the list even when it holds nothing.
        if !self.items.iter().any(|f| folder_within(&f.folder, from))
            && !self.empty.iter().any(|f| f == from)
        {
            return None;
        }
        self.rename_folder(from, &to);
        Some(to)
    }

    /// The bookmarks in `folder`, and (with `deep`) in the folders below it, in order.
    pub fn urls_in(&self, folder: &str, deep: bool) -> Vec<String> {
        self.items
            .iter()
            .filter(|f| {
                if deep {
                    folder_within(&f.folder, folder)
                } else {
                    f.folder == folder
                }
            })
            .map(|f| f.url.clone())
            .collect()
    }

    /// Add into a folder (made on the way). Returns false for a duplicate or a non-web address.
    pub fn add_in(&mut self, url: &str, title: &str, folder: &str) -> bool {
        if !self.add(url, title) {
            return false;
        }
        if !folder.is_empty() {
            self.set_folder(url, folder);
        }
        true
    }

    /// Change a bookmark's name and address. Returns false if it isn't saved, the new address is
    /// not a web page, or another bookmark already has it.
    pub fn edit(&mut self, url: &str, title: &str, new_url: &str) -> bool {
        let new_url = new_url.trim();
        let ok = new_url.starts_with("https://") || new_url.starts_with("http://");
        if !ok || (!same_page(url, new_url) && self.contains(new_url)) || !self.contains(url) {
            return false;
        }
        let title: String = title.trim().chars().take(120).collect();
        if let Some(f) = self.items.iter_mut().find(|f| same_page(&f.url, url)) {
            f.url = new_url.to_string();
            f.title = if title.is_empty() {
                new_url.to_string()
            } else {
                title
            };
        }
        self.save();
        true
    }

    /// Put a bookmark in `folder` just before the bookmark `before` (at the end when `None` or
    /// not found). Drag-and-drop and "move to" both come through here.
    pub fn move_to(&mut self, url: &str, folder: &str, before: Option<&str>) -> bool {
        let Some(pos) = self.items.iter().position(|f| same_page(&f.url, url)) else {
            return false;
        };
        let mut item = self.items.remove(pos);
        item.folder = clean_folder(folder);
        let at = before
            .and_then(|b| self.items.iter().position(|f| same_page(&f.url, b)))
            .unwrap_or(self.items.len());
        self.items.insert(at, item);
        self.save();
        true
    }

    /// Remove several at once. Returns how many were removed.
    pub fn remove_many(&mut self, urls: &[String]) -> usize {
        let before = self.items.len();
        self.items
            .retain(|f| !urls.iter().any(|u| same_page(&f.url, u)));
        let n = before - self.items.len();
        if n > 0 {
            self.save();
        }
        n
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
            folder: String::new(),
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
                folder: clean_folder(&f.folder),
            });
        }
        let added = self.items.len() - before;
        if added > 0 {
            self.save();
        }
        added
    }

    /// File a favourite in `folder` ("" for the top level). Returns false if it isn't saved.
    pub fn set_folder(&mut self, url: &str, folder: &str) -> bool {
        let folder = clean_folder(folder);
        let Some(f) = self.items.iter_mut().find(|f| same_page(&f.url, url)) else {
            return false;
        };
        if f.folder != folder {
            f.folder = folder;
            self.save();
        }
        true
    }

    /// Every folder in use, sorted.
    pub fn folders(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .items
            .iter()
            .filter(|f| !f.folder.is_empty())
            .map(|f| f.folder.clone())
            .collect();
        out.sort_by_key(|f| f.to_lowercase());
        out.dedup();
        out
    }

    /// Rename a folder and the folders nested in it (an empty `to` moves its favourites up to the
    /// top level).
    pub fn rename_folder(&mut self, from: &str, to: &str) -> usize {
        let to = clean_folder(to);
        let mut n = 0;
        let prefix = format!("{from} / ");
        for f in self.items.iter_mut() {
            let rest = if f.folder == from {
                Some("")
            } else {
                f.folder.strip_prefix(&prefix)
            };
            let Some(rest) = rest else { continue };
            f.folder = match (to.is_empty(), rest.is_empty()) {
                (_, true) => to.clone(),
                (true, false) => rest.to_string(),
                (false, false) => format!("{to} / {rest}"),
            };
            n += 1;
        }
        for f in self.empty.iter_mut() {
            let rest = if f == from {
                Some("")
            } else {
                f.strip_prefix(&prefix)
            };
            let Some(rest) = rest else { continue };
            *f = match (to.is_empty(), rest.is_empty()) {
                (_, true) => to.clone(),
                (true, false) => rest.to_string(),
                (false, false) => format!("{to} / {rest}"),
            };
            n += 1;
        }
        self.empty.retain(|f| !f.is_empty());
        self.empty.sort();
        self.empty.dedup();
        if n > 0 {
            self.save();
        }
        n
    }

    /// The favourites as a Netscape bookmarks file, which every browser can import.
    pub fn to_netscape_html(&self) -> String {
        fn esc(s: &str) -> String {
            s.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;")
                .replace('"', "&quot;")
        }
        let line = |f: &Favourite, pad: &str| {
            format!(
                "{pad}<DT><A HREF=\"{}\">{}</A>\n",
                esc(&f.url),
                esc(&f.title)
            )
        };
        let mut out = String::from(
            "<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; charset=UTF-8\">\n<TITLE>Bookmarks</TITLE>\n<H1>Bookmarks</H1>\n<DL><p>\n",
        );
        let all = self.all_folders();
        // Nested to any depth: a folder's bookmarks, then the folders directly below it.
        fn write(
            favs: &Favourites,
            folder: &str,
            all: &[String],
            depth: usize,
            out: &mut String,
            line: &dyn Fn(&Favourite, &str) -> String,
        ) {
            let pad = "    ".repeat(depth);
            for f in favs.items.iter().filter(|f| f.folder == folder) {
                out.push_str(&line(f, &pad));
            }
            for child in all.iter().filter(|c| folder_parent(c) == folder) {
                out.push_str(&format!(
                    "{pad}<DT><H3>{}</H3>\n{pad}<DL><p>\n",
                    esc(folder_leaf(child))
                ));
                write(favs, child, all, depth + 1, out, line);
                out.push_str(&format!("{pad}</DL><p>\n"));
            }
        }
        write(self, "", &all, 1, &mut out, &line);
        out.push_str("</DL><p>\n");
        out
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
        let side = Self::folders_path(&self.path);
        if self.empty.is_empty() {
            let _ = std::fs::remove_file(&side);
        } else if let Ok(json) = serde_json::to_string_pretty(&self.empty) {
            let _ = write_atomic(&side, &json);
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

/// The lower-cased words of a search query.
fn query_words(query: &str) -> Vec<String> {
    query
        .to_lowercase()
        .split_whitespace()
        .map(String::from)
        .collect()
}

/// Whether `hay` contains the already lower-cased `word`, ignoring case. ASCII words (nearly all
/// of them) are compared in place; this runs for every history entry on every keystroke, so it
/// must not allocate.
fn contains_ci(hay: &str, word: &str) -> bool {
    if word.is_ascii() {
        let (h, w) = (hay.as_bytes(), word.as_bytes());
        w.is_empty() || h.windows(w.len()).any(|win| win.eq_ignore_ascii_case(w))
    } else {
        hay.to_lowercase().contains(word)
    }
}

fn entry_matches(e: &HistoryEntry, words: &[String]) -> bool {
    words
        .iter()
        .all(|w| contains_ci(&e.title, w) || contains_ci(&e.url, w))
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
        let words = query_words(query);
        let mut hits: Vec<&HistoryEntry> = self
            .items
            .iter()
            .filter(|e| entry_matches(e, &words))
            .collect();
        let rank = |e: &&HistoryEntry| std::cmp::Reverse((e.visits.min(20), e.last_visit));
        // Only the best few are wanted: partition first, then sort just those.
        if hits.len() > limit && limit > 0 {
            hits.select_nth_unstable_by_key(limit - 1, rank);
        }
        hits.truncate(limit);
        hits.sort_by_key(rank);
        hits
    }

    pub fn clear(&mut self) {
        self.items.clear();
        self.dirty = true;
        self.flush();
    }

    /// The site the user most likely means by `typed` (the start of an address, such as "yout"),
    /// judged by how often and how recently they went there: its address as `https://host/`
    /// and the host as it should be completed in the address bar. `None` when nothing visited
    /// starts with what was typed.
    pub fn autocomplete(&self, typed: &str) -> Option<(String, String)> {
        let typed = typed.trim().to_lowercase();
        let typed = typed
            .strip_prefix("https://")
            .or_else(|| typed.strip_prefix("http://"))
            .unwrap_or(&typed);
        let typed = typed.strip_prefix("www.").unwrap_or(typed);
        if typed.is_empty() || typed.contains(char::is_whitespace) || typed.contains('/') {
            return None;
        }
        // (host, https?) -> (score, newest visit)
        let mut best: std::collections::HashMap<String, (u64, u64, bool)> =
            std::collections::HashMap::new();
        for e in &self.items {
            let secure = e.url.starts_with("https://");
            let Some(rest) = e
                .url
                .strip_prefix("https://")
                .or_else(|| e.url.strip_prefix("http://"))
            else {
                continue;
            };
            let host = rest.split(['/', '?', '#']).next().unwrap_or("");
            let host = host.rsplit('@').next().unwrap_or("").to_lowercase();
            let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
            // The host itself, or its parent domains ("en.wikipedia.org" also counts as
            // "wikipedia.org"), whichever the typed start belongs to.
            let mut rest = host.as_str();
            loop {
                if rest.starts_with(typed) {
                    let entry = best.entry(rest.to_string()).or_insert((0, 0, secure));
                    entry.0 += u64::from(e.visits.clamp(1, 20));
                    entry.1 = entry.1.max(e.last_visit);
                    entry.2 |= secure;
                }
                match rest.split_once('.') {
                    Some((_, parent)) if parent.contains('.') => rest = parent,
                    _ => break,
                }
            }
        }
        best.into_iter()
            .max_by_key(|(_, (score, last, _))| (*score, *last))
            .map(|(host, (_, _, secure))| {
                (
                    format!("{}://{host}/", if secure { "https" } else { "http" }),
                    host,
                )
            })
    }

    /// One page of the history list, most recently visited first, filtered by `query` (every
    /// word must appear in the title or address). Returns the page and how many entries match.
    pub fn page(&self, query: &str, offset: usize, limit: usize) -> (Vec<&HistoryEntry>, usize) {
        let words = query_words(query);
        let mut hits: Vec<&HistoryEntry> = self
            .items
            .iter()
            .filter(|e| entry_matches(e, &words))
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

/* ------------------------------------------------------------- keepsakes */

/// The small hidden things in Shoji that can be found, in the order the wish page lists them.
/// Ids only: what each one is stays in the pages that hold it.
pub const SECRETS: [&str; 13] = [
    "welcome", "hello", "fold", "colours", "kanji", "seasons", "konami", "sleep", "night", "koan",
    "tea", "about", "wish",
];

/// A crane count worth a quiet word on the way to a thousand (and each thousand after).
pub fn crane_milestone(before: u32, after: u32) -> Option<u32> {
    [100, 250, 500]
        .into_iter()
        .find(|&m| before < m && after >= m)
        .or_else(|| (before / THOUSAND != after / THOUSAND).then_some(after / THOUSAND * THOUSAND))
}

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct KeepsakeFile {
    /// When Shoji and its user met (ms since the epoch)
    met: u64,
    /// When Shoji was last opened
    last_seen: u64,
    /// Hidden things found so far (ids from `SECRETS`)
    found: Vec<String>,
}

/// What the crane remembers: when you met, when it last saw you, and which of its hidden
/// things you have found. Like the crane count, it only stays on this PC.
#[derive(Debug, Default)]
pub struct Keepsakes {
    path: PathBuf,
    data: KeepsakeFile,
    /// `last_seen` as it was before this run
    previous: u64,
}

impl Keepsakes {
    /// Load, and mark this run as a visit. `met_fallback` dates an install that predates this
    /// file (the age of the settings file, say).
    pub fn load(path: PathBuf, met_fallback: u64, now: u64) -> Self {
        let mut data: KeepsakeFile = read_json(&path);
        let previous = data.last_seen;
        if data.met == 0 {
            data.met = if met_fallback > 0 {
                met_fallback.min(now)
            } else {
                now
            };
        }
        data.last_seen = now;
        data.found.retain(|id| SECRETS.contains(&id.as_str()));
        let k = Self {
            path,
            data,
            previous,
        };
        k.save();
        k
    }

    fn save(&self) {
        if let Ok(json) = serde_json::to_string(&self.data) {
            let _ = write_atomic(&self.path, &json);
        }
    }

    pub fn met(&self) -> u64 {
        self.data.met
    }

    /// When Shoji was opened before this run (0 the first time)
    pub fn previous_visit(&self) -> u64 {
        self.previous
    }

    pub fn found(&self) -> &[String] {
        &self.data.found
    }

    /// Shoji is opened again (a window from the tray): returns when it was last open before.
    pub fn visit(&mut self, now: u64) -> u64 {
        self.previous = self.data.last_seen;
        self.data.last_seen = now;
        self.save();
        self.previous
    }

    /// Still here (the last window is closing): the next visit counts from now.
    pub fn touch(&mut self, now: u64) {
        self.data.last_seen = now;
        self.save();
    }

    /// Note a hidden thing as found. True the first time; unknown ids are ignored.
    pub fn find(&mut self, id: &str) -> bool {
        if !SECRETS.contains(&id) || self.data.found.iter().any(|f| f == id) {
            return false;
        }
        self.data.found.push(id.to_string());
        self.save();
        true
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

/// What is remembered about one site.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SiteEntry {
    pub host: String,
    pub zoom: Option<f64>,
    /// (permission, allowed)
    pub permissions: Vec<(String, bool)>,
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

    /// Everything remembered, one entry per site, for the Settings list.
    pub fn entries(&self) -> Vec<SiteEntry> {
        let mut hosts: BTreeSet<&String> = self.data.zoom.keys().collect();
        hosts.extend(self.data.permissions.keys());
        hosts
            .into_iter()
            .map(|host| SiteEntry {
                host: host.clone(),
                zoom: self.data.zoom.get(host).copied(),
                permissions: self
                    .data
                    .permissions
                    .get(host)
                    .map(|p| p.iter().map(|(k, v)| (k.clone(), v == "allow")).collect())
                    .unwrap_or_default(),
            })
            .collect()
    }

    /// Forget one remembered choice: `kind` is a permission name, `"zoom"`, or `None` for the
    /// whole site. Returns whether anything was removed.
    pub fn forget(&mut self, host: &str, kind: Option<&str>) -> bool {
        let changed = match kind {
            None => {
                self.data.zoom.remove(host).is_some() | self.data.permissions.remove(host).is_some()
            }
            Some("zoom") => self.data.zoom.remove(host).is_some(),
            Some(k) => {
                let removed = self
                    .data
                    .permissions
                    .get_mut(host)
                    .is_some_and(|p| p.remove(k).is_some());
                if self
                    .data
                    .permissions
                    .get(host)
                    .is_some_and(|p| p.is_empty())
                {
                    self.data.permissions.remove(host);
                }
                removed
            }
        };
        if changed {
            self.save();
        }
        changed
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

fn default_session_split_color() -> String {
    "blue".to_string()
}

const fn default_session_split_ratio() -> f64 {
    crate::tabs::DEFAULT_SPLIT_RATIO
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSplit {
    /// Group id (0 in files written before split groups).
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default = "default_session_split_color")]
    pub color: String,
    /// Indices into this workspace's tabs of the split's panes, in screen order.
    pub panes: Vec<usize>,
    #[serde(default)]
    pub orientation: crate::tabs::SplitOrientation,
    #[serde(default = "default_session_split_ratio")]
    pub ratio: f64,
    /// Each pane's share of the space (empty in older files: `ratio` or equal shares)
    #[serde(default)]
    pub weights: Vec<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionWorkspace {
    pub id: u32,
    pub name: String,
    pub color: String,
    /// Index into this workspace's tabs of the tab that was active in this workspace.
    #[serde(default)]
    pub active: usize,
    /// This workspace's split groups.
    #[serde(default)]
    pub splits: Vec<SessionSplit>,
    /// Legacy single split (read and migrated, never written).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<SessionSplit>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionTab {
    pub url: String,
    pub title: String,
    #[serde(default)]
    pub group: Option<u32>,
    #[serde(default)]
    pub pinned: bool,
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
    fn autocomplete_prefers_the_site_visited_most() {
        let mut h = History::load(std::env::temp_dir().join("shoji_hist_auto_unused.json"));
        for _ in 0..5 {
            h.record("https://www.youtube.com/watch?v=1", "A video", 100);
        }
        h.record("https://www.youtube.com/watch?v=2", "Another", 200);
        h.record("https://youtu.be/xyz", "Short link", 300);
        h.record("https://example.org/", "Example", 400);
        let (url, host) = h.autocomplete("yout").unwrap();
        assert_eq!(
            (url.as_str(), host.as_str()),
            ("https://youtube.com/", "youtube.com")
        );
        assert_eq!(h.autocomplete("youtu.").unwrap().1, "youtu.be");
        assert_eq!(h.autocomplete("https://www.exa").unwrap().1, "example.org");
        h.record("https://en.wikipedia.org/wiki/Rust", "Rust", 500);
        assert_eq!(h.autocomplete("wiki").unwrap().1, "wikipedia.org");
        assert_eq!(h.autocomplete("en.wi").unwrap().1, "en.wikipedia.org");
        assert!(h.autocomplete("nothing").is_none());
        assert!(h.autocomplete("two words").is_none());
        assert!(h.autocomplete("").is_none());
    }

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

    /// Each test gets its own folder: bookmark folders are kept in a file beside the list, which
    /// tests sharing the temp folder itself would overwrite for each other.
    fn path(n: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shoji_hist_{}_{n}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        dir.join("data.json")
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
            Favourite::new("https://a.com", "A"),
            Favourite::new("https://a.com/", "dup"),
            Favourite::new("ftp://x", "no"),
            Favourite::new("http://b.com", ""),
        ]);
        assert_eq!(n, 2);
        assert_eq!(f.items()[1].title, "http://b.com");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn favourite_folders_file_rename_and_export() {
        let p = path("fav-folders");
        let _ = std::fs::remove_file(&p);
        let mut f = Favourites::load(p.clone());
        f.add("https://a.com", "A & B");
        f.add("https://b.com", "B");
        assert!(f.set_folder("https://a.com/", "  News "));
        assert!(!f.set_folder("https://nope.com", "X"));
        assert_eq!(f.folders(), vec!["News".to_string()]);

        let html = f.to_netscape_html();
        assert!(html.contains("<H3>News</H3>"));
        assert!(html.contains("A &amp; B"));
        // What we export, we can import back, folders included.
        let back = crate::import::bookmarks::from_netscape_html(&html);
        assert_eq!(back.len(), 2);
        assert_eq!(
            back.iter()
                .find(|x| x.url == "https://a.com")
                .unwrap()
                .folder,
            "News"
        );

        assert_eq!(f.rename_folder("News", "Reading"), 1);
        let reloaded = Favourites::load(p.clone());
        assert_eq!(reloaded.folders(), vec!["Reading".to_string()]);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn bookmark_folders_nest_empty_move_and_delete() {
        let p = path("bm-folders");
        let side = Favourites::folders_path(&p);
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(&side);
        let mut f = Favourites::load(p.clone());
        assert_eq!(clean_folder(" Work / /Docs//Q1 "), "Work / Docs / Q1");
        assert_eq!(f.add_folder("Work/Docs").as_deref(), Some("Work / Docs"));
        assert_eq!(f.add_folder("  "), None);
        // Empty folders exist, with the one above them, and survive a reload.
        assert_eq!(f.all_folders(), vec!["Work", "Work / Docs"]);
        assert_eq!(Favourites::load(p.clone()).all_folders().len(), 2);

        assert!(f.add_in("https://a.com", "A", "Work / Docs"));
        assert!(f.add_in("https://b.com", "B", ""));
        assert!(f.add_in("https://c.com", "C", "Work"));
        assert!(!f.add_in("https://c.com/", "again", ""));
        assert_eq!(f.urls_in("Work", false), vec!["https://c.com"]);
        assert_eq!(f.urls_in("Work", true).len(), 2);

        // Edit the name and address; a clash or a non-web address is refused.
        assert!(f.edit("https://b.com", "Bee", "https://bee.com"));
        assert!(!f.edit("https://bee.com", "x", "https://a.com"));
        assert!(!f.edit("https://bee.com", "x", "javascript:1"));
        assert!(f.contains("https://bee.com") && !f.contains("https://b.com"));

        // Reorder: C goes to the top level, ahead of A.
        assert!(f.move_to("https://c.com", "", Some("https://a.com")));
        let order: Vec<&str> = f.items().iter().map(|x| x.url.as_str()).collect();
        assert_eq!(
            order,
            vec!["https://c.com", "https://a.com", "https://bee.com"]
        );
        assert_eq!(f.items()[0].folder, "");

        // A folder moves under another, but never into itself.
        assert_eq!(f.add_folder("Reading").as_deref(), Some("Reading"));
        assert_eq!(
            f.move_folder("Work / Docs", "Reading").as_deref(),
            Some("Reading / Docs")
        );
        assert_eq!(f.move_folder("Reading", "Reading / Docs"), None);
        assert_eq!(
            f.items()
                .iter()
                .find(|x| x.url == "https://a.com")
                .unwrap()
                .folder,
            "Reading / Docs"
        );

        // Exports nest, and importing the export gives the same folders back.
        let back = crate::import::bookmarks::from_netscape_html(&f.to_netscape_html());
        assert_eq!(
            back.iter()
                .find(|x| x.url == "https://a.com")
                .unwrap()
                .folder,
            "Reading / Docs"
        );

        // Deleting a folder either keeps its bookmarks (one level up) or takes them along.
        assert_eq!(f.delete_folder("Reading", true), 1);
        assert_eq!(
            f.items()
                .iter()
                .find(|x| x.url == "https://a.com")
                .unwrap()
                .folder,
            "Docs"
        );
        assert_eq!(f.delete_folder("Docs", false), 1);
        assert!(!f.contains("https://a.com"));
        assert_eq!(
            f.remove_many(&["https://bee.com".into(), "https://c.com/".into()]),
            2
        );
        assert!(f.items().is_empty());
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_file(&side);
    }
}

#[cfg(test)]
mod keepsake_tests {
    use super::*;

    #[test]
    fn keepsakes_remember_meeting_visits_and_finds() {
        let p = std::env::temp_dir().join(format!("shoji-keepsakes-{}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        let mut k = Keepsakes::load(p.clone(), 500, 1000);
        assert_eq!(k.met(), 500, "an older install keeps its date");
        assert_eq!(k.previous_visit(), 0);
        assert!(k.find("konami"));
        assert!(!k.find("konami"), "found once");
        assert!(!k.find("not-a-secret"));
        let k = Keepsakes::load(p.clone(), 0, 9000);
        assert_eq!(k.met(), 500, "the meeting is never moved");
        assert_eq!(k.previous_visit(), 1000);
        assert_eq!(k.found(), ["konami"]);
        let _ = std::fs::remove_file(&p);
        let fresh = Keepsakes::load(p.clone(), 0, 42);
        assert_eq!(fresh.met(), 42);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn milestones_on_the_way_to_a_thousand() {
        assert_eq!(crane_milestone(99, 100), Some(100));
        assert_eq!(crane_milestone(100, 101), None);
        assert_eq!(crane_milestone(249, 251), Some(250));
        assert_eq!(crane_milestone(499, 500), Some(500));
        assert_eq!(crane_milestone(999, 1000), Some(1000));
        assert_eq!(crane_milestone(1999, 2000), Some(2000));
        assert_eq!(crane_milestone(10, 11), None);
    }
}
