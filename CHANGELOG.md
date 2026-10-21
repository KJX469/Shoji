# Changelog

## [Unreleased]

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

[Unreleased]: https://github.com/KJX469/Shoji/compare/v0.1.5...HEAD
[0.1.5]: https://github.com/KJX469/Shoji/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/KJX469/Shoji/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/KJX469/Shoji/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/KJX469/Shoji/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/KJX469/Shoji/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/KJX469/Shoji/releases/tag/v0.1.0
