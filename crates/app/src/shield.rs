//! Byōbu inside Shoji: request blocking and cosmetic injection for tab webviews, plus the
//! background thread that keeps the upstream lists fresh.

use crate::BrowserEvent;
use base64::Engine as _;
use shoji_byobu::{Decision, Mode, RequestKind, Shield, Updater};
use shoji_core::settings::ShieldSettings;
use shoji_core::tabs::TabId;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::*;
use windows::core::{Interface, PCWSTR, PWSTR};
use windows::Win32::UI::Shell::SHCreateMemStream;
use winit::event_loop::EventLoopProxy;
use wry::{WebView, WebViewExtWindows};

/// Looks up the shield mode that applies to a page URL (reads the live settings).
pub type ModeFor = Arc<dyn Fn(&str) -> Mode + Send + Sync>;

pub fn parse_mode(s: &str) -> Mode {
    match s {
        "off" => Mode::Off,
        "aggressive" => Mode::Aggressive,
        _ => Mode::Standard,
    }
}

/* ---------------------------------------------------------------- hub */

/// The user's shield configuration that the updater thread needs.
#[derive(Default, Clone, PartialEq)]
struct Config {
    custom: Vec<String>,
    disabled: Vec<String>,
    rules: String,
}

struct TabStats {
    blocked: u32,
    replaced: u32,
}

struct LogEntry {
    t: u64,
    tab: u64,
    page: String,
    url: String,
    kind: &'static str,
    action: &'static str,
    rule: Option<String>,
}

struct ListInfo {
    id: String,
    title: String,
    url: String,
    enabled: bool,
    custom: bool,
    bytes: u64,
    rules: usize,
    checked: u64,
}

#[derive(Default)]
struct Inner {
    tabs: HashMap<u64, TabStats>,
    log: VecDeque<LogEntry>,
    lists: Vec<ListInfo>,
    /// Blocked requests per day (unix day number), the last 14 days
    days: BTreeMap<u64, u64>,
    total: u64,
    dirty: bool,
    /// One-time tokens that authorise the element picker's reply, per tab
    tokens: HashMap<u64, String>,
    /// Last state pushed to each surface, so identical updates are skipped
    synced: HashMap<String, String>,
    updating: bool,
    ready: bool,
}

pub struct Hub {
    inner: Mutex<Inner>,
    config: Mutex<Config>,
    wake: Mutex<Option<mpsc::Sender<bool>>>,
    proxy: Mutex<Option<EventLoopProxy<BrowserEvent>>>,
}

const LOG_CAP: usize = 3000;

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    HUB.get_or_init(|| Hub {
        inner: Mutex::new(Inner::default()),
        config: Mutex::new(Config::default()),
        wake: Mutex::new(None),
        proxy: Mutex::new(None),
    })
}

impl Hub {
    fn record(
        &self,
        tab: u64,
        page: &str,
        url: &str,
        kind: RequestKind,
        decision: &Decision,
        rule: Option<String>,
    ) {
        let action = match decision {
            Decision::Block => "blocked",
            Decision::Redirect(_) => "replaced",
            Decision::Rewrite(_) => "cleaned",
            Decision::Allow => return,
        };
        let day = now_ms() / 86_400_000;
        if let Ok(mut h) = self.inner.lock() {
            let stats = h.tabs.entry(tab).or_insert(TabStats {
                blocked: 0,
                replaced: 0,
            });
            match decision {
                Decision::Block => stats.blocked += 1,
                _ => stats.replaced += 1,
            }
            if matches!(decision, Decision::Block) {
                h.total += 1;
                *h.days.entry(day).or_insert(0) += 1;
                h.dirty = true;
            }
            if h.log.len() >= LOG_CAP {
                h.log.pop_front();
            }
            let host = crate::extract_host(page).unwrap_or_default();
            h.log.push_back(LogEntry {
                t: now_ms(),
                tab,
                page: host,
                url: url.chars().take(400).collect(),
                kind: kind_name(kind),
                action,
                rule: rule.map(|r| r.chars().take(300).collect()),
            });
        }
        self.touch(tab);
    }

    /// Ask the host to refresh whatever shows this tab's counters.
    fn touch(&self, tab: u64) {
        if let Ok(p) = self.proxy.lock() {
            if let Some(p) = p.as_ref() {
                let _ = p.send_event(BrowserEvent::ShieldTouched(TabId(tab)));
            }
        }
    }

