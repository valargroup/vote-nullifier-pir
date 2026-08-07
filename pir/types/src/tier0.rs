//! Tier 0 reader: parse and query the plaintext internal nodes and subtree records.
//!
//! Supports the compiled-in default layout and negotiated plaintext-tier sizes.

use pasta_curves::Fp;

use crate::fp_utils::{binary_search_records, read_fp, validate_all_fp_chunks};
use crate::{TIER0_LAYERS, TIER1_ROWS};

/// Number of internal nodes in Tier 0 (depths 0 through TIER0_LAYERS-1).
pub const TIER0_INTERNAL_NODES: usize = (1 << TIER0_LAYERS) - 1; // 4,095

/// Total size of Tier 0 data in bytes for the default layout.
pub const TIER0_BYTES: usize = TIER0_INTERNAL_NODES * 32 + TIER1_ROWS * 64; // 393,184

/// Parsed Tier 0 data: internal node hashes and subtree records.
pub struct Tier0Data {
    data: Vec<u8>,
    layers: usize,
    num_subtrees: usize,
    internal_nodes: usize,
}

impl Tier0Data {
    /// Parse default-layout Tier 0 bytes.
    pub fn from_bytes(data: Vec<u8>) -> anyhow::Result<Self> {
        Self::from_bytes_layout(data, TIER0_LAYERS, TIER1_ROWS)
    }

    /// Parse Tier 0 bytes for an arbitrary plaintext tier size.
    pub fn from_bytes_layout(
        data: Vec<u8>,
        layers: usize,
        num_subtrees: usize,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(layers > 0 && layers <= 32, "invalid tier0 layers {layers}");
        anyhow::ensure!(
            num_subtrees == (1usize << layers),
            "num_subtrees {num_subtrees} != 2^{layers}"
        );
        let internal_nodes = num_subtrees - 1;
        let expected = internal_nodes
            .checked_mul(32)
            .and_then(|n| n.checked_add(num_subtrees.checked_mul(64)?))
            .ok_or_else(|| anyhow::anyhow!("tier0 size overflow"))?;
        anyhow::ensure!(
            data.len() == expected,
            "Tier 0 data size mismatch: got {} bytes, expected {expected}",
            data.len()
        );
        validate_all_fp_chunks(&data, "Tier 0")?;
        Ok(Self {
            data,
            layers,
            num_subtrees,
            internal_nodes,
        })
    }

    pub fn layers(&self) -> usize {
        self.layers
    }

    /// Root hash (depth 0).
    pub fn root(&self) -> Fp {
        read_fp(&self.data[0..32])
    }

    /// Internal node hash at the given top-down depth and index.
    pub fn node_at(&self, depth: usize, index: usize) -> Fp {
        debug_assert!(depth < self.layers);
        debug_assert!(index < (1 << depth));
        let bfs_pos = (1usize << depth) - 1 + index;
        let offset = bfs_pos * 32;
        read_fp(&self.data[offset..offset + 32])
    }

    /// Number of subtree records.
    pub fn num_subtrees(&self) -> usize {
        self.num_subtrees
    }

    /// Subtree record at depth `layers`: (hash, min_key).
    pub fn subtree_record(&self, index: usize) -> (Fp, Fp) {
        debug_assert!(index < self.num_subtrees);
        let base = self.internal_nodes * 32 + index * 64;
        let hash = read_fp(&self.data[base..base + 32]);
        let min_key = read_fp(&self.data[base + 32..base + 64]);
        (hash, min_key)
    }

    /// Binary search the subtree min_keys to find which subtree contains `value`.
    pub fn find_subtree(&self, value: Fp) -> Option<usize> {
        let base = self.internal_nodes * 32;
        binary_search_records(&self.data, base, self.num_subtrees, 64, 32, value)
    }

    /// Extract sibling hashes from Tier 0 for a given subtree index.
    pub fn extract_siblings(&self, subtree_idx: usize) -> Vec<Fp> {
        let mut siblings = Vec::with_capacity(self.layers);

        let sibling = subtree_idx ^ 1;
        let (hash, _) = self.subtree_record(sibling);
        siblings.push(hash);

        let mut pos = subtree_idx;
        for d in (1..self.layers).rev() {
            pos >>= 1;
            let sibling_pos = pos ^ 1;
            siblings.push(self.node_at(d, sibling_pos));
        }
        siblings
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_bytes_rejects_wrong_size() {
        let too_short = vec![0u8; TIER0_BYTES - 1];
        let err = Tier0Data::from_bytes(too_short)
            .err()
            .expect("should reject wrong size");
        assert!(
            err.to_string().contains("size mismatch"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn from_bytes_rejects_non_canonical_field_element() {
        let mut data = vec![0u8; TIER0_BYTES];
        data[0..32].fill(0xFF);
        let err = Tier0Data::from_bytes(data)
            .err()
            .expect("should reject non-canonical Fp");
        assert!(
            err.to_string().contains("invalid field element"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn from_bytes_accepts_all_zeros() {
        let data = vec![0u8; TIER0_BYTES];
        let tier0 = Tier0Data::from_bytes(data).expect("all-zeros is valid");
        assert_eq!(tier0.root(), Fp::zero());
        assert_eq!(tier0.num_subtrees(), TIER1_ROWS);
        assert_eq!(tier0.extract_siblings(0).len(), TIER0_LAYERS);
    }

    #[test]
    fn find_subtree_on_all_zeros() {
        let data = vec![0u8; TIER0_BYTES];
        let tier0 = Tier0Data::from_bytes(data).expect("valid");
        let result = tier0.find_subtree(Fp::from(42u64));
        assert!(result.is_some());
        assert!(result.unwrap() < TIER1_ROWS);
    }
}
