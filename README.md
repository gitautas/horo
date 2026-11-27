# horo

A native Matrix client for desktop, with end-to-end encryption and native
LiveKit voice/video calls built in. Written in Rust with GPUI.

<img align="center" src="docs/screenshot.png" height="380">

## Getting started
Download the latest build for your platform from [GitHub releases]. Installed
apps on macOS, Windows, and Linux AppImage update themselves; `.deb` installs
notify you when a new version is out.

[GitHub releases]: https://github.com/gitautas/horo/releases/latest

## Developing
```bash
git submodule update --init --depth 1 third_party/libvpx
scripts/desktop-dev.sh           # macOS dev .app bundle (isolated data dir)
cargo run -p horo-desktop        # Linux / Windows dev build
```

```bash
cargo test --workspace           # Rust unit tests; skips homeserver/livekit tests
scripts/test-homeserver/start.sh # a throwaway Synapse in Docker, then:
HORO_TEST_HOMESERVER=http://localhost:8008 cargo test -p horo-core --test homeserver
```

See [docs/releasing.md](docs/releasing.md) for packaging and release
instructions. Tagging `v*` builds every desktop platform and publishes a signed
release using `TAURI_SIGNING_PRIVATE_KEY` (and
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD`) in GitHub Actions secrets.
