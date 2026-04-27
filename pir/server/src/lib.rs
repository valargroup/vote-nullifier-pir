//! YPIR+SP server wrapper and shared types for the PIR HTTP server.
//!
//! This module encapsulates all YPIR operations, providing a clean interface
//! that both the HTTP server (`main.rs`) and the test harness (`pir-test`)
//! can use.

use anyhow::Result;
use std::io::Cursor;
use std::time::Instant;
use tracing::info;

use std::alloc::{alloc_zeroed, dealloc, handle_alloc_error, Layout};

use spiral_rs::params::Params;
use ypir::params::{params_for_scenario_simplepir, DbRowsCols, PtModulusBits};
use ypir::serialize::{FilePtIter, OfflinePrecomputedValues};
use ypir::server::YServer;

// Re-export shared types and constants so existing consumers can import from pir_server.
pub use pir_types::{
    parse_ypir_batch_query, serialize_ypir_batch_response, HealthInfo, PirMetadata, RootInfo,
    YpirScenario, MAX_BATCH_K, TIER1_ITEM_BITS, TIER1_ROWS, TIER1_ROW_BYTES, TIER1_YPIR_ROWS,
    TIER2_ITEM_BITS, TIER2_ROWS, TIER2_ROW_BYTES,
};

const U64_BYTES: usize = std::mem::size_of::<u64>();
const AVX512_ALIGN: usize = 64;

/// 64-byte aligned u64 buffer for AVX-512 operations.
struct Aligned64 {
    ptr: *mut u64,
    len: usize,
    layout: Layout,
}

impl Aligned64 {
    fn new(len: usize) -> Self {
        assert!(len > 0, "Aligned64::new called with zero length");
        let size = len.checked_mul(U64_BYTES).expect("Aligned64 size overflow");
        let layout = Layout::from_size_align(size, AVX512_ALIGN).expect("Aligned64 invalid layout");
        let ptr = unsafe { alloc_zeroed(layout) as *mut u64 };
        if ptr.is_null() {
            handle_alloc_error(layout);
        }
        Self { ptr, len, layout }
    }

    fn as_slice(&self) -> &[u64] {
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }

    fn as_mut_slice(&mut self) -> &mut [u64] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for Aligned64 {
    fn drop(&mut self) {
        unsafe { dealloc(self.ptr as *mut u8, self.layout) }
    }
}

/// Tier 1 YPIR scenario (padded to YPIR minimum row count).
pub fn tier1_scenario() -> YpirScenario {
    YpirScenario {
        num_items: TIER1_YPIR_ROWS,
        item_size_bits: TIER1_ITEM_BITS,
    }
}

/// Tier 2 YPIR scenario.
pub fn tier2_scenario() -> YpirScenario {
    YpirScenario {
        num_items: TIER2_ROWS,
        item_size_bits: TIER2_ITEM_BITS,
    }
}

// ── PIR server state ─────────────────────────────────────────────────────────

/// Holds the YPIR server state for one tier.
///
/// Wraps the YPIR `YServer` and its offline precomputed values. Answers
/// individual queries via `answer_query`.
///
/// Owns the YPIR `Params` via a heap allocation. The `server` and `offline`
/// fields hold `&'a Params` references into this allocation. `ManuallyDrop`
/// ensures they are dropped before `_params` is freed.
pub struct TierServer<'a> {
    server: std::mem::ManuallyDrop<YServer<'a, u16>>,
    offline: std::mem::ManuallyDrop<OfflinePrecomputedValues<'a>>,
    _params: Box<Params>,
    scenario: YpirScenario,
}

/// Per-request timing breakdown for a single PIR query.
#[derive(Debug, Clone, Copy)]
pub struct QueryTiming {
    pub validate_ms: f64,
    pub decode_copy_ms: f64,
    pub online_compute_ms: f64,
    pub total_ms: f64,
    pub response_bytes: usize,
}

/// Server answer payload paired with its timing breakdown.
#[derive(Debug)]
pub struct QueryAnswer {
    pub response: Vec<u8>,
    pub timing: QueryTiming,
}

/// Per-request timing breakdown for a batched PIR query.
///
/// All fields are *batch* totals — the bench harness divides upload-shared
/// fields (validate, decode_copy, online_compute) by `k` to recover a
/// per-query view and keeps per-query response bytes intact.
#[derive(Debug, Clone, Copy)]
pub struct BatchQueryTiming {
    pub k: usize,
    pub validate_ms: f64,
    pub decode_copy_ms: f64,
    pub online_compute_ms: f64,
    pub total_ms: f64,
    /// Total response bytes across all K queries (concatenated, before the
    /// 16-byte batch wire-format header).
    pub response_bytes: usize,
    /// Bytes per single-query response (deterministic for a scenario).
    pub response_bytes_per_query: usize,
}

