//! Keeps Shoji itself up to date from the project's GitHub releases.
//!
//! A background thread asks GitHub for the latest release every hour. When it is newer than
//! this build, it downloads `ShojiSetup.exe` next to a published SHA-256 and keeps it only if the
//! checksum matches. The browser then runs it (`--update`, no windows) once no Shoji window is
//! open, which is when it sits in the tray, so a running session is never interrupted.
//!
//! Limits worth knowing: the installer is not code-signed, and the checksum comes from the same
//! release, so it protects against corruption and truncated downloads, not against a
//! compromised GitHub account.

use crate::BrowserEvent;
use std::path::{Path, PathBuf};
use std::time::Duration;
use winit::event_loop::EventLoopProxy;

const REPO_API: &str = "https://api.github.com/repos/KJX469/Shoji/releases/latest";
/// Installers are only ever fetched from this project's release downloads.
const DOWNLOAD_PREFIX: &str = "https://github.com/KJX469/Shoji/releases/download/";
const INSTALLER_ASSET: &str = "ShojiSetup.exe";
const CHECKSUM_ASSET: &str = "ShojiSetup.exe.sha256";
/// How often to look for a release, and how soon to retry after a failure.
const INTERVAL: Duration = Duration::from_secs(3600);
const RETRY: Duration = Duration::from_secs(600);
/// First check shortly after start, so a launch doesn't compete with start-up work.
const FIRST_DELAY: Duration = Duration::from_secs(45);
/// Refuse to buffer anything larger than this.
const MAX_INSTALLER_BYTES: u64 = 96 * 1024 * 1024;

/// A release newer than the running build, with where to fetch it.
#[derive(Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub installer_url: String,
    pub checksum_url: String,
}

/// Is `remote` a higher dotted version than `local`? Unparseable versions never update.
pub fn is_newer(remote: &str, local: &str) -> bool {
    let parse = |v: &str| shoji_core::env::parse_version_tuple(v.trim().trim_start_matches('v'));
    let (Some(mut r), Some(mut l)) = (parse(remote), parse(local)) else {
        return false;
    };
    let n = r.len().max(l.len());
    r.resize(n, 0);
    l.resize(n, 0);
    r > l
}

/// Pull the version and the two asset URLs out of GitHub's "latest release" JSON.
pub fn parse_release(json: &str, prefix: &str) -> Option<Release> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    if v.get("draft").and_then(|d| d.as_bool()).unwrap_or(false)
        || v.get("prerelease")
            .and_then(|d| d.as_bool())
            .unwrap_or(false)
    {
        return None;
    }
    let version = v
        .get("tag_name")?
        .as_str()?
        .trim()
        .trim_start_matches('v')
        .to_string();
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
    Some(Release {
        version,
        installer_url,
        checksum_url,
    })
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
        .timeout_global(Some(Duration::from_secs(120)))
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

fn get_bytes(agent: &ureq::Agent, url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut resp = agent.get(url).call().map_err(|e| e.to_string())?;
    if resp.status().as_u16() != 200 {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    resp.body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|e| e.to_string())
}

/// Where a verified installer for `version` is kept.
fn installer_path(dir: &Path, version: &str) -> PathBuf {
    dir.join(format!("ShojiSetup-{version}.exe"))
}

/// One pass: returns the path of a verified installer newer than this build, if there is one.
fn check_once(dir: &Path, api: &str, prefix: &str) -> Result<Option<(String, PathBuf)>, String> {
    let agent = agent();
    let json = get_text(&agent, api, 2 * 1024 * 1024)?;
    let Some(release) = parse_release(&json, prefix) else {
        return Ok(None);
    };
    if !is_newer(&release.version, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let _ = std::fs::create_dir_all(dir);
    let target = installer_path(dir, &release.version);
    if target.exists() {
        return Ok(Some((release.version, target)));
    }
    let want = parse_checksum(&get_text(&agent, &release.checksum_url, 4096)?)
        .ok_or("the published checksum is malformed")?;
    let bytes = get_bytes(&agent, &release.installer_url, MAX_INSTALLER_BYTES)?;
    if sha256_hex(&bytes) != want {
        return Err("downloaded installer does not match its checksum".into());
    }
    let tmp = target.with_extension("part");
    std::fs::write(&tmp, &bytes).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &target).map_err(|e| e.to_string())?;
    // Keep the folder to one file: older downloads are no longer needed.
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            if e.path() != target {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(Some((release.version, target)))
}

/// Start the hourly check. `enabled` is read from the saved settings on each pass, so the
/// toggle takes effect without a restart.
pub fn spawn_checker(proxy: EventLoopProxy<BrowserEvent>, data_dir: PathBuf) {
    let _ = std::thread::Builder::new()
        .name("shoji-update".into())
        .spawn(move || {
            let dir = data_dir.join("updates");
            let (api, prefix, first) = endpoints();
            std::thread::sleep(first);
            loop {
                let enabled =
                    shoji_core::settings::Settings::load_from_path(&data_dir.join("settings.json"))
                        .performance
                        .auto_update;
                let wait = if enabled {
                    match check_once(&dir, &api, &prefix) {
                        Ok(Some((version, installer))) => {
                            let _ =
                                proxy.send_event(BrowserEvent::UpdateReady { version, installer });
                            INTERVAL
                        }
                        Ok(None) => INTERVAL,
                        Err(_) => RETRY,
                    }
                } else {
                    INTERVAL
                };
                std::thread::sleep(wait);
            }
        });
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
    }

    #[test]
    fn release_json_yields_assets_only_from_our_downloads() {
        let json = |installer: &str| {
            format!(
                r#"{{"tag_name":"v0.1.10","draft":false,"prerelease":false,"assets":[
                  {{"name":"ShojiSetup.exe","browser_download_url":"{installer}"}},
                  {{"name":"ShojiSetup.exe.sha256","browser_download_url":"{DOWNLOAD_PREFIX}v0.1.10/ShojiSetup.exe.sha256"}}]}}"#
            )
        };
        let good = format!("{DOWNLOAD_PREFIX}v0.1.10/ShojiSetup.exe");
        let r = parse_release(&json(&good), DOWNLOAD_PREFIX).expect("release");
        assert_eq!(r.version, "0.1.10");
        assert_eq!(r.installer_url, good);

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
