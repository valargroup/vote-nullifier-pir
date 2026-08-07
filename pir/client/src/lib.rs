//! PIR client library for private Merkle path retrieval.
//!
//! Provides [`PirClient`] which connects to a `pir-server` instance and
//! retrieves circuit-ready `ImtProofData` without revealing the
//! queried nullifier to the server.

use std::{sync::Arc, time::Instant};

use anyhow::{Context, Result};
use ff::PrimeField as _;
use imt_tree::tree::{precompute_empty_hashes, TREE_DEPTH};
use pasta_curves::Fp;
// Re-exported so downstream crates (e.g. zcash_voting) can reference the type
// returned by PirClientBlocking::fetch_proof without a direct imt-tree dependency.
pub use imt_tree::ImtProofData;

mod reconstruct;
mod transport;
pub use transport::{Transport, TransportFuture, TransportResponse};

use pir_types::tier0::Tier0Data;
use pir_types::{
    current_layout, encrypted_tier_count, serialize_ypir_query, validate_layout, LayoutBounds,
    PirLayout, RootInfo, TierTransport, YpirScenario, CIRCUIT_HEIGHT, PIR_DEPTH,
};

use ypir::client::YPIRClient;

// ── Timing breakdown ─────────────────────────────────────────────────────────

/// Per-tier timing breakdown for a single YPIR query, measuring each stage
/// of the client-server round trip.
#[derive(Clone)]
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

/// Per-note timing breakdown for encrypted YPIR queries.
pub struct NoteTiming {
    /// First encrypted tier timing (legacy field used by load/bench tooling).
    pub tier1: TierTiming,
    /// Timings for every encrypted tier in layout order (includes `tier1`).
    pub tiers: Vec<TierTiming>,
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
    tier0: Tier0Data,
    layout: PirLayout,
    /// YPIR scenarios for each encrypted tier (layout ordinals 1..).
    encrypted_scenarios: Vec<YpirScenario>,
    num_ranges: usize,
    empty_hashes: [Fp; TREE_DEPTH],
    circuits_root: Fp,
    zcash_network: pir_types::ZcashNetwork,
    height: Option<u64>,
}

impl PirClient {
    /// Connect using a caller-provided HTTP transport.
    pub async fn with_transport(server_url: &str, transport: Arc<dyn Transport>) -> Result<Self> {
        let base = server_url.trim_end_matches('/');

        // Download Tier 0 data, first-tier YPIR params, and root concurrently.
        // Additional encrypted-tier params are fetched after layout negotiation.
        let t0 = Instant::now();
        let tier0_url = format!("{base}/tier0");
        let tier1_url = format!("{base}/params/tier1");
        let root_url = format!("{base}/root");
        let (tier0_resp, tier1_resp, root_resp) = tokio::try_join!(
            transport.get(&tier0_url),
            transport.get(&tier1_url),
            transport.get(&root_url),
        )
        .map_err(|e| anyhow::anyhow!("connect fetch failed: {e}"))?;

        let root_info: RootInfo =
            serde_json::from_slice(&body_for_status(root_resp, "GET /root failed")?)
                .context("parse /root response")?;
        anyhow::ensure!(
            pir_types::is_current_dataset(&root_info.nullifier_pool, root_info.dataset_version),
            "server nullifier dataset {:?} version {} is unsupported; expected {:?} version {}",
            root_info.nullifier_pool,
            root_info.dataset_version,
            pir_types::NULLIFIER_POOL,
            pir_types::DATASET_VERSION
        );
        anyhow::ensure!(
            root_info.pir_depth == PIR_DEPTH,
            "server pir_depth {} != expected {}",
            root_info.pir_depth,
            PIR_DEPTH
        );

        let layout = root_info.layout.clone();
        validate_layout(&layout, &LayoutBounds::default()).map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            layout.circuit_height == CIRCUIT_HEIGHT && layout.pir_height == root_info.pir_depth,
            "layout heights disagree with /root"
        );

