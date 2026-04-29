//! YPIR+SP server wrapper and shared types for the PIR HTTP server.
//!
//! This module encapsulates all YPIR operations, providing a clean interface
//! that both the HTTP server (`main.rs`) and the test harness (`pir-test`)
//! can use.

use anyhow::{Context, Result};
use std::io::Cursor;
use std::path::Path;
use std::time::Instant;
use tracing::{info, warn};

use std::alloc::{alloc_zeroed, dealloc, handle_alloc_error, Layout};

use spiral_rs::params::Params;
use ypir::params::{params_for_scenario_simplepir, DbRowsCols, PtModulusBits};
use ypir::serialize::{FilePtIter, OfflinePrecomputedValues};
use ypir::server::YServer;

pub mod precompute_cache;

// Re-export shared types and constants so existing consumers can import from pir_server.
pub use pir_types::{
    HealthInfo, PirMetadata, RootInfo, YpirScenario, TIER1_ITEM_BITS, TIER1_ROWS, TIER1_ROW_BYTES,
    TIER1_YPIR_ROWS, TIER2_ITEM_BITS, TIER2_ROWS, TIER2_ROW_BYTES,
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

impl<'a> TierServer<'a> {
    /// Build the YPIR `(YServer, OfflinePrecomputedValues)` pair from raw
    /// tier bytes against a pre-allocated `Params`. Slow path: ~30s for
    /// tier 1, ~90-120s for tier 2; this is what the precompute cache
    /// (see [`OwnedTierState::new_or_load`]) skips on subsequent boots.
    ///
    /// Factored out so the cache-miss path in `OwnedTierState::new_or_load`
    /// can drive the same construction without allocating its own `Params`.
    fn build_ypir_state(
        params: &'a Params,
        data: &'a [u8],
        scenario: &YpirScenario,
    ) -> (YServer<'a, u16>, OfflinePrecomputedValues<'a>) {
        let t0 = Instant::now();
        info!(
            num_items = scenario.num_items,
            item_size_bits = scenario.item_size_bits,
            "YPIR server init"
        );

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

        (server, offline)
    }

    /// Allocate a fresh `Params` box for this scenario. Caller is
    /// responsible for keeping it alive longer than any borrows.
    fn alloc_params(scenario: &YpirScenario) -> Box<Params> {
        Box::new(params_for_scenario_simplepir(
            scenario.num_items as u64,
            scenario.item_size_bits as u64,
        ))
    }

    /// Wrap pre-built YPIR state (e.g. from `build_ypir_state` or from a
    /// cache load) in a `TierServer`. The caller must guarantee that
    /// `server` and `offline` borrow from `params_box` (so the box must
    /// outlive them, which the `ManuallyDrop` + Drop impl ensures).
    fn wrap(
        server: YServer<'a, u16>,
        offline: OfflinePrecomputedValues<'a>,
        scenario: YpirScenario,
        params_box: Box<Params>,
    ) -> Self {
        Self {
            server: std::mem::ManuallyDrop::new(server),
            offline: std::mem::ManuallyDrop::new(offline),
            _params: params_box,
            scenario,
        }
    }

    /// Initialize a YPIR+SP server from raw tier data.
    ///
    /// `data` is the flat binary tier file (rows × row_bytes).
    /// This performs the expensive offline precomputation. Used by
    /// in-process callers (tests) that don't want the cache. Server-side
    /// startup uses [`OwnedTierState::new_or_load`] instead.
    pub fn new(data: &'a [u8], scenario: YpirScenario) -> Self {
        let params_box = Self::alloc_params(&scenario);
        // SAFETY: We extend the reference lifetime to 'a. Sound because
        // params_box is a heap allocation with a stable address, and
        // server/offline are dropped before _params in our Drop impl.
        let params: &'a Params =
            unsafe { std::mem::transmute::<&Params, &'a Params>(params_box.as_ref()) };
        let (server, offline) = Self::build_ypir_state(params, data, &scenario);
        Self::wrap(server, offline, scenario, params_box)
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
    /// Used by in-process callers (tests) that don't want the warm-restart
    /// cache. Server-side startup should use [`OwnedTierState::new_or_load`]
    /// instead so that subsequent boots skip the ~2-minute YPIR setup.
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

    /// Load YPIR state for one tier, preferring the on-disk precompute cache
    /// at `cache_path` over re-running the ~2-minute YPIR setup. Cache miss
    /// (file absent / hash mismatch / corrupt / version mismatch) falls back
    /// to the slow path and atomically writes the cache for next boot.
    ///
    /// Returns `(state, cache_hit)`. The `cache_hit` flag is purely
    /// informational; used by `load_serving_state` for hit/miss logging.
    /// The cache write at the end is best-effort: ENOSPC / EROFS / I/O
    /// errors during the dump log a warning and return the (correct) state
    /// anyway. The server stays correct; we just lose the optimization for
    /// next boot.
    pub fn new_or_load(
        tier_path: &Path,
        scenario: YpirScenario,
        cache_path: &Path,
    ) -> Result<(Self, bool)> {
        // Allocate Params first so both the cache-hit and cache-miss paths
        // can hand it to the YPIR loader/builder against the same address.
        let params_box = TierServer::alloc_params(&scenario);
        // SAFETY: see `TierServer::new`; params_box stable address; YServer
        // and OfflinePrecomputedValues are dropped before _params via the
        // ManuallyDrop pattern in `TierServer::Drop`.
        let params: &'static Params =
            unsafe { std::mem::transmute::<&Params, &'static Params>(params_box.as_ref()) };

        let t_load = Instant::now();
        match precompute_cache::try_load_cache(cache_path, tier_path, &scenario, params) {
            Ok(loaded) => {
                info!(
                    cache = %cache_path.display(),
                    elapsed_s = format!("{:.1}", t_load.elapsed().as_secs_f64()),
                    "precompute cache hit"
                );
                let server = TierServer::wrap(loaded.server, loaded.offline, scenario, params_box);
                return Ok((Self { server }, true));
            }
            Err(reason) => {
                info!(
                    cache = %cache_path.display(),
                    reason = %reason,
                    "precompute cache miss; recomputing"
                );
            }
        }

        // Slow path: read tier data, run YPIR setup, write cache.
        let tier_data = std::fs::read(tier_path)
            .with_context(|| format!("read tier file {}", tier_path.display()))?;

        // Hash the EXACT buffer we're about to feed YPIR. write_cache takes
        // this hash as a parameter rather than re-reading tier_path, which
        // closes a TOCTOU window: if tier_path were replaced during the long
        // YPIR setup, re-hashing the file later would record the new tier
        // hash with payload built from old bytes -- and the next load would
        // silently accept the cache and serve wrong responses against the
        // mismatched tier file. Hashing the in-memory buffer guarantees the
        // recorded hash describes the bytes the payload was built from.
        let tier_source_hash: [u8; 32] = blake3::hash(&tier_data).into();

        // SAFETY: see `OwnedTierState::new`; tier_data is moved into this
        // function and lives until after build_ypir_state returns; YPIR's
        // FilePtIter consumes it before then, so the &'static borrow does
        // not outlive the underlying buffer.
        let data_ref: &'static [u8] =
            unsafe { std::mem::transmute::<&[u8], &'static [u8]>(&tier_data[..]) };
        let (server, offline) = TierServer::build_ypir_state(params, data_ref, &scenario);

        // Best-effort cache write. Failure is logged but does not fail
        // startup; caching is an optimization.
        if let Err(e) = precompute_cache::write_cache(
            cache_path,
            &tier_source_hash,
            &scenario,
            &server,
            &offline,
        ) {
            warn!(
                cache = %cache_path.display(),
                error = %e,
                "precompute cache write failed; serving from memory, next boot will recompute"
            );
        } else {
            info!(cache = %cache_path.display(), "precompute cache written");
        }

        // Drop tier_data only after cache write completes (write_cache borrows
        // server/offline which still hold the &'static slice). After this
        // function returns, the YPIR state has copied everything into its
        // own buffers.
        drop(tier_data);

        let server = TierServer::wrap(server, offline, scenario, params_box);
        Ok((Self { server }, false))
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

/// On-disk file name for the tier-1 plaintext rows.
pub const TIER1_FILE: &str = "tier1.bin";
/// On-disk file name for the tier-2 plaintext rows.
pub const TIER2_FILE: &str = "tier2.bin";
/// On-disk file name for the tier-1 precompute cache.
pub const TIER1_PRECOMPUTE_FILE: &str = "tier1.precompute";
/// On-disk file name for the tier-2 precompute cache.
pub const TIER2_PRECOMPUTE_FILE: &str = "tier2.precompute";

/// Generate or validate the on-disk precompute caches for both YPIR tiers.
///
/// This is used by snapshot publishing to materialize cache artifacts beside a
/// freshly exported `tier{1,2}.bin` snapshot. It intentionally goes through
/// the same cache-aware path as server startup, so existing valid caches are
/// reused and missing/stale caches are regenerated with the production cache
/// header format.
pub fn generate_precompute_caches(pir_data_dir: &std::path::Path) -> Result<()> {
    let tier1_path = pir_data_dir.join(TIER1_FILE);
    let tier1_size = std::fs::metadata(&tier1_path)?.len() as usize;
    anyhow::ensure!(
        tier1_size == TIER1_YPIR_ROWS * TIER1_ROW_BYTES,
        "tier1.bin size mismatch: got {} bytes, expected {}",
        tier1_size,
        TIER1_YPIR_ROWS * TIER1_ROW_BYTES
    );
    let tier1_cache_path = pir_data_dir.join(TIER1_PRECOMPUTE_FILE);
    let (_, tier1_hit) =
        OwnedTierState::new_or_load(&tier1_path, tier1_scenario(), &tier1_cache_path)?;
    info!(cache_hit = tier1_hit, "Tier 1 precompute cache ready");

    let tier2_path = pir_data_dir.join(TIER2_FILE);
    let tier2_size = std::fs::metadata(&tier2_path)?.len() as usize;
    anyhow::ensure!(
        tier2_size == TIER2_ROWS * TIER2_ROW_BYTES,
        "tier2.bin size mismatch: got {} bytes, expected {}",
        tier2_size,
        TIER2_ROWS * TIER2_ROW_BYTES
    );
    let tier2_cache_path = pir_data_dir.join(TIER2_PRECOMPUTE_FILE);
    let (_, tier2_hit) =
        OwnedTierState::new_or_load(&tier2_path, tier2_scenario(), &tier2_cache_path)?;
    info!(cache_hit = tier2_hit, "Tier 2 precompute cache ready");

    Ok(())
}

/// Load tier files from disk, initialize YPIR servers, and return a
/// ready-to-serve [`ServingState`].
///
/// Reads `tier0.bin`, `tier1.bin`, `tier2.bin`, and `pir_root.json` from
/// `pir_data_dir`, plus the precompute caches `tier{1,2}.precompute` if
/// present and valid. Cache miss falls back to recompute and writes a
/// fresh cache for next boot.
pub fn load_serving_state(pir_data_dir: &std::path::Path) -> Result<ServingState> {
    let t_total = Instant::now();

    let tier0_data = Bytes::from(std::fs::read(pir_data_dir.join("tier0.bin"))?);
    info!(bytes = tier0_data.len(), "Tier 0 loaded");

    // Validate tier{1,2}.bin sizes BEFORE attempting cache load. Cache
    // validation hashes the tier file but doesn't constrain its size; a
    // malformed tier file must be rejected up front because the server
    // still serves rows directly from tier{0,1,2}.bin for some operations.
    let tier1_path = pir_data_dir.join(TIER1_FILE);
    let tier1_size = std::fs::metadata(&tier1_path)?.len() as usize;
    info!(
        bytes = tier1_size,
        rows = tier1_size / TIER1_ROW_BYTES,
        "Tier 1 sized"
    );
    anyhow::ensure!(
        tier1_size == TIER1_YPIR_ROWS * TIER1_ROW_BYTES,
        "tier1.bin size mismatch: got {} bytes, expected {}",
        tier1_size,
        TIER1_YPIR_ROWS * TIER1_ROW_BYTES
    );

    let tier2_path = pir_data_dir.join(TIER2_FILE);
    let tier2_size = std::fs::metadata(&tier2_path)?.len() as usize;
    info!(
        bytes = tier2_size,
        rows = tier2_size / TIER2_ROW_BYTES,
        "Tier 2 sized"
    );
    anyhow::ensure!(
        tier2_size == TIER2_ROWS * TIER2_ROW_BYTES,
        "tier2.bin size mismatch: got {} bytes, expected {}",
        tier2_size,
        TIER2_ROWS * TIER2_ROW_BYTES
    );

    let metadata: PirMetadata = serde_json::from_str(&std::fs::read_to_string(
        pir_data_dir.join("pir_root.json"),
    )?)?;
    info!(num_ranges = metadata.num_ranges, "Metadata loaded");

    info!("Initializing YPIR servers");
    let tier1_scenario = tier1_scenario();
    let tier1_cache_path = pir_data_dir.join(TIER1_PRECOMPUTE_FILE);
    let (tier1, tier1_hit) =
        OwnedTierState::new_or_load(&tier1_path, tier1_scenario.clone(), &tier1_cache_path)?;
    info!(cache_hit = tier1_hit, "Tier 1 YPIR ready");

    let tier2_scenario = tier2_scenario();
    let tier2_cache_path = pir_data_dir.join(TIER2_PRECOMPUTE_FILE);
    let (tier2, tier2_hit) =
        OwnedTierState::new_or_load(&tier2_path, tier2_scenario.clone(), &tier2_cache_path)?;
    info!(cache_hit = tier2_hit, "Tier 2 YPIR ready");

    info!(
        elapsed_s = format!("{:.1}", t_total.elapsed().as_secs_f64()),
        tier1_cache_hit = tier1_hit,
        tier2_cache_hit = tier2_hit,
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
