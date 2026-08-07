//! Negotiated PIR tier layout: server-advertised topology the client consumes.
//!
//! A wallet that understands this contract can follow future in-envelope tier
//! splits (one encrypted query today, two tomorrow) without another release.
//! Ordinary height/root/range changes are also server-only updates.

use serde::{Deserialize, Serialize};

use crate::{
    YpirScenario, CIRCUIT_HEIGHT, DATASET_VERSION, NULLIFIER_POOL, PIR_DEPTH, TIER0_LAYERS,
    TIER1_ITEM_BITS, TIER1_LAYERS, TIER1_LEAF_BYTES, TIER1_LEAVES, TIER1_ROWS, TIER1_ROW_BYTES,
    YPIR_MIN_ITEM_BITS, YPIR_MIN_ROWS,
};

/// Wire schema version for [`PirLayout`].
pub const LAYOUT_WIRE_VERSION: u32 = 1;

/// Canonical leaf encoding for Ironwood punctured-range (K=2) proofs.
pub const LEAF_ENCODING_PUNCTURED_RANGE_K2_V1: &str = "punctured-range-k2-v1";

/// How a tier's rows are delivered to the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TierTransport {
    /// Downloaded once at connect time (Tier 0).
    Plaintext,
    /// One encrypted YPIR SimplePIR query per proof.
    YpirSimplepirV1,
}

/// Binary row codec used inside a tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TierRowEncoding {
    /// Records of `(subtree_hash, min_key)` — 64 bytes each.
    BoundaryIndexV1,
    /// Terminal punctured-range records `(nf_lo, nf_mid, nf_hi)` — 96 bytes each.
    PuncturedRangeK2V1,
}

impl TierRowEncoding {
    /// Bytes per record for this encoding.
    pub const fn record_bytes(self) -> usize {
        match self {
            Self::BoundaryIndexV1 => 64,
            Self::PuncturedRangeK2V1 => TIER1_LEAF_BYTES,
        }
    }
}

/// Descriptor for one tier in an ordered layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierDescriptor {
    /// Zero-based ordinal; must match vector index.
    pub ordinal: u32,
    pub transport: TierTransport,
    pub row_encoding: TierRowEncoding,
    /// Merkle layers contributed by this tier.
    pub layers: usize,
    /// Number of addressable logical rows before YPIR padding.
    pub logical_rows: usize,
    /// Records (children or leaves) packed into each logical row.
    pub records_per_row: usize,
    /// Decrypted/plaintext payload size of one logical row in bytes.
    pub payload_bytes: usize,
    /// Present iff `transport == YpirSimplepirV1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pir: Option<YpirScenario>,
}

/// Server-advertised layout bound to a snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PirLayout {
    /// Layout wire schema version.
    pub wire_version: u32,
    /// Opaque snapshot identity (binds network/height/root/layout).
    pub snapshot_id: String,
    /// PIR Merkle depth (layers summed by `tiers`).
    pub pir_height: usize,
    /// Circuit authentication-path depth (always [`CIRCUIT_HEIGHT`] for this generation).
    pub circuit_height: usize,
    /// Leaf commitment encoding.
    pub leaf_encoding: String,
    /// Ordered tiers from root toward the leaves.
    pub tiers: Vec<TierDescriptor>,
}

/// Resource / protocol envelope enforced by this wallet generation.
#[derive(Debug, Clone, Copy)]
pub struct LayoutBounds {
    pub min_tiers: usize,
    pub max_tiers: usize,
    pub max_pir_height: usize,
    pub max_plaintext_bytes: usize,
    pub max_row_payload_bytes: usize,
    pub max_logical_rows: usize,
}

impl Default for LayoutBounds {
    fn default() -> Self {
        Self {
            min_tiers: 2,
            max_tiers: 8,
            max_pir_height: CIRCUIT_HEIGHT,
            max_plaintext_bytes: 64 * 1024 * 1024,
            max_row_payload_bytes: 16 * 1024 * 1024,
            max_logical_rows: 1 << 19,
        }
    }
}

/// Build the current dataset-v2 default layout: plaintext Tier 0 + one YPIR tier (`[12, 7]`).
pub fn current_layout(snapshot_id: impl Into<String>) -> PirLayout {
    layout_from_splits(
        snapshot_id,
        PIR_DEPTH,
        CIRCUIT_HEIGHT,
        &[TIER0_LAYERS, TIER1_LAYERS],
    )
    .expect("default [12,7] layout is valid")
}

