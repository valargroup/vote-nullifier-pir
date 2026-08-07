//! Generic punctured-range (K=2) terminal row.

use pasta_curves::Fp;

use crate::fp_utils::{binary_search_records, read_fp, validate_all_fp_chunks};
use crate::TIER1_LEAF_BYTES;

/// Parsed terminal punctured-range row with a runtime layer count.
pub struct PuncturedRangeRow<'a> {
    data: &'a [u8],
    leaves: usize,
    layers: usize,
}

impl<'a> PuncturedRangeRow<'a> {
    pub fn from_bytes(data: &'a [u8], layers: usize) -> anyhow::Result<Self> {
        anyhow::ensure!(
            layers > 0 && layers <= 32,
            "invalid punctured layers {layers}"
        );
        let leaves = 1usize << layers;
        let expected = leaves * TIER1_LEAF_BYTES;
        anyhow::ensure!(
            data.len() == expected,
            "punctured row size mismatch: got {} bytes, expected {expected} for {layers} layers",
            data.len()
        );
        validate_all_fp_chunks(data, "punctured-range row")?;
        Ok(Self {
            data,
            leaves,
            layers,
        })
    }

    pub fn layers(&self) -> usize {
        self.layers
    }

    pub fn leaves(&self) -> usize {
        self.leaves
    }

    pub fn leaf_record(&self, i: usize) -> (Fp, Fp, Fp) {
        debug_assert!(i < self.leaves);
        let base = i * TIER1_LEAF_BYTES;
        let nf_lo = read_fp(&self.data[base..base + 32]);
        let nf_mid = read_fp(&self.data[base + 32..base + 64]);
        let nf_hi = read_fp(&self.data[base + 64..base + 96]);
        (nf_lo, nf_mid, nf_hi)
    }

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
    use imt_tree::hasher::PoseidonHasher;

    #[test]
    fn find_leaf_on_single_record() {
        let layers = 2usize;
        let mut row = vec![0u8; (1 << layers) * TIER1_LEAF_BYTES];
        write_fp(&mut row[0..], Fp::from(10u64));
        write_fp(&mut row[32..], Fp::from(20u64));
        write_fp(&mut row[64..], Fp::from(30u64));
        let parsed = PuncturedRangeRow::from_bytes(&row, layers).unwrap();
        assert_eq!(parsed.find_leaf(Fp::from(15u64), 1), Some(0));
        assert!(parsed.find_leaf(Fp::from(20u64), 1).is_none());
        let hasher = PoseidonHasher::new();
        assert_eq!(parsed.extract_siblings(0, 1, &hasher).len(), layers);
    }
}
