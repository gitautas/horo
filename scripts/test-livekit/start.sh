#!/usr/bin/env bash
# Start a disposable LiveKit server for horo-livekit's integration tests:
#
#   scripts/test-livekit/start.sh
#   HORO_TEST_LIVEKIT=ws://127.0.0.1:7880 cargo test -p horo-livekit --test livekit
#
# `--dev` mode: API key `devkey`, secret `secret`, no TURN. `stop.sh` removes it.
set -euo pipefail

IMAGE="${LIVEKIT_IMAGE:-livekit/livekit-server:v1.9.1}"
NAME="${LIVEKIT_CONTAINER:-horo-test-livekit}"

docker rm -f "$NAME" >/dev/null 2>&1 || true
docker run -d --name "$NAME" \
  -p 7880:7880 -p 7881:7881 -p 7882:7882/udp \
  "$IMAGE" --dev --bind 0.0.0.0 --node-ip 127.0.0.1 >/dev/null

for _ in $(seq 1 30); do
  if curl -fsS http://127.0.0.1:7880 >/dev/null 2>&1; then
    echo "livekit is up at ws://127.0.0.1:7880"
    exit 0
  fi
  sleep 1
done
echo "livekit did not come up; its log:" >&2
docker logs "$NAME" >&2 || true
exit 1
