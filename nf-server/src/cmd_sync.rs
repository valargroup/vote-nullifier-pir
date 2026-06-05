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
/// Env: when `--non-interactive` and local checkpoint is ahead of the active
/// voting round snapshot height, must be exactly `RESYNC` to wipe artifacts and continue.
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
    for name in [
        "tier0.bin",
        "tier1.bin",
        "tier2.bin",
        "pir_root.json",
        // Precompute caches are derived from tier files; reset wipes them
        // too. Eviction at write-sites covers the common case; this catches
        // any cache files left behind from a prior sync that crashed before
        // the next eviction could fire.
        "tier1.precompute",
        "tier1.precompute.tmp",
        "tier2.precompute",
        "tier2.precompute.tmp",
    ] {
        let p = tier_dir.join(name);
        if p.exists() {
            std::fs::remove_file(&p).with_context(|| format!("remove {}", p.display()))?;
        }
    }
    Ok(())
}

fn prompt_resync_ahead_of_voting(local: u64, snap: u64, non_interactive: bool) -> Result<()> {
    eprintln!(
        "Local nullifier checkpoint height ({local}) is above active voting round snapshot height ({snap}).\n\
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

/// Resolve the byte offset to use for historical export.
///
/// Returns `Ok(None)` when current checkpoint is already at/behind target,
/// which means the caller should load the full `nullifiers.bin`.
/// Returns `Ok(Some(offset))` only when an exact index entry exists for
/// `export_target`; floor matches are rejected to prevent mislabeled exports.
fn resolve_export_byte_offset(
    data_dir: &Path,
    checkpoint_height: u64,
    export_target: u64,
) -> Result<Option<u64>> {
    if checkpoint_height <= export_target {
        return Ok(None);
    }

    let (idx_h, byte_off) = file_store::offset_for_height(data_dir, export_target)?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "nullifiers.index has no entry for export height {export_target} \
                 (checkpoint is {checkpoint_height}); run a full sync so the index covers this height"
            )
        })?;
    if idx_h != export_target {
        bail!(
            "nullifiers.index floor lookup returned height {} for export target {}; \
             exact height entry is required to bind exported contents to advertised snapshot height",
            idx_h,
            export_target
        );
    }

    Ok(Some(byte_off))
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
        default_value = "https://us.zec.stardust.rest:443",
        env = "SVOTE_PIR_MAINNET_RPC_URL"
    )]
    lwd_url: String,

    /// Stop syncing at this block height (must be a multiple of 10). Capped by
    /// chain tip and, when set, by the active voting round snapshot height.
    #[arg(long)]
    max_height: Option<u64>,

    /// Voting service-discovery URL. When non-empty, resolves `vote_servers`
    /// through the static/dynamic config flow and queries the chain's active
    /// round snapshot height, which caps the sync target. Empty disables this
    /// check (offline / dev).
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
    if lwd_urls.len() < 2 {
        bail!(
            "dual-provider range agreement requires at least 2 lightwalletd URLs (resolved {}); set LWD_URLS to two or more endpoints",
            lwd_urls.len()
        );
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

            let nfs = if let Some(byte_off) =
                resolve_export_byte_offset(&data_dir_c, ch_c, export_target_c)?
            {
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

#[cfg(test)]
mod tests {
    use super::resolve_export_byte_offset;
    use nf_ingest::file_store;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock before unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "nf_server_cmd_sync_{label}_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn resolve_export_offset_rejects_floor_match() {
        let dir = temp_dir("floor_reject");
        file_store::append_index(&dir, 100, 64).expect("append index 100");
        file_store::append_index(&dir, 120, 96).expect("append index 120");

        let err = resolve_export_byte_offset(&dir, 130, 125).expect_err("must reject floor");
        let msg = err.to_string();
        assert!(msg.contains("floor lookup returned height 120"));
        assert!(msg.contains("export target 125"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolve_export_offset_accepts_exact_match() {
        let dir = temp_dir("exact_ok");
        file_store::append_index(&dir, 100, 64).expect("append index 100");
        file_store::append_index(&dir, 120, 96).expect("append index 120");

        let offset = resolve_export_byte_offset(&dir, 130, 120).expect("lookup succeeds");
        assert_eq!(offset, Some(96));

        let _ = std::fs::remove_dir_all(&dir);
    }
}
