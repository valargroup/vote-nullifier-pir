//! Tier 2 reader: parse and query a single Tier 2 row (punctured-range leaves, K=2).

use pasta_curves::Fp;

use crate::fp_utils::{binary_search_records, read_fp, validate_all_fp_chunks};
use crate::{TIER2_INTERNAL_NODES, TIER2_LAYERS, TIER2_LEAF_BYTES, TIER2_LEAVES, TIER2_ROW_BYTES};

/// Parsed Tier 2 row: internal nodes (relative depths 1-6) and 128 punctured-range
/// leaf records at relative depth 7.
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

    /// Internal node at relative depth d (1..TIER2_LAYERS-1), position p (0..2^d - 1).
    pub fn internal_node(&self, rel_depth: usize, pos: usize) -> Fp {
        debug_assert!((1..TIER2_LAYERS).contains(&rel_depth));
        debug_assert!(pos < (1 << rel_depth));
        let bfs_idx = (1usize << rel_depth) - 2 + pos;
        let offset = bfs_idx * 32;
        read_fp(&self.data[offset..offset + 32])
    }

    /// Leaf record at index i: `(nf_lo, nf_mid, nf_hi)` — the three boundary
    /// nullifiers of a punctured range.
    pub fn leaf_record(&self, i: usize) -> (Fp, Fp, Fp) {
        debug_assert!(i < TIER2_LEAVES);
        let base = TIER2_INTERNAL_NODES * 32 + i * TIER2_LEAF_BYTES;
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
        let base = TIER2_INTERNAL_NODES * 32;
        let idx = binary_search_records(self.data, base, valid_leaves, TIER2_LEAF_BYTES, 0, value)?;

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

    /// Extract the sibling hashes from this Tier 2 row for a given leaf index.
    ///
    /// The sibling at the leaf level (bottom-up 0) is computed by hashing the
    /// sibling's three boundary nullifiers with `hash3`. Upper siblings are
    /// read from the pre-computed internal nodes.
    pub fn extract_siblings(
        &self,
        leaf_idx: usize,
        valid_leaves: usize,
        hasher: &imt_tree::hasher::PoseidonHasher,
    ) -> [Fp; TIER2_LAYERS] {
        debug_assert!(valid_leaves <= TIER2_LEAVES);
        let mut siblings = [Fp::default(); TIER2_LAYERS];

        let sibling_leaf_idx = leaf_idx ^ 1;
        siblings[0] = if sibling_leaf_idx < valid_leaves {
            let (nf_lo, nf_mid, nf_hi) = self.leaf_record(sibling_leaf_idx);
            hasher.hash3(nf_lo, nf_mid, nf_hi)
        } else {
            hasher.hash3(Fp::zero(), Fp::zero(), Fp::zero())
        };

        let mut pos = leaf_idx;
        for rd in (1..TIER2_LAYERS).rev() {
            pos >>= 1;
            let sibling_pos = pos ^ 1;
            siblings[TIER2_LAYERS - rd] = self.internal_node(rd, sibling_pos);
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
        let base = TIER2_INTERNAL_NODES * 32;
        let hasher = PoseidonHasher::new();

        // Write a punctured range [10, 20, 30] at leaf 0
        write_fp(&mut row[base..], Fp::from(10u64));
        write_fp(&mut row[base + 32..], Fp::from(20u64));
        write_fp(&mut row[base + 64..], Fp::from(30u64));

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

        // Sibling hash uses hash3
        let sibs = tier2.extract_siblings(0, 1, &hasher);
        assert_eq!(sibs[0], hasher.hash3(Fp::zero(), Fp::zero(), Fp::zero()));
    }
}
