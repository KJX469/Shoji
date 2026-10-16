# Changelog

All notable changes to Shoji are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- **Pinned tabs.** Right-click a tab, Pin tab: it shrinks to its icon, sits at the left, is kept across restarts, and ignores Ctrl+W.
- **Favourite folders.** Imports keep their folders (Chrome, Edge, Brave, Firefox, bookmarks files, IE favourites); right-click a favourite on the new tab page, or use the tab menu, to file it in a folder. The new tab page groups them.
- **Export favourites** as a bookmarks file any browser can import (Settings → Privacy).
- **Site choices list** (Settings → Privacy → Manage…): every remembered zoom and allowed or blocked permission, each removable, or forget a whole site.
- **Block third-party cookies** switch (Settings → Privacy; applies after a restart; not yet runtime-verified).
- **Extensions (experimental).** Settings → Extensions: switch on, restart, then add an unzipped Chromium extension folder; the engine loads it into the browsing profile. No store, no toolbar button, background pages may not run.
- **Mute.** A speaker mark appears on a tab that is playing sound; click it, or use the tab menu, to mute.

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

[Unreleased]: https://github.com/KJX469/Shoji/compare/v0.1.4...HEAD
[0.1.4]: https://github.com/KJX469/Shoji/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/KJX469/Shoji/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/KJX469/Shoji/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/KJX469/Shoji/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/KJX469/Shoji/releases/tag/v0.1.0
