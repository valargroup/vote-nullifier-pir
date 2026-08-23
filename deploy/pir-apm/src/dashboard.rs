use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{extract::State, response::Html};
use tokio::sync::RwLock;

use crate::{alerts::Alert, host::HostHealth, metrics::EndpointWindow};

pub type SharedDashboard = Arc<RwLock<DashboardData>>;

#[derive(Clone, Debug)]
pub struct DashboardData {
    pub environment: String,
    pub hostname: String,
    pub last_scrape: Option<SystemTime>,
    pub scrape_error: Option<String>,
    pub health_status: Option<u16>,
    pub health_body: Option<String>,
    pub ready_status: Option<u16>,
    pub endpoints: BTreeMap<String, EndpointWindow>,
    pub snapshot_gauges: BTreeMap<String, f64>,
    pub process_resident_memory_bytes: Option<f64>,
    pub host: HostHealth,
    pub active_alerts: Vec<Alert>,
    pub recent_alerts: Vec<(SystemTime, String)>,
}

impl DashboardData {
    pub fn new(environment: String, hostname: String, host: HostHealth) -> Self {
        Self {
            environment,
            hostname,
            last_scrape: None,
            scrape_error: Some("waiting for first scrape".to_string()),
            health_status: None,
            health_body: None,
            ready_status: None,
            endpoints: BTreeMap::new(),
            snapshot_gauges: BTreeMap::new(),
            process_resident_memory_bytes: None,
            host,
            active_alerts: Vec::new(),
            recent_alerts: Vec::new(),
        }
    }
}

pub async fn index(State(state): State<SharedDashboard>) -> Html<String> {
    let data = state.read().await.clone();
    Html(render(&data))
}

pub async fn healthz() -> &'static str {
    "ok\n"
}

