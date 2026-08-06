use std::path::Path;

use anyhow::{Context, Result};
use tonic::transport::Channel;
use tonic::Request;
use tracing::{info, warn};

use pir_types::ZcashNetwork;

use crate::download::connect_lwd;
use crate::file_store;
use crate::rpc::compact_tx_streamer_client::CompactTxStreamerClient;
use crate::rpc::{BlockId, BlockRange, ChainSpec, CompactBlock, TreeState};

/// How many blocks to request per gRPC streaming call.
const BATCH_SIZE: u64 = 10_000;

/// Block-height granularity used when aligning sync targets.
const BLOCK_ALIGNMENT: u64 = 10;

/// Fetch the current chain tip height from a lightwalletd server.
///
/// Connects to the given URL and calls `GetLatestBlock`. Returns the height
/// of the chain tip as reported by the server.
pub async fn fetch_chain_tip(lwd_url: &str) -> Result<u64> {
    let mut client = connect_lwd(lwd_url).await?;
    let latest = client
        .get_latest_block(Request::new(ChainSpec {}))
        .await?;
    Ok(latest.into_inner().height)
}

/// Determine the block height to resume syncing from.
///
/// Reads the checkpoint file and truncates any uncommitted bytes from
/// the data file, then returns the last fully-committed height.
/// If no checkpoint exists, returns the block before NU6.3 activation so the
/// first request includes the activation block.
pub fn resume_height(dir: &Path, network: ZcashNetwork) -> Result<u64> {
    let activation_height = crate::config::nu6_3_activation_height(network);
    file_store::ensure_ironwood_dataset(dir, network)?;
    match file_store::load_checkpoint(dir)? {
        Some((h, offset)) if h >= activation_height => {
            file_store::truncate_to_checkpoint(dir, offset)?;
            Ok(h)
        }
        Some((h, _)) => anyhow::bail!(
            "checkpoint height {h} predates NU6.3 activation; set SVOTE_PIR_SYNC_RESET=1 to rebuild"
        ),
        None => {
            file_store::truncate_to_checkpoint(dir, 0)?;
            Ok(activation_height - 1)
        }
    }
}

fn extract_ironwood_nullifiers(block: CompactBlock) -> Result<Vec<(u64, Vec<u8>)>> {
    let mut nullifiers = Vec::new();
    for tx in block.vtx {
        for action in tx.ironwood_actions {
            anyhow::ensure!(
                action.nullifier.len() == 32,
                "Ironwood nullifier at height {} has {} bytes; expected 32",
                block.height,
                action.nullifier.len()
            );
            nullifiers.push((block.height, action.nullifier));
        }
    }
    Ok(nullifiers)
}

fn validate_ironwood_tree_state(
    url: &str,
    expected_network: ZcashNetwork,
    expected_height: u64,
    state: &TreeState,
) -> Result<()> {
    anyhow::ensure!(
        state.network == expected_network.as_str(),
        "lightwalletd {url} returned network {:?}; expected {}",
        state.network,
        expected_network
    );
    anyhow::ensure!(
        state.height == expected_height,
        "lightwalletd {url} returned tree state height {}; expected {}",
        state.height,
        expected_height
    );
    anyhow::ensure!(
        !state.ironwood_tree.is_empty(),
        "lightwalletd {url} omitted the Ironwood tree at height {expected_height}; use a post-NU6.3 endpoint"
    );
    Ok(())
}

async fn require_ironwood_tree_state(
    client: &mut CompactTxStreamerClient<Channel>,
    url: &str,
    network: ZcashNetwork,
    height: u64,
) -> Result<()> {
    let state = client
        .get_tree_state(Request::new(BlockId {
            height,
            hash: vec![],
        }))
        .await
        .with_context(|| format!("get Ironwood tree state from {url} at height {height}"))?
        .into_inner();
    validate_ironwood_tree_state(url, network, height, &state)
}

