//! Dual-read `SVOTE_PIR_*` environment variables with legacy `SVOTE_*` aliases.
//!
//! When both primary and legacy are set to different values, the primary wins
//! and a warning is logged. When only the legacy name is set, a deprecation
//! warning is logged.

use std::ffi::OsString;
use std::path::PathBuf;

use tracing::warn;

pub fn env_present(key: &str) -> bool {
    std::env::var_os(key).is_some()
}

fn loose_url_compare(a: &str, b: &str) -> bool {
    a.trim().trim_end_matches('/') == b.trim().trim_end_matches('/')
}

/// Single string from env: `primary` preferred over `legacy`, then CLI/default.
pub fn pick_string_urlish(primary: &str, legacy: &str, from_cli: String) -> String {
    if env_present(primary) {
        let pv = std::env::var(primary).unwrap_or_default();
        if env_present(legacy) {
            let lv = std::env::var(legacy).unwrap_or_default();
            if !loose_url_compare(&pv, &lv) {
                warn!(
                    primary,
                    legacy,
                    primary_value = %pv,
                    legacy_value = %lv,
                    "both env vars are set with different values; using {primary}"
                );
            }
        }
        return pv;
    }
    if env_present(legacy) {
        warn!(
            legacy,
            primary,
            "{legacy} is deprecated; set {primary} to the same value instead"
        );
        return std::env::var(legacy).unwrap_or_default();
    }
    from_cli
}

#[cfg_attr(not(feature = "serve"), allow(dead_code))]
pub fn pick_optional_string(
    primary: &str,
    legacy: &str,
    from_cli: Option<String>,
) -> Option<String> {
    if env_present(primary) {
        let pv = std::env::var(primary).unwrap_or_default();
        if env_present(legacy) {
            let lv = std::env::var(legacy).unwrap_or_default();
            if pv.trim() != lv.trim() {
                warn!(
                    primary,
                    legacy,
                    primary_value = %pv,
                    legacy_value = %lv,
                    "both env vars are set with different values; using {primary}"
                );
            }
        }
        return if pv.is_empty() { None } else { Some(pv) };
    }
    if env_present(legacy) {
        warn!(
            legacy,
            primary,
            "{legacy} is deprecated; set {primary} to the same value instead"
        );
        let v = std::env::var(legacy).unwrap_or_default();
        return if v.is_empty() { None } else { Some(v) };
    }
    from_cli
}

pub fn pick_path(primary: &str, legacy: &str, from_cli: PathBuf) -> PathBuf {
    if env_present(primary) {
        let pv: PathBuf = std::env::var_os(primary)
            .map(OsString::into_string)
            .and_then(Result::ok)
            .unwrap_or_default()
            .into();
        if env_present(legacy) {
            let lv: PathBuf = std::env::var_os(legacy)
                .map(OsString::into_string)
                .and_then(Result::ok)
                .unwrap_or_default()
                .into();
            if pv != lv {
                warn!(
                    primary,
                    legacy,
                    primary_value = %pv.display(),
                    legacy_value = %lv.display(),
                    "both env vars are set with different values; using {primary}"
                );
            }
        }
        return pv;
    }
    if env_present(legacy) {
        warn!(
            legacy,
            primary,
            "{legacy} is deprecated; set {primary} to the same path instead"
        );
        return std::env::var_os(legacy)
            .map(OsString::into_string)
            .and_then(Result::ok)
            .unwrap_or_default()
            .into();
    }
    from_cli
}

fn parse_u64_env(key: &str) -> Option<u64> {
    if !env_present(key) {
        return None;
    }
    match std::env::var(key) {
        Ok(s) => match s.parse::<u64>() {
            Ok(v) => Some(v),
            Err(_) => {
                warn!(
                    key,
                    input = %s,
                    "env var value is not a valid unsigned integer; ignoring"
                );
                None
            }
        },
        Err(_) => None,
    }
}