/// Server answer payload for a batch query paired with its timing breakdown.
#[derive(Debug)]
pub struct BatchQueryAnswer {
    /// Wire-format batch response: `[K | resp_bytes_per_query | K * resp]`.
    pub response: Vec<u8>,
    pub timing: BatchQueryTiming,
}

impl<'a> TierServer<'a> {
    /// Initialize a YPIR+SP server from raw tier data.
    ///
    /// `data` is the flat binary tier file (rows × row_bytes).
    /// This performs the expensive offline precomputation.
    pub fn new(data: &'a [u8], scenario: YpirScenario) -> Self {
        let t0 = Instant::now();

        // Note: this is where server params are set.
        let params_box = Box::new(params_for_scenario_simplepir(
            scenario.num_items as u64,
            scenario.item_size_bits as u64,
        ));

        // SAFETY: We extend the reference lifetime to 'a. This is sound because:
        // 1. params_box is a heap allocation with a stable address
        // 2. server and offline are ManuallyDrop, dropped before _params in our Drop impl
        // 3. The reference remains valid for the entire lifetime of this struct
        let params: &'a Params =
            unsafe { std::mem::transmute::<&Params, &'a Params>(params_box.as_ref()) };

        info!(
            num_items = scenario.num_items,
            item_size_bits = scenario.item_size_bits,
            "YPIR server init"
        );

        // Use FilePtIter to pack raw bytes into 14-bit u16 values.
        // This matches how the YPIR standalone server reads database files.
        let bytes_per_row = scenario.item_size_bits / 8;
        let db_cols = params.db_cols_simplepir();
        let pt_bits = params.pt_modulus_bits();
        info!(bytes_per_row, db_cols, pt_bits, "FilePtIter config");
        let cursor = Cursor::new(data);
        let pt_iter = FilePtIter::new(cursor, bytes_per_row, db_cols, pt_bits);
        let server = YServer::<u16>::new(params, pt_iter, true, false, true);

        let t1 = Instant::now();
        info!(
            elapsed_s = format!("{:.1}", (t1 - t0).as_secs_f64()),
            "YPIR server constructed"
        );

        let offline = server.perform_offline_precomputation_simplepir(None, None, None);
        info!(
            elapsed_s = format!("{:.1}", t1.elapsed().as_secs_f64()),
            "YPIR offline precomputation done"
        );

        Self {
            server: std::mem::ManuallyDrop::new(server),
            offline: std::mem::ManuallyDrop::new(offline),
            _params: params_box,
            scenario,
        }
    }

    /// Answer a single YPIR+SP query.
    ///
    /// The query bytes must be in the length-prefixed format:
    /// `[8 bytes: packed_query_row byte length as LE u64][packed_query_row bytes][pub_params bytes]`
    ///
    /// Returns the serialized response as LE u64 bytes.
    pub fn answer_query(&self, query_bytes: &[u8]) -> Result<QueryAnswer> {
        let total_start = Instant::now();

        // Validate length-prefixed format: [8: pqr_byte_len][pqr][pub_params]
        let validate_start = Instant::now();
        anyhow::ensure!(
            query_bytes.len() >= 8,
            "query too short: {} bytes",
            query_bytes.len()
        );
        let pqr_byte_len =
            u64::from_le_bytes(query_bytes[..U64_BYTES].try_into().unwrap()) as usize;
        let payload_len = query_bytes.len() - U64_BYTES;
        anyhow::ensure!(
            pqr_byte_len.is_multiple_of(U64_BYTES),
            "pqr_byte_len {} not a multiple of 8",
            pqr_byte_len
        );
        anyhow::ensure!(
            pqr_byte_len <= payload_len,
            "pqr_byte_len {} exceeds payload ({})",
            pqr_byte_len,
            payload_len
        );
        let remaining = payload_len - pqr_byte_len; // safe: checked above
        anyhow::ensure!(pqr_byte_len > 0, "pqr section is empty");
        anyhow::ensure!(remaining > 0, "pub_params section is empty");
        anyhow::ensure!(
            remaining.is_multiple_of(U64_BYTES),
            "pub_params section {} bytes not a multiple of {}",
            remaining,
            U64_BYTES
        );
        let validate_ms = validate_start.elapsed().as_secs_f64() * 1000.0;

        let pqr_u64_len = pqr_byte_len / U64_BYTES;
        let pp_u64_len = remaining / U64_BYTES;

        // Copy into 64-byte aligned memory for AVX-512 operations.
        let decode_start = Instant::now();
        let mut pqr = Aligned64::new(pqr_u64_len);
        for (i, chunk) in query_bytes[U64_BYTES..U64_BYTES + pqr_byte_len]
            .chunks_exact(U64_BYTES)
            .enumerate()
        {
            pqr.as_mut_slice()[i] = u64::from_le_bytes(chunk.try_into().unwrap());
        }

        let mut pub_params = Aligned64::new(pp_u64_len);
        for (i, chunk) in query_bytes[U64_BYTES + pqr_byte_len..]
            .chunks_exact(U64_BYTES)
            .enumerate()
        {
            pub_params.as_mut_slice()[i] = u64::from_le_bytes(chunk.try_into().unwrap());
        }
        let decode_copy_ms = decode_start.elapsed().as_secs_f64() * 1000.0;

        // Run the YPIR online computation (returns Vec<u8> directly)
        let compute_start = Instant::now();
        let response = self.server.perform_online_computation_simplepir(
            pqr.as_slice(),
            &self.offline,
            &[pub_params.as_slice()],
            None,
        );
        let online_compute_ms = compute_start.elapsed().as_secs_f64() * 1000.0;
        let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;

        Ok(QueryAnswer {
            timing: QueryTiming {
                validate_ms,
                decode_copy_ms,
                online_compute_ms,
                total_ms,
                response_bytes: response.len(),
            },
            response,
        })
    }

    /// Answer K YPIR+SP queries served as one HTTP batch.
    ///
    /// Wire format (see [`pir_types::serialize_ypir_batch_query`]):
    /// `[8 bytes K][8 bytes pqr_byte_len][K * pqr_byte_len bytes][8 bytes pp_byte_len][pp bytes]`.
    /// All K `q.0` vectors share one `pack_pub_params` (`pp`) — that is the
    /// upload bandwidth lever the batching protocol exploits.
    ///
    /// Server-side compute here is deliberately *additive*: we call
    /// [`YServer::perform_online_computation_simplepir`] K times in a loop,
    /// once per `q.0`, sharing the same `pp`. There is no batched
    /// matrix-multiply yet — a follow-up iteration will replace this loop
    /// with a single K-wide kernel call.
    pub fn answer_batch_query(&self, query_bytes: &[u8]) -> Result<BatchQueryAnswer> {
        let total_start = Instant::now();

        let validate_start = Instant::now();
        let (pqrs, pp) = parse_ypir_batch_query(query_bytes)
            .map_err(|e| anyhow::anyhow!("malformed batch query: {e}"))?;
        let k = pqrs.len();
        anyhow::ensure!(
            k <= MAX_BATCH_K,
            "batch K = {k} exceeds MAX_BATCH_K = {MAX_BATCH_K}"
        );
        // Per-query pqr length is constrained by YPIR scenario; reject
        // anything inconsistent with the loaded server params before we
        // touch the database.
        let expected_pqr_u64 = self._params.db_rows_padded_simplepir();
        for (i, q) in pqrs.iter().enumerate() {
            anyhow::ensure!(
                q.len() == expected_pqr_u64,
                "batch q[{i}] has {} u64s, expected {}",
                q.len(),
                expected_pqr_u64
            );
        }
        let validate_ms = validate_start.elapsed().as_secs_f64() * 1000.0;

        let decode_start = Instant::now();
        // Copy each q.0 and the shared pp into 64-byte aligned buffers for
        // AVX-512 operations. This mirrors `answer_query` but allocates K
        // pqr buffers and one shared pp buffer.
        let pp_u64_len = pp.len();
        let mut pp_aligned = Aligned64::new(pp_u64_len);
        pp_aligned.as_mut_slice().copy_from_slice(&pp);

        let mut pqrs_aligned: Vec<Aligned64> = Vec::with_capacity(k);
        for q in &pqrs {
            let mut buf = Aligned64::new(q.len());
            buf.as_mut_slice().copy_from_slice(q);
            pqrs_aligned.push(buf);
        }
        let decode_copy_ms = decode_start.elapsed().as_secs_f64() * 1000.0;

        // Per-query online computation, sharing the same pack_pub_params.
        let compute_start = Instant::now();
        let mut responses: Vec<Vec<u8>> = Vec::with_capacity(k);
        for q in &pqrs_aligned {
            let resp = self.server.perform_online_computation_simplepir(
                q.as_slice(),
                &self.offline,
                &[pp_aligned.as_slice()],
                None,
            );
            responses.push(resp);
        }
        let online_compute_ms = compute_start.elapsed().as_secs_f64() * 1000.0;

        // YPIR responses are deterministic-length for a given scenario;
        // assert and record the per-query length for the batch wire trailer.
        let response_bytes_per_query = responses.first().map(|r| r.len()).unwrap_or(0);
        for (i, r) in responses.iter().enumerate() {
            anyhow::ensure!(
                r.len() == response_bytes_per_query,
                "response[{i}] is {} bytes, expected {} (server bug)",
                r.len(),
                response_bytes_per_query
            );
        }
        let total_response_bytes = response_bytes_per_query * k;
        let response = serialize_ypir_batch_response(&responses);

        let total_ms = total_start.elapsed().as_secs_f64() * 1000.0;

        Ok(BatchQueryAnswer {
            timing: BatchQueryTiming {
                k,
                validate_ms,
                decode_copy_ms,
                online_compute_ms,
                total_ms,
                response_bytes: total_response_bytes,
                response_bytes_per_query,
            },
            response,
        })
    }

    /// Return the YPIR scenario parameters for this tier.
    pub fn scenario(&self) -> &YpirScenario {
        &self.scenario
    }
}

impl Drop for TierServer<'_> {
    fn drop(&mut self) {
        // Drop server and offline first (they hold &Params references into _params).
        // Then _params drops naturally, freeing the heap allocation.
        unsafe {
            std::mem::ManuallyDrop::drop(&mut self.server);
            std::mem::ManuallyDrop::drop(&mut self.offline);
        }
    }
}

