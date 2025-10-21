use std::collections::HashSet;

use crate::z_coin::{ZCoin, ZTxHistoryError};
use common::PagingOptionsEnum;
use mm2_err_handle::prelude::*;
use primitives::hash::H256;
use std::convert::TryInto;
use zcash_primitives::transaction::TxId;

cfg_wasm32!(
    use crate::z_coin::storage::wasm::tables::{WalletDbBlocksTable, WalletDbReceivedNotesTable, WalletDbTransactionsTable};
    use crate::MarketCoinOps;
    use mm2_number::BigInt;
    use mm2_db::indexed_db::cursor_prelude::CursorError;
    use num_traits::ToPrimitive;
);

cfg_native!(
    use crate::z_coin::BLOCKS_TABLE;
    use common::async_blocking;
    use db_common::sqlite::sql_builder::{name, SqlBuilder, SqlName};
    use db_common::sqlite::rusqlite::Error as SqliteError;
    use db_common::sqlite::rusqlite::Row;
    use db_common::sqlite::offset_by_id;
    use db_common::sqlite::rusqlite::types::Type;
);

#[cfg(not(target_arch = "wasm32"))]
const TRANSACTIONS_TABLE: &str = "transactions";

pub(crate) struct ZCoinTxHistoryItem {
    pub(crate) tx_hash: H256,
    pub(crate) internal_id: i64,
    pub(crate) height: i64,
    pub(crate) timestamp: i64,
    pub(crate) received_amount: i64,
    pub(crate) spent_amount: i64,
}

pub(crate) struct ZTxHistoryRes {
    pub(crate) total_tx_count: u32,
    pub(crate) transactions: Vec<ZCoinTxHistoryItem>,
    pub(crate) skipped: usize,
}

/// Fetch transaction history from the database.
#[cfg(target_arch = "wasm32")]
pub(crate) async fn fetch_tx_history_from_db(
    z: &ZCoin,
    limit: usize,
    paging_options: PagingOptionsEnum<i64>,
) -> Result<ZTxHistoryRes, MmError<ZTxHistoryError>> {
    let wallet_db = z.z_fields.light_wallet_db.clone();
    let wallet_db = wallet_db.db.lock_db().await.unwrap();
    let db_transaction = wallet_db.get_inner().transaction().await.map_mm_err()?;
    let tx_table = db_transaction.table::<WalletDbTransactionsTable>().await.map_mm_err()?;
    let total_tx_count = tx_table.count_all().await.map_mm_err()? as u32;
    let offset = match paging_options {
        PagingOptionsEnum::PageNumber(page_number) => ((page_number.get() - 1) * limit) as i64,
        PagingOptionsEnum::FromId(tx_id) => {
            if tx_id > total_tx_count as i64 {
                return MmError::err(ZTxHistoryError::FromIdDoesNotExist(tx_id));
            }
            (total_tx_count as i64 - tx_id) + 1
        },
    };

    // Fetch transactions
    let txs = tx_table
        .cursor_builder()
        .only("ticker", z.ticker())
        .map_mm_err()?
        .offset(offset as u32)
        .limit(limit)
        .reverse()
        .open_cursor("ticker")
        .await
        .map_mm_err()?
        .collect()
        .await
        .map_mm_err()?;

    // Fetch received notes
    let rn_table = db_transaction
        .table::<WalletDbReceivedNotesTable>()
        .await
        .map_mm_err()?;
    let received_notes = rn_table
        .cursor_builder()
        .only("ticker", z.ticker())
        .map_mm_err()?
        .open_cursor("ticker")
        .await
        .map_mm_err()?
        .collect()
        .await
        .map_mm_err()?;

    // Fetch blocks
    let blocks_table = db_transaction.table::<WalletDbBlocksTable>().await.map_mm_err()?;
    let blocks = blocks_table
        .cursor_builder()
        .only("ticker", z.ticker())
        .map_mm_err()?
        .open_cursor("ticker")
        .await
        .map_mm_err()?
        .collect()
        .await
        .map_mm_err()?;

    // Process transactions and construct tx_details
    let mut tx_details = Vec::new();
    for (tx_id, tx) in txs {
        if let Some((_, WalletDbBlocksTable { height, time, .. })) = blocks
            .iter()
            .find(|(_, block)| tx.block.map(|b| b == block.height).unwrap_or_default())
        {
            let internal_id = tx_id;
            let mut received_amount = 0;
            let mut spent_amount = 0;

            for (_, note) in &received_notes {
                if internal_id == note.tx {
                    received_amount += note.value.to_u64().ok_or_else(|| {
                        ZTxHistoryError::IndexedDbError("Number is too large to fit in a u64".to_string())
                    })? as i64;
                }

                // detecting spent amount by "spent" field in received_notes table
                if let Some(spent) = &note.spent {
                    if &BigInt::from(internal_id) == spent {
                        spent_amount += note.value.to_u64().ok_or_else(|| {
                            ZTxHistoryError::IndexedDbError("Number is too large to fit in a u64".to_string())
                        })? as i64;
                    }
                }
            }

            let mut tx_hash: [u8; 32] = tx
                .txid
                .try_into()
                .map_err(|_| ZTxHistoryError::IndexedDbError("Expected 32 bytes for transaction hash".to_string()))?;
            tx_hash.reverse();

            tx_details.push(ZCoinTxHistoryItem {
                tx_hash: H256::from(tx_hash),
                internal_id: internal_id as i64,
                height: *height as i64,
                timestamp: *time as i64,
                received_amount,
                spent_amount,
            });
        }
    }

    Ok(ZTxHistoryRes {
        transactions: tx_details,
        total_tx_count,
        skipped: offset as usize,
    })
}

