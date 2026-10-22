//! The matcher: network decisions and per-site cosmetic payloads.

use crate::cname::{self, Uncloaker};
use crate::popups::Popups;
use crate::sources::ListSource;
use crate::updater::Updater;
use adblock::lists::{FilterSet, ParseOptions};
use adblock::request::Request;
use adblock::resources::{PermissionMask, Resource};
use adblock::Engine;
use std::collections::HashMap;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};

/// The compiled engine kept between runs, and the key of what it was compiled from.
const SAVED_ENGINE: &str = "engine.dat";
const SAVED_KEY: &str = "engine.key";

/// How firmly the shield acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Mode {
    Off,
    #[default]
    Standard,
    /// Standard, and tracking parameters are stripped from every request.
    Aggressive,
}

/// What kind of request the browser reports (mapped from the WebView2 resource context).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RequestKind {
    Document,
    Subdocument,
    Stylesheet,
    Image,
    Media,
    Font,
    Script,
    Xhr,
    Ping,
    Websocket,
    /// A new window (`window.open`, `target="_blank"`), matched with `$popup` rules
    Popup,
    /// A file download, matched with page (`$document`) rules
    Download,
    Other,
}

impl RequestKind {
    fn name(self) -> &'static str {
        match self {
            RequestKind::Document => "main_frame",
            RequestKind::Subdocument => "sub_frame",
            RequestKind::Stylesheet => "stylesheet",
            RequestKind::Image => "image",
            RequestKind::Media => "media",
            RequestKind::Font => "font",
            RequestKind::Script => "script",
            RequestKind::Xhr => "xhr",
            RequestKind::Ping => "ping",
            RequestKind::Websocket => "websocket",
            RequestKind::Popup => "popup",
            RequestKind::Download => "other",
            RequestKind::Other => "other",
        }
    }
}

/// What to do with a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Block,
    /// Serve this redirect stub instead (a `data:` URL).
    Redirect(String),
    /// Let it through, but to this URL (tracking parameters removed).
    Rewrite(String),
}

/// Everything a page needs at document start.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CosmeticPayload {
    /// Selectors to hide (`display: none`)
    pub hide: Vec<String>,
    /// JSON-encoded procedural filters (`:has-text`, `:upward`, style/remove actions, ...)
    pub procedural: Vec<String>,
    /// Scriptlet source to run at document start
    pub script: String,
    /// The site opted out of generic (class/id based) rules
    pub generichide: bool,
}

/// A hot-swappable engine. Cheap to clone and share across threads.
#[derive(Clone)]
pub struct Shield {
    engine: Arc<RwLock<Option<Arc<Engine>>>>,
    /// Exceptions of the most recently asked-about page, for generic class/id lookups.
    exceptions: Arc<RwLock<(String, Arc<HashSet<String>>)>>,
    /// Hosts the user chose to open despite a blocked-page warning (this session only)
    allowed_pages: Arc<RwLock<HashSet<String>>>,
    /// Bumped whenever a new engine is swapped in; caches of its answers check it
    generation: Arc<AtomicU64>,
    /// Cosmetic payloads (and their exceptions) by host and mode, for the current engine
    cosmetics: Arc<Mutex<CosmeticCache>>,
    /// The lists' `$popup` rules, which the engine itself drops
    popups: Arc<RwLock<Option<Arc<Popups>>>>,
    /// Canonical names of first-party subdomains, once lookups are switched on
    cname: Arc<OnceLock<Uncloaker>>,
    /// The page-level rules (`$document`, `$doc`, `$all`) with their text, for warnings
    documents: Arc<RwLock<Option<Arc<Engine>>>>,
    /// The current lists compiled again with every rule's text, built only while the logger
    /// needs to name rules (the engine above keeps none: about half its memory)
    named: Arc<Mutex<Named>>,
    /// How to read the current lists again (for `named`)
    recipe: Arc<RwLock<Option<Recipe>>>,
    /// Size of the current engine, saved, in bytes
    size: Arc<AtomicU64>,
}

