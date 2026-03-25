//! Tier 2 export: 262,144 rows, each a depth-18 subtree with 7 internal layers
//! + 128 punctured-range leaf records (K=2).
//!
//! Row layout (16,320 bytes):
//! ```text
//! [internal nodes: 126 × 32 bytes, relative depths 1-6 in BFS order]
//!   depth 1: 2 nodes  → bytes [0..64)
//!   depth 2: 4 nodes  → bytes [64..192)
//!   ...
//!   depth 6: 64 nodes → bytes [3008..4032)
//! [leaf records: 128 × (32-byte nf_lo + 32-byte nf_mid + 32-byte nf_hi)]
//!   record i: nf_lo at 4032+i*96, nf_mid at 4032+i*96+32, nf_hi at 4032+i*96+64
//! ```

use std::io::Write;

use anyhow::Result;
use pasta_curves::Fp;

use imt_tree::tree::{PuncturedRange, TREE_DEPTH};

use crate::{
    write_fp, write_internal_nodes, PIR_DEPTH, TIER0_LAYERS, TIER1_LAYERS, TIER2_INTERNAL_NODES,
    TIER2_LAYERS, TIER2_LEAF_BYTES, TIER2_LEAVES, TIER2_ROWS, TIER2_ROW_BYTES,
};

pub use pir_types::tier2::Tier2Row;

const PROGRESS_INTERVAL: usize = 100_000;

/// Export all Tier 2 rows to a writer.
pub fn export(
    levels: &[Vec<Fp>],
    ranges: &[PuncturedRange],
    empty_hashes: &[Fp; TREE_DEPTH],
    writer: &mut impl Write,
) -> Result<()> {
    let mut buf = vec![0u8; TIER2_ROW_BYTES];

    for s in 0..TIER2_ROWS {
        write_row(levels, ranges, empty_hashes, s, &mut buf);
        writer.write_all(&buf)?;
        if s > 0 && s % PROGRESS_INTERVAL == 0 {
            tracing::info!(row = s, total = TIER2_ROWS, "Tier 2 export progress");
        }
    }

    Ok(())
}

/// Write a single Tier 2 row for subtree index `s` (at depth 18).
fn write_row(
    levels: &[Vec<Fp>],
    ranges: &[PuncturedRange],
    empty_hashes: &[Fp; TREE_DEPTH],
    s: usize,
    buf: &mut [u8],
) {
    buf.fill(0);
    let bu_base = PIR_DEPTH - TIER0_LAYERS - TIER1_LAYERS;

    let mut offset = write_internal_nodes(levels, empty_hashes, bu_base, TIER2_LAYERS, s, buf);
    debug_assert_eq!(offset, TIER2_INTERNAL_NODES * 32);

    let leaf_start = s * TIER2_LEAVES;

    for i in 0..TIER2_LEAVES {
        let global_idx = leaf_start + i;
        if global_idx < ranges.len() {
            let [nf_lo, nf_mid, nf_hi] = ranges[global_idx];
            write_fp(&mut buf[offset..], nf_lo);
            offset += 32;
            write_fp(&mut buf[offset..], nf_mid);
            offset += 32;
            write_fp(&mut buf[offset..], nf_hi);
            offset += 32;
        } else {
            offset += TIER2_LEAF_BYTES; // already zeroed by buf.fill(0)
        }
    }

    debug_assert_eq!(offset, TIER2_ROW_BYTES);
}
