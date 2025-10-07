use super::{
    index_key_as_array, CursorAction, CursorBoundValue, CursorDriverImpl, CursorError, CursorItemAction, CursorResult,
};
use common::{deserialize_from_js, serialize_to_js, stringify_js_error};
use js_sys::Array;
use mm2_err_handle::prelude::*;
use serde_json::{json, Value as Json};
use wasm_bindgen::prelude::*;
use web_sys::IdbKeyRange;

/// The representation of a range that includes records
/// with the multiple `only` and `bound` restrictions.
/// https://developer.mozilla.org/en-US/docs/Web/API/IDBKeyRange/bound
pub struct IdbMultiKeyBoundCursor {
    only_values: Vec<(String, Json)>,
    bound_values: Vec<(String, CursorBoundValue, CursorBoundValue)>,
}

impl IdbMultiKeyBoundCursor {
    pub(super) fn new(
        only_values: Vec<(String, Json)>,
        bound_values: Vec<(String, CursorBoundValue, CursorBoundValue)>,
    ) -> CursorResult<IdbMultiKeyBoundCursor> {
        Self::check_bounds(&only_values, &bound_values)?;
        Ok(IdbMultiKeyBoundCursor {
            only_values,
            bound_values,
        })
    }

    fn check_bounds(
        only_values: &[(String, Json)],
        bound_values: &[(String, CursorBoundValue, CursorBoundValue)],
    ) -> CursorResult<()> {
        if bound_values.is_empty() || (only_values.len() + bound_values.len() < 2) {
            let description = format!(
                "Incorrect usage of 'IdbMultipleBoundCursor': expected more than one cursor bound, found '{}' only_values, '{}' bound_values",
                only_values.len(),
                bound_values.len()
            );
            return MmError::err(CursorError::IncorrectUsage { description });
        }
        for (_index, lower_bound, upper_bound) in bound_values {
            if !lower_bound.same_inner_type(upper_bound) {
                let description = format!(
                    "Expected same inner type of lower and upper bounds, found: {lower_bound:?}, {upper_bound:?}"
                );
                return MmError::err(CursorError::InvalidKeyRange { description });
            }
            if lower_bound > upper_bound {
                let description = format!(
                    "lower_bound '{lower_bound:?}' is expected to be less or equal to upper_bound '{upper_bound:?}'"
                );
                return MmError::err(CursorError::InvalidKeyRange { description });
            }
        }
        Ok(())
    }

    /// Fill the given `array` with the lower values of the corresponding key bounds
    /// starting from the `starting_from_idx` index of [`IdbMultiKeyBoundCursor::bound_values`].
    fn fill_array_with_lower_values_starting_from_idx(
        &self,
        array: &Array,
        starting_from_bound: usize,
    ) -> CursorResult<()> {
        let mut idx = starting_from_bound;
        while idx < self.bound_values.len() {
            let (_index, lower_value, _upper_value) = &self.bound_values[idx];
            array.push(&lower_value.to_js_value()?);
            idx += 1;
        }
        Ok(())
    }
}

impl CursorDriverImpl for IdbMultiKeyBoundCursor {
    fn key_range(&self) -> CursorResult<Option<IdbKeyRange>> {
        let lower = Array::new();
        let upper = Array::new();

        // first, push the `only` values
        for (field, value) in self.only_values.iter() {
            let js_value = serialize_to_js(value).map_to_mm(|e| CursorError::ErrorSerializingIndexFieldValue {
                field: field.to_owned(),
                value: format!("{value:?}"),
                description: e.to_string(),
            })?;
            lower.push(&js_value);
            upper.push(&js_value);
        }

        for (_index, lower_value, upper_value) in self.bound_values.iter() {
            let lower_js_value = lower_value.to_js_value()?;
            let upper_js_value = upper_value.to_js_value()?;

            lower.push(&lower_js_value);
            upper.push(&upper_js_value);
        }

        let key_range = IdbKeyRange::bound(&lower, &upper).map_to_mm(|e| CursorError::InvalidKeyRange {
            description: stringify_js_error(&e),
        })?;
        Ok(Some(key_range))
    }

