//! Keeps Shoji itself up to date from the project's GitHub releases, and only with consent.
//!
//! A background thread asks GitHub for the latest release every hour (and whenever Settings asks).
//! A newer release is only announced: the crane brings it as a letter, and nothing is downloaded
//! or installed until the user picks "Restart and update" or "When I close Shoji". Then
//! `ShojiSetup.exe` is fetched next to its published SHA-256 and kept only if the checksum
//! matches. "Restart" saves the open tabs, runs the installer with `--update --reopen` (it asks
//! this copy to quit, swaps the files and starts the new version with the same tabs); "When I
//! close" puts the new files in place with `--stage` straight away and finishes with `--update`
//! once no Shoji window is open.
//!
//! With "Get beta updates" on, pre-releases are offered too (never the `byobu-data` release,
//! which has no installer). If a new version keeps closing just after it starts, the version
//! before it can be put back (see `shoji_core::launches`): its installer is kept from the
//! update, or fetched again from its release.
//!
//! Limits worth knowing: the installer is not code-signed, and the checksum comes from the same
//! release, so it protects against corruption and truncated downloads, not against a
//! compromised GitHub account.

use crate::BrowserEvent;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;
use winit::event_loop::EventLoopProxy;

const REPO_API: &str = "https://api.github.com/repos/KJX469/Shoji/releases/latest";
/// Installers are only ever fetched from this project's release downloads.
const DOWNLOAD_PREFIX: &str = "https://github.com/KJX469/Shoji/releases/download/";
/// The "read the whole letter" link only ever opens this project's release pages.
const PAGE_PREFIX: &str = "https://github.com/KJX469/Shoji/releases/";
const INSTALLER_ASSET: &str = "ShojiSetup.exe";
const CHECKSUM_ASSET: &str = "ShojiSetup.exe.sha256";
/// How often to look for a release, and how soon to retry after a failure.
const INTERVAL: Duration = Duration::from_secs(3600);
const RETRY: Duration = Duration::from_secs(600);
/// First check shortly after start, so a launch doesn't compete with start-up work.
const FIRST_DELAY: Duration = Duration::from_secs(45);
/// Refuse to buffer anything larger than this.
const MAX_INSTALLER_BYTES: u64 = 96 * 1024 * 1024;
/// At most this many lines of the release notes go in the letter.
const NOTE_LINES: usize = 4;

/// A release newer than the running build, with where to fetch it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    pub installer_url: String,
    pub checksum_url: String,
    /// A few short lines from the release notes
    #[serde(default)]
    pub notes: Vec<String>,
    /// The release's page, for the whole letter
    #[serde(default)]
    pub page_url: String,
}

/// Is `remote` a later version than `local` (`0.1.8-beta.1` comes after `0.1.7` and before
/// `0.1.8`)? Unparseable versions never update.
pub fn is_newer(remote: &str, local: &str) -> bool {
    shoji_core::env::is_newer_release(remote, local)
}

/// Pull the version, the two asset URLs and a few lines of notes out of GitHub's
/// "latest release" JSON.
pub fn parse_release(json: &str, prefix: &str) -> Option<Release> {
    release_from(&serde_json::from_str(json).ok()?, prefix, false)
}

/// The newest release in GitHub's release list JSON, pre-releases included when `beta`.
pub fn parse_releases(json: &str, prefix: &str, beta: bool) -> Option<Release> {
    let list: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    list.iter()
        .filter_map(|v| release_from(v, prefix, beta))
        .reduce(|a, b| {
            if is_newer(&b.version, &a.version) {
                b
            } else {
                a
            }
        })
}

