//! Block decoding with the backend's `zcash_primitives` implementation.
//!
//! `zakura-primitives` and upstream `zcash_primitives` share this API and both expose
//! it under the crate name `zcash_primitives`.

use std::io::Cursor;

use anyhow::{Context, Result};
use pir_types::ZcashNetwork;
use sha2::{Digest, Sha256};
use zcash_primitives::block::Block;
use zcash_protocol::consensus::Network;

use super::{BlockHash, DecodedBlock, TxId};

pub(crate) const MAX_BLOCK_BYTES: usize = zcash_protocol::constants::MAX_BLOCK_BYTES;

/// Consensus parameters for `network`, which select the transaction format by height.
fn parameters(network: ZcashNetwork) -> Network {
    match network {
        ZcashNetwork::Main => Network::MainNetwork,
        ZcashNetwork::Test => Network::TestNetwork,
    }
}

/// The decoder also rejects a block whose coinbase does not encode a height, and a
/// V5-or-later coinbase whose consensus branch does not match that height on `network`.
pub(crate) fn decode_block(raw: &[u8], network: ZcashNetwork) -> Result<DecodedBlock> {
    let mut cursor = Cursor::new(raw);
    let block = Block::read(&mut cursor, &parameters(network))?;
    let header = block.header();
    Ok(DecodedBlock {
        encoded_len: usize::try_from(cursor.position()).context("decoded length")?,
        hash: BlockHash::from_serialized_bytes(header.hash().0),
        previous_hash: BlockHash::from_serialized_bytes(header.prev_block.0),
        merkle_root: header.merkle_root,
        coinbase_height: Some(u64::from(u32::from(block.claimed_height()))),
        txids: block
            .vtx()
            .iter()
            .map(|tx| TxId::from_serialized_bytes(*tx.txid().as_ref()))
            .collect(),
        ironwood_nullifiers: block
            .vtx()
            .iter()
            .filter_map(|tx| tx.ironwood_bundle())
            .flat_map(|bundle| bundle.actions().iter())
            .map(|action| action.nullifier().to_bytes())
            .collect(),
    })
}

pub(crate) fn transaction_merkle_root(txids: &[TxId]) -> Option<[u8; 32]> {
    let leaves: Vec<[u8; 32]> = txids.iter().map(TxId::serialized_bytes).collect();
    std::iter::successors(Some(leaves), |level| {
        (level.len() > 1).then(|| parent_level(level))
    })
    .last()
    .and_then(|root_level| root_level.first().copied())
}

/// Hash adjacent pairs of `level`, pairing a trailing odd node with itself.
fn parent_level(level: &[[u8; 32]]) -> Vec<[u8; 32]> {
    level
        .chunks(2)
        .map(|pair| {
            // `chunks` never yields an empty slice.
            let left = &pair[0];
            let right = pair.get(1).unwrap_or(left);
            Sha256::digest(
                Sha256::new()
                    .chain_update(left)
                    .chain_update(right)
                    .finalize(),
            )
            .into()
        })
        .collect()
}