    /// The range `IDBKeyRange.bound([2,2], [4,4])` includes values like `[3,0]` and `[3,5]` as `[2,2] < [3,0] < [3,5] < [4,4]`,
    /// so we need to do additional filtering.
    /// For more information on why it's required, see https://stackoverflow.com/a/32976384.
    fn on_iteration(&mut self, index_key: JsValue) -> CursorResult<(CursorItemAction, CursorAction)> {
        let index_keys_js_array = index_key_as_array(index_key)?;
        let index_keys: Vec<Json> = index_keys_js_array
            .iter()
            .map(|js_value| {
                deserialize_from_js(js_value).map_to_mm(|e| CursorError::ErrorDeserializingIndexValue {
                    description: e.to_string(),
                })
            }) // Item = Result<Json, MmError<CursorError>>
            .collect::<Result<Vec<Json>, MmError<CursorError>>>()?;

        let expected_keys = self.only_values.len() + self.bound_values.len();
        if index_keys.len() != expected_keys {
            return MmError::err(CursorError::IncorrectNumberOfKeysPerIndex {
                expected: expected_keys,
                found: index_keys.len(),
            });
        }

        // Since we've put `only_values` to the start of the lower and upper indexes, we have to check only `bound` values.
        let mut idx_in_index = self.only_values.len();
        let mut idx_in_bounds = 0;

        // The value consists of:
        // * idx of the last increased key in the `index_keys`;
        // * idx of the last increased key in the [`IdbMultiKeyBoundCursor::bound_values`];
        // * the last increased key.
        let mut last_increased_index_key: Option<(usize, usize, CursorBoundValue)> = None;

        while idx_in_bounds < self.bound_values.len() {
            let (_index, lower_bound, upper_bound) = &self.bound_values[idx_in_bounds];

            let actual_index_value = &index_keys[idx_in_index];
            // We expect that the lower and upper bounds have the same inner type as it's checked in [`IdbMultiKeyBoundCursor::check_bounds`].
            let actual_index_value = CursorBoundValue::deserialize_with_expected_type(actual_index_value, lower_bound)?;

            if &actual_index_value < lower_bound {
                // A case №1, similar to the following:
                // Got `[4, 1]`, but lower bound is `[3, 4]` and upper bound is `[5, 8]`.
                // Obviously, this index is out of the expected range, so we have to continue iterating
                // with an index `[4, 4]` to skip values like `[4, 2]`, `[4, 3]`.

                // It means, we have to continue iterating with a DB index, whose `[0, idx_in_index)` keys are the same as in `index_keys`,
                // but with the lowest keys starting from `idx_in_index`.
                let new_index = index_keys_js_array.slice(0, idx_in_index as u32);
                self.fill_array_with_lower_values_starting_from_idx(&new_index, idx_in_bounds)?;

                return Ok((
                    // the `actual_index_value` is not in our expected bounds
                    CursorItemAction::Skip,
                    CursorAction::ContinueWithValue(new_index.into()),
                ));
            }
            if &actual_index_value > upper_bound {
                // A case №2, similar to the following:
                // Got `[4, 6]`, but lower bound is `[3, 2]` and upper bound is `[5, 4]`.
                // Obviously, this index is out of the expected range, so we have to continue iterating
                // with an index `[5, 2]` to skip values like `[4, 7]`, `[4, 8]` etc.

                // It means, we have to continue iterating with a DB index that is greater than actual `index` but is in the given bounds.
                if let Some((last_increased_idx_in_index, last_increased_idx_in_bounds, last_increased_value)) =
                    last_increased_index_key
                {
                    let new_index = index_keys_js_array.slice(0, last_increased_idx_in_index as u32);
                    new_index.push(&last_increased_value.to_js_value()?);
                    self.fill_array_with_lower_values_starting_from_idx(&new_index, last_increased_idx_in_bounds + 1)?;

                    return Ok((
                        // the `actual_index_value` is not in our expected bounds
                        CursorItemAction::Skip,
                        CursorAction::ContinueWithValue(new_index.into()),
                    ));
                }

                // otherwise there is no an index greater than actual `index`, stop the cursor
                return Ok((CursorItemAction::Skip, CursorAction::Stop));
            }

            let increased_index_key = actual_index_value.next();
            if &increased_index_key <= upper_bound {
                last_increased_index_key = Some((idx_in_index, idx_in_bounds, increased_index_key));
            } // otherwise we don't need to remember this value that can't be increased

            idx_in_index += 1;
            idx_in_bounds += 1;
        }
        Ok((CursorItemAction::Include, CursorAction::Continue))
    }
}

