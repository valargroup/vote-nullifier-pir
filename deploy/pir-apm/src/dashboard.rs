use std::{
    collections::BTreeMap,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use axum::{extract::State, response::Html};
use tokio::sync::RwLock;

use crate::{
    alerts::Alert,
    host::HostHealth,
    metrics::{EndpointWindow, LatencyWindow},
    thresholds,
};

pub type SharedDashboard = Arc<RwLock<DashboardData>>;

/// Seconds between browser refreshes. Matched to the default scrape interval so
/// the page turns over at roughly the same rate the data behind it does.
const REFRESH_SECONDS: u64 = 15;

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

const STYLE: &str = r#"
*,*::before,*::after{box-sizing:border-box}
:root{
color-scheme:dark;
--vellum:#17150f;--ink:#efe5d0;--gold:#c6a15b;--lapis:#3a95dc;
--parchment:188,169,138;
--p85:rgba(var(--parchment),.85);--p62:rgba(var(--parchment),.62);
--p42:rgba(var(--parchment),.42);--p22:rgba(var(--parchment),.22);
--p16:rgba(var(--parchment),.16);--p08:rgba(var(--parchment),.08);
--p05:rgba(var(--parchment),.05);--p02:rgba(var(--parchment),.02);
--ok:#8fb573;--warn:#c6a15b;--bad:#d2694f;
--sans:"IBM Plex Sans",ui-sans-serif,system-ui,-apple-system,sans-serif;
--mono:"IBM Plex Mono",ui-monospace,SFMono-Regular,Menlo,monospace;
}
html{-webkit-text-size-adjust:100%}
body{margin:0;background:var(--vellum);color:var(--p85);font-family:var(--sans);
font-size:15px;line-height:1.55;letter-spacing:-.005em;-webkit-font-smoothing:antialiased}
#app{max-width:1180px;margin:0 auto;padding:clamp(28px,5vh,56px) clamp(16px,4vw,32px) 72px}
h1,h2,h3{color:var(--ink);font-weight:400;margin:0}
code{font-family:var(--mono);color:var(--p62);font-size:.92em}
.num{font-family:var(--mono);font-variant-numeric:tabular-nums;font-feature-settings:"tnum" 1}
.ok{color:var(--ok)}.warn{color:var(--warn)}.bad{color:var(--bad)}.muted{color:var(--p42)}
.eyebrow{font-size:10.5px;letter-spacing:.13em;text-transform:uppercase;color:var(--p42);
font-weight:500;margin:0 0 16px}

.masthead{display:flex;flex-wrap:wrap;gap:20px;align-items:flex-end;
justify-content:space-between;padding-bottom:22px;border-bottom:1px solid var(--p16)}
.brand{display:flex;align-items:center;gap:14px}
.mark{width:26px;height:26px;flex:none;border:1px solid var(--gold);
transform:rotate(45deg);position:relative}
.mark::after{content:"";position:absolute;inset:5px;background:var(--gold);opacity:.55}
.brand h1{font-size:22px;letter-spacing:.055em;text-transform:uppercase;line-height:1.1}
.brand .sub{margin:2px 0 0;font-size:11px;letter-spacing:.1em;text-transform:uppercase;
color:var(--p42)}
.tags{display:flex;flex-wrap:wrap;gap:8px}
.pill{border:1px solid var(--p22);border-radius:999px;padding:4px 12px;font-size:11px;
letter-spacing:.08em;text-transform:uppercase;color:var(--p62);white-space:nowrap}
.pill.env{border-color:rgba(198,161,91,.45);color:var(--gold)}

.statusbar{display:flex;flex-wrap:wrap;gap:12px 24px;align-items:center;
justify-content:space-between;padding:16px 0 30px;border-bottom:1px solid var(--p08)}
.chips{display:flex;flex-wrap:wrap;gap:8px}
.chip{display:inline-flex;align-items:center;gap:7px;border:1px solid var(--p16);
border-radius:4px;padding:5px 11px;font-size:11.5px;letter-spacing:.06em;
text-transform:uppercase;color:var(--p62);background:var(--p02)}
.chip .dot{width:6px;height:6px;border-radius:50%;flex:none;background:currentColor}
.chip.is-ok{color:var(--ok);border-color:rgba(143,181,115,.3)}
.chip.is-bad{color:var(--bad);border-color:rgba(210,105,79,.35)}
.chip b{font-weight:500;font-family:var(--mono)}
.updated{display:flex;align-items:center;gap:9px;font-size:12px;color:var(--p42)}
.live{width:6px;height:6px;border-radius:50%;background:var(--ok);flex:none;
animation:pulse 2.4s ease-in-out infinite}
@keyframes pulse{0%,100%{opacity:1}50%{opacity:.25}}

.kpis{display:grid;grid-template-columns:repeat(4,1fr);gap:1px;background:var(--p16);
border:1px solid var(--p16);margin:30px 0 34px}
.kpi{background:var(--vellum);padding:20px 22px 22px}
.kpi .figure{font-family:var(--mono);font-variant-numeric:tabular-nums;font-size:30px;
line-height:1.1;color:var(--ink);margin:0;letter-spacing:-.02em}
.kpi .figure.warn{color:var(--warn)}.kpi .figure.bad{color:var(--bad)}
.kpi .unit{margin:7px 0 0;font-size:11.5px;color:var(--p42);letter-spacing:.02em}
.kpi .eyebrow{margin-bottom:12px}

.card{border:1px solid var(--p16);background:var(--p02);padding:22px 24px 24px;
margin-bottom:20px}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(420px,1fr));gap:20px}
.grid .card{margin:0}
.card .rows+.note,.card .meter+.rows{margin-top:22px}

