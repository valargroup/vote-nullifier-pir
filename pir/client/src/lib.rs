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
pub use pir_types::{PirLayout, COMPILED_PIR_LAYOUT};
pub use transport::{Transport, TransportFuture, TransportResponse};

use pir_types::tier0::Tier0Data;
use pir_types::{serialize_ypir_query, RootInfo, YpirScenario};

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

/// Per-note timing breakdown for the note's YPIR queries.
pub struct NoteTiming {
    pub tier1: TierTiming,
    /// Tier 2 timing; present only for three-tier layouts.
    pub tier2: Option<TierTiming>,
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
    tier1_payload_bytes: usize,
    tier1_scenario: YpirScenario,
    /// Present iff `layout.tier2_enabled()`.
    tier2_payload_bytes: Option<usize>,
    tier2_scenario: Option<YpirScenario>,
    num_ranges: usize,
    empty_hashes: [Fp; TREE_DEPTH],
    root29: Fp,
}

/// Validate a layout against the supported geometry envelope, labelling the
/// source of the layout (`expected` config vs `server`) in the error.
fn validate_layout(label: &str, layout: PirLayout) -> Result<()> {
    layout
        .validate_split()
        .map_err(|e| anyhow::anyhow!("{label} {e}"))?;
    anyhow::ensure!(
        layout.pir_depth <= TREE_DEPTH,
        "{label} PIR layout depth {} exceeds circuit depth {}",
        layout.pir_depth,
        TREE_DEPTH
    );
    layout
        .validate_ypir_bounds()
        .map_err(|e| anyhow::anyhow!("{label} {e}"))?;
    Ok(())
}

/// Convert a caught panic into a fixed-format error.
///
/// The panic payload is server-influenced (it comes from parsing or decoding
/// hostile bytes), so it is logged at debug level only and never embedded in
/// the returned error string.
fn panic_to_error(tier_name: &str, payload: Box<dyn std::any::Any + Send>) -> anyhow::Error {
    let msg = payload
        .downcast_ref::<String>()
        .map(|s| s.as_str())
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("unknown panic");
    log::debug!(
        "[PIR] {tier_name} processing panicked: {}",
        truncate_ascii(msg)
    );
    anyhow::anyhow!("{tier_name} row processing panicked")
}

/// Truncate and ASCII-escape server-influenced text for debug logging.
fn truncate_ascii(s: &str) -> String {
    s.chars()
        .take(256)
        .flat_map(|c| c.escape_default())
        .collect()
}

