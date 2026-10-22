//! Installing an extension from the Chrome Web Store. The engine only loads unpacked extension
//! folders, so the store's `.crx` package is downloaded, its zip payload is unpacked into the
//! data folder, and the folder is handed to the engine like any other.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Largest package accepted.
const MAX_CRX_BYTES: u64 = 80 * 1024 * 1024;
/// Largest total size of the unpacked files.
const MAX_UNPACKED_BYTES: u64 = 300 * 1024 * 1024;
const MAX_FILES: usize = 20_000;

/// The 32-letter extension id in a store link (or the id on its own).
pub fn parse_id(input: &str) -> Option<String> {
    input
        .split(|c: char| !c.is_ascii_alphanumeric())
        .find(|t| t.len() == 32 && t.bytes().all(|b| (b'a'..=b'p').contains(&b)))
        .map(str::to_owned)
}

/// The extension id of a Chrome Web Store detail page address, `None` for any other address.
pub fn store_page_id(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let (host, path) = rest.split_once('/')?;
    let path = path.split(['?', '#']).next()?;
    let detail = match host {
        "chromewebstore.google.com" => path.starts_with("detail/"),
        "chrome.google.com" => path.starts_with("webstore/detail/"),
        _ => false,
    };
    detail.then(|| parse_id(path)).flatten()
}

/// The zip archive inside a CRX package (versions 2 and 3).
pub fn crx_payload(bytes: &[u8]) -> Option<&[u8]> {
    if bytes.get(..4)? != b"Cr24" {
        return None;
    }
    let word = |at: usize| -> Option<usize> {
        Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize)
    };
    let start = match word(4)? {
        2 => 16usize.checked_add(word(8)?)?.checked_add(word(12)?)?,
        3 => 12usize.checked_add(word(8)?)?,
        _ => return None,
    };
    bytes.get(start..)
}

/// Used when the engine's version can't be read: the store withholds extensions that need a
/// newer Chrome than the one claimed (1Password asks for 140).
const FALLBACK_VERSION: &str = "140.0.0.0";

/// The engine version as the store wants it: four dotted numbers.
fn clean_version(raw: &str) -> Option<String> {
    let v = raw.split_whitespace().next()?;
    let parts: Vec<&str> = v.split('.').collect();
    (parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())))
    .then(|| v.to_string())
}

/// The installed WebView2 runtime's version, which is the Chrome the store should assume.
fn product_version() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        use webview2_com::Microsoft::Web::WebView2::Win32::GetAvailableCoreWebView2BrowserVersionString;
        let mut out = windows::core::PWSTR::null();
        // SAFETY: the loader fills `out` with a CoTaskMem string, copied and freed here.
        let raw = unsafe {
            GetAvailableCoreWebView2BrowserVersionString(windows::core::PCWSTR::null(), &mut out)
                .ok()
                .and_then(|()| out.to_string().ok())
                .inspect(|_| windows::Win32::System::Com::CoTaskMemFree(Some(out.0 as _)))
        };
        raw.and_then(|r| clean_version(&r))
            .unwrap_or_else(|| FALLBACK_VERSION.to_string())
    })
}

fn download(id: &str) -> Result<Vec<u8>, String> {
    let url = format!(
        "https://clients2.google.com/service/update2/crx?response=redirect&prodversion={}&acceptformat=crx2,crx3&x=id%3D{id}%26installsource%3Dondemand%26uc",
        product_version()
    );
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(120)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut resp = agent.get(&url).call().map_err(|e| e.to_string())?;
    match resp.status().as_u16() {
        200 => {}
        204 | 404 => return Err("The Chrome Web Store has no extension with that id.".into()),
        code => return Err(format!("The Chrome Web Store answered {code}.")),
    }
    // Google's download host closes the connection without a TLS close_notify once the whole
    // file is sent, which the stream reports as an error. A package that is complete anyway
    // (its zip index is at the end, so a cut one never parses) is taken as it is.
    let mut bytes = Vec::new();
    let mut reader = resp.body_mut().with_config().limit(MAX_CRX_BYTES).reader();
    match std::io::Read::read_to_end(&mut reader, &mut bytes) {
        Ok(_) => Ok(bytes),
        Err(_) if is_complete_package(&bytes) => Ok(bytes),
        Err(e) => Err(e.to_string()),
    }
}

