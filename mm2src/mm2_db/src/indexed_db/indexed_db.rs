//! The representation of [Indexed DB](https://developer.mozilla.org/en-US/docs/Web/API/IndexedDB_API)
//! based on low-level interface described in `db_driver.rs`.
//!
//! # Implementation
//!
//! Since the wrappers represented in `db_driver.rs` are not `Send`,
//! the implementation below initializes and spawns a `IdbDatabaseImpl` database instance locally
//! and communicate with it through the `mpsc` channel.

use async_trait::async_trait;
use common::executor::spawn_local;
use common::log::debug;
use common::stringify_js_error;
use derive_more::Display;
use futures::channel::{mpsc, oneshot};
use futures::StreamExt;
use mm2_core::DbNamespaceId;
use mm2_err_handle::prelude::*;
use primitives::hash::H160;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{self as json, Value as Json};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Mutex;
use wasm_bindgen::JsCast;
use web_sys::{Window, WorkerGlobalScope};

macro_rules! try_serialize_index_value {
    ($exp:expr, $index:expr) => {{
        match $exp {
            Ok(res) => res,
            Err(ser_err) => {
                return MmError::err(DbTransactionError::ErrorSerializingIndex {
                    index: $index.to_owned(),
                    description: ser_err.to_string(),
                });
            },
        }
    }};
}

mod be_big_uint;
mod db_driver;
mod db_lock;
mod indexed_cursor;

pub use be_big_uint::BeBigUint;
pub use db_driver::{
    DbTransactionError, DbTransactionResult, DbUpgrader, InitDbError, InitDbResult, ItemId, OnUpgradeError,
    OnUpgradeResult,
};
pub use db_lock::{ConstructibleDb, DbLocked, SharedDb, WeakDb};

use db_driver::{IdbDatabaseBuilder, IdbDatabaseImpl, IdbObjectStoreImpl, IdbTransactionImpl, OnUpgradeNeededCb};
use indexed_cursor::{
    cursor_event_loop, CursorBuilder, CursorDriver, CursorError, CursorFilters, CursorFiltersExt, CursorResult,
    DbCursorEventTx,
};

type DbEventTx = mpsc::UnboundedSender<internal::DbEvent>;
type DbTransactionEventTx = mpsc::UnboundedSender<internal::DbTransactionEvent>;
type DbTableEventTx = mpsc::UnboundedSender<internal::DbTableEvent>;

pub mod cursor_prelude {
    pub use crate::indexed_db::indexed_cursor::{CursorError, CursorResult};
}

pub trait TableSignature: DeserializeOwned + Serialize + 'static {
    const TABLE_NAME: &'static str;

    fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()>;
}

/// Essential operations for initializing an IndexedDb instance.
#[async_trait]
pub trait DbInstance: Sized {
    /// Returns the static name of the database.
    const DB_NAME: &'static str;

    /// Initialize the database with the provided identifier.
    /// This method ensures that the database is properly set up with the correct version
    /// and has the required tables.
    async fn init(db_id: DbIdentifier) -> InitDbResult<Self>;
}

#[derive(Clone, Display)]
#[display(fmt = "{}::{}::{}", namespace_id, "self.display_rmd160()", db_name)]
pub struct DbIdentifier {
    namespace_id: DbNamespaceId,
    /// The `RIPEMD160(SHA256(x))` where x is secp256k1 pubkey derived from passphrase.
    /// This value is used to distinguish different databases corresponding to user's different seed phrases.
    wallet_rmd160: Option<H160>,
    db_name: &'static str,
}

impl DbIdentifier {
    pub fn db_name(&self) -> &'static str {
        self.db_name
    }

    pub fn new<Db: DbInstance>(namespace_id: DbNamespaceId, wallet_rmd160: Option<H160>) -> DbIdentifier {
        DbIdentifier {
            namespace_id,
            wallet_rmd160,
            db_name: Db::DB_NAME,
        }
    }

    pub fn for_test(db_name: &'static str) -> DbIdentifier {
        DbIdentifier {
            namespace_id: DbNamespaceId::for_test(),
            wallet_rmd160: Some(H160::default()),
            db_name,
        }
    }

    pub fn display_rmd160(&self) -> String {
        self.wallet_rmd160
            .map(hex::encode)
            .unwrap_or_else(|| "KOMODEFI".to_string())
    }
}

pub struct IndexedDbBuilder {
    pub db_name: String,
    pub db_version: u32,
    pub tables: HashMap<String, OnUpgradeNeededCb>,
}

impl IndexedDbBuilder {
    pub fn new(db_id: DbIdentifier) -> IndexedDbBuilder {
        IndexedDbBuilder {
            db_name: db_id.to_string(),
            db_version: 1,
            tables: HashMap::new(),
        }
    }

    pub fn with_version(mut self, db_version: u32) -> IndexedDbBuilder {
        self.db_version = db_version;
        self
    }

    pub fn with_table<Table: TableSignature>(mut self) -> IndexedDbBuilder {
        let on_upgrade_needed_cb = Box::new(Table::on_upgrade_needed);
        self.tables.insert(Table::TABLE_NAME.to_owned(), on_upgrade_needed_cb);
        self
    }

    pub async fn build(self) -> InitDbResult<IndexedDb> {
        let (init_tx, init_rx) = oneshot::channel();
        let (event_tx, event_rx) = mpsc::unbounded();

        self.init_and_spawn(init_tx, event_rx);
        init_rx.await.expect("The init channel must not be closed")?;
        Ok(IndexedDb { event_tx })
    }

    fn init_and_spawn(
        self,
        init_tx: oneshot::Sender<InitDbResult<()>>,
        event_rx: mpsc::UnboundedReceiver<internal::DbEvent>,
    ) {
        let fut = async move {
            let db = match IdbDatabaseBuilder::new(&self.db_name)
                .with_version(self.db_version)
                .with_tables(self.tables.into_iter())
                .build()
                .await
            {
                Ok(db) => db,
                Err(e) => {
                    // ignore if the receiver is closed
                    let _res = init_tx.send(Err(e));
                    return;
                },
            };

            // ignore if the receiver is closed
            let _res = init_tx.send(Ok(()));
            // run the event loop
            IndexedDb::event_loop(event_rx, db).await;
        };

        // `IndexedDb::event_loop` will finish almost immediately once the opposite `event_tx` is dropped.
        spawn_local(fut);
    }
}

pub struct IndexedDb {
    event_tx: DbEventTx,
}

async fn send_event_recv_response<Event, Item, Error>(
    event_tx: &mpsc::UnboundedSender<Event>,
    event: Event,
    result_rx: oneshot::Receiver<MmResult<Item, Error>>,
) -> MmResult<Item, Error>
where
    Error: WithInternal + NotMmError,
{
    if let Err(e) = event_tx.unbounded_send(event) {
        return MmError::err(Error::internal(format!("Error sending event: {e}")));
    }
    match result_rx.await {
        Ok(result) => result,
        Err(e) => MmError::err(Error::internal(format!("Error receiving result: {e}"))),
    }
}

