# Shoji architecture

Shoji is a thin native shell (Rust, `winit` + `wry`) around the WebView2 runtime that ships
with Windows. The shell draws the browser's own surfaces as small HTML pages and hosts each
web page in its own WebView2 controller, all as child windows of one top-level frame.

```
┌ top-level window (winit) ─────────────────────────────────────────────┐
│ chrome strip webview   tabs · address bar · find / permission trays   │
├───────────────────────────────────────────────────────┬───────────────┤
│ active tab webview                                    │ side panel    │
│ (hidden tabs: SetIsVisible(false), memory level Low,  │ webview,      │
│  then TrySuspend after the idle timeout)              │ created lazily│
└───────────────────────────────────────────────────────┴───────────────┘
```

## Crates

| crate | role |
| --- | --- |
| `shoji-brand` | The crane: path data, the seven palettes, a dependency-free rasterizer and `.ico` encoder, plus the shared theme stylesheet. No dependencies. |
| `shoji-core` | Engine-agnostic state: `Settings` (merge-patch updates, tolerant loading, atomic saves), `TabManager`, the typed IPC protocol, environment detection, and Windows shell helpers (shortcuts via `IShellLinkW`). |
| `shoji-byobu` | The ad and tracker shield: a wrapper around Brave's `adblock` matcher and the list updater. |
| `shoji` (app) | The event loop, window and tab management, WebView2 event wiring, internal pages. |
| `shoji-setup` | Per-user installer and uninstaller. |

## Event flow

* **Pages → host.** Every webview posts JSON `UiToHostMessage`s through `window.ipc`. Messages
  from the chrome strip and side panel are trusted. Messages from tabs are accepted only when the
  sending document is one of Shoji's own pages (`http://shoji.<page>/`, served by the custom
  protocol handler), except for a tiny allowlist (`FindResult`) that ordinary web pages may send.
* **WebView2 → host.** COM event handlers (`webview.rs`) send `BrowserEvent`s keyed by
  **tab id**, never by window. The host looks up the owning window when the event arrives, which
  is what lets a live tab be re-parented into another window during tear-off.
* **Host → UI.** State changes only mark a window dirty. `about_to_wait` flushes each dirty window
  once per event-loop turn and skips the send entirely if the serialized tab state is unchanged,
  so a page load that fires five WebView2 events costs one UI update, not five.

## Internal pages

`shoji://newtab`, `shoji://settings`, `shoji://welcome` and the crash / certificate notices are
rendered on request by a custom-protocol handler from templates in `crates/app/ui/`. Because
they are real URLs, back/forward and reload behave normally, and the settings page always shows
current values. Every template inlines the same theme stylesheet; a colour change is one
`data-accent` attribute flip per open document.

## Title bar

The native caption is removed with a window subclass that answers `WM_NCCALCSIZE` (the rest
of the frame stays native, so Windows keeps the shadow, edge resizing and Aero Snap). The tab
strip occupies the top row and draws its own minimize / maximize / close buttons; empty strip
space is a CSS `app-region: drag` area, which WebView2's non-client region support turns into
a native title bar (drag, double-click to maximize, right-click for the system menu). A 4 px top
edge resizes. The window title follows the active page, for the
taskbar and Alt+Tab.

## Theme and icons

* **Modes.** *Washi* (light) and *Sumi* (dark) tokens, chosen by `prefers-color-scheme`. Each
  webview's preferred colour scheme is set from the user's mode (System / Light / Dark), and the
  native caption bar is tinted through DWM to match the tab strip.
* **Accents.** Seven pastel tones derived from the crane palettes. Greyscale (White) is the default.
* **Type.** Three profiles (`TypeProfile` in `shoji-brand`) fill four roles: controls, headings,
  reading text and small labels, as `--font*` variables under `data-fonts`. The four faces are
  Latin-subset variable WOFF2 files built by `scripts/build-fonts.py`, compiled into the
  executable and served at `http://shoji.fonts/` to every surface, including the chrome strip and
  side panel, which load from inline HTML. A change is one attribute flip per open document.
* **Icons.** The crane is rasterized at runtime for the title-bar and taskbar icons
  (`set_window_icon` / `set_taskbar_icon`). On a colour change a tinted `.ico` is written to the
  data folder and Shoji's Start menu, desktop and pinned-taskbar shortcuts are re-pointed at it —
  only shortcuts that launch this copy of `shoji.exe`. The executable's own icon is generated
  from the same data at build time (`build.rs`).

## Dark websites