/// `bytes` is a whole CRX package: a header and a zip that opens.
fn is_complete_package(bytes: &[u8]) -> bool {
    crx_payload(bytes).is_some_and(|zip| zip::ZipArchive::new(std::io::Cursor::new(zip)).is_ok())
}

/// Unpack `zip_bytes` into `dest`, replacing what is there. Entries that would land outside
/// `dest` are skipped.
pub fn unpack(zip_bytes: &[u8], dest: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes))
        .map_err(|_| "That package isn't a valid extension.".to_string())?;
    if archive.len() > MAX_FILES {
        return Err("That extension has too many files.".into());
    }
    let tmp = dest.with_extension("part");
    let _ = std::fs::remove_dir_all(&tmp);
    // A damaged package leaves nothing behind: the folder in use is only replaced at the end.
    if let Err(e) = unpack_into(&mut archive, &tmp) {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(e);
    }
    let _ = std::fs::remove_dir_all(dest);
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())
}

fn unpack_into(
    archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    tmp: &Path,
) -> Result<(), String> {
    std::fs::create_dir_all(tmp).map_err(|e| e.to_string())?;
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| e.to_string())?;
        let Some(rel) = file.enclosed_name() else {
            continue;
        };
        let out = tmp.join(rel);
        if file.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            continue;
        }
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let mut data = Vec::new();
        (&mut file)
            .take(MAX_UNPACKED_BYTES - total + 1)
            .read_to_end(&mut data)
            .map_err(|e| e.to_string())?;
        total += data.len() as u64;
        if total > MAX_UNPACKED_BYTES {
            return Err("That extension is too large.".into());
        }
        std::fs::write(&out, data).map_err(|e| e.to_string())?;
    }
    if !tmp.join("manifest.json").is_file() {
        return Err("That package has no manifest.json.".into());
    }
    Ok(())
}

/// Download the store's extension `id` and unpack it under `extensions_dir`. Blocking.
pub fn fetch(id: &str, extensions_dir: &Path) -> Result<PathBuf, String> {
    let bytes = download(id)?;
    let zip = crx_payload(&bytes).ok_or("The download wasn't an extension package.")?;
    let dest = extensions_dir.join(id);
    let existed = dest.exists();
    std::fs::create_dir_all(extensions_dir).map_err(|e| e.to_string())?;
    unpack(zip, &dest)?;
    if let Some(why) = unsupported(&dest) {
        if !existed {
            let _ = std::fs::remove_dir_all(&dest);
        }
        return Err(why);
    }
    Ok(dest)
}

/// Why the engine can't run the unpacked extension in `dir`, if it can't: it only runs
/// Manifest V3, and the store still lists older ones.
fn unsupported(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("manifest.json")).ok()?;
    let manifest: serde_json::Value =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    let version = manifest.get("manifest_version")?.as_u64()?;
    (version < 3).then(|| {
        "This extension is built for an older Chrome standard (Manifest V2), which browsers no longer run. Look for its newer version in the store."
            .to_string()
    })
}

/// What a manifest calls the extension: its name, looked up in `_locales` when it is a
/// `__MSG_key__` placeholder.
fn display_name(dir: &Path, manifest: &serde_json::Value) -> Option<String> {
    let name = manifest.get("name")?.as_str()?;
    let Some(key) = name
        .strip_prefix("__MSG_")
        .and_then(|k| k.strip_suffix("__"))
    else {
        return Some(name.to_string());
    };
    let locale = manifest
        .get("default_locale")
        .and_then(|l| l.as_str())
        .unwrap_or("en");
    let text =
        std::fs::read_to_string(dir.join("_locales").join(locale).join("messages.json")).ok()?;
    let messages: serde_json::Value =
        serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
    messages
        .as_object()?
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(key))?
        .1
        .get("message")?
        .as_str()
        .map(str::to_owned)
}

