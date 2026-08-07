//! Integration test: full PIR round-trip without YPIR.
//!
//! Builds a depth-19 punctured-range tree (K=2) from synthetic nullifiers,
//! exports tier data, parses it back, constructs proofs, and verifies them.

use ff::{Field, PrimeField as _};
use pasta_curves::Fp;

use imt_tree::hasher::PoseidonHasher;
use imt_tree::tree::TREE_DEPTH;
use imt_tree::ImtProofData;

use pir_export::tier0::Tier0Data;
use pir_export::tier1::Tier1Row;
use pir_export::{
    build_pir_tree, build_ranges_with_sentinels, PIR_DEPTH, TIER0_LAYERS, TIER1_LEAVES,
    TIER1_ROW_BYTES,
};

/// Perform local proof construction from tier data (mirrors pir_client::fetch_proof_local).
fn construct_proof(
    tier0_data: &[u8],
    tier1_data: &[u8],
    num_ranges: usize,
    value: Fp,
    empty_hashes: &[Fp; TREE_DEPTH],
    root29: Fp,
) -> Option<ImtProofData> {
    let hasher = PoseidonHasher::new();
    let tier0 = Tier0Data::from_bytes(tier0_data.to_vec()).ok()?;

    let s1 = tier0.find_subtree(value)?;

    let mut path = [Fp::default(); TREE_DEPTH];

    let tier0_siblings = tier0.extract_siblings(s1);
    for (i, &sib) in tier0_siblings.iter().enumerate() {
        path[PIR_DEPTH - TIER0_LAYERS + i] = sib;
    }

    let t1_offset = s1 * TIER1_ROW_BYTES;
    let tier1_row = &tier1_data[t1_offset..t1_offset + TIER1_ROW_BYTES];
    let tier1 = Tier1Row::from_bytes(tier1_row).ok()?;

    let valid_leaves = num_ranges
        .saturating_sub(s1 * TIER1_LEAVES)
        .min(TIER1_LEAVES);
    let leaf_idx = tier1.find_leaf(value, valid_leaves)?;
    let tier1_siblings = tier1.extract_siblings(leaf_idx, valid_leaves, &hasher);
    for (i, &sib) in tier1_siblings.iter().enumerate() {
        path[i] = sib;
    }

    path[PIR_DEPTH..TREE_DEPTH].copy_from_slice(&empty_hashes[PIR_DEPTH..TREE_DEPTH]);

    let global_leaf_idx = s1 * TIER1_LEAVES + leaf_idx;
    let (nf_lo, nf_mid, nf_hi) = tier1.leaf_record(leaf_idx);

    Some(ImtProofData {
        root: root29,
        nf_bounds: [nf_lo, nf_mid, nf_hi],
        leaf_pos: global_leaf_idx as u32,
        path,
    })
}

#[test]
fn test_small_tree_round_trip() {
    // Build a small tree with 100 nullifiers
    let mut rng = rand::thread_rng();
    let raw_nfs: Vec<Fp> = (0..100).map(|_| Fp::random(&mut rng)).collect();
    let ranges = build_ranges_with_sentinels(&raw_nfs);

    eprintln!("  Ranges: {}", ranges.len());

    let tree = build_pir_tree(ranges.clone()).unwrap();
    eprintln!("  PIR root: {}", hex::encode(tree.root25.to_repr()));
    eprintln!("  Root29: {}", hex::encode(tree.root29.to_repr()));

    // Export tier data
    let tier0_data =
        pir_export::tier0::export(&tree.root25, &tree.levels, &tree.ranges, &tree.empty_hashes);

    let mut tier1_data = Vec::new();
    pir_export::tier1::export(&tree.ranges, &mut tier1_data).unwrap();

    eprintln!("  Tier sizes: {} / {}", tier0_data.len(), tier1_data.len());

    // Test multiple values
    let mut passed = 0;
    for &[nf_lo, _, _] in ranges.iter().take(20) {
        // nf_lo + 1 is always strictly inside the punctured range
        let value = nf_lo + Fp::one();
        let proof = construct_proof(
            &tier0_data,
            &tier1_data,
            ranges.len(),
            value,
            &tree.empty_hashes,
            tree.root29,
        );

        match proof {
            Some(p) => {
                assert!(
                    p.verify(value),
                    "Proof failed verification for value {}",
                    hex::encode(value.to_repr())
                );
                passed += 1;
            }
            None => {
                panic!(
                    "Failed to construct proof for value {} (low of a valid range)",
                    hex::encode(value.to_repr())
                );
            }
        }
    }

    eprintln!("  {} proofs passed", passed);
}

