use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use axum::{body::to_bytes, http::StatusCode};
use pir_server::{dispatch_query, OwnedTierState, YpirScenario};
use pir_types::serialize_ypir_query;
use ypir::{client::YPIRClient, params::YPIRSPConfig};

#[tokio::test]
async fn malformed_queries_return_400_then_valid_query_succeeds() {
    let scenario = YpirScenario {
        num_items: 2048,
        item_size_bits: 28_672,
        poly_len: 2048,
    };
    let row_bytes = scenario.item_size_bits / 8;
    let data: Vec<u8> = (0..scenario.num_items * row_bytes)
        .map(|i| ((i / row_bytes + i) % 251) as u8)
        .collect();
    let tier = OwnedTierState::new(&data, scenario.clone());
    let client = YPIRClient::from_db_sz_simplepir_with_config(
        scenario.num_items as u64,
        scenario.item_size_bits as u64,
        YPIRSPConfig::for_poly_len(scenario.poly_len),
    );
    let ((query, parameters), seed) = client.generate_query_simplepir(3);
    let valid = serialize_ypir_query(query.as_slice(), parameters.as_slice());
    let mut extra_parameters = valid.clone();
    extra_parameters.extend_from_slice(&0u64.to_le_bytes());
    let malformed = [
        serialize_ypir_query(&[0], &[0]),
        serialize_ypir_query(query.as_slice(), &[0]),
        serialize_ypir_query(&query.as_slice()[..query.len() - 1], parameters.as_slice()),
        valid[..valid.len() - 8].to_vec(),
        extra_parameters,
    ];
    let next_req_id = AtomicU64::new(0);
    let inflight = AtomicUsize::new(0);

    for body in &malformed {
        let response = dispatch_query(&tier, "tier1", body, &next_req_id, &inflight);
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(inflight.load(Ordering::Relaxed), 0);
    }

    let response = dispatch_query(&tier, "tier1", &valid, &next_req_id, &inflight);
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(inflight.load(Ordering::Relaxed), 0);
    assert_eq!(
        next_req_id.load(Ordering::Relaxed),
        malformed.len() as u64 + 1
    );
    let response_bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let decoded = client.decode_response_simplepir(seed, &response_bytes);
    assert_eq!(decoded, data[3 * row_bytes..4 * row_bytes]);
}