impl IndexedDb {
    pub async fn transaction(&self) -> DbTransactionResult<DbTransaction<'_>> {
        let (result_tx, result_rx) = oneshot::channel();
        let event = internal::DbEvent::CreateTransaction { result_tx };
        let transaction_event_tx = send_event_recv_response(&self.event_tx, event, result_rx).await?;
        Ok(DbTransaction {
            event_tx: transaction_event_tx,
            phantom: PhantomData,
        })
    }

    async fn event_loop(mut rx: mpsc::UnboundedReceiver<internal::DbEvent>, db: IdbDatabaseImpl) {
        while let Some(event) = rx.next().await {
            match event {
                internal::DbEvent::CreateTransaction { result_tx } => Self::create_transaction(&db, result_tx),
            }
        }
    }

    fn create_transaction(db: &IdbDatabaseImpl, result_tx: oneshot::Sender<DbTransactionResult<DbTransactionEventTx>>) {
        let transaction = match db.transaction() {
            Ok(transaction) => transaction,
            Err(e) => {
                // ignore if the receiver is closed
                result_tx.send(Err(e)).ok();
                return;
            },
        };
        let (transaction_event_tx, transaction_event_rx) = mpsc::unbounded();

        // Spawn the event loop.
        let fut = async move { DbTransaction::event_loop(transaction_event_rx, transaction).await };
        // `DbTransaction::event_loop` will finish almost immediately once `transaction_event_rx` is dropped.
        spawn_local(fut);

        // ignore if the receiver is closed
        result_tx.send(Ok(transaction_event_tx)).ok();
    }
}

pub struct DbTransaction<'transaction> {
    event_tx: DbTransactionEventTx,
    phantom: PhantomData<&'transaction ()>,
}

impl DbTransaction<'_> {
    pub async fn table<Table: TableSignature>(&self) -> DbTransactionResult<DbTable<'_, Table>> {
        let (result_tx, result_rx) = oneshot::channel();
        let event = internal::DbTransactionEvent::OpenTable {
            table_name: Table::TABLE_NAME.to_owned(),
            result_tx,
        };
        let transaction_event_tx = send_event_recv_response(&self.event_tx, event, result_rx).await?;
        Ok(DbTable {
            event_tx: transaction_event_tx,
            phantom: PhantomData,
        })
    }

    pub async fn aborted(&self) -> DbTransactionResult<bool> {
        let (result_tx, result_rx) = oneshot::channel();
        let event = internal::DbTransactionEvent::IsAborted { result_tx };
        send_event_recv_response(&self.event_tx, event, result_rx).await
    }

    async fn event_loop(
        mut rx: mpsc::UnboundedReceiver<internal::DbTransactionEvent>,
        transaction: IdbTransactionImpl,
    ) {
        while let Some(event) = rx.next().await {
            match event {
                internal::DbTransactionEvent::OpenTable { table_name, result_tx } => {
                    Self::open_table(&transaction, table_name, result_tx)
                },
                internal::DbTransactionEvent::IsAborted { result_tx } => {
                    result_tx.send(Ok(transaction.aborted())).ok();
                },
            }
        }
    }

    fn open_table(
        transaction: &IdbTransactionImpl,
        table_name: String,
        result_tx: oneshot::Sender<DbTransactionResult<mpsc::UnboundedSender<internal::DbTableEvent>>>,
    ) {
        let table = match transaction.open_table(&table_name) {
            Ok(table) => table,
            Err(e) => {
                // ignore if the receiver is closed
                result_tx.send(Err(e)).ok();
                return;
            },
        };
        let (table_event_tx, table_event_rx) = mpsc::unbounded();

        let fut = async move { table_event_loop(table_event_rx, table).await };
        // `table_event_loop` will finish almost immediately once `table_event_tx` is dropped.
        spawn_local(fut);

        // ignore if the receiver is closed
        result_tx.send(Ok(table_event_tx)).ok();
    }
}

pub struct DbTable<'transaction, Table: TableSignature> {
    event_tx: DbTableEventTx,
    phantom: PhantomData<&'transaction Table>,
}

pub enum AddOrIgnoreResult {
    Added(ItemId),
    ExistAlready(ItemId),
}

impl AddOrIgnoreResult {
    pub fn get_id(&self) -> ItemId {
        match self {
            AddOrIgnoreResult::Added(id) => *id,
            AddOrIgnoreResult::ExistAlready(id) => *id,
        }
    }
}

impl<'transaction, Table: TableSignature> DbTable<'transaction, Table> {
    /// Adds the given item to the table.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/add
    pub async fn add_item(&self, item: &Table) -> DbTransactionResult<ItemId> {
        let item = json::to_value(item).map_to_mm(|e| DbTransactionError::ErrorSerializingItem(e.to_string()))?;
        let (result_tx, result_rx) = oneshot::channel();
        let event = internal::DbTableEvent::AddItem { item, result_tx };
        send_event_recv_response(&self.event_tx, event, result_rx).await
    }

    /// Adds the given `item` if there are no items with the same `index`.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/add
    ///
    /// * `index` - the name of a corresponding `Table`'s field by which records will be searched.
    /// * `index_value` - the value of the `index`, therefore the value of a corresponding `Table`'s field.
    pub async fn add_item_or_ignore_by_unique_index<Value>(
        &self,
        index: &str,
        index_value: Value,
        item: &Table,
    ) -> DbTransactionResult<AddOrIgnoreResult>
    where
        Value: Serialize,
    {
        let ids = self.get_item_ids(index, index_value).await?;
        match ids.len() {
            0 => self.add_item(item).await.map(AddOrIgnoreResult::Added),
            1 => Ok(AddOrIgnoreResult::ExistAlready(ids[0])),
            got_items => MmError::err(DbTransactionError::MultipleItemsByUniqueIndex {
                index: index.to_owned(),
                got_items,
            }),
        }
    }

    /// Adds the given `item` if there are no items with the same multiple indexes.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/add
    ///
    /// For more details on multiple indexes see [`TableUpgrader::create_multi_index`].
    pub async fn add_item_or_ignore_by_unique_multi_index(
        &self,
        multi_index: MultiIndex,
        item: &Table,
    ) -> DbTransactionResult<AddOrIgnoreResult> {
        self.add_item_or_ignore_by_unique_index(&multi_index.index, multi_index.values, item)
            .await
    }

