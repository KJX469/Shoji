#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
//! Shoji — a paper-thin browser shell around the system WebView2 runtime.

mod cert;
mod default_browser;
mod icons;
mod import_win;
mod keys;
mod pages;
mod shield;
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

const CHROME_HEIGHT: f64 = 76.0;
const TRAY_HEIGHT: f64 = 38.0;
const SIDEBAR_WIDTH: f64 = 320.0;
/// Draggable divider width/height between the two panes of split view
const SPLIT_GAP: f64 = 6.0;
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
    /// A newer, checksum-verified installer has been downloaded
    UpdateReady {
        version: String,
        installer: PathBuf,
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
    PermissionPrompt {
        tab_id: TabId,
        permission_id: u64,
        origin: String,
        kind: &'static str,
    },
    TabCrashed(TabId, i32),
    CertificateError {
        tab_id: TabId,
        host: String,
        uri: String,
    },
    CertificateReady(String),
    DownloadProgress {
        download_id: u64,
        filename: String,
        received_bytes: i64,
        total_bytes: i64,
        state: &'static str,
    },
    TabSuspended(TabId),
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
}

type SharedPages = Arc<RwLock<PageContext>>;

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
        ),
        "welcome" => pages::welcome(s),
        "passwords" => pages::passwords(s),
        "byobu" => pages::byobu(s),
        "settings" => pages::settings(s, &ctx.engine_version, &ctx.data_dir, ctx.site_choices),
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

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

struct Tab {
    view: WebView,
    /// Cookie partition this view was built with (persistent vs. in-memory)
    persistent: bool,
    hidden_since: Option<Instant>,
    suspended: bool,
    /// Registered Dark Reader document script, if any
    dark_script: webview::ScriptSlot,
    /// The page's own colour (`#rrggbb`), used to tint the toolbar
    tint: Option<String>,
    /// The page holds a video (shows the picture-in-picture button)
    has_video: bool,
}

/// What a text prompt in the overlay is asking for.
#[derive(Clone, Copy)]
enum Prompt {
    NewGroup(TabId),
    RenameGroup(u32),
    NewWorkspace,
    RenameWorkspace(u32),
}

struct BrowserWindow {
    window: Arc<Window>,
    tabs: TabManager,
    views: HashMap<TabId, Tab>,
    chrome: Option<WebView>,
    sidebar: Option<WebView>,
    sidebar_open: bool,
    sidebar_mode: &'static str,
    notes: Option<WebView>,
    notes_open: bool,
    notes_expanded: bool,
    /// The visible part of the notepad, relative to its desk (None while hidden)
    notes_clip: Option<Frame>,
    /// The notepad page has loaded (until then there is nothing to unroll)
    notes_ready: bool,
    notes_anim: Option<NotesAnim>,
    width: f64,
    height: f64,
    find_open: bool,
    permission_open: bool,
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
    /// Script waiting for the overlay page to finish loading
    overlay_pending: Option<String>,
    /// When the overlay was last shown (a blur right after showing is ignored)
    overlay_shown: Option<Instant>,
    overlay_items: Vec<shoji_core::ipc::OverlayItem>,
    prompt: Option<Prompt>,
    /// Tab currently in HTML5 fullscreen (e.g. fullscreen video in a split pane)
    fullscreen_tab: Option<TabId>,
    /// Whether the user is dragging the split divider
    divider_dragging: bool,
    /// Last cursor logical coordinates in this window
    cursor_pos: (f64, f64),
    /// Last click time on the split divider (for double-click reset to 50/50)
    last_divider_click: Option<Instant>,
    /// Tabs currently showing the reader view
    reader: HashSet<TabId>,
    /// Last tint / page-state pushed to the chrome strip
    last_page_sync: String,
    last_fav_sync: String,
}

impl BrowserWindow {
    fn chrome_height(&self) -> f64 {
        let trays = self.find_open as u8 + self.permission_open as u8;
        CHROME_HEIGHT + TRAY_HEIGHT * trays as f64
    }

    fn content_tuple(&self) -> (f64, f64, f64, f64) {
        let top = self.chrome_height();
        let width = if self.sidebar_open {
            (self.width - SIDEBAR_WIDTH).max(1.0)
        } else {
            self.width.max(1.0)
        };
        (0.0, top, width, (self.height - top).max(1.0))
    }

    fn content_bounds(&self) -> Rect {
        let (x, y, w, h) = self.content_tuple();
        bounds(x, y, w, h)
    }

    /// Whether the active tab is one of the two split-view panes.
    fn split_showing(&self) -> Option<[TabId; 2]> {
        self.tabs.split_panes()
    }

    /// Hit-test `(x, y)` against the draggable split divider gap.
    fn divider_hit(&self, x: f64, y: f64) -> Option<shoji_core::tabs::SplitOrientation> {
        if self.fullscreen_tab.is_some() {
            return None;
        }
        let split = self.tabs.visible_split()?;
        let (dx, dy, dw, dh) = split.divider_rect(self.content_tuple(), SPLIT_GAP);
        if x >= dx - 2.0 && x <= dx + dw + 2.0 && y >= dy - 2.0 && y <= dy + dh + 2.0 {
            Some(split.orientation)
        } else {
            None
        }
    }

