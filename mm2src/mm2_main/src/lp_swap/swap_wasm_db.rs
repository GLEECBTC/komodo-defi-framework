use async_trait::async_trait;
use mm2_db::indexed_db::{
    DbIdentifier, DbInstance, DbUpgrader, IndexedDb, IndexedDbBuilder, OnUpgradeError, OnUpgradeResult, TableSignature,
};
use std::ops::Deref;
use uuid::Uuid;

pub use mm2_db::indexed_db::{
    cursor_prelude, DbTransactionError, DbTransactionResult, InitDbError, InitDbResult, ItemId,
};
pub use tables::{MySwapsFiltersTable, SavedSwapTable, SwapLockTable, SwapsMigrationTable};

const DB_NAME: &str = "swap";
const DB_VERSION: u32 = 2;

pub const IS_FINISHED_SWAP_TYPE_INDEX: &str = "is_finished_swap_type";

pub struct SwapDb {
    inner: IndexedDb,
}

#[async_trait]
impl DbInstance for SwapDb {
    const DB_NAME: &'static str = "swap";

    async fn init(db_id: DbIdentifier) -> InitDbResult<Self> {
        let inner = IndexedDbBuilder::new(db_id)
            .with_version(DB_VERSION)
            .with_table::<SwapLockTable>()
            .with_table::<SavedSwapTable>()
            .with_table::<MySwapsFiltersTable>()
            .with_table::<SwapsMigrationTable>()
            .build()
            .await?;
        Ok(SwapDb { inner })
    }
}

impl Deref for SwapDb {
    type Target = IndexedDb;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

pub mod tables {
    use super::*;
    use common::bool_as_int::BoolAsInt;
    use mm2_err_handle::prelude::MmError;
    use serde_json::Value as Json;

    #[derive(Debug, Deserialize, Clone, Serialize, PartialEq)]
    pub struct SwapLockTable {
        pub uuid: Uuid,
        pub timestamp: u64,
    }

    impl TableSignature for SwapLockTable {
        const TABLE_NAME: &'static str = "swap_lock";

        fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
            on_upgrade_swap_table_by_uuid_v1(upgrader, old_version, new_version, Self::TABLE_NAME)
        }
    }

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    pub struct SavedSwapTable {
        pub uuid: Uuid,
        pub saved_swap: Json,
    }

    impl TableSignature for SavedSwapTable {
        const TABLE_NAME: &'static str = "saved_swap";

        fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
            on_upgrade_swap_table_by_uuid_v1(upgrader, old_version, new_version, Self::TABLE_NAME)
        }
    }

    /// This table is used to select uuids applying given filters.
    /// When we iterate over an index like `["my_coin", "other_coin"]`, a cursor returns items with all fields.
    /// So, if we combine `SavedSwapTable` and `MySwapsFiltersTable` into one, we will get `saved_swap` on every cursor callback that is overhead.
    #[derive(Debug, Serialize, Deserialize, Eq, Ord, PartialEq, PartialOrd)]
    pub struct MySwapsFiltersTable {
        pub uuid: Uuid,
        pub my_coin: String,
        pub other_coin: String,
        pub started_at: u32,
        #[serde(default)]
        pub is_finished: BoolAsInt,
        #[serde(default)]
        pub swap_type: u8,
    }

    impl TableSignature for MySwapsFiltersTable {
        const TABLE_NAME: &'static str = "my_swaps";

        fn on_upgrade_needed(upgrader: &DbUpgrader, mut old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
            while old_version < new_version {
                match old_version {
                    0 => {
                        let table = upgrader.create_table(Self::TABLE_NAME)?;
                        table.create_index("uuid", true)?;
                        table.create_index("started_at", false)?;
                        table.create_multi_index("with_my_coin", &["my_coin", "started_at"], false)?;
                        table.create_multi_index("with_other_coin", &["other_coin", "started_at"], false)?;
                        table.create_multi_index(
                            "with_my_other_coins",
                            &["my_coin", "other_coin", "started_at"],
                            false,
                        )?;
                    },
                    1 => {
                        let table = upgrader.open_table(Self::TABLE_NAME)?;
                        table.create_multi_index(IS_FINISHED_SWAP_TYPE_INDEX, &["is_finished", "swap_type"], false)?;
                    },
                    unsupported_version => {
                        return MmError::err(OnUpgradeError::UnsupportedVersion {
                            unsupported_version,
                            old_version,
                            new_version,
                        })
                    },
                }

                old_version += 1;
            }
            Ok(())
        }
    }

    /// [`TableSignature::on_upgrade_needed`] implementation common for the most tables with the only `uuid` unique index.
    fn on_upgrade_swap_table_by_uuid_v1(
        upgrader: &DbUpgrader,
        mut old_version: u32,
        new_version: u32,
        table_name: &'static str,
    ) -> OnUpgradeResult<()> {
        while old_version < new_version {
            match old_version {
                0 => {
                    let table = upgrader.create_table(table_name)?;
                    table.create_index("uuid", true)?;
                },
                1 => {
                    // do nothing explicitly because no action is required for SwapLockTable and SavedSwapTable
                },
                unsupported_version => {
                    return MmError::err(OnUpgradeError::UnsupportedVersion {
                        unsupported_version,
                        old_version,
                        new_version,
                    })
                },
            }

            old_version += 1;
        }
        Ok(())
    }

    #[derive(Deserialize, Serialize)]
    pub struct SwapsMigrationTable {
        pub(crate) migration: u32,
    }

    impl TableSignature for SwapsMigrationTable {
        const TABLE_NAME: &'static str = "swaps_migration";

        fn on_upgrade_needed(upgrader: &DbUpgrader, mut old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
            while old_version < new_version {
                match old_version {
                    0 => {
                        // do nothing explicitly because the table should be created on upgrade
                        // from version 1 to 2 in order to avoid breaking existing databases
                    },
                    1 => {
                        let table = upgrader.create_table(Self::TABLE_NAME)?;
                        table.create_index("migration", true)?;
                    },
                    unsupported_version => {
                        return MmError::err(OnUpgradeError::UnsupportedVersion {
                            unsupported_version,
                            old_version,
                            new_version,
                        })
                    },
                }

                old_version += 1;
            }
            Ok(())
        }
    }
}