mod tests {
    use super::*;
    use common::log::wasm_log::register_wasm_log;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    /// The given indexes are expected to be in the bound.
    fn test_in_bound_indexes(cursor: &mut IdbMultiKeyBoundCursor, input_indexes: Vec<Json>) {
        for input_index in input_indexes {
            let input_index_js_value = serialize_to_js(&input_index).unwrap();
            let result = cursor.on_iteration(input_index_js_value);
            assert_eq!(
                result,
                Ok((CursorItemAction::Include, CursorAction::Continue)),
                "'{input_index}' index is expected to be in a bound",
            );
        }
    }

    /// `input_indexes` consists of the pairs of:
    /// * Index that is out of bound;
    /// * Next index that is expected to be returned.
    fn test_out_of_bound_indexes_with_next(cursor: &mut IdbMultiKeyBoundCursor, input_indexes: Vec<(Json, Json)>) {
        for (input_index, expected_next) in input_indexes {
            let input_index_js_value = serialize_to_js(&input_index).unwrap();
            let (item_action, cursor_action) = cursor
                .on_iteration(input_index_js_value)
                .unwrap_or_else(|_| panic!("Error due to the index '{input_index:?}'"));

            let actual_next: Json = match cursor_action {
                CursorAction::ContinueWithValue(next_index_js_value) => {
                    deserialize_from_js(next_index_js_value).expect("Error deserializing next index}")
                },
                action => panic!("Expected 'CursorAction::ContinueWithValue', found '{action:?}'"),
            };
            assert_eq!(item_action, CursorItemAction::Skip);
            assert_eq!(actual_next, expected_next);
        }
    }

    // `input_indexes` are expected to be out of bound and can't be found next index to continue.
    fn test_out_of_bound_indexes(cursor: &mut IdbMultiKeyBoundCursor, input_indexes: Vec<Json>) {
        for input_index in input_indexes {
            let input_index_js_value = serialize_to_js(&input_index).unwrap();
            let result = cursor.on_iteration(input_index_js_value);
            assert_eq!(
                result,
                Ok((CursorItemAction::Skip, CursorAction::Stop)),
                "'{input_index}' index is expected to be out of bound",
            );
        }
    }

