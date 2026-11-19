#!/usr/bin/env bash
# Remove the containers started by start.sh and start-turn.sh.
set -euo pipefail
docker rm -f "${LIVEKIT_CONTAINER:-horo-test-livekit}" "${LIVEKIT_TURN_CONTAINER:-horo-test-livekit-turn}" >/dev/null 2>&1 || true