/// Reads the lists the current engine was built from.
type Recipe = Arc<dyn Fn() -> Vec<(String, bool)> + Send + Sync>;

#[derive(Default)]
struct Named {
    engine: Option<(u64, Arc<Engine>)>,
    building: bool,
}

type Exceptions = Arc<HashSet<String>>;

#[derive(Default)]
struct CosmeticCache {
    generation: u64,
    by_host: HashMap<(String, Mode), (CosmeticPayload, Exceptions)>,
}

/// How many hosts' cosmetic payloads are kept.
const COSMETIC_HOSTS: usize = 256;

impl std::hash::Hash for Mode {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (*self as u8).hash(state)
    }
}

impl Default for Shield {
    fn default() -> Self {
        Self::new()
    }
}

/// Compile list texts (`(text, trusted)`) into a matcher; `named` keeps each rule's text.
fn compile(lists: Vec<(String, bool)>, named: bool) -> Engine {
    let mut set = FilterSet::new(named);
    for (text, trusted) in lists {
        let permissions = if trusted {
            PermissionMask::from_bits(1)
        } else {
            PermissionMask::default()
        };
        set.add_filter_list(
            text,
            ParseOptions {
                permissions,
                ..ParseOptions::default()
            },
        );
    }
    Engine::new_with_filter_set(set)
}

/// The rules of `lists` that act on whole pages: those whose options name the document.
fn page_rules(lists: &[(String, bool)]) -> Vec<(String, bool)> {
    lists
        .iter()
        .map(|(text, trusted)| {
            let picked: Vec<&str> = text
                .lines()
                .filter(|l| {
                    let l = l.trim();
                    !l.starts_with('!')
                        && !l.contains("##")
                        && l.rsplit_once('$').is_some_and(|(_, opts)| {
                            opts.split(',')
                                .any(|o| matches!(o.trim(), "document" | "doc" | "all"))
                        })
                })
                .collect();
            (picked.join("\n"), *trusted)
        })
        .filter(|(t, _)| !t.is_empty())
        .collect()
}

/// Identifies a set of lists, and the build of Shoji that compiled them.
fn engine_key(lists: &[(String, bool)]) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    lists.hash(&mut h);
    format!(
        "{} lean {:016x} {}",
        env!("CARGO_PKG_VERSION"),
        h.finish(),
        lists.len()
    )
}

impl Shield {
    pub fn new() -> Self {
        Self {
            engine: Arc::new(RwLock::new(None)),
            exceptions: Arc::new(RwLock::new((String::new(), Arc::new(HashSet::new())))),
            allowed_pages: Arc::new(RwLock::new(HashSet::new())),
            generation: Arc::new(AtomicU64::new(0)),
            cosmetics: Arc::default(),
            popups: Arc::default(),
            cname: Arc::default(),
            documents: Arc::default(),
            named: Arc::default(),
            recipe: Arc::default(),
            size: Arc::default(),
        }
    }

    /// Size of the current engine, saved, in bytes (0 until one is loaded).
    pub fn engine_bytes(&self) -> u64 {
        self.size.load(Ordering::Relaxed)
    }

    /// Match first-party subdomains under their canonical names too, looked up by `u`.
    pub fn uncloak_with(&self, u: Uncloaker) {
        let _ = self.cname.set(u);
    }

    /// Changes whenever a request's decision may have changed: a new engine, or a host found
    /// to be (or no longer be) a CNAME for another site. Caches of decisions check it.
    pub fn decision_generation(&self) -> u64 {
        let cname = self.cname.get().map_or(0, |u| u.epoch());
        self.generation()
            .wrapping_add(cname.wrapping_mul(0x9e37_79b9_7f4a_7c15))
    }

