//! Shared types and constants for the PIR subsystem.
//!
//! Wire types are serialized over HTTP between `pir-server` and `pir-client`.
//! Tier-layout constants define the data-format contract shared by all crates
//! (export, server, client, test).
//!
//! The default feature set is lightweight (only `serde`). Enable the `reader`
//! feature to get tier-data parsers ([`tier0::Tier0Data`], [`tier1::Tier1Row`])
//! and Fp serialization helpers ([`fp_utils`]).

use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};

#[cfg(feature = "reader")]
pub mod fp_utils;
#[cfg(feature = "reader")]
pub mod tier0;
#[cfg(feature = "reader")]
pub mod tier1;

// ── Tier-layout constants ────────────────────────────────────────────────────

/// Depth of the PIR Merkle tree.
///
/// With punctured-range leaves (K=2), each leaf covers two gaps, halving the
/// leaf count compared to K=1. Depth 19 supports 2^19 = 524,288 leaf slots.
pub const PIR_DEPTH: usize = 19;

/// Number of layers in Tier 0 (root at depth 0 down to subtree records at depth 12).
pub const TIER0_LAYERS: usize = 12;

/// Number of layers in each Tier 1 subtree (depth 12 to depth 19).
pub const TIER1_LAYERS: usize = 7;

/// Number of Tier 1 rows (one per depth-12 subtree).
pub const TIER1_ROWS: usize = 1 << TIER0_LAYERS; // 4,096

/// Number of leaves per Tier 1 subtree (at relative depth 7 = global depth 19).
pub const TIER1_LEAVES: usize = 1 << TIER1_LAYERS; // 128

/// YPIR SimplePIR requires at least 2048 rows (`poly_len`). When TIER1_ROWS
/// is smaller, the layout is invalid.
pub const YPIR_MIN_ROWS: usize = 2048;

/// YPIR's minimum supported item size.
pub const YPIR_MIN_ITEM_BITS: usize = 28_672;

/// Byte size of each Tier 1 leaf record: 3 field elements for punctured range
/// `[nf_lo, nf_mid, nf_hi]`.
pub const TIER1_LEAF_BYTES: usize = 96;

/// Byte size of one Tier 1 row: 128 × 96 (leaf records only).
pub const TIER1_ROW_BYTES: usize = TIER1_LEAVES * TIER1_LEAF_BYTES; // 12,288

/// Tier 1 item size in bits (for YPIR parameter setup).
pub const TIER1_ITEM_BITS: usize = TIER1_ROW_BYTES * 8;

const _: () = assert!(PIR_DEPTH == TIER0_LAYERS + TIER1_LAYERS);
const _: () = assert!(TIER1_ROWS >= YPIR_MIN_ROWS);
const _: () = assert!(TIER1_ITEM_BITS >= YPIR_MIN_ITEM_BITS);

// ── Metadata ─────────────────────────────────────────────────────────────────

/// Shielded pool represented by the current PIR dataset.
pub const NULLIFIER_POOL: &str = "ironwood";

/// Version of the nullifier dataset contract.
pub const DATASET_VERSION: u32 = 2;

/// Zcash network represented by a PIR dataset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZcashNetwork {
    /// Zcash Mainnet.
    Main,
    /// Zcash public Testnet.
    Test,
}

impl ZcashNetwork {
    /// Return the canonical wire value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Test => "test",
        }
    }
}

impl fmt::Display for ZcashNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ZcashNetwork {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "main" => Ok(Self::Main),
            "test" => Ok(Self::Test),
            _ => Err(format!(
                "unsupported Zcash network {value:?}; expected main or test"
            )),
        }
    }
}

/// Returns whether a pool and version identify the current PIR dataset.
pub fn is_current_dataset(nullifier_pool: &str, dataset_version: u32) -> bool {
    nullifier_pool == NULLIFIER_POOL && dataset_version == DATASET_VERSION
}

/// Metadata written to `pir_root.json` alongside the tier files.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PirMetadata {
    /// Zcash network whose nullifiers populate this dataset.
    pub zcash_network: ZcashNetwork,
    /// Shielded pool whose nullifiers populate this dataset.
    pub nullifier_pool: String,
    /// Version of the nullifier dataset contract.
    pub dataset_version: u32,
    /// Hex-encoded PIR-depth Merkle root (PIR tree root for K=2).
    ///
    /// The `root25` field name is retained as a legacy wire/API name; in
    /// dataset version 2 it contains the depth-19 root.
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
    /// Zcash network whose nullifiers populate this dataset.
    pub zcash_network: ZcashNetwork,
    /// Shielded pool whose nullifiers populate this dataset.
    pub nullifier_pool: String,
    /// Version of the nullifier dataset contract.
    pub dataset_version: u32,
    pub root29: String,
    /// Legacy wire name for the PIR-depth root; depth 19 in dataset version 2.
    pub root25: String,
    pub num_ranges: usize,
    pub pir_depth: usize,
    /// Number of rows in the Tier 1 PIR database.
    #[serde(default)]
    pub tier1_rows: usize,
    /// Tier 1 PIR row size in bytes.
    #[serde(default)]
    pub tier1_row_bytes: usize,
    pub height: Option<u64>,
}

/// Health check response returned by `GET /health`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthInfo {
    pub status: String,
    pub tier1_rows: usize,
    pub tier1_row_bytes: usize,
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
    fn zcash_network_wire_values() {
        assert_eq!(
            serde_json::to_string(&ZcashNetwork::Main).unwrap(),
            "\"main\""
        );
        assert_eq!(
            serde_json::to_string(&ZcashNetwork::Test).unwrap(),
            "\"test\""
        );
        assert_eq!("main".parse(), Ok(ZcashNetwork::Main));
        assert_eq!("test".parse(), Ok(ZcashNetwork::Test));
        assert!("regtest".parse::<ZcashNetwork>().is_err());
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
