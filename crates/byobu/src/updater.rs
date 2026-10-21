//! Conditional-GET list cache. Network work happens on the caller's (worker) thread.

use crate::sources::{self, ListSource};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Default, Serialize, Deserialize)]
struct Meta {
    etag: Option<String>,
    last_modified: Option<String>,
    checked_at: u64,
}

/// Disk cache of upstream lists and resources, laid out as `<dir>/<id>.<ext>` + `<id>.meta.json`.
pub struct Updater {
    dir: PathBuf,
    agent: ureq::Agent,
}

/// What a refresh pass found.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Refresh {
    /// Number of cached files that actually changed.
    pub changed: usize,
    /// Number of sources that could not be fetched (cached copies stay in use).
    pub failed: usize,
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Updater {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(45)))
            .user_agent("Shoji-Byobu/0.1")
            .http_status_as_error(false)
            .build()
            .into();
        let dir = dir.into();
        let _ = fs::create_dir_all(&dir);
        Self { dir, agent }
    }

    /// The cache folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn file(&self, id: &str, ext: &str) -> PathBuf {
        self.dir.join(format!("{id}.{ext}"))
    }

    fn meta_path(&self, id: &str) -> PathBuf {
        self.dir.join(format!("{id}.meta.json"))
    }

    fn read_meta(path: &Path) -> Meta {
        fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Fetch one file if it is due (or `force`). `Ok(true)` when the cached copy changed.
    fn fetch(
        &self,
        id: &str,
        ext: &str,
        url: &str,
        every: u64,
        force: bool,
    ) -> Result<bool, String> {
        let file = self.file(id, ext);
        let meta_path = self.meta_path(id);
        let mut meta = Self::read_meta(&meta_path);
        let have = file.exists();
        if have && !force && now().saturating_sub(meta.checked_at) < every {
            return Ok(false);
        }
        let mut req = self.agent.get(url);
        if have {
            if let Some(e) = &meta.etag {
                req = req.header("If-None-Match", e);
            }
            if let Some(m) = &meta.last_modified {
                req = req.header("If-Modified-Since", m);
            }
        }
        let mut resp = req.call().map_err(|e| e.to_string())?;
        meta.checked_at = now();
        let status = resp.status().as_u16();
        let changed = match status {
            304 => false,
            200 => {
                let hv = |n: &str| {
                    resp.headers()
                        .get(n)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned)
                };
                meta.etag = hv("etag");
                meta.last_modified = hv("last-modified");
                let body = resp
                    .body_mut()
                    .read_to_string()
                    .map_err(|e| e.to_string())?;
                if body.trim().is_empty() {
                    return Err("empty response".into());
                }
                let same = fs::read_to_string(&file)
                    .map(|old| old == body)
                    .unwrap_or(false);
                if !same {
                    let tmp = file.with_extension("tmp");
                    fs::write(&tmp, body).map_err(|e| e.to_string())?;
                    fs::rename(&tmp, &file).map_err(|e| e.to_string())?;
                }
                !same
            }
            s => return Err(format!("HTTP {s}")),
        };
        let _ = fs::write(&meta_path, serde_json::to_string(&meta).unwrap_or_default());
        Ok(changed)
    }

    /// The full source list: uBlock's set, the community lists, and Brave's current defaults.
    pub fn sources(&self, force: bool) -> Vec<ListSource> {
        self.sources_with(force, &[])
    }

    /// Like [`Updater::sources`], plus lists the user added by URL.
    pub fn sources_with(&self, force: bool, custom: &[String]) -> Vec<ListSource> {
        let mut all = sources::ublock_lists();
        all.extend(sources::community_lists());
        let _ = self.fetch("catalog", "json", sources::CATALOG_URL, 6 * 3600, force);
        if let Ok(json) = fs::read_to_string(self.file("catalog", "json")) {
            let extra = sources::brave_lists_from_catalog(&json, &all);
            all.extend(extra);
        }
        for url in custom {
            if url.starts_with("https://") || url.starts_with("http://") {
                let l = sources::custom_list(url);
                if !all.iter().any(|a| a.id == l.id) {
                    all.push(l);
                }
            }
        }
        all
    }

    /// Check every source that is due. Safe to call often; only due sources hit the network.
    pub fn refresh(&self, force: bool) -> (Vec<ListSource>, Refresh) {
        self.refresh_with(force, &[], &[])
    }

    /// [`Updater::refresh`] for a configured set: `custom` list URLs are added and lists in
    /// `disabled` are neither fetched nor returned.
    pub fn refresh_with(
        &self,
        force: bool,
        custom: &[String],
        disabled: &[String],
    ) -> (Vec<ListSource>, Refresh) {
        let all: Vec<ListSource> = self
            .sources_with(force, custom)
            .into_iter()
            .filter(|s| !disabled.contains(&s.id))
            .collect();
        let mut r = Refresh::default();
        let mut tally = |res: Result<bool, String>| match res {
            Ok(true) => r.changed += 1,
            Ok(false) => {}
            Err(_) => r.failed += 1,
        };
        tally(self.fetch("resources", "json", sources::RESOURCES_URL, 6 * 3600, force));
        tally(self.fetch(
            "ubo-resources",
            "json",
            sources::UBO_RESOURCES_URL,
            6 * 3600,
            force,
        ));
        for s in &all {
            tally(self.fetch(&s.id, "txt", &s.url, s.refresh_secs, force));
        }
        (all, r)
    }

    /// Resource bundles in priority order: uBlock Origin's scriptlets and redirect stubs, then
    /// Brave's own. Either may be absent until it has been fetched once.
    pub fn resource_jsons(&self) -> Vec<String> {
        ["ubo-resources", "resources"]
            .iter()
            .filter_map(|id| fs::read_to_string(self.file(id, "json")).ok())
            .collect()
    }

    /// Cached list text for a source, if it has ever been fetched.
    pub fn list_text(&self, id: &str) -> Option<String> {
        fs::read_to_string(self.file(id, "txt")).ok()
    }

    pub fn resources_json(&self) -> Option<String> {
        fs::read_to_string(self.file("resources", "json")).ok()
    }

    /// What the cache holds for a source: (size in bytes, rule lines, last check in unix seconds).
    pub fn list_stats(&self, id: &str) -> (u64, usize, u64) {
        let text = fs::read_to_string(self.file(id, "txt")).unwrap_or_default();
        let rules = text
            .lines()
            .filter(|l| {
                let l = l.trim();
                !l.is_empty() && !l.starts_with('!') && !l.starts_with('[') && !l.starts_with('#')
            })
            .count();
        let checked = Self::read_meta(&self.meta_path(id)).checked_at;
        (text.len() as u64, rules, checked)
    }

    /// Seconds until the earliest source falls due (for scheduling the next pass).
    pub fn next_due_in(&self, all: &[ListSource]) -> u64 {
        all.iter()
            .map(|s| {
                let m = Self::read_meta(&self.meta_path(&s.id));
                (m.checked_at + s.refresh_secs).saturating_sub(now())
            })
            .min()
            .unwrap_or(3600)
            .max(300)
    }
}
