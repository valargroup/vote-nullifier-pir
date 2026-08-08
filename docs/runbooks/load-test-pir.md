# Runbook: load-test the PIR server

Use `pir-test load` to drive real end-to-end traffic through
`PirClient::fetch_proof`: Tier 0 lookup, one encrypted Tier 1 request,
decryption, subtree reconstruction, and proof verification.

## Prerequisites

- A `nullifiers.bin` matching the deployed snapshot. When downloading it from
  `snapshots/<network>/<height>/`, verify it against `manifest.json` and ensure
  its dataset marker identifies Ironwood dataset version 2.
- A reachable `nf-server` endpoint.
- A release build of the test harness for representative client crypto timing.

## Basic run

```bash
cargo build --release -p pir-test

./target/release/pir-test load \
  --url http://localhost:3000 \
  --nullifiers ./nullifiers.bin \
  --concurrency 2 \
  --duration 30s \
  --warmup 5s
```

Closed-loop mode is the default: each of `--concurrency N` workers starts its
next proof as soon as the previous proof completes.

For open-loop traffic, set a target request rate and an in-flight cap:

```bash
./target/release/pir-test load \
  --url https://pir.example.org \
  --nullifiers ./nullifiers.bin \
  --rps 10 \
  --max-inflight 64 \
  --duration 5m \
  --warmup 30s \
  --json-out load-summary.json
```

## Important flags

- `--url`: required server base URL.
- `--nullifiers`: required matching raw nullifier file.
- `--concurrency`: closed-loop worker count; default 8.
- `--rps`: enables open-loop scheduling at the requested rate.
- `--max-inflight`: bounds open-loop queued work; default 256.
- `--duration`: measured phase; default 60 seconds.
- `--warmup`: unmeasured traffic before collection; default 10 seconds.
- `--json-out`: writes a machine-readable summary.
- `--no-verify`: skips final Merkle proof verification.
- `--seed`: fixes query selection for repeatability.
- `--max-error-rate`: nonzero exit threshold; default 0.01.
- `--slo-p99-ms`: optional end-to-end p99 failure threshold.

## Output

The summary reports:

- `end-to-end`: full proof retrieval and reconstruction latency;
- `tier1_rtt`: the single `POST /tier1/query` round trip;
- `tier1_srvr`: server-reported Tier 1 processing time when timing headers are
  present;
- completed requests, errors, error rate, and classified failures.

For a healthy run, `end-to-end` should be close to `tier1_rtt` plus local
decode and seven-level subtree reconstruction. There is no second sequential
PIR round trip in dataset version 2.

## Recommended validation sequence

1. Smoke the new snapshot with concurrency 1 for 30 seconds and proof
   verification enabled.
2. Run the same command against primary and backup.
3. Repeat a pinned baseline configuration, preserving URL, nullifier file,
   seed, duration, warmup, and host.
4. Increase concurrency gradually and watch p95/p99, server compute, error
   rate, CPU, memory, and network throughput.
5. Save the JSON summaries with the release and snapshot identifiers.

Example regression run:

```bash
./target/release/pir-test load \
  --url https://pir.example.org \
  --nullifiers ./nullifiers.bin \
  --concurrency 8 \
  --duration 5m \
  --warmup 30s \
  --seed 42 \
  --max-error-rate 0.001 \
  --slo-p99-ms 2000 \
  --json-out load-v2-c8.json
```

## Troubleshooting

- HTTP 503: the server is still starting/rebuilding or is not ready.
- Decode failures: client and server parameters or snapshot versions differ.
- Proof verification failures: the `nullifiers.bin` query source does not
  match the server snapshot, or tier/root data is inconsistent.
- Rising `tier1_srvr` with concurrency: server CPU or memory bandwidth is
  saturated.
- Stable server time but rising RTT: inspect network, proxy, and queueing.
- Open-loop in-flight saturation: reduce `--rps` or increase capacity; do not
  raise `--max-inflight` merely to hide an overloaded server.
