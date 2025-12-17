use async_trait::async_trait;
use mm2_db::indexed_db::{
    DbIdentifier, DbInstance, DbUpgrader, IndexedDb, IndexedDbBuilder, OnUpgradeResult, TableSignature,
};
use std::ops::Deref;
use uuid::Uuid;

pub use mm2_db::indexed_db::{
    cursor_prelude, DbTransactionError, DbTransactionResult, InitDbError, InitDbResult, ItemId,
};
pub use tables::{
    MyActiveMakerOrdersTable, MyActiveTakerOrdersTable, MyFilteringHistoryOrdersTable, MyHistoryOrdersTable,
};

const DB_VERSION: u32 = 1;

pub struct OrdermatchDb {
    inner: IndexedDb,
}

#[async_trait]
impl DbInstance for OrdermatchDb {
    const DB_NAME: &'static str = "ordermatch";

    async fn init(db_id: DbIdentifier) -> InitDbResult<Self> {
        let inner = IndexedDbBuilder::new(db_id)
            .with_version(DB_VERSION)
            .with_table::<MyActiveMakerOrdersTable>()
            .with_table::<MyActiveTakerOrdersTable>()
            .with_table::<MyHistoryOrdersTable>()
            .with_table::<MyFilteringHistoryOrdersTable>()
            .build()
            .await?;
        Ok(OrdermatchDb { inner })
    }
}

impl Deref for OrdermatchDb {
    type Target = IndexedDb;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

pub mod tables {
    use super::*;
    use crate::lp_ordermatch::{MakerOrder, Order, TakerOrder};
    use serde_json::Value as Json;

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    pub struct MyActiveMakerOrdersTable {
        pub uuid: Uuid,
        pub order_payload: MakerOrder,
    }

    impl TableSignature for MyActiveMakerOrdersTable {
        const TABLE_NAME: &'static str = "my_active_maker_orders";

        fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
            on_upgrade_swap_table_by_uuid_v1(upgrader, old_version, new_version, Self::TABLE_NAME)
        }
    }

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    pub struct MyActiveTakerOrdersTable {
        pub uuid: Uuid,
        pub order_payload: TakerOrder,
    }

    impl TableSignature for MyActiveTakerOrdersTable {
        const TABLE_NAME: &'static str = "my_active_taker_orders";

        fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
            on_upgrade_swap_table_by_uuid_v1(upgrader, old_version, new_version, Self::TABLE_NAME)
        }
    }

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    pub struct MyHistoryOrdersTable {
        pub uuid: Uuid,
        pub order_payload: Order,
    }

    impl TableSignature for MyHistoryOrdersTable {
        const TABLE_NAME: &'static str = "my_history_orders";

        fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
            on_upgrade_swap_table_by_uuid_v1(upgrader, old_version, new_version, Self::TABLE_NAME)
        }
    }

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    pub struct MyFilteringHistoryOrdersTable {
        pub uuid: Uuid,
        pub order_type: String,
        pub initial_action: String,
        pub base: String,
        pub rel: String,
        pub price: f64,
        pub volume: f64,
        pub created_at: u32,
        pub last_updated: u32,
        pub was_taker: bool,
        pub status: String,
    }

    impl TableSignature for MyFilteringHistoryOrdersTable {
        const TABLE_NAME: &'static str = "my_filtering_history_orders";

        fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
            if let (0, 1) = (old_version, new_version) {
                let table = upgrader.create_table(Self::TABLE_NAME)?;
                table.create_index("uuid", true)?;
                // TODO add other indexes during [`MyOrdersStorage::select_orders_by_filter`] implementation.
            }
            Ok(())
        }
    }

    /// [`TableSignature::on_upgrade_needed`] implementation common for the most tables with the only `uuid` unique index.
    fn on_upgrade_swap_table_by_uuid_v1(
        upgrader: &DbUpgrader,
        old_version: u32,
        new_version: u32,
        table_name: &'static str,
    ) -> OnUpgradeResult<()> {
        if let (0, 1) = (old_version, new_version) {
            let table = upgrader.create_table(table_name)?;
            table.create_index("uuid", true)?;
        }
        Ok(())
    }
}
