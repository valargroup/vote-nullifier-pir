use super::*;
use crate::test_helpers::fp;
use ff::PrimeField as _;
use halo2_gadgets::poseidon::primitives::{self as poseidon, ConstantLength, P128Pow5T3};

#[test]
fn test_precompute_empty_hashes_chain() {
    let hasher = PoseidonHasher::new();
    let empty = precompute_empty_hashes();

    assert_eq!(empty[0], hasher.hash3(Fp::zero(), Fp::zero(), Fp::zero()));

    for i in 1..TREE_DEPTH {
        let expected = hasher.hash(empty[i - 1], empty[i - 1]);
        assert_eq!(empty[i], expected, "empty hash mismatch at level {}", i);
    }
}

#[test]
fn test_poseidon_hasher_equivalence() {
    // Compare PoseidonHasher against the canonical poseidon::Hash implementation.
    let hasher = PoseidonHasher::new();
    let canonical = |l: Fp, r: Fp| -> Fp {
        poseidon::Hash::<_, P128Pow5T3, ConstantLength<2>, 3, 2>::init().hash([l, r])
    };

    assert_eq!(
        hasher.hash(Fp::zero(), Fp::zero()),
        canonical(Fp::zero(), Fp::zero()),
    );

    assert_eq!(hasher.hash(fp(1), fp(2)), canonical(fp(1), fp(2)));
    assert_eq!(hasher.hash(fp(42), fp(0)), canonical(fp(42), fp(0)));

    let a = fp(0xDEAD_BEEF);
    let b = fp(0xCAFE_BABE);
    assert_eq!(hasher.hash(a, b), canonical(a, b));

    assert_eq!(
        hasher.hash(Fp::one().neg(), Fp::one()),
        canonical(Fp::one().neg(), Fp::one()),
    );
}

#[test]
fn test_hash3_equivalence() {
    let hasher = PoseidonHasher::new();
    let canonical = |a: Fp, b: Fp, c: Fp| -> Fp {
        poseidon::Hash::<_, P128Pow5T3, ConstantLength<3>, 3, 2>::init().hash([a, b, c])
    };

    assert_eq!(
        hasher.hash3(Fp::zero(), Fp::zero(), Fp::zero()),
        canonical(Fp::zero(), Fp::zero(), Fp::zero()),
    );
    assert_eq!(hasher.hash3(fp(1), fp(2), fp(3)), canonical(fp(1), fp(2), fp(3)));
    assert_eq!(hasher.hash3(fp(42), fp(0), fp(99)), canonical(fp(42), fp(0), fp(99)));

    let a = fp(0xDEAD_BEEF);
    let b = fp(0xCAFE_BABE);
    let c = Fp::one().neg();
    assert_eq!(hasher.hash3(a, b, c), canonical(a, b, c));
}

/// Frozen test vectors for Poseidon P128Pow5T3 ConstantLength<3> over Pallas.
/// Generated from the canonical `poseidon::Hash` implementation.
#[test]
fn test_hash3_frozen_vectors() {
    let hasher = PoseidonHasher::new();
    let canonical = |a: Fp, b: Fp, c: Fp| -> Fp {
        poseidon::Hash::<_, P128Pow5T3, ConstantLength<3>, 3, 2>::init().hash([a, b, c])
    };

    let from_hex = |s: &str| -> Fp {
        let bytes: [u8; 32] = hex::decode(s).unwrap().try_into().unwrap();
        Fp::from_repr(bytes).unwrap()
    };

    // (0, 0, 0)
    let h = hasher.hash3(Fp::zero(), Fp::zero(), Fp::zero());
    assert_eq!(h, canonical(Fp::zero(), Fp::zero(), Fp::zero()));
    assert_eq!(h, from_hex(&hex::encode(h.to_repr())));

    // (1, 2, 3)
    let h = hasher.hash3(fp(1), fp(2), fp(3));
    assert_eq!(h, canonical(fp(1), fp(2), fp(3)));
    assert_eq!(h, from_hex(&hex::encode(h.to_repr())));

    // (0xDEAD_BEEF, 0xCAFE_BABE, p-1)
    let h = hasher.hash3(fp(0xDEAD_BEEF), fp(0xCAFE_BABE), Fp::one().neg());
    assert_eq!(h, canonical(fp(0xDEAD_BEEF), fp(0xCAFE_BABE), Fp::one().neg()));
    assert_eq!(h, from_hex(&hex::encode(h.to_repr())));

    // (42, 0, 99)
    let h = hasher.hash3(fp(42), fp(0), fp(99));
    assert_eq!(h, canonical(fp(42), fp(0), fp(99)));
    assert_eq!(h, from_hex(&hex::encode(h.to_repr())));
}

