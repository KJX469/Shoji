# Contributing to Shoji

Thanks for taking a look. Shoji is a small, deliberately quiet browser, so the bar for new
features is "does this belong in a paper-thin shell?" Bug fixes, polish and tests are always welcome.
For anything larger, open an issue first so we can agree on the shape before you spend time on it.

## Set up

You need Windows 10/11 x64, the Rust MSVC toolchain, and the Visual Studio C++ build tools.

```powershell
git clone https://github.com/KJX469/Shoji.git
cd shoji
cargo run -p shoji                 # run the browser (debug)
cargo test --workspace             # unit, security and UI-template tests
```

To look at every internal page in an ordinary browser, without launching Shoji:

```powershell
$env:SHOJI_DUMP = "$PWD\target\pages"; cargo test -p shoji dump_pages -- --ignored
```

[docs/architecture.md](docs/architecture.md) explains how the crates and pages fit together.

## Workflow

`main` is protected: nobody pushes to it directly. Branch, push the branch, and open a pull
request; the maintainer reviews and merges it. Releases (`v*` tags) are cut by the maintainer only.

```powershell
git pull
git checkout -b my-change
# edit, commit
git push -u origin my-change
```

## Before you open a pull request

- `cargo fmt --all` leaves no changes, and `cargo clippy --workspace --all-targets` shows no new
  warnings.
- `cargo test --workspace` passes. Add a test for behaviour you change where one is practical.
- The change is focused. Unrelated clean-ups belong in their own pull request.
- Visual changes include a before/after screenshot. The look is intentional (calm, paper and ink,
  nothing loud), so say why a change fits it.
- You have not added a dependency without a reason. Binary size is a feature.
- Nothing personal is committed: no profiles, logs, settings files or local paths.

## What fits

- **Fits:** correctness, performance and memory, accessibility, Windows integration, the Byōbu
  shield's matching and interface, installer polish, documentation.
- **Needs discussion first:** new internal pages, new settings, anything that talks to the network
  on its own, anything that changes the visual identity.
- **Does not fit:** accounts or sync services, telemetry of any kind, bundling a browser engine.

## Byōbu and upstream data

Filter lists are fetched at runtime and are never committed. uBlock Origin's scriptlets are GPLv3
and are assembled by the `byobu-resources` workflow into a separate release asset; they must not
be compiled into the browser binary, which is MIT. `scripts/byobu-sync.ps1` reports when the
upstream engines move.

## Dark Reader

`third_party/darkreader` is Dark Reader's source with a few Shoji changes, listed in
`third_party/darkreader/SHOJI.md`. Keep changes there minimal, and rebuild the bundle in
`crates/app/ui/vendor/` as described in that file.

## Licence

By contributing you agree that your work is released under the [MIT licence](LICENSE).
