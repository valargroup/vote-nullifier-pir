//! PIR tree builder and tier data exporter.
//!
//! Builds a depth-19 Merkle tree from punctured-range leaves (K=2) and exports
//! the two tier files consumed by `pir-server`:
//!
//! - **Tier 0** (~384 KB): plaintext internal nodes + subtree records.
//! - **Tier 1** (48 MiB): TIER1_ROWS rows × TIER1_ROW_BYTES. Each row contains
//!   TIER1_LEAVES punctured-range leaf records (nf_lo + nf_mid + nf_hi). No
//!   internal nodes; the client rebuilds the subtree locally.

pub mod tier0;
pub mod tier1;

mod tree_checkpoint;

pub use tree_checkpoint::{
    load_tree_checkpoint, read_tree_checkpoint_header, save_tree_checkpoint, TREE_HEADER_LEN,
    TREE_MAGIC,
};

use std::io::Write;
use std::path::Path;
use std::time::Instant;

use anyhow::{Context, Result};
use ff::PrimeField as _;
use tracing::info;
use voting_crypto_deps::pasta_curves::Fp;

use imt_tree::hasher::PoseidonHasher;
use imt_tree::tree::{
    build_levels, build_punctured_ranges, commit_punctured_ranges, precompute_empty_hashes,
    verify_punctured_range_spans, PuncturedRange, TREE_DEPTH,
};

// Re-export tier-layout constants and PirMetadata from pir-types so that
// existing consumers (tier submodules, tests, downstream crates) keep working.
pub use pir_types::{
    PirLayout, PirMetadata, COMPILED_PIR_LAYOUT, DATASET_VERSION, NULLIFIER_POOL, PIR_DEPTH,
    TIER0_LAYERS, TIER1_ITEM_BITS, TIER1_LAYERS, TIER1_LEAF_BYTES, TIER1_LEAVES, TIER1_ROWS,
    TIER1_ROW_BYTES,
};

/// Depth of the full circuit tree (unchanged from existing system).
pub const FULL_DEPTH: usize = TREE_DEPTH; // 29

// ── Tree building ────────────────────────────────────────────────────────────

/// Result of building the PIR tree.
pub struct PirTree {
    /// PIR-depth Merkle root (PIR tree root for K=2).
    pub pir_root: Fp,
    /// Depth-29 Merkle root (extended with empty hashes for circuit compatibility).
    pub circuit_root: Fp,
    /// Tree levels (bottom-up): levels[0] = leaf hashes, levels[PIR_DEPTH-1] = root's children.
    pub levels: Vec<Vec<Fp>>,
    /// Punctured ranges (K=2): each element is `[nf_lo, nf_mid, nf_hi]`.
    pub ranges: Vec<PuncturedRange>,
    /// Precomputed empty hashes for all 29 levels.
    pub empty_hashes: [Fp; TREE_DEPTH],
}

/// Build a depth-19 PIR tree from punctured ranges (K=2).
///
/// The ranges must already be constructed via `build_punctured_ranges`.
/// This function hashes them into leaf commitments and builds the Merkle
/// tree, then extends the root to depth 29 for circuit compatibility.
pub fn build_pir_tree(ranges: Vec<PuncturedRange>) -> Result<PirTree> {
    anyhow::ensure!(
        ranges.len() <= 1 << PIR_DEPTH,
        "too many ranges ({}) for PIR depth {} (max {})",
        ranges.len(),
        PIR_DEPTH,
        1 << PIR_DEPTH
    );
    verify_punctured_range_spans(&ranges)?;
    let t0 = Instant::now();
    let leaves = commit_punctured_ranges(&ranges);
    info!(
        count = leaves.len(),
        elapsed_s = format!("{:.1}", t0.elapsed().as_secs_f64()),
        "PIR leaf hashing"
    );

    let empty_hashes = precompute_empty_hashes();

    let t1 = Instant::now();
    let (pir_root, levels) = build_levels(leaves, &empty_hashes, PIR_DEPTH);
    info!(
        level_count = levels.len(),
        elapsed_s = format!("{:.1}", t1.elapsed().as_secs_f64()),
        "PIR tree built"
    );

    let circuit_root = extend_root(pir_root, &empty_hashes);
    info!(
        circuit_root = hex::encode(circuit_root.to_repr()),
        "circuit root"
    );

    Ok(PirTree {
        pir_root,
        circuit_root,
        levels,
        ranges,
        empty_hashes,
    })
}

