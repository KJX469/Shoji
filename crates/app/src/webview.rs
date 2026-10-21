//! WebView2 integration below wry: keyboard accelerators, per-tab navigation events,
//! favicons, permissions, downloads, TLS interstitials, crash detection, and suspension.
//!
//! Tab events carry only the [`TabId`]; the host resolves which window owns the tab when the
//! event arrives. That is what lets a live webview move between windows (tab tear-off)
//! without being rebuilt.

use crate::keys::command_for;
use crate::BrowserEvent;
use shoji_core::tabs::TabId;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use webview2_com::Microsoft::Web::WebView2::Win32::*;
use webview2_com::*;
use windows::core::{Interface, BOOL, PCWSTR, PWSTR};
use winit::event_loop::EventLoopProxy;
use winit::window::WindowId;
use wry::{WebView, WebViewExtWindows};

/// Where a keyboard shortcut came from.
#[derive(Debug, Clone, Copy)]
pub enum Origin {
    Window(WindowId),
    Tab(TabId),
}

/// Take ownership of a COM-allocated string.
fn take(pwstr: PWSTR) -> String {
    if pwstr.is_null() {
        return String::new();
    }
    let s = unsafe { pwstr.to_string() }.unwrap_or_default();
    unsafe { windows::Win32::System::Com::CoTaskMemFree(Some(pwstr.0 as _)) };
    s
}

fn is_reportable(uri: &str) -> bool {
    !uri.is_empty() && !uri.starts_with("data:") && uri != "about:blank"
}

/* ---------- Keyboard ---------- */

pub fn attach_accelerators(webview: &WebView, origin: Origin, proxy: EventLoopProxy<BrowserEvent>) {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_CONTROL, VK_MENU, VK_SHIFT};
    let handler = AcceleratorKeyPressedEventHandler::create(Box::new(move |_, args| {
        let Some(args) = args else { return Ok(()) };
        let mut kind = COREWEBVIEW2_KEY_EVENT_KIND(0);
        unsafe { args.KeyEventKind(&mut kind)? };
        if kind != COREWEBVIEW2_KEY_EVENT_KIND_KEY_DOWN
            && kind != COREWEBVIEW2_KEY_EVENT_KIND_SYSTEM_KEY_DOWN
        {
            return Ok(());
        }
        let mut vkey = 0;
        unsafe { args.VirtualKey(&mut vkey)? };
        let down = |vk: u16| unsafe { GetKeyState(vk as i32) } < 0;
        if let Some(cmd) = command_for(vkey, down(VK_CONTROL.0), down(VK_SHIFT.0), down(VK_MENU.0))
        {
            unsafe { args.SetHandled(true)? };
            let _ = proxy.send_event(BrowserEvent::Command(origin, cmd));
        }
        Ok(())
    }));
    let mut token = Default::default();
    let _ = unsafe {
        webview
            .controller()
            .add_AcceleratorKeyPressed(&handler, &mut token)
    };
}

/* ---------- Popups ---------- */

/// A page asked for a new window (`window.open`, `target="_blank"`). Held until the host has a
/// tab for it, so the page keeps its handle to the new window (opener, name, postMessage).
pub struct PopupRequest {
    pub tab_id: TabId,
    pub uri: String,
    args: ICoreWebView2NewWindowRequestedEventArgs,
    deferral: ICoreWebView2Deferral,
}
// SAFETY: created, handed to the host and completed on the UI thread; the event queue only
// parks it in between.
unsafe impl Send for PopupRequest {}

impl std::fmt::Debug for PopupRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PopupRequest")
            .field("tab_id", &self.tab_id)
            .field("uri", &self.uri)
            .finish()
    }
}

impl PopupRequest {
    /// Give the page `view`, a tab that has not navigated yet, as the window it asked for.
    pub fn attach(self, view: &WebView) {
        unsafe {
            if let Ok(core) = view.controller().CoreWebView2() {
                let _ = self.args.SetNewWindow(&core);
            }
            let _ = self.args.SetHandled(true);
            let _ = self.deferral.Complete();
        }
    }

    /// Do not open anything (the page gets no window).
    pub fn refuse(self) {
        unsafe {
            let _ = self.args.SetHandled(true);
            let _ = self.deferral.Complete();
        }
    }
}

/* ---------- Permissions ---------- */

struct PendingPermission {
    args: ICoreWebView2PermissionRequestedEventArgs,
    deferral: ICoreWebView2Deferral,
}
// SAFETY: created and completed on the UI thread; the map only parks them in between.
unsafe impl Send for PendingPermission {}

static NEXT_PERMISSION_ID: AtomicU64 = AtomicU64::new(1);
static PERMISSIONS: Mutex<Option<HashMap<u64, PendingPermission>>> = Mutex::new(None);

pub fn resolve_permission(permission_id: u64, allow: bool) {
    let pending = PERMISSIONS
        .lock()
        .ok()
        .and_then(|mut m| m.as_mut()?.remove(&permission_id));
    if let Some(p) = pending {
        let state = if allow {
            COREWEBVIEW2_PERMISSION_STATE_ALLOW
        } else {
            COREWEBVIEW2_PERMISSION_STATE_DENY
        };
        unsafe {
            let _ = p.args.SetState(state);
            let _ = p.deferral.Complete();
        }
    }
}

fn permission_name(kind: COREWEBVIEW2_PERMISSION_KIND) -> &'static str {
    match kind {
        COREWEBVIEW2_PERMISSION_KIND_MICROPHONE => "Microphone",
        COREWEBVIEW2_PERMISSION_KIND_CAMERA => "Camera",
        COREWEBVIEW2_PERMISSION_KIND_GEOLOCATION => "Geolocation",
        COREWEBVIEW2_PERMISSION_KIND_NOTIFICATIONS => "Notifications",
        COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ => "Clipboard",
        _ => "Site permission",
    }
}

/* ---------- Downloads ---------- */

struct DownloadOp(ICoreWebView2DownloadOperation);
// SAFETY: only touched from the UI thread (event handlers and host commands).
unsafe impl Send for DownloadOp {}

static NEXT_DOWNLOAD_ID: AtomicU64 = AtomicU64::new(1);
static DOWNLOADS: Mutex<Option<HashMap<u64, DownloadOp>>> = Mutex::new(None);
static ASK_WHERE_TO_SAVE: AtomicBool = AtomicBool::new(false);
static DOWNLOAD_FOLDER: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Mirror the download preferences that COM handlers need without touching app state.
pub fn set_download_prefs(ask: bool, folder: PathBuf) {
    ASK_WHERE_TO_SAVE.store(ask, Ordering::Relaxed);
    // The default is `%USERPROFILE%\Downloads`; Windows may keep that folder elsewhere.
    let default = std::env::var_os("USERPROFILE").map(|u| PathBuf::from(u).join("Downloads"));
    let folder = match downloads_dir() {
        Some(known) if Some(&folder) == default.as_ref() => known,
        _ => folder,
    };
    if let Ok(mut f) = DOWNLOAD_FOLDER.lock() {
        *f = Some(folder);
    }
}

