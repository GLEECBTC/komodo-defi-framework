use crate::account::storage::{AccountStorage, AccountStorageError, AccountStorageResult};
use crate::account::{
    AccountId, AccountInfo, AccountType, AccountWithCoins, AccountWithEnabledFlag, EnabledAccountId,
    EnabledAccountType, HwPubkey, MAX_ACCOUNT_DESCRIPTION_LENGTH, MAX_ACCOUNT_NAME_LENGTH, MAX_TICKER_LENGTH,
};
use async_trait::async_trait;
use common::some_or_return_ok_none;
use db_common::foreign_columns;
use db_common::sql_build::*;
use db_common::sqlite::rusqlite::types::Type;
use db_common::sqlite::rusqlite::{Connection, Error as SqlError, Result as SqlResult, Row};
use db_common::sqlite::{is_constraint_error, SqliteConnShared};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::BigDecimal;
use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr;
use std::sync::{Arc, MutexGuard};

const DEVICE_PUBKEY_MAX_LENGTH: usize = 20;
const BALANCE_MAX_LENGTH: usize = 255;

mod account_table {
    /// The table name.
    pub(super) const TABLE_NAME: &str = "gui_account";

    // The following constants are the column names.
    pub(super) const ACCOUNT_TYPE: &str = "account_type";
    pub(super) const ACCOUNT_IDX: &str = "account_idx";
    pub(super) const DEVICE_PUBKEY: &str = "device_pubkey";
    pub(super) const NAME: &str = "name";
    pub(super) const DESCRIPTION: &str = "description";
    pub(super) const BALANCE_USD: &str = "balance_usd";

    /// The table PRIMARY KEY name.
    pub(super) const ACCOUNT_ID_PRIMARY_KEY: &str = "account_id_primary";
}

mod account_coins_table {
    /// The table name.
    pub(super) const TABLE_NAME: &str = "gui_account_coins";

    // The following constants are the column names.
    pub(super) const ACCOUNT_TYPE: &str = "account_type";
    pub(super) const ACCOUNT_IDX: &str = "account_idx";
    pub(super) const DEVICE_PUBKEY: &str = "device_pubkey";
    pub(super) const COIN: &str = "coin";

    /// The table UNIQUE constraint.
    pub(super) const ACCOUNT_ID_COIN_CONSTRAINT: &str = "account_id_coin_constraint";
}

mod enabled_account_table {
    /// The table name.
    pub(super) const TABLE_NAME: &str = "gui_account_enabled";

    // The following constants are the column names.
    pub(super) const ACCOUNT_TYPE: &str = "account_type";
    pub(super) const ACCOUNT_IDX: &str = "account_idx";
    pub(super) const DEVICE_PUBKEY: &str = "device_pubkey";
}

impl From<SqlError> for AccountStorageError {
    fn from(e: SqlError) -> Self {
        let error = e.to_string();
        match e {
            SqlError::FromSqlConversionFailure(_, _, _)
            | SqlError::IntegralValueOutOfRange(_, _)
            | SqlError::InvalidColumnIndex(_)
            | SqlError::InvalidColumnType(_, _, _) => AccountStorageError::ErrorDeserializing(error),
            SqlError::Utf8Error(_) | SqlError::NulError(_) | SqlError::ToSqlConversionFailure(_) => {
                AccountStorageError::ErrorSerializing(error)
            },
            _ => AccountStorageError::Internal(error),
        }
    }
}

impl AccountId {
    /// An alternative to [`AccountId::to_tuple`] that returns SQL compatible types.
    fn to_sql_tuple(&self) -> (i64, i64, String) {
        let (account_type, account_idx, device_pubkey) = self.to_tuple();
        (account_type as i64, account_idx as i64, device_pubkey.to_string())
    }