/// Extend the PIR-depth root to a depth-29 root by hashing with empty subtrees.
///
/// At each extension level, the existing root is the left child and an empty
/// subtree of the appropriate height is the right child. This produces the
/// same root as building a depth-29 tree with the same leaves.
pub fn extend_root(pir_root: Fp, empty_hashes: &[Fp; TREE_DEPTH]) -> Fp {
    let hasher = PoseidonHasher::new();
    let mut root = pir_root;
    for empty_hash in &empty_hashes[PIR_DEPTH..FULL_DEPTH] {
        root = hasher.hash(root, *empty_hash);
    }
    root
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Get the min_key for a subtree given its leftmost leaf index.
///
/// Returns `ranges[leaf_start][0]` (the `nf_lo` of the first punctured range
/// in the subtree). For empty subtrees (leaf_start >= ranges.len()),
/// returns the largest Fp value so binary search skips them.
pub fn subtree_min_key(ranges: &[PuncturedRange], leaf_start: usize) -> Fp {
    if leaf_start < ranges.len() {
        ranges[leaf_start][0]
    } else {
        Fp::one().neg() // p - 1
    }
}

pub use pir_types::fp_utils::write_fp;

/// Get a node hash from the tree levels, returning empty_hash if out of bounds.
#[inline]
pub fn node_or_empty(levels: &[Vec<Fp>], level: usize, index: usize, empty_hashes: &[Fp]) -> Fp {
    if index < levels[level].len() {
        levels[level][index]
    } else {
        empty_hashes[level]
    }
}

/// Write BFS-ordered internal node hashes for a subtree into `buf`.
///
/// Iterates relative depths 1 through `num_layers - 1`. At each depth `d`,
/// the bottom-up level is `bu_base - d` and the nodes are at global indices
/// `subtree_index * 2^d .. subtree_index * 2^d + 2^d - 1`.
///
/// Returns the number of bytes written (`((2^num_layers) - 2) * 32`).
pub fn write_internal_nodes(
    levels: &[Vec<Fp>],
    empty_hashes: &[Fp],
    bu_base: usize,
    num_layers: usize,
    subtree_index: usize,
    buf: &mut [u8],
) -> usize {
    let mut offset = 0;
    for d in 1..num_layers {
        let bu_level = bu_base - d;
        let count = 1usize << d;
        let start = subtree_index * count;
        for i in 0..count {
            let val = node_or_empty(levels, bu_level, start + i, empty_hashes);
            write_fp(&mut buf[offset..], val);
            offset += 32;
        }
    }
    offset
}

/// Exponent used for sentinel nullifier spacing: `2^SENTINEL_EXPONENT`.
///
/// With K=2 punctured ranges each leaf spans two consecutive sentinel
/// intervals, so the outer span is `2 * 2^SENTINEL_EXPONENT`. The circuit's
/// range check is 250 bits, requiring span < 2^250. Therefore the exponent
/// must be at most 249.
const SENTINEL_EXPONENT: u64 = 249;

/// Number of sentinel nullifiers injected: `0, 1*step, 2*step, ..., SENTINEL_COUNT*step`.
///
/// With step = 2^249, we need 33 sentinels (0..=32) so that
/// `32 * 2^249 = 2^254` covers the Pallas field (p ≈ 2^254.9).
const SENTINEL_COUNT: u64 = 32;

/// Sort raw nullifiers, inject circuit-required sentinels, and build punctured ranges (K=2).
///
/// Sentinels injected:
/// - `k * 2^249` for `k = 0..=32` — required by the circuit's 250-bit range check
///   (each K=2 leaf spans two intervals → span = 2 * 2^249 = 2^250).
/// - `p - 1` — closes the tail of the field so every non-nullifier value is
///   covered by some punctured range.
///
/// After injection the list is sorted, deduplicated, and (if necessary) padded
/// to an odd count so that `build_punctured_ranges` can group them into
/// complete triples.
pub fn prepare_nullifiers(mut nfs: Vec<Fp>) -> Vec<PuncturedRange> {
    use ff::Field;

    nfs.sort();
    let step = Fp::from(2u64).pow([SENTINEL_EXPONENT, 0, 0, 0]);
    let mut sentinels: Vec<Fp> = (0u64..=SENTINEL_COUNT)
        .map(|k| step * Fp::from(k))
        .collect();
    // Close the tail: p-1 ensures the last punctured range extends to the
    // end of the field, so values above the largest real nullifier are covered.
    sentinels.push(Fp::one().neg()); // p - 1
    nfs.extend(sentinels);
    nfs.sort();
    nfs.dedup();
    // K=2 requires an odd number of sorted nullifiers. If even, insert 2
    // right after sentinel 0. A real nullifier at exactly 2 has probability
    // ~2^{-254}, so this slot is effectively always free.
    if nfs.len().is_multiple_of(2) {
        debug_assert_eq!(nfs[0], Fp::zero(), "sentinel 0 must be first");
        nfs.insert(1, Fp::from(2u64));
    }
    build_punctured_ranges(&nfs)
}

/// Sort / sentinel-inject nullifiers and build the depth-19 PIR Merkle structure.
pub fn build_pir_tree_from_nullifiers(nfs: Vec<Fp>) -> Result<PirTree> {
    let ranges = prepare_nullifiers(nfs);
    build_pir_tree(ranges)
}

/// Build the tree from nullifiers and write [`save_tree_checkpoint`] to `tree_path`.
pub fn materialize_tree_checkpoint_with_progress(
    nfs: Vec<Fp>,
    tree_path: &Path,
    chain_height: u64,
    on_progress: impl Fn(&str, u8),
) -> Result<PirTree> {
    on_progress("sorting nullifiers", 0);
    let t1 = std::time::Instant::now();
    let ranges = prepare_nullifiers(nfs);
    info!(
        count = ranges.len(),
        elapsed_s = format!("{:.1}", t1.elapsed().as_secs_f64()),
        "ranges built"
    );

    on_progress("building Merkle tree", 15);
    info!(depth = PIR_DEPTH, "building PIR tree");
    let tree = build_pir_tree(ranges)?;
    info!(
        depth = PIR_DEPTH,
        root = hex::encode(tree.pir_root.to_repr()),
        "PIR-depth root"
    );
    info!(
        depth = FULL_DEPTH,
        root = hex::encode(tree.circuit_root.to_repr()),
        "circuit root"
    );

    on_progress("writing tree checkpoint", 35);
    save_tree_checkpoint(tree_path, &tree, chain_height)?;
    Ok(tree)
}

/// Write `tier*.bin` and `pir_root.json` from an in-memory [`PirTree`].
pub fn export_tiers_from_tree(
    tree: &PirTree,
    output_dir: &Path,
    network: pir_types::ZcashNetwork,
    height: Option<u64>,
) -> Result<()> {
    export_all(tree, output_dir, network, height)
}

/// Returns `true` when `pir_root.json` lists `expected_height` and tier files
/// on disk match the expected sizes from layout constants and metadata.
pub fn tiers_complete_for_height(
    output_dir: &Path,
    expected_network: pir_types::ZcashNetwork,
    expected_height: u64,
) -> Result<bool> {
    let root_path = output_dir.join("pir_root.json");
    if !root_path.exists() {
        return Ok(false);
    }
    let Ok(meta) = serde_json::from_str::<PirMetadata>(
        &std::fs::read_to_string(&root_path).context("read pir_root.json")?,
    ) else {
        return Ok(false);
    };
    let Ok(()) = meta.pir_layout.validate_supported() else {
        return Ok(false);
    };
    let Ok(layout_rows) = meta.pir_layout.tier1_rows() else {
        return Ok(false);
    };
    let Ok(layout_row_bytes) = meta.pir_layout.tier1_row_bytes() else {
        return Ok(false);
    };
    let Ok(expected_tier0_bytes) = meta.pir_layout.tier0_bytes() else {
        return Ok(false);
    };
    if meta.zcash_network != expected_network
        || meta.height != Some(expected_height)
        || !pir_types::is_current_dataset(&meta.nullifier_pool, meta.dataset_version)
        || meta.pir_depth != meta.pir_layout.pir_depth
        || meta.tier1_rows != layout_rows
        || meta.tier1_row_bytes != layout_row_bytes
        || meta.tier0_bytes != expected_tier0_bytes
    {
        return Ok(false);
    }
    let t0 = output_dir.join("tier0.bin");
    let t1 = output_dir.join("tier1.bin");
    if !t0.exists() || !t1.exists() {
        return Ok(false);
    }
    let s0 = std::fs::metadata(&t0)?.len() as usize;
    if s0 != expected_tier0_bytes {
        return Ok(false);
    }
    let Ok(tier0) = tier0::Tier0Data::from_layout(std::fs::read(&t0)?, meta.pir_layout) else {
        return Ok(false);
    };
    let Ok(pir_root_bytes) = hex::decode(&meta.pir_root) else {
        return Ok(false);
    };
    if pir_root_bytes.len() != 32 {
        return Ok(false);
    }
    let mut pir_root_repr = [0u8; 32];
    pir_root_repr.copy_from_slice(&pir_root_bytes);
    let Some(metadata_pir_root) = Option::<Fp>::from(Fp::from_repr(pir_root_repr)) else {
        return Ok(false);
    };
    if tier0.root() != metadata_pir_root {
        return Ok(false);
    }
    let exp1 = layout_rows * layout_row_bytes;
    if std::fs::metadata(&t1)?.len() as usize != exp1 {
        return Ok(false);
    }
    Ok(true)
}

/// Build a PIR tree from raw nullifiers (sort, sentinel injection, tree build)
/// and export all tier files.
///
/// High-level entry point for tests and callers that want one-shot build+export.
pub fn build_and_export(
    nfs: Vec<Fp>,
    output_dir: &std::path::Path,
    network: pir_types::ZcashNetwork,
    height: Option<u64>,
) -> Result<PirTree> {
    build_and_export_with_progress(nfs, output_dir, network, height, |_, _| {})
}

/// Build the PIR tree and export tier files, calling `on_progress(message, pct)`
/// at each major stage so callers can report progress to users.
pub fn build_and_export_with_progress(
    nfs: Vec<Fp>,
    output_dir: &std::path::Path,
    network: pir_types::ZcashNetwork,
    height: Option<u64>,
    on_progress: impl Fn(&str, u8),
) -> Result<PirTree> {
    on_progress("sorting nullifiers", 0);
    let t1 = std::time::Instant::now();
    let tree = build_pir_tree_from_nullifiers(nfs)?;
    info!(
        count = tree.ranges.len(),
        elapsed_s = format!("{:.1}", t1.elapsed().as_secs_f64()),
        "PIR tree built"
    );

    on_progress("building Merkle tree", 15);
    info!(
        depth = PIR_DEPTH,
        root = hex::encode(tree.pir_root.to_repr()),
        "PIR-depth root"
    );
    info!(
        depth = FULL_DEPTH,
        root = hex::encode(tree.circuit_root.to_repr()),
        "circuit root"
    );

    on_progress("writing tier files", 40);
    info!(?output_dir, "exporting tier files");
    export_tiers_from_tree(&tree, output_dir, network, height)?;

    on_progress("tier files written", 55);
    Ok(tree)
}

/// Best-effort eviction of a stale precompute cache (`tier{N}.precompute`)
/// after the corresponding `tier{N}.bin` has been (re)written. Prevents the
/// stale cache from sitting on disk between snapshot rotation and the next
/// `serve` restart. Logs but never errors (the next `serve` would reject the
/// stale cache via tier-source-hash mismatch anyway).
fn evict_stale_precompute(tier_path: &std::path::Path) {
    let cache_path = tier_path.with_extension("precompute");
    let tmp_path = cache_path.with_extension("precompute.tmp");
    for path in [&cache_path, &tmp_path] {
        match std::fs::remove_file(path) {
            Ok(()) => info!(cache = %path.display(), "evicted stale precompute cache"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => tracing::warn!(
                cache = %path.display(),
                error = %e,
                "failed to evict stale precompute cache (next serve will reject via hash)"
            ),
        }
    }
}

/// Export all tier files and metadata to the given directory using the
/// compiled production layout.
pub fn export_all(
    tree: &PirTree,
    output_dir: &std::path::Path,
    network: pir_types::ZcashNetwork,
    height: Option<u64>,
) -> Result<()> {
    export_all_with_layout(tree, output_dir, network, height, COMPILED_PIR_LAYOUT)
}

/// Export tier blobs and metadata for a supported two-tier layout.
pub fn export_all_with_layout(
    tree: &PirTree,
    output_dir: &std::path::Path,
    network: pir_types::ZcashNetwork,
    height: Option<u64>,
    layout: PirLayout,
) -> Result<()> {
    layout
        .validate_supported()
        .map_err(anyhow::Error::msg)
        .context("invalid export layout")?;

    std::fs::create_dir_all(output_dir)?;
    // Dataset v1 used a second PIR database. Remove its artifacts so a
    // regenerated v2 directory contains only the current contract.
    for name in ["tier2.bin", "tier2.precompute", "tier2.precompute.tmp"] {
        let path = output_dir.join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => info!(legacy = %path.display(), "removed legacy Tier 2 artifact"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(e)
                    .with_context(|| format!("remove legacy artifact {}", path.display()));
            }
        }
    }

    // Tier 0
    let t0 = Instant::now();
    let tier0_data = tier0::export_layout(
        &tree.pir_root,
        &tree.levels,
        &tree.ranges,
        &tree.empty_hashes,
        layout,
    )?;
    let tier0_path = output_dir.join("tier0.bin");
    std::fs::write(&tier0_path, &tier0_data)?;
    evict_stale_precompute(&tier0_path);
    info!(
        bytes = tier0_data.len(),
        elapsed_s = format!("{:.1}", t0.elapsed().as_secs_f64()),
        "Tier 0 exported"
    );

    // Tier 1
    let t1 = Instant::now();
    let tier1_path = output_dir.join("tier1.bin");
    let mut f1 = std::io::BufWriter::new(std::fs::File::create(&tier1_path)?);
    tier1::export_layout(&tree.ranges, &mut f1, layout)?;
    f1.flush()?;
    drop(f1);
    evict_stale_precompute(&tier1_path);
    info!(
        elapsed_s = format!("{:.1}", t1.elapsed().as_secs_f64()),
        "Tier 1 exported"
    );

    let tier1_rows = layout.tier1_rows().map_err(anyhow::Error::msg)?;
    let tier1_row_bytes = layout.tier1_row_bytes().map_err(anyhow::Error::msg)?;

    // Metadata
    let metadata = PirMetadata {
        zcash_network: network,
        nullifier_pool: NULLIFIER_POOL.to_owned(),
        dataset_version: DATASET_VERSION,
        pir_root: hex::encode(tree.pir_root.to_repr()),
        circuit_root: hex::encode(tree.circuit_root.to_repr()),
        num_ranges: tree.ranges.len(),
        pir_depth: layout.pir_depth,
        pir_layout: layout,
        tier0_bytes: tier0_data.len(),
        tier1_rows,
        tier1_row_bytes,
        height,
    };
    let json = serde_json::to_string_pretty(&metadata)?;
    std::fs::write(output_dir.join("pir_root.json"), json)?;
    info!("metadata written to pir_root.json");

    Ok(())
}

/// In-memory two-tier export for tests (no filesystem).
pub fn export_for_layout(tree: &PirTree, layout: PirLayout) -> Result<(Vec<u8>, Vec<u8>)> {
    let tier0 = tier0::export_layout(
        &tree.pir_root,
        &tree.levels,
        &tree.ranges,
        &tree.empty_hashes,
        layout,
    )?;
    let mut tier1 = Vec::new();
    tier1::export_layout(&tree.ranges, &mut tier1, layout)?;
    Ok((tier0, tier1))
}

// ── Test utilities ───────────────────────────────────────────────────────────

/// Build punctured ranges from raw nullifiers with sentinel nullifiers injected.
///
/// Sentinels are `k * 2^249` for k in 0..=32 plus `p - 1` to close the tail.
/// After injection the list is sorted, deduplicated, and padded to an odd
/// count if needed.
pub fn build_ranges_with_sentinels(raw_nfs: &[Fp]) -> Vec<PuncturedRange> {
    use ff::Field as _;
    let step = Fp::from(2u64).pow([SENTINEL_EXPONENT, 0, 0, 0]);
    let mut all_nfs: Vec<Fp> = (0u64..=SENTINEL_COUNT)
        .map(|k| step * Fp::from(k))
        .collect();
    all_nfs.push(Fp::one().neg()); // p - 1
    all_nfs.extend_from_slice(raw_nfs);
    all_nfs.sort();
    all_nfs.dedup();
    if all_nfs.len().is_multiple_of(2) {
        debug_assert_eq!(all_nfs[0], Fp::zero(), "sentinel 0 must be first");
        all_nfs.insert(1, Fp::from(2u64));
    }
    build_punctured_ranges(&all_nfs)
}