pub fn cancel_download(id: u64) {
    let op = DOWNLOADS
        .lock()
        .ok()
        .and_then(|mut m| m.as_mut()?.remove(&id));
    if let Some(op) = op {
        let _ = unsafe { op.0.Cancel() };
    }
}

fn download_state(op: &ICoreWebView2DownloadOperation) -> (i64, i64, &'static str) {
    let (mut received, mut total) = (0i64, 0i64);
    let mut state = COREWEBVIEW2_DOWNLOAD_STATE(0);
    unsafe {
        let _ = op.BytesReceived(&mut received);
        let _ = op.TotalBytesToReceive(&mut total);
        let _ = op.State(&mut state);
    }
    let label = match state {
        COREWEBVIEW2_DOWNLOAD_STATE_COMPLETED => "Completed",
        COREWEBVIEW2_DOWNLOAD_STATE_INTERRUPTED => "Interrupted",
        _ => "InProgress",
    };
    (received, total, label)
}

/* ---------- Tab events ---------- */

/// Wire every per-tab WebView2 event to the host event loop.
pub fn attach_tab_events(
    webview: &WebView,
    tab_id: TabId,
    proxy: EventLoopProxy<BrowserEvent>,
    allowed_cert_hosts: Arc<Mutex<HashSet<String>>>,
) {
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return;
    };

    // Address + history buttons. SourceChanged covers redirects, pushState, back/forward;
    // HistoryChanged keeps the back/forward buttons exact.
    let send_nav = {
        let core = core.clone();
        let proxy = proxy.clone();
        move || {
            let (mut back, mut fwd) = (BOOL(0), BOOL(0));
            let mut uri = PWSTR::null();
            unsafe {
                let _ = core.CanGoBack(&mut back);
                let _ = core.CanGoForward(&mut fwd);
                let _ = core.Source(&mut uri);
            }
            let uri = take(uri);
            let _ = proxy.send_event(BrowserEvent::TabUpdate {
                tab_id,
                url: is_reportable(&uri).then_some(uri),
                title: None,
                history: Some((back.as_bool(), fwd.as_bool())),
                favicon: None,
            });
        }
    };
    let on_source = send_nav.clone();
    let mut token = Default::default();
    unsafe {
        let _ = core.add_SourceChanged(
            &SourceChangedEventHandler::create(Box::new(move |_, _| {
                on_source();
                Ok(())
            })),
            &mut token,
        );
        let _ = core.add_HistoryChanged(
            &HistoryChangedEventHandler::create(Box::new(move |_, _| {
                send_nav();
                Ok(())
            })),
            &mut token,
        );
    }

    // Loading state, for the toolbar's progress stroke and the tab's flapping crane
    {
        let on_start = proxy.clone();
        let mut token = Default::default();
        let _ = unsafe {
            core.add_NavigationStarting(
                &NavigationStartingEventHandler::create(Box::new(move |_, _| {
                    let _ = on_start.send_event(BrowserEvent::TabLoading {
                        tab_id,
                        loading: true,
                    });
                    Ok(())
                })),
                &mut token,
            )
        };
        let on_done = proxy.clone();
        let _ = unsafe {
            core.add_NavigationCompleted(
                &NavigationCompletedEventHandler::create(Box::new(move |_, _| {
                    let _ = on_done.send_event(BrowserEvent::TabLoading {
                        tab_id,
                        loading: false,
                    });
                    Ok(())
                })),
                &mut token,
            )
        };
    }

    // Title
    {
        let core_t = core.clone();
        let proxy = proxy.clone();
        let handler = DocumentTitleChangedEventHandler::create(Box::new(move |_, _| {
            let mut title = PWSTR::null();
            unsafe { core_t.DocumentTitle(&mut title)? };
            let title = take(title);
            if !title.is_empty() {
                let _ = proxy.send_event(BrowserEvent::TabUpdate {
                    tab_id,
                    url: None,
                    title: Some(title),
                    history: None,
                    favicon: None,
                });
            }
            Ok(())
        }));
        let _ = unsafe { core.add_DocumentTitleChanged(&handler, &mut token) };
    }

    // Favicon straight from the page (no third-party favicon service).
    if let Ok(core15) = core.cast::<ICoreWebView2_15>() {
        let core15_f = core15.clone();
        let proxy = proxy.clone();
        let handler = FaviconChangedEventHandler::create(Box::new(move |_, _| {
            let mut uri = PWSTR::null();
            unsafe { core15_f.FaviconUri(&mut uri)? };
            let uri = take(uri);
            let _ = proxy.send_event(BrowserEvent::TabUpdate {
                tab_id,
                url: None,
                title: None,
                history: None,
                favicon: Some(
                    (uri.starts_with("https://") || uri.starts_with("http://")).then_some(uri),
                ),
            });
            Ok(())
        }));
        let _ = unsafe { core15.add_FaviconChanged(&handler, &mut token) };
    }

    // Audio playback and mute state (`ICoreWebView2_8`).
    if let Ok(core8) = core.cast::<ICoreWebView2_8>() {
        let send_audio = {
            let core8 = core8.clone();
            let proxy = proxy.clone();
            move || {
                let (mut playing, mut muted) = (BOOL(0), BOOL(0));
                unsafe {
                    let _ = core8.IsDocumentPlayingAudio(&mut playing);
                    let _ = core8.IsMuted(&mut muted);
                }
                let _ = proxy.send_event(BrowserEvent::TabAudio {
                    tab_id,
                    playing: playing.as_bool(),
                    muted: muted.as_bool(),
                });
            }
        };
        let on_playing = send_audio.clone();
        unsafe {
            let _ = core8.add_IsDocumentPlayingAudioChanged(
                &IsDocumentPlayingAudioChangedEventHandler::create(Box::new(move |_, _| {
                    on_playing();
                    Ok(())
                })),
                &mut token,
            );
            let _ = core8.add_IsMutedChanged(
                &IsMutedChangedEventHandler::create(Box::new(move |_, _| {
                    send_audio();
                    Ok(())
                })),
                &mut token,
            );
        }
    }

    // Fullscreen element (e.g. HTML5 video in a split pane).
    {
        let core_fs = core.clone();
        let proxy = proxy.clone();
        let handler =
            ContainsFullScreenElementChangedEventHandler::create(Box::new(move |_, _| {
                let mut fs = BOOL(0);
                unsafe { core_fs.ContainsFullScreenElement(&mut fs)? };
                let _ = proxy.send_event(BrowserEvent::TabFullscreen {
                    tab_id,
                    fullscreen: fs.as_bool(),
                });
                Ok(())
            }));
        let _ = unsafe { core.add_ContainsFullScreenElementChanged(&handler, &mut token) };
    }

    // target="_blank" and window.open() become tabs.
    {
        let proxy = proxy.clone();
        let handler = NewWindowRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut uri = PWSTR::null();
            unsafe { args.Uri(&mut uri)? };
            let uri = take(uri);
            // Held open while the host builds a tab, which becomes the page's popup.
            if let Ok(deferral) = unsafe { args.GetDeferral() } {
                let _ = proxy.send_event(BrowserEvent::PopupRequested(PopupRequest {
                    tab_id,
                    uri,
                    args: args.clone(),
                    deferral,
                }));
                return Ok(());
            }
            unsafe { args.SetHandled(true)? };
            if !uri.is_empty() {
                let _ = proxy.send_event(BrowserEvent::NewTabFrom(tab_id, uri));
            }
            Ok(())
        }));
        let _ = unsafe { core.add_NewWindowRequested(&handler, &mut token) };
    }

    // Permissions: deferred until the host applies the user's policy or the user answers.
    {
        let proxy = proxy.clone();
        let handler = PermissionRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let deferral = unsafe { args.GetDeferral()? };
            let mut uri = PWSTR::null();
            let mut kind = COREWEBVIEW2_PERMISSION_KIND(0);
            unsafe {
                let _ = args.Uri(&mut uri);
                let _ = args.PermissionKind(&mut kind);
            }
            let uri = take(uri);
            let id = NEXT_PERMISSION_ID.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut map) = PERMISSIONS.lock() {
                map.get_or_insert_with(HashMap::new).insert(
                    id,
                    PendingPermission {
                        args: args.clone(),
                        deferral,
                    },
                );
            }
            let _ = proxy.send_event(BrowserEvent::PermissionPrompt {
                tab_id,
                permission_id: id,
                origin: crate::extract_host(&uri).unwrap_or(uri),
                kind: permission_name(kind),
            });
            Ok(())
        }));
        let _ = unsafe { core.add_PermissionRequested(&handler, &mut token) };
    }

    // Renderer / browser process failures
    {
        let proxy = proxy.clone();
        let handler = ProcessFailedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND(0);
            unsafe { args.ProcessFailedKind(&mut kind)? };
            let _ = proxy.send_event(BrowserEvent::TabCrashed(tab_id, kind.0));
            Ok(())
        }));
        let _ = unsafe { core.add_ProcessFailed(&handler, &mut token) };
    }

    // Strict TLS: cancel and show an interstitial unless the user bypassed this host.
    if let Ok(core14) = core.cast::<ICoreWebView2_14>() {
        let proxy = proxy.clone();
        let handler =
            ServerCertificateErrorDetectedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut uri = PWSTR::null();
                unsafe { args.RequestUri(&mut uri)? };
                let uri = take(uri);
                let host = crate::extract_host(&uri).unwrap_or_else(|| uri.clone());
                let allowed = allowed_cert_hosts
                    .lock()
                    .map(|set| set.contains(&host))
                    .unwrap_or(false);
                unsafe {
                    args.SetAction(if allowed {
                        COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_ALWAYS_ALLOW
                    } else {
                        COREWEBVIEW2_SERVER_CERTIFICATE_ERROR_ACTION_CANCEL
                    })?
                };
                if !allowed {
                    let _ = proxy.send_event(BrowserEvent::CertificateError { tab_id, host, uri });
                }
                Ok(())
            }));
        let _ = unsafe { core14.add_ServerCertificateErrorDetected(&handler, &mut token) };
    }

    // Downloads
    if let Ok(core4) = core.cast::<ICoreWebView2_4>() {
        let handler = DownloadStartingEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let op = unsafe { args.DownloadOperation()? };
            let mut path = PWSTR::null();
            unsafe { op.ResultFilePath(&mut path)? };
            let suggested = PathBuf::from(take(path));
            let filename = suggested
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "download".into());

            let target = if ASK_WHERE_TO_SAVE.load(Ordering::Relaxed) {
                show_save_dialog(&filename)
            } else {
                DOWNLOAD_FOLDER
                    .lock()
                    .ok()
                    .and_then(|f| f.clone())
                    .map(|dir| unique_path(&dir, &filename))
            };
            let Some(target) = target else {
                unsafe {
                    args.SetCancel(true)?;
                    args.SetHandled(true)?;
                }
                return Ok(());
            };
            let wide: Vec<u16> = target
                .as_os_str()
                .to_string_lossy()
                .encode_utf16()
                .chain(Some(0))
                .collect();
            unsafe {
                args.SetResultFilePath(PCWSTR(wide.as_ptr()))?;
                args.SetHandled(true)?;
            }
            let filename = target
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(filename);

            let id = NEXT_DOWNLOAD_ID.fetch_add(1, Ordering::Relaxed);
            if let Ok(mut map) = DOWNLOADS.lock() {
                map.get_or_insert_with(HashMap::new)
                    .insert(id, DownloadOp(op.clone()));
            }
            let target_path = target.to_string_lossy().into_owned();
            let source = {
                let mut uri = PWSTR::null();
                if unsafe { op.Uri(&mut uri) }.is_ok() {
                    crate::extract_host(&take(uri)).unwrap_or_default()
                } else {
                    String::new()
                }
            };
            let report = {
                let op = op.clone();
                let proxy = proxy.clone();
                let target_path = target_path.clone();
                let source = source.clone();
                move || {
                    let (received_bytes, total_bytes, state) = download_state(&op);
                    if state != "InProgress" {
                        if let Ok(mut map) = DOWNLOADS.lock() {
                            map.as_mut().map(|m| m.remove(&id));
                        }
                    }
                    let _ = proxy.send_event(BrowserEvent::DownloadProgress {
                        download_id: id,
                        filename: filename.clone(),
                        path: target_path.clone(),
                        source: source.clone(),
                        received_bytes,
                        total_bytes,
                        state,
                    });
                }
            };
            let on_state = report.clone();
            let mut token = Default::default();
            unsafe {
                op.add_BytesReceivedChanged(
                    &BytesReceivedChangedEventHandler::create(Box::new(move |_, _| {
                        report();
                        Ok(())
                    })),
                    &mut token,
                )?;
                op.add_StateChanged(
                    &StateChangedEventHandler::create(Box::new(move |_, _| {
                        on_state();
                        Ok(())
                    })),
                    &mut token,
                )?;
            }
            let (_, total_bytes, _) = download_state(&op);
            let _ = proxy.send_event(BrowserEvent::DownloadProgress {
                download_id: id,
                filename: target
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: target_path,
                source,
                received_bytes: 0,
                total_bytes,
                state: "InProgress",
            });
            Ok(())
        }));
        let _ = unsafe { core4.add_DownloadStarting(&handler, &mut token) };
    }
}

