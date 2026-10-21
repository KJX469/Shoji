//! A browser window: its views, layout and what it shows.

use super::*;

impl BrowserWindow {
    /// Height of the title bar and toolbar. The find and permission cards float over the page,
    /// so this never changes and the page is never resized by them.
    pub(super) fn chrome_height(&self) -> f64 {
        if self.fullscreen {
            0.0
        } else if self.bar {
            CHROME_HEIGHT + BAR_HEIGHT
        } else {
            CHROME_HEIGHT
        }
    }

    /// Width the page gives up to a pinned side panel (floating dropdowns take none).
    pub(super) fn docked_width(&self) -> f64 {
        if self.sidebar_open && self.sidebar_docked {
            SIDEBAR_WIDTH
        } else {
            0.0
        }
    }

    pub(super) fn content_tuple(&self) -> (f64, f64, f64, f64) {
        let top = self.chrome_height();
        let width = (self.width - self.docked_width()).max(1.0);
        (0.0, top, width, (self.height - top).max(1.0))
    }

    pub(super) fn content_bounds(&self) -> Rect {
        let (x, y, w, h) = self.content_tuple();
        bounds(x, y, w, h)
    }

    /// Where the floating dropdown sits: under its toolbar button, right edge kept inside.
    pub(super) fn popover_frame(&self) -> Frame {
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
    pub(super) fn pop_cx(&self) -> f64 {
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
    pub(super) fn split_showing(&self) -> Option<Vec<TabId>> {
        self.tabs.split_panes()
    }

    /// Which split divider `(x, y)` is on, if any.
    pub(super) fn divider_hit(&self, x: f64, y: f64) -> Option<usize> {
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
    pub(super) fn divider_centre(&self, index: usize) -> Option<(SplitOrientation, f64)> {
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
    pub(super) fn push_pane_rects(&mut self) {
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
    pub(super) fn layout_panes_only(&mut self) {
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
    pub(super) fn drive_divider(&mut self, cursor_screen: (i32, i32)) {
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
    pub(super) fn end_divider_drag(&mut self) {
        if self.divider_dragging {
            self.divider_dragging = false;
            self.ui_dirty = true;
            self.window.set_cursor(winit::window::CursorIcon::Default);
        }
    }

    /// Where tab `id` sits: full window in fullscreen, its split pane rectangle, or the page area.
    pub(super) fn rect_for(&self, id: TabId) -> Rect {
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

    pub(super) fn active(&self) -> Option<&Tab> {
        self.views.get(&self.tabs.active_tab_id()?)
    }

    /// Only the visible tab(s) are resized; hidden tabs get their bounds when shown.
    pub(super) fn layout(&mut self) {
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
    pub(super) fn notes_desk(&self) -> Frame {
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
    pub(super) fn notes_sheet(&self) -> Frame {
        let [_, _, w, h] = self.notes_desk();
        if self.notes_expanded {
            [0.0, 0.0, w, h]
        } else {
            let width = NOTES_WIDTH.min(w);
            [w - width, 0.0, width, NOTES_MAX_HEIGHT.min(h)]
        }
    }

    pub(super) fn set_notes_clip(&mut self, clip: Frame) {
        if let Some(n) = &self.notes {
            let scale = self.window.scale_factor();
            let px = clip.map(|v| (v * scale).round() as i32);
            webview::set_region(n, px[0], px[1], px[0] + px[2].max(1), px[1] + px[3].max(1));
        }
        self.notes_clip = Some(clip);
    }

    pub(super) fn animate_notes(&mut self, from: Frame, to: Frame, ms: f32, closing: bool) {
        self.notes_anim = Some(NotesAnim {
            start: Instant::now(),
            duration: Duration::from_secs_f32(ms / 1000.0),
            from,
            to,
            closing,
        });
    }

    /// Advance the notepad animation one frame. Returns true while it is still running.
    pub(super) fn step_notes(&mut self) -> bool {
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
    pub(super) fn raise_overlays(&self) {
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
    pub(super) fn step_popover(&mut self) -> Option<Instant> {
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
    pub(super) fn sync_frame(&mut self) {
        let state = (self.window.is_maximized(), self.window.has_focus());
        if self.frame_state != Some(state) {
            self.frame_state = Some(state);
            self.eval_chrome(&format!(
                "window.__syncWindow && window.__syncWindow({{maximized:{},focused:{}}})",
                state.0, state.1
            ));
        }
    }

    pub(super) fn eval_chrome(&self, js: &str) {
        if let Some(c) = &self.chrome {
            let _ = c.evaluate_script(js);
        }
    }

    pub(super) fn eval_sidebar(&self, js: &str) {
        if let Some(s) = &self.sidebar {
            let _ = s.evaluate_script(js);
        }
    }

    /// Run a script in the window's internal pages (new tab, settings, welcome).
    pub(super) fn eval_internal_tabs(&self, js: &str) {
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
    pub(super) fn settle_veil(&mut self) -> Option<Instant> {
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

    pub(super) fn hide(tab: &mut Tab) {
        let _ = tab.view.set_visible(false);
        let _ = tab.view.set_memory_usage_level(wry::MemoryUsageLevel::Low);
        tab.hidden_since = Some(Instant::now());
    }

    /// Bring `id` to the front, demoting whichever tab was visible.
    pub(super) fn show_tab(&mut self, id: TabId) {
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

    pub(super) fn zoom(&self) -> f64 {
        let mut factor = 1.0;
        if let Some(tab) = self.active() {
            let _ = unsafe { tab.view.controller().ZoomFactor(&mut factor) };
        }
        factor
    }

    pub(super) fn set_zoom(&self, factor: f64) {
        let factor = ((factor * 100.0).round() / 100.0).clamp(0.25, 3.0);
        if let Some(tab) = self.active() {
            let _ = unsafe { tab.view.controller().SetZoomFactor(factor) };
        }
        self.sync_zoom();
    }

    pub(super) fn sync_zoom(&self) {
        let js = format!("window.__syncZoom && window.__syncZoom({})", self.zoom());
        self.eval_chrome(&js);
        self.eval_sidebar(&js);
    }

    pub(super) fn find(&self, query: &str, forward: bool) {
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