    fn reset_tab(&self, tab: u64) {
        if let Ok(mut h) = self.inner.lock() {
            h.tabs.insert(
                tab,
                TabStats {
                    blocked: 0,
                    replaced: 0,
                },
            );
            h.tokens.remove(&tab);
        }
        self.touch(tab);
    }

    pub fn forget_tab(&self, tab: u64) {
        if let Ok(mut h) = self.inner.lock() {
            h.tabs.remove(&tab);
            h.tokens.remove(&tab);
        }
    }

    pub fn counts(&self, tab: u64) -> (u32, u32) {
        self.inner
            .lock()
            .ok()
            .and_then(|h| h.tabs.get(&tab).map(|s| (s.blocked, s.replaced)))
            .unwrap_or((0, 0))
    }

    /// True when `state` differs from what was last pushed under `key` (and remembers it).
    pub fn changed(&self, key: &str, state: &str) -> bool {
        let Ok(mut h) = self.inner.lock() else {
            return true;
        };
        if h.synced.get(key).map(String::as_str) == Some(state) {
            return false;
        }
        h.synced.insert(key.to_string(), state.to_string());
        true
    }

    /// Forget what was pushed under `key` (a surface that was just (re)created).
    pub fn forget_sync(&self, key: &str) {
        if let Ok(mut h) = self.inner.lock() {
            h.synced.remove(key);
        }
    }

    fn week(h: &Inner) -> u64 {
        let today = now_ms() / 86_400_000;
        h.days
            .range(today.saturating_sub(6)..=today)
            .map(|(_, n)| n)
            .sum()
    }

    /// Everything the settings and logger pages show. `log_tab`: only that tab's log entries.
    pub fn state_json(&self, with_log: bool, log_tab: Option<u64>) -> String {
        let Ok(h) = self.inner.lock() else {
            return "{}".into();
        };
        let lists: Vec<_> = h
            .lists
            .iter()
            .map(|l| {
                serde_json::json!({
                    "id": l.id, "title": l.title, "url": l.url, "enabled": l.enabled,
                    "custom": l.custom, "bytes": l.bytes, "rules": l.rules, "checked": l.checked,
                })
            })
            .collect();
        let log: Vec<_> = if with_log {
            h.log
                .iter()
                .rev()
                .filter(|e| log_tab.is_none_or(|t| e.tab == t))
                .take(600)
                .map(|e| {
                    serde_json::json!({
                        "t": e.t, "tab": e.tab, "page": e.page, "url": e.url,
                        "kind": e.kind, "action": e.action, "rule": e.rule,
                    })
                })
                .collect()
        } else {
            Vec::new()
        };
        serde_json::json!({
            "lists": lists, "log": log, "total": h.total, "week": Self::week(&h),
            "updating": h.updating, "ready": h.ready,
        })
        .to_string()
    }

    /// Per-tab summary for the panel and the toolbar button.
    pub fn tab_json(&self, tab: u64, host: &str, state: &str) -> String {
        let (blocked, replaced) = self.counts(tab);
        let (week, total, ready) = self
            .inner
            .lock()
            .map(|h| (Self::week(&h), h.total, h.ready))
            .unwrap_or((0, 0, false));
        serde_json::json!({
            "host": host, "state": state, "blocked": blocked, "replaced": replaced,
            "week": week, "total": total, "ready": ready,
        })
        .to_string()
    }

    /// Start the element picker in a tab; returns the token the page must echo back.
    pub fn begin_pick(&self, tab: u64) -> String {
        let token = format!(
            "{:016x}{:016x}",
            now_ms().wrapping_mul(0x9E37_79B9_7F4A_7C15),
            {
                let t = std::time::Instant::now();
                // Not cryptographic, but unguessable from page script: it never leaves the host
                // until the picker is injected.
                (t.elapsed().as_nanos() as u64) ^ (tab.wrapping_mul(0xBF58_476D_1CE4_E5B9))
            }
        );
        if let Ok(mut h) = self.inner.lock() {
            h.tokens.insert(tab, token.clone());
        }
        token
    }

    fn take_token(&self, tab: u64, presented: &str) -> bool {
        let Ok(mut h) = self.inner.lock() else {
            return false;
        };
        match h.tokens.get(&tab) {
            Some(t) if t == presented => {
                h.tokens.remove(&tab);
                true
            }
            _ => false,
        }
    }

