#!/usr/bin/env bash
# Start a disposable Synapse for horo-core's integration tests:
#
#   scripts/test-homeserver/start.sh
#   HORO_TEST_HOMESERVER=http://localhost:8008 cargo test -p horo-core --test homeserver
#
# The container is recreated from scratch every time, so each run starts with
# no users and no rooms. `stop.sh` removes it.
set -euo pipefail

IMAGE="${SYNAPSE_IMAGE:-matrixdotorg/synapse:v1.135.0}"
NAME="${SYNAPSE_CONTAINER:-horo-test-synapse}"
PORT="${SYNAPSE_PORT:-8008}"
HERE="$(cd "$(dirname "$0")" && pwd)"
DATA="$(mktemp -d "${TMPDIR:-/tmp}/horo-synapse.XXXXXX")"

docker rm -f "$NAME" >/dev/null 2>&1 || true

# Run as the calling user, so the generated config can be appended to here.
docker run --rm -v "$DATA:/data" \
  -e SYNAPSE_SERVER_NAME=localhost -e SYNAPSE_REPORT_STATS=no \
  -e UID="$(id -u)" -e GID="$(id -g)" \
  "$IMAGE" generate >/dev/null
cat "$HERE/overrides.yaml" >> "$DATA/homeserver.yaml"

docker run -d --name "$NAME" -p "$PORT:8008" -v "$DATA:/data" \
  -e UID="$(id -u)" -e GID="$(id -g)" \
  "$IMAGE" >/dev/null

for _ in $(seq 1 60); do
  if curl -fsS "http://localhost:$PORT/health" >/dev/null 2>&1; then
    echo "synapse is up at http://localhost:$PORT (data in $DATA)"
    exit 0
  fi
  sleep 1
done

echo "synapse did not come up; its log:" >&2
docker logs "$NAME" >&2 || true
exit 1
