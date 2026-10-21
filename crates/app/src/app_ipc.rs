//! Messages from the browser UI and pages, and keyboard commands.

use super::*;

impl BrowserApp {
    pub(super) fn handle_ipc(
        &mut self,
        win_id: WindowId,
        from_tab: Option<TabId>,
        msg: UiToHostMessage,
        event_loop: &ActiveEventLoop,
    ) {
        use UiToHostMessage as M;
        match msg {
            M::ChromeReady => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.last_ui_json.clear();
                    win.last_page_sync.clear();
                    win.last_fav_sync.clear();
                    win.frame_state = None;
                    win.ui_dirty = true;
                    win.sync_zoom();
                    win.sync_frame();
                    win.focus_address = true;
                    if win.sidebar.is_none() && win.sidebar_warm.is_none() {
                        win.sidebar_warm = Some(Instant::now() + Duration::from_millis(900));
                    }
                }
                self.push_data();
                self.push_update();
                self.push_extension_buttons(win_id);
                if self.settings.first_run_complete {
                    self.greet_after_update();
                    if !self.offer_rollback() {
                        self.mention_crash();
                    }
                }
            }
            M::MinimizeWindow | M::ToggleMaximize | M::StartWindowResize => {
                let Some(win) = self.windows.get(&win_id) else {
                    return;
                };
                let w = &win.window;
                match msg {
                    M::MinimizeWindow => w.set_minimized(true),
                    M::ToggleMaximize => w.set_maximized(!w.is_maximized()),
                    _ => {
                        let _ = w.drag_resize_window(winit::window::ResizeDirection::North);
                    }
                }
            }
            M::ToggleDarkWebsites => self.update_settings(|s| {
                s.appearance.dark_websites = Some(s.appearance.dark_websites != Some(true));
            }),
            M::SetDarkWebsites { on } => {
                self.update_settings(|s| s.appearance.dark_websites = Some(on))
            }
            M::ToggleDarkSite { host } => {
                let host = shoji_core::settings::extract_domain(&host);
                if host.is_empty() {
                    return;
                }
                // Flip what the user sees on this site: dark becomes never-dark, and the other way.
                self.update_settings(|s| {
                    let mode = if s.dark_on(&host) { "never" } else { "always" };
                    set_dark_site_mode(s, &host, mode);
                });
            }
            M::ToggleDarkPanel => self.toggle_sidebar(win_id, "dark"),
            M::OpenPage { page } => {
                if let Some(page) = [
                    "history",
                    "bookmarks",
                    "downloads",
                    "passwords",
                    "byobu",
                    "settings",
                    "wish",
                    "tea",
                    "koan",
                ]
                .into_iter()
                .find(|p| *p == page)
                {
                    self.open_internal(win_id, page);
                }
            }
            M::PrintPage => self.handle_command(win_id, Command::Print, event_loop),
            M::OpenClearData => self.handle_command(win_id, Command::ClearData, event_loop),
            M::HistoryQuery {
                query,
                offset,
                limit,
                seq,
            } => {
                let private = self.settings.privacy.ephemeral_default;
                let (items, total) = self.history.page(&query, offset, limit.clamp(1, 200));
                let items: Vec<serde_json::Value> = items
                    .iter()
                    .map(|h| {
                        serde_json::json!({
                            "url": h.url, "title": h.title, "visits": h.visits, "last": h.last_visit,
                        })
                    })
                    .collect();
                let clear = offset == 0 && std::mem::take(&mut self.clear_pending);
                let data = serde_json::json!({
                    "items": items, "total": total, "private": private, "clear": clear,
                });
                self.eval_all_internal(&format!(
                    "window.__historyPage && window.__historyPage({seq},{data})"
                ));
            }
            M::HistoryRemove { urls } => {
                self.history.remove(&urls);
                self.eval_all_internal("window.__historyChanged && window.__historyChanged()");
            }
            M::DownloadsQuery => self.push_downloads(),
            M::DownloadAction { key, action } => {
                let Some(entry) = self.downloads.get(key).cloned() else {
                    return;
                };
                let path = std::path::PathBuf::from(&entry.path);
                match action.as_str() {
                    "open" if path.is_file() => {
                        let _ = std::process::Command::new("explorer").arg(&path).spawn();
                    }
                    "show" => {
                        if path.is_file() {
                            let _ = std::process::Command::new("explorer")
                                .arg(format!("/select,{}", path.display()))
                                .spawn();
                        } else if let Some(dir) = path.parent().filter(|d| d.is_dir()) {
                            let _ = std::process::Command::new("explorer").arg(dir).spawn();
                        }
                    }
                    "remove" => {
                        self.downloads.remove(key);
                        self.push_downloads();
                    }
                    _ => {}
                }
            }
            M::ClearBrowsingData {
                range,
                history,
                downloads,
                cookies,
                cache,
            } => {
                let now_ms = now_secs() * 1000;
                let span_ms: Option<u64> = match range.as_str() {
                    "hour" => Some(3_600_000),
                    "day" => Some(86_400_000),
                    "week" => Some(7 * 86_400_000),
                    "month" => Some(30 * 86_400_000),
                    _ => None,
                };
                let since_ms = span_ms.map(|s| now_ms.saturating_sub(s));
                let mut forgot = 0;
                if history {
                    forgot = match since_ms {
                        Some(since) => self.history.clear_since(since),
                        None => {
                            let n = self.history.len();
                            self.history.clear();
                            n
                        }
                    };
                }
                if downloads {
                    self.downloads.clear_since(since_ms.unwrap_or(0));
                    self.downloads.flush();
                    self.push_downloads();
                }
                if cookies || cache {
                    let since_secs = since_ms.map(|ms| ms as f64 / 1000.0);
                    if let Some(tab) = self.windows.get(&win_id).and_then(|w| w.active()) {
                        webview::clear_browsing_data(&tab.view, cookies, cache, since_secs);
                    }
                }
                self.eval_all_internal(&format!(
                    "window.__dataCleared && window.__dataCleared({{history:{forgot}}})"
                ));
                self.eval_all_internal("window.__historyChanged && window.__historyChanged()");
            }
            M::UpdateAction { choice } => self.update_choice(win_id, &choice),
            M::FoundSecret { id } => {
                if self.keepsakes.find(&id) {
                    if let Ok(mut ctx) = self.pages.write() {
                        ctx.found = self.keepsakes.found().to_vec();
                    }
                }
            }
            M::TrayRects {
                find,
                permission,
                toast,
                letter,
            } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.tray_rects = [find, permission, toast, letter];
                    win.layout();
                    win.raise_overlays();
                }
            }
            M::SetDarkTheme { host, theme } => {
                let host = host.map(|h| shoji_core::settings::extract_domain(&h));
                self.update_settings(|s| match host {
                    Some(h) if !h.is_empty() => {
                        s.appearance.dark_site_themes.insert(h, theme);
                    }
                    _ => s.appearance.dark_theme = theme,
                });
            }
            M::SetDarkSiteMode { host, mode } => {
                let host = shoji_core::settings::extract_domain(&host);
                if !host.is_empty() {
                    self.update_settings(|s| set_dark_site_mode(s, &host, &mode));
                }
            }
            M::SetBrowserTheme { theme } => {
                self.update_settings(|s| s.appearance.browser_theme = theme);
            }
            M::ResetDarkSite { host } => {
                let host = shoji_core::settings::extract_domain(&host);
                self.update_settings(|s| {
                    s.appearance.dark_site_themes.remove(&host);
                });
            }
            M::SetDarkSchedule { enabled, from, to } => {
                self.update_settings(|s| {
                    s.appearance.dark_schedule =
                        shoji_core::settings::DarkSchedule { enabled, from, to };
                });
            }
            M::ToggleNotes => {
                if self.windows.get(&win_id).is_some_and(|w| w.notes_open) {
                    self.close_notes(win_id);
                } else {
                    self.open_notes(win_id);
                }
            }
            M::CloseNotes => self.close_notes(win_id),
            M::NotesReady => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.notes_ready = true;
                    if let (true, Some(n)) = (win.notes_open, &win.notes) {
                        let _ = n.set_visible(true);
                        webview::bring_to_top(n);
                    }
                    if let (true, None, Some(from)) =
                        (win.notes_open, &win.notes_anim, win.notes_clip)
                    {
                        let sheet = win.notes_sheet();
                        win.animate_notes(from, sheet, NOTES_OPEN_MS, false);
                    }
                }
            }
            M::SetNotesExpanded { expanded } => self.set_notes_expanded(win_id, expanded),
            M::SaveNote { id, created, text } => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or_default();
                if let Err(e) = self.notebook.save(&id, created, &text, now) {
                    eprintln!("Failed to save note: {e}");
                }
            }
            M::DeleteNote { id } => {
                let _ = self.notebook.delete(&id);
            }
            M::CloseWindow => self.close_window(win_id, event_loop),
            M::CreateTab { url } => {
                let url =
                    url.and_then(|u| normalize_url(&u, self.settings.search_url_template()).ok());
                self.create_tab(win_id, url);
            }
            M::OpenNewWindow => self.new_window(event_loop, None),
            M::SwitchTab { id } => {
                let target_ws = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.tabs.get_tab_by_id(id).map(|t| t.workspace));
                if let Some(ws) = target_ws {
                    if self
                        .windows
                        .get(&win_id)
                        .is_some_and(|w| w.tabs.active_workspace_id() != ws)
                    {
                        self.switch_workspace(win_id, ws);
                    }
                }
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.show_tab(id);
                    win.layout();
                }
            }
            M::CloseTab { id } => self.close_tab(win_id, id, event_loop),
            M::Navigate { url } => {
                // From a page: navigate the tab that sent it, not whichever is active.
                if let Some(tab) = from_tab {
                    if let Some(win) = self.windows.get_mut(&win_id) {
                        win.show_tab(tab);
                    }
                }
                let url = if url.is_empty() {
                    format!("{INTERNAL_SCHEME}newtab")
                } else {
                    url
                };
                self.navigate(win_id, &url);
            }
            M::GoBack | M::GoForward | M::Stop | M::OpenDevTools => {
                if let Some(tab) = self.windows.get(&win_id).and_then(|w| w.active()) {
                    if let M::OpenDevTools = msg {
                        return tab.view.open_devtools();
                    }
                    if let Ok(core) = unsafe { tab.view.controller().CoreWebView2() } {
                        let _ = unsafe {
                            match msg {
                                M::GoBack => core.GoBack(),
                                M::GoForward => core.GoForward(),
                                _ => core.Stop(),
                            }
                        };
                    }
                }
            }
            M::Reload => self.reload(win_id, false),
            M::ReloadTab { tab_id } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.show_tab(tab_id);
                }
                self.reload(win_id, false);
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if let Some(t) = win.tabs.tabs_mut().iter_mut().find(|t| t.id == tab_id) {
                        t.status = TabStatus::Active;
                    }
                }
            }
            M::ToggleMenuPanel => self.toggle_sidebar(win_id, "menu"),
            M::ToggleSecurityPanel => self.toggle_sidebar(win_id, "security"),
            M::ToggleShieldPanel => self.toggle_sidebar(win_id, "shield"),
            M::ShieldPickElement => {
                if let Some(win) = self.windows.get(&win_id) {
                    if let (Some(id), Some(tab)) = (win.tabs.active_tab_id(), win.active()) {
                        shield::pick_element(&tab.view, id);
                        webview::focus(&tab.view);
                    }
                }
                self.close_sidebar(win_id, false);
            }
            M::ReportBrokenSite => {
                self.report_broken_site(win_id);
                self.close_sidebar(win_id, false);
            }
            M::ShieldState { refresh, log } => {
                if refresh {
                    shield::hub().refresh(true);
                }
                let tab = from_tab.and_then(|id| self.windows.get(&win_id)?.views.get(&id));
                if let Some(tab) = tab {
                    let json = shield::hub().state_json(log, None);
                    let _ = tab.view.evaluate_script(&format!(
                        "window.__byobuState&&window.__byobuState({json})"
                    ));
                }
            }
            M::ToggleDownloadsSidebar => self.toggle_sidebar(win_id, "downloads"),
            M::CloseSidebar => self.close_sidebar(win_id, true),
            M::SidebarPointer { inside } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.pop_pointer_in = inside;
                }
            }
            M::SidebarBlur => {
                let mode = self.windows.get(&win_id).map(|w| w.sidebar_mode);
                let open = self.windows.get(&win_id).is_some_and(|w| {
                    w.sidebar_open
                        && !w.sidebar_docked
                        && !(w.overlay_open
                            && matches!(w.overlay_mode, "menu" | "savebm")
                            && w.sidebar_mode == "bookmarks")
                        && w.pop_hide_at.is_none()
                        && w.pop_opened
                            .is_none_or(|t| t.elapsed() > Duration::from_millis(300))
                });
                if let (true, Some(mode)) = (open, mode) {
                    self.close_sidebar(win_id, false);
                    if let Some(win) = self.windows.get_mut(&win_id) {
                        win.pop_blurred = Some((Instant::now(), mode));
                    }
                }
            }
            M::SidebarHeight { height } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    let height = height.clamp(40.0, 1200.0).ceil();
                    if (win.pop_height - height).abs() > 0.5 {
                        win.pop_height = height;
                        if win.sidebar_open && !win.sidebar_docked {
                            win.layout();
                        }
                    }
                }
            }
            // Dropdowns always float; there is no pinned side panel any more.
            M::PinSidebar { .. } => {}
            M::SiteCookies | M::ClearSiteCookies => {
                let clear = matches!(msg, M::ClearSiteCookies);
                let Some(tab) = self.windows.get(&win_id).and_then(|w| w.active()) else {
                    return;
                };
                let url = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.tabs.active_tab())
                    .map(|t| t.url.clone())
                    .unwrap_or_default();
                if !(url.starts_with("https://") || url.starts_with("http://"))
                    || url.starts_with("http://shoji.")
                {
                    return;
                }
                if clear {
                    // Stored data (local storage, databases, caches, workers) goes with the cookies.
                    let origin = url.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
                    let params = serde_json::json!({
                        "origin": origin,
                        "storageTypes": "local_storage,indexeddb,cache_storage,service_workers,file_systems,websql",
                    })
                    .to_string();
                    webview::cdp(&tab.view, "Storage.clearDataForOrigin", &params);
                }
                let proxy = self.proxy.clone();
                webview::site_cookies(&tab.view, &url, clear, move |n| {
                    let _ = proxy.send_event(BrowserEvent::SiteCookies(win_id, n, clear));
                });
            }
            M::SplitZones { zones } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.split_zones = zones.iter().map(|z| (z[0] as u32, z[1], z[2])).collect();
                }
            }
            M::ToolbarAnchors { items } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.anchors = items.into_iter().collect();
                    if win.sidebar_open && !win.sidebar_docked {
                        win.layout();
                    }
                }
            }
            M::OpenCertificateDialog { host } => cert::open_certificate_dialog(&host),
            M::ReorderTab {
                from_index,
                to_index,
            } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if self.settings.tabs.enable_tab_reordering
                        && win.tabs.reorder_tab(from_index, to_index)
                    {
                        win.ui_dirty = true;
                    }
                }
            }
            M::TabDragStart {
                tab_id,
                grab_dx,
                grab_dy,
                strip_left,
            } => self.start_tab_drag(win_id, tab_id, (grab_dx, grab_dy, strip_left), event_loop),
            M::TabCenters { centers } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.tab_centers = centers;
                }
            }
            M::ReorderGroup {
                from_index,
                count,
                to_index,
            } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.reorder_block(from_index, count, to_index) {
                        win.ui_dirty = true;
                    }
                }
            }
            M::DetachTabToNewWindow {
                tab_id,
                screen_x,
                screen_y,
            } => {
                if self.settings.tabs.enable_tab_tearoff {
                    self.detach_tab(win_id, tab_id, screen_x.zip(screen_y), event_loop);
                }
            }
            M::ZoomIn | M::ZoomOut | M::ZoomReset => {
                // The zoom is remembered per site; Reset goes back to the default and forgets it.
                let mut remember: Option<(String, Option<f64>)> = None;
                if let Some(win) = self.windows.get(&win_id) {
                    let z = win.zoom();
                    win.set_zoom(match msg {
                        M::ZoomIn => z + 0.1,
                        M::ZoomOut => z - 0.1,
                        _ => self.settings.appearance.default_zoom_level,
                    });
                    if let Some(host) = win.tabs.active_tab().and_then(|t| extract_host(&t.url)) {
                        let reset = matches!(msg, M::ZoomReset);
                        remember = Some((host, (!reset).then(|| win.zoom())));
                    }
                }
                if let Some((host, zoom)) = remember {
                    self.site_data.set_zoom(&host, zoom);
                    self.push_data();
                }
            }
            M::ToggleFindBar => {
                self.close_sidebar(win_id, false);
                if let Some(win) = self.windows.get(&win_id) {
                    win.eval_chrome("window.__toggleFindBar && window.__toggleFindBar()");
                    if let Some(c) = &win.chrome {
                        webview::focus(c);
                    }
                }
            }
            M::OpenFindInPage | M::CloseFindInPage => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.find_open = matches!(msg, M::OpenFindInPage);
                    if !win.find_open {
                        if let Some(tab) = win.active() {
                            let _ = tab.view.evaluate_script("getSelection().removeAllRanges()");
                        }
                    }
                    win.layout();
                }
            }
            M::FindInPage { query, forward } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.find_query = query.clone();
                    win.find(&query, forward);
                }
            }
            M::FindResult { current, total } => {
                if let Some(win) = self.windows.get(&win_id) {
                    win.eval_chrome(&format!(
                        "window.__syncFindResult && window.__syncFindResult({{current:{current},total:{total}}})"
                    ));
                }
            }
            M::CancelDownload { download_id } => webview::cancel_download(download_id),
            M::ChangeDownloadFolder => {
                if let Some(folder) = webview::pick_folder(&self.settings.downloads.default_folder)
                {
                    let js = format!(
                        "window.__setDownloadFolder && window.__setDownloadFolder({})",
                        js_string(&folder.to_string_lossy())
                    );
                    self.update_settings(|s| s.downloads.default_folder = folder);
                    for win in self.windows.values() {
                        win.eval_internal_tabs(&js);
                    }
                }
            }
            M::OpenPermissionPrompt | M::ClosePermissionPrompt => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.permission_open = matches!(msg, M::OpenPermissionPrompt);
                    win.layout();
                }
            }
            M::PermissionResponse {
                permission_id,
                allow,
            } => {
                // The answer is remembered for that site, so it isn't asked again.
                if let Some((host, kind)) = self.perm_pending.remove(&permission_id) {
                    self.site_data.set_permission(&host, kind, allow);
                    self.push_data();
                }
                webview::resolve_permission(permission_id, allow)
            }
            M::OpenSettings => self.open_internal(win_id, "settings"),
            M::SaveSettings { patch } => {
                let bar = self.settings.appearance.bookmarks_bar;
                self.update_settings(|s| {
                    s.apply_patch(&patch);
                });
                if bar != self.settings.appearance.bookmarks_bar {
                    self.set_bookmarks_bar(self.settings.appearance.bookmarks_bar);
                }

                // Pausing or resuming the shield for a site only takes effect on the next load,
                // so reload the page that is showing (never a Shoji page, such as Settings).
                if patch.pointer("/shield/paused_sites").is_some() {
                    let live = self
                        .windows
                        .get(&win_id)
                        .and_then(|w| w.tabs.active_tab())
                        .is_some_and(|t| !is_internal(&t.url));
                    if live {
                        self.reload(win_id, false);
                    }
                }
            }
            M::SetAccent { accent } => {
                let accent = Accent::from_id(&accent).id().to_string();
                self.update_settings(|s| s.appearance.accent = accent);
            }
            M::SetTheme { theme } => self.update_settings(|s| {
                s.apply_patch(&serde_json::json!({ "appearance": { "theme": theme } }));
            }),
            M::FinishWelcome => {
                self.update_settings(|s| s.first_run_complete = true);
                if let Some(tab) = from_tab {
                    if let Some(win) = self.windows.get_mut(&win_id) {
                        win.tabs.update_url(tab, format!("{INTERNAL_SCHEME}newtab"));
                        win.tabs.update_title(tab, "New tab".into());
                        win.ui_dirty = true;
                        if let Some(t) = win.views.get_mut(&tab) {
                            t.pending_url = None;
                            let _ = t.view.load_url(&engine_url("shoji://newtab"));
                        }
                    }
                }
            }
            M::ListImportSources => {
                let json =
                    serde_json::to_string(&import_win::detect()).unwrap_or_else(|_| "[]".into());
                self.eval_welcome_and_settings(
                    win_id,
                    &format!("window.__importSources && window.__importSources({json})"),
                );
            }
            M::PickImportSource { source } => {
                if let Some(src) = shoji_core::import::catalog::find(&source) {
                    let picked = if src.family == shoji_core::import::catalog::Family::HtmlFile {
                        webview::pick_html_file()
                    } else {
                        webview::pick_folder(&import_win::picker_start(src))
                    };
                    if let Some(path) = picked {
                        self.eval_welcome_and_settings(
                            win_id,
                            &format!(
                                "window.__importPicked && window.__importPicked({},{})",
                                js_string(&source),
                                js_string(&path.to_string_lossy())
                            ),
                        );
                    }
                }
            }
            M::StartImport {
                source,
                profile,
                path,
                mut kinds,
            } => {
                let Some(src) = shoji_core::import::catalog::find(&source) else {
                    return;
                };
                // Private mode keeps no history or sign-ins, so there is nothing to put them in.
                if self.settings.privacy.ephemeral_default {
                    kinds.history = false;
                    kinds.cookies = false;
                }
                let chosen = import_win::profiles_for(src, path.as_deref())
                    .into_iter()
                    .nth(profile);
                let Some(chosen) = chosen else {
                    self.eval_welcome_and_settings(
                        win_id,
                        &format!(
                            "window.__importFailed && window.__importFailed({})",
                            js_string(&format!("Couldn't find {} on this PC.", src.name))
                        ),
                    );
                    return;
                };
                let proxy = self.proxy.clone();
                std::thread::spawn(move || {
                    let progress_proxy = proxy.clone();
                    let mut progress = |text: &str| {
                        let _ = progress_proxy.send_event(BrowserEvent::ImportProgress {
                            win: win_id,
                            text: text.to_string(),
                        });
                    };
                    let imported = import_win::read(src, &chosen, kinds, &mut progress);
                    let _ = proxy.send_event(BrowserEvent::ImportDone {
                        win: win_id,
                        imported: Box::new(imported),
                    });
                });
            }
            M::CheckDefaultBrowser => {
                let on = default_browser::is_default();
                self.eval_welcome_and_settings(
                    win_id,
                    &format!("window.__defaultBrowser && window.__defaultBrowser({on})"),
                );
            }
            M::SetDefaultBrowser => default_browser::open_settings(),
            M::SearchHistory {
                query,
                seq,
                overlay,
            } => {
                let items: Vec<serde_json::Value> = if self.settings.privacy.ephemeral_default {
                    Vec::new()
                } else {
                    self.history
                        .search(&query, 5)
                        .iter()
                        .map(|h| serde_json::json!({ "url": h.url, "title": h.title }))
                        .collect()
                };
                let auto = if self.settings.privacy.ephemeral_default {
                    None
                } else {
                    self.history.autocomplete(&query)
                };
                if let Some(win) = self.windows.get(&win_id) {
                    if overlay {
                        if let Some(o) = &win.overlay {
                            let _ = o.evaluate_script(&format!(
                                "window.__paneHistory && window.__paneHistory({seq},{},{})",
                                serde_json::Value::Array(items),
                                serde_json::json!(auto)
                            ));
                        }
                    } else {
                        win.eval_chrome(&format!(
                            "window.__historyResults && window.__historyResults({seq},{},{})",
                            serde_json::Value::Array(items),
                            serde_json::json!(auto)
                        ));
                    }
                }
            }
            M::SuggestQuery {
                query,
                seq,
                overlay,
            } => {
                let Some(template) = pages::suggest_url(&self.settings) else {
                    return;
                };
                let url = template.replace("%s", &shoji_core::tabs::url_encode(&query));
                let proxy = self.proxy.clone();
                let _ = std::thread::Builder::new()
                    .name("shoji-suggest".into())
                    .spawn(move || {
                        let words = fetch_suggestions(&url);
                        let _ = proxy
                            .send_event(BrowserEvent::SuggestWords(win_id, seq, words, overlay));
                    });
            }
            M::ClearHistory => self.history.clear(),
            M::VaultStatus => self.push_vault(),
            M::VaultList { query } => {
                self.vault_open(false);
                self.push_vault_list(&query);
            }
            M::VaultReveal { id } => {
                if self.vault_open(false) {
                    self.vault.touch();
                    if let Some(l) = self.vault.get(&id) {
                        let secret = serde_json::json!({ "password": l.password, "note": l.note });
                        self.eval_all_internal(&format!(
                            "window.__vaultSecret && window.__vaultSecret({}, {secret})",
                            js_string(&id)
                        ));
                    }
                }
            }
            M::VaultCopy { id, field } => {
                if self.vault_open(false) {
                    self.vault.touch();
                    let text = self.vault.get(&id).map(|l| {
                        if field == "username" {
                            l.username.clone()
                        } else {
                            l.password.clone()
                        }
                    });
                    if let Some(text) = text {
                        if vault_win::copy_secret(&text) {
                            self.vault.mark_used(&id, now_ms());
                            self.vault_toast(if field == "username" {
                                "Username copied"
                            } else {
                                "Password copied. It clears from the clipboard in 30 seconds"
                            });
                        }
                    }
                }
            }
            M::VaultSave {
                id,
                url,
                title,
                username,
                password,
                note,
            } => {
                if self.vault_open(true) {
                    let draft = shoji_core::vault::Draft {
                        url,
                        title,
                        username,
                        password,
                        note,
                    };
                    let ok = match id {
                        Some(id) => self.vault.edit(&id, &draft, now_ms()),
                        None => self.vault.upsert(&draft, now_ms()).is_some(),
                    };
                    self.vault_toast(if ok {
                        "Saved"
                    } else {
                        "Couldn't save that login"
                    });
                    self.vault_changed();
                }
            }
            M::VaultDelete { id } => {
                if self.vault_open(false) && self.vault.remove(&id) {
                    self.vault_toast("Deleted");
                    self.vault_changed();
                }
            }
            M::VaultDeleteAll => {
                if self.vault_open(false) {
                    self.vault.delete_all();
                    self.vault_toast("All passwords deleted");
                    self.vault_changed();
                }
            }
            M::VaultUnlock { master } => {
                if self.vault.unlock_master(&master) {
                    self.vault_changed();
                } else {
                    let mut s = self.vault_state_json();
                    s["error"] = "That isn't the master password.".into();
                    self.eval_all_internal(&format!(
                        "window.__vaultState && window.__vaultState({s})"
                    ));
                }
            }
            M::VaultLock => {
                self.vault.lock();
                self.forget_breaches();
                self.push_vault();
            }
            M::VaultSetMaster { master } => {
                if self.vault_open(true) && self.vault.set_master(&master) {
                    self.vault_toast("Master password set");
                } else {
                    self.vault_toast("Couldn't set the master password");
                }
                self.vault_changed();
            }
            M::VaultRemoveMaster => {
                if self.vault.is_unlocked() && self.vault.remove_master(&vault_win::WinProtector) {
                    self.vault_toast("Master password removed");
                }
                self.vault_changed();
            }
            M::VaultGenerate {
                length,
                lower,
                upper,
                digits,
                symbols,
            } => {
                let pw = shoji_core::vault::generate(shoji_core::vault::GenOptions {
                    length,
                    lower,
                    upper,
                    digits,
                    symbols,
                });
                self.eval_all_internal(&format!(
                    "window.__vaultGenerated && window.__vaultGenerated({})",
                    js_string(&pw)
                ));
            }
            M::VaultImportCsv => {
                let Some(path) =
                    webview::pick_file("Choose a passwords CSV file", "CSV files (*.csv)", "*.csv")
                else {
                    return;
                };
                let text = std::fs::read(&path)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                let drafts = shoji_core::vault::drafts_from_csv(&text);
                if drafts.is_empty() {
                    self.vault_toast("No logins found in that file");
                } else if self.vault_open(true) {
                    let (added, updated, skipped) = self.vault.import(&drafts, now_ms());
                    self.eval_all_internal(&format!(
                        "window.__vaultImported && window.__vaultImported({{\"added\":{added},\"updated\":{updated},\"skipped\":{skipped}}})"
                    ));
                    self.vault_changed();
                } else {
                    self.vault_toast("Unlock your passwords first");
                }
            }
            M::VaultExportCsv => {
                if self.vault_open(false) {
                    if let Some(path) = webview::pick_save_csv("shoji-passwords.csv") {
                        let ok = std::fs::write(&path, self.vault.to_csv()).is_ok();
                        self.vault_toast(if ok {
                            "Exported. Keep that file safe"
                        } else {
                            "Couldn't write the file"
                        });
                    }
                }
            }
            M::LoginSubmitted { username, password } => {
                let Some(tab) = from_tab else {
                    return;
                };
                if password.is_empty() || password.len() > 1024 || username.len() > 512 {
                    return;
                }
                let Some(url) = self.login_page(win_id, tab) else {
                    return;
                };
                let host = shoji_core::vault::host_of(&url);
                if self.vault.state() != shoji_core::vault::State::Empty && !self.vault_open(false)
                {
                    return; // locked behind a master password: stay quiet
                }
                if self.vault.is_never(&host) {
                    return;
                }
                let existing = self
                    .vault
                    .logins()
                    .iter()
                    .find(|l| l.host() == host && l.username == username)
                    .map(|l| (l.id.clone(), l.password == password));
                let existing_id = match existing {
                    Some((id, true)) => {
                        self.vault.mark_used(&id, now_ms());
                        return;
                    }
                    Some((id, false)) => Some(id),
                    None => None,
                };
                self.pending_login = Some(PendingLogin {
                    url,
                    username,
                    password,
                    existing: existing_id,
                    at: Instant::now(),
                });
                self.offer_save(win_id);
            }
            M::LoginFieldFocused { x, y, w, h } => {
                let Some(tab) = from_tab else {
                    return;
                };
                let Some(url) = self.login_page(win_id, tab) else {
                    return;
                };
                let host = shoji_core::vault::host_of(&url);
                let Some(win) = self.windows.get(&win_id) else {
                    return;
                };
                // Only the tab being looked at, and not in split view (the page offsets differ).
                if win.tabs.active_tab_id() != Some(tab) || win.split_showing().is_some() {
                    return;
                }
                if self.vault.state() == shoji_core::vault::State::Empty || !self.vault_open(false)
                {
                    return;
                }
                use shoji_core::ipc::OverlayItem;
                let items: Vec<OverlayItem> = self
                    .vault
                    .for_host(&host)
                    .iter()
                    .take(5)
                    .map(|l| OverlayItem {
                        kind: "login".into(),
                        value: l.id.clone(),
                        label: if l.username.is_empty() {
                            "Saved login".into()
                        } else {
                            l.username.clone()
                        },
                        sub: format!("Fill with Shoji · {}", l.host()),
                        color: None,
                    })
                    .collect();
                if items.is_empty() {
                    return;
                }
                let Some(win) = self.windows.get(&win_id) else {
                    return;
                };
                let width = w.clamp(240.0, 380.0);
                let left = x.clamp(4.0, (win.width - width - 4.0).max(4.0));
                let top = win.chrome_height() + y + h + 2.0;
                let frame = [left, top, width, items.len() as f64 * 46.0 + 12.0];
                self.show_overlay(win_id, "suggest", items, frame, "", "", "");
            }
            M::ToggleFavourite => self.bookmark_current(win_id),
            M::RunAction { value } => self.run_action(win_id, &value, event_loop),
            m @ (M::BookmarksQuery
            | M::BookmarkEdit { .. }
            | M::BookmarkMove { .. }
            | M::BookmarkNewFolder { .. }
            | M::BookmarkRenameFolder { .. }
            | M::BookmarkMoveFolder { .. }
            | M::BookmarkDeleteFolder { .. }
            | M::BookmarkRemove { .. }
            | M::BookmarkOpen { .. }
            | M::BookmarkMenu { .. }
            | M::BookmarkFolderMenu { .. }
            | M::BookmarkOverflow { .. }
            | M::BookmarkPage { .. }) => self.bookmark_ipc(win_id, m, event_loop),
            M::ToggleBookmarksBar => {
                let on = !self.settings.appearance.bookmarks_bar;
                self.set_bookmarks_bar(on);
            }
            M::SetSidebarMode { mode } => self.set_sidebar_mode(&mode),
            M::HoverBookmarks { left, right, bar } => {
                // Only a free pointer opens it: not over another dropdown, a menu, or a drag.
                let free = !left_button_down()
                    && self.windows.get(&win_id).is_some_and(|w| {
                        !w.sidebar_open
                            && !w.overlay_open
                            && w.pop_blurred
                                .is_none_or(|(at, _)| at.elapsed() > Duration::from_millis(350))
                    });
                if free {
                    self.open_sidebar(win_id, "bookmarks", Some((left, right, bar)));
                }
            }
            M::PinHover => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.pop_hover = false;
                    win.hover_out_since = None;
                }
            }
            M::MenuDataQuery => {
                let json = self.menu_data(win_id);
                if let Some(win) = self.windows.get(&win_id) {
                    win.eval_sidebar(&format!("window.__menuData && window.__menuData({json})"));
                }
            }
            M::AddFavourite { url, title } => {
                if self.favourites.add(&url, &title) {
                    self.push_data();
                }
            }
            M::FavouriteFolder { url } => self.ask_favourite_folder(win_id, &url),
            M::ExportFavourites => {
                if let Some(path) = webview::pick_save_html("shoji-bookmarks.html") {
                    let ok = std::fs::write(&path, self.favourites.to_netscape_html()).is_ok();
                    self.toast(&serde_json::json!({
                        "title": if ok { "Exported." } else { "Couldn't write the file." },
                        "text": if ok { "Your bookmarks are in a file any browser can import." } else { "Pick another place to save it." },
                        "pose": "unfold",
                    }));
                }
            }
            M::RemoveFavourite { url } => {
                if self.favourites.remove(&url) {
                    self.push_data();
                }
            }
            M::SuggestShow { items, x, width } => {
                let Some(win) = self.windows.get(&win_id) else {
                    return;
                };
                if items.is_empty() {
                    return self.hide_overlay(win_id, false);
                }
                let w = width.clamp(300.0, 760.0).min(win.width - 8.0);
                let x = x.min(win.width - w - 4.0).max(4.0);
                // Rows with a second line (the address) are taller than plain ones.
                let h = items
                    .iter()
                    .take(8)
                    .map(|i| if i.sub.is_empty() { 34.0 } else { 46.0 })
                    .sum::<f64>()
                    + 12.0;
                let top = win.chrome_height();
                self.show_overlay(win_id, "suggest", items, [x, top, w, h], "", "", "");
            }
            M::SuggestHighlight { index } => {
                if let Some(win) = self.windows.get(&win_id) {
                    if let (true, Some(o)) = (win.overlay_open, &win.overlay) {
                        let _ = o.evaluate_script(&format!(
                            "window.__overlayHighlight && window.__overlayHighlight({index})"
                        ));
                    }
                }
            }
            M::SuggestClose => {
                if self
                    .windows
                    .get(&win_id)
                    .is_some_and(|w| w.overlay_mode == "suggest")
                {
                    self.hide_overlay(win_id, false);
                }
            }
            M::OpenPalette => {
                if self
                    .windows
                    .get(&win_id)
                    .is_some_and(|w| w.overlay_open && w.overlay_mode == "palette")
                {
                    return self.hide_overlay(win_id, true);
                }
                let items = self.palette_items(win_id);
                let Some(win) = self.windows.get(&win_id) else {
                    return;
                };
                let w = (win.width - 40.0).clamp(300.0, 620.0);
                let top = win.chrome_height() + 12.0;
                let h = (win.height - top - 24.0).clamp(160.0, 440.0);
                let x = ((win.width - w) / 2.0).max(4.0);
                self.show_overlay(
                    win_id,
                    "palette",
                    items,
                    [x, top, w, h],
                    "",
                    "Jump to a tab, bookmark, setting or action",
                    "",
                );
            }
            M::OverlayReady => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.overlay_ready = true;
                    let mode = win.overlay_mode;
                    if let (Some(js), Some(o)) = (win.overlay_pending.take(), &win.overlay) {
                        let _ = o.evaluate_script(&js);
                        if win.overlay_open {
                            let _ = o.set_visible(true);
                            webview::bring_to_top(o);
                        }
                        if win.overlay_open && mode != "suggest" {
                            webview::focus(o);
                        }
                    }
                }
            }
            M::SidebarReady => {
                let bookmarks = format!(
                    "window.__bookmarks && window.__bookmarks({})",
                    self.bookmarks_json()
                );
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.sidebar_ready = true;
                    win.eval_sidebar(&bookmarks);
                    if win.sidebar_open {
                        win.layout();
                        if let Some(s) = &win.sidebar {
                            let _ = s.set_visible(true);
                            webview::bring_to_top(s);
                            if let Some(js) = win.sidebar_pending.clone() {
                                let _ = s.evaluate_script(&js);
                            }
                        }
                    }
                }
            }
            M::OverlayClose => {
                // Focus moving between surfaces right after the overlay appears isn't a dismissal.
                let fresh = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.overlay_shown)
                    .is_some_and(|t| t.elapsed() < Duration::from_millis(250));
                if !fresh {
                    self.hide_overlay(win_id, true);
                }
            }
            M::OverlayChoose { kind, value } => {
                // Navigation and tab switches focus the page themselves; actions that open a
                // prompt must keep the focus for it.
                let opens_prompt = [
                    "group-new",
                    "group-rename",
                    "split-rename",
                    "fav-folder",
                    "ws-new",
                    "ws-rename",
                ]
                .iter()
                .any(|p| value.starts_with(p));
                self.hide_overlay(win_id, kind == "action" && !opens_prompt);
                match kind.as_str() {
                    "url" => self.navigate(win_id, &value),
                    "search" => {
                        let url = self
                            .settings
                            .search_url_template()
                            .replace("%s", &shoji_core::tabs::url_encode(&value));
                        self.navigate(win_id, &url);
                    }
                    "tab" => {
                        if let Ok(id) = value.parse::<u64>() {
                            let tab_id = TabId(id);
                            let ws = self
                                .windows
                                .get(&win_id)
                                .and_then(|w| w.tabs.get_tab_by_id(tab_id).map(|t| t.workspace));
                            if let Some(ws) = ws {
                                if self
                                    .windows
                                    .get(&win_id)
                                    .is_some_and(|w| w.tabs.active_workspace_id() != ws)
                                {
                                    self.switch_workspace(win_id, ws);
                                }
                            }
                            if let Some(win) = self.windows.get_mut(&win_id) {
                                win.show_tab(tab_id);
                                win.layout();
                            }
                        }
                    }
                    "action" => self.run_action(win_id, &value, event_loop),
                    "login" => self.fill_login(win_id, &value),
                    "login-save" => self.commit_login(&value),
                    "text" => self.finish_prompt(win_id, &value),
                    _ => {}
                }
            }
            M::TabMenu { tab_id, x, y } => {
                let items = self.tab_menu_items(win_id, tab_id);
                let Some(win) = self.windows.get(&win_id) else {
                    return;
                };
                let w = 260.0f64.min(win.width - 8.0);
                let x = x.min(win.width - w - 4.0).max(4.0);
                let h = items.len() as f64 * 32.0 + 10.0;
                self.show_overlay(win_id, "menu", items, [x, y.max(30.0), w, h], "", "", "");
            }
            M::EngineMenu { x, y } => {
                use shoji_core::ipc::OverlayItem;
                let current = self.settings.search_engine.to_lowercase();
                let items: Vec<OverlayItem> = [
                    ("google", "Google"),
                    ("duckduckgo", "DuckDuckGo"),
                    ("bing", "Bing"),
                    ("brave", "Brave"),
                    ("ecosia", "Ecosia"),
                ]
                .iter()
                .map(|(id, name)| OverlayItem {
                    kind: "action".into(),
                    value: format!("engine:{id}"),
                    label: format!("{}{name}", if *id == current { "● " } else { "" }),
                    sub: String::new(),
                    color: None,
                })
                .collect();
                let Some(win) = self.windows.get(&win_id) else {
                    return;
                };
                let w = 220.0f64.min(win.width - 8.0);
                let x = x.min(win.width - w - 4.0).max(4.0);
                let h = items.len() as f64 * 32.0 + 10.0;
                self.show_overlay(win_id, "menu", items, [x, y.max(30.0), w, h], "", "", "");
            }
            M::WorkspaceMenu { workspace_id, x, y } => {
                let items = self.workspace_menu_items(win_id, workspace_id);
                let Some(win) = self.windows.get(&win_id) else {
                    return;
                };
                let w = 240.0f64.min(win.width - 8.0);
                let x = x.min(win.width - w - 4.0).max(4.0);
                let h = items.len() as f64 * 32.0 + 10.0;
                self.show_overlay(win_id, "menu", items, [x, y.max(30.0), w, h], "", "", "");
            }
            M::ToggleReader => self.toggle_reader(win_id),
            M::TogglePictureInPicture => {
                if let Some(tab) = self.windows.get(&win_id).and_then(|w| w.active()) {
                    let params = serde_json::json!({
                        "expression": PIP_SCRIPT,
                        "userGesture": true,
                        "awaitPromise": true,
                    })
                    .to_string();
                    webview::cdp(&tab.view, "Runtime.evaluate", &params);
                }
            }
            M::SplitWith { tab_id } => self.split_with(win_id, tab_id),
            M::SplitCreate { first, second } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.open_split(first, second).is_some() {
                        win.layout();
                        win.ui_dirty = true;
                    }
                }
            }
            M::SplitClose { id } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.close_split(id) {
                        win.layout();
                        win.ui_dirty = true;
                    }
                }
            }
            M::SplitRename { id, name } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.rename_split(id, &name) {
                        win.ui_dirty = true;
                    }
                }
            }
            M::SplitSwapPanes { id } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.swap_split_panes(id) {
                        win.layout();
                        win.ui_dirty = true;
                    }
                }
            }
            M::SplitSetColor { id, color } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.set_split_color(id, &color) {
                        win.ui_dirty = true;
                    }
                }
            }
            // In a split this adds another tab to it; leaving a split is the "x" on its toolbar
            // or "Ungroup" in the tab's menu.
            M::ToggleSplit => {
                // The same combo closes the picker it opened.
                if self
                    .windows
                    .get(&win_id)
                    .is_some_and(|w| w.overlay_open && w.overlay_mode == "paneadd")
                {
                    self.hide_overlay(win_id, true);
                } else {
                    self.open_split_picker(win_id);
                }
            }
            M::SetSplitRatio { ratio } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.set_split_ratio(ratio) {
                        win.layout();
                        win.ui_dirty = true;
                    }
                }
            }
            M::ResetSplitRatio => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.reset_split_ratio() {
                        win.layout();
                        win.ui_dirty = true;
                    }
                }
            }
            M::SetSplitOrientation { orientation } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.set_split_orientation(orientation) {
                        win.layout();
                        win.ui_dirty = true;
                    }
                }
            }
            M::ToggleSplitOrientation => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.toggle_split_orientation().is_some() {
                        win.layout();
                        win.ui_dirty = true;
                    }
                }
            }
            M::SplitFocus { index } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if let Some(id) = win.tabs.split_panes().and_then(|p| p.get(index).copied()) {
                        win.show_tab(id);
                        win.ui_dirty = true;
                    }
                }
            }
            M::SplitAdd {
                split_id,
                tab_id,
                at,
            } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.add_to_split(split_id, tab_id, at) {
                        win.show_tab(tab_id);
                        win.layout();
                        win.ui_dirty = true;
                        win.sidebar_dirty = true;
                    }
                }
            }
            M::SplitSwapPair { id, index } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.swap_split_pair(id, index) {
                        win.layout();
                        win.ui_dirty = true;
                    }
                }
            }
            M::SplitRemove { tab_id } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.remove_from_split(tab_id) {
                        if let Some(active) = win.tabs.active_tab_id() {
                            win.show_tab(active);
                        }
                        win.layout();
                        win.ui_dirty = true;
                        win.sidebar_dirty = true;
                    }
                }
            }
            M::SwitchSplitPane => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if let Some(next) = win.tabs.switch_split_pane(now_secs()) {
                        win.show_tab(next);
                    }
                }
            }
            M::ToggleWorkspacesSidebar => self.toggle_sidebar(win_id, "spaces"),
            M::CreateWorkspace { name, color } => {
                let ws_id = if let Some(win) = self.windows.get_mut(&win_id) {
                    let color = if color.trim().is_empty() {
                        next_workspace_color(win).to_string()
                    } else {
                        color
                    };
                    let name = if name.trim().is_empty() {
                        format!("Workspace {}", win.tabs.workspaces().len() + 1)
                    } else {
                        name
                    };
                    Some(win.tabs.create_workspace(&name, &color))
                } else {
                    None
                };
                if let Some(ws_id) = ws_id {
                    self.switch_workspace(win_id, ws_id);
                }
            }
            M::SwitchWorkspace { id } => {
                self.switch_workspace(win_id, id);
            }
            M::RenameWorkspace { id, name } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.tabs.rename_workspace(id, &name);
                    win.ui_dirty = true;
                    win.sidebar_dirty = true;
                }
            }
            M::SetWorkspaceColor { id, color } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.tabs.set_workspace_color(id, &color);
                    win.ui_dirty = true;
                    win.sidebar_dirty = true;
                }
            }
            M::ReorderWorkspace {
                from_index,
                to_index,
            } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    if win.tabs.reorder_workspace(from_index, to_index) {
                        win.ui_dirty = true;
                        win.sidebar_dirty = true;
                    }
                }
            }
            M::DeleteWorkspace { id } => {
                self.delete_workspace(win_id, id);
            }
            M::MoveTabToWorkspace {
                tab_id,
                workspace_id,
            } => {
                self.move_tab_to_workspace(win_id, tab_id, workspace_id);
            }
            M::ToggleMuteTab { tab_id } => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    let next = win
                        .tabs
                        .get_tab_by_id(tab_id)
                        .map(|t| !t.is_muted)
                        .unwrap_or(false);
                    if let Some(tab) = win.views.get(&tab_id) {
                        webview::set_muted(&tab.view, next);
                    }
                    win.tabs.set_tab_muted(tab_id, next);
                    win.ui_dirty = true;
                    win.sidebar_dirty = true;
                }
            }
            M::LinkDropped { url } => {
                // Any page can send this, so it only ever opens a web address.
                let Ok(url) = normalize_url(&url, self.settings.search_url_template()) else {
                    return;
                };
                if !(url.starts_with("https://") || url.starts_with("http://"))
                    || is_internal_origin(&url)
                {
                    return;
                }
                let in_split = self
                    .windows
                    .get(&win_id)
                    .is_some_and(|w| w.tabs.visible_split().is_some());
                if in_split {
                    if let Some(target_pane) = from_tab {
                        if let Some(win) = self.windows.get_mut(&win_id) {
                            win.show_tab(target_pane);
                        }
                        self.navigate(win_id, &url);
                    }
                } else {
                    self.create_tab(win_id, Some(url));
                }
            }
            M::RestoreSession => self.restore_session(win_id, event_loop),
            M::DismissSession => {
                self.pending_session = None;
                if let Ok(mut ctx) = self.pages.write() {
                    ctx.restore_tabs = 0;
                }
                self.push_data();
            }
            M::SiteChoicesQuery => {
                let json = serde_json::to_string(&self.site_data.entries()).unwrap_or_default();
                self.eval_welcome_and_settings(
                    win_id,
                    &format!("window.__siteChoices && window.__siteChoices({json})"),
                );
            }
            M::ForgetSiteChoice { host, kind } => {
                if self.site_data.forget(&host, kind.as_deref()) {
                    self.push_data();
                }
            }
            M::ExtensionsQuery => self.request_extension_list(),
            M::AddExtension => self.add_extension_from_folder(),
            M::ToggleExtensionPopup { id, right } => {
                self.toggle_extension_popup(win_id, &id, right)
            }
            M::InstallExtension { source } => self.install_extension_from_store(&source),
            M::RemoveExtension { id } => {
                if let Some(view) = self.persistent_view() {
                    webview::remove_extension(view, id, self.proxy.clone());
                }
            }
            M::ClearSiteData => {
                self.site_data.clear();
                self.push_data();
            }
            M::ClearSavedLogins => {
                if let Some(tab) = self.windows.get(&win_id).and_then(|w| w.active()) {
                    webview::clear_saved_logins(&tab.view);
                }
            }
            M::UninstallShoji { confirm } => {
                if confirm.trim() == shoji_core::ipc::UNINSTALL_CONFIRMATION
                    && start_uninstall(&self.data_dir)
                {
                    // Exit at once, without the usual save-on-close, so nothing is written back.
                    std::process::exit(0);
                }
            }
            M::PageInfo { color, has_video } => {
                if let Some(tab_id) = from_tab {
                    let tint = color.as_deref().and_then(parse_css_color);
                    if let Some(tab) = self
                        .windows
                        .get_mut(&win_id)
                        .and_then(|w| w.views.get_mut(&tab_id))
                    {
                        tab.tint = tint;
                        tab.has_video = has_video;
                    }
                }
            }
            M::ContinueInsecure { tab_id, host, url } => {
                if url.starts_with("http://")
                    && extract_host(&url).as_deref() == Some(host.as_str())
                {
                    if let Ok(mut set) = self.insecure_hosts.lock() {
                        set.insert(host);
                    }
                    if let Some(win) = self.windows.get_mut(&win_id) {
                        win.tabs.update_url(tab_id, url.clone());
                        win.ui_dirty = true;
                        if let Some(tab) = win.views.get(&tab_id) {
                            let _ = tab.view.load_url(&url);
                        }
                    }
                }
            }
            M::ContinueBlockedPage { tab_id, host, url } => {
                if (url.starts_with("http://") || url.starts_with("https://"))
                    && extract_host(&url).as_deref() == Some(host.as_str())
                {
                    self.shield.allow_page_host(&host);
                    if let Some(win) = self.windows.get_mut(&win_id) {
                        win.tabs.update_url(tab_id, url.clone());
                        win.ui_dirty = true;
                        if let Some(tab) = win.views.get(&tab_id) {
                            let _ = tab.view.load_url(&url);
                        }
                    }
                }
            }
            M::DownloadAnyway { id } => {
                let Some((tab_id, url)) = shield::hub().take_blocked_download(id) else {
                    return;
                };
                let Some(host) = extract_host(&url) else {
                    return;
                };
                self.shield.allow_page_host(&host);
                let view = self
                    .owner(tab_id)
                    .and_then(|w| self.windows.get(&w))
                    .and_then(|w| w.views.get(&tab_id));
                if let Some(tab) = view {
                    let _ = tab.view.load_url(&url);
                }
            }
            M::BypassCertificateError { tab_id, host, url } => {
                if let Ok(mut set) = self.allowed_cert_hosts.lock() {
                    set.insert(host);
                }
                if let Some(tab) = self.windows.get(&win_id).and_then(|w| w.views.get(&tab_id)) {
                    let _ = tab.view.load_url(&url);
                }
            }
        }
    }

    pub(super) fn handle_command(
        &mut self,
        win_id: WindowId,
        cmd: Command,
        event_loop: &ActiveEventLoop,
    ) {
        self.sync_pending = true;
        self.session_dirty = true;
        use UiToHostMessage as M;
        let ipc = |app: &mut Self, msg| app.handle_ipc(win_id, None, msg, event_loop);
        match cmd {
            Command::NewTab => self.create_tab(win_id, None),
            Command::ReopenClosedTab => {
                let url = self
                    .windows
                    .get_mut(&win_id)
                    .and_then(|w| w.tabs.pop_last_closed());
                if let Some(u) = &url {
                    if let Ok(mut ctx) = self.pages.write() {
                        ctx.recent.retain(|c| &c.url != u);
                    }
                }
                if url.is_some() {
                    self.create_tab(win_id, url);
                    self.push_data();
                }
            }
            Command::NewWindow => self.new_window(event_loop, None),
            Command::NewPrivateWindow => {
                self.open_window(event_loop, None, true);
            }
            Command::BookmarkManager => self.open_internal(win_id, "bookmarks"),
            Command::BookmarksBar => {
                let on = !self.settings.appearance.bookmarks_bar;
                self.set_bookmarks_bar(on);
            }
            Command::BookmarkAllTabs => {
                self.run_action(win_id, "bookmark-all", event_loop);
            }
            Command::CloseTab => {
                if let Some(id) = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.tabs.active_tab_id())
                {
                    // Ctrl+W leaves a pinned tab alone; its menu and middle-click still close it.
                    let pinned = self
                        .windows
                        .get(&win_id)
                        .is_some_and(|w| w.tabs.get_tab_by_id(id).is_some_and(|t| t.pinned));
                    if !pinned {
                        self.close_tab(win_id, id, event_loop);
                    }
                }
            }
            Command::FocusAddress => {
                if let Some(win) = self.windows.get(&win_id) {
                    if let Some(c) = &win.chrome {
                        webview::focus(c);
                    }
                    win.eval_chrome("window.__focusOmnibox && window.__focusOmnibox()");
                }
            }
            Command::Find => ipc(self, M::ToggleFindBar),
            Command::Downloads => ipc(self, M::ToggleDownloadsSidebar),
            Command::Print => {
                if let Some(tab) = self.windows.get(&win_id).and_then(|w| w.active()) {
                    let _ = tab.view.print();
                }
            }
            Command::Reload | Command::HardReload => {
                // Five reloads in a few seconds earns the crane's sympathy.
                let now = Instant::now();
                self.reloads
                    .retain(|t| now.duration_since(*t) < Duration::from_secs(6));
                self.reloads.push(now);
                if self.reloads.len() >= 5 {
                    self.reloads.clear();
                    self.toast(&serde_json::json!({
                        "title": "It's not you.",
                        "text": "Probably them. Maybe try again in a minute?",
                        "pose": "tilt",
                    }));
                }
                self.reload(win_id, cmd == Command::HardReload);
            }
            Command::Back => ipc(self, M::GoBack),
            Command::Forward => ipc(self, M::GoForward),
            Command::NextTab | Command::PrevTab | Command::SelectTab(_) => {
                let Some(win) = self.windows.get_mut(&win_id) else {
                    return;
                };
                let tabs = win.tabs.tabs();
                if tabs.is_empty() {
                    return;
                }
                let current = win
                    .tabs
                    .active_tab_id()
                    .and_then(|a| tabs.iter().position(|t| t.id == a))
                    .unwrap_or(0);
                let n = tabs.len();
                let index = match cmd {
                    Command::NextTab => (current + 1) % n,
                    Command::PrevTab => (current + n - 1) % n,
                    Command::SelectTab(i) => i.min(n - 1),
                    _ => current,
                };
                let id = tabs[index].id;
                win.show_tab(id);
                win.layout();
            }
            Command::ZoomIn => ipc(self, M::ZoomIn),
            Command::ZoomOut => ipc(self, M::ZoomOut),
            Command::ZoomReset => ipc(self, M::ZoomReset),
            Command::DevTools => ipc(self, M::OpenDevTools),
            Command::Palette => ipc(self, M::OpenPalette),
            Command::ToggleFavourite => ipc(self, M::ToggleFavourite),
            Command::Reader => ipc(self, M::ToggleReader),
            Command::History => self.open_internal(win_id, "history"),
            Command::ClearData => {
                let open = self
                    .windows
                    .get(&win_id)
                    .is_some_and(|w| w.tabs.tabs().iter().any(|t| t.url == "shoji://history"));
                if open {
                    self.open_internal(win_id, "history");
                    self.eval_all_internal("window.__showClear && window.__showClear()");
                } else {
                    self.clear_pending = true;
                    self.open_internal(win_id, "history");
                }
            }
            Command::TabSearch => ipc(self, M::OpenPalette),
            Command::Settings => self.open_internal(win_id, "settings"),
            Command::NextTabPage => self.handle_command(win_id, Command::NextTab, event_loop),
            Command::PrevTabPage => self.handle_command(win_id, Command::PrevTab, event_loop),
            Command::FocusAddressF6 => {
                self.handle_command(win_id, Command::FocusAddress, event_loop)
            }
            Command::Home => self.navigate(win_id, "shoji://newtab"),
            Command::ViewSource => {
                let url = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.tabs.active_tab())
                    .map(|t| t.url.clone())
                    .unwrap_or_default();
                if url.starts_with("http://") || url.starts_with("https://") {
                    self.create_tab(win_id, Some(format!("view-source:{url}")));
                }
            }
            Command::SavePage => {
                if let Some(tab) = self.windows.get(&win_id).and_then(|w| w.active()) {
                    webview::save_page(&tab.view);
                }
            }
            Command::FullScreen => {
                if let Some(win) = self.windows.get_mut(&win_id) {
                    win.fullscreen = !win.fullscreen;
                    let mode = win
                        .fullscreen
                        .then_some(winit::window::Fullscreen::Borderless(None));
                    win.window.set_fullscreen(mode);
                    if let Some(c) = &win.chrome {
                        let _ = c.set_visible(!win.fullscreen);
                    }
                    win.layout();
                }
            }
            Command::FindNext | Command::FindPrev => {
                let forward = cmd == Command::FindNext;
                let open = self.windows.get(&win_id).is_some_and(|w| w.find_open);
                if open {
                    let query = self
                        .windows
                        .get(&win_id)
                        .map(|w| w.find_query.clone())
                        .unwrap_or_default();
                    if let Some(win) = self.windows.get(&win_id) {
                        win.find(&query, forward);
                    }
                } else {
                    ipc(self, M::ToggleFindBar);
                }
            }
            Command::ToggleSplit => ipc(self, M::ToggleSplit),
            Command::ToggleSplitOrientation => ipc(self, M::ToggleSplitOrientation),
            Command::SwitchSplitPane => ipc(self, M::SwitchSplitPane),
            Command::Workspaces => ipc(self, M::ToggleWorkspacesSidebar),
            Command::NewWorkspace => {
                self.open_prompt(win_id, Prompt::NewWorkspace, "Name this workspace", "");
            }
            // Inside a split, Alt+1..9 picks the pane (Alt+9 the last one); elsewhere a space.
            Command::SelectWorkspace(i)
                if self
                    .windows
                    .get(&win_id)
                    .is_some_and(|w| w.tabs.visible_split().is_some()) =>
            {
                let last = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.tabs.visible_split().map(|s| s.panes.len() - 1))
                    .unwrap_or(0);
                ipc(self, M::SplitFocus { index: i.min(last) });
            }
            Command::SelectWorkspace(i) => {
                let ws_id = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.tabs.workspaces().get(i).map(|ws| ws.id));
                if let Some(ws_id) = ws_id {
                    self.switch_workspace(win_id, ws_id);
                }
            }
        }
    }
}