    /// Wake the updater thread: `force` re-checks every list now.
    pub fn refresh(&self, force: bool) {
        if let Ok(w) = self.wake.lock() {
            if let Some(w) = w.as_ref() {
                let _ = w.send(force);
            }
        }
    }
}

/// Hand the hub the event-loop proxy so request handlers can notify the host.
pub fn init(proxy: EventLoopProxy<BrowserEvent>) {
    if let Ok(mut p) = hub().proxy.lock() {
        *p = Some(proxy);
    }
}

/// Apply the user's shield settings (custom lists, disabled lists, own rules).
pub fn apply_settings(s: &ShieldSettings) {
    let new = Config {
        custom: s.custom_lists.clone(),
        disabled: s.disabled_lists.clone(),
        rules: s.custom_rules.clone(),
    };
    let changed = hub().config.lock().map(|mut c| {
        let changed = *c != new;
        *c = new;
        changed
    });
    if changed.unwrap_or(false) {
        hub().refresh(false);
    }
}

fn kind_name(k: RequestKind) -> &'static str {
    match k {
        RequestKind::Document => "document",
        RequestKind::Subdocument => "frame",
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

fn load_stats(dir: &std::path::Path) {
    let Ok(text) = std::fs::read_to_string(dir.join("stats.json")) else {
        return;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
        return;
    };
    if let Ok(mut h) = hub().inner.lock() {
        h.total = v.get("total").and_then(|t| t.as_u64()).unwrap_or(0);
        if let Some(days) = v.get("days").and_then(|d| d.as_object()) {
            for (k, n) in days {
                if let (Ok(k), Some(n)) = (k.parse::<u64>(), n.as_u64()) {
                    h.days.insert(k, n);
                }
            }
        }
    }
}

fn save_stats(dir: &std::path::Path) {
    let json = {
        let Ok(mut h) = hub().inner.lock() else {
            return;
        };
        if !h.dirty {
            return;
        }
        h.dirty = false;
        let today = now_ms() / 86_400_000;
        let keep = today.saturating_sub(13);
        h.days.retain(|d, _| *d >= keep);
        let days: serde_json::Map<String, serde_json::Value> = h
            .days
            .iter()
            .map(|(d, n)| (d.to_string(), (*n).into()))
            .collect();
        serde_json::json!({ "total": h.total, "days": days }).to_string()
    };
    let _ = std::fs::write(dir.join("stats.json"), json);
}

/// Keep the engine fresh: serve the cache at once, then refresh whatever is due (hourly at
/// the fastest), and rebuild whenever a list, a setting or the user's own rules change.
/// Runs on its own thread; a rebuilt engine is swapped in atomically.
pub fn spawn_updater(shield: Shield, dir: PathBuf) {
    let (tx, rx) = mpsc::channel::<bool>();
    if let Ok(mut w) = hub().wake.lock() {
        *w = Some(tx);
    }
    let _ = std::thread::Builder::new()
        .name("byobu-updater".into())
        .spawn(move || {
            let updater = Updater::new(&dir);
            load_stats(&dir);
            let mut built: Option<Config> = None;
            let mut next_check = Instant::now();
            let mut force = false;
            loop {
                let cfg = hub().config.lock().map(|c| c.clone()).unwrap_or_default();
                let due = force || Instant::now() >= next_check;
                let config_changed = built.as_ref() != Some(&cfg);
                if due || config_changed || !shield.is_ready() {
                    if let Ok(mut h) = hub().inner.lock() {
                        h.updating = true;
                    }
                    hub().touch(0);
                    // Serve from the cache before the network, so a cold start isn't unprotected.
                    if !shield.is_ready() {
                        let cached = updater.sources_with(false, &cfg.custom);
                        let cached: Vec<_> = cached
                            .into_iter()
                            .filter(|s| !cfg.disabled.contains(&s.id))
                            .collect();
                        shield.rebuild_with(&updater, &cached, &cfg.rules);
                    }
                    let (sources, r) = updater.refresh_with(force, &cfg.custom, &cfg.disabled);
                    if r.changed > 0 || config_changed || built.is_none() {
                        shield.rebuild_with(&updater, &sources, &cfg.rules);
                    }
                    built = Some(cfg.clone());
                    let all = updater.sources_with(false, &cfg.custom);
                    let infos: Vec<ListInfo> = all
                        .iter()
                        .map(|s| {
                            let (bytes, rules, checked) = updater.list_stats(&s.id);
                            ListInfo {
                                id: s.id.clone(),
                                title: s.title.clone(),
                                url: s.url.clone(),
                                enabled: !cfg.disabled.contains(&s.id),
                                custom: s.id.starts_with("custom-"),
                                bytes,
                                rules,
                                checked,
                            }
                        })
                        .collect();
                    if let Ok(mut h) = hub().inner.lock() {
                        h.lists = infos;
                        h.updating = false;
                        h.ready = shield.is_ready();
                    }
                    let wait = updater.next_due_in(&sources).min(3600);
                    let wait = if r.failed > 0 { wait.min(900) } else { wait };
                    next_check = Instant::now() + Duration::from_secs(wait);
                    force = false;
                    hub().touch(0);
                }
                save_stats(&dir);
                match rx.recv_timeout(Duration::from_secs(60)) {
                    Ok(f) => force |= f,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        });
}

fn kind_of(ctx: COREWEBVIEW2_WEB_RESOURCE_CONTEXT) -> RequestKind {
    match ctx {
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_DOCUMENT => RequestKind::Document,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => RequestKind::Stylesheet,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => RequestKind::Image,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => RequestKind::Media,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT => RequestKind::Font,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_SCRIPT => RequestKind::Script,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_XML_HTTP_REQUEST
        | COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FETCH => RequestKind::Xhr,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING
        | COREWEBVIEW2_WEB_RESOURCE_CONTEXT_CSP_VIOLATION_REPORT => RequestKind::Ping,
        COREWEBVIEW2_WEB_RESOURCE_CONTEXT_WEBSOCKET => RequestKind::Websocket,
        _ => RequestKind::Other,
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn take(p: PWSTR) -> String {
    if p.is_null() {
        return String::new();
    }
    let s = unsafe { p.to_string() }.unwrap_or_default();
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as _)) };
    s
}

/// `data:<mime>;base64,<payload>` -> (mime, bytes)
fn decode_data_url(url: &str) -> Option<(String, Vec<u8>)> {
    let rest = url.strip_prefix("data:")?;
    let (meta, payload) = rest.split_once(',')?;
    let mime = meta.split(';').next().unwrap_or("text/plain").to_string();
    let bytes = if meta.ends_with(";base64") {
        base64::engine::general_purpose::STANDARD
            .decode(payload)
            .ok()?
    } else {
        payload.as_bytes().to_vec()
    };
    Some((mime, bytes))
}

/// Subscribe a tab to the shield: block requests, and register the page's cosmetic filters
/// and scriptlets before each navigation commits.
pub fn attach(webview: &WebView, shield: Shield, mode_for: ModeFor, tab_id: TabId) {
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return;
    };
    let Ok(core2) = core.cast::<ICoreWebView2_2>() else {
        return;
    };
    let mut token = Default::default();
    // WebView2 reports iframes as DOCUMENT too; the main frame is the navigation target.
    let top_target: Arc<Mutex<String>> = Arc::default();

    // Network: every http(s)/ws request goes through the matcher.
    {
        let core_src = core.clone();
        let shield = shield.clone();
        let mode_for = mode_for.clone();
        let core_env = core2.clone();
        let top_target = top_target.clone();
        let tab = tab_id.0;
        let handler = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let request = unsafe { args.Request()? };
            let mut uri = PWSTR::null();
            unsafe { request.Uri(&mut uri)? };
            let url = take(uri);
            if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("ws"))
                || url.starts_with("http://shoji.")
            {
                return Ok(());
            }
            let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
            unsafe { args.ResourceContext(&mut ctx)? };
            let mut kind = kind_of(ctx);
            let mut src = PWSTR::null();
            unsafe { core_src.Source(&mut src)? };
            let page = take(src);
            if kind == RequestKind::Document {
                let is_top = top_target.lock().map(|t| *t == url).unwrap_or(true) || url == page;
                if is_top {
                    return Ok(());
                }
                kind = RequestKind::Subdocument;
            }
            let mode = mode_for(&page);
            if mode == Mode::Off {
                return Ok(());
            }
            let (decision, rule) = shield.check_detailed(&url, &page, kind, mode);
            if decision == Decision::Allow {
                return Ok(());
            }
            hub().record(tab, &page, &url, kind, &decision, rule);
            let env = unsafe { core_env.Environment()? };
            match decision {
                Decision::Allow => {}
                Decision::Block => {
                    let reason = wide("Blocked");
                    let headers = wide("");
                    let resp = unsafe {
                        env.CreateWebResourceResponse(
                            None,
                            403,
                            PCWSTR(reason.as_ptr()),
                            PCWSTR(headers.as_ptr()),
                        )?
                    };
                    unsafe { args.SetResponse(&resp)? };
                }
                Decision::Redirect(data) => {
                    if let Some((mime, bytes)) = decode_data_url(&data) {
                        let stream = unsafe { SHCreateMemStream(Some(&bytes)) };
                        let reason = wide("OK");
                        let headers = wide(&format!("Content-Type: {mime}"));
                        let resp = unsafe {
                            env.CreateWebResourceResponse(
                                stream.as_ref(),
                                200,
                                PCWSTR(reason.as_ptr()),
                                PCWSTR(headers.as_ptr()),
                            )?
                        };
                        unsafe { args.SetResponse(&resp)? };
                    }
                }
                Decision::Rewrite(new_url) => {
                    let w = wide(&new_url);
                    unsafe { request.SetUri(PCWSTR(w.as_ptr()))? };
                }
            }
            Ok(())
        }));
        unsafe {
            let all = wide("*");
            let _ = core.AddWebResourceRequestedFilter(
                PCWSTR(all.as_ptr()),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            );
            let _ = core.add_WebResourceRequested(&handler, &mut token);
        }
    }

    // Generic class/id rules: the page reports the classes and ids it uses, and gets back the
    // selectors that apply to them.
    {
        let core_msg = core.clone();
        let shield = shield.clone();
        let mode_for = mode_for.clone();
        let tab = tab_id.0;
        let handler = WebMessageReceivedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut msg = PWSTR::null();
            if unsafe { args.TryGetWebMessageAsString(&mut msg) }.is_err() {
                return Ok(());
            }
            let msg = take(msg);
            if let Some(body) = msg.strip_prefix("byobu:rule:") {
                accept_rule(&core_msg, tab, body);
                return Ok(());
            }
            let Some(body) = msg.strip_prefix("byobu:") else {
                return Ok(());
            };
            let Ok(ask) = serde_json::from_str::<serde_json::Value>(body) else {
                return Ok(());
            };
            let strings = |key: &str| -> Vec<String> {
                ask.get(key)
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| s.as_str().map(str::to_owned))
                            .take(1000)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let (classes, ids) = (strings("c"), strings("i"));
            let mut src = PWSTR::null();
            unsafe { core_msg.Source(&mut src)? };
            let page = take(src);
            let mode = mode_for(&page);
            let list = shield.generic_hide(&page, &classes, &ids, mode);
            if list.is_empty() {
                return Ok(());
            }
            let js = format!(
                "window.__byobuHide&&window.__byobuHide({})",
                serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
            );
            let w = wide(&js);
            let done = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
            let _ = unsafe { core_msg.ExecuteScript(PCWSTR(w.as_ptr()), &done) };
            Ok(())
        }));
        let _ = unsafe { core.add_WebMessageReceived(&handler, &mut token) };
    }

    // Cosmetics: computed for the destination URL as the navigation starts.
    {
        let core_nav = core.clone();
        let top_target = top_target.clone();
        let slot: Arc<Mutex<DocScript>> = Arc::default();
        let handler = NavigationStartingEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut uri = PWSTR::null();
            unsafe { args.Uri(&mut uri)? };
            let url = take(uri);
            if let Ok(mut t) = top_target.lock() {
                *t = url.clone();
            }
            if url.starts_with("http://") || url.starts_with("https://") {
                hub().reset_tab(tab_id.0);
            }
            let script = cosmetic_script(&shield, &mode_for, &url);
            register(&core_nav, &slot, script.as_deref());
            Ok(())
        }));
        let _ = unsafe { core.add_NavigationStarting(&handler, &mut token) };
    }
}

