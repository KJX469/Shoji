//! The floating overlay: suggestions, the command palette, prompts and menus.

use super::*;

impl BrowserApp {
    pub(super) fn ensure_overlay(&mut self, win_id: WindowId) {
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
    pub(super) fn show_overlay(
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

    pub(super) fn hide_overlay(&mut self, win_id: WindowId, refocus: bool) {
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

    pub(super) fn open_prompt(
        &mut self,
        win_id: WindowId,
        prompt: Prompt,
        title: &str,
        value: &str,
    ) {
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

    pub(super) fn finish_prompt(&mut self, win_id: WindowId, text: &str) {
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

    pub(super) fn palette_items(&self, win_id: WindowId) -> Vec<shoji_core::ipc::OverlayItem> {
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
            ("extensions", "Extensions", ""),
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

    pub(super) fn tab_menu_items(
        &self,
        win_id: WindowId,
        tab_id: TabId,
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

    pub(super) fn workspace_menu_items(
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
    pub(super) fn add_new_pane(&mut self, win_id: WindowId, url: Option<String>) {
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

    pub(super) fn run_action(
        &mut self,
        win_id: WindowId,
        value: &str,
        event_loop: &ActiveEventLoop,
    ) {
        use UiToHostMessage as M;
        // "split-open:<address or search>" carries a colon of its own, so it is read first.
        if let Some(rest) = value.strip_prefix("split-open:") {
            return self.add_new_pane(win_id, Some(rest.to_string()));
        }
        if let Some(id) = value.strip_prefix("ext-open:") {
            let right = self
                .windows
                .get(&win_id)
                .and_then(|w| w.anchors.get("ext").copied())
                .map_or(0.0, |x| x + 16.0);
            return self.toggle_extension_popup(win_id, id, right);
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
            "extensions" => self.open_internal(win_id, "extensions"),
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
}