/// `a` is a newer extension version than `b` (dotted numbers; a missing part is 0).
pub fn newer(a: &str, b: &str) -> bool {
    let parts = |v: &str| -> Vec<u64> {
        v.split('.')
            .map(|p| p.trim().parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parts(a), parts(b));
    (0..a.len().max(b.len())).find_map(|i| {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        (x != y).then_some(x > y)
    }) == Some(true)
}

/// The store's update check for `installed` (id, version) pairs: one request answers for all.
pub fn update_check_url(installed: &[(String, String)]) -> String {
    let mut x = String::new();
    for (id, version) in installed {
        if !x.is_empty() {
            x.push_str("%26x%3D");
        }
        x.push_str(&format!("id%3D{id}%26v%3D{version}%26uc"));
    }
    format!(
        "https://clients2.google.com/service/update2/crx?response=updatecheck&prodversion={}&acceptformat=crx2,crx3&x={x}",
        product_version()
    )
}

/// (id, version) of each `<app>` the update answer says has a newer version than the one
/// installed.
pub fn updates_in(xml: &str, installed: &[(String, String)]) -> Vec<(String, String)> {
    let attr = |tag: &str, name: &str| -> Option<String> {
        let at = tag.find(&format!(" {name}=\""))? + name.len() + 3;
        let end = tag[at..].find('"')?;
        Some(tag[at..at + end].to_string())
    };
    let mut out = Vec::new();
    let mut app: Option<String> = None;
    for tag in xml.split('<').skip(1) {
        let tag = tag.split('>').next().unwrap_or("");
        if tag.starts_with("app ") {
            app = attr(tag, "appid");
        } else if tag.starts_with("updatecheck ") {
            let (Some(id), Some(version)) = (app.take(), attr(tag, "version")) else {
                continue;
            };
            let current = installed.iter().find(|(i, _)| *i == id).map(|(_, v)| v);
            if attr(tag, "status").as_deref() == Some("ok")
                && current.is_some_and(|c| newer(&version, c))
            {
                out.push((id, version));
            }
        }
    }
    out
}

/// (store id, version) of each store-installed extension under `extensions_dir` (those unpacked
/// into a folder named by their id).
pub fn installed_versions(extensions_dir: &Path) -> Vec<(String, String)> {
    let Ok(entries) = std::fs::read_dir(extensions_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            parse_id(&id).filter(|i| *i == id)?;
            let text = std::fs::read_to_string(e.path().join("manifest.json")).ok()?;
            let m: serde_json::Value =
                serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()?;
            Some((id, m.get("version")?.as_str()?.to_string()))
        })
        .collect()
}

/// Ask the store which installed extensions have a newer version and download them over the old
/// folders. Returns the ids updated. Blocking.
pub fn update_all(extensions_dir: &Path) -> Result<Vec<String>, String> {
    let installed = installed_versions(extensions_dir);
    if installed.is_empty() {
        return Ok(Vec::new());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut resp = agent
        .get(&update_check_url(&installed))
        .call()
        .map_err(|e| e.to_string())?;
    if resp.status().as_u16() != 200 {
        return Err(format!("The Chrome Web Store answered {}.", resp.status()));
    }
    let xml = resp
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let mut done = Vec::new();
    for (id, _) in updates_in(&xml, &installed) {
        if fetch(&id, extensions_dir).is_ok() {
            done.push(id);
        }
    }
    Ok(done)
}

/// What an installed extension offers the browser's own UI.
#[derive(Debug, Default, PartialEq)]
pub struct ExtensionPages {
    /// The folder it was loaded from (the store id for store installs)
    pub folder: String,
    pub name: String,
    /// Paths inside the extension
    pub popup: Option<String>,
    pub options: Option<String>,
    /// Its toolbar icon as a `data:` URL
    pub icon: Option<String>,
}

/// The toolbar icon a manifest names: `action.default_icon` (a path, or paths by size), then
/// the extension's own `icons`. The size nearest 32 px wins (16 px at 200%).
fn icon_path(m: &serde_json::Value) -> Option<String> {
    let pick = |v: &serde_json::Value| -> Option<String> {
        if let Some(p) = v.as_str() {
            return Some(p.to_string());
        }
        v.as_object()?
            .iter()
            .filter_map(|(size, p)| Some((size.parse::<i64>().ok()?, p.as_str()?)))
            .min_by_key(|(size, _)| ((size - 32).abs(), -size))
            .map(|(_, p)| p.to_string())
    };
    [
        "/action/default_icon",
        "/browser_action/default_icon",
        "/icons",
    ]
    .iter()
    .find_map(|k| m.pointer(k).and_then(pick))
}

/// `path` inside `dir` as a `data:` URL, if it is a small image that stays inside `dir`.
fn icon_data(dir: &Path, path: &str) -> Option<String> {
    let rel = Path::new(path.trim_start_matches('/'));
    if rel
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return None;
    }
    let mime = match rel.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        _ => return None,
    };
    let bytes = std::fs::read(dir.join(rel)).ok()?;
    if bytes.len() > 256 * 1024 {
        return None;
    }
    use base64::Engine as _;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// What each extension folder under `extensions_dir` offers.
