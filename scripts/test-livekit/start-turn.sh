#!/usr/bin/env bash
# A disposable LiveKit with its embedded TURN server on, for horo-rtc's
# relayed-call tests (crates/horo-rtc/tests/call.rs):
#
#   eval "$(scripts/test-livekit/start-turn.sh)"
#   cargo test -p horo-rtc --test call
#
# It prints the environment those tests read. Alongside start.sh's server:
# signalling on 7980, TURN/UDP on 3478, TURN/TLS on 443 (what LiveKit
# advertises; the container listens on 5349).
#
# - The TLS certificate is for `turn.horo.test`, signed by a throwaway CA the
#   tests trust; rustls rightly refuses a self-signed certificate that is its
#   own CA.
# - `node_ip` is the container's own address. With 127.0.0.1 the SFU sends
#   from the container address, which has no TURN permission, and every
#   relayed packet is dropped. It also leaves the SFU reachable only through
#   the relay, which is what these tests are for.
set -euo pipefail

NAME="${LIVEKIT_TURN_CONTAINER:-horo-test-livekit-turn}"
IMAGE="${LIVEKIT_IMAGE:-livekit/livekit-server:v1.9.1}"
# Outside --dev, LiveKit wants a secret of 32 characters or more.
SECRET=horo-test-secret-that-is-at-least-32-characters
STATE="${LIVEKIT_TURN_STATE:-${TMPDIR:-/tmp}/horo-test-livekit-turn}"
mkdir -p "$STATE"
cd "$STATE"

if [ ! -f cert.pem ]; then
  openssl req -x509 -newkey rsa:2048 -nodes -days 30 -subj "/CN=horo test CA" \
    -keyout ca-key.pem -out ca.pem 2>/dev/null
  openssl req -newkey rsa:2048 -nodes -subj "/CN=turn.horo.test" \
    -keyout key.pem -out leaf.csr 2>/dev/null
  printf 'basicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\nsubjectAltName=DNS:turn.horo.test\n' > leaf.ext
  openssl x509 -req -in leaf.csr -CA ca.pem -CAkey ca-key.pem -CAcreateserial \
    -days 30 -extfile leaf.ext -out cert.pem 2>/dev/null
fi

write_config() {
  cat > livekit.yaml <<YAML
port: 7880
bind_addresses: ["0.0.0.0"]
rtc:
  tcp_port: 7881
  udp_port: 7882
  node_ip: $1
  use_external_ip: false
keys:
  devkey: $SECRET
turn:
  enabled: true
  domain: turn.horo.test
  udp_port: 3478
  tls_port: 5349
  cert_file: /lk/cert.pem
  key_file: /lk/key.pem
YAML
}

docker rm -f "$NAME" >/dev/null 2>&1 || true
write_config 127.0.0.1
docker run -d --name "$NAME" -v "$STATE:/lk" \
  -p 7980:7880 -p 3478:3478/udp -p 443:5349 \
  "$IMAGE" --config /lk/livekit.yaml >/dev/null
ip=$(docker inspect -f '{{range .NetworkSettings.Networks}}{{.IPAddress}}{{end}}' "$NAME")
write_config "$ip"
docker restart "$NAME" >/dev/null

for _ in $(seq 1 30); do
  if curl -fsS http://127.0.0.1:7980 >/dev/null 2>&1; then
    echo "export HORO_TEST_LIVEKIT_TURN=ws://127.0.0.1:7980 HORO_TEST_TURN_CA=$STATE/ca.pem"
    exit 0
  fi
  sleep 1
done
echo "livekit (TURN) did not come up; its log:" >&2
docker logs "$NAME" >&2 || true
exit 1
