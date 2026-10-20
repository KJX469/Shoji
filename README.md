<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/assets/banner-dark.svg">
  <img src="docs/assets/banner-light.svg" alt="Shoji — a paper-thin browser for Windows" width="100%">
</picture>

<br>

[![Latest release](https://img.shields.io/github/v/release/KJX469/Shoji?style=flat-square&color=397AFF&label=release)](../../releases/latest)
![Platform](https://img.shields.io/badge/Windows-10%20%7C%2011-1b1a18?style=flat-square)
[![MIT](https://img.shields.io/badge/license-MIT-21b86e?style=flat-square)](LICENSE)

**障子** · A browser that's mostly empty space.

[**Download**](../../releases/latest)

</div>

<br>

<p align="center">
  <img src="docs/assets/browser-dark.png" width="49%" alt="Shoji's new tab in the dark theme">
  <img src="docs/assets/browser-light.png" width="49%" alt="Shoji showing a page in the light theme">
</p>

## The idea

A *shoji* is the sliding paper screen of a Japanese room: a thin frame that lets light through
and keeps very little for itself. Shoji the browser works the same way. It doesn't ship a
browser engine. It uses the Microsoft Edge WebView2 runtime that Windows 10 and 11 already
install and patch, and adds only what a browser needs around it — tabs, an address bar, a
side panel, settings — drawn in a calm paper-and-ink interface.

## Almost nothing

- **~6 MB.** Fonts and ad blocker included.
- **No account, no sync, no telemetry.**
- **Ads and trackers gone** from the first launch, tracking links cleaned, Global Privacy Control on.
- **Dark mode for every website**, tuned like the extension: presets, brightness, contrast,
  colors, fonts, a schedule, and a look of its own for any site.
- **Bookmarks that behave.** A bookmarks bar (Ctrl+Shift+B), a bookmark manager (Ctrl+Shift+O),
  folders, and imports from Chrome, Edge, Brave and Firefox.
- **Split view and pinned tabs.** Put tabs side by side in named split groups (Ctrl+Shift+S), pin the
  ones you keep, and drag a tab out into its own window.
- **History, downloads and the usual shortcuts** (Ctrl+H, Ctrl+J, Ctrl+Shift+Del) where you expect them.
- **Nothing resizes the page.** Menus and panels float over it.
- **Private windows** (Ctrl+Shift+N) keep no cookies, history or logins.
- **Extensions (experimental):** add a Chromium extension from its Chrome Web Store link.
- A few small things aren't listed here. Have fun finding them.

## Install

Download **`ShojiSetup.exe`** from the [latest release](../../releases/latest). It installs for your
user only — no administrator prompt. When a new version is out, Shoji tells you and asks before
it updates; nothing downloads until you choose.

To check the download, compare its hash with `ShojiSetup.exe.sha256` from the same release:

```powershell
(Get-FileHash .\ShojiSetup.exe -Algorithm SHA256).Hash
```

> Shoji isn't code-signed yet, so SmartScreen may say the app is unrecognized. Choose
> **More info → Run anyway**.

## About this repository

Shoji's source code isn't published here. This repository hosts the installer
([releases](../../releases)), the [website](https://kjx469.github.io/Shoji/), the
[changelog](CHANGELOG.md) and the filter data Byōbu downloads. Found a bug? [Open an
issue](../../issues/new/choose).

## License

[MIT](LICENSE). Built on Microsoft WebView2, [Dark Reader](https://github.com/darkreader/darkreader),
Brave's [adblock-rust](https://github.com/brave/adblock-rust) and uBlock Origin's filter lists; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).

Browser names and logos shown when importing data (Chrome, Edge, Firefox, Brave and others) are
trademarks of their owners. Shoji is not affiliated with or endorsed by them; the logos only
identify the browser you are importing from.
