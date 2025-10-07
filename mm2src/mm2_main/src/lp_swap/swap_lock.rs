use async_trait::async_trait;
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use uuid::Uuid;

#[cfg(target_arch = "wasm32")]
use common::now_sec;
#[cfg(not(target_arch = "wasm32"))]
pub use native_lock::SwapLock;
#[cfg(target_arch = "wasm32")]
pub use wasm_lock::SwapLock;

pub type SwapLockResult<T> = Result<T, MmError<SwapLockError>>;

#[derive(Debug, Display)]
pub enum SwapLockError {
    #[display(fmt = "Error reading timestamp: {_0}")]
    ErrorReadingTimestamp(String),
    #[display(fmt = "Error writing timestamp: {_0}")]
    ErrorWritingTimestamp(String),
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    #[display(fmt = "Internal error: {_0}")]
    InternalError(String),
}

#[async_trait]
pub trait SwapLockOps: Sized {
    async fn lock(ctx: &MmArc, swap_uuid: Uuid, ttl_sec: f64) -> SwapLockResult<Option<Self>>;

    async fn touch(&self) -> SwapLockResult<()>;
}

#[cfg(not(target_arch = "wasm32"))]
mod native_lock {
    use super::*;
    use mm2_io::file_lock::{FileLock, FileLockError};
    use std::path::PathBuf;

    impl From<FileLockError> for SwapLockError {
        fn from(e: FileLockError) -> Self {
            match e {
                FileLockError::ErrorReadingTimestamp { path, error } => {
                    SwapLockError::ErrorReadingTimestamp(format!("Path: {path:?}, Error: {error}"))
                },
                FileLockError::ErrorWritingTimestamp { path, error }
                | FileLockError::ErrorCreatingLockFile { path, error } => {
                    SwapLockError::ErrorWritingTimestamp(format!("Path: {path:?}, Error: {error}"))
                },
            }
        }
    }

    pub struct SwapLock {
        file_lock: FileLock<PathBuf>,
    }

    #[async_trait]
    impl SwapLockOps for SwapLock {
        async fn lock(ctx: &MmArc, swap_uuid: Uuid, ttl_sec: f64) -> SwapLockResult<Option<SwapLock>> {
            let lock_path = if cfg!(feature = "new-db-arch") {
                ctx.global_dir().join("swap_locks").join(format!("{swap_uuid}.lock"))
            } else {
                ctx.global_dir()
                    .join("SWAPS")
                    .join("MY")
                    .join(format!("{swap_uuid}.lock"))
            };
            let file_lock = some_or_return_ok_none!(FileLock::lock(lock_path, ttl_sec).map_mm_err()?);

            Ok(Some(SwapLock { file_lock }))
        }

