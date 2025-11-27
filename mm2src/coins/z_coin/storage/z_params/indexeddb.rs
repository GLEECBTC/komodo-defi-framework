use crate::z_coin::z_coin_errors::ZcoinStorageError;

use mm2_core::mm_ctx::MmArc;
use mm2_db::indexed_db::{
    ConstructibleDb, DbIdentifier, DbInstance, DbLocked, DbUpgrader, IndexedDb, IndexedDbBuilder, InitDbResult,
    OnUpgradeResult, SharedDb, TableSignature,
};
use mm2_err_handle::prelude::*;

const CHAIN: &str = "z_coin";
const DB_NAME: &str = "z_params";
const DB_VERSION: u32 = 1;
const TARGET_SPEND_CHUNKS: usize = 12;

pub(crate) type ZcashParamsWasmRes<T> = MmResult<T, ZcoinStorageError>;
pub(crate) type ZcashParamsInnerLocked<'a> = DbLocked<'a, ZcashParamsWasmInner>;

/// Since sapling_spend data way is greater than indexeddb max_data(267386880) bytes to save, we need to split
/// sapling_spend and insert to db multiple times with index(sapling_spend_id)
#[derive(Clone, Debug, Deserialize, Serialize)]
struct ZcashParamsWasmTable {
    sapling_spend_id: u8,
    sapling_spend: Vec<u8>,
    sapling_output: Vec<u8>,
    ticker: String,
}

impl ZcashParamsWasmTable {
    const SPEND_OUTPUT_INDEX: &'static str = "sapling_spend_sapling_output_index";
}

impl TableSignature for ZcashParamsWasmTable {
    const TABLE_NAME: &'static str = "z_params_bytes";

    fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
        if let (0, 1) = (old_version, new_version) {
            let table = upgrader.create_table(Self::TABLE_NAME)?;
            table.create_multi_index(Self::SPEND_OUTPUT_INDEX, &["sapling_spend", "sapling_output"], true)?;
            table.create_index("sapling_spend", false)?;
            table.create_index("sapling_output", false)?;
            table.create_index("sapling_spend_id", true)?;
            table.create_index("ticker", false)?;
        }

        Ok(())
    }
}

pub(crate) struct ZcashParamsWasmInner(IndexedDb);

#[async_trait::async_trait]
impl DbInstance for ZcashParamsWasmInner {
    const DB_NAME: &'static str = DB_NAME;

    async fn init(db_id: DbIdentifier) -> InitDbResult<Self> {
        let inner = IndexedDbBuilder::new(db_id)
            .with_version(DB_VERSION)
            .with_table::<ZcashParamsWasmTable>()
            .build()
            .await?;

        Ok(Self(inner))
    }
}

impl ZcashParamsWasmInner {
    pub(crate) fn get_inner(&self) -> &IndexedDb {
        &self.0
    }
}

#[derive(Clone)]
pub(crate) struct ZcashParamsWasmImpl(SharedDb<ZcashParamsWasmInner>);

impl ZcashParamsWasmImpl {
    pub(crate) async fn new(ctx: &MmArc) -> MmResult<Self, ZcoinStorageError> {
        Ok(Self(ConstructibleDb::new(ctx).into_shared()))
    }

    async fn lock_db(&self) -> ZcashParamsWasmRes<ZcashParamsInnerLocked<'_>> {
        self.0
            .get_or_initialize()
            .await
            .mm_err(|err| ZcoinStorageError::DbError(err.to_string()))
    }

    /// Given sapling_spend, sapling_output and sapling_spend_id, save to indexeddb storage.
    pub(crate) async fn save_params(
        &self,
        sapling_spend_id: u8,
        sapling_spend: &[u8],
        sapling_output: &[u8],
    ) -> MmResult<(), ZcoinStorageError> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let params_db = db_transaction.table::<ZcashParamsWasmTable>().await.map_mm_err()?;
        let params = ZcashParamsWasmTable {
            sapling_spend_id,
            sapling_spend: sapling_spend.to_vec(),
            sapling_output: sapling_output.to_vec(),
            ticker: CHAIN.to_string(),
        };

        params_db
            .replace_item_by_unique_index("sapling_spend_id", sapling_spend_id as u32, &params)
            .await
            .map(|_| ())
            .map_mm_err()
    }

    /// Check if z_params is previously stored.
    pub(crate) async fn check_params(&self) -> MmResult<bool, ZcoinStorageError> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let params_db = db_transaction.table::<ZcashParamsWasmTable>().await.map_mm_err()?;
        let count = params_db.count_all().await.map_mm_err()?;
        if count != TARGET_SPEND_CHUNKS {
            params_db.delete_items_by_index("ticker", CHAIN).await.map_mm_err()?;
        }

        Ok(count == TARGET_SPEND_CHUNKS)
    }

    /// Get z_params from storage.
    pub(crate) async fn get_params(&self) -> MmResult<(Vec<u8>, Vec<u8>), ZcoinStorageError> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let params_db = db_transaction.table::<ZcashParamsWasmTable>().await.map_mm_err()?;
        let mut maybe_params = params_db
            .cursor_builder()
            .only("ticker", CHAIN)
            .map_mm_err()?
            .open_cursor("ticker")
            .await
            .map_mm_err()?;

        let mut sapling_spend = vec![];
        let mut sapling_output = vec![];

        while let Some((_, params)) = maybe_params.next().await.map_mm_err()? {
            sapling_spend.extend_from_slice(&params.sapling_spend);
            if params.sapling_spend_id == 0 {
                sapling_output = params.sapling_output
            }
        }

        Ok((sapling_spend, sapling_output))
    }

    /// Download and save z_params to storage.
    pub(crate) async fn download_and_save_params(&self) -> MmResult<(Vec<u8>, Vec<u8>), ZcoinStorageError> {
        let (sapling_spend, sapling_output) = super::download_parameters()
            .await
            .mm_err(|err| ZcoinStorageError::ZcashParamsError(err.to_string()))?;

        if sapling_spend.len() <= sapling_output.len() {
            self.save_params(0, &sapling_spend, &sapling_output).await?
        } else {
            let spends = sapling_spend_to_chunks(&sapling_spend);
            if let Some((first_spend, remaining_spends)) = spends.split_first() {
                self.save_params(0, first_spend, &sapling_output).await?;

                for (i, spend) in remaining_spends.iter().enumerate() {
                    self.save_params((i + 1) as u8, spend, &[]).await?;
                }
            }
        }

        Ok((sapling_spend, sapling_output))
    }
}

/// Since sapling_spend data way is greater than indexeddb max_data(267386880) bytes to save, we need to split
/// sapling_spend into chunks of 12 and insert to db multiple times with index(sapling_spend_id)
fn sapling_spend_to_chunks(sapling_spend: &[u8]) -> Vec<&[u8]> {
    // Calculate the target size for each chunk
    let chunk_size = sapling_spend.len() / TARGET_SPEND_CHUNKS;
    // Calculate the remainder for cases when the length is not perfectly divisible
    let remainder = sapling_spend.len() % TARGET_SPEND_CHUNKS;
    let mut sapling_spend_chunks = Vec::with_capacity(TARGET_SPEND_CHUNKS);
    let mut start = 0;
    for i in 0..TARGET_SPEND_CHUNKS {
        let end = start + chunk_size + usize::from(i < remainder);
        sapling_spend_chunks.push(&sapling_spend[start..end]);
        start = end;
    }

    sapling_spend_chunks
}