impl PirClient {
    /// Connect using a caller-provided HTTP transport and configuration layout.
    ///
    /// Fail-closed: every layout, dataset, and geometry check below completes
    /// before Tier 0 bytes are parsed, and a constructed client is required
    /// before any private query can be issued.
    pub async fn with_transport(
        server_url: &str,
        expected_layout: PirLayout,
        transport: Arc<dyn Transport>,
    ) -> Result<Self> {
        let base = server_url.trim_end_matches('/');

        // Validate the configured layout before any network I/O; the connect
        // fetch pattern below is then a function of local config only, never
        // of server responses.
        validate_layout("expected", expected_layout)?;
        let derived_tier1 = expected_layout
            .tier1_scenario()
            .map_err(|e| anyhow::anyhow!("expected {e}"))?;
        let tier1_payload_bytes = expected_layout
            .tier1_row_bytes()
            .map_err(|e| anyhow::anyhow!("expected {e}"))?;
        let derived_tier2 = expected_layout
            .tier2_scenario()
            .map_err(|e| anyhow::anyhow!("expected {e}"))?;
        let tier2_payload_bytes = expected_layout
            .tier2_row_bytes()
            .map_err(|e| anyhow::anyhow!("expected {e}"))?;

        // Download Tier 0 data, YPIR params, and root concurrently.
        let t0 = Instant::now();
        let tier0_url = format!("{base}/tier0");
        let tier1_url = format!("{base}/params/tier1");
        let root_url = format!("{base}/root");
        let (tier0_resp, tier1_resp, root_resp, tier2_resp) = if expected_layout.tier2_enabled() {
            let tier2_url = format!("{base}/params/tier2");
            let (a, b, c, d) = tokio::try_join!(
                transport.get(&tier0_url),
                transport.get(&tier1_url),
                transport.get(&root_url),
                transport.get(&tier2_url),
            )
            .map_err(|e| anyhow::anyhow!("connect fetch failed: {e}"))?;
            (a, b, c, Some(d))
        } else {
            let (a, b, c) = tokio::try_join!(
                transport.get(&tier0_url),
                transport.get(&tier1_url),
                transport.get(&root_url),
            )
            .map_err(|e| anyhow::anyhow!("connect fetch failed: {e}"))?;
            (a, b, c, None)
        };

        let tier1_scenario: YpirScenario =
            serde_json::from_slice(&body_for_status(tier1_resp, "GET /params/tier1 failed")?)
                .context("parse /params/tier1 response")?;

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

        // Geometry cross-checks: the layout-derived scenario is the single
        // source of truth; /root's tier fields and the /params endpoints are
        // confirmations, never sources.
        anyhow::ensure!(
            root_info.tier1_rows == derived_tier1.num_items
                && root_info.tier1_row_bytes == derived_tier1.item_size_bits / 8
                && tier1_scenario.num_items == derived_tier1.num_items
                && tier1_scenario.item_size_bits == derived_tier1.item_size_bits,
            "server Tier 1 shape mismatch: layout implies {} rows x {} bytes, /root reports {}x{} \
             bytes, and /params reports {} items x {} bits",
            derived_tier1.num_items,
            derived_tier1.item_size_bits / 8,
            root_info.tier1_rows,
            root_info.tier1_row_bytes,
            tier1_scenario.num_items,
            tier1_scenario.item_size_bits
        );

        let tier2_scenario = match (&derived_tier2, tier2_resp) {
            (Some(derived_tier2), Some(resp)) => {
                let served: YpirScenario =
                    serde_json::from_slice(&body_for_status(resp, "GET /params/tier2 failed")?)
                        .context("parse /params/tier2 response")?;
                anyhow::ensure!(
                    root_info.tier2_rows == derived_tier2.num_items
                        && root_info.tier2_row_bytes == derived_tier2.item_size_bits / 8
                        && served.num_items == derived_tier2.num_items
                        && served.item_size_bits == derived_tier2.item_size_bits,
                    "server Tier 2 shape mismatch: layout implies {} rows x {} bytes, /root \
                     reports {}x{} bytes, and /params reports {} items x {} bits",
                    derived_tier2.num_items,
                    derived_tier2.item_size_bits / 8,
                    root_info.tier2_rows,
                    root_info.tier2_row_bytes,
                    served.num_items,
                    served.item_size_bits
                );
                Some(served)
            }
            (None, _) => {
                anyhow::ensure!(
                    root_info.tier2_rows == 0 && root_info.tier2_row_bytes == 0,
                    "server advertises Tier 2 shape {}x{} bytes under a two-tier layout",
                    root_info.tier2_rows,
                    root_info.tier2_row_bytes
                );
                None
            }
            (Some(_), None) => unreachable!("tier2 params fetched iff layout enables tier2"),
        };

        let tier0_bytes = body_for_status(tier0_resp, "GET /tier0 failed")?;
        log::debug!(
            "Downloaded Tier 0: {} bytes in {:.1}s",
            tier0_bytes.len(),
            t0.elapsed().as_secs_f64()
        );
        let tier0 = Tier0Data::from_layout(tier0_bytes.to_vec(), expected_layout)?;

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
            transport,
            layout: expected_layout,
            tier0,
            tier1_payload_bytes,
            tier1_scenario,
            tier2_payload_bytes,
            tier2_scenario,
            num_ranges: root_info.num_ranges,
            empty_hashes,
            root29,
        })
    }

    /// The negotiated layout this client follows.
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
    /// Oracle invariant: for a fixed layout, every proof attempt produces an
    /// identical request trace — the advertised number of encrypted queries
    /// (1 or 2), in fixed order, with row-independent payload sizes —
    /// regardless of which nullifier is queried and regardless of any failure
    /// the server can induce (crafted Tier 0 gaps, hostile rows, decode
    /// panics, HTTP errors). Failures are latched into `early_err` and
    /// surfaced only after all advertised queries have been attempted; a
    /// latched attempt queries dummy row 0. To the server a YPIR query for
    /// any row is indistinguishable under LWE (fresh OsRng seed per query),
    /// so the dummy index carries no signal; fixed 0 is always in range and
    /// keeps tests deterministic.
    async fn fetch_proof_inner(&self, nullifier: Fp) -> Result<(ImtProofData, NoteTiming)> {
        let note_start = Instant::now();
        let mut path = [Fp::default(); TREE_DEPTH];
        let mut early_err: Option<anyhow::Error> = None;

        // Tier 0 selects the first encrypted row. Server-supplied bytes:
        // catch panics and latch failures instead of skipping the queries.
        let row1 = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            reconstruct::process_plaintext_tier0(&self.tier0, &self.layout, nullifier, &mut path)
        })) {
            Ok(Ok(s1)) => s1,
            Ok(Err(e)) => {
                early_err = Some(e);
                0
            }
            Err(payload) => {
                early_err = Some(panic_to_error("tier0", payload));
                0
            }
        };

        let query1_idx = if early_err.is_some() || row1 >= self.tier1_scenario.num_items {
            if early_err.is_none() {
                early_err = Some(anyhow::anyhow!("tier1 row index out of range"));
            }
            0
        } else {
            row1
        };
        let tier1_result = self
            .ypir_query(
                &self.tier1_scenario,
                "tier1",
                query1_idx,
                self.tier1_payload_bytes,
            )
            .await;

        let (proof, tier1_timing, tier2_timing) =
            match (&self.tier2_scenario, self.tier2_payload_bytes) {
                // ── Three-tier: boundary Tier 1, terminal Tier 2 ─────────────
                (Some(tier2_scenario), Some(tier2_payload_bytes)) => {
                    let (row2, tier1_timing) = match tier1_result {
                        Ok((tier1_row, timing)) => {
                            if early_err.is_none() {
                                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    reconstruct::process_boundary_tier1(
                                        &tier1_row,
                                        &self.layout,
                                        row1,
                                        nullifier,
                                        &mut path,
                                    )
                                })) {
                                    Ok(Ok(next)) => (next, Some(timing)),
                                    Ok(Err(e)) => {
                                        early_err = Some(e);
                                        reconstruct::dummy_boundary_work(&self.layout, nullifier);
                                        (0, Some(timing))
                                    }
                                    Err(payload) => {
                                        early_err = Some(panic_to_error("tier1", payload));
                                        reconstruct::dummy_boundary_work(&self.layout, nullifier);
                                        (0, Some(timing))
                                    }
                                }
                            } else {
                                // Latched earlier: run the same local work shape
                                // before the dummy query so the inter-query gap
                                // does not reveal that the failure happened.
                                reconstruct::dummy_boundary_work(&self.layout, nullifier);
                                (0, Some(timing))
                            }
                        }
                        Err(e) => {
                            if early_err.is_none() {
                                early_err = Some(e);
                            }
                            reconstruct::dummy_boundary_work(&self.layout, nullifier);
                            (0, None)
                        }
                    };

                    // row2 < 2^(t0+t1) == num_items holds arithmetically
                    // (row1 < 2^t0 from Tier 0 parse, child < 2^t1 from the
                    // total scan); clamp defensively — never skip the query.
                    let query2_idx = if early_err.is_some() || row2 >= tier2_scenario.num_items {
                        0
                    } else {
                        row2
                    };
                    let tier2_result = self
                        .ypir_query(tier2_scenario, "tier2", query2_idx, tier2_payload_bytes)
                        .await;

                    let (proof, tier2_timing) = match tier2_result {
                        Ok((tier2_row, timing)) => {
                            if early_err.is_none() {
                                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    reconstruct::process_terminal(
                                        &tier2_row,
                                        &self.layout,
                                        self.layout.tier2_layers,
                                        row2,
                                        self.num_ranges,
                                        nullifier,
                                        &mut path,
                                        &self.empty_hashes,
                                        self.root29,
                                    )
                                })) {
                                    Ok(Ok(p)) => (Some(p), Some(timing)),
                                    Ok(Err(e)) => {
                                        early_err = Some(e);
                                        (None, Some(timing))
                                    }
                                    Err(payload) => {
                                        early_err = Some(panic_to_error("tier2", payload));
                                        (None, Some(timing))
                                    }
                                }
                            } else {
                                (None, Some(timing))
                            }
                        }
                        Err(e) => {
                            if early_err.is_none() {
                                early_err = Some(e);
                            }
                            (None, None)
                        }
                    };
                    (proof, tier1_timing, tier2_timing)
                }
                // ── Two-tier: terminal Tier 1 (bit-for-bit today's behavior) ─
                _ => {
                    let (proof, tier1_timing) = match tier1_result {
                        Ok((tier1_row, timing)) => {
                            if early_err.is_none() {
                                match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    reconstruct::process_terminal(
                                        &tier1_row,
                                        &self.layout,
                                        self.layout.tier1_layers,
                                        row1,
                                        self.num_ranges,
                                        nullifier,
                                        &mut path,
                                        &self.empty_hashes,
                                        self.root29,
                                    )
                                })) {
                                    Ok(Ok(p)) => (Some(p), Some(timing)),
                                    Ok(Err(e)) => {
                                        early_err = Some(e);
                                        (None, Some(timing))
                                    }
                                    Err(payload) => {
                                        early_err = Some(panic_to_error("tier1", payload));
                                        (None, Some(timing))
                                    }
                                }
                            } else {
                                (None, Some(timing))
                            }
                        }
                        Err(e) => {
                            if early_err.is_none() {
                                early_err = Some(e);
                            }
                            (None, None)
                        }
                    };
                    (proof, tier1_timing, None)
                }
            };

        if let Some(e) = early_err {
            return Err(e);
        }
        let proof = proof.expect("proof present when no error latched");
        let tier1 = tier1_timing.expect("tier1 timing present when no error latched");

        let total_ms = note_start.elapsed().as_secs_f64() * 1000.0;
        Ok((
            proof,
            NoteTiming {
                tier1,
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
        // Callers clamp row_idx into range before scheduling the query (a
        // scheduled query must never be aborted — see fetch_proof_inner);
        // this is a programmer-error backstop only.
        debug_assert!(row_idx < scenario.num_items);
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
            // Never embed server-controlled body bytes in the returned error
            // (they propagate into caller logs); log truncated at debug only.
            log::debug!(
                "[PIR] {tier_name} query HTTP {status} body={}",
                truncate_ascii(&String::from_utf8_lossy(&response_bytes))
            );
            anyhow::bail!("{} query failed: HTTP {}", tier_name, status);
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
            log::debug!(
                "[PIR] {tier_name} response decryption panicked: {}",
                truncate_ascii(msg)
            );
            anyhow::anyhow!("{} response decryption panicked", tier_name)
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
        // Server-controlled body bytes stay out of the error string; log
        // truncated at debug only.
        log::debug!(
            "[PIR] {context}: HTTP {} body={}",
            response.status,
            truncate_ascii(&String::from_utf8_lossy(&response.body))
        );
        anyhow::bail!("{}: HTTP {}", context, response.status)
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
    /// Connect with a caller-provided HTTP transport and configuration layout.
    pub fn with_transport(
        server_url: &str,
        expected_layout: PirLayout,
        transport: Arc<dyn Transport>,
    ) -> Result<Self> {
        let rt = tokio::runtime::Runtime::new()?;
        let inner = rt.block_on(PirClient::with_transport(
            server_url,
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

    /// The depth-29 root (PIR depth 19 padded to tree depth 29).
    pub fn root29(&self) -> Fp {
        self.inner.root29
    }
}

// ── Local (in-process) PIR client ────────────────────────────────────────────

/// Perform a complete local PIR proof retrieval without HTTP, for the
/// compiled default two-tier layout.
///
/// This is used by `pir-test local` mode. It takes the tier data directly
/// (as built by `pir-export`) and performs the row operations in-process.
pub fn fetch_proof_local(
    tier0_data: &[u8],
    tier1_data: &[u8],
    num_ranges: usize,
    nullifier: Fp,
    empty_hashes: &[Fp; TREE_DEPTH],
    root29: Fp,
) -> Result<ImtProofData> {
    fetch_proof_local_with_layout(
        tier0_data,
        tier1_data,
        None,
        num_ranges,
        nullifier,
        empty_hashes,
        root29,
        &COMPILED_PIR_LAYOUT,
    )
}

/// Slice one logical row's payload out of a flat local tier blob.
///
/// The blob may use either the padded on-disk stride or unpadded logical
/// rows; the stride is inferred from the blob length.
fn local_row(data: &[u8], rows: usize, payload_bytes: usize, idx: usize) -> Result<&[u8]> {
    anyhow::ensure!(
        rows > 0 && data.len().is_multiple_of(rows),
        "tier data length {} is not a multiple of {} rows",
        data.len(),
        rows
    );
    let stride = data.len() / rows;
    anyhow::ensure!(
        stride >= payload_bytes && idx < rows,
        "tier row out of range or stride too small"
    );
    Ok(&data[idx * stride..idx * stride + payload_bytes])
}

/// [`fetch_proof_local`] for an explicit layout, with `tier2_data` required
/// iff the layout enables Tier 2.
#[allow(clippy::too_many_arguments)]
pub fn fetch_proof_local_with_layout(
    tier0_data: &[u8],
    tier1_data: &[u8],
    tier2_data: Option<&[u8]>,
    num_ranges: usize,
    nullifier: Fp,
    empty_hashes: &[Fp; TREE_DEPTH],
    root29: Fp,
    layout: &PirLayout,
) -> Result<ImtProofData> {
    let mut path = [Fp::default(); TREE_DEPTH];
    let tier0 = Tier0Data::from_layout(tier0_data.to_vec(), *layout)?;
    let tier1_rows = layout.tier1_rows().map_err(anyhow::Error::msg)?;
    let tier1_payload_bytes = layout.tier1_row_bytes().map_err(anyhow::Error::msg)?;

    let row1 = reconstruct::process_plaintext_tier0(&tier0, layout, nullifier, &mut path)?;
    let tier1_row = local_row(tier1_data, tier1_rows, tier1_payload_bytes, row1)?;

    if !layout.tier2_enabled() {
        reconstruct::process_terminal(
            tier1_row,
            layout,
            layout.tier1_layers,
            row1,
            num_ranges,
            nullifier,
            &mut path,
            empty_hashes,
            root29,
        )
    } else {
        {
            let tier2_rows = layout
                .tier2_rows()
                .map_err(anyhow::Error::msg)?
                .context("tier2 rows")?;
            let tier2_payload_bytes = layout
                .tier2_row_bytes()
                .map_err(anyhow::Error::msg)?
                .context("tier2 row bytes")?;
            let tier2_data = tier2_data.context("tier2 data required for a three-tier layout")?;
            let row2 =
                reconstruct::process_boundary_tier1(tier1_row, layout, row1, nullifier, &mut path)?;
            let tier2_row = local_row(tier2_data, tier2_rows, tier2_payload_bytes, row2)?;
            reconstruct::process_terminal(
                tier2_row,
                layout,
                layout.tier2_layers,
                row2,
                num_ranges,
                nullifier,
                &mut path,
                empty_hashes,
                root29,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ff::Field;
    use pasta_curves::Fp;
    use pir_export::build_ranges_with_sentinels;
    use pir_types::{PIR_DEPTH, TIER0_LAYERS, TIER1_LEAVES, TIER1_ROW_BYTES};

    /// A 12+4+3 three-tier layout used across the tier-2 tests.
    const THREE_TIER: PirLayout = PirLayout {
        pir_depth: 19,
        tier0_layers: 12,
        tier1_layers: 4,
        tier2_layers: 3,
    };

    fn layout(pir_depth: usize, t0: usize, t1: usize, t2: usize) -> PirLayout {
        PirLayout {
            pir_depth,
            tier0_layers: t0,
            tier1_layers: t1,
            tier2_layers: t2,
        }
    }

    /// Build a tree and export the tier blobs for a layout.
    struct TestFixture {
        layout: PirLayout,
        tree: pir_export::PirTree,
        tier0_data: Vec<u8>,
        tier1_data: Vec<u8>,
        tier2_data: Option<Vec<u8>>,
        ranges: Vec<[Fp; 3]>,
        empty_hashes: [Fp; TREE_DEPTH],
        root29: Fp,
    }

    impl TestFixture {
        fn build(raw_nfs: &[Fp]) -> Self {
            Self::build_with_layout(raw_nfs, &COMPILED_PIR_LAYOUT)
        }

        fn build_with_layout(raw_nfs: &[Fp], layout: &PirLayout) -> Self {
            let ranges = build_ranges_with_sentinels(raw_nfs);
            let tree =
                pir_export::build_pir_tree_with_depth(ranges.clone(), layout.pir_depth).unwrap();

            let tier0_data = pir_export::tier0::export_layout(
                &tree.root25,
                &tree.levels,
                &tree.ranges,
                &tree.empty_hashes,
                *layout,
            )
            .unwrap();
            let mut tier1_data = Vec::new();
            if layout.tier2_enabled() {
                pir_export::export_boundary_tier(&tree, layout, &mut tier1_data).unwrap();
            } else {
                pir_export::tier1::export_layout(&tree.ranges, &mut tier1_data, *layout).unwrap();
            }
            let tier2_data = layout.tier2_enabled().then(|| {
                let mut data = Vec::new();
                pir_export::export_tier2(&tree.ranges, layout, &mut data).unwrap();
                data
            });

            let empty_hashes = tree.empty_hashes;
            let root29 = tree.root29;
            Self {
                layout: *layout,
                tree,
                tier0_data,
                tier1_data,
                tier2_data,
                ranges,
                empty_hashes,
                root29,
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
            fix.ranges.len(),
            value,
            &fix.empty_hashes,
            fix.root29,
        )
        .unwrap();

        assert_eq!(proof.root, fix.root29);
        assert_eq!(proof.path.len(), TREE_DEPTH);
    }

    #[test]
    fn fetch_proof_local_depth20_11_5_4_verifies() {
        // Non-default tier0 layer count and PIR depth: exercises the
        // generalized Tier 0 parser and runtime path offsets/padding.
        let raw_nfs: Vec<Fp> = (1u64..=200).map(|i| Fp::from(i * 991)).collect();
        let fix = TestFixture::build_with_layout(&raw_nfs, &layout(20, 11, 5, 4));

        for &[nf_lo, _, _] in fix.ranges.iter().take(20) {
            let value = nf_lo + Fp::one();
            let proof = fetch_proof_local_with_layout(
                &fix.tier0_data,
                &fix.tier1_data,
                fix.tier2_data.as_deref(),
                fix.ranges.len(),
                value,
                &fix.empty_hashes,
                fix.root29,
                &fix.layout,
            )
            .expect("depth-20 local proof should succeed");
            assert!(proof.verify(value));
            assert_eq!(proof.root, fix.root29);
        }
    }

    #[test]
    fn fetch_proof_local_three_tier_verifies_for_known_ranges() {
        let raw_nfs: Vec<Fp> = (1u64..=300).map(|i| Fp::from(i * 977)).collect();
        let fix = TestFixture::build_with_layout(&raw_nfs, &THREE_TIER);

        for &[nf_lo, _, _] in fix.ranges.iter().take(30) {
            let value = nf_lo + Fp::one();
            let proof = fetch_proof_local_with_layout(
                &fix.tier0_data,
                &fix.tier1_data,
                fix.tier2_data.as_deref(),
                fix.ranges.len(),
                value,
                &fix.empty_hashes,
                fix.root29,
                &fix.layout,
            )
            .expect("three-tier local proof should succeed for a value in range");
            assert!(proof.verify(value));
            assert_eq!(proof.root, fix.root29);
        }
        // Cover the tail of the field too.
        let value = Fp::from(2u64).neg();
        let proof = fetch_proof_local_with_layout(
            &fix.tier0_data,
            &fix.tier1_data,
            fix.tier2_data.as_deref(),
            fix.ranges.len(),
            value,
            &fix.empty_hashes,
            fix.root29,
            &fix.layout,
        )
        .unwrap();
        assert!(proof.verify(value));
        assert_eq!(proof.leaf_pos as usize, fix.ranges.len() - 1);
    }

    #[test]
    fn two_and_three_tier_proofs_match_for_same_dataset() {
        let raw_nfs: Vec<Fp> = (1u64..=200).map(|i| Fp::from(i * 1013)).collect();
        let two = TestFixture::build(&raw_nfs);
        let three = TestFixture::build_with_layout(&raw_nfs, &THREE_TIER);
        assert_eq!(two.root29, three.root29);

        for &[nf_lo, _, _] in two.ranges.iter().take(20) {
            let value = nf_lo + Fp::one();
            let p2 = fetch_proof_local(
                &two.tier0_data,
                &two.tier1_data,
                two.ranges.len(),
                value,
                &two.empty_hashes,
                two.root29,
            )
            .unwrap();
            let p3 = fetch_proof_local_with_layout(
                &three.tier0_data,
                &three.tier1_data,
                three.tier2_data.as_deref(),
                three.ranges.len(),
                value,
                &three.empty_hashes,
                three.root29,
                &three.layout,
            )
            .unwrap();
            assert_eq!(p2.root, p3.root);
            assert_eq!(p2.leaf_pos, p3.leaf_pos);
            assert_eq!(p2.nf_bounds, p3.nf_bounds);
            assert_eq!(p2.path, p3.path);
        }
    }

    // ── reconstruct helpers ──────────────────────────────────────────────

    #[test]
    fn process_tier0_fills_correct_path_region() {
        let raw_nfs: Vec<Fp> = (1u64..=30).map(|i| Fp::from(i * 1013)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let tier0 = Tier0Data::from_bytes(fix.tier0_data).unwrap();

        let value = fix.ranges[0][0];
        let mut path = [Fp::default(); TREE_DEPTH];
        let s1 =
            reconstruct::process_plaintext_tier0(&tier0, &COMPILED_PIR_LAYOUT, value, &mut path)
                .unwrap();

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
        let s1 =
            reconstruct::process_plaintext_tier0(&tier0, &COMPILED_PIR_LAYOUT, bogus, &mut path)
                .unwrap();
        assert!(s1 < pir_types::TIER1_ROWS);
    }

    #[test]
    fn process_terminal_produces_verifiable_proof() {
        let raw_nfs: Vec<Fp> = (1u64..=30).map(|i| Fp::from(i * 1013)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let tier0 = Tier0Data::from_bytes(fix.tier0_data.clone()).unwrap();

        let value = fix.ranges[0][0] + Fp::one();
        let mut path = [Fp::default(); TREE_DEPTH];

        let s1 =
            reconstruct::process_plaintext_tier0(&tier0, &COMPILED_PIR_LAYOUT, value, &mut path)
                .unwrap();
        let t1_offset = s1 * TIER1_ROW_BYTES;
        let proof = reconstruct::process_terminal(
            &fix.tier1_data[t1_offset..t1_offset + TIER1_ROW_BYTES],
            &COMPILED_PIR_LAYOUT,
            COMPILED_PIR_LAYOUT.tier1_layers,
            s1,
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
        use reconstruct::valid_leaves_for_row;
        let n = TIER1_LEAVES;
        assert_eq!(valid_leaves_for_row(n, 0, n), n);
        assert_eq!(valid_leaves_for_row(n + 1, 0, n), n);
        assert_eq!(valid_leaves_for_row(n + 1, 1, n), 1);
        assert_eq!(valid_leaves_for_row(0, 0, n), 0);
        assert_eq!(valid_leaves_for_row(1, 0, n), 1);
        assert_eq!(valid_leaves_for_row(1, 1, n), 0);
        // Runtime records-per-row (tier2 with 3 layers = 8 leaves per row).
        assert_eq!(valid_leaves_for_row(20, 2, 8), 4);
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
            fix.root29,
        );
        assert!(result.is_err());
    }

    // ── Mock transport ────────────────────────────────────────────────────

    struct MockTransport {
        gets: std::collections::HashMap<&'static str, TransportResponse>,
        posts: std::collections::HashMap<&'static str, TransportResponse>,
        hits: std::sync::Mutex<Vec<String>>,
    }

    impl MockTransport {
        fn new(fix: &TestFixture) -> Self {
            use ff::PrimeField as _;

            let tier1_scenario = fix.layout.tier1_scenario().unwrap();
            let tier2_scenario = fix.layout.tier2_scenario().unwrap();
            let root_info = pir_types::RootInfo {
                zcash_network: pir_types::ZcashNetwork::Test,
                nullifier_pool: pir_types::NULLIFIER_POOL.to_owned(),
                dataset_version: pir_types::DATASET_VERSION,
                root29: hex::encode(fix.tree.root29.to_repr()),
                root25: hex::encode(fix.tree.root25.to_repr()),
                num_ranges: fix.ranges.len(),
                pir_layout: fix.layout,
                pir_depth: fix.layout.pir_depth,
                tier1_rows: tier1_scenario.num_items,
                tier1_row_bytes: tier1_scenario.item_size_bits / 8,
                tier2_rows: tier2_scenario.as_ref().map_or(0, |s| s.num_items),
                tier2_row_bytes: tier2_scenario.as_ref().map_or(0, |s| s.item_size_bits / 8),
                height: None,
            };

            let mut gets: std::collections::HashMap<&'static str, TransportResponse> = [
                ("/tier0", response(fix.tier0_data.clone())),
                (
                    "/params/tier1",
                    response(serde_json::to_vec(&tier1_scenario).unwrap()),
                ),
                ("/root", response(serde_json::to_vec(&root_info).unwrap())),
            ]
            .into_iter()
            .collect();
            // Mock responses are deliberately corrupt ciphertext; tests using
            // this transport assert on request traces, not on proofs.
            let mut posts: std::collections::HashMap<&'static str, TransportResponse> =
                [("/tier1/query", response(vec![0xDE; 65536]))]
                    .into_iter()
                    .collect();
            if let Some(tier2_scenario) = &tier2_scenario {
                gets.insert(
                    "/params/tier2",
                    response(serde_json::to_vec(tier2_scenario).unwrap()),
                );
                posts.insert("/tier2/query", response(vec![0xDE; 65536]));
            }

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

        fn query_hits(&self) -> Vec<String> {
            self.hits
                .lock()
                .unwrap()
                .iter()
                .filter(|hit| hit.ends_with("/query"))
                .cloned()
                .collect()
        }

        fn update_root(&mut self, update: impl FnOnce(&mut serde_json::Value)) {
            let mut root: serde_json::Value =
                serde_json::from_slice(&self.gets.get("/root").unwrap().body).unwrap();
            update(&mut root);
            self.gets
                .insert("/root", response(serde_json::to_vec(&root).unwrap()));
        }

        /// Overwrite the first Tier 0 subtree record's min_key so that every
        /// min_key exceeds small query values — a server-craftable Tier 0
        /// gap that makes `find_subtree` miss.
        fn poison_tier0_gap(&mut self, layout: &PirLayout) {
            let mut tier0 = self.gets.get("/tier0").unwrap().body.clone();
            let internal_nodes = layout.tier0_internal_nodes().unwrap();
            let min_key_offset = internal_nodes * 32 + 32;
            pir_types::fp_utils::write_fp(&mut tier0[min_key_offset..], Fp::from(1_000_000u64));
            self.gets.insert("/tier0", response(tier0));
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
        match PirClient::with_transport("https://pir.example", expected_layout, transport).await {
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

    async fn connect(fix: &TestFixture, transport: Arc<MockTransport>) -> PirClient {
        PirClient::with_transport("https://pir.example", fix.layout, transport)
            .await
            .unwrap()
    }

    // ── Oracle cardinality: the request trace is failure-independent ─────

    #[tokio::test]
    async fn proof_attempt_sends_exactly_one_pir_request() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let transport = Arc::new(MockTransport::new(&fix));
        let client = connect(&fix, transport.clone()).await;

        // The mock response is deliberately corrupt; request count is the
        // property under test.
        assert!(client
            .fetch_proof(fix.ranges[0][0] + Fp::one())
            .await
            .is_err());
        assert_eq!(transport.count_hits("/tier1/query"), 1);
        assert_eq!(transport.query_hits().len(), 1);
    }

    #[tokio::test]
    async fn three_tier_proof_attempt_sends_both_queries_in_order() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build_with_layout(&raw_nfs, &THREE_TIER);
        let transport = Arc::new(MockTransport::new(&fix));
        let client = connect(&fix, transport.clone()).await;

        // Corrupt tier1 ciphertext (decode fails) must still be followed by
        // exactly one tier2 query.
        assert!(client
            .fetch_proof(fix.ranges[0][0] + Fp::one())
            .await
            .is_err());
        assert_eq!(
            transport.query_hits(),
            vec!["/tier1/query".to_string(), "/tier2/query".to_string()]
        );
    }

    #[tokio::test]
    async fn tier0_miss_still_sends_all_advertised_queries() {
        // A malicious server crafting Tier 0 boundary keys with a gap over a
        // chosen range must not learn from request cardinality whether the
        // client's nullifier fell into the gap.
        for three_tier in [false, true] {
            let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
            let layout = if three_tier {
                THREE_TIER
            } else {
                COMPILED_PIR_LAYOUT
            };
            let fix = TestFixture::build_with_layout(&raw_nfs, &layout);
            let mut transport = MockTransport::new(&fix);
            transport.poison_tier0_gap(&layout);
            let transport = Arc::new(transport);
            let client = connect(&fix, transport.clone()).await;

            let err = client
                .fetch_proof(Fp::from(5u64))
                .await
                .expect_err("tier0 miss must surface an error");
            assert!(
                err.to_string().contains("Tier 0"),
                "unexpected error: {err}"
            );
            let expected: Vec<String> = if three_tier {
                vec!["/tier1/query".into(), "/tier2/query".into()]
            } else {
                vec!["/tier1/query".into()]
            };
            assert_eq!(transport.query_hits(), expected);
        }
    }

    #[tokio::test]
    async fn tier1_http_error_still_sends_tier2_query() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build_with_layout(&raw_nfs, &THREE_TIER);
        let mut transport = MockTransport::new(&fix);
        transport.posts.insert(
            "/tier1/query",
            TransportResponse {
                status: 400,
                headers: Vec::new(),
                body: b"SENTINEL-SERVER-BODY".to_vec(),
            },
        );
        let transport = Arc::new(transport);
        let client = connect(&fix, transport.clone()).await;

        let err = client
            .fetch_proof(fix.ranges[0][0] + Fp::one())
            .await
            .expect_err("tier1 HTTP error must surface");
        // The transport-level failure must not suppress the tier2 attempt.
        assert_eq!(
            transport.query_hits(),
            vec!["/tier1/query".to_string(), "/tier2/query".to_string()]
        );
        // Server-controlled body bytes must not leak into the error text.
        assert!(
            !err.to_string().contains("SENTINEL-SERVER-BODY"),
            "server body leaked into error: {err}"
        );
    }

    #[tokio::test]
    async fn hostile_boundary_row_latches_and_sends_dummy_tier2() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build_with_layout(&raw_nfs, &THREE_TIER);
        let transport = Arc::new(MockTransport::new(&fix));
        let client = connect(&fix, transport.clone()).await;

        assert!(client
            .fetch_proof(fix.ranges[0][0] + Fp::one())
            .await
            .is_err());
        assert_eq!(transport.count_hits("/tier1/query"), 1);
        assert_eq!(transport.count_hits("/tier2/query"), 1);
    }

    #[tokio::test]
    async fn batch_waits_for_all_pir_requests_before_propagating_decode_error() {
        const K: usize = 5;

        let raw_nfs: Vec<Fp> = (1u64..=20).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let transport = Arc::new(MockTransport::new(&fix));
        let client = connect(&fix, transport.clone()).await;
        let values: Vec<Fp> = fix
            .ranges
            .iter()
            .take(K)
            .map(|range| range[0] + Fp::one())
            .collect();

        assert!(client.fetch_proofs(&values).await.is_err());
        assert_eq!(transport.count_hits("/tier1/query"), K);
    }

    #[tokio::test]
    async fn three_tier_batch_sends_two_queries_per_note() {
        const K: usize = 5;

        let raw_nfs: Vec<Fp> = (1u64..=20).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build_with_layout(&raw_nfs, &THREE_TIER);
        let transport = Arc::new(MockTransport::new(&fix));
        let client = connect(&fix, transport.clone()).await;
        let values: Vec<Fp> = fix
            .ranges
            .iter()
            .take(K)
            .map(|range| range[0] + Fp::one())
            .collect();

        assert!(client.fetch_proofs(&values).await.is_err());
        assert_eq!(transport.count_hits("/tier1/query"), K);
        assert_eq!(transport.count_hits("/tier2/query"), K);
    }

    // ── Layout validation ─────────────────────────────────────────────────

    #[test]
    fn rejects_inconsistent_or_out_of_circuit_layout_geometry() {
        let inconsistent = layout(19, 12, 8, 0);
        let err = validate_layout("test", inconsistent)
            .unwrap_err()
            .to_string();
        assert!(err.contains("is inconsistent"), "{err}");

        let too_deep = layout(TREE_DEPTH + 1, 15, 15, 0);
        let err = validate_layout("test", too_deep).unwrap_err().to_string();
        assert!(err.contains("exceeds circuit depth"), "{err}");

        let tier0_out_of_range = layout(29, 17, 6, 6);
        let err = validate_layout("test", tier0_out_of_range)
            .unwrap_err()
            .to_string();
        assert!(err.contains("tier0_layers"), "{err}");

        let missing_tier1 = layout(19, 12, 0, 7);
        let err = validate_layout("test", missing_tier1)
            .unwrap_err()
            .to_string();
        assert!(err.contains("tier1_layers"), "{err}");
    }

    #[tokio::test]
    async fn rejects_missing_layout_metadata_without_query() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let mut transport = MockTransport::new(&fix);
        transport.update_root(|root| {
            root.as_object_mut().unwrap().remove("pir_layout");
        });
        transport.gets.insert("/tier0", response(vec![0xff]));
        let transport = Arc::new(transport);

        let err = rejected_connect(COMPILED_PIR_LAYOUT, transport.clone()).await;
        assert!(err.contains("parse /root response"), "{err}");
        assert_eq!(transport.query_hits().len(), 0);
    }

    #[tokio::test]
    async fn rejects_config_depth_and_split_mismatches_without_query() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let mismatches = [layout(19, 11, 8, 0), layout(19, 13, 6, 0)];

        for expected_layout in mismatches {
            let transport = Arc::new(MockTransport::new(&fix));
            let err = rejected_connect(expected_layout, transport.clone()).await;
            assert!(err.contains("expected") && err.contains("server"), "{err}");
            assert_eq!(transport.query_hits().len(), 0);
        }
    }

    #[tokio::test]
    async fn rejects_server_depth_and_split_mismatches_without_query() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let mismatches = [layout(19, 11, 8, 0), layout(19, 13, 6, 0)];

        for server_layout in mismatches {
            let mut transport = MockTransport::new(&fix);
            transport.update_root(|root| {
                root["pir_layout"] = serde_json::to_value(server_layout).unwrap();
            });
            let transport = Arc::new(transport);
            let err = rejected_connect(COMPILED_PIR_LAYOUT, transport.clone()).await;
            assert!(err.contains("expected") && err.contains("server"), "{err}");
            assert_eq!(transport.query_hits().len(), 0);
        }
    }

    #[tokio::test]
    async fn rejects_out_of_bounds_layout_even_when_server_agrees() {
        // The compiled bounds (tier0 in 11..=16, depth <= 29, tier1 >= 1)
        // hold even when config and server fully agree on the layout.
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let out_of_bounds = [
            layout(19, 10, 9, 0),
            layout(19, 17, 2, 0),
            layout(19, 12, 0, 7),
        ];

        for negotiated_layout in out_of_bounds {
            let mut transport = MockTransport::new(&fix);
            transport.update_root(|root| {
                root["pir_layout"] = serde_json::to_value(negotiated_layout).unwrap();
            });
            let transport = Arc::new(transport);
            let err = rejected_connect(negotiated_layout, transport.clone()).await;
            assert!(
                err.contains("tier0_layers")
                    || err.contains("non-zero")
                    || err.contains("below YPIR minimum"),
                "{err}"
            );
            assert_eq!(transport.query_hits().len(), 0);
        }
    }

    #[tokio::test]
    async fn rejects_tier_count_mismatch_without_query() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();

        // Config three-tier vs server two-tier.
        let two = TestFixture::build(&raw_nfs);
        let transport = Arc::new(MockTransport::new(&two));
        let err =
            match PirClient::with_transport("https://pir.example", THREE_TIER, transport.clone())
                .await
            {
                Ok(_) => panic!("tier-count mismatch must be rejected"),
                Err(err) => err.to_string(),
            };
        // The two-tier mock has no /params/tier2 route; whether the connect
        // fails there or at the layout equality, no query may be sent.
        assert!(!err.is_empty());
        assert_eq!(transport.query_hits().len(), 0);

        // Config two-tier vs server three-tier.
        let three = TestFixture::build_with_layout(&raw_nfs, &THREE_TIER);
        let transport = Arc::new(MockTransport::new(&three));
        let err = rejected_connect(COMPILED_PIR_LAYOUT, transport.clone()).await;
        assert!(err.contains("PIR layout mismatch"), "{err}");
        assert_eq!(transport.query_hits().len(), 0);
    }

    #[tokio::test]
    async fn rejects_tier2_shape_mismatch_without_query() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build_with_layout(&raw_nfs, &THREE_TIER);
        let mut transport = MockTransport::new(&fix);
        transport.update_root(|root| {
            root["tier2_rows"] = serde_json::Value::from(1234);
        });
        let transport = Arc::new(transport);

        let err = rejected_connect(THREE_TIER, transport.clone()).await;
        assert!(err.contains("Tier 2 shape mismatch"), "{err}");
        assert_eq!(transport.query_hits().len(), 0);
    }

    #[tokio::test]
    async fn rejects_nonzero_tier2_fields_under_two_tier_layout() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let mut transport = MockTransport::new(&fix);
        transport.update_root(|root| {
            root["tier2_rows"] = serde_json::Value::from(4096);
            root["tier2_row_bytes"] = serde_json::Value::from(3584);
        });
        let transport = Arc::new(transport);

        let err = rejected_connect(COMPILED_PIR_LAYOUT, transport.clone()).await;
        assert!(err.contains("Tier 2"), "{err}");
        assert_eq!(transport.query_hits().len(), 0);
    }

    #[tokio::test]
    async fn rejects_wrong_nullifier_pool() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let mut transport = MockTransport::new(&fix);
        transport.update_root(|root| {
            root["nullifier_pool"] = serde_json::Value::String("orchard".to_owned());
        });
        let transport = Arc::new(transport);

        let err = rejected_connect(COMPILED_PIR_LAYOUT, transport.clone()).await;
        assert!(err.contains("orchard"), "{err}");
    }

    #[tokio::test]
    async fn rejects_mismatched_tier_shape() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let mut transport = MockTransport::new(&fix);
        transport.update_root(|root| {
            root["tier1_row_bytes"] = serde_json::Value::from(4_096);
        });
        let transport = Arc::new(transport);

        let err = rejected_connect(COMPILED_PIR_LAYOUT, transport.clone()).await;
        assert!(err.contains("Tier 1 shape mismatch"), "{err}");
        assert_eq!(transport.query_hits().len(), 0);
    }

    #[tokio::test]
    async fn rejects_version_one_root_before_shape_check() {
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build(&raw_nfs);
        let mut transport = MockTransport::new(&fix);
        transport.update_root(|root| {
            root["dataset_version"] = serde_json::Value::from(1);
            root.as_object_mut().unwrap().remove("tier1_rows");
            root.as_object_mut().unwrap().remove("tier1_row_bytes");
        });

        let err = rejected_connect(COMPILED_PIR_LAYOUT, Arc::new(transport)).await;
        assert!(err.contains("version 1 is unsupported"), "{err}");
    }

    #[tokio::test]
    async fn error_strings_contain_no_row_indices() {
        // Client-local errors propagate into wallet logs; they must not
        // carry the nullifier-derived row/bucket indices.
        let raw_nfs: Vec<Fp> = (1u64..=10).map(|i| Fp::from(i * 7)).collect();
        let fix = TestFixture::build_with_layout(&raw_nfs, &THREE_TIER);
        let transport = Arc::new(MockTransport::new(&fix));
        let client = connect(&fix, transport.clone()).await;

        let err = client
            .fetch_proof(fix.ranges[0][0] + Fp::one())
            .await
            .expect_err("corrupt responses must fail");
        let text = format!("{err:#}");
        assert!(
            !text.contains("row_idx") && !text.contains("num_items"),
            "row indices leaked into error: {text}"
        );
    }
}
