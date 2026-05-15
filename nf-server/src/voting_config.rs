//! Resolve the active on-chain round snapshot height from the published
//! voting service-discovery URL.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ServiceDiscoveryConfig {
    #[serde(default)]
    vote_servers: Vec<ServiceEntry>,
    #[serde(default)]
    dynamic_config_url: Option<String>,
    #[serde(default)]
    static_config_version: Option<u32>,
}

#[derive(Debug, Deserialize)]
struct ServiceEntry {
    url: String,
}

#[derive(Debug, Deserialize)]
struct ActiveRoundResponse {
    #[serde(default)]
    round: Option<ChainRound>,
}

#[derive(Debug, Deserialize)]
struct ChainRound {
    #[serde(default)]
    snapshot_height: Option<JsonU64>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum JsonU64 {
    Number(u64),
    String(String),
}

impl JsonU64 {
    fn parse(self) -> Result<u64> {
        match self {
            JsonU64::Number(n) => Ok(n),
            JsonU64::String(s) => s
                .parse::<u64>()
                .with_context(|| format!("parse snapshot_height {s:?}")),
        }
    }
}

async fn fetch_discovery_config(
    client: &reqwest::Client,
    url: &str,
) -> Result<ServiceDiscoveryConfig> {
    let resp = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("GET {url} (non-2xx)"))?;
    resp.json()
        .await
        .with_context(|| format!("decode {url} as voting service-discovery config"))
}

fn configured_vote_servers<'a>(cfg: &'a ServiceDiscoveryConfig, url: &str) -> Result<Vec<&'a str>> {
    let vote_servers: Vec<&str> = cfg
        .vote_servers
        .iter()
        .map(|entry| entry.url.trim_end_matches('/'))
        .filter(|url| !url.is_empty())
        .collect();
    if vote_servers.is_empty() {
        bail!("dynamic voting-config at {url} has no vote_servers");
    }
    Ok(vote_servers)
}

async fn resolve_vote_servers(client: &reqwest::Client, url: &str) -> Result<Vec<String>> {
    let cfg = fetch_discovery_config(client, url).await?;

    if let Some(dynamic_url) = cfg.dynamic_config_url.as_deref() {
        let dynamic_url = dynamic_url.trim();
        if dynamic_url.is_empty() {
            bail!("static voting-config at {url} has an empty dynamic_config_url");
        }
        let dynamic = fetch_discovery_config(client, dynamic_url).await?;
        return configured_vote_servers(&dynamic, dynamic_url)
            .map(|servers| servers.into_iter().map(ToOwned::to_owned).collect());
    }

    match configured_vote_servers(&cfg, url) {
        Ok(servers) => Ok(servers.into_iter().map(ToOwned::to_owned).collect()),
        Err(e) if cfg.static_config_version.is_some() => Err(e)
            .with_context(|| format!("static voting-config at {url} has no dynamic_config_url")),
        Err(e) => Err(e).with_context(|| format!("voting-config at {url} is not usable")),
    }
}

/// GET the wallet-facing config, then query configured vote servers until one
/// returns the active chain round `snapshot_height`.
pub async fn fetch_voting_snapshot_height(url: &str, timeout: Duration) -> Result<Option<u64>> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .context("build reqwest client")?;
    let vote_servers = resolve_vote_servers(&client, url).await?;

    let mut saw_no_active_round = false;
    let mut first_error: Option<anyhow::Error> = None;
    for vote_server in vote_servers {
        match fetch_vote_server_active_snapshot_height(&client, &vote_server).await {
            Ok(Some(height)) => return Ok(Some(height)),
            Ok(None) => saw_no_active_round = true,
            Err(err) => {
                if first_error.is_none() {
                    first_error = Some(err);
                }
            }
        }
    }

    if saw_no_active_round {
        Ok(None)
    } else if let Some(err) = first_error {
        Err(err)
    } else {
        Ok(None)
    }
}

async fn fetch_vote_server_active_snapshot_height(
    client: &reqwest::Client,
    vote_server: &str,
) -> Result<Option<u64>> {
    let active_url = format!("{vote_server}/shielded-vote/v1/rounds/active");
    let resp = client
        .get(&active_url)
        .send()
        .await
        .with_context(|| format!("GET {active_url}"))?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let resp = resp
        .error_for_status()
        .with_context(|| format!("GET {active_url} (non-2xx)"))?;
    let active: ActiveRoundResponse = resp
        .json()
        .await
        .with_context(|| format!("decode {active_url} as active round"))?;

    let height = active
        .round
        .and_then(|round| round.snapshot_height)
        .map(JsonU64::parse)
        .transpose()?;
    Ok(height)
}

