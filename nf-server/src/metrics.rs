//! Prometheus metrics for `nf-server serve`.
//!
//! Exposed at `GET /metrics` in the standard text exposition format.
//! Lazily initialised in a process-global registry so handlers can
//! call the `*_inc` / `*_set` helpers without threading a registry
//! handle through state.
//!
//! ## Metric naming
//!
//! All metrics are prefixed `nf_` to align with the `nf-server` /
//! `nf-ingest` binary names. Bootstrap-specific metrics live under
//! `nf_snapshot_bootstrap_*`; serving-side gauges (`served_height`,
//! `expected_height`) under `nf_snapshot_*` so the dashboard can pair
//! them across the fleet.

use std::sync::OnceLock;

use prometheus::{
    Encoder, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge, Opts, Registry,
    TextEncoder,
};

struct Metrics {
    registry: Registry,
    bootstrap_attempts: IntCounter,
    bootstrap_outcomes: IntCounterVec,
    bootstrap_bytes: IntCounter,
    bootstrap_duration: HistogramVec,
    served_height: IntGauge,
    expected_height: IntGauge,
}

fn metrics() -> &'static Metrics {
    static INSTANCE: OnceLock<Metrics> = OnceLock::new();
    INSTANCE.get_or_init(|| {
        let registry = Registry::new();

        let bootstrap_attempts = IntCounter::new(
            "nf_snapshot_bootstrap_attempts_total",
            "Total number of snapshot-bootstrap attempts at startup.",
        )
        .expect("valid metric name");

        // `result` is one of: "disabled", "already_at_height",
        // "bootstrapped", "fell_through". Sum across labels equals
        // `nf_snapshot_bootstrap_attempts_total` for every successful run.
        let bootstrap_outcomes = IntCounterVec::new(
            Opts::new(
                "nf_snapshot_bootstrap_outcomes_total",
                "Snapshot-bootstrap outcomes at startup, partitioned by result.",
            ),
            &["result"],
        )
        .expect("valid metric");

        let bootstrap_bytes = IntCounter::new(
            "nf_snapshot_bootstrap_bytes_total",
            "Cumulative bytes downloaded by the snapshot bootstrap (manifest + tier files).",
        )
        .expect("valid metric");

        // Wide buckets: a tier0 download from a slow region can sit in
        // the multi-minute range; we want a single histogram that's
        // useful both for a fast bootstrap and a slow one.
        let bootstrap_duration = HistogramVec::new(
            HistogramOpts::new(
                "nf_snapshot_bootstrap_duration_seconds",
                "End-to-end snapshot-bootstrap duration, including manifest + tier downloads.",
            )
            .buckets(vec![
                1.0, 5.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 1200.0, 1800.0,
            ]),
            &[],
        )
        .expect("valid metric");

        let served_height = IntGauge::new(
            "nf_snapshot_served_height",
            "Block height of the snapshot currently loaded on disk \
             (0 if no usable local snapshot at startup).",
        )
        .expect("valid metric");

        let expected_height = IntGauge::new(
            "nf_snapshot_expected_height",
            "Block height the published voting-config says we should be serving \
             (0 if voting-config didn't declare one or hasn't been fetched yet).",
        )
        .expect("valid metric");

        registry
            .register(Box::new(bootstrap_attempts.clone()))
            .expect("register attempts");
        registry
            .register(Box::new(bootstrap_outcomes.clone()))
            .expect("register outcomes");
        registry
            .register(Box::new(bootstrap_bytes.clone()))
            .expect("register bytes");
        registry
            .register(Box::new(bootstrap_duration.clone()))
            .expect("register duration");
        registry
            .register(Box::new(served_height.clone()))
            .expect("register served");
        registry
            .register(Box::new(expected_height.clone()))
            .expect("register expected");

        Metrics {
            registry,
            bootstrap_attempts,
            bootstrap_outcomes,
            bootstrap_bytes,
            bootstrap_duration,
            served_height,
            expected_height,
        }
    })
}

pub fn bootstrap_attempts_inc() {
    metrics().bootstrap_attempts.inc();
}

pub fn bootstrap_outcome_inc(result: &str) {
    metrics()
        .bootstrap_outcomes
        .with_label_values(&[result])
        .inc();
}

pub fn bootstrap_bytes_inc(bytes: u64) {
    metrics().bootstrap_bytes.inc_by(bytes);
}

pub fn bootstrap_duration_observe(d: std::time::Duration) {
    metrics()
        .bootstrap_duration
        .with_label_values(&[])
        .observe(d.as_secs_f64());
}

pub fn served_height_set(h: u64) {
    metrics().served_height.set(h as i64);
}

pub fn expected_height_set(h: u64) {
    metrics().expected_height.set(h as i64);
}

/// `GET /metrics` handler — Prometheus text exposition.
pub async fn handle_metrics() -> impl axum::response::IntoResponse {
    let m = metrics();
    let mf = m.registry.gather();
    let encoder = TextEncoder::new();
    // `format_type()` borrows from the encoder, so we must keep it
    // alive long enough to copy the result into an owned `String`.
    let content_type = encoder.format_type().to_string();
    let mut buf = Vec::with_capacity(4096);
    if let Err(e) = encoder.encode(&mf, &mut buf) {
        return (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            [(
                axum::http::header::CONTENT_TYPE,
                "text/plain; charset=utf-8".to_string(),
            )],
            format!("metrics encode failed: {e}"),
        );
    }
    let body = String::from_utf8(buf).unwrap_or_else(|_| "<invalid utf-8>".to_string());
    (
        axum::http::StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, content_type)],
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_initialises_and_observes() {
        bootstrap_attempts_inc();
        bootstrap_outcome_inc("bootstrapped");
        bootstrap_bytes_inc(123);
        bootstrap_duration_observe(std::time::Duration::from_secs(2));
        served_height_set(100);
        expected_height_set(101);

        let mf = metrics().registry.gather();
        let names: Vec<&str> = mf.iter().map(|f| f.get_name()).collect();
        assert!(names.contains(&"nf_snapshot_bootstrap_attempts_total"));
        assert!(names.contains(&"nf_snapshot_bootstrap_outcomes_total"));
        assert!(names.contains(&"nf_snapshot_bootstrap_bytes_total"));
        assert!(names.contains(&"nf_snapshot_bootstrap_duration_seconds"));
        assert!(names.contains(&"nf_snapshot_served_height"));
        assert!(names.contains(&"nf_snapshot_expected_height"));
    }
}
