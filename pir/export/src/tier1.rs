//! Tier 1 export: TIER1_ROWS rows of TIER1_LEAVES punctured-range leaf records (K=2).
//!
//! Row layout (TIER1_ROW_BYTES bytes):
//! ```text
//! [leaf records: TIER1_LEAVES × (32-byte nf_lo + 32-byte nf_mid + 32-byte nf_hi)]
//!   record i: nf_lo at i*96, nf_mid at i*96+32, nf_hi at i*96+64
//! ```
//!
//! Internal nodes are not stored; the client rebuilds the subtree locally.

use std::io::Write;

use anyhow::Result;

use imt_tree::tree::PuncturedRange;

use crate::{write_fp, TIER1_LEAF_BYTES, TIER1_LEAVES, TIER1_ROWS, TIER1_ROW_BYTES};

pub use pir_types::tier1::Tier1Row;

/// Export all Tier 1 rows to a writer.
pub fn export(ranges: &[PuncturedRange], writer: &mut impl Write) -> Result<()> {
    let mut buf = vec![0u8; TIER1_ROW_BYTES];

    for s in 0..TIER1_ROWS {
        write_row(ranges, s, &mut buf);
        writer.write_all(&buf)?;
    }

    Ok(())
}

/// Write a single Tier 1 row for subtree index `s`.
fn write_row(ranges: &[PuncturedRange], s: usize, buf: &mut [u8]) {
    buf.fill(0);
    let leaf_start = s * TIER1_LEAVES;
    let mut offset = 0;

    for i in 0..TIER1_LEAVES {
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
            offset += TIER1_LEAF_BYTES; // already zeroed by buf.fill(0)
        }
    }

    debug_assert_eq!(offset, TIER1_ROW_BYTES);
}
