//! `bitcoin` crate interoperability for `mm2_bitcoin::chain`.
//!
//! This module is compiled only when the `ext-bitcoin` feature is enabled.
//! It centralizes all conversions between `chain` core types and the upstream
//! `bitcoin` crate types to keep feature-gated code out of the core modules.

pub use bitcoin::blockdata::block::BlockHeader as BitcoinBlockHeader;
pub use bitcoin::blockdata::transaction::{
    OutPoint as BitcoinOutPoint, Transaction as BitcoinTransaction, TxIn as BitcoinTxIn, TxOut as BitcoinTxOut,
};
pub use bitcoin::hash_types::{
    BlockHash as BitcoinBlockHash, TxMerkleNode as BitcoinTxMerkleNode, Txid as BitcoinTxid,
};
pub use bitcoin::{PackedLockTime as BitcoinPackedLockTime, Sequence as BitcoinSequence, Witness as BitcoinWitness};

use block_header::{BlockHeader, BlockHeaderNonce};
use transaction::{OutPoint, Transaction, TransactionInput, TransactionOutput};

impl From<BlockHeader> for BitcoinBlockHeader {
    fn from(header: BlockHeader) -> Self {
        let prev_blockhash = BitcoinBlockHash::from_hash(header.previous_header_hash.to_sha256d());
        let merkle_root = BitcoinTxMerkleNode::from_hash(header.merkle_root_hash.to_sha256d());
        // Note: H256 nonce is not supported for bitcoin, we will just set nonce to 0 in this case since this will never happen.
        let nonce = match header.nonce {
            BlockHeaderNonce::U32(n) => n,
            _ => 0,
        };
        BitcoinBlockHeader {
            version: header.version as i32,
            prev_blockhash,
            merkle_root,
            time: header.time,
            bits: header.bits.into(),
            nonce,
        }
    }
}

impl From<OutPoint> for BitcoinOutPoint {
    fn from(outpoint: OutPoint) -> Self {
        BitcoinOutPoint {
            txid: BitcoinTxid::from_hash(outpoint.hash.to_sha256d()),
            vout: outpoint.index,
        }
    }
}

impl From<TransactionInput> for BitcoinTxIn {
    fn from(txin: TransactionInput) -> Self {
        BitcoinTxIn {
            previous_output: txin.previous_output.into(),
            script_sig: txin.script_sig.take().into(),
            sequence: BitcoinSequence(txin.sequence),
            witness: BitcoinWitness::from_vec(txin.script_witness.into_iter().map(|s| s.take()).collect()),
        }
    }
}

impl From<TransactionOutput> for BitcoinTxOut {
    fn from(txout: TransactionOutput) -> Self {
        BitcoinTxOut {
            value: txout.value,
            script_pubkey: txout.script_pubkey.take().into(),
        }
    }
}

impl From<Transaction> for BitcoinTransaction {
    fn from(tx: Transaction) -> Self {
        BitcoinTransaction {
            version: tx.version,
            lock_time: BitcoinPackedLockTime(tx.lock_time),
            input: tx.inputs.into_iter().map(|i| i.into()).collect(),
            output: tx.outputs.into_iter().map(|o| o.into()).collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_from_blockheader_to_ext_blockheader() {
        // https://live.blockcypher.com/btc/block/00000000000000000020cf2bdc6563fb25c424af588d5fb7223461e72715e4a9/
        let header: BlockHeader = "0200000066720b99e07d284bd4fe67ff8c49a5db1dd8514fcdab610000000000000000007829844f4c3a41a537b3131ca992643eaa9d093b2383e4cdc060ad7dc548118751eb505ac1910018de19b302".into();
        let ext_header = BitcoinBlockHeader::from(header.clone());
        assert_eq!(
            header.hash().reversed().to_string(),
            ext_header.block_hash().to_string()
        );
    }

    #[test]
    fn test_from_tx_to_ext_tx() {
        // https://live.blockcypher.com/btc-testnet/tx/2be90e03abb4d5328bf7e9467ca9c571aef575837b55f1253119b87e85ccb94f/
        let tx: Transaction = "010000000001016546e6d844ad0142c8049a839e8deae16c17f0a6587e36e75ff2181ed7020a800100000000ffffffff0247070800000000002200200bbfbd271853ec0a775e5455d4bb19d32818e9b5bda50655ac183fb15c9aa01625910300000000001600149a85cc05e9a722575feb770a217c73fd6145cf0102473044022002eac5d11f3800131985c14a3d1bc03dfe5e694f5731bde39b0d2b183eb7d3d702201d62e7ff2dd433260bf7a8223db400d539a2c4eccd27a5aa24d83f5ad9e9e1750121031ac6d25833a5961e2a8822b2e8b0ac1fd55d90cbbbb18a780552cbd66fc02bb35c099c61".into();
        let ext_tx = BitcoinTransaction::from(tx.clone());
        assert_eq!(tx.hash().reversed().to_string(), ext_tx.txid().to_string());
    }
}