#[test]
fn test_root_extension_is_deterministic() {
    let mut rng = rand::thread_rng();
    let raw_nfs: Vec<Fp> = (0..50).map(|_| Fp::random(&mut rng)).collect();

    let ranges1 = build_ranges_with_sentinels(&raw_nfs);
    let tree1 = build_pir_tree(ranges1).unwrap();

    let ranges2 = build_ranges_with_sentinels(&raw_nfs);
    let tree2 = build_pir_tree(ranges2).unwrap();

    assert_eq!(tree1.root25, tree2.root25);
    assert_eq!(tree1.root29, tree2.root29);
}

#[test]
fn test_pir_proof_verifies_independently() {
    let mut rng = rand::thread_rng();
    let raw_nfs: Vec<Fp> = (0..200).map(|_| Fp::random(&mut rng)).collect();

    let ranges = build_ranges_with_sentinels(&raw_nfs);
    let tree = build_pir_tree(ranges.clone()).unwrap();

    let tier0_data =
        pir_export::tier0::export(&tree.root25, &tree.levels, &tree.ranges, &tree.empty_hashes);
    let mut tier1_data = Vec::new();
    pir_export::tier1::export(&tree.ranges, &mut tier1_data).unwrap();

    for &[nf_lo, _, _] in ranges.iter().take(50) {
        let value = nf_lo + Fp::one();

        let proof_pir = construct_proof(
            &tier0_data,
            &tier1_data,
            ranges.len(),
            value,
            &tree.empty_hashes,
            tree.root29,
        )
        .expect("PIR proof construction failed");

        assert!(proof_pir.verify(value), "PIR proof verification failed");
    }
}

#[test]
fn test_pir_proofs_across_all_populated_tier1_rows() {
    // Approximate the production dataset size with deterministic, well-spaced
    // nullifiers. This produces more than 100 Tier 1 rows and ensures that the
    // local-to-global leaf index conversion is exercised with non-zero rows.
    let raw_nfs: Vec<Fp> = (1u64..=31_000).map(|i| Fp::from(i * 1_000)).collect();
    let ranges = build_ranges_with_sentinels(&raw_nfs);
    let tree = build_pir_tree(ranges.clone()).unwrap();

    let tier0_data =
        pir_export::tier0::export(&tree.root25, &tree.levels, &tree.ranges, &tree.empty_hashes);
    let mut tier1_data = Vec::new();
    pir_export::tier1::export(&tree.ranges, &mut tier1_data).unwrap();

    for (row_idx, row) in ranges.chunks(TIER1_LEAVES).enumerate() {
        let mut local_indices = vec![0, 1, row.len() - 1];
        local_indices.sort_unstable();
        local_indices.dedup();

        for local_idx in local_indices {
            let expected_idx = row_idx * TIER1_LEAVES + local_idx;
            let expected_bounds = ranges[expected_idx];
            let value = expected_bounds[0] + Fp::one();
            let proof = construct_proof(
                &tier0_data,
                &tier1_data,
                ranges.len(),
                value,
                &tree.empty_hashes,
                tree.root29,
            )
            .unwrap_or_else(|| {
                panic!("proof construction failed for row {row_idx}, local leaf {local_idx}")
            });

            assert!(
                proof.verify(value),
                "proof failed for row {row_idx}, local leaf {local_idx}"
            );
            assert_eq!(proof.root, tree.root29);
            assert_eq!(proof.leaf_pos as usize, expected_idx);
            assert_eq!(proof.nf_bounds, expected_bounds);
        }
    }

    // The first row probe covers the lower endpoint at value 1. Also query
    // p - 2 to cover the opposite end of the final punctured range.
    let value = Fp::from(2u64).neg();
    let expected_idx = ranges.len() - 1;
    let proof = construct_proof(
        &tier0_data,
        &tier1_data,
        ranges.len(),
        value,
        &tree.empty_hashes,
        tree.root29,
    )
    .expect("proof construction failed near the field maximum");
    assert!(proof.verify(value));
    assert_eq!(proof.root, tree.root29);
    assert_eq!(proof.leaf_pos as usize, expected_idx);
    assert_eq!(proof.nf_bounds, ranges[expected_idx]);
}

