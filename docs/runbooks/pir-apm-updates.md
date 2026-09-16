# PIR APM sidecar updates

`pir-apm` is the local monitoring sidecar. It scrapes `nf-server`, renders the
`/apm/` dashboard, and raises Slack alerts. It never answers a PIR query.

## Why this is a separate service

`nf-server` is installed by the coordinator-signed updater. `pir-apm` used to be
installed only by the `deploy.yml` fleet deploy. The two advanced on different
schedules, so they drifted: staging reached `nf-server` v0.12.0-rc.5 while its
sidecar sat at v0.0.45 from a month earlier. The stale sidecar kept paging on
upload-inclusive Tier1 latency long after the server split that metric, while
the correct processing histogram sat exported and unread beside it.

Once every host enrolled in signed updates, `deploy.yml` also became unable to
run anywhere, because its preflight refuses enrolled hosts. That left the
sidecar with no automated update path at all.

`pir-apm-updater` closes both gaps. It follows the same `binary_tag` as the
serving updater, so the sidecar and the server converge on one pointer and
cannot drift apart.

## Trust boundary

**The coordinator does not sign the sidecar, and should not.** `pir-apm` is not
on the serving path, so putting it in the coordinator's signing scope would
widen what that key authorizes for no benefit, and would force a signing-message
schema change across every verifier that shares the test vector.

This updater therefore reads `pir.json` for its `binary_tag` and **does not
fetch or verify `pir_attestations.json`**. Integrity comes from the release's
`SHA256SUMS`, the same anchor `deploy.yml` has always used for this binary.

What that admits, stated plainly: someone able to tamper with `pir.json` in
transit can pin the sidecar to a different *genuine* Valargroup release, because
the binary must still match that release's `SHA256SUMS`. That degrades
monitoring fidelity; it does not compromise serving. Two guards bound it:

- `binary_tag` is matched against `^v[0-9]+\.[0-9]+\.[0-9]+(-rc\.[0-9]+)?$`
  before it reaches a URL, so a crafted value cannot redirect the download.
- An asset absent from `SHA256SUMS` fails closed rather than installing
  unverified.

Downgrades are followed deliberately. A coordinator-signed rollback of
`nf-server` moves `binary_tag` backwards, and the sidecar must follow it or it
would strand itself ahead of the server — reintroducing the exact skew this
service exists to prevent.

## Decoupling guarantees

- Own settings (`/opt/pir-apm-updater/settings.json`), own state
  (`/var/lib/pir-apm-updater/state.json`), own lock
  (`/run/lock/pir-apm-update.lock`).
- Never reads `/opt/pir-updater`, never touches `nf-server`, its unit, its
  drop-in or its generations, and never takes the serving lock.
- Activation failure restores the previous binary and unit and restarts; it
  cannot affect query availability.
- `/etc/default/pir-apm` is read for the listen address and never written, so
  the Slack webhook and dashboard settings stay under operator control.

## Fleet-only by construction

Valargroup fleet hosts run this service. Integrators do not install it, and so
never receive a `pir-apm` binary or unit. There is no flag to set and nothing to
opt out of: a host without the service is simply unmanaged.

## Install

```bash
sudo scripts/install_pir_apm_updater.sh \
  --config-url https://voting.valargroup.dev/stage/pir.json
```

Use the `prod` URL on production hosts. The installer writes the unit and timer,
enables the timer, and reconciles once immediately.

## Operation

The timer polls every 5 minutes with up to 60s of jitter so hosts do not fetch
in lockstep. Each tick resolves `binary_tag`, exits if it already matches the
installed tag, and otherwise verifies, smoke-tests, installs, restarts, and
health-checks the sidecar before recording the tag.

Inspect a host:

```bash
cat /var/lib/pir-apm-updater/state.json     # installed_tag, last_check, error
systemctl list-timers pir-apm-updater.timer
journalctl -u pir-apm-updater -n 50
```

Force a reconcile:

```bash
sudo python3 /opt/pir-apm-updater/pir_apm_updater.py --once
```

Pause updates without uninstalling:

```bash
sudo systemctl disable --now pir-apm-updater.timer
```

## Failure behaviour

A failed poll records `error` in `state.json` and retries on the next tick. A
sidecar that does not become healthy after install is rolled back to the
previous binary and unit automatically.

If the sidecar ends up stale or stopped despite this, it still surfaces:
`pir-apm` raises `tier1_query_processing_metric_missing` when Tier1 serves
traffic but exports no processing histogram, rather than failing open on a
latency check that can never fire.
