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
use std::time::Instant;

use prometheus::{
    Encoder, Histogram, HistogramOpts, HistogramVec, IntCounter, IntCounterVec, IntGauge,
    IntGaugeVec, Opts, Registry, TextEncoder,
};

struct Metrics {
    registry: Registry,
    bootstrap_attempts: IntCounter,
    bootstrap_outcomes: IntCounterVec,
    bootstrap_bytes: IntCounter,
    bootstrap_duration: HistogramVec,
    served_height: IntGauge,
    expected_height: IntGauge,
    stale_seconds: IntGauge,
    http_requests: IntCounterVec,
    http_request_duration: HistogramVec,
    http_request_body_receive_duration: HistogramVec,
    http_request_processing_duration: HistogramVec,
    http_in_flight: IntGaugeVec,
    http_processing_in_flight: IntGaugeVec,
}

fn build_metrics() -> Metrics {
    let registry = Registry::new();

    let bootstrap_attempts = IntCounter::new(
        "nf_snapshot_bootstrap_attempts_total",
        "Total number of snapshot-bootstrap attempts at startup.",
    )
    .expect("valid metric name");

    // `result` is one of: "disabled", "already_at_height",
    // "using_local_snapshot", "bootstrapped", "fell_through",
    // "failed_voting_config".
    // Sum across labels equals `nf_snapshot_bootstrap_attempts_total`
    // for every completed attempt (including failed_voting_config).
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
        "Block height the configured snapshot source says we should be serving \
         (0 if no source declared one or it hasn't been fetched yet).",
    )
    .expect("valid metric");

    // Seconds the host has been observed serving a stale snapshot
    // (`served_height < expected_height` with `expected_height > 0`).
    // 0 while converged. Reset to 0 the moment served catches up.
    // Updated by the watchdog loop in `serve::watchdog`; the alert
    // rule fires when this exceeds the configured threshold.
    let stale_seconds = IntGauge::new(
        "nf_snapshot_stale_seconds",
        "Seconds this host has been continuously observed serving a snapshot \
         older than the canonical configured height (0 if currently converged).",
    )
    .expect("valid metric");

    let http_requests = IntCounterVec::new(
        Opts::new(
            "nf_http_requests_total",
            "PIR API requests partitioned by allowlisted endpoint, method, and status.",
        ),
        &["endpoint", "method", "status"],
    )
    .expect("valid metric");
    let http_request_duration = HistogramVec::new(
        HistogramOpts::new(
            "nf_http_request_duration_seconds",
            "Time from nf-server receiving request headers until the route produces a response. \
             Includes request body receive time but excludes response transmission.",
        )
        .buckets(vec![0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0]),
        &["endpoint"],
    )
    .expect("valid metric");
    let http_request_body_receive_duration = HistogramVec::new(
        HistogramOpts::new(
            "nf_http_request_body_receive_duration_seconds",
            "Time from nf-server receiving request headers until the complete request body is \
             available for processing.",
        )
        .buckets(vec![
            0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0,
        ]),
        &["endpoint"],
    )
    .expect("valid metric");
    let http_request_processing_duration = HistogramVec::new(
        HistogramOpts::new(
            "nf_http_request_processing_duration_seconds",
            "Time spent processing an allowlisted PIR request after its complete body is available.",
        )
        .buckets(vec![0.05, 0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0]),
        &["endpoint"],
    )
    .expect("valid metric");
    let http_in_flight = IntGaugeVec::new(
        Opts::new(
            "nf_http_in_flight",
            "Requests received by nf-server that have not produced a response.",
        ),
        &["endpoint"],
    )
    .expect("valid metric");
    let http_processing_in_flight = IntGaugeVec::new(
        Opts::new(
            "nf_http_processing_in_flight",
            "Requests being processed after their complete bodies have been received.",
        ),
        &["endpoint"],
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
    registry
        .register(Box::new(stale_seconds.clone()))
        .expect("register stale_seconds");
    registry
        .register(Box::new(http_requests.clone()))
        .expect("register http_requests");
    registry
        .register(Box::new(http_request_duration.clone()))
        .expect("register http_request_duration");
    registry
        .register(Box::new(http_request_body_receive_duration.clone()))
        .expect("register http_request_body_receive_duration");
    registry
        .register(Box::new(http_request_processing_duration.clone()))
        .expect("register http_request_processing_duration");
    registry
        .register(Box::new(http_in_flight.clone()))
        .expect("register http_in_flight");
    registry
        .register(Box::new(http_processing_in_flight.clone()))
        .expect("register http_processing_in_flight");
    #[cfg(target_os = "linux")]
    registry
        .register(Box::new(
            prometheus::process_collector::ProcessCollector::for_self(),
        ))
        .expect("register process collector");

    Metrics {
        registry,
        bootstrap_attempts,
        bootstrap_outcomes,
        bootstrap_bytes,
        bootstrap_duration,
        served_height,
        expected_height,
        stale_seconds,
        http_requests,
        http_request_duration,
        http_request_body_receive_duration,
        http_request_processing_duration,
        http_in_flight,
        http_processing_in_flight,
    }
}