table{width:100%;border-collapse:collapse;font-size:13.5px}
thead th{font-size:10.5px;letter-spacing:.11em;text-transform:uppercase;color:var(--p42);
font-weight:500;text-align:right;padding:0 0 12px 16px;border-bottom:1px solid var(--p16);
white-space:nowrap}
thead th:first-child{text-align:left;padding:0 16px 12px 0}
tbody th,tbody td{padding:13px 0;border-bottom:1px solid var(--p08);text-align:right;
font-family:var(--mono);font-variant-numeric:tabular-nums;white-space:nowrap}
tbody th{font-weight:400;text-align:left;color:var(--ink);font-size:13px;padding-right:16px}
tbody td{padding-left:16px}
tbody tr:last-child th,tbody tr:last-child td{border-bottom:0}
.wrap{overflow-x:auto;margin:0 -4px;padding:0 4px}
.errcell{display:flex;flex-direction:column;align-items:flex-end;gap:5px}
.errbar{width:52px;height:2px;background:var(--p08)}
.errbar i{display:block;height:100%;background:var(--bad)}

.rows{list-style:none;margin:0;padding:0}
.rows li{display:flex;justify-content:space-between;gap:18px;padding:11px 0;
border-bottom:1px solid var(--p08);font-size:13.5px}
.rows li:last-child{border-bottom:0}
.rows .k{color:var(--p62)}
.rows .v{font-family:var(--mono);font-variant-numeric:tabular-nums;color:var(--ink);
text-align:right;white-space:nowrap}

.meter{margin-top:22px}
.meter:first-of-type{margin-top:0}
.meter-head{display:flex;justify-content:space-between;gap:16px;font-size:12.5px;
align-items:baseline;margin-bottom:9px}
.meter-head .k{color:var(--p62)}
.meter-head .v{font-family:var(--mono);font-variant-numeric:tabular-nums;color:var(--p85);
white-space:nowrap}
.meter-head .v.warn{color:var(--warn)}
.meter-head .v.bad{color:var(--bad)}
.meter-foot{margin:9px 0 0;font-size:11.5px;color:var(--p42);font-family:var(--mono);
font-variant-numeric:tabular-nums}
.bar{height:3px;background:var(--p08);overflow:hidden}
.bar i{display:block;height:100%;background:var(--p42)}
.bar i.warn{background:var(--warn)}
.bar i.bad{background:var(--bad)}

.alerts{list-style:none;margin:0;padding:0}
.alerts li{border-left:2px solid var(--p22);padding:2px 0 2px 14px;margin-bottom:16px;
font-size:13.5px}
.alerts li:last-child{margin-bottom:0}
.alerts li.is-bad{border-left-color:var(--bad)}
.alerts li.is-ok{border-left-color:var(--ok)}
.alerts .name{color:var(--ink);font-family:var(--mono);font-size:13px}
.alerts .detail{margin:3px 0 0;color:var(--p62)}
.alerts .when{margin:3px 0 0;font-size:11.5px;color:var(--p42);font-family:var(--mono)}
.empty{color:var(--p42);font-size:13.5px;margin:0}

