#!/usr/bin/env bash
# Remove the container started by start.sh.
set -euo pipefail
docker rm -f "${LIVEKIT_CONTAINER:-horo-test-livekit}" >/dev/null 2>&1 || true