    /// An alternative to [`AccountId::try_from_tuple`] that takes SQL compatible types.
    pub(crate) fn try_from_sql_tuple(
        account_type: i64,
        account_idx: u32,
        device_pubkey: &str,
    ) -> AccountStorageResult<AccountId> {
        let account_type = AccountType::try_from(account_type)?;
        let device_pubkey =
            HwPubkey::from_str(device_pubkey).map_to_mm(|e| AccountStorageError::ErrorDeserializing(e.to_string()))?;
        AccountId::try_from_tuple(account_type, account_idx, device_pubkey)
    }
}

impl EnabledAccountId {
    /// An alternative to [`EnabledAccountId::to_pair`] that returns SQL compatible types.
    /// Please note `device_pubkey` is always default.
    fn to_sql_tuple(self) -> (i64, i64, String) {
        let (account_type, account_idx, device_pubkey) = self.to_tuple();
        (account_type as i64, account_idx as i64, device_pubkey.to_string())
    }

    /// An alternative to [`EnabledAccountId::try_from_pair`] that takes SQL compatible types.
    pub(crate) fn try_from_sql_pair(account_type: i64, account_idx: u32) -> AccountStorageResult<EnabledAccountId> {
        let account_type = EnabledAccountType::try_from(account_type)?;
        EnabledAccountId::try_from_pair(account_type, account_idx)
    }
}

pub(crate) struct SqliteAccountStorage {
    conn: SqliteConnShared,
}

impl SqliteAccountStorage {
    pub(crate) fn new(ctx: &MmArc) -> AccountStorageResult<SqliteAccountStorage> {
        let shared = ctx
            .sqlite_connection
            .get()
            .or_mm_err(|| AccountStorageError::Internal("'MmCtx::sqlite_connection' is not initialized".to_owned()))?;
        Ok(SqliteAccountStorage {
            conn: Arc::clone(shared),
        })
    }

