# Third-party notices

Shoji is MIT licensed (see [LICENSE](LICENSE)). It includes, links against or downloads the
following. Each keeps its own licence.

## Included in the source tree

| Component | Licence | Where |
| --- | --- | --- |
| [Dark Reader](https://github.com/darkreader/darkreader) (modified) | MIT | `third_party/darkreader/`, bundled at `crates/app/ui/vendor/darkreader.min.js` (licence beside it) |
| Browser logos ([browser-logos](https://github.com/alrra/browser-logos), MIT, and [Simple Icons](https://simpleicons.org), CC0). The logos remain trademarks of their owners and only identify the browser to import from | MIT / CC0 | `crates/brand/assets/logos/` |
| Instrument Sans, Newsreader, Fraunces, Literata | SIL Open Font License 1.1 | `assets/fonts/` (licence texts alongside) |

## Linked into the browser

Rust crates resolved in [Cargo.lock](Cargo.lock), including:

| Crate | Licence |
| --- | --- |
| [adblock](https://github.com/brave/adblock-rust) (Brave) | MPL-2.0 |
| [winit](https://github.com/rust-windowing/winit), [wry](https://github.com/tauri-apps/wry) | Apache-2.0 / MIT |
| [windows](https://github.com/microsoft/windows-rs), webview2-com | MIT / Apache-2.0 |
| [ureq](https://github.com/algesten/ureq), rustls | MIT / Apache-2.0 (rustls also ISC) |
| [rusqlite](https://github.com/rusqlite/rusqlite) with bundled [SQLite](https://sqlite.org) (reads other browsers' data on import) | MIT; SQLite is public domain |
| [ring](https://github.com/briansmith/ring) (encrypts the password vault) | ISC-style / OpenSSL-derived |
| serde, serde_json and the other dependencies | MIT and/or Apache-2.0 |

Run `cargo tree` for the full list. The MPL-2.0 source for `adblock` is available from its
repository.

## Downloaded at runtime (not part of the binary)

| Data | Licence | Source |
| --- | --- | --- |
| uBlock Origin filter lists and scriptlets | GPL-3.0 | [uBlockOrigin/uAssets](https://github.com/uBlockOrigin/uAssets), [gorhill/uBlock](https://github.com/gorhill/uBlock) |
| EasyList and EasyPrivacy | GPL-3.0 / CC BY-SA 3.0 | [easylist.to](https://easylist.to) |
| Brave filter lists and resources | MPL-2.0 | [brave/adblock-resources](https://github.com/brave/adblock-resources) |

These are fetched onto the user's machine when Byōbu updates and are never compiled into
`shoji.exe`. The `byobu-data` release asset published by this repository's workflow is a build of
uBlock Origin's scriptlets and remains under GPL-3.0.

## Provided by Windows

Web content is rendered by the Microsoft Edge WebView2 runtime. Microsoft and WebView2 are
trademarks of the Microsoft group of companies.
