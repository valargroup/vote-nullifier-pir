#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
channel_script="${repo_root}/scripts/release-channel.sh"
promotion_script="${repo_root}/scripts/validate-release-promotion.sh"
pointer_script="${repo_root}/scripts/publish-release-pointers.sh"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

[ "$($channel_script v0.0.41)" = "stable" ] || fail "stable tag classification"
[ "$($channel_script v0.0.41-rc.1)" = "rc" ] || fail "RC tag classification"
for tag in v0.0 v0.0.41-rc v0.0.41-beta.1 v0.0.41.1; do
  if "$channel_script" "$tag" >/dev/null 2>&1; then
    fail "invalid tag accepted: $tag"
  fi
done

[ "$($promotion_script v0.0.41 v0.0.41)" = "v0.0.41" ] \
  || fail "held stable release promotion validation"
if "$promotion_script" v0.0.41-rc.1 v0.0.41-rc.1 >/dev/null 2>&1; then
  fail "RC release promotion accepted"
fi
if "$promotion_script" v0.0.41 v0.0.42 >/dev/null 2>&1; then
  fail "promotion tag did not match hold"
fi
if "$promotion_script" v0.0.41 "" >/dev/null 2>&1; then
  fail "promotion accepted without a hold"
fi

tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

sed "s|__RELEASE_TAG__|v0.0.41|g" \
  "${repo_root}/scripts/start_pir.sh.template" > "${tmp_dir}/start_pir.sh"
sed "s|__RELEASE_TAG__|v0.0.41|g" \
  "${repo_root}/scripts/update_pir.sh.template" > "${tmp_dir}/update_pir.sh"
printf '{"snapshot_height": 4134000}\n' > "${tmp_dir}/pir.json"

cat > "${tmp_dir}/s3cmd" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$*" >> "$S3CMD_LOG"
EOF
chmod +x "${tmp_dir}/s3cmd"

export S3CMD_BIN="${tmp_dir}/s3cmd"
export S3CMD_LOG="${tmp_dir}/uploads.log"
export PIR_CONFIG_URL="file://${tmp_dir}/pir.json"

"$pointer_script" v0.0.41-rc.1 "${tmp_dir}/s3cfg" \
  "${tmp_dir}/start_pir.sh" "${tmp_dir}/update_pir.sh"
[ ! -s "$S3CMD_LOG" ] || fail "RC release updated mutable pointers"

"$pointer_script" v0.0.41 "${tmp_dir}/s3cfg" \
  "${tmp_dir}/start_pir.sh" "${tmp_dir}/update_pir.sh"
grep -q 's3://shielded-vote/scripts/start_pir/4134000/start_pir.sh' "$S3CMD_LOG" \
  || fail "snapshot-height installer pointer missing"
grep -q 's3://shielded-vote/update_pir.sh' "$S3CMD_LOG" \
  || fail "stable updater pointer missing"
tail -n 1 "$S3CMD_LOG" | grep -q 's3://shielded-vote/start_pir.sh' \
  || fail "stable installer pointer was not published last"

echo "PASS: release channel tests"