    fn lock_conn_mutex(&self) -> AccountStorageResult<MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_to_mm(|e| AccountStorageError::Internal(format!("Error locking sqlite connection: {e}")))
    }

    fn init_account_table(conn: &Connection) -> AccountStorageResult<()> {
        let mut create_sql = SqlCreateTable::new(conn, account_table::TABLE_NAME);
        create_sql
            .if_not_exist()
            .column(SqlColumn::new(account_table::ACCOUNT_TYPE, SqlType::Integer).not_null())
            .column(SqlColumn::new(account_table::ACCOUNT_IDX, SqlType::Integer).not_null())
            .column(SqlColumn::new(account_table::DEVICE_PUBKEY, SqlType::Varchar(DEVICE_PUBKEY_MAX_LENGTH)).not_null())
            .column(SqlColumn::new(account_table::NAME, SqlType::Varchar(MAX_ACCOUNT_NAME_LENGTH)).not_null())
            .column(SqlColumn::new(
                account_table::DESCRIPTION,
                SqlType::Varchar(MAX_ACCOUNT_DESCRIPTION_LENGTH),
            ))
            .column(SqlColumn::new(account_table::BALANCE_USD, SqlType::Varchar(BALANCE_MAX_LENGTH)).not_null())
            .constraint(PrimaryKey::new(
                account_table::ACCOUNT_ID_PRIMARY_KEY,
                [
                    account_table::ACCOUNT_TYPE,
                    account_table::ACCOUNT_IDX,
                    account_table::DEVICE_PUBKEY,
                ],
            )?);
        create_sql.create().map_to_mm(AccountStorageError::from)
    }

    fn init_account_coins_table(conn: &Connection) -> AccountStorageResult<()> {
        let mut create_sql = SqlCreateTable::new(conn, account_coins_table::TABLE_NAME);
        create_sql
            .if_not_exist()
            .column(SqlColumn::new(account_coins_table::ACCOUNT_TYPE, SqlType::Integer).not_null())
            .column(SqlColumn::new(account_coins_table::ACCOUNT_IDX, SqlType::Integer).not_null())
            .column(
                SqlColumn::new(
                    account_coins_table::DEVICE_PUBKEY,
                    SqlType::Varchar(DEVICE_PUBKEY_MAX_LENGTH),
                )
                .not_null(),
            )
            .column(SqlColumn::new(account_coins_table::COIN, SqlType::Varchar(MAX_TICKER_LENGTH)).not_null())
            .constraint(
                ForeignKey::new(
                    foreign_key::ParentTable(account_table::TABLE_NAME),
                    foreign_columns![
                        account_coins_table::ACCOUNT_TYPE => account_table::ACCOUNT_TYPE,
                        account_coins_table::ACCOUNT_IDX => account_table::ACCOUNT_IDX,
                        account_coins_table::DEVICE_PUBKEY => account_table::DEVICE_PUBKEY
                    ],
                )?
                // Delete all coins from `account_coins_table` if the corresponding `account_table` record has been deleted.
                .on_event(foreign_key::Event::OnDelete, foreign_key::Action::Cascade),
            )
            .constraint(Unique::new(
                account_coins_table::ACCOUNT_ID_COIN_CONSTRAINT,
                [
                    account_coins_table::ACCOUNT_TYPE,
                    account_coins_table::ACCOUNT_IDX,
                    account_coins_table::DEVICE_PUBKEY,
                    account_coins_table::COIN,
                ],
            )?);
        create_sql.create().map_to_mm(AccountStorageError::from)
    }

    fn init_enabled_account_table(conn: &Connection) -> AccountStorageResult<()> {
        let mut create_sql = SqlCreateTable::new(conn, enabled_account_table::TABLE_NAME);
        create_sql
            .if_not_exist()
            .column(SqlColumn::new(enabled_account_table::ACCOUNT_TYPE, SqlType::Integer).not_null())
            .column(SqlColumn::new(enabled_account_table::ACCOUNT_IDX, SqlType::Integer).not_null())
            // `device_pubkey` column is used to declare a foreign key that refers to the `account_table` primary key.
            .column(
                SqlColumn::new(
                    enabled_account_table::DEVICE_PUBKEY,
                    SqlType::Varchar(DEVICE_PUBKEY_MAX_LENGTH),
                )
                .not_null(),
            )
            .constraint(
                ForeignKey::new(
                    foreign_key::ParentTable(account_table::TABLE_NAME),
                    foreign_columns![
                        enabled_account_table::ACCOUNT_TYPE => account_table::ACCOUNT_TYPE,
                        enabled_account_table::ACCOUNT_IDX => account_table::ACCOUNT_IDX,
                        enabled_account_table::DEVICE_PUBKEY => account_table::DEVICE_PUBKEY,
                    ],
                )?
                // Delete an enabled account from `enabled_account_table` if the corresponding `account_table` record has been deleted.
                .on_event(foreign_key::Event::OnDelete, foreign_key::Action::Cascade),
            );
        create_sql.create().map_to_mm(AccountStorageError::from)
    }

    /// Loads `AccountId` of an enabled account or returns an error if there is no enabled account yet.
    fn load_enabled_account_id_or_err(conn: &Connection) -> AccountStorageResult<EnabledAccountId> {
        let mut query = SqlQuery::select_from(conn, enabled_account_table::TABLE_NAME)?;
        query
            .field(enabled_account_table::ACCOUNT_TYPE)?
            .field(enabled_account_table::ACCOUNT_IDX)?;
        query
            .query_single_row(enabled_account_id_from_row)?
            .or_mm_err(|| AccountStorageError::NoEnabledAccount)
    }

    /// Loads the given `accoint_id` activated coins.
    ///
    /// # Note
    ///
    /// The function doesn't check if the account is present in `account_table`.
    fn load_account_coins(conn: &Connection, account_id: &AccountId) -> AccountStorageResult<BTreeSet<String>> {
        let mut query = SqlQuery::select_from(conn, account_coins_table::TABLE_NAME)?;

        let (account_type, account_id, device_pubkey) = account_id.to_sql_tuple();
        query
            .field(account_coins_table::COIN)?
            .and_where_eq(account_table::ACCOUNT_TYPE, account_type)?
            .and_where_eq(account_table::ACCOUNT_IDX, account_id)?
            .and_where_eq_param(account_table::DEVICE_PUBKEY, device_pubkey)?;
        let coins = query.query(|row| row.get::<_, String>(0))?.into_iter().collect();
        Ok(coins)
    }

    /// Loads `AccountWithCoins`.
    /// This method takes `conn` to ensure data coherence.
    fn load_account_with_coins(
        conn: &Connection,
        account_id: &AccountId,
    ) -> AccountStorageResult<Option<AccountWithCoins>> {
        let account_info = some_or_return_ok_none!(Self::load_account(conn, account_id)?);

        let coins = Self::load_account_coins(conn, account_id)?;
        Ok(Some(AccountWithCoins { account_info, coins }))
    }

    /// Tries to load an account info.
    /// This method takes `conn` to ensure data coherence.
    fn load_account(conn: &Connection, account_id: &AccountId) -> AccountStorageResult<Option<AccountInfo>> {
        let mut query = SqlQuery::select_from(conn, account_table::TABLE_NAME)?;
        query
            .field(account_table::ACCOUNT_TYPE)?
            .field(account_table::ACCOUNT_IDX)?
            .field(account_table::DEVICE_PUBKEY)?
            .field(account_table::NAME)?
            .field(account_table::DESCRIPTION)?
            .field(account_table::BALANCE_USD)?;

        let (account_type, account_id, device_pubkey) = account_id.to_sql_tuple();
        query
            .and_where_eq(account_table::ACCOUNT_TYPE, account_type)?
            .and_where_eq(account_table::ACCOUNT_IDX, account_id)?
            .and_where_eq_param(account_table::DEVICE_PUBKEY, device_pubkey)?;
        query
            .query_single_row(account_from_row)
            .map_to_mm(AccountStorageError::from)
    }

    fn load_accounts(conn: &Connection) -> AccountStorageResult<BTreeMap<AccountId, AccountInfo>> {
        let mut query = SqlQuery::select_from(conn, account_table::TABLE_NAME)?;
        query
            .field(account_table::ACCOUNT_TYPE)?
            .field(account_table::ACCOUNT_IDX)?
            .field(account_table::DEVICE_PUBKEY)?
            .field(account_table::NAME)?
            .field(account_table::DESCRIPTION)?
            .field(account_table::BALANCE_USD)?;
        let accounts = query
            .query(account_from_row)?
            .into_iter()
            .map(|account| (account.account_id.clone(), account))
            .collect();
        Ok(accounts)
    }

    fn account_exists(conn: &Connection, account_id: &AccountId) -> AccountStorageResult<bool> {
        let mut query = SqlQuery::select_from(conn, account_table::TABLE_NAME)?;
        query.count(account_table::NAME)?;

        let (account_type, account_idx, device_pubkey) = account_id.to_sql_tuple();
        query
            .and_where_eq(account_table::ACCOUNT_TYPE, account_type)?
            .and_where_eq(account_table::ACCOUNT_IDX, account_idx)?
            .and_where_eq_param(account_table::DEVICE_PUBKEY, device_pubkey)?;

        let accounts = query
            .query_single_row(count_from_row)?
            .or_mm_err(|| AccountStorageError::Internal("'count' should have returned one row exactly".to_string()))?;

        Ok(accounts > 0)
    }

    fn upload_account(conn: &Connection, account: AccountInfo) -> AccountStorageResult<()> {
        let mut sql_insert = SqlInsert::new(conn, account_table::TABLE_NAME);

        let (account_type, account_idx, device_pubkey) = account.account_id.to_sql_tuple();
        sql_insert
            .column(account_table::ACCOUNT_TYPE, account_type)?
            .column(account_table::ACCOUNT_IDX, account_idx)?
            .column_param(account_table::DEVICE_PUBKEY, device_pubkey)?
            .column_param(account_table::NAME, account.name)?
            .column_param(account_table::DESCRIPTION, account.description)?
            .column_param(account_table::BALANCE_USD, account.balance_usd.to_string())?;

        // A constraint error occurs if there is an account with the same primary key (`account_id`).
        handle_constraint_error(sql_insert.insert(), || {
            AccountStorageError::AccountExistsAlready(account.account_id)
        })?;
        Ok(())
    }

    fn delete_account(conn: &Connection, account_id: AccountId) -> AccountStorageResult<()> {
        let (account_type, account_idx, device_pubkey) = account_id.to_sql_tuple();

        // Remove the account info.
        let mut sql_delete_account = SqlDelete::new(conn, account_table::TABLE_NAME)?;
        sql_delete_account
            .and_where_eq(account_table::ACCOUNT_TYPE, account_type)?
            .and_where_eq(account_table::ACCOUNT_IDX, account_idx)?
            .and_where_eq_param(account_table::DEVICE_PUBKEY, device_pubkey)?;

        let deleted = sql_delete_account.delete()?;
        // The number of deleted accounts is 0 if only there is no account with the given `account_id`.
        if deleted == 0 {
            return MmError::err(AccountStorageError::NoSuchAccount(account_id));
        }

        Ok(())
    }

    /// Updates the given `account_id` account by applying the `update_cb` callback to an `SqlUpdate` SQL builder.
    fn update_account<F>(conn: &Connection, account_id: AccountId, update_cb: F) -> AccountStorageResult<()>
    where
        F: FnOnce(&mut SqlUpdate) -> SqlResult<()>,
    {
        let mut sql_update = SqlUpdate::new(conn, account_table::TABLE_NAME)?;
        update_cb(&mut sql_update)?;

        let (account_type, account_idx, device_pubkey) = account_id.to_sql_tuple();
        sql_update
            .and_where_eq(account_table::ACCOUNT_TYPE, account_type)?
            .and_where_eq(account_table::ACCOUNT_IDX, account_idx)?
            .and_where_eq_param(account_table::DEVICE_PUBKEY, device_pubkey)?;

        let updated = sql_update.update()?;
        // The number of updated accounts is 0 if only there is no account with the given `account_id`.
        if updated == 0 {
            return MmError::err(AccountStorageError::NoSuchAccount(account_id));
        }
        Ok(())
    }
}