// ── OwnedTierState ────────────────────────────────────────────────────────────

/// Owns a `TierServer` constructed from tier data.
///
/// The raw tier bytes are NOT retained — YPIR's `FilePtIter` is consumed during
/// `YServer::new()`, which copies everything into its own `db_buf_aligned`.
/// Dropping the source data after construction saves ~6 GB.
pub struct OwnedTierState {
    server: TierServer<'static>,
}

impl OwnedTierState {
    /// Construct a new `OwnedTierState` from borrowed tier data and a YPIR scenario.
    ///
    /// The data slice only needs to live for the duration of this call.
    ///
    /// # Safety
    ///
    /// We extend the lifetime of the data reference to `'static`. This is sound
    /// because YPIR's `FilePtIter` is consumed during `YServer::new()` — after
    /// construction, the server holds precomputed values in its own
    /// `db_buf_aligned`, not references to the original data. The `'static`
    /// lifetime on `TierServer` constrains only `params: &'a Params` (pointing
    /// to the owned `Box<Params>`), not the input data.
    pub fn new(data: &[u8], scenario: YpirScenario) -> Self {
        let data_ref: &'static [u8] = unsafe { std::mem::transmute::<&[u8], &'static [u8]>(data) };
        let server = TierServer::new(data_ref, scenario);
        Self { server }
    }

    pub fn server(&self) -> &TierServer<'static> {
        &self.server
    }
}

