//! Readers for the SQLite databases browsers keep: history, cookies and saved logins. The source
//! browser usually has them open, so each is copied to a temporary file first (with its
//! write-ahead log, which holds the newest rows) and read from the copy.

use super::{Cookie, Decryptor, ImportError};
use crate::userdata::{Favourite, HistoryEntry};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

/// A read-only connection to a private copy of `path`, deleted when dropped.
struct Copy {
    conn: Option<Connection>,
    files: Vec<PathBuf>,
}

impl Drop for Copy {
    fn drop(&mut self) {
        self.conn = None;
        for f in &self.files {
            let _ = std::fs::remove_file(f);
        }
    }
}

static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn open(path: &Path) -> Result<Copy, ImportError> {
    if !path.exists() {
        return Err(ImportError::Missing(path.to_path_buf()));
    }
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("shoji-import-{}-{n}.db", std::process::id()));
    let mut files = vec![tmp.clone()];
    std::fs::copy(path, &tmp).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        // A sharing violation (Windows error 32/33) means the browser holds the file open.
        if matches!(e.raw_os_error(), Some(32) | Some(33)) || e.kind() == std::io::ErrorKind::PermissionDenied {
            ImportError::Locked(path.to_path_buf())
        } else {
            ImportError::Other(e.to_string())
        }
    })?;
    // The newest rows may only be in the log; it is named after the database file.
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    let wal = PathBuf::from(wal);
    if wal.exists() {
        let mut tmp_wal = tmp.as_os_str().to_owned();
        tmp_wal.push("-wal");
        let tmp_wal = PathBuf::from(tmp_wal);
        if std::fs::copy(&wal, &tmp_wal).is_ok() {
            files.push(tmp_wal);
        }
    }
    let mut shm = tmp.as_os_str().to_owned();
    shm.push("-shm");
    files.push(PathBuf::from(shm));
    let conn = Connection::open_with_flags(&tmp, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|e| ImportError::Other(e.to_string()))?;
    Ok(Copy {
        conn: Some(conn),
        files,
    })
}

impl Copy {
    fn conn(&self) -> &Connection {
        self.conn.as_ref().expect("connection lives until drop")
    }
}

fn other(e: rusqlite::Error) -> ImportError {
    ImportError::Other(e.to_string())
}

const MAX_HISTORY: usize = 50_000;

/// Microseconds since 1601 (Chromium) to Unix milliseconds.
fn webkit_to_unix_ms(t: i64) -> i64 {
    (t / 1000) - 11_644_473_600_000
}

fn web(url: &str) -> bool {
    url.starts_with("https://") || url.starts_with("http://")
}

pub fn chromium_history(path: &Path) -> Result<Vec<HistoryEntry>, ImportError> {
    let db = open(path)?;
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT url, title, visit_count, last_visit_time FROM urls \
             WHERE hidden = 0 ORDER BY last_visit_time DESC LIMIT ?1",
        )
        .map_err(other)?;
    let rows = stmt
        .query_map([MAX_HISTORY as i64], |r| {
            Ok(HistoryEntry {
                url: r.get(0)?,
                title: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                visits: r.get::<_, i64>(2)?.max(1) as u32,
                last_visit: webkit_to_unix_ms(r.get(3)?).max(0) as u64,
            })
        })
        .map_err(other)?;
    Ok(rows.flatten().filter(|h| web(&h.url)).collect())
}

pub fn firefox_history(path: &Path) -> Result<Vec<HistoryEntry>, ImportError> {
    let db = open(path)?;
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT url, title, visit_count, last_visit_date FROM moz_places \
             WHERE hidden = 0 AND visit_count > 0 AND last_visit_date IS NOT NULL \
             ORDER BY last_visit_date DESC LIMIT ?1",
        )
        .map_err(other)?;
    let rows = stmt
        .query_map([MAX_HISTORY as i64], |r| {
            Ok(HistoryEntry {
                url: r.get(0)?,
                title: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                visits: r.get::<_, i64>(2)?.max(1) as u32,
                last_visit: (r.get::<_, i64>(3)? / 1000).max(0) as u64,
            })
        })
        .map_err(other)?;
    Ok(rows.flatten().filter(|h| web(&h.url)).collect())
}

/// Firefox bookmarks: the toolbar and menu first (they have the lowest folder ids).
pub fn firefox_bookmarks(path: &Path) -> Result<Vec<Favourite>, ImportError> {
    let db = open(path)?;
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT p.url, b.title FROM moz_bookmarks b JOIN moz_places p ON b.fk = p.id \
             WHERE b.type = 1 ORDER BY b.parent, b.position",
        )
        .map_err(other)?;
    let rows = stmt
        .query_map([], |r| {
            Ok(Favourite {
                url: r.get(0)?,
                title: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            })
        })
        .map_err(other)?;
    let mut out: Vec<Favourite> = Vec::new();
    for f in rows.flatten() {
        if web(&f.url) && !out.iter().any(|o| o.url == f.url) {
            out.push(f);
        }
    }
    Ok(out)
}

