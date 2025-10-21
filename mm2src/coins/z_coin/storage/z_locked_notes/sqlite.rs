use super::{LockedNote, LockedNotesStorage, LockedNotesStorageError};
use db_common::async_sql_conn::{AsyncConnError, AsyncConnection};
use db_common::sqlite::run_optimization_pragmas;
use db_common::sqlite::rusqlite::params;
use futures::lock::Mutex;
use itertools::Itertools;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use std::convert::TryInto;
use std::sync::Arc;

const TABLE_NAME: &str = "locked_notes_cache";

async fn create_table(conn: Arc<Mutex<AsyncConnection>>) -> Result<(), AsyncConnError> {
    let conn = conn.lock().await;
    conn.call(move |conn| {
        run_optimization_pragmas(conn)?;
        conn.execute(
            &format!(
                "CREATE TABLE IF NOT EXISTS {TABLE_NAME} (
                    variant TEXT NOT NULL,    -- 'Spent' or 'Change'
                    txid VARCHAR NOT NULL,
                    rseed VARCHAR,            -- only for Spent
                    value INTEGER,            -- only for Change
                    UNIQUE (variant, txid, rseed, value)
                )"
            ),
            [],
        )?;
        Ok(())
    }).await
}

impl LockedNotesStorage {
    #[cfg(not(any(test, feature = "run-docker-tests")))]
    pub(crate) async fn new(ctx: &MmArc, address: String) -> MmResult<Self, LockedNotesStorageError> {
        let path = ctx.wallet_dir().join(format!("{address}_locked_notes_cache.db"));
        let db = AsyncConnection::open(path)
            .await
            .map_to_mm(|err| LockedNotesStorageError::SqliteError(err.to_string()))?;
        let db = Arc::new(Mutex::new(db));

        create_table(db.clone()).await?;

        Ok(Self { db, address })
    }

    #[cfg(any(test, feature = "run-docker-tests"))]
    pub(crate) async fn new(ctx: &MmArc, address: String) -> MmResult<Self, LockedNotesStorageError> {
        #[cfg(feature = "run-docker-tests")]
        let db = {
            let path = ctx.wallet_dir().join(format!("{address}_locked_notes_cache.db"));
            mm2_io::fs::create_parents_async(&path)
                .await
                .map_err(|err| LockedNotesStorageError::SqliteError(err.to_string()))?;
            Arc::new(Mutex::new(
                AsyncConnection::open(path)
                    .await
                    .map_to_mm(|err| LockedNotesStorageError::SqliteError(err.to_string()))?,
            ))
        };
        #[cfg(all(test, not(feature = "run-docker-tests")))]
        let db = {
            let test_conn = Arc::new(Mutex::new(AsyncConnection::open_in_memory().await.unwrap()));
            ctx.async_sqlite_connection.get().cloned().unwrap_or(test_conn)
        };

        create_table(db.clone()).await?;

        Ok(Self { db, address })
    }

    pub(crate) async fn insert_spent_note(
        &self,
        txid: String,
        rseed: String,
    ) -> MmResult<(), LockedNotesStorageError> {
        let db = self.db.lock().await;
        Ok(db.call(move |conn| {
            conn.prepare(&format!(
                "INSERT OR REPLACE INTO {TABLE_NAME} (variant, txid, rseed, value) VALUES (?, ?, ?, NULL)"
            ))?
                .execute(params!["Spent", txid, rseed])?;
            Ok(())
        }).await?)
    }

    pub(crate) async fn insert_change_note(
        &self,
        txid: String,
        value: u64,
    ) -> MmResult<(), LockedNotesStorageError> {
        let db = self.db.lock().await;
        Ok(db.call(move |conn| {
            conn.prepare(&format!(
                "INSERT OR REPLACE INTO {TABLE_NAME} (variant, txid, rseed, value) VALUES (?, ?, NULL, ?)"
            ))?
                .execute(params!["Change", txid, value as i64])?;
            Ok(())
        }).await?)
    }

    pub(crate) async fn remove_notes_for_txid(&self, txid: String) -> MmResult<(), LockedNotesStorageError> {
        let db = self.db.lock().await;
        Ok(db
            .call(move |conn| {
                conn.execute(
                    &format!("DELETE FROM {TABLE_NAME} WHERE txid=?"),
                    [&txid],
                )?;
                Ok(())
            })
            .await?)
    }

    pub(crate) async fn load_all_notes(&self) -> MmResult<Vec<LockedNote>, LockedNotesStorageError> {
        let db = self.db.lock().await;
        Ok(db.call(move |conn| {
            let mut stmt = conn.prepare(&format!(
                "SELECT variant, txid, rseed, value FROM {TABLE_NAME};"
            ))?;
           let rows = stmt.query_map(params![], |row| {
                let variant: String = row.get(0)?;
                let rseed: Option<String> = row.get(2)?;
                let value: Option<i64> = row.get(3)?;

                match variant.as_str() {
                    "Spent" => {
                        let rseed = rseed.ok_or_else(|| db_common::sqlite::rusqlite::Error::FromSqlConversionFailure(
                            2, // Column index for "rseed"
                            db_common::sqlite::rusqlite::types::Type::Text,
                            "NULL value found for required rseed field".into()
                        ))?;
                        Ok(LockedNote::Spent { rseed })
                    },
                    "Change" => {
                        let i64_value = value.ok_or_else(|| db_common::sqlite::rusqlite::Error::FromSqlConversionFailure(
                            3, // Column index for "value"
                            db_common::sqlite::rusqlite::types::Type::Integer,
                            "NULL value found for required value field".into()
                        ))?;

                        let value = i64_value.try_into()
                            .map_err(|_| db_common::sqlite::rusqlite::Error::IntegralValueOutOfRange(3, i64_value))?;

                        Ok(LockedNote::Change { value })
                    },
                    unexpected => Err(db_common::sqlite::rusqlite::Error::FromSqlConversionFailure(
                        0, // Column index for "variant"
                        db_common::sqlite::rusqlite::types::Type::Text,
                        format!("Unexpected variant value: {unexpected}").into()
                    )),
                }
            })?;
            Ok(rows.flatten().collect_vec())
        }).await?)
    }
}
