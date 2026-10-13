# Dark Reader in Shoji

This is Dark Reader's source with four Shoji changes. Shoji uses it to darken websites
when "Dark websites" is on.

* `src/defaults.ts` — the dark palette is Shoji's Sumi ink (`#1b1a18` / `#ebe6dc`).
* `src/api/shoji.ts` (new) — `DarkReader.shoji({on, exclude})`: the controller the browser
  calls. It skips Shoji's own pages and user-excluded sites, and uses Dark Reader's detector
  to leave sites that already have a native dark theme alone.
* `src/api/index.ts` — exports the controller.
* `src/inject/detector.ts` — finds Dark Reader's own styles by element, so the dark-theme
  detector can set them aside in API mode (where `document.styleSheets` hides them).

Rebuild the embedded bundle after changing anything here:

```powershell
cd third_party/darkreader
npm ci --ignore-scripts
npm run api
npx -y terser@5.36.0 darkreader.js -c passes=2 -m --comments "/Dark Reader v/" -o ../../crates/app/ui/vendor/darkreader.min.js
```

Dark Reader is MIT-licensed; its license is in `LICENSE` here and next to the bundle.
