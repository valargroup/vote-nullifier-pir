//! Versioned on-disk checkpoint for [`super::PirTree`] (`nullifiers.tree`).
//!
//! Layout: fixed header + bincode payload. Files without the `SVOTEPT1` magic
//! are rejected so callers can remove them and rebuild.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use ff::PrimeField;
use imt_tree::tree::TREE_DEPTH as IMT_TREE_DEPTH;
use pasta_curves::Fp;
use serde::{Deserialize, Serialize};

use super::{PirTree, PIR_DEPTH};

/// Magic ASCII tag for `nullifiers.tree` v1.
pub const TREE_MAGIC: &[u8; 8] = b"SVOTEPT1";

/// Header: magic (8) + schema_version u32 LE (4) + height u64 LE (8) + reserved u64 LE (8).
pub const TREE_HEADER_LEN: usize = 8 + 4 + 8 + 8;

const TREE_SCHEMA_V1: u32 = 1;

#[derive(Serialize, Deserialize)]
struct PirTreeWire {
    root25: [u8; 32],
    root29: [u8; 32],
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
    for i in 0..IMT_TREE_DEPTH {
        empty_hashes[i] = fp_to_bytes(tree.empty_hashes[i]);
    }
    let wire = PirTreeWire {
        root25: fp_to_bytes(tree.root25),
        root29: fp_to_bytes(tree.root29),
        levels,
        ranges,
        empty_hashes,
    };
    bincode::serialize(&wire).context("bincode serialize PirTree")
}

fn decode_tree(bytes: &[u8]) -> Result<PirTree> {
    let wire: PirTreeWire = bincode::deserialize(bytes).context("bincode deserialize PirTree")?;
    let root25 = fp_from_bytes(wire.root25)?;
    let root29 = fp_from_bytes(wire.root29)?;
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
    for i in 0..IMT_TREE_DEPTH {
        empty_hashes[i] = fp_from_bytes(wire.empty_hashes[i])?;
    }
    Ok(PirTree {
        root25,
        root29,
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
            "nullifiers.tree at {} is not a v1 tree checkpoint (missing magic SVOTEPT1); \
             remove it and re-run sync",
            path.display()
        );
    }
    let schema = u32::from_le_bytes(hdr[8..12].try_into().unwrap());
    let height = u64::from_le_bytes(hdr[12..20].try_into().unwrap());
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
            "nullifiers.tree at {} is not a v1 tree checkpoint; remove it and re-run sync",
            path.display()
        );
    }
    let schema = u32::from_le_bytes(hdr[8..12].try_into().unwrap());
    if schema != TREE_SCHEMA_V1 {
        bail!(
            "unsupported nullifiers.tree schema_version {} (expected {})",
            schema,
            TREE_SCHEMA_V1
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
    let mut payload = Vec::new();
    f.read_to_end(&mut payload)
        .with_context(|| format!("read payload {}", path.display()))?;
    let tree = decode_tree(&payload)?;
    // Light sanity: level count matches PIR depth.
    anyhow::ensure!(
        tree.levels.len() == PIR_DEPTH,
        "checkpoint levels len {} != PIR_DEPTH {}",
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
    hdr[8..12].copy_from_slice(&TREE_SCHEMA_V1.to_le_bytes());
    hdr[12..20].copy_from_slice(&chain_height.to_le_bytes());
    hdr[20..28].copy_from_slice(&0u64.to_le_bytes());
    f.write_all(&hdr)?;
    f.write_all(&payload)?;
    f.sync_all().context("fsync tree checkpoint tmp")?;
    drop(f);
    fs::rename(&tmp, path).with_context(|| format!("rename tree checkpoint to {}", path.display()))?;
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
        let (root25, levels) = imt_tree::build_levels(leaves, &empty_hashes, PIR_DEPTH);
        let root29 = crate::extend_root(root25, &empty_hashes);
        PirTree {
            root25,
            root29,
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
        assert_eq!(loaded.root25, tree.root25);
        assert_eq!(loaded.ranges.len(), tree.ranges.len());
    }

    #[test]
    fn wrong_height_fails() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("nullifiers.tree");
        save_tree_checkpoint(&path, &tiny_tree(), 1).unwrap();
        assert!(load_tree_checkpoint(&path, 2).is_err());
    }
}
