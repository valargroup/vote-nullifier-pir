# Runbook: Setup PIR Server

This runbook explains how to set up a vote nullifier private information retrieval (PIR) server.

**Linux (production):** install the release-tagged binary, bootstrap env pins, and the `nullifier-query-server` systemd unit in one step:

```bash
curl -fsSL https://vote.fra1.digitaloceanspaces.com/start_pir.sh | sudo bash
```

What it does:

- Downloads the latest binaries
- Configures the service per the recommended parameters
- Creates an automated **systemd** unit that auto-restarts on start-up and on failure
- Bootstraps from pre-computed snapshots
- Installs the binary
- Serves PIR queries

Piping a remote script into `bash` trusts the publisher and your TLS path; inspect the script first (`curl -fsSL …/start_pir.sh`) when your policy requires it. The same installer is also published per voting-config `snapshot_height` at `…/scripts/start_pir/<snapshot_height>/start_pir.sh` (details in [deploy-setup.md](../deploy-setup.md)). The script matches the bootstrap defaults in `nf-server` and writes `/etc/default/nf-server` like that guide. For Caddy/TLS in front of the listen port, extra environment variables, or a manual binary download, see [deploy-setup.md](../deploy-setup.md).

For operators who prefer manual setup, or for debugging, manual approaches are outlined below.

There are two modes for starting up:

1. **Bootstrapped** — the PIR server downloads pre-computed snapshot data from Valar Group–hosted object storage.
2. **Synced** — the PIR server runs `nf-server sync`: stream Orchard nullifiers from lightwalletd up to a chosen height (or chain tip), materialize a versioned `nullifiers.tree` checkpoint, then write the 3-tier representation per [PIR tree spec](../pir-tree-spec.md). Each stage resumes from on-disk artifacts after failure.

## Recommended hardware

We recommend a 4 Intel vCPU machine with AVX-512 support, 32 GB RAM, and at least 35 GB free disk.

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

- **Bootstrapped** startup (until **`GET /ready` returns 200**) is usually **~2–2.5 minutes wall clock** on a 4 vCPU Intel reference host, but the work splits differently on **cold** vs **warm** boots. After the HTTP listener binds, `nf-server` still runs a background pipeline: **nullifier index maintenance** on `pir-data/`, **voting-config fetch** and **snapshot bootstrap** (either CDN download + hash verification of `tier0.bin` / `tier1.bin` / ~**3 GiB** `tier2.bin` / `pir_root.json`, or an immediate no-op when local `pir_root.json` already matches `snapshot_height`), then **mmap** of the tiers and **YPIR offline precomputation**. On one `fra1` trace, CDN work logged **~16 s**; tier **1** YPIR precompute logged **~2 s**; tier **2** dominated with **~70 s** matrix construction plus **~46–52 s** offline precompute before **Server ready**—that tier‑2 CPU phase remains on **warm** restarts because it is not persisted to disk.
- **Synced** mode wall time depends on chain tip, lightwalletd performance, and how much of the pipeline is skipped from existing files; a **fresh** full mainnet run is far longer than a sub-minute smoke test, but it is not inherently “hours” on fast paths—always budget using your own measurement.

### Measured reference (April 2026)

These numbers are **benchmark snapshots** on DigitalOcean; they move with chain height, snapshot height, region, and peers. Use them as ballpark calibration, not an SLA.

| Item | Value |
|------|--------|
| Host | `m-4vcpu-32gb-intel`, region `fra1`, Ubuntu 22.04, **100 GiB** root disk (slug default) |
| Binary | `nf-server` **v0.0.16** `linux-amd64` release (pre-built `serve` + `avx512`) |

**Session A — full mainnet tip sync** (2026-04-22 UTC):

- `nf-server doctor`: 4 logical CPUs, **31.3 GiB** RAM (DO reports slightly under 32 GiB on this SKU), **AVX-512F: yes**, ~95 GiB free on `/` at `pir-data`.
- **Elapsed (wall clock): 13 m 38 s** from `/usr/bin/time` (`SVOTE_PIR_VOTING_CONFIG_URL` **empty** so height is not capped by voting-config; `SVOTE_PIR_SYNC_RESET=1`; `nf-server sync --non-interactive --pir-data-dir /opt/nf-ingest/pir-data --lwd-url https://zec.rocks:443`). Chain tip height **3317378** that day; nullifier streaming dominated wall time, then tree + tier export in the same process. Peak RSS about **12 GiB**.

