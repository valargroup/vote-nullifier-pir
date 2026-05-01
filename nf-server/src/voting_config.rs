//! Resolve the active on-chain round snapshot height from the published
//! `voting-config.json` service-discovery URL.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct VotingConfig {
    #[serde(default)]
    vote_servers: Vec<ServiceEntry>,
}

#[derive(Debug, Deserialize)]
struct ServiceEntry {
    url: String,
}

#[derive(Debug, Deserialize)]
struct ActiveRoundResponse {
    #[serde(default)]
    round: Option<ChainRound>,
}

#[derive(Debug, Deserialize)]
struct ChainRound {
    #[serde(default)]
    snapshot_height: Option<JsonU64>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum JsonU64 {
    Number(u64),
    String(String),
}

impl JsonU64 {
    fn parse(self) -> Result<u64> {
        match self {
            JsonU64::Number(n) => Ok(n),
            JsonU64::String(s) => s
                .parse::<u64>()
                .with_context(|| format!("parse snapshot_height {s:?}")),
        }
    }
}

/// GET the wallet-facing config, then query configured vote servers until one
/// returns the active chain round `snapshot_height`.
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

    let vote_servers: Vec<&str> = cfg
        .vote_servers
        .iter()
        .map(|entry| entry.url.trim_end_matches('/'))
        .filter(|url| !url.is_empty())
        .collect();
    if vote_servers.is_empty() {
        bail!("voting-config at {url} has no vote_servers");
    }

    let mut saw_no_active_round = false;
    let mut first_error: Option<anyhow::Error> = None;
    for vote_server in vote_servers {
        match fetch_vote_server_active_snapshot_height(&client, vote_server).await {
            Ok(Some(height)) => return Ok(Some(height)),
            Ok(None) => saw_no_active_round = true,
            Err(err) => {
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
        }
    }

    if saw_no_active_round {
        Ok(None)
    } else if let Some(err) = first_error {
        Err(err)
    } else {
        Ok(None)
    }
}

async fn fetch_vote_server_active_snapshot_height(
    client: &reqwest::Client,
    vote_server: &str,
) -> Result<Option<u64>> {
    let active_url = format!("{vote_server}/shielded-vote/v1/rounds/active");
    let resp = client
        .get(&active_url)
        .send()
        .await
        .with_context(|| format!("GET {active_url}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let resp = resp
        .error_for_status()
        .with_context(|| format!("GET {active_url} (non-2xx)"))?;
    let active: ActiveRoundResponse = resp
        .json()
        .await
        .with_context(|| format!("decode {active_url} as active round"))?;

    let height = active
        .round
        .and_then(|round| round.snapshot_height)
        .map(JsonU64::parse)
        .transpose()?;
    Ok(height)
}

/// Same as [`fetch_voting_snapshot_height`] but requires a numeric height.
pub async fn fetch_required_snapshot_height(url: &str, timeout: Duration) -> Result<u64> {
    fetch_voting_snapshot_height(url, timeout)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no on-chain round with snapshot_height discovered via voting-config at {url}; disable check with empty --voting-config-url / SVOTE_PIR_VOTING_CONFIG_URL"
            )
        })
}
