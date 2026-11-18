#!/usr/bin/env bash
# Remove the container started by start.sh.
set -euo pipefail
docker rm -f "${SYNAPSE_CONTAINER:-horo-test-synapse}" >/dev/null 2>&1 || true