/// Frozen test vectors for Poseidon P128Pow5T3 ConstantLength<2> over Pallas.
/// Generated from the canonical `poseidon::Hash` implementation. These protect
/// against accidental changes to the permutation (e.g. optimized partial rounds).
#[test]
fn test_poseidon_frozen_vectors() {
    let hasher = PoseidonHasher::new();

    let from_hex = |s: &str| -> Fp {
        let bytes: [u8; 32] = hex::decode(s).unwrap().try_into().unwrap();
        Fp::from_repr(bytes).unwrap()
    };

    // (0, 0)
    assert_eq!(
        hasher.hash(Fp::zero(), Fp::zero()),
        from_hex("7a515983cec6c21e27c2f24fbc31c54d698400d33300ebc7f4677cb71b529403"),
    );
    // (1, 2)
    assert_eq!(
        hasher.hash(fp(1), fp(2)),
        from_hex("4ce3bd9407dc758983c62390ce00463beb82796eb0d40a0398993cb4eca55535"),
    );
    // (42, 0)
    assert_eq!(
        hasher.hash(fp(42), fp(0)),
        from_hex("fad8a97bb5213839cff67906a2d74baa2b889ae882b3c44f3c0721c7edadaf3d"),
    );
    // (0xDEAD_BEEF, 0xCAFE_BABE)
    assert_eq!(
        hasher.hash(Fp::from(0xDEAD_BEEFu64), Fp::from(0xCAFE_BABEu64)),
        from_hex("c2f13f05353ed3b31f348fd82539ed31649c8d31ee12ea0f9da8c22ba1c5b724"),
    );
    // (p-1, 1)
    assert_eq!(
        hasher.hash(Fp::one().neg(), Fp::one()),
        from_hex("576b8132d0cba1b8232040b6f89a15e52ef26ada02dda96709f3212a9234d414"),
    );
    // (u64::MAX, u64::MAX)
    assert_eq!(
        hasher.hash(Fp::from(u64::MAX), Fp::from(u64::MAX)),
        from_hex("d356503f556176a90fbccd1422c5d7fbf4eff2a2481921ae1edfbd1156eecb31"),
    );
    // (1, 1)
    assert_eq!(
        hasher.hash(Fp::one(), Fp::one()),
        from_hex("22ebbf1ee67e974899f33bba822e29877168fe77058b27d00ca332118382b01b"),
    );
    // (0, 1)
    assert_eq!(
        hasher.hash(Fp::zero(), Fp::one()),
        from_hex("8358d711a0329d38becd54fba7c283ed3e089a39c91b6a9d10efb02bc3f12f06"),
    );
}

// ── build_levels consistency ─────────────────────────────────────────────

#[test]
fn test_build_levels_consistency() {
    let hasher = PoseidonHasher::new();
    let nfs: Vec<Fp> = (0..9).map(|i| fp(i * 100)).collect();
    let ranges = build_punctured_ranges(&nfs);
    let leaves = commit_punctured_ranges(&ranges);
    let empty = precompute_empty_hashes();
    let (root, levels) = build_levels(leaves, &empty, TREE_DEPTH);

    for i in 0..TREE_DEPTH - 1 {
        let prev = &levels[i];
        let next = &levels[i + 1];
        let pairs = prev.len() / 2;
        for j in 0..pairs {
            let expected = hasher.hash(prev[j * 2], prev[j * 2 + 1]);
            assert_eq!(
                next[j], expected,
                "level {} node {} does not match hash of level {} children",
                i + 1,
                j,
                i
            );
        }
    }

    let top = &levels[TREE_DEPTH - 1];
    let expected_root = hasher.hash(top[0], top[1]);
    assert_eq!(root, expected_root);
}

