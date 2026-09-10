//! Bounded, credential-redacting JSON-RPC transport used only by `verify-root`.
//! No lightwalletd configuration or standard sync state is consulted.

use std::{path::Path, time::Duration};

use anyhow::{bail, ensure, Context, Result};
use reqwest::{Client, StatusCode, Url};
use serde::Deserialize;
use zakura_chain::block;

use crate::root_verifier::MAX_BLOCK_BYTES;

// A raw block is hex-encoded in a small JSON envelope. Bound the envelope too.
const MAX_RESPONSE_BYTES: usize = MAX_BLOCK_BYTES * 2 + 65_536;
const ATTEMPTS: usize = 3;

pub(crate) struct RawBlockRpc {
    client: Client,
    url: Url,
    credentials: Option<(String, String)>,
}

enum FetchError {
    Transient,
    Invalid(anyhow::Error),
}

#[derive(Deserialize)]
struct Response {
    id: serde_json::Value,
    result: Option<String>,
    error: Option<serde_json::Value>,
}

impl RawBlockRpc {
    /// Configure an explicit HTTP(S) source, optionally reading a `user:password`
    /// cookie. Rejects URL credentials, fragments, zero timeouts, and bad cookies.
    /// Redirects are disabled; request/response bodies and credentials are never logged.
    pub(crate) fn new(url: &str, cookie_file: Option<&Path>, timeout_secs: u64) -> Result<Self> {
        let url = Url::parse(url).map_err(|_| anyhow::anyhow!("invalid block RPC URL"))?;
        ensure!(
            matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
            "block RPC URL must use HTTP or HTTPS"
        );
        ensure!(
            url.username().is_empty() && url.password().is_none() && url.fragment().is_none(),
            "use a cookie file for RPC credentials; URL credentials and fragments are forbidden"
        );
        ensure!(timeout_secs > 0, "HTTP timeout must be positive");
        let credentials = cookie_file
            .map(|path| -> Result<_> {
                let cookie = std::fs::read_to_string(path).context("read RPC cookie file")?;
                let (user, password) = cookie
                    .trim()
                    .split_once(':')
                    .context("RPC cookie must contain user:password")?;
                ensure!(
                    !user.is_empty() && !password.is_empty(),
                    "RPC cookie credentials must be nonempty"
                );
                Ok((user.to_owned(), password.to_owned()))
            })
            .transpose()?;
        let client = Client::builder()
            .timeout(Duration::from_secs(timeout_secs))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("build block RPC client")?;
        Ok(Self {
            client,
            url,
            credentials,
        })
    }

    /// Fetch a complete raw block by hash. Retries transient transport, 429, and
    /// 5xx failures at most three times; malformed/oversized data fails immediately.
    /// A returned block is untrusted until `root_verifier::verify_block` accepts it.
    pub(crate) async fn fetch(&self, hash: block::Hash) -> Result<Vec<u8>> {
        for attempt in 0..ATTEMPTS {
            match self.fetch_once(hash).await {
                Ok(raw) => return Ok(raw),
                Err(FetchError::Invalid(error)) => return Err(error),
                Err(FetchError::Transient) if attempt + 1 < ATTEMPTS => {
                    tokio::time::sleep(Duration::from_millis(250 * (attempt as u64 + 1))).await;
                }
                Err(FetchError::Transient) => {
                    bail!("block RPC transport failed after {ATTEMPTS} attempts")
                }
            }
        }
        unreachable!("the last attempt always returns")
    }