    /// Changes whenever a different engine (new lists, new rules) is in use.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn is_ready(&self) -> bool {
        self.engine.read().map(|e| e.is_some()).unwrap_or(false)
    }

    /// Swap in an engine built from raw list texts. Heavy: call off the UI thread.
    /// `lists` is `(text, trusted)`. Returns false when there was nothing to load.
    pub fn load(&self, lists: Vec<(String, bool)>, resource_jsons: &[String]) -> bool {
        let again = lists.clone();
        self.set_recipe(Arc::new(move || again.clone()));
        self.load_saved(lists, resource_jsons, None)
    }

    fn set_recipe(&self, r: Recipe) {
        if let Ok(mut slot) = self.recipe.write() {
            *slot = Some(r);
        }
    }

    /// [`Shield::load`], reusing the engine compiled from the same lists last time when `dir`
    /// holds one, and saving it there otherwise. Loading a saved engine takes milliseconds where
    /// compiling takes seconds, so pages opened right after start are protected.
    pub fn load_saved(
        &self,
        lists: Vec<(String, bool)>,
        resource_jsons: &[String],
        dir: Option<&Path>,
    ) -> bool {
        if lists.is_empty() {
            return false;
        }
        let key = engine_key(&lists);
        let documents = compile(page_rules(&lists), true);
        let popups = Popups::compile(&lists).map(Arc::new);
        let saved = dir.and_then(|d| {
            let same = std::fs::read_to_string(d.join(SAVED_KEY)).ok()? == key;
            let data = std::fs::read(d.join(SAVED_ENGINE)).ok().filter(|_| same)?;
            let mut engine = Engine::default();
            engine.deserialize(&data).ok()?;
            Some(engine)
        });
        let mut engine = match saved {
            Some(engine) => engine,
            None => {
                let engine = compile(lists, false);
                if let Some(d) = dir {
                    // Key last, so a half-written engine is never taken for a good one.
                    let _ = std::fs::remove_file(d.join(SAVED_KEY));
                    if std::fs::write(d.join(SAVED_ENGINE), engine.serialize()).is_ok() {
                        let _ = std::fs::write(d.join(SAVED_KEY), &key);
                    }
                }
                engine
            }
        };
        self.size
            .store(engine.serialize().len() as u64, Ordering::Relaxed);
        let mut seen = std::collections::HashSet::new();
        let mut resources: Vec<Resource> = Vec::new();
        for json in resource_jsons {
            for r in serde_json::from_str::<Vec<Resource>>(json).unwrap_or_default() {
                if seen.insert(r.name.clone()) {
                    resources.push(r);
                }
            }
        }
        engine.use_resources(resources);
        if let Ok(mut slot) = self.engine.write() {
            *slot = Some(Arc::new(engine));
        }
        if let Ok(mut slot) = self.documents.write() {
            *slot = Some(Arc::new(documents));
        }
        if let Ok(mut slot) = self.popups.write() {
            *slot = popups;
        }
        self.generation.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut n) = self.named.lock() {
            n.engine = None;
        }
        true
    }

    /// Build from the updater's cache and swap in. Returns the number of lists loaded.
    pub fn rebuild(&self, updater: &Updater, sources: &[ListSource]) -> usize {
        self.rebuild_with(updater, sources, "")
    }

    /// [`Shield::rebuild`] with the user's own rules appended (trusted: they wrote them).
    pub fn rebuild_with(
        &self,
        updater: &Updater,
        sources: &[ListSource],
        own_rules: &str,
    ) -> usize {
        let mut lists: Vec<(String, bool)> = sources
            .iter()
            .filter_map(|s| updater.list_text(&s.id).map(|t| (t, s.trusted)))
            .collect();
        if !own_rules.trim().is_empty() {
            lists.push((own_rules.to_string(), true));
        }
        {
            let dir = updater.dir().to_path_buf();
            let ids: Vec<(String, bool)> =
                sources.iter().map(|s| (s.id.clone(), s.trusted)).collect();
            let own = own_rules.to_string();
            self.set_recipe(Arc::new(move || {
                let u = Updater::new(&dir);
                let mut l: Vec<(String, bool)> = ids
                    .iter()
                    .filter_map(|(id, t)| u.list_text(id).map(|text| (text, *t)))
                    .collect();
                if !own.trim().is_empty() {
                    l.push((own.clone(), true));
                }
                l
            }));
        }
        let n = lists.len();
        if self.load_saved(lists, &updater.resource_jsons(), Some(updater.dir())) {
            n
        } else {
            0
        }
    }

    fn current(&self) -> Option<Arc<Engine>> {
        self.engine.read().ok()?.clone()
    }

    /// Decide on a request. `source_url` is the top-level page URL.
    pub fn check(&self, url: &str, source_url: &str, kind: RequestKind, mode: Mode) -> Decision {
        self.check_detailed(url, source_url, kind, mode).0
    }

    /// [`Shield::check`], plus the filter line that decided it (for the logger).
    pub fn check_detailed(
        &self,
        url: &str,
        source_url: &str,
        kind: RequestKind,
        mode: Mode,
    ) -> (Decision, Option<String>) {
        self.check_request(url, source_url, kind, mode, "GET")
    }

    /// [`Shield::check_detailed`] for a request made with `method` (`$method=` rules need it).
    pub fn check_request(
        &self,
        url: &str,
        source_url: &str,
        kind: RequestKind,
        mode: Mode,
        method: &str,
    ) -> (Decision, Option<String>) {
        if mode == Mode::Off || kind == RequestKind::Document {
            return (Decision::Allow, None);
        }
        let Some(engine) = self.current() else {
            return (Decision::Allow, None);
        };
        let Ok(req) = Request::new(url, source_url, kind.name(), method) else {
            return (Decision::Allow, None);
        };
        let r = engine.check_network_request(&req);
        let blocked = r.should_block();
        // The engine keeps no rule text; `rule_for` names the rule when the logger asks.
        let rule = r.filter.and_then(|f| f.raw_line);
        if let Some(redirect) = r.redirect {
            return (Decision::Redirect(redirect), rule);
        }
        if blocked {
            return (Decision::Block, rule);
        }
        if r.exception.is_none() && !req.is_third_party {
            if let Some(answer) = self.uncloaked(&engine, &req, source_url, kind, method) {
                return answer;
            }
        }
        match r.rewritten_url {
            Some(u) if u != url => (Decision::Rewrite(u), rule),
            // Aggressive: tracking parameters leave on every request, not just links.
            _ if mode == Mode::Aggressive && url.contains('?') => {
                match crate::links::strip_tracking(url) {
                    Some(clean) => (Decision::Rewrite(clean), None),
                    None => (Decision::Allow, None),
                }
            }
            _ => (Decision::Allow, None),
        }
    }

    /// The decision for a first-party request whose host is a CNAME for another site, matched
    /// under that site's name. `None` when it isn't cloaked, isn't known yet, or is allowed.
    fn uncloaked(
        &self,
        engine: &Engine,
        req: &Request,
        source_url: &str,
        kind: RequestKind,
        method: &str,
    ) -> Option<(Decision, Option<String>)> {
        let u = self.cname.get()?;
        let host = req.hostname.as_str();
        // The page's own host serves the page; only its other subdomains are looked up.
        if !cname::worth_resolving(host)
            || url::Url::parse(source_url).ok()?.host_str() == Some(host)
        {
            return None;
        }
        let canonical = u.canonical(host)?;
        let replay = cname::origin_with_host(&req.url, &canonical)?;
        let creq = Request::new(&replay, source_url, kind.name(), method).ok()?;
        // A CNAME within the same site hides nothing.
        if !creq.is_third_party {
            return None;
        }
        let r = engine.check_network_request(&creq);
        // An exception written for the visible name still applies (as in Brave).
        if r.filter.is_some()
            && !r.important
            && engine
                .check_network_request_subset(req, true, true)
                .exception
                .is_some()
        {
            return None;
        }
        let blocked = r.should_block();
        let rule = r.filter.and_then(|f| f.raw_line);
        match r.redirect {
            Some(redirect) => Some((Decision::Redirect(redirect), rule)),
            None if blocked => Some((Decision::Block, rule)),
            None => None,
        }
    }

    /// The filter line that blocks opening `url` as a page (a `$document` rule, such as the
    /// malware and phishing lists), unless the user has chosen to open that host anyway.
    pub fn check_document(&self, url: &str, mode: Mode) -> Option<String> {
        if mode == Mode::Off {
            return None;
        }
        let engine = self.documents.read().ok()?.clone()?;
        let host = url::Url::parse(url).ok()?.host_str()?.to_string();
        if self
            .allowed_pages
            .read()
            .map(|a| a.contains(&host))
            .unwrap_or(false)
        {
            return None;
        }
        let req = Request::new(url, url, "main_frame", "GET").ok()?;
        let r = engine.check_network_request(&req);
        // Only an explicit page-level block counts; ordinary resource rules do not.
        if !(r.exception.is_none() && r.redirect.is_none() && r.should_block()) {
            return None;
        }
        // Ordinary resource rules also match a page address; only rules that name the page
        // (`$document`, `$doc`, `$all`) are page blocks.
        let line = r.filter.and_then(|f| f.raw_line)?;
        let (_, options) = line.rsplit_once('$')?;
        options
            .split(',')
            .any(|o| matches!(o.trim(), "document" | "doc" | "all"))
            .then_some(line)
    }

    /// The text of the rule that decides `url` (for the logger). Rule text is kept only in an
    /// engine built on first ask, in the background, so this is `None` until it is ready.
    pub fn rule_for(
        &self,
        url: &str,
        source_url: &str,
        kind: RequestKind,
        method: &str,
    ) -> Option<String> {
        let generation = self.generation();
        let engine = {
            let mut n = self.named.lock().ok()?;
            match &n.engine {
                Some((g, e)) if *g == generation => Some(e.clone()),
                _ => {
                    if !n.building {
                        n.building = true;
                        let recipe = self.recipe.read().ok()?.clone()?;
                        let named = self.named.clone();
                        let current = self.generation.clone();
                        let _ = std::thread::Builder::new()
                            .name("byobu-rule-names".into())
                            .spawn(move || {
                                let engine = compile(recipe(), true);
                                if let Ok(mut n) = named.lock() {
                                    n.building = false;
                                    if current.load(Ordering::Acquire) == generation {
                                        n.engine = Some((generation, Arc::new(engine)));
                                    }
                                }
                            });
                    }
                    None
                }
            }
        }?;
        let req = Request::new(url, source_url, kind.name(), method).ok()?;
        engine.check_network_request(&req).filter?.raw_line
    }

    /// Drop the engine with rule text (the logger closed).
    pub fn forget_rule_names(&self) {
        if let Ok(mut n) = self.named.lock() {
            n.engine = None;
        }
    }

    /// The rule that stops the page at `opener` opening `url` in a new window (a `$popup` rule:
    /// popunders, ad windows), if one does.
    pub fn check_popup(&self, url: &str, opener: &str, mode: Mode) -> Option<String> {
        if mode == Mode::Off || !(url.starts_with("https://") || url.starts_with("http://")) {
            return None;
        }
        let popups = self.popups.read().ok()?.clone()?;
        popups.blocks(url, opener)
    }

    /// Open `host` despite the warning, for the rest of this session.
    pub fn allow_page_host(&self, host: &str) {
        if let Ok(mut a) = self.allowed_pages.write() {
            a.insert(host.to_ascii_lowercase());
        }
    }

    /// Raw cosmetic sets for diagnostics: (hide selectors, procedural actions).
    pub fn debug_cosmetic(&self, url: &str) -> (Vec<String>, Vec<String>) {
        let Some(engine) = self.current() else {
            return Default::default();
        };
        let r = engine.url_cosmetic_resources(url);
        (
            r.hide_selectors.into_iter().collect(),
            r.procedural_actions.into_iter().collect(),
        )
    }

    /// Cosmetic filters and scriptlets for the page at `url`. They depend only on its host, so
    /// they are worked out once per host for each engine (iframes from the same ad host share them).
    pub fn cosmetic(&self, url: &str, mode: Mode) -> CosmeticPayload {
        if mode == Mode::Off {
            return CosmeticPayload::default();
        }
        let Some(engine) = self.current() else {
            return CosmeticPayload::default();
        };
        let generation = self.generation();
        let host = url::Url::parse(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_ascii_lowercase));
        let key = host.map(|h| (h, mode));
        let cached = key.as_ref().and_then(|k| {
            let c = self.cosmetics.lock().ok()?;
            (c.generation == generation).then(|| c.by_host.get(k).cloned())?
        });
        let (payload, exceptions) = match cached {
            Some(hit) => hit,
            None => {
                let res = engine.url_cosmetic_resources(url);
                let mut hide: Vec<String> = res.hide_selectors.into_iter().collect();
                hide.sort_unstable();
                let mut procedural: Vec<String> = res.procedural_actions.into_iter().collect();
                procedural.sort_unstable();
                let payload = CosmeticPayload {
                    hide,
                    procedural,
                    script: res.injected_script,
                    generichide: res.generichide,
                };
                let exceptions = Arc::new(res.exceptions);
                if let (Some(k), Ok(mut c)) = (key, self.cosmetics.lock()) {
                    if c.generation != generation || c.by_host.len() >= COSMETIC_HOSTS {
                        c.by_host.clear();
                        c.generation = generation;
                    }
                    c.by_host.insert(k, (payload.clone(), exceptions.clone()));
                }
                (payload, exceptions)
            }
        };
        if let Ok(mut slot) = self.exceptions.write() {
            *slot = (url.to_string(), exceptions);
        }
        payload
    }

    /// Generic selectors that apply to the classes and ids a page at `url` actually uses.
    pub fn generic_hide(
        &self,
        url: &str,
        classes: &[String],
        ids: &[String],
        mode: Mode,
    ) -> Vec<String> {
        if mode == Mode::Off {
            return Vec::new();
        }
        let Some(engine) = self.current() else {
            return Vec::new();
        };
        let known = self
            .exceptions
            .read()
            .ok()
            .filter(|e| e.0 == url)
            .map(|e| e.1.clone());
        let exceptions = match known {
            Some(e) => e,
            None => {
                let r = engine.url_cosmetic_resources(url);
                let e = Arc::new(r.exceptions);
                if let Ok(mut slot) = self.exceptions.write() {
                    *slot = (url.to_string(), e.clone());
                }
                e
            }
        };
        engine.hidden_class_id_selectors(classes, ids, &exceptions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shield(rules: &str) -> Shield {
        let s = Shield::new();
        assert!(s.load(vec![(rules.to_string(), true)], &[]));
        s
    }

    #[test]
    fn cloaked_first_party_trackers_are_matched_by_their_real_name() {
        let s = shield(
            "||tracker.example^$third-party\n/ads/*\n@@||ok.news.test^\n||news.test/never^\n",
        );
        s.uncloak_with(Uncloaker::spawn(Box::new(|h: &str| {
            let canonical = match h {
                "metrics.news.test" | "ok.news.test" | "www.news.test" => "eu.tracker.example",
                "cdn.news.test" => "edge.cdn.example",
                "img.news.test" => "static.news.test",
                _ => return None,
            };
            Some(crate::Resolution {
                canonical: Some(canonical.into()),
                ttl: std::time::Duration::from_secs(600),
            })
        })));
        let page = "https://www.news.test/story";
        let check = |url: &str| s.check(url, page, RequestKind::Script, Mode::Standard);
        let g0 = s.decision_generation();
        // The first request goes through while the name is looked up.
        assert_eq!(check("https://metrics.news.test/t.js"), Decision::Allow);
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while check("https://metrics.news.test/t.js") == Decision::Allow
            && std::time::Instant::now() < end
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(check("https://metrics.news.test/t.js"), Decision::Block);
        assert_ne!(s.decision_generation(), g0, "cached decisions are dropped");
        // Exceptions on the visible name, the page's own host, CNAMEs within the site and
        // path rules replayed onto another host all stay allowed.
        for url in [
            "https://ok.news.test/t.js",
            "https://www.news.test/t.js",
            "https://img.news.test/t.js",
            "https://cdn.news.test/ads/x.js",
        ] {
            check(url);
        }
        let end = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while check("https://cdn.news.test/ads/x.js") == Decision::Allow
            && std::time::Instant::now() < end
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert_eq!(check("https://ok.news.test/t.js"), Decision::Allow);
        assert_eq!(check("https://www.news.test/t.js"), Decision::Allow);
        assert_eq!(check("https://img.news.test/t.js"), Decision::Allow);
        assert_eq!(check("https://cdn.news.test/ads/x.js"), Decision::Block);
        assert_eq!(check("https://cdn.news.test/app.js"), Decision::Allow);
    }

    #[test]
    fn blocks_listed_third_party_and_allows_rest() {
        let s = shield("||ads.example^$third-party\n##.banner\n");
        let page = "https://news.test/story";
        assert_eq!(
            s.check(
                "https://ads.example/x.js",
                page,
                RequestKind::Script,
                Mode::Standard
            ),
            Decision::Block
        );
        assert_eq!(
            s.check(
                "https://news.test/app.js",
                page,
                RequestKind::Script,
                Mode::Standard
            ),
            Decision::Allow
        );
        assert_eq!(
            s.check(
                "https://ads.example/x.js",
                page,
                RequestKind::Script,
                Mode::Off
            ),
            Decision::Allow
        );
    }

    #[test]
    fn a_saved_engine_is_reused_until_the_lists_change() {
        let dir = std::env::temp_dir().join(format!("byobu-saved-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let lists = || {
            vec![(
                "||ads.example^
news.test##.banner
"
                .to_string(),
                false,
            )]
        };
        let blocks = |s: &Shield| {
            s.check(
                "https://ads.example/x.js",
                "https://news.test/",
                RequestKind::Script,
                Mode::Standard,
            ) == Decision::Block
        };
        assert!(Shield::new().load_saved(lists(), &[], Some(&dir)));
        let key = std::fs::read_to_string(dir.join(SAVED_KEY)).unwrap();
        let saved = std::fs::read(dir.join(SAVED_ENGINE)).unwrap();
        // The same lists load the saved engine, which matches like a fresh one.
        let s = Shield::new();
        assert!(s.load_saved(lists(), &[], Some(&dir)));
        assert!(blocks(&s));
        assert!(s
            .cosmetic("https://news.test/", Mode::Standard)
            .hide
            .contains(&".banner".into()));
        assert_eq!(std::fs::read(dir.join(SAVED_ENGINE)).unwrap(), saved);
        // A damaged file is compiled over, not trusted.
        std::fs::write(dir.join(SAVED_ENGINE), b"junk").unwrap();
        let s = Shield::new();
        assert!(s.load_saved(lists(), &[], Some(&dir)));
        assert!(blocks(&s));
        // Different lists get a new key.
        assert!(Shield::new().load_saved(
            vec![("||other.example^".into(), false)],
            &[],
            Some(&dir)
        ));
        assert_ne!(std::fs::read_to_string(dir.join(SAVED_KEY)).unwrap(), key);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cosmetic_css_hides_selectors() {
        let s = shield("news.test##.banner\n");
        let p = s.cosmetic("https://news.test/story", Mode::Standard);
        assert!(p.hide.iter().any(|s| s == ".banner"), "{:?}", p.hide);
    }

    #[test]
    fn cosmetic_payloads_are_cached_until_the_engine_changes() {
        let s = shield(
            "news.test##.banner
",
        );
        let first = s.cosmetic("https://news.test/a", Mode::Standard);
        assert_eq!(s.cosmetic("https://news.test/b", Mode::Standard), first);
        let g = s.generation();
        assert!(s.load(
            vec![(
                "news.test##.promo
"
                .to_string(),
                true
            )],
            &[]
        ));
        assert_ne!(s.generation(), g);
        let next = s.cosmetic("https://news.test/a", Mode::Standard);
        assert!(next.hide.contains(&".promo".to_string()), "{:?}", next.hide);
        assert!(!next.hide.contains(&".banner".to_string()));
    }

    #[test]
    fn the_engine_keeps_no_rule_text_but_can_name_rules() {
        let s = shield(
            "||ads.example^
||bad.example^$document
",
        );
        let (d, rule) = s.check_detailed(
            "https://ads.example/x.js",
            "https://news.test/",
            RequestKind::Script,
            Mode::Standard,
        );
        assert_eq!((d, rule), (Decision::Block, None));
        assert!(s.engine_bytes() > 0);
        // Page warnings still name their rule.
        assert_eq!(
            s.check_document("https://bad.example/", Mode::Standard)
                .as_deref(),
            Some("||bad.example^$document")
        );
        // Names come from an engine built on first ask.
        let end = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut name = None;
        while name.is_none() && std::time::Instant::now() < end {
            name = s.rule_for(
                "https://ads.example/x.js",
                "https://news.test/",
                RequestKind::Script,
                "GET",
            );
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(name.as_deref(), Some("||ads.example^"));
    }

    #[test]
    fn aggressive_strips_tracking_from_every_request_and_standard_does_not() {
        let s = shield("||ads.example^\n");
        let url = "https://news.test/a?utm_source=x&id=1";
        let page = "https://news.test/";
        assert_eq!(
            s.check(url, page, RequestKind::Xhr, Mode::Standard),
            Decision::Allow
        );
        assert_eq!(
            s.check(url, page, RequestKind::Xhr, Mode::Aggressive),
            Decision::Rewrite("https://news.test/a?id=1".into())
        );
    }

    #[test]
    fn method_rules_see_the_real_method_and_documents_can_be_blocked() {
        let s = shield("||m.example^$method=post\n||bad.example^$document\n");
        let page = "https://news.test/";
        let post = s.check_request(
            "https://m.example/x",
            page,
            RequestKind::Ping,
            Mode::Standard,
            "POST",
        );
        let get = s.check_request(
            "https://m.example/x",
            page,
            RequestKind::Ping,
            Mode::Standard,
            "GET",
        );
        assert_eq!(post.0, Decision::Block);
        assert_eq!(get.0, Decision::Allow);
        assert!(s
            .check_document("https://bad.example/", Mode::Standard)
            .is_some());
        assert!(s
            .check_document("https://fine.example/", Mode::Standard)
            .is_none());
        // An ordinary resource rule is not a page block.
        assert!(shield(
            "||ads.example^
"
        )
        .check_document("https://ads.example/", Mode::Standard)
        .is_none());
        s.allow_page_host("bad.example");
        assert!(s
            .check_document("https://bad.example/", Mode::Standard)
            .is_none());
    }

    #[test]
    fn popup_rules_are_kept_for_new_windows() {
        let s = shield("||popads.test^$popup\n||ads.test^\n");
        let page = "https://news.test/";
        assert!(s
            .check_popup("https://popads.test/x", page, Mode::Standard)
            .is_some());
        assert!(s
            .check_popup("https://popads.test/x", page, Mode::Off)
            .is_none());
        assert!(s
            .check_popup("https://ads.test/x", page, Mode::Standard)
            .is_none());
        // The engine never sees a popup, so the rule blocks nothing else.
        assert_eq!(
            s.check(
                "https://popads.test/x.js",
                page,
                RequestKind::Script,
                Mode::Standard
            ),
            Decision::Allow
        );
    }

    #[test]
    fn removeparam_rewrites() {
        let s = shield("$removeparam=utm_source\n");
        let d = s.check(
            "https://news.test/a?utm_source=x&id=1",
            "https://news.test/",
            RequestKind::Xhr,
            Mode::Standard,
        );
        assert_eq!(d, Decision::Rewrite("https://news.test/a?id=1".into()));
    }
}