/// Stream blocks `[start, end]` from a single server and return collected
/// `(height, nullifier)` pairs.
async fn fetch_block_range(
    client: &mut CompactTxStreamerClient<Channel>,
    start: u64,
    end: u64,
) -> Result<Vec<(u64, Vec<u8>)>> {
    let mut stream = client
        .get_block_range(Request::new(BlockRange {
            start: Some(BlockId {
                height: start,
                hash: vec![],
            }),
            end: Some(BlockId {
                height: end,
                hash: vec![],
            }),
            spam_filter_threshold: 0,
        }))
        .await?
        .into_inner();

    let mut nf_buffer: Vec<(u64, Vec<u8>)> = Vec::new();
    while let Some(block) = stream.message().await? {
        nf_buffer.extend(extract_ironwood_nullifiers(block)?);
    }
    Ok(nf_buffer)
}

/// Compute the effective sync target height, accounting for rewinds and chain tip.
fn resolve_target(start: u64, max_height: Option<u64>, chain_tip: u64) -> u64 {
    match max_height {
        Some(h) if h < start => {
            let next_multiple = ((start / BLOCK_ALIGNMENT) + 1) * BLOCK_ALIGNMENT;
            info!(
                sync_height = h,
                checkpoint = start,
                next_multiple,
                "SYNC_HEIGHT below checkpoint, advancing target"
            );
            std::cmp::min(next_multiple, chain_tip)
        }
        Some(h) => std::cmp::min(h, chain_tip),
        None => chain_tip,
    }
}

/// Partition `[current, target]` into up to `n` consecutive batch ranges of
/// at most [`BATCH_SIZE`] blocks each.
fn build_batch_ranges(current: u64, target: u64, n: usize) -> Vec<(u64, u64)> {
    let mut ranges = Vec::with_capacity(n);
    let mut batch_start = current;
    for _ in 0..n {
        if batch_start > target {
            break;
        }
        let batch_end = std::cmp::min(batch_start + BATCH_SIZE - 1, target);
        ranges.push((batch_start, batch_end));
        batch_start = batch_end + 1;
    }
    ranges
}

/// Sync nullifiers from multiple lightwalletd servers into flat files.
///
/// Connects to each URL in `lwd_urls`, streams blocks from the resume point to
/// `max_height` (or chain tip when `None`) using parallel downloads (one batch
/// per server), and appends all Ironwood nullifiers to the data file. Calls
/// `progress` after each parallel cycle with
/// `(last_height, target_height, cycle_nullifier_count, total_nullifier_count)`.
pub async fn sync(
    dir: &Path,
    lwd_urls: &[String],
    network: ZcashNetwork,
    max_height: Option<u64>,
    progress: impl Fn(u64, u64, u64, u64),
) -> Result<SyncResult> {
    anyhow::ensure!(
        !lwd_urls.is_empty(),
        "at least one lightwalletd URL is required"
    );
    file_store::ensure_ironwood_dataset(dir, network)?;

    let mut clients = Vec::with_capacity(lwd_urls.len());
    let mut connected_urls = Vec::with_capacity(lwd_urls.len());
    let mut connection_errors = Vec::new();
    for url in lwd_urls {
        match connect_lwd(url).await {
            Ok(client) => {
                clients.push(client);
                connected_urls.push(url.as_str());
            }
            Err(error) => {
                warn!(%url, %error, "skipping unavailable lightwalletd");
                connection_errors.push(format!("{url}: {error:#}"));
            }
        }
    }
    anyhow::ensure!(
        !clients.is_empty(),
        "failed to connect to any lightwalletd endpoint: {}",
        connection_errors.join("; ")
    );
    let n = clients.len();

    let latest = clients[0]
        .get_latest_block(Request::new(ChainSpec {}))
        .await?;
    let chain_tip = latest.into_inner().height;

    let activation_height = crate::config::nu6_3_activation_height(network);
    let start = resume_height(dir, network)?;
    let existing = file_store::nullifier_count(dir)?;
    let target = resolve_target(start, max_height, chain_tip);

    for (url, client) in connected_urls.iter().zip(clients.iter_mut()) {
        require_ironwood_tree_state(client, url, network, target).await?;
    }

    if start >= activation_height {
        info!(height = start, existing, "resuming from checkpoint");
    } else {
        info!(
            height = activation_height,
            "starting fresh from NU6.3 activation"
        );
    }
    if let Some(h) = max_height {
        info!(max_height = h, chain_tip, "max height set");
    }
    info!(target, blocks_remaining = target.saturating_sub(start), "sync target");

    if start >= target {
        return Ok(SyncResult {
            chain_tip,
            blocks_synced: 0,
            nullifiers_synced: 0,
        });
    }

    let mut current = start + 1;
    let mut total_nfs: u64 = 0;
    let mut blocks_synced: u64 = 0;

    while current <= target {
        let batch_ranges = build_batch_ranges(current, target, n);

        let mut handles = Vec::with_capacity(batch_ranges.len());
        for (i, &(range_start, range_end)) in batch_ranges.iter().enumerate() {
            let mut client = clients[i].clone();
            handles.push(tokio::spawn(async move {
                fetch_block_range(&mut client, range_start, range_end).await
            }));
        }

        let mut all_nfs: Vec<(u64, Vec<u8>)> = Vec::new();
        for handle in handles {
            all_nfs.extend(handle.await??);
        }
        let cycle_end = batch_ranges.last().expect("batch_ranges is non-empty").1;
        let cycle_nfs = all_nfs.len() as u64;

        let offset = file_store::append_nullifiers(dir, &all_nfs)?;
        file_store::save_checkpoint(dir, cycle_end, offset)?;

        drop(all_nfs);

        total_nfs += cycle_nfs;
        blocks_synced += cycle_end - current + 1;
        progress(cycle_end, target, cycle_nfs, total_nfs);

        current = cycle_end + 1;
    }

    Ok(SyncResult {
        chain_tip,
        blocks_synced,
        nullifiers_synced: total_nfs,
    })
}

