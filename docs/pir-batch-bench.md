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

# Phase 0.5.3: K=5 single-tls (HTTP/1.1, single TCP, sequential — used to
# isolate HTTP/2 contention from per-query upload bandwidth).
./target/release/pir-test bench-server \
    --url https://pir.valargroup.org \
    --nullifiers ./nullifiers.bin \
    --iterations 30 --warmup 3 \
    --batch-size 5 --mode single-tls \
    --seed 42 \
    --label primary-k5-single-tls \
    --json-out docs/baselines/primary-k5-single-tls.json
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
    - `single-tls` (added in §0.5.3) — K awaits over one HTTP/1.1 TCP/TLS
      connection (`http1_only()`, `pool_max_idle_per_host(1)`).
      Disambiguates HTTP/2 contention vs per-query upload bandwidth.
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

## Phase 0.5 — Pre-implementation experiments

Five read-only / harness-side experiments run on top of the Phase 0
baseline to convert Phase 1's success criteria from *inferred* to
*measured* and to lock in regression spec for the upcoming batched
route. Outcomes summarised in this section drive the gates in the next
section.

### 0.5.1 — `pack_pub_params` vs. `q.0` byte split

The harness now records `upload_pp_bytes` (`query.1` —
`pack_pub_params`) and `upload_q_bytes` (`query.0` — the SimplePIR
query vector / `pqr`) per tier per query. They are deterministic given
`(num_items, item_size_bits)` so the K=1 single run on primary is
sufficient (see [`baselines/primary-k1-single.json`](baselines/primary-k1-single.json)).

| Tier | `pp` (per query) | `q` (per query) | Upload | `pp` share |
|---|---:|---:|---:|---:|
| Tier 1 | **528.0 KB** |  16.0 KB | 544.0 KB | **97.0 %** |
| Tier 2 | **528.0 KB** | 256.0 KB | 784.0 KB | **67.3 %** |

`pp` is **identical between tier 1 and tier 2** because it depends only
on the YPIR `client_seed` + LWE polynomial parameters
(`poly_len = 2048`), not on database geometry. This is the bandwidth
lever Phase 1 batching exploits: ship `pp` once per tier-batch and
include K small `q` vectors.

### 0.5.2 — YPIR API path verdict (Path C-additive)

Read-only audit of [`ypir/src/client.rs`](../../ypir/src/client.rs)
and the underlying `valar-spiral-rs` 0.5.1 client led to picking
**Path C-additive: a separate batch path inside `valar-ypir`,
single-query path left byte-identical**.

Why neither A nor B alone works:

- `YClient::generate_full_query_simplepir` (originally lines 510–556)
  **rebuilds `pack_pub_params` on every call** by running
  `raw_generate_expansion_params`.
- `raw_generate_expansion_params` seeds its secret `ChaCha20Rng` with
  `from_entropy()`, so even a fixed `client_seed` yields
  **byte-different `pp` blobs** across successive calls.
- `YClient::new` is `pub(crate)`, and `YClient::from_seed` /
  `generate_full_query_simplepir` are private — `pir-client` cannot
  drive Path B from outside the `valar-ypir` crate even if the
  recompute-and-pray approach were acceptable.

#### What landed

A new batch path was added alongside the existing single-query path.
The single-query path's public API and wire-byte behavior are
unchanged. The implementation lives in
[`ypir/src/client.rs`](../../ypir/src/client.rs):

- `pub type YPIRSimpleBatchQuery = (Vec<AlignedMemory64>, AlignedMemory64);`
  — K SimplePIR `q.0` query vectors plus a single shared
  `pack_pub_params`.
- `pub fn YPIRClient::generate_query_simplepir_batch(&self,
  target_rows: &[usize]) -> (YPIRSimpleBatchQuery, Seed)` — generates
  K queries under one fresh `client_seed` (= one `s`), one shared
  `pp`, and K independent per-row `q.0` (each drawing fresh `OsRng`
  entropy for its LWE noise vector `e_k`).
