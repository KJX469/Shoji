//! Byōbu inside Shoji: request blocking and cosmetic injection for tab webviews, plus the
//! background thread that keeps the upstream lists fresh.

use crate::BrowserEvent;
use base64::Engine as _;
use shoji_byobu::{Decision, Mode, RequestKind, Shield, Updater};
use shoji_core::settings::ShieldSettings;
use shoji_core::tabs::TabId;
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
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

/// Fingerprinting protection for a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fp {
    Off,
    Standard,
    Strict,
}

pub fn parse_fp(s: &str) -> Fp {
    match s {
        "off" => Fp::Off,
        "strict" => Fp::Strict,
        _ => Fp::Standard,
    }
}

/// Looks up the fingerprinting protection that applies to a page URL.
pub type FpFor = Arc<dyn Fn(&str) -> Fp + Send + Sync>;

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
    /// A counter refresh is already queued for the host; more would only wake it for nothing
    touch_queued: std::sync::atomic::AtomicBool,
}

const LOG_CAP: usize = 3000;

/// How long the last [`CHECK_SAMPLES`] request checks took on the UI thread, in microseconds.
static CHECK_TIMES: Mutex<VecDeque<u32>> = Mutex::new(VecDeque::new());
const CHECK_SAMPLES: usize = 4096;

fn note_check_time(d: Duration) {
    if let Ok(mut t) = CHECK_TIMES.lock() {
        if t.len() == CHECK_SAMPLES {
            t.pop_front();
        }
        t.push_back(d.as_micros().min(u32::MAX as u128) as u32);
    }
}

/// (median, 99th percentile) of recent request checks, in microseconds.
fn check_times() -> Option<(u32, u32)> {
    let mut t: Vec<u32> = CHECK_TIMES.lock().ok()?.iter().copied().collect();
    if t.is_empty() {
        return None;
    }
    t.sort_unstable();
    let at = |q: f64| t[((t.len() - 1) as f64 * q).round() as usize];
    Some((at(0.5), at(0.99)))
}

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
        touch_queued: std::sync::atomic::AtomicBool::new(false),
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

    /// Tell the host a page was stopped from opening.
    fn blocked_page(&self, tab: u64, url: &str) {
        if let Ok(p) = self.proxy.lock() {
            if let Some(p) = p.as_ref() {
                let _ = p.send_event(BrowserEvent::ShieldBlockedPage {
                    tab_id: TabId(tab),
                    url: url.to_string(),
                });
            }
        }
    }

    /// Ask the host to refresh whatever shows this tab's counters.
    fn touch(&self, tab: u64) {
        // A page that keeps retrying blocked requests would otherwise wake the host once per
        // request; the counters are read from the hub anyway, so one pending refresh is enough.
        if tab != 0
            && self
                .touch_queued
                .swap(true, std::sync::atomic::Ordering::AcqRel)
        {
            return;
        }
        if let Ok(p) = self.proxy.lock() {
            if let Some(p) = p.as_ref() {
                let _ = p.send_event(BrowserEvent::ShieldTouched(TabId(tab)));
            }
        }
    }

    /// The host has seen the queued counter refresh.
    pub fn touch_handled(&self) {
        self.touch_queued
            .store(false, std::sync::atomic::Ordering::Release);
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
            "timing": check_times().map(|(p50, p99)| serde_json::json!({"p50": p50, "p99": p99})),
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
        // SipHash under `RandomState`'s secret keys (seeded from the OS's random source, and
        // different for each instance): not derivable from anything page script can see.
        use std::hash::{BuildHasher, RandomState};
        let seed = (now_ms(), tab, std::time::Instant::now());
        let token = format!(
            "{:016x}{:016x}",
            RandomState::new().hash_one(seed),
            RandomState::new().hash_one(seed)
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

static CLEAN_LINKS: AtomicBool = AtomicBool::new(true);
static SEND_GPC: AtomicBool = AtomicBool::new(true);
static HOOK_REQUESTS: AtomicBool = AtomicBool::new(true);

/// Whether tabs need to see their network requests at all: to filter them, or to add the GPC
/// header. Every hooked request waits on the UI thread, so tabs drop the hook when neither is on.
pub fn wants_request_hook() -> bool {
    HOOK_REQUESTS.load(Ordering::Relaxed)
}

/// Start or stop routing a tab's requests through the shield (see [`wants_request_hook`]).
///
/// Requests from every source are routed: the page, cross-origin iframes (where ads and their
/// pixels live), and service and shared workers. The older filter only sees the main frame and
/// same-origin iframes, so it is a fallback for runtimes that predate `ICoreWebView2_22`.
pub fn set_request_hook(webview: &WebView, on: bool) {
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return;
    };
    let all = wide("*");
    if let Ok(core22) = core.cast::<ICoreWebView2_22>() {
        let kinds = COREWEBVIEW2_WEB_RESOURCE_REQUEST_SOURCE_KINDS_ALL;
        let _ = unsafe {
            if on {
                core22.AddWebResourceRequestedFilterWithRequestSourceKinds(
                    PCWSTR(all.as_ptr()),
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                    kinds,
                )
            } else {
                core22.RemoveWebResourceRequestedFilterWithRequestSourceKinds(
                    PCWSTR(all.as_ptr()),
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
                    kinds,
                )
            }
        };
        return;
    }
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        eprintln!("byobu: this WebView2 runtime can't filter cross-origin iframes and workers")
    });
    let _ = unsafe {
        if on {
            core.AddWebResourceRequestedFilter(
                PCWSTR(all.as_ptr()),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            )
        } else {
            core.RemoveWebResourceRequestedFilter(
                PCWSTR(all.as_ptr()),
                COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL,
            )
        }
    };
}

