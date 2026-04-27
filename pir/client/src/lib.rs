//! PIR client library for private Merkle path retrieval.
//!
//! Provides [`PirClient`] which connects to a `pir-server` instance and
//! retrieves circuit-ready `ImtProofData` without revealing the
//! queried nullifier to the server.

use std::time::Instant;

use anyhow::{Context, Result};
use ff::PrimeField as _;
use imt_tree::hasher::PoseidonHasher;
use imt_tree::tree::{precompute_empty_hashes, TREE_DEPTH};
use pasta_curves::Fp;
// Re-exported so downstream crates (e.g. zcash_voting) can reference the type
// returned by PirClientBlocking::fetch_proof without a direct imt-tree dependency.
pub use imt_tree::ImtProofData;

use pir_types::tier0::Tier0Data;
use pir_types::tier1::Tier1Row;
use pir_types::tier2::Tier2Row;
use pir_types::{
    parse_ypir_batch_response, serialize_ypir_batch_query, serialize_ypir_query, RootInfo,
    YpirScenario, MAX_BATCH_K, PIR_DEPTH, TIER0_LAYERS, TIER1_LAYERS, TIER1_LEAVES,
    TIER1_ROW_BYTES, TIER2_LEAVES, TIER2_ROW_BYTES,
};

use ypir::client::YPIRClient;

// ── Timing breakdown ─────────────────────────────────────────────────────────

/// Per-tier timing breakdown for a single YPIR query, measuring each stage
/// of the client-server round trip.
pub struct TierTiming {
    /// Client-side YPIR query generation time.
    pub gen_ms: f64,
    /// Size of the uploaded query payload.
    pub upload_bytes: usize,
    /// Bytes of the uploaded query attributable to the SimplePIR query
    /// vector itself (`q.0` / `pqr` — the first arg to
    /// [`pir_types::serialize_ypir_query`]). The batched protocol keeps
    /// this per-query, so the projected upload is `pp + K * q`.
    pub upload_q_bytes: usize,
    /// Bytes of the uploaded query attributable to `pack_pub_params`
    /// (the second arg to [`pir_types::serialize_ypir_query`]). Identical
    /// across queries that share a YPIR `client_seed`, which is the
    /// bandwidth lever the batched protocol exploits.
    pub upload_pp_bytes: usize,
    /// Size of the downloaded encrypted response.
    pub download_bytes: usize,
    /// Wall-clock round-trip time (upload + server compute + download).
    pub rtt_ms: f64,
    /// Client-side YPIR response decryption time.
    pub decode_ms: f64,
    /// Server-assigned request ID (from response header).
    pub server_req_id: Option<u64>,
    /// Server-reported total processing time.
    pub server_total_ms: Option<f64>,
    /// Server-reported query validation time.
    pub server_validate_ms: Option<f64>,
    /// Server-reported decode+copy time.
    pub server_decode_copy_ms: Option<f64>,
    /// Server-reported YPIR online computation time.
    pub server_compute_ms: Option<f64>,
    /// Estimated network + queue latency (RTT minus server time).
    pub net_queue_ms: Option<f64>,
    /// Estimated upload-to-server latency.
    pub upload_to_server_ms: Option<f64>,
    /// Estimated download-from-server latency.
    pub download_from_server_ms: f64,
}

/// Per-note timing breakdown covering both tier 1 and tier 2 YPIR queries.
pub struct NoteTiming {
    pub tier1: TierTiming,
    pub tier2: TierTiming,
    /// Total wall-clock time for this note's proof retrieval.
    pub total_ms: f64,
}

/// Aggregate timing for a single batched `POST /tier{1,2}/batch_query`.
///
/// Fields named `*_total` are batch totals; per-query fields capture the
/// (deterministic) per-query payload sizes. The
/// [`BatchTierTiming::per_note`] projection maps these to a per-note
/// [`TierTiming`] using the convention: shared costs (upload / server
/// stages / net-queue) are divided by `k` so summing across notes
/// recovers the batch totals; per-query metrics (download bytes, decode
/// time, RTT, download_from_server) are kept per-note.
struct BatchTierTiming {
    k: usize,
    /// Wall-clock time to generate K queries (one
    /// `generate_query_simplepir_batch` call covers all K).
    gen_ms: f64,
    /// Total wire upload bytes (header + K * q + pp).
    upload_bytes_total: usize,
    /// Bytes of `pack_pub_params` shared across the batch.
    upload_pp_bytes_total: usize,
    /// Bytes of one `q.0` (identical across the batch by scenario).
    upload_q_bytes_per_query: usize,
    /// Bytes of one decoded response chunk (identical across the batch).
    download_bytes_per_query: usize,
    /// Wall-clock RTT for the batched HTTP request (upload + server +
    /// download).
    rtt_ms: f64,
    /// Per-note decryption wall-clock time, captured outside the
    /// shared-by-K accounting so each note retains its own decode cost.
    per_note_decode_ms: Vec<f64>,
    server_req_id: Option<u64>,
    server_total_ms: Option<f64>,
    server_validate_ms: Option<f64>,
    server_decode_copy_ms: Option<f64>,
    server_compute_ms: Option<f64>,
    /// `x-pir-batch-k` header echoed back by the server. Used in tests
    /// to assert the round trip matched the requested batch size; the
    /// field is otherwise informational and only consumed at debug time.
    #[allow(dead_code)]
    server_batch_k: Option<u64>,
    upload_to_server_ms: Option<f64>,
    /// Wall-clock time for response body download (after server compute).
    /// Per-batch quantity, identical for every note in the batch.
    download_from_server_ms: f64,
}

impl BatchTierTiming {
    /// Project to a per-note [`TierTiming`].
    ///
    /// Convention:
    /// - **Per-query (unchanged)**: `upload_q_bytes`, `download_bytes`,
    ///   `decode_ms`, `rtt_ms`, `download_from_server_ms`.
    /// - **Shared / divided by K**: `gen_ms`, `upload_bytes`,
    ///   `upload_pp_bytes`, `server_*_ms`, `upload_to_server_ms`,
    ///   `net_queue_ms`.
    ///
    /// Summing per-note shared metrics across the batch recovers the
    /// batch-level total; percentile metrics on per-note RTT match the
    /// batch RTT (every note in the batch saw the same wall-clock RTT).
    fn per_note(&self, i: usize) -> TierTiming {
        let k = self.k.max(1) as f64;
        TierTiming {
            gen_ms: self.gen_ms / k,
            upload_bytes: self.upload_bytes_total / self.k.max(1),
            upload_q_bytes: self.upload_q_bytes_per_query,
            upload_pp_bytes: self.upload_pp_bytes_total / self.k.max(1),
            download_bytes: self.download_bytes_per_query,
            rtt_ms: self.rtt_ms,
            decode_ms: self.per_note_decode_ms[i],
            server_req_id: self.server_req_id,
            server_total_ms: self.server_total_ms.map(|m| m / k),
            server_validate_ms: self.server_validate_ms.map(|m| m / k),
            server_decode_copy_ms: self.server_decode_copy_ms.map(|m| m / k),
            server_compute_ms: self.server_compute_ms.map(|m| m / k),
            net_queue_ms: self
                .server_total_ms
                .map(|server_ms| ((self.rtt_ms - server_ms).max(0.0)) / k),
            upload_to_server_ms: self.upload_to_server_ms.map(|m| m / k),
            download_from_server_ms: self.download_from_server_ms,
        }
    }
}

// ── HTTP-based PIR client ────────────────────────────────────────────────────

/// Client-side options for [`PirClient`].
#[derive(Clone, Debug)]
pub struct PirClientConfig {
    /// When `true`, never use `POST /tier{1,2}/batch_query` for multi-fetch;
    /// use the same per-note fan-out as when the server omits batch support.
    pub disable_batch: bool,
}

impl Default for PirClientConfig {
    fn default() -> Self {
        Self { disable_batch: false }
    }
}

/// PIR client that connects to a `pir-server` instance over HTTP.
///
/// Downloads Tier 0 data and YPIR parameters during `connect()`, then
/// performs private queries via `fetch_proof()`.
pub struct PirClient {
    server_url: String,
    http: reqwest::Client,
    tier0: Tier0Data,
    tier1_scenario: YpirScenario,
    tier2_scenario: YpirScenario,
    num_ranges: usize,
    empty_hashes: [Fp; TREE_DEPTH],
    root29: Fp,
    /// Capability flag mirrored from `RootInfo.supports_batch_query`.
    /// When `true` we use `POST /tier{1,2}/batch_query`; when `false` we
    /// fall back to the per-note single-query endpoints (`/tier{1,2}/query`).
    supports_batch_query: bool,
    config: PirClientConfig,
}