fn release_from(v: &serde_json::Value, prefix: &str, beta: bool) -> Option<Release> {
    if v.get("draft").and_then(|d| d.as_bool()).unwrap_or(false)
        || (!beta
            && v.get("prerelease")
                .and_then(|d| d.as_bool())
                .unwrap_or(false))
    {
        return None;
    }
    let version = v
        .get("tag_name")?
        .as_str()?
        .trim()
        .trim_start_matches('v')
        .to_string();
    shoji_core::env::compare_release_versions(&version, "0.0")?;
    let url_of = |name: &str| -> Option<String> {
        v.get("assets")?.as_array()?.iter().find_map(|a| {
            (a.get("name")?.as_str()? == name)
                .then(|| a.get("browser_download_url")?.as_str().map(str::to_owned))
                .flatten()
        })
    };
    let installer_url = url_of(INSTALLER_ASSET)?;
    let checksum_url = url_of(CHECKSUM_ASSET)?;
    if !installer_url.starts_with(prefix) || !checksum_url.starts_with(prefix) {
        return None;
    }
    let notes = v
        .get("body")
        .and_then(|b| b.as_str())
        .map(note_lines)
        .unwrap_or_default();
    let page_url = v
        .get("html_url")
        .and_then(|u| u.as_str())
        .filter(|u| u.starts_with(PAGE_PREFIX))
        .unwrap_or_default()
        .to_string();
    Some(Release {
        version,
        installer_url,
        checksum_url,
        notes,
        page_url,
    })
}

/// The first few changelog bullets, each cut down to its headline: the bold lead-in when there
/// is one ("**Bookmarks, properly.** …" gives "Bookmarks, properly"), else its first sentence.
pub fn note_lines(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in body.lines() {
        let Some(item) = line
            .trim_start()
            .strip_prefix("- ")
            .or_else(|| line.trim_start().strip_prefix("* "))
        else {
            continue;
        };
        let item = item.trim();
        let head = match item.strip_prefix("**").and_then(|r| r.split_once("**")) {
            Some((bold, _)) => bold.to_string(),
            None => item
                .split_once(". ")
                .map(|(first, _)| first)
                .unwrap_or(item)
                .to_string(),
        };
        let clean: String = head.replace('`', "").replace("**", "");
        let clean = clean.trim().trim_end_matches(['.', ':']).trim().to_string();
        if clean.is_empty() {
            continue;
        }
        let clean = if clean.chars().count() > 72 {
            let cut: String = clean.chars().take(70).collect();
            format!("{}…", cut.trim_end())
        } else {
            clean
        };
        out.push(clean);
        if out.len() == NOTE_LINES {
            break;
        }
    }
    out
}

/// The 64-hex-digit digest at the start of a `sha256sum`-style file.
pub fn parse_checksum(text: &str) -> Option<String> {
    let first = text.split_whitespace().next()?.to_ascii_lowercase();
    (first.len() == 64 && first.bytes().all(|b| b.is_ascii_hexdigit())).then_some(first)
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    digest.as_ref().iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(300)))
        .user_agent(concat!("Shoji/", env!("CARGO_PKG_VERSION")))
        .http_status_as_error(false)
        .build()
        .into()
}