/// Same as [`fetch_voting_snapshot_height`] but requires a numeric height.
pub async fn fetch_required_snapshot_height(url: &str, timeout: Duration) -> Result<u64> {
    fetch_voting_snapshot_height(url, timeout)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no on-chain round with snapshot_height discovered via voting-config at {url}; disable check with empty --voting-config-url / SVOTE_PIR_VOTING_CONFIG_URL"
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn spawn_server<F>(requests: usize, handler_factory: impl FnOnce(String) -> F) -> String
    where
        F: Fn(&str) -> (u16, String) + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handler = handler_factory(base.clone());
        thread::spawn(move || {
            for _ in 0..requests {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 4096];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let path = req
                    .lines()
                    .next()
                    .and_then(|line| line.split_whitespace().nth(1))
                    .unwrap_or("/");
                let (status, body) = handler(path);
                let reason = match status {
                    200 => "OK",
                    404 => "Not Found",
                    _ => "Error",
                };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(resp.as_bytes()).unwrap();
            }
        });
        base
    }

    #[tokio::test(flavor = "current_thread")]
    async fn direct_dynamic_config_returns_active_height() {
        let base = spawn_server(2, |base| {
            move |path| match path {
                "/config.json" => (
                    200,
                    format!(r#"{{"vote_servers":[{{"url":"{base}/vote"}}]}}"#),
                ),
                "/vote/shielded-vote/v1/rounds/active" => {
                    (200, r#"{"round":{"snapshot_height":"120"}}"#.to_string())
                }
                _ => (404, "{}".to_string()),
            }
        });

        let height =
            fetch_voting_snapshot_height(&format!("{base}/config.json"), Duration::from_secs(5))
                .await
                .unwrap();
        assert_eq!(height, Some(120));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn static_config_follows_dynamic_config_url() {
        let base = spawn_server(3, |base| {
            move |path| match path {
                "/static.json" => (
                    200,
                    format!(
                        r#"{{"static_config_version":1,"dynamic_config_url":"{base}/dynamic.json"}}"#
                    ),
                ),
                "/dynamic.json" => (
                    200,
                    format!(r#"{{"vote_servers":[{{"url":"{base}/vote"}}]}}"#),
                ),
                "/vote/shielded-vote/v1/rounds/active" => {
                    (200, r#"{"round":{"snapshot_height":130}}"#.to_string())
                }
                _ => (404, "{}".to_string()),
            }
        });

        let height =
            fetch_voting_snapshot_height(&format!("{base}/static.json"), Duration::from_secs(5))
                .await
                .unwrap();
        assert_eq!(height, Some(130));
    }

    #[tokio::test(flavor = "current_thread")]
    async fn direct_dynamic_config_returns_none_when_no_active_round() {
        let base = spawn_server(2, |base| {
            move |path| match path {
                "/config.json" => (
                    200,
                    format!(r#"{{"vote_servers":[{{"url":"{base}/vote"}}]}}"#),
                ),
                "/vote/shielded-vote/v1/rounds/active" => (404, "{}".to_string()),
                _ => (404, "{}".to_string()),
            }
        });

        let height =
            fetch_voting_snapshot_height(&format!("{base}/config.json"), Duration::from_secs(5))
                .await
                .unwrap();
        assert_eq!(height, None);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn static_config_rejects_empty_dynamic_config_url() {
        let base = spawn_server(1, |_base| {
            move |path| match path {
                "/static.json" => (
                    200,
                    r#"{"static_config_version":1,"dynamic_config_url":""}"#.to_string(),
                ),
                _ => (404, "{}".to_string()),
            }
        });

        let err =
            fetch_voting_snapshot_height(&format!("{base}/static.json"), Duration::from_secs(5))
                .await
                .err()
                .expect("expected error");
        let s = format!("{err:#}");
        assert!(
            s.contains("empty dynamic_config_url"),
            "unexpected error: {s}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn dynamic_config_requires_vote_servers() {
        let base = spawn_server(2, |base| {
            move |path| match path {
                "/static.json" => (
                    200,
                    format!(
                        r#"{{"static_config_version":1,"dynamic_config_url":"{base}/dynamic.json"}}"#
                    ),
                ),
                "/dynamic.json" => (200, r#"{"pir_endpoints":[]}"#.to_string()),
                _ => (404, "{}".to_string()),
            }
        });

        let err =
            fetch_voting_snapshot_height(&format!("{base}/static.json"), Duration::from_secs(5))
                .await
                .err()
                .expect("expected error");
        let s = format!("{err:#}");
        assert!(s.contains("no vote_servers"), "unexpected error: {s}");
    }
}