/// Build a layout from an ordered split of layer counts.
///
/// The first tier is always plaintext boundary-index. Intermediate encrypted
/// tiers (if any) use boundary-index rows. The last tier is always a
/// punctured-range terminal tier.
pub fn layout_from_splits(
    snapshot_id: impl Into<String>,
    pir_height: usize,
    circuit_height: usize,
    splits: &[usize],
) -> Result<PirLayout, String> {
    if splits.len() < 2 {
        return Err("layout requires at least plaintext + terminal tiers".into());
    }
    let sum: usize = splits.iter().sum();
    if sum != pir_height {
        return Err(format!(
            "tier splits sum to {sum}, expected pir_height {pir_height}"
        ));
    }

    let mut tiers = Vec::with_capacity(splits.len());
    let mut cumulative = 0usize;
    for (i, &layers) in splits.iter().enumerate() {
        let logical_rows = if cumulative == 0 {
            1
        } else {
            1usize
                .checked_shl(cumulative as u32)
                .ok_or_else(|| format!("logical_rows overflow at tier {i}"))?
        };
        let records_per_row = 1usize
            .checked_shl(layers as u32)
            .ok_or_else(|| format!("records_per_row overflow at tier {i}"))?;
        let is_first = i == 0;
        let is_last = i + 1 == splits.len();

        let (transport, row_encoding) = if is_first {
            (TierTransport::Plaintext, TierRowEncoding::BoundaryIndexV1)
        } else if is_last {
            (
                TierTransport::YpirSimplepirV1,
                TierRowEncoding::PuncturedRangeK2V1,
            )
        } else {
            (
                TierTransport::YpirSimplepirV1,
                TierRowEncoding::BoundaryIndexV1,
            )
        };

        let payload_bytes = if is_first {
            // BFS internal nodes for `layers` depths + `records_per_row` boundary records.
            let internal_nodes = records_per_row
                .checked_sub(1)
                .ok_or_else(|| "plaintext layers must be >= 1".to_string())?;
            internal_nodes
                .checked_mul(32)
                .and_then(|n| n.checked_add(records_per_row.checked_mul(64)?))
                .ok_or_else(|| "plaintext payload_bytes overflow".to_string())?
        } else {
            records_per_row
                .checked_mul(row_encoding.record_bytes())
                .ok_or_else(|| format!("payload_bytes overflow at tier {i}"))?
        };

        let pir = if matches!(transport, TierTransport::YpirSimplepirV1) {
            let num_items = logical_rows.max(YPIR_MIN_ROWS);
            let item_size_bits = payload_bytes
                .checked_mul(8)
                .ok_or_else(|| format!("item_size_bits overflow at tier {i}"))?
                .max(YPIR_MIN_ITEM_BITS);
            Some(YpirScenario {
                num_items,
                item_size_bits,
            })
        } else {
            None
        };

        tiers.push(TierDescriptor {
            ordinal: i as u32,
            transport,
            row_encoding,
            layers,
            logical_rows,
            records_per_row,
            payload_bytes,
            pir,
        });
        cumulative = cumulative
            .checked_add(layers)
            .ok_or_else(|| "cumulative layer overflow".to_string())?;
    }

    let layout = PirLayout {
        wire_version: LAYOUT_WIRE_VERSION,
        snapshot_id: snapshot_id.into(),
        pir_height,
        circuit_height,
        leaf_encoding: LEAF_ENCODING_PUNCTURED_RANGE_K2_V1.to_owned(),
        tiers,
    };
    validate_layout(&layout, &LayoutBounds::default())?;
    Ok(layout)
}