/// Test the `build_and_export` convenience function (used by the serve rebuild path).
///
/// This exercises the full pipeline: sort, sentinel injection, tree build, and
/// tier file export to disk. Verifies the output files exist and the metadata
/// records the correct height.
#[test]
fn test_build_and_export_writes_files() {
    let dir = std::env::temp_dir().join(format!("pir_build_export_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let nfs: Vec<Fp> = (1u64..=50).map(|i| Fp::from(i * 997)).collect();
    let tree =
        pir_export::build_and_export(nfs, &dir, pir_types::ZcashNetwork::Test, Some(4_134_000))
            .unwrap();

    // Verify files exist
    assert!(dir.join("tier0.bin").exists());
    assert!(dir.join("tier1.bin").exists());
    assert!(dir.join("pir_root.json").exists());

    // Verify metadata
    let meta: pir_export::PirMetadata =
        serde_json::from_str(&std::fs::read_to_string(dir.join("pir_root.json")).unwrap()).unwrap();
    assert_eq!(meta.zcash_network, pir_types::ZcashNetwork::Test);
    assert_eq!(meta.height, Some(4_134_000));
    assert_eq!(meta.nullifier_pool, pir_types::NULLIFIER_POOL);
    assert_eq!(meta.dataset_version, pir_types::DATASET_VERSION);
    assert_eq!(meta.pir_depth, pir_export::PIR_DEPTH);
    assert_eq!(meta.pir_layout, pir_types::COMPILED_PIR_LAYOUT);
    assert_eq!(meta.root29, hex::encode(tree.root29.to_repr()));
    assert!(meta.num_ranges > 25); // K=2 punctured ranges from 50 nfs + sentinels
    assert!(pir_export::tiers_complete_for_height(
        &dir,
        pir_types::ZcashNetwork::Test,
        4_134_000,
        &pir_export::COMPILED_PIR_LAYOUT,
    )
    .unwrap());
    assert!(!pir_export::tiers_complete_for_height(
        &dir,
        pir_types::ZcashNetwork::Main,
        4_134_000,
        &pir_export::COMPILED_PIR_LAYOUT,
    )
    .unwrap());

    let _ = std::fs::remove_dir_all(&dir);
}

/// Test that `build_and_export` with a subset of nullifiers produces a different
/// root than the full set, and that the exported tier files are valid.
///
/// This verifies the target-height export path: when the export pipeline operates
/// on a subset of nullifiers (as it does with `--target-height`), the resulting
/// tree has a distinct, self-consistent root.
#[test]
fn test_subset_export_produces_different_root() {
    let raw_nfs: Vec<Fp> = (1u64..=200).map(|i| Fp::from(i * 7919)).collect();

    // Build tree from full set
    let full_ranges = build_ranges_with_sentinels(&raw_nfs);
    let full_tree = build_pir_tree(full_ranges.clone()).unwrap();

    // Build tree from first half (simulates export at a lower target height)
    let half = raw_nfs.len() / 2;
    let subset_ranges = build_ranges_with_sentinels(&raw_nfs[..half]);
    let subset_tree = build_pir_tree(subset_ranges.clone()).unwrap();

    // Roots must differ (different nullifier sets produce different trees)
    assert_ne!(
        full_tree.root29, subset_tree.root29,
        "subset root must differ from full root"
    );

    // Export the subset tree and verify it round-trips correctly
    let tier0_data = pir_export::tier0::export(
        &subset_tree.root25,
        &subset_tree.levels,
        &subset_tree.ranges,
        &subset_tree.empty_hashes,
    );
    let mut tier1_data = Vec::new();
    pir_export::tier1::export(&subset_tree.ranges, &mut tier1_data).unwrap();

    // Verify proofs for the subset tree work
    for &[nf_lo, _, _] in subset_ranges.iter().take(20) {
        let proof = construct_proof(
            &tier0_data,
            &tier1_data,
            subset_ranges.len(),
            nf_lo + Fp::one(),
            &subset_tree.empty_hashes,
            subset_tree.root29,
        )
        .expect("subset proof construction failed");
        assert!(
            proof.verify(nf_lo + Fp::one()),
            "subset proof verification failed"
        );
    }
}

/// Test that tier export is deterministic: exporting the same tree
/// twice produces byte-identical output.
#[test]
fn test_export_deterministic() {
    let raw_nfs: Vec<Fp> = (1u64..=100).map(|i| Fp::from(i * 1013)).collect();
    let ranges = build_ranges_with_sentinels(&raw_nfs);
    let tree = build_pir_tree(ranges).unwrap();

    // Export tier1 twice
    let mut tier1_a = Vec::new();
    pir_export::tier1::export(&tree.ranges, &mut tier1_a).unwrap();

    let mut tier1_b = Vec::new();
    pir_export::tier1::export(&tree.ranges, &mut tier1_b).unwrap();

    assert_eq!(
        tier1_a, tier1_b,
        "tier1 parallel export must be deterministic"
    );
}

#[test]
/// Regression test: a leaf whose tier-1 sibling is an empty padding slot must
/// still produce a valid proof. Before the K=2 empty-hash fix, `extract_siblings`
/// used `hash3(0,0,0)` while `build_levels` padded with `hash(0,0)`, causing a
/// Merkle-path mismatch.
fn test_proof_with_empty_tier1_sibling() {
    // 3 nullifiers + sentinels → very few ranges. Most tier-1 leaf slots are
    // empty padding, so the LAST populated leaf in its row has an empty sibling.
    let raw_nfs: Vec<Fp> = vec![Fp::from(100u64), Fp::from(200u64), Fp::from(300u64)];
    let ranges = build_ranges_with_sentinels(&raw_nfs);
    let tree = build_pir_tree(ranges.clone()).unwrap();

    let tier0_data =
        pir_export::tier0::export(&tree.root25, &tree.levels, &tree.ranges, &tree.empty_hashes);
    let mut tier1_data = Vec::new();
    pir_export::tier1::export(&tree.ranges, &mut tier1_data).unwrap();

    // Find the last populated range — its sibling leaf slot is empty padding.
    let last_idx = ranges.len() - 1;
    let is_even_idx = last_idx.is_multiple_of(2);
    // An even-indexed leaf has sibling at idx+1 (odd), which is empty if it's
    // the last populated leaf. Pick the value to query accordingly.
    let target_idx = if is_even_idx { last_idx } else { last_idx - 1 };
    let [nf_lo, _, _] = ranges[target_idx];
    let value = nf_lo + Fp::one();

    let proof = construct_proof(
        &tier0_data,
        &tier1_data,
        ranges.len(),
        value,
        &tree.empty_hashes,
        tree.root29,
    )
    .expect("proof construction should succeed for leaf with empty sibling");

    assert!(
        proof.verify(value),
        "proof with empty tier-1 sibling should verify \
         (regression for K=2 empty-hash mismatch)"
    );
}

#[test]
fn test_tail_coverage_near_field_max() {
    use imt_tree::tree::find_punctured_range_for_value;

    let raw_nfs: Vec<Fp> = (1u64..=50).map(|i| Fp::from(i * 997)).collect();
    let ranges = build_ranges_with_sentinels(&raw_nfs);

    // p - 2 is the largest non-sentinel value (p - 1 is a sentinel).
    let near_max = Fp::from(2u64).neg(); // p - 2
    let result = find_punctured_range_for_value(&ranges, near_max);
    assert!(
        result.is_some(),
        "value p-2 should be covered by a punctured range (tail sentinel at p-1)"
    );

    // p - 1 itself is a sentinel (nullifier), so it must NOT be found.
    let p_minus_1 = Fp::one().neg();
    assert!(
        find_punctured_range_for_value(&ranges, p_minus_1).is_none(),
        "p-1 is a sentinel and should not be in any range"
    );

    // Value 0 is also a sentinel.
    assert!(
        find_punctured_range_for_value(&ranges, Fp::zero()).is_none(),
        "0 is a sentinel and should not be in any range"
    );

    // Value 1 should be covered (just above sentinel 0).
    assert!(
        find_punctured_range_for_value(&ranges, Fp::one()).is_some(),
        "value 1 should be covered (above sentinel 0)"
    );
}

#[test]
fn test_tier0_binary_search() {
    let raw_nfs: Vec<Fp> = (1u64..=50).map(|i| Fp::from(i * 1000)).collect();
    let ranges = build_ranges_with_sentinels(&raw_nfs);
    let tree = build_pir_tree(ranges.clone()).unwrap();

    let tier0_data =
        pir_export::tier0::export(&tree.root25, &tree.levels, &tree.ranges, &tree.empty_hashes);
    let tier0 = Tier0Data::from_bytes(tier0_data).unwrap();

    // Test that values within ranges are found
    for &[nf_lo, _, _] in ranges.iter().take(10) {
        let result = tier0.find_subtree(nf_lo + Fp::one());
        assert!(
            result.is_some(),
            "find_subtree failed for nf_lo={:?}",
            nf_lo
        );
    }
}

/// Three-tier (12+4+3) build+export: files, padded sizes, metadata, and
/// proof reconstruction through the boundary + terminal tiers.
#[test]
fn test_three_tier_export_round_trip() {
    use pir_types::PirLayout;

    let layout = PirLayout {
        pir_depth: 19,
        tier0_layers: 12,
        tier1_layers: 4,
        tier2_layers: 3,
    };
    let dir = std::env::temp_dir().join(format!("pir_three_tier_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let nfs: Vec<Fp> = (1u64..=500).map(|i| Fp::from(i * 997)).collect();
    let tree = pir_export::build_and_export_with_layout(
        nfs,
        &dir,
        pir_types::ZcashNetwork::Test,
        Some(4_134_000),
        &layout,
    )
    .unwrap();

    // Sub-floor payloads are padded to the YPIR stride on disk.
    assert_eq!(
        layout.tier1_padded_row_bytes().unwrap(),
        pir_types::YPIR_MIN_ITEM_BITS / 8
    );
    assert_eq!(
        std::fs::metadata(dir.join("tier0.bin")).unwrap().len() as usize,
        layout.tier0_bytes().unwrap()
    );
    assert_eq!(
        std::fs::metadata(dir.join("tier1.bin")).unwrap().len() as usize,
        layout.tier1_file_bytes().unwrap()
    );
    assert_eq!(
        std::fs::metadata(dir.join("tier2.bin")).unwrap().len() as usize,
        layout.tier2_file_bytes().unwrap().unwrap()
    );

    let meta: pir_export::PirMetadata =
        serde_json::from_str(&std::fs::read_to_string(dir.join("pir_root.json")).unwrap()).unwrap();
    assert_eq!(meta.pir_layout, layout);
    assert_eq!(meta.tier2_rows, layout.tier2_rows().unwrap().unwrap());
    assert_eq!(
        meta.tier2_row_bytes,
        layout.tier2_padded_row_bytes().unwrap().unwrap()
    );

    assert!(pir_export::tiers_complete_for_height(
        &dir,
        pir_types::ZcashNetwork::Test,
        4_134_000,
        &layout,
    )
    .unwrap());
    // The compiled two-tier layout must not consider this directory complete.
    assert!(!pir_export::tiers_complete_for_height(
        &dir,
        pir_types::ZcashNetwork::Test,
        4_134_000,
        &pir_export::COMPILED_PIR_LAYOUT,
    )
    .unwrap());

    // Proof reconstruction through the boundary chain is covered by
    // pir-client's fetch_proof_local_with_layout tests, which consume the
    // same exporters.
    let _ = tree;
    let _ = std::fs::remove_dir_all(&dir);
}

/// Non-default tier0 layer count and PIR depth (11+5+4 = depth 20):
/// exercises the generalized Tier 0 exporter and runtime depth padding.
#[test]
fn test_non_default_tier0_and_depth_round_trip() {
    use pir_types::PirLayout;

    let layout = PirLayout {
        pir_depth: 20,
        tier0_layers: 11,
        tier1_layers: 5,
        tier2_layers: 4,
    };
    let dir = std::env::temp_dir().join(format!("pir_depth20_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);

    let nfs: Vec<Fp> = (1u64..=300).map(|i| Fp::from(i * 1013)).collect();
    let tree = pir_export::build_and_export_with_layout(
        nfs,
        &dir,
        pir_types::ZcashNetwork::Test,
        Some(4_134_000),
        &layout,
    )
    .unwrap();
    assert_eq!(tree.pir_depth, 20);

    let tier0_data = std::fs::read(dir.join("tier0.bin")).unwrap();
    assert_eq!(tier0_data.len(), layout.tier0_bytes().unwrap());
    assert_eq!(
        std::fs::metadata(dir.join("tier1.bin")).unwrap().len() as usize,
        layout.tier1_file_bytes().unwrap()
    );
    assert_eq!(
        std::fs::metadata(dir.join("tier2.bin")).unwrap().len() as usize,
        layout.tier2_file_bytes().unwrap().unwrap()
    );
    assert!(pir_export::tiers_complete_for_height(
        &dir,
        pir_types::ZcashNetwork::Test,
        4_134_000,
        &layout,
    )
    .unwrap());

    let _ = std::fs::remove_dir_all(&dir);
}
