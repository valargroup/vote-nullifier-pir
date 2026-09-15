//! Independently rebuild the Ironwood root for an authenticated chain snapshot.
//! This command has no dependency on compact-stream ingestion or its disk caches.

use std::path::PathBuf;

use anyhow::{ensure, Context, Result};
use clap::Args as ClapArgs;
use hex::FromHex;
use pir_types::ZcashNetwork;
use voting_crypto_deps::pasta_curves::{group::ff::PrimeField, Fp};
use zakura_chain::block;

use crate::{raw_block_rpc::RawBlockRpc, root_verifier::verify_block};

#[derive(ClapArgs)]
pub(crate) struct Args {
    /// Network belonging to the independently authenticated snapshot.
    #[arg(long)]
    zcash_network: ZcashNetwork,
    /// Exact snapshot height (at/after Ironwood activation and divisible by ten).
    #[arg(long)]
    height: u64,
    /// Independently authenticated ending block hash, in RPC display hex order.
    #[arg(long)]
    trusted_block_hash: String,
    /// Proposed depth-29 circuit root, in pir_root.json canonical field hex order.
    #[arg(long)]
    expected_circuit_root: String,
    /// Explicit raw-block JSON-RPC source; may be untrusted. No LWD env overrides.
    #[arg(long)]
    block_rpc_url: String,
    /// Optional node RPC cookie file containing user:password.
    #[arg(long)]
    block_rpc_cookie_file: Option<PathBuf>,
    /// Timeout per raw-block RPC attempt; transient failures get at most 3 attempts.
    #[arg(long, default_value_t = 30)]
    http_timeout_secs: u64,
}

/// Verify exactly activation..=height and print one JSON result on completion.
///
/// The caller must independently authenticate the network/height/block-hash tuple.
/// Returns an error on any incomplete/invalid history or root mismatch. A mismatch
/// still prints both roots with `matches: false`; other failures print no result.
/// Reads an optional RPC cookie and uses network I/O and RAM, but writes no files.
pub(crate) async fn run(args: Args) -> Result<()> {
    nf_ingest::config::validate_export_height(args.height, args.zcash_network)?;
    let trusted_hash = block::Hash::from_hex(&args.trusted_block_hash)
        .context("trusted block hash must be 32 bytes of RPC display-order hex")?;
    let expected_root = parse_root(&args.expected_circuit_root)?;
    let rpc = RawBlockRpc::new(
        &args.block_rpc_url,
        args.block_rpc_cookie_file.as_deref(),
        args.http_timeout_secs,
    )?;
    let activation = nf_ingest::config::nu6_3_activation_height(args.zcash_network);
    let mut expected_hash = trusted_hash;
    let mut nullifiers = Vec::new();
    let block_count = args.height - activation + 1;

    for height in (activation..=args.height).rev() {
        let raw = rpc
            .fetch(expected_hash)
            .await
            .with_context(|| format!("fetch block at height {height}"))?;
        let verified = verify_block(&raw, expected_hash, height)?;
        expected_hash = verified.previous_hash;
        nullifiers.extend(verified.nullifiers);
        // This v1 keeps action occurrences in memory. Bound them to the current
        // tree's approximate capacity; the tree builder checks exact range capacity.
        ensure!(
            nullifiers.len() <= 2 * (1 << pir_types::PIR_DEPTH),
            "Ironwood action count exceeds verifier capacity"
        );
        if height == activation || (args.height - height).is_multiple_of(100) {
            eprintln!(
                "Verified {}/{} blocks; {} Ironwood actions",
                args.height - height + 1,
                block_count,
                nullifiers.len()
            );
        }
    }

    let action_count = nullifiers.len();
    let tree = tokio::task::spawn_blocking(move || -> Result<_> {
        let bytes: Vec<u8> = nullifiers.into_iter().flatten().collect();
        let fields = nf_ingest::file_store::parse_nullifier_bytes(&bytes)?;
        pir_export::build_pir_tree_from_nullifiers(fields)
    })
    .await
    .context("join root construction")??;
    let matches = tree.circuit_root == expected_root;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "zcash_network": args.zcash_network,
            "nullifier_pool": pir_types::NULLIFIER_POOL,
            "dataset_version": pir_types::DATASET_VERSION,
            "height": args.height,
            "trusted_block_hash": trusted_hash.to_string(),
            "expected_circuit_root": hex::encode(expected_root.to_repr()),
            "computed_circuit_root": hex::encode(tree.circuit_root.to_repr()),
            "computed_pir_root": hex::encode(tree.pir_root.to_repr()),
            "verified_blocks": block_count,
            "ironwood_actions": action_count,
            "matches": matches,
        }))?
    );
    ensure!(
        matches,
        "proposed circuit root does not match the authenticated Ironwood dataset"
    );
    Ok(())
}

/// Parse exactly 32 bytes of canonical Pallas field representation, without reduction.
fn parse_root(encoded: &str) -> Result<Fp> {
    let bytes: [u8; 32] =
        <[u8; 32]>::from_hex(encoded).context("circuit root must be 32 bytes of hex")?;
    Option::from(Fp::from_repr(bytes)).context("circuit root is not a canonical field element")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_root_encoding() {
        let root = Fp::from(123);
        assert_eq!(parse_root(&hex::encode(root.to_repr())).unwrap(), root);
        for encoded in [
            "".to_owned(),
            "00".repeat(31),
            "ff".repeat(32),
            "zz".repeat(32),
        ] {
            assert!(parse_root(&encoded).is_err());
        }
    }
}
