# Runbook: Setup PIR Server

This runbook explains how to set up a vote nullifier private information retrieval (PIR) server.

**Linux (production):** install the release-tagged binary, bootstrap env pins, and the `nullifier-query-server` systemd unit in one step:

```bash
curl -fsSL https://vote.fra1.digitaloceanspaces.com/start_pir.sh | sudo bash
```

Piping a remote script into `bash` trusts the publisher and your TLS path; inspect the script first (`curl -fsSL …/start_pir.sh`) when your policy requires it. The same installer is published per voting-config `snapshot_height` at `…/scripts/start_pir/<snapshot_height>/start_pir.sh` (see [deploy-setup.md](../deploy-setup.md)). On minimal Ubuntu/Debian images the script installs **`curl`** and **`ca-certificates`** via `apt-get` when needed. For Caddy/TLS, extra environment variables, or a manual binary download, see [deploy-setup.md](../deploy-setup.md).

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

- On the order of tens of minutes in **bootstrap** mode (CDN download size and link dominate).
- **Synced** mode depends on how far behind the data directory is; a fresh sync to mainnet tip is much longer than 90 seconds unless the range is tiny.

**TODO:** Validate numbers on reference hardware and extend the Rationale section.

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
