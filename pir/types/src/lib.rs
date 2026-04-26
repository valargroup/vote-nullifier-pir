//! Shared types and constants for the PIR subsystem.
//!
//! Wire types are serialized over HTTP between `pir-server` and `pir-client`.
//! Tier-layout constants define the data-format contract shared by all crates
//! (export, server, client, test).
//!
//! The default feature set is lightweight (only `serde`). Enable the `reader`
//! feature to get tier-data parsers ([`tier0::Tier0Data`], [`tier1::Tier1Row`],
//! [`tier2::Tier2Row`]) and Fp serialization helpers ([`fp_utils`]).

use serde::{Deserialize, Serialize};

#[cfg(feature = "reader")]
pub mod fp_utils;
#[cfg(feature = "reader")]
pub mod tier0;
#[cfg(feature = "reader")]
pub mod tier1;
#[cfg(feature = "reader")]
pub mod tier2;

// ── Tier-layout constants ────────────────────────────────────────────────────

/// Depth of the PIR Merkle tree.
///
/// With punctured-range leaves (K=2), each leaf covers two gaps, halving the
/// leaf count compared to K=1. Depth 25 supports 2^25 = 33,554,432 leaf
/// slots, enough for ~25.5M punctured ranges from ~51M nullifiers.
pub const PIR_DEPTH: usize = 25;

/// Number of layers in Tier 0 (root at depth 0 down to subtree records at depth 9).
pub const TIER0_LAYERS: usize = 9;

/// Number of layers in each Tier 1 subtree (depth 9 to depth 15).
pub const TIER1_LAYERS: usize = 6;

/// Number of layers in each Tier 2 subtree (depth 15 to depth 25).
pub const TIER2_LAYERS: usize = 10;

/// Number of Tier 1 rows (one per depth-9 subtree).
pub const TIER1_ROWS: usize = 1 << TIER0_LAYERS; // 512

/// Number of Tier 2 rows (one per depth-15 subtree).
pub const TIER2_ROWS: usize = 1 << (TIER0_LAYERS + TIER1_LAYERS); // 32,768

/// Number of leaves per Tier 1 subtree (at relative depth 6 = global depth 15).
pub const TIER1_LEAVES: usize = 1 << TIER1_LAYERS; // 64

/// Number of leaves per Tier 2 subtree (at relative depth 10 = global depth 25).
pub const TIER2_LEAVES: usize = 1 << TIER2_LAYERS; // 1,024

/// YPIR SimplePIR requires at least 2048 rows (`poly_len`). When TIER1_ROWS
/// is smaller, the YPIR database is padded with zero rows up to this minimum.
pub const YPIR_MIN_ROWS: usize = 2048;

/// Number of rows in the Tier 1 YPIR database (padded to YPIR minimum).
pub const TIER1_YPIR_ROWS: usize = if TIER1_ROWS >= YPIR_MIN_ROWS { TIER1_ROWS } else { YPIR_MIN_ROWS }; // 2,048

/// Byte size of each Tier 2 leaf record: 3 field elements for punctured range
/// `[nf_lo, nf_mid, nf_hi]`.
pub const TIER2_LEAF_BYTES: usize = 96;

/// Byte size of one Tier 1 row: 64 × 64 (leaf records only).
pub const TIER1_ROW_BYTES: usize = TIER1_LEAVES * 64; // 4,096

/// Byte size of one Tier 2 row: 1,024 × 96 (leaf records only).
pub const TIER2_ROW_BYTES: usize = TIER2_LEAVES * TIER2_LEAF_BYTES; // 98,304

/// Tier 1 item size in bits (for YPIR parameter setup).
pub const TIER1_ITEM_BITS: usize = TIER1_ROW_BYTES * 8;

/// Tier 2 item size in bits (for YPIR parameter setup).
pub const TIER2_ITEM_BITS: usize = TIER2_ROW_BYTES * 8;

// ── Metadata ─────────────────────────────────────────────────────────────────

/// Metadata written to `pir_root.json` alongside the tier files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PirMetadata {
    /// Hex-encoded depth-25 Merkle root (PIR tree root for K=2).
    pub root25: String,
    /// Hex-encoded depth-29 Merkle root (circuit-compatible).
    pub root29: String,
    /// Number of populated leaf ranges in the tree.
    pub num_ranges: usize,
    /// PIR tree depth.
    pub pir_depth: usize,
    /// Tier 0 size in bytes.
    pub tier0_bytes: usize,
    /// Number of Tier 1 rows.
    pub tier1_rows: usize,
    /// Tier 1 row size in bytes.
    pub tier1_row_bytes: usize,
    /// Number of Tier 2 rows.
    pub tier2_rows: usize,
    /// Tier 2 row size in bytes.
    pub tier2_row_bytes: usize,
    /// Block height the tree was built from (if known).
    pub height: Option<u64>,
}

