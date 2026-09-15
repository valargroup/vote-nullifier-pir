//! Content authentication against a caller-authenticated snapshot block hash.
//!
//! This is not consensus validation. The caller authenticates the network, height,
//! and ending hash together. Transaction effects (including Ironwood nullifiers)
//! are bound through txids, transaction Merkle roots, and predecessor headers.
//! V5/V6 authorizing data is outside that commitment and is not verified here.

use std::{collections::HashSet, io::Cursor};

use anyhow::{ensure, Context, Result};
use zakura_chain::{
    block::{self, Block},
    serialization::ZcashDeserialize,
};

/// Maximum serialized block size supported by the pinned protocol implementation.
pub(crate) const MAX_BLOCK_BYTES: usize = zakura_chain::block::MAX_BLOCK_BYTES as usize;

/// Effects of one block authenticated against the expected hash and height.
#[derive(Debug)]
pub(crate) struct VerifiedBlock {
    /// Authenticated predecessor, to use as the next request's expected hash.
    pub(crate) previous_hash: block::Hash,
    /// All Ironwood action nullifiers, in transaction/action order, as canonical bytes.
    pub(crate) nullifiers: Vec<[u8; 32]>,
}

/// Authenticate a complete block's effects and return its Ironwood nullifiers.
///
/// Rejects size/encoding errors, trailing bytes, wrong hash or coinbase height,
/// empty transaction lists, duplicate txids, and a mismatched transaction root.
/// Does not validate PoW, chain selection, transaction signatures, or ZK proofs.
/// No I/O or persistent state is used; the expected hash must be authenticated.
pub(crate) fn verify_block(
    raw: &[u8],
    expected_hash: block::Hash,
    expected_height: u64,
) -> Result<VerifiedBlock> {
    ensure!(raw.len() <= MAX_BLOCK_BYTES, "raw block exceeds size limit");
    let mut cursor = Cursor::new(raw);
    let block = Block::zcash_deserialize(&mut cursor).context("decode raw block")?;
    ensure!(
        cursor.position() == raw.len() as u64,
        "trailing bytes after block"
    );
    ensure!(
        block.hash() == expected_hash,
        "block hash mismatch at height {expected_height}"
    );
    ensure!(
        block.coinbase_height().map(|h| u64::from(h.0)) == Some(expected_height),
        "coinbase height mismatch at height {expected_height}"
    );
    ensure!(!block.transactions.is_empty(), "empty transaction list");

    let txids: Vec<_> = block.transactions.iter().map(|tx| tx.hash()).collect();
    let unique: HashSet<_> = txids.iter().collect();
    // The Bitcoin-style Merkle tree duplicates its last odd leaf. Reject actual
    // duplicate txids so a provider cannot exploit CVE-2012-2459's ambiguity.
    ensure!(unique.len() == txids.len(), "duplicate transaction IDs");
    let root: block::merkle::Root = txids.into_iter().collect();
    ensure!(
        root == block.header.merkle_root,
        "transaction Merkle root mismatch at height {expected_height}"
    );

    Ok(VerifiedBlock {
        previous_hash: block.header.previous_block_hash,
        nullifiers: block.ironwood_nullifiers().map(|nf| (*nf).into()).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex::{FromHex, ToHex};
    use std::sync::Arc;
    use zakura_chain::{serialization::ZcashSerialize, transaction::Transaction};

    const RAW: &[u8] = include_bytes!("../tests/fixtures/verify-root/mainnet-3428150.bin");
    const HEIGHT: u64 = 3_428_150;

    fn metadata() -> serde_json::Value {
        serde_json::from_str(include_str!(
            "../tests/fixtures/verify-root/mainnet-3428150.json"
        ))
        .unwrap()
    }

    fn hash() -> block::Hash {
        block::Hash::from_hex(metadata()["hash"].as_str().unwrap()).unwrap()
    }

    fn parsed() -> Block {
        Block::zcash_deserialize(RAW).unwrap()
    }

    fn serialized(block: &Block) -> Vec<u8> {
        let mut bytes = Vec::new();
        block.zcash_serialize(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn mainnet_ironwood_known_answers_and_pool_separation() {
        let block = parsed();
        let meta = metadata();
        assert_eq!(block.hash(), hash());
        assert_eq!(
            block.header.merkle_root.encode_hex::<String>(),
            meta["merkle_root"].as_str().unwrap()
        );
        let txids: Vec<_> = block
            .transactions
            .iter()
            .map(|tx| tx.hash().to_string())
            .collect();
        assert_eq!(serde_json::to_value(txids).unwrap(), meta["txids"]);
        assert_eq!(block.orchard_nullifiers().count(), 14);
        assert!(block
            .transactions
            .iter()
            .any(|tx| tx.ironwood_actions().count() > 1));
        let verified = verify_block(RAW, hash(), HEIGHT).unwrap();
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
        let hash = block::Hash::from_hex(meta["hash"].as_str().unwrap()).unwrap();
        assert!(verify_block(raw, hash, 3_428_143)
            .unwrap()
            .nullifiers
            .is_empty());
    }

    #[test]
    fn rejects_omitted_transaction_with_genuine_header() {
        let mut block = parsed();
        let i = block
            .transactions
            .iter()
            .position(|tx| tx.ironwood_actions().count() > 0)
            .unwrap();
        block.transactions.remove(i);
        let err = verify_block(&serialized(&block), hash(), HEIGHT).unwrap_err();
        assert!(err.to_string().contains("Merkle root mismatch"), "{err:#}");
    }

    #[test]
    fn rejects_modified_nullifier_with_genuine_header() {
        let mut block = parsed();
        for tx in &mut block.transactions {
            if let Transaction::V6 {
                ironwood_shielded_data: Some(bundle),
                ..
            } = Arc::make_mut(tx)
            {
                bundle.actions.iter_mut().next().unwrap().action.nullifier =
                    [0; 32].try_into().unwrap();
                break;
            }
        }
        let err = verify_block(&serialized(&block), hash(), HEIGHT).unwrap_err();
        assert!(err.to_string().contains("Merkle root mismatch"), "{err:#}");
    }

    #[test]
    fn rejects_omitted_action_even_with_well_formed_remaining_bundle() {
        let mut block = parsed();
        for tx in &mut block.transactions {
            if let Transaction::V6 {
                ironwood_shielded_data: Some(bundle),
                ..
            } = Arc::make_mut(tx)
            {
                if bundle.actions.len() > 1 {
                    bundle.actions = bundle
                        .actions
                        .iter()
                        .skip(1)
                        .cloned()
                        .collect::<Vec<_>>()
                        .try_into()
                        .unwrap();
                    // Keep the provider's forged body structurally parseable. Proof
                    // validity is intentionally not the property under test here.
                    bundle.proof.0.truncate(bundle.proof.0.len() - 2272);
                    break;
                }
            }
        }
        let err = verify_block(&serialized(&block), hash(), HEIGHT).unwrap_err();
        assert!(err.to_string().contains("Merkle root mismatch"), "{err:#}");
    }

    #[test]
    fn rejects_duplicate_last_transaction_even_when_merkle_root_is_unchanged() {
        let mut block = parsed();
        assert_eq!(block.transactions.len() % 2, 1);
        block
            .transactions
            .push(block.transactions.last().unwrap().clone());
        let root: block::merkle::Root = block.transactions.iter().collect();
        assert_eq!(root, block.header.merkle_root);
        assert!(verify_block(&serialized(&block), hash(), HEIGHT)
            .unwrap_err()
            .to_string()
            .contains("duplicate transaction"));
    }

    #[test]
    fn rejects_invalid_anchor_height_encoding_and_trailing_data() {
        assert!(verify_block(RAW, block::Hash([0; 32]), HEIGHT)
            .unwrap_err()
            .to_string()
            .contains("hash mismatch"));
        assert!(verify_block(RAW, hash(), HEIGHT + 1)
            .unwrap_err()
            .to_string()
            .contains("height mismatch"));
        assert!(verify_block(&RAW[..RAW.len() - 1], hash(), HEIGHT).is_err());
        assert!(verify_block(&[], hash(), HEIGHT).is_err());
        assert!(verify_block(&vec![0; MAX_BLOCK_BYTES + 1], hash(), HEIGHT).is_err());
        let mut trailing = RAW.to_vec();
        trailing.push(0);
        assert!(verify_block(&trailing, hash(), HEIGHT)
            .unwrap_err()
            .to_string()
            .contains("trailing bytes"));
        let mut block = parsed();
        Arc::make_mut(&mut block.header).previous_block_hash = block::Hash([0; 32]);
        assert!(verify_block(&serialized(&block), hash(), HEIGHT)
            .unwrap_err()
            .to_string()
            .contains("hash mismatch"));
    }
}
