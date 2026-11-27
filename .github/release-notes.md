## What's new

### Native desktop app (GPUI)

horo is now a native Rust desktop app built with GPUI — no WebView, lower memory
use, faster startup, and the same Matrix E2EE and UwU design system. Existing
installs upgrade in place and keep their session, encryption keys, and settings.

### Native voice & video calls everywhere

Voice and video calls now run on horo's own native MatrixRTC + LiveKit media
engine (`str0m`, Opus, `sonora` echo cancellation/noise suppression, VP8),
including on Linux where WebKitGTK previously couldn't place calls at all.

### Polish

- Floating modal settings window with sidebar navigation.
- Collapsible right-hand room info sidebar (click `✕` to close, or click the
  room/DM avatar in the top bar to reopen).
- Cleaner attachment previews, video thumbnails, and auto-dismissing call
  notices.

<!-- Prepend the changelog for this release above this line before publishing. -->

## Updating

The macOS, Windows and AppImage builds update themselves: they check on launch
and offer the new version in-app. A `.deb` is owned by your package manager
rather than by horo, so those builds tell you a release is out and send you back
here instead.

This release is **desktop only** (macOS, Windows, Linux).

## Installing

These builds are **not code-signed** yet, so each OS warns on first launch. This is
expected; here is how to get past it.

### macOS — `.dmg` (Apple Silicon: `aarch64`, Intel: `x64`)

The app is ad-hoc signed but not notarized. On first launch macOS will refuse to
open it ("Apple could not verify…"). Either:

1. Open **System Settings → Privacy & Security**, scroll down, and click
   **Open Anyway** (macOS 15+ removed the old right-click → Open shortcut), or
2. clear the quarantine flag in a terminal: `xattr -cr /Applications/horo.app`

### Windows — `-setup.exe` (x64)

SmartScreen will show "Windows protected your PC". Click **More info → Run anyway**.
The `-setup.exe` (NSIS) installer installs per-user into `%LOCALAPPDATA%\horo`
and needs no admin rights. On Windows-on-ARM devices, use the x64 build — it
runs under emulation.

### Linux — `.deb` / `.AppImage` (x86_64 `amd64` and arm64 `aarch64`)

- **AppImage:** `chmod +x horo_*.AppImage`, then run it. No installation needed.
- **deb:** requires Ubuntu 22.04+ / Debian 12+:
  `sudo apt install ./horo_*.deb`
- Known gap: **voice calls do not work on Linux yet** (WebKitGTK's WebRTC support
  is not enabled by the app). Everything else, including E2EE messaging, works.