- `pub fn YPIRClient::decode_response_simplepir_batch(&self,
  client_seed: Seed, responses: &[&[u8]]) -> Vec<Vec<u8>>` and the
  raw `_batch_raw` variant — decode K independent SimplePIR responses
  under the same shared `s`. Each chunk decodes independently of the
  others (test:
  `client::batch_path_tests::batch_decode_chunk_independence`).

The single-query path was touched in exactly one place: the inline
`pack_pub_params` construction was hoisted into a private
`YClient::build_pack_pub_params` helper so both paths can share it
without duplicating ~30 lines. The hoist is byte-faithful — given the
same `client_seed` and the same secret RNG seed, the helper produces
identical `pp` bytes (test:
`client::batch_path_tests::batch_query_pp_matches_under_fixed_secret_rng`).

#### Phase 1 invariants pinned in tests

The new `mod batch_path_tests` in
[`ypir/src/client.rs`](../../ypir/src/client.rs) pins:

| Invariant | Test |
|---|---|
| Batch returns K queries + 1 `pp` of correct shape | `batch_query_returns_k_queries_and_one_pp` |
| Hoisted `build_pack_pub_params` is byte-faithful given fixed RNGs | `batch_query_pp_matches_under_fixed_secret_rng` |
| K independent SimplePIR responses decode to the K expected plaintexts under one `client_seed` | `batch_query_each_q_decodes_independently` |
| Per-row `q.0` differ even for identical target rows (independent `e_k` under shared `s`) | `batch_query_distinct_secret_rngs_produce_distinct_q` |
| Per-chunk decoding doesn't couple slots — batch decode of N chunks equals N standalone decodes | `batch_decode_chunk_independence` |

Plus 39 pre-existing `client::sp_decode_pipeline_tests::*` and
`client::malformed_response_tests::*` tests still pass, confirming the
hoist did not regress the legacy single-query path.

Risk register update: the cross-repo patch to `valar-ypir` is
**additive**, not a refactor. No legacy callers (including the
existing `pir-client::ypir_query` flow) need to change. Downstream
consumers — `pir-types::serialize_ypir_batch_query`, the
`/tier{1,2}/batch_query` server route, and `pir-client::client_batch_query`
— can be designed against this stable API surface in follow-up PRs.

### 0.5.3 — single-TLS bench (HTTP/2 contention vs upload bandwidth)

New `--mode single-tls` issues K queries one at a time over a single
HTTP/1.1 TCP/TLS connection (`http1_only()`,
`pool_max_idle_per_host(1)`). Comparison to the existing K=1 single
mode disambiguates whether today's K=5 parallel upload p50 is bounded
by HTTP/2 stream multiplexing contention (then single-tls per-query
upload should match K=1 single) or by raw per-query upload bandwidth
on the observer's uplink (then single-tls would still pay
`K × upload_bytes`, just spread across K serial RTTs).

Per-query upload p50 (n=30 for K=1 single, n=150 for K=5 single-tls):

| Endpoint | Mode    | T1 upload p50 | T2 upload p50 | T1 RTT p50 | T2 RTT p50 |
|---|---|---:|---:|---:|---:|
| primary | K=1 single        | 225 ms | 228 ms |  290 ms | 2950 ms |
| primary | **K=5 single-tls**| **225 ms** | **228 ms** | **287 ms** | **2700 ms** |
| primary | K=5 parallel      | 229 ms | 233 ms |  508 ms | 3220 ms |
| backup  | K=1 single        | 221 ms | 224 ms |  275 ms | 2760 ms |
| backup  | **K=5 single-tls**| **224 ms** | **224 ms** | **278 ms** | **2830 ms** |
| backup  | K=5 parallel      | 236 ms | 754 ms |  341 ms | 3140 ms |

Wall-clock (per K=5 delegation) p50:

| Endpoint | K=1 single (×5) | K=5 single-tls | **K=5 parallel** |
|---|---:|---:|---:|
| primary | 16.5 s | **15.97 s** | **4.65 s** |
| backup  | 15.8 s | **16.08 s** | **4.31 s** |

Findings:

