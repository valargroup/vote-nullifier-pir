#!/bin/bash
# Run as root with an uploaded, checksum-verified artifact bundle.
set -euo pipefail
bundle=${1:?usage: install.sh BUNDLE VERSION}
version=${2:?version required}
[[ "$version" =~ ^[a-zA-Z0-9._-]+$ ]] || exit 2
(cd "$bundle" && sha256sum --check SHA256SUMS)
getent passwd pir-probe >/dev/null || useradd --system --home-dir /var/lib/pir-probe --shell /usr/sbin/nologin pir-probe
install -d -m 0755 /opt/pir-probe/releases /etc/pir-probe
install -d -m 0700 -o pir-probe -g pir-probe /var/lib/pir-probe
release=/opt/pir-probe/releases/$version
[[ ! -e "$release" ]] || { echo 'Release already installed; use a new version' >&2; exit 1; }
install -d -m 0755 "$release"
install -m 0755 "$bundle/pir-probe" "$bundle/pir-probe-query" "$bundle/commission.py" "$bundle/fault-test.py" "$release/"
install -m 0644 "$bundle/SHA256SUMS" "$bundle/BUILD.json" "$release/"
if [[ -f /etc/systemd/system/pir-probe.service ]]; then cp -a /etc/systemd/system/pir-probe.service "$release/previous.service"; fi
if [[ -e /opt/pir-probe/current ]]; then readlink -f /opt/pir-probe/current > "$release/previous-release"; fi
if [[ -f /etc/pir-probe/config.json ]]; then cp -a /etc/pir-probe/config.json "$release/previous-config.json"; fi
install -m 0644 "$bundle/production.json" /etc/pir-probe/config.json
install -m 0644 "$bundle/pir-probe.service" /etc/systemd/system/pir-probe.service
ln -s "$release" /opt/pir-probe/current.new
mv -Tf /opt/pir-probe/current.new /opt/pir-probe/current
systemd-analyze verify /etc/systemd/system/pir-probe.service
systemctl daemon-reload
# Deliberately not started until secrets and commissioning are complete.
echo "Installed $version; not started"
