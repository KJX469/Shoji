# Runtime verification log

What has been exercised in a running build (an isolated portable build with its own profile, driven over the
DevTools protocol), as opposed to only compiled and unit-tested. Updated 2026-10-16 (v0.1.4 + unreleased work).

| Feature | Status |
| --- | --- |
| Pinned tabs: pin from the tab menu, icon-only width, order, restore after a force close | Verified |
| Sound mark on a tab playing audio; mute from the tab | Verified (state and class; the audio itself was not listened to) |
| Extensions: add an unpacked folder, content script runs, survives restart, listed and removable in Settings | Verified |
| Tab groups: new group from the tab menu, name prompt, chip | Verified |
| Reader view on a real article page | Verified |
| Picture-in-picture, enter and leave | Verified |
| Frosted toolbar: switch applies on restart, transparent page and translucent bar | Verified in CSS and settings; the Mica backdrop itself was not seen |
| Favourite folders: import, filing, export | Unit-tested; the new tab page grouping and prompt were not driven |
| Site choices list in Settings | Unit-tested; the Settings UI was not driven |
| Block third-party cookies switch | Wired to the engine flag; its effect was not measured |
| Clear browsing data keeps data older than the chosen range | Not tested (needs older data); documented engine behaviour |
| Byobu scriptlets | The published `ubo-resources.json` has 200 resources including `json-prune` and `trusted-replace-xhr-response`; ad blocking on live YouTube was not measured |
| `edge://settings/passwords` | Nothing in the code refers to it; the built-in vault replaces it |
