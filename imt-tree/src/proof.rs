use pasta_curves::Fp;

use crate::hasher::PoseidonHasher;
use crate::tree::TREE_DEPTH;

/// Number of excluded nullifiers per punctured-range leaf (K=2).
///
/// Each leaf stores K+1 = 3 boundary nullifiers `[nf_lo, nf_mid, nf_hi]` and
/// covers the punctured interval `(nf_lo, nf_hi) \ {nf_mid}`.
pub const PUNCTURE_K: usize = 2;

/// Walk a Merkle authentication path and check it recomputes to `root`.
fn verify_merkle_path(
    hasher: &PoseidonHasher,
    leaf: Fp,
    mut pos: u32,
    path: &[Fp; TREE_DEPTH],
    root: Fp,
) -> bool {
    let mut current = leaf;
    for sibling in path.iter() {
        let (l, r) = if pos & 1 == 0 {
            (current, *sibling)
        } else {
            (*sibling, current)
        };
        current = hasher.hash(l, r);
        pos >>= 1;
    }
    current == root
}

/// Circuit-compatible IMT non-membership proof data.
///
/// Each field maps directly to a circuit witness:
///
/// - `root`: public input, checked against the IMT root in the instance column
/// - `low`, `width`: witnessed interval `(low, width)` pair, hashed to the leaf commitment
/// - `leaf_pos`: position bits determine swap ordering at each Merkle level
/// - `path`: sibling hashes for the 29-level Merkle authentication path
#[derive(Clone, Debug)]
pub struct ImtProofData {
    /// The Merkle root of the IMT.
    pub root: Fp,
    /// Interval start (low bound of the bracketing leaf).
    pub low: Fp,
    /// Interval width (`high - low`, pre-computed during tree construction).
    pub width: Fp,
    /// Position of the leaf in the tree.
    pub leaf_pos: u32,
    /// Sibling hashes along the 29-level Merkle path (pure siblings).
    pub path: [Fp; TREE_DEPTH],
}

impl ImtProofData {
    /// Verify this proof out-of-circuit.
    ///
    /// Checks that `value` falls within `[low, low + width]` and that the
    /// Merkle path recomputes to `root`.
    pub fn verify(&self, value: Fp) -> bool {
        // value - low <= width: if value < low, field subtraction wraps to a
        // huge value that exceeds any valid width, so the check fails correctly.
        let offset = value - self.low;
        if offset > self.width {
            return false;
        }
        let hasher = PoseidonHasher::new();
        let leaf = hasher.hash(self.low, self.width);
        verify_merkle_path(&hasher, leaf, self.leaf_pos, &self.path, self.root)
    }
}

/// Circuit-compatible IMT non-membership proof for punctured-range leaves (K=2).
///
/// Instead of storing a single `(low, width)` gap, each leaf commits to three
/// sorted nullifier boundaries `[nf_lo, nf_mid, nf_hi]` via `Poseidon3(...)`.
/// The leaf covers the punctured interval `(nf_lo, nf_hi) \ {nf_mid}`.
///
/// Circuit witnesses:
/// - `root`: public input
/// - `nf_bounds`: three field elements hashed to the leaf commitment
/// - `leaf_pos`: position bits for Merkle path swap ordering
/// - `path`: 29-level authentication path (same depth as [`ImtProofData`])
#[derive(Clone, Debug)]
pub struct PuncturedImtProofData {
    /// The Merkle root of the IMT.
    pub root: Fp,
    /// Three sorted nullifier boundaries: `[nf_lo, nf_mid, nf_hi]`.
    pub nf_bounds: [Fp; PUNCTURE_K + 1],
    /// Position of the leaf in the tree.
    pub leaf_pos: u32,
    /// Sibling hashes along the 29-level Merkle authentication path.
    pub path: [Fp; TREE_DEPTH],
}