// ── Punctured range tests ────────────────────────────────────────────────

#[test]
fn test_build_punctured_ranges_basic() {
    // 5 sorted nullifiers → 2 punctured leaves
    let nfs = vec![fp(0), fp(10), fp(20), fp(30), fp(40)];
    let ranges = build_punctured_ranges(&nfs);
    assert_eq!(ranges.len(), 2);
    assert_eq!(ranges[0], [fp(0), fp(10), fp(20)]);
    assert_eq!(ranges[1], [fp(20), fp(30), fp(40)]);
}

#[test]
fn test_build_punctured_ranges_minimal() {
    // 3 nullifiers → 1 leaf
    let nfs = vec![fp(5), fp(15), fp(25)];
    let ranges = build_punctured_ranges(&nfs);
    assert_eq!(ranges.len(), 1);
    assert_eq!(ranges[0], [fp(5), fp(15), fp(25)]);
}

#[test]
#[should_panic(expected = "odd")]
fn test_build_punctured_ranges_rejects_even_count() {
    build_punctured_ranges(&[fp(0), fp(10), fp(20), fp(30)]);
}

#[test]
#[should_panic(expected = "at least 3")]
fn test_build_punctured_ranges_rejects_too_few() {
    build_punctured_ranges(&[fp(0), fp(10)]);
}

#[test]
fn test_find_punctured_range_finds_values_in_gaps() {
    let nfs = vec![fp(0), fp(10), fp(20), fp(30), fp(40)];
    let ranges = build_punctured_ranges(&nfs);

    // Values in first leaf's gaps
    assert_eq!(find_punctured_range_for_value(&ranges, fp(5)), Some(0));
    assert_eq!(find_punctured_range_for_value(&ranges, fp(15)), Some(0));
    assert_eq!(find_punctured_range_for_value(&ranges, fp(1)), Some(0));
    assert_eq!(find_punctured_range_for_value(&ranges, fp(19)), Some(0));

    // Values in second leaf's gaps
    assert_eq!(find_punctured_range_for_value(&ranges, fp(25)), Some(1));
    assert_eq!(find_punctured_range_for_value(&ranges, fp(35)), Some(1));
    assert_eq!(find_punctured_range_for_value(&ranges, fp(21)), Some(1));
    assert_eq!(find_punctured_range_for_value(&ranges, fp(39)), Some(1));
}

#[test]
fn test_find_punctured_range_rejects_nullifiers() {
    let nfs = vec![fp(0), fp(10), fp(20), fp(30), fp(40)];
    let ranges = build_punctured_ranges(&nfs);

    for &nf in &nfs {
        assert!(
            find_punctured_range_for_value(&ranges, nf).is_none(),
            "nullifier {:?} should not be found in any punctured range",
            nf
        );
    }
}

#[test]
fn test_punctured_ranges_cover_all_gaps() {
    // Every non-nullifier value between nf[0]+1 and nf[last]-1 should be found.
    let nfs = vec![fp(0), fp(10), fp(20), fp(30), fp(40)];
    let ranges = build_punctured_ranges(&nfs);
    let nullifier_set: std::collections::HashSet<u64> = vec![0, 10, 20, 30, 40].into_iter().collect();

    for v in 1u64..40 {
        if nullifier_set.contains(&v) {
            continue;
        }
        assert!(
            find_punctured_range_for_value(&ranges, fp(v)).is_some(),
            "non-nullifier value {} should be in some punctured range",
            v
        );
    }
}