#[cfg(not(target_arch = "wasm32"))]
impl ZCoinTxHistoryItem {
    fn try_from_sql_row(row: &Row<'_>) -> Result<Self, SqliteError> {
        let tx_bytes: Vec<u8> = row.get(0)?;
        let mut tx_hash: [u8; 32] = tx_bytes.try_into().map_err(|_| {
            SqliteError::FromSqlConversionFailure(
                0,
                Type::Blob,
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "Expected 32 bytes for transaction hash",
                )),
            )
        })?;
        tx_hash.reverse();

        Ok(ZCoinTxHistoryItem {
            tx_hash: H256::from(tx_hash),
            internal_id: row.get(1)?,
            height: row.get(2)?,
            timestamp: row.get(3)?,
            received_amount: row.get(4)?,
            spent_amount: row.get(5)?,
        })
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn fetch_tx_history_from_db(
    z: &ZCoin,
    limit: usize,
    paging_options: PagingOptionsEnum<i64>,
) -> Result<ZTxHistoryRes, MmError<ZTxHistoryError>> {
    let wallet_db = z.z_fields.light_wallet_db.clone();
    async_blocking(move || {
        let db_guard = wallet_db.db.inner();
        let db_guard = db_guard.lock().unwrap();
        let conn = db_guard.sql_conn();

        let total_sql = SqlBuilder::select_from(TRANSACTIONS_TABLE)
            .field("COUNT(id_tx)")
            .sql()
            .expect("valid SQL");
        let total_tx_count = conn.query_row(&total_sql, [], |row| row.get(0))?;

        let mut sql_builder = SqlBuilder::select_from(name!(TRANSACTIONS_TABLE; "txes"));
        sql_builder
            .field("txes.txid")
            .field("txes.id_tx as internal_id")
            .field("txes.block as block");

        let offset = match paging_options {
            PagingOptionsEnum::PageNumber(page) => (page.get() - 1) * limit,
            PagingOptionsEnum::FromId(id) => {
                offset_by_id(conn, &sql_builder, [id], "id_tx", "block DESC, id_tx ASC", "id_tx = ?1")?
                    .ok_or(ZTxHistoryError::FromIdDoesNotExist(id))?
            },
        };

        let sql = sql_builder
            .field("blocks.time")
            .field("COALESCE(rn.received_amount, 0)")
            .field("COALESCE(sn.sent_amount, 0)")
            .left()
            .join("(SELECT tx, SUM(value) as received_amount FROM received_notes GROUP BY tx) as rn")
            .on("txes.id_tx = rn.tx")
            // detecting spent amount by "spent" field in received_notes table
            .join("(SELECT spent, SUM(value) as sent_amount FROM received_notes GROUP BY spent) as sn")
            .on("txes.id_tx = sn.spent")
            .join(BLOCKS_TABLE)
            .on("txes.block = blocks.height")
            .group_by("internal_id")
            .order_by("block", true)
            .order_by("internal_id", false)
            .offset(offset)
            .limit(limit)
            .sql()
            .expect("valid query");

        let sql_items = conn
            .prepare(&sql)?
            .query_map([], ZCoinTxHistoryItem::try_from_sql_row)?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ZTxHistoryRes {
            transactions: sql_items,
            total_tx_count,
            skipped: offset,
        })
    })
    .await
}

