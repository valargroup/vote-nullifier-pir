//! Tier 1 reader: parse and query a single Tier 1 row (punctured-range leaves, K=2).
//!
//! Each row contains leaf records only (no pre-computed internal nodes). The
//! client rebuilds the subtree locally to extract siblings. Row width follows
//! the negotiated [`crate::PirLayout`].

use pasta_curves::Fp;

use crate::fp_utils::{binary_search_records, read_fp, validate_all_fp_chunks};
use crate::{PirLayout, COMPILED_PIR_LAYOUT, TIER1_LEAF_BYTES};

/// Parsed Tier 1 row: punctured-range leaf records for a runtime layer count.
pub struct Tier1Row<'a> {
    data: &'a [u8],
    leaves: usize,
    layers: usize,
}

impl<'a> Tier1Row<'a> {
    /// Parse a default-layout Tier 1 row.
    pub fn from_bytes(data: &'a [u8]) -> anyhow::Result<Self> {
        Self::from_layout(data, COMPILED_PIR_LAYOUT)
    }

    /// Parse a Tier 1 row for a negotiated two-tier layout.
    pub fn from_layout(data: &'a [u8], layout: PirLayout) -> anyhow::Result<Self> {
        layout
            .validate_split()
            .map_err(anyhow::Error::msg)?;
        let layers = layout.tier1_layers;
        let leaves = layout.tier1_leaves().map_err(anyhow::Error::msg)?;
        let expected = layout.tier1_row_bytes().map_err(anyhow::Error::msg)?;
        anyhow::ensure!(
            data.len() == expected,
            "Tier 1 row size mismatch: got {} bytes, expected {expected}",
            data.len()
        );
        validate_all_fp_chunks(data, "Tier 1 row")?;
        Ok(Self {
            data,
            leaves,
            layers,
        })
    }

    /// Number of Tier 1 layers encoded in this row.
    pub fn layers(&self) -> usize {
        self.layers
    }

    /// Number of leaf slots in this row.
    pub fn leaves(&self) -> usize {
        self.leaves
    }

    /// Leaf record at index i: `(nf_lo, nf_mid, nf_hi)` — the three boundary
    /// nullifiers of a punctured range.
    pub fn leaf_record(&self, i: usize) -> (Fp, Fp, Fp) {
        debug_assert!(i < self.leaves);
        let base = i * TIER1_LEAF_BYTES;
        let nf_lo = read_fp(&self.data[base..base + 32]);
        let nf_mid = read_fp(&self.data[base + 32..base + 64]);
        let nf_hi = read_fp(&self.data[base + 64..base + 96]);
        (nf_lo, nf_mid, nf_hi)
    }

    /// Find the leaf whose punctured range contains `value`.
    ///
    /// Binary-searches on `nf_lo`, then checks `nf_lo < value < nf_hi` and
    /// `value != nf_mid`.
    pub fn find_leaf(&self, value: Fp, valid_leaves: usize) -> Option<usize> {
        debug_assert!(valid_leaves <= self.leaves);
        if valid_leaves == 0 {
            return None;
        }
        let idx = binary_search_records(self.data, 0, valid_leaves, TIER1_LEAF_BYTES, 0, value)?;

        let (nf_lo, nf_mid, nf_hi) = self.leaf_record(idx);
        let offset = value - nf_lo;
        let span = nf_hi - nf_lo;
        if offset == Fp::zero() || offset >= span || value == nf_mid {
            return None;
        }
        Some(idx)
    }

    /// Rebuild the subtree from leaf data and extract sibling hashes.
    ///
    /// The client builds the tree bottom-up from the leaf hashes to collect
    /// the siblings needed for the Merkle authentication path.
    pub fn extract_siblings(
        &self,
        leaf_idx: usize,
        valid_leaves: usize,
        hasher: &imt_tree::hasher::PoseidonHasher,
    ) -> Vec<Fp> {
        debug_assert!(valid_leaves <= self.leaves);

        let empty_leaf = hasher.hash3(Fp::zero(), Fp::zero(), Fp::zero());
        let mut current_level: Vec<Fp> = (0..self.leaves)
            .map(|i| {
                if i < valid_leaves {
                    let (lo, mid, hi) = self.leaf_record(i);
                    hasher.hash3(lo, mid, hi)
                } else {
                    empty_leaf
                }
            })
            .collect();

        let mut siblings = Vec::with_capacity(self.layers);
        let mut pos = leaf_idx;
        for level in 0..self.layers {
            siblings.push(current_level[pos ^ 1]);
            if level < self.layers - 1 {
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
    use crate::fp_utils::write_fp;
    use crate::{TIER1_LAYERS, TIER1_ROW_BYTES};
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
    fn extract_siblings_returns_correct_count() {
        let row = vec![0u8; TIER1_ROW_BYTES];
        let tier1 = Tier1Row::from_bytes(&row).unwrap();
        let hasher = PoseidonHasher::new();
        let siblings = tier1.extract_siblings(0, 0, &hasher);
        assert_eq!(siblings.len(), TIER1_LAYERS);
    }

    #[test]
    fn punctured_leaf_record_round_trip() {
        let mut row = vec![0u8; TIER1_ROW_BYTES];
        let hasher = PoseidonHasher::new();

        write_fp(&mut row[0..], Fp::from(10u64));
        write_fp(&mut row[32..], Fp::from(20u64));
        write_fp(&mut row[64..], Fp::from(30u64));

        let tier1 = Tier1Row::from_bytes(&row).unwrap();
        let (nf_lo, nf_mid, nf_hi) = tier1.leaf_record(0);
        assert_eq!(nf_lo, Fp::from(10u64));
        assert_eq!(nf_mid, Fp::from(20u64));
        assert_eq!(nf_hi, Fp::from(30u64));
        assert!(tier1.find_leaf(Fp::from(15u64), 1).is_some());
        assert!(tier1.find_leaf(Fp::from(25u64), 1).is_some());
        assert!(tier1.find_leaf(Fp::from(10u64), 1).is_none());
        assert!(tier1.find_leaf(Fp::from(20u64), 1).is_none());
        assert!(tier1.find_leaf(Fp::from(30u64), 1).is_none());

        let siblings = tier1.extract_siblings(0, 1, &hasher);
        assert_eq!(
            siblings[0],
            hasher.hash3(Fp::zero(), Fp::zero(), Fp::zero())
        );
    }

    #[test]
    fn from_layout_accepts_thirteen_six() {
        let layout = PirLayout {
            pir_depth: 19,
            tier0_layers: 13,
            tier1_layers: 6,
        };
        let row_bytes = layout.tier1_row_bytes().unwrap();
        let row = vec![0u8; row_bytes];
        let tier1 = Tier1Row::from_layout(&row, layout).unwrap();
        assert_eq!(tier1.layers(), 6);
        assert_eq!(tier1.leaves(), 1 << 6);
        let hasher = PoseidonHasher::new();
        assert_eq!(tier1.extract_siblings(0, 0, &hasher).len(), 6);
    }
}
