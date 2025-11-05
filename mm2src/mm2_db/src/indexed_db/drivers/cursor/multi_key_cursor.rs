use super::{CursorAction, CursorDriverImpl, CursorError, CursorItemAction, CursorResult};
use common::{serialize_to_js, stringify_js_error};
use js_sys::Array;
use mm2_err_handle::prelude::*;
use serde_json::Value as Json;
use wasm_bindgen::prelude::*;
use web_sys::IdbKeyRange;

/// The representation of a range that includes records
/// whose fields have only the specified [`IdbSingleCursor::only_values`] values.
/// https://developer.mozilla.org/en-US/docs/Web/API/IDBKeyRange/only
pub struct IdbMultiKeyCursor {
    only_values: Vec<(String, Json)>,
}

impl IdbMultiKeyCursor {
    pub(super) fn new(only_values: Vec<(String, Json)>) -> IdbMultiKeyCursor {
        IdbMultiKeyCursor { only_values }
    }
}

impl CursorDriverImpl for IdbMultiKeyCursor {
    fn key_range(&self) -> CursorResult<Option<IdbKeyRange>> {
        let only = Array::new();

        for (field, value) in self.only_values.iter() {
            let js_value = serialize_to_js(value).map_to_mm(|e| CursorError::ErrorSerializingIndexFieldValue {
                field: field.to_owned(),
                value: format!("{value:?}"),
                description: e.to_string(),
            })?;
            only.push(&js_value);
        }

        let key_range = IdbKeyRange::only(&only).map_to_mm(|e| CursorError::InvalidKeyRange {
            description: stringify_js_error(&e),
        })?;
        Ok(Some(key_range))
    }

    fn on_iteration(&mut self, _key: JsValue) -> CursorResult<(CursorItemAction, CursorAction)> {
        Ok((CursorItemAction::Include, CursorAction::Continue))
    }
}