pub fn pick_u64(primary: &str, legacy: &str, from_cli: u64) -> u64 {
    let pv = parse_u64_env(primary);
    let lv = parse_u64_env(legacy);
    match (pv, lv) {
        (Some(a), Some(b)) if a != b => {
            warn!(
                primary,
                legacy,
                primary_value = a,
                legacy_value = b,
                "both env vars are set with different values; using {primary}"
            );
            a
        }
        (Some(a), _) => a,
        (None, Some(b)) => {
            warn!(
                legacy,
                primary,
                "{legacy} is deprecated; set {primary} instead"
            );
            b
        }
        (None, None) => from_cli,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, MutexGuard};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct RestoreEnv {
        keys: Vec<(&'static str, Option<OsString>)>,
        _guard: MutexGuard<'static, ()>,
    }

    impl RestoreEnv {
        fn new() -> Self {
            Self {
                keys: vec![],
                _guard: ENV_LOCK.lock().expect("env test lock"),
            }
        }

        fn set(mut self, key: &'static str, val: &str) -> Self {
            self.keys.push((key, std::env::var_os(key)));
            std::env::set_var(key, val);
            self
        }

        fn remove(mut self, key: &'static str) -> Self {
            self.keys.push((key, std::env::var_os(key)));
            std::env::remove_var(key);
            self
        }
    }

    impl Drop for RestoreEnv {
        fn drop(&mut self) {
            for (key, old) in self.keys.iter().rev() {
                match old {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    #[test]
    fn pick_string_primary_wins_when_both_set() {
        let _r = RestoreEnv::new()
            .set("SVOTE_PIR_VOTING_CONFIG_URL", "https://a/x/")
            .set("SVOTE_VOTING_CONFIG_URL", "https://b/y");
        let v = pick_string_urlish(
            "SVOTE_PIR_VOTING_CONFIG_URL",
            "SVOTE_VOTING_CONFIG_URL",
            "def".into(),
        );
        assert_eq!(v, "https://a/x/");
    }

    #[test]
    fn pick_string_uses_legacy_when_primary_unset() {
        let _r = RestoreEnv::new()
            .remove("SVOTE_PIR_VOTING_CONFIG_URL")
            .set("SVOTE_VOTING_CONFIG_URL", "https://legacy");
        let v = pick_string_urlish(
            "SVOTE_PIR_VOTING_CONFIG_URL",
            "SVOTE_VOTING_CONFIG_URL",
            "def".into(),
        );
        assert_eq!(v, "https://legacy");
    }

    #[test]
    fn pick_string_no_warn_when_both_equal_modulo_slash() {
        let _r = RestoreEnv::new()
            .set("SVOTE_PIR_PRECOMPUTED_BASE_URL", "https://cdn.example")
            .set("SVOTE_PRECOMPUTED_BASE_URL", "https://cdn.example/");
        let v = pick_string_urlish(
            "SVOTE_PIR_PRECOMPUTED_BASE_URL",
            "SVOTE_PRECOMPUTED_BASE_URL",
            "".into(),
        );
        assert_eq!(v, "https://cdn.example");
    }

    #[test]
    fn pick_u64_primary_wins() {
        let _r = RestoreEnv::new()
            .set("SVOTE_PIR_BOOTSTRAP_TIMEOUT_SECS", "99")
            .set("SVOTE_BOOTSTRAP_TIMEOUT_SECS", "100");
        assert_eq!(
            pick_u64(
                "SVOTE_PIR_BOOTSTRAP_TIMEOUT_SECS",
                "SVOTE_BOOTSTRAP_TIMEOUT_SECS",
                1
            ),
            99
        );
    }

    #[test]
    fn pick_u64_cli_when_unset() {
        let _r = RestoreEnv::new()
            .remove("SVOTE_PIR_BOOTSTRAP_TIMEOUT_SECS")
            .remove("SVOTE_BOOTSTRAP_TIMEOUT_SECS");
        assert_eq!(
            pick_u64(
                "SVOTE_PIR_BOOTSTRAP_TIMEOUT_SECS",
                "SVOTE_BOOTSTRAP_TIMEOUT_SECS",
                42
            ),
            42
        );
    }
}