/// Apply the user's shield settings (custom lists, disabled lists, own rules).
pub fn apply_settings(s: &ShieldSettings) {
    CLEAN_LINKS.store(s.clean_links, Ordering::Relaxed);
    SEND_GPC.store(s.send_gpc, Ordering::Relaxed);
    HOOK_REQUESTS.store(s.mode != "off" || s.send_gpc, Ordering::Relaxed);
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

/// How often Byōbu re-checks its lists while the browser is running.
const UPDATE_INTERVAL_SECS: u64 = 3600;
/// How soon to retry after a pass in which a fetch failed.
const UPDATE_RETRY_SECS: u64 = 600;

/// Keep the engine fresh: serve the cache at once, check every list at launch and then hourly,
/// and rebuild whenever a list, a setting or the user's own rules change.
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
            // Check every list at launch and then every hour, whether or not a window is open.
            // These are conditional requests, so an unchanged list costs a 304 and nothing is
            // downloaded.
            let mut force = true;
            loop {
                let cfg = hub().config.lock().map(|c| c.clone()).unwrap_or_default();
                let hourly = Instant::now() >= next_check;
                let due = force || hourly;
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
                    let (sources, r) =
                        updater.refresh_with(force || hourly, &cfg.custom, &cfg.disabled);
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
                    // Every hour; sooner after a failed fetch (offline, or the host was down).
                    let wait = if r.failed > 0 {
                        UPDATE_RETRY_SECS
                    } else {
                        UPDATE_INTERVAL_SECS
                    };
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
/// and scriptlets before each navigation commits. Returns whether requests are hooked now.
pub fn attach(
    webview: &WebView,
    shield: Shield,
    mode_for: ModeFor,
    fp_for: FpFor,
    tab_id: TabId,
) -> bool {
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return false;
    };
    let hooked = wants_request_hook();
    let Ok(core2) = core.cast::<ICoreWebView2_2>() else {
        return false;
    };
    let mut token = Default::default();
    // The page as of its last top-level navigation. WebView2 reports iframes as DOCUMENT too;
    // the main frame's request is the one for this URL. Requests from every frame and worker
    // are matched against it, which is what third-party rules expect.
    let top_target: Arc<Mutex<String>> = Arc::default();

    // Network: every http(s)/ws request goes through the matcher.
    {
        let shield = shield.clone();
        let mode_for = mode_for.clone();
        let core_env = core2.clone();
        let top_target = top_target.clone();
        let tab = tab_id.0;
        // One empty 403 serves every blocked request: building a response is several calls to
        // the browser process, and a page that retries blocked requests makes thousands.
        let blocked_response: std::cell::RefCell<Option<ICoreWebView2WebResourceResponse>> =
            std::cell::RefCell::new(None);
        let handler = WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
            let started = Instant::now();
            let result = (|| -> windows::core::Result<()> {
                let Some(args) = args else { return Ok(()) };
                let request = unsafe { args.Request()? };
                let mut uri = PWSTR::null();
                unsafe { request.Uri(&mut uri)? };
                let url = take(uri);
                if !(url.starts_with("http://")
                    || url.starts_with("https://")
                    || url.starts_with("ws"))
                    || url.starts_with("http://shoji.")
                {
                    return Ok(());
                }
                if SEND_GPC.load(Ordering::Relaxed) {
                    if let Ok(headers) = unsafe { request.Headers() } {
                        let (name, value) = (wide("Sec-GPC"), wide("1"));
                        let _ = unsafe {
                            headers.SetHeader(PCWSTR(name.as_ptr()), PCWSTR(value.as_ptr()))
                        };
                    }
                }
                let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
                unsafe { args.ResourceContext(&mut ctx)? };
                let mut kind = kind_of(ctx);
                let page = top_target.lock().map(|t| t.clone()).unwrap_or_default();
                if kind == RequestKind::Document {
                    if url == page {
                        return Ok(());
                    }
                    kind = RequestKind::Subdocument;
                }
                let mode = mode_for(&page);
                if mode == Mode::Off {
                    return Ok(());
                }
                let mut method = PWSTR::null();
                unsafe { request.Method(&mut method)? };
                let method = take(method);
                let (decision, rule) = shield.check_request(&url, &page, kind, mode, &method);
                if decision == Decision::Allow {
                    return Ok(());
                }
                hub().record(tab, &page, &url, kind, &decision, rule);
                match decision {
                    Decision::Allow => {}
                    Decision::Block => {
                        let mut cached = blocked_response.borrow_mut();
                        if cached.is_none() {
                            let env = unsafe { core_env.Environment()? };
                            let reason = wide("Blocked");
                            let headers = wide("");
                            *cached = Some(unsafe {
                                env.CreateWebResourceResponse(
                                    None,
                                    403,
                                    PCWSTR(reason.as_ptr()),
                                    PCWSTR(headers.as_ptr()),
                                )?
                            });
                        }
                        if let Some(resp) = cached.as_ref() {
                            unsafe { args.SetResponse(resp)? };
                        }
                    }
                    Decision::Redirect(data) => {
                        if let Some((mime, bytes)) = decode_data_url(&data) {
                            let env = unsafe { core_env.Environment()? };
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
            })();
            note_check_time(started.elapsed());
            result
        }));
        unsafe {
            let _ = core.add_WebResourceRequested(&handler, &mut token);
        }
        if hooked {
            set_request_hook(webview, true);
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
            let mut src = PWSTR::null();
            unsafe { core_msg.Source(&mut src)? };
            let page = take(src);
            let Some(js) = generic_reply(&shield, &page, body, mode_for(&page)) else {
                return Ok(());
            };
            let w = wide(&js);
            let done = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
            let _ = unsafe { core_msg.ExecuteScript(PCWSTR(w.as_ptr()), &done) };
            Ok(())
        }));
        let _ = unsafe { core.add_WebMessageReceived(&handler, &mut token) };
    }

    // The page's address once a navigation commits, or changes in place. A navigation that
    // turns into a download never commits, and the page that stays is still the source.
    {
        let core_src = core.clone();
        let top_target = top_target.clone();
        let handler = SourceChangedEventHandler::create(Box::new(move |_, _| {
            let mut src = PWSTR::null();
            unsafe { core_src.Source(&mut src)? };
            if let Ok(mut t) = top_target.lock() {
                *t = take(src);
            }
            Ok(())
        }));
        let _ = unsafe { core.add_SourceChanged(&handler, &mut token) };
    }

    // Iframes get their own host's cosmetics and scriptlets.
    let frames = Rc::new(Frames {
        core: core.clone(),
        shield: shield.clone(),
        mode_for: mode_for.clone(),
        top_target: top_target.clone(),
        hosts: RefCell::default(),
        slot: Arc::default(),
    });
    if let Ok(core4) = core.cast::<ICoreWebView2_4>() {
        let frames = frames.clone();
        let handler = FrameCreatedEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                hook_frame(unsafe { args.Frame()? }, &frames);
            }
            Ok(())
        }));
        let _ = unsafe { core4.add_FrameCreated(&handler, &mut token) };
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
            if CLEAN_LINKS.load(Ordering::Relaxed) && mode_for(&url) != Mode::Off {
                if let Some(clean) = shoji_byobu::links::clean(&url) {
                    // Cancel and go to the cleaned address; it passes through here again
                    // and is left alone, since there is nothing more to clean.
                    unsafe { args.SetCancel(true)? };
                    let w = wide(&clean);
                    let _ = unsafe { core_nav.Navigate(PCWSTR(w.as_ptr())) };
                    return Ok(());
                }
            }
            let web = (url.starts_with("http://") || url.starts_with("https://"))
                && !url.starts_with("http://shoji.");
            if web {
                // Pages on the malware / phishing lists are not opened without a warning.
                if let Some(rule) = shield.check_document(&url, mode_for(&url)) {
                    unsafe { args.SetCancel(true)? };
                    hub().record(
                        tab_id.0,
                        &url,
                        &url,
                        RequestKind::Document,
                        &Decision::Block,
                        Some(rule),
                    );
                    hub().blocked_page(tab_id.0, &url);
                    return Ok(());
                }
            }
            if let Ok(mut t) = top_target.lock() {
                *t = url.clone();
            }
            if url.starts_with("http://") || url.starts_with("https://") {
                hub().reset_tab(tab_id.0);
            }
            let mut script = cosmetic_script(&shield, &mode_for, &url);
            if let Some(fp) = fingerprint_script(&url, fp_for(&url)) {
                // First, so pages read the protected values from their very first script.
                script = Some(format!(
                    "{fp}
{}",
                    script.as_deref().unwrap_or("")
                ));
            }
            if let Some(fix) = crate::darkfixes::script_for(&url) {
                // The site's dark-mode fix rides along with the page's own scripts.
                script = Some(format!(
                    "{fix}
{}",
                    script.as_deref().unwrap_or("")
                ));
            }
            if web && SEND_GPC.load(Ordering::Relaxed) {
                // The header alone is not enough: consent scripts read the property.
                let gpc = format!("{GPC_SCRIPT}\n{}", script.as_deref().unwrap_or(""));
                script = Some(gpc);
            }
            register(&core_nav, &slot, script.as_deref());
            frames.page_changed();
            Ok(())
        }));
        let _ = unsafe { core.add_NavigationStarting(&handler, &mut token) };
    }
    hooked
}

