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

pub async fn fetch_required_snapshot_height(url: &str, timeout: Duration) -> Result<u64> {
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

    if cfg.schema_version != 1 {
        bail!(
            "pir config schema_version = {} (only 1 is supported)",
            cfg.schema_version
        );
    }
    let height = cfg.snapshot_height.parse()?;
    validate_snapshot_height(height)?;
    Ok(height)
}

fn validate_snapshot_height(height: u64) -> Result<()> {
    if height == 0 {
        bail!("pir config snapshot_height must be greater than zero");
    }
    if height % 10 != 0 {
        bail!("pir config snapshot_height must be a multiple of 10, got {height}");
    }
    Ok(())
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
    fn validates_height_shape() {
        assert!(validate_snapshot_height(100).is_ok());
        assert!(validate_snapshot_height(0).is_err());
        assert!(validate_snapshot_height(101).is_err());
    }
}