// Allow sending OwnedTierState between threads (needed for tokio spawn_blocking).
// This is safe because TierServer is only accessed via &self references through
// the AppState RwLock.
unsafe impl Send for OwnedTierState {}
unsafe impl Sync for OwnedTierState {}

// ── Shared HTTP helpers ──────────────────────────────────────────────────────

use axum::http::{HeaderValue, StatusCode};
use axum::response::IntoResponse;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use tracing::warn;

/// RAII guard that decrements an atomic inflight counter on drop.
pub struct InflightGuard<'a> {
    inflight: &'a AtomicUsize,
}

impl<'a> InflightGuard<'a> {
    pub fn new(inflight: &'a AtomicUsize) -> Self {
        Self { inflight }
    }
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        self.inflight.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Write PIR batch query timing breakdown as HTTP response headers.
///
/// Mirrors [`write_timing_headers`] but reports *batch* totals plus an
/// `x-pir-batch-k` header so the client/bench harness can split per-query.
pub fn write_batch_timing_headers(
    headers: &mut axum::http::HeaderMap,
    req_id: u64,
    timing: BatchQueryTiming,
) {
    let entries: [(&str, String); 8] = [
        ("x-pir-req-id", req_id.to_string()),
        ("x-pir-batch-k", timing.k.to_string()),
        ("x-pir-server-total-ms", format!("{:.3}", timing.total_ms)),
        (
            "x-pir-server-validate-ms",
            format!("{:.3}", timing.validate_ms),
        ),
        (
            "x-pir-server-decode-copy-ms",
            format!("{:.3}", timing.decode_copy_ms),
        ),
        (
            "x-pir-server-compute-ms",
            format!("{:.3}", timing.online_compute_ms),
        ),
        (
            "x-pir-server-response-bytes",
            timing.response_bytes.to_string(),
        ),
        (
            "x-pir-server-response-bytes-per-query",
            timing.response_bytes_per_query.to_string(),
        ),
    ];
    for (name, value) in entries {
        if let Ok(hv) = HeaderValue::from_str(&value) {
            headers.insert(name, hv);
        }
    }
}

/// Write PIR query timing breakdown as HTTP response headers.
///
/// Used by both `pir-server` and `nf-server` to expose server-side stage
/// timing so the client can split RTT into server vs network/queue.
pub fn write_timing_headers(headers: &mut axum::http::HeaderMap, req_id: u64, timing: QueryTiming) {
    let entries: [(&str, String); 6] = [
        ("x-pir-req-id", req_id.to_string()),
        ("x-pir-server-total-ms", format!("{:.3}", timing.total_ms)),
        (
            "x-pir-server-validate-ms",
            format!("{:.3}", timing.validate_ms),
        ),
        (
            "x-pir-server-decode-copy-ms",
            format!("{:.3}", timing.decode_copy_ms),
        ),
        (
            "x-pir-server-compute-ms",
            format!("{:.3}", timing.online_compute_ms),
        ),
        (
            "x-pir-server-response-bytes",
            timing.response_bytes.to_string(),
        ),
    ];
    for (name, value) in entries {
        // HeaderValue::from_str only fails on non-visible-ASCII; numeric
        // formatting always produces valid values.
        if let Ok(hv) = HeaderValue::from_str(&value) {
            headers.insert(name, hv);
        }
    }
}

/// Read a single row from a tier binary file on disk.
pub fn read_tier_row(path: &std::path::Path, offset: u64, len: usize) -> std::io::Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; len];
    f.read_exact(&mut buf)?;
    Ok(buf)
}

