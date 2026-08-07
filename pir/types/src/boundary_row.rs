//! Boundary-index row for an intermediate encrypted tier: `(subtree_hash,
//! min_key)` records with a runtime layer count.

use pasta_curves::Fp;

use crate::fp_utils::{read_fp, validate_all_fp_chunks};
use crate::BOUNDARY_RECORD_BYTES;

/// Parsed boundary-index row with a runtime layer count.
pub struct BoundaryRow<'a> {
    data: &'a [u8],
    records: usize,
    layers: usize,
}

impl<'a> BoundaryRow<'a> {
    pub fn from_bytes(data: &'a [u8], layers: usize) -> anyhow::Result<Self> {
        anyhow::ensure!(
            layers > 0 && layers <= 32,
            "invalid boundary layers {layers}"
        );
        let records = 1usize << layers;
        let expected = records * BOUNDARY_RECORD_BYTES;
        anyhow::ensure!(
            data.len() == expected,
            "boundary row size mismatch: got {} bytes, expected {expected} for {layers} layers",
            data.len()
        );
        validate_all_fp_chunks(data, "boundary row")?;
        Ok(Self {
            data,
            records,
            layers,
        })
    }

    pub fn layers(&self) -> usize {
        self.layers
    }

    pub fn records(&self) -> usize {
        self.records
    }

    /// Record at index i: `(subtree_hash, min_key)`.
    pub fn record(&self, i: usize) -> (Fp, Fp) {
        debug_assert!(i < self.records);
        let base = i * BOUNDARY_RECORD_BYTES;
        let hash = read_fp(&self.data[base..base + 32]);
        let min_key = read_fp(&self.data[base + 32..base + 64]);
        (hash, min_key)
    }

    /// Select the child whose key range contains `value`: the largest index
    /// with `min_key <= value`, or 0 when every `min_key` exceeds `value`.
    ///
    /// Total (never fails) and fixed-work (always scans every record): the
    /// row bytes are server-supplied, so neither a crafted row nor the scan
    /// duration may influence whether or when the follow-up tier query is
    /// sent.
    pub fn find_child(&self, value: Fp) -> usize {
        let mut child = 0usize;
        for i in 0..self.records {
            let base = i * BOUNDARY_RECORD_BYTES;
            let min_key = read_fp(&self.data[base + 32..base + 64]);
            if min_key <= value {
                child = i;
            }
        }
        child
    }

    /// Rebuild the subtree from child hashes and extract sibling hashes.
    pub fn extract_siblings(
        &self,
        child_idx: usize,
        hasher: &imt_tree::hasher::PoseidonHasher,
    ) -> Vec<Fp> {
        let mut current_level: Vec<Fp> = (0..self.records)
            .map(|i| {
                let (hash, _) = self.record(i);
                hash
            })
            .collect();

        let mut siblings = Vec::with_capacity(self.layers);
        let mut pos = child_idx;
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
    fn rejects_wrong_size() {
        assert!(BoundaryRow::from_bytes(&[0u8; 10], 3).is_err());
    }

    #[test]
    fn find_child_is_total_on_zero_rows() {
        let layers = 3usize;
        let data = vec![0u8; (1 << layers) * BOUNDARY_RECORD_BYTES];
        let row = BoundaryRow::from_bytes(&data, layers).unwrap();
        // All min_keys are 0 <= value, so the largest index wins.
        assert_eq!(row.find_child(Fp::from(1u64)), row.records() - 1);
        let hasher = PoseidonHasher::new();
        assert_eq!(row.extract_siblings(0, &hasher).len(), layers);
    }

    #[test]
    fn find_child_selects_last_min_key_at_or_below_value() {
        let layers = 2usize;
        let mut data = vec![0u8; (1 << layers) * BOUNDARY_RECORD_BYTES];
        for (i, min_key) in [0u64, 10, 20, 30].into_iter().enumerate() {
            write_fp(
                &mut data[i * BOUNDARY_RECORD_BYTES + 32..],
                Fp::from(min_key),
            );
        }
        let row = BoundaryRow::from_bytes(&data, layers).unwrap();
        assert_eq!(row.find_child(Fp::from(5u64)), 0);
        assert_eq!(row.find_child(Fp::from(10u64)), 1);
        assert_eq!(row.find_child(Fp::from(25u64)), 2);
        assert_eq!(row.find_child(Fp::from(99u64)), 3);
    }

    #[test]
    fn find_child_is_total_on_non_monotone_rows() {
        // A hostile row with descending min_keys must still yield an index.
        let layers = 2usize;
        let mut data = vec![0u8; (1 << layers) * BOUNDARY_RECORD_BYTES];
        for (i, min_key) in [30u64, 20, 10, 0].into_iter().enumerate() {
            write_fp(
                &mut data[i * BOUNDARY_RECORD_BYTES + 32..],
                Fp::from(min_key),
            );
        }
        let row = BoundaryRow::from_bytes(&data, layers).unwrap();
        let child = row.find_child(Fp::from(15u64));
        assert!(child < row.records());
    }
}