    /// Make one bounded request. Deliberately discard provider error text and
    /// reqwest error details, which could echo credentials or sensitive URLs.
    async fn fetch_once(&self, hash: block::Hash) -> std::result::Result<Vec<u8>, FetchError> {
        let mut request = self.client.post(self.url.clone()).json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1, "method": "getblock", "params": [hash.to_string(), 0]
        }));
        if let Some((user, password)) = &self.credentials {
            request = request.basic_auth(user, Some(password));
        }
        let mut response = request.send().await.map_err(|_| FetchError::Transient)?;
        let status = response.status();
        if status.is_server_error() || status == StatusCode::TOO_MANY_REQUESTS {
            return Err(FetchError::Transient);
        }
        if !status.is_success() {
            return Err(FetchError::Invalid(anyhow::anyhow!(
                "block RPC returned HTTP status {}",
                status.as_u16()
            )));
        }
        let mut body = Vec::new();
        if response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
        {
            return Err(FetchError::Invalid(anyhow::anyhow!(
                "block RPC response exceeds size limit"
            )));
        }
        while let Some(chunk) = response.chunk().await.map_err(|_| FetchError::Transient)? {
            if chunk.len() > MAX_RESPONSE_BYTES - body.len() {
                return Err(FetchError::Invalid(anyhow::anyhow!(
                    "block RPC response exceeds size limit"
                )));
            }
            body.extend_from_slice(&chunk);
        }
        decode_response(&body).map_err(FetchError::Invalid)
    }
}

/// Decode an RPC envelope without including provider-controlled text in errors.
fn decode_response(body: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        body.len() <= MAX_RESPONSE_BYTES,
        "block RPC response exceeds size limit"
    );
    let response: Response =
        serde_json::from_slice(body).map_err(|_| anyhow::anyhow!("invalid block RPC response"))?;
    ensure!(
        response.id == serde_json::json!(1),
        "block RPC response ID mismatch"
    );
    ensure!(
        response.error.is_none(),
        "block RPC reported an error (block may be unavailable)"
    );
    let encoded = response.result.context("block RPC omitted the raw block")?;
    ensure!(
        encoded.len() <= MAX_BLOCK_BYTES * 2,
        "raw block exceeds size limit"
    );
    hex::decode(encoded).map_err(|_| anyhow::anyhow!("block RPC returned invalid raw-block hex"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_envelope_checks_and_error_redaction() {
        assert_eq!(
            decode_response(br#"{"id":1,"result":"abcd","error":null}"#).unwrap(),
            [0xab, 0xcd]
        );
        for body in [
            br#"{"id":2,"result":"abcd"}"#.as_slice(),
            br#"{"id":1,"result":null}"#,
            br#"{"id":1,"result":"0"}"#,
            br#"{"id":1,"result":"zz"}"#,
            br#"{"id":1,"result":"00","error":{"message":"credential-canary"}}"#,
            b"not-json credential-canary",
        ] {
            let err = decode_response(body).unwrap_err().to_string();
            assert!(!err.contains("credential-canary"));
        }
        let encoded = serde_json::to_vec(
            &serde_json::json!({"id":1,"result":"00".repeat(MAX_BLOCK_BYTES + 1)}),
        )
        .unwrap();
        assert!(decode_response(&encoded)
            .unwrap_err()
            .to_string()
            .contains("size limit"));
        assert!(decode_response(&vec![b' '; MAX_RESPONSE_BYTES + 1]).is_err());
    }

    #[test]
    fn explicit_url_and_cookie_contract() {
        for url in [
            "file:///tmp/node",
            "not a url",
            "http://name:credential-canary@localhost/",
            "http://localhost/#fragment",
        ] {
            let err = RawBlockRpc::new(url, None, 1).err().unwrap().to_string();
            assert!(!err.contains("credential-canary"));
        }
        assert!(RawBlockRpc::new("http://127.0.0.1:8232", None, 0).is_err());
        let dir = tempfile::tempdir().unwrap();
        let cookie = dir.path().join("cookie");
        for value in ["", "credential-canary", ":password", "user:"] {
            std::fs::write(&cookie, value).unwrap();
            assert!(RawBlockRpc::new("http://127.0.0.1:8232", Some(&cookie), 1).is_err());
        }
        std::fs::write(&cookie, "user:password\n").unwrap();
        assert!(RawBlockRpc::new("http://127.0.0.1:8232", Some(&cookie), 1).is_ok());
    }
}
