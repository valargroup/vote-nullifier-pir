use std::{env, net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Context, Result};

#[derive(Clone, Debug)]
pub struct Config {
    pub scrape_url: String,
    pub listen: SocketAddr,
    pub slack_webhook_url: Option<String>,
    pub environment: String,
    pub hostname: String,
    pub data_dir: PathBuf,
    pub interval: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let scrape_url = env_value("PIR_APM_SCRAPE_URL", "http://127.0.0.1:3000")
            .trim_end_matches('/')
            .to_string();
        let listen = env_value("PIR_APM_LISTEN", "127.0.0.1:3002")
            .parse()
            .context("PIR_APM_LISTEN must be an IP:port socket address")?;
        let slack_webhook_url = env::var("PIR_APM_SLACK_WEBHOOK_URL")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let interval_seconds: u64 = env_value("PIR_APM_INTERVAL_SECONDS", "15")
            .parse()
            .context("PIR_APM_INTERVAL_SECONDS must be an integer")?;
        if interval_seconds == 0 {
            anyhow::bail!("PIR_APM_INTERVAL_SECONDS must be greater than zero");
        }
        Ok(Self {
            scrape_url,
            listen,
            slack_webhook_url,
            environment: env_value("PIR_APM_ENVIRONMENT", "unknown"),
            hostname: env::var("PIR_APM_HOSTNAME")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .or_else(sysinfo::System::host_name)
                .unwrap_or_else(|| "unknown".to_string()),
            data_dir: PathBuf::from(env_value("PIR_APM_DATA_DIR", "/opt/nf-ingest/pir-data")),
            interval: Duration::from_secs(interval_seconds),
        })
    }
}

fn env_value(name: &str, default: &str) -> String {
    env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}