/// `dir/name`, or `dir/name (2).ext` etc. if that file already exists.
fn unique_path(dir: &std::path::Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let p = std::path::Path::new(name);
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy())
        .unwrap_or_default();
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|c| !c.exists())
        .unwrap_or(candidate)
}

/* ---------- Visibility, memory and suspension ---------- */

/// Show a tab: normal memory priority, resumed if it was suspended, focused.
pub fn wake(webview: &WebView) {
    let _ = webview.set_memory_usage_level(wry::MemoryUsageLevel::Normal);
    let controller = webview.controller();
    unsafe {
        let _ = controller.NotifyParentWindowPositionChanged();
        if let Ok(core3) = controller
            .CoreWebView2()
            .and_then(|c| c.cast::<ICoreWebView2_3>())
        {
            let mut suspended = BOOL(0);
            if core3.IsSuspended(&mut suspended).is_ok() && suspended.as_bool() {
                let _ = core3.Resume();
            }
        }
    }
}

pub fn focus(webview: &WebView) {
    let _ = unsafe {
        webview
            .controller()
            .MoveFocus(COREWEBVIEW2_MOVE_FOCUS_REASON_PROGRAMMATIC)
    };
}

/// Ask WebView2 to suspend a hidden tab. Pages playing audio or holding locks decline.
pub fn try_suspend(webview: &WebView, tab_id: TabId, proxy: EventLoopProxy<BrowserEvent>) {
    let Ok(core3) =
        (unsafe { webview.controller().CoreWebView2() }).and_then(|c| c.cast::<ICoreWebView2_3>())
    else {
        return;
    };
    let handler = TrySuspendCompletedHandler::create(Box::new(move |result, suspended| {
        if result.is_ok() && suspended {
            let _ = proxy.send_event(BrowserEvent::TabSuspended(tab_id));
        }
        Ok(())
    }));
    let _ = unsafe { core3.TrySuspend(&handler) };
}