/// The in-page runtime (hide rules, procedural filters, generic-rule bridge).
const COSMETIC_RUNTIME: &str = include_str!("../ui/byobu-cosmetic.js");

/// Document-start script for `url`: hides elements and runs scriptlets, only on that host.
fn cosmetic_script(shield: &Shield, mode_for: &ModeFor, url: &str) -> Option<String> {
    if !(url.starts_with("http://") || url.starts_with("https://"))
        || url.starts_with("http://shoji.")
    {
        return None;
    }
    let mode = mode_for(url);
    if mode == Mode::Off {
        return None;
    }
    let payload = shield.cosmetic(url, mode);
    let host = crate::extract_host(url)?;
    let procedural: Vec<serde_json::Value> = payload
        .procedural
        .iter()
        .filter_map(|p| serde_json::from_str(p).ok())
        .collect();
    let data = serde_json::json!({
        "host": host,
        "hide": payload.hide,
        "procedural": procedural,
        "generic": !payload.generichide,
    });
    // JSON is valid JS except for these two line separators.
    let data = data
        .to_string()
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029");
    let mut js = COSMETIC_RUNTIME.replace("__BYOBU_PAYLOAD__", &data);
    if !payload.script.is_empty() {
        // Scriptlets run in the page's world at document start; one failing never stops the rest.
        let h = serde_json::to_string(&host).unwrap_or_default();
        js.push_str(&format!(
            "\n(function(){{if(location.hostname!=={h})return;try{{{}}}catch(e){{}}}})();",
            payload.script
        ));
    }
    Some(js)
}

