//! Tier 0 export: plaintext internal nodes + subtree records.
//!
//! Layout (layout.tier0_bytes() bytes):
//! ```text
//! [depth 0: 1 × 32 bytes (root)]
//! [depth 1: 2 × 32 bytes]
//! ...
//! [depth tier0_layers-1: 2^(tier0_layers-1) × 32 bytes]
//! [subtree records: tier1_rows × (32-byte hash + 32-byte min_key)]
//! ```
//!
//! BFS position of node at depth d, index i: `(2^d - 1) + i`.
//! Byte offset: `((2^d - 1) + i) * 32`.

use anyhow::{Context, Result};
use pasta_curves::Fp;

use imt_tree::tree::{PuncturedRange, TREE_DEPTH};

use crate::{node_or_empty, subtree_min_key, write_fp, write_internal_nodes, COMPILED_PIR_LAYOUT};
use pir_types::PirLayout;

pub use pir_types::tier0::{Tier0Data, TIER0_BYTES, TIER0_INTERNAL_NODES};

/// Export Tier 0 for the compiled production layout.
pub fn export(
    root: &Fp,
    levels: &[Vec<Fp>],
    ranges: &[PuncturedRange],
    empty_hashes: &[Fp; TREE_DEPTH],
) -> Vec<u8> {
    export_layout(root, levels, ranges, empty_hashes, COMPILED_PIR_LAYOUT)
        .expect("compiled PIR layout Tier 0 export is valid")
}

/// Export Tier 0 as a flat binary blob for a supported two-tier layout.
///
/// The returned Vec contains all internal node hashes (depths 0 through
/// `tier0_layers - 1` in BFS order) followed by `tier1_rows` subtree records
/// (hash + min_key) at depth `tier0_layers`.
pub fn export_layout(
    root: &Fp,
    levels: &[Vec<Fp>],
    ranges: &[PuncturedRange],
    empty_hashes: &[Fp; TREE_DEPTH],
    layout: PirLayout,
) -> Result<Vec<u8>> {
    layout
        .validate_supported()
        .map_err(anyhow::Error::msg)
        .context("invalid Tier 0 export layout")?;

    let tier0_layers = layout.tier0_layers;
    let num_subtrees = layout.tier1_rows().map_err(anyhow::Error::msg)?;
    let expected = layout.tier0_bytes().map_err(anyhow::Error::msg)?;
    let internal_nodes = layout.tier0_internal_nodes().map_err(anyhow::Error::msg)?;

    let mut buf = vec![0u8; expected];
    let mut offset = 0;

    write_fp(&mut buf[offset..], *root);
    offset += 32;

    offset += write_internal_nodes(
        levels,
        empty_hashes,
        layout.pir_depth,
        tier0_layers,
        0,
        &mut buf[offset..],
    );

    debug_assert_eq!(offset, internal_nodes * 32);

    let bu_subtree_level = layout.pir_depth - tier0_layers;
    let leaves_per_subtree = 1usize << (layout.pir_depth - tier0_layers);

    for s in 0..num_subtrees {
        let hash = node_or_empty(levels, bu_subtree_level, s, empty_hashes);
        write_fp(&mut buf[offset..], hash);
        offset += 32;

        let leaf_start = s * leaves_per_subtree;
        let mk = subtree_min_key(ranges, leaf_start);
        write_fp(&mut buf[offset..], mk);
        offset += 32;
    }

    debug_assert_eq!(offset, expected);
    Ok(buf)
}
