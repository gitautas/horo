#!/usr/bin/env bash
# Build the gpui desktop app and wrap it in a .app bundle, so macOS treats it
# as an app (Dock icon, window list, screenshots), then open it.
#
#   scripts/desktop-dev.sh [--release] [--no-open]
#
# HORO_DATA points it at another data directory (its session and stores).
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

app="$root/src-tauri/target/$profile/horo-dev.app"
mkdir -p "$app/Contents/MacOS"
# A copy still running would only be brought forward by `open`, on the old
# build: quit it first, the way the app quits itself (a kill makes macOS
# offer to reopen its windows next time).
if pgrep -f "$app/Contents/MacOS/horo" >/dev/null; then
  osascript -e 'tell application id "lt.uwu.horo.dev" to quit' >/dev/null 2>&1 || true
  for _ in $(seq 40); do pgrep -f "$app/Contents/MacOS/horo" >/dev/null || break; sleep 0.25; done
fi
# Replace the binary rather than write over it: macOS caches a binary's
# code signature by file, and launching a rewritten file under the old
# cached signature fails with "Launchd job spawn failed" (POSIX 162).
rm -f "$app/Contents/MacOS/horo"
cp "$root/src-tauri/target/$profile/horo" "$app/Contents/MacOS/horo"
cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleIdentifier</key><string>lt.uwu.horo.dev</string>
  <key>CFBundleName</key><string>horo (gpui dev)</string>
  <key>CFBundleExecutable</key><string>horo</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.0.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSMicrophoneUsageDescription</key><string>horo uses the microphone in calls.</string>
  <key>NSCameraUsageDescription</key><string>horo uses the camera in video calls.</string>
</dict></plist>
PLIST
echo "$app"
if [ "$open_it" = 1 ]; then
  if [ -n "${HORO_DATA:-}" ]; then
    open -n "$app" --env "HORO_DATA=$HORO_DATA"
  else
    open -n "$app"
  fi
fi
