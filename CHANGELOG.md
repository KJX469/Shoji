# Changelog

All notable changes to Shoji are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [0.2.0] - 2026-10-13

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

[Unreleased]: https://github.com/KJX469/shoji/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/KJX469/shoji/releases/tag/v0.1.0