    /// This test doesn't check [`IdbMultiKeyBoundCursor::filter`].
    #[wasm_bindgen_test]
    fn test_on_iteration_multiple_only_and_bound_values() {
        register_wasm_log();

        let only_values = vec![
            ("field1".to_owned(), json!("value1")),
            ("field2".to_owned(), json!(2)),
            ("field3".to_owned(), json!("value 3")),
        ];
        let bound_values = vec![
            (
                "field4".to_owned(),
                CursorBoundValue::Uint(2), // lower bound
                CursorBoundValue::Uint(5), // upper bound
            ),
            (
                "field5".to_owned(),
                CursorBoundValue::Int(-10), // lower bound
                CursorBoundValue::Int(10),  // upper bound
            ),
            (
                "field6".to_owned(),
                CursorBoundValue::Uint(7), // lower bound
                CursorBoundValue::Uint(7), // upper bound
            ),
            (
                "field7".to_owned(),
                CursorBoundValue::Int(-1), // lower bound
                CursorBoundValue::Int(1),  // upper bound
            ),
        ];

        let mut cursor = IdbMultiKeyBoundCursor::new(only_values, bound_values).unwrap();

        //////////////////

        // The following inexes are expected to be in the bound.
        let in_bound_keys: Vec<_> = vec![
            json!(["value1", 2, "value 3", 2, 0, 7, 1]),
            json!(["value1", 2, "value 3", 3, 3, 7, -1]),
            json!(["value1", 2, "value 3", 3, 3, 7, 0]),
            json!(["value1", 2, "value 3", 5, -10, 7, 1]),
            json!(["value1", 2, "value 3", 5, 10, 7, 1]),
        ];
        test_in_bound_indexes(&mut cursor, in_bound_keys);

        //////////////////

        let out_of_bound_indexes_with_next: Vec<_> = vec![
            // Case №1
            (
                json!(["value1", 2, "value 3", 2, 0, 7, -2]),
                json!(["value1", 2, "value 3", 2, 0, 7, -1]),
            ),
            // Case №1
            (
                json!(["value1", 2, "value 3", 2, 0, 6, 1]),
                json!(["value1", 2, "value 3", 2, 0, 7, -1]),
            ),
            // Case №1
            (
                json!(["value1", 2, "value 3", 1, 10, 7, 1]),
                json!(["value1", 2, "value 3", 2, -10, 7, -1]),
            ),
            // Case №1
            (
                json!(["value1", 2, "value 3", 3, -100, 5, 100]),
                json!(["value1", 2, "value 3", 3, -10, 7, -1]),
            ),
            // Case №1
            (
                json!(["value1", 2, "value 3", 2, -10, 7, -2]),
                json!(["value1", 2, "value 3", 2, -10, 7, -1]),
            ),
            // Case №2
            (
                json!(["value1", 2, "value 3", 3, 0, 8, 1]),
                json!(["value1", 2, "value 3", 3, 1, 7, -1]),
            ),
            // Case №2
            (
                json!(["value1", 2, "value 3", 3, 10, 8, 1]),
                json!(["value1", 2, "value 3", 4, -10, 7, -1]),
            ),
            // Case №2
            (
                json!(["value1", 2, "value 3", 3, 10, 7, 2]),
                json!(["value1", 2, "value 3", 4, -10, 7, -1]),
            ),
        ];
        test_out_of_bound_indexes_with_next(&mut cursor, out_of_bound_indexes_with_next);

        //////////////////

        let out_of_bound_indexes: Vec<_> = vec![
            json!(["value1", 2, "value 3", 5, 10, 7, 2]),
            json!(["value1", 2, "value 3", 5, 10, 8, 1]),
            json!(["value1", 2, "value 3", 5, 11, 7, 1]),
            json!(["value1", 2, "value 3", 6, 10, 7, 1]),
            json!(["value1", 2, "value 3", 6, -10, 7, 0]),
        ];
        test_out_of_bound_indexes(&mut cursor, out_of_bound_indexes);
    }

    /// This test doesn't check [`IdbMultiKeyBoundCursor::filter`].
    #[wasm_bindgen_test]
    fn test_on_iteration_multiple_bound_values() {
        register_wasm_log();

        let only_values = Vec::new();
        let bound_values = vec![
            (
                "field1".to_owned(),
                CursorBoundValue::Uint(2), // lower bound
                CursorBoundValue::Uint(5), // upper bound
            ),
            (
                "field2".to_owned(),
                CursorBoundValue::Int(-10), // lower bound
                CursorBoundValue::Int(10),  // upper bound
            ),
        ];

        let mut cursor = IdbMultiKeyBoundCursor::new(only_values, bound_values).unwrap();

        //////////////////

        // The following inexes are expected to be in the bound.
        let in_bound_keys: Vec<_> = vec![
            json!([2, -10]),
            json!([2, 0]),
            json!([2, 10]),
            json!([5, -10]),
            json!([5, 10]),
        ];
        test_in_bound_indexes(&mut cursor, in_bound_keys);

        //////////////////

        let out_of_bound_indexes_with_next: Vec<_> = vec![
            // Case №1
            (json!([2, -100]), json!([2, -10])),
            // Case №1
            (json!([1, 10]), json!([2, -10])),
            // Case №1
            (json!([1, -100]), json!([2, -10])),
            // Case №1
            (json!([5, -11]), json!([5, -10])),
            // Case №2
            (json!([2, 11]), json!([3, -10])),
            // Case №2
            (json!([4, 101]), json!([5, -10])),
        ];
        test_out_of_bound_indexes_with_next(&mut cursor, out_of_bound_indexes_with_next);

        //////////////////

        let out_of_bound_indexes: Vec<_> = vec![
            //
            json!([5, 11]),
            json!([6, 10]),
            json!([6, -10]),
        ];
        test_out_of_bound_indexes(&mut cursor, out_of_bound_indexes);
    }

