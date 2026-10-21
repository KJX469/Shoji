//! Split view, workspaces, reader, notes, the side panel and the crane.

use super::*;

impl BrowserApp {
    /// Ask which tab should sit beside the active one. Split groups are only made on purpose.
    pub(super) fn open_split_picker(&mut self, win_id: WindowId) {
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
    pub(super) fn split_with(&mut self, win_id: WindowId, other: Option<TabId>) {
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
    pub(super) fn switch_workspace(&mut self, win_id: WindowId, ws_id: u32) {
        let switched = self.windows.get_mut(&win_id).is_some_and(|win| {
            win.fullscreen_tab = None;
            win.tabs.switch_workspace(ws_id, now_secs())
        });
        if switched {
            self.sync_window_views(win_id);
        }
    }

    pub(super) fn delete_workspace(&mut self, win_id: WindowId, ws_id: u32) {
        let deleted = self
            .windows
            .get_mut(&win_id)
            .is_some_and(|win| win.tabs.delete_workspace(ws_id, now_secs()));
        if deleted {
            self.sync_window_views(win_id);
            self.push_data();
        }
    }

    pub(super) fn move_tab_to_workspace(
        &mut self,
        win_id: WindowId,
        tab_id: TabId,
        target_ws: u32,
    ) {
        let moved = self.windows.get_mut(&win_id).is_some_and(|win| {
            win.tabs
                .move_tab_to_workspace(tab_id, target_ws, now_secs())
        });
        if moved {
            self.sync_window_views(win_id);
        }
    }

    pub(super) fn toggle_reader(&mut self, win_id: WindowId) {
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

    pub(super) fn open_notes(&mut self, win_id: WindowId) {
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

    pub(super) fn set_notes_expanded(&mut self, win_id: WindowId, expanded: bool) {
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

    pub(super) fn close_notes(&mut self, win_id: WindowId) {
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

    pub(super) fn toggle_sidebar(&mut self, win_id: WindowId, mode: &'static str) {
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
    pub(super) fn ensure_sidebar(&mut self, win_id: WindowId) {
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
    pub(super) fn warm_sidebars(&mut self) -> Option<Instant> {
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
    pub(super) fn open_sidebar(
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
    pub(super) fn fold_crane(&mut self) {
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
    pub(super) fn clean_bounce_trackers(&mut self) {
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
    pub(super) fn note_ui_window(&self) {
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
    pub(super) fn mention_crash(&self) {
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
    pub(super) fn toast(&self, toast: &serde_json::Value) {
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

    pub(super) fn push_downloads(&mut self) {
        let json = serde_json::to_string(self.downloads.items()).unwrap_or_else(|_| "[]".into());
        let js = format!("window.__downloadsList && window.__downloadsList({json})");
        for win in self.windows.values() {
            win.eval_sidebar(&js);
        }
        self.eval_all_internal(&js);
    }

    /// Close the dropdown (or pinned panel). `refocus` hands the keyboard back to the page, as
    /// when the user pressed Esc; a click elsewhere has already given focus to what it hit.
    pub(super) fn close_sidebar(&mut self, win_id: WindowId, refocus: bool) {
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
    pub(super) fn step_hover(&mut self) -> Option<Instant> {
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
}
