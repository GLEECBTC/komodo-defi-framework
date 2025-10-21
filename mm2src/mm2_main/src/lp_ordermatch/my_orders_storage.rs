use super::{
    MakerOrder, MakerOrderCancellationReason, MyOrdersFilter, Order, RecentOrdersSelectResult, TakerOrder,
    TakerOrderCancellationReason,
};
use async_trait::async_trait;
use common::log::LogOnError;
use common::{BoxFut, PagingOptions};
use derive_more::Display;
use futures::{FutureExt, TryFutureExt};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
#[cfg(test)]
use mocktopus::macros::*;
use uuid::Uuid;

pub type MyOrdersResult<T> = Result<T, MmError<MyOrdersError>>;

#[cfg(target_arch = "wasm32")]
pub use wasm_impl::MyOrdersStorage;

#[cfg(not(target_arch = "wasm32"))]
pub use native_impl::MyOrdersStorage;

#[derive(Debug, Display, Eq, PartialEq)]
pub enum MyOrdersError {
    #[display(fmt = "Order with uuid {uuid} is not found")]
    NoSuchOrder { uuid: Uuid },
    #[display(fmt = "Error saving an order: {_0}")]
    ErrorSaving(String),
    #[display(fmt = "Error loading an order: {_0}")]
    ErrorLoading(String),
    #[display(fmt = "Error deserializing an order: {_0}")]
    ErrorDeserializing(String),
    #[display(fmt = "Error serializing an order: {_0}")]
    ErrorSerializing(String),
    #[allow(dead_code)]
    #[display(fmt = "Internal error: {_0}")]
    InternalError(String),
}

pub async fn save_my_new_maker_order(ctx: MmArc, order: &MakerOrder) -> MyOrdersResult<()> {
    let storage = MyOrdersStorage::new(ctx);
    storage
        .save_new_active_maker_order(order)
        .await
        .error_log_with_msg("!save_new_active_maker_order");

    if order.save_in_history {
        storage.save_maker_order_in_filtering_history(order).await?;
    }
    Ok(())
}

pub async fn save_my_new_taker_order(ctx: MmArc, order: &TakerOrder) -> MyOrdersResult<()> {
    let storage = MyOrdersStorage::new(ctx);
    storage
        .save_new_active_taker_order(order)
        .await
        .error_log_with_msg("!save_new_active_taker_order");

    if order.save_in_history {
        storage.save_taker_order_in_filtering_history(order).await?;
    }
    Ok(())
}

pub async fn save_maker_order_on_update(ctx: MmArc, order: &MakerOrder) -> MyOrdersResult<()> {
    let storage = MyOrdersStorage::new(ctx);
    storage.update_active_maker_order(order).await?;

    if order.save_in_history {
        storage.update_maker_order_in_filtering_history(order).await?;
    }
    Ok(())
}

#[cfg_attr(test, mockable)]
pub fn delete_my_taker_order(ctx: MmArc, order: TakerOrder, reason: TakerOrderCancellationReason) -> BoxFut<(), ()> {
    let fut = async move {
        let uuid = order.request.uuid;
        let save_in_history = order.save_in_history;

        let storage = MyOrdersStorage::new(ctx);
        storage
            .delete_active_taker_order(uuid)
            .await
            .error_log_with_msg("!delete_active_taker_order");

        match reason {
            TakerOrderCancellationReason::ToMaker => (),
            _ => {
                if save_in_history {
                    storage
                        .save_order_in_history(&Order::Taker(order))
                        .await
                        .error_log_with_msg("!save_order_in_history");
                }
            },
        }

        if save_in_history {
            storage
                .update_order_status_in_filtering_history(uuid, reason.to_string())
                .await
                .error_log_with_msg("!update_order_status_in_filtering_history");
        }
        Ok(())
    };
    Box::new(fut.boxed().compat())
}

#[cfg_attr(test, mockable)]
pub fn delete_my_maker_order(ctx: MmArc, order: MakerOrder, reason: MakerOrderCancellationReason) -> BoxFut<(), ()> {
    let fut = async move {
        let mut order_to_save = order;
        let uuid = order_to_save.uuid;
        let save_in_history = order_to_save.save_in_history;

        let storage = MyOrdersStorage::new(ctx);
        if order_to_save.was_updated() {
            if let Ok(order_from_file) = storage.load_active_maker_order(order_to_save.uuid).await {
                order_to_save = order_from_file;
            }
        }
        storage
            .delete_active_maker_order(uuid)
            .await
            .error_log_with_msg("!delete_active_maker_order");

        if save_in_history {
            storage
                .save_order_in_history(&Order::Maker(order_to_save.clone()))
                .await
                .error_log_with_msg("!save_order_in_history");
            storage
                .update_order_status_in_filtering_history(uuid, reason.to_string())
                .await
                .error_log_with_msg("!update_order_status_in_filtering_history");
        }
        Ok(())
    };
    Box::new(fut.boxed().compat())
}