**Session B — bootstrap cold vs warm** (2026-04-22 UTC, second droplet, same slug/region; production `SVOTE_PIR_VOTING_CONFIG_URL` + `SVOTE_PIR_PRECOMPUTED_BASE_URL=https://vote.fra1.digitaloceanspaces.com`; empty `pir-data/`, then two consecutive `nf-server serve …` runs, each timed until **`GET /ready` returns HTTP 200**):

- **Cold (CDN populate): 147 s** (~2 m 27 s). INFO logs on that run: snapshot bootstrap **~15.6 s** for ~3.2 GiB of tier payload + metadata; tier **1** YPIR offline precompute **~2.3 s**; tier **2** YPIR **~73.8 s** construct + **~51.5 s** offline precompute (internal **Server ready** elapsed **~130 s** vs **147 s** wall including index work and polling).
- **Warm (immediate restart, `AlreadyAtHeight`): 120 s** (~2 m 0 s). Bootstrap skipped re-downloading tier blobs; tier **2** YPIR still logged **~69.9 s** construct + **~45.8 s** offline precompute (internal **Server ready** **~119 s**).

Use **`/ready`**, not **`/health`**: the listener comes up before the background startup pipeline finishes, so `/health` can return `"starting"` while bootstrap and YPIR precomputation are still running ([`nf-server` binds first, then runs index rebuild → bootstrap → load in a spawned task](../../nf-server/src/cmd_serve.rs)).

**Caveats:** `fra1` is co-located with the default Spaces region (favorable CDN). A separate cold-only run on another instance the same evening saw **137 s** to `/ready`—treat **~2–2.5 min** as the noise band, not a guarantee. Warm restart saves CDN time but not tier‑2 YPIR CPU time (see Session B). Lightwalletd peers and routing differ by provider. RAM below 32 GiB on this SKU triggered `doctor` WARN only.

## Bootstrapped mode

Run:

```bash
make serve
```

**Policy:** If local PIR tier state is unusable and bootstrap cannot fix it (for example nothing valid under `SVOTE_PIR_DATA_DIR` and CDN fetch failed), startup fails after bootstrap. Fix bootstrap configuration, network, or use **Synced** mode / pre-staged files.

**What happens in the background?**

Behavior matches `nf-server serve` startup: index maintenance under `SVOTE_PIR_DATA_DIR`, then snapshot bootstrap (voting-config + optional CDN tier fetch), then loading mmap’d tier files. The binary **defaults** to a non-empty voting-config URL (`https://valargroup.github.io/token-holder-voting-config/voting-config.json`), so operators normally configure nothing. While that URL stays non-empty (default or your override), its fetch and the `snapshot_height` field are **required**—startup fails otherwise. **Offline / pre-staged tiers only:** set `SVOTE_PIR_VOTING_CONFIG_URL=` (or `--voting-config-url ""`) to turn bootstrap off. After the canonical height is known, CDN tier download failures may still log warnings and fall through to existing files on disk under `SVOTE_PIR_DATA_DIR`; the process **errors** if tier files ultimately cannot be loaded. Prometheus metrics are exposed at `GET /metrics` on the serve port; optional Sentry reporting uses `SENTRY_DSN`, and snapshot staleness alerting uses `SVOTE_PIR_STALE_THRESHOLD_SECS` / `SVOTE_PIR_WATCHDOG_TICK_SECS` when Sentry is configured.

1. Fetch `voting-config.json` from the configured URL (same default as above unless you override it).
   - Require `snapshot_height` in the JSON whenever bootstrap is enabled (non-empty URL).
2. Compare canonical height to local `pir_root.json` height.
   - If equal, continue to load and serve.
   - If not equal, attempt to download the snapshot for the expected height from the pre-computed base URL (`…/snapshots/<height>/…`), verify hashes from `manifest.json`, and install into `SVOTE_PIR_DATA_DIR`.
3. If CDN sync fails but raw nullifier files exist at the expected height, an operator may run `make sync` (or `nf-server sync`) to rebuild `nullifiers.tree` and tiers locally. If local **nullifier checkpoint** is above `snapshot_height` while the voting-config URL is enabled, `nf-server sync` prompts to type **`RESYNC`** (or set `SVOTE_PIR_SYNC_ACK_HEIGHT_MISMATCH=RESYNC` with `--non-interactive`) to wipe and realign.

**Fatal errors (typical):**

