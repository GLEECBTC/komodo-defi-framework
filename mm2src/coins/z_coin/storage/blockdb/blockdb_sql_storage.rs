use crate::z_coin::storage::{
    scan_cached_block, validate_chain, BlockDbImpl, BlockProcessingMode, CompactBlockRow, LockedNotesStorage,
    ZcoinStorageRes,
};
use crate::z_coin::tx_history_events::ZCoinTxHistoryEventStreamer;
use crate::z_coin::z_balance_streaming::ZCoinBalanceEventStreamer;
use crate::z_coin::z_coin_errors::ZcoinStorageError;
use crate::z_coin::ZcoinConsensusParams;

use common::async_blocking;
use db_common::sqlite::rusqlite::{params, Connection};
use db_common::sqlite::{query_single_row, run_optimization_pragmas, rusqlite};
use itertools::Itertools;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_event_stream::DeriveStreamerId;
use protobuf::Message;
use std::sync::{Arc, Mutex};
use zcash_client_backend::data_api::error::Error as ChainError;
use zcash_client_backend::proto::compact_formats::CompactBlock;
use zcash_client_sqlite::error::{SqliteClientError as ZcashClientError, SqliteClientError};
use zcash_extras::NoteId;
use zcash_extras::WalletRead;
use zcash_primitives::block::BlockHash;
use zcash_primitives::consensus::BlockHeight;

impl From<ZcashClientError> for ZcoinStorageError {
    fn from(value: ZcashClientError) -> Self {
        match value {
            SqliteClientError::CorruptedData(err) => Self::CorruptedData(err),
            SqliteClientError::IncorrectHrpExtFvk => Self::IncorrectHrpExtFvk,
            SqliteClientError::InvalidNote => Self::InvalidNote(value.to_string()),
            SqliteClientError::InvalidNoteId => Self::InvalidNoteId,
            SqliteClientError::TableNotEmpty => Self::TableNotEmpty(value.to_string()),
            SqliteClientError::Bech32(err) => Self::DecodingError(err.to_string()),
            SqliteClientError::Base58(err) => Self::DecodingError(err.to_string()),
            SqliteClientError::DbError(err) => Self::DecodingError(err.to_string()),
            SqliteClientError::Io(err) => Self::IoError(err.to_string()),
            SqliteClientError::InvalidMemo(err) => Self::InvalidMemo(err.to_string()),
            SqliteClientError::BackendError(err) => Self::BackendError(err.to_string()),
        }
    }
}

impl From<ChainError<NoteId>> for ZcoinStorageError {
    fn from(value: ChainError<NoteId>) -> Self {
        Self::SqliteError(ZcashClientError::from(value))
    }
}

