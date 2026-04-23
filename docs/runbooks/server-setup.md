# Runbook: Setup PIR Server

## Overview

Vote-nullifier PIR lets a client prove that a Zcash Orchard nullifier is **not** in the on-chain nullifier set, without revealing *which* nullifier it is asking about. This service is a building block for shielded voting.

This runbook covers the operator side: standing up an `nf-server` host that answers PIR queries from clients over HTTP. One server is a single `nf-server` binary listening on a single port (default `3000`); see [Recommended hardware](#recommended-hardware) for the target SKU.

**Who this runbook is for:**

- **Operators**: use the release binary + systemd path. The one-liner below is the shortcut; the rest of the runbook leads with that path and uses the installed `nf-server` binary directly.
- **Custom-layout / non-Linux**: see [Manual install](#manual-install-no-start_pirsh).
- **Developers** iterating from a source checkout (`cargo run`, `make sync`, `make serve`): see [CONTRIBUTING.md](../../CONTRIBUTING.md). Those workflows are intentionally out of scope here.

There are two data-source modes the server can run in:

1. **Bootstrapped** — the PIR server downloads pre-computed snapshot data from Valar Group–hosted object storage. This is the **default** mode under the shipped systemd unit.
2. **Synced** — the PIR server runs `nf-server sync`: stream Orchard nullifiers from lightwalletd up to a chosen height (or chain tip), materialize a versioned `nullifiers.tree` checkpoint, then write the 3-tier representation per [PIR tree spec](../pir-tree-spec.md). Each stage resumes from on-disk artifacts after failure. Operators run `nf-server sync` ad-hoc; the systemd unit only covers `serve`.

## Quick start

On Linux, we recommend using this one-CLI command to get started:

```bash
curl -fsSL https://vote.fra1.digitaloceanspaces.com/start_pir.sh | sudo bash
```

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

**Production target: `linux-amd64` with AVX-512, 4 vCPU, 32 GB RAM, and at least 35 GB free disk.** Other platforms build but are not recommended for serving traffic (see [Platform support](#platform-support)).

Why these numbers:

- AVX-512 meaningfully accelerates PIR packing and query-side linear algebra; without it, queries fall back to the scalar path.
- ~35 GB disk covers ~2 GB nullifier data, ~7 GB tier files, and working space, with headroom.
- 4 vCPUs parallelize the matrix–vector steps that dominate query latency.

Verify a candidate host with [`nf-server doctor`](#host-health-check-nf-server-doctor) before installing.

## Network requirements

The server needs the following network access:

| Direction | Destination | Purpose |
|-----------|-------------|---------|
| Outbound 443 | `vote.fra1.digitaloceanspaces.com` | Binary, `SHA256SUMS`, `start_pir.sh`, snapshot tier downloads |
| Outbound 443 | `github.com`, `objects.githubusercontent.com` | Binary / unit-file fallback |
| Outbound 443 | `valargroup.github.io` | `voting-config.json` (default) |
| Outbound 443 | `sentry.io` (DSN-specific host) | Optional — only when `SENTRY_DSN` is set |
| Outbound 443 | lightwalletd (e.g. `zec.rocks:443`) | **Synced mode only** |
| Inbound 3000 | client / reverse proxy | PIR query traffic |

## Platform support

- **`linux-amd64`** — recommended production target. Requires AVX-512; older Intel/AMD CPUs will SIGILL on startup. Run `nf-server doctor` first to confirm.
- **`linux-arm64`** — supported but slower (no PIR-specific SIMD); not recommended for serving production traffic.
- **`darwin-arm64`** — recommended for local dev on Apple Silicon.
- **`darwin-amd64`** — dev-only: ships without the `serve` subcommand. Use for `doctor` / `sync` on Intel Macs only.

## Install

`start_pir.sh` is the recommended path; the rest of this section documents what it does so you can reproduce or audit it manually.

### Release artifacts

Each `v*` release publishes the `nf-server-<platform>` binary, `SHA256SUMS`, and `nullifier-query-server.service` to **DigitalOcean Spaces** (primary) with **GitHub Releases** as a fallback. `start_pir.sh` tries Spaces first, then GitHub. Exact URL patterns are in the curl commands in [Manual install](#manual-install-no-start_pirsh).

`start_pir.sh` itself is served at two URLs:

- `https://vote.fra1.digitaloceanspaces.com/start_pir.sh` — always the **latest** release.
- `https://vote.fra1.digitaloceanspaces.com/scripts/start_pir/<snapshot_height>/start_pir.sh` — pinned to the release that matches a given voting `snapshot_height`. Use this for reproducible installs.

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

4. **Write the env files** the unit reads (see [Configuring the service](#configuring-the-service) for the full layout). **Do not indent the lines inside the heredoc** — leading spaces would end up in the file and break `EnvironmentFile` parsing.

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

## Smoke test

After install, verify end-to-end without a real client:

```bash
curl -fsS http://127.0.0.1:3000/ready                                   # 200 OK
curl -fsS http://127.0.0.1:3000/health | jq -r '.status'                # ok (starting / rebuilding / error while warming)
curl -fsS http://127.0.0.1:3000/root   | jq '{height, pir_depth, num_ranges}'
```

`GET /health` returns a stable `status` string derived from the internal server phase. For a structured `phase` object (e.g. `{ "phase": "Starting", ... }`), probe `GET /ready` while the server is still warming — it returns **503** with that JSON body until the process reaches `Serving`.

Compare the `height` from `/root` to `voting-config.json` `snapshot_height`; they should match while bootstrap is enabled.

## Host health check (`nf-server doctor`)

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

**Startup time:** ~2 min cold on the recommended SKU (dominated by tier-2 setup). Warm restarts, when tier files are already on disk, finish in ~15 s — bounded by the CDN check.

**On startup**, `serve` fetches `voting-config.json`, compares its `snapshot_height` to local `pir_root.json`, and downloads the matching snapshot tiers from `SVOTE_PIR_PRECOMPUTED_BASE_URL` if they don't match. Defaults are correct for production — operators normally configure nothing.

**Policy:** if local tier state is unusable and bootstrap can't fix it (e.g. CDN fetch failed and no valid files under `SVOTE_PIR_DATA_DIR`), startup fails. Fix the network / configuration, fall back to [Synced mode](#synced-mode), or pre-stage files.

To disable bootstrap entirely (offline / pre-staged tiers), set `SVOTE_PIR_VOTING_CONFIG_URL=`.

For startup phase semantics, error symptoms, and recovery, see [Troubleshooting](#troubleshooting).

## Synced mode

The shipped systemd unit only covers `serve`; sync is operator-driven. Stop the service, run `nf-server sync` against the same data directory, then start the service again:

```bash
systemctl stop nullifier-query-server
# Optional: load the same env as systemd so sync picks up voting-config height cap
# sudo set -a && . /etc/default/nf-server && set +a

SNAPSHOT=$(curl -fsSL https://valargroup.github.io/token-holder-voting-config/voting-config.json | jq -r '.snapshot_height')

sudo /opt/nf-ingest/nf-server sync \
    --pir-data-dir /opt/nf-ingest/pir-data \
    --non-interactive \
    --max-height "$SNAPSHOT"
systemctl start nullifier-query-server
```

`--lwd-url` defaults to `https://zec.rocks:443`; omit it unless you need a different lightwalletd.

Useful flags:

- `--non-interactive` — required from CI / unattended SSH (no TTY prompts).
- `--invalidate-after-blocks` — force `nullifiers.tree` and tier blobs to rebuild when new blocks stream in.
- `--max-height <H>` — stop at `H` (must be a multiple of 10). Without it, syncs to mainnet chain tip, capped by `voting-config.snapshot_height` when bootstrap is enabled.

`nf-server sync` runs three resumable stages: stream nullifiers from lightwalletd → build `nullifiers.tree` → write tier files (`tier0.bin`, `tier1.bin`, `tier2.bin`, `pir_root.json`). Rerunning after partial failure picks up where it stopped. To start clean, set `SVOTE_PIR_SYNC_RESET=1`.

**Sync time** is governed by lightwalletd nullifier streaming, not local CPU — roughly ~16 min from NU5 activation to mainnet tip as of early 2026 (grows with chain length; refresh on each release).

After sync, tier files are local — but CDN bootstrap still runs on the next `serve` startup unless you disable it (`SVOTE_PIR_VOTING_CONFIG_URL=` in `/etc/default/nf-server`).

### Height-mismatch wipe (`RESYNC`)

When bootstrap is enabled and your local nullifier checkpoint is **above** the canonical `snapshot_height`, `nf-server sync` refuses to silently roll back. Confirm by typing `RESYNC` at the prompt, or — under `--non-interactive` — set:

```bash
SVOTE_PIR_SYNC_ACK_HEIGHT_MISMATCH=RESYNC
```

This wipes `nullifiers.bin`, the checkpoint, the index, `nullifiers.tree`, and tier files, then re-syncs from scratch.

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


### TLS / reverse proxy

`nf-server` speaks plaintext HTTP on `--port`; clients should reach it over TLS. Terminate TLS in a reverse proxy on the same host (or upstream LB). Minimal Caddy example:

```caddyfile
pir.example.org {
    reverse_proxy 127.0.0.1:3000
    # Restrict debug rows to internal callers; clients don't need them.
    @debug path /tier1/row/* /tier2/row/*
    handle @debug { respond 403 }
}
```

Caddy obtains a certificate automatically. For nginx, use any standard `proxy_pass` config to `127.0.0.1:3000` and block the `/tier1/row/*` and `/tier2/row/*` paths.

## Observability

**Prometheus**: scrape `GET /metrics` on the serve port. Useful signals to alert on:

- `up{job="nf-server"} == 0` for >1 m — process down (or scrape failing).
- `/ready` returning 503 for >5 m via blackbox probing — stuck out of the `Serving` state.
- Snapshot staleness past `SVOTE_PIR_STALE_THRESHOLD_SECS` (also surfaces via Sentry when configured).

Browse `/metrics` once after install for the full series list; names are stable across patch releases.

**Sentry**: optional. Create a project at [sentry.io](https://sentry.io), set `SENTRY_DSN` in `/opt/nf-ingest/.env`. The in-process snapshot watchdog emits stale-snapshot events when `SVOTE_PIR_STALE_THRESHOLD_SECS` is non-zero; `SVOTE_PIR_WATCHDOG_TICK_SECS` controls how often it checks.

**Logs**: the server logs to stdout; `journalctl -u nullifier-query-server -f` follows them. Verbosity is controlled by `RUST_LOG` (e.g. `RUST_LOG=info,nf_server=debug`); set it in `/etc/default/nf-server` and restart.

## Backup and disaster recovery

`SVOTE_PIR_DATA_DIR` is **disposable** for bootstrapped hosts: tier files come from the CDN and the snapshot height is fixed by `voting-config.json`. To recover, reinstall and restart — `start_pir.sh` and the systemd unit will re-bootstrap. No backups required for serve-only hosts.

For synced hosts, `nullifiers.bin` + `nullifiers.checkpoint` + `nullifiers.index` represent ~16 minutes of lightwalletd streaming work; back them up if you want to skip a re-stream after disk loss. Tier files are derivable.

## Upgrading

Repeat steps 1–2 of [Manual install](#manual-install-no-start_pirsh) with a new `TAG` (re-download the binary, re-check `SHA256SUMS`, reinstall to `/opt/nf-ingest/nf-server`, run `doctor`), then `sudo systemctl restart nullifier-query-server`. If the unit file itself changed in the new release (re-download it in step 3), also run `sudo systemctl daemon-reload` before the restart. `start_pir.sh` performs the equivalent end-to-end and is idempotent — re-running it against a newer release is the supported upgrade path.

## Tagging and releases

Semantic versioning applies to `nf-server` releases (`v*` tags drive CI artifacts). Integrators should pin both the **binary version** and the **voting snapshot height** they expect.

**When to upgrade:**

- A new voting round publishes a new `snapshot_height` in `voting-config.json`. A bootstrapped server picks it up on next restart, but you should also confirm the pinned binary is still supported.
- A new `v*` release with security or correctness fixes (watch GitHub Releases; subscribe via the repo's release feed).
- Otherwise, no need to chase patch releases mid-round.

For pinned-snapshot installs, use the per-snapshot `start_pir.sh` URL: `https://vote.fra1.digitaloceanspaces.com/scripts/start_pir/<snapshot_height>/start_pir.sh`.

## Configuration reference

### `nf-server serve` (CLI / env)

Variables the shipped systemd unit honors. Set them in `/etc/default/nf-server` (or, for `SENTRY_DSN`, `/opt/nf-ingest/.env`). Run `nf-server serve --help` for the full list.

**Common:**

| Variable | Role |
|----------|------|
| `SVOTE_PIR_DATA_DIR` | Single on-disk root for nullifiers, tree checkpoint, and tier files. Unit overrides via `--pir-data-dir /opt/nf-ingest/pir-data`. |
| `SVOTE_PIR_PORT` | HTTP listen port. Unit overrides via `--port 3000`. |
| `SVOTE_PIR_VOTING_CONFIG_URL` | Defaults to the production voting-config URL. Empty string disables bootstrap (offline / pre-staged tiers). |
| `SVOTE_PIR_PRECOMPUTED_BASE_URL` | CDN base URL for tier downloads. Defaults to production object storage. |
| `SVOTE_PIR_STALE_THRESHOLD_SECS` | Snapshot-staleness threshold for the watchdog (Sentry alerts gated on `SENTRY_DSN`). |
| `SENTRY_DSN` | Enables Sentry error / trace reporting. Lives in `/opt/nf-ingest/.env` (mode `0600`). |

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
| `SVOTE_PIR_DATA_DIR` | Nullifier + tree root (same env as `serve`; default `./pir-data`). |
| `--output-dir` | Optional; tier export directory (defaults to `--pir-data-dir`). |
| `SVOTE_PIR_SYNC_RESET` | When `1` or `true`, delete nullifiers + tree + tiers before run. |
| `SVOTE_PIR_SYNC_ACK_HEIGHT_MISMATCH` | With `--non-interactive`, must be `RESYNC` when local checkpoint is above voting `snapshot_height`. |
| `SVOTE_PIR_VOTING_CONFIG_URL` | Empty string skips voting-config fetch and height cap; non-empty requires `snapshot_height`. |

See [Synced mode](#synced-mode) for the common ad-hoc flags (`--non-interactive`, `--invalidate-after-blocks`, `--max-height`). `nf-server sync --help` has the full list.

## Files under `SVOTE_PIR_DATA_DIR`

Everything on disk under `--pir-data-dir` (default `/opt/nf-ingest/pir-data` for the systemd unit), grouped by which `nf-server sync` stage writes it. Stage 3 outputs are also what `serve` bootstrap fetches from the CDN, so they may appear without sync ever having run locally.

| File | Stage / source | Purpose |
|------|----------------|---------|
| `nullifiers.bin` | Stage 1 — sync | Append-only raw 32-byte Orchard nullifiers streamed from lightwalletd. The underlying data; everything else is derived. |
| `nullifiers.checkpoint` | Stage 1 — sync | Durable commit point for `nullifiers.bin`; half-written batches are discarded on startup. |
| `nullifiers.index` | Stage 1 — sync | Per-batch height index; lets `sync` and `POST /snapshot/prepare` export a snapshot at a past height. Auto-rebuilt if missing. |
| `nullifiers.tree` | Stage 2 — sync | Versioned checkpoint of the depth-25 PIR tree at a specific height. Lets Stage 3 skip the tree rebuild. Safe to delete to force a rebuild. |
| `tier0.bin`, `tier1.bin`, `tier2.bin` | Stage 3 — sync **or** serve bootstrap | The PIR database that answers queries (mmap'd by `serve`). Identical to `<precomputed-base>/snapshots/<height>/tier*.bin`. |
| `pir_root.json` | Stage 3 — sync **or** serve bootstrap | Metadata: tree roots, tier byte sizes, and `height`. Source of truth for "what height am I serving"; installed **last** so a half-applied bootstrap retries cleanly next start. |

When in doubt, `SVOTE_PIR_SYNC_RESET=1 nf-server sync` deletes all of the above (except CDN staging) and rebuilds from lightwalletd; for tier-only corruption on a `serve` host, `rm -rf /opt/nf-ingest/pir-data/* && systemctl restart nullifier-query-server` re-bootstraps from the CDN.

## HTTP endpoints

`nf-server serve` exposes the routes below on `--port`. The **Audience** column shows who calls each route in normal operation; routes outside the **Client** audience can be safely blocked at the reverse proxy.

| Method & path | Audience | Purpose |
|---------------|----------|---------|
| `GET /tier0` | Client | Download tier-0 of the PIR tree in plaintext (small, public). |
| `GET /params/tier1`, `GET /params/tier2` | Client | YPIR scenario parameters needed to build a query. |
| `POST /tier1/query`, `POST /tier2/query` | Client | Submit an encrypted PIR query, get an encrypted response. |
| `GET /root` | Client | Current tree roots, depth, `num_ranges`, and serving `height`. |
| `GET /health` | Ops | JSON: `status` (`starting` / `ok` / `rebuilding` / `error`) plus tier row metadata. Always 200. |
| `GET /ready` | Ops / load balancer | 200 only when the internal phase is `Serving`; **503** with a JSON `phase` body while still starting or on error. |
| `GET /metrics` | Ops | Prometheus exposition. |
| `GET /tier1/row/:idx`, `GET /tier2/row/:idx` | Debug only | Raw tier row, **not** privacy-preserving. Block at the proxy. |

## Troubleshooting

Start with `journalctl -u nullifier-query-server -n 200 --no-pager` and `curl -fsS http://127.0.0.1:3000/health | jq .`. The JSON `status` field mirrors the internal lifecycle (`starting` / `ok` / `rebuilding` / `error`). For finer-grained `Starting { progress: ... }` payloads, inspect logs or `curl` `/ready` while it still returns 503.

| Symptom | Likely cause | Action |
|---------|--------------|--------|
| `status` stays `"starting"` for >2 min, log shows `voting-config.json` fetch errors | Outbound HTTPS to `valargroup.github.io` blocked, or URL overridden incorrectly | Check egress (see [Network requirements](#network-requirements)); confirm `SVOTE_PIR_VOTING_CONFIG_URL`; for offline hosts set it to empty and pre-stage tiers. |
| `status` stays `"starting"`, log shows `snapshot_height required` | Bootstrap is enabled but `voting-config.json` lacks `snapshot_height` (or you pointed at a non-canonical URL) | Restore the default URL or use a config that defines `snapshot_height`. |
| `status` stays `"starting"`, log shows tier download 404 / hash mismatch | CDN base URL wrong, or release/snapshot mismatch | Verify `SVOTE_PIR_PRECOMPUTED_BASE_URL`; confirm `<base>/snapshots/<height>/manifest.json` exists. |
| `status` is `"error"` after bootstrap, "tier load failed" | Corrupt or partial files under `SVOTE_PIR_DATA_DIR` | `rm -rf /opt/nf-ingest/pir-data/* && systemctl restart nullifier-query-server` to re-bootstrap from the CDN. |
| Crash-loop, `journalctl` shows `SIGILL` immediately at startup | Binary built with AVX-512 on a CPU without it | Run `nf-server doctor`; move to an AVX-512 host or use `linux-arm64`. |
| `/ready` returns 503 indefinitely, no errors | Long bootstrap (cold start) — see [Bootstrapped mode](#bootstrapped-mode) | Wait ~2 min on the recommended SKU. If it doesn't clear, check `/health`. |
| `nf-server sync` aborts with `RESYNC` prompt | Local nullifier checkpoint is above canonical `snapshot_height` | See [Height-mismatch wipe](#height-mismatch-wipe-resync). |
| `nullifiers.tree` rejected as unknown format | Tree file left over from an older build | Delete the file or set `SVOTE_PIR_SYNC_RESET=1` and rerun sync. |

For deeper investigation, raise verbosity with `RUST_LOG=debug,nf_server=trace` in `/etc/default/nf-server` and restart.

## See also

- [vote-infrastructure](https://github.com/valargroup/vote-infrastructure) — Terraform / DigitalOcean droplet provisioning.
