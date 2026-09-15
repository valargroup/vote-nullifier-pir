#!/usr/bin/env bash
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
image="pir-updater-systemd-test:local"
docker build -q -t "$image" -f "$root/deploy/pir-updater/tests/Dockerfile" "$root/deploy/pir-updater"
container=$(docker run -d --privileged --cgroupns=private --tmpfs /run --tmpfs /run/lock --tmpfs /tmp "$image")
trap 'docker rm -f "$container" >/dev/null' EXIT
for attempt in $(seq 1 30); do
  if docker exec "$container" test -d /run/systemd/system; then break; fi
  sleep 1
done
docker exec "$container" python3 /opt/pir-updater/tests/systemd_test.py
