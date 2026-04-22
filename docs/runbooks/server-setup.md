# Runbook: Setup PIR Server

This runbook explains how to setup a vote nullifier private information retireval (PIR) server.

We recommend a fully automated one-CLI-command solution that:
- Downloads the latest binaries
- Configures the service per the recommended parameters
- Creates an automated systemcl service that auto-restarts on start-up and on-failure.
- Bootstraps from pre-computed snapshots
- Installs the binary
- Serves PIR queries

For operators, preferring manual setup and for debugging purposes, we outline manual approaches to get started below/

There are 2 modes for starting up:
1. **Bootsrapped** - pir server downloads pre-computed snapshot data from Valar Group cloud instances.
2. **Synced** - pir server syncs Zcash mainnet blocks from genesis up until the pre-configured height. It then constructs a 3-tier Merkle tree of nullifiers per specification [here](../pir-tree-spec.md), encodes it into the database and saves the snapshot to disk to avoid repeating expensive work for every restart.

## Recommended Hardware

We recommend running a 4 Intel vCPU machine with AVX-512 instruction set support, 32 GB RAM and have at least 35 GB available on disk.

## Startup Time Estimate

The estimates are provided, assuming the recommended hardware.

30 minutes in bootstrap mode.

90 seconds in synched mode.

**TODO**: validate above and add Rationale entry

## Bootstrapped Mode

Run

```bash
make serve
```

Policy: if local state is unusable AND we couldn't bootstrap, this is a hard failure. In that case, either fix the bootstrap process or sync manually.

**What happens in the background?**

Each step below is a soft-check. For example, if we do not have local data for start-up, fall back to downloading. The system errors only if all methods of achieving start-up requirements fail.

1. Fetch `token-holder-voting-config/voting-config.json`
  * Parses `snapshot_height`
  * If unavailable, fail start-up.
2. Validate that the fetched height is equal to the locally stored height.
   * If equal, we are done and are ready to start.
   * If not equal, attempt to automatically sync up to the expected height. Compare the downloaded data hashes against a manifest file. If success, the server is ready to start.
      * If syncing fails but we have raw nullifier data (`nullifiers.bin`, `nullifiers.checkpoint`, `nullifiers.tree`, `nullifiers.index`) present locally with the correct expected height, automatically export the tiers `tier0.bin`, `tier1.bin`, `tier2.bin`, and `pir_root.json`
      and proceed to starting. Note: this workload matches `nf-server export` (or `make export-nf`); `make ingest` runs sync then export.
      * If the local height is above the expected voting height, prompt the user if they want to
      download the correct pre-computed data to be able to serve from an earlier height. 

Possible fatal errors:
- `voting-config.json` fails to be fetched.
   * To resolve, confirm that `SVOTE_VOTING_CONFIG_URL` is correctly set.
- Snapshot data fails to be fetched and no raw nullifier data prent
   * To resolve, confirm that `SVOTE_PRECOMPUTED_BASE_URL` is correctly set.

## Synced Mode

Run

```bash
# Sync nullifiers from lightwalletd, then export PIR tier files (one command)
make ingest
```

**What happens in the background?**

1. **Sync** (`nf-server ingest` inside `make ingest`): Orchard nullifiers from NU5 activation up until `SYNC_HEIGHT`, or chain tip when `SYNC_HEIGHT` is unset (see the [Makefile](../../Makefile)). Produces `nullifiers.bin` and `nullifiers.checkpoint`.
2. **Export** (same `make ingest`, after a successful sync): runs `nf-server export`, builds the Merkle tree sidecar where applicable, and writes `tier0.bin`, `tier1.bin`, `tier2.bin`, and `pir_root.json` under `pir-data/`. Use `make export-nf` alone when nullifiers are already synced and only tiers need rebuilding.
3. **Serve**: start the HTTP server (for example `make serve`).

```bash
# After `make ingest`, tier files exist under `pir-data/`; then serve locally.
make serve
```

