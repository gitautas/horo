# horo

A Matrix client with end-to-end encryption and LiveKit voice, in the UwU design
system. Tauri 2 + Rust backend, React + TypeScript frontend.

```bash
npm install
npm run app          # dev, with hot reload
npm run app:build    # release .app / .dmg

npm run android         # dev build on a connected device
npm run android:build -- --apk   # release .apk (--aab for Play)

npm run ios             # dev build on a connected device or simulator
npm run ios:build       # release .ipa
```

macOS and iOS build today, and Android runs on a device. Linux and Windows are
kept viable — no macOS-only crates, and both custom-protocol URL forms are
handled — but neither is tested yet.

**Voice does not work on Linux**, and not for a reason horo can fix. The
WebView there is WebKitGTK, whose `ENABLE_WEB_RTC` follows
`ENABLE_EXPERIMENTAL_FEATURES` — off — and no distribution overrides it, so
`RTCPeerConnection` does not exist however the `enable-webrtc` setting reads
back. Everything else works; joining a call says so rather than failing
obscurely.

A WebKitGTK built with WebRTC in it is the only thing that could change that,
and **nobody has got one working yet** — the route below is where to start, not
a recipe known to arrive. On Arch, rebuild the distribution package with two
flags added to `cmake_options`:

```bash
git clone https://gitlab.archlinux.org/archlinux/packaging/packages/webkit2gtk-4.1
cd webkit2gtk-4.1
# in cmake_options: -D ENABLE_WEB_RTC=ON  -D USE_LIBRICE=OFF
makepkg -si
```

`USE_LIBRICE` defaults to `ON` in 2.52 and no distribution packages librice yet,
so cmake stops on it; `OFF` puts ICE back on libnice. `USE_GSTREAMER_WEBRTC` is
already `ON` by default, and OpenSSL 3 and `gstreamer-webrtc-1.0` (from
gst-plugins-bad) are the only other build-time requirements. Expect hours and
tens of gigabytes, and an `IgnorePkg` entry so the next `-Syu` doesn't undo it.

That build then has to be the one the app actually loads, which the **AppImage
is not** — linuxdeploy bundles a WebKitGTK and its `WebKitWebProcess` inside, so
the AppImage ignores whatever is installed on the system. Run a locally built
horo (`npm run app`, or `npm run app:build` on the patched machine) to get the
system WebKitGTK.

An attempt at exactly this on Arch with 2.52.6 did not end in a working call,
and why is not yet established — so treat the flags as researched rather than
proven, and check `typeof RTCPeerConnection` in the rebuilt WebKitGTK before
blaming anything in this repository.

Android needs an SDK, an NDK and a JDK. `npm run android` finds all three
itself if they are installed; from nothing, that is:

```bash
brew install --cask android-commandlinetools temurin
sdkmanager --licenses
sdkmanager platform-tools "platforms;android-36" "build-tools;36.0.0" "ndk;27.3.13750724"
rustup target add aarch64-linux-android armv7-linux-androideabi i686-linux-android x86_64-linux-android
```

## Updates

Every `v*` tag produces one release that serves both halves of distribution: the
installers people download for the first time, and `latest.json` — the manifest
the installed app polls. They are built from the same run, and the release only
leaves draft once every platform's assets are attached, so the app can never be
offered a version that isn't fully published.

The app can only *apply* an update where it owns the install: macOS, Windows,
and Linux via AppImage. A `.deb`, an `.apk` and an `.ipa` belong to apt or to
the phone, so those builds say a new version exists and open the release page
instead. Which of the two applies is decided in `src-tauri/src/update.rs`, not
guessed in the frontend — on Linux the same binary ships both ways.

Updates are signed, and the app installs nothing the key below did not sign. The
public half lives in `tauri.conf.json` (`plugins.updater.pubkey`); the private
half must exist in the repository's Actions secrets, or a tag fails immediately:

| Secret | What it is |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | the contents of the `.key` from `tauri signer generate` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | the password set when generating it |

Like the Android keystore, this key cannot be rotated freely: an app only
accepts updates signed by the key **it** was built with, so a new key strands
every copy already installed until its owner downloads a build by hand. Keep the
private key and its password somewhere they outlive the machine that made them.

```bash
npm run tauri signer generate -- -w ~/.tauri/horo-updater.key
```

## What works

Login (password and SSO), session persistence in the OS keychain, sliding sync,
room list with spaces and favourites, timeline with replies, reactions, threads,
edits, redactions, typing, read receipts and file upload, device verification,
voice calls over MatrixRTC, and profiles — bio, status and cover photo, with a
card behind every avatar.

## Where to look next

- [ARCHITECTURE.md](ARCHITECTURE.md) — how it's put together, and the traps that
  cost hours to find. Read the traps before changing anything in the timeline or
  media paths.
- [DESIGN.md](DESIGN.md) — the visual spec: tokens, voice, motion, and the rules
  the UI is meant to hold to.
- [PLAN.md](PLAN.md) — features researched but not built: custom emoji and
  stickers, room backgrounds. Event shapes and traps included.

---

*This file is a placeholder — the two documents above carry the real content.*
