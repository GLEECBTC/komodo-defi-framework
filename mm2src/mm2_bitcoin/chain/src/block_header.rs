use compact::Compact;
use crypto::dhash256;
#[cfg(not(target_arch = "wasm32"))]
use ext_bitcoin::blockdata::block::BlockHeader as ExtBlockHeader;
#[cfg(not(target_arch = "wasm32"))]
use ext_bitcoin::hash_types::{BlockHash as ExtBlockHash, TxMerkleNode as ExtTxMerkleNode};
use hash::H256;
use hex::FromHex;
use primitives::bytes::Bytes;
use primitives::U256;
use ser::{deserialize, serialize, CoinVariant, Deserializable, Reader, Serializable, Stream};
use std::io;
use transaction::{deserialize_tx, TxType};
use {OutPoint, Transaction};

#[derive(Clone, Debug, PartialEq)]
pub enum BlockHeaderNonce {
    U32(u32),
    H256(H256),
}

impl Serializable for BlockHeaderNonce {
    fn serialize(&self, s: &mut Stream) {
        match self {
            BlockHeaderNonce::U32(n) => s.append(n),
            BlockHeaderNonce::H256(h) => s.append(h),
        };
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum BlockHeaderBits {
    Compact(Compact),
    U32(u32),
}

impl Serializable for BlockHeaderBits {
    fn serialize(&self, s: &mut Stream) {
        match self {
            BlockHeaderBits::Compact(c) => s.append(c),
            BlockHeaderBits::U32(n) => s.append(n),
        };
    }
}

impl From<BlockHeaderBits> for u32 {
    fn from(bits: BlockHeaderBits) -> Self {
        match bits {
            BlockHeaderBits::Compact(c) => c.into(),
            BlockHeaderBits::U32(n) => n,
        }
    }
}

impl From<BlockHeaderBits> for Compact {
    fn from(bits: BlockHeaderBits) -> Self {
        match bits {
            BlockHeaderBits::Compact(c) => c,
            BlockHeaderBits::U32(n) => Compact::new(n),
        }
    }
}

// The version number used by AuxPow-enabled coins is a bitmask.
const AUXPOW_VERSION_FLAG: u32 = 1 << 8;
const MTP_POW_VERSION: u32 = 0x20001000u32;
const PROG_POW_SWITCH_TIME: u32 = 1635228000;
const BIP9_NO_SOFT_FORK_BLOCK_HEADER_VERSION: u32 = 536870912;
// RVN
const KAWPOW_VERSION: u32 = 805306368;

#[derive(Clone, Debug, PartialEq, Deserializable, Serializable)]
pub struct MerkleBranch {
    branch_hashes: Vec<H256>,
    branch_side_mask: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AuxPow {
    coinbase_tx: Transaction,
    parent_block_hash: H256,
    coinbase_branch: MerkleBranch,
    blockchain_branch: MerkleBranch,
    parent_block_header: Box<BlockHeader>,
}

#[derive(Clone, Debug, Deserializable, PartialEq, Serializable)]
pub struct ProgPow {
    n_height: u32,
    n_nonce_64: u64,
    mix_hash: H256,
}

#[derive(Clone, Debug, Deserializable, PartialEq, Serializable)]
pub struct MtpPow {
    n_version_mtp: i32,
    mtp_hash_value: H256,
    reserved_0: H256,
    reserved_1: H256,
}

impl Serializable for AuxPow {
    fn serialize(&self, s: &mut Stream) {
        s.append(&self.coinbase_tx);
        s.append(&self.parent_block_hash);
        s.append(&self.coinbase_branch);
        s.append(&self.blockchain_branch);
        s.append(self.parent_block_header.as_ref());
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlockHeader {
    pub version: u32,
    pub previous_header_hash: H256,
    pub merkle_root_hash: H256,
    /// https://github.com/lbryio/lbrycrd/blob/71fc94b1dea1c7818f84a09832ec3db736481e0f/src/primitives/block.h#L40
    pub claim_trie_root: Option<H256>,
    pub hash_final_sapling_root: Option<H256>,
    pub time: u32,
    pub bits: BlockHeaderBits,
    pub nonce: BlockHeaderNonce,
    pub solution: Option<Vec<u8>>,
    /// https://en.bitcoin.it/wiki/Merged_mining_specification#Merged_mining_coinbase
    pub aux_pow: Option<AuxPow>,
    /// https://github.com/firoorg/firo/blob/904984eecdc15df5de19bd34c70fd989847a2f08/src/primitives/block.h#L197
    pub prog_pow: Option<ProgPow>,
    /// https://github.com/firoorg/firo/blob/75f72d061eb793c39148c6d3f3eb5595159fdca0/src/primitives/block.h#L159
    pub mtp_pow: Option<MtpPow>,
    pub is_verus: bool,
    /// https://github.com/qtumproject/qtum/blob/2792457a6a7b7922bc33ba934c3ed47a3ff66bf9/src/primitives/block.h#L30
    pub hash_state_root: Option<H256>,
    /// https://github.com/qtumproject/qtum/blob/2792457a6a7b7922bc33ba934c3ed47a3ff66bf9/src/primitives/block.h#L31
    pub hash_utxo_root: Option<H256>,
    /// https://github.com/qtumproject/qtum/blob/2792457a6a7b7922bc33ba934c3ed47a3ff66bf9/src/primitives/block.h#L33
    pub prevout_stake: Option<OutPoint>,
    /// https://github.com/qtumproject/qtum/blob/2792457a6a7b7922bc33ba934c3ed47a3ff66bf9/src/primitives/block.h#L34
    pub vch_block_sig_dlgt: Option<Vec<u8>>,
    /// https://github.com/RavenProject/Ravencoin/blob/61c790447a5afe150d9892705ac421d595a2df60/src/primitives/block.h#L49
    pub n_height: Option<u32>,
    /// https://github.com/RavenProject/Ravencoin/blob/61c790447a5afe150d9892705ac421d595a2df60/src/primitives/block.h#L50
    pub n_nonce_u64: Option<u64>,
    /// https://github.com/RavenProject/Ravencoin/blob/61c790447a5afe150d9892705ac421d595a2df60/src/primitives/block.h#L51
    pub mix_hash: Option<H256>,
}

impl Serializable for BlockHeader {
    fn serialize(&self, s: &mut Stream) {
        if self.is_verus {
            s.append(&(self.version ^ 0x00010000));
        } else {
            s.append(&self.version);
        }
        s.append(&self.previous_header_hash);
        s.append(&self.merkle_root_hash);
        if let Some(claim) = &self.claim_trie_root {
            s.append(claim);
        }
        // For Zcash-style headers, the sapling root is serialized before the time.
        if self.solution.is_some() {
            if let Some(h) = &self.hash_final_sapling_root {
                s.append(h);
            }
        }
        s.append(&self.time);
        s.append(&self.bits);
        // If a BTC header uses KAWPOW_VERSION, the nonce can't be zero
        if !self.is_prog_pow() && (self.version != KAWPOW_VERSION || self.nonce != BlockHeaderNonce::U32(0)) {
            s.append(&self.nonce);
        }
        // For PIVX-style headers, the sapling root is serialized after the nonce.
        if self.solution.is_none() {
            if let Some(h) = &self.hash_final_sapling_root {
                s.append(h);
            }
        }
        if let Some(sol) = &self.solution {
            s.append_list(sol);
        }
        if let Some(pow) = &self.aux_pow {
            s.append(pow);
        }

        if let Some(pow) = &self.prog_pow {
            s.append(pow);
        }

        if let Some(pow) = &self.mtp_pow {
            s.append(pow);
        }

        if let Some(root) = &self.hash_state_root {
            s.append(root);
        }

        if let Some(root) = &self.hash_utxo_root {
            s.append(root);
        }

        if let Some(prevout) = &self.prevout_stake {
            s.append(prevout);
        }

        if let Some(vec) = &self.vch_block_sig_dlgt {
            s.append_list(vec);
        }

        if let Some(n_height) = &self.n_height {
            s.append(n_height);
        }

        if let Some(n_nonce_u64) = &self.n_nonce_u64 {
            s.append(n_nonce_u64);
        }

        if let Some(mix_hash) = &self.mix_hash {
            s.append(mix_hash);
        }
    }
}

impl Deserializable for BlockHeader {
    fn deserialize<T: io::Read>(reader: &mut Reader<T>) -> Result<Self, ser::Error>
    where
        Self: Sized,
    {
        let mut version = reader.read()?;
        let is_verus = (version ^ 0x00010000) == 4;
        if is_verus {
            version ^= 0x00010000;
        }
        let previous_header_hash = reader.read()?;
        let merkle_root_hash = reader.read()?;

        // This is needed to deserialize coin like LBC correctly.
        let claim_trie_root = if version == BIP9_NO_SOFT_FORK_BLOCK_HEADER_VERSION && reader.coin_variant().is_lbc() {
            Some(reader.read()?)
        } else {
            None
        };

        let is_zcash_style = (version == 4 && !reader.coin_variant().is_btc() && !reader.coin_variant().is_ppc())
            || reader.coin_variant().is_kmd_assetchain();
        let mut hash_final_sapling_root = if is_zcash_style { Some(reader.read()?) } else { None };
        let time = reader.read()?;
        let bits = if is_zcash_style {
            BlockHeaderBits::U32(reader.read()?)
        } else {
            BlockHeaderBits::Compact(reader.read()?)
        };
        let nonce = if is_zcash_style {
            BlockHeaderNonce::H256(reader.read()?)
        } else if (version == KAWPOW_VERSION && reader.coin_variant().is_rvn())
            || (version == MTP_POW_VERSION && time >= PROG_POW_SWITCH_TIME)
        {
            BlockHeaderNonce::U32(0)
        } else {
            BlockHeaderNonce::U32(reader.read()?)
        };
        // A PIVX header is a standard header with a `hash_final_sapling_root` added after the nonce.
        hash_final_sapling_root = if reader.coin_variant().is_pivx() {
            Some(reader.read()?)
        } else {
            hash_final_sapling_root
        };
        let solution = if is_zcash_style {
            Some(reader.read_list()?)
        } else {
            None
        };

        // https://en.bitcoin.it/wiki/Merged_mining_specification#Merged_mining_coinbase
        // AuxPoW is only for AuxPoW coins (Dogecoin, Namecoin, Syscoin, etc.).
        // BTC-like variants (BTC, BCH, NAV, ...) must NOT interpret bit 8 as AuxPoW.
        let aux_pow = if (version & AUXPOW_VERSION_FLAG) != 0 && !reader.coin_variant().is_btc() {
            let coinbase_tx = deserialize_tx(reader, TxType::StandardWithWitness)?;
            let parent_block_hash = reader.read()?;
            let coinbase_branch = reader.read()?;
            let blockchain_branch = reader.read()?;
            let parent_block_header = Box::new(reader.read()?);
            Some(AuxPow {
                coinbase_tx,
                parent_block_hash,
                coinbase_branch,
                blockchain_branch,
                parent_block_header,
            })
        } else {
            None
        };

        let prog_pow = if version == MTP_POW_VERSION && time >= PROG_POW_SWITCH_TIME {
            Some(reader.read()?)
        } else {
            None
        };

        let mtp_pow = if version == MTP_POW_VERSION && time < PROG_POW_SWITCH_TIME && prog_pow.is_none() {
            Some(reader.read()?)
        } else {
            None
        };

        let (hash_state_root, hash_utxo_root, prevout_stake, vch_block_sig_dlgt) =
            if version == BIP9_NO_SOFT_FORK_BLOCK_HEADER_VERSION && reader.coin_variant().is_qtum() {
                (
                    Some(reader.read()?),
                    Some(reader.read()?),
                    Some(reader.read()?),
                    Some(reader.read_list()?),
                )
            } else {
                (None, None, None, None)
            };

        // https://github.com/RavenProject/Ravencoin/blob/61c790447a5afe150d9892705ac421d595a2df60/src/primitives/block.h#L67
        let (n_height, n_nonce_u64, mix_hash) = if version == KAWPOW_VERSION && reader.coin_variant().is_rvn() {
            (Some(reader.read()?), Some(reader.read()?), Some(reader.read()?))
        } else {
            (None, None, None)
        };

        Ok(BlockHeader {
            version,
            previous_header_hash,
            merkle_root_hash,
            claim_trie_root,
            hash_final_sapling_root,
            time,
            bits,
            nonce,
            solution,
            aux_pow,
            prog_pow,
            mtp_pow,
            is_verus,
            hash_state_root,
            hash_utxo_root,
            prevout_stake,
            vch_block_sig_dlgt,
            n_height,
            n_nonce_u64,
            mix_hash,
        })
    }
}

impl BlockHeader {
    pub fn try_from_string_with_coin_variant(header: String, coin_variant: CoinVariant) -> Result<Self, ser::Error> {
        let buffer = &header
            .from_hex::<Vec<u8>>()
            .map_err(|e| ser::Error::Custom(e.to_string()))? as &[u8];
        let mut reader = Reader::new_with_coin_variant(buffer, coin_variant);
        reader.read::<BlockHeader>()
    }

    pub fn hash(&self) -> H256 {
        dhash256(&serialize(self))
    }

    pub fn is_prog_pow(&self) -> bool {
        self.version == MTP_POW_VERSION && self.time >= PROG_POW_SWITCH_TIME
    }
    pub fn raw(&self) -> Bytes {
        serialize(self)
    }
    pub fn target(&self) -> Result<U256, U256> {
        match self.bits {
            BlockHeaderBits::Compact(compact) => compact.to_u256(),
            BlockHeaderBits::U32(nb) => Ok(U256::from(nb)),
        }
    }
}

impl From<&'static str> for BlockHeader {
    fn from(s: &'static str) -> Self {
        deserialize(&s.from_hex::<Vec<u8>>().unwrap() as &[u8]).unwrap()
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<BlockHeader> for ExtBlockHeader {
    fn from(header: BlockHeader) -> Self {
        let prev_blockhash = ExtBlockHash::from_hash(header.previous_header_hash.to_sha256d());
        let merkle_root = ExtTxMerkleNode::from_hash(header.merkle_root_hash.to_sha256d());
        // note: H256 nonce is not supported for bitcoin, we will just set nonce to 0 in this case since this will never happen
        let nonce = match header.nonce {
            BlockHeaderNonce::U32(n) => n,
            _ => 0,
        };
        ExtBlockHeader {
            version: header.version as i32,
            prev_blockhash,
            merkle_root,
            time: header.time,
            bits: header.bits.into(),
            nonce,
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(not(target_arch = "wasm32"))]
    use super::ExtBlockHeader;
    use block_header::{
        BlockHeader, BlockHeaderBits, BlockHeaderNonce, BIP9_NO_SOFT_FORK_BLOCK_HEADER_VERSION, KAWPOW_VERSION,
        MTP_POW_VERSION, PROG_POW_SWITCH_TIME,
    };
    use hex::FromHex;
    use primitives::bytes::Bytes;
    use ser::{deserialize, serialize, serialize_list, CoinVariant, Error as ReaderError, Reader, Stream};

    const AUX_POW_VERSION_DOGE: u32 = 6422788;

    #[test]
    fn test_block_header_stream() {
        let block_header = BlockHeader {
            version: 1,
            previous_header_hash: [2; 32].into(),
            merkle_root_hash: [3; 32].into(),
            claim_trie_root: None,
            hash_final_sapling_root: None,
            time: 4,
            bits: BlockHeaderBits::Compact(5.into()),
            nonce: BlockHeaderNonce::U32(6),
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
        };

        let mut stream = Stream::default();
        stream.append(&block_header);

        let expected = vec![
            1, 0, 0, 0, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
            3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 4, 0, 0, 0,
            5, 0, 0, 0, 6, 0, 0, 0,
        ]
        .into();

        assert_eq!(stream.out(), expected);
    }

    #[test]
    fn test_block_header_reader() {
        let buffer = vec![
            1, 0, 0, 0, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
            3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 4, 0, 0, 0,
            5, 0, 0, 0, 6, 0, 0, 0,
        ];

        let mut reader = Reader::new(&buffer);

        let expected = BlockHeader {
            version: 1,
            previous_header_hash: [2; 32].into(),
            merkle_root_hash: [3; 32].into(),
            claim_trie_root: None,
            hash_final_sapling_root: Default::default(),
            time: 4,
            bits: BlockHeaderBits::Compact(5.into()),
            nonce: BlockHeaderNonce::U32(6),
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
        };

        assert_eq!(expected, reader.read().unwrap());
        assert_eq!(ReaderError::UnexpectedEnd, reader.read::<BlockHeader>().unwrap_err());
    }

    #[test]
    fn test_sapling_block_header_serde() {
        // block header of https://kmdexplorer.io/block/01ad31e22fea912a974c3e1eea11dc26348676528a586f77199ac3cfe29e271f
        let header_hex = "040000008e4e7283b71dd1572d220935db0a1654d1042e92378579f8abab67b143f93a02fa026610d2634b72ff729b9ea7850c0d2c25eeaf7a82878ca42a8e9912028863a2d8a734eb73a4dc734072dbfd12406f1e7121bfe0e3d6c10922495c44e5cc1c91185d5ee519011d0400b9caaf41d4b63a6ab55bb4e6925d46fc3adea7be37b713d3a615e7cf0000fd40050001a80fa65b9a46fdb1506a7a4d26f43e7995d69902489b9f6c4599c88f9c169605cc135258953da0d6299ada4ff81a76ad63c943261078d5dd1918f91cea68b65b7fc362e9df49ba57c2ea5c6dba91591c85eb0d59a1905ac66e2295b7a291a1695301489a3cc7310fd45f2b94e3b8d94f3051e9bbaada1e0641fcec6e0d6230e76753aa9574a3f3e28eaa085959beffd3231dbe1aeea3955328f3a973650a38e31632a4ffc7ec007a3345124c0b99114e2444b3ef0ada75adbd077b247bbf3229adcffbe95bc62daac88f96317d5768540b5db636f8c39a8529a736465ed830ab2c1bbddf523587abe14397a6f1835d248092c4b5b691a955572607093177a5911e317739187b41f4aa662aa6bca0401f1a0a77915ebb6947db686cff549c5f4e7b9dd93123b00a1ae8d411cfb13fa7674de21cbee8e9fc74e12aa6753b261eab3d9256c7c32cc9b16219dad73c61014e7d88d74d5e218f12e11bc47557347ff49a9ab4490647418d2a5c2da1df24d16dfb611173608fe4b10a357b0fa7a1918b9f2d7836c84bf05f384e1e678b2fdd47af0d8e66e739fe45209ede151a180aba1188058a0db093e30bc9851980cf6fbfa5adb612d1146905da662c3347d7e7e569a1041641049d951ab867bc0c6a3863c7667d43f596a849434958cee2b63dc8fa11bd0f38aa96df86ed66461993f64736345313053508c4e939506c08a766f5b6ed0950759f3901bbc4db3dc97e05bf20b9dda4ff242083db304a4e487ac2101b823998371542354e5d534b5b6ae6420cc19b11512108b61208f4d9a5a97263d2c060da893544dea6251bcadc682d2238af35f2b1c2f65a73b89a4e194f9e1eef6f0e5948ef8d0d2862f48fd3356126b00c6a2d3770ecd0d1a78fa34974b454f270b23d461e357c9356c19496522b59ff9d5b4608c542ff89e558798324021704b2cfe9f6c1a70906c43c7a690f16615f198d29fa647d84ce8461fa570b33e3eada2ed7d77e1f280a0d2e9f03c2e1db535d922b1759a191b417595f3c15d8e8b7f810527ff942e18443a3860e67ccba356809ecedc31c5d8db59c7e039dae4b53d126679e8ffa20cc26e8b9d229c8f6ee434ad053f5f4f5a94e249a13afb995aad82b4d90890187e516e114b168fc7c7e291b9738ea578a7bab0ba31030b14ba90b772b577806ea2d17856b0cb9e74254ba582a9f2638ea7ed2ca23be898c6108ff8f466b443537ed9ec56b8771bfbf0f2f6e1092a28a7fd182f111e1dbdd155ea82c6cb72d5f9e6518cc667b8226b5f5c6646125fc851e97cf125f48949f988ed37c4283072fc03dd1da3e35161e17f44c0e22c76f708bb66405737ef24176e291b4fc2eadab876115dc62d48e053a85f0ad132ef07ad5175b036fe39e1ad14fcdcdc6ac5b3daabe05161a72a50545dd812e0f9af133d061b726f491e904d89ee57811ef58d3bda151f577aed381963a30d91fb98dc49413300d132a7021a5e834e266b4ac982d76e00f43f5336b8e8028a0cacfa11813b01e50f71236a73a4c0d0757c1832b0680ada56c80edf070f438ab2bc587542f926ff8d3644b8b8a56c78576f127dec7aed9cb3e1bc2442f978a9df1dc3056a63e653132d0f419213d3cb86e7b61720de1aa3af4b3757a58156970da27560c6629257158452b9d5e4283dc6fe7df42d2fda3352d5b62ce5a984d912777c3b01837df8968a4d494db1b663e0e68197dbf196f21ea11a77095263dec548e2010460840231329d83978885ee2423e8b327785970e27c6c6d436157fb5b56119b19239edbb730ebae013d82c35df4a6e70818a74d1ef7a2e87c090ff90e32939f58ed24e85b492b5750fd2cd14b9b8517136b76b1cc6ccc6f6f027f65f1967a0eb4f32cd6e5d5315";
        let header_bytes: Vec<u8> = header_hex.from_hex().unwrap();
        let header: BlockHeader = deserialize(header_bytes.as_slice()).unwrap();
        let expected_header = BlockHeader {
            version: 4,
            previous_header_hash: "8e4e7283b71dd1572d220935db0a1654d1042e92378579f8abab67b143f93a02".into(),
            merkle_root_hash: "fa026610d2634b72ff729b9ea7850c0d2c25eeaf7a82878ca42a8e9912028863".into(),
            claim_trie_root: None,
            hash_final_sapling_root: Some("a2d8a734eb73a4dc734072dbfd12406f1e7121bfe0e3d6c10922495c44e5cc1c".into()),
            time: 1583159441,
            bits: BlockHeaderBits::U32(486611429),
            nonce: BlockHeaderNonce::H256("0400b9caaf41d4b63a6ab55bb4e6925d46fc3adea7be37b713d3a615e7cf0000".into()), 
            solution: Some("0001a80fa65b9a46fdb1506a7a4d26f43e7995d69902489b9f6c4599c88f9c169605cc135258953da0d6299ada4ff81a76ad63c943261078d5dd1918f91cea68b65b7fc362e9df49ba57c2ea5c6dba91591c85eb0d59a1905ac66e2295b7a291a1695301489a3cc7310fd45f2b94e3b8d94f3051e9bbaada1e0641fcec6e0d6230e76753aa9574a3f3e28eaa085959beffd3231dbe1aeea3955328f3a973650a38e31632a4ffc7ec007a3345124c0b99114e2444b3ef0ada75adbd077b247bbf3229adcffbe95bc62daac88f96317d5768540b5db636f8c39a8529a736465ed830ab2c1bbddf523587abe14397a6f1835d248092c4b5b691a955572607093177a5911e317739187b41f4aa662aa6bca0401f1a0a77915ebb6947db686cff549c5f4e7b9dd93123b00a1ae8d411cfb13fa7674de21cbee8e9fc74e12aa6753b261eab3d9256c7c32cc9b16219dad73c61014e7d88d74d5e218f12e11bc47557347ff49a9ab4490647418d2a5c2da1df24d16dfb611173608fe4b10a357b0fa7a1918b9f2d7836c84bf05f384e1e678b2fdd47af0d8e66e739fe45209ede151a180aba1188058a0db093e30bc9851980cf6fbfa5adb612d1146905da662c3347d7e7e569a1041641049d951ab867bc0c6a3863c7667d43f596a849434958cee2b63dc8fa11bd0f38aa96df86ed66461993f64736345313053508c4e939506c08a766f5b6ed0950759f3901bbc4db3dc97e05bf20b9dda4ff242083db304a4e487ac2101b823998371542354e5d534b5b6ae6420cc19b11512108b61208f4d9a5a97263d2c060da893544dea6251bcadc682d2238af35f2b1c2f65a73b89a4e194f9e1eef6f0e5948ef8d0d2862f48fd3356126b00c6a2d3770ecd0d1a78fa34974b454f270b23d461e357c9356c19496522b59ff9d5b4608c542ff89e558798324021704b2cfe9f6c1a70906c43c7a690f16615f198d29fa647d84ce8461fa570b33e3eada2ed7d77e1f280a0d2e9f03c2e1db535d922b1759a191b417595f3c15d8e8b7f810527ff942e18443a3860e67ccba356809ecedc31c5d8db59c7e039dae4b53d126679e8ffa20cc26e8b9d229c8f6ee434ad053f5f4f5a94e249a13afb995aad82b4d90890187e516e114b168fc7c7e291b9738ea578a7bab0ba31030b14ba90b772b577806ea2d17856b0cb9e74254ba582a9f2638ea7ed2ca23be898c6108ff8f466b443537ed9ec56b8771bfbf0f2f6e1092a28a7fd182f111e1dbdd155ea82c6cb72d5f9e6518cc667b8226b5f5c6646125fc851e97cf125f48949f988ed37c4283072fc03dd1da3e35161e17f44c0e22c76f708bb66405737ef24176e291b4fc2eadab876115dc62d48e053a85f0ad132ef07ad5175b036fe39e1ad14fcdcdc6ac5b3daabe05161a72a50545dd812e0f9af133d061b726f491e904d89ee57811ef58d3bda151f577aed381963a30d91fb98dc49413300d132a7021a5e834e266b4ac982d76e00f43f5336b8e8028a0cacfa11813b01e50f71236a73a4c0d0757c1832b0680ada56c80edf070f438ab2bc587542f926ff8d3644b8b8a56c78576f127dec7aed9cb3e1bc2442f978a9df1dc3056a63e653132d0f419213d3cb86e7b61720de1aa3af4b3757a58156970da27560c6629257158452b9d5e4283dc6fe7df42d2fda3352d5b62ce5a984d912777c3b01837df8968a4d494db1b663e0e68197dbf196f21ea11a77095263dec548e2010460840231329d83978885ee2423e8b327785970e27c6c6d436157fb5b56119b19239edbb730ebae013d82c35df4a6e70818a74d1ef7a2e87c090ff90e32939f58ed24e85b492b5750fd2cd14b9b8517136b76b1cc6ccc6f6f027f65f1967a0eb4f32cd6e5d5315".from_hex().unwrap()), 
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
        };
        assert_eq!(expected_header, header);
        let serialized = serialize(&header);
        assert_eq!(serialized.take(), header_bytes);
    }

    #[test]
    fn test_doge_block_headers_serde_2() {
        // block headers of https://dogechain.info/block/3631810 and https://dogechain.info/block/3631811
        #[allow(clippy::zero_prefixed_literal)]
        let headers_bytes: &[u8] = &[
            02, 4, 1, 98, 0, 169, 253, 69, 196, 153, 115, 241, 239, 162, 112, 182, 254, 4, 175, 104, 238, 165, 178, 80,
            67, 77, 109, 241, 134, 124, 3, 242, 203, 235, 211, 98, 185, 102, 124, 144, 105, 144, 228, 58, 25, 26, 29,
            216, 102, 231, 53, 25, 58, 159, 46, 197, 119, 233, 12, 222, 197, 160, 216, 46, 103, 50, 8, 32, 168, 206,
            162, 64, 96, 194, 112, 3, 26, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 85, 3, 44, 175, 30, 65, 216, 16, 40, 191,
            172, 120, 159, 65, 216, 16, 40, 191, 75, 114, 51, 47, 76, 84, 67, 46, 84, 79, 80, 47, 250, 190, 109, 109,
            43, 81, 248, 197, 12, 188, 108, 251, 133, 201, 23, 87, 181, 238, 195, 234, 79, 166, 231, 37, 167, 174, 120,
            157, 213, 105, 44, 122, 118, 203, 54, 251, 1, 0, 0, 0, 0, 0, 0, 0, 155, 13, 193, 150, 4, 0, 0, 0, 0, 0, 0,
            0, 255, 255, 255, 255, 2, 189, 135, 129, 74, 0, 0, 0, 0, 25, 118, 169, 20, 12, 97, 127, 219, 46, 164, 42,
            237, 48, 165, 9, 89, 94, 226, 27, 163, 246, 104, 141, 176, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36,
            170, 33, 169, 237, 189, 110, 250, 90, 149, 67, 81, 162, 34, 33, 76, 16, 172, 85, 135, 43, 220, 178, 255,
            87, 123, 75, 46, 134, 48, 209, 202, 92, 79, 18, 11, 164, 0, 0, 0, 0, 215, 213, 182, 131, 194, 95, 244, 213,
            149, 120, 21, 208, 183, 72, 141, 171, 212, 164, 167, 119, 251, 21, 37, 177, 229, 184, 97, 162, 24, 119,
            242, 161, 4, 209, 134, 48, 129, 122, 174, 143, 140, 6, 234, 87, 92, 113, 77, 128, 196, 62, 199, 14, 21,
            210, 137, 140, 250, 158, 150, 215, 86, 67, 93, 91, 139, 0, 245, 112, 111, 136, 183, 150, 231, 215, 166,
            109, 16, 186, 116, 56, 110, 194, 165, 34, 90, 99, 84, 66, 184, 117, 82, 7, 219, 250, 77, 91, 51, 209, 43,
            142, 88, 192, 2, 229, 82, 194, 220, 219, 237, 19, 233, 162, 174, 32, 217, 118, 222, 150, 192, 215, 97, 141,
            172, 255, 3, 235, 10, 56, 221, 210, 25, 28, 187, 23, 252, 102, 236, 147, 174, 64, 20, 78, 177, 193, 179,
            152, 118, 74, 96, 166, 100, 24, 97, 159, 51, 154, 71, 207, 194, 165, 155, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 32, 31, 50, 15, 173, 125, 76, 148, 26, 189, 206, 165, 146, 170, 134, 156, 146, 171, 130, 145, 255, 94,
            39, 213, 55, 204, 174, 206, 38, 32, 121, 53, 132, 76, 220, 7, 99, 160, 170, 90, 57, 46, 105, 165, 61, 210,
            58, 159, 187, 210, 33, 167, 58, 217, 231, 58, 121, 219, 26, 28, 79, 51, 73, 198, 161, 253, 162, 64, 96, 56,
            160, 1, 26, 179, 6, 202, 226, 4, 1, 98, 0, 251, 54, 203, 118, 122, 44, 105, 213, 157, 120, 174, 167, 37,
            231, 166, 79, 234, 195, 238, 181, 87, 23, 201, 133, 251, 108, 188, 12, 197, 248, 81, 43, 135, 148, 130, 18,
            84, 184, 105, 138, 17, 165, 157, 180, 227, 34, 105, 187, 76, 248, 74, 60, 56, 75, 253, 10, 39, 3, 210, 17,
            35, 239, 79, 73, 100, 163, 64, 96, 73, 207, 2, 26, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 93, 3, 45, 175, 30,
            26, 47, 86, 105, 97, 66, 84, 67, 47, 77, 105, 110, 101, 100, 32, 98, 121, 32, 106, 101, 102, 102, 56, 56,
            56, 56, 47, 44, 250, 190, 109, 109, 97, 206, 202, 198, 28, 194, 8, 255, 246, 174, 74, 110, 232, 94, 195,
            183, 51, 148, 238, 56, 158, 70, 208, 240, 182, 52, 132, 156, 133, 82, 177, 163, 16, 0, 0, 0, 0, 0, 0, 0,
            16, 252, 249, 144, 1, 88, 220, 181, 179, 211, 187, 198, 156, 2, 0, 0, 0, 255, 255, 255, 255, 2, 15, 158,
            162, 74, 0, 0, 0, 0, 25, 118, 169, 20, 225, 108, 40, 20, 110, 212, 134, 156, 25, 11, 63, 11, 220, 24, 216,
            13, 69, 249, 33, 52, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 170, 33, 169, 237, 2, 66, 232, 219,
            204, 165, 29, 255, 185, 205, 155, 16, 117, 64, 45, 24, 80, 20, 179, 238, 117, 246, 211, 22, 9, 119, 54,
            211, 20, 85, 74, 6, 0, 0, 0, 0, 43, 57, 188, 104, 36, 21, 183, 215, 42, 5, 102, 127, 202, 214, 108, 27,
            197, 78, 223, 117, 192, 184, 134, 95, 200, 0, 82, 210, 90, 48, 120, 31, 7, 109, 146, 28, 130, 42, 82, 151,
            152, 163, 13, 231, 93, 146, 206, 199, 97, 18, 81, 19, 20, 6, 180, 179, 243, 8, 66, 160, 156, 116, 142, 49,
            129, 159, 248, 160, 150, 185, 241, 19, 67, 139, 52, 214, 253, 19, 72, 94, 83, 47, 211, 73, 60, 64, 51, 17,
            205, 49, 64, 60, 101, 96, 141, 43, 55, 180, 136, 70, 78, 130, 81, 124, 47, 252, 28, 10, 240, 32, 175, 114,
            198, 188, 17, 161, 166, 212, 248, 96, 171, 190, 173, 10, 150, 239, 161, 243, 217, 9, 169, 105, 92, 111, 42,
            195, 51, 5, 245, 171, 165, 29, 74, 61, 62, 150, 221, 185, 137, 79, 121, 37, 17, 168, 208, 58, 59, 235, 188,
            196, 123, 110, 112, 16, 207, 56, 189, 15, 210, 113, 249, 225, 1, 34, 91, 139, 248, 187, 81, 47, 11, 33,
            234, 33, 211, 194, 103, 248, 88, 69, 209, 229, 119, 113, 197, 177, 190, 178, 170, 56, 78, 205, 245, 238,
            241, 101, 115, 157, 54, 41, 150, 78, 7, 122, 171, 19, 81, 82, 24, 164, 131, 138, 72, 2, 234, 244, 240, 15,
            193, 148, 82, 85, 95, 75, 216, 23, 62, 158, 77, 240, 54, 9, 168, 136, 95, 38, 217, 48, 133, 43, 45, 71,
            124, 138, 211, 25, 134, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 226, 246, 28, 63, 113, 209, 222, 253, 63, 169, 153, 223, 163, 105, 83, 117, 92,
            105, 6, 137, 121, 153, 98, 180, 139, 235, 216, 54, 151, 78, 140, 249, 130, 61, 12, 168, 187, 14, 212, 164,
            65, 32, 137, 209, 167, 140, 244, 182, 71, 110, 180, 21, 135, 85, 93, 252, 166, 190, 24, 216, 150, 239, 125,
            52, 148, 133, 125, 62, 8, 145, 143, 112, 57, 93, 146, 6, 65, 15, 191, 169, 66, 241, 168, 137, 170, 90, 184,
            24, 142, 195, 60, 47, 110, 32, 125, 199, 8, 0, 0, 0, 0, 0, 0, 32, 215, 213, 182, 131, 194, 95, 244, 213,
            149, 120, 21, 208, 183, 72, 141, 171, 212, 164, 167, 119, 251, 21, 37, 177, 229, 184, 97, 162, 24, 119,
            242, 161, 248, 83, 77, 1, 16, 248, 195, 108, 190, 102, 184, 134, 65, 164, 171, 176, 181, 203, 34, 69, 74,
            19, 48, 160, 149, 131, 65, 190, 33, 165, 67, 202, 136, 163, 64, 96, 56, 160, 1, 26, 204, 233, 213, 44,
        ];
        let mut reader = Reader::new(headers_bytes);
        let headers = reader.read_list::<BlockHeader>().unwrap();
        for header in headers.iter() {
            assert_eq!(header.version, AUX_POW_VERSION_DOGE);
            assert!(header.aux_pow.is_some());
        }
        let serialized = serialize_list(&headers);
        assert_eq!(serialized.take(), headers_bytes);
    }

    #[test]
    fn test_doge_block_headers_serde_11() {
        let headers_bytes: &[u8] = &[
            11, 4, 1, 98, 0, 169, 253, 69, 196, 153, 115, 241, 239, 162, 112, 182, 254, 4, 175, 104, 238, 165, 178, 80,
            67, 77, 109, 241, 134, 124, 3, 242, 203, 235, 211, 98, 185, 102, 124, 144, 105, 144, 228, 58, 25, 26, 29,
            216, 102, 231, 53, 25, 58, 159, 46, 197, 119, 233, 12, 222, 197, 160, 216, 46, 103, 50, 8, 32, 168, 206,
            162, 64, 96, 194, 112, 3, 26, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 85, 3, 44, 175, 30, 65, 216, 16, 40, 191,
            172, 120, 159, 65, 216, 16, 40, 191, 75, 114, 51, 47, 76, 84, 67, 46, 84, 79, 80, 47, 250, 190, 109, 109,
            43, 81, 248, 197, 12, 188, 108, 251, 133, 201, 23, 87, 181, 238, 195, 234, 79, 166, 231, 37, 167, 174, 120,
            157, 213, 105, 44, 122, 118, 203, 54, 251, 1, 0, 0, 0, 0, 0, 0, 0, 155, 13, 193, 150, 4, 0, 0, 0, 0, 0, 0,
            0, 255, 255, 255, 255, 2, 189, 135, 129, 74, 0, 0, 0, 0, 25, 118, 169, 20, 12, 97, 127, 219, 46, 164, 42,
            237, 48, 165, 9, 89, 94, 226, 27, 163, 246, 104, 141, 176, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36,
            170, 33, 169, 237, 189, 110, 250, 90, 149, 67, 81, 162, 34, 33, 76, 16, 172, 85, 135, 43, 220, 178, 255,
            87, 123, 75, 46, 134, 48, 209, 202, 92, 79, 18, 11, 164, 0, 0, 0, 0, 215, 213, 182, 131, 194, 95, 244, 213,
            149, 120, 21, 208, 183, 72, 141, 171, 212, 164, 167, 119, 251, 21, 37, 177, 229, 184, 97, 162, 24, 119,
            242, 161, 4, 209, 134, 48, 129, 122, 174, 143, 140, 6, 234, 87, 92, 113, 77, 128, 196, 62, 199, 14, 21,
            210, 137, 140, 250, 158, 150, 215, 86, 67, 93, 91, 139, 0, 245, 112, 111, 136, 183, 150, 231, 215, 166,
            109, 16, 186, 116, 56, 110, 194, 165, 34, 90, 99, 84, 66, 184, 117, 82, 7, 219, 250, 77, 91, 51, 209, 43,
            142, 88, 192, 2, 229, 82, 194, 220, 219, 237, 19, 233, 162, 174, 32, 217, 118, 222, 150, 192, 215, 97, 141,
            172, 255, 3, 235, 10, 56, 221, 210, 25, 28, 187, 23, 252, 102, 236, 147, 174, 64, 20, 78, 177, 193, 179,
            152, 118, 74, 96, 166, 100, 24, 97, 159, 51, 154, 71, 207, 194, 165, 155, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 32, 31, 50, 15, 173, 125, 76, 148, 26, 189, 206, 165, 146, 170, 134, 156, 146, 171, 130, 145, 255, 94,
            39, 213, 55, 204, 174, 206, 38, 32, 121, 53, 132, 76, 220, 7, 99, 160, 170, 90, 57, 46, 105, 165, 61, 210,
            58, 159, 187, 210, 33, 167, 58, 217, 231, 58, 121, 219, 26, 28, 79, 51, 73, 198, 161, 253, 162, 64, 96, 56,
            160, 1, 26, 179, 6, 202, 226, 4, 1, 98, 0, 251, 54, 203, 118, 122, 44, 105, 213, 157, 120, 174, 167, 37,
            231, 166, 79, 234, 195, 238, 181, 87, 23, 201, 133, 251, 108, 188, 12, 197, 248, 81, 43, 135, 148, 130, 18,
            84, 184, 105, 138, 17, 165, 157, 180, 227, 34, 105, 187, 76, 248, 74, 60, 56, 75, 253, 10, 39, 3, 210, 17,
            35, 239, 79, 73, 100, 163, 64, 96, 73, 207, 2, 26, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 93, 3, 45, 175, 30,
            26, 47, 86, 105, 97, 66, 84, 67, 47, 77, 105, 110, 101, 100, 32, 98, 121, 32, 106, 101, 102, 102, 56, 56,
            56, 56, 47, 44, 250, 190, 109, 109, 97, 206, 202, 198, 28, 194, 8, 255, 246, 174, 74, 110, 232, 94, 195,
            183, 51, 148, 238, 56, 158, 70, 208, 240, 182, 52, 132, 156, 133, 82, 177, 163, 16, 0, 0, 0, 0, 0, 0, 0,
            16, 252, 249, 144, 1, 88, 220, 181, 179, 211, 187, 198, 156, 2, 0, 0, 0, 255, 255, 255, 255, 2, 15, 158,
            162, 74, 0, 0, 0, 0, 25, 118, 169, 20, 225, 108, 40, 20, 110, 212, 134, 156, 25, 11, 63, 11, 220, 24, 216,
            13, 69, 249, 33, 52, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 170, 33, 169, 237, 2, 66, 232, 219,
            204, 165, 29, 255, 185, 205, 155, 16, 117, 64, 45, 24, 80, 20, 179, 238, 117, 246, 211, 22, 9, 119, 54,
            211, 20, 85, 74, 6, 0, 0, 0, 0, 43, 57, 188, 104, 36, 21, 183, 215, 42, 5, 102, 127, 202, 214, 108, 27,
            197, 78, 223, 117, 192, 184, 134, 95, 200, 0, 82, 210, 90, 48, 120, 31, 7, 109, 146, 28, 130, 42, 82, 151,
            152, 163, 13, 231, 93, 146, 206, 199, 97, 18, 81, 19, 20, 6, 180, 179, 243, 8, 66, 160, 156, 116, 142, 49,
            129, 159, 248, 160, 150, 185, 241, 19, 67, 139, 52, 214, 253, 19, 72, 94, 83, 47, 211, 73, 60, 64, 51, 17,
            205, 49, 64, 60, 101, 96, 141, 43, 55, 180, 136, 70, 78, 130, 81, 124, 47, 252, 28, 10, 240, 32, 175, 114,
            198, 188, 17, 161, 166, 212, 248, 96, 171, 190, 173, 10, 150, 239, 161, 243, 217, 9, 169, 105, 92, 111, 42,
            195, 51, 5, 245, 171, 165, 29, 74, 61, 62, 150, 221, 185, 137, 79, 121, 37, 17, 168, 208, 58, 59, 235, 188,
            196, 123, 110, 112, 16, 207, 56, 189, 15, 210, 113, 249, 225, 1, 34, 91, 139, 248, 187, 81, 47, 11, 33,
            234, 33, 211, 194, 103, 248, 88, 69, 209, 229, 119, 113, 197, 177, 190, 178, 170, 56, 78, 205, 245, 238,
            241, 101, 115, 157, 54, 41, 150, 78, 7, 122, 171, 19, 81, 82, 24, 164, 131, 138, 72, 2, 234, 244, 240, 15,
            193, 148, 82, 85, 95, 75, 216, 23, 62, 158, 77, 240, 54, 9, 168, 136, 95, 38, 217, 48, 133, 43, 45, 71,
            124, 138, 211, 25, 134, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 226, 246, 28, 63, 113, 209, 222, 253, 63, 169, 153, 223, 163, 105, 83, 117, 92,
            105, 6, 137, 121, 153, 98, 180, 139, 235, 216, 54, 151, 78, 140, 249, 130, 61, 12, 168, 187, 14, 212, 164,
            65, 32, 137, 209, 167, 140, 244, 182, 71, 110, 180, 21, 135, 85, 93, 252, 166, 190, 24, 216, 150, 239, 125,
            52, 148, 133, 125, 62, 8, 145, 143, 112, 57, 93, 146, 6, 65, 15, 191, 169, 66, 241, 168, 137, 170, 90, 184,
            24, 142, 195, 60, 47, 110, 32, 125, 199, 8, 0, 0, 0, 0, 0, 0, 32, 215, 213, 182, 131, 194, 95, 244, 213,
            149, 120, 21, 208, 183, 72, 141, 171, 212, 164, 167, 119, 251, 21, 37, 177, 229, 184, 97, 162, 24, 119,
            242, 161, 248, 83, 77, 1, 16, 248, 195, 108, 190, 102, 184, 134, 65, 164, 171, 176, 181, 203, 34, 69, 74,
            19, 48, 160, 149, 131, 65, 190, 33, 165, 67, 202, 136, 163, 64, 96, 56, 160, 1, 26, 204, 233, 213, 44, 4,
            1, 98, 0, 127, 242, 182, 251, 95, 69, 79, 34, 41, 159, 163, 118, 75, 127, 174, 167, 213, 24, 216, 127, 207,
            163, 50, 3, 17, 234, 95, 113, 224, 95, 254, 27, 25, 155, 156, 158, 158, 206, 228, 229, 229, 128, 166, 131,
            195, 181, 11, 140, 100, 23, 241, 209, 225, 178, 225, 206, 85, 254, 233, 201, 51, 13, 176, 163, 152, 163,
            64, 96, 39, 83, 3, 26, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 82, 3, 46, 175, 30, 25, 77, 105, 110, 101, 100,
            32, 98, 121, 32, 65, 110, 116, 80, 111, 111, 108, 32, 250, 0, 41, 2, 161, 238, 219, 94, 250, 190, 109, 109,
            70, 224, 194, 193, 250, 214, 88, 42, 153, 94, 77, 118, 0, 165, 168, 8, 177, 29, 130, 238, 113, 83, 111,
            194, 210, 192, 123, 229, 112, 29, 64, 195, 1, 0, 0, 0, 0, 0, 0, 0, 140, 27, 0, 0, 48, 0, 0, 0, 255, 255,
            255, 255, 2, 120, 148, 134, 74, 0, 0, 0, 0, 25, 118, 169, 20, 165, 244, 209, 44, 227, 104, 87, 129, 178,
            39, 193, 243, 149, 72, 221, 239, 66, 158, 151, 131, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 170, 33,
            169, 237, 0, 245, 136, 33, 66, 144, 137, 206, 202, 89, 157, 182, 222, 107, 81, 147, 216, 223, 117, 48, 232,
            112, 191, 237, 138, 246, 184, 203, 57, 128, 201, 219, 0, 0, 0, 0, 99, 234, 242, 138, 36, 113, 235, 35, 87,
            47, 93, 29, 251, 128, 105, 151, 9, 22, 187, 155, 53, 166, 136, 242, 146, 2, 0, 0, 0, 0, 0, 0, 6, 250, 141,
            163, 113, 99, 142, 219, 194, 148, 14, 184, 88, 186, 157, 212, 82, 94, 45, 223, 68, 44, 33, 180, 117, 202,
            61, 18, 161, 51, 125, 71, 3, 109, 226, 16, 89, 222, 203, 233, 8, 99, 252, 5, 134, 253, 27, 144, 16, 151,
            191, 28, 180, 30, 216, 45, 161, 247, 233, 210, 197, 10, 207, 160, 245, 229, 66, 101, 138, 152, 113, 211,
            71, 187, 163, 75, 119, 185, 16, 237, 26, 131, 243, 181, 15, 85, 62, 85, 1, 149, 139, 88, 220, 133, 214,
            212, 119, 138, 112, 51, 192, 137, 106, 255, 129, 148, 116, 227, 115, 239, 114, 76, 55, 59, 204, 161, 224,
            44, 43, 8, 227, 210, 235, 102, 165, 185, 87, 1, 245, 120, 193, 151, 102, 172, 224, 155, 13, 157, 107, 28,
            72, 144, 176, 136, 132, 235, 192, 123, 5, 182, 121, 18, 3, 178, 32, 3, 49, 253, 187, 170, 224, 14, 42, 92,
            228, 195, 82, 233, 94, 201, 194, 219, 75, 116, 159, 108, 18, 123, 109, 184, 157, 191, 69, 43, 172, 182, 73,
            50, 57, 36, 41, 156, 167, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 32, 43, 57, 188, 104, 36, 21, 183, 215, 42,
            5, 102, 127, 202, 214, 108, 27, 197, 78, 223, 117, 192, 184, 134, 95, 200, 0, 82, 210, 90, 48, 120, 31,
            137, 48, 71, 115, 137, 248, 113, 69, 23, 118, 219, 89, 21, 92, 232, 17, 167, 192, 204, 244, 14, 214, 201,
            200, 208, 158, 37, 238, 158, 99, 24, 121, 203, 163, 64, 96, 56, 160, 1, 26, 17, 176, 103, 223, 4, 1, 98, 0,
            195, 64, 29, 112, 229, 123, 192, 210, 194, 111, 83, 113, 238, 130, 29, 177, 8, 168, 165, 0, 118, 77, 94,
            153, 42, 88, 214, 250, 193, 194, 224, 70, 241, 105, 113, 53, 110, 118, 153, 81, 206, 58, 26, 91, 27, 243,
            196, 176, 193, 41, 195, 171, 214, 224, 171, 224, 35, 194, 187, 138, 231, 150, 233, 206, 226, 163, 64, 96,
            247, 68, 3, 26, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 85, 3, 46, 175, 30, 65, 216, 16, 41, 12, 18, 29, 33,
            65, 216, 16, 41, 11, 140, 106, 86, 47, 76, 84, 67, 46, 84, 79, 80, 47, 250, 190, 109, 109, 134, 41, 157,
            61, 56, 148, 121, 134, 218, 176, 202, 104, 238, 151, 201, 239, 243, 162, 177, 178, 55, 193, 178, 105, 180,
            75, 199, 69, 4, 215, 214, 125, 1, 0, 0, 0, 0, 0, 0, 0, 103, 15, 221, 16, 31, 0, 0, 0, 0, 0, 0, 0, 255, 255,
            255, 255, 2, 150, 148, 146, 74, 0, 0, 0, 0, 25, 118, 169, 20, 12, 97, 127, 219, 46, 164, 42, 237, 48, 165,
            9, 89, 94, 226, 27, 163, 246, 104, 141, 176, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 170, 33, 169,
            237, 143, 83, 32, 173, 210, 250, 233, 124, 27, 230, 208, 79, 152, 121, 78, 0, 124, 217, 159, 70, 21, 12,
            230, 183, 208, 212, 188, 212, 37, 40, 31, 184, 0, 0, 0, 0, 140, 46, 50, 42, 101, 143, 65, 228, 92, 222, 54,
            48, 240, 136, 62, 111, 106, 14, 138, 138, 146, 193, 241, 150, 19, 53, 20, 153, 161, 6, 54, 133, 8, 250,
            141, 163, 113, 99, 142, 219, 194, 148, 14, 184, 88, 186, 157, 212, 82, 94, 45, 223, 68, 44, 33, 180, 117,
            202, 61, 18, 161, 51, 125, 71, 3, 98, 168, 200, 54, 70, 16, 205, 122, 193, 221, 106, 148, 130, 55, 33, 121,
            205, 210, 29, 154, 169, 186, 49, 248, 108, 182, 46, 36, 199, 215, 79, 62, 158, 92, 133, 27, 216, 206, 189,
            83, 178, 59, 152, 49, 61, 157, 12, 193, 85, 241, 44, 225, 215, 228, 75, 94, 83, 237, 182, 138, 99, 178,
            171, 198, 49, 253, 3, 51, 158, 146, 142, 187, 2, 61, 137, 159, 37, 185, 136, 156, 51, 69, 233, 41, 46, 170,
            151, 88, 79, 26, 248, 34, 65, 146, 63, 82, 58, 166, 191, 123, 140, 87, 56, 174, 106, 67, 142, 18, 186, 152,
            54, 184, 226, 161, 121, 69, 216, 118, 70, 246, 181, 29, 31, 67, 108, 64, 43, 116, 215, 111, 217, 246, 29,
            213, 124, 46, 162, 76, 101, 177, 167, 42, 162, 223, 70, 0, 255, 223, 248, 18, 205, 47, 40, 15, 237, 192,
            158, 26, 244, 134, 22, 228, 182, 78, 135, 244, 108, 232, 163, 185, 191, 254, 63, 201, 222, 1, 61, 226, 80,
            125, 195, 164, 96, 185, 135, 64, 34, 71, 67, 192, 19, 22, 202, 143, 62, 159, 249, 206, 6, 84, 82, 127, 2,
            9, 197, 105, 46, 207, 130, 30, 242, 226, 117, 9, 68, 28, 100, 241, 144, 188, 173, 41, 35, 1, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 32, 43, 57, 188, 104, 36, 21, 183, 215, 42, 5, 102, 127, 202, 214, 108, 27, 197, 78,
            223, 117, 192, 184, 134, 95, 200, 0, 82, 210, 90, 48, 120, 31, 56, 246, 29, 114, 105, 233, 19, 193, 124,
            60, 84, 119, 220, 245, 71, 114, 195, 11, 233, 16, 225, 138, 232, 25, 180, 192, 243, 136, 178, 46, 54, 213,
            46, 164, 64, 96, 56, 160, 1, 26, 201, 167, 65, 127, 4, 1, 98, 0, 125, 214, 215, 4, 69, 199, 75, 180, 105,
            178, 193, 55, 178, 177, 162, 243, 239, 201, 151, 238, 104, 202, 176, 218, 134, 121, 148, 56, 61, 157, 41,
            134, 44, 108, 105, 192, 12, 41, 159, 150, 159, 171, 80, 211, 188, 88, 188, 222, 161, 10, 202, 110, 27, 251,
            70, 120, 97, 151, 15, 219, 37, 138, 87, 197, 54, 164, 64, 96, 234, 82, 3, 26, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255,
            255, 100, 3, 47, 175, 30, 44, 250, 190, 109, 109, 88, 212, 157, 184, 188, 186, 166, 189, 202, 118, 122,
            217, 159, 94, 220, 141, 159, 43, 217, 120, 185, 151, 219, 253, 39, 118, 125, 211, 172, 205, 50, 12, 8, 0,
            0, 0, 240, 159, 144, 159, 0, 22, 77, 105, 110, 101, 100, 32, 98, 121, 32, 121, 117, 49, 51, 56, 50, 54, 52,
            51, 49, 49, 54, 55, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 0, 74, 7, 0, 0, 0, 0,
            0, 0, 2, 2, 132, 133, 74, 0, 0, 0, 0, 25, 118, 169, 20, 170, 55, 80, 170, 24, 184, 160, 243, 240, 89, 7,
            49, 225, 250, 185, 52, 133, 102, 128, 207, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 47, 106, 36, 170, 33, 169,
            237, 197, 208, 117, 85, 8, 227, 136, 127, 166, 234, 92, 24, 16, 111, 123, 88, 144, 54, 173, 234, 154, 76,
            253, 156, 12, 28, 198, 48, 91, 240, 22, 72, 8, 0, 0, 0, 0, 0, 0, 0, 0, 11, 29, 111, 64, 97, 202, 153, 212,
            97, 129, 68, 232, 18, 145, 96, 45, 99, 87, 164, 137, 201, 72, 225, 38, 82, 59, 141, 238, 76, 1, 0, 0, 0, 0,
            0, 0, 4, 91, 72, 242, 93, 202, 249, 61, 197, 124, 127, 39, 102, 95, 41, 133, 228, 253, 157, 109, 40, 93,
            63, 104, 205, 144, 187, 210, 53, 141, 182, 24, 132, 189, 12, 214, 154, 72, 102, 24, 122, 247, 235, 179,
            106, 158, 67, 212, 103, 5, 149, 76, 45, 111, 94, 122, 48, 144, 193, 70, 150, 255, 90, 46, 103, 221, 215,
            48, 120, 9, 150, 90, 45, 81, 8, 20, 199, 59, 209, 35, 186, 173, 132, 198, 206, 234, 172, 40, 100, 34, 137,
            175, 41, 100, 222, 228, 44, 190, 106, 247, 85, 79, 38, 147, 216, 180, 41, 241, 230, 130, 150, 56, 157, 190,
            110, 26, 202, 45, 126, 26, 8, 180, 85, 110, 79, 41, 134, 79, 8, 0, 0, 0, 0, 3, 1, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 75, 209, 126, 243, 171, 30, 25, 51,
            106, 33, 127, 117, 78, 243, 139, 62, 150, 185, 218, 57, 199, 155, 252, 230, 181, 30, 157, 66, 206, 52, 187,
            209, 130, 84, 55, 185, 152, 132, 32, 200, 13, 5, 167, 226, 175, 250, 30, 205, 237, 65, 111, 29, 202, 222,
            90, 106, 202, 141, 173, 130, 195, 249, 208, 236, 0, 0, 0, 0, 0, 0, 0, 32, 140, 46, 50, 42, 101, 143, 65,
            228, 92, 222, 54, 48, 240, 136, 62, 111, 106, 14, 138, 138, 146, 193, 241, 150, 19, 53, 20, 153, 161, 6,
            54, 133, 156, 181, 164, 233, 242, 108, 31, 124, 140, 26, 212, 14, 0, 205, 102, 31, 144, 210, 91, 228, 76,
            175, 253, 87, 83, 74, 11, 204, 70, 174, 11, 89, 57, 164, 64, 96, 56, 160, 1, 26, 174, 31, 103, 0, 4, 1, 98,
            0, 96, 123, 127, 101, 230, 81, 218, 12, 252, 115, 88, 237, 31, 72, 145, 164, 49, 201, 24, 121, 12, 237,
            219, 60, 102, 140, 46, 32, 14, 231, 9, 67, 233, 169, 50, 134, 135, 19, 237, 132, 6, 207, 187, 219, 211,
            177, 215, 56, 4, 3, 253, 218, 105, 233, 74, 159, 17, 234, 243, 221, 2, 150, 191, 130, 60, 164, 64, 96, 117,
            125, 3, 26, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 75, 3, 48, 175, 30, 8, 47, 86, 105, 97, 66, 84, 67, 47, 44,
            250, 190, 109, 109, 68, 166, 169, 32, 48, 213, 62, 52, 32, 96, 182, 92, 3, 109, 112, 245, 52, 226, 77, 231,
            69, 186, 14, 63, 186, 145, 12, 47, 87, 236, 140, 8, 16, 0, 0, 0, 0, 0, 0, 0, 16, 19, 250, 144, 1, 88, 220,
            181, 179, 211, 200, 111, 132, 15, 0, 0, 0, 255, 255, 255, 255, 2, 3, 93, 168, 74, 0, 0, 0, 0, 25, 118, 169,
            20, 225, 108, 40, 20, 110, 212, 134, 156, 25, 11, 63, 11, 220, 24, 216, 13, 69, 249, 33, 52, 136, 172, 0,
            0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 170, 33, 169, 237, 173, 96, 6, 68, 177, 184, 150, 248, 112, 15, 131, 45,
            46, 212, 181, 126, 185, 108, 224, 4, 75, 189, 145, 44, 85, 129, 40, 198, 10, 86, 218, 63, 0, 0, 0, 0, 116,
            58, 63, 178, 140, 32, 42, 202, 20, 147, 18, 209, 152, 207, 51, 106, 119, 252, 235, 73, 211, 14, 9, 247,
            214, 133, 232, 114, 124, 39, 122, 191, 7, 31, 193, 63, 72, 147, 95, 147, 236, 23, 144, 134, 151, 83, 249,
            147, 59, 142, 120, 115, 231, 158, 194, 205, 138, 68, 4, 77, 25, 216, 95, 130, 251, 237, 201, 148, 24, 101,
            210, 76, 201, 96, 203, 82, 113, 214, 73, 232, 14, 209, 120, 156, 111, 183, 173, 114, 105, 60, 2, 204, 155,
            241, 134, 35, 16, 216, 118, 245, 253, 36, 13, 127, 84, 146, 241, 148, 168, 189, 116, 29, 182, 218, 100,
            221, 159, 190, 2, 222, 109, 208, 167, 90, 110, 150, 154, 26, 44, 188, 49, 0, 53, 200, 234, 152, 57, 170,
            97, 40, 15, 186, 170, 71, 76, 12, 100, 209, 123, 234, 191, 22, 118, 255, 143, 190, 227, 131, 117, 63, 243,
            70, 85, 75, 103, 180, 17, 207, 122, 182, 69, 126, 53, 197, 126, 213, 62, 107, 140, 248, 82, 59, 250, 11,
            35, 31, 234, 233, 27, 244, 91, 124, 240, 180, 87, 62, 42, 220, 247, 153, 154, 117, 123, 3, 55, 175, 167,
            128, 90, 195, 173, 9, 116, 202, 89, 97, 206, 178, 147, 208, 156, 245, 160, 188, 164, 89, 41, 134, 102, 172,
            35, 206, 207, 81, 243, 185, 212, 55, 234, 136, 9, 156, 232, 164, 227, 163, 226, 93, 96, 218, 6, 26, 48,
            129, 233, 22, 241, 0, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 226, 246, 28, 63, 113, 209, 222, 253, 63, 169, 153, 223, 163, 105, 83, 117, 92,
            105, 6, 137, 121, 153, 98, 180, 139, 235, 216, 54, 151, 78, 140, 249, 221, 191, 73, 154, 220, 40, 170, 13,
            219, 177, 241, 157, 204, 38, 82, 96, 115, 11, 198, 108, 58, 212, 231, 226, 197, 227, 98, 67, 250, 59, 220,
            53, 148, 133, 125, 62, 8, 145, 143, 112, 57, 93, 146, 6, 65, 15, 191, 169, 66, 241, 168, 137, 170, 90, 184,
            24, 142, 195, 60, 47, 110, 32, 125, 199, 8, 0, 0, 0, 0, 0, 0, 32, 201, 115, 36, 45, 58, 79, 229, 129, 3,
            108, 192, 53, 24, 119, 16, 93, 7, 221, 10, 3, 223, 92, 169, 249, 74, 200, 248, 120, 188, 179, 20, 152, 60,
            178, 19, 5, 249, 153, 8, 71, 128, 182, 209, 162, 255, 197, 146, 4, 243, 49, 81, 44, 82, 51, 230, 189, 179,
            77, 2, 225, 56, 72, 32, 177, 133, 164, 64, 96, 56, 160, 1, 26, 20, 156, 133, 165, 4, 1, 98, 0, 153, 149,
            116, 125, 194, 160, 252, 2, 116, 242, 146, 49, 63, 170, 126, 206, 239, 30, 195, 100, 232, 4, 172, 130, 61,
            30, 159, 182, 66, 133, 108, 90, 23, 91, 78, 213, 223, 152, 152, 186, 205, 106, 101, 111, 89, 40, 240, 129,
            187, 206, 245, 161, 209, 173, 18, 50, 105, 84, 222, 210, 215, 127, 56, 57, 162, 164, 64, 96, 28, 36, 3, 26,
            0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 255, 255, 255, 255, 68, 3, 49, 175, 30, 4, 96, 64, 164, 164, 44, 250, 190, 109, 109, 147,
            177, 176, 242, 92, 47, 142, 134, 133, 113, 175, 233, 241, 21, 31, 22, 100, 64, 79, 59, 101, 159, 242, 113,
            131, 190, 217, 102, 170, 54, 126, 55, 64, 0, 0, 0, 0, 0, 0, 0, 4, 47, 76, 80, 47, 8, 87, 0, 58, 51, 46, 58,
            15, 0, 255, 255, 255, 255, 2, 46, 24, 133, 74, 0, 0, 0, 0, 25, 118, 169, 20, 87, 117, 126, 221, 0, 29, 22,
            82, 140, 122, 163, 55, 179, 20, 167, 186, 179, 3, 238, 128, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36,
            170, 33, 169, 237, 42, 222, 139, 222, 195, 247, 213, 195, 168, 13, 64, 183, 19, 215, 165, 61, 0, 68, 81,
            149, 33, 147, 177, 16, 75, 120, 166, 151, 91, 66, 47, 36, 0, 0, 0, 0, 180, 167, 192, 191, 239, 47, 151, 30,
            205, 186, 67, 45, 46, 97, 96, 72, 175, 50, 226, 132, 15, 58, 225, 117, 163, 141, 113, 134, 162, 171, 132,
            207, 4, 188, 18, 165, 182, 217, 69, 62, 77, 138, 124, 118, 134, 51, 131, 171, 86, 2, 70, 175, 168, 21, 72,
            36, 197, 196, 187, 144, 16, 171, 82, 90, 1, 102, 86, 234, 192, 243, 68, 49, 233, 227, 148, 21, 204, 147,
            28, 254, 191, 221, 132, 33, 169, 10, 140, 206, 91, 172, 129, 36, 96, 95, 183, 247, 0, 150, 88, 85, 237, 56,
            221, 41, 245, 157, 90, 35, 132, 89, 140, 32, 73, 74, 34, 40, 37, 59, 46, 50, 177, 53, 44, 97, 127, 64, 215,
            16, 61, 10, 111, 144, 74, 204, 222, 3, 219, 225, 131, 196, 93, 198, 77, 108, 154, 101, 24, 238, 241, 70,
            251, 8, 225, 62, 151, 124, 163, 233, 154, 96, 37, 0, 0, 0, 0, 6, 169, 75, 0, 160, 219, 218, 93, 170, 181,
            242, 13, 15, 90, 157, 202, 35, 184, 91, 192, 226, 71, 47, 219, 82, 254, 121, 174, 38, 200, 242, 183, 178,
            226, 246, 28, 63, 113, 209, 222, 253, 63, 169, 153, 223, 163, 105, 83, 117, 92, 105, 6, 137, 121, 153, 98,
            180, 139, 235, 216, 54, 151, 78, 140, 249, 125, 36, 219, 43, 250, 65, 71, 75, 251, 47, 135, 125, 104, 143,
            172, 95, 170, 94, 16, 162, 128, 140, 249, 222, 48, 115, 112, 185, 51, 82, 229, 72, 148, 133, 125, 62, 8,
            145, 143, 112, 57, 93, 146, 6, 65, 15, 191, 169, 66, 241, 168, 137, 170, 90, 184, 24, 142, 195, 60, 47,
            110, 32, 125, 199, 25, 191, 18, 3, 211, 191, 72, 57, 60, 105, 204, 37, 89, 137, 20, 187, 158, 13, 48, 47,
            54, 61, 152, 37, 219, 160, 185, 251, 149, 156, 163, 59, 131, 94, 152, 183, 67, 103, 222, 106, 139, 79, 135,
            78, 127, 44, 207, 118, 55, 89, 10, 91, 196, 86, 147, 90, 68, 152, 126, 97, 221, 155, 22, 213, 56, 0, 0, 0,
            0, 0, 0, 32, 116, 58, 63, 178, 140, 32, 42, 202, 20, 147, 18, 209, 152, 207, 51, 106, 119, 252, 235, 73,
            211, 14, 9, 247, 214, 133, 232, 114, 124, 39, 122, 191, 159, 134, 106, 61, 11, 4, 76, 200, 150, 163, 95,
            19, 121, 38, 83, 145, 251, 176, 150, 86, 170, 170, 77, 183, 180, 179, 24, 189, 54, 39, 43, 242, 164, 164,
            64, 96, 56, 160, 1, 26, 186, 94, 110, 179, 4, 1, 98, 0, 248, 201, 65, 186, 199, 106, 99, 193, 9, 53, 177,
            191, 123, 236, 127, 194, 229, 63, 31, 164, 183, 154, 96, 150, 99, 128, 43, 31, 23, 108, 48, 104, 21, 116,
            206, 160, 55, 244, 216, 243, 18, 59, 136, 106, 122, 58, 185, 20, 149, 68, 158, 171, 188, 49, 215, 0, 105,
            232, 236, 205, 200, 140, 82, 246, 245, 164, 64, 96, 30, 103, 3, 26, 0, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 95,
            3, 50, 175, 30, 4, 21, 165, 64, 96, 47, 112, 111, 111, 108, 105, 110, 46, 99, 111, 109, 47, 250, 190, 109,
            109, 142, 90, 93, 145, 31, 124, 63, 207, 192, 13, 154, 171, 25, 226, 121, 7, 102, 110, 245, 207, 26, 247,
            189, 114, 181, 100, 42, 151, 226, 17, 161, 147, 1, 0, 0, 0, 0, 0, 0, 0, 228, 9, 235, 160, 185, 253, 167,
            88, 4, 52, 32, 223, 144, 199, 207, 103, 0, 72, 48, 9, 167, 0, 88, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255,
            2, 180, 245, 168, 74, 0, 0, 0, 0, 25, 118, 169, 20, 53, 64, 57, 186, 154, 102, 102, 246, 183, 41, 162, 87,
            15, 184, 182, 96, 232, 111, 20, 34, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 170, 33, 169, 237, 118,
            14, 151, 127, 13, 2, 95, 34, 67, 78, 193, 253, 219, 198, 72, 213, 178, 192, 49, 46, 223, 2, 87, 170, 86,
            245, 49, 235, 55, 242, 212, 164, 212, 103, 151, 122, 152, 32, 74, 200, 210, 143, 84, 20, 53, 72, 99, 226,
            209, 136, 249, 236, 118, 148, 29, 28, 32, 146, 65, 78, 20, 238, 29, 4, 242, 82, 151, 235, 7, 209, 231, 195,
            143, 37, 99, 131, 144, 245, 58, 15, 138, 133, 55, 138, 44, 54, 158, 236, 91, 233, 134, 57, 112, 164, 102,
            96, 112, 166, 13, 31, 246, 37, 99, 181, 0, 84, 124, 246, 166, 40, 183, 37, 232, 108, 235, 170, 138, 196,
            247, 121, 59, 141, 139, 110, 189, 156, 11, 120, 133, 58, 127, 108, 228, 149, 185, 40, 247, 62, 97, 232,
            170, 13, 138, 66, 109, 137, 249, 119, 77, 3, 205, 108, 229, 147, 163, 135, 28, 101, 154, 101, 253, 145, 75,
            171, 241, 188, 255, 254, 178, 203, 73, 113, 231, 119, 24, 80, 177, 220, 173, 111, 156, 251, 203, 172, 246,
            179, 112, 91, 116, 6, 22, 251, 55, 235, 80, 78, 82, 104, 7, 211, 188, 7, 233, 212, 171, 138, 37, 135, 9,
            160, 232, 149, 149, 82, 18, 1, 29, 247, 161, 71, 77, 154, 210, 48, 227, 127, 81, 164, 137, 173, 225, 137,
            153, 205, 33, 193, 148, 206, 72, 235, 54, 210, 129, 94, 180, 144, 79, 199, 74, 8, 192, 234, 163, 186, 217,
            6, 231, 238, 161, 220, 157, 61, 160, 203, 46, 54, 204, 88, 190, 62, 200, 226, 167, 186, 210, 152, 81, 98,
            154, 185, 69, 49, 68, 199, 251, 192, 186, 232, 203, 106, 142, 250, 172, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            32, 180, 167, 192, 191, 239, 47, 151, 30, 205, 186, 67, 45, 46, 97, 96, 72, 175, 50, 226, 132, 15, 58, 225,
            117, 163, 141, 113, 134, 162, 171, 132, 207, 180, 21, 0, 204, 206, 63, 72, 7, 175, 163, 245, 202, 211, 255,
            54, 139, 135, 197, 86, 69, 146, 87, 98, 37, 53, 167, 180, 177, 208, 144, 221, 84, 21, 165, 64, 96, 56, 160,
            1, 26, 91, 138, 208, 11, 4, 1, 98, 0, 147, 161, 17, 226, 151, 42, 100, 181, 114, 189, 247, 26, 207, 245,
            110, 102, 7, 121, 226, 25, 171, 154, 13, 192, 207, 63, 124, 31, 145, 93, 90, 142, 146, 76, 109, 197, 146,
            103, 4, 159, 253, 66, 28, 234, 199, 16, 182, 11, 127, 33, 230, 192, 113, 130, 198, 134, 249, 20, 243, 36,
            211, 241, 228, 139, 119, 165, 64, 96, 39, 132, 3, 26, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 85, 3, 51, 175,
            30, 65, 216, 16, 41, 112, 86, 133, 73, 65, 216, 16, 41, 111, 143, 197, 111, 47, 76, 84, 67, 46, 84, 79, 80,
            47, 250, 190, 109, 109, 176, 224, 45, 106, 57, 181, 93, 44, 27, 243, 159, 117, 177, 30, 185, 130, 62, 199,
            82, 168, 130, 92, 112, 47, 198, 200, 96, 42, 121, 253, 175, 164, 1, 0, 0, 0, 0, 0, 0, 0, 156, 10, 228, 69,
            39, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 2, 28, 157, 236, 74, 0, 0, 0, 0, 25, 118, 169, 20, 12, 97,
            127, 219, 46, 164, 42, 237, 48, 165, 9, 89, 94, 226, 27, 163, 246, 104, 141, 176, 136, 172, 0, 0, 0, 0, 0,
            0, 0, 0, 38, 106, 36, 170, 33, 169, 237, 170, 163, 60, 127, 31, 150, 63, 14, 98, 181, 178, 228, 44, 176,
            108, 167, 136, 90, 255, 23, 64, 18, 20, 133, 115, 198, 122, 241, 172, 92, 116, 187, 0, 0, 0, 0, 48, 225,
            109, 224, 8, 81, 217, 199, 69, 83, 191, 161, 73, 188, 43, 205, 45, 203, 79, 81, 114, 183, 123, 84, 82, 174,
            63, 19, 189, 19, 57, 90, 8, 132, 143, 197, 58, 149, 18, 240, 47, 19, 38, 63, 162, 224, 31, 35, 60, 3, 25,
            10, 192, 213, 11, 222, 132, 246, 218, 228, 96, 5, 14, 191, 4, 126, 250, 123, 209, 51, 208, 53, 0, 58, 137,
            176, 222, 153, 187, 69, 97, 33, 43, 54, 78, 173, 23, 121, 165, 110, 108, 183, 239, 70, 123, 36, 246, 7,
            237, 38, 115, 113, 140, 67, 2, 67, 60, 208, 43, 196, 155, 41, 5, 1, 91, 128, 183, 20, 55, 219, 66, 75, 151,
            203, 212, 232, 249, 53, 68, 63, 222, 122, 211, 152, 68, 92, 233, 129, 28, 207, 69, 53, 71, 48, 189, 29, 96,
            250, 190, 99, 145, 108, 26, 71, 148, 109, 82, 74, 141, 182, 13, 68, 71, 221, 187, 86, 29, 131, 196, 203, 8,
            105, 181, 156, 236, 145, 118, 24, 238, 52, 194, 29, 23, 167, 17, 134, 108, 66, 183, 156, 59, 234, 82, 11,
            183, 123, 95, 85, 49, 144, 147, 134, 230, 251, 98, 233, 114, 250, 1, 252, 219, 127, 222, 152, 72, 112, 76,
            123, 166, 192, 14, 227, 95, 114, 188, 35, 197, 131, 35, 125, 68, 109, 190, 43, 200, 42, 80, 42, 72, 35,
            134, 51, 206, 211, 173, 153, 22, 200, 187, 14, 106, 176, 252, 5, 1, 1, 110, 60, 13, 59, 126, 223, 53, 195,
            218, 66, 211, 84, 8, 156, 134, 168, 185, 22, 152, 72, 249, 15, 123, 106, 37, 33, 49, 176, 31, 35, 44, 147,
            204, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 32, 235, 151, 82, 242, 4, 29, 238, 20, 78, 65, 146, 32, 28, 29,
            148, 118, 236, 249, 136, 209, 226, 99, 72, 53, 20, 84, 143, 210, 200, 74, 32, 152, 170, 122, 183, 101, 144,
            105, 148, 138, 192, 77, 206, 58, 51, 162, 16, 223, 86, 15, 229, 47, 222, 119, 229, 23, 198, 182, 62, 90,
            186, 221, 46, 26, 190, 165, 64, 96, 56, 160, 1, 26, 134, 175, 237, 47, 4, 1, 98, 0, 164, 175, 253, 121, 42,
            96, 200, 198, 47, 112, 92, 130, 168, 82, 199, 62, 130, 185, 30, 177, 117, 159, 243, 27, 44, 93, 181, 57,
            106, 45, 224, 176, 104, 26, 209, 156, 235, 23, 154, 40, 28, 172, 150, 156, 67, 191, 239, 171, 33, 109, 209,
            118, 16, 42, 221, 18, 146, 22, 204, 159, 30, 160, 185, 126, 221, 165, 64, 96, 44, 252, 3, 26, 0, 0, 0, 0,
            1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 255, 255, 255, 255, 93, 3, 52, 175, 30, 26, 47, 86, 105, 97, 66, 84, 67, 47, 77, 105, 110, 101, 100, 32,
            98, 121, 32, 99, 104, 105, 110, 116, 101, 115, 116, 47, 44, 250, 190, 109, 109, 71, 132, 173, 215, 163,
            251, 170, 225, 64, 121, 24, 16, 91, 236, 101, 62, 171, 116, 120, 170, 246, 250, 23, 15, 140, 137, 21, 195,
            141, 248, 86, 70, 16, 0, 0, 0, 0, 0, 0, 0, 16, 135, 201, 90, 2, 144, 177, 144, 102, 60, 161, 5, 149, 27, 0,
            0, 0, 255, 255, 255, 255, 2, 194, 105, 133, 74, 0, 0, 0, 0, 25, 118, 169, 20, 225, 108, 40, 20, 110, 212,
            134, 156, 25, 11, 63, 11, 220, 24, 216, 13, 69, 249, 33, 52, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36,
            170, 33, 169, 237, 84, 235, 234, 182, 80, 32, 21, 164, 1, 47, 141, 222, 118, 59, 215, 217, 4, 103, 7, 207,
            71, 220, 72, 231, 108, 90, 121, 217, 144, 134, 137, 232, 0, 0, 0, 0, 160, 13, 44, 14, 68, 222, 92, 212,
            151, 150, 0, 232, 21, 238, 177, 71, 22, 88, 189, 162, 190, 219, 125, 234, 140, 153, 109, 85, 90, 4, 212,
            233, 5, 191, 72, 80, 83, 51, 241, 56, 211, 24, 155, 32, 218, 23, 52, 138, 136, 216, 49, 80, 162, 6, 72, 14,
            49, 76, 17, 4, 127, 183, 65, 169, 235, 53, 129, 190, 193, 212, 217, 11, 11, 210, 160, 41, 38, 79, 151, 75,
            135, 159, 203, 189, 25, 128, 16, 82, 109, 161, 49, 13, 227, 27, 162, 68, 35, 172, 202, 192, 106, 206, 58,
            135, 119, 69, 170, 217, 27, 224, 252, 228, 169, 38, 220, 216, 175, 183, 147, 171, 255, 55, 232, 36, 62, 92,
            142, 62, 217, 213, 128, 253, 128, 134, 125, 77, 137, 83, 182, 137, 186, 209, 230, 237, 84, 124, 28, 201,
            25, 113, 150, 164, 159, 65, 48, 53, 161, 237, 237, 245, 117, 150, 235, 154, 26, 199, 1, 190, 15, 114, 41,
            42, 252, 28, 51, 36, 78, 205, 132, 169, 10, 72, 50, 222, 159, 101, 135, 236, 7, 192, 35, 64, 37, 0, 0, 0,
            0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 226,
            246, 28, 63, 113, 209, 222, 253, 63, 169, 153, 223, 163, 105, 83, 117, 92, 105, 6, 137, 121, 153, 98, 180,
            139, 235, 216, 54, 151, 78, 140, 249, 125, 36, 219, 43, 250, 65, 71, 75, 251, 47, 135, 125, 104, 143, 172,
            95, 170, 94, 16, 162, 128, 140, 249, 222, 48, 115, 112, 185, 51, 82, 229, 72, 148, 133, 125, 62, 8, 145,
            143, 112, 57, 93, 146, 6, 65, 15, 191, 169, 66, 241, 168, 137, 170, 90, 184, 24, 142, 195, 60, 47, 110, 32,
            125, 199, 8, 0, 0, 0, 0, 0, 0, 32, 48, 225, 109, 224, 8, 81, 217, 199, 69, 83, 191, 161, 73, 188, 43, 205,
            45, 203, 79, 81, 114, 183, 123, 84, 82, 174, 63, 19, 189, 19, 57, 90, 249, 66, 35, 177, 57, 61, 98, 237,
            138, 201, 173, 73, 213, 230, 84, 145, 18, 223, 85, 143, 226, 168, 193, 242, 205, 129, 67, 129, 252, 61,
            223, 214, 223, 165, 64, 96, 56, 160, 1, 26, 152, 136, 213, 186, 4, 1, 98, 0, 148, 84, 227, 50, 4, 34, 22,
            8, 68, 171, 136, 209, 192, 152, 158, 156, 0, 21, 64, 28, 109, 70, 28, 89, 156, 190, 71, 74, 54, 95, 28, 90,
            27, 58, 215, 222, 107, 142, 190, 105, 121, 156, 229, 76, 180, 122, 147, 228, 218, 200, 130, 30, 110, 237,
            174, 143, 241, 114, 112, 107, 225, 47, 197, 152, 7, 166, 64, 96, 47, 81, 4, 26, 0, 0, 0, 0, 1, 0, 0, 0, 1,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255,
            255, 255, 100, 3, 53, 175, 30, 44, 250, 190, 109, 109, 1, 42, 250, 75, 173, 255, 178, 225, 162, 236, 160,
            191, 155, 108, 122, 164, 55, 99, 33, 76, 76, 126, 225, 97, 217, 247, 150, 238, 148, 91, 195, 110, 8, 0, 0,
            0, 240, 159, 144, 159, 0, 14, 77, 105, 110, 101, 100, 32, 98, 121, 32, 106, 122, 103, 103, 121, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 0, 2, 0, 0, 0, 0, 0, 0, 0, 2,
            112, 160, 140, 74, 0, 0, 0, 0, 25, 118, 169, 20, 170, 55, 80, 170, 24, 184, 160, 243, 240, 89, 7, 49, 225,
            250, 185, 52, 133, 102, 128, 207, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 47, 106, 36, 170, 33, 169, 237, 173,
            157, 54, 156, 8, 57, 108, 141, 41, 38, 49, 168, 34, 125, 114, 203, 134, 104, 216, 155, 107, 53, 37, 208,
            120, 15, 134, 153, 132, 175, 203, 63, 8, 0, 0, 0, 0, 0, 0, 0, 0, 41, 9, 231, 60, 224, 190, 222, 59, 134, 9,
            156, 162, 78, 235, 105, 206, 100, 155, 198, 14, 50, 244, 234, 176, 208, 123, 252, 166, 138, 3, 0, 0, 0, 0,
            0, 0, 6, 20, 26, 112, 82, 50, 27, 168, 85, 116, 36, 9, 11, 151, 247, 227, 240, 228, 37, 5, 45, 117, 8, 207,
            141, 40, 31, 210, 202, 18, 14, 209, 86, 26, 44, 216, 41, 220, 204, 42, 1, 109, 131, 136, 14, 42, 59, 187,
            21, 21, 197, 49, 50, 112, 100, 155, 15, 211, 214, 119, 190, 175, 253, 31, 118, 145, 213, 221, 130, 144, 48,
            116, 253, 178, 53, 86, 177, 198, 42, 177, 120, 229, 74, 175, 108, 58, 134, 6, 9, 99, 85, 156, 243, 68, 190,
            112, 56, 216, 152, 195, 158, 88, 36, 122, 45, 133, 71, 45, 28, 154, 165, 22, 32, 6, 250, 217, 12, 103, 147,
            116, 254, 111, 21, 210, 254, 222, 161, 122, 114, 138, 154, 16, 11, 146, 145, 253, 141, 228, 63, 158, 168,
            254, 217, 139, 47, 64, 65, 242, 255, 141, 253, 14, 219, 49, 98, 177, 147, 53, 182, 101, 189, 49, 40, 210,
            44, 28, 223, 177, 34, 146, 195, 160, 1, 26, 130, 243, 43, 102, 224, 113, 213, 139, 51, 165, 190, 39, 176,
            88, 245, 65, 40, 37, 211, 0, 0, 0, 0, 3, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 75, 209, 126, 243, 171, 30, 25, 51, 106, 33, 127, 117, 78, 243, 139, 62, 150,
            185, 218, 57, 199, 155, 252, 230, 181, 30, 157, 66, 206, 52, 187, 209, 68, 11, 140, 189, 202, 48, 30, 215,
            217, 161, 96, 242, 143, 12, 96, 142, 163, 56, 70, 191, 17, 51, 42, 238, 153, 149, 134, 73, 44, 159, 109,
            254, 0, 0, 0, 0, 0, 0, 0, 32, 160, 13, 44, 14, 68, 222, 92, 212, 151, 150, 0, 232, 21, 238, 177, 71, 22,
            88, 189, 162, 190, 219, 125, 234, 140, 153, 109, 85, 90, 4, 212, 233, 5, 240, 197, 194, 187, 134, 238, 8,
            46, 252, 212, 68, 187, 21, 12, 8, 44, 190, 138, 206, 178, 177, 35, 12, 250, 23, 103, 161, 194, 138, 83, 57,
            9, 166, 64, 96, 56, 160, 1, 26, 83, 116, 249, 236,
        ];
        let mut reader = Reader::new(headers_bytes);
        let headers = reader.read_list::<BlockHeader>().unwrap();
        for header in headers.iter() {
            assert_eq!(header.version, AUX_POW_VERSION_DOGE);
            assert!(header.aux_pow.is_some());
        }
        let serialized = serialize_list(&headers);
        assert_eq!(serialized.take(), headers_bytes);
    }

    #[test]
    fn test_firo_block_headers_prog_pow() {
        let header_hex = "0010002000a0f9fd846c553ab0f42da219319846c24946306d83153f9232578375ef0d786a30257842bd435908bb8392e27caee825dc6af229d3fd117f422972e8191642cff13e624011081b760f0700de2da2e90000004795c4b220117e6b0d326188de71879e14ad9887838ff3cb1d0c146da2805d7361";
        let header_bytes: Vec<u8> = header_hex.from_hex().unwrap();
        let header: BlockHeader = deserialize(header_bytes.as_slice()).unwrap();
        assert!(header.time >= PROG_POW_SWITCH_TIME);
        let serialized = serialize(&header);
        assert_eq!(serialized.take(), header_bytes);
    }

    #[test]
    fn test_firo_block_headers_serde_11() {
        let headers_bytes: &[u8] = &[
            11, 0, 16, 0, 32, 211, 25, 36, 95, 143, 251, 214, 115, 13, 244, 188, 48, 166, 108, 254, 16, 136, 240, 63,
            167, 167, 224, 62, 7, 211, 75, 1, 239, 132, 183, 230, 110, 10, 110, 245, 37, 15, 224, 212, 228, 21, 15, 0,
            128, 129, 25, 129, 193, 114, 115, 104, 246, 220, 231, 174, 93, 118, 53, 55, 93, 171, 3, 31, 46, 76, 177,
            89, 96, 81, 74, 76, 27, 233, 64, 41, 10, 0, 0, 16, 0, 139, 248, 9, 214, 44, 78, 251, 22, 56, 201, 250, 184,
            205, 174, 73, 97, 138, 42, 139, 105, 185, 224, 219, 51, 84, 162, 48, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 16, 0, 32, 153, 197, 87, 0, 55, 153, 49, 34,
            210, 50, 234, 47, 192, 101, 168, 113, 193, 155, 29, 55, 18, 44, 195, 11, 122, 224, 245, 241, 176, 202, 37,
            194, 45, 20, 140, 84, 113, 189, 7, 30, 91, 208, 50, 227, 168, 65, 36, 26, 185, 219, 144, 127, 217, 44, 76,
            16, 250, 12, 73, 36, 179, 239, 224, 202, 37, 178, 89, 96, 81, 74, 76, 27, 32, 46, 19, 67, 0, 0, 16, 0, 40,
            235, 31, 35, 3, 33, 13, 116, 22, 217, 255, 188, 217, 124, 60, 239, 38, 25, 239, 186, 126, 69, 9, 114, 187,
            90, 53, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 16, 0, 32, 76, 220, 116, 84, 109, 245, 38, 55, 168, 33, 146, 206, 244, 226, 195, 44, 60, 101, 236, 156,
            193, 18, 175, 34, 160, 149, 154, 60, 190, 31, 96, 204, 157, 22, 72, 59, 72, 224, 72, 19, 198, 11, 127, 55,
            231, 9, 192, 142, 192, 160, 90, 123, 8, 29, 113, 55, 180, 225, 28, 198, 34, 109, 110, 8, 227, 182, 89, 96,
            81, 74, 76, 27, 193, 247, 243, 9, 0, 0, 16, 0, 57, 92, 165, 169, 163, 235, 158, 149, 71, 177, 170, 102, 80,
            3, 202, 78, 173, 165, 219, 47, 152, 10, 121, 173, 85, 35, 56, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 16, 0, 32, 160, 246, 226, 204, 225, 182, 239, 27,
            16, 249, 7, 94, 201, 180, 97, 36, 136, 101, 45, 243, 233, 25, 54, 188, 107, 39, 42, 87, 163, 92, 81, 40, 4,
            114, 205, 28, 170, 122, 168, 201, 254, 122, 213, 79, 192, 130, 238, 218, 198, 175, 242, 115, 39, 197, 231,
            24, 54, 129, 166, 138, 251, 43, 132, 31, 28, 183, 89, 96, 81, 74, 76, 27, 202, 165, 177, 65, 0, 0, 16, 0,
            148, 112, 192, 209, 86, 225, 156, 242, 176, 130, 123, 241, 76, 252, 72, 31, 52, 77, 65, 137, 49, 151, 0,
            116, 26, 159, 15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 16, 0, 32, 232, 31, 230, 78, 144, 41, 125, 54, 87, 47, 223, 83, 174, 236, 84, 213, 50, 133,
            105, 131, 216, 250, 228, 54, 87, 245, 97, 206, 231, 197, 54, 104, 240, 86, 139, 171, 13, 111, 24, 126, 154,
            72, 142, 100, 56, 142, 95, 186, 181, 221, 234, 110, 155, 203, 247, 12, 56, 254, 134, 113, 221, 19, 23, 51,
            62, 184, 89, 96, 8, 93, 76, 27, 197, 129, 145, 3, 0, 0, 16, 0, 107, 63, 136, 53, 144, 91, 234, 23, 214,
            127, 133, 147, 37, 35, 72, 54, 199, 186, 164, 115, 1, 167, 192, 54, 92, 130, 40, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 16, 0, 32, 45, 209, 148, 141, 0,
            18, 2, 111, 207, 192, 193, 81, 244, 191, 4, 45, 40, 61, 252, 229, 5, 160, 8, 220, 114, 131, 248, 156, 137,
            139, 26, 61, 247, 93, 113, 238, 9, 9, 70, 132, 127, 82, 93, 18, 249, 201, 182, 112, 127, 64, 58, 26, 188,
            251, 124, 89, 8, 79, 251, 134, 182, 150, 149, 51, 85, 184, 89, 96, 8, 93, 76, 27, 62, 175, 117, 13, 0, 0,
            16, 0, 223, 200, 239, 222, 7, 188, 10, 137, 16, 217, 24, 158, 78, 14, 82, 148, 81, 131, 171, 236, 83, 133,
            166, 39, 185, 142, 70, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 16, 0, 32, 109, 233, 220, 44, 121, 249, 48, 133, 249, 24, 39, 90, 16, 17, 181, 124, 12,
            14, 135, 131, 151, 129, 169, 53, 131, 181, 139, 212, 42, 149, 85, 231, 35, 166, 60, 202, 137, 173, 15, 162,
            167, 182, 93, 148, 147, 159, 138, 148, 74, 147, 174, 137, 222, 105, 128, 149, 101, 131, 100, 184, 42, 140,
            31, 52, 212, 184, 89, 96, 8, 93, 76, 27, 128, 158, 192, 10, 0, 0, 16, 0, 67, 243, 15, 127, 142, 136, 141,
            135, 54, 231, 87, 117, 203, 58, 214, 127, 162, 137, 20, 194, 253, 28, 93, 7, 43, 16, 59, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 16, 0, 32, 90, 249,
            130, 156, 151, 83, 35, 173, 142, 186, 184, 41, 65, 165, 82, 107, 162, 128, 53, 113, 180, 246, 107, 128,
            208, 245, 46, 3, 212, 97, 184, 79, 153, 197, 97, 80, 115, 138, 217, 53, 79, 169, 89, 7, 245, 172, 158, 81,
            185, 188, 42, 127, 103, 77, 228, 245, 224, 46, 39, 198, 166, 166, 158, 145, 27, 185, 89, 96, 8, 93, 76, 27,
            183, 15, 142, 2, 0, 0, 16, 0, 174, 28, 73, 99, 132, 248, 236, 186, 224, 251, 253, 103, 186, 143, 153, 246,
            82, 4, 213, 99, 246, 111, 155, 154, 130, 61, 29, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 16, 0, 32, 122, 180, 53, 6, 207, 18, 246, 63, 14, 18, 77, 23,
            190, 124, 155, 196, 254, 143, 81, 53, 101, 219, 171, 150, 139, 216, 242, 173, 231, 21, 223, 198, 74, 219,
            114, 120, 53, 226, 150, 88, 173, 55, 43, 250, 170, 157, 87, 157, 151, 139, 160, 119, 12, 2, 36, 191, 134,
            168, 70, 64, 213, 19, 57, 66, 78, 186, 89, 96, 8, 93, 76, 27, 59, 203, 122, 30, 0, 0, 16, 0, 229, 102, 179,
            37, 13, 188, 69, 167, 131, 196, 20, 23, 123, 230, 172, 242, 25, 57, 11, 85, 210, 19, 226, 68, 236, 195, 53,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 16,
            0, 32, 150, 15, 128, 62, 201, 141, 77, 178, 30, 65, 24, 183, 252, 50, 158, 32, 22, 16, 250, 133, 124, 89,
            16, 142, 1, 206, 246, 208, 159, 87, 195, 82, 157, 254, 118, 38, 80, 162, 91, 208, 2, 168, 180, 208, 84,
            189, 227, 95, 26, 169, 124, 4, 227, 85, 233, 164, 127, 100, 112, 119, 27, 224, 147, 120, 33, 187, 89, 96,
            8, 93, 76, 27, 163, 206, 141, 22, 0, 0, 16, 0, 40, 155, 42, 147, 9, 142, 1, 26, 1, 199, 46, 150, 174, 35,
            51, 151, 134, 0, 118, 163, 163, 237, 146, 134, 240, 162, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 16, 0, 32, 224, 45, 255, 146, 132, 163, 233, 151,
            51, 188, 159, 226, 253, 79, 54, 19, 79, 27, 69, 173, 86, 33, 205, 154, 122, 102, 127, 146, 155, 47, 118,
            235, 135, 103, 36, 126, 178, 21, 135, 192, 108, 15, 190, 238, 217, 17, 23, 45, 167, 214, 80, 26, 131, 1,
            229, 63, 48, 164, 180, 152, 134, 234, 116, 141, 232, 187, 89, 96, 8, 93, 76, 27, 5, 90, 172, 9, 0, 0, 16,
            0, 146, 56, 110, 135, 109, 183, 136, 204, 44, 86, 19, 165, 184, 3, 116, 130, 3, 91, 208, 236, 188, 215,
            128, 185, 13, 39, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0,
        ];
        let mut reader = Reader::new(headers_bytes);
        let headers = reader.read_list::<BlockHeader>().unwrap();
        for header in headers.iter() {
            assert_eq!(header.version, MTP_POW_VERSION);
        }
        let serialized = serialize_list(&headers);
        assert_eq!(serialized.take(), headers_bytes);
    }

    #[test]
    fn test_lbc_block_herders_serde_11() {
        let headers_bytes: &[u8] = &[
            11, 0, 0, 0, 32, 4, 226, 205, 70, 162, 151, 56, 130, 229, 25, 56, 118, 91, 160, 178, 217, 85, 27, 1, 56,
            140, 20, 226, 40, 81, 171, 69, 77, 146, 40, 3, 245, 151, 115, 182, 25, 85, 159, 63, 6, 216, 43, 43, 130,
            111, 1, 126, 229, 53, 112, 195, 185, 7, 49, 80, 227, 104, 71, 129, 155, 14, 80, 229, 38, 102, 215, 134, 81,
            191, 250, 187, 217, 169, 93, 203, 220, 82, 217, 39, 221, 45, 2, 138, 18, 143, 248, 133, 15, 59, 140, 214,
            169, 65, 98, 208, 61, 46, 30, 213, 98, 213, 174, 0, 26, 75, 221, 59, 116, 0, 0, 0, 32, 196, 109, 14, 170,
            9, 36, 64, 47, 8, 64, 125, 180, 223, 157, 207, 10, 17, 147, 142, 204, 121, 184, 222, 20, 96, 107, 117, 222,
            175, 16, 105, 186, 108, 207, 33, 68, 226, 33, 122, 81, 173, 178, 158, 158, 8, 214, 102, 132, 57, 8, 233,
            138, 194, 133, 26, 124, 84, 0, 79, 31, 203, 142, 45, 232, 94, 218, 52, 2, 164, 160, 62, 236, 233, 187, 156,
            61, 122, 43, 214, 177, 87, 143, 144, 214, 64, 70, 168, 150, 210, 240, 78, 38, 204, 182, 2, 226, 137, 30,
            213, 98, 207, 195, 0, 26, 26, 210, 86, 33, 0, 0, 0, 32, 248, 242, 27, 4, 83, 243, 188, 89, 225, 120, 109,
            155, 92, 211, 112, 226, 110, 106, 117, 155, 244, 215, 210, 185, 63, 178, 180, 223, 137, 162, 58, 251, 107,
            190, 58, 159, 209, 134, 141, 129, 158, 66, 135, 230, 248, 146, 217, 232, 6, 92, 244, 181, 139, 203, 184,
            35, 35, 209, 240, 53, 56, 222, 63, 95, 180, 75, 178, 246, 193, 163, 175, 209, 16, 201, 188, 164, 245, 0,
            44, 216, 64, 8, 22, 170, 213, 238, 21, 240, 118, 16, 197, 201, 177, 29, 109, 91, 217, 30, 213, 98, 171,
            186, 0, 26, 3, 57, 180, 217, 0, 0, 0, 32, 107, 241, 118, 66, 209, 221, 160, 24, 69, 212, 76, 221, 81, 47,
            150, 6, 196, 22, 254, 107, 67, 96, 165, 129, 93, 87, 86, 253, 174, 166, 240, 6, 87, 49, 250, 185, 201, 18,
            49, 115, 222, 30, 154, 134, 214, 114, 183, 95, 87, 218, 116, 4, 176, 126, 196, 187, 210, 197, 142, 91, 129,
            175, 158, 88, 141, 159, 87, 247, 179, 75, 237, 84, 58, 216, 37, 210, 72, 167, 226, 154, 15, 69, 10, 254,
            149, 135, 74, 149, 110, 178, 91, 9, 134, 36, 137, 183, 57, 32, 213, 98, 182, 176, 0, 26, 184, 35, 19, 17,
            0, 0, 0, 32, 54, 175, 218, 241, 215, 229, 54, 226, 65, 178, 208, 185, 207, 176, 61, 171, 1, 203, 94, 53,
            30, 40, 188, 54, 70, 35, 33, 68, 234, 37, 203, 110, 248, 170, 156, 152, 52, 72, 42, 28, 248, 97, 9, 159,
            106, 83, 6, 68, 106, 134, 222, 38, 97, 227, 211, 181, 75, 36, 154, 182, 130, 133, 161, 8, 94, 210, 79, 13,
            155, 245, 191, 77, 157, 95, 40, 208, 220, 69, 190, 192, 248, 200, 39, 148, 177, 163, 88, 210, 23, 209, 106,
            200, 239, 23, 147, 3, 87, 32, 213, 98, 41, 206, 0, 26, 36, 6, 216, 89, 0, 0, 0, 32, 191, 206, 127, 113,
            217, 20, 89, 51, 103, 46, 155, 38, 64, 52, 51, 218, 35, 153, 161, 11, 24, 225, 116, 139, 117, 96, 245, 112,
            218, 168, 88, 99, 13, 117, 103, 10, 163, 133, 114, 186, 178, 138, 120, 22, 245, 23, 32, 108, 186, 238, 18,
            250, 254, 20, 236, 152, 247, 1, 229, 145, 249, 121, 29, 19, 64, 176, 250, 1, 212, 155, 54, 168, 176, 22,
            10, 174, 183, 38, 88, 77, 17, 174, 221, 73, 84, 133, 174, 149, 222, 93, 168, 85, 47, 122, 254, 142, 191,
            32, 213, 98, 139, 185, 0, 26, 11, 156, 81, 208, 0, 0, 0, 32, 185, 10, 134, 138, 197, 14, 90, 184, 108, 77,
            52, 169, 245, 158, 113, 196, 92, 63, 10, 42, 93, 138, 96, 146, 248, 103, 105, 107, 110, 92, 153, 32, 46,
            134, 91, 9, 151, 59, 17, 87, 145, 104, 83, 124, 153, 71, 154, 44, 254, 1, 16, 179, 214, 82, 219, 179, 57,
            94, 110, 85, 137, 231, 104, 113, 216, 65, 67, 201, 131, 14, 28, 55, 72, 31, 197, 88, 146, 130, 243, 56,
            173, 30, 153, 236, 36, 53, 220, 45, 18, 114, 217, 223, 59, 74, 60, 143, 214, 35, 213, 98, 91, 179, 0, 26,
            184, 79, 48, 125, 0, 0, 0, 32, 207, 9, 44, 157, 84, 2, 136, 174, 93, 198, 173, 105, 35, 141, 243, 6, 21,
            214, 24, 80, 113, 166, 149, 36, 207, 78, 157, 77, 195, 76, 118, 75, 93, 168, 103, 196, 147, 125, 5, 145,
            183, 30, 98, 150, 37, 135, 29, 3, 184, 160, 62, 61, 205, 109, 89, 124, 161, 161, 99, 166, 76, 198, 57, 104,
            21, 200, 173, 105, 181, 83, 152, 148, 115, 92, 215, 131, 108, 16, 121, 168, 25, 212, 150, 158, 205, 1, 128,
            106, 232, 31, 177, 216, 61, 70, 106, 149, 19, 36, 213, 98, 8, 13, 1, 26, 66, 126, 250, 117, 0, 0, 0, 32,
            14, 226, 195, 145, 216, 149, 97, 248, 36, 168, 95, 51, 163, 63, 36, 79, 144, 213, 195, 185, 88, 79, 179,
            68, 6, 166, 42, 176, 56, 129, 130, 158, 157, 158, 185, 82, 70, 100, 135, 53, 130, 252, 84, 56, 153, 183, 4,
            64, 167, 75, 237, 171, 159, 117, 76, 115, 169, 115, 131, 251, 119, 23, 3, 24, 159, 110, 115, 78, 6, 134,
            71, 167, 91, 187, 203, 252, 77, 121, 169, 201, 152, 67, 155, 21, 5, 19, 61, 136, 215, 31, 55, 173, 225,
            126, 138, 230, 51, 36, 213, 98, 77, 249, 0, 26, 153, 65, 133, 15, 0, 0, 0, 32, 98, 23, 129, 255, 228, 196,
            211, 128, 56, 58, 241, 153, 48, 139, 73, 26, 39, 169, 57, 4, 91, 179, 149, 36, 158, 254, 190, 185, 52, 110,
            160, 218, 227, 60, 248, 234, 204, 240, 216, 126, 70, 68, 180, 22, 122, 203, 103, 19, 177, 126, 226, 83, 44,
            83, 2, 36, 157, 164, 127, 241, 219, 47, 100, 53, 86, 216, 76, 242, 161, 117, 71, 214, 50, 154, 144, 42,
            180, 57, 145, 57, 95, 10, 18, 217, 161, 66, 70, 4, 116, 79, 24, 64, 71, 88, 47, 160, 77, 38, 213, 98, 8,
            226, 0, 26, 6, 75, 190, 72, 0, 0, 0, 32, 92, 201, 103, 9, 253, 13, 166, 231, 44, 6, 225, 162, 236, 10, 255,
            4, 42, 43, 196, 53, 83, 118, 220, 157, 231, 6, 104, 2, 111, 141, 219, 28, 162, 101, 33, 136, 243, 11, 130,
            200, 145, 88, 52, 186, 115, 44, 146, 231, 80, 158, 155, 182, 134, 156, 229, 247, 68, 55, 145, 249, 71, 225,
            115, 127, 165, 240, 245, 7, 151, 6, 193, 202, 139, 227, 48, 79, 12, 178, 233, 104, 176, 129, 136, 171, 201,
            184, 15, 6, 37, 118, 246, 130, 5, 177, 140, 190, 58, 38, 213, 98, 92, 42, 1, 26, 63, 91, 195, 195,
        ];
        let mut reader = Reader::new_with_coin_variant(headers_bytes, CoinVariant::LBC);
        let headers = reader.read_list::<BlockHeader>().unwrap();
        for header in headers.iter() {
            assert_eq!(header.version, BIP9_NO_SOFT_FORK_BLOCK_HEADER_VERSION);
        }
        let serialized = serialize_list(&headers);
        assert_eq!(serialized.take(), headers_bytes);
    }

    #[test]
    fn test_nmc_block_headers_serde_11() {
        const AUX_POW_VERSION_NMC: u32 = 65796;
        // NMC block headers
        // start - #622807
        // end - #622796
        // Ref: https://chainz.cryptoid.info/nmc/block.dws?622807.htm
        let headers_bytes: Bytes = include_str!("for_tests/nmc_block_headers_hex").into();
        let headers_bytes = headers_bytes.as_slice();
        let mut reader = Reader::new(headers_bytes);
        let headers = reader.read_list::<BlockHeader>().unwrap();
        for header in headers.iter() {
            assert_eq!(header.version, AUX_POW_VERSION_NMC);
            assert!(header.aux_pow.is_some());
        }
        let serialized = serialize_list(&headers);
        assert_eq!(serialized.take(), headers_bytes);
    }

    #[test]
    fn test_verus_block_headers_serde_11() {
        let headers_bytes: &[u8] = &[
            11, 4, 0, 1, 0, 240, 78, 41, 165, 99, 246, 89, 216, 246, 137, 15, 91, 165, 80, 198, 192, 19, 139, 33, 228,
            3, 240, 183, 44, 215, 109, 14, 0, 0, 0, 0, 0, 190, 198, 65, 161, 80, 213, 207, 57, 93, 23, 215, 209, 139,
            149, 214, 252, 148, 236, 169, 150, 168, 171, 68, 254, 45, 25, 89, 89, 140, 49, 118, 187, 44, 50, 68, 177,
            104, 32, 149, 113, 251, 70, 149, 167, 158, 105, 70, 61, 78, 237, 4, 252, 175, 239, 111, 209, 9, 200, 250,
            138, 92, 39, 186, 115, 117, 145, 121, 96, 81, 61, 16, 27, 178, 160, 110, 57, 162, 1, 61, 210, 224, 92, 93,
            114, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 253, 64, 5, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 144, 170, 142, 48, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 4, 0, 1, 0, 232, 1, 74, 16, 207, 203, 102, 50, 197, 251, 130, 71, 4, 200, 42, 135, 140, 249,
            72, 152, 247, 123, 199, 148, 167, 128, 1, 0, 0, 0, 0, 0, 247, 56, 112, 149, 35, 22, 187, 36, 229, 247, 64,
            61, 37, 5, 97, 116, 216, 209, 196, 32, 52, 158, 123, 182, 62, 35, 150, 37, 97, 64, 63, 98, 44, 50, 68, 177,
            104, 32, 149, 113, 251, 70, 149, 167, 158, 105, 70, 61, 78, 237, 4, 252, 175, 239, 111, 209, 9, 200, 250,
            138, 92, 39, 186, 115, 188, 145, 121, 96, 57, 247, 19, 27, 243, 206, 6, 26, 130, 106, 66, 222, 56, 234,
            178, 183, 35, 183, 184, 160, 135, 58, 199, 98, 82, 101, 180, 179, 127, 107, 216, 74, 160, 129, 117, 209,
            253, 64, 5, 5, 0, 0, 0, 0, 0, 0, 0, 240, 157, 131, 17, 196, 216, 178, 7, 99, 130, 92, 243, 160, 48, 14, 76,
            35, 169, 124, 203, 5, 109, 157, 186, 138, 33, 66, 92, 171, 128, 60, 131, 19, 38, 225, 207, 251, 76, 189,
            24, 22, 240, 69, 254, 22, 105, 134, 229, 90, 194, 203, 8, 15, 93, 101, 17, 227, 56, 213, 106, 231, 26, 155,
            127, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 1, 0, 227, 22, 82, 165, 71, 25, 190, 77, 123, 82, 33, 24,
            212, 52, 121, 38, 72, 194, 188, 61, 151, 243, 75, 110, 167, 123, 188, 26, 115, 124, 0, 49, 152, 58, 7, 163,
            185, 105, 219, 114, 90, 10, 112, 23, 167, 110, 250, 124, 28, 1, 13, 107, 106, 131, 234, 101, 80, 43, 249,
            101, 174, 197, 254, 104, 44, 50, 68, 177, 104, 32, 149, 113, 251, 70, 149, 167, 158, 105, 70, 61, 78, 237,
            4, 252, 175, 239, 111, 209, 9, 200, 250, 138, 92, 39, 186, 115, 190, 145, 121, 96, 71, 18, 20, 27, 78, 35,
            7, 26, 145, 184, 222, 189, 139, 59, 12, 54, 37, 145, 18, 3, 121, 98, 249, 113, 17, 72, 11, 169, 136, 100,
            233, 120, 49, 214, 103, 56, 253, 64, 5, 5, 0, 0, 0, 0, 0, 0, 0, 222, 31, 184, 129, 105, 14, 170, 143, 157,
            156, 168, 173, 250, 70, 13, 51, 20, 197, 32, 190, 216, 140, 195, 196, 188, 142, 255, 164, 69, 89, 164, 184,
            84, 231, 129, 164, 174, 62, 211, 237, 111, 65, 199, 100, 95, 163, 102, 22, 48, 166, 99, 118, 147, 209, 143,
            202, 145, 154, 127, 116, 83, 71, 127, 42, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 1, 0, 164, 164, 154,
            183, 0, 3, 117, 163, 49, 223, 82, 216, 193, 164, 43, 153, 167, 37, 6, 111, 32, 89, 1, 58, 115, 129, 148,
            142, 136, 183, 159, 11, 173, 128, 38, 230, 208, 81, 99, 126, 216, 143, 229, 235, 88, 37, 209, 208, 215, 84,
            160, 116, 25, 53, 8, 135, 219, 96, 8, 29, 185, 87, 174, 169, 44, 50, 68, 177, 104, 32, 149, 113, 251, 70,
            149, 167, 158, 105, 70, 61, 78, 237, 4, 252, 175, 239, 111, 209, 9, 200, 250, 138, 92, 39, 186, 115, 193,
            145, 121, 96, 8, 75, 19, 27, 197, 237, 6, 26, 219, 79, 142, 239, 41, 46, 47, 101, 225, 150, 3, 214, 232,
            193, 198, 41, 178, 212, 114, 151, 53, 179, 42, 229, 59, 8, 37, 93, 253, 64, 5, 5, 0, 0, 0, 0, 0, 0, 0, 139,
            191, 176, 241, 111, 169, 150, 197, 73, 253, 168, 243, 107, 222, 74, 126, 223, 61, 135, 68, 13, 235, 10,
            189, 66, 155, 150, 232, 169, 11, 28, 93, 30, 220, 165, 241, 16, 162, 250, 197, 41, 214, 40, 61, 153, 172,
            234, 87, 222, 144, 64, 11, 253, 109, 115, 5, 67, 22, 180, 222, 248, 39, 106, 45, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 4, 0, 1, 0, 110, 16, 59, 232, 64, 32, 180, 250, 52, 196, 148, 201, 255, 109, 228, 131, 149, 225,
            23, 120, 66, 200, 168, 201, 39, 158, 236, 52, 212, 209, 85, 71, 65, 178, 119, 35, 198, 231, 91, 70, 229,
            234, 110, 114, 67, 205, 33, 1, 77, 120, 155, 236, 40, 140, 148, 190, 187, 119, 29, 12, 144, 47, 39, 36, 44,
            50, 68, 177, 104, 32, 149, 113, 251, 70, 149, 167, 158, 105, 70, 61, 78, 237, 4, 252, 175, 239, 111, 209,
            9, 200, 250, 138, 92, 39, 186, 115, 197, 145, 121, 96, 75, 139, 18, 27, 237, 161, 6, 26, 58, 96, 229, 184,
            235, 49, 189, 43, 6, 29, 188, 247, 123, 53, 171, 62, 187, 24, 143, 181, 253, 87, 120, 38, 93, 30, 95, 91,
            253, 64, 5, 5, 0, 0, 0, 0, 0, 0, 0, 11, 116, 26, 113, 195, 31, 178, 79, 89, 243, 43, 212, 68, 193, 56, 99,
            234, 232, 184, 160, 230, 24, 128, 145, 154, 148, 78, 21, 110, 229, 193, 81, 244, 9, 180, 132, 191, 62, 113,
            97, 125, 139, 130, 207, 92, 62, 157, 39, 76, 26, 251, 96, 160, 119, 7, 219, 242, 11, 161, 30, 42, 249, 245,
            229, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 1, 0, 107, 139, 48, 179, 141, 210, 160, 37, 98, 21, 23,
            105, 1, 188, 95, 119, 186, 18, 250, 3, 102, 34, 196, 109, 215, 63, 214, 134, 108, 128, 188, 141, 86, 54,
            213, 223, 26, 235, 196, 166, 209, 88, 97, 236, 187, 241, 187, 128, 7, 198, 176, 103, 22, 44, 4, 174, 196,
            199, 81, 196, 243, 15, 31, 191, 44, 50, 68, 177, 104, 32, 149, 113, 251, 70, 149, 167, 158, 105, 70, 61,
            78, 237, 4, 252, 175, 239, 111, 209, 9, 200, 250, 138, 92, 39, 186, 115, 198, 145, 121, 96, 189, 197, 17,
            27, 181, 71, 6, 26, 205, 108, 109, 245, 33, 234, 198, 172, 2, 227, 207, 243, 132, 56, 94, 27, 145, 93, 224,
            4, 35, 115, 30, 159, 1, 23, 55, 48, 253, 64, 5, 5, 0, 0, 0, 0, 0, 0, 0, 2, 163, 65, 33, 173, 224, 199, 203,
            28, 110, 80, 219, 141, 36, 117, 67, 202, 57, 209, 152, 211, 203, 8, 116, 115, 206, 64, 178, 36, 248, 209,
            196, 251, 160, 96, 178, 220, 132, 50, 213, 64, 69, 28, 24, 50, 95, 82, 83, 99, 20, 37, 84, 234, 72, 116,
            85, 79, 44, 42, 25, 133, 39, 200, 31, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 1, 0, 91, 32, 83, 42, 110,
            208, 204, 250, 242, 146, 75, 254, 29, 133, 64, 118, 187, 150, 123, 235, 83, 216, 66, 251, 72, 118, 11, 114,
            166, 182, 68, 50, 146, 249, 167, 132, 17, 197, 93, 145, 91, 80, 248, 77, 93, 127, 83, 49, 210, 190, 142,
            243, 254, 193, 40, 0, 82, 108, 83, 177, 87, 222, 187, 219, 44, 50, 68, 177, 104, 32, 149, 113, 251, 70,
            149, 167, 158, 105, 70, 61, 78, 237, 4, 252, 175, 239, 111, 209, 9, 200, 250, 138, 92, 39, 186, 115, 196,
            145, 121, 96, 188, 1, 17, 27, 214, 233, 5, 26, 23, 216, 68, 17, 203, 88, 158, 107, 184, 179, 68, 15, 36,
            211, 53, 68, 217, 114, 161, 40, 120, 118, 46, 251, 68, 114, 65, 82, 253, 64, 5, 5, 0, 0, 0, 0, 0, 0, 0,
            154, 12, 59, 67, 85, 85, 211, 206, 251, 37, 198, 98, 4, 85, 153, 102, 123, 146, 156, 102, 69, 229, 1, 114,
            201, 20, 32, 226, 107, 102, 118, 163, 175, 228, 254, 100, 0, 22, 210, 200, 165, 221, 129, 122, 74, 253,
            153, 129, 39, 242, 206, 83, 163, 130, 9, 247, 172, 96, 212, 223, 169, 132, 189, 102, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 4, 0, 1, 0, 133, 66, 37, 207, 133, 6, 212, 148, 137, 132, 190, 103, 170, 253, 186, 230, 217,
            138, 159, 110, 202, 237, 233, 22, 243, 18, 254, 139, 74, 189, 36, 181, 128, 179, 255, 172, 182, 56, 157,
            21, 209, 11, 141, 35, 86, 86, 117, 185, 42, 165, 134, 85, 51, 211, 25, 253, 152, 56, 133, 94, 73, 168, 147,
            33, 44, 50, 68, 177, 104, 32, 149, 113, 251, 70, 149, 167, 158, 105, 70, 61, 78, 237, 4, 252, 175, 239,
            111, 209, 9, 200, 250, 138, 92, 39, 186, 115, 249, 145, 121, 96, 81, 52, 16, 27, 180, 234, 19, 70, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 253, 64, 5, 5, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 215,
            149, 115, 122, 0, 0, 0, 0, 4, 0, 1, 0, 224, 176, 64, 33, 109, 75, 1, 7, 107, 194, 140, 253, 30, 175, 124,
            58, 15, 231, 213, 76, 164, 32, 237, 162, 245, 30, 8, 0, 0, 0, 0, 0, 17, 244, 69, 164, 19, 125, 119, 33, 99,
            95, 81, 74, 228, 229, 105, 38, 72, 65, 52, 48, 122, 60, 85, 7, 159, 74, 78, 138, 222, 246, 134, 142, 44,
            50, 68, 177, 104, 32, 149, 113, 251, 70, 149, 167, 158, 105, 70, 61, 78, 237, 4, 252, 175, 239, 111, 209,
            9, 200, 250, 138, 92, 39, 186, 115, 251, 145, 121, 96, 212, 23, 16, 27, 108, 10, 169, 49, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 3, 6, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 253, 64, 5, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 95, 201, 213, 2, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 4, 0, 1, 0, 66, 66, 75, 50, 181, 237, 122, 125, 31, 192, 8, 56, 216, 77, 180, 204, 103, 174,
            25, 71, 147, 234, 213, 24, 2, 2, 2, 0, 0, 0, 0, 0, 213, 75, 69, 111, 43, 49, 191, 188, 45, 24, 189, 124,
            235, 65, 45, 235, 56, 122, 10, 245, 75, 2, 145, 2, 109, 187, 242, 57, 127, 146, 69, 28, 44, 50, 68, 177,
            104, 32, 149, 113, 251, 70, 149, 167, 158, 105, 70, 61, 78, 237, 4, 252, 175, 239, 111, 209, 9, 200, 250,
            138, 92, 39, 186, 115, 2, 146, 121, 96, 111, 88, 15, 27, 181, 85, 27, 59, 48, 115, 78, 48, 120, 178, 143,
            2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 253, 64, 5, 5, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 53, 39, 209, 96, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 4, 0, 1, 0, 15, 162, 153, 82, 186, 205, 43, 97, 224, 118, 74, 101, 254, 223, 128, 118, 219, 124,
            75, 49, 21, 16, 182, 240, 176, 117, 7, 0, 0, 0, 0, 0, 83, 84, 207, 208, 72, 6, 13, 190, 53, 210, 7, 5, 228,
            219, 245, 201, 106, 82, 0, 19, 180, 164, 56, 211, 84, 49, 53, 152, 32, 197, 223, 2, 44, 50, 68, 177, 104,
            32, 149, 113, 251, 70, 149, 167, 158, 105, 70, 61, 78, 237, 4, 252, 175, 239, 111, 209, 9, 200, 250, 138,
            92, 39, 186, 115, 81, 146, 121, 96, 76, 164, 14, 27, 160, 228, 5, 26, 17, 83, 63, 189, 122, 133, 237, 149,
            194, 112, 40, 106, 6, 84, 217, 0, 16, 11, 102, 160, 112, 67, 211, 109, 252, 205, 2, 132, 253, 64, 5, 5, 0,
            0, 0, 0, 0, 0, 0, 60, 84, 191, 26, 116, 227, 61, 253, 132, 154, 61, 146, 45, 143, 249, 1, 17, 63, 113, 157,
            5, 146, 6, 202, 79, 79, 109, 182, 213, 100, 18, 184, 117, 199, 119, 69, 88, 44, 75, 235, 138, 30, 107, 132,
            172, 148, 149, 20, 252, 241, 219, 94, 5, 78, 195, 138, 53, 73, 150, 250, 102, 108, 139, 199, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0,
        ];
        let mut reader = Reader::new(headers_bytes);
        let headers = reader.read_list::<BlockHeader>().unwrap();
        for header in headers.iter() {
            assert_eq!(header.version, 4);
        }
        let serialized = serialize_list(&headers);
        assert_eq!(serialized.take(), headers_bytes);
    }

    #[test]
    fn test_sys_block_headers_serde_11() {
        const AUX_POW_VERSION_SYS: u32 = 537919744;
        let headers_bytes: &[u8] = &[
            11, 0, 1, 16, 32, 224, 75, 244, 161, 185, 248, 38, 215, 191, 60, 214, 46, 170, 129, 104, 51, 104, 181, 69,
            119, 171, 121, 183, 144, 38, 57, 67, 7, 99, 24, 201, 157, 180, 182, 0, 37, 120, 169, 194, 158, 75, 173,
            221, 24, 95, 225, 193, 118, 236, 140, 211, 55, 133, 19, 95, 54, 31, 113, 242, 43, 106, 3, 40, 125, 223,
            135, 143, 96, 140, 254, 2, 24, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 100, 3, 182, 102, 10, 44, 250, 190,
            109, 109, 110, 125, 25, 186, 64, 147, 107, 100, 23, 55, 22, 127, 104, 194, 252, 224, 66, 33, 21, 196, 31,
            91, 140, 209, 48, 120, 78, 68, 167, 50, 13, 88, 16, 0, 0, 0, 240, 159, 144, 159, 8, 47, 70, 50, 80, 111,
            111, 108, 47, 18, 77, 105, 110, 101, 100, 32, 98, 121, 32, 116, 49, 55, 108, 105, 97, 110, 104, 101, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 0, 142, 146, 1, 0, 0, 0, 0, 0, 5, 218, 169, 10, 43, 0, 0,
            0, 0, 25, 118, 169, 20, 200, 37, 161, 236, 242, 166, 131, 12, 68, 1, 98, 12, 58, 22, 241, 153, 80, 87, 194,
            171, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 170, 33, 169, 237, 62, 114, 40, 49, 251, 46, 110, 211,
            155, 249, 96, 126, 50, 98, 175, 208, 76, 139, 81, 193, 231, 179, 246, 163, 118, 142, 11, 67, 2, 159, 220,
            132, 0, 0, 0, 0, 0, 0, 0, 0, 54, 106, 52, 72, 97, 116, 104, 125, 169, 173, 86, 243, 74, 144, 174, 82, 60,
            84, 226, 235, 42, 226, 215, 198, 251, 63, 112, 133, 173, 185, 253, 154, 102, 122, 101, 60, 198, 103, 73, 8,
            157, 200, 197, 223, 111, 72, 111, 144, 137, 242, 15, 129, 7, 70, 142, 0, 0, 0, 0, 0, 0, 0, 0, 44, 106, 76,
            41, 82, 83, 75, 66, 76, 79, 67, 75, 58, 255, 219, 148, 186, 86, 177, 155, 128, 115, 251, 138, 148, 83, 236,
            49, 155, 107, 157, 111, 109, 234, 116, 132, 223, 150, 177, 223, 35, 0, 50, 160, 204, 0, 0, 0, 0, 0, 0, 0,
            0, 38, 106, 36, 185, 225, 27, 109, 164, 237, 69, 26, 137, 171, 127, 96, 127, 226, 106, 103, 105, 107, 198,
            170, 136, 55, 139, 215, 196, 17, 38, 232, 159, 110, 18, 100, 39, 60, 255, 205, 111, 128, 75, 63, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 103, 78, 60, 243,
            201, 154, 100, 47, 201, 240, 6, 50, 180, 45, 226, 132, 81, 62, 232, 210, 78, 46, 233, 80, 194, 66, 57, 211,
            164, 242, 10, 194, 8, 206, 226, 101, 164, 212, 15, 81, 160, 154, 137, 213, 62, 214, 37, 38, 86, 167, 183,
            234, 52, 46, 213, 24, 58, 89, 122, 77, 192, 60, 228, 16, 15, 7, 36, 89, 77, 151, 60, 56, 141, 19, 200, 80,
            15, 22, 170, 168, 158, 97, 58, 155, 179, 163, 87, 210, 219, 217, 67, 212, 94, 105, 156, 141, 96, 61, 255,
            29, 61, 52, 157, 84, 209, 73, 87, 49, 92, 31, 153, 130, 1, 25, 122, 159, 224, 1, 147, 16, 246, 143, 180,
            57, 250, 134, 31, 131, 5, 126, 10, 220, 252, 192, 227, 114, 44, 144, 189, 172, 38, 8, 129, 220, 70, 140,
            11, 249, 1, 181, 185, 158, 19, 195, 114, 215, 53, 211, 201, 100, 29, 221, 106, 202, 221, 128, 209, 200,
            174, 21, 40, 41, 138, 196, 164, 219, 208, 201, 18, 199, 72, 242, 22, 127, 105, 194, 36, 39, 11, 134, 118,
            121, 65, 89, 48, 227, 206, 176, 80, 163, 53, 105, 98, 5, 83, 144, 130, 74, 95, 203, 199, 217, 149, 103,
            116, 202, 63, 5, 39, 131, 97, 0, 15, 6, 186, 73, 174, 136, 109, 94, 249, 141, 208, 95, 101, 10, 132, 104,
            2, 125, 44, 40, 112, 221, 192, 42, 6, 117, 53, 116, 171, 241, 177, 250, 95, 14, 122, 194, 214, 227, 139,
            62, 37, 83, 51, 197, 47, 246, 119, 143, 74, 60, 5, 61, 125, 188, 17, 37, 231, 8, 194, 188, 165, 34, 64,
            121, 10, 185, 0, 0, 0, 0, 4, 106, 216, 115, 250, 26, 101, 114, 174, 93, 131, 161, 216, 29, 235, 104, 124,
            89, 181, 170, 170, 167, 79, 57, 101, 147, 51, 163, 133, 172, 119, 177, 113, 63, 238, 229, 36, 76, 24, 175,
            182, 18, 196, 132, 106, 139, 241, 176, 119, 89, 254, 219, 222, 201, 209, 57, 173, 233, 244, 200, 39, 54,
            225, 127, 232, 234, 131, 111, 33, 220, 180, 160, 41, 60, 10, 139, 189, 132, 129, 41, 100, 118, 160, 148,
            77, 66, 120, 59, 142, 51, 164, 52, 222, 183, 81, 252, 167, 37, 156, 76, 103, 135, 11, 250, 249, 236, 197,
            50, 139, 80, 115, 240, 121, 6, 36, 121, 92, 252, 32, 44, 37, 131, 129, 9, 147, 118, 229, 45, 8, 14, 0, 0,
            0, 4, 0, 0, 32, 223, 231, 40, 180, 245, 91, 36, 16, 251, 88, 2, 125, 168, 46, 197, 86, 210, 219, 178, 55,
            212, 45, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 163, 110, 213, 131, 54, 127, 5, 96, 244, 163, 248, 248, 144, 128,
            120, 223, 127, 115, 176, 207, 147, 254, 95, 89, 81, 247, 9, 30, 234, 255, 64, 84, 55, 136, 143, 96, 99,
            168, 13, 23, 12, 242, 56, 177, 0, 1, 16, 32, 93, 249, 168, 223, 236, 83, 108, 246, 76, 77, 125, 57, 163,
            53, 250, 79, 39, 146, 85, 234, 235, 109, 114, 174, 167, 157, 216, 202, 231, 215, 119, 30, 223, 220, 167,
            236, 168, 206, 232, 212, 185, 14, 58, 48, 102, 98, 30, 90, 154, 108, 230, 255, 22, 165, 133, 156, 114, 150,
            22, 111, 53, 89, 98, 182, 67, 136, 143, 96, 140, 254, 2, 24, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 66, 3,
            182, 102, 10, 44, 250, 190, 109, 109, 112, 71, 117, 99, 194, 50, 193, 246, 16, 57, 171, 200, 75, 67, 175,
            236, 42, 74, 18, 134, 187, 73, 67, 51, 148, 87, 129, 46, 177, 221, 141, 113, 16, 0, 0, 0, 240, 159, 148,
            165, 7, 47, 72, 117, 111, 66, 105, 47, 8, 3, 5, 50, 0, 173, 177, 7, 0, 0, 0, 0, 0, 5, 69, 242, 241, 43, 0,
            0, 0, 0, 25, 118, 169, 20, 149, 192, 223, 26, 68, 124, 56, 56, 225, 34, 226, 90, 36, 210, 48, 162, 46, 253,
            110, 214, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 170, 33, 169, 237, 41, 141, 133, 122, 133, 163,
            173, 150, 239, 175, 90, 40, 86, 29, 2, 208, 9, 23, 134, 180, 222, 121, 240, 157, 201, 2, 107, 178, 104, 11,
            232, 167, 0, 0, 0, 0, 0, 0, 0, 0, 54, 106, 52, 72, 97, 116, 104, 116, 102, 91, 254, 112, 217, 212, 63, 243,
            58, 238, 102, 24, 10, 70, 89, 220, 230, 120, 90, 120, 96, 181, 3, 246, 63, 201, 117, 156, 221, 134, 80,
            166, 226, 62, 57, 140, 227, 67, 53, 184, 167, 97, 185, 20, 212, 9, 55, 0, 0, 0, 0, 0, 0, 0, 0, 44, 106, 76,
            41, 82, 83, 75, 66, 76, 79, 67, 75, 58, 83, 20, 104, 13, 196, 197, 252, 160, 124, 201, 151, 123, 175, 108,
            232, 68, 155, 183, 63, 240, 234, 116, 132, 223, 150, 177, 223, 32, 0, 50, 160, 211, 0, 0, 0, 0, 0, 0, 0, 0,
            38, 106, 36, 185, 225, 27, 109, 220, 11, 216, 104, 88, 60, 24, 59, 15, 120, 81, 38, 113, 181, 195, 58, 100,
            178, 49, 33, 22, 229, 130, 0, 92, 212, 254, 203, 200, 41, 19, 101, 189, 195, 164, 41, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 10, 103, 78, 60, 243, 201, 154,
            100, 47, 201, 240, 6, 50, 180, 45, 226, 132, 81, 62, 232, 210, 78, 46, 233, 80, 194, 66, 57, 211, 164, 242,
            10, 194, 8, 206, 226, 101, 164, 212, 15, 81, 160, 154, 137, 213, 62, 214, 37, 38, 86, 167, 183, 234, 52,
            46, 213, 24, 58, 89, 122, 77, 192, 60, 228, 16, 132, 136, 210, 209, 107, 5, 246, 179, 205, 108, 107, 159,
            128, 139, 21, 6, 61, 212, 242, 114, 87, 33, 27, 206, 224, 32, 162, 205, 146, 186, 177, 83, 229, 27, 50, 64,
            117, 223, 49, 69, 20, 1, 119, 91, 250, 82, 242, 43, 86, 46, 116, 255, 67, 142, 246, 147, 102, 214, 229, 93,
            121, 156, 192, 59, 41, 83, 5, 5, 88, 68, 39, 42, 147, 131, 176, 68, 81, 120, 47, 145, 191, 156, 147, 38,
            158, 29, 28, 201, 249, 14, 246, 199, 181, 158, 245, 159, 163, 104, 173, 148, 224, 174, 200, 53, 155, 151,
            98, 164, 0, 124, 180, 45, 86, 4, 47, 79, 226, 67, 15, 199, 206, 180, 131, 15, 204, 35, 202, 14, 94, 226,
            116, 106, 185, 0, 118, 19, 25, 205, 43, 245, 145, 111, 217, 113, 18, 237, 72, 28, 0, 163, 21, 33, 12, 194,
            43, 92, 162, 194, 140, 57, 57, 221, 10, 130, 208, 237, 149, 156, 118, 165, 25, 197, 179, 205, 201, 33, 241,
            101, 80, 192, 65, 93, 186, 37, 190, 111, 215, 120, 60, 201, 191, 56, 59, 11, 160, 115, 80, 50, 200, 244,
            166, 246, 151, 162, 201, 141, 93, 153, 255, 255, 192, 82, 159, 77, 38, 233, 212, 217, 127, 72, 112, 1, 59,
            144, 202, 55, 203, 60, 222, 43, 223, 170, 73, 27, 217, 214, 105, 57, 27, 233, 164, 9, 184, 61, 115, 119,
            249, 171, 248, 12, 194, 41, 242, 175, 212, 106, 0, 0, 0, 0, 4, 64, 64, 36, 166, 194, 19, 206, 95, 87, 124,
            107, 130, 199, 226, 110, 177, 251, 58, 110, 167, 230, 226, 188, 5, 64, 7, 146, 252, 196, 198, 117, 96, 63,
            238, 229, 36, 76, 24, 175, 182, 18, 196, 132, 106, 139, 241, 176, 119, 89, 254, 219, 222, 201, 209, 57,
            173, 233, 244, 200, 39, 54, 225, 127, 232, 234, 131, 111, 33, 220, 180, 160, 41, 60, 10, 139, 189, 132,
            129, 41, 100, 118, 160, 148, 77, 66, 120, 59, 142, 51, 164, 52, 222, 183, 81, 252, 167, 132, 104, 137, 192,
            210, 86, 209, 118, 11, 28, 221, 57, 9, 196, 183, 174, 20, 41, 202, 246, 120, 12, 220, 104, 228, 106, 201,
            147, 135, 159, 56, 211, 14, 0, 0, 0, 0, 224, 255, 55, 223, 231, 40, 180, 245, 91, 36, 16, 251, 88, 2, 125,
            168, 46, 197, 86, 210, 219, 178, 55, 212, 45, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 119, 229, 213, 110, 27, 207, 7,
            50, 147, 55, 223, 172, 178, 140, 44, 115, 220, 98, 148, 114, 124, 199, 98, 18, 77, 251, 238, 46, 122, 201,
            123, 87, 47, 137, 143, 96, 99, 168, 13, 23, 76, 229, 73, 115, 0, 1, 16, 32, 69, 192, 87, 244, 203, 87, 170,
            65, 74, 124, 233, 109, 98, 185, 36, 106, 172, 97, 157, 153, 157, 70, 10, 73, 129, 193, 152, 83, 229, 7,
            219, 27, 190, 207, 17, 80, 160, 20, 9, 191, 106, 6, 137, 106, 48, 105, 200, 203, 191, 212, 71, 244, 60, 13,
            58, 209, 42, 96, 16, 195, 4, 143, 89, 165, 61, 137, 143, 96, 140, 254, 2, 24, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255,
            255, 66, 3, 182, 102, 10, 44, 250, 190, 109, 109, 141, 161, 100, 226, 131, 164, 95, 69, 48, 104, 203, 234,
            94, 190, 10, 230, 141, 49, 80, 104, 242, 68, 128, 246, 194, 78, 147, 204, 14, 74, 197, 160, 16, 0, 0, 0,
            240, 159, 148, 165, 7, 47, 72, 117, 111, 66, 105, 47, 8, 3, 1, 222, 0, 60, 242, 6, 0, 0, 0, 0, 0, 5, 102,
            22, 20, 44, 0, 0, 0, 0, 25, 118, 169, 20, 149, 192, 223, 26, 68, 124, 56, 56, 225, 34, 226, 90, 36, 210,
            48, 162, 46, 253, 110, 214, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 170, 33, 169, 237, 94, 185, 46,
            161, 216, 52, 209, 197, 25, 164, 115, 38, 150, 182, 214, 203, 236, 19, 65, 250, 248, 173, 89, 222, 15, 69,
            17, 3, 110, 42, 33, 18, 0, 0, 0, 0, 0, 0, 0, 0, 54, 106, 52, 72, 97, 116, 104, 255, 145, 58, 207, 175, 233,
            234, 150, 65, 177, 243, 29, 128, 167, 85, 216, 190, 56, 239, 220, 234, 37, 93, 245, 239, 1, 75, 106, 122,
            253, 23, 101, 119, 43, 247, 15, 236, 239, 72, 82, 149, 231, 119, 164, 84, 212, 220, 84, 0, 0, 0, 0, 0, 0,
            0, 0, 44, 106, 76, 41, 82, 83, 75, 66, 76, 79, 67, 75, 58, 218, 20, 132, 7, 238, 151, 100, 139, 193, 145,
            54, 98, 213, 148, 102, 197, 222, 121, 179, 236, 234, 116, 132, 223, 150, 177, 223, 33, 0, 50, 160, 212, 0,
            0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 185, 225, 27, 109, 249, 70, 90, 229, 2, 50, 67, 193, 171, 28, 78, 11,
            187, 9, 147, 126, 10, 249, 209, 235, 162, 177, 58, 29, 91, 11, 140, 97, 99, 157, 196, 222, 238, 34, 88, 37,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 10, 103,
            78, 60, 243, 201, 154, 100, 47, 201, 240, 6, 50, 180, 45, 226, 132, 81, 62, 232, 210, 78, 46, 233, 80, 194,
            66, 57, 211, 164, 242, 10, 194, 13, 33, 95, 43, 222, 172, 115, 216, 190, 156, 30, 64, 178, 17, 76, 251,
            174, 235, 227, 76, 41, 135, 84, 75, 11, 78, 30, 70, 170, 60, 148, 200, 56, 222, 141, 191, 190, 66, 88, 26,
            171, 48, 171, 252, 152, 215, 24, 212, 114, 190, 180, 23, 20, 162, 128, 192, 170, 79, 16, 80, 73, 173, 78,
            92, 128, 70, 22, 13, 117, 11, 38, 44, 31, 183, 187, 46, 133, 228, 48, 15, 104, 123, 111, 6, 161, 221, 33,
            120, 87, 36, 193, 160, 24, 68, 39, 128, 135, 73, 231, 224, 183, 213, 211, 245, 126, 158, 179, 160, 137, 39,
            196, 153, 217, 174, 211, 57, 174, 193, 239, 86, 58, 102, 99, 220, 71, 87, 45, 0, 67, 159, 25, 89, 64, 233,
            226, 71, 254, 196, 109, 60, 18, 167, 54, 243, 231, 155, 185, 164, 41, 248, 250, 71, 207, 254, 33, 52, 62,
            72, 112, 200, 101, 7, 97, 133, 233, 217, 33, 92, 11, 9, 164, 56, 49, 88, 56, 136, 97, 249, 163, 69, 130,
            156, 68, 218, 155, 58, 92, 228, 231, 180, 221, 124, 196, 142, 106, 102, 126, 108, 151, 192, 91, 43, 154,
            205, 225, 245, 19, 209, 56, 251, 59, 105, 179, 116, 52, 114, 210, 222, 114, 25, 134, 63, 122, 212, 14, 71,
            37, 51, 76, 140, 219, 71, 231, 30, 170, 165, 63, 46, 166, 225, 165, 121, 142, 226, 131, 174, 96, 207, 129,
            8, 12, 169, 7, 101, 223, 129, 70, 16, 152, 220, 175, 31, 182, 44, 67, 189, 125, 99, 183, 21, 216, 99, 252,
            188, 234, 141, 52, 172, 130, 249, 75, 206, 144, 34, 3, 227, 33, 229, 0, 0, 0, 0, 4, 176, 87, 196, 182, 12,
            93, 107, 21, 7, 8, 154, 236, 162, 155, 15, 137, 86, 136, 53, 127, 76, 166, 14, 47, 173, 247, 36, 21, 209,
            6, 31, 198, 63, 238, 229, 36, 76, 24, 175, 182, 18, 196, 132, 106, 139, 241, 176, 119, 89, 254, 219, 222,
            201, 209, 57, 173, 233, 244, 200, 39, 54, 225, 127, 232, 234, 131, 111, 33, 220, 180, 160, 41, 60, 10, 139,
            189, 132, 129, 41, 100, 118, 160, 148, 77, 66, 120, 59, 142, 51, 164, 52, 222, 183, 81, 252, 167, 18, 222,
            217, 51, 231, 118, 62, 183, 162, 82, 163, 149, 66, 15, 2, 145, 239, 20, 183, 61, 132, 255, 228, 45, 187,
            81, 153, 226, 39, 229, 236, 69, 14, 0, 0, 0, 0, 224, 255, 47, 223, 231, 40, 180, 245, 91, 36, 16, 251, 88,
            2, 125, 168, 46, 197, 86, 210, 219, 178, 55, 212, 45, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 72, 28, 198, 234, 222,
            89, 77, 209, 231, 63, 223, 18, 203, 242, 129, 199, 178, 193, 91, 150, 121, 30, 173, 87, 21, 169, 24, 113,
            88, 217, 64, 152, 76, 137, 143, 96, 99, 168, 13, 23, 153, 98, 194, 176, 0, 1, 16, 32, 217, 119, 76, 179,
            216, 105, 154, 227, 186, 13, 171, 171, 227, 44, 138, 143, 86, 109, 120, 14, 22, 37, 235, 245, 130, 106,
            144, 152, 177, 220, 197, 133, 137, 83, 40, 250, 194, 136, 169, 185, 245, 17, 59, 216, 199, 91, 53, 14, 219,
            160, 45, 171, 211, 195, 156, 209, 93, 194, 40, 194, 155, 199, 225, 95, 80, 137, 143, 96, 140, 254, 2, 24,
            0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 255, 255, 255, 255, 100, 3, 183, 102, 10, 44, 250, 190, 109, 109, 118, 71, 36, 17, 219, 23,
            151, 64, 210, 139, 204, 234, 216, 136, 21, 193, 148, 201, 200, 103, 48, 178, 173, 29, 148, 33, 57, 210,
            192, 55, 251, 77, 16, 0, 0, 0, 240, 159, 144, 159, 8, 47, 70, 50, 80, 111, 111, 108, 47, 24, 77, 105, 110,
            101, 100, 32, 98, 121, 32, 122, 104, 117, 110, 100, 111, 110, 103, 97, 108, 108, 50, 48, 50, 48, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 5, 0, 230, 23, 0, 0, 0, 0, 0, 0, 4, 64, 190, 64, 37, 0, 0, 0, 0, 25, 118, 169, 20,
            200, 37, 161, 236, 242, 166, 131, 12, 68, 1, 98, 12, 58, 22, 241, 153, 80, 87, 194, 171, 136, 172, 0, 0, 0,
            0, 0, 0, 0, 0, 54, 106, 52, 72, 97, 116, 104, 233, 251, 142, 102, 24, 106, 208, 238, 26, 184, 252, 31, 231,
            124, 85, 200, 163, 65, 54, 19, 116, 123, 47, 27, 58, 149, 34, 180, 250, 244, 216, 65, 179, 11, 126, 60,
            186, 79, 70, 39, 138, 47, 181, 126, 41, 15, 142, 114, 0, 0, 0, 0, 0, 0, 0, 0, 44, 106, 76, 41, 82, 83, 75,
            66, 76, 79, 67, 75, 58, 231, 100, 163, 107, 103, 38, 84, 233, 236, 86, 77, 132, 19, 213, 63, 70, 197, 190,
            121, 30, 234, 116, 132, 223, 150, 177, 223, 33, 0, 50, 160, 213, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 185,
            225, 27, 109, 185, 179, 154, 147, 147, 162, 234, 217, 71, 153, 226, 14, 36, 0, 181, 158, 43, 86, 102, 145,
            204, 51, 153, 28, 250, 201, 1, 239, 141, 214, 212, 108, 88, 199, 66, 61, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 182, 46, 159, 77, 66, 128,
            222, 194, 173, 99, 181, 73, 8, 3, 28, 89, 68, 57, 151, 211, 41, 171, 133, 133, 175, 208, 140, 65, 130, 79,
            213, 200, 63, 238, 229, 36, 76, 24, 175, 182, 18, 196, 132, 106, 139, 241, 176, 119, 89, 254, 219, 222,
            201, 209, 57, 173, 233, 244, 200, 39, 54, 225, 127, 232, 234, 131, 111, 33, 220, 180, 160, 41, 60, 10, 139,
            189, 132, 129, 41, 100, 118, 160, 148, 77, 66, 120, 59, 142, 51, 164, 52, 222, 183, 81, 252, 167, 168, 239,
            195, 117, 130, 98, 8, 243, 150, 248, 110, 135, 127, 216, 249, 209, 230, 82, 72, 142, 184, 15, 101, 122, 71,
            27, 209, 103, 73, 87, 71, 95, 14, 0, 0, 0, 4, 224, 255, 39, 215, 164, 225, 155, 0, 250, 211, 5, 90, 78,
            134, 176, 17, 170, 69, 202, 110, 242, 138, 25, 99, 188, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 194, 168, 158, 100,
            195, 13, 163, 132, 41, 110, 213, 69, 74, 132, 5, 135, 210, 13, 119, 10, 216, 170, 213, 38, 198, 245, 144,
            223, 1, 168, 16, 225, 100, 137, 143, 96, 99, 168, 13, 23, 15, 255, 129, 96, 0, 1, 16, 32, 22, 238, 179,
            172, 48, 164, 111, 194, 182, 254, 140, 28, 224, 103, 229, 235, 116, 202, 189, 51, 5, 106, 232, 184, 135,
            188, 64, 137, 192, 86, 189, 162, 227, 47, 16, 86, 2, 255, 143, 59, 57, 204, 255, 45, 36, 255, 41, 233, 1,
            74, 56, 50, 57, 53, 143, 224, 85, 66, 36, 219, 87, 25, 221, 150, 101, 137, 143, 96, 140, 254, 2, 24, 0, 0,
            0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 255, 255, 255, 255, 100, 3, 183, 102, 10, 44, 250, 190, 109, 109, 12, 52, 171, 10, 1, 98, 137, 34,
            241, 56, 206, 137, 178, 140, 35, 143, 148, 253, 247, 33, 160, 63, 183, 120, 12, 172, 90, 104, 197, 251,
            224, 107, 16, 0, 0, 0, 240, 159, 144, 159, 8, 47, 70, 50, 80, 111, 111, 108, 47, 15, 77, 105, 110, 101,
            100, 32, 98, 121, 32, 103, 121, 108, 111, 110, 103, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 5, 0, 69, 106, 0, 0, 0, 0, 0, 0, 5, 39, 45, 29, 38, 0, 0, 0, 0, 25, 118, 169, 20, 200, 37, 161, 236,
            242, 166, 131, 12, 68, 1, 98, 12, 58, 22, 241, 153, 80, 87, 194, 171, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38,
            106, 36, 170, 33, 169, 237, 249, 93, 170, 17, 205, 215, 143, 24, 30, 245, 41, 95, 244, 241, 11, 126, 133,
            231, 32, 165, 14, 30, 208, 214, 70, 55, 106, 8, 99, 160, 174, 47, 0, 0, 0, 0, 0, 0, 0, 0, 54, 106, 52, 72,
            97, 116, 104, 74, 221, 217, 204, 182, 202, 178, 106, 213, 252, 178, 105, 114, 178, 171, 69, 184, 136, 250,
            62, 168, 118, 169, 82, 108, 91, 226, 154, 193, 125, 99, 229, 188, 100, 132, 89, 115, 11, 70, 211, 147, 138,
            192, 10, 154, 161, 68, 43, 0, 0, 0, 0, 0, 0, 0, 0, 44, 106, 76, 41, 82, 83, 75, 66, 76, 79, 67, 75, 58,
            183, 208, 39, 60, 76, 213, 164, 115, 29, 158, 80, 1, 61, 155, 62, 184, 170, 249, 177, 112, 234, 116, 132,
            223, 150, 177, 223, 32, 0, 50, 160, 215, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 185, 225, 27, 109, 7, 42, 94,
            92, 220, 202, 240, 35, 197, 89, 78, 217, 74, 213, 183, 202, 37, 57, 249, 25, 209, 118, 20, 9, 235, 93, 26,
            117, 62, 123, 192, 202, 75, 165, 24, 59, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 225, 141, 168, 204, 236, 161, 233, 179, 71, 160, 90, 108, 155, 52, 32,
            211, 111, 91, 69, 105, 60, 194, 98, 69, 155, 148, 18, 152, 239, 207, 28, 140, 152, 133, 118, 0, 111, 203,
            118, 89, 77, 96, 55, 165, 240, 54, 112, 16, 91, 188, 134, 31, 130, 214, 157, 206, 178, 242, 6, 194, 24, 81,
            26, 46, 21, 45, 90, 116, 128, 199, 157, 201, 121, 34, 219, 106, 169, 109, 14, 21, 154, 24, 233, 238, 185,
            74, 18, 201, 59, 99, 228, 196, 2, 15, 227, 83, 45, 18, 51, 200, 227, 90, 40, 173, 142, 169, 111, 212, 173,
            8, 26, 21, 168, 11, 24, 185, 19, 189, 2, 125, 252, 192, 31, 225, 126, 20, 203, 133, 207, 147, 88, 37, 165,
            180, 90, 31, 64, 84, 73, 197, 54, 168, 190, 189, 146, 218, 127, 129, 218, 108, 211, 139, 147, 13, 96, 51,
            177, 202, 198, 155, 84, 230, 62, 65, 24, 73, 175, 230, 55, 158, 67, 159, 176, 208, 3, 144, 231, 156, 36,
            30, 22, 40, 192, 62, 105, 219, 134, 203, 217, 188, 144, 253, 47, 227, 119, 90, 174, 74, 202, 86, 232, 131,
            64, 33, 71, 157, 236, 43, 24, 239, 109, 197, 133, 76, 113, 80, 229, 154, 24, 118, 100, 152, 20, 130, 96,
            192, 239, 186, 65, 20, 160, 212, 139, 139, 88, 78, 100, 78, 37, 141, 132, 130, 229, 211, 114, 135, 110, 71,
            55, 199, 46, 208, 250, 195, 12, 106, 35, 19, 93, 225, 95, 76, 19, 7, 127, 202, 173, 112, 25, 38, 164, 83,
            189, 166, 123, 236, 224, 99, 176, 200, 78, 174, 105, 85, 116, 186, 181, 242, 0, 0, 0, 0, 4, 130, 47, 41,
            241, 86, 189, 232, 181, 37, 24, 72, 82, 0, 114, 254, 179, 152, 111, 246, 108, 10, 41, 65, 188, 173, 43, 20,
            86, 240, 204, 213, 239, 63, 238, 229, 36, 76, 24, 175, 182, 18, 196, 132, 106, 139, 241, 176, 119, 89, 254,
            219, 222, 201, 209, 57, 173, 233, 244, 200, 39, 54, 225, 127, 232, 234, 131, 111, 33, 220, 180, 160, 41,
            60, 10, 139, 189, 132, 129, 41, 100, 118, 160, 148, 77, 66, 120, 59, 142, 51, 164, 52, 222, 183, 81, 252,
            167, 66, 223, 47, 64, 6, 159, 1, 84, 244, 205, 208, 203, 112, 9, 30, 124, 99, 253, 97, 122, 102, 139, 31,
            10, 207, 198, 174, 174, 173, 125, 248, 138, 14, 0, 0, 0, 4, 0, 128, 32, 215, 164, 225, 155, 0, 250, 211, 5,
            90, 78, 134, 176, 17, 170, 69, 202, 110, 242, 138, 25, 99, 188, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 46, 79, 150,
            13, 65, 110, 152, 95, 122, 216, 161, 22, 112, 55, 80, 234, 186, 142, 106, 192, 21, 71, 81, 90, 65, 164,
            114, 242, 13, 248, 78, 29, 158, 137, 143, 96, 99, 168, 13, 23, 25, 8, 110, 239, 0, 1, 16, 32, 191, 191, 80,
            204, 56, 119, 127, 64, 19, 33, 27, 100, 181, 12, 202, 149, 72, 254, 180, 206, 240, 73, 203, 254, 40, 58,
            182, 223, 221, 168, 238, 93, 122, 98, 235, 94, 10, 183, 146, 198, 121, 199, 59, 30, 234, 237, 197, 109, 86,
            110, 136, 163, 193, 101, 223, 25, 146, 224, 55, 142, 36, 48, 124, 217, 172, 137, 143, 96, 140, 254, 2, 24,
            0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 255, 255, 255, 255, 100, 3, 183, 102, 10, 44, 250, 190, 109, 109, 155, 5, 178, 198, 96, 154,
            127, 26, 37, 211, 13, 220, 105, 0, 27, 66, 150, 161, 187, 121, 167, 25, 3, 177, 65, 245, 142, 122, 60, 15,
            101, 19, 16, 0, 0, 0, 240, 159, 144, 159, 8, 47, 70, 50, 80, 111, 111, 108, 47, 17, 77, 105, 110, 101, 100,
            32, 98, 121, 32, 115, 117, 99, 111, 114, 105, 50, 48, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            5, 0, 172, 105, 9, 0, 0, 0, 0, 0, 5, 82, 48, 70, 38, 0, 0, 0, 0, 25, 118, 169, 20, 200, 37, 161, 236, 242,
            166, 131, 12, 68, 1, 98, 12, 58, 22, 241, 153, 80, 87, 194, 171, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106,
            36, 170, 33, 169, 237, 202, 25, 61, 23, 110, 95, 170, 9, 150, 134, 150, 13, 249, 203, 122, 47, 109, 177,
            85, 237, 240, 237, 162, 157, 131, 95, 194, 23, 239, 26, 153, 47, 0, 0, 0, 0, 0, 0, 0, 0, 54, 106, 52, 72,
            97, 116, 104, 191, 18, 109, 164, 106, 40, 134, 225, 111, 229, 64, 20, 13, 176, 233, 116, 150, 190, 62, 154,
            95, 213, 41, 160, 157, 56, 208, 51, 154, 32, 212, 118, 17, 56, 28, 75, 33, 216, 67, 168, 137, 91, 18, 23,
            170, 253, 151, 249, 0, 0, 0, 0, 0, 0, 0, 0, 44, 106, 76, 41, 82, 83, 75, 66, 76, 79, 67, 75, 58, 119, 209,
            144, 52, 107, 102, 77, 237, 1, 62, 98, 51, 39, 98, 19, 140, 55, 179, 147, 85, 234, 116, 132, 223, 150, 177,
            223, 34, 0, 50, 160, 217, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 185, 225, 27, 109, 196, 127, 90, 215, 100,
            245, 110, 209, 165, 207, 239, 134, 191, 154, 118, 178, 10, 61, 212, 203, 173, 231, 217, 99, 231, 85, 16,
            215, 77, 250, 22, 220, 68, 249, 77, 63, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 10, 225, 141, 168, 204, 236, 161, 233, 179, 71, 160, 90, 108, 155, 52, 32,
            211, 111, 91, 69, 105, 60, 194, 98, 69, 155, 148, 18, 152, 239, 207, 28, 140, 152, 133, 118, 0, 111, 203,
            118, 89, 77, 96, 55, 165, 240, 54, 112, 16, 91, 188, 134, 31, 130, 214, 157, 206, 178, 242, 6, 194, 24, 81,
            26, 46, 21, 45, 90, 116, 128, 199, 157, 201, 121, 34, 219, 106, 169, 109, 14, 21, 154, 24, 233, 238, 185,
            74, 18, 201, 59, 99, 228, 196, 2, 15, 227, 83, 22, 223, 22, 240, 158, 79, 2, 67, 254, 93, 235, 198, 160,
            118, 193, 169, 164, 0, 182, 213, 76, 199, 44, 18, 157, 203, 134, 141, 85, 103, 65, 118, 254, 92, 15, 202,
            18, 92, 246, 249, 147, 113, 90, 43, 48, 231, 169, 237, 16, 242, 78, 46, 149, 62, 190, 225, 175, 71, 230,
            87, 15, 12, 9, 16, 175, 213, 125, 12, 224, 1, 67, 130, 176, 192, 34, 71, 209, 187, 214, 247, 149, 193, 138,
            156, 52, 113, 62, 16, 100, 222, 249, 97, 150, 134, 218, 21, 174, 110, 92, 223, 214, 53, 46, 13, 246, 173,
            59, 19, 77, 97, 177, 133, 218, 200, 90, 23, 141, 4, 109, 138, 17, 17, 76, 144, 189, 232, 130, 60, 180, 36,
            1, 84, 190, 158, 240, 197, 28, 115, 18, 91, 25, 5, 70, 224, 185, 180, 210, 14, 134, 246, 195, 37, 191, 242,
            155, 66, 181, 240, 207, 27, 245, 249, 173, 185, 108, 214, 17, 224, 136, 47, 30, 58, 38, 206, 73, 164, 171,
            155, 254, 168, 152, 88, 13, 140, 127, 214, 235, 33, 37, 152, 175, 185, 49, 224, 0, 58, 13, 234, 197, 59,
            43, 96, 100, 112, 200, 159, 3, 68, 49, 10, 248, 40, 96, 156, 211, 129, 37, 69, 117, 55, 115, 250, 150, 162,
            0, 0, 0, 0, 4, 55, 11, 113, 113, 126, 174, 146, 133, 232, 58, 242, 37, 32, 184, 31, 237, 30, 191, 239, 240,
            79, 138, 83, 227, 54, 90, 14, 107, 90, 33, 122, 215, 63, 238, 229, 36, 76, 24, 175, 182, 18, 196, 132, 106,
            139, 241, 176, 119, 89, 254, 219, 222, 201, 209, 57, 173, 233, 244, 200, 39, 54, 225, 127, 232, 196, 252,
            94, 32, 5, 152, 181, 129, 183, 59, 97, 205, 43, 112, 25, 76, 163, 110, 64, 189, 81, 212, 130, 30, 252, 67,
            173, 81, 184, 184, 194, 122, 160, 235, 246, 113, 146, 131, 148, 128, 234, 150, 56, 233, 157, 168, 34, 220,
            25, 115, 65, 38, 4, 63, 160, 37, 28, 215, 238, 16, 34, 244, 36, 148, 14, 0, 0, 0, 4, 32, 0, 32, 215, 164,
            225, 155, 0, 250, 211, 5, 90, 78, 134, 176, 17, 170, 69, 202, 110, 242, 138, 25, 99, 188, 3, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 110, 84, 27, 228, 118, 231, 40, 218, 240, 96, 237, 39, 10, 99, 172, 11, 16, 204, 65, 200, 39,
            178, 203, 160, 86, 162, 227, 210, 5, 53, 217, 87, 207, 137, 143, 96, 99, 168, 13, 23, 109, 202, 114, 226,
            0, 1, 16, 32, 35, 86, 25, 32, 104, 15, 229, 41, 79, 64, 80, 76, 16, 108, 165, 152, 133, 154, 209, 213, 89,
            243, 119, 189, 55, 139, 7, 202, 171, 95, 111, 155, 241, 5, 36, 221, 92, 170, 140, 213, 249, 46, 199, 81,
            249, 197, 175, 244, 205, 255, 188, 125, 187, 23, 87, 87, 204, 222, 198, 56, 105, 254, 45, 200, 209, 137,
            143, 96, 140, 254, 2, 24, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 66, 3, 183, 102, 10, 44, 250, 190, 109, 109,
            220, 230, 49, 127, 222, 165, 77, 101, 37, 226, 126, 160, 198, 32, 113, 59, 8, 98, 211, 143, 6, 78, 97, 202,
            103, 207, 56, 189, 129, 78, 139, 60, 16, 0, 0, 0, 240, 159, 148, 165, 7, 47, 72, 117, 111, 66, 105, 47, 8,
            2, 9, 247, 0, 154, 63, 0, 0, 0, 0, 0, 0, 5, 32, 150, 72, 38, 0, 0, 0, 0, 25, 118, 169, 20, 149, 192, 223,
            26, 68, 124, 56, 56, 225, 34, 226, 90, 36, 210, 48, 162, 46, 253, 110, 214, 136, 172, 0, 0, 0, 0, 0, 0, 0,
            0, 38, 106, 36, 170, 33, 169, 237, 186, 79, 101, 161, 125, 116, 165, 185, 127, 28, 146, 45, 249, 29, 221,
            203, 130, 38, 233, 109, 94, 191, 122, 90, 168, 139, 111, 95, 6, 212, 85, 126, 0, 0, 0, 0, 0, 0, 0, 0, 54,
            106, 52, 72, 97, 116, 104, 71, 111, 135, 97, 230, 85, 132, 189, 195, 117, 134, 106, 237, 109, 212, 78, 181,
            140, 248, 61, 35, 71, 62, 248, 147, 215, 1, 172, 21, 219, 83, 66, 216, 215, 151, 143, 88, 205, 75, 88, 135,
            241, 183, 188, 55, 19, 96, 70, 0, 0, 0, 0, 0, 0, 0, 0, 44, 106, 76, 41, 82, 83, 75, 66, 76, 79, 67, 75, 58,
            123, 210, 45, 97, 74, 57, 98, 242, 90, 248, 239, 59, 140, 210, 109, 115, 68, 76, 14, 4, 234, 116, 132, 223,
            150, 177, 223, 35, 0, 50, 160, 218, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 185, 225, 27, 109, 141, 126, 171,
            162, 71, 1, 71, 98, 13, 55, 56, 81, 92, 68, 168, 221, 231, 90, 158, 240, 16, 38, 36, 236, 190, 6, 142, 83,
            162, 201, 36, 144, 109, 135, 200, 43, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 10, 225, 141, 168, 204, 236, 161, 233, 179, 71, 160, 90, 108, 155, 52, 32, 211,
            111, 91, 69, 105, 60, 194, 98, 69, 155, 148, 18, 152, 239, 207, 28, 140, 152, 133, 118, 0, 111, 203, 118,
            89, 77, 96, 55, 165, 240, 54, 112, 16, 91, 188, 134, 31, 130, 214, 157, 206, 178, 242, 6, 194, 24, 81, 26,
            46, 21, 45, 90, 116, 128, 199, 157, 201, 121, 34, 219, 106, 169, 109, 14, 21, 154, 24, 233, 238, 185, 74,
            18, 201, 59, 99, 228, 196, 2, 15, 227, 83, 22, 223, 22, 240, 158, 79, 2, 67, 254, 93, 235, 198, 160, 118,
            193, 169, 164, 0, 182, 213, 76, 199, 44, 18, 157, 203, 134, 141, 85, 103, 65, 118, 36, 0, 38, 31, 53, 174,
            202, 161, 4, 66, 44, 231, 18, 183, 30, 99, 251, 129, 91, 41, 241, 147, 110, 143, 164, 182, 177, 44, 76, 40,
            81, 123, 163, 19, 183, 122, 53, 101, 74, 104, 183, 8, 160, 216, 145, 232, 215, 130, 222, 219, 52, 34, 60,
            21, 81, 183, 177, 53, 213, 226, 197, 209, 242, 16, 208, 120, 125, 238, 48, 242, 46, 107, 63, 120, 122, 87,
            106, 77, 102, 220, 130, 23, 68, 141, 245, 75, 22, 94, 92, 220, 131, 179, 82, 61, 144, 210, 94, 228, 254,
            190, 57, 54, 67, 147, 75, 114, 112, 83, 1, 11, 140, 225, 149, 118, 26, 37, 201, 157, 217, 242, 113, 83, 86,
            141, 171, 122, 104, 132, 157, 114, 4, 139, 88, 8, 95, 90, 210, 194, 78, 69, 78, 39, 181, 10, 245, 44, 34,
            154, 97, 87, 134, 147, 173, 158, 206, 16, 226, 238, 35, 141, 81, 196, 42, 123, 126, 103, 4, 211, 144, 15,
            172, 239, 172, 131, 45, 196, 166, 193, 174, 101, 3, 14, 210, 109, 120, 188, 134, 22, 195, 95, 168, 39, 0,
            0, 0, 0, 4, 215, 30, 145, 74, 172, 125, 3, 37, 143, 10, 207, 236, 148, 203, 139, 164, 104, 205, 162, 178,
            131, 13, 129, 52, 49, 206, 159, 4, 208, 112, 114, 86, 63, 238, 229, 36, 76, 24, 175, 182, 18, 196, 132,
            106, 139, 241, 176, 119, 89, 254, 219, 222, 201, 209, 57, 173, 233, 244, 200, 39, 54, 225, 127, 232, 196,
            252, 94, 32, 5, 152, 181, 129, 183, 59, 97, 205, 43, 112, 25, 76, 163, 110, 64, 189, 81, 212, 130, 30, 252,
            67, 173, 81, 184, 184, 194, 122, 238, 46, 211, 98, 146, 48, 39, 92, 228, 130, 230, 176, 71, 133, 176, 186,
            215, 231, 245, 151, 69, 22, 115, 255, 44, 192, 11, 94, 179, 169, 85, 216, 14, 0, 0, 0, 0, 224, 255, 63,
            215, 164, 225, 155, 0, 250, 211, 5, 90, 78, 134, 176, 17, 170, 69, 202, 110, 242, 138, 25, 99, 188, 3, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 130, 122, 103, 219, 237, 134, 133, 142, 112, 146, 7, 248, 48, 31, 5, 178, 239, 100,
            103, 239, 74, 14, 120, 46, 218, 97, 222, 11, 143, 176, 88, 78, 225, 137, 143, 96, 99, 168, 13, 23, 41, 175,
            221, 148, 0, 1, 16, 32, 20, 80, 46, 93, 163, 135, 156, 44, 184, 248, 252, 91, 121, 183, 161, 99, 107, 124,
            135, 145, 182, 132, 72, 231, 45, 80, 204, 160, 220, 171, 217, 175, 41, 243, 47, 114, 203, 201, 218, 88,
            227, 223, 106, 13, 143, 40, 43, 245, 180, 28, 124, 223, 218, 106, 251, 155, 4, 5, 84, 95, 242, 103, 246,
            162, 234, 137, 143, 96, 140, 254, 2, 24, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 100, 3, 184, 102, 10, 44, 250,
            190, 109, 109, 121, 101, 175, 177, 239, 255, 210, 195, 30, 118, 159, 35, 123, 251, 42, 207, 112, 110, 123,
            37, 88, 81, 211, 70, 114, 92, 87, 148, 104, 0, 174, 246, 16, 0, 0, 0, 240, 159, 144, 159, 8, 47, 70, 50,
            80, 111, 111, 108, 47, 17, 77, 105, 110, 101, 100, 32, 98, 121, 32, 115, 117, 99, 111, 114, 105, 50, 48, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 0, 75, 46, 0, 0, 0, 0, 0, 0, 5, 65, 120, 163, 37, 0,
            0, 0, 0, 25, 118, 169, 20, 200, 37, 161, 236, 242, 166, 131, 12, 68, 1, 98, 12, 58, 22, 241, 153, 80, 87,
            194, 171, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 170, 33, 169, 237, 177, 21, 243, 77, 198, 221,
            120, 175, 163, 12, 81, 79, 18, 245, 76, 24, 210, 223, 183, 187, 200, 200, 142, 226, 139, 132, 138, 108, 80,
            223, 142, 43, 0, 0, 0, 0, 0, 0, 0, 0, 54, 106, 52, 72, 97, 116, 104, 81, 210, 33, 124, 181, 0, 168, 158,
            34, 127, 181, 253, 108, 113, 63, 230, 75, 29, 108, 116, 186, 114, 239, 106, 33, 141, 235, 247, 211, 50,
            127, 127, 17, 16, 88, 142, 143, 50, 68, 221, 144, 252, 126, 76, 120, 21, 228, 195, 0, 0, 0, 0, 0, 0, 0, 0,
            44, 106, 76, 41, 82, 83, 75, 66, 76, 79, 67, 75, 58, 181, 96, 38, 205, 89, 213, 227, 104, 157, 208, 66, 17,
            223, 140, 175, 85, 138, 245, 239, 128, 234, 116, 132, 223, 150, 177, 223, 33, 0, 50, 160, 219, 0, 0, 0, 0,
            0, 0, 0, 0, 38, 106, 36, 185, 225, 27, 109, 17, 13, 43, 49, 117, 86, 104, 1, 2, 101, 43, 141, 133, 61, 148,
            217, 69, 23, 29, 64, 105, 16, 182, 207, 58, 132, 240, 86, 130, 225, 212, 24, 93, 142, 235, 61, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 180, 222, 3, 212,
            164, 207, 81, 42, 129, 115, 56, 219, 61, 55, 51, 93, 88, 14, 124, 65, 147, 159, 63, 204, 142, 233, 217,
            135, 51, 159, 127, 17, 186, 145, 112, 247, 240, 56, 81, 251, 48, 37, 145, 217, 102, 187, 138, 199, 2, 111,
            32, 249, 129, 28, 240, 82, 231, 125, 64, 245, 158, 179, 116, 46, 37, 201, 184, 121, 249, 235, 216, 96, 243,
            108, 32, 218, 113, 181, 195, 55, 251, 186, 25, 221, 190, 164, 201, 1, 79, 211, 182, 108, 247, 87, 158, 201,
            148, 30, 215, 222, 226, 14, 60, 222, 6, 3, 236, 230, 44, 255, 185, 243, 227, 245, 37, 193, 252, 119, 13,
            59, 233, 16, 160, 51, 28, 244, 28, 157, 96, 86, 125, 36, 193, 229, 98, 42, 105, 12, 131, 145, 173, 86, 131,
            118, 111, 170, 204, 244, 69, 188, 176, 81, 132, 97, 104, 218, 250, 186, 113, 242, 225, 50, 46, 120, 249,
            59, 93, 248, 49, 18, 99, 189, 168, 131, 127, 138, 59, 208, 30, 29, 160, 22, 18, 20, 169, 192, 217, 245, 72,
            248, 23, 33, 0, 0, 0, 0, 4, 215, 30, 145, 74, 172, 125, 3, 37, 143, 10, 207, 236, 148, 203, 139, 164, 104,
            205, 162, 178, 131, 13, 129, 52, 49, 206, 159, 4, 208, 112, 114, 86, 63, 238, 229, 36, 76, 24, 175, 182,
            18, 196, 132, 106, 139, 241, 176, 119, 89, 254, 219, 222, 201, 209, 57, 173, 233, 244, 200, 39, 54, 225,
            127, 232, 19, 211, 64, 82, 2, 171, 68, 162, 110, 58, 122, 152, 98, 106, 214, 166, 198, 112, 101, 233, 75,
            90, 31, 6, 119, 245, 55, 206, 55, 106, 113, 189, 141, 210, 25, 17, 220, 218, 61, 252, 242, 248, 76, 211,
            54, 154, 37, 100, 119, 68, 173, 240, 96, 255, 25, 128, 241, 107, 212, 216, 252, 173, 174, 81, 14, 0, 0, 0,
            4, 224, 255, 39, 218, 240, 36, 68, 3, 189, 102, 83, 191, 140, 42, 249, 90, 164, 43, 213, 139, 25, 109, 215,
            210, 120, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 207, 169, 102, 6, 255, 15, 6, 64, 142, 187, 145, 175, 94, 204, 93,
            172, 210, 81, 49, 181, 159, 130, 5, 136, 162, 119, 187, 38, 177, 160, 112, 49, 249, 137, 143, 96, 99, 168,
            13, 23, 104, 220, 27, 102, 0, 1, 16, 32, 0, 20, 121, 236, 189, 227, 245, 30, 96, 43, 254, 94, 41, 26, 244,
            191, 144, 120, 56, 140, 157, 148, 17, 251, 71, 209, 135, 206, 15, 61, 205, 209, 223, 21, 115, 6, 68, 151,
            104, 225, 95, 167, 0, 22, 15, 118, 19, 64, 253, 180, 17, 9, 243, 165, 124, 3, 93, 118, 207, 14, 48, 250,
            77, 65, 250, 137, 143, 96, 140, 254, 2, 24, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 100, 3, 184, 102, 10, 44,
            250, 190, 109, 109, 51, 66, 89, 115, 28, 42, 107, 255, 107, 149, 243, 51, 70, 177, 244, 160, 160, 75, 31,
            229, 202, 89, 244, 254, 44, 208, 148, 26, 11, 253, 104, 5, 16, 0, 0, 0, 240, 159, 144, 159, 8, 47, 70, 50,
            80, 111, 111, 108, 47, 12, 77, 105, 110, 101, 100, 32, 98, 121, 32, 107, 122, 51, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 0, 0, 84, 103, 30, 0, 4, 144, 36, 181, 37, 0, 0,
            0, 0, 25, 118, 169, 20, 200, 37, 161, 236, 242, 166, 131, 12, 68, 1, 98, 12, 58, 22, 241, 153, 80, 87, 194,
            171, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 170, 33, 169, 237, 62, 22, 120, 75, 244, 47, 58, 95,
            51, 173, 40, 169, 104, 241, 192, 33, 1, 39, 103, 61, 109, 244, 210, 37, 19, 251, 22, 181, 19, 61, 234, 146,
            0, 0, 0, 0, 0, 0, 0, 0, 54, 106, 52, 72, 97, 116, 104, 217, 224, 134, 64, 233, 111, 156, 59, 54, 220, 43,
            45, 15, 155, 215, 114, 140, 166, 236, 19, 187, 188, 62, 209, 182, 135, 42, 182, 248, 243, 46, 158, 91, 200,
            21, 183, 197, 233, 71, 176, 172, 164, 71, 135, 181, 136, 212, 79, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 185,
            225, 27, 109, 99, 83, 120, 97, 140, 225, 208, 33, 118, 75, 52, 18, 209, 86, 144, 252, 19, 174, 68, 193,
            145, 169, 199, 192, 161, 170, 89, 0, 213, 70, 175, 136, 36, 212, 130, 73, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 8, 10, 82, 97, 20, 29, 249, 134, 47, 42,
            127, 42, 119, 184, 40, 77, 139, 7, 8, 17, 57, 192, 81, 28, 245, 83, 107, 169, 60, 231, 184, 91, 133, 212,
            70, 61, 144, 149, 192, 140, 123, 218, 155, 227, 137, 173, 218, 63, 33, 90, 84, 255, 196, 159, 24, 30, 222,
            162, 199, 177, 135, 208, 138, 79, 174, 195, 127, 87, 179, 49, 241, 4, 6, 148, 23, 89, 101, 4, 234, 49, 206,
            29, 40, 5, 193, 77, 39, 200, 38, 47, 191, 40, 80, 64, 130, 30, 169, 196, 218, 75, 129, 26, 15, 247, 228,
            134, 73, 62, 80, 241, 99, 17, 78, 237, 114, 23, 36, 31, 109, 30, 232, 224, 139, 167, 136, 187, 60, 181, 31,
            216, 137, 141, 20, 212, 180, 86, 175, 40, 105, 134, 155, 46, 149, 23, 204, 138, 115, 44, 128, 44, 193, 37,
            42, 135, 127, 178, 59, 13, 247, 148, 211, 25, 203, 253, 237, 207, 242, 3, 95, 22, 141, 76, 56, 162, 22, 33,
            205, 20, 97, 209, 248, 41, 114, 241, 28, 67, 114, 94, 164, 175, 27, 186, 46, 79, 217, 29, 189, 38, 61, 89,
            98, 40, 215, 245, 185, 97, 229, 29, 80, 218, 69, 92, 89, 45, 213, 127, 130, 213, 41, 210, 89, 229, 160, 68,
            180, 215, 191, 35, 164, 16, 52, 0, 40, 167, 9, 202, 190, 11, 37, 149, 138, 152, 17, 136, 235, 45, 134, 98,
            166, 122, 82, 179, 27, 246, 237, 14, 89, 0, 0, 0, 0, 4, 152, 149, 22, 138, 131, 217, 136, 221, 210, 94,
            206, 76, 131, 188, 41, 211, 183, 167, 103, 196, 63, 64, 7, 35, 100, 214, 178, 141, 6, 162, 183, 211, 63,
            238, 229, 36, 76, 24, 175, 182, 18, 196, 132, 106, 139, 241, 176, 119, 89, 254, 219, 222, 201, 209, 57,
            173, 233, 244, 200, 39, 54, 225, 127, 232, 19, 211, 64, 82, 2, 171, 68, 162, 110, 58, 122, 152, 98, 106,
            214, 166, 198, 112, 101, 233, 75, 90, 31, 6, 119, 245, 55, 206, 55, 106, 113, 189, 197, 53, 1, 15, 77, 19,
            136, 144, 129, 20, 61, 194, 63, 104, 20, 117, 110, 238, 45, 242, 22, 93, 103, 215, 249, 140, 215, 242, 229,
            138, 95, 181, 14, 0, 0, 0, 4, 0, 0, 32, 218, 240, 36, 68, 3, 189, 102, 83, 191, 140, 42, 249, 90, 164, 43,
            213, 139, 25, 109, 215, 210, 120, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 80, 166, 140, 55, 117, 180, 76, 2, 2, 152,
            116, 56, 104, 17, 218, 185, 162, 208, 39, 55, 202, 118, 11, 37, 63, 206, 18, 70, 124, 204, 234, 180, 26,
            138, 143, 96, 99, 168, 13, 23, 27, 117, 228, 171, 0, 1, 16, 32, 133, 167, 226, 120, 108, 89, 76, 22, 190,
            35, 243, 3, 65, 16, 105, 143, 106, 93, 138, 230, 214, 254, 246, 234, 138, 91, 140, 156, 173, 44, 135, 164,
            155, 52, 30, 137, 55, 40, 186, 253, 209, 14, 7, 146, 88, 91, 40, 21, 32, 120, 110, 226, 202, 55, 131, 13,
            181, 55, 25, 184, 238, 107, 249, 188, 59, 138, 143, 96, 140, 254, 2, 24, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255,
            255, 100, 3, 184, 102, 10, 44, 250, 190, 109, 109, 71, 1, 223, 127, 164, 48, 33, 156, 38, 7, 137, 23, 113,
            211, 92, 187, 7, 48, 77, 72, 212, 160, 211, 83, 237, 140, 109, 7, 169, 68, 90, 54, 16, 0, 0, 0, 240, 159,
            144, 159, 8, 47, 70, 50, 80, 111, 111, 108, 47, 21, 77, 105, 110, 101, 100, 32, 98, 121, 32, 100, 101, 115,
            105, 114, 101, 48, 98, 108, 111, 111, 100, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 0, 139, 153, 0, 0,
            0, 0, 0, 0, 5, 187, 27, 39, 38, 0, 0, 0, 0, 25, 118, 169, 20, 200, 37, 161, 236, 242, 166, 131, 12, 68, 1,
            98, 12, 58, 22, 241, 153, 80, 87, 194, 171, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 170, 33, 169,
            237, 86, 163, 32, 240, 188, 72, 153, 79, 133, 68, 9, 238, 176, 2, 50, 226, 177, 223, 164, 133, 26, 152, 49,
            161, 185, 102, 220, 170, 99, 227, 114, 87, 0, 0, 0, 0, 0, 0, 0, 0, 54, 106, 52, 72, 97, 116, 104, 232, 130,
            93, 41, 228, 136, 237, 32, 216, 112, 97, 76, 164, 175, 184, 3, 104, 56, 178, 43, 175, 32, 37, 234, 23, 252,
            190, 219, 74, 112, 68, 124, 3, 99, 37, 157, 237, 152, 64, 109, 136, 19, 4, 125, 51, 65, 37, 30, 0, 0, 0, 0,
            0, 0, 0, 0, 44, 106, 76, 41, 82, 83, 75, 66, 76, 79, 67, 75, 58, 170, 205, 131, 85, 27, 176, 12, 87, 167,
            226, 152, 91, 245, 2, 190, 36, 149, 125, 33, 54, 234, 116, 132, 223, 150, 177, 223, 31, 0, 50, 160, 220, 0,
            0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 185, 225, 27, 109, 225, 30, 80, 80, 70, 123, 6, 214, 144, 201, 29, 74,
            131, 237, 148, 152, 8, 172, 28, 105, 238, 80, 127, 97, 235, 217, 89, 198, 213, 110, 49, 220, 48, 55, 89,
            59, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9, 23,
            145, 127, 20, 105, 199, 111, 157, 185, 169, 18, 242, 188, 211, 41, 104, 186, 68, 156, 21, 6, 80, 100, 34,
            123, 250, 81, 239, 81, 135, 147, 243, 203, 123, 8, 191, 148, 82, 242, 214, 205, 186, 28, 54, 117, 99, 209,
            166, 124, 73, 128, 53, 224, 221, 51, 66, 18, 0, 21, 248, 89, 85, 47, 9, 44, 119, 58, 250, 251, 193, 109,
            43, 93, 17, 229, 191, 172, 113, 43, 49, 89, 39, 101, 172, 224, 117, 109, 3, 88, 129, 83, 232, 208, 172,
            174, 163, 192, 33, 209, 237, 160, 240, 22, 44, 217, 217, 187, 243, 235, 105, 14, 108, 110, 61, 180, 202,
            110, 186, 183, 26, 66, 92, 231, 153, 219, 126, 97, 200, 229, 102, 165, 84, 78, 175, 147, 69, 115, 185, 195,
            46, 144, 44, 53, 185, 101, 71, 131, 17, 92, 195, 251, 5, 70, 71, 62, 227, 28, 137, 91, 11, 74, 150, 118,
            193, 43, 204, 28, 51, 244, 124, 234, 80, 112, 79, 157, 166, 59, 45, 147, 76, 197, 41, 1, 96, 237, 217, 106,
            131, 51, 226, 218, 55, 214, 195, 195, 116, 11, 188, 123, 98, 234, 156, 213, 18, 32, 99, 151, 189, 41, 94,
            213, 78, 113, 97, 238, 77, 82, 210, 158, 121, 78, 253, 110, 220, 82, 56, 195, 153, 60, 152, 133, 219, 255,
            33, 211, 26, 133, 202, 14, 177, 71, 223, 167, 154, 209, 159, 58, 203, 231, 217, 88, 4, 141, 168, 152, 182,
            36, 206, 7, 106, 14, 98, 196, 75, 131, 42, 10, 168, 242, 132, 226, 117, 162, 68, 109, 253, 246, 9, 190,
            102, 52, 52, 56, 123, 118, 136, 218, 160, 0, 0, 0, 0, 4, 232, 226, 216, 127, 249, 81, 107, 7, 75, 216, 190,
            19, 138, 234, 173, 198, 219, 179, 7, 181, 227, 90, 109, 126, 196, 111, 237, 169, 125, 224, 137, 201, 63,
            238, 229, 36, 76, 24, 175, 182, 18, 196, 132, 106, 139, 241, 176, 119, 89, 254, 219, 222, 201, 209, 57,
            173, 233, 244, 200, 39, 54, 225, 127, 232, 19, 211, 64, 82, 2, 171, 68, 162, 110, 58, 122, 152, 98, 106,
            214, 166, 198, 112, 101, 233, 75, 90, 31, 6, 119, 245, 55, 206, 55, 106, 113, 189, 3, 253, 90, 42, 246,
            223, 27, 215, 187, 142, 158, 75, 214, 82, 232, 150, 136, 168, 202, 147, 104, 209, 174, 26, 86, 209, 17, 23,
            14, 15, 60, 69, 14, 0, 0, 0, 4, 0, 0, 32, 218, 240, 36, 68, 3, 189, 102, 83, 191, 140, 42, 249, 90, 164,
            43, 213, 139, 25, 109, 215, 210, 120, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 41, 203, 122, 2, 233, 52, 187, 81,
            171, 140, 211, 106, 141, 126, 13, 254, 25, 119, 160, 36, 194, 189, 254, 188, 86, 168, 164, 74, 206, 231,
            236, 188, 80, 138, 143, 96, 99, 168, 13, 23, 183, 74, 219, 53, 0, 1, 16, 32, 86, 205, 212, 211, 86, 106,
            137, 201, 214, 188, 23, 205, 100, 127, 64, 4, 197, 15, 29, 103, 232, 12, 143, 90, 81, 130, 49, 193, 242,
            71, 225, 195, 176, 94, 35, 241, 132, 81, 135, 222, 157, 119, 101, 114, 186, 140, 113, 99, 115, 204, 172,
            94, 69, 135, 238, 131, 1, 233, 161, 60, 251, 123, 251, 174, 90, 138, 143, 96, 140, 254, 2, 24, 0, 0, 0, 0,
            1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 255, 255, 255, 255, 100, 3, 184, 102, 10, 44, 250, 190, 109, 109, 168, 87, 17, 174, 236, 146, 226, 87,
            31, 52, 215, 17, 30, 154, 7, 108, 236, 61, 196, 237, 37, 2, 226, 98, 171, 42, 65, 91, 199, 168, 159, 41,
            16, 0, 0, 0, 240, 159, 144, 159, 8, 47, 70, 50, 80, 111, 111, 108, 47, 13, 77, 105, 110, 101, 100, 32, 98,
            121, 32, 103, 101, 111, 49, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 0, 129,
            38, 0, 0, 0, 0, 0, 0, 4, 107, 39, 72, 38, 0, 0, 0, 0, 25, 118, 169, 20, 200, 37, 161, 236, 242, 166, 131,
            12, 68, 1, 98, 12, 58, 22, 241, 153, 80, 87, 194, 171, 136, 172, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 170,
            33, 169, 237, 204, 124, 57, 238, 144, 102, 231, 143, 253, 45, 11, 22, 183, 146, 196, 17, 210, 143, 238,
            188, 212, 211, 227, 171, 194, 39, 252, 231, 51, 209, 225, 172, 0, 0, 0, 0, 0, 0, 0, 0, 54, 106, 52, 72, 97,
            116, 104, 44, 12, 84, 76, 31, 81, 168, 120, 107, 146, 57, 169, 189, 39, 121, 120, 35, 224, 30, 203, 29,
            185, 244, 104, 78, 72, 145, 106, 169, 185, 98, 50, 243, 252, 3, 146, 67, 211, 76, 15, 182, 44, 208, 37,
            207, 109, 38, 182, 0, 0, 0, 0, 0, 0, 0, 0, 38, 106, 36, 185, 225, 27, 109, 225, 30, 80, 80, 70, 123, 6,
            214, 144, 201, 29, 74, 131, 237, 148, 152, 8, 172, 28, 105, 238, 80, 127, 97, 235, 217, 89, 198, 213, 110,
            49, 220, 232, 100, 183, 61, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 9, 23, 145, 127, 20, 105, 199, 111, 157, 185, 169, 18, 242, 188, 211, 41, 104, 186, 68,
            156, 21, 6, 80, 100, 34, 123, 250, 81, 239, 81, 135, 147, 243, 237, 62, 118, 198, 206, 238, 98, 45, 3, 106,
            185, 209, 76, 6, 178, 74, 231, 133, 240, 33, 89, 117, 217, 130, 160, 35, 137, 39, 63, 128, 186, 104, 95,
            125, 140, 227, 121, 155, 197, 130, 17, 62, 132, 40, 198, 24, 162, 208, 156, 119, 236, 248, 139, 17, 109,
            55, 19, 38, 38, 160, 50, 161, 61, 210, 251, 80, 157, 61, 145, 63, 148, 56, 60, 237, 48, 116, 233, 106, 26,
            159, 244, 113, 251, 78, 3, 38, 64, 99, 8, 26, 194, 43, 129, 80, 159, 72, 36, 18, 122, 218, 134, 172, 30,
            75, 33, 68, 33, 164, 251, 66, 126, 228, 150, 150, 198, 48, 178, 89, 108, 218, 56, 232, 114, 14, 91, 49, 18,
            43, 171, 6, 69, 174, 242, 240, 30, 68, 112, 160, 92, 55, 246, 109, 252, 145, 120, 67, 95, 118, 102, 238,
            151, 151, 60, 148, 135, 58, 121, 99, 252, 167, 91, 34, 171, 119, 233, 124, 43, 14, 101, 232, 52, 32, 71,
            182, 58, 32, 239, 246, 182, 107, 106, 185, 147, 46, 205, 1, 85, 185, 239, 108, 128, 76, 211, 18, 193, 135,
            139, 254, 210, 215, 67, 100, 133, 164, 67, 244, 99, 250, 233, 117, 64, 5, 27, 168, 33, 149, 29, 55, 60,
            164, 55, 146, 110, 195, 124, 170, 66, 153, 37, 237, 230, 127, 222, 9, 104, 135, 41, 247, 55, 128, 78, 137,
            171, 32, 138, 31, 180, 178, 193, 108, 146, 92, 87, 255, 62, 197, 0, 0, 0, 0, 4, 227, 123, 119, 103, 90,
            239, 41, 90, 184, 154, 206, 64, 19, 141, 219, 32, 133, 181, 12, 79, 65, 178, 131, 249, 223, 69, 134, 59,
            132, 35, 41, 119, 63, 238, 229, 36, 76, 24, 175, 182, 18, 196, 132, 106, 139, 241, 176, 119, 89, 254, 219,
            222, 201, 209, 57, 173, 233, 244, 200, 39, 54, 225, 127, 232, 19, 211, 64, 82, 2, 171, 68, 162, 110, 58,
            122, 152, 98, 106, 214, 166, 198, 112, 101, 233, 75, 90, 31, 6, 119, 245, 55, 206, 55, 106, 113, 189, 15,
            193, 175, 217, 17, 59, 212, 43, 26, 218, 137, 77, 65, 247, 18, 62, 212, 229, 8, 2, 167, 89, 47, 61, 149,
            191, 109, 121, 178, 138, 27, 97, 14, 0, 0, 0, 4, 0, 0, 32, 218, 240, 36, 68, 3, 189, 102, 83, 191, 140, 42,
            249, 90, 164, 43, 213, 139, 25, 109, 215, 210, 120, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 93, 180, 127, 252, 218,
            30, 190, 2, 241, 188, 126, 250, 140, 254, 89, 141, 122, 70, 62, 193, 108, 51, 17, 177, 70, 236, 200, 213,
            232, 85, 159, 202, 112, 138, 143, 96, 99, 168, 13, 23, 177, 233, 235, 113,
        ];
        let mut reader = Reader::new(headers_bytes);
        let headers = reader.read_list::<BlockHeader>().unwrap();
        for header in headers.iter() {
            assert_eq!(header.version, AUX_POW_VERSION_SYS);
            assert!(header.aux_pow.is_some());
        }
        let serialized = serialize_list(&headers);
        assert_eq!(serialized.take(), headers_bytes);
    }

    #[test]
    fn test_qtum_block_headers_serde_11() {
        let headers_bytes: &[u8] = &[
            11, 0, 0, 0, 32, 84, 248, 53, 90, 38, 181, 8, 110, 19, 148, 91, 169, 240, 150, 182, 184, 55, 183, 121, 139,
            25, 204, 109, 86, 254, 164, 73, 253, 131, 248, 209, 188, 139, 217, 156, 97, 217, 113, 70, 233, 134, 46,
            198, 35, 63, 42, 245, 207, 162, 38, 194, 198, 192, 140, 187, 207, 133, 13, 129, 230, 129, 220, 75, 222,
            176, 146, 76, 95, 177, 152, 8, 26, 0, 0, 0, 0, 164, 45, 100, 130, 183, 211, 232, 228, 203, 167, 25, 125,
            44, 76, 114, 241, 2, 147, 221, 153, 106, 45, 219, 226, 118, 157, 78, 245, 205, 54, 2, 117, 221, 178, 135,
            247, 109, 175, 212, 234, 59, 84, 109, 232, 193, 212, 156, 39, 188, 164, 193, 160, 82, 18, 71, 17, 208, 172,
            132, 193, 80, 153, 114, 158, 15, 20, 53, 111, 205, 1, 11, 201, 236, 155, 27, 181, 249, 145, 163, 19, 65,
            200, 247, 5, 194, 63, 159, 11, 94, 197, 229, 167, 139, 202, 32, 80, 1, 0, 0, 0, 65, 32, 92, 171, 222, 183,
            152, 10, 131, 119, 237, 132, 198, 202, 122, 29, 94, 254, 157, 167, 79, 205, 158, 137, 195, 88, 213, 73,
            144, 217, 100, 206, 126, 143, 85, 78, 221, 95, 221, 36, 237, 168, 232, 153, 228, 73, 229, 98, 122, 42, 183,
            156, 95, 27, 182, 154, 214, 107, 102, 166, 48, 240, 204, 100, 193, 194, 0, 0, 0, 32, 45, 180, 196, 69, 152,
            237, 209, 202, 136, 123, 0, 224, 112, 199, 234, 35, 207, 231, 127, 11, 121, 45, 143, 218, 163, 186, 35,
            199, 141, 14, 74, 88, 7, 33, 4, 226, 55, 47, 177, 47, 110, 197, 214, 234, 245, 51, 145, 247, 103, 104, 44,
            8, 187, 218, 62, 29, 13, 248, 242, 138, 54, 127, 53, 231, 96, 147, 76, 95, 144, 135, 8, 26, 0, 0, 0, 0,
            164, 45, 100, 130, 183, 211, 232, 228, 203, 167, 25, 125, 44, 76, 114, 241, 2, 147, 221, 153, 106, 45, 219,
            226, 118, 157, 78, 245, 205, 54, 2, 117, 221, 178, 135, 247, 109, 175, 212, 234, 59, 84, 109, 232, 193,
            212, 156, 39, 188, 164, 193, 160, 82, 18, 71, 17, 208, 172, 132, 193, 80, 153, 114, 158, 80, 135, 235, 188,
            117, 127, 200, 73, 221, 83, 176, 97, 106, 109, 59, 128, 85, 242, 43, 48, 238, 63, 137, 154, 56, 14, 115,
            58, 175, 28, 138, 57, 2, 0, 0, 0, 65, 31, 186, 207, 83, 249, 74, 45, 37, 73, 208, 10, 137, 176, 39, 197,
            207, 213, 88, 27, 14, 93, 67, 22, 25, 49, 127, 113, 107, 80, 176, 145, 37, 195, 10, 76, 222, 67, 18, 177,
            208, 254, 88, 6, 68, 187, 19, 27, 73, 91, 181, 14, 111, 217, 203, 56, 72, 169, 238, 213, 94, 194, 96, 226,
            68, 113, 0, 0, 0, 32, 254, 42, 73, 106, 64, 102, 80, 104, 98, 135, 2, 229, 153, 170, 1, 70, 29, 80, 143, 2,
            163, 60, 44, 244, 103, 52, 26, 107, 6, 112, 166, 56, 113, 220, 182, 182, 188, 181, 197, 197, 119, 209, 224,
            158, 254, 47, 87, 212, 249, 238, 104, 241, 244, 149, 78, 212, 111, 16, 167, 142, 33, 73, 77, 119, 128, 147,
            76, 95, 88, 187, 8, 26, 0, 0, 0, 0, 164, 45, 100, 130, 183, 211, 232, 228, 203, 167, 25, 125, 44, 76, 114,
            241, 2, 147, 221, 153, 106, 45, 219, 226, 118, 157, 78, 245, 205, 54, 2, 117, 221, 178, 135, 247, 109, 175,
            212, 234, 59, 84, 109, 232, 193, 212, 156, 39, 188, 164, 193, 160, 82, 18, 71, 17, 208, 172, 132, 193, 80,
            153, 114, 158, 126, 41, 219, 12, 226, 204, 246, 250, 17, 137, 175, 134, 219, 249, 81, 218, 137, 144, 42,
            247, 122, 166, 11, 44, 209, 104, 178, 15, 109, 190, 33, 87, 2, 0, 0, 0, 65, 31, 108, 153, 195, 77, 192,
            145, 175, 143, 212, 238, 126, 172, 170, 162, 16, 228, 145, 227, 200, 136, 15, 80, 33, 20, 38, 18, 89, 242,
            182, 42, 209, 13, 40, 27, 250, 101, 148, 23, 66, 147, 8, 195, 250, 236, 251, 196, 46, 44, 181, 57, 83, 79,
            47, 218, 187, 118, 68, 93, 82, 187, 2, 179, 100, 148, 0, 0, 0, 32, 114, 126, 145, 132, 68, 36, 217, 64,
            205, 174, 182, 251, 51, 227, 180, 169, 224, 72, 51, 225, 42, 196, 116, 30, 173, 49, 189, 184, 227, 66, 129,
            101, 180, 184, 146, 146, 241, 21, 238, 45, 89, 85, 67, 227, 13, 36, 3, 125, 197, 178, 96, 249, 224, 40,
            223, 148, 4, 8, 182, 35, 5, 233, 25, 139, 176, 147, 76, 95, 250, 84, 8, 26, 0, 0, 0, 0, 164, 45, 100, 130,
            183, 211, 232, 228, 203, 167, 25, 125, 44, 76, 114, 241, 2, 147, 221, 153, 106, 45, 219, 226, 118, 157, 78,
            245, 205, 54, 2, 117, 221, 178, 135, 247, 109, 175, 212, 234, 59, 84, 109, 232, 193, 212, 156, 39, 188,
            164, 193, 160, 82, 18, 71, 17, 208, 172, 132, 193, 80, 153, 114, 158, 233, 212, 67, 169, 198, 132, 228,
            111, 9, 40, 203, 56, 94, 145, 97, 13, 117, 149, 132, 178, 20, 128, 133, 48, 186, 154, 73, 251, 150, 89,
            142, 201, 1, 0, 0, 0, 65, 32, 223, 7, 71, 175, 243, 162, 41, 40, 153, 243, 225, 59, 69, 79, 111, 5, 1, 0,
            249, 113, 228, 127, 29, 81, 245, 130, 248, 235, 28, 41, 152, 3, 56, 220, 165, 190, 245, 222, 211, 120, 242,
            233, 226, 200, 93, 196, 84, 121, 56, 87, 104, 184, 55, 228, 167, 245, 57, 139, 111, 119, 240, 45, 144, 99,
            0, 0, 0, 32, 148, 10, 172, 208, 86, 229, 240, 133, 0, 72, 98, 247, 241, 120, 3, 170, 194, 55, 84, 91, 231,
            178, 107, 15, 109, 176, 56, 73, 240, 31, 136, 4, 133, 192, 185, 41, 241, 137, 142, 220, 180, 134, 114, 77,
            14, 104, 169, 186, 60, 73, 219, 216, 41, 147, 140, 228, 239, 168, 51, 123, 128, 32, 84, 72, 224, 147, 76,
            95, 67, 3, 8, 26, 0, 0, 0, 0, 164, 45, 100, 130, 183, 211, 232, 228, 203, 167, 25, 125, 44, 76, 114, 241,
            2, 147, 221, 153, 106, 45, 219, 226, 118, 157, 78, 245, 205, 54, 2, 117, 221, 178, 135, 247, 109, 175, 212,
            234, 59, 84, 109, 232, 193, 212, 156, 39, 188, 164, 193, 160, 82, 18, 71, 17, 208, 172, 132, 193, 80, 153,
            114, 158, 156, 139, 183, 171, 75, 206, 255, 142, 247, 23, 198, 102, 54, 132, 174, 122, 65, 12, 121, 102,
            112, 40, 3, 101, 15, 23, 95, 165, 211, 181, 138, 45, 28, 0, 0, 0, 130, 31, 55, 154, 187, 125, 213, 81, 206,
            220, 143, 45, 236, 223, 185, 22, 130, 128, 236, 228, 132, 162, 252, 96, 93, 149, 58, 215, 198, 73, 164,
            132, 85, 225, 126, 118, 14, 80, 141, 207, 245, 11, 166, 9, 30, 131, 93, 243, 236, 109, 50, 160, 109, 244,
            33, 75, 82, 52, 229, 142, 101, 41, 74, 124, 255, 255, 31, 24, 191, 133, 48, 252, 149, 19, 178, 127, 244,
            30, 195, 70, 141, 215, 201, 141, 251, 86, 103, 67, 138, 87, 169, 64, 100, 252, 185, 212, 22, 57, 197, 88,
            41, 62, 229, 249, 209, 225, 58, 58, 182, 112, 147, 181, 240, 177, 154, 26, 227, 16, 131, 4, 135, 93, 49,
            83, 75, 86, 217, 150, 110, 78, 67, 0, 0, 0, 32, 108, 83, 240, 180, 48, 164, 126, 147, 244, 99, 248, 43,
            188, 112, 176, 142, 222, 159, 109, 17, 9, 179, 13, 180, 101, 252, 238, 221, 51, 156, 223, 149, 123, 150,
            111, 239, 239, 203, 72, 143, 98, 240, 161, 96, 220, 56, 183, 60, 162, 0, 185, 24, 63, 101, 110, 76, 133,
            126, 2, 129, 65, 90, 34, 251, 240, 147, 76, 95, 173, 180, 7, 26, 0, 0, 0, 0, 164, 45, 100, 130, 183, 211,
            232, 228, 203, 167, 25, 125, 44, 76, 114, 241, 2, 147, 221, 153, 106, 45, 219, 226, 118, 157, 78, 245, 205,
            54, 2, 117, 221, 178, 135, 247, 109, 175, 212, 234, 59, 84, 109, 232, 193, 212, 156, 39, 188, 164, 193,
            160, 82, 18, 71, 17, 208, 172, 132, 193, 80, 153, 114, 158, 112, 182, 36, 39, 191, 36, 13, 247, 234, 4, 29,
            108, 116, 115, 176, 235, 149, 11, 6, 140, 182, 31, 215, 148, 229, 78, 213, 202, 210, 179, 192, 88, 1, 0, 0,
            0, 65, 31, 100, 150, 146, 162, 176, 72, 154, 202, 37, 120, 254, 171, 246, 43, 124, 147, 2, 135, 132, 183,
            89, 20, 12, 37, 7, 89, 82, 7, 220, 56, 145, 205, 53, 233, 89, 193, 160, 182, 113, 234, 225, 132, 217, 76,
            41, 44, 82, 149, 235, 116, 91, 36, 248, 37, 161, 47, 141, 216, 189, 102, 76, 7, 76, 32, 0, 0, 0, 32, 176,
            49, 240, 249, 175, 178, 209, 115, 195, 57, 234, 108, 161, 102, 76, 225, 166, 98, 179, 144, 108, 125, 133,
            128, 161, 115, 187, 237, 163, 219, 130, 154, 246, 86, 242, 85, 69, 211, 161, 86, 102, 40, 130, 37, 227, 88,
            109, 111, 20, 182, 132, 45, 227, 98, 180, 18, 113, 162, 30, 160, 140, 83, 190, 88, 96, 148, 76, 95, 177,
            75, 7, 26, 0, 0, 0, 0, 164, 45, 100, 130, 183, 211, 232, 228, 203, 167, 25, 125, 44, 76, 114, 241, 2, 147,
            221, 153, 106, 45, 219, 226, 118, 157, 78, 245, 205, 54, 2, 117, 221, 178, 135, 247, 109, 175, 212, 234,
            59, 84, 109, 232, 193, 212, 156, 39, 188, 164, 193, 160, 82, 18, 71, 17, 208, 172, 132, 193, 80, 153, 114,
            158, 225, 158, 3, 111, 51, 13, 178, 17, 13, 89, 241, 15, 46, 217, 133, 227, 101, 15, 233, 11, 34, 56, 167,
            181, 128, 104, 138, 211, 30, 126, 78, 104, 1, 0, 0, 0, 65, 31, 221, 135, 0, 218, 137, 174, 200, 234, 25,
            246, 71, 164, 146, 218, 162, 123, 159, 103, 84, 224, 119, 27, 28, 7, 87, 170, 187, 161, 245, 25, 76, 218,
            94, 97, 76, 130, 28, 2, 84, 190, 101, 84, 110, 157, 182, 201, 0, 246, 20, 226, 134, 30, 125, 40, 104, 191,
            28, 221, 248, 155, 252, 30, 198, 51, 0, 0, 0, 32, 173, 216, 94, 63, 96, 82, 158, 179, 106, 63, 161, 252,
            219, 233, 142, 34, 58, 85, 55, 251, 38, 166, 227, 118, 195, 106, 137, 120, 137, 29, 120, 156, 108, 185,
            238, 156, 201, 134, 140, 72, 44, 50, 172, 96, 81, 201, 40, 49, 47, 144, 1, 62, 34, 68, 198, 21, 131, 190,
            235, 30, 58, 139, 193, 199, 224, 149, 76, 95, 40, 61, 7, 26, 0, 0, 0, 0, 164, 45, 100, 130, 183, 211, 232,
            228, 203, 167, 25, 125, 44, 76, 114, 241, 2, 147, 221, 153, 106, 45, 219, 226, 118, 157, 78, 245, 205, 54,
            2, 117, 221, 178, 135, 247, 109, 175, 212, 234, 59, 84, 109, 232, 193, 212, 156, 39, 188, 164, 193, 160,
            82, 18, 71, 17, 208, 172, 132, 193, 80, 153, 114, 158, 159, 4, 3, 118, 76, 106, 199, 80, 166, 147, 76, 83,
            178, 58, 161, 160, 99, 234, 239, 226, 71, 129, 16, 154, 113, 142, 171, 25, 216, 81, 55, 67, 1, 0, 0, 0, 65,
            32, 146, 73, 82, 253, 190, 226, 250, 245, 235, 109, 221, 150, 114, 15, 225, 11, 36, 63, 115, 202, 119, 127,
            217, 191, 181, 51, 111, 146, 209, 80, 129, 155, 40, 154, 29, 193, 16, 63, 172, 152, 215, 59, 137, 20, 156,
            77, 9, 89, 22, 67, 160, 2, 193, 173, 247, 18, 90, 54, 251, 62, 167, 186, 197, 88, 0, 0, 0, 32, 108, 183,
            41, 174, 13, 230, 76, 231, 233, 110, 211, 167, 7, 107, 102, 16, 218, 49, 178, 102, 8, 144, 98, 44, 0, 23,
            31, 224, 222, 136, 91, 198, 105, 15, 233, 57, 85, 44, 115, 125, 77, 144, 28, 117, 110, 248, 145, 46, 187,
            121, 41, 104, 160, 26, 207, 90, 8, 237, 227, 200, 123, 254, 153, 110, 16, 150, 76, 95, 230, 51, 8, 26, 0,
            0, 0, 0, 164, 45, 100, 130, 183, 211, 232, 228, 203, 167, 25, 125, 44, 76, 114, 241, 2, 147, 221, 153, 106,
            45, 219, 226, 118, 157, 78, 245, 205, 54, 2, 117, 221, 178, 135, 247, 109, 175, 212, 234, 59, 84, 109, 232,
            193, 212, 156, 39, 188, 164, 193, 160, 82, 18, 71, 17, 208, 172, 132, 193, 80, 153, 114, 158, 16, 121, 152,
            51, 192, 203, 189, 183, 31, 30, 22, 171, 79, 85, 206, 192, 115, 82, 71, 28, 178, 28, 135, 6, 121, 164, 23,
            230, 88, 78, 64, 105, 2, 0, 0, 0, 65, 31, 75, 23, 186, 246, 44, 185, 94, 212, 150, 98, 117, 27, 27, 221,
            170, 5, 247, 36, 43, 26, 108, 131, 249, 144, 53, 54, 182, 159, 128, 24, 67, 93, 33, 242, 50, 81, 15, 233,
            179, 147, 13, 146, 178, 106, 123, 128, 98, 63, 160, 8, 199, 136, 15, 103, 125, 229, 132, 199, 75, 209, 61,
            14, 142, 50, 0, 0, 0, 32, 185, 1, 22, 170, 87, 98, 186, 17, 242, 169, 93, 39, 10, 188, 170, 150, 192, 235,
            131, 77, 180, 153, 186, 73, 231, 123, 166, 13, 63, 194, 6, 144, 79, 174, 64, 113, 102, 126, 213, 254, 151,
            115, 10, 85, 113, 140, 70, 20, 212, 111, 251, 108, 161, 105, 88, 0, 102, 21, 17, 4, 161, 50, 228, 65, 160,
            150, 76, 95, 115, 227, 7, 26, 0, 0, 0, 0, 164, 45, 100, 130, 183, 211, 232, 228, 203, 167, 25, 125, 44, 76,
            114, 241, 2, 147, 221, 153, 106, 45, 219, 226, 118, 157, 78, 245, 205, 54, 2, 117, 221, 178, 135, 247, 109,
            175, 212, 234, 59, 84, 109, 232, 193, 212, 156, 39, 188, 164, 193, 160, 82, 18, 71, 17, 208, 172, 132, 193,
            80, 153, 114, 158, 60, 42, 198, 17, 174, 7, 59, 141, 168, 12, 37, 8, 135, 23, 213, 20, 248, 186, 220, 198,
            11, 55, 199, 189, 36, 223, 29, 227, 255, 89, 200, 128, 2, 0, 0, 0, 65, 31, 96, 101, 239, 29, 169, 30, 193,
            224, 111, 44, 10, 229, 143, 19, 197, 157, 145, 33, 54, 38, 49, 140, 48, 148, 128, 173, 16, 110, 67, 185,
            218, 121, 115, 63, 43, 162, 200, 77, 145, 51, 74, 251, 201, 45, 40, 59, 155, 230, 186, 142, 159, 184, 41,
            27, 74, 1, 14, 173, 203, 246, 62, 117, 28, 14, 0, 0, 0, 32, 104, 17, 117, 135, 192, 203, 57, 114, 17, 110,
            103, 215, 4, 58, 107, 192, 79, 21, 44, 76, 2, 8, 183, 228, 76, 177, 172, 196, 15, 40, 122, 2, 126, 168, 93,
            207, 82, 209, 116, 85, 112, 162, 58, 225, 83, 191, 142, 245, 174, 223, 98, 186, 187, 132, 28, 242, 21, 22,
            208, 239, 218, 219, 50, 150, 224, 150, 76, 95, 73, 243, 7, 26, 0, 0, 0, 0, 164, 45, 100, 130, 183, 211,
            232, 228, 203, 167, 25, 125, 44, 76, 114, 241, 2, 147, 221, 153, 106, 45, 219, 226, 118, 157, 78, 245, 205,
            54, 2, 117, 221, 178, 135, 247, 109, 175, 212, 234, 59, 84, 109, 232, 193, 212, 156, 39, 188, 164, 193,
            160, 82, 18, 71, 17, 208, 172, 132, 193, 80, 153, 114, 158, 151, 207, 93, 166, 125, 159, 189, 9, 42, 152,
            28, 47, 11, 93, 73, 80, 156, 50, 201, 143, 162, 33, 205, 25, 158, 133, 161, 228, 67, 207, 109, 216, 2, 0,
            0, 0, 65, 31, 51, 71, 165, 69, 107, 96, 27, 47, 65, 155, 93, 145, 49, 183, 182, 228, 207, 33, 41, 124, 31,
            226, 1, 77, 51, 114, 115, 8, 152, 211, 49, 161, 62, 190, 80, 119, 154, 30, 193, 226, 46, 248, 169, 69, 226,
            86, 134, 101, 238, 115, 14, 63, 174, 123, 30, 7, 123, 174, 60, 13, 100, 49, 23, 123,
        ];
        let mut reader = Reader::new_with_coin_variant(headers_bytes, CoinVariant::Qtum);
        let headers = reader.read_list::<BlockHeader>().unwrap();
        for header in headers.iter() {
            assert_eq!(header.version, BIP9_NO_SOFT_FORK_BLOCK_HEADER_VERSION);
        }
        let serialized = serialize_list(&headers);
        assert_eq!(serialized.take(), headers_bytes);
    }

    #[test]
    fn test_zer_block_headers_serde_11() {
        let headers_bytes: &[u8] = &[
            11, 4, 0, 0, 0, 12, 31, 177, 243, 98, 47, 86, 82, 110, 153, 169, 148, 211, 117, 114, 54, 80, 129, 169, 243,
            56, 141, 237, 18, 155, 178, 228, 245, 86, 6, 0, 0, 248, 96, 174, 126, 114, 121, 252, 224, 11, 0, 214, 214,
            166, 106, 123, 99, 176, 203, 205, 143, 86, 202, 166, 74, 7, 116, 213, 102, 123, 230, 33, 93, 28, 194, 158,
            128, 255, 171, 220, 133, 139, 225, 108, 38, 25, 223, 68, 22, 199, 42, 33, 201, 82, 68, 139, 87, 250, 170,
            233, 53, 75, 189, 189, 96, 127, 171, 192, 96, 22, 124, 13, 30, 48, 0, 21, 196, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 0, 0, 0, 2, 1, 2, 0, 253, 144, 1, 2, 116, 130, 132, 119, 206, 105, 12,
            86, 123, 18, 81, 150, 148, 143, 108, 115, 13, 206, 211, 112, 155, 212, 102, 72, 21, 50, 186, 60, 13, 252,
            17, 157, 69, 52, 162, 37, 28, 23, 212, 127, 182, 244, 51, 137, 3, 11, 236, 12, 93, 40, 141, 202, 192, 254,
            17, 14, 87, 165, 124, 60, 4, 37, 82, 238, 175, 29, 184, 49, 186, 81, 205, 245, 14, 33, 83, 31, 190, 245,
            154, 191, 229, 207, 158, 89, 109, 222, 54, 25, 122, 39, 39, 143, 141, 226, 221, 215, 108, 61, 190, 5, 12,
            131, 141, 2, 26, 3, 46, 100, 212, 178, 149, 167, 243, 85, 85, 194, 169, 62, 205, 234, 159, 144, 34, 226,
            12, 34, 4, 250, 84, 3, 226, 41, 213, 214, 68, 82, 152, 164, 158, 53, 150, 163, 134, 173, 185, 25, 191, 30,
            88, 10, 32, 166, 24, 209, 119, 209, 138, 106, 87, 129, 103, 178, 89, 14, 217, 172, 236, 197, 79, 56, 52,
            230, 197, 55, 32, 118, 198, 124, 163, 97, 43, 243, 163, 188, 99, 78, 195, 18, 37, 10, 145, 167, 126, 169,
            246, 157, 174, 41, 197, 3, 97, 8, 133, 173, 246, 164, 71, 159, 153, 2, 157, 209, 91, 69, 193, 118, 34, 216,
            190, 79, 242, 108, 57, 69, 30, 139, 247, 103, 248, 182, 10, 52, 225, 146, 100, 119, 56, 138, 62, 94, 98,
            138, 182, 52, 30, 229, 239, 206, 221, 6, 43, 4, 89, 177, 164, 221, 208, 40, 24, 60, 201, 162, 223, 3, 189,
            189, 98, 161, 33, 67, 73, 190, 11, 143, 14, 234, 47, 76, 26, 144, 3, 237, 198, 37, 90, 18, 2, 93, 84, 22,
            1, 152, 237, 114, 67, 42, 238, 56, 190, 6, 37, 70, 92, 156, 239, 145, 224, 31, 210, 26, 21, 49, 197, 43, 3,
            148, 111, 17, 234, 69, 165, 208, 7, 170, 7, 140, 166, 197, 227, 195, 139, 135, 196, 51, 88, 65, 80, 192,
            73, 1, 247, 156, 144, 227, 238, 141, 204, 177, 40, 27, 179, 172, 64, 7, 11, 148, 156, 31, 251, 129, 50,
            242, 43, 149, 98, 81, 159, 10, 42, 40, 91, 71, 127, 146, 30, 223, 87, 227, 159, 110, 158, 103, 149, 26, 25,
            173, 202, 90, 41, 95, 149, 9, 223, 157, 211, 191, 246, 237, 195, 4, 0, 0, 0, 183, 19, 227, 196, 219, 114,
            34, 159, 170, 142, 14, 29, 143, 244, 4, 235, 6, 189, 224, 115, 3, 38, 71, 154, 200, 206, 19, 222, 242, 3,
            0, 0, 0, 239, 187, 77, 126, 12, 71, 174, 10, 215, 7, 16, 213, 83, 8, 54, 212, 50, 202, 48, 3, 73, 82, 93,
            219, 120, 72, 139, 0, 220, 6, 225, 94, 211, 180, 239, 16, 221, 39, 220, 196, 255, 10, 113, 209, 66, 22, 50,
            106, 128, 200, 192, 110, 142, 38, 218, 211, 246, 27, 89, 177, 28, 156, 15, 197, 172, 192, 96, 158, 85, 13,
            30, 23, 255, 255, 254, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 106, 246,
            75, 0, 253, 144, 1, 1, 151, 12, 120, 248, 148, 72, 78, 146, 196, 196, 115, 196, 217, 228, 37, 90, 166, 85,
            238, 47, 73, 216, 225, 241, 5, 52, 5, 35, 118, 150, 53, 207, 219, 127, 184, 22, 69, 135, 135, 94, 28, 163,
            57, 155, 250, 50, 233, 25, 110, 19, 25, 48, 190, 215, 43, 21, 64, 234, 117, 236, 108, 114, 11, 61, 172,
            137, 209, 4, 203, 132, 192, 186, 31, 136, 34, 47, 141, 74, 87, 147, 54, 254, 113, 61, 169, 87, 146, 54,
            208, 14, 83, 186, 213, 45, 236, 44, 159, 34, 220, 7, 5, 207, 208, 42, 104, 83, 186, 24, 148, 208, 203, 64,
            236, 148, 88, 197, 147, 25, 196, 191, 115, 4, 33, 57, 23, 49, 48, 70, 236, 208, 220, 75, 178, 60, 152, 227,
            51, 143, 86, 90, 179, 88, 129, 209, 132, 29, 10, 54, 119, 18, 98, 49, 230, 191, 77, 94, 26, 22, 87, 153,
            124, 230, 96, 182, 115, 153, 72, 33, 173, 192, 143, 10, 117, 226, 21, 91, 96, 164, 225, 34, 137, 9, 164,
            73, 220, 224, 39, 149, 140, 126, 132, 138, 142, 54, 95, 147, 181, 47, 92, 1, 171, 168, 92, 98, 251, 236,
            168, 217, 119, 100, 122, 3, 88, 234, 83, 116, 175, 49, 83, 227, 55, 69, 105, 41, 6, 227, 156, 255, 221,
            132, 25, 174, 180, 57, 234, 35, 81, 176, 94, 37, 80, 8, 214, 111, 88, 239, 116, 106, 48, 10, 161, 172, 172,
            86, 138, 88, 234, 164, 55, 152, 182, 164, 131, 236, 6, 39, 242, 9, 206, 123, 167, 44, 230, 220, 16, 109,
            159, 234, 200, 199, 216, 44, 205, 89, 237, 80, 3, 11, 222, 70, 110, 246, 213, 88, 123, 157, 33, 22, 42, 28,
            78, 145, 40, 6, 224, 46, 45, 106, 59, 83, 48, 84, 195, 144, 211, 84, 54, 53, 101, 97, 148, 228, 29, 59, 77,
            38, 207, 113, 85, 135, 102, 199, 244, 249, 30, 41, 213, 182, 72, 180, 69, 119, 182, 22, 38, 141, 163, 59,
            58, 43, 112, 144, 43, 192, 77, 19, 230, 249, 142, 45, 235, 243, 93, 33, 142, 90, 101, 175, 44, 159, 29,
            170, 255, 27, 45, 130, 141, 185, 192, 21, 244, 35, 113, 251, 190, 138, 230, 145, 167, 132, 229, 71, 49,
            226, 139, 237, 30, 44, 84, 4, 0, 0, 0, 25, 54, 48, 205, 202, 249, 148, 69, 32, 106, 2, 200, 17, 9, 65, 109,
            223, 158, 225, 145, 91, 107, 9, 30, 229, 191, 250, 188, 61, 2, 0, 0, 92, 16, 151, 21, 67, 223, 226, 188,
            207, 109, 93, 146, 165, 13, 183, 129, 42, 84, 20, 83, 195, 231, 194, 158, 54, 31, 4, 180, 223, 94, 115,
            163, 94, 211, 180, 239, 16, 221, 39, 220, 196, 255, 10, 113, 209, 66, 22, 50, 106, 128, 200, 192, 110, 142,
            38, 218, 211, 246, 27, 89, 177, 28, 156, 15, 11, 173, 192, 96, 74, 123, 13, 30, 80, 0, 0, 16, 201, 142,
            246, 139, 26, 203, 135, 126, 144, 253, 177, 172, 231, 128, 52, 200, 0, 0, 0, 0, 0, 0, 0, 0, 18, 20, 82, 16,
            253, 144, 1, 0, 98, 58, 238, 210, 37, 13, 102, 208, 109, 14, 78, 224, 246, 33, 246, 100, 139, 150, 126,
            159, 111, 157, 124, 99, 124, 181, 188, 253, 219, 110, 236, 117, 197, 154, 71, 160, 72, 77, 26, 44, 58, 143,
            146, 72, 141, 227, 237, 222, 144, 0, 237, 152, 97, 217, 218, 192, 65, 235, 209, 217, 35, 147, 47, 169, 105,
            183, 250, 106, 11, 124, 167, 169, 44, 226, 69, 27, 248, 209, 146, 143, 157, 245, 240, 190, 59, 102, 5, 177,
            233, 255, 75, 68, 151, 149, 81, 9, 218, 176, 220, 4, 71, 3, 116, 228, 35, 88, 143, 156, 23, 135, 145, 112,
            95, 165, 6, 201, 255, 249, 46, 73, 163, 254, 159, 164, 18, 53, 54, 206, 63, 102, 9, 190, 63, 47, 129, 84,
            33, 60, 238, 22, 69, 192, 252, 146, 137, 241, 5, 217, 132, 15, 28, 242, 71, 210, 232, 45, 91, 140, 151, 24,
            231, 211, 61, 90, 27, 43, 164, 9, 169, 147, 29, 211, 56, 0, 21, 146, 97, 67, 76, 163, 163, 105, 236, 83,
            210, 143, 228, 152, 253, 75, 237, 33, 41, 187, 85, 189, 234, 124, 37, 2, 6, 29, 146, 103, 216, 209, 159,
            81, 95, 162, 132, 211, 244, 83, 109, 32, 118, 234, 51, 168, 163, 49, 162, 54, 15, 24, 242, 208, 183, 2,
            150, 44, 197, 158, 39, 6, 180, 9, 46, 222, 138, 63, 225, 35, 3, 241, 151, 217, 16, 22, 195, 161, 141, 131,
            200, 19, 58, 118, 210, 63, 61, 178, 208, 223, 47, 209, 100, 37, 116, 233, 229, 45, 185, 38, 24, 9, 20, 16,
            129, 224, 149, 14, 250, 149, 226, 174, 2, 137, 167, 23, 149, 242, 197, 89, 60, 170, 179, 163, 229, 6, 57,
            252, 36, 124, 152, 154, 85, 103, 85, 227, 87, 84, 150, 188, 34, 178, 154, 27, 130, 73, 11, 240, 39, 24, 36,
            219, 153, 180, 160, 87, 212, 64, 175, 157, 24, 6, 133, 244, 219, 251, 106, 78, 85, 195, 67, 141, 63, 185,
            163, 99, 217, 61, 230, 110, 235, 44, 219, 119, 59, 243, 108, 6, 150, 132, 119, 125, 43, 187, 37, 111, 205,
            202, 37, 224, 100, 143, 9, 120, 63, 207, 169, 197, 192, 25, 73, 253, 199, 26, 32, 70, 92, 141, 154, 225,
            167, 251, 119, 79, 189, 4, 0, 0, 0, 114, 193, 23, 14, 189, 208, 241, 210, 235, 78, 212, 47, 134, 20, 144,
            100, 38, 190, 237, 130, 9, 48, 89, 240, 54, 102, 198, 194, 59, 5, 0, 0, 211, 36, 35, 72, 44, 129, 145, 176,
            255, 55, 44, 62, 107, 93, 244, 86, 133, 235, 53, 186, 186, 165, 211, 153, 159, 26, 114, 82, 138, 137, 156,
            124, 94, 211, 180, 239, 16, 221, 39, 220, 196, 255, 10, 113, 209, 66, 22, 50, 106, 128, 200, 192, 110, 142,
            38, 218, 211, 246, 27, 89, 177, 28, 156, 15, 67, 173, 192, 96, 27, 198, 13, 30, 7, 255, 255, 218, 79, 137,
            28, 45, 118, 239, 254, 181, 174, 148, 7, 252, 18, 231, 114, 232, 0, 0, 0, 0, 0, 0, 0, 0, 103, 58, 84, 197,
            253, 144, 1, 0, 22, 47, 117, 135, 99, 116, 184, 198, 159, 60, 27, 69, 70, 90, 116, 34, 226, 61, 137, 2,
            251, 176, 251, 186, 28, 61, 184, 33, 242, 241, 24, 21, 179, 45, 144, 171, 130, 97, 243, 30, 177, 215, 53,
            46, 117, 154, 174, 52, 26, 21, 165, 191, 125, 42, 29, 15, 21, 193, 116, 14, 59, 98, 52, 250, 210, 23, 11,
            237, 113, 30, 87, 210, 12, 52, 74, 49, 174, 39, 136, 46, 87, 106, 236, 172, 187, 95, 151, 38, 108, 53, 226,
            18, 95, 25, 194, 179, 220, 8, 76, 1, 137, 141, 100, 28, 34, 192, 221, 109, 7, 241, 168, 52, 89, 15, 86,
            224, 69, 238, 223, 49, 157, 137, 30, 61, 13, 25, 236, 118, 195, 125, 30, 211, 155, 88, 204, 205, 211, 73,
            172, 98, 201, 91, 185, 36, 212, 206, 192, 239, 26, 20, 50, 68, 125, 144, 56, 155, 10, 142, 252, 102, 169,
            98, 39, 176, 222, 116, 7, 157, 41, 224, 95, 27, 156, 227, 39, 118, 22, 166, 233, 21, 80, 97, 50, 152, 114,
            127, 148, 207, 60, 156, 0, 239, 165, 151, 234, 83, 18, 122, 226, 3, 12, 166, 240, 166, 35, 15, 252, 24, 95,
            65, 234, 231, 216, 12, 36, 181, 110, 178, 16, 252, 59, 61, 201, 64, 59, 27, 190, 255, 18, 49, 143, 138, 60,
            42, 19, 255, 170, 222, 69, 190, 112, 123, 251, 35, 160, 83, 243, 32, 246, 5, 95, 254, 209, 203, 90, 213,
            193, 58, 212, 123, 91, 64, 243, 7, 51, 26, 134, 12, 190, 197, 28, 225, 63, 68, 94, 105, 96, 53, 129, 10,
            41, 206, 169, 127, 240, 216, 150, 45, 181, 44, 233, 195, 187, 80, 106, 75, 227, 7, 207, 7, 25, 61, 125,
            214, 222, 2, 61, 38, 2, 82, 61, 132, 13, 8, 154, 159, 54, 206, 9, 106, 149, 188, 196, 172, 13, 10, 170, 33,
            24, 63, 148, 65, 111, 153, 205, 84, 50, 152, 120, 146, 123, 77, 90, 176, 78, 217, 102, 171, 222, 7, 164,
            94, 68, 239, 189, 139, 38, 248, 45, 14, 122, 209, 53, 105, 237, 130, 90, 20, 189, 237, 124, 226, 155, 189,
            12, 184, 61, 244, 228, 241, 221, 60, 203, 146, 104, 85, 54, 185, 207, 100, 63, 12, 166, 89, 14, 197, 196,
            160, 89, 4, 0, 0, 0, 184, 13, 53, 28, 34, 206, 18, 14, 222, 170, 140, 99, 78, 243, 94, 174, 97, 174, 111,
            14, 125, 29, 171, 104, 208, 11, 239, 124, 76, 8, 0, 0, 163, 177, 219, 26, 119, 147, 226, 246, 243, 113, 37,
            60, 204, 179, 185, 196, 168, 55, 45, 16, 218, 209, 48, 166, 181, 229, 6, 36, 42, 49, 246, 158, 220, 106,
            150, 246, 77, 156, 74, 126, 248, 136, 124, 100, 155, 18, 239, 188, 174, 15, 248, 45, 212, 253, 101, 40,
            112, 132, 179, 217, 252, 199, 171, 36, 126, 173, 192, 96, 224, 77, 14, 30, 111, 255, 255, 240, 99, 131,
            114, 93, 159, 74, 31, 46, 183, 246, 245, 171, 101, 56, 221, 66, 0, 0, 0, 0, 0, 0, 0, 0, 4, 36, 63, 170,
            253, 144, 1, 3, 72, 133, 111, 88, 251, 72, 85, 244, 138, 50, 160, 4, 183, 108, 221, 189, 44, 93, 245, 239,
            243, 208, 155, 182, 52, 30, 149, 75, 60, 248, 151, 164, 126, 157, 226, 116, 100, 70, 30, 79, 148, 24, 174,
            17, 88, 209, 100, 195, 220, 20, 106, 29, 147, 38, 170, 115, 177, 118, 90, 190, 211, 193, 199, 129, 133,
            137, 47, 53, 82, 240, 148, 228, 199, 185, 29, 119, 154, 54, 111, 193, 232, 180, 74, 91, 7, 125, 84, 92, 81,
            126, 24, 97, 21, 190, 226, 185, 28, 212, 230, 3, 86, 102, 164, 215, 243, 90, 80, 40, 110, 100, 72, 118,
            185, 41, 132, 81, 99, 245, 227, 192, 2, 242, 218, 21, 5, 235, 4, 193, 156, 179, 82, 43, 170, 158, 234, 75,
            84, 103, 90, 170, 91, 146, 193, 209, 94, 51, 165, 4, 196, 41, 14, 206, 193, 40, 58, 91, 25, 241, 185, 36,
            240, 147, 222, 249, 43, 179, 130, 6, 208, 119, 169, 124, 27, 3, 47, 160, 0, 109, 35, 1, 24, 143, 59, 153,
            209, 67, 164, 71, 75, 15, 253, 61, 213, 207, 248, 83, 173, 42, 249, 3, 238, 219, 245, 42, 42, 219, 178,
            129, 86, 134, 55, 134, 2, 220, 228, 242, 207, 206, 87, 237, 199, 255, 192, 114, 22, 107, 154, 42, 140, 254,
            52, 23, 190, 223, 175, 2, 213, 146, 39, 51, 186, 218, 114, 159, 77, 207, 202, 154, 206, 20, 251, 193, 87,
            70, 14, 232, 50, 150, 89, 25, 86, 194, 11, 241, 20, 166, 173, 165, 5, 192, 137, 242, 80, 141, 35, 228, 216,
            167, 142, 150, 92, 175, 93, 80, 141, 82, 56, 26, 229, 44, 242, 21, 54, 108, 223, 227, 186, 78, 243, 5, 124,
            34, 157, 15, 89, 41, 95, 245, 90, 45, 140, 81, 182, 74, 226, 124, 48, 21, 178, 222, 138, 242, 14, 34, 23,
            170, 140, 43, 10, 208, 150, 236, 163, 145, 167, 107, 54, 115, 233, 163, 142, 199, 143, 128, 139, 139, 238,
            67, 248, 17, 255, 60, 203, 76, 132, 209, 129, 3, 76, 63, 139, 85, 246, 39, 198, 119, 45, 138, 85, 61, 91,
            163, 179, 56, 41, 189, 91, 104, 98, 32, 87, 181, 1, 242, 74, 62, 214, 105, 197, 197, 79, 186, 133, 187, 15,
            215, 185, 194, 254, 4, 0, 0, 0, 219, 64, 41, 178, 38, 227, 244, 33, 187, 109, 253, 46, 216, 106, 143, 42,
            98, 135, 238, 171, 76, 37, 122, 65, 83, 168, 179, 80, 68, 14, 0, 0, 132, 247, 218, 58, 201, 127, 9, 93,
            233, 202, 222, 108, 154, 45, 1, 186, 2, 230, 67, 100, 145, 167, 142, 250, 182, 89, 108, 107, 254, 197, 142,
            219, 220, 106, 150, 246, 77, 156, 74, 126, 248, 136, 124, 100, 155, 18, 239, 188, 174, 15, 248, 45, 212,
            253, 101, 40, 112, 132, 179, 217, 252, 199, 171, 36, 22, 174, 192, 96, 77, 50, 14, 30, 48, 0, 21, 98, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0, 93, 135, 30, 1, 253, 144, 1, 2, 29, 14,
            28, 87, 38, 225, 216, 60, 211, 155, 93, 130, 168, 155, 11, 207, 111, 245, 143, 226, 219, 247, 145, 215, 65,
            164, 228, 224, 21, 29, 95, 146, 168, 182, 211, 142, 213, 247, 34, 219, 252, 69, 142, 79, 99, 239, 63, 107,
            138, 5, 6, 161, 85, 137, 184, 196, 57, 4, 202, 133, 126, 7, 60, 110, 190, 231, 253, 209, 217, 112, 89, 186,
            24, 113, 6, 229, 169, 26, 220, 46, 94, 23, 120, 30, 195, 127, 85, 47, 66, 164, 93, 98, 246, 34, 47, 243,
            129, 156, 53, 33, 155, 85, 218, 239, 7, 95, 39, 133, 212, 43, 186, 50, 56, 250, 145, 238, 71, 163, 130,
            225, 175, 212, 132, 109, 43, 135, 203, 105, 182, 183, 223, 120, 176, 185, 156, 228, 51, 212, 32, 100, 235,
            154, 93, 150, 47, 121, 87, 110, 175, 40, 191, 82, 198, 170, 151, 104, 77, 27, 217, 177, 81, 166, 114, 60,
            62, 82, 8, 34, 142, 99, 81, 249, 133, 86, 59, 246, 255, 189, 146, 124, 84, 186, 174, 252, 229, 8, 214, 29,
            224, 220, 91, 183, 245, 168, 216, 185, 103, 230, 83, 8, 211, 81, 8, 67, 126, 70, 90, 127, 217, 137, 119,
            242, 255, 144, 211, 116, 65, 28, 254, 33, 67, 88, 222, 105, 11, 69, 180, 154, 213, 111, 2, 221, 182, 97,
            239, 48, 177, 166, 16, 241, 83, 103, 58, 62, 172, 49, 212, 59, 145, 13, 104, 142, 80, 230, 31, 85, 153,
            208, 121, 124, 48, 65, 249, 131, 115, 250, 51, 192, 135, 223, 69, 25, 247, 136, 31, 92, 33, 205, 81, 8,
            122, 173, 7, 30, 188, 232, 82, 48, 81, 251, 154, 249, 168, 145, 209, 155, 175, 255, 180, 25, 166, 20, 112,
            30, 44, 143, 83, 61, 30, 60, 23, 20, 2, 27, 179, 224, 40, 241, 174, 20, 155, 19, 231, 81, 74, 48, 69, 197,
            87, 201, 19, 52, 239, 183, 9, 115, 122, 110, 229, 166, 199, 205, 150, 240, 186, 91, 133, 195, 204, 29, 160,
            89, 164, 199, 108, 163, 114, 20, 210, 83, 68, 69, 185, 163, 203, 159, 24, 177, 150, 165, 78, 243, 235, 41,
            92, 96, 109, 206, 62, 171, 44, 124, 40, 216, 78, 151, 201, 215, 86, 61, 161, 32, 63, 142, 188, 179, 201,
            251, 227, 4, 0, 0, 0, 188, 89, 209, 206, 20, 73, 185, 57, 58, 66, 2, 229, 104, 106, 84, 183, 219, 195, 165,
            63, 228, 182, 178, 118, 77, 105, 184, 119, 89, 3, 0, 0, 206, 62, 63, 18, 131, 8, 22, 21, 63, 239, 158, 178,
            27, 204, 90, 34, 143, 129, 207, 16, 253, 173, 19, 225, 191, 217, 6, 254, 80, 152, 145, 65, 220, 106, 150,
            246, 77, 156, 74, 126, 248, 136, 124, 100, 155, 18, 239, 188, 174, 15, 248, 45, 212, 253, 101, 40, 112,
            132, 179, 217, 252, 199, 171, 36, 211, 174, 192, 96, 188, 89, 14, 30, 103, 254, 196, 207, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 230, 221, 179, 0, 253, 144, 1, 1, 209, 2, 79, 205,
            202, 0, 205, 32, 1, 211, 125, 23, 35, 10, 165, 75, 12, 49, 213, 213, 45, 12, 48, 128, 38, 172, 63, 88, 144,
            179, 84, 69, 91, 147, 226, 199, 138, 44, 144, 253, 214, 34, 82, 155, 155, 193, 198, 130, 89, 15, 150, 42,
            18, 233, 244, 38, 93, 107, 119, 242, 219, 225, 158, 248, 85, 202, 200, 101, 213, 140, 23, 156, 25, 104, 35,
            46, 101, 175, 254, 199, 95, 213, 80, 25, 59, 163, 9, 155, 178, 167, 82, 191, 226, 124, 124, 217, 168, 7, 9,
            4, 106, 110, 215, 60, 197, 210, 14, 9, 219, 140, 46, 227, 189, 104, 26, 178, 105, 195, 222, 219, 247, 247,
            251, 104, 19, 234, 38, 208, 72, 225, 84, 85, 237, 139, 59, 40, 17, 80, 26, 95, 23, 229, 102, 3, 54, 225, 2,
            42, 167, 6, 176, 158, 72, 60, 80, 22, 31, 119, 83, 246, 138, 97, 185, 21, 173, 216, 16, 189, 175, 72, 29,
            105, 136, 77, 25, 230, 197, 115, 109, 152, 39, 78, 136, 180, 107, 1, 2, 245, 21, 19, 104, 55, 149, 6, 55,
            208, 174, 53, 123, 5, 30, 84, 206, 250, 81, 218, 56, 97, 84, 63, 168, 225, 114, 243, 215, 2, 70, 6, 208,
            190, 83, 183, 215, 208, 8, 51, 155, 4, 143, 125, 175, 175, 13, 31, 46, 204, 243, 129, 10, 19, 219, 165,
            181, 94, 229, 116, 245, 92, 147, 16, 195, 53, 44, 22, 18, 235, 97, 165, 124, 50, 203, 101, 223, 71, 14,
            236, 188, 75, 142, 136, 15, 210, 64, 199, 27, 202, 77, 187, 236, 244, 219, 128, 25, 175, 169, 106, 2, 84,
            240, 221, 145, 46, 25, 62, 142, 184, 238, 2, 175, 14, 206, 204, 244, 172, 113, 5, 224, 128, 159, 215, 201,
            146, 76, 196, 205, 194, 206, 41, 78, 32, 69, 229, 8, 219, 31, 190, 3, 145, 76, 23, 22, 12, 167, 149, 204,
            154, 70, 221, 103, 196, 155, 91, 165, 195, 49, 129, 85, 206, 25, 29, 70, 79, 174, 29, 32, 202, 10, 4, 165,
            84, 208, 100, 163, 194, 210, 207, 25, 202, 222, 131, 137, 133, 186, 4, 60, 175, 159, 39, 71, 20, 221, 228,
            197, 211, 173, 223, 53, 171, 172, 19, 220, 37, 94, 163, 23, 181, 198, 16, 148, 4, 0, 0, 0, 251, 166, 65,
            54, 97, 9, 150, 105, 220, 120, 84, 74, 79, 29, 131, 221, 52, 171, 44, 84, 205, 207, 189, 153, 139, 197,
            224, 89, 142, 11, 0, 0, 57, 109, 36, 39, 134, 155, 193, 87, 125, 249, 164, 120, 228, 217, 182, 207, 245,
            95, 111, 57, 98, 205, 176, 34, 210, 218, 1, 17, 82, 62, 228, 8, 220, 106, 150, 246, 77, 156, 74, 126, 248,
            136, 124, 100, 155, 18, 239, 188, 174, 15, 248, 45, 212, 253, 101, 40, 112, 132, 179, 217, 252, 199, 171,
            36, 217, 174, 192, 96, 93, 218, 14, 30, 48, 0, 0, 52, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 189, 230, 60, 0, 253, 144, 1, 2, 190, 156, 254, 123, 20, 88, 24, 195, 48, 16, 27, 34,
            238, 217, 227, 219, 87, 133, 67, 94, 252, 183, 89, 97, 11, 107, 84, 135, 63, 197, 169, 164, 246, 186, 191,
            166, 101, 190, 199, 238, 38, 243, 10, 10, 211, 185, 223, 2, 112, 11, 95, 120, 212, 90, 55, 18, 54, 3, 106,
            199, 230, 129, 208, 124, 114, 128, 156, 74, 148, 135, 63, 243, 66, 237, 34, 211, 255, 174, 193, 91, 34,
            169, 44, 17, 111, 119, 244, 19, 157, 255, 192, 164, 145, 193, 200, 247, 45, 152, 249, 2, 218, 50, 179, 156,
            96, 206, 27, 237, 237, 79, 229, 192, 99, 227, 138, 198, 71, 82, 37, 123, 227, 223, 99, 113, 5, 232, 50,
            144, 250, 17, 134, 104, 162, 20, 36, 178, 112, 186, 219, 209, 71, 186, 138, 93, 104, 207, 205, 117, 177,
            12, 234, 51, 198, 4, 227, 139, 12, 37, 184, 175, 208, 85, 138, 68, 35, 250, 130, 193, 108, 134, 77, 238,
            71, 235, 79, 37, 107, 94, 152, 214, 221, 234, 198, 150, 184, 84, 187, 118, 43, 47, 247, 22, 130, 233, 188,
            237, 211, 223, 6, 5, 191, 133, 22, 239, 249, 219, 87, 117, 185, 106, 76, 1, 242, 102, 213, 117, 190, 194,
            75, 43, 87, 120, 113, 53, 58, 104, 172, 172, 54, 69, 16, 49, 249, 11, 247, 53, 244, 150, 212, 42, 166, 155,
            77, 152, 10, 151, 100, 99, 160, 26, 136, 55, 177, 218, 12, 56, 76, 32, 223, 116, 195, 82, 121, 214, 34,
            120, 245, 145, 72, 215, 45, 29, 211, 53, 53, 184, 218, 168, 216, 221, 21, 242, 127, 46, 74, 169, 52, 4,
            133, 227, 165, 68, 99, 191, 202, 99, 251, 90, 30, 26, 39, 30, 19, 255, 240, 161, 202, 38, 85, 162, 117,
            234, 185, 91, 46, 122, 143, 54, 180, 22, 197, 251, 5, 223, 62, 101, 84, 197, 31, 74, 162, 149, 41, 150,
            153, 127, 246, 103, 121, 126, 185, 118, 94, 154, 58, 87, 78, 20, 221, 65, 189, 133, 224, 81, 233, 90, 115,
            31, 20, 219, 145, 181, 38, 35, 75, 209, 57, 137, 88, 108, 185, 80, 80, 58, 71, 207, 159, 86, 247, 237, 22,
            128, 26, 149, 139, 19, 39, 166, 98, 252, 85, 212, 102, 59, 74, 147, 85, 214, 123, 4, 0, 0, 0, 178, 253,
            239, 61, 116, 130, 95, 99, 15, 6, 78, 63, 7, 121, 224, 128, 137, 4, 22, 167, 68, 80, 110, 68, 236, 176, 67,
            252, 255, 0, 0, 0, 143, 32, 179, 1, 17, 61, 20, 181, 220, 83, 124, 13, 80, 103, 224, 200, 48, 136, 231, 1,
            216, 137, 142, 36, 32, 15, 73, 120, 163, 160, 54, 187, 220, 106, 150, 246, 77, 156, 74, 126, 248, 136, 124,
            100, 155, 18, 239, 188, 174, 15, 248, 45, 212, 253, 101, 40, 112, 132, 179, 217, 252, 199, 171, 36, 228,
            174, 192, 96, 135, 3, 15, 30, 88, 0, 0, 43, 215, 99, 39, 179, 228, 254, 18, 167, 176, 63, 124, 39, 153, 70,
            243, 136, 0, 0, 0, 0, 0, 0, 0, 0, 70, 191, 188, 103, 253, 144, 1, 1, 33, 14, 212, 103, 233, 207, 193, 2,
            222, 185, 26, 211, 248, 96, 194, 179, 194, 133, 159, 239, 98, 242, 102, 89, 3, 200, 103, 19, 13, 227, 90,
            47, 83, 244, 39, 125, 114, 67, 239, 231, 239, 186, 46, 81, 18, 109, 65, 113, 214, 15, 31, 30, 220, 254,
            100, 198, 208, 82, 50, 2, 55, 49, 4, 24, 90, 165, 205, 66, 22, 236, 167, 165, 120, 230, 61, 35, 252, 120,
            195, 36, 116, 193, 241, 189, 118, 235, 119, 115, 224, 255, 161, 134, 254, 71, 206, 145, 98, 15, 2, 28, 57,
            136, 64, 79, 219, 119, 108, 177, 251, 246, 18, 234, 224, 82, 14, 164, 40, 75, 35, 123, 7, 163, 193, 172,
            30, 246, 225, 61, 201, 60, 210, 25, 64, 94, 204, 211, 211, 118, 245, 219, 197, 124, 113, 120, 147, 7, 48,
            153, 161, 39, 72, 35, 234, 111, 115, 144, 85, 114, 180, 65, 255, 103, 33, 207, 133, 221, 221, 85, 210, 214,
            237, 17, 81, 96, 43, 200, 254, 58, 238, 171, 17, 84, 132, 141, 207, 127, 244, 110, 65, 107, 174, 142, 213,
            74, 235, 115, 226, 162, 40, 7, 175, 251, 215, 224, 79, 164, 110, 123, 219, 188, 52, 178, 73, 252, 161, 139,
            141, 110, 177, 174, 215, 129, 102, 19, 18, 209, 9, 244, 229, 184, 147, 140, 68, 142, 250, 211, 132, 179,
            180, 94, 154, 205, 161, 152, 230, 245, 8, 166, 57, 17, 46, 158, 190, 223, 137, 154, 248, 39, 20, 149, 167,
            115, 8, 184, 234, 233, 131, 173, 105, 206, 233, 231, 203, 68, 30, 56, 182, 107, 25, 176, 84, 63, 44, 117,
            92, 51, 130, 201, 53, 204, 54, 236, 53, 189, 233, 243, 132, 88, 177, 12, 199, 68, 78, 33, 26, 18, 54, 59,
            60, 140, 238, 81, 133, 103, 85, 119, 142, 72, 183, 128, 50, 183, 26, 112, 94, 135, 1, 56, 145, 51, 25, 34,
            91, 29, 60, 238, 153, 49, 15, 111, 51, 203, 70, 148, 28, 185, 113, 167, 165, 17, 157, 88, 25, 39, 1, 202,
            135, 228, 231, 194, 113, 33, 208, 95, 198, 133, 180, 98, 65, 237, 171, 106, 22, 103, 58, 69, 161, 180, 219,
            130, 156, 41, 246, 183, 100, 29, 99, 223, 236, 70, 55, 102, 133, 10, 193, 145, 12, 12, 71, 4, 0, 0, 0, 47,
            173, 224, 18, 48, 16, 71, 146, 49, 87, 134, 43, 159, 255, 121, 128, 172, 114, 168, 165, 20, 98, 14, 17,
            179, 81, 27, 240, 27, 13, 0, 0, 87, 102, 111, 48, 206, 0, 179, 77, 134, 192, 200, 112, 46, 38, 60, 235, 38,
            157, 219, 13, 239, 113, 234, 204, 237, 71, 180, 194, 232, 95, 23, 61, 220, 106, 150, 246, 77, 156, 74, 126,
            248, 136, 124, 100, 155, 18, 239, 188, 174, 15, 248, 45, 212, 253, 101, 40, 112, 132, 179, 217, 252, 199,
            171, 36, 133, 175, 192, 96, 242, 16, 15, 30, 127, 255, 255, 235, 132, 38, 62, 247, 152, 251, 12, 82, 62,
            40, 198, 34, 24, 174, 133, 125, 0, 0, 0, 0, 0, 0, 0, 0, 78, 134, 7, 7, 253, 144, 1, 0, 93, 219, 187, 68,
            202, 196, 93, 12, 72, 165, 72, 129, 151, 212, 172, 205, 175, 161, 242, 240, 219, 0, 187, 99, 50, 52, 90,
            73, 84, 240, 206, 147, 204, 18, 42, 108, 184, 157, 37, 110, 165, 141, 102, 209, 115, 101, 148, 163, 55, 21,
            79, 113, 36, 23, 197, 237, 143, 93, 26, 225, 130, 85, 220, 153, 123, 65, 225, 186, 233, 248, 207, 232, 98,
            50, 22, 222, 58, 190, 74, 17, 12, 235, 220, 152, 57, 100, 131, 52, 234, 122, 188, 112, 61, 234, 142, 73,
            227, 13, 165, 6, 140, 99, 209, 146, 59, 83, 222, 230, 57, 118, 47, 66, 77, 250, 201, 191, 18, 124, 231,
            154, 215, 68, 108, 154, 20, 69, 124, 244, 111, 1, 135, 30, 172, 67, 165, 219, 210, 142, 210, 242, 47, 202,
            136, 196, 227, 93, 189, 134, 80, 21, 220, 233, 38, 147, 107, 213, 146, 202, 123, 217, 182, 132, 211, 193,
            194, 196, 241, 241, 218, 83, 79, 121, 152, 213, 25, 62, 93, 208, 204, 136, 140, 183, 128, 44, 146, 96, 210,
            108, 11, 118, 246, 79, 37, 110, 128, 141, 254, 208, 249, 2, 69, 226, 143, 165, 85, 178, 46, 124, 187, 97,
            230, 196, 7, 180, 99, 209, 4, 150, 34, 185, 129, 211, 132, 149, 14, 36, 218, 30, 251, 113, 202, 159, 83,
            14, 235, 28, 201, 74, 23, 231, 255, 212, 79, 8, 166, 133, 231, 235, 45, 3, 11, 154, 38, 93, 213, 48, 59,
            40, 121, 14, 84, 234, 18, 119, 134, 20, 86, 110, 217, 45, 21, 171, 107, 169, 48, 255, 153, 97, 112, 41,
            229, 59, 152, 220, 171, 115, 56, 120, 89, 223, 93, 208, 30, 85, 109, 95, 74, 129, 224, 3, 31, 156, 136,
            204, 215, 242, 108, 173, 95, 227, 105, 112, 175, 106, 155, 226, 212, 140, 182, 192, 169, 8, 141, 177, 8,
            111, 201, 55, 87, 93, 20, 49, 137, 95, 221, 215, 1, 113, 136, 70, 201, 100, 221, 94, 158, 90, 254, 72, 51,
            48, 243, 102, 244, 18, 110, 18, 47, 171, 202, 27, 116, 70, 26, 67, 157, 251, 88, 6, 24, 223, 141, 207, 150,
            145, 55, 156, 237, 222, 192, 213, 164, 227, 229, 189, 39, 119, 182, 186, 140, 6, 64, 82, 17, 240, 180, 117,
            140, 57, 90, 4, 0, 0, 0, 252, 211, 32, 165, 184, 226, 151, 110, 53, 250, 221, 144, 217, 235, 21, 221, 135,
            82, 29, 148, 28, 56, 104, 179, 204, 74, 102, 55, 227, 7, 0, 0, 231, 155, 54, 126, 46, 9, 199, 26, 14, 92,
            141, 133, 187, 66, 112, 231, 212, 3, 211, 127, 158, 7, 165, 199, 198, 94, 22, 235, 46, 36, 35, 199, 220,
            106, 150, 246, 77, 156, 74, 126, 248, 136, 124, 100, 155, 18, 239, 188, 174, 15, 248, 45, 212, 253, 101,
            40, 112, 132, 179, 217, 252, 199, 171, 36, 217, 175, 192, 96, 126, 245, 14, 30, 23, 255, 255, 254, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 101, 196, 76, 0, 253, 144, 1, 0, 171, 202,
            230, 126, 247, 220, 113, 208, 23, 6, 9, 241, 176, 16, 154, 204, 130, 77, 30, 66, 35, 29, 13, 112, 24, 36,
            57, 196, 241, 175, 6, 148, 210, 44, 123, 21, 45, 84, 192, 30, 182, 21, 75, 88, 223, 197, 212, 255, 173, 24,
            24, 235, 195, 110, 17, 154, 92, 140, 152, 204, 155, 101, 192, 249, 14, 7, 231, 17, 163, 188, 43, 64, 181,
            245, 38, 62, 167, 26, 38, 206, 218, 211, 230, 141, 190, 41, 40, 21, 180, 174, 229, 84, 7, 55, 42, 91, 230,
            253, 122, 6, 158, 9, 240, 190, 227, 10, 21, 212, 44, 60, 48, 102, 232, 105, 206, 29, 17, 53, 245, 128, 243,
            102, 238, 172, 60, 63, 217, 118, 156, 1, 178, 137, 73, 220, 136, 97, 149, 22, 181, 230, 137, 221, 51, 8,
            153, 69, 230, 15, 219, 12, 234, 58, 241, 211, 130, 208, 206, 198, 176, 85, 87, 68, 83, 130, 179, 158, 189,
            33, 132, 242, 96, 226, 74, 6, 17, 17, 33, 121, 222, 253, 175, 153, 91, 158, 134, 219, 37, 143, 142, 166,
            183, 182, 46, 103, 248, 27, 191, 149, 217, 1, 108, 227, 53, 191, 119, 3, 176, 47, 131, 150, 147, 242, 106,
            34, 137, 102, 150, 187, 103, 89, 1, 181, 192, 212, 15, 212, 85, 150, 65, 54, 175, 95, 194, 89, 128, 53,
            194, 71, 116, 134, 191, 95, 123, 93, 167, 161, 196, 221, 105, 20, 243, 156, 80, 50, 184, 140, 49, 115, 250,
            4, 92, 179, 152, 244, 27, 194, 123, 46, 188, 24, 103, 193, 182, 133, 28, 141, 195, 175, 65, 171, 12, 172,
            222, 217, 195, 30, 249, 98, 209, 181, 229, 195, 159, 96, 33, 155, 247, 102, 65, 14, 121, 103, 148, 105,
            255, 81, 161, 94, 219, 141, 13, 72, 27, 10, 150, 187, 67, 94, 25, 215, 225, 137, 119, 104, 88, 161, 101,
            73, 80, 116, 107, 120, 118, 53, 208, 211, 8, 16, 185, 76, 115, 12, 210, 106, 152, 183, 168, 243, 189, 16,
            208, 24, 26, 30, 73, 77, 62, 113, 223, 92, 214, 241, 194, 33, 104, 249, 81, 37, 174, 235, 255, 151, 182,
            217, 32, 161, 122, 60, 57, 208, 175, 224, 149, 253, 156, 91, 114, 115, 150, 124, 72, 55, 165, 3, 5, 46,
            166, 81, 146,
        ];
        let mut reader = Reader::new(headers_bytes);
        let headers = reader.read_list::<BlockHeader>().unwrap();
        for header in headers.iter() {
            assert_eq!(header.version, 4);
        }
        let serialized = serialize_list(&headers);
        assert_eq!(serialized.take(), headers_bytes);
    }

    #[test]
    fn test_rvn_block_headers_serde_11() {
        let headers_bytes = &[
            11, 0, 0, 0, 48, 237, 150, 167, 79, 155, 50, 15, 136, 236, 70, 161, 176, 214, 64, 229, 229, 69, 183, 232,
            114, 246, 196, 59, 43, 5, 30, 0, 0, 0, 0, 0, 0, 213, 226, 41, 199, 180, 74, 252, 32, 38, 127, 106, 226, 60,
            19, 160, 217, 103, 138, 154, 182, 22, 6, 83, 101, 244, 235, 5, 165, 48, 131, 231, 126, 93, 9, 100, 97, 82,
            134, 0, 27, 238, 7, 30, 0, 213, 196, 74, 140, 0, 0, 64, 138, 1, 178, 161, 34, 211, 170, 42, 102, 56, 162,
            29, 192, 168, 197, 48, 60, 137, 52, 29, 42, 185, 182, 115, 85, 228, 232, 128, 249, 4, 50, 212, 59, 0, 0, 0,
            48, 215, 115, 197, 24, 96, 121, 142, 147, 192, 42, 215, 30, 163, 100, 98, 50, 33, 34, 149, 193, 243, 25,
            226, 146, 225, 62, 0, 0, 0, 0, 0, 0, 163, 244, 11, 42, 83, 181, 125, 55, 236, 41, 192, 30, 84, 70, 222, 1,
            85, 75, 26, 240, 241, 220, 43, 248, 229, 75, 104, 96, 149, 43, 206, 171, 178, 9, 100, 97, 183, 134, 0, 27,
            239, 7, 30, 0, 49, 18, 15, 137, 126, 56, 125, 2, 82, 251, 19, 97, 61, 193, 112, 9, 7, 202, 128, 151, 140,
            4, 46, 123, 188, 176, 164, 36, 44, 4, 112, 82, 189, 192, 231, 193, 214, 167, 205, 68, 0, 0, 0, 48, 172, 16,
            240, 170, 92, 180, 66, 142, 172, 209, 252, 59, 43, 74, 221, 218, 66, 193, 86, 182, 97, 187, 153, 39, 1, 82,
            0, 0, 0, 0, 0, 0, 107, 145, 230, 170, 222, 8, 38, 115, 116, 60, 191, 244, 130, 145, 210, 1, 122, 78, 155,
            225, 216, 163, 173, 79, 216, 10, 13, 147, 148, 123, 145, 94, 37, 10, 100, 97, 160, 135, 0, 27, 240, 7, 30,
            0, 225, 144, 195, 113, 185, 3, 106, 6, 54, 64, 2, 182, 116, 186, 49, 67, 172, 71, 14, 167, 164, 142, 29,
            62, 29, 255, 205, 151, 246, 169, 138, 85, 146, 11, 96, 235, 81, 190, 61, 172, 0, 0, 0, 48, 108, 91, 255,
            188, 45, 22, 67, 122, 118, 145, 114, 92, 29, 168, 138, 138, 222, 251, 142, 100, 107, 25, 135, 113, 83, 31,
            0, 0, 0, 0, 0, 0, 88, 78, 209, 50, 65, 171, 24, 74, 139, 176, 177, 177, 124, 126, 238, 247, 34, 89, 65, 97,
            176, 3, 73, 189, 158, 144, 120, 153, 218, 254, 174, 65, 49, 10, 100, 97, 161, 136, 0, 27, 241, 7, 30, 0,
            236, 0, 233, 111, 0, 0, 0, 254, 36, 60, 183, 188, 193, 129, 108, 114, 227, 173, 116, 70, 77, 161, 86, 209,
            128, 186, 238, 127, 98, 133, 78, 160, 182, 26, 0, 2, 163, 137, 140, 52, 0, 0, 0, 48, 0, 214, 104, 7, 144,
            180, 18, 98, 90, 210, 99, 155, 247, 68, 91, 222, 255, 135, 33, 83, 60, 54, 182, 79, 96, 77, 0, 0, 0, 0, 0,
            0, 224, 117, 225, 217, 60, 255, 146, 85, 51, 155, 218, 221, 109, 239, 1, 73, 128, 231, 68, 17, 221, 97,
            177, 186, 234, 105, 213, 87, 103, 88, 243, 70, 60, 10, 100, 97, 132, 135, 0, 27, 242, 7, 30, 0, 143, 12,
            128, 48, 121, 113, 61, 15, 14, 39, 27, 235, 62, 75, 73, 138, 60, 154, 168, 121, 196, 136, 255, 132, 244,
            189, 208, 0, 81, 142, 93, 202, 166, 206, 133, 96, 200, 216, 67, 207, 0, 0, 0, 48, 59, 251, 204, 23, 121,
            110, 167, 208, 122, 42, 29, 211, 246, 163, 149, 252, 197, 240, 162, 163, 14, 209, 59, 48, 85, 2, 0, 0, 0,
            0, 0, 0, 28, 87, 85, 39, 100, 124, 112, 104, 88, 14, 163, 135, 199, 136, 218, 194, 135, 83, 171, 230, 74,
            215, 92, 194, 114, 99, 124, 70, 243, 211, 175, 83, 152, 10, 100, 97, 11, 135, 0, 27, 243, 7, 30, 0, 60, 52,
            76, 6, 0, 149, 204, 228, 222, 161, 141, 90, 216, 210, 203, 5, 93, 69, 187, 163, 7, 202, 28, 107, 249, 238,
            237, 225, 129, 141, 114, 11, 249, 147, 197, 5, 37, 180, 91, 234, 0, 0, 0, 48, 4, 220, 253, 53, 17, 79, 68,
            222, 146, 191, 217, 94, 137, 210, 233, 63, 254, 66, 114, 137, 85, 218, 30, 252, 33, 104, 0, 0, 0, 0, 0, 0,
            51, 168, 163, 46, 216, 31, 219, 22, 89, 182, 12, 95, 27, 212, 59, 33, 106, 106, 248, 12, 134, 109, 146, 54,
            53, 218, 181, 132, 90, 198, 199, 32, 183, 10, 100, 97, 37, 136, 0, 27, 244, 7, 30, 0, 109, 89, 178, 91,
            175, 69, 55, 108, 47, 22, 177, 124, 201, 92, 9, 158, 150, 198, 75, 171, 194, 215, 175, 42, 210, 241, 60,
            219, 141, 148, 44, 12, 75, 100, 136, 203, 161, 185, 137, 143, 0, 0, 0, 48, 170, 242, 251, 214, 160, 130,
            252, 189, 231, 235, 159, 158, 146, 129, 248, 248, 132, 144, 96, 215, 205, 196, 3, 113, 247, 36, 0, 0, 0, 0,
            0, 0, 37, 148, 126, 160, 112, 48, 102, 220, 19, 76, 146, 122, 235, 121, 120, 110, 166, 144, 166, 156, 10,
            81, 224, 113, 76, 127, 93, 118, 194, 86, 108, 1, 3, 11, 100, 97, 173, 135, 0, 27, 245, 7, 30, 0, 211, 118,
            38, 66, 121, 111, 246, 1, 178, 56, 37, 235, 22, 71, 185, 12, 165, 37, 181, 149, 22, 158, 132, 180, 139,
            205, 172, 165, 158, 129, 183, 15, 152, 124, 248, 127, 127, 50, 74, 199, 0, 0, 0, 48, 156, 23, 51, 157, 98,
            189, 191, 169, 41, 39, 153, 187, 167, 179, 67, 243, 30, 51, 179, 150, 148, 215, 235, 192, 170, 122, 0, 0,
            0, 0, 0, 0, 104, 241, 21, 82, 116, 136, 197, 212, 102, 160, 80, 192, 124, 186, 59, 237, 122, 207, 80, 38,
            72, 244, 184, 217, 179, 22, 99, 171, 111, 203, 12, 139, 15, 11, 100, 97, 149, 136, 0, 27, 246, 7, 30, 0,
            225, 188, 27, 190, 89, 0, 236, 88, 223, 66, 178, 12, 193, 214, 213, 132, 39, 245, 34, 48, 154, 26, 217,
            196, 181, 90, 62, 87, 184, 96, 170, 84, 168, 163, 5, 69, 74, 103, 180, 26, 0, 0, 0, 48, 100, 220, 213, 41,
            169, 96, 184, 160, 46, 26, 57, 41, 80, 129, 187, 169, 209, 137, 85, 4, 222, 222, 164, 106, 14, 9, 0, 0, 0,
            0, 0, 0, 121, 57, 108, 229, 242, 102, 227, 179, 193, 186, 171, 143, 86, 24, 111, 217, 67, 79, 3, 215, 102,
            20, 54, 238, 215, 71, 37, 108, 190, 43, 21, 69, 115, 11, 100, 97, 118, 136, 0, 27, 247, 7, 30, 0, 205, 59,
            255, 220, 99, 0, 149, 106, 82, 62, 64, 75, 70, 12, 184, 92, 216, 85, 154, 188, 207, 36, 51, 39, 59, 73, 83,
            255, 148, 170, 168, 22, 23, 174, 156, 54, 12, 103, 82, 248, 0, 0, 0, 48, 25, 234, 210, 117, 105, 129, 40,
            237, 127, 97, 12, 185, 242, 7, 108, 71, 11, 164, 38, 152, 146, 104, 73, 208, 244, 86, 0, 0, 0, 0, 0, 0,
            240, 151, 209, 102, 224, 19, 99, 121, 183, 69, 10, 98, 12, 152, 95, 5, 26, 70, 130, 95, 19, 178, 214, 197,
            38, 169, 86, 15, 54, 136, 89, 43, 154, 11, 100, 97, 118, 137, 0, 27, 248, 7, 30, 0, 196, 214, 13, 83, 49,
            111, 94, 161, 227, 50, 66, 9, 101, 130, 70, 156, 72, 83, 164, 129, 34, 31, 162, 78, 59, 133, 79, 177, 46,
            252, 71, 214, 56, 220, 173, 79, 220, 196, 15, 211,
        ];

        let mut reader = Reader::new_with_coin_variant(headers_bytes, CoinVariant::RVN);
        let headers = reader.read_list::<BlockHeader>().unwrap();
        for header in headers.iter() {
            assert_eq!(header.version, KAWPOW_VERSION);
        }
        let serialized = serialize_list(&headers);
        assert_eq!(serialized.take(), headers_bytes);
    }

    #[test]
    fn test_btc_v4_block_headers_serde_11() {
        // https://live.blockcypher.com/btc/block/0000000000000000097336f8439779072501753e2f48b8798c66188139f2d9cf/
        let header = "04000000462a79dfa51b541648ee55df74cdc14b9ea7feb932e912060000000000000000374c1707a72691be50070bc5029d586e9200d672c6c3dfd29d267bf6b2b01b9e0ace395654a91118923bd9d5";
        let header_bytes = &header.from_hex::<Vec<u8>>().unwrap() as &[u8];
        let mut reader = Reader::new_with_coin_variant(header_bytes, CoinVariant::BTC);
        let header = reader.read::<BlockHeader>().unwrap();
        assert_eq!(header.version, 4);
        let serialized = serialize(&header);
        assert_eq!(serialized.take(), header_bytes);
    }

    #[test]
    fn test_btc_kow_pow_version_block_headers_serde_11() {
        // https://live.blockcypher.com/btc/block/000000000000000006e35d6675fb0fec767a5f3b346261a5160f6e2a8d258070/
        let header = "00000030af7e7389ca428b05d8902fcdc148e70974524d39cb56bc0100000000000000007ce0cd0c9c648d1b585d29b9ab23ebc987619d43925b3c768d7cb4bc097cfb821441c05614a107187aef1ee1";
        let header_bytes = &header.from_hex::<Vec<u8>>().unwrap() as &[u8];
        let mut reader = Reader::new_with_coin_variant(header_bytes, CoinVariant::BTC);
        let header = reader.read::<BlockHeader>().unwrap();
        assert_eq!(header.version, KAWPOW_VERSION);
        let serialized = serialize(&header);
        assert_eq!(serialized.take(), header_bytes);
    }

    #[test]
    fn test_bch_kow_pow_version_header() {
        // https://explorer.bitcoinunlimited.info/block/000000000000000001fa10bec90256a006203df5347de869d6500814f27668f1
        let header = "0000003075135a61aad9ebe8db6a4e9c5e3ee6febd92a6788ee1860100000000000000007098ff568e4c66422bc62b5e3106fc16c08f64c91d7c6d5cd0519f1fade0d7332106286694a10218f925a695";
        let header_bytes = &header.from_hex::<Vec<u8>>().unwrap() as &[u8];
        let mut reader = Reader::new_with_coin_variant(header_bytes, CoinVariant::BTC);
        let header = reader.read::<BlockHeader>().unwrap();
        assert_eq!(header.version, KAWPOW_VERSION);
        assert_eq!(
            header.hash().reversed().to_string(),
            "000000000000000001fa10bec90256a006203df5347de869d6500814f27668f1"
        );
        let serialized = serialize(&header);
        assert_eq!(serialized.take(), header_bytes);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn test_from_blockheader_to_ext_blockheader() {
        // https://live.blockcypher.com/btc/block/00000000000000000020cf2bdc6563fb25c424af588d5fb7223461e72715e4a9/
        let header: BlockHeader = "0200000066720b99e07d284bd4fe67ff8c49a5db1dd8514fcdab610000000000000000007829844f4c3a41a537b3131ca992643eaa9d093b2383e4cdc060ad7dc548118751eb505ac1910018de19b302".into();
        let ext_header = ExtBlockHeader::from(header.clone());
        assert_eq!(
            header.hash().reversed().to_string(),
            ext_header.block_hash().to_string()
        );
    }

    #[test]
    fn test_ppc_block_headers() {
        // PeerCoin block 659052 has version 4, but it doesn't use Zcash format
        // https://chainz.cryptoid.info/ppc/block.dws?5a25b9f21589539d55e1ddf7c103fe8e466ce54f79054a0991943ba43651110f.htm
        let serialized_headers = [
            11, 3, 0, 0, 0, 170, 140, 242, 85, 87, 6, 202, 247, 184, 135, 155, 111, 244, 7, 250, 105, 75, 131, 174, 4,
            68, 10, 106, 137, 90, 28, 4, 96, 181, 143, 104, 84, 73, 165, 123, 22, 178, 86, 249, 78, 33, 244, 192, 200,
            2, 183, 125, 71, 54, 15, 96, 186, 191, 46, 215, 152, 60, 167, 94, 166, 182, 242, 94, 107, 43, 41, 162, 99,
            91, 235, 12, 28, 0, 0, 0, 0, 3, 0, 0, 0, 60, 147, 16, 89, 128, 81, 253, 79, 190, 212, 190, 221, 218, 157,
            229, 16, 245, 67, 18, 1, 66, 18, 223, 157, 166, 112, 129, 243, 52, 159, 49, 220, 232, 7, 49, 243, 79, 108,
            233, 206, 255, 34, 84, 189, 216, 195, 24, 51, 82, 26, 120, 248, 241, 209, 149, 58, 197, 242, 247, 155, 4,
            107, 232, 40, 3, 43, 162, 99, 110, 241, 12, 28, 0, 0, 0, 0, 3, 0, 244, 1, 61, 94, 238, 109, 77, 96, 208,
            33, 232, 235, 188, 45, 37, 209, 123, 164, 70, 208, 9, 90, 255, 232, 106, 210, 145, 159, 90, 139, 194, 33,
            48, 83, 36, 246, 212, 171, 113, 69, 163, 220, 189, 156, 20, 156, 170, 75, 50, 63, 213, 108, 106, 210, 166,
            132, 73, 223, 30, 33, 34, 172, 100, 195, 96, 223, 111, 44, 162, 99, 42, 227, 0, 25, 104, 157, 248, 189, 3,
            0, 0, 0, 158, 127, 227, 143, 52, 181, 82, 70, 130, 219, 103, 4, 12, 154, 127, 247, 44, 118, 114, 147, 175,
            128, 70, 195, 0, 0, 0, 0, 0, 0, 0, 0, 25, 21, 28, 189, 23, 44, 136, 69, 26, 212, 166, 23, 21, 62, 51, 232,
            27, 89, 127, 183, 48, 140, 50, 189, 16, 112, 169, 208, 253, 253, 226, 202, 121, 46, 162, 99, 7, 240, 12,
            28, 0, 0, 0, 0, 3, 0, 0, 0, 119, 48, 233, 42, 176, 54, 147, 19, 154, 100, 166, 71, 125, 179, 17, 228, 237,
            97, 166, 70, 104, 199, 231, 5, 213, 86, 190, 233, 96, 123, 71, 205, 89, 169, 110, 206, 201, 104, 90, 164,
            242, 7, 167, 8, 97, 48, 192, 154, 91, 243, 131, 128, 49, 237, 132, 250, 244, 76, 87, 125, 244, 80, 89, 159,
            140, 49, 162, 99, 40, 243, 12, 28, 0, 0, 0, 0, 3, 0, 0, 0, 96, 162, 150, 174, 178, 72, 227, 162, 16, 64,
            16, 68, 13, 184, 177, 217, 9, 29, 8, 238, 106, 37, 230, 205, 211, 113, 255, 234, 160, 249, 57, 175, 165,
            63, 215, 82, 194, 170, 131, 11, 206, 154, 94, 196, 150, 229, 231, 34, 194, 229, 69, 211, 54, 121, 189, 212,
            80, 3, 36, 171, 133, 237, 170, 87, 15, 53, 162, 99, 52, 245, 12, 28, 0, 0, 0, 0, 3, 0, 0, 0, 70, 5, 106,
            135, 225, 93, 205, 85, 243, 111, 204, 51, 76, 26, 188, 4, 66, 146, 196, 202, 107, 173, 22, 155, 40, 88,
            250, 240, 228, 200, 72, 65, 152, 164, 228, 76, 113, 62, 236, 192, 36, 75, 188, 13, 230, 207, 213, 216, 249,
            119, 94, 3, 149, 212, 25, 244, 160, 188, 121, 223, 96, 67, 70, 149, 45, 53, 162, 99, 122, 248, 12, 28, 0,
            0, 0, 0, 3, 0, 0, 0, 251, 222, 167, 109, 77, 158, 185, 21, 151, 167, 84, 246, 45, 202, 184, 247, 174, 45,
            233, 187, 120, 144, 135, 102, 230, 252, 243, 183, 211, 86, 136, 112, 121, 79, 60, 217, 208, 58, 95, 249,
            230, 74, 182, 253, 231, 46, 206, 48, 11, 43, 251, 60, 166, 119, 192, 121, 226, 125, 113, 136, 150, 192, 4,
            77, 231, 53, 162, 99, 57, 242, 12, 28, 0, 0, 0, 0, 3, 0, 0, 0, 222, 159, 64, 62, 117, 52, 203, 195, 40, 60,
            183, 13, 66, 177, 106, 251, 41, 145, 128, 156, 141, 166, 128, 89, 161, 124, 153, 194, 1, 198, 153, 255, 43,
            16, 241, 125, 110, 84, 130, 214, 2, 235, 242, 190, 229, 210, 21, 65, 114, 23, 255, 58, 252, 35, 107, 16,
            194, 22, 98, 9, 125, 223, 242, 236, 74, 56, 162, 99, 176, 237, 12, 28, 0, 0, 0, 0, 3, 0, 0, 0, 216, 24,
            144, 128, 67, 125, 152, 252, 140, 131, 29, 130, 32, 94, 202, 17, 221, 161, 250, 195, 8, 211, 36, 43, 144,
            152, 52, 113, 155, 71, 225, 7, 78, 158, 15, 190, 68, 106, 75, 245, 149, 116, 42, 233, 64, 193, 140, 100,
            117, 43, 232, 172, 120, 75, 39, 111, 184, 74, 205, 69, 246, 191, 7, 231, 251, 56, 162, 99, 206, 237, 12,
            28, 0, 0, 0, 0, 4, 0, 0, 0, 233, 77, 140, 50, 73, 4, 68, 207, 105, 49, 245, 61, 211, 154, 138, 215, 242,
            181, 232, 126, 200, 159, 164, 87, 64, 192, 115, 48, 54, 13, 132, 225, 158, 218, 135, 111, 168, 192, 173,
            53, 161, 162, 247, 132, 187, 50, 235, 188, 174, 70, 185, 245, 211, 141, 119, 79, 178, 153, 254, 11, 140,
            176, 126, 250, 146, 57, 162, 99, 45, 233, 12, 28, 0, 0, 0, 0,
        ];
        let mut reader = Reader::new_with_coin_variant(&serialized_headers, CoinVariant::PPC);
        let headers = reader.read_list::<BlockHeader>().unwrap();
        let serialized_from_deserialized = serialize_list(&headers);
        assert_eq!(serialized_from_deserialized.take(), serialized_headers);
    }

    #[test]
    fn test_rick_v1_block_header_des() {
        // RICK Block header 0 bytes.
        // https://rick.explorer.dexstats.info/block/027e3758c3a65b12aa1046462b486d0a63bfa1beae327897f56c5cfb7daaae71
        let header_bytes = [
            1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            59, 163, 237, 253, 122, 123, 18, 178, 122, 199, 44, 62, 103, 118, 143, 97, 127, 200, 27, 195, 136, 138, 81,
            50, 58, 159, 184, 170, 75, 30, 94, 74, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 41, 171, 95, 73, 15, 15, 15, 32, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 253, 64, 5, 0, 13, 91, 167, 205, 165, 212, 115, 148,
            114, 99, 191, 25, 66, 133, 49, 113, 121, 210, 176, 211, 7, 17, 156, 46, 124, 196, 189, 138, 196, 86, 240,
            119, 75, 213, 43, 12, 217, 36, 155, 233, 212, 7, 24, 182, 57, 122, 76, 123, 189, 143, 43, 50, 114, 254,
            210, 130, 60, 210, 175, 75, 209, 99, 34, 0, 186, 75, 247, 150, 114, 125, 99, 71, 178, 37, 246, 112, 242,
            146, 52, 50, 116, 204, 53, 9, 148, 102, 245, 251, 95, 12, 209, 193, 5, 18, 27, 40, 33, 61, 21, 219, 46,
            215, 189, 186, 73, 11, 76, 237, 198, 151, 66, 165, 123, 124, 37, 175, 36, 72, 94, 82, 58, 173, 187, 119,
            160, 20, 79, 199, 111, 121, 239, 115, 189, 133, 48, 212, 43, 159, 59, 155, 237, 28, 19, 90, 209, 254, 21,
            41, 35, 250, 254, 152, 249, 95, 118, 241, 97, 94, 100, 196, 171, 177, 19, 127, 76, 49, 178, 24, 186, 39,
            130, 188, 21, 83, 71, 136, 221, 162, 204, 8, 160, 238, 41, 135, 200, 178, 127, 244, 27, 212, 227, 28, 213,
            251, 86, 67, 223, 232, 98, 201, 160, 44, 169, 249, 12, 140, 81, 166, 103, 29, 104, 29, 4, 173, 71, 228,
            181, 59, 21, 24, 212, 190, 250, 254, 254, 140, 173, 251, 145, 47, 61, 3, 5, 27, 30, 251, 241, 223, 227,
            123, 86, 233, 58, 116, 29, 141, 253, 128, 213, 118, 202, 37, 11, 238, 85, 250, 177, 49, 31, 199, 179, 37,
            89, 119, 85, 140, 221, 166, 247, 214, 248, 117, 48, 110, 67, 161, 68, 19, 250, 205, 174, 210, 244, 96, 147,
            224, 239, 30, 143, 138, 150, 62, 22, 50, 220, 190, 235, 216, 228, 159, 209, 107, 87, 212, 155, 8, 249, 118,
            45, 232, 145, 87, 198, 82, 51, 246, 12, 142, 56, 161, 245, 3, 164, 140, 85, 95, 142, 196, 93, 237, 236,
            213, 116, 163, 118, 1, 50, 60, 39, 190, 89, 123, 149, 99, 67, 16, 127, 139, 216, 15, 58, 146, 90, 250, 243,
            8, 17, 223, 131, 196, 2, 17, 107, 185, 193, 229, 35, 28, 112, 255, 248, 153, 167, 200, 47, 115, 201, 2,
            186, 84, 218, 83, 204, 69, 155, 123, 241, 17, 61, 182, 92, 200, 246, 145, 77, 54, 24, 86, 14, 166, 154,
            189, 19, 101, 143, 167, 182, 175, 146, 211, 116, 214, 236, 169, 82, 159, 139, 213, 101, 22, 110, 79, 203,
            242, 168, 223, 179, 201, 182, 149, 57, 212, 210, 238, 46, 147, 33, 184, 91, 51, 25, 37, 223, 25, 89, 21,
            242, 117, 118, 55, 194, 128, 94, 29, 65, 49, 225, 173, 158, 249, 188, 27, 177, 199, 50, 216, 219, 164, 115,
            135, 22, 211, 81, 171, 48, 201, 150, 200, 101, 123, 171, 57, 86, 126, 227, 178, 156, 109, 5, 75, 113, 20,
            149, 192, 213, 46, 28, 213, 216, 229, 91, 79, 15, 3, 37, 185, 115, 105, 40, 7, 85, 180, 106, 2, 175, 213,
            75, 228, 221, 217, 247, 124, 34, 39, 43, 139, 187, 23, 255, 81, 24, 254, 219, 174, 37, 100, 82, 78, 121,
            123, 210, 139, 95, 116, 247, 7, 157, 83, 44, 204, 5, 152, 7, 152, 159, 148, 210, 103, 244, 126, 114, 75,
            63, 30, 207, 224, 14, 201, 230, 84, 28, 150, 16, 128, 216, 137, 18, 81, 184, 75, 68, 128, 188, 41, 47, 106,
            24, 11, 234, 8, 159, 239, 91, 189, 165, 110, 30, 65, 57, 13, 124, 14, 133, 186, 14, 245, 48, 247, 23, 116,
            19, 72, 26, 34, 100, 101, 163, 110, 246, 175, 225, 226, 188, 166, 157, 32, 120, 113, 43, 57, 18, 187, 161,
            169, 155, 31, 191, 240, 211, 85, 214, 255, 231, 38, 210, 187, 111, 188, 16, 60, 74, 197, 117, 110, 91, 238,
            110, 71, 225, 116, 36, 235, 203, 241, 182, 61, 140, 185, 12, 226, 228, 1, 152, 180, 244, 25, 134, 137, 218,
            234, 37, 67, 7, 229, 42, 37, 86, 47, 76, 20, 85, 52, 15, 15, 254, 177, 15, 157, 142, 145, 71, 117, 227,
            125, 14, 220, 160, 25, 251, 27, 156, 110, 248, 18, 85, 237, 134, 188, 81, 197, 57, 30, 5, 145, 72, 15, 102,
            226, 216, 140, 95, 79, 215, 39, 118, 151, 150, 134, 86, 169, 177, 19, 171, 151, 248, 116, 253, 213, 242,
            70, 94, 85, 89, 83, 62, 1, 186, 19, 239, 74, 143, 122, 33, 208, 44, 48, 200, 222, 214, 142, 140, 84, 96,
            58, 185, 200, 8, 78, 246, 217, 235, 78, 146, 199, 91, 7, 133, 57, 226, 174, 120, 110, 186, 182, 218, 183,
            58, 9, 224, 170, 154, 197, 117, 188, 239, 178, 158, 147, 10, 230, 86, 229, 139, 203, 81, 63, 126, 60, 23,
            224, 121, 220, 228, 240, 91, 93, 188, 24, 194, 168, 114, 178, 37, 9, 116, 14, 190, 106, 57, 3, 224, 10,
            209, 171, 197, 80, 118, 68, 24, 98, 100, 63, 147, 96, 110, 61, 195, 94, 141, 159, 44, 174, 243, 238, 107,
            225, 77, 81, 59, 46, 6, 43, 33, 208, 6, 29, 227, 189, 86, 136, 23, 19, 161, 165, 193, 127, 90, 206, 5, 225,
            236, 9, 218, 83, 249, 148, 66, 223, 23, 90, 73, 189, 21, 74, 169, 110, 73, 73, 222, 205, 82, 254, 215, 156,
            207, 124, 203, 206, 50, 148, 20, 25, 195, 20, 227, 116, 228, 163, 150, 172, 85, 62, 23, 181, 52, 3, 54,
            161, 162, 92, 34, 249, 228, 42, 36, 59, 165, 64, 68, 80, 182, 80, 172, 252, 130, 106, 110, 67, 41, 113,
            172, 231, 118, 225, 87, 25, 81, 94, 22, 52, 206, 185, 164, 163, 80, 97, 182, 104, 199, 73, 152, 211, 223,
            181, 130, 127, 98, 56, 236, 1, 83, 119, 230, 249, 201, 79, 56, 16, 135, 104, 207, 110, 92, 139, 19, 46, 3,
            3, 251, 90, 32, 3, 104, 248, 69, 173, 157, 70, 52, 48, 53, 166, 255, 148, 3, 29, 248, 216, 48, 148, 21,
            187, 63, 108, 213, 237, 233, 193, 53, 253, 171, 204, 3, 5, 153, 133, 141, 128, 60, 15, 133, 190, 118, 97,
            200, 137, 132, 216, 143, 170, 61, 38, 251, 14, 154, 172, 0, 86, 165, 63, 27, 93, 11, 174, 215, 19, 200, 83,
            196, 162, 114, 104, 105, 160, 161, 36, 168, 165, 187, 192, 252, 14, 248, 12, 138, 228, 203, 83, 99, 106,
            160, 37, 3, 184, 106, 30, 185, 131, 111, 204, 37, 152, 35, 226, 105, 45, 146, 29, 136, 225, 255, 193, 230,
            203, 43, 222, 67, 147, 156, 235, 63, 50, 166, 17, 104, 111, 83, 159, 143, 124, 159, 11, 240, 3, 129, 247,
            67, 96, 125, 64, 150, 15, 6, 211, 71, 209, 205, 138, 200, 165, 25, 105, 194, 94, 55, 21, 14, 253, 247, 170,
            76, 32, 55, 162, 253, 5, 22, 251, 68, 69, 37, 171, 21, 122, 14, 208, 167, 65, 43, 47, 166, 155, 33, 127,
            227, 151, 38, 49, 83, 120, 44, 15, 100, 53, 31, 189, 242, 103, 143, 160, 220, 133, 105, 145, 45, 205, 142,
            60, 202, 211, 143, 52, 242, 59, 187, 206, 20, 198, 162, 106, 194, 73, 17, 179, 8, 184, 44, 126, 67, 6, 45,
            24, 11, 174, 172, 75, 167, 21, 56, 88, 54, 92, 114, 198, 61, 207, 95, 106, 91, 8, 7, 11, 115, 10, 219, 1,
            122, 234, 233, 37, 183, 208, 67, 153, 121, 226, 103, 159, 69, 237, 47, 37, 167, 237, 207, 210, 251, 119,
            168, 121, 70, 48, 40, 92, 203, 10, 7, 31, 92, 206, 65, 11, 70, 219, 249, 117, 11, 3, 84, 170, 232, 182, 85,
            116, 80, 28, 198, 158, 251, 91, 106, 67, 68, 64, 116, 254, 225, 22, 100, 27, 178, 157, 165, 108, 43, 74,
            127, 69, 105, 145, 252, 146, 178,
        ];
        let mut reader = Reader::new_with_coin_variant(&header_bytes, "RICK".into());
        let header = reader.read::<BlockHeader>().unwrap();
        assert_eq!(header.version, 1);
        let serialized = serialize(&header);
        assert_eq!(serialized.take(), header_bytes);
    }

    #[test]
    fn test_rick_v4_block_header_des() {
        // RICK Block header 1 bytes.
        // https://rick.explorer.dexstats.info/block/027e3758c3a65b12aa1046462b486d0a63bfa1beae327897f56c5cfb7daaae71
        let header_bytes = [
            4, 0, 0, 0, 113, 174, 170, 125, 251, 92, 108, 245, 151, 120, 50, 174, 190, 161, 191, 99, 10, 109, 72, 43,
            70, 70, 16, 170, 18, 91, 166, 195, 88, 55, 126, 2, 194, 29, 71, 248, 246, 242, 7, 112, 124, 147, 83, 57,
            42, 168, 11, 219, 126, 193, 43, 185, 130, 203, 144, 55, 29, 176, 71, 96, 191, 113, 242, 146, 251, 194, 244,
            48, 12, 1, 240, 183, 130, 13, 0, 227, 52, 124, 141, 164, 238, 97, 70, 116, 55, 108, 188, 69, 53, 157, 170,
            84, 249, 181, 73, 62, 77, 26, 64, 93, 15, 15, 15, 32, 3, 0, 126, 43, 89, 14, 131, 200, 63, 244, 88, 32,
            110, 251, 196, 31, 34, 78, 14, 187, 253, 190, 176, 188, 22, 99, 91, 146, 255, 244, 0, 0, 253, 64, 5, 0, 12,
            186, 229, 59, 82, 42, 159, 171, 130, 1, 115, 22, 85, 192, 57, 113, 206, 186, 138, 31, 36, 22, 28, 176, 192,
            16, 57, 183, 3, 138, 197, 18, 222, 93, 10, 250, 1, 51, 217, 78, 106, 3, 118, 233, 158, 30, 14, 236, 18,
            177, 236, 240, 153, 219, 121, 174, 108, 139, 17, 165, 186, 202, 24, 121, 228, 130, 173, 87, 175, 197, 87,
            129, 52, 171, 140, 105, 113, 69, 71, 104, 87, 191, 67, 0, 16, 171, 80, 86, 154, 131, 153, 191, 215, 102,
            28, 59, 244, 71, 185, 183, 220, 80, 8, 203, 14, 21, 60, 152, 170, 116, 45, 13, 226, 75, 227, 207, 92, 113,
            155, 221, 248, 219, 55, 35, 18, 1, 113, 143, 5, 125, 200, 23, 248, 108, 210, 241, 104, 253, 104, 254, 213,
            238, 111, 211, 135, 46, 16, 122, 250, 202, 163, 20, 61, 109, 161, 183, 50, 48, 11, 37, 182, 197, 255, 201,
            251, 126, 229, 1, 188, 206, 238, 28, 143, 117, 165, 239, 142, 65, 145, 13, 34, 21, 252, 184, 27, 24, 97,
            87, 36, 107, 2, 13, 244, 84, 242, 183, 75, 142, 68, 25, 25, 221, 75, 185, 44, 191, 142, 143, 78, 9, 63,
            119, 107, 99, 130, 132, 94, 39, 41, 16, 175, 203, 106, 133, 48, 183, 160, 73, 30, 79, 12, 141, 177, 19,
            237, 108, 68, 79, 232, 83, 213, 174, 193, 231, 164, 134, 244, 133, 187, 167, 92, 18, 119, 141, 220, 191,
            147, 51, 125, 27, 148, 213, 136, 222, 55, 90, 174, 185, 89, 60, 22, 117, 24, 202, 63, 40, 155, 157, 6, 111,
            136, 131, 65, 250, 107, 48, 167, 133, 180, 182, 150, 17, 5, 40, 199, 190, 14, 225, 23, 219, 211, 50, 237,
            214, 53, 71, 223, 93, 157, 194, 206, 51, 192, 6, 41, 236, 31, 184, 74, 252, 69, 85, 234, 246, 36, 213, 218,
            93, 76, 125, 226, 255, 222, 17, 243, 3, 57, 7, 16, 150, 9, 6, 137, 238, 44, 144, 191, 7, 14, 164, 200, 180,
            60, 76, 210, 232, 11, 249, 251, 69, 151, 117, 29, 31, 184, 23, 247, 95, 184, 82, 49, 234, 165, 188, 152,
            238, 195, 24, 74, 124, 137, 198, 213, 191, 197, 225, 2, 101, 213, 130, 195, 13, 189, 148, 38, 90, 232, 107,
            36, 63, 139, 62, 156, 17, 11, 11, 173, 236, 50, 182, 69, 121, 53, 210, 115, 72, 253, 14, 152, 6, 155, 76,
            67, 255, 67, 149, 128, 197, 132, 162, 223, 111, 245, 221, 149, 222, 74, 112, 2, 31, 28, 181, 57, 232, 68,
            233, 247, 105, 181, 20, 51, 21, 181, 124, 65, 38, 86, 159, 28, 115, 124, 9, 103, 244, 209, 251, 230, 24,
            109, 182, 129, 119, 181, 228, 244, 238, 241, 239, 153, 59, 120, 53, 23, 151, 227, 111, 10, 71, 100, 67,
            176, 93, 55, 54, 218, 210, 54, 54, 253, 150, 94, 121, 124, 3, 252, 232, 111, 52, 98, 168, 145, 158, 14,
            183, 84, 23, 237, 13, 170, 233, 255, 249, 248, 231, 4, 167, 22, 124, 181, 171, 43, 47, 116, 44, 161, 25,
            26, 95, 59, 216, 133, 129, 113, 250, 77, 25, 87, 114, 170, 27, 70, 104, 137, 72, 244, 2, 25, 209, 204, 234,
            140, 236, 217, 212, 63, 206, 69, 45, 226, 206, 125, 27, 6, 237, 118, 31, 9, 99, 57, 217, 13, 214, 118, 6,
            125, 244, 73, 6, 169, 55, 35, 85, 15, 220, 34, 221, 112, 195, 139, 180, 219, 123, 217, 123, 95, 159, 249,
            242, 8, 209, 222, 29, 162, 160, 218, 239, 224, 88, 194, 52, 86, 60, 194, 85, 158, 245, 253, 18, 90, 19,
            184, 68, 182, 103, 151, 247, 119, 215, 5, 117, 170, 251, 179, 8, 237, 135, 99, 78, 162, 173, 35, 125, 101,
            132, 238, 174, 102, 7, 232, 39, 52, 29, 193, 250, 243, 1, 201, 138, 175, 164, 24, 8, 1, 142, 4, 149, 90,
            102, 59, 195, 93, 198, 210, 221, 254, 87, 85, 217, 131, 191, 192, 246, 17, 84, 173, 114, 88, 199, 243, 189,
            215, 161, 195, 70, 49, 125, 176, 1, 108, 122, 59, 132, 95, 24, 6, 9, 222, 199, 222, 181, 107, 22, 229, 147,
            93, 170, 240, 23, 93, 174, 184, 15, 182, 122, 32, 32, 146, 149, 173, 19, 62, 107, 126, 206, 52, 155, 58,
            248, 110, 133, 154, 225, 143, 73, 199, 12, 21, 239, 50, 143, 9, 13, 164, 106, 237, 217, 105, 247, 207, 237,
            166, 151, 124, 186, 214, 47, 36, 152, 79, 51, 74, 91, 178, 93, 5, 222, 132, 103, 172, 87, 134, 59, 68, 62,
            159, 94, 91, 24, 32, 154, 35, 11, 12, 247, 144, 188, 247, 209, 149, 26, 155, 18, 13, 220, 124, 220, 213,
            24, 46, 50, 221, 115, 2, 208, 79, 123, 18, 173, 115, 106, 244, 59, 134, 46, 157, 203, 85, 52, 23, 13, 130,
            57, 121, 108, 232, 231, 63, 162, 70, 244, 78, 130, 239, 59, 253, 82, 92, 190, 241, 108, 111, 226, 6, 239,
            48, 166, 25, 139, 168, 251, 137, 94, 143, 110, 43, 79, 48, 167, 92, 235, 14, 55, 252, 63, 255, 43, 149, 16,
            175, 239, 160, 244, 49, 234, 93, 59, 185, 86, 100, 82, 201, 23, 64, 247, 125, 245, 94, 21, 43, 111, 39,
            160, 180, 255, 21, 73, 122, 42, 125, 50, 190, 83, 83, 28, 43, 121, 114, 213, 94, 45, 11, 8, 9, 75, 68, 74,
            234, 153, 175, 70, 254, 123, 238, 7, 50, 22, 151, 217, 30, 153, 249, 91, 203, 122, 203, 44, 182, 115, 255,
            27, 82, 38, 157, 51, 59, 31, 159, 62, 22, 249, 87, 105, 134, 203, 141, 162, 126, 112, 153, 133, 83, 90,
            177, 188, 88, 126, 36, 93, 7, 218, 138, 218, 191, 147, 4, 118, 135, 155, 87, 118, 211, 35, 150, 189, 30,
            122, 165, 8, 213, 109, 193, 178, 156, 180, 25, 251, 229, 116, 217, 55, 102, 233, 65, 162, 82, 208, 134, 76,
            85, 62, 223, 110, 237, 233, 179, 149, 244, 92, 203, 197, 51, 114, 135, 31, 74, 209, 92, 32, 27, 53, 195,
            134, 58, 203, 172, 250, 35, 205, 8, 197, 125, 167, 229, 61, 22, 200, 163, 60, 178, 21, 73, 9, 11, 55, 15,
            147, 239, 85, 237, 141, 151, 111, 101, 75, 24, 166, 57, 81, 222, 190, 137, 9, 242, 157, 142, 205, 237, 49,
            155, 169, 140, 130, 96, 102, 29, 27, 249, 61, 166, 84, 213, 104, 16, 100, 187, 68, 88, 172, 116, 53, 170,
            161, 195, 253, 149, 211, 153, 129, 41, 50, 144, 214, 85, 20, 251, 15, 42, 108, 237, 156, 15, 201, 109, 50,
            136, 244, 197, 147, 99, 63, 135, 93, 52, 100, 27, 125, 199, 228, 105, 122, 130, 29, 229, 110, 117, 26, 210,
            202, 81, 199, 35, 192, 155, 80, 238, 10, 49, 6, 53, 124, 93, 150, 106, 242, 15, 234, 106, 255, 215, 162,
            203, 44, 99, 186, 73, 24, 19, 14, 130, 5, 243, 226, 211, 87, 164, 100, 129, 254, 123, 241, 245, 133, 160,
            99, 111, 88, 212, 12, 136, 8, 219, 131, 22, 10, 69, 22, 69, 193, 163, 123, 44, 152, 216, 241, 107, 211, 83,
            110, 95, 107, 251, 242, 119, 229, 106, 201, 202, 244, 168, 236, 66, 85, 220, 14, 63, 3, 154, 31, 138, 11,
            221, 112, 246, 74, 19, 37, 121, 215, 57, 68, 75, 52, 105, 144, 206, 173, 67, 183, 54, 6, 17, 182, 39, 164,
            14, 198, 181, 134, 94, 192, 35, 12, 213, 100, 205, 226, 173, 186, 218, 40, 42, 12, 251, 180, 148, 18, 18,
            177, 201, 167, 14, 130, 112, 23, 34, 136, 129, 229, 62, 122, 148, 56, 32, 94, 62, 254, 254, 207, 15, 102,
            197, 30, 99, 165, 167, 45, 90, 134, 148, 30, 190, 180, 68,
        ];
        let mut reader = Reader::new_with_coin_variant(&header_bytes, "RICK".into());
        let header = reader.read::<BlockHeader>().unwrap();
        assert_eq!(header.version, 4);
        let serialized = serialize(&header);
        assert_eq!(serialized.take(), header_bytes);
    }

    #[test]
    fn test_morty_v1_block_header_des() {
        // MORTY Block header 0 bytes.
        // https://morty.explorer.dexstats.info/block/027e3758c3a65b12aa1046462b486d0a63bfa1beae327897f56c5cfb7daaae71
        let header_bytes = [
            1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            59, 163, 237, 253, 122, 123, 18, 178, 122, 199, 44, 62, 103, 118, 143, 97, 127, 200, 27, 195, 136, 138, 81,
            50, 58, 159, 184, 170, 75, 30, 94, 74, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 41, 171, 95, 73, 15, 15, 15, 32, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 253, 64, 5, 0, 13, 91, 167, 205, 165, 212, 115, 148,
            114, 99, 191, 25, 66, 133, 49, 113, 121, 210, 176, 211, 7, 17, 156, 46, 124, 196, 189, 138, 196, 86, 240,
            119, 75, 213, 43, 12, 217, 36, 155, 233, 212, 7, 24, 182, 57, 122, 76, 123, 189, 143, 43, 50, 114, 254,
            210, 130, 60, 210, 175, 75, 209, 99, 34, 0, 186, 75, 247, 150, 114, 125, 99, 71, 178, 37, 246, 112, 242,
            146, 52, 50, 116, 204, 53, 9, 148, 102, 245, 251, 95, 12, 209, 193, 5, 18, 27, 40, 33, 61, 21, 219, 46,
            215, 189, 186, 73, 11, 76, 237, 198, 151, 66, 165, 123, 124, 37, 175, 36, 72, 94, 82, 58, 173, 187, 119,
            160, 20, 79, 199, 111, 121, 239, 115, 189, 133, 48, 212, 43, 159, 59, 155, 237, 28, 19, 90, 209, 254, 21,
            41, 35, 250, 254, 152, 249, 95, 118, 241, 97, 94, 100, 196, 171, 177, 19, 127, 76, 49, 178, 24, 186, 39,
            130, 188, 21, 83, 71, 136, 221, 162, 204, 8, 160, 238, 41, 135, 200, 178, 127, 244, 27, 212, 227, 28, 213,
            251, 86, 67, 223, 232, 98, 201, 160, 44, 169, 249, 12, 140, 81, 166, 103, 29, 104, 29, 4, 173, 71, 228,
            181, 59, 21, 24, 212, 190, 250, 254, 254, 140, 173, 251, 145, 47, 61, 3, 5, 27, 30, 251, 241, 223, 227,
            123, 86, 233, 58, 116, 29, 141, 253, 128, 213, 118, 202, 37, 11, 238, 85, 250, 177, 49, 31, 199, 179, 37,
            89, 119, 85, 140, 221, 166, 247, 214, 248, 117, 48, 110, 67, 161, 68, 19, 250, 205, 174, 210, 244, 96, 147,
            224, 239, 30, 143, 138, 150, 62, 22, 50, 220, 190, 235, 216, 228, 159, 209, 107, 87, 212, 155, 8, 249, 118,
            45, 232, 145, 87, 198, 82, 51, 246, 12, 142, 56, 161, 245, 3, 164, 140, 85, 95, 142, 196, 93, 237, 236,
            213, 116, 163, 118, 1, 50, 60, 39, 190, 89, 123, 149, 99, 67, 16, 127, 139, 216, 15, 58, 146, 90, 250, 243,
            8, 17, 223, 131, 196, 2, 17, 107, 185, 193, 229, 35, 28, 112, 255, 248, 153, 167, 200, 47, 115, 201, 2,
            186, 84, 218, 83, 204, 69, 155, 123, 241, 17, 61, 182, 92, 200, 246, 145, 77, 54, 24, 86, 14, 166, 154,
            189, 19, 101, 143, 167, 182, 175, 146, 211, 116, 214, 236, 169, 82, 159, 139, 213, 101, 22, 110, 79, 203,
            242, 168, 223, 179, 201, 182, 149, 57, 212, 210, 238, 46, 147, 33, 184, 91, 51, 25, 37, 223, 25, 89, 21,
            242, 117, 118, 55, 194, 128, 94, 29, 65, 49, 225, 173, 158, 249, 188, 27, 177, 199, 50, 216, 219, 164, 115,
            135, 22, 211, 81, 171, 48, 201, 150, 200, 101, 123, 171, 57, 86, 126, 227, 178, 156, 109, 5, 75, 113, 20,
            149, 192, 213, 46, 28, 213, 216, 229, 91, 79, 15, 3, 37, 185, 115, 105, 40, 7, 85, 180, 106, 2, 175, 213,
            75, 228, 221, 217, 247, 124, 34, 39, 43, 139, 187, 23, 255, 81, 24, 254, 219, 174, 37, 100, 82, 78, 121,
            123, 210, 139, 95, 116, 247, 7, 157, 83, 44, 204, 5, 152, 7, 152, 159, 148, 210, 103, 244, 126, 114, 75,
            63, 30, 207, 224, 14, 201, 230, 84, 28, 150, 16, 128, 216, 137, 18, 81, 184, 75, 68, 128, 188, 41, 47, 106,
            24, 11, 234, 8, 159, 239, 91, 189, 165, 110, 30, 65, 57, 13, 124, 14, 133, 186, 14, 245, 48, 247, 23, 116,
            19, 72, 26, 34, 100, 101, 163, 110, 246, 175, 225, 226, 188, 166, 157, 32, 120, 113, 43, 57, 18, 187, 161,
            169, 155, 31, 191, 240, 211, 85, 214, 255, 231, 38, 210, 187, 111, 188, 16, 60, 74, 197, 117, 110, 91, 238,
            110, 71, 225, 116, 36, 235, 203, 241, 182, 61, 140, 185, 12, 226, 228, 1, 152, 180, 244, 25, 134, 137, 218,
            234, 37, 67, 7, 229, 42, 37, 86, 47, 76, 20, 85, 52, 15, 15, 254, 177, 15, 157, 142, 145, 71, 117, 227,
            125, 14, 220, 160, 25, 251, 27, 156, 110, 248, 18, 85, 237, 134, 188, 81, 197, 57, 30, 5, 145, 72, 15, 102,
            226, 216, 140, 95, 79, 215, 39, 118, 151, 150, 134, 86, 169, 177, 19, 171, 151, 248, 116, 253, 213, 242,
            70, 94, 85, 89, 83, 62, 1, 186, 19, 239, 74, 143, 122, 33, 208, 44, 48, 200, 222, 214, 142, 140, 84, 96,
            58, 185, 200, 8, 78, 246, 217, 235, 78, 146, 199, 91, 7, 133, 57, 226, 174, 120, 110, 186, 182, 218, 183,
            58, 9, 224, 170, 154, 197, 117, 188, 239, 178, 158, 147, 10, 230, 86, 229, 139, 203, 81, 63, 126, 60, 23,
            224, 121, 220, 228, 240, 91, 93, 188, 24, 194, 168, 114, 178, 37, 9, 116, 14, 190, 106, 57, 3, 224, 10,
            209, 171, 197, 80, 118, 68, 24, 98, 100, 63, 147, 96, 110, 61, 195, 94, 141, 159, 44, 174, 243, 238, 107,
            225, 77, 81, 59, 46, 6, 43, 33, 208, 6, 29, 227, 189, 86, 136, 23, 19, 161, 165, 193, 127, 90, 206, 5, 225,
            236, 9, 218, 83, 249, 148, 66, 223, 23, 90, 73, 189, 21, 74, 169, 110, 73, 73, 222, 205, 82, 254, 215, 156,
            207, 124, 203, 206, 50, 148, 20, 25, 195, 20, 227, 116, 228, 163, 150, 172, 85, 62, 23, 181, 52, 3, 54,
            161, 162, 92, 34, 249, 228, 42, 36, 59, 165, 64, 68, 80, 182, 80, 172, 252, 130, 106, 110, 67, 41, 113,
            172, 231, 118, 225, 87, 25, 81, 94, 22, 52, 206, 185, 164, 163, 80, 97, 182, 104, 199, 73, 152, 211, 223,
            181, 130, 127, 98, 56, 236, 1, 83, 119, 230, 249, 201, 79, 56, 16, 135, 104, 207, 110, 92, 139, 19, 46, 3,
            3, 251, 90, 32, 3, 104, 248, 69, 173, 157, 70, 52, 48, 53, 166, 255, 148, 3, 29, 248, 216, 48, 148, 21,
            187, 63, 108, 213, 237, 233, 193, 53, 253, 171, 204, 3, 5, 153, 133, 141, 128, 60, 15, 133, 190, 118, 97,
            200, 137, 132, 216, 143, 170, 61, 38, 251, 14, 154, 172, 0, 86, 165, 63, 27, 93, 11, 174, 215, 19, 200, 83,
            196, 162, 114, 104, 105, 160, 161, 36, 168, 165, 187, 192, 252, 14, 248, 12, 138, 228, 203, 83, 99, 106,
            160, 37, 3, 184, 106, 30, 185, 131, 111, 204, 37, 152, 35, 226, 105, 45, 146, 29, 136, 225, 255, 193, 230,
            203, 43, 222, 67, 147, 156, 235, 63, 50, 166, 17, 104, 111, 83, 159, 143, 124, 159, 11, 240, 3, 129, 247,
            67, 96, 125, 64, 150, 15, 6, 211, 71, 209, 205, 138, 200, 165, 25, 105, 194, 94, 55, 21, 14, 253, 247, 170,
            76, 32, 55, 162, 253, 5, 22, 251, 68, 69, 37, 171, 21, 122, 14, 208, 167, 65, 43, 47, 166, 155, 33, 127,
            227, 151, 38, 49, 83, 120, 44, 15, 100, 53, 31, 189, 242, 103, 143, 160, 220, 133, 105, 145, 45, 205, 142,
            60, 202, 211, 143, 52, 242, 59, 187, 206, 20, 198, 162, 106, 194, 73, 17, 179, 8, 184, 44, 126, 67, 6, 45,
            24, 11, 174, 172, 75, 167, 21, 56, 88, 54, 92, 114, 198, 61, 207, 95, 106, 91, 8, 7, 11, 115, 10, 219, 1,
            122, 234, 233, 37, 183, 208, 67, 153, 121, 226, 103, 159, 69, 237, 47, 37, 167, 237, 207, 210, 251, 119,
            168, 121, 70, 48, 40, 92, 203, 10, 7, 31, 92, 206, 65, 11, 70, 219, 249, 117, 11, 3, 84, 170, 232, 182, 85,
            116, 80, 28, 198, 158, 251, 91, 106, 67, 68, 64, 116, 254, 225, 22, 100, 27, 178, 157, 165, 108, 43, 74,
            127, 69, 105, 145, 252, 146, 178,
        ];
        let mut reader = Reader::new_with_coin_variant(&header_bytes, "MORTY".into());
        let header = reader.read::<BlockHeader>().unwrap();
        assert_eq!(header.version, 1);
        let serialized = serialize(&header);
        assert_eq!(serialized.take(), header_bytes);
    }

    #[test]
    fn test_morty_v4_block_header_des() {
        // MORTY Block header 0 bytes.
        // https://morty.explorer.dexstats.info/block/027e3758c3a65b12aa1046462b486d0a63bfa1beae327897f56c5cfb7daaae71
        let header_bytes = [
            4, 0, 0, 0, 113, 174, 170, 125, 251, 92, 108, 245, 151, 120, 50, 174, 190, 161, 191, 99, 10, 109, 72, 43,
            70, 70, 16, 170, 18, 91, 166, 195, 88, 55, 126, 2, 151, 199, 90, 42, 33, 212, 194, 188, 209, 116, 36, 128,
            91, 2, 174, 229, 185, 93, 183, 2, 179, 206, 206, 175, 86, 80, 11, 96, 37, 196, 9, 164, 251, 194, 244, 48,
            12, 1, 240, 183, 130, 13, 0, 227, 52, 124, 141, 164, 238, 97, 70, 116, 55, 108, 188, 69, 53, 157, 170, 84,
            249, 181, 73, 62, 9, 28, 64, 93, 15, 15, 15, 32, 14, 0, 253, 84, 160, 204, 64, 38, 208, 250, 142, 84, 16,
            96, 7, 47, 42, 82, 199, 204, 159, 177, 5, 61, 174, 22, 236, 47, 153, 181, 0, 0, 253, 64, 5, 0, 120, 233,
            249, 24, 200, 221, 174, 207, 128, 194, 81, 54, 187, 214, 14, 169, 15, 56, 52, 72, 59, 218, 242, 180, 5,
            181, 105, 111, 216, 85, 87, 238, 68, 91, 32, 187, 117, 194, 94, 221, 176, 36, 56, 100, 163, 232, 91, 25,
            200, 226, 110, 166, 99, 252, 208, 200, 219, 139, 38, 220, 253, 33, 64, 148, 229, 11, 33, 97, 160, 165, 47,
            58, 148, 106, 112, 172, 177, 43, 38, 140, 251, 17, 115, 5, 143, 218, 114, 172, 144, 228, 162, 209, 154,
            160, 151, 45, 145, 60, 6, 99, 48, 55, 176, 198, 31, 100, 235, 237, 214, 101, 10, 175, 50, 87, 71, 81, 76,
            96, 247, 251, 30, 92, 255, 82, 244, 14, 95, 109, 79, 96, 225, 112, 253, 165, 16, 97, 146, 109, 220, 81,
            234, 156, 158, 54, 68, 58, 22, 252, 90, 161, 18, 174, 179, 233, 125, 59, 243, 84, 30, 39, 86, 74, 64, 214,
            54, 192, 250, 8, 191, 189, 121, 70, 207, 204, 176, 187, 252, 80, 252, 163, 235, 190, 96, 96, 125, 213, 246,
            210, 9, 37, 230, 27, 125, 27, 88, 11, 144, 215, 197, 92, 189, 75, 172, 150, 237, 15, 120, 223, 247, 17,
            151, 63, 235, 148, 206, 253, 223, 97, 194, 72, 67, 140, 241, 129, 35, 117, 135, 30, 42, 141, 44, 226, 34,
            145, 115, 114, 210, 17, 184, 178, 66, 240, 66, 252, 236, 208, 204, 104, 214, 132, 15, 33, 209, 97, 116,
            231, 53, 189, 21, 250, 70, 178, 235, 235, 248, 252, 197, 85, 118, 241, 66, 200, 69, 17, 196, 179, 100, 90,
            112, 247, 220, 19, 120, 10, 245, 236, 93, 98, 249, 129, 186, 154, 85, 42, 56, 198, 221, 179, 77, 199, 165,
            192, 60, 116, 202, 106, 190, 159, 50, 94, 246, 219, 141, 162, 83, 216, 59, 75, 110, 85, 31, 31, 81, 53,
            214, 10, 3, 202, 54, 121, 234, 50, 126, 148, 125, 9, 161, 137, 3, 164, 67, 250, 0, 117, 118, 17, 80, 85,
            220, 251, 196, 217, 141, 23, 124, 159, 10, 46, 44, 122, 31, 99, 188, 97, 33, 191, 161, 27, 51, 148, 157,
            208, 175, 102, 148, 152, 100, 11, 205, 241, 189, 151, 135, 53, 137, 39, 238, 180, 45, 178, 93, 108, 130,
            105, 75, 217, 62, 215, 70, 61, 51, 21, 171, 248, 166, 27, 205, 111, 117, 36, 184, 73, 187, 230, 11, 244,
            31, 23, 175, 20, 178, 235, 182, 224, 89, 8, 169, 87, 175, 51, 74, 37, 170, 10, 1, 19, 101, 148, 241, 89,
            33, 174, 43, 137, 187, 215, 223, 147, 248, 67, 214, 180, 40, 244, 178, 163, 134, 131, 220, 91, 151, 24,
            169, 86, 244, 179, 83, 220, 125, 180, 69, 162, 216, 32, 207, 65, 36, 219, 176, 231, 48, 154, 24, 217, 154,
            215, 178, 29, 201, 9, 239, 49, 100, 25, 220, 234, 178, 85, 62, 38, 154, 213, 35, 12, 156, 98, 15, 33, 71,
            42, 64, 65, 178, 100, 202, 55, 204, 100, 241, 224, 46, 95, 21, 177, 63, 119, 247, 224, 113, 146, 172, 171,
            103, 71, 68, 138, 67, 84, 122, 250, 236, 172, 125, 194, 90, 23, 57, 71, 107, 17, 177, 154, 15, 192, 175,
            66, 46, 109, 110, 110, 182, 111, 61, 244, 182, 27, 96, 29, 101, 39, 240, 95, 30, 101, 43, 83, 247, 122,
            194, 89, 148, 146, 150, 180, 218, 250, 196, 13, 113, 92, 174, 0, 45, 101, 35, 209, 254, 7, 94, 240, 253,
            11, 250, 190, 54, 125, 88, 67, 57, 32, 109, 228, 186, 175, 45, 119, 177, 181, 51, 255, 66, 225, 252, 73,
            98, 178, 123, 82, 18, 13, 193, 84, 229, 197, 209, 56, 25, 70, 4, 40, 142, 243, 235, 170, 138, 178, 152,
            153, 123, 88, 23, 115, 205, 215, 50, 220, 204, 95, 135, 5, 193, 219, 145, 171, 182, 234, 9, 181, 147, 180,
            42, 0, 161, 173, 225, 52, 71, 243, 170, 128, 252, 178, 249, 149, 54, 62, 238, 126, 25, 29, 114, 30, 30,
            217, 86, 254, 58, 164, 125, 111, 95, 220, 114, 126, 172, 243, 154, 165, 143, 117, 187, 180, 3, 11, 45, 196,
            149, 179, 117, 164, 201, 238, 197, 164, 56, 215, 104, 109, 25, 229, 64, 153, 183, 232, 28, 67, 33, 243, 35,
            78, 222, 254, 241, 203, 193, 217, 53, 162, 127, 237, 231, 179, 156, 212, 198, 1, 12, 212, 199, 26, 243, 41,
            229, 165, 107, 150, 218, 71, 92, 209, 171, 29, 36, 157, 219, 179, 29, 93, 52, 244, 170, 158, 83, 111, 52,
            228, 194, 25, 93, 249, 6, 91, 5, 242, 121, 165, 58, 29, 202, 71, 177, 210, 15, 115, 163, 32, 175, 151, 43,
            139, 78, 67, 5, 213, 200, 59, 191, 130, 54, 85, 239, 82, 29, 126, 61, 63, 244, 52, 21, 242, 44, 241, 26,
            202, 115, 237, 90, 110, 2, 0, 176, 205, 88, 137, 87, 163, 69, 89, 70, 193, 97, 171, 205, 175, 208, 255,
            129, 139, 49, 15, 35, 224, 53, 106, 215, 82, 181, 253, 42, 177, 18, 217, 127, 192, 69, 241, 165, 81, 77,
            235, 139, 8, 229, 153, 53, 242, 195, 65, 110, 241, 128, 146, 224, 194, 104, 242, 10, 187, 65, 94, 160, 123,
            28, 82, 221, 134, 222, 91, 69, 201, 41, 31, 70, 53, 27, 221, 10, 191, 107, 90, 29, 52, 187, 0, 219, 198,
            104, 87, 73, 118, 8, 105, 226, 176, 209, 108, 16, 233, 116, 231, 149, 23, 59, 246, 36, 194, 190, 129, 166,
            164, 197, 105, 194, 239, 227, 252, 125, 203, 206, 13, 49, 123, 180, 219, 117, 2, 133, 221, 153, 79, 135,
            146, 194, 161, 191, 225, 24, 243, 145, 47, 25, 23, 87, 17, 219, 91, 35, 13, 185, 78, 136, 205, 153, 62,
            174, 92, 57, 86, 180, 230, 38, 147, 40, 11, 61, 101, 180, 1, 206, 196, 194, 120, 88, 177, 251, 43, 148,
            131, 198, 41, 244, 35, 201, 27, 212, 248, 121, 194, 7, 218, 98, 8, 32, 4, 139, 208, 173, 49, 192, 203, 155,
            147, 201, 250, 124, 118, 244, 160, 65, 4, 114, 231, 198, 184, 129, 131, 142, 124, 150, 132, 103, 163, 247,
            125, 214, 110, 229, 215, 139, 245, 55, 90, 155, 212, 26, 31, 77, 47, 126, 219, 21, 74, 22, 204, 12, 93,
            253, 127, 145, 118, 156, 15, 103, 122, 219, 2, 228, 48, 37, 87, 206, 2, 93, 86, 74, 106, 24, 231, 202, 221,
            237, 232, 47, 162, 195, 114, 16, 20, 214, 151, 38, 212, 198, 33, 36, 75, 233, 98, 139, 203, 28, 23, 55, 21,
            69, 74, 238, 226, 26, 201, 205, 22, 137, 35, 248, 13, 79, 174, 88, 254, 50, 149, 155, 146, 92, 123, 53, 88,
            148, 219, 226, 172, 223, 226, 5, 238, 106, 223, 20, 146, 173, 133, 54, 179, 132, 5, 186, 40, 177, 227, 15,
            114, 16, 240, 192, 86, 56, 197, 247, 231, 169, 251, 216, 208, 48, 166, 26, 200, 193, 46, 192, 165, 206,
            237, 103, 176, 148, 88, 166, 46, 137, 37, 31, 203, 241, 187, 229, 34, 99, 6, 241, 51, 182, 172, 105, 205,
            216, 131, 231, 117, 49, 145, 83, 135, 96, 60, 239, 227, 40, 86, 164, 104, 218, 81, 238, 242, 212, 213, 52,
            19, 143, 213, 69, 245, 94, 198, 149, 46, 127, 13, 142, 74, 11, 212, 179, 48, 71, 197, 234, 177, 160, 43,
            150, 17, 242, 127, 195, 249, 125, 236, 27, 157, 254, 146, 96, 230, 138, 133, 199, 232, 225, 251, 248, 92,
            159, 41, 168, 137, 250, 119, 111, 19, 156, 63, 1, 145, 242, 254, 191, 198, 39, 50, 135, 1, 206, 153, 173,
            18, 144, 31, 139, 113, 34, 195, 127, 249, 240, 148, 137, 108, 183, 210, 82, 68, 79, 47, 159, 196, 184, 61,
            124, 219, 155,
        ];
        let mut reader = Reader::new_with_coin_variant(&header_bytes, "MORTY".into());
        let header = reader.read::<BlockHeader>().unwrap();
        assert_eq!(header.version, 4);
        let serialized = serialize(&header);
        assert_eq!(serialized.take(), header_bytes);
    }

    #[test]
    fn test_chta_kawpow_version_header() {
        let header_hex = "00000030f0aceae7f05f5951ec0a6adae323f6e77bcd28beda092749e30800000000000072c4e8c753c4694ec6152fe97c73f72baf5b58176ca05adc7060989267b4816265ae8268cada0a1afcabfdbd";
        let header_bytes: Vec<u8> = header_hex.from_hex().unwrap();
        let header: BlockHeader = deserialize(header_bytes.as_slice()).unwrap();

        assert_eq!(header.version, KAWPOW_VERSION);

        let serialized = serialize(&header);
        assert_eq!(serialized.take(), header_bytes);
    }

    #[test]
    fn test_pivx_sapling_header() {
        let header_hex = "0b000000097d36aeeb2585e6c08226f8f48cb91213708fcad603cb67be76efa5b3b31c0baf86a77624fd298be0f5a7b908d17d3d83edf8f681de2913b2584fb92380e152594229684411051b00000000c801eff496c2720766cdbf2ec20b5436b37350e2945f85a7feb8a4b4a12d4323";
        let header_bytes = &header_hex.from_hex::<Vec<u8>>().unwrap() as &[u8];
        let mut reader = Reader::new_with_coin_variant(header_bytes, CoinVariant::PIVX);
        let header: BlockHeader = reader.read().unwrap();

        // Sapling root must be present
        assert!(header.hash_final_sapling_root.is_some());

        let serialized = serialize(&header);
        assert_eq!(serialized.take(), header_bytes);
    }

    #[test]
    fn test_nav_block_header_v7_no_auxpow_and_roundtrip() {
        // NAV block #9561948
        // https://explorer.navcoin.org/block/9561948
        //
        // version: 0x7bf5ffff (bit 8 set, but NAV is BTC-style and must NOT parse AuxPoW)
        // prev    = 57719d707a2e86fd3cb7a379d0575943cd4679de77e3629bcb7728b75abbeb6a
        // merkle  = 5b57bb3d2ca2706d3d53a86a0ac6d81cbb5590ff61c357e70654b66d4bcaf195
        // time    = 0x68936f70
        // bits    = 0x1a2dab8e
        // nonce   = 7
        let header_hex = "fffff57b\
                      6aebbb5ab72877cb9b62e377de7946cd435957d079a3b73cfd862e7a709d7157\
                      95f1ca4b6db65406e757c361ff9055bb1cd8c60a6aa8533d6d70a22c3dbb575b\
                      706f9368\
                      8eab2d1a\
                      07000000";

        let header_bytes: Vec<u8> = header_hex.from_hex().unwrap();

        // Treat NAV as BTC-style (no AuxPoW parsing)
        let mut reader = Reader::new_with_coin_variant(header_bytes.as_slice(), CoinVariant::BTC);
        let header: BlockHeader = reader.read().unwrap();

        assert_eq!(header.version, 0x7bf5ffff);
        assert!(header.aux_pow.is_none(), "NAV must not parse AuxPoW payloads");

        // Round-trip
        let serialized = serialize(&header);
        assert_eq!(serialized.take(), header_bytes);
    }
}