/// Return the number of populated leaves in a Tier 2 row, clamped to
/// [`TIER2_LEAVES`]. The final row may be only partially filled when
/// `num_ranges` is not a multiple of the row size.
#[inline]
fn valid_leaves_for_row(num_ranges: usize, row_idx: usize) -> usize {
    let row_start = row_idx.saturating_mul(TIER2_LEAVES);
    num_ranges.saturating_sub(row_start).min(TIER2_LEAVES)
}

// ── Shared tier-processing helpers ───────────────────────────────────────────

/// Copy `siblings` into `path` starting at `offset`.
#[inline]
fn fill_path(path: &mut [Fp; TREE_DEPTH], offset: usize, siblings: &[Fp]) {
    path[offset..offset + siblings.len()].copy_from_slice(siblings);
}

/// Locate the nullifier's subtree in Tier 0, fill its siblings into `path`,
/// and return the subtree index `s1`.
fn process_tier0(tier0: &Tier0Data, nullifier: Fp, path: &mut [Fp; TREE_DEPTH]) -> Result<usize> {
    let s1 = tier0
        .find_subtree(nullifier)
        .context("nullifier not found in any Tier 0 subtree")?;
    fill_path(path, PIR_DEPTH - TIER0_LAYERS, &tier0.extract_siblings(s1));
    Ok(s1)
}

/// Parse a Tier 1 row, locate the nullifier's sub-subtree, fill its siblings
/// into `path`, and return the sub-subtree index `s2`.
fn process_tier1(tier1_row: &[u8], nullifier: Fp, path: &mut [Fp; TREE_DEPTH]) -> Result<usize> {
    let hasher = PoseidonHasher::new();
    let tier1 = Tier1Row::from_bytes(tier1_row)?;
    let s2 = tier1
        .find_sub_subtree(nullifier)
        .context("nullifier not found in any Tier 1 sub-subtree")?;
    fill_path(
        path,
        PIR_DEPTH - TIER0_LAYERS - TIER1_LAYERS,
        &tier1.extract_siblings(s2, &hasher),
    );
    Ok(s2)
}

/// Parse a Tier 2 row, locate the nullifier's leaf, fill tier-2 and padding
/// siblings into `path`, and assemble the final [`ImtProofData`].
fn process_tier2_and_build(
    tier2_row: &[u8],
    t2_row_idx: usize,
    num_ranges: usize,
    nullifier: Fp,
    path: &mut [Fp; TREE_DEPTH],
    empty_hashes: &[Fp; TREE_DEPTH],
    root29: Fp,
) -> Result<ImtProofData> {
    let hasher = PoseidonHasher::new();
    let tier2 = Tier2Row::from_bytes(tier2_row)?;
    let valid_leaves = valid_leaves_for_row(num_ranges, t2_row_idx);

    let leaf_local_idx = tier2
        .find_leaf(nullifier, valid_leaves)
        .context("nullifier not found in Tier 2 leaf scan")?;

    fill_path(
        path,
        0,
        &tier2.extract_siblings(leaf_local_idx, valid_leaves, &hasher),
    );
    // Pad from PIR depth (25) to circuit depth (29) with empty hashes.
    fill_path(path, PIR_DEPTH, &empty_hashes[PIR_DEPTH..TREE_DEPTH]);

    let global_leaf_idx = t2_row_idx * TIER2_LEAVES + leaf_local_idx;
    let (nf_lo, nf_mid, nf_hi) = tier2.leaf_record(leaf_local_idx);

    Ok(ImtProofData {
        root: root29,
        nf_bounds: [nf_lo, nf_mid, nf_hi],
        leaf_pos: global_leaf_idx as u32,
        path: *path,
    })
}

impl PirClient {
    /// Connect to a PIR server, downloading Tier 0 data and YPIR parameters.
    pub async fn connect(server_url: &str) -> Result<Self> {
        Self::connect_with_http_and_config(
            server_url,
            reqwest::Client::new(),
            PirClientConfig::default(),
        )
        .await
    }

    /// Like [`connect`](Self::connect) but with a caller-provided
    /// [`reqwest::Client`]. Used by `pir-test bench-server --mode single-tls`
    /// to force HTTP/1.1 with a single connection (no HTTP/2 stream
    /// multiplexing) so we can isolate per-query upload bandwidth from
    /// HTTP/2 contention when projecting batching wins.
    pub async fn connect_with_http(server_url: &str, http: reqwest::Client) -> Result<Self> {
        Self::connect_with_http_and_config(server_url, http, PirClientConfig::default()).await
    }

    /// Like [`connect_with_http`](Self::connect_with_http) but with explicit
    /// [`PirClientConfig`] (e.g. [`PirClientConfig::disable_batch`] to force
    /// per-note fan-out even when the server advertises batch support).
    pub async fn connect_with_http_and_config(
        server_url: &str,
        http: reqwest::Client,
        config: PirClientConfig,
    ) -> Result<Self> {
        let base = server_url.trim_end_matches('/');

        // Download Tier 0 data, YPIR params, and root concurrently
        let t0 = Instant::now();
        let (tier0_resp, tier1_resp, tier2_resp, root_resp) = tokio::try_join!(
            http.get(format!("{base}/tier0")).send(),
            http.get(format!("{base}/params/tier1")).send(),
            http.get(format!("{base}/params/tier2")).send(),
            http.get(format!("{base}/root")).send(),
        )
        .map_err(|e| anyhow::anyhow!("connect fetch failed: {e}"))?;

        let tier0_bytes = tier0_resp.error_for_status()?.bytes().await?;
        log::debug!(
            "Downloaded Tier 0: {} bytes in {:.1}s",
            tier0_bytes.len(),
            t0.elapsed().as_secs_f64()
        );
        let tier0 = Tier0Data::from_bytes(tier0_bytes.to_vec())?;

        let tier1_scenario: YpirScenario = tier1_resp
            .error_for_status()
            .context("GET /params/tier1 failed")?
            .json()
            .await?;
        let tier2_scenario: YpirScenario = tier2_resp
            .error_for_status()
            .context("GET /params/tier2 failed")?
            .json()
            .await?;

        let root_info: RootInfo = root_resp
            .error_for_status()
            .context("GET /root failed")?
            .json()
            .await?;
        anyhow::ensure!(
            root_info.pir_depth == PIR_DEPTH,
            "server pir_depth {} != expected {}",
            root_info.pir_depth,
            PIR_DEPTH
        );
        let root29_bytes = hex::decode(&root_info.root29)?;
        anyhow::ensure!(
            root29_bytes.len() == 32,
            "root29 hex decoded to {} bytes, expected 32",
            root29_bytes.len()
        );
        let mut root29_arr = [0u8; 32];
        root29_arr.copy_from_slice(&root29_bytes);
        let root29 = Option::from(Fp::from_repr(root29_arr))
            .ok_or_else(|| anyhow::anyhow!("invalid root29 field element"))?;

        let empty_hashes = precompute_empty_hashes();

        Ok(Self {
            server_url: base.to_string(),
            http,
            tier0,
            tier1_scenario,
            tier2_scenario,
            num_ranges: root_info.num_ranges,
            empty_hashes,
            root29,
            supports_batch_query: root_info.supports_batch_query,
            config,
        })
    }

    /// Returns `true` when the server advertised batch query support
    /// in `GET /root`. Exposed so tests / the bench harness can verify the
    /// capability handshake.
    ///
    /// This reflects the server only; multi-fetch may still use per-note
    /// fan-out when [`PirClientConfig::disable_batch`] is set.
    pub fn supports_batch_query(&self) -> bool {
        self.supports_batch_query
    }

    /// Perform private Merkle path retrieval for a nullifier.
    ///
    /// Returns circuit-ready `ImtProofData` with a 29-element path
    /// (25 PIR siblings + 4 empty-hash padding).
    pub async fn fetch_proof(&self, nullifier: Fp) -> Result<ImtProofData> {
        let (proof, _timing) = self.fetch_proof_inner(nullifier).await?;
        Ok(proof)
    }

    /// Like [`fetch_proof`](Self::fetch_proof) but also returns the full
    /// client+server timing breakdown for load-testing / observability.
    pub async fn fetch_proof_with_timing(
        &self,
        nullifier: Fp,
    ) -> Result<(ImtProofData, NoteTiming)> {
        self.fetch_proof_inner(nullifier).await
    }

