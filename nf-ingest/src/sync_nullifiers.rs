use std::path::Path;

use anyhow::{bail, Context, Result};
use tonic::transport::Channel;
use tonic::Request;
use tracing::info;

use crate::download::connect_lwd;
use crate::file_store;
use crate::rpc::compact_tx_streamer_client::CompactTxStreamerClient;
use crate::rpc::{BlockId, BlockRange, ChainSpec};

/// NU5 (Orchard) activation height on Zcash mainnet.
pub const NU5_ACTIVATION_HEIGHT: u64 = 1_687_104;

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
/// If no checkpoint exists, starts from NU5 activation.
pub fn resume_height(dir: &Path) -> Result<u64> {
    match file_store::load_checkpoint(dir)? {
        Some((h, offset)) if h >= NU5_ACTIVATION_HEIGHT => {
            file_store::truncate_to_checkpoint(dir, offset)?;
            Ok(h)
        }
        _ => Ok(NU5_ACTIVATION_HEIGHT),
    }
}

/// Stream blocks `[start, end]` from a single server and return collected
/// `(height, nullifier)` pairs.
fn observe_range_height(
    start: u64,
    end: u64,
    next_expected: &mut u64,
    completed: &mut bool,
    height: u64,
) -> Result<()> {
    if *completed {
        bail!(
            "received extra block {} after completing requested range [{}..={}]",
            height,
            start,
            end
        );
    }
    if height < start || height > end {
        bail!(
            "received out-of-range block {} for requested range [{}..={}]",
            height,
            start,
            end
        );
    }
    if height != *next_expected {
        bail!(
            "non-contiguous block stream for range [{}..={}]: expected {}, got {}",
            start,
            end,
            *next_expected,
            height
        );
    }

    if *next_expected == end {
        *completed = true;
    } else {
        *next_expected += 1;
    }
    Ok(())
}

fn ensure_range_complete(start: u64, end: u64, completed: bool, last_seen: Option<u64>) -> Result<()> {
    if completed {
        return Ok(());
    }

    match last_seen {
        Some(height) => bail!(
            "incomplete block stream for range [{}..={}]: stream ended at {}",
            start,
            end,
            height
        ),
        None => bail!(
            "incomplete block stream for range [{}..={}]: server returned no blocks",
            start,
            end
        ),
    }
}

async fn fetch_block_range(
    client: &mut CompactTxStreamerClient<Channel>,
    start: u64,
    end: u64,
) -> Result<Vec<(u64, Vec<u8>)>> {
    if start > end {
        bail!("invalid block range: start {} is greater than end {}", start, end);
    }

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
    let mut next_expected = start;
    let mut completed = false;
    let mut last_seen_height: Option<u64> = None;
    while let Some(block) = stream.message().await? {
        observe_range_height(
            start,
            end,
            &mut next_expected,
            &mut completed,
            block.height,
        )?;
        last_seen_height = Some(block.height);

        for tx in block.vtx {
            for a in tx.actions {
                nf_buffer.push((block.height, a.nullifier));
            }
        }
    }
    ensure_range_complete(start, end, completed, last_seen_height)?;
    Ok(nf_buffer)
}

/// Compare two providers' canonical range payloads and return the agreed payload.
///
/// This is intentionally fail-closed: any first divergence in height ordering,
/// nullifier bytes, or payload length aborts the range before checkpoint commit.
fn compare_range_payloads(
    start: u64,
    end: u64,
    provider_a_url: &str,
    provider_a_payload: Vec<(u64, Vec<u8>)>,
    provider_b_url: &str,
    provider_b_payload: Vec<(u64, Vec<u8>)>,
) -> Result<Vec<(u64, Vec<u8>)>> {
    if provider_a_payload == provider_b_payload {
        return Ok(provider_a_payload);
    }

    let shared_len = std::cmp::min(provider_a_payload.len(), provider_b_payload.len());
    for i in 0..shared_len {
        let (a_height, a_nf) = &provider_a_payload[i];
        let (b_height, b_nf) = &provider_b_payload[i];
        if a_height != b_height {
            bail!(
                "provider mismatch for range [{}..={}]: first height mismatch at index {} ({}:{} vs {}:{})",
                start,
                end,
                i,
                provider_a_url,
                a_height,
                provider_b_url,
                b_height
            );
        }
        if a_nf != b_nf {
            bail!(
                "provider mismatch for range [{}..={}]: first nullifier mismatch at height {} index {} ({} vs {})",
                start,
                end,
                a_height,
                i,
                provider_a_url,
                provider_b_url
            );
        }
    }

    if provider_a_payload.len() != provider_b_payload.len() {
        let extra = if provider_a_payload.len() > provider_b_payload.len() {
            provider_a_payload[shared_len].0
        } else {
            provider_b_payload[shared_len].0
        };
        bail!(
            "provider mismatch for range [{}..={}]: payload length differs ({}={} rows vs {}={} rows), first extra height {}",
            start,
            end,
            provider_a_url,
            provider_a_payload.len(),
            provider_b_url,
            provider_b_payload.len(),
            extra
        );
    }

    unreachable!("equal length and all entries matched should have returned early");
}