/// Mute or unmute everything the page plays.
pub fn set_muted(webview: &WebView, muted: bool) {
    if let Ok(core8) =
        (unsafe { webview.controller().CoreWebView2() }).and_then(|c| c.cast::<ICoreWebView2_8>())
    {
        let _ = unsafe { core8.SetIsMuted(muted) };
    }
}

/* ---------- Extensions ---------- */

fn extension_profile(webview: &WebView) -> Option<ICoreWebView2Profile7> {
    let core = unsafe { webview.controller().CoreWebView2() }.ok()?;
    unsafe { core.cast::<ICoreWebView2_13>().ok()?.Profile().ok()? }
        .cast::<ICoreWebView2Profile7>()
        .ok()
}

/// Install an unpacked extension from `folder` into the profile `webview` uses. Asks the host
/// to re-list when the engine answers.
pub fn add_extension(
    webview: &WebView,
    folder: &std::path::Path,
    proxy: EventLoopProxy<BrowserEvent>,
) {
    use windows::core::HSTRING;
    let Some(profile) = extension_profile(webview) else {
        let _ = proxy.send_event(BrowserEvent::ExtensionsChanged(Some(
            "The engine can't load extensions here".into(),
        )));
        return;
    };
    let handler = ProfileAddBrowserExtensionCompletedHandler::create(Box::new(move |result, _| {
        let _ = proxy.send_event(BrowserEvent::ExtensionsChanged(
            result.err().map(|e| e.message().to_string()),
        ));
        Ok(())
    }));
    let _ = unsafe { profile.AddBrowserExtension(&HSTRING::from(folder.as_os_str()), &handler) };
}

/// Extensions the engine ships with; they aren't the user's to list or remove.
const BUILT_IN_EXTENSIONS: [&str; 2] =
    ["Microsoft Clipboard Extension", "Microsoft Edge PDF Viewer"];

fn each_extension(
    list: &ICoreWebView2BrowserExtensionList,
    mut f: impl FnMut(&ICoreWebView2BrowserExtension, String, String, bool),
) {
    let mut count = 0u32;
    if unsafe { list.Count(&mut count) }.is_err() {
        return;
    }
    for i in 0..count {
        let Ok(ext) = (unsafe { list.GetValueAtIndex(i) }) else {
            continue;
        };
        let (mut id, mut name, mut enabled) = (PWSTR::null(), PWSTR::null(), BOOL(0));
        unsafe {
            let _ = ext.Id(&mut id);
            let _ = ext.Name(&mut name);
            let _ = ext.IsEnabled(&mut enabled);
        }
        let (id, name) = (take(id), take(name));
        if !BUILT_IN_EXTENSIONS.contains(&name.as_str()) {
            f(&ext, id, name, enabled.as_bool());
        }
    }
}

/// Ask the engine which extensions are installed; the host gets them as JSON.
pub fn list_extensions(webview: &WebView, proxy: EventLoopProxy<BrowserEvent>) {
    let Some(profile) = extension_profile(webview) else {
        let _ = proxy.send_event(BrowserEvent::ExtensionsListed("[]".into()));
        return;
    };
    let handler =
        ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(move |result, list| {
            let mut out = Vec::new();
            if result.is_ok() {
                if let Some(list) = list {
                    each_extension(&list, |_, id, name, enabled| {
                        out.push(serde_json::json!({ "id": id, "name": name, "enabled": enabled }));
                    });
                }
            }
            let _ = proxy.send_event(BrowserEvent::ExtensionsListed(
                serde_json::Value::Array(out).to_string(),
            ));
            Ok(())
        }));
    let _ = unsafe { profile.GetBrowserExtensions(&handler) };
}

/// Remove the extension with `id`.
pub fn remove_extension(webview: &WebView, id: String, proxy: EventLoopProxy<BrowserEvent>) {
    let Some(profile) = extension_profile(webview) else {
        return;
    };
    let handler =
        ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(move |result, list| {
            let mut found = None;
            if result.is_ok() {
                if let Some(list) = list {
                    each_extension(&list, |ext, ext_id, _, _| {
                        if ext_id == id {
                            found = Some(ext.clone());
                        }
                    });
                }
            }
            match found {
                Some(ext) => {
                    let proxy = proxy.clone();
                    let done =
                        BrowserExtensionRemoveCompletedHandler::create(Box::new(move |result| {
                            let _ = proxy.send_event(BrowserEvent::ExtensionsChanged(
                                result.err().map(|e| e.message().to_string()),
                            ));
                            Ok(())
                        }));
                    let _ = unsafe { ext.Remove(&done) };
                }
                None => {
                    let _ = proxy.send_event(BrowserEvent::ExtensionsChanged(None));
                }
            }
            Ok(())
        }));
    let _ = unsafe { profile.GetBrowserExtensions(&handler) };
}

/// Turn the extension with `id` on or off.
pub fn set_extension_enabled(
    webview: &WebView,
    id: String,
    enabled: bool,
    proxy: EventLoopProxy<BrowserEvent>,
) {
    let Some(profile) = extension_profile(webview) else {
        return;
    };
    let handler =
        ProfileGetBrowserExtensionsCompletedHandler::create(Box::new(move |result, list| {
            let mut found = None;
            if result.is_ok() {
                if let Some(list) = list {
                    each_extension(&list, |ext, ext_id, _, _| {
                        if ext_id == id {
                            found = Some(ext.clone());
                        }
                    });
                }
            }
            match found {
                Some(ext) => {
                    let proxy = proxy.clone();
                    let done =
                        BrowserExtensionEnableCompletedHandler::create(Box::new(move |result| {
                            let _ = proxy.send_event(BrowserEvent::ExtensionsChanged(
                                result.err().map(|e| e.message().to_string()),
                            ));
                            Ok(())
                        }));
                    let _ = unsafe { ext.Enable(enabled, &done) };
                }
                None => {
                    let _ = proxy.send_event(BrowserEvent::ExtensionsChanged(None));
                }
            }
            Ok(())
        }));
    let _ = unsafe { profile.GetBrowserExtensions(&handler) };
}

