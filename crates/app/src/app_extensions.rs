//! Browser extensions: listing, installing, and their toolbar buttons and popups.

use super::*;

impl BrowserApp {
    pub(super) fn request_extension_list(&self) {
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
    pub(super) fn install_extension_from_store(&self, source: &str) {
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
    pub(super) fn add_extension_from_folder(&self) {
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
}

/// An extension's popup, open under its toolbar button.
pub(super) struct ExtensionPopup {
    id: String,
    view: WebView,
    /// Right edge of the button it hangs from (logical px)
    right: f64,
    /// It has been sized and shown
    shown: bool,
}

/// Chrome's limits for a popup's size.
const POPUP_MIN: (f64, f64) = (25.0, 25.0);
const POPUP_MAX: (f64, f64) = (800.0, 600.0);

/// Runs in the popup's own page: report the size its content wants, as it changes. Measured at
/// its natural width, and from a short viewport so content taller than it shows.
const POPUP_SIZE_SCRIPT: &str = r#"(() => {
  if (window.top !== window) return;
  let queued = false;
  const send = () => {
    queued = false;
    const d = document.documentElement;
    if (!d || !window.ipc) return;
    const was = d.style.width;
    d.style.width = 'max-content';
    const w = Math.ceil(d.getBoundingClientRect().width);
    d.style.width = was;
    const h = Math.ceil(Math.max(d.scrollHeight, document.body ? document.body.scrollHeight : 0));
    window.ipc.postMessage(JSON.stringify({ w, h }));
  };
  const queue = () => { if (!queued) { queued = true; requestAnimationFrame(send); } };
  addEventListener('DOMContentLoaded', () => {
    const ro = new ResizeObserver(queue);
    ro.observe(document.documentElement);
    if (document.body) ro.observe(document.body);
    queue();
  });
  addEventListener('load', queue);
  addEventListener('keydown', e => { if (e.key === 'Escape' && !e.defaultPrevented) window.ipc.postMessage('"close"'); });
})();"#;

#[derive(serde::Deserialize)]
#[serde(untagged)]
enum PopupMessage {
    Size { w: f64, h: f64 },
    Close(String),
}

impl BrowserApp {
    /// What the engine's extension list means for the toolbar and Settings: each extension's
    /// pages and icon, matched from its folder. Returns the list for Settings.
    pub(super) fn extensions_listed(&mut self, json: &str) -> String {
        let pages = extstore::pages(&self.data_dir.join("Extensions"));
        let mut items: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
        let mut buttons = Vec::new();
        self.extension_popups.clear();
        self.extension_menu.clear();
        for item in &mut items {
            let id = item["id"].as_str().unwrap_or("").to_string();
            let name = item["name"].as_str().unwrap_or("").to_string();
            let enabled = item["enabled"].as_bool().unwrap_or(true);
            let Some(p) = extstore::find(&pages, &id, &name) else {
                continue;
            };
            let url =
                |p: &Option<String>| p.as_ref().map(|p| format!("chrome-extension://{id}/{p}"));
            let (popup, options) = (url(&p.popup), url(&p.options));
            item["popup"] = serde_json::json!(popup);
            item["options"] = serde_json::json!(options);
            let pinned = !self.settings.extensions.unpinned.contains(&id);
            item["pinned"] = serde_json::json!(pinned);
            item["icon"] = serde_json::json!(p.icon);
            if !enabled {
                continue;
            }
            // Every enabled extension can open: its popup, else its options page. Pinned ones
            // also get a toolbar button.
            if let Some(target) = popup.clone().or(options) {
                if pinned {
                    buttons.push(serde_json::json!({
                        "id": id, "name": name, "icon": p.icon, "popup": popup.is_some(),
                    }));
                }
                self.extension_menu.push((id.clone(), name.clone()));
                self.extension_popups.insert(id, (target, popup.is_some()));
            }
        }
        self.extension_buttons = serde_json::json!({
            "buttons": buttons,
            "any": !self.extension_popups.is_empty(),
        })
        .to_string();
        let ids: Vec<WindowId> = self.windows.keys().copied().collect();
        for id in ids {
            self.push_extension_buttons(id);
        }
        serde_json::Value::Array(items).to_string()
    }

