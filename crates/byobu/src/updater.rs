//! Conditional-GET list cache. Network work happens on the caller's (worker) thread.
//!
//! A cached copy is only replaced by a download that looks like the same kind of file: an error
//! page, a truncated list or a broken resource bundle is refused and the last good copy stays in
//! use, with the error kept for the Byōbu page.

use crate::sources::{self, ListSource};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Meta {
    etag: Option<String>,
    last_modified: Option<String>,
    checked_at: u64,
    /// When upstream last confirmed the cached copy is current (a new copy or a 304)
    ok_at: u64,
    /// Why the last check failed, if it did
    error: Option<String>,
    error_at: u64,
}

impl Meta {
    /// When the cached copy was last known to be current. Caches from before `ok_at` was kept
    /// use their last check.
    fn fresh_at(&self) -> u64 {
        if self.ok_at > 0 {
            self.ok_at
        } else {
            self.checked_at
        }
    }
}

/// What the cache holds for one source, for the Byōbu page.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ListHealth {
    pub bytes: u64,
    pub rules: usize,
    /// When the copy in use was last confirmed current (unix seconds, 0 if never fetched)
    pub fresh_at: u64,
    /// The last check's error, while it keeps failing
    pub error: Option<String>,
    pub error_at: u64,
}

/// Lists below this size are never judged by how much they shrank.
const SHRINK_FLOOR: usize = 200;

fn rule_count(text: &str) -> usize {
    text.lines()
        .filter(|l| {
            let l = l.trim();
            !l.is_empty() && !l.starts_with('!') && !l.starts_with('[') && !l.starts_with('#')
        })
        .count()
}

