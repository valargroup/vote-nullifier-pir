//! Standalone PIR HTTP server binary.
//!
//! This is the simpler, single-purpose alternative to `nf-server serve`.
//! It loads tier files from a directory, initialises YPIR server state,
//! and exposes the same HTTP API endpoints as `nf-server` in serve mode.
//!
//! Usage: `pir-server [PIR_DATA_DIR] [PORT]`

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize};
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::Router;

const MAX_BODY_BYTES: usize = 512 * 1024 * 1024;
const DEFAULT_PORT: u16 = 3001;

use pir_server::{
    dispatch_query, read_tier_row, HealthInfo, RootInfo, ServingState, TIER2_DISABLED_BODY,
};
use tracing::info;

/// Shared application state: loaded tier data plus per-process counters.
struct AppState {
    serving: ServingState,
    data_dir: PathBuf,
    debug_row_endpoints: bool,
    next_req_id: AtomicU64,
    inflight_requests: AtomicUsize,
}

/// Whether the plaintext `/tier{n}/row/:idx` debug endpoints are enabled.
///
/// Off by default: they expose an unauthenticated disk-read primitive and are
/// not privacy-preserving. Set `PIR_DEBUG_ROW_ENDPOINTS=1` to enable.
fn debug_row_endpoints_enabled() -> bool {
    std::env::var("PIR_DEBUG_ROW_ENDPOINTS").is_ok_and(|v| v == "1")
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    let data_dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./pir-data"));
    let port: u16 = match std::env::args().nth(2) {
        Some(s) => s.parse().context("invalid port number")?,
        None => DEFAULT_PORT,
    };
    let network: pir_types::ZcashNetwork = std::env::var("SVOTE_ZCASH_NETWORK")
        .context("SVOTE_ZCASH_NETWORK must be set to main or test")?
        .parse()
        .map_err(anyhow::Error::msg)?;

    info!(dir = ?data_dir, "Loading tier files");
    let serving = pir_server::load_serving_state(&data_dir, network)?;

    let state = Arc::new(AppState {
        serving,
        data_dir: data_dir.clone(),
        debug_row_endpoints: debug_row_endpoints_enabled(),
        next_req_id: AtomicU64::new(0),
        inflight_requests: AtomicUsize::new(0),
    });

    let app = Router::new()
        .route("/tier0", get(get_tier0))
        .route("/params/tier1", get(get_params_tier1))
        .route("/tier1/query", post(post_tier1_query))
        .route("/tier1/row/:idx", get(get_tier1_row))
        .route("/params/tier2", get(get_params_tier2))
        .route("/tier2/query", post(post_tier2_query))
        .route("/tier2/row/:idx", get(get_tier2_row))
        .route("/root", get(get_root))
        .route("/health", get(get_health))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(state);

    let addr = format!("0.0.0.0:{port}");
    info!(addr, "Listening");
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// ── Handlers ─────────────────────────────────────────────────────────────────

async fn get_tier0(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    (
        [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
        state.serving.tier0_data.clone(),
    )
}

async fn get_params_tier1(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    axum::Json(state.serving.tier1.scenario.clone())
}

async fn post_tier1_query(State(state): State<Arc<AppState>>, body: Bytes) -> impl IntoResponse {
    dispatch_query(
        &state.serving.tier1.state,
        "tier1",
        &body,
        &state.next_req_id,
        &state.inflight_requests,
    )
}

async fn get_params_tier2(State(state): State<Arc<AppState>>) -> axum::response::Response {
    match &state.serving.tier2 {
        Some(tier2) => axum::Json(tier2.scenario.clone()).into_response(),
        None => (StatusCode::NOT_FOUND, TIER2_DISABLED_BODY).into_response(),
    }
}

async fn post_tier2_query(
    State(state): State<Arc<AppState>>,
    body: Bytes,
) -> axum::response::Response {
    match &state.serving.tier2 {
        Some(tier2) => dispatch_query(
            &tier2.state,
            "tier2",
            &body,
            &state.next_req_id,
            &state.inflight_requests,
        ),
        None => (StatusCode::NOT_FOUND, TIER2_DISABLED_BODY).into_response(),
    }
}

async fn get_tier1_row(
    State(state): State<Arc<AppState>>,
    Path(idx): Path<usize>,
) -> impl IntoResponse {
    let scenario = state.serving.tier1.scenario.clone();
    get_tier_row_inner(
        &state,
        idx,
        "tier1.bin",
        scenario.num_items,
        scenario.item_size_bits / 8,
    )
}

async fn get_tier2_row(
    State(state): State<Arc<AppState>>,
    Path(idx): Path<usize>,
) -> axum::response::Response {
    match &state.serving.tier2 {
        Some(tier2) => {
            let scenario = tier2.scenario.clone();
            get_tier_row_inner(
                &state,
                idx,
                "tier2.bin",
                scenario.num_items,
                scenario.item_size_bits / 8,
            )
        }
        None => (StatusCode::NOT_FOUND, TIER2_DISABLED_BODY).into_response(),
    }
}

fn get_tier_row_inner(
    state: &AppState,
    idx: usize,
    filename: &str,
    num_rows: usize,
    row_bytes: usize,
) -> axum::response::Response {
    if !state.debug_row_endpoints {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    if idx >= num_rows {
        return (StatusCode::NOT_FOUND, "row index out of range").into_response();
    }
    let path = state.data_dir.join(filename);
    let offset = (idx * row_bytes) as u64;
    match read_tier_row(&path, offset, row_bytes) {
        Ok(row) => (
            [(axum::http::header::CONTENT_TYPE, "application/octet-stream")],
            row,
        )
            .into_response(),
        Err(e) => {
            tracing::warn!(filename, error = %e, "tier row read failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "read error").into_response()
        }
    }
}

async fn get_root(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let info = RootInfo {
        zcash_network: state.serving.metadata.zcash_network,
        nullifier_pool: state.serving.metadata.nullifier_pool.clone(),
        dataset_version: state.serving.metadata.dataset_version,
        root29: state.serving.metadata.root29.clone(),
        root25: state.serving.metadata.root25.clone(),
        num_ranges: state.serving.metadata.num_ranges,
        pir_layout: state.serving.metadata.pir_layout,
        pir_depth: state.serving.metadata.pir_depth,
        tier1_rows: state.serving.metadata.tier1_rows,
        tier1_row_bytes: state.serving.metadata.tier1_row_bytes,
        tier2_rows: state.serving.metadata.tier2_rows,
        tier2_row_bytes: state.serving.metadata.tier2_row_bytes,
        height: state.serving.metadata.height,
    };
    axum::Json(info)
}

async fn get_health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let info = HealthInfo {
        status: "ok".to_string(),
        tier1_rows: state.serving.metadata.tier1_rows,
        tier1_row_bytes: state.serving.metadata.tier1_row_bytes,
    };
    axum::Json(info)
}
