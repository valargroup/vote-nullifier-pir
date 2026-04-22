#!/usr/bin/env bash
# Rename legacy nf-server bootstrap env keys in /etc/default/nf-server on PIR hosts.
# Post–PR #47 nf-server only reads SVOTE_PIR_VOTING_CONFIG_URL and SVOTE_PIR_PRECOMPUTED_BASE_URL.
#
# Usage (SSH config host aliases or user@hostname):
#   ./scripts/pir-fleet-migrate-env-defaults.sh BACKUP_SSH_TARGET PRIMARY_SSH_TARGET
#
# Order: pass backup first, then primary (matches deploy/restart roll order).
# Idempotent: lines already using SVOTE_PIR_* are left unchanged.
set -euo pipefail

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" || $# -lt 2 ]]; then
  echo "Usage: $0 <backup_ssh_target> <primary_ssh_target>" >&2
  echo "  Renames SVOTE_VOTING_CONFIG_URL -> SVOTE_PIR_VOTING_CONFIG_URL" >&2
  echo "  and SVOTE_PRECOMPUTED_BASE_URL -> SVOTE_PIR_PRECOMPUTED_BASE_URL" >&2
  echo "  in /etc/default/nf-server on each host (with timestamped backup)." >&2
  exit 1
fi

BACKUP="$1"
PRIMARY="$2"

migrate_one() {
  local label="$1"
  local host="$2"
  echo "==> Migrating /etc/default/nf-server on ${label} (${host})"
  ssh -o BatchMode=yes "$host" 'set -euo pipefail
    if [[ ! -f /etc/default/nf-server ]]; then
      echo "ERROR: /etc/default/nf-server missing on this host" >&2
      exit 1
    fi
    ts=$(date +%s)
    sudo cp /etc/default/nf-server "/etc/default/nf-server.bak.${ts}"
    tmp=$(mktemp)
    sudo cp /etc/default/nf-server "$tmp"
    sudo sed -i \
      -e "s/^SVOTE_VOTING_CONFIG_URL=/SVOTE_PIR_VOTING_CONFIG_URL=/" \
      -e "s/^SVOTE_PRECOMPUTED_BASE_URL=/SVOTE_PIR_PRECOMPUTED_BASE_URL=/" \
      "$tmp"
    sudo install -m 0644 -o root -g root "$tmp" /etc/default/nf-server
    sudo rm -f "$tmp"
    echo "    Updated file (backup at /etc/default/nf-server.bak.${ts})"
    grep -E "^(SVOTE_PIR_VOTING_CONFIG_URL|SVOTE_PIR_PRECOMPUTED_BASE_URL|#)" /etc/default/nf-server 2>/dev/null | head -20 || true
  '
}

migrate_one "backup" "$BACKUP"
migrate_one "primary" "$PRIMARY"
echo "Done. If you changed the systemd unit, run: sudo systemctl daemon-reload"
echo "Then deploy or restart nullifier-query-server when ready."
