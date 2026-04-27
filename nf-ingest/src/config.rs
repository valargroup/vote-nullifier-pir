//! Shared configuration constants and helpers for the nullifier pipeline.

/// Default lightwalletd gRPC endpoints used when no override is provided.
pub const DEFAULT_LWD_URLS: &[&str] = &[
    "https://zec.rocks:443",
    "https://eu2.zec.stardust.rest:443",
    "https://eu.zec.stardust.rest:443",
];

/// The default single URL used in CLI `--lwd-url` defaults.
/// When the resolved URL list contains only this entry (and no `LWD_URLS` env
/// override was set), the full `DEFAULT_LWD_URLS` list is used instead.
const DEFAULT_SINGLE_LWD_URL: &str = "https://zec.rocks:443";

/// Tree checkpoint files under the nullifier root to remove when forcing a rebuild
/// after new blocks were synced from lightwalletd (`--invalidate-after-blocks`).
pub const INVALIDATE_AFTER_BLOCKS_TREE_FILES: &[&str] = &["nullifiers.tree", "nullifiers.tree.tmp"];

/// PIR tier files under the tier output directory for the same invalidation pass.
pub const INVALIDATE_AFTER_BLOCKS_TIER_FILES: &[&str] =
    &["tier0.bin", "tier1.bin", "tier2.bin", "pir_root.json"];

/// Validate that `height` is a legal export target: at or above NU5 activation
/// and a multiple of 10 (the ingestion block-alignment granularity).
pub fn validate_export_height(height: u64) -> anyhow::Result<()> {
    use crate::sync_nullifiers::NU5_ACTIVATION_HEIGHT;
    anyhow::ensure!(
        height >= NU5_ACTIVATION_HEIGHT,
        "height {} is below NU5 activation ({})",
        height,
        NU5_ACTIVATION_HEIGHT
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
    let urls: Vec<String> = std::env::var("LWD_URLS")
        .map(|s| s.split(',').map(|u| u.trim().to_string()).collect())
        .unwrap_or_else(|_| vec![cli_url.to_string()]);

    if urls.len() == 1 && urls[0] == DEFAULT_SINGLE_LWD_URL {
        DEFAULT_LWD_URLS.iter().map(|s| s.to_string()).collect()
    } else {
        urls
    }
}