fn same_site(v: i64) -> &'static str {
    match v {
        0 => "none",
        1 => "lax",
        2 => "strict",
        _ => "",
    }
}

pub fn firefox_cookies(path: &Path) -> Result<Vec<Cookie>, ImportError> {
    let db = open(path)?;
    let mut stmt = db
        .conn()
        .prepare("SELECT host, name, value, path, expiry, isSecure, isHttpOnly, sameSite FROM moz_cookies")
        .map_err(other)?;
    let rows = stmt
        .query_map([], |r| {
            let expiry: i64 = r.get(4)?;
            // Newer Firefox stores milliseconds; older stores seconds.
            let expiry = if expiry > 100_000_000_000 { expiry / 1000 } else { expiry };
            Ok(Cookie {
                host: r.get(0)?,
                name: r.get(1)?,
                value: r.get(2)?,
                path: r.get(3)?,
                expires: (expiry > 0).then_some(expiry),
                secure: r.get::<_, i64>(5)? != 0,
                http_only: r.get::<_, i64>(6)? != 0,
                same_site: same_site(r.get(7)?),
            })
        })
        .map_err(other)?;
    Ok(rows.flatten().collect())
}

/// Chromium cookies. Returns the readable ones and how many could not be decrypted.
pub fn chromium_cookies(
    path: &Path,
    decryptor: &dyn Decryptor,
) -> Result<(Vec<Cookie>, usize), ImportError> {
    let db = open(path)?;
    // From database version 24 the decrypted value starts with a 32-byte hash of the host.
    let version: i64 = db
        .conn()
        .query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| {
            r.get::<_, String>(0)
        })
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, \
             is_httponly, samesite FROM cookies",
        )
        .map_err(other)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Vec<u8>>(3)?,
                r.get::<_, String>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)? != 0,
                r.get::<_, i64>(7)? != 0,
                r.get::<_, i64>(8)?,
            ))
        })
        .map_err(other)?;
    let (mut out, mut skipped) = (Vec::new(), 0);
    for (host, name, plain, enc, cpath, expires, secure, http_only, ss) in rows.flatten() {
        let value = if !plain.is_empty() || enc.is_empty() {
            plain
        } else {
            match decryptor.decrypt(&enc) {
                Some(mut bytes) => {
                    if version >= 24 && bytes.len() >= 32 {
                        bytes.drain(..32);
                    }
                    String::from_utf8_lossy(&bytes).into_owned()
                }
                None => {
                    skipped += 1;
                    continue;
                }
            }
        };
        // 0 means a session cookie; otherwise microseconds since 1601.
        let expires = (expires > 0).then(|| webkit_to_unix_ms(expires) / 1000);
        // Already expired: not worth carrying over.
        if expires.is_some_and(|e| e < now_secs()) {
            continue;
        }
        out.push(Cookie {
            host,
            name,
            value,
            path: cpath,
            secure,
            http_only,
            expires,
            // Chromium: -1 unspecified, 0 none, 1 lax, 2 strict
            same_site: same_site(ss),
        });
    }
    Ok((out, skipped))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Chromium saved logins. Returns the readable ones and how many could not be decrypted.
