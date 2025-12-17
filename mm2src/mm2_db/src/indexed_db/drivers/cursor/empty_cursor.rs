use super::{CursorAction, CursorDriverImpl, CursorItemAction, CursorResult};
use wasm_bindgen::prelude::*;
use web_sys::IdbKeyRange;

/// The representation of a range that includes all records.
pub struct IdbEmptyCursor;

impl CursorDriverImpl for IdbEmptyCursor {
    fn key_range(&self) -> CursorResult<Option<IdbKeyRange>> {
        Ok(None)
    }

    fn on_iteration(&mut self, _key: JsValue) -> CursorResult<(CursorItemAction, CursorAction)> {
        Ok((CursorItemAction::Include, CursorAction::Continue))
    }
}
