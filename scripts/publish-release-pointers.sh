#!/usr/bin/env bash
set -euo pipefail

version="${1:-}"
s3_config="${2:-}"
start_script="${3:-}"
update_script="${4:-}"
height_output="${5:-}"
bucket="${DO_BUCKET:-shielded-vote}"
s3cmd_bin="${S3CMD_BIN:-s3cmd}"
pir_config_url="${PIR_CONFIG_URL:-https://voting.valargroup.org/prod/pir.json}"
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

[ -n "$version" ] || { echo "ERROR: release version is required." >&2; exit 1; }
[ -n "$s3_config" ] || { echo "ERROR: s3cmd config path is required." >&2; exit 1; }
[ -f "$start_script" ] || { echo "ERROR: start_pir.sh is required." >&2; exit 1; }
[ -f "$update_script" ] || { echo "ERROR: update_pir.sh is required." >&2; exit 1; }

channel="$("${repo_root}/scripts/release-channel.sh" "$version")"
if [ "$channel" != "stable" ]; then
  echo "Skipping mutable release pointers for ${version}."
  exit 0
fi

grep -Fq "readonly START_RELEASE_TAG='${version}'" "$start_script" \
  || { echo "ERROR: start_pir.sh does not target ${version}." >&2; exit 1; }
grep -Fq "readonly UPDATE_DEFAULT_RELEASE_TAG='${version}'" "$update_script" \
  || { echo "ERROR: update_pir.sh does not target ${version}." >&2; exit 1; }

put() {
  "$s3cmd_bin" -c "$s3_config" put -m text/plain --acl-public --force \
    "$1" "s3://${bucket}/$2"
}

if ! config="$(curl -fsSL "$pir_config_url")"; then
  echo "ERROR: unable to load PIR config from ${pir_config_url}." >&2
  exit 1
fi
if ! height="$(jq -er '.snapshot_height // empty' <<<"$config" 2>/dev/null)"; then
  echo "ERROR: PIR config does not contain a snapshot_height." >&2
  exit 1
fi
case "$height" in
  *[!0-9]*)
    echo "PIR config snapshot_height must be numeric, got: ${height}" >&2
    exit 1
    ;;
esac

if [ -n "$height_output" ]; then
  printf '%s\n' "$height" > "$height_output"
fi

put "$start_script" "scripts/start_pir/${height}/start_pir.sh"

put "$update_script" "update_pir.sh"
put "$start_script" "start_pir.sh"
