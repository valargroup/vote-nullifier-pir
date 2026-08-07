//! Shared blocking pipeline: tree checkpoint (`nullifiers.tree`) + PIR tier export.

use std::path::Path;

use anyhow::Result;
use pasta_curves::Fp;
use pir_types::PirLayout;

/// Resolve the tier layout to export, from `SVOTE_PIR_TIER_SPLIT` (e.g.
/// `12,7` or `12,4,3`; PIR depth = sum of the parts). Defaults to the
/// compiled 12+7 layout when unset or empty.
pub fn configured_pir_layout() -> Result<PirLayout> {
    let spec = match std::env::var("SVOTE_PIR_TIER_SPLIT") {
        Ok(s) if !s.trim().is_empty() => s,
        _ => return Ok(pir_types::COMPILED_PIR_LAYOUT),
    };
    let parts: Vec<usize> = spec
        .split(',')
        .map(|p| p.trim().parse::<usize>())
        .collect::<Result<_, _>>()
        .map_err(|e| anyhow::anyhow!("invalid SVOTE_PIR_TIER_SPLIT {spec:?}: {e}"))?;
    anyhow::ensure!(
        parts.len() == 2 || parts.len() == 3,
        "SVOTE_PIR_TIER_SPLIT must have 2 or 3 comma-separated layer counts, got {}",
        parts.len()
    );
    let layout = PirLayout {
        pir_depth: parts.iter().sum(),
        tier0_layers: parts[0],
        tier1_layers: parts[1],
        tier2_layers: parts.get(2).copied().unwrap_or(0),
    };
    layout.validate_split().map_err(anyhow::Error::msg)?;
    layout.validate_ypir_bounds().map_err(anyhow::Error::msg)?;
    Ok(layout)
}

/// Build or load `nullifiers.tree` at `chain_height`, then write tier files
/// for `layout` under `pir_dir`.
pub fn export_tree_and_tiers_from_nullifiers(
    nfs: Vec<Fp>,
    data_dir: &Path,
    pir_dir: &Path,
    network: pir_types::ZcashNetwork,
    chain_height: u64,
    layout: &PirLayout,
    progress: impl Fn(&str, u8) + Send,
) -> Result<()> {
    let tree_path = data_dir.join("nullifiers.tree");
    if let Ok(Some((_, hh))) = pir_export::read_tree_checkpoint_header(&tree_path) {
        if hh != chain_height {
            let _ = std::fs::remove_file(&tree_path);
        }
    }
    let tree = match pir_export::load_tree_checkpoint(&tree_path, chain_height, layout.pir_depth)? {
        Some(t) => t,
        None => pir_export::materialize_tree_checkpoint_with_progress(
            nfs,
            &tree_path,
            chain_height,
            layout.pir_depth,
            progress,
        )?,
    };
    pir_export::export_tiers_from_tree(&tree, pir_dir, network, Some(chain_height), layout)?;
    Ok(())
}
