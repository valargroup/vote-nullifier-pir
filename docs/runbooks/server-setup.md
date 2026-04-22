# Runbook: Setup PIR Server

This runbook explains how to set up a vote nullifier private information retrieval (PIR) server.

We recommend a fully automated one-CLI-command solution that:

- Downloads the latest binaries
- Configures the service per the recommended parameters
- Creates an automated **systemd** unit that auto-restarts on start-up and on failure
- Bootstraps from pre-computed snapshots
- Installs the binary
- Serves PIR queries

For operators who prefer manual setup, or for debugging, manual approaches are outlined below.

There are two modes for starting up:

1. **Bootstrapped** — the PIR server downloads pre-computed snapshot data from Valar Group–hosted object storage.
2. **Synced** — the PIR server ingests Zcash mainnet Orchard nullifiers from lightwalletd up to a chosen height (or chain tip), then exports a 3-tier representation per [PIR tree spec](../pir-tree-spec.md), writing flat files to disk so expensive work is not repeated on every restart.

## Recommended hardware

We recommend a 4 Intel vCPU machine with AVX-512 support, 32 GB RAM, and at least 35 GB free disk.

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

**Policy:** If local PIR tier state is unusable and bootstrap cannot fix it (for example nothing valid under `pir-data/` and CDN fetch failed), startup fails after bootstrap. Fix bootstrap configuration, network, or use **Synced** mode / pre-staged files.

**What happens in the background?**

