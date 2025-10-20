use crate::conf::SPVBlockHeader;
use crate::storage::{BlockHeaderStorageError, BlockHeaderStorageOps};
use chain::BlockHeaderBits;
use derive_more::Display;
use primitives::compact::Compact;
use primitives::U256;
use serde::{Deserialize, Serialize};
use std::cmp;

const RETARGETING_FACTOR: u32 = 4;
const TARGET_SPACING_SECONDS: u32 = 10 * 60;
const TARGET_TIMESPAN_SECONDS: u32 = 2 * 7 * 24 * 60 * 60;

/// The Target number of blocks equals to 2 weeks or 2016 blocks
pub(crate) const RETARGETING_INTERVAL: u32 = TARGET_TIMESPAN_SECONDS / TARGET_SPACING_SECONDS;

/// The upper and lower bounds for retargeting timespan
const MIN_TIMESPAN: u32 = TARGET_TIMESPAN_SECONDS / RETARGETING_FACTOR;
const MAX_TIMESPAN: u32 = TARGET_TIMESPAN_SECONDS * RETARGETING_FACTOR;

/// The maximum value for bits corresponding to lowest difficulty of 1
pub const MAX_BITS_BTC: u32 = 486604799;

fn is_retarget_height(height: u64) -> bool {
    height.is_multiple_of(RETARGETING_INTERVAL as u64)
}

#[derive(Clone, Debug, Display, Eq, PartialEq)]
pub enum NextBlockBitsError {
    #[display(fmt = "Block headers storage error: {_0}")]
    StorageError(BlockHeaderStorageError),
    #[display(fmt = "Can't find Block header for {coin} with height {height}")]
    NoSuchBlockHeader { coin: String, height: u64 },
    #[display(fmt = "Can't find a Block header for {coin} with no max bits")]
    NoBlockHeaderWithNoMaxBits { coin: String },
}