    /// Perform private Merkle path retrieval for multiple nullifiers.
    ///
    /// When the server advertises batch support
    /// ([`PirClient::supports_batch_query`]) and
    /// [`PirClientConfig::disable_batch`] is `false`, we ship one tier-1 batch
    /// followed by one tier-2 batch (each in a single HTTP POST). Otherwise
    /// we use a parallel `try_join_all` of per-note `fetch_proof_inner` calls
    /// (fan-out to `/tier{1,2}/query`).
    pub async fn fetch_proofs(&self, nullifiers: &[Fp]) -> Result<Vec<ImtProofData>> {
        let pairs = self.fetch_proofs_with_timing(nullifiers).await?;
        Ok(pairs.into_iter().map(|(p, _)| p).collect())
    }

    /// Like [`fetch_proofs`](Self::fetch_proofs) but also returns the
    /// per-note client+server timing breakdown.
    ///
    /// Batching vs fan-out follows [`fetch_proofs`](Self::fetch_proofs)
    /// (server `supports_batch_query` and [`PirClientConfig::disable_batch`]).
    pub async fn fetch_proofs_with_timing(
        &self,
        nullifiers: &[Fp],
    ) -> Result<Vec<(ImtProofData, NoteTiming)>> {
        if nullifiers.is_empty() {
            return Ok(Vec::new());
        }
        let use_batch = self.supports_batch_query && !self.config.disable_batch;
        if !use_batch {
            return self.fetch_proofs_fanout(nullifiers).await;
        }
        // K is bounded by the wire format DoS guard.
        if nullifiers.len() > MAX_BATCH_K {
            anyhow::bail!(
                "fetch_proofs called with {} nullifiers (MAX_BATCH_K = {})",
                nullifiers.len(),
                MAX_BATCH_K
            );
        }
        self.fetch_proofs_batched(nullifiers).await
    }

    /// Fan-out: one HTTP POST per tier per note, all running concurrently.
    /// Used when the server does not advertise batch support or when
    /// [`PirClientConfig::disable_batch`] is set.
    async fn fetch_proofs_fanout(
        &self,
        nullifiers: &[Fp],
    ) -> Result<Vec<(ImtProofData, NoteTiming)>> {
        log::debug!(
            "[PIR] Starting parallel fetch for {} notes (fan-out)...",
            nullifiers.len()
        );
        let wall_start = Instant::now();

        let futures: Vec<_> = nullifiers
            .iter()
            .enumerate()
            .map(|(i, &nf)| async move {
                let (proof, timing) = self.fetch_proof_inner(nf).await?;
                Ok::<_, anyhow::Error>((i, proof, timing))
            })
            .collect();

        let results_with_timing = futures::future::try_join_all(futures).await?;
        let wall_ms = wall_start.elapsed().as_secs_f64() * 1000.0;
        print_timing_table_fanout(&results_with_timing, wall_ms);

        Ok(results_with_timing
            .into_iter()
            .map(|(_, proof, t)| (proof, t))
            .collect())
    }

