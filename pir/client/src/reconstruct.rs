//! Layout-driven Merkle path reconstruction from plaintext Tier 0 and
//! decrypted encrypted-tier rows.
//!
//! Path layout for a `t0 + t1 + t2` split (all offsets runtime):
//! `path[0..t_last]` terminal siblings, `path[t2..t2+t1]` boundary siblings
//! (three-tier only), `path[depth-t0..depth]` Tier 0 siblings,
//! `path[depth..29]` circuit padding.

use anyhow::{Context, Result};
use imt_tree::hasher::PoseidonHasher;
use imt_tree::tree::TREE_DEPTH;
use imt_tree::ImtProofData;
use pasta_curves::Fp;
use pir_types::boundary_row::BoundaryRow;
use pir_types::tier0::Tier0Data;
use pir_types::tier1::Tier1Row;
use pir_types::PirLayout;

/// Copy `siblings` into `path` starting at `offset`.
#[inline]
pub(crate) fn fill_path(path: &mut [Fp; TREE_DEPTH], offset: usize, siblings: &[Fp]) -> Result<()> {
    anyhow::ensure!(
        offset
            .checked_add(siblings.len())
            .is_some_and(|end| end <= TREE_DEPTH),
        "path fill out of bounds"
    );
    path[offset..offset + siblings.len()].copy_from_slice(siblings);
    Ok(())
}

/// Return the number of populated leaves in a terminal-tier row, clamped to
/// `leaves_per_row`. The final row may be only partially filled when
/// `num_ranges` is not a multiple of the row size.
#[inline]
pub(crate) fn valid_leaves_for_row(
    num_ranges: usize,
    row_idx: usize,
    leaves_per_row: usize,
) -> usize {
    let row_start = row_idx.saturating_mul(leaves_per_row);
    num_ranges.saturating_sub(row_start).min(leaves_per_row)
}

/// Locate the nullifier's subtree in Tier 0, fill its siblings into `path`,
/// and return the subtree index (the Tier 1 row index).
pub(crate) fn process_plaintext_tier0(
    tier0: &Tier0Data,
    layout: &PirLayout,
    nullifier: Fp,
    path: &mut [Fp; TREE_DEPTH],
) -> Result<usize> {
    let s1 = tier0
        .find_subtree(nullifier)
        .context("nullifier not found in any Tier 0 subtree")?;
    fill_path(
        path,
        layout.tier0_path_offset(),
        &tier0.extract_siblings(s1),
    )?;
    Ok(s1)
}

/// Parse a boundary Tier 1 row (three-tier layouts), select the child
/// containing the nullifier, fill the Tier 1 siblings into `path`, and
/// return the Tier 2 row index (`row1 * 2^tier1_layers + child`).
///
/// Child selection is total and fixed-work (see [`BoundaryRow::find_child`]):
/// a hostile row can steer the follow-up index but can never make selection
/// fail, so it cannot suppress the Tier 2 query.
pub(crate) fn process_boundary_tier1(
    row: &[u8],
    layout: &PirLayout,
    row1: usize,
    nullifier: Fp,
    path: &mut [Fp; TREE_DEPTH],
) -> Result<usize> {
    let parsed = BoundaryRow::from_bytes(row, layout.tier1_layers)?;
    let child = parsed.find_child(nullifier);
    let hasher = PoseidonHasher::new();
    fill_path(
        path,
        layout.tier1_path_offset(),
        &parsed.extract_siblings(child, &hasher),
    )?;
    row1.checked_mul(parsed.records())
        .and_then(|v| v.checked_add(child))
        .context("tier2 row index overflow")
}

/// Parse the terminal punctured-range row (Tier 1 when Tier 2 is disabled,
/// Tier 2 otherwise), locate the nullifier's leaf, fill terminal siblings and
/// circuit padding into `path`, and assemble the final [`ImtProofData`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn process_terminal(
    row: &[u8],
    layout: &PirLayout,
    terminal_layers: usize,
    row_idx: usize,
    num_ranges: usize,
    nullifier: Fp,
    path: &mut [Fp; TREE_DEPTH],
    empty_hashes: &[Fp; TREE_DEPTH],
    root29: Fp,
) -> Result<ImtProofData> {
    let hasher = PoseidonHasher::new();
    let parsed = Tier1Row::from_layers(row, terminal_layers)?;
    let leaves_per_row = parsed.leaves();
    let valid_leaves = valid_leaves_for_row(num_ranges, row_idx, leaves_per_row);

    let leaf_local_idx = parsed
        .find_leaf(nullifier, valid_leaves)
        .context("nullifier not found in terminal leaf scan")?;

    fill_path(
        path,
        0,
        &parsed.extract_siblings(leaf_local_idx, valid_leaves, &hasher),
    )?;
    // Pad from the PIR depth to the circuit depth with empty hashes.
    fill_path(
        path,
        layout.pir_depth,
        &empty_hashes[layout.pir_depth..TREE_DEPTH],
    )?;

    let global_leaf_idx = row_idx
        .checked_mul(leaves_per_row)
        .and_then(|v| v.checked_add(leaf_local_idx))
        .context("global leaf index overflow")?;
    let (nf_lo, nf_mid, nf_hi) = parsed.leaf_record(leaf_local_idx);

    Ok(ImtProofData {
        root: root29,
        nf_bounds: [nf_lo, nf_mid, nf_hi],
        leaf_pos: global_leaf_idx as u32,
        path: *path,
    })
}

/// Run the same work shape as a real boundary-row scan against a zeroed row.
///
/// Called on latched-failure paths so the tier1→tier2 inter-query gap does
/// not reveal (at local-compute timescales) whether reconstruction succeeded.
/// Residual ms-scale jitter is dominated by network variance and accepted.
pub(crate) fn dummy_boundary_work(layout: &PirLayout, nullifier: Fp) {
    let payload_bytes = match layout.tier1_row_bytes() {
        Ok(bytes) => bytes,
        Err(_) => return,
    };
    let zero_row = vec![0u8; payload_bytes];
    if let Ok(parsed) = BoundaryRow::from_bytes(&zero_row, layout.tier1_layers) {
        let child = parsed.find_child(nullifier);
        let hasher = PoseidonHasher::new();
        let _ = parsed.extract_siblings(child, &hasher);
    }
}
