//! Tier 0 export: plaintext internal nodes + subtree records.
//!
//! Layout (TIER0_BYTES bytes):
//! ```text
//! [depth 0: 1 × 32 bytes (root)]
//! [depth 1: 2 × 32 bytes]
//! ...
//! [depth TIER0_LAYERS-1: 2^(TIER0_LAYERS-1) × 32 bytes]
//! [subtree records: TIER1_ROWS × (32-byte hash + 32-byte min_key)]
//! ```
//!
//! BFS position of node at depth d, index i: `(2^d - 1) + i`.
//! Byte offset: `((2^d - 1) + i) * 32`.

use pasta_curves::Fp;

use imt_tree::tree::{PuncturedRange, TREE_DEPTH};

use crate::{
    node_or_empty, subtree_min_key, write_fp, write_internal_nodes, PIR_DEPTH, TIER0_LAYERS,
    TIER1_ROWS,
};

pub use pir_types::tier0::{Tier0Data, TIER0_BYTES, TIER0_INTERNAL_NODES};

/// Export Tier 0 as a flat binary blob.
///
/// The returned Vec contains all internal node hashes (depths 0 through
/// TIER0_LAYERS-1 in BFS order) followed by TIER1_ROWS subtree records
/// (hash + min_key) at depth TIER0_LAYERS.
pub fn export(
    root: &Fp,
    levels: &[Vec<Fp>],
    ranges: &[PuncturedRange],
    empty_hashes: &[Fp; TREE_DEPTH],
) -> Vec<u8> {
    let mut buf = vec![0u8; TIER0_BYTES];
    let mut offset = 0;

    // ── Internal nodes: depths 0 through TIER0_LAYERS-1 (= 8) ─────────

    // Depth 0 = root (not part of the generic subtree loop)
    write_fp(&mut buf[offset..], *root);
    offset += 32;

    // Depths 1 through TIER0_LAYERS-1 (= 8).
    offset += write_internal_nodes(
        levels,
        empty_hashes,
        PIR_DEPTH,
        TIER0_LAYERS,
        0,
        &mut buf[offset..],
    );

    debug_assert_eq!(offset, TIER0_INTERNAL_NODES * 32);

    // ── Subtree records at depth TIER0_LAYERS (= 9) ────────────────────
    //
    // Each record: 32-byte hash (the node hash at this depth) + 32-byte min_key.
    // The hash is at bottom-up level PIR_DEPTH - TIER0_LAYERS (= 16).
    let bu_subtree_level = PIR_DEPTH - TIER0_LAYERS; // 16

    for s in 0..TIER1_ROWS {
        let hash = node_or_empty(levels, bu_subtree_level, s, empty_hashes);
        write_fp(&mut buf[offset..], hash);
        offset += 32;

        // min_key: smallest `low` among all leaves in this subtree.
        // Each subtree covers 2^(PIR_DEPTH - TIER0_LAYERS) = 2^16 = 65,536 leaves.
        let leaf_start = s * (1 << (PIR_DEPTH - TIER0_LAYERS));
        let mk = subtree_min_key(ranges, leaf_start);
        write_fp(&mut buf[offset..], mk);
        offset += 32;
    }

    debug_assert_eq!(offset, TIER0_BYTES);
    buf
}
