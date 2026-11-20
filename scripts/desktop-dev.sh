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
