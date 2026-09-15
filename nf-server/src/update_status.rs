//! Read the updater's public status projection; never read host secrets.
use serde_json::{json, Value};
/// Return allowlisted updater status fields, or null when status is unavailable.
pub async fn read() -> Value {
    let path = "/var/lib/pir-updater/status.json";
    match tokio::fs::metadata(path).await {
        Ok(m) if m.len() <= 65536 => {}
        _ => return Value::Null,
    }
    let Ok(bytes) = tokio::fs::read(path).await else {
        return Value::Null;
    };
    let Ok(value) = serde_json::from_slice::<Value>(&bytes) else {
        return Value::Null;
    };
    let mut result = json!({});
    for key in [
        "enabled",
        "phase",
        "desired",
        "converged",
        "last_check",
        "last_verified",
        "last_success",
        "next_retry",
        "error",
        "failures_total",
        "rollbacks",
    ] {
        if let Some(v) = value.get(key) {
            result[key] = v.clone();
        }
    }
    result
}

/// Render build identity and persisted updater counters as Prometheus exposition.
pub async fn metrics() -> String {
    let status = read().await;
    let info = crate::build_info::info();
    // JSON string escaping is also valid for these ASCII Prometheus label values.
    let mut out = format!(
        "# TYPE nf_build_info gauge\nnf_build_info{{release_tag={},commit_sha={}}} 1\n",
        info["release_tag"], info["commit_sha"]
    );
    for (field, metric) in [
        ("last_check", "last_check_seconds"),
        ("last_verified", "last_verified_seconds"),
        ("last_success", "last_success_seconds"),
        ("next_retry", "next_retry_seconds"),
        ("failures_total", "failures_total"),
        ("rollbacks", "rollbacks_total"),
    ] {
        if let Some(n) = status[field].as_u64() {
            out.push_str(&format!("nf_updater_{metric} {n}\n"));
        }
    }
    for field in ["enabled", "converged"] {
        if let Some(v) = status[field].as_bool() {
            out.push_str(&format!("nf_updater_{field} {}\n", u8::from(v)));
        }
    }
    if let Some(h) = status["desired"]["snapshot_height"].as_u64() {
        out.push_str(&format!("nf_updater_desired_height {h}\n"));
    }
    if let Some(tag) = status["desired"]["binary_tag"].as_str() {
        out.push_str(&format!(
            "nf_updater_desired_info{{release_tag={}}} 1\n",
            json!(tag)
        ));
    }
    out
}
