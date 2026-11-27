#!/usr/bin/env bash
# Build the gpui desktop app and wrap it in a .app bundle, so macOS treats it
# as an app (Dock icon, window list, screenshots), then open it.
#
#   scripts/desktop-dev.sh [--release] [--no-open]
#
# HORO_DATA points it at another data directory (its session and stores);
# by default the dev build's own, never the installed app's.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
profile=debug
flags=()
open_it=1
for arg in "$@"; do
  case "$arg" in
    --release) profile=release; flags+=(--release) ;;
    --no-open) open_it=0 ;;
  esac
done

cargo build -p horo-desktop ${flags[@]+"${flags[@]}"} --manifest-path "$root/Cargo.toml"

app="$root/target/$profile/horo-dev.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
# A copy still running would only be brought forward by `open`, on the old
# build: quit it first, the way the app quits itself (a kill makes macOS
# offer to reopen its windows next time).
# Matched by the bundle's tail, not its full path: a copy started by hand
# (`horo-dev.app/Contents/MacOS/horo > log`) has a relative one. Two copies
# share one session and device, and kick each other out of calls.
running="horo-dev.app/Contents/MacOS/horo"
for _ in 1 2 3; do
  pgrep -f "$running" >/dev/null || break
  osascript -e 'tell application id "lt.uwu.horo.dev" to quit' >/dev/null 2>&1 || true
  for _ in $(seq 40); do pgrep -f "$running" >/dev/null || break; sleep 0.25; done
done
# Replace the binary rather than write over it: macOS caches a binary's
# code signature by file, and launching a rewritten file under the old
# cached signature fails with "Launchd job spawn failed" (POSIX 162).
rm -f "$app/Contents/MacOS/horo"
cp "$root/target/$profile/horo" "$app/Contents/MacOS/horo"
cp "$root/apps/desktop/packaging/icons/icon.icns" "$app/Contents/Resources/icon.icns"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>lt.uwu.horo.dev</string>
  <key>CFBundleName</key><string>horo (gpui dev)</string>
  <key>CFBundleExecutable</key><string>horo</string>
  <key>CFBundleIconFile</key><string>icon.icns</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.0.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>horo uses the microphone in calls.</string>
  <key>NSCameraUsageDescription</key><string>horo uses the camera in video calls.</string>
</dict></plist>
PLIST
touch "$app"
echo "$app"
# A dev build never runs on the installed app's data (or its keychain
# entry): its own directory unless told otherwise.
HORO_DATA="${HORO_DATA:-$HOME/Library/Application Support/lt.uwu.horo}"
if [ "$open_it" = 1 ]; then
  open -n "$app" --env "HORO_DATA=$HORO_DATA"
fi