#[async_trait]
impl AccountStorage for SqliteAccountStorage {
    async fn init(&self) -> AccountStorageResult<()> {
        let mut conn = self.lock_conn_mutex()?;
        let transaction = conn.transaction()?;

        SqliteAccountStorage::init_account_table(&transaction)?;
        SqliteAccountStorage::init_account_coins_table(&transaction)?;
        SqliteAccountStorage::init_enabled_account_table(&transaction)?;

        transaction.commit()?;
        Ok(())
    }

    async fn load_account_coins(&self, account_id: AccountId) -> AccountStorageResult<BTreeSet<String>> {
        let conn = self.lock_conn_mutex()?;
        let coins = Self::load_account_coins(&conn, &account_id)?;

        // Check if the account is present in `account_table` if **only** there are no activated coins.
        // Otherwise we're sure that the account exists.
        if coins.is_empty() && !Self::account_exists(&conn, &account_id)? {
            return MmError::err(AccountStorageError::NoSuchAccount(account_id));
        }
        Ok(coins)
    }

    async fn load_accounts(&self) -> AccountStorageResult<BTreeMap<AccountId, AccountInfo>> {
        let conn = self.lock_conn_mutex()?;
        Self::load_accounts(&conn)
    }

    async fn load_accounts_with_enabled_flag(
        &self,
    ) -> AccountStorageResult<BTreeMap<AccountId, AccountWithEnabledFlag>> {
        let conn = self.lock_conn_mutex()?;
        let enabled_account_id = AccountId::from(Self::load_enabled_account_id_or_err(&conn)?);

        let mut found_enabled = false;
        let accounts = Self::load_accounts(&conn)?
            .into_iter()
            .map(|(account_id, account_info)| {
                let enabled = account_id == enabled_account_id;
                found_enabled |= enabled;
                Ok((account_id, AccountWithEnabledFlag { account_info, enabled }))
            })
            .collect::<AccountStorageResult<BTreeMap<_, _>>>()?;

        // If `AccountStorage::load_enabled_account_id_or_err` returns an `AccountId`,
        // then corresponding account must be in `AccountTable`.
        if !found_enabled {
            return MmError::err(AccountStorageError::unknown_account_in_enabled_table(
                enabled_account_id,
            ));
        }
        Ok(accounts)
    }

