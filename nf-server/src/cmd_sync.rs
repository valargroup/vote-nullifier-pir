//! `nf-server sync` — resumable nullifier sync, `nullifiers.tree` checkpoint, and PIR tier export.

use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Args as ClapArgs;

use nf_ingest::config;
use nf_ingest::file_store;
use nf_ingest::sync_nullifiers;

use crate::sync_pipeline;
use crate::voting_config;

/// Env: set to `1` or `true` to delete nullifier + PIR artifacts before syncing.
pub const ENV_SYNC_RESET: &str = "SVOTE_PIR_SYNC_RESET";
/// Env: when `--non-interactive` and local checkpoint is ahead of voting
/// `snapshot_height`, must be exactly `RESYNC` to wipe artifacts and continue.
pub const ENV_SYNC_ACK_MISMATCH: &str = "SVOTE_PIR_SYNC_ACK_HEIGHT_MISMATCH";

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

fn delete_sync_artifacts(data_dir: &Path, pir_data_dir: &Path) -> Result<()> {
    for name in [
        "nullifiers.bin",
        "nullifiers.checkpoint",
        "nullifiers.index",
        "nullifiers.tree",
        "nullifiers.tree.tmp",
    ] {
        let p = data_dir.join(name);
        if p.exists() {
            std::fs::remove_file(&p).with_context(|| format!("remove {}", p.display()))?;
        }
    }
    std::fs::create_dir_all(pir_data_dir)?;
    for name in ["tier0.bin", "tier1.bin", "tier2.bin", "pir_root.json"] {
        let p = pir_data_dir.join(name);
        if p.exists() {
            std::fs::remove_file(&p).with_context(|| format!("remove {}", p.display()))?;
        }
    }
    Ok(())
}

fn prompt_resync_ahead_of_voting(local: u64, snap: u64, non_interactive: bool) -> Result<()> {
    eprintln!(
        "Local nullifier checkpoint height ({local}) is above voting-config snapshot_height ({snap}).\n\
         Delete local nullifiers + PIR artifacts and re-sync, or abort.\n\
         Type RESYNC to wipe nullifiers, tree checkpoint, and tier files, then continue."
    );
    if non_interactive {
        let ack = std::env::var(ENV_SYNC_ACK_MISMATCH).unwrap_or_default();
        if ack.trim().eq_ignore_ascii_case("RESYNC") {
            return Ok(());
        }
        bail!(
            "non-interactive mode: set {ENV_SYNC_ACK_MISMATCH}=RESYNC to confirm wipe, or run on a TTY"
        );
    }
    if !io::stdin().is_terminal() {
        bail!(
            "stdin is not a terminal; use --non-interactive with {ENV_SYNC_ACK_MISMATCH}=RESYNC"
        );
    }
    print!("> ");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    if line.trim().eq_ignore_ascii_case("RESYNC") {
        Ok(())
    } else {
        bail!("aborted (expected RESYNC)");
    }
}

#[derive(ClapArgs)]
pub struct Args {
    /// Directory containing nullifiers.bin and nullifiers.checkpoint.
    #[arg(long, default_value = ".")]
    data_dir: PathBuf,

    /// Output directory for PIR tier files (tier0.bin, tier1.bin, tier2.bin, pir_root.json).
    #[arg(long, default_value = "./pir-data")]
    output_dir: PathBuf,

    /// Lightwalletd endpoint URL. Overridden by LWD_URLS env (comma-separated).
    #[arg(long, default_value = "https://zec.rocks:443")]
    lwd_url: String,

    /// Stop syncing at this block height (must be a multiple of 10). Capped by
    /// chain tip and, when set, by voting-config `snapshot_height`.
    #[arg(long)]
    max_height: Option<u64>,

    /// voting-config.json URL. When non-empty, `snapshot_height` is required
    /// and caps the sync target. Empty disables this check (offline / dev).
    #[arg(long, env = "SVOTE_VOTING_CONFIG_URL", default_value = "")]
    voting_config_url: String,

    /// HTTP timeout for voting-config fetch.
    #[arg(long, default_value_t = 120)]
    http_timeout_secs: u64,

    /// Do not prompt on TTY; use `SVOTE_PIR_SYNC_ACK_HEIGHT_MISMATCH` when a wipe is required.
    #[arg(long)]
    non_interactive: bool,

    /// After syncing at least one new block from lightwalletd, delete `nullifiers.tree`
    /// and PIR tier files so the tree checkpoint and tiers rebuild.
    #[arg(long)]
    invalidate_after_blocks: bool,
}