/// How many iframe hosts a tab keeps document-start cosmetics for.
const FRAME_HOSTS: usize = 24;

/// A tab's iframe cosmetics. WebView2 runs document scripts in every frame, so one script holds
/// the payloads of the frame hosts the tab has seen, and each frame picks its own. A frame on a
/// host seen for the first time may load before that script is updated, so it also gets its
/// payload (without scriptlets) once its DOM is ready.
struct Frames {
    core: ICoreWebView2,
    shield: Shield,
    mode_for: ModeFor,
    top_target: Arc<Mutex<String>>,
    /// Hosts seen in iframes, oldest first
    hosts: RefCell<VecDeque<String>>,
    slot: Arc<Mutex<DocScript>>,
}

impl Frames {
    fn page(&self) -> String {
        self.top_target
            .lock()
            .map(|t| t.clone())
            .unwrap_or_default()
    }

    fn rebuild(&self) {
        let page = self.page();
        let top_host = crate::extract_host(&page).unwrap_or_default();
        let hosts: Vec<String> = self.hosts.borrow().iter().cloned().collect();
        let mode = (self.mode_for)(&page);
        let script = frame_script(&self.shield, mode, &hosts, &top_host, true);
        register(&self.core, &self.slot, script.as_deref());
    }

    /// The top page is navigating: its mode, or which host is the top one, may change.
    fn page_changed(&self) {
        if !self.hosts.borrow().is_empty() {
            self.rebuild();
        }
    }