## Connfiguring The Service

TODO: fill in the details for how to do it.

## Observability

The server is capable of emitting events, metrics and errors into Sentry. Configure your setup at [sentry.com](https://sentry.com), copy the DSN and set it as `SENTRY_DSN` environment variable. 

## Rationale

### Recommended Hardware

- AVX-512 instruction set meaningfully accelerates PIR packing/compression operations
- 35 GB is sufficient for toring the original ~2 GB nullifier data set and ~7 GB pre-computed tiers, the rest is buffer.
- 4 vCPUs allow for parallelizing matrix-vector for applying the

## Useful Configruation

Attached are the environment variables to override the default configuration.

### Ingest

```bash
# Directory containing tier0.bin, tier1.bin, tier2.bin, and pir_root.json
# Default: ./pir-data
SVOTE_PIR_DATA_DIR

# Zcash mainnet RPC URL.
# Default: https://zec.rocks:443
SVOTE_PIR_MAINNET_RPC_URL

# Stop syncing at this block height (must be a multiple of 10).
# If unset, sync until the latest multiple of 10.
SVOTE_PIR_MAX_HEIGHT

# Delete stale sidecar files (nullifiers.tree, tier files) after ingestion.
SVOTE_PIR_INVALIDATE
```

### Serve

```bash
# Listen port
# Default: 3000
SVOTE_PIR_PORT

# Directory containing tier0.bin, tier1.bin, tier2.bin, and pir_root.json
# Default: ./pir-data
SVOTE_PIR_DATA_DIR

# Zcash mainnet RPC URL.
# Default: https://zec.rocks:443
SVOTE_PIR_MAINNET_RPC_URL

# URL of the published `voting-config.json` whose `snapshot_height`
# is treated as the canonical height every PIR replica should
# serve. Set to an empty string to disable the startup
# self-bootstrap entirely (operator manages snapshots manually).
# Default: https://valargroup.github.io/token-holder-voting-config/voting-config.json
SVOTE_PIR_VOTING_CONFIG_URL

# Bucket origin for pre-computed PIR snapshots (matches the
# admin UI's `SVOTE_PRECOMPUTED_BASE_URL`). The bootstrap fetches
# `<base>/snapshots/<height>/{manifest.json,tier0.bin,...}`.
# Trailing slashes are trimmed. Empty disables the download
# portion of the bootstrap (operators relying on out-of-band
# staging can keep the voting-config height check enabled).
# Default: https://vote.fra1.digitaloceanspaces.com
# Note that the system hardcodex a suffix "/snapshots"
SVOTE_PIR_PRECOMPUTED_BASE_URL
```

## Tagging Rules

We follow semantic versioning (sem-ver) policy.

## Open Questions:

- What happens when the voting-config.json is unavailable. What height is chosen then?
- What is the difference between `nullifiers.checkpoint` and `nullifiers.index`? Do we need both?
- Can we remove `POST /snapshot/prepare`?
- Should we keep a CHANGELOG and define a release/tag policy so that integrators can track what can be broken and when.

## TODO
- Can we merge `data_dir` (`nullifiers.bin`, `nullifiers.checkpoint`, `nullifiers.tree`, `nullifiers.index` ) and `pir_data_dir` `tier0.bin`, `tier1.bin`, `tier2.bin`, and `pir_root.json`? 

   * We should also make it such that if there is no `pir_data_dir`, we can still leverage `data_dir` to start using `make serve`. Just fails at the moment.
   * At the moment, the flow seems to be to run `make export-nf` manually to export these. Instead, it should be auto exported in the case of failure and started from.
- `make ingest` now runs export after a successful sync; resume-from-partial-run behavior is not implemented yet.
- Add optional step for setting up a DO instance from Terraform.
- Confirm if `SVOTE_VOTE_CHAIN_URL` is needed. Remove if not or added to useful configurations.
- Instead of having users run Makefile commands, can we just have them install the binaries and then interact with the binaries directly instead?
