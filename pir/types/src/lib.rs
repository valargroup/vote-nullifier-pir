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
pub mod boundary_row;
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

/// Explicit PIR tree layout negotiated between configuration, server, and client.
///
/// Clients accept any valid split that satisfies geometry and YPIR bounds;
/// [`COMPILED_PIR_LAYOUT`] is only the production default identity.
///
/// Tier 0 is always the plaintext index. When `tier2_layers == 0` the single
/// encrypted tier (Tier 1) holds the punctured-range leaves. When
/// `tier2_layers > 0`, Tier 1 becomes an intermediate boundary-index tier and
/// Tier 2 holds the leaves; the tiers chain by
/// `tier2_row = tier1_row * 2^tier1_layers + child_index`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PirLayout {
    /// Depth of the PIR Merkle tree.
    pub pir_depth: usize,
    /// Number of public Tier 0 tree layers.
    pub tier0_layers: usize,
    /// Number of privately queried Tier 1 tree layers.
    pub tier1_layers: usize,
    /// Number of privately queried Tier 2 tree layers (0 = no Tier 2).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tier2_layers: usize,
}

fn is_zero(v: &usize) -> bool {
    *v == 0
}

/// Layout compiled into this version as the production default advertise/export
/// identity. Clients no longer require connect-time equality against this value.
pub const COMPILED_PIR_LAYOUT: PirLayout = PirLayout {
    pir_depth: PIR_DEPTH,
    tier0_layers: TIER0_LAYERS,
    tier1_layers: TIER1_LAYERS,
    tier2_layers: 0,
};

/// Largest supported Tier 0 layer count. Caps the plaintext Tier 0 blob every
/// client downloads (~96 B x 2^tier0_layers, ~6 MiB at 16). The lower bound
/// follows from [`YPIR_MIN_ROWS`] via [`PirLayout::validate_ypir_bounds`].
pub const TIER0_MAX_LAYERS: usize = 16;

/// Byte size of a boundary record in an intermediate encrypted tier:
/// 32-byte subtree hash + 32-byte min_key.
pub const BOUNDARY_RECORD_BYTES: usize = 64;

impl PirLayout {
    /// Ensure `pir_depth == tier0_layers + tier1_layers + tier2_layers` with
    /// non-zero Tier 0 / Tier 1 and a bounded Tier 0.
    pub fn validate_split(self) -> Result<(), String> {
        if self.tier0_layers == 0 || self.tier1_layers == 0 {
            return Err(format!("PIR layout tiers must be non-zero: {:?}", self));
        }
        if self.tier0_layers > TIER0_MAX_LAYERS {
            return Err(format!(
                "PIR layout tier0_layers {} exceeds the supported maximum {TIER0_MAX_LAYERS}",
                self.tier0_layers
            ));
        }
        let split = self
            .tier0_layers
            .checked_add(self.tier1_layers)
            .and_then(|v| v.checked_add(self.tier2_layers))
            .ok_or_else(|| "PIR layout tier split overflows".to_owned())?;
        if self.pir_depth != split {
            return Err(format!(
                "PIR layout is inconsistent: pir_depth {} != tier0_layers {} + tier1_layers {} \
                 + tier2_layers {}",
                self.pir_depth, self.tier0_layers, self.tier1_layers, self.tier2_layers
            ));
        }
        Ok(())
    }

