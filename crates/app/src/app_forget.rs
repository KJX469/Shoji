//! Forgetting sites: a site's cookies and data when its last tab closes, and every site's (but
//! the kept ones) when Shoji closes.

use super::*;
use std::rc::Rc;

/// How long closing waits for the engine to finish deleting before it goes ahead anyway.
const LEAVE_WAIT: Duration = Duration::from_secs(5);
/// How long a closed tab's view is kept to finish forgetting its site.
const PARK_WAIT: Duration = Duration::from_secs(15);

/// What happens once the deletions that closing waits for are done.
pub(super) enum Leaving {
    Close(WindowId),
    Quit,
}

/// Start deleting `forget`'s sites through `view` (its profile, see `webview::forget_sites`), or
/// every site's data with `everything`; `job` is reported back when the engine is done.
fn start_forget(
    proxy: &EventLoopProxy<BrowserEvent>,
    job: u64,
    view: &wry::WebView,
    forget: impl Fn(&str) -> bool + 'static,
    hosts: Vec<String>,
    everything: bool,
) {
    let proxy = proxy.clone();
    let done = move || {
        let _ = proxy.send_event(BrowserEvent::SiteDataForgotten(job));
    };
    if everything {
        webview::clear_all_site_data(view, done);
    } else {
        webview::forget_sites(view, forget, hosts, done);
    }
}

impl BrowserApp {
    /// Tabs whose views were just removed: a site set to be forgotten goes with its last tab
    /// (in the same container). The deletion runs through another view of the same profile, or
    /// the closed tab's own view is kept, hidden, until the engine has finished.
    pub(super) fn forget_closed_tabs(&mut self, closed: Vec<Tab>) {
        if self.settings.privacy.forget_on_close.is_empty() {
            return;
        }
        for tab in closed {
            if !tab.persistent {
                continue;
            }
            let url = tab.view.url().unwrap_or_default();
            if !self.settings.forgets_on_close(&url) {
                continue;
            }
            let Some((site, host)) = webview::site_of(&url) else {
                continue;
            };
            let still_open = self.windows.values().any(|w| {
                w.tabs.all_tabs().any(|t| {
                    t.container == tab.container
                        && webview::site_of(&t.url).is_some_and(|(s, _)| s == site)
                })
            });
            if still_open {
                continue;
            }
            self.next_forget += 1;
            let job = self.next_forget;
            let other = self
                .windows
                .values()
                .filter(|w| !w.private)
                .flat_map(|w| w.views.values())
                .find(|t| t.persistent && t.container == tab.container);
            let forget = move |s: &str| s == site;
            match other {
                Some(other) => {
                    start_forget(&self.proxy, job, &other.view, forget, vec![host], false);
                    self.forgetting.insert(job, (None, Instant::now()));
                }
                None => {
                    start_forget(&self.proxy, job, &tab.view, forget, vec![host], false);
                    let _ = tab.view.set_visible(false);
                    self.forgetting.insert(job, (Some(tab), Instant::now()));
                }
            }
        }
    }

    /// The engine finished one deletion.
    pub(super) fn site_data_forgotten(&mut self, job: u64, event_loop: &ActiveEventLoop) {
        self.forgetting.remove(&job);
        if self.forgetting.is_empty() {
            self.finish_leaving(event_loop);
        }
    }

    /// Drop deletions that never answered, and stop waiting for them once closing has waited
    /// long enough. Returns when to look again.
    pub(super) fn forget_tick(&mut self, event_loop: &ActiveEventLoop) -> Option<Instant> {
        let now = Instant::now();
        self.forgetting
            .retain(|_, (parked, at)| now.duration_since(*at) < PARK_WAIT || parked.is_none());
        if let Some((_, deadline)) = &self.leaving {
            if now >= *deadline || self.forgetting.is_empty() {
                self.finish_leaving(event_loop);
            }
        }
        let parked = self
            .forgetting
            .values()
            .filter(|(p, _)| p.is_some())
            .map(|(_, at)| *at + PARK_WAIT);
        self.leaving
            .as_ref()
            .map(|(_, d)| *d)
            .into_iter()
            .chain(parked)
            .min()
    }

    fn finish_leaving(&mut self, event_loop: &ActiveEventLoop) {
        let Some((leaving, _)) = self.leaving.take() else {
            return;
        };
        self.forgetting.clear();
        self.left_clean = true;
        match leaving {
            Leaving::Close(id) => self.close_window(id, event_loop),
            Leaving::Quit => self.quit(event_loop),
        }
        self.left_clean = false;
    }

    /// Before the last window closes (or Shoji quits): delete what "Clear when Shoji closes" and
    /// "Forget this site" ask for, through a view in each profile that has tabs in `windows`.
    /// Returns true when closing should wait for the engine (the windows are hidden meanwhile).
    pub(super) fn leave_clean(&mut self, windows: &[WindowId], then: Leaving) -> bool {
        if self.left_clean || self.leaving.is_some() {
            return self.leaving.is_some();
        }
        let clear = self.settings.privacy.clear_on_exit;
        let keep_none = self.settings.privacy.persistent_sites.is_empty();
        let settings = Rc::new(self.settings.clone());
        let since = self.started_ms;
        let mut hosts: HashSet<String> = self
            .history
            .items()
            .iter()
            .filter(|e| e.last_visit >= since)
            .filter_map(|e| extract_host(&e.url))
            .collect();
        // One view per profile (the main one and each container) with open tabs.
        let mut profiles: Vec<(Option<String>, TabId, WindowId)> = Vec::new();
        for id in windows {
            let Some(win) = self.windows.get(id) else {
                continue;
            };
            if win.private {
                continue;
            }
            for (tab_id, tab) in &win.views {
                if !tab.persistent {
                    continue;
                }
                if let Some(t) = win.tabs.get_tab_by_id(*tab_id) {
                    if let Some(h) = extract_host(&t.url) {
                        hosts.insert(h);
                    }
                }
                if !profiles.iter().any(|(c, _, _)| *c == tab.container) {
                    profiles.push((tab.container.clone(), *tab_id, *id));
                }
            }
        }
        let forget_open = !self.settings.privacy.forget_on_close.is_empty()
            && hosts.iter().any(|h| self.settings.forgets_on_close(h));
        if !clear && !forget_open && self.forgetting.is_empty() {
            return false;
        }
        let hosts: Vec<String> = hosts.into_iter().collect();
        if clear || forget_open {
            for (_, tab_id, win_id) in &profiles {
                let Some(view) = self
                    .windows
                    .get(win_id)
                    .and_then(|w| w.views.get(tab_id))
                    .map(|t| &t.view)
                else {
                    continue;
                };
                let s = settings.clone();
                let forget = move |site: &str| {
                    s.forgets_on_close(site) || (s.privacy.clear_on_exit && !s.keeps_site(site))
                };
                self.next_forget += 1;
                let job = self.next_forget;
                start_forget(
                    &self.proxy,
                    job,
                    view,
                    forget,
                    hosts.clone(),
                    clear && keep_none,
                );
                self.forgetting.insert(job, (None, Instant::now()));
            }
        }
        if self.forgetting.is_empty() {
            return false;
        }
        for id in windows {
            if let Some(w) = self.windows.get(id) {
                w.window.set_visible(false);
            }
        }
        self.leaving = Some((then, Instant::now() + LEAVE_WAIT));
        true
    }
}
