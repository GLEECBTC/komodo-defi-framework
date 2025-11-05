use super::{CursorAction, CursorBoundValue, CursorDriverImpl, CursorError, CursorItemAction, CursorResult};
use common::{log::warn, stringify_js_error};
use mm2_err_handle::prelude::*;
use wasm_bindgen::prelude::*;
use web_sys::IdbKeyRange;

/// The representation of a range that includes records
/// whose value of the [`IdbSingleBoundCursor::field_name`] field is lower than [`IdbSingleBoundCursor::lower_bound_value`]
/// and greater than [`IdbSingleBoundCursor::upper_bound_value`].
/// https://developer.mozilla.org/en-US/docs/Web/API/IDBKeyRange/bound
pub struct IdbSingleKeyBoundCursor {
    #[allow(dead_code)]
    field_name: String,
    lower_bound: CursorBoundValue,
    upper_bound: CursorBoundValue,
}

impl IdbSingleKeyBoundCursor {
    pub(super) fn new(
        field_name: String,
        lower_bound: CursorBoundValue,
        upper_bound: CursorBoundValue,
    ) -> CursorResult<IdbSingleKeyBoundCursor> {
        Self::check_bounds(&lower_bound, &upper_bound)?;
        Ok(IdbSingleKeyBoundCursor {
            field_name,
            lower_bound,
            upper_bound,
        })
    }
}

impl IdbSingleKeyBoundCursor {
    fn check_bounds(lower_bound: &CursorBoundValue, upper_bound: &CursorBoundValue) -> CursorResult<()> {
        if lower_bound > upper_bound {
            let description = format!(
                "Incorrect usage of 'IdbSingleKeyBoundCursor': lower_bound '{lower_bound:?}' is expected to be less or equal to upper_bound '{upper_bound:?}'"
            );
            return MmError::err(CursorError::InvalidKeyRange { description });
        }
        if lower_bound == upper_bound {
            warn!("lower_bound '{lower_bound:?}' equals to upper_bound '{upper_bound:?}'. Consider using 'IdbObjectStoreImpl::get_items' instead");
        }
        Ok(())
    }
}

impl CursorDriverImpl for IdbSingleKeyBoundCursor {
    fn key_range(&self) -> CursorResult<Option<IdbKeyRange>> {
        let key_range = IdbKeyRange::bound(&self.lower_bound.to_js_value()?, &self.upper_bound.to_js_value()?)
            .map_to_mm(|e| CursorError::InvalidKeyRange {
                description: stringify_js_error(&e),
            })?;
        Ok(Some(key_range))
    }

    fn on_iteration(&mut self, _key: JsValue) -> CursorResult<(CursorItemAction, CursorAction)> {
        Ok((CursorItemAction::Include, CursorAction::Continue))
    }
}