    /// Queries items from the store matching the specified `index`.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/getAllKeys
    ///
    /// * `index` - the name of a corresponding `Table`'s field by which records will be searched.
    /// * `index_value` - the value of the `index`, therefore the value of a corresponding `Table`'s field.
    pub async fn get_items<Value>(&self, index: &str, index_value: Value) -> DbTransactionResult<Vec<(ItemId, Table)>>
    where
        Value: Serialize,
    {
        let (result_tx, result_rx) = oneshot::channel();
        let index_value = try_serialize_index_value!(json::to_value(index_value), index);
        let event = internal::DbTableEvent::GetItems {
            index: index.to_owned(),
            index_value,
            result_tx,
        };
        send_event_recv_response(&self.event_tx, event, result_rx)
            .await
            .and_then(|items| Self::deserialize_items(items))
    }

    /// Queries items from the store matching the specified multiple indexes.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/getAllKeys
    ///
    /// For more details on multiple indexes see [`TableUpgrader::create_multi_index`].
    pub async fn get_items_by_multi_index(&self, multi_index: MultiIndex) -> DbTransactionResult<Vec<(ItemId, Table)>> {
        self.get_items(&multi_index.index, multi_index.values).await
    }

    /// Queries an item from the store matching the specified **unique** `index`.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/getAllKeys
    ///
    /// * `index` - the name of a corresponding `Table`'s field by which records will be searched.
    /// * `index_value` - the value of the `index`, therefore the value of a corresponding `Table`'s field.
    ///
    pub async fn get_item_by_unique_index<Value>(
        &self,
        index: &str,
        index_value: Value,
    ) -> DbTransactionResult<Option<(ItemId, Table)>>
    where
        Value: Serialize,
    {
        let items = self.get_items(index, index_value).await?;
        if items.len() > 1 {
            return MmError::err(DbTransactionError::MultipleItemsByUniqueIndex {
                index: index.to_owned(),
                got_items: items.len(),
            });
        }
        Ok(items.into_iter().next())
    }

    /// Queries an item from the store matching the specified **unique** multiple indexes.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/getAllKeys
    ///
    /// For more details on multiple indexes see [`TableUpgrader::create_multi_index`].
    pub async fn get_item_by_unique_multi_index(
        &self,
        multi_index: MultiIndex,
    ) -> DbTransactionResult<Option<(ItemId, Table)>> {
        self.get_item_by_unique_index(&multi_index.index, multi_index.values)
            .await
    }

    /// Queries IDs of items from the store matching the specified `index`.
    /// Such IDs can be used to delete, replace items.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/getAllKeys
    ///
    /// * `index` - the name of a corresponding `Table`'s field by which records will be searched.
    /// * `index_value` - the value of the `index`, therefore the value of a corresponding `Table`'s field.
    pub async fn get_item_ids<Value>(&self, index: &str, index_value: Value) -> DbTransactionResult<Vec<ItemId>>
    where
        Value: Serialize,
    {
        let (result_tx, result_rx) = oneshot::channel();
        let index_value = try_serialize_index_value!(json::to_value(index_value), index);
        let event = internal::DbTableEvent::GetItemIds {
            index: index.to_owned(),
            index_value,
            result_tx,
        };
        send_event_recv_response(&self.event_tx, event, result_rx).await
    }

    /// Queries IDs of items from the store matching the specified multiple indexes.
    /// Such IDs can be used to delete, replace items.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/getAllKeys
    ///
    /// For more details on multiple indexes see [`TableUpgrader::create_multi_index`].
    pub async fn get_item_ids_by_multi_index(&self, multi_index: MultiIndex) -> DbTransactionResult<Vec<ItemId>> {
        self.get_item_ids(&multi_index.index, multi_index.values).await
    }

    /// Queries all items from the store.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/getAll
    pub async fn get_all_items(&self) -> DbTransactionResult<Vec<(ItemId, Table)>> {
        let (result_tx, result_rx) = oneshot::channel();
        let event = internal::DbTableEvent::GetAllItems { result_tx };
        send_event_recv_response(&self.event_tx, event, result_rx)
            .await
            .and_then(|items| Self::deserialize_items(items))
    }

    /// Returns the number of items matching the specified `index`.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/count
    ///
    /// * `index` - the name of a corresponding `Table`'s field by which records will be searched.
    /// * `index_value` - the value of the `index`, therefore the value of a corresponding `Table`'s field.
    pub async fn count<Value: Serialize>(&self, index: &str, index_value: Value) -> DbTransactionResult<usize> {
        let (result_tx, result_rx) = oneshot::channel();
        let index_value = try_serialize_index_value!(json::to_value(index_value), index);
        let event = internal::DbTableEvent::Count {
            index: index.to_owned(),
            index_value,
            result_tx,
        };
        send_event_recv_response(&self.event_tx, event, result_rx).await
    }

    /// Returns the number of items matching the specified multiple indexes.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/count
    ///
    /// For more details on multiple indexes see [`TableUpgrader::create_multi_index`].
    pub async fn count_by_multi_index(&self, multi_index: MultiIndex) -> DbTransactionResult<usize> {
        self.count(&multi_index.index, multi_index.values).await
    }

    /// Returns the number of items in the store.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/count
    pub async fn count_all(&self) -> DbTransactionResult<usize> {
        let (result_tx, result_rx) = oneshot::channel();
        let event = internal::DbTableEvent::CountAll { result_tx };
        send_event_recv_response(&self.event_tx, event, result_rx).await
    }

    /// Adds the given `item` or replace the previous one.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/put
    pub async fn replace_item(&self, item_id: ItemId, item: &Table) -> DbTransactionResult<ItemId> {
        let item = json::to_value(item).map_to_mm(|e| DbTransactionError::ErrorSerializingItem(e.to_string()))?;

        let (result_tx, result_rx) = oneshot::channel();
        let event = internal::DbTableEvent::ReplaceItem {
            item_id,
            item,
            result_tx,
        };
        send_event_recv_response(&self.event_tx, event, result_rx).await
    }

    /// Adds the given `item` or replace the previous one if such item with the specified `index` exists already.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/put
    ///
    /// * `index` - the name of a corresponding `Table`'s field by which records will be searched.
    /// * `index_value` - the value of the `index`, therefore the value of a corresponding `Table`'s field.
    pub async fn replace_item_by_unique_index<Value>(
        &self,
        index: &str,
        index_value: Value,
        item: &Table,
    ) -> DbTransactionResult<ItemId>
    where
        Value: Serialize,
    {
        let ids = self.get_item_ids(index, index_value).await?;
        match ids.len() {
            0 => self.add_item(item).await,
            1 => {
                let item_id = ids[0];
                self.replace_item(item_id, item).await
            },
            got_items => MmError::err(DbTransactionError::MultipleItemsByUniqueIndex {
                index: index.to_owned(),
                got_items,
            }),
        }
    }