pub async fn run(args: Args) -> Result<()> {
    if env_truthy(ENV_SYNC_RESET) {
        println!(
            "{} is set: clearing nullifiers + PIR tier files before sync",
            ENV_SYNC_RESET
        );
        delete_sync_artifacts(&args.data_dir, &args.output_dir)?;
    }

    let voting_url = args.voting_config_url.trim();
    let timeout = Duration::from_secs(args.http_timeout_secs.max(1));

    let snapshot_height: Option<u64> = if voting_url.is_empty() {
        None
    } else {
        Some(
            voting_config::fetch_required_snapshot_height(voting_url, timeout)
                .await
                .with_context(|| format!("fetch voting-config from {voting_url}"))?,
        )
    };

    let lwd_urls = config::resolve_lwd_urls(&args.lwd_url);
    if lwd_urls.is_empty() {
        bail!("no lightwalletd URLs resolved");
    }
    let chain_tip = sync_nullifiers::fetch_chain_tip(&lwd_urls[0])
        .await
        .context("fetch chain tip")?;

    let mut target = chain_tip;
    if let Some(m) = args.max_height {
        config::validate_export_height(m)?;
        target = target.min(m);
    }
    if let Some(s) = snapshot_height {
        target = target.min(s);
    }

    let data_dir = &args.data_dir;
    let pir_dir = &args.output_dir;

    loop {
        file_store::rebuild_index(data_dir)?;

        let local_cp = file_store::load_checkpoint(data_dir)?.map(|(h, _)| h);

        if let Some(snap) = snapshot_height {
            if let Some(local) = local_cp {
                if local > snap {
                    prompt_resync_ahead_of_voting(local, snap, args.non_interactive)?;
                    delete_sync_artifacts(data_dir, pir_dir)?;
                    continue; // re-read checkpoint after wipe
                }
            }
        }

        let needs_nullifier_sync = match local_cp {
            None => true,
            Some(h) => h < target,
        };

        println!("Data directory: {}", data_dir.display());
        println!("PIR output directory: {}", pir_dir.display());
        println!("Target block height: {target} (chain_tip={chain_tip})");
        if needs_nullifier_sync {
            println!(
                "Stage 1/3: syncing Orchard nullifiers via {} lightwalletd server(s)",
                lwd_urls.len()
            );
            let t_start = std::time::Instant::now();
            let nullifier_sync = sync_nullifiers::sync(data_dir, &lwd_urls, Some(target), |height, tgt, batch, total| {
                let elapsed = t_start.elapsed().as_secs_f64();
                let bps = if elapsed > 0.0 {
                    (height - sync_nullifiers::NU5_ACTIVATION_HEIGHT) as f64 / elapsed
                } else {
                    0.0
                };
                let remaining = (tgt - height) as f64 / bps.max(1.0);
                println!(
                    "  height {}/{} | +{} nfs | {} total nfs | {:.0} blocks/s | ~{:.0}s remaining",
                    height, tgt, batch, total, bps, remaining
                );
            })
            .await?;
            if args.invalidate_after_blocks && nullifier_sync.blocks_synced > 0 {
                for name in config::STALE_FILES {
                    let path = if name.starts_with("pir-data/") {
                        pir_dir.join(name.trim_start_matches("pir-data/"))
                    } else {
                        data_dir.join(name)
                    };
                    if path.exists() {
                        std::fs::remove_file(&path)
                            .with_context(|| format!("invalidate: remove {}", path.display()))?;
                        println!("Deleted stale artifact: {}", path.display());
                    }
                }
            }
        } else {
            println!("Stage 1/3: nullifiers already at checkpoint >= target");
        }

        let (ch, _) = file_store::load_checkpoint(data_dir)?.with_context(|| {
            format!(
                "missing nullifiers.checkpoint under {} — cannot build tree or tiers",
                data_dir.display()
            )
        })?;

        if ch < target {
            bail!(
                "checkpoint height {} is still below target {}; check SYNC_HEIGHT / voting snapshot",
                ch,
                target
            );
        }

        let tree_path = data_dir.join("nullifiers.tree");
        if let Ok(Some((_, hh))) = pir_export::read_tree_checkpoint_header(&tree_path) {
            if hh != ch {
                eprintln!(
                    "Removing stale nullifiers.tree (checkpoint height {}, tree header {})",
                    ch, hh
                );
                let _ = std::fs::remove_file(&tree_path);
            }
        }

        println!("Stage 2/3: PIR Merkle tree (nullifiers.tree checkpoint at height {ch})");
        let data_dir_c = data_dir.to_path_buf();
        let pir_dir_c = pir_dir.to_path_buf();
        tokio::task::spawn_blocking(move || -> Result<()> {
            if pir_export::tiers_complete_for_height(&pir_dir_c, ch)? {
                println!("Stage 3/3: PIR tier files already complete at height {ch}");
                return Ok(());
            }

            let nfs = file_store::load_all_nullifiers(&data_dir_c)?;
            println!("  Building/resuming tree and exporting tiers …");
            sync_pipeline::export_tree_and_tiers_from_nullifiers(
                nfs,
                &data_dir_c,
                &pir_dir_c,
                ch,
                |msg, _| eprintln!("    {msg}"),
            )?;
            Ok(())
        })
        .await
        .context("join export task")??;

        let count = file_store::nullifier_count(data_dir)?;
        println!("Done. Total nullifiers: {count}");
        break;
    }

    Ok(())
}