#[async_trait]
pub trait MyActiveOrders {
    async fn load_active_maker_orders(&self) -> MyOrdersResult<Vec<MakerOrder>>;

    async fn load_active_maker_order(&self, uuid: Uuid) -> MyOrdersResult<MakerOrder>;

    async fn load_active_taker_orders(&self) -> MyOrdersResult<Vec<TakerOrder>>;

    #[expect(dead_code)]
    async fn save_new_active_order(&self, order: &Order) -> MyOrdersResult<()> {
        match order {
            Order::Maker(maker) => self.save_new_active_maker_order(maker).await,
            Order::Taker(taker) => self.save_new_active_taker_order(taker).await,
        }
    }

    async fn save_new_active_maker_order(&self, order: &MakerOrder) -> MyOrdersResult<()>;

    async fn save_new_active_taker_order(&self, order: &TakerOrder) -> MyOrdersResult<()>;

    async fn delete_active_maker_order(&self, uuid: Uuid) -> MyOrdersResult<()>;

    async fn delete_active_taker_order(&self, uuid: Uuid) -> MyOrdersResult<()>;

    async fn update_active_maker_order(&self, order: &MakerOrder) -> MyOrdersResult<()>;

    async fn update_active_taker_order(&self, order: &TakerOrder) -> MyOrdersResult<()>;
}

#[async_trait]
pub trait MyOrdersHistory {
    async fn save_order_in_history(&self, order: &Order) -> MyOrdersResult<()>;

    async fn load_order_from_history(&self, uuid: Uuid) -> MyOrdersResult<Order>;
}

#[async_trait]
pub trait MyOrdersFilteringHistory {
    async fn select_orders_by_filter(
        &self,
        filter: &MyOrdersFilter,
        paging_options: Option<&PagingOptions>,
    ) -> MyOrdersResult<RecentOrdersSelectResult>;

    async fn select_order_status(&self, uuid: Uuid) -> MyOrdersResult<String>;

    #[expect(dead_code)]
    async fn save_order_in_filtering_history(&self, order: &Order) -> MyOrdersResult<()> {
        match order {
            Order::Maker(maker) => self.save_maker_order_in_filtering_history(maker).await,
            Order::Taker(taker) => self.save_taker_order_in_filtering_history(taker).await,
        }
    }

    async fn save_maker_order_in_filtering_history(&self, order: &MakerOrder) -> MyOrdersResult<()>;

    async fn save_taker_order_in_filtering_history(&self, order: &TakerOrder) -> MyOrdersResult<()>;

    async fn update_maker_order_in_filtering_history(&self, order: &MakerOrder) -> MyOrdersResult<()>;

    async fn update_order_status_in_filtering_history(&self, uuid: Uuid, status: String) -> MyOrdersResult<()>;

    async fn update_was_taker_in_filtering_history(&self, uuid: Uuid) -> MyOrdersResult<()>;
}

#[cfg(not(target_arch = "wasm32"))]
mod native_impl {
    use super::*;
    use crate::database::my_orders::{
        insert_maker_order, insert_taker_order, select_orders_by_filter, select_status_by_uuid, update_maker_order,
        update_order_status, update_was_taker,
    };
    use crate::lp_ordermatch::{
        my_maker_order_file_path, my_maker_orders_dir, my_order_history_file_path, my_taker_order_file_path,
        my_taker_orders_dir,
    };
    use mm2_io::fs::{read_dir_json, read_json, remove_file_async, write_json, FsJsonError};

    const USE_TMP_FILE: bool = true;

    impl From<FsJsonError> for MyOrdersError {
        fn from(fs: FsJsonError) -> Self {
            match fs {
                FsJsonError::IoReading(reading) => MyOrdersError::ErrorLoading(reading.to_string()),
                FsJsonError::IoWriting(writing) => MyOrdersError::ErrorSaving(writing.to_string()),
                FsJsonError::Serializing(serializing) => MyOrdersError::ErrorSerializing(serializing.to_string()),
                FsJsonError::Deserializing(deserializing) => {
                    MyOrdersError::ErrorDeserializing(deserializing.to_string())
                },
            }
        }
    }

    #[derive(Clone)]
    pub struct MyOrdersStorage {
        ctx: MmArc,
    }

    impl MyOrdersStorage {
        pub fn new(ctx: MmArc) -> MyOrdersStorage {
            MyOrdersStorage { ctx }
        }
    }

    #[async_trait]
    impl MyActiveOrders for MyOrdersStorage {
        async fn load_active_maker_orders(&self) -> MyOrdersResult<Vec<MakerOrder>> {
            let dir_path = my_maker_orders_dir(&self.ctx);
            Ok(read_dir_json(&dir_path).await.map_mm_err()?)
        }

        async fn load_active_maker_order(&self, uuid: Uuid) -> MyOrdersResult<MakerOrder> {
            let path = my_maker_order_file_path(&self.ctx, &uuid);
            read_json(&path)
                .await
                .map_mm_err()?
                .or_mm_err(|| MyOrdersError::NoSuchOrder { uuid })
        }

