//! End-to-end tests for the snapshot self-bootstrap.
//!
//! Stands up a real HTTP server (axum on a random port) that mimics
//! what the publisher CI uploads to the bucket, then drives
//! `bootstrap::run` against it and asserts on:
//!   * full success (manifest + tier files installed),
//!   * sha256 mismatch detection (file removed, fall-through outcome),
//!   * wrong-height manifest detection,
//!   * skip when local height already matches voting-config,
//!   * skip when voting-config has no `snapshot_height`,
//!   * fall-through when voting-config URL is unreachable.
//!
//! All tests run with `serve` enabled because that's the only build
//! configuration the bootstrap module is compiled in.

#![cfg(feature = "serve")]

use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::State;
use axum::http::{header, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use tokio::sync::oneshot;

// Re-export the modules we need to test. Integration tests can only
// reach the binary's modules when they are also compiled as part of
// `lib.rs` — but `nf-server` is a binary crate, so we declare these
// modules inline using `#[path]` to keep one source of truth.
// `dead_code` is expected: the integration test only exercises the
// bootstrap surface, never the `/metrics` HTTP handler or the URL
// default constants (those are exercised by `cmd_serve.rs` flag
// defaults, not by tests here).
#[path = "../src/bootstrap.rs"]
#[allow(dead_code)]
mod bootstrap;
#[path = "../src/metrics.rs"]
#[allow(dead_code)]
mod metrics;
#[path = "../src/voting_config.rs"]
#[allow(dead_code)]
mod voting_config;

use bootstrap::{Config, Outcome};

/// Per-route response: content-type plus body. Aliased so clippy's
/// `type_complexity` lint stays quiet on the `MockBucket` struct.
type RouteTable = BTreeMap<String, (String, Vec<u8>)>;

/// Minimal in-memory mirror of what the bucket serves.
#[derive(Clone, Default)]
struct MockBucket {
    /// `path → (content_type, body)` for arbitrary GETs.
    routes: Arc<std::sync::RwLock<RouteTable>>,
}

impl MockBucket {
    fn put(&self, path: &str, content_type: &str, body: Vec<u8>) {
        self.routes
            .write()
            .unwrap()
            .insert(path.to_string(), (content_type.to_string(), body));
    }
}

async fn handle_get(State(bucket): State<MockBucket>, uri: axum::http::Uri) -> impl IntoResponse {
    let path = uri.path().to_string();
    match bucket.routes.read().unwrap().get(&path).cloned() {
        Some((ct, body)) => (StatusCode::OK, [(header::CONTENT_TYPE, ct)], body).into_response(),
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

/// Spawn an axum server on a random port. Returns `(base_url, shutdown_tx)`.
async fn spawn_mock(bucket: MockBucket) -> (String, oneshot::Sender<()>) {
    let app = Router::new().fallback(get(handle_get)).with_state(bucket);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    let (tx, rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = rx.await;
            })
            .await;
    });
    // Tiny delay to make sure the listener is accepting before tests
    // race ahead. 5ms is generous on localhost.
    tokio::time::sleep(Duration::from_millis(5)).await;
    (format!("http://{addr}"), tx)
}