- **Per-query upload time is nearly identical across single-tls and
  K=1**, confirming that today's K=5 parallel **does not** bottleneck
  on per-query upload bandwidth on this observer link — most queries
  upload in ~225 ms regardless of how many are in flight.
- The one exception is **backup K=5 parallel tier 2 upload p50 = 754 ms**
  (3.4× single-query). That is HTTP/2 stream multiplexing contention
  on the backup uplink path, exactly the case where 5 simultaneous
  784 KB pub_params blobs share a single TCP window.
- K=5 parallel wall-clock is dramatically lower than K=5 single-tls
  (4.65 s vs 15.97 s on primary, 4.31 s vs 16.08 s on backup). The
  delta is not contention; it's **server-side parallelism** —
  parallel mode lets 5 concurrent requests use the server's Rayon
  pool concurrently, while single-tls forces strict serialisation on
  the server.

This reframes the Phase 1 wall-clock projection: **Phase 1 alone
(serial K matvecs in one request handler) will not match today's K=5
parallel wall-clock unless the Phase 1 server route does its K matvecs
under Rayon parallelism inside one request.** The bandwidth wins are
real and large; the wall-clock wins depend on how the batched route
schedules work server-side. Phase 2's batched matmul recovers the rest
by streaming the DB once per K queries.

### 0.5.4 — Post-decode side-channel audit

Inventory of all observables in the post-decode path of
[`fetch_proof_inner`](../pir/client/src/lib.rs) and
[`process_tier2_and_build`](../pir/client/src/lib.rs), plus downstream
consumers in
[`zcash_voting/src/storage/operations.rs`](../../zcash_voting/zcash_voting/src/storage/operations.rs)
and the Halo2 prover at
[`zcash_voting/src/zkp1.rs`](../../zcash_voting/zcash_voting/src/zkp1.rs).

| Classification | Count | Examples |
|---|---:|---|
| **safe** (plaintext-derived, `s`-independent under correct decryption) | 12 | `process_tier1` / `process_tier2_and_build` branches; `tier1_outcome` accept/reject; `t2_bounds_err` clamp |
| **safe-because-public** (witnesses the verifier must re-derive anyway) | 4 | `nf_bounds`, `path`, `leaf_pos` in `ImtProofData`; circuit packing |
| **flag-for-Phase-1** (`s`-correlated timing) | 4 | `TierTiming.gen_ms`, `TierTiming.decode_ms`, `print_timing_table` log emissions, `NoteTiming.total_ms` |

The four flagged items are all **timing channels** that include
secret-dependent crypto work (`generate_query_simplepir`,
`decode_response_simplepir`). They are exposed to callers via the
public `fetch_proof_with_timing` API and to logs at `Debug` /
`Trace` level. They were already soft side-channels under per-query
`s`; under Phase 1's shared-`s`-per-batch they leak about a single key
material instead of K independent secrets — a strict downgrade of
adversary advantage relative to per-query `s` only if the channel is
exploited within one batch lifetime. Action items for Phase 1:

- Treat `gen_ms` / `decode_ms` as batch-level observables and gate
  the `print_timing_table` Trace logging behind a non-production
  feature flag (or strip those columns under shared-`s` mode).
- Document `fetch_proof_with_timing` as diagnostic-only when the
  client uses the batched route.

**Important correction to plan §0.5.4**: the
`decryption_outcome_independent_of_secret_key` invariant tests cited
in the plan at `pir-client/src/lib.rs:964/1055/1080/1104` actually
live in `ypir/src/client.rs` (`decode_simplepir_random_response_same_outcome_different_keys`,
~line 1200). The `pir-client` side has only the structural
`tier2_query_sent_despite_tier1_decode_failure` test (~line 970).
Phase 1 must keep both sets green.

The audit also surfaced an **inter-tier client timing channel that
predates this work**: `process_tier1` runs between the tier 1 and
tier 2 HTTP requests, and its branch-data-dependent steps (binary
search depth, Poseidon walk) run for `s`-independent but
nullifier-dependent durations. A server that times the tier-1 →
tier-2 gap can learn something about the queried index. This is a
client privacy concern, not an `s`-oracle, and is unchanged by
Phase 1 batching (in Phase 1 the gap is amortised across K queries,
which strictly weakens the channel).

