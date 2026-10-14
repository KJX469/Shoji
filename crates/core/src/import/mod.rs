//! Bringing data over from another browser: what can be imported, where each browser keeps it,
//! and the readers for its formats. Everything here is portable and read-only; the parts that
//! need Windows (decrypting protected values, injecting cookies, writing the result into the
//! engine) live in the app crate and plug in through [`Decryptor`].

pub mod bookmarks;
pub mod catalog;
pub mod sqlite;

use crate::userdata::{Favourite, HistoryEntry};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::{Path, PathBuf};

/// What to bring over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Kinds {
    pub favourites: bool,
    pub history: bool,
    pub cookies: bool,
    pub passwords: bool,
}

impl Default for Kinds {
    fn default() -> Self {
        Self {
            favourites: true,
            history: true,
            cookies: true,
            passwords: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cookie {
    pub host: String,
    pub name: String,
    pub value: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
    /// Unix seconds; `None` for a session cookie
    pub expires: Option<i64>,
    /// "lax", "strict", "none" or "" (unspecified)
    pub same_site: &'static str,
}

/// Unlocks values the other browser protected for this Windows user. `None` means "cannot be
/// read", which is reported to the user rather than treated as an error.
pub trait Decryptor {
    /// Prepare for a browser whose `Local State` file is at `local_state` (Chromium only).
    fn load_key(&mut self, local_state: &Path) -> bool;
    fn decrypt(&self, blob: &[u8]) -> Option<Vec<u8>>;
}

/// A decryptor that can read nothing (used when no key is available, and in tests).
pub struct NoDecryptor;

impl Decryptor for NoDecryptor {
    fn load_key(&mut self, _: &Path) -> bool {
        false
    }
    fn decrypt(&self, _: &[u8]) -> Option<Vec<u8>> {
        None
    }
}

#[derive(Debug)]
pub enum ImportError {
    /// The source browser has the file open; ask the user to close it
    Locked(PathBuf),
    Missing(PathBuf),
    Other(String),
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ImportError::Locked(p) => write!(f, "{} is in use", p.display()),
            ImportError::Missing(p) => write!(f, "{} was not found", p.display()),
            ImportError::Other(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for ImportError {}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Report {
    pub favourites: usize,
    pub history: usize,
    pub cookies: usize,
    pub passwords: usize,
    /// Things that could not be brought over, as short sentences for the user
    pub notes: Vec<String>,
    /// The source browser must be closed for some data to be read
    pub locked: bool,
}

#[derive(Debug, Default)]
pub struct Imported {
    pub favourites: Vec<Favourite>,
    pub history: Vec<HistoryEntry>,
    pub cookies: Vec<Cookie>,
    /// Saved logins, ready for the password manager
    pub logins: Vec<crate::vault::Draft>,
    pub report: Report,
}

/// Read what `kinds` asks for from one profile of `source`. `progress` is told what is being read.
pub fn read_profile(
    source: &catalog::Source,
    profile: &catalog::Profile,
    kinds: Kinds,
    decryptor: &mut dyn Decryptor,
    progress: &mut dyn FnMut(&str),
) -> Imported {
    use catalog::Family;
    let mut out = Imported::default();
    let name = source.name;
    let note = |out: &mut Imported, what: &str, e: ImportError| {
        if matches!(e, ImportError::Locked(_)) {
            out.report.locked = true;
        }
        out.report.notes.push(format!("{what}: {e}"));
    };
    match source.family {
        Family::Chromium => {
            let dir = &profile.dir;
            if kinds.favourites {
                progress("Reading favourites");
                match std::fs::read_to_string(dir.join("Bookmarks")) {
                    Ok(json) => out.favourites = bookmarks::from_chromium_json(&json),
                    Err(_) => out.report.notes.push(format!("{name} has no favourites file")),
                }
            }
            if kinds.history {
                progress("Reading history");
                match sqlite::chromium_history(&dir.join("History")) {
                    Ok(h) => out.history = h,
                    Err(e) => note(&mut out, "History", e),
                }
            }
            let need_key = kinds.cookies || kinds.passwords;
            let have_key = need_key && decryptor.load_key(&profile.local_state);
            if need_key && !have_key {
                out.report
                    .notes
                    .push(format!("Sign-ins and passwords in {name} are protected and can't be read"));
            }
            if kinds.cookies && have_key {
                progress("Reading sign-ins");
                let path = [dir.join("Network").join("Cookies"), dir.join("Cookies")]
                    .into_iter()
                    .find(|p| p.exists());
                match path {
                    Some(p) => match sqlite::chromium_cookies(&p, decryptor) {
                        Ok((c, skipped)) => {
                            out.cookies = c;
                            if skipped > 0 {
                                out.report.notes.push(format!(
                                    "{skipped} sign-ins are locked to {name} and can't be moved"
                                ));
                            }
                        }
                        Err(e) => note(&mut out, "Sign-ins", e),
                    },
                    None => {}
                }
            }
            if kinds.passwords && have_key {
                progress("Reading passwords");
                match sqlite::chromium_logins(&dir.join("Login Data"), decryptor) {
                    Ok((l, skipped)) => {
                        out.logins = l;
                        if skipped > 0 {
                            out.report.notes.push(format!(
                                "{skipped} passwords are locked to {name} and can't be moved"
                            ));
                        }
                    }
                    Err(e) => note(&mut out, "Passwords", e),
                }
            }
        }
        Family::Firefox => {
            let dir = &profile.dir;
            if kinds.favourites {
                progress("Reading favourites");
                match sqlite::firefox_bookmarks(&dir.join("places.sqlite")) {
                    Ok(f) => out.favourites = f,
                    Err(e) => note(&mut out, "Favourites", e),
                }
            }
            if kinds.history {
                progress("Reading history");
                match sqlite::firefox_history(&dir.join("places.sqlite")) {
                    Ok(h) => out.history = h,
                    Err(e) => note(&mut out, "History", e),
                }
            }
            if kinds.passwords && dir.join("logins.json").exists() {
                out.report.notes.push(format!(
                    "{name} keeps passwords in a format Shoji can't open. Export them from {name} (Passwords → Export) and use Import in Shoji's password manager"
                ));
            }
            if kinds.cookies {
                progress("Reading sign-ins");
                match sqlite::firefox_cookies(&dir.join("cookies.sqlite")) {
                    Ok(c) => out.cookies = c,
                    Err(e) => note(&mut out, "Sign-ins", e),
                }
            }
        }
        Family::Favorites => {
            if kinds.favourites {
                progress("Reading favourites");
                out.favourites = bookmarks::from_url_files(&profile.dir);
            }
        }
        Family::HtmlFile => {
            if kinds.favourites {
                progress("Reading favourites");
                match std::fs::read_to_string(&profile.dir) {
                    Ok(html) => out.favourites = bookmarks::from_netscape_html(&html),
                    Err(e) => note(&mut out, "Favourites", ImportError::Other(e.to_string())),
                }
            }
        }
    }
    out.report.favourites = out.favourites.len();
    out.report.history = out.history.len();
    out.report.cookies = out.cookies.len();
    out.report.passwords = out.logins.len();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_chromium_profile_end_to_end() {
        let dir = std::env::temp_dir().join(format!("shoji_e2e_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Bookmarks"),
            r#"{"roots":{"bookmark_bar":{"type":"folder","children":[{"type":"url","name":"A","url":"https://a.com/"}]}}}"#,
        )
        .unwrap();
        let db = rusqlite::Connection::open(dir.join("History")).unwrap();
        db.execute_batch(
            "CREATE TABLE urls(id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER, last_visit_time INTEGER, hidden INTEGER);
             INSERT INTO urls VALUES (1,'https://a.com/','A',1,13300000000000000,0);",
        )
        .unwrap();
        drop(db);
        let source = catalog::find("chrome").unwrap();
        let profile = catalog::Profile {
            name: "Default".into(),
            dir: dir.clone(),
            local_state: dir.join("Local State"),
        };
        let mut steps = Vec::new();
        let got = read_profile(source, &profile, Kinds::default(), &mut NoDecryptor, &mut |s| {
            steps.push(s.to_string())
        });
        assert_eq!((got.report.favourites, got.report.history, got.report.cookies), (1, 1, 0));
        assert!(!got.report.locked);
        assert!(got.report.notes.iter().any(|n| n.contains("Sign-ins")));
        assert_eq!(steps, ["Reading favourites", "Reading history"]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