fn metrics() -> &'static Metrics {
    static INSTANCE: OnceLock<Metrics> = OnceLock::new();
    INSTANCE.get_or_init(build_metrics)
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

pub fn served_height_get() -> u64 {
    metrics().served_height.get().max(0) as u64
}

pub fn expected_height_set(h: u64) {
    metrics().expected_height.set(h as i64);
}

pub fn expected_height_get() -> u64 {
    metrics().expected_height.get().max(0) as u64
}

pub fn stale_seconds_set(s: u64) {
    // i64::MAX is plenty (~292 billion years); clamp anyway for safety.
    metrics().stale_seconds.set(s.min(i64::MAX as u64) as i64);
}

fn allowlisted_endpoint(method: &axum::http::Method, path: &str) -> Option<&'static str> {
    match (method, path) {
        (&axum::http::Method::GET, "/tier0") => Some("tier0"),
        (&axum::http::Method::GET, "/params/tier1") => Some("params_tier1"),
        (&axum::http::Method::POST, "/tier1/query") => Some("tier1_query"),
        _ => None,
    }
}

/// Timestamp captured when an allowlisted request first reaches nf-server.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RequestStarted(Instant);

struct GaugeGuard(IntGauge);

impl GaugeGuard {
    fn new(gauge: IntGauge) -> Self {
        gauge.inc();
        Self(gauge)
    }
}

impl Drop for GaugeGuard {
    fn drop(&mut self) {
        self.0.dec();
    }
}

/// Observes Tier1 processing time and keeps its post-upload concurrency gauge balanced.
pub(crate) struct ProcessingTimer {
    started: Instant,
    histogram: Histogram,
    _in_flight: GaugeGuard,
}

impl Drop for ProcessingTimer {
    fn drop(&mut self) {
        self.histogram.observe(self.started.elapsed().as_secs_f64());
    }
}

fn start_tier1_processing_with(
    metrics: &Metrics,
    request_started: RequestStarted,
) -> ProcessingTimer {
    const ENDPOINT: &str = "tier1_query";
    metrics
        .http_request_body_receive_duration
        .with_label_values(&[ENDPOINT])
        .observe(request_started.0.elapsed().as_secs_f64());
    ProcessingTimer {
        started: Instant::now(),
        histogram: metrics
            .http_request_processing_duration
            .with_label_values(&[ENDPOINT]),
        _in_flight: GaugeGuard::new(
            metrics
                .http_processing_in_flight
                .with_label_values(&[ENDPOINT]),
        ),
    }
}

/// Start measuring Tier1 work after Axum has received the complete request body.
pub(crate) fn start_tier1_processing(request_started: RequestStarted) -> ProcessingTimer {
    start_tier1_processing_with(metrics(), request_started)
}

