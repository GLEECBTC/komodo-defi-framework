use crate::z_coin::storage::{
    scan_cached_block, validate_chain, BlockDbImpl, BlockProcessingMode, CompactBlockRow, LockedNotesStorage,
    ZcoinConsensusParams, ZcoinStorageRes,
};
use crate::z_coin::tx_history_events::ZCoinTxHistoryEventStreamer;
use crate::z_coin::z_balance_streaming::ZCoinBalanceEventStreamer;
use crate::z_coin::z_coin_errors::ZcoinStorageError;

use async_trait::async_trait;
use mm2_core::mm_ctx::MmArc;
use mm2_db::indexed_db::{
    BeBigUint, ConstructibleDb, DbIdentifier, DbInstance, DbLocked, DbUpgrader, IndexedDb, IndexedDbBuilder,
    InitDbResult, MultiIndex, OnUpgradeResult, TableSignature,
};
use mm2_err_handle::prelude::*;
use mm2_event_stream::DeriveStreamerId;
use protobuf::Message;
use zcash_client_backend::proto::compact_formats::CompactBlock;
use zcash_extras::WalletRead;
use zcash_primitives::block::BlockHash;
use zcash_primitives::consensus::BlockHeight;

const DB_NAME: &str = "z_compactblocks_cache";
const DB_VERSION: u32 = 1;

pub type BlockDbInnerLocked<'a> = DbLocked<'a, BlockDbInner>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BlockDbTable {
    height: u32,
    data: Vec<u8>,
    ticker: String,
}

impl BlockDbTable {
    pub const TICKER_HEIGHT_INDEX: &'static str = "ticker_height_index";
}

impl TableSignature for BlockDbTable {
    const TABLE_NAME: &'static str = "compactblocks";

    fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
        if let (0, 1) = (old_version, new_version) {
            let table = upgrader.create_table(Self::TABLE_NAME)?;
            table.create_multi_index(Self::TICKER_HEIGHT_INDEX, &["ticker", "height"], true)?;
            table.create_index("ticker", false)?;
            table.create_index("height", false)?;
        }
        Ok(())
    }
}

pub struct BlockDbInner(IndexedDb);

#[async_trait]
impl DbInstance for BlockDbInner {
    const DB_NAME: &'static str = DB_NAME;

    async fn init(db_id: DbIdentifier) -> InitDbResult<Self> {
        let inner = IndexedDbBuilder::new(db_id)
            .with_version(DB_VERSION)
            .with_table::<BlockDbTable>()
            .build()
            .await?;

        Ok(Self(inner))
    }
}

impl BlockDbInner {
    pub fn get_inner(&self) -> &IndexedDb {
        &self.0
    }
}

impl BlockDbImpl {
    pub async fn new(ctx: &MmArc, ticker: String) -> ZcoinStorageRes<Self> {
        Ok(Self {
            db: ConstructibleDb::new(ctx).into_shared(),
            ticker,
        })
    }

