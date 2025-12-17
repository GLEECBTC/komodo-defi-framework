use super::{construct_event_closure, DbTransactionError, DbTransactionResult, InternalItem, ItemId};
use common::{deserialize_from_js, serialize_to_js, stringify_js_error};
use futures::channel::mpsc;
use futures::StreamExt;
use mm2_err_handle::prelude::*;
use serde_json::Value as Json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{IdbIndex, IdbObjectStore, IdbRequest};

pub struct IdbObjectStoreImpl {
    pub(crate) object_store: IdbObjectStore,
    pub(crate) aborted: Arc<AtomicBool>,
    pub(crate) _not_send: common::NotSend,
}

impl IdbObjectStoreImpl {
    pub(crate) fn aborted(&self) -> bool {
        self.aborted.load(Ordering::Relaxed)
    }

    pub(crate) async fn add_item(&self, item: &Json) -> DbTransactionResult<ItemId> {
        if self.aborted.load(Ordering::Relaxed) {
            return MmError::err(DbTransactionError::TransactionAborted);
        }

        // The [`InternalItem::item`] is a flatten field, so if we add the item without the [`InternalItem::_item_id`] id,
        // it will be calculated automatically.
        let js_value = match serialize_to_js(item) {
            Ok(value) => value,
            Err(e) => return MmError::err(DbTransactionError::ErrorSerializingItem(e.to_string())),
        };
        let add_request = match self.object_store.add(&js_value) {
            Ok(request) => request,
            Err(e) => return MmError::err(DbTransactionError::ErrorUploadingItem(stringify_js_error(&e))),
        };

        if let Err(_error_event) = Self::wait_for_request_complete(&add_request).await {
            self.aborted.store(true, Ordering::Relaxed);
            let error = Self::error_from_failed_request(&add_request);
            return MmError::err(DbTransactionError::ErrorUploadingItem(error));
        }

        Self::item_id_from_completed_request(&add_request)
    }

    pub(crate) async fn get_items(
        &self,
        index_str: &str,
        index_value: Json,
    ) -> DbTransactionResult<Vec<(ItemId, Json)>> {
        if self.aborted.load(Ordering::Relaxed) {
            return MmError::err(DbTransactionError::TransactionAborted);
        }

        let index = index_str.to_owned();
        let index_value_js = try_serialize_index_value!(serialize_to_js(&index_value), index_str);

        let db_index = self.open_index(index_str)?;
        let get_request = match db_index.get_all_with_key(&index_value_js) {
            Ok(request) => request,
            Err(e) => {
                return MmError::err(DbTransactionError::InvalidIndex {
                    index,
                    index_value,
                    description: stringify_js_error(&e),
                })
            },
        };

        if let Err(_error_event) = Self::wait_for_request_complete(&get_request).await {
            self.aborted.store(true, Ordering::Relaxed);
            let error = Self::error_from_failed_request(&get_request);
            return MmError::err(DbTransactionError::ErrorGettingItems(error));
        }

        Self::items_from_completed_request(&get_request)
    }

    pub(crate) async fn get_item_ids(&self, index_str: &str, index_value: Json) -> DbTransactionResult<Vec<ItemId>> {
        if self.aborted.load(Ordering::Relaxed) {
            return MmError::err(DbTransactionError::TransactionAborted);
        }

        let index = index_str.to_owned();
        let index_value_js = try_serialize_index_value!(serialize_to_js(&index_value), index);

        let db_index = self.open_index(index_str)?;
        let get_request = match db_index.get_all_keys_with_key(&index_value_js) {
            Ok(request) => request,
            Err(e) => {
                return MmError::err(DbTransactionError::InvalidIndex {
                    index,
                    index_value,
                    description: stringify_js_error(&e),
                })
            },
        };

        if let Err(_error_event) = Self::wait_for_request_complete(&get_request).await {
            self.aborted.store(true, Ordering::Relaxed);
            let error = Self::error_from_failed_request(&get_request);
            return MmError::err(DbTransactionError::ErrorGettingItems(error));
        }

        Self::item_ids_from_completed_request(&get_request)
    }

