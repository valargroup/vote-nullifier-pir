//! Shared types for the PIR subsystem.
//!
//! These types are serialized over HTTP between `pir-server` and `pir-client`,
//! so they live in a separate lightweight crate to avoid coupling the client
//! to the server's heavy YPIR dependencies (which require nightly).

use serde::{Deserialize, Serialize};

/// Parameters describing a YPIR database scenario.
///
/// Serialized as JSON over HTTP so the client can reconstruct matching
/// YPIR parameters locally without knowing the tier layout constants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YpirScenario {
    pub num_items: usize,
    pub item_size_bits: usize,
}

/// Root hash and metadata returned by `GET /root`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootInfo {
    pub root29: String,
    pub root26: String,
    pub num_ranges: usize,
    pub pir_depth: usize,
    pub height: Option<u64>,
}

/// Health check response returned by `GET /health`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthInfo {
    pub status: String,
    pub tier1_rows: usize,
    pub tier2_rows: usize,
    pub tier1_row_bytes: usize,
    pub tier2_row_bytes: usize,
}

const U64_BYTES: usize = std::mem::size_of::<u64>();

/// Serialize a YPIR SimplePIR query into the wire format expected by `pir-server`.
///
/// Layout: `[8-byte LE pqr_byte_len][pqr as LE u64s][pub_params as LE u64s]`
pub fn serialize_ypir_query(pqr: &[u64], pub_params: &[u64]) -> Vec<u8> {
    let pqr_byte_len = pqr.len() * U64_BYTES;
    let mut payload = Vec::with_capacity(U64_BYTES + (pqr.len() + pub_params.len()) * U64_BYTES);
    payload.extend_from_slice(&(pqr_byte_len as u64).to_le_bytes());
    for &v in pqr {
        payload.extend_from_slice(&v.to_le_bytes());
    }
    for &v in pub_params {
        payload.extend_from_slice(&v.to_le_bytes());
    }
    payload
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serialize_ypir_query_empty() {
        let result = serialize_ypir_query(&[], &[]);
        assert_eq!(result.len(), U64_BYTES);
        assert_eq!(u64::from_le_bytes(result[..8].try_into().unwrap()), 0);
    }

    #[test]
    fn serialize_ypir_query_round_trip_layout() {
        let pqr = vec![1u64, 2, 3];
        let pp = vec![100u64, 200];
        let payload = serialize_ypir_query(&pqr, &pp);

        let expected_len = U64_BYTES + (pqr.len() + pp.len()) * U64_BYTES;
        assert_eq!(payload.len(), expected_len);

        let pqr_byte_len = u64::from_le_bytes(payload[..8].try_into().unwrap()) as usize;
        assert_eq!(pqr_byte_len, pqr.len() * U64_BYTES);

        for (i, &expected) in pqr.iter().enumerate() {
            let offset = U64_BYTES + i * U64_BYTES;
            let val = u64::from_le_bytes(payload[offset..offset + U64_BYTES].try_into().unwrap());
            assert_eq!(val, expected);
        }

        for (i, &expected) in pp.iter().enumerate() {
            let offset = U64_BYTES + pqr_byte_len + i * U64_BYTES;
            let val = u64::from_le_bytes(payload[offset..offset + U64_BYTES].try_into().unwrap());
            assert_eq!(val, expected);
        }
    }

    #[test]
    fn serialize_ypir_query_length_prefix_correctness() {
        let pqr = vec![42u64];
        let pp = vec![99u64];
        let payload = serialize_ypir_query(&pqr, &pp);

        let pqr_byte_len = u64::from_le_bytes(payload[..8].try_into().unwrap()) as usize;
        assert_eq!(pqr_byte_len, 8);

        let remaining = payload.len() - U64_BYTES - pqr_byte_len;
        assert_eq!(remaining, pp.len() * U64_BYTES);
    }
}
