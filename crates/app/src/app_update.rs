//! The update letter: checking, offering and installing new versions.

use super::*;

impl BrowserApp {
    /// Tell every window's letter, and any open Settings page, where the update is at.
    pub(super) fn push_update(&self) {
        let state = self
            .update
            .json(&self.update_memo, update::running_installed_copy());
        let js = format!("window.__updateState && window.__updateState({state})");
        for win in self.windows.values() {
            win.eval_chrome(&js);
            win.eval_internal_tabs(&js);
        }
    }

    pub(super) fn update_request(&self, request: update::Request) {
        if let Some(tx) = &self.update_tx {
            let _ = tx.send(request);
        }
    }

    /// Let the letter open by itself in the focused window, unless the user said Later, skipped
    /// this version or saw it already today. Never over full screen and never during setup;
    /// without a focused window it waits for one. It never takes focus.
    pub(super) fn maybe_unfold_letter(&mut self) {
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
    pub(super) fn update_choice(&mut self, win_id: WindowId, choice: &str) {
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
            "rollback" => self.start_rollback(),
            "keep-version" => {
                self.rollback_offer = None;
                shoji_core::launches::Launches::keep(&self.launches_path);
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
    pub(super) fn proceed_update(&mut self) {
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
    pub(super) fn stage_update(&mut self) {
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
    pub(super) fn apply_update(&mut self) {
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
    pub(super) fn restart_into_update(&mut self) {
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
    pub(super) fn greet_after_update(&mut self) {
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

    /// This run has lasted long enough, or is quitting normally: it wasn't an early close.
    pub(super) fn settle_launch(&mut self) {
        if self.launch_settles.take().is_some() {
            shoji_core::launches::Launches::settle(&self.launches_path);
        }
    }

    /// This version closed unexpectedly twice just after starting: offer, once per start, to go
    /// back to the version before it. Returns whether the offer was shown.
    pub(super) fn offer_rollback(&mut self) -> bool {
        let Some(previous) = self.rollback_offer.clone() else {
            return false;
        };
        if self.rolling_back || !update::running_installed_copy() {
            return false;
        }
        crash::mark_seen(&self.data_dir);
        let current = env!("CARGO_PKG_VERSION");
        self.toast(&serde_json::json!({
            "title": format!("Shoji {current} keeps closing"),
            "text": format!("It closed unexpectedly twice just after starting. Shoji {previous} can be put back, with your tabs and settings."),
            "pose": "bow",
            "ms": 30000,
            "actions": [
                {"label": format!("Go back to {previous}"), "action": "UpdateAction",
                 "payload": {"choice": "rollback"}, "primary": true},
                {"label": format!("Keep {current}"), "action": "UpdateAction",
                 "payload": {"choice": "keep-version"}},
            ],
        }));
        true
    }

    /// "Go back": skip this version from now on and fetch (or reuse) the earlier installer.
    fn start_rollback(&mut self) {
        let Some(previous) = self.rollback_offer.clone() else {
            return;
        };
        if self.rolling_back || !update::running_installed_copy() {
            return;
        }
        self.rolling_back = true;
        self.update_memo.skip = env!("CARGO_PKG_VERSION").to_string();
        self.update_memo.installing = None;
        self.update_memo.save(&self.update_memo_path);
        self.update_request(update::Request::Rollback(update::release_of(&previous)));
        self.toast(&serde_json::json!({
            "title": format!("Going back to Shoji {previous}"),
            "text": "Shoji restarts with your tabs in a moment.",
            "pose": "carry",
            "ms": 8000,
        }));
    }

    /// The earlier installer is ready: restart into it, with the open tabs.
    pub(super) fn rollback_ready(&mut self, version: &str, result: Result<PathBuf, String>) {
        if !self.rolling_back {
            return;
        }
        let started = result.and_then(|installer| {
            self.write_session(true);
            let mut cmd = std::process::Command::new(installer);
            cmd.args(["--update", "--reopen"]);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0000_0008); // DETACHED_PROCESS: outlives this copy
            }
            cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
        });
        if started.is_ok() {
            self.update_started = true;
            return;
        }
        self.rolling_back = false;
        self.toast(&serde_json::json!({
            "title": format!("Couldn't get Shoji {version}"),
            "text": "Check the connection and try again, or install it from its release page.",
            "pose": "bow",
            "ms": 12000,
            "actions": [
                {"label": "Try again", "action": "UpdateAction",
                 "payload": {"choice": "rollback"}, "primary": true},
                {"label": "Release page", "action": "CreateTab",
                 "payload": {"url": update::release_of(version).page_url}},
            ],
        }));
    }

    /* ----- password manager ----- */
}
