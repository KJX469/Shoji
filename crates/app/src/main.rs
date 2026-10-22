#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Shoji — a paper-thin browser shell around the system WebView2 runtime.

mod app_extensions;
mod app_ipc;
mod app_menus;
mod app_panels;
mod app_session;
mod app_tabs;
mod app_update;
mod app_vault;
mod bookmarks;
mod browser_window;
mod cert;
mod crash;
mod darkfixes;
mod default_browser;
mod dns_win;
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
    /// Breach counts by password hash, and whether every lookup was answered
    VaultBreaches(HashMap<String, u64>, bool),
    /// Store extensions were checked for updates: the ids that were replaced on disk
    ExtensionsUpdated(Result<Vec<String>, String>),
    /// The store catalog was downloaded again
    CatalogUpdated,
    /// A window's extension popup wants this size (logical px)
    ExtensionPopupSize(WindowId, f64, f64),
    /// A window's extension popup lost focus or closed itself
    ExtensionPopupClosed(WindowId),
    /// A window's extension popup lost focus
    ExtensionPopupBlurred(WindowId),
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
    /// The matcher has answers for held requests
    ShieldAnswers,
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
        "extensions" => pages::extensions(s),
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
    /// An extension's popup, open under its toolbar button
    ext_popup: Option<app_extensions::ExtensionPopup>,
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
    /// The toolbar's extension buttons (JSON for the chrome strip)
    extension_buttons: String,
    /// What each extension's button opens, and whether it is a popup (by extension id)
    extension_popups: HashMap<String, (String, bool)>,
    /// Extensions the puzzle menu lists, in order: (id, name)
    extension_menu: Vec<(String, String)>,
    /// The extension popup that last closed by losing focus, and when (a click on its own
    /// button closes it first; that click must not open it again)
    popup_blurred: Option<(String, Instant)>,
    /// When store extensions were last checked for updates
    extensions_checked: Option<Instant>,
    catalog_checked: Option<Instant>,
    extensions_offered: std::collections::HashSet<String>,
    /// The toolbar has asked the engine for the installed extensions
    extensions_asked: std::cell::Cell<bool>,
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
    breaches: app_vault::Breaches,
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
        shield.uncloak_with(shoji_byobu::Uncloaker::spawn(Box::new(dns_win::resolve)));
        shield::init(proxy.clone());
        shield::apply_settings(&settings.shield);
        darkfixes::set_enabled(settings.dark_needed());
        shield::spawn_updater(shield.clone(), data_dir.join("byobu"));
        Self {
            windows: HashMap::new(),
            proxy,
            shield,
            extension_buttons: r#"{"buttons":[],"any":false}"#.into(),
            extension_popups: HashMap::new(),
            extension_menu: Vec::new(),
            popup_blurred: None,
            extensions_asked: std::cell::Cell::new(false),
            extensions_checked: None,
            catalog_checked: None,
            extensions_offered: Default::default(),
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
            breaches: Default::default(),
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
                ext_popup: None,
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

    fn eval_all_internal(&self, js: &str) {
        for win in self.windows.values() {
            win.eval_internal_tabs(js);
        }
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
                let reported = url.clone();
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
                // A tab that opens straight at a store page never "moves" there, so every report
                // of the front tab's address is looked at.
                let front = win.tabs.active_tab_id() == Some(tab_id);
                if let Some(address) = reported.filter(|_| front) {
                    self.offer_store_extension(&address);
                }
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
                // Popups open from the toolbar; options pages open as ordinary tabs.
                let json = self.extensions_listed(&json);
                self.check_extension_updates();
                let js = format!(
                    "window.__extensions && window.__extensions({{active:{on},items:{json}}})"
                );
                let ids: Vec<WindowId> = self.windows.keys().copied().collect();
                for id in ids {
                    self.eval_welcome_and_settings(id, &js);
                }
            }
            BrowserEvent::CatalogUpdated => self.push_extension_catalog(),
            BrowserEvent::ExtensionPopupSize(win_id, w, h) => {
                self.size_extension_popup(win_id, w, h)
            }
            BrowserEvent::ExtensionPopupClosed(win_id) => self.blur_extension_popup(win_id),
            BrowserEvent::ExtensionPopupBlurred(win_id) => self.extension_popup_blurred(win_id),
            BrowserEvent::ExtensionsUpdated(result) => {
                if let Ok(ids) = result {
                    if !ids.is_empty() {
                        self.toast(&serde_json::json!({
                            "title": "Extensions updated.",
                            "text": "New versions are used after you restart Shoji.",
                            "pose": "unfold",
                        }));
                    }
                }
            }
            BrowserEvent::VaultBreaches(found, complete) => self.breaches_checked(found, complete),
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
            BrowserEvent::ShieldAnswers => shield::apply_answers(),
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

/// Delete what the engine remembered for sites (cookies, storage, caches, history, form data)
/// from the main profile and every container, keeping installed extensions and preferences.
fn forget_remembered(engine_dir: &std::path::Path) {
    let keep = |name: &str| name.contains("Extension") || name.ends_with("Preferences");
    for e in std::fs::read_dir(engine_dir)
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.to_ascii_lowercase().starts_with("wv2profile_c-") {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    for e in std::fs::read_dir(engine_dir.join("Default"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = e.file_name().to_string_lossy().into_owned();
        if keep(&name) {
            continue;
        }
        let path = e.path();
        let _ = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Test runners (CI's are elevated) may run a debug build anyway; release builds never do.
    let elevated_ok = cfg!(debug_assertions) && std::env::var_os("SHOJI_ALLOW_ELEVATED").is_some();
    if is_process_elevated() && !elevated_ok {
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
            forget_remembered(&webview_data.join("EBWebView"));
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
