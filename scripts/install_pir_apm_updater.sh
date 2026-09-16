#!/usr/bin/env bash
# Install the decoupled pir-apm sidecar updater on a Valargroup fleet host.
#
# Valargroup fleet hosts only. Integrator hosts never run this, never receive a
# pir-apm binary or unit, and are unaffected by it. It does not read or modify
# /opt/pir-updater, nf-server, its unit or its generations.
#
# Usage: install_pir_apm_updater.sh --config-url https://voting.valargroup.dev/<env>/pir.json
set -euo pipefail

CONFIG_URL=""
while [ $# -gt 0 ]; do
  case "$1" in
    --config-url) CONFIG_URL="${2:-}"; shift 2 ;;
    *) echo "unknown argument: $1" >&2; exit 1 ;;
  esac
done

case "$CONFIG_URL" in
  https://*/pir.json) ;;
  *) echo 'A --config-url https://.../pir.json is required' >&2; exit 1 ;;
esac

[ "$(id -u)" -eq 0 ] || { echo 'Run as root' >&2; exit 1; }
command -v python3 >/dev/null || { echo 'Install python3 first' >&2; exit 1; }

src="$(cd "$(dirname "$0")/.." && pwd)"
install -d -m 0755 /opt/pir-apm-updater /var/lib/pir-apm-updater
install -m 0755 "$src/deploy/pir-apm-updater/pir_apm_updater.py" /opt/pir-apm-updater/pir_apm_updater.py
install -m 0644 "$src/deploy/systemd/pir-apm-updater.service" /etc/systemd/system/pir-apm-updater.service
install -m 0644 "$src/deploy/systemd/pir-apm-updater.timer" /etc/systemd/system/pir-apm-updater.timer

# Its own settings file. The config_url value matches the serving updater's,
# but is configured independently so the two share no state.
tmp="$(mktemp)"
printf '{"config_url": "%s"}\n' "$CONFIG_URL" > "$tmp"
install -m 0644 -o root -g root "$tmp" /opt/pir-apm-updater/settings.json
rm -f "$tmp"

systemctl daemon-reload
systemctl enable --now pir-apm-updater.timer
echo "Installed. Reconciling once now:"
python3 /opt/pir-apm-updater/pir_apm_updater.py --once
systemctl list-timers pir-apm-updater.timer --no-pager