    async fn load_enabled_account_id(&self) -> AccountStorageResult<EnabledAccountId> {
        let conn = self.lock_conn_mutex()?;
        Self::load_enabled_account_id_or_err(&conn)
    }

    async fn load_enabled_account_with_coins(&self) -> AccountStorageResult<AccountWithCoins> {
        let conn = self.lock_conn_mutex()?;
        let account_id = AccountId::from(Self::load_enabled_account_id_or_err(&conn)?);

        Self::load_account_with_coins(&conn, &account_id)?
            .or_mm_err(|| AccountStorageError::unknown_account_in_enabled_table(account_id))
    }

    async fn enable_account(&self, enabled_account_id: EnabledAccountId) -> AccountStorageResult<()> {
        let mut conn = self.lock_conn_mutex()?;
        let transaction = conn.transaction()?;

        // Remove the previous enabled account by clearing the table.
        SqlDelete::new(&transaction, enabled_account_table::TABLE_NAME)?.delete()?;

        let mut sql_insert = SqlInsert::new(&transaction, enabled_account_table::TABLE_NAME);

        let (account_type, account_idx, _device_pubkey) = enabled_account_id.to_sql_tuple();
        sql_insert
            .column(enabled_account_table::ACCOUNT_TYPE, account_type)?
            .column(enabled_account_table::ACCOUNT_IDX, account_idx)?
            .column_param(enabled_account_table::DEVICE_PUBKEY, _device_pubkey)?;

        // A constraint error occurs if there is no account in `account_table` with the given `account_id`.
        let inserted = handle_constraint_error(sql_insert.insert(), || {
            AccountStorageError::NoSuchAccount(AccountId::from(enabled_account_id))
        })?;
        // The number of inserted accounts is expected to be '1'.
        if inserted != 1 {
            let error = format!("Expected exactly '1' inserted account, found '{inserted}'");
            return MmError::err(AccountStorageError::Internal(error));
        }

        transaction.commit()?;
        Ok(())
    }