/// Fetch the same block range from two providers and require exact agreement.
///
/// Both requests execute concurrently to minimize added latency from redundancy.
async fn fetch_and_compare_range(
    primary_client: &mut CompactTxStreamerClient<Channel>,
    primary_url: &str,
    secondary_client: &mut CompactTxStreamerClient<Channel>,
    secondary_url: &str,
    start: u64,
    end: u64,
) -> Result<Vec<(u64, Vec<u8>)>> {
    let (primary_res, secondary_res) = tokio::join!(
        fetch_block_range(primary_client, start, end),
        fetch_block_range(secondary_client, start, end),
    );
    let primary_payload = primary_res.with_context(|| {
        format!(
            "fetch range [{}..={}] from primary provider {}",
            start, end, primary_url
        )
    })?;
    let secondary_payload = secondary_res.with_context(|| {
        format!(
            "fetch range [{}..={}] from secondary provider {}",
            start, end, secondary_url
        )
    })?;
    compare_range_payloads(
        start,
        end,
        primary_url,
        primary_payload,
        secondary_url,
        secondary_payload,
    )
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
/// per server), and appends all Orchard nullifiers to the data file.  Calls
/// `progress` after each parallel cycle with
/// `(last_height, target_height, cycle_nullifier_count, total_nullifier_count)`.
pub async fn sync(
    dir: &Path,
    lwd_urls: &[String],
    max_height: Option<u64>,
    progress: impl Fn(u64, u64, u64, u64),
) -> Result<SyncResult> {
    std::fs::create_dir_all(dir)?;
    if lwd_urls.len() < 2 {
        bail!(
            "dual-provider range agreement requires at least 2 lightwalletd URLs, got {}",
            lwd_urls.len()
        );
    }

    let mut clients = Vec::with_capacity(lwd_urls.len());
    for url in lwd_urls {
        clients.push(connect_lwd(url).await?);
    }
    let n = clients.len();

    let latest = clients[0]
        .get_latest_block(Request::new(ChainSpec {}))
        .await?;
    let chain_tip = latest.into_inner().height;

    let start = resume_height(dir)?;
    let existing = file_store::nullifier_count(dir)?;
    let target = resolve_target(start, max_height, chain_tip);

    if start > NU5_ACTIVATION_HEIGHT {
        info!(height = start, existing, "resuming from checkpoint");
    } else {
        info!(height = NU5_ACTIVATION_HEIGHT, "starting fresh from NU5 activation");
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
            let primary_idx = i % n;
            let secondary_idx = (primary_idx + 1) % n;
            let mut primary_client = clients[primary_idx].clone();
            let mut secondary_client = clients[secondary_idx].clone();
            let primary_url = lwd_urls[primary_idx].clone();
            let secondary_url = lwd_urls[secondary_idx].clone();
            // Pair each range with two distinct providers; the cycle commits only
            // after all range pairs agree.
            handles.push(tokio::spawn(async move {
                fetch_and_compare_range(
                    &mut primary_client,
                    &primary_url,
                    &mut secondary_client,
                    &secondary_url,
                    range_start,
                    range_end,
                )
                .await
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
    /// Number of Orchard nullifiers appended in this sync cycle.
    pub nullifiers_synced: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        crate::test_helpers::temp_dir("sync", name)
    }

    #[test]
    fn resume_height_fresh() {
        let dir = temp_dir("fresh");
        assert_eq!(resume_height(&dir).unwrap(), NU5_ACTIVATION_HEIGHT);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn range_coverage_accepts_exact_contiguous_sequence() {
        let start = 100u64;
        let end = 103u64;
        let mut next_expected = start;
        let mut completed = false;
        let mut last_seen = None;

        for h in [100u64, 101, 102, 103] {
            observe_range_height(start, end, &mut next_expected, &mut completed, h).unwrap();
            last_seen = Some(h);
        }
        ensure_range_complete(start, end, completed, last_seen).unwrap();
    }

    #[test]
    fn range_coverage_rejects_empty_response() {
        let err = ensure_range_complete(100, 103, false, None).unwrap_err();
        assert!(err
            .to_string()
            .contains("server returned no blocks"));
    }

    #[test]
    fn range_coverage_rejects_gap() {
        let start = 100u64;
        let end = 103u64;
        let mut next_expected = start;
        let mut completed = false;
        observe_range_height(start, end, &mut next_expected, &mut completed, 100).unwrap();
        let err = observe_range_height(start, end, &mut next_expected, &mut completed, 102).unwrap_err();
        assert!(err.to_string().contains("expected 101, got 102"));
    }

    #[test]
    fn range_coverage_rejects_duplicate() {
        let start = 100u64;
        let end = 103u64;
        let mut next_expected = start;
        let mut completed = false;
        observe_range_height(start, end, &mut next_expected, &mut completed, 100).unwrap();
        let err = observe_range_height(start, end, &mut next_expected, &mut completed, 100).unwrap_err();
        assert!(err.to_string().contains("expected 101, got 100"));
    }

    #[test]
    fn range_coverage_rejects_out_of_order() {
        let start = 100u64;
        let end = 103u64;
        let mut next_expected = start;
        let mut completed = false;
        observe_range_height(start, end, &mut next_expected, &mut completed, 100).unwrap();
        let err = observe_range_height(start, end, &mut next_expected, &mut completed, 103).unwrap_err();
        assert!(err.to_string().contains("expected 101, got 103"));
    }

    #[test]
    fn range_coverage_rejects_out_of_range() {
        let start = 100u64;
        let end = 103u64;
        let mut next_expected = start;
        let mut completed = false;
        let err = observe_range_height(start, end, &mut next_expected, &mut completed, 99).unwrap_err();
        assert!(err.to_string().contains("out-of-range block 99"));
    }

    #[test]
    fn resume_height_from_checkpoint() {
        let dir = temp_dir("resume");

        // Write some nullifiers and commit a checkpoint
        let nfs = vec![
            (1_700_000u64, vec![1u8; 32]),
            (1_700_000, vec![2u8; 32]),
            (1_700_001, vec![3u8; 32]),
        ];
        let offset = file_store::append_nullifiers(&dir, &nfs).unwrap();
        file_store::save_checkpoint(&dir, 1_700_001, offset).unwrap();

        let h = resume_height(&dir).unwrap();
        assert_eq!(h, 1_700_001);

        // All 3 nullifiers should still be present (checkpoint was exact)
        assert_eq!(file_store::nullifier_count(&dir).unwrap(), 3);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_height_truncates_uncommitted() {
        let dir = temp_dir("trunc");

        // Committed batch
        let batch1 = vec![(1_700_000u64, vec![1u8; 32]), (1_700_000, vec![2u8; 32])];
        let offset = file_store::append_nullifiers(&dir, &batch1).unwrap();
        file_store::save_checkpoint(&dir, 1_700_000, offset).unwrap();

        // Uncommitted partial batch (simulates crash)
        let batch2 = vec![(1_700_001u64, vec![3u8; 32])];
        file_store::append_nullifiers(&dir, &batch2).unwrap();
        assert_eq!(file_store::nullifier_count(&dir).unwrap(), 3);

        // resume_height should truncate back to the committed state
        let h = resume_height(&dir).unwrap();
        assert_eq!(h, 1_700_000);
        assert_eq!(file_store::nullifier_count(&dir).unwrap(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compare_range_payloads_accepts_identical_payloads() {
        let payload = vec![
            (100u64, vec![1u8; 32]),
            (100u64, vec![2u8; 32]),
            (101u64, vec![3u8; 32]),
        ];
        let agreed = compare_range_payloads(
            100,
            101,
            "https://a.example",
            payload.clone(),
            "https://b.example",
            payload,
        )
        .unwrap();
        assert_eq!(agreed.len(), 3);
    }

    #[test]
    fn compare_range_payloads_rejects_mismatch() {
        let provider_a = vec![(200u64, vec![9u8; 32])];
        let provider_b = vec![(200u64, vec![8u8; 32])];
        let err = compare_range_payloads(
            200,
            200,
            "https://a.example",
            provider_a,
            "https://b.example",
            provider_b,
        )
        .unwrap_err();
        assert!(err.to_string().contains("first nullifier mismatch"));
        assert!(err.to_string().contains("https://a.example"));
        assert!(err.to_string().contains("https://b.example"));
    }

    #[test]
    fn compare_range_payloads_rejects_height_mismatch() {
        let provider_a = vec![(300u64, vec![1u8; 32])];
        let provider_b = vec![(301u64, vec![1u8; 32])];
        let err = compare_range_payloads(
            300,
            301,
            "https://a.example",
            provider_a,
            "https://b.example",
            provider_b,
        )
        .unwrap_err();
        assert!(err.to_string().contains("first height mismatch"));
        assert!(err.to_string().contains("https://a.example:300"));
        assert!(err.to_string().contains("https://b.example:301"));
    }

    #[test]
    fn compare_range_payloads_rejects_length_mismatch() {
        let provider_a = vec![(400u64, vec![7u8; 32]), (401u64, vec![8u8; 32])];
        let provider_b = vec![(400u64, vec![7u8; 32])];
        let err = compare_range_payloads(
            400,
            401,
            "https://a.example",
            provider_a,
            "https://b.example",
            provider_b,
        )
        .unwrap_err();
        assert!(err.to_string().contains("payload length differs"));
        assert!(err.to_string().contains("first extra height 401"));
    }

    #[test]
    fn sync_rejects_single_provider() {
        let dir = temp_dir("single-provider-rejected");
        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(sync(
            &dir,
            &[String::from("https://only.example")],
            Some(NU5_ACTIVATION_HEIGHT + 10),
            |_, _, _, _| {},
        ));
        assert!(result.is_err());
        let err = result.err().expect("single-provider sync should fail");
        assert!(err
            .to_string()
            .contains("requires at least 2 lightwalletd URLs"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
