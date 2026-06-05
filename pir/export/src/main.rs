//! Standalone CLI for building PIR tier files from a nullifier data file.
//!
//! Dev helper: build tier files from a `nullifiers.bin` at an arbitrary path.
//! For production paths, use `nf-server sync` (nullifiers + `nullifiers.tree` + tiers).
//! Requires the `cli` feature
//! (enabled by default when building this binary target).
//!
//! Usage: `pir-export --nullifiers nullifiers.bin [--output-dir ./pir-data]`

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use clap::Parser;

use nf_ingest::file_store;

/// CLI arguments for the export command.
#[derive(Parser)]
#[command(name = "pir-export", about = "Build PIR tier databases from nullifier data")]
struct Args {
    /// Path to nullifiers.bin (sorted 32-byte Fp elements).
    #[arg(long)]
    nullifiers: PathBuf,

    /// Output directory for tier files.
    #[arg(long, default_value = "./pir-data")]
    output_dir: PathBuf,

    /// Path to nullifiers.checkpoint (16 bytes: [u64 height LE][u64 offset LE]).
    /// If provided, the sync height is embedded in root_info.json.
    #[arg(long)]
    checkpoint: Option<PathBuf>,
}

const CHECKPOINT_SIZE: usize = 16;
const NULLIFIER_SIZE: usize = 32;

/// Parse a checkpoint payload as `(height, committed_byte_offset)`.
///
/// The format is exactly 16 bytes: `[u64 LE height][u64 LE offset]`.
fn parse_checkpoint_bytes(cp_data: &[u8]) -> Result<(u64, u64)> {
    anyhow::ensure!(
        cp_data.len() == CHECKPOINT_SIZE,
        "checkpoint file size {} bytes (expected exactly {})",
        cp_data.len(),
        CHECKPOINT_SIZE
    );
    let height = u64::from_le_bytes(cp_data[..8].try_into().expect("checkpoint height slice"));
    let offset = u64::from_le_bytes(cp_data[8..].try_into().expect("checkpoint offset slice"));
    Ok((height, offset))
}

/// Return the committed `nullifiers.bin` prefix for a checkpoint offset.
///
/// Validates bounds and 32-byte nullifier alignment before slicing.
fn nullifier_prefix_for_offset<'a>(data: &'a [u8], offset: u64) -> Result<&'a [u8]> {
    anyhow::ensure!(
        offset <= data.len() as u64,
        "checkpoint offset {} exceeds nullifiers file size {}",
        offset,
        data.len()
    );
    anyhow::ensure!(
        offset.is_multiple_of(NULLIFIER_SIZE as u64),
        "checkpoint offset {} is not a multiple of {}",
        offset,
        NULLIFIER_SIZE
    );
    Ok(&data[..offset as usize])
}

fn main() -> Result<()> {
    let args = Args::parse();
    let t_total = Instant::now();

    eprintln!("Loading nullifiers from {:?}...", args.nullifiers);
    let t0 = Instant::now();
    let data = std::fs::read(&args.nullifiers).context("read nullifiers file")?;
    match &args.checkpoint {
        Some(cp_path) => {
            let cp_data = std::fs::read(cp_path)
                .with_context(|| format!("read checkpoint file {:?}", cp_path))?;
            let (h, offset) = parse_checkpoint_bytes(&cp_data)?;
            let committed = nullifier_prefix_for_offset(&data, offset)?;
            let nfs = file_store::parse_nullifier_bytes(committed)?;
            eprintln!(
                "  Loaded {} nullifiers from committed prefix ({} bytes) in {:.1}s",
                nfs.len(),
                offset,
                t0.elapsed().as_secs_f64()
            );
            eprintln!("  Checkpoint sync height: {}", h);
            pir_export::build_and_export(nfs, &args.output_dir, Some(h))?;
        }
        None => {
            let nfs = file_store::parse_nullifier_bytes(&data)?;
            eprintln!("  Loaded {} nullifiers in {:.1}s", nfs.len(), t0.elapsed().as_secs_f64());
            pir_export::build_and_export(nfs, &args.output_dir, None)?;
        }
    }

    eprintln!("\nDone! Total time: {:.1}s", t_total.elapsed().as_secs_f64());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{nullifier_prefix_for_offset, parse_checkpoint_bytes};

    #[test]
    fn parse_checkpoint_bytes_requires_exact_size() {
        let err = parse_checkpoint_bytes(&[0u8; 8]).expect_err("must reject short checkpoint");
        assert!(err.to_string().contains("expected exactly 16"));
    }

    #[test]
    fn parse_checkpoint_bytes_round_trip() {
        let mut cp = [0u8; 16];
        cp[..8].copy_from_slice(&123u64.to_le_bytes());
        cp[8..].copy_from_slice(&64u64.to_le_bytes());
        let (h, o) = parse_checkpoint_bytes(&cp).expect("valid checkpoint");
        assert_eq!(h, 123);
        assert_eq!(o, 64);
    }

    #[test]
    fn nullifier_prefix_rejects_unaligned_offset() {
        let data = vec![0u8; 64];
        let err = nullifier_prefix_for_offset(&data, 63).expect_err("must reject unaligned");
        assert!(err.to_string().contains("not a multiple of 32"));
    }

    #[test]
    fn nullifier_prefix_rejects_offset_beyond_file() {
        let data = vec![0u8; 64];
        let err = nullifier_prefix_for_offset(&data, 96).expect_err("must reject out of bounds");
        assert!(err.to_string().contains("exceeds nullifiers file size"));
    }

    #[test]
    fn nullifier_prefix_accepts_valid_offset() {
        let data = vec![0u8; 96];
        let prefix = nullifier_prefix_for_offset(&data, 64).expect("valid prefix");
        assert_eq!(prefix.len(), 64);
    }
}