/// The single document script registered for the tab's current navigation.
#[derive(Default)]
struct DocScript {
    /// Bumped on every registration; a completion for an older one removes itself.
    generation: u64,
    id: Option<String>,
}

/// Replace the tab's shield script. Registration is asynchronous and redirects start a new
/// navigation immediately, so a late completion must not leave a stale script behind.
fn register(core: &ICoreWebView2, state: &Arc<Mutex<DocScript>>, script: Option<&str>) {
    let remove = |id: &str| {
        let w = wide(id);
        let _ = unsafe { core.RemoveScriptToExecuteOnDocumentCreated(PCWSTR(w.as_ptr())) };
    };
    let generation = {
        let Ok(mut st) = state.lock() else { return };
        st.generation += 1;
        if let Some(old) = st.id.take() {
            remove(&old);
        }
        st.generation
    };
    let Some(script) = script else { return };
    let (state, core_done) = (state.clone(), core.clone());
    let handler =
        AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(move |result, id| {
            if result.is_ok() {
                let current = state.lock().map(|mut st| {
                    let live = st.generation == generation;
                    if live {
                        st.id = Some(id.clone());
                    }
                    live
                });
                if !current.unwrap_or(false) {
                    let w = wide(&id);
                    let _ = unsafe {
                        core_done.RemoveScriptToExecuteOnDocumentCreated(PCWSTR(w.as_ptr()))
                    };
                }
            }
            Ok(())
        }));
    let w = wide(script);
    let _ = unsafe { core.AddScriptToExecuteOnDocumentCreated(PCWSTR(w.as_ptr()), &handler) };
}

