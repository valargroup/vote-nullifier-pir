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

/// Maximum PIR depth supported by the depth-29 circuit.
pub const MAX_PIR_DEPTH: usize = 29;

/// Maximum public-tier depth supported by client resource limits.
pub const MAX_TIER0_LAYERS: usize = 16;

/// Maximum privately queried subtree depth supported by YPIR resource limits.
pub const MAX_TIER1_LAYERS: usize = 15;

/// Number of layers in Tier 0 (root at depth 0 down to subtree records at depth 12).
pub const TIER0_LAYERS: usize = 12;

/// Number of layers in each Tier 1 subtree (depth 12 to depth 19).
pub const TIER1_LAYERS: usize = 7;

/// Explicit PIR tree layout negotiated between configuration, server, and client.
///
/// Supported layouts satisfy the shared protocol and YPIR constraints;
/// [`COMPILED_PIR_LAYOUT`] is the production default identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PirLayout {
    /// Depth of the PIR Merkle tree.
    pub pir_depth: usize,
    /// Number of public Tier 0 tree layers.
    pub tier0_layers: usize,
    /// Number of privately queried Tier 1 tree layers.
    pub tier1_layers: usize,
}

/// Layout compiled into this version as the production default advertise/export
/// identity. Clients no longer require connect-time equality against this value.
pub const COMPILED_PIR_LAYOUT: PirLayout = PirLayout {
    pir_depth: PIR_DEPTH,
    tier0_layers: TIER0_LAYERS,
    tier1_layers: TIER1_LAYERS,
};

impl PirLayout {
    /// Ensure `pir_depth == tier0_layers + tier1_layers` with non-zero tiers.
    pub fn validate_split(self) -> Result<(), String> {
        if self.tier0_layers == 0 || self.tier1_layers == 0 {
            return Err(format!("PIR layout tiers must be non-zero: {:?}", self));
        }
        let split = self
            .tier0_layers
            .checked_add(self.tier1_layers)
            .ok_or_else(|| "PIR layout tier split overflows".to_owned())?;
        if self.pir_depth != split {
            return Err(format!(
                "PIR layout is inconsistent: pir_depth {} != tier0_layers {} + tier1_layers {}",
                self.pir_depth, self.tier0_layers, self.tier1_layers
            ));
        }
        Ok(())
    }

    /// Number of Tier 1 rows (`2^tier0_layers`).
    pub fn tier1_rows(self) -> Result<usize, String> {
        self.validate_split()?;
        let shift = u32::try_from(self.tier0_layers)
            .map_err(|_| "PIR layout tier0_layers does not fit a shift count".to_owned())?;
        1usize
            .checked_shl(shift)
            .ok_or_else(|| "PIR layout tier0_layers exceeds platform geometry".to_owned())
    }

    /// Leaves per Tier 1 row (`2^tier1_layers`).
    pub fn tier1_leaves(self) -> Result<usize, String> {
        self.validate_split()?;
        let shift = u32::try_from(self.tier1_layers)
            .map_err(|_| "PIR layout tier1_layers does not fit a shift count".to_owned())?;
        1usize
            .checked_shl(shift)
            .ok_or_else(|| "PIR layout tier1_layers exceeds platform geometry".to_owned())
    }

    /// Byte width of one Tier 1 row.
    pub fn tier1_row_bytes(self) -> Result<usize, String> {
        self.tier1_leaves()?
            .checked_mul(TIER1_LEAF_BYTES)
            .ok_or_else(|| "PIR layout Tier 1 row width overflows".to_owned())
    }

    /// YPIR item size in bits for one Tier 1 row.
    pub fn tier1_item_bits(self) -> Result<usize, String> {
        self.tier1_row_bytes()?
            .checked_mul(8)
            .ok_or_else(|| "PIR layout Tier 1 item bits overflow".to_owned())
    }

    /// Number of BFS internal nodes stored in Tier 0 (`2^tier0_layers - 1`).
    pub fn tier0_internal_nodes(self) -> Result<usize, String> {
        let rows = self.tier1_rows()?;
        rows.checked_sub(1)
            .ok_or_else(|| "PIR layout tier0_layers must be >= 1".to_owned())
    }

    /// Total Tier 0 blob size in bytes.
    pub fn tier0_bytes(self) -> Result<usize, String> {
        let internal = self.tier0_internal_nodes()?;
        let rows = self.tier1_rows()?;
        internal
            .checked_mul(32)
            .and_then(|n| n.checked_add(rows.checked_mul(64)?))
            .ok_or_else(|| "PIR layout Tier 0 size overflows".to_owned())
    }

    /// Reject layouts below YPIR SimplePIR minima.
    pub fn validate_ypir_bounds(self) -> Result<(), String> {
        let rows = self.tier1_rows()?;
        let bits = self.tier1_item_bits()?;
        if rows < YPIR_MIN_ROWS {
            return Err(format!(
                "PIR layout Tier 1 rows {rows} below YPIR minimum {YPIR_MIN_ROWS}"
            ));
        }
        if bits < YPIR_MIN_ITEM_BITS {
            return Err(format!(
                "PIR layout Tier 1 item bits {bits} below YPIR minimum {YPIR_MIN_ITEM_BITS}"
            ));
        }
        Ok(())
    }

