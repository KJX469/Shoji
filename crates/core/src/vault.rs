//! Shoji's password manager: an encrypted vault of logins.
//!
//! The vault is one file. Its contents are sealed with AES-256-GCM under a random 256-bit data
//! key, and that key is itself protected one of two ways:
//!
//! * **auto** (the default): by the operating system for the current user (DPAPI on Windows),
//!   so it opens by itself for whoever is signed in to this Windows account, like a browser's
//!   own saved passwords;
//! * **master**: by a key derived from a master password (PBKDF2-HMAC-SHA256, 600,000 rounds),
//!   so nothing opens until the password is typed, and it locks again after a quiet spell.
//!
//! The operating-system part is a [`KeyProtector`], which the app supplies, so everything here
//! runs and is tested anywhere.

use crate::userdata::write_atomic;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const AAD: &[u8] = b"shoji-vault-v1";
const PBKDF2_ROUNDS: u32 = 600_000;
const MAX_LOGINS: usize = 20_000;

/// Protects the data key for the current OS user (DPAPI on Windows).
pub trait KeyProtector {
    fn protect(&self, data: &[u8]) -> Option<Vec<u8>>;
    fn unprotect(&self, data: &[u8]) -> Option<Vec<u8>>;
}

#[derive(Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Login {
    pub id: String,
    /// The address the login is for, e.g. `https://example.com/login`
    pub url: String,
    pub title: String,
    pub username: String,
    pub password: String,
    pub note: String,
    /// Unix milliseconds
    pub created: u64,
    pub modified: u64,
    pub last_used: u64,
}

impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Login")
            .field("id", &self.id)
            .field("url", &self.url)
            .field("username", &self.username)
            .field("password", &"<hidden>")
            .finish()
    }
}

impl Login {
    pub fn host(&self) -> String {
        host_of(&self.url)
    }
}

/// What a caller supplies to add or change a login.
#[derive(Clone, Default)]
pub struct Draft {
    pub url: String,
    pub title: String,
    pub username: String,
    pub password: String,
    pub note: String,
}

impl std::fmt::Debug for Draft {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Draft")
            .field("url", &self.url)
            .field("username", &self.username)
            .field("password", &"<hidden>")
            .finish()
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct Data {
    logins: Vec<Login>,
    /// Sites the user said never to offer saving for
    never: Vec<String>,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct File {
    v: u32,
    /// "auto" or "master"
    mode: String,
    salt: String,
    iter: u32,
    /// auto: the OS-protected data key. master: nonce + data key sealed under the derived key.
    wrapped: String,
    nonce: String,
    data: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No vault file yet
    Empty,
    Locked,
    Unlocked,
}

pub struct Vault {
    path: PathBuf,
    file: Option<File>,
    key: Option<[u8; 32]>,
    data: Data,
    last_activity: Instant,
}

/* ------------------------------------------------------------ crypto helpers */

fn random(n: usize) -> Vec<u8> {
    let mut v = vec![0u8; n];
    SystemRandom::new().fill(&mut v).expect("system randomness");
    v
}

fn seal(key: &[u8], plain: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).ok()?);
    let nonce_bytes = random(12);
    let nonce = Nonce::try_assume_unique_for_key(&nonce_bytes).ok()?;
    let mut buf = plain.to_vec();
    key.seal_in_place_append_tag(nonce, Aad::from(AAD), &mut buf)
        .ok()?;
    Some((nonce_bytes, buf))
}

fn open(key: &[u8], nonce: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).ok()?);
    let nonce = Nonce::try_assume_unique_for_key(nonce).ok()?;
    let mut buf = sealed.to_vec();
    let plain = key.open_in_place(nonce, Aad::from(AAD), &mut buf).ok()?;
    Some(plain.to_vec())
}