.note{border:1px solid var(--p08);background:transparent;padding:20px 24px;
font-size:12.5px;color:var(--p42);line-height:1.7;margin:0}
.note strong{color:var(--p62);font-weight:500}

@media (max-width:900px){.kpis{grid-template-columns:repeat(2,1fr)}}
@media (max-width:720px){
#app{padding-bottom:48px}
.kpis{grid-template-columns:1fr}
.grid{grid-template-columns:1fr}
.masthead,.statusbar{align-items:flex-start;flex-direction:column}
.kpi .figure{font-size:26px}
}
@media (prefers-reduced-motion:reduce){*{animation:none!important;transition:none!important}}
"#;

const SCRIPT: &str = r#"
(function () {
  var PERIOD = 15000;
  var timer;
  function schedule() { clearTimeout(timer); timer = setTimeout(run, PERIOD); }
  function run() {
    if (document.hidden) { schedule(); return; }
    fetch(location.href, { cache: 'no-store', credentials: 'same-origin' })
      .then(function (response) { return response.ok ? response.text() : null; })
      .then(function (text) {
        if (!text) return;
        var next = new DOMParser().parseFromString(text, 'text/html').getElementById('app');
        var current = document.getElementById('app');
        if (next && current) current.replaceWith(next);
      })
      .catch(function () {})
      .then(schedule);
  }
  document.addEventListener('visibilitychange', function () {
    if (!document.hidden) { clearTimeout(timer); run(); }
  });
  schedule();
})();
"#;

