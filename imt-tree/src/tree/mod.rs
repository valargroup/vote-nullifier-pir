use ff::Field as _;
use pasta_curves::Fp;
use rayon::prelude::*;

pub(crate) use crate::hasher::PoseidonHasher;

#[cfg(test)]
mod tests;

/// Depth of the nullifier Merkle tree.
///
/// Each on-chain nullifier produces approximately one gap; with K=2 punctured
/// ranges, ~n/2 leaves are needed for n nullifiers. Zcash mainnet currently
/// has under 64M Orchard nullifiers. We plan for this circuit to support up
/// to 256M nullifiers, so the tree needs capacity for ~2^28 leaves:
/// `log2(256 << 20) + 1 = 29`.
pub const TREE_DEPTH: usize = 29;

/// A punctured range `[nf_lo, nf_mid, nf_hi]` representing the interval
/// `(nf_lo, nf_hi) \ {nf_mid}` — two adjacent gaps joined by excluding the
/// nullifier between them.
///
/// With K=2, each leaf stores three sorted nullifier boundaries. The leaf
/// commitment is `Poseidon3(nf_lo, nf_mid, nf_hi)`.
pub type PuncturedRange = [Fp; 3];

/// Build punctured ranges (K=2) from a sorted, deduplicated nullifier list.
///
/// Groups consecutive nullifiers into overlapping triples:
///   `[nf_0, nf_1, nf_2]`, `[nf_2, nf_3, nf_4]`, `[nf_4, nf_5, nf_6]`, ...
///
/// Each triple covers the punctured interval `(nf_lo, nf_hi) \ {nf_mid}`.
/// Consecutive triples share boundary nullifiers, so every gap between
/// adjacent nullifiers is covered by exactly one leaf.
///
/// # Panics
///
/// Panics if `sorted_nfs` has fewer than 3 elements or an even length
/// (which would leave a trailing gap without a matching triple — callers
/// should ensure an odd count via sentinel injection).
pub fn build_punctured_ranges(sorted_nfs: &[Fp]) -> Vec<PuncturedRange> {
    let n = sorted_nfs.len();
    assert!(
        n >= 3,
        "need at least 3 sorted nullifiers for K=2 punctured ranges, got {n}"
    );
    assert!(
        n % 2 == 1,
        "sorted nullifier count must be odd for K=2 (got {n}); \
         inject an additional sentinel to fix"
    );

    let num_leaves = (n - 1) / 2;
    (0..num_leaves)
        .map(|i| {
            let base = i * 2;
            [sorted_nfs[base], sorted_nfs[base + 1], sorted_nfs[base + 2]]
        })
        .collect()
}

/// Hash each punctured range triple into a single leaf commitment.
pub fn commit_punctured_ranges(ranges: &[PuncturedRange]) -> Vec<Fp> {
    ranges
        .par_iter()
        .map_init(PoseidonHasher::new, |hasher, &[a, b, c]| {
            hasher.hash3(a, b, c)
        })
        .collect()
}

/// Find the punctured-range index that contains `value`.
///
/// Returns `Some(i)` where `ranges[i]` is `[nf_lo, nf_mid, nf_hi]` and
/// `nf_lo < value < nf_hi` and `value != nf_mid`. Returns `None` if the
/// value is an existing nullifier.
pub fn find_punctured_range_for_value(ranges: &[PuncturedRange], value: Fp) -> Option<usize> {
    let i = ranges.partition_point(|[nf_lo, _, _]| *nf_lo < value);
    if i == 0 {
        return None;
    }
    let idx = i - 1;
    let [nf_lo, nf_mid, nf_hi] = ranges[idx];
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

/// Verify that every punctured range has outer span `≤ 2^251`.
///
/// For K=2, the outer span `nf_hi - nf_lo` can be up to twice a single gap
/// width. With the standard sentinel spacing of `2^250`, spans between
/// consecutive sentinels are exactly `2^251`. This function checks that no
/// span exceeds that bound.
///
/// A tighter bound (`≤ 2^250`) can be achieved by doubling sentinel density
/// (spacing `2^249`). See `pir-export` for sentinel injection.
pub fn verify_punctured_range_spans(ranges: &[PuncturedRange]) -> anyhow::Result<()> {
    let max_span = Fp::from(2u64).pow([251, 0, 0, 0]);
    for (i, &[nf_lo, _, nf_hi]) in ranges.iter().enumerate() {
        let span = nf_hi - nf_lo;
        anyhow::ensure!(
            span <= max_span,
            "punctured range {i} has span > 2^251: nf_lo={nf_lo:?}, nf_hi={nf_hi:?}"
        );
    }
    Ok(())
}

/// Pre-compute the empty subtree hash at each tree level.
///
/// `empty[0] = hash3(0, 0, 0)` -- the commitment of an all-zero punctured range.
/// `empty[i] = hash(empty[i-1], empty[i-1])` for higher levels.
pub fn precompute_empty_hashes() -> [Fp; TREE_DEPTH] {
    let hasher = PoseidonHasher::new();
    let mut empty = [Fp::default(); TREE_DEPTH];
    empty[0] = hasher.hash3(Fp::zero(), Fp::zero(), Fp::zero());
    for i in 1..TREE_DEPTH {
        empty[i] = hasher.hash(empty[i - 1], empty[i - 1]);
    }
    empty
}

/// Build Merkle tree levels bottom-up from leaf hashes.
///
/// `depth` controls the number of tree levels (use `TREE_DEPTH` for a full
/// depth-29 tree, or a smaller value like 25 for the PIR tree).
/// Returns `(root, levels)` where `levels[0]` contains leaf hashes and
/// `levels[depth-1]` contains the root's two children.
///
/// Each level is padded to even length using the pre-computed empty hash so
/// that pair-wise hashing produces the next level cleanly. All intermediate
/// layers are retained so Merkle auth paths can be extracted in O(`depth`)
/// via simple sibling lookups.
pub fn build_levels(mut leaves: Vec<Fp>, empty: &[Fp; TREE_DEPTH], depth: usize) -> (Fp, Vec<Vec<Fp>>) {
    let hasher = PoseidonHasher::new();
    let mut levels: Vec<Vec<Fp>> = Vec::with_capacity(depth);

    // Level 0 = leaf commitments, padded to even length.
    // Takes ownership of `leaves` to avoid a 1.6 GB memcpy at scale.
    if leaves.is_empty() {
        leaves.push(empty[0]);
    }
    if leaves.len() & 1 == 1 {
        leaves.push(empty[0]);
    }
    levels.push(leaves);

    const PAR_THRESHOLD: usize = 1024;

    for i in 0..depth - 1 {
        let prev = &levels[i];
        let pairs = prev.len() / 2;
        let mut next: Vec<Fp> = if pairs >= PAR_THRESHOLD {
            prev.par_chunks_exact(2)
                .map_init(PoseidonHasher::new, |h, pair| h.hash(pair[0], pair[1]))
                .collect()
        } else {
            (0..pairs)
                .map(|j| hasher.hash(prev[j * 2], prev[j * 2 + 1]))
                .collect()
        };
        if next.len() & 1 == 1 {
            next.push(empty[i + 1]);
        }
        levels.push(next);
    }

    let top = &levels[depth - 1];
    let root = hasher.hash(top[0], top[1]);

    (root, levels)
}


