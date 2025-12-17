use crate::my_tx_history_v2::{GetHistoryResult, RemoveTxResult, TxHistoryStorage, TxHistoryStorageError};
use crate::tx_history_storage::{
    token_id_from_tx_type, ConfirmationStatus, CreateTxHistoryStorageError, FilteringAddresses, GetTxHistoryFilters,
    WalletId,
};
use crate::TransactionDetails;
use async_trait::async_trait;
use common::{async_blocking, PagingOptionsEnum};
use db_common::sql_build::*;
use db_common::sqlite::rusqlite::types::Type;
use db_common::sqlite::rusqlite::{Connection, Error as SqlError, Row};
use db_common::sqlite::{query_single_row, string_from_row, validate_table_name, CHECK_TABLE_EXISTS_SQL};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use rpc::v1::types::Bytes as BytesJson;
use serde_json::{self as json};
use std::convert::TryInto;
use std::sync::{Arc, Mutex};

fn tx_history_table(wallet_id: &WalletId) -> String {
    wallet_id.to_sql_table_name() + "_tx_history"
}

fn tx_address_table(wallet_id: &WalletId) -> String {
    wallet_id.to_sql_table_name() + "_tx_address"
}

/// Please note TX cache table name doesn't depend on [`WalletId::hd_wallet_rmd160`].
fn tx_cache_table(wallet_id: &WalletId) -> String {
    format!("{}_tx_cache", wallet_id.ticker)
}

fn create_tx_history_table_sql(wallet_id: &WalletId) -> Result<String, MmError<SqlError>> {
    let table_name = tx_history_table(wallet_id);
    validate_table_name(&table_name)?;

    let sql = format!(
        "CREATE TABLE IF NOT EXISTS {table_name} (
            id INTEGER NOT NULL PRIMARY KEY,
            tx_hash VARCHAR(255) NOT NULL,
            internal_id VARCHAR(255) NOT NULL UNIQUE,
            block_height INTEGER NOT NULL,
            confirmation_status INTEGER NOT NULL,
            token_id VARCHAR(255) NOT NULL,
            details_json TEXT
        );"
    );

    Ok(sql)
}

fn create_tx_address_table_sql(wallet_id: &WalletId) -> Result<String, MmError<SqlError>> {
    let tx_address_table = tx_address_table(wallet_id);
    validate_table_name(&tx_address_table)?;

    let sql = format!(
        "CREATE TABLE IF NOT EXISTS {tx_address_table} (
            id INTEGER NOT NULL PRIMARY KEY,
            internal_id VARCHAR(255) NOT NULL,
            address TEXT NOT NULL
        );"
    );

    Ok(sql)
}

fn create_tx_cache_table_sql(wallet_id: &WalletId) -> Result<String, MmError<SqlError>> {
    let table_name = tx_cache_table(wallet_id);
    validate_table_name(&table_name)?;

    let sql = format!(
        "CREATE TABLE IF NOT EXISTS {table_name} (
            tx_hash VARCHAR(255) NOT NULL UNIQUE,
            tx_hex TEXT NOT NULL
        );"
    );

    Ok(sql)
}

fn create_internal_id_index_sql<F>(wallet_id: &WalletId, table_name_creator: F) -> Result<String, MmError<SqlError>>
where
    F: FnOnce(&WalletId) -> String,
{
    let table_name = table_name_creator(wallet_id);
    validate_table_name(&table_name)?;

    let sql = format!("CREATE INDEX IF NOT EXISTS internal_id_idx ON {table_name} (internal_id);");
    Ok(sql)
}

fn insert_tx_in_history_sql(wallet_id: &WalletId) -> Result<String, MmError<SqlError>> {
    let table_name = tx_history_table(wallet_id);
    validate_table_name(&table_name)?;

    let sql = format!(
        "INSERT INTO {table_name} (
            tx_hash,
            internal_id,
            block_height,
            confirmation_status,
            token_id,
            details_json
        ) VALUES (
            ?1, ?2, ?3, ?4, ?5, ?6
        );"
    );

    Ok(sql)
}

