# Changelog

All notable changes to Shoji are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- **Bookmarks, properly.** "Favourites" are now Bookmarks everywhere. A bookmarks bar under the address bar (Ctrl+Shift+B, or Settings): folders first, then bookmarks, extras behind a » button, drag to reorder or onto a folder, right-click for open / rename / move / copy / delete. Folders nest, can be empty, and open as a dropdown from the bar.
- **Bookmark manager** (`shoji://bookmarks`, Ctrl+Shift+O): folder tree, search, add and edit (name, address, folder), drag to file, multi-select to open, move or delete, import and export.
- **Bookmarks dropdown.** Point at the toolbar's bookmarks button (Settings: Bookmarks button) or at "All bookmarks" on the bar and the list drops down; move off and it folds away at once; only typing inside keeps it. It floats over the page, never takes page space and never opens by itself. It is built ahead of time, so it opens at once, and draws at most 300 rows however many bookmarks there are; arrow keys, Enter, Delete and Right / Left on folders work.
- **The star asks where a bookmark should live.** Ctrl+D or the star saves the page and drops a short "Save to" list under the star: Top level, every folder (the one it is in is ticked), New folder…, Remove bookmark. A click files it; ignoring it keeps it at the top level.
- **Ctrl+Shift+D** bookmarks every open tab into a dated folder.
- **New private window** (Ctrl+Shift+N): no cookies, history, saved logins or session; a violet title bar and "Private" label.
- **A fuller menu, like Brave's:** Passwords and autofill, History (with recent and recently closed), Bookmarks and lists, Extensions, Find and edit (cut, copy, paste, select all), Save and share, More tools, Help, Exit, and Zoom with full screen. Shoji's own tools sit in a row of tiles.

### Fixed
- **Uninstall removes everything.** It runs from a temporary copy, no longer locks the profile it deletes, stops only the installed copy, and keeps retrying until the program folder, settings, profile, updates, temp files, shortcuts and Windows' records of Shoji are gone. Deleting your data is now ticked by default.
- Setup no longer leaves a `ShojiSetup.exe.WebView2` folder next to itself.
- **The toolbar and tab strip no longer wash out to light grey on a light page** (Brave Search reports white while it is being darkened): the page-colour tint now skips colours that fight the theme.
- **Darkened pages are held back as the dark canvas until they look dark**, instead of showing their white page for a second first.
- **Dragged tabs stay inside the strip** (they were clipped at its edges); tabs shrink to an icon before the strip ever scrolls.
- **Split pills** have a clearer outline and a roomier seam, so the L|R badge no longer crowds a pane's title or close button.

### Changed
- The bookmark manager draws the first rows at once and the rest as you scroll, marks selected rows without redrawing, and listens through the list instead of per row, so thousands of bookmarks stay quick.
- Back and forward are one solid arrow (turned round for back), in the toolbar and the split-view pane bars.
- The menu is tighter (29 px rows) so all of it fits a normal window without scrolling.
- Exporting bookmarks writes folders at any depth.

## [0.1.5] - 2026-10-17

### Added
- **Split groups.** A split pair is now a named, coloured group you create on purpose (Ctrl+Shift+S picks the partner, or "Split view with the open tab" in the tab menu). You can have several. Each shows a chip ahead of its tabs and an L/R (or T/B) badge on each pane, and the marks stay visible while other tabs are open. Rename, recolour, swap panes or ungroup from the chip's menu. Nothing is split for you any more (dropping a link no longer opens a split).
- **Drag a tab out and its window follows the cursor.** Pull a tab off the strip and its real window comes with the pointer: drop it in another Shoji window's strip (a bar shows where), on the desktop to keep it as a window, or press Esc to put it back. Drag a tab near the left or right edge of another window and a tile glides into that half; let go to snap the tab in beside the open page as a split group.
- **Faster tear-off and divider.** The dragged window appears at once and stays above the others (a spare window is built while idle); the split divider follows the live mouse, so fast drags no longer lag.
- **Bigger tabs.** Taller tabs, bigger close, mute and new-tab buttons, and a larger title bar; tabs, split groups and tab groups are higher contrast in light and dark.
- **Pinned tabs.** Right-click a tab, Pin tab: it shrinks to its icon, sits at the left, is kept across restarts, and ignores Ctrl+W.
- **Favourite folders.** Imports keep their folders (Chrome, Edge, Brave, Firefox, bookmarks files, IE favourites); right-click a favourite on the new tab page, or use the tab menu, to file it in a folder. The new tab page groups them.
- **Export favourites** as a bookmarks file any browser can import (Settings → Privacy).
- **Site choices list** (Settings → Privacy → Manage…): every remembered zoom and allowed or blocked permission, each removable, or forget a whole site.
- **Block third-party cookies** switch (Settings → Privacy; applies after a restart; not yet runtime-verified).
- **Extensions (experimental).** Settings → Extensions: switch on, restart, then add an unzipped Chromium extension folder; the engine loads it into the browsing profile. No store, no toolbar button, background pages may not run.
- **Mute.** A speaker mark appears on a tab that is playing sound; click it, or use the tab menu, to mute.
- **Workspaces and Split View.** Organize tabs into named, colored spaces and view any two tabs side by side with a vertical or horizontal divider.