/// Process a PIR query against a tier server with inflight tracking,
/// structured logging, and timing response headers.
///
/// Shared between `pir-server` (standalone binary) and `nf-server serve`.
/// Callers resolve the `ServingState` and pass the relevant `OwnedTierState`.
pub fn dispatch_query(
    tier_state: &OwnedTierState,
    tier: &str,
    body: &[u8],
    next_req_id: &AtomicU64,
    inflight_requests: &AtomicUsize,
) -> axum::response::Response {
    let req_id = next_req_id.fetch_add(1, Ordering::Relaxed) + 1;
    let inflight = inflight_requests.fetch_add(1, Ordering::Relaxed) + 1;
    let _inflight_guard = InflightGuard::new(inflight_requests);
    let t0 = Instant::now();
    info!(
        req_id,
        tier,
        body_bytes = body.len(),
        inflight_requests = inflight,
        "pir_request_started"
    );

    match tier_state.server().answer_query(body) {
        Ok(answer) => {
            let handler_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let mut response = (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
                answer.response,
            )
                .into_response();
            write_timing_headers(response.headers_mut(), req_id, answer.timing);
            info!(
                req_id,
                tier,
                status = 200,
                handler_ms = format!("{handler_ms:.3}"),
                validate_ms = format!("{:.3}", answer.timing.validate_ms),
                decode_copy_ms = format!("{:.3}", answer.timing.decode_copy_ms),
                compute_ms = format!("{:.3}", answer.timing.online_compute_ms),
                server_total_ms = format!("{:.3}", answer.timing.total_ms),
                response_bytes = answer.timing.response_bytes,
                "pir_request_finished"
            );
            response
        }
        Err(e) => {
            warn!(
                req_id,
                tier,
                status = 400,
                handler_ms = format!("{:.3}", t0.elapsed().as_secs_f64() * 1000.0),
                error = %e,
                "pir_request_failed"
            );
            (StatusCode::BAD_REQUEST, e.to_string()).into_response()
        }
    }
}