fn get_text(agent: &ureq::Agent, url: &str, limit: u64) -> Result<String, String> {
    let mut resp = agent
        .get(url)
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?;
    if resp.status().as_u16() != 200 {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    resp.body_mut()
        .with_config()
        .limit(limit)
        .read_to_string()
        .map_err(|e| e.to_string())
}

/// Download `url`, reporting progress (0..=100) as it goes.
fn get_bytes(
    agent: &ureq::Agent,
    url: &str,
    limit: u64,
    progress: &dyn Fn(u32),
) -> Result<Vec<u8>, String> {
    let mut resp = agent.get(url).call().map_err(|e| e.to_string())?;
    if resp.status().as_u16() != 200 {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let total = resp.body().content_length().unwrap_or(0);
    let mut reader = resp.body_mut().with_config().limit(limit).reader();
    let mut out = Vec::with_capacity(total.min(limit) as usize);
    let mut buf = vec![0u8; 64 * 1024];
    let mut last = u32::MAX;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if let Some(pct) = (out.len() as u64 * 100).checked_div(total) {
            let pct = pct.min(100) as u32;
            if pct != last && (pct.is_multiple_of(4) || pct == 100) {
                last = pct;
                progress(pct);
            }
        }
    }
    Ok(out)
}

/// Where a verified installer for `version` is kept.
fn installer_path(dir: &Path, version: &str) -> PathBuf {
    dir.join(format!("ShojiSetup-{version}.exe"))
}

/// Ask GitHub for the latest release (`api` is the "latest release" address; betas come from
/// the release list beside it): `Some` when it is newer than this build.
fn check_latest(api: &str, prefix: &str, beta: bool) -> Result<Option<Release>, String> {
    let found = if beta {
        let list = format!("{}?per_page=20", api.strip_suffix("/latest").unwrap_or(api));
        parse_releases(&get_text(&agent(), &list, 4 * 1024 * 1024)?, prefix, true)
    } else {
        parse_release(&get_text(&agent(), api, 2 * 1024 * 1024)?, prefix)
    };
    Ok(found.filter(|r| is_newer(&r.version, env!("CARGO_PKG_VERSION"))))
}

/// The published release of `version`, at the address every release's assets have.
pub fn release_of(version: &str) -> Release {
    let base = format!("{DOWNLOAD_PREFIX}v{version}/");
    Release {
        version: version.to_string(),
        installer_url: format!("{base}{INSTALLER_ASSET}"),
        checksum_url: format!("{base}{CHECKSUM_ASSET}"),
        notes: Vec::new(),
        page_url: format!("{PAGE_PREFIX}tag/v{version}"),
    }
}

/// Fetch and verify the installer for `release` (or reuse one fetched before).
fn download(dir: &Path, release: &Release, progress: &dyn Fn(u32)) -> Result<PathBuf, String> {
    let _ = std::fs::create_dir_all(dir);
    let target = installer_path(dir, &release.version);
    if target.exists() {
        return Ok(target);
    }
    let agent = agent();
    let want = parse_checksum(&get_text(&agent, &release.checksum_url, 4096)?)
        .ok_or("the published checksum is malformed")?;
    let bytes = get_bytes(
        &agent,
        &release.installer_url,
        MAX_INSTALLER_BYTES,
        progress,
    )?;
    if sha256_hex(&bytes) != want {
        return Err("the download doesn't match its published checksum".into());
    }
    let tmp = target.with_extension("part");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &target).map_err(|e| e.to_string())?;
    // Keep the new installer and the running version's (to go back to if the new one fails);
    // older downloads are no longer needed.
    let running = installer_path(dir, env!("CARGO_PKG_VERSION"));
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.path() != target && e.path() != running {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(target)
}

/// Is this the copy the installer manages (`%LOCALAPPDATA%\Programs\Shoji`)? A portable or
/// development copy never starts the installer, which would swap a different program in.
pub fn running_installed_copy() -> bool {
    let (Ok(exe), Some(local)) = (std::env::current_exe(), std::env::var_os("LOCALAPPDATA")) else {
        return false;
    };
    let dir = PathBuf::from(local)
        .join("Programs")
        .join(shoji_core::APP_NAME);
    exe.parent().is_some_and(|p| {
        p.to_string_lossy()
            .eq_ignore_ascii_case(&dir.to_string_lossy())
    })
}

/// Delete what an in-place update moved aside (the previous version's executable), now that
/// nothing runs from it.
pub fn sweep_old_files() {
    if !running_installed_copy() {
        return;
    }
    let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    else {
        return;
    };
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().ends_with(".old") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// What the browser asks of the update thread.
pub enum Request {
    /// Look now. `manual` when Settings asked, so the answer is shown even when there is none.
    Check { manual: bool },
    /// The user said yes: fetch and verify this release's installer.
    Download(Release),
    /// Go back to this earlier release: its installer, kept or fetched again.
    Rollback(Release),
}

/// Start the update thread: an hourly check (while `performance.auto_update` is on, read from
/// the saved settings on each pass so the switch takes effect at once), plus whatever the
/// browser asks for through the returned sender.
pub fn spawn_checker(
    proxy: EventLoopProxy<BrowserEvent>,
    data_dir: PathBuf,
) -> mpsc::Sender<Request> {
    let (tx, rx) = mpsc::channel::<Request>();
    let _ = std::thread::Builder::new()
        .name("shoji-update".into())
        .spawn(move || {
            let dir = data_dir.join("updates");
            let (api, prefix, first) = endpoints();
            let mut wait = first;
            loop {
                let request = match rx.recv_timeout(wait) {
                    Ok(r) => r,
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let enabled = shoji_core::settings::Settings::load_from_path(
                            &data_dir.join("settings.json"),
                        )
                        .performance
                        .auto_update;
                        if !enabled {
                            wait = INTERVAL;
                            continue;
                        }
                        Request::Check { manual: false }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                };
                match request {
                    Request::Check { manual } => {
                        let beta = shoji_core::settings::Settings::load_from_path(
                            &data_dir.join("settings.json"),
                        )
                        .performance
                        .beta_updates;
                        let result = check_latest(&api, &prefix, beta);
                        wait = if result.is_ok() { INTERVAL } else { RETRY };
                        let _ = proxy.send_event(BrowserEvent::UpdateChecked { result, manual });
                    }
                    Request::Download(release) => {
                        let version = release.version.clone();
                        let report = |pct| {
                            let _ = proxy.send_event(BrowserEvent::UpdateProgress { pct });
                        };
                        let result = download(&dir, &release, &report);
                        let _ =
                            proxy.send_event(BrowserEvent::UpdateDownloaded { version, result });
                    }
                    Request::Rollback(release) => {
                        let result = download(&dir, &release, &|_| {});
                        let _ = proxy.send_event(BrowserEvent::RollbackReady {
                            version: release.version,
                            result,
                        });
                    }
                }
            }
        });
    tx
}

#[cfg(not(debug_assertions))]
fn endpoints() -> (String, String, Duration) {
    (REPO_API.into(), DOWNLOAD_PREFIX.into(), FIRST_DELAY)
}

/// Debug builds can be pointed at a local server for testing; release builds cannot.
#[cfg(debug_assertions)]
fn endpoints() -> (String, String, Duration) {
    let api = std::env::var("SHOJI_UPDATE_API").unwrap_or_else(|_| REPO_API.into());
    let prefix = std::env::var("SHOJI_UPDATE_PREFIX").unwrap_or_else(|_| DOWNLOAD_PREFIX.into());
    let first = std::env::var("SHOJI_UPDATE_DELAY_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(FIRST_DELAY);
    (api, prefix, first)
}

/* ----- the letter ----- */

/// What the user told the letter, kept in `update-letter.json` so a restart doesn't ask again.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Memo {
    /// "Skip this version": no letter for it (a newer one still comes)
    pub skip: String,
    /// "Later": the card doesn't open by itself again before this time (ms since the epoch)
    pub later_until: u64,
    /// The version whose card last opened by itself, and when
    pub unfolded: String,
    pub unfolded_at: u64,
    /// The release being installed, so the new version can say what it brought
    pub installing: Option<Release>,
}

/// A card opens by itself at most once a day for the same version.
const UNFOLD_AGAIN_MS: u64 = 20 * 3600 * 1000;
/// "Later" means tomorrow.
pub const LATER_MS: u64 = 20 * 3600 * 1000;

impl Memo {
    pub fn load(path: &Path) -> Self {
        shoji_core::userdata::read_text(path)
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) {
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = shoji_core::userdata::write_atomic(path, &json);
        }
    }

    /// Should the toolbar letter show for `version`?
    pub fn wants_letter(&self, version: &str) -> bool {
        self.skip != version
    }

    /// May the card open by itself for `version` now? Never for a skipped version, not before
    /// "Later" runs out, and not twice in a day.
    pub fn may_unfold(&self, version: &str, now: u64) -> bool {
        self.wants_letter(version)
            && now >= self.later_until
            && !(self.unfolded == version && now < self.unfolded_at + UNFOLD_AGAIN_MS)
    }
}

/// Where the update is at, as the letter and Settings show it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    Downloading,
    /// Verified installer on disk
    Ready,
}

