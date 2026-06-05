//! Shared configuration constants and helpers for the nullifier pipeline.

/// Default lightwalletd gRPC endpoints used when no override is provided.
pub const DEFAULT_LWD_URLS: &[&str] = &[
    "https://us.zec.stardust.rest:443",
    "https://eu2.zec.stardust.rest:443",
    "https://eu.zec.stardust.rest:443",
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
    if let Ok(env_urls) = std::env::var("LWD_URLS") {
        if !env_urls.trim().is_empty() {
            return env_urls.split(',').map(|u| u.trim().to_string()).collect();
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
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvVarGuard {
        key: &'static str,
        old: Option<String>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let old = std::env::var(key).ok();
            std::env::set_var(key, value);
            Self { key, old }
        }

        fn remove(key: &'static str) -> Self {
            let old = std::env::var(key).ok();
            std::env::remove_var(key);
            Self { key, old }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(old) = &self.old {
                std::env::set_var(self.key, old);
            } else {
                std::env::remove_var(self.key);
            }
        }
    }

    #[test]
    fn env_pin_to_default_single_url_does_not_expand() {
        let _lock = ENV_LOCK.lock().expect("env lock poisoned");
        let _guard = EnvVarGuard::set("LWD_URLS", DEFAULT_SINGLE_LWD_URL);
        assert_eq!(
            resolve_lwd_urls("http://cli-should-not-matter.invalid"),
            vec![DEFAULT_SINGLE_LWD_URL.to_string()]
        );
    }

    #[test]
    fn default_cli_url_expands_when_no_env_override_exists() {
        let _lock = ENV_LOCK.lock().expect("env lock poisoned");
        let _guard = EnvVarGuard::remove("LWD_URLS");
        assert_eq!(
            resolve_lwd_urls(DEFAULT_SINGLE_LWD_URL),
            DEFAULT_LWD_URLS
                .iter()
                .map(|url| url.to_string())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn empty_env_value_falls_back_to_cli_resolution() {
        let _lock = ENV_LOCK.lock().expect("env lock poisoned");
        let _guard = EnvVarGuard::set("LWD_URLS", "   ");
        assert_eq!(
            resolve_lwd_urls("http://cli.example"),
            vec!["http://cli.example".to_string()]
        );
    }
}