impl BlockDbImpl {
    #[cfg(not(test))]
    pub async fn new(ctx: &MmArc, ticker: String) -> ZcoinStorageRes<Self> {
        let path = ctx.global_dir().join(format!("{ticker}_cache.db"));
        async_blocking(move || {
            mm2_io::fs::create_parents(&path).map_err(|err| ZcoinStorageError::IoError(err.to_string()))?;
            let conn = Connection::open(path).map_to_mm(|err| ZcoinStorageError::DbError(err.to_string()))?;
            let conn = Arc::new(Mutex::new(conn));
            let conn_lock = conn.lock().unwrap();
            run_optimization_pragmas(&conn_lock).map_to_mm(|err| ZcoinStorageError::DbError(err.to_string()))?;
            conn_lock
                .execute(
                    "CREATE TABLE IF NOT EXISTS compactblocks (
            height INTEGER PRIMARY KEY,
            data BLOB NOT NULL
        )",
                    [],
                )
                .map_to_mm(|err| ZcoinStorageError::DbError(err.to_string()))?;
            drop(conn_lock);

            Ok(Self { db: conn, ticker })
        })
        .await
    }

    #[cfg(test)]
    pub(crate) async fn new(ctx: &MmArc, ticker: String) -> ZcoinStorageRes<Self> {
        let ctx = ctx.clone();
        async_blocking(move || {
            let conn = ctx
                .sqlite_connection
                .get()
                .cloned()
                .unwrap_or_else(|| Arc::new(Mutex::new(Connection::open_in_memory().unwrap())));
            let conn_lock = conn.lock().unwrap();
            run_optimization_pragmas(&conn_lock).map_err(|err| ZcoinStorageError::DbError(err.to_string()))?;
            conn_lock
                .execute(
                    "CREATE TABLE IF NOT EXISTS compactblocks (
            height INTEGER PRIMARY KEY,
            data BLOB NOT NULL
        )",
                    [],
                )
                .map_to_mm(|err| ZcoinStorageError::DbError(err.to_string()))?;
            drop(conn_lock);

            Ok(BlockDbImpl { db: conn, ticker })
        })
        .await
    }

    pub(crate) async fn get_latest_block(&self) -> ZcoinStorageRes<u32> {
        let db = self.db.clone();
        Ok(async_blocking(move || {
            query_single_row(
                &db.lock().unwrap(),
                "SELECT height FROM compactblocks ORDER BY height DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
        })
        .await
        .map_to_mm(|err| ZcoinStorageError::DbError(err.to_string()))?
        .unwrap_or(0))
    }

    pub(crate) async fn insert_block(&self, height: u32, cb_bytes: Vec<u8>) -> ZcoinStorageRes<usize> {
        let db = self.db.clone();
        async_blocking(move || {
            let db = db.lock().unwrap();
            let insert = db
                .prepare("INSERT INTO compactblocks (height, data) VALUES (?, ?)")
                .map_to_mm(|err| ZcoinStorageError::AddToStorageErr(err.to_string()))?
                .execute(params![height, cb_bytes])
                .map_to_mm(|err| ZcoinStorageError::AddToStorageErr(err.to_string()))?;

            Ok(insert)
        })
        .await
    }

    pub(crate) async fn rewind_to_height(&self, height: BlockHeight) -> ZcoinStorageRes<usize> {
        let db = self.db.clone();
        async_blocking(move || {
            db.lock()
                .unwrap()
                .execute("DELETE from compactblocks WHERE height > ?1", [u32::from(height)])
                .map_to_mm(|err| ZcoinStorageError::RemoveFromStorageErr(err.to_string()))
        })
        .await
    }

    pub(crate) async fn get_earliest_block(&self) -> ZcoinStorageRes<u32> {
        let db = self.db.clone();
        Ok(async_blocking(move || {
            query_single_row(
                &db.lock().unwrap(),
                "SELECT MIN(height) from compactblocks",
                [],
                |row| row.get::<_, Option<u32>>(0),
            )
        })
        .await
        .map_to_mm(|err| ZcoinStorageError::GetFromStorageError(err.to_string()))?
        .flatten()
        .unwrap_or(0))
    }

    pub(crate) async fn query_blocks_by_limit(
        &self,
        from_height: BlockHeight,
        limit: Option<u32>,
    ) -> ZcoinStorageRes<Vec<rusqlite::Result<CompactBlockRow>>> {
        let db = self.db.clone();
        async_blocking(move || {
            // Fetch the CompactBlocks we need to scan
            let db = db.lock().unwrap();
            let mut stmt_blocks = db
                .prepare(
                    "SELECT height, data FROM compactblocks WHERE height > ? ORDER BY height ASC \
        LIMIT ?",
                )
                .map_to_mm(|err| ZcoinStorageError::AddToStorageErr(err.to_string()))?;

            let rows = stmt_blocks
                .query_map(params![u32::from(from_height), limit.unwrap_or(u32::MAX),], |row| {
                    Ok(CompactBlockRow {
                        height: BlockHeight::from_u32(row.get(0)?),
                        data: row.get(1)?,
                    })
                })
                .map_to_mm(|err| ZcoinStorageError::AddToStorageErr(err.to_string()))?;

            Ok(rows.collect_vec())
        })
        .await
    }

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
            BlockProcessingMode::Scan(data, _) => {
                let data = data.inner();
                data.block_height_extrema().await.map(|opt| {
                    opt.map(|(_, max)| max)
                        .unwrap_or(BlockHeight::from_u32(params.sapling_activation_height) - 1)
                })?
            },
        };

        let rows = self.query_blocks_by_limit(from_height, limit).await?;

        let mut prev_height = from_height;
        let mut prev_hash: Option<BlockHash> = validate_from.map(|(_, hash)| hash);

        for row_result in rows {
            let cbr = row_result.map_err(|err| ZcoinStorageError::AddToStorageErr(err.to_string()))?;
            let block = CompactBlock::parse_from_bytes(&cbr.data)
                .map_err(|err| ZcoinStorageError::ChainError(err.to_string()))?;

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
