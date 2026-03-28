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

/// Circuit-compatible IMT non-membership proof for punctured-range leaves (K=2).
///
/// Instead of storing a single `(low, width)` gap, each leaf commits to three
/// sorted nullifier boundaries `[nf_lo, nf_mid, nf_hi]` via `Poseidon3(...)`.
/// The leaf covers the punctured interval `(nf_lo, nf_hi) \ {nf_mid}`.
///
/// ## Circuit witnesses
///
/// Each field maps directly to a witness in the delegation circuit's
/// condition 13 (IMT non-membership):
///
/// - `root`: public input, constrained equal to `nf_imt_root` in the
///   `q_per_note` gate.
/// - `nf_bounds`: three field elements `[nf_lo, nf_mid, nf_hi]`, witnessed
///   and hashed in-circuit via `Poseidon3` (two permutations) to produce
///   the leaf commitment.
/// - `leaf_pos`: position bits that determine swap ordering at each Merkle
///   level via 29 `q_imt_swap` gates.
/// - `path`: 29-level authentication path (25 PIR siblings + 4 empty-hash
///   padding for circuit depth).
///
/// ## Circuit constraints (condition 13)
///
/// 1. **Leaf hash**: `leaf = Poseidon3(nf_lo, nf_mid, nf_hi)` — two
///    Poseidon permutations (width-3 sponge, `ConstantLength<3>`).
/// 2. **Merkle path**: 29 × swap + Poseidon → `computed_root`.
/// 3. **Strict interval**: `nf_lo < value < nf_hi` via two 251-bit range
///    checks on `value - nf_lo - 1` and `nf_hi - value - 1`.
/// 4. **Non-equality**: `value ≠ nf_mid` by witnessing `inverse(value - nf_mid)`.
/// 5. **Root pin**: `computed_root = nf_imt_root` (not gated on `is_note_real`).
///
/// ## Soundness invariants
///
/// The interval check relies on `nf_hi - nf_lo ≤ 2^251`, which is
/// guaranteed by sentinel nullifiers at `k × 2^250` spacing. Without this
/// bound, a field-wrapping span would defeat the range check.
///
/// The ordering `nf_lo < nf_mid < nf_hi` is enforced by tree construction
/// (sorted nullifier input) and locked by the Merkle commitment — a
/// forger cannot alter `nf_bounds` without breaking the Merkle path.
#[derive(Clone, Debug)]
pub struct ImtProofData {
    /// The Merkle root of the IMT.
    pub root: Fp,
    /// Three sorted nullifier boundaries: `[nf_lo, nf_mid, nf_hi]`.
    pub nf_bounds: [Fp; PUNCTURE_K + 1],
    /// Position of the leaf in the tree.
    pub leaf_pos: u32,
    /// Sibling hashes along the 29-level Merkle authentication path.
    pub path: [Fp; TREE_DEPTH],
}

impl ImtProofData {
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
    use crate::test_helpers::fp;
    use crate::tree::{precompute_empty_hashes, TREE_DEPTH};

    fn make_punctured_proof(nf_bounds: [Fp; 3]) -> ImtProofData {
        let hasher = PoseidonHasher::new();
        let empty = precompute_empty_hashes();
        let leaf = hasher.hash3(nf_bounds[0], nf_bounds[1], nf_bounds[2]);
        let mut current = leaf;
        let mut path = [Fp::zero(); TREE_DEPTH];
        for i in 0..TREE_DEPTH {
            path[i] = empty[i];
            current = hasher.hash(current, empty[i]);
        }
        ImtProofData {
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
