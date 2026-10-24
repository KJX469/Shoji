# Changelog

## [Unreleased]

## [0.1.7] - 2026-10-23

### Added
- Cookie notices are answered with "Reject all" (Consent-O-Matic's rules), and can be hidden.
- Ad popups and popunders are blocked.
- Downloads from known dangerous sites are stopped, with a way to download anyway.
- Byōbu uncloaks trackers hidden behind a site's own subdomains (CNAME).
- Links lose more tracking parameters (AdGuard URL Tracking, Brave's per-site rules); tracker redirects are skipped and AMP pages open on the publisher's site.
- Regional ad lists for the Windows display language.
- Shoji fixes: Shoji's own filter list, updated hourly, so broken sites can be fixed without a release.
- Sound waits for a click; muted video still plays. Sites can be allowed from the This site menu.
- Secure DNS setting (Automatic by default).
- Optional blocking of Facebook, X and LinkedIn embeds.
- Per-site shield categories, and an offer to lower the shield after repeated reloads.
- Sites can be forgotten when their tab closes or when Shoji exits.
- Password breach check on the passwords page.
- Report a broken site from the shield panel.
- A Wait or Close page card when a page stops responding.
- Beta updates (opt-in), and an offer to go back after a new version closes early twice.
- Extensions: a store page with a catalog, the Web Store's Add to Chrome button and an Add to Shoji card on every extension page, toolbar buttons with popups, and a puzzle menu with logos, pin and remove.

### Changed
- Byōbu checks requests on a thread of its own and uses about half the memory.
- Global Privacy Control, de-AMP, the sound hold and fingerprinting protection apply from the first script of every page.
- Fingerprinting protection covers more surfaces.
- Cross-site requests send only the page's origin as referrer; third-party requests lose high-entropy client hints; click pings are blocked.
- Hidden tabs rest after 30 seconds, sooner when Windows is low on memory; Energy saver also stops animations.
- The Byōbu page shows each list's age and last error, and warns about lists over a week old.
- Settings and Byōbu controls are named for screen readers.
- The browser's icon is the crane again; the shoji-screen tile is the installer's.

### Fixed
- Engine switches, such as third-party cookie blocking, apply on every machine.
- Installed extensions come back after Shoji is closed by a crash or Task Manager.
- Coloured shortcut icons follow icon changes after an update.
- Damaged browser profiles, extension packages or settings files can no longer crash Shoji.

### Known limitations
- 1Password's Add account doesn't work yet; its desktop app may be needed.

## [0.1.6] - 2026-10-21

### Added
- Containers: Work, Personal, Shopping, Banking and temporary ones, each with its own cookies; a site can always open in one.
- Fingerprinting protection (Standard by default, Strict, Off).
- Bounce-tracking cleanup: sites that only appear as redirect hops lose their cookies after a week.
- Crash reports kept on this PC, with an offer to open a prefilled issue.
- The Byōbu page shows how long a request check takes.

### Changed
- Byōbu blocks and hides in cross-origin frames and workers too.
- Byōbu is ready at start: the compiled shield is reused (about 90 ms instead of 950 ms).
- Third-party cookies are blocked by default.
- Byōbu's scriptlet data comes from reviewed, pinned uBlock Origin and Brave commits.
- Internal pages send a strict Content-Security-Policy; page messages are checked more strictly.

### Fixed
- Private mode now forgets data kept before it was turned on.

## [0.1.5] - 2026-10-19

### Added
- Updates ask first: a letter in the toolbar offers restart now, on close, later or skip.
- Settings › About: check for updates and see what's new.
- New Kage icon.
- New welcome screen and installer.
- Bookmarks bar (Ctrl+Shift+B), bookmark manager (Ctrl+Shift+O) and bookmarks dropdown.
- The star (Ctrl+D) asks which folder to save to; Ctrl+Shift+D bookmarks all tabs.
- Private windows (Ctrl+Shift+N).
- Fuller main menu.
- Split groups: named, coloured pairs of tabs side by side (Ctrl+Shift+S).
- Drag a tab out and its window follows the cursor; drop it in another window or snap it to an edge.
- Pinned tabs, tab mute, workspaces.
- Export bookmarks; manage per-site zoom and permissions.
- Block third-party cookies.
- Chromium extensions from a folder or Chrome Web Store link.

### Changed
- Bigger tabs and title bar, higher contrast.
- Address bar completes sites you visit, shows search suggestions and lets you switch engine from its logo.
- The Byōbu shield sits inside every address bar.
- Clear a site's cookies and data from the site dropdown.
- Dropdowns always float over the page.
- Dark websites restyle open pages at once and handle more sites.
- Idle pages rest; the window no longer wakes when idle.
- Updates install in the background.

### Fixed
- Uninstall removes everything, and setup leaves nothing beside itself.
- Light flashes and white bands in dark mode.
- Heavy pages no longer freeze the window.
- Popups keep their opener, so sign-in and payment windows work.
- One bad value in `settings.json` no longer resets the rest.
- Dark websites on pages with strict Content-Security-Policy.

## [0.1.4] - 2026-10-13

### Added
- Browser dark mode is adjustable.

### Fixed
- Ctrl+S and Ctrl+G are left to web apps.
- Tabs stuck loading stop after 30 seconds.

## [0.1.3] - 2026-10-13

### Added
- History (Ctrl+H) and clear browsing data (Ctrl+Shift+Del).
- Downloads page.
- Dark websites settings: presets, brightness, contrast, colours, fonts, schedule, per site.
- More shortcuts.
- Not-found and offline pages.

### Changed
- Menus and panels float over the page.

### Fixed
- Pages opened from the address bar are recorded in history.

## [0.1.2] - 2026-10-13

### Added
- Tracking links cleaned and Global Privacy Control sent.
- Import from 40+ browsers.
- Password manager.
- Set as default browser.
- Browsing history.

### Fixed
- Version number matches the release.

## [0.1.1] - 2026-10-13

### Added
- Automatic updates.
- Runs in the tray.
- One copy per install.
- Uninstall from Settings.

### Changed
- Filter lists update hourly.

## [0.1.0] - 2026-10-12

First release.

[Unreleased]: https://github.com/KJX469/Shoji/compare/v0.1.7...HEAD
[0.1.7]: https://github.com/KJX469/Shoji/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/KJX469/Shoji/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/KJX469/Shoji/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/KJX469/Shoji/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/KJX469/Shoji/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/KJX469/Shoji/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/KJX469/Shoji/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/KJX469/Shoji/releases/tag/v0.1.0