        async fn load_active_taker_orders(&self) -> MyOrdersResult<Vec<TakerOrder>> {
            let dir_path = my_taker_orders_dir(&self.ctx);
            Ok(read_dir_json(&dir_path).await.map_mm_err()?)
        }

        async fn save_new_active_maker_order(&self, order: &MakerOrder) -> MyOrdersResult<()> {
            let path = my_maker_order_file_path(&self.ctx, &order.uuid);
            write_json(order, &path, USE_TMP_FILE).await.map_mm_err()?;
            Ok(())
        }

        async fn save_new_active_taker_order(&self, order: &TakerOrder) -> MyOrdersResult<()> {
            let path = my_taker_order_file_path(&self.ctx, &order.request.uuid);
            write_json(order, &path, USE_TMP_FILE).await.map_mm_err()?;
            Ok(())
        }

        async fn delete_active_maker_order(&self, uuid: Uuid) -> MyOrdersResult<()> {
            let path = my_maker_order_file_path(&self.ctx, &uuid);
            remove_file_async(&path)
                .await
                .mm_err(|e| MyOrdersError::ErrorSaving(e.to_string()))?;
            Ok(())
        }

        async fn delete_active_taker_order(&self, uuid: Uuid) -> MyOrdersResult<()> {
            let path = my_taker_order_file_path(&self.ctx, &uuid);
            remove_file_async(&path)
                .await
                .mm_err(|e| MyOrdersError::ErrorSaving(e.to_string()))?;
            Ok(())
        }

        async fn update_active_maker_order(&self, order: &MakerOrder) -> MyOrdersResult<()> {
            self.save_new_active_maker_order(order).await
        }

        async fn update_active_taker_order(&self, order: &TakerOrder) -> MyOrdersResult<()> {
            self.save_new_active_taker_order(order).await
        }
    }

    #[async_trait]
    impl MyOrdersHistory for MyOrdersStorage {
        async fn save_order_in_history(&self, order: &Order) -> MyOrdersResult<()> {
            let path = my_order_history_file_path(&self.ctx, &order.uuid());
            write_json(order, &path, USE_TMP_FILE).await.map_mm_err()?;
            Ok(())
        }

        async fn load_order_from_history(&self, uuid: Uuid) -> MyOrdersResult<Order> {
            let path = my_order_history_file_path(&self.ctx, &uuid);
            read_json(&path)
                .await
                .map_mm_err()?
                .or_mm_err(|| MyOrdersError::NoSuchOrder { uuid })
        }
    }

    #[async_trait]
    impl MyOrdersFilteringHistory for MyOrdersStorage {
        async fn select_orders_by_filter(
            &self,
            filter: &MyOrdersFilter,
            paging_options: Option<&PagingOptions>,
        ) -> MyOrdersResult<RecentOrdersSelectResult> {
            select_orders_by_filter(&self.ctx.sqlite_connection(), filter, paging_options)
                .map_to_mm(|e| MyOrdersError::ErrorLoading(e.to_string()))
        }

        async fn select_order_status(&self, uuid: Uuid) -> MyOrdersResult<String> {
            select_status_by_uuid(&self.ctx.sqlite_connection(), &uuid)
                .map_to_mm(|e| MyOrdersError::ErrorLoading(e.to_string()))
        }

        async fn save_maker_order_in_filtering_history(&self, order: &MakerOrder) -> MyOrdersResult<()> {
            insert_maker_order(&self.ctx, order.uuid, order).map_to_mm(|e| MyOrdersError::ErrorSaving(e.to_string()))
        }

        async fn save_taker_order_in_filtering_history(&self, order: &TakerOrder) -> MyOrdersResult<()> {
            insert_taker_order(&self.ctx, order.request.uuid, order)
                .map_to_mm(|e| MyOrdersError::ErrorSaving(e.to_string()))
        }

        async fn update_maker_order_in_filtering_history(&self, order: &MakerOrder) -> MyOrdersResult<()> {
            update_maker_order(&self.ctx, order.uuid, order).map_to_mm(|e| MyOrdersError::ErrorSaving(e.to_string()))
        }

        async fn update_order_status_in_filtering_history(&self, uuid: Uuid, status: String) -> MyOrdersResult<()> {
            update_order_status(&self.ctx, uuid, status).map_to_mm(|e| MyOrdersError::ErrorSaving(e.to_string()))
        }

