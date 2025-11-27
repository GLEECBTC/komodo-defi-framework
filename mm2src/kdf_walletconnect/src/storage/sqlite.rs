use async_trait::async_trait;
use common::log::debug;
use db_common::async_sql_conn::InternalError;
use db_common::sqlite::rusqlite::Result as SqlResult;
use db_common::sqlite::{query_single_row, string_from_row, CHECK_TABLE_EXISTS_SQL};
use db_common::{
    async_sql_conn::{AsyncConnError, AsyncConnection},
    sqlite::validate_table_name,
};
use futures::lock::{Mutex, MutexGuard};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use relay_rpc::domain::Topic;
use std::sync::Arc;

use super::WalletConnectStorageOps;
use crate::session::Session;

const SESSION_TABLE_NAME: &str = "wc_session";

/// Sessions table
fn create_sessions_table() -> SqlResult<String> {
    validate_table_name(SESSION_TABLE_NAME)?;
    Ok(format!(
        "CREATE TABLE IF NOT EXISTS {SESSION_TABLE_NAME} (
        topic char(32) PRIMARY KEY,
        data TEXT NOT NULL,
        expiry BIGINT NOT NULL
    );"
    ))
}

#[derive(Clone, Debug)]
pub(crate) struct SqliteSessionStorage {
    pub conn: Arc<Mutex<AsyncConnection>>,
}

impl SqliteSessionStorage {
    pub(crate) fn new(ctx: &MmArc) -> MmResult<Self, AsyncConnError> {
        let conn = ctx
            .async_sqlite_connection
            .get()
            .ok_or(AsyncConnError::Internal(InternalError(
                "async_sqlite_connection is not initialized".to_owned(),
            )))?;

        Ok(Self { conn: conn.clone() })
    }

    pub(crate) async fn lock_db(&self) -> MutexGuard<'_, AsyncConnection> {
        self.conn.lock().await
    }
}

#[async_trait]
impl WalletConnectStorageOps for SqliteSessionStorage {
    type Error = AsyncConnError;

    async fn init(&self) -> MmResult<(), Self::Error> {
        debug!("Initializing WalletConnect session storage");
        let lock = self.lock_db().await;
        lock.call(move |conn| {
            conn.execute(&create_sessions_table()?, []).map(|_| ())?;
            Ok(())
        })
        .await
        .map_to_mm(AsyncConnError::from)
    }

    async fn is_initialized(&self) -> MmResult<bool, Self::Error> {
        let lock = self.lock_db().await;
        validate_table_name(SESSION_TABLE_NAME).map_err(AsyncConnError::from)?;
        lock.call(move |conn| {
            let initialized = query_single_row(conn, CHECK_TABLE_EXISTS_SQL, [SESSION_TABLE_NAME], string_from_row)?;
            Ok(initialized.is_some())
        })
        .await
        .map_to_mm(AsyncConnError::from)
    }

    async fn save_session(&self, session: &Session) -> MmResult<(), Self::Error> {
        debug!("[{}] Saving WalletConnect session to storage", session.topic);
        let lock = self.lock_db().await;

        let session = session.clone();
        lock.call(move |conn| {
            let sql = format!("INSERT INTO {SESSION_TABLE_NAME} (topic, data, expiry) VALUES (?1, ?2, ?3);");
            let transaction = conn.transaction()?;

            let session_data = serde_json::to_string(&session).map_err(|err| AsyncConnError::from(err.to_string()))?;

            let params = [session.topic.to_string(), session_data, session.expiry.to_string()];

            transaction.execute(&sql, params)?;
            transaction.commit()?;

            Ok(())
        })
        .await
        .map_to_mm(AsyncConnError::from)
    }

    async fn get_session(&self, topic: &Topic) -> MmResult<Option<Session>, Self::Error> {
        debug!("[{topic}] Retrieving WalletConnect session from storage");
        let lock = self.lock_db().await;
        let topic = topic.clone();
        let session_str = lock
            .call(move |conn| {
                let sql = format!("SELECT topic, data, expiry FROM {SESSION_TABLE_NAME} WHERE topic=?1;");
                let mut stmt = conn.prepare(&sql)?;
                let session: String = stmt.query_row([topic.to_string()], |row| row.get::<_, String>(1))?;
                Ok(session)
            })
            .await
            .map_to_mm(AsyncConnError::from)?;

        let session = serde_json::from_str(&session_str).map_to_mm(|err| AsyncConnError::from(err.to_string()))?;
        Ok(session)
    }

    async fn get_all_sessions(&self) -> MmResult<Vec<Session>, Self::Error> {
        debug!("Loading WalletConnect sessions from storage");
        let lock = self.lock_db().await;
        let sessions_str = lock
            .call(move |conn| {
                let sql = format!("SELECT topic, data, expiry FROM {SESSION_TABLE_NAME};");
                let mut stmt = conn.prepare(&sql)?;
                let sessions = stmt.query_map([], |row| row.get::<_, String>(1))?.collect::<Vec<_>>();
                Ok(sessions)
            })
            .await
            .map_to_mm(AsyncConnError::from)?;

        let mut sessions = Vec::with_capacity(sessions_str.len());
        for session in sessions_str {
            let session = serde_json::from_str(&session.map_to_mm(AsyncConnError::from)?)
                .map_to_mm(|err| AsyncConnError::from(err.to_string()))?;
            sessions.push(session);
        }

        Ok(sessions)
    }

    async fn delete_session(&self, topic: &Topic) -> MmResult<(), Self::Error> {
        debug!("[{topic}] Deleting WalletConnect session from storage");
        let topic = topic.clone();
        let lock = self.lock_db().await;
        lock.call(move |conn| {
            let sql = format!("DELETE FROM {SESSION_TABLE_NAME} WHERE topic = ?1");
            let mut stmt = conn.prepare(&sql)?;
            let _ = stmt.execute([topic.to_string()])?;

            Ok(())
        })
        .await
        .map_to_mm(AsyncConnError::from)
    }

    async fn update_session(&self, session: &Session) -> MmResult<(), Self::Error> {
        debug!("[{}] Updating WalletConnect session in storage", session.topic);
        let session = session.clone();
        let lock = self.lock_db().await;
        lock.call(move |conn| {
            let sql = format!("UPDATE {SESSION_TABLE_NAME} SET data = ?1, expiry = ?2 WHERE topic = ?3");
            let session_data = serde_json::to_string(&session).map_err(|err| AsyncConnError::from(err.to_string()))?;
            let params = [session_data, session.expiry.to_string(), session.topic.to_string()];
            let _row = conn.prepare(&sql)?.execute(params)?;

            Ok(())
        })
        .await
        .map_to_mm(AsyncConnError::from)
    }
}
