//! Tier 0 reader: parse and query the plaintext internal nodes and subtree records.

use pasta_curves::Fp;

use crate::fp_utils::{binary_search_records, read_fp, validate_all_fp_chunks};
use crate::{TIER0_LAYERS, TIER1_ROWS};

/// Number of internal nodes in Tier 0 (depths 0 through TIER0_LAYERS-1).
pub const TIER0_INTERNAL_NODES: usize = (1 << TIER0_LAYERS) - 1; // 511

/// Total size of Tier 0 data in bytes.
pub const TIER0_BYTES: usize = TIER0_INTERNAL_NODES * 32 + TIER1_ROWS * 64; // 49,120

/// Number of siblings extracted from Tier 0.
const TIER0_LAYERS_COUNT: usize = TIER0_LAYERS; // 9

/// Parsed Tier 0 data: internal node hashes and subtree records at depth TIER0_LAYERS.
pub struct Tier0Data {
    data: Vec<u8>,
}

impl Tier0Data {
    pub fn from_bytes(data: Vec<u8>) -> anyhow::Result<Self> {
        anyhow::ensure!(
            data.len() == TIER0_BYTES,
            "Tier 0 data size mismatch: got {} bytes, expected {}",
            data.len(),
            TIER0_BYTES
        );
        validate_all_fp_chunks(&data, "Tier 0")?;
        Ok(Self { data })
    }

    /// Root hash (depth 0).
    pub fn root(&self) -> Fp {
        read_fp(&self.data[0..32])
    }

    /// Internal node hash at the given top-down depth and index.
    pub fn node_at(&self, depth: usize, index: usize) -> Fp {
        debug_assert!(depth < TIER0_LAYERS);
        debug_assert!(index < (1 << depth));
        let bfs_pos = (1usize << depth) - 1 + index;
        let offset = bfs_pos * 32;
        read_fp(&self.data[offset..offset + 32])
    }

    /// Number of subtree records.
    pub fn num_subtrees(&self) -> usize {
        TIER1_ROWS
    }

    /// Subtree record at depth TIER0_LAYERS: (hash, min_key).
    pub fn subtree_record(&self, index: usize) -> (Fp, Fp) {
        debug_assert!(index < TIER1_ROWS);
        let base = TIER0_INTERNAL_NODES * 32 + index * 64;
        let hash = read_fp(&self.data[base..base + 32]);
        let min_key = read_fp(&self.data[base + 32..base + 64]);
        (hash, min_key)
    }

    /// Binary search the subtree min_keys to find which subtree contains `value`.
    pub fn find_subtree(&self, value: Fp) -> Option<usize> {
        let base = TIER0_INTERNAL_NODES * 32;
        binary_search_records(&self.data, base, TIER1_ROWS, 64, 32, value)
    }

    /// Extract sibling hashes from Tier 0 for a given subtree index.
    pub fn extract_siblings(&self, subtree_idx: usize) -> [Fp; TIER0_LAYERS_COUNT] {
        let mut siblings = [Fp::default(); TIER0_LAYERS_COUNT];

        let sibling = subtree_idx ^ 1;
        let (hash, _) = self.subtree_record(sibling);
        siblings[0] = hash;

        let mut pos = subtree_idx;
        for d in (1..TIER0_LAYERS).rev() {
            pos >>= 1;
            let sibling_pos = pos ^ 1;
            siblings[TIER0_LAYERS_COUNT - d] = self.node_at(d, sibling_pos);
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
    }

    #[test]
    fn node_at_returns_root_at_depth_zero() {
        let data = vec![0u8; TIER0_BYTES];
        let tier0 = Tier0Data::from_bytes(data).expect("valid");
        assert_eq!(tier0.node_at(0, 0), tier0.root());
    }

    #[test]
    fn find_subtree_on_all_zeros() {
        let data = vec![0u8; TIER0_BYTES];
        let tier0 = Tier0Data::from_bytes(data).expect("valid");
        let result = tier0.find_subtree(Fp::from(42u64));
        assert!(result.is_some());
        assert!(result.unwrap() < TIER1_ROWS);
    }

    #[test]
    fn extract_siblings_returns_correct_count() {
        let data = vec![0u8; TIER0_BYTES];
        let tier0 = Tier0Data::from_bytes(data).expect("valid");
        let siblings = tier0.extract_siblings(0);
        assert_eq!(siblings.len(), TIER0_LAYERS);
    }

    #[test]
    fn subtree_record_round_trip() {
        let data = vec![0u8; TIER0_BYTES];
        let tier0 = Tier0Data::from_bytes(data).expect("valid");
        let (hash, min_key) = tier0.subtree_record(0);
        assert_eq!(hash, Fp::zero());
        assert_eq!(min_key, Fp::zero());
    }
}