    pub(crate) async fn get_all_items(&self) -> DbTransactionResult<Vec<(ItemId, Json)>> {
        if self.aborted.load(Ordering::Relaxed) {
            return MmError::err(DbTransactionError::TransactionAborted);
        }

        let get_request = match self.object_store.get_all() {
            Ok(request) => request,
            Err(e) => return MmError::err(DbTransactionError::UnexpectedState(stringify_js_error(&e))),
        };

        if let Err(_error_event) = Self::wait_for_request_complete(&get_request).await {
            self.aborted.store(true, Ordering::Relaxed);
            let error = Self::error_from_failed_request(&get_request);
            return MmError::err(DbTransactionError::ErrorGettingItems(error));
        }

        Self::items_from_completed_request(&get_request)
    }

    pub(crate) async fn count(&self, index_str: &str, index_value: Json) -> DbTransactionResult<usize> {
        if self.aborted.load(Ordering::Relaxed) {
            return MmError::err(DbTransactionError::TransactionAborted);
        }

        let index = index_str.to_owned();
        let index_value_js = try_serialize_index_value!(serialize_to_js(&index_value), index);

        let db_index = self.open_index(index_str)?;
        let count_request = match db_index.count_with_key(&index_value_js) {
            Ok(request) => request,
            Err(e) => {
                return MmError::err(DbTransactionError::InvalidIndex {
                    index,
                    index_value,
                    description: stringify_js_error(&e),
                })
            },
        };

        if let Err(_error_event) = Self::wait_for_request_complete(&count_request).await {
            self.aborted.store(true, Ordering::Relaxed);
            let error = Self::error_from_failed_request(&count_request);
            return MmError::err(DbTransactionError::ErrorCountingItems(error));
        }

        Self::count_from_completed_request(&count_request)
    }

    pub(crate) async fn count_all(&self) -> DbTransactionResult<usize> {
        if self.aborted.load(Ordering::Relaxed) {
            return MmError::err(DbTransactionError::TransactionAborted);
        }

        let count_request = match self.object_store.count() {
            Ok(request) => request,
            Err(e) => return MmError::err(DbTransactionError::ErrorCountingItems(stringify_js_error(&e))),
        };

        if let Err(_error_event) = Self::wait_for_request_complete(&count_request).await {
            self.aborted.store(true, Ordering::Relaxed);
            let error = Self::error_from_failed_request(&count_request);
            return MmError::err(DbTransactionError::ErrorCountingItems(error));
        }

        Self::count_from_completed_request(&count_request)
    }

    pub(crate) async fn replace_item(&self, _item_id: ItemId, item: Json) -> DbTransactionResult<ItemId> {
        if self.aborted.load(Ordering::Relaxed) {
            return MmError::err(DbTransactionError::TransactionAborted);
        }

        let item_with_key = InternalItem { _item_id, item };
        let js_value = match serialize_to_js(&item_with_key) {
            Ok(value) => value,
            Err(e) => return MmError::err(DbTransactionError::ErrorSerializingItem(e.to_string())),
        };
        let replace_request = match self.object_store.put(&js_value) {
            Ok(request) => request,
            Err(e) => return MmError::err(DbTransactionError::ErrorUploadingItem(stringify_js_error(&e))),
        };

        if let Err(_error_event) = Self::wait_for_request_complete(&replace_request).await {
            self.aborted.store(true, Ordering::Relaxed);
            let error = Self::error_from_failed_request(&replace_request);
            return MmError::err(DbTransactionError::ErrorUploadingItem(error));
        }

        Self::item_id_from_completed_request(&replace_request)
    }

    pub(crate) async fn delete_item(&self, item_id: ItemId) -> DbTransactionResult<()> {
        if self.aborted.load(Ordering::Relaxed) {
            return MmError::err(DbTransactionError::TransactionAborted);
        }

        let item_id = JsValue::from(item_id);

        let delete_request = match self.object_store.delete(&item_id) {
            Ok(request) => request,
            Err(e) => return MmError::err(DbTransactionError::ErrorDeletingItems(stringify_js_error(&e))),
        };

        if let Err(_error_event) = Self::wait_for_request_complete(&delete_request).await {
            self.aborted.store(true, Ordering::Relaxed);
            let error = Self::error_from_failed_request(&delete_request);
            return MmError::err(DbTransactionError::ErrorDeletingItems(error));
        }

        Ok(())
    }