/// Validate a server-advertised layout against the protocol envelope.
pub fn validate_layout(layout: &PirLayout, bounds: &LayoutBounds) -> Result<(), String> {
    if layout.wire_version != LAYOUT_WIRE_VERSION {
        return Err(format!(
            "unsupported layout wire_version {}; expected {}",
            layout.wire_version, LAYOUT_WIRE_VERSION
        ));
    }
    if layout.snapshot_id.is_empty() {
        return Err("snapshot_id must be non-empty".into());
    }
    if layout.circuit_height != CIRCUIT_HEIGHT {
        return Err(format!(
            "circuit_height {} != supported {}",
            layout.circuit_height, CIRCUIT_HEIGHT
        ));
    }
    if layout.pir_height == 0 || layout.pir_height > bounds.max_pir_height {
        return Err(format!(
            "pir_height {} outside 1..={}",
            layout.pir_height, bounds.max_pir_height
        ));
    }
    if layout.pir_height > layout.circuit_height {
        return Err("pir_height cannot exceed circuit_height".into());
    }
    if layout.leaf_encoding != LEAF_ENCODING_PUNCTURED_RANGE_K2_V1 {
        return Err(format!(
            "unsupported leaf_encoding {:?}",
            layout.leaf_encoding
        ));
    }
    if layout.tiers.len() < bounds.min_tiers || layout.tiers.len() > bounds.max_tiers {
        return Err(format!(
            "tier count {} outside {}..={}",
            layout.tiers.len(),
            bounds.min_tiers,
            bounds.max_tiers
        ));
    }

    let mut layer_sum = 0usize;
    let mut cumulative = 0usize;
    for (i, tier) in layout.tiers.iter().enumerate() {
        if tier.ordinal as usize != i {
            return Err(format!("tier ordinal {} != index {}", tier.ordinal, i));
        }
        if tier.layers == 0 || tier.layers > layout.pir_height {
            return Err(format!("tier {i} layers {} out of range", tier.layers));
        }
        let expected_records = 1usize
            .checked_shl(tier.layers as u32)
            .ok_or_else(|| format!("tier {i} records_per_row overflow"))?;
        if tier.records_per_row != expected_records {
            return Err(format!(
                "tier {i} records_per_row {} != 2^{}",
                tier.records_per_row, tier.layers
            ));
        }
        let expected_logical = if cumulative == 0 {
            1
        } else {
            1usize
                .checked_shl(cumulative as u32)
                .ok_or_else(|| format!("tier {i} logical_rows overflow"))?
        };
        if tier.logical_rows != expected_logical {
            return Err(format!(
                "tier {i} logical_rows {} != expected {}",
                tier.logical_rows, expected_logical
            ));
        }
        if tier.logical_rows > bounds.max_logical_rows {
            return Err(format!(
                "tier {i} logical_rows {} exceeds max {}",
                tier.logical_rows, bounds.max_logical_rows
            ));
        }

        let is_first = i == 0;
        let is_last = i + 1 == layout.tiers.len();
        match (is_first, is_last, tier.transport, tier.row_encoding) {
            (true, _, TierTransport::Plaintext, TierRowEncoding::BoundaryIndexV1) => {}
            (false, true, TierTransport::YpirSimplepirV1, TierRowEncoding::PuncturedRangeK2V1) => {}
            (false, false, TierTransport::YpirSimplepirV1, TierRowEncoding::BoundaryIndexV1) => {}
            _ => {
                return Err(format!(
                    "tier {i} has unsupported transport/encoding combination"
                ));
            }
        }

        let expected_payload = if is_first {
            let internal = expected_records - 1;
            internal * 32 + expected_records * 64
        } else {
            expected_records * tier.row_encoding.record_bytes()
        };
        if tier.payload_bytes != expected_payload {
            return Err(format!(
                "tier {i} payload_bytes {} != expected {}",
                tier.payload_bytes, expected_payload
            ));
        }
        if is_first && tier.payload_bytes > bounds.max_plaintext_bytes {
            return Err(format!(
                "plaintext tier exceeds max {} bytes",
                bounds.max_plaintext_bytes
            ));
        }
        if !is_first && tier.payload_bytes > bounds.max_row_payload_bytes {
            return Err(format!(
                "tier {i} payload_bytes exceeds max {}",
                bounds.max_row_payload_bytes
            ));
        }

        match tier.transport {
            TierTransport::Plaintext => {
                if tier.pir.is_some() {
                    return Err(format!("plaintext tier {i} must not advertise pir params"));
                }
            }
            TierTransport::YpirSimplepirV1 => {
                let pir = tier
                    .pir
                    .as_ref()
                    .ok_or_else(|| format!("encrypted tier {i} missing pir params"))?;
                if pir.num_items < tier.logical_rows {
                    return Err(format!(
                        "tier {i} pir.num_items {} < logical_rows {}",
                        pir.num_items, tier.logical_rows
                    ));
                }
                if pir.num_items < YPIR_MIN_ROWS {
                    return Err(format!(
                        "tier {i} pir.num_items {} < YPIR minimum {}",
                        pir.num_items, YPIR_MIN_ROWS
                    ));
                }
                if pir.item_size_bits % 8 != 0 {
                    return Err(format!("tier {i} item_size_bits not byte-aligned"));
                }
                if pir.item_size_bits / 8 < tier.payload_bytes {
                    return Err(format!("tier {i} item_size_bits/8 < payload_bytes"));
                }
                if pir.item_size_bits < YPIR_MIN_ITEM_BITS {
                    return Err(format!(
                        "tier {i} item_size_bits {} < YPIR minimum {}",
                        pir.item_size_bits, YPIR_MIN_ITEM_BITS
                    ));
                }
            }
        }

        layer_sum = layer_sum
            .checked_add(tier.layers)
            .ok_or_else(|| "layer sum overflow".to_string())?;
        cumulative = cumulative
            .checked_add(tier.layers)
            .ok_or_else(|| "cumulative overflow".to_string())?;
    }

    if layer_sum != layout.pir_height {
        return Err(format!(
            "tier layers sum to {layer_sum}, expected pir_height {}",
            layout.pir_height
        ));
    }
    Ok(())
}