### 0.5.5 — Batched error-oracle test

New test
[`batched_tier2_queries_all_sent_despite_tier1_decode_failure`](../pir/client/src/lib.rs)
asserts the **batch-level** analogue of the existing
`tier2_query_sent_despite_tier1_decode_failure`: when `fetch_proofs(K=5)`
encounters corrupted tier-1 responses for **every** note, the wiremock
server must record **K tier 1 POSTs and K tier 2 POSTs**.

Today (per-query fresh `s`, parallel `try_join_all`) it passes by
transitivity of the per-note mitigation — verified on
`bench/pir-batch-phase0`. Phase 1's `/tier{1,2}/batch_query` route
must keep this green even though all K queries share one `client_seed`;
this is an explicit gate before merge.

While adding the test, we discovered that the existing
`tier2_query_sent_despite_tier1_decode_failure` was using
`num_items: TIER1_ROWS` (= 512), which is below the YPIR
`poly_len = 2048` floor and triggered an arithmetic underflow in
`valar-ypir` `params.rs:130`. Both tests now use `TIER1_YPIR_ROWS`
(= 2048), matching what
[`pir_server::tier1_scenario()`](../pir/server/src/lib.rs) emits in
production.

### 0.5.6 — Refined Phase 1 / Phase 2 projections

With the byte split and contention measurements above, the Phase 1
projected upload table tightens from inferred to measured:

| Tier | `pp` once | K × `q` | Phase 1 batch upload | K=5 today | Reduction |
|---|---:|---:|---:|---:|---:|
| Tier 1 | 528.0 KB | 5 × 16.0 KB =  80 KB | **608 KB**  | 2720 KB | **−77.6 %** |
| Tier 2 | 528.0 KB | 5 × 256.0 KB = 1280 KB | **1808 KB** | 3920 KB | **−53.9 %** |
| **Total** | — | — | **2416 KB** | **6640 KB** | **−63.6 %** |

Total bytes (upload + download) per delegation: today **8.44 MB**,
Phase 1 projected **4.22 MB** (downloads unchanged at 1.80 MB).

**Phase 1 wall-clock projection (revised, single-tls-aware)**:

- *If* the Phase 1 server route dispatches its K matvecs under
  Rayon parallelism within one request handler: wall-clock matches
  today's K=5 parallel **plus** removes the backup-uplink upload
  contention spike (754 ms → ~225 ms tier-2 upload p50 on backup).
  Expected p50 wall delta: **−400 to −800 ms on backup, −0 to
  −200 ms on primary.** Bandwidth still drops by 4.2 MB.
- *If* the Phase 1 server route does its K matvecs **serially**:
  per-query tier-2 server compute returns to single-query (~919 ms
  primary, ~833 ms backup), but K matvecs run in series, so the
  tier-2 server time per K=5 batch is ~4.6 s — a **wall-clock
  regression** vs. today's K=5 parallel (~3.2 s tier-2 RTT). This
  case wins on bandwidth only; it would gate Phase 2 to ship
  immediately after to recover wall.

Either way, the **upload bandwidth gate** is unconditionally winnable
and is the metric on which Phase 1 is principally judged.

## Phase 1 / Phase 2 success criteria

These are the gates that block merging the SDK swap to the batched path
on a tier server. All comparisons are against the **K=5 parallel** rows
above for the same endpoint, measured with the same harness from a
fixed observer host (CI runner, region tagged in JSON). All numbers are
**p50 unless a stronger percentile is named**.

### Phase 1 (HTTP/wire batching, K matvecs server-side)

All gates derive from §0.5.x measured numbers, not estimates.

- **Tier 1 upload bytes per K=5 batch ≤ 700 KB**
  (= `pp` 528 KB + 5 × `q` 16 KB + small framing = **608 KB
  measured** projection from §0.5.1; gate has 92 KB headroom for
  serialization framing). Today: 2720 KB.
