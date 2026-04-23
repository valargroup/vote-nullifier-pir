#!/usr/bin/env bash
# Runs inside a fresh Ubuntu container (see ci_smoke_start_pir.sh).
set -euo pipefail

mkdir -p /mockbin
cat >/mockbin/systemctl <<'MOCK'
#!/bin/bash
echo "[smoke] systemctl $*" >&2
exit 0
MOCK
chmod +x /mockbin/systemctl
export PATH="/mockbin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

bash /start_pir.sh

# Optional: replace the release download with a known-portable binary (CI builds
# with x86-64-v3) so installer smoke stays green while older tagged assets used
# a higher microarchitecture level.
if [ -n "${OVERRIDE_NF_SERVER:-}" ] && [ -x "${OVERRIDE_NF_SERVER}" ]; then
  install -m 755 "${OVERRIDE_NF_SERVER}" /opt/nf-ingest/nf-server
fi

test -x /opt/nf-ingest/nf-server
test -f /etc/systemd/system/nullifier-query-server.service
test -f /etc/default/nf-server
grep -Fq SVOTE_PIR_VOTING_CONFIG_URL /etc/default/nf-server
grep -Fq SVOTE_PIR_PRECOMPUTED_BASE_URL /etc/default/nf-server
command -v curl >/dev/null
/opt/nf-ingest/nf-server --help >/dev/null
/opt/nf-ingest/nf-server snapshot --help >/dev/null
grep -Fq 'admin-listen' /etc/systemd/system/nullifier-query-server.service
grep -Fq 'admin.sock' /etc/systemd/system/nullifier-query-server.service

echo "start_pir smoke: OK"