// ── Wire types ───────────────────────────────────────────────────────────────

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
    pub root25: String,
    pub num_ranges: usize,
    pub pir_depth: usize,
    pub height: Option<u64>,
    /// Capability flag — `true` when the server exposes
    /// `POST /tier{1,2}/batch_query`. Defaults to `false` for backward
    /// compatibility with older servers that lack the batch route;
    /// clients should fall back to the legacy `/tier{1,2}/query` route
    /// in that case.
    #[serde(default)]
    pub supports_batch_query: bool,
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

/// Maximum permitted batch size for `/tier{1,2}/batch_query`.
///
/// Production currently only exercises K=5 but the wire format and server
/// route are K-generic up to this cap. The cap is a DoS guard: a malicious
/// client cannot force the server to allocate / compute for huge K.
pub const MAX_BATCH_K: usize = 16;

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

/// Parsed view over a single-query wire payload as produced by
/// [`serialize_ypir_query`]. Returns the `pqr` and `pub_params` u64 slices
/// converted from the on-wire LE byte stream.
///
/// Errors if the layout is malformed — caller should map to HTTP 400.
pub fn parse_ypir_query(query_bytes: &[u8]) -> Result<(Vec<u64>, Vec<u64>), &'static str> {
    if query_bytes.len() < U64_BYTES {
        return Err("query too short");
    }
    let pqr_byte_len =
        u64::from_le_bytes(query_bytes[..U64_BYTES].try_into().unwrap()) as usize;
    if !pqr_byte_len.is_multiple_of(U64_BYTES) {
        return Err("pqr_byte_len not a multiple of 8");
    }
    let payload_len = query_bytes.len() - U64_BYTES;
    if pqr_byte_len > payload_len {
        return Err("pqr_byte_len exceeds payload");
    }
    let remaining = payload_len - pqr_byte_len;
    if !remaining.is_multiple_of(U64_BYTES) {
        return Err("pp section bytes not a multiple of 8");
    }
    let pqr = query_bytes[U64_BYTES..U64_BYTES + pqr_byte_len]
        .chunks_exact(U64_BYTES)
        .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
        .collect();
    let pp = query_bytes[U64_BYTES + pqr_byte_len..]
        .chunks_exact(U64_BYTES)
        .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
        .collect();
    Ok((pqr, pp))
}

/// Serialize a YPIR SimplePIR **batch** query into the wire format expected
/// by `pir-server` at `POST /tier{1,2}/batch_query`.
///
/// All `K` `pqr` slices must have the same length (the `pqr` shape is
/// determined by the YPIR scenario, not the per-query `q.0` content).
///
/// Layout:
/// ```text
/// [8 bytes LE u64: K]
/// [8 bytes LE u64: pqr_byte_len]      (per query; identical across the batch)
/// [K * pqr_byte_len bytes: q_1 || q_2 || ... || q_K as LE u64s]
/// [8 bytes LE u64: pp_byte_len]
/// [pp_byte_len bytes: shared pack_pub_params as LE u64s]
/// ```
///
/// `pack_pub_params` is shared across the K queries because they were
/// generated under one YPIR `client_seed` (one secret `s`). This is the
/// upload bandwidth lever the batching protocol exploits.
pub fn serialize_ypir_batch_query(pqrs: &[&[u64]], pub_params: &[u64]) -> Vec<u8> {
    let k = pqrs.len();
    let pqr_u64_len = pqrs.first().map(|q| q.len()).unwrap_or(0);
    for q in pqrs {
        debug_assert_eq!(
            q.len(),
            pqr_u64_len,
            "all per-query pqr vectors must share the same length"
        );
    }
    let pqr_byte_len = pqr_u64_len * U64_BYTES;
    let pp_byte_len = pub_params.len() * U64_BYTES;
    let mut payload = Vec::with_capacity(
        3 * U64_BYTES + k * pqr_byte_len + pp_byte_len,
    );
    payload.extend_from_slice(&(k as u64).to_le_bytes());
    payload.extend_from_slice(&(pqr_byte_len as u64).to_le_bytes());
    for q in pqrs {
        for &v in q.iter() {
            payload.extend_from_slice(&v.to_le_bytes());
        }
    }
    payload.extend_from_slice(&(pp_byte_len as u64).to_le_bytes());
    for &v in pub_params {
        payload.extend_from_slice(&v.to_le_bytes());
    }
    payload
}

