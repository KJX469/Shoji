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

/// The pages an installed extension offers: (name, popup path, options path), for each folder
/// under `extensions_dir`.
pub fn pages(extensions_dir: &Path) -> Vec<(String, Option<String>, Option<String>)> {
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
        };
        let popup = path(
            m.pointer("/action/default_popup")
                .or_else(|| m.pointer("/browser_action/default_popup")),
        );
        let options = path(
            m.get("options_page")
                .or_else(|| m.pointer("/options_ui/page")),
        );
        out.push((name, popup, options));
    }
    out
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
