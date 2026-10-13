# Shoji — working rules

- `installer/` (gitignored) holds the latest `ShojiSetup.exe`; refresh it from the newest GitHub release.
- **Releases:** after a meaningful batch of changes, bump the version in every `crates/*/Cargo.toml`,
  add a terse `CHANGELOG.md` entry (one line per change, no filler), run
  `cargo test -p shoji-core -p shoji-byobu -p shoji-brand`, commit, push, then
  `git tag vX.Y.Z && git push origin vX.Y.Z`. The `release` workflow builds the installer and
  publishes the release with the notes from that CHANGELOG section. Not every turn.
- **Never commit:** `target/`, `dist/`, `installer/`, `*.exe`, secrets, editor/tool files (see `.gitignore`).
- The app is Windows-only; only the portable crates can be tested on Linux.
- **Git identity:** commit and push as the user (`KJX469`), never as Claude: no Claude author and no
  Co-Authored-By/attribution trailers. If pushing as the user isn't possible, don't push.