### Changed
- "All bookmarks" on the bookmarks bar drops the whole list down when you hover it (folders open beside it); clicking it opens the bookmarks page.
- **Clear a site's cookies from the site dropdown.** The dropdown behind the seal/lock shows how many cookies the page uses and has "Clear cookies and site data…" (press twice): it deletes the site's cookies and its stored data (local storage, databases, caches), then reloads the page.
- **Dropdowns can no longer be pinned.** The pin button is gone from every dropdown; they always float over the page and never resize the windows or split panes.
- Split panes' address bars show the same red seal on secure pages as the single bar (not a padlock).
- **Adding a split pane works like the address bar.** The add-a-pane window is now an address bar: open tabs that match what you type come first (Enter adds that very tab, nothing reloads); under them are the ways to open it in a new pane: the site from your history, what you typed, matching history, and the search engine's suggestions. With no matching tab open, the site from your history comes first. Parent domains complete too ("wiki" finds wikipedia.org).
- **The Byōbu shield lives at the right end of every address bar** (inside the field, as in Brave), in the same spot on every page and in every split pane; the logo or site icon stays at the left.
- **The address bar learns your sites.** Typing the start of a site you visit ("yout") completes it in the bar ("youtube.com") and puts it first, already selected, so Enter goes straight there; the more and the more recently you visit a site, the likelier it is. What you typed stays the second row, as a search or an address.
- **Suggestions no longer flash.** The suggestion window keeps one size and shows only the part that is needed, instead of resizing on every key, so the page behind never shows through.
- **Proportions like Brave's.** Tabs are 36 px in a 44 px strip, the address bar is 34 px in a 40 px toolbar (same 84 px in all).
- The crane colour on the desktop and Start menu shortcuts is restored at every start, so an update or reinstall no longer leaves them white.
- **Search engine list on the logo.** Click the search engine's logo in the address bar (on a new tab) to pick another engine; on a website the same spot still opens the site's information. The Byōbu shield is now always the first thing at the left of every address bar, in the same place on every page and in every split pane.
- **Search suggestions work and the first one is selected.** The engine's suggestions (Google, Bing, Brave, Ecosia, DuckDuckGo) are now fetched by the browser itself, since the page was refused by their cross-origin rules and never showed them; what you typed is the first row, already selected, so Enter takes it.
- **Split view strip.** A split is a plain pill with numbered seam badges and a raised selected pane (the bracket is gone); tabs are shorter (34 px), address bars are 32 px in single and split view alike, and a pane's Byōbu shield sits at the left of its address field with no padlock beside it.
- **Address bar like Brave's.** On a new tab the search provider's logo sits at the left of the address field (instead of the square), the Byōbu shield always lives inside the field beside it (in split view too, on every pane's bar), and the hint reads "Search Google or type a URL". A window that opens on the new tab page starts with the address bar active, so typing goes there and gets the search engine's suggestions.
- Dark websites: picking another colour restyles open pages at once (including YouTube); Dark Reader's per-site fixes are built in; darkened pages report a light colour scheme so sites that follow the system theme (YouTube, GitHub) are darkened fully; sites with their own dark theme get your chosen look (`dark_detect_native` in `settings.json` restores leaving them alone); the pages on screen change first.
- Websites show their text in the interface font you pick in Settings (icons and code keep their own).
- Closing a window's last tab with Ctrl+W closes the window.
- **Split view for many tabs.** A split is now one combined tab in the strip (no chip taking room): every pane keeps its favicon, title, mute and close, a numbered badge sits on each seam (L|R for two), and the selected pane is marked by the accent line dropping down both seams and running under it. Splits hold up to 20 panes in equal columns with a draggable divider between each (the flip button stacks them), and each pane has its own toolbar (back, forward, reload, address, favourite, dark mode) that drops buttons into a menu as it narrows; downloads, notes and the menu stay window-wide beside the window controls. Add a tab with the split button, by dropping a tab on the combined tab, or by dragging a window to the edge of one. Creating a split slides the tabs together with a short merge animation.
- **The "Main" space pill only shows once you have more than one space** (Alt+W or "Move to a new space" in a tab's menu).
- **Dragging a tab into another window** shows a preview tab that follows the pointer along the strip; nothing moves until you let go, and Esc puts it back.
- **Smoother tab drag.** A tab pulled out of the strip stays under the pointer. Over another window's tab strip the dragged window melts away while the tabs part and a slot opens; on drop the tab settles into it. Tabs now slide into place when one closes, opens, moves or is dropped in, instead of jumping.
- **A clearer dark mode panel.** One switch for dark websites, then three plain choices for the site you are on (same as other sites, always dark, never dark), then the style: presets, with the sliders under Fine-tune, for every site or just this one. Every choice now does something whichever one was showing before, so you no longer have to pick "left as is" first.
- **Dark mode on pages that resisted it.** A page that is still light a moment after dark mode was applied (Brave Search, for one) is now darkened by inverting it, so turning dark mode off and on again isn't needed.
- **Extensions work out of the box.** They are on by default, you can install one by pasting its Chrome Web Store link or id in Settings, and each one's popup and options pages open from the list (they have no toolbar button).
- **The tray icon follows your crane colour.**
- **Browser-like defaults.** Search suggestions are on (choosing Private mode in setup turns them off), and downloads go straight to your Downloads folder without asking (that folder is found through Windows, so it follows a redirected Downloads). Settings saved by older versions are brought along once.
- **Updates install in the background.** A new version is put in place beside the running copy as soon as it is downloaded (nothing closes, nothing is shown), so the next start is already the new one; it restarts into it from the tray when the last window closes. Only the installed copy updates itself, never a portable or development one.

### Fixed
- A split's tab pill no longer squeezes to nothing in a crowded tab strip and spills over the tab next to it; it keeps room for each pane.
- The first letter typed in the address bar no longer drops the keyboard focus: the suggestion window, built the first time it is needed, used to take focus from the bar.
- Frosted toolbar: the toolbar and tab strip no longer show as a white band (windowed WebView2 paints a transparent background white); the toolbar keeps its translucent look over a solid colour.
- Idle CPU: the window no longer wakes continuously when there is one tab (it used to use a whole core); the tear-off spare window is built only when nothing is loading.
- Heavy pages (ad-laden news sites) no longer pin a CPU core and freeze the window when several tabs are open: the loading crane in the tab now animates in steps instead of at 60 fps.
- Black flashes: the notepad, dropdowns, menus and new tab pages are shown only after their first frame.
- `window.open()` and `target="_blank"` keep their opener (sign-in popups, payment frames); tab ids are unique across windows; closing a split no longer leaves a pane on top; many tabs scroll in the strip; idle tabs show their titles.
- One wrong value in `settings.json` keeps the rest; damaged settings, vault and data files are kept as `.bad`; `localhost:3000` and IPs open over http; `blob:`, `mailto:`, `ftp:` are refused.
- Dark websites now work on pages whose Content-Security-Policy blocks inline styles.
- Shield: Aggressive strips tracking parameters from every request; `$document` rules (malware, phishing) stop a page with a warning; `navigator.globalPrivacyControl`; `$method=` rules; link cleaning handles Google `url=`, Reddit, href.li and DuckDuckGo and keeps signed links intact.

## [0.1.4] - 2026-10-13

### Added
- **Browser dark mode is adjustable.** The dark websites dropdown has a Browser tab with the same
  presets, brightness, contrast, sepia, grayscale and colors, applied live to the toolbar, menus,
  title bar and Shoji's own pages.

### Fixed
- Ctrl+S and Ctrl+G are no longer taken from web apps (Save and Find next stay in the palette).
- A tab stuck "loading" stops flapping and showing a stop button after 30 seconds.
- The toolbar strip's window clip is only changed when a card opens or closes.

## [0.1.3] - 2026-10-13

### Added
- **History page** (`shoji://history`, Ctrl+H): by day, searchable, select and remove, with
  clear-browsing-data (Ctrl+Shift+Del) for a time range: history, download list, cookies, cache.
- **Downloads page** (`shoji://downloads`) and a download list kept across restarts, with Open and
  Show in folder; a card appears when a file is saved.
- **Dark websites dropdown**: presets (Sumi, Midnight, Dusk, Oled, Paper), mode, brightness, contrast,
  sepia, grayscale, background and text colors, font, text stroke, a daily schedule, and a look of
  its own for any site.
- **Shortcuts:** Ctrl+H, Ctrl+Shift+Del, Ctrl+U, Ctrl+S, F11, F3 / Ctrl+G, F6, Ctrl+, , Ctrl+Shift+A,
  Ctrl+PgUp / PgDn, Alt+Home.
- **A crane with a few things to say.** It guards the shield dropdown, carries downloads, naps on empty
  pages and sleeping tabs, flaps in a tab that is loading, looks around on a page that can't be found,
  and keeps a count of folded paper cranes (`shoji://wish`). A few more things aren't listed.
- A vermilion site seal in place of the padlock, a brush stroke across the toolbar while a page loads,
  tabs that fold away, and the new tab page's season and greeting (Settings → Appearance).
- Page not found and offline pages.

### Changed
- Menu, site, shield and downloads are floating dropdowns with their own motion, hanging from their
  buttons. They no longer resize the page. Pin one to dock it as a side panel.
- Find, permission and download cards float over the page instead of pushing it down.

### Fixed
- Visits to pages opened from the address bar were not recorded in history.

## [0.1.2] - 2026-10-13

### Added
- **Clean links.** Tracker redirects (Google, Facebook, Reddit, YouTube and others) are skipped and
  tracking parameters (`utm_*`, `fbclid`, `gclid`, ...) are removed before a page loads.
- **Global Privacy Control.** Every request carries `Sec-GPC: 1`.
- Both are on by default, with switches under Settings → Privacy.
- **Import from your old browser** in setup (and Settings → Privacy → Import…): favourites, history, sign-ins
  and saved passwords from Chrome, Edge, Brave, Firefox and 40+ more (Opera, Vivaldi, Arc, Yandex, LibreWolf,
  Waterfox, Zen, Safari bookmarks files, Internet Explorer favourites…), each with its real logo. Installed
  browsers are marked Found and every profile is listed. Values sealed to the old browser itself (Chrome, Edge
  and Brave "app-bound" sign-ins and passwords) are skipped and reported. Firefox passwords import as CSV.
- **Password manager** (`shoji://passwords`): an encrypted vault with search, add/edit/delete, a generator,
  weak and reused checks, CSV import and export, and an optional master password (locks after 15 idle minutes).
  Shoji offers to save logins when you sign in and fills them from a menu on the login field. Copied passwords
  skip clipboard history and clear after 30 seconds. It replaces the engine's own password saving.
- **Default browser.** Shoji now registers with Windows and offers to become your default in setup.
- **History.** Shoji keeps a browsing history (off in Private mode, cleared in Settings) that feeds address-bar suggestions.
- Setup shows progress across every step, answers Enter and Esc, and lets you skip each step. The crane
  has a gesture for every step and flies off when setup ends, however you leave it.

### Changed
- Removed the faint halo around the crane.
- Favourites hold up to 5,000 entries (was 500).
- Setup's Privacy step comes before Search and uses icons.

### Fixed
- Version number now matches the release, so the updater no longer re-offers the current build.
- Uninstall path guard also rejects drive roots on any platform.

## [0.1.1] - 2026-10-13

### Added
- **Automatic updates.** Shoji checks the project's GitHub releases every hour, downloads a newer
  `ShojiSetup.exe`, verifies it against the release's published SHA-256, and installs it only
  while no window is open (when it is waiting in the tray). The new version starts back in the
  tray. Turn it off under Settings → Performance.
- **Runs in the tray.** Closing the last window keeps Shoji running in the notification area
  (Open / Quit menu) so Byōbu's filter lists keep updating. Turn it off under Settings →
  Performance. Private mode always quits.
- **One copy per install.** Starting Shoji again opens a window in the running copy instead of a
  second process. The installer and uninstaller ask a running copy to quit (`--quit`) first.
- A red **Uninstall Shoji** button under Settings → Danger zone. After a warning that notes,
  logins and everything else will be erased, and typing `UNINSTALL`, it removes the app, its
  shortcuts, its Windows registration, and all settings and profile data.

### Changed
- Byōbu checks every filter list at launch and then every hour, instead of every 6–12 hours.

## [0.1.0] - 2026-10-12

First public version.

### Added
- A Rust shell around the WebView2 runtime: tabs in the title bar, address bar, side panel,
  find in page, downloads, tear-off tabs and per-tab memory management.
- Seven crane colours, the Washi (light) and Sumi (dark) themes, three type sets and a first-run
  setup.
- Remember-by-default profile in `%LOCALAPPDATA%\Shoji\User Data`, with a Private mode that keeps
  everything in memory and a list of sites to keep signed in.
- Dark websites, built on a modified Dark Reader.
- Quick notes, reader view, split view, tab groups, favourites, a command palette and session
  restore.
- **Byōbu**, the built-in ad and tracker shield: Brave's `adblock` engine with uBlock Origin and
  Brave lists fetched at runtime, a toolbar shield, per-site pause, request log, element picker,
  list management and custom rules.
- A per-user installer and uninstaller that need no administrator rights.

[Unreleased]: https://github.com/KJX469/Shoji/compare/v0.1.5...HEAD
[0.1.5]: https://github.com/KJX469/Shoji/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/KJX469/Shoji/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/KJX469/Shoji/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/KJX469/Shoji/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/KJX469/Shoji/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/KJX469/Shoji/releases/tag/v0.1.0