/// Parsed view over a batch query wire payload as produced by
/// [`serialize_ypir_batch_query`].
///
/// Returns `(pqrs, pub_params)` where `pqrs` is a `Vec<Vec<u64>>` of length K
/// (one per-query `q.0` vector) and `pub_params` is the shared
/// `pack_pub_params` blob.
///
/// Caller is responsible for enforcing `K <= MAX_BATCH_K` after parsing.
pub fn parse_ypir_batch_query(
    query_bytes: &[u8],
) -> Result<(Vec<Vec<u64>>, Vec<u64>), &'static str> {
    let mut cursor = 0usize;
    if query_bytes.len() < 2 * U64_BYTES {
        return Err("batch query too short for header");
    }
    let k = u64::from_le_bytes(query_bytes[cursor..cursor + U64_BYTES].try_into().unwrap()) as usize;
    cursor += U64_BYTES;
    if k == 0 {
        return Err("batch K must be >= 1");
    }
    let pqr_byte_len =
        u64::from_le_bytes(query_bytes[cursor..cursor + U64_BYTES].try_into().unwrap()) as usize;
    cursor += U64_BYTES;
    if !pqr_byte_len.is_multiple_of(U64_BYTES) {
        return Err("pqr_byte_len not a multiple of 8");
    }
    if pqr_byte_len == 0 {
        return Err("pqr section is empty");
    }
    let pqrs_total = k.checked_mul(pqr_byte_len).ok_or("K * pqr overflow")?;
    if cursor + pqrs_total + U64_BYTES > query_bytes.len() {
        return Err("batch query truncated in pqr section");
    }
    let mut pqrs = Vec::with_capacity(k);
    for _ in 0..k {
        let q: Vec<u64> = query_bytes[cursor..cursor + pqr_byte_len]
            .chunks_exact(U64_BYTES)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
            .collect();
        pqrs.push(q);
        cursor += pqr_byte_len;
    }
    let pp_byte_len =
        u64::from_le_bytes(query_bytes[cursor..cursor + U64_BYTES].try_into().unwrap()) as usize;
    cursor += U64_BYTES;
    if !pp_byte_len.is_multiple_of(U64_BYTES) {
        return Err("pp_byte_len not a multiple of 8");
    }
    if pp_byte_len == 0 {
        return Err("pp section is empty");
    }
    if cursor + pp_byte_len != query_bytes.len() {
        return Err("batch query trailing bytes do not match pp_byte_len");
    }
    let pp: Vec<u64> = query_bytes[cursor..cursor + pp_byte_len]
        .chunks_exact(U64_BYTES)
        .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
        .collect();
    Ok((pqrs, pp))
}

/// Serialize K SimplePIR responses into the batch response wire format.
///
/// All K responses must share the same length (deterministic for a given
/// scenario's `db_cols_simplepir()` and switching parameters).
///
/// Layout:
/// ```text
/// [8 bytes LE u64: K]
/// [8 bytes LE u64: response_bytes_per_query]
/// [K * response_bytes_per_query bytes]
/// ```
pub fn serialize_ypir_batch_response(responses: &[Vec<u8>]) -> Vec<u8> {
    let k = responses.len();
    let resp_len = responses.first().map(|r| r.len()).unwrap_or(0);
    for r in responses {
        debug_assert_eq!(
            r.len(),
            resp_len,
            "all per-query responses must share the same length"
        );
    }
    let mut out = Vec::with_capacity(2 * U64_BYTES + k * resp_len);
    out.extend_from_slice(&(k as u64).to_le_bytes());
    out.extend_from_slice(&(resp_len as u64).to_le_bytes());
    for r in responses {
        out.extend_from_slice(r);
    }
    out
}