/// Is `body` fit to replace `old` as the cached `ext` file? Refuses HTML error and sign-in
/// pages served with a 200, a filter list that lost most of its rules (a cut-off download),
/// and resource bundles that aren't a JSON array.
fn check_body(ext: &str, body: &str, old: Option<&str>) -> Result<(), String> {
    let head = body.trim_start().as_bytes();
    let head = head[..head.len().min(16)].to_ascii_lowercase();
    if [&b"<!doctype"[..], b"<html", b"<?xml"]
        .iter()
        .any(|p| head.starts_with(p))
    {
        return Err("the server sent a web page instead of the file".into());
    }
    if ext == "json" {
        return match serde_json::from_str::<serde_json::Value>(body) {
            Ok(serde_json::Value::Array(_)) | Ok(serde_json::Value::Object(_)) => Ok(()),
            _ => Err("the file is damaged (not valid JSON)".into()),
        };
    }
    let before = old.map(rule_count).unwrap_or(0);
    let now = rule_count(body);
    if before >= SHRINK_FLOOR && now < before / 4 {
        return Err(format!(
            "the new copy has {now} rules where the last had {before}; kept the last one"
        ));
    }
    Ok(())
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

/// Refuse to buffer a single file larger than this (the biggest lists are a few MB).
const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;

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

    /// Fetch one file if it is due (or `force`). `Ok(true)` when the cached copy changed. A
    /// failure is noted in the file's meta and the cached copy is left as it was.
    fn fetch(
        &self,
        id: &str,
        ext: &str,
        url: &str,
        every: u64,
        force: bool,
    ) -> Result<bool, String> {
        let meta_path = self.meta_path(id);
        let mut meta = Self::read_meta(&meta_path);
        let have = self.file(id, ext).exists();
        if have && !force && now().saturating_sub(meta.checked_at) < every {
            return Ok(false);
        }
        let result = self.download(id, ext, url, &mut meta);
        match &result {
            Ok(_) => {
                meta.ok_at = now();
                meta.error = None;
                meta.error_at = 0;
            }
            Err(e) => {
                meta.error = Some(e.clone());
                meta.error_at = now();
            }
        }
        let _ = fs::write(&meta_path, serde_json::to_string(&meta).unwrap_or_default());
        result
    }

    fn download(&self, id: &str, ext: &str, url: &str, meta: &mut Meta) -> Result<bool, String> {
        let file = self.file(id, ext);
        let have = file.exists();
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
        let status = resp.status().as_u16();
        match status {
            304 => {
                meta.checked_at = now();
                Ok(false)
            }
            200 => {
                let hv = |n: &str| {
                    resp.headers()
                        .get(n)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_owned)
                };
                let (etag, last_modified) = (hv("etag"), hv("last-modified"));
                let body = resp
                    .body_mut()
                    .with_config()
                    .limit(MAX_FILE_BYTES)
                    .read_to_string()
                    .map_err(|e| e.to_string())?;
                if body.trim().is_empty() {
                    return Err("empty response".into());
                }
                let old = fs::read_to_string(&file).ok();
                if old.as_deref() == Some(body.as_str()) {
                    meta.checked_at = now();
                    (meta.etag, meta.last_modified) = (etag, last_modified);
                    return Ok(false);
                }
                // A refused copy keeps the old validators, so the next check fetches it again.
                check_body(ext, &body, old.as_deref())?;
                let tmp = file.with_extension("tmp");
                fs::write(&tmp, body).map_err(|e| e.to_string())?;
                fs::rename(&tmp, &file).map_err(|e| e.to_string())?;
                meta.checked_at = now();
                (meta.etag, meta.last_modified) = (etag, last_modified);
                Ok(true)
            }
            s => Err(format!("HTTP {s}")),
        }
    }

    /// The full source list: uBlock's set, the community lists, and Brave's current defaults.
    pub fn sources(&self, force: bool) -> Vec<ListSource> {
        self.sources_with(force, &[])
    }

    /// Like [`Updater::sources`], plus lists the user added by URL.
    pub fn sources_with(&self, force: bool, custom: &[String]) -> Vec<ListSource> {
        let mut all = vec![sources::shoji_fixes()];
        all.extend(sources::ublock_lists());
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

    /// What the cache holds for a source, how fresh it is and why its last check failed.
    pub fn list_health(&self, id: &str) -> ListHealth {
        let text = fs::read_to_string(self.file(id, "txt")).unwrap_or_default();
        let meta = Self::read_meta(&self.meta_path(id));
        ListHealth {
            bytes: text.len() as u64,
            rules: rule_count(&text),
            fresh_at: if text.is_empty() { 0 } else { meta.fresh_at() },
            error: meta.error,
            error_at: meta.error_at,
        }
    }

    /// How long ago (seconds) the stalest of `ids`, or of the resource bundles, was last
    /// confirmed current. Files never fetched are not counted.
    pub fn oldest_age(&self, ids: &[&str]) -> u64 {
        let now = now();
        ids.iter()
            .copied()
            .chain(["resources", "ubo-resources"])
            .map(|id| Self::read_meta(&self.meta_path(id)).fresh_at())
            .filter(|t| *t > 0)
            .map(|t| now.saturating_sub(t))
            .max()
            .unwrap_or(0)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn list(n: usize) -> String {
        let mut s = String::from("! Title: test\n");
        for i in 0..n {
            s.push_str(&format!("||ads{i}.example^\n"));
        }
        s
    }

    #[test]
    fn error_pages_and_cut_off_lists_never_replace_the_last_good_copy() {
        let page = "\n  <!DOCTYPE html><html><body>Rate limited</body></html>";
        assert!(check_body("txt", page, None).is_err());
        assert!(check_body("txt", "<HTML>", Some(&list(10))).is_err());
        assert!(check_body("json", "<html>", None).is_err());
        assert!(check_body("json", "[{\"name\":\"x\"", None).is_err());
        assert!(check_body("json", "[]", None).is_ok());
        // A list may shrink, but not to a fraction of itself.
        assert!(check_body("txt", &list(700), Some(&list(1000))).is_ok());
        assert!(check_body("txt", &list(200), Some(&list(1000))).is_err());
        // Small lists (the Shoji fix list, custom lists) aren't judged by size.
        assert!(check_body("txt", &list(1), Some(&list(150))).is_ok());
        assert!(check_body("txt", &list(5), None).is_ok());
        // Multi-byte text at the start never trips the sniffing.
        assert!(check_body("txt", "é".repeat(40).as_str(), None).is_ok());
    }

    /// Answer each request with the next of `responses`, then stop.
    fn serve(responses: Vec<(u16, String)>) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            let mut responses = responses.into_iter();
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let Some((status, body)) = responses.next() else {
                    return;
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        addr
    }

    #[test]
    fn failures_are_kept_with_the_list_and_cleared_by_the_next_good_fetch() {
        let dir = std::env::temp_dir().join(format!("byobu-health-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let good = list(400);
        let base = serve(vec![
            (200, good.clone()),
            (503, String::new()),
            (200, "<!doctype html><p>Sign in".into()),
            (200, list(10)),
            (200, list(401)),
        ]);
        let up = Updater::new(&dir);
        let url = format!("{base}/list.txt");
        assert_eq!(up.fetch("l", "txt", &url, 0, true), Ok(true));
        let h = up.list_health("l");
        assert_eq!((h.rules, h.error.is_none()), (400, true));
        assert!(h.fresh_at > 0);

        for want in ["HTTP 503", "web page", "kept the last one"] {
            let err = up.fetch("l", "txt", &url, 0, true).unwrap_err();
            assert!(err.contains(want), "{err}");
            let h = up.list_health("l");
            assert_eq!(h.rules, 400, "the last good copy stays in use");
            assert!(h.error.as_deref().is_some_and(|e| e.contains(want)));
            assert!(h.error_at > 0);
        }
        assert_eq!(up.list_text("l").as_deref(), Some(good.as_str()));

        assert_eq!(up.fetch("l", "txt", &url, 0, true), Ok(true));
        let h = up.list_health("l");
        assert_eq!((h.rules, h.error), (401, None));
        assert!(up.oldest_age(&["l"]) < 5);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn age_falls_back_to_the_last_check_for_older_caches() {
        let dir = std::env::temp_dir().join(format!("byobu-age-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let up = Updater::new(&dir);
        fs::write(up.file("old", "txt"), list(3)).unwrap();
        let week_ago = now() - 8 * 86_400;
        fs::write(
            up.meta_path("old"),
            format!(r#"{{"etag":null,"last_modified":null,"checked_at":{week_ago}}}"#),
        )
        .unwrap();
        assert_eq!(up.list_health("old").fresh_at, week_ago);
        assert!(up.oldest_age(&["old", "never-fetched"]) >= 8 * 86_400);
        let _ = fs::remove_dir_all(&dir);
    }
}
