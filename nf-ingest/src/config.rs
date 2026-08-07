//! Shared configuration constants and helpers for the nullifier pipeline.

use pir_types::ZcashNetwork;

/// NU6.3 activation height on Zcash Mainnet.
pub const NU6_3_MAINNET_ACTIVATION_HEIGHT: u64 = 3_428_143;

/// NU6.3 activation height on Zcash Testnet.
pub const NU6_3_TESTNET_ACTIVATION_HEIGHT: u64 = 4_134_000;

/// NU6.3 activation height for `network`.
pub const fn nu6_3_activation_height(network: ZcashNetwork) -> u64 {
    match network {
        ZcashNetwork::Main => NU6_3_MAINNET_ACTIVATION_HEIGHT,
        ZcashNetwork::Test => NU6_3_TESTNET_ACTIVATION_HEIGHT,
    }
}

/// Default lightwalletd gRPC endpoints used when no override is provided.
pub const DEFAULT_LWD_URLS: &[&str] = &[
    "https://us.zec.stardust.rest:443",
    "https://eu.zec.stardust.rest:443",
    "https://zec.rocks:443",
];

/// The default single URL used in CLI `--lwd-url` defaults.
/// When the resolved URL list contains only this entry (and no `LWD_URLS` env
/// override was set), the full `DEFAULT_LWD_URLS` list is used instead.
const DEFAULT_SINGLE_LWD_URL: &str = "https://us.zec.stardust.rest:443";

/// Tree checkpoint files under the nullifier root to remove when forcing a rebuild
/// after new blocks were synced from lightwalletd (`--invalidate-after-blocks`).
pub const INVALIDATE_AFTER_BLOCKS_TREE_FILES: &[&str] = &["nullifiers.tree", "nullifiers.tree.tmp"];

/// PIR tier files under the tier output directory for the same invalidation pass.
pub const INVALIDATE_AFTER_BLOCKS_TIER_FILES: &[&str] =
    &["tier0.bin", "tier1.bin", "pir_root.json"];

/// Validate that `height` is a legal export target: at or above NU6.3 activation
/// and a multiple of 10 (the ingestion block-alignment granularity).
pub fn validate_export_height(height: u64, network: ZcashNetwork) -> anyhow::Result<()> {
    let activation_height = nu6_3_activation_height(network);
    anyhow::ensure!(
        height >= activation_height,
        "height {} is below NU6.3 activation on {} ({})",
        height,
        network,
        activation_height
    );
    anyhow::ensure!(
        height.is_multiple_of(10),
        "height {} must be a multiple of 10",
        height
    );
    Ok(())
}

/// Resolve lightwalletd URLs from the `LWD_URLS` env var, a CLI-provided URL,
/// or the hardcoded defaults.
///
/// Priority:
/// 1. `LWD_URLS` env var (comma-separated) if set and non-empty
/// 2. `cli_url` if it differs from the default single URL
/// 3. `DEFAULT_LWD_URLS` as a fallback
pub fn resolve_lwd_urls(cli_url: &str) -> Vec<String> {
    let env_urls = std::env::var("LWD_URLS").ok();
    resolve_lwd_urls_with_override(cli_url, env_urls.as_deref())
}

fn resolve_lwd_urls_with_override(cli_url: &str, override_urls: Option<&str>) -> Vec<String> {
    if let Some(override_urls) = override_urls {
        let urls: Vec<String> = override_urls
            .split(',')
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(str::to_owned)
            .collect();
        if !urls.is_empty() {
            return urls;
        }
    }

    if cli_url == DEFAULT_SINGLE_LWD_URL {
        DEFAULT_LWD_URLS.iter().map(|s| s.to_string()).collect()
    } else {
        vec![cli_url.to_string()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_cli_expands_only_without_override() {
        assert_eq!(
            resolve_lwd_urls_with_override(DEFAULT_SINGLE_LWD_URL, None),
            DEFAULT_LWD_URLS
        );
    }

    #[test]
    fn custom_cli_is_used_without_override() {
        assert_eq!(
            resolve_lwd_urls_with_override("https://custom.example:443", None),
            vec!["https://custom.example:443"]
        );
    }

    #[test]
    fn explicit_single_default_url_is_not_expanded() {
        assert_eq!(
            resolve_lwd_urls_with_override(DEFAULT_SINGLE_LWD_URL, Some(DEFAULT_SINGLE_LWD_URL)),
            vec![DEFAULT_SINGLE_LWD_URL]
        );
    }

    #[test]
    fn explicit_urls_are_trimmed_and_empty_entries_are_ignored() {
        assert_eq!(
            resolve_lwd_urls_with_override(
                "unused",
                Some(" https://one.example:443, ,https://two.example:443 ")
            ),
            vec!["https://one.example:443", "https://two.example:443"]
        );
    }

    #[test]
    fn empty_override_uses_cli_fallback() {
        assert_eq!(
            resolve_lwd_urls_with_override("https://custom.example:443", Some(" , ")),
            vec!["https://custom.example:443"]
        );
    }

    #[test]
    fn validates_network_specific_activation_heights() {
        assert!(validate_export_height(3_428_150, ZcashNetwork::Main).is_ok());
        assert!(validate_export_height(3_428_150, ZcashNetwork::Test).is_err());
        assert!(validate_export_height(4_134_000, ZcashNetwork::Test).is_ok());
    }
}
