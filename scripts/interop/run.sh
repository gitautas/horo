#!/usr/bin/env bash
# horo's call engine and livekit-client in one room, each sending a tone the
# other has to hear. Needs scripts/test-livekit/start.sh running and the
# repo's node_modules (livekit-client).
#
#   scripts/interop/run.sh [seconds]
#
# Serves the browser half on http://localhost:8765 and prints the URL to
# open; press Start there, then this runs the horo half (440 Hz out,
# listening for the browser's 660 Hz).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
seconds="${1:-30}"
room="interop-$(date +%s)"
url="ws://127.0.0.1:7880"

site="$(mktemp -d)"
trap 'kill "${server:-0}" 2>/dev/null || true; rm -rf "$site"' EXIT
cp "$here/index.html" "$site/"
cp "$root/node_modules/livekit-client/dist/livekit-client.esm.mjs" "$site/"
python3 -m http.server 8765 --bind 127.0.0.1 --directory "$site" >/dev/null 2>&1 &
server=$!

browser_token="$(python3 -I "$here/token.py" browser "$room")"
echo "open: http://localhost:8765/index.html?url=$url&hz=660&token=$browser_token"
read -r -p "press Start in the page, then Enter here "
cargo run -q -p horo-call --example interop -- "$url" "$(python3 -I "$here/token.py" horo "$room")" 440 660 "$seconds"