/* ---------- Native dialogs ---------- */

fn show_save_dialog(default_name: &str) -> Option<PathBuf> {
    use windows::core::HSTRING;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    use windows::Win32::UI::Shell::{FileSaveDialog, IFileSaveDialog, SIGDN_FILESYSPATH};
    unsafe {
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_ALL).ok()?;
        let _ = dialog.SetFileName(&HSTRING::from(default_name));
        if let Some(dir) = DOWNLOAD_FOLDER.lock().ok().and_then(|f| f.clone()) {
            if let Ok(item) = windows::Win32::UI::Shell::SHCreateItemFromParsingName::<
                _,
                _,
                windows::Win32::UI::Shell::IShellItem,
            >(&HSTRING::from(dir.as_os_str()), None)
            {
                let _ = dialog.SetFolder(&item);
            }
        }
        dialog.Show(None).ok()?;
        let path = dialog
            .GetResult()
            .ok()?
            .GetDisplayName(SIGDN_FILESYSPATH)
            .ok()?;
        Some(PathBuf::from(take(path)))
    }
}

/// The user's Documents folder (wherever Windows has it, e.g. redirected to OneDrive).
pub fn documents_dir() -> Option<PathBuf> {
    use windows::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    let path = unsafe { SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None) }.ok()?;
    Some(PathBuf::from(take(path)))
}

/// The user's Downloads folder (wherever Windows has it).
pub fn downloads_dir() -> Option<PathBuf> {
    use windows::Win32::UI::Shell::{FOLDERID_Downloads, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    let path = unsafe { SHGetKnownFolderPath(&FOLDERID_Downloads, KF_FLAG_DEFAULT, None) }.ok()?;
    Some(PathBuf::from(take(path)))
}

/// Folder picker for the default download location.
pub fn pick_folder(start: &std::path::Path) -> Option<PathBuf> {
    use windows::core::HSTRING;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    use windows::Win32::UI::Shell::{
        FileOpenDialog, IFileOpenDialog, IShellItem, SHCreateItemFromParsingName, FOS_PICKFOLDERS,
        SIGDN_FILESYSPATH,
    };
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL).ok()?;
        let _ = dialog.SetOptions(dialog.GetOptions().ok()? | FOS_PICKFOLDERS);
        if let Ok(item) =
            SHCreateItemFromParsingName::<_, _, IShellItem>(&HSTRING::from(start.as_os_str()), None)
        {
            let _ = dialog.SetFolder(&item);
        }
        dialog.Show(None).ok()?;
        let path = dialog
            .GetResult()
            .ok()?
            .GetDisplayName(SIGDN_FILESYSPATH)
            .ok()?;
        Some(PathBuf::from(take(path)))
    }
}

/// Open-file dialog filtered to `spec` (e.g. `"*.csv"`), titled `title`.
pub fn pick_file(title: &str, filter_name: &str, spec: &str) -> Option<PathBuf> {
    use windows::core::HSTRING;
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{FileOpenDialog, IFileOpenDialog, SIGDN_FILESYSPATH};
    unsafe {
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_ALL).ok()?;
        let name = HSTRING::from(filter_name);
        let spec = HSTRING::from(spec);
        let all = HSTRING::from("All files");
        let star = HSTRING::from("*.*");
        let filters = [
            COMDLG_FILTERSPEC {
                pszName: windows::core::PCWSTR(name.as_ptr()),
                pszSpec: windows::core::PCWSTR(spec.as_ptr()),
            },
            COMDLG_FILTERSPEC {
                pszName: windows::core::PCWSTR(all.as_ptr()),
                pszSpec: windows::core::PCWSTR(star.as_ptr()),
            },
        ];
        let _ = dialog.SetFileTypes(&filters);
        let _ = dialog.SetTitle(&HSTRING::from(title));
        dialog.Show(None).ok()?;
        let path = dialog
            .GetResult()
            .ok()?
            .GetDisplayName(SIGDN_FILESYSPATH)
            .ok()?;
        Some(PathBuf::from(take(path)))
    }
}

/// Bookmarks export (`.html`).
pub fn pick_html_file() -> Option<PathBuf> {
    pick_file(
        "Choose a bookmarks file",
        "Bookmarks (*.html;*.htm)",
        "*.html;*.htm",
    )
}

/// Save-as dialog for a CSV file named `default_name`.
pub fn pick_save_csv(default_name: &str) -> Option<PathBuf> {
    use windows::core::{w, HSTRING};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{FileSaveDialog, IFileSaveDialog, SIGDN_FILESYSPATH};
    unsafe {
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_ALL).ok()?;
        let filters = [COMDLG_FILTERSPEC {
            pszName: w!("CSV file (*.csv)"),
            pszSpec: w!("*.csv"),
        }];
        let _ = dialog.SetFileTypes(&filters);
        let _ = dialog.SetDefaultExtension(w!("csv"));
        let _ = dialog.SetFileName(&HSTRING::from(default_name));
        let _ = dialog.SetTitle(&HSTRING::from("Save passwords as CSV"));
        dialog.Show(None).ok()?;
        let path = dialog
            .GetResult()
            .ok()?
            .GetDisplayName(SIGDN_FILESYSPATH)
            .ok()?;
        Some(PathBuf::from(take(path)))
    }
}

/// Save-as dialog for a bookmarks file named `default_name`.
pub fn pick_save_html(default_name: &str) -> Option<PathBuf> {
    use windows::core::{w, HSTRING};
    use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL};
    use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
    use windows::Win32::UI::Shell::{FileSaveDialog, IFileSaveDialog, SIGDN_FILESYSPATH};
    unsafe {
        let dialog: IFileSaveDialog = CoCreateInstance(&FileSaveDialog, None, CLSCTX_ALL).ok()?;
        let filters = [COMDLG_FILTERSPEC {
            pszName: w!("Bookmarks (*.html)"),
            pszSpec: w!("*.html"),
        }];
        let _ = dialog.SetFileTypes(&filters);
        let _ = dialog.SetDefaultExtension(w!("html"));
        let _ = dialog.SetFileName(&HSTRING::from(default_name));
        let _ = dialog.SetTitle(&HSTRING::from("Export bookmarks"));
        dialog.Show(None).ok()?;
        let path = dialog
            .GetResult()
            .ok()?
            .GetDisplayName(SIGDN_FILESYSPATH)
            .ok()?;
        Some(PathBuf::from(take(path)))
    }
}

