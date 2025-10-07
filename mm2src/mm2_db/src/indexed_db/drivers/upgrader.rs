use common::stringify_js_error;
use derive_more::Display;
use js_sys::Array;
use mm2_err_handle::prelude::*;
use wasm_bindgen::prelude::*;
use web_sys::{IdbDatabase, IdbIndexParameters, IdbObjectStore, IdbObjectStoreParameters, IdbTransaction};

const ITEM_KEY_PATH: &str = "_item_id";

pub type OnUpgradeResult<T> = Result<T, MmError<OnUpgradeError>>;
pub type OnUpgradeNeededCb = Box<dyn FnOnce(&DbUpgrader, u32, u32) -> OnUpgradeResult<()> + Send>;

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
}

pub struct DbUpgrader {
    db: IdbDatabase,
    transaction: IdbTransaction,
}

impl DbUpgrader {
    pub(crate) fn new(db: IdbDatabase, transaction: IdbTransaction) -> DbUpgrader {
        DbUpgrader { db, transaction }
    }

    pub fn create_table(&self, table: &str) -> OnUpgradeResult<TableUpgrader> {
        // We use the [in-line](https://developer.mozilla.org/en-US/docs/Web/API/IndexedDB_API/Basic_Concepts_Behind_IndexedDB#gloss_inline_key) primary keys.
        let key_path = JsValue::from(ITEM_KEY_PATH);

        let params = IdbObjectStoreParameters::new();
        params.set_key_path(&key_path);
        params.set_auto_increment(true);

        match self.db.create_object_store_with_optional_parameters(table, &params) {
            Ok(object_store) => Ok(TableUpgrader { object_store }),
            Err(e) => MmError::err(OnUpgradeError::ErrorCreatingTable {
                table: table.to_owned(),
                description: stringify_js_error(&e),
            }),
        }
    }

    /// Open the `table` if it was created already.
    pub fn open_table(&self, table: &str) -> OnUpgradeResult<TableUpgrader> {
        match self.transaction.object_store(table) {
            Ok(object_store) => Ok(TableUpgrader { object_store }),
            Err(e) => MmError::err(OnUpgradeError::ErrorOpeningTable {
                table: table.to_owned(),
                description: stringify_js_error(&e),
            }),
        }
    }
}

pub struct TableUpgrader {
    object_store: IdbObjectStore,
}

impl TableUpgrader {
    /// Creates an index.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/createIndex
    pub fn create_index(&self, index: &str, unique: bool) -> OnUpgradeResult<()> {
        let params = IdbIndexParameters::new();
        params.set_unique(unique);
        self.object_store
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
            .delete_index(index)
            .map(|_| ())
            .map_to_mm(|e| OnUpgradeError::ErrorDeletingIndex {
                index: index.to_owned(),
                description: stringify_js_error(&e),
            })
    }
}
