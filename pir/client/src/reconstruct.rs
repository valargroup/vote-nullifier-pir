//! Layout-driven Merkle path reconstruction from plaintext + decrypted tier rows.

use anyhow::{Context, Result};
use imt_tree::hasher::PoseidonHasher;
use imt_tree::tree::TREE_DEPTH;
use imt_tree::ImtProofData;
use pasta_curves::Fp;
use pir_types::boundary_row::BoundaryRow;
use pir_types::punctured_row::PuncturedRangeRow;
use pir_types::tier0::Tier0Data;
use pir_types::{path_offset_for_tier, PirLayout, TierRowEncoding, TierTransport};

/// Copy `siblings` into `path` starting at `offset`.
#[inline]
pub fn fill_path(path: &mut [Fp; TREE_DEPTH], offset: usize, siblings: &[Fp]) -> Result<()> {
    anyhow::ensure!(
        offset
            .checked_add(siblings.len())
            .is_some_and(|end| end <= TREE_DEPTH),
        "path fill out of bounds: offset {offset} len {}",
        siblings.len()
    );
    path[offset..offset + siblings.len()].copy_from_slice(siblings);
    Ok(())
}

/// Valid punctured-range leaves in a terminal row.
#[inline]
pub fn valid_leaves_for_row(num_ranges: usize, row_idx: usize, leaves_per_row: usize) -> usize {
    let row_start = row_idx.saturating_mul(leaves_per_row);
    num_ranges.saturating_sub(row_start).min(leaves_per_row)
}

/// Process plaintext Tier 0 and return the selected child index.
pub fn process_plaintext_tier0(
    tier0: &Tier0Data,
    layout: &PirLayout,
    nullifier: Fp,
    path: &mut [Fp; TREE_DEPTH],
) -> Result<usize> {
    anyhow::ensure!(
        matches!(layout.tiers[0].transport, TierTransport::Plaintext),
        "tier 0 must be plaintext"
    );
    let s1 = tier0
        .find_subtree(nullifier)
        .context("nullifier not found in any Tier 0 subtree")?;
    let offset = path_offset_for_tier(layout, 0).map_err(anyhow::Error::msg)?;
    fill_path(path, offset, &tier0.extract_siblings(s1))?;
    Ok(s1)
}

/// Process one intermediate boundary-index row; returns the next global row index.
pub fn process_boundary_tier(
    row: &[u8],
    layout: &PirLayout,
    tier_index: usize,
    current_row: usize,
    nullifier: Fp,
    path: &mut [Fp; TREE_DEPTH],
) -> Result<usize> {
    let tier = layout
        .tiers
        .get(tier_index)
        .context("missing boundary tier")?;
    anyhow::ensure!(
        matches!(tier.row_encoding, TierRowEncoding::BoundaryIndexV1),
        "tier {tier_index} is not boundary-index"
    );
    let parsed = BoundaryRow::from_bytes(row, tier.layers)?;
    let child = parsed
        .find_child(nullifier)
        .context("nullifier not found in boundary row")?;
    let hasher = PoseidonHasher::new();
    let offset = path_offset_for_tier(layout, tier_index).map_err(anyhow::Error::msg)?;
    fill_path(path, offset, &parsed.extract_siblings(child, &hasher))?;
    let next = current_row
        .checked_mul(tier.records_per_row)
        .and_then(|v| v.checked_add(child))
        .context("next row index overflow")?;
    Ok(next)
}

/// Process the terminal punctured-range row and assemble the circuit proof.
#[allow(clippy::too_many_arguments)]
pub fn process_terminal_tier(
    row: &[u8],
    layout: &PirLayout,
    tier_index: usize,
    row_idx: usize,
    num_ranges: usize,
    nullifier: Fp,
    path: &mut [Fp; TREE_DEPTH],
    empty_hashes: &[Fp; TREE_DEPTH],
    circuits_root: Fp,
) -> Result<ImtProofData> {
    let tier = layout
        .tiers
        .get(tier_index)
        .context("missing terminal tier")?;
    anyhow::ensure!(
        matches!(tier.row_encoding, TierRowEncoding::PuncturedRangeK2V1),
        "tier {tier_index} is not punctured-range"
    );
    let hasher = PoseidonHasher::new();
    let parsed = PuncturedRangeRow::from_bytes(row, tier.layers)?;
    let valid_leaves = valid_leaves_for_row(num_ranges, row_idx, tier.records_per_row);
    let leaf_local_idx = parsed
        .find_leaf(nullifier, valid_leaves)
        .context("nullifier not found in terminal leaf scan")?;
    let offset = path_offset_for_tier(layout, tier_index).map_err(anyhow::Error::msg)?;
    fill_path(
        path,
        offset,
        &parsed.extract_siblings(leaf_local_idx, valid_leaves, &hasher),
    )?;
    // Pad from PIR depth to circuit depth.
    fill_path(
        path,
        layout.pir_height,
        &empty_hashes[layout.pir_height..TREE_DEPTH],
    )?;

    let global_leaf_idx = row_idx
        .checked_mul(tier.records_per_row)
        .and_then(|v| v.checked_add(leaf_local_idx))
        .context("global leaf index overflow")?;
    let (nf_lo, nf_mid, nf_hi) = parsed.leaf_record(leaf_local_idx);
    Ok(ImtProofData {
        root: circuits_root,
        nf_bounds: [nf_lo, nf_mid, nf_hi],
        leaf_pos: global_leaf_idx as u32,
        path: *path,
    })
}
