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

installer_output="$(mktemp)"
if bash /start_pir.sh >"$installer_output" 2>&1; then
  :
else
  status=$?
  cat "$installer_output" >&2
  exit "$status"
fi
cat "$installer_output"
if grep -Fq '% Total' "$installer_output"; then
  echo "start_pir smoke: installer leaked curl progress output" >&2
  exit 1
fi
grep -Fq '==> Downloading nf-server' "$installer_output"
grep -Fq '==> Verifying nf-server checksum' "$installer_output"
grep -Fq '==> Starting nullifier-query-server' "$installer_output"

test -x /opt/nf-ingest/nf-server
test -L /usr/local/bin/nf-server
test "$(readlink /usr/local/bin/nf-server)" = /opt/nf-ingest/nf-server
test -f /etc/systemd/system/nullifier-query-server.service
test -f /etc/default/nf-server
grep -Fq SVOTE_PIR_VOTING_CONFIG_URL /etc/default/nf-server
grep -Fq SVOTE_PIR_PRECOMPUTED_BASE_URL /etc/default/nf-server
command -v curl >/dev/null

if [ "$(uname -m)" = "x86_64" ]; then
  # linux-amd64 release binaries are built for the production AVX-512 fleet
  # target. Generic CI runners may install them successfully but SIGILL when
  # executing them, so only run the binary when the host exposes the full ISA.
  if ! awk '
    /^flags[[:space:]]*:/ {
      has = 1
      for (i = 1; i <= NF; i++) {
        seen[$i] = 1
      }
      exit
    }
    END {
      exit !(has && seen["avx512f"] && seen["avx512bw"] && seen["avx512cd"] && seen["avx512dq"] && seen["avx512vl"])
    }
  ' /proc/cpuinfo; then
    echo "start_pir smoke: skipping nf-server execution; CI CPU lacks x86-64-v4/AVX-512"
    echo "start_pir smoke: OK"
    exit 0
  fi
fi

/opt/nf-ingest/nf-server --help >/dev/null
nf-server --help >/dev/null

echo "start_pir smoke: OK"
