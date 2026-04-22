#!/usr/bin/env bash
# Host-side helper: run the rendered installer inside a clean ubuntu:24.04
# container (mock systemctl; real binary download from GitHub).
set -euo pipefail

if [ "$#" -lt 1 ] || [ -z "${1:-}" ]; then
  echo "usage: $0 /path/to/rendered-start_pir.sh" >&2
  exit 1
fi

installer="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
if [ ! -f "$installer" ]; then
  echo "not a file: $1" >&2
  exit 1
fi

repo_root="$(cd "$(dirname "$0")/.." && pwd)"
inner="${repo_root}/scripts/ci_smoke_start_pir_inner.sh"

docker run --rm \
  -v "${installer}:/start_pir.sh:ro" \
  -v "${inner}:/inner.sh:ro" \
  ubuntu:24.04 \
  bash /inner.sh
