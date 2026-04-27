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

fn delete_sync_artifacts(nullifier_root: &Path, tier_dir: &Path) -> Result<()> {
    for name in [
        "nullifiers.bin",
        "nullifiers.checkpoint",
        "nullifiers.index",
        "nullifiers.tree",
        "nullifiers.tree.tmp",
    ] {
        let p = nullifier_root.join(name);
        if p.exists() {
            std::fs::remove_file(&p).with_context(|| format!("remove {}", p.display()))?;
        }
    }
    std::fs::create_dir_all(tier_dir)?;
    for name in ["tier0.bin", "tier1.bin", "tier2.bin", "pir_root.json"] {
        let p = tier_dir.join(name);
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
        bail!("stdin is not a terminal; use --non-interactive with {ENV_SYNC_ACK_MISMATCH}=RESYNC");
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
    /// Directory for nullifiers.bin, nullifiers.checkpoint, nullifiers.index, and
    /// `nullifiers.tree` (same root passed to the tree export step).
    #[arg(long, default_value = "./pir-data", env = "SVOTE_PIR_DATA_DIR")]
    pir_data_dir: PathBuf,

    /// Directory for PIR tier files (tier0.bin, tier1.bin, tier2.bin, pir_root.json).
    /// When omitted, defaults to `--pir-data-dir`.
    #[arg(long)]
    output_dir: Option<PathBuf>,

    /// Lightwalletd endpoint URL. Overridden by LWD_URLS env (comma-separated).
    #[arg(
        long,
        default_value = "https://zec.rocks:443",
        env = "SVOTE_PIR_MAINNET_RPC_URL"
    )]
    lwd_url: String,

    /// Stop syncing at this block height (must be a multiple of 10). Capped by
    /// chain tip and, when set, by voting-config `snapshot_height`.
    #[arg(long)]
    max_height: Option<u64>,

    /// voting-config.json URL. When non-empty, `snapshot_height` is required
    /// and caps the sync target. Empty disables this check (offline / dev).
    #[arg(long, env = "SVOTE_PIR_VOTING_CONFIG_URL", default_value = "")]
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
    let nullifier_root = args.pir_data_dir.clone();
    let tier_dir = args
        .output_dir
        .clone()
        .unwrap_or_else(|| nullifier_root.clone());

    std::fs::create_dir_all(&nullifier_root)
        .with_context(|| format!("create {}", nullifier_root.display()))?;
    std::fs::create_dir_all(&tier_dir).with_context(|| format!("create {}", tier_dir.display()))?;

    if env_truthy(ENV_SYNC_RESET) {
        println!(
            "{} is set: clearing nullifiers + PIR tier files before sync",
            ENV_SYNC_RESET
        );
        delete_sync_artifacts(&nullifier_root, &tier_dir)?;
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

    // PIR snapshots and voting-config `snapshot_height` are defined on 10-block
    // boundaries (see `nf_ingest::config::validate_export_height`).
    let export_target = (target / 10) * 10;
    config::validate_export_height(export_target).with_context(|| {
        format!("aligned export height {export_target} (from cap {target}, chain_tip={chain_tip})")
    })?;

    let data_dir = &nullifier_root;
    let pir_dir = &tier_dir;

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
            Some(h) => h < export_target,
        };

        println!("Nullifier / tree directory: {}", data_dir.display());
        println!("Tier output directory: {}", pir_dir.display());
        println!("Export block height: {export_target} (cap {target}, chain_tip={chain_tip})");
        if needs_nullifier_sync {
            println!(
                "Stage 1/3: syncing Orchard nullifiers via {} lightwalletd server(s)",
                lwd_urls.len()
            );
            let t_start = std::time::Instant::now();
            let nullifier_sync = sync_nullifiers::sync(
                data_dir,
                &lwd_urls,
                Some(export_target),
                |height, tgt, batch, total| {
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
                },
            )
            .await?;
            if args.invalidate_after_blocks && nullifier_sync.blocks_synced > 0 {
                for name in config::INVALIDATE_AFTER_BLOCKS_TREE_FILES {
                    let path = data_dir.join(name);
                    if path.exists() {
                        std::fs::remove_file(&path)
                            .with_context(|| format!("invalidate: remove {}", path.display()))?;
                        println!("Deleted stale artifact: {}", path.display());
                    }
                }
                for name in config::INVALIDATE_AFTER_BLOCKS_TIER_FILES {
                    let path = pir_dir.join(name);
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

        if ch < export_target {
            bail!(
                "checkpoint height {} is still below export target {}; check SYNC_HEIGHT / voting snapshot",
                ch,
                export_target
            );
        }

        let tree_path = data_dir.join("nullifiers.tree");
        if let Ok(Some((_, hh))) = pir_export::read_tree_checkpoint_header(&tree_path) {
            if hh != export_target {
                eprintln!(
                    "Removing stale nullifiers.tree (on-disk checkpoint height {}, tree header {}, export target {})",
                    ch, hh, export_target
                );
                let _ = std::fs::remove_file(&tree_path);
            }
        }

        println!(
            "Stage 2/3: PIR Merkle tree (nullifiers.checkpoint height {ch}, export target {export_target})"
        );
        let data_dir_c = data_dir.to_path_buf();
        let pir_dir_c = pir_dir.to_path_buf();
        let export_target_c = export_target;
        let ch_c = ch;
        tokio::task::spawn_blocking(move || -> Result<()> {
            if pir_export::tiers_complete_for_height(&pir_dir_c, export_target_c)? {
                println!("Stage 3/3: PIR tier files already complete at height {export_target_c}");
                return Ok(());
            }

            let nfs = if ch_c > export_target_c {
                let (_idx_h, byte_off) = file_store::offset_for_height(&data_dir_c, export_target_c)?
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "nullifiers.index has no entry for export height {export_target_c} \
                             (checkpoint is {ch_c}); run a full sync so the index covers this height"
                        )
                    })?;
                file_store::load_nullifiers_up_to(&data_dir_c, byte_off)
                    .with_context(|| format!("load nullifiers up to byte offset {byte_off}"))?
            } else {
                file_store::load_all_nullifiers(&data_dir_c)?
            };
            println!("  Building/resuming tree and exporting tiers …");
            sync_pipeline::export_tree_and_tiers_from_nullifiers(
                nfs,
                &data_dir_c,
                &pir_dir_c,
                export_target_c,
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
