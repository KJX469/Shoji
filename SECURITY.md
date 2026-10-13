# Security policy

## Reporting a vulnerability

Please **do not open a public issue** for a security problem.

Use GitHub's private reporting instead:
[**Report a vulnerability**](https://github.com/KJX469/Shoji/security/advisories/new)
(the *Security* tab → *Report a vulnerability*).

Include what you found, how to reproduce it, the Shoji version (from the file properties of
`shoji.exe`) and your Windows version. You should get an acknowledgement within
a few days. Fixes are released as soon as they are ready, and you are credited unless you would
rather not be.

## Supported versions

Only the latest release is supported. Shoji has no background updater yet, so please install the
newest `ShojiSetup.exe` from the [releases page](https://github.com/KJX469/Shoji/releases/latest).

## Automatic updates

Shoji downloads new releases from this repository's GitHub releases only, checks the installer
against the SHA-256 published in the same release, and refuses anything else. Installers are not
yet code-signed, so the checksum guards against corruption, not against a compromised account.
Updates can be turned off in Settings.

## Scope

Shoji is a shell around the Microsoft Edge WebView2 runtime, which Windows updates on its own.

| In scope | Out of scope |
| --- | --- |
| Shoji's own pages and IPC (web content reaching privileged browser actions) | Vulnerabilities in WebView2 / Chromium itself (report to Microsoft) |
| The installer and uninstaller (paths, registry entries, elevation) | Vulnerabilities in third-party filter lists |
| Byōbu's rule, scriptlet and element-picker handling | Issues that require an already-compromised machine |
| Data written to disk (settings, notes, profile location) | Missing code signing (known; SmartScreen warns) |

## Design notes that matter for review

- The browser refuses to run elevated.
- Web pages may send the host only a small allowlist of messages; everything else is accepted only
  from Shoji's own `shoji://` pages.
- Element-picker replies are authorised by a one-time token and only for the page's own host.
- Filter lists and resources are fetched from fixed upstream URLs over HTTPS.
