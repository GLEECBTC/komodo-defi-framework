use async_trait::async_trait;
use chain::BlockHeader;
use common::async_blocking;
use db_common::{
    sqlite::rusqlite::Error as SqlError,
    sqlite::rusqlite::{params_from_iter, Connection, Row, ToSql},
    sqlite::string_from_row,
    sqlite::validate_table_name,
    sqlite::CHECK_TABLE_EXISTS_SQL,
};
use primitives::hash::H256;
use serialization::Reader;
use spv_validation::storage::{BlockHeaderStorageError, BlockHeaderStorageOps};
use std::collections::HashMap;
use std::convert::TryInto;
use std::num::TryFromIntError;
use std::sync::{Arc, Mutex};

pub(crate) fn block_headers_cache_table(ticker: &str) -> String {
    ticker.to_owned() + "_block_headers_cache"
}

fn get_table_name_and_validate(for_coin: &str) -> Result<String, BlockHeaderStorageError> {
    let table_name = block_headers_cache_table(for_coin);
    validate_table_name(&table_name).map_err(|e| BlockHeaderStorageError::CantRetrieveTableError {
        coin: for_coin.to_string(),
        reason: e.to_string(),
    })?;
    Ok(table_name)
}

fn create_block_header_cache_table_sql(for_coin: &str) -> Result<String, BlockHeaderStorageError> {
    let table_name = get_table_name_and_validate(for_coin)?;
    let sql = format!(
        "CREATE TABLE IF NOT EXISTS {table_name} (
            block_height INTEGER NOT NULL UNIQUE,
            hex TEXT NOT NULL,
            block_bits INTEGER NOT NULL,
            block_hash VARCHAR(255) NOT NULL UNIQUE
        );"
    );

    Ok(sql)
}

fn insert_block_header_in_cache_sql(for_coin: &str) -> Result<String, BlockHeaderStorageError> {
    let table_name = get_table_name_and_validate(for_coin)?;
    // Always update the block headers with new values just in case a chain reorganization occurs.
    let sql = format!(
        "INSERT OR REPLACE INTO {table_name} (block_height, hex, block_bits, block_hash) VALUES (?1, ?2, ?3, ?4);"
    );
    Ok(sql)
}

fn get_block_header_by_height(for_coin: &str) -> Result<String, BlockHeaderStorageError> {
    let table_name = get_table_name_and_validate(for_coin)?;
    let sql = format!("SELECT hex FROM {table_name} WHERE block_height=?1;");

    Ok(sql)
}

fn get_last_block_height_sql(for_coin: &str) -> Result<String, BlockHeaderStorageError> {
    let table_name = get_table_name_and_validate(for_coin)?;
    let sql = format!("SELECT block_height FROM {table_name} ORDER BY block_height DESC LIMIT 1;");

    Ok(sql)
}

fn get_last_block_header_with_non_max_bits_sql(
    for_coin: &str,
    max_bits: u32,
) -> Result<String, BlockHeaderStorageError> {
    let table_name = get_table_name_and_validate(for_coin)?;
    let sql = format!("SELECT hex FROM {table_name} WHERE block_bits<>{max_bits} ORDER BY block_height DESC LIMIT 1;");

    Ok(sql)
}

fn get_block_height_by_hash(for_coin: &str) -> Result<String, BlockHeaderStorageError> {
    let table_name = get_table_name_and_validate(for_coin)?;
    let sql = format!("SELECT block_height FROM {table_name} WHERE block_hash=?1;");

    Ok(sql)
}

fn remove_headers_from_to_height_sql(for_coin: &str) -> Result<String, BlockHeaderStorageError> {
    let table_name = get_table_name_and_validate(for_coin)?;
    let sql = format!(
        "DELETE FROM {table_name} WHERE block_height BETWEEN ?1 AND
    ?2;"
    );

    Ok(sql)
}

#[derive(Clone, Debug)]
pub struct SqliteBlockHeadersStorage {
    pub ticker: String,
    pub conn: Arc<Mutex<Connection>>,
}

