//! PIR client library for private Merkle path retrieval.
//!
//! Provides [`PirClient`] which connects to a `pir-server` instance and
//! retrieves circuit-ready `ImtProofData` without revealing the
//! queried nullifier to the server.

use std::{sync::Arc, time::Instant};

use anyhow::{Context, Result};
use ff::PrimeField as _;
use imt_tree::hasher::PoseidonHasher;
use imt_tree::tree::{precompute_empty_hashes, TREE_DEPTH};
use pasta_curves::Fp;
// Re-exported so downstream crates (e.g. zcash_voting) can reference the type
// returned by PirClientBlocking::fetch_proof without a direct imt-tree dependency.
pub use imt_tree::ImtProofData;

mod transport;
pub use pir_types::{PirLayout, ZcashNetwork, COMPILED_PIR_LAYOUT};
pub use transport::{Transport, TransportFuture, TransportResponse};

use pir_types::tier0::Tier0Data;
use pir_types::tier1::Tier1Row;
use pir_types::{serialize_ypir_query, RootInfo, YpirScenario, YPIR_MIN_ITEM_BITS, YPIR_MIN_ROWS};

use ypir::client::YPIRClient;

/// Valid row used when Tier 0 cannot safely route a nullifier. The encrypted
/// query is still sent so a server cannot distinguish routing failures by
/// observing whether `/tier1/query` was requested.
const DUMMY_TIER1_ROW: usize = 0;

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
    /// [`pir_types::serialize_ypir_query`]).
    pub upload_q_bytes: usize,
    /// Bytes of the uploaded query attributable to `pack_pub_params`
    /// (the second arg to [`pir_types::serialize_ypir_query`]). Identical
    /// across queries that share a YPIR `client_seed`.
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

/// Per-note timing breakdown for the single YPIR query.
pub struct NoteTiming {
    pub tier1: TierTiming,
    /// Total wall-clock time for this note's proof retrieval.
    pub total_ms: f64,
}

// ── HTTP-based PIR client ────────────────────────────────────────────────────

/// PIR client that connects to a `pir-server` instance over HTTP.
///
/// Downloads Tier 0 data and YPIR parameters during `connect()`, then
/// performs private queries via `fetch_proof()`.
pub struct PirClient {
    server_url: String,
    transport: Arc<dyn Transport>,
    layout: PirLayout,
    tier0: Tier0Data,
    tier1_scenario: YpirScenario,
    tier1_row_bytes: usize,
    num_ranges: usize,
    empty_hashes: [Fp; TREE_DEPTH],
    circuit_root: Fp,
}

/// Return the number of populated leaves in a Tier 1 row, clamped to
/// `leaves_per_row`. The final row may be only partially filled when
/// `num_ranges` is not a multiple of the row size.
#[inline]
fn valid_leaves_for_row(num_ranges: usize, row_idx: usize, leaves_per_row: usize) -> usize {
    let row_start = row_idx.saturating_mul(leaves_per_row);
    num_ranges.saturating_sub(row_start).min(leaves_per_row)
}

// ── Shared tier-processing helpers ───────────────────────────────────────────

/// Copy `siblings` into `path` starting at `offset`.
#[inline]
fn fill_path(path: &mut [Fp; TREE_DEPTH], offset: usize, siblings: &[Fp]) {
    path[offset..offset + siblings.len()].copy_from_slice(siblings);
}

/// Locate the nullifier's subtree in Tier 0, fill its siblings into `path`,
/// and return the subtree index `s1`.
fn process_tier0(
    tier0: &Tier0Data,
    layout: PirLayout,
    nullifier: Fp,
    path: &mut [Fp; TREE_DEPTH],
) -> Result<usize> {
    let s1 = tier0
        .find_subtree(nullifier)
        .context("nullifier not found in any Tier 0 subtree")?;
    let offset = layout
        .pir_depth
        .checked_sub(layout.tier0_layers)
        .context("PIR layout tier0_layers exceeds pir_depth")?;
    fill_path(path, offset, &tier0.extract_siblings(s1));
    Ok(s1)
}

/// Parse a Tier 1 row, locate the nullifier's leaf, fill its siblings and
/// circuit padding into `path`, and assemble the final [`ImtProofData`].
struct Tier1ProofInput<'a> {
    tier1_row: &'a [u8],
    layout: PirLayout,
    row_idx: usize,
    num_ranges: usize,
    nullifier: Fp,
    empty_hashes: &'a [Fp; TREE_DEPTH],
    circuit_root: Fp,
}

fn process_tier1_and_build(
    input: Tier1ProofInput<'_>,
    path: &mut [Fp; TREE_DEPTH],
) -> Result<ImtProofData> {
    let Tier1ProofInput {
        tier1_row,
        layout,
        row_idx,
        num_ranges,
        nullifier,
        empty_hashes,
        circuit_root,
    } = input;
    let hasher = PoseidonHasher::new();
    let tier1 = Tier1Row::from_layout(tier1_row, layout)?;
    let leaves = layout.tier1_leaves().map_err(anyhow::Error::msg)?;
    let valid_leaves = valid_leaves_for_row(num_ranges, row_idx, leaves);

    let leaf_local_idx = tier1
        .find_leaf(nullifier, valid_leaves)
        .context("nullifier not found in Tier 1 leaf scan")?;

    fill_path(
        path,
        0,
        &tier1.extract_siblings(leaf_local_idx, valid_leaves, &hasher),
    );
    // Pad from PIR depth to circuit depth (29) with empty hashes.
    fill_path(
        path,
        layout.pir_depth,
        &empty_hashes[layout.pir_depth..TREE_DEPTH],
    );

    let global_leaf_idx = row_idx
        .checked_mul(leaves)
        .and_then(|v| v.checked_add(leaf_local_idx))
        .context("global leaf index overflow")?;
    let (nf_lo, nf_mid, nf_hi) = tier1.leaf_record(leaf_local_idx);

    Ok(ImtProofData {
        root: circuit_root,
        nf_bounds: [nf_lo, nf_mid, nf_hi],
        leaf_pos: global_leaf_idx as u32,
        path: *path,
    })
}

