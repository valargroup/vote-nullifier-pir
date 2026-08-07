//! Generic boundary-index row: `(subtree_hash, min_key)` records.

use pasta_curves::Fp;

use crate::fp_utils::{binary_search_records, read_fp, validate_all_fp_chunks};

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
        let expected = records * 64;
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

    pub fn record(&self, i: usize) -> (Fp, Fp) {
        debug_assert!(i < self.records);
        let base = i * 64;
        let hash = read_fp(&self.data[base..base + 32]);
        let min_key = read_fp(&self.data[base + 32..base + 64]);
        (hash, min_key)
    }

    /// Binary-search `min_key` fields for the child containing `value`.
    pub fn find_child(&self, value: Fp) -> Option<usize> {
        binary_search_records(self.data, 0, self.records, 64, 32, value)
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
    use imt_tree::hasher::PoseidonHasher;

    #[test]
    fn rejects_wrong_size() {
        assert!(BoundaryRow::from_bytes(&[0u8; 10], 3).is_err());
    }

    #[test]
    fn zeros_round_trip() {
        let layers = 3usize;
        let data = vec![0u8; (1 << layers) * 64];
        let row = BoundaryRow::from_bytes(&data, layers).unwrap();
        let found = row.find_child(Fp::from(1u64));
        assert!(found.is_some());
        assert!(found.unwrap() < row.records());
        let hasher = PoseidonHasher::new();
        assert_eq!(row.extract_siblings(0, &hasher).len(), layers);
    }
}