fn query_single_row<T, P, F>(
    conn: &Connection,
    query: &str,
    params: P,
    map_fn: F,
) -> Result<Option<T>, BlockHeaderStorageError>
where
    P: db_common::sqlite::rusqlite::Params,
    F: FnOnce(&Row<'_>) -> Result<T, SqlError>,
{
    db_common::sqlite::query_single_row(conn, query, params, map_fn).map_err(|e| BlockHeaderStorageError::QueryError {
        query: query.to_string(),
        reason: e.to_string(),
    })
}

#[async_trait]
impl BlockHeaderStorageOps for SqliteBlockHeadersStorage {
    async fn init(&self) -> Result<(), BlockHeaderStorageError> {
        let coin = self.ticker.clone();
        let sql_cache = create_block_header_cache_table_sql(&coin)?;
        let selfi = self.clone();

        async_blocking(move || {
            let conn = selfi.conn.lock().unwrap();
            conn.execute(&sql_cache, [])
                .map(|_| ())
                .map_err(|e| BlockHeaderStorageError::InitializationError {
                    coin,
                    reason: e.to_string(),
                })?;
            Ok(())
        })
        .await
    }

    async fn is_initialized_for(&self) -> Result<bool, BlockHeaderStorageError> {
        let block_headers_cache_table = get_table_name_and_validate(&self.ticker)?;
        let selfi = self.clone();

        async_blocking(move || {
            let conn = selfi.conn.lock().unwrap();
            let cache_initialized = query_single_row(
                &conn,
                CHECK_TABLE_EXISTS_SQL,
                [block_headers_cache_table],
                string_from_row,
            )?;
            Ok(cache_initialized.is_some())
        })
        .await
    }

    async fn add_block_headers_to_storage(
        &self,
        headers: HashMap<u64, BlockHeader>,
    ) -> Result<(), BlockHeaderStorageError> {
        let coin = self.ticker.clone();
        let selfi = self.clone();

        async_blocking(move || {
            let mut conn = selfi.conn.lock().unwrap();
            let sql_transaction = conn
                .transaction()
                .map_err(|e| BlockHeaderStorageError::AddToStorageError {
                    coin: coin.clone(),
                    reason: e.to_string(),
                })?;

            for (height, header) in headers {
                let height = height as i64;
                let hash = header.hash().reversed().to_string();
                let raw_header = hex::encode(header.raw());
                let bits: u32 = header.bits.into();
                let block_cache_params = [
                    &height as &dyn ToSql,
                    &raw_header as &dyn ToSql,
                    &bits as &dyn ToSql,
                    &hash as &dyn ToSql,
                ];
                sql_transaction
                    .execute(&insert_block_header_in_cache_sql(&coin.clone())?, block_cache_params)
                    .map_err(|e| BlockHeaderStorageError::AddToStorageError {
                        coin: coin.clone(),
                        reason: e.to_string(),
                    })?;
            }
            sql_transaction
                .commit()
                .map_err(|e| BlockHeaderStorageError::AddToStorageError {
                    coin,
                    reason: e.to_string(),
                })?;
            Ok(())
        })
        .await
    }

    async fn get_block_header(&self, height: u64) -> Result<Option<BlockHeader>, BlockHeaderStorageError> {
        let coin = self.ticker.clone();
        if let Some(header_raw) = self.get_block_header_raw(height).await? {
            let serialized = &hex::decode(header_raw).map_err(|e| BlockHeaderStorageError::DecodeError {
                coin: coin.clone(),
                reason: e.to_string(),
            })?;
            let mut reader = Reader::new_with_coin_variant(serialized, coin.as_str().into());
            let header: BlockHeader =
                reader
                    .read()
                    .map_err(|e: serialization::Error| BlockHeaderStorageError::DecodeError {
                        coin,
                        reason: e.to_string(),
                    })?;
            return Ok(Some(header));
        }
        Ok(None)
    }

    async fn get_block_header_raw(&self, height: u64) -> Result<Option<String>, BlockHeaderStorageError> {
        let coin = self.ticker.clone();
        let params = [height as i64];
        let sql = get_block_header_by_height(&coin)?;
        let selfi = self.clone();

        async_blocking(move || {
            let conn = selfi.conn.lock().unwrap();
            query_single_row(&conn, &sql, params, string_from_row)
        })
        .await
        .map_err(|e| BlockHeaderStorageError::GetFromStorageError {
            coin,
            reason: e.to_string(),
        })
    }

    async fn get_last_block_height(&self) -> Result<Option<u64>, BlockHeaderStorageError> {
        let coin = self.ticker.clone();
        let sql = get_last_block_height_sql(&coin)?;
        let selfi = self.clone();

        async_blocking(move || {
            let conn = selfi.conn.lock().unwrap();
            query_single_row(&conn, &sql, [], |row| row.get::<_, i64>(0))
        })
        .await
        .map_err(|e| BlockHeaderStorageError::GetFromStorageError {
            coin: coin.clone(),
            reason: e.to_string(),
        })?
        .map(|h| h.try_into())
        .transpose()
        .map_err(|e: TryFromIntError| BlockHeaderStorageError::DecodeError {
            coin,
            reason: e.to_string(),
        })
    }

    async fn get_last_block_header_with_non_max_bits(
        &self,
        max_bits: u32,
    ) -> Result<Option<BlockHeader>, BlockHeaderStorageError> {
        let coin = self.ticker.clone();
        let sql = get_last_block_header_with_non_max_bits_sql(&coin, max_bits)?;
        let selfi = self.clone();

        let maybe_header_raw = async_blocking(move || {
            let conn = selfi.conn.lock().unwrap();
            query_single_row(&conn, &sql, [], string_from_row)
        })
        .await
        .map_err(|e| BlockHeaderStorageError::GetFromStorageError {
            coin: coin.clone(),
            reason: e.to_string(),
        })?;

        if let Some(header_raw) = maybe_header_raw {
            let header = BlockHeader::try_from_string_with_coin_variant(header_raw, coin.as_str().into()).map_err(
                |e: serialization::Error| BlockHeaderStorageError::DecodeError {
                    coin,
                    reason: e.to_string(),
                },
            )?;
            return Ok(Some(header));
        }
        Ok(None)
    }

    async fn get_block_height_by_hash(&self, hash: H256) -> Result<Option<i64>, BlockHeaderStorageError> {
        let coin = self.ticker.clone();
        let params = [hash.to_string()];
        let sql = get_block_height_by_hash(&coin)?;
        let selfi = self.clone();

        async_blocking(move || {
            let conn = selfi.conn.lock().unwrap();
            query_single_row(&conn, &sql, params, |row| row.get(0))
        })
        .await
        .map_err(|e| BlockHeaderStorageError::GetFromStorageError {
            coin,
            reason: e.to_string(),
        })
    }

    async fn remove_headers_from_storage(
        &self,
        from_height: u64,
        to_height: u64,
    ) -> Result<(), BlockHeaderStorageError> {
        let coin = self.ticker.clone();
        let sql = remove_headers_from_to_height_sql(&coin)?;
        let params = [from_height.to_string(), to_height.to_string()];
        let selfi = self.clone();

        async_blocking(move || {
            let conn = selfi.conn.lock().unwrap();
            conn.execute(&sql, params_from_iter(params.iter()))
        })
        .await
        .map_err(|err| BlockHeaderStorageError::UnableToDeleteHeaders {
            coin,
            from_height,
            to_height,
            reason: err.to_string(),
        })?;

        Ok(())
    }

    async fn is_table_empty(&self) -> Result<(), BlockHeaderStorageError> {
        let table_name = get_table_name_and_validate(&self.ticker).unwrap();
        let sql = format!("SELECT COUNT(block_height) FROM {table_name};");
        let conn = self.conn.lock().unwrap();
        let rows_count: u32 = conn.query_row(&sql, [], |row| row.get(0)).unwrap();
        if rows_count == 0 {
            return Ok(());
        };

        Err(BlockHeaderStorageError::table_err(
            &self.ticker,
            "Table is not empty".to_string(),
        ))
    }
}

#[cfg(test)]
impl SqliteBlockHeadersStorage {
    pub fn in_memory(ticker: String) -> Self {
        SqliteBlockHeadersStorage {
            ticker,
            conn: Arc::new(Mutex::new(Connection::open_in_memory().unwrap())),
        }
    }
}