A modified Dark Reader (`third_party/darkreader`, rebuilt into `ui/vendor/darkreader.min.js`)
is registered per tab with `AddScriptToExecuteOnDocumentCreated` only while the setting is on,
so pages pay nothing when it's off. Toggling swaps the registered script and restyles open pages
in place; Shoji's own pages and user-skipped sites are left alone, and Dark Reader's detector
backs off on sites that already render dark.

## Notes

`notes.json` in the data folder (newest first, written atomically). The notepad is a floating
webview raised above the page views; opening it moves keyboard focus into it so typing lands on
the page immediately. Empty pages are never stored.

## Memory

* Only the visible tab is resized; hidden tabs receive bounds when shown.
* Hidden tabs drop to WebView2's low memory-usage level immediately and are suspended with
  `TrySuspend` after the configured idle time (default 5 minutes). Pages playing audio decline
  and are retried later. Showing a tab resumes it.
* The side panel's webview is created on first use. Certificate details are fetched only when
  the Site panel is open.

## Privacy

* By default tabs use the normal WebView2 profile, so logins, cookies, cache and history persist.
  The profile lives in `%LOCALAPPDATA%\Shoji\User Data`; the engine encrypts cookies and saved passwords with the user's Windows credentials.
  An older profile in `%APPDATA%\shoji\webview2_data` is moved there once at startup.
* With Private mode (`privacy.ephemeral_default`) tabs run in WebView2's in-memory (InPrivate)
  partition unless the site is on the user's "sites to keep" list; navigating between the two
  rebuilds the tab in the right partition. `privacy.mode_chosen` records an explicit choice.
* Favicons come from the page itself (`FaviconChanged`), not from a third-party favicon service.
* The browser refuses to run elevated.

## Byōbu (ad and tracker shield)

`shoji-byobu` wraps Brave's `adblock` matcher. A worker thread
(`crates/app/src/shield.rs::spawn_updater`) fetches uBlock Origin's lists (`quick-fixes` hourly,
the rest every 6–12 h), EasyList/EasyPrivacy, Brave's default lists (read from its catalog, so
Brave's list set is followed automatically) and Brave's own resources, using conditional GETs,
and swaps a rebuilt engine in atomically.

uBlock's scriptlets and redirect stubs are ES modules, so there is no file to download from
upstream. `scripts/byobu-assemble.mjs` imports them from a sparse clone of gorhill/uBlock and emits
adblock `Resource`s; the `byobu-resources` workflow runs it every 6 h and publishes the result as
the `byobu-data` release asset, which the updater downloads like any other list. Users need no
Node or other tooling, and uBlock's GPLv3 code stays out of the MIT binary. Until the first
download succeeds (or offline) network blocking and cosmetic filtering still work; only
scriptlet-based fixes wait.

Each tab subscribes to `WebResourceRequested` (all contexts) for network blocking (blocks,
redirect stubs, `$removeparam`) and `NavigationStarting` to register a per-navigation document
script: `ui/byobu-cosmetic.js` with that site's hide selectors and procedural filters
(`:has-text`, `:upward`, `:xpath`, `:matches-css`, ...), plus scriptlets. Generic class/id rules
use a bridge: the page posts the classes and ids it uses (`byobu:` web messages) and the host
answers with `__byobuHide([...])`. Settings: `shield.mode` (off / standard / aggressive) and
`shield.paused_sites`. `scripts/byobu-sync.ps1` reports upstream releases and updates the matcher.

### Byōbu interface

* **Toolbar shield** (`chrome.html`): a button with a count badge, outline when idle, struck
  through when paused on the site. The host pushes `__shieldSync` from `flush()` so each window
  shows its active tab's counters; counters live in the hub in `shield.rs` and reset on every
  top-level navigation.
* **Side panel, Shield tab** (`sidebar.html`): this page's counts, the per-site switch (writes
  `shield.paused_sites` through `SaveSettings`), "Hide an element…", the log, and settings.
* **Element picker** (`ui/byobu-picker.js`): injected on request with a one-time token; the
  reply (`byobu:rule:` web message) is accepted only with that token, for the page's own host,
  and only as a plain `host##selector` line appended to `shield.custom_rules`.
* **Request log** (`shoji://byobu`): the newest 3,000 requests Byōbu acted on, with the filter
  line that matched; weekly totals persist in `byobu/stats.json`.
* **Settings**: per-list switches, lists added by URL, "Update now", paused sites and "My rules"
  (uBlock syntax). Changes wake the updater thread, which rebuilds the engine.

