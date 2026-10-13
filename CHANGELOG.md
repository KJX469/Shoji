# Changelog

All notable changes to Shoji are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and versions follow
[Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- A red **Uninstall Shoji** button under Settings → Danger zone. After a warning that notes,
  logins and everything else will be erased, and typing `UNINSTALL`, it removes the app, its
  shortcuts, its Windows registration, and all settings and profile data.

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