pub fn pages(extensions_dir: &Path) -> Vec<ExtensionPages> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(extensions_dir) else {
        return out;
    };
    for e in entries.flatten() {
        let dir = e.path();
        let Ok(text) = std::fs::read_to_string(dir.join("manifest.json")) else {
            continue;
        };
        let Ok(m) = serde_json::from_str::<serde_json::Value>(text.trim_start_matches('\u{feff}'))
        else {
            continue;
        };
        let Some(name) = display_name(&dir, &m) else {
            continue;
        };
        let path = |v: Option<&serde_json::Value>| {
            v.and_then(|p| p.as_str())
                .map(|p| p.trim_start_matches('/').to_string())
                .filter(|p| !p.is_empty())
        };
        let popup = path(
            m.pointer("/action/default_popup")
                .or_else(|| m.pointer("/browser_action/default_popup")),
        );
        let options = path(
            m.get("options_page")
                .or_else(|| m.pointer("/options_ui/page")),
        );
        let icon = icon_path(&m).and_then(|p| icon_data(&dir, &p));
        out.push(ExtensionPages {
            folder: e.file_name().to_string_lossy().into_owned(),
            name,
            popup,
            options,
            icon,
        });
    }
    out
}

/// The entry in `pages` for the engine's extension `id` called `name`: by folder first (store
/// installs are unpacked under their id), then by name.
pub fn find<'a>(pages: &'a [ExtensionPages], id: &str, name: &str) -> Option<&'a ExtensionPages> {
    pages
        .iter()
        .find(|p| p.folder == id)
        .or_else(|| pages.iter().find(|p| p.name == name))
}

/// The catalog the store page browses, built weekly into the Byobu data release.
const CATALOG_URL: &str =
    "https://github.com/KJX469/Shoji/releases/download/byobu-data/extension-catalog.json";
const CATALOG_MAX_BYTES: u64 = 16 * 1024 * 1024;
const CATALOG_MAX_AGE: Duration = Duration::from_secs(3 * 24 * 60 * 60);

fn catalog_file(data_dir: &Path) -> PathBuf {
    data_dir.join("extension-catalog.json")
}

/// The catalog's entries that are well formed, as JSON. Icons must be embedded images so the
/// store page never makes a request of its own.
fn clean_catalog(json: &str) -> Option<String> {
    let mut list: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    list.retain_mut(|e| {
        let ok = e["id"].as_str().is_some_and(|id| parse_id(id).is_some())
            && e["name"].as_str().is_some_and(|n| !n.is_empty());
        if ok
            && !e["icon"]
                .as_str()
                .is_some_and(|i| i.starts_with("data:image/"))
        {
            e["icon"] = serde_json::Value::String(String::new());
        }
        ok
    });
    (!list.is_empty())
        .then(|| serde_json::to_string(&list).ok())
        .flatten()
}

/// The downloaded catalog, if there is one.
pub fn read_catalog(data_dir: &Path) -> Option<String> {
    clean_catalog(&std::fs::read_to_string(catalog_file(data_dir)).ok()?)
}