- Tier load fails after bootstrap (missing or corrupt `tier0.bin` / `pir_root.json`, etc.).
- `voting-config.json` cannot be fetched or decoded, or `snapshot_height` is missing, while bootstrap is still enabled (default: non-empty voting-config URL). For offline-only disks, set `SVOTE_PIR_VOTING_CONFIG_URL=` so bootstrap is skipped and pre-staged files under `SVOTE_PIR_DATA_DIR` are served.

Resolution hints:

- Production: defaults are usually correct; override `SVOTE_PIR_VOTING_CONFIG_URL` only for a mirror or staging config. For fully local tiers, set it to empty to disable bootstrap.
- Confirm `SVOTE_PIR_PRECOMPUTED_BASE_URL` when relying on CDN tier download (default points at production object storage).

## Synced mode

Run the unified pipeline, then serve:

```bash
make sync
make serve
```

`make sync-invalidate` runs `nf-server sync --invalidate-after-blocks` so when new blocks are synced from lightwalletd, `nullifiers.tree` and tier blobs are removed and rebuilt.

**What happens in the background?**

1. **Stage 1 — Nullifiers** (`nf-server sync`): stream Orchard nullifiers from NU5 activation up through `SYNC_HEIGHT`, or up to **mainnet chain tip** when `SYNC_HEIGHT` is unset (see the [Makefile](../../Makefile)). When `SVOTE_PIR_VOTING_CONFIG_URL` is **non-empty**, `snapshot_height` is fetched and caps the target height; if your local checkpoint is **above** that height, the CLI stops until you confirm **`RESYNC`** (wipe) or set `SVOTE_PIR_SYNC_ACK_HEIGHT_MISMATCH=RESYNC` with `--non-interactive`. Writes `nullifiers.bin`, `nullifiers.checkpoint`, and `nullifiers.index` (see `nf-ingest` crate docs).
2. **Stage 2 — Tree checkpoint**: builds the PIR Merkle structure and writes **`nullifiers.tree`** (magic `SVOTEPT1`, temp + rename). If this file already matches the checkpoint height, the stage is skipped.
3. **Stage 3 — Tiers**: writes `tier0.bin`, `tier1.bin`, `tier2.bin`, and `pir_root.json` (by default under the same `PIR_DATA_DIR` as nullifiers; use `--output-dir` to split for staging uploads). If those files already match the expected height and sizes, the stage is skipped.

**Resume:** rerunning `make sync` continues after partial failure (e.g. if `nullifiers.bin` exists, nullifier sync resumes from checkpoint; if `nullifiers.tree` exists for the target height, tier export resumes; if tiers are complete, nothing heavy runs).

**Fresh start:** set `SVOTE_PIR_SYNC_RESET=1` (or `true`) before `nf-server sync` to delete `nullifiers.bin`, checkpoint, index, `nullifiers.tree`, and tier files under the nullifier root and tier output directory (`PIR_DATA_DIR` / `--pir-data-dir` by default), then run a full pipeline.

**Unknown `nullifiers.tree` format:** files without the `SVOTEPT1` header are rejected; remove them or set `SVOTE_PIR_SYNC_RESET=1` so sync can rebuild.

```bash
# After sync, tier files are local; CDN bootstrap may still run on serve
# unless you disable it in the environment as below.
make serve
```

## Configuring the service

Ship a **systemd** unit (the release artifact includes `nullifier-query-server.service`) under `/etc/systemd/system/`, point `ExecStart` at `nf-server serve`, and pass configuration via the environment—commonly `/etc/default/nf-server`, `EnvironmentFile=` in the unit, or an `.env` next to the binary. Run `systemctl daemon-reload`, then `enable` and `start` the unit. For HTTPS in front of the listen port, run a reverse proxy (for example Caddy or nginx) on the host. Rolling restarts across replicas are described in [restart-pir-fleet.md](restart-pir-fleet.md).

## Observability

