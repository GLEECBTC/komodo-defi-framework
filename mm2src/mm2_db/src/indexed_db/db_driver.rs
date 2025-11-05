//! The representation of [Indexed DB](https://developer.mozilla.org/en-US/docs/Web/API/IndexedDB_API).
//! This module consists of low-level Rust wrappers over basic JS structures like `IDBDatabase`, `IDBTransaction`, `IDBObjectStore` etc.
//!
//! # Usage
//!
//! Since the wrappers represented below are not `Send`, it's strongly recommended NOT to use them directly.
//! Please consider using a higher-level interface from `indexed_db.rs`.

use common::log::{error, info};
use common::stringify_js_error;
use futures::channel::mpsc;
use js_sys::Array;
use lazy_static::lazy_static;
use mm2_err_handle::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::collections::HashSet;
use std::fmt;
use std::sync::Mutex;
use wasm_bindgen::prelude::*;
use web_sys::{IdbDatabase, IdbTransactionMode};

#[path = "drivers/builder.rs"]
mod builder;
#[path = "drivers/cursor/cursor.rs"]
pub(super) mod cursor;
#[path = "drivers/object_store.rs"]
mod object_store;
#[path = "drivers/transaction.rs"]
mod transaction;
#[path = "drivers/upgrader.rs"]
mod upgrader;

pub use builder::{IdbDatabaseBuilder, InitDbError, InitDbResult};
pub use object_store::IdbObjectStoreImpl;
pub use transaction::{DbTransactionError, DbTransactionResult, IdbTransactionImpl};
pub use upgrader::{DbUpgrader, OnUpgradeError, OnUpgradeNeededCb, OnUpgradeResult};

lazy_static! {
    static ref OPEN_DATABASES: Mutex<HashSet<String>> = Mutex::new(HashSet::new());
}

pub type ItemId = u32;

#[derive(Debug, Deserialize, Serialize)]
struct InternalItem {
    _item_id: ItemId,
    #[serde(flatten)]
    item: Json,
}

impl InternalItem {
    pub fn into_pair(self) -> (ItemId, Json) {
        (self._item_id, self.item)
    }
}

pub struct IdbDatabaseImpl {
    db: IdbDatabase,
    db_name: String,
    tables: HashSet<String>,
    _not_send: common::NotSend,
}

impl fmt::Debug for IdbDatabaseImpl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "IndexedDb {{ db_name: {:?}, tables: {:?} }}",
            self.db_name, self.tables
        )
    }
}

impl IdbDatabaseImpl {
    pub fn transaction(&self) -> DbTransactionResult<IdbTransactionImpl> {
        let store_names = Array::new();
        for table in self.tables.iter() {
            store_names.push(&JsValue::from(table));
        }

        match self
            .db
            .transaction_with_str_sequence_and_mode(&store_names, IdbTransactionMode::Readwrite)
        {
            Ok(transaction) => Ok(IdbTransactionImpl::init(transaction, self.tables.clone())),
            Err(e) => MmError::err(DbTransactionError::ErrorCreatingTransaction(stringify_js_error(&e))),
        }
    }
}

impl Drop for IdbDatabaseImpl {
    fn drop(&mut self) {
        info!("'{}' database has been closed", self.db_name);
        self.db.close();
        let mut open_databases = OPEN_DATABASES.lock().expect("!OPEN_DATABASES.lock()");
        open_databases.remove(&self.db_name);
    }
}

/// Please note the `Event` type can be `JsValue`. It doesn't lead to a runtime error, because [`JsValue::dyn_into<JsValue>()`] returns itself.
fn construct_event_closure<F, Event>(mut f: F, mut event_tx: mpsc::Sender<Event>) -> Closure<dyn FnMut(JsValue)>
where
    F: FnMut(JsValue) -> Event + 'static,
    Event: fmt::Debug + 'static,
{
    Closure::new(move |event: JsValue| {
        let open_event = f(event);
        if let Err(e) = event_tx.try_send(open_event) {
            let error = e.to_string();
            let event = e.into_inner();
            error!("Error sending the '{:?}' event: {}", event, error);
        }
    })
}
