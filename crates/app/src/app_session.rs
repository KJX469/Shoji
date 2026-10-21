//! Saving and restoring the open windows and tabs.

use super::*;

impl BrowserApp {
    pub(super) fn build_session(&self, clean: bool) -> shoji_core::userdata::Session {
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

    pub(super) fn session_enabled(&self) -> bool {
        !self.settings.privacy.ephemeral_default && self.settings.privacy.restore_session != "never"
    }

    pub(super) fn write_session(&mut self, clean: bool) {
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
    pub(super) fn maybe_write_session(&mut self) -> Option<Instant> {
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
    pub(super) fn open_session_window(
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

    pub(super) fn restore_session(&mut self, win_id: WindowId, event_loop: &ActiveEventLoop) {
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
}
