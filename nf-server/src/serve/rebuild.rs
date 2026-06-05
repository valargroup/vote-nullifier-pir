use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use nf_ingest::sync_nullifiers;

use super::state::AppState;

// ── Snapshot management endpoints ─────────────────────────────────────────────

// NOTE: `POST /snapshot/prepare` is intentionally disabled. Snapshot updates
// are performed by restarting `nf-server serve` so bootstrap loads published
// snapshot artifacts for the active voting round.

/// `POST /snapshot/prepare` is disabled.
///
/// We no longer support kicking off in-process snapshot rebuilds over HTTP.
/// To move a server to a newer height, restart `nf-server serve` and let
/// `bootstrap::run` pull the latest published snapshot from the CDN
/// (`<precomputed_base_url>/snapshots/<height>/...`); the canonical height
/// comes from the active on-chain voting round. The handler is kept (and
/// wired into the router) for historical reasons so that callers get a
/// clear, structured 410 response instead of a 404.
pub(crate) async fn post_snapshot_prepare(
    State(_state): State<Arc<AppState>>,
) -> impl IntoResponse {
    (
        StatusCode::GONE,
        axum::Json(serde_json::json!({
            "error": "POST /snapshot/prepare is disabled",
            "recommendation": "restart nf-server serve to bootstrap from the published snapshot for the active voting round",
        })),
    )
        .into_response()
}

pub(crate) async fn get_snapshot_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let (phase_json, height, num_ranges) = {
        let phase = state.phase.read().await;
        let serving = state.serving.read().await;
        let h = serving.as_ref().and_then(|s| s.metadata.height);
        let n = serving.as_ref().map(|s| s.metadata.num_ranges);
        (serde_json::to_value(&*phase).unwrap_or_default(), h, n)
    };

    let zcash_tip = if let Some(lwd_url) = state.lwd_urls.first() {
        sync_nullifiers::fetch_chain_tip(lwd_url).await.ok()
    } else {
        None
    };

    let mut resp = phase_json;
    if let Some(obj) = resp.as_object_mut() {
        obj.insert("height".to_string(), serde_json::json!(height));
        obj.insert("num_ranges".to_string(), serde_json::json!(num_ranges));
        obj.insert("zcash_tip".to_string(), serde_json::json!(zcash_tip));
    }

    axum::Json(resp)
}