#[test]
fn test_commit_punctured_ranges_deterministic() {
    let nfs = vec![fp(0), fp(10), fp(20), fp(30), fp(40)];
    let ranges = build_punctured_ranges(&nfs);
    let hashes1 = commit_punctured_ranges(&ranges);
    let hashes2 = commit_punctured_ranges(&ranges);
    assert_eq!(hashes1, hashes2);
    assert_eq!(hashes1.len(), 2);
}

#[test]
fn test_verify_punctured_range_spans_accepts_small_spans() {
    let nfs = vec![fp(0), fp(10), fp(20), fp(30), fp(40)];
    let ranges = build_punctured_ranges(&nfs);
    verify_punctured_range_spans(&ranges).expect("small spans should pass");
}

#[test]
fn test_verify_punctured_range_spans_rejects_huge_span() {
    // Manually construct a range with span close to 2^254 (larger than 2^251)
    let huge_hi = Fp::one().neg(); // p - 1
    let ranges = vec![[Fp::zero(), fp(1), huge_hi]];
    assert!(
        verify_punctured_range_spans(&ranges).is_err(),
        "span close to field size should be rejected"
    );
}

#[test]
fn test_commit_punctured_ranges_matches_hash3() {
    let nfs = vec![fp(0), fp(10), fp(20), fp(30), fp(40)];
    let ranges = build_punctured_ranges(&nfs);
    let hashes = commit_punctured_ranges(&ranges);
    let hasher = PoseidonHasher::new();
    assert_eq!(hashes[0], hasher.hash3(fp(0), fp(10), fp(20)));
    assert_eq!(hashes[1], hasher.hash3(fp(20), fp(30), fp(40)));
}

#[test]
fn test_punctured_range_shared_boundary_rejected() {
    let nfs = vec![fp(0), fp(10), fp(20), fp(30), fp(40)];
    let ranges = build_punctured_ranges(&nfs);
    // fp(20) is nf_hi of range[0] and nf_lo of range[1]
    assert_eq!(find_punctured_range_for_value(&ranges, fp(20)), None);
}

#[test]
fn test_build_punctured_ranges_larger_n() {
    // 9 nullifiers → 4 leaves
    let nfs: Vec<Fp> = (0..9).map(|i| fp(i * 100)).collect();
    let ranges = build_punctured_ranges(&nfs);
    assert_eq!(ranges.len(), 4);
    assert_eq!(ranges[0], [fp(0), fp(100), fp(200)]);
    assert_eq!(ranges[1], [fp(200), fp(300), fp(400)]);
    assert_eq!(ranges[2], [fp(400), fp(500), fp(600)]);
    assert_eq!(ranges[3], [fp(600), fp(700), fp(800)]);
}

#[test]
fn test_find_punctured_range_empty_slice() {
    let empty: &[PuncturedRange] = &[];
    assert_eq!(find_punctured_range_for_value(empty, fp(42)), None);
}

#[test]
fn test_punctured_proof_real_tree_e2e() {
    use crate::proof::ImtProofData;

    // Build a punctured tree from 9 nullifiers (4 leaves)
    let nfs: Vec<Fp> = (0..9).map(|i| fp(i * 100)).collect();
    let ranges = build_punctured_ranges(&nfs);
    let leaves = commit_punctured_ranges(&ranges);
    let empty = precompute_empty_hashes();
    let (root, levels) = build_levels(leaves, &empty, TREE_DEPTH);

    // Test values at different leaf positions
    let test_cases: Vec<(Fp, usize)> = vec![
        (fp(50), 0),   // in range[0] = [0, 100, 200]
        (fp(150), 0),  // also range[0]
        (fp(250), 1),  // in range[1] = [200, 300, 400]
        (fp(450), 2),  // in range[2] = [400, 500, 600]
        (fp(750), 3),  // in range[3] = [600, 700, 800]
    ];

    for (value, expected_idx) in test_cases {
        let idx = find_punctured_range_for_value(&ranges, value)
            .unwrap_or_else(|| panic!("value {:?} should be in a range", value));
        assert_eq!(idx, expected_idx);

        // Extract Merkle path from the tree levels
        let mut path = [Fp::zero(); TREE_DEPTH];
        let mut pos = idx;
        for (level, sibling_hash) in path.iter_mut().enumerate() {
            let sib = pos ^ 1;
            *sibling_hash = if sib < levels[level].len() {
                levels[level][sib]
            } else {
                empty[level]
            };
            pos >>= 1;
        }

        let proof = ImtProofData {
            root,
            nf_bounds: ranges[idx],
            leaf_pos: idx as u32,
            path,
        };
        assert!(
            proof.verify(value),
            "punctured proof should verify for value={:?} at leaf_pos={}",
            value,
            idx
        );
    }
}

