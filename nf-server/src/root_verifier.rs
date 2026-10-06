//! Content authentication against a caller-authenticated snapshot block hash.
//!
//! This is not consensus validation. The caller authenticates the network, height,
//! and ending hash together. Transaction effects (including Ironwood nullifiers)
//! are bound through txids, transaction Merkle roots, and predecessor headers.
//! V5/V6 authorizing data is outside that commitment and is not verified here.

use std::collections::HashSet;

use anyhow::{ensure, Context, Result};
use pir_types::ZcashNetwork;

use crate::chain_backend::{self, BlockHash, MAX_BLOCK_BYTES};

/// Effects of one block authenticated against the expected hash and height.
#[derive(Debug)]
pub(crate) struct VerifiedBlock {
    /// Authenticated predecessor, to use as the next request's expected hash.
    pub(crate) previous_hash: BlockHash,
    /// All Ironwood action nullifiers, in transaction/action order, as canonical bytes.
    pub(crate) nullifiers: Vec<[u8; 32]>,
}

/// Authenticate a complete block's effects and return its Ironwood nullifiers.
///
/// Rejects size/encoding errors, trailing bytes, wrong hash or coinbase height,
/// empty transaction lists, duplicate txids, and a mismatched transaction root.
/// Does not validate PoW, chain selection, transaction signatures, or ZK proofs.
/// No I/O or persistent state is used; the expected hash must be authenticated.
/// `network` supplies the consensus parameters that the decoder may need.
pub(crate) fn verify_block(
    raw: &[u8],
    network: ZcashNetwork,
    expected_hash: BlockHash,
    expected_height: u64,
) -> Result<VerifiedBlock> {
    ensure!(raw.len() <= MAX_BLOCK_BYTES, "raw block exceeds size limit");
    let block = chain_backend::decode_block(raw, network).context("decode raw block")?;
    ensure!(block.encoded_len == raw.len(), "trailing bytes after block");
    ensure!(
        block.hash == expected_hash,
        "block hash mismatch at height {expected_height}"
    );
    ensure!(
        block.coinbase_height == Some(expected_height),
        "coinbase height mismatch at height {expected_height}"
    );
    ensure!(!block.txids.is_empty(), "empty transaction list");

    let unique: HashSet<_> = block.txids.iter().collect();
    // The Bitcoin-style Merkle tree duplicates its last odd leaf. Reject actual
    // duplicate txids so a provider cannot exploit CVE-2012-2459's ambiguity.
    ensure!(
        unique.len() == block.txids.len(),
        "duplicate transaction IDs"
    );
    ensure!(
        chain_backend::transaction_merkle_root(&block.txids) == Some(block.merkle_root),
        "transaction Merkle root mismatch at height {expected_height}"
    );

    Ok(VerifiedBlock {
        previous_hash: block.previous_hash,
        nullifiers: block.ironwood_nullifiers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chain_backend::{
        test_blocks::{replace_unique, FixtureBlock},
        TxId,
    };
    use hex::FromHex;

    const RAW: &[u8] = include_bytes!("../tests/fixtures/verify-root/mainnet-3428150.bin");
    const HEIGHT: u64 = 3_428_150;
    const NETWORK: ZcashNetwork = ZcashNetwork::Main;
    /// Consecutive mainnet blocks from the NU6.3 activation height through `HEIGHT`.
    const CHAIN: [&[u8]; 8] = [
        include_bytes!("../tests/fixtures/verify-root/mainnet-3428143.bin"),
        include_bytes!("../tests/fixtures/verify-root/mainnet-3428144.bin"),
        include_bytes!("../tests/fixtures/verify-root/mainnet-3428145.bin"),
        include_bytes!("../tests/fixtures/verify-root/mainnet-3428146.bin"),
        include_bytes!("../tests/fixtures/verify-root/mainnet-3428147.bin"),
        include_bytes!("../tests/fixtures/verify-root/mainnet-3428148.bin"),
        include_bytes!("../tests/fixtures/verify-root/mainnet-3428149.bin"),
        RAW,
    ];

    fn metadata() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/verify-root/mainnet-3428150.json"
        ))
        .unwrap()
    }

    fn hash() -> BlockHash {
        BlockHash::from_hex(metadata()["hash"].as_str().unwrap()).unwrap()
    }

    /// Hex of a serialized-order hash, in RPC display order.
    fn display_hex(bytes: [u8; 32]) -> String {
        bytes.iter().rev().map(|b| format!("{b:02x}")).collect()
    }

    #[test]
    fn mainnet_ironwood_known_answers_and_pool_separation() {
        let block = chain_backend::decode_block(RAW, NETWORK).unwrap();
        let meta = metadata();
        assert_eq!(block.hash, hash());
        assert_eq!(block.hash.to_string(), meta["hash"].as_str().unwrap());
        assert_eq!(
            display_hex(block.merkle_root),
            meta["merkle_root"].as_str().unwrap()
        );
        let txids: Vec<_> = block
            .txids
            .iter()
            .map(|txid| display_hex(txid.serialized_bytes()))
            .collect();
        assert_eq!(serde_json::to_value(txids).unwrap(), meta["txids"]);
        let fixture = FixtureBlock::parse(RAW);
        assert_eq!(
            fixture
                .transactions
                .iter()
                .map(|tx| tx.orchard_actions)
                .sum::<usize>(),
            14
        );
        assert!(fixture
            .transactions
            .iter()
            .any(|tx| tx.ironwood_actions > 1));
        let verified = verify_block(RAW, NETWORK, hash(), HEIGHT).unwrap();
        let nfs: Vec<_> = verified.nullifiers.iter().map(hex::encode).collect();
        assert_eq!(
            serde_json::to_value(nfs).unwrap(),
            meta["ironwood_nullifiers"]
        );
        assert_eq!(verified.nullifiers.len(), 12);
    }

    #[test]
    fn activation_block_with_no_ironwood_actions() {
        let raw = include_bytes!("../tests/fixtures/verify-root/mainnet-3428143.bin");
        let meta: serde_json::Value = serde_json::from_str(include_str!(
            "../tests/fixtures/verify-root/mainnet-3428143.json"
        ))
        .unwrap();
        let hash = BlockHash::from_hex(meta["hash"].as_str().unwrap()).unwrap();
        assert!(verify_block(raw, NETWORK, hash, 3_428_143)
            .unwrap()
            .nullifiers
            .is_empty());
    }

    #[test]
    fn transaction_merkle_root_matches_every_fixture_header() {
        for raw in CHAIN {
            let block = chain_backend::decode_block(raw, NETWORK).unwrap();
            assert_eq!(
                chain_backend::transaction_merkle_root(&block.txids),
                Some(block.merkle_root)
            );
        }
        assert_eq!(chain_backend::transaction_merkle_root(&[]), None);
        let only = TxId::from_serialized_bytes([7; 32]);
        assert_eq!(
            chain_backend::transaction_merkle_root(&[only]),
            Some(only.serialized_bytes())
        );
    }

    #[test]
    fn ironwood_extraction_agrees_with_independently_downloaded_compact_nullifiers() {
        let snapshot: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/verify-root/snapshot.json"))
                .unwrap();
        let activation = nf_ingest::config::nu6_3_activation_height(NETWORK);
        let chain: Vec<_> = (activation..).zip(CHAIN).collect();
        let mut expected_hash = BlockHash::from_hex(snapshot["hash"].as_str().unwrap()).unwrap();
        let mut actual = Vec::new();
        for (height, raw) in chain.into_iter().rev() {
            let verified = verify_block(raw, NETWORK, expected_hash, height).unwrap();
            expected_hash = verified.previous_hash;
            actual.extend(verified.nullifiers.iter().map(hex::encode));
        }
        let mut expected: Vec<_> = snapshot["nullifiers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_owned())
            .collect();
        actual.sort();
        expected.sort();
        assert_eq!(actual, expected);
    }

    #[test]
    fn rejects_omitted_transaction_with_genuine_header() {
        let mut block = FixtureBlock::parse(RAW);
        block.remove_first_transaction_with_ironwood_actions();
        let err = verify_block(&block.to_bytes(), NETWORK, hash(), HEIGHT).unwrap_err();
        assert!(err.to_string().contains("Merkle root mismatch"), "{err:#}");
    }

    #[test]
    fn rejects_modified_nullifier_with_genuine_header() {
        let first = verify_block(RAW, NETWORK, hash(), HEIGHT)
            .unwrap()
            .nullifiers[0];
        let forged = replace_unique(RAW, &first, &[0; 32]);
        let err = verify_block(&forged, NETWORK, hash(), HEIGHT).unwrap_err();
        assert!(err.to_string().contains("Merkle root mismatch"), "{err:#}");
    }

    #[test]
    fn rejects_omitted_action_even_with_well_formed_remaining_bundle() {
        let mut block = FixtureBlock::parse(RAW);
        // Keep the provider's forged body structurally parseable. Proof validity is
        // intentionally not the property under test here.
        block.remove_first_action_of_multi_action_ironwood_bundle();
        let err = verify_block(&block.to_bytes(), NETWORK, hash(), HEIGHT).unwrap_err();
        assert!(err.to_string().contains("Merkle root mismatch"), "{err:#}");
    }

    #[test]
    fn rejects_duplicate_last_transaction_even_when_merkle_root_is_unchanged() {
        let decoded = chain_backend::decode_block(RAW, NETWORK).unwrap();
        let mut block = FixtureBlock::parse(RAW);
        assert_eq!(block.transactions.len() % 2, 1);
        block.duplicate_last_transaction();
        let mut txids = decoded.txids.clone();
        txids.push(*txids.last().unwrap());
        assert_eq!(
            chain_backend::transaction_merkle_root(&txids),
            Some(decoded.merkle_root)
        );
        assert!(verify_block(&block.to_bytes(), NETWORK, hash(), HEIGHT)
            .unwrap_err()
            .to_string()
            .contains("duplicate transaction"));
    }

    #[test]
    fn rejects_invalid_anchor_height_encoding_and_trailing_data() {
        let zero_hash = BlockHash::from_serialized_bytes([0; 32]);
        assert!(verify_block(RAW, NETWORK, zero_hash, HEIGHT)
            .unwrap_err()
            .to_string()
            .contains("hash mismatch"));
        assert!(verify_block(RAW, NETWORK, hash(), HEIGHT + 1)
            .unwrap_err()
            .to_string()
            .contains("height mismatch"));
        assert!(verify_block(&RAW[..RAW.len() - 1], NETWORK, hash(), HEIGHT).is_err());
        assert!(verify_block(&[], NETWORK, hash(), HEIGHT).is_err());
        assert!(verify_block(&vec![0; MAX_BLOCK_BYTES + 1], NETWORK, hash(), HEIGHT).is_err());
        let mut trailing = RAW.to_vec();
        trailing.push(0);
        assert!(verify_block(&trailing, NETWORK, hash(), HEIGHT)
            .unwrap_err()
            .to_string()
            .contains("trailing bytes"));
        let mut block = FixtureBlock::parse(RAW);
        block.set_previous_hash([0; 32]);
        assert!(verify_block(&block.to_bytes(), NETWORK, hash(), HEIGHT)
            .unwrap_err()
            .to_string()
            .contains("hash mismatch"));
    }
}
