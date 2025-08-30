use common::stringify_js_error;
use derive_more::Display;
use futures::future::BoxFuture;
use js_sys::Array;
use mm2_err_handle::prelude::*;
use wasm_bindgen::prelude::*;
use web_sys::{IdbIndexParameters, IdbObjectStoreParameters};

use crate::indexed_db::db_driver::{IdbDatabaseImpl, IdbObjectStoreImpl, IdbTransactionImpl};

const ITEM_KEY_PATH: &str = "_item_id";

pub type OnUpgradeResult<T> = Result<T, MmError<OnUpgradeError>>;
pub type OnUpgradeNeededCb =
    Box<dyn for<'a> FnOnce(&'a DbUpgrader, u32, u32) -> BoxFuture<'a, OnUpgradeResult<()>> + Send>;

#[derive(Debug, Display, PartialEq)]
pub enum OnUpgradeError {
    #[display(fmt = "Error occurred due to creating the '{table}' table: {description}")]
    ErrorCreatingTable { table: String, description: String },
    #[display(fmt = "Error occurred due to opening the '{table}' table: {description}")]
    ErrorOpeningTable { table: String, description: String },
    #[display(fmt = "Error occurred due to creating the '{index}' index: {description}")]
    ErrorCreatingIndex { index: String, description: String },
    #[display(
        fmt = "Upgrade attempt to an unsupported version: {unsupported_version}, old: {old_version}, new: {new_version}"
    )]
    UnsupportedVersion {
        unsupported_version: u32,
        old_version: u32,
        new_version: u32,
    },
    #[display(fmt = "Error occurred due to deleting the '{index}' index: {description}")]
    ErrorDeletingIndex { index: String, description: String },
    #[display(fmt = "Error occurred due to clearing the '{table}' table: {description}")]
    ErrorClearingTable { table: String, description: String },
}

pub struct DbUpgrader {
    db: IdbDatabaseImpl,
    transaction: IdbTransactionImpl,
}

/// This is a hack to get us to pass `&'a DbUpgrader` around in on_upgrade_needed callbacks.
/// That said, `DbUpgrader` is short lived and is used solely for the on_upgrade_needed callbacks,
/// and then dropped right away. The content inside it shouldn't change after initialization
/// and the only mutable part is `transaction.aborted` which is an `Arc<AtomicBool>`, thus it is safe
/// to share.
/// Again, this really won't be shared across multiple writers at the same time, and is only passed
/// to a single on_upgrade_needed callback at a time.
unsafe impl Sync for DbUpgrader {}

impl DbUpgrader {
    pub(crate) fn new(db: IdbDatabaseImpl, transaction: IdbTransactionImpl) -> DbUpgrader {
        DbUpgrader { db, transaction }
    }

    pub fn create_table(&self, table: &str) -> OnUpgradeResult<TableUpgrader> {
        // We use the [in-line](https://developer.mozilla.org/en-US/docs/Web/API/IndexedDB_API/Basic_Concepts_Behind_IndexedDB#gloss_inline_key) primary keys.
        let key_path = JsValue::from(ITEM_KEY_PATH);

        let params = IdbObjectStoreParameters::new();
        params.set_key_path(&key_path);
        params.set_auto_increment(true);

        match self.db.db.create_object_store_with_optional_parameters(table, &params) {
            Ok(object_store) => Ok(TableUpgrader {
                object_store: IdbObjectStoreImpl {
                    object_store,
                    aborted: self.transaction.aborted.clone(),
                    _not_send: Default::default(),
                },
            }),
            Err(e) => MmError::err(OnUpgradeError::ErrorCreatingTable {
                table: table.to_owned(),
                description: stringify_js_error(&e),
            }),
        }
    }

    /// Open the `table` if it was created already.
    pub fn open_table(&self, table: &str) -> OnUpgradeResult<TableUpgrader> {
        match self.transaction.transaction.object_store(table) {
            Ok(object_store) => Ok(TableUpgrader {
                object_store: IdbObjectStoreImpl {
                    object_store,
                    aborted: self.transaction.aborted.clone(),
                    _not_send: Default::default(),
                },
            }),
            Err(e) => MmError::err(OnUpgradeError::ErrorOpeningTable {
                table: table.to_owned(),
                description: stringify_js_error(&e),
            }),
        }
    }
}

pub struct TableUpgrader {
    object_store: IdbObjectStoreImpl,
}

impl TableUpgrader {
    /// Creates an index.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/createIndex
    pub fn create_index(&self, index: &str, unique: bool) -> OnUpgradeResult<()> {
        let params = IdbIndexParameters::new();
        params.set_unique(unique);
        self.object_store
            .object_store
            .create_index_with_str_and_optional_parameters(index, index, &params)
            .map(|_| ())
            .map_to_mm(|e| OnUpgradeError::ErrorCreatingIndex {
                index: index.to_owned(),
                description: stringify_js_error(&e),
            })
    }

    /// Creates an index with the multiple keys.
    /// Each key of the index has to be a field of the table.
    /// Such indexes are used to find records that satisfy constraints imposed on multiple fields.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/createIndex
    pub fn create_multi_index(&self, index: &str, fields: &[&str], unique: bool) -> OnUpgradeResult<()> {
        let params = IdbIndexParameters::new();
        params.set_unique(unique);

        let fields_key_path = Array::new();
        for field in fields {
            fields_key_path.push(&JsValue::from(*field));
        }

        self.object_store
            .object_store
            .create_index_with_str_sequence_and_optional_parameters(index, &fields_key_path, &params)
            .map(|_| ())
            .map_to_mm(|e| OnUpgradeError::ErrorCreatingIndex {
                index: index.to_owned(),
                description: stringify_js_error(&e),
            })
    }

    /// Deletes an index.
    /// Regardless of whether the index is created using one or multiple fields, the deleteIndex()
    /// method is used to delete any type of index, and it works in the same way for both.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/deleteIndex
    pub fn delete_index(&self, index: &str) -> OnUpgradeResult<()> {
        self.object_store
            .object_store
            .delete_index(index)
            .map(|_| ())
            .map_to_mm(|e| OnUpgradeError::ErrorDeletingIndex {
                index: index.to_owned(),
                description: stringify_js_error(&e),
            })
    }
}