/// Number of encrypted (YPIR) tiers in a layout.
pub fn encrypted_tier_count(layout: &PirLayout) -> usize {
    layout
        .tiers
        .iter()
        .filter(|t| matches!(t.transport, TierTransport::YpirSimplepirV1))
        .count()
}

/// Path offset (leaf-up) where a tier's siblings begin.
pub fn path_offset_for_tier(layout: &PirLayout, tier_index: usize) -> Result<usize, String> {
    let lower: usize = layout
        .tiers
        .iter()
        .skip(tier_index + 1)
        .map(|t| t.layers)
        .sum();
    Ok(lower)
}

/// Snapshot id derived from network, height, and roots.
pub fn derive_snapshot_id(
    network: &str,
    height: Option<u64>,
    _pir_root_hex: &str,
    circuits_root_hex: &str,
) -> String {
    format!(
        "{}:{}:{}:{}:{}:v{}",
        NULLIFIER_POOL,
        DATASET_VERSION,
        network,
        height
            .map(|h| h.to_string())
            .unwrap_or_else(|| "none".into()),
        &circuits_root_hex[..circuits_root_hex.len().min(16)],
        LAYOUT_WIRE_VERSION
    )
}

/// Assert the compiled-in default constants match `current_layout`.
pub fn assert_default_constants_match_layout() {
    let layout = current_layout("constants-check");
    assert_eq!(layout.pir_height, PIR_DEPTH);
    assert_eq!(layout.tiers[0].layers, TIER0_LAYERS);
    assert_eq!(layout.tiers[1].layers, TIER1_LAYERS);
    assert_eq!(layout.tiers[1].logical_rows, TIER1_ROWS);
    assert_eq!(layout.tiers[1].records_per_row, TIER1_LEAVES);
    assert_eq!(layout.tiers[1].payload_bytes, TIER1_ROW_BYTES);
    let pir = layout.tiers[1].pir.as_ref().unwrap();
    assert_eq!(pir.num_items, TIER1_ROWS);
    assert_eq!(pir.item_size_bits, TIER1_ITEM_BITS);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_layout_is_twelve_seven() {
        let layout = current_layout("test");
        assert_eq!(layout.tiers.len(), 2);
        assert_eq!(layout.tiers[0].layers, 12);
        assert_eq!(layout.tiers[1].layers, 7);
        assert_eq!(encrypted_tier_count(&layout), 1);
        assert_default_constants_match_layout();
    }

    #[test]
    fn future_twelve_three_four_is_valid() {
        let layout = layout_from_splits("future", 19, 29, &[12, 3, 4]).unwrap();
        assert_eq!(encrypted_tier_count(&layout), 2);
        assert_eq!(
            layout.tiers[1].row_encoding,
            TierRowEncoding::BoundaryIndexV1
        );
        assert_eq!(
            layout.tiers[2].row_encoding,
            TierRowEncoding::PuncturedRangeK2V1
        );
        assert_eq!(layout.tiers[1].logical_rows, 1 << 12);
        assert_eq!(layout.tiers[2].logical_rows, 1 << 15);
        assert_eq!(layout.tiers[2].records_per_row, 16);
        assert_eq!(path_offset_for_tier(&layout, 0).unwrap(), 7);
        assert_eq!(path_offset_for_tier(&layout, 1).unwrap(), 4);
        assert_eq!(path_offset_for_tier(&layout, 2).unwrap(), 0);
    }

    #[test]
    fn historical_nine_six_ten_is_valid() {
        let layout = layout_from_splits("hist", 25, 29, &[9, 6, 10]).unwrap();
        assert_eq!(encrypted_tier_count(&layout), 2);
        assert_eq!(
            layout.tiers[0].payload_bytes,
            ((1 << 9) - 1) * 32 + (1 << 9) * 64
        );
        // Tier 1 logical rows are 512; YPIR pads to 2048.
        assert_eq!(layout.tiers[1].logical_rows, 512);
        assert_eq!(
            layout.tiers[1].pir.as_ref().unwrap().num_items,
            YPIR_MIN_ROWS
        );
        assert_eq!(layout.tiers[2].logical_rows, 1 << 15);
        assert_eq!(layout.tiers[2].records_per_row, 1 << 10);
    }

    #[test]
    fn rejects_bad_sum_and_codec_mismatch() {
        assert!(layout_from_splits("x", 19, 29, &[12, 8]).is_err());
        let mut layout = current_layout("x");
        layout.tiers[1].row_encoding = TierRowEncoding::BoundaryIndexV1;
        assert!(validate_layout(&layout, &LayoutBounds::default()).is_err());
    }

    #[test]
    fn rejects_wrong_circuit_height() {
        assert!(layout_from_splits("x", 19, 28, &[12, 7]).is_err());
    }
}
