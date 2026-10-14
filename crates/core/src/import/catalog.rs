//! Every browser Shoji can import from, and where each keeps its data on Windows.

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Family {
    /// Chrome and everything built on it: JSON bookmarks, SQLite history/cookies/logins
    Chromium,
    /// Firefox and its forks: `profiles.ini`, `places.sqlite`, `cookies.sqlite`
    Firefox,
    /// A folder of `.url` shortcuts (Internet Explorer / legacy Edge favourites)
    Favorites,
    /// A bookmarks file exported as HTML (Safari, or any browser's export)
    HtmlFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Root {
    Local,
    Roaming,
    Home,
    /// Not in a fixed place: the user points at it
    Pick,
}

#[derive(Debug, Clone)]
pub struct Source {
    pub id: &'static str,
    pub name: &'static str,
    pub family: Family,
    /// Shown as a card; the rest are in the "More browsers" list
    pub popular: bool,
    /// Brand colour, used for the logo fallback tile
    pub color: &'static str,
    pub root: Root,
    /// Folder under the root. For Chromium this is the "User Data" folder (or the profile itself
    /// when `flat`); for Firefox the folder holding `profiles.ini`.
    pub rel: &'static str,
    /// The folder is itself the one profile (Opera)
    pub flat: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Profile {
    pub name: String,
    /// The profile folder (or the file, for [`Family::HtmlFile`])
    #[serde(skip)]
    pub dir: PathBuf,
    /// Chromium: the `Local State` file that holds the key for this profile's secrets
    #[serde(skip)]
    pub local_state: PathBuf,
}

const fn s(
    id: &'static str,
    name: &'static str,
    family: Family,
    popular: bool,
    color: &'static str,
    root: Root,
    rel: &'static str,
    flat: bool,
) -> Source {
    Source {
        id,
        name,
        family,
        popular,
        color,
        root,
        rel,
        flat,
    }
}

use Family::*;
use Root::*;

pub static SOURCES: &[Source] = &[
    // The ones almost everybody is coming from
    s(
        "chrome",
        "Google Chrome",
        Chromium,
        true,
        "#4285f4",
        Local,
        r"Google\Chrome\User Data",
        false,
    ),
    s(
        "edge",
        "Microsoft Edge",
        Chromium,
        true,
        "#0f9d9a",
        Local,
        r"Microsoft\Edge\User Data",
        false,
    ),
    s(
        "brave",
        "Brave",
        Chromium,
        true,
        "#fb542b",
        Local,
        r"BraveSoftware\Brave-Browser\User Data",
        false,
    ),
    s(
        "firefox",
        "Firefox",
        Firefox,
        true,
        "#ff7139",
        Roaming,
        r"Mozilla\Firefox",
        false,
    ),
    // More Chromium browsers
    s(
        "opera",
        "Opera",
        Chromium,
        false,
        "#ff1b2d",
        Roaming,
        r"Opera Software\Opera Stable",
        true,
    ),
    s(
        "opera-gx",
        "Opera GX",
        Chromium,
        false,
        "#fa1e4e",
        Roaming,
        r"Opera Software\Opera GX Stable",
        true,
    ),
    s(
        "vivaldi",
        "Vivaldi",
        Chromium,
        false,
        "#ef3939",
        Local,
        r"Vivaldi\User Data",
        false,
    ),
    s(
        "arc",
        "Arc",
        Chromium,
        false,
        "#5c6cff",
        Local,
        r"Packages\TheBrowserCompany.Arc_ttt1ap7aakyb4\LocalCache\Local\Arc\User Data",
        false,
    ),
    s(
        "comet",
        "Comet",
        Chromium,
        false,
        "#20808d",
        Local,
        r"Perplexity\Comet\User Data",
        false,
    ),
    s(
        "chromium",
        "Chromium",
        Chromium,
        false,
        "#3f7de0",
        Local,
        r"Chromium\User Data",
        false,
    ),
    s(
        "chrome-beta",
        "Chrome Beta",
        Chromium,
        false,
        "#e8710a",
        Local,
        r"Google\Chrome Beta\User Data",
        false,
    ),
    s(
        "chrome-dev",
        "Chrome Dev",
        Chromium,
        false,
        "#188038",
        Local,
        r"Google\Chrome Dev\User Data",
        false,
    ),
    s(
        "chrome-canary",
        "Chrome Canary",
        Chromium,
        false,
        "#f9ab00",
        Local,
        r"Google\Chrome SxS\User Data",
        false,
    ),
    s(
        "edge-beta",
        "Edge Beta",
        Chromium,
        false,
        "#1f8a70",
        Local,
        r"Microsoft\Edge Beta\User Data",
        false,
    ),
    s(
        "edge-dev",
        "Edge Dev",
        Chromium,
        false,
        "#3a7bd5",
        Local,
        r"Microsoft\Edge Dev\User Data",
        false,
    ),
    s(
        "brave-beta",
        "Brave Beta",
        Chromium,
        false,
        "#fb542b",
        Local,
        r"BraveSoftware\Brave-Browser-Beta\User Data",
        false,
    ),
    s(
        "brave-nightly",
        "Brave Nightly",
        Chromium,
        false,
        "#fb542b",
        Local,
        r"BraveSoftware\Brave-Browser-Nightly\User Data",
        false,
    ),
    s(
        "yandex",
        "Yandex Browser",
        Chromium,
        false,
        "#fc3f1d",
        Local,
        r"Yandex\YandexBrowser\User Data",
        false,
    ),
    s(
        "whale",
        "Naver Whale",
        Chromium,
        false,
        "#03c75a",
        Local,
        r"Naver\Naver Whale\User Data",
        false,
    ),
    s(
        "thorium",
        "Thorium",
        Chromium,
        false,
        "#e04f5f",
        Local,
        r"Thorium\User Data",
        false,
    ),
    s(
        "iridium",
        "Iridium",
        Chromium,
        false,
        "#3b6ea5",
        Local,
        r"Iridium\User Data",
        false,
    ),
    s(
        "epic",
        "Epic Privacy Browser",
        Chromium,
        false,
        "#6b3fa0",
        Local,
        r"Epic Privacy Browser\User Data",
        false,
    ),
    s(
        "slimjet",
        "Slimjet",
        Chromium,
        false,
        "#2aa4dd",
        Local,
        r"Slimjet\User Data",
        false,
    ),
    s(
        "maxthon",
        "Maxthon",
        Chromium,
        false,
        "#2b7de9",
        Local,
        r"Maxthon\User Data",
        false,
    ),
    s(
        "avast",
        "Avast Secure Browser",
        Chromium,
        false,
        "#ff7800",
        Local,
        r"AVAST Software\Browser\User Data",
        false,
    ),
    s(
        "avg",
        "AVG Secure Browser",
        Chromium,
        false,
        "#1a73c9",
        Local,
        r"AVG\Browser\User Data",
        false,
    ),
    s(
        "ccleaner",
        "CCleaner Browser",
        Chromium,
        false,
        "#e3392b",
        Local,
        r"CCleaner Browser\User Data",
        false,
    ),
    s(
        "coccoc",
        "Cốc Cốc",
        Chromium,
        false,
        "#2f9e44",
        Local,
        r"CocCoc\Browser\User Data",
        false,
    ),
    s(
        "qq",
        "QQ Browser",
        Chromium,
        false,
        "#12b7f5",
        Local,
        r"Tencent\QQBrowser\User Data",
        false,
    ),
    s(
        "360",
        "360 Browser",
        Chromium,
        false,
        "#19b955",
        Local,
        r"360Chrome\Chrome\User Data",
        false,
    ),
    s(
        "cent",
        "Cent Browser",
        Chromium,
        false,
        "#2d89ef",
        Local,
        r"CentBrowser\User Data",
        false,
    ),
    s(
        "citrio",
        "Citrio",
        Chromium,
        false,
        "#e8590c",
        Local,
        r"CatalinaGroup\Citrio\User Data",
        false,
    ),
    s(
        "torch",
        "Torch",
        Chromium,
        false,
        "#f08c00",
        Local,
        r"Torch\User Data",
        false,
    ),
    s(
        "dragon",
        "Comodo Dragon",
        Chromium,
        false,
        "#c92a2a",
        Local,
        r"Comodo\Dragon\User Data",
        false,
    ),
    s(
        "sidekick",
        "Sidekick",
        Chromium,
        false,
        "#7048e8",
        Local,
        r"Sidekick\User Data",
        false,
    ),
    // Firefox family
    s(
        "librewolf",
        "LibreWolf",
        Firefox,
        false,
        "#00acff",
        Roaming,
        "librewolf",
        false,
    ),
    s(
        "waterfox", "Waterfox", Firefox, false, "#0aa5ff", Roaming, "Waterfox", false,
    ),
    s(
        "floorp", "Floorp", Firefox, false, "#2b8bf0", Roaming, "Floorp", false,
    ),
    s(
        "zen",
        "Zen Browser",
        Firefox,
        false,
        "#f76f53",
        Roaming,
        "zen",
        false,
    ),
    s(
        "palemoon",
        "Pale Moon",
        Firefox,
        false,
        "#4c6ef5",
        Roaming,
        r"Moonchild Productions\Pale Moon",
        false,
    ),
    s(
        "seamonkey",
        "SeaMonkey",
        Firefox,
        false,
        "#1c7ed6",
        Roaming,
        r"Mozilla\SeaMonkey",
        false,
    ),
    s(
        "tor",
        "Tor Browser",
        Firefox,
        false,
        "#7d4698",
        Pick,
        "",
        false,
    ),
    s(
        "mullvad",
        "Mullvad Browser",
        Firefox,
        false,
        "#294d73",
        Pick,
        "",
        false,
    ),
    // Everything else
    s(
        "ie",
        "Internet Explorer",
        Favorites,
        false,
        "#1ebbee",
        Home,
        "Favorites",
        false,
    ),
    s(
        "safari", "Safari", HtmlFile, false, "#1c9bef", Pick, "", false,
    ),
    s(
        "file",
        "Bookmarks file (HTML)",
        HtmlFile,
        false,
        "#868e96",
        Pick,
        "",
        false,
    ),
];

pub fn find(id: &str) -> Option<&'static Source> {
    SOURCES.iter().find(|s| s.id == id)
}

/// Where the install of `source` keeps its data, given the three Windows base folders.
pub fn data_dir(source: &Source, local: &Path, roaming: &Path, home: &Path) -> Option<PathBuf> {
    let base = match source.root {
        Local => local,
        Roaming => roaming,
        Home => home,
        Pick => return None,
    };
    Some(base.join(source.rel.replace('\\', std::path::MAIN_SEPARATOR_STR)))
}

/// The profiles found in `dir` for `source`. Empty when the browser isn't installed (or its
/// data is gone).
pub fn profiles(source: &Source, dir: &Path) -> Vec<Profile> {
    match source.family {
        Chromium => chromium_profiles(dir, source.flat),
        Firefox => firefox_profiles(dir),
        Favorites => {
            if dir.is_dir() {
                vec![Profile {
                    name: "Favorites".into(),
                    dir: dir.to_path_buf(),
                    local_state: PathBuf::new(),
                }]
            } else {
                Vec::new()
            }
        }
        HtmlFile => {
            if dir.is_file() {
                vec![Profile {
                    name: file_name(dir),
                    dir: dir.to_path_buf(),
                    local_state: PathBuf::new(),
                }]
            } else {
                Vec::new()
            }
        }
    }
}

/// What the page needs to draw one source: the catalogue entry plus what was found on this PC.
#[derive(Debug, Clone, Serialize)]
pub struct Detected {
    pub id: &'static str,
    pub name: &'static str,
    pub family: Family,
    pub popular: bool,
    pub color: &'static str,
    /// Found on this PC (it has at least one profile)
    pub installed: bool,
    /// Has no fixed location, so the user has to pick a file or folder
    pub pick: bool,
    pub profiles: Vec<Profile>,
}

/// Every source with its profiles. The four popular ones come first, then installed browsers,
/// then the rest in catalogue order.
pub fn detect(local: &Path, roaming: &Path, home: &Path) -> Vec<Detected> {
    let mut all: Vec<Detected> = SOURCES
        .iter()
        .map(|s| {
            let profiles = data_dir(s, local, roaming, home)
                .map(|d| profiles(s, &d))
                .unwrap_or_default();
            Detected {
                id: s.id,
                name: s.name,
                family: s.family,
                popular: s.popular,
                color: s.color,
                installed: !profiles.is_empty(),
                pick: s.root == Pick,
                profiles,
            }
        })
        .collect();
    // Stable: popular first, then installed, then the rest.
    all.sort_by_key(|d| (!d.popular, !d.installed));
    all
}

/// The profiles at a location the user picked (a browser's data folder, one profile folder, or a
/// bookmarks file).
pub fn profiles_at(source: &Source, path: &Path) -> Vec<Profile> {
    if source.family == Chromium
        && (path.join("Bookmarks").exists() || path.join("History").exists())
    {
        let sibling = path
            .parent()
            .map(|p| p.join("Local State"))
            .unwrap_or_default();
        let own = path.join("Local State");
        return vec![Profile {
            name: file_name(path),
            dir: path.to_path_buf(),
            local_state: if own.exists() { own } else { sibling },
        }];
    }
    profiles(source, path)
}

fn file_name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn chromium_profiles(dir: &Path, flat: bool) -> Vec<Profile> {
    let local_state = dir.join("Local State");
    if flat {
        return if dir.join("Bookmarks").exists() || dir.join("History").exists() {
            vec![Profile {
                name: "Default".into(),
                dir: dir.to_path_buf(),
                local_state: if local_state.exists() {
                    local_state
                } else {
                    dir.parent()
                        .map(|p| p.join("Local State"))
                        .unwrap_or_default()
                },
            }]
        } else {
            Vec::new()
        };
    }
    let names: serde_json::Value = std::fs::read_to_string(&local_state)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let folder = file_name(&e.path());
        let is_profile = folder == "Default" || folder.starts_with("Profile ");
        if !is_profile || !e.path().join("Preferences").exists() {
            continue;
        }
        let name = names["profile"]["info_cache"][&folder]["name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .unwrap_or(&folder)
            .to_string();
        out.push(Profile {
            name,
            dir: e.path(),
            local_state: local_state.clone(),
        });
    }
    // "Default" first, then by folder name ("Profile 2" before "Profile 10" is not worth the code).
    out.sort_by(|a, b| {
        (a.dir.file_name() != Some("Default".as_ref()), &a.dir)
            .cmp(&(b.dir.file_name() != Some("Default".as_ref()), &b.dir))
    });
    out
}

/// Profiles listed in `profiles.ini` (or the folder itself when a profile was picked directly).
fn firefox_profiles(dir: &Path) -> Vec<Profile> {
    if dir.join("places.sqlite").exists() || dir.join("cookies.sqlite").exists() {
        return vec![Profile {
            name: file_name(dir),
            dir: dir.to_path_buf(),
            local_state: PathBuf::new(),
        }];
    }
    let Ok(ini) = std::fs::read_to_string(dir.join("profiles.ini")) else {
        return Vec::new();
    };
    let mut out: Vec<Profile> = Vec::new();
    let (mut name, mut path, mut relative) = (String::new(), String::new(), true);
    let mut flush = |name: &mut String, path: &mut String, relative: &mut bool| {
        if !path.is_empty() {
            let p = if *relative {
                dir.join(path.replace('/', std::path::MAIN_SEPARATOR_STR))
            } else {
                PathBuf::from(path.as_str())
            };
            if p.is_dir() && !out.iter().any(|o| o.dir == p) {
                out.push(Profile {
                    name: if name.is_empty() {
                        file_name(&p)
                    } else {
                        name.clone()
                    },
                    dir: p,
                    local_state: PathBuf::new(),
                });
            }
        }
        name.clear();
        path.clear();
        *relative = true;
    };
    for line in ini.lines().map(str::trim) {
        if line.starts_with('[') {
            flush(&mut name, &mut path, &mut relative);
        } else if let Some(v) = line.strip_prefix("Name=") {
            name = v.to_string();
        } else if let Some(v) = line.strip_prefix("Path=") {
            path = v.to_string();
        } else if let Some(v) = line.strip_prefix("IsRelative=") {
            relative = v != "0";
        }
    }
    flush(&mut name, &mut path, &mut relative);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_the_popular_four_lead() {
        let mut ids: Vec<_> = SOURCES.iter().map(|s| s.id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), SOURCES.len());
        let popular: Vec<_> = SOURCES.iter().filter(|s| s.popular).map(|s| s.id).collect();
        assert_eq!(popular, ["chrome", "edge", "brave", "firefox"]);
        assert!(SOURCES.len() >= 40);
        assert!(SOURCES
            .iter()
            .all(|s| s.color.starts_with('#') && s.color.len() == 7));
    }

    #[test]
    fn detection_puts_popular_then_installed_first() {
        let base = std::env::temp_dir().join(format!("shoji_det_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let vivaldi = base.join("local/Vivaldi/User Data/Default");
        std::fs::create_dir_all(&vivaldi).unwrap();
        std::fs::write(vivaldi.join("Preferences"), "{}").unwrap();
        let found = detect(
            &base.join("local"),
            &base.join("roaming"),
            &base.join("home"),
        );
        let order: Vec<_> = found.iter().take(6).map(|d| d.id).collect();
        assert_eq!(
            order,
            ["chrome", "edge", "brave", "firefox", "vivaldi", "opera"]
        );
        assert!(found.iter().find(|d| d.id == "vivaldi").unwrap().installed);
        assert!(!found.iter().find(|d| d.id == "chrome").unwrap().installed);
        assert!(found.iter().find(|d| d.id == "safari").unwrap().pick);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_picked_profile_folder_is_one_profile() {
        let base = std::env::temp_dir().join(format!("shoji_pick_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(base.join("Profile 3")).unwrap();
        std::fs::write(base.join("Profile 3/Bookmarks"), "{}").unwrap();
        std::fs::write(base.join("Local State"), "{}").unwrap();
        let p = profiles_at(find("chrome").unwrap(), &base.join("Profile 3"));
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].local_state, base.join("Local State"));
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn firefox_profiles_come_from_profiles_ini() {
        let dir = std::env::temp_dir().join(format!("shoji_ff_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Profiles/abc.default-release")).unwrap();
        std::fs::write(
            dir.join("profiles.ini"),
            "[Profile0]\nName=default-release\nIsRelative=1\nPath=Profiles/abc.default-release\nDefault=1\n\n[General]\nVersion=2\n",
        )
        .unwrap();
        let found = firefox_profiles(&dir);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "default-release");
        assert!(found[0].dir.ends_with("abc.default-release"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn chromium_profiles_use_their_display_names() {
        let dir = std::env::temp_dir().join(format!("shoji_cr_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for p in ["Default", "Profile 1", "System Profile"] {
            std::fs::create_dir_all(dir.join(p)).unwrap();
            std::fs::write(dir.join(p).join("Preferences"), "{}").unwrap();
        }
        std::fs::write(
            dir.join("Local State"),
            r#"{"profile":{"info_cache":{"Default":{"name":"Person 1"},"Profile 1":{"name":"Work"}}}}"#,
        )
        .unwrap();
        let found = chromium_profiles(&dir, false);
        let names: Vec<_> = found.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Person 1", "Work"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
