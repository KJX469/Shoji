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

fn download(id: &str) -> Result<Vec<u8>, String> {
    let url = format!(
        "https://clients2.google.com/service/update2/crx?response=redirect&prodversion=130.0.0.0&acceptformat=crx2,crx3&x=id%3D{id}%26installsource%3Dondemand%26uc"
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
    resp.body_mut()
        .with_config()
        .limit(MAX_CRX_BYTES)
        .read_to_vec()
        .map_err(|e| e.to_string())
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
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
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
            let _ = std::fs::remove_dir_all(&tmp);
            return Err("That extension is too large.".into());
        }
        std::fs::write(&out, data).map_err(|e| e.to_string())?;
    }
    if !tmp.join("manifest.json").is_file() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("That package has no manifest.json.".into());
    }
    let _ = std::fs::remove_dir_all(dest);
    std::fs::rename(&tmp, dest).map_err(|e| e.to_string())
}

/// Download the store's extension `id` and unpack it under `extensions_dir`. Blocking.
pub fn fetch(id: &str, extensions_dir: &Path) -> Result<PathBuf, String> {
    let bytes = download(id)?;
    let zip = crx_payload(&bytes).ok_or("The download wasn't an extension package.")?;
    let dest = extensions_dir.join(id);
    std::fs::create_dir_all(extensions_dir).map_err(|e| e.to_string())?;
    unpack(zip, &dest)?;
    Ok(dest)
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
        "https://clients2.google.com/service/update2/crx?response=updatecheck&prodversion=130.0.0.0&acceptformat=crx2,crx3&x={x}"
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
