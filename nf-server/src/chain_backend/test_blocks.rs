//! Structural edits of serialized mainnet fixture blocks, for negative tests.
//!
//! The unit tests and the `verify_root` integration test share this file, so it refers
//! only to external crates. Both backends expose their primitives crate as
//! `zcash_primitives`. Edits keep the genuine header unless stated otherwise.

// Each including crate uses a different subset of these helpers.
#![allow(dead_code)]

use sha2::{Digest, Sha256};

/// Byte offset of the previous-block hash in a serialized header (after the version).
const PREVIOUS_HASH_OFFSET: usize = 4;
/// Length of a serialized block hash.
const HASH_LEN: usize = 32;
/// Halo 2 proof bytes contributed by each action (proof size is 2720 + 2272 per action).
const PROOF_BYTES_PER_ACTION: usize = 2272;

/// One serialized transaction of a fixture block, with its shielded action counts.
pub(crate) struct FixtureTransaction {
    pub(crate) bytes: Vec<u8>,
    pub(crate) orchard_actions: usize,
    pub(crate) ironwood_actions: usize,
}

/// A mainnet fixture block split into its serialized header and transactions.
pub(crate) struct FixtureBlock {
    pub(crate) header: Vec<u8>,
    pub(crate) transactions: Vec<FixtureTransaction>,
}

impl FixtureBlock {
    /// Split a well-formed mainnet block. Panics on malformed input.
    pub(crate) fn parse(raw: &[u8]) -> Self {
        backend::parse(raw)
    }

    /// Serialize the header followed by the CompactSize-prefixed transactions.
    pub(crate) fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = self.header.clone();
        write_compact_size(&mut bytes, self.transactions.len());
        for tx in &self.transactions {
            bytes.extend_from_slice(&tx.bytes);
        }
        bytes
    }

    /// The header hash in RPC display order.
    pub(crate) fn hash_display(&self) -> String {
        let mut hash: [u8; HASH_LEN] = Sha256::digest(Sha256::digest(&self.header)).into();
        hash.reverse();
        hex::encode(hash)
    }

    /// Overwrite the header's previous-block hash, which changes the block hash.
    pub(crate) fn set_previous_hash(&mut self, hash: [u8; HASH_LEN]) {
        self.header[PREVIOUS_HASH_OFFSET..PREVIOUS_HASH_OFFSET + HASH_LEN].copy_from_slice(&hash);
    }

    /// Remove the first transaction that has Ironwood actions.
    pub(crate) fn remove_first_transaction_with_ironwood_actions(&mut self) {
        let i = self
            .transactions
            .iter()
            .position(|tx| tx.ironwood_actions > 0)
            .expect("fixture has an Ironwood transaction");
        self.transactions.remove(i);
    }

    /// Append a copy of the last transaction.
    pub(crate) fn duplicate_last_transaction(&mut self) {
        let last = self.transactions.last().expect("fixture has transactions");
        let copy = FixtureTransaction {
            bytes: last.bytes.clone(),
            orchard_actions: last.orchard_actions,
            ironwood_actions: last.ironwood_actions,
        };
        self.transactions.push(copy);
    }

    /// In the first transaction with several Ironwood actions, drop the first action
    /// with its spend authorization signature and its share of the proof. The edited
    /// transaction stays structurally parseable; its proof is not valid.
    pub(crate) fn remove_first_action_of_multi_action_ironwood_bundle(&mut self) {
        let tx = self
            .transactions
            .iter_mut()
            .find(|tx| tx.ironwood_actions > 1)
            .expect("fixture has a multi-action Ironwood bundle");
        tx.bytes = backend::without_first_ironwood_action(&tx.bytes);
        tx.ironwood_actions -= 1;
    }
}

/// Replace the single occurrence of `needle` in `raw`. Panics unless it occurs exactly once.
pub(crate) fn replace_unique(raw: &[u8], needle: &[u8], replacement: &[u8]) -> Vec<u8> {
    assert_eq!(needle.len(), replacement.len());
    let positions: Vec<_> = raw
        .windows(needle.len())
        .enumerate()
        .filter(|(_, window)| *window == needle)
        .map(|(i, _)| i)
        .collect();
    assert_eq!(positions.len(), 1, "needle must occur exactly once");
    let mut edited = raw.to_vec();
    edited[positions[0]..positions[0] + needle.len()].copy_from_slice(replacement);
    edited
}

