# PIR batch query — production baseline (Phase 0)

This document is the **pre-rollout baseline** for the PIR batch-query work
described in the rollout plan. Every later phase (HTTP/wire batching,
server-side batched matmul) reports a delta against the numbers below.

The harness is `pir-test bench-server` ([pir/test/src/bench_server.rs](../pir/test/src/bench_server.rs))
and the raw histogram dumps live in [`baselines/`](baselines/). To regenerate:

```bash
cd vote-nullifier-pir
mise exec -- cargo build --release -p pir-test

# K=1 single query
./target/release/pir-test bench-server \
    --url https://pir.valargroup.org \
    --nullifiers ./nullifiers.bin \
    --iterations 30 --warmup 3 \
    --batch-size 1 --mode single \
    --seed 42 \
    --label primary-k1-single \
    --json-out docs/baselines/primary-k1-single.json

# K=5 parallel (today's prod path: PirClient::fetch_proofs uses try_join_all)
./target/release/pir-test bench-server \
    --url https://pir.valargroup.org \
    --nullifiers ./nullifiers.bin \
    --iterations 30 --warmup 3 \
    --batch-size 5 --mode parallel \
    --seed 42 \
    --label primary-k5-parallel \
    --json-out docs/baselines/primary-k5-parallel.json

# K=5 sequential (naive for-loop; useful upper bound)
./target/release/pir-test bench-server \
    --url https://pir.valargroup.org \
    --nullifiers ./nullifiers.bin \
    --iterations 15 --warmup 3 \
    --batch-size 5 --mode sequential \
    --seed 42 \
    --label primary-k5-sequential \
    --json-out docs/baselines/primary-k5-sequential.json

# Same triplet against pir-backup.valargroup.org.
```

## Methodology

- **Endpoints**: production PIR fleet referenced by
  [`token-holder-voting-config/voting-config.json`](../../token-holder-voting-config/voting-config.json):
  `https://pir.valargroup.org` (primary) and `https://pir-backup.valargroup.org` (backup).
- **Server snapshot**: height **3,317,500**, 24,962,943 ranges, root29
  `c338a0…3a3f` (verified via `GET /root` on 2026-04-26).
- **Local nullifier set**: `vote-nullifier-pir/nullifiers.bin` at height
  3,304,638 (49,897,273 nullifiers → 24,948,653 ranges). Production has a
  superset of these ranges, so `nf_lo + 1` query values picked from local
  ranges land in production punctured ranges with overwhelming probability
  (only fails if a nullifier added between heights 3,304,638 and 3,317,500
  hashes to exactly `nf_lo + 1` — vanishingly unlikely). All 6 runs below
  observed **0 errors / 0 verify failures**.