/// When to install, once the user has agreed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Consent {
    #[default]
    None,
    /// Restart as soon as the installer is ready
    Now,
    /// Install once no window is open
    OnClose,
}

/// The browser's whole view of updating.
#[derive(Debug, Default)]
pub struct Status {
    pub phase: Phase,
    pub consent: Consent,
    /// The newer release, once one was found
    pub release: Option<Release>,
    pub installer: Option<PathBuf>,
    pub pct: u32,
    /// When GitHub last answered (ms since the epoch)
    pub checked_at: u64,
    pub error: Option<String>,
    /// Settings asked; its answer should be shown even when there is nothing new
    pub manual: bool,
}

impl Status {
    /// JSON for the letter in the toolbar and the updates row in Settings.
    pub fn json(&self, memo: &Memo, installed_copy: bool) -> serde_json::Value {
        let phase = match self.phase {
            Phase::Idle => "idle",
            Phase::Checking => "checking",
            Phase::Downloading => "downloading",
            Phase::Ready => "ready",
        };
        let consent = match self.consent {
            Consent::None => "",
            Consent::Now => "now",
            Consent::OnClose => "close",
        };
        let r = self.release.as_ref();
        serde_json::json!({
            "current": env!("CARGO_PKG_VERSION"),
            "version": r.map(|r| r.version.as_str()).unwrap_or(""),
            "notes": r.map(|r| r.notes.clone()).unwrap_or_default(),
            "page": r.is_some_and(|r| !r.page_url.is_empty()),
            "letter": r.is_some_and(|r| memo.wants_letter(&r.version)),
            "skipped": r.is_some_and(|r| !memo.wants_letter(&r.version)),
            "phase": phase,
            "consent": consent,
            "pct": self.pct,
            "checked": self.checked_at,
            "error": self.error,
            "manual": self.manual,
            "installable": installed_copy,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("v0.1.10", "0.1.9"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(
            is_newer("0.10.0", "0.9.0"),
            "10 is greater than 9, not less"
        );
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.1.1"));
        assert!(!is_newer("1.0", "1.0.0"));
        assert!(!is_newer("nightly", "0.1.0"));
        assert!(is_newer("0.1.8-beta.1", "0.1.7"));
        assert!(!is_newer("0.1.8-beta.1", "0.1.8"));
    }

    #[test]
    fn betas_are_offered_only_when_asked_for() {
        let rel = |tag: &str, pre: bool| {
            format!(
                r#"{{"tag_name":"{tag}","draft":false,"prerelease":{pre},"assets":[
                  {{"name":"ShojiSetup.exe","browser_download_url":"{DOWNLOAD_PREFIX}{tag}/ShojiSetup.exe"}},
                  {{"name":"ShojiSetup.exe.sha256","browser_download_url":"{DOWNLOAD_PREFIX}{tag}/ShojiSetup.exe.sha256"}}]}}"#
            )
        };
        // The data release is a pre-release with no installer; it never counts.
        let data = r#"{"tag_name":"byobu-data","prerelease":true,"assets":[]}"#;
        let list = format!(
            "[{data},{},{},{}]",
            rel("v0.1.8-beta.2", true),
            rel("v0.1.7", false),
            rel("v0.1.8-beta.1", true)
        );
        let pick = |beta| parse_releases(&list, DOWNLOAD_PREFIX, beta).map(|r| r.version);
        assert_eq!(pick(true).as_deref(), Some("0.1.8-beta.2"));
        assert_eq!(pick(false).as_deref(), Some("0.1.7"));
        // A stable release newer than every beta wins either way.
        let list = format!("[{},{}]", rel("v0.1.8-beta.2", true), rel("v0.1.8", false));
        assert_eq!(
            parse_releases(&list, DOWNLOAD_PREFIX, true).map(|r| r.version),
            Some("0.1.8".into())
        );
        assert!(parse_releases("{}", DOWNLOAD_PREFIX, true).is_none());
        // A tag that isn't a version is never offered.
        assert!(parse_release(&rel("nightly", false), DOWNLOAD_PREFIX).is_none());
    }