        let tier0_desc = &layout.tiers[0];
        let tier0_bytes = body_for_status(tier0_resp, "GET /tier0 failed")?;
        log::debug!(
            "Downloaded Tier 0: {} bytes in {:.1}s (layout {} encrypted tiers)",
            tier0_bytes.len(),
            t0.elapsed().as_secs_f64(),
            encrypted_tier_count(&layout)
        );
        anyhow::ensure!(
            tier0_bytes.len() == tier0_desc.payload_bytes,
            "Tier 0 size {} != layout payload_bytes {}",
            tier0_bytes.len(),
            tier0_desc.payload_bytes
        );
        let tier0 = Tier0Data::from_bytes_layout(
            tier0_bytes.to_vec(),
            tier0_desc.layers,
            tier0_desc.records_per_row,
        )?;

        let tier1_scenario: YpirScenario =
            serde_json::from_slice(&body_for_status(tier1_resp, "GET /params/tier1 failed")?)
                .context("parse /params/tier1 response")?;

        // Collect YPIR scenarios: prefer server /params/tierN, fall back to layout.
        let mut encrypted_scenarios = Vec::new();
        for (i, tier) in layout.tiers.iter().enumerate().skip(1) {
            anyhow::ensure!(
                matches!(tier.transport, TierTransport::YpirSimplepirV1),
                "tier {i} must be encrypted"
            );
            let scenario = if i == 1 {
                tier1_scenario.clone()
            } else {
                let url = format!("{base}/params/tier{i}");
                match transport.get(&url).await {
                    Ok(resp) if is_success(resp.status) => {
                        serde_json::from_slice(&resp.body).context("parse extra tier params")?
                    }
                    _ => tier
                        .pir
                        .clone()
                        .context("missing pir params for encrypted tier")?,
                }
            };
            let expected = tier.pir.as_ref().context("layout missing pir params")?;
            anyhow::ensure!(
                scenario.num_items == expected.num_items
                    && scenario.item_size_bits == expected.item_size_bits,
                "tier {i} params mismatch: server {:?}, layout {:?}",
                scenario,
                expected
            );
            encrypted_scenarios.push(scenario);
        }

        // Legacy shape fields must agree with the negotiated first encrypted tier.
        let first = &layout.tiers[1];
        let expected_rows = first
            .pir
            .as_ref()
            .map(|p| p.num_items)
            .unwrap_or(first.logical_rows);
        if root_info.tier1_rows != 0 {
            anyhow::ensure!(
                root_info.tier1_rows == expected_rows || root_info.tier1_rows == first.logical_rows,
                "server Tier 1 rows {} disagree with layout (logical={}, ypir={})",
                root_info.tier1_rows,
                first.logical_rows,
                expected_rows
            );
        }
        if root_info.tier1_row_bytes != 0 {
            anyhow::ensure!(
                root_info.tier1_row_bytes == first.payload_bytes,
                "server Tier 1 shape mismatch: /root reports {} row bytes; layout expects {}",
                root_info.tier1_row_bytes,
                first.payload_bytes
            );
        }

        let circuits_root_bytes = hex::decode(&root_info.circuits_root)?;
        anyhow::ensure!(
            circuits_root_bytes.len() == 32,
            "circuits_root hex decoded to {} bytes, expected 32",
            circuits_root_bytes.len()
        );
        let mut circuits_root_arr = [0u8; 32];
        circuits_root_arr.copy_from_slice(&circuits_root_bytes);
        let circuits_root = Option::from(Fp::from_repr(circuits_root_arr))
            .ok_or_else(|| anyhow::anyhow!("invalid circuits_root field element"))?;

        let empty_hashes = precompute_empty_hashes();

