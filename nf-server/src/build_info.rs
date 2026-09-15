//! Build identity is embedded in the executable, not supplied by its environment.
/// Return the identity embedded by the release build, independent of runtime settings.
pub fn info() -> serde_json::Value {
    serde_json::json!({
        "release_tag": option_env!("NF_RELEASE_TAG").unwrap_or("development"),
        "commit_sha": option_env!("NF_COMMIT_SHA").unwrap_or("unknown"),
        "package_version": env!("CARGO_PKG_VERSION"),
        "pir_update_protocol": if cfg!(feature = "serve") { 1 } else { 0 }
    })
}
