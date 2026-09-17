# External PIR monitoring

`pir-probe` is a Python 3.11+ supervisor with an isolated Rust `pir-probe-query`
worker. The worker reuses `pir-client`; no new cryptography or server protocol is
introduced. Run it on the production explorer, outside the PIR hosts.

## Checks and trust boundary

`production.json` names four origins and two load balancers. Each minute the
supervisor checks `/ready` and, for origins, APM scrape freshness. Every five
minutes it resolves the production config layout and performs one fresh private
query against each target, sequentially. The Rust worker checks dataset/layout
compatibility, verifies the proof, and binds it to the root obtained during client
initialization. It rejects the wrong network. If the snapshot changes during a
query it retries once, within the parent's 30-second process deadline.

This is a functional consistency check against **server-advertised roots**. It is
not independent verification of a coordinator-authorized snapshot, nor a security
audit of PIR. Synthetic values come from OS randomness reduced through the existing field
library; no wallet data, nullifiers, queries, or proofs are logged. Logs include public
snapshot roots, heights, timings, and fixed error classifications only.

The APM compatibility adapter reads the absolute timestamp from `.updated` in the
existing dashboard. More than 90 seconds stale, more than 30 seconds in the future,
missing markup, and unparseable timestamps are failures, never silent passes.
Update its fixture test if the dashboard format changes.

HTTP operations also run in killable subprocesses: socket inactivity timeouts
alone do not bound a slow/trickling response. TLS validation is on; redirects are
rejected. Query responses have size limits. The parent kills a hung query, and a
loop failure terminates the service so systemd can restart it.

## Alert state and secrets

Two consecutive failures open availability/query incidents; invalid proofs and
inconsistent responses open immediately. Two successes recover an incident.
State, reminders, and an ordered notification queue survive restarts in
`/var/lib/pir-probe/status.json`. Reminders occur every 30 minutes; at most one
reminder per check remains queued. Slack requests have a 10-second deadline and
retry from 5 seconds up to 5 minutes. Recovery never overtakes its failure alert.
Delivery is at-least-once: a crash after Slack accepts but before the state write
can duplicate a message. A malformed state file fails closed rather than silently
forgetting incidents. Use a separate state directory for commissioning/tests.

Root-owned `/etc/default/pir-probe` supplies:

- `PIR_PROBE_SLACK_WEBHOOK_URL`: sourced from vote/prod `PIR_APM_SLACK_WEBHOOK_URL`.
- `PIR_PROBE_HC_AVAILABILITY`: period 60s, grace 120s.
- `PIR_PROBE_HC_QUERY`: period 300s, grace 300s.
- `PIR_PROBE_HC_DELIVERY`: period 60s, grace 120s.

Cycle heartbeats mean all targets produced outcomes, not that all targets passed.
A config failure prevents the query heartbeat. The delivery check sends `/fail`
when the oldest notification is at least two minutes overdue. With no pending
notifications it reports delivery-worker liveness, not a fresh end-to-end Slack
test. The Healthchecks Slack integration must be independent of the probe webhook.
Missing heartbeat URLs are explicitly reported at startup; in that configuration
the independent dead-man protection is **not operational**.

The existing watchdog accepts `WATCHDOG_HEARTBEAT_URL`, period 300s/grace 300s.
Its implementation lives in the accompanying vote-infrastructure change. The
Healthchecks management key is used only for provisioning, never deployed.

## Build, install and rollback

Build the query worker with Rust 1.91 and the checked-in lockfile. The supplied
`Dockerfile.build` uses an arm64 build host with a baseline x86-64 cross compiler;
it does not require AVX-512 on the explorer. Native Linux amd64 builders can simply
run `cargo build --locked --release -p pir-probe-query`.

Bundle `pir-probe`, `pir-probe-query`, `commission.py`, `fault-test.py`,
`production.json`, `pir-probe.service`, `BUILD.json`, and `SHA256SUMS` with
`install.sh`. Include the source revisions/hashes and compiler/target in
`BUILD.json`. Copy over SSH and run `install.sh BUNDLE VERSION` as root. It verifies
checksums, installs a versioned release, atomically switches `current`, and installs
but does not start the service. Preserve the prior release and configuration.

Supply secrets from Infisical over SSH stdin, mode 0600, without printing them.
Run `commission.py --config /etc/pir-probe/config.json --state-dir SEPARATE_DIRECTORY
--cycles 3` under the unit's CPU/memory/user limits. Canary primary-beta first,
then all targets. Commissioning runs accelerated cycles and emits no notifications
or heartbeats. Enable the daemon with `systemctl enable --now pir-probe` once those
checks pass. Normal polling uses the configured 60s/300s intervals.

Healthchecks provisioning: inject `HEALTHCHECKS_API_KEY` from vote/prod and run
`provision-healthchecks.py --list`, then `--channel-id ID` selecting #thv-alerts.
The script creates/updates the four checks and stores their ping URLs in Infisical
before use. It refuses duplicate names or a non-Slack integration. Enable heartbeats
only after the checks' independent Slack integration is configured.

Rollback: stop `pir-probe`, point `/opt/pir-probe/current` to the recorded previous
release, restore its configuration/unit, daemon-reload and start if returning to a
previous running version. Pause unused Healthchecks checks. Restore the saved
watchdog binary/environment if rolling back that change. No PIR server binary,
Caddy configuration, snapshot, or signed updater state is changed by this rollout.

## Validation and operations

- `cargo test -p pir-probe-query` checks valid proofs, corrupt paths, wrong roots,
  nullifier boundaries, and snapshot identity.
- `python3 -m unittest discover -s deploy/pir-probe -p test_probe.py` checks failure
  episodes, restart deduplication, recovery ordering, retries, stale/unreadable APM,
  malformed readiness, config errors, and killable worker deadlines.
- `fault-test.py` uses an isolated local HTTP server and temporary state. With
  `--send-alerts`, it sends four explicitly labelled test messages through the
  configured webhook: HTTP failure/recovery and invalid-proof classification/
  recovery. It never breaks the production servers or writes production state.
- `pir-probe query-once --url https://pir-primary-beta.valargroup.org` runs a
  single functional check using the configured layout.
- Inspect `systemctl status pir-probe`, `journalctl -u pir-probe`, and the status
  JSON. Inspect Healthchecks last-ping/status and confirm receipt in #thv-alerts.
- Acceptance for dead-man monitoring requires stopping only the new probe long
  enough for an actual missed-check-in alert, then observing its recovery. Do not
  simulate a server outage by stopping `nf-server`.