    /// An iframe is navigating to `url`.
    fn frame_navigating(&self, url: &str) {
        if !is_web(url) {
            return;
        }
        let Some(host) = crate::extract_host(url) else {
            return;
        };
        {
            let mut hosts = self.hosts.borrow_mut();
            if hosts.contains(&host) {
                return;
            }
            hosts.push_back(host);
            if hosts.len() > FRAME_HOSTS {
                hosts.pop_front();
            }
        }
        self.rebuild();
    }
}

fn is_web(url: &str) -> bool {
    (url.starts_with("http://") || url.starts_with("https://")) && !url.starts_with("http://shoji.")
}

fn execute_in_frame(frame: &ICoreWebView2Frame, js: &str) {
    if let Ok(f2) = frame.cast::<ICoreWebView2Frame2>() {
        let w = wide(js);
        let done = ExecuteScriptCompletedHandler::create(Box::new(|_, _| Ok(())));
        let _ = unsafe { f2.ExecuteScript(PCWSTR(w.as_ptr()), &done) };
    }
}

/// Follow one iframe (and the frames inside it): its navigations, its DOM, its generic-rule asks.
fn hook_frame(frame: ICoreWebView2Frame, frames: &Rc<Frames>) {
    let Ok(frame2) = frame.cast::<ICoreWebView2Frame2>() else {
        return;
    };
    let mut token = Default::default();
    let url: Rc<RefCell<String>> = Rc::default();
    {
        let (frames, url) = (frames.clone(), url.clone());
        let handler = FrameNavigationStartingEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut uri = PWSTR::null();
            unsafe { args.Uri(&mut uri)? };
            let uri = take(uri);
            frames.frame_navigating(&uri);
            *url.borrow_mut() = uri;
            Ok(())
        }));
        let _ = unsafe { frame2.add_NavigationStarting(&handler, &mut token) };
    }
    {
        let (frames, url) = (frames.clone(), url.clone());
        let handler = FrameDOMContentLoadedEventHandler::create(Box::new(move |sender, _| {
            let host = crate::extract_host(&url.borrow());
            let (Some(sender), Some(host)) = (sender, host) else {
                return Ok(());
            };
            let page = frames.page();
            let top_host = crate::extract_host(&page).unwrap_or_default();
            let mode = (frames.mode_for)(&page);
            if let Some(js) = frame_script(&frames.shield, mode, &[host], &top_host, false) {
                execute_in_frame(&sender, &js);
            }
            Ok(())
        }));
        let _ = unsafe { frame2.add_DOMContentLoaded(&handler, &mut token) };
    }
    {
        let (frames, url) = (frames.clone(), url.clone());
        let handler = FrameWebMessageReceivedEventHandler::create(Box::new(move |sender, args| {
            let (Some(sender), Some(args)) = (sender, args) else {
                return Ok(());
            };
            let mut msg = PWSTR::null();
            if unsafe { args.TryGetWebMessageAsString(&mut msg) }.is_err() {
                return Ok(());
            }
            let msg = take(msg);
            let Some(body) = msg.strip_prefix("byobu:").filter(|b| b.starts_with('{')) else {
                return Ok(());
            };
            let mode = (frames.mode_for)(&frames.page());
            if let Some(js) = generic_reply(&frames.shield, &url.borrow(), body, mode) {
                execute_in_frame(&sender, &js);
            }
            Ok(())
        }));
        let _ = unsafe { frame2.add_WebMessageReceived(&handler, &mut token) };
    }
    if let Ok(frame7) = frame.cast::<ICoreWebView2Frame7>() {
        let frames = frames.clone();
        let handler = FrameChildFrameCreatedEventHandler::create(Box::new(move |_, args| {
            if let Some(args) = args {
                hook_frame(unsafe { args.Frame()? }, &frames);
            }
            Ok(())
        }));
        let _ = unsafe { frame7.add_FrameCreated(&handler, &mut token) };
    }
}