    async fn lock_db(&self) -> ZcoinStorageRes<BlockDbInnerLocked<'_>> {
        self.db
            .get_or_initialize()
            .await
            .mm_err(|err| ZcoinStorageError::DbError(err.to_string()))
    }

    /// Get latest block of the current active ZCOIN.
    pub async fn get_latest_block(&self) -> ZcoinStorageRes<u32> {
        let ticker = self.ticker.clone();
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let block_db = db_transaction.table::<BlockDbTable>().await.map_mm_err()?;
        let maybe_height = block_db
            .cursor_builder()
            .only("ticker", &ticker)
            .map_mm_err()?
            .bound("height", 0u32, u32::MAX)
            .reverse()
            .where_first()
            .open_cursor(BlockDbTable::TICKER_HEIGHT_INDEX)
            .await
            .map_mm_err()?
            .next()
            .await
            .map_mm_err()?;

        Ok(maybe_height.map(|(_, item)| item.height).unwrap_or_else(|| 0))
    }

    /// Insert new block to BlockDbTable given the provided data.
    pub async fn insert_block(&self, height: u32, cb_bytes: Vec<u8>) -> ZcoinStorageRes<usize> {
        let ticker = self.ticker.clone();
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let block_db = db_transaction.table::<BlockDbTable>().await.map_mm_err()?;

        let indexes = MultiIndex::new(BlockDbTable::TICKER_HEIGHT_INDEX)
            .with_value(&ticker)
            .map_mm_err()?
            .with_value(BeBigUint::from(height))
            .map_mm_err()?;
        let block = BlockDbTable {
            height,
            data: cb_bytes,
            ticker,
        };

        Ok(block_db
            .add_item_or_ignore_by_unique_multi_index(indexes, &block)
            .await
            .map_mm_err()?
            .get_id() as usize)
    }

    /// Asynchronously rewinds the storage to a specified block height, effectively
    /// removing data beyond the specified height from the storage.
    pub async fn rewind_to_height(&self, height: BlockHeight) -> ZcoinStorageRes<usize> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let block_db = db_transaction.table::<BlockDbTable>().await.map_mm_err()?;

        let blocks = block_db
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .bound("height", 0u32, u32::MAX)
            .reverse()
            .open_cursor(BlockDbTable::TICKER_HEIGHT_INDEX)
            .await
            .map_mm_err()?
            .collect()
            .await
            .map_mm_err()?;

        for (_, block) in &blocks {
            if block.height > u32::from(height) {
                block_db
                    .delete_item_by_unique_multi_index(
                        MultiIndex::new(BlockDbTable::TICKER_HEIGHT_INDEX)
                            .with_value(&self.ticker)
                            .map_mm_err()?
                            .with_value(block.height)
                            .map_mm_err()?,
                    )
                    .await
                    .map_mm_err()?;
            }
        }

        Ok(blocks.last().map(|(_, block)| block.height).unwrap_or_default() as usize)
    }

    #[allow(unused)]
    pub(crate) async fn get_earliest_block(&self) -> ZcoinStorageRes<u32> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let block_db = db_transaction.table::<BlockDbTable>().await.map_mm_err()?;
        let maybe_min_block = block_db
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .bound("height", 0u32, u32::MAX)
            .where_first()
            .open_cursor(BlockDbTable::TICKER_HEIGHT_INDEX)
            .await
            .map_mm_err()?
            .next()
            .await
            .map_mm_err()?;

        Ok(maybe_min_block.map(|(_, b)| b.height).unwrap_or(0))
    }

    /// Queries and retrieves a list of `CompactBlockRow` records from the database, starting
    /// from a specified block height and optionally limited by a maximum number of blocks.
    pub async fn query_blocks_by_limit(
        &self,
        from_height: BlockHeight,
        limit: Option<u32>,
    ) -> ZcoinStorageRes<Vec<CompactBlockRow>> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let block_db = db_transaction.table::<BlockDbTable>().await.map_mm_err()?;

        // Fetch CompactBlocks block_db are needed for scanning.
        let min = u32::from(from_height + 1);
        let mut maybe_blocks = block_db
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .bound("height", min, u32::MAX)
            .open_cursor(BlockDbTable::TICKER_HEIGHT_INDEX)
            .await
            .map_mm_err()?;

        let mut blocks_to_scan = vec![];
        while let Some((_, block)) = maybe_blocks.next().await.map_mm_err()? {
            if let Some(limit) = limit {
                if blocks_to_scan.len() > limit as usize {
                    break;
                }
            };

            blocks_to_scan.push(CompactBlockRow {
                height: block.height.into(),
                data: block.data,
            });
        }

        Ok(blocks_to_scan)
    }

    /// Processes blockchain blocks with a specified mode of operation, such as validation or scanning.
    ///
    /// Processes blocks based on the provided `BlockProcessingMode` and other parameters,
    /// which may include a starting block height, validation criteria, and a processing limit.
    pub(crate) async fn process_blocks_with_mode(
        &self,
        params: ZcoinConsensusParams,
        mode: BlockProcessingMode,
        validate_from: Option<(BlockHeight, BlockHash)>,
        limit: Option<u32>,
        locked_notes_db: &LockedNotesStorage,
    ) -> ZcoinStorageRes<()> {
        let ticker = self.ticker.to_owned();
        let mut from_height = match &mode {
            BlockProcessingMode::Validate => validate_from
                .map(|(height, _)| height)
                .unwrap_or(BlockHeight::from_u32(params.sapling_activation_height) - 1),
            BlockProcessingMode::Scan(data, _) => data.inner().block_height_extrema().await.map(|opt| {
                opt.map(|(_, max)| max)
                    .unwrap_or(BlockHeight::from_u32(params.sapling_activation_height) - 1)
            })?,
        };
        let mut prev_height = from_height;
        let mut prev_hash: Option<BlockHash> = validate_from.map(|(_, hash)| hash);

        let blocks_to_scan = self.query_blocks_by_limit(from_height, limit).await?;
        for block in blocks_to_scan {
            let cbr = block;
            let block = CompactBlock::parse_from_bytes(&cbr.data)
                .map_to_mm(|err| ZcoinStorageError::DecodingError(err.to_string()))?;

            if block.height() != cbr.height {
                return MmError::err(ZcoinStorageError::CorruptedData(format!(
                    "{ticker}, Block height {} did not match row's height field value {}",
                    block.height(),
                    cbr.height
                )));
            }

            match &mode.clone() {
                BlockProcessingMode::Validate => {
                    validate_chain(block, &mut prev_height, &mut prev_hash).await?;
                },
                BlockProcessingMode::Scan(data, streaming_manager) => {
                    let txs = scan_cached_block(data, &params, &block, locked_notes_db, &mut from_height).await?;
                    if !txs.is_empty() {
                        // Stream out the new transactions.
                        streaming_manager
                            .send(&ZCoinTxHistoryEventStreamer::derive_streamer_id(&ticker), txs)
                            .ok();
                        // And also stream balance changes.
                        streaming_manager
                            .send(&ZCoinBalanceEventStreamer::derive_streamer_id(&ticker), ())
                            .ok();
                    };
                },
            }
        }

        Ok(())
    }
}
