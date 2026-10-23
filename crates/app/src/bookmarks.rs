//! Bookmarks (kept in the favourites store), the bookmarks bar and side panel, private windows
//! and the menu's own actions. The pages draw; this is where their requests land.

use super::*;
use shoji_core::ipc::OverlayItem;
use shoji_core::userdata::{clean_folder, folder_leaf, folder_parent};

fn item(kind: &str, value: String, label: &str, sub: &str, color: Option<String>) -> OverlayItem {
    OverlayItem {
        kind: kind.to_string(),
        value,
        label: label.to_string(),
        sub: sub.to_string(),
        color,
        ..Default::default()
    }
}

fn join_folder(parent: &str, name: &str) -> String {
    clean_folder(&format!("{parent} / {name}"))
}

/// Today as `YYYY-MM-DD` (UTC), for the folder "Bookmark all tabs" makes.
fn today() -> String {
    let days = (now_secs() / 86_400) as i64;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

impl BrowserApp {
    /// Everything the bookmark surfaces draw.
    pub(super) fn bookmarks_json(&self) -> serde_json::Value {
        serde_json::json!({
            "items": self.favourites.items(),
            "folders": self.favourites.all_folders(),
        })
    }

    /// Where a surface's top-left corner sits in the window, so a menu asked for in its own
    /// coordinates opens under the pointer.
    fn surface_origin(&self, win_id: WindowId, from: &str) -> (f64, f64) {
        let Some(win) = self.windows.get(&win_id) else {
            return (0.0, 0.0);
        };
        match from {
            "panel" => {
                let f = win.popover_frame();
                (f[0], f[1])
            }
            "page" => (0.0, win.chrome_height()),
            _ => (0.0, 0.0),
        }
    }

    fn show_menu_at(&mut self, win_id: WindowId, items: Vec<OverlayItem>, x: f64, y: f64, w: f64) {
        let Some(win) = self.windows.get(&win_id) else {
            return;
        };
        if items.is_empty() {
            return;
        }
        let w = w.min(win.width - 8.0);
        let x = x.min(win.width - w - 4.0).max(4.0);
        let y = y.max(30.0);
        let h = (items.len() as f64 * 30.0 + 10.0).min((win.height - y - 8.0).max(80.0));
        if let Some(win) = self.windows.get_mut(&win_id) {
            win.menu_at = (x, y, w);
        }
        self.show_overlay(win_id, "menu", items, [x, y, w, h], "", "", "");
    }

    /// The right-click menu of a bookmark, a folder, or (neither) the bar itself.
    fn bookmark_context_menu(
        &mut self,
        win_id: WindowId,
        url: Option<String>,
        folder: Option<String>,
        x: f64,
        y: f64,
    ) {
        let mut out = Vec::new();
        let act = |v: String, l: &str| item("action", v, l, "", None);
        if let Some(url) = url {
            if !self.favourites.contains(&url) {
                return;
            }
            out.push(act(format!("bm:open:{url}"), "Open in new tab"));
            out.push(act(format!("bm:open-window:{url}"), "Open in new window"));
            out.push(act(
                format!("bm:open-private:{url}"),
                "Open in new private window",
            ));
            out.push(act(format!("bm:rename:{url}"), "Rename…"));
            out.push(act(format!("bm:folder-of:{url}"), "Move to folder…"));
            out.push(act(format!("bm:copy:{url}"), "Copy link"));
            out.push(act("bm:manager".into(), "Show in bookmark manager"));
            out.push(act(format!("bm:remove:{url}"), "Delete"));
        } else if let Some(folder) = folder {
            let n = self.favourites.urls_in(&folder, true).len();
            if n > 0 {
                out.push(act(
                    format!("bm:open-all:{folder}"),
                    &format!("Open all ({n}) in new tabs"),
                ));
                out.push(act(
                    format!("bm:open-all-window:{folder}"),
                    "Open all in a new window",
                ));
                out.push(act(
                    format!("bm:open-all-group:{folder}"),
                    "Open all in a new tab group",
                ));
            }
            out.push(act(format!("bm:rename-folder:{folder}"), "Rename…"));
            out.push(act(format!("bm:new-folder:{folder}"), "New folder inside…"));
            out.push(act(
                format!("bm:delete-folder-keep:{folder}"),
                "Delete folder, keep its bookmarks",
            ));
            out.push(act(
                format!("bm:delete-folder:{folder}"),
                "Delete folder and its bookmarks",
            ));
        } else {
            let bar = self.settings.appearance.bookmarks_bar;
            out.push(act("bookmark-page".into(), "Bookmark this page"));
            out.push(act("bm:new-folder:".into(), "New folder…"));
            out.push(act(
                "bookmarks-bar".into(),
                if bar {
                    "Hide bookmarks bar"
                } else {
                    "Show bookmarks bar"
                },
            ));
            out.push(act("bm:manager".into(), "Bookmark manager"));
        }
        self.show_menu_at(win_id, out, x, y, 270.0);
    }

    /// The dropdown of a bar folder: its bookmarks, and the folders inside it.
    fn bookmark_folder_menu(&mut self, win_id: WindowId, folder: &str, x: f64, y: f64) {
        let mut out = Vec::new();
        let all = self.favourites.all_folders();
        for child in all.iter().filter(|c| folder_parent(c) == folder) {
            out.push(item(
                "action",
                format!("bm:folder:{child}"),
                &format!("{}  ›", folder_leaf(child)),
                "",
                Some("var(--accent)".into()),
            ));
        }
        for f in self
            .favourites
            .items()
            .iter()
            .filter(|f| f.folder == folder)
        {
            out.push(item("url", f.url.clone(), &f.title, "", None));
        }
        if out.is_empty() {
            out.push(item("action", "bm:none".into(), "(empty)", "", None));
        } else if !self.favourites.urls_in(folder, true).is_empty() {
            let n = self.favourites.urls_in(folder, true).len();
            out.push(item(
                "action",
                format!("bm:open-all:{folder}"),
                &format!("Open all ({n})"),
                "",
                None,
            ));
        }
        self.show_menu_at(win_id, out, x, y, 300.0);
    }

    /// What the overflow button of the bar lists: the items that did not fit.
    fn bookmark_overflow_menu(&mut self, win_id: WindowId, skip: usize, x: f64, y: f64) {
        let mut out = Vec::new();
        let all = self.favourites.all_folders();
        for child in all.iter().filter(|c| folder_parent(c).is_empty()) {
            out.push(item(
                "action",
                format!("bm:folder:{child}"),
                &format!("{}  ›", folder_leaf(child)),
                "",
                Some("var(--accent)".into()),
            ));
        }
        for f in self
            .favourites
            .items()
            .iter()
            .filter(|f| f.folder.is_empty())
        {
            out.push(item("url", f.url.clone(), &f.title, "", None));
        }
        let rest: Vec<OverlayItem> = out.into_iter().skip(skip).take(300).collect();
        self.show_menu_at(win_id, rest, x, y, 300.0);
    }

    /// Open bookmarks: in tabs of this window, a new window, a private window, or one tab group.
    fn open_bookmarks(
        &mut self,
        win_id: WindowId,
        urls: Vec<String>,
        how: &str,
        group: &str,
        event_loop: &ActiveEventLoop,
    ) {
        let urls: Vec<String> = urls
            .into_iter()
            .filter(|u| u.starts_with("https://") || u.starts_with("http://"))
            .take(100)
            .collect();
        let Some(first) = urls.first().cloned() else {
            return;
        };
        match how {
            "here" => self.navigate(win_id, &first),
            "window" | "private" => {
                if let Some(id) = self.open_window(event_loop, Some(first), how == "private") {
                    for u in urls.into_iter().skip(1) {
                        self.create_tab(id, Some(u));
                    }
                    if let Some(win) = self.windows.get_mut(&id) {
                        if let Some(first_tab) = win.tabs.tabs().first().map(|t| t.id) {
                            win.show_tab(first_tab);
                            win.layout();
                            win.ui_dirty = true;
                        }
                    }
                }
            }
            _ => {
                let mut made = Vec::new();
                for u in urls {
                    if let Some(id) = self.create_tab_at(win_id, u, true) {
                        made.push(id);
                    }
                }
                let Some(win) = self.windows.get_mut(&win_id) else {
                    return;
                };
                if how == "group" && !made.is_empty() {
                    let color = next_group_color(win);
                    let name = if group.is_empty() { "Bookmarks" } else { group };
                    let gid = win.tabs.create_group(name, color);
                    for id in &made {
                        win.tabs.set_tab_group(*id, Some(gid));
                        gather_group(win, *id, gid);
                    }
                }
                if let Some(id) = made.first() {
                    win.show_tab(*id);
                    win.layout();
                }
                win.ui_dirty = true;
            }
        }
    }

    /// Bookmark the open page, then ask where it should live.
    pub(super) fn bookmark_current(&mut self, win_id: WindowId) {
        let Some(t) = self
            .windows
            .get(&win_id)
            .and_then(|w| w.tabs.active_tab().cloned())
        else {
            return;
        };
        self.bookmark_page(win_id, &t.url, &t.title);
    }

    /// Keep a page (it is saved at once, so nothing is lost if the question is ignored) and ask
    /// which folder it belongs in. An already-kept page is asked about again, to move or remove it.
    pub(super) fn bookmark_page(&mut self, win_id: WindowId, url: &str, title: &str) {
        if !self.favourites.contains(url) {
            if !self.favourites.add(url, title) {
                let web = url.starts_with("http://") || url.starts_with("https://");
                self.toast(&serde_json::json!({
                    "title": if web { "Bookmarks are full" } else { "Can't bookmark this page" },
                    "text": if web { "Delete some to make room." } else { "Only web pages can be bookmarked." },
                    "pose": "look",
                    "ms": 3500,
                }));
                return;
            }
            self.push_data();
        }
        self.ask_favourite_folder(win_id, url);
    }

    /// "Save to": the card under the star. It lists the folders that exist, one click files the
    /// page, and it can start a new folder or take the bookmark away again.
    pub(super) fn ask_favourite_folder(&mut self, win_id: WindowId, url: &str) {
        let Some(current) = self
            .favourites
            .items()
            .iter()
            .find(|f| shoji_core::userdata::same_page(&f.url, url))
            .map(|f| f.folder.clone())
        else {
            return;
        };
        let row = |value: String, label: &str, sub: &str, here: bool| {
            item(
                "action",
                value,
                label,
                sub,
                here.then(|| "current".to_string()),
            )
        };
        let mut items = vec![row("bm:put:".into(), "Top level", "", current.is_empty())];
        for f in self.favourites.all_folders() {
            items.push(row(
                format!("bm:put:{f}"),
                folder_leaf(&f),
                folder_parent(&f),
                f == current,
            ));
        }
        items.push(item(
            "action",
            "bm:put-new:".into(),
            "New folder…",
            "",
            None,
        ));
        items.push(item(
            "action",
            "bm:unsave:".into(),
            "Remove bookmark",
            "",
            None,
        ));
        let Some(win) = self.windows.get_mut(&win_id) else {
            return;
        };
        win.save_url = Some(url.to_string());
        let w = 280.0f64.min(win.width - 8.0);
        let cx = win.anchors.get("fav").copied().unwrap_or(win.width - 160.0);
        let x = (cx - w / 2.0).clamp(4.0, (win.width - w - 4.0).max(4.0));
        let top = CHROME_HEIGHT + 2.0;
        let h = (items.len() as f64 * 30.0 + 60.0)
            .min(400.0)
            .min((win.height - top - 12.0).max(120.0));
        self.show_overlay(win_id, "savebm", items, [x, top, w, h], "Save to", "", "");
    }

    fn bookmark_all_tabs(&mut self, win_id: WindowId) {
        let urls: Vec<(String, String)> = self
            .windows
            .get(&win_id)
            .map(|w| {
                w.tabs
                    .tabs()
                    .iter()
                    .filter(|t| t.url.starts_with("http"))
                    .map(|t| (t.url.clone(), t.title.clone()))
                    .collect()
            })
            .unwrap_or_default();
        // A page that is already bookmarked stays where it is.
        let urls: Vec<(String, String)> = urls
            .into_iter()
            .filter(|(u, _)| !self.favourites.contains(u))
            .collect();
        if urls.is_empty() {
            self.toast(&serde_json::json!({
                "title": "Nothing new to bookmark",
                "text": "The open tabs are already bookmarks, or are Shoji pages.",
                "pose": "look",
                "ms": 3500,
            }));
            return;
        }
        let folder = self
            .favourites
            .add_folder(&format!("Open tabs {}", today()))
            .unwrap_or_default();
        let mut n = 0;
        for (url, title) in &urls {
            if self.favourites.add_in(url, title, &folder) {
                n += 1;
            }
        }
        self.push_data();
        self.toast(&serde_json::json!({
            "title": format!("{n} tab{} bookmarked", if n == 1 { "" } else { "s" }),
            "text": format!("In the folder \u{201c}{folder}\u{201d}"),
            "pose": "cheer",
            "ms": 5000,
            "actions": [{"label": "Show", "action": "RunAction", "payload": {"value": "bookmark-manager"}, "primary": true}],
        }));
    }

    pub(super) fn set_bookmarks_bar(&mut self, on: bool) {
        self.update_settings(|s| s.appearance.bookmarks_bar = on);
        for win in self.windows.values_mut() {
            if win.bar != on {
                win.bar = on;
                win.layout();
            }
        }
    }

    pub(super) fn set_sidebar_mode(&mut self, mode: &str) {
        if !matches!(mode, "off" | "on") {
            return;
        }
        let mode = mode.to_string();
        self.update_settings(|s| s.appearance.sidebar = mode.clone());
    }

    /// The bookmark requests of the pages.
    pub(super) fn bookmark_ipc(
        &mut self,
        win_id: WindowId,
        msg: UiToHostMessage,
        event_loop: &ActiveEventLoop,
    ) {
        use UiToHostMessage as M;
        let changed = match msg {
            M::BookmarksQuery => true,
            M::BookmarkEdit {
                url,
                title,
                new_url,
            } => self.favourites.edit(&url, &title, &new_url),
            M::BookmarkMove {
                url,
                folder,
                before,
            } => self.favourites.move_to(&url, &folder, before.as_deref()),
            M::BookmarkNewFolder { parent, name } => self
                .favourites
                .add_folder(&join_folder(&parent, &name))
                .is_some(),
            M::BookmarkRenameFolder { from, name } => {
                let leaf = name.replace('/', "-");
                self.favourites
                    .rename_folder(&from, &join_folder(folder_parent(&from), &leaf))
                    > 0
            }
            M::BookmarkMoveFolder { from, parent } => {
                self.favourites.move_folder(&from, &parent).is_some()
            }
            M::BookmarkDeleteFolder { folder, keep_items } => {
                self.favourites.delete_folder(&folder, keep_items);
                true
            }
            M::BookmarkRemove { urls } => self.favourites.remove_many(&urls) > 0,
            M::BookmarkOpen { urls, how, group } => {
                self.open_bookmarks(win_id, urls, &how, &group, event_loop);
                false
            }
            M::BookmarkMenu {
                url,
                folder,
                from,
                x,
                y,
            } => {
                let (ox, oy) = self.surface_origin(win_id, &from);
                let y = if from == "bar" {
                    self.windows
                        .get(&win_id)
                        .map_or(y, |w| w.chrome_height() - 4.0)
                } else {
                    y + oy
                };
                self.bookmark_context_menu(win_id, url, folder, x + ox, y);
                false
            }
            M::BookmarkFolderMenu { folder, x } => {
                let y = self
                    .windows
                    .get(&win_id)
                    .map_or(84.0, |w| w.chrome_height() - 4.0);
                self.bookmark_folder_menu(win_id, &folder, x, y);
                false
            }
            M::BookmarkOverflow { skip, x } => {
                let y = self
                    .windows
                    .get(&win_id)
                    .map_or(84.0, |w| w.chrome_height() - 4.0);
                self.bookmark_overflow_menu(win_id, skip, x, y);
                false
            }
            M::BookmarkPage { folder } => {
                let page = self
                    .windows
                    .get(&win_id)
                    .and_then(|w| w.tabs.active_tab().cloned());
                if let Some(t) = page {
                    self.favourites.add_in(&t.url, &t.title, &folder)
                } else {
                    false
                }
            }
            _ => false,
        };
        if changed {
            self.push_data();
        }
    }

    /// `bm:<verb>:<payload>` from a bookmark menu.
    pub(super) fn bookmark_action(
        &mut self,
        win_id: WindowId,
        rest: &str,
        event_loop: &ActiveEventLoop,
    ) {
        let (verb, arg) = rest.split_once(':').unwrap_or((rest, ""));
        let folder_urls = |app: &Self| app.favourites.urls_in(arg, true);
        match verb {
            "open" => self.open_bookmarks(win_id, vec![arg.to_string()], "tab", "", event_loop),
            "open-window" => {
                self.open_bookmarks(win_id, vec![arg.to_string()], "window", "", event_loop)
            }
            "open-private" => {
                self.open_bookmarks(win_id, vec![arg.to_string()], "private", "", event_loop)
            }
            "open-all" => {
                let urls = folder_urls(self);
                self.open_bookmarks(win_id, urls, "tab", "", event_loop)
            }
            "open-all-window" => {
                let urls = folder_urls(self);
                self.open_bookmarks(win_id, urls, "window", "", event_loop)
            }
            "open-all-group" => {
                let urls = folder_urls(self);
                self.open_bookmarks(win_id, urls, "group", folder_leaf(arg), event_loop)
            }
            "rename" => {
                let current = self
                    .favourites
                    .items()
                    .iter()
                    .find(|f| shoji_core::userdata::same_page(&f.url, arg))
                    .map(|f| f.title.clone());
                if let Some(current) = current {
                    self.open_prompt(
                        win_id,
                        Prompt::BookmarkName(arg.to_string()),
                        "Rename bookmark",
                        &current,
                    );
                }
            }
            "folder-of" => self.ask_favourite_folder(win_id, arg),
            // The "Save to" card: file the bookmark it was opened for.
            "put" | "put-new" | "unsave" => {
                let Some(url) = self.windows.get(&win_id).and_then(|w| w.save_url.clone()) else {
                    return;
                };
                match verb {
                    "put" => {
                        self.favourites.set_folder(&url, arg);
                    }
                    "unsave" => {
                        self.favourites.remove(&url);
                    }
                    _ => {
                        return self.open_prompt(
                            win_id,
                            Prompt::NewFolderFor(url),
                            "New folder",
                            "",
                        );
                    }
                }
                self.push_data();
            }
            "copy" => {
                vault_win::copy_text(arg);
            }
            "remove" => {
                if self.favourites.remove(arg) {
                    self.push_data();
                }
            }
            "rename-folder" => self.open_prompt(
                win_id,
                Prompt::RenameBookmarkFolder(arg.to_string()),
                "Rename folder",
                folder_leaf(arg),
            ),
            "new-folder" => self.open_prompt(
                win_id,
                Prompt::NewBookmarkFolder(arg.to_string()),
                "Name the new folder",
                "",
            ),
            "delete-folder-keep" | "delete-folder" => {
                self.favourites
                    .delete_folder(arg, verb == "delete-folder-keep");
                self.push_data();
            }
            "folder" => {
                // A folder inside a folder opens beside the menu it was picked from.
                let (x, y, w) = self
                    .windows
                    .get(&win_id)
                    .map_or((8.0, 84.0, 300.0), |w| w.menu_at);
                self.bookmark_folder_menu(win_id, arg, x + w - 6.0, y + 4.0);
            }
            "manager" => self.open_internal(win_id, "bookmarks"),
            _ => {}
        }
    }

    /// Menu and palette actions that are not tab or window plumbing.
    pub(super) fn extra_action(
        &mut self,
        win_id: WindowId,
        name: &str,
        event_loop: &ActiveEventLoop,
    ) -> bool {
        use UiToHostMessage as M;
        let active_url = |app: &Self| {
            app.windows
                .get(&win_id)
                .and_then(|w| w.tabs.active_tab())
                .map(|t| t.url.clone())
                .unwrap_or_default()
        };
        match name {
            "new-private-window" => {
                self.open_window(event_loop, None, true);
            }
            "bookmark-page" => self.bookmark_current(win_id),
            "bookmark-all" => self.bookmark_all_tabs(win_id),
            "bookmarks-bar" => {
                let on = !self.settings.appearance.bookmarks_bar;
                self.set_bookmarks_bar(on);
            }
            "bookmark-manager" => self.open_internal(win_id, "bookmarks"),
            "bookmarks-panel" => self.toggle_sidebar(win_id, "bookmarks"),
            "bookmark-export" => self.handle_ipc(win_id, None, M::ExportFavourites, event_loop),
            "bookmark-import" => self.create_tab(win_id, Some("shoji://welcome#import".into())),
            "print" => self.handle_command(win_id, Command::Print, event_loop),
            "edit-cut" | "edit-copy" | "edit-selectall" => {
                let js = match name {
                    "edit-cut" => "document.execCommand('cut')",
                    "edit-copy" => "document.execCommand('copy')",
                    _ => "document.execCommand('selectAll')",
                };
                if let Some(tab) = self.windows.get(&win_id).and_then(|w| w.active()) {
                    let params = serde_json::json!({ "expression": js, "userGesture": true });
                    webview::cdp(&tab.view, "Runtime.evaluate", &params.to_string());
                    webview::focus(&tab.view);
                }
            }
            "edit-paste" => {
                if let (Some(text), Some(tab)) = (
                    vault_win::paste_text(),
                    self.windows.get(&win_id).and_then(|w| w.active()),
                ) {
                    webview::focus(&tab.view);
                    let params = serde_json::json!({ "text": text });
                    webview::cdp(&tab.view, "Input.insertText", &params.to_string());
                }
            }
            "copy-link" => {
                let url = active_url(self);
                if url.starts_with("http") {
                    vault_win::copy_text(&url);
                    self.toast(&serde_json::json!({
                        "title": "Link copied",
                        "text": url,
                        "pose": "unfold",
                        "ms": 2500,
                    }));
                }
            }
            "exit" => self.quit(event_loop),
            "help-about" => self.open_settings_section(win_id, "about"),
            "extensions" => self.open_settings_section(win_id, "extensions"),
            "help-whatsnew" => self.create_tab(
                win_id,
                Some("https://github.com/KJX469/Shoji/blob/main/CHANGELOG.md".into()),
            ),
            "help-update" => self.create_tab(
                win_id,
                Some("https://github.com/KJX469/Shoji/releases/latest".into()),
            ),
            "help-report" => self.create_tab(
                win_id,
                Some("https://github.com/KJX469/Shoji/issues/new/choose".into()),
            ),
            "hard-reload" => self.handle_command(win_id, Command::HardReload, event_loop),
            "view-source" => self.handle_command(win_id, Command::ViewSource, event_loop),
            _ => return false,
        }
        true
    }

    /// Open Settings scrolled to a section ("about", "extensions").
    fn open_settings_section(&mut self, win_id: WindowId, section: &str) {
        let has = self.windows.get(&win_id).is_some_and(|w| {
            w.tabs
                .tabs()
                .iter()
                .any(|t| t.url.starts_with("shoji://settings"))
        });
        if has {
            self.open_internal(win_id, "settings");
            self.eval_all_internal(&format!(
                "window.__goto && window.__goto({})",
                js_string(section)
            ));
        } else {
            self.create_tab(win_id, Some(format!("shoji://settings#{section}")));
        }
    }

    /// What the menu's History and Recently closed lists show.
    pub(super) fn menu_data(&self, win_id: WindowId) -> serde_json::Value {
        let private = self.windows.get(&win_id).is_some_and(|w| w.private);
        let history: Vec<serde_json::Value> = if private || self.settings.privacy.ephemeral_default
        {
            Vec::new()
        } else {
            self.history
                .page("", 0, 10)
                .0
                .iter()
                .map(|h| serde_json::json!({ "url": h.url, "title": h.title }))
                .collect()
        };
        let closed: Vec<serde_json::Value> = self
            .pages
            .read()
            .map(|c| {
                c.recent
                    .iter()
                    .take(8)
                    .map(|r| serde_json::json!({ "url": r.url, "title": r.title }))
                    .collect()
            })
            .unwrap_or_default();
        serde_json::json!({
            "history": history,
            "closed": closed,
            "private": private,
            "bar": self.settings.appearance.bookmarks_bar,
            "sidebar": self.settings.appearance.sidebar,
        })
    }
}