        Ok(Self {
            server_url: base.to_string(),
            transport,
            tier0,
            layout,
            encrypted_scenarios,
            num_ranges: root_info.num_ranges,
            empty_hashes,
            circuits_root,
            zcash_network: root_info.zcash_network,
            height: root_info.height,
        })
    }

    /// Negotiated layout accepted at connect time.
    pub fn layout(&self) -> &PirLayout {
        &self.layout
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
    /// **Error-oracle mitigation**: every advertised encrypted query is
    /// always sent, even when an earlier stage fails — a Tier 0 lookup miss
    /// or panic, a tier that fails to decode/process, or an out-of-range row
    /// index all latch the error and query dummy row 0 for the remaining
    /// tiers. The server therefore always observes the advertised request
    /// cardinality regardless of where (or whether) reconstruction failed.
    async fn fetch_proof_inner(&self, nullifier: Fp) -> Result<(ImtProofData, NoteTiming)> {
        let note_start = Instant::now();
        let mut path = [Fp::default(); TREE_DEPTH];
        let mut early_err: Option<anyhow::Error> = None;

        // Tier 0 is server-supplied plaintext, and `find_subtree` failure is
        // nullifier-dependent: a malicious server could craft boundary keys
        // with a gap over a chosen nullifier range and use "client fetched
        // /tier0 but sent no queries" as a range oracle. Latch the error
        // instead of returning so every advertised encrypted query below is
        // still sent (dummy row 0) before the error surfaces.
        let mut row_idx = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            reconstruct::process_plaintext_tier0(&self.tier0, &self.layout, nullifier, &mut path)
        })) {
            Ok(Ok(idx)) => idx,
            Ok(Err(e)) => {
                early_err = Some(e);
                0
            }
            Err(panic_payload) => {
                early_err = Some(anyhow::anyhow!(
                    "tier0 processing panicked: {}",
                    panic_message(&panic_payload)
                ));
                0
            }
        };

        let enc_tiers: Vec<_> = self.layout.tiers.iter().enumerate().skip(1).collect();
        anyhow::ensure!(
            enc_tiers.len() == self.encrypted_scenarios.len(),
            "encrypted tier/scenario count mismatch"
        );

        let mut timings: Vec<TierTiming> = Vec::with_capacity(enc_tiers.len());
        let mut terminal_row: Option<Vec<u8>> = None;

        for (enc_i, (tier_index, tier)) in enc_tiers.iter().enumerate() {
            let scenario = &self.encrypted_scenarios[enc_i];
            let query_idx = if early_err.is_some() || row_idx >= scenario.num_items {
                if early_err.is_none() && row_idx >= scenario.num_items {
                    early_err = Some(anyhow::anyhow!(
                        "tier{} row_idx {} >= num_items {}",
                        tier_index,
                        row_idx,
                        scenario.num_items
                    ));
                }
                0
            } else {
                row_idx
            };

            let tier_name = format!("tier{tier_index}");
            let query_result = self
                .ypir_query(scenario, &tier_name, query_idx, tier.payload_bytes)
                .await;

            match query_result {
                Ok((row, timing)) => {
                    timings.push(timing);
                    if early_err.is_some() {
                        continue;
                    }
                    let is_last = enc_i + 1 == enc_tiers.len();
                    if is_last {
                        terminal_row = Some(row);
                    } else {
                        // Reconstruction runs on server-controlled row bytes;
                        // catch panics so a hostile row cannot unwind past the
                        // loop and suppress the remaining advertised queries.
                        let boundary =
                            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                reconstruct::process_boundary_tier(
                                    &row,
                                    &self.layout,
                                    *tier_index,
                                    row_idx,
                                    nullifier,
                                    &mut path,
                                )
                            }))
                            .unwrap_or_else(|panic_payload| {
                                Err(anyhow::anyhow!(
                                    "tier{} boundary reconstruction panicked: {}",
                                    tier_index,
                                    panic_message(&panic_payload)
                                ))
                            });
                        match boundary {
                            Ok(next) => row_idx = next,
                            Err(e) => early_err = Some(e),
                        }
                    }
                }
                Err(e) => {
                    // Request was attempted; do not retry this ordinal. Continue
                    // so remaining advertised tiers still observe a query.
                    early_err = Some(early_err.unwrap_or(e));
                }
            }
        }

        if let Some(err) = early_err {
            return Err(err);
        }
        anyhow::ensure!(
            !timings.is_empty(),
            "layout must advertise at least one encrypted tier"
        );
        let last_row = terminal_row.context("missing terminal row")?;
        let last_tier_index = enc_tiers.last().map(|(i, _)| *i).unwrap();
        let proof = reconstruct::process_terminal_tier(
            &last_row,
            &self.layout,
            last_tier_index,
            row_idx,
            self.num_ranges,
            nullifier,
            &mut path,
            &self.empty_hashes,
            self.circuits_root,
        )?;

        let total_ms = note_start.elapsed().as_secs_f64() * 1000.0;
        Ok((
            proof,
            NoteTiming {
                tier1: timings[0].clone(),
                tiers: timings,
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
            anyhow::anyhow!(
                "{} response decryption panicked: {}",
                tier_name,
                panic_message(&panic_payload)
            )
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

/// Extract a human-readable message from a `catch_unwind` panic payload.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> &str {
    payload
        .downcast_ref::<String>()
        .map(|s| s.as_str())
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("unknown panic")
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
    /// Connect to a PIR server with a caller-provided HTTP transport.
    pub fn with_transport(server_url: &str, transport: Arc<dyn Transport>) -> Result<Self> {
        let rt = tokio::runtime::Runtime::new()?;
        let inner = rt.block_on(PirClient::with_transport(server_url, transport))?;
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

    /// The depth-29 root (PIR depth 19 padded to tree depth 29).
    pub fn circuits_root(&self) -> Fp {
        self.inner.circuits_root
    }

    /// Negotiated layout accepted at connect time.
    pub fn layout(&self) -> &PirLayout {
        &self.inner.layout
    }

    /// Server-reported Zcash network.
    pub fn zcash_network(&self) -> pir_types::ZcashNetwork {
        self.inner.zcash_network
    }

    /// Server-reported snapshot height, if any.
    pub fn height(&self) -> Option<u64> {
        self.inner.height
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
    circuits_root: Fp,
) -> Result<ImtProofData> {
    let layout = current_layout("local");
    fetch_proof_local_with_layout(
        &layout,
        tier0_data,
        &[tier1_data],
        num_ranges,
        nullifier,
        empty_hashes,
        circuits_root,
    )
}

/// Local reconstruction against an arbitrary negotiated layout.
///
/// `encrypted_tier_dbs[i]` is the concatenated logical-row database for
/// layout tier `i + 1` (YPIR padding rows are not required in local mode).
pub fn fetch_proof_local_with_layout(
    layout: &PirLayout,
    tier0_data: &[u8],
    encrypted_tier_dbs: &[&[u8]],
    num_ranges: usize,
    nullifier: Fp,
    empty_hashes: &[Fp; TREE_DEPTH],
    circuits_root: Fp,
) -> Result<ImtProofData> {
    validate_layout(layout, &LayoutBounds::default()).map_err(anyhow::Error::msg)?;
    anyhow::ensure!(
        encrypted_tier_dbs.len() + 1 == layout.tiers.len(),
        "encrypted db count mismatch"
    );

    let t0 = &layout.tiers[0];
    let tier0 = Tier0Data::from_bytes_layout(tier0_data.to_vec(), t0.layers, t0.records_per_row)?;
    let mut path = [Fp::default(); TREE_DEPTH];
    let mut row_idx = reconstruct::process_plaintext_tier0(&tier0, layout, nullifier, &mut path)?;

    let mut selected: Vec<Vec<u8>> = Vec::with_capacity(encrypted_tier_dbs.len());
    for (enc_i, db) in encrypted_tier_dbs.iter().enumerate() {
        let tier_index = enc_i + 1;
        let tier = &layout.tiers[tier_index];
        let offset = row_idx
            .checked_mul(tier.payload_bytes)
            .context("row offset overflow")?;
        anyhow::ensure!(
            offset + tier.payload_bytes <= db.len(),
            "tier{tier_index} data too short: need {} bytes at offset {offset}, have {}",
            tier.payload_bytes,
            db.len()
        );
        let row = db[offset..offset + tier.payload_bytes].to_vec();
        let is_last = tier_index + 1 == layout.tiers.len();
        if !is_last {
            row_idx = reconstruct::process_boundary_tier(
                &row, layout, tier_index, row_idx, nullifier, &mut path,
            )?;
        }
        selected.push(row);
    }

    let last = selected.last().context("missing terminal row")?;
    let last_index = layout.tiers.len() - 1;
    reconstruct::process_terminal_tier(
        last,
        layout,
        last_index,
        row_idx,
        num_ranges,
        nullifier,
        &mut path,
        empty_hashes,
        circuits_root,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff::Field;
    use pasta_curves::Fp;
    use pir_export::build_ranges_with_sentinels;
    use pir_types::{derive_snapshot_id, TIER1_ROWS, TIER1_ROW_BYTES};

    /// Build a tree and export both tier blobs.
    struct TestFixture {
        tier0_data: Vec<u8>,
        tier1_data: Vec<u8>,
        ranges: Vec<[Fp; 3]>,
        empty_hashes: [Fp; TREE_DEPTH],
        circuits_root: Fp,
    }

    impl TestFixture {
        fn build(raw_nfs: &[Fp]) -> Self {
            let ranges = build_ranges_with_sentinels(raw_nfs);
            let tree = pir_export::build_pir_tree(ranges.clone()).unwrap();

            let tier0_data = pir_export::tier0::export(
                &tree.pir_root,
                &tree.levels,
                &tree.ranges,
                &tree.empty_hashes,
            );
            let mut tier1_data = Vec::new();
            pir_export::tier1::export(&tree.ranges, &mut tier1_data).unwrap();

            Self {
                tier0_data,
                tier1_data,
                ranges,
                empty_hashes: tree.empty_hashes,
                circuits_root: tree.circuits_root,
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
                fix.circuits_root,
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
            fix.circuits_root,
        )
        .unwrap();

        assert_eq!(proof.root, fix.circuits_root);
        assert_eq!(proof.path.len(), TREE_DEPTH);
    }

    // ── layout-driven reconstruction ─────────────────────────────────────

    #[test]
    fn process_plaintext_tier0_fills_correct_path_region() {
        let raw_nfs: Vec<Fp> = (1u64..=30).map(|i| Fp::from(i * 1013)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let layout = current_layout("test");
        let tier0 = Tier0Data::from_bytes(fix.tier0_data).unwrap();

        let value = fix.ranges[0][0];
        let mut path = [Fp::default(); TREE_DEPTH];
        let s1 = reconstruct::process_plaintext_tier0(&tier0, &layout, value, &mut path).unwrap();

        assert!(s1 < pir_types::TIER1_ROWS);

        let tier0_region = &path[PIR_DEPTH - pir_types::TIER0_LAYERS..PIR_DEPTH];
        assert!(
            tier0_region.iter().any(|&v| v != Fp::default()),
            "tier0 should write at least one non-zero sibling"
        );

        let below = &path[..PIR_DEPTH - pir_types::TIER0_LAYERS];
        assert!(
            below.iter().all(|&v| v == Fp::default()),
            "path below tier0 region should be untouched"
        );
    }

    #[test]
    fn twelve_seven_and_twelve_three_four_proofs_match() {
        let raw_nfs: Vec<Fp> = (1u64..=80).map(|i| Fp::from(i * 997)).collect();
        let ranges = build_ranges_with_sentinels(&raw_nfs);
        let tree = pir_export::build_pir_tree(ranges).unwrap();
        let one = pir_export::layout_export::export_for_splits(&tree, &[12, 7], "one").unwrap();
        let two = pir_export::layout_export::export_for_splits(&tree, &[12, 3, 4], "two").unwrap();
        let value = tree.ranges[0][0] + Fp::one();

        let proof_one = fetch_proof_local_with_layout(
            &one.layout,
            &one.tier0,
            &one.encrypted_tiers
                .iter()
                .map(|t| t.as_slice())
                .collect::<Vec<_>>(),
            tree.ranges.len(),
            value,
            &tree.empty_hashes,
            tree.circuits_root,
        )
        .unwrap();
        let proof_two = fetch_proof_local_with_layout(
            &two.layout,
            &two.tier0,
            &two.encrypted_tiers
                .iter()
                .map(|t| t.as_slice())
                .collect::<Vec<_>>(),
            tree.ranges.len(),
            value,
            &tree.empty_hashes,
            tree.circuits_root,
        )
        .unwrap();

        assert!(proof_one.verify(value));
        assert!(proof_two.verify(value));
        assert_eq!(proof_one.root, proof_two.root);
        assert_eq!(proof_one.nf_bounds, proof_two.nf_bounds);
        assert_eq!(proof_one.leaf_pos, proof_two.leaf_pos);
        assert_eq!(proof_one.path, proof_two.path);
    }

    #[test]
    fn height_bump_keeps_same_client_layout_contract() {
        // Two snapshots at different heights share the same negotiated split;
        // only roots/range counts change.
        let a = TestFixture::build(&(1u64..=20).map(|i| Fp::from(i * 3)).collect::<Vec<_>>());
        let b = TestFixture::build(&(1u64..=40).map(|i| Fp::from(i * 5)).collect::<Vec<_>>());
        let layout_a = current_layout("height-a");
        let layout_b = current_layout("height-b");
        assert_eq!(layout_a.pir_height, layout_b.pir_height);
        assert_eq!(
            layout_a.tiers.iter().map(|t| t.layers).collect::<Vec<_>>(),
            layout_b.tiers.iter().map(|t| t.layers).collect::<Vec<_>>()
        );
        assert_ne!(a.circuits_root, b.circuits_root);
    }

    #[test]
    fn valid_leaves_for_row_basic() {
        use pir_types::TIER1_LEAVES;
        use reconstruct::valid_leaves_for_row;
        assert_eq!(
            valid_leaves_for_row(TIER1_LEAVES, 0, TIER1_LEAVES),
            TIER1_LEAVES
        );
        assert_eq!(
            valid_leaves_for_row(TIER1_LEAVES + 1, 0, TIER1_LEAVES),
            TIER1_LEAVES
        );
        assert_eq!(valid_leaves_for_row(TIER1_LEAVES + 1, 1, TIER1_LEAVES), 1);
        assert_eq!(valid_leaves_for_row(0, 0, TIER1_LEAVES), 0);
        assert_eq!(valid_leaves_for_row(1, 0, TIER1_LEAVES), 1);
        assert_eq!(valid_leaves_for_row(1, 1, TIER1_LEAVES), 0);
    }

    // ── fetch_proof_local error paths ─────────────────────────────────────

    #[test]
    fn fetch_proof_local_rejects_truncated_tier1() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build(&raw_nfs);

        let result = fetch_proof_local(
            &fix.tier0_data,
            &fix.tier1_data[..TIER1_ROW_BYTES / 2],
            fix.ranges.len(),
            fix.ranges[0][0],
            &fix.empty_hashes,
            fix.circuits_root,
        );
        assert!(result.is_err());
    }

    struct MockTransport {
        gets: std::collections::HashMap<&'static str, TransportResponse>,
        posts: std::collections::HashMap<&'static str, TransportResponse>,
        hits: std::sync::Mutex<Vec<String>>,
    }

    impl MockTransport {
        fn new(tree: &pir_export::PirTree) -> Self {
            use ff::PrimeField as _;
            use pir_types::TIER1_ITEM_BITS;

            let tier0_data = pir_export::tier0::export(
                &tree.pir_root,
                &tree.levels,
                &tree.ranges,
                &tree.empty_hashes,
            );
            let layout = current_layout(derive_snapshot_id(
                "test",
                None,
                &hex::encode(tree.pir_root.to_repr()),
                &hex::encode(tree.circuits_root.to_repr()),
            ));
            let root_info = pir_types::RootInfo {
                zcash_network: pir_types::ZcashNetwork::Test,
                nullifier_pool: pir_types::NULLIFIER_POOL.to_owned(),
                dataset_version: pir_types::DATASET_VERSION,
                circuits_root: hex::encode(tree.circuits_root.to_repr()),
                pir_root: hex::encode(tree.pir_root.to_repr()),
                num_ranges: tree.ranges.len(),
                pir_depth: PIR_DEPTH,
                tier1_rows: TIER1_ROWS,
                tier1_row_bytes: TIER1_ROW_BYTES,
                height: None,
                layout,
            };
            let tier1_scenario = YpirScenario {
                num_items: TIER1_ROWS,
                item_size_bits: TIER1_ITEM_BITS,
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
        let client = PirClient::with_transport("https://pir.example", transport.clone())
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
    async fn tier0_gap_forgery_still_sends_all_pir_queries() {
        use ff::PrimeField as _;

        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();
        let mut transport = MockTransport::new(&tree);

        // Forge Tier 0 the way a malicious server would: raise every boundary
        // min_key above the target nullifier so find_subtree returns None.
        let layout = current_layout("forged");
        let t0 = &layout.tiers[0];
        let mut tier0_data = transport.gets.get("/tier0").unwrap().body.clone();
        let base = (t0.records_per_row - 1) * 32;
        let big_key = Fp::from(u64::MAX).to_repr();
        for rec in 0..t0.records_per_row {
            let key_off = base + rec * 64 + 32;
            tier0_data[key_off..key_off + 32].copy_from_slice(big_key.as_ref());
        }
        transport.gets.insert("/tier0", response(tier0_data));

        let transport = Arc::new(transport);
        let client = PirClient::with_transport("https://pir.example", transport.clone())
            .await
            .unwrap();

        let err = client
            .fetch_proof(tree.ranges[0][0] + Fp::one())
            .await
            .expect_err("forged tier0 must fail reconstruction");
        assert!(
            err.to_string().contains("Tier 0"),
            "error should surface the tier0 lookup miss: {err}"
        );
        // The oracle-closing property: the encrypted query was still sent.
        assert_eq!(transport.count_hits("/tier1/query"), 1);
    }

    #[tokio::test]
    async fn batch_waits_for_all_pir_requests_before_propagating_decode_error() {
        const K: usize = 5;

        let raw_nfs: Vec<Fp> = (1u64..=20).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();
        let transport = Arc::new(MockTransport::new(&tree));
        let client = PirClient::with_transport("https://pir.example", transport.clone())
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

    #[tokio::test]
    async fn multi_tier_layout_sends_two_pir_requests_including_after_error() {
        use pir_types::layout_from_splits;

        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let tree = pir_export::build_pir_tree(build_ranges_with_sentinels(&raw_nfs)).unwrap();
        let layout = layout_from_splits("multi", PIR_DEPTH, CIRCUIT_HEIGHT, &[12, 3, 4]).unwrap();
        let mut transport = MockTransport::new(&tree);

        let mut root: serde_json::Value =
            serde_json::from_slice(&transport.gets.get("/root").unwrap().body).unwrap();
        root["layout"] = serde_json::to_value(&layout).unwrap();
        // First encrypted tier is now 3-layer boundary with padded YPIR rows.
        let t1 = &layout.tiers[1];
        let t2 = &layout.tiers[2];
        root["tier1_rows"] = serde_json::json!(t1.pir.as_ref().unwrap().num_items);
        root["tier1_row_bytes"] = serde_json::json!(t1.payload_bytes);
        transport
            .gets
            .insert("/root", response(serde_json::to_vec(&root).unwrap()));
        transport.gets.insert(
            "/params/tier1",
            response(serde_json::to_vec(t1.pir.as_ref().unwrap()).unwrap()),
        );
        // Keep static str keys by using leaked strings for dynamic paths...
        // MockTransport uses &'static str keys; rebuild with owned map instead.
        let transport = MultiTierMockTransport {
            gets: {
                let mut m = std::collections::HashMap::new();
                m.insert(
                    "/tier0".to_string(),
                    transport.gets.remove("/tier0").unwrap(),
                );
                m.insert("/root".to_string(), transport.gets.remove("/root").unwrap());
                m.insert(
                    "/params/tier1".to_string(),
                    response(serde_json::to_vec(t1.pir.as_ref().unwrap()).unwrap()),
                );
                m.insert(
                    "/params/tier2".to_string(),
                    response(serde_json::to_vec(t2.pir.as_ref().unwrap()).unwrap()),
                );
                m
            },
            posts: {
                let mut m = std::collections::HashMap::new();
                m.insert("/tier1/query".to_string(), response(vec![0xDE; 65536]));
                m.insert("/tier2/query".to_string(), response(vec![0xDE; 65536]));
                m
            },
            hits: std::sync::Mutex::new(Vec::new()),
        };
        let transport = Arc::new(transport);
        let client = PirClient::with_transport("https://pir.example", transport.clone())
            .await
            .unwrap();
        assert_eq!(encrypted_tier_count(client.layout()), 2);
        assert!(client
            .fetch_proof(tree.ranges[0][0] + Fp::one())
            .await
            .is_err());
        assert_eq!(transport.count_hits("/tier1/query"), 1);
        assert_eq!(transport.count_hits("/tier2/query"), 1);
    }

    struct MultiTierMockTransport {
        gets: std::collections::HashMap<String, TransportResponse>,
        posts: std::collections::HashMap<String, TransportResponse>,
        hits: std::sync::Mutex<Vec<String>>,
    }

    impl MultiTierMockTransport {
        fn count_hits(&self, path: &str) -> usize {
            self.hits
                .lock()
                .unwrap()
                .iter()
                .filter(|hit| hit.as_str() == path)
                .count()
        }
    }

    impl Transport for MultiTierMockTransport {
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

        let err = match PirClient::with_transport("https://pir.example", Arc::new(transport)).await
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

        let err = match PirClient::with_transport("https://pir.example", Arc::new(transport)).await
        {
            Ok(_) => panic!("mismatched Tier 1 shape must be rejected"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("Tier 1 shape mismatch"), "{err}");
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

        let err = match PirClient::with_transport("https://pir.example", Arc::new(transport)).await
        {
            Ok(_) => panic!("version-one root must be rejected"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("version 1 is unsupported"), "{err}");
    }
}
