# Runbook: restart the PIR fleet

A rolling restart of the production PIR replicas
(`vote-nullifier-pir-primary`, `vote-nullifier-pir-backup`).

The canonical trigger is the
[**Restart PIR fleet**](https://github.com/valargroup/vote-nullifier-pir/actions/workflows/restart.yml)
workflow. SSH-from-laptop is documented at the bottom as a fallback for
when GitHub Actions is unavailable.

## When to use

| Scenario | Action |
|----------|--------|
| A new on-chain voting round with a new `snapshot_height` is active and replicas need to pick up the new snapshot. | Run `Restart PIR fleet` with `targets=both`. |
| Sentry fired `alert:snapshot_stale` for one host and the underlying issue is resolved. | Run `Restart PIR fleet` with `targets=primary` or `targets=backup`. |
| You changed `/etc/default/nf-server` (e.g. flipped `SVOTE_PIR_VOTING_CONFIG_URL` to a staging mirror). | Run `Restart PIR fleet` with `targets=both`. |
| You need to force replicas onto a specific already-published DO snapshot, regardless of active-round config. | Run `Restart PIR fleet` with `height=<snapshot_height>`. |
| You're deploying a new `nf-server` binary. | Use `Deploy nf-server` instead — it does the binary swap *and* the restart. |
| You need a new snapshot from chain (nothing published at the new height yet). | Run `Publish nullifier snapshot` first, then this workflow. |

The workflow is **idempotent**: if a replica is already on the
expected height, it just gets a fresh process with the same loaded
snapshot. There is no harm in running it again.

## What the workflow does

`workflow_dispatch` inputs:

| Input | Default | Notes |
|-------|---------|-------|
| `targets` | `both` | `both`, `primary`, or `backup`. |
| `verify_height_converged` | `true` | After restart, fail the job if `nf_snapshot_served_height < nf_snapshot_expected_height`. Set `false` if you intentionally want to restart without checking convergence (e.g. you're rolling back to an older config and `expected` is going to be lower than `served`). |
| `height` | *(empty)* | Optional forced DO snapshot height. Must be numeric, a multiple of 10, and already published under `https://vote.fra1.digitaloceanspaces.com/snapshots/<height>/`. |

When `height` is set, the workflow first validates the DO snapshot manifest and
the required tier objects (`tier0.bin`, `tier1.bin`, `tier2.bin`,
`pir_root.json`) before touching any host. Each host then gets a temporary
systemd drop-in:

```ini
[Service]
Environment=SVOTE_PIR_VOTING_CONFIG_URL=
Environment=SVOTE_PIR_BOOTSTRAP_SNAPSHOT_HEIGHT=<height>
Environment=SVOTE_PIR_PRECOMPUTED_BASE_URL=https://vote.fra1.digitaloceanspaces.com
```

That disables active-round discovery for the restart and forces `nf-server` to
download from `snapshots/<height>/`. After `/ready` succeeds and
`nf_snapshot_served_height == height`, the workflow removes the drop-in and
runs `systemctl daemon-reload` again. The running process keeps serving the
loaded snapshot, but future restarts return to normal config-driven behavior.
If the host fails before readiness/verification, the drop-in is left in place so
the failed state is inspectable and a retry uses the same forced settings.

For `targets=both` the workflow restarts **backup first**, waits for
it to come back healthy *and* converge on the expected snapshot
height, then restarts primary. Primary is gated on backup succeeding
— if backup fails, primary is **not** restarted, so the fleet never
loses both replicas at once. The convenience alias `pir.<domain>`
points at primary, so primary carries traffic while backup is
restarting; once primary restarts, backup is already serving the new
snapshot.

For each host the SSH session:

1. `sudo systemctl restart nullifier-query-server`
2. polls `http://localhost:3000/ready` every 5 s for up to 10 minutes
   (the cold-start budget is ~30 s for the in-region snapshot
   download from DO Spaces, plus 60–90 s to mmap and parse the
   ~6 GB of tier files into memory; `/ready` only returns 200 once
   tier files are mmapped and queries can be served, whereas
   `/health` returns 200 as soon as the listener binds),
3. reads `nf_snapshot_served_height` and `nf_snapshot_expected_height`
   from `/metrics`, and
4. fails the job if convergence checks are enabled and `served < expected`.

On failure the SSH step dumps `systemctl status` and the most recent
80 journal lines (or syslog tail) into the workflow log.

## Running it

### From the GitHub UI

1. Open
   [Actions → Restart PIR fleet](https://github.com/valargroup/vote-nullifier-pir/actions/workflows/restart.yml).
2. Click **Run workflow**.
3. Pick `targets` (default `both`) and leave `verify_height_converged`
   on unless you have a specific reason to disable it.
   To force a published DO snapshot, fill in `height`.
4. Watch the two jobs in the run page. Each takes 2–3 minutes.

### From the CLI

```bash
gh workflow run restart.yml \
    --repo valargroup/vote-nullifier-pir \
    -f targets=both
```

Force a specific published snapshot:

```bash
gh workflow run restart.yml \
    --repo valargroup/vote-nullifier-pir \
    -f targets=both \
    -f height=3341750
```

To watch the run from the terminal:

```bash
gh run list --workflow=restart.yml --repo valargroup/vote-nullifier-pir --limit 1
gh run watch --repo valargroup/vote-nullifier-pir <run-id>
```

### Confirming convergence externally

Even with `verify_height_converged=true` it's worth eyeballing the
public endpoints once the workflow is green:

```bash
for host in pir-primary pir-backup; do
    echo "=== $host ==="
    curl -s "https://$host.valargroup.org/root" | jq '.height, .root25'
done
```

Both should report identical heights and roots.

## Failure modes

| Symptom | Likely cause | Recovery |
|---------|--------------|----------|
| `restart_backup` job times out at the readiness-check loop | Snapshot bootstrap couldn't fetch from `vote.fra1.digitaloceanspaces.com` (network / 5xx), sha256 mismatch on a tier file, or `load_serving_state` is still mmapping after 10 min. | Look at the dumped journal in the failed step. Re-run the workflow once for transient errors; if it keeps failing, run `Publish nullifier snapshot` against the same height and re-try. |
| `restart_primary` job is skipped after `restart_backup` failed | By design — the workflow refuses to restart primary while backup is unhealthy. | Fix backup first (see row above). Once backup is healthy, run the workflow again with `targets=primary`. |
| `validate_height` fails | The requested `height` is malformed, not a multiple of 10, the manifest is missing, the manifest height differs, or a required tier object is unavailable. | Publish or re-publish the snapshot with `Publish nullifier snapshot`, then rerun `Restart PIR fleet` with the same height. |
| Job fails with `served (X) != forced height (Y)` | Forced bootstrap ran but the host did not load the requested snapshot. | Inspect `nf_snapshot_bootstrap_outcomes_total` in the workflow log and `journalctl -u nullifier-query-server`. The temporary force-snapshot drop-in is intentionally left on the host for debugging/retry. |
| Job logs `expected=0` and `served>0` | No active round exposed a `snapshot_height`, so the server kept serving its local snapshot. This is acceptable while `/ready` is green. | No action unless you expected an active round. Confirm the active-round API and static/dynamic config if this is surprising. |
| Job fails with `expected=0` and `served=0` | The server is ready but has no usable local snapshot, or metrics are missing. | Check `nf_snapshot_bootstrap_outcomes_total` in the workflow log, then inspect `journalctl -u nullifier-query-server`. If this is a fresh host with no active round, set `SVOTE_PIR_BOOTSTRAP_SNAPSHOT_HEIGHT` or wait for an active round. |
| Job fails with `served (X) < expected (Y)` | Replica started but the bootstrap "fell through" — check `nf_snapshot_bootstrap_outcomes_total{result="fell_through"}`. | Confirm the snapshot exists in the bucket: `curl -sfI https://vote.fra1.digitaloceanspaces.com/snapshots/<expected>/manifest.json`. If 404, run `Publish nullifier snapshot` for that height. If 200, look for a sha256 mismatch in the journal. |
| Job fails with `tier1.bin size mismatch` (or similar) | Locally cached `pir-data/` is from a partial bootstrap or a different `nf-server` build. | SSH in: `sudo rm -rf /opt/nf-ingest/pir-data/* && sudo systemctl restart nullifier-query-server`. The next bootstrap repopulates from the bucket. |
| Sentry fires `alert:snapshot_stale` for the host you just restarted | Same as the row above — bootstrap fell through and `served < expected` for >30 minutes. | Same recovery. The watchdog emits a follow-up info event ("snapshot height converged") once the gap closes. |

## SSH fallback (CI unavailable)

If GitHub Actions is down, you can do the same rolling restart from a
laptop with SSH access:

```bash
# Pre-flight: confirm both replicas are healthy on the current height
for host in pir-primary pir-backup; do
    curl -s "https://$host.valargroup.org/root" | jq '.height'
done

# Backup first
ssh root@pir-backup.valargroup.org sudo systemctl restart nullifier-query-server

# Wait for backup to be ready (tier files mmapped) AND on the new
# height before touching primary. /ready — not /health — is the
# gate: /health returns 200 as soon as the listener binds, whereas
# /ready only flips to 200 once queries can be served.
until curl -sf --max-time 4 https://pir-backup.valargroup.org/ready > /dev/null; do
    echo "waiting for backup..."; sleep 5
done
ssh root@pir-backup.valargroup.org \
    'curl -sf http://localhost:3000/metrics | awk "/^nf_snapshot_(served|expected)_height/ {print}"'
# If expected > 0, served must be >= expected. If expected == 0,
# /ready plus served > 0 means "no active round; serving local snapshot".

# Primary
ssh root@pir-primary.valargroup.org sudo systemctl restart nullifier-query-server
until curl -sf --max-time 4 https://pir-primary.valargroup.org/ready > /dev/null; do
    echo "waiting for primary..."; sleep 5
done
```

Same convergence check at the end as in [Confirming convergence
externally](#confirming-convergence-externally).

## Related

- [`docs/runbooks/server-setup.md`](server-setup.md) — host-side
  install and configuration, including `/etc/default/nf-server`.
- [`docs/runbooks/ci-setup.md`](ci-setup.md) — CI/CD pipeline, GitHub
  secrets, and the Sentry-side wiring for the snapshot-stale alert.
- [Snapshot-bump runbook](https://valargroup.github.io/shielded-vote-book/operations/snapshot-bumps.html)
  in `shielded-vote-book` — end-to-end procedure for moving the fleet
  from one snapshot height to the next, of which this workflow is
  step 4.
- [`Publish nullifier snapshot`](https://github.com/valargroup/vote-nullifier-pir/actions/workflows/publish-snapshot.yml) — what to run before this workflow if no snapshot exists at the new height yet.
- [`Deploy nf-server`](https://github.com/valargroup/vote-nullifier-pir/actions/workflows/deploy.yml) — what to run instead of this workflow when shipping a new binary (it does the binary swap *and* the restart). Note: `deploy.yml` runs both hosts in parallel; if you need rolling order during a binary deploy, run it twice with `targets=backup` then `targets=primary`.
- Snapshot-stale watchdog: [`docs/runbooks/ci-setup.md#snapshot-stale-alerting`](ci-setup.md#snapshot-stale-alerting).
