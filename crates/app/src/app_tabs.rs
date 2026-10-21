//! Tab views: building them (profiles, containers, scripts), opening, closing, moving.

use super::*;

impl BrowserApp {
    /// `navigate` is false for a popup: the page that opened it hands the view its address.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn build_view(
        &self,
        window: &Window,
        tab_id: TabId,
        url: &str,
        rect: Rect,
        navigate: bool,
        container: Option<String>,
    ) -> Option<Tab> {
        let internal = is_internal(url);
        let private = self
            .windows
            .values()
            .any(|w| w.private && w.window.id() == window.id());
        // A container keeps its own cookies (outside private windows), for every page in the tab.
        let persistent = !private
            && if container.is_some() {
                true
            } else {
                !internal && self.settings.is_site_persistent(url)
            };
        let dark = self.is_dark(window);

        let pages = self.pages.clone();
        let nav_pages = self.pages.clone();
        let nav_insecure = self.insecure_hosts.clone();
        let proxy_nav = self.proxy.clone();
        let proxy_ipc = self.proxy.clone();
        let nav_container = container.clone();
        let mut builder = new_builder()
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
                        .header("Content-Security-Policy", INTERNAL_CSP)
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
                let wants_persistent = !private
                    && nav.starts_with("http")
                    && !nav.starts_with(INTERNAL_ORIGIN)
                    && nav_pages
                        .read()
                        .map(|c| c.settings.is_site_persistent(&nav))
                        .unwrap_or(false);
                // A site that always opens in a container goes there from tabs outside containers.
                // A tab the user put in a container keeps whatever it opens.
                let ruled = nav_container.is_none() && !nav.starts_with(INTERNAL_ORIGIN);
                if let Some(host) = extract_host(&nav).filter(|_| ruled) {
                    let rule = nav_pages
                        .read()
                        .ok()
                        .and_then(|c| c.settings.containers.for_host(&host).map(str::to_owned));
                    if rule.is_some() && rule != nav_container {
                        let _ = proxy_nav.send_event(BrowserEvent::OpenInContainer {
                            tab_id,
                            url: nav,
                            container: rule,
                        });
                        return false;
                    }
                }
                if wants_persistent && !persistent && nav_container.is_none() {
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
                let privileged = is_internal_origin(&req.uri().to_string());
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
            if self.settings.privacy.save_passwords
                && !self.settings.privacy.ephemeral_default
                && !private
            {
                builder = builder.with_initialization_script(pages::LOGIN_SCRIPT);
            }
        }
        // Every tab, not just web ones: an internal tab can navigate to a site in place.
        builder = builder.with_initialization_script(HIDE_SCROLLBAR_SCRIPT);
        builder = builder.with_initialization_script(SPLIT_DROP_SCRIPT);
        // Built blank, so document scripts are registered before the first navigation.
        if let Some(c) = &container {
            builder = builder.with_profile_name(container_profile(c));
        }
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
        let font_script = webview::ScriptSlot::default();
        if let Some(script) = pages::font_document_script(&self.settings) {
            webview::set_document_script(&view, &font_script, Some(&script));
        }
        let emulated;
        let request_hook;
        {
            let pages = self.pages.clone();
            let mode_for: shield::ModeFor = Arc::new(move |page: &str| {
                pages
                    .read()
                    .map(|c| shield::parse_mode(c.settings.shield_mode_for(page)))
                    .unwrap_or(shoji_byobu::Mode::Standard)
            });
            let pages_fp = self.pages.clone();
            let fp_for: shield::FpFor = Arc::new(move |page: &str| {
                pages_fp
                    .read()
                    .map(|c| shield::parse_fp(c.settings.fingerprint_mode_for(page)))
                    .unwrap_or(shield::Fp::Standard)
            });
            request_hook = shield::attach(&view, self.shield.clone(), mode_for, fp_for, tab_id);
            let pages = self.pages.clone();
            emulated = darkfixes::attach(
                &view,
                Arc::new(move |url: &str| {
                    pages.read().is_ok_and(|c| {
                        !c.settings.appearance.dark_schedule.enabled
                            && extract_host(url).is_some_and(|h| c.settings.dark_on(&h))
                    })
                }),
            );
        }
        if navigate {
            let _ = view.load_url(&engine_url(url));
        }
        webview::attach_accelerators(&view, Origin::Tab(tab_id), self.proxy.clone());
        webview::attach_tab_events(
            &view,
            tab_id,
            self.proxy.clone(),
            self.allowed_cert_hosts.clone(),
        );
        webview::attach_focus(&view, tab_id, self.proxy.clone());
        webview::attach_nav_failures(&view, tab_id, self.proxy.clone());
        // Bounce tracking is cleaned in the main cookie jar; containers and private windows keep
        // their own (or nothing).
        if persistent && container.is_none() {
            webview::attach_bounce_watch(&view, self.proxy.clone());
        }
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
            container,
            hidden_since: None,
            suspended: false,
            dark_script,
            font_script,
            emulated,
            request_hook,
            tint: None,
            has_video: false,
            born: Instant::now(),
            painted: false,
            pending_url: None,
        })
    }

    /// A new temporary container: it lasts until its last tab closes.
    pub(super) fn new_temp_container(&mut self) -> String {
        format!(
            "{}{}",
            shoji_core::settings::TEMP_CONTAINER,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        )
    }

    /// Reopen a tab (at `url`, or where it is) in `container`. A view can't change its engine
    /// profile, so a new tab takes its place and the old one closes.
    pub(super) fn reopen_in_container(
        &mut self,
        win_id: WindowId,
        tab_id: TabId,
        url: Option<String>,
        container: Option<String>,
    ) {
        let Some(state) = self
            .windows
            .get(&win_id)
            .and_then(|w| w.tabs.get_tab_by_id(tab_id).cloned())
        else {
            return;
        };
        let url = url.unwrap_or(state.url);
        let Some(new_id) = self.create_tab_in(win_id, url, true, container) else {
            return;
        };
        if let Some(win) = self.windows.get_mut(&win_id) {
            let pos = |id: TabId| win.tabs.tabs().iter().position(|t| t.id == id);
            if let (Some(old), Some(new)) = (pos(tab_id), pos(new_id)) {
                let _ = win.tabs.reorder_tab(new, old);
            }
            let _ = win.tabs.close_workspace_tab(tab_id, now_secs());
            win.ui_dirty = true;
        }
        self.sync_window_views(win_id);
    }

    /// Every container a tab can be in right now, for the tab strip's marks.
    pub(super) fn containers_for_ui(&self) -> Vec<shoji_core::settings::Container> {
        let mut all = self.settings.containers.list.clone();
        let temps = self
            .windows
            .values()
            .flat_map(|w| w.tabs.all_tabs().filter_map(|t| t.container.clone()))
            .filter(|c| is_temp_container(c));
        for id in temps {
            if !all.iter().any(|c| c.id == id) {
                all.push(shoji_core::settings::Container {
                    id,
                    name: "Temporary".into(),
                    color: "#8a8a8a".into(),
                });
            }
        }
        all
    }

    pub(super) fn create_tab(&mut self, win_id: WindowId, url: Option<String>) {
        let url = url.unwrap_or_else(|| format!("{INTERNAL_SCHEME}newtab"));
        self.create_tab_at(win_id, url, true);
    }

    /// A tab for a window a page opened. The view is left blank for the page to fill in.
    pub(super) fn open_popup(&mut self, req: webview::PopupRequest) {
        let Some(win_id) = self.owner(req.tab_id) else {
            return req.refuse();
        };
        let uri = req.uri.clone();
        let url = if uri.is_empty() || uri == "about:blank" {
            "about:blank".to_string()
        } else {
            match normalize_url(&uri, self.settings.search_url_template()) {
                Ok(u) if u.starts_with("http") => u,
                _ => return req.refuse(),
            }
        };
        match self.create_tab_at(win_id, url, false) {
            Some(id) => {
                let view = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.views.get(&id))
                    .map(|t| &t.view);
                match view {
                    Some(view) => req.attach(view),
                    None => req.refuse(),
                }
            }
            None => req.refuse(),
        }
    }

    pub(super) fn create_tab_at(
        &mut self,
        win_id: WindowId,
        url: String,
        navigate: bool,
    ) -> Option<TabId> {
        // A site with a container rule opens in that container.
        let container = extract_host(&url)
            .and_then(|h| self.settings.containers.for_host(&h).map(str::to_owned));
        self.create_tab_in(win_id, url, navigate, container)
    }

    pub(super) fn create_tab_in(
        &mut self,
        win_id: WindowId,
        url: String,
        navigate: bool,
        container: Option<String>,
    ) -> Option<TabId> {
        let tab_id = TabId(
            self.tab_ids
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst),
        );
        let win = self.windows.get(&win_id)?;
        let window = win.window.clone();
        let rect = win.content_bounds();
        let tab = self.build_view(&window, tab_id, &url, rect, navigate, container.clone())?;
        let win = self.windows.get_mut(&win_id)?;
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
                notice: false,
                is_muted: false,
                status: TabStatus::Inactive,
                last_active_timestamp_secs: now_secs(),
                group: None,
                pinned: false,
                workspace,
                container,
            },
            None,
        );
        win.views.insert(tab_id, tab);
        win.show_tab(tab_id);
        win.layout();
        let open = win.tabs.tabs().len();
        if open >= 40 && !self.hoarding_told {
            self.hoarding_told = true;
            self.toast(&serde_json::json!({
                "title": format!("{open} tabs open"),
                "text": "Which one is the real one?",
                "pose": "look",
                "ms": 6500,
            }));
        }
        Some(tab_id)
    }

    /// Reconcile `win.views` with `win.tabs`: drop views for removed tabs, build views for any
    /// newly created tabs, hide & suspend tabs in inactive workspaces, and show the active tab(s).
    pub(super) fn sync_window_views(&mut self, win_id: WindowId) {
        let mut closing_temp: Vec<Tab> = Vec::new();
        let (window, rect, to_build, removed, inactive_ids, shown) = {
            let Some(win) = self.windows.get_mut(&win_id) else {
                return;
            };
            let live_ids: HashSet<TabId> = win.tabs.all_tabs().map(|t| t.id).collect();
            let max_id = live_ids.iter().map(|id| id.0).max().unwrap_or(0);
            self.tab_ids
                .fetch_max(max_id + 1, std::sync::atomic::Ordering::SeqCst);
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
                if let Some(tab) = win.views.remove(id) {
                    if tab.container.as_deref().is_some_and(is_temp_container) {
                        closing_temp.push(tab);
                    }
                }
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
                    to_build.push((t.id, url, t.container.clone()));
                }
            }
            let inactive_ids: HashSet<TabId> =
                win.tabs.inactive_tabs().iter().map(|t| t.id).collect();
            let shown: HashSet<TabId> = win
                .tabs
                .active_tab_id()
                .into_iter()
                .chain(win.tabs.split_panes().into_iter().flatten())
                .collect();
            (
                win.window.clone(),
                win.content_bounds(),
                to_build,
                removed,
                inactive_ids,
                shown,
            )
        };
        if !removed.is_empty() {
            if let Ok(mut ctx) = self.pages.write() {
                for id in &removed {
                    ctx.notices.remove(&id.0);
                }
            }
        }
        // Only what is about to show loads now; every other tab loads the first time it is shown,
        // so restoring a long session doesn't fetch every page at once.
        // A temporary container goes with its last tab: its cookies and storage are deleted.
        for tab in closing_temp {
            let c = tab.container.clone().unwrap_or_default();
            let in_use = self.windows.values().any(|w| {
                w.tabs
                    .all_tabs()
                    .any(|t| t.container.as_deref() == Some(c.as_str()))
            });
            if !in_use {
                webview::delete_profile(&tab.view);
            }
        }
        for (id, url, container) in to_build {
            let now = shown.contains(&id);
            if let Some(mut tab) = self.build_view(&window, id, &url, rect, now, container) {
                if !now {
                    tab.pending_url = Some(url.clone());
                }
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
                    if !tab.suspended && tab.pending_url.is_none() {
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

    pub(super) fn close_tab(
        &mut self,
        win_id: WindowId,
        tab_id: TabId,
        event_loop: &ActiveEventLoop,
    ) {
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        // Closing a window's only tab closes the window, as in other browsers.
        let was_last = win.tabs.all_tabs().count() == 1;
        // Remember it for the new tab page's "recently closed" list.
        if let Some(state) = win
            .tabs
            .get_tab_by_id(tab_id)
            .cloned()
            .filter(|_| !win.private)
        {
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
        self.fold_crane();
        if was_last {
            self.close_window(win_id, event_loop);
        }
    }

    pub(super) fn open_internal(&mut self, win_id: WindowId, page: &str) {
        let url = format!("{INTERNAL_SCHEME}{page}");
        if let Some(win) = self.windows.get_mut(&win_id) {
            win.sidebar_open = false;
            win.pop_hide_at = None;
            if let Some(s) = &win.sidebar {
                let _ = s.set_visible(false);
            }
            // A page already open (even scrolled to a section with "#name") is shown, not duplicated.
            let same = |t: &str| {
                t == url
                    || t.strip_prefix(&url)
                        .is_some_and(|r| r.starts_with('#') || r.starts_with("/#"))
            };
            if let Some(existing) = win.tabs.tabs().iter().find(|t| same(&t.url)).map(|t| t.id) {
                win.show_tab(existing);
                win.layout();
                return;
            }
        }
        self.create_tab(win_id, Some(url));
    }

    /// Navigate the active tab, rebuilding it if the cookie partition must change.
    pub(super) fn navigate(&mut self, win_id: WindowId, raw: &str) {
        let target = match normalize_url(raw, self.settings.search_url_template()) {
            Ok(t) => t,
            Err(e) => return eprintln!("Navigation blocked: {e}"),
        };
        for page in [
            "settings",
            "passwords",
            "history",
            "bookmarks",
            "downloads",
            "wish",
            "tea",
            "koan",
        ] {
            if target == format!("{INTERNAL_SCHEME}{page}") {
                return self.open_internal(win_id, page);
            }
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
        let wants_persistent =
            !is_internal(&target) && !win.private && self.settings.is_site_persistent(&target);
        win.tabs.update_url(active, target.clone());
        win.tabs.update_favicon(active, None);
        win.reader.remove(&active);
        win.ui_dirty = true;
        win.sidebar_dirty = true;
        let same_partition = win.views.get(&active).map(|t| t.persistent) == Some(wants_persistent);
        if same_partition || is_internal(&target) {
            if let Some(tab) = win.views.get_mut(&active) {
                tab.pending_url = None;
                let _ = tab.view.load_url(&engine_url(&target));
                if !is_internal(&target) {
                    let _ = unsafe { tab.view.controller().SetZoomFactor(zoom) };
                }
            }
            return;
        }
        let window = win.window.clone();
        let rect = win.rect_for(active);
        let container = win
            .tabs
            .get_tab_by_id(active)
            .and_then(|t| t.container.clone());
        if let Some(tab) = self.build_view(&window, active, &target, rect, true, container) {
            if let Some(win) = self.windows.get_mut(&win_id) {
                win.views.insert(active, tab);
                win.show_tab(active);
                win.layout();
            }
        }
    }

    pub(super) fn reload(&mut self, win_id: WindowId, hard: bool) {
        let Some(win) = self.windows.get(&win_id) else {
            return;
        };
        let Some(state) = win.tabs.active_tab().cloned() else {
            return;
        };
        let partition_ok = win.views.get(&state.id).map(|t| t.persistent)
            == Some(
                !is_internal(&state.url)
                    && !win.private
                    && self.settings.is_site_persistent(&state.url),
            );
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

    /// Move a live tab into another window (or a new one) without reloading it, from a drop at
    /// `screen` (logical px) as reported by the page.
    pub(super) fn detach_tab(
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
        let Some(dst) = self.move_tab(src, tab_id, target, None, event_loop) else {
            return;
        };
        if target.is_none() {
            if let (Some((x, y)), Some(dst_win)) = (screen, self.windows.get(&dst)) {
                dst_win.window.set_outer_position(LogicalPosition::new(
                    (x - 120.0).max(0.0),
                    (y - 16.0).max(0.0),
                ));
            }
        }
    }

    /// A window to put a moved tab in: the ready spare when there is one, else a new one.
    pub(super) fn take_window(&mut self, event_loop: &ActiveEventLoop) -> Option<WindowId> {
        if let Some((id, mut win)) = self.spare.take() {
            {
                use winit::platform::windows::WindowExtWindows;
                win.window.set_skip_taskbar(false);
                win.window
                    .set_outer_position(winit::dpi::PhysicalPosition::new(0, 0));
            }
            let logical = win
                .window
                .inner_size()
                .to_logical::<f64>(win.window.scale_factor());
            win.width = logical.width;
            win.height = logical.height;
            self.windows.insert(id, win);
            self.spare_due = Instant::now() + Duration::from_secs(2);
            return Some(id);
        }
        self.create_window(event_loop, false)
    }

    /// Build the spare window while nothing is going on, so tearing a tab out is instant.
    pub(super) fn warm_spare(&mut self, event_loop: &ActiveEventLoop) {
        if self.spare.is_some() || self.tab_drag.is_some() || Instant::now() < self.spare_due {
            return;
        }
        if !self.settings.tabs.enable_tab_tearoff
            || left_button_down()
            || !self
                .windows
                .values()
                .any(|w| w.tabs.tabs().len() + w.tabs.inactive_tabs().len() > 1)
        {
            // Not wanted right now. Look again shortly instead of leaving `spare_due` in the past,
            // which would make the event loop wake continuously.
            self.spare_due = Instant::now() + Duration::from_secs(2);
            return;
        }
        // Building a window while pages load is several times slower and blocks the UI for all
        // of it; wait for a quiet moment.
        if self
            .windows
            .values()
            .any(|w| w.tabs.tabs().iter().any(|t| t.is_loading))
        {
            self.spare_due = Instant::now() + Duration::from_secs(1);
            return;
        }
        self.spare_building = true;
        let made = self.create_window(event_loop, false);
        self.spare_building = false;
        if let Some(id) = made {
            if let Some(win) = self.windows.remove(&id) {
                win.window.set_visible(false);
                self.spare = Some((id, win));
            }
        }
    }

    /// Move a live tab from `src` into window `dst` at strip position `at` (or into a new window
    /// when `dst` is `None`) without reloading it. Returns the window it ended up in. When `src`
    /// is left with nothing it closes.
    pub(super) fn move_tab(
        &mut self,
        src: WindowId,
        tab_id: TabId,
        dst: Option<WindowId>,
        at: Option<usize>,
        event_loop: &ActiveEventLoop,
    ) -> Option<WindowId> {
        if dst == Some(src) {
            return None;
        }
        let win = self.windows.get_mut(&src)?;
        let (Some(state), Some(tab)) = (win.tabs.extract_tab(tab_id), win.views.remove(&tab_id))
        else {
            return None;
        };
        if win.fullscreen_tab == Some(tab_id) {
            win.fullscreen_tab = None;
        }
        win.reader.remove(&tab_id);
        let src_next = win.tabs.active_tab_id();
        let src_has_other_workspaces = win.tabs.workspaces().len() > 1;
        let dst = dst.or_else(|| self.take_window(event_loop))?;
        let dst_win = self.windows.get_mut(&dst)?;
        if let Ok(handle) =
            winit::raw_window_handle::HasWindowHandle::window_handle(dst_win.window.as_ref())
        {
            if let winit::raw_window_handle::RawWindowHandle::Win32(h) = handle.as_raw() {
                let _ = tab.view.reparent(h.hwnd.get());
            }
        }
        for other in dst_win.views.values_mut() {
            if other.hidden_since.is_none() {
                BrowserWindow::hide(other);
            }
        }
        // Group, split and workspace ids are per window.
        let mut state = state;
        state.group = None;
        state.workspace = dst_win.tabs.active_workspace_id();
        dst_win.tabs.insert_tab(state, at);
        dst_win.views.insert(tab_id, tab);
        dst_win.show_tab(tab_id);
        dst_win.layout();
        dst_win.ui_dirty = true;
        dst_win.window.set_visible(true);
        dst_win.window.focus_window();

        match src_next {
            Some(next) => {
                if let Some(win) = self.windows.get_mut(&src) {
                    win.show_tab(next);
                    win.layout();
                    win.ui_dirty = true;
                }
            }
            None if src_has_other_workspaces => {
                self.create_tab(src, None);
            }
            None => self.close_window(src, event_loop),
        }
        Some(dst)
    }

    /* ----- favourites, sessions, per-site data ----- */

    /// Zoom for a page: its site's remembered zoom, else the default.
    pub(super) fn zoom_for(&self, url: &str) -> f64 {
        extract_host(url)
            .and_then(|h| self.site_data.zoom_for(&h))
            .unwrap_or(self.settings.appearance.default_zoom_level)
    }

    /// Hand `target` to HTTPS-only mode: the https:// address to load instead, or `target` itself.
    pub(super) fn upgrade_target(&mut self, tab: TabId, target: String) -> String {
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
}
