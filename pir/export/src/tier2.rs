//! Tier 2 export: TIER2_ROWS rows of TIER2_LEAVES punctured-range leaf records (K=2).
//!
//! Row layout (TIER2_ROW_BYTES bytes):
//! ```text
//! [leaf records: TIER2_LEAVES × (32-byte nf_lo + 32-byte nf_mid + 32-byte nf_hi)]
//!   record i: nf_lo at i*96, nf_mid at i*96+32, nf_hi at i*96+64
//! ```
//!
//! Internal nodes are not stored; the client rebuilds the subtree locally.

use std::io::Write;

use anyhow::Result;

use imt_tree::tree::PuncturedRange;

use crate::{write_fp, TIER2_LEAF_BYTES, TIER2_LEAVES, TIER2_ROWS, TIER2_ROW_BYTES};

pub use pir_types::tier2::Tier2Row;

const PROGRESS_INTERVAL: usize = 100_000;

/// Export all Tier 2 rows to a writer.
pub fn export(ranges: &[PuncturedRange], writer: &mut impl Write) -> Result<()> {
    let mut buf = vec![0u8; TIER2_ROW_BYTES];

    for s in 0..TIER2_ROWS {
        write_row(ranges, s, &mut buf);
        writer.write_all(&buf)?;
        if s > 0 && s % PROGRESS_INTERVAL == 0 {
            tracing::info!(row = s, total = TIER2_ROWS, "Tier 2 export progress");
        }
    }

    Ok(())
}

/// Write a single Tier 2 row for subtree index `s` (at depth TIER0_LAYERS + TIER1_LAYERS = 15).
fn write_row(ranges: &[PuncturedRange], s: usize, buf: &mut [u8]) {
    buf.fill(0);
    let leaf_start = s * TIER2_LEAVES;
    let mut offset = 0;

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
