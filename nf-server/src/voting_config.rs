//! Fetch `snapshot_height` from the published `voting-config.json` URL.

use std::time::Duration;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct VotingConfig {
    #[serde(default)]
    snapshot_height: Option<u64>,
}

/// GET JSON and return `snapshot_height` when present.
pub async fn fetch_voting_snapshot_height(url: &str, timeout: Duration) -> Result<Option<u64>> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .context("build reqwest client")?;
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("GET {url} (non-2xx)"))?;
    let cfg: VotingConfig = resp
        .json()
        .await
        .with_context(|| format!("decode {url} as voting-config"))?;
    Ok(cfg.snapshot_height)
}

/// Same as [`fetch_voting_snapshot_height`] but requires a numeric height.
pub async fn fetch_required_snapshot_height(url: &str, timeout: Duration) -> Result<u64> {
    fetch_voting_snapshot_height(url, timeout)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "voting-config at {url} has no snapshot_height; disable check with empty --voting-config-url / SVOTE_VOTING_CONFIG_URL"
            )
        })
}