/// Result of a sync operation.
pub struct SyncResult {
    /// Chain tip height as reported by the first lightwalletd server.
    pub chain_tip: u64,
    /// Number of blocks downloaded in this sync cycle.
    pub blocks_synced: u64,
    /// Number of Ironwood nullifiers appended in this sync cycle.
    pub nullifiers_synced: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAINNET_ACTIVATION: u64 = crate::config::NU6_3_MAINNET_ACTIVATION_HEIGHT;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        crate::test_helpers::temp_dir("sync", name)
    }

    #[test]
    fn resume_height_fresh() {
        let dir = temp_dir("fresh");
        assert_eq!(
            resume_height(&dir, ZcashNetwork::Main).unwrap(),
            MAINNET_ACTIVATION - 1
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_height_discards_uncheckpointed_first_batch() {
        let dir = temp_dir("fresh_partial");
        file_store::ensure_ironwood_dataset(&dir, ZcashNetwork::Main).unwrap();
        file_store::append_nullifiers(&dir, &[(MAINNET_ACTIVATION, vec![1u8; 32])])
            .unwrap();

        assert_eq!(
            resume_height(&dir, ZcashNetwork::Main).unwrap(),
            MAINNET_ACTIVATION - 1
        );
        assert_eq!(file_store::nullifier_count(&dir).unwrap(), 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compact_tx_decodes_ironwood_tag_nine() {
        use prost::Message;

        let mut encoded = vec![0x4a, 0x22, 0x0a, 0x20];
        encoded.extend([7u8; 32]);
        let tx = crate::rpc::CompactTx::decode(encoded.as_slice()).unwrap();

        assert!(tx.actions.is_empty());
        assert_eq!(tx.ironwood_actions.len(), 1);
        assert_eq!(tx.ironwood_actions[0].nullifier, vec![7u8; 32]);
    }

    #[test]
    fn extracts_only_ironwood_actions() {
        let mut tx = crate::rpc::CompactTx::default();
        tx.actions.push(crate::rpc::CompactOrchardAction {
            nullifier: vec![1u8; 32],
            ..Default::default()
        });
        tx.ironwood_actions.push(crate::rpc::CompactOrchardAction {
            nullifier: vec![2u8; 32],
            ..Default::default()
        });
        let mut block = CompactBlock {
            height: MAINNET_ACTIVATION,
            ..Default::default()
        };
        block.vtx.push(tx);

        let nullifiers = extract_ironwood_nullifiers(block).unwrap();
        assert_eq!(nullifiers, vec![(MAINNET_ACTIVATION, vec![2u8; 32])]);
    }

    #[test]
    fn rejects_malformed_ironwood_nullifier() {
        let mut tx = crate::rpc::CompactTx::default();
        tx.ironwood_actions.push(crate::rpc::CompactOrchardAction {
            nullifier: vec![2u8; 31],
            ..Default::default()
        });
        let mut block = CompactBlock {
            height: MAINNET_ACTIVATION,
            ..Default::default()
        };
        block.vtx.push(tx);

        let err = extract_ironwood_nullifiers(block).unwrap_err().to_string();
        assert!(err.contains("31 bytes"), "{err}");
    }

    #[test]
    fn tree_state_decodes_ironwood_tag_seven() {
        use prost::Message;

        let state = TreeState::decode([0x3a, 0x02, b'i', b'w'].as_slice()).unwrap();
        assert_eq!(state.ironwood_tree, "iw");
    }

    #[test]
    fn validates_ironwood_tree_state() {
        let state = TreeState {
            network: "main".to_owned(),
            height: MAINNET_ACTIVATION,
            ironwood_tree: "00".to_owned(),
            ..Default::default()
        };
        validate_ironwood_tree_state(
            "https://lwd.example",
            ZcashNetwork::Main,
            MAINNET_ACTIVATION,
            &state,
        )
        .unwrap();

        let mut legacy = state.clone();
        legacy.ironwood_tree.clear();
        let err = validate_ironwood_tree_state(
            "https://legacy.example",
            ZcashNetwork::Main,
            MAINNET_ACTIVATION,
            &legacy,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("legacy.example"), "{err}");
        assert!(err.contains("Ironwood"), "{err}");

        let mut wrong_network = state.clone();
        wrong_network.network = "test".to_owned();
        let err = validate_ironwood_tree_state(
            "https://testnet.example",
            ZcashNetwork::Main,
            MAINNET_ACTIVATION,
            &wrong_network,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("expected main"), "{err}");

        let mut wrong_height = state;
        wrong_height.height += 1;
        let err = validate_ironwood_tree_state(
            "https://lagging.example",
            ZcashNetwork::Main,
            MAINNET_ACTIVATION,
            &wrong_height,
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("expected 3428143"), "{err}");

        let testnet_state = TreeState {
            network: "test".to_owned(),
            height: crate::config::NU6_3_TESTNET_ACTIVATION_HEIGHT,
            ironwood_tree: "00".to_owned(),
            ..Default::default()
        };
        validate_ironwood_tree_state(
            "https://testnet.example",
            ZcashNetwork::Test,
            crate::config::NU6_3_TESTNET_ACTIVATION_HEIGHT,
            &testnet_state,
        )
        .unwrap();
    }

    #[test]
    fn resume_height_from_checkpoint() {
        let dir = temp_dir("resume");
        file_store::ensure_ironwood_dataset(&dir, ZcashNetwork::Main).unwrap();

        // Write some nullifiers and commit a checkpoint
        let nfs = vec![
            (3_500_000u64, vec![1u8; 32]),
            (3_500_000, vec![2u8; 32]),
            (3_500_001, vec![3u8; 32]),
        ];
        let offset = file_store::append_nullifiers(&dir, &nfs).unwrap();
        file_store::save_checkpoint(&dir, 3_500_001, offset).unwrap();

        let h = resume_height(&dir, ZcashNetwork::Main).unwrap();
        assert_eq!(h, 3_500_001);

        // All 3 nullifiers should still be present (checkpoint was exact)
        assert_eq!(file_store::nullifier_count(&dir).unwrap(), 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_height_truncates_uncommitted() {
        let dir = temp_dir("trunc");
        file_store::ensure_ironwood_dataset(&dir, ZcashNetwork::Main).unwrap();

        // Committed batch
        let batch1 = vec![(3_500_000u64, vec![1u8; 32]), (3_500_000, vec![2u8; 32])];
        let offset = file_store::append_nullifiers(&dir, &batch1).unwrap();
        file_store::save_checkpoint(&dir, 3_500_000, offset).unwrap();

        // Uncommitted partial batch (simulates crash)
        let batch2 = vec![(3_500_001u64, vec![3u8; 32])];
        file_store::append_nullifiers(&dir, &batch2).unwrap();
        assert_eq!(file_store::nullifier_count(&dir).unwrap(), 3);

        // resume_height should truncate back to the committed state
        let h = resume_height(&dir, ZcashNetwork::Main).unwrap();
        assert_eq!(h, 3_500_000);
        assert_eq!(file_store::nullifier_count(&dir).unwrap(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
