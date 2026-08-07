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
use pasta_curves::Fp;
use tracing::info;

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
    pub root25: Fp,
    /// Depth-29 Merkle root (extended with empty hashes for circuit compatibility).
    pub root29: Fp,
    /// Tree levels (bottom-up): levels[0] = leaf hashes, levels[pir_depth-1] = root's children.
    pub levels: Vec<Vec<Fp>>,
    /// Punctured ranges (K=2): each element is `[nf_lo, nf_mid, nf_hi]`.
    pub ranges: Vec<PuncturedRange>,
    /// Precomputed empty hashes for all 29 levels.
    pub empty_hashes: [Fp; TREE_DEPTH],
    /// Depth this PIR tree was built at (== levels.len()).
    pub pir_depth: usize,
}

/// Build a PIR tree of the given depth from punctured ranges (K=2).
///
/// The ranges must already be constructed via `build_punctured_ranges`.
/// This function hashes them into leaf commitments and builds the Merkle
/// tree, then extends the root to depth 29 for circuit compatibility.
pub fn build_pir_tree_with_depth(ranges: Vec<PuncturedRange>, pir_depth: usize) -> Result<PirTree> {
    anyhow::ensure!(
        (1..=FULL_DEPTH).contains(&pir_depth),
        "PIR depth {pir_depth} outside 1..={FULL_DEPTH}"
    );
    anyhow::ensure!(
        ranges.len() <= 1 << pir_depth,
        "too many ranges ({}) for PIR depth {} (max {})",
        ranges.len(),
        pir_depth,
        1 << pir_depth
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
    let (root25, levels) = build_levels(leaves, &empty_hashes, pir_depth);
    info!(
        level_count = levels.len(),
        elapsed_s = format!("{:.1}", t1.elapsed().as_secs_f64()),
        "PIR tree built"
    );

    let root29 = extend_root_from_depth(root25, &empty_hashes, pir_depth);
    info!(root29 = hex::encode(root29.to_repr()), "depth-29 root");

    Ok(PirTree {
        root25,
        root29,
        levels,
        ranges,
        empty_hashes,
        pir_depth,
    })
}

/// Build a PIR tree at the compiled default depth ([`PIR_DEPTH`]).
pub fn build_pir_tree(ranges: Vec<PuncturedRange>) -> Result<PirTree> {
    build_pir_tree_with_depth(ranges, PIR_DEPTH)
}

/// Extend a PIR root at `pir_depth` to a depth-29 root by hashing with empty
/// subtrees.
///
/// At each extension level, the existing root is the left child and an empty
/// subtree of the appropriate height is the right child. This produces the
/// same root as building a depth-29 tree with the same leaves.
pub fn extend_root_from_depth(
    pir_root: Fp,
    empty_hashes: &[Fp; TREE_DEPTH],
    pir_depth: usize,
) -> Fp {
    let hasher = PoseidonHasher::new();
    let mut root = pir_root;
    for empty_hash in &empty_hashes[pir_depth..FULL_DEPTH] {
        root = hasher.hash(root, *empty_hash);
    }
    root
}

/// Extend the default-depth PIR root to a depth-29 root.
pub fn extend_root(root25: Fp, empty_hashes: &[Fp; TREE_DEPTH]) -> Fp {
    extend_root_from_depth(root25, empty_hashes, PIR_DEPTH)
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

/// Sort / sentinel-inject nullifiers and build the PIR Merkle structure at
/// the given depth.
pub fn build_pir_tree_from_nullifiers_with_depth(
    nfs: Vec<Fp>,
    pir_depth: usize,
) -> Result<PirTree> {
    let ranges = prepare_nullifiers(nfs);
    build_pir_tree_with_depth(ranges, pir_depth)
}

/// Sort / sentinel-inject nullifiers and build the default-depth PIR Merkle structure.
pub fn build_pir_tree_from_nullifiers(nfs: Vec<Fp>) -> Result<PirTree> {
    build_pir_tree_from_nullifiers_with_depth(nfs, PIR_DEPTH)
}

/// Build the tree from nullifiers and write [`save_tree_checkpoint`] to `tree_path`.
pub fn materialize_tree_checkpoint_with_progress(
    nfs: Vec<Fp>,
    tree_path: &Path,
    chain_height: u64,
    pir_depth: usize,
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
    info!(depth = pir_depth, "building PIR tree");
    let tree = build_pir_tree_with_depth(ranges, pir_depth)?;
    info!(
        depth = pir_depth,
        root = hex::encode(tree.root25.to_repr()),
        "PIR-depth root"
    );
    info!(
        depth = FULL_DEPTH,
        root = hex::encode(tree.root29.to_repr()),
        "root-29"
    );

    on_progress("writing tree checkpoint", 35);
    save_tree_checkpoint(tree_path, &tree, chain_height)?;
    Ok(tree)
}

/// Write `tier*.bin` and `pir_root.json` from an in-memory [`PirTree`] using
/// the given tier layout.
pub fn export_tiers_from_tree(
    tree: &PirTree,
    output_dir: &Path,
    network: pir_types::ZcashNetwork,
    height: Option<u64>,
    layout: &PirLayout,
) -> Result<()> {
    export_all_with_layout(tree, output_dir, network, height, layout)
}

/// Returns `true` when `pir_root.json` lists `expected_height`, was exported
/// with `expected_layout`, and tier files on disk match the sizes derived
/// from that layout.
pub fn tiers_complete_for_height(
    output_dir: &Path,
    expected_network: pir_types::ZcashNetwork,
    expected_height: u64,
    expected_layout: &PirLayout,
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
    let tier1_rows = expected_layout.tier1_rows().map_err(anyhow::Error::msg)?;
    let tier1_stride = expected_layout
        .tier1_padded_row_bytes()
        .map_err(anyhow::Error::msg)?;
    let tier1_file_bytes = expected_layout
        .tier1_file_bytes()
        .map_err(anyhow::Error::msg)?;
    let tier2_rows = expected_layout.tier2_rows().map_err(anyhow::Error::msg)?;
    let tier2_stride = expected_layout
        .tier2_padded_row_bytes()
        .map_err(anyhow::Error::msg)?;
    let tier2_file_bytes = expected_layout
        .tier2_file_bytes()
        .map_err(anyhow::Error::msg)?;
    let expected_tier0_bytes = expected_layout.tier0_bytes().map_err(anyhow::Error::msg)?;
    if meta.zcash_network != expected_network
        || meta.height != Some(expected_height)
        || !pir_types::is_current_dataset(&meta.nullifier_pool, meta.dataset_version)
        || meta.pir_layout != *expected_layout
        || meta.pir_depth != expected_layout.pir_depth
        || meta.tier1_rows != tier1_rows
        || meta.tier1_row_bytes != tier1_stride
        || meta.tier2_rows != tier2_rows.unwrap_or(0)
        || meta.tier2_row_bytes != tier2_stride.unwrap_or(0)
    {
        return Ok(false);
    }
    let t0 = output_dir.join("tier0.bin");
    let t1 = output_dir.join("tier1.bin");
    if !t0.exists() || !t1.exists() {
        return Ok(false);
    }
    let s0 = std::fs::metadata(&t0)?.len() as usize;
    if s0 != expected_tier0_bytes || meta.tier0_bytes != expected_tier0_bytes {
        return Ok(false);
    }
    if std::fs::metadata(&t1)?.len() as usize != tier1_file_bytes {
        return Ok(false);
    }
    let t2 = output_dir.join("tier2.bin");
    match tier2_file_bytes {
        Some(expected) => {
            if !t2.exists() || std::fs::metadata(&t2)?.len() as usize != expected {
                return Ok(false);
            }
        }
        None => {
            if t2.exists() {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

/// Validate a layout for export: split geometry plus YPIR bounds.
fn validate_layout(layout: &PirLayout) -> Result<()> {
    layout
        .validate_split()
        .map_err(anyhow::Error::msg)
        .context("invalid export layout")?;
    layout
        .validate_ypir_bounds()
        .map_err(anyhow::Error::msg)
        .context("export layout fails YPIR bounds")?;
    Ok(())
}

/// Build a PIR tree from raw nullifiers (sort, sentinel injection, tree build)
/// and export all tier files for the compiled default layout.
///
/// High-level entry point for tests and callers that want one-shot build+export.
pub fn build_and_export(
    nfs: Vec<Fp>,
    output_dir: &std::path::Path,
    network: pir_types::ZcashNetwork,
    height: Option<u64>,
) -> Result<PirTree> {
    build_and_export_with_progress(
        nfs,
        output_dir,
        network,
        height,
        &COMPILED_PIR_LAYOUT,
        |_, _| {},
    )
}

/// [`build_and_export`] for an explicit tier layout.
pub fn build_and_export_with_layout(
    nfs: Vec<Fp>,
    output_dir: &std::path::Path,
    network: pir_types::ZcashNetwork,
    height: Option<u64>,
    layout: &PirLayout,
) -> Result<PirTree> {
    build_and_export_with_progress(nfs, output_dir, network, height, layout, |_, _| {})
}

/// Build the PIR tree and export tier files, calling `on_progress(message, pct)`
/// at each major stage so callers can report progress to users.
pub fn build_and_export_with_progress(
    nfs: Vec<Fp>,
    output_dir: &std::path::Path,
    network: pir_types::ZcashNetwork,
    height: Option<u64>,
    layout: &PirLayout,
    on_progress: impl Fn(&str, u8),
) -> Result<PirTree> {
    validate_layout(layout)?;
    on_progress("sorting nullifiers", 0);
    let t1 = std::time::Instant::now();
    let tree = build_pir_tree_from_nullifiers_with_depth(nfs, layout.pir_depth)?;
    info!(
        count = tree.ranges.len(),
        elapsed_s = format!("{:.1}", t1.elapsed().as_secs_f64()),
        "PIR tree built"
    );

    on_progress("building Merkle tree", 15);
    info!(
        depth = layout.pir_depth,
        root = hex::encode(tree.root25.to_repr()),
        "PIR-depth root"
    );
    info!(
        depth = FULL_DEPTH,
        root = hex::encode(tree.root29.to_repr()),
        "root-29"
    );

    on_progress("writing tier files", 40);
    info!(?output_dir, "exporting tier files");
    export_tiers_from_tree(&tree, output_dir, network, height, layout)?;

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

/// Export the boundary-index Tier 1 of a three-tier layout.
///
/// Rows correspond to Tier 0 subtrees; each record is a `(subtree_hash,
/// min_key)` pair for a child node `tier1_layers` levels below. Rows are
/// zero-padded to the YPIR stride (`layout.tier1_padded_row_bytes()`).
pub fn export_boundary_tier(
    tree: &PirTree,
    layout: &PirLayout,
    writer: &mut impl Write,
) -> Result<()> {
    anyhow::ensure!(
        layout.tier2_enabled(),
        "boundary Tier 1 export requires a three-tier layout"
    );
    let rows = layout.tier1_rows().map_err(anyhow::Error::msg)?;
    let records_per_row = layout.tier1_leaves().map_err(anyhow::Error::msg)?;
    let payload_bytes = layout.tier1_row_bytes().map_err(anyhow::Error::msg)?;
    let stride = layout
        .tier1_padded_row_bytes()
        .map_err(anyhow::Error::msg)?;
    let child_depth = layout.tier0_layers + layout.tier1_layers;
    let bu_level = layout
        .pir_depth
        .checked_sub(child_depth)
        .context("boundary child depth exceeds PIR depth")?;
    let leaves_per_child = 1usize << bu_level;
    let mut buf = vec![0u8; stride];
    for row in 0..rows {
        buf.fill(0);
        let mut offset = 0;
        for r in 0..records_per_row {
            let child_idx = row * records_per_row + r;
            let hash = node_or_empty(&tree.levels, bu_level, child_idx, &tree.empty_hashes);
            write_fp(&mut buf[offset..], hash);
            offset += 32;
            let leaf_start = child_idx * leaves_per_child;
            let mk = subtree_min_key(&tree.ranges, leaf_start);
            write_fp(&mut buf[offset..], mk);
            offset += 32;
        }
        debug_assert_eq!(offset, payload_bytes);
        writer.write_all(&buf)?;
    }
    Ok(())
}

/// Export the terminal punctured-range Tier 2 of a three-tier layout.
/// Rows are zero-padded to the YPIR stride.
pub fn export_tier2(
    ranges: &[PuncturedRange],
    layout: &PirLayout,
    writer: &mut impl Write,
) -> Result<()> {
    let rows = layout
        .tier2_rows()
        .map_err(anyhow::Error::msg)?
        .context("Tier 2 export requires a three-tier layout")?;
    let records_per_row = layout
        .tier2_leaves()
        .map_err(anyhow::Error::msg)?
        .context("Tier 2 export requires a three-tier layout")?;
    let payload_bytes = layout
        .tier2_row_bytes()
        .map_err(anyhow::Error::msg)?
        .context("Tier 2 export requires a three-tier layout")?;
    let stride = layout
        .tier2_padded_row_bytes()
        .map_err(anyhow::Error::msg)?
        .context("Tier 2 export requires a three-tier layout")?;
    let mut buf = vec![0u8; stride];
    for row in 0..rows {
        buf.fill(0);
        let leaf_start = row * records_per_row;
        let mut offset = 0;
        for i in 0..records_per_row {
            let global_idx = leaf_start + i;
            if global_idx < ranges.len() {
                let [nf_lo, nf_mid, nf_hi] = ranges[global_idx];
                write_fp(&mut buf[offset..], nf_lo);
                offset += 32;
                write_fp(&mut buf[offset..], nf_mid);
                offset += 32;
                write_fp(&mut buf[offset..], nf_hi);
                offset += 32;
            } else {
                offset += TIER1_LEAF_BYTES; // already zeroed by buf.fill(0)
            }
        }
        debug_assert_eq!(offset, payload_bytes);
        writer.write_all(&buf)?;
    }
    Ok(())
}

fn write_tier_file(
    path: &std::path::Path,
    write: impl FnOnce(&mut std::io::BufWriter<std::fs::File>) -> Result<()>,
) -> Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    write(&mut f)?;
    f.flush()?;
    drop(f);
    evict_stale_precompute(path);
    Ok(())
}

/// Export all tier files and metadata to the given directory using the
/// compiled production layout.
pub fn export_all(
    tree: &PirTree,
    output_dir: &std::path::Path,
    network: pir_types::ZcashNetwork,
    height: Option<u64>,
) -> Result<()> {
    export_all_with_layout(tree, output_dir, network, height, &COMPILED_PIR_LAYOUT)
}

/// Export tier blobs and metadata for an arbitrary valid layout.
pub fn export_all_with_layout(
    tree: &PirTree,
    output_dir: &std::path::Path,
    network: pir_types::ZcashNetwork,
    height: Option<u64>,
    layout: &PirLayout,
) -> Result<()> {
    validate_layout(layout)?;
    anyhow::ensure!(
        tree.pir_depth == layout.pir_depth,
        "tree depth {} does not match layout depth {}",
        tree.pir_depth,
        layout.pir_depth
    );
    let tier2_enabled = layout.tier2_enabled();
    std::fs::create_dir_all(output_dir)?;
    if !tier2_enabled {
        // Dataset v1 used a second PIR database, and a prior export may have
        // used a three-tier layout. Remove Tier 2 artifacts so the directory
        // contains only the current layout's files.
        for name in ["tier2.bin", "tier2.precompute", "tier2.precompute.tmp"] {
            let path = output_dir.join(name);
            match std::fs::remove_file(&path) {
                Ok(()) => info!(legacy = %path.display(), "removed stale Tier 2 artifact"),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e)
                        .with_context(|| format!("remove stale artifact {}", path.display()));
                }
            }
        }
    }

    // Tier 0
    let t0 = Instant::now();
    let tier0_data = tier0::export_layout(
        &tree.root25,
        &tree.levels,
        &tree.ranges,
        &tree.empty_hashes,
        *layout,
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
    if tier2_enabled {
        write_tier_file(&tier1_path, |w| export_boundary_tier(tree, layout, w))?;
    } else {
        write_tier_file(&tier1_path, |w| {
            tier1::export_layout(&tree.ranges, w, *layout)
        })?;
    }
    info!(
        elapsed_s = format!("{:.1}", t1.elapsed().as_secs_f64()),
        "Tier 1 exported"
    );

    // Tier 2 (terminal tier when enabled)
    if tier2_enabled {
        let t2 = Instant::now();
        let tier2_path = output_dir.join("tier2.bin");
        write_tier_file(&tier2_path, |w| export_tier2(&tree.ranges, layout, w))?;
        info!(
            elapsed_s = format!("{:.1}", t2.elapsed().as_secs_f64()),
            "Tier 2 exported"
        );
    }

    // Metadata (tier row widths report the padded YPIR stride)
    let metadata = PirMetadata {
        zcash_network: network,
        nullifier_pool: NULLIFIER_POOL.to_owned(),
        dataset_version: DATASET_VERSION,
        root25: hex::encode(tree.root25.to_repr()),
        root29: hex::encode(tree.root29.to_repr()),
        num_ranges: tree.ranges.len(),
        pir_layout: *layout,
        pir_depth: layout.pir_depth,
        tier0_bytes: tier0_data.len(),
        tier1_rows: layout.tier1_rows().map_err(anyhow::Error::msg)?,
        tier1_row_bytes: layout
            .tier1_padded_row_bytes()
            .map_err(anyhow::Error::msg)?,
        tier2_rows: layout
            .tier2_rows()
            .map_err(anyhow::Error::msg)?
            .unwrap_or(0),
        tier2_row_bytes: layout
            .tier2_padded_row_bytes()
            .map_err(anyhow::Error::msg)?
            .unwrap_or(0),
        height,
    };
    let json = serde_json::to_string_pretty(&metadata)?;
    std::fs::write(output_dir.join("pir_root.json"), json)?;
    info!("metadata written to pir_root.json");

    Ok(())
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
