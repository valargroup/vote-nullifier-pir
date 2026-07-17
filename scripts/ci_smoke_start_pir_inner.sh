#!/usr/bin/env bash
# Runs inside a fresh Ubuntu container (see ci_smoke_start_pir.sh).
set -euo pipefail

mkdir -p /mockbin
cat >/mockbin/systemctl <<'MOCK'
#!/bin/bash
echo "[smoke] systemctl $*" >&2
echo "$*" >> /tmp/systemctl.log
exit 0
MOCK
chmod +x /mockbin/systemctl

cat >/mockbin/curl <<'MOCK'
#!/bin/bash
case "$*" in
  *'http://127.0.0.1:3000/ready'*)
    exit 0
    ;;
  *'/nullifier-query-server.service'*)
    output=""
    previous=""
    for arg in "$@"; do
      if [ "$previous" = "-o" ]; then
        output="$arg"
        break
      fi
      previous="$arg"
    done
    [ -n "$output" ] || exit 2
    cp /release-service "$output"
    exit 0
    ;;
esac
exec /usr/bin/curl "$@"
MOCK
chmod +x /mockbin/curl
export PATH="/mockbin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"

missing_network_output="$(mktemp)"
if env -u SVOTE_ZCASH_NETWORK bash /start_pir.sh >"$missing_network_output" 2>&1; then
  echo "start_pir smoke: installer accepted a missing SVOTE_ZCASH_NETWORK" >&2
  exit 1
fi
grep -Fq 'SVOTE_ZCASH_NETWORK must be set to main or test' "$missing_network_output"
export SVOTE_ZCASH_NETWORK=test

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
test -d /opt/nf-ingest/pir-data/test
test -f /etc/systemd/system/nullifier-query-server.service
grep -Fxq 'ExecStart=/opt/nf-ingest/nf-server serve --port 3000' /etc/systemd/system/nullifier-query-server.service
if grep -Fq -- '--pir-data-dir' /etc/systemd/system/nullifier-query-server.service; then
  echo "start_pir smoke: service overrides SVOTE_PIR_DATA_DIR" >&2
  exit 1
fi
test -f /etc/default/nf-server
grep -Fxq 'SVOTE_ZCASH_NETWORK=test' /etc/default/nf-server
grep -Fxq 'SVOTE_PIR_DATA_DIR=/opt/nf-ingest/pir-data/test' /etc/default/nf-server
grep -Fxq 'SVOTE_PIR_CONFIG_URL=https://voting.valargroup.org/stage/pir.json' /etc/default/nf-server
grep -Fxq 'SVOTE_PIR_VOTING_CONFIG_URL=https://voting.valargroup.org/stage/static-voting-config.json' /etc/default/nf-server
grep -Fq SVOTE_PIR_PRECOMPUTED_BASE_URL /etc/default/nf-server
command -v curl >/dev/null

if [ -f /update_pir.sh ]; then
  echo 'RUST_LOG=info,nf_server=debug' >> /etc/default/nf-server
  cp /etc/default/nf-server /tmp/nf-server.defaults.before-update

  systemctl_lines_before="$(wc -l < /tmp/systemctl.log)"
  conflicting_network_output="$(mktemp)"
  if SVOTE_ZCASH_NETWORK=main bash /update_pir.sh --force >"$conflicting_network_output" 2>&1; then
    echo "start_pir smoke: updater changed an installed network" >&2
    exit 1
  fi
  grep -Fq 'Refusing to change Zcash network from test to main' "$conflicting_network_output"
  if grep -Fq '==> Downloading release checksums' "$conflicting_network_output"; then
    echo "start_pir smoke: updater downloaded artifacts before rejecting a network change" >&2
    exit 1
  fi
  cmp -s /tmp/nf-server.defaults.before-update /etc/default/nf-server
  test ! -d /opt/nf-ingest/pir-data/main
  test "$(wc -l < /tmp/systemctl.log)" -eq "$systemctl_lines_before"

  unset SVOTE_ZCASH_NETWORK

  updater_output="$(mktemp)"
  if bash /update_pir.sh --force >"$updater_output" 2>&1; then
    :
  else
    status=$?
    cat "$updater_output" >&2
    exit "$status"
  fi
  cat "$updater_output"
  if grep -Fq '% Total' "$updater_output"; then
    echo "start_pir smoke: updater leaked curl progress output" >&2
    exit 1
  fi
  grep -Fq '==> Downloading release checksums' "$updater_output"
  grep -Fq '==> Verifying nf-server checksum' "$updater_output"
  grep -Fq '==> Restarting nullifier-query-server.service' "$updater_output"
  grep -Fq 'PIR server update completed successfully' "$updater_output"

  cmp -s /tmp/nf-server.defaults.before-update /etc/default/nf-server
  test -x /opt/nf-ingest/nf-server
  test -L /usr/local/bin/nf-server
  test "$(readlink /usr/local/bin/nf-server)" = /opt/nf-ingest/nf-server
  test -d /opt/nf-ingest/pir-data/test
  test -f /etc/systemd/system/nullifier-query-server.service
  grep -Fxq 'ExecStart=/opt/nf-ingest/nf-server serve --port 3000' /etc/systemd/system/nullifier-query-server.service
  if grep -Fq -- '--pir-data-dir' /etc/systemd/system/nullifier-query-server.service; then
    echo "start_pir smoke: refreshed service overrides SVOTE_PIR_DATA_DIR" >&2
    exit 1
  fi
  grep -Fxq 'SVOTE_ZCASH_NETWORK=test' /etc/default/nf-server
  grep -Fxq 'SVOTE_PIR_DATA_DIR=/opt/nf-ingest/pir-data/test' /etc/default/nf-server
  grep -Fxq 'daemon-reload' /tmp/systemctl.log
  grep -Fxq 'restart nullifier-query-server.service' /tmp/systemctl.log
fi

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
