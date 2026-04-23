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

test -x /opt/nf-ingest/nf-server
test -f /etc/systemd/system/nullifier-query-server.service
test -f /etc/default/nf-server
grep -Fq SVOTE_PIR_VOTING_CONFIG_URL /etc/default/nf-server
grep -Fq SVOTE_PIR_PRECOMPUTED_BASE_URL /etc/default/nf-server
command -v curl >/dev/null

# Release linux-amd64 builds may use AVX-512; `docker run --platform linux/amd64`
# on Apple Silicon often hits SIGILL (exit 132) even for `--help`. Real DO
# recommended hardware runs this natively and should pass.
set +e
/opt/nf-ingest/nf-server --help >/dev/null 2>&1
nf_help_ec=$?
set -e
if [ "$nf_help_ec" -ne 0 ]; then
  if [ "$nf_help_ec" -eq 132 ]; then
    echo "start_pir smoke: install layout OK (skipped nf-server --help: SIGILL/emulation or missing CPU features)" >&2
  else
    exit "$nf_help_ec"
  fi
fi

echo "start_pir smoke: OK"
