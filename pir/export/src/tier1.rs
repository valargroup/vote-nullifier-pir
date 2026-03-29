//! Tier 1 export: TIER1_ROWS logical rows of TIER1_LEAVES leaf records (hash + min_key),
//! padded to TIER1_YPIR_ROWS rows for YPIR's minimum row requirement.
//!
//! Row layout (TIER1_ROW_BYTES bytes):
//! ```text
//! [leaf records: TIER1_LEAVES × (32-byte hash + 32-byte min_key)]
//!   record i: hash at i*64, min_key at i*64+32
//! ```
//!
//! Internal nodes are not stored; the client rebuilds the subtree locally.

use std::io::Write;

use anyhow::Result;
use pasta_curves::Fp;

use imt_tree::tree::{PuncturedRange, TREE_DEPTH};

use crate::{
    node_or_empty, subtree_min_key, write_fp, PIR_DEPTH, TIER0_LAYERS, TIER1_LAYERS,
    TIER1_LEAVES, TIER1_ROWS, TIER1_ROW_BYTES, TIER1_YPIR_ROWS, TIER2_LEAVES,
};

pub use pir_types::tier1::Tier1Row;

/// Export all Tier 1 rows to a writer.
///
/// Writes TIER1_YPIR_ROWS rows total. The first TIER1_ROWS contain real data;
/// the remaining rows are zero-padded to satisfy YPIR's minimum row count.
pub fn export(
    levels: &[Vec<Fp>],
    ranges: &[PuncturedRange],
    empty_hashes: &[Fp; TREE_DEPTH],
    writer: &mut impl Write,
) -> Result<()> {
    let mut buf = vec![0u8; TIER1_ROW_BYTES];

    for s in 0..TIER1_YPIR_ROWS {
        if s < TIER1_ROWS {
            write_row(levels, ranges, empty_hashes, s, &mut buf);
        } else {
            buf.fill(0);
        }
        writer.write_all(&buf)?;
    }

    Ok(())
}

/// Write a single Tier 1 row for subtree index `s`.
fn write_row(
    levels: &[Vec<Fp>],
    ranges: &[PuncturedRange],
    empty_hashes: &[Fp; TREE_DEPTH],
    s: usize,
    buf: &mut [u8],
) {
    buf.fill(0);
    let bu_base = PIR_DEPTH - TIER0_LAYERS;

    let bu_leaf = bu_base - TIER1_LAYERS;
    let leaf_start = s * TIER1_LEAVES;
    let mut offset = 0;

    for i in 0..TIER1_LEAVES {
        let global_idx = leaf_start + i;

        let hash = node_or_empty(levels, bu_leaf, global_idx, empty_hashes);
        write_fp(&mut buf[offset..], hash);
        offset += 32;

        // min_key: smallest nf_lo among all leaves in this Tier 2 subtree.
        let range_start = global_idx * TIER2_LEAVES;
        let mk = subtree_min_key(ranges, range_start);
        write_fp(&mut buf[offset..], mk);
        offset += 32;
    }

    debug_assert_eq!(offset, TIER1_ROW_BYTES);
}
