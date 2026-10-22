//! Shoji's password manager: unlocking, offering to save, filling.

use super::*;

/// A breach check that couldn't reach the service is tried again after this.
const BREACH_RETRY: Duration = Duration::from_secs(10 * 60);

/// What the breach check has found while the vault is open, by password hash.
#[derive(Default)]
pub(super) struct Breaches {
    counts: HashMap<String, u64>,
    running: bool,
    failed_at: Option<Instant>,
    /// The passwords page's search, to list again when an answer arrives
    query: String,
}

impl BrowserApp {
    /// Make the vault usable: open an OS-protected one, or (when `create`) start one. A vault
    /// with a master password only opens when the user types it.
    pub(super) fn vault_open(&mut self, create: bool) -> bool {
        use shoji_core::vault::State;
        match self.vault.state() {
            State::Unlocked => true,
            State::Locked => {
                !self.vault.has_master() && self.vault.unlock_auto(&vault_win::WinProtector)
            }
            State::Empty => create && self.vault.create(&vault_win::WinProtector),
        }
    }

    pub(super) fn vault_state_json(&mut self) -> serde_json::Value {
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

    pub(super) fn push_vault(&mut self) {
        let js = format!(
            "window.__vaultState && window.__vaultState({})",
            self.vault_state_json()
        );
        self.eval_all_internal(&js);
    }

    pub(super) fn vault_toast(&self, text: &str) {
        self.eval_all_internal(&format!(
            "window.__vaultToast && window.__vaultToast({})",
            js_string(text)
        ));
    }

    pub(super) fn push_vault_list(&mut self, query: &str) {
        if !self.vault.is_unlocked() {
            return;
        }
        self.breaches.query = query.to_string();
        let audit = self.vault.audit();
        let weak: HashSet<&String> = audit.weak.iter().collect();
        let reused: HashSet<&String> = audit.reused.iter().flatten().collect();
        let check = self.settings.privacy.breach_check;
        // Hashes are kept only while the vault is open, and never written anywhere.
        let hashes: HashMap<&str, String> = if check {
            self.vault
                .logins()
                .iter()
                .filter(|l| !l.password.is_empty())
                .map(|l| (l.id.as_str(), shoji_core::breach::sha1_hex(&l.password)))
                .collect()
        } else {
            HashMap::new()
        };
        let seen = |id: &str| {
            hashes
                .get(id)
                .and_then(|h| self.breaches.counts.get(h))
                .copied()
                .unwrap_or(0)
        };
        let items: Vec<serde_json::Value> = self
            .vault
            .search(query)
            .iter()
            .map(|l| {
                serde_json::json!({
                    "id": l.id, "title": l.title, "url": l.url, "host": l.host(),
                    "username": l.username, "modified": l.modified,
                    "weak": weak.contains(&l.id), "reused": reused.contains(&l.id),
                    "breached": seen(&l.id),
                })
            })
            .collect();
        let breached = self
            .vault
            .logins()
            .iter()
            .filter(|l| seen(&l.id) > 0)
            .count();
        let unknown: Vec<String> = hashes
            .values()
            .filter(|h| !self.breaches.counts.contains_key(*h))
            .cloned()
            .collect();
        let status = if !check {
            "off"
        } else if self.breaches.running {
            "checking"
        } else if unknown.is_empty() {
            "done"
        } else if self
            .breaches
            .failed_at
            .is_some_and(|t| t.elapsed() < BREACH_RETRY)
        {
            "failed"
        } else {
            self.start_breach_check(unknown);
            "checking"
        };
        let payload = serde_json::json!({
            "items": items, "weak": audit.weak.len(), "reused": audit.reused.iter().map(Vec::len).sum::<usize>(),
            "breached": breached, "breachCheck": status,
        });
        self.eval_all_internal(&format!(
            "window.__vaultList && window.__vaultList({payload})"
        ));
    }

    /// Look `hashes` up in the background; the answer comes back as `VaultBreaches`.
    fn start_breach_check(&mut self, hashes: Vec<String>) {
        self.breaches.running = true;
        let proxy = self.proxy.clone();
        let spawned = std::thread::Builder::new()
            .name("shoji-breaches".into())
            .spawn(move || {
                let agent: ureq::Agent = ureq::Agent::config_builder()
                    .timeout_global(Some(Duration::from_secs(20)))
                    .build()
                    .into();
                let mut all = true;
                let found = shoji_core::breach::check(hashes.iter().map(String::as_str), |p| {
                    let body = agent
                        .get(&format!("{}{p}", shoji_core::breach::RANGE_URL))
                        // Every answer is padded to a similar size, so its length says nothing.
                        .header("Add-Padding", "true")
                        .header("User-Agent", "Shoji")
                        .call()
                        .ok()
                        .and_then(|mut r| r.body_mut().read_to_string().ok());
                    all &= body.is_some();
                    body
                });
                let _ = proxy.send_event(BrowserEvent::VaultBreaches(found, all));
            });
        if spawned.is_err() {
            self.breaches.running = false;
        }
    }

    /// A breach check finished: `found` has a count for each hash it could look up.
    pub(super) fn breaches_checked(&mut self, found: HashMap<String, u64>, complete: bool) {
        self.breaches.running = false;
        self.breaches.failed_at = (!complete).then(Instant::now);
        if !self.vault.is_unlocked() {
            return;
        }
        self.breaches.counts.extend(found);
        let query = self.breaches.query.clone();
        self.push_vault_list(&query);
    }

    /// Forget what the breach check learned (the vault was locked).
    pub(super) fn forget_breaches(&mut self) {
        self.breaches.counts.clear();
        self.breaches.failed_at = None;
    }

    pub(super) fn vault_changed(&mut self) {
        self.push_vault();
        self.push_vault_list("");
    }

    /// Whether the saved-login prompts and fill menu are on for `win_id`'s active page, and the
    /// page's address.
    pub(super) fn login_page(&self, win_id: WindowId, tab: TabId) -> Option<String> {
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
    pub(super) fn offer_save(&mut self, win_id: WindowId) {
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

    pub(super) fn commit_login(&mut self, choice: &str) {
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
    pub(super) fn fill_login(&mut self, win_id: WindowId, id: &str) {
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
    pub(super) fn vault_tick(&mut self) -> Option<Instant> {
        if self.vault.lock_if_idle(VAULT_IDLE) {
            self.forget_breaches();
            self.push_vault();
        }
        self.vault.idle_deadline(VAULT_IDLE)
    }
}