/// The script answering a frame's `byobu:{"c":[...],"i":[...]}` ask: the generic selectors that
/// apply to the classes and ids it uses.
fn generic_reply(shield: &Shield, url: &str, body: &str, mode: Mode) -> Option<String> {
    let ask = serde_json::from_str::<serde_json::Value>(body).ok()?;
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
    let list = shield.generic_hide(url, &strings("c"), &strings("i"), mode);
    (!list.is_empty()).then(|| {
        format!(
            "window.__byobuHide&&window.__byobuHide({})",
            serde_json::to_string(&list).unwrap_or_else(|_| "[]".into())
        )
    })
}

/// Picks the running frame's entry out of [`frame_script`]'s data, as the runtime's payload.
const FRAME_PICK: &str = "(function(D){var e=D.H[location.hostname];\
if(!e||window===window.top)return null;\
return{host:location.hostname,hide:e.h.map(function(i){return D.S[i]}),\
procedural:e.p,generic:e.g}})";

/// Document-start script for iframes on `hosts` (the top page's own host is left to the page
/// script). Selectors shared between hosts, like the generic ones, are sent once.
fn frame_script(
    shield: &Shield,
    mode: Mode,
    hosts: &[String],
    top_host: &str,
    scriptlets: bool,
) -> Option<String> {
    if mode == Mode::Off {
        return None;
    }
    let mut table: Vec<String> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut entries = serde_json::Map::new();
    let mut scripts = String::new();
    for host in hosts.iter().filter(|h| h.as_str() != top_host) {
        let payload = shield.cosmetic(&format!("https://{host}/"), mode);
        let ids: Vec<usize> = payload
            .hide
            .into_iter()
            .map(|s| {
                *index.entry(s).or_insert_with_key(|s| {
                    table.push(s.clone());
                    table.len() - 1
                })
            })
            .collect();
        let procedural: Vec<serde_json::Value> = payload
            .procedural
            .iter()
            .filter_map(|p| serde_json::from_str(p).ok())
            .collect();
        entries.insert(
            host.clone(),
            serde_json::json!({ "h": ids, "p": procedural, "g": !payload.generichide }),
        );
        if scriptlets && !payload.script.is_empty() {
            let h = serde_json::to_string(host).unwrap_or_default();
            scripts.push_str(&format!(
                "\nif(location.hostname==={h}&&window!==window.top){{try{{{}}}catch(e){{}}}}",
                payload.script
            ));
        }
    }
    if entries.is_empty() {
        return None;
    }
    let data = js_data(&serde_json::json!({ "S": table, "H": entries }));
    let mut js = COSMETIC_RUNTIME.replace("__BYOBU_PAYLOAD__", &format!("{FRAME_PICK}({data})"));
    js.push_str(&scripts);
    Some(js)
}