- **Modes**:
    - `single` — one `PirClient::fetch_proof_with_timing` per iteration. K=1.
    - `parallel` — K calls to `fetch_proof_with_timing` joined with
      `futures::future::join_all` per iteration. This mirrors what the SDK
      does today via `PirClient::fetch_proofs`
      (see [pir/client/src/lib.rs](../pir/client/src/lib.rs) lines 260–286,
      which uses `try_join_all`; we use `join_all` so a single per-note
      error doesn't drop timings for the rest of the batch).
    - `sequential` — K awaits in a `for` loop. Useful as a naive upper bound.
- **Sampling**:
    - Wall-clock (per-iteration): K=1/K=5 parallel use 30 measured iterations
      after 3 warmup iterations. K=5 sequential uses 15 measured iterations
      after 3 warmup (each iteration is ~17s).
    - Per-query metrics (RTT, server compute, bytes): K samples per
      iteration, so K=5 parallel reports n=150 across 30 iterations;
      K=5 sequential reports n=75 across 15 iterations.
- **Observer**: `Romans-MacBook-Pro-2.local` on residential WiFi
  (uplink ≈ 5–10 Mbit/s). The high `net+queue` numbers below are dominated
  by upload time on this link, not server round-trip. **Phase 1 will retest
  from a fixed CI runner** so the numbers move primarily because of code,
  not network conditions.
- **Reproducibility**: `--seed 42` for query selection. Re-running the same
  command on the same nullifiers.bin issues identical query indices; the
  server response sizes are deterministic per scenario (see "Bandwidth"
  below) — only timings and TLS jitter vary.

## Wall-clock per delegation (K = 5 notes)

`p99 max` is the worst-case observed over the run; iteration counts above.

| Endpoint | Mode | p50 | p90 | p95 | p99 |
|---|---|---:|---:|---:|---:|
| primary | K=1 single (×5 mentally) | 3.36 s | 4.30 s | 4.96 s | 5.70 s |
| primary | **K=5 parallel (today)** | **4.65 s** | **7.89 s** | **8.66 s** | **8.71 s** |
| primary | K=5 sequential | 16.74 s | 19.53 s | 22.27 s | 22.27 s |
| backup  | K=1 single (×5 mentally) | 3.15 s | 3.51 s | 5.12 s | 5.61 s |
| backup  | **K=5 parallel (today)** | **4.31 s** | **5.20 s** | **5.37 s** | **7.17 s** |
| backup  | K=5 sequential | 16.89 s | 19.99 s | 22.09 s | 25.40 s |

What this says about the current SDK path:

- A 5-note delegation today takes **~4.5 s p50 / ~8.5 s p99** end-to-end on
  primary, **~4.3 s p50 / ~7.2 s p99** on backup. Tier 2 is the dominating
  tier (~3 s of the 4.5 s); tier 1 contributes only the last ~0.3 s on the
  critical path because the parallel mode hides per-note tier-1 latency.
- Going from K=1 to K=5 parallel costs **+1.3 s at p50 / +3 s at p99** on
  primary — i.e. ~30 % more wall-clock for 5× the work. The K=5 parallel
  bench also finds **30–82 % higher tier-2 server compute** under
  contention vs. the K=1 baseline (see next section), which is what we
  expect to recover once the server can amortise DB streaming across K
  queries (Phase 2).
- Naive `for-loop` sequential code path costs ~17 s p50 / ~25 s p99 — so
  any client that doesn't already use the parallel `fetch_proofs` path
  on the wire is paying 4× over today's baseline.

## Per-tier RTT (per query)

Tier 1 (n=30 for single, n=150 for K=5 parallel, n=75 for K=5 sequential):

| Endpoint / Mode | p50 | p95 | p99 |
|---|---:|---:|---:|
| primary K=1 single      | 290 ms |   309 ms | 2140 ms |
| primary K=5 parallel    | 508 ms |   581 ms | 2240 ms |
| primary K=5 sequential  | 286 ms |   719 ms | 2060 ms |
| backup  K=1 single      | 275 ms |  2020 ms | 2020 ms |
| backup  K=5 parallel    | 341 ms |   910 ms | 2170 ms |
| backup  K=5 sequential  | 275 ms |  2030 ms | 2200 ms |

Tier 2:

| Endpoint / Mode | p50 | p95 | p99 |
|---|---:|---:|---:|
| primary K=1 single      | 2950 ms | 4270 ms | 5290 ms |
| primary K=5 parallel    | 3220 ms | 6960 ms | 7930 ms |
| primary K=5 sequential  | 2920 ms | 3460 ms | 4360 ms |
| backup  K=1 single      | 2760 ms | 3470 ms | 3620 ms |
| backup  K=5 parallel    | 3140 ms | 3930 ms | 4340 ms |
| backup  K=5 sequential  | 2980 ms | 3760 ms | 4860 ms |

## Per-tier upload / server / download time decomposition (per query)

Three numbers per tier — upload-to-server (`upload_to_server_ms`), server
total (`server_total_ms`), and download-from-server
(`download_from_server_ms`) — read off the response headers and request-side
clocks. They are *estimates* whose sum can drift a few percent from
`rtt_ms` due to header parsing and clock skew, but they're the cleanest
attribution we have without instrumenting both ends.

Tier 1 (24 KB response — download is essentially free):

| Endpoint / Mode | upload p50 | server p50 | download p50 | download p99 |
|---|---:|---:|---:|---:|
| primary K=1 single      | 225 ms | 63 ms |   0 ms | 1843 ms |
| primary K=5 parallel    | 229 ms | 66 ms | 205 ms | 1744 ms |
| primary K=5 sequential  | 223 ms | 62 ms |   0 ms |  868 ms |
| backup  K=1 single      | 221 ms | 53 ms |   0 ms |  215 ms |
| backup  K=5 parallel    | 236 ms | 56 ms |   1 ms |  245 ms |
| backup  K=5 sequential  | 221 ms | 53 ms |   0 ms |  947 ms |

Tier 2 (336 KB response — download is the biggest piece of the wall):

| Endpoint / Mode | upload p50 | server p50 | **download p50** | download p99 |
|---|---:|---:|---:|---:|
| primary K=1 single      | 228 ms |  942 ms | **1751 ms** | 4073 ms |
| primary K=5 parallel    | 233 ms | 1220 ms | **1101 ms** | 4923 ms |
| primary K=5 sequential  | 226 ms |  936 ms | **1738 ms** | 2767 ms |
| backup  K=1 single      | 224 ms |  839 ms | **1494 ms** | 2020 ms |
| backup  K=5 parallel    | 754 ms | 1530 ms |  **855 ms** | 1934 ms |
| backup  K=5 sequential  | 230 ms |  835 ms | **1817 ms** | 3488 ms |

What this means:

- Tier 2 **download is the single largest component of the wall** on this
  observer (336 KB × 5 = 1.68 MB returned per delegation today, all on the
  critical path). Phase 1 does **not** change tier-2 download bytes — the
  server still sends back 5 individual encrypted column vectors. The
  Phase 1 wire-batching win is therefore upload-side only.
- The `parallel` mode `download p50` numbers are roughly K× lower than
  `K=1 single` because the K HTTP responses share the underlying TCP/TLS
  pipe — each one *individually* takes longer to drain (notice the higher
  `p99`). For wall-clock that's a wash; the parallel critical path is
  bounded by max-of-K not K×, so the headline wall-clock numbers above
  are the right thing to look at.
- Tier 2 `upload p50` jumps from ~230 ms to ~750 ms specifically on
  **backup K=5 parallel** — the backup uplink path gets congested by 5
  simultaneous 784 KB pub_params blobs. This is exactly what Phase 1's
  shared pub_params is supposed to fix.

## Per-tier server compute (per query)

This is the time the server spent inside `perform_online_computation_simplepir`,
read off `x-pir-server-compute-ms`. It excludes upload, queue, and download.

| Endpoint / Mode | Tier 1 p50 | Tier 1 p99 | Tier 2 p50 | Tier 2 p99 |
|---|---:|---:|---:|---:|
| primary K=1 single      |  63 ms |  81 ms |  942 ms |  975 ms |
| primary K=5 parallel    |  66 ms |  95 ms | 1220 ms | 1770 ms |
| primary K=5 sequential  |  62 ms |  67 ms |  936 ms |  992 ms |
| backup  K=1 single      |  53 ms |  66 ms |  839 ms |  862 ms |
| backup  K=5 parallel    |  56 ms | 329 ms | 1530 ms | 1970 ms |
| backup  K=5 sequential  |  53 ms |  56 ms |  835 ms |  879 ms |

The K=5 parallel rows show the contention cost of having five concurrent
PIR computations fight over the server's Rayon pool — tier 2 server
compute climbs **+30 % on primary** and **+82 % on backup** at p50 vs. the
single-query baseline. Phase 2 (one batched matmul instead of five
matvecs) is the lever that recovers this and goes further by streaming
the DB once.

## Per-tier bandwidth (per query, deterministic)

Per query (one nullifier going through one tier):

| Tier | Upload | Download | Total |
|---|---:|---:|---:|
| Tier 1 | **544 KB** | **24 KB**  | **568 KB**  |
| Tier 2 | **784 KB** | **336 KB** | **1.09 MB** |
| **Sum, K=1 (one note, both tiers)** | **1.33 MB** | **360 KB** | **1.69 MB** |

K=5 today (5 independent queries per tier — current production behavior):

| Tier | Upload | Download | Total |
|---|---:|---:|---:|
| Tier 1 | 5 × 544 KB = **2.72 MB** | 5 × 24 KB = **120 KB**  | **2.84 MB** |
| Tier 2 | 5 × 784 KB = **3.92 MB** | 5 × 336 KB = **1.68 MB** | **5.60 MB** |
| **Sum, K=5 today (one delegation)** | **6.64 MB** | **1.80 MB** | **8.44 MB** |

K=5 Phase 1 projected (1× shared `pack_pub_params` per tier + 5× small
`q.0` vectors per tier; download is unchanged since per-query response
shape is preserved and Phase 1 just bundles the K responses into one
HTTP body):

| Tier | Upload | Download | Total |
|---|---:|---:|---:|
| Tier 1 | **≤ 0.70 MB** | **120 KB**  | **≤ 0.82 MB** |
| Tier 2 | **≤ 1.00 MB** | **1.68 MB** | **≤ 2.68 MB** |
| **Sum, K=5 Phase 1 (projected)** | **≤ 1.70 MB** | **1.80 MB** | **≤ 3.50 MB** |

Reduction is concentrated on the upload side and asymmetric by tier:

- **Tier 1** total drops 2.84 MB → ≤ 0.82 MB (~**3.5×**). Tier 1 has the
  smallest download fraction so the upload win dominates.
- **Tier 2** total drops 5.60 MB → ≤ 2.68 MB (~**2.1×**). Tier 2 is
  download-heavy (1.68 MB stays put), so the same upload saving moves
  the needle less in relative terms — but it's the larger **absolute**
  byte saving (≈ 2.92 MB out of the 4.94 MB total Phase 1 cut).
- **Total** drops 8.44 MB → ≤ 3.50 MB (~**2.4×**).

Phase 2 is a server-internal swap with no wire format changes, so the
per-tier and total byte numbers are identical to Phase 1 above. Phase 2
wins are server compute time, not bytes.

Today the client uploads a fresh `pack_pub_params` blob with every query.
Inside `pack_pub_params` is the bulk of the upload bytes (the simplepir
`q.0` query vector is much smaller). Phase 1 sharing the pub_params
across the K queries in a batch is therefore the biggest single
bandwidth win available: the K=5 total upload should drop from 6.64 MB
to roughly **1× pub_params + 5× q.0 ≈ ~1.5–2 MB** per delegation. We'll
measure the exact number from a Phase 1 staging server and add it back
to this doc when Phase 1 lands.

Download bytes are **unchanged by Phase 1 or Phase 2** — both phases
keep the wire-format payloads per query identical, so a K=5 batch still
returns 5 × (24 KB tier 1 + 336 KB tier 2) = 1.80 MB. The wall-clock
gain on the download side comes entirely from re-using a single TCP/TLS
connection and HTTP/2 multiplexing, which the parallel mode already
exploits today (see the parallel `download p50` ≈ K× lower than the
single-query baseline above). The only way to actually shrink download
bytes would be a follow-up that pushes the Tier 2 row hint set down to
the client up-front, which is out of scope for this batching effort.

## Phase 1 / Phase 2 success criteria

These are the gates that block merging the SDK swap to the batched path
on a tier server. All comparisons are against the **K=5 parallel** rows
above for the same endpoint, measured with the same harness from a
fixed observer host (CI runner, region tagged in JSON). All numbers are
**p50 unless a stronger percentile is named**.

### Phase 1 (HTTP/wire batching, K independent matvecs server-side)

Must hit at least:

- **Wall-clock**: ≥ 1.0 s reduction at p50 vs. K=5 parallel (one tier-2
  RTT class equivalent) on the slowest of {primary, backup}. p99 must
  not regress.
- **Tier 2 upload bytes**: drop from 5 × 784 KB ≈ 3.92 MB to ≤ 1.0 MB
  per K=5 batch — i.e. one shared `pack_pub_params` plus K small `q.0`
  vectors.
- **Tier 1 upload bytes**: drop from 5 × 544 KB ≈ 2.72 MB to ≤ 0.7 MB.
- **Download bytes**: must not regress (expected unchanged at 5 × 24 KB
  tier 1 + 5 × 336 KB tier 2 = 1.80 MB per K=5 batch — Phase 1 keeps
  per-query response shape identical and just bundles the K responses
  into one HTTP body).
- **Server compute**: must not regress (Phase 1 still does K matvecs;
  this is just a sanity check that the wire format change didn't add
  new server-side work).
- **Error rate**: 0 across `iterations × batch_size` for both endpoints.
- **Capability/fallback**: SDK paths older than the rollout flip
  cleanly fall back to the legacy `/tier{1,2}/query` route — verified
  by integration test, not bench gate.

### Phase 2 (server-side batched matmul, same wire route)

Must hit at least:

- **Tier 2 server_compute p50** ≥ **1.5×** lower than the Phase 1
  baseline for the same endpoint at K=5 (the kernel is DRAM-bound on
  tier 2, so streaming the 784 KB rows once instead of 5× should give
  roughly the K=5 amortisation factor).
- **Tier 1 server_compute p50** must not regress (tier 1 may already
  be CPU-bound on the small DB; we don't gate on improvement).
- **Wall-clock at p50** drops by at least the per-query tier-2
  server_compute delta × 1 (i.e. the server wins flow through to the
  wall).
- **No change to wire format** (Phase 2 is a server-internal swap only).

### Phase 0 (this doc)

- All six baselines committed under [`baselines/`](baselines/) with
  3-iteration warmup, 30 / 30 / 15 measured iterations and matching
  `--seed 42`.
- `pir-test bench-server --help` documents the harness.
- Wall-clock and per-tier server_compute summarised in the tables
  above with the same percentile shape used for the Phase 1 / 2 gates.