fn sha256_hex(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

/// Stage a published snapshot in the mock bucket at the canonical
/// `<base>/snapshots/<height>/...` paths. Returns the byte payloads
/// keyed by file name so tests can assert against installed contents.
fn stage_snapshot(bucket: &MockBucket, height: u64) -> BTreeMap<String, Vec<u8>> {
    let mut blobs = BTreeMap::new();
    blobs.insert("tier0.bin".to_string(), b"tier0-payload".to_vec());
    blobs.insert("tier1.bin".to_string(), b"tier1-payload".to_vec());
    blobs.insert("tier2.bin".to_string(), b"tier2-payload".to_vec());
    blobs.insert(
        "pir_root.json".to_string(),
        serde_json::to_vec(&json!({
            "root25": "00",
            "root29": "00",
            "num_ranges": 1,
            "pir_depth": 1,
            "tier0_bytes": 0,
            "tier1_rows": 0,
            "tier1_row_bytes": 0,
            "tier2_rows": 0,
            "tier2_row_bytes": 0,
            "height": height,
        }))
        .unwrap(),
    );

    let mut files_json = serde_json::Map::new();
    for (name, body) in &blobs {
        files_json.insert(
            name.clone(),
            json!({ "size": body.len() as u64, "sha256": sha256_hex(body) }),
        );
    }
    let manifest = json!({
        "schema_version": 1,
        "height": height,
        "created_at": "2026-01-01T00:00:00Z",
        "nf_server_sha256": "deadbeef",
        "publisher": { "git_ref": "main", "git_sha": "abc" },
        "files": files_json,
    });

    let prefix = format!("/snapshots/{height}");
    for (name, body) in &blobs {
        bucket.put(
            &format!("{prefix}/{name}"),
            "application/octet-stream",
            body.clone(),
        );
    }
    bucket.put(
        &format!("{prefix}/manifest.json"),
        "application/json",
        serde_json::to_vec(&manifest).unwrap(),
    );

    blobs
}

fn stage_voting_config(bucket: &MockBucket, snapshot_height: Option<u64>) {
    let body = match snapshot_height {
        Some(h) => json!({ "snapshot_height": h }),
        None => json!({}),
    };
    bucket.put(
        "/voting-config.json",
        "application/json",
        serde_json::to_vec(&body).unwrap(),
    );
}

#[tokio::test]
async fn full_bootstrap_installs_all_files() {
    let bucket = MockBucket::default();
    let h = 100u64;
    let blobs = stage_snapshot(&bucket, h);
    stage_voting_config(&bucket, Some(h));
    let (base, _shutdown) = spawn_mock(bucket).await;

    let tmp = TempDir::new().unwrap();
    let cfg = Config {
        voting_config_url: format!("{base}/voting-config.json"),
        precomputed_base_url: base.clone(),
        pir_data_dir: tmp.path().to_path_buf(),
        http_timeout: Duration::from_secs(5),
    };

    let outcome = bootstrap::run(&cfg).await.unwrap();
    assert_eq!(outcome, Outcome::BootstrappedTo(h));

    for (name, expected) in &blobs {
        let actual = std::fs::read(tmp.path().join(name)).expect(name);
        assert_eq!(&actual, expected, "{name} contents");
    }
    assert!(
        !tmp.path().join(".bootstrap-staging").exists(),
        "staging dir should be cleaned"
    );
}

#[tokio::test]
async fn sha256_mismatch_falls_through_and_removes_partial() {
    let bucket = MockBucket::default();
    let h = 200u64;
    stage_snapshot(&bucket, h);
    stage_voting_config(&bucket, Some(h));
    // Corrupt tier1.bin: serve different bytes than the manifest hash
    // covers. The manifest still claims the original sha.
    bucket.put(
        &format!("/snapshots/{h}/tier1.bin"),
        "application/octet-stream",
        b"corrupted-payload-different-length".to_vec(),
    );
    let (base, _shutdown) = spawn_mock(bucket).await;

    let tmp = TempDir::new().unwrap();
    let cfg = Config {
        voting_config_url: format!("{base}/voting-config.json"),
        precomputed_base_url: base,
        pir_data_dir: tmp.path().to_path_buf(),
        http_timeout: Duration::from_secs(5),
    };

    let outcome = bootstrap::run(&cfg).await.unwrap();
    match outcome {
        Outcome::FellThrough { reason } => {
            assert!(
                reason.contains("CDN fetch failed"),
                "unexpected reason: {reason}"
            );
        }
        other => panic!("expected FellThrough, got {other:?}"),
    }
    // The bad tier1.bin must have been removed, and pir_root.json
    // must NOT have been moved (we abort before the rename phase).
    assert!(!tmp.path().join("pir_root.json").exists());
    let staging = tmp.path().join(".bootstrap-staging");
    if staging.exists() {
        assert!(!staging.join("tier1.bin").exists());
    }
}

#[tokio::test]
async fn missing_remote_snapshot_falls_through() {
    let bucket = MockBucket::default();
    let h = 300u64;
    stage_snapshot(&bucket, h);
    // Voting-config asks for h+10, but only h is published — the
    // bootstrap will hit a 404 on `/snapshots/{h+10}/manifest.json`.
    stage_voting_config(&bucket, Some(h + 10));
    let (base, _shutdown) = spawn_mock(bucket).await;

    let tmp = TempDir::new().unwrap();
    let cfg = Config {
        voting_config_url: format!("{base}/voting-config.json"),
        precomputed_base_url: base,
        pir_data_dir: tmp.path().to_path_buf(),
        http_timeout: Duration::from_secs(5),
    };

    let outcome = bootstrap::run(&cfg).await.unwrap();
    assert!(
        matches!(outcome, Outcome::FellThrough { .. }),
        "expected FellThrough when manifest at requested height is missing, got {outcome:?}"
    );
}

#[tokio::test]
async fn manifest_height_mismatch_falls_through() {
    // A more subtle case than `missing_remote_snapshot_falls_through`:
    // the manifest IS reachable at the requested URL but its embedded
    // `height` field disagrees with the URL. This is the
    // "publisher uploaded under the wrong prefix" failure mode that
    // the manifest-vs-URL guard in `fetch_and_install` catches before
    // we touch the local snapshot.
    let bucket = MockBucket::default();
    let h = 350u64;
    stage_snapshot(&bucket, h);
    stage_voting_config(&bucket, Some(h));

    // Overwrite the manifest at /snapshots/h/manifest.json with one
    // whose embedded height claims h+1.
    let bogus_manifest = serde_json::json!({
        "schema_version": 1,
        "height": h + 1,
        "created_at": "2026-01-01T00:00:00Z",
        "files": {
            "tier0.bin":     { "size": 1, "sha256": "00" },
            "tier1.bin":     { "size": 1, "sha256": "00" },
            "tier2.bin":     { "size": 1, "sha256": "00" },
            "pir_root.json": { "size": 1, "sha256": "00" }
        }
    });
    bucket.put(
        &format!("/snapshots/{h}/manifest.json"),
        "application/json",
        serde_json::to_vec(&bogus_manifest).unwrap(),
    );
    let (base, _shutdown) = spawn_mock(bucket).await;

    let tmp = TempDir::new().unwrap();
    let cfg = Config {
        voting_config_url: format!("{base}/voting-config.json"),
        precomputed_base_url: base,
        pir_data_dir: tmp.path().to_path_buf(),
        http_timeout: Duration::from_secs(5),
    };

    let outcome = bootstrap::run(&cfg).await.unwrap();
    match outcome {
        Outcome::FellThrough { reason } => assert!(
            reason.contains("manifest height"),
            "expected manifest-height failure, got: {reason}"
        ),
        other => panic!("expected FellThrough, got {other:?}"),
    }
    // No tier files should have been moved into pir-data.
    assert!(!tmp.path().join("tier0.bin").exists());
    assert!(!tmp.path().join("pir_root.json").exists());
}

#[tokio::test]
async fn already_at_height_is_a_no_op() {
    let bucket = MockBucket::default();
    let h = 400u64;
    stage_snapshot(&bucket, h); // available but should not be downloaded
    stage_voting_config(&bucket, Some(h));
    let (base, _shutdown) = spawn_mock(bucket).await;

    let tmp = TempDir::new().unwrap();
    // Pre-stage a local pir_root.json at the same height.
    std::fs::write(
        tmp.path().join("pir_root.json"),
        serde_json::to_vec(&json!({
            "root25": "00",
            "root29": "00",
            "num_ranges": 0,
            "pir_depth": 0,
            "tier0_bytes": 0,
            "tier1_rows": 0,
            "tier1_row_bytes": 0,
            "tier2_rows": 0,
            "tier2_row_bytes": 0,
            "height": h,
        }))
        .unwrap(),
    )
    .unwrap();

    let cfg = Config {
        voting_config_url: format!("{base}/voting-config.json"),
        precomputed_base_url: base,
        pir_data_dir: tmp.path().to_path_buf(),
        http_timeout: Duration::from_secs(5),
    };

    let outcome = bootstrap::run(&cfg).await.unwrap();
    assert_eq!(outcome, Outcome::AlreadyAtHeight(h));
    // tier files must NOT have been written.
    assert!(!tmp.path().join("tier0.bin").exists());
}

#[tokio::test]
async fn voting_config_without_height_errors() {
    let bucket = MockBucket::default();
    stage_voting_config(&bucket, None);
    let (base, _shutdown) = spawn_mock(bucket).await;

    let tmp = TempDir::new().unwrap();
    let cfg = Config {
        voting_config_url: format!("{base}/voting-config.json"),
        precomputed_base_url: base,
        pir_data_dir: tmp.path().to_path_buf(),
        http_timeout: Duration::from_secs(5),
    };

    let err = bootstrap::run(&cfg).await.err().expect("expected error");
    let s = format!("{err:#}");
    assert!(s.contains("snapshot_height"), "unexpected error: {s}");
}

#[tokio::test]
async fn unreachable_voting_config_errors() {
    let tmp = TempDir::new().unwrap();
    let cfg = Config {
        // Localhost on a port we don't bind: connection refused.
        voting_config_url: "http://127.0.0.1:1/voting-config.json".to_string(),
        precomputed_base_url: "http://127.0.0.1:1".to_string(),
        pir_data_dir: tmp.path().to_path_buf(),
        http_timeout: Duration::from_secs(1),
    };

    let err = bootstrap::run(&cfg).await.err().expect("expected error");
    let s = format!("{err:#}");
    assert!(
        s.contains("strict bootstrap") || s.contains("voting-config"),
        "unexpected error: {s}"
    );
}
