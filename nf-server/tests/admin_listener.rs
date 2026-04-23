//! Black-box checks: `POST /snapshot/prepare` is not on the public listener and the admin
//! Unix socket serves `GET /snapshot/status`.

use std::process::{Command, Stdio};
use std::time::Duration;

#[cfg(all(unix, feature = "serve"))]
#[tokio::test]
async fn public_listener_does_not_expose_snapshot_prepare() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sock = dir.path().join("admin.sock");

    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral tcp");
    let port = tcp.local_addr().expect("local_addr").port();
    drop(tcp);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let endpoint = format!("unix:///{}", sock.display());
    let mut child = Command::new(env!("CARGO_BIN_EXE_nf-server"))
        .args([
            "serve",
            "--pir-data-dir",
            dir.path().to_str().expect("utf8 tempdir"),
            "--port",
            &port.to_string(),
            "--voting-config-url",
            "",
            "--precomputed-base-url",
            "",
            "--admin-listen",
            &endpoint,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn nf-server serve");

    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        sock.exists(),
        "admin unix socket was not created at {}",
        sock.display()
    );

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("reqwest client");
    let public_prepare = format!("http://127.0.0.1:{port}/snapshot/prepare");
    let r = client
        .post(&public_prepare)
        .json(&serde_json::json!({ "height": 1_687_104u64 }))
        .send()
        .await
        .expect("public POST /snapshot/prepare");
    assert_eq!(
        r.status(),
        reqwest::StatusCode::NOT_FOUND,
        "public listener must not expose /snapshot/prepare"
    );

    let out = Command::new(env!("CARGO_BIN_EXE_nf-server"))
        .args(["snapshot", "status", "--endpoint", &endpoint])
        .output()
        .expect("nf-server snapshot status");
    assert!(
        out.status.success(),
        "snapshot status failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).expect("snapshot status JSON");
    assert!(
        v.get("phase").is_some(),
        "expected phase in status JSON: {}",
        String::from_utf8_lossy(&out.stdout)
    );

    let _ = child.kill();
    let _ = child.wait();
}
