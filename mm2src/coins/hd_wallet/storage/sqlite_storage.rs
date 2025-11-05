#![allow(deprecated)] // TODO: remove this once rusqlite is >= 0.29

use crate::hd_wallet::{
    HDAccountStorageItem, HDWalletId, HDWalletStorageError, HDWalletStorageInternalOps, HDWalletStorageResult,
};
use async_trait::async_trait;
use common::async_blocking;
use db_common::owned_named_params;
use db_common::sqlite::rusqlite::{Connection, Error as SqlError, Row};
use db_common::sqlite::{
    query_single_row_with_named_params, AsSqlNamedParams, OwnedSqlNamedParams, SqliteConnShared, SqliteConnWeak,
};
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use std::convert::TryFrom;
use std::sync::MutexGuard;

const CREATE_HD_ACCOUNT_TABLE: &str = "CREATE TABLE IF NOT EXISTS hd_account (
    coin VARCHAR(255) NOT NULL,
    hd_wallet_rmd160 VARCHAR(255) NOT NULL,
    account_id INTEGER NOT NULL,
    account_xpub VARCHAR(255) NOT NULL,
    external_addresses_number INTEGER NOT NULL,
    internal_addresses_number INTEGER NOT NULL
);";

const INSERT_ACCOUNT: &str = "INSERT INTO hd_account
    (coin, hd_wallet_rmd160, account_id, account_xpub, external_addresses_number, internal_addresses_number)
    VALUES (:coin, :hd_wallet_rmd160, :account_id, :account_xpub, :external_addresses_number, :internal_addresses_number);";

const DELETE_ACCOUNTS_BY_WALLET_ID: &str =
    "DELETE FROM hd_account WHERE coin=:coin AND hd_wallet_rmd160=:hd_wallet_rmd160;";

const SELECT_ACCOUNT: &str = "SELECT account_id, account_xpub, external_addresses_number, internal_addresses_number
    FROM hd_account
    WHERE coin=:coin AND hd_wallet_rmd160=:hd_wallet_rmd160 AND account_id=:account_id;";

const SELECT_ACCOUNTS_BY_WALLET_ID: &str =
    "SELECT account_id, account_xpub, external_addresses_number, internal_addresses_number
    FROM hd_account
    WHERE coin=:coin AND hd_wallet_rmd160=:hd_wallet_rmd160;";

impl From<SqlError> for HDWalletStorageError {
    fn from(e: SqlError) -> Self {
        let error = e.to_string();
        match e {
            SqlError::FromSqlConversionFailure(_, _, _)
            | SqlError::IntegralValueOutOfRange(_, _)
            | SqlError::InvalidColumnIndex(_)
            | SqlError::InvalidColumnType(_, _, _) => HDWalletStorageError::ErrorDeserializing(error),
            SqlError::Utf8Error(_) | SqlError::NulError(_) | SqlError::ToSqlConversionFailure(_) => {
                HDWalletStorageError::ErrorSerializing(error)
            },
            _ => HDWalletStorageError::Internal(error),
        }
    }
}

impl TryFrom<&Row<'_>> for HDAccountStorageItem {
    type Error = SqlError;

    fn try_from(row: &Row<'_>) -> Result<Self, Self::Error> {
        Ok(HDAccountStorageItem {
            account_id: row.get(0)?,
            account_xpub: row.get(1)?,
            external_addresses_number: row.get(2)?,
            internal_addresses_number: row.get(3)?,
        })
    }
}

impl HDAccountStorageItem {
    fn to_sql_params_with_wallet_id(&self, wallet_id: HDWalletId) -> OwnedSqlNamedParams {
        let mut params = wallet_id.to_sql_params();
        params.extend(owned_named_params! {
            ":account_id": self.account_id,
            ":account_xpub": self.account_xpub.clone(),
            ":external_addresses_number": self.external_addresses_number,
            ":internal_addresses_number": self.internal_addresses_number,
        });
        params
    }
}

impl HDWalletId {
    fn to_sql_params(&self) -> OwnedSqlNamedParams {
        owned_named_params! {
            ":coin": self.coin.clone(),
            ":hd_wallet_rmd160": self.hd_wallet_rmd160.clone(),
        }
    }
}

#[derive(Clone)]
pub(super) struct HDWalletSqliteStorage {
    conn: SqliteConnWeak,
}

#[async_trait]
impl HDWalletStorageInternalOps for HDWalletSqliteStorage {
    async fn init(ctx: &MmArc) -> HDWalletStorageResult<Self>
    where
        Self: Sized,
    {
        let shared = ctx.shared_sqlite_conn.get().or_mm_err(|| {
            HDWalletStorageError::Internal("'MmCtx::shared_sqlite_conn' is not initialized".to_owned())
        })?;
        let storage = HDWalletSqliteStorage {
            conn: SqliteConnShared::downgrade(shared),
        };
        storage.init_tables().await?;
        Ok(storage)
    }

