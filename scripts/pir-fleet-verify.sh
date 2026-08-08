#!/usr/bin/env bash
# Post-deploy checks: env file uses SVOTE_PIR_* for bootstrap URLs and
# localhost endpoints show Ironwood dataset identity and snapshot heights.
#
# Usage:
#   ./scripts/pir-fleet-verify.sh BACKUP_SSH_TARGET PRIMARY_SSH_TARGET
set -euo pipefail

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" || $# -lt 2 ]]; then
  echo "Usage: $0 <backup_ssh_target> <primary_ssh_target>" >&2
  exit 1
fi

BACKUP="$1"
PRIMARY="$2"

verify_one() {
  local label="$1"
  local host="$2"
  echo "==> Verifying ${label} (${host})"
  ssh -o BatchMode=yes "$host" 'set -euo pipefail
    echo "--- /etc/default/nf-server (bootstrap lines) ---"
    if [[ -f /etc/default/nf-server ]]; then
      grep -E "^SVOTE_" /etc/default/nf-server || true
      if grep -q "^SVOTE_VOTING_CONFIG_URL=" /etc/default/nf-server 2>/dev/null; then
        echo "WARN: legacy SVOTE_VOTING_CONFIG_URL still present (ignored by new nf-server)" >&2
      fi
      if grep -q "^SVOTE_PRECOMPUTED_BASE_URL=" /etc/default/nf-server 2>/dev/null; then
        echo "WARN: legacy SVOTE_PRECOMPUTED_BASE_URL still present (ignored by new nf-server)" >&2
      fi
      NETWORK=$(grep -E "^SVOTE_ZCASH_NETWORK=(main|test)$" /etc/default/nf-server | tail -1 | cut -d= -f2- || true)
      if [[ -z "$NETWORK" ]]; then
        echo "FAIL: SVOTE_ZCASH_NETWORK must be main or test" >&2
        exit 1
      fi
      grep -Fxq "SVOTE_PIR_DATA_DIR=/opt/nf-ingest/pir-data/${NETWORK}" /etc/default/nf-server || {
        echo "FAIL: SVOTE_PIR_DATA_DIR is not namespaced for ${NETWORK}" >&2
        exit 1
      }
    else
      echo "FAIL: no /etc/default/nf-server" >&2
      exit 1
    fi
    echo "--- /ready ---"
    curl -sfS --max-time 5 http://127.0.0.1:3000/ready >/dev/null && echo OK || { echo "FAIL: /ready" >&2; exit 1; }
    echo "--- /root ---"
    ROOT=$(curl -sfS --max-time 5 http://127.0.0.1:3000/root)
    echo "$ROOT"
    grep -Eq "\"nullifier_pool\"[[:space:]]*:[[:space:]]*\"ironwood\"" <<<"$ROOT" || { echo "FAIL: /root nullifier_pool" >&2; exit 1; }
    grep -Eq "\"zcash_network\"[[:space:]]*:[[:space:]]*\"${NETWORK}\"" <<<"$ROOT" || { echo "FAIL: /root zcash_network" >&2; exit 1; }
    grep -Eq "\"dataset_version\"[[:space:]]*:[[:space:]]*2[[:space:]]*([,}])" <<<"$ROOT" || { echo "FAIL: /root dataset_version" >&2; exit 1; }
    grep -Eq "\"pir_depth\"[[:space:]]*:[[:space:]]*19[[:space:]]*([,}])" <<<"$ROOT" || { echo "FAIL: /root pir_depth" >&2; exit 1; }
    grep -Eq "\"tier1_rows\"[[:space:]]*:[[:space:]]*4096[[:space:]]*([,}])" <<<"$ROOT" || { echo "FAIL: /root tier1_rows" >&2; exit 1; }
    grep -Eq "\"tier1_row_bytes\"[[:space:]]*:[[:space:]]*12288[[:space:]]*([,}])" <<<"$ROOT" || { echo "FAIL: /root tier1_row_bytes" >&2; exit 1; }
    echo "--- metrics heights ---"
    curl -sfS --max-time 5 http://127.0.0.1:3000/metrics | awk "\$1==\"nf_snapshot_served_height\" || \$1==\"nf_snapshot_expected_height\" {print}"
  '
}

verify_one "backup" "$BACKUP"
verify_one "primary" "$PRIMARY"
echo "Done."
