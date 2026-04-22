# Runbook: load-test the PIR server

Drive realistic YPIR query traffic against a running `nf-server` to
measure latency, throughput, and stability under load.

The tool is `pir-test load` — a subcommand of the existing `pir-test`
binary. It reuses `PirClient::fetch_proof` end-to-end (real YPIR
encrypt → tier 1 + tier 2 HTTP POST → decrypt → verify), so the
traffic is indistinguishable from a real wallet client.

## When to use

| Scenario | Action |
|----------|--------|
| Validating a new `nf-server` release before fleet rollout. | Run against backup with moderate concurrency. |
| Capacity planning — how many concurrent users can one replica handle? | Ramp concurrency until p99 degrades or errors appear. |
| Regression check after infra changes (droplet resize, kernel upgrade, new snapshot). | Compare JSON summaries before and after. |
| Verifying a new snapshot loads correctly and serves valid proofs. | Run with `--concurrency 1 --duration 30s` — even one successful proof is a good signal. |

## Prerequisites

- **`nullifiers.bin`** — the raw nullifier file for the deployed
  snapshot. Available locally in the repo root after `make bootstrap`,
  or from the published bucket at
  `https://vote.fra1.digitaloceanspaces.com/snapshots/<snapshot_height>/nullifiers.bin`
  where `<snapshot_height>` matches
  [`voting-config.json`](https://valargroup.github.io/token-holder-voting-config/voting-config.json).
  That object exists only after **Publish nullifier snapshot** was run
  for that height with **`include_nullifier_artifacts`** enabled; verify
  size and SHA-256 against `snapshots/<height>/manifest.json` before
  trusting a download.
- **A reachable PIR server** — either `localhost:3000` or a production
  host behind the Caddy TLS reverse proxy.

## Running locally

```bash
# Build (release mode recommended — the client does real YPIR crypto)
cargo build --release -p pir-test

# Minimal run: 2 workers, 30 seconds
./target/release/pir-test load \
  --url http://localhost:3000 \
  --nullifiers ./nullifiers.bin \
  --concurrency 2 \
  --duration 30s \
  --warmup 5s
```

### Startup time

The tool loads `nullifiers.bin` and calls `prepare_nullifiers` to build
the range index. With ~50 M nullifiers this takes **~15–20 s** (release
mode) before the first request is sent. This is a one-time cost per
run; the actual load phase starts after the "Starting load phase" line.

### All flags

| Flag | Default | Description |
|------|---------|-------------|
| `--url` | *(required)* | Server base URL. |
| `--nullifiers` | *(required)* | Path to `nullifiers.bin`. |
| `--concurrency` | `8` | Number of workers (closed-loop mode). Each worker sends `fetch_proof` back-to-back. |
| `--rps` | *(unset)* | Target requests/sec (open-loop mode). Overrides `--concurrency` as the load-shaping mechanism. |
| `--max-inflight` | `256` | Caps in-flight requests in open-loop mode so a slow server can't exhaust memory. |
| `--duration` | `60s` | How long the timed measurement phase runs. Accepts `humantime` syntax (`30s`, `5m`, `1h`). |
| `--warmup` | `10s` | Traffic is sent during warmup but not measured. Set to `0s` to skip. |
| `--json-out` | *(unset)* | Write a JSON summary to this path. |
| `--no-verify` | `false` | Skip Merkle proof verification. Useful to isolate transport+crypto latency from proof correctness. |
| `--seed` | *(random)* | Deterministic RNG seed for query selection. Useful for reproducible runs. |
| `--max-error-rate` | `0.01` | Exit non-zero if the error rate exceeds this fraction. |
| `--slo-p99-ms` | *(unset)* | Exit non-zero if end-to-end p99 exceeds this many milliseconds. |

### Closed-loop vs. open-loop

- **Closed-loop** (default): `--concurrency N` persistent workers. Each
  starts the next request immediately after the previous one completes.
  Throughput is determined by server latency × N. Good for "how fast can
  N clients go?" questions.

- **Open-loop**: `--rps R` spawns requests at a fixed rate regardless
  of how fast the server responds. Caps in-flight via `--max-inflight`.
  Good for "what happens at 10 req/s?" questions — latency isn't masked
  by coordinated omission.

## Running from CI

The
[**Load test PIR**](https://github.com/valargroup/vote-nullifier-pir/actions/workflows/loadtest.yml)
workflow is a `workflow_dispatch` job with these inputs:

| Input | Default | Description |
|-------|---------|-------------|
| `target` | `backup` | Which host to hit (`primary` or `backup`). |
| `concurrency` | `8` | Passed to `--concurrency`. |
| `rps` | *(empty)* | If non-empty, enables open-loop mode. |
| `duration` | `60s` | Passed to `--duration`. |

The workflow builds `pir-test` in release mode, reads
`snapshot_height` from the published
[`voting-config.json`](https://valargroup.github.io/token-holder-voting-config/voting-config.json),
downloads `snapshots/<height>/manifest.json` plus `nullifiers.bin`, checks
size and SHA-256 against the manifest, then runs the load test and uploads
`summary.json` as a build artifact. If the snapshot was published without
**`include_nullifier_artifacts`**, the job fails fast with an instructive
error — re-publish that height with the flag enabled.

The workflow resolves the PIR base URL from the same config
(`pir_endpoints[0]` for primary, `pir_endpoints[1]` for backup), so no
extra secrets are required. The target host must be reachable from
GitHub-hosted runners over HTTPS — the Caddy reverse proxy in front of
`nf-server` handles this.

## Reading the output

### Live progress

During the load phase, a status line prints every 5 seconds:

```
  elapsed=5s  reqs=10  in_flight=2
  elapsed=10s  reqs=19  in_flight=2
```

- **reqs** — total requests completed so far (success + error).
- **in_flight** — currently active requests.

### Summary table

```
=== pir-test load summary ===
url=http://localhost:3000   duration=20s   concurrency=2   completed=37   errors=0 (0.00%)
                        p50        p90        p95        p99        max        n
      end-to-end      1.06s      1.11s      1.12s      1.99s      1.99s       37
       tier1_rtt       42ms       87ms      102ms      117ms      117ms       37
       tier2_rtt      899ms      934ms      964ms      1.78s      1.78s       37
      tier1_srvr       41ms       43ms       51ms       52ms       52ms       37
      tier2_srvr      897ms      928ms      957ms      964ms      964ms       37
```

| Row | What it measures | Source |
|-----|-----------------|--------|
| **end-to-end** | Wall-clock time for one complete proof (tier 0 lookup + tier 1 YPIR + tier 2 YPIR + client decode + verify). | Client-side `Instant` |
| **tier1_rtt** | Round-trip for the `POST /tier1/query` request (upload + server compute + download). | Client-side `Instant` |
| **tier2_rtt** | Round-trip for the `POST /tier2/query` request. | Client-side `Instant` |
| **tier1_srvr** | Server-reported compute time for tier 1 (from `x-pir-server-total-ms` response header). | Server-side |
| **tier2_srvr** | Server-reported compute time for tier 2. | Server-side |

**Key relationships:**

- `end-to-end ≈ tier1_rtt + tier2_rtt` (they run sequentially).
- `tierN_rtt − tierN_srvr = network + upload/download + queue time`.
  On localhost this gap is negligible; over the internet it shows real
  network cost.
- Tier 2 dominates because the database is ~384× larger than tier 1.

### Error breakdown

If errors occurred, a final line lists them:

```
errors by class: timeout=2 http_503=1 verify_fail=1
```

| Class | Meaning |
|-------|---------|
| `timeout` | The HTTP request timed out (reqwest default or server overloaded). |
| `http_503` | Server returned 503 (typically during a snapshot rebuild). |
| `verify_fail` | The proof was returned but `proof.verify(nullifier)` failed — indicates data corruption or a PIR bug. |
| `other` | Any other error (connection refused, DNS failure, malformed response, etc). |

### JSON summary

The `--json-out` file mirrors the table in a machine-readable format:

```json
{
  "url": "http://localhost:3000",
  "duration_s": 20.0,
  "concurrency": 2,
  "completed": 37,
  "errors": 0,
  "error_rate": 0.0,
  "stages": [
    { "name": "end-to-end", "n": 37, "p50_ms": 1058.0, "p90_ms": 1113.0, ... },
    { "name": "tier1_rtt",  "n": 37, "p50_ms": 42.0, ... },
    ...
  ],
  "error_classes": []
}
```

Use this for automated regression checks: compare `p99_ms` and
`error_rate` across runs. The `--slo-p99-ms` and `--max-error-rate`
flags automate this — the process exits non-zero if the threshold is
breached.

## Interpreting results

### Healthy baseline (single replica, no competing traffic)

These numbers are from a local test on Apple M-series (no AVX-512).
Production Intel hosts with AVX-512 are roughly 2× faster on
server-side compute.

| Metric | Local (M-series) | Expected prod (AVX-512) |
|--------|------------------|------------------------|
| Tier 1 server compute | ~40 ms | ~20 ms |
| Tier 2 server compute | ~900 ms | ~400–500 ms |
| End-to-end (localhost) | ~1.0 s | n/a |
| End-to-end (over internet) | n/a | ~1.5–3.0 s (network-dependent) |

### What to look for

- **p99 ≫ p50**: The server is CPU-bound and requests are queuing.
  Reduce concurrency or add a replica.
- **tier1_srvr or tier2_srvr climbing with concurrency**: Classic
  saturation — each request takes longer because it contends for CPU.
  Note: `nf-server` processes one YPIR query at a time per tier; two
  concurrent tier 2 queries serialize.
- **errors > 0**: Any `verify_fail` is a serious bug. `http_503` during
  normal operation (no rebuild in progress) means the server is in a bad
  state. `timeout` at low concurrency means the server is stuck.
- **RTT − server time is large**: Network is the bottleneck, not
  compute. PIR queries upload ~130 KB and download ~12 MB (tier 2), so
  bandwidth matters. GitHub-hosted runners have ~1–10 Gbps, which is
  fine.

### Concurrency scaling test

Run a series with increasing concurrency and compare:

```bash
for c in 1 2 4 8; do
  ./target/release/pir-test load \
    --url http://localhost:3000 \
    --nullifiers ./nullifiers.bin \
    --concurrency $c \
    --duration 30s \
    --warmup 5s \
    --json-out "load-c${c}.json"
done
```

Plot `p50_ms` and throughput (`completed / duration_s`) vs. concurrency.
You should see throughput plateau once the server CPU is saturated.
