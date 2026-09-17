//! Isolated functional check. Its parent enforces a hard process deadline.
//! The served root is a consistency anchor, not an independently trusted ledger root.
use anyhow::{anyhow, bail, ensure, Context, Result};
use pir_client::{PirClient, PirLayout, Transport, TransportFuture, TransportResponse};
use rand::RngCore;
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use voting_crypto_deps::pasta_curves::{
    group::ff::{FromUniformBytes, PrimeField},
    Fp,
};

struct Http {
    client: reqwest::Client,
    root: Mutex<Option<Value>>,
}
impl Http {
    async fn request(&self, url: &str, body: Option<Vec<u8>>) -> Result<TransportResponse> {
        let cap = if url.ends_with("/root") || url.ends_with("/params/tier1") {
            1024 * 1024
        } else {
            32 * 1024 * 1024
        };
        let request = match body {
            Some(b) => self.client.post(url).body(b),
            None => self.client.get(url),
        };
        let mut response = request
            .send()
            .await
            .map_err(|_| anyhow!("HTTP transport failed"))?;
        let status = response.status().as_u16();
        ensure!(
            response.content_length().unwrap_or(0) <= cap,
            "response size exceeds limit"
        );
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| anyhow!("response body read failed"))?
        {
            ensure!(
                bytes.len() + chunk.len() <= cap as usize,
                "response size exceeds limit"
            );
            bytes.extend_from_slice(&chunk);
        }
        if url.ends_with("/root") && status == 200 {
            *self.root.lock().unwrap() =
                Some(serde_json::from_slice(&bytes).context("malformed root JSON")?);
        }
        // Never pass an arbitrary error body into pir-client's errors/logs.
        if status != 200 {
            bail!("HTTP status {status}");
        }
        Ok(TransportResponse {
            status,
            headers: vec![],
            body: bytes,
        })
    }
}
impl Transport for Http {
    fn get<'a>(&'a self, url: &'a str) -> TransportFuture<'a> {
        Box::pin(self.request(url, None))
    }
    fn post<'a>(&'a self, url: &'a str, body: Vec<u8>) -> TransportFuture<'a> {
        Box::pin(self.request(url, Some(body)))
    }
}
fn identity(root: &Value) -> Value {
    json!([
        root["height"],
        root["circuit_root"],
        root["pir_root"],
        root["pir_layout"],
        root["nullifier_pool"],
        root["dataset_version"],
        root["zcash_network"]
    ])
}
fn check_proof(proof: &pir_client::ImtProofData, nf: Fp, root: &Value) -> Result<()> {
    ensure!(proof.verify(nf), "invalid_proof");
    ensure!(
        Some(hex::encode(proof.root.to_repr())).as_deref() == root["circuit_root"].as_str(),
        "invalid_proof_root"
    );
    Ok(())
}
async fn attempt(url: &str, layout: PirLayout) -> Result<Value> {
    let http = Arc::new(Http {
        client: reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("valargroup-pir-probe/0.1")
            .build()?,
        root: Mutex::new(None),
    });
    let client = PirClient::with_transport(url, layout, http.clone())
        .await
        .context("initialize_client")?;
    let before = http
        .root
        .lock()
        .unwrap()
        .clone()
        .context("missing_initial_root")?;
    ensure!(before["zcash_network"] == "main", "wrong_network");
    let mut random_bytes = [0u8; 64];
    rand::rngs::OsRng.fill_bytes(&mut random_bytes);
    let nf = Fp::from_uniform_bytes(&random_bytes);
    let proof_result = client.fetch_proof(nf).await;
    http.get(&format!("{}/root", url.trim_end_matches('/')))
        .await
        .context("root_recheck")?;
    let after = http
        .root
        .lock()
        .unwrap()
        .clone()
        .context("missing_final_root")?;
    ensure!(identity(&before) == identity(&after), "snapshot_changed");
    let proof = proof_result.context("query_failed")?;
    check_proof(&proof, nf, &before)?;
    Ok(
        json!({"ok":true,"height":before["height"],"circuit_root":before["circuit_root"],"pir_root":before["pir_root"]}),
    )
}
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let start = Instant::now();
    let result: Result<Value> = async {
        let args: Vec<String> = std::env::args().collect();
        ensure!(args.len() == 3, "expected URL and layout JSON");
        let layout: PirLayout = serde_json::from_str(&args[2])?;
        layout
            .validate_supported()
            .map_err(|_| anyhow!("invalid_config_layout"))?;
        if args[1] == "--validate-layout" {
            return Ok(json!({"ok":true}));
        }
        let url = reqwest::Url::parse(&args[1])?;
        ensure!(
            url.scheme() == "https" && url.username().is_empty() && url.password().is_none(),
            "HTTPS URL required"
        );
        match attempt(&args[1], layout).await {
            Err(e) if e.to_string() == "snapshot_changed" => attempt(&args[1], layout).await,
            r => r,
        }
    }
    .await;
    let (mut output, code) = match result {
        Ok(v) => (v, 0),
        Err(e) => {
            let chain = format!("{e:#}");
            let kind = if chain.contains("invalid_proof") {
                "invalid_proof"
            } else if chain.contains("snapshot_changed") {
                "unstable_snapshot"
            } else if chain.contains("mismatch")
                || chain.contains("wrong_network")
                || chain.contains("malformed")
                || chain.contains("parse")
                || chain.contains("unsupported")
                || chain.contains("response size exceeds")
            {
                "inconsistent_response"
            } else {
                "query_failed"
            };
            // Library errors may contain response-derived data. Emit only fixed classifications.
            (
                json!({"ok":false,"error":kind,"stage":e.to_string().split(':').next().unwrap_or("query_failed").chars().take(48).collect::<String>()}),
                1,
            )
        }
    };
    output["duration_ms"] = json!(start.elapsed().as_millis());
    println!("{output}");
    std::process::exit(code);
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshot_identity_includes_layout_and_network() {
        let a = json!({"height":10,"circuit_root":"a","pir_root":"b","zcash_network":"main"});
        let mut b = a.clone();
        b["zcash_network"] = json!("test");
        assert_ne!(identity(&a), identity(&b));
        b = a.clone();
        b["pir_layout"] = json!({"pir_depth":19});
        assert_ne!(identity(&a), identity(&b));
    }
    #[test]
    fn valid_proof_is_accepted_and_mutations_are_rejected() {
        let ranges = pir_export::build_ranges_with_sentinels(&[Fp::from(100u64), Fp::from(200u64)]);
        let tree = pir_export::build_pir_tree(ranges.clone()).unwrap();
        let layout = pir_types::COMPILED_PIR_LAYOUT;
        let (t0, t1) = pir_export::export_for_layout(&tree, layout).unwrap();
        let nf = Fp::from(1u64);
        let proof = pir_client::fetch_proof_local(
            &t0,
            &t1,
            ranges.len(),
            nf,
            &tree.empty_hashes,
            tree.circuit_root,
        )
        .unwrap();
        let root = json!({"circuit_root": hex::encode(proof.root.to_repr())});
        assert!(check_proof(&proof, nf, &root).is_ok());
        assert!(check_proof(&proof, nf, &json!({"circuit_root":"00"})).is_err());
        let mut corrupted = proof.clone();
        corrupted.path[0] += Fp::from(1u64);
        assert!(check_proof(&corrupted, nf, &root).is_err());
        assert!(check_proof(&proof, proof.nf_bounds[1], &root).is_err());
    }
}