/// Count the cookies the page at `url` is sent and, with `clear`, delete them. `done` gets the
/// count once the engine has answered.
pub fn site_cookies(webview: &WebView, url: &str, clear: bool, done: impl FnOnce(usize) + 'static) {
    use windows::core::HSTRING;
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return done(0);
    };
    let Ok(core2) = core.cast::<ICoreWebView2_2>() else {
        return done(0);
    };
    let Ok(manager) = (unsafe { core2.CookieManager() }) else {
        return done(0);
    };
    let deleter = manager.clone();
    let handler = GetCookiesCompletedHandler::create(Box::new(move |result, list| {
        let mut n = 0usize;
        if result.is_ok() {
            if let Some(list) = list {
                let mut count = 0u32;
                unsafe {
                    let _ = list.Count(&mut count);
                }
                for i in 0..count {
                    if let Ok(cookie) = unsafe { list.GetValueAtIndex(i) } {
                        n += 1;
                        if clear {
                            let _ = unsafe { deleter.DeleteCookie(&cookie) };
                        }
                    }
                }
            }
        }
        done(n);
        Ok(())
    }));
    let _ = unsafe { manager.GetCookies(&HSTRING::from(url), &handler) };
}

/// Put cookies read from another browser into the engine's profile (shared by every persistent
/// tab). Returns how many were accepted.
pub fn add_cookies(webview: &WebView, cookies: &[shoji_core::import::Cookie]) -> usize {
    use windows::core::HSTRING;
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return 0;
    };
    let Ok(core2) = core.cast::<ICoreWebView2_2>() else {
        return 0;
    };
    let Ok(manager) = (unsafe { core2.CookieManager() }) else {
        return 0;
    };
    let mut added = 0;
    for c in cookies {
        let Ok(cookie) = (unsafe {
            manager.CreateCookie(
                &HSTRING::from(c.name.as_str()),
                &HSTRING::from(c.value.as_str()),
                &HSTRING::from(c.host.as_str()),
                &HSTRING::from(c.path.as_str()),
            )
        }) else {
            continue;
        };
        unsafe {
            if let Some(exp) = c.expires {
                let _ = cookie.SetExpires(exp as f64);
            }
            let _ = cookie.SetIsHttpOnly(c.http_only);
            let _ = cookie.SetIsSecure(c.secure);
            let _ = cookie.SetSameSite(match c.same_site {
                "strict" => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_STRICT,
                "none" => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_NONE,
                _ => COREWEBVIEW2_COOKIE_SAME_SITE_KIND_LAX,
            });
            if manager.AddOrUpdateCookie(&cookie).is_ok() {
                added += 1;
            }
        }
    }
    added
}

/* ---------- Engine settings, focus, failed navigations, DevTools protocol ---------- */

/// General autofill (addresses and the like; off in private mode) and the tracking-prevention
/// level. The engine's own password saving stays off: Shoji's password manager does that, so
/// there is one list the user can see, search, export and protect.
pub fn configure_engine(webview: &WebView, autofill: bool, tracking: Tracking) {
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return;
    };
    if let Ok(settings) = unsafe { core.Settings() } {
        if let Ok(s4) = settings.cast::<ICoreWebView2Settings4>() {
            unsafe {
                let _ = s4.SetIsPasswordAutosaveEnabled(false);
                let _ = s4.SetIsGeneralAutofillEnabled(autofill);
            }
        }
    }
    set_tracking_prevention(webview, tracking);
}

/// The engine's built-in tracking prevention: switched off with the shield, otherwise strict
/// (when the setting asks for it) or the engine's own default.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tracking {
    Off,
    Default,
    Strict,
}

pub fn set_tracking_prevention(webview: &WebView, tracking: Tracking) {
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return;
    };
    let level = match tracking {
        Tracking::Off => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_NONE,
        Tracking::Default => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_BALANCED,
        Tracking::Strict => COREWEBVIEW2_TRACKING_PREVENTION_LEVEL_STRICT,
    };
    if let Ok(core13) = core.cast::<ICoreWebView2_13>() {
        if let Ok(profile) = unsafe { core13.Profile() } {
            if let Ok(p3) = profile.cast::<ICoreWebView2Profile3>() {
                let _ = unsafe { p3.SetPreferredTrackingPreventionLevel(level) };
            }
        }
    }
}

/// Delete the view's engine profile with all its data (a temporary container's). The engine
/// closes the profile's views as it does.
pub fn delete_profile(webview: &WebView) -> bool {
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return false;
    };
    let Ok(profile) = core
        .cast::<ICoreWebView2_13>()
        .and_then(|c| unsafe { c.Profile() })
        .and_then(|p| p.cast::<ICoreWebView2Profile8>())
    else {
        return false;
    };
    unsafe { profile.Delete() }.is_ok()
}

/// Delete every saved login and autofill entry from the profile. The engine offers no way to
/// list them, so this is the management Shoji can honestly provide.
pub fn clear_saved_logins(webview: &WebView) -> bool {
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return false;
    };
    let Ok(core13) = core.cast::<ICoreWebView2_13>() else {
        return false;
    };
    let Ok(profile) = (unsafe { core13.Profile() }) else {
        return false;
    };
    let Ok(p2) = profile.cast::<ICoreWebView2Profile2>() else {
        return false;
    };
    let handler = ClearBrowsingDataCompletedHandler::create(Box::new(|_| Ok(())));
    let kinds = COREWEBVIEW2_BROWSING_DATA_KINDS(
        COREWEBVIEW2_BROWSING_DATA_KINDS_PASSWORD_AUTOSAVE.0
            | COREWEBVIEW2_BROWSING_DATA_KINDS_GENERAL_AUTOFILL.0,
    );
    unsafe { p2.ClearBrowsingData(kinds, &handler) }.is_ok()
}

/// Clear browsing data held by the engine: cookies and site storage, and/or the cache. With
/// `since_secs` (Unix seconds) only data from that time on goes; `None` clears everything.
pub fn clear_browsing_data(
    webview: &WebView,
    cookies: bool,
    cache: bool,
    since_secs: Option<f64>,
) -> bool {
    let mut bits = 0;
    if cookies {
        bits |= COREWEBVIEW2_BROWSING_DATA_KINDS_ALL_SITE.0;
    }
    if cache {
        bits |= COREWEBVIEW2_BROWSING_DATA_KINDS_DISK_CACHE.0
            | COREWEBVIEW2_BROWSING_DATA_KINDS_CACHE_STORAGE.0;
    }
    if bits == 0 {
        return true;
    }
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return false;
    };
    let Ok(core13) = core.cast::<ICoreWebView2_13>() else {
        return false;
    };
    let Ok(profile) = (unsafe { core13.Profile() }) else {
        return false;
    };
    let Ok(p2) = profile.cast::<ICoreWebView2Profile2>() else {
        return false;
    };
    let kinds = COREWEBVIEW2_BROWSING_DATA_KINDS(bits);
    let handler = ClearBrowsingDataCompletedHandler::create(Box::new(|_| Ok(())));
    match since_secs {
        Some(start) => {
            // The end of the range is "now and beyond".
            unsafe { p2.ClearBrowsingDataInTimeRange(kinds, start, 4_102_444_800.0, &handler) }
                .is_ok()
        }
        None => unsafe { p2.ClearBrowsingData(kinds, &handler) }.is_ok(),
    }
}