fn validate_layout(label: &str, layout: PirLayout) -> Result<()> {
    layout
        .validate_supported()
        .map_err(|e| anyhow::anyhow!("{label} {e}"))?;
    Ok(())
}

fn tier1_geometry(layout: PirLayout) -> Result<(usize, usize, usize)> {
    let rows = layout.tier1_rows().map_err(anyhow::Error::msg)?;
    let leaves = layout.tier1_leaves().map_err(anyhow::Error::msg)?;
    let row_bytes = layout.tier1_row_bytes().map_err(anyhow::Error::msg)?;
    Ok((rows, leaves, row_bytes))
}

impl PirClient {
    /// Connect using a caller-provided HTTP transport, expected Zcash network,
    /// and configuration layout.
    pub async fn with_transport(
        server_url: &str,
        expected_network: ZcashNetwork,
        expected_layout: PirLayout,
        transport: Arc<dyn Transport>,
    ) -> Result<Self> {
        // Validate caller-controlled configuration before it can commit the
        // client to any network download or YPIR parameter generation.
        validate_layout("expected", expected_layout)?;

        let base = server_url.trim_end_matches('/');

        // Validate the small root response before downloading layout-sized
        // Tier 0 data or YPIR parameters.
        let t0 = Instant::now();
        let tier0_url = format!("{base}/tier0");
        let tier1_url = format!("{base}/params/tier1");
        let root_url = format!("{base}/root");
        let root_resp = transport
            .get(&root_url)
            .await
            .map_err(|e| anyhow::anyhow!("connect root fetch failed: {e}"))?;
        let root_info: RootInfo =
            serde_json::from_slice(&body_for_status(root_resp, "GET /root failed")?)
                .context("parse /root response")?;
        anyhow::ensure!(
            root_info.zcash_network == expected_network,
            "Zcash network mismatch: expected {}, server advertised {}",
            expected_network,
            root_info.zcash_network
        );
        anyhow::ensure!(
            pir_types::is_current_dataset(&root_info.nullifier_pool, root_info.dataset_version),
            "server nullifier dataset {:?} version {} is unsupported; expected {:?} version {}",
            root_info.nullifier_pool,
            root_info.dataset_version,
            pir_types::NULLIFIER_POOL,
            pir_types::DATASET_VERSION
        );
        validate_layout("server", root_info.pir_layout)?;
        anyhow::ensure!(
            expected_layout == root_info.pir_layout,
            "PIR layout mismatch: expected {:?}, server advertised {:?}",
            expected_layout,
            root_info.pir_layout
        );
        anyhow::ensure!(
            root_info.pir_depth == root_info.pir_layout.pir_depth,
            "server pir_depth {} disagrees with advertised layout depth {}",
            root_info.pir_depth,
            root_info.pir_layout.pir_depth
        );

        let (tier0_resp, tier1_resp) =
            tokio::try_join!(transport.get(&tier0_url), transport.get(&tier1_url))
                .map_err(|e| anyhow::anyhow!("connect data fetch failed: {e}"))?;
        let tier1_scenario: YpirScenario =
            serde_json::from_slice(&body_for_status(tier1_resp, "GET /params/tier1 failed")?)
                .context("parse /params/tier1 response")?;

        let (layout_rows, _layout_leaves, layout_row_bytes) = tier1_geometry(root_info.pir_layout)?;
        let scenario_item_bits = root_info
            .tier1_row_bytes
            .checked_mul(8)
            .context("server Tier 1 row width overflows bit size")?;
        anyhow::ensure!(
            layout_rows == root_info.tier1_rows
                && layout_row_bytes == root_info.tier1_row_bytes
                && tier1_scenario.num_items == root_info.tier1_rows
                && tier1_scenario.item_size_bits == scenario_item_bits
                && layout_rows >= YPIR_MIN_ROWS
                && scenario_item_bits >= YPIR_MIN_ITEM_BITS,
            "server Tier 1 shape mismatch: layout implies {}x{} bytes, /root reports {}x{} bytes, and /params reports {} items x {} bits",
            layout_rows,
            layout_row_bytes,
            root_info.tier1_rows,
            root_info.tier1_row_bytes,
            tier1_scenario.num_items,
            tier1_scenario.item_size_bits
        );

        let expected_tier0_bytes = root_info
            .pir_layout
            .tier0_bytes()
            .map_err(anyhow::Error::msg)?;
        let tier0_bytes = body_for_status(tier0_resp, "GET /tier0 failed")?;
        log::debug!(
            "Downloaded Tier 0: {} bytes in {:.1}s",
            tier0_bytes.len(),
            t0.elapsed().as_secs_f64()
        );
        anyhow::ensure!(
            tier0_bytes.len() == expected_tier0_bytes,
            "Tier 0 size mismatch: got {} bytes, layout expects {}",
            tier0_bytes.len(),
            expected_tier0_bytes
        );
        let tier0 = Tier0Data::from_layout(tier0_bytes.to_vec(), root_info.pir_layout)?;

        let pir_root_bytes = hex::decode(&root_info.pir_root)?;
        anyhow::ensure!(
            pir_root_bytes.len() == 32,
            "pir_root hex decoded to {} bytes, expected 32",
            pir_root_bytes.len()
        );
        let mut pir_root_arr = [0u8; 32];
        pir_root_arr.copy_from_slice(&pir_root_bytes);
        let pir_root = Option::from(Fp::from_repr(pir_root_arr))
            .ok_or_else(|| anyhow::anyhow!("invalid pir_root field element"))?;
        anyhow::ensure!(
            tier0.root() == pir_root,
            "Tier 0 root does not match server pir_root"
        );

        let circuit_root_bytes = hex::decode(&root_info.circuit_root)?;
        anyhow::ensure!(
            circuit_root_bytes.len() == 32,
            "circuit_root hex decoded to {} bytes, expected 32",
            circuit_root_bytes.len()
        );
        let mut circuit_root_arr = [0u8; 32];
        circuit_root_arr.copy_from_slice(&circuit_root_bytes);
        let circuit_root = Option::from(Fp::from_repr(circuit_root_arr))
            .ok_or_else(|| anyhow::anyhow!("invalid circuit_root field element"))?;

        let empty_hashes = precompute_empty_hashes();

        Ok(Self {
            server_url: base.to_string(),
            transport,
            layout: root_info.pir_layout,
            tier0,
            tier1_scenario,
            tier1_row_bytes: layout_row_bytes,
            num_ranges: root_info.num_ranges,
            empty_hashes,
            circuit_root,
        })
    }