    /// Where tab `id` sits: full window in fullscreen, its split pane rectangle, or the page area.
    fn rect_for(&self, id: TabId) -> Rect {
        if self.fullscreen_tab == Some(id) {
            return bounds(0.0, 0.0, self.width, self.height);
        }
        let area = self.content_tuple();
        if let Some(split) = self.tabs.visible_split() {
            if id == split.panes[0] || id == split.panes[1] {
                let [r0, r1] = split.layout(area, SPLIT_GAP);
                let r = if id == split.panes[0] { r0 } else { r1 };
                return bounds(r.0, r.1, r.2, r.3);
            }
        }
        bounds(area.0, area.1, area.2, area.3)
    }

    fn active(&self) -> Option<&Tab> {
        self.views.get(&self.tabs.active_tab_id()?)
    }

    /// Only the visible tab(s) are resized; hidden tabs get their bounds when shown.
    fn layout(&mut self) {
        if let Some(fs_id) = self.fullscreen_tab {
            if let Some(chrome) = &self.chrome {
                let _ = chrome.set_visible(false);
            }
            if let Some(sidebar) = &self.sidebar {
                let _ = sidebar.set_visible(false);
            }
            for (&id, tab) in &self.views {
                if id == fs_id {
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
            let _ = chrome.set_bounds(bounds(0.0, 0.0, self.width, top));
        }
        let visible: Vec<TabId> = match self.split_showing() {
            Some(pair) => pair.to_vec(),
            None => self.tabs.active_tab_id().into_iter().collect(),
        };
        for id in visible {
            let rect = self.rect_for(id);
            if let Some(tab) = self.views.get(&id) {
                let _ = tab.view.set_bounds(rect);
                let _ = tab.view.set_visible(true);
            }
        }
        if let (true, Some(sidebar)) = (self.sidebar_open, &self.sidebar) {
            let _ = sidebar.set_visible(true);
            let _ = sidebar.set_bounds(bounds(
                self.width - SIDEBAR_WIDTH,
                top,
                SIDEBAR_WIDTH,
                self.height - top,
            ));
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
        let sidebar = if self.sidebar_open {
            SIDEBAR_WIDTH
        } else {
            0.0
        };
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
        if let (true, Some(notes)) = (self.notes_open, &self.notes) {
            webview::bring_to_top(notes);
        }
        if let (true, Some(overlay)) = (self.overlay_open, &self.overlay) {
            webview::bring_to_top(overlay);
        }
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
        let keep = |t: TabId| t == id || pair.is_some_and(|p| p.contains(&t));
        let mut showing: Vec<TabId> = previous.into_iter().collect();
        showing.extend(prev_split.into_iter().flatten());
        showing.extend(pair.into_iter().flatten());
        for (&tid, tab) in self.views.iter_mut() {
            if !keep(tid) && (showing.contains(&tid) || tab.hidden_since.is_none()) {
                Self::hide(tab);
            }
        }
        let mut panes = vec![id];
        panes.extend(pair.into_iter().flatten().filter(|t| *t != id));
        for t in panes {
            let rect = self.rect_for(t);
            if let Some(tab) = self.views.get_mut(&t) {
                let _ = tab.view.set_bounds(rect);
                let _ = tab.view.set_visible(true);
                webview::wake(&tab.view);
                if t == id {
                    webview::focus(&tab.view);
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
    next_tab_id: u64,
    modifiers: ModifiersState,
    last_engine_restart: Option<Instant>,
    notebook: shoji_core::notes::Notebook,
    shield: shoji_byobu::Shield,
    favourites: shoji_core::userdata::Favourites,
    history: shoji_core::userdata::History,
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
    /// A downloaded update, and whether its installer has been started
    pending_update: Option<(String, PathBuf)>,
    update_started: bool,
    /// Tabs from the previous run, waiting for the user's answer (or restored at startup)
    pending_session: Option<shoji_core::userdata::Session>,
    restore_on_start: bool,
    /// Last session JSON written, and when (writes are throttled)
    session_json: String,
    session_written: Instant,
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
        webview::set_download_prefs(
            settings.downloads.ask_where_to_save,
            settings.downloads.default_folder.clone(),
        );
        let favourites = shoji_core::userdata::Favourites::load(data_dir.join("favourites.json"));
        let site_data = shoji_core::userdata::SiteData::load(data_dir.join("sitedata.json"));
        let history = shoji_core::userdata::History::load(data_dir.join("history.json"));
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
        let offer = !private
            && mode != "never"
            && previous.has_pages()
            && (!previous.clean_exit || mode == "always");
        let restore_on_start = offer && mode == "always";
        let restore_tabs = if offer && !restore_on_start {
            previous.tab_count()
        } else {
            0
        };
        let pending_session = offer.then_some(previous);

        let pages = Arc::new(RwLock::new(PageContext {
            settings: settings.clone(),
            engine_version,
            data_dir: data_dir.clone(),
            notices: HashMap::new(),
            favourites: favourites.items().to_vec(),
            recent: Vec::new(),
            restore_tabs,
            site_choices: site_data.count(),
        }));
        let shield = shoji_byobu::Shield::new();
        shield::init(proxy.clone());
        shield::apply_settings(&settings.shield);
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
            next_tab_id: 1,
            modifiers: ModifiersState::default(),
            last_engine_restart: None,
            notebook: shoji_core::notes::Notebook::load(data_dir.join("notes.json")),
            favourites,
            history,
            vault,
            pending_login: None,
            pending_cookies: Mutex::new(Vec::new()),
            site_data,
            session_path,
            tray: None,
            pending_update: None,
            update_started: false,
            pending_session,
            restore_on_start,
            session_json: String::new(),
            session_written: Instant::now(),
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

    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Option<WindowId> {
        let attrs = window::attributes(self.settings.accent(), &self.settings.appearance.theme);
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

        let frosted = self.settings.appearance.frosted_toolbar;
        window::apply_backdrop(&window, frosted, dark);
        let proxy = self.proxy.clone();
        let chrome = WebViewBuilder::new()
            .with_bounds(bounds(0.0, 0.0, size.width, CHROME_HEIGHT))
            .with_html(pages::chrome(&self.settings))
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
                tabs: TabManager::new(),
                views: HashMap::new(),
                chrome,
                sidebar: None,
                sidebar_open: false,
                sidebar_mode: "menu",
                notes: None,
                notes_open: false,
                notes_expanded: false,
                notes_clip: None,
                notes_ready: false,
                notes_anim: None,
                width: size.width,
                height: size.height,
                find_open: false,
                permission_open: false,
                ui_dirty: true,
                sidebar_dirty: false,
                last_ui_json: String::new(),
                frame_state: None,
                last_title: String::new(),
                overlay: None,
                overlay_open: false,
                overlay_mode: "suggest",
                overlay_ready: false,
                overlay_pending: None,
                overlay_shown: None,
                overlay_items: Vec::new(),
                prompt: None,
                fullscreen_tab: None,
                divider_dragging: false,
                cursor_pos: (0.0, 0.0),
                last_divider_click: None,
                reader: HashSet::new(),
                last_page_sync: String::new(),
                last_fav_sync: String::new(),
            },
        );
        Some(id)
    }

    fn close_window(&mut self, id: WindowId, event_loop: &ActiveEventLoop) {
        // Closing the last window is a normal quit: keep its tabs and mark the exit clean.
        if self.windows.len() == 1 && self.windows.contains_key(&id) {
            self.write_session(true);
        }
        self.windows.remove(&id);
        if self.windows.is_empty() {
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

    /// Run a downloaded installer in update mode. Only when no window is open (the browser is
    /// in the tray, or about to exit): the installer asks this copy to quit, replaces the files
    /// and starts the new version in the tray.
    fn apply_update(&mut self) {
        if self.update_started || !self.windows.is_empty() || !self.settings.performance.auto_update
        {
            return;
        }
        let Some((_, installer)) = &self.pending_update else {
            return;
        };
        if !installer.exists() {
            return;
        }
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
        event_loop.exit();
    }

    /* ----- tabs ----- */

    fn build_view(&self, window: &Window, tab_id: TabId, url: &str, rect: Rect) -> Option<Tab> {
        let internal = is_internal(url);
        let persistent = !internal && self.settings.is_site_persistent(url);
        let dark = self.is_dark(window);

        let pages = self.pages.clone();
        let nav_pages = self.pages.clone();
        let nav_insecure = self.insecure_hosts.clone();
        let proxy_nav = self.proxy.clone();
        let proxy_ipc = self.proxy.clone();
        let mut builder = WebViewBuilder::new()
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
                let wants_persistent = nav.starts_with("http")
                    && !nav.starts_with(INTERNAL_ORIGIN)
                    && nav_pages
                        .read()
                        .map(|c| c.settings.is_site_persistent(&nav))
                        .unwrap_or(false);
                if wants_persistent && !persistent {
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
                let privileged = req.uri().to_string().starts_with(INTERNAL_ORIGIN);
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
            if self.settings.privacy.save_passwords && !self.settings.privacy.ephemeral_default {
                builder = builder.with_initialization_script(pages::LOGIN_SCRIPT);
            }
        }
        // Every tab, not just web ones: an internal tab can navigate to a site in place.
        builder = builder.with_initialization_script(HIDE_SCROLLBAR_SCRIPT);
        builder = builder.with_initialization_script(SPLIT_DROP_SCRIPT);
        // Built blank, so document scripts are registered before the first navigation.
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
        {
            let pages = self.pages.clone();
            let mode_for: shield::ModeFor = Arc::new(move |page: &str| {
                pages
                    .read()
                    .map(|c| shield::parse_mode(c.settings.shield_mode_for(page)))
                    .unwrap_or(shoji_byobu::Mode::Standard)
            });
            shield::attach(&view, self.shield.clone(), mode_for, tab_id);
        }
        let _ = view.load_url(&engine_url(url));
        webview::attach_accelerators(&view, Origin::Tab(tab_id), self.proxy.clone());
        webview::attach_tab_events(
            &view,
            tab_id,
            self.proxy.clone(),
            self.allowed_cert_hosts.clone(),
        );
        webview::attach_focus(&view, tab_id, self.proxy.clone());
        webview::attach_nav_failures(&view, tab_id, self.proxy.clone());
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
            hidden_since: None,
            suspended: false,
            dark_script,
            tint: None,
            has_video: false,
        })
    }

    fn create_tab(&mut self, win_id: WindowId, url: Option<String>) {
        let url = url.unwrap_or_else(|| format!("{INTERNAL_SCHEME}newtab"));
        let tab_id = TabId(self.next_tab_id);
        self.next_tab_id += 1;
        let Some(win) = self.windows.get(&win_id) else {
            return;
        };
        let window = win.window.clone();
        let rect = win.content_bounds();
        let Some(tab) = self.build_view(&window, tab_id, &url, rect) else {
            return;
        };
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
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
                is_muted: false,
                status: TabStatus::Inactive,
                last_active_timestamp_secs: now_secs(),
                group: None,
                workspace,
            },
            None,
        );
        win.views.insert(tab_id, tab);
        win.show_tab(tab_id);
        win.layout();
    }

    /// Reconcile `win.views` with `win.tabs`: drop views for removed tabs, build views for any
    /// newly created tabs, hide & suspend tabs in inactive workspaces, and show the active tab(s).
    fn sync_window_views(&mut self, win_id: WindowId) {
        let (window, rect, to_build, removed, inactive_ids) = {
            let Some(win) = self.windows.get_mut(&win_id) else {
                return;
            };
            let live_ids: HashSet<TabId> = win.tabs.all_tabs().map(|t| t.id).collect();
            let max_id = live_ids.iter().map(|id| id.0).max().unwrap_or(0);
            if max_id >= self.next_tab_id {
                self.next_tab_id = max_id + 1;
            }
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
                win.views.remove(id);
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
                    to_build.push((t.id, url));
                }
            }
            let inactive_ids: HashSet<TabId> =
                win.tabs.inactive_tabs().iter().map(|t| t.id).collect();
            (
                win.window.clone(),
                win.content_bounds(),
                to_build,
                removed,
                inactive_ids,
            )
        };
        if !removed.is_empty() {
            if let Ok(mut ctx) = self.pages.write() {
                for id in &removed {
                    ctx.notices.remove(&id.0);
                }
            }
        }
        for (id, url) in to_build {
            if let Some(tab) = self.build_view(&window, id, &url, rect) {
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
                    if !tab.suspended {
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

    fn close_tab(&mut self, win_id: WindowId, tab_id: TabId, _event_loop: &ActiveEventLoop) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        // Remember it for the new tab page's "recently closed" list.
        if let Some(state) = win.tabs.get_tab_by_id(tab_id).cloned() {
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
    }

    fn open_internal(&mut self, win_id: WindowId, page: &str) {
        let url = format!("{INTERNAL_SCHEME}{page}");
        if let Some(win) = self.windows.get_mut(&win_id) {
            win.sidebar_open = false;
            if let Some(s) = &win.sidebar {
                let _ = s.set_visible(false);
            }
            if let Some(existing) = win.tabs.tabs().iter().find(|t| t.url == url).map(|t| t.id) {
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
        if target == "shoji://settings" {
            return self.open_internal(win_id, "settings");
        }
        if target == "shoji://passwords" {
            return self.open_internal(win_id, "passwords");
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
        let wants_persistent = !is_internal(&target) && self.settings.is_site_persistent(&target);
        win.tabs.update_url(active, target.clone());
        win.tabs.update_favicon(active, None);
        win.reader.remove(&active);
        win.ui_dirty = true;
        win.sidebar_dirty = true;
        let same_partition = win.views.get(&active).map(|t| t.persistent) == Some(wants_persistent);
        if same_partition || is_internal(&target) {
            if let Some(tab) = win.views.get(&active) {
                let _ = tab.view.load_url(&engine_url(&target));
                if !is_internal(&target) {
                    let _ = unsafe { tab.view.controller().SetZoomFactor(zoom) };
                }
            }
            return;
        }
        let window = win.window.clone();
        let rect = win.rect_for(active);
        if let Some(tab) = self.build_view(&window, active, &target, rect) {
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
            == Some(!is_internal(&state.url) && self.settings.is_site_persistent(&state.url));
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

    /// Move a live tab into another window (or a new one) without reloading it.
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
        let Some(win) = self.windows.get_mut(&src) else {
            return;
        };
        let (Some(state), Some(tab)) = (win.tabs.extract_tab(tab_id), win.views.remove(&tab_id))
        else {
            return;
        };
        if win.fullscreen_tab == Some(tab_id) {
            win.fullscreen_tab = None;
        }
        win.reader.remove(&tab_id);
        let src_next = win.tabs.active_tab_id();
        let src_has_other_workspaces = win.tabs.workspaces().len() > 1;
        let dst = match target.or_else(|| self.create_window(event_loop)) {
            Some(id) => id,
            None => return,
        };
        let Some(dst_win) = self.windows.get_mut(&dst) else {
            return;
        };
        if let Ok(handle) =
            winit::raw_window_handle::HasWindowHandle::window_handle(dst_win.window.as_ref())
        {
            if let winit::raw_window_handle::RawWindowHandle::Win32(h) = handle.as_raw() {
                let _ = tab.view.reparent(h.hwnd.get());
            }
        }
        if target.is_none() {
            if let Some((x, y)) = screen {
                dst_win.window.set_outer_position(LogicalPosition::new(
                    (x - 120.0).max(0.0),
                    (y - 16.0).max(0.0),
                ));
            }
        }
        for other in dst_win.views.values_mut() {
            if other.hidden_since.is_none() {
                BrowserWindow::hide(other);
            }
        }
        // Group and workspace ids are per window.
        let mut state = state;
        state.group = None;
        state.workspace = dst_win.tabs.active_workspace_id();
        dst_win.tabs.insert_tab(state, None);
        dst_win.views.insert(tab_id, tab);
        dst_win.show_tab(tab_id);
        dst_win.layout();
        dst_win.window.set_visible(true);
        dst_win.window.focus_window();

        match src_next {
            Some(next) => {
                if let Some(win) = self.windows.get_mut(&src) {
                    win.show_tab(next);
                    win.layout();
                }
            }
            None if src_has_other_workspaces => {
                self.create_tab(src, None);
            }
            None => self.close_window(src, event_loop),
        }
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
            serde_json::json!({ "favs": favs, "recent": recent.iter().take(20).collect::<Vec<_>>() })
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
        }
    }

    fn build_session(&self, clean: bool) -> shoji_core::userdata::Session {
        let mut windows = Vec::new();
        for win in self.windows.values() {
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
        if !self.session_enabled() || self.windows.is_empty() {
            return None;
        }
        let json = serde_json::to_string(&self.build_session(false)).ok()?;
        if json == self.session_json {
            return None;
        }
        let due = self.session_written + Duration::from_secs(1);
        if Instant::now() >= due {
            self.write_session(false);
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
                let id = self.create_window(event_loop);
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
        win.overlay = WebViewBuilder::new()
            .with_bounds(bounds(0.0, 0.0, 10.0, 10.0))
            .with_visible(false)
            .with_html(pages::overlay(&self.settings))
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
        if let Some(o) = &win.overlay {
            webview::attach_accelerators(o, Origin::Window(win_id), self.proxy.clone());
        }
    }

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
        });
        win.overlay_items = items;
        let js = format!("window.__overlayShow && window.__overlayShow({payload})");
        if let Some(o) = &win.overlay {
            let _ = o.set_bounds(frame_rect(frame));
            let _ = o.set_visible(true);
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
            if let Some(tab) = win.active() {
                webview::focus(&tab.view);
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
        win.ui_dirty = true;
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
            (
                "favourite",
                "Add or remove this page from favourites",
                "Ctrl+D",
            ),
            ("reader", "Reader view", "F9"),
            ("pip", "Picture in picture", ""),
            ("find", "Find in page", "Ctrl+F"),
            ("downloads", "Downloads", "Ctrl+J"),
            ("notes", "Quick note", ""),
            ("dark", "Turn dark websites on or off", ""),
            ("settings", "Settings", ""),
            ("passwords", "Passwords", ""),
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
        if win.tabs.is_in_split(tab_id) {
            out.push(item("split-exit".into(), "Exit split view", None));
            out.push(item("split-orient".into(), "Flip split orientation", None));
            out.push(item("split-reset".into(), "Reset split to 50/50", None));
        } else if Some(tab_id) != active && tab.workspace == win.tabs.active_workspace_id() {
            out.push(item(
                format!("split:{}", tab_id.0),
                "Open in split view",
                None,
            ));
        }
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
        for g in win.tabs.groups() {
            if tab.group != Some(g.id) {
                out.push(item(
                    format!("group-set:{}:{}", tab_id.0, g.id),
                    &format!("Move to group: {}", g.name),
                    group_tone(win, g.id),
                ));
            }
        }
        out.push(item(
            format!("group-new:{}", tab_id.0),
            "New group with this tab…",
            None,
        ));
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
                "Remove from favourites"
            } else {
                "Add to favourites"
            };
            out.push(item(format!("fav:{}", tab_id.0), label, None));
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

    fn run_action(&mut self, win_id: WindowId, value: &str, event_loop: &ActiveEventLoop) {
        use UiToHostMessage as M;
        let mut parts = value.splitn(3, ':');
        let name = parts.next().unwrap_or("");
        let a = parts.next().unwrap_or("");
        let b = parts.next().unwrap_or("");
        let tab = a.parse::<u64>().ok().map(TabId);
        match name {
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
            "devtools" => self.handle_ipc(win_id, None, M::OpenDevTools, event_loop),
            "split" => self.split_with(win_id, tab),
            "split-exit" => self.split_with(win_id, None),
            "split-toggle" => self.handle_ipc(win_id, None, M::ToggleSplit, event_loop),
            "split-orient" => self.handle_ipc(win_id, None, M::ToggleSplitOrientation, event_loop),
            "split-focus" => self.handle_ipc(win_id, None, M::SwitchSplitPane, event_loop),
            "split-reset" => self.handle_ipc(win_id, None, M::ResetSplitRatio, event_loop),
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
                    self.favourites.toggle(&t.url, &t.title);
                    self.push_data();
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
            _ => {}
        }
    }

    /// Show `other` beside the active tab, or leave split view when `None`.
    fn split_with(&mut self, win_id: WindowId, other: Option<TabId>) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        match other {
            None => {
                win.tabs.close_split();
            }
            Some(o) => {
                if !win.views.contains_key(&o) || !win.tabs.open_split(o) {
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
            win.notes = WebViewBuilder::new()
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
            let _ = n.set_visible(true);
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
            return self.close_sidebar(win_id);
        }
        if win.sidebar.is_none() {
            let proxy = self.proxy.clone();
            let dark = window::is_dark(&win.window, &self.settings.appearance.theme);
            win.sidebar = WebViewBuilder::new()
                .with_bounds(bounds(0.0, 0.0, SIDEBAR_WIDTH, 1.0))
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
        win.sidebar_open = true;
        win.sidebar_mode = mode;
        win.sidebar_dirty = true;
        if let Some(s) = &win.sidebar {
            let _ = s.set_visible(true);
        }
        win.layout();
        win.sync_zoom();
    }

    fn close_sidebar(&mut self, win_id: WindowId) {
        if let Some(win) = self.windows.get_mut(&win_id) {
            win.sidebar_open = false;
            if let Some(s) = &win.sidebar {
                let _ = s.set_visible(false);
            }
            win.layout();
        }
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
        self.save_settings();
        let s = &self.settings;

        if s.accent() != before.accent()
            || s.appearance.theme != before.appearance.theme
            || s.fonts() != before.fonts()
        {
            let js = pages::apply_theme_js(s);
            for win in self.windows.values() {
                win.eval_chrome(&js);
                win.eval_sidebar(&js);
                win.eval_internal_tabs(&js);
                if let Some(n) = &win.notes {
                    let _ = n.evaluate_script(&js);
                }
            }
        }
        if s.appearance.dark_websites != before.appearance.dark_websites
            || s.appearance.dark_websites_exclude != before.appearance.dark_websites_exclude
        {
            // Swap the per-document script for future pages and restyle open ones in place.
            let script = pages::dark_document_script(s);
            let apply = pages::dark_apply_js(s);
            let skip = format!(
                "window.__setDarkSkip && window.__setDarkSkip({})",
                serde_json::to_string(&s.appearance.dark_websites_exclude).unwrap_or_default()
            );
            for win in self.windows.values() {
                for tab in win.views.values() {
                    webview::set_document_script(&tab.view, &tab.dark_script, script.as_deref());
                    let _ = tab.view.evaluate_script(&apply);
                }
                win.eval_internal_tabs(&skip);
            }
        }
        if s.accent() != before.accent() {
            for win in self.windows.values() {
                window::set_icons(&win.window, s.accent());
            }
            icons::retint_shortcuts(self.data_dir.clone(), s.accent());
        }
        if s.shield != before.shield {
            shield::apply_settings(&s.shield);
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
            }
            // Rebuild each window's active tab in the right storage partition straight away.
            // Background tabs follow when they next navigate or reload.
            let stale: Vec<(WindowId, String)> = self
                .windows
                .iter()
                .filter_map(|(id, win)| {
                    let tab = win.tabs.active_tab()?;
                    let wants =
                        !is_internal(&tab.url) && self.settings.is_site_persistent(&tab.url);
                    let has = win.views.get(&tab.id)?.persistent;
                    (wants != has && !is_internal(&tab.url)).then(|| (*id, tab.url.clone()))
                })
                .collect();
            for (id, url) in stale {
                self.navigate(id, &url);
            }
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
                }
                self.push_data();
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
                self.update_settings(|s| {
                    let list = &mut s.appearance.dark_websites_exclude;
                    match list.iter().position(|h| *h == host) {
                        Some(i) => {
                            list.remove(i);
                            // Including a site again implies wanting dark websites on.
                            s.appearance.dark_websites = Some(true);
                        }
                        None => list.push(host),
                    }
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
                self.close_sidebar(win_id);
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
            M::CloseSidebar => self.close_sidebar(win_id),
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
                self.close_sidebar(win_id);
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
                if let Some(win) = self.windows.get(&win_id) {
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
            M::SaveSettings { patch } => self.update_settings(|s| {
                s.apply_patch(&patch);
            }),
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
                        if let Some(t) = win.views.get(&tab) {
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
            M::SearchHistory { query, seq } => {
                let items: Vec<serde_json::Value> = if self.settings.privacy.ephemeral_default {
                    Vec::new()
                } else {
                    self.history
                        .search(&query, 5)
                        .iter()
                        .map(|h| serde_json::json!({ "url": h.url, "title": h.title }))
                        .collect()
                };
                if let Some(win) = self.windows.get(&win_id) {
                    win.eval_chrome(&format!(
                        "window.__historyResults && window.__historyResults({seq},{})",
                        serde_json::Value::Array(items)
                    ));
                }
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
            M::ToggleFavourite => {
                let page = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.tabs.active_tab().cloned());
                if let Some(t) = page {
                    self.favourites.toggle(&t.url, &t.title);
                    self.push_data();
                }
            }
            M::AddFavourite { url, title } => {
                if self.favourites.add(&url, &title) {
                    self.push_data();
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
                    "Jump to a tab, favourite, setting or action",
                    "",
                );
            }
            M::OverlayReady => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.overlay_ready = true;
                    let mode = win.overlay_mode;
                    if let (Some(js), Some(o)) = (win.overlay_pending.take(), &win.overlay) {
                        let _ = o.evaluate_script(&js);
                        if win.overlay_open && mode != "suggest" {
                            webview::focus(o);
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
                let opens_prompt = value.starts_with("group-new")
                    || value.starts_with("group-rename")
                    || value.starts_with("ws-new")
                    || value.starts_with("ws-rename");
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
            M::ToggleSplit => {
                let toggled = self
                    .windows
                    .get_mut(&win_id)
                    .is_some_and(|win| win.tabs.toggle_split(now_secs()));
                if toggled {
                    self.sync_window_views(win_id);
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
                let Ok(url) = normalize_url(&url, self.settings.search_url_template()) else {
                    return;
                };
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
                    let prev = self
                        .windows
                        .get(&win_id)
                        .and_then(|w| w.tabs.active_tab_id());
                    self.create_tab(win_id, Some(url));
                    if let Some(prev) = prev {
                        self.split_with(win_id, Some(prev));
                    }
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
            Command::CloseTab => {
                if let Some(id) = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.tabs.active_tab_id())
                {
                    self.close_tab(win_id, id, event_loop);
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
            Command::Reload => self.reload(win_id, false),
            Command::HardReload => self.reload(win_id, true),
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
            Command::ToggleSplit => ipc(self, M::ToggleSplit),
            Command::ToggleSplitOrientation => ipc(self, M::ToggleSplitOrientation),
            Command::SwitchSplitPane => ipc(self, M::SwitchSplitPane),
            Command::Workspaces => ipc(self, M::ToggleWorkspacesSidebar),
            Command::NewWorkspace => {
                self.open_prompt(win_id, Prompt::NewWorkspace, "Name this workspace", "");
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
        if let Some(t) = &self.tray {
            t.hide();
        }
        if let Some(id) = self.create_window(event_loop) {
            self.create_tab(id, url);
            if let Some(win) = self.windows.get(&id) {
                win.window.set_visible(true);
            }
        }
    }

    /// Push pending tab-strip and side-panel state, at most once per event-loop turn.
    fn flush(&mut self) {
        let proxy = &self.proxy;
        for win in self.windows.values_mut() {
            if win.ui_dirty {
                win.ui_dirty = false;
                let msg = HostToUiMessage::TabStateSync {
                    tabs: win.tabs.tabs().to_vec(),
                    active_tab_id: win.tabs.active_tab_id(),
                    groups: win.tabs.groups().to_vec(),
                    split: win.tabs.split_panes(),
                    split_state: win.tabs.visible_split().cloned(),
                    workspaces: win.tabs.workspaces().to_vec(),
                    active_workspace_id: win.tabs.active_workspace_id(),
                    inactive_tabs: win.tabs.inactive_tabs().to_vec(),
                };
                let title = match win.tabs.active_tab() {
                    Some(t) if !is_internal(&t.url) && !t.title.is_empty() => {
                        format!("{} — {}", t.title, shoji_core::APP_NAME)
                    }
                    _ => shoji_core::APP_NAME.to_string(),
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
                if !win.last_ui_json.is_empty() {
                    win.eval_sidebar(&format!(
                        "window.__shellUpdate && window.__shellUpdate({})",
                        win.last_ui_json
                    ));
                }
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
            win.overlay = None;
            win.overlay_open = false;
            win.overlay_ready = false;
            let window = win.window.clone();
            let tabs: Vec<(TabId, String)> = win
                .tabs
                .tabs()
                .iter()
                .chain(win.tabs.inactive_tabs().iter())
                .map(|t| (t.id, t.url.clone()))
                .collect();
            let active = win.tabs.active_tab_id();
            win.views.clear();
            win.chrome = None;
            let rect = win.content_bounds();
            for (id, url) in tabs {
                if let Some(tab) = self.build_view(&window, id, &url, rect) {
                    let _ = tab.view.set_visible(false);
                    if let Some(win) = self.windows.get_mut(&win_id) {
                        win.views.insert(id, tab);
                    }
                }
            }
            let dark = self.is_dark(&window);
            let chrome = WebViewBuilder::new()
                .with_html(pages::chrome(&self.settings))
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
            BrowserEvent::UpdateReady { version, installer } => {
                self.pending_update = Some((version, installer));
                self.apply_update();
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
                let mut changed = false;
                let mut moved: Option<(String, String)> = None;
                if let Some(url) = url {
                    // Notice pages keep showing the address that failed.
                    if !url.starts_with("http://shoji.notice-") {
                        let url = display_url(&url);
                        if let Some(cur_url) = win.tabs.get_tab_by_id(tab_id).map(|t| t.url.clone())
                        {
                            if cur_url != url {
                                if is_internal(&url) != is_internal(&cur_url) {
                                    win.tabs.update_favicon(tab_id, None);
                                }
                                moved = Some((cur_url, url.to_string()));
                                if let Some(tab) = win.views.get_mut(&tab_id) {
                                    tab.tint = None;
                                    tab.has_video = false;
                                }
                                win.reader.remove(&tab_id);
                                win.tabs.update_url(tab_id, url.to_string());
                                if !self.settings.privacy.ephemeral_default && !is_internal(&url) {
                                    let cur_title = win
                                        .tabs
                                        .get_tab_by_id(tab_id)
                                        .map(|t| t.title.clone())
                                        .unwrap_or_default();
                                    let now = std::time::SystemTime::now()
                                        .duration_since(std::time::UNIX_EPOCH)
                                        .map_or(0, |d| d.as_millis() as u64);
                                    self.history.record(&url, &cur_title, now);
                                }
                                if let Some(state) =
                                    win.tabs.tabs_mut().iter_mut().find(|t| t.id == tab_id)
                                {
                                    if state.status == TabStatus::Crashed {
                                        state.status = TabStatus::Active;
                                    }
                                }
                                changed = true;
                                win.sidebar_dirty = true;
                            }
                        }
                    }
                }
                if let Some(title) = title {
                    if let Some(cur) = win.tabs.get_tab_by_id(tab_id) {
                        if cur.title != title {
                            let cur_url = cur.url.clone();
                            if !self.settings.privacy.ephemeral_default && !is_internal(&cur_url) {
                                self.history.set_title(&cur_url, &title);
                            }
                            win.tabs.update_title(tab_id, title);
                            changed = true;
                            win.sidebar_dirty = true;
                        }
                    }
                }
                if let Some((back, fwd)) = history {
                    if win
                        .tabs
                        .get_tab_by_id(tab_id)
                        .is_some_and(|t| (t.can_go_back, t.can_go_forward) != (back, fwd))
                    {
                        win.tabs.update_history_state(tab_id, back, fwd);
                        changed = true;
                    }
                }
                if let Some(icon) = favicon {
                    if win
                        .tabs
                        .get_tab_by_id(tab_id)
                        .is_some_and(|t| t.favicon_uri != icon)
                    {
                        win.tabs.update_favicon(tab_id, icon);
                        changed = true;
                        win.sidebar_dirty = true;
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
            BrowserEvent::TabCrashed(tab_id, kind) => {
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
                received_bytes,
                total_bytes,
                state,
            } => {
                let json = serde_json::json!({
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
            BrowserEvent::TabFocused(tab_id) => {
                if let Some(win) = self.owner(tab_id).and_then(|id| self.windows.get_mut(&id)) {
                    if win.tabs.is_in_split(tab_id) && win.tabs.active_tab_id() != Some(tab_id) {
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
                // Counters are picked up by the next flush; tab 0 means the lists changed.
                if tab_id.0 == 0 {
                    for win in self.windows.values() {
                        win.eval_internal_tabs("window.__byobuDirty&&window.__byobuDirty()");
                    }
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
            WindowEvent::Focused(focused) => {
                if let Some(win) = self.windows.get_mut(&window_id) {
                    win.sync_frame();
                    if focused {
                        if let Some([a, b]) = win.tabs.split_panes() {
                            for id in [a, b] {
                                if let Some(tab) = win.views.get(&id) {
                                    webview::wake(&tab.view);
                                }
                            }
                        } else if let Some(tab) = win.active() {
                            webview::wake(&tab.view);
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(win) = self.windows.get_mut(&window_id) {
                    let logical = position.to_logical::<f64>(win.window.scale_factor());
                    win.cursor_pos = (logical.x, logical.y);
                    if win.divider_dragging {
                        let area = win.content_tuple();
                        if let Some(ratio) = win
                            .tabs
                            .visible_split()
                            .map(|s| s.ratio_from_cursor(area, logical.x, logical.y, SPLIT_GAP))
                        {
                            if win.tabs.set_split_ratio(ratio) {
                                win.layout();
                                win.ui_dirty = true;
                            }
                        }
                    } else {
                        let icon = match win.divider_hit(logical.x, logical.y) {
                            Some(SplitOrientation::Vertical) => winit::window::CursorIcon::EwResize,
                            Some(SplitOrientation::Horizontal) => {
                                winit::window::CursorIcon::NsResize
                            }
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
                        if win.divider_hit(x, y).is_some() {
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
                            }
                        }
                    } else {
                        win.divider_dragging = false;
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
        self.flush();
        let mut animating = false;
        for win in self.windows.values_mut() {
            animating |= win.step_notes();
        }
        let session_due = self.maybe_write_session();
        let vault_due = self.vault_tick();
        let next = self
            .rest_background_tabs()
            .into_iter()
            .chain(session_due)
            .chain(vault_due)
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
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let local = shoji_core::env::local_app_directory();
    let installed = std::env::var_os("LOCALAPPDATA")
        .map(|l| PathBuf::from(l).join("Programs").join(shoji_core::APP_NAME));
    let uninstallers = exe
        .parent()
        .map(PathBuf::from)
        .into_iter()
        .chain(installed)
        .map(|d| d.join("uninstall.exe"));
    for uninstaller in uninstallers {
        if uninstaller.exists() {
            return std::process::Command::new(uninstaller)
                .args(["--uninstall", "--purge"])
                .spawn()
                .is_ok();
        }
    }

    let mut dirs = vec![data_dir.to_path_buf()];
    dirs.extend(local);
    let mut files = shoji_core::shell::all_shortcuts();
    files.push(exe);
    match shoji_core::env::delayed_purge_script(&dirs, &files) {
        Some(script) => std::process::Command::new("cmd")
            .args(["/c", &script])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .is_ok(),
        None => false,
    }
}

#[cfg(not(windows))]
fn start_uninstall(_data_dir: &std::path::Path) -> bool {
    false
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
    // SAFETY: set before any other thread exists.
    unsafe { std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &webview_data) };

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
    update::spawn_checker(event_loop.create_proxy(), update_dir);
    event_loop.run_app(&mut app)?;
    Ok(())
}

#[cfg(test)]
mod feature_tests {
    use super::parse_css_color;

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
