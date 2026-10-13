## What's new

### Pick your font, and your text size

*Settings → appearance* now has a font picker: Nunito (the default), the system
font, Baloo, a monospace option, or the name of any font installed on your
machine. A misspelt or missing font falls back to Nunito rather than Times.

There's also a slider for message text size, from 12 to 22px. It resizes
messages and the composer, and leaves the rest of the app alone.

### Transparent images look like stickers

A PNG, WebP or GIF that's mostly see-through, like a character cut out of its
background, now sits on the timeline without a card, a border or a dark
background behind it. This works whichever app it was sent from.

### Changed

- The *send with enter* toggle is gone. Enter always sends, and Shift+Enter
  starts a new line.

<!-- Prepend the changelog for this release above this line before publishing. -->

## Updating

The macOS, Windows and AppImage builds update themselves: they check on launch
and offer the new version in-app. A `.deb` is owned by your package manager
rather than by horo, so those builds tell you a release is out and send you back
here instead.

This release is **desktop only** — the Android and iOS builds are not signed
yet, so no `.apk` or `.ipa` is attached.

## Installing

These builds are **not code-signed** yet, so each OS warns on first launch. This is
expected; here is how to get past it.

### macOS — `.dmg` (Apple Silicon: `aarch64`, Intel: `x64`)

The app is ad-hoc signed but not notarized. On first launch macOS will refuse to
open it ("Apple could not verify…"). Either:

1. Open **System Settings → Privacy & Security**, scroll down, and click
   **Open Anyway** (macOS 15+ removed the old right-click → Open shortcut), or
2. clear the quarantine flag in a terminal: `xattr -cr /Applications/horo.app`

### Windows — `.msi` / `-setup.exe` (x64)

SmartScreen will show "Windows protected your PC". Click **More info → Run anyway**.
The `-setup.exe` (NSIS) installer installs per-user and needs no admin rights; the
`.msi` installs per-machine. On Windows-on-ARM devices, use the x64 build — it runs
under emulation.

### Linux — `.deb` / `.AppImage` (x86_64 `amd64` and arm64 `aarch64`)

- **AppImage:** `chmod +x horo_*.AppImage`, then run it. No installation needed.
- **deb:** requires Ubuntu 22.04+ / Debian 12+ (WebKitGTK 4.1):
  `sudo apt install ./horo_*.deb`
- Known gap: **voice calls do not work on Linux yet** (WebKitGTK's WebRTC support
  is not enabled by the app). Everything else, including E2EE messaging, works.