    /// Batched fetch: one POST per tier (`/tier{1,2}/batch_query`).
    ///
    /// **Error-oracle invariant**: regardless of how the tier-1 batch
    /// resolves per slot, the tier-2 batch is *always* sent. Per-slot
    /// tier-1 failures populate the corresponding tier-2 query with a
    /// dummy index 0 so the server cannot use the presence/absence of a
    /// tier-2 query as an oracle bit. This is exactly the invariant
    /// enforced per-note by [`fetch_proof_inner`](Self::fetch_proof_inner)
    /// — the batched path lifts it to the batch granularity (the
    /// failing-spec `batched_oracle` test in this crate verifies it
    /// end-to-end).
    async fn fetch_proofs_batched(
        &self,
        nullifiers: &[Fp],
    ) -> Result<Vec<(ImtProofData, NoteTiming)>> {
        let k = nullifiers.len();
        log::debug!("[PIR] Starting batched fetch for {k} notes...");
        let wall_start = Instant::now();

        // ── Tier 0 (plaintext) ───────────────────────────────────────
        let mut paths = vec![[Fp::default(); TREE_DEPTH]; k];
        let mut s1s = vec![0usize; k];
        let mut tier0_errs: Vec<Option<anyhow::Error>> = (0..k).map(|_| None).collect();
        for i in 0..k {
            match process_tier0(&self.tier0, nullifiers[i], &mut paths[i]) {
                Ok(s1) => s1s[i] = s1,
                Err(e) => tier0_errs[i] = Some(e),
            }
        }

        // Substitute dummy index 0 for tier-0 failures so the tier-1
        // batch always carries K well-formed queries.
        let t1_indices: Vec<usize> = (0..k)
            .map(|i| if tier0_errs[i].is_some() { 0 } else { s1s[i] })
            .collect();

        // ── Tier 1 batch ────────────────────────────────────────────
        let (t1_results, t1_timing) = self
            .ypir_batch_query(&self.tier1_scenario, "tier1", &t1_indices, TIER1_ROW_BYTES)
            .await?;

        // Per-note tier-1 outcome: the global tier-2 row index on success,
        // or an Err carrying the failure reason. Any error here will
        // be surfaced *after* the tier-2 batch is sent.
        let mut tier1_outcomes: Vec<Result<usize>> = Vec::with_capacity(k);
        let mut t2_indices: Vec<usize> = Vec::with_capacity(k);
        for i in 0..k {
            if let Some(e) = tier0_errs[i].take() {
                tier1_outcomes.push(Err(e));
                t2_indices.push(0);
                continue;
            }
            match t1_results[i].as_ref() {
                Ok(row) => {
                    let mut_path = &mut paths[i];
                    let nf = nullifiers[i];
                    let s2_outcome =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            process_tier1(row, nf, mut_path)
                        }))
                        .unwrap_or_else(|payload| {
                            let msg = payload
                                .downcast_ref::<String>()
                                .map(|s| s.as_str())
                                .or_else(|| payload.downcast_ref::<&str>().copied())
                                .unwrap_or("unknown panic");
                            Err(anyhow::anyhow!("process_tier1 panicked: {msg}"))
                        });
                    match s2_outcome {
                        Ok(s2) => {
                            let global = s1s[i] * TIER1_LEAVES + s2;
                            if global >= self.tier2_scenario.num_items {
                                tier1_outcomes.push(Err(anyhow::anyhow!(
                                    "tier2 row_idx {global} >= num_items {}",
                                    self.tier2_scenario.num_items
                                )));
                                t2_indices.push(0);
                            } else {
                                tier1_outcomes.push(Ok(global));
                                t2_indices.push(global);
                            }
                        }
                        Err(e) => {
                            tier1_outcomes.push(Err(e));
                            t2_indices.push(0);
                        }
                    }
                }
                Err(e) => {
                    tier1_outcomes.push(Err(anyhow::anyhow!(
                        "tier1 batch slot {i}: {e}"
                    )));
                    t2_indices.push(0);
                }
            }
        }

        // ── Tier 2 batch (always sent) ──────────────────────────────
        let (t2_results, t2_timing) = self
            .ypir_batch_query(&self.tier2_scenario, "tier2", &t2_indices, TIER2_ROW_BYTES)
            .await?;

        // ── Per-note proof assembly ─────────────────────────────────
        let mut out = Vec::with_capacity(k);
        for i in 0..k {
            // Surface tier-1 errors first (after both batches sent).
            let s_global = match &tier1_outcomes[i] {
                Ok(v) => *v,
                Err(e) => return Err(anyhow::anyhow!("note {i}: {e:#}")),
            };
            let row = match t2_results[i].as_ref() {
                Ok(r) => r,
                Err(e) => return Err(anyhow::anyhow!("note {i} tier2: {e:#}")),
            };
            let nf = nullifiers[i];
            let proof = process_tier2_and_build(
                row,
                s_global,
                self.num_ranges,
                nf,
                &mut paths[i],
                &self.empty_hashes,
                self.root29,
            )?;
            let total_ms = wall_start.elapsed().as_secs_f64() * 1000.0;
            let note_timing = NoteTiming {
                tier1: t1_timing.per_note(i),
                tier2: t2_timing.per_note(i),
                total_ms,
            };
            out.push((proof, note_timing));
        }

        let wall_ms = wall_start.elapsed().as_secs_f64() * 1000.0;
        let view: Vec<(usize, &NoteTiming)> = out
            .iter()
            .enumerate()
            .map(|(i, (_, t))| (i, t))
            .collect();
        print_timing_table(&view, wall_ms);
        Ok(out)
    }

    /// Fetch proof and return timing breakdown.
    ///
    /// **Error-oracle mitigation**: the tier 2 query is always sent even when
    /// tier 1 fails. A malicious server could craft a tier 1 response whose
    /// decryption outcome depends on the client's secret key material (e.g. by
    /// triggering an assert in the LWE decode path). If the client aborted
    /// before sending the tier 2 query, the server could observe its absence
    /// and use the binary "crash / no-crash" signal as an oracle. By
    /// unconditionally sending a (possibly dummy) tier 2 query we ensure the
    /// server always sees both requests and gains no information from errors.
    async fn fetch_proof_inner(&self, nullifier: Fp) -> Result<(ImtProofData, NoteTiming)> {
        let note_start = Instant::now();
        let mut path = [Fp::default(); TREE_DEPTH];

        // Process tier 0 (plaintext, not server-controlled)
        let s1 = process_tier0(&self.tier0, nullifier, &mut path)?;

        // Process tier 1 (PIR) — capture the outcome without `?` so that a
        // tier 2 query is always sent regardless of tier 1 success.
        //
        // process_tier1 is wrapped in catch_unwind so that a panic (e.g. from
        // a debug_assert or an unexpected slice bounds violation) cannot
        // prevent the tier 2 query from being sent. Without this, a panic
        // here would unwind past the tier 2 dispatch and give the server an
        // observable one-query-vs-two oracle.
        let tier1_outcome = self
            .ypir_query(&self.tier1_scenario, "tier1", s1, TIER1_ROW_BYTES)
            .await
            .and_then(|(row, timing)| {
                let mut_path = &mut path;
                let s2 = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    process_tier1(&row, nullifier, mut_path)
                }))
                .unwrap_or_else(|payload| {
                    let msg = payload
                        .downcast_ref::<String>()
                        .map(|s| s.as_str())
                        .or_else(|| payload.downcast_ref::<&str>().copied())
                        .unwrap_or("unknown panic");
                    Err(anyhow::anyhow!("process_tier1 panicked: {}", msg))
                })?;
                Ok((s1 * TIER1_LEAVES + s2, timing))
            });

        // Real index on success, dummy index 0 on failure. PIR hides the
        // queried index from the server, so the dummy is indistinguishable.
        let t2_row_idx = tier1_outcome.as_ref().map(|(idx, _)| *idx).unwrap_or(0);

        // Validate the tier 2 index before passing it to ypir_query.
        // ypir_query has an ensure!(row_idx < num_items) that returns Err
        // *before* sending the HTTP request — if that fires, no tier 2
        // request reaches the server and we leak an oracle bit. A malicious
        // server can trigger this by setting tier2 num_items too small or
        // crafting tier 1 data that produces out-of-bounds indices. Clamp to
        // dummy index 0 so the query always goes out; propagate the error
        // only after both queries have been sent.
        let t2_bounds_err = if t2_row_idx >= self.tier2_scenario.num_items {
            Some(anyhow::anyhow!(
                "tier2 row_idx {} >= num_items {}",
                t2_row_idx,
                self.tier2_scenario.num_items
            ))
        } else {
            None
        };
        let t2_query_idx = if t2_bounds_err.is_some() {
            0
        } else {
            t2_row_idx
        };

        // Always send tier 2 to void error-based oracles.
        let tier2_result = self
            .ypir_query(&self.tier2_scenario, "tier2", t2_query_idx, TIER2_ROW_BYTES)
            .await;

        // Propagate errors only after both queries have been sent.
        let (t2_row_idx, tier1_timing) = tier1_outcome?;
        if let Some(e) = t2_bounds_err {
            return Err(e);
        }
        let (tier2_row, tier2_timing) = tier2_result?;

        let proof = process_tier2_and_build(
            &tier2_row,
            t2_row_idx,
            self.num_ranges,
            nullifier,
            &mut path,
            &self.empty_hashes,
            self.root29,
        )?;

        let total_ms = note_start.elapsed().as_secs_f64() * 1000.0;
        Ok((
            proof,
            NoteTiming {
                tier1: tier1_timing,
                tier2: tier2_timing,
                total_ms,
            },
        ))
    }

    /// Send a YPIR query for a tier row and return the decrypted row bytes.
    /// This function handles the key client PIR operations:
    /// 1. Generate keys
    /// 2. Query
    /// 3. Recover
    async fn ypir_query(
        &self,
        scenario: &YpirScenario,
        tier_name: &str,
        row_idx: usize,
        expected_row_bytes: usize,
    ) -> Result<(Vec<u8>, TierTiming)> {
        anyhow::ensure!(
            row_idx < scenario.num_items,
            "{} row_idx {} >= num_items {}",
            tier_name,
            row_idx,
            scenario.num_items
        );
        let t0 = Instant::now();
        let ypir_client = YPIRClient::from_db_sz(
            scenario.num_items as u64,
            scenario.item_size_bits as u64,
            true,
        );

        // Generate PIR query from a fresh secret created from OsRng seed.
        let (query, seed) = ypir_client.generate_query_simplepir(row_idx);
        let gen_ms = t0.elapsed().as_secs_f64() * 1000.0;

        // Serialize query. `query.0` is the SimplePIR query vector
        // (per-query); `query.1` is `pack_pub_params` (depends only on the
        // client's `client_seed`, so the batched protocol can ship it once
        // per tier-batch instead of once per query).
        let upload_q_bytes = query.0.as_slice().len() * std::mem::size_of::<u64>();
        let upload_pp_bytes = query.1.as_slice().len() * std::mem::size_of::<u64>();
        let payload = serialize_ypir_query(query.0.as_slice(), query.1.as_slice());
        let upload_bytes = payload.len();

        // Send the request
        let t1 = Instant::now();
        let url = format!("{}/{}/query", self.server_url, tier_name);
        let send_result = self.http.post(&url).body(payload).send().await;
        let send_ms = t1.elapsed().as_secs_f64() * 1000.0;
        let resp = match send_result {
            Ok(r) => r,
            Err(e) => {
                log::warn!("YPIR {} send error: {:?}", tier_name, e);
                return Err(e.into());
            }
        };
        let server_req_id = parse_header_u64(resp.headers(), "x-pir-req-id");
        let server_total_ms = parse_header_f64(resp.headers(), "x-pir-server-total-ms");
        let server_validate_ms = parse_header_f64(resp.headers(), "x-pir-server-validate-ms");
        let server_decode_copy_ms = parse_header_f64(resp.headers(), "x-pir-server-decode-copy-ms");
        let server_compute_ms = parse_header_f64(resp.headers(), "x-pir-server-compute-ms");
        let status = resp.status();
        let response_bytes = resp.bytes().await?;
        if !status.is_success() {
            anyhow::bail!(
                "{} query failed: HTTP {} body={}",
                tier_name,
                status,
                String::from_utf8_lossy(&response_bytes)
            );
        }
        let rtt_ms = t1.elapsed().as_secs_f64() * 1000.0;
        let download_from_server_ms = (rtt_ms - send_ms).max(0.0);
        let net_queue_ms = server_total_ms.map(|server_ms| (rtt_ms - server_ms).max(0.0));
        let upload_to_server_ms = server_total_ms.map(|server_ms| (send_ms - server_ms).max(0.0));

        // Decode the response. Wrap in catch_unwind so that assert panics
        // in the YPIR library (e.g. `val < lwe_q_prime` in the LWE decode
        // path) become recoverable errors rather than process aborts. This is
        // necessary for the error-oracle mitigation in fetch_proof_inner:
        // a panic here must not prevent the second query from being sent.
        let t2 = Instant::now();
        let decoded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            ypir_client.decode_response_simplepir(seed, &response_bytes)
        }))
        .map_err(|panic_payload| {
            let msg = panic_payload
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| panic_payload.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic");
            anyhow::anyhow!("{} response decryption panicked: {}", tier_name, msg)
        })?;
        let decode_ms = t2.elapsed().as_secs_f64() * 1000.0;

        anyhow::ensure!(
            decoded.len() >= expected_row_bytes,
            "{} decoded response too short: {} bytes, expected >= {}",
            tier_name,
            decoded.len(),
            expected_row_bytes
        );
        Ok((
            decoded[..expected_row_bytes].to_vec(),
            TierTiming {
                gen_ms,
                upload_bytes,
                upload_q_bytes,
                upload_pp_bytes,
                download_bytes: response_bytes.len(),
                rtt_ms,
                decode_ms,
                server_req_id,
                server_total_ms,
                server_validate_ms,
                server_decode_copy_ms,
                server_compute_ms,
                net_queue_ms,
                upload_to_server_ms,
                download_from_server_ms,
            },
        ))
    }

    /// Batched analogue of [`ypir_query`](Self::ypir_query). Issues K YPIR
    /// queries against `tier_name` in a single
    /// `POST /tier{1,2}/batch_query` request.
    ///
    /// All K queries share one `client_seed` (and therefore one secret
    /// `s` and one `pack_pub_params`), which is the upload bandwidth
    /// lever the batched protocol exploits. The per-query LWE error vectors
    /// `e_k` remain independent because each `q.0` is generated under
    /// its own fresh secret RNG inside the YPIR client.
    ///
    /// Returns one inner `Result<Vec<u8>>` per slot. Per-slot errors
    /// (decryption panics, short responses) are reported in the inner
    /// result so the *other* slots can still succeed; HTTP-level errors
    /// (connect failures, malformed batch wire format) propagate via
    /// the outer `Result` and abort the whole batch. Each per-slot
    /// decode is wrapped in `catch_unwind` so an LWE-decode panic on
    /// one note cannot prevent decoding of the other notes — the
    /// error-oracle invariant of [`fetch_proof_inner`](Self::fetch_proof_inner)
    /// extends to the whole batch.
    async fn ypir_batch_query(
        &self,
        scenario: &YpirScenario,
        tier_name: &str,
        row_indices: &[usize],
        expected_row_bytes: usize,
    ) -> Result<(Vec<Result<Vec<u8>>>, BatchTierTiming)> {
        let k = row_indices.len();
        anyhow::ensure!(k >= 1, "{tier_name} batch must have at least 1 query");
        anyhow::ensure!(
            k <= MAX_BATCH_K,
            "{tier_name} batch K = {k} exceeds MAX_BATCH_K = {MAX_BATCH_K}"
        );
        for (i, &row_idx) in row_indices.iter().enumerate() {
            anyhow::ensure!(
                row_idx < scenario.num_items,
                "{tier_name} batch[{i}] row_idx {row_idx} >= num_items {}",
                scenario.num_items
            );
        }

        let t0 = Instant::now();
        let ypir_client = YPIRClient::from_db_sz(
            scenario.num_items as u64,
            scenario.item_size_bits as u64,
            true,
        );
        let (batch_query, seed) = ypir_client.generate_query_simplepir_batch(row_indices);
        let gen_ms = t0.elapsed().as_secs_f64() * 1000.0;

        // Build the wire payload: one shared `pp` plus K independent `q.0`s.
        let mut pqr_refs: Vec<&[u64]> = Vec::with_capacity(batch_query.0.len());
        for aq in batch_query.0.iter() {
            let s: &[u64] = aq.as_slice();
            pqr_refs.push(s);
        }
        let upload_q_bytes_per_query =
            pqr_refs.first().map(|q| q.len()).unwrap_or(0) * std::mem::size_of::<u64>();
        let upload_pp_bytes_total = batch_query.1.as_slice().len() * std::mem::size_of::<u64>();
        let payload = serialize_ypir_batch_query(&pqr_refs, batch_query.1.as_slice());
        let upload_bytes_total = payload.len();

        let t1 = Instant::now();
        let url = format!("{}/{}/batch_query", self.server_url, tier_name);
        let send_result = self.http.post(&url).body(payload).send().await;
        let send_ms = t1.elapsed().as_secs_f64() * 1000.0;
        let resp = match send_result {
            Ok(r) => r,
            Err(e) => {
                log::warn!("YPIR {tier_name} batch send error: {e:?}");
                return Err(e.into());
            }
        };
        let server_req_id = parse_header_u64(resp.headers(), "x-pir-req-id");
        let server_batch_k = parse_header_u64(resp.headers(), "x-pir-batch-k");
        let server_total_ms = parse_header_f64(resp.headers(), "x-pir-server-total-ms");
        let server_validate_ms = parse_header_f64(resp.headers(), "x-pir-server-validate-ms");
        let server_decode_copy_ms = parse_header_f64(resp.headers(), "x-pir-server-decode-copy-ms");
        let server_compute_ms = parse_header_f64(resp.headers(), "x-pir-server-compute-ms");
        let status = resp.status();
        let response_bytes = resp.bytes().await?;
        if !status.is_success() {
            anyhow::bail!(
                "{tier_name} batch query failed: HTTP {status} body={}",
                String::from_utf8_lossy(&response_bytes)
            );
        }
        let rtt_ms = t1.elapsed().as_secs_f64() * 1000.0;
        let download_from_server_ms = (rtt_ms - send_ms).max(0.0);
        let upload_to_server_ms = server_total_ms.map(|s| (send_ms - s).max(0.0));

        // Sanity-check the round trip: the server should echo back our K.
        if let Some(echoed) = server_batch_k {
            anyhow::ensure!(
                echoed as usize == k,
                "{tier_name} batch K mismatch: sent {k}, server echoed {echoed}"
            );
        }

        // Wire-format parse failures (and per-slot decode panics) become
        // per-slot `Err`s inside the returned vector — this keeps the
        // error-oracle invariant: even if every tier-1 slot fails to
        // decode, the caller still sees `Ok((vec![Err; K], _))` and
        // can dispatch the next-tier batch. HTTP-level errors above
        // still bail the outer `Result` because the server would have
        // been able to gate based on retry behaviour anyway (the
        // single-query path has the same property).
        let mut per_note_results: Vec<Result<Vec<u8>>> = Vec::with_capacity(k);
        let mut per_note_decode_ms: Vec<f64> = vec![0.0; k];
        let download_bytes_per_query;

        match parse_ypir_batch_response(&response_bytes) {
            Ok(chunks) if chunks.len() == k => {
                download_bytes_per_query = chunks.first().map(|c| c.len()).unwrap_or(0);
                // Decode each response one at a time, wrapped in
                // `catch_unwind` for per-slot error isolation. The YPIR
                // `decode_response_simplepir_batch` API decodes K
                // responses inside a single call sharing one `YClient`
                // — but a panic in any one slot poisons the whole
                // result, which would defeat the error-oracle
                // mitigation. Single-shot decode rebuilds the YClient
                // K times; that is cheap compared to one LWE decode
                // pass.
                for (i, chunk) in chunks.iter().enumerate() {
                    let dec_start = Instant::now();
                    let decoded =
                        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            ypir_client.decode_response_simplepir(seed, chunk)
                        }))
                        .map_err(|payload| {
                            let msg = payload
                                .downcast_ref::<String>()
                                .map(|s| s.as_str())
                                .or_else(|| payload.downcast_ref::<&str>().copied())
                                .unwrap_or("unknown panic");
                            anyhow::anyhow!(
                                "{tier_name} batch[{i}] response decryption panicked: {msg}"
                            )
                        });
                    per_note_decode_ms[i] = dec_start.elapsed().as_secs_f64() * 1000.0;
                    let row_result = decoded.and_then(|d| {
                        anyhow::ensure!(
                            d.len() >= expected_row_bytes,
                            "{tier_name} batch[{i}] decoded response too short: {} bytes, expected >= {}",
                            d.len(),
                            expected_row_bytes
                        );
                        Ok(d[..expected_row_bytes].to_vec())
                    });
                    per_note_results.push(row_result);
                }
            }
            Ok(chunks) => {
                // K mismatch — fail every slot so the caller still
                // dispatches the next-tier batch.
                let actual = chunks.len();
                download_bytes_per_query = 0;
                for i in 0..k {
                    per_note_results.push(Err(anyhow::anyhow!(
                        "{tier_name} batch[{i}] response had {actual} chunks, expected {k}"
                    )));
                }
            }
            Err(e) => {
                download_bytes_per_query = 0;
                for i in 0..k {
                    per_note_results.push(Err(anyhow::anyhow!(
                        "{tier_name} batch[{i}] response parse: {e}"
                    )));
                }
            }
        }

        let timing = BatchTierTiming {
            k,
            gen_ms,
            upload_bytes_total,
            upload_pp_bytes_total,
            upload_q_bytes_per_query,
            download_bytes_per_query,
            rtt_ms,
            per_note_decode_ms,
            server_req_id,
            server_total_ms,
            server_validate_ms,
            server_decode_copy_ms,
            server_compute_ms,
            server_batch_k,
            upload_to_server_ms,
            download_from_server_ms,
        };
        Ok((per_note_results, timing))
    }
}

