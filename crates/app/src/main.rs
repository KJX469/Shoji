#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Shoji — a paper-thin browser shell around the system WebView2 runtime.

mod bookmarks;
mod cert;
mod crash;
mod darkfixes;
mod default_browser;
mod extstore;
mod icons;
mod import_win;
mod keys;
mod pages;
mod shield;
mod snapghost;
mod tabdrag;
mod tray;
mod update;
mod vault_win;
mod webview;
mod window;

use keys::Command;
use shoji_brand::Accent;
use shoji_core::env::{detect_webview2_runtime, is_process_elevated};
use shoji_core::ipc::{HostToUiMessage, UiToHostMessage};
use shoji_core::settings::Settings;
use shoji_core::tabs::{normalize_url, SplitOrientation, TabId, TabManager, TabState, TabStatus};
use shoji_core::INTERNAL_SCHEME;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use webview::Origin;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalPosition, LogicalSize, Position, Size};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{Window, WindowId};
use wry::{Rect, WebView, WebViewBuilder, WebViewBuilderExtWindows, WebViewExtWindows};

/// Whether the engine was created with browser extensions on. Every webview must agree, because
/// they share one engine; it is read once at startup.
static EXTENSIONS_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn new_builder<'a>() -> WebViewBuilder<'a> {
    WebViewBuilder::new()
        .with_browser_extensions_enabled(EXTENSIONS_ON.load(std::sync::atomic::Ordering::Relaxed))
}

const CHROME_HEIGHT: f64 = 84.0;
/// The bookmarks bar under the address bar, when it is showing
const BAR_HEIGHT: f64 = 34.0;
/// Height of the suggestions window (eight two-line rows and the padding)
const SUGGEST_MAX_HEIGHT: f64 = 8.0 * 46.0 + 12.0;
/// Width of the side panel while it is pinned open
const SIDEBAR_WIDTH: f64 = 320.0;
/// Corner radius of the floating dropdowns, and how long they take to fold away
const POP_RADIUS: f64 = 14.0;
const POP_CLOSE_MS: u64 = 140;
/// How long the pointer may be off a hover-opened dropdown and its trigger before it folds away
const HOVER_LEAVE_MS: u64 = 120;

/// Width of the dropdown for each side-panel mode.
fn pop_width(mode: &str) -> f64 {
    match mode {
        "menu" => 300.0,
        "downloads" => 372.0,
        "bookmarks" => 360.0,
        "dark" => 348.0,
        _ => 340.0,
    }
}

/// The toolbar button a dropdown hangs from (a key of the chrome strip's anchor report).
fn pop_anchor(mode: &str) -> &'static str {
    match mode {
        "security" => "lock",
        "shield" => "shield",
        "downloads" => "downloads",
        "dark" => "dark",
        "bookmarks" => "panel",
        _ => "menu",
    }
}
/// Whether the left mouse button is physically down (a lost mouse capture never reports release).
#[cfg(windows)]
fn left_button_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON};
    (unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16 & 0x8000) != 0
}
#[cfg(not(windows))]
fn left_button_down() -> bool {
    true
}
/// Draggable divider width/height between the two panes of split view
const SPLIT_GAP: f64 = 10.0;
/// Floating notepad, anchored to the top-right of the page area
const NOTES_WIDTH: f64 = 440.0;
const NOTES_MAX_HEIGHT: f64 = 620.0;
const NOTES_MARGIN: f64 = 10.0;
const NOTES_EXPANDED_MAX_WIDTH: f64 = 920.0;
/// Notepad motion: unrolls like a hand scroll on open, rolls up on close.
const NOTES_OPEN_MS: f32 = 300.0;
const NOTES_CLOSE_MS: f32 = 190.0;
const NOTES_RESIZE_MS: f32 = 260.0;

/// x, y, width, height in logical pixels
type Frame = [f64; 4];

/// An in-flight notepad clip animation, stepped from the event loop.
///
/// The notepad webview itself never moves or resizes while open (WebView2 can't repaint fast
/// enough to follow a per-frame resize, so it shows stale, clipped frames). Instead it covers the
/// whole desk area, the page moves its sheet with CSS, and the host animates only the window
/// region, which clips drawing and clicks to the sheet and needs no repaint.
struct NotesAnim {
    start: Instant,
    duration: Duration,
    from: Frame,
    to: Frame,
    closing: bool,
}

fn frame_rect(f: Frame) -> Rect {
    bounds(f[0], f[1], f[2], f[3])
}

/// The page-side half of a notepad move: where the sheet sits in the desk, and how it gets there.
fn notes_layout_js(phase: &str, sheet: Frame, ms: f32) -> String {
    format!(
        "window.__notesLayout && window.__notesLayout({{phase:'{phase}',sheet:[{},{},{},{}],ms:{ms}}})",
        sheet[0], sheet[1], sheet[2], sheet[3]
    )
}
/// Internal pages are served from `http://shoji.<page>/` by wry's custom-protocol bridge.
const INTERNAL_ORIGIN: &str = "http://shoji.";

/// Internal pages carry everything they run inline: no script, frame, connection or form may come
/// from anywhere else, so markup that slipped into one couldn't load code that talks to the host.
const INTERNAL_CSP: &str = concat!(
    "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; ",
    "img-src http: https: data: blob:; font-src http://shoji.fonts data:; media-src data: blob:; ",
    "base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
);

/// Destination-URL bubble shown while hovering links (opt-out in Settings).
const LINK_PREVIEW_SCRIPT: &str = r#"(() => {
  if (window.top !== window) return;
  let tip, shown = false;
  const ensure = () => {
    if (tip) return tip;
    tip = document.createElement('div');
    tip.style.cssText = 'all:initial;position:fixed;left:8px;bottom:8px;max-width:min(560px,70vw);padding:4px 10px;border-radius:7px;background:rgba(34,33,31,.92);color:#ebe6dc;font:11.5px "Segoe UI Variable Text","Segoe UI",sans-serif;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;z-index:2147483647;pointer-events:none;opacity:0;transition:opacity .12s';
    document.documentElement.appendChild(tip);
    return tip;
  };
  addEventListener('mouseover', e => {
    const a = e.target.closest && e.target.closest('a[href]');
    if (a) { const t = ensure(); t.textContent = a.href; t.style.opacity = '1'; shown = true; }
    else if (shown) { tip.style.opacity = '0'; shown = false; }
  }, { passive: true });
})();"#;

/// Lets a link dragged from one pane be dropped onto the other pane to navigate it.
const SPLIT_DROP_SCRIPT: &str = r#"(() => {
  if (window.top !== window || !window.ipc) return;
  const editable = el => el && (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA' || el.tagName === 'SELECT' || el.isContentEditable);
  addEventListener('dragstart', e => {
    const a = e.target && e.target.closest && e.target.closest('a[href]');
    if (a && a.href && e.dataTransfer) {
      try { e.dataTransfer.setData('text/uri-list', a.href); } catch (_) {}
    }
  }, true);
  addEventListener('dragover', e => {
    if (editable(e.target) || !e.dataTransfer) return;
    const types = [...(e.dataTransfer.types || [])];
    if (types.includes('text/uri-list') || types.includes('text/plain')) {
      e.preventDefault();
      e.dataTransfer.dropEffect = 'link';
    }
  });
  addEventListener('drop', e => {
    if (editable(e.target) || !e.dataTransfer) return;
    const raw = (e.dataTransfer.getData('text/uri-list') || e.dataTransfer.getData('text/plain') || '').trim();
    const line = raw.split(/\r?\n/).find(l => l && !l.startsWith('#')) || '';
    if (/^https?:\/\//i.test(line)) {
      e.preventDefault();
      e.stopPropagation();
      window.ipc.postMessage(JSON.stringify({ action: 'LinkDropped', payload: { url: line.trim() } }));
    }
  });
})();"#;

#[derive(Debug)]
pub(crate) enum BrowserEvent {
    /// From the tray icon, or a later launch of Shoji handed to this copy
    Tray(tray::Action),
    /// Progress text from an import running on a worker thread
    ImportProgress {
        win: WindowId,
        text: String,
    },
    /// An import finished reading; its data is ready to be applied
    ImportDone {
        win: WindowId,
        imported: Box<shoji_core::import::Imported>,
    },
    /// GitHub answered (or didn't): a newer release, nothing new, or why it couldn't be asked
    UpdateChecked {
        result: Result<Option<update::Release>, String>,
        manual: bool,
    },
    /// How far the agreed download has got (0..=100)
    UpdateProgress {
        pct: u32,
    },
    /// The agreed download finished: a checksum-verified installer, or why not
    UpdateDownloaded {
        version: String,
        result: Result<PathBuf, String>,
    },
    /// From the chrome strip or side panel of a window
    WindowIpc(WindowId, UiToHostMessage),
    /// From a page (already filtered: web content may only send harmless messages)
    TabIpc(TabId, UiToHostMessage),
    Command(Origin, Command),
    TabUpdate {
        tab_id: TabId,
        url: Option<String>,
        title: Option<String>,
        history: Option<(bool, bool)>,
        favicon: Option<Option<String>>,
    },
    NewTabFrom(TabId, String),
    /// A page opened a window and is waiting for one (see `webview::PopupRequest`)
    PopupRequested(webview::PopupRequest),
    PermissionPrompt {
        tab_id: TabId,
        permission_id: u64,
        origin: String,
        kind: &'static str,
    },
    TabCrashed(TabId, i32),
    /// A tab passed through `host` as a redirect hop, or the user visited it (see `bounce`)
    Bounce {
        host: String,
        hop: bool,
    },
    /// A tab navigated to a site that always opens in another container
    OpenInContainer {
        tab_id: TabId,
        url: String,
        container: Option<String>,
    },
    CertificateError {
        tab_id: TabId,
        host: String,
        uri: String,
    },
    CertificateReady(String),
    DownloadProgress {
        download_id: u64,
        filename: String,
        /// Full path the file is being saved to
        path: String,
        /// Host the file came from (may be empty)
        source: String,
        received_bytes: i64,
        total_bytes: i64,
        state: &'static str,
    },
    TabSuspended(TabId),
    /// A page could not be found (`offline` false) or the network is down (`offline` true)
    Unreachable {
        tab_id: TabId,
        uri: String,
        offline: bool,
    },
    /// A tab started or finished loading a page
    TabLoading {
        tab_id: TabId,
        loading: bool,
    },
    /// The engine's list of installed extensions (JSON)
    ExtensionsListed(String),
    /// An extension was added or removed (`Some` is why it failed); the list needs a refresh
    ExtensionsChanged(Option<String>),
    /// A store extension was downloaded and unpacked; the engine can load this folder now
    ExtensionFetched(Result<PathBuf, String>),
    /// The search engine's suggestions for what was typed in a window's address bar
    SuggestWords(WindowId, u64, Vec<String>, bool),
    /// Cookies of a window's page: how many, and whether they were just deleted
    SiteCookies(WindowId, usize, bool),
    /// Audio playback or mute state changed on `tab_id`
    TabAudio {
        tab_id: TabId,
        playing: bool,
        muted: bool,
    },
    /// HTML5 fullscreen state changed on `tab_id`
    TabFullscreen {
        tab_id: TabId,
        fullscreen: bool,
    },
    /// Byobu blocked something for this tab (or its lists changed: tab 0)
    ShieldTouched(TabId),
    /// Byobu stopped a page from opening (a `$document` rule matched)
    ShieldBlockedPage {
        tab_id: TabId,
        url: String,
    },
    /// The element picker produced a rule for `host`
    ShieldRule {
        host: String,
        selector: String,
    },
    /// The user clicked into a tab's page (split view: that pane becomes the active one)
    TabFocused(TabId),
    /// An https:// navigation could not connect (HTTPS-only mode may offer a fallback)
    NavFailed {
        tab_id: TabId,
        uri: String,
    },
    /// The reader script finished: JSON of {title, byline, site, html}, or "null"
    ReaderReady {
        tab_id: TabId,
        json: String,
    },
}

/// What internal pages need to render; shared with the custom-protocol handler.
struct PageContext {
    settings: Settings,
    engine_version: String,
    data_dir: PathBuf,
    /// One-off notice pages (crash, certificate, reader) keyed by tab
    notices: HashMap<u64, String>,
    favourites: Vec<shoji_core::userdata::Favourite>,
    recent: Vec<shoji_core::tabs::ClosedTab>,
    /// Tabs from a crashed session offered on the new tab page
    restore_tabs: usize,
    /// Number of remembered per-site choices (shown in Settings)
    site_choices: usize,
    /// Paper cranes folded so far, for the wish page
    cranes: u32,
    /// When Shoji and its user met, and the hidden things found so far
    met: u64,
    found: Vec<String>,
    /// Days since Shoji was last open, and whether this is the very first visit; said once,
    /// on the first new tab after a start (or a return from the tray)
    away_days: u64,
    first_visit: bool,
    greet_once: std::sync::atomic::AtomicBool,
}

type SharedPages = Arc<RwLock<PageContext>>;

/// What the crane remembers, for the new tab, the wish page and Settings. `greet` lets the
/// once-only words through (welcome back, a first visit).
fn memory_json(ctx: &PageContext, greet: bool) -> String {
    serde_json::json!({
        "met": ctx.met,
        "found": ctx.found,
        "secrets": shoji_core::userdata::SECRETS,
        "cranes": ctx.cranes,
        "away": if greet { ctx.away_days } else { 0 },
        "first": greet && ctx.first_visit,
    })
    .to_string()
    .replace("</", "<\\/")
}

fn internal_page(ctx: &PageContext, page: &str) -> Option<String> {
    let s = &ctx.settings;
    Some(match page {
        "newtab" => pages::home(
            s,
            &pages::HomeData {
                favourites: &ctx.favourites,
                recent: &ctx.recent,
                restore_tabs: ctx.restore_tabs,
            },
            &memory_json(
                ctx,
                ctx.greet_once
                    .swap(false, std::sync::atomic::Ordering::Relaxed),
            ),
        ),
        "welcome" => pages::welcome(s),
        "passwords" => pages::passwords(s),
        "history" => pages::history(s, "history"),
        "downloads" => pages::history(s, "downloads"),
        "bookmarks" => pages::bookmarks(s),
        "wish" => pages::fun(s, "wish", ctx.cranes, &memory_json(ctx, false)),
        "tea" => pages::fun(s, "tea", ctx.cranes, &memory_json(ctx, false)),
        "koan" => pages::fun(s, "koan", ctx.cranes, &memory_json(ctx, false)),
        "byobu" => pages::byobu(s),
        "settings" => pages::settings(
            s,
            &ctx.engine_version,
            &ctx.data_dir,
            ctx.site_choices,
            &memory_json(ctx, false),
        ),
        _ => {
            let id = page.strip_prefix("notice-")?.parse().ok()?;
            ctx.notices.get(&id)?.clone()
        }
    })
}

/// Serves the bundled UI fonts at `http://shoji.fonts/<file>` to every Shoji surface.
/// The chrome strip and side panel load from inline HTML, so the response allows any origin.
fn font_response(request: &wry::http::Request<Vec<u8>>) -> wry::http::Response<Cow<'static, [u8]>> {
    match pages::font_file(request.uri().path().trim_start_matches('/')) {
        Some(bytes) => wry::http::Response::builder()
            .header("Content-Type", "font/woff2")
            .header("Cache-Control", "max-age=31536000, immutable")
            .header("Access-Control-Allow-Origin", "*")
            .body(Cow::Borrowed(bytes)),
        None => wry::http::Response::builder()
            .status(404)
            .body(Cow::Borrowed(&b"Not found"[..])),
    }
    .unwrap_or_else(|_| wry::http::Response::new(Cow::Borrowed(&[][..])))
}

/// `http://shoji.settings/` -> `shoji://settings`
fn display_url(source: &str) -> Cow<'_, str> {
    match source.strip_prefix(INTERNAL_ORIGIN) {
        Some(rest) => Cow::Owned(format!("{INTERNAL_SCHEME}{}", rest.trim_end_matches('/'))),
        None => Cow::Borrowed(source),
    }
}

/// `shoji://settings` -> `http://shoji.settings/`
fn engine_url(url: &str) -> Cow<'_, str> {
    match url.strip_prefix(INTERNAL_SCHEME) {
        Some(page) => Cow::Owned(format!("{INTERNAL_ORIGIN}{}/", page.trim_end_matches('/'))),
        None => Cow::Borrowed(url),
    }
}

/// Hides the page's own scrollbar on the right; scrolling itself is unchanged.
const HIDE_SCROLLBAR_SCRIPT: &str = "(() => { try { const s = new CSSStyleSheet();     s.replaceSync('html { scrollbar-width: none !important; } html::-webkit-scrollbar { display: none !important; }');     document.adoptedStyleSheets = [...document.adoptedStyleSheets, s]; } catch (e) {} })();";

/// With the shield off the engine's own tracking prevention goes off too.
fn tracking_level(settings: &Settings) -> webview::Tracking {
    if settings.shield.mode == "off" {
        webview::Tracking::Off
    } else if settings.privacy.strict_tracking_prevention {
        webview::Tracking::Strict
    } else {
        webview::Tracking::Default
    }
}

/// Is `url` one of Shoji's own pages (`http://shoji.<page>/…`)? Only those are served by Shoji
/// itself, so only their messages are trusted. A look-alike with credentials or a port in it
/// (`http://shoji.x@site/`) is not one, whatever the engine would make of it.
fn is_internal_origin(url: &str) -> bool {
    let Some(rest) = url.strip_prefix(INTERNAL_ORIGIN) else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    !authority.is_empty()
        && authority
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.')
}

/// The engine profile behind a container (`c-work`); temporary ones are `c-temp-…`.
fn container_profile(id: &str) -> String {
    format!("c-{id}")
}

fn is_temp_container(id: &str) -> bool {
    id.starts_with(shoji_core::settings::TEMP_CONTAINER)
}

fn is_internal(url: &str) -> bool {
    url.starts_with(INTERNAL_SCHEME) || url == "about:blank"
}

pub(crate) fn extract_host(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let host = authority.rsplit('@').next()?.split(':').next()?;
    (!host.is_empty()).then(|| host.to_ascii_lowercase())
}

fn bounds(x: f64, y: f64, w: f64, h: f64) -> Rect {
    Rect {
        position: Position::Logical(LogicalPosition::new(x, y)),
        size: Size::Logical(LogicalSize::new(w.max(1.0), h.max(1.0))),
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

/// Tab-group and workspace colours, in the order new groups/workspaces pick them (crane accent ids).
const GROUP_COLORS: [&str; 7] = ["blue", "green", "gold", "purple", "red", "cyan", "white"];

/// Runs in the page (with a user gesture) to toggle picture-in-picture on its main video.
const PIP_SCRIPT: &str = r#"(async () => {
  if (document.pictureInPictureElement) { await document.exitPictureInPicture(); return; }
  const vids = [...document.querySelectorAll('video')].filter(v => v.readyState > 0 && !v.disablePictureInPicture);
  vids.sort((a, b) => b.videoWidth * b.videoHeight - a.videoWidth * a.videoHeight);
  const v = vids.find(v => !v.paused) || vids[0];
  if (v) await v.requestPictureInPicture();
})()"#;

/// The first crane colour no group in this window uses yet.
fn next_group_color(win: &BrowserWindow) -> &'static str {
    GROUP_COLORS
        .iter()
        .find(|c| !win.tabs.groups().iter().any(|g| g.color == **c))
        .copied()
        .unwrap_or(GROUP_COLORS[win.tabs.groups().len() % GROUP_COLORS.len()])
}

/// The first crane colour no tab group or split group in this window uses yet.
fn next_split_color(win: &BrowserWindow) -> &'static str {
    GROUP_COLORS
        .iter()
        .find(|c| {
            !win.tabs.splits().iter().any(|s| s.color == **c)
                && !win.tabs.groups().iter().any(|g| g.color == **c)
        })
        .copied()
        .unwrap_or(GROUP_COLORS[win.tabs.splits().len() % GROUP_COLORS.len()])
}

/// The first crane colour no workspace in this window uses yet.
fn next_workspace_color(win: &BrowserWindow) -> &'static str {
    GROUP_COLORS
        .iter()
        .find(|c| !win.tabs.workspaces().iter().any(|w| w.color == **c))
        .copied()
        .unwrap_or(GROUP_COLORS[win.tabs.workspaces().len() % GROUP_COLORS.len()])
}

/// The display tone of a group's colour.
fn group_tone(win: &BrowserWindow, gid: u32) -> Option<String> {
    let g = win.tabs.groups().iter().find(|g| g.id == gid)?;
    Some(
        shoji_brand::theme::tones(Accent::from_id(&g.color))
            .0
            .to_string(),
    )
}

/// The display tone of a workspace's colour.
fn workspace_tone(win: &BrowserWindow, wid: u32) -> Option<String> {
    let w = win.tabs.workspaces().iter().find(|w| w.id == wid)?;
    Some(
        shoji_brand::theme::tones(Accent::from_id(&w.color))
            .0
            .to_string(),
    )
}

/// Move `tab` next to the other members of its group, so a group stays contiguous in the strip.
fn gather_group(win: &mut BrowserWindow, tab: TabId, gid: u32) {
    let tabs = win.tabs.tabs();
    let Some(from) = tabs.iter().position(|t| t.id == tab) else {
        return;
    };
    let Some(last) = tabs
        .iter()
        .enumerate()
        .filter(|(_, t)| t.group == Some(gid) && t.id != tab)
        .map(|(i, _)| i)
        .next_back()
    else {
        return;
    };
    let to = if from > last { last + 1 } else { last };
    win.tabs.reorder_tab(from, to);
}

/// Reduce a CSS colour (`#rgb`, `#rrggbb`, `rgb()`/`rgba()`) to `#rrggbb`. Anything else, or a
/// mostly-transparent colour, gives `None`.
fn parse_css_color(input: &str) -> Option<String> {
    let s = input.trim().to_ascii_lowercase();
    if let Some(hex) = s.strip_prefix('#') {
        if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        return match hex.len() {
            3 => Some(format!(
                "#{}",
                hex.chars().flat_map(|c| [c, c]).collect::<String>()
            )),
            6 | 8 => Some(format!("#{}", &hex[..6])),
            _ => None,
        };
    }
    let inner = s
        .strip_prefix("rgba(")
        .or_else(|| s.strip_prefix("rgb("))?
        .strip_suffix(')')?;
    let nums: Vec<f64> = inner
        .split([',', ' ', '/'])
        .filter(|p| !p.is_empty())
        .map(|p| p.trim_end_matches('%').parse::<f64>().ok())
        .collect::<Option<_>>()?;
    if nums.len() < 3 || nums.len() > 4 || nums[..3].iter().any(|v| !(0.0..=255.0).contains(v)) {
        return None;
    }
    if nums.get(3).is_some_and(|a| *a < 0.5) {
        return None;
    }
    Some(format!(
        "#{:02x}{:02x}{:02x}",
        nums[0] as u8, nums[1] as u8, nums[2] as u8
    ))
}

fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/* ---------------------------------------------------------------- windows */

/// A login form the user submitted, held until they say whether to keep it.
struct PendingLogin {
    url: String,
    username: String,
    password: String,
    /// Already saved for this site and username with an older password
    existing: Option<String>,
    at: Instant,
}

/// How long a master-password vault stays open without being used.
const VAULT_IDLE: Duration = Duration::from_secs(15 * 60);