/// Report when the user clicks into a tab (split view: the pane that was clicked becomes active).
pub fn attach_focus(webview: &WebView, tab_id: TabId, proxy: EventLoopProxy<BrowserEvent>) {
    let handler = FocusChangedEventHandler::create(Box::new(move |_, _| {
        let _ = proxy.send_event(BrowserEvent::TabFocused(tab_id));
        Ok(())
    }));
    let mut token = Default::default();
    let _ = unsafe { webview.controller().add_GotFocus(&handler, &mut token) };
}

/// An extension popup closes when the user clicks away from it or its page calls
/// `window.close()`, as in Chrome.
pub fn attach_popup_events(webview: &WebView, close: impl Fn() + Clone + 'static) {
    let on_blur = close.clone();
    let blur = FocusChangedEventHandler::create(Box::new(move |_, _| {
        on_blur();
        Ok(())
    }));
    let mut token = Default::default();
    let _ = unsafe { webview.controller().add_LostFocus(&blur, &mut token) };
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return;
    };
    let closed = WindowCloseRequestedEventHandler::create(Box::new(move |_, _| {
        close();
        Ok(())
    }));
    let _ = unsafe { core.add_WindowCloseRequested(&closed, &mut token) };
}

/// Report an https:// navigation that failed because the server couldn't be reached securely
/// (certificate problems are handled separately by the TLS interstitial).
pub fn attach_nav_failures(webview: &WebView, tab_id: TabId, proxy: EventLoopProxy<BrowserEvent>) {
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return;
    };
    let handler = NavigationCompletedEventHandler::create(Box::new(move |sender, args| {
        let (Some(sender), Some(args)) = (sender, args) else {
            return Ok(());
        };
        let mut ok = BOOL(0);
        unsafe { args.IsSuccess(&mut ok)? };
        if ok.as_bool() {
            return Ok(());
        }
        let mut status = COREWEBVIEW2_WEB_ERROR_STATUS(0);
        unsafe { args.WebErrorStatus(&mut status)? };
        // Everything except cancellations, name-resolution failures (HTTP won't help) and
        // certificate / credential problems (handled by their own pages) counts.
        let connect_failure = !matches!(
            status,
            COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED
                | COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED
                | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_COMMON_NAME_IS_INCORRECT
                | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_EXPIRED
                | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_IS_INVALID
                | COREWEBVIEW2_WEB_ERROR_STATUS_CERTIFICATE_REVOKED
                | COREWEBVIEW2_WEB_ERROR_STATUS_CLIENT_CERTIFICATE_CONTAINS_ERRORS
                | COREWEBVIEW2_WEB_ERROR_STATUS_VALID_AUTHENTICATION_CREDENTIALS_REQUIRED
                | COREWEBVIEW2_WEB_ERROR_STATUS_VALID_PROXY_AUTHENTICATION_REQUIRED
        );
        // A page that could not be found or reached at all gets Shoji's own explanation
        // instead of the engine's plain error page.
        let unreachable = match status {
            COREWEBVIEW2_WEB_ERROR_STATUS_HOST_NAME_NOT_RESOLVED => Some(false),
            COREWEBVIEW2_WEB_ERROR_STATUS_DISCONNECTED
            | COREWEBVIEW2_WEB_ERROR_STATUS_SERVER_UNREACHABLE => Some(true),
            _ => None,
        };
        if let Some(offline) = unreachable {
            let mut uri = PWSTR::null();
            unsafe { sender.Source(&mut uri)? };
            let uri = take(uri);
            if uri.starts_with("http://") || uri.starts_with("https://") {
                let _ = proxy.send_event(BrowserEvent::Unreachable {
                    tab_id,
                    uri,
                    offline,
                });
            }
        }
        if !connect_failure {
            return Ok(());
        }
        let mut uri = PWSTR::null();
        unsafe { sender.Source(&mut uri)? };
        let uri = take(uri);
        if uri.starts_with("https://") {
            let _ = proxy.send_event(BrowserEvent::NavFailed { tab_id, uri });
        }
        Ok(())
    }));
    let mut token = Default::default();
    let _ = unsafe { core.add_NavigationCompleted(&handler, &mut token) };
}

/// Call a DevTools-protocol method on a tab (fire and forget).
pub fn cdp(webview: &WebView, method: &str, params: &str) {
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return;
    };
    let m: Vec<u16> = method.encode_utf16().chain(Some(0)).collect();
    let p: Vec<u16> = params.encode_utf16().chain(Some(0)).collect();
    let _ =
        unsafe { core.CallDevToolsProtocolMethod(PCWSTR(m.as_ptr()), PCWSTR(p.as_ptr()), None) };
}

/* ---------- Document scripts, drag regions, z-order ---------- */

/// The script registered with `AddScriptToExecuteOnDocumentCreated`. Registration is
/// asynchronous, so the id arrives later; `wanted` counts replacements, and a registration
/// that completes after a newer one was asked for is removed again instead of kept.
#[derive(Default)]
pub struct ScriptState {
    id: Option<String>,
    wanted: u64,
}

pub type ScriptSlot = Arc<Mutex<ScriptState>>;

fn remove_document_script(core: &ICoreWebView2, id: &str) {
    let id: Vec<u16> = id.encode_utf16().chain(Some(0)).collect();
    let _ = unsafe { core.RemoveScriptToExecuteOnDocumentCreated(PCWSTR(id.as_ptr())) };
}

/// Replace the script that runs at the start of every document in this webview.
/// `None` removes it. Pages already open are not affected; callers evaluate separately.
pub fn set_document_script(webview: &WebView, slot: &ScriptSlot, script: Option<&str>) {
    set_document_script_core(&webview.webview(), slot, script);
}

/// Same as [`set_document_script`], for code that holds the core webview (event handlers).
pub fn set_document_script_core(core: &ICoreWebView2, slot: &ScriptSlot, script: Option<&str>) {
    let Ok(mut state) = slot.lock() else { return };
    state.wanted += 1;
    let ticket = state.wanted;
    if let Some(old) = state.id.take() {
        remove_document_script(core, &old);
    }
    drop(state);
    let Some(script) = script else { return };
    let slot = slot.clone();
    let core_done = core.clone();
    let handler =
        AddScriptToExecuteOnDocumentCreatedCompletedHandler::create(Box::new(move |result, id| {
            if result.is_ok() {
                match slot.lock() {
                    Ok(mut s) if s.wanted == ticket => s.id = Some(id),
                    // Replaced (or removed) while this one was being registered.
                    _ => remove_document_script(&core_done, &id),
                }
            }
            Ok(())
        }));
    let wide: Vec<u16> = script.encode_utf16().chain(Some(0)).collect();
    let _ = unsafe { core.AddScriptToExecuteOnDocumentCreated(PCWSTR(wide.as_ptr()), &handler) };
}