fn fmt_time(ms: f64) -> String {
    if ms >= 1000.0 {
        format!("{:>5.1}s ", ms / 1000.0)
    } else {
        format!("{:>5.0}ms", ms)
    }
}

fn fmt_opt_time(ms: Option<f64>) -> String {
    match ms {
        Some(v) => fmt_time(v),
        None => "  n/a ".to_string(),
    }
}

/// Print a detailed timing breakdown table for fan-out (per-note) PIR fetches.
fn print_timing_table_fanout(results: &[(usize, ImtProofData, NoteTiming)], wall_ms: f64) {
    let view: Vec<(usize, &NoteTiming)> = results.iter().map(|(i, _, t)| (*i, t)).collect();
    print_timing_table(&view, wall_ms);
}

/// Print a detailed timing breakdown table for a batch of PIR proof fetches.
///
/// Borrowed-view variant so the batched path can reuse the formatting
/// without cloning [`NoteTiming`] for every slot.
fn print_timing_table(results: &[(usize, &NoteTiming)], wall_ms: f64) {
    if !log::log_enabled!(log::Level::Debug) {
        return;
    }

    log::debug!("[PIR] ┌─────┬──────────┬─────────────┬──────────┬──────────┬─────────────┬──────────┬────────┐");
    log::debug!("[PIR] │ Note│ T1 keygen│ T1 upload+  │ T1 decode│ T2 keygen│ T2 upload+  │ T2 decode│ Total  │");
    log::debug!("[PIR] │     │ (client) │ server+down │ (client) │ (client) │ server+down │ (client) │        │");
    log::debug!("[PIR] ├─────┼──────────┼─────────────┼──────────┼──────────┼─────────────┼──────────┼────────┤");
    for &(i, t) in results {
        log::debug!(
            "[PIR] │  {i:>2} │  {:>6} │   {:>7}   │  {:>6} │  {:>6} │   {:>7}   │  {:>6} │{} │",
            fmt_time(t.tier1.gen_ms),
            fmt_time(t.tier1.rtt_ms),
            fmt_time(t.tier1.decode_ms),
            fmt_time(t.tier2.gen_ms),
            fmt_time(t.tier2.rtt_ms),
            fmt_time(t.tier2.decode_ms),
            fmt_time(t.total_ms),
        );
    }
    log::debug!("[PIR] └─────┴──────────┴─────────────┴──────────┴──────────┴─────────────┴──────────┴────────┘");
    log::debug!(
        "[PIR] Upload per note: T1={:.0}KB T2={:.1}MB  |  Wall clock: {:.2}s",
        results
            .first()
            .map(|(_, t)| t.tier1.upload_bytes)
            .unwrap_or(0) as f64
            / 1024.0,
        results
            .first()
            .map(|(_, t)| t.tier2.upload_bytes)
            .unwrap_or(0) as f64
            / (1024.0 * 1024.0),
        wall_ms / 1000.0,
    );

    for &(i, t) in results {
        log::trace!(
            "[PIR] Note {i:>2} transfer: T1 up={:.0}KB down={:.0}KB | T2 up={:.1}MB down={:.0}KB",
            t.tier1.upload_bytes as f64 / 1024.0,
            t.tier1.download_bytes as f64 / 1024.0,
            t.tier2.upload_bytes as f64 / (1024.0 * 1024.0),
            t.tier2.download_bytes as f64 / 1024.0,
        );
        log::trace!(
            "[PIR] Note {i:>2} server/net: T1 {} / {} | T2 {} / {}",
            fmt_opt_time(t.tier1.server_total_ms),
            fmt_opt_time(t.tier1.net_queue_ms),
            fmt_opt_time(t.tier2.server_total_ms),
            fmt_opt_time(t.tier2.net_queue_ms),
        );
        log::trace!(
            "[PIR] Note {i:>2} up/srv/down: T1 {} / {} / {} | T2 {} / {} / {}",
            fmt_opt_time(t.tier1.upload_to_server_ms),
            fmt_opt_time(t.tier1.server_total_ms),
            fmt_time(t.tier1.download_from_server_ms),
            fmt_opt_time(t.tier2.upload_to_server_ms),
            fmt_opt_time(t.tier2.server_total_ms),
            fmt_time(t.tier2.download_from_server_ms),
        );
        log::trace!(
            "[PIR] Note {i:>2} server stages: T1(v={} copy={} compute={}) T2(v={} copy={} compute={})",
            fmt_opt_time(t.tier1.server_validate_ms),
            fmt_opt_time(t.tier1.server_decode_copy_ms),
            fmt_opt_time(t.tier1.server_compute_ms),
            fmt_opt_time(t.tier2.server_validate_ms),
            fmt_opt_time(t.tier2.server_decode_copy_ms),
            fmt_opt_time(t.tier2.server_compute_ms),
        );
        log::trace!(
            "[PIR] Note {i:>2} req ids: T1={:?} T2={:?}",
            t.tier1.server_req_id,
            t.tier2.server_req_id
        );
    }
}