    /// This test doesn't check [`IdbMultiKeyBoundCursor::filter`].
    #[wasm_bindgen_test]
    fn test_on_iteration_single_only_and_bound_values() {
        register_wasm_log();

        let only_values = vec![("field1".to_owned(), json!(2))];
        let bound_values = vec![(
            "field2".to_owned(),
            CursorBoundValue::Uint(2), // lower bound
            CursorBoundValue::Uint(5), // upper bound
        )];

        let mut cursor = IdbMultiKeyBoundCursor::new(only_values, bound_values).unwrap();

        //////////////////

        // The following inexes are expected to be in the bound.
        let in_bound_keys: Vec<_> = vec![
            //
            json!([2, 2]),
            json!([2, 3]),
            json!([2, 4]),
            json!([2, 5]),
        ];
        test_in_bound_indexes(&mut cursor, in_bound_keys);

        //////////////////

        let out_of_bound_indexes_with_next: Vec<_> = vec![
            // Case №1
            (json!([2, 0]), json!([2, 2])),
            // Case №1
            (json!([2, 1]), json!([2, 2])),
        ];
        test_out_of_bound_indexes_with_next(&mut cursor, out_of_bound_indexes_with_next);

        //////////////////

        let out_of_bound_indexes: Vec<_> = vec![
            //
            json!([2, 6]),
            json!([6, 7]),
            json!([6, 100]),
        ];
        test_out_of_bound_indexes(&mut cursor, out_of_bound_indexes);
    }

    #[wasm_bindgen_test]
    fn test_on_iteration_error() {
        register_wasm_log();

        let only_values = vec![("field1".to_owned(), json!(2u32))];
        let bound_values = vec![(
            "field2".to_owned(),
            CursorBoundValue::Uint(2), // lower bound
            CursorBoundValue::Uint(5), // upper bound
        )];

        let mut cursor = IdbMultiKeyBoundCursor::new(only_values, bound_values).unwrap();

        //////////////////

        // The following indexes must lead to the `CursorError::TypeMismatch` error.
        let input_indexes: Vec<_> = vec![
            // the second field is expected to be `i32`, but it's `u32`
            (json!([2, -10]), "u32"),
            (json!([2, "a string value"]), "u32"),
            // the indexes are expected to be arrays
            (json!(2), "js_sys::Array"),
            (json!("Foo"), "js_sys::Array"),
        ];
        for (input_index, expected_type) in input_indexes {
            let input_index_js_value = serialize_to_js(&input_index).unwrap();
            let error = cursor
                .on_iteration(input_index_js_value)
                .expect_err(&format!("'{input_index:?}' must lead to 'CursorError::TypeMismatch'"));
            match error.into_inner() {
                CursorError::TypeMismatch { expected, .. } => assert_eq!(expected, expected_type),
                e => panic!("Expected 'CursorError::TypeMismatch', found '{e:?}'"),
            }
        }

        // The following indexes have unexpected size.
        let input_indexes: Vec<_> = vec![
            //
            json!([2]),
            json!([2, 10, 1]),
            json!([2, 10, 1, 2]),
        ];
        for input_index in input_indexes {
            let input_index_js_value = serialize_to_js(&input_index).unwrap();
            let error = cursor
                .on_iteration(input_index_js_value)
                .expect_err(&format!("'{input_index:?}' must lead to 'CursorError::TypeMismatch'"));
            match error.into_inner() {
                CursorError::IncorrectNumberOfKeysPerIndex { .. } => (),
                e => panic!("Expected 'CursorError::IncorrectNumberOfKeysPerIndex', found '{e:?}'"),
            }
        }
    }
}