/// Let `app-region: drag` areas of this webview move the top-level window natively
/// (and double-click to maximize, right-click for the system menu).
pub fn enable_drag_regions(webview: &WebView) {
    if let Ok(settings) = unsafe { webview.webview().Settings() } {
        if let Ok(s9) = settings.cast::<ICoreWebView2Settings9>() {
            let _ = unsafe { s9.SetIsNonClientRegionSupportEnabled(true) };
        }
    }
}

/// Clip a child webview's drawing and hit-testing to a rectangle, in physical pixels relative to
/// its top-left corner. Cheap: unlike a resize, nothing has to be repainted.
pub fn set_region(webview: &WebView, left: i32, top: i32, right: i32, bottom: i32) {
    use windows::Win32::Graphics::Gdi::{CreateRectRgn, SetWindowRgn};
    unsafe {
        let region = CreateRectRgn(left, top, right, bottom);
        // On success the window owns the region; it frees the previous one itself.
        SetWindowRgn(webview.hwnd(), Some(region), true);
    }
}

/// Raise a webview above its sibling child windows (used for floating panels).
pub fn bring_to_top(webview: &WebView) {
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, HWND_TOP, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };
    let _ = unsafe {
        SetWindowPos(
            webview.hwnd(),
            Some(HWND_TOP),
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
        )
    };
}

/// Clip a child webview to a rounded rectangle of `width` x `height` physical pixels, so a
/// floating dropdown has soft corners.
pub fn set_round_region(webview: &WebView, width: i32, height: i32, radius: i32) {
    use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};
    unsafe {
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, radius * 2, radius * 2);
        SetWindowRgn(webview.hwnd(), Some(region), true);
    }
}

/// Clip a child webview to the union of rounded rectangles, each `(left, top, right, bottom,
/// radius)` in physical pixels. Everything outside them neither draws nor takes clicks.
pub fn set_union_region(webview: &WebView, rects: &[(i32, i32, i32, i32, i32)]) {
    use windows::Win32::Graphics::Gdi::{
        CombineRgn, CreateRoundRectRgn, DeleteObject, SetWindowRgn, RGN_OR,
    };
    unsafe {
        let mut union = None;
        for &(l, t, r, b, radius) in rects {
            let piece = CreateRoundRectRgn(l, t, r + 1, b + 1, radius * 2, radius * 2);
            match union {
                None => union = Some(piece),
                Some(acc) => {
                    CombineRgn(Some(acc), Some(acc), Some(piece), RGN_OR);
                    let _ = DeleteObject(piece.into());
                }
            }
        }
        SetWindowRgn(webview.hwnd(), union, true);
    }
}

/// Remove any clip set with [`set_region`] or [`set_round_region`].
pub fn clear_region(webview: &WebView) {
    use windows::Win32::Graphics::Gdi::SetWindowRgn;
    unsafe {
        SetWindowRgn(webview.hwnd(), None, true);
    }
}

/// Ask the engine to save the page being shown (its own "Save as" dialog: HTML, MHTML or PDF).
pub fn save_page(webview: &WebView) {
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return;
    };
    let Ok(core25) = core.cast::<ICoreWebView2_25>() else {
        return;
    };
    let handler = ShowSaveAsUICompletedHandler::create(Box::new(|_, _| Ok(())));
    let _ = unsafe { core25.ShowSaveAsUI(&handler) };
}

/* ---------- Bounce tracking ---------- */

/// A page left in less than this, without the user asking, was a hop in a redirect chain.
const BOUNCE_DWELL: std::time::Duration = std::time::Duration::from_secs(5);

/// The registrable domain of a URL's host.
pub fn site_of(url: &str) -> Option<(String, String)> {
    let host = crate::extract_host(url)?;
    let site = psl::domain_str(&host).unwrap_or(&host).to_string();
    Some((site, host))
}

/// Report the tab's redirect hops and the sites the user really visits (see `shoji_core::bounce`):
/// a server redirect through another site, or a page that sends you on to another site within
/// a few seconds without you asking, is a hop; a page you open yourself, or stay on, is a visit.
pub fn attach_bounce_watch(webview: &WebView, proxy: EventLoopProxy<BrowserEvent>) {
    let Ok(core) = (unsafe { webview.controller().CoreWebView2() }) else {
        return;
    };
    #[derive(Default)]
    struct Watch {
        /// The last address of the navigation under way
        chain: String,
        /// The page on screen and when it arrived
        page: Option<(String, std::time::Instant)>,
    }
    let state = std::rc::Rc::new(std::cell::RefCell::new(Watch::default()));
    let web = |u: &str| {
        (u.starts_with("https://") || u.starts_with("http://")) && !u.starts_with("http://shoji.")
    };
    let mut token = Default::default();
    {
        let state = state.clone();
        let handler = NavigationStartingEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut uri = PWSTR::null();
            unsafe { args.Uri(&mut uri)? };
            let uri = take(uri);
            let (mut redirected, mut user) = (BOOL(0), BOOL(0));
            let _ = unsafe { args.IsRedirected(&mut redirected) };
            let _ = unsafe { args.IsUserInitiated(&mut user) };
            let mut st = state.borrow_mut();
            let send = |host: String, hop: bool| {
                let _ = proxy.send_event(BrowserEvent::Bounce { host, hop });
            };
            let to_site = site_of(&uri).map(|s| s.0);
            if redirected.as_bool() {
                let from = std::mem::replace(&mut st.chain, uri.clone());
                if let Some((site, host)) = site_of(&from).filter(|_| web(&from)) {
                    if to_site.as_deref() != Some(site.as_str()) {
                        send(host, true);
                    }
                }
                return Ok(());
            }
            st.chain = uri.clone();
            if let Some((page, at)) = st.page.take() {
                if let Some((site, host)) = site_of(&page).filter(|_| web(&page)) {
                    if at.elapsed() >= BOUNCE_DWELL {
                        send(host, false);
                    } else if !user.as_bool() && to_site.as_deref() != Some(site.as_str()) {
                        send(host, true);
                    }
                }
            }
            if user.as_bool() && web(&uri) {
                if let Some((_, host)) = site_of(&uri) {
                    send(host, false);
                }
            }
            Ok(())
        }));
        let _ = unsafe { core.add_NavigationStarting(&handler, &mut token) };
    }
    {
        let core_src = core.clone();
        let handler = SourceChangedEventHandler::create(Box::new(move |_, _| {
            let mut src = PWSTR::null();
            unsafe { core_src.Source(&mut src)? };
            let src = take(src);
            let mut st = state.borrow_mut();
            // A same-document change keeps the time the page arrived.
            if st.page.as_ref().map(|p| site_of(&p.0)) != Some(site_of(&src)) {
                st.page = Some((src, std::time::Instant::now()));
            }
            Ok(())
        }));
        let _ = unsafe { core.add_SourceChanged(&handler, &mut token) };
    }
}