/// Whole days between a previous visit and now (0 when there was none).
fn away_days(previous: u64, now: u64) -> u64 {
    if previous == 0 {
        0
    } else {
        now.saturating_sub(previous) / 86_400_000
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

struct Tab {
    view: WebView,
    /// Cookie partition this view was built with (persistent vs. in-memory)
    persistent: bool,
    /// Container (engine profile) this view was built in
    container: Option<String>,
    hidden_since: Option<Instant>,
    suspended: bool,
    /// Registered Dark Reader document script, if any
    dark_script: webview::ScriptSlot,
    /// Registered interface-font script, if any
    font_script: webview::ScriptSlot,
    /// Whether the page is told to report a light colour scheme (see `darkfixes`)
    emulated: darkfixes::Emulated,
    /// Its network requests go through the shield (see `shield::wants_request_hook`)
    request_hook: bool,
    /// The page's own colour (`#rrggbb`), used to tint the toolbar
    tint: Option<String>,
    /// The page holds a video (shows the picture-in-picture button)
    has_video: bool,
    /// When the view was built
    born: Instant,
    /// The first load finished, so the view has put a frame on screen
    painted: bool,
    /// A restored tab not shown yet: its address loads the first time it is (see `load_pending`)
    pending_url: Option<String>,
}

/// How long a new tab view may stay hidden waiting for its first frame.
const VEIL_MAX: Duration = Duration::from_millis(1500);

impl Tab {
    /// A brand-new WebView2 shows black until it has painted, so the view that was showing stays
    /// up until this is true.
    fn settled(&self) -> bool {
        self.painted || self.born.elapsed() > VEIL_MAX
    }

    /// Start loading a restored tab's page, if it has not been loaded yet.
    fn load_pending(&mut self) {
        if let Some(url) = self.pending_url.take() {
            self.born = Instant::now();
            let _ = self.view.load_url(&engine_url(&url));
        }
    }
}

/// What a text prompt in the overlay is asking for.
#[derive(Clone)]
enum Prompt {
    NewGroup(TabId),
    RenameGroup(u32),
    RenameSplit(u32),
    /// Name of a new folder to file the bookmark with this address in
    NewFolderFor(String),
    NewWorkspace,
    RenameWorkspace(u32),
    /// New name for the bookmark with this address
    BookmarkName(String),
    /// New name for the bookmark folder at this path
    RenameBookmarkFolder(String),
    /// Name of a new bookmark folder inside this one ("" for the top level)
    NewBookmarkFolder(String),
}

struct BrowserWindow {
    window: Arc<Window>,
    tabs: TabManager,
    views: HashMap<TabId, Tab>,
    chrome: Option<WebView>,
    /// The dropdown behind the toolbar's menu, site, shield, downloads and dark buttons. It
    /// floats over the page; only when pinned (`sidebar_docked`) does it take page width.
    sidebar: Option<WebView>,
    sidebar_open: bool,
    sidebar_mode: &'static str,
    sidebar_docked: bool,
    /// Natural height of the dropdown's content, reported by its page
    pop_height: f64,
    /// When the closing animation ends and the dropdown can be hidden
    pop_hide_at: Option<Instant>,
    /// When the dropdown was last opened (focus settling just after it is not a click away)
    pop_opened: Option<Instant>,
    /// When (and which) dropdown a click elsewhere just closed, so the button that opened it
    /// does not reopen it in the same click
    pop_blurred: Option<(Instant, &'static str)>,
    /// Horizontal centre of each toolbar button, reported by the chrome strip
    anchors: HashMap<String, f64>,
    /// The dropdown opened because the pointer rested on a trigger: it folds away when the
    /// pointer is off both, until something inside it is pressed
    pop_hover: bool,
    /// The span of that trigger, and whether it sits in the bookmarks bar
    pop_span: Option<(f64, f64)>,
    pop_top: Option<f64>,
    /// Since when the pointer has been off a hover-opened dropdown and its trigger
    hover_out_since: Option<Instant>,
    /// The dropdown page reports the pointer on it (see `step_hover`)
    pop_pointer_in: bool,
    /// When to build the dropdown page in the background, so its first opening is instant
    sidebar_warm: Option<Instant>,
    /// The bookmark the "Save to" card is filing
    save_url: Option<String>,
    notes: Option<WebView>,
    notes_open: bool,
    notes_expanded: bool,
    /// The visible part of the notepad, relative to its desk (None while hidden)
    notes_clip: Option<Frame>,
    /// The notepad page has loaded (until then there is nothing to unroll)
    notes_ready: bool,
    /// The dropdown page has loaded and painted. A new WebView2 shows black until its first
    /// frame, so it is only made visible after this.
    sidebar_ready: bool,
    /// A tab that was switched to before its first frame; see [`Tab::settled`]
    veil: Option<TabId>,
    /// Last script handed to the dropdown, repeated once it is ready
    sidebar_pending: Option<String>,
    notes_anim: Option<NotesAnim>,
    width: f64,
    height: f64,
    find_open: bool,
    /// The last address recorded in history for each tab, so a visit counts once
    recorded: HashMap<TabId, String>,
    /// The text last searched for, for F3 / Ctrl+G
    find_query: String,
    /// The window is full screen with the toolbar hidden
    fullscreen: bool,
    permission_open: bool,
    /// The chrome strip currently has a window region set (cards are showing)
    chrome_clipped: bool,
    /// Frames of the floating find and permission cards (None while closed)
    tray_rects: [Option<Frame>; 4],
    /// Horizontal centres of the tabs in the strip (logical px), reported by the strip
    tab_centers: Vec<f64>,
    /// Tab strip needs a sync at the end of this event-loop turn
    ui_dirty: bool,
    sidebar_dirty: bool,
    last_ui_json: String,
    /// Last frame state pushed to the chrome strip (maximized, focused)
    frame_state: Option<(bool, bool)>,
    last_title: String,
    /// Floating surface for address-bar suggestions, the command palette, menus and prompts
    overlay: Option<WebView>,
    overlay_open: bool,
    overlay_mode: &'static str,
    overlay_ready: bool,
    /// The address bar should be made active once the toolbar has its first state (a window
    /// that opens on the new tab page)
    focus_address: bool,
    /// Script waiting for the overlay page to finish loading
    overlay_pending: Option<String>,
    /// When the overlay was last shown (a blur right after showing is ignored)
    overlay_shown: Option<Instant>,
    overlay_items: Vec<shoji_core::ipc::OverlayItem>,
    prompt: Option<Prompt>,
    /// Tab currently in HTML5 fullscreen (e.g. fullscreen video in a split pane)
    fullscreen_tab: Option<TabId>,
    /// Whether the user is dragging a split divider, and which one (0 = between panes 0 and 1)
    divider_dragging: bool,
    divider_index: usize,
    /// Distance from the divider's centre to the cursor when the drag began (along its axis)
    divider_grab: f64,
    /// A drag moved the divider; the panes are re-laid out once per event-loop turn
    pending_split_layout: bool,
    /// Last cursor logical coordinates in this window
    cursor_pos: (f64, f64),
    /// Last click time on the split divider (for double-click reset to 50/50)
    last_divider_click: Option<Instant>,
    /// Tabs currently showing the reader view
    reader: HashSet<TabId>,
    /// Last tint / page-state pushed to the chrome strip
    last_page_sync: String,
    /// Pane rectangles last sent to the toolbar strip (so a drag only sends what changed)
    last_pane_rects: String,
    /// Per-pane shield / reader / video state last sent to the toolbar strip
    last_pane_state: String,
    /// Split pills in the strip: (split id, left, right) in logical px
    split_zones: Vec<(u32, f64, f64)>,
    /// Where each split pane was last placed, so a divider drag moves only the panes that changed
    applied_panes: HashMap<TabId, (f64, f64, f64, f64)>,
    last_fav_sync: String,
    /// A private window: its tabs keep nothing (no cookies, history, logins or saved session)
    private: bool,
    /// The bookmarks bar is showing under the address bar
    bar: bool,
    /// Where the last bookmark menu opened (x, y, width), so a folder inside it opens beside it
    menu_at: (f64, f64, f64),
}

impl BrowserWindow {
    /// Height of the title bar and toolbar. The find and permission cards float over the page,
    /// so this never changes and the page is never resized by them.
    fn chrome_height(&self) -> f64 {
        if self.fullscreen {
            0.0
        } else if self.bar {
            CHROME_HEIGHT + BAR_HEIGHT
        } else {
            CHROME_HEIGHT
        }
    }

    /// Width the page gives up to a pinned side panel (floating dropdowns take none).
    fn docked_width(&self) -> f64 {
        if self.sidebar_open && self.sidebar_docked {
            SIDEBAR_WIDTH
        } else {
            0.0
        }
    }

    fn content_tuple(&self) -> (f64, f64, f64, f64) {
        let top = self.chrome_height();
        let width = (self.width - self.docked_width()).max(1.0);
        (0.0, top, width, (self.height - top).max(1.0))
    }

    fn content_bounds(&self) -> Rect {
        let (x, y, w, h) = self.content_tuple();
        bounds(x, y, w, h)
    }

    /// Where the floating dropdown sits: under its toolbar button, right edge kept inside.
    fn popover_frame(&self) -> Frame {
        let w = pop_width(self.sidebar_mode).min((self.width - 16.0).max(200.0));
        let top = self.pop_top.unwrap_or(CHROME_HEIGHT + 2.0);
        let max_h = (self.height - top - 12.0).max(120.0);
        let h = if self.pop_height > 0.0 {
            self.pop_height
        } else {
            260.0
        }
        .clamp(72.0, max_h);
        let cx = self.pop_cx();
        let x = (cx - w / 2.0).clamp(8.0, (self.width - w - 8.0).max(8.0));
        [x, top, w, h]
    }

    /// Horizontal centre of what the dropdown hangs from.
    fn pop_cx(&self) -> f64 {
        match self.pop_span {
            Some((l, r)) if self.pop_hover => (l + r) / 2.0,
            _ => self
                .anchors
                .get(pop_anchor(self.sidebar_mode))
                .copied()
                .unwrap_or(self.width - 40.0),
        }
    }

    /// The split-view panes, in screen order, when the active tab is in a split.
    fn split_showing(&self) -> Option<Vec<TabId>> {
        self.tabs.split_panes()
    }

    /// Which split divider `(x, y)` is on, if any.
    fn divider_hit(&self, x: f64, y: f64) -> Option<usize> {
        if self.fullscreen_tab.is_some() {
            return None;
        }
        let split = self.tabs.visible_split()?;
        split
            .divider_rects(self.content_tuple(), SPLIT_GAP)
            .iter()
            .position(|&(dx, dy, dw, dh)| x >= dx && x <= dx + dw && y >= dy && y <= dy + dh)
    }

    /// Centre of divider `index` along its drag axis, in window coordinates.
    fn divider_centre(&self, index: usize) -> Option<(SplitOrientation, f64)> {
        let split = self.tabs.visible_split()?;
        let (dx, dy, dw, dh) = *split
            .divider_rects(self.content_tuple(), SPLIT_GAP)
            .get(index)?;
        Some(match split.orientation {
            SplitOrientation::Vertical => (split.orientation, dx + dw / 2.0),
            SplitOrientation::Horizontal => (split.orientation, dy + dh / 2.0),
        })
    }

    /// Tell the toolbar strip where the split panes are, so each pane's bar sits above it.
    fn push_pane_rects(&mut self) {
        let json = match (self.fullscreen_tab, self.tabs.visible_split()) {
            (None, Some(s)) => serde_json::to_string(
                &s.layout(self.content_tuple(), SPLIT_GAP)
                    .into_iter()
                    .map(|r| [r.0, r.1, r.2, r.3])
                    .collect::<Vec<_>>(),
            )
            .unwrap_or_else(|_| "[]".into()),
            _ => "[]".into(),
        };
        if json != self.last_pane_rects {
            self.eval_chrome(&format!(
                "window.__splitRects && window.__splitRects({json})"
            ));
            self.last_pane_rects = json;
        }
    }

    /// Move only the split panes to their current rectangles (used while dragging a divider).
    fn layout_panes_only(&mut self) {
        self.pending_split_layout = false;
        self.push_pane_rects();
        let Some(split) = self.tabs.visible_split() else {
            return;
        };
        let panes = split.panes.clone();
        let rects = split.layout(self.content_tuple(), SPLIT_GAP);
        for (id, r) in panes.into_iter().zip(rects) {
            if self.applied_panes.get(&id) == Some(&r) {
                continue;
            }
            self.applied_panes.insert(id, r);
            if let Some(tab) = self.views.get(&id) {
                let _ = tab.view.set_bounds(bounds(r.0, r.1, r.2, r.3));
            }
        }
    }

    /// One frame of a divider drag, from where the mouse really is now. Reading the live cursor
    /// (instead of replaying queued move events) means a fast drag never lags behind a backlog.
    fn drive_divider(&mut self, cursor_screen: (i32, i32)) {
        if !left_button_down() {
            self.end_divider_drag();
            return;
        }
        let scale = self.window.scale_factor();
        let Ok(origin) = self.window.inner_position() else {
            return;
        };
        let (x, y) = (
            (cursor_screen.0 - origin.x) as f64 / scale,
            (cursor_screen.1 - origin.y) as f64 / scale,
        );
        let area = self.content_tuple();
        let Some(orientation) = self.tabs.visible_split().map(|s| s.orientation) else {
            self.end_divider_drag();
            return;
        };
        let along = if orientation == SplitOrientation::Horizontal {
            y - self.divider_grab
        } else {
            x - self.divider_grab
        };
        if self
            .tabs
            .drag_split_divider(self.divider_index, area, SPLIT_GAP, along)
        {
            self.layout_panes_only();
        }
    }

    /// End a divider drag; the chrome learns the final sizes once.
    fn end_divider_drag(&mut self) {
        if self.divider_dragging {
            self.divider_dragging = false;
            self.ui_dirty = true;
            self.window.set_cursor(winit::window::CursorIcon::Default);
        }
    }

    /// Where tab `id` sits: full window in fullscreen, its split pane rectangle, or the page area.
    fn rect_for(&self, id: TabId) -> Rect {
        if self.fullscreen_tab == Some(id) {
            return bounds(0.0, 0.0, self.width, self.height);
        }
        let area = self.content_tuple();
        if let Some(split) = self.tabs.visible_split() {
            if let Some(i) = split.panes.iter().position(|&p| p == id) {
                if let Some(r) = split.layout(area, SPLIT_GAP).get(i) {
                    return bounds(r.0, r.1, r.2, r.3);
                }
            }
        }
        bounds(area.0, area.1, area.2, area.3)
    }

    fn active(&self) -> Option<&Tab> {
        self.views.get(&self.tabs.active_tab_id()?)
    }

    /// Only the visible tab(s) are resized; hidden tabs get their bounds when shown.
    fn layout(&mut self) {
        self.push_pane_rects();
        self.applied_panes.clear();
        if let Some(fs_id) = self.fullscreen_tab {
            if let Some(chrome) = &self.chrome {
                let _ = chrome.set_visible(false);
            }
            if let Some(sidebar) = &self.sidebar {
                let _ = sidebar.set_visible(false);
            }
            for (&id, tab) in self.views.iter_mut() {
                if id == fs_id {
                    tab.load_pending();
                    let _ = tab
                        .view
                        .set_bounds(bounds(0.0, 0.0, self.width, self.height));
                    let _ = tab.view.set_visible(true);
                } else if self.tabs.is_tab_visible(id) {
                    let _ = tab.view.set_visible(false);
                }
            }
            return;
        }

        let top = self.chrome_height();
        if let Some(chrome) = &self.chrome {
            let _ = chrome.set_visible(true);
            let cards: Vec<Frame> = self.tray_rects.iter().flatten().copied().collect();
            if cards.is_empty() {
                let _ = chrome.set_bounds(bounds(0.0, 0.0, self.width, top));
                if self.chrome_clipped {
                    self.chrome_clipped = false;
                    webview::clear_region(chrome);
                }
            } else {
                self.chrome_clipped = true;
                // The strip grows to cover the cards but is clipped to the bar and the cards, so
                // the page underneath still shows and takes clicks everywhere else.
                let scale = self.window.scale_factor();
                let reach = cards.iter().map(|c| c[1] + c[3]).fold(top, f64::max) + 8.0;
                let _ = chrome.set_bounds(bounds(0.0, 0.0, self.width, reach));
                let px = |v: f64| (v * scale).round() as i32;
                let mut rects = vec![(0, 0, px(self.width), px(top), 0)];
                rects.extend(cards.iter().map(|c| {
                    (
                        px(c[0]),
                        px(c[1]),
                        px(c[0] + c[2]),
                        px(c[1] + c[3]),
                        px(12.0),
                    )
                }));
                webview::set_union_region(chrome, &rects);
                webview::bring_to_top(chrome);
            }
        }
        let visible: Vec<TabId> = match self.split_showing() {
            Some(pair) => pair.to_vec(),
            None => self.tabs.active_tab_id().into_iter().collect(),
        };
        for &id in &visible {
            let rect = self.rect_for(id);
            if let Some(tab) = self.views.get_mut(&id) {
                tab.load_pending();
                let _ = tab.view.set_bounds(rect);
                let _ = tab.view.set_visible(tab.settled());
            }
        }
        // A pane that is no longer showing (a split that was closed) must not stay on top.
        if self.veil.is_none() {
            for (id, tab) in self.views.iter_mut() {
                if !visible.contains(id) && tab.hidden_since.is_none() {
                    Self::hide(tab);
                }
            }
        }
        if let (true, Some(sidebar)) = (self.sidebar_open, &self.sidebar) {
            let _ = sidebar.set_visible(self.sidebar_ready);
            if self.sidebar_docked {
                let _ = sidebar.set_bounds(bounds(
                    self.width - SIDEBAR_WIDTH,
                    top,
                    SIDEBAR_WIDTH,
                    self.height - top,
                ));
                webview::clear_region(sidebar);
            } else {
                let f = self.popover_frame();
                let _ = sidebar.set_bounds(frame_rect(f));
                let scale = self.window.scale_factor();
                webview::set_round_region(
                    sidebar,
                    (f[2] * scale).round() as i32,
                    (f[3] * scale).round() as i32,
                    (POP_RADIUS * scale).round() as i32,
                );
                webview::bring_to_top(sidebar);
            }
        }
        if let (true, Some(n)) = (self.notes_open, &self.notes) {
            let sheet = self.notes_sheet();
            let _ = n.set_bounds(frame_rect(self.notes_desk()));
            let _ = n.evaluate_script(&notes_layout_js("place", sheet, 0.0));
            match &mut self.notes_anim {
                // Mid-animation: retarget instead of snapping.
                Some(anim) if !anim.closing => anim.to = sheet,
                Some(_) => {}
                None => self.set_notes_clip(sheet),
            }
        }
    }

    /// The area the notepad webview covers while open: room for the expanded sheet, in the
    /// top-right corner of the page area.
    fn notes_desk(&self) -> Frame {
        let top = self.chrome_height() + NOTES_MARGIN;
        let sidebar = self.docked_width();
        let right = self.width - sidebar - NOTES_MARGIN;
        let width = NOTES_EXPANDED_MAX_WIDTH.min(right - NOTES_MARGIN).max(1.0);
        [
            right - width,
            top,
            width,
            (self.height - top - NOTES_MARGIN).max(1.0),
        ]
    }

    /// Resting place of the sheet within the desk: the top-right corner, or all of it when expanded.
    fn notes_sheet(&self) -> Frame {
        let [_, _, w, h] = self.notes_desk();
        if self.notes_expanded {
            [0.0, 0.0, w, h]
        } else {
            let width = NOTES_WIDTH.min(w);
            [w - width, 0.0, width, NOTES_MAX_HEIGHT.min(h)]
        }
    }

    fn set_notes_clip(&mut self, clip: Frame) {
        if let Some(n) = &self.notes {
            let scale = self.window.scale_factor();
            let px = clip.map(|v| (v * scale).round() as i32);
            webview::set_region(n, px[0], px[1], px[0] + px[2].max(1), px[1] + px[3].max(1));
        }
        self.notes_clip = Some(clip);
    }

    fn animate_notes(&mut self, from: Frame, to: Frame, ms: f32, closing: bool) {
        self.notes_anim = Some(NotesAnim {
            start: Instant::now(),
            duration: Duration::from_secs_f32(ms / 1000.0),
            from,
            to,
            closing,
        });
    }

    /// Advance the notepad animation one frame. Returns true while it is still running.
    fn step_notes(&mut self) -> bool {
        let Some(anim) = &self.notes_anim else {
            return false;
        };
        let t = (anim.start.elapsed().as_secs_f32() / anim.duration.as_secs_f32()).min(1.0);
        // Ease out when unrolling or resizing, ease in when rolling up.
        let e = if anim.closing {
            t * t * t
        } else {
            1.0 - (1.0 - t).powi(3)
        } as f64;
        let mut f = [0.0; 4];
        for (i, v) in f.iter_mut().enumerate() {
            *v = anim.from[i] + (anim.to[i] - anim.from[i]) * e;
        }
        let (done, closing) = (t >= 1.0, anim.closing);
        self.set_notes_clip(f);
        if done {
            self.notes_anim = None;
            if closing {
                if let Some(n) = &self.notes {
                    let _ = n.set_visible(false);
                }
                self.notes_clip = None;
            }
        }
        !done
    }

    /// Floating panels stay above the page views (which are re-stacked as tabs are created).
    fn raise_overlays(&self) {
        if let (true, Some(chrome)) = (self.tray_rects.iter().any(Option::is_some), &self.chrome) {
            webview::bring_to_top(chrome);
        }
        if let (true, Some(notes)) = (self.notes_open, &self.notes) {
            webview::bring_to_top(notes);
        }
        if let (true, Some(sidebar)) = (self.sidebar_open, &self.sidebar) {
            webview::bring_to_top(sidebar);
        }
        if let (true, Some(overlay)) = (self.overlay_open, &self.overlay) {
            webview::bring_to_top(overlay);
        }
    }

    /// Hide the dropdown once its closing animation has played. Returns the time it is next due.
    fn step_popover(&mut self) -> Option<Instant> {
        let due = self.pop_hide_at?;
        if Instant::now() < due {
            return Some(due);
        }
        self.pop_hide_at = None;
        if !self.sidebar_open {
            if let Some(s) = &self.sidebar {
                let _ = s.set_visible(false);
            }
        }
        None
    }

    /// Tell the title bar whether the window is maximized / focused (only when it changed).
    fn sync_frame(&mut self) {
        let state = (self.window.is_maximized(), self.window.has_focus());
        if self.frame_state != Some(state) {
            self.frame_state = Some(state);
            self.eval_chrome(&format!(
                "window.__syncWindow && window.__syncWindow({{maximized:{},focused:{}}})",
                state.0, state.1
            ));
        }
    }

    fn eval_chrome(&self, js: &str) {
        if let Some(c) = &self.chrome {
            let _ = c.evaluate_script(js);
        }
    }

    fn eval_sidebar(&self, js: &str) {
        if let Some(s) = &self.sidebar {
            let _ = s.evaluate_script(js);
        }
    }

    /// Run a script in the window's internal pages (new tab, settings, welcome).
    fn eval_internal_tabs(&self, js: &str) {
        for t in self.tabs.all_tabs() {
            if is_internal(&t.url) {
                if let Some(tab) = self.views.get(&t.id) {
                    let _ = tab.view.evaluate_script(js);
                }
            }
        }
    }

    /// Finish a switch to a tab that was still blank: show it and hide what it replaces. Returns
    /// when to look again if it is still waiting.
    fn settle_veil(&mut self) -> Option<Instant> {
        let id = self.veil?;
        match self.views.get(&id) {
            Some(t) if t.settled() => {
                self.veil = None;
                if self.tabs.active_tab_id() == Some(id) {
                    self.show_tab(id);
                }
                None
            }
            Some(t) => Some(t.born + VEIL_MAX + Duration::from_millis(5)),
            None => {
                self.veil = None;
                None
            }
        }
    }

    fn hide(tab: &mut Tab) {
        let _ = tab.view.set_visible(false);
        let _ = tab.view.set_memory_usage_level(wry::MemoryUsageLevel::Low);
        tab.hidden_since = Some(Instant::now());
    }

    /// Bring `id` to the front, demoting whichever tab was visible.
    fn show_tab(&mut self, id: TabId) {
        let previous = self.tabs.active_tab_id();
        let prev_split = self.tabs.split_panes();
        if !self.tabs.switch_tab(id, now_secs()) {
            return;
        }
        // Split view keeps both panes up; anything else that was showing goes away.
        let pair = self.tabs.split_panes();
        let keep = |t: TabId| t == id || pair.as_ref().is_some_and(|p| p.contains(&t));
        // A tab that has not painted yet is shown (and the old one hidden) once it has.
        self.veil = self
            .views
            .get(&id)
            .is_some_and(|t| !t.settled())
            .then_some(id);
        let mut showing: Vec<TabId> = previous.into_iter().collect();
        showing.extend(prev_split.into_iter().flatten());
        showing.extend(pair.iter().flatten().copied());
        let veiled = self.veil.is_some();
        for (&tid, tab) in self.views.iter_mut() {
            if !veiled && !keep(tid) && (showing.contains(&tid) || tab.hidden_since.is_none()) {
                Self::hide(tab);
            }
        }
        let mut panes = vec![id];
        panes.extend(pair.iter().flatten().copied().filter(|t| *t != id));
        for t in panes {
            let rect = self.rect_for(t);
            if let Some(tab) = self.views.get_mut(&t) {
                tab.load_pending();
                let _ = tab.view.set_bounds(rect);
                let _ = tab.view.set_visible(tab.settled());
                webview::wake(&tab.view);
                if t == id {
                    // A new tab page hands the keyboard to the address bar, so typing goes there
                    // (and gets the search engine's suggestions) rather than into the page.
                    let newtab = self
                        .tabs
                        .get_tab_by_id(t)
                        .is_some_and(|s| is_new_tab_page(&s.url));
                    match (&self.chrome, newtab) {
                        (Some(c), true) => {
                            webview::focus(c);
                            let _ = c.evaluate_script(
                                "window.__focusOmnibox && window.__focusOmnibox()",
                            );
                        }
                        _ => webview::focus(&tab.view),
                    }
                }
                tab.hidden_since = None;
                tab.suspended = false;
            }
        }
        self.ui_dirty = true;
        self.sidebar_dirty = true;
        self.sync_zoom();
        self.raise_overlays();
    }

    fn zoom(&self) -> f64 {
        let mut factor = 1.0;
        if let Some(tab) = self.active() {
            let _ = unsafe { tab.view.controller().ZoomFactor(&mut factor) };
        }
        factor
    }

    fn set_zoom(&self, factor: f64) {
        let factor = ((factor * 100.0).round() / 100.0).clamp(0.25, 3.0);
        if let Some(tab) = self.active() {
            let _ = unsafe { tab.view.controller().SetZoomFactor(factor) };
        }
        self.sync_zoom();
    }

    fn sync_zoom(&self) {
        let js = format!("window.__syncZoom && window.__syncZoom({})", self.zoom());
        self.eval_chrome(&js);
        self.eval_sidebar(&js);
    }

    fn find(&self, query: &str, forward: bool) {
        let Some(tab) = self.active() else { return };
        let script = format!(
            r#"(() => {{
  const q = {q};
  if (!q) return;
  const found = window.find(q, false, {back}, true, false, false, false);
  const text = document.body ? document.body.innerText : '';
  const total = text.toLowerCase().split(q.toLowerCase()).length - 1;
  window.ipc && window.ipc.postMessage(JSON.stringify({{ action: 'FindResult', payload: {{ current: found && total ? 1 : 0, total }} }}));
}})()"#,
            q = js_string(query),
            back = !forward
        );
        let _ = tab.view.evaluate_script(&script);
    }
}

/* -------------------------------------------------------------------- app */

struct BrowserApp {
    windows: HashMap<WindowId, BrowserWindow>,
    proxy: EventLoopProxy<BrowserEvent>,
    settings: Settings,
    settings_path: PathBuf,
    data_dir: PathBuf,
    pages: SharedPages,
    cert_cache: cert::CertificateCache,
    allowed_cert_hosts: Arc<Mutex<HashSet<String>>>,
    /// Tab ids for every window (see `TabManager::with_shared_ids`)
    tab_ids: Arc<std::sync::atomic::AtomicU64>,
    modifiers: ModifiersState,
    last_engine_restart: Option<Instant>,
    notebook: shoji_core::notes::Notebook,
    shield: shoji_byobu::Shield,
    favourites: shoji_core::userdata::Favourites,
    history: shoji_core::userdata::History,
    /// Files downloaded, newest first
    downloads: shoji_core::userdata::DownloadLog,
    /// Paper cranes folded so far (one per closed tab and per saved file)
    cranes: shoji_core::userdata::Cranes,
    /// When you met, when Shoji was last open, which hidden things were found
    keepsakes: shoji_core::userdata::Keepsakes,
    /// Reload presses in quick succession, for the crane's remark
    reloads: Vec<Instant>,
    /// Told the user about their pile of tabs this session
    hoarding_told: bool,
    /// Redirect hops and visits, for bounce-tracking cleanup
    bounce: shoji_core::bounce::BounceLog,
    /// When hop-only sites are next checked for cleaning
    next_bounce_clean: Instant,
    /// A tab being dragged out of a strip (its window follows the cursor)
    tab_drag: Option<tabdrag::TabDrag>,
    /// A finished, hidden window kept ready so a dragged-out tab gets its window at once (building
    /// one takes most of a second). It is not in `windows` until a tab is torn out into it.
    spare: Option<(WindowId, BrowserWindow)>,
    /// When the next spare may be built (never while the user is mid-gesture)
    spare_due: Instant,
    /// A spare window is being built (see `create_window`)
    spare_building: bool,
    /// Engine download id -> key in `downloads` while it is running
    dl_keys: HashMap<u64, u64>,
    /// The history page should open with its clear-data card showing
    clear_pending: bool,
    /// When the history and download lists were last written to disk
    stores_flushed: Instant,
    vault: shoji_core::vault::Vault,
    /// A login the user just submitted on a page, waiting for their answer to "save it?"
    pending_login: Option<PendingLogin>,
    /// Cookies read by an import, waiting for a tab that uses the real profile (internal pages
    /// run in a throwaway one)
    pending_cookies: Mutex<Vec<shoji_core::import::Cookie>>,
    site_data: shoji_core::userdata::SiteData,
    session_path: PathBuf,
    /// Instance window and tray icon (None if it could not be created)
    tray: Option<tray::Tray>,
    /// Shoji's own updates: what was found, what the user agreed to, how far it has got
    update: update::Status,
    /// The user's answers to the letter (Later, Skip), kept between runs
    update_memo: update::Memo,
    update_memo_path: PathBuf,
    /// Requests to the update thread (None until it is started)
    update_tx: Option<std::sync::mpsc::Sender<update::Request>>,
    /// The letter should open by itself once a window has focus
    update_unfold_pending: bool,
    /// The installer has been started in update mode
    update_started: bool,
    /// The version whose files were already put in place beside this running copy
    update_staged: Option<String>,
    /// Tabs from the previous run, waiting for the user's answer (or restored at startup)
    pending_session: Option<shoji_core::userdata::Session>,
    restore_on_start: bool,
    /// Last session JSON written, and when (writes are throttled)
    session_json: String,
    session_written: Instant,
    /// When the session was last compared with what is on disk (building it is not free)
    session_checked: Instant,
    /// Something that the session records may have changed since it was last compared
    session_dirty: bool,
    /// An app event arrived since the last flush, so page-derived toolbar state may be stale
    /// (mouse moves alone never change it)
    sync_pending: bool,
    /// permission_id -> (host, kind) for decisions the user may want remembered
    perm_pending: HashMap<u64, (String, &'static str)>,
    /// Hosts the user chose to open over plain HTTP
    insecure_hosts: Arc<Mutex<HashSet<String>>>,
    /// Hosts recently upgraded to https:// (so a failed connect can offer the HTTP fallback)
    upgraded: HashMap<String, Instant>,
    /// Per tab: the last address upgraded, to break upgrade/redirect loops
    last_upgrade: HashMap<TabId, (String, Instant)>,
}

impl BrowserApp {
    fn new(proxy: EventLoopProxy<BrowserEvent>, data_dir: PathBuf, engine_version: String) -> Self {
        let settings_path = data_dir.join("settings.json");
        let settings = Settings::load_from_path(&settings_path);
        {
            let (frame, page) = pages::browser_look(&settings).frame_and_page();
            window::set_dark_colors(frame, page);
        }
        webview::set_download_prefs(
            settings.downloads.ask_where_to_save,
            settings.downloads.default_folder.clone(),
        );
        let favourites = shoji_core::userdata::Favourites::load(data_dir.join("favourites.json"));
        let site_data = shoji_core::userdata::SiteData::load(data_dir.join("sitedata.json"));
        let history = shoji_core::userdata::History::load(data_dir.join("history.json"));
        let downloads = shoji_core::userdata::DownloadLog::load(data_dir.join("downloads.json"));
        let cranes = shoji_core::userdata::Cranes::load(data_dir.join("cranes.json"));
        let vault = shoji_core::vault::Vault::load(data_dir.join("vault.json"));

        // Tabs from the last run. Private mode keeps none; otherwise a crash or force-close
        // (clean_exit still false) is offered for restore, or restored outright if so chosen.
        let session_path = data_dir.join("session.json");
        let mode = settings.privacy.restore_session.clone();
        let private = settings.privacy.ephemeral_default;
        let previous = if private || mode == "never" {
            shoji_core::userdata::Session::default()
        } else {
            shoji_core::userdata::Session::load(&session_path)
        };
        if private || mode == "never" {
            let _ = std::fs::remove_file(&session_path);
        } else {
            // Mark "running": a crash from here on leaves clean_exit false, with the old tabs kept.
            let mut marker = previous.clone();
            marker.clean_exit = false;
            marker.save(&session_path);
        }
        // `--restore-session`: the installer restarting Shoji after "Restart and update" (the
        // tabs were saved on the way out and come straight back).
        let after_update = std::env::args().any(|a| a == "--restore-session");
        let offer = !private
            && mode != "never"
            && previous.has_pages()
            && (!previous.clean_exit || mode == "always" || after_update);
        let restore_on_start = offer && (mode == "always" || after_update);
        let update_memo_path = data_dir.join("update-letter.json");
        let update_memo = update::Memo::load(&update_memo_path);
        let restore_tabs = if offer && !restore_on_start {
            previous.tab_count()
        } else {
            0
        };
        let pending_session = offer.then_some(previous);

        // An install from before keepsakes were kept dates from its settings file.
        let met_fallback = std::fs::metadata(&settings_path)
            .and_then(|m| m.created().or_else(|_| m.modified()))
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let keepsakes = shoji_core::userdata::Keepsakes::load(
            data_dir.join("keepsakes.json"),
            met_fallback,
            now_ms(),
        );
        let away_days = away_days(keepsakes.previous_visit(), now_ms());

        let pages = Arc::new(RwLock::new(PageContext {
            settings: settings.clone(),
            engine_version,
            data_dir: data_dir.clone(),
            notices: HashMap::new(),
            favourites: favourites.items().to_vec(),
            recent: Vec::new(),
            restore_tabs,
            site_choices: site_data.count(),
            cranes: cranes.count(),
            met: keepsakes.met(),
            found: keepsakes.found().to_vec(),
            away_days,
            first_visit: keepsakes.previous_visit() == 0,
            greet_once: std::sync::atomic::AtomicBool::new(true),
        }));
        let shield = shoji_byobu::Shield::new();
        shield::init(proxy.clone());
        shield::apply_settings(&settings.shield);
        darkfixes::set_enabled(settings.dark_needed());
        shield::spawn_updater(shield.clone(), data_dir.join("byobu"));
        Self {
            windows: HashMap::new(),
            proxy,
            shield,
            settings,
            settings_path,
            data_dir: data_dir.clone(),
            pages,
            cert_cache: cert::CertificateCache::default(),
            allowed_cert_hosts: Arc::new(Mutex::new(HashSet::new())),
            tab_ids: Arc::new(std::sync::atomic::AtomicU64::new(1)),
            modifiers: ModifiersState::default(),
            last_engine_restart: None,
            notebook: shoji_core::notes::Notebook::load(data_dir.join("notes.json")),
            favourites,
            history,
            downloads,
            cranes,
            keepsakes,
            reloads: Vec::new(),
            hoarding_told: false,
            bounce: shoji_core::bounce::BounceLog::load(&data_dir.join("bounce.json")),
            next_bounce_clean: Instant::now() + Duration::from_secs(90),
            tab_drag: None,
            spare: None,
            spare_due: Instant::now() + Duration::from_secs(3),
            spare_building: false,
            dl_keys: HashMap::new(),
            clear_pending: false,
            stores_flushed: Instant::now(),
            vault,
            pending_login: None,
            pending_cookies: Mutex::new(Vec::new()),
            site_data,
            session_path,
            tray: None,
            update: update::Status::default(),
            update_memo,
            update_memo_path,
            update_tx: None,
            update_unfold_pending: false,
            update_started: false,
            update_staged: None,
            pending_session,
            restore_on_start,
            session_json: String::new(),
            session_written: Instant::now(),
            session_checked: Instant::now(),
            session_dirty: true,
            sync_pending: true,
            perm_pending: HashMap::new(),
            insecure_hosts: Arc::new(Mutex::new(HashSet::new())),
            upgraded: HashMap::new(),
            last_upgrade: HashMap::new(),
        }
    }

    fn owner(&self, tab_id: TabId) -> Option<WindowId> {
        self.windows
            .iter()
            .find(|(_, w)| w.views.contains_key(&tab_id))
            .map(|(id, _)| *id)
    }

    fn is_dark(&self, window: &Window) -> bool {
        window::is_dark(window, &self.settings.appearance.theme)
    }

    /* ----- windows ----- */

    fn create_window(&mut self, event_loop: &ActiveEventLoop, private: bool) -> Option<WindowId> {
        let mut attrs = window::attributes(self.settings.accent(), &self.settings.appearance.theme);
        if self.spare_building {
            // A page starts far sooner in a window that is shown, so the spare is shown
            // off-screen (no focus, no taskbar button) while it is built, then hidden again.
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs = attrs
                .with_visible(true)
                .with_active(false)
                .with_skip_taskbar(true)
                .with_position(winit::dpi::PhysicalPosition::new(-32000, -32000));
        }
        let window = Arc::new(event_loop.create_window(attrs).ok()?);
        window.set_min_inner_size(Some(LogicalSize::new(480.0, 320.0)));
        window::remove_caption(&window);
        let id = window.id();
        let dark = self.is_dark(&window);
        window::apply_frame(&window, dark);
        if self.settings.appearance.dark_websites.is_none() {
            // First run: darken websites if Windows is in dark mode.
            self.settings.appearance.dark_websites = Some(dark);
            self.save_settings();
        }
        let size = window.inner_size().to_logical::<f64>(window.scale_factor());

        // The frosted toolbar no longer relies on a see-through webview: windowed WebView2
        // paints a transparent background white, so the toolbar came out as a white strip. The
        // chrome page draws its own translucent layers over a solid colour instead.
        let frosted = false;
        window::apply_backdrop(&window, frosted, dark);
        let proxy = self.proxy.clone();
        let chrome = new_builder()
            .with_bounds(bounds(
                0.0,
                0.0,
                size.width,
                if self.settings.appearance.bookmarks_bar {
                    CHROME_HEIGHT + BAR_HEIGHT
                } else {
                    CHROME_HEIGHT
                },
            ))
            .with_html(pages::chrome(&self.settings, private))
            .with_custom_protocol("shoji".into(), |_, request| font_response(&request))
            .with_incognito(true)
            .with_transparent(frosted)
            .with_background_color(if frosted {
                (0, 0, 0, 0)
            } else {
                window::page_rgba(dark)
            })
            .with_theme(window::webview_theme(&self.settings.appearance.theme))
            .with_ipc_handler(move |req: wry::http::Request<String>| {
                if let Ok(msg) = serde_json::from_str(req.body()) {
                    let _ = proxy.send_event(BrowserEvent::WindowIpc(id, msg));
                }
            })
            .build_as_child(window.as_ref())
            .ok();
        if let Some(c) = &chrome {
            webview::attach_accelerators(c, Origin::Window(id), self.proxy.clone());
            webview::enable_drag_regions(c);
        }

        self.windows.insert(
            id,
            BrowserWindow {
                window,
                tabs: TabManager::with_shared_ids(self.tab_ids.clone()),
                views: HashMap::new(),
                chrome,
                sidebar: None,
                sidebar_open: false,
                sidebar_mode: "menu",
                sidebar_docked: false,
                pop_height: 0.0,
                pop_hide_at: None,
                pop_opened: None,
                pop_blurred: None,
                anchors: HashMap::new(),
                pop_hover: false,
                pop_span: None,
                pop_top: None,
                hover_out_since: None,
                pop_pointer_in: false,
                sidebar_warm: None,
                save_url: None,
                notes: None,
                notes_open: false,
                notes_expanded: false,
                notes_clip: None,
                notes_ready: false,
                sidebar_ready: false,
                veil: None,
                sidebar_pending: None,
                notes_anim: None,
                width: size.width,
                height: size.height,
                find_open: false,
                recorded: HashMap::new(),
                find_query: String::new(),
                fullscreen: false,
                permission_open: false,
                tray_rects: [None, None, None, None],
                tab_centers: Vec::new(),
                chrome_clipped: false,
                ui_dirty: true,
                sidebar_dirty: false,
                last_ui_json: String::new(),
                frame_state: None,
                last_title: String::new(),
                overlay: None,
                overlay_open: false,
                overlay_mode: "suggest",
                overlay_ready: false,
                focus_address: false,
                overlay_pending: None,
                overlay_shown: None,
                overlay_items: Vec::new(),
                prompt: None,
                fullscreen_tab: None,
                divider_dragging: false,
                divider_index: 0,
                divider_grab: 0.0,
                pending_split_layout: false,
                cursor_pos: (0.0, 0.0),
                last_divider_click: None,
                reader: HashSet::new(),
                last_page_sync: String::new(),
                last_pane_rects: String::new(),
                last_pane_state: String::new(),
                split_zones: Vec::new(),
                applied_panes: HashMap::new(),
                last_fav_sync: String::new(),
                private,
                bar: self.settings.appearance.bookmarks_bar,
                menu_at: (0.0, 0.0, 0.0),
            },
        );
        if private {
            if let Some(w) = self.windows.get(&id) {
                w.window.set_title("Private — Shoji");
            }
        }
        Some(id)
    }

    fn close_window(&mut self, id: WindowId, event_loop: &ActiveEventLoop) {
        if !self.windows.contains_key(&id) {
            return; // the hidden spare, or a window that is already gone
        }
        // Closing the last window is a normal quit: keep its tabs and mark the exit clean.
        if self.windows.len() == 1 && self.windows.contains_key(&id) {
            self.write_session(true);
        }
        self.windows.remove(&id);
        self.session_dirty = true;
        if self.windows.is_empty() {
            self.spare = None;
            self.keepsakes.touch(now_ms());
            // No window is open, so a downloaded update can install without interrupting anyone.
            self.apply_update();
            // Keep running in the notification area so Byōbu keeps updating its lists. Private
            // mode always quits: its in-memory data must not outlive the window.
            let keep = self.settings.performance.run_in_background
                && !self.settings.privacy.ephemeral_default;
            match (&self.tray, keep) {
                (Some(tray), true) => tray.show(),
                _ => event_loop.exit(),
            }
        }
    }

    /// Bring Shoji back to the front: show a window (restoring the closed session when that
    /// is the user's choice) and open `url` in it if one was given.
    fn reopen(&mut self, event_loop: &ActiveEventLoop, url: Option<String>) {
        if self.windows.is_empty() {
            let before = self.keepsakes.visit(now_ms());
            if let Ok(ctx) = self.pages.write() {
                let mut ctx = ctx;
                ctx.away_days = away_days(before, now_ms());
                ctx.first_visit = false;
                ctx.greet_once
                    .store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        let url = url.and_then(|a| {
            normalize_url(&a, self.settings.search_url_template())
                .ok()
                .filter(|_| self.settings.first_run_complete)
        });
        if let Some(id) = self.windows.keys().next().copied() {
            if let Some(w) = self.windows.get(&id) {
                w.window.set_minimized(false);
                w.window.set_visible(true);
                w.window.focus_window();
            }
            if url.is_some() {
                self.create_tab(id, url);
            }
            return;
        }
        let restore = self.session_enabled() && self.settings.privacy.restore_session == "always";
        if restore {
            let previous = shoji_core::userdata::Session::load(&self.session_path);
            if previous.has_pages() {
                self.pending_session = Some(previous);
            }
        }
        let restoring = restore && self.pending_session.is_some();
        self.new_window(event_loop, if restoring { None } else { url.clone() });
        if let Some(id) = self.windows.keys().next().copied() {
            if restoring {
                self.restore_session(id, event_loop);
                if url.is_some() {
                    self.create_tab(id, url);
                }
            }
            if let Some(w) = self.windows.get(&id) {
                w.window.focus_window();
            }
        }
    }

    /* ----- Shoji's own updates ----- */

    /// Tell every window's letter, and any open Settings page, where the update is at.
    fn push_update(&self) {
        let state = self
            .update
            .json(&self.update_memo, update::running_installed_copy());
        let js = format!("window.__updateState && window.__updateState({state})");
        for win in self.windows.values() {
            win.eval_chrome(&js);
            win.eval_internal_tabs(&js);
        }
    }

    fn update_request(&self, request: update::Request) {
        if let Some(tx) = &self.update_tx {
            let _ = tx.send(request);
        }
    }

    /// Let the letter open by itself in the focused window, unless the user said Later, skipped
    /// this version or saw it already today. Never over full screen and never during setup;
    /// without a focused window it waits for one. It never takes focus.
    fn maybe_unfold_letter(&mut self) {
        if !self.update_unfold_pending {
            return;
        }
        let Some(version) = self.update.release.as_ref().map(|r| r.version.clone()) else {
            self.update_unfold_pending = false;
            return;
        };
        let now = now_ms();
        if !self.update_memo.may_unfold(&version, now)
            || !self.settings.first_run_complete
            || self.update.consent != update::Consent::None
        {
            self.update_unfold_pending = false;
            return;
        }
        let Some(win) = self
            .windows
            .values()
            .find(|w| w.window.has_focus() && w.window.fullscreen().is_none())
        else {
            return;
        };
        win.eval_chrome("window.__unfoldLetter && window.__unfoldLetter()");
        self.update_unfold_pending = false;
        self.update_memo.unfolded = version;
        self.update_memo.unfolded_at = now;
        self.update_memo.save(&self.update_memo_path);
    }

    /// The letter's buttons, and the updates row in Settings.
    fn update_choice(&mut self, win_id: WindowId, choice: &str) {
        use update::{Consent, Phase};
        match choice {
            "check" => {
                if self.update.phase == Phase::Idle {
                    self.update.phase = Phase::Checking;
                }
                self.update.manual = true;
                self.update.error = None;
                self.update_request(update::Request::Check { manual: true });
            }
            "now" | "close" => {
                let Some(release) = self.update.release.clone() else {
                    return self.push_update();
                };
                self.update.error = None;
                if !update::running_installed_copy() {
                    self.update.error = Some(
                        "This copy of Shoji isn't the installed one, so it can't replace itself. \
                         The release page has the new installer."
                            .into(),
                    );
                    return self.push_update();
                }
                if self.update_memo.skip == release.version {
                    self.update_memo.skip.clear();
                }
                self.update.consent = if choice == "now" {
                    Consent::Now
                } else {
                    Consent::OnClose
                };
                self.update_memo.installing = Some(release.clone());
                self.update_memo.save(&self.update_memo_path);
                match self.update.phase {
                    Phase::Ready => self.proceed_update(),
                    Phase::Downloading => {}
                    _ => {
                        self.update.phase = Phase::Downloading;
                        self.update.pct = 0;
                        self.update_request(update::Request::Download(release));
                    }
                }
            }
            "later" => {
                self.update_memo.later_until = now_ms() + update::LATER_MS;
                self.update_memo.save(&self.update_memo_path);
            }
            "skip" => {
                if let Some(r) = &self.update.release {
                    self.update_memo.skip = r.version.clone();
                    self.update_memo.save(&self.update_memo_path);
                }
            }
            "unskip" => {
                self.update_memo.skip.clear();
                self.update_memo.save(&self.update_memo_path);
            }
            "notes" => {
                let page = self
                    .update
                    .release
                    .as_ref()
                    .map(|r| r.page_url.clone())
                    .filter(|u| !u.is_empty());
                if let Some(url) = page {
                    self.create_tab(win_id, Some(url));
                }
            }
            _ => {}
        }
        self.push_update();
    }

    /// The installer is ready and the user agreed: restart now, or stage it for when Shoji closes.
    fn proceed_update(&mut self) {
        match self.update.consent {
            update::Consent::Now => self.restart_into_update(),
            update::Consent::OnClose => {
                self.stage_update();
                self.apply_update();
            }
            update::Consent::None => {}
        }
    }

    /// "When I close Shoji": put the new files in place beside the running copy straight away,
    /// so the next start is already the new version. Nothing is closed and nothing is shown.
    fn stage_update(&mut self) {
        let (Some(release), Some(installer)) = (&self.update.release, &self.update.installer)
        else {
            return;
        };
        if self.update_staged.as_ref() == Some(&release.version)
            || self.update.consent != update::Consent::OnClose
            || !update::running_installed_copy()
            || !installer.exists()
        {
            return;
        }
        let mut cmd = std::process::Command::new(installer);
        cmd.arg("--stage");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0008); // no window, outlives this copy
        }
        if cmd.spawn().is_ok() {
            self.update_staged = Some(release.version.clone());
        }
    }