    #[test]
    fn an_earlier_release_is_fetched_from_its_own_tag() {
        let r = release_of("0.1.6");
        assert_eq!(
            r.installer_url,
            "https://github.com/KJX469/Shoji/releases/download/v0.1.6/ShojiSetup.exe"
        );
        assert!(r.checksum_url.ends_with("/v0.1.6/ShojiSetup.exe.sha256"));
        assert!(r.page_url.starts_with(PAGE_PREFIX));
    }

    #[test]
    fn release_json_yields_assets_only_from_our_downloads() {
        let json = |installer: &str| {
            format!(
                r##"{{"tag_name":"v0.1.10","draft":false,"prerelease":false,
                  "html_url":"https://github.com/KJX469/Shoji/releases/tag/v0.1.10",
                  "body":"Added\n- **Bookmarks, properly.** Long text.\n- A plain line. More.\n","assets":[
                  {{"name":"ShojiSetup.exe","browser_download_url":"{installer}"}},
                  {{"name":"ShojiSetup.exe.sha256","browser_download_url":"{DOWNLOAD_PREFIX}v0.1.10/ShojiSetup.exe.sha256"}}]}}"##
            )
        };
        let good = format!("{DOWNLOAD_PREFIX}v0.1.10/ShojiSetup.exe");
        let r = parse_release(&json(&good), DOWNLOAD_PREFIX).expect("release");
        assert_eq!(r.version, "0.1.10");
        assert_eq!(r.installer_url, good);
        assert_eq!(r.notes, vec!["Bookmarks, properly", "A plain line"]);
        assert!(r.page_url.starts_with(PAGE_PREFIX));

        // An asset hosted anywhere else is never fetched.
        assert!(parse_release(
            &json("https://evil.example/ShojiSetup.exe"),
            DOWNLOAD_PREFIX
        )
        .is_none());
        // Prereleases (like the byobu-data release) and drafts are ignored.
        let pre = json(&good).replace(r#""prerelease":false"#, r#""prerelease":true"#);
        assert!(parse_release(&pre, DOWNLOAD_PREFIX).is_none());
        // A release without a checksum is not installed.
        let no_sum = format!(
            r#"{{"tag_name":"v0.1.10","assets":[{{"name":"ShojiSetup.exe","browser_download_url":"{good}"}}]}}"#
        );
        assert!(parse_release(&no_sum, DOWNLOAD_PREFIX).is_none());
        // A page link anywhere else is dropped.
        let elsewhere = json(&good).replace(
            "https://github.com/KJX469/Shoji/releases/tag/v0.1.10",
            "https://evil.example/",
        );
        assert_eq!(
            parse_release(&elsewhere, DOWNLOAD_PREFIX).unwrap().page_url,
            ""
        );
    }

    #[test]
    fn notes_are_short_headlines() {
        let body = "### Added\n- **Ctrl+Shift+D** bookmarks every open tab.\n* `code` in a line. Then more\n\
                    - **A**\n- x\n- five\n- six\n";
        let lines = note_lines(body);
        assert_eq!(lines, vec!["Ctrl+Shift+D", "code in a line", "A", "x"]);
        let long = format!("- {}", "word ".repeat(30));
        assert!(note_lines(&long)[0].chars().count() <= 71);
        assert!(note_lines("no bullets here").is_empty());
    }

    #[test]
    fn the_letter_respects_later_skip_and_once_a_day() {
        let mut m = Memo::default();
        assert!(m.may_unfold("0.2.0", 1000));
        m.unfolded = "0.2.0".into();
        m.unfolded_at = 1000;
        assert!(!m.may_unfold("0.2.0", 2000), "not twice in a day");
        assert!(m.may_unfold("0.2.0", 1000 + UNFOLD_AGAIN_MS));
        assert!(m.may_unfold("0.2.1", 2000), "a newer version may");
        m.later_until = 5000;
        assert!(!m.may_unfold("0.2.1", 4000));
        m.skip = "0.2.1".into();
        assert!(!m.wants_letter("0.2.1"));
        assert!(!m.may_unfold("0.2.1", 1 << 50));
        assert!(m.wants_letter("0.2.2"));
    }

    /// Serve `routes` (path -> body) over plain HTTP on a local port until dropped.
    fn serve(routes: Vec<(&'static str, Vec<u8>)>) -> String {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut stream = stream;
                let mut buf = [0u8; 2048];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                let body = routes
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, b)| b.clone());
                let (status, body) = match body {
                    Some(b) => ("200 OK", b),
                    None => ("404 Not Found", Vec::new()),
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(&body);
            }
        });
        addr
    }

    #[test]
    fn download_keeps_only_a_matching_installer_and_reports_progress() {
        let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let good = format!("{}  ShojiSetup.exe\n", sha256_hex(&payload));
        let bad = format!("{}  ShojiSetup.exe\n", sha256_hex(b"something else"));
        let base = serve(vec![
            ("/setup.exe", payload.clone()),
            ("/good.sha256", good.into_bytes()),
            ("/bad.sha256", bad.into_bytes()),
        ]);
        let dir = std::env::temp_dir().join(format!("shoji-upd-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let release = |sum: &str, version: &str| Release {
            version: version.into(),
            installer_url: format!("{base}/setup.exe"),
            checksum_url: format!("{base}/{sum}"),
            notes: vec![],
            page_url: String::new(),
        };
        // A checksum that doesn't match leaves nothing behind.
        let err = download(&dir, &release("bad.sha256", "9.9.8"), &|_| {}).unwrap_err();
        assert!(err.contains("checksum"), "{err}");
        assert!(!installer_path(&dir, "9.9.8").exists());
        // A matching one is kept, with progress up to 100 on the way.
        let seen = std::sync::Mutex::new(Vec::new());
        let path = download(&dir, &release("good.sha256", "9.9.9"), &|p| {
            seen.lock().unwrap().push(p)
        })
        .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), payload);
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.last(), Some(&100), "{seen:?}");
        assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");
        // Asked again, it reuses the verified file without fetching.
        let again = download(&dir, &release("missing.sha256", "9.9.9"), &|_| {}).unwrap();
        assert_eq!(again, path);
        // A newer download keeps the running version's installer (to go back to), not others.
        let running = installer_path(&dir, env!("CARGO_PKG_VERSION"));
        std::fs::write(&running, b"running").unwrap();
        download(&dir, &release("good.sha256", "9.9.10"), &|_| {}).unwrap();
        assert!(running.exists());
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn checksum_file_and_digest() {
        // sha256("abc") from FIPS 180-2
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
        assert_eq!(sha256_hex(b"abc"), abc);
        assert_eq!(
            parse_checksum(&format!("{}  ShojiSetup.exe\n", abc.to_uppercase())).as_deref(),
            Some(abc)
        );
        assert_eq!(parse_checksum("not a hash"), None);
        assert_eq!(parse_checksum(&abc[..63]), None);
    }
}
