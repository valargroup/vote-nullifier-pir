#!/usr/bin/env bash
# Host-side helper: run the rendered installer/updater inside a clean
# ubuntu:24.04 container (mock systemctl; real binary download from GitHub).
set -euo pipefail

if [ "$#" -lt 1 ] || [ -z "${1:-}" ]; then
  echo "usage: $0 /path/to/rendered-start_pir.sh [/path/to/rendered-update_pir.sh]" >&2
  exit 1
fi

installer="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
if [ ! -f "$installer" ]; then
  echo "not a file: $1" >&2
  exit 1
fi

updater=""
if [ "${2:-}" != "" ]; then
  updater="$(cd "$(dirname "$2")" && pwd)/$(basename "$2")"
  if [ ! -f "$updater" ]; then
    echo "not a file: $2" >&2
    exit 1
  fi
fi

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
inner="${repo_root}/scripts/ci_smoke_start_pir_inner.sh"
service="${repo_root}/docs/nullifier-query-server.service"

docker_args=(
  --rm
  -v "${installer}:/start_pir.sh:ro"
  -v "${inner}:/inner.sh:ro"
  -v "${service}:/release-service:ro"
)
if [ -n "$updater" ]; then
  docker_args+=(-v "${updater}:/update_pir.sh:ro")
fi

docker run "${docker_args[@]}" ubuntu:24.04 bash /inner.sh