Behavior matches `nf-server serve` startup: index maintenance on the nullifier `data_dir`, then [bootstrap](../deploy-setup.md#2-configure-the-snapshot-bootstrap) (voting-config + optional CDN tier fetch), then loading mmap’d tier files. Network steps log warnings and may fall through to local disk; the process still **errors** if tier files cannot be loaded. For exact metrics and watchdog behavior, see [Deploy setup](../deploy-setup.md).

1. Fetch `voting-config.json` (default URL in [Deploy setup](../deploy-setup.md)).
   - Read `snapshot_height` when present.
2. Compare canonical height to local `pir_root.json` height.
   - If equal, continue to load and serve.
   - If not equal, attempt to download the snapshot for the expected height from the pre-computed base URL (`…/snapshots/<height>/…`), verify hashes from `manifest.json`, and install into `pir-data/`.
3. **Aspirational (not all paths are implemented today):** if CDN sync fails but raw nullifier files exist at the expected height, an operator may run `make export-nf` (or `nf-server export`) to build tiers locally. The server does not currently prompt interactively if local height is above the voting height; reconcile data out of band.

**Fatal errors (typical):**

- Tier load fails after bootstrap (missing or corrupt `tier0.bin` / `pir_root.json`, etc.).
- `voting-config.json` cannot be fetched **and** there is no usable local snapshot when the server requires one.

Resolution hints:

- Confirm `SVOTE_VOTING_CONFIG_URL` (or the name your release documents) points at the intended config.
- Confirm `SVOTE_PRECOMPUTED_BASE_URL` when using CDN bootstrap.

## Synced mode

Ingest nullifiers, export PIR tiers, then serve:

```bash
make ingest
make export-nf
```

**What happens in the background?**

1. **Ingest** (`make ingest` → `nf-server ingest`): sync Orchard nullifiers from NU5 activation up through `SYNC_HEIGHT`, or up to **mainnet chain tip** when `SYNC_HEIGHT` is unset (see the [Makefile](../../Makefile) — there is no “round down to latest multiple of 10” behavior when height is unset). Writes `nullifiers.bin` and `nullifiers.checkpoint`; index sidecar behavior is described in the `nf-ingest` crate docs.
2. **Export** (`make export-nf` → `nf-server export`): builds the Merkle tree sidecar where applicable and writes `tier0.bin`, `tier1.bin`, `tier2.bin`, and `pir_root.json` under `pir-data/` (paths depend on `DATA_DIR` / `PIR_DATA_DIR` in the Makefile).
3. **Serve** (`make serve`): with tiers present, the same HTTP server starts; if voting-config and precomputed URLs remain at defaults, startup bootstrap may still refresh tiers from the CDN when the published height moves—operators doing fully local workflows often clear or override those env vars (see [Deploy setup](../deploy-setup.md)).

```bash
# After manual ingest + export, tier files are local; CDN bootstrap may
# still run unless you disable it—see deploy-setup.md.
make serve
```

## Configuring the service

Use the systemd and `/etc/default/nf-server` (or `.env`) steps in [Deploy setup](../deploy-setup.md#binary-setup-operators). Caddy TLS and fleet restart ordering (backup then primary) are documented there and in [restart-pir-fleet.md](restart-pir-fleet.md) where applicable.

## Observability

The server can emit errors and traces to Sentry. Create a project at [sentry.io](https://sentry.io), copy the DSN, and set `SENTRY_DSN` (see [Deploy setup](../deploy-setup.md#snapshot-stale-alerting) for watchdog interaction).

## Rationale

### Recommended hardware

- AVX-512 meaningfully accelerates PIR packing and query-side linear algebra.
- Roughly 35 GB disk is enough for ~2 GB nullifier data, ~7 GB tier files, and working space. The rest is headroom.
- 4 vCPUs help parallelize large matrix–vector steps during queries.

## Useful configuration


Makefile-oriented development variables (see [Makefile](../../Makefile)):

| Variable | Role |
|----------|------|
| `DATA_DIR` | Directory for `nullifiers.bin`, checkpoint, tree sidecar |
| `PIR_DATA_DIR` | Output directory for tier blobs (default `$(DATA_DIR)/pir-data`) |
| `LWD_URL` | First lightwalletd gRPC URL passed to ingest/serve |
| `SYNC_HEIGHT` | Optional; if set, must be a multiple of 10; caps ingest |
| `PORT` | HTTP listen port for `make serve` |

### Ingest (CLI / future env parity)

Planned names (see tracking issues / PRs): `SVOTE_PIR_MAX_HEIGHT`, `SVOTE_PIR_INVALIDATE`, data directory overrides. Today use Makefile variables or `nf-server ingest --help`.

### Serve (CLI / env)

Planned names include `SVOTE_PIR_PORT`, `SVOTE_PIR_DATA_DIR`, lightwalletd URL, `SVOTE_PIR_VOTING_CONFIG_URL`, `SVOTE_PIR_PRECOMPUTED_BASE_URL`. **Today** the binary and [Deploy setup](../deploy-setup.md) document `SVOTE_VOTING_CONFIG_URL`, `SVOTE_PRECOMPUTED_BASE_URL`, `SVOTE_DATA_DIR`, `SVOTE_PIR_DATA_DIR`, `SVOTE_MAINNET_RPC_DIR`, watchdog and bootstrap timeouts, etc.

## Tagging and releases

Semantic versioning applies to `nf-server` releases (`v*` tags drive CI artifacts). Integrators should pin **binary version** and the **voting snapshot height** they expect.

## Decisions (formerly open questions)

| Topic | Decision |
|-------|----------|
| Voting-config unavailable when its URL is set | **Target policy:** non-empty URL implies a required canonical `snapshot_height`; fetch/parse failure should fail startup (enforcement may land after this doc update; see repository PRs). **Offline / manual disks:** set voting-config URL to empty and stage `pir-data/` yourself ([Deploy setup](../deploy-setup.md)). |
| `nullifiers.checkpoint` vs `nullifiers.index` | **Checkpoint** is the durable commit point (height + byte offset into `nullifiers.bin`). **Index** records per-batch offsets for export at specific aligned heights. Both are kept. |
| Remove `POST /snapshot/prepare`? | **Keep** for in-service rebuilds when nullifier files live on the server; fleet CDN workflow does not replace every ops scenario. |
| CHANGELOG and tag policy | **Yes** — maintain `CHANGELOG.md` and document SemVer + `v*` release tagging for integrators. |

## TODO (product / engineering backlog)

- Merge or unify `data_dir` (nullifier flat files) and `pir_data_dir` (tier blobs) so `make serve` can start from nullifiers alone when tiers are missing (auto-export path).
- `export-nf` as a separate step vs integrated ingest stages with resume.
- Optional: Terraform / DigitalOcean droplet setup pointer to [vote-infrastructure](https://github.com/valargroup/vote-infrastructure) (see [Deploy setup](../deploy-setup.md#infrastructure)).
- Document `SVOTE_VOTE_CHAIN_URL` (optional; active-round guard for `POST /snapshot/prepare`) in deploy-setup when stable.
- Prefer installing release binaries + systemd for operators; Makefile remains the developer shortcut.