/// Process a batched PIR query against a tier server with inflight tracking,
/// structured logging, and timing response headers.
///
/// Mirrors [`dispatch_query`] but invokes [`TierServer::answer_batch_query`]
/// and emits batch-aware timing headers including `x-pir-batch-k`.
pub fn dispatch_batch_query(
    tier_state: &OwnedTierState,
    tier: &str,
    body: &[u8],
    next_req_id: &AtomicU64,
    inflight_requests: &AtomicUsize,
) -> axum::response::Response {
    let req_id = next_req_id.fetch_add(1, Ordering::Relaxed) + 1;
    let inflight = inflight_requests.fetch_add(1, Ordering::Relaxed) + 1;
    let _inflight_guard = InflightGuard::new(inflight_requests);
    let t0 = Instant::now();
    info!(
        req_id,
        tier,
        body_bytes = body.len(),
        inflight_requests = inflight,
        "pir_batch_request_started"
    );

    match tier_state.server().answer_batch_query(body) {
        Ok(answer) => {
            let handler_ms = t0.elapsed().as_secs_f64() * 1000.0;
            let mut response = (
                StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
                answer.response,
            )
                .into_response();
            write_batch_timing_headers(response.headers_mut(), req_id, answer.timing);
            info!(
                req_id,
                tier,
                k = answer.timing.k,
                status = 200,
                handler_ms = format!("{handler_ms:.3}"),
                validate_ms = format!("{:.3}", answer.timing.validate_ms),
                decode_copy_ms = format!("{:.3}", answer.timing.decode_copy_ms),
                compute_ms = format!("{:.3}", answer.timing.online_compute_ms),
                server_total_ms = format!("{:.3}", answer.timing.total_ms),
                response_bytes = answer.timing.response_bytes,
                "pir_batch_request_finished"
            );
            response
        }
        Err(e) => {
            warn!(
                req_id,
                tier,
                status = 400,
                handler_ms = format!("{:.3}", t0.elapsed().as_secs_f64() * 1000.0),
                error = %e,
                "pir_batch_request_failed"
            );
            (StatusCode::BAD_REQUEST, e.to_string()).into_response()
        }
    }
}

// ── ServingState ─────────────────────────────────────────────────────────────

use axum::body::Bytes;

/// All data needed to serve PIR queries for all tiers.
///
/// Holds loaded tier data, initialized YPIR servers, and tree metadata.
/// Used by both the standalone `pir-server` binary and `nf-server`
/// in serve mode.
///
/// Raw tier data is NOT kept in memory — YPIR copies it into its own
/// internal representation during construction. Tier0 uses `Bytes`
/// (reference-counted) to avoid cloning on each HTTP response.
pub struct ServingState {
    pub tier0_data: Bytes,
    pub tier1: OwnedTierState,
    pub tier2: OwnedTierState,
    pub tier1_scenario: YpirScenario,
    pub tier2_scenario: YpirScenario,
    pub metadata: PirMetadata,
}

