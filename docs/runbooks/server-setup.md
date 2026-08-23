# Runbook: Setup PIR Server

## Overview

Vote-nullifier PIR lets a client prove that a Zcash Ironwood nullifier is **not** in the on-chain Ironwood nullifier set, without revealing *which* nullifier it is asking about. This service is a building block for shielded voting.

This runbook covers the operator side: standing up an `nf-server` host that answers PIR queries from clients over HTTP. One server is a single `nf-server` binary listening on a single port (default `3000`); see [Recommended hardware](#recommended-hardware) for the target SKU.

**Who this runbook is for:**

- **Operators**: use the release binary + systemd path. The one-liner below is the shortcut; the rest of the runbook leads with that path and uses the installed `nf-server` binary directly.
- **Custom-layout / non-Linux**: see [Manual install](#manual-install-no-start_pirsh).
- **Developers** iterating from a source checkout (`cargo run`, `make sync`, `make serve`): see [CONTRIBUTING.md](../../CONTRIBUTING.md). Those workflows are intentionally out of scope here.

There are two data-source modes the server can run in:

1. **Bootstrapped** — the PIR server downloads pre-computed snapshot data from Valar Group–hosted object storage. This is the **default** mode under the shipped systemd unit.
2. **Synced** — the PIR server runs `nf-server sync`: stream Ironwood nullifiers from lightwalletd up to a chosen height (or chain tip), materialize a versioned `nullifiers.tree` checkpoint, then write the 12+7 two-tier representation per [PIR tree spec](../pir-tree-spec.md). Each stage resumes from compatible on-disk artifacts after failure. Operators run `nf-server sync` ad-hoc; the systemd unit only covers `serve`.

## Quick start

On Linux, we recommend using this one-CLI command to get started:

```bash
curl -fsSL https://shielded-vote.nyc3.digitaloceanspaces.com/start_pir.sh \
  | sudo env SVOTE_ZCASH_NETWORK=main bash
```

Use `main` for production and `test` for staging.

What it does:
- Downloads the latest binaries and verifies `nf-server` against `SHA256SUMS` for the pinned release.
- Configures the service per the recommended parameters
- Creates an automated **systemd** unit that auto-restarts on start-up and on failure
- Bootstraps from pre-computed snapshots
- Installs the binary to `/opt/nf-ingest/nf-server` and symlinks it into `/usr/local/bin`, so `nf-server doctor` (and friends) work from any shell
- Serves PIR queries

After install, operate the service with:

```bash
systemctl status nullifier-query-server
systemctl restart nullifier-query-server
journalctl -u nullifier-query-server -f
```

See [Smoke test](#smoke-test) for a post-install check.

## Recommended hardware

**Production target: `linux-amd64` with AVX-512, 4 vCPU, 32 GB RAM, and at least 1 GiB free disk plus twice the measured `tier1.precompute` size.** Other platforms build but are not recommended for serving traffic (see [Platform support](#platform-support)).

Why these numbers:

- AVX-512 meaningfully accelerates PIR packing and query-side linear algebra; without it, queries fall back to the scalar path.
- 4 vCPUs parallelize the matrix–vector steps that dominate query latency.
- **Disk budget:**
  - 48 MiB `tier1.bin` plus a 384 KiB `tier0.bin`
  - one derived `tier1.precompute` cache, written by `serve` after first YPIR setup
  - transient cache-rewrite space because atomic `.tmp` + rename briefly keeps old and new files
  - nullifier data and working space, which grow with Ironwood usage
  - 1 GiB is the baseline for tier data and snapshot staging; after the first cold boot, reserve at least twice the observed cache size in addition so an atomic cache rewrite cannot exhaust the volume
  - On synced hosts, re-check `nullifiers.bin` size before each snapshot rotation.

Verify a candidate host with [`nf-server doctor`](#host-health-check-nf-server-doctor) before installing.

## Network requirements

The server needs the following network access:

| Direction | Destination | Purpose |
|-----------|-------------|---------|
| Outbound 443 | `shielded-vote.nyc3.digitaloceanspaces.com` | Binary, `SHA256SUMS`, `start_pir.sh`, snapshot tier downloads |
| Outbound 443 | `github.com`, `objects.githubusercontent.com` | Binary / unit-file fallback |
| Outbound 443 | `voting.valargroup.dev` | PIR snapshot config and static/dynamic voting config fallback |
| Outbound 443 | `sentry.io` (DSN-specific host) | Optional — only when `SENTRY_DSN` is set |
| Outbound 443 | lightwalletd (e.g. `us.zec.stardust.rest:443`) | **Synced mode only** |
| Inbound 3000 | client / reverse proxy | PIR query traffic |

## Platform support

- **`linux-amd64`** — recommended production target. Requires AVX-512; older Intel/AMD CPUs will SIGILL on startup. Run `nf-server doctor` first to confirm.
- **`linux-arm64`** — supported but slower (no PIR-specific SIMD); not recommended for serving production traffic.
- **`darwin-arm64`** — recommended for local dev on Apple Silicon.
- **`darwin-amd64`** — dev-only: ships without the `serve` subcommand. Use for `doctor` / `sync` on Intel Macs only.

## Install

`start_pir.sh` is the recommended path; the rest of this section documents what it does so you can reproduce or audit it manually.

### Release artifacts

Each `v*` release publishes the `nf-server-<platform>` binary, `SHA256SUMS`, and `nullifier-query-server.service` to **DigitalOcean Spaces** (primary) with **GitHub Releases** as a fallback. `start_pir.sh` tries Spaces first, then GitHub. The release workflow derives the public Spaces origin from `DO_BUCKET` + `DO_REGION` unless `DO_PUBLIC_BASE_URL` is set. The workflow default is `https://shielded-vote.nyc3.digitaloceanspaces.com`; override `DO_BUCKET`, `DO_REGION`, or `DO_PUBLIC_BASE_URL` only when publishing to a different bucket. Exact URL patterns are in the curl commands in [Manual install](#manual-install-no-start_pirsh).

`start_pir.sh` is served at these paths under the configured Spaces origin:

- `<spaces-origin>/scripts/start_pir/<tag>/start_pir.sh` — pinned to any release tag, including an RC.
- `<spaces-origin>/start_pir.sh` — the latest stable release.
- `<spaces-origin>/scripts/start_pir/<snapshot_height>/start_pir.sh` — the stable release matching a voting `snapshot_height`.

`update_pir.sh` is served alongside it for existing `start_pir.sh` hosts:

- `<spaces-origin>/update_pir.sh` — updates to the latest stable release while preserving existing config and data files.
- `<spaces-origin>/scripts/update_pir/<tag>/update_pir.sh` — pinned to a specific release tag for rollback or reproducible testing.

For a coordinated release, set the repository variable `RELEASE_HOLD_TAG` to
the stable tag before pushing it. The release workflow publishes GitHub and
tag-scoped Spaces artifacts without changing GitHub Latest, the unversioned
installer aliases, or the snapshot-height installer alias. When the release is
ready to become the default, run the **Promote release** workflow with that
exact tag. It verifies the artifacts, updates all mutable aliases, marks the
GitHub release Latest, and verifies the result. `RELEASE_HOLD_TAG` can remain
as an audit marker because it affects only that exact tag; replace it before
the next coordinated release. Promotion refuses to replace a newer stable
release that has since become Latest, and it fails unless the current
snapshot-height installer alias can be published and verified.

For an ordinary stable release, GitHub publishes the release without changing
Latest, updates and verifies the mutable Spaces aliases, and only then marks the
release Latest. Rerunning the current Latest tag preserves that status while the
aliases are republished.

Promotion changes future installer and updater resolution. It does not install
or restart `nf-server` on any running host.

Prereleases publish only the tag-scoped paths and do not change the stable aliases.

### Manual install (no `start_pir.sh`)

For custom layouts, non-Linux platforms, or when debugging the installer:

**Prerequisites (Linux):** `systemd`, `curl`, and CA certificates for HTTPS. On minimal Ubuntu/Debian images install them first (the one-liner installer does this automatically):

```bash
sudo apt-get update && sudo apt-get install -y curl ca-certificates jq
```

`jq` is optional but matches the [Smoke test](#smoke-test) commands below.

1. **Download the binary** for your platform from one of the URLs above. Save it as `/tmp/nf-server-${PLATFORM}` (the name used in `SHA256SUMS`) regardless of whether you pull from Spaces or GitHub:

   ```bash
   PLATFORM=linux-amd64        # or linux-arm64, darwin-arm64, darwin-amd64
   TAG=v0.x.y                  # pin the release tag
   ZCASH_NETWORK=main          # test for staging
   LWD_URLS=https://example-lightwalletd:443
   case "$ZCASH_NETWORK" in main) CONFIG_ENV=prod ;; test) CONFIG_ENV=stage ;; *) exit 1 ;; esac
   PIR_DATA_DIR="/opt/nf-ingest/pir-data/${ZCASH_NETWORK}"
   PIR_SPACES_BASE="${PIR_SPACES_BASE:-https://shielded-vote.nyc3.digitaloceanspaces.com}"

   curl -fL -o "/tmp/nf-server-${PLATFORM}" \
     "${PIR_SPACES_BASE}/binaries/vote-pir/nf-server-${TAG}-${PLATFORM}" \
     || curl -fL -o "/tmp/nf-server-${PLATFORM}" \
       "https://github.com/valargroup/vote-nullifier-pir/releases/download/${TAG}/nf-server-${PLATFORM}"

   curl -fL -o /tmp/SHA256SUMS \
     "${PIR_SPACES_BASE}/binaries/vote-pir/SHA256SUMS-${TAG}" \
     || curl -fL -o /tmp/SHA256SUMS \
       "https://github.com/valargroup/vote-nullifier-pir/releases/download/${TAG}/SHA256SUMS"

   ( cd /tmp && sha256sum -c SHA256SUMS --ignore-missing )

   sudo install -d /opt/nf-ingest "$PIR_DATA_DIR"
   sudo install -m 0755 "/tmp/nf-server-${PLATFORM}" /opt/nf-ingest/nf-server
   ```

2. **Sanity-check** the binary and the host:

   ```bash
   /opt/nf-ingest/nf-server --version
   /opt/nf-ingest/nf-server doctor --pir-data-dir "$PIR_DATA_DIR"
   ```

3. **Download the systemd unit** and install it:

   ```bash
   sudo curl -fL -o /etc/systemd/system/nullifier-query-server.service \
     "https://github.com/valargroup/vote-nullifier-pir/releases/download/${TAG}/nullifier-query-server.service"
   ```

4. **Write the env files** the unit reads (see [Configuring the service](#configuring-the-service) for the full layout). **Do not indent the lines inside the heredoc** — leading spaces would end up in the file and break `EnvironmentFile` parsing.

   ```bash
   PIR_PRECOMPUTED_BASE_URL=https://shielded-vote.nyc3.digitaloceanspaces.com
   sudo tee /etc/default/nf-server >/dev/null <<EOF
SVOTE_ZCASH_NETWORK=${ZCASH_NETWORK}
LWD_URLS=${LWD_URLS}
SVOTE_PIR_DATA_DIR=${PIR_DATA_DIR}
SVOTE_PIR_CONFIG_URL=https://voting.valargroup.dev/${CONFIG_ENV}/pir.json
SVOTE_PIR_VOTING_CONFIG_URL=https://voting.valargroup.dev/${CONFIG_ENV}/static-voting-config.json
SVOTE_PIR_PRECOMPUTED_BASE_URL=${PIR_PRECOMPUTED_BASE_URL}
EOF

   # Optional: Sentry DSN for observability (see configuration below)
   sudo install -d -m 0755 /opt/nf-ingest
   echo "SENTRY_DSN=https://…@…ingest.sentry.io/…" | sudo install -m 0600 /dev/stdin /opt/nf-ingest/.env
   ```

5. **Enable and start** the service:

   ```bash
   sudo systemctl daemon-reload
   sudo systemctl enable --now nullifier-query-server
   sudo systemctl status nullifier-query-server
   curl -fsS http://127.0.0.1:3000/ready
   ```

## Smoke test

After install, verify end-to-end without a real client:

```bash
curl -fsS http://127.0.0.1:3000/ready                                   # 200 OK
curl -fsS http://127.0.0.1:3000/health | jq -r '.status'                # ok (starting / rebuilding / error while warming)
curl -fsS http://127.0.0.1:3000/root   | jq '{zcash_network, nullifier_pool, dataset_version, height, pir_layout, pir_depth, tier1_rows, tier1_row_bytes, num_ranges}'
```

`GET /health` returns a stable `status` string derived from the internal server phase. For a structured `phase` object (e.g. `{ "phase": "Starting", ... }`), probe `GET /ready` while the server is still warming — it returns **503** with that JSON body until the process reaches `Serving`.

Confirm `/root` reports the configured network, `nullifier_pool: "ironwood"`,
`dataset_version: 2`, `pir_layout: { pir_depth: 19, tier0_layers: 12,
tier1_layers: 7 }`, `pir_depth: 19`, `tier1_rows: 4096`, and
`tier1_row_bytes: 12288`.
Its `height` should match `snapshot_height` from the environment's published
`pir.json` while bootstrap is enabled.

## Host health check (`nf-server doctor`)

Before provisioning or when debugging a host, run:

```bash
nf-server doctor
```

Use the same PIR data root as `serve` / `sync` (defaults to `./pir-data`; override with `--pir-data-dir` or `SVOTE_PIR_DATA_DIR`):

```bash
nf-server doctor --pir-data-dir /opt/nf-ingest/pir-data/main
```

The command prints logical CPU count, system RAM, free space on the volume backing the data directory, and (on x86_64) whether AVX-512F is visible at runtime. It compares these to the recommendations above and prints `WARN: …` lines to stderr when something is undersized or missing; **exit status is always 0** so automation and CI can run it as a smoke check without failing undersized dev machines.

Production binaries should be built with `--features serve` (and `--features avx512` on capable hardware); `doctor` notes when those compile-time features are off.

## Bootstrapped mode

This is what the shipped systemd unit runs by default. After install, the service is already enabled and started; nothing more to do for the happy path.

```bash
systemctl status nullifier-query-server
journalctl -u nullifier-query-server -f
```

To re-bootstrap (for example after editing `/etc/default/nf-server` or after a binary upgrade):

```bash
systemctl restart nullifier-query-server
```

**Startup time:** roughly two phases.

- **Cold (no `tier1.precompute` cache present)**: download the 48 MiB database, run one YPIR precomputation, and write the cache.
- **Warm (`tier1.precompute` present and valid)**: load the cache and skip YPIR offline precomputation.

Cache invalidation is automatic: any change to `tier{N}.bin` (sync rebuild, bootstrap snapshot rotation, manual edit) invalidates the corresponding cache via content hash, and the server falls back to recompute. Operators do not manage these files.

**On startup**, `serve` fetches the environment's PIR snapshot config
(`SVOTE_PIR_CONFIG_URL`, for example `https://voting.valargroup.dev/prod/pir.json`),
reads its `snapshot_height`, compares that height and the Ironwood dataset
identity to local `pir_root.json`, and downloads matching snapshot tiers from
`SVOTE_PIR_PRECOMPUTED_BASE_URL/snapshots/<network>/<height>` if they don't match. For zero-touch migration, hosts that only have the legacy
`SVOTE_PIR_VOTING_CONFIG_URL` derive `prod/pir.json` or `stage/pir.json` when
that URL clearly identifies an environment; ambiguous legacy URLs fall back to
the old active-round discovery path. With default workflow settings, existing
installs use the `shielded-vote` bucket in `nyc3`.

The compiled fallback for `SVOTE_PIR_PRECOMPUTED_BASE_URL` points at
`https://shielded-vote.nyc3.digitaloceanspaces.com`. Set the variable in
`/etc/default/nf-server` only when serving from a different bucket, or publish
`start_pir.sh` with matching `DO_BUCKET`, `DO_REGION`, and `DO_PUBLIC_BASE_URL`
values so fresh installs write that value automatically.

If no configured snapshot height is available, `serve` keeps serving a
compatible local snapshot. On a fresh host with no local `pir_root.json`, set
`SVOTE_PIR_FORCE_SNAPSHOT_HEIGHT=<height>` to bootstrap a specific published
snapshot. The force setting bypasses PIR/voting-config discovery, so remove it
after the host has loaded the intended snapshot unless the override is still
desired.

**Policy:** if local tier state is unusable and bootstrap can't fix it (e.g. CDN fetch failed and no valid files under `SVOTE_PIR_DATA_DIR`), startup fails. Fix the network / configuration, fall back to [Synced mode](#synced-mode), or pre-stage files.

To disable bootstrap entirely (offline / pre-staged tiers), set `SVOTE_PIR_CONFIG_URL=`.

For startup phase semantics, error symptoms, and recovery, see [Troubleshooting](#troubleshooting).

## Synced mode

The shipped systemd unit only covers `serve`; sync is operator-driven. Stop the service, run `nf-server sync` against the same data directory, then start the service again:

```bash
systemctl stop nullifier-query-server
# Optional: load the same env as systemd so sync picks up the active-round height cap
# sudo set -a && . /etc/default/nf-server && set +a

CONFIG=$(curl -fsSL https://voting.valargroup.dev/prod/static-voting-config.json)
DYNAMIC_CONFIG_URL=$(jq -r '.dynamic_config_url // empty' <<<"$CONFIG")
if [ -n "$DYNAMIC_CONFIG_URL" ]; then
    CONFIG=$(curl -fsSL "$DYNAMIC_CONFIG_URL")
fi
VOTE_SERVER=$(jq -r '.vote_servers[0].url' <<<"$CONFIG")
SNAPSHOT=$(curl -fsSL "${VOTE_SERVER%/}/shielded-vote/v1/rounds/active" | jq -r '.round.snapshot_height')

sudo env SVOTE_ZCASH_NETWORK=main LWD_URLS="$LWD_URLS" \
  /opt/nf-ingest/nf-server sync \
    --pir-data-dir /opt/nf-ingest/pir-data/main \
    --non-interactive \
    --max-height "$SNAPSHOT"
systemctl start nullifier-query-server
```

Set `LWD_URLS` to post-NU6.3 endpoints for `SVOTE_ZCASH_NETWORK`. Staging uses public testnet endpoints; production uses mainnet endpoints.

Useful flags:

- `--non-interactive` — required from CI / unattended SSH (no TTY prompts).
- `--invalidate-after-blocks` — force `nullifiers.tree` and tier blobs to rebuild when new blocks stream in.
- `--max-height <H>` — stop at `H` (must be a multiple of 10). Without it, syncs to the configured network tip, capped by the active round snapshot height when bootstrap is enabled.

`nf-server sync` runs three resumable stages: stream nullifiers from lightwalletd → build `nullifiers.tree` → write tier files (`tier0.bin`, `tier1.bin`, `pir_root.json`). Rerunning after partial failure picks up where it stopped when the dataset identity matches.

Unlabeled, wrong-network, and legacy Orchard data cannot be resumed. Use a separate data directory for each network and set `SVOTE_PIR_SYNC_RESET=1` when rebuilding incompatible data.

Dataset version 2 also changes the `nullifiers.dataset.json` identity. The
first migration from version 1 requires `SVOTE_PIR_SYNC_RESET=1` and a full
nullifier re-stream; deleting only the tree and tier files is insufficient.

**Sync time** is governed by lightwalletd nullifier streaming, not local CPU, and grows with chain length from Ironwood activation.

After sync, tier files are local — but CDN bootstrap still runs on the next `serve` startup unless you disable it (`SVOTE_PIR_CONFIG_URL=` in `/etc/default/nf-server`).

### Height-mismatch wipe (`RESYNC`)

When bootstrap is enabled and your local nullifier checkpoint is **above** the active round `snapshot_height`, `nf-server sync` refuses to silently roll back. Confirm by typing `RESYNC` at the prompt, or — under `--non-interactive` — set:

```bash
SVOTE_PIR_SYNC_ACK_HEIGHT_MISMATCH=RESYNC
```

This wipes `nullifiers.bin`, `nullifiers.dataset.json`, the checkpoint, the index, `nullifiers.tree`, and tier files, then re-syncs from scratch.

## Configuring the service

The release ships `nullifier-query-server.service` and `start_pir.sh` installs it to `/etc/systemd/system/`. The unit:

- runs `Type=simple` with `Restart=on-failure` and `RestartSec=30`;
- has `WorkingDirectory=/opt/nf-ingest`;
- `ExecStart=/opt/nf-ingest/nf-server serve --port 3000`;
- pulls environment from two files (both optional, `EnvironmentFile=-…`):
- `/etc/default/nf-server` — operator / cloud-init owned. Holds required `SVOTE_ZCASH_NETWORK`, network-specific `SVOTE_PIR_DATA_DIR`, and bootstrap URLs.
  - `/opt/nf-ingest/.env` — deploy-workflow owned. Holds `SENTRY_DSN`, `SENTRY_ENVIRONMENT`, and `SENTRY_RELEASE`. Mode `0600`.

To change settings, edit the appropriate env file and:

```bash
systemctl daemon-reload   # only after editing the .service file itself
systemctl restart nullifier-query-server
```


### TLS / reverse proxy

`nf-server` speaks plaintext HTTP on `--port`; clients should reach it over TLS. Terminate TLS in a reverse proxy on the same host (or upstream LB). Minimal Caddy example:

```caddyfile
pir.example.org {
    reverse_proxy 127.0.0.1:3000
    # Restrict debug rows to internal callers; clients don't need them.
    @debug path /tier1/row/*
    handle @debug { respond 403 }
}
```

Caddy obtains a certificate automatically. For nginx, use any standard `proxy_pass` config to `127.0.0.1:3000` and block the `/tier1/row/*` path.

## Observability

**Prometheus**: scrape `GET /metrics` on the serve port. Useful signals to alert on:

- `up{job="nf-server"} == 0` for >1 m — process down (or scrape failing).
- `/ready` returning 503 for >5 m via blackbox probing — stuck out of the `Serving` state.
- Snapshot staleness past `SVOTE_PIR_STALE_THRESHOLD_SECS` (also surfaces via Sentry when configured).

Browse `/metrics` once after install for the full series list; names are stable across patch releases.

Fleet deploys also install the localhost-only `pir-apm` sidecar. It scrapes
`/metrics`, `/health`, and `/ready` every 15 seconds, renders service/host
health and tier0/tier1 latency at `/apm/` on the existing HTTPS host, and
sends coded-threshold incidents and recoveries to `#thv-alert`. Caddy basic
auth protects the dashboard and blocks public access to `/metrics`; direct
port 3002 access is not exposed by the firewall.

The request metrics are deliberately limited to fixed endpoint, method, and
status labels. They never include a client IP, request ID, User-Agent, or
other request header. See [`../../deploy/README.md`](../../deploy/README.md)
for sidecar configuration and alert thresholds.

**Sentry**: optional. Create a project at [sentry.io](https://sentry.io), set `SENTRY_DSN` in `/opt/nf-ingest/.env`, set `SENTRY_ENVIRONMENT` to `staging` or `production`, and set `SENTRY_RELEASE` to the deployed release tag. Sentry tracing is disabled; only process errors and explicit operational messages are sent, so proxy-added client identity headers never leave the PIR service. The in-process snapshot watchdog emits stale-snapshot events when `SVOTE_PIR_STALE_THRESHOLD_SECS` is non-zero; `SVOTE_PIR_WATCHDOG_TICK_SECS` controls how often it checks.

**Logs**: the server logs to stdout; `journalctl -u nullifier-query-server -f` follows them. Verbosity is controlled by `RUST_LOG` (e.g. `RUST_LOG=info,nf_server=debug`); set it in `/etc/default/nf-server` and restart.

## Backup and disaster recovery

`SVOTE_PIR_DATA_DIR` is **disposable** for bootstrapped hosts: tier files come from the CDN and the snapshot height is fixed by the environment `pir.json` (or the legacy active-round fallback for older host configs). To recover, reinstall and restart — `start_pir.sh` and the systemd unit will re-bootstrap. No backups required for serve-only hosts.

For synced hosts, back up `nullifiers.bin`, `nullifiers.dataset.json`, `nullifiers.checkpoint`, and `nullifiers.index` together if you want to skip a re-stream after disk loss. Tier files are derivable.

## Upgrading

For hosts already installed by `start_pir.sh`, use the latest-release updater when you only want to refresh the release-managed artifacts and preserve local configuration:

```bash
curl -fsSL https://shielded-vote.nyc3.digitaloceanspaces.com/update_pir.sh \
  | sudo bash
```

The updater reads the installed network from `/etc/default/nf-server` and refuses to change it. It preserves existing URL and Sentry settings, adds the network-specific data directory, refreshes the binary and unit, then waits for `/ready`.

For a legacy installation with no `SVOTE_ZCASH_NETWORK` entry, provide it once during migration:

```bash
curl -fsSL https://shielded-vote.nyc3.digitaloceanspaces.com/update_pir.sh \
  | sudo env SVOTE_ZCASH_NETWORK=test bash
```

Use `test` for staging and `main` for production.

Useful options:

```bash
# Reinstall and restart even when the installed binary already matches.
curl -fsSL https://shielded-vote.nyc3.digitaloceanspaces.com/update_pir.sh | sudo bash -s -- --force

# Install a specific release tag.
curl -fsSL https://shielded-vote.nyc3.digitaloceanspaces.com/update_pir.sh | sudo bash -s -- --tag v0.x.y
```

For a full reconfiguration back to the published defaults, re-run `start_pir.sh`; it is idempotent but rewrites `/etc/default/nf-server`. For custom layouts, repeat steps 1–2 of [Manual install](#manual-install-no-start_pirsh) with a new `TAG` (re-download the binary, re-check `SHA256SUMS`, reinstall to `/opt/nf-ingest/nf-server`, run `doctor`), then `sudo systemctl restart nullifier-query-server`. If the unit file itself changed in the new release (re-download it in step 3), also run `sudo systemctl daemon-reload` before the restart.

## Tagging and releases

Semantic versioning applies to `nf-server` releases (`v*` tags drive CI artifacts). Integrators should pin both the **binary version** and the **voting snapshot height** they expect.

**When to upgrade:**

- The environment `pir.json` is raised to a new `snapshot_height`. A bootstrapped server picks it up on next restart, but you should also confirm the pinned binary is still supported.
- A new `v*` release with security or correctness fixes (watch GitHub Releases; subscribe via the repo's release feed).
- Otherwise, no need to chase patch releases mid-round.

For pinned-snapshot installs, use the per-snapshot `start_pir.sh` URL under the configured Spaces origin, for example `https://shielded-vote.nyc3.digitaloceanspaces.com/scripts/start_pir/<snapshot_height>/start_pir.sh`.

## Configuration reference

### `nf-server serve` (CLI / env)

Variables the shipped systemd unit honors. Set them in `/etc/default/nf-server` (or, for `SENTRY_DSN`, `/opt/nf-ingest/.env`). Run `nf-server serve --help` for the full list.

**Common:**

| Variable | Role |
|----------|------|
| `SVOTE_ZCASH_NETWORK` | Required Zcash network: `main` or `test`. |
| `SVOTE_PIR_DATA_DIR` | Network-specific on-disk root. Fleet hosts use `/opt/nf-ingest/pir-data/<network>`. |
| `SVOTE_PIR_PORT` | HTTP listen port. Unit overrides via `--port 3000`. |
| `SVOTE_PIR_CONFIG_URL` | Environment PIR snapshot config URL. Empty string disables bootstrap (offline / pre-staged tiers). New installs use `https://voting.valargroup.dev/prod/pir.json` by default. |
| `SVOTE_PIR_VOTING_CONFIG_URL` | Legacy static voting-config URL. Used to derive the PIR config URL on old hosts and as a fallback active-round discovery path for ambiguous configs. |
| `SVOTE_PIR_PRECOMPUTED_BASE_URL` | CDN base URL for tier downloads. Defaults to `https://shielded-vote.nyc3.digitaloceanspaces.com`. |
| `SVOTE_PIR_FORCE_SNAPSHOT_HEIGHT` | Optional operator override for bootstrapping and serving one specific published snapshot height. Bypasses PIR/voting-config discovery while set. |
| `SVOTE_PIR_STALE_THRESHOLD_SECS` | Snapshot-staleness threshold for the watchdog (Sentry alerts gated on `SENTRY_DSN`). |
| `SENTRY_DSN` | Enables Sentry reporting for startup, load, and snapshot-watchdog failures plus one-time snapshot recovery. Successful readiness logs locally. Sentry tracing is disabled. Lives in `/opt/nf-ingest/.env` (mode `0600`). |
| `SENTRY_ENVIRONMENT` | Tags Sentry events as `staging` or `production`. Lives in `/opt/nf-ingest/.env` and defaults to `production` if omitted. |
| `SENTRY_RELEASE` | Tags Sentry events with the deployed release tag. The deploy workflow sets this from `release_tag`; if omitted, the binary falls back to the compiled Cargo package version. |

**Advanced** (rarely touched; see `--help` for more):

| Variable | Role |
|----------|------|
| `LWD_URLS` | Comma-separated lightwalletd gRPC URLs. **If set and non-empty, this wins over** `--lwd-url` / `SVOTE_PIR_MAINNET_RPC_URL` (see `nf_ingest::config::resolve_lwd_urls`). |
| `SVOTE_PIR_MAINNET_RPC_URL` | Historical env name bound to `--lwd-url`: primary lightwalletd gRPC URL for `sync` / rebuild paths. *Not* a zcashd JSON-RPC endpoint despite the name. |
| `SVOTE_PIR_BOOTSTRAP_TIMEOUT_SECS` | Cap on bootstrap wall time before startup fails. |
| `SVOTE_PIR_WATCHDOG_TICK_SECS` | How often the watchdog re-checks staleness. |
| `SVOTE_PIR_VOTE_CHAIN_URL` | Optional active-round guard URL for `POST /snapshot/prepare`. |

### `nf-server sync` (CLI / env)

Sync is run ad-hoc by the operator (see [Synced mode](#synced-mode)); no systemd unit ships for it.

| Variable / flag | Role |
|-----------------|------|
| `SVOTE_ZCASH_NETWORK` | Required Zcash network: `main` or `test`. |
| `SVOTE_PIR_DATA_DIR` | Nullifier + tree root (same env as `serve`; default `./pir-data`). |
| `--output-dir` | Optional; tier export directory (defaults to `--pir-data-dir`). |
| `LWD_URLS` | Comma-separated post-NU6.3 lightwalletd gRPC URLs. Overrides `--lwd-url` when set. |
| `SVOTE_PIR_SYNC_RESET` | When `1` or `true`, delete the dataset marker, nullifiers, tree, and tiers before the run. Required once when migrating legacy Orchard data. |
| `SVOTE_PIR_SYNC_ACK_HEIGHT_MISMATCH` | With `--non-interactive`, must be `RESYNC` when local checkpoint is above the active round `snapshot_height`. |
| `SVOTE_PIR_VOTING_CONFIG_URL` | Empty string skips voting-config fetch and height cap; non-empty requires `vote_servers` that expose an active round with `snapshot_height`. |

See [Synced mode](#synced-mode) for the common ad-hoc flags (`--non-interactive`, `--invalidate-after-blocks`, `--max-height`). `nf-server sync --help` has the full list.

## Files under `SVOTE_PIR_DATA_DIR`

Everything under `SVOTE_PIR_DATA_DIR` belongs to exactly one Zcash network. Fleet hosts use `/opt/nf-ingest/pir-data/<network>`.

| File | Stage / source | Purpose |
|------|----------------|---------|
| `nullifiers.bin` | Stage 1 — sync | Append-only raw 32-byte Ironwood nullifiers streamed from lightwalletd. The underlying data; everything else is derived. |
| `nullifiers.dataset.json` | Stage 1 — sync | Required identity marker with `zcash_network`, `nullifier_pool: "ironwood"`, and `dataset_version: 2`. |
| `nullifiers.checkpoint` | Stage 1 — sync | Durable commit point for `nullifiers.bin`; half-written batches are discarded on startup. |
| `nullifiers.index` | Stage 1 — sync | Per-batch height index; lets `sync` and `POST /snapshot/prepare` export a snapshot at a past height. Auto-rebuilt if missing. |
| `nullifiers.tree` | Stage 2 — sync | Versioned checkpoint of the depth-19 PIR tree at a specific height. Lets Stage 3 skip the tree rebuild. Safe to delete to force a rebuild. |
| `tier0.bin`, `tier1.bin` | Stage 3 — sync **or** serve bootstrap | The public index and 48 MiB PIR database. Identical to `<precomputed-base>/snapshots/<network>/<height>/tier*.bin`. |
| `pir_root.json` | Stage 3 — sync **or** serve bootstrap | Metadata: dataset identity, tree roots, tier byte sizes, and `height`. Installed **last** so a half-applied bootstrap retries cleanly next start. |
| `tier1.precompute` | Stage 4: written by `serve` after first YPIR setup | Warm-restart cache for YPIR pre-computed material. Auto-invalidated by the Tier 1 content hash; safe to delete (next boot recomputes). **Not** distributed via the CDN; each host writes its own. |

When in doubt, reset only the selected network directory. For example, `rm -rf /opt/nf-ingest/pir-data/test/* && systemctl restart nullifier-query-server` re-bootstraps testnet from the CDN.

`nf-server doctor` reports cache presence and size per tier; use it for warm-start regression triage ("did the cache disappear?").

## HTTP endpoints

`nf-server serve` exposes the routes below on `--port`. The **Audience** column shows who calls each route in normal operation; routes outside the **Client** audience can be safely blocked at the reverse proxy.

| Method & path | Audience | Purpose |
|---------------|----------|---------|
| `GET /tier0` | Client | Download tier-0 of the PIR tree in plaintext (small, public). |
| `GET /params/tier1` | Client | YPIR scenario parameters needed to build the single query. |
| `POST /tier1/query` | Client | Submit an encrypted PIR query and receive an encrypted response. |
| `GET /root` | Client | Zcash network, dataset identity, tree roots, depth, `num_ranges`, and serving `height`. |
| `GET /health` | Ops | JSON: `status` (`starting` / `ok` / `rebuilding` / `error`) plus tier row metadata. Always 200. |
| `GET /ready` | Ops / load balancer | 200 only when the internal phase is `Serving`; **503** with a JSON `phase` body while still starting or on error. |
| `GET /metrics` | Ops | Prometheus exposition. |
| `GET /tier1/row/:idx` | Debug only | Raw tier row, **not** privacy-preserving. Block at the proxy. |

## Troubleshooting

Start with `journalctl -u nullifier-query-server -n 200 --no-pager` and `curl -fsS http://127.0.0.1:3000/health | jq .`. The JSON `status` field mirrors the internal lifecycle (`starting` / `ok` / `rebuilding` / `error`). For finer-grained `Starting { progress: ... }` payloads, inspect logs or `curl` `/ready` while it still returns 503.

| Symptom | Likely cause | Action |
|---------|--------------|--------|
| `status` stays `"starting"` for >2 min, log shows PIR/voting-config fetch errors | Outbound HTTPS to config hosts blocked, or URL overridden incorrectly | Check egress (see [Network requirements](#network-requirements)); confirm `SVOTE_PIR_CONFIG_URL` and legacy `SVOTE_PIR_VOTING_CONFIG_URL`; for offline hosts set `SVOTE_PIR_CONFIG_URL=` and pre-stage tiers. |
| `status` stays `"starting"`, log shows no configured snapshot height and no local snapshot | Bootstrap is enabled on a fresh host but neither `pir.json` nor legacy active-round fallback produced a height. | Set `SVOTE_PIR_FORCE_SNAPSHOT_HEIGHT` to a published snapshot height, pre-stage `pir-data`, or publish/update the environment `pir.json`. |
| `status` stays `"starting"`, log shows tier download 404 / hash mismatch | CDN base URL wrong, network mismatch, or unpublished snapshot | Confirm `<base>/snapshots/<network>/<height>/manifest.json` exists. |
| `status` is `"error"` after bootstrap, "tier load failed" | Corrupt or partial files under `SVOTE_PIR_DATA_DIR` | Clear only that network directory and restart. |
| Sync rejects a missing or incompatible dataset marker | Existing files are legacy or belong to another dataset version | Set `SVOTE_PIR_SYNC_RESET=1` and rerun sync. Do not create the marker by hand. |
| Crash-loop, `journalctl` shows `SIGILL` immediately at startup | Binary built with AVX-512 on a CPU without it | Run `nf-server doctor`; move to an AVX-512 host or use `linux-arm64`. |
| `/ready` returns 503 indefinitely, no errors | Long bootstrap (cold start) — see [Bootstrapped mode](#bootstrapped-mode) | Wait ~2 min on the recommended SKU. If it doesn't clear, check `/health`. |
| `nf-server sync` aborts with `RESYNC` prompt | Local nullifier checkpoint is above the active round `snapshot_height` | See [Height-mismatch wipe](#height-mismatch-wipe-resync). |
| `nullifiers.tree` rejected as unknown format | Tree file left over from an older build | Delete the file or set `SVOTE_PIR_SYNC_RESET=1` and rerun sync. |
| Logs show `precompute cache miss; recomputing` after a binary upgrade or snapshot rotation | Expected: cache header binds to the YPIR version, build target, and tier-source hash. Any of those changing forces a recompute. | None. Next `serve` boot writes a fresh cache; subsequent restarts will be warm. |
| `precompute cache write failed; serving from memory` warning | `tier{N}.precompute.tmp` couldn't be flushed (typically ENOSPC) | Free disk under `SVOTE_PIR_DATA_DIR`; the server is still serving correctly, but the next restart will pay the full cold-start cost. Cache will be written on the next successful boot. |

For deeper investigation, raise verbosity with `RUST_LOG=debug,nf_server=trace` in `/etc/default/nf-server` and restart.

## See also

- [vote-infrastructure](https://github.com/valargroup/vote-infrastructure) — Terraform / DigitalOcean droplet provisioning.
