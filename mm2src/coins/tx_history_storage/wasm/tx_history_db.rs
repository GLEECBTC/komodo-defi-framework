use crate::tx_history_storage::wasm::tx_history_storage_v1::TxHistoryTableV1;
use crate::tx_history_storage::wasm::tx_history_storage_v2::{TxCacheTableV2, TxHistoryTableV2};
use async_trait::async_trait;
use mm2_db::indexed_db::{DbIdentifier, DbInstance, DbLocked, IndexedDb, IndexedDbBuilder, InitDbResult};

const DB_VERSION: u32 = 1;

pub type TxHistoryDbLocked<'a> = DbLocked<'a, TxHistoryDb>;

pub struct TxHistoryDb {
    inner: IndexedDb,
}

#[async_trait]
impl DbInstance for TxHistoryDb {
    const DB_NAME: &'static str = "tx_history";

    async fn init(db_id: DbIdentifier) -> InitDbResult<Self> {
        let inner = IndexedDbBuilder::new(db_id)
            .with_version(DB_VERSION)
            .with_table::<TxHistoryTableV1>()
            .with_table::<TxHistoryTableV2>()
            .with_table::<TxCacheTableV2>()
            .build()
            .await?;
        Ok(TxHistoryDb { inner })
    }
}

impl TxHistoryDb {
    pub fn get_inner(&self) -> &IndexedDb {
        &self.inner
    }
}
