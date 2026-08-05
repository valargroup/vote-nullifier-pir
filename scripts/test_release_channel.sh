#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
channel_script="${repo_root}/scripts/release-channel.sh"
metadata_script="${repo_root}/scripts/release-metadata.sh"
promotion_script="${repo_root}/scripts/validate-release-promotion.sh"
channel_update_script="${repo_root}/scripts/validate-release-channel-update.sh"
pointer_script="${repo_root}/scripts/publish-release-pointers.sh"
release_workflow="${repo_root}/.github/workflows/release.yml"
promotion_workflow="${repo_root}/.github/workflows/promote-release.yml"

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

expected_rc_metadata=$'prerelease=true\nmake_latest=false\nalready_latest=false\npublish_mutable_pointers=false'
[ "$($metadata_script v0.0.41-rc.1 v0.0.41-rc.1)" = "$expected_rc_metadata" ] \
  || fail "held RC was not kept as a prerelease"
expected_held_metadata=$'prerelease=false\nmake_latest=false\nalready_latest=false\npublish_mutable_pointers=false'
[ "$($metadata_script v0.0.41 v0.0.41)" = "$expected_held_metadata" ] \
  || fail "held stable release metadata"
expected_stable_metadata=$'prerelease=false\nmake_latest=true\nalready_latest=false\npublish_mutable_pointers=true'
[ "$($metadata_script v0.0.41 v0.0.42)" = "$expected_stable_metadata" ] \
  || fail "unheld stable release metadata"
expected_current_metadata=$'prerelease=false\nmake_latest=true\nalready_latest=true\npublish_mutable_pointers=true'
[ "$($metadata_script v0.0.41 v0.0.41 v0.0.41)" = "$expected_current_metadata" ] \
  || fail "already promoted held release metadata"

if grep -Fq 'softprops/action-gh-release' "$release_workflow"; then
  fail "release creation can mutate Latest outside the channel lock"
fi
grep -Fq -- '--latest=false --verify-tag' "$release_workflow" \
  || fail "new GitHub releases are not created outside Latest"
grep -Fq 'gh release upload' "$release_workflow" \
  || fail "release reruns do not preserve channel state while refreshing assets"
pointer_line="$(grep -n 'scripts/publish-release-pointers.sh' "$release_workflow" | tail -n 1 | cut -d: -f1)"
verify_line="$(grep -n -- '- name: Verify mutable release pointers' "$release_workflow" | cut -d: -f1)"
latest_line="$(grep -n -- '- name: Mark GitHub release latest' "$release_workflow" | cut -d: -f1)"
[ "$pointer_line" -lt "$latest_line" ] || fail "GitHub Latest advances before mutable pointers"
[ "$pointer_line" -lt "$verify_line" ] && [ "$verify_line" -lt "$latest_line" ] \
  || fail "stable pointers are not verified before GitHub Latest advances"
[ "$(grep -Fc 'uses: actions/checkout@v4' "$promotion_workflow")" -eq 1 ] \
  || fail "promotion switches away from the trusted checkout"

if grep -q '^concurrency:' "$release_workflow" "$promotion_workflow"; then
  fail "workflow-level release concurrency can cancel queued artifact publication"
fi
[ "$(grep -Fc 'group: release-channel' "$release_workflow")" -eq 1 ] \
  || fail "stable channel mutation job does not own one shared lock"
[ "$(grep -Fc 'group: release-channel' "$promotion_workflow")" -eq 1 ] \
  || fail "promotion job does not own one shared lock"
release_channel_job_line="$(grep -n '^  publish-release-channel:' "$release_workflow" | cut -d: -f1)"
release_concurrency_line="$(grep -n '^    concurrency:' "$release_workflow" | cut -d: -f1)"
promotion_job_line="$(grep -n '^  promote:' "$promotion_workflow" | cut -d: -f1)"
promotion_concurrency_line="$(grep -n '^    concurrency:' "$promotion_workflow" | cut -d: -f1)"
[ "$release_channel_job_line" -lt "$release_concurrency_line" ] \
  || fail "release concurrency is not scoped to the channel mutation job"
[ "$promotion_job_line" -lt "$promotion_concurrency_line" ] \
  || fail "promotion concurrency is not scoped to the promotion job"

promotion_asset_line="$(grep -n -- '--pattern nullifier-query-server.service' "$promotion_workflow" | cut -d: -f1)"
promotion_pointer_line="$(grep -n 'scripts/publish-release-pointers.sh' "$promotion_workflow" | cut -d: -f1)"
promotion_verify_line="$(grep -n -- '- name: Verify mutable release pointers' "$promotion_workflow" | cut -d: -f1)"
promotion_latest_line="$(grep -n -- '- name: Mark GitHub release latest' "$promotion_workflow" | cut -d: -f1)"
[ "$promotion_asset_line" -lt "$promotion_pointer_line" ] \
  || fail "promotion mutates channels before verifying the GitHub service asset"
[ "$promotion_pointer_line" -lt "$promotion_verify_line" ] \
  && [ "$promotion_verify_line" -lt "$promotion_latest_line" ] \
  || fail "promotion aliases are not verified before GitHub Latest advances"