/* ---------------------------------------------------------------- picker */

const PICKER: &str = include_str!("../ui/byobu-picker.js");

/// Start the "hide an element" picker in a tab.
pub fn pick_element(view: &WebView, tab: TabId) {
    let token = hub().begin_pick(tab.0);
    let _ = view.evaluate_script(&PICKER.replace("__TOKEN__", &token));
}

/// A rule the picker sent back. Accepted only with the tab's one-time token, for the page's
/// own host, and only in the plain `host##selector` shape.
fn accept_rule(core: &ICoreWebView2, tab: u64, body: &str) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return;
    };
    let (Some(token), Some(selector)) = (
        v.get("t").and_then(|t| t.as_str()),
        v.get("s").and_then(|s| s.as_str()),
    ) else {
        return;
    };
    if !hub().take_token(tab, token) {
        return;
    }
    let selector = selector.trim();
    let bad = selector.is_empty()
        || selector.len() > 300
        || selector.contains(['{', '}', '\n', '\r'])
        || selector.contains("##")
        || selector.contains("+js(");
    if bad {
        return;
    }
    let mut src = PWSTR::null();
    if unsafe { core.Source(&mut src) }.is_err() {
        return;
    }
    let Some(host) = crate::extract_host(&take(src)) else {
        return;
    };
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    if let Ok(p) = hub().proxy.lock() {
        if let Some(p) = p.as_ref() {
            let _ = p.send_event(BrowserEvent::ShieldRule {
                host,
                selector: selector.to_string(),
            });
        }
    }
}