    /// Whether this layout has a second encrypted tier.
    pub fn tier2_enabled(self) -> bool {
        self.tier2_layers > 0
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

    /// Byte width of one Tier 1 record: a punctured-range leaf when Tier 1
    /// is terminal, or a `(subtree_hash, min_key)` boundary pair when Tier 2
    /// holds the leaves.
    pub fn tier1_record_bytes(self) -> usize {
        if self.tier2_enabled() {
            BOUNDARY_RECORD_BYTES
        } else {
            TIER1_LEAF_BYTES
        }
    }

    /// Payload byte width of one Tier 1 row (records only, unpadded).
    pub fn tier1_row_bytes(self) -> Result<usize, String> {
        self.tier1_leaves()?
            .checked_mul(self.tier1_record_bytes())
            .ok_or_else(|| "PIR layout Tier 1 row width overflows".to_owned())
    }

    /// On-disk / served byte stride of one Tier 1 row.
    ///
    /// Three-tier boundary rows whose payload falls under the YPIR item-size
    /// floor are zero-padded up to it (the floor is a hard assert inside the
    /// YPIR parameter derivation). Two-tier layouts are never padded — they
    /// must clear the floor natively (see [`Self::validate_ypir_bounds`]).
    pub fn tier1_padded_row_bytes(self) -> Result<usize, String> {
        let payload = self.tier1_row_bytes()?;
        if self.tier2_enabled() {
            Ok(payload.max(YPIR_MIN_ITEM_BITS / 8))
        } else {
            Ok(payload)
        }
    }

    /// YPIR item size in bits for one Tier 1 row (padded stride).
    pub fn tier1_item_bits(self) -> Result<usize, String> {
        self.tier1_padded_row_bytes()?
            .checked_mul(8)
            .ok_or_else(|| "PIR layout Tier 1 item bits overflow".to_owned())
    }

    /// Total Tier 1 file size in bytes (`rows * padded stride`).
    pub fn tier1_file_bytes(self) -> Result<usize, String> {
        self.tier1_rows()?
            .checked_mul(self.tier1_padded_row_bytes()?)
            .ok_or_else(|| "PIR layout Tier 1 file size overflows".to_owned())
    }

    /// YPIR scenario served for Tier 1.
    pub fn tier1_scenario(self) -> Result<YpirScenario, String> {
        Ok(YpirScenario {
            num_items: self.tier1_rows()?,
            item_size_bits: self.tier1_item_bits()?,
        })
    }

    /// Number of Tier 2 rows (`2^(tier0_layers + tier1_layers)`), or `None`
    /// when Tier 2 is disabled.
    pub fn tier2_rows(self) -> Result<Option<usize>, String> {
        self.validate_split()?;
        if !self.tier2_enabled() {
            return Ok(None);
        }
        let shift = self
            .tier0_layers
            .checked_add(self.tier1_layers)
            .and_then(|v| u32::try_from(v).ok())
            .ok_or_else(|| "PIR layout tier2 row shift overflows".to_owned())?;
        1usize
            .checked_shl(shift)
            .map(Some)
            .ok_or_else(|| "PIR layout tier2 rows exceed platform geometry".to_owned())
    }

    /// Leaves per Tier 2 row (`2^tier2_layers`), or `None` when disabled.
    pub fn tier2_leaves(self) -> Result<Option<usize>, String> {
        self.validate_split()?;
        if !self.tier2_enabled() {
            return Ok(None);
        }
        let shift = u32::try_from(self.tier2_layers)
            .map_err(|_| "PIR layout tier2_layers does not fit a shift count".to_owned())?;
        1usize
            .checked_shl(shift)
            .map(Some)
            .ok_or_else(|| "PIR layout tier2_layers exceeds platform geometry".to_owned())
    }

    /// Payload byte width of one Tier 2 row, or `None` when disabled.
    pub fn tier2_row_bytes(self) -> Result<Option<usize>, String> {
        match self.tier2_leaves()? {
            None => Ok(None),
            Some(leaves) => leaves
                .checked_mul(TIER1_LEAF_BYTES)
                .map(Some)
                .ok_or_else(|| "PIR layout Tier 2 row width overflows".to_owned()),
        }
    }

    /// On-disk / served byte stride of one Tier 2 row (zero-padded up to the
    /// YPIR item-size floor), or `None` when disabled.
    pub fn tier2_padded_row_bytes(self) -> Result<Option<usize>, String> {
        Ok(self
            .tier2_row_bytes()?
            .map(|payload| payload.max(YPIR_MIN_ITEM_BITS / 8)))
    }

    /// Total Tier 2 file size in bytes, or `None` when disabled.
    pub fn tier2_file_bytes(self) -> Result<Option<usize>, String> {
        match (self.tier2_rows()?, self.tier2_padded_row_bytes()?) {
            (Some(rows), Some(stride)) => rows
                .checked_mul(stride)
                .map(Some)
                .ok_or_else(|| "PIR layout Tier 2 file size overflows".to_owned()),
            _ => Ok(None),
        }
    }

    /// YPIR scenario served for Tier 2, or `None` when disabled.
    pub fn tier2_scenario(self) -> Result<Option<YpirScenario>, String> {
        match (self.tier2_rows()?, self.tier2_padded_row_bytes()?) {
            (Some(rows), Some(stride)) => Ok(Some(YpirScenario {
                num_items: rows,
                item_size_bits: stride
                    .checked_mul(8)
                    .ok_or_else(|| "PIR layout Tier 2 item bits overflow".to_owned())?,
            })),
            _ => Ok(None),
        }
    }

    /// Path offset of the Tier 0 siblings: `path[pir_depth - tier0_layers..pir_depth]`.
    pub fn tier0_path_offset(self) -> usize {
        self.pir_depth - self.tier0_layers
    }

    /// Path offset of the Tier 1 siblings (0 when Tier 2 is disabled).
    pub fn tier1_path_offset(self) -> usize {
        self.tier2_layers
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
    ///
    /// Row counts must clear the floor for every encrypted tier. Item bits
    /// must clear the floor natively for two-tier layouts; three-tier rows
    /// are zero-padded to the floor stride instead (no split of a three-tier
    /// layout can clear the item floor natively on both encrypted tiers).
    pub fn validate_ypir_bounds(self) -> Result<(), String> {
        let rows = self.tier1_rows()?;
        if rows < YPIR_MIN_ROWS {
            return Err(format!(
                "PIR layout Tier 1 rows {rows} below YPIR minimum {YPIR_MIN_ROWS}"
            ));
        }
        if let Some(tier2_rows) = self.tier2_rows()? {
            if tier2_rows < YPIR_MIN_ROWS {
                return Err(format!(
                    "PIR layout Tier 2 rows {tier2_rows} below YPIR minimum {YPIR_MIN_ROWS}"
                ));
            }
        } else {
            let bits = self.tier1_item_bits()?;
            if bits < YPIR_MIN_ITEM_BITS {
                return Err(format!(
                    "PIR layout Tier 1 item bits {bits} below YPIR minimum {YPIR_MIN_ITEM_BITS}"
                ));
            }
        }
        Ok(())
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
    /// Explicit two-tier split used when this snapshot was exported.
    ///
    /// Required on the wire; snapshots without `pir_layout` fail to deserialize.
    /// Loaders verify on-disk blob sizes against this layout.
    pub pir_layout: PirLayout,
    /// Tier 0 size in bytes.
    pub tier0_bytes: usize,
    /// Number of Tier 1 rows.
    pub tier1_rows: usize,
    /// Tier 1 row size in bytes (padded YPIR stride).
    pub tier1_row_bytes: usize,
    /// Number of Tier 2 rows (0 when Tier 2 is disabled).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tier2_rows: usize,
    /// Tier 2 row size in bytes (padded YPIR stride; 0 when disabled).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tier2_row_bytes: usize,
    /// Block height the tree was built from (if known).
    pub height: Option<u64>,
}

// ── Wire types ───────────────────────────────────────────────────────────────

/// Parameters describing a YPIR database scenario.
///
/// Serialized as JSON over HTTP so the client can reconstruct matching
/// YPIR parameters locally without knowing the tier layout constants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Explicit depth and tier split advertised by the serving snapshot.
    pub pir_layout: PirLayout,
    /// Legacy top-level depth retained as an independent consistency check.
    pub pir_depth: usize,
    /// Number of rows in the Tier 1 PIR database.
    #[serde(default)]
    pub tier1_rows: usize,
    /// Tier 1 PIR row size in bytes (padded YPIR stride).
    #[serde(default)]
    pub tier1_row_bytes: usize,
    /// Number of rows in the Tier 2 PIR database (0 when disabled).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tier2_rows: usize,
    /// Tier 2 PIR row size in bytes (padded YPIR stride; 0 when disabled).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tier2_row_bytes: usize,
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

    fn layout(pir_depth: usize, t0: usize, t1: usize, t2: usize) -> PirLayout {
        PirLayout {
            pir_depth,
            tier0_layers: t0,
            tier1_layers: t1,
            tier2_layers: t2,
        }
    }

    #[test]
    fn validate_split_accepts_three_tier_shapes() {
        for l in [
            layout(19, 12, 4, 3),
            layout(19, 12, 1, 6),
            layout(20, 11, 5, 4),
            layout(29, 16, 7, 6),
        ] {
            l.validate_split()
                .unwrap_or_else(|e| panic!("{l:?} must validate: {e}"));
            l.validate_ypir_bounds()
                .unwrap_or_else(|e| panic!("{l:?} must pass bounds: {e}"));
        }
    }

    #[test]
    fn validate_split_rejects_unsupported_shapes() {
        let cases = [
            (layout(19, 12, 8, 0), "inconsistent"), // sum mismatch
            (layout(19, 12, 7, 1), "inconsistent"), // sum mismatch with tier2
            (layout(19, 12, 0, 7), "non-zero"),     // encrypted tier missing
            (layout(30, 17, 7, 6), "maximum"),      // tier0 above cap
            (layout(19, 12, usize::MAX, 3), "overflows"),
        ];
        for (l, needle) in cases {
            let err = l
                .validate_split()
                .expect_err(&format!("{l:?} must be rejected"));
            assert!(err.contains(needle), "{l:?}: unexpected error {err}");
        }
        // Tier 0 below the YPIR row floor is caught by the bounds check.
        let err = layout(19, 10, 9, 0).validate_ypir_bounds().unwrap_err();
        assert!(err.contains("below YPIR minimum"), "{err}");
    }

    #[test]
    fn three_tier_geometry_pads_sub_floor_rows() {
        let l = layout(19, 12, 4, 3);
        assert!(l.tier2_enabled());
        assert_eq!(l.tier1_record_bytes(), BOUNDARY_RECORD_BYTES);
        assert_eq!(l.tier1_rows().unwrap(), 4096);
        assert_eq!(l.tier1_leaves().unwrap(), 16);
        assert_eq!(l.tier1_row_bytes().unwrap(), 1024);
        assert_eq!(l.tier1_padded_row_bytes().unwrap(), YPIR_MIN_ITEM_BITS / 8);
        assert_eq!(l.tier1_item_bits().unwrap(), YPIR_MIN_ITEM_BITS);
        assert_eq!(l.tier2_rows().unwrap(), Some(1 << 16));
        assert_eq!(l.tier2_leaves().unwrap(), Some(8));
        assert_eq!(l.tier2_row_bytes().unwrap(), Some(768));
        assert_eq!(
            l.tier2_padded_row_bytes().unwrap(),
            Some(YPIR_MIN_ITEM_BITS / 8)
        );
        let s2 = l.tier2_scenario().unwrap().unwrap();
        assert_eq!(s2.num_items, 1 << 16);
        assert_eq!(s2.item_size_bits, YPIR_MIN_ITEM_BITS);
        assert_eq!(l.tier0_path_offset(), 7);
        assert_eq!(l.tier1_path_offset(), 3);
        l.validate_ypir_bounds().unwrap();
    }

    #[test]
    fn two_tier_geometry_is_never_padded() {
        let l = COMPILED_PIR_LAYOUT;
        assert!(!l.tier2_enabled());
        assert_eq!(l.tier1_record_bytes(), TIER1_LEAF_BYTES);
        assert_eq!(l.tier1_row_bytes().unwrap(), TIER1_ROW_BYTES);
        assert_eq!(l.tier1_padded_row_bytes().unwrap(), TIER1_ROW_BYTES);
        assert_eq!(l.tier1_item_bits().unwrap(), TIER1_ITEM_BITS);
        assert_eq!(l.tier2_rows().unwrap(), None);
        assert_eq!(l.tier2_scenario().unwrap(), None);
        assert_eq!(l.tier0_path_offset(), PIR_DEPTH - TIER0_LAYERS);
        assert_eq!(l.tier1_path_offset(), 0);
        let s1 = l.tier1_scenario().unwrap();
        assert_eq!(s1.num_items, TIER1_ROWS);
        assert_eq!(s1.item_size_bits, TIER1_ITEM_BITS);
    }

    #[test]
    fn pir_layout_serde_defaults_tier2_to_zero() {
        let legacy = r#"{"pir_depth":19,"tier0_layers":12,"tier1_layers":7}"#;
        let parsed: PirLayout = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed, COMPILED_PIR_LAYOUT);
        // Disabled state serializes without the tier2 field (wire-compatible
        // with pre-tier2 consumers).
        let json = serde_json::to_string(&COMPILED_PIR_LAYOUT).unwrap();
        assert!(!json.contains("tier2_layers"), "{json}");
        let three = layout(19, 12, 4, 3);
        let json = serde_json::to_string(&three).unwrap();
        assert!(json.contains("\"tier2_layers\":3"), "{json}");
        assert_eq!(serde_json::from_str::<PirLayout>(&json).unwrap(), three);
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
        for (t0, t1) in [(11usize, 8usize), (12, 7), (13, 6)] {
            let layout = PirLayout {
                pir_depth: 19,
                tier0_layers: t0,
                tier1_layers: t1,
                tier2_layers: 0,
            };
            layout.validate_split().unwrap();
            layout.validate_ypir_bounds().unwrap();
        }
    }

    #[test]
    fn metadata_requires_pir_layout() {
        let missing = r#"{
            "zcash_network":"main",
            "nullifier_pool":"ironwood",
            "dataset_version":2,
            "root25":"00",
            "root29":"00",
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
            "root25":"00",
            "root29":"00",
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
