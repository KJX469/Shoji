//! The matcher: network decisions and per-site cosmetic payloads.

use crate::sources::ListSource;
use crate::updater::Updater;
use adblock::lists::{FilterSet, ParseOptions};
use adblock::request::Request;
use adblock::resources::{PermissionMask, Resource};
use adblock::Engine;
use std::collections::HashSet;
use std::sync::{Arc, RwLock};

/// How firmly the shield acts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    Off,
    #[default]
    Standard,
    /// Standard, and tracking parameters are stripped from every request.
    Aggressive,
}

/// What kind of request the browser reports (mapped from the WebView2 resource context).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

impl Default for Shield {
    fn default() -> Self {
        Self::new()
    }
}

impl Shield {
    pub fn new() -> Self {
        Self {
            engine: Arc::new(RwLock::new(None)),
            exceptions: Arc::new(RwLock::new((String::new(), Arc::new(HashSet::new())))),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.engine.read().map(|e| e.is_some()).unwrap_or(false)
    }

    /// Swap in an engine built from raw list texts. Heavy: call off the UI thread.
    /// `lists` is `(text, trusted)`. Returns false when there was nothing to load.
    pub fn load(&self, lists: Vec<(String, bool)>, resource_jsons: &[String]) -> bool {
        if lists.is_empty() {
            return false;
        }
        let mut set = FilterSet::new(true);
        for (text, trusted) in lists {
            let permissions =
                if trusted { PermissionMask::from_bits(1) } else { PermissionMask::default() };
            set.add_filter_list(text, ParseOptions { permissions, ..ParseOptions::default() });
        }
        let mut engine = Engine::new_with_filter_set(set);
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
        true
    }

    /// Build from the updater's cache and swap in. Returns the number of lists loaded.
    pub fn rebuild(&self, updater: &Updater, sources: &[ListSource]) -> usize {
        self.rebuild_with(updater, sources, "")
    }

    /// [`Shield::rebuild`] with the user's own rules appended (trusted: they wrote them).
    pub fn rebuild_with(&self, updater: &Updater, sources: &[ListSource], own_rules: &str) -> usize {
        let mut lists: Vec<(String, bool)> = sources
            .iter()
            .filter_map(|s| updater.list_text(&s.id).map(|t| (t, s.trusted)))
            .collect();
        if !own_rules.trim().is_empty() {
            lists.push((own_rules.to_string(), true));
        }
        let n = lists.len();
        if self.load(lists, &updater.resource_jsons()) {
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
        if mode == Mode::Off || kind == RequestKind::Document {
            return (Decision::Allow, None);
        }
        let Some(engine) = self.current() else { return (Decision::Allow, None) };
        let Ok(req) = Request::new(url, source_url, kind.name(), "GET") else {
            return (Decision::Allow, None);
        };
        let r = engine.check_network_request(&req);
        let blocked = r.should_block();
        let rule = r.filter.and_then(|f| f.raw_line);
        if let Some(redirect) = r.redirect {
            return (Decision::Redirect(redirect), rule);
        }
        if blocked {
            return (Decision::Block, rule);
        }
        match r.rewritten_url {
            Some(u) if u != url => (Decision::Rewrite(u), rule),
            _ => (Decision::Allow, None),
        }
    }

    /// Raw cosmetic sets for diagnostics: (hide selectors, procedural actions).
    pub fn debug_cosmetic(&self, url: &str) -> (Vec<String>, Vec<String>) {
        let Some(engine) = self.current() else { return Default::default() };
        let r = engine.url_cosmetic_resources(url);
        (r.hide_selectors.into_iter().collect(), r.procedural_actions.into_iter().collect())
    }

    /// Cosmetic filters and scriptlets for the page at `url`.
    pub fn cosmetic(&self, url: &str, mode: Mode) -> CosmeticPayload {
        if mode == Mode::Off {
            return CosmeticPayload::default();
        }
        let Some(engine) = self.current() else { return CosmeticPayload::default() };
        let res = engine.url_cosmetic_resources(url);
        if let Ok(mut slot) = self.exceptions.write() {
            *slot = (url.to_string(), Arc::new(res.exceptions.clone()));
        }
        let mut hide: Vec<String> = res.hide_selectors.into_iter().collect();
        hide.sort_unstable();
        let mut procedural: Vec<String> = res.procedural_actions.into_iter().collect();
        procedural.sort_unstable();
        CosmeticPayload { hide, procedural, script: res.injected_script, generichide: res.generichide }
    }

    /// Generic selectors that apply to the classes and ids a page at `url` actually uses.
    pub fn generic_hide(&self, url: &str, classes: &[String], ids: &[String], mode: Mode) -> Vec<String> {
        if mode == Mode::Off {
            return Vec::new();
        }
        let Some(engine) = self.current() else { return Vec::new() };
        let known = self.exceptions.read().ok().filter(|e| e.0 == url).map(|e| e.1.clone());
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
    fn blocks_listed_third_party_and_allows_rest() {
        let s = shield("||ads.example^$third-party\n##.banner\n");
        let page = "https://news.test/story";
        assert_eq!(
            s.check("https://ads.example/x.js", page, RequestKind::Script, Mode::Standard),
            Decision::Block
        );
        assert_eq!(
            s.check("https://news.test/app.js", page, RequestKind::Script, Mode::Standard),
            Decision::Allow
        );
        assert_eq!(
            s.check("https://ads.example/x.js", page, RequestKind::Script, Mode::Off),
            Decision::Allow
        );
    }

    #[test]
    fn cosmetic_css_hides_selectors() {
        let s = shield("news.test##.banner\n");
        let p = s.cosmetic("https://news.test/story", Mode::Standard);
        assert!(p.hide.iter().any(|s| s == ".banner"), "{:?}", p.hide);
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
