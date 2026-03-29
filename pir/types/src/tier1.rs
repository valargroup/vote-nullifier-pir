//! Tier 1 reader: parse and query a single Tier 1 row.
//!
//! Each row contains TIER1_LEAVES leaf records only (no pre-computed internal
//! nodes). The client rebuilds the subtree locally to extract siblings.

use pasta_curves::Fp;

use crate::fp_utils::{binary_search_records, read_fp, validate_all_fp_chunks};
use crate::{TIER1_LAYERS, TIER1_LEAVES, TIER1_ROW_BYTES};

/// Parsed Tier 1 row: TIER1_LEAVES leaf records.
pub struct Tier1Row<'a> {
    data: &'a [u8],
}

impl<'a> Tier1Row<'a> {
    pub fn from_bytes(data: &'a [u8]) -> anyhow::Result<Self> {
        anyhow::ensure!(
            data.len() == TIER1_ROW_BYTES,
            "Tier 1 row size mismatch: got {} bytes, expected {}",
            data.len(),
            TIER1_ROW_BYTES
        );
        validate_all_fp_chunks(data, "Tier 1 row")?;
        Ok(Self { data })
    }

    /// Leaf record at index i: (hash, min_key).
    pub fn leaf_record(&self, i: usize) -> (Fp, Fp) {
        debug_assert!(i < TIER1_LEAVES);
        let base = i * 64;
        let hash = read_fp(&self.data[base..base + 32]);
        let min_key = read_fp(&self.data[base + 32..base + 64]);
        (hash, min_key)
    }

    /// Binary search the leaf min_keys to find which sub-subtree contains `value`.
    pub fn find_sub_subtree(&self, value: Fp) -> Option<usize> {
        binary_search_records(self.data, 0, TIER1_LEAVES, 64, 32, value)
    }

    /// Rebuild the subtree from leaf hashes and extract sibling hashes.
    ///
    /// The client builds the tree bottom-up from the leaf hashes to collect
    /// the TIER1_LAYERS siblings needed for the Merkle authentication path.
    pub fn extract_siblings(
        &self,
        sub_idx: usize,
        hasher: &imt_tree::hasher::PoseidonHasher,
    ) -> [Fp; TIER1_LAYERS] {
        let mut current_level: Vec<Fp> = (0..TIER1_LEAVES)
            .map(|i| {
                let (hash, _) = self.leaf_record(i);
                hash
            })
            .collect();

        let mut siblings = [Fp::default(); TIER1_LAYERS];
        let mut pos = sub_idx;
        for level in 0..TIER1_LAYERS {
            siblings[level] = current_level[pos ^ 1];
            if level < TIER1_LAYERS - 1 {
                let next_len = current_level.len() / 2;
                let mut next_level = Vec::with_capacity(next_len);
                for j in 0..next_len {
                    next_level.push(hasher.hash(current_level[2 * j], current_level[2 * j + 1]));
                }
                current_level = next_level;
            }
            pos >>= 1;
        }
        siblings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imt_tree::hasher::PoseidonHasher;

    #[test]
    fn from_bytes_rejects_non_canonical_field_element() {
        let mut row = vec![0u8; TIER1_ROW_BYTES];
        row[0..32].fill(0xFF);
        let err = Tier1Row::from_bytes(&row)
            .err()
            .expect("row should be rejected");
        assert!(
            err.to_string().contains("invalid field element"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn from_bytes_rejects_wrong_size() {
        let short = vec![0u8; TIER1_ROW_BYTES - 1];
        assert!(Tier1Row::from_bytes(&short).is_err());
    }

    #[test]
    fn find_sub_subtree_on_all_zeros() {
        let row = vec![0u8; TIER1_ROW_BYTES];
        let tier1 = Tier1Row::from_bytes(&row).unwrap();
        let result = tier1.find_sub_subtree(Fp::from(42u64));
        assert!(result.is_some());
        assert!(result.unwrap() < TIER1_LEAVES);
    }

    #[test]
    fn extract_siblings_returns_correct_count() {
        let row = vec![0u8; TIER1_ROW_BYTES];
        let tier1 = Tier1Row::from_bytes(&row).unwrap();
        let hasher = PoseidonHasher::new();
        let siblings = tier1.extract_siblings(0, &hasher);
        assert_eq!(siblings.len(), TIER1_LAYERS);
    }

    #[test]
    fn leaf_record_round_trip_on_zeros() {
        let row = vec![0u8; TIER1_ROW_BYTES];
        let tier1 = Tier1Row::from_bytes(&row).unwrap();
        let (hash, min_key) = tier1.leaf_record(0);
        assert_eq!(hash, Fp::zero());
        assert_eq!(min_key, Fp::zero());
    }

    #[test]
    fn extract_siblings_correctness() {
        use crate::fp_utils::write_fp;

        let hasher = PoseidonHasher::new();
        let mut row = vec![0u8; TIER1_ROW_BYTES];

        // Write distinct hash values into the first 4 leaf records
        // (min_key fields are irrelevant for sibling extraction).
        let leaf_hashes: Vec<Fp> = (0..TIER1_LEAVES)
            .map(|i| Fp::from((i + 1) as u64))
            .collect();
        for (i, &h) in leaf_hashes.iter().enumerate() {
            write_fp(&mut row[i * 64..], h);
        }

        let tier1 = Tier1Row::from_bytes(&row).unwrap();
        let siblings = tier1.extract_siblings(0, &hasher);

        // Level 0 sibling: leaf at index 0 ^ 1 = 1
        assert_eq!(siblings[0], leaf_hashes[1]);

        // Level 1 sibling: parent of leaves 2,3
        let expected_level1_sib = hasher.hash(leaf_hashes[2], leaf_hashes[3]);
        assert_eq!(siblings[1], expected_level1_sib);

        // Verify all siblings by building the tree independently
        let mut level = leaf_hashes.clone();
        let mut pos = 0usize;
        for lev in 0..TIER1_LAYERS {
            assert_eq!(siblings[lev], level[pos ^ 1]);
            let next: Vec<Fp> = level
                .chunks_exact(2)
                .map(|pair| hasher.hash(pair[0], pair[1]))
                .collect();
            level = next;
            pos >>= 1;
        }
    }
}