/// Append Bitcoin's CompactSize encoding of `n`.
fn write_compact_size(out: &mut Vec<u8>, n: usize) {
    match (u8::try_from(n), u16::try_from(n), u32::try_from(n)) {
        (Ok(small), _, _) if small < 0xfd => out.push(small),
        (_, Ok(medium), _) => {
            out.push(0xfd);
            out.extend_from_slice(&medium.to_le_bytes());
        }
        (_, _, Ok(large)) => {
            out.push(0xfe);
            out.extend_from_slice(&large.to_le_bytes());
        }
        _ => {
            out.push(0xff);
            out.extend_from_slice(&(n as u64).to_le_bytes());
        }
    }
}

mod backend {
    use zcash_primitives::{
        block::Block,
        transaction::{
            components::orchard::{write_action_without_auth, write_v6_bundle},
            Transaction,
        },
    };
    use zcash_protocol::consensus::{BranchId, Network};

    use super::{write_compact_size, FixtureBlock, FixtureTransaction, PROOF_BYTES_PER_ACTION};

    pub(super) fn parse(raw: &[u8]) -> FixtureBlock {
        let block = Block::read(raw, &Network::MainNetwork).expect("fixture block decodes");
        let mut header = Vec::new();
        block.header().write(&mut header).unwrap();
        FixtureBlock {
            header,
            transactions: block
                .vtx()
                .iter()
                .map(|tx| FixtureTransaction {
                    bytes: serialize(tx),
                    orchard_actions: tx.orchard_bundle().map_or(0, |b| b.actions().len()),
                    ironwood_actions: tx.ironwood_bundle().map_or(0, |b| b.actions().len()),
                })
                .collect(),
        }
    }

    fn serialize(tx: &Transaction) -> Vec<u8> {
        let mut bytes = Vec::new();
        tx.write(&mut bytes).unwrap();
        bytes
    }

    /// A V6 transaction encodes its Ironwood bundle last, so re-encode that suffix
    /// without the first action, its signature, and its share of the proof.
    pub(super) fn without_first_ironwood_action(tx_bytes: &[u8]) -> Vec<u8> {
        // V5 and later transactions encode their own branch ID; this argument is unused.
        let tx = Transaction::read(tx_bytes, BranchId::Nu6_3).expect("fixture transaction");
        assert_eq!(serialize(&tx), tx_bytes, "fixture transaction round-trips");
        let bundle = tx
            .ironwood_bundle()
            .expect("transaction has an Ironwood bundle");
        let mut suffix = Vec::new();
        write_v6_bundle(Some(bundle), &mut suffix).unwrap();
        let prefix = tx_bytes
            .strip_suffix(suffix.as_slice())
            .expect("the Ironwood bundle is the transaction's last field");

        let kept = || bundle.actions().iter().skip(1);
        let mut edited = prefix.to_vec();
        write_compact_size(&mut edited, kept().count());
        for action in kept() {
            write_action_without_auth(&mut edited, action).unwrap();
        }
        edited.push(bundle.flag_byte());
        edited.extend_from_slice(&bundle.value_balance().to_i64_le_bytes());
        edited.extend_from_slice(&bundle.anchor().to_bytes());
        let proof = bundle.authorization().proof().as_ref();
        let proof = &proof[..proof.len() - PROOF_BYTES_PER_ACTION];
        write_compact_size(&mut edited, proof.len());
        edited.extend_from_slice(proof);
        for action in kept() {
            edited.extend_from_slice(&<[u8; 64]>::from(action.authorization()));
        }
        edited.extend_from_slice(&<[u8; 64]>::from(
            bundle.authorization().binding_signature(),
        ));

        let reparsed = Transaction::read(edited.as_slice(), BranchId::Nu6_3)
            .expect("edited transaction stays parseable");
        assert_eq!(
            reparsed.ironwood_bundle().map(|b| b.actions().len()),
            Some(kept().count())
        );
        edited
    }
}