fn render(data: &DashboardData) -> String {
    let endpoint_rows = crate::metrics::ENDPOINTS
        .iter()
        .map(|endpoint| {
            let values = data.endpoints.get(*endpoint).cloned().unwrap_or_default();
            format!(
                "<tr><th>{}</th><td>{:.3}</td><td>{:.0}</td><td>{:.0}</td><td>{}</td><td>{}</td><td>{}</td><td>{:.0} ({:.2}%)</td></tr>",
                escape(endpoint),
                values.qps,
                values.in_flight,
                values.requests,
                seconds(values.p50),
                seconds(values.p95),
                seconds(values.p99),
                values.errors_5xx,
                values.error_ratio * 100.0,
            )
        })
        .collect::<String>();
    let snapshot_rows = if data.snapshot_gauges.is_empty() {
        "<tr><td colspan=\"2\">No snapshot metrics yet</td></tr>".to_string()
    } else {
        data.snapshot_gauges
            .iter()
            .map(|(name, value)| {
                format!(
                    "<tr><th>{}</th><td>{}</td></tr>",
                    escape(name),
                    format_number(*value)
                )
            })
            .collect()
    };
    let active_alerts = if data.active_alerts.is_empty() {
        "<li class=\"ok\">No active alerts</li>".to_string()
    } else {
        data.active_alerts
            .iter()
            .map(|alert| {
                format!(
                    "<li class=\"bad\"><strong>{}</strong>: {} (threshold {}; since {})</li>",
                    escape(&alert.check),
                    escape(&alert.observed),
                    escape(&alert.threshold),
                    time(alert.fired_at),
                )
            })
            .collect()
    };
    let recent_alerts = if data.recent_alerts.is_empty() {
        "<li>No alert history in this process</li>".to_string()
    } else {
        data.recent_alerts
            .iter()
            .map(|(at, message)| format!("<li>{}: {}</li>", time(*at), escape(message)))
            .collect()
    };
    let scrape = data
        .scrape_error
        .as_ref()
        .map(|error| format!("<span class=\"bad\">{}</span>", escape(error)))
        .unwrap_or_else(|| "<span class=\"ok\">OK</span>".to_string());
    let process_memory = data
        .process_resident_memory_bytes
        .map(|bytes| bytes_human(bytes as u64))
        .unwrap_or_else(|| "unavailable".to_string());
    let health_body = data
        .health_body
        .as_deref()
        .map(escape)
        .unwrap_or_else(|| "unavailable".to_string());

    format!(
        r#"<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width">
<meta http-equiv="refresh" content="15"><title>PIR APM</title>
<style>
:root {{ color-scheme: dark; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }}
body {{ max-width: 1180px; margin: 2rem auto; padding: 0 1rem; background:#0d1117; color:#d8dee9; }}
h1,h2 {{ color:#f0f6fc; }} .grid {{ display:grid; grid-template-columns:repeat(auto-fit,minmax(280px,1fr)); gap:1rem; }}
.card {{ background:#161b22; border:1px solid #30363d; border-radius:8px; padding:1rem; }}
table {{ width:100%; border-collapse:collapse; }} th,td {{ padding:.5rem; border-bottom:1px solid #30363d; text-align:right; }}
th:first-child,td:first-child {{ text-align:left; }} .ok {{ color:#3fb950; }} .bad {{ color:#f85149; }}
.muted {{ color:#8b949e; }} code {{ color:#79c0ff; }} ul {{ padding-left:1.25rem; }}
</style></head><body>
<h1>PIR APM <span class="muted">{environment} / {hostname}</span></h1>
<p>Last scrape: {last_scrape} · scrape: {scrape} · health: {health} · ready: {ready}</p>
<div class="card"><h2>5-minute PIR endpoint window</h2>
<table><thead><tr><th>Endpoint</th><th>QPS</th><th>Inflight</th><th>Requests</th><th>p50</th><th>p95</th><th>p99</th><th>5xx</th></tr></thead>
<tbody>{endpoint_rows}</tbody></table></div>
<div class="grid">
<section class="card"><h2>Service & snapshot</h2><p>Health: <code>{health_body}</code></p><table><tbody>{snapshot_rows}</tbody></table>
<p>Server resident memory: {process_memory}</p></section>
<section class="card"><h2>Host</h2><p>Load 1/5/15: {load_one:.2} / {load_five:.2} / {load_fifteen:.2}</p>
<p>Memory: {memory_available} available / {memory_total} total</p>
<p>Disk <code>{data_dir}</code>: {disk_used:.1}% used ({disk_available} available / {disk_total} total)</p></section>
<section class="card"><h2>Active alerts</h2><ul>{active_alerts}</ul></section>
<section class="card"><h2>Last alerts</h2><ul>{recent_alerts}</ul></section>
</div>
<section class="card"><h2>Privacy</h2><p>This sidecar scrapes only aggregate, allowlisted PIR metrics and local host health.
It does not collect request bodies, nullifiers, IP addresses, headers, query identifiers, or other per-user data.
All dashboard values are server-rendered; the browser never calls the PIR server or its <code>/metrics</code> endpoint.</p></section>
</body></html>"#,
        environment = escape(&data.environment),
        hostname = escape(&data.hostname),
        last_scrape = data
            .last_scrape
            .map(time)
            .unwrap_or_else(|| "never".to_string()),
        health = status(data.health_status),
        ready = status(data.ready_status),
        health_body = health_body,
        load_one = data.host.load_one,
        load_five = data.host.load_five,
        load_fifteen = data.host.load_fifteen,
        memory_available = bytes_human(data.host.available_memory_bytes),
        memory_total = bytes_human(data.host.total_memory_bytes),
        data_dir = escape(&data.host.data_dir.display().to_string()),
        disk_used = data.host.disk_used_ratio * 100.0,
        disk_available = bytes_human(data.host.disk_available_bytes),
        disk_total = bytes_human(data.host.disk_total_bytes),
    )
}

fn status(value: Option<u16>) -> String {
    match value {
        Some(code) if (200..300).contains(&code) => format!("<span class=\"ok\">{code}</span>"),
        Some(code) => format!("<span class=\"bad\">{code}</span>"),
        None => "<span class=\"bad\">unavailable</span>".to_string(),
    }
}

fn seconds(value: Option<f64>) -> String {
    value
        .map(|seconds| format!("{seconds:.3}s"))
        .unwrap_or_else(|| "—".to_string())
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.3}")
    }
}

fn bytes_human(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes as f64 >= GIB {
        format!("{:.2} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.1} MiB", bytes as f64 / MIB)
    }
}

fn time(value: SystemTime) -> String {
    let seconds = value
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    chrono::DateTime::from_timestamp(seconds as i64, 0)
        .map(|value| value.to_rfc3339())
        .unwrap_or_else(|| seconds.to_string())
}

fn escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
