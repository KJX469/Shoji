# Third-party notices

Shoji is MIT licensed. It includes, links or downloads the following, each under its own licence.

## Included

| Component | Licence |
| --- | --- |
| [Dark Reader](https://github.com/darkreader/darkreader) (modified) | MIT |
| Browser logos from [browser-logos](https://github.com/alrra/browser-logos) and [Simple Icons](https://simpleicons.org) | MIT / CC0 |
| Instrument Sans, Newsreader, Fraunces, Literata | SIL OFL 1.1 |

## Linked

| Crate | Licence |
| --- | --- |
| [adblock](https://github.com/brave/adblock-rust) | MPL-2.0 |
| [winit](https://github.com/rust-windowing/winit), [wry](https://github.com/tauri-apps/wry) | Apache-2.0 / MIT |
| [windows](https://github.com/microsoft/windows-rs), webview2-com | MIT / Apache-2.0 |
| [ureq](https://github.com/algesten/ureq), rustls | MIT / Apache-2.0 / ISC |
| [rusqlite](https://github.com/rusqlite/rusqlite) with [SQLite](https://sqlite.org) | MIT / public domain |
| [ring](https://github.com/briansmith/ring) | ISC-style |
| Other dependencies (see `Cargo.lock`) | MIT and/or Apache-2.0 |

## Downloaded at runtime

| Data | Licence |
| --- | --- |
| [uBlock Origin](https://github.com/gorhill/uBlock) scriptlets and [uAssets](https://github.com/uBlockOrigin/uAssets) lists | GPL-3.0 |
| [EasyList, EasyPrivacy and EasyList Cookie](https://easylist.to) | GPL-3.0 / CC BY-SA 3.0 |
| [Brave lists and resources](https://github.com/brave/adblock-resources), [debounce and URL-cleaning rules](https://github.com/brave/adblock-lists) | MPL-2.0 |
| [AdGuard URL Tracking filter](https://github.com/AdguardTeam/AdguardFilters) | GPL-3.0 |
| Regional lists from Brave's catalog, for the display language | Each list's own |
| [Consent-O-Matic](https://github.com/cavi-au/Consent-O-Matic) cookie-notice rules | MIT |

These are never compiled into `shoji.exe`. The `byobu-data` release carries a build of uBlock
Origin's scriptlets (GPL-3.0), an unmodified copy of Brave's resources (MPL-2.0) and
Consent-O-Matic's rules merged into one file (MIT, © Janus Bager Kristensen and Rolf Bagge,
CAVI, Aarhus University).

## Trademarks

Browser names and logos shown on the import screen belong to their owners and only identify the
browser to import from. Shoji is not affiliated with or endorsed by any of them. Microsoft and
WebView2 are trademarks of the Microsoft group of companies.