/// Parse a batch response wire payload back into K equal-length response
/// chunks suitable for [`crate`]-external decoding.
pub fn parse_ypir_batch_response(bytes: &[u8]) -> Result<Vec<Vec<u8>>, &'static str> {
    if bytes.len() < 2 * U64_BYTES {
        return Err("batch response too short for header");
    }
    let k = u64::from_le_bytes(bytes[..U64_BYTES].try_into().unwrap()) as usize;
    let resp_len =
        u64::from_le_bytes(bytes[U64_BYTES..2 * U64_BYTES].try_into().unwrap()) as usize;
    if k == 0 {
        return Err("batch response K must be >= 1");
    }
    let total = k.checked_mul(resp_len).ok_or("K * resp_len overflow")?;
    if 2 * U64_BYTES + total != bytes.len() {
        return Err("batch response payload length mismatch");
    }
    let mut chunks = Vec::with_capacity(k);
    let mut cursor = 2 * U64_BYTES;
    for _ in 0..k {
        chunks.push(bytes[cursor..cursor + resp_len].to_vec());
        cursor += resp_len;
    }
    Ok(chunks)
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

    #[test]
    fn parse_ypir_query_round_trips_serialize() {
        let pqr = vec![1u64, 2, 3, 4];
        let pp = vec![10u64, 20, 30];
        let payload = serialize_ypir_query(&pqr, &pp);
        let (got_pqr, got_pp) = parse_ypir_query(&payload).unwrap();
        assert_eq!(got_pqr, pqr);
        assert_eq!(got_pp, pp);
    }

    #[test]
    fn parse_ypir_query_rejects_malformed() {
        assert!(parse_ypir_query(&[]).is_err());
        // Length prefix says 16 pqr bytes but payload is empty:
        assert!(parse_ypir_query(&[16u8, 0, 0, 0, 0, 0, 0, 0]).is_err());
        // Length prefix not a multiple of 8:
        assert!(parse_ypir_query(&[5u8, 0, 0, 0, 0, 0, 0, 0]).is_err());
    }

    // ── batch wire format ─────────────────────────────────────────────────

    #[test]
    fn serialize_parse_batch_query_round_trips() {
        let q1 = vec![1u64, 2, 3];
        let q2 = vec![4u64, 5, 6];
        let q3 = vec![7u64, 8, 9];
        let pp = vec![100u64, 200, 300, 400];
        let pqrs: Vec<&[u64]> = vec![&q1, &q2, &q3];
        let payload = serialize_ypir_batch_query(&pqrs, &pp);

        // Header: K, pqr_byte_len, payload bytes for K queries, pp_byte_len, pp.
        let expected_len = U64_BYTES // K
            + U64_BYTES // pqr_byte_len
            + 3 * (3 * U64_BYTES) // K * pqr_byte_len
            + U64_BYTES // pp_byte_len
            + 4 * U64_BYTES; // pp
        assert_eq!(payload.len(), expected_len);

        let (got_pqrs, got_pp) = parse_ypir_batch_query(&payload).unwrap();
        assert_eq!(got_pqrs.len(), 3);
        assert_eq!(got_pqrs[0], q1);
        assert_eq!(got_pqrs[1], q2);
        assert_eq!(got_pqrs[2], q3);
        assert_eq!(got_pp, pp);
    }

    #[test]
    fn batch_query_k1_equivalent_payload_size() {
        let q = vec![1u64, 2, 3];
        let pp = vec![10u64, 20];
        let single = serialize_ypir_query(&q, &pp);
        let batch = serialize_ypir_batch_query(&[q.as_slice()], &pp);
        // Batch is always 16 bytes longer than the single-query layout
        // (extra K prefix + extra pp_byte_len prefix - the single layout has
        // no K prefix and no separate pp_byte_len).
        assert_eq!(batch.len(), single.len() + 2 * U64_BYTES);
    }

    #[test]
    fn parse_batch_query_rejects_zero_k() {
        let mut payload = Vec::new();
        payload.extend_from_slice(&0u64.to_le_bytes());
        payload.extend_from_slice(&8u64.to_le_bytes());
        assert!(parse_ypir_batch_query(&payload).is_err());
    }

    #[test]
    fn parse_batch_query_rejects_truncated() {
        let q1 = vec![1u64, 2];
        let pp = vec![5u64];
        let pqrs: Vec<&[u64]> = vec![&q1];
        let mut payload = serialize_ypir_batch_query(&pqrs, &pp);
        payload.truncate(payload.len() - 1);
        assert!(parse_ypir_batch_query(&payload).is_err());
    }

    #[test]
    fn batch_response_round_trips() {
        let r1 = vec![1u8, 2, 3, 4];
        let r2 = vec![5u8, 6, 7, 8];
        let r3 = vec![9u8, 10, 11, 12];
        let payload = serialize_ypir_batch_response(&[r1.clone(), r2.clone(), r3.clone()]);

        let chunks = parse_ypir_batch_response(&payload).unwrap();
        assert_eq!(chunks, vec![r1, r2, r3]);
    }

    #[test]
    fn batch_response_rejects_mismatch() {
        // Length mismatch in trailer.
        let mut payload = serialize_ypir_batch_response(&[vec![1u8, 2, 3]]);
        payload.push(0);
        assert!(parse_ypir_batch_response(&payload).is_err());
    }
}