/// Parse an HTTP response header value as `f64`, returning `None` on missing or malformed values.
fn parse_header_f64(headers: &reqwest::header::HeaderMap, name: &'static str) -> Option<f64> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<f64>().ok())
}

/// Parse an HTTP response header value as `u64`, returning `None` on missing or malformed values.
fn parse_header_u64(headers: &reqwest::header::HeaderMap, name: &'static str) -> Option<u64> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
}

// ── Blocking wrapper ─────────────────────────────────────────────────────────

/// Synchronous wrapper around [`PirClient`] for use from non-async code.
///
/// Owns a Tokio runtime internally so callers (e.g. zcash_voting, which must
/// stay synchronous for the Halo2 prover) don't need to manage one.
pub struct PirClientBlocking {
    inner: PirClient,
    rt: tokio::runtime::Runtime,
}

impl PirClientBlocking {
    /// Connect to a PIR server (blocking). Downloads Tier 0 data and YPIR params.
    pub fn connect(server_url: &str) -> Result<Self> {
        let rt = tokio::runtime::Runtime::new()?;
        let inner = rt.block_on(PirClient::connect(server_url))?;
        Ok(Self { inner, rt })
    }

    /// Like [`connect`](Self::connect) but with explicit [`PirClientConfig`].
    pub fn connect_with_config(server_url: &str, config: PirClientConfig) -> Result<Self> {
        let rt = tokio::runtime::Runtime::new()?;
        let inner = rt.block_on(PirClient::connect_with_http_and_config(
            server_url,
            reqwest::Client::new(),
            config,
        ))?;
        Ok(Self { inner, rt })
    }

    /// Like [`PirClient::connect_with_http_and_config`] but synchronous.
    pub fn connect_with_http_and_config(
        server_url: &str,
        http: reqwest::Client,
        config: PirClientConfig,
    ) -> Result<Self> {
        let rt = tokio::runtime::Runtime::new()?;
        let inner =
            rt.block_on(PirClient::connect_with_http_and_config(server_url, http, config))?;
        Ok(Self { inner, rt })
    }

    /// Perform a private Merkle path retrieval for a nullifier (blocking).
    pub fn fetch_proof(&self, nullifier: Fp) -> Result<ImtProofData> {
        self.rt.block_on(self.inner.fetch_proof(nullifier))
    }

    /// Perform private Merkle path retrieval for multiple nullifiers in parallel (blocking).
    pub fn fetch_proofs(&self, nullifiers: &[Fp]) -> Result<Vec<ImtProofData>> {
        self.rt.block_on(self.inner.fetch_proofs(nullifiers))
    }

    /// Like [`PirClient::fetch_proofs_with_timing`] but synchronous.
    pub fn fetch_proofs_with_timing(
        &self,
        nullifiers: &[Fp],
    ) -> Result<Vec<(ImtProofData, NoteTiming)>> {
        self.rt
            .block_on(self.inner.fetch_proofs_with_timing(nullifiers))
    }

    /// The depth-29 root (PIR depth 25 padded to tree depth 29).
    pub fn root29(&self) -> Fp {
        self.inner.root29
    }
}

// ── Local (in-process) PIR client ────────────────────────────────────────────