fn derive(master: &str, salt: &[u8], rounds: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    ring::pbkdf2::derive(
        ring::pbkdf2::PBKDF2_HMAC_SHA256,
        NonZeroU32::new(rounds.max(1)).expect("non-zero"),
        salt,
        master.as_bytes(),
        &mut out,
    );
    out
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            out.push(if i <= c.len() {
                B64[((n >> (18 - 6 * i)) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

fn unb64(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0);
    for c in s.bytes().take_while(|&c| c != b'=') {
        let v = B64.iter().position(|&b| b == c)? as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/* --------------------------------------------------------------- hosts, ids */

/// The host of an address, lower-case, without a leading `www.`. Bare hosts work too.
pub fn host_of(url: &str) -> String {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host).to_ascii_lowercase();
    host.strip_prefix("www.").unwrap_or(&host).to_string()
}

/// Whether a login saved for `saved` may be offered on a page at `page` (both hosts as
/// returned by [`host_of`]): the same site, or one of its subdomains.
pub fn host_matches(saved: &str, page: &str) -> bool {
    if saved.is_empty() || page.is_empty() {
        return false;
    }
    saved == page || (saved.contains('.') && page.ends_with(&format!(".{saved}")))
}

fn new_id() -> String {
    random(8).iter().map(|b| format!("{b:02x}")).collect()
}

/* --------------------------------------------------------------------- vault */

impl Vault {
    pub fn load(path: PathBuf) -> Self {
        let text = crate::userdata::read_text(&path);
        let file = text
            .as_deref()
            .and_then(|s| serde_json::from_str::<File>(s).ok())
            .filter(|f| f.v == 1);
        // A vault file that can't be read is not an empty vault: keep it before a new one is
        // created over it.
        if file.is_none() && text.is_some_and(|t| !t.trim().is_empty()) {
            crate::userdata::preserve_unreadable(&path);
        }
        Self {
            path,
            file,
            key: None,
            data: Data::default(),
            last_activity: Instant::now(),
        }
    }

    pub fn state(&self) -> State {
        match (&self.file, &self.key) {
            (None, _) => State::Empty,
            (Some(_), None) => State::Locked,
            (Some(_), Some(_)) => State::Unlocked,
        }
    }

    pub fn is_unlocked(&self) -> bool {
        self.key.is_some()
    }

    /// Protected by a master password (rather than just the OS account)?
    pub fn has_master(&self) -> bool {
        self.file.as_ref().is_some_and(|f| f.mode == "master")
    }

    pub fn len(&self) -> usize {
        self.data.logins.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.logins.is_empty()
    }

    /// Start a new, empty vault protected by the OS account.
    pub fn create(&mut self, protector: &dyn KeyProtector) -> bool {
        let key: [u8; 32] = random(32).try_into().expect("32 bytes");
        let Some(wrapped) = protector.protect(&key) else {
            return false;
        };
        self.file = Some(File {
            v: 1,
            mode: "auto".into(),
            wrapped: b64(&wrapped),
            ..File::default()
        });
        self.key = Some(key);
        self.data = Data::default();
        self.touch();
        self.save()
    }

    /// Open an OS-protected vault. Does nothing (and returns false) for a master-password one.
    pub fn unlock_auto(&mut self, protector: &dyn KeyProtector) -> bool {
        let Some(f) = self.file.as_ref().filter(|f| f.mode == "auto") else {
            return false;
        };
        let key = unb64(&f.wrapped)
            .and_then(|w| protector.unprotect(&w))
            .and_then(|k| <[u8; 32]>::try_from(k).ok());
        self.finish_unlock(key)
    }

    pub fn unlock_master(&mut self, master: &str) -> bool {
        let Some(f) = self.file.as_ref().filter(|f| f.mode == "master") else {
            return false;
        };
        let (Some(salt), Some(w)) = (unb64(&f.salt), unb64(&f.wrapped)) else {
            return false;
        };
        if w.len() < 12 {
            return false;
        }
        let kek = derive(master, &salt, f.iter);
        let key = open(&kek, &w[..12], &w[12..]).and_then(|k| <[u8; 32]>::try_from(k).ok());
        self.finish_unlock(key)
    }

    fn finish_unlock(&mut self, key: Option<[u8; 32]>) -> bool {
        let Some(key) = key else {
            return false;
        };
        let Some(f) = &self.file else {
            return false;
        };
        let data = unb64(&f.nonce)
            .zip(unb64(&f.data))
            .and_then(|(n, d)| open(&key, &n, &d))
            .and_then(|plain| serde_json::from_slice::<Data>(&plain).ok());
        match data {
            Some(d) => {
                self.data = d;
                self.key = Some(key);
                self.touch();
                true
            }
            None => false,
        }
    }

    pub fn lock(&mut self) {
        self.key = None;
        self.data = Data::default();
    }

    pub fn touch(&mut self) {
        self.last_activity = Instant::now();
    }

    /// When a master-password vault that is open will lock itself, if ever.
    pub fn idle_deadline(&self, after: Duration) -> Option<Instant> {
        (self.is_unlocked() && self.has_master()).then(|| self.last_activity + after)
    }

    /// Lock a master-password vault that has been idle for `after`. Returns true if it locked.
    pub fn lock_if_idle(&mut self, after: Duration) -> bool {
        if self.is_unlocked() && self.has_master() && self.last_activity.elapsed() >= after {
            self.lock();
            return true;
        }
        false
    }

    /// Switch to (or change) a master password. Needs the vault open.
    pub fn set_master(&mut self, master: &str) -> bool {
        let (Some(key), Some(_)) = (self.key, &self.file) else {
            return false;
        };
        if master.chars().count() < 8 {
            return false;
        }
        let salt = random(16);
        let kek = derive(master, &salt, PBKDF2_ROUNDS);
        let Some((nonce, sealed)) = seal(&kek, &key) else {
            return false;
        };
        let mut wrapped = nonce;
        wrapped.extend(sealed);
        if let Some(f) = &mut self.file {
            f.mode = "master".into();
            f.salt = b64(&salt);
            f.iter = PBKDF2_ROUNDS;
            f.wrapped = b64(&wrapped);
        }
        self.save()
    }

    /// Go back to OS-account protection. Needs the vault open.
    pub fn remove_master(&mut self, protector: &dyn KeyProtector) -> bool {
        let (Some(key), Some(_)) = (self.key, &self.file) else {
            return false;
        };
        let Some(wrapped) = protector.protect(&key) else {
            return false;
        };
        if let Some(f) = &mut self.file {
            f.mode = "auto".into();
            f.salt.clear();
            f.iter = 0;
            f.wrapped = b64(&wrapped);
        }
        self.save()
    }

    fn save(&mut self) -> bool {
        let (Some(key), Some(file)) = (self.key, self.file.as_mut()) else {
            return false;
        };
        let Ok(plain) = serde_json::to_vec(&self.data) else {
            return false;
        };
        let Some((nonce, sealed)) = seal(&key, &plain) else {
            return false;
        };
        file.nonce = b64(&nonce);
        file.data = b64(&sealed);
        match serde_json::to_string(file) {
            Ok(json) => write_atomic(&self.path, &json).is_ok(),
            Err(_) => false,
        }
    }

    /* ---- logins ---- */

    pub fn logins(&self) -> &[Login] {
        &self.data.logins
    }

    pub fn get(&self, id: &str) -> Option<&Login> {
        self.data.logins.iter().find(|l| l.id == id)
    }

    /// Logins that may be offered on a page at `host`, most recently used first.
    pub fn for_host(&self, host: &str) -> Vec<&Login> {
        let mut v: Vec<&Login> = self
            .data
            .logins
            .iter()
            .filter(|l| host_matches(&l.host(), host))
            .collect();
        v.sort_by(|a, b| {
            b.last_used
                .max(b.modified)
                .cmp(&a.last_used.max(a.modified))
        });
        v
    }

    pub fn search(&self, query: &str) -> Vec<&Login> {
        let words: Vec<String> = query
            .to_lowercase()
            .split_whitespace()
            .map(String::from)
            .collect();
        let mut v: Vec<&Login> = self
            .data
            .logins
            .iter()
            .filter(|l| {
                let hay = format!("{} {} {}", l.title, l.url, l.username).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .collect();
        v.sort_by_key(|l| (l.host(), l.username.to_lowercase()));
        v
    }

    /// Add a login, or update the one for the same site and username. Returns its id, and
    /// whether it was new. `None` when the vault is closed, the draft is unusable, or it is full.
    pub fn upsert(&mut self, d: &Draft, now: u64) -> Option<(String, bool)> {
        let r = self.upsert_unsaved(d, now)?;
        self.save();
        Some(r)
    }

    fn upsert_unsaved(&mut self, d: &Draft, now: u64) -> Option<(String, bool)> {
        if !self.is_unlocked() || d.password.is_empty() {
            return None;
        }
        let host = host_of(&d.url);
        if host.is_empty() {
            return None;
        }
        self.touch();
        let id = if let Some(l) = self
            .data
            .logins
            .iter_mut()
            .find(|l| l.host() == host && l.username == d.username)
        {
            l.password = d.password.clone();
            if !d.note.is_empty() {
                l.note = d.note.clone();
            }
            if !d.title.is_empty() {
                l.title = d.title.clone();
            }
            l.modified = now;
            (l.id.clone(), false)
        } else {
            if self.data.logins.len() >= MAX_LOGINS {
                return None;
            }
            let id = new_id();
            self.data.logins.push(Login {
                id: id.clone(),
                url: d.url.trim().to_string(),
                title: if d.title.trim().is_empty() {
                    host.clone()
                } else {
                    d.title.trim().to_string()
                },
                username: d.username.clone(),
                password: d.password.clone(),
                note: d.note.clone(),
                created: now,
                modified: now,
                last_used: 0,
            });
            (id, true)
        };
        Some(id)
    }

    /// Replace every field of an existing login (the edit form).
    pub fn edit(&mut self, id: &str, d: &Draft, now: u64) -> bool {
        if !self.is_unlocked() || d.password.is_empty() || host_of(&d.url).is_empty() {
            return false;
        }
        self.touch();
        let Some(l) = self.data.logins.iter_mut().find(|l| l.id == id) else {
            return false;
        };
        l.url = d.url.trim().to_string();
        l.title = if d.title.trim().is_empty() {
            host_of(&d.url)
        } else {
            d.title.trim().to_string()
        };
        l.username = d.username.clone();
        l.password = d.password.clone();
        l.note = d.note.clone();
        l.modified = now;
        self.save()
    }

    pub fn remove(&mut self, id: &str) -> bool {
        self.touch();
        let before = self.data.logins.len();
        self.data.logins.retain(|l| l.id != id);
        let changed = self.data.logins.len() != before;
        if changed {
            self.save();
        }
        changed
    }

    pub fn mark_used(&mut self, id: &str, now: u64) {
        if let Some(l) = self.data.logins.iter_mut().find(|l| l.id == id) {
            l.last_used = now;
            self.save();
        }
    }

    pub fn delete_all(&mut self) {
        self.data.logins.clear();
        self.save();
    }

    /// Add many at once (an import), saving once. Returns (added, updated, skipped).
    pub fn import(&mut self, drafts: &[Draft], now: u64) -> (usize, usize, usize) {
        let (mut added, mut updated, mut skipped) = (0, 0, 0);
        for d in drafts {
            let same = self.data.logins.iter().any(|l| {
                l.host() == host_of(&d.url) && l.username == d.username && l.password == d.password
            });
            if same || !self.is_unlocked() {
                skipped += 1;
                continue;
            }
            match self.upsert_unsaved(d, now) {
                Some((_, true)) => added += 1,
                Some((_, false)) => updated += 1,
                None => skipped += 1,
            }
        }
        self.save();
        (added, updated, skipped)
    }

    /* ---- never save ---- */

    pub fn is_never(&self, host: &str) -> bool {
        self.data.never.iter().any(|h| h == host)
    }

    pub fn never_save(&mut self, host: &str) {
        if !host.is_empty() && !self.is_never(host) && self.is_unlocked() {
            self.data.never.push(host.to_string());
            self.save();
        }
    }

    pub fn forget_never(&mut self) {
        self.data.never.clear();
        self.save();
    }

    /* ---- health ---- */

    /// Weak passwords, and groups of logins sharing one password. Ids only.
    pub fn audit(&self) -> Audit {
        let weak = self
            .data
            .logins
            .iter()
            .filter(|l| strength(&l.password) < 2)
            .map(|l| l.id.clone())
            .collect();
        let mut by_pw: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for l in &self.data.logins {
            by_pw
                .entry(l.password.as_str())
                .or_default()
                .push(l.id.clone());
        }
        let reused = by_pw.into_values().filter(|g| g.len() > 1).collect();
        Audit { weak, reused }
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct Audit {
    pub weak: Vec<String>,
    pub reused: Vec<Vec<String>>,
}

/* ------------------------------------------------------- strength, generator */

const COMMON: &[&str] = &[
    "password",
    "123456",
    "12345678",
    "123456789",
    "qwerty",
    "abc123",
    "letmein",
    "111111",
    "iloveyou",
    "admin",
    "welcome",
    "monkey",
    "dragon",
    "football",
    "baseball",
    "master",
    "sunshine",
    "princess",
    "passw0rd",
    "trustno1",
    "000000",
    "1234567",
    "qwertyuiop",
    "654321",
    "superman",
    "michael",
];

/// A rough 0 (very weak) to 4 (strong) score from length, variety and obvious patterns.
pub fn strength(pw: &str) -> u8 {
    let lower = pw.to_lowercase();
    if COMMON.iter().any(|c| lower.contains(c)) && pw.chars().count() < 14 {
        return 0;
    }
    let n = pw.chars().count();
    let pool = [
        (pw.chars().any(|c| c.is_ascii_lowercase()), 26.0),
        (pw.chars().any(|c| c.is_ascii_uppercase()), 26.0),
        (pw.chars().any(|c| c.is_ascii_digit()), 10.0),
        (pw.chars().any(|c| !c.is_alphanumeric()), 33.0),
    ]
    .iter()
    .filter(|(on, _)| *on)
    .map(|(_, n)| n)
    .sum::<f64>();
    let distinct = pw.chars().collect::<std::collections::BTreeSet<_>>().len();
    if distinct <= 2 {
        return 0;
    }
    // People are far less random than the full alphabet allows, so count about 60% of it.
    let bits = (n as f64) * pool.max(2.0).log2() * 0.6;
    match bits {
        b if b < 28.0 => 0,
        b if b < 38.0 => 1,
        b if b < 54.0 => 2,
        b if b < 72.0 => 3,
        _ => 4,
    }
}

#[derive(Clone, Copy)]
pub struct GenOptions {
    pub length: usize,
    pub lower: bool,
    pub upper: bool,
    pub digits: bool,
    pub symbols: bool,
}

impl Default for GenOptions {
    fn default() -> Self {
        Self {
            length: 20,
            lower: true,
            upper: true,
            digits: true,
            symbols: true,
        }
    }
}

/// A random password with at least one character of every class that is switched on. Easily
/// confused characters (`0 O o 1 l I`) are left out.
pub fn generate(o: GenOptions) -> String {
    let sets: Vec<&[u8]> = [
        (o.lower, &b"abcdefghijkmnpqrstuvwxyz"[..]),
        (o.upper, &b"ABCDEFGHJKLMNPQRSTUVWXYZ"[..]),
        (o.digits, &b"23456789"[..]),
        (o.symbols, &b"!@#$%^&*-_=+?"[..]),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .map(|(_, s)| s)
    .collect();
    let sets = if sets.is_empty() {
        vec![&b"abcdefghijkmnpqrstuvwxyz"[..]]
    } else {
        sets
    };
    let len = o.length.clamp(8, 128).max(sets.len());
    let all: Vec<u8> = sets.iter().flat_map(|s| s.iter().copied()).collect();
    // Unbiased pick from `n` options by rejection sampling.
    let pick = |n: usize| -> usize {
        let limit = 256 - (256 % n);
        loop {
            let b = random(1)[0] as usize;
            if b < limit {
                return b % n;
            }
        }
    };
    let mut chars: Vec<u8> = sets.iter().map(|s| s[pick(s.len())]).collect();
    while chars.len() < len {
        chars.push(all[pick(all.len())]);
    }
    for i in (1..chars.len()).rev() {
        chars.swap(i, pick(i + 1));
    }
    String::from_utf8(chars).expect("ascii")
}

/* ------------------------------------------------------------------------ CSV */

fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let (mut rows, mut row, mut field) = (Vec::new(), Vec::new(), String::new());
    let (mut quoted, mut chars) = (
        false,
        text.trim_start_matches('\u{feff}').chars().peekable(),
    );
    while let Some(c) = chars.next() {
        match (c, quoted) {
            ('"', true) if chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            ('"', _) => quoted = !quoted,
            (',', false) => row.push(std::mem::take(&mut field)),
            ('\r', false) => {}
            ('\n', false) => {
                row.push(std::mem::take(&mut field));
                rows.push(std::mem::take(&mut row));
            }
            (c, _) => field.push(c),
        }
    }
    if !field.is_empty() || !row.is_empty() {
        row.push(field);
        rows.push(row);
    }
    rows
}

/// Logins from a CSV export of Chrome, Edge, Firefox, Bitwarden, 1Password, LastPass or
/// Shoji itself. Columns are found by name, so the exact layout doesn't matter.
pub fn drafts_from_csv(text: &str) -> Vec<Draft> {
    let rows = parse_csv(text);
    let Some((head, body)) = rows.split_first() else {
        return Vec::new();
    };
    let find = |names: &[&str]| {
        head.iter()
            .position(|h| names.contains(&h.trim().to_lowercase().as_str()))
    };
    let (Some(url), Some(pass)) = (
        find(&[
            "url",
            "login_uri",
            "website",
            "origin url",
            "web site",
            "address",
        ]),
        find(&["password", "login_password"]),
    ) else {
        return Vec::new();
    };
    let user = find(&["username", "login_username", "user", "login", "email"]);
    let title = find(&["name", "title"]);
    let note = find(&["note", "notes", "extra", "comments"]);
    body.iter()
        .filter_map(|r| {
            let get = |i: Option<usize>| i.and_then(|i| r.get(i)).cloned().unwrap_or_default();
            let d = Draft {
                url: get(Some(url)),
                title: get(title),
                username: get(user),
                password: get(Some(pass)),
                note: get(note),
            };
            (!host_of(&d.url).is_empty() && !d.password.is_empty() && d.url != "android://")
                .then_some(d)
        })
        .collect()
}

impl Vault {
    /// Every login as CSV (the same layout Chrome and Edge use), for moving to another manager.
    pub fn to_csv(&self) -> String {
        let q = |s: &str| format!("\"{}\"", s.replace('"', "\"\""));
        let mut out = String::from("name,url,username,password,note\n");
        for l in &self.data.logins {
            out.push_str(&format!(
                "{},{},{},{},{}\n",
                q(&l.title),
                q(&l.url),
                q(&l.username),
                q(&l.password),
                q(&l.note)
            ));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in for DPAPI: reversible, and different from the plain key.
    struct Fake;
    impl KeyProtector for Fake {
        fn protect(&self, d: &[u8]) -> Option<Vec<u8>> {
            Some(d.iter().map(|b| b ^ 0xA5).collect())
        }
        fn unprotect(&self, d: &[u8]) -> Option<Vec<u8>> {
            self.protect(d)
        }
    }
    struct Broken;
    impl KeyProtector for Broken {
        fn protect(&self, _: &[u8]) -> Option<Vec<u8>> {
            None
        }
        fn unprotect(&self, _: &[u8]) -> Option<Vec<u8>> {
            None
        }
    }

    fn path(n: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("shoji_vault_{}_{n}.json", std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    fn draft(url: &str, user: &str, pw: &str) -> Draft {
        Draft {
            url: url.into(),
            username: user.into(),
            password: pw.into(),
            ..Draft::default()
        }
    }

    #[test]
    fn base64_round_trips() {
        for data in [
            &b""[..],
            b"a",
            b"ab",
            b"abc",
            b"abcd",
            &[0u8, 255, 128, 7, 9],
        ] {
            assert_eq!(unb64(&b64(data)).unwrap(), data);
        }
    }

    #[test]
    fn auto_vault_saves_reopens_and_never_stores_plaintext() {
        let p = path("auto");
        let mut v = Vault::load(p.clone());
        assert_eq!(v.state(), State::Empty);
        assert!(!v.create(&Broken));
        assert!(v.create(&Fake));
        let (id, new) = v
            .upsert(
                &draft("https://example.com/login", "me", "hunter2-hunter2"),
                5,
            )
            .unwrap();
        assert!(new);
        let raw = std::fs::read_to_string(&p).unwrap();
        assert!(!raw.contains("hunter2") && !raw.contains("example.com"));

        let mut again = Vault::load(p.clone());
        assert_eq!(again.state(), State::Locked);
        assert!(!again.unlock_master("anything"));
        assert!(again.unlock_auto(&Fake));
        assert_eq!(again.get(&id).unwrap().password, "hunter2-hunter2");
        assert_eq!(again.for_host("www.example.com").len(), 1);
        assert_eq!(again.for_host("login.example.com").len(), 1);
        assert!(again.for_host("evil-example.com").is_empty());
        assert!(again.for_host("com").is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn same_site_and_user_updates_instead_of_duplicating() {
        let p = path("dup");
        let mut v = Vault::load(p.clone());
        v.create(&Fake);
        let (a, _) = v
            .upsert(&draft("https://a.com", "me", "one-one-one"), 1)
            .unwrap();
        let (b, new) = v
            .upsert(&draft("https://www.a.com/x", "me", "two-two-two"), 2)
            .unwrap();
        assert_eq!((a.clone(), new), (b, false));
        assert_eq!(
            (v.len(), v.get(&a).unwrap().password.as_str()),
            (1, "two-two-two")
        );
        assert!(
            v.upsert(&draft("https://a.com", "you", "x-x-x-x"), 3)
                .unwrap()
                .1
        );
        assert!(v.upsert(&draft("https://a.com", "z", ""), 3).is_none());
        assert!(v.upsert(&draft("", "z", "pw"), 3).is_none());
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn master_password_locks_and_unlocks() {
        let p = path("master");
        let mut v = Vault::load(p.clone());
        v.create(&Fake);
        v.upsert(&draft("https://a.com", "me", "secret-secret"), 1);
        assert!(!v.set_master("short"));
        assert!(v.set_master("correct horse battery"));
        assert!(v.has_master());
        v.lock();
        assert!(v.logins().is_empty());

        let mut again = Vault::load(p.clone());
        assert!(!again.unlock_auto(&Fake));
        assert!(!again.unlock_master("wrong password!"));
        assert_eq!(again.state(), State::Locked);
        assert!(again.unlock_master("correct horse battery"));
        assert_eq!(again.len(), 1);
        assert!(again.lock_if_idle(Duration::ZERO));
        assert!(!again.is_unlocked());
        assert!(again.unlock_master("correct horse battery"));
        assert!(again.remove_master(&Fake));
        let mut third = Vault::load(p.clone());
        assert!(third.unlock_auto(&Fake));
        assert_eq!(third.len(), 1);
        // Without a master password there is nothing to idle-lock.
        assert!(!third.lock_if_idle(Duration::ZERO));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn a_tampered_file_does_not_open() {
        let p = path("tamper");
        let mut v = Vault::load(p.clone());
        v.create(&Fake);
        v.upsert(&draft("https://a.com", "me", "secret-secret"), 1);
        let mut f: File = serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
        let mut data = unb64(&f.data).unwrap();
        data[3] ^= 1;
        f.data = b64(&data);
        std::fs::write(&p, serde_json::to_string(&f).unwrap()).unwrap();
        let mut bad = Vault::load(p.clone());
        assert!(!bad.unlock_auto(&Fake));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn closed_vault_refuses_changes() {
        let mut v = Vault::load(path("closed"));
        assert!(v
            .upsert(&draft("https://a.com", "me", "pw-pw-pw-pw"), 1)
            .is_none());
        assert_eq!(
            v.import(&[draft("https://a.com", "me", "pw")], 1),
            (0, 0, 1)
        );
    }

    #[test]
    fn edit_remove_and_never() {
        let p = path("edit");
        let mut v = Vault::load(p.clone());
        v.create(&Fake);
        let (id, _) = v
            .upsert(&draft("https://a.com", "me", "pw-pw-pw-pw"), 1)
            .unwrap();
        assert!(v.edit(
            &id,
            &Draft {
                url: "https://b.com".into(),
                username: "you".into(),
                password: "new-new-new".into(),
                title: "B".into(),
                note: "n".into()
            },
            9
        ));
        assert_eq!(
            (
                v.get(&id).unwrap().title.as_str(),
                v.get(&id).unwrap().modified
            ),
            ("B", 9)
        );
        assert!(!v.edit("nope", &draft("https://x.com", "", "p"), 1));
        v.never_save("spam.com");
        assert!(v.is_never("spam.com"));
        assert!(v.remove(&id) && !v.remove(&id));
        let mut again = Vault::load(p.clone());
        again.unlock_auto(&Fake);
        assert!(again.is_empty() && again.is_never("spam.com"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn audit_finds_weak_and_reused() {
        let p = path("audit");
        let mut v = Vault::load(p.clone());
        v.create(&Fake);
        let strong = generate(GenOptions::default());
        v.upsert(&draft("https://a.com", "me", "password"), 1);
        v.upsert(&draft("https://b.com", "me", &strong), 1);
        v.upsert(&draft("https://c.com", "me", &strong), 1);
        let a = v.audit();
        assert_eq!(a.weak.len(), 1);
        assert_eq!(a.reused.len(), 1);
        assert_eq!(a.reused[0].len(), 2);
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn strength_scores_sensibly() {
        assert_eq!(strength("password"), 0);
        assert_eq!(strength("aaaaaaaaaaaa"), 0);
        assert!(strength("abc123") <= 1);
        assert!(strength("Tr0ub4dor&3") >= 2);
        assert!(strength("correct horse battery staple") >= 3);
        assert_eq!(
            strength(&generate(GenOptions {
                length: 24,
                ..GenOptions::default()
            })),
            4
        );
    }

    #[test]
    fn generator_honours_its_options() {
        for _ in 0..50 {
            let pw = generate(GenOptions {
                length: 12,
                lower: true,
                upper: true,
                digits: true,
                symbols: true,
            });
            assert_eq!(pw.chars().count(), 12);
            assert!(pw.chars().any(|c| c.is_ascii_lowercase()));
            assert!(pw.chars().any(|c| c.is_ascii_uppercase()));
            assert!(pw.chars().any(|c| c.is_ascii_digit()));
            assert!(pw.chars().any(|c| !c.is_alphanumeric()));
            assert!(!pw.contains(['0', 'O', 'o', '1', 'l', 'I']));
        }
        let digits = generate(GenOptions {
            length: 16,
            lower: false,
            upper: false,
            digits: true,
            symbols: false,
        });
        assert!(digits.chars().all(|c| c.is_ascii_digit()));
        assert_ne!(
            generate(GenOptions::default()),
            generate(GenOptions::default())
        );
        assert_eq!(
            generate(GenOptions {
                length: 1000,
                ..GenOptions::default()
            })
            .len(),
            128
        );
    }

    #[test]
    fn csv_from_several_managers() {
        let chrome = "name,url,username,password,note\n\"A\",https://a.com/,me,\"p,\"\"1\"\"\",\n";
        let d = drafts_from_csv(chrome);
        assert_eq!(
            (d.len(), d[0].password.as_str(), d[0].title.as_str()),
            (1, "p,\"1\"", "A")
        );
        let firefox = "\"url\",\"username\",\"password\",\"httpRealm\",\"formActionOrigin\",\"guid\"\n\"https://b.com\",\"you\",\"pw2\",\"\",\"https://b.com\",\"{x}\"\n";
        assert_eq!(drafts_from_csv(firefox)[0].username, "you");
        let bitwarden = "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\n,,login,Site,note,,0,https://c.com,u,pw3,\n";
        let d = drafts_from_csv(bitwarden);
        assert_eq!(
            (d[0].url.as_str(), d[0].note.as_str()),
            ("https://c.com", "note")
        );
        assert!(drafts_from_csv("a,b\n1,2\n").is_empty());
        assert!(drafts_from_csv("").is_empty());
        let multiline = "url,username,password,note\r\nhttps://d.com,u,pw,\"line1\nline2\"\r\n";
        assert_eq!(drafts_from_csv(multiline)[0].note, "line1\nline2");
    }

    #[test]
    fn export_then_import_round_trips() {
        let (p1, p2) = (path("e1"), path("e2"));
        let mut a = Vault::load(p1.clone());
        a.create(&Fake);
        a.upsert(
            &Draft {
                url: "https://a.com".into(),
                title: "A \"quoted\"".into(),
                username: "me".into(),
                password: "pw,with,commas".into(),
                note: "x\ny".into(),
            },
            1,
        );
        let csv = a.to_csv();
        let mut b = Vault::load(p2.clone());
        b.create(&Fake);
        assert_eq!(b.import(&drafts_from_csv(&csv), 2), (1, 0, 0));
        assert_eq!(b.logins()[0].password, "pw,with,commas");
        assert_eq!(b.logins()[0].note, "x\ny");
        // Importing the same file again changes nothing.
        assert_eq!(b.import(&drafts_from_csv(&csv), 3), (0, 0, 1));
        let _ = (std::fs::remove_file(&p1), std::fs::remove_file(&p2));
    }

    #[test]
    fn hosts_are_compared_by_site() {
        assert_eq!(
            host_of("https://User:x@WWW.Example.com:8443/a?b#c"),
            "example.com"
        );
        assert_eq!(host_of("example.com"), "example.com");
        assert!(host_matches("example.com", "accounts.example.com"));
        assert!(!host_matches("example.com", "notexample.com"));
        assert!(!host_matches("localhost", "a.localhost"));
        assert!(!host_matches("", "a.com"));
    }

    #[test]
    fn debug_never_prints_the_password() {
        let l = Login {
            password: "hunter2".into(),
            ..Login::default()
        };
        assert!(!format!("{l:?}").contains("hunter2"));
    }
}
