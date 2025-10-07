use super::{LockedNote, LockedNotesStorage, LockedNotesStorageError};

use mm2_core::mm_ctx::MmArc;
use mm2_db::indexed_db::{ConstructibleDb, DbIdentifier, DbInstance, DbLocked, DbUpgrader, IndexedDb, IndexedDbBuilder,
                         InitDbResult, OnUpgradeResult, TableSignature, OnUpgradeError};
use mm2_err_handle::prelude::*;

const DB_NAME: &str = "z_change_note_storage";
const DB_VERSION: u32 = 1;

pub type LockedNotesDbInnerLocked<'a> = DbLocked<'a, LockedNoteDbInner>;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LockedNoteTable {
    address: String,
    variant: String,         // "Spent" or "Change"
    txid: String,
    rseed: Option<String>,   // Only for Spent
    value: Option<u64>,      // Only for Change
}

impl TableSignature for LockedNoteTable {
    const TABLE_NAME: &'static str = "change_notes";

    fn on_upgrade_needed(upgrader: &DbUpgrader, mut old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
        while old_version < new_version {
            match old_version {
                0 => {
                    let table = upgrader.create_table(Self::TABLE_NAME)?;
                    table.create_index("address", false)?;
                    table.create_index("variant", false)?;
                    table.create_index("txid", false)?;
                    table.create_index("rseed", false)?;
                    table.create_index("value", false)?;
                }
                unsupported_version => {
                    return MmError::err(OnUpgradeError::UnsupportedVersion {
                        unsupported_version,
                        old_version,
                        new_version,
                    });
                }
            }
            old_version += 1;
        }
        Ok(())
    }
}

pub struct LockedNoteDbInner(IndexedDb);

#[async_trait::async_trait]
impl DbInstance for LockedNoteDbInner {
    const DB_NAME: &'static str = DB_NAME;

    async fn init(db_id: DbIdentifier) -> InitDbResult<Self> {
        let inner = IndexedDbBuilder::new(db_id)
            .with_version(DB_VERSION)
            .with_table::<LockedNoteTable>()
            .build()
            .await?;

        Ok(Self(inner))
    }
}

impl LockedNoteDbInner {
    pub fn get_inner(&self) -> &IndexedDb { &self.0 }
}

impl LockedNotesStorage {
    async fn lockdb(&self) -> MmResult<LockedNotesDbInnerLocked<'_>, LockedNotesStorageError> {
        self.db.get_or_initialize().await.map_mm_err()
    }
}

impl LockedNotesStorage {
    pub(crate) async fn new(ctx: &MmArc, address: String) -> MmResult<Self, LockedNotesStorageError> {
        let db = ConstructibleDb::new(ctx).into_shared();
        Ok(Self { address, db })
    }

    pub(crate) async fn insert_spent_note(
        &self,
        txid: String,
        rseed: String,
    ) -> MmResult<(), LockedNotesStorageError> {
        let db = self.lockdb().await?;
        let address = self.address.clone();
        let transaction = db.get_inner().transaction().await.map_mm_err()?;
        let change_note_table = transaction.table::<LockedNoteTable>().await.map_mm_err()?;

        let change_note = LockedNoteTable {
            address,
            variant: "Spent".to_owned(),
            txid,
            rseed: Some(rseed),
            value: None,
        };
        change_note_table
            .add_item(&change_note)
            .await
            .map(|_| ()).map_mm_err()
    }

    pub(crate) async fn insert_change_note(
        &self,
        txid: String,
        value: u64,
    ) -> MmResult<(), LockedNotesStorageError> {
        let db = self.lockdb().await?;
        let address = self.address.clone();
        let transaction = db.get_inner().transaction().await.map_mm_err()?;
        let change_note_table = transaction.table::<LockedNoteTable>().await.map_mm_err()?;

        let change_note = LockedNoteTable {
            address,
            variant: "Change".to_owned(),
            txid,
            rseed: None,
            value: Some(value),
        };
        change_note_table
            .add_item(&change_note)
            .await
            .map(|_| ()).map_mm_err()
    }

        pub(crate) async fn remove_notes_for_txid(&self, txid: String) -> MmResult<(), LockedNotesStorageError> {
            let db = self.lockdb().await?;
            let transaction = db.get_inner().transaction().await.map_mm_err()?;
            let change_note_table = transaction.table::<LockedNoteTable>().await.map_mm_err()?;
            change_note_table.delete_items_by_index("txid", &txid).await.map_mm_err()?;

            Ok(())
        }

    pub(crate) async fn load_all_notes(&self) -> MmResult<Vec<LockedNote>, LockedNotesStorageError> {
        let db = self.lockdb().await?;
        let transaction = db.get_inner().transaction().await.map_mm_err()?;
        let change_note_table = transaction.table::<LockedNoteTable>().await.map_mm_err()?;
        let records = change_note_table.get_items("address", &self.address).await.map_mm_err()?;
        Ok(records
            .into_iter()
            .filter_map(|(_, n)| {
                match n.variant.as_str() {
                    "Spent" => n.rseed.clone().map(|rseed| LockedNote::Spent {
                        rseed,
                    }),
                    "Change" => n.value.map(|value| LockedNote::Change {
                        value,
                    }),
                    _ => None,
                }
            })
            .collect())
    }
}
