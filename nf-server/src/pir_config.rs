//! Resolve the recommended PIR snapshot height from an environment-scoped
//! `pir.json` document.
//!
//! New installs should set `SVOTE_PIR_CONFIG_URL` directly. Existing hosts that
//! only have `SVOTE_PIR_VOTING_CONFIG_URL` can still restart into a new binary:
//! we derive the matching `prod/pir.json` or `stage/pir.json` URL when the old
//! voting-config URL clearly identifies an environment, and otherwise let the
//! caller fall back to active-round discovery.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use pir_types::ZcashNetwork;
use serde::Deserialize;

pub const DEFAULT_PROD_PIR_CONFIG_URL: &str = "https://voting.valargroup.org/prod/pir.json";
pub const DEFAULT_STAGE_PIR_CONFIG_URL: &str = "https://voting.valargroup.org/stage/pir.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// `SVOTE_PIR_CONFIG_URL=` explicitly disables config-driven bootstrap.
    Disabled,
    /// Use this PIR config URL. `strict=false` means it was derived for
    /// compatibility and may fall back to the old voting-config path.
    Url { url: String, strict: bool },
    /// Could not infer a PIR config URL from old host config.
    FallbackToVotingConfig { reason: String },
}

#[derive(Debug, Deserialize)]
struct PirSnapshotConfig {
    schema_version: u32,
    zcash_network: ZcashNetwork,
    snapshot_height: JsonU64,
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

pub fn resolve_source(
    explicit_pir_config_url: Option<&str>,
    voting_config_url: &str,
) -> Result<Source> {
    if let Some(url) = explicit_pir_config_url {
        let url = trim_url(url);
        return if url.is_empty() {
            Ok(Source::Disabled)
        } else {
            Ok(Source::Url {
                url: url.to_string(),
                strict: true,
            })
        };
    }

    let voting_config_url = trim_url(voting_config_url);
    if voting_config_url.is_empty() {
        return Ok(Source::Disabled);
    }
    if is_stage_url(voting_config_url) {
        return Ok(Source::Url {
            url: DEFAULT_STAGE_PIR_CONFIG_URL.to_string(),
            strict: false,
        });
    }
    if is_prod_url(voting_config_url) {
        return Ok(Source::Url {
            url: DEFAULT_PROD_PIR_CONFIG_URL.to_string(),
            strict: false,
        });
    }

    Ok(Source::FallbackToVotingConfig {
        reason: format!("could not infer PIR config environment from {voting_config_url}"),
    })
}

pub async fn fetch_required_snapshot_height(
    url: &str,
    expected_network: ZcashNetwork,
    timeout: Duration,
) -> Result<u64> {
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
    let cfg: PirSnapshotConfig = resp
        .json()
        .await
        .with_context(|| format!("decode {url} as PIR snapshot config"))?;

    if cfg.schema_version != 2 {
        bail!(
            "pir config schema_version = {} (only 2 is supported)",
            cfg.schema_version
        );
    }
    if cfg.zcash_network != expected_network {
        bail!(
            "pir config Zcash network is {}; expected {}",
            cfg.zcash_network,
            expected_network
        );
    }
    let height = cfg.snapshot_height.parse()?;
    nf_ingest::config::validate_export_height(height, expected_network)?;
    Ok(height)
}

fn trim_url(url: &str) -> &str {
    url.trim().trim_end_matches('/')
}

fn is_stage_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains("/stage/") || lower.contains("stage/static-voting-config")
}

fn is_prod_url(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    lower.contains("/prod/")
        || lower == "https://voting.valargroup.org/static-voting-config.json"
        || lower == "https://voting.valargroup.org/prod/static-voting-config.json"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_url_wins() {
        let source = resolve_source(
            Some("https://example.com/pir.json/"),
            "https://voting.valargroup.org/prod/static-voting-config.json",
        )
        .unwrap();
        assert_eq!(
            source,
            Source::Url {
                url: "https://example.com/pir.json".to_string(),
                strict: true
            }
        );
    }

    #[test]
    fn explicit_empty_url_disables_bootstrap() {
        assert_eq!(
            resolve_source(Some(""), DEFAULT_PROD_PIR_CONFIG_URL).unwrap(),
            Source::Disabled
        );
    }

    #[test]
    fn derives_prod_from_old_voting_config_url() {
        assert_eq!(
            resolve_source(
                None,
                "https://voting.valargroup.org/prod/static-voting-config.json"
            )
            .unwrap(),
            Source::Url {
                url: DEFAULT_PROD_PIR_CONFIG_URL.to_string(),
                strict: false
            }
        );
    }

    #[test]
    fn derives_prod_from_legacy_root_voting_config_url() {
        assert_eq!(
            resolve_source(
                None,
                "https://voting.valargroup.org/static-voting-config.json"
            )
            .unwrap(),
            Source::Url {
                url: DEFAULT_PROD_PIR_CONFIG_URL.to_string(),
                strict: false
            }
        );
    }

    #[test]
    fn derives_stage_from_old_voting_config_url() {
        assert_eq!(
            resolve_source(
                None,
                "https://voting.valargroup.org/stage/static-voting-config.json"
            )
            .unwrap(),
            Source::Url {
                url: DEFAULT_STAGE_PIR_CONFIG_URL.to_string(),
                strict: false
            }
        );
    }

    #[test]
    fn ambiguous_url_uses_legacy_fallback() {
        assert!(matches!(
            resolve_source(None, "https://mirror.example.com/config.json").unwrap(),
            Source::FallbackToVotingConfig { .. }
        ));
    }

    #[test]
    fn validates_network_specific_height_shape() {
        assert!(nf_ingest::config::validate_export_height(3_428_150, ZcashNetwork::Main).is_ok());
        assert!(nf_ingest::config::validate_export_height(3_428_150, ZcashNetwork::Test).is_err());
        assert!(nf_ingest::config::validate_export_height(4_134_000, ZcashNetwork::Test).is_ok());
    }
}