impl PuncturedImtProofData {
    /// Verify this proof out-of-circuit.
    ///
    /// Checks that `value` lies strictly inside the punctured interval
    /// `(nf_lo, nf_hi) \ {nf_mid}` and that the Merkle path recomputes
    /// to `root`.
    pub fn verify(&self, value: Fp) -> bool {
        let [nf_lo, nf_mid, nf_hi] = self.nf_bounds;

        // value must be strictly between the outer boundaries.
        // offset = value - nf_lo must satisfy 0 < offset < span = nf_hi - nf_lo.
        // If value <= nf_lo the subtraction wraps to a huge value (> span).
        let offset = value - nf_lo;
        let span = nf_hi - nf_lo;
        if offset == Fp::zero() || offset >= span {
            return false;
        }

        // value must not equal the interior nullifier.
        if value == nf_mid {
            return false;
        }

        let hasher = PoseidonHasher::new();
        let leaf = hasher.hash3(nf_lo, nf_mid, nf_hi);
        verify_merkle_path(&hasher, leaf, self.leaf_pos, &self.path, self.root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{fp, four_nullifiers};
    use crate::tree::{precompute_empty_hashes, precompute_empty_hashes_k2, NullifierTree, TREE_DEPTH};

    #[test]
    fn test_proof_verify_rejects_wrong_value() {
        let tree = NullifierTree::build(four_nullifiers());

        let proof = tree.prove(fp(15)).unwrap();
        assert!(!proof.verify(fp(5)));
        assert!(!proof.verify(fp(10)));
    }

    #[test]
    fn test_proof_verify_rejects_wrong_root() {
        let tree = NullifierTree::build(four_nullifiers());

        let mut proof = tree.prove(fp(15)).unwrap();
        proof.root = Fp::zero();
        assert!(!proof.verify(fp(15)));
    }

    #[test]
    fn test_verify_rejects_tampered_auth_path_level_0() {
        let tree = NullifierTree::build(four_nullifiers());
        let value = fp(15);
        let mut proof = tree.prove(value).unwrap();

        proof.path[0] += Fp::one();
        assert!(
            !proof.verify(value),
            "tampered auth_path[0] should fail verification"
        );
    }

    #[test]
    fn test_verify_rejects_tampered_auth_path_mid_level() {
        let tree = NullifierTree::build(four_nullifiers());
        let value = fp(15);
        let mut proof = tree.prove(value).unwrap();

        let mid = TREE_DEPTH / 2;
        proof.path[mid] = Fp::zero();
        assert!(
            !proof.verify(value),
            "tampered auth_path[{}] should fail verification",
            mid
        );
    }

    #[test]
    fn test_verify_rejects_tampered_low() {
        let tree = NullifierTree::build(four_nullifiers());
        let value = fp(15);
        let mut proof = tree.prove(value).unwrap();

        proof.low = Fp::from(999u64);
        assert!(
            !proof.verify(value),
            "tampered low bound should fail verification"
        );
    }

    #[test]
    fn test_verify_rejects_tampered_position() {
        let tree = NullifierTree::build(four_nullifiers());
        let value = fp(15);
        let mut proof = tree.prove(value).unwrap();
        assert_eq!(proof.leaf_pos, 1);

        proof.leaf_pos = 0;
        assert!(!proof.verify(value), "position 0 (wrong) should fail");

        proof.leaf_pos = 2;
        assert!(!proof.verify(value), "position 2 (wrong) should fail");

        proof.leaf_pos = u32::MAX;
        assert!(!proof.verify(value), "position MAX (wrong) should fail");
    }

    #[test]
    fn test_verify_rejects_swapped_range_fields() {
        let tree = NullifierTree::build(four_nullifiers());
        let value = fp(15);
        let mut proof = tree.prove(value).unwrap();

        let (low, width) = (proof.low, proof.width);
        proof.low = width;
        proof.width = low;
        assert!(
            !proof.verify(value),
            "swapped range fields should fail verification"
        );
    }

    // ── PuncturedImtProofData tests ──────────────────────────────────────

    /// Build a minimal punctured proof by hand: a single leaf with a
    /// dummy 29-level path (all-zero siblings, always-left position).
    fn make_punctured_proof(nf_bounds: [Fp; 3]) -> PuncturedImtProofData {
        let hasher = PoseidonHasher::new();
        let empty = precompute_empty_hashes_k2();
        let leaf = hasher.hash3(nf_bounds[0], nf_bounds[1], nf_bounds[2]);
        let mut current = leaf;
        let mut path = [Fp::zero(); TREE_DEPTH];
        for i in 0..TREE_DEPTH {
            path[i] = empty[i];
            current = hasher.hash(current, empty[i]);
        }
        PuncturedImtProofData {
            root: current,
            nf_bounds,
            leaf_pos: 0,
            path,
        }
    }

    #[test]
    fn punctured_verify_accepts_value_in_gap() {
        let proof = make_punctured_proof([fp(10), fp(20), fp(30)]);
        assert!(proof.verify(fp(15)), "value in first gap should pass");
        assert!(proof.verify(fp(25)), "value in second gap should pass");
        assert!(proof.verify(fp(11)), "value just above nf_lo should pass");
        assert!(proof.verify(fp(29)), "value just below nf_hi should pass");
    }

    #[test]
    fn punctured_verify_rejects_boundary_nullifiers() {
        let proof = make_punctured_proof([fp(10), fp(20), fp(30)]);
        assert!(!proof.verify(fp(10)), "nf_lo should be rejected");
        assert!(!proof.verify(fp(20)), "nf_mid should be rejected");
        assert!(!proof.verify(fp(30)), "nf_hi should be rejected");
    }

    #[test]
    fn punctured_verify_rejects_out_of_range() {
        let proof = make_punctured_proof([fp(10), fp(20), fp(30)]);
        assert!(!proof.verify(fp(5)), "value below nf_lo should fail");
        assert!(!proof.verify(fp(31)), "value above nf_hi should fail");
        assert!(!proof.verify(fp(0)), "zero should fail");
        assert!(!proof.verify(Fp::one().neg()), "p-1 should fail");
    }

    #[test]
    fn punctured_verify_rejects_tampered_path() {
        let mut proof = make_punctured_proof([fp(10), fp(20), fp(30)]);
        proof.path[0] += Fp::one();
        assert!(!proof.verify(fp(15)), "tampered path should fail");
    }

    #[test]
    fn punctured_verify_rejects_tampered_bounds() {
        let mut proof = make_punctured_proof([fp(10), fp(20), fp(30)]);
        proof.nf_bounds[1] = fp(21);
        assert!(!proof.verify(fp(15)), "tampered nf_mid should fail Merkle check");
    }

    #[test]
    fn punctured_verify_rejects_wrong_root() {
        let mut proof = make_punctured_proof([fp(10), fp(20), fp(30)]);
        proof.root = Fp::zero();
        assert!(!proof.verify(fp(15)), "wrong root should fail");
    }

    #[test]
    fn punctured_verify_rejects_wrong_position() {
        let mut proof = make_punctured_proof([fp(10), fp(20), fp(30)]);
        proof.leaf_pos = 1;
        assert!(!proof.verify(fp(15)), "wrong leaf_pos should fail");
    }
}
