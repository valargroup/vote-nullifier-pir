//! Versioned on-disk checkpoint for [`super::PirTree`] (`nullifiers.tree`).
//!
//! Layout: fixed header + bincode payload. Files without the `SVOTEPT2` magic
//! are rejected so callers can remove them and rebuild.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use imt_tree::tree::TREE_DEPTH as IMT_TREE_DEPTH;
use serde::{Deserialize, Serialize};
use voting_crypto_deps::pasta_curves::group::ff::PrimeField;
use voting_crypto_deps::pasta_curves::Fp;

use super::{PirTree, PIR_DEPTH};

/// Magic ASCII tag for the Ironwood `nullifiers.tree` format.
pub const TREE_MAGIC: &[u8; 8] = b"SVOTEPT2";

/// Header: magic (8) + schema_version u32 LE (4) + height u64 LE (8) + reserved u64 LE (8).
pub const TREE_HEADER_LEN: usize = 8 + 4 + 8 + 8;

const TREE_SCHEMA_VERSION: u32 = 2;

#[derive(Serialize, Deserialize)]
struct PirTreeWire {
    pir_root: [u8; 32],
    circuit_root: [u8; 32],
    levels: Vec<Vec<[u8; 32]>>,
    ranges: Vec<[[u8; 32]; 3]>,
    empty_hashes: [[u8; 32]; IMT_TREE_DEPTH],
}

fn fp_to_bytes(x: Fp) -> [u8; 32] {
    x.to_repr()
}

fn fp_from_bytes(arr: [u8; 32]) -> Result<Fp> {
    Option::from(Fp::from_repr(arr))
        .ok_or_else(|| anyhow::anyhow!("non-canonical Fp encoding in tree checkpoint"))
}

fn encode_tree(tree: &PirTree) -> Result<Vec<u8>> {
    let levels: Vec<Vec<[u8; 32]>> = tree
        .levels
        .iter()
        .map(|row| row.iter().copied().map(fp_to_bytes).collect())
        .collect();
    let ranges: Vec<[[u8; 32]; 3]> = tree
        .ranges
        .iter()
        .map(|[a, b, c]| [fp_to_bytes(*a), fp_to_bytes(*b), fp_to_bytes(*c)])
        .collect();
    let mut empty_hashes = [[0u8; 32]; IMT_TREE_DEPTH];
    for (encoded, empty_hash) in empty_hashes.iter_mut().zip(tree.empty_hashes) {
        *encoded = fp_to_bytes(empty_hash);
    }
    let wire = PirTreeWire {
        pir_root: fp_to_bytes(tree.pir_root),
        circuit_root: fp_to_bytes(tree.circuit_root),
        levels,
        ranges,
        empty_hashes,
    };
    bincode::serialize(&wire).context("bincode serialize PirTree")
}

fn decode_tree(bytes: &[u8]) -> Result<PirTree> {
    let wire: PirTreeWire = bincode::deserialize(bytes).context("bincode deserialize PirTree")?;
    let pir_root = fp_from_bytes(wire.pir_root)?;
    let circuit_root = fp_from_bytes(wire.circuit_root)?;
    let mut levels = Vec::with_capacity(wire.levels.len());
    for row in wire.levels {
        let mut out = Vec::with_capacity(row.len());
        for b in row {
            out.push(fp_from_bytes(b)?);
        }
        levels.push(out);
    }
    let mut ranges = Vec::with_capacity(wire.ranges.len());
    for [a, b, c] in wire.ranges {
        ranges.push([fp_from_bytes(a)?, fp_from_bytes(b)?, fp_from_bytes(c)?]);
    }
    let mut empty_hashes = [Fp::zero(); IMT_TREE_DEPTH];
    for (empty_hash, encoded) in empty_hashes.iter_mut().zip(wire.empty_hashes) {
        *empty_hash = fp_from_bytes(encoded)?;
    }
    Ok(PirTree {
        pir_root,
        circuit_root,
        levels,
        ranges,
        empty_hashes,
    })
}

/// Read header fields. Returns `None` if file is missing. Errors if corrupt or unknown format.
pub fn read_tree_checkpoint_header(path: &Path) -> Result<Option<(u32, u64)>> {
    if !path.exists() {
        return Ok(None);
    }
    let mut f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hdr = [0u8; TREE_HEADER_LEN];
    f.read_exact(&mut hdr)
        .with_context(|| format!("read header {}", path.display()))?;
    if &hdr[0..8] != TREE_MAGIC.as_slice() {
        bail!(
            "nullifiers.tree at {} is not an Ironwood tree checkpoint (missing magic SVOTEPT2); \
             remove it and re-run sync",
            path.display()
        );
    }
    let schema = u32::from_le_bytes(hdr[8..12].try_into().unwrap());
    let height = u64::from_le_bytes(hdr[12..20].try_into().unwrap());
    let dataset_version = u64::from_le_bytes(hdr[20..28].try_into().unwrap());
    anyhow::ensure!(
        schema == TREE_SCHEMA_VERSION && dataset_version == u64::from(pir_types::DATASET_VERSION),
        "unsupported nullifiers.tree schema or dataset version; remove it and re-run sync"
    );
    Ok(Some((schema, height)))
}