    /// Validate that this layout is supported by every component in this
    /// protocol release.
    pub fn validate_supported(self) -> Result<(), String> {
        self.validate_split()?;
        if !(1..=MAX_PIR_DEPTH).contains(&self.pir_depth) {
            return Err(format!(
                "unsupported PIR layout depth {}; expected 1..={MAX_PIR_DEPTH}",
                self.pir_depth,
            ));
        }
        if self.tier0_layers > MAX_TIER0_LAYERS {
            return Err(format!(
                "PIR layout Tier 0 layers {} exceeds maximum {MAX_TIER0_LAYERS}",
                self.tier0_layers
            ));
        }
        if self.tier1_layers > MAX_TIER1_LAYERS {
            return Err(format!(
                "PIR layout Tier 1 layers {} exceeds maximum {MAX_TIER1_LAYERS}",
                self.tier1_layers
            ));
        }
        self.validate_ypir_bounds()
    }
}

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
    #[serde(alias = "root25")]
    pub pir_root: String,
    /// Hex-encoded depth-29 Merkle root consumed by the circuit.
    #[serde(alias = "root29")]
    pub circuit_root: String,
    /// Number of populated leaf ranges in the tree.
    pub num_ranges: usize,
    /// PIR tree depth.
    pub pir_depth: usize,
    /// Explicit two-tier split used when this snapshot was exported.
    ///
    /// Required on the wire; snapshots without `pir_layout` fail to deserialize.
    /// Loaders verify on-disk blob sizes against this layout.
    pub pir_layout: PirLayout,
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
    /// RLWE polynomial degree selected by the server.
    #[serde(default = "default_ypir_poly_len")]
    pub poly_len: usize,
}

pub const DEFAULT_YPIR_POLY_LEN: usize = 2048;

fn default_ypir_poly_len() -> usize {
    DEFAULT_YPIR_POLY_LEN
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
    /// Hex-encoded depth-29 Merkle root consumed by the circuit.
    #[serde(alias = "root29")]
    pub circuit_root: String,
    /// Hex-encoded root at the advertised PIR depth.
    #[serde(alias = "root25")]
    pub pir_root: String,
    pub num_ranges: usize,
    /// Explicit depth and tier split advertised by the serving snapshot.
    pub pir_layout: PirLayout,
    /// Legacy top-level depth retained as an independent consistency check.
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
    fn ypir_scenario_defaults_legacy_responses_to_degree_2048() {
        let scenario: YpirScenario =
            serde_json::from_str(r#"{"num_items":4096,"item_size_bits":98304}"#).unwrap();
        assert_eq!(scenario.poly_len, DEFAULT_YPIR_POLY_LEN);
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
    fn layout_geometry_matches_compiled_constants() {
        assert_eq!(COMPILED_PIR_LAYOUT.tier1_rows().unwrap(), TIER1_ROWS);
        assert_eq!(COMPILED_PIR_LAYOUT.tier1_leaves().unwrap(), TIER1_LEAVES);
        assert_eq!(
            COMPILED_PIR_LAYOUT.tier1_row_bytes().unwrap(),
            TIER1_ROW_BYTES
        );
        assert_eq!(
            COMPILED_PIR_LAYOUT.tier1_item_bits().unwrap(),
            TIER1_ITEM_BITS
        );
        assert_eq!(
            COMPILED_PIR_LAYOUT.tier0_bytes().unwrap(),
            ((1usize << TIER0_LAYERS) - 1) * 32 + TIER1_ROWS * 64
        );
        COMPILED_PIR_LAYOUT.validate_ypir_bounds().unwrap();
    }

    #[test]
    fn alt_splits_pass_ypir_bounds() {
        for (tier0_layers, tier1_layers) in [(11, 8), (12, 7), (13, 6)] {
            let layout = PirLayout {
                pir_depth: PIR_DEPTH,
                tier0_layers,
                tier1_layers,
            };
            layout.validate_supported().unwrap();
        }
    }

    #[test]
    fn unsupported_layouts_fail_shared_validation() {
        let unsupported_split = PirLayout {
            pir_depth: PIR_DEPTH,
            tier0_layers: 10,
            tier1_layers: 9,
        };
        assert!(unsupported_split
            .validate_supported()
            .unwrap_err()
            .contains("below YPIR minimum"));

        let unsupported_depth = PirLayout {
            pir_depth: 30,
            tier0_layers: 19,
            tier1_layers: 11,
        };
        assert!(unsupported_depth
            .validate_supported()
            .unwrap_err()
            .contains("expected 1..=29"));
    }

    #[test]
    fn metadata_requires_pir_layout() {
        let missing = r#"{
            "zcash_network":"main",
            "nullifier_pool":"ironwood",
            "dataset_version":2,
            "pir_root":"00",
            "circuit_root":"00",
            "num_ranges":0,
            "pir_depth":19,
            "tier0_bytes":0,
            "tier1_rows":4096,
            "tier1_row_bytes":12288,
            "height":null
        }"#;
        assert!(serde_json::from_str::<PirMetadata>(missing).is_err());

        let present = r#"{
            "zcash_network":"main",
            "nullifier_pool":"ironwood",
            "dataset_version":2,
            "pir_root":"00",
            "circuit_root":"00",
            "num_ranges":0,
            "pir_depth":19,
            "pir_layout":{"pir_depth":19,"tier0_layers":12,"tier1_layers":7},
            "tier0_bytes":0,
            "tier1_rows":4096,
            "tier1_row_bytes":12288,
            "height":null
        }"#;
        let meta: PirMetadata = serde_json::from_str(present).unwrap();
        assert_eq!(meta.pir_layout, COMPILED_PIR_LAYOUT);
    }
}