- **Tier 2 upload bytes per K=5 batch ≤ 1.85 MB**
  (= `pp` 528 KB + 5 × `q` 256 KB + framing = **1808 KB measured**
  projection from §0.5.1). Today: 3920 KB.
- **Combined upload reduction ≥ 60 %** at K=5
  (target: ~63.6 % from §0.5.6 projection).
- **Wall-clock at p50** must satisfy **at least one** of:
  - **Primary** p50 ≤ today's K=5 parallel p50 (4.65 s) **and**
    **backup** p50 ≤ today's K=5 parallel p50 − 300 ms (4.31 s →
    ≤ 4.0 s) — captures the backup-uplink contention recovery
    measured in §0.5.3 (754 ms → 225 ms tier-2 upload on backup).
  - p50 wall regression ≤ 500 ms on either endpoint **and**
    Phase 2 ships in the same release window — accepts the
    serial-K-matvec case identified in §0.5.6 with explicit
    Phase 2 follow-up commitment.
- **p99 wall** must not regress by more than 1.0 s on either endpoint.
- **Download bytes**: unchanged (5 × 24 KB tier 1 + 5 × 336 KB tier 2
  = 1.80 MB per K=5 batch — per-query response shape preserved;
  Phase 1 just bundles the K responses).
- **Server compute per K matvec**: tier-2 ≤ today's K=5 parallel
  tier-2 server compute p50 (1220 ms primary, 1530 ms backup) —
  Phase 1 must not introduce new per-query server overhead.
- **Error rate**: 0 across `iterations × batch_size` for both
  endpoints in the bench harness.
- **Security regression gate**:
  `tier2_query_sent_despite_tier1_decode_failure` (per-note,
  pre-existing) **and**
  `batched_tier2_queries_all_sent_despite_tier1_decode_failure`
  (batch, added in §0.5.5) **and** the existing
  `decode_simplepir_random_response_same_outcome_different_keys`
  test in `valar-ypir` must all pass against the new
  `/tier{1,2}/batch_query` route under shared-`s` batching. New
  test in this PR: `pir-client/src/lib.rs::tests::batched_tier2_queries_all_sent_despite_tier1_decode_failure`.
- **Capability/fallback**: SDK builds older than the rollout flip
  fall back to the legacy `/tier{1,2}/query` route — verified by
  integration test, not bench gate.

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

- All six original baselines + two `single-tls` baselines committed
  under [`baselines/`](baselines/) with 3-iteration warmup,
  30 / 30 / 15 / 30 measured iterations (single / parallel /
  sequential / single-tls) and matching `--seed 42`.
- `pir-test bench-server --help` documents the harness, including
  the `single-tls` mode added in §0.5.3.
- Wall-clock and per-tier server_compute summarised in the tables
  above with the same percentile shape used for the Phase 1 / 2 gates.

### Phase 0.5 (this doc)

- §0.5.1 instrumentation landed in
  [`pir/client/src/lib.rs`](../pir/client/src/lib.rs)
  (`TierTiming.{upload_q_bytes, upload_pp_bytes}`) and surfaces in
  the bench JSON. Re-ran `primary-k1-single` to capture the split.
- §0.5.2 audit committed inline above; Path C dependency on
  `valar-ypir` documented.
- §0.5.3 `--mode single-tls` shipped in
  [`pir/test/src/bench_server.rs`](../pir/test/src/bench_server.rs)
  via a new `connect_with_http` constructor on `PirClient`. Two new
  baselines committed.
- §0.5.4 audit committed inline above; four timing observables
  flagged for Phase 1 hygiene work.
- §0.5.5 batched regression test
  `batched_tier2_queries_all_sent_despite_tier1_decode_failure`
  added and green; pre-existing test fixed for `valar-ypir`'s
  `poly_len = 2048` floor.
- §0.5.6 success criteria for Phase 1 rewritten on top of measured
  data — see "Phase 1 / Phase 2 success criteria" above.
