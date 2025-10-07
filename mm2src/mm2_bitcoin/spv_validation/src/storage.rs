use async_trait::async_trait;
use chain::BlockHeader;
use derive_more::Display;
use primitives::hash::H256;
use std::collections::HashMap;

#[derive(Clone, Debug, Display, Eq, PartialEq)]
pub enum BlockHeaderStorageError {
    #[display(fmt = "Can't add to the storage for {coin} - reason: {reason}")]
    AddToStorageError {
        coin: String,
        reason: String,
    },
    #[display(fmt = "Can't get from the storage for {coin} - reason: {reason}")]
    GetFromStorageError {
        coin: String,
        reason: String,
    },
    #[display(fmt = "Can't retrieve the table from the storage for {coin} - reason: {reason}")]
    CantRetrieveTableError {
        coin: String,
        reason: String,
    },
    #[display(
        fmt = "Unable to delete block headers from_height: {from_height} to_height: {to_height} from storage for {coin} - reason: {reason}"
    )]
    UnableToDeleteHeaders {
        from_height: u64,
        to_height: u64,
        coin: String,
        reason: String,
    },
    #[display(fmt = "Can't query from the storage - query: {query} - reason: {reason}")]
    QueryError {
        query: String,
        reason: String,
    },
    #[display(fmt = "Can't init from the storage - coin: {coin} - reason: {reason}")]
    InitializationError {
        coin: String,
        reason: String,
    },
    #[display(fmt = "Can't decode/deserialize from storage for {coin} - reason: {reason}")]
    DecodeError {
        coin: String,
        reason: String,
    },
    Internal(String),
}

impl BlockHeaderStorageError {
    pub fn init_err(ticker: &str, reason: String) -> BlockHeaderStorageError {
        BlockHeaderStorageError::InitializationError {
            coin: ticker.to_string(),
            reason,
        }
    }

    pub fn add_err(ticker: &str, reason: String) -> BlockHeaderStorageError {
        BlockHeaderStorageError::AddToStorageError {
            coin: ticker.to_string(),
            reason,
        }
    }

    pub fn table_err(ticker: &str, reason: String) -> BlockHeaderStorageError {
        BlockHeaderStorageError::CantRetrieveTableError {
            coin: ticker.to_string(),
            reason,
        }
    }

    pub fn get_err(ticker: &str, reason: String) -> BlockHeaderStorageError {
        BlockHeaderStorageError::GetFromStorageError {
            coin: ticker.to_string(),
            reason,
        }
    }

    pub fn delete_err(ticker: &str, reason: String, from_height: u64, to_height: u64) -> BlockHeaderStorageError {
        BlockHeaderStorageError::UnableToDeleteHeaders {
            from_height,
            to_height,
            coin: ticker.to_string(),
            reason,
        }
    }
}

#[async_trait]
pub trait BlockHeaderStorageOps: Send + Sync + 'static {
    /// Initializes collection/tables in storage for a specified coin
    async fn init(&self) -> Result<(), BlockHeaderStorageError>;

    async fn is_initialized_for(&self) -> Result<bool, BlockHeaderStorageError>;

    // Adds multiple block headers to the selected coin's header storage
    // Should store it as `COIN_HEIGHT=hex_string`
    // use this function for headers that comes from `blockchain_block_headers`
    async fn add_block_headers_to_storage(
        &self,
        headers: HashMap<u64, BlockHeader>,
    ) -> Result<(), BlockHeaderStorageError>;

    /// Gets the block header by height from the selected coin's storage as BlockHeader
    async fn get_block_header(&self, height: u64) -> Result<Option<BlockHeader>, BlockHeaderStorageError>;

    /// Gets the block header by height from the selected coin's storage as hex
    async fn get_block_header_raw(&self, height: u64) -> Result<Option<String>, BlockHeaderStorageError>;

    async fn get_last_block_height(&self) -> Result<Option<u64>, BlockHeaderStorageError>;

    async fn get_last_block_header_with_non_max_bits(
        &self,
        max_bits: u32,
    ) -> Result<Option<BlockHeader>, BlockHeaderStorageError>;

    async fn get_block_height_by_hash(&self, hash: H256) -> Result<Option<i64>, BlockHeaderStorageError>;

    async fn remove_headers_from_storage(&self, from: u64, to: u64) -> Result<(), BlockHeaderStorageError>;

    async fn is_table_empty(&self) -> Result<(), BlockHeaderStorageError>;
}
