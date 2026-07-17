//! Shared blocking pipeline: tree checkpoint (`nullifiers.tree`) + PIR tier export.

use std::path::Path;

use anyhow::Result;
use pasta_curves::Fp;

/// Build or load `nullifiers.tree` at `chain_height`, then write tier files under `pir_dir`.
pub fn export_tree_and_tiers_from_nullifiers(
    nfs: Vec<Fp>,
    data_dir: &Path,
    pir_dir: &Path,
    network: pir_types::ZcashNetwork,
    chain_height: u64,
    progress: impl Fn(&str, u8) + Send,
) -> Result<()> {
    let tree_path = data_dir.join("nullifiers.tree");
    if let Ok(Some((_, hh, checkpoint_network))) =
        pir_export::read_tree_checkpoint_header(&tree_path)
    {
        if hh != chain_height || checkpoint_network != network {
            let _ = std::fs::remove_file(&tree_path);
        }
    }
    let tree = match pir_export::load_tree_checkpoint(&tree_path, network, chain_height)? {
        Some(t) => t,
        None => pir_export::materialize_tree_checkpoint_with_progress(
            nfs,
            &tree_path,
            network,
            chain_height,
            progress,
        )?,
    };
    pir_export::export_tiers_from_tree(&tree, pir_dir, network, Some(chain_height))?;
    Ok(())
}