impl From<BlockHeaderStorageError> for NextBlockBitsError {
    fn from(e: BlockHeaderStorageError) -> Self {
        NextBlockBitsError::StorageError(e)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum DifficultyAlgorithm {
    #[serde(rename = "Bitcoin Mainnet")]
    BitcoinMainnet,
    #[serde(rename = "Bitcoin Testnet")]
    BitcoinTestnet,
}

pub async fn next_block_bits(
    coin: &str,
    current_block_timestamp: u32,
    last_block_header: SPVBlockHeader,
    storage: &dyn BlockHeaderStorageOps,
    algorithm: &DifficultyAlgorithm,
) -> Result<BlockHeaderBits, NextBlockBitsError> {
    match algorithm {
        DifficultyAlgorithm::BitcoinMainnet => btc_mainnet_next_block_bits(coin, last_block_header, storage).await,
        DifficultyAlgorithm::BitcoinTestnet => {
            btc_testnet_next_block_bits(coin, current_block_timestamp, last_block_header, storage).await
        },
    }
}

fn range_constrain(value: i64, min: i64, max: i64) -> i64 {
    cmp::min(cmp::max(value, min), max)
}

/// Returns constrained number of seconds since last retarget
fn retarget_timespan(retarget_timestamp: u32, last_timestamp: u32) -> u32 {
    // subtract unsigned 32 bit numbers in signed 64 bit space in
    // order to prevent underflow before applying the range constraint.
    let timespan = last_timestamp as i64 - retarget_timestamp as i64;
    range_constrain(timespan, MIN_TIMESPAN as i64, MAX_TIMESPAN as i64) as u32
}

async fn btc_retarget_bits(
    coin: &str,
    last_block_header: SPVBlockHeader,
    storage: &dyn BlockHeaderStorageOps,
) -> Result<BlockHeaderBits, NextBlockBitsError> {
    let max_bits_compact: Compact = MAX_BITS_BTC.into();

    let retarget_ref = last_block_header.height + 1 - RETARGETING_INTERVAL as u64;
    if retarget_ref == 0 {
        return Ok(BlockHeaderBits::Compact(max_bits_compact));
    }

    let retarget_header = storage
        .get_block_header(retarget_ref)
        .await?
        .map(|h| SPVBlockHeader::from_block_header_and_height(&h, retarget_ref))
        .ok_or(NextBlockBitsError::NoSuchBlockHeader {
            coin: coin.into(),
            height: retarget_ref,
        })?;

    // timestamp of block(height - RETARGETING_INTERVAL)
    let retarget_timestamp = retarget_header.time;
    // timestamp of last block
    let last_timestamp = last_block_header.time;

    let retarget: Compact = last_block_header.bits.into();
    let retarget: U256 = retarget.into();
    let retarget_timespan: U256 = retarget_timespan(retarget_timestamp, last_timestamp).into();
    let retarget: U256 = retarget * retarget_timespan;
    let target_timespan_seconds: U256 = TARGET_TIMESPAN_SECONDS.into();
    let retarget = retarget / target_timespan_seconds;

    let max_bits: U256 = max_bits_compact.into();
    if retarget > max_bits {
        Ok(BlockHeaderBits::Compact(max_bits_compact))
    } else {
        Ok(BlockHeaderBits::Compact(retarget.into()))
    }
}

async fn btc_mainnet_next_block_bits(
    coin: &str,
    last_block_header: SPVBlockHeader,
    storage: &dyn BlockHeaderStorageOps,
) -> Result<BlockHeaderBits, NextBlockBitsError> {
    if last_block_header.height == 0 {
        return Ok(BlockHeaderBits::Compact(MAX_BITS_BTC.into()));
    }

    let next_height = last_block_header.height + 1;
    let last_block_bits = last_block_header.bits.clone();

    if is_retarget_height(next_height) {
        btc_retarget_bits(coin, last_block_header, storage).await
    } else {
        Ok(last_block_bits)
    }
}

async fn btc_testnet_next_block_bits(
    coin: &str,
    current_block_timestamp: u32,
    last_block_header: SPVBlockHeader,
    storage: &dyn BlockHeaderStorageOps,
) -> Result<BlockHeaderBits, NextBlockBitsError> {
    let max_bits = BlockHeaderBits::Compact(MAX_BITS_BTC.into());
    if last_block_header.height == 0 {
        return Ok(max_bits);
    }

    let next_height = last_block_header.height + 1;
    let last_block_bits = last_block_header.bits.clone();
    let max_time_gap = last_block_header.time + 2 * TARGET_SPACING_SECONDS;

    if is_retarget_height(next_height) {
        btc_retarget_bits(coin, last_block_header, storage).await
    } else if current_block_timestamp > max_time_gap {
        Ok(max_bits)
    } else if last_block_bits != max_bits {
        Ok(last_block_bits.clone())
    } else {
        let last_non_max_bits = storage
            .get_last_block_header_with_non_max_bits(MAX_BITS_BTC)
            .await?
            .map(|header| header.bits)
            .unwrap_or(max_bits);
        Ok(last_non_max_bits)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::storage::{BlockHeaderStorageError, BlockHeaderStorageOps};
    use async_trait::async_trait;
    use chain::BlockHeader;
    use common::block_on;
    use lazy_static::lazy_static;
    use primitives::hash::H256;
    use serde::Deserialize;
    use std::collections::HashMap;

    const BLOCK_HEADERS_STR: &str = include_str!("./for_tests/workTestVectors.json");

    #[derive(Deserialize)]
    struct TestRawHeader {
        height: u64,
        hex: String,
    }

    lazy_static! {
        static ref BLOCK_HEADERS_MAP: HashMap<String, Vec<TestRawHeader>> = parse_block_headers();
    }

    fn parse_block_headers() -> HashMap<String, Vec<TestRawHeader>> {
        serde_json::from_str(BLOCK_HEADERS_STR).unwrap()
    }

    fn get_block_headers_for_coin(coin: &str) -> HashMap<u64, BlockHeader> {
        BLOCK_HEADERS_MAP
            .get(coin)
            .unwrap()
            .iter()
            .map(|h| (h.height, h.hex.as_str().into()))
            .collect()
    }

    pub struct TestBlockHeadersStorage {
        pub(crate) ticker: String,
    }

    #[async_trait]
    impl BlockHeaderStorageOps for TestBlockHeadersStorage {
        async fn init(&self) -> Result<(), BlockHeaderStorageError> {
            Ok(())
        }

        async fn is_initialized_for(&self) -> Result<bool, BlockHeaderStorageError> {
            Ok(true)
        }

        async fn add_block_headers_to_storage(
            &self,
            _headers: HashMap<u64, BlockHeader>,
        ) -> Result<(), BlockHeaderStorageError> {
            Ok(())
        }

        async fn get_block_header(&self, height: u64) -> Result<Option<BlockHeader>, BlockHeaderStorageError> {
            Ok(get_block_headers_for_coin(&self.ticker).get(&height).cloned())
        }

        async fn get_block_header_raw(&self, _height: u64) -> Result<Option<String>, BlockHeaderStorageError> {
            Ok(None)
        }

        async fn get_last_block_height(&self) -> Result<Option<u64>, BlockHeaderStorageError> {
            Ok(Some(
                get_block_headers_for_coin(&self.ticker)
                    .into_keys()
                    .max_by(|a, b| a.cmp(b))
                    .unwrap(),
            ))
        }

        async fn get_last_block_header_with_non_max_bits(
            &self,
            max_bits: u32,
        ) -> Result<Option<BlockHeader>, BlockHeaderStorageError> {
            let mut headers = get_block_headers_for_coin(&self.ticker);
            headers.retain(|_, h| h.bits != BlockHeaderBits::Compact(max_bits.into()));
            let header = headers.into_iter().max_by(|a, b| a.0.cmp(&b.0));
            Ok(header.map(|(_, h)| h))
        }

        async fn get_block_height_by_hash(&self, _hash: H256) -> Result<Option<i64>, BlockHeaderStorageError> {
            Ok(None)
        }

        async fn remove_headers_from_storage(
            &self,
            _from_height: u64,
            _to_height: u64,
        ) -> Result<(), BlockHeaderStorageError> {
            Ok(())
        }

        async fn is_table_empty(&self) -> Result<(), BlockHeaderStorageError> {
            Ok(())
        }
    }

    #[test]
    fn test_btc_mainnet_next_block_bits() {
        let storage = TestBlockHeadersStorage { ticker: "BTC".into() };

        let last_header: BlockHeader = "000000201d758432ecd495a2177b44d3fe6c22af183461a0b9ea0d0000000000000000008283a1dfa795d9b68bd8c18601e443368265072cbf8c76bfe58de46edd303798035de95d3eb2151756fdb0e8".into();

        let next_block_bits = block_on(btc_mainnet_next_block_bits(
            "BTC",
            SPVBlockHeader::from_block_header_and_height(&last_header, 606815),
            &storage,
        ))
        .unwrap();
        assert_eq!(next_block_bits, BlockHeaderBits::Compact(387308498.into()));

        // check that bits for very early blocks that didn't change difficulty because of low hashrate is calculated correctly.
        let last_header: BlockHeader = "010000000d9c8c96715756b619116cc2160937fb26c655a2f8e28e3a0aff59c0000000007676252e8434de408ea31920d986aba297bd6f7c6f20756be08748713f7c135962719449ffff001df8c1cb01".into();

        let next_block_bits = block_on(btc_mainnet_next_block_bits(
            "BTC",
            SPVBlockHeader::from_block_header_and_height(&last_header, 4031),
            &storage,
        ))
        .unwrap();
        assert_eq!(next_block_bits, BlockHeaderBits::Compact(486604799.into()));

        // check that bits stay the same when the next block is not a retarget block
        // https://live.blockcypher.com/btc/block/00000000000000000002622f52b6afe70a5bb139c788e67f221ffc67a762a1e0/
        let last_header: BlockHeader = "00e0ff2f44d953fe12a047129bbc7164668c6d96f3e7a553528b02000000000000000000d0b950384cd23ab0854d1c8f23fa7a97411a6ffd92347c0a3aea4466621e4093ec09c762afa7091705dad220".into();

        let next_block_bits = block_on(btc_mainnet_next_block_bits(
            "BTC",
            SPVBlockHeader::from_block_header_and_height(&last_header, 744014),
            &storage,
        ))
        .unwrap();
        assert_eq!(next_block_bits, BlockHeaderBits::Compact(386508719.into()));
    }

    #[test]
    fn test_btc_testnet_next_block_bits() {
        let storage = TestBlockHeadersStorage { ticker: "tBTC".into() };

        // https://live.blockcypher.com/btc-testnet/block/000000000057db3806384e2ec1b02b2c86bd928206ff8dff98f54d616b7fa5f2/
        let current_header: BlockHeader = "02000000303505969a1df329e5fccdf69b847a201772e116e557eb7f119d1a9600000000469267f52f43b8799e72f0726ba2e56432059a8ad02b84d4fff84b9476e95f7716e41353ab80011c168cb471".into();
        // https://live.blockcypher.com/btc-testnet/block/00000000961a9d117feb57e516e17217207a849bf6cdfce529f31d9a96053530/
        let last_header: BlockHeader = "02000000ea01a61a2d7420a1b23875e40eb5eb4ca18b378902c8e6384514ad0000000000c0c5a1ae80582b3fe319d8543307fa67befc2a734b8eddb84b1780dfdf11fa2b20e71353ffff001d00805fe0".into();

        let next_block_bits = block_on(btc_testnet_next_block_bits(
            "tBTC",
            current_header.time,
            SPVBlockHeader::from_block_header_and_height(&last_header, 201595),
            &storage,
        ))
        .unwrap();
        assert_eq!(next_block_bits, BlockHeaderBits::Compact(469860523.into()));

        // https://live.blockcypher.com/btc-testnet/block/00000000961a9d117feb57e516e17217207a849bf6cdfce529f31d9a96053530/
        let current_header: BlockHeader = "02000000ea01a61a2d7420a1b23875e40eb5eb4ca18b378902c8e6384514ad0000000000c0c5a1ae80582b3fe319d8543307fa67befc2a734b8eddb84b1780dfdf11fa2b20e71353ffff001d00805fe0".into();
        // https://live.blockcypher.com/btc-testnet/block/0000000000ad144538e6c80289378ba14cebb50ee47538b2a120742d1aa601ea/
        let last_header: BlockHeader = "02000000cbed7fd98f1f06e85c47e13ff956533642056be45e7e6b532d4d768f00000000f2680982f333fcc9afa7f9a5e2a84dc54b7fe10605cd187362980b3aa882e9683be21353ab80011c813e1fc0".into();

        let next_block_bits = block_on(btc_testnet_next_block_bits(
            "tBTC",
            current_header.time,
            SPVBlockHeader::from_block_header_and_height(&last_header, 201594),
            &storage,
        ))
        .unwrap();
        assert_eq!(next_block_bits, BlockHeaderBits::Compact(486604799.into()));

        // test testnet retarget bits

        // https://live.blockcypher.com/btc-testnet/block/0000000000376bb71314321c45de3015fe958543afcbada242a3b1b072498e38/
        let current_header: BlockHeader = "02000000ee689e4dcdc3c7dac591b98e1e4dc83aae03ff9fb9d469d704a64c0100000000bfffaded2a67821eb5729b362d613747e898d08d6c83b5704646c26c13146f4c6de91353c02a601b3a817f87".into();
        // https://live.blockcypher.com/btc-testnet/block/00000000014ca604d769d4b99fff03ae3ac84d1e8eb991c5dac7c3cd4d9e68ee/
        let last_header: BlockHeader = "02000000a9dccfcf372d6ce6ae784786ea94d20ce174e093520d779348e5a9000000000077c037863a0134ac05a8c19d258d6c03c225043a08687c90813e8352a144d68035e81353ab80011ca71f3849".into();

        let next_block_bits = block_on(btc_testnet_next_block_bits(
            "tBTC",
            current_header.time,
            SPVBlockHeader::from_block_header_and_height(&last_header, 201599),
            &storage,
        ))
        .unwrap();
        assert_eq!(next_block_bits, BlockHeaderBits::Compact(459287232.into()));
    }
}