height_output_pattern='SNAPSHOT_HEIGHT: $''{{ steps.publish.outputs.snapshot_height }}'
[ "$(grep -Fc "$height_output_pattern" "$release_workflow")" -eq 1 ] \
  || fail "release verification does not reuse the published snapshot height"
[ "$(grep -Fc "$height_output_pattern" "$promotion_workflow")" -eq 1 ] \
  || fail "promotion verification does not reuse the published snapshot height"

[ "$($channel_update_script v0.0.41 v0.0.40)" = "v0.0.41" ] \
  || fail "newer stable channel update validation"
[ "$($channel_update_script v0.0.41 v0.0.41)" = "v0.0.41" ] \
  || fail "idempotent stable channel update validation"
[ "$($channel_update_script v1.0.0 v0.99.99)" = "v1.0.0" ] \
  || fail "new major stable channel update validation"
if "$channel_update_script" v0.0.41 v0.0.42 >/dev/null 2>&1; then
  fail "stable channel update replaced a newer Latest release"
fi
if "$channel_update_script" v0.0.41-rc.1 v0.0.41 >/dev/null 2>&1; then
  fail "RC release accepted for stable channel update"
fi

[ "$($promotion_script v0.0.41 v0.0.41 v0.0.40)" = "v0.0.41" ] \
  || fail "held stable release promotion validation"
[ "$($promotion_script v0.0.41 v0.0.41 v0.0.41)" = "v0.0.41" ] \
  || fail "idempotent promotion validation"
[ "$($promotion_script v1.0.0 v1.0.0 v0.99.99)" = "v1.0.0" ] \
  || fail "new major release promotion validation"
if "$promotion_script" v0.0.41 v0.0.41 v0.0.42 >/dev/null 2>&1; then
  fail "promotion replaced a newer Latest release"
fi
if "$promotion_script" v1.0.0 v1.0.0 v10.0.0 >/dev/null 2>&1; then
  fail "promotion replaced a newer multi-digit major release"
fi
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
  "${tmp_dir}/start_pir.sh" "${tmp_dir}/update_pir.sh" \
  "${tmp_dir}/published-height"
[ ! -s "$S3CMD_LOG" ] || fail "RC release updated mutable pointers"
[ ! -e "${tmp_dir}/published-height" ] || fail "RC release reported a published height"

"$pointer_script" v0.0.41 "${tmp_dir}/s3cfg" \
  "${tmp_dir}/start_pir.sh" "${tmp_dir}/update_pir.sh" \
  "${tmp_dir}/published-height"
[ "$(cat "${tmp_dir}/published-height")" = "4134000" ] \
  || fail "publisher did not report the snapshot height it used"
grep -q 's3://shielded-vote/scripts/start_pir/4134000/start_pir.sh' "$S3CMD_LOG" \
  || fail "snapshot-height installer pointer missing"
grep -q 's3://shielded-vote/update_pir.sh' "$S3CMD_LOG" \
  || fail "stable updater pointer missing"
tail -n 1 "$S3CMD_LOG" | grep -q 's3://shielded-vote/start_pir.sh' \
  || fail "stable installer pointer was not published last"

: > "$S3CMD_LOG"
rm -f "${tmp_dir}/published-height"
export PIR_CONFIG_URL="file://${tmp_dir}/missing.json"
if "$pointer_script" v0.0.41 "${tmp_dir}/s3cfg" \
  "${tmp_dir}/start_pir.sh" "${tmp_dir}/update_pir.sh" \
  "${tmp_dir}/published-height" >/dev/null 2>&1; then
  fail "stable pointers published without PIR config"
fi
[ ! -s "$S3CMD_LOG" ] || fail "pointers changed before PIR config validation"
[ ! -e "${tmp_dir}/published-height" ] || fail "missing PIR config reported a published height"

printf '{}\n' > "${tmp_dir}/pir-without-height.json"
export PIR_CONFIG_URL="file://${tmp_dir}/pir-without-height.json"
if "$pointer_script" v0.0.41 "${tmp_dir}/s3cfg" \
  "${tmp_dir}/start_pir.sh" "${tmp_dir}/update_pir.sh" \
  "${tmp_dir}/published-height" >/dev/null 2>&1; then
  fail "stable pointers published without snapshot height"
fi
[ ! -s "$S3CMD_LOG" ] || fail "pointers changed before snapshot height validation"
[ ! -e "${tmp_dir}/published-height" ] || fail "missing snapshot height was reported as published"

: > "$S3CMD_LOG"
printf '{"snapshot_height":""}\n' > "${tmp_dir}/pir-with-empty-height.json"
export PIR_CONFIG_URL="file://${tmp_dir}/pir-with-empty-height.json"
if "$pointer_script" v0.0.41 "${tmp_dir}/s3cfg" \
  "${tmp_dir}/start_pir.sh" "${tmp_dir}/update_pir.sh" \
  "${tmp_dir}/published-height" >/dev/null 2>&1; then
  fail "stable pointers published with an empty snapshot height"
fi
[ ! -s "$S3CMD_LOG" ] || fail "pointers changed before empty snapshot height validation"
[ ! -e "${tmp_dir}/published-height" ] || fail "empty snapshot height was reported as published"

echo "PASS: release channel tests"
