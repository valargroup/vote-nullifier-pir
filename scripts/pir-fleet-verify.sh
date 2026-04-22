#!/usr/bin/env bash
# Post-deploy checks: env file uses SVOTE_PIR_* for bootstrap URLs and
# localhost metrics show snapshot heights on each PIR host.
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
    else
      echo "WARN: no /etc/default/nf-server" >&2
    fi
    echo "--- /ready ---"
    curl -sfS --max-time 5 http://127.0.0.1:3000/ready >/dev/null && echo OK || { echo "FAIL: /ready" >&2; exit 1; }
    echo "--- metrics heights ---"
    curl -sfS --max-time 5 http://127.0.0.1:3000/metrics | awk "\$1==\"nf_snapshot_served_height\" || \$1==\"nf_snapshot_expected_height\" {print}"
  '
}

verify_one "backup" "$BACKUP"
verify_one "primary" "$PRIMARY"
echo "Done."
