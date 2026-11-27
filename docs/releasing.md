# Releasing horo

horo ships as the gpui desktop app (`apps/desktop`), packaged by
[cargo-packager](https://github.com/crabnebula-dev/cargo-packager).

## Cutting a release

1. Bump `version` under `[workspace.package]` in the root `Cargo.toml` and
   commit.
2. Tag it `v<version>` and push the tag.

`.github/workflows/release.yml` then builds every platform, attaches the
bundles to a draft release, writes `latest.json` from what actually got
attached, and publishes. A manual run (`workflow_dispatch`) builds the same
matrix but only uploads the bundles as run artifacts.

| Platform | What users download | What the updater installs |
|---|---|---|
| macOS (Apple Silicon, Intel) | `.dmg` | `.app.tar.gz` |
| Windows x64 | `-setup.exe` (NSIS, per user) | the same installer |
| Linux x64, arm64 | `.AppImage`, `.deb` | the `.AppImage` |

A `.deb` install doesn't update itself: the app links to the release page.

## Updates

Installed copies poll
`https://github.com/gitautas/horo/releases/latest/download/latest.json`
every few hours. Each entry there names a bundle and its minisign
signature; the app installs only a bundle that verifies against
`horo_core::update::PUBKEY` (see `crates/horo-ui/src/updater.rs`).

The private half is the `TAURI_SIGNING_PRIVATE_KEY` repository secret (and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`, if it has one). The names are from the
Tauri app, whose key this still is: lose it and no installed copy can ever
update again; leak it and anyone can push an update to every user.

### Installs of the Tauri app

The Tauri app polls the same address with the same key. `latest.json` lists
each macOS bundle under both updaters' names (`darwin-*` for Tauri,
`macos-*` for cargo-packager-updater; Linux and Windows are spelled alike),
so a Tauri install updates straight into the gpui app. The gpui app keeps
the Tauri app's identifier (`lt.uwu.uwum`), data directory and keychain
entry, so it starts signed in, with the same keys.

## Building locally

```bash
cargo install cargo-packager --version 0.11.8 --locked
cargo build --release -p horo-desktop
cargo packager --release -p horo-desktop --formats app,dmg
```

The bundles land in the Cargo target directory under `release/`. Add
`--target <triple>` to both commands to cross-build. Without a signing key
in `CARGO_PACKAGER_SIGN_PRIVATE_KEY` there's no `.sig` and no
`.app.tar.gz`, which only the updater needs.

For day-to-day work use `scripts/desktop-dev.sh` instead: a debug build with
its own bundle identifier and data directory, so it never touches an
installed copy.
