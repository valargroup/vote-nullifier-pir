//! Standalone CLI for building PIR tier files from a nullifier data file.
//!
//! Dev helper: build tier files from an Ironwood `nullifiers.bin`.
//! A matching `nullifiers.dataset.json` must be in the same directory.
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
#[command(
    name = "pir-export",
    about = "Build PIR tier databases from nullifier data"
)]
struct Args {
    /// Path to an Ironwood nullifiers.bin with an adjacent dataset marker.
    #[arg(long)]
    nullifiers: PathBuf,

    /// Output directory for tier files.
    #[arg(long, default_value = "./pir-data")]
    output_dir: PathBuf,

    /// Path to nullifiers.checkpoint (16 bytes: [u64 height LE][u64 offset LE]).
    /// If provided, the sync height is embedded in root_info.json.
    #[arg(long)]
    checkpoint: Option<PathBuf>,

    /// Tier split as comma-separated layer counts, e.g. `12,7` (two tiers)
    /// or `12,4,3` (three tiers). PIR depth is the sum of the parts.
    /// Defaults to the compiled 12,7 layout.
    #[arg(long)]
    tier_split: Option<String>,
}

fn parse_tier_split(spec: &str) -> Result<pir_types::PirLayout> {
    let parts: Vec<usize> = spec
        .split(',')
        .map(|p| p.trim().parse::<usize>())
        .collect::<Result<_, _>>()
        .with_context(|| format!("invalid --tier-split {spec:?}"))?;
    anyhow::ensure!(
        parts.len() == 2 || parts.len() == 3,
        "--tier-split must have 2 or 3 comma-separated layer counts, got {}",
        parts.len()
    );
    let layout = pir_types::PirLayout {
        pir_depth: parts.iter().sum(),
        tier0_layers: parts[0],
        tier1_layers: parts[1],
        tier2_layers: parts.get(2).copied().unwrap_or(0),
    };
    pir_types::validate_pir_layout(&layout).map_err(anyhow::Error::msg)?;
    Ok(layout)
}

fn main() -> Result<()> {
    let args = Args::parse();
    let t_total = Instant::now();

    let nullifier_dir = args
        .nullifiers
        .parent()
        .context("nullifiers path has no parent directory")?;
    let network = file_store::dataset_network(nullifier_dir)?;

    eprintln!("Loading nullifiers from {:?}...", args.nullifiers);
    let t0 = Instant::now();
    let data = std::fs::read(&args.nullifiers).context("read nullifiers file")?;
    let nfs = file_store::parse_nullifier_bytes(&data)?;
    eprintln!(
        "  Loaded {} nullifiers in {:.1}s",
        nfs.len(),
        t0.elapsed().as_secs_f64()
    );

    let height = match &args.checkpoint {
        Some(cp_path) => {
            let cp_data = std::fs::read(cp_path)
                .with_context(|| format!("read checkpoint file {:?}", cp_path))?;
            anyhow::ensure!(
                cp_data.len() >= 8,
                "checkpoint file too small: {} bytes (expected at least 8)",
                cp_data.len()
            );
            let h = u64::from_le_bytes(cp_data[..8].try_into().map_err(|_| {
                anyhow::anyhow!("checkpoint height prefix must be exactly 8 bytes")
            })?);
            eprintln!("  Checkpoint sync height: {}", h);
            Some(h)
        }
        None => None,
    };

    let layout = match &args.tier_split {
        Some(spec) => parse_tier_split(spec)?,
        None => pir_types::COMPILED_PIR_LAYOUT,
    };
    pir_export::build_and_export_with_layout(nfs, &args.output_dir, network, height, &layout)?;

    eprintln!(
        "\nDone! Total time: {:.1}s",
        t_total.elapsed().as_secs_f64()
    );
    Ok(())
}
