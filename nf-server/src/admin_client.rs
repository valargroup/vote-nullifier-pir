//! Minimal HTTP client for `nf-server snapshot` against the admin listener (TCP or Unix).

use std::net::SocketAddr;

use anyhow::{Context, Result};
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::{Request, StatusCode, Uri};
use hyper_util::rt::TokioIo;

use crate::admin_listen::AdminBind;

fn localhost_uri(path: &str) -> Result<Uri> {
    let p = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    format!("http://localhost{p}")
        .parse::<Uri>()
        .with_context(|| format!("invalid request path {path:?}"))
}

/// GET `path` (must start with `/`), return status + body bytes.
pub async fn get_bytes(endpoint: &AdminBind, path: &str) -> Result<(StatusCode, Vec<u8>)> {
    match endpoint {
        AdminBind::Tcp(addr) => get_tcp(*addr, path).await,
        #[cfg(unix)]
        AdminBind::Unix(pathbuf) => get_unix(pathbuf, path).await,
        #[cfg(not(unix))]
        AdminBind::Unix(_) => anyhow::bail!("unix admin endpoints are not supported on this platform"),
    }
}

/// POST JSON to `path`, return status + body bytes.
pub async fn post_json(
    endpoint: &AdminBind,
    path: &str,
    json: &serde_json::Value,
) -> Result<(StatusCode, Vec<u8>)> {
    let body = serde_json::to_vec(json).context("serialize JSON body")?;
    match endpoint {
        AdminBind::Tcp(addr) => post_tcp(*addr, path, body).await,
        #[cfg(unix)]
        AdminBind::Unix(pathbuf) => post_unix(pathbuf, path, body).await,
        #[cfg(not(unix))]
        AdminBind::Unix(_) => anyhow::bail!("unix admin endpoints are not supported on this platform"),
    }
}

fn normalized_path(path: &str) -> String {
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

async fn get_tcp(addr: SocketAddr, path: &str) -> Result<(StatusCode, Vec<u8>)> {
    let client = reqwest::Client::builder()
        .build()
        .context("build reqwest client")?;
    let url = format!("http://{addr}{}", normalized_path(path));
    let resp = client.get(url).send().await.context("GET request")?;
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = resp.bytes().await.context("read GET body")?.to_vec();
    Ok((status, bytes))
}

async fn post_tcp(addr: SocketAddr, path: &str, body: Vec<u8>) -> Result<(StatusCode, Vec<u8>)> {
    let client = reqwest::Client::builder()
        .build()
        .context("build reqwest client")?;
    let url = format!("http://{addr}{}", normalized_path(path));
    let resp = client
        .post(url)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
        .context("POST request")?;
    let status = StatusCode::from_u16(resp.status().as_u16()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = resp.bytes().await.context("read POST body")?.to_vec();
    Ok((status, bytes))
}

#[cfg(unix)]
async fn get_unix(socket_path: &std::path::Path, path: &str) -> Result<(StatusCode, Vec<u8>)> {
    use tokio::net::UnixStream;

    let stream = UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("connect unix socket {}", socket_path.display()))?;
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake::<_, Full<Bytes>>(io)
        .await
        .context("http1 handshake (unix)")?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!(error = %e, "admin unix client connection closed");
        }
    });

    let uri = localhost_uri(path)?;
    let req = Request::builder()
        .method("GET")
        .uri(uri)
        .header(hyper::header::HOST, "localhost")
        .body(Full::from(Bytes::new()))
        .context("build GET request")?;

    let res = sender.send_request(req).await.context("send GET")?;
    let status = res.status();
    let body = res
        .into_body()
        .collect()
        .await
        .context("collect GET body")?
        .to_bytes()
        .to_vec();
    Ok((status, body))
}

#[cfg(unix)]
async fn post_unix(socket_path: &std::path::Path, path: &str, body: Vec<u8>) -> Result<(StatusCode, Vec<u8>)> {
    use tokio::net::UnixStream;

    let stream = UnixStream::connect(socket_path)
        .await
        .with_context(|| format!("connect unix socket {}", socket_path.display()))?;
    let io = TokioIo::new(stream);
    let (mut sender, conn) = hyper::client::conn::http1::handshake::<_, Full<Bytes>>(io)
        .await
        .context("http1 handshake (unix)")?;
    tokio::spawn(async move {
        if let Err(e) = conn.await {
            tracing::debug!(error = %e, "admin unix client connection closed");
        }
    });

    let uri = localhost_uri(path)?;
    let req = Request::builder()
        .method("POST")
        .uri(uri)
        .header(hyper::header::HOST, "localhost")
        .header(hyper::header::CONTENT_TYPE, "application/json")
        .body(Full::from(Bytes::from(body)))
        .context("build POST request")?;

    let res = sender.send_request(req).await.context("send POST")?;
    let status = res.status();
    let collected = res
        .into_body()
        .collect()
        .await
        .context("collect POST body")?;
    let bytes = collected.to_bytes().to_vec();
    Ok((status, bytes))
}