    async fn upload_account(&self, account: AccountInfo) -> AccountStorageResult<()> {
        let conn = self.lock_conn_mutex()?;
        Self::upload_account(&conn, account)
    }

    async fn delete_account(&self, account_id: AccountId) -> AccountStorageResult<()> {
        let conn = self.lock_conn_mutex()?;
        Self::delete_account(&conn, account_id)
    }

    async fn set_name(&self, account_id: AccountId, name: String) -> AccountStorageResult<()> {
        let conn = self.lock_conn_mutex()?;

        Self::update_account(&conn, account_id, |sql_update| {
            sql_update.set_param(account_table::NAME, name)?;
            Ok(())
        })
    }

    async fn set_description(&self, account_id: AccountId, description: String) -> AccountStorageResult<()> {
        let conn = self.lock_conn_mutex()?;

        Self::update_account(&conn, account_id, |sql_update| {
            sql_update.set_param(account_table::DESCRIPTION, description)?;
            Ok(())
        })
    }

    async fn set_balance(&self, account_id: AccountId, balance_usd: BigDecimal) -> AccountStorageResult<()> {
        let conn = self.lock_conn_mutex()?;

        Self::update_account(&conn, account_id, |sql_update| {
            sql_update.set_param(account_table::BALANCE_USD, balance_usd.to_string())?;
            Ok(())
        })
    }

