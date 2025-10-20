use bitcrypto::dhash256;
use chain::{BlockHeader, BlockHeaderBits, BlockHeaderNonce, Transaction as UtxoTx};
use mm2_number::{BigDecimal, BigInt};
use rpc::v1::types::{Bytes as BytesJson, H256 as H256Json};
use serialization::serialize;

#[derive(Debug, Deserialize)]
pub struct ElectrumTxHistoryItem {
    pub height: i64,
    pub tx_hash: H256Json,
    pub fee: Option<i64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ElectrumUnspent {
    pub height: Option<u64>,
    pub tx_hash: H256Json,
    pub tx_pos: u32,
    pub value: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum ElectrumNonce {
    Number(u64),
    Hash(H256Json),
}

#[allow(clippy::from_over_into)]
impl Into<BlockHeaderNonce> for ElectrumNonce {
    fn into(self) -> BlockHeaderNonce {
        match self {
            ElectrumNonce::Number(n) => BlockHeaderNonce::U32(n as u32),
            ElectrumNonce::Hash(h) => BlockHeaderNonce::H256(h.into()),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ElectrumBlockHeadersRes {
    pub count: u64,
    pub hex: BytesJson,
    #[allow(dead_code)]
    max: u64,
}

/// The block header compatible with Electrum 1.2
#[derive(Clone, Debug, Deserialize)]
pub struct ElectrumBlockHeaderV12 {
    pub bits: u64,
    pub block_height: u64,
    pub merkle_root: H256Json,
    pub nonce: ElectrumNonce,
    pub prev_block_hash: H256Json,
    pub timestamp: u64,
    pub version: u64,
}

impl ElectrumBlockHeaderV12 {
    fn as_block_header(&self) -> BlockHeader {
        BlockHeader {
            version: self.version as u32,
            previous_header_hash: self.prev_block_hash.into(),
            merkle_root_hash: self.merkle_root.into(),
            claim_trie_root: None,
            hash_final_sapling_root: None,
            time: self.timestamp as u32,
            bits: BlockHeaderBits::U32(self.bits as u32),
            nonce: self.nonce.clone().into(),
            solution: None,
            aux_pow: None,
            prog_pow: None,
            mtp_pow: None,
            is_verus: false,
            hash_state_root: None,
            hash_utxo_root: None,
            prevout_stake: None,
            vch_block_sig_dlgt: None,
            n_height: None,
            n_nonce_u64: None,
            mix_hash: None,
        }
    }

    #[inline]
    pub fn as_hex(&self) -> String {
        let block_header = self.as_block_header();
        let serialized = serialize(&block_header);
        hex::encode(serialized)
    }

    #[inline]
    pub fn hash(&self) -> H256Json {
        let block_header = self.as_block_header();
        BlockHeader::hash(&block_header).into()
    }
}

/// The block header compatible with Electrum 1.4
#[derive(Clone, Debug, Deserialize)]
pub struct ElectrumBlockHeaderV14 {
    pub height: u64,
    pub hex: BytesJson,
}

impl ElectrumBlockHeaderV14 {
    pub fn hash(&self) -> H256Json {
        dhash256(&self.hex.clone().into_vec()).into()
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum ElectrumBlockHeader {
    V12(ElectrumBlockHeaderV12),
    V14(ElectrumBlockHeaderV14),
}

impl ElectrumBlockHeader {
    pub fn block_height(&self) -> u64 {
        match self {
            ElectrumBlockHeader::V12(h) => h.block_height,
            ElectrumBlockHeader::V14(h) => h.height,
        }
    }

    pub fn block_hash(&self) -> H256Json {
        match self {
            ElectrumBlockHeader::V12(h) => h.hash(),
            ElectrumBlockHeader::V14(h) => h.hash(),
        }
    }
}

/// The merkle branch of a confirmed transaction
#[derive(Clone, Debug, Deserialize)]
pub struct TxMerkleBranch {
    pub merkle: Vec<H256Json>,
    pub block_height: u64,
    pub pos: usize,
}

#[derive(Clone)]
pub struct ConfirmedTransactionInfo {
    pub tx: UtxoTx,
    pub header: BlockHeader,
    pub index: u64,
    pub height: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ElectrumBalance {
    pub(crate) confirmed: i128,
    pub(crate) unconfirmed: i128,
}

impl ElectrumBalance {
    #[inline]
    pub fn to_big_decimal(&self, decimals: u8) -> BigDecimal {
        let balance_sat = BigInt::from(self.confirmed) + BigInt::from(self.unconfirmed);
        BigDecimal::from(balance_sat) / BigDecimal::from(10u64.pow(decimals as u32))
    }
}

#[derive(Debug, Deserialize, Serialize)]
/// Deserializable Electrum protocol version representation for RPC
/// https://electrumx-spesmilo.readthedocs.io/en/latest/protocol-methods.html#server.version
pub struct ElectrumProtocolVersion {
    pub server_software_version: String,
    pub protocol_version: String,
}
