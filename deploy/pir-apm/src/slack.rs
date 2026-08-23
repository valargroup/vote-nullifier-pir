use anyhow::Result;
use reqwest::Client;

use crate::{alerts::Alert, config::Config};

#[derive(Clone)]
pub struct SlackNotifier {
    client: Client,
    webhook_url: Option<String>,
    environment: String,
    hostname: String,
}

impl SlackNotifier {
    pub fn new(config: &Config) -> Self {
        Self {
            client: Client::new(),
            webhook_url: config.slack_webhook_url.clone(),
            environment: config.environment.clone(),
            hostname: config.hostname.clone(),
        }
    }

    pub async fn fire(&self, alert: &Alert) -> Result<()> {
        self.send(format!(
            "🔥 PIR APM ALERT\nEnvironment: {}\nHostname: {}\nCheck: {}\nObserved: {}\nThreshold: {}\nDashboard: /apm/",
            self.environment, self.hostname, alert.check, alert.observed, alert.threshold
        ))
        .await
    }

    pub async fn recover(&self, alert: &Alert) -> Result<()> {
        self.send(format!(
            "✅ PIR APM RECOVERY\nEnvironment: {}\nHostname: {}\nCheck: {}\nObserved: clear\nThreshold: {}\nDashboard: /apm/",
            self.environment, self.hostname, alert.check, alert.threshold
        ))
        .await
    }

    pub async fn test(&self) -> Result<()> {
        self.send(format!(
            "🧪 PIR APM TEST ALERT\nEnvironment: {}\nHostname: {}\nCheck: test_alert\nObserved: CLI test requested\nThreshold: n/a\nDashboard: /apm/",
            self.environment, self.hostname
        ))
        .await
    }

    pub async fn forced_high_latency(&self) -> Result<()> {
        let alert = Alert {
            check: "high_latency".to_string(),
            observed: "synthetic staging verification".to_string(),
            threshold: "forced by --force-alert".to_string(),
            fired_at: std::time::SystemTime::now(),
        };
        self.fire(&alert).await?;
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        self.recover(&alert).await
    }

    async fn send(&self, text: String) -> Result<()> {
        let Some(webhook_url) = &self.webhook_url else {
            eprintln!("Slack webhook not configured; notification logged only: {text}");
            return Ok(());
        };
        let response = self
            .client
            .post(webhook_url)
            .json(&serde_json::json!({ "text": text }))
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("Slack notification request failed"))?;
        if !response.status().is_success() {
            anyhow::bail!("Slack notification returned HTTP {}", response.status());
        }
        Ok(())
    }
}