    pub(crate) async fn clear(&self) -> DbTransactionResult<()> {
        if self.aborted.load(Ordering::Relaxed) {
            return MmError::err(DbTransactionError::TransactionAborted);
        }

        let clear_request = match self.object_store.clear() {
            Ok(request) => request,
            Err(e) => return MmError::err(DbTransactionError::ErrorDeletingItems(stringify_js_error(&e))),
        };

        if let Err(_error_event) = Self::wait_for_request_complete(&clear_request).await {
            self.aborted.store(true, Ordering::Relaxed);
            let error = Self::error_from_failed_request(&clear_request);
            return MmError::err(DbTransactionError::ErrorDeletingItems(error));
        }

        Ok(())
    }

    pub(crate) fn open_index(&self, index_str: &str) -> DbTransactionResult<IdbIndex> {
        match self.object_store.index(index_str) {
            Ok(index) => Ok(index),
            Err(_) => MmError::err(DbTransactionError::NoSuchIndex {
                index: index_str.to_owned(),
            }),
        }
    }

    async fn wait_for_request_complete(request: &IdbRequest) -> Result<JsValue, JsValue> {
        let (tx, mut rx) = mpsc::channel(2);

        let onsuccess_closure = construct_event_closure(Ok, tx.clone());
        let onerror_closure = construct_event_closure(Err, tx.clone());

        request.set_onsuccess(Some(onsuccess_closure.as_ref().unchecked_ref()));
        request.set_onerror(Some(onerror_closure.as_ref().unchecked_ref()));

        rx.next().await.expect("The request event channel must not be closed")
    }

    fn item_id_from_completed_request(request: &IdbRequest) -> DbTransactionResult<ItemId> {
        let result_js_value = match request.result() {
            Ok(res) => res,
            Err(e) => return MmError::err(DbTransactionError::UnexpectedState(stringify_js_error(&e))),
        };

        deserialize_from_js(result_js_value).map_to_mm(|e| DbTransactionError::ErrorDeserializingItem(e.to_string()))
    }

    fn items_from_completed_request(request: &IdbRequest) -> DbTransactionResult<Vec<(ItemId, Json)>> {
        let result_js_value = match request.result() {
            Ok(res) => res,
            Err(e) => return MmError::err(DbTransactionError::UnexpectedState(stringify_js_error(&e))),
        };

        if result_js_value.is_null() || result_js_value.is_undefined() {
            return Ok(Vec::new());
        }

        let items: Vec<InternalItem> = match deserialize_from_js(result_js_value) {
            Ok(items) => items,
            Err(e) => return MmError::err(DbTransactionError::ErrorDeserializingItem(e.to_string())),
        };

        Ok(items.into_iter().map(|item| (item._item_id, item.item)).collect())
    }

    fn item_ids_from_completed_request(request: &IdbRequest) -> DbTransactionResult<Vec<ItemId>> {
        let result_js_value = match request.result() {
            Ok(res) => res,
            Err(e) => return MmError::err(DbTransactionError::UnexpectedState(stringify_js_error(&e))),
        };

        if result_js_value.is_null() || result_js_value.is_undefined() {
            return Ok(Vec::new());
        }

        deserialize_from_js(result_js_value).map_to_mm(|e| DbTransactionError::ErrorDeserializingItem(e.to_string()))
    }

    fn count_from_completed_request(request: &IdbRequest) -> DbTransactionResult<usize> {
        let result_js_value = match request.result() {
            Ok(res) => res,
            Err(e) => return MmError::err(DbTransactionError::UnexpectedState(stringify_js_error(&e))),
        };

        if result_js_value.is_null() || result_js_value.is_undefined() {
            return Ok(0);
        }

        deserialize_from_js(result_js_value).map_to_mm(|e| DbTransactionError::ErrorDeserializingItem(e.to_string()))
    }

    fn error_from_failed_request(request: &IdbRequest) -> String {
        match request.error() {
            Ok(Some(exception)) => format!("{exception:?}"),
            _ => "Unknown".to_owned(),
        }
    }
}