fn insert_tx_address_sql(wallet_id: &WalletId) -> Result<String, MmError<SqlError>> {
    let table_name = tx_address_table(wallet_id);
    validate_table_name(&table_name)?;

    let sql = format!(
        "INSERT INTO {table_name} (
            internal_id,
            address
        ) VALUES (?1, ?2);"
    );

    Ok(sql)
}

fn insert_tx_in_cache_sql(wallet_id: &WalletId) -> Result<String, MmError<SqlError>> {
    let table_name = tx_cache_table(wallet_id);
    validate_table_name(&table_name)?;

    // We can simply ignore the repetitive attempt to insert the same tx_hash
    let sql = format!("INSERT OR IGNORE INTO {table_name} (tx_hash, tx_hex) VALUES (?1, ?2);");

    Ok(sql)
}

fn remove_tx_by_internal_id_sql<F>(wallet_id: &WalletId, table_name_creator: F) -> Result<String, MmError<SqlError>>
where
    F: FnOnce(&WalletId) -> String,
{
    let table_name = table_name_creator(wallet_id);
    validate_table_name(&table_name)?;
    let sql = format!("DELETE FROM {table_name} WHERE internal_id=?1;");
    Ok(sql)
}

fn select_tx_by_internal_id_sql(wallet_id: &WalletId) -> Result<String, MmError<SqlError>> {
    let table_name = tx_history_table(wallet_id);
    validate_table_name(&table_name)?;

    let sql = format!("SELECT details_json FROM {table_name} WHERE internal_id=?1;");

    Ok(sql)
}

fn select_highest_block_height_sql(wallet_id: &WalletId) -> Result<String, MmError<SqlError>> {
    let table_name = tx_history_table(wallet_id);
    validate_table_name(&table_name)?;

    let sql = format!("SELECT block_height FROM {table_name} ORDER BY block_height DESC LIMIT 1;");

    Ok(sql)
}

fn update_tx_in_table_by_internal_id_sql(wallet_id: &WalletId) -> Result<String, MmError<SqlError>> {
    let table_name = tx_history_table(wallet_id);
    validate_table_name(&table_name)?;

    let sql = format!(
        "UPDATE {table_name} SET
            block_height = ?1,
            confirmation_status = ?2,
            details_json = ?3
        WHERE
            internal_id=?4;"
    );

    Ok(sql)
}

fn has_transactions_with_hash_sql(wallet_id: &WalletId) -> Result<String, MmError<SqlError>> {
    let table_name = tx_history_table(wallet_id);
    validate_table_name(&table_name)?;

    let sql = format!("SELECT COUNT(id) FROM {table_name} WHERE tx_hash = ?1;");

    Ok(sql)
}

fn get_tx_hex_from_cache_sql(wallet_id: &WalletId) -> Result<String, MmError<SqlError>> {
    let table_name = tx_cache_table(wallet_id);
    validate_table_name(&table_name)?;

    let sql = format!("SELECT tx_hex FROM {table_name} WHERE tx_hash = ?1 LIMIT 1;");

    Ok(sql)
}

/// Creates `SqlQuery` builder to query transactions from `tx_history` table
/// joining `tx_addresses` table and specifying from/to `for_addresses` addresses.
fn tx_history_with_addresses_builder_preimage<'a>(
    connection: &'a Connection,
    wallet_id: &WalletId,
    for_addresses: FilteringAddresses,
) -> Result<SqlQuery<'a>, MmError<SqlError>> {
    let mut sql_builder = SqlQuery::select_from_alias(connection, &tx_history_table(wallet_id), "tx_history")?;

    // Query transactions that were sent from/to `for_addresses` addresses.
    let tx_address_table_name = tx_address_table(wallet_id);

    sql_builder
        .join_alias(&tx_address_table_name, "tx_address")?
        .on_join_eq("tx_history.internal_id", "tx_address.internal_id")?;

    sql_builder
        .and_where_in_params("tx_address.address", for_addresses)?
        .group_by("tx_history.internal_id")?;

    Ok(sql_builder)
}

