//! Tier 2 reader: parse and query a single Tier 2 row (punctured-range leaves, K=2).
//!
//! Each row contains TIER2_LEAVES leaf records only (no pre-computed internal
//! nodes). The client rebuilds the subtree locally to extract siblings.

use pasta_curves::Fp;

use crate::fp_utils::{binary_search_records, read_fp, validate_all_fp_chunks};
use crate::{TIER2_LAYERS, TIER2_LEAF_BYTES, TIER2_LEAVES, TIER2_ROW_BYTES};

/// Parsed Tier 2 row: TIER2_LEAVES punctured-range leaf records.
pub struct Tier2Row<'a> {
    data: &'a [u8],
}

impl<'a> Tier2Row<'a> {
    pub fn from_bytes(data: &'a [u8]) -> anyhow::Result<Self> {
        anyhow::ensure!(
            data.len() == TIER2_ROW_BYTES,
            "Tier 2 row size mismatch: got {} bytes, expected {}",
            data.len(),
            TIER2_ROW_BYTES
        );
        validate_all_fp_chunks(data, "Tier 2 row")?;
        Ok(Self { data })
    }

    /// Leaf record at index i: `(nf_lo, nf_mid, nf_hi)` — the three boundary
    /// nullifiers of a punctured range.
    pub fn leaf_record(&self, i: usize) -> (Fp, Fp, Fp) {
        debug_assert!(i < TIER2_LEAVES);
        let base = i * TIER2_LEAF_BYTES;
        let nf_lo = read_fp(&self.data[base..base + 32]);
        let nf_mid = read_fp(&self.data[base + 32..base + 64]);
        let nf_hi = read_fp(&self.data[base + 64..base + 96]);
        (nf_lo, nf_mid, nf_hi)
    }

    /// Find the leaf whose punctured range contains `value`.
    ///
    /// Binary-searches on `nf_lo` (the first field element of each 96-byte record),
    /// then checks `nf_lo < value < nf_hi` and `value != nf_mid`.
    pub fn find_leaf(&self, value: Fp, valid_leaves: usize) -> Option<usize> {
        debug_assert!(valid_leaves <= TIER2_LEAVES);
        if valid_leaves == 0 {
            return None;
        }
        let idx = binary_search_records(self.data, 0, valid_leaves, TIER2_LEAF_BYTES, 0, value)?;

        let (nf_lo, nf_mid, nf_hi) = self.leaf_record(idx);
        let offset = value - nf_lo;
        let span = nf_hi - nf_lo;
        if offset == Fp::zero() || offset >= span {
            return None;
        }
        if value == nf_mid {
            return None;
        }
        Some(idx)
    }

    /// Rebuild the subtree from leaf data and extract sibling hashes.
    ///
    /// The client hashes all leaf records and builds the tree bottom-up to
    /// collect the TIER2_LAYERS siblings needed for the Merkle authentication path.
    pub fn extract_siblings(
        &self,
        leaf_idx: usize,
        valid_leaves: usize,
        hasher: &imt_tree::hasher::PoseidonHasher,
    ) -> [Fp; TIER2_LAYERS] {
        debug_assert!(valid_leaves <= TIER2_LEAVES);

        let empty_leaf = hasher.hash3(Fp::zero(), Fp::zero(), Fp::zero());

        let mut current_level: Vec<Fp> = (0..TIER2_LEAVES)
            .map(|i| {
                if i < valid_leaves {
                    let (lo, mid, hi) = self.leaf_record(i);
                    hasher.hash3(lo, mid, hi)
                } else {
                    empty_leaf
                }
            })
            .collect();

        let mut siblings = [Fp::default(); TIER2_LAYERS];
        let mut pos = leaf_idx;
        for level in 0..TIER2_LAYERS {
            siblings[level] = current_level[pos ^ 1];
            let next_len = current_level.len() / 2;
            let mut next_level = Vec::with_capacity(next_len);
            for j in 0..next_len {
                next_level.push(hasher.hash(current_level[2 * j], current_level[2 * j + 1]));
            }
            current_level = next_level;
            pos >>= 1;
        }
        siblings
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fp_utils::write_fp;
    use imt_tree::hasher::PoseidonHasher;

    #[test]
    fn from_bytes_rejects_non_canonical_field_element() {
        let mut row = vec![0u8; TIER2_ROW_BYTES];
        row[0..32].fill(0xFF);
        let err = Tier2Row::from_bytes(&row)
            .err()
            .expect("row should be rejected");
        assert!(
            err.to_string().contains("invalid field element"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn punctured_leaf_record_round_trip() {
        let mut row = vec![0u8; TIER2_ROW_BYTES];
        let hasher = PoseidonHasher::new();

        // Write a punctured range [10, 20, 30] at leaf 0
        write_fp(&mut row[0..], Fp::from(10u64));
        write_fp(&mut row[32..], Fp::from(20u64));
        write_fp(&mut row[64..], Fp::from(30u64));

        let tier2 = Tier2Row::from_bytes(&row).unwrap();
        let (nf_lo, nf_mid, nf_hi) = tier2.leaf_record(0);
        assert_eq!(nf_lo, Fp::from(10u64));
        assert_eq!(nf_mid, Fp::from(20u64));
        assert_eq!(nf_hi, Fp::from(30u64));

        // Find a value in the punctured range
        assert!(tier2.find_leaf(Fp::from(15u64), 1).is_some());
        assert!(tier2.find_leaf(Fp::from(25u64), 1).is_some());
        // Boundaries and interior nullifier are rejected
        assert!(tier2.find_leaf(Fp::from(10u64), 1).is_none());
        assert!(tier2.find_leaf(Fp::from(20u64), 1).is_none());
        assert!(tier2.find_leaf(Fp::from(30u64), 1).is_none());

        // Sibling at leaf level: empty because leaf 1 is padding
        let sibs = tier2.extract_siblings(0, 1, &hasher);
        assert_eq!(sibs[0], hasher.hash3(Fp::zero(), Fp::zero(), Fp::zero()));
    }
}
