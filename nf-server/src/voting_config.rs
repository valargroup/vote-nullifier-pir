//! Resolve the active on-chain round snapshot height from the published
//! `voting-config.json` service-discovery URL.

use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct VotingConfig {
    #[serde(default)]
    vote_servers: Vec<ServiceEntry>,
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
struct RoundsResponse {
    #[serde(default)]
    rounds: Vec<ChainRound>,
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

/// GET the wallet-facing config, then query configured vote servers until one
/// returns a chain round `snapshot_height`.
pub async fn fetch_active_round_snapshot_height(
    config_url: &str,
    timeout: Duration,
) -> Result<Option<u64>> {
    let client = reqwest::Client::builder()
        .timeout(timeout)
        .build()
        .context("build reqwest client")?;

    let resp = client
        .get(config_url)
        .send()
        .await
        .with_context(|| format!("GET {config_url}"))?
        .error_for_status()
        .with_context(|| format!("GET {config_url} (non-2xx)"))?;
    let cfg: VotingConfig = resp
        .json()
        .await
        .with_context(|| format!("decode {config_url} as voting-config"))?;

    let vote_servers: Vec<&str> = cfg
        .vote_servers
        .iter()
        .map(|entry| entry.url.trim_end_matches('/'))
        .filter(|url| !url.is_empty())
        .collect();
    if vote_servers.is_empty() {
        bail!("voting-config at {config_url} has no vote_servers");
    }

    let mut saw_no_active_round = false;
    let mut first_error: Option<anyhow::Error> = None;
    for vote_server in vote_servers {
        match fetch_vote_server_active_snapshot_height(&client, vote_server).await {
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
    if height.is_some() {
        return Ok(height);
    }

    fetch_vote_server_rounds_snapshot_height(client, vote_server).await
}

async fn fetch_vote_server_rounds_snapshot_height(
    client: &reqwest::Client,
    vote_server: &str,
) -> Result<Option<u64>> {
    let rounds_url = format!("{vote_server}/shielded-vote/v1/rounds");
    let resp = client
        .get(&rounds_url)
        .send()
        .await
        .with_context(|| format!("GET {rounds_url}"))?
        .error_for_status()
        .with_context(|| format!("GET {rounds_url} (non-2xx)"))?;
    let rounds: RoundsResponse = resp
        .json()
        .await
        .with_context(|| format!("decode {rounds_url} as rounds"))?;

    rounds
        .rounds
        .into_iter()
        .find_map(|round| round.snapshot_height)
        .map(JsonU64::parse)
        .transpose()
}

/// Same as [`fetch_active_round_snapshot_height`] but requires a numeric height.
pub async fn fetch_required_active_round_snapshot_height(
    config_url: &str,
    timeout: Duration,
) -> Result<u64> {
    match fetch_active_round_snapshot_height(config_url, timeout).await? {
        Some(height) => Ok(height),
        None => {
            bail!(
                "no on-chain round with snapshot_height discovered via voting-config at {config_url}; \
                 disable check with empty --voting-config-url / SVOTE_PIR_VOTING_CONFIG_URL"
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    fn spawn_one_request_server(body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    fn spawn_one_status_server(status: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    fn spawn_two_request_server(first_body: &'static str, second_body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        thread::spawn(move || {
            for body in [first_body, second_body] {
                let (mut stream, _) = listener.accept().unwrap();
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    #[tokio::test(flavor = "current_thread")]
    async fn resolves_snapshot_height_from_active_round() {
        let active_server = spawn_one_request_server(r#"{"round":{"snapshot_height":"3317510"}}"#);
        let config_body = format!(
            r#"{{"vote_servers":[{{"url":"{}","label":"primary"}}]}}"#,
            active_server
        );
        let config_server = spawn_one_request_server(Box::leak(config_body.into_boxed_str()));
        let height = fetch_required_active_round_snapshot_height(
            &format!("{config_server}/voting-config.json"),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(height, 3_317_510);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn falls_back_to_rounds_list_when_active_round_is_null() {
        let active_server = spawn_two_request_server(
            r#"{"round":null}"#,
            r#"{"rounds":[{"snapshot_height":3317515}]}"#,
        );
        let config_body = format!(
            r#"{{"vote_servers":[{{"url":"{}","label":"primary"}}]}}"#,
            active_server
        );
        let config_server = spawn_one_request_server(Box::leak(config_body.into_boxed_str()));
        let height = fetch_required_active_round_snapshot_height(
            &format!("{config_server}/voting-config.json"),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(height, 3_317_515);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn falls_back_to_later_vote_server() {
        let active_server = spawn_one_request_server(r#"{"round":{"snapshot_height":3317520}}"#);
        let config_body = format!(
            r#"{{"vote_servers":[{{"url":"http://127.0.0.1:1","label":"down"}},{{"url":"{}","label":"secondary"}}]}}"#,
            active_server
        );
        let config_server = spawn_one_request_server(Box::leak(config_body.into_boxed_str()));
        let height = fetch_required_active_round_snapshot_height(
            &format!("{config_server}/voting-config.json"),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(height, 3_317_520);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn falls_back_after_non_success_vote_server() {
        let failing_server =
            spawn_one_status_server("500 Internal Server Error", r#"{"error":"boom"}"#);
        let active_server = spawn_one_request_server(r#"{"round":{"snapshot_height":3317530}}"#);
        let config_body = format!(
            r#"{{"vote_servers":[{{"url":"{}","label":"failing"}},{{"url":"{}","label":"secondary"}}]}}"#,
            failing_server, active_server
        );
        let config_server = spawn_one_request_server(Box::leak(config_body.into_boxed_str()));
        let height = fetch_required_active_round_snapshot_height(
            &format!("{config_server}/voting-config.json"),
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(height, 3_317_530);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn errors_when_config_has_no_vote_servers() {
        let config_server = spawn_one_request_server(r#"{"vote_servers":[]}"#);
        let err = fetch_required_active_round_snapshot_height(
            &format!("{config_server}/voting-config.json"),
            Duration::from_secs(5),
        )
        .await
        .err()
        .expect("expected missing vote_servers error");
        assert!(format!("{err:#}").contains("vote_servers"));
    }
}
