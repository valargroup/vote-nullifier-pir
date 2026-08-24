mod alerts;
mod config;
mod dashboard;
mod host;
mod metrics;
mod slack;
mod thresholds;

use std::{
    sync::Arc,
    time::{Instant, SystemTime},
};

use alerts::{AlertEngine, AlertInput, AlertTransition};
use anyhow::{Context, Result};
use axum::{routing::get, Router};
use clap::{Parser, ValueEnum};
use config::Config;
use dashboard::{DashboardData, SharedDashboard};
use metrics::RollingMetrics;
use reqwest::Client;
use slack::SlackNotifier;
use tokio::sync::RwLock;

#[derive(Debug, Parser)]
#[command(about = "Local PIR metrics dashboard and alerting sidecar")]
struct Cli {
    /// Send a test notification and exit.
    #[arg(long)]
    send_test_alert: bool,

    /// Fire a synthetic alert, then a recovery, and exit.
    #[arg(long, value_enum)]
    force_alert: Option<ForcedAlert>,
}

#[derive(Clone, Debug, ValueEnum)]
enum ForcedAlert {
    #[value(name = "high_latency")]
    HighLatency,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::from_env()?;
    let notifier = SlackNotifier::new(&config);
    if cli.send_test_alert {
        notifier.test().await?;
        return Ok(());
    }
    if matches!(cli.force_alert, Some(ForcedAlert::HighLatency)) {
        notifier.forced_high_latency().await?;
        return Ok(());
    }

    let initial_host = host::collect(&config.data_dir);
    let dashboard: SharedDashboard = Arc::new(RwLock::new(DashboardData::new(
        config.environment.clone(),
        config.hostname.clone(),
        initial_host,
    )));
    let scrape_dashboard = Arc::clone(&dashboard);
    let scrape_config = config.clone();
    tokio::spawn(async move {
        if let Err(error) = scrape_loop(scrape_config, scrape_dashboard, notifier).await {
            eprintln!("scrape loop stopped: {error:#}");
        }
    });

    let app = Router::new()
        .route("/", get(dashboard::index))
        .route("/apm", get(dashboard::index))
        .route("/apm/", get(dashboard::index))
        .route("/healthz", get(dashboard::healthz))
        .with_state(dashboard);
    let listener = tokio::net::TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("binding dashboard listener {}", config.listen))?;
    eprintln!("PIR APM listening on {}", config.listen);
    axum::serve(listener, app)
        .await
        .context("serving dashboard")
}

async fn scrape_loop(
    config: Config,
    dashboard: SharedDashboard,
    notifier: SlackNotifier,
) -> Result<()> {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .context("building HTTP client")?;
    let mut rolling = RollingMetrics::default();
    let mut alerts = AlertEngine::default();
    let mut interval = tokio::time::interval(config.interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        interval.tick().await;
        let now = Instant::now();
        let (metrics_result, health_result, ready_result) = tokio::join!(
            fetch(&client, &config.scrape_url, "/metrics", true),
            fetch(&client, &config.scrape_url, "/health", false),
            fetch(&client, &config.scrape_url, "/ready", false),
        );
        let health_status = health_result.as_ref().ok().map(|response| response.status);
        let ready_status = ready_result.as_ref().ok().map(|response| response.status);
        let ready_ok = ready_status.is_some_and(|status| (200..300).contains(&status));
        let scrape_ok = metrics_result.is_ok() && health_result.is_ok() && ready_result.is_ok();
        let host = host::collect(&config.data_dir);
        let mut scrape_error = None;

        match metrics_result {
            Ok(response) => match metrics::parse_prometheus(&response.body, now) {
                Ok(snapshot) => rolling.push(snapshot),
                Err(error) => scrape_error = Some(format!("metrics parse failed: {error}")),
            },
            Err(error) => scrape_error = Some(error),
        }
        if scrape_error.is_none() {
            if let Err(error) = &health_result {
                scrape_error = Some(error.clone());
            } else if let Err(error) = &ready_result {
                scrape_error = Some(error.clone());
            }
        }

        let windows = rolling.windows();
        let transitions = alerts.evaluate(AlertInput {
            now,
            scrape_ok: scrape_ok && scrape_error.is_none(),
            ready_ok,
            endpoints: &windows,
            host: &host,
        });
        {
            let latest = rolling.latest();
            let mut view = dashboard.write().await;
            view.last_scrape = Some(SystemTime::now());
            view.scrape_error = scrape_error;
            view.health_status = health_status;
            view.health_body = health_result
                .as_ref()
                .ok()
                .map(|response| response.body.clone());
            view.ready_status = ready_status;
            view.endpoints = windows;
            view.snapshot_gauges = latest
                .map(|snapshot| snapshot.snapshot_gauges.clone())
                .unwrap_or_default();
            view.process_resident_memory_bytes =
                latest.and_then(|snapshot| snapshot.resident_memory_bytes);
            view.host = host;
            view.active_alerts = alerts.active();
            view.recent_alerts = alerts.recent();
        }
        for transition in transitions {
            let result = match transition {
                AlertTransition::Fired(alert) => notifier.fire(&alert).await,
                AlertTransition::Recovered(alert) => notifier.recover(&alert).await,
            };
            if let Err(error) = result {
                eprintln!("Slack notification failed: {error}");
            }
        }
    }
}

struct FetchResponse {
    status: u16,
    body: String,
}

async fn fetch(
    client: &Client,
    base_url: &str,
    path: &str,
    require_success: bool,
) -> Result<FetchResponse, String> {
    let response = client
        .get(format!("{base_url}{path}"))
        .send()
        .await
        .map_err(|error| format!("{path} request failed: {}", error.without_url()))?;
    let status = response.status();
    if require_success && !status.is_success() {
        return Err(format!("{path} returned HTTP {}", status.as_u16()));
    }
    let body = response
        .text()
        .await
        .map_err(|error| format!("{path} body failed: {}", error.without_url()))?;
    Ok(FetchResponse {
        status: status.as_u16(),
        body,
    })
}