fn count_unique_tx_hashes_preimage<'a>(
    connection: &'a Connection,
    wallet_id: &WalletId,
    for_addresses: FilteringAddresses,
) -> Result<SqlQuery<'a>, MmError<SqlError>> {
    /// The alias is needed so that the external query can access the results of the subquery.
    /// Example:
    ///   SUBQUERY: `SELECT h.tx_hash AS __TX_HASH_ALIAS FROM tx_history h JOIN tx_address a ON h.internal_id = a.internal_id WHERE a.address IN ('address_2', 'address_4') GROUP BY h.internal_id`
    ///   EXTERNAL_QUERY: `SELECT COUNT(DISTINCT __TX_HASH_ALIAS) FROM (<SUBQUERY>);`
    /// Here we can't use `h.tx_hash` in the external query because it doesn't know about the `tx_history h` table.
    /// So we need to give the `h.tx_hash` an alias like `__TX_HASH_ALIAS`.
    const TX_HASH_ALIAS: &str = "__TX_HASH_ALIAS";

    let subquery = {
        let mut sql_builder = tx_history_with_addresses_builder_preimage(connection, wallet_id, for_addresses)?;

        // Query `tx_hash` field and give it the `__TX_HASH_ALIAS` alias.
        sql_builder.field_alias("tx_history.tx_hash", TX_HASH_ALIAS)?;

        drop_mutability!(sql_builder);
        sql_builder.subquery()
    };

    let mut external_query = SqlQuery::select_from_subquery(subquery)?;
    external_query.count_distinct(TX_HASH_ALIAS)?;
    Ok(external_query)
}

fn history_contains_unconfirmed_txes_preimage<'a>(
    connection: &'a Connection,
    wallet_id: &WalletId,
    for_addresses: FilteringAddresses,
) -> Result<SqlQuery<'a>, MmError<SqlError>> {
    /// The alias is needed so that the external query can access the results of the subquery.
    /// Example:
    ///   SUBQUERY: `SELECT h.id AS __ID_ALIAS FROM tx_history h JOIN tx_address a ON h.internal_id = a.internal_id WHERE a.address IN ('address_2', 'address_4') GROUP BY h.internal_id`
    ///   EXTERNAL_QUERY: `SELECT COUNT(__ID_ALIAS) FROM (<SUBQUERY>);`
    /// Here we can't use `h.id` in the external query because it doesn't know about the `tx_history h` table.
    /// So we need to give the `h.id` an alias like `__ID_ALIAS`.
    const ID_ALIAS: &str = "__ID_ALIAS";

    let subquery = {
        let mut sql_builder = tx_history_with_addresses_builder_preimage(connection, wallet_id, for_addresses)?;

        // Query `tx_hash` field and give it the `__ID_ALIAS` alias.
        sql_builder
            .field_alias("tx_history.id", ID_ALIAS)?
            .and_where_eq("confirmation_status", ConfirmationStatus::Unconfirmed.to_sql_param())?;

        drop_mutability!(sql_builder);
        sql_builder.subquery()
    };

    let mut external_query = SqlQuery::select_from_subquery(subquery)?;
    external_query.count(ID_ALIAS)?;
    Ok(external_query)
}

fn get_unconfirmed_txes_builder_preimage<'a>(
    connection: &'a Connection,
    wallet_id: &WalletId,
    for_addresses: FilteringAddresses,
) -> Result<SqlQuery<'a>, MmError<SqlError>> {
    let mut sql_builder = tx_history_with_addresses_builder_preimage(connection, wallet_id, for_addresses)?;

    sql_builder
        .field("details_json")?
        .and_where_eq("confirmation_status", ConfirmationStatus::Unconfirmed.to_sql_param())?;

    drop_mutability!(sql_builder);
    Ok(sql_builder)
}