#[cfg(target_arch = "wasm32")]
pub(crate) async fn fetch_txs_from_db(
    z: &ZCoin,
    tx_hashes: HashSet<TxId>,
) -> Result<Vec<ZCoinTxHistoryItem>, MmError<ZTxHistoryError>> {
    let wallet_db = z.z_fields.light_wallet_db.clone();
    let wallet_db = wallet_db.db.lock_db().await.unwrap();
    let db_transaction = wallet_db.get_inner().transaction().await.map_mm_err()?;
    let tx_table = db_transaction.table::<WalletDbTransactionsTable>().await.map_mm_err()?;

    let limit = tx_hashes.len();
    let condition = {
        // Convert TxIds to Vec<u8>s for comparison.
        let tx_hashes: HashSet<_> = tx_hashes.into_iter().map(|txid| txid.0.to_vec()).collect();
        move |tx| {
            let tx = serde_json::from_value::<WalletDbTransactionsTable>(tx)
                .map_to_mm(|err| CursorError::ErrorDeserializingItem(err.to_string()))?;
            Ok(tx_hashes.contains(&tx.txid))
        }
    };

    // Fetch transactions
    let txs = tx_table
        .cursor_builder()
        .only("ticker", z.ticker())
        .map_mm_err()?
        // We need to explicitly set a limit since `where_` implicitly sets a limit of 1 if no limit is set.
        // TODO: Remove when `where_` doesn't exhibit this behavior.
        .limit(limit)
        .where_(condition)
        .reverse()
        .open_cursor("ticker")
        .await
        .map_mm_err()?
        .collect()
        .await
        .map_mm_err()?;

    // Fetch received notes
    let rn_table = db_transaction
        .table::<WalletDbReceivedNotesTable>()
        .await
        .map_mm_err()?;
    let received_notes = rn_table
        .cursor_builder()
        .only("ticker", z.ticker())
        .map_mm_err()?
        .open_cursor("ticker")
        .await
        .map_mm_err()?
        .collect()
        .await
        .map_mm_err()?;

    // Fetch blocks
    let blocks_table = db_transaction.table::<WalletDbBlocksTable>().await.map_mm_err()?;
    let blocks = blocks_table
        .cursor_builder()
        .only("ticker", z.ticker())
        .map_mm_err()?
        .open_cursor("ticker")
        .await
        .map_mm_err()?
        .collect()
        .await
        .map_mm_err()?;

    // Process transactions and construct tx_details
    let mut transactions = Vec::new();
    for (tx_id, tx) in txs {
        if let Some((_, WalletDbBlocksTable { height, time, .. })) = blocks
            .iter()
            .find(|(_, block)| tx.block.map(|b| b == block.height).unwrap_or_default())
        {
            let internal_id = tx_id;
            let mut received_amount = 0;
            let mut spent_amount = 0;

            for (_, note) in &received_notes {
                if internal_id == note.tx {
                    received_amount += note.value.to_u64().ok_or_else(|| {
                        ZTxHistoryError::IndexedDbError("Number is too large to fit in a u64".to_string())
                    })? as i64;
                }

                // detecting spent amount by "spent" field in received_notes table
                if let Some(spent) = &note.spent {
                    if &BigInt::from(internal_id) == spent {
                        spent_amount += note.value.to_u64().ok_or_else(|| {
                            ZTxHistoryError::IndexedDbError("Number is too large to fit in a u64".to_string())
                        })? as i64;
                    }
                }
            }

            let mut tx_hash: [u8; 32] = tx
                .txid
                .try_into()
                .map_err(|_| ZTxHistoryError::IndexedDbError("Expected 32 bytes for transaction hash".to_string()))?;
            tx_hash.reverse();

            transactions.push(ZCoinTxHistoryItem {
                tx_hash: H256::from(tx_hash),
                internal_id: internal_id as i64,
                height: *height as i64,
                timestamp: *time as i64,
                received_amount,
                spent_amount,
            });
        }
    }

    Ok(transactions)
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) async fn fetch_txs_from_db(
    z: &ZCoin,
    tx_hashes: HashSet<TxId>,
) -> Result<Vec<ZCoinTxHistoryItem>, MmError<ZTxHistoryError>> {
    let wallet_db = z.z_fields.light_wallet_db.clone();
    async_blocking(move || {
        let sql_query = SqlBuilder::select_from(name!(TRANSACTIONS_TABLE; "txes"))
            .field("txes.txid as tx_hash")
            .field("txes.id_tx as internal_id")
            .field("txes.block as block")
            .field("blocks.time")
            .field("COALESCE(rn.received_amount, 0)")
            .field("COALESCE(sn.sent_amount, 0)")
            .and_where_in_quoted(
                // Make sure the tx hash from the DB is lowercase,
                "lower(hex(tx_hash))",
                &tx_hashes
                    .iter()
                    // as well as the tx hashes we are looking for.
                    .map(|tx_hash| hex::encode(tx_hash.0).to_lowercase())
                    .collect::<Vec<_>>(),
            )
            .left()
            .join("(SELECT tx, SUM(value) as received_amount FROM received_notes GROUP BY tx) as rn")
            .on("txes.id_tx = rn.tx")
            .join("(SELECT spent, SUM(value) as sent_amount FROM received_notes GROUP BY spent) as sn")
            .on("txes.id_tx = sn.spent")
            .join(BLOCKS_TABLE)
            .on("txes.block = blocks.height")
            .group_by("internal_id")
            .order_by("block", true)
            .order_by("internal_id", false)
            .sql()
            .expect("valid query");

        let txs = wallet_db
            .db
            .inner()
            .lock()
            .unwrap()
            .sql_conn()
            .prepare(&sql_query)?
            .query_map([], ZCoinTxHistoryItem::try_from_sql_row)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(txs)
    })
    .await
}