/// Load a tree checkpoint. Returns `None` if the file does not exist.
pub fn load_tree_checkpoint(path: &Path, expected_height: u64) -> Result<Option<PirTree>> {
    if !path.exists() {
        return Ok(None);
    }
    let mut f = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hdr = [0u8; TREE_HEADER_LEN];
    f.read_exact(&mut hdr)
        .with_context(|| format!("read header {}", path.display()))?;
    if &hdr[0..8] != TREE_MAGIC.as_slice() {
        bail!(
            "nullifiers.tree at {} is not an Ironwood tree checkpoint; remove it and re-run sync",
            path.display()
        );
    }
    let schema = u32::from_le_bytes(hdr[8..12].try_into().unwrap());
    if schema != TREE_SCHEMA_VERSION {
        bail!(
            "unsupported nullifiers.tree schema_version {} (expected {})",
            schema,
            TREE_SCHEMA_VERSION
        );
    }
    let height = u64::from_le_bytes(hdr[12..20].try_into().unwrap());
    if height != expected_height {
        bail!(
            "nullifiers.tree height {} does not match expected {}; remove tree/tiers and re-sync",
            height,
            expected_height
        );
    }
    let dataset_version = u64::from_le_bytes(hdr[20..28].try_into().unwrap());
    anyhow::ensure!(
        dataset_version == u64::from(pir_types::DATASET_VERSION),
        "nullifiers.tree dataset version {} does not match expected {}; remove it and re-run sync",
        dataset_version,
        pir_types::DATASET_VERSION
    );
    let mut payload = Vec::new();
    f.read_to_end(&mut payload)
        .with_context(|| format!("read payload {}", path.display()))?;
    let tree = decode_tree(&payload)?;
    // Light sanity: level count matches PIR depth.
    anyhow::ensure!(
        tree.levels.len() == PIR_DEPTH,
        "nullifiers.tree has {} Merkle levels but this binary requires PIR_DEPTH={}; \
         remove the checkpoint and tier files, then re-run sync",
        tree.levels.len(),
        PIR_DEPTH
    );
    Ok(Some(tree))
}

/// Atomically write `nullifiers.tree` (temp + fsync + rename).
pub fn save_tree_checkpoint(path: &Path, tree: &PirTree, chain_height: u64) -> Result<()> {
    let tmp = path.with_extension("tree.tmp");
    if tmp.exists() {
        let _ = fs::remove_file(&tmp);
    }
    let payload = encode_tree(tree)?;
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)
        .with_context(|| format!("create {}", tmp.display()))?;

    let mut hdr = [0u8; TREE_HEADER_LEN];
    hdr[0..8].copy_from_slice(TREE_MAGIC.as_slice());
    hdr[8..12].copy_from_slice(&TREE_SCHEMA_VERSION.to_le_bytes());
    hdr[12..20].copy_from_slice(&chain_height.to_le_bytes());
    hdr[20..28].copy_from_slice(&u64::from(pir_types::DATASET_VERSION).to_le_bytes());
    f.write_all(&hdr)?;
    f.write_all(&payload)?;
    f.sync_all().context("fsync tree checkpoint tmp")?;
    drop(f);
    fs::rename(&tmp, path)
        .with_context(|| format!("rename tree checkpoint to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use imt_tree::tree::PuncturedRange;
    use tempfile::tempdir;

    fn tiny_tree() -> PirTree {
        let a = Fp::from(3u64);
        let b = Fp::from(5u64);
        let c = Fp::from(7u64);
        let d = Fp::from(11u64);
        let e = Fp::from(13u64);
        let ranges: Vec<PuncturedRange> = vec![[a, b, c], [c, d, e]];
        let leaves = imt_tree::commit_punctured_ranges(&ranges);
        let empty_hashes = imt_tree::precompute_empty_hashes();
        let (pir_root, levels) = imt_tree::build_levels(leaves, &empty_hashes, PIR_DEPTH);
        let circuit_root = crate::extend_root(pir_root, &empty_hashes);
        PirTree {
            pir_root,
            circuit_root,
            levels,
            ranges,
            empty_hashes,
        }
    }

    #[test]
    fn round_trip_checkpoint() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nullifiers.tree");
        let tree = tiny_tree();
        save_tree_checkpoint(&path, &tree, 1_700_000).unwrap();
        let loaded = load_tree_checkpoint(&path, 1_700_000).unwrap().unwrap();
        assert_eq!(loaded.pir_root, tree.pir_root);
        assert_eq!(loaded.ranges.len(), tree.ranges.len());
    }

    #[test]
    fn wrong_height_fails() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nullifiers.tree");
        save_tree_checkpoint(&path, &tiny_tree(), 1).unwrap();
        assert!(load_tree_checkpoint(&path, 2).is_err());
    }

    #[test]
    fn wrong_pir_depth_fails_with_rebuild_guidance() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nullifiers.tree");
        let mut tree = tiny_tree();
        tree.levels.extend((PIR_DEPTH..25).map(|_| Vec::new()));
        save_tree_checkpoint(&path, &tree, 1).unwrap();

        let err = match load_tree_checkpoint(&path, 1) {
            Ok(_) => panic!("wrong-depth checkpoint must be rejected"),
            Err(err) => err.to_string(),
        };
        assert!(err.contains("requires PIR_DEPTH=19"), "{err}");
        assert!(err.contains("re-run sync"), "{err}");
    }

    #[test]
    fn legacy_tree_magic_fails() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nullifiers.tree");
        let mut header = [0u8; TREE_HEADER_LEN];
        header[..8].copy_from_slice(b"SVOTEPT1");
        std::fs::write(&path, header).unwrap();

        let err = read_tree_checkpoint_header(&path).unwrap_err().to_string();
        assert!(err.contains("Ironwood"), "{err}");
    }
}
