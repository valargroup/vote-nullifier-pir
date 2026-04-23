# Runbook: Setup PIR Server

## Overview

Vote-nullifier PIR lets a client prove that a Zcash Orchard nullifier is **not** in the on-chain nullifier set, without revealing *which* nullifier it is asking about — a building block for shielded voting. See the [project README](../../README.md) and the [PIR tree spec](../pir-tree-spec.md) for background.

This runbook covers the operator side: standing up an `nf-server` host that answers PIR queries from clients over HTTP. One server is a single `nf-server` binary listening on a single port (default `3000`); see [Recommended hardware](#recommended-hardware) for the target SKU.

## Quick start

On Linux, we recommend using this one-CLI command to get started:

```bash
curl -fsSL https://vote.fra1.digitaloceanspaces.com/start_pir.sh | sudo bash
```

What it does:
- Downloads the latest binaries and verifies `nf-server` against `SHA256SUMS` for the pinned release (Spaces first, then GitHub Releases)
- Configures the service per the recommended parameters
- Creates an automated **systemd** unit that auto-restarts on start-up and on failure
- Bootstraps from pre-computed snapshots
- Installs the binary
- Serves PIR queries

**Operators should use the release binary + systemd path** (the one-liner above is the shortcut for it). The body of this runbook leads with that path and uses the installed `nf-server` binary directly. Manual / non-systemd install is covered below for air-gapped, custom-layout, or non-Linux environments. Developers iterating from a source checkout (`cargo run`, `make sync`, `make serve`, etc.) should consult [CONTRIBUTING.md](../../CONTRIBUTING.md) instead — those workflows are intentionally not part of this runbook.

After install, operate the service with:

```bash
systemctl status nullifier-query-server
systemctl restart nullifier-query-server
journalctl -u nullifier-query-server -f
```


There are two data-source modes:

1. **Bootstrapped** — the PIR server downloads pre-computed snapshot data from Valar Group–hosted object storage. This is the **default** mode under the shipped systemd unit.
2. **Synced** — the PIR server runs `nf-server sync`: stream Orchard nullifiers from lightwalletd up to a chosen height (or chain tip), materialize a versioned `nullifiers.tree` checkpoint, then write the 3-tier representation per [PIR tree spec](../pir-tree-spec.md). Each stage resumes from on-disk artifacts after failure. Operators run `nf-server sync` ad-hoc; the systemd unit only covers `serve`.

## Recommended hardware

We recommend a 4 Intel vCPU machine with AVX-512 support, 32 GB RAM, and at least 35 GB free disk. See [Pre-flight check](#pre-flight-check-nf-server-doctor) to verify a host before installing, and the [Rationale](#recommended-hardware-1) below for why.

## Install

`start_pir.sh` is the recommended path; the rest of this section documents what it does so you can reproduce or audit it manually.

### Where the binaries live

Each `v*` release publishes the same artifacts to two locations:

| Artifact | DigitalOcean Spaces (primary) | GitHub Releases (fallback) |
|----------|--------------------------------|-----------------------------|
| `nf-server-linux-amd64` | `https://vote.fra1.digitaloceanspaces.com/binaries/vote-pir/nf-server-<tag>-linux-amd64` | `https://github.com/valargroup/vote-nullifier-pir/releases/download/<tag>/nf-server-linux-amd64` |
| `nf-server-linux-arm64` | `…/nf-server-<tag>-linux-arm64` | `…/<tag>/nf-server-linux-arm64` |
| `nf-server-darwin-arm64` | `…/nf-server-<tag>-darwin-arm64` | `…/<tag>/nf-server-darwin-arm64` |
| `nf-server-darwin-amd64` | `…/nf-server-<tag>-darwin-amd64` | `…/<tag>/nf-server-darwin-amd64` |
| `nullifier-query-server.service` | — | `…/<tag>/nullifier-query-server.service` |
| `SHA256SUMS` | `…/SHA256SUMS-<tag>` (same line format as GitHub; file names match GitHub asset names) | `…/<tag>/SHA256SUMS` |

`start_pir.sh` itself is published to:

- `https://vote.fra1.digitaloceanspaces.com/start_pir.sh` — always points at the **latest** release.
- `https://vote.fra1.digitaloceanspaces.com/scripts/start_pir/<snapshot_height>/start_pir.sh` — pinned to the release that matched a given voting `snapshot_height`. Use this when you need a reproducible install of a specific snapshot.

`start_pir.sh` tries Spaces first, then falls back to GitHub Releases for the binary, `SHA256SUMS`, and the unit file.

### Build-time caveats per platform

- **`linux-amd64`** is built with `--features avx512` against `target-cpu=x86-64-v4`. It requires a CPU with AVX-512; older Intel/AMD hardware will SIGILL on startup. Run `nf-server doctor` first to confirm.
- **`linux-arm64`** is built with `--features serve` (no AVX, no PIR-specific SIMD).
- **`darwin-arm64`** is built with `--features serve` and is the recommended Mac target.
- **`darwin-amd64`** ships **without the `serve` feature**; use it only for `nf-server doctor` and `nf-server sync` on Intel Macs. Production serving on Intel Mac is unsupported.

### Manual install (no `start_pir.sh`)

For air-gapped hosts, custom layouts, non-Linux platforms, or when debugging the installer:

1. **Download the binary** for your platform from one of the URLs above. Save it as `/tmp/nf-server-${PLATFORM}` (the name used in `SHA256SUMS`) regardless of whether you pull from Spaces or GitHub:

   ```bash
   PLATFORM=linux-amd64        # or linux-arm64, darwin-arm64, darwin-amd64
   TAG=v0.x.y                  # pin the release tag

   curl -fL -o "/tmp/nf-server-${PLATFORM}" \
     "https://vote.fra1.digitaloceanspaces.com/binaries/vote-pir/nf-server-${TAG}-${PLATFORM}" \
     || curl -fL -o "/tmp/nf-server-${PLATFORM}" \
       "https://github.com/valargroup/vote-nullifier-pir/releases/download/${TAG}/nf-server-${PLATFORM}"

   curl -fL -o /tmp/SHA256SUMS \
     "https://vote.fra1.digitaloceanspaces.com/binaries/vote-pir/SHA256SUMS-${TAG}" \
     || curl -fL -o /tmp/SHA256SUMS \
       "https://github.com/valargroup/vote-nullifier-pir/releases/download/${TAG}/SHA256SUMS"

   ( cd /tmp && sha256sum -c SHA256SUMS --ignore-missing )

   sudo install -d /opt/nf-ingest /opt/nf-ingest/pir-data
   sudo install -m 0755 "/tmp/nf-server-${PLATFORM}" /opt/nf-ingest/nf-server
   ```

2. **Sanity-check** the binary and the host:

   ```bash
   /opt/nf-ingest/nf-server --version
   /opt/nf-ingest/nf-server doctor --pir-data-dir /opt/nf-ingest/pir-data
   ```

3. **Download the systemd unit** and install it:

   ```bash
   sudo curl -fL -o /etc/systemd/system/nullifier-query-server.service \
     "https://github.com/valargroup/vote-nullifier-pir/releases/download/${TAG}/nullifier-query-server.service"
   ```

4. **Write the env files** the unit reads (see [Configuring the service](#configuring-the-service) for the full layout):

   ```bash
   sudo tee /etc/default/nf-server >/dev/null <<'EOF'
   SVOTE_PIR_VOTING_CONFIG_URL=https://valargroup.github.io/token-holder-voting-config/voting-config.json
   SVOTE_PIR_PRECOMPUTED_BASE_URL=https://vote.fra1.digitaloceanspaces.com
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

### Upgrading

Repeat steps 1–2 with a new `TAG`, then `sudo systemctl restart nullifier-query-server`. If the unit file itself changed in the new release (re-download it in step 3), also run `sudo systemctl daemon-reload` before the restart. `start_pir.sh` performs the equivalent end-to-end and is idempotent — re-running it against a newer release is the supported upgrade path.

## Pre-flight check (`nf-server doctor`)

Before provisioning or when debugging a host, run:

```bash
nf-server doctor
```

Use the same PIR data root as `serve` / `sync` (defaults to `./pir-data`; override with `--pir-data-dir` or `SVOTE_PIR_DATA_DIR`):

```bash
nf-server doctor --pir-data-dir /opt/nf-ingest/pir-data
```

The command prints logical CPU count, system RAM, free space on the volume backing the data directory, and (on x86_64) whether AVX-512F is visible at runtime. It compares these to the recommendations above and prints `WARN: …` lines to stderr when something is undersized or missing; **exit status is always 0** so automation and CI can run it as a smoke check without failing undersized dev machines.

Production binaries should be built with `--features serve` (and `--features avx512` on capable hardware); `doctor` notes when those compile-time features are off.

## Startup time estimate

Estimates assume the recommended hardware.

- **Bootstrap** wall time is dominated by **tier 2** on the recommended SKU: ~70 s matrix construction plus ~45–50 s YPIR offline precompute. Warm restarts only recover the CDN download cost (~15s).
- **Synced** wall time on the reference host is governed by lightwalletd nullifier streaming, not local CPU. As of April 2026, ~16 minutes from NU5 activation to mainnet tip.

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

**Policy:** If local PIR tier state is unusable and bootstrap cannot fix it (for example nothing valid under `SVOTE_PIR_DATA_DIR` and CDN fetch failed), startup fails after bootstrap. Fix bootstrap configuration, network, or use **Synced** mode / pre-staged files.

**What happens in the background?**

`nf-server serve` startup runs three phases: index maintenance under `SVOTE_PIR_DATA_DIR`, snapshot bootstrap (voting-config fetch + optional CDN tier download), then loading the mmap'd tier files.

By default the binary points at a non-empty voting-config URL (`https://valargroup.github.io/token-holder-voting-config/voting-config.json`), so operators normally configure nothing. Setting `SVOTE_PIR_VOTING_CONFIG_URL=` (or `--voting-config-url ""`) disables bootstrap — use this only for offline / pre-staged tiers.

1. Fetch `voting-config.json` from the configured URL. While bootstrap is enabled (non-empty URL), `snapshot_height` is **required**; startup fails otherwise.
2. Compare canonical `snapshot_height` to local `pir_root.json` height.
   - If equal, continue to load and serve.
   - If not equal, download the snapshot for the expected height from the pre-computed base URL (`…/snapshots/<height>/…`), verify hashes from `manifest.json`, and install into `SVOTE_PIR_DATA_DIR`. CDN download failures log a warning and fall through to existing on-disk files; startup **errors** if tier files ultimately cannot be loaded.
3. If CDN sync fails but raw nullifier files exist at the expected height, an operator may run `nf-server sync` (see [Synced mode](#synced-mode)) to rebuild `nullifiers.tree` and tiers locally. If local **nullifier checkpoint** is above `snapshot_height` while the voting-config URL is enabled, `nf-server sync` prompts to type **`RESYNC`** (or set `SVOTE_PIR_SYNC_ACK_HEIGHT_MISMATCH=RESYNC` with `--non-interactive`) to wipe and realign.

**Fatal errors (typical):**

- Tier load fails after bootstrap (missing or corrupt `tier0.bin` / `pir_root.json`, etc.).
- `voting-config.json` cannot be fetched or decoded, or `snapshot_height` is missing, while bootstrap is enabled. For offline-only disks, set `SVOTE_PIR_VOTING_CONFIG_URL=` so bootstrap is skipped and pre-staged files under `SVOTE_PIR_DATA_DIR` are served.

Resolution hints:

- Production: defaults are usually correct; override `SVOTE_PIR_VOTING_CONFIG_URL` only for a mirror or staging config. For fully local tiers, set it to empty to disable bootstrap.
- Confirm `SVOTE_PIR_PRECOMPUTED_BASE_URL` when relying on CDN tier download (default points at production object storage).

## Synced mode

The shipped systemd unit only covers `serve`; sync is operator-driven. Stop the service, run `nf-server sync` against the same data directory the unit uses, then start the service again:

```bash
systemctl stop nullifier-query-server
sudo /opt/nf-ingest/nf-server sync \
    --pir-data-dir /opt/nf-ingest/pir-data \
    --lwd-url https://zec.rocks:443
systemctl start nullifier-query-server
```

Pass `--invalidate-after-blocks` to rebuild `nullifiers.tree` and tier blobs whenever new blocks are streamed in this run. Pass `--non-interactive` from non-TTY contexts (CI, unattended SSH); when doing so, also set `SVOTE_PIR_SYNC_ACK_HEIGHT_MISMATCH=RESYNC` if a wipe is expected.

Environment from `/etc/default/nf-server` (e.g. `SVOTE_PIR_VOTING_CONFIG_URL`) is **not** auto-loaded by an interactive shell. Either pass the same vars explicitly with `sudo --preserve-env=…`, source the env file (`set -a; . /etc/default/nf-server; set +a`), or run sync inside the unit's environment with `systemd-run`:

```bash
sudo systemd-run --pty --uid=root \
    --working-directory=/opt/nf-ingest \
    -p EnvironmentFile=-/etc/default/nf-server \
    -p EnvironmentFile=-/opt/nf-ingest/.env \
    /opt/nf-ingest/nf-server sync \
    --pir-data-dir /opt/nf-ingest/pir-data \
    --lwd-url https://zec.rocks:443
```

**What happens in the background?**

1. **Stage 1 — Nullifiers** (`nf-server sync`): stream Orchard nullifiers from NU5 activation up through the `--max-height` flag if set (must be a multiple of 10), or up to **mainnet chain tip** when unset. When `SVOTE_PIR_VOTING_CONFIG_URL` is **non-empty**, `snapshot_height` is fetched and caps the target height; if your local checkpoint is **above** that height, the CLI stops until you confirm **`RESYNC`** (wipe) or set `SVOTE_PIR_SYNC_ACK_HEIGHT_MISMATCH=RESYNC` with `--non-interactive`. Writes `nullifiers.bin`, `nullifiers.checkpoint`, and `nullifiers.index` (see `nf-ingest` crate docs).
2. **Stage 2 — Tree checkpoint**: builds the PIR Merkle structure and writes **`nullifiers.tree`** (magic `SVOTEPT1`, temp + rename). If this file already matches the checkpoint height, the stage is skipped.
3. **Stage 3 — Tiers**: writes `tier0.bin`, `tier1.bin`, `tier2.bin`, and `pir_root.json` (by default under the same `--pir-data-dir` as nullifiers; use `--output-dir` to split for staging uploads). If those files already match the expected height and sizes, the stage is skipped.

**Resume:** rerunning `nf-server sync` continues after partial failure (e.g. if `nullifiers.bin` exists, nullifier sync resumes from checkpoint; if `nullifiers.tree` exists for the target height, tier export resumes; if tiers are complete, nothing heavy runs).

**Fresh start:** set `SVOTE_PIR_SYNC_RESET=1` (or `true`) before `nf-server sync` to delete `nullifiers.bin`, checkpoint, index, `nullifiers.tree`, and tier files under the nullifier root and tier output directory (`--pir-data-dir` by default), then run a full pipeline.

**Unknown `nullifiers.tree` format:** files without the `SVOTEPT1` header are rejected; remove them or set `SVOTE_PIR_SYNC_RESET=1` so sync can rebuild.

After sync, tier files are local but CDN bootstrap may still run on the next `serve` startup unless you disable it (`SVOTE_PIR_VOTING_CONFIG_URL=` in `/etc/default/nf-server` for the systemd unit).

## Configuring the service

The release ships `nullifier-query-server.service` and `start_pir.sh` installs it to `/etc/systemd/system/`. The unit:

- runs `Type=simple` with `Restart=on-failure` and `RestartSec=30`;
- has `WorkingDirectory=/opt/nf-ingest`;
- `ExecStart=/opt/nf-ingest/nf-server serve --pir-data-dir /opt/nf-ingest/pir-data --port 3000`;
- pulls environment from two files (both optional, `EnvironmentFile=-…`):
  - `/etc/default/nf-server` — operator / cloud-init owned. Holds `SVOTE_PIR_VOTING_CONFIG_URL` and `SVOTE_PIR_PRECOMPUTED_BASE_URL`. Edit this file to point at a mirror or to disable bootstrap (`SVOTE_PIR_VOTING_CONFIG_URL=`).
  - `/opt/nf-ingest/.env` — deploy-workflow owned. Holds `SENTRY_DSN`. Mode `0600`.

To change settings, edit the appropriate env file and:

```bash
systemctl daemon-reload   # only after editing the .service file itself
systemctl restart nullifier-query-server
```

For HTTPS in front of the listen port, run a reverse proxy (for example Caddy or nginx) on the host. Rolling restarts across replicas are described in [restart-pir-fleet.md](restart-pir-fleet.md). For step-by-step manual install (without `start_pir.sh`), see [Install → Manual install](#manual-install-no-start_pirsh).

## Observability

Prometheus metrics are exposed at `GET /metrics` on the serve port; scrape with your usual tooling.

The server can also emit errors and traces to Sentry. Create a project at [sentry.io](https://sentry.io), copy the DSN, and set `SENTRY_DSN`. The in-process snapshot watchdog emits stale-snapshot events through Sentry when `SVOTE_PIR_STALE_THRESHOLD_SECS` is non-zero and a DSN is present; tune `SVOTE_PIR_WATCHDOG_TICK_SECS` for how often it checks gauges versus the threshold.

## Rationale

### Recommended hardware

- AVX-512 meaningfully accelerates PIR packing and query-side linear algebra.
- Roughly 35 GB disk is enough for ~2 GB nullifier data, ~7 GB tier files, and working space. The rest is headroom.
- 4 vCPUs help parallelize large matrix–vector steps during queries.

## Useful configuration

### Operator: `nf-server serve` (CLI / env)

These are the variables the shipped systemd unit honors. Set them in `/etc/default/nf-server` (or, for `SENTRY_DSN`, `/opt/nf-ingest/.env`). See `nf-server serve --help` for the full list.

| Variable | Role |
|----------|------|
| `SVOTE_PIR_DATA_DIR` | Single on-disk root for nullifiers, tree checkpoint, and tier files. Unit overrides via `--pir-data-dir /opt/nf-ingest/pir-data`. |
| `SVOTE_PIR_PORT` | HTTP listen port. Unit overrides via `--port 3000`. |
| `SVOTE_PIR_VOTING_CONFIG_URL` | Defaults to the production voting-config URL. Empty string disables bootstrap (offline / pre-staged tiers). |
| `SVOTE_PIR_PRECOMPUTED_BASE_URL` | CDN base URL for tier downloads. Defaults to production object storage. |
| `LWD_URLS` | Comma-separated lightwalletd gRPC URLs (overrides built-in defaults). |
| `SVOTE_PIR_MAINNET_RPC_URL` | Optional zcashd JSON-RPC endpoint for chain-tip checks. |
| `SVOTE_PIR_BOOTSTRAP_TIMEOUT_SECS` | Cap on bootstrap wall time before startup fails. |
| `SVOTE_PIR_STALE_THRESHOLD_SECS` | Snapshot-staleness threshold for the watchdog (Sentry alerts gated on `SENTRY_DSN`). |
| `SVOTE_PIR_WATCHDOG_TICK_SECS` | How often the watchdog re-checks staleness. |
| `SVOTE_PIR_VOTE_CHAIN_URL` | Optional active-round guard URL for `POST /snapshot/prepare`. |
| `SENTRY_DSN` | Enables Sentry error / trace reporting. Live in `/opt/nf-ingest/.env` (mode `0600`). |

### Operator: `nf-server sync` (CLI / env)

Sync is run ad-hoc by the operator (see [Synced mode](#synced-mode)); no systemd unit ships for it.

| Variable / flag | Role |
|-----------------|------|
| `SVOTE_PIR_DATA_DIR` | Nullifier + tree root (same env as `serve`; default `./pir-data`). |
| `--output-dir` | Optional; tier export directory (defaults to `--pir-data-dir`). |
| `SVOTE_PIR_SYNC_RESET` | When `1` or `true`, delete nullifiers + tree + tiers before run. |
| `SVOTE_PIR_SYNC_ACK_HEIGHT_MISMATCH` | With `--non-interactive`, must be `RESYNC` when local checkpoint is above voting `snapshot_height`. |
| `SVOTE_PIR_VOTING_CONFIG_URL` | Empty string skips voting-config fetch and height cap; non-empty requires `snapshot_height`. |
| `--non-interactive` | No TTY prompts (CI / SSH). |
| `--invalidate-after-blocks` | After new blocks are synced from lightwalletd in this run, delete `nullifiers.tree` and tier files so they rebuild. |

## Tagging and releases

Semantic versioning applies to `nf-server` releases (`v*` tags drive CI artifacts). Integrators should pin **binary version** and the **voting snapshot height** they expect.

## Files under `SVOTE_PIR_DATA_DIR`

Everything on disk under `--pir-data-dir` (default `/opt/nf-ingest/pir-data` for the systemd unit), grouped by which `nf-server sync` stage writes it. Stage 3 outputs are also what `serve` bootstrap fetches from the CDN, so they may appear without sync ever having run locally.

| File | Stage / source | Purpose |
|------|----------------|---------|
| `nullifiers.bin` | Stage 1 — sync | Append-only raw 32-byte Orchard nullifiers streamed from lightwalletd. The underlying data; everything else is derived. |
| `nullifiers.checkpoint` | Stage 1 — sync | 16-byte `(height, offset)` written atomically per batch. Durable commit point: on startup, `nullifiers.bin` is truncated to `offset`, discarding any half-written batch. |
| `nullifiers.index` | Stage 1 — sync | Append-only log of `(height, offset)` per committed batch. Lets `sync` and `POST /snapshot/prepare` export a snapshot at a past `voting-config.snapshot_height`. Auto-rebuilt from the checkpoint if missing. |
| `nullifiers.tree` | Stage 2 — sync | Versioned `SVOTEPT1` checkpoint of the depth-25 PIR tree at a specific height. Lets Stage 3 skip the tree rebuild. Safe to delete to force a rebuild. |
| `tier0.bin`, `tier1.bin`, `tier2.bin` | Stage 3 — sync **or** serve bootstrap | The PIR database that answers queries (mmap'd by `serve`). Identical to `<precomputed-base>/snapshots/<height>/tier*.bin`. |
| `pir_root.json` | Stage 3 — sync **or** serve bootstrap | Metadata: tree roots, tier byte sizes, and `height`. Source of truth for "what height am I serving"; installed **last** so a half-applied bootstrap retries cleanly next start. |

When in doubt, `SVOTE_PIR_SYNC_RESET=1 nf-server sync` deletes all of the above (except CDN staging) and rebuilds from lightwalletd; for tier-only corruption on a `serve` host, `rm -rf /opt/nf-ingest/pir-data/* && systemctl restart nullifier-query-server` re-bootstraps from the CDN.

### HTTP endpoints

`nf-server serve` exposes the routes below on `--port`. The **Audience** column shows who calls each route in normal operation; locking the rest down at a reverse proxy is reasonable.

| Method & path | Audience | Purpose |
|---------------|----------|---------|
| `GET /tier0` | Client | Download tier-0 of the PIR tree in plaintext (small, public). |
| `GET /params/tier1`, `GET /params/tier2` | Client | YPIR scenario parameters needed to build a query. |
| `POST /tier1/query`, `POST /tier2/query` | Client | Submit an encrypted PIR query, get an encrypted response. |
| `GET /root` | Client | Current tree roots, depth, `num_ranges`, and serving `height`. |
| `GET /health` | Ops | JSON: server `phase` (`starting` / `ok` / `rebuilding` / `error`) and tier shape. Always 200. |
| `GET /ready` | Ops / load balancer | 200 only when phase is `Serving`; 503 otherwise. |
| `GET /metrics` | Ops | Prometheus exposition. |
| `GET /tier1/row/:idx`, `GET /tier2/row/:idx` | Debug only | Raw tier row, **not** privacy-preserving. Restrict at the proxy. |

## See also

- [vote-infrastructure](https://github.com/valargroup/vote-infrastructure) — Terraform / DigitalOcean droplet provisioning.
