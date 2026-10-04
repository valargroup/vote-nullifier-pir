//! Raw-block decoding over the protocol implementation selected at compile time.
//!
//! The `zakura` feature decodes with `zakura-primitives`; the `upstream` feature
//! decodes with `zcash_primitives`. Content authentication is written once against
//! [`DecodedBlock`].

use std::fmt;

use hex::FromHex;

#[cfg(all(feature = "zakura", feature = "upstream"))]
compile_error!("the `zakura` and `upstream` features are mutually exclusive");
#[cfg(not(any(feature = "zakura", feature = "upstream")))]
compile_error!("select a protocol backend with the `zakura` or `upstream` feature");

mod decode;

#[cfg(test)]
pub(crate) mod test_blocks;

/// Maximum serialized block size supported by the selected protocol implementation.
pub(crate) use decode::MAX_BLOCK_BYTES;

/// Decode one block from the start of `raw` for the given network.
///
/// Bytes after the block are not consumed; see [`DecodedBlock::encoded_len`].
/// Returns an error if the prefix of `raw` is not a well-formed block.
pub(crate) use decode::decode_block;

/// Compute the Bitcoin-style transaction Merkle root over `txids` in block order.
///
/// A tree level with an odd number of nodes pairs its last node with itself.
/// Returns `None` for an empty list.
pub(crate) use decode::transaction_merkle_root;

/// A block hash in serialized (internal) byte order.
///
/// Hex parsing and `Display` use RPC display order, the byte reversal of serialized order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct BlockHash([u8; 32]);

impl BlockHash {
    /// Construct a hash from its serialized byte order.
    pub(crate) const fn from_serialized_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
}

impl FromHex for BlockHash {
    type Error = hex::FromHexError;

    fn from_hex<T: AsRef<[u8]>>(hex: T) -> Result<Self, Self::Error> {
        let mut bytes = <[u8; 32]>::from_hex(hex)?;
        bytes.reverse();
        Ok(Self(bytes))
    }
}

impl fmt::Display for BlockHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut display = self.0;
        display.reverse();
        f.write_str(&hex::encode(display))
    }
}

/// A transaction ID in serialized (internal) byte order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct TxId([u8; 32]);

impl TxId {
    /// Construct a transaction ID from its serialized byte order.
    pub(crate) const fn from_serialized_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// The transaction ID in serialized byte order.
    pub(crate) const fn serialized_bytes(&self) -> [u8; 32] {
        self.0
    }
}

/// The parts of a decoded block that content authentication consumes.
#[derive(Debug)]
pub(crate) struct DecodedBlock {
    /// Number of bytes of the input that encode the block.
    pub(crate) encoded_len: usize,
    /// Hash of the block header.
    pub(crate) hash: BlockHash,
    /// Previous-block hash committed to by the header.
    pub(crate) previous_hash: BlockHash,
    /// Transaction Merkle root committed to by the header, in serialized byte order.
    pub(crate) merkle_root: [u8; 32],
    /// Height claimed by the coinbase input, if the first transaction carries one.
    pub(crate) coinbase_height: Option<u64>,
    /// Transaction IDs in block order.
    pub(crate) txids: Vec<TxId>,
    /// All Ironwood action nullifiers, in transaction/action order, as canonical bytes.
    pub(crate) ironwood_nullifiers: Vec<[u8; 32]>,
}