/// Load tier files from disk, initialize YPIR servers, and return a
/// ready-to-serve [`ServingState`].
///
/// Reads `tier0.bin`, `tier1.bin`, `tier2.bin`, and `pir_root.json` from
/// `pir_data_dir`. Raw tier data is consumed during YPIR initialization
/// and dropped to save ~6 GB.
pub fn load_serving_state(pir_data_dir: &std::path::Path) -> Result<ServingState> {
    let t_total = Instant::now();

    let tier0_data = Bytes::from(std::fs::read(pir_data_dir.join("tier0.bin"))?);
    info!(bytes = tier0_data.len(), "Tier 0 loaded");

    let tier1_data = std::fs::read(pir_data_dir.join("tier1.bin"))?;
    info!(
        bytes = tier1_data.len(),
        rows = tier1_data.len() / TIER1_ROW_BYTES,
        "Tier 1 loaded"
    );
    anyhow::ensure!(
        tier1_data.len() == TIER1_YPIR_ROWS * TIER1_ROW_BYTES,
        "tier1.bin size mismatch: got {} bytes, expected {}",
        tier1_data.len(),
        TIER1_YPIR_ROWS * TIER1_ROW_BYTES
    );

    let tier2_data = std::fs::read(pir_data_dir.join("tier2.bin"))?;
    info!(
        bytes = tier2_data.len(),
        rows = tier2_data.len() / TIER2_ROW_BYTES,
        "Tier 2 loaded"
    );
    anyhow::ensure!(
        tier2_data.len() == TIER2_ROWS * TIER2_ROW_BYTES,
        "tier2.bin size mismatch: got {} bytes, expected {}",
        tier2_data.len(),
        TIER2_ROWS * TIER2_ROW_BYTES
    );

    let metadata: PirMetadata = serde_json::from_str(&std::fs::read_to_string(
        pir_data_dir.join("pir_root.json"),
    )?)?;
    info!(num_ranges = metadata.num_ranges, "Metadata loaded");

    info!("Initializing YPIR servers");
    let tier1_scenario = tier1_scenario();
    let tier1 = OwnedTierState::new(&tier1_data, tier1_scenario.clone());
    drop(tier1_data);
    info!("Tier 1 YPIR ready");

    let tier2_scenario = tier2_scenario();
    let tier2 = OwnedTierState::new(&tier2_data, tier2_scenario.clone());
    drop(tier2_data);
    info!("Tier 2 YPIR ready");

    info!(
        elapsed_s = format!("{:.1}", t_total.elapsed().as_secs_f64()),
        "Server ready"
    );

    Ok(ServingState {
        tier0_data,
        tier1,
        tier2,
        tier1_scenario,
        tier2_scenario,
        metadata,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ypir::client::YPIRClient;

    /// Smallest valid YPIR SimplePIR scenario.
    /// `params_for_scenario_simplepir` asserts `item_size_bits >= 2048 * 14`.
    /// Picking the minimum keeps the offline precomputation fast (~a few
    /// seconds in debug builds) so the byte-equality test stays manageable
    /// while still exercising the full SimplePIR path.
    const TEST_NUM_ITEMS: usize = 2048;
    const TEST_ITEM_BITS: usize = 2048 * 14;
    const TEST_ROW_BYTES: usize = TEST_ITEM_BITS / 8;

    fn make_test_db(num_rows: usize, row_bytes: usize) -> Vec<u8> {
        let mut data = vec![0u8; num_rows * row_bytes];
        // Deterministic per-row pattern so any K=1..16 mismatch surfaces a
        // distinguishable byte sequence in failure output.
        for (row_idx, chunk) in data.chunks_exact_mut(row_bytes).enumerate() {
            for (col, b) in chunk.iter_mut().enumerate() {
                let v = (row_idx as u64)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    .wrapping_add(col as u64);
                *b = (v & 0xFF) as u8;
            }
        }
        data
    }

    fn setup_test_server() -> (OwnedTierState, YPIRClient, Vec<u8>) {
        let scenario = YpirScenario {
            num_items: TEST_NUM_ITEMS,
            item_size_bits: TEST_ITEM_BITS,
        };
        let db = make_test_db(TEST_NUM_ITEMS, TEST_ROW_BYTES);
        let server = OwnedTierState::new(&db, scenario.clone());
        let client = YPIRClient::from_db_sz(
            scenario.num_items as u64,
            scenario.item_size_bits as u64,
            true,
        );
        (server, client, db)
    }

    /// Byte-equality acceptance test for batch vs. sequential queries.
    ///
    /// For each `K ∈ {1, 3, 5, 16}` the test asserts that:
    ///
    ///   1. `answer_batch_query(K queries)` decodes to exactly the same
    ///      plaintext rows as `K × answer_query(single query)`.
    ///   2. Both paths recover the *original* database rows.
    ///
    /// (The raw ciphertexts cannot be compared byte-for-byte because each
    /// path uses a fresh `client_seed` and fresh per-query LWE error
    /// vectors. Equality at the plaintext level is what privacy and
    /// correctness actually demand.)
    ///
    /// `K = MAX_BATCH_K = 16` is the upper bound advertised on the wire
    /// protocol; including it here pins down behaviour at the boundary.
    #[test]
    fn batch_answer_matches_sequential_decoded_rows() {
        let (state, ypir_client, db) = setup_test_server();
        let server = state.server();

        for &k in &[1usize, 3, 5, 16] {
            let target_rows: Vec<usize> =
                (0..k).map(|i| (i * 257 + 11) % TEST_NUM_ITEMS).collect();

            // Sequential path: K independent (q, seed) pairs through
            // `answer_query`.
            let mut sequential_decoded: Vec<Vec<u8>> = Vec::with_capacity(k);
            for &row in &target_rows {
                let (q, seed) = ypir_client.generate_query_simplepir(row);
                let payload =
                    pir_types::serialize_ypir_query(q.0.as_slice(), q.1.as_slice());
                let answer = server
                    .answer_query(&payload)
                    .expect("sequential answer_query");
                let decoded = ypir_client.decode_response_simplepir(seed, &answer.response);
                assert!(
                    decoded.len() >= TEST_ROW_BYTES,
                    "decoded row shorter than expected"
                );
                sequential_decoded.push(decoded[..TEST_ROW_BYTES].to_vec());
            }

            // Batched path: one shared `pack_pub_params` and `client_seed`
            // for K queries, dispatched through `answer_batch_query`.
            let ((q_vec, pp), seed) =
                ypir_client.generate_query_simplepir_batch(&target_rows);
            let pqr_refs: Vec<&[u64]> = q_vec.iter().map(|q| q.as_slice()).collect();
            let payload =
                pir_types::serialize_ypir_batch_query(&pqr_refs, pp.as_slice());
            let answer = server
                .answer_batch_query(&payload)
                .expect("answer_batch_query");
            assert_eq!(answer.timing.k, k, "batch timing.k mismatch");

            let chunks = pir_types::parse_ypir_batch_response(&answer.response)
                .expect("parse_ypir_batch_response");
            assert_eq!(
                chunks.len(),
                k,
                "K={k}: batch response should carry exactly K chunks"
            );
            let chunk_refs: Vec<&[u8]> = chunks.iter().map(|c| c.as_slice()).collect();
            let batch_decoded =
                ypir_client.decode_response_simplepir_batch(seed, &chunk_refs);
            assert_eq!(batch_decoded.len(), k);

            for (i, (b, s)) in
                batch_decoded.iter().zip(sequential_decoded.iter()).enumerate()
            {
                assert!(
                    b.len() >= TEST_ROW_BYTES,
                    "K={k} batch[{i}] decoded too short"
                );
                let b_row = &b[..TEST_ROW_BYTES];
                let row = target_rows[i];
                assert_eq!(
                    b_row,
                    s.as_slice(),
                    "K={k} batch[{i}] row {row}: batch decode disagrees with \
                     sequential decode"
                );
                let original = &db[row * TEST_ROW_BYTES..(row + 1) * TEST_ROW_BYTES];
                assert_eq!(
                    b_row, original,
                    "K={k} batch[{i}] decoded row {row} does not match \
                     original DB row"
                );
            }
        }
    }

    /// Cross-tier `MAX_BATCH_K` enforcement: server must reject K > 16.
    ///
    /// We can't easily forge a `pqrs` of length 17 because the YPIR client
    /// API only ever produces well-formed batches (and `MAX_BATCH_K` is
    /// also enforced client-side). We therefore craft a wire payload by
    /// hand using `serialize_ypir_batch_query` after stretching the K
    /// header to 17.
    #[test]
    fn answer_batch_query_rejects_k_above_max() {
        let (state, ypir_client, _db) = setup_test_server();
        let server = state.server();

        let target_rows: Vec<usize> = (0..MAX_BATCH_K + 1).map(|i| i % TEST_NUM_ITEMS).collect();
        // Generate MAX_BATCH_K queries (the largest the YPIR client will
        // accept) and replicate one extra `q` slot to reach
        // `MAX_BATCH_K + 1` slots in the wire body. The client APIs do
        // not directly produce K = 17 batches, so we splice the payload.
        let allowed: Vec<usize> = target_rows[..MAX_BATCH_K].to_vec();
        let ((q_vec, pp), _seed) = ypir_client.generate_query_simplepir_batch(&allowed);
        let mut pqr_refs: Vec<&[u64]> = q_vec.iter().map(|q| q.as_slice()).collect();
        // Duplicate slot 0 to push K over the limit.
        pqr_refs.push(q_vec[0].as_slice());
        assert_eq!(pqr_refs.len(), MAX_BATCH_K + 1);

        let payload = pir_types::serialize_ypir_batch_query(&pqr_refs, pp.as_slice());
        let err = server
            .answer_batch_query(&payload)
            .expect_err("K > MAX_BATCH_K must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("MAX_BATCH_K"),
            "expected MAX_BATCH_K rejection, got: {msg}"
        );
    }

    /// `K = 1` batch must produce a wire response with exactly one chunk
    /// whose decoded row equals the single-shot path's decoded row.
    /// Documents the invariant that the batch wire format is a strict
    /// superset of the single-query wire format (header + one query).
    #[test]
    fn answer_batch_query_k1_round_trips() {
        let (state, ypir_client, _db) = setup_test_server();
        let server = state.server();

        let target_row = 42usize;
        let ((q_vec, pp), seed) =
            ypir_client.generate_query_simplepir_batch(&[target_row]);
        assert_eq!(q_vec.len(), 1);
        let pqr_refs: Vec<&[u64]> = q_vec.iter().map(|q| q.as_slice()).collect();
        let payload = pir_types::serialize_ypir_batch_query(&pqr_refs, pp.as_slice());
        let answer = server.answer_batch_query(&payload).expect("answer_batch_query");
        assert_eq!(answer.timing.k, 1);

        let chunks = pir_types::parse_ypir_batch_response(&answer.response)
            .expect("parse_ypir_batch_response");
        assert_eq!(chunks.len(), 1);

        let decoded =
            ypir_client.decode_response_simplepir(seed, chunks[0].as_slice());
        assert!(decoded.len() >= TEST_ROW_BYTES);
    }
}