    /// Run an agreed installer in update mode once no window is open (the browser is in the
    /// tray, or about to exit): it asks this copy to quit, replaces the files and starts the new
    /// version in the tray.
    fn apply_update(&mut self) {
        if self.update_started
            || !self.windows.is_empty()
            || self.update.consent == update::Consent::None
            || !update::running_installed_copy()
        {
            return;
        }
        let Some(installer) = self.update.installer.clone().filter(|p| p.exists()) else {
            return;
        };
        let mut cmd = std::process::Command::new(installer);
        cmd.arg("--update");
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0000_0008); // DETACHED_PROCESS: outlives this copy
        }
        if cmd.spawn().is_ok() {
            self.update_started = true;
        }
    }

    /// "Restart and update": save the open tabs, then let the installer quit this copy, swap the
    /// files and start the new version with the same tabs (`--reopen`).
    fn restart_into_update(&mut self) {
        if self.update_started || !update::running_installed_copy() {
            return;
        }
        let Some(installer) = self.update.installer.clone().filter(|p| p.exists()) else {
            return;
        };
        self.write_session(true);
        let mut cmd = std::process::Command::new(installer);
        cmd.args(["--update", "--reopen"]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0000_0008); // DETACHED_PROCESS: outlives this copy
        }
        if cmd.spawn().is_ok() {
            self.update_started = true;
        } else {
            self.update.consent = update::Consent::None;
            self.update.error = Some("Couldn't start the installer. Try again in a moment.".into());
        }
    }

    /// The first window after an update: the crane says, once, what the new version brought.
    fn greet_after_update(&mut self) {
        let Some(release) = self.update_memo.installing.take() else {
            return;
        };
        self.update_memo.save(&self.update_memo_path);
        if release.version != env!("CARGO_PKG_VERSION") {
            return; // the update didn't happen (or was older); nothing to say
        }
        let text = if release.notes.is_empty() {
            format!("This is Shoji {}.", release.version)
        } else {
            release.notes.join(" · ")
        };
        let mut actions = Vec::new();
        if !release.page_url.is_empty() {
            actions.push(serde_json::json!({
                "label": "Read the letter",
                "action": "CreateTab",
                "payload": {"url": release.page_url},
                "primary": true,
            }));
        }
        let toast = serde_json::json!({
            "title": format!("Folded anew · Shoji {}", release.version),
            "text": text,
            "pose": "unfold",
            "ms": 9000,
            "actions": actions,
        });
        self.toast(&toast);
    }

    /* ----- password manager ----- */

    /// Make the vault usable: open an OS-protected one, or (when `create`) start one. A vault
    /// with a master password only opens when the user types it.
    fn vault_open(&mut self, create: bool) -> bool {
        use shoji_core::vault::State;
        match self.vault.state() {
            State::Unlocked => true,
            State::Locked => {
                !self.vault.has_master() && self.vault.unlock_auto(&vault_win::WinProtector)
            }
            State::Empty => create && self.vault.create(&vault_win::WinProtector),
        }
    }

    fn eval_all_internal(&self, js: &str) {
        for win in self.windows.values() {
            win.eval_internal_tabs(js);
        }
    }

    fn vault_state_json(&mut self) -> serde_json::Value {
        use shoji_core::vault::State;
        // An OS-protected vault opens on its own, so the page never shows it as locked.
        if self.vault.state() == State::Locked {
            self.vault_open(false);
        }
        let state = match self.vault.state() {
            State::Empty => "empty",
            State::Locked => "locked",
            State::Unlocked => "unlocked",
        };
        serde_json::json!({ "state": state, "master": self.vault.has_master(), "count": self.vault.len() })
    }

    fn push_vault(&mut self) {
        let js = format!(
            "window.__vaultState && window.__vaultState({})",
            self.vault_state_json()
        );
        self.eval_all_internal(&js);
    }

    fn vault_toast(&self, text: &str) {
        self.eval_all_internal(&format!(
            "window.__vaultToast && window.__vaultToast({})",
            js_string(text)
        ));
    }

    fn push_vault_list(&mut self, query: &str) {
        if !self.vault.is_unlocked() {
            return;
        }
        let audit = self.vault.audit();
        let weak: HashSet<&String> = audit.weak.iter().collect();
        let reused: HashSet<&String> = audit.reused.iter().flatten().collect();
        let items: Vec<serde_json::Value> = self
            .vault
            .search(query)
            .iter()
            .map(|l| {
                serde_json::json!({
                    "id": l.id, "title": l.title, "url": l.url, "host": l.host(),
                    "username": l.username, "modified": l.modified,
                    "weak": weak.contains(&l.id), "reused": reused.contains(&l.id),
                })
            })
            .collect();
        let payload = serde_json::json!({
            "items": items, "weak": audit.weak.len(), "reused": audit.reused.iter().map(Vec::len).sum::<usize>(),
        });
        self.eval_all_internal(&format!(
            "window.__vaultList && window.__vaultList({payload})"
        ));
    }

    fn vault_changed(&mut self) {
        self.push_vault();
        self.push_vault_list("");
    }

    /// Whether the saved-login prompts and fill menu are on for `win_id`'s active page, and the
    /// page's address.
    fn login_page(&self, win_id: WindowId, tab: TabId) -> Option<String> {
        if !self.settings.privacy.save_passwords || self.settings.privacy.ephemeral_default {
            return None;
        }
        let url = self
            .windows
            .get(&win_id)?
            .tabs
            .get_tab_by_id(tab)?
            .url
            .clone();
        (url.starts_with("https://") || url.starts_with("http://")).then_some(url)
    }

    /// Ask whether to keep the login just submitted.
    fn offer_save(&mut self, win_id: WindowId) {
        use shoji_core::ipc::OverlayItem;
        let Some(p) = &self.pending_login else {
            return;
        };
        let host = shoji_core::vault::host_of(&p.url);
        let who = if p.username.is_empty() {
            String::new()
        } else {
            format!(" for {}", p.username)
        };
        let (label, value) = if p.existing.is_some() {
            (format!("Update the saved password{who}"), "save")
        } else {
            (format!("Save password{who}"), "save")
        };
        let item = |kind: &str, value: &str, label: String, sub: String| OverlayItem {
            kind: kind.into(),
            value: value.into(),
            label,
            sub,
            color: None,
        };
        let items = vec![
            item("login-save", value, label, host.clone()),
            item(
                "login-save",
                "never",
                format!("Never for {host}"),
                String::new(),
            ),
            item("login-save", "dismiss", "Not now".into(), String::new()),
        ];
        let Some(win) = self.windows.get(&win_id) else {
            return;
        };
        let w = 320.0f64.min(win.width - 16.0);
        let frame = [
            win.width - w - 12.0,
            win.chrome_height(),
            w,
            46.0 + 2.0 * 34.0 + 12.0,
        ];
        self.show_overlay(win_id, "menu", items, frame, "", "", "");
    }

    fn commit_login(&mut self, choice: &str) {
        let Some(p) = self.pending_login.take() else {
            return;
        };
        if p.at.elapsed() > Duration::from_secs(600) {
            return;
        }
        let host = shoji_core::vault::host_of(&p.url);
        match choice {
            "save" => {
                if self.vault_open(true) {
                    let draft = shoji_core::vault::Draft {
                        url: p.url.clone(),
                        title: host,
                        username: p.username,
                        password: p.password,
                        note: String::new(),
                    };
                    self.vault.upsert(&draft, now_ms());
                    self.vault_changed();
                }
            }
            "never" if self.vault_open(true) => {
                self.vault.never_save(&host);
            }
            _ => {}
        }
    }

    /// Fill the active page's login form with a saved login, if it is for this page's site.
    fn fill_login(&mut self, win_id: WindowId, id: &str) {
        let Some(login) = self.vault.get(id).cloned() else {
            return;
        };
        let Some(win) = self.windows.get(&win_id) else {
            return;
        };
        let Some(tab) = win.active() else {
            return;
        };
        let page_host = win
            .tabs
            .active_tab()
            .map(|t| shoji_core::vault::host_of(&t.url))
            .unwrap_or_default();
        if !shoji_core::vault::host_matches(&login.host(), &page_host) {
            return;
        }
        let js = format!(
            "window.__shojiFill && window.__shojiFill({}, {})",
            js_string(&login.username),
            js_string(&login.password)
        );
        let _ = tab.view.evaluate_script(&js);
        self.vault.mark_used(id, now_ms());
    }

    /// Lock an idle master-password vault.
    fn vault_tick(&mut self) -> Option<Instant> {
        if self.vault.lock_if_idle(VAULT_IDLE) {
            self.push_vault();
        }
        self.vault.idle_deadline(VAULT_IDLE)
    }

    /// Run a script in the welcome and settings pages of a window.
    /// A webview that uses the real profile, which is where extensions live.
    fn persistent_view(&self) -> Option<&wry::WebView> {
        self.windows
            .values()
            .flat_map(|w| w.views.values())
            .find(|t| t.persistent)
            .map(|t| &t.view)
    }

    fn request_extension_list(&self) {
        let view = self
            .persistent_view()
            .filter(|_| EXTENSIONS_ON.load(std::sync::atomic::Ordering::Relaxed));
        match view {
            Some(view) => webview::list_extensions(view, self.proxy.clone()),
            None => {
                let _ = self
                    .proxy
                    .send_event(BrowserEvent::ExtensionsListed("[]".into()));
            }
        }
    }

    /// Download an extension from the Chrome Web Store (by link or id) and load it.
    fn install_extension_from_store(&self, source: &str) {
        let say = |app: &Self, title: &str, text: &str| {
            app.toast(&serde_json::json!({ "title": title, "text": text, "pose": "unfold" }));
        };
        if !EXTENSIONS_ON.load(std::sync::atomic::Ordering::Relaxed) {
            say(
                self,
                "Turn extensions on first.",
                "Switch them on in Settings and restart Shoji.",
            );
            return;
        }
        let Some(id) = extstore::parse_id(source) else {
            say(
                self,
                "That isn't an extension link.",
                "Paste the page's address from the Chrome Web Store, or the 32-letter id.",
            );
            return;
        };
        say(
            self,
            "Downloading the extension…",
            "It is added in a moment.",
        );
        let proxy = self.proxy.clone();
        let dir = self.data_dir.join("Extensions");
        let _ = std::thread::Builder::new()
            .name("shoji-extension".into())
            .spawn(move || {
                let _ =
                    proxy.send_event(BrowserEvent::ExtensionFetched(extstore::fetch(&id, &dir)));
            });
    }

    /// Pick an unpacked extension, copy it into the data folder (so moving the original can't
    /// break it) and hand it to the engine.
    fn add_extension_from_folder(&self) {
        let say = |app: &Self, title: &str, text: &str| {
            app.toast(&serde_json::json!({ "title": title, "text": text, "pose": "unfold" }));
        };
        if !EXTENSIONS_ON.load(std::sync::atomic::Ordering::Relaxed) {
            say(
                self,
                "Turn extensions on first.",
                "Switch them on in Settings and restart Shoji.",
            );
            return;
        }
        let Some(view) = self.persistent_view() else {
            say(
                self,
                "Open a website first.",
                "Extensions are added to the browsing profile.",
            );
            return;
        };
        let start = webview::documents_dir().unwrap_or_else(|| self.data_dir.clone());
        let Some(picked) = webview::pick_folder(&start) else {
            return;
        };
        if !picked.join("manifest.json").is_file() {
            say(
                self,
                "Not an extension folder.",
                "It needs a manifest.json at the top. Unzip the extension first.",
            );
            return;
        }
        let extensions_dir = self.data_dir.join("Extensions");
        // A folder already inside our own copies is loaded in place; copying it onto itself
        // would delete it first.
        let dest = if picked
            .canonicalize()
            .ok()
            .zip(extensions_dir.canonicalize().ok())
            .is_some_and(|(p, e)| p.starts_with(e))
        {
            picked
        } else {
            // Named after the folder plus a hash of where it came from, so two extensions that
            // are both unzipped to "dist" don't overwrite each other.
            let stem: String = picked
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "extension".into())
                .chars()
                .map(|c| {
                    if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                        c
                    } else {
                        '_'
                    }
                })
                .collect();
            let hash = picked
                .to_string_lossy()
                .to_lowercase()
                .bytes()
                .fold(0xcbf29ce484222325u64, |h, b| {
                    (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
                });
            let dest = extensions_dir.join(format!("{stem}-{:08x}", hash as u32));
            if copy_dir_all(&picked, &dest, 0).is_err() {
                say(
                    self,
                    "Couldn't copy the extension.",
                    "Check that the folder can be read.",
                );
                return;
            }
            dest
        };
        webview::add_extension(view, &dest, self.proxy.clone());
    }

    fn eval_welcome_and_settings(&self, win_id: WindowId, js: &str) {
        if let Some(win) = self.windows.get(&win_id) {
            win.eval_internal_tabs(js);
        }
    }

    /// Hand cookies waiting from an import to `view`, which must use the real profile.
    fn apply_pending_cookies(&self, view: &wry::WebView) -> usize {
        let cookies = self
            .pending_cookies
            .lock()
            .map(|mut c| std::mem::take(&mut *c))
            .unwrap_or_default();
        if cookies.is_empty() {
            0
        } else {
            webview::add_cookies(view, &cookies)
        }
    }

    /// Save the session and leave the event loop (tray menu, or the installer's `--quit`).
    fn quit(&mut self, event_loop: &ActiveEventLoop) {
        self.history.flush();
        if !self.windows.is_empty() {
            self.write_session(true);
        }
        self.windows.clear();
        self.spare = None;
        event_loop.exit();
    }

    /* ----- tabs ----- */

    /// `navigate` is false for a popup: the page that opened it hands the view its address.
    #[allow(clippy::too_many_arguments)]
    fn build_view(
        &self,
        window: &Window,
        tab_id: TabId,
        url: &str,
        rect: Rect,
        navigate: bool,
        container: Option<String>,
    ) -> Option<Tab> {
        let internal = is_internal(url);
        let private = self
            .windows
            .values()
            .any(|w| w.private && w.window.id() == window.id());
        // A container keeps its own cookies (outside private windows), for every page in the tab.
        let persistent = !private
            && if container.is_some() {
                true
            } else {
                !internal && self.settings.is_site_persistent(url)
            };
        let dark = self.is_dark(window);

        let pages = self.pages.clone();
        let nav_pages = self.pages.clone();
        let nav_insecure = self.insecure_hosts.clone();
        let proxy_nav = self.proxy.clone();
        let proxy_ipc = self.proxy.clone();
        let nav_container = container.clone();
        let mut builder = new_builder()
            .with_bounds(rect)
            .with_incognito(!persistent)
            .with_background_color(window::page_rgba(dark))
            .with_theme(window::webview_theme(&self.settings.appearance.theme))
            .with_devtools(true)
            .with_custom_protocol("shoji".into(), move |_, request| {
                let host = request.uri().host().unwrap_or_default();
                let page = host.strip_prefix("shoji.").unwrap_or(host).to_string();
                if page == "fonts" {
                    return font_response(&request);
                }
                let html = pages.read().ok().and_then(|ctx| internal_page(&ctx, &page));
                match html {
                    Some(html) => wry::http::Response::builder()
                        .header("Content-Type", "text/html; charset=utf-8")
                        .header("Cache-Control", "no-store")
                        .header("Content-Security-Policy", INTERNAL_CSP)
                        .body(Cow::Owned(html.into_bytes())),
                    None => wry::http::Response::builder()
                        .status(404)
                        .body(Cow::Borrowed(&b"Not found"[..])),
                }
                .unwrap_or_else(|_| wry::http::Response::new(Cow::Borrowed(&[][..])))
            })
            .with_navigation_handler(move |nav: String| {
                // A tab in the in-memory partition can't hold a kept site's cookies:
                // hand the navigation to the host, which rebuilds the view.
                let wants_persistent = !private
                    && nav.starts_with("http")
                    && !nav.starts_with(INTERNAL_ORIGIN)
                    && nav_pages
                        .read()
                        .map(|c| c.settings.is_site_persistent(&nav))
                        .unwrap_or(false);
                // A site that always opens in a container goes there from tabs outside containers.
                // A tab the user put in a container keeps whatever it opens.
                let ruled = nav_container.is_none() && !nav.starts_with(INTERNAL_ORIGIN);
                if let Some(host) = extract_host(&nav).filter(|_| ruled) {
                    let rule = nav_pages
                        .read()
                        .ok()
                        .and_then(|c| c.settings.containers.for_host(&host).map(str::to_owned));
                    if rule.is_some() && rule != nav_container {
                        let _ = proxy_nav.send_event(BrowserEvent::OpenInContainer {
                            tab_id,
                            url: nav,
                            container: rule,
                        });
                        return false;
                    }
                }
                if wants_persistent && !persistent && nav_container.is_none() {
                    let _ = proxy_nav.send_event(BrowserEvent::TabIpc(
                        tab_id,
                        UiToHostMessage::Navigate { url: nav },
                    ));
                    return false;
                }
                // HTTPS-only: plain-HTTP navigations are handed to the host, which upgrades them.
                if nav.starts_with("http://")
                    && !nav.starts_with(INTERNAL_ORIGIN)
                    && shoji_core::tabs::upgrade_to_https(&nav).is_some()
                    && nav_pages
                        .read()
                        .map(|c| c.settings.privacy.https_only)
                        .unwrap_or(false)
                {
                    let allowed = extract_host(&nav)
                        .map(|h| nav_insecure.lock().map(|s| s.contains(&h)).unwrap_or(false))
                        .unwrap_or(false);
                    if !allowed {
                        let _ = proxy_nav.send_event(BrowserEvent::TabIpc(
                            tab_id,
                            UiToHostMessage::Navigate { url: nav },
                        ));
                        return false;
                    }
                }
                true
            })
            .with_ipc_handler(move |req: wry::http::Request<String>| {
                let privileged = is_internal_origin(&req.uri().to_string());
                if let Ok(msg) = serde_json::from_str::<UiToHostMessage>(req.body()) {
                    if privileged || msg.allowed_from_web_content() {
                        let _ = proxy_ipc.send_event(BrowserEvent::TabIpc(tab_id, msg));
                    }
                }
            });
        if self.settings.appearance.enable_link_preview {
            builder = builder.with_initialization_script(LINK_PREVIEW_SCRIPT);
        }
        if !internal {
            builder = builder.with_initialization_script(pages::PAGE_INFO_SCRIPT);
            // Password manager: report login forms and fields (see ui/login.js).
            if self.settings.privacy.save_passwords
                && !self.settings.privacy.ephemeral_default
                && !private
            {
                builder = builder.with_initialization_script(pages::LOGIN_SCRIPT);
            }
        }
        // Every tab, not just web ones: an internal tab can navigate to a site in place.
        builder = builder.with_initialization_script(HIDE_SCROLLBAR_SCRIPT);
        builder = builder.with_initialization_script(SPLIT_DROP_SCRIPT);
        // Built blank, so document scripts are registered before the first navigation.
        if let Some(c) = &container {
            builder = builder.with_profile_name(container_profile(c));
        }
        let view = builder
            .build_as_child(window)
            .map_err(|e| eprintln!("Failed to create tab: {e:?}"))
            .ok()?;
        if persistent {
            self.apply_pending_cookies(&view);
        }
        let dark_script = webview::ScriptSlot::default();
        if let Some(script) = pages::dark_document_script(&self.settings) {
            webview::set_document_script(&view, &dark_script, Some(&script));
        }
        let font_script = webview::ScriptSlot::default();
        if let Some(script) = pages::font_document_script(&self.settings) {
            webview::set_document_script(&view, &font_script, Some(&script));
        }
        let emulated;
        let request_hook;
        {
            let pages = self.pages.clone();
            let mode_for: shield::ModeFor = Arc::new(move |page: &str| {
                pages
                    .read()
                    .map(|c| shield::parse_mode(c.settings.shield_mode_for(page)))
                    .unwrap_or(shoji_byobu::Mode::Standard)
            });
            let pages_fp = self.pages.clone();
            let fp_for: shield::FpFor = Arc::new(move |page: &str| {
                pages_fp
                    .read()
                    .map(|c| shield::parse_fp(c.settings.fingerprint_mode_for(page)))
                    .unwrap_or(shield::Fp::Standard)
            });
            request_hook = shield::attach(&view, self.shield.clone(), mode_for, fp_for, tab_id);
            let pages = self.pages.clone();
            emulated = darkfixes::attach(
                &view,
                Arc::new(move |url: &str| {
                    pages.read().is_ok_and(|c| {
                        !c.settings.appearance.dark_schedule.enabled
                            && extract_host(url).is_some_and(|h| c.settings.dark_on(&h))
                    })
                }),
            );
        }
        if navigate {
            let _ = view.load_url(&engine_url(url));
        }
        webview::attach_accelerators(&view, Origin::Tab(tab_id), self.proxy.clone());
        webview::attach_tab_events(
            &view,
            tab_id,
            self.proxy.clone(),
            self.allowed_cert_hosts.clone(),
        );
        webview::attach_focus(&view, tab_id, self.proxy.clone());
        webview::attach_nav_failures(&view, tab_id, self.proxy.clone());
        // Bounce tracking is cleaned in the main cookie jar; containers and private windows keep
        // their own (or nothing).
        if persistent && container.is_none() {
            webview::attach_bounce_watch(&view, self.proxy.clone());
        }
        // Autofill only where the browser also remembers things (passwords are Shoji's own).
        webview::configure_engine(
            &view,
            self.settings.privacy.save_passwords && persistent,
            tracking_level(&self.settings),
        );
        let _ = unsafe { view.controller().SetZoomFactor(self.zoom_for(url)) };
        Some(Tab {
            view,
            persistent,
            container,
            hidden_since: None,
            suspended: false,
            dark_script,
            font_script,
            emulated,
            request_hook,
            tint: None,
            has_video: false,
            born: Instant::now(),
            painted: false,
            pending_url: None,
        })
    }

    /// A new temporary container: it lasts until its last tab closes.
    fn new_temp_container(&mut self) -> String {
        format!(
            "{}{}",
            shoji_core::settings::TEMP_CONTAINER,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        )
    }

    /// Reopen a tab (at `url`, or where it is) in `container`. A view can't change its engine
    /// profile, so a new tab takes its place and the old one closes.
    fn reopen_in_container(
        &mut self,
        win_id: WindowId,
        tab_id: TabId,
        url: Option<String>,
        container: Option<String>,
    ) {
        let Some(state) = self
            .windows
            .get(&win_id)
            .and_then(|w| w.tabs.get_tab_by_id(tab_id).cloned())
        else {
            return;
        };
        let url = url.unwrap_or(state.url);
        let Some(new_id) = self.create_tab_in(win_id, url, true, container) else {
            return;
        };
        if let Some(win) = self.windows.get_mut(&win_id) {
            let pos = |id: TabId| win.tabs.tabs().iter().position(|t| t.id == id);
            if let (Some(old), Some(new)) = (pos(tab_id), pos(new_id)) {
                let _ = win.tabs.reorder_tab(new, old);
            }
            let _ = win.tabs.close_workspace_tab(tab_id, now_secs());
            win.ui_dirty = true;
        }
        self.sync_window_views(win_id);
    }

    /// Every container a tab can be in right now, for the tab strip's marks.
    fn containers_for_ui(&self) -> Vec<shoji_core::settings::Container> {
        let mut all = self.settings.containers.list.clone();
        let temps = self
            .windows
            .values()
            .flat_map(|w| w.tabs.all_tabs().filter_map(|t| t.container.clone()))
            .filter(|c| is_temp_container(c));
        for id in temps {
            if !all.iter().any(|c| c.id == id) {
                all.push(shoji_core::settings::Container {
                    id,
                    name: "Temporary".into(),
                    color: "#8a8a8a".into(),
                });
            }
        }
        all
    }

    fn create_tab(&mut self, win_id: WindowId, url: Option<String>) {
        let url = url.unwrap_or_else(|| format!("{INTERNAL_SCHEME}newtab"));
        self.create_tab_at(win_id, url, true);
    }

    /// A tab for a window a page opened. The view is left blank for the page to fill in.
    fn open_popup(&mut self, req: webview::PopupRequest) {
        let Some(win_id) = self.owner(req.tab_id) else {
            return req.refuse();
        };
        let uri = req.uri.clone();
        let url = if uri.is_empty() || uri == "about:blank" {
            "about:blank".to_string()
        } else {
            match normalize_url(&uri, self.settings.search_url_template()) {
                Ok(u) if u.starts_with("http") => u,
                _ => return req.refuse(),
            }
        };
        match self.create_tab_at(win_id, url, false) {
            Some(id) => {
                let view = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.views.get(&id))
                    .map(|t| &t.view);
                match view {
                    Some(view) => req.attach(view),
                    None => req.refuse(),
                }
            }
            None => req.refuse(),
        }
    }

    fn create_tab_at(&mut self, win_id: WindowId, url: String, navigate: bool) -> Option<TabId> {
        // A site with a container rule opens in that container.
        let container = extract_host(&url)
            .and_then(|h| self.settings.containers.for_host(&h).map(str::to_owned));
        self.create_tab_in(win_id, url, navigate, container)
    }

    fn create_tab_in(
        &mut self,
        win_id: WindowId,
        url: String,
        navigate: bool,
        container: Option<String>,
    ) -> Option<TabId> {
        let tab_id = TabId(
            self.tab_ids
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        );
        let win = self.windows.get(&win_id)?;
        let window = win.window.clone();
        let rect = win.content_bounds();
        let tab = self.build_view(&window, tab_id, &url, rect, navigate, container.clone())?;
        let win = self.windows.get_mut(&win_id)?;
        let title = match url.as_str() {
            "shoji://settings" => "Settings",
            "shoji://welcome" => "Welcome",
            "shoji://passwords" => "Passwords",
            _ => "New tab",
        };
        let workspace = win.tabs.active_workspace_id();
        win.tabs.insert_tab(
            TabState {
                id: tab_id,
                url,
                title: title.to_string(),
                favicon_uri: None,
                is_loading: false,
                can_go_back: false,
                can_go_forward: false,
                is_audio_playing: false,
                notice: false,
                is_muted: false,
                status: TabStatus::Inactive,
                last_active_timestamp_secs: now_secs(),
                group: None,
                pinned: false,
                workspace,
                container,
            },
            None,
        );
        win.views.insert(tab_id, tab);
        win.show_tab(tab_id);
        win.layout();
        let open = win.tabs.tabs().len();
        if open >= 40 && !self.hoarding_told {
            self.hoarding_told = true;
            self.toast(&serde_json::json!({
                "title": format!("{open} tabs open"),
                "text": "Which one is the real one?",
                "pose": "look",
                "ms": 6500,
            }));
        }
        Some(tab_id)
    }

    /// Reconcile `win.views` with `win.tabs`: drop views for removed tabs, build views for any
    /// newly created tabs, hide & suspend tabs in inactive workspaces, and show the active tab(s).
    fn sync_window_views(&mut self, win_id: WindowId) {
        let mut closing_temp: Vec<Tab> = Vec::new();
        let (window, rect, to_build, removed, inactive_ids, shown) = {
            let Some(win) = self.windows.get_mut(&win_id) else {
                return;
            };
            let live_ids: HashSet<TabId> = win.tabs.all_tabs().map(|t| t.id).collect();
            let max_id = live_ids.iter().map(|id| id.0).max().unwrap_or(0);
            self.tab_ids
                .fetch_max(max_id + 1, std::sync::atomic::Ordering::SeqCst);
            let removed: Vec<TabId> = win
                .views
                .keys()
                .copied()
                .filter(|id| !live_ids.contains(id))
                .collect();
            for id in &removed {
                if win.fullscreen_tab == Some(*id) {
                    win.fullscreen_tab = None;
                }
                win.reader.remove(id);
                self.last_upgrade.remove(id);
                if let Some(tab) = win.views.remove(id) {
                    if tab.container.as_deref().is_some_and(is_temp_container) {
                        closing_temp.push(tab);
                    }
                }
                shield::hub().forget_tab(id.0);
            }
            let mut to_build = Vec::new();
            for t in win.tabs.all_tabs() {
                if !win.views.contains_key(&t.id) {
                    let url = if t.url == "about:blank" || t.url.is_empty() {
                        format!("{INTERNAL_SCHEME}newtab")
                    } else {
                        t.url.clone()
                    };
                    to_build.push((t.id, url, t.container.clone()));
                }
            }
            let inactive_ids: HashSet<TabId> =
                win.tabs.inactive_tabs().iter().map(|t| t.id).collect();
            let shown: HashSet<TabId> = win
                .tabs
                .active_tab_id()
                .into_iter()
                .chain(win.tabs.split_panes().into_iter().flatten())
                .collect();
            (
                win.window.clone(),
                win.content_bounds(),
                to_build,
                removed,
                inactive_ids,
                shown,
            )
        };
        if !removed.is_empty() {
            if let Ok(mut ctx) = self.pages.write() {
                for id in &removed {
                    ctx.notices.remove(&id.0);
                }
            }
        }
        // Only what is about to show loads now; every other tab loads the first time it is shown,
        // so restoring a long session doesn't fetch every page at once.
        // A temporary container goes with its last tab: its cookies and storage are deleted.
        for tab in closing_temp {
            let c = tab.container.clone().unwrap_or_default();
            let in_use = self.windows.values().any(|w| {
                w.tabs
                    .all_tabs()
                    .any(|t| t.container.as_deref() == Some(c.as_str()))
            });
            if !in_use {
                webview::delete_profile(&tab.view);
            }
        }
        for (id, url, container) in to_build {
            let now = shown.contains(&id);
            if let Some(mut tab) = self.build_view(&window, id, &url, rect, now, container) {
                if !now {
                    tab.pending_url = Some(url.clone());
                }
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if url == format!("{INTERNAL_SCHEME}newtab") {
                        win.tabs.update_url(id, url);
                    }
                    win.views.insert(id, tab);
                }
            }
        }
        if let Some(win) = self.windows.get_mut(&win_id) {
            for &id in &inactive_ids {
                if let Some(tab) = win.views.get_mut(&id) {
                    if tab.hidden_since.is_none() {
                        BrowserWindow::hide(tab);
                    }
                    if !tab.suspended && tab.pending_url.is_none() {
                        webview::try_suspend(&tab.view, id, self.proxy.clone());
                    }
                }
            }
            if let Some(active) = win.tabs.active_tab_id() {
                win.show_tab(active);
            }
            win.layout();
            win.ui_dirty = true;
            win.sidebar_dirty = true;
        }
    }

    fn close_tab(&mut self, win_id: WindowId, tab_id: TabId, event_loop: &ActiveEventLoop) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        // Closing a window's only tab closes the window, as in other browsers.
        let was_last = win.tabs.all_tabs().count() == 1;
        // Remember it for the new tab page's "recently closed" list.
        if let Some(state) = win
            .tabs
            .get_tab_by_id(tab_id)
            .cloned()
            .filter(|_| !win.private)
        {
            if !is_internal(&state.url)
                && !state.url.is_empty()
                && !state.url.starts_with("edge://")
            {
                if let Ok(mut ctx) = self.pages.write() {
                    ctx.recent.retain(|c| c.url != state.url);
                    ctx.recent.insert(
                        0,
                        shoji_core::tabs::ClosedTab {
                            url: state.url,
                            title: state.title,
                        },
                    );
                    ctx.recent.truncate(20);
                }
            }
        }
        let _ = win.tabs.close_workspace_tab(tab_id, now_secs());
        self.sync_window_views(win_id);
        self.push_data();
        self.fold_crane();
        if was_last {
            self.close_window(win_id, event_loop);
        }
    }

    fn open_internal(&mut self, win_id: WindowId, page: &str) {
        let url = format!("{INTERNAL_SCHEME}{page}");
        if let Some(win) = self.windows.get_mut(&win_id) {
            win.sidebar_open = false;
            win.pop_hide_at = None;
            if let Some(s) = &win.sidebar {
                let _ = s.set_visible(false);
            }
            // A page already open (even scrolled to a section with "#name") is shown, not duplicated.
            let same = |t: &str| {
                t == url
                    || t.strip_prefix(&url)
                        .is_some_and(|r| r.starts_with('#') || r.starts_with("/#"))
            };
            if let Some(existing) = win.tabs.tabs().iter().find(|t| same(&t.url)).map(|t| t.id) {
                win.show_tab(existing);
                win.layout();
                return;
            }
        }
        self.create_tab(win_id, Some(url));
    }

    /// Navigate the active tab, rebuilding it if the cookie partition must change.
    fn navigate(&mut self, win_id: WindowId, raw: &str) {
        let target = match normalize_url(raw, self.settings.search_url_template()) {
            Ok(t) => t,
            Err(e) => return eprintln!("Navigation blocked: {e}"),
        };
        for page in [
            "settings",
            "passwords",
            "history",
            "bookmarks",
            "downloads",
            "wish",
            "tea",
            "koan",
        ] {
            if target == format!("{INTERNAL_SCHEME}{page}") {
                return self.open_internal(win_id, page);
            }
        }
        let Some(active) = self
            .windows
            .get(&win_id)
            .and_then(|w| w.tabs.active_tab_id())
        else {
            return;
        };
        let target = self.upgrade_target(active, target);
        let zoom = self.zoom_for(&target);
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        let wants_persistent =
            !is_internal(&target) && !win.private && self.settings.is_site_persistent(&target);
        win.tabs.update_url(active, target.clone());
        win.tabs.update_favicon(active, None);
        win.reader.remove(&active);
        win.ui_dirty = true;
        win.sidebar_dirty = true;
        let same_partition = win.views.get(&active).map(|t| t.persistent) == Some(wants_persistent);
        if same_partition || is_internal(&target) {
            if let Some(tab) = win.views.get_mut(&active) {
                tab.pending_url = None;
                let _ = tab.view.load_url(&engine_url(&target));
                if !is_internal(&target) {
                    let _ = unsafe { tab.view.controller().SetZoomFactor(zoom) };
                }
            }
            return;
        }
        let window = win.window.clone();
        let rect = win.rect_for(active);
        let container = win
            .tabs
            .get_tab_by_id(active)
            .and_then(|t| t.container.clone());
        if let Some(tab) = self.build_view(&window, active, &target, rect, true, container) {
            if let Some(win) = self.windows.get_mut(&win_id) {
                win.views.insert(active, tab);
                win.show_tab(active);
                win.layout();
            }
        }
    }

    fn reload(&mut self, win_id: WindowId, hard: bool) {
        let Some(win) = self.windows.get(&win_id) else {
            return;
        };
        let Some(state) = win.tabs.active_tab().cloned() else {
            return;
        };
        let partition_ok = win.views.get(&state.id).map(|t| t.persistent)
            == Some(
                !is_internal(&state.url)
                    && !win.private
                    && self.settings.is_site_persistent(&state.url),
            );
        if !partition_ok {
            return self.navigate(win_id, &state.url);
        }
        let Some(tab) = win.views.get(&state.id) else {
            return;
        };
        if state.status == TabStatus::Crashed {
            let _ = tab.view.load_url(&engine_url(&state.url));
        } else if hard {
            if let Ok(core) = unsafe { tab.view.controller().CoreWebView2() } {
                let _ = unsafe {
                    core.CallDevToolsProtocolMethod(
                        windows::core::w!("Page.reload"),
                        windows::core::w!("{\"ignoreCache\":true}"),
                        None,
                    )
                };
            }
        } else {
            let _ = tab.view.reload();
        }
    }

    /// Move a live tab into another window (or a new one) without reloading it, from a drop at
    /// `screen` (logical px) as reported by the page.
    fn detach_tab(
        &mut self,
        src: WindowId,
        tab_id: TabId,
        screen: Option<(f64, f64)>,
        event_loop: &ActiveEventLoop,
    ) {
        let only_tab = self
            .windows
            .get(&src)
            .is_some_and(|w| w.tabs.tabs().len() == 1 && w.tabs.workspaces().len() == 1);
        let target = screen.and_then(|(x, y)| {
            self.windows.iter().find_map(|(id, w)| {
                let pos = w.window.outer_position().ok()?;
                let scale = w.window.scale_factor();
                let pos = pos.to_logical::<f64>(scale);
                let size = w.window.outer_size().to_logical::<f64>(scale);
                (*id != src
                    && x >= pos.x
                    && x <= pos.x + size.width
                    && y >= pos.y
                    && y <= pos.y + size.height)
                    .then_some(*id)
            })
        });
        if target.is_none() && only_tab {
            return; // dragging the last tab out of its own window: nothing to do
        }
        let Some(dst) = self.move_tab(src, tab_id, target, None, event_loop) else {
            return;
        };
        if target.is_none() {
            if let (Some((x, y)), Some(dst_win)) = (screen, self.windows.get(&dst)) {
                dst_win.window.set_outer_position(LogicalPosition::new(
                    (x - 120.0).max(0.0),
                    (y - 16.0).max(0.0),
                ));
            }
        }
    }

    /// A window to put a moved tab in: the ready spare when there is one, else a new one.
    fn take_window(&mut self, event_loop: &ActiveEventLoop) -> Option<WindowId> {
        if let Some((id, mut win)) = self.spare.take() {
            {
                use winit::platform::windows::WindowExtWindows;
                win.window.set_skip_taskbar(false);
                win.window
                    .set_outer_position(winit::dpi::PhysicalPosition::new(0, 0));
            }
            let logical = win
                .window
                .inner_size()
                .to_logical::<f64>(win.window.scale_factor());
            win.width = logical.width;
            win.height = logical.height;
            self.windows.insert(id, win);
            self.spare_due = Instant::now() + Duration::from_secs(2);
            return Some(id);
        }
        self.create_window(event_loop, false)
    }

    /// Build the spare window while nothing is going on, so tearing a tab out is instant.
    fn warm_spare(&mut self, event_loop: &ActiveEventLoop) {
        if self.spare.is_some() || self.tab_drag.is_some() || Instant::now() < self.spare_due {
            return;
        }
        if !self.settings.tabs.enable_tab_tearoff
            || left_button_down()
            || !self
                .windows
                .values()
                .any(|w| w.tabs.tabs().len() + w.tabs.inactive_tabs().len() > 1)
        {
            // Not wanted right now. Look again shortly instead of leaving `spare_due` in the past,
            // which would make the event loop wake continuously.
            self.spare_due = Instant::now() + Duration::from_secs(2);
            return;
        }
        // Building a window while pages load is several times slower and blocks the UI for all
        // of it; wait for a quiet moment.
        if self
            .windows
            .values()
            .any(|w| w.tabs.tabs().iter().any(|t| t.is_loading))
        {
            self.spare_due = Instant::now() + Duration::from_secs(1);
            return;
        }
        self.spare_building = true;
        let made = self.create_window(event_loop, false);
        self.spare_building = false;
        if let Some(id) = made {
            if let Some(win) = self.windows.remove(&id) {
                win.window.set_visible(false);
                self.spare = Some((id, win));
            }
        }
    }

    /// Move a live tab from `src` into window `dst` at strip position `at` (or into a new window
    /// when `dst` is `None`) without reloading it. Returns the window it ended up in. When `src`
    /// is left with nothing it closes.
    fn move_tab(
        &mut self,
        src: WindowId,
        tab_id: TabId,
        dst: Option<WindowId>,
        at: Option<usize>,
        event_loop: &ActiveEventLoop,
    ) -> Option<WindowId> {
        if dst == Some(src) {
            return None;
        }
        let win = self.windows.get_mut(&src)?;
        let (Some(state), Some(tab)) = (win.tabs.extract_tab(tab_id), win.views.remove(&tab_id))
        else {
            return None;
        };
        if win.fullscreen_tab == Some(tab_id) {
            win.fullscreen_tab = None;
        }
        win.reader.remove(&tab_id);
        let src_next = win.tabs.active_tab_id();
        let src_has_other_workspaces = win.tabs.workspaces().len() > 1;
        let dst = dst.or_else(|| self.take_window(event_loop))?;
        let dst_win = self.windows.get_mut(&dst)?;
        if let Ok(handle) =
            winit::raw_window_handle::HasWindowHandle::window_handle(dst_win.window.as_ref())
        {
            if let winit::raw_window_handle::RawWindowHandle::Win32(h) = handle.as_raw() {
                let _ = tab.view.reparent(h.hwnd.get());
            }
        }
        for other in dst_win.views.values_mut() {
            if other.hidden_since.is_none() {
                BrowserWindow::hide(other);
            }
        }
        // Group, split and workspace ids are per window.
        let mut state = state;
        state.group = None;
        state.workspace = dst_win.tabs.active_workspace_id();
        dst_win.tabs.insert_tab(state, at);
        dst_win.views.insert(tab_id, tab);
        dst_win.show_tab(tab_id);
        dst_win.layout();
        dst_win.ui_dirty = true;
        dst_win.window.set_visible(true);
        dst_win.window.focus_window();

        match src_next {
            Some(next) => {
                if let Some(win) = self.windows.get_mut(&src) {
                    win.show_tab(next);
                    win.layout();
                    win.ui_dirty = true;
                }
            }
            None if src_has_other_workspaces => {
                self.create_tab(src, None);
            }
            None => self.close_window(src, event_loop),
        }
        Some(dst)
    }

    /* ----- favourites, sessions, per-site data ----- */

    /// Zoom for a page: its site's remembered zoom, else the default.
    fn zoom_for(&self, url: &str) -> f64 {
        extract_host(url)
            .and_then(|h| self.site_data.zoom_for(&h))
            .unwrap_or(self.settings.appearance.default_zoom_level)
    }

    /// Hand `target` to HTTPS-only mode: the https:// address to load instead, or `target` itself.
    fn upgrade_target(&mut self, tab: TabId, target: String) -> String {
        if !self.settings.privacy.https_only {
            return target;
        }
        let (Some(up), Some(host)) = (
            shoji_core::tabs::upgrade_to_https(&target),
            extract_host(&target),
        ) else {
            return target;
        };
        if self
            .insecure_hosts
            .lock()
            .map(|s| s.contains(&host))
            .unwrap_or(false)
        {
            return target;
        }
        // A site that redirects https -> http would loop forever: let the second attempt through.
        if let Some((last, at)) = self.last_upgrade.get(&tab) {
            if *last == target && at.elapsed() < Duration::from_secs(3) {
                return target;
            }
        }
        self.last_upgrade.insert(tab, (target, Instant::now()));
        self.upgraded.insert(host, Instant::now());
        up
    }

    /// Push favourites, recently closed tabs and remembered-choice counts to every surface.
    fn push_data(&mut self) {
        let (favs, recent, restore) = {
            let Ok(mut ctx) = self.pages.write() else {
                return;
            };
            ctx.favourites = self.favourites.items().to_vec();
            ctx.site_choices = self.site_data.count();
            (ctx.favourites.clone(), ctx.recent.clone(), ctx.restore_tabs)
        };
        let chrome = format!(
            "window.__setData && window.__setData({})",
            serde_json::json!({
                "favs": favs,
                "folders": self.favourites.all_folders(),
                "recent": recent.iter().take(20).collect::<Vec<_>>(),
            })
        );
        let bookmarks = format!(
            "window.__bookmarks && window.__bookmarks({})",
            self.bookmarks_json()
        );
        let home = format!(
            "window.__homeData && window.__homeData({})",
            pages::home_json(&pages::HomeData {
                favourites: &favs,
                recent: &recent,
                restore_tabs: restore
            })
        );
        for win in self.windows.values() {
            win.eval_chrome(&chrome);
            win.eval_internal_tabs(&home);
            win.eval_internal_tabs(&bookmarks);
            // Kept current even while the dropdown is shut, so it opens with its list ready.
            if win.sidebar.is_some() {
                win.eval_sidebar(&bookmarks);
            }
        }
    }

    fn build_session(&self, clean: bool) -> shoji_core::userdata::Session {
        let mut windows = Vec::new();
        for win in self.windows.values().filter(|w| !w.private) {
            if let Some(sw) = win.tabs.to_session_window() {
                windows.push(sw);
            }
        }
        let mut session = shoji_core::userdata::Session {
            clean_exit: clean,
            windows,
        };
        // Until the user answers the restore offer, never overwrite the old tabs with nothing.
        if !session.has_pages() {
            if let Some(previous) = &self.pending_session {
                session = previous.clone();
                session.clean_exit = false;
            }
        }
        session
    }

    fn session_enabled(&self) -> bool {
        !self.settings.privacy.ephemeral_default && self.settings.privacy.restore_session != "never"
    }

    fn write_session(&mut self, clean: bool) {
        self.history.flush();
        if !self.session_enabled() {
            return;
        }
        let session = self.build_session(clean);
        if let Ok(json) = serde_json::to_string(&session) {
            if json != self.session_json {
                session.save(&self.session_path);
                self.session_json = json;
            }
        }
        self.session_written = Instant::now();
    }

    /// Keep the on-disk list of open tabs current, at most once a second.
    /// Returns when it next needs a look, if a change is waiting.
    fn maybe_write_session(&mut self) -> Option<Instant> {
        // Only rebuild the session after something it records may have changed.
        if !self.session_dirty || !self.session_enabled() || self.windows.is_empty() {
            return None;
        }
        // Every event ends in about_to_wait; don't rebuild the whole session for each one.
        let check_due = self.session_checked + Duration::from_millis(400);
        if Instant::now() < check_due {
            return Some(check_due);
        }
        self.session_checked = Instant::now();
        let json = serde_json::to_string(&self.build_session(false)).ok()?;
        if json == self.session_json {
            self.session_dirty = false;
            return None;
        }
        let due = self.session_written + Duration::from_secs(1);
        if Instant::now() >= due {
            self.write_session(false);
            self.session_dirty = false;
            None
        } else {
            Some(due)
        }
    }

    /// Open the tabs of one saved window into `win_id`.
    fn open_session_window(
        &mut self,
        win_id: WindowId,
        saved: &shoji_core::userdata::SessionWindow,
        _event_loop: &ActiveEventLoop,
    ) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        win.views.clear();
        win.reader.clear();
        win.tabs.restore_session_window(saved, now_secs());
        self.sync_window_views(win_id);
    }

    fn restore_session(&mut self, win_id: WindowId, event_loop: &ActiveEventLoop) {
        let Some(session) = self.pending_session.take() else {
            return;
        };
        if let Ok(mut ctx) = self.pages.write() {
            ctx.restore_tabs = 0;
        }
        for (i, saved) in session.windows.iter().enumerate() {
            let target = if i == 0 {
                Some(win_id)
            } else {
                let id = self.create_window(event_loop, false);
                if let Some(w) = id.and_then(|id| self.windows.get(&id)) {
                    w.window.set_visible(true);
                }
                id
            };
            if let Some(target) = target {
                self.open_session_window(target, saved, event_loop);
            }
        }
        self.push_data();
    }

    /* ----- overlay: suggestions, command palette, menus, prompts ----- */

    fn ensure_overlay(&mut self, win_id: WindowId) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        if win.overlay.is_some() {
            return;
        }
        let proxy = self.proxy.clone();
        let dark = window::is_dark(&win.window, &self.settings.appearance.theme);
        win.overlay_ready = false;
        win.overlay = new_builder()
            .with_bounds(bounds(0.0, 0.0, 10.0, 10.0))
            .with_visible(false)
            .with_html(pages::overlay(&self.settings))
            .with_custom_protocol("shoji".into(), |_, request| font_response(&request))
            .with_incognito(true)
            .with_background_color(window::page_rgba(dark))
            // A new webview takes the keyboard by default. This one is built the first time the
            // suggestions appear, which must not pull focus out of the address bar mid-typing;
            // menus and prompts focus it themselves.
            .with_focused(false)
            .with_theme(window::webview_theme(&self.settings.appearance.theme))
            .with_ipc_handler(move |req: wry::http::Request<String>| {
                if let Ok(msg) = serde_json::from_str(req.body()) {
                    let _ = proxy.send_event(BrowserEvent::WindowIpc(win_id, msg));
                }
            })
            .build_as_child(win.window.as_ref())
            .ok();
        if let Some(o) = &win.overlay {
            webview::attach_accelerators(o, Origin::Window(win_id), self.proxy.clone());
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn show_overlay(
        &mut self,
        win_id: WindowId,
        mode: &'static str,
        items: Vec<shoji_core::ipc::OverlayItem>,
        frame: Frame,
        title: &str,
        placeholder: &str,
        value: &str,
    ) {
        self.ensure_overlay(win_id);
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        win.overlay_mode = mode;
        win.overlay_open = true;
        win.overlay_shown = Some(Instant::now());
        let payload = serde_json::json!({
            "mode": mode, "items": items, "title": title, "placeholder": placeholder, "value": value,
            "h": frame[3],
        });
        win.overlay_items = items;
        let js = format!("window.__overlayShow && window.__overlayShow({payload})");
        if let Some(o) = &win.overlay {
            if mode == "suggest" {
                // The suggestion list changes height with every keystroke. Resizing a webview
                // repaints it, and for a moment the new part shows what is behind it; so the
                // window stays one size and only the part that is wanted is shown (cheap, and
                // nothing has to be repainted).
                let _ = o.set_bounds(frame_rect([
                    frame[0],
                    frame[1],
                    frame[2],
                    SUGGEST_MAX_HEIGHT,
                ]));
                let scale = win.window.scale_factor();
                webview::set_region(
                    o,
                    0,
                    0,
                    (frame[2] * scale).ceil() as i32,
                    (frame[3] * scale).ceil() as i32,
                );
            } else {
                webview::clear_region(o);
                let _ = o.set_bounds(frame_rect(frame));
            }
            // Until the page is ready the view would show black; `OverlayReady` reveals it.
            let _ = o.set_visible(win.overlay_ready);
            webview::bring_to_top(o);
            if win.overlay_ready {
                let _ = o.evaluate_script(&js);
                if mode != "suggest" {
                    webview::focus(o);
                }
            } else {
                win.overlay_pending = Some(js);
            }
        }
    }

    fn hide_overlay(&mut self, win_id: WindowId, refocus: bool) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        if !win.overlay_open {
            return;
        }
        win.overlay_open = false;
        win.overlay_pending = None;
        win.overlay_items.clear();
        if let Some(o) = &win.overlay {
            let _ = o.set_visible(false);
        }
        if refocus {
            // A floating bookmarks panel that asked for the menu gets the keyboard back.
            match (
                &win.sidebar,
                win.sidebar_open && !win.sidebar_docked && win.sidebar_mode == "bookmarks",
            ) {
                (Some(s), true) => webview::focus(s),
                _ => {
                    if let Some(tab) = win.active() {
                        webview::focus(&tab.view);
                    }
                }
            }
        }
    }

    fn open_prompt(&mut self, win_id: WindowId, prompt: Prompt, title: &str, value: &str) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        win.prompt = Some(prompt);
        let w = 420.0f64.min(win.width - 20.0);
        let frame = [
            ((win.width - w) / 2.0).max(4.0),
            win.chrome_height() + 24.0,
            w,
            112.0,
        ];
        self.show_overlay(win_id, "prompt", Vec::new(), frame, title, "Name", value);
    }

    fn finish_prompt(&mut self, win_id: WindowId, text: &str) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        match win.prompt.take() {
            Some(Prompt::NewGroup(tab)) => {
                let color = next_group_color(win);
                let gid = win.tabs.create_group(text, color);
                win.tabs.set_tab_group(tab, Some(gid));
                gather_group(win, tab, gid);
            }
            Some(Prompt::RenameGroup(gid)) => win.tabs.rename_group(gid, text),
            Some(Prompt::RenameSplit(id)) => {
                win.tabs.rename_split(id, text);
            }
            Some(Prompt::NewFolderFor(url)) => {
                self.favourites.set_folder(&url, text);
                self.push_data();
            }
            Some(Prompt::BookmarkName(url)) => {
                self.favourites.edit(&url, text, &url);
                self.push_data();
            }
            Some(Prompt::RenameBookmarkFolder(from)) => {
                let leaf = text.replace('/', "-");
                let to = shoji_core::userdata::clean_folder(&format!(
                    "{} / {leaf}",
                    shoji_core::userdata::folder_parent(&from)
                ));
                self.favourites.rename_folder(&from, &to);
                self.push_data();
            }
            Some(Prompt::NewBookmarkFolder(parent)) => {
                let name = text.replace('/', "-");
                self.favourites.add_folder(&format!("{parent} / {name}"));
                self.push_data();
            }
            Some(Prompt::NewWorkspace) => {
                let color = next_workspace_color(win);
                let ws_id = win.tabs.create_workspace(text, color);
                self.switch_workspace(win_id, ws_id);
                return;
            }
            Some(Prompt::RenameWorkspace(ws_id)) => {
                win.tabs.rename_workspace(ws_id, text);
                win.sidebar_dirty = true;
            }
            None => {}
        }
        if let Some(win) = self.windows.get_mut(&win_id) {
            win.ui_dirty = true;
        }
    }

    fn palette_items(&self, win_id: WindowId) -> Vec<shoji_core::ipc::OverlayItem> {
        use shoji_core::ipc::OverlayItem;
        let item = |kind: &str, value: String, label: &str, sub: String| OverlayItem {
            kind: kind.to_string(),
            value,
            label: label.to_string(),
            sub,
            color: None,
        };
        let mut out = Vec::new();
        let Some(win) = self.windows.get(&win_id) else {
            return out;
        };
        let active = win.tabs.active_tab_id();
        for t in win.tabs.tabs() {
            let label = if t.title.is_empty() {
                t.url.as_str()
            } else {
                t.title.as_str()
            };
            let mut it = item("tab", t.id.0.to_string(), label, t.url.clone());
            it.color = t.group.and_then(|g| group_tone(win, g));
            out.push(it);
        }
        for ws in win.tabs.workspaces() {
            if ws.id != win.tabs.active_workspace_id() {
                let mut it = item(
                    "action",
                    format!("ws-switch:{}", ws.id),
                    &format!("Switch to workspace: {}", ws.name),
                    String::new(),
                );
                it.color = workspace_tone(win, ws.id);
                out.push(it);
            }
        }
        for f in self.favourites.items() {
            out.push(item("url", f.url.clone(), &f.title, f.url.clone()));
        }
        for (name, label, sub) in [
            ("new-tab", "New tab", "Ctrl+T"),
            ("new-window", "New window", "Ctrl+N"),
            ("reopen", "Reopen closed tab", "Ctrl+Shift+T"),
            ("spaces", "Spaces sidebar", "Alt+W"),
            ("ws-new", "New workspace", "Alt+Shift+N"),
            ("split-toggle", "Toggle split view", "Ctrl+Shift+S"),
            ("split-orient", "Flip split view orientation", "Alt+Shift+S"),
            ("split-focus", "Focus other split pane", "Alt+S"),
            ("favourite", "Bookmark or unbookmark this page", "Ctrl+D"),
            ("bookmark-all", "Bookmark all tabs", "Ctrl+Shift+D"),
            ("bookmark-manager", "Bookmark manager", "Ctrl+Shift+O"),
            (
                "bookmarks-bar",
                "Show or hide the bookmarks bar",
                "Ctrl+Shift+B",
            ),
            ("bookmarks-panel", "Bookmarks panel", ""),
            ("new-private-window", "New private window", "Ctrl+Shift+N"),
            ("reader", "Reader view", "F9"),
            ("pip", "Picture in picture", ""),
            ("find", "Find in page", "Ctrl+F"),
            ("downloads", "Downloads", "Ctrl+J"),
            ("notes", "Quick note", ""),
            ("dark", "Turn dark websites on or off", ""),
            ("history", "History", "Ctrl+H"),
            ("clear-data", "Clear browsing data…", "Ctrl+Shift+Del"),
            ("darkpanel", "Dark website settings…", ""),
            ("settings", "Settings", ""),
            ("passwords", "Passwords", ""),
            ("view-source", "View page source", "Ctrl+U"),
            ("save-page", "Save page as…", "Ctrl+S"),
            ("fullscreen", "Full screen", "F11"),
            ("wish", "Make a wish", "Senbazuru: a thousand cranes"),
            ("tea", "Take a tea break", "418"),
            ("koan", "A word from the crane", "A koan or a haiku"),
            ("fold", "Fold a crane", ""),
            ("devtools", "Developer tools", "F12"),
        ] {
            out.push(item("action", name.to_string(), label, sub.to_string()));
        }
        {
            let (page, label) = (
                "shoji://settings",
                "Settings: appearance, privacy, passwords",
            );
            out.push(item("url", page.to_string(), label, page.to_string()));
        }
        if win.tabs.visible_split().is_some() {
            out.push(item(
                "action",
                "split-exit".into(),
                "Exit split view",
                "Ctrl+Shift+S".into(),
            ));
            out.push(item(
                "action",
                "split-reset".into(),
                "Reset split divider to 50/50",
                String::new(),
            ));
        } else {
            for t in win.tabs.tabs().iter().filter(|t| Some(t.id) != active) {
                let label = if t.title.is_empty() {
                    t.url.as_str()
                } else {
                    t.title.as_str()
                };
                out.push(item(
                    "action",
                    format!("split:{}", t.id.0),
                    &format!("Split view with: {label}"),
                    String::new(),
                ));
            }
        }
        if let Some(a) = active {
            out.push(item(
                "action",
                format!("group-new:{}", a.0),
                "New tab group with this tab",
                String::new(),
            ));
        }
        out
    }

    fn tab_menu_items(&self, win_id: WindowId, tab_id: TabId) -> Vec<shoji_core::ipc::OverlayItem> {
        use shoji_core::ipc::OverlayItem;
        let item = |value: String, label: &str, color: Option<String>| OverlayItem {
            kind: "action".into(),
            value,
            label: label.to_string(),
            sub: String::new(),
            color,
        };
        let mut out = Vec::new();
        let Some(win) = self.windows.get(&win_id) else {
            return out;
        };
        let Some(tab) = win.tabs.get_tab_by_id(tab_id) else {
            return out;
        };
        let active = win.tabs.active_tab_id();
        if let Some(sp) = win.tabs.split_of(tab_id) {
            let sid = sp.id;
            out.push(item(
                format!("split-rename:{sid}"),
                &format!("Rename split group \u{201c}{}\u{201d}\u{2026}", sp.name),
                None,
            ));
            out.push(item(
                format!("split-color:{sid}"),
                "Change split group colour",
                None,
            ));
            out.push(item(
                "split-toggle".into(),
                "Add a tab to this split\u{2026}",
                None,
            ));
            out.push(item(
                format!("split-remove:{}", tab_id.0),
                "Take this tab out of the split",
                None,
            ));
            out.push(item(
                format!("split-swap:{sid}"),
                if sp.panes.len() > 2 {
                    "Reverse the order of the panes"
                } else {
                    "Swap the two panes"
                },
                None,
            ));
            out.push(item("split-orient".into(), "Flip split orientation", None));
            out.push(item("split-reset".into(), "Even out the panes", None));
            out.push(item(
                format!("split-close:{sid}"),
                "Ungroup (close split view)",
                None,
            ));
        } else if Some(tab_id) != active && tab.workspace == win.tabs.active_workspace_id() {
            out.push(item(
                format!("split:{}", tab_id.0),
                "Split view with the open tab",
                None,
            ));
        }
        for c in &self.settings.containers.list {
            if tab.container.as_deref() != Some(c.id.as_str()) {
                out.push(item(
                    format!("ctr:{}:{}", tab_id.0, c.id),
                    &format!("Open in container: {}", c.name),
                    Some(c.color.clone()),
                ));
            }
        }
        out.push(item(
            format!("ctr:{}:temp", tab_id.0),
            "Open in a temporary container",
            None,
        ));
        if tab.container.is_some() {
            out.push(item(
                format!("ctr:{}:", tab_id.0),
                "Open outside containers",
                None,
            ));
        }
        if let (Some(c), Some(host)) = (
            tab.container
                .as_deref()
                .and_then(|c| self.settings.containers.get(c)),
            extract_host(&tab.url),
        ) {
            let site = shoji_core::settings::extract_domain(&host);
            let site = site.trim_start_matches("www.");
            if self.settings.containers.for_host(site) == Some(c.id.as_str()) {
                out.push(item(
                    format!("ctr-never:{}", tab_id.0),
                    &format!("Stop always opening {site} in {}", c.name),
                    None,
                ));
            } else {
                out.push(item(
                    format!("ctr-always:{}", tab_id.0),
                    &format!("Always open {site} in {}", c.name),
                    Some(c.color.clone()),
                ));
            }
        }
        out.push(item(
            format!("pin:{}", tab_id.0),
            if tab.pinned { "Unpin tab" } else { "Pin tab" },
            None,
        ));
        if tab.is_audio_playing || tab.is_muted {
            out.push(item(
                format!("mute:{}", tab_id.0),
                if tab.is_muted {
                    "Unmute tab"
                } else {
                    "Mute tab"
                },
                None,
            ));
        }
        for ws in win.tabs.workspaces() {
            if ws.id != tab.workspace {
                out.push(item(
                    format!("ws-move:{}:{}", tab_id.0, ws.id),
                    &format!("Move to workspace: {}", ws.name),
                    workspace_tone(win, ws.id),
                ));
            }
        }
        out.push(item(
            format!("tab-new-space:{}", tab_id.0),
            "Move to a new space",
            None,
        ));
        for g in win.tabs.groups().iter().filter(|_| !tab.pinned) {
            if tab.group != Some(g.id) {
                out.push(item(
                    format!("group-set:{}:{}", tab_id.0, g.id),
                    &format!("Move to group: {}", g.name),
                    group_tone(win, g.id),
                ));
            }
        }
        if !tab.pinned {
            out.push(item(
                format!("group-new:{}", tab_id.0),
                "New group with this tab…",
                None,
            ));
        }
        if let Some(g) = tab.group {
            out.push(item(
                format!("group-none:{}", tab_id.0),
                "Remove from group",
                None,
            ));
            out.push(item(format!("group-rename:{g}"), "Rename group…", None));
            out.push(item(
                format!("group-color:{g}"),
                "Change group color",
                group_tone(win, g),
            ));
        }
        let web = tab.url.starts_with("http://") || tab.url.starts_with("https://");
        if web {
            let label = if self.favourites.contains(&tab.url) {
                "Remove bookmark"
            } else {
                "Bookmark this tab"
            };
            out.push(item(format!("fav:{}", tab_id.0), label, None));
            if self.favourites.contains(&tab.url) {
                out.push(item(
                    format!("fav-folder:{}", tab_id.0),
                    "Move bookmark to a folder…",
                    None,
                ));
            }
        }
        out.push(item(format!("reload:{}", tab_id.0), "Reload", None));
        out.push(item(format!("close:{}", tab_id.0), "Close tab", None));
        out
    }

    fn workspace_menu_items(
        &self,
        win_id: WindowId,
        ws_id: u32,
    ) -> Vec<shoji_core::ipc::OverlayItem> {
        use shoji_core::ipc::OverlayItem;
        let item = |value: String, label: &str, color: Option<String>| OverlayItem {
            kind: "action".into(),
            value,
            label: label.to_string(),
            sub: String::new(),
            color,
        };
        let mut out = Vec::new();
        let Some(win) = self.windows.get(&win_id) else {
            return out;
        };
        for ws in win.tabs.workspaces() {
            let prefix = if ws.id == win.tabs.active_workspace_id() {
                "● "
            } else {
                ""
            };
            out.push(item(
                format!("ws-switch:{}", ws.id),
                &format!("{prefix}{}", ws.name),
                workspace_tone(win, ws.id),
            ));
        }
        out.push(item("ws-new".into(), "New workspace…", None));
        out.push(item("spaces".into(), "Manage spaces in sidebar", None));
        out.push(item(
            format!("ws-rename:{ws_id}"),
            "Rename workspace…",
            None,
        ));
        out.push(item(
            format!("ws-color:{ws_id}"),
            "Change workspace color",
            workspace_tone(win, ws_id),
        ));
        if let Some(idx) = win.tabs.workspaces().iter().position(|w| w.id == ws_id) {
            if idx > 0 {
                out.push(item(
                    format!("ws-reorder:{}:{}", idx, idx - 1),
                    "Move workspace earlier",
                    None,
                ));
            }
            if idx + 1 < win.tabs.workspaces().len() {
                out.push(item(
                    format!("ws-reorder:{}:{}", idx, idx + 1),
                    "Move workspace later",
                    None,
                ));
            }
        }
        if win.tabs.workspaces().len() > 1 {
            out.push(item(format!("ws-delete:{ws_id}"), "Delete workspace", None));
        }
        out
    }

    /// Open `url` (a page, a search, or the new tab page for `None`) in a new tab and show it as
    /// another pane beside the one in view.
    fn add_new_pane(&mut self, win_id: WindowId, url: Option<String>) {
        let url = url.and_then(|raw| {
            normalize_url(&raw, self.settings.search_url_template())
                .ok()
                .filter(|u| !u.starts_with("javascript:"))
        });
        let prev = self
            .windows
            .get(&win_id)
            .and_then(|w| w.tabs.active_tab_id());
        self.create_tab(win_id, url);
        if let Some(win) = self.windows.get_mut(&win_id) {
            if let (Some(prev), Some(new)) = (prev, win.tabs.active_tab_id()) {
                if prev == new {
                    return;
                }
                let joined = match win.tabs.split_of(prev).map(|s| s.id) {
                    Some(group) => win.tabs.add_to_split(group, new, None),
                    None => {
                        let color = next_split_color(win);
                        win.tabs.open_split_with(prev, new, "", color).is_some()
                    }
                };
                if joined {
                    win.show_tab(new);
                    win.layout();
                    win.ui_dirty = true;
                }
            }
        }
    }

    fn run_action(&mut self, win_id: WindowId, value: &str, event_loop: &ActiveEventLoop) {
        use UiToHostMessage as M;
        // "split-open:<address or search>" carries a colon of its own, so it is read first.
        if let Some(rest) = value.strip_prefix("split-open:") {
            return self.add_new_pane(win_id, Some(rest.to_string()));
        }
        if let Some(rest) = value.strip_prefix("bm:") {
            return self.bookmark_action(win_id, rest, event_loop);
        }
        let mut parts = value.splitn(3, ':');
        let name = parts.next().unwrap_or("");
        let a = parts.next().unwrap_or("");
        let b = parts.next().unwrap_or("");
        let tab = a.parse::<u64>().ok().map(TabId);
        match name {
            "engine" => {
                if ["google", "duckduckgo", "bing", "brave", "ecosia"].contains(&a) {
                    self.update_settings(|s| s.search_engine = a.to_string());
                }
            }
            "new-tab" => self.create_tab(win_id, None),
            "new-window" => self.new_window(event_loop, None),
            "reopen" => self.handle_command(win_id, Command::ReopenClosedTab, event_loop),
            "spaces" => self.toggle_sidebar(win_id, "spaces"),
            "downloads" => self.handle_ipc(win_id, None, M::ToggleDownloadsSidebar, event_loop),
            "notes" => self.handle_ipc(win_id, None, M::ToggleNotes, event_loop),
            "find" => self.handle_ipc(win_id, None, M::ToggleFindBar, event_loop),
            "reader" => self.toggle_reader(win_id),
            "pip" => self.handle_ipc(win_id, None, M::TogglePictureInPicture, event_loop),
            "dark" => self.handle_ipc(win_id, None, M::ToggleDarkWebsites, event_loop),
            "favourite" => self.handle_ipc(win_id, None, M::ToggleFavourite, event_loop),
            "settings" => self.open_internal(win_id, "settings"),
            "passwords" => self.open_internal(win_id, "passwords"),
            "history" => self.handle_command(win_id, Command::History, event_loop),
            "clear-data" => self.handle_command(win_id, Command::ClearData, event_loop),
            "darkpanel" => self.toggle_sidebar(win_id, "dark"),
            "view-source" => self.handle_command(win_id, Command::ViewSource, event_loop),
            "save-page" => self.handle_command(win_id, Command::SavePage, event_loop),
            "fullscreen" => self.handle_command(win_id, Command::FullScreen, event_loop),
            "wish" => self.open_internal(win_id, "wish"),
            "tea" => self.open_internal(win_id, "tea"),
            "koan" => self.open_internal(win_id, "koan"),
            "fold" => self.toast(&serde_json::json!({
                "title": "Folded.",
                "text": "One square, a few creases, and it flies.",
                "pose": "unfold",
            })),
            "devtools" => self.handle_ipc(win_id, None, M::OpenDevTools, event_loop),
            "split" => self.split_with(win_id, tab),
            "split-exit" => self.split_with(win_id, None),
            "split-newtab" => self.add_new_pane(win_id, None),
            "split-toggle" => self.handle_ipc(win_id, None, M::ToggleSplit, event_loop),
            "split-orient" => self.handle_ipc(win_id, None, M::ToggleSplitOrientation, event_loop),
            "split-focus" => self.handle_ipc(win_id, None, M::SwitchSplitPane, event_loop),
            "split-reset" => self.handle_ipc(win_id, None, M::ResetSplitRatio, event_loop),
            "split-close" => {
                if let Ok(id) = a.parse::<u32>() {
                    self.handle_ipc(win_id, None, M::SplitClose { id }, event_loop);
                }
            }
            "split-swap" => {
                if let Ok(id) = a.parse::<u32>() {
                    self.handle_ipc(win_id, None, M::SplitSwapPanes { id }, event_loop);
                }
            }
            "split-rename" => {
                if let Ok(id) = a.parse::<u32>() {
                    let current = self
                        .windows
                        .get(&win_id)
                        .and_then(|w| w.tabs.splits().iter().find(|s| s.id == id))
                        .map(|s| s.name.clone())
                        .unwrap_or_default();
                    self.open_prompt(
                        win_id,
                        Prompt::RenameSplit(id),
                        "Rename split group",
                        &current,
                    );
                }
            }
            "split-color" => {
                if let Ok(id) = a.parse::<u32>() {
                    let next = self
                        .windows
                        .get(&win_id)
                        .and_then(|w| w.tabs.splits().iter().find(|s| s.id == id))
                        .map(|s| {
                            let i = GROUP_COLORS.iter().position(|c| *c == s.color).unwrap_or(0);
                            GROUP_COLORS[(i + 1) % GROUP_COLORS.len()].to_string()
                        });
                    if let Some(color) = next {
                        self.handle_ipc(win_id, None, M::SplitSetColor { id, color }, event_loop);
                    }
                }
            }
            "mute" => {
                if let Some(t) = tab {
                    self.handle_ipc(win_id, None, M::ToggleMuteTab { tab_id: t }, event_loop);
                }
            }
            "ws-new" => self.open_prompt(win_id, Prompt::NewWorkspace, "Name this workspace", ""),
            "ws-switch" => {
                if let Ok(ws_id) = a.parse::<u32>() {
                    self.switch_workspace(win_id, ws_id);
                }
            }
            "ws-rename" => {
                if let Ok(ws_id) = a.parse::<u32>() {
                    let current = self
                        .windows
                        .get(&win_id)
                        .and_then(|w| w.tabs.workspaces().iter().find(|ws| ws.id == ws_id))
                        .map(|ws| ws.name.clone())
                        .unwrap_or_default();
                    self.open_prompt(
                        win_id,
                        Prompt::RenameWorkspace(ws_id),
                        "Rename workspace",
                        &current,
                    );
                }
            }
            "ws-color" => {
                if let (Ok(ws_id), Some(win)) = (a.parse::<u32>(), self.windows.get_mut(&win_id)) {
                    let current = win
                        .tabs
                        .workspaces()
                        .iter()
                        .find(|ws| ws.id == ws_id)
                        .map(|ws| ws.color.clone());
                    if let Some(current) = current {
                        let i = GROUP_COLORS.iter().position(|c| *c == current).unwrap_or(0);
                        win.tabs
                            .set_workspace_color(ws_id, GROUP_COLORS[(i + 1) % GROUP_COLORS.len()]);
                        win.ui_dirty = true;
                        win.sidebar_dirty = true;
                    }
                }
            }
            "ws-reorder" => {
                if let (Ok(from), Ok(to), Some(win)) = (
                    a.parse::<usize>(),
                    b.parse::<usize>(),
                    self.windows.get_mut(&win_id),
                ) {
                    if win.tabs.reorder_workspace(from, to) {
                        win.ui_dirty = true;
                        win.sidebar_dirty = true;
                    }
                }
            }
            "ws-delete" => {
                if let Ok(ws_id) = a.parse::<u32>() {
                    self.delete_workspace(win_id, ws_id);
                }
            }
            "split-remove" => {
                if let Some(t) = tab {
                    self.handle_ipc(win_id, None, M::SplitRemove { tab_id: t }, event_loop);
                }
            }
            "tab-new-space" => {
                // A new space that starts with this tab; the window stays where it is.
                let created = self.windows.get_mut(&win_id).and_then(|win| {
                    let color = next_workspace_color(win).to_string();
                    let name = format!("Space {}", win.tabs.workspaces().len() + 1);
                    tab.map(|t| (t, win.tabs.create_workspace(&name, &color)))
                });
                if let Some((t, ws)) = created {
                    self.move_tab_to_workspace(win_id, t, ws);
                }
            }
            "ws-move" => {
                if let (Some(t), Ok(ws)) = (tab, b.parse::<u32>()) {
                    self.move_tab_to_workspace(win_id, t, ws);
                }
            }
            "group-new" => {
                if let Some(t) = tab {
                    self.open_prompt(win_id, Prompt::NewGroup(t), "Name this group", "");
                }
            }
            "group-set" => {
                let gid = b.parse::<u32>().ok();
                if let (Some(t), Some(gid), Some(win)) = (tab, gid, self.windows.get_mut(&win_id)) {
                    if win.tabs.set_tab_group(t, Some(gid)) {
                        gather_group(win, t, gid);
                        win.ui_dirty = true;
                    }
                }
            }
            "group-none" => {
                if let (Some(t), Some(win)) = (tab, self.windows.get_mut(&win_id)) {
                    win.tabs.set_tab_group(t, None);
                    win.ui_dirty = true;
                }
            }
            "group-rename" => {
                if let Ok(gid) = a.parse::<u32>() {
                    let current = self
                        .windows
                        .get(&win_id)
                        .and_then(|w| w.tabs.groups().iter().find(|g| g.id == gid))
                        .map(|g| g.name.clone())
                        .unwrap_or_default();
                    self.open_prompt(win_id, Prompt::RenameGroup(gid), "Rename group", &current);
                }
            }
            "group-color" => {
                if let (Ok(gid), Some(win)) = (a.parse::<u32>(), self.windows.get_mut(&win_id)) {
                    let current = win
                        .tabs
                        .groups()
                        .iter()
                        .find(|g| g.id == gid)
                        .map(|g| g.color.clone());
                    if let Some(current) = current {
                        let i = GROUP_COLORS.iter().position(|c| *c == current).unwrap_or(0);
                        win.tabs
                            .set_group_color(gid, GROUP_COLORS[(i + 1) % GROUP_COLORS.len()]);
                        win.ui_dirty = true;
                    }
                }
            }
            "fav" => {
                let page =
                    tab.and_then(|t| self.windows.get(&win_id)?.tabs.get_tab_by_id(t).cloned());
                if let Some(t) = page {
                    if self.favourites.contains(&t.url) {
                        self.favourites.remove(&t.url);
                        self.push_data();
                    } else {
                        self.bookmark_page(win_id, &t.url, &t.title);
                    }
                }
            }
            "ctr" => {
                if let Some(t) = tab {
                    let container = match b {
                        "" => None,
                        "temp" => Some(self.new_temp_container()),
                        id if self.settings.containers.get(id).is_some() => Some(id.to_string()),
                        _ => return,
                    };
                    self.reopen_in_container(win_id, t, None, container);
                }
            }
            "ctr-always" | "ctr-never" => {
                let Some(state) = tab.and_then(|t| {
                    self.windows
                        .get(&win_id)
                        .and_then(|w| w.tabs.get_tab_by_id(t).cloned())
                }) else {
                    return;
                };
                let Some(host) = extract_host(&state.url) else {
                    return;
                };
                let site = shoji_core::settings::extract_domain(&host);
                let rule = (name == "ctr-always")
                    .then_some(state.container)
                    .flatten()
                    .filter(|c| !is_temp_container(c));
                self.update_settings(|s| s.containers.set_rule(&site, rule.as_deref()));
            }
            "pin" => {
                if let (Some(t), Some(win)) = (tab, self.windows.get_mut(&win_id)) {
                    let pinned = win.tabs.get_tab_by_id(t).is_some_and(|s| !s.pinned);
                    if win.tabs.set_pinned(t, pinned) {
                        win.ui_dirty = true;
                    }
                }
            }
            "fav-folder" => {
                let page =
                    tab.and_then(|t| self.windows.get(&win_id)?.tabs.get_tab_by_id(t).cloned());
                if let Some(t) = page {
                    self.ask_favourite_folder(win_id, &t.url);
                }
            }
            "reload" => {
                if let Some(t) = tab {
                    self.handle_ipc(win_id, None, M::ReloadTab { tab_id: t }, event_loop);
                }
            }
            "close" => {
                if let Some(t) = tab {
                    self.close_tab(win_id, t, event_loop);
                }
            }
            _ => {
                self.extra_action(win_id, name, event_loop);
            }
        }
    }

    /// Ask which tab should sit beside the active one. Split groups are only made on purpose.
    fn open_split_picker(&mut self, win_id: WindowId) {
        use shoji_core::ipc::OverlayItem;
        let Some(win) = self.windows.get(&win_id) else {
            return;
        };
        let active = win.tabs.active_tab_id();
        let ws = win.tabs.active_workspace_id();
        let mut items: Vec<OverlayItem> = win
            .tabs
            .tabs()
            .iter()
            .filter(|t| {
                Some(t.id) != active
                    && t.workspace == ws
                    && !win
                        .tabs
                        .visible_split()
                        .is_some_and(|s| s.panes.contains(&t.id))
            })
            .map(|t| OverlayItem {
                kind: "action".to_string(),
                value: format!("split:{}", t.id.0),
                label: if t.title.is_empty() {
                    t.url.clone()
                } else {
                    t.title.clone()
                },
                sub: t.url.clone(),
                color: None,
            })
            .collect();
        items.push(OverlayItem {
            kind: "action".to_string(),
            value: "split-newtab".to_string(),
            label: "A new tab beside this one".to_string(),
            sub: String::new(),
            color: None,
        });
        let w = (win.width - 40.0).clamp(300.0, 560.0);
        let top = win.chrome_height() + 12.0;
        let h = (win.height - top - 24.0).clamp(160.0, 460.0);
        let x = ((win.width - w) / 2.0).max(4.0);
        // An address bar of its own: open tabs first, then what to open in a new pane. The engine's
        // name rides along (as the field's value) so the window can show its logo.
        let engine = self.settings.search_engine_display_name();
        self.show_overlay(
            win_id,
            "paneadd",
            items,
            [x, top, w, h],
            "",
            &format!("Search {engine} or type a URL to add"),
            engine,
        );
    }

    /// Show `other` beside the active tab, or leave split view when `None`.
    fn split_with(&mut self, win_id: WindowId, other: Option<TabId>) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        match other {
            None => {
                win.tabs.close_visible_split();
            }
            Some(o) => {
                let Some(active) = win.tabs.active_tab_id() else {
                    return;
                };
                if !win.views.contains_key(&o) {
                    return;
                }
                // Already in a split: the tab joins it. Otherwise the two make a new one.
                let joined = match win.tabs.split_of(active).map(|s| s.id) {
                    Some(group) => win.tabs.add_to_split(group, o, None),
                    None => {
                        let color = next_split_color(win);
                        win.tabs.open_split_with(active, o, "", color).is_some()
                    }
                };
                if !joined {
                    return;
                }
            }
        }
        if let Some(active) = win.tabs.active_tab_id() {
            win.show_tab(active);
        }
        win.layout();
        win.ui_dirty = true;
        win.sidebar_dirty = true;
    }

    /// Switch `win_id` to `ws_id`, suspending the outgoing workspace's tabs and restoring the
    /// incoming workspace's active tab (and split view if any).
    fn switch_workspace(&mut self, win_id: WindowId, ws_id: u32) {
        let switched = self.windows.get_mut(&win_id).is_some_and(|win| {
            win.fullscreen_tab = None;
            win.tabs.switch_workspace(ws_id, now_secs())
        });
        if switched {
            self.sync_window_views(win_id);
        }
    }

    fn delete_workspace(&mut self, win_id: WindowId, ws_id: u32) {
        let deleted = self
            .windows
            .get_mut(&win_id)
            .is_some_and(|win| win.tabs.delete_workspace(ws_id, now_secs()));
        if deleted {
            self.sync_window_views(win_id);
            self.push_data();
        }
    }

    fn move_tab_to_workspace(&mut self, win_id: WindowId, tab_id: TabId, target_ws: u32) {
        let moved = self.windows.get_mut(&win_id).is_some_and(|win| {
            win.tabs
                .move_tab_to_workspace(tab_id, target_ws, now_secs())
        });
        if moved {
            self.sync_window_views(win_id);
        }
    }

    fn toggle_reader(&mut self, win_id: WindowId) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        let Some(state) = win.tabs.active_tab().cloned() else {
            return;
        };
        let Some(tab) = win.views.get(&state.id) else {
            return;
        };
        if win.reader.remove(&state.id) {
            // Back to the article itself.
            let _ = tab.view.load_url(&state.url);
            win.ui_dirty = true;
            return;
        }
        if !(state.url.starts_with("http://") || state.url.starts_with("https://")) {
            return;
        }
        let proxy = self.proxy.clone();
        let id = state.id;
        let _ = tab
            .view
            .evaluate_script_with_callback(pages::READER_SCRIPT, move |json| {
                let _ = proxy.send_event(BrowserEvent::ReaderReady { tab_id: id, json });
            });
    }

    /* ----- notes ----- */

    fn open_notes(&mut self, win_id: WindowId) {
        let notes_json = self.notebook.to_json();
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        if win.notes.is_none() {
            let proxy = self.proxy.clone();
            let dark = window::is_dark(&win.window, &self.settings.appearance.theme);
            win.notes = new_builder()
                .with_bounds(frame_rect(win.notes_desk()))
                // The page can't be scripted before it loads, so the first open is handed over here.
                .with_initialization_script(format!(
                    "window.__notesInit = () => {};",
                    notes_layout_js("open", win.notes_sheet(), NOTES_OPEN_MS)
                ))
                .with_html(pages::notes(&self.settings, &notes_json))
                .with_custom_protocol("shoji".into(), |_, request| font_response(&request))
                .with_incognito(true)
                // Pre-painted in the paper colour, so the first unroll never shows a blank rectangle.
                .with_background_color(if dark {
                    (35, 34, 31, 255)
                } else {
                    (251, 249, 243, 255)
                })
                .with_theme(window::webview_theme(&self.settings.appearance.theme))
                .with_ipc_handler(move |req: wry::http::Request<String>| {
                    if let Ok(msg) = serde_json::from_str(req.body()) {
                        let _ = proxy.send_event(BrowserEvent::WindowIpc(win_id, msg));
                    }
                })
                .build_as_child(win.window.as_ref())
                .ok();
            if let Some(n) = &win.notes {
                webview::attach_accelerators(n, Origin::Window(win_id), self.proxy.clone());
            }
        } else if let Some(n) = &win.notes {
            let _ = n.evaluate_script(&format!(
                "window.__notesOpen && window.__notesOpen({notes_json})"
            ));
        }
        win.notes_open = true;
        let sheet = win.notes_sheet();
        // Unroll downward from the top edge, unless it is already partly open (reopened mid-close).
        let from = win
            .notes_clip
            .unwrap_or([sheet[0], sheet[1], sheet[2], 1.0]);
        win.set_notes_clip(from);
        if let Some(n) = &win.notes {
            let _ = n.evaluate_script(&notes_layout_js("open", sheet, NOTES_OPEN_MS));
            let _ = n.set_bounds(frame_rect(win.notes_desk()));
            // A brand-new view shows black until its first frame; `NotesReady` reveals it.
            let _ = n.set_visible(win.notes_ready);
            webview::bring_to_top(n);
            // Keystrokes go straight onto the paper.
            webview::focus(n);
        }
        // On the very first open the unroll waits for the page (see `NotesReady`).
        if win.notes_ready {
            win.animate_notes(from, sheet, NOTES_OPEN_MS, false);
        }
        win.eval_chrome("window.__syncNotes && window.__syncNotes(true)");
    }

    fn set_notes_expanded(&mut self, win_id: WindowId, expanded: bool) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        if !win.notes_open || win.notes_expanded == expanded {
            return;
        }
        win.notes_expanded = expanded;
        let sheet = win.notes_sheet();
        if let Some(n) = &win.notes {
            let _ = n.evaluate_script(&notes_layout_js("resize", sheet, NOTES_RESIZE_MS));
        }
        // The page eases the sheet with the same curve and duration; the clip follows it.
        let from = win.notes_clip.unwrap_or(sheet);
        win.animate_notes(from, sheet, NOTES_RESIZE_MS, false);
    }

    fn close_notes(&mut self, win_id: WindowId) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        if !win.notes_open {
            return;
        }
        win.notes_open = false;
        // Roll up toward the top edge, then hide.
        if let (Some(n), Some(from)) = (&win.notes, win.notes_clip) {
            let _ = n.evaluate_script(&notes_layout_js("close", win.notes_sheet(), NOTES_CLOSE_MS));
            win.animate_notes(from, [from[0], from[1], from[2], 1.0], NOTES_CLOSE_MS, true);
        }
        win.eval_chrome("window.__syncNotes && window.__syncNotes(false)");
        if let Some(tab) = win.active() {
            webview::focus(&tab.view);
        }
    }

    /* ----- side panel ----- */

    fn toggle_sidebar(&mut self, win_id: WindowId, mode: &'static str) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        if win.sidebar_open && win.sidebar_mode == mode {
            // Pressing the button of a dropdown that opened by hovering changes nothing: it
            // still folds away when the pointer leaves.
            if win.pop_hover {
                return;
            }
            return self.close_sidebar(win_id, true);
        }
        // Clicking the button that opened a dropdown first moves focus away from it, which
        // already closed it; that same click must not open it again.
        if let Some((at, was)) = win.pop_blurred.take() {
            if was == mode && at.elapsed() < Duration::from_millis(350) {
                return;
            }
        }
        self.open_sidebar(win_id, mode, None);
    }

    /// Build the dropdown's page (hidden). Done ahead of time, a moment after a window opens, so
    /// the first opening does not wait for a web view to start.
    fn ensure_sidebar(&mut self, win_id: WindowId) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        if win.sidebar.is_some() {
            return;
        }
        let proxy = self.proxy.clone();
        let dark = window::is_dark(&win.window, &self.settings.appearance.theme);
        win.sidebar = new_builder()
            .with_bounds(bounds(0.0, 0.0, SIDEBAR_WIDTH, 1.0))
            .with_visible(false)
            // Built in the background: it must not take the keyboard from the page.
            .with_focused(false)
            .with_html(pages::sidebar(&self.settings))
            .with_custom_protocol("shoji".into(), |_, request| font_response(&request))
            .with_incognito(true)
            .with_background_color(window::page_rgba(dark))
            .with_theme(window::webview_theme(&self.settings.appearance.theme))
            .with_ipc_handler(move |req: wry::http::Request<String>| {
                if let Ok(msg) = serde_json::from_str(req.body()) {
                    let _ = proxy.send_event(BrowserEvent::WindowIpc(win_id, msg));
                }
            })
            .build_as_child(win.window.as_ref())
            .ok();
        if let Some(s) = &win.sidebar {
            webview::attach_accelerators(s, Origin::Window(win_id), self.proxy.clone());
        }
    }

    /// Build the dropdowns that are due, and say when the next one is.
    fn warm_sidebars(&mut self) -> Option<Instant> {
        let now = Instant::now();
        let mut due: Option<Instant> = None;
        let mut ready = Vec::new();
        for (id, win) in &self.windows {
            match win.sidebar_warm {
                Some(t) if t <= now => ready.push(*id),
                Some(t) => due = due.into_iter().chain([t]).min(),
                None => {}
            }
        }
        for id in ready {
            if let Some(win) = self.windows.get_mut(&id) {
                win.sidebar_warm = None;
            }
            self.ensure_sidebar(id);
        }
        due
    }

    /// Open a dropdown. `hover` is `(left, right, in_bar)` of the trigger when the pointer opened
    /// it: such a dropdown does not take the keyboard, and folds away when the pointer leaves.
    fn open_sidebar(
        &mut self,
        win_id: WindowId,
        mode: &'static str,
        hover: Option<(f64, f64, bool)>,
    ) {
        self.ensure_sidebar(win_id);
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        // The dropdowns float over the page; none of them ever takes page width.
        win.sidebar_docked = false;
        win.sidebar_open = true;
        win.sidebar_mode = mode;
        win.pop_hide_at = None;
        win.pop_opened = Some(Instant::now());
        win.pop_hover = hover.is_some();
        win.pop_span = hover.map(|(l, r, _)| (l, r));
        win.pop_top = hover.filter(|h| h.2).map(|_| win.chrome_height() + 2.0);
        win.hover_out_since = None;
        win.pop_pointer_in = false;
        win.sidebar_dirty = true;
        win.layout();
        let [x, _, w, _] = win.popover_frame();
        let caret = (win.pop_cx() - x).clamp(18.0, w - 18.0);
        let js = format!(
            "window.__popOpen && window.__popOpen({{mode:'{mode}',caret:{caret},docked:false,hover:{}}})",
            hover.is_some()
        );
        win.sidebar_pending = Some(js.clone());
        if let Some(s) = &win.sidebar {
            let _ = s.set_visible(win.sidebar_ready);
            webview::bring_to_top(s);
            let _ = s.evaluate_script(&js);
            if hover.is_none() {
                webview::focus(s);
            }
        }
        win.sync_zoom();
        if mode == "downloads" {
            self.push_downloads();
        }
    }

    /// Send the saved downloads list to the dropdown and any open downloads page.
    /// Fold one more paper crane. On the way to a thousand the crane says a quiet word at
    /// 100, 250 and 500; at the thousandth (and each after) it offers a wish.
    fn fold_crane(&mut self) {
        let before = self.cranes.count();
        let crossed = self.cranes.fold(1);
        if let Ok(mut ctx) = self.pages.write() {
            ctx.cranes = self.cranes.count();
        }
        let milestone = shoji_core::userdata::crane_milestone(before, self.cranes.count());
        if let (Some(m), false) = (milestone, crossed) {
            let (title, text) = match m {
                100 => ("A hundred cranes", "Nine hundred to go. No hurry."),
                250 => ("A quarter of the way", "250 cranes hang on their strings."),
                _ => (
                    "Halfway to a thousand",
                    "500 cranes. The strings are getting full.",
                ),
            };
            self.toast(&serde_json::json!({
                "title": title,
                "text": text,
                "pose": "cheer",
                "ms": 5200,
                "actions": [{"label": "See them", "action": "OpenPage", "payload": {"page": "wish"}}],
            }));
        } else if crossed && self.cranes.count() > shoji_core::userdata::THOUSAND {
            self.toast(&serde_json::json!({
                "title": "Another thousand cranes",
                "text": format!("{} folded. There is room for another wish.", self.cranes.count()),
                "pose": "cheer",
                "ms": 8000,
                "actions": [{"label": "Make a wish", "action": "OpenPage", "payload": {"page": "wish"}, "primary": true}],
            }));
        } else if crossed {
            let toast = serde_json::json!({
                "title": "A thousand cranes",
                "text": "Senbazuru is complete. Make a wish at shoji://wish",
                "pose": "cheer",
                "ms": 9000,
                "actions": [{"label": "Make a wish", "action": "OpenPage", "payload": {"page": "wish"}, "primary": true}],
            });
            self.toast(&toast);
        }
    }

    /// Delete the cookies and storage of sites that only ever appeared as redirect hops (see
    /// `shoji_core::bounce`). Needs a tab in the main cookie jar to reach it through; sites open
    /// in a tab are left alone.
    fn clean_bounce_trackers(&mut self) {
        let open: HashSet<String> = self
            .windows
            .values()
            .flat_map(|w| w.tabs.all_tabs())
            .filter_map(|t| webview::site_of(&t.url).map(|s| s.0))
            .collect();
        let due = self.bounce.due(now_secs());
        let Some(view) = self
            .windows
            .values()
            .flat_map(|w| w.views.values())
            .find(|t| t.persistent && t.container.is_none())
            .map(|t| &t.view)
        else {
            self.next_bounce_clean = Instant::now() + Duration::from_secs(600);
            return;
        };
        for (site, hosts) in due.into_iter().filter(|(s, _)| !open.contains(s)) {
            for host in &hosts {
                for scheme in ["https", "http"] {
                    let origin = format!("{scheme}://{host}");
                    let params = serde_json::json!({
                        "origin": origin,
                        "storageTypes": "local_storage,indexeddb,cache_storage,service_workers,file_systems,websql",
                    })
                    .to_string();
                    webview::cdp(view, "Storage.clearDataForOrigin", &params);
                    webview::site_cookies(view, &format!("{origin}/"), true, |_| {});
                }
            }
            self.bounce.forget(&site);
        }
        self.bounce.save(&self.data_dir.join("bounce.json"));
    }

    /// Give the UI watchdog a window of this thread to ping.
    fn note_ui_window(&self) {
        let Some(win) = self.windows.values().next() else {
            return;
        };
        if let Ok(handle) =
            winit::raw_window_handle::HasWindowHandle::window_handle(win.window.as_ref())
        {
            if let winit::raw_window_handle::RawWindowHandle::Win32(h) = handle.as_raw() {
                crash::set_ui_window(h.hwnd.get());
            }
        }
    }

    /// If Shoji crashed since it last said so, offer to open a prefilled bug report.
    fn mention_crash(&self) {
        let reports = crash::unseen_reports(&self.data_dir);
        let Some(newest) = reports.first() else {
            return;
        };
        crash::mark_seen(&self.data_dir);
        let report = std::fs::read_to_string(newest).unwrap_or_default();
        self.toast(&serde_json::json!({
            "title": "Shoji closed unexpectedly",
            "text": "A report was saved on this PC. Nothing is sent unless you open it and submit it.",
            "pose": "bow",
            "ms": 12000,
            "actions": [{
                "label": "Report on GitHub",
                "action": "CreateTab",
                "payload": {"url": crash::issue_url(&report)},
                "primary": true,
            }],
        }));
    }

    /// Show the crane's card in the focused window (or every window if none has focus).
    fn toast(&self, toast: &serde_json::Value) {
        let js = format!("window.__toast && window.__toast({toast})");
        let focused: Vec<&BrowserWindow> = self
            .windows
            .values()
            .filter(|w| w.window.has_focus())
            .collect();
        if focused.is_empty() {
            for win in self.windows.values() {
                win.eval_chrome(&js);
            }
        } else {
            for win in focused {
                win.eval_chrome(&js);
            }
        }
    }

    fn push_downloads(&mut self) {
        let json = serde_json::to_string(self.downloads.items()).unwrap_or_else(|_| "[]".into());
        let js = format!("window.__downloadsList && window.__downloadsList({json})");
        for win in self.windows.values() {
            win.eval_sidebar(&js);
        }
        self.eval_all_internal(&js);
    }

    /// Close the dropdown (or pinned panel). `refocus` hands the keyboard back to the page, as
    /// when the user pressed Esc; a click elsewhere has already given focus to what it hit.
    fn close_sidebar(&mut self, win_id: WindowId, refocus: bool) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        if !win.sidebar_open {
            return;
        }
        win.sidebar_open = false;
        win.pop_hover = false;
        if win.sidebar_docked {
            if let Some(s) = &win.sidebar {
                let _ = s.set_visible(false);
            }
            win.layout();
        } else {
            // Let the page fold the dropdown away, then hide it.
            win.eval_sidebar("window.__popClose && window.__popClose()");
            win.pop_hide_at = Some(Instant::now() + Duration::from_millis(POP_CLOSE_MS));
        }
        if refocus {
            if let Some(tab) = win.active() {
                webview::focus(&tab.view);
            }
        }
    }

    /// Fold away a dropdown that opened by hovering once the pointer has been off it, and off the
    /// button that opened it, for a moment. The cursor is read from the system rather than from
    /// the pages, so it works wherever the pointer goes. Returns when to look again.
    fn step_hover(&mut self) -> Option<Instant> {
        if !self.windows.values().any(|w| w.sidebar_open && w.pop_hover) {
            return None;
        }
        let (cx, cy) = tabdrag::cursor_pos();
        let mut next: Option<Instant> = None;
        let mut close = Vec::new();
        for (id, win) in self.windows.iter_mut() {
            if !(win.sidebar_open && win.pop_hover) {
                win.hover_out_since = None;
                continue;
            }
            // A menu opened from the dropdown makes it a deliberate one.
            if win.overlay_open {
                win.pop_hover = false;
                win.hover_out_since = None;
                continue;
            }
            let Ok(origin) = win.window.inner_position() else {
                continue;
            };
            let scale = win.window.scale_factor();
            let (x, y) = (
                (cx - origin.x) as f64 / scale,
                (cy - origin.y) as f64 / scale,
            );
            let f = win.popover_frame();
            let pad = 6.0;
            let on_list = x >= f[0] - pad
                && x <= f[0] + f[2] + pad
                && y >= f[1] - pad
                && y <= f[1] + f[3] + pad;
            // The trigger, and the strip between it and the list, so the pointer can travel.
            let (l, r) = win.pop_span.unwrap_or((f[0], f[0] + f[2]));
            let on_trigger = x >= l - 6.0 && x <= r + 6.0 && y >= 0.0 && y <= f[1];
            // Resting on the list (which reports leaving it): check now and then in case a leave
            // went unreported; in transit or outside, check every frame-ish.
            let pace = if on_list && win.pop_pointer_in {
                250
            } else {
                25
            };
            if on_list || on_trigger {
                win.hover_out_since = None;
            } else {
                let since = *win.hover_out_since.get_or_insert_with(Instant::now);
                if since.elapsed() >= Duration::from_millis(HOVER_LEAVE_MS) {
                    close.push(*id);
                    continue;
                }
            }
            next = next
                .into_iter()
                .chain([Instant::now() + Duration::from_millis(pace)])
                .min();
        }
        for id in close {
            self.close_sidebar(id, false);
        }
        next
    }

    /* ----- settings & appearance ----- */

    fn save_settings(&self) {
        if let Err(e) = self.settings.save_to_path(&self.settings_path) {
            eprintln!("Failed to save settings: {e}");
        }
        if let Ok(mut ctx) = self.pages.write() {
            ctx.settings = self.settings.clone();
        }
    }

    /// Apply a settings change and push its effects to every open surface.
    fn update_settings(&mut self, change: impl FnOnce(&mut Settings)) {
        let before = self.settings.clone();
        change(&mut self.settings);
        if self.settings == before {
            return;
        }
        if let Ok(mut ctx) = self.pages.write() {
            ctx.settings = self.settings.clone();
        }
        let s = &self.settings;

        if s.appearance.browser_theme != before.appearance.browser_theme {
            let look = pages::browser_look(s);
            let (frame, page) = look.frame_and_page();
            window::set_dark_colors(frame, page);
            for win in self.windows.values_mut() {
                window::apply_frame(
                    &win.window,
                    window::is_dark(&win.window, &s.appearance.theme),
                );
                win.sidebar_dirty = true;
            }
        }
        if s.accent() != before.accent()
            || s.appearance.theme != before.appearance.theme
            || s.fonts() != before.fonts()
            || s.appearance.browser_theme != before.appearance.browser_theme
        {
            let js = pages::apply_theme_js(s);
            for win in self.windows.values() {
                win.eval_chrome(&js);
                win.eval_sidebar(&js);
                win.eval_internal_tabs(&js);
                if let Some(n) = &win.notes {
                    let _ = n.evaluate_script(&js);
                }
                if let Some(o) = &win.overlay {
                    let _ = o.evaluate_script(&js);
                }
            }
        }
        if s.fonts() != before.fonts()
            || s.appearance.fonts_on_websites != before.appearance.fonts_on_websites
        {
            let script = pages::font_document_script(s);
            for win in self.windows.values_mut() {
                for (id, tab) in &win.views {
                    webview::set_document_script(&tab.view, &tab.font_script, script.as_deref());
                    if !is_internal(win.tabs.get_tab_by_id(*id).map_or("", |t| &t.url)) {
                        let _ = tab
                            .view
                            .evaluate_script(script.as_deref().unwrap_or(pages::FONT_REMOVE_JS));
                    }
                }
            }
        }
        if s.appearance.dark_websites != before.appearance.dark_websites
            || s.appearance.dark_websites_exclude != before.appearance.dark_websites_exclude
            || s.appearance.dark_websites_include != before.appearance.dark_websites_include
            || s.appearance.dark_theme != before.appearance.dark_theme
            || s.appearance.dark_site_themes != before.appearance.dark_site_themes
            || s.appearance.dark_schedule != before.appearance.dark_schedule
            || s.appearance.dark_detect_native != before.appearance.dark_detect_native
        {
            // Restyle open pages in place, the ones on screen first so they change at once; the
            // per-document script for future pages is swapped afterwards.
            darkfixes::set_enabled(s.dark_needed());
            let script = pages::dark_document_script(s);
            let apply = pages::dark_apply_js(s);
            let skip = format!(
                "window.__setDarkSkip && window.__setDarkSkip({})",
                serde_json::to_string(&s.appearance.dark_websites_exclude).unwrap_or_default()
            );
            // A page opened while dark mode was off has no site fix yet: hand it one first.
            let restyle = |win: &BrowserWindow, id: &TabId, tab: &Tab| {
                let fix = win
                    .tabs
                    .get_tab_by_id(*id)
                    .and_then(|t| darkfixes::script_for(&t.url))
                    .unwrap_or_default();
                let light = !s.appearance.dark_schedule.enabled
                    && win.tabs.get_tab_by_id(*id).is_some_and(|t| {
                        !is_internal(&t.url) && extract_host(&t.url).is_some_and(|h| s.dark_on(&h))
                    });
                darkfixes::emulate(&tab.view, &tab.emulated, light);
                let _ = tab.view.evaluate_script(&format!("{fix}{apply}"));
            };
            for win in self.windows.values() {
                for (id, tab) in &win.views {
                    if win.tabs.is_tab_visible(*id) {
                        restyle(win, id, tab);
                    }
                }
            }
            for win in self.windows.values_mut() {
                for (id, tab) in &win.views {
                    if !win.tabs.is_tab_visible(*id) {
                        restyle(win, id, tab);
                    }
                    webview::set_document_script(&tab.view, &tab.dark_script, script.as_deref());
                }
                win.eval_internal_tabs(&skip);
                win.sidebar_dirty = true;
            }
        }
        if s.accent() != before.accent() {
            for win in self.windows.values() {
                window::set_icons(&win.window, s.accent());
            }
            icons::retint_shortcuts(self.data_dir.clone(), s.accent());
            if let Some(tray) = &self.tray {
                app_set_tray_icon(tray, s.accent());
            }
        }
        if s.shield != before.shield {
            shield::apply_settings(&s.shield);
            let hook = shield::wants_request_hook();
            for win in self.windows.values_mut() {
                for tab in win.views.values_mut() {
                    if tab.request_hook != hook {
                        tab.request_hook = hook;
                        shield::set_request_hook(&tab.view, hook);
                    }
                }
            }
        }
        if tracking_level(s) != tracking_level(&before) {
            let level = tracking_level(s);
            for win in self.windows.values() {
                for tab in win.views.values() {
                    webview::set_tracking_prevention(&tab.view, level);
                }
            }
        }
        if s.appearance.theme != before.appearance.theme {
            let pref = s.appearance.theme.clone();
            for win in self.windows.values() {
                win.window.set_theme(window::forced_theme(&pref));
                let theme = window::webview_theme(&pref);
                for wv in win
                    .chrome
                    .iter()
                    .chain(&win.sidebar)
                    .chain(&win.notes)
                    .chain(win.views.values().map(|t| &t.view))
                {
                    let _ = wv.set_theme(theme);
                }
                window::apply_frame(&win.window, window::is_dark(&win.window, &pref));
            }
        }
        if s.search_engine != before.search_engine {
            let js = format!(
                "window.__syncSearchEngine && window.__syncSearchEngine({})",
                js_string(s.search_engine_display_name())
            );
            for win in self.windows.values() {
                win.eval_chrome(&js);
            }
        }
        let prefs = pages::prefs_json(s);
        if prefs != pages::prefs_json(&before) {
            let js = format!("window.__applyPrefs && window.__applyPrefs({prefs})");
            for win in self.windows.values() {
                win.eval_chrome(&js);
            }
        }
        if s.downloads != before.downloads {
            webview::set_download_prefs(
                s.downloads.ask_where_to_save,
                s.downloads.default_folder.clone(),
            );
        }
        if self.settings.privacy != before.privacy {
            if self.settings.privacy.ephemeral_default {
                // Private mode keeps no list of open tabs.
                let _ = std::fs::remove_file(&self.session_path);
                self.session_json.clear();
                self.session_dirty = true;
            }
            // Rebuild each window's active tab in the right storage partition straight away.
            // Background tabs follow when they next navigate or reload.
            let stale: Vec<(WindowId, String)> = self
                .windows
                .iter()
                .filter_map(|(id, win)| {
                    let tab = win.tabs.active_tab()?;
                    let wants = !is_internal(&tab.url)
                        && !win.private
                        && self.settings.is_site_persistent(&tab.url);
                    let has = win.views.get(&tab.id)?.persistent;
                    (wants != has && !is_internal(&tab.url)).then(|| (*id, tab.url.clone()))
                })
                .collect();
            for (id, url) in stale {
                self.navigate(id, &url);
            }
        }
        if let Err(e) = self.settings.save_to_path(&self.settings_path) {
            eprintln!("Failed to save settings: {e}");
        }
    }

    /* ----- events ----- */

    fn handle_ipc(
        &mut self,
        win_id: WindowId,
        from_tab: Option<TabId>,
        msg: UiToHostMessage,
        event_loop: &ActiveEventLoop,
    ) {
        use UiToHostMessage as M;
        match msg {
            M::ChromeReady => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.last_ui_json.clear();
                    win.last_page_sync.clear();
                    win.last_fav_sync.clear();
                    win.frame_state = None;
                    win.ui_dirty = true;
                    win.sync_zoom();
                    win.sync_frame();
                    win.focus_address = true;
                    if win.sidebar.is_none() && win.sidebar_warm.is_none() {
                        win.sidebar_warm = Some(Instant::now() + Duration::from_millis(900));
                    }
                }
                self.push_data();
                self.push_update();
                if self.settings.first_run_complete {
                    self.greet_after_update();
                    self.mention_crash();
                }
            }
            M::MinimizeWindow | M::ToggleMaximize | M::StartWindowResize => {
                let Some(win) = self.windows.get(&win_id) else {
                    return;
                };
                let w = &win.window;
                match msg {
                    M::MinimizeWindow => w.set_minimized(true),
                    M::ToggleMaximize => w.set_maximized(!w.is_maximized()),
                    _ => {
                        let _ = w.drag_resize_window(winit::window::ResizeDirection::North);
                    }
                }
            }
            M::ToggleDarkWebsites => self.update_settings(|s| {
                s.appearance.dark_websites = Some(s.appearance.dark_websites != Some(true));
            }),
            M::SetDarkWebsites { on } => {
                self.update_settings(|s| s.appearance.dark_websites = Some(on))
            }
            M::ToggleDarkSite { host } => {
                let host = shoji_core::settings::extract_domain(&host);
                if host.is_empty() {
                    return;
                }
                // Flip what the user sees on this site: dark becomes never-dark, and the other way.
                self.update_settings(|s| {
                    let mode = if s.dark_on(&host) { "never" } else { "always" };
                    set_dark_site_mode(s, &host, mode);
                });
            }
            M::ToggleDarkPanel => self.toggle_sidebar(win_id, "dark"),
            M::OpenPage { page } => {
                if let Some(page) = [
                    "history",
                    "bookmarks",
                    "downloads",
                    "passwords",
                    "byobu",
                    "settings",
                    "wish",
                    "tea",
                    "koan",
                ]
                .into_iter()
                .find(|p| *p == page)
                {
                    self.open_internal(win_id, page);
                }
            }
            M::PrintPage => self.handle_command(win_id, Command::Print, event_loop),
            M::OpenClearData => self.handle_command(win_id, Command::ClearData, event_loop),
            M::HistoryQuery {
                query,
                offset,
                limit,
                seq,
            } => {
                let private = self.settings.privacy.ephemeral_default;
                let (items, total) = self.history.page(&query, offset, limit.clamp(1, 200));
                let items: Vec<serde_json::Value> = items
                    .iter()
                    .map(|h| {
                        serde_json::json!({
                            "url": h.url, "title": h.title, "visits": h.visits, "last": h.last_visit,
                        })
                    })
                    .collect();
                let clear = offset == 0 && std::mem::take(&mut self.clear_pending);
                let data = serde_json::json!({
                    "items": items, "total": total, "private": private, "clear": clear,
                });
                self.eval_all_internal(&format!(
                    "window.__historyPage && window.__historyPage({seq},{data})"
                ));
            }
            M::HistoryRemove { urls } => {
                self.history.remove(&urls);
                self.eval_all_internal("window.__historyChanged && window.__historyChanged()");
            }
            M::DownloadsQuery => self.push_downloads(),
            M::DownloadAction { key, action } => {
                let Some(entry) = self.downloads.get(key).cloned() else {
                    return;
                };
                let path = std::path::PathBuf::from(&entry.path);
                match action.as_str() {
                    "open" if path.is_file() => {
                        let _ = std::process::Command::new("explorer").arg(&path).spawn();
                    }
                    "show" => {
                        if path.is_file() {
                            let _ = std::process::Command::new("explorer")
                                .arg(format!("/select,{}", path.display()))
                                .spawn();
                        } else if let Some(dir) = path.parent().filter(|d| d.is_dir()) {
                            let _ = std::process::Command::new("explorer").arg(dir).spawn();
                        }
                    }
                    "remove" => {
                        self.downloads.remove(key);
                        self.push_downloads();
                    }
                    _ => {}
                }
            }
            M::ClearBrowsingData {
                range,
                history,
                downloads,
                cookies,
                cache,
            } => {
                let now_ms = now_secs() * 1000;
                let span_ms: Option<u64> = match range.as_str() {
                    "hour" => Some(3_600_000),
                    "day" => Some(86_400_000),
                    "week" => Some(7 * 86_400_000),
                    "month" => Some(30 * 86_400_000),
                    _ => None,
                };
                let since_ms = span_ms.map(|s| now_ms.saturating_sub(s));
                let mut forgot = 0;
                if history {
                    forgot = match since_ms {
                        Some(since) => self.history.clear_since(since),
                        None => {
                            let n = self.history.len();
                            self.history.clear();
                            n
                        }
                    };
                }
                if downloads {
                    self.downloads.clear_since(since_ms.unwrap_or(0));
                    self.downloads.flush();
                    self.push_downloads();
                }
                if cookies || cache {
                    let since_secs = since_ms.map(|ms| ms as f64 / 1000.0);
                    if let Some(tab) = self.windows.get(&win_id).and_then(|w| w.active()) {
                        webview::clear_browsing_data(&tab.view, cookies, cache, since_secs);
                    }
                }
                self.eval_all_internal(&format!(
                    "window.__dataCleared && window.__dataCleared({{history:{forgot}}})"
                ));
                self.eval_all_internal("window.__historyChanged && window.__historyChanged()");
            }
            M::UpdateAction { choice } => self.update_choice(win_id, &choice),
            M::FoundSecret { id } => {
                if self.keepsakes.find(&id) {
                    if let Ok(mut ctx) = self.pages.write() {
                        ctx.found = self.keepsakes.found().to_vec();
                    }
                }
            }
            M::TrayRects {
                find,
                permission,
                toast,
                letter,
            } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.tray_rects = [find, permission, toast, letter];
                    win.layout();
                    win.raise_overlays();
                }
            }
            M::SetDarkTheme { host, theme } => {
                let host = host.map(|h| shoji_core::settings::extract_domain(&h));
                self.update_settings(|s| match host {
                    Some(h) if !h.is_empty() => {
                        s.appearance.dark_site_themes.insert(h, theme);
                    }
                    _ => s.appearance.dark_theme = theme,
                });
            }
            M::SetDarkSiteMode { host, mode } => {
                let host = shoji_core::settings::extract_domain(&host);
                if !host.is_empty() {
                    self.update_settings(|s| set_dark_site_mode(s, &host, &mode));
                }
            }
            M::SetBrowserTheme { theme } => {
                self.update_settings(|s| s.appearance.browser_theme = theme);
            }
            M::ResetDarkSite { host } => {
                let host = shoji_core::settings::extract_domain(&host);
                self.update_settings(|s| {
                    s.appearance.dark_site_themes.remove(&host);
                });
            }
            M::SetDarkSchedule { enabled, from, to } => {
                self.update_settings(|s| {
                    s.appearance.dark_schedule =
                        shoji_core::settings::DarkSchedule { enabled, from, to };
                });
            }
            M::ToggleNotes => {
                if self.windows.get(&win_id).is_some_and(|w| w.notes_open) {
                    self.close_notes(win_id);
                } else {
                    self.open_notes(win_id);
                }
            }
            M::CloseNotes => self.close_notes(win_id),
            M::NotesReady => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.notes_ready = true;
                    if let (true, Some(n)) = (win.notes_open, &win.notes) {
                        let _ = n.set_visible(true);
                        webview::bring_to_top(n);
                    }
                    if let (true, None, Some(from)) =
                        (win.notes_open, &win.notes_anim, win.notes_clip)
                    {
                        let sheet = win.notes_sheet();
                        win.animate_notes(from, sheet, NOTES_OPEN_MS, false);
                    }
                }
            }
            M::SetNotesExpanded { expanded } => self.set_notes_expanded(win_id, expanded),
            M::SaveNote { id, created, text } => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or_default();
                if let Err(e) = self.notebook.save(&id, created, &text, now) {
                    eprintln!("Failed to save note: {e}");
                }
            }
            M::DeleteNote { id } => {
                let _ = self.notebook.delete(&id);
            }
            M::CloseWindow => self.close_window(win_id, event_loop),
            M::CreateTab { url } => {
                let url =
                    url.and_then(|u| normalize_url(&u, self.settings.search_url_template()).ok());
                self.create_tab(win_id, url);
            }
            M::OpenNewWindow => self.new_window(event_loop, None),
            M::SwitchTab { id } => {
                let target_ws = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.tabs.get_tab_by_id(id).map(|t| t.workspace));
                if let Some(ws) = target_ws {
                    if self
                        .windows
                        .get(&win_id)
                        .is_some_and(|w| w.tabs.active_workspace_id() != ws)
                    {
                        self.switch_workspace(win_id, ws);
                    }
                }
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.show_tab(id);
                    win.layout();
                }
            }
            M::CloseTab { id } => self.close_tab(win_id, id, event_loop),
            M::Navigate { url } => {
                // From a page: navigate the tab that sent it, not whichever is active.
                if let Some(tab) = from_tab {
                    if let Some(win) = self.windows.get_mut(&win_id) {
                        win.show_tab(tab);
                    }
                }
                let url = if url.is_empty() {
                    format!("{INTERNAL_SCHEME}newtab")
                } else {
                    url
                };
                self.navigate(win_id, &url);
            }
            M::GoBack | M::GoForward | M::Stop | M::OpenDevTools => {
                if let Some(tab) = self.windows.get(&win_id).and_then(|w| w.active()) {
                    if let M::OpenDevTools = msg {
                        return tab.view.open_devtools();
                    }
                    if let Ok(core) = unsafe { tab.view.controller().CoreWebView2() } {
                        let _ = unsafe {
                            match msg {
                                M::GoBack => core.GoBack(),
                                M::GoForward => core.GoForward(),
                                _ => core.Stop(),
                            }
                        };
                    }
                }
            }
            M::Reload => self.reload(win_id, false),
            M::ReloadTab { tab_id } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.show_tab(tab_id);
                }
                self.reload(win_id, false);
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if let Some(t) = win.tabs.tabs_mut().iter_mut().find(|t| t.id == tab_id) {
                        t.status = TabStatus::Active;
                    }
                }
            }
            M::ToggleMenuPanel => self.toggle_sidebar(win_id, "menu"),
            M::ToggleSecurityPanel => self.toggle_sidebar(win_id, "security"),
            M::ToggleShieldPanel => self.toggle_sidebar(win_id, "shield"),
            M::ShieldPickElement => {
                if let Some(win) = self.windows.get(&win_id) {
                    if let (Some(id), Some(tab)) = (win.tabs.active_tab_id(), win.active()) {
                        shield::pick_element(&tab.view, id);
                        webview::focus(&tab.view);
                    }
                }
                self.close_sidebar(win_id, false);
            }
            M::ShieldState { refresh, log } => {
                if refresh {
                    shield::hub().refresh(true);
                }
                let tab = from_tab.and_then(|id| self.windows.get(&win_id)?.views.get(&id));
                if let Some(tab) = tab {
                    let json = shield::hub().state_json(log, None);
                    let _ = tab.view.evaluate_script(&format!(
                        "window.__byobuState&&window.__byobuState({json})"
                    ));
                }
            }
            M::ToggleDownloadsSidebar => self.toggle_sidebar(win_id, "downloads"),
            M::CloseSidebar => self.close_sidebar(win_id, true),
            M::SidebarPointer { inside } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.pop_pointer_in = inside;
                }
            }
            M::SidebarBlur => {
                let mode = self.windows.get(&win_id).map(|w| w.sidebar_mode);
                let open = self.windows.get(&win_id).is_some_and(|w| {
                    w.sidebar_open
                        && !w.sidebar_docked
                        && !(w.overlay_open
                            && matches!(w.overlay_mode, "menu" | "savebm")
                            && w.sidebar_mode == "bookmarks")
                        && w.pop_hide_at.is_none()
                        && w.pop_opened
                            .is_none_or(|t| t.elapsed() > Duration::from_millis(300))
                });
                if let (true, Some(mode)) = (open, mode) {
                    self.close_sidebar(win_id, false);
                    if let Some(win) = self.windows.get_mut(&win_id) {
                        win.pop_blurred = Some((Instant::now(), mode));
                    }
                }
            }
            M::SidebarHeight { height } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    let height = height.clamp(40.0, 1200.0).ceil();
                    if (win.pop_height - height).abs() > 0.5 {
                        win.pop_height = height;
                        if win.sidebar_open && !win.sidebar_docked {
                            win.layout();
                        }
                    }
                }
            }
            // Dropdowns always float; there is no pinned side panel any more.
            M::PinSidebar { .. } => {}
            M::SiteCookies | M::ClearSiteCookies => {
                let clear = matches!(msg, M::ClearSiteCookies);
                let Some(tab) = self.windows.get(&win_id).and_then(|w| w.active()) else {
                    return;
                };
                let url = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.tabs.active_tab())
                    .map(|t| t.url.clone())
                    .unwrap_or_default();
                if !(url.starts_with("https://") || url.starts_with("http://"))
                    || url.starts_with("http://shoji.")
                {
                    return;
                }
                if clear {
                    // Stored data (local storage, databases, caches, workers) goes with the cookies.
                    let origin = url.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
                    let params = serde_json::json!({
                        "origin": origin,
                        "storageTypes": "local_storage,indexeddb,cache_storage,service_workers,file_systems,websql",
                    })
                    .to_string();
                    webview::cdp(&tab.view, "Storage.clearDataForOrigin", &params);
                }
                let proxy = self.proxy.clone();
                webview::site_cookies(&tab.view, &url, clear, move |n| {
                    let _ = proxy.send_event(BrowserEvent::SiteCookies(win_id, n, clear));
                });
            }
            M::SplitZones { zones } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.split_zones = zones.iter().map(|z| (z[0] as u32, z[1], z[2])).collect();
                }
            }
            M::ToolbarAnchors { items } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.anchors = items.into_iter().collect();
                    if win.sidebar_open && !win.sidebar_docked {
                        win.layout();
                    }
                }
            }
            M::OpenCertificateDialog { host } => cert::open_certificate_dialog(&host),
            M::ReorderTab {
                from_index,
                to_index,
            } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if self.settings.tabs.enable_tab_reordering
                        && win.tabs.reorder_tab(from_index, to_index)
                    {
                        win.ui_dirty = true;
                    }
                }
            }
            M::TabDragStart {
                tab_id,
                grab_dx,
                grab_dy,
                strip_left,
            } => self.start_tab_drag(win_id, tab_id, (grab_dx, grab_dy, strip_left), event_loop),
            M::TabCenters { centers } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.tab_centers = centers;
                }
            }
            M::ReorderGroup {
                from_index,
                count,
                to_index,
            } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.reorder_block(from_index, count, to_index) {
                        win.ui_dirty = true;
                    }
                }
            }
            M::DetachTabToNewWindow {
                tab_id,
                screen_x,
                screen_y,
            } => {
                if self.settings.tabs.enable_tab_tearoff {
                    self.detach_tab(win_id, tab_id, screen_x.zip(screen_y), event_loop);
                }
            }
            M::ZoomIn | M::ZoomOut | M::ZoomReset => {
                // The zoom is remembered per site; Reset goes back to the default and forgets it.
                let mut remember: Option<(String, Option<f64>)> = None;
                if let Some(win) = self.windows.get(&win_id) {
                    let z = win.zoom();
                    win.set_zoom(match msg {
                        M::ZoomIn => z + 0.1,
                        M::ZoomOut => z - 0.1,
                        _ => self.settings.appearance.default_zoom_level,
                    });
                    if let Some(host) = win.tabs.active_tab().and_then(|t| extract_host(&t.url)) {
                        let reset = matches!(msg, M::ZoomReset);
                        remember = Some((host, (!reset).then(|| win.zoom())));
                    }
                }
                if let Some((host, zoom)) = remember {
                    self.site_data.set_zoom(&host, zoom);
                    self.push_data();
                }
            }
            M::ToggleFindBar => {
                self.close_sidebar(win_id, false);
                if let Some(win) = self.windows.get(&win_id) {
                    win.eval_chrome("window.__toggleFindBar && window.__toggleFindBar()");
                    if let Some(c) = &win.chrome {
                        webview::focus(c);
                    }
                }
            }
            M::OpenFindInPage | M::CloseFindInPage => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.find_open = matches!(msg, M::OpenFindInPage);
                    if !win.find_open {
                        if let Some(tab) = win.active() {
                            let _ = tab.view.evaluate_script("getSelection().removeAllRanges()");
                        }
                    }
                    win.layout();
                }
            }
            M::FindInPage { query, forward } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.find_query = query.clone();
                    win.find(&query, forward);
                }
            }
            M::FindResult { current, total } => {
                if let Some(win) = self.windows.get(&win_id) {
                    win.eval_chrome(&format!(
                        "window.__syncFindResult && window.__syncFindResult({{current:{current},total:{total}}})"
                    ));
                }
            }
            M::CancelDownload { download_id } => webview::cancel_download(download_id),
            M::ChangeDownloadFolder => {
                if let Some(folder) = webview::pick_folder(&self.settings.downloads.default_folder)
                {
                    let js = format!(
                        "window.__setDownloadFolder && window.__setDownloadFolder({})",
                        js_string(&folder.to_string_lossy())
                    );
                    self.update_settings(|s| s.downloads.default_folder = folder);
                    for win in self.windows.values() {
                        win.eval_internal_tabs(&js);
                    }
                }
            }
            M::OpenPermissionPrompt | M::ClosePermissionPrompt => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.permission_open = matches!(msg, M::OpenPermissionPrompt);
                    win.layout();
                }
            }
            M::PermissionResponse {
                permission_id,
                allow,
            } => {
                // The answer is remembered for that site, so it isn't asked again.
                if let Some((host, kind)) = self.perm_pending.remove(&permission_id) {
                    self.site_data.set_permission(&host, kind, allow);
                    self.push_data();
                }
                webview::resolve_permission(permission_id, allow)
            }
            M::OpenSettings => self.open_internal(win_id, "settings"),
            M::SaveSettings { patch } => {
                let bar = self.settings.appearance.bookmarks_bar;
                self.update_settings(|s| {
                    s.apply_patch(&patch);
                });
                if bar != self.settings.appearance.bookmarks_bar {
                    self.set_bookmarks_bar(self.settings.appearance.bookmarks_bar);
                }

                // Pausing or resuming the shield for a site only takes effect on the next load,
                // so reload the page that is showing (never a Shoji page, such as Settings).
                if patch.pointer("/shield/paused_sites").is_some() {
                    let live = self
                        .windows
                        .get(&win_id)
                        .and_then(|w| w.tabs.active_tab())
                        .is_some_and(|t| !is_internal(&t.url));
                    if live {
                        self.reload(win_id, false);
                    }
                }
            }
            M::SetAccent { accent } => {
                let accent = Accent::from_id(&accent).id().to_string();
                self.update_settings(|s| s.appearance.accent = accent);
            }
            M::SetTheme { theme } => self.update_settings(|s| {
                s.apply_patch(&serde_json::json!({ "appearance": { "theme": theme } }));
            }),
            M::FinishWelcome => {
                self.update_settings(|s| s.first_run_complete = true);
                if let Some(tab) = from_tab {
                    if let Some(win) = self.windows.get_mut(&win_id) {
                        win.tabs.update_url(tab, format!("{INTERNAL_SCHEME}newtab"));
                        win.tabs.update_title(tab, "New tab".into());
                        win.ui_dirty = true;
                        if let Some(t) = win.views.get_mut(&tab) {
                            t.pending_url = None;
                            let _ = t.view.load_url(&engine_url("shoji://newtab"));
                        }
                    }
                }
            }
            M::ListImportSources => {
                let json =
                    serde_json::to_string(&import_win::detect()).unwrap_or_else(|_| "[]".into());
                self.eval_welcome_and_settings(
                    win_id,
                    &format!("window.__importSources && window.__importSources({json})"),
                );
            }
            M::PickImportSource { source } => {
                if let Some(src) = shoji_core::import::catalog::find(&source) {
                    let picked = if src.family == shoji_core::import::catalog::Family::HtmlFile {
                        webview::pick_html_file()
                    } else {
                        webview::pick_folder(&import_win::picker_start(src))
                    };
                    if let Some(path) = picked {
                        self.eval_welcome_and_settings(
                            win_id,
                            &format!(
                                "window.__importPicked && window.__importPicked({},{})",
                                js_string(&source),
                                js_string(&path.to_string_lossy())
                            ),
                        );
                    }
                }
            }
            M::StartImport {
                source,
                profile,
                path,
                mut kinds,
            } => {
                let Some(src) = shoji_core::import::catalog::find(&source) else {
                    return;
                };
                // Private mode keeps no history or sign-ins, so there is nothing to put them in.
                if self.settings.privacy.ephemeral_default {
                    kinds.history = false;
                    kinds.cookies = false;
                }
                let chosen = import_win::profiles_for(src, path.as_deref())
                    .into_iter()
                    .nth(profile);
                let Some(chosen) = chosen else {
                    self.eval_welcome_and_settings(
                        win_id,
                        &format!(
                            "window.__importFailed && window.__importFailed({})",
                            js_string(&format!("Couldn't find {} on this PC.", src.name))
                        ),
                    );
                    return;
                };
                let proxy = self.proxy.clone();
                std::thread::spawn(move || {
                    let progress_proxy = proxy.clone();
                    let mut progress = |text: &str| {
                        let _ = progress_proxy.send_event(BrowserEvent::ImportProgress {
                            win: win_id,
                            text: text.to_string(),
                        });
                    };
                    let imported = import_win::read(src, &chosen, kinds, &mut progress);
                    let _ = proxy.send_event(BrowserEvent::ImportDone {
                        win: win_id,
                        imported: Box::new(imported),
                    });
                });
            }
            M::CheckDefaultBrowser => {
                let on = default_browser::is_default();
                self.eval_welcome_and_settings(
                    win_id,
                    &format!("window.__defaultBrowser && window.__defaultBrowser({on})"),
                );
            }
            M::SetDefaultBrowser => default_browser::open_settings(),
            M::SearchHistory {
                query,
                seq,
                overlay,
            } => {
                let items: Vec<serde_json::Value> = if self.settings.privacy.ephemeral_default {
                    Vec::new()
                } else {
                    self.history
                        .search(&query, 5)
                        .iter()
                        .map(|h| serde_json::json!({ "url": h.url, "title": h.title }))
                        .collect()
                };
                let auto = if self.settings.privacy.ephemeral_default {
                    None
                } else {
                    self.history.autocomplete(&query)
                };
                if let Some(win) = self.windows.get(&win_id) {
                    if overlay {
                        if let Some(o) = &win.overlay {
                            let _ = o.evaluate_script(&format!(
                                "window.__paneHistory && window.__paneHistory({seq},{},{})",
                                serde_json::Value::Array(items),
                                serde_json::json!(auto)
                            ));
                        }
                    } else {
                        win.eval_chrome(&format!(
                            "window.__historyResults && window.__historyResults({seq},{},{})",
                            serde_json::Value::Array(items),
                            serde_json::json!(auto)
                        ));
                    }
                }
            }
            M::SuggestQuery {
                query,
                seq,
                overlay,
            } => {
                let Some(template) = pages::suggest_url(&self.settings) else {
                    return;
                };
                let url = template.replace("%s", &shoji_core::tabs::url_encode(&query));
                let proxy = self.proxy.clone();
                let _ = std::thread::Builder::new()
                    .name("shoji-suggest".into())
                    .spawn(move || {
                        let words = fetch_suggestions(&url);
                        let _ = proxy
                            .send_event(BrowserEvent::SuggestWords(win_id, seq, words, overlay));
                    });
            }
            M::ClearHistory => self.history.clear(),
            M::VaultStatus => self.push_vault(),
            M::VaultList { query } => {
                self.vault_open(false);
                self.push_vault_list(&query);
            }
            M::VaultReveal { id } => {
                if self.vault_open(false) {
                    self.vault.touch();
                    if let Some(l) = self.vault.get(&id) {
                        let secret = serde_json::json!({ "password": l.password, "note": l.note });
                        self.eval_all_internal(&format!(
                            "window.__vaultSecret && window.__vaultSecret({}, {secret})",
                            js_string(&id)
                        ));
                    }
                }
            }
            M::VaultCopy { id, field } => {
                if self.vault_open(false) {
                    self.vault.touch();
                    let text = self.vault.get(&id).map(|l| {
                        if field == "username" {
                            l.username.clone()
                        } else {
                            l.password.clone()
                        }
                    });
                    if let Some(text) = text {
                        if vault_win::copy_secret(&text) {
                            self.vault.mark_used(&id, now_ms());
                            self.vault_toast(if field == "username" {
                                "Username copied"
                            } else {
                                "Password copied. It clears from the clipboard in 30 seconds"
                            });
                        }
                    }
                }
            }
            M::VaultSave {
                id,
                url,
                title,
                username,
                password,
                note,
            } => {
                if self.vault_open(true) {
                    let draft = shoji_core::vault::Draft {
                        url,
                        title,
                        username,
                        password,
                        note,
                    };
                    let ok = match id {
                        Some(id) => self.vault.edit(&id, &draft, now_ms()),
                        None => self.vault.upsert(&draft, now_ms()).is_some(),
                    };
                    self.vault_toast(if ok {
                        "Saved"
                    } else {
                        "Couldn't save that login"
                    });
                    self.vault_changed();
                }
            }
            M::VaultDelete { id } => {
                if self.vault_open(false) && self.vault.remove(&id) {
                    self.vault_toast("Deleted");
                    self.vault_changed();
                }
            }
            M::VaultDeleteAll => {
                if self.vault_open(false) {
                    self.vault.delete_all();
                    self.vault_toast("All passwords deleted");
                    self.vault_changed();
                }
            }
            M::VaultUnlock { master } => {
                if self.vault.unlock_master(&master) {
                    self.vault_changed();
                } else {
                    let mut s = self.vault_state_json();
                    s["error"] = "That isn't the master password.".into();
                    self.eval_all_internal(&format!(
                        "window.__vaultState && window.__vaultState({s})"
                    ));
                }
            }
            M::VaultLock => {
                self.vault.lock();
                self.push_vault();
            }
            M::VaultSetMaster { master } => {
                if self.vault_open(true) && self.vault.set_master(&master) {
                    self.vault_toast("Master password set");
                } else {
                    self.vault_toast("Couldn't set the master password");
                }
                self.vault_changed();
            }
            M::VaultRemoveMaster => {
                if self.vault.is_unlocked() && self.vault.remove_master(&vault_win::WinProtector) {
                    self.vault_toast("Master password removed");
                }
                self.vault_changed();
            }
            M::VaultGenerate {
                length,
                lower,
                upper,
                digits,
                symbols,
            } => {
                let pw = shoji_core::vault::generate(shoji_core::vault::GenOptions {
                    length,
                    lower,
                    upper,
                    digits,
                    symbols,
                });
                self.eval_all_internal(&format!(
                    "window.__vaultGenerated && window.__vaultGenerated({})",
                    js_string(&pw)
                ));
            }
            M::VaultImportCsv => {
                let Some(path) =
                    webview::pick_file("Choose a passwords CSV file", "CSV files (*.csv)", "*.csv")
                else {
                    return;
                };
                let text = std::fs::read(&path)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                let drafts = shoji_core::vault::drafts_from_csv(&text);
                if drafts.is_empty() {
                    self.vault_toast("No logins found in that file");
                } else if self.vault_open(true) {
                    let (added, updated, skipped) = self.vault.import(&drafts, now_ms());
                    self.eval_all_internal(&format!(
                        "window.__vaultImported && window.__vaultImported({{\"added\":{added},\"updated\":{updated},\"skipped\":{skipped}}})"
                    ));
                    self.vault_changed();
                } else {
                    self.vault_toast("Unlock your passwords first");
                }
            }
            M::VaultExportCsv => {
                if self.vault_open(false) {
                    if let Some(path) = webview::pick_save_csv("shoji-passwords.csv") {
                        let ok = std::fs::write(&path, self.vault.to_csv()).is_ok();
                        self.vault_toast(if ok {
                            "Exported. Keep that file safe"
                        } else {
                            "Couldn't write the file"
                        });
                    }
                }
            }
            M::LoginSubmitted { username, password } => {
                let Some(tab) = from_tab else {
                    return;
                };
                if password.is_empty() || password.len() > 1024 || username.len() > 512 {
                    return;
                }
                let Some(url) = self.login_page(win_id, tab) else {
                    return;
                };
                let host = shoji_core::vault::host_of(&url);
                if self.vault.state() != shoji_core::vault::State::Empty && !self.vault_open(false)
                {
                    return; // locked behind a master password: stay quiet
                }
                if self.vault.is_never(&host) {
                    return;
                }
                let existing = self
                    .vault
                    .logins()
                    .iter()
                    .find(|l| l.host() == host && l.username == username)
                    .map(|l| (l.id.clone(), l.password == password));
                let existing_id = match existing {
                    Some((id, true)) => {
                        self.vault.mark_used(&id, now_ms());
                        return;
                    }
                    Some((id, false)) => Some(id),
                    None => None,
                };
                self.pending_login = Some(PendingLogin {
                    url,
                    username,
                    password,
                    existing: existing_id,
                    at: Instant::now(),
                });
                self.offer_save(win_id);
            }
            M::LoginFieldFocused { x, y, w, h } => {
                let Some(tab) = from_tab else {
                    return;
                };
                let Some(url) = self.login_page(win_id, tab) else {
                    return;
                };
                let host = shoji_core::vault::host_of(&url);
                let Some(win) = self.windows.get(&win_id) else {
                    return;
                };
                // Only the tab being looked at, and not in split view (the page offsets differ).
                if win.tabs.active_tab_id() != Some(tab) || win.split_showing().is_some() {
                    return;
                }
                if self.vault.state() == shoji_core::vault::State::Empty || !self.vault_open(false)
                {
                    return;
                }
                use shoji_core::ipc::OverlayItem;
                let items: Vec<OverlayItem> = self
                    .vault
                    .for_host(&host)
                    .iter()
                    .take(5)
                    .map(|l| OverlayItem {
                        kind: "login".into(),
                        value: l.id.clone(),
                        label: if l.username.is_empty() {
                            "Saved login".into()
                        } else {
                            l.username.clone()
                        },
                        sub: format!("Fill with Shoji · {}", l.host()),
                        color: None,
                    })
                    .collect();
                if items.is_empty() {
                    return;
                }
                let Some(win) = self.windows.get(&win_id) else {
                    return;
                };
                let width = w.clamp(240.0, 380.0);
                let left = x.clamp(4.0, (win.width - width - 4.0).max(4.0));
                let top = win.chrome_height() + y + h + 2.0;
                let frame = [left, top, width, items.len() as f64 * 46.0 + 12.0];
                self.show_overlay(win_id, "suggest", items, frame, "", "", "");
            }
            M::ToggleFavourite => self.bookmark_current(win_id),
            M::RunAction { value } => self.run_action(win_id, &value, event_loop),
            m @ (M::BookmarksQuery
            | M::BookmarkEdit { .. }
            | M::BookmarkMove { .. }
            | M::BookmarkNewFolder { .. }
            | M::BookmarkRenameFolder { .. }
            | M::BookmarkMoveFolder { .. }
            | M::BookmarkDeleteFolder { .. }
            | M::BookmarkRemove { .. }
            | M::BookmarkOpen { .. }
            | M::BookmarkMenu { .. }
            | M::BookmarkFolderMenu { .. }
            | M::BookmarkOverflow { .. }
            | M::BookmarkPage { .. }) => self.bookmark_ipc(win_id, m, event_loop),
            M::ToggleBookmarksBar => {
                let on = !self.settings.appearance.bookmarks_bar;
                self.set_bookmarks_bar(on);
            }
            M::SetSidebarMode { mode } => self.set_sidebar_mode(&mode),
            M::HoverBookmarks { left, right, bar } => {
                // Only a free pointer opens it: not over another dropdown, a menu, or a drag.
                let free = !left_button_down()
                    && self.windows.get(&win_id).is_some_and(|w| {
                        !w.sidebar_open
                            && !w.overlay_open
                            && w.pop_blurred
                                .is_none_or(|(at, _)| at.elapsed() > Duration::from_millis(350))
                    });
                if free {
                    self.open_sidebar(win_id, "bookmarks", Some((left, right, bar)));
                }
            }
            M::PinHover => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.pop_hover = false;
                    win.hover_out_since = None;
                }
            }
            M::MenuDataQuery => {
                let json = self.menu_data(win_id);
                if let Some(win) = self.windows.get(&win_id) {
                    win.eval_sidebar(&format!("window.__menuData && window.__menuData({json})"));
                }
            }
            M::AddFavourite { url, title } => {
                if self.favourites.add(&url, &title) {
                    self.push_data();
                }
            }
            M::FavouriteFolder { url } => self.ask_favourite_folder(win_id, &url),
            M::ExportFavourites => {
                if let Some(path) = webview::pick_save_html("shoji-bookmarks.html") {
                    let ok = std::fs::write(&path, self.favourites.to_netscape_html()).is_ok();
                    self.toast(&serde_json::json!({
                        "title": if ok { "Exported." } else { "Couldn't write the file." },
                        "text": if ok { "Your bookmarks are in a file any browser can import." } else { "Pick another place to save it." },
                        "pose": "unfold",
                    }));
                }
            }
            M::RemoveFavourite { url } => {
                if self.favourites.remove(&url) {
                    self.push_data();
                }
            }
            M::SuggestShow { items, x, width } => {
                let Some(win) = self.windows.get(&win_id) else {
                    return;
                };
                if items.is_empty() {
                    return self.hide_overlay(win_id, false);
                }
                let w = width.clamp(300.0, 760.0).min(win.width - 8.0);
                let x = x.min(win.width - w - 4.0).max(4.0);
                // Rows with a second line (the address) are taller than plain ones.
                let h = items
                    .iter()
                    .take(8)
                    .map(|i| if i.sub.is_empty() { 34.0 } else { 46.0 })
                    .sum::<f64>()
                    + 12.0;
                let top = win.chrome_height();
                self.show_overlay(win_id, "suggest", items, [x, top, w, h], "", "", "");
            }
            M::SuggestHighlight { index } => {
                if let Some(win) = self.windows.get(&win_id) {
                    if let (true, Some(o)) = (win.overlay_open, &win.overlay) {
                        let _ = o.evaluate_script(&format!(
                            "window.__overlayHighlight && window.__overlayHighlight({index})"
                        ));
                    }
                }
            }
            M::SuggestClose => {
                if self
                    .windows
                    .get(&win_id)
                    .is_some_and(|w| w.overlay_mode == "suggest")
                {
                    self.hide_overlay(win_id, false);
                }
            }
            M::OpenPalette => {
                if self
                    .windows
                    .get(&win_id)
                    .is_some_and(|w| w.overlay_open && w.overlay_mode == "palette")
                {
                    return self.hide_overlay(win_id, true);
                }
                let items = self.palette_items(win_id);
                let Some(win) = self.windows.get(&win_id) else {
                    return;
                };
                let w = (win.width - 40.0).clamp(300.0, 620.0);
                let top = win.chrome_height() + 12.0;
                let h = (win.height - top - 24.0).clamp(160.0, 440.0);
                let x = ((win.width - w) / 2.0).max(4.0);
                self.show_overlay(
                    win_id,
                    "palette",
                    items,
                    [x, top, w, h],
                    "",
                    "Jump to a tab, bookmark, setting or action",
                    "",
                );
            }
            M::OverlayReady => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.overlay_ready = true;
                    let mode = win.overlay_mode;
                    if let (Some(js), Some(o)) = (win.overlay_pending.take(), &win.overlay) {
                        let _ = o.evaluate_script(&js);
                        if win.overlay_open {
                            let _ = o.set_visible(true);
                            webview::bring_to_top(o);
                        }
                        if win.overlay_open && mode != "suggest" {
                            webview::focus(o);
                        }
                    }
                }
            }
            M::SidebarReady => {
                let bookmarks = format!(
                    "window.__bookmarks && window.__bookmarks({})",
                    self.bookmarks_json()
                );
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.sidebar_ready = true;
                    win.eval_sidebar(&bookmarks);
                    if win.sidebar_open {
                        win.layout();
                        if let Some(s) = &win.sidebar {
                            let _ = s.set_visible(true);
                            webview::bring_to_top(s);
                            if let Some(js) = win.sidebar_pending.clone() {
                                let _ = s.evaluate_script(&js);
                            }
                        }
                    }
                }
            }
            M::OverlayClose => {
                // Focus moving between surfaces right after the overlay appears isn't a dismissal.
                let fresh = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.overlay_shown)
                    .is_some_and(|t| t.elapsed() < Duration::from_millis(250));
                if !fresh {
                    self.hide_overlay(win_id, true);
                }
            }
            M::OverlayChoose { kind, value } => {
                // Navigation and tab switches focus the page themselves; actions that open a
                // prompt must keep the focus for it.
                let opens_prompt = [
                    "group-new",
                    "group-rename",
                    "split-rename",
                    "fav-folder",
                    "ws-new",
                    "ws-rename",
                ]
                .iter()
                .any(|p| value.starts_with(p));
                self.hide_overlay(win_id, kind == "action" && !opens_prompt);
                match kind.as_str() {
                    "url" => self.navigate(win_id, &value),
                    "search" => {
                        let url = self
                            .settings
                            .search_url_template()
                            .replace("%s", &shoji_core::tabs::url_encode(&value));
                        self.navigate(win_id, &url);
                    }
                    "tab" => {
                        if let Ok(id) = value.parse::<u64>() {
                            let tab_id = TabId(id);
                            let ws = self
                                .windows
                                .get(&win_id)
                                .and_then(|w| w.tabs.get_tab_by_id(tab_id).map(|t| t.workspace));
                            if let Some(ws) = ws {
                                if self
                                    .windows
                                    .get(&win_id)
                                    .is_some_and(|w| w.tabs.active_workspace_id() != ws)
                                {
                                    self.switch_workspace(win_id, ws);
                                }
                            }
                            if let Some(win) = self.windows.get_mut(&win_id) {
                                win.show_tab(tab_id);
                                win.layout();
                            }
                        }
                    }
                    "action" => self.run_action(win_id, &value, event_loop),
                    "login" => self.fill_login(win_id, &value),
                    "login-save" => self.commit_login(&value),
                    "text" => self.finish_prompt(win_id, &value),
                    _ => {}
                }
            }
            M::TabMenu { tab_id, x, y } => {
                let items = self.tab_menu_items(win_id, tab_id);
                let Some(win) = self.windows.get(&win_id) else {
                    return;
                };
                let w = 260.0f64.min(win.width - 8.0);
                let x = x.min(win.width - w - 4.0).max(4.0);
                let h = items.len() as f64 * 32.0 + 10.0;
                self.show_overlay(win_id, "menu", items, [x, y.max(30.0), w, h], "", "", "");
            }
            M::EngineMenu { x, y } => {
                use shoji_core::ipc::OverlayItem;
                let current = self.settings.search_engine.to_lowercase();
                let items: Vec<OverlayItem> = [
                    ("google", "Google"),
                    ("duckduckgo", "DuckDuckGo"),
                    ("bing", "Bing"),
                    ("brave", "Brave"),
                    ("ecosia", "Ecosia"),
                ]
                .iter()
                .map(|(id, name)| OverlayItem {
                    kind: "action".into(),
                    value: format!("engine:{id}"),
                    label: format!("{}{name}", if *id == current { "● " } else { "" }),
                    sub: String::new(),
                    color: None,
                })
                .collect();
                let Some(win) = self.windows.get(&win_id) else {
                    return;
                };
                let w = 220.0f64.min(win.width - 8.0);
                let x = x.min(win.width - w - 4.0).max(4.0);
                let h = items.len() as f64 * 32.0 + 10.0;
                self.show_overlay(win_id, "menu", items, [x, y.max(30.0), w, h], "", "", "");
            }
            M::WorkspaceMenu { workspace_id, x, y } => {
                let items = self.workspace_menu_items(win_id, workspace_id);
                let Some(win) = self.windows.get(&win_id) else {
                    return;
                };
                let w = 240.0f64.min(win.width - 8.0);
                let x = x.min(win.width - w - 4.0).max(4.0);
                let h = items.len() as f64 * 32.0 + 10.0;
                self.show_overlay(win_id, "menu", items, [x, y.max(30.0), w, h], "", "", "");
            }
            M::ToggleReader => self.toggle_reader(win_id),
            M::TogglePictureInPicture => {
                if let Some(tab) = self.windows.get(&win_id).and_then(|w| w.active()) {
                    let params = serde_json::json!({
                        "expression": PIP_SCRIPT,
                        "userGesture": true,
                        "awaitPromise": true,
                    })
                    .to_string();
                    webview::cdp(&tab.view, "Runtime.evaluate", &params);
                }
            }
            M::SplitWith { tab_id } => self.split_with(win_id, tab_id),
            M::SplitCreate { first, second } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.open_split(first, second).is_some() {
                        win.layout();
                        win.ui_dirty = true;
                    }
                }
            }
            M::SplitClose { id } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.close_split(id) {
                        win.layout();
                        win.ui_dirty = true;
                    }
                }
            }
            M::SplitRename { id, name } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.rename_split(id, &name) {
                        win.ui_dirty = true;
                    }
                }
            }
            M::SplitSwapPanes { id } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.swap_split_panes(id) {
                        win.layout();
                        win.ui_dirty = true;
                    }
                }
            }
            M::SplitSetColor { id, color } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.set_split_color(id, &color) {
                        win.ui_dirty = true;
                    }
                }
            }
            // In a split this adds another tab to it; leaving a split is the "x" on its toolbar
            // or "Ungroup" in the tab's menu.
            M::ToggleSplit => {
                // The same combo closes the picker it opened.
                if self
                    .windows
                    .get(&win_id)
                    .is_some_and(|w| w.overlay_open && w.overlay_mode == "paneadd")
                {
                    self.hide_overlay(win_id, true);
                } else {
                    self.open_split_picker(win_id);
                }
            }
            M::SetSplitRatio { ratio } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.set_split_ratio(ratio) {
                        win.layout();
                        win.ui_dirty = true;
                    }
                }
            }
            M::ResetSplitRatio => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.reset_split_ratio() {
                        win.layout();
                        win.ui_dirty = true;
                    }
                }
            }
            M::SetSplitOrientation { orientation } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.set_split_orientation(orientation) {
                        win.layout();
                        win.ui_dirty = true;
                    }
                }
            }
            M::ToggleSplitOrientation => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.toggle_split_orientation().is_some() {
                        win.layout();
                        win.ui_dirty = true;
                    }
                }
            }
            M::SplitFocus { index } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if let Some(id) = win.tabs.split_panes().and_then(|p| p.get(index).copied()) {
                        win.show_tab(id);
                        win.ui_dirty = true;
                    }
                }
            }
            M::SplitAdd {
                split_id,
                tab_id,
                at,
            } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.add_to_split(split_id, tab_id, at) {
                        win.show_tab(tab_id);
                        win.layout();
                        win.ui_dirty = true;
                        win.sidebar_dirty = true;
                    }
                }
            }
            M::SplitSwapPair { id, index } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.swap_split_pair(id, index) {
                        win.layout();
                        win.ui_dirty = true;
                    }
                }
            }
            M::SplitRemove { tab_id } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.remove_from_split(tab_id) {
                        if let Some(active) = win.tabs.active_tab_id() {
                            win.show_tab(active);
                        }
                        win.layout();
                        win.ui_dirty = true;
                        win.sidebar_dirty = true;
                    }
                }
            }
            M::SwitchSplitPane => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if let Some(next) = win.tabs.switch_split_pane(now_secs()) {
                        win.show_tab(next);
                    }
                }
            }
            M::ToggleWorkspacesSidebar => self.toggle_sidebar(win_id, "spaces"),
            M::CreateWorkspace { name, color } => {
                let ws_id = if let Some(win) = self.windows.get_mut(&win_id) {
                    let color = if color.trim().is_empty() {
                        next_workspace_color(win).to_string()
                    } else {
                        color
                    };
                    let name = if name.trim().is_empty() {
                        format!("Workspace {}", win.tabs.workspaces().len() + 1)
                    } else {
                        name
                    };
                    Some(win.tabs.create_workspace(&name, &color))
                } else {
                    None
                };
                if let Some(ws_id) = ws_id {
                    self.switch_workspace(win_id, ws_id);
                }
            }
            M::SwitchWorkspace { id } => {
                self.switch_workspace(win_id, id);
            }
            M::RenameWorkspace { id, name } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.tabs.rename_workspace(id, &name);
                    win.ui_dirty = true;
                    win.sidebar_dirty = true;
                }
            }
            M::SetWorkspaceColor { id, color } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.tabs.set_workspace_color(id, &color);
                    win.ui_dirty = true;
                    win.sidebar_dirty = true;
                }
            }
            M::ReorderWorkspace {
                from_index,
                to_index,
            } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.reorder_workspace(from_index, to_index) {
                        win.ui_dirty = true;
                        win.sidebar_dirty = true;
                    }
                }
            }
            M::DeleteWorkspace { id } => {
                self.delete_workspace(win_id, id);
            }
            M::MoveTabToWorkspace {
                tab_id,
                workspace_id,
            } => {
                self.move_tab_to_workspace(win_id, tab_id, workspace_id);
            }
            M::ToggleMuteTab { tab_id } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    let next = win
                        .tabs
                        .get_tab_by_id(tab_id)
                        .map(|t| !t.is_muted)
                        .unwrap_or(false);
                    if let Some(tab) = win.views.get(&tab_id) {
                        webview::set_muted(&tab.view, next);
                    }
                    win.tabs.set_tab_muted(tab_id, next);
                    win.ui_dirty = true;
                    win.sidebar_dirty = true;
                }
            }
            M::LinkDropped { url } => {
                // Any page can send this, so it only ever opens a web address.
                let Ok(url) = normalize_url(&url, self.settings.search_url_template()) else {
                    return;
                };
                if !(url.starts_with("https://") || url.starts_with("http://"))
                    || is_internal_origin(&url)
                {
                    return;
                }
                let in_split = self
                    .windows
                    .get(&win_id)
                    .is_some_and(|w| w.tabs.visible_split().is_some());
                if in_split {
                    if let Some(target_pane) = from_tab {
                        if let Some(win) = self.windows.get_mut(&win_id) {
                            win.show_tab(target_pane);
                        }
                        self.navigate(win_id, &url);
                    }
                } else {
                    self.create_tab(win_id, Some(url));
                }
            }
            M::RestoreSession => self.restore_session(win_id, event_loop),
            M::DismissSession => {
                self.pending_session = None;
                if let Ok(mut ctx) = self.pages.write() {
                    ctx.restore_tabs = 0;
                }
                self.push_data();
            }
            M::SiteChoicesQuery => {
                let json = serde_json::to_string(&self.site_data.entries()).unwrap_or_default();
                self.eval_welcome_and_settings(
                    win_id,
                    &format!("window.__siteChoices && window.__siteChoices({json})"),
                );
            }
            M::ForgetSiteChoice { host, kind } => {
                if self.site_data.forget(&host, kind.as_deref()) {
                    self.push_data();
                }
            }
            M::ExtensionsQuery => self.request_extension_list(),
            M::AddExtension => self.add_extension_from_folder(),
            M::InstallExtension { source } => self.install_extension_from_store(&source),
            M::RemoveExtension { id } => {
                if let Some(view) = self.persistent_view() {
                    webview::remove_extension(view, id, self.proxy.clone());
                }
            }
            M::ClearSiteData => {
                self.site_data.clear();
                self.push_data();
            }
            M::ClearSavedLogins => {
                if let Some(tab) = self.windows.get(&win_id).and_then(|w| w.active()) {
                    webview::clear_saved_logins(&tab.view);
                }
            }
            M::UninstallShoji { confirm } => {
                if confirm.trim() == shoji_core::ipc::UNINSTALL_CONFIRMATION
                    && start_uninstall(&self.data_dir)
                {
                    // Exit at once, without the usual save-on-close, so nothing is written back.
                    std::process::exit(0);
                }
            }
            M::PageInfo { color, has_video } => {
                if let Some(tab_id) = from_tab {
                    let tint = color.as_deref().and_then(parse_css_color);
                    if let Some(tab) = self
                        .windows
                        .get_mut(&win_id)
                        .and_then(|w| w.views.get_mut(&tab_id))
                    {
                        tab.tint = tint;
                        tab.has_video = has_video;
                    }
                }
            }
            M::ContinueInsecure { tab_id, host, url } => {
                if url.starts_with("http://")
                    && extract_host(&url).as_deref() == Some(host.as_str())
                {
                    if let Ok(mut set) = self.insecure_hosts.lock() {
                        set.insert(host);
                    }
                    if let Some(win) = self.windows.get_mut(&win_id) {
                        win.tabs.update_url(tab_id, url.clone());
                        win.ui_dirty = true;
                        if let Some(tab) = win.views.get(&tab_id) {
                            let _ = tab.view.load_url(&url);
                        }
                    }
                }
            }
            M::ContinueBlockedPage { tab_id, host, url } => {
                if (url.starts_with("http://") || url.starts_with("https://"))
                    && extract_host(&url).as_deref() == Some(host.as_str())
                {
                    self.shield.allow_page_host(&host);
                    if let Some(win) = self.windows.get_mut(&win_id) {
                        win.tabs.update_url(tab_id, url.clone());
                        win.ui_dirty = true;
                        if let Some(tab) = win.views.get(&tab_id) {
                            let _ = tab.view.load_url(&url);
                        }
                    }
                }
            }
            M::BypassCertificateError { tab_id, host, url } => {
                if let Ok(mut set) = self.allowed_cert_hosts.lock() {
                    set.insert(host);
                }
                if let Some(tab) = self.windows.get(&win_id).and_then(|w| w.views.get(&tab_id)) {
                    let _ = tab.view.load_url(&url);
                }
            }
        }
    }

    fn handle_command(&mut self, win_id: WindowId, cmd: Command, event_loop: &ActiveEventLoop) {
        self.sync_pending = true;
        self.session_dirty = true;
        use UiToHostMessage as M;
        let ipc = |app: &mut Self, msg| app.handle_ipc(win_id, None, msg, event_loop);
        match cmd {
            Command::NewTab => self.create_tab(win_id, None),
            Command::ReopenClosedTab => {
                let url = self
                    .windows
                    .get_mut(&win_id)
                    .and_then(|w| w.tabs.pop_last_closed());
                if let Some(u) = &url {
                    if let Ok(mut ctx) = self.pages.write() {
                        ctx.recent.retain(|c| &c.url != u);
                    }
                }
                if url.is_some() {
                    self.create_tab(win_id, url);
                    self.push_data();
                }
            }
            Command::NewWindow => self.new_window(event_loop, None),
            Command::NewPrivateWindow => {
                self.open_window(event_loop, None, true);
            }
            Command::BookmarkManager => self.open_internal(win_id, "bookmarks"),
            Command::BookmarksBar => {
                let on = !self.settings.appearance.bookmarks_bar;
                self.set_bookmarks_bar(on);
            }
            Command::BookmarkAllTabs => {
                self.run_action(win_id, "bookmark-all", event_loop);
            }
            Command::CloseTab => {
                if let Some(id) = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.tabs.active_tab_id())
                {
                    // Ctrl+W leaves a pinned tab alone; its menu and middle-click still close it.
                    let pinned = self
                        .windows
                        .get(&win_id)
                        .is_some_and(|w| w.tabs.get_tab_by_id(id).is_some_and(|t| t.pinned));
                    if !pinned {
                        self.close_tab(win_id, id, event_loop);
                    }
                }
            }
            Command::FocusAddress => {
                if let Some(win) = self.windows.get(&win_id) {
                    if let Some(c) = &win.chrome {
                        webview::focus(c);
                    }
                    win.eval_chrome("window.__focusOmnibox && window.__focusOmnibox()");
                }
            }
            Command::Find => ipc(self, M::ToggleFindBar),
            Command::Downloads => ipc(self, M::ToggleDownloadsSidebar),
            Command::Print => {
                if let Some(tab) = self.windows.get(&win_id).and_then(|w| w.active()) {
                    let _ = tab.view.print();
                }
            }
            Command::Reload | Command::HardReload => {
                // Five reloads in a few seconds earns the crane's sympathy.
                let now = Instant::now();
                self.reloads
                    .retain(|t| now.duration_since(*t) < Duration::from_secs(6));
                self.reloads.push(now);
                if self.reloads.len() >= 5 {
                    self.reloads.clear();
                    self.toast(&serde_json::json!({
                        "title": "It's not you.",
                        "text": "Probably them. Maybe try again in a minute?",
                        "pose": "tilt",
                    }));
                }
                self.reload(win_id, cmd == Command::HardReload);
            }
            Command::Back => ipc(self, M::GoBack),
            Command::Forward => ipc(self, M::GoForward),
            Command::NextTab | Command::PrevTab | Command::SelectTab(_) => {
                let Some(win) = self.windows.get_mut(&win_id) else {
                    return;
                };
                let tabs = win.tabs.tabs();
                if tabs.is_empty() {
                    return;
                }
                let current = win
                    .tabs
                    .active_tab_id()
                    .and_then(|a| tabs.iter().position(|t| t.id == a))
                    .unwrap_or(0);
                let n = tabs.len();
                let index = match cmd {
                    Command::NextTab => (current + 1) % n,
                    Command::PrevTab => (current + n - 1) % n,
                    Command::SelectTab(i) => i.min(n - 1),
                    _ => current,
                };
                let id = tabs[index].id;
                win.show_tab(id);
                win.layout();
            }
            Command::ZoomIn => ipc(self, M::ZoomIn),
            Command::ZoomOut => ipc(self, M::ZoomOut),
            Command::ZoomReset => ipc(self, M::ZoomReset),
            Command::DevTools => ipc(self, M::OpenDevTools),
            Command::Palette => ipc(self, M::OpenPalette),
            Command::ToggleFavourite => ipc(self, M::ToggleFavourite),
            Command::Reader => ipc(self, M::ToggleReader),
            Command::History => self.open_internal(win_id, "history"),
            Command::ClearData => {
                let open = self
                    .windows
                    .get(&win_id)
                    .is_some_and(|w| w.tabs.tabs().iter().any(|t| t.url == "shoji://history"));
                if open {
                    self.open_internal(win_id, "history");
                    self.eval_all_internal("window.__showClear && window.__showClear()");
                } else {
                    self.clear_pending = true;
                    self.open_internal(win_id, "history");
                }
            }
            Command::TabSearch => ipc(self, M::OpenPalette),
            Command::Settings => self.open_internal(win_id, "settings"),
            Command::NextTabPage => self.handle_command(win_id, Command::NextTab, event_loop),
            Command::PrevTabPage => self.handle_command(win_id, Command::PrevTab, event_loop),
            Command::FocusAddressF6 => {
                self.handle_command(win_id, Command::FocusAddress, event_loop)
            }
            Command::Home => self.navigate(win_id, "shoji://newtab"),
            Command::ViewSource => {
                let url = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.tabs.active_tab())
                    .map(|t| t.url.clone())
                    .unwrap_or_default();
                if url.starts_with("http://") || url.starts_with("https://") {
                    self.create_tab(win_id, Some(format!("view-source:{url}")));
                }
            }
            Command::SavePage => {
                if let Some(tab) = self.windows.get(&win_id).and_then(|w| w.active()) {
                    webview::save_page(&tab.view);
                }
            }
            Command::FullScreen => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.fullscreen = !win.fullscreen;
                    let mode = win
                        .fullscreen
                        .then_some(winit::window::Fullscreen::Borderless(None));
                    win.window.set_fullscreen(mode);
                    if let Some(c) = &win.chrome {
                        let _ = c.set_visible(!win.fullscreen);
                    }
                    win.layout();
                }
            }
            Command::FindNext | Command::FindPrev => {
                let forward = cmd == Command::FindNext;
                let open = self.windows.get(&win_id).is_some_and(|w| w.find_open);
                if open {
                    let query = self
                        .windows
                        .get(&win_id)
                        .map(|w| w.find_query.clone())
                        .unwrap_or_default();
                    if let Some(win) = self.windows.get(&win_id) {
                        win.find(&query, forward);
                    }
                } else {
                    ipc(self, M::ToggleFindBar);
                }
            }
            Command::ToggleSplit => ipc(self, M::ToggleSplit),
            Command::ToggleSplitOrientation => ipc(self, M::ToggleSplitOrientation),
            Command::SwitchSplitPane => ipc(self, M::SwitchSplitPane),
            Command::Workspaces => ipc(self, M::ToggleWorkspacesSidebar),
            Command::NewWorkspace => {
                self.open_prompt(win_id, Prompt::NewWorkspace, "Name this workspace", "");
            }
            // Inside a split, Alt+1..9 picks the pane (Alt+9 the last one); elsewhere a space.
            Command::SelectWorkspace(i)
                if self
                    .windows
                    .get(&win_id)
                    .is_some_and(|w| w.tabs.visible_split().is_some()) =>
            {
                let last = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.tabs.visible_split().map(|s| s.panes.len() - 1))
                    .unwrap_or(0);
                ipc(self, M::SplitFocus { index: i.min(last) });
            }
            Command::SelectWorkspace(i) => {
                let ws_id = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.tabs.workspaces().get(i).map(|ws| ws.id));
                if let Some(ws_id) = ws_id {
                    self.switch_workspace(win_id, ws_id);
                }
            }
        }
    }

    fn new_window(&mut self, event_loop: &ActiveEventLoop, url: Option<String>) {
        self.open_window(event_loop, url, false);
    }

    /// A window with one tab. A private window keeps nothing once it is closed.
    fn open_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        url: Option<String>,
        private: bool,
    ) -> Option<WindowId> {
        let id = self.create_window(event_loop, private)?;
        self.create_tab(id, url);
        if let Some(win) = self.windows.get(&id) {
            win.window.set_visible(true);
        }
        Some(id)
    }

    /// Push pending tab-strip and side-panel state, at most once per event-loop turn.
    fn flush(&mut self) {
        let proxy = &self.proxy;
        let app_event = std::mem::take(&mut self.sync_pending);
        let containers = self.containers_for_ui();
        for win in self.windows.values_mut() {
            // Page-derived state below only changes through app events or a strip change; a
            // batch of mouse moves skips it.
            let page_sync = app_event || win.ui_dirty;
            if win.ui_dirty {
                win.ui_dirty = false;
                self.session_dirty = true;
                let msg = HostToUiMessage::TabStateSync {
                    tabs: win.tabs.tabs().to_vec(),
                    active_tab_id: win.tabs.active_tab_id(),
                    groups: win.tabs.groups().to_vec(),
                    split: win.tabs.split_panes(),
                    split_state: win.tabs.visible_split().cloned(),
                    splits: win.tabs.splits().to_vec(),
                    workspaces: win.tabs.workspaces().to_vec(),
                    active_workspace_id: win.tabs.active_workspace_id(),
                    inactive_tabs: win.tabs.inactive_tabs().to_vec(),
                    containers: containers.clone(),
                };
                let title = match win.tabs.active_tab() {
                    Some(t) if !is_internal(&t.url) && !t.title.is_empty() => {
                        format!("{} — {}", t.title, shoji_core::APP_NAME)
                    }
                    _ => shoji_core::APP_NAME.to_string(),
                };
                let title = if win.private {
                    format!("{title} (Private)")
                } else {
                    title
                };
                if title != win.last_title {
                    win.window.set_title(&title);
                    win.last_title = title;
                }
                if let Ok(json) = serde_json::to_string(&msg) {
                    if json != win.last_ui_json {
                        let js = format!("window.__shellUpdate && window.__shellUpdate({json})");
                        win.eval_chrome(&js);
                        if win.sidebar_open {
                            win.eval_sidebar(&js);
                        }
                        win.last_ui_json = json;
                        // The toolbar now knows its tabs: a window on the new tab page starts with
                        // the address bar active, so typing goes straight into it.
                        if std::mem::take(&mut win.focus_address)
                            && win
                                .tabs
                                .active_tab()
                                .is_some_and(|t| is_new_tab_page(&t.url))
                        {
                            if let Some(c) = &win.chrome {
                                webview::focus(c);
                            }
                            win.eval_chrome("window.__focusOmnibox && window.__focusOmnibox()");
                        }
                    }
                }
            }
            if win.sidebar_dirty && win.sidebar_open {
                win.sidebar_dirty = false;
                let url = win
                    .tabs
                    .active_tab()
                    .map(|t| t.url.clone())
                    .unwrap_or_default();
                let persistent = win.active().is_some_and(|t| t.persistent);
                let info = self.cert_cache.describe(
                    &url,
                    persistent,
                    win.sidebar_mode == "security",
                    proxy,
                );
                let msg = HostToUiMessage::SidebarStateSync {
                    open: true,
                    mode: win.sidebar_mode.to_string(),
                    security_info: Some(Box::new(info)),
                };
                if let Ok(json) = serde_json::to_string(&msg) {
                    win.eval_sidebar(&format!(
                        "window.__sidebarSync && window.__sidebarSync({json})"
                    ));
                }
                if win.sidebar_mode == "dark" {
                    let host = extract_host(&url).unwrap_or_default();
                    let json = pages::dark_panel_json(&self.settings, &host);
                    win.eval_sidebar(&format!("window.__darkPanel && window.__darkPanel({json})"));
                }
                if !win.last_ui_json.is_empty() {
                    win.eval_sidebar(&format!(
                        "window.__shellUpdate && window.__shellUpdate({})",
                        win.last_ui_json
                    ));
                }
            }
            if !page_sync {
                continue;
            }
            // Page-derived toolbar state: tint colour, picture-in-picture, reader, favourite star.
            let active = win.tabs.active_tab_id();
            let (tint, video) = active
                .and_then(|id| win.views.get(&id))
                .map(|t| (t.tint.clone(), t.has_video))
                .unwrap_or((None, false));
            let reader = active.is_some_and(|id| win.reader.contains(&id));
            let page = format!("{tint:?}|{video}|{reader}");
            if page != win.last_page_sync {
                win.last_page_sync = page;
                let tint_js = tint
                    .as_deref()
                    .map(js_string)
                    .unwrap_or_else(|| "null".into());
                win.eval_chrome(&format!(
                    "window.__tint && window.__tint({tint_js}); window.__syncPage && window.__syncPage({{hasVideo:{video},reader:{reader}}})"
                ));
            }
            // Byobu: the toolbar shield and (when open) its side-panel tab.
            {
                let (tab, url) = win
                    .tabs
                    .active_tab()
                    .map(|t| (t.id.0, t.url.clone()))
                    .unwrap_or_default();
                let host = extract_host(&url).unwrap_or_default();
                let state = if url.is_empty() || is_internal(&url) {
                    "none"
                } else if self.settings.shield.mode == "off" {
                    "off"
                } else if self.settings.shield_mode_for(&url) == "off" {
                    "paused"
                } else {
                    "on"
                };
                let json = shield::hub().tab_json(tab, &host, state);
                let key = u64::from(win.window.id());
                if shield::hub().changed(&format!("c{key}"), &json) {
                    win.eval_chrome(&format!(
                        "window.__shieldSync && window.__shieldSync({json})"
                    ));
                }
                if win.sidebar_open && win.sidebar_mode == "shield" {
                    let paused = serde_json::to_string(&self.settings.shield.paused_sites)
                        .unwrap_or_default();
                    let js =
                        format!("window.__shieldPanel && window.__shieldPanel({json}, {paused})");
                    if shield::hub().changed(&format!("s{key}"), &js) {
                        win.eval_sidebar(&js);
                    }
                } else {
                    shield::hub().forget_sync(&format!("s{key}"));
                }
            }
            // Each pane of a split has its own shield, reader and video buttons.
            if let Some(split) = win.tabs.visible_split() {
                let panes: Vec<serde_json::Value> = split
                    .panes
                    .iter()
                    .filter_map(|&id| {
                        let t = win.tabs.get_tab_by_id(id)?;
                        let state = if t.url.is_empty() || is_internal(&t.url) {
                            "none"
                        } else if self.settings.shield.mode == "off" {
                            "off"
                        } else if self.settings.shield_mode_for(&t.url) == "off" {
                            "paused"
                        } else {
                            "on"
                        };
                        let (blocked, replaced) = shield::hub().counts(id.0);
                        Some(serde_json::json!({
                            "id": id.0,
                            "shield": state,
                            "n": blocked + replaced,
                            "video": win.views.get(&id).is_some_and(|v| v.has_video),
                            "reader": win.reader.contains(&id),
                        }))
                    })
                    .collect();
                let json = serde_json::Value::Array(panes).to_string();
                if json != win.last_pane_state {
                    win.eval_chrome(&format!("window.__paneState && window.__paneState({json})"));
                    win.last_pane_state = json;
                }
            } else if !win.last_pane_state.is_empty() {
                win.last_pane_state.clear();
            }
            let fav = win
                .tabs
                .active_tab()
                .is_some_and(|t| self.favourites.contains(&t.url));
            let fav_state = fav.to_string();
            if fav_state != win.last_fav_sync {
                win.last_fav_sync = fav_state;
                win.eval_chrome(&format!("window.__syncFav && window.__syncFav({fav})"));
            }
        }
    }

    /// Suspend background tabs that have been hidden long enough; returns the next deadline.
    fn rest_background_tabs(&mut self) -> Option<Instant> {
        let after = self.settings.performance.sleep_after_secs;
        if after == 0 {
            return None;
        }
        let after = Duration::from_secs(after);
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        for win in self.windows.values_mut() {
            for (id, tab) in win.views.iter_mut() {
                let Some(since) = tab.hidden_since.filter(|_| !tab.suspended) else {
                    continue;
                };
                let due = since + after;
                if due <= now {
                    webview::try_suspend(&tab.view, *id, self.proxy.clone());
                    // If the page declines (audio, locks), try again after another interval.
                    tab.hidden_since = Some(now);
                    next = Some(next.map_or(now + after, |n| n.min(now + after)));
                } else {
                    next = Some(next.map_or(due, |n| n.min(due)));
                }
            }
        }
        next
    }

    /// The WebView2 browser process died: every view in this app is gone. Rebuild them.
    fn restart_engine(&mut self) {
        if self
            .last_engine_restart
            .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
        {
            return;
        }
        self.last_engine_restart = Some(Instant::now());
        let ids: Vec<WindowId> = self.windows.keys().copied().collect();
        for win_id in ids {
            let Some(win) = self.windows.get_mut(&win_id) else {
                continue;
            };
            win.sidebar = None;
            win.sidebar_open = false;
            win.pop_hide_at = None;
            win.overlay = None;
            win.overlay_open = false;
            win.overlay_ready = false;
            let window = win.window.clone();
            let tabs: Vec<(TabId, String, Option<String>)> = win
                .tabs
                .tabs()
                .iter()
                .chain(win.tabs.inactive_tabs().iter())
                .map(|t| (t.id, t.url.clone(), t.container.clone()))
                .collect();
            let active = win.tabs.active_tab_id();
            let shown: HashSet<TabId> = active
                .into_iter()
                .chain(win.tabs.split_panes().into_iter().flatten())
                .collect();
            win.views.clear();
            win.chrome = None;
            let rect = win.content_bounds();
            // As on restore: only the tabs on screen load now, the rest when first shown.
            for (id, url, container) in tabs {
                let now = shown.contains(&id);
                if let Some(mut tab) = self.build_view(&window, id, &url, rect, now, container) {
                    if !now {
                        tab.pending_url = Some(url);
                    }
                    let _ = tab.view.set_visible(false);
                    if let Some(win) = self.windows.get_mut(&win_id) {
                        win.views.insert(id, tab);
                    }
                }
            }
            let dark = self.is_dark(&window);
            let chrome = new_builder()
                .with_html(pages::chrome(
                    &self.settings,
                    self.windows.get(&win_id).is_some_and(|w| w.private),
                ))
                .with_custom_protocol("shoji".into(), |_, request| font_response(&request))
                .with_incognito(true)
                .with_background_color(window::page_rgba(dark))
                .with_theme(window::webview_theme(&self.settings.appearance.theme))
                .with_ipc_handler({
                    let proxy = self.proxy.clone();
                    move |req: wry::http::Request<String>| {
                        if let Ok(msg) = serde_json::from_str(req.body()) {
                            let _ = proxy.send_event(BrowserEvent::WindowIpc(win_id, msg));
                        }
                    }
                })
                .build_as_child(window.as_ref())
                .ok();
            if let Some(win) = self.windows.get_mut(&win_id) {
                if let Some(c) = &chrome {
                    webview::attach_accelerators(c, Origin::Window(win_id), self.proxy.clone());
                }
                win.chrome = chrome;
                win.last_ui_json.clear();
                win.ui_dirty = true;
                if let Some(a) = active {
                    win.show_tab(a);
                }
                win.layout();
            }
        }
    }
}