        async fn update_was_taker_in_filtering_history(&self, uuid: Uuid) -> MyOrdersResult<()> {
            update_was_taker(&self.ctx, uuid).map_to_mm(|e| MyOrdersError::ErrorSaving(e.to_string()))
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_impl {
    use super::*;
    use crate::lp_ordermatch::ordermatch_wasm_db::{
        DbTransactionError, InitDbError, MyActiveMakerOrdersTable, MyActiveTakerOrdersTable,
        MyFilteringHistoryOrdersTable, MyHistoryOrdersTable,
    };
    use crate::lp_ordermatch::OrdermatchContext;
    use common::log::warn;
    use mm2_rpc::data::legacy::TakerAction;
    use num_traits::ToPrimitive;
    use std::sync::Arc;

    impl From<InitDbError> for MyOrdersError {
        fn from(e: InitDbError) -> Self {
            MyOrdersError::InternalError(e.to_string())
        }
    }

    impl From<DbTransactionError> for MyOrdersError {
        fn from(e: DbTransactionError) -> Self {
            let stringified_error = e.to_string();
            match e {
                DbTransactionError::NoSuchTable { .. }
                | DbTransactionError::ErrorCreatingTransaction(_)
                | DbTransactionError::ErrorOpeningTable { .. }
                // We don't expect that the `Uuid` type serialization to fail.
                | DbTransactionError::ErrorSerializingIndex { .. }
                | DbTransactionError::MultipleItemsByUniqueIndex { .. }
                | DbTransactionError::NoSuchIndex { .. }
                | DbTransactionError::InvalidIndex { .. }
                | DbTransactionError::UnexpectedState(_)
                | DbTransactionError::TransactionAborted => MyOrdersError::InternalError(stringified_error),
                DbTransactionError::ErrorSerializingItem(_) => MyOrdersError::ErrorSerializing(stringified_error),
                DbTransactionError::ErrorDeserializingItem(_) => MyOrdersError::ErrorDeserializing(stringified_error),
                DbTransactionError::ErrorUploadingItem(_)
                | DbTransactionError::ErrorDeletingItems(_) => MyOrdersError::ErrorSaving(stringified_error),
                DbTransactionError::ErrorGettingItems(_)
                | DbTransactionError::ErrorCountingItems(_) => MyOrdersError::ErrorLoading(stringified_error),
            }
        }
    }

    #[derive(Clone)]
    pub struct MyOrdersStorage {
        ctx: Arc<OrdermatchContext>,
    }

    impl MyOrdersStorage {
        pub fn new(ctx: MmArc) -> MyOrdersStorage {
            MyOrdersStorage {
                ctx: OrdermatchContext::from_ctx(&ctx).expect("!OrdermatchContext::from_ctx"),
            }
        }
    }

    #[async_trait]
    impl MyActiveOrders for MyOrdersStorage {
        async fn load_active_maker_orders(&self) -> MyOrdersResult<Vec<MakerOrder>> {
            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<MyActiveMakerOrdersTable>().await.map_mm_err()?;
            let maker_orders = table.get_all_items().await.map_mm_err()?;
            Ok(maker_orders
                .into_iter()
                .map(|(_item_id, MyActiveMakerOrdersTable { order_payload, .. })| order_payload)
                .collect())
        }

        async fn load_active_maker_order(&self, uuid: Uuid) -> MyOrdersResult<MakerOrder> {
            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<MyActiveMakerOrdersTable>().await.map_mm_err()?;

            table
                .get_item_by_unique_index("uuid", uuid)
                .await
                .map_mm_err()?
                .map(|(_item_id, MyActiveMakerOrdersTable { order_payload, .. })| order_payload)
                .or_mm_err(|| MyOrdersError::NoSuchOrder { uuid })
        }

        async fn load_active_taker_orders(&self) -> MyOrdersResult<Vec<TakerOrder>> {
            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<MyActiveTakerOrdersTable>().await.map_mm_err()?;
            let maker_orders = table.get_all_items().await.map_mm_err()?;
            Ok(maker_orders
                .into_iter()
                .map(|(_item_id, MyActiveTakerOrdersTable { order_payload, .. })| order_payload)
                .collect())
        }

        async fn save_new_active_maker_order(&self, order: &MakerOrder) -> MyOrdersResult<()> {
            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<MyActiveMakerOrdersTable>().await.map_mm_err()?;

            let item = MyActiveMakerOrdersTable {
                uuid: order.uuid,
                order_payload: order.clone(),
            };
            table.add_item(&item).await.map_mm_err()?;
            Ok(())
        }

        async fn save_new_active_taker_order(&self, order: &TakerOrder) -> MyOrdersResult<()> {
            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<MyActiveTakerOrdersTable>().await.map_mm_err()?;

            let item = MyActiveTakerOrdersTable {
                uuid: order.request.uuid,
                order_payload: order.clone(),
            };
            table.add_item(&item).await.map_mm_err()?;
            Ok(())
        }

        async fn delete_active_maker_order(&self, uuid: Uuid) -> MyOrdersResult<()> {
            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<MyActiveMakerOrdersTable>().await.map_mm_err()?;
            table.delete_item_by_unique_index("uuid", uuid).await.map_mm_err()?;
            Ok(())
        }

        async fn delete_active_taker_order(&self, uuid: Uuid) -> MyOrdersResult<()> {
            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<MyActiveTakerOrdersTable>().await.map_mm_err()?;
            table.delete_item_by_unique_index("uuid", uuid).await.map_mm_err()?;
            Ok(())
        }

        async fn update_active_maker_order(&self, order: &MakerOrder) -> MyOrdersResult<()> {
            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<MyActiveMakerOrdersTable>().await.map_mm_err()?;

            let item = MyActiveMakerOrdersTable {
                uuid: order.uuid,
                order_payload: order.clone(),
            };
            table
                .replace_item_by_unique_index("uuid", order.uuid, &item)
                .await
                .map_mm_err()?;
            Ok(())
        }

        async fn update_active_taker_order(&self, order: &TakerOrder) -> MyOrdersResult<()> {
            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<MyActiveTakerOrdersTable>().await.map_mm_err()?;

            let item = MyActiveTakerOrdersTable {
                uuid: order.request.uuid,
                order_payload: order.clone(),
            };
            table
                .replace_item_by_unique_index("uuid", order.request.uuid, &item)
                .await
                .map_mm_err()?;
            Ok(())
        }
    }

    #[async_trait]
    impl MyOrdersHistory for MyOrdersStorage {
        async fn save_order_in_history(&self, order: &Order) -> MyOrdersResult<()> {
            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<MyHistoryOrdersTable>().await.map_mm_err()?;

            let item = MyHistoryOrdersTable {
                uuid: order.uuid(),
                order_payload: order.clone(),
            };
            table.add_item(&item).await.map_mm_err()?;
            Ok(())
        }

        async fn load_order_from_history(&self, uuid: Uuid) -> MyOrdersResult<Order> {
            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<MyHistoryOrdersTable>().await.map_mm_err()?;

            table
                .get_item_by_unique_index("uuid", uuid)
                .await
                .map_mm_err()?
                .map(|(_item_id, MyHistoryOrdersTable { order_payload, .. })| order_payload)
                .or_mm_err(|| MyOrdersError::NoSuchOrder { uuid })
        }
    }

    #[async_trait]
    impl MyOrdersFilteringHistory for MyOrdersStorage {
        async fn select_orders_by_filter(
            &self,
            _filter: &MyOrdersFilter,
            _paging_options: Option<&PagingOptions>,
        ) -> MyOrdersResult<RecentOrdersSelectResult> {
            warn!("'select_orders_by_filter' not supported in WASM yet");
            MmError::err(MyOrdersError::InternalError(
                "'select_orders_by_filter' not supported in WASM".to_owned(),
            ))
        }

        async fn select_order_status(&self, uuid: Uuid) -> MyOrdersResult<String> {
            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction
                .table::<MyFilteringHistoryOrdersTable>()
                .await
                .map_mm_err()?;

            table
                .get_item_by_unique_index("uuid", uuid)
                .await
                .map_mm_err()?
                .map(|(_item_id, MyFilteringHistoryOrdersTable { status, .. })| status)
                .or_mm_err(|| MyOrdersError::NoSuchOrder { uuid })
        }

        async fn save_maker_order_in_filtering_history(&self, order: &MakerOrder) -> MyOrdersResult<()> {
            let item = maker_order_to_filtering_history_item(order, "Created".to_owned(), false)?;

            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction
                .table::<MyFilteringHistoryOrdersTable>()
                .await
                .map_mm_err()?;
            table.add_item(&item).await.map_mm_err()?;
            Ok(())
        }

        async fn save_taker_order_in_filtering_history(&self, order: &TakerOrder) -> MyOrdersResult<()> {
            let item = taker_order_to_filtering_history_item(order, "Created".to_owned())?;

            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction
                .table::<MyFilteringHistoryOrdersTable>()
                .await
                .map_mm_err()?;
            table.add_item(&item).await.map_mm_err()?;
            Ok(())
        }

        async fn update_maker_order_in_filtering_history(&self, order: &MakerOrder) -> MyOrdersResult<()> {
            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction
                .table::<MyFilteringHistoryOrdersTable>()
                .await
                .map_mm_err()?;
            // get the previous item to see if the order was taker
            let (item_id, prev_item) = table
                .get_item_by_unique_index("uuid", order.uuid)
                .await
                .map_mm_err()?
                .or_mm_err(|| MyOrdersError::NoSuchOrder { uuid: order.uuid })?;

            let item = maker_order_to_filtering_history_item(order, "Updated".to_owned(), prev_item.was_taker)?;
            table.replace_item(item_id, &item).await.map_mm_err()?;
            Ok(())
        }

        async fn update_order_status_in_filtering_history(&self, uuid: Uuid, status: String) -> MyOrdersResult<()> {
            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction
                .table::<MyFilteringHistoryOrdersTable>()
                .await
                .map_mm_err()?;

            let (item_id, mut item) = table
                .get_item_by_unique_index("uuid", uuid)
                .await
                .map_mm_err()?
                .or_mm_err(|| MyOrdersError::NoSuchOrder { uuid })?;

            item.status = status;
            table.replace_item(item_id, &item).await.map_mm_err()?;
            Ok(())
        }

        async fn update_was_taker_in_filtering_history(&self, uuid: Uuid) -> MyOrdersResult<()> {
            let db = self.ctx.ordermatch_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction
                .table::<MyFilteringHistoryOrdersTable>()
                .await
                .map_mm_err()?;

            let (item_id, mut item) = table
                .get_item_by_unique_index("uuid", uuid)
                .await
                .map_mm_err()?
                .or_mm_err(|| MyOrdersError::NoSuchOrder { uuid })?;

            item.was_taker = true;
            table.replace_item(item_id, &item).await.map_mm_err()?;
            Ok(())
        }
    }

    pub(super) fn maker_order_to_filtering_history_item(
        order: &MakerOrder,
        status: String,
        was_taker: bool,
    ) -> MyOrdersResult<MyFilteringHistoryOrdersTable> {
        let price_dec = order.price.to_decimal();
        let price = price_dec.to_f64().or_mm_err(|| {
            let error = format!("Couldn't convert the order price '{price_dec}' to f64");
            MyOrdersError::ErrorSerializing(error)
        })?;

        let volume_dec = order.max_base_vol.to_decimal();
        let volume = volume_dec.to_f64().or_mm_err(|| {
            let error = format!("Couldn't convert the order volume '{volume_dec}' to f64");
            MyOrdersError::ErrorSerializing(error)
        })?;

        Ok(MyFilteringHistoryOrdersTable {
            uuid: order.uuid,
            order_type: "Maker".to_owned(),
            initial_action: "Sell".to_owned(),
            base: order.base.clone(),
            rel: order.rel.clone(),
            price,
            volume,
            created_at: order.created_at as u32,
            last_updated: order.updated_at.unwrap_or(0) as u32,
            was_taker,
            status,
        })
    }

    pub(super) fn taker_order_to_filtering_history_item(
        order: &TakerOrder,
        status: String,
    ) -> MyOrdersResult<MyFilteringHistoryOrdersTable> {
        let price_dec = order.request.rel_amount.to_decimal() / order.request.base_amount.to_decimal();
        let price = price_dec.to_f64().or_mm_err(|| {
            let error = format!("Couldn't convert the order price '{price_dec}' to f64");
            MyOrdersError::ErrorSerializing(error)
        })?;

        let volume_dec = order.request.base_amount.to_decimal();
        let volume = volume_dec.to_f64().or_mm_err(|| {
            let error = format!("Couldn't convert the order volume '{volume_dec}' to f64");
            MyOrdersError::ErrorSerializing(error)
        })?;

        Ok(MyFilteringHistoryOrdersTable {
            uuid: order.request.uuid,
            order_type: "Taker".to_owned(),
            initial_action: format!("{:?}", order.request.action),
            base: order.request.base.clone(),
            rel: order.request.rel.clone(),
            price,
            volume,
            created_at: order.created_at as u32,
            last_updated: order.created_at as u32,
            was_taker: false,
            status,
        })
    }
}

#[cfg(target_arch = "wasm32")]
mod tests {
    use super::wasm_impl::{maker_order_to_filtering_history_item, taker_order_to_filtering_history_item};
    use super::*;
    use crate::lp_ordermatch::ordermatch_wasm_db::{ItemId, MyFilteringHistoryOrdersTable};
    use crate::lp_ordermatch::{OrdermatchContext, TakerRequest};
    use crate::swap_versioning::SwapVersion;
    use common::{new_uuid, now_ms};
    use futures::compat::Future01CompatExt;
    use itertools::Itertools;
    use mm2_core::mm_ctx::MmCtxBuilder;
    use mm2_db::indexed_db::TableSignature;
    use mm2_rpc::data::legacy::{MatchBy, OrderType, TakerAction};
    use std::collections::HashMap;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    fn maker_order_for_test() -> MakerOrder {
        MakerOrder {
            base: "BASE".to_owned(),
            rel: "REL".to_owned(),
            created_at: now_ms(),
            updated_at: Some(now_ms()),
            max_base_vol: 10.into(),
            min_base_vol: 0.into(),
            price: 1.into(),
            matches: HashMap::new(),
            started_swaps: Vec::new(),
            uuid: new_uuid(),
            conf_settings: None,
            changes_history: None,
            save_in_history: true,
            base_orderbook_ticker: None,
            rel_orderbook_ticker: None,
            p2p_privkey: None,
            swap_version: SwapVersion::default(),
            #[cfg(feature = "ibc-routing-for-swaps")]
            order_metadata: crate::lp_ordermatch::OrderMetadata::default(),
            timeout_in_minutes: None,
        }
    }

    fn taker_order_for_test() -> TakerOrder {
        TakerOrder {
            request: TakerRequest {
                base: "BASE".to_owned(),
                rel: "REL".to_owned(),
                uuid: new_uuid(),
                dest_pub_key: Default::default(),
                sender_pubkey: Default::default(),
                base_amount: 10.into(),
                rel_amount: 20.into(),
                action: TakerAction::Buy,
                match_by: MatchBy::Any,
                conf_settings: None,
                base_protocol_info: None,
                rel_protocol_info: None,
                swap_version: SwapVersion::default(),
            },
            matches: HashMap::new(),
            created_at: now_ms(),
            order_type: OrderType::GoodTillCancelled,
            min_volume: 0.into(),
            timeout: 30,
            save_in_history: true,
            base_orderbook_ticker: None,
            rel_orderbook_ticker: None,
            p2p_privkey: None,
            #[cfg(feature = "ibc-routing-for-swaps")]
            order_metadata: crate::lp_ordermatch::OrderMetadata::default(),
        }
    }

    async fn get_all_items<Table: TableSignature>(ctx: &MmArc) -> Vec<Table> {
        let ordermatch_ctx = OrdermatchContext::from_ctx(ctx).unwrap();
        let db = ordermatch_ctx.ordermatch_db().await.unwrap();
        let transaction = db.transaction().await.unwrap();
        let table = transaction.table::<Table>().await.unwrap();
        table
            .get_all_items()
            .await
            .expect("Error getting items")
            .into_iter()
            .map(|(_item_id, item)| item)
            .collect()
    }

    #[wasm_bindgen_test]
    async fn test_delete_my_maker_order() {
        let ctx = MmCtxBuilder::new().with_test_db_namespace().into_mm_arc();
        let storage = MyOrdersStorage::new(ctx.clone());

        let maker1 = maker_order_for_test();

        // Save the `maker1` order and remove it with the `InsufficientBalance` reason.
        // The order has to be saved in history table.

        save_my_new_maker_order(ctx.clone(), &maker1).await.unwrap();
        delete_my_maker_order(
            ctx.clone(),
            maker1.clone(),
            MakerOrderCancellationReason::InsufficientBalance,
        )
        .compat()
        .await
        .unwrap();

        let actual_active_maker_orders = storage
            .load_active_taker_orders()
            .await
            .expect("!MyOrdersStorage::load_active_taker_orders");
        assert!(actual_active_maker_orders.is_empty());
        let actual_history_order = storage
            .load_order_from_history(maker1.uuid)
            .await
            .expect("!MyOrdersStorage::load_order_from_history");
        assert_eq!(actual_history_order, Order::Maker(maker1.clone()));
        let actual_filtering_history_items = get_all_items::<MyFilteringHistoryOrdersTable>(&ctx).await;
        let expected_filtering_history_items =
            vec![maker_order_to_filtering_history_item(&maker1, "InsufficientBalance".to_owned(), false).unwrap()];
        assert_eq!(actual_filtering_history_items, expected_filtering_history_items);
    }

    #[wasm_bindgen_test]
    async fn test_delete_my_taker_order() {
        let ctx = MmCtxBuilder::new().with_test_db_namespace().into_mm_arc();
        let storage = MyOrdersStorage::new(ctx.clone());

        let taker1 = taker_order_for_test();
        let taker2 = TakerOrder {
            created_at: 10000,
            timeout: 10,
            ..taker_order_for_test()
        };

        // Save the `taker1` order and remove it with the `TimedOut` reason.
        // The order has to be saved in history table.

        save_my_new_taker_order(ctx.clone(), &taker1).await.unwrap();
        delete_my_taker_order(ctx.clone(), taker1.clone(), TakerOrderCancellationReason::TimedOut)
            .compat()
            .await
            .unwrap();

        let actual_active_taker_orders = storage
            .load_active_taker_orders()
            .await
            .expect("!MyOrdersStorage::load_active_taker_orders");
        assert!(actual_active_taker_orders.is_empty());
        let actual_history_order = storage
            .load_order_from_history(taker1.request.uuid)
            .await
            .expect("!MyOrdersStorage::load_order_from_history");
        assert_eq!(actual_history_order, Order::Taker(taker1.clone()));
        let actual_filtering_history_items = get_all_items::<MyFilteringHistoryOrdersTable>(&ctx).await;
        let expected_filtering_history_items =
            vec![taker_order_to_filtering_history_item(&taker1, "TimedOut".to_owned()).unwrap()];
        assert_eq!(actual_filtering_history_items, expected_filtering_history_items);

        // Save the `taker2` order and remove it with the `ToMaker` reason.
        // The order hasn't to be saved in history as it's assumed to be active as a `MakerOrder`.

        save_my_new_taker_order(ctx.clone(), &taker2).await.unwrap();
        delete_my_taker_order(ctx.clone(), taker2.clone(), TakerOrderCancellationReason::ToMaker)
            .compat()
            .await
            .unwrap();

        let actual_active_taker_orders = storage
            .load_active_taker_orders()
            .await
            .expect("!MyOrdersStorage::load_active_taker_orders");
        assert!(actual_active_taker_orders.is_empty());
        let error = storage.load_order_from_history(taker2.request.uuid).await.expect_err(
            "!MyOrdersStorage::load_order_from_history should have failed with the 'MyOrdersError::NoSuchOrder' error",
        );
        assert_eq!(
            error.into_inner(),
            MyOrdersError::NoSuchOrder {
                uuid: taker2.request.uuid
            }
        );
    }

    #[wasm_bindgen_test]
    async fn test_load_active_maker_taker_orders() {
        let ctx = MmCtxBuilder::new().with_test_db_namespace().into_mm_arc();
        let storage = MyOrdersStorage::new(ctx.clone());

        let maker1 = maker_order_for_test();
        let mut maker2 = MakerOrder {
            base: "RICK".to_owned(),
            rel: "MORTY".to_owned(),
            ..maker_order_for_test()
        };
        let taker1 = taker_order_for_test();

        // Save the `taker1` order and remove it with the `TimedOut` reason.
        // The order has to be saved in history table.

        save_my_new_taker_order(ctx.clone(), &taker1).await.unwrap();
        save_my_new_maker_order(ctx.clone(), &maker1).await.unwrap();
        save_my_new_maker_order(ctx.clone(), &maker2).await.unwrap();

        maker2.rel = "KMD".to_owned();
        storage
            .update_active_maker_order(&maker2)
            .await
            .expect("!MyOrdersStorage::update_active_maker_order");

        let actual_maker_orders: Vec<_> = storage
            .load_active_maker_orders()
            .await
            .expect("!MyOrdersStorage::load_active_maker_orders")
            .into_iter()
            .sorted_by(|x, y| x.uuid.cmp(&y.uuid))
            .collect();
        let expected_maker_orders: Vec<_> = vec![maker1, maker2]
            .into_iter()
            .sorted_by(|x, y| x.uuid.cmp(&y.uuid))
            .collect();
        assert_eq!(actual_maker_orders, expected_maker_orders);

        let actual_taker_orders: Vec<_> = storage
            .load_active_taker_orders()
            .await
            .expect("!MyOrdersStorage::load_active_taker_orders");
        let expected_taker_orders = vec![taker1];
        assert_eq!(actual_taker_orders, expected_taker_orders);
    }

    #[wasm_bindgen_test]
    async fn test_filtering_history() {
        let ctx = MmCtxBuilder::new().with_test_db_namespace().into_mm_arc();
        let storage = MyOrdersStorage::new(ctx.clone());

        let maker1 = maker_order_for_test();
        let mut maker2 = MakerOrder {
            base: "RICK".to_owned(),
            rel: "MORTY".to_owned(),
            ..maker_order_for_test()
        };
        let taker1 = taker_order_for_test();

        storage
            .save_maker_order_in_filtering_history(&maker1)
            .await
            .expect("!MyOrdersStorage::save_maker_order_in_filtering_history");
        storage
            .save_maker_order_in_filtering_history(&maker2)
            .await
            .expect("!MyOrdersStorage::save_maker_order_in_filtering_history");
        storage
            .save_taker_order_in_filtering_history(&taker1)
            .await
            .expect("!MyOrdersStorage::save_taker_order_in_filtering_history");

        storage
            .update_order_status_in_filtering_history(taker1.request.uuid, "MyCustomStatus".to_owned())
            .await
            .expect("!MyOrdersStorage::update_order_status_in_filtering_history");

        maker2.base = "KMD".to_owned();
        storage
            .update_maker_order_in_filtering_history(&maker2)
            .await
            .expect("MyOrdersStorage::update_maker_order_in_filtering_history");

        storage
            .update_was_taker_in_filtering_history(maker1.uuid)
            .await
            .expect("MyOrdersStorage::update_was_taker_in_filtering_history");

        let actual_items: Vec<_> = get_all_items::<MyFilteringHistoryOrdersTable>(&ctx)
            .await
            .into_iter()
            .sorted_by(|x, y| x.uuid.cmp(&y.uuid))
            .collect();

        let expected_items: Vec<_> = vec![
            maker_order_to_filtering_history_item(&maker1, "Created".to_owned(), true).unwrap(),
            maker_order_to_filtering_history_item(&maker2, "Updated".to_owned(), false).unwrap(),
            taker_order_to_filtering_history_item(&taker1, "MyCustomStatus".to_owned()).unwrap(),
        ]
        .into_iter()
        .sorted_by(|x, y| x.uuid.cmp(&y.uuid))
        .collect();

        assert_eq!(actual_items, expected_items);

        let taker1_status = storage
            .select_order_status(taker1.request.uuid)
            .await
            .expect("!MyOrdersStorage::select_order_status");
        assert_eq!(taker1_status, "MyCustomStatus");

        let unknown_uuid = new_uuid();
        let err = storage
            .select_order_status(unknown_uuid)
            .await
            .expect_err("!MyOrdersStorage::select_order_status should have failed");
        assert_eq!(err.into_inner(), MyOrdersError::NoSuchOrder { uuid: unknown_uuid });
    }
}