    async fn load_accounts(&self, wallet_id: HDWalletId) -> HDWalletStorageResult<Vec<HDAccountStorageItem>> {
        let selfi = self.clone();
        async_blocking(move || {
            let conn_shared = selfi.get_shared_conn()?;
            let conn = Self::lock_conn_mutex(&conn_shared)?;

            let mut statement = conn.prepare(SELECT_ACCOUNTS_BY_WALLET_ID)?;

            let params = wallet_id.to_sql_params();
            let rows = statement
                .query_map_named(&params.as_sql_named_params(), |row: &Row<'_>| {
                    HDAccountStorageItem::try_from(row)
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    async fn load_account(
        &self,
        wallet_id: HDWalletId,
        account_id: u32,
    ) -> HDWalletStorageResult<Option<HDAccountStorageItem>> {
        let selfi = self.clone();
        async_blocking(move || {
            let conn_shared = selfi.get_shared_conn()?;
            let conn = Self::lock_conn_mutex(&conn_shared)?;

            let mut params = wallet_id.to_sql_params();
            params.extend(owned_named_params! {
                ":account_id": account_id,
            });
            query_single_row_with_named_params(&conn, SELECT_ACCOUNT, &params.as_sql_named_params(), |row: &Row<'_>| {
                HDAccountStorageItem::try_from(row)
            })
            .map_to_mm(HDWalletStorageError::from)
        })
        .await
    }

    async fn update_external_addresses_number(
        &self,
        wallet_id: HDWalletId,
        account_id: u32,
        new_external_addresses_number: u32,
    ) -> HDWalletStorageResult<()> {
        self.update_addresses_number(
            UpdatingProperty::ExternalAddressesNumber,
            wallet_id,
            account_id,
            new_external_addresses_number,
        )
        .await
    }

    async fn update_internal_addresses_number(
        &self,
        wallet_id: HDWalletId,
        account_id: u32,
        new_internal_addresses_number: u32,
    ) -> HDWalletStorageResult<()> {
        self.update_addresses_number(
            UpdatingProperty::InternalAddressesNumber,
            wallet_id,
            account_id,
            new_internal_addresses_number,
        )
        .await
    }

    async fn upload_new_account(
        &self,
        wallet_id: HDWalletId,
        account: HDAccountStorageItem,
    ) -> HDWalletStorageResult<()> {
        let selfi = self.clone();
        async_blocking(move || {
            let conn_shared = selfi.get_shared_conn()?;
            let conn = Self::lock_conn_mutex(&conn_shared)?;

            let params = account.to_sql_params_with_wallet_id(wallet_id);
            conn.execute_named(INSERT_ACCOUNT, &params.as_sql_named_params())
                .map(|_| ())
                .map_to_mm(HDWalletStorageError::from)
        })
        .await
    }

    async fn clear_accounts(&self, wallet_id: HDWalletId) -> HDWalletStorageResult<()> {
        let selfi = self.clone();
        async_blocking(move || {
            let conn_shared = selfi.get_shared_conn()?;
            let conn = Self::lock_conn_mutex(&conn_shared)?;

            let params = wallet_id.to_sql_params();
            conn.execute_named(DELETE_ACCOUNTS_BY_WALLET_ID, &params.as_sql_named_params())
                .map(|_| ())
                .map_to_mm(HDWalletStorageError::from)
        })
        .await
    }
}

impl HDWalletSqliteStorage {
    fn get_shared_conn(&self) -> HDWalletStorageResult<SqliteConnShared> {
        self.conn
            .upgrade()
            .or_mm_err(|| HDWalletStorageError::Internal("'HDWalletSqliteStorage::conn' doesn't exist".to_owned()))
    }

    fn lock_conn_mutex(conn: &SqliteConnShared) -> HDWalletStorageResult<MutexGuard<'_, Connection>> {
        conn.lock()
            .map_to_mm(|e| HDWalletStorageError::Internal(format!("Error locking sqlite connection: {e}")))
    }

    async fn init_tables(&self) -> HDWalletStorageResult<()> {
        let conn_shared = self.get_shared_conn()?;
        let conn = Self::lock_conn_mutex(&conn_shared)?;
        conn.execute(CREATE_HD_ACCOUNT_TABLE, [])
            .map(|_| ())
            .map_to_mm(HDWalletStorageError::from)
    }

    async fn update_addresses_number(
        &self,
        updating_property: UpdatingProperty,
        wallet_id: HDWalletId,
        account_id: u32,
        new_addresses_number: u32,
    ) -> HDWalletStorageResult<()> {
        let sql = format!(
            "UPDATE hd_account SET {updating_property}=:new_value WHERE coin=:coin AND hd_wallet_rmd160=:hd_wallet_rmd160 AND account_id=:account_id;",
        );

        let selfi = self.clone();
        async_blocking(move || {
            let conn_shared = selfi.get_shared_conn()?;
            let conn = Self::lock_conn_mutex(&conn_shared)?;

            let mut params = owned_named_params! {
                ":new_value": new_addresses_number,
                ":account_id": account_id,
            };
            params.extend(wallet_id.to_sql_params());

            conn.execute_named(&sql, &params.as_sql_named_params())
                .map(|_| ())
                .map_to_mm(HDWalletStorageError::from)
        })
        .await
    }
}

#[derive(Display)]
enum UpdatingProperty {
    #[display(fmt = "external_addresses_number")]
    ExternalAddressesNumber,
    #[display(fmt = "internal_addresses_number")]
    InternalAddressesNumber,
}

/// This function is used in `hd_wallet_storage::tests`.
#[cfg(test)]
pub(crate) async fn get_all_storage_items(ctx: &MmArc) -> Vec<HDAccountStorageItem> {
    const SELECT_ALL_ACCOUNTS: &str =
        "SELECT account_id, account_xpub, external_addresses_number, internal_addresses_number FROM hd_account";

    let conn = ctx.shared_sqlite_conn();
    let mut statement = conn.prepare(SELECT_ALL_ACCOUNTS).unwrap();
    statement
        .query_map([], |row: &Row<'_>| HDAccountStorageItem::try_from(row))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}
