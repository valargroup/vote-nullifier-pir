//! Export negotiated tier blobs from a built [`PirTree`].
//!
//! The default production layout remains `[12, 7]` via [`crate::tier0`] /
//! [`crate::tier1`]. This module additionally supports alternative in-envelope
//! splits such as `[12, 3, 4]` for proving that one client can follow server
//! metadata without a rebuild.

use anyhow::{Context, Result};

use imt_tree::tree::{PuncturedRange, TREE_DEPTH};

use crate::{
    node_or_empty, subtree_min_key, write_fp, PirTree, PIR_DEPTH, TIER0_LAYERS, TIER1_LEAF_BYTES,
};
use pir_types::{layout_from_splits, PirLayout, TierRowEncoding, TierTransport};

/// In-memory multi-tier export produced from a single [`PirTree`].
pub struct LayoutExport {
    pub layout: PirLayout,
    /// Plaintext Tier 0 bytes.
    pub tier0: Vec<u8>,
    /// Encrypted-tier databases in ordinal order after Tier 0.
    /// Each entry is a concatenation of logical rows (YPIR padding omitted;
    /// callers/tests pad when constructing YPIR scenarios).
    pub encrypted_tiers: Vec<Vec<u8>>,
}

/// Export tier blobs for an arbitrary split that sums to [`PIR_DEPTH`].
///
/// The first split must equal [`TIER0_LAYERS`] so the existing Tier 0 exporter
/// can be reused.
pub fn export_for_splits(
    tree: &PirTree,
    splits: &[usize],
    snapshot_id: &str,
) -> Result<LayoutExport> {
    anyhow::ensure!(
        !splits.is_empty() && splits[0] == TIER0_LAYERS,
        "first split must be TIER0_LAYERS ({TIER0_LAYERS})"
    );
    let layout = layout_from_splits(snapshot_id, PIR_DEPTH, TREE_DEPTH, splits)
        .map_err(anyhow::Error::msg)?;

    let tier0 = crate::tier0::export(
        &tree.pir_root,
        &tree.levels,
        &tree.ranges,
        &tree.empty_hashes,
    );

    let mut encrypted_tiers = Vec::new();
    let mut depth_after = splits[0];
    for (tier_idx, tier) in layout.tiers.iter().enumerate().skip(1) {
        let blob = match tier.row_encoding {
            TierRowEncoding::BoundaryIndexV1 => export_boundary_tier(
                tree,
                depth_after,
                tier.layers,
                tier.logical_rows,
                tier.records_per_row,
                tier.payload_bytes,
            )?,
            TierRowEncoding::PuncturedRangeK2V1 => export_punctured_tier(
                &tree.ranges,
                tier.logical_rows,
                tier.records_per_row,
                tier.payload_bytes,
            )?,
        };
        anyhow::ensure!(
            matches!(tier.transport, TierTransport::YpirSimplepirV1),
            "tier {tier_idx} must be encrypted"
        );
        encrypted_tiers.push(blob);
        depth_after = depth_after
            .checked_add(tier.layers)
            .context("depth overflow")?;
    }
    anyhow::ensure!(
        depth_after == PIR_DEPTH,
        "exported depth {depth_after} != {PIR_DEPTH}"
    );

    Ok(LayoutExport {
        layout,
        tier0,
        encrypted_tiers,
    })
}

fn export_boundary_tier(
    tree: &PirTree,
    parent_depth: usize,
    layers: usize,
    logical_rows: usize,
    records_per_row: usize,
    payload_bytes: usize,
) -> Result<Vec<u8>> {
    let child_depth = parent_depth + layers;
    let bu_level = PIR_DEPTH
        .checked_sub(child_depth)
        .context("child_depth exceeds PIR_DEPTH")?;
    let leaves_per_child = 1usize << (PIR_DEPTH - child_depth);
    let mut out = vec![0u8; logical_rows * payload_bytes];
    for row in 0..logical_rows {
        let row_off = row * payload_bytes;
        let mut offset = row_off;
        for r in 0..records_per_row {
            let child_idx = row * records_per_row + r;
            let hash = node_or_empty(&tree.levels, bu_level, child_idx, &tree.empty_hashes);
            write_fp(&mut out[offset..], hash);
            offset += 32;
            let leaf_start = child_idx * leaves_per_child;
            let mk = subtree_min_key(&tree.ranges, leaf_start);
            write_fp(&mut out[offset..], mk);
            offset += 32;
        }
        anyhow::ensure!(offset - row_off == payload_bytes);
    }
    Ok(out)
}

fn export_punctured_tier(
    ranges: &[PuncturedRange],
    logical_rows: usize,
    records_per_row: usize,
    payload_bytes: usize,
) -> Result<Vec<u8>> {
    let mut out = vec![0u8; logical_rows * payload_bytes];
    for row in 0..logical_rows {
        let row_off = row * payload_bytes;
        let mut offset = row_off;
        let leaf_start = row * records_per_row;
        for i in 0..records_per_row {
            let global_idx = leaf_start + i;
            if global_idx < ranges.len() {
                let [nf_lo, nf_mid, nf_hi] = ranges[global_idx];
                write_fp(&mut out[offset..], nf_lo);
                offset += 32;
                write_fp(&mut out[offset..], nf_mid);
                offset += 32;
                write_fp(&mut out[offset..], nf_hi);
                offset += 32;
            } else {
                offset += TIER1_LEAF_BYTES;
            }
        }
        anyhow::ensure!(offset - row_off == payload_bytes);
    }
    Ok(out)
}

/// Convenience: default `[12, 7]` export as a [`LayoutExport`].
pub fn export_default(tree: &PirTree, snapshot_id: &str) -> Result<LayoutExport> {
    export_for_splits(tree, &[TIER0_LAYERS, PIR_DEPTH - TIER0_LAYERS], snapshot_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_ranges_with_sentinels;
    use pasta_curves::Fp;

    #[test]
    fn twelve_three_four_export_sizes() {
        let nfs: Vec<Fp> = (1u64..=40).map(|i| Fp::from(i * 17)).collect();
        let tree = crate::build_pir_tree(build_ranges_with_sentinels(&nfs)).unwrap();
        let exported = export_for_splits(&tree, &[12, 3, 4], "t").unwrap();
        assert_eq!(exported.encrypted_tiers.len(), 2);
        assert_eq!(
            exported.encrypted_tiers[0].len(),
            exported.layout.tiers[1].logical_rows * exported.layout.tiers[1].payload_bytes
        );
        assert_eq!(
            exported.encrypted_tiers[1].len(),
            exported.layout.tiers[2].logical_rows * exported.layout.tiers[2].payload_bytes
        );
    }
}
