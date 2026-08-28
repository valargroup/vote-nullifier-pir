//! HTTP handlers for the PIR server.
//!
//! Each handler acquires the shared [`AppState`] and returns 503 if the
//! server is currently rebuilding its snapshot. The YPIR query endpoints
//! track inflight request counts for backpressure monitoring.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;

use pir_server::{dispatch_query, read_tier_row, HealthInfo, RootInfo};

use crate::metrics;

use super::state::{AppState, ServerPhase};

// ── PIR data endpoints ───────────────────────────────────────────────────────

/// `GET /tier0` — Return the full Tier 0 binary blob (plaintext, small).
pub(crate) async fn get_tier0(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let guard = require_serving!(state);
    let s = guard.as_ref().expect("guaranteed Some by require_serving");
    (
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        s.tier0_data.clone(),
    )
        .into_response()
}

/// `GET /params/tier1` — Return the Tier 1 YPIR scenario parameters as JSON.
pub(crate) async fn get_params_tier1(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let guard = require_serving!(state);
    let s = guard.as_ref().expect("guaranteed Some by require_serving");
    axum::Json(s.tier1_scenario.clone()).into_response()
}

// ── YPIR query endpoints ─────────────────────────────────────────────────────

/// `POST /tier1/query` — Process an encrypted YPIR query against Tier 1.
pub(crate) async fn post_tier1_query(
    State(state): State<Arc<AppState>>,
    request_started: Option<Extension<metrics::RequestStarted>>,
    body: Bytes,
) -> impl IntoResponse {
    let _processing = request_started
        .map(|Extension(request_started)| metrics::start_tier1_processing(request_started));
    let guard = require_serving!(state);
    let s = guard.as_ref().expect("guaranteed Some by require_serving");
    dispatch_query(
        &s.tier1,
        "tier1",
        &body,
        &state.next_req_id,
        &state.inflight_requests,
    )
}

// ── Tier row endpoints (raw row reads for debugging) ─────────────────────────

/// `GET /tier1/row/:idx` — Read a raw Tier 1 row from disk (for debugging).
pub(crate) async fn get_tier1_row(
    State(state): State<Arc<AppState>>,
    Path(idx): Path<usize>,
) -> impl IntoResponse {
    let guard = state.serving.read().await;
    let (rows, row_bytes) = match guard.as_ref() {
        Some(s) => (s.metadata.tier1_rows, s.metadata.tier1_row_bytes),
        None => {
            drop(guard);
            let phase = state.phase.read().await;
            let body = serde_json::to_string(&*phase).unwrap_or_default();
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                body,
            )
                .into_response();
        }
    };
    drop(guard);
    get_tier_row(&state, idx, "tier1.bin", rows, row_bytes).await
}

/// Shared handler for raw tier row reads. Validates index bounds and reads
/// the row directly from the tier binary file on disk.
async fn get_tier_row(
    state: &AppState,
    idx: usize,
    filename: &str,
    num_rows: usize,
    row_bytes: usize,
) -> axum::response::Response {
    let guard = state.serving.read().await;
    if guard.is_none() {
        let phase = state.phase.read().await;
        let body = serde_json::to_string(&*phase).unwrap_or_default();
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            body,
        )
            .into_response();
    }
    if idx >= num_rows {
        return (StatusCode::NOT_FOUND, "row index out of range").into_response();
    }
    let path = state.pir_data_dir.join(filename);
    let offset = (idx * row_bytes) as u64;
    match read_tier_row(&path, offset, row_bytes) {
        Ok(row) => (
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            row,
        )
            .into_response(),
        Err(ref e) => {
            sentry::capture_error(e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("read error: {e}"),
            )
                .into_response()
        }
    }
}

// ── Root and health ──────────────────────────────────────────────────────────

/// `GET /root` — Return the current tree root hash and metadata as JSON.
pub(crate) async fn get_root(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let guard = require_serving!(state);
    let s = guard.as_ref().expect("guaranteed Some by require_serving");
    let mut pir_layout = s.metadata.pir_layout;
    // Advertise the process-configured YPIR degree on /root so wallets can
    // authenticate it as part of pir_layout, matching /params/tier1.
    pir_layout.poly_len = s.tier1_scenario.poly_len;
    let info = RootInfo {
        zcash_network: s.metadata.zcash_network,
        nullifier_pool: s.metadata.nullifier_pool.clone(),
        dataset_version: s.metadata.dataset_version,
        circuit_root: s.metadata.circuit_root.clone(),
        pir_root: s.metadata.pir_root.clone(),
        num_ranges: s.metadata.num_ranges,
        pir_layout,
        pir_depth: s.metadata.pir_depth,
        tier1_rows: s.metadata.tier1_rows,
        tier1_row_bytes: s.metadata.tier1_row_bytes,
        height: s.metadata.height,
    };
    axum::Json(info).into_response()
}

/// `GET /health` — Return server health (`status` + tier metadata).
pub(crate) async fn get_health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let phase = state.phase.read().await;
    let serving = state.serving.read().await;
    let tier1_rows = match serving.as_ref() {
        Some(s) => s.tier1_scenario.num_items,
        None => 0,
    };
    let status = match &*phase {
        ServerPhase::Starting { .. } => "starting",
        ServerPhase::Serving => "ok",
        ServerPhase::Rebuilding { .. } => "rebuilding",
        ServerPhase::Error { .. } => "error",
    };

    let info = HealthInfo {
        status: status.to_string(),
        tier1_rows,
        tier1_row_bytes: serving
            .as_ref()
            .map(|s| s.metadata.tier1_row_bytes)
            .unwrap_or(0),
    };
    axum::Json(info)
}

/// `GET /ready` — Return 200 only when the server is serving queries.
pub(crate) async fn get_ready(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let phase = state.phase.read().await;
    match &*phase {
        ServerPhase::Serving => (
            StatusCode::OK,
            axum::Json(serde_json::json!({ "status": "ok" })),
        )
            .into_response(),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            axum::Json(serde_json::json!(&*phase)),
        )
            .into_response(),
    }
}