fn render(data: &DashboardData) -> String {
    let mut out = String::with_capacity(16 * 1024);
    out.push_str("<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">");
    out.push_str("<meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">");
    out.push_str("<title>PIR APM</title>");
    out.push_str(&format!(
        "<noscript><meta http-equiv=\"refresh\" content=\"{REFRESH_SECONDS}\"></noscript>"
    ));
    out.push_str("<style>");
    out.push_str(STYLE);
    out.push_str("</style></head><body><main id=\"app\">");

    out.push_str(&masthead(data));
    out.push_str(&statusbar(data));
    out.push_str(&kpis(data));
    out.push_str(&endpoint_table(data));
    out.push_str(&tier1_latency_split(data));
    out.push_str("<div class=\"grid\">");
    out.push_str(&service_card(data));
    out.push_str(&host_card(&data.host));
    out.push_str(&active_alerts_card(&data.active_alerts));
    out.push_str(&recent_alerts_card(&data.recent_alerts));
    out.push_str("</div>");
    out.push_str(&privacy_note());

    out.push_str("</main><script>");
    out.push_str(SCRIPT);
    out.push_str("</script></body></html>");
    out
}

fn masthead(data: &DashboardData) -> String {
    format!(
        "<header class=\"masthead\">\
<div class=\"brand\"><span class=\"mark\"></span>\
<div><h1>PIR APM</h1><p class=\"sub\">Valar Group &middot; Shielded Vote</p></div></div>\
<div class=\"tags\"><span class=\"pill env\">{environment}</span>\
<span class=\"pill\">{hostname}</span></div></header>",
        environment = escape(&data.environment),
        hostname = escape(&data.hostname),
    )
}

fn statusbar(data: &DashboardData) -> String {
    let scrape = match &data.scrape_error {
        Some(error) => chip("scrape", &escape(error), false),
        None => chip("scrape", "ok", true),
    };
    let updated = match data.last_scrape {
        Some(at) => format!(
            "<span class=\"live\"></span>Updated {} &middot; {}",
            relative_time(at),
            time(at)
        ),
        None => "<span class=\"live\"></span>Awaiting first scrape".to_string(),
    };
    format!(
        "<div class=\"statusbar\"><div class=\"chips\">{scrape}{health}{ready}</div>\
<div class=\"updated\">{updated}</div></div>",
        health = status_chip("health", data.health_status),
        ready = status_chip("ready", data.ready_status),
    )
}

fn chip(label: &str, value: &str, ok: bool) -> String {
    let tone = if ok { "is-ok" } else { "is-bad" };
    format!("<span class=\"chip {tone}\"><span class=\"dot\"></span>{label} <b>{value}</b></span>")
}

fn status_chip(label: &str, code: Option<u16>) -> String {
    match code {
        Some(code) => chip(label, &code.to_string(), (200..300).contains(&code)),
        None => chip(label, "n/a", false),
    }
}

fn kpis(data: &DashboardData) -> String {
    let windows: Vec<&EndpointWindow> = crate::metrics::ENDPOINTS
        .iter()
        .filter_map(|endpoint| data.endpoints.get(*endpoint))
        .collect();
    let total_qps: f64 = windows.iter().map(|window| window.qps).sum();
    let worst_p95 = crate::metrics::ENDPOINTS
        .iter()
        .filter_map(|endpoint| {
            data.endpoints
                .get(*endpoint)
                .and_then(|window| window.alert_latency(endpoint).p95)
        })
        .fold(None::<f64>, |acc, value| {
            Some(acc.map_or(value, |current: f64| current.max(value)))
        });
    // Round before deciding the tone so a value that displays as "0" is never
    // painted as an error.
    let errors = windows
        .iter()
        .map(|window| window.errors_5xx)
        .sum::<f64>()
        .round();
    let alerts = data.active_alerts.len();

    let mut out = String::from("<section class=\"kpis\">");
    out.push_str(&kpi(
        "Throughput",
        &format!("{total_qps:.3}"),
        "req/s over 5m",
        "",
    ));
    out.push_str(&kpi(
        "Worst server p95",
        &worst_p95
            .map(|value| format!("{value:.3}s"))
            .unwrap_or_else(|| "—".to_string()),
        "slowest alert basis",
        "",
    ));
    out.push_str(&kpi(
        "5xx responses",
        &format!("{errors:.0}"),
        "across all endpoints",
        if errors > 0.0 { "bad" } else { "" },
    ));
    out.push_str(&kpi(
        "Active alerts",
        &alerts.to_string(),
        if alerts == 1 {
            "check firing"
        } else {
            "checks firing"
        },
        if alerts > 0 { "bad" } else { "" },
    ));
    out.push_str("</section>");
    out
}

fn kpi(label: &str, figure: &str, unit: &str, tone: &str) -> String {
    format!(
        "<div class=\"kpi\"><p class=\"eyebrow\">{label}</p>\
<p class=\"figure{tone}\">{figure}</p><p class=\"unit\">{unit}</p></div>",
        tone = class_suffix(tone),
    )
}

/// Renders a tone as a trailing class name, collapsing the empty case so the
/// markup never carries a dangling `class="figure "`.
fn class_suffix(tone: &str) -> String {
    if tone.is_empty() {
        String::new()
    } else {
        format!(" {tone}")
    }
}

fn endpoint_table(data: &DashboardData) -> String {
    let rows = crate::metrics::ENDPOINTS
        .iter()
        .map(|endpoint| {
            let values = data.endpoints.get(*endpoint).cloned().unwrap_or_default();
            let budget = latency_budget(endpoint);
            format!(
                "<tr><th>{name}</th><td>{qps:.3}</td><td>{in_flight:.0}</td>\
<td>{requests:.0}</td>{p50}{p95}{p99}<td>{errors}</td></tr>",
                name = escape(endpoint),
                qps = values.qps,
                in_flight = values.in_flight,
                requests = values.requests,
                p50 = latency_cell(values.observed.p50, budget),
                p95 = latency_cell(values.observed.p95, budget),
                p99 = latency_cell(values.observed.p99, budget),
                errors = error_cell(&values),
            )
        })
        .collect::<String>();
    format!(
        "<section class=\"card\"><p class=\"eyebrow\">Observed PIR request latency &middot; 5 minute window</p>\
<div class=\"wrap\"><table><thead><tr><th>Endpoint</th><th>QPS</th><th>Inflight</th>\
<th>Requests</th><th>p50</th><th>p95</th><th>p99</th><th>5xx</th></tr></thead>\
<tbody>{rows}</tbody></table></div></section>"
    )
}

fn tier1_latency_split(data: &DashboardData) -> String {
    let values = data
        .endpoints
        .get("tier1_query")
        .cloned()
        .unwrap_or_default();
    let rows = [
        latency_split_row(
            "Observed total",
            &values.observed,
            values.in_flight,
            None,
            "informational",
        ),
        latency_split_row(
            "Server processing",
            &values.processing,
            values.processing_in_flight,
            Some(thresholds::TIER1_PROCESSING_P99_SECONDS),
            "pages on p99",
        ),
    ]
    .join("");
    format!(
        "<section class=\"card\"><p class=\"eyebrow\">Tier1 latency split &middot; 5 minute window</p>\
<div class=\"wrap\"><table><thead><tr><th>Stage</th><th>Samples</th><th>Inflight</th>\
<th>p50</th><th>p95</th><th>p99</th><th>Alerting</th></tr></thead>\
<tbody>{rows}</tbody></table></div>\
<p class=\"note\"><strong>Alert basis.</strong> Server processing begins after the complete request body reaches nf-server and is the only distribution evaluated against the {threshold:.3}s p99 latency threshold. Body-receive timing remains available in nf-server operator metrics and is not rendered here.</p></section>",
        threshold = thresholds::TIER1_PROCESSING_P99_SECONDS,
    )
}

fn latency_split_row(
    label: &str,
    latency: &LatencyWindow,
    in_flight: f64,
    budget: Option<f64>,
    policy: &str,
) -> String {
    format!(
        "<tr><th>{label}</th><td>{samples:.0}</td><td>{in_flight:.0}</td>\
{p50}{p95}{p99}<td class=\"muted\">{policy}</td></tr>",
        label = escape(label),
        samples = latency.samples,
        p50 = latency_cell(latency.p50, budget),
        p95 = latency_cell(latency.p95, budget),
        p99 = latency_cell(latency.p99, budget),
        policy = escape(policy),
    )
}

/// p99 alert threshold for the observed-duration table. Tier1 observed latency
/// is informational because its alert uses the processing-only distribution.
fn latency_budget(endpoint: &str) -> Option<f64> {
    match endpoint {
        "tier0" => Some(thresholds::TIER0_P99_SECONDS),
        "params_tier1" => Some(thresholds::PARAMS_P99_SECONDS),
        "tier1_query" => None,
        _ => None,
    }
}

fn latency_cell(value: Option<f64>, budget: Option<f64>) -> String {
    let Some(seconds) = value else {
        return "<td class=\"muted\">—</td>".to_string();
    };
    let tone = match budget {
        Some(budget) if seconds >= budget => "bad",
        Some(budget) if seconds >= budget * 0.6 => "warn",
        _ => "",
    };
    format!("<td class=\"{tone}\">{seconds:.3}s</td>")
}

fn error_cell(values: &EndpointWindow) -> String {
    let count = values.errors_5xx.round();
    if count <= 0.0 {
        return "<span class=\"muted\">0</span>".to_string();
    }
    let fill = ((values.error_ratio / thresholds::HTTP_5XX_RATIO) * 100.0).clamp(4.0, 100.0);
    format!(
        "<span class=\"errcell\"><span class=\"bad\">{count:.0} ({ratio:.2}%)</span>\
<span class=\"errbar\"><i style=\"width:{fill:.0}%\"></i></span></span>",
        ratio = values.error_ratio * 100.0,
    )
}

fn service_card(data: &DashboardData) -> String {
    let mut rows = String::new();
    if data.snapshot_gauges.is_empty() {
        rows.push_str("<li><span class=\"k\">Snapshot metrics</span><span class=\"v muted\">none yet</span></li>");
    } else {
        for (name, value) in &data.snapshot_gauges {
            rows.push_str(&format!(
                "<li><span class=\"k\" title=\"{raw}\">{name}</span>\
<span class=\"v\">{value}</span></li>",
                raw = escape(name),
                name = escape(&humanize_gauge(name)),
                value = format_number(*value),
            ));
        }
    }
    let memory = data
        .process_resident_memory_bytes
        .map(|bytes| bytes_human(bytes as u64))
        .unwrap_or_else(|| "unavailable".to_string());
    rows.push_str(&format!(
        "<li><span class=\"k\">Resident memory</span><span class=\"v\">{memory}</span></li>"
    ));
    let health_body = data
        .health_body
        .as_deref()
        .map(escape)
        .unwrap_or_else(|| "unavailable".to_string());
    format!(
        "<section class=\"card\"><p class=\"eyebrow\">Service &amp; snapshot</p>\
<ul class=\"rows\">{rows}</ul>\
<p class=\"note\"><strong>/health</strong> <code>{health_body}</code></p></section>"
    )
}

fn host_card(host: &HostHealth) -> String {
    let memory_used = if host.total_memory_bytes > 0 {
        1.0 - host.available_memory_bytes as f64 / host.total_memory_bytes as f64
    } else {
        0.0
    };
    let memory_tone = if host.available_memory_bytes < thresholds::MEMORY_AVAILABLE_BYTES {
        "bad"
    } else if host.available_memory_bytes < thresholds::MEMORY_AVAILABLE_BYTES * 2 {
        "warn"
    } else {
        ""
    };
    let disk_tone = if host.disk_used_ratio > thresholds::DISK_USED_RATIO {
        "bad"
    } else if host.disk_used_ratio > 0.80 {
        "warn"
    } else {
        ""
    };
    format!(
        "<section class=\"card\"><p class=\"eyebrow\">Host</p>\
{memory}{disk}\
<ul class=\"rows\">\
<li><span class=\"k\">Load 1 / 5 / 15</span>\
<span class=\"v\">{load_one:.2} · {load_five:.2} · {load_fifteen:.2}</span></li>\
<li><span class=\"k\">Data directory</span><span class=\"v\">{data_dir}</span></li>\
</ul></section>",
        memory = meter(
            "Memory",
            &format!(
                "{} free of {}",
                bytes_human(host.available_memory_bytes),
                bytes_human(host.total_memory_bytes)
            ),
            memory_used,
            memory_tone,
        ),
        disk = meter(
            "Disk",
            &format!(
                "{} free of {}",
                bytes_human(host.disk_available_bytes),
                bytes_human(host.disk_total_bytes)
            ),
            host.disk_used_ratio,
            disk_tone,
        ),
        load_one = host.load_one,
        load_five = host.load_five,
        load_fifteen = host.load_fifteen,
        data_dir = escape(&host.data_dir.display().to_string()),
    )
}

fn meter(label: &str, detail: &str, ratio: f64, tone: &str) -> String {
    let percent = (ratio * 100.0).clamp(0.0, 100.0);
    format!(
        "<div class=\"meter\"><div class=\"meter-head\"><span class=\"k\">{label}</span>\
<span class=\"v{tone_class}\">{percent:.0}% used</span></div>\
<div class=\"bar\"><i class=\"{tone}\" style=\"width:{percent:.1}%\"></i></div>\
<p class=\"meter-foot\">{detail}</p></div>",
        tone_class = class_suffix(tone),
    )
}

fn active_alerts_card(alerts: &[Alert]) -> String {
    if alerts.is_empty() {
        return "<section class=\"card\"><p class=\"eyebrow\">Active alerts</p>\
<ul class=\"alerts\"><li class=\"is-ok\"><p class=\"name\">All checks passing</p>\
<p class=\"detail\">No thresholds are currently breached.</p></li></ul></section>"
            .to_string();
    }
    let items = alerts
        .iter()
        .map(|alert| {
            format!(
                "<li class=\"is-bad\"><p class=\"name\">{check}</p>\
<p class=\"detail\">{observed} <span class=\"muted\">(threshold {threshold})</span></p>\
<p class=\"when\">firing {since}</p></li>",
                check = escape(&alert.check),
                observed = escape(&alert.observed),
                threshold = escape(&alert.threshold),
                since = relative_time(alert.fired_at),
            )
        })
        .collect::<String>();
    format!(
        "<section class=\"card\"><p class=\"eyebrow\">Active alerts</p>\
<ul class=\"alerts\">{items}</ul></section>"
    )
}

fn recent_alerts_card(recent: &[(SystemTime, String)]) -> String {
    if recent.is_empty() {
        return "<section class=\"card\"><p class=\"eyebrow\">Alert history</p>\
<p class=\"empty\">Nothing recorded since this sidecar started.</p></section>"
            .to_string();
    }
    let items = recent
        .iter()
        .map(|(at, message)| {
            format!(
                "<li><span class=\"k\">{message}</span><span class=\"v\">{when}</span></li>",
                message = escape(message),
                when = relative_time(*at),
            )
        })
        .collect::<String>();
    format!(
        "<section class=\"card\"><p class=\"eyebrow\">Alert history</p>\
<ul class=\"rows\">{items}</ul></section>"
    )
}

fn privacy_note() -> String {
    format!(
        "<p class=\"note\"><strong>Privacy.</strong> This sidecar scrapes only aggregate, \
allowlisted PIR metrics and local host health. It does not collect request bodies, nullifiers, \
IP addresses, headers, query identifiers, or any other per-user data. Every value on this page is \
server-rendered; the browser polls this dashboard every {REFRESH_SECONDS} seconds and never \
contacts the PIR server or its <code>/metrics</code> endpoint.</p>"
    )
}

/// Turns a raw gauge name like `nf_snapshot_age_seconds` into "Age (seconds)".
/// Callers keep the original name as a tooltip so an operator can still map a
/// row back to its Prometheus series.
fn humanize_gauge(name: &str) -> String {
    let stem = name.strip_prefix("nf_snapshot_").unwrap_or(name);
    let (stem, unit) = ["seconds", "bytes", "ratio"]
        .iter()
        .find_map(|unit| {
            stem.strip_suffix(&format!("_{unit}"))
                .filter(|rest| !rest.is_empty())
                .map(|rest| (rest, Some(*unit)))
        })
        .unwrap_or((stem, None));
    let mut label = stem.replace('_', " ");
    if let Some(first) = label.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    match unit {
        Some(unit) => format!("{label} ({unit})"),
        None => label,
    }
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

/// Human "3s ago" style age. Clock skew or a future timestamp collapses to
/// "just now" rather than rendering a negative duration.
fn relative_time(value: SystemTime) -> String {
    let Ok(elapsed) = SystemTime::now().duration_since(value) else {
        return "just now".to_string();
    };
    let seconds = elapsed.as_secs();
    match seconds {
        0..=4 => "just now".to_string(),
        5..=59 => format!("{seconds}s ago"),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86400),
    }
}

fn escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sample() -> DashboardData {
        let mut data = DashboardData::new(
            "staging".to_string(),
            "pir-primary".to_string(),
            HostHealth {
                load_one: 0.4,
                load_five: 0.5,
                load_fifteen: 0.6,
                total_memory_bytes: 16 * 1024 * 1024 * 1024,
                available_memory_bytes: 8 * 1024 * 1024 * 1024,
                disk_total_bytes: 200 * 1024 * 1024 * 1024,
                disk_available_bytes: 100 * 1024 * 1024 * 1024,
                disk_used_ratio: 0.5,
                data_dir: "/opt/nf-ingest/pir-data".into(),
            },
        );
        data.scrape_error = None;
        data.last_scrape = Some(SystemTime::now());
        data.health_status = Some(200);
        data.ready_status = Some(200);
        data.endpoints.insert(
            "tier0".to_string(),
            EndpointWindow {
                qps: 1.5,
                requests: 450.0,
                errors_5xx: 0.0,
                error_ratio: 0.0,
                observed: LatencyWindow {
                    samples: 450.0,
                    p50: Some(0.01),
                    p95: Some(0.05),
                    p99: Some(0.09),
                },
                in_flight: 2.0,
                ..Default::default()
            },
        );
        data
    }

    #[test]
    fn renders_a_complete_document() {
        let html = render(&sample());
        assert!(html.starts_with("<!doctype html>"));
        assert!(html.ends_with("</html>"));
        assert!(html.contains("id=\"app\""));
        assert!(html.contains("PIR APM"));
    }

    #[test]
    fn every_allowlisted_endpoint_gets_a_row_even_without_data() {
        let html = render(&sample());
        for endpoint in crate::metrics::ENDPOINTS {
            assert!(html.contains(endpoint), "missing row for {endpoint}");
        }
    }

    #[test]
    fn hostile_values_are_escaped() {
        let mut data = sample();
        data.hostname = "<script>alert(1)</script>".to_string();
        data.scrape_error = Some("bad \"quote\" & <tag>".to_string());
        let html = render(&data);
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
        assert!(html.contains("&quot;quote&quot;"));
    }

    #[test]
    fn latency_is_graded_against_the_alert_budget() {
        let budget = latency_budget("tier0");
        assert!(latency_cell(Some(0.01), budget).contains("class=\"\""));
        assert!(latency_cell(Some(0.7), budget).contains("warn"));
        assert!(latency_cell(Some(1.5), budget).contains("bad"));
        assert!(latency_cell(None, budget).contains("—"));
    }

    #[test]
    fn unknown_endpoints_have_no_latency_budget() {
        assert!(latency_budget("nope").is_none());
        assert!(latency_budget("tier1_query").is_none());
        assert!(!latency_cell(Some(99.0), None).contains("bad"));
    }

    #[test]
    fn tier1_split_keeps_body_receive_timing_off_the_public_dashboard() {
        let mut data = sample();
        data.endpoints.insert(
            "tier1_query".to_string(),
            EndpointWindow {
                observed: LatencyWindow {
                    samples: 20.0,
                    p99: Some(10.0),
                    ..Default::default()
                },
                processing: LatencyWindow {
                    samples: 20.0,
                    p99: Some(3.0),
                    ..Default::default()
                },
                in_flight: 3.0,
                processing_in_flight: 1.0,
                ..Default::default()
            },
        );
        let html = render(&data);
        assert!(html.contains("Tier1 latency split"));
        assert!(!html.contains("Body receive (upload proxy)"));
        assert!(html.contains("Server processing"));
        assert!(html.contains("only distribution evaluated"));
        assert!(html.contains("Body-receive timing remains available in nf-server operator metrics"));
        assert_eq!(html.matches("<td class=\"bad\">").count(), 1);
    }

    #[test]
    fn a_count_that_displays_as_zero_is_not_flagged_as_an_error() {
        let window = EndpointWindow {
            errors_5xx: 0.2,
            error_ratio: 0.001,
            ..EndpointWindow::default()
        };
        let cell = error_cell(&window);
        assert!(
            cell.contains("muted"),
            "sub-half count should read as clean"
        );
        assert!(!cell.contains("bad"));

        let mut data = sample();
        data.endpoints.insert("tier0".to_string(), window);
        assert!(!render(&data).contains("figure bad"));
    }

    #[test]
    fn meters_stay_within_the_track() {
        assert!(meter("Disk", "x", 1.8, "bad").contains("width:100.0%"));
        assert!(meter("Disk", "x", -0.5, "").contains("width:0.0%"));
    }

    #[test]
    fn gauge_names_become_readable_labels() {
        assert_eq!(humanize_gauge("nf_snapshot_height"), "Height");
        assert_eq!(
            humanize_gauge("nf_snapshot_nullifier_count"),
            "Nullifier count"
        );
        assert_eq!(humanize_gauge("nf_snapshot_age_seconds"), "Age (seconds)");
        assert_eq!(humanize_gauge("unprefixed_gauge"), "Unprefixed gauge");
        // A name that is nothing but a unit keeps its stem rather than
        // collapsing to a bare "()".
        assert_eq!(humanize_gauge("nf_snapshot_seconds"), "Seconds");
    }

    #[test]
    fn the_raw_gauge_name_survives_as_a_tooltip() {
        let mut data = sample();
        data.snapshot_gauges
            .insert("nf_snapshot_height".into(), 42.0);
        let html = render(&data);
        assert!(html.contains("title=\"nf_snapshot_height\""));
        assert!(html.contains(">Height<"));
    }

    #[test]
    fn relative_time_reads_naturally() {
        let now = SystemTime::now();
        assert_eq!(relative_time(now), "just now");
        assert_eq!(relative_time(now - Duration::from_secs(30)), "30s ago");
        assert_eq!(relative_time(now - Duration::from_secs(600)), "10m ago");
        assert_eq!(relative_time(now + Duration::from_secs(60)), "just now");
    }

    #[test]
    fn first_paint_before_any_scrape_is_renderable() {
        let data = DashboardData::new(
            "unknown".to_string(),
            "host".to_string(),
            HostHealth::default(),
        );
        let html = render(&data);
        assert!(html.contains("Awaiting first scrape"));
        assert!(html.contains("waiting for first scrape"));
    }
}