    async fn activate_coins(&self, account_id: AccountId, tickers: Vec<String>) -> AccountStorageResult<()> {
        let mut conn = self.lock_conn_mutex()?;
        let transaction = conn.transaction()?;

        for ticker in tickers {
            let mut sql_insert = SqlInsert::new(&transaction, account_coins_table::TABLE_NAME);

            let (account_type, account_idx, device_pubkey) = account_id.to_sql_tuple();
            sql_insert
                .or_ignore()
                .column(account_coins_table::ACCOUNT_TYPE, account_type)?
                .column(account_coins_table::ACCOUNT_IDX, account_idx)?
                .column_param(account_coins_table::DEVICE_PUBKEY, device_pubkey)?
                .column_param(account_coins_table::COIN, ticker)?;

            // A constraint error occurs if **only** there is no account in `account_table` with the given `account_id`.
            // If there is the same coin for the given `account_id` already, then the insertion will be ignored.
            handle_constraint_error(sql_insert.insert(), || {
                AccountStorageError::NoSuchAccount(account_id.clone())
            })?;
        }

        transaction.commit()?;
        Ok(())
    }

    async fn deactivate_coins(&self, account_id: AccountId, tickers: Vec<String>) -> AccountStorageResult<()> {
        let conn = self.lock_conn_mutex()?;

        let mut sql_delete = SqlDelete::new(&conn, account_coins_table::TABLE_NAME)?;

        let (account_type, account_idx, device_pubkey) = account_id.to_sql_tuple();
        sql_delete
            .and_where_eq(account_coins_table::ACCOUNT_TYPE, account_type)?
            .and_where_eq(account_coins_table::ACCOUNT_IDX, account_idx)?
            .and_where_eq_param(account_coins_table::DEVICE_PUBKEY, device_pubkey)?
            .and_where_in_params(account_coins_table::COIN, tickers)?;

        let deleted = sql_delete.delete()?;
        if deleted > 0 {
            // We're sure that the account exists since there were coins associated with it.
            return Ok(());
        }

        // Check if the account exists.
        if Self::account_exists(&conn, &account_id)? {
            Ok(())
        } else {
            return MmError::err(AccountStorageError::NoSuchAccount(account_id));
        }
    }
}

fn account_id_from_row(row: &Row<'_>) -> Result<AccountId, SqlError> {
    let account_type: i64 = row.get(0)?;
    let account_idx: u32 = row.get(1)?;
    let device_pubkey: String = row.get(2)?;
    AccountId::try_from_sql_tuple(account_type, account_idx, &device_pubkey)
        .map_err(|e| SqlError::FromSqlConversionFailure(0, Type::Text, Box::new(e)))
}

fn enabled_account_id_from_row(row: &Row<'_>) -> Result<EnabledAccountId, SqlError> {
    let account_type: i64 = row.get(0)?;
    let account_idx: u32 = row.get(1)?;
    EnabledAccountId::try_from_sql_pair(account_type, account_idx)
        .map_err(|e| SqlError::FromSqlConversionFailure(0, Type::Text, Box::new(e)))
}

fn account_from_row(row: &Row<'_>) -> Result<AccountInfo, SqlError> {
    let account_id = account_id_from_row(row)?;
    let name = row.get(3)?;
    let description = row.get(4)?;
    let balance_usd = bigdecimal_from_row(row, 5)?;
    Ok(AccountInfo {
        account_id,
        name,
        description,
        balance_usd,
    })
}

fn count_from_row(row: &Row<'_>) -> Result<i64, SqlError> {
    row.get(0)
}

fn bigdecimal_from_row(row: &Row<'_>, idx: usize) -> Result<BigDecimal, SqlError> {
    let decimal: String = row.get(idx)?;
    BigDecimal::from_str(&decimal).map_err(|e| SqlError::FromSqlConversionFailure(idx, Type::Text, Box::new(e)))
}

fn handle_constraint_error<T, F>(result: SqlResult<T>, on_constraint_error: F) -> AccountStorageResult<T>
where
    F: FnOnce() -> AccountStorageError,
{
    result.map_to_mm(|e| {
        if is_constraint_error(&e) {
            on_constraint_error()
        } else {
            AccountStorageError::from(e)
        }
    })
}