/// Perform a complete local PIR proof retrieval without HTTP.
///
/// This is used by `pir-test local` mode. It takes the tier data directly
/// (as built by `pir-export`) and performs the YPIR operations in-process.
pub fn fetch_proof_local(
    tier0_data: &[u8],
    tier1_data: &[u8],
    tier2_data: &[u8],
    num_ranges: usize,
    nullifier: Fp,
    empty_hashes: &[Fp; TREE_DEPTH],
    root29: Fp,
) -> Result<ImtProofData> {
    let mut path = [Fp::default(); TREE_DEPTH];
    let tier0 = Tier0Data::from_bytes(tier0_data.to_vec())?;

    let s1 = process_tier0(&tier0, nullifier, &mut path)?;

    // ── Tier 1: direct row lookup (no YPIR in local mode) ────────────────
    let t1_offset = s1 * TIER1_ROW_BYTES;
    anyhow::ensure!(
        t1_offset + TIER1_ROW_BYTES <= tier1_data.len(),
        "tier1 data too short: need {} bytes at offset {}, have {}",
        TIER1_ROW_BYTES,
        t1_offset,
        tier1_data.len()
    );
    let s2 = process_tier1(
        &tier1_data[t1_offset..t1_offset + TIER1_ROW_BYTES],
        nullifier,
        &mut path,
    )?;

    // ── Tier 2: direct row lookup (no YPIR in local mode) ────────────────
    let t2_row_idx = s1 * TIER1_LEAVES + s2;
    let t2_offset = t2_row_idx * TIER2_ROW_BYTES;
    anyhow::ensure!(
        t2_offset + TIER2_ROW_BYTES <= tier2_data.len(),
        "tier2 data too short: need {} bytes at offset {}, have {}",
        TIER2_ROW_BYTES,
        t2_offset,
        tier2_data.len()
    );

    process_tier2_and_build(
        &tier2_data[t2_offset..t2_offset + TIER2_ROW_BYTES],
        t2_row_idx,
        num_ranges,
        nullifier,
        &mut path,
        empty_hashes,
        root29,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff::Field;
    use pasta_curves::Fp;
    use pir_export::build_ranges_with_sentinels;

    /// Build a tree and export all three tier blobs.
    struct TestFixture {
        tier0_data: Vec<u8>,
        tier1_data: Vec<u8>,
        tier2_data: Vec<u8>,
        ranges: Vec<[Fp; 3]>,
        empty_hashes: [Fp; TREE_DEPTH],
        root29: Fp,
    }

    impl TestFixture {
        fn build(raw_nfs: &[Fp]) -> Self {
            let ranges = build_ranges_with_sentinels(raw_nfs);
            let tree = pir_export::build_pir_tree(ranges.clone()).unwrap();

            let tier0_data = pir_export::tier0::export(
                &tree.root25,
                &tree.levels,
                &tree.ranges,
                &tree.empty_hashes,
            );
            let mut tier1_data = Vec::new();
            pir_export::tier1::export(
                &tree.levels,
                &tree.ranges,
                &tree.empty_hashes,
                &mut tier1_data,
            )
            .unwrap();
            let mut tier2_data = Vec::new();
            pir_export::tier2::export(&tree.ranges, &mut tier2_data).unwrap();

            Self {
                tier0_data,
                tier1_data,
                tier2_data,
                ranges,
                empty_hashes: tree.empty_hashes,
                root29: tree.root29,
            }
        }
    }

    // ── fetch_proof_local round-trip ──────────────────────────────────────

    #[test]
    fn fetch_proof_local_verifies_for_known_ranges() {
        let mut rng = rand::thread_rng();
        let raw_nfs: Vec<Fp> = (0..100).map(|_| Fp::random(&mut rng)).collect();
        let fix = TestFixture::build(&raw_nfs);

        for &[nf_lo, _, _] in fix.ranges.iter().take(20) {
            let value = nf_lo + Fp::one();
            let proof = fetch_proof_local(
                &fix.tier0_data,
                &fix.tier1_data,
                &fix.tier2_data,
                fix.ranges.len(),
                value,
                &fix.empty_hashes,
                fix.root29,
            )
            .expect("fetch_proof_local should succeed for a value in range");
            assert!(
                proof.verify(value),
                "proof should verify for value {:?}",
                value,
            );
        }
    }

    #[test]
    fn fetch_proof_local_correct_root_and_path_length() {
        let raw_nfs: Vec<Fp> = (1u64..=50).map(|i| Fp::from(i * 997)).collect();
        let fix = TestFixture::build(&raw_nfs);

        let value = fix.ranges[0][0] + Fp::one(); // nf_lo + 1 is inside the range
        let proof = fetch_proof_local(
            &fix.tier0_data,
            &fix.tier1_data,
            &fix.tier2_data,
            fix.ranges.len(),
            value,
            &fix.empty_hashes,
            fix.root29,
        )
        .unwrap();

        assert_eq!(proof.root, fix.root29);
        assert_eq!(proof.path.len(), TREE_DEPTH);
    }

    // ── process_tier0 ────────────────────────────────────────────────────

    #[test]
    fn process_tier0_fills_correct_path_region() {
        let raw_nfs: Vec<Fp> = (1u64..=30).map(|i| Fp::from(i * 1013)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let tier0 = Tier0Data::from_bytes(fix.tier0_data).unwrap();

        let value = fix.ranges[0][0];
        let mut path = [Fp::default(); TREE_DEPTH];
        let s1 = process_tier0(&tier0, value, &mut path).unwrap();

        assert!(s1 < pir_types::TIER1_ROWS);

        let tier0_region = &path[PIR_DEPTH - TIER0_LAYERS..PIR_DEPTH];
        assert!(
            tier0_region.iter().any(|&v| v != Fp::default()),
            "tier0 should write at least one non-zero sibling"
        );

        let below = &path[..PIR_DEPTH - TIER0_LAYERS];
        assert!(
            below.iter().all(|&v| v == Fp::default()),
            "path below tier0 region should be untouched"
        );
    }

    #[test]
    fn process_tier0_handles_arbitrary_field_element() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let tier0 = Tier0Data::from_bytes(fix.tier0_data).unwrap();

        // Sentinel nullifiers span the field, so every non-nullifier value
        // falls in some gap range. Verify this doesn't panic and returns a
        // valid subtree index.
        let bogus = Fp::from(u64::MAX);
        let mut path = [Fp::default(); TREE_DEPTH];
        let s1 = process_tier0(&tier0, bogus, &mut path).unwrap();
        assert!(s1 < pir_types::TIER1_ROWS);
    }

    // ── process_tier1 ────────────────────────────────────────────────────

    #[test]
    fn process_tier1_fills_correct_path_region() {
        let raw_nfs: Vec<Fp> = (1u64..=30).map(|i| Fp::from(i * 1013)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let tier0 = Tier0Data::from_bytes(fix.tier0_data.clone()).unwrap();

        let value = fix.ranges[0][0];
        let mut path = [Fp::default(); TREE_DEPTH];
        let s1 = process_tier0(&tier0, value, &mut path).unwrap();

        let t1_offset = s1 * TIER1_ROW_BYTES;
        let tier1_row = &fix.tier1_data[t1_offset..t1_offset + TIER1_ROW_BYTES];
        let s2 = process_tier1(tier1_row, value, &mut path).unwrap();

        assert!(s2 < TIER1_LEAVES);

        let tier1_region = &path[PIR_DEPTH - TIER0_LAYERS - TIER1_LAYERS..PIR_DEPTH - TIER0_LAYERS];
        assert!(
            tier1_region.iter().any(|&v| v != Fp::default()),
            "tier1 should write at least one non-zero sibling"
        );
    }

    // ── process_tier2_and_build ───────────────────────────────────────────

    #[test]
    fn process_tier2_and_build_produces_verifiable_proof() {
        let raw_nfs: Vec<Fp> = (1u64..=30).map(|i| Fp::from(i * 1013)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let tier0 = Tier0Data::from_bytes(fix.tier0_data.clone()).unwrap();

        let value = fix.ranges[0][0] + Fp::one();
        let mut path = [Fp::default(); TREE_DEPTH];

        let s1 = process_tier0(&tier0, value, &mut path).unwrap();
        let t1_offset = s1 * TIER1_ROW_BYTES;
        let s2 = process_tier1(
            &fix.tier1_data[t1_offset..t1_offset + TIER1_ROW_BYTES],
            value,
            &mut path,
        )
        .unwrap();

        let t2_row_idx = s1 * TIER1_LEAVES + s2;
        let t2_offset = t2_row_idx * TIER2_ROW_BYTES;
        let proof = process_tier2_and_build(
            &fix.tier2_data[t2_offset..t2_offset + TIER2_ROW_BYTES],
            t2_row_idx,
            fix.ranges.len(),
            value,
            &mut path,
            &fix.empty_hashes,
            fix.root29,
        )
        .unwrap();

        assert!(proof.verify(value));
        assert_eq!(proof.root, fix.root29);
    }

    // ── valid_leaves_for_row ──────────────────────────────────────────────

    #[test]
    fn valid_leaves_for_row_basic() {
        assert_eq!(valid_leaves_for_row(TIER2_LEAVES, 0), TIER2_LEAVES);
        assert_eq!(valid_leaves_for_row(TIER2_LEAVES + 1, 0), TIER2_LEAVES);
        assert_eq!(valid_leaves_for_row(TIER2_LEAVES + 1, 1), 1);
        assert_eq!(valid_leaves_for_row(0, 0), 0);
        assert_eq!(valid_leaves_for_row(1, 0), 1);
        assert_eq!(valid_leaves_for_row(1, 1), 0);
    }

    // ── fetch_proof_local error paths ─────────────────────────────────────

    #[test]
    fn fetch_proof_local_rejects_truncated_tier1() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build(&raw_nfs);

        let result = fetch_proof_local(
            &fix.tier0_data,
            &fix.tier1_data[..TIER1_ROW_BYTES / 2],
            &fix.tier2_data,
            fix.ranges.len(),
            fix.ranges[0][0],
            &fix.empty_hashes,
            fix.root29,
        );
        assert!(result.is_err());
    }

    #[test]
    fn fetch_proof_local_rejects_truncated_tier2() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build(&raw_nfs);

        let result = fetch_proof_local(
            &fix.tier0_data,
            &fix.tier1_data,
            &fix.tier2_data[..TIER2_ROW_BYTES / 2],
            fix.ranges.len(),
            fix.ranges[0][0],
            &fix.empty_hashes,
            fix.root29,
        );
        assert!(result.is_err());
    }

    // ── Error-oracle mitigation ─────────────────────────────────────────

    /// Verify that the tier 2 query is always sent to the server even when
    /// the tier 1 response is corrupted.
    ///
    /// A malicious server could craft a tier 1 response whose decryption
    /// outcome depends on the client's secret key material (e.g. by
    /// triggering an assert in the LWE decode path). Without the
    /// mitigation, a decode failure would prevent the tier 2 query from
    /// being sent, and the server could use the absence of query 2 as a
    /// single-bit oracle. This test asserts that both queries are always
    /// issued regardless of tier 1 outcome.
    #[tokio::test]
    async fn tier2_query_sent_despite_tier1_decode_failure() {
        use ff::PrimeField as _;
        use pir_types::{TIER1_ITEM_BITS, TIER1_YPIR_ROWS, TIER2_ITEM_BITS};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        // Build real tier0 data so PirClient::connect() succeeds and
        // process_tier0() produces a valid subtree index.
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let ranges = build_ranges_with_sentinels(&raw_nfs);
        let tree = pir_export::build_pir_tree(ranges).unwrap();
        let tier0_data =
            pir_export::tier0::export(&tree.root25, &tree.levels, &tree.ranges, &tree.empty_hashes);

        let root_info = pir_types::RootInfo {
            root29: hex::encode(tree.root29.to_repr()),
            root25: hex::encode(tree.root25.to_repr()),
            num_ranges: tree.ranges.len(),
            pir_depth: PIR_DEPTH,
            height: None,
            supports_batch_query: true,
        };

        // Use the real item_size_bits to satisfy YPIR's internal
        // parameter constraints. num_items=TIER1_YPIR_ROWS (2048) matches
        // production tier1 (which pads TIER1_ROWS=512 up to the YPIR
        // poly_len floor) and is large enough for any s1 value.
        let tier1_scenario = YpirScenario {
            num_items: TIER1_YPIR_ROWS,
            item_size_bits: TIER1_ITEM_BITS,
        };
        let tier2_scenario = YpirScenario {
            num_items: TIER1_YPIR_ROWS,
            item_size_bits: TIER2_ITEM_BITS,
        };

        let server = MockServer::start().await;

        // ── setup endpoints (valid data) ────────────────────────────────
        Mock::given(method("GET"))
            .and(path("/tier0"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(tier0_data))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/params/tier1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&tier1_scenario))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/params/tier2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&tier2_scenario))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/root"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&root_info))
            .mount(&server)
            .await;

        // ── query endpoints (corrupted responses) ───────────────────────
        Mock::given(method("POST"))
            .and(path("/tier1/query"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0xDE; 65536]))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/tier2/query"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0xAD; 65536]))
            .mount(&server)
            .await;

        // ── run the client ──────────────────────────────────────────────
        let client = PirClient::connect(&server.uri()).await.unwrap();
        let nullifier = tree.ranges[0][0];
        let result = client.fetch_proof(nullifier).await;

        assert!(
            result.is_err(),
            "fetch_proof should fail with corrupted tier1 response"
        );

        // ── verify both queries were sent ───────────────────────────────
        let received = server.received_requests().await.unwrap();
        let tier1_hits = received
            .iter()
            .filter(|r| r.url.path() == "/tier1/query")
            .count();
        let tier2_hits = received
            .iter()
            .filter(|r| r.url.path() == "/tier2/query")
            .count();

        assert_eq!(tier1_hits, 1, "tier1 query should have been sent");
        assert_eq!(
            tier2_hits, 1,
            "tier2 query must still be sent when tier1 decode fails \
             (error-oracle mitigation)"
        );
    }

    /// Batch-level analog of [`tier2_query_sent_despite_tier1_decode_failure`].
    ///
    /// With `supports_batch_query: true` the client sends one
    /// `POST /tier1/batch_query` and one `POST /tier2/batch_query`
    /// regardless of K. The error-oracle invariant adapted to batching
    /// becomes: **the tier 2 batch query MUST be sent even when the
    /// tier 1 batch response cannot be decoded**, so the server cannot
    /// use "did the client retry tier 2?" as a single-bit oracle on
    /// the client's secret material.
    ///
    /// We assert exactly one POST on each batch endpoint (independent
    /// of K), and zero POSTs on the legacy single-query endpoints (the
    /// server advertised batching support).
    #[tokio::test]
    async fn batched_tier2_query_sent_despite_tier1_decode_failure() {
        use ff::PrimeField as _;
        use pir_types::{TIER1_ITEM_BITS, TIER1_YPIR_ROWS, TIER2_ITEM_BITS};
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        const K: usize = 5;

        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let ranges = build_ranges_with_sentinels(&raw_nfs);
        let tree = pir_export::build_pir_tree(ranges).unwrap();
        let tier0_data =
            pir_export::tier0::export(&tree.root25, &tree.levels, &tree.ranges, &tree.empty_hashes);

        let root_info = pir_types::RootInfo {
            root29: hex::encode(tree.root29.to_repr()),
            root25: hex::encode(tree.root25.to_repr()),
            num_ranges: tree.ranges.len(),
            pir_depth: PIR_DEPTH,
            height: None,
            supports_batch_query: true,
        };

        let tier1_scenario = YpirScenario {
            num_items: TIER1_YPIR_ROWS,
            item_size_bits: TIER1_ITEM_BITS,
        };
        let tier2_scenario = YpirScenario {
            num_items: TIER1_YPIR_ROWS,
            item_size_bits: TIER2_ITEM_BITS,
        };

        let server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/tier0"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(tier0_data))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/params/tier1"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&tier1_scenario))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/params/tier2"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&tier2_scenario))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/root"))
            .respond_with(ResponseTemplate::new(200).set_body_json(&root_info))
            .mount(&server)
            .await;

        // Corrupted batch responses: the wire-format parser will fail
        // (since the K header is bogus), surfacing per-slot Errs. The
        // client must still dispatch the tier-2 batch.
        Mock::given(method("POST"))
            .and(path("/tier1/batch_query"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0xDE; 65536]))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/tier2/batch_query"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0xAD; 65536]))
            .mount(&server)
            .await;

        let client = PirClient::connect(&server.uri()).await.unwrap();
        assert!(
            client.supports_batch_query(),
            "test setup requires server to advertise batching support",
        );

        let nullifiers: Vec<Fp> = tree
            .ranges
            .iter()
            .take(K)
            .map(|r| r[0] + Fp::one())
            .collect();
        assert_eq!(nullifiers.len(), K);

        let result = client.fetch_proofs(&nullifiers).await;
        assert!(
            result.is_err(),
            "fetch_proofs should fail with corrupted tier1 batch response"
        );

        let received = server.received_requests().await.unwrap();
        let tier1_batch_hits = received
            .iter()
            .filter(|r| r.url.path() == "/tier1/batch_query")
            .count();
        let tier2_batch_hits = received
            .iter()
            .filter(|r| r.url.path() == "/tier2/batch_query")
            .count();
        let tier1_legacy_hits = received
            .iter()
            .filter(|r| r.url.path() == "/tier1/query")
            .count();
        let tier2_legacy_hits = received
            .iter()
            .filter(|r| r.url.path() == "/tier2/query")
            .count();

        assert_eq!(
            tier1_batch_hits, 1,
            "exactly one tier1 batch query must be sent (got {})",
            tier1_batch_hits,
        );
        assert_eq!(
            tier2_batch_hits, 1,
            "tier2 batch query must still be sent when tier1 batch \
             decode fails (batch-level error-oracle mitigation)",
        );
        assert_eq!(
            tier1_legacy_hits, 0,
            "client should not fall back to per-note tier1 endpoint when \
             server advertises batching"
        );
        assert_eq!(
            tier2_legacy_hits, 0,
            "client should not fall back to per-note tier2 endpoint when \
             server advertises batching"
        );

        // The body of each batch POST should parse as a wire-format
        // batch query of exactly K entries — the client must not have
        // sent a malformed payload.
        for path_name in ["/tier1/batch_query", "/tier2/batch_query"] {
            let req = received
                .iter()
                .find(|r| r.url.path() == path_name)
                .expect("batch request body present");
            let (pqrs, _pp) = pir_types::parse_ypir_batch_query(&req.body)
                .expect("batch query body parses as wire format");
            assert_eq!(
                pqrs.len(),
                K,
                "{path_name} body must contain {K} per-query payloads"
            );
        }
    }
}