/// Download the catalog unless the one on disk is recent. `Ok(true)` when it was replaced.
/// Blocking.
pub fn refresh_catalog(data_dir: &Path) -> Result<bool, String> {
    let file = catalog_file(data_dir);
    let fresh = std::fs::metadata(&file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < CATALOG_MAX_AGE);
    if fresh {
        return Ok(false);
    }
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .into();
    let mut resp = agent.get(CATALOG_URL).call().map_err(|e| e.to_string())?;
    if resp.status().as_u16() != 200 {
        return Err(format!("The catalog download answered {}.", resp.status()));
    }
    let body = resp
        .body_mut()
        .with_config()
        .limit(CATALOG_MAX_BYTES)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    let clean = clean_catalog(&body).ok_or("The catalog download was empty.")?;
    let tmp = file.with_extension("tmp");
    std::fs::write(&tmp, clean).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &file).map_err(|e| e.to_string())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_entries_are_checked_and_icons_kept_local() {
        let id = "nngceckbapebfimnlniiiahkandclblb";
        let json = format!(
            r#"[{{"id":"{id}","name":"A","icon":"https://lh3.example/x"}},{{"id":"short","name":"B"}},{{"id":"{id}","name":"","icon":""}},{{"id":"{id}","name":"C","icon":"data:image/webp;base64,AAAA"}}]"#
        );
        let out: Vec<serde_json::Value> =
            serde_json::from_str(&clean_catalog(&json).unwrap()).unwrap();
        assert_eq!(out.len(), 2);
        assert_eq!(out[0]["icon"], "");
        assert_eq!(out[1]["icon"], "data:image/webp;base64,AAAA");
        assert_eq!(clean_catalog("[]"), None);
        assert_eq!(clean_catalog("not json"), None);
    }

    #[test]
    fn manifest_v2_extensions_are_refused() {
        let dir = std::env::temp_dir().join(format!("shoji-mv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("manifest.json"), r#"{"manifest_version":2}"#).unwrap();
        assert!(unsupported(&dir).is_some());
        std::fs::write(dir.join("manifest.json"), r#"{"manifest_version":3}"#).unwrap();
        assert!(unsupported(&dir).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_detail_pages_are_recognised() {
        let id = "nngceckbapebfimnlniiiahkandclblb";
        for url in [
            format!("https://chromewebstore.google.com/detail/bitwarden/{id}"),
            format!("https://chromewebstore.google.com/detail/{id}?hl=en"),
            format!("https://chrome.google.com/webstore/detail/bitwarden/{id}"),
        ] {
            assert_eq!(store_page_id(&url).as_deref(), Some(id), "{url}");
        }
        for url in [
            "https://chromewebstore.google.com/".to_string(),
            format!("https://chromewebstore.google.com/search/{id}"),
            format!("https://evil.example/detail/x/{id}"),
            format!("http://chromewebstore.google.com/detail/x/{id}"),
        ] {
            assert_eq!(store_page_id(&url), None, "{url}");
        }
    }

    #[test]
    fn a_cut_package_is_not_complete() {
        use std::io::Write;
        let mut zip = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut zip));
            w.start_file("manifest.json", zip::write::SimpleFileOptions::default())
                .unwrap();
            w.write_all(b"{}").unwrap();
            w.finish().unwrap();
        }
        let mut crx = b"Cr24".to_vec();
        crx.extend_from_slice(&3u32.to_le_bytes());
        crx.extend_from_slice(&0u32.to_le_bytes());
        crx.extend_from_slice(&zip);
        assert!(is_complete_package(&crx));
        assert!(!is_complete_package(&crx[..crx.len() - 10]));
        assert!(!is_complete_package(b""));
    }

    #[test]
    fn engine_versions_are_four_numbers() {
        assert_eq!(
            clean_version("130.0.2849.80\n").as_deref(),
            Some("130.0.2849.80")
        );
        assert_eq!(clean_version("130.0.2849"), None);
        assert_eq!(clean_version("130.0.x.1"), None);
    }

    #[test]
    fn id_comes_out_of_links_and_bare_ids() {
        let id = "eimadpbcbfnmbkopoojfekhnkhdbieeh";
        assert_eq!(parse_id(id).as_deref(), Some(id));
        assert_eq!(
            parse_id(&format!(
                "https://chromewebstore.google.com/detail/dark-reader/{id}?hl=en"
            ))
            .as_deref(),
            Some(id)
        );
        assert_eq!(
            parse_id(&format!("https://chrome.google.com/webstore/detail/{id}")).as_deref(),
            Some(id)
        );
        assert_eq!(parse_id("not an id"), None);
        // z is not in the id alphabet
        assert_eq!(parse_id("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"), None);
    }

    #[test]
    fn crx_headers_are_skipped() {
        let mut v3 = b"Cr24".to_vec();
        v3.extend(3u32.to_le_bytes());
        v3.extend(4u32.to_le_bytes());
        v3.extend([9, 9, 9, 9]);
        v3.extend(b"PK-zip");
        assert_eq!(crx_payload(&v3), Some(&b"PK-zip"[..]));

        let mut v2 = b"Cr24".to_vec();
        v2.extend(2u32.to_le_bytes());
        v2.extend(2u32.to_le_bytes());
        v2.extend(1u32.to_le_bytes());
        v2.extend([7, 7, 8]);
        v2.extend(b"PK-zip");
        assert_eq!(crx_payload(&v2), Some(&b"PK-zip"[..]));

        assert_eq!(crx_payload(b"PK\x03\x04"), None);
        assert_eq!(crx_payload(b"Cr24\x03\x00\x00\x00\xff\xff\xff\x7f"), None);
    }

    #[test]
    fn toolbar_icons_come_from_the_manifest_and_stay_inside_the_extension() {
        let dir = std::env::temp_dir().join(format!("shoji-ext-{}", std::process::id()));
        let ext = dir.join("abcdefghijklmnopabcdefghijklmnop");
        std::fs::create_dir_all(ext.join("img")).unwrap();
        std::fs::write(ext.join("img/32.png"), b"\x89PNG").unwrap();
        std::fs::write(
            ext.join("manifest.json"),
            r#"{"name":"Vault","manifest_version":3,"background":{"service_worker":"sw.js"},
               "action":{"default_popup":"/popup/index.html",
                         "default_icon":{"16":"img/16.png","32":"img/32.png","128":"img/128.png"}},
               "options_ui":{"page":"options.html"}}"#,
        )
        .unwrap();
        let found = pages(&dir);
        assert_eq!(found.len(), 1);
        let p = &found[0];
        assert_eq!(p.popup.as_deref(), Some("popup/index.html"));
        assert_eq!(p.options.as_deref(), Some("options.html"));
        assert_eq!(p.icon.as_deref(), Some("data:image/png;base64,iVBORw=="));
        assert_eq!(
            find(&found, "abcdefghijklmnopabcdefghijklmnop", "x").map(|p| p.name.as_str()),
            Some("Vault")
        );
        assert_eq!(
            find(&found, "other", "Vault").map(|p| p.name.as_str()),
            Some("Vault")
        );
        assert_eq!(icon_data(&ext, "../../secret.png"), None);
        assert_eq!(icon_data(&ext, "img/32.exe"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_answers_name_only_newer_versions() {
        assert!(newer("1.10.0", "1.9.9"));
        assert!(newer("2", "1.99"));
        assert!(!newer("1.0", "1.0.0"));
        assert!(!newer("1.0.1", "1.0.2"));
        let installed = vec![
            ("a".repeat(32), "1.0.0".to_string()),
            ("b".repeat(32), "3.1".to_string()),
        ];
        let url = update_check_url(&installed);
        assert!(url.contains(&format!(
            "id%3D{}%26v%3D1.0.0%26uc%26x%3Did%3D{}",
            "a".repeat(32),
            "b".repeat(32)
        )));
        let xml = format!(
            r#"<?xml version="1.0"?><gupdate xmlns="http://www.google.com/update2/response" protocol="2.0">
               <app appid="{a}" status="ok"><updatecheck codebase="https://x/a.crx" version="1.2.0" status="ok"/></app>
               <app appid="{b}" status="ok"><updatecheck status="noupdate"/></app></gupdate>"#,
            a = "a".repeat(32),
            b = "b".repeat(32)
        );
        assert_eq!(
            updates_in(&xml, &installed),
            vec![("a".repeat(32), "1.2.0".to_string())]
        );
    }

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n.max(1) as u64) as usize
        }
    }

    /// A package with a manifest, a nested file, a large compressible file and entries whose
    /// names try to escape the folder.
    fn hostile_zip() -> Vec<u8> {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in [
            (
                "manifest.json",
                br#"{"name":"x","manifest_version":3}"#.to_vec(),
            ),
            ("js/app.js", b"console.log(1)".to_vec()),
            ("big.bin", vec![0u8; 1 << 20]),
            ("../escape.txt", b"no".to_vec()),
            ("/abs.txt", b"no".to_vec()),
            ("js/../../up.txt", b"no".to_vec()),
            ("C:\\win.txt", b"no".to_vec()),
        ] {
            zip.start_file(name, opts).unwrap();
            zip.write_all(&body).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    /// Every file under `dir`, relative to it.
    fn files_under(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p.strip_prefix(dir).unwrap().to_path_buf());
                }
            }
        }
        out
    }

    #[test]
    fn crx_headers_never_point_outside_the_package() {
        let mut rng = Rng(0xc0de_cafe_f00d_0001);
        for _ in 0..20_000 {
            let mut b = b"Cr24".to_vec();
            b.extend(((rng.below(4)) as u32).to_le_bytes());
            for _ in 0..rng.below(4) {
                let n = match rng.below(3) {
                    0 => rng.below(64) as u32,
                    1 => u32::MAX - rng.below(16) as u32,
                    _ => rng.next() as u32,
                };
                b.extend(n.to_le_bytes());
            }
            b.extend((0..rng.below(64)).map(|_| rng.next() as u8));
            b.truncate(rng.below(b.len() + 1));
            if let Some(payload) = crx_payload(&b) {
                assert!(payload.len() <= b.len());
            }
        }
    }

    /// The unpacker over damaged packages: nothing panics, nothing lands outside the folder, and
    /// a package is either unpacked whole with its manifest or not at all.
    #[test]
    fn damaged_packages_never_escape_or_half_install() {
        let root = std::env::temp_dir().join(format!("shoji-crx-fuzz-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let dest = root.join("exts").join("abc");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let good = hostile_zip();
        unpack(&good, &dest).unwrap();
        // Names that climb out are skipped; absolute ones are kept inside the folder.
        let found = files_under(&dest);
        for want in ["manifest.json", "js/app.js", "big.bin"] {
            assert!(found.contains(&PathBuf::from(want)), "{found:?}");
        }
        assert!(
            !found
                .iter()
                .any(|p| p.ends_with("escape.txt") || p.ends_with("up.txt")),
            "{found:?}"
        );
        let mut rng = Rng(0x5eed_0fa1_17e5_7000);
        for _ in 0..400 {
            let mut bytes = good.clone();
            match rng.below(3) {
                0 => bytes.truncate(rng.below(bytes.len())),
                1 => {
                    for _ in 0..1 + rng.below(16) {
                        let at = rng.below(bytes.len());
                        bytes[at] = rng.next() as u8;
                    }
                }
                _ => {
                    let at = rng.below(bytes.len());
                    let end = (at + rng.below(256)).min(bytes.len());
                    bytes.drain(at..end);
                }
            }
            // On failure the last good install stays and the half-unpacked copy is gone.
            let result = unpack(&bytes, &dest);
            assert!(dest.join("manifest.json").is_file(), "{result:?}");
            if result.is_err() {
                assert!(!dest.with_extension("part").exists());
            }
            let stray: Vec<_> = files_under(&root)
                .into_iter()
                .filter(|p| !p.starts_with("exts"))
                .collect();
            assert!(stray.is_empty(), "{stray:?}");
            for name in ["escape.txt", "up.txt", "abs.txt", "win.txt"] {
                assert!(!root.join(name).exists() && !root.join("exts").join(name).exists());
            }
        }
        let _ = std::fs::remove_dir_all(&root);
    }
}