impl ApplicationHandler<BrowserEvent> for BrowserApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if !self.windows.is_empty() {
            return;
        }
        // `--background` (used after an automatic update) starts straight into the tray.
        if std::env::args().any(|a| a == "--background")
            && self.settings.first_run_complete
            && self.settings.performance.run_in_background
            && !self.settings.privacy.ephemeral_default
        {
            if let Some(tray) = &self.tray {
                tray.show();
                return;
            }
        }
        let first = if self.settings.first_run_complete {
            std::env::args()
                .skip(1)
                .find(|a| !a.starts_with('-'))
                .and_then(|a| normalize_url(&a, self.settings.search_url_template()).ok())
        } else {
            Some(format!("{INTERNAL_SCHEME}welcome"))
        };
        self.new_window(event_loop, first);
        self.note_ui_window();
        if self.windows.is_empty() {
            event_loop.exit();
        } else if self.restore_on_start {
            self.restore_on_start = false;
            if let Some(id) = self.windows.keys().next().copied() {
                self.restore_session(id, event_loop);
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: BrowserEvent) {
        let _doing = crash::doing(crash::intern(match &event {
            BrowserEvent::TabIpc(_, m) | BrowserEvent::WindowIpc(_, m) => {
                format!("{} {}", crash::variant_name(&event), crash::variant_name(m))
            }
            _ => crash::variant_name(&event),
        }));
        self.sync_pending = true;
        self.session_dirty = true;
        match event {
            BrowserEvent::Tray(tray::Action::Open(url)) => self.reopen(event_loop, url),
            BrowserEvent::Tray(tray::Action::Quit) => self.quit(event_loop),
            BrowserEvent::ImportProgress { win, text } => {
                self.eval_welcome_and_settings(
                    win,
                    &format!(
                        "window.__importProgress && window.__importProgress({})",
                        js_string(&text)
                    ),
                );
            }
            BrowserEvent::ImportDone { win, imported } => {
                let imported = *imported;
                let favourites = self.favourites.add_many(&imported.favourites);
                let history = self.history.merge(imported.history);
                let cookies = imported.cookies.len();
                // Saved passwords go to the password manager (a master-password vault must be open).
                let mut passwords = 0;
                let mut vault_locked = false;
                if !imported.logins.is_empty() {
                    if self.vault_open(true) {
                        let (added, updated, _) = self.vault.import(&imported.logins, now_ms());
                        passwords = added + updated;
                        self.vault_changed();
                    } else {
                        vault_locked = true;
                    }
                }
                if cookies > 0 {
                    if let Ok(mut pending) = self.pending_cookies.lock() {
                        pending.extend(imported.cookies);
                    }
                    // A tab that already uses the real profile can take them now.
                    let view = self.windows.values().find_map(|w| {
                        w.tabs
                            .tabs()
                            .iter()
                            .find(|t| !is_internal(&t.url))
                            .and_then(|t| w.views.get(&t.id))
                    });
                    if let Some(tab) = view {
                        self.apply_pending_cookies(&tab.view);
                    }
                }
                self.push_data();
                let r = imported.report;
                let mut notes = r.notes;
                if vault_locked {
                    notes.push("Passwords weren't imported: unlock Shoji's password manager first, then import again".into());
                }
                let summary = serde_json::json!({
                    "favourites": favourites,
                    "history": history,
                    "cookies": cookies,
                    "passwords": passwords,
                    "notes": notes,
                    "locked": r.locked,
                });
                self.eval_welcome_and_settings(
                    win,
                    &format!("window.__importDone && window.__importDone({summary})"),
                );
            }
            BrowserEvent::UpdateChecked { result, manual } => {
                use update::Phase;
                if self.update.phase == Phase::Checking {
                    self.update.phase = Phase::Idle;
                }
                self.update.manual = manual;
                match result {
                    Ok(found) => {
                        self.update.checked_at = now_ms();
                        self.update.error = None;
                        // A release already agreed to keeps its course until it is installed.
                        let settled =
                            matches!(self.update.phase, Phase::Downloading | Phase::Ready);
                        if let Some(release) = found.filter(|_| !settled) {
                            let known = self.update.release.as_ref().map(|r| r.version.as_str());
                            if known != Some(release.version.as_str()) {
                                self.update_unfold_pending = true;
                            }
                            self.update.release = Some(release);
                        }
                    }
                    Err(_) => {
                        if manual {
                            self.update.error = Some(
                                "Couldn't reach GitHub. Check the connection and try again.".into(),
                            );
                        }
                    }
                }
                self.maybe_unfold_letter();
                self.push_update();
            }
            BrowserEvent::UpdateProgress { pct } => {
                self.update.pct = pct;
                self.push_update();
            }
            BrowserEvent::UpdateDownloaded { version, result } => {
                if self.update.release.as_ref().map(|r| r.version.as_str())
                    == Some(version.as_str())
                {
                    match result {
                        Ok(installer) => {
                            self.update.installer = Some(installer);
                            self.update.phase = update::Phase::Ready;
                            self.update.pct = 100;
                            self.proceed_update();
                        }
                        Err(e) => {
                            self.update.phase = update::Phase::Idle;
                            self.update.consent = update::Consent::None;
                            // A bad checksum is worth naming; a dropped connection isn't.
                            self.update.error = Some(if e.contains("checksum") {
                                "The download didn't match its published checksum, so it was thrown away. Try again later.".into()
                            } else {
                                "The download didn't arrive. Try again later.".into()
                            });
                        }
                    }
                    self.push_update();
                }
            }
            BrowserEvent::WindowIpc(win_id, msg) => self.handle_ipc(win_id, None, msg, event_loop),
            BrowserEvent::TabIpc(tab_id, msg) => {
                if let Some(win_id) = self.owner(tab_id) {
                    self.handle_ipc(win_id, Some(tab_id), msg, event_loop);
                }
            }
            BrowserEvent::Command(origin, cmd) => {
                let win_id = match origin {
                    Origin::Window(id) => Some(id),
                    Origin::Tab(tab) => self.owner(tab),
                };
                if let Some(win_id) = win_id {
                    self.handle_command(win_id, cmd, event_loop);
                }
            }
            BrowserEvent::TabUpdate {
                tab_id,
                url,
                title,
                history,
                favicon,
            } => {
                let Some(win) = self.owner(tab_id).and_then(|id| self.windows.get_mut(&id)) else {
                    return;
                };
                // A restored tab that hasn't loaded yet keeps its saved address and title.
                if win
                    .views
                    .get(&tab_id)
                    .is_some_and(|t| t.pending_url.is_some())
                {
                    return;
                }
                // Tabs in other spaces keep loading (and redirecting) too.
                let Some(state) = win.tabs.get_tab_by_id_mut(tab_id) else {
                    return;
                };
                let mut changed = false;
                let mut moved: Option<(String, String)> = None;
                if let Some(url) = url {
                    // Notice pages keep showing the address that failed.
                    if url.starts_with("http://shoji.notice-") {
                        if !state.notice {
                            state.notice = true;
                            changed = true;
                        }
                    } else {
                        if state.notice {
                            state.notice = false;
                            changed = true;
                        }
                        let url = display_url(&url);
                        // A page the host navigated to already has its address on the tab, so
                        // record visits by what the engine reports, once per address.
                        if !self.settings.privacy.ephemeral_default
                            && !win.private
                            && !is_internal(&url)
                            && !url.starts_with("view-source:")
                            && win.recorded.get(&tab_id).map(String::as_str) != Some(&*url)
                        {
                            win.recorded.insert(tab_id, url.to_string());
                            let now = std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map_or(0, |d| d.as_millis() as u64);
                            self.history.record(&url, &state.title, now);
                        }
                        if state.url != url {
                            if is_internal(&url) != is_internal(&state.url) {
                                state.favicon_uri = None;
                            }
                            moved = Some((state.url.clone(), url.to_string()));
                            if let Some(tab) = win.views.get_mut(&tab_id) {
                                tab.tint = None;
                                tab.has_video = false;
                            }
                            win.reader.remove(&tab_id);
                            state.url = url.into_owned();
                            if state.status == TabStatus::Crashed {
                                state.status = TabStatus::Active;
                            }
                            changed = true;
                            win.sidebar_dirty = true;
                        }
                    }
                }
                if let Some(title) = title {
                    if state.title != title {
                        if !self.settings.privacy.ephemeral_default
                            && !win.private
                            && !is_internal(&state.url)
                        {
                            self.history.set_title(&state.url, &title);
                        }
                        state.title = title;
                        changed = true;
                    }
                }
                if let Some((back, fwd)) = history {
                    if (state.can_go_back, state.can_go_forward) != (back, fwd) {
                        state.can_go_back = back;
                        state.can_go_forward = fwd;
                        changed = true;
                    }
                }
                if let Some(icon) = favicon {
                    if state.favicon_uri != icon {
                        state.favicon_uri = icon;
                        changed = true;
                    }
                }
                win.ui_dirty |= changed;
                // A different site: use its remembered zoom (or the default).
                if let Some((old, new)) = moved {
                    if extract_host(&old) != extract_host(&new) && extract_host(&new).is_some() {
                        let zoom = self.zoom_for(&new);
                        if let Some(win) =
                            self.owner(tab_id).and_then(|id| self.windows.get_mut(&id))
                        {
                            if let Some(tab) = win.views.get(&tab_id) {
                                let _ = unsafe { tab.view.controller().SetZoomFactor(zoom) };
                            }
                            if win.tabs.active_tab_id() == Some(tab_id) {
                                win.sync_zoom();
                            }
                        }
                    }
                }
            }
            BrowserEvent::TabAudio {
                tab_id,
                playing,
                muted,
            } => {
                if let Some(win) = self.owner(tab_id).and_then(|id| self.windows.get_mut(&id)) {
                    win.tabs.set_audio_playing(tab_id, playing);
                    win.tabs.set_tab_muted(tab_id, muted);
                    win.ui_dirty = true;
                    win.sidebar_dirty = true;
                }
            }
            BrowserEvent::TabFullscreen { tab_id, fullscreen } => {
                if let Some(win) = self.owner(tab_id).and_then(|id| self.windows.get_mut(&id)) {
                    win.fullscreen_tab = fullscreen.then_some(tab_id);
                    win.window.set_fullscreen(
                        fullscreen.then_some(winit::window::Fullscreen::Borderless(None)),
                    );
                    win.layout();
                    win.sync_frame();
                }
            }
            BrowserEvent::PopupRequested(req) => self.open_popup(req),
            BrowserEvent::NewTabFrom(tab_id, url) => {
                if let Some(win_id) = self.owner(tab_id) {
                    let url = normalize_url(&url, self.settings.search_url_template()).ok();
                    if url.is_some() {
                        self.create_tab(win_id, url);
                    }
                }
            }
            BrowserEvent::PermissionPrompt {
                tab_id,
                permission_id,
                origin,
                kind,
            } => match self.settings.permission_policy(kind) {
                "allow" => webview::resolve_permission(permission_id, true),
                "block" => webview::resolve_permission(permission_id, false),
                _ if self.site_data.permission(&origin, kind).is_some() => {
                    let allow = self.site_data.permission(&origin, kind).unwrap_or(false);
                    webview::resolve_permission(permission_id, allow)
                }
                _ => {
                    let Some(win) = self.owner(tab_id).and_then(|id| self.windows.get(&id)) else {
                        return webview::resolve_permission(permission_id, false);
                    };
                    self.perm_pending
                        .insert(permission_id, (origin.clone(), kind));
                    let json = serde_json::json!({
                        "permission_id": permission_id,
                        "origin": origin,
                        "permission_kind": kind,
                    });
                    win.eval_chrome(&format!(
                        "window.__permissionPrompt && window.__permissionPrompt({json})"
                    ));
                }
            },
            BrowserEvent::Bounce { host, hop } => {
                let Some((site, host)) = webview::site_of(&format!("https://{host}/")) else {
                    return;
                };
                if hop {
                    self.bounce.hop(&site, &host, now_secs());
                } else {
                    self.bounce.visit(&site, now_secs());
                }
                self.bounce.save(&self.data_dir.join("bounce.json"));
            }
            BrowserEvent::OpenInContainer {
                tab_id,
                url,
                container,
            } => {
                let Some(win_id) = self.owner(tab_id) else {
                    return;
                };
                // A fresh tab (nothing to go back to) moves; otherwise the site opens beside it.
                let fresh = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.tabs.get_tab_by_id(tab_id))
                    .is_some_and(|t| !t.can_go_back);
                if fresh {
                    self.reopen_in_container(win_id, tab_id, Some(url), container);
                } else {
                    self.create_tab_in(win_id, url, true, container);
                }
            }
            BrowserEvent::TabCrashed(tab_id, kind) => {
                let runtime = self
                    .pages
                    .read()
                    .map(|c| c.engine_version.clone())
                    .unwrap_or_default();
                crash::log_engine_failure(&self.data_dir, kind, &runtime);
                // 0 = browser process exited (all views are dead); 1/2 = this page's renderer.
                if kind == 0 {
                    return self.restart_engine();
                }
                if kind != 1 && kind != 2 {
                    return; // GPU / utility processes restart on their own
                }
                let Some(win) = self.owner(tab_id).and_then(|id| self.windows.get_mut(&id)) else {
                    return;
                };
                win.tabs.mark_tab_crashed(tab_id);
                win.ui_dirty = true;
                if let Ok(mut ctx) = self.pages.write() {
                    ctx.notices
                        .insert(tab_id.0, pages::crash(&self.settings, tab_id.0));
                }
                if let Some(tab) = win.views.get(&tab_id) {
                    let _ = tab
                        .view
                        .load_url(&format!("{INTERNAL_ORIGIN}notice-{}/", tab_id.0));
                }
            }
            BrowserEvent::CertificateError { tab_id, host, uri } => {
                let Some(win) = self.owner(tab_id).and_then(|id| self.windows.get(&id)) else {
                    return;
                };
                if let Ok(mut ctx) = self.pages.write() {
                    ctx.notices.insert(
                        tab_id.0,
                        pages::certificate_error(&self.settings, tab_id.0, &host, &uri),
                    );
                }
                if let Some(tab) = win.views.get(&tab_id) {
                    let _ = tab
                        .view
                        .load_url(&format!("{INTERNAL_ORIGIN}notice-{}/", tab_id.0));
                }
            }
            BrowserEvent::CertificateReady(host) => {
                for win in self.windows.values_mut() {
                    if win
                        .tabs
                        .active_tab()
                        .and_then(|t| extract_host(&t.url))
                        .as_deref()
                        == Some(host.as_str())
                    {
                        win.sidebar_dirty = true;
                    }
                }
            }
            BrowserEvent::DownloadProgress {
                download_id,
                filename,
                path,
                source,
                received_bytes,
                total_bytes,
                state,
            } => {
                let now_ms = now_secs() * 1000;
                let key = match self.dl_keys.get(&download_id) {
                    Some(k) => *k,
                    None => {
                        let k = self.downloads.start(&filename, &path, &source, now_ms);
                        self.dl_keys.insert(download_id, k);
                        k
                    }
                };
                if state != "InProgress" {
                    let ended = if state == "Completed" {
                        "done"
                    } else {
                        "stopped"
                    };
                    self.downloads
                        .finish(key, ended, received_bytes.max(0) as u64);
                    self.dl_keys.remove(&download_id);
                    self.push_downloads();
                    if state == "Completed" {
                        self.fold_crane();
                        let toast = serde_json::json!({
                            "title": filename,
                            "text": "Saved to your downloads",
                            "pose": "bow",
                            "actions": [
                                {"label": "Open", "action": "DownloadAction", "payload": {"key": key, "action": "open"}, "primary": true},
                                {"label": "Show", "action": "DownloadAction", "payload": {"key": key, "action": "show"}},
                            ],
                        });
                        self.toast(&toast);
                    }
                } else if received_bytes == 0 {
                    self.push_downloads();
                }
                let json = serde_json::json!({
                    "key": key,
                    "download_id": download_id,
                    "filename": filename,
                    "received_bytes": received_bytes,
                    "total_bytes": total_bytes,
                    "state": state,
                });
                let chrome_js =
                    format!("window.__downloadProgress && window.__downloadProgress({json})");
                let sidebar_js =
                    format!("window.__syncDownloads && window.__syncDownloads({json})");
                for win in self.windows.values() {
                    win.eval_chrome(&chrome_js);
                    win.eval_sidebar(&sidebar_js);
                }
            }
            BrowserEvent::Unreachable {
                tab_id,
                uri,
                offline,
            } => {
                let Some(host) = extract_host(&uri) else {
                    return;
                };
                // HTTPS-only mode's own fallback page takes this one's place just after an upgrade.
                let upgrading = self.settings.privacy.https_only
                    && self
                        .upgraded
                        .get(&host)
                        .is_some_and(|t| t.elapsed() < Duration::from_secs(60));
                if upgrading && offline {
                    return;
                }
                let Some(win) = self.owner(tab_id).and_then(|id| self.windows.get(&id)) else {
                    return;
                };
                if let Ok(mut ctx) = self.pages.write() {
                    ctx.notices.insert(
                        tab_id.0,
                        pages::unreachable(&self.settings, tab_id.0, &host, offline),
                    );
                }
                if let Some(tab) = win.views.get(&tab_id) {
                    let _ = tab
                        .view
                        .load_url(&format!("{INTERNAL_ORIGIN}notice-{}/", tab_id.0));
                }
            }
            BrowserEvent::TabLoading { tab_id, loading } => {
                if let Some(win) = self.owner(tab_id).and_then(|id| self.windows.get_mut(&id)) {
                    if !loading {
                        if let Some(tab) = win.views.get_mut(&tab_id) {
                            tab.painted = true;
                        }
                        if win.veil == Some(tab_id) {
                            win.settle_veil();
                        }
                    }
                    if let Some(state) = win.tabs.tabs_mut().iter_mut().find(|t| t.id == tab_id) {
                        if state.is_loading != loading {
                            state.is_loading = loading;
                            win.ui_dirty = true;
                        }
                    }
                }
            }
            BrowserEvent::ExtensionsListed(json) => {
                let on = EXTENSIONS_ON.load(std::sync::atomic::Ordering::Relaxed);
                // Their popup and options pages open as ordinary tabs.
                let json = {
                    let pages = extstore::pages(&self.data_dir.join("Extensions"));
                    let mut items: Vec<serde_json::Value> =
                        serde_json::from_str(&json).unwrap_or_default();
                    for item in &mut items {
                        let id = item["id"].as_str().unwrap_or("").to_string();
                        let name = item["name"].as_str().unwrap_or("").to_string();
                        if let Some((_, popup, options)) = pages.iter().find(|p| p.0 == name) {
                            let url = |p: &Option<String>| {
                                p.as_ref().map(|p| format!("chrome-extension://{id}/{p}"))
                            };
                            item["popup"] = serde_json::json!(url(popup));
                            item["options"] = serde_json::json!(url(options));
                        }
                    }
                    serde_json::Value::Array(items).to_string()
                };
                let js = format!(
                    "window.__extensions && window.__extensions({{active:{on},items:{json}}})"
                );
                let ids: Vec<WindowId> = self.windows.keys().copied().collect();
                for id in ids {
                    self.eval_welcome_and_settings(id, &js);
                }
            }
            BrowserEvent::SiteCookies(win_id, n, cleared) => {
                if let Some(win) = self.windows.get(&win_id) {
                    win.eval_sidebar(&format!(
                        "window.__siteCookies && window.__siteCookies({{n:{n},cleared:{cleared}}})"
                    ));
                    if cleared {
                        if let Some(tab) = win.active() {
                            let _ = tab.view.evaluate_script("location.reload()");
                        }
                    }
                }
            }
            BrowserEvent::SuggestWords(win_id, seq, words, overlay) => {
                if let Some(win) = self.windows.get(&win_id) {
                    if overlay {
                        if let Some(o) = &win.overlay {
                            let _ = o.evaluate_script(&format!(
                                "window.__paneWords && window.__paneWords({seq},{})",
                                serde_json::json!(words)
                            ));
                        }
                    } else {
                        win.eval_chrome(&format!(
                            "window.__suggestWords && window.__suggestWords({seq},{})",
                            serde_json::json!(words)
                        ));
                    }
                }
            }
            BrowserEvent::ExtensionFetched(result) => match result {
                Ok(dir) => match self.persistent_view() {
                    Some(view) => webview::add_extension(view, &dir, self.proxy.clone()),
                    None => self.toast(&serde_json::json!({
                        "title": "Open a website first.",
                        "text": "Extensions are added to the browsing profile.",
                        "pose": "unfold",
                    })),
                },
                Err(error) => {
                    let _ = self
                        .proxy
                        .send_event(BrowserEvent::ExtensionsChanged(Some(error)));
                }
            },
            BrowserEvent::ExtensionsChanged(error) => {
                if let Some(error) = error {
                    self.toast(&serde_json::json!({
                        "title": "That didn't work.",
                        "text": error,
                        "pose": "unfold",
                    }));
                }
                self.request_extension_list();
            }
            BrowserEvent::TabFocused(tab_id) => {
                if let Some(win) = self.owner(tab_id).and_then(|id| self.windows.get_mut(&id)) {
                    // Only a click into the other pane of the split that is on screen counts. A pane
                    // that is being hidden also reports focus, and must not pull the user back.
                    let on_screen = win
                        .tabs
                        .visible_split()
                        .is_some_and(|s| s.panes.contains(&tab_id));
                    if on_screen && win.tabs.active_tab_id() != Some(tab_id) {
                        win.show_tab(tab_id);
                    }
                }
            }
            BrowserEvent::NavFailed { tab_id, uri } => {
                let Some(host) = extract_host(&uri) else {
                    return;
                };
                let recent = self
                    .upgraded
                    .get(&host)
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(60));
                if !recent || !self.settings.privacy.https_only {
                    return;
                }
                self.upgraded.remove(&host);
                let Some(win) = self.owner(tab_id).and_then(|id| self.windows.get(&id)) else {
                    return;
                };
                let http_url = format!("http://{}", uri.trim_start_matches("https://"));
                if let Ok(mut ctx) = self.pages.write() {
                    ctx.notices.insert(
                        tab_id.0,
                        pages::insecure_fallback(&self.settings, tab_id.0, &host, &http_url),
                    );
                }
                if let Some(tab) = win.views.get(&tab_id) {
                    let _ = tab
                        .view
                        .load_url(&format!("{INTERNAL_ORIGIN}notice-{}/", tab_id.0));
                }
            }
            BrowserEvent::ReaderReady { tab_id, json } => {
                struct Article {
                    title: String,
                    byline: String,
                    site: String,
                    html: String,
                }
                let article = serde_json::from_str::<serde_json::Value>(&json)
                    .ok()
                    .and_then(|v| {
                        let field = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
                        Some(Article {
                            title: field("title")?,
                            byline: field("byline").unwrap_or_default(),
                            site: field("site").unwrap_or_default(),
                            html: field("html")?,
                        })
                    });
                let Some(win_id) = self.owner(tab_id) else {
                    return;
                };
                let Some(article) = article else {
                    if let Some(win) = self.windows.get(&win_id) {
                        win.eval_chrome("window.__noReader && window.__noReader()");
                    }
                    return;
                };
                let Some(win) = self.windows.get_mut(&win_id) else {
                    return;
                };
                let original = win
                    .tabs
                    .get_tab_by_id(tab_id)
                    .map(|t| t.url.clone())
                    .unwrap_or_default();
                let page = pages::reader(
                    &self.settings,
                    &article.title,
                    &article.byline,
                    &article.site,
                    &article.html,
                    &original,
                );
                if let Ok(mut ctx) = self.pages.write() {
                    ctx.notices.insert(tab_id.0, page);
                }
                win.reader.insert(tab_id);
                win.ui_dirty = true;
                if let Some(tab) = win.views.get(&tab_id) {
                    let _ = tab
                        .view
                        .load_url(&format!("{INTERNAL_ORIGIN}notice-{}/", tab_id.0));
                }
            }
            BrowserEvent::ShieldTouched(tab_id) => {
                shield::hub().touch_handled();
                // Counters are picked up by the next flush; tab 0 means the lists changed.
                if tab_id.0 == 0 {
                    for win in self.windows.values() {
                        win.eval_internal_tabs("window.__byobuDirty&&window.__byobuDirty()");
                    }
                }
            }
            BrowserEvent::ShieldBlockedPage { tab_id, url } => {
                let Some(win) = self.owner(tab_id).and_then(|id| self.windows.get(&id)) else {
                    return;
                };
                let host = extract_host(&url).unwrap_or_default();
                if let Ok(mut ctx) = self.pages.write() {
                    ctx.notices.insert(
                        tab_id.0,
                        pages::shield_blocked(&self.settings, tab_id.0, &host, &url),
                    );
                }
                if let Some(tab) = win.views.get(&tab_id) {
                    let _ = tab
                        .view
                        .load_url(&format!("{INTERNAL_ORIGIN}notice-{}/", tab_id.0));
                }
            }
            BrowserEvent::ShieldRule { host, selector } => {
                let line = format!("{host}##{selector}");
                self.update_settings(|s| {
                    if !s.shield.custom_rules.lines().any(|l| l.trim() == line) {
                        if !s.shield.custom_rules.is_empty()
                            && !s.shield.custom_rules.ends_with('\n')
                        {
                            s.shield.custom_rules.push('\n');
                        }
                        s.shield.custom_rules.push_str(&line);
                        s.shield.custom_rules.push('\n');
                    }
                });
            }
            BrowserEvent::TabSuspended(tab_id) => {
                if let Some(win) = self.owner(tab_id).and_then(|id| self.windows.get_mut(&id)) {
                    if !win.tabs.is_tab_visible(tab_id) {
                        if let Some(tab) = win.views.get_mut(&tab_id) {
                            tab.suspended = true;
                            tab.hidden_since = None;
                        }
                        win.tabs.set_tab_suspended(tab_id, true);
                        win.ui_dirty = true;
                        win.sidebar_dirty = true;
                    }
                }
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let _doing = crash::doing(crash::intern(format!(
            "window {}",
            crash::variant_name(&event)
        )));
        match event {
            WindowEvent::CloseRequested => self.close_window(window_id, event_loop),
            WindowEvent::Resized(size) => {
                if let Some(win) = self.windows.get_mut(&window_id) {
                    let logical = size.to_logical::<f64>(win.window.scale_factor());
                    win.width = logical.width;
                    win.height = logical.height;
                    win.layout();
                    win.sync_frame();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                if let Some(win) = self.windows.get_mut(&window_id) {
                    let logical = win
                        .window
                        .inner_size()
                        .to_logical::<f64>(win.window.scale_factor());
                    win.width = logical.width;
                    win.height = logical.height;
                    win.layout();
                    win.sync_frame();
                }
            }
            WindowEvent::ThemeChanged(theme) => {
                if self.settings.appearance.theme == "system" {
                    if let Some(win) = self.windows.get(&window_id) {
                        window::apply_frame(&win.window, theme == winit::window::Theme::Dark);
                    }
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if let Some(win) = self.windows.get_mut(&window_id) {
                    if win.divider_dragging && !left_button_down() {
                        win.end_divider_drag();
                    }
                }
            }
            WindowEvent::Focused(focused) => {
                if let Some(win) = self.windows.get_mut(&window_id) {
                    if !focused {
                        win.end_divider_drag();
                    }
                    win.sync_frame();
                    if focused {
                        if let Some(panes) = win.tabs.split_panes() {
                            for id in panes {
                                if let Some(tab) = win.views.get(&id) {
                                    webview::wake(&tab.view);
                                }
                            }
                        } else if let Some(tab) = win.active() {
                            webview::wake(&tab.view);
                        }
                    }
                }
                // A letter that arrived while no window had focus opens now.
                if focused && self.update_unfold_pending {
                    self.maybe_unfold_letter();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(win) = self.windows.get_mut(&window_id) {
                    let logical = position.to_logical::<f64>(win.window.scale_factor());
                    win.cursor_pos = (logical.x, logical.y);
                    if win.divider_dragging {
                        // Keep the resize cursor: the panes' own windows set theirs over them.
                        let horizontal = win.tabs.visible_split().map(|s| s.orientation)
                            == Some(SplitOrientation::Horizontal);
                        win.window.set_cursor(if horizontal {
                            winit::window::CursorIcon::NsResize
                        } else {
                            winit::window::CursorIcon::EwResize
                        });
                    } else {
                        let icon = match win.divider_hit(logical.x, logical.y) {
                            Some(_) => match win.tabs.visible_split().map(|s| s.orientation) {
                                Some(SplitOrientation::Horizontal) => {
                                    winit::window::CursorIcon::NsResize
                                }
                                _ => winit::window::CursorIcon::EwResize,
                            },
                            None => winit::window::CursorIcon::Default,
                        };
                        win.window.set_cursor(icon);
                    }
                }
            }
            WindowEvent::MouseInput {
                state,
                button: winit::event::MouseButton::Left,
                ..
            } => {
                if let Some(win) = self.windows.get_mut(&window_id) {
                    if state.is_pressed() {
                        let (x, y) = win.cursor_pos;
                        if let Some(hit) = win.divider_hit(x, y) {
                            if win
                                .last_divider_click
                                .is_some_and(|t| t.elapsed() < Duration::from_millis(350))
                            {
                                win.last_divider_click = None;
                                win.divider_dragging = false;
                                if win.tabs.reset_split_ratio() {
                                    win.layout();
                                    win.ui_dirty = true;
                                }
                            } else {
                                win.last_divider_click = Some(Instant::now());
                                win.divider_dragging = true;
                                win.divider_index = hit;
                                win.divider_grab = match win.divider_centre(hit) {
                                    Some((SplitOrientation::Horizontal, c)) => y - c,
                                    Some((_, c)) => x - c,
                                    None => 0.0,
                                };
                            }
                        }
                    } else {
                        win.end_divider_drag();
                    }
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::KeyboardInput { event, .. }
                if event.state.is_pressed() && !event.repeat =>
            {
                let m = self.modifiers;
                if let Some(cmd) = keys::vkey_from_winit(&event.logical_key).and_then(|vk| {
                    keys::command_for(vk, m.control_key(), m.shift_key(), m.alt_key())
                }) {
                    self.handle_command(window_id, cmd, event_loop);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let _doing = crash::doing("about_to_wait");
        self.note_ui_window();
        if Instant::now() >= self.next_bounce_clean {
            self.next_bounce_clean = Instant::now() + Duration::from_secs(6 * 3600);
            self.clean_bounce_trackers();
        }
        self.flush();
        let mut animating = self.step_tab_drag(event_loop);
        self.warm_spare(event_loop);
        let mut pop_due: Option<Instant> = None;
        for win in self.windows.values_mut() {
            if win.divider_dragging {
                win.drive_divider(tabdrag::cursor_pos());
                animating = true;
            }
            animating |= win.step_notes();
            pop_due = pop_due
                .into_iter()
                .chain(win.step_popover())
                .chain(win.settle_veil())
                .min();
        }
        if self.stores_flushed.elapsed() > Duration::from_secs(20) {
            self.stores_flushed = Instant::now();
            self.history.flush();
            self.cranes.flush();
            if !self.settings.privacy.ephemeral_default {
                self.downloads.flush();
            }
        }
        let session_due = self.maybe_write_session();
        let vault_due = self.vault_tick();
        let hover_due = self.step_hover();
        let warm_due = self.warm_sidebars();
        let next = self
            .rest_background_tabs()
            .into_iter()
            .chain(session_due)
            .chain(vault_due)
            .chain(pop_due)
            .chain(hover_due)
            .chain(warm_due)
            .chain(
                self.spare
                    .is_none()
                    .then_some(self.spare_due)
                    .filter(|due| *due > Instant::now()),
            )
            .min();
        // While something animates, wake for the next frame; otherwise sleep until needed.
        let frame = animating.then(|| Instant::now() + Duration::from_millis(8));
        event_loop.set_control_flow(match frame.into_iter().chain(next).min() {
            Some(deadline) => ControlFlow::WaitUntil(deadline),
            None => ControlFlow::Wait,
        });
    }
}

/// Start deleting Shoji and everything it stored, then let the caller exit. An installed copy
/// hands over to its uninstaller in purge mode; a copy that was never installed gets a detached
/// cleanup script instead. Returns false if nothing could be started.
#[cfg(windows)]
fn start_uninstall(data_dir: &std::path::Path) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let uninstallers = exe
        .parent()
        .map(PathBuf::from)
        .into_iter()
        .chain(shoji_core::env::install_directory())
        .map(|d| d.join("uninstall.exe"));
    for uninstaller in uninstallers {
        if uninstaller.exists() {
            return std::process::Command::new(uninstaller)
                .args(["--uninstall", "--purge"])
                // The uninstaller's own window must not open (and lock) this browser's profile.
                .env_remove("WEBVIEW2_USER_DATA_FOLDER")
                .spawn()
                .is_ok();
        }
    }

    // A copy that was never installed: delete its data, shortcuts and itself once it exits.
    let mut dirs = vec![data_dir.to_path_buf()];
    dirs.extend(shoji_core::env::footprint_dirs());
    let mut files = shoji_core::shell::all_shortcuts();
    for p in shoji_core::env::temp_leftovers() {
        if p.is_dir() {
            dirs.push(p);
        } else {
            files.push(p);
        }
    }
    files.push(exe);
    shoji_core::env::start_purge(&dirs, &files)
}

#[cfg(not(windows))]
fn start_uninstall(_data_dir: &std::path::Path) -> bool {
    false
}

/// Copy a folder tree (symbolic links are skipped, depth is capped).
/// Set how dark mode treats `host` ("follow" the global switch, "always" dark or "never" dark).
/// A host is in at most one of the two lists.
fn set_dark_site_mode(s: &mut Settings, host: &str, mode: &str) {
    let a = &mut s.appearance;
    a.dark_websites_exclude.retain(|e| e != host);
    a.dark_websites_include.retain(|e| e != host);
    match mode {
        "always" => a.dark_websites_include.push(host.to_string()),
        "never" => a.dark_websites_exclude.push(host.to_string()),
        _ => {}
    }
}

/// Draw the tray icon in the chosen crane colour.
fn app_set_tray_icon(tray: &tray::Tray, accent: Accent) {
    let size = tray::Tray::icon_size();
    tray.set_icon(shoji_brand::render_rgba(accent, size), size);
}

/// Search suggestions from an OpenSearch-style endpoint (`["query", ["a", "b", ...]]`); empty
/// when the engine can't be reached.
fn fetch_suggestions(url: &str) -> Vec<String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(3)))
        .build()
        .into();
    let Ok(mut resp) = agent.get(url).call() else {
        return Vec::new();
    };
    let Ok(bytes) = resp.body_mut().with_config().limit(64 * 1024).read_to_vec() else {
        return Vec::new();
    };
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| v.get(1).and_then(|w| w.as_array()).cloned())
        .map(|w| {
            w.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .take(5)
                .collect()
        })
        .unwrap_or_default()
}

/// The new tab page, by either of its addresses.
fn is_new_tab_page(url: &str) -> bool {
    url.starts_with("shoji://newtab") || url.starts_with("http://shoji.newtab")
}

fn copy_dir_all(from: &std::path::Path, to: &std::path::Path, depth: usize) -> std::io::Result<()> {
    if depth > 12 {
        return Ok(());
    }
    if to.exists() && depth == 0 {
        std::fs::remove_dir_all(to)?;
    }
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            copy_dir_all(&entry.path(), &target, depth + 1)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if is_process_elevated() {
        #[cfg(windows)]
        unsafe {
            use windows::core::w;
            use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
            MessageBoxW(
                None,
                w!("Shoji doesn't run as Administrator, to keep web content away from elevated privileges.\n\nPlease start it as a standard user."),
                w!("Shoji"),
                MB_OK | MB_ICONERROR,
            );
        }
        std::process::exit(1);
    }

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));
    let args: Vec<String> = std::env::args().collect();

    if args.iter().any(|a| a == "--uninstall") {
        let uninstaller = exe_dir.join("uninstall.exe");
        if uninstaller.exists() {
            let _ = std::process::Command::new(uninstaller)
                .arg("--uninstall")
                .spawn();
        }
        return Ok(());
    }

    // One running copy per install: a later launch opens a window in it (or quits it, for
    // `--quit`, which the installer uses) and exits.
    let instance = tray::instance_name();
    let quit_requested = args.iter().any(|a| a == "--quit");
    if !tray::claim_instance(&instance) {
        if args.iter().any(|a| a == "--background") {
            return Ok(()); // already running: don't pop a window
        }
        let url = args.iter().skip(1).find(|a| !a.starts_with('-'));
        tray::forward_launch(&instance, quit_requested, url.map(String::as_str));
        return Ok(());
    }
    if quit_requested {
        return Ok(()); // nothing was running
    }

    // The installed copy lists itself in Windows' browser list (this also covers versions installed
    // before Shoji could be a default browser).
    if let Ok(exe) = std::env::current_exe() {
        if shoji_core::shell::is_installed_copy(&exe) {
            let _ = shoji_core::shell::register_as_browser(&exe);
        }
    }

    let data_dir = shoji_core::env::resolve_data_directory(&exe_dir);
    crash::install(&data_dir);
    crash::spawn_watchdog(&data_dir);
    let webview_data = shoji_core::env::resolve_profile_directory(&data_dir);
    // Private mode with nothing on the keep list: drop anything remembered from before it was
    // switched on. Runs before the engine starts, so no browser process holds the files.
    {
        let s = Settings::load_from_path(&data_dir.join("settings.json"));
        if s.privacy.ephemeral_default && s.privacy.persistent_sites.is_empty() {
            let _ = std::fs::remove_dir_all(webview_data.join("Default"));
        }
    }
    let _ = std::fs::create_dir_all(&webview_data);
    // Temporary containers left behind by a crash: their data was never meant to outlive them.
    {
        let dir = webview_data.join("EBWebView");
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            // The engine keeps profile "<name>" in a folder named "WV2Profile_<name>".
            if e.file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .trim_start_matches("wv2profile_")
                .starts_with(&container_profile(shoji_core::settings::TEMP_CONTAINER))
            {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    // SAFETY: set before any other thread exists.
    unsafe { std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &webview_data) };
    let startup_settings = Settings::load_from_path(&data_dir.join("settings.json"));
    EXTENSIONS_ON.store(
        startup_settings.extensions.enabled,
        std::sync::atomic::Ordering::Relaxed,
    );
    if startup_settings.privacy.block_third_party_cookies {
        let mut flags = std::env::var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS").unwrap_or_default();
        if !flags.is_empty() {
            flags.push(' ');
        }
        flags.push_str("--test-third-party-cookie-phaseout");
        // SAFETY: still single-threaded.
        unsafe { std::env::set_var("WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", flags) };
    }

    let Some(engine_version) = detect_webview2_runtime() else {
        #[cfg(windows)]
        unsafe {
            use windows::core::w;
            use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
            MessageBoxW(
                None,
                w!("Shoji needs the Microsoft Edge WebView2 Runtime, which wasn't found.\n\nInstall it from https://developer.microsoft.com/microsoft-edge/webview2/"),
                w!("Shoji"),
                MB_OK | MB_ICONERROR,
            );
        }
        std::process::exit(1);
    };

    let event_loop = EventLoop::<BrowserEvent>::with_user_event().build()?;
    let update_dir = data_dir.clone();
    let mut app = BrowserApp::new(event_loop.create_proxy(), data_dir, engine_version);
    app.tray = tray::Tray::start(event_loop.create_proxy(), &instance);
    // The icon stays in the notification area for as long as Shoji runs.
    if let Some(tray) = &app.tray {
        app_set_tray_icon(tray, app.settings.accent());
        tray.show();
    }
    // An update or reinstall rewrites the shortcuts with the plain white icon; put the chosen
    // colour back (does nothing when they already have it).
    icons::retint_shortcuts(app.data_dir.clone(), app.settings.accent());
    app.update_tx = Some(update::spawn_checker(event_loop.create_proxy(), update_dir));
    update::sweep_old_files();
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[cfg(test)]
mod feature_tests {
    use super::parse_css_color;

    #[test]
    fn only_shojis_own_pages_count_as_internal() {
        use super::is_internal_origin as internal;
        assert!(internal("http://shoji.settings/"));
        assert!(internal("http://shoji.notice-12/?x=1"));
        assert!(!internal("https://shoji.settings/"));
        assert!(!internal("http://shoji.x@evil.example/"));
        assert!(!internal("http://shoji.settings:8080/"));
        assert!(!internal("http://shoji./"));
        assert!(!internal("http://example.com/http://shoji.settings/"));
    }

    #[test]
    fn page_colours_are_reduced_to_hex_or_dropped() {
        assert_eq!(parse_css_color("#fff").as_deref(), Some("#ffffff"));
        assert_eq!(parse_css_color("#1a2B3c").as_deref(), Some("#1a2b3c"));
        assert_eq!(parse_css_color("#11223344").as_deref(), Some("#112233"));
        assert_eq!(
            parse_css_color("rgb(10, 20, 30)").as_deref(),
            Some("#0a141e")
        );
        assert_eq!(
            parse_css_color("rgba(10, 20, 30, 1)").as_deref(),
            Some("#0a141e")
        );
        assert_eq!(parse_css_color("rgba(0, 0, 0, 0)"), None);
        assert_eq!(parse_css_color("rgb(300, 0, 0)"), None);
        assert_eq!(parse_css_color("red"), None);
        assert_eq!(parse_css_color("url(javascript:alert(1))"), None);
        assert_eq!(parse_css_color("#12"), None);
    }
}