    /// Adds the given `item` or replace the previous one if such item with the specified multiple indexes exists already.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/put
    ///
    /// For more details on multiple indexes see [`TableUpgrader::create_multi_index`].
    pub async fn replace_item_by_unique_multi_index(
        &self,
        multi_index: MultiIndex,
        item: &Table,
    ) -> DbTransactionResult<ItemId> {
        self.replace_item_by_unique_index(&multi_index.index, multi_index.values, item)
            .await
    }

    /// Deletes an item from the store by the specified `item_id`.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/delete
    pub async fn delete_item(&self, item_id: ItemId) -> DbTransactionResult<()> {
        let (result_tx, result_rx) = oneshot::channel();
        let event = internal::DbTableEvent::DeleteItem { item_id, result_tx };
        send_event_recv_response(&self.event_tx, event, result_rx).await
    }

    /// Tries to find an item by the **unique** `index` and removes it if it exists.
    /// Returns `Ok(None)` if there is no an item with the given `index`.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/delete
    ///
    /// * `index` - the name of a corresponding `Table`'s field by which records will be searched.
    /// * `index_value` - the value of the `index`, therefore the value of a corresponding `Table`'s field.
    pub async fn delete_item_by_unique_index<Value>(
        &self,
        index: &str,
        index_value: Value,
    ) -> DbTransactionResult<Option<ItemId>>
    where
        Value: Serialize,
    {
        let ids = self.get_item_ids(index, index_value).await?;
        match ids.len() {
            0 => Ok(None),
            1 => {
                let item_id = ids[0];
                self.delete_item(item_id).await?;
                Ok(Some(item_id))
            },
            got_items => MmError::err(DbTransactionError::MultipleItemsByUniqueIndex {
                index: index.to_owned(),
                got_items,
            }),
        }
    }

    /// Tries to find an item matching the specified **unique** multiple indexes and removes the item if it exists.
    /// Returns `Ok(None)` if there is no an item with the given keys.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/delete
    ///
    /// For more details on multiple indexes see [`TableUpgrader::create_multi_index`].
    pub async fn delete_item_by_unique_multi_index(
        &self,
        multi_index: MultiIndex,
    ) -> DbTransactionResult<Option<ItemId>> {
        self.delete_item_by_unique_index(&multi_index.index, multi_index.values)
            .await
    }

    /// Tries to find items matching the given `index` and removes them from the store.
    /// Returns IDs of removed items.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/delete
    pub async fn delete_items_by_index<Value>(
        &self,
        index: &str,
        index_value: Value,
    ) -> DbTransactionResult<Vec<ItemId>>
    where
        Value: Serialize,
    {
        let ids = self.get_item_ids(index, index_value).await?;
        for item_id in ids.iter() {
            self.delete_item(*item_id).await?;
        }
        Ok(ids)
    }

    /// Tries to find items matching the given multiple indexes and removes them from the store.
    /// Returns IDs of removed items.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/delete
    ///
    /// For more details on multiple indexes see [`TableUpgrader::create_multi_index`].
    pub async fn delete_items_by_multi_index(&self, multi_index: MultiIndex) -> DbTransactionResult<Vec<ItemId>> {
        self.delete_items_by_index(&multi_index.index, multi_index.values).await
    }

    /// Deletes all items from the store.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/clear
    pub async fn clear(&self) -> DbTransactionResult<()> {
        let (result_tx, result_rx) = oneshot::channel();
        let event = internal::DbTableEvent::Clear { result_tx };
        send_event_recv_response(&self.event_tx, event, result_rx).await
    }

    /// Returns a `CursorBuilder` builder. It can be used to open a cursor at the specified with specific key bounds.
    /// See [`CursorBuilder::open_cursor`].
    pub fn cursor_builder<'reference>(&'reference self) -> CursorBuilder<'transaction, 'reference, Table> {
        CursorBuilder::new(self)
    }

    /// Whether the transaction is aborted.
    pub async fn aborted(&self) -> DbTransactionResult<bool> {
        let (result_tx, result_rx) = oneshot::channel();
        let event = internal::DbTableEvent::IsAborted { result_tx };
        send_event_recv_response(&self.event_tx, event, result_rx).await
    }

    /// Opens a cursor by the specified `index`.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/openCursor
    async fn open_cursor(
        &self,
        index: &str,
        filters: CursorFilters,
        filters_ext: CursorFiltersExt,
    ) -> CursorResult<DbCursorEventTx> {
        let (result_tx, result_rx) = oneshot::channel();
        let event = internal::DbTableEvent::OpenCursor {
            index: index.to_owned(),
            filters,
            filters_ext,
            result_tx,
        };
        let cursor_event_tx = send_event_recv_response(&self.event_tx, event, result_rx)
            .await
            .mm_err(|e| CursorError::UnexpectedState(e.to_string()))?;
        Ok(cursor_event_tx)
    }

    fn deserialize_items(items: Vec<(ItemId, Json)>) -> DbTransactionResult<Vec<(ItemId, Table)>> {
        items
            .into_iter()
            .map(|(item_id, item)| {
                let item: Table =
                    json::from_value(item).map_to_mm(|e| DbTransactionError::ErrorDeserializingItem(e.to_string()))?;
                Ok((item_id, item))
            })
            .collect()
    }
}

/// This event loop cannot be part of the `DbTable`, because the `Table` type parameter is not known when this function is called.
async fn table_event_loop(mut rx: mpsc::UnboundedReceiver<internal::DbTableEvent>, table: IdbObjectStoreImpl) {
    while let Some(event) = rx.next().await {
        match event {
            internal::DbTableEvent::AddItem { item, result_tx } => {
                let res = table.add_item(&item).await;
                result_tx.send(res).ok();
            },
            internal::DbTableEvent::GetItems {
                index,
                index_value,
                result_tx,
            } => {
                let res = table.get_items(&index, index_value).await;
                result_tx.send(res).ok();
            },
            internal::DbTableEvent::GetItemIds {
                index,
                index_value,
                result_tx,
            } => {
                let res = table.get_item_ids(&index, index_value).await;
                result_tx.send(res).ok();
            },
            internal::DbTableEvent::GetAllItems { result_tx } => {
                let res = table.get_all_items().await;
                result_tx.send(res).ok();
            },
            internal::DbTableEvent::Count {
                index,
                index_value,
                result_tx,
            } => {
                let res = table.count(&index, index_value).await;
                result_tx.send(res).ok();
            },
            internal::DbTableEvent::CountAll { result_tx } => {
                let res = table.count_all().await;
                result_tx.send(res).ok();
            },
            internal::DbTableEvent::ReplaceItem {
                item_id,
                item,
                result_tx,
            } => {
                let res = table.replace_item(item_id, item).await;
                result_tx.send(res).ok();
            },
            internal::DbTableEvent::DeleteItem { item_id, result_tx } => {
                let res = table.delete_item(item_id).await;
                result_tx.send(res).ok();
            },
            internal::DbTableEvent::Clear { result_tx } => {
                let res = table.clear().await;
                result_tx.send(res).ok();
            },
            internal::DbTableEvent::IsAborted { result_tx } => {
                result_tx.send(Ok(table.aborted())).ok();
            },
            internal::DbTableEvent::OpenCursor {
                index,
                filters,
                filters_ext,
                result_tx,
            } => {
                open_cursor(&table, index, filters, filters_ext, result_tx);
            },
        }
    }
}

