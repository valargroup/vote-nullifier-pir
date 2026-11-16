//! Exercise the production HTTP router without allocating a PIR database.
//! A not-yet-serving handler returns 503 after body extraction; an oversized
//! request is rejected with 413 before the handler. Query cryptography and
//! section dimensions are tested separately by pir-server's query tests.

#![cfg(feature = "serve")]

use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct ServerProcess(Child);

impl Drop for ServerProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn expanded_query_envelope_passes_the_production_http_body_limit() {
    let directory = tempfile::tempdir().unwrap();
    let socket = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = socket.local_addr().unwrap().port();
    drop(socket);
    let mut server = ServerProcess(
        Command::new(env!("CARGO_BIN_EXE_nf-server"))
            .env_clear()
            .current_dir(directory.path())
            .args([
                "serve",
                "--zcash-network",
                "main",
                "--port",
                &port.to_string(),
                "--pir-data-dir",
                directory.path().to_str().unwrap(),
                "--pir-config-url",
                "",
                "--voting-config-url",
                "",
                "--precomputed-base-url",
                "",
                "--stale-threshold-secs",
                "0",
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if client.get(format!("{base}/ready")).send().await.is_ok() {
            break;
        }
        assert!(
            server.0.try_wait().unwrap().is_none(),
            "server exited before opening HTTP"
        );
        assert!(
            Instant::now() < deadline,
            "server did not open HTTP before the deadline"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    // The generated-query test verifies these dimensions for 12+8 / degree
    // 4096. Zero contents are sufficient to test HTTP extraction here.
    let query = pir_types::serialize_ypir_query(&vec![0; 4096], &vec![0; 196_608]);
    assert_eq!(query.len(), 1_605_640);
    let response = client
        .post(format!("{base}/tier1/query"))
        .body(query)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), reqwest::StatusCode::SERVICE_UNAVAILABLE);
    assert!(response
        .json::<serde_json::Value>()
        .await
        .unwrap()
        .get("phase")
        .is_some());

    // Preserve the deployed limit, including its inclusive boundary.
    assert_eq!(pir_server::MAX_QUERY_BODY_BYTES, 2_097_152);
    for (size, expected) in [
        (2_097_152, reqwest::StatusCode::SERVICE_UNAVAILABLE),
        (2_097_153, reqwest::StatusCode::PAYLOAD_TOO_LARGE),
    ] {
        let response = client
            .post(format!("{base}/tier1/query"))
            .body(vec![0; size])
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
    }
}