The server can emit errors and traces to Sentry. Create a project at [sentry.io](https://sentry.io), copy the DSN, and set `SENTRY_DSN`. The in-process snapshot watchdog emits stale-snapshot events through Sentry when `SVOTE_PIR_STALE_THRESHOLD_SECS` is non-zero and a DSN is present; tune `SVOTE_PIR_WATCHDOG_TICK_SECS` for how often it checks gauges versus the threshold.

## Rationale

### Recommended hardware

- AVX-512 meaningfully accelerates PIR packing and query-side linear algebra.
- Roughly 35 GB disk is enough for ~2 GB nullifier data, ~7 GB tier files, and working space. The rest is headroom.
- 4 vCPUs help parallelize large matrix–vector steps during queries.
- See **Measured reference** under [Startup time estimate](#startup-time-estimate) for recent `m-4vcpu-32gb-intel` datapoints (full-tip sync, cold bootstrap, warm restart).

## Useful configuration

Makefile-oriented development variables (see [Makefile](../../Makefile)):

| Variable | Role |
|----------|------|
| `PIR_DATA_DIR` | Single on-disk root for nullifiers, tree checkpoint, and tier files (same as `SVOTE_PIR_DATA_DIR` for `nf-server`; default `pir-data` in the Makefile) |
| `LWD_URL` | First lightwalletd gRPC URL passed to sync/serve |
| `SYNC_HEIGHT` | Optional; if set, must be a multiple of 10; caps sync target (with chain tip and voting snapshot) |
| `PORT` | HTTP listen port for `make serve` |

### `nf-server sync` (CLI / env)

| Variable / flag | Role |
|-----------------|------|
| `SVOTE_PIR_DATA_DIR` | Nullifier + tree root (same env as `serve`; default `./pir-data`) |
| `--output-dir` | Optional; tier export directory (defaults to `--pir-data-dir`) |
| `SVOTE_PIR_SYNC_RESET` | When `1` or `true`, delete nullifiers + tree + tiers before run |
| `SVOTE_PIR_SYNC_ACK_HEIGHT_MISMATCH` | With `--non-interactive`, must be `RESYNC` when local checkpoint is above voting `snapshot_height` |
| `SVOTE_PIR_VOTING_CONFIG_URL` | Empty string skips voting-config fetch and height cap; non-empty requires `snapshot_height` |
| `--non-interactive` | No TTY prompts (CI / SSH) |
| `--invalidate-after-blocks` | After new blocks are synced from lightwalletd in this run, delete `nullifiers.tree` and tier files so they rebuild |

### Serve (CLI / env)

The `nf-server serve` CLI (see `nf-server serve --help`) reads environment variables including: `SVOTE_PIR_PORT`, `SVOTE_PIR_DATA_DIR`, `SVOTE_PIR_VOTING_CONFIG_URL` (defaults to the production voting-config URL when unset), `SVOTE_PIR_PRECOMPUTED_BASE_URL`, `SVOTE_PIR_MAINNET_RPC_URL`, `LWD_URLS` (comma-separated override for lightwalletd), `SVOTE_PIR_BOOTSTRAP_TIMEOUT_SECS`, `SVOTE_PIR_STALE_THRESHOLD_SECS`, `SVOTE_PIR_WATCHDOG_TICK_SECS`, `SVOTE_PIR_VOTE_CHAIN_URL`, and `SENTRY_DSN`.

## Tagging and releases

Semantic versioning applies to `nf-server` releases (`v*` tags drive CI artifacts). Integrators should pin **binary version** and the **voting snapshot height** they expect.

## Decisions (formerly open questions)

| Topic | Decision |
|-------|----------|
| Voting-config unavailable when its URL is set | With the default non-empty URL (or any non-empty override), fetch and `snapshot_height` are required or startup fails. **Offline / manual disks:** explicitly clear `SVOTE_PIR_VOTING_CONFIG_URL` and stage tier files under `SVOTE_PIR_DATA_DIR` yourself. |
| `nullifiers.checkpoint` vs `nullifiers.index` | **Checkpoint** is the durable commit point (height + byte offset into `nullifiers.bin`). **Index** records per-batch offsets for export at specific aligned heights. Both are kept. |
| Remove `POST /snapshot/prepare`? | **Keep** for in-service rebuilds when nullifier files live on the server; fleet CDN workflow does not replace every ops scenario. |
| CHANGELOG and tag policy | **Yes** — maintain `CHANGELOG.md` and document SemVer + `v*` release tagging for integrators. |

## TODO (product / engineering backlog)

- Optional: Terraform / DigitalOcean droplet setup in [vote-infrastructure](https://github.com/valargroup/vote-infrastructure).
- Document `SVOTE_PIR_VOTE_CHAIN_URL` (optional; active-round guard for `POST /snapshot/prepare`) in operator-facing docs when stable.
- Prefer installing release binaries + systemd for operators; Makefile remains the developer shortcut.