pub struct MultiIndex {
    index: String,
    values: Vec<Json>,
}

impl MultiIndex {
    pub fn new(index: &str) -> MultiIndex {
        MultiIndex {
            index: index.to_owned(),
            values: Vec::new(),
        }
    }

    pub fn push<Value: Serialize>(&mut self, value: Value) -> DbTransactionResult<&mut Self> {
        let index_value = try_serialize_index_value!(json::to_value(value), self.index);
        self.values.push(index_value);
        Ok(self)
    }

    pub fn with_value<Value: Serialize>(mut self, value: Value) -> DbTransactionResult<Self> {
        self.push(value)?;
        Ok(self)
    }
}

fn open_cursor(
    table: &IdbObjectStoreImpl,
    index: String,
    filters: CursorFilters,
    filter_ext: CursorFiltersExt,
    result_tx: oneshot::Sender<CursorResult<DbCursorEventTx>>,
) {
    let db_index = match table.open_index(&index) {
        Ok(db_index) => db_index,
        Err(tr_err) => {
            let cursor_err = tr_err.map(|tr_err| CursorError::ErrorOpeningCursor {
                description: tr_err.to_string(),
            });
            result_tx.send(Err(cursor_err)).ok();
            return;
        },
    };
    let cursor = match CursorDriver::init_cursor(db_index, filters, filter_ext) {
        Ok(cursor) => cursor,
        Err(e) => {
            result_tx.send(Err(e)).ok();
            return;
        },
    };

    let (event_tx, event_rx) = mpsc::unbounded();

    let fut = async move { cursor_event_loop(event_rx, cursor).await };
    // `cursor_event_loop` will finish almost immediately once `event_tx` is dropped.
    spawn_local(fut);

    // ignore if the receiver is closed
    result_tx.send(Ok(event_tx)).ok();
}

/// Detects the current execution environment (window or worker) and follows the appropriate way
/// of getting `web_sys::IdbFactory` instance.
pub(crate) fn get_idb_factory() -> Result<web_sys::IdbFactory, InitDbError> {
    // try getting global with type safety and explicit type conversion.
    let global = js_sys::global()
        .dyn_into::<js_sys::Object>()
        .map_err(|err| InitDbError::NotSupported(format!("{err:?}")))?;

    let idb_factory = if let Some(window) = global.dyn_ref::<Window>() {
        window.indexed_db()
    } else if let Some(worker) = global.dyn_ref::<WorkerGlobalScope>() {
        worker.indexed_db()
    } else {
        return Err(InitDbError::NotSupported("Unknown WASM environment.".to_string()));
    };

    match idb_factory {
        Ok(Some(db)) => Ok(db),
        Ok(None) => Err(InitDbError::NotSupported(
            if global.dyn_ref::<Window>().is_some() {
                "IndexedDB not supported in window context"
            } else {
                "IndexedDB not supported in worker context"
            }
            .to_string(),
        )),
        Err(e) => Err(InitDbError::NotSupported(stringify_js_error(&e))),
    }
}

/// Internal events.
mod internal {
    use super::*;

    pub(super) enum DbEvent {
        CreateTransaction {
            result_tx: oneshot::Sender<DbTransactionResult<DbTransactionEventTx>>,
        },
    }

    pub(super) enum DbTransactionEvent {
        OpenTable {
            table_name: String,
            result_tx: oneshot::Sender<DbTransactionResult<mpsc::UnboundedSender<DbTableEvent>>>,
        },
        IsAborted {
            result_tx: oneshot::Sender<DbTransactionResult<bool>>,
        },
    }

    pub(super) enum DbTableEvent {
        AddItem {
            item: Json,
            result_tx: oneshot::Sender<DbTransactionResult<ItemId>>,
        },
        GetItems {
            index: String,
            index_value: Json,
            result_tx: oneshot::Sender<DbTransactionResult<Vec<(ItemId, Json)>>>,
        },
        GetItemIds {
            index: String,
            index_value: Json,
            result_tx: oneshot::Sender<DbTransactionResult<Vec<ItemId>>>,
        },
        GetAllItems {
            result_tx: oneshot::Sender<DbTransactionResult<Vec<(ItemId, Json)>>>,
        },
        Count {
            index: String,
            index_value: Json,
            result_tx: oneshot::Sender<DbTransactionResult<usize>>,
        },
        CountAll {
            result_tx: oneshot::Sender<DbTransactionResult<usize>>,
        },
        ReplaceItem {
            item_id: ItemId,
            item: Json,
            result_tx: oneshot::Sender<DbTransactionResult<ItemId>>,
        },
        DeleteItem {
            item_id: ItemId,
            result_tx: oneshot::Sender<DbTransactionResult<()>>,
        },
        Clear {
            result_tx: oneshot::Sender<DbTransactionResult<()>>,
        },
        IsAborted {
            result_tx: oneshot::Sender<DbTransactionResult<bool>>,
        },
        OpenCursor {
            index: String,
            filters: CursorFilters,
            filters_ext: CursorFiltersExt,
            result_tx: oneshot::Sender<CursorResult<DbCursorEventTx>>,
        },
    }
}

mod tests {
    use super::*;
    use common::log::wasm_log::register_wasm_log;
    use lazy_static::lazy_static;
    use serde::{Deserialize, Serialize};
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
    #[serde(deny_unknown_fields)]
    struct TxTable {
        ticker: String,
        tx_hash: String,
        block_height: u64,
    }

    impl TableSignature for TxTable {
        const TABLE_NAME: &'static str = "tx_table";

        fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, _new_version: u32) -> OnUpgradeResult<()> {
            if old_version > 0 {
                // the table is initialized already
                return Ok(());
            }
            let table_upgrader = upgrader.create_table("tx_table")?;
            table_upgrader.create_index("ticker", false)?;
            table_upgrader.create_index("tx_hash", true)
        }
    }

    #[wasm_bindgen_test]
    async fn test_add_get_item() {
        const DB_NAME: &str = "TEST_ADD_GET_ITEM";
        const DB_VERSION: u32 = 1;

        let rick_tx_1 = TxTable {
            ticker: "RICK".to_owned(),
            tx_hash: "0a0fda88364b960000f445351fe7678317a1e0c80584de0413377ede00ba696f".to_owned(),
            block_height: 10000,
        };
        let rick_tx_2 = TxTable {
            ticker: "RICK".to_owned(),
            tx_hash: "ba881ecca15b5d4593f14f25debbcdfe25f101fd2e9cf8d0b5d92d19813d4424".to_owned(),
            block_height: 10000,
        };
        let morty_tx_1 = TxTable {
            ticker: "MORTY".to_owned(),
            tx_hash: "1fc789133239260ed16361190a026a88cab2243935f02f1ccd794f1d06a22246".to_owned(),
            block_height: 20000,
        };

        register_wasm_log();

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<TxTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction()");
        let table = transaction
            .table::<TxTable>()
            .await
            .expect("!DbTransaction::open_table");

        let rick_tx_1_id = table
            .add_item(&rick_tx_1)
            .await
            .expect("!Couldn't add a 'RICK' transaction");
        let rick_tx_2_id = table
            .add_item(&rick_tx_2)
            .await
            .expect("!Couldn't add a 'RICK' transaction with the different 'tx_hash'");
        let morty_tx_1_id = table
            .add_item(&morty_tx_1)
            .await
            .expect("!Couldn't add a 'MORTY' transaction");
        assert!(rick_tx_1_id != rick_tx_2_id && rick_tx_2_id != morty_tx_1_id);

        let actual_rick_txs = table
            .get_items("ticker", "RICK")
            .await
            .expect("!Couldn't get items by the index 'ticker=RICK'");
        let expected_rick_txs = vec![(rick_tx_1_id, rick_tx_1), (rick_tx_2_id, rick_tx_2.clone())];
        assert_eq!(actual_rick_txs, expected_rick_txs);

        let actual_rick_tx_ids = table
            .get_item_ids("ticker", "RICK")
            .await
            .expect("Couldn't get item ids by the index 'ticker=RICK'");
        let expected_rick_tx_ids = vec![rick_tx_1_id, rick_tx_2_id];
        assert_eq!(actual_rick_tx_ids, expected_rick_tx_ids);

        let actual_rick_2_tx = table
            .get_items(
                "tx_hash",
                "ba881ecca15b5d4593f14f25debbcdfe25f101fd2e9cf8d0b5d92d19813d4424",
            )
            .await
            .expect("!Couldn't get items by the index 'tx_hash'");
        let expected_rick_txs = vec![(rick_tx_2_id, rick_tx_2)];
        assert_eq!(actual_rick_2_tx, expected_rick_txs);
    }

    #[wasm_bindgen_test]
    async fn test_add_item_or_ignore() {
        const DB_NAME: &str = "TEST_ADD_ITEM_OR_IGNORE";
        const DB_VERSION: u32 = 1;
        const TX_HASH: &str = "0a0fda88364b960000f445351fe7678317a1e0c80584de0413377ede00ba696f";

        let tx_1 = TxTable {
            ticker: "RICK".to_owned(),
            tx_hash: TX_HASH.to_owned(),
            block_height: 10000,
        };
        let tx_2 = TxTable {
            ticker: "RICK".to_owned(),
            tx_hash: TX_HASH.to_owned(),
            block_height: 20000,
        };

        register_wasm_log();

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<TxTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction()");
        let table = transaction
            .table::<TxTable>()
            .await
            .expect("!DbTransaction::open_table");

        let tx_1_id = match table
            .add_item_or_ignore_by_unique_index("tx_hash", TX_HASH, &tx_1)
            .await
            .expect("!Couldn't add a 'RICK' transaction")
        {
            AddOrIgnoreResult::Added(item_id) => item_id,
            AddOrIgnoreResult::ExistAlready(unknown_tx_id) => {
                panic!("Transaction should be added: found '{unknown_tx_id}'")
            },
        };
        let found_tx_id = match table
            .add_item_or_ignore_by_unique_index("tx_hash", TX_HASH, &tx_2)
            .await
            .expect("'add_item_or_ignore_by_unique_index' failed, but should just ignore the transaction")
        {
            // Transaction shouldn't be added since we added `tx_1` with the same `tx_hash` already.
            AddOrIgnoreResult::Added(_) => panic!("'add_item_or_ignore_by_unique_index' shouldn't have added 'tx_2'"),
            AddOrIgnoreResult::ExistAlready(exist_tx_id) => exist_tx_id,
        };
        assert_eq!(tx_1_id, found_tx_id);

        let actual_txs = table.get_all_items().await.expect("Couldn't get items");
        assert_eq!(actual_txs, vec![(tx_1_id, tx_1)]);
    }

    #[wasm_bindgen_test]
    async fn test_count() {
        const DB_NAME: &str = "TEST_COUNT";
        const DB_VERSION: u32 = 1;

        let rick_tx_1 = TxTable {
            ticker: "RICK".to_owned(),
            tx_hash: "0a0fda88364b960000f445351fe7678317a1e0c80584de0413377ede00ba696f".to_owned(),
            block_height: 10000,
        };
        let rick_tx_2 = TxTable {
            ticker: "RICK".to_owned(),
            tx_hash: "ba881ecca15b5d4593f14f25debbcdfe25f101fd2e9cf8d0b5d92d19813d4424".to_owned(),
            block_height: 10000,
        };
        let morty_tx_1 = TxTable {
            ticker: "MORTY".to_owned(),
            tx_hash: "1fc789133239260ed16361190a026a88cab2243935f02f1ccd794f1d06a22246".to_owned(),
            block_height: 20000,
        };

        register_wasm_log();

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<TxTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction()");
        let table = transaction
            .table::<TxTable>()
            .await
            .expect("!DbTransaction::open_table");

        table
            .add_item(&rick_tx_1)
            .await
            .expect("!Couldn't add a 'RICK' transaction");
        table
            .add_item(&rick_tx_2)
            .await
            .expect("!Couldn't add a 'RICK' transaction with the different 'tx_hash'");
        table
            .add_item(&morty_tx_1)
            .await
            .expect("!Couldn't add a 'MORTY' transaction");

        assert_eq!(3, table.count_all().await.expect("!IndexedDb::count_all()"));
        assert_eq!(2, table.count("ticker", "RICK").await.expect("!IndexedDb::count()"));
        assert_eq!(1, table.count("ticker", "MORTY").await.expect("!IndexedDb::count()"));
    }

    #[wasm_bindgen_test]
    async fn test_replace_item() {
        const DB_NAME: &str = "TEST_REPLACE_ITEM";
        const DB_VERSION: u32 = 1;

        let rick_tx_1 = TxTable {
            ticker: "RICK".to_owned(),
            tx_hash: "0a0fda88364b960000f445351fe7678317a1e0c80584de0413377ede00ba696f".to_owned(),
            block_height: 10000,
        };
        let rick_tx_2 = TxTable {
            ticker: "RICK".to_owned(),
            tx_hash: "ba881ecca15b5d4593f14f25debbcdfe25f101fd2e9cf8d0b5d92d19813d4424".to_owned(),
            block_height: 10000,
        };
        let rick_tx_1_updated = TxTable {
            ticker: "RICK".to_owned(),
            tx_hash: "0a0fda88364b960000f445351fe7678317a1e0c80584de0413377ede00ba696f".to_owned(),
            block_height: 20000,
        };

        register_wasm_log();

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<TxTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction()");
        let table = transaction
            .table::<TxTable>()
            .await
            .expect("!DbTransaction::open_table");

        let rick_tx_1_id = table.add_item(&rick_tx_1).await.expect("Couldn't add an item");
        let rick_tx_2_id = table.add_item(&rick_tx_2).await.expect("Couldn't add an item");

        // Open new transaction.
        let transaction = db.transaction().await.expect("!IndexedDb::transaction()");
        let table = transaction
            .table::<TxTable>()
            .await
            .expect("!DbTransaction::open_table");

        // Try to add the updated RICK tx item with the same [`TxTable::tx_hash`].
        // [`TxTable::tx_hash`] is a unique index, so this operation must fail.
        let err = table
            .add_item(&rick_tx_1_updated)
            .await
            .expect_err("'DbTable::add_item' should have failed");
        match err.into_inner() {
            DbTransactionError::ErrorUploadingItem(err) => debug!("error: {err}"),
            e => panic!("Expected 'DbTransactionError::ErrorUploadingItem', found: {e:?}"),
        }
        assert_eq!(table.aborted().await, Ok(true));
        assert_eq!(transaction.aborted().await, Ok(true));

        // But we should be able to replace the updated item.
        // Since the last operation failed, we have to reopen the transaction.
        let transaction = db.transaction().await.expect("!IndexedDb::transaction()");
        let table = transaction
            .table::<TxTable>()
            .await
            .expect("!DbTransaction::open_table");

        let rick_tx_1_updated_id = table
            .replace_item(rick_tx_1_id, &rick_tx_1_updated)
            .await
            .expect("!Couldn't replace an item");
        assert_eq!(rick_tx_1_updated_id, rick_tx_1_id);

        let actual_rick_txs = table
            .get_items("ticker", "RICK")
            .await
            .expect("Couldn't get items by the index 'ticker=RICK'");
        assert_eq!(
            actual_rick_txs,
            vec![(rick_tx_1_id, rick_tx_1_updated), (rick_tx_2_id, rick_tx_2),]
        );
    }

    #[wasm_bindgen_test]
    async fn test_delete_item() {
        const DB_NAME: &str = "TEST_DELETE_ITEM";
        const DB_VERSION: u32 = 1;

        let rick_tx_1 = TxTable {
            ticker: "RICK".to_owned(),
            tx_hash: "0a0fda88364b960000f445351fe7678317a1e0c80584de0413377ede00ba696f".to_owned(),
            block_height: 10000,
        };
        let rick_tx_2 = TxTable {
            ticker: "RICK".to_owned(),
            tx_hash: "ba881ecca15b5d4593f14f25debbcdfe25f101fd2e9cf8d0b5d92d19813d4424".to_owned(),
            block_height: 10000,
        };

        register_wasm_log();

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<TxTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction()");
        let table = transaction
            .table::<TxTable>()
            .await
            .expect("!DbTransaction::open_table");

        let rick_tx_1_id = table.add_item(&rick_tx_1).await.expect("Couldn't add an item");
        let rick_tx_2_id = table.add_item(&rick_tx_2).await.expect("Couldn't add an item");

        table.delete_item(rick_tx_1_id).await.expect("Couldn't delete an item");

        let actual_rick_txs = table
            .get_items("ticker", "RICK")
            .await
            .expect("Couldn't get items by the index 'ticker=RICK'");
        assert_eq!(actual_rick_txs, vec![(rick_tx_2_id, rick_tx_2)]);
    }

    #[wasm_bindgen_test]
    async fn test_clear() {
        const DB_NAME: &str = "TEST_CLEAR";
        const DB_VERSION: u32 = 1;

        let rick_tx_1 = TxTable {
            ticker: "RICK".to_owned(),
            tx_hash: "0a0fda88364b960000f445351fe7678317a1e0c80584de0413377ede00ba696f".to_owned(),
            block_height: 10000,
        };
        let rick_tx_2 = TxTable {
            ticker: "RICK".to_owned(),
            tx_hash: "ba881ecca15b5d4593f14f25debbcdfe25f101fd2e9cf8d0b5d92d19813d4424".to_owned(),
            block_height: 10000,
        };

        register_wasm_log();

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<TxTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction()");
        let table = transaction
            .table::<TxTable>()
            .await
            .expect("!DbTransaction::open_table");

        let _rick_tx_1_id = table.add_item(&rick_tx_1).await.expect("Couldn't add an item");
        let _rick_tx_2_id = table.add_item(&rick_tx_2).await.expect("Couldn't add an item");

        table.clear().await.expect("Couldn't clear the database");

        let actual_rick_txs = table
            .get_items("ticker", "RICK")
            .await
            .expect("Couldn't get items by the index 'ticker=RICK'");
        assert!(actual_rick_txs.is_empty());
    }

    #[wasm_bindgen_test]
    async fn test_upgrade_needed() {
        const DB_NAME: &str = "TEST_UPGRADE_NEEDED";

        lazy_static! {
            static ref LAST_VERSIONS: Mutex<Option<(u32, u32)>> = Mutex::new(None);
        }

        #[derive(Serialize, Deserialize)]
        struct UpgradableTable;

        impl TableSignature for UpgradableTable {
            const TABLE_NAME: &'static str = "upgradable_table";

            fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
                let mut versions = LAST_VERSIONS.lock().expect("!old_new_versions.lock()");
                *versions = Some((old_version, new_version));

                match (old_version, new_version) {
                    (0, 1) => {
                        let table = upgrader.create_table("upgradable_table")?;
                        table.create_index("first_index", false)?;
                    },
                    (0, 2) => {
                        let table = upgrader.create_table("upgradable_table")?;
                        table.create_index("first_index", false)?;
                        table.create_index("second_index", false)?;
                    },
                    (1, 2) => {
                        let table = upgrader.open_table("upgradable_table")?;
                        table.create_index("second_index", false)?;
                    },
                    v => panic!("Unexpected old, new versions: {v:?}"),
                }
                Ok(())
            }
        }

        async fn init_and_check(
            db_identifier: DbIdentifier,
            version: u32,
            expected_old_new_versions: Option<(u32, u32)>,
        ) -> Result<(), String> {
            {
                let mut versions = LAST_VERSIONS.lock().expect("!LAST_VERSIONS.lock()");
                *versions = None;
            }

            let _db = IndexedDbBuilder::new(db_identifier)
                .with_version(version)
                .with_table::<UpgradableTable>()
                .build()
                .await
                .map_err(|e| format!("{e}"))?;

            let actual_versions = LAST_VERSIONS.lock().unwrap();
            if *actual_versions == expected_old_new_versions {
                Ok(())
            } else {
                Err(format!(
                    "Expected {expected_old_new_versions:?}, found {actual_versions:?}"
                ))
            }
        }

        register_wasm_log();

        let db_identifier = DbIdentifier::for_test(DB_NAME);

        init_and_check(db_identifier.clone(), 1, Some((0, 1))).await.unwrap();
        init_and_check(db_identifier.clone(), 2, Some((1, 2))).await.unwrap();
        // the same 2 version, `on_upgrade_needed` must not be called
        init_and_check(db_identifier, 2, None).await.unwrap();
    }

    #[wasm_bindgen_test]
    async fn test_open_twice() {
        const DB_NAME: &str = "TEST_OPEN_TWICE";
        const DB_VERSION: u32 = 1;

        register_wasm_log();
        let db_identifier = DbIdentifier::for_test(DB_NAME);

        let _db = IndexedDbBuilder::new(db_identifier.clone())
            .with_version(DB_VERSION)
            .with_table::<TxTable>()
            .build()
            .await
            .expect("!IndexedDb::init first time");

        match IndexedDbBuilder::new(db_identifier.clone())
            .with_version(DB_VERSION + 1)
            .with_table::<TxTable>()
            .build()
            .await
        {
            Ok(_) => panic!("!IndexedDb::init should have failed"),
            Err(e) => assert_eq!(
                e.into_inner(),
                InitDbError::DbIsOpenAlready {
                    db_name: db_identifier.to_string()
                }
            ),
        }
    }

    #[wasm_bindgen_test]
    async fn test_open_close_and_open() {
        const DB_NAME: &str = "TEST_OPEN_CLOSE_AND_OPEN";
        const DB_VERSION: u32 = 1;

        register_wasm_log();
        let db_identifier = DbIdentifier::for_test(DB_NAME);

        let db = IndexedDbBuilder::new(db_identifier.clone())
            .with_version(DB_VERSION)
            .with_table::<TxTable>()
            .build()
            .await
            .expect("!IndexedDb::init first time");
        drop(db);

        let _db = IndexedDbBuilder::new(db_identifier)
            .with_version(DB_VERSION)
            .with_table::<TxTable>()
            .build()
            .await
            .expect("!IndexedDb::init second time");
    }

    #[wasm_bindgen_test]
    async fn test_non_string_index() {
        const DB_NAME: &str = "TEST_NON_STRING_INDEX";
        const DB_VERSION: u32 = 1;

        #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
        struct Uuid(Vec<u64>);

        #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
        #[serde(deny_unknown_fields)]
        struct SwapTable {
            swap_uuid: Uuid,
            started_at: u64,
            some_data: String,
        }

        impl TableSignature for SwapTable {
            const TABLE_NAME: &'static str = "swap_table";

            fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, _new_version: u32) -> OnUpgradeResult<()> {
                if old_version > 0 {
                    // the table is initialized already
                    return Ok(());
                }
                let table_upgrader = upgrader.create_table("swap_table")?;
                table_upgrader.create_index("swap_uuid", false)?;
                table_upgrader.create_index("started_at", false)
            }
        }

        register_wasm_log();

        let swap_1 = SwapTable {
            swap_uuid: Uuid(vec![1, 2, 3]),
            started_at: 123,
            some_data: "Some data 1".to_owned(),
        };
        let swap_2 = SwapTable {
            swap_uuid: Uuid(vec![3, 2, 1]),
            started_at: 321,
            some_data: "Some data 2".to_owned(),
        };

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<SwapTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction()");
        let table = transaction
            .table::<SwapTable>()
            .await
            .expect("!DbTransaction::open_table");

        let swap_1_id = table.add_item(&swap_1).await.expect("Couldn't add an item");
        let swap_2_id = table.add_item(&swap_2).await.expect("Couldn't add an item");

        let actual = table
            .get_items("swap_uuid", vec![3, 2, 1])
            .await
            .expect("Couldn't get items");
        assert_eq!(actual, vec![(swap_2_id, swap_2)]);

        let actual = table.get_items("started_at", 123).await.expect("Couldn't get items");
        assert_eq!(actual, vec![(swap_1_id, swap_1)]);
    }

    #[wasm_bindgen_test]
    async fn test_transaction_abort_on_error() {
        const DB_NAME: &str = "TEST_TRANSACTION_ABORT_ON_ERROR";
        const DB_VERSION: u32 = 1;

        let tx_1 = TxTable {
            ticker: "RICK".to_owned(),
            tx_hash: "0a0fda88364b960000f445351fe7678317a1e0c80584de0413377ede00ba696f".to_owned(),
            block_height: 10000,
        };
        let tx_2 = TxTable {
            ticker: "RICK".to_owned(),
            tx_hash: "ba881ecca15b5d4593f14f25debbcdfe25f101fd2e9cf8d0b5d92d19813d4424".to_owned(),
            block_height: 10000,
        };
        // The transaction has the same `tx_hash` as `tx_1`.
        let invalid_tx = TxTable {
            ticker: "RICK".to_owned(),
            tx_hash: "0a0fda88364b960000f445351fe7678317a1e0c80584de0413377ede00ba696f".to_owned(),
            block_height: 20000,
        };

        register_wasm_log();

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<TxTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction()");
        let table = transaction
            .table::<TxTable>()
            .await
            .expect("!DbTransaction::open_table");

        let _tx_1_id = table.add_item(&tx_1).await.expect("Couldn't add an item");
        let _tx_2_id = table.add_item(&tx_2).await.expect("Couldn't add an item");

        // Try to add the updated RICK tx item with the same [`TxTable::tx_hash`].
        // [`TxTable::tx_hash`] is a unique index, so this operation must fail.
        let _err = table
            .add_item(&invalid_tx)
            .await
            .expect_err("'DbTable::add_item' should have failed");
        assert_eq!(table.aborted().await, Ok(true));
        assert_eq!(transaction.aborted().await, Ok(true));

        // Check if `tx_1` and `tx_2` hasn't been uploaded.
        // Since the last operation failed, we have to reopen the transaction.
        let transaction = db.transaction().await.expect("!IndexedDb::transaction()");
        let table = transaction
            .table::<TxTable>()
            .await
            .expect("!DbTransaction::open_table");

        let actual_txs = table
            .get_items("ticker", "RICK")
            .await
            .expect("Couldn't get items by the index 'ticker=RICK'");
        assert!(actual_txs.is_empty());
    }
}