/// JSON as a JS literal: JSON is valid JS except for these two line separators.
fn js_data(v: &serde_json::Value) -> String {
    v.to_string()
        .replace('\u{2028}', "\\u2028")
        .replace('\u{2029}', "\\u2029")
}
/// The in-page fingerprinting protection (see `ui/fingerprint.js`).
const FINGERPRINT: &str = include_str!("../ui/fingerprint.js");

/// A random key for this run of Shoji: fingerprinting noise is stable for a site until Shoji
/// restarts, and never the same between sites.
fn session_key() -> u64 {
    static KEY: OnceLock<u64> = OnceLock::new();
    *KEY.get_or_init(|| {
        let mut b = [0u8; 8];
        let _ = ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut b);
        u64::from_le_bytes(b)
    })
}

/// The seed a page's noise comes from: its site (registrable domain) and the session key, so
/// `a.example.com` and `b.example.com` agree and `other.com` differs.
fn fingerprint_seed(host: &str, key: u64) -> u32 {
    use std::hash::{Hash, Hasher};
    let site = psl::domain_str(host).unwrap_or(host);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (key, site).hash(&mut h);
    h.finish() as u32
}

/// Document-start script that adds fingerprinting noise on `url` (runs in every frame, seeded
/// from the top page's site, as Brave does).
fn fingerprint_script(url: &str, fp: Fp) -> Option<String> {
    if fp == Fp::Off || !is_web(url) {
        return None;
    }
    let host = crate::extract_host(url)?;
    let data = serde_json::json!({
        "seed": fingerprint_seed(&host, session_key()),
        "strict": fp == Fp::Strict,
    });
    Some(FINGERPRINT.replace("__SHOJI_FP__", &js_data(&data)))
}

/// Global Privacy Control as a page-visible signal (the `Sec-GPC` header is sent separately).
const GPC_SCRIPT: &str = "try{Object.defineProperty(Navigator.prototype,'globalPrivacyControl',{get:function(){return true},configurable:true,enumerable:true})}catch(e){}";

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
    let mut js = COSMETIC_RUNTIME.replace("__BYOBU_PAYLOAD__", &js_data(&data));
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
