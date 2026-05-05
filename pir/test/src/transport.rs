//! HTTP transport used only by the `pir-test` harness.
//!
//! `pir-client` deliberately exposes an abstract transport so library
//! consumers do not inherit a particular HTTP stack. The test binary still
//! needs a real network client for `server`, `bench-server`, and `load` modes,
//! so it keeps this Hyper/Rustls implementation locally.

use anyhow::{Context, Result};
use bytes::Bytes;
use http::{Method, Request};
use http_body_util::{BodyExt, Full};
use hyper_rustls::HttpsConnector;
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use pir_client::{Transport, TransportFuture, TransportResponse};

type RequestBody = Full<Bytes>;
type HyperClient = Client<HttpsConnector<HttpConnector>, RequestBody>;

pub struct HyperTransport {
    client: HyperClient,
}

impl HyperTransport {
    /// Default harness transport: allow HTTP/2 so production smoke tests and
    /// load tests exercise the same multiplexed path wallets normally use.
    pub fn new() -> Self {
        Self::builder(true)
    }

    /// Force HTTP/1.1 on a single pooled TLS connection.
    ///
    /// `bench-server --mode single-tls` uses this to separate per-query upload
    /// bandwidth from HTTP/2 stream contention when investigating latency.
    pub fn http1_only() -> Self {
        Self::builder(false)
    }

    fn builder(enable_http2: bool) -> Self {
        let mut connector = HttpConnector::new();
        connector.enforce_http(false);
        let https = hyper_rustls::HttpsConnectorBuilder::new()
            .with_webpki_roots()
            .https_or_http()
            .enable_http1();
        let https = if enable_http2 {
            https.enable_http2().wrap_connector(connector)
        } else {
            https.wrap_connector(connector)
        };
        let client = Client::builder(TokioExecutor::new()).build(https);

        Self { client }
    }

    async fn request(&self, method: Method, url: &str, body: Vec<u8>) -> Result<TransportResponse> {
        // Keep request/response conversion here so the client crate's
        // Transport trait can stay small and independent of Hyper's body types.
        let request = Request::builder()
            .method(method)
            .uri(url)
            .body(Full::new(Bytes::from(body)))
            .context("build PIR HTTP request")?;
        let response = self
            .client
            .request(request)
            .await
            .context("send PIR HTTP request")?;
        let status = response.status().as_u16();
        let headers = response
            .headers()
            .iter()
            .filter_map(|(name, value)| {
                value
                    .to_str()
                    .ok()
                    .map(|value| (name.as_str().to_string(), value.to_string()))
            })
            .collect();
        let body = response
            .into_body()
            .collect()
            .await
            .context("read PIR HTTP response body")?
            .to_bytes()
            .to_vec();

        Ok(TransportResponse {
            status,
            headers,
            body,
        })
    }
}

impl Default for HyperTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for HyperTransport {
    fn get<'a>(&'a self, url: &'a str) -> TransportFuture<'a> {
        Box::pin(async move { self.request(Method::GET, url, Vec::new()).await })
    }

    fn post<'a>(&'a self, url: &'a str, body: Vec<u8>) -> TransportFuture<'a> {
        Box::pin(async move { self.request(Method::POST, url, body).await })
    }
}