        async fn touch(&self) -> SwapLockResult<()> {
            Ok(self.file_lock.touch().map_mm_err()?)
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_lock {
    use super::*;
    use crate::lp_swap::swap_wasm_db::{DbTransactionError, InitDbError, ItemId, SwapLockTable};
    use crate::lp_swap::SwapsContext;
    use common::executor::SpawnFuture;
    use common::log::{debug, error};
    use common::{now_float, now_ms};

    impl From<DbTransactionError> for SwapLockError {
        fn from(e: DbTransactionError) -> Self {
            match e {
                e @ DbTransactionError::NoSuchTable { .. }
                | e @ DbTransactionError::ErrorCreatingTransaction(_)
                | e @ DbTransactionError::ErrorOpeningTable { .. }
                | e @ DbTransactionError::ErrorSerializingIndex { .. }
                | e @ DbTransactionError::ErrorSerializingItem(_)
                | e @ DbTransactionError::MultipleItemsByUniqueIndex { .. }
                | e @ DbTransactionError::NoSuchIndex { .. }
                | e @ DbTransactionError::InvalidIndex { .. }
                | e @ DbTransactionError::UnexpectedState(_)
                | e @ DbTransactionError::TransactionAborted => SwapLockError::InternalError(e.to_string()),
                e @ DbTransactionError::ErrorDeserializingItem(_)
                | e @ DbTransactionError::ErrorGettingItems(_)
                | e @ DbTransactionError::ErrorCountingItems(_) => SwapLockError::ErrorReadingTimestamp(e.to_string()),
                e @ DbTransactionError::ErrorDeletingItems(_) | e @ DbTransactionError::ErrorUploadingItem(_) => {
                    SwapLockError::ErrorWritingTimestamp(e.to_string())
                },
            }
        }
    }

    impl From<InitDbError> for SwapLockError {
        fn from(e: InitDbError) -> Self {
            SwapLockError::InternalError(e.to_string())
        }
    }

    pub struct SwapLock {
        ctx: MmArc,
        swap_uuid: Uuid,
        /// The identifier of the timestamp record in the `SwapLockTable`.
        pub(super) record_id: ItemId,
    }

    impl Drop for SwapLock {
        fn drop(&mut self) {
            let ctx = self.ctx.clone();
            let record_id = self.record_id;
            let fut = async move {
                if let Err(e) = Self::release(ctx, record_id).await {
                    error!("Error realising the SwapLock: {}", e);
                }
                debug!("SwapLock::drop] Finish");
            };
            self.ctx.spawner().spawn(fut);
        }
    }

    #[async_trait]
    impl SwapLockOps for SwapLock {
        async fn lock(ctx: &MmArc, uuid: Uuid, ttl_sec: f64) -> SwapLockResult<Option<Self>> {
            let swaps_ctx = SwapsContext::from_ctx(ctx).map_to_mm(SwapLockError::InternalError)?;
            let db = swaps_ctx.swap_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<SwapLockTable>().await.map_mm_err()?;

            if let Some((item_id, SwapLockTable { timestamp, .. })) =
                table.get_item_by_unique_index("uuid", uuid).await.map_mm_err()?
            {
                let time_passed = now_float() - timestamp as f64;
                if time_passed <= ttl_sec {
                    return Ok(None);
                }
                // delete the timestamp from the table before the new timestamp is written
                table.delete_item(item_id).await.map_mm_err()?;
            }

            let item = SwapLockTable {
                uuid,
                timestamp: now_sec(),
            };
            let record_id = table.add_item(&item).await.map_mm_err()?;

            Ok(Some(SwapLock {
                ctx: ctx.clone(),
                swap_uuid: uuid,
                record_id,
            }))
        }

        async fn touch(&self) -> SwapLockResult<()> {
            let swaps_ctx = SwapsContext::from_ctx(&self.ctx).map_to_mm(SwapLockError::InternalError)?;
            let db = swaps_ctx.swap_db().await.map_mm_err()?;

            let item = SwapLockTable {
                uuid: self.swap_uuid,
                timestamp: now_sec(),
            };

            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<SwapLockTable>().await.map_mm_err()?;

            let replaced_record_id = table.replace_item(self.record_id, &item).await.map_mm_err()?;

            if self.record_id != replaced_record_id {
                let error = format!("Expected {} record id, found {}", self.record_id, replaced_record_id);
                return MmError::err(SwapLockError::ErrorWritingTimestamp(error));
            }
            Ok(())
        }
    }

    impl SwapLock {
        async fn release(ctx: MmArc, record_id: ItemId) -> SwapLockResult<()> {
            let swaps_ctx = SwapsContext::from_ctx(&ctx).map_to_mm(SwapLockError::InternalError)?;
            let db = swaps_ctx.swap_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<SwapLockTable>().await.map_mm_err()?;
            table.delete_item(record_id).await.map_mm_err()?;
            Ok(())
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod tests {
    use super::wasm_lock::*;
    use super::*;
    use crate::lp_swap::swap_wasm_db::SwapLockTable;
    use crate::lp_swap::SwapsContext;
    use common::executor::Timer;
    use common::new_uuid;
    use common::now_ms;
    use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};
    use mm2_db::indexed_db::ItemId;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    async fn get_all_items(ctx: &MmArc) -> Vec<(ItemId, SwapLockTable)> {
        let swaps_ctx = SwapsContext::from_ctx(ctx).unwrap();
        let db = swaps_ctx.swap_db().await.expect("Error getting SwapDb");
        let transaction = db.transaction().await.expect("Error creating transaction");
        let table = transaction.table::<SwapLockTable>().await.expect("Error opening table");
        table.get_all_items().await.expect("Error getting items")
    }

    #[wasm_bindgen_test]
    async fn test_file_lock_should_create_file_and_record_timestamp_and_then_delete_on_drop() {
        let ctx = MmCtxBuilder::new().with_test_db_namespace().into_mm_arc();
        let uuid = new_uuid();

        let started_at = now_sec();
        let swap_lock = SwapLock::lock(&ctx, uuid, 10.)
            .await
            .expect("!SwapLock::lock")
            .expect("SwapLock::lock must return a value");

        let items = get_all_items(&ctx).await;
        assert_eq!(items.len(), 1);
        let (record_id, lock_item) = items[0].clone();

        assert_eq!(record_id, swap_lock.record_id);
        assert_eq!(lock_item.uuid, uuid);
        assert!(started_at <= lock_item.timestamp);

        drop(swap_lock);

        Timer::sleep(1.).await;
        let actual = get_all_items(&ctx).await;
        assert!(
            actual.is_empty(),
            "SwapLockTable must be empty after the SwapLock is dropped"
        );
    }

    #[wasm_bindgen_test]
    async fn test_file_lock_should_return_none_if_lock_acquired() {
        let ctx = MmCtxBuilder::new().with_test_db_namespace().into_mm_arc();
        let uuid = new_uuid();
        let _lock = SwapLock::lock(&ctx, uuid, 10.)
            .await
            .expect("!SwapLock::lock")
            .expect("SwapLock::lock must return a value");
        let new_lock = SwapLock::lock(&ctx, uuid, 10.).await.expect("!SwapLock::lock");
        assert!(
            new_lock.is_none(),
            "SwapLock::lock must return None if the lock has already been acquired"
        );
    }

    #[wasm_bindgen_test]
    async fn test_file_lock_should_acquire_and_update_timestamp_if_ttl_expired() {
        let ctx = MmCtxBuilder::new().with_test_db_namespace().into_mm_arc();
        let uuid = new_uuid();

        let started_at = now_sec();
        let first_lock = SwapLock::lock(&ctx, uuid, 1.)
            .await
            .expect("!SwapLock::lock")
            .expect("SwapLock::lock must return a value");

        let items = get_all_items(&ctx).await;
        assert_eq!(items.len(), 1);
        let (first_record_id, first_lock_item) = items[0].clone();

        assert_eq!(first_record_id, first_lock.record_id);
        assert_eq!(first_lock_item.uuid, uuid);
        assert!(started_at <= first_lock_item.timestamp);

        Timer::sleep(2.).await;

        let second_lock = SwapLock::lock(&ctx, uuid, 1.)
            .await
            .expect("!SwapLock::lock")
            .expect("SwapLock::lock must return a value after the last ttl is over");

        let items = get_all_items(&ctx).await;
        assert_eq!(items.len(), 1);
        let (second_record_id, second_lock_item) = items[0].clone();

        assert_eq!(second_record_id, second_lock.record_id);
        assert_ne!(first_record_id, second_record_id);
        assert_eq!(second_lock_item.uuid, uuid);
        assert!(first_lock_item.timestamp < second_lock_item.timestamp);
    }
}