/// Record only the fixed PIR client routes. Deliberately excludes request IDs,
/// remote addresses, headers, and arbitrary paths from metric labels.
pub async fn track_pir_request(
    mut request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let Some(endpoint) = allowlisted_endpoint(request.method(), request.uri().path()) else {
        return next.run(request).await;
    };
    let method = request.method().as_str().to_owned();
    let m = metrics();
    let started = Instant::now();
    request.extensions_mut().insert(RequestStarted(started));
    let _in_flight = GaugeGuard::new(m.http_in_flight.with_label_values(&[endpoint]));
    let response = next.run(request).await;
    m.http_request_duration
        .with_label_values(&[endpoint])
        .observe(started.elapsed().as_secs_f64());
    m.http_requests
        .with_label_values(&[endpoint, &method, response.status().as_str()])
        .inc();
    response
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
    use axum::{body::Bytes, extract::Extension, http::StatusCode, routing::post, Router};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn registry_initialises_and_observes() {
        let m = build_metrics();
        m.bootstrap_attempts.inc();
        m.bootstrap_outcomes
            .with_label_values(&["bootstrapped"])
            .inc();
        m.bootstrap_bytes.inc_by(123);
        m.bootstrap_duration
            .with_label_values(&[])
            .observe(std::time::Duration::from_secs(2).as_secs_f64());
        m.served_height.set(100);
        m.expected_height.set(101);
        m.stale_seconds.set(42);
        m.http_requests
            .with_label_values(&["tier0", "GET", "200"])
            .inc();
        m.http_request_duration
            .with_label_values(&["tier0"])
            .observe(0.1);
        m.http_request_body_receive_duration
            .with_label_values(&["tier1_query"])
            .observe(1.0);
        m.http_request_processing_duration
            .with_label_values(&["tier1_query"])
            .observe(0.1);
        m.http_in_flight.with_label_values(&["tier0"]).set(1);
        m.http_processing_in_flight
            .with_label_values(&["tier1_query"])
            .set(1);

        let mf = m.registry.gather();
        let names: Vec<&str> = mf.iter().map(|f| f.get_name()).collect();
        assert!(names.contains(&"nf_snapshot_bootstrap_attempts_total"));
        assert!(names.contains(&"nf_snapshot_bootstrap_outcomes_total"));
        assert!(names.contains(&"nf_snapshot_bootstrap_bytes_total"));
        assert!(names.contains(&"nf_snapshot_bootstrap_duration_seconds"));
        assert!(names.contains(&"nf_snapshot_served_height"));
        assert!(names.contains(&"nf_snapshot_expected_height"));
        assert!(names.contains(&"nf_snapshot_stale_seconds"));
        assert!(names.contains(&"nf_http_requests_total"));
        assert!(names.contains(&"nf_http_request_duration_seconds"));
        assert!(names.contains(&"nf_http_request_body_receive_duration_seconds"));
        assert!(names.contains(&"nf_http_request_processing_duration_seconds"));
        assert!(names.contains(&"nf_http_in_flight"));
        assert!(names.contains(&"nf_http_processing_in_flight"));

        // Gauges reflect the most recent set call on this isolated registry.
        assert_eq!(m.served_height.get().max(0) as u64, 100);
        assert_eq!(m.expected_height.get().max(0) as u64, 101);
    }

    #[test]
    fn tier1_timing_separates_body_receive_from_processing() {
        let m = build_metrics();
        let request_started = RequestStarted(Instant::now() - std::time::Duration::from_secs(1));
        {
            let _processing = start_tier1_processing_with(&m, request_started);
            assert_eq!(
                m.http_processing_in_flight
                    .with_label_values(&["tier1_query"])
                    .get(),
                1
            );
        }

        let receive = m
            .http_request_body_receive_duration
            .with_label_values(&["tier1_query"]);
        let processing = m
            .http_request_processing_duration
            .with_label_values(&["tier1_query"]);
        assert_eq!(receive.get_sample_count(), 1);
        assert!(receive.get_sample_sum() >= 1.0);
        assert_eq!(processing.get_sample_count(), 1);
        assert_eq!(
            m.http_processing_in_flight
                .with_label_values(&["tier1_query"])
                .get(),
            0
        );
    }

    #[tokio::test]
    async fn middleware_splits_slow_body_receive_from_handler_processing() {
        async fn timed_handler(
            request_started: Option<Extension<RequestStarted>>,
            _body: Bytes,
        ) -> StatusCode {
            let Some(Extension(request_started)) = request_started else {
                return StatusCode::INTERNAL_SERVER_ERROR;
            };
            let _processing = start_tier1_processing(request_started);
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            StatusCode::OK
        }

        let endpoint = "tier1_query";
        let m = metrics();
        let total_in_flight = m.http_in_flight.with_label_values(&[endpoint]);
        let processing_in_flight = m.http_processing_in_flight.with_label_values(&[endpoint]);
        let total_count_before = m
            .http_request_duration
            .with_label_values(&[endpoint])
            .get_sample_count();
        let receive = m
            .http_request_body_receive_duration
            .with_label_values(&[endpoint]);
        let receive_count_before = receive.get_sample_count();
        let processing = m
            .http_request_processing_duration
            .with_label_values(&[endpoint]);
        let processing_count_before = processing.get_sample_count();

        let app = Router::new()
            .route("/tier1/query", post(timed_handler))
            .layer(axum::middleware::from_fn(track_pir_request));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream
            .write_all(
                b"POST /tier1/query HTTP/1.1\r\nHost: localhost\r\nContent-Length: 4\r\nConnection: close\r\n\r\na",
            )
            .await
            .unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while total_in_flight.get() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(total_in_flight.get(), 1);
        assert_eq!(processing_in_flight.get(), 0);
        assert_eq!(receive.get_sample_count(), receive_count_before);

        stream.write_all(b"bcd").await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while processing_in_flight.get() == 0 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(processing_in_flight.get(), 1);
        assert_eq!(receive.get_sample_count(), receive_count_before + 1);

        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        assert!(response.starts_with(b"HTTP/1.1 200 OK"));
        assert_eq!(total_in_flight.get(), 0);
        assert_eq!(processing_in_flight.get(), 0);
        assert_eq!(
            m.http_request_duration
                .with_label_values(&[endpoint])
                .get_sample_count(),
            total_count_before + 1
        );
        assert_eq!(processing.get_sample_count(), processing_count_before + 1);

        server.abort();
    }

    #[test]
    fn endpoint_labels_are_fixed_and_allowlisted() {
        assert_eq!(
            allowlisted_endpoint(&axum::http::Method::GET, "/tier0"),
            Some("tier0")
        );
        assert_eq!(
            allowlisted_endpoint(&axum::http::Method::POST, "/tier1/query"),
            Some("tier1_query")
        );
        assert_eq!(
            allowlisted_endpoint(&axum::http::Method::GET, "/tier1/row/7"),
            None
        );
        assert_eq!(
            allowlisted_endpoint(&axum::http::Method::GET, "/metrics"),
            None
        );
    }
}