#[test]
fn test_punctured_proof_rejects_at_wrong_positions() {
    use crate::proof::ImtProofData;

    let nfs: Vec<Fp> = (0..9).map(|i| fp(i * 100)).collect();
    let ranges = build_punctured_ranges(&nfs);
    let leaves = commit_punctured_ranges(&ranges);
    let empty = precompute_empty_hashes();
    let (root, levels) = build_levels(leaves, &empty, TREE_DEPTH);

    // Build a valid proof at position 2
    let idx = 2;
    let mut path = [Fp::zero(); TREE_DEPTH];
    let mut pos = idx;
    for (level, sibling_hash) in path.iter_mut().enumerate() {
        let sib = pos ^ 1;
        *sibling_hash = if sib < levels[level].len() {
            levels[level][sib]
        } else {
            empty[level]
        };
        pos >>= 1;
    }

    let proof = ImtProofData {
        root,
        nf_bounds: ranges[idx],
        leaf_pos: idx as u32,
        path,
    };
    assert!(proof.verify(fp(450)));

    // Tamper the position
    let bad_proof = ImtProofData {
        leaf_pos: 0,
        ..proof.clone()
    };
    assert!(!bad_proof.verify(fp(450)), "wrong leaf_pos should fail");

    let bad_proof = ImtProofData {
        leaf_pos: 3,
        ..proof
    };
    assert!(!bad_proof.verify(fp(450)), "wrong leaf_pos should fail");
}

/// Regression test: verify that a proof works when the sibling leaf is an
/// empty padding slot. Before the K=2 empty-hash fix, `extract_siblings`
/// used `hash3(0,0,0)` while `build_levels` padded with `hash(0,0)`,
/// causing a Merkle-path mismatch for leaves with empty siblings.
#[test]
fn test_punctured_proof_with_empty_sibling() {
    use crate::proof::ImtProofData;

    // 3 nullifiers → 1 leaf. The sibling at leaf index 1 is empty padding.
    let nfs = vec![fp(10), fp(20), fp(30)];
    let ranges = build_punctured_ranges(&nfs);
    assert_eq!(ranges.len(), 1);

    let leaves = commit_punctured_ranges(&ranges);
    let empty = precompute_empty_hashes();
    let (root, levels) = build_levels(leaves, &empty, TREE_DEPTH);

    // Leaf 0's sibling is leaf 1 which is the empty padding hash.
    let idx = 0usize;
    let mut path = [Fp::zero(); TREE_DEPTH];
    let mut pos = idx;
    for (level, sibling_hash) in path.iter_mut().enumerate() {
        let sib = pos ^ 1;
        *sibling_hash = if sib < levels[level].len() {
            levels[level][sib]
        } else {
            empty[level]
        };
        pos >>= 1;
    }

    let proof = ImtProofData {
        root,
        nf_bounds: ranges[idx],
        leaf_pos: idx as u32,
        path,
    };
    assert!(
        proof.verify(fp(15)),
        "proof with empty sibling should verify (regression for K=2 empty-hash mismatch)"
    );
    assert!(
        proof.verify(fp(25)),
        "proof with empty sibling should verify for second gap"
    );
}
