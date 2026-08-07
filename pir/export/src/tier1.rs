//! Tier 1 export: punctured-range leaf records (K=2) for a negotiated layout.
//!
//! Row layout (`layout.tier1_row_bytes()` bytes):
//! ```text
//! [leaf records: tier1_leaves × (32-byte nf_lo + 32-byte nf_mid + 32-byte nf_hi)]
//!   record i: nf_lo at i*96, nf_mid at i*96+32, nf_hi at i*96+64
//! ```
//!
//! Internal nodes are not stored; the client rebuilds the subtree locally.

use std::io::Write;

use anyhow::{Context, Result};

use imt_tree::tree::PuncturedRange;

use crate::{write_fp, COMPILED_PIR_LAYOUT, TIER1_LEAF_BYTES};
use pir_types::PirLayout;

pub use pir_types::tier1::Tier1Row;

/// Export all Tier 1 rows for the compiled production layout.
pub fn export(ranges: &[PuncturedRange], writer: &mut impl Write) -> Result<()> {
    export_layout(ranges, writer, COMPILED_PIR_LAYOUT)
}

/// Export all Tier 1 rows for a supported two-tier layout.
pub fn export_layout(
    ranges: &[PuncturedRange],
    writer: &mut impl Write,
    layout: PirLayout,
) -> Result<()> {
    layout
        .validate_supported()
        .map_err(anyhow::Error::msg)
        .context("invalid Tier 1 export layout")?;

    let num_rows = layout.tier1_rows().map_err(anyhow::Error::msg)?;
    let leaves = layout.tier1_leaves().map_err(anyhow::Error::msg)?;
    let row_bytes = layout.tier1_row_bytes().map_err(anyhow::Error::msg)?;
    let mut buf = vec![0u8; row_bytes];

    for s in 0..num_rows {
        write_row(ranges, s, leaves, row_bytes, &mut buf);
        writer.write_all(&buf)?;
    }

    Ok(())
}

/// Write a single Tier 1 row for subtree index `s`.
fn write_row(ranges: &[PuncturedRange], s: usize, leaves: usize, row_bytes: usize, buf: &mut [u8]) {
    buf.fill(0);
    let leaf_start = s * leaves;
    let mut offset = 0;

    for i in 0..leaves {
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

    debug_assert_eq!(offset, row_bytes);
}