    /// Perform private Merkle path retrieval for a nullifier.
    ///
    /// Returns circuit-ready `ImtProofData` with a 29-element path
    /// (19 PIR siblings + 10 empty-hash padding).
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

    /// Perform private Merkle path retrieval for multiple nullifiers in parallel.
    ///
    /// All queries run concurrently via `join_all`, sharing the same
    /// `PirClient` (and thus the same HTTP client and Tier 0 data). Errors are
    /// propagated only after every sibling request has completed.
    pub async fn fetch_proofs(&self, nullifiers: &[Fp]) -> Result<Vec<ImtProofData>> {
        log::debug!(
            "[PIR] Starting parallel fetch for {} notes...",
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

        let results_with_timing = futures::future::join_all(futures)
            .await
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        let wall_ms = wall_start.elapsed().as_secs_f64() * 1000.0;

        print_timing_table(&results_with_timing, wall_ms);

        let proofs = results_with_timing
            .into_iter()
            .map(|(_, proof, _)| proof)
            .collect();
        Ok(proofs)
    }

    /// Fetch proof and return timing breakdown.
    ///
    async fn fetch_proof_inner(&self, nullifier: Fp) -> Result<(ImtProofData, NoteTiming)> {
        let note_start = Instant::now();
        let mut path = [Fp::default(); TREE_DEPTH];

        // A hostile Tier 0 must not be able to suppress the private request.
        // Convert both normal errors and unwind panics into a deferred routing
        // error, and query a fixed valid row when routing failed. Fresh YPIR
        // randomness makes that dummy query indistinguishable from a real one.
        let tier0_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            process_tier0(&self.tier0, self.layout, nullifier, &mut path)
        }))
        .map_err(|panic_payload| {
            let msg = panic_payload
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| panic_payload.downcast_ref::<&str>().copied())
                .unwrap_or("unknown panic");
            anyhow::anyhow!("Tier 0 processing panicked: {msg}")
        })
        .and_then(|result| result);
        let query_row = tier0_result.as_ref().copied().unwrap_or(DUMMY_TIER1_ROW);

        // Await the Tier 1 attempt before propagating the deferred Tier 0
        // failure, preserving one request attempt per input nullifier.
        let tier1_result = self
            .ypir_query(
                &self.tier1_scenario,
                "tier1",
                query_row,
                self.tier1_row_bytes,
            )
            .await;
        let s1 = tier0_result?;
        let (tier1_row, tier1_timing) = tier1_result?;
        let proof = process_tier1_and_build(
            Tier1ProofInput {
                tier1_row: &tier1_row,
                layout: self.layout,
                row_idx: s1,
                num_ranges: self.num_ranges,
                nullifier,
                empty_hashes: &self.empty_hashes,
                circuit_root: self.circuit_root,
            },
            &mut path,
        )?;

        let total_ms = note_start.elapsed().as_secs_f64() * 1000.0;
        Ok((
            proof,
            NoteTiming {
                tier1: tier1_timing,
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
        // (per-query); `query.1` is `pack_pub_params` (depends only on
        // the client's `client_seed`).
        let upload_q_bytes = std::mem::size_of_val(query.0.as_slice());
        let upload_pp_bytes = std::mem::size_of_val(query.1.as_slice());
        let payload = serialize_ypir_query(query.0.as_slice(), query.1.as_slice());
        let upload_bytes = payload.len();

        // Send the request
        let t1 = Instant::now();
        let url = format!("{}/{}/query", self.server_url, tier_name);
        let send_result = self.transport.post(&url, payload).await;
        let send_ms = t1.elapsed().as_secs_f64() * 1000.0;
        let resp = match send_result {
            Ok(r) => r,
            Err(e) => {
                log::warn!("YPIR {} send error: {:?}", tier_name, e);
                return Err(e);
            }
        };
        let server_req_id = parse_header_u64(&resp.headers, "x-pir-req-id");
        let server_total_ms = parse_header_f64(&resp.headers, "x-pir-server-total-ms");
        let server_validate_ms = parse_header_f64(&resp.headers, "x-pir-server-validate-ms");
        let server_decode_copy_ms = parse_header_f64(&resp.headers, "x-pir-server-decode-copy-ms");
        let server_compute_ms = parse_header_f64(&resp.headers, "x-pir-server-compute-ms");
        let status = resp.status;
        let response_bytes = resp.body;
        if !is_success(status) {
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

        // Decode the response. Wrap in catch_unwind so that assertion panics
        // in the YPIR library (e.g. `val < lwe_q_prime` in the LWE decode
        // path) become recoverable errors rather than allowing a hostile
        // response to unwind through and abort the client process.
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

fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

fn body_for_status(response: TransportResponse, context: &'static str) -> Result<Vec<u8>> {
    if is_success(response.status) {
        Ok(response.body)
    } else {
        anyhow::bail!(
            "{}: HTTP {} body={}",
            context,
            response.status,
            String::from_utf8_lossy(&response.body)
        )
    }
}

/// Print a detailed timing breakdown table for a batch of PIR proof fetches.
fn print_timing_table(results: &[(usize, ImtProofData, NoteTiming)], wall_ms: f64) {
    if !log::log_enabled!(log::Level::Debug) {
        return;
    }

    log::debug!("[PIR] ┌─────┬──────────┬─────────────┬──────────┬────────┐");
    log::debug!("[PIR] │ Note│ T1 keygen│ T1 upload+  │ T1 decode│ Total  │");
    log::debug!("[PIR] │     │ (client) │ server+down │ (client) │        │");
    log::debug!("[PIR] ├─────┼──────────┼─────────────┼──────────┼────────┤");
    for &(i, _, ref t) in results {
        log::debug!(
            "[PIR] │  {i:>2} │  {:>6} │   {:>7}   │  {:>6} │{} │",
            fmt_time(t.tier1.gen_ms),
            fmt_time(t.tier1.rtt_ms),
            fmt_time(t.tier1.decode_ms),
            fmt_time(t.total_ms),
        );
    }
    log::debug!("[PIR] └─────┴──────────┴─────────────┴──────────┴────────┘");
    log::debug!(
        "[PIR] Upload per note: T1={:.0}KB  |  Wall clock: {:.2}s",
        results
            .first()
            .map(|(_, _, t)| t.tier1.upload_bytes)
            .unwrap_or(0) as f64
            / 1024.0,
        wall_ms / 1000.0,
    );

    for &(i, _, ref t) in results {
        log::trace!(
            "[PIR] Note {i:>2} transfer: T1 up={:.0}KB down={:.0}KB",
            t.tier1.upload_bytes as f64 / 1024.0,
            t.tier1.download_bytes as f64 / 1024.0,
        );
        log::trace!(
            "[PIR] Note {i:>2} server/net: T1 {} / {}",
            fmt_opt_time(t.tier1.server_total_ms),
            fmt_opt_time(t.tier1.net_queue_ms),
        );
        log::trace!(
            "[PIR] Note {i:>2} up/srv/down: T1 {} / {} / {}",
            fmt_opt_time(t.tier1.upload_to_server_ms),
            fmt_opt_time(t.tier1.server_total_ms),
            fmt_time(t.tier1.download_from_server_ms),
        );
        log::trace!(
            "[PIR] Note {i:>2} server stages: T1(v={} copy={} compute={})",
            fmt_opt_time(t.tier1.server_validate_ms),
            fmt_opt_time(t.tier1.server_decode_copy_ms),
            fmt_opt_time(t.tier1.server_compute_ms),
        );
        log::trace!("[PIR] Note {i:>2} req id: T1={:?}", t.tier1.server_req_id);
    }
}

/// Parse an HTTP response header value as `f64`, returning `None` on missing or malformed values.
fn parse_header_f64(headers: &[(String, String)], name: &'static str) -> Option<f64> {
    headers
        .iter()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .and_then(|(_, value)| value.parse::<f64>().ok())
}

/// Parse an HTTP response header value as `u64`, returning `None` on missing or malformed values.
fn parse_header_u64(headers: &[(String, String)], name: &'static str) -> Option<u64> {
    headers
        .iter()
        .find(|(header_name, _)| header_name.eq_ignore_ascii_case(name))
        .and_then(|(_, value)| value.parse::<u64>().ok())
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
    /// Connect with a caller-provided HTTP transport, expected Zcash network,
    /// and configuration layout.
    pub fn with_transport(
        server_url: &str,
        expected_network: ZcashNetwork,
        expected_layout: PirLayout,
        transport: Arc<dyn Transport>,
    ) -> Result<Self> {
        let rt = tokio::runtime::Runtime::new()?;
        let inner = rt.block_on(PirClient::with_transport(
            server_url,
            expected_network,
            expected_layout,
            transport,
        ))?;
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

    /// The circuit root (the PIR root padded to tree depth 29).
    pub fn circuit_root(&self) -> Fp {
        self.inner.circuit_root
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
    num_ranges: usize,
    nullifier: Fp,
    empty_hashes: &[Fp; TREE_DEPTH],
    circuit_root: Fp,
) -> Result<ImtProofData> {
    fetch_proof_local_layout(
        tier0_data,
        tier1_data,
        COMPILED_PIR_LAYOUT,
        num_ranges,
        nullifier,
        empty_hashes,
        circuit_root,
    )
}

/// Local proof retrieval driven by an explicit two-tier [`PirLayout`].
pub fn fetch_proof_local_layout(
    tier0_data: &[u8],
    tier1_data: &[u8],
    layout: PirLayout,
    num_ranges: usize,
    nullifier: Fp,
    empty_hashes: &[Fp; TREE_DEPTH],
    circuit_root: Fp,
) -> Result<ImtProofData> {
    validate_layout("local", layout)?;
    let mut path = [Fp::default(); TREE_DEPTH];
    let tier0 = Tier0Data::from_layout(tier0_data.to_vec(), layout)?;
    let row_bytes = layout.tier1_row_bytes().map_err(anyhow::Error::msg)?;

    let s1 = process_tier0(&tier0, layout, nullifier, &mut path)?;

    // ── Tier 1: direct row lookup (no YPIR in local mode) ────────────────
    let t1_offset = s1
        .checked_mul(row_bytes)
        .context("tier1 row offset overflow")?;
    let t1_end = t1_offset
        .checked_add(row_bytes)
        .context("tier1 row end overflow")?;
    anyhow::ensure!(
        t1_end <= tier1_data.len(),
        "tier1 data too short: need {} bytes at offset {}, have {}",
        row_bytes,
        t1_offset,
        tier1_data.len()
    );
    process_tier1_and_build(
        Tier1ProofInput {
            tier1_row: &tier1_data[t1_offset..t1_end],
            layout,
            row_idx: s1,
            num_ranges,
            nullifier,
            empty_hashes,
            circuit_root,
        },
        &mut path,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff::Field;
    use pasta_curves::Fp;
    use pir_export::build_ranges_with_sentinels;

    /// Build a tree and export both tier blobs for a layout.
    struct TestFixture {
        layout: PirLayout,
        tier0_data: Vec<u8>,
        tier1_data: Vec<u8>,
        ranges: Vec<[Fp; 3]>,
        empty_hashes: [Fp; TREE_DEPTH],
        circuit_root: Fp,
    }

    impl TestFixture {
        fn build(raw_nfs: &[Fp]) -> Self {
            Self::build_layout(raw_nfs, COMPILED_PIR_LAYOUT)
        }

        fn build_layout(raw_nfs: &[Fp], layout: PirLayout) -> Self {
            let ranges = build_ranges_with_sentinels(raw_nfs);
            let tree = pir_export::build_pir_tree(ranges.clone()).unwrap();
            let (tier0_data, tier1_data) = pir_export::export_for_layout(&tree, layout).unwrap();

            Self {
                layout,
                tier0_data,
                tier1_data,
                ranges,
                empty_hashes: tree.empty_hashes,
                circuit_root: tree.circuit_root,
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
                fix.ranges.len(),
                value,
                &fix.empty_hashes,
                fix.circuit_root,
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
            fix.ranges.len(),
            value,
            &fix.empty_hashes,
            fix.circuit_root,
        )
        .unwrap();

        assert_eq!(proof.root, fix.circuit_root);
        assert_eq!(proof.path.len(), TREE_DEPTH);
    }

    // ── process_tier0 ────────────────────────────────────────────────────

    #[test]
    fn process_tier0_fills_correct_path_region() {
        let raw_nfs: Vec<Fp> = (1u64..=30).map(|i| Fp::from(i * 1013)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let tier0 = Tier0Data::from_layout(fix.tier0_data, fix.layout).unwrap();

        let value = fix.ranges[0][0];
        let mut path = [Fp::default(); TREE_DEPTH];
        let s1 = process_tier0(&tier0, fix.layout, value, &mut path).unwrap();

        assert!(s1 < fix.layout.tier1_rows().unwrap());

        let offset = fix.layout.pir_depth - fix.layout.tier0_layers;
        let tier0_region = &path[offset..fix.layout.pir_depth];
        assert!(
            tier0_region.iter().any(|&v| v != Fp::default()),
            "tier0 should write at least one non-zero sibling"
        );

        let below = &path[..offset];
        assert!(
            below.iter().all(|&v| v == Fp::default()),
            "path below tier0 region should be untouched"
        );
    }

    #[test]
    fn process_tier0_handles_arbitrary_field_element() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let tier0 = Tier0Data::from_layout(fix.tier0_data, fix.layout).unwrap();

        // Sentinel nullifiers span the field, so every non-nullifier value
        // falls in some gap range. Verify this doesn't panic and returns a
        // valid subtree index.
        let bogus = Fp::from(u64::MAX);
        let mut path = [Fp::default(); TREE_DEPTH];
        let s1 = process_tier0(&tier0, fix.layout, bogus, &mut path).unwrap();
        assert!(s1 < fix.layout.tier1_rows().unwrap());
    }

    #[test]
    fn process_tier1_and_build_produces_verifiable_proof() {
        let raw_nfs: Vec<Fp> = (1u64..=30).map(|i| Fp::from(i * 1013)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let tier0 = Tier0Data::from_layout(fix.tier0_data.clone(), fix.layout).unwrap();
        let row_bytes = fix.layout.tier1_row_bytes().unwrap();

        let value = fix.ranges[0][0] + Fp::one();
        let mut path = [Fp::default(); TREE_DEPTH];

        let s1 = process_tier0(&tier0, fix.layout, value, &mut path).unwrap();
        let t1_offset = s1 * row_bytes;
        let proof = process_tier1_and_build(
            Tier1ProofInput {
                tier1_row: &fix.tier1_data[t1_offset..t1_offset + row_bytes],
                layout: fix.layout,
                row_idx: s1,
                num_ranges: fix.ranges.len(),
                nullifier: value,
                empty_hashes: &fix.empty_hashes,
                circuit_root: fix.circuit_root,
            },
            &mut path,
        )
        .unwrap();

        assert!(proof.verify(value));
        assert_eq!(proof.root, fix.circuit_root);
    }

    // ── valid_leaves_for_row ──────────────────────────────────────────────

    #[test]
    fn valid_leaves_for_row_basic() {
        let leaves = COMPILED_PIR_LAYOUT.tier1_leaves().unwrap();
        assert_eq!(valid_leaves_for_row(leaves, 0, leaves), leaves);
        assert_eq!(valid_leaves_for_row(leaves + 1, 0, leaves), leaves);
        assert_eq!(valid_leaves_for_row(leaves + 1, 1, leaves), 1);
        assert_eq!(valid_leaves_for_row(0, 0, leaves), 0);
        assert_eq!(valid_leaves_for_row(1, 0, leaves), 1);
        assert_eq!(valid_leaves_for_row(1, 1, leaves), 0);
    }

    // ── fetch_proof_local error paths ─────────────────────────────────────

    #[test]
    fn fetch_proof_local_rejects_truncated_tier1() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let row_bytes = fix.layout.tier1_row_bytes().unwrap();

        let result = fetch_proof_local(
            &fix.tier0_data,
            &fix.tier1_data[..row_bytes / 2],
            fix.ranges.len(),
            fix.ranges[0][0],
            &fix.empty_hashes,
            fix.circuit_root,
        );
        assert!(result.is_err());
    }

    #[test]
    fn reconstruct_verifies_for_alt_two_tier_splits() {
        let splits = [(11usize, 8usize), (12, 7), (13, 6)];
        let raw_nfs: Vec<Fp> = (1u64..=80).map(|i| Fp::from(i * 997)).collect();

        for (t0, t1) in splits {
            let layout = PirLayout {
                pir_depth: 19,
                tier0_layers: t0,
                tier1_layers: t1,
            };
            let fix = TestFixture::build_layout(&raw_nfs, layout);
            for &[nf_lo, _, _] in fix.ranges.iter().take(10) {
                let value = nf_lo + Fp::one();
                let proof = fetch_proof_local_layout(
                    &fix.tier0_data,
                    &fix.tier1_data,
                    layout,
                    fix.ranges.len(),
                    value,
                    &fix.empty_hashes,
                    fix.circuit_root,
                )
                .unwrap_or_else(|e| panic!("reconstruct {t0}+{t1} failed: {e}"));
                assert!(
                    proof.verify(value),
                    "proof should verify for {t0}+{t1} value {value:?}"
                );
            }
        }
    }

    struct MockTransport {
        gets: std::collections::HashMap<&'static str, TransportResponse>,
        posts: std::collections::HashMap<&'static str, TransportResponse>,
        hits: std::sync::Mutex<Vec<String>>,
    }

    impl MockTransport {
        fn new(tree: &pir_export::PirTree) -> Self {
            Self::new_layout(tree, COMPILED_PIR_LAYOUT)
        }

        fn new_layout(tree: &pir_export::PirTree, layout: PirLayout) -> Self {
            use ff::PrimeField as _;

            let (tier0_data, _) = pir_export::export_for_layout(tree, layout).unwrap();
            let rows = layout.tier1_rows().unwrap();
            let row_bytes = layout.tier1_row_bytes().unwrap();
            let item_bits = layout.tier1_item_bits().unwrap();
            let root_info = pir_types::RootInfo {
                zcash_network: pir_types::ZcashNetwork::Test,
                nullifier_pool: pir_types::NULLIFIER_POOL.to_owned(),
                dataset_version: pir_types::DATASET_VERSION,
                circuit_root: hex::encode(tree.circuit_root.to_repr()),
                pir_root: hex::encode(tree.pir_root.to_repr()),
                num_ranges: tree.ranges.len(),
                pir_layout: layout,
                pir_depth: layout.pir_depth,
                tier1_rows: rows,
                tier1_row_bytes: row_bytes,
                height: None,
            };
            let tier1_scenario = YpirScenario {
                num_items: rows,
                item_size_bits: item_bits,
            };

            let gets = [
                ("/tier0", response(tier0_data)),
                (
                    "/params/tier1",
                    response(serde_json::to_vec(&tier1_scenario).unwrap()),
                ),
                ("/root", response(serde_json::to_vec(&root_info).unwrap())),
            ]
            .into_iter()
            .collect();
            let posts = [("/tier1/query", response(vec![0xDE; 65536]))]
                .into_iter()
                .collect();

            Self {
                gets,
                posts,
                hits: std::sync::Mutex::new(Vec::new()),
            }
        }

        fn count_hits(&self, path: &str) -> usize {
            self.hits
                .lock()
                .unwrap()
                .iter()
                .filter(|hit| hit.as_str() == path)
                .count()
        }

        fn update_root(&mut self, update: impl FnOnce(&mut serde_json::Value)) {
            let mut root: serde_json::Value =
                serde_json::from_slice(&self.gets.get("/root").unwrap().body).unwrap();
            update(&mut root);
            self.gets
                .insert("/root", response(serde_json::to_vec(&root).unwrap()));
        }
    }

    fn response(body: Vec<u8>) -> TransportResponse {
        TransportResponse {
            status: 200,
            headers: Vec::new(),
            body,
        }
    }

    fn request_path(url: &str) -> &str {
        let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
        without_scheme
            .find('/')
            .map(|idx| &without_scheme[idx..])
            .unwrap_or("/")
    }

    async fn rejected_connect(expected_layout: PirLayout, transport: Arc<MockTransport>) -> String {
        match PirClient::with_transport(
            "https://pir.example",
            ZcashNetwork::Test,
            expected_layout,
            transport,
        )
        .await
        {
            Ok(_) => panic!("layout mismatch must be rejected"),
            Err(err) => err.to_string(),
        }
    }

    impl Transport for MockTransport {
        fn get<'a>(&'a self, url: &'a str) -> transport::TransportFuture<'a> {
            Box::pin(async move {
                let path = request_path(url);
                self.hits.lock().unwrap().push(path.to_string());
                self.gets
                    .get(path)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("unexpected GET {path}"))
            })
        }

        fn post<'a>(&'a self, url: &'a str, _body: Vec<u8>) -> transport::TransportFuture<'a> {
            Box::pin(async move {
                let path = request_path(url);
                self.hits.lock().unwrap().push(path.to_string());
                self.posts
                    .get(path)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("unexpected POST {path}"))
            })
        }
    }

    #[tokio::test]
    async fn proof_attempt_sends_exactly_one_pir_request() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();
        let transport = Arc::new(MockTransport::new(&tree));
        let client = PirClient::with_transport(
            "https://pir.example",
            ZcashNetwork::Test,
            COMPILED_PIR_LAYOUT,
            transport.clone(),
        )
        .await
        .unwrap();

        // The mock response is deliberately corrupt; request count is the
        // property under test.
        assert!(client
            .fetch_proof(tree.ranges[0][0] + Fp::one())
            .await
            .is_err());
        assert_eq!(transport.count_hits("/tier1/query"), 1);
        assert_eq!(
            transport
                .hits
                .lock()
                .unwrap()
                .iter()
                .filter(|hit| hit.ends_with("/query"))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn tier0_routing_failure_still_sends_one_pir_request_per_nullifier() {
        const K: usize = 5;

        let raw_nfs: Vec<Fp> = (1u64..=20).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();
        let mut transport = MockTransport::new(&tree);

        // Set every subtree minimum above each queried value. The metadata is
        // structurally valid, but every local Tier 0 lookup returns None.
        let tier0 = &mut transport.gets.get_mut("/tier0").unwrap().body;
        let records_base = pir_types::tier0::TIER0_INTERNAL_NODES * 32;
        let threshold = Fp::from(1_000_000u64).to_repr();
        for row in 0..COMPILED_PIR_LAYOUT.tier1_rows().unwrap() {
            let min_key_offset = records_base + row * 64 + 32;
            tier0[min_key_offset..min_key_offset + 32].copy_from_slice(&threshold);
        }

        let transport = Arc::new(transport);
        let client = PirClient::with_transport(
            "https://pir.example",
            ZcashNetwork::Test,
            COMPILED_PIR_LAYOUT,
            transport.clone(),
        )
        .await
        .unwrap();
        let values: Vec<Fp> = (1u64..=K as u64).map(Fp::from).collect();

        let err = client.fetch_proofs(&values).await.unwrap_err().to_string();
        assert!(
            err.contains("nullifier not found in any Tier 0 subtree"),
            "{err}"
        );
        assert_eq!(transport.count_hits("/tier1/query"), K);
    }

    #[tokio::test]
    async fn batch_waits_for_all_pir_requests_before_propagating_decode_error() {
        const K: usize = 5;

        let raw_nfs: Vec<Fp> = (1u64..=20).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();
        let transport = Arc::new(MockTransport::new(&tree));
        let client = PirClient::with_transport(
            "https://pir.example",
            ZcashNetwork::Test,
            COMPILED_PIR_LAYOUT,
            transport.clone(),
        )
        .await
        .unwrap();
        let values: Vec<Fp> = tree
            .ranges
            .iter()
            .take(K)
            .map(|range| range[0] + Fp::one())
            .collect();

        assert!(client.fetch_proofs(&values).await.is_err());
        assert_eq!(transport.count_hits("/tier1/query"), K);
    }

    #[test]
    fn rejects_inconsistent_or_unsupported_depth_layout_geometry() {
        let inconsistent = PirLayout {
            pir_depth: 19,
            tier0_layers: 12,
            tier1_layers: 8,
        };
        let err = validate_layout("test", inconsistent)
            .unwrap_err()
            .to_string();
        assert!(err.contains("is inconsistent"), "{err}");

        let too_deep = PirLayout {
            pir_depth: TREE_DEPTH + 1,
            tier0_layers: 15,
            tier1_layers: 15,
        };
        let err = validate_layout("test", too_deep).unwrap_err().to_string();
        assert!(err.contains("expected 1..=29"), "{err}");
    }

    #[test]
    fn rejects_layouts_outside_shared_protocol_constraints() {
        let excessive_tier0 = PirLayout {
            pir_depth: 29,
            tier0_layers: 23,
            tier1_layers: 6,
        };
        let err = validate_layout("test", excessive_tier0)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Tier 0 layers 23 exceeds maximum 16"), "{err}");

        let excessive_tier1 = PirLayout {
            pir_depth: 27,
            tier0_layers: 11,
            tier1_layers: 16,
        };
        let err = validate_layout("test", excessive_tier1)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Tier 1 layers 16 exceeds maximum 15"), "{err}");
    }

    #[test]
    fn rejects_zero_tier_and_other_unsupported_layouts() {
        let zero_tier = PirLayout {
            pir_depth: 19,
            tier0_layers: 19,
            tier1_layers: 0,
        };
        let err = validate_layout("test", zero_tier).unwrap_err().to_string();
        assert!(err.contains("tiers must be non-zero"), "{err}");

        let too_few_rows = PirLayout {
            pir_depth: 19,
            tier0_layers: 10,
            tier1_layers: 9,
        };
        let err = validate_layout("test", too_few_rows)
            .unwrap_err()
            .to_string();
        assert!(err.contains("below YPIR minimum"), "{err}");

        let too_few_item_bits = PirLayout {
            pir_depth: 18,
            tier0_layers: 13,
            tier1_layers: 5,
        };
        let err = validate_layout("test", too_few_item_bits)
            .unwrap_err()
            .to_string();
        assert!(err.contains("below YPIR minimum"), "{err}");
    }

    #[tokio::test]
    async fn rejects_unsafe_config_layout_before_network_io() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();
        let transport = Arc::new(MockTransport::new(&tree));
        let unsafe_layout = PirLayout {
            pir_depth: 29,
            tier0_layers: 23,
            tier1_layers: 6,
        };

        let err = rejected_connect(unsafe_layout, transport.clone()).await;

        assert!(err.contains("Tier 0 layers 23 exceeds maximum 16"), "{err}");
        assert!(transport.hits.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn rejects_missing_layout_metadata_without_query() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();
        let mut transport = MockTransport::new(&tree);
        transport.update_root(|root| {
            root.as_object_mut().unwrap().remove("pir_layout");
        });
        transport.gets.insert("/tier0", response(vec![0xff]));
        let transport = Arc::new(transport);

        let err = rejected_connect(COMPILED_PIR_LAYOUT, transport.clone()).await;
        assert!(err.contains("parse /root response"), "{err}");
        assert_eq!(transport.count_hits("/root"), 1);
        assert_eq!(transport.count_hits("/tier0"), 0);
        assert_eq!(transport.count_hits("/params/tier1"), 0);
        assert_eq!(transport.count_hits("/tier1/query"), 0);
    }

    #[tokio::test]
    async fn rejects_tier0_root_from_different_snapshot() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();
        let mut transport = MockTransport::new(&tree);
        transport.update_root(|root| {
            root["pir_root"] = serde_json::Value::String(hex::encode(Fp::from(0).to_repr()));
        });
        let transport = Arc::new(transport);

        let err = rejected_connect(COMPILED_PIR_LAYOUT, transport.clone()).await;
        assert!(
            err.contains("Tier 0 root does not match server pir_root"),
            "{err}"
        );
        assert_eq!(transport.count_hits("/root"), 1);
        assert_eq!(transport.count_hits("/tier0"), 1);
        assert_eq!(transport.count_hits("/tier1/query"), 0);
    }

    #[tokio::test]
    async fn rejects_config_depth_and_split_mismatches_without_query() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();
        let mismatches = [
            PirLayout {
                pir_depth: 20,
                tier0_layers: 12,
                tier1_layers: 8,
            },
            PirLayout {
                pir_depth: 19,
                tier0_layers: 11,
                tier1_layers: 8,
            },
        ];

        for expected_layout in mismatches {
            let transport = Arc::new(MockTransport::new(&tree));
            let err = rejected_connect(expected_layout, transport.clone()).await;
            assert!(err.contains("expected") && err.contains("server"), "{err}");
            assert_eq!(transport.count_hits("/root"), 1);
            assert_eq!(transport.count_hits("/tier0"), 0);
            assert_eq!(transport.count_hits("/params/tier1"), 0);
            assert_eq!(transport.count_hits("/tier1/query"), 0);
        }
    }

    #[tokio::test]
    async fn rejects_server_depth_and_split_mismatches_without_query() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();
        let mismatches = [
            PirLayout {
                pir_depth: 20,
                tier0_layers: 12,
                tier1_layers: 8,
            },
            PirLayout {
                pir_depth: 19,
                tier0_layers: 11,
                tier1_layers: 8,
            },
        ];

        for server_layout in mismatches {
            let mut transport = MockTransport::new(&tree);
            transport.update_root(|root| {
                root["pir_layout"] = serde_json::to_value(server_layout).unwrap();
            });
            let transport = Arc::new(transport);
            let err = rejected_connect(COMPILED_PIR_LAYOUT, transport.clone()).await;
            assert!(err.contains("expected") && err.contains("server"), "{err}");
            assert_eq!(transport.count_hits("/root"), 1);
            assert_eq!(transport.count_hits("/tier0"), 0);
            assert_eq!(transport.count_hits("/params/tier1"), 0);
            assert_eq!(transport.count_hits("/tier1/query"), 0);
        }
    }

    #[tokio::test]
    async fn connects_matching_alt_two_tier_splits_without_query() {
        let splits = [(11usize, 8usize), (12, 7), (13, 6)];
        let raw_nfs: Vec<Fp> = (1u64..=20).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();

        for (t0, t1) in splits {
            let layout = PirLayout {
                pir_depth: 19,
                tier0_layers: t0,
                tier1_layers: t1,
            };
            let transport = Arc::new(MockTransport::new_layout(&tree, layout));
            let client = PirClient::with_transport(
                "https://pir.example",
                ZcashNetwork::Test,
                layout,
                transport.clone(),
            )
            .await
            .unwrap_or_else(|e| panic!("connect {t0}+{t1} should succeed: {e}"));
            assert_eq!(client.layout, layout);
            assert_eq!(transport.count_hits("/tier1/query"), 0);
        }
    }

    #[tokio::test]
    async fn rejects_config_server_layout_mismatch_before_query() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();
        let server_layout = PirLayout {
            pir_depth: 19,
            tier0_layers: 11,
            tier1_layers: 8,
        };
        let expected = PirLayout {
            pir_depth: 19,
            tier0_layers: 13,
            tier1_layers: 6,
        };
        let transport = Arc::new(MockTransport::new_layout(&tree, server_layout));
        let err = rejected_connect(expected, transport.clone()).await;
        assert!(err.contains("PIR layout mismatch"), "{err}");
        assert_eq!(transport.count_hits("/root"), 1);
        assert_eq!(transport.count_hits("/tier0"), 0);
        assert_eq!(transport.count_hits("/params/tier1"), 0);
        assert_eq!(transport.count_hits("/tier1/query"), 0);
    }

    #[tokio::test]
    async fn rejects_wrong_zcash_network_before_data_download() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();
        let transport = Arc::new(MockTransport::new(&tree));

        let err = match PirClient::with_transport(
            "https://pir.example",
            ZcashNetwork::Main,
            COMPILED_PIR_LAYOUT,
            transport.clone(),
        )
        .await
        {
            Ok(_) => panic!("wrong Zcash network must be rejected"),
            Err(err) => err.to_string(),
        };
        assert!(
            err.contains("expected main") && err.contains("server advertised test"),
            "{err}"
        );
        assert_eq!(transport.count_hits("/root"), 1);
        assert_eq!(transport.count_hits("/tier0"), 0);
        assert_eq!(transport.count_hits("/params/tier1"), 0);
        assert_eq!(transport.count_hits("/tier1/query"), 0);
    }

    #[tokio::test]
    async fn rejects_wrong_nullifier_pool() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();
        let mut transport = MockTransport::new(&tree);
        let mut root: serde_json::Value =
            serde_json::from_slice(&transport.gets.get("/root").unwrap().body).unwrap();
        root["nullifier_pool"] = serde_json::Value::String("orchard".to_owned());
        transport
            .gets
            .insert("/root", response(serde_json::to_vec(&root).unwrap()));
        let transport = Arc::new(transport);

        let err = match PirClient::with_transport(
            "https://pir.example",
            ZcashNetwork::Test,
            COMPILED_PIR_LAYOUT,
            transport.clone(),
        )
        .await
        {
            Ok(_) => panic!("wrong pool must be rejected"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("orchard"), "{err}");
    }

    #[tokio::test]
    async fn rejects_mismatched_tier_shape() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();
        let mut transport = MockTransport::new(&tree);
        let mut root: serde_json::Value =
            serde_json::from_slice(&transport.gets.get("/root").unwrap().body).unwrap();
        root["tier1_row_bytes"] = serde_json::Value::from(4_096);
        transport
            .gets
            .insert("/root", response(serde_json::to_vec(&root).unwrap()));
        let transport = Arc::new(transport);

        let err = match PirClient::with_transport(
            "https://pir.example",
            ZcashNetwork::Test,
            COMPILED_PIR_LAYOUT,
            transport.clone(),
        )
        .await
        {
            Ok(_) => panic!("mismatched Tier 1 shape must be rejected"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("Tier 1 shape mismatch"), "{err}");
        assert_eq!(transport.count_hits("/tier1/query"), 0);
    }

    #[tokio::test]
    async fn rejects_version_one_root_before_shape_check() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();
        let mut transport = MockTransport::new(&tree);
        let mut root: serde_json::Value =
            serde_json::from_slice(&transport.gets.get("/root").unwrap().body).unwrap();
        root["dataset_version"] = serde_json::Value::from(1);
        root.as_object_mut().unwrap().remove("tier1_rows");
        root.as_object_mut().unwrap().remove("tier1_row_bytes");
        transport
            .gets
            .insert("/root", response(serde_json::to_vec(&root).unwrap()));

        let err = match PirClient::with_transport(
            "https://pir.example",
            ZcashNetwork::Test,
            COMPILED_PIR_LAYOUT,
            Arc::new(transport),
        )
        .await
        {
            Ok(_) => panic!("version-one root must be rejected"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("version 1 is unsupported"), "{err}");
    }
}