/// Creates an `SqlQuery` instance with the required `WHERE`, `ORDER`, `GROUP_BY` constraints.
///
/// # Note
///
/// 1) You can refer to the [`tx_history_table(wallet_id)`] table by the `tx_history` alias.
/// 2) The selected transactions will be ordered the same way as `compare_transaction_details` is implemented.
fn get_history_builder_preimage<'a>(
    connection: &'a Connection,
    wallet_id: &WalletId,
    token_id: String,
    for_addresses: FilteringAddresses,
) -> Result<SqlQuery<'a>, MmError<SqlError>> {
    let mut sql_builder = tx_history_with_addresses_builder_preimage(connection, wallet_id, for_addresses)?;

    // Set other query conditions.
    sql_builder
        .and_where_eq_param("tx_history.token_id", token_id)?
        // The following statements repeat the `compare_transaction_details` implementation:
        .order_asc("tx_history.confirmation_status")?
        .order_desc("tx_history.block_height")?
        .order_asc("tx_history.internal_id")?;
    Ok(sql_builder)
}

fn finalize_get_total_count_sql_builder(mut subquery: SqlQuery<'_>) -> Result<SqlQuery<'_>, MmError<SqlError>> {
    /// The alias is needed so that the external query can access the results of the subquery.
    /// Example:
    ///   SUBQUERY: `SELECT h.internal_id AS __INTERNAL_ID_ALIAS FROM tx_history h JOIN tx_address a ON h.internal_id = a.internal_id WHERE a.address IN ('address_2', 'address_4') GROUP BY h.internal_id`
    ///   EXTERNAL_QUERY: `SELECT COUNT(__INTERNAL_ID_ALIAS) FROM (<SUBQUERY>);`
    /// Here we can't use `h.internal_id` in the external query because it doesn't know about the `tx_history h` table.
    /// So we need to give the `h.internal_id` an alias like `__INTERNAL_ID_ALIAS`.
    const INTERNAL_ID_ALIAS: &str = "__INTERNAL_ID_ALIAS";

    // Query `id_field` and give it the `__ID_FIELD` alias.
    subquery.field_alias("tx_history.internal_id", INTERNAL_ID_ALIAS)?;

    let mut external_query = SqlQuery::select_from_subquery(subquery.subquery())?;
    external_query.count(INTERNAL_ID_ALIAS)?;
    Ok(external_query)
}

fn finalize_get_history_sql_builder(sql_builder: &mut SqlQuery, offset: usize, limit: usize) -> Result<(), SqlError> {
    sql_builder
        .field("tx_history.details_json")?
        .offset(offset)
        .limit(limit);
    Ok(())
}

fn tx_details_from_row(row: &Row<'_>) -> Result<TransactionDetails, SqlError> {
    let json_string: String = row.get(0)?;
    json::from_str(&json_string).map_err(|e| SqlError::FromSqlConversionFailure(0, Type::Text, Box::new(e)))
}

fn block_height_from_row(row: &Row<'_>) -> Result<u32, SqlError> {
    row.get(0)
}

impl TxHistoryStorageError for SqlError {}

impl ConfirmationStatus {
    fn to_sql_param_str(self) -> String {
        (self as u8).to_string()
    }

    fn to_sql_param(self) -> i64 {
        self as i64
    }
}

impl WalletId {
    fn to_sql_table_name(&self) -> String {
        match self.hd_wallet_rmd160 {
            Some(hd_wallet_rmd160) => format!("{}_{}", self.ticker, hd_wallet_rmd160),
            None => self.ticker.clone(),
        }
    }
}

#[derive(Clone)]
pub struct SqliteTxHistoryStorage(Arc<Mutex<Connection>>);

impl SqliteTxHistoryStorage {
    pub fn new(ctx: &MmArc) -> Result<Self, MmError<CreateTxHistoryStorageError>> {
        let sqlite_connection =
            ctx.sqlite_connection
                .get()
                .ok_or(MmError::new(CreateTxHistoryStorageError::Internal(
                    "sqlite_connection is not initialized".to_owned(),
                )))?;
        Ok(SqliteTxHistoryStorage(sqlite_connection.clone()))
    }
}

#[async_trait]
impl TxHistoryStorage for SqliteTxHistoryStorage {
    type Error = SqlError;

    async fn init(&self, wallet_id: &WalletId) -> Result<(), MmError<Self::Error>> {
        let selfi = self.clone();

        let sql_history = create_tx_history_table_sql(wallet_id)?;
        let sql_cache = create_tx_cache_table_sql(wallet_id)?;
        let sql_addr = create_tx_address_table_sql(wallet_id)?;

        let sql_history_index = create_internal_id_index_sql(wallet_id, tx_history_table)?;
        let sql_addr_index = create_internal_id_index_sql(wallet_id, tx_address_table)?;

        async_blocking(move || {
            let conn = selfi.0.lock().unwrap();

            conn.execute(&sql_history, []).map(|_| ())?;
            conn.execute(&sql_addr, []).map(|_| ())?;
            conn.execute(&sql_cache, []).map(|_| ())?;

            conn.execute(&sql_history_index, []).map(|_| ())?;
            conn.execute(&sql_addr_index, []).map(|_| ())?;
            Ok(())
        })
        .await
    }

    async fn is_initialized_for(&self, wallet_id: &WalletId) -> Result<bool, MmError<Self::Error>> {
        let tx_history_table = tx_history_table(wallet_id);
        validate_table_name(&tx_history_table)?;

        let tx_cache_table = tx_cache_table(wallet_id);
        validate_table_name(&tx_cache_table)?;

        let selfi = self.clone();
        async_blocking(move || {
            let conn = selfi.0.lock().unwrap();
            let history_initialized =
                query_single_row(&conn, CHECK_TABLE_EXISTS_SQL, [tx_history_table], string_from_row)?;
            let cache_initialized = query_single_row(&conn, CHECK_TABLE_EXISTS_SQL, [tx_cache_table], string_from_row)?;
            Ok(history_initialized.is_some() && cache_initialized.is_some())
        })
        .await
    }

    async fn add_transactions_to_history<I>(
        &self,
        wallet_id: &WalletId,
        transactions: I,
    ) -> Result<(), MmError<Self::Error>>
    where
        I: IntoIterator<Item = TransactionDetails> + Send + 'static,
        I::IntoIter: Send,
    {
        let selfi = self.clone();
        let wallet_id = wallet_id.clone();
        async_blocking(move || {
            let mut conn = selfi.0.lock().unwrap();
            let sql_transaction = conn.transaction()?;

            for tx in transactions {
                let Some(tx_hash) = tx.tx.tx_hash() else { continue };
                let Some(tx_hex) = tx.tx.tx_hex().cloned() else {
                    continue;
                };
                let tx_hex = format!("{tx_hex:02x}");

                let internal_id = format!("{:02x}", tx.internal_id);
                let confirmation_status = ConfirmationStatus::from_block_height(tx.block_height);
                let token_id = token_id_from_tx_type(&tx.transaction_type);
                let tx_json = json::to_string(&tx).expect("serialization should not fail");

                let tx_cache_params = [tx_hash, &tx_hex];

                sql_transaction.execute(&insert_tx_in_cache_sql(&wallet_id)?, tx_cache_params)?;

                let params = [
                    tx_hash,
                    &internal_id,
                    &tx.block_height.to_string(),
                    &confirmation_status.to_sql_param_str(),
                    &token_id,
                    &tx_json,
                ];
                sql_transaction.execute(&insert_tx_in_history_sql(&wallet_id)?, params)?;

                let addresses: FilteringAddresses = tx.from.into_iter().chain(tx.to.into_iter()).collect();
                for address in addresses {
                    let params = [internal_id.clone(), address];
                    sql_transaction.execute(&insert_tx_address_sql(&wallet_id)?, params)?;
                }
            }
            sql_transaction.commit()?;
            Ok(())
        })
        .await
    }

    async fn remove_tx_from_history(
        &self,
        wallet_id: &WalletId,
        internal_id: &BytesJson,
    ) -> Result<RemoveTxResult, MmError<Self::Error>> {
        let remove_tx_history_sql = remove_tx_by_internal_id_sql(wallet_id, tx_history_table)?;
        let remove_tx_addr_sql = remove_tx_by_internal_id_sql(wallet_id, tx_address_table)?;

        let params = [format!("{internal_id:02x}")];
        let selfi = self.clone();

        async_blocking(move || {
            let mut conn = selfi.0.lock().unwrap();
            let sql_transaction = conn.transaction()?;

            sql_transaction.execute(&remove_tx_addr_sql, params.clone())?;

            let rows_num = sql_transaction.execute(&remove_tx_history_sql, params)?;
            let remove_tx_result = if rows_num > 0 {
                RemoveTxResult::TxRemoved
            } else {
                RemoveTxResult::TxDidNotExist
            };

            sql_transaction.commit()?;
            Ok(remove_tx_result)
        })
        .await
    }

    async fn get_tx_from_history(
        &self,
        wallet_id: &WalletId,
        internal_id: &BytesJson,
    ) -> Result<Option<TransactionDetails>, MmError<Self::Error>> {
        let params = [format!("{internal_id:02x}")];
        let sql = select_tx_by_internal_id_sql(wallet_id)?;
        let selfi = self.clone();

        async_blocking(move || {
            let conn = selfi.0.lock().unwrap();
            query_single_row(&conn, &sql, params, tx_details_from_row).map_to_mm(SqlError::from)
        })
        .await
    }

    async fn get_highest_block_height(&self, wallet_id: &WalletId) -> Result<Option<u32>, MmError<Self::Error>> {
        let sql = select_highest_block_height_sql(wallet_id)?;
        let selfi = self.clone();

        async_blocking(move || {
            let conn = selfi.0.lock().unwrap();
            query_single_row(&conn, &sql, [], block_height_from_row).map_to_mm(SqlError::from)
        })
        .await
    }

    async fn history_contains_unconfirmed_txes(
        &self,
        wallet_id: &WalletId,
        for_addresses: FilteringAddresses,
    ) -> Result<bool, MmError<Self::Error>> {
        let wallet_id = wallet_id.clone();
        let selfi = self.clone();

        async_blocking(move || {
            let conn = selfi.0.lock().unwrap();
            let sql_query = history_contains_unconfirmed_txes_preimage(&conn, &wallet_id, for_addresses)?;

            let count_unconfirmed: u32 = sql_query
                .query_single_row(|row| row.get(0))?
                .or_mm_err(|| SqlError::QueryReturnedNoRows)?;
            Ok(count_unconfirmed > 0)
        })
        .await
    }

    async fn get_unconfirmed_txes_from_history(
        &self,
        wallet_id: &WalletId,
        for_addresses: FilteringAddresses,
    ) -> Result<Vec<TransactionDetails>, MmError<Self::Error>> {
        let wallet_id = wallet_id.clone();
        let selfi = self.clone();

        async_blocking(move || {
            let conn = selfi.0.lock().unwrap();

            let sql_query = get_unconfirmed_txes_builder_preimage(&conn, &wallet_id, for_addresses)?;
            let result = sql_query.query(tx_details_from_row)?;
            Ok(result)
        })
        .await
    }

    async fn update_tx_in_history(
        &self,
        wallet_id: &WalletId,
        tx: &TransactionDetails,
    ) -> Result<(), MmError<Self::Error>> {
        let sql = update_tx_in_table_by_internal_id_sql(wallet_id)?;

        let block_height = tx.block_height.to_string();
        let confirmation_status = ConfirmationStatus::from_block_height(tx.block_height);
        let json_details = json::to_string(tx).unwrap();
        let internal_id = format!("{:02x}", tx.internal_id);

        let params = [
            block_height,
            confirmation_status.to_sql_param_str(),
            json_details,
            internal_id,
        ];

        let selfi = self.clone();
        async_blocking(move || {
            let conn = selfi.0.lock().unwrap();
            conn.execute(&sql, params).map(|_| ()).map_err(MmError::new)
        })
        .await
    }

    async fn history_has_tx_hash(&self, wallet_id: &WalletId, tx_hash: &str) -> Result<bool, MmError<Self::Error>> {
        let sql = has_transactions_with_hash_sql(wallet_id)?;
        let params = [tx_hash.to_owned()];

        let selfi = self.clone();
        async_blocking(move || {
            let conn = selfi.0.lock().unwrap();
            let count: u32 = conn.query_row(&sql, params, |row| row.get(0))?;
            Ok(count > 0)
        })
        .await
    }

    async fn unique_tx_hashes_num_in_history(
        &self,
        wallet_id: &WalletId,
        for_addresses: FilteringAddresses,
    ) -> Result<usize, MmError<Self::Error>> {
        let selfi = self.clone();
        let wallet_id = wallet_id.clone();

        async_blocking(move || {
            let conn = selfi.0.lock().unwrap();

            let sql_query = count_unique_tx_hashes_preimage(&conn, &wallet_id, for_addresses)?;
            let count: u32 = sql_query
                .query_single_row(|row| row.get(0))?
                .or_mm_err(|| SqlError::QueryReturnedNoRows)?;
            Ok(count as usize)
        })
        .await
    }

    async fn add_tx_to_cache(
        &self,
        wallet_id: &WalletId,
        tx_hash: &str,
        tx_hex: &BytesJson,
    ) -> Result<(), MmError<Self::Error>> {
        let sql = insert_tx_in_cache_sql(wallet_id)?;
        let params = [tx_hash.to_owned(), format!("{tx_hex:02x}")];
        let selfi = self.clone();
        async_blocking(move || {
            let conn = selfi.0.lock().unwrap();
            conn.execute(&sql, params)?;
            Ok(())
        })
        .await
    }

    async fn tx_bytes_from_cache(
        &self,
        wallet_id: &WalletId,
        tx_hash: &str,
    ) -> Result<Option<BytesJson>, MmError<Self::Error>> {
        let sql = get_tx_hex_from_cache_sql(wallet_id)?;
        let params = [tx_hash.to_owned()];
        let selfi = self.clone();
        async_blocking(move || {
            let conn = selfi.0.lock().unwrap();
            let maybe_tx_hex: Result<String, _> = conn.query_row(&sql, params, |row| row.get(0));
            if let Err(SqlError::QueryReturnedNoRows) = maybe_tx_hex {
                return Ok(None);
            }
            let tx_hex = maybe_tx_hex?;
            let tx_bytes =
                hex::decode(tx_hex).map_err(|e| SqlError::FromSqlConversionFailure(0, Type::Text, Box::new(e)))?;
            Ok(Some(tx_bytes.into()))
        })
        .await
    }

    async fn get_history(
        &self,
        wallet_id: &WalletId,
        filters: GetTxHistoryFilters,
        paging: PagingOptionsEnum<BytesJson>,
        limit: usize,
    ) -> Result<GetHistoryResult, MmError<Self::Error>> {
        // Check if [`GetTxHistoryFilters::for_addresses`] is empty.
        // If it is, it's much more efficient to return an empty result before we do any query.
        if filters.for_addresses.is_empty() {
            return Ok(GetHistoryResult {
                transactions: Vec::new(),
                skipped: 0,
                total: 0,
            });
        }

        let wallet_id = wallet_id.clone();
        let selfi = self.clone();

        async_blocking(move || {
            let conn = selfi.0.lock().unwrap();
            let token_id = filters.token_id_or_exclude();
            let mut sql_builder = get_history_builder_preimage(&conn, &wallet_id, token_id, filters.for_addresses)?;

            let total_count_builder = finalize_get_total_count_sql_builder(sql_builder.clone())?;
            let total: isize = total_count_builder
                .query_single_row(|row| row.get(0))?
                .or_mm_err(|| SqlError::QueryReturnedNoRows)?;
            let total = total.try_into().expect("count should be always above zero");

            let offset = match paging {
                PagingOptionsEnum::PageNumber(page) => (page.get() - 1) * limit,
                PagingOptionsEnum::FromId(from_internal_id) => {
                    let maybe_offset = sql_builder
                        .clone()
                        .query_offset_by_id("tx_history.internal_id", format!("{from_internal_id:02x}"))?;
                    match maybe_offset {
                        Some(offset) => offset,
                        None => {
                            // TODO do we need to return `SqlError::QueryReturnedNoRows` error instead?
                            return Ok(GetHistoryResult {
                                transactions: vec![],
                                skipped: 0,
                                total,
                            });
                        },
                    }
                },
            };

            finalize_get_history_sql_builder(&mut sql_builder, offset, limit)?;
            let transactions = sql_builder.query(tx_details_from_row)?;

            let result = GetHistoryResult {
                transactions,
                skipped: offset,
                total,
            };
            Ok(result)
        })
        .await
    }
}