pub fn chromium_logins(
    path: &Path,
    decryptor: &dyn Decryptor,
) -> Result<(Vec<crate::vault::Draft>, usize), ImportError> {
    let db = open(path)?;
    let mut stmt = db
        .conn()
        .prepare(
            "SELECT origin_url, username_value, password_value FROM logins \
             WHERE blacklisted_by_user = 0 AND password_value IS NOT NULL",
        )
        .map_err(other)?;
    let rows = stmt
        .query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?))
        })
        .map_err(other)?;
    let (mut out, mut skipped) = (Vec::new(), 0);
    for (url, username, blob) in rows.flatten() {
        if !web(&url) {
            continue;
        }
        match decryptor.decrypt(&blob) {
            Some(p) if !p.is_empty() => out.push(crate::vault::Draft {
                url,
                username,
                password: String::from_utf8_lossy(&p).into_owned(),
                ..Default::default()
            }),
            _ => skipped += 1,
        }
    }
    Ok((out, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Xor;
    impl Decryptor for Xor {
        fn load_key(&mut self, _: &Path) -> bool {
            true
        }
        fn decrypt(&self, blob: &[u8]) -> Option<Vec<u8>> {
            blob.strip_prefix(b"v10").map(|b| b.iter().map(|x| x ^ 0x55).collect())
        }
    }

    fn enc(s: &str) -> Vec<u8> {
        let mut v = b"v10".to_vec();
        v.extend(s.bytes().map(|b| b ^ 0x55));
        v
    }

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("shoji_sql_{}_{name}", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    #[test]
    fn chromium_history_converts_times_and_skips_internal_pages() {
        let p = temp("hist");
        let c = Connection::open(&p).unwrap();
        c.execute_batch(
            "CREATE TABLE urls(id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER, last_visit_time INTEGER, hidden INTEGER);
             INSERT INTO urls VALUES (1,'https://a.com/','A',3,13300000000000000,0);
             INSERT INTO urls VALUES (2,'chrome://settings/','S',1,13300000000000000,0);
             INSERT INTO urls VALUES (3,'https://hidden.com/','H',1,13300000000000000,1);",
        )
        .unwrap();
        drop(c);
        let got = chromium_history(&p).unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].url, "https://a.com/");
        assert_eq!(got[0].visits, 3);
        assert_eq!(got[0].last_visit, (13_300_000_000_000_000i64 / 1000 - 11_644_473_600_000) as u64);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn missing_file_is_reported() {
        assert!(matches!(
            chromium_history(Path::new("/definitely/not/here")),
            Err(ImportError::Missing(_))
        ));
    }

    #[test]
    fn chromium_cookies_decrypt_and_count_the_unreadable() {
        let p = temp("cookies");
        let c = Connection::open(&p).unwrap();
        c.execute_batch(
            "CREATE TABLE meta(key TEXT, value TEXT);
             INSERT INTO meta VALUES ('version','20');
             CREATE TABLE cookies(host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB, path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER, samesite INTEGER);",
        )
        .unwrap();
        let far = 13_999_999_999_000_000i64;
        c.execute("INSERT INTO cookies VALUES ('.a.com','sid','',?1,'/',?2,1,1,1)", rusqlite::params![enc("secret"), far]).unwrap();
        c.execute("INSERT INTO cookies VALUES ('.a.com','plain','v',x'','/',0,0,0,-1)", []).unwrap();
        c.execute("INSERT INTO cookies VALUES ('.a.com','locked','',?1,'/',?2,0,0,0)", rusqlite::params![b"v20xxxx".to_vec(), far]).unwrap();
        c.execute("INSERT INTO cookies VALUES ('.a.com','old','x',x'','/',12000000000000000,0,0,0)", []).unwrap();
        drop(c);
        let (got, skipped) = chromium_cookies(&p, &Xor).unwrap();
        assert_eq!(skipped, 1);
        let names: Vec<_> = got.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["sid", "plain"]);
        assert_eq!(got[0].value, "secret");
        assert_eq!(got[0].same_site, "lax");
        assert!(got[0].secure && got[0].http_only);
        assert_eq!(got[1].expires, None);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn chromium_logins_decrypt() {
        let p = temp("logins");
        let c = Connection::open(&p).unwrap();
        c.execute_batch(
            "CREATE TABLE logins(origin_url TEXT, username_value TEXT, password_value BLOB, blacklisted_by_user INTEGER);",
        )
        .unwrap();
        c.execute("INSERT INTO logins VALUES ('https://a.com/','me',?1,0)", [enc("hunter2")]).unwrap();
        c.execute("INSERT INTO logins VALUES ('https://b.com/','you',?1,0)", [b"v20zzz".to_vec()]).unwrap();
        c.execute("INSERT INTO logins VALUES ('https://c.com/','never',?1,1)", [enc("x")]).unwrap();
        c.execute("INSERT INTO logins VALUES ('android://x@com.app','a',?1,0)", [enc("y")]).unwrap();
        drop(c);
        let (got, skipped) = chromium_logins(&p, &Xor).unwrap();
        assert_eq!((got.len(), skipped), (1, 1));
        assert_eq!((got[0].password.as_str(), got[0].username.as_str()), ("hunter2", "me"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn firefox_places_bookmarks_and_history() {
        let p = temp("places");
        let c = Connection::open(&p).unwrap();
        c.execute_batch(
            "CREATE TABLE moz_places(id INTEGER PRIMARY KEY, url TEXT, title TEXT, visit_count INTEGER, hidden INTEGER, last_visit_date INTEGER);
             CREATE TABLE moz_bookmarks(id INTEGER PRIMARY KEY, type INTEGER, fk INTEGER, parent INTEGER, position INTEGER, title TEXT);
             INSERT INTO moz_places VALUES (1,'https://a.com/','A',2,0,1700000000000000);
             INSERT INTO moz_places VALUES (2,'place:folder=1','',0,0,NULL);
             INSERT INTO moz_bookmarks VALUES (1,1,1,3,0,'Bookmark A');
             INSERT INTO moz_bookmarks VALUES (2,1,2,3,1,'Query');",
        )
        .unwrap();
        drop(c);
        let fav = firefox_bookmarks(&p).unwrap();
        assert_eq!(fav.len(), 1);
        assert_eq!(fav[0].title, "Bookmark A");
        let hist = firefox_history(&p).unwrap();
        assert_eq!(hist.len(), 1);
        assert_eq!(hist[0].last_visit, 1_700_000_000_000);
        let _ = std::fs::remove_file(&p);
    }
}