    /// Check the store for newer versions of installed extensions, at most every six hours.
    pub(super) fn check_extension_updates(&mut self) {
        const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
        if self.extensions_checked.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.extensions_checked = Some(Instant::now());
        let proxy = self.proxy.clone();
        let dir = self.data_dir.join("Extensions");
        let _ = std::thread::Builder::new()
            .name("shoji-extension-updates".into())
            .spawn(move || {
                let _ =
                    proxy.send_event(BrowserEvent::ExtensionsUpdated(extstore::update_all(&dir)));
            });
    }

    /// A tab reached a Chrome Web Store page for an extension that isn't installed: offer to add
    /// it, once per extension each session.
    pub(super) fn offer_store_extension(&mut self, url: &str) {
        if !EXTENSIONS_ON.load(std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        let Some(id) = extstore::store_page_id(url) else {
            return;
        };
        let installed = extstore::installed_versions(&self.data_dir.join("Extensions"));
        if installed.iter().any(|(i, _)| *i == id) || !self.extensions_offered.insert(id.clone()) {
            return;
        }
        self.toast(&serde_json::json!({
            "title": "Add this extension to Shoji?",
            "text": "It is downloaded from the Chrome Web Store and added in a moment.",
            "pose": "unfold",
            "ms": 12000,
            "actions": [{ "label": "Add to Shoji", "primary": true, "action": "InstallExtension", "payload": { "source": id } }],
        }));
    }

    /// Send the downloaded catalog to the open store pages.
    pub(super) fn push_extension_catalog(&mut self) {
        let Some(json) = extstore::read_catalog(&self.data_dir) else {
            return;
        };
        let js = format!("window.__catalog && window.__catalog({json})");
        let ids: Vec<WindowId> = self.windows.keys().copied().collect();
        for id in ids {
            self.eval_welcome_and_settings(id, &js);
        }
    }

    /// Download a newer catalog if the one on disk is old, at most every six hours.
    pub(super) fn refresh_extension_catalog(&mut self) {
        const EVERY: Duration = Duration::from_secs(6 * 60 * 60);
        if self.catalog_checked.is_some_and(|t| t.elapsed() < EVERY) {
            return;
        }
        self.catalog_checked = Some(Instant::now());
        let proxy = self.proxy.clone();
        let dir = self.data_dir.clone();
        let _ = std::thread::Builder::new()
            .name("shoji-extension-catalog".into())
            .spawn(move || {
                if extstore::refresh_catalog(&dir) == Ok(true) {
                    let _ = proxy.send_event(BrowserEvent::CatalogUpdated);
                }
            });
    }

    pub(super) fn push_extension_buttons(&self, win_id: WindowId) {
        if let Some(win) = self.windows.get(&win_id) {
            win.eval_chrome(&format!(
                "window.__extButtons && window.__extButtons({})",
                self.extension_buttons
            ));
        }
    }

    /// The extension's toolbar button was clicked: open its popup under it, or close it if it
    /// is the one open. An extension without a popup opens its options page in a tab.
    pub(super) fn toggle_extension_popup(&mut self, win_id: WindowId, id: &str, right: f64) {
        let Some((target, is_popup)) = self.extension_popups.get(id).cloned() else {
            eprintln!("extension popup: no page known for {id}");
            return;
        };
        if !right.is_finite() {
            eprintln!("extension popup: bad button position {right}");
            return;
        }
        let same = self
            .windows
            .get(&win_id)
            .and_then(|w| w.ext_popup.as_ref())
            .is_some_and(|p| p.id == id)
            || self
                .popup_blurred
                .take()
                .is_some_and(|(b, at)| b == id && at.elapsed() < Duration::from_millis(400));
        self.close_extension_popup(win_id);
        if same {
            eprintln!("extension popup: {id} was open, closed it");
            return;
        }
        if !is_popup {
            self.create_tab(win_id, Some(target));
            return;
        }
        self.hide_overlay(win_id, false);
        if self
            .windows
            .get(&win_id)
            .is_some_and(|w| w.sidebar_open && !w.sidebar_docked)
        {
            self.close_sidebar(win_id, false);
        }
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        let proxy = self.proxy.clone();
        let close_proxy = self.proxy.clone();
        let dark = window::is_dark(&win.window, &self.settings.appearance.theme);
        let top = CHROME_HEIGHT + 2.0;
        let x = (right - POPUP_MIN.0).clamp(8.0, (win.width - POPUP_MIN.0 - 8.0).max(8.0));
        let view = new_builder()
            .with_bounds(bounds(x, top, POPUP_MIN.0, POPUP_MIN.1))
            .with_visible(false)
            // The real profile, where extensions and their storage live.
            .with_incognito(false)
            .with_devtools(true)
            .with_background_color(window::page_rgba(dark))
            .with_theme(window::webview_theme(&self.settings.appearance.theme))
            .with_initialization_script(POPUP_SIZE_SCRIPT)
            // The page is the extension's, so the only thing it may say is how big it is.
            .with_ipc_handler(move |req: wry::http::Request<String>| {
                let event = match serde_json::from_str::<PopupMessage>(req.body()) {
                    Ok(PopupMessage::Size { w, h }) if w.is_finite() && h.is_finite() => {
                        BrowserEvent::ExtensionPopupSize(win_id, w, h)
                    }
                    Ok(PopupMessage::Close(c)) if c == "close" => {
                        BrowserEvent::ExtensionPopupClosed(win_id)
                    }
                    _ => return,
                };
                let _ = proxy.send_event(event);
            })
            .with_url(&target)
            .build_as_child(win.window.as_ref());
        let view = match view {
            Ok(view) => view,
            Err(e) => {
                eprintln!("extension popup: could not open {target}: {e:?}");
                self.toast(&serde_json::json!({
                    "title": "That extension's popup wouldn't open.",
                    "text": format!("{e}"),
                    "pose": "unfold",
                }));
                return;
            }
        };
        eprintln!("extension popup: opened {target}");
        webview::attach_popup_events(&view, move || {
            let _ = close_proxy.send_event(BrowserEvent::ExtensionPopupClosed(win_id));
        });
        webview::bring_to_top(&view);
        win.ext_popup = Some(ExtensionPopup {
            id: id.to_string(),
            view,
            right,
            shown: false,
        });
    }

    /// The popup's content wants `w` × `h`: size it (within Chrome's limits) and show it.
    pub(super) fn size_extension_popup(&mut self, win_id: WindowId, w: f64, h: f64) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        let (width, height) = (win.width, win.height);
        let Some(p) = win.ext_popup.as_mut() else {
            return;
        };
        let top = CHROME_HEIGHT + 2.0;
        let w = w.clamp(POPUP_MIN.0, POPUP_MAX.0.min(width - 16.0).max(POPUP_MIN.0));
        let h = h.clamp(
            POPUP_MIN.1,
            POPUP_MAX.1.min(height - top - 8.0).max(POPUP_MIN.1),
        );
        let x = (p.right - w).clamp(8.0, (width - w - 8.0).max(8.0));
        let _ = p.view.set_bounds(bounds(x, top, w, h));
        if !p.shown {
            p.shown = true;
            let _ = p.view.set_visible(true);
            webview::bring_to_top(&p.view);
            webview::focus(&p.view);
        }
    }

    /// The popup lost focus (or closed itself).
    pub(super) fn blur_extension_popup(&mut self, win_id: WindowId) {
        if let Some(p) = self.windows.get(&win_id).and_then(|w| w.ext_popup.as_ref()) {
            self.popup_blurred = Some((p.id.clone(), Instant::now()));
        }
        self.close_extension_popup(win_id);
    }

    pub(super) fn close_extension_popup(&mut self, win_id: WindowId) {
        if let Some(win) = self.windows.get_mut(&win_id) {
            // Dropping the view closes it.
            win.ext_popup = None;
        }
    }
}

impl BrowserApp {
    /// The puzzle button's menu: every extension that can open, then the manager.
    pub(super) fn show_extensions_menu(&mut self, win_id: WindowId, x: f64, y: f64) {
        use shoji_core::ipc::OverlayItem;
        let item = |value: String, label: String, sub: &str| OverlayItem {
            kind: "action".into(),
            value,
            label,
            sub: sub.into(),
            color: None,
        };
        let mut items: Vec<OverlayItem> = self
            .extension_menu
            .iter()
            .map(|(id, name)| {
                let pinned = !self.settings.extensions.unpinned.contains(id);
                item(
                    format!("ext-open:{id}"),
                    name.clone(),
                    if pinned { "On the toolbar" } else { "" },
                )
            })
            .collect();
        items.push(item("extensions".into(), "Manage extensions".into(), ""));
        let Some(win) = self.windows.get(&win_id) else {
            return;
        };
        let w = 280.0f64.min(win.width - 8.0);
        let x = (x - w / 2.0).clamp(4.0, (win.width - w - 4.0).max(4.0));
        let h = items.len() as f64 * 46.0 + 10.0;
        self.show_overlay(win_id, "menu", items, [x, y.max(30.0), w, h], "", "", "");
    }
}
