//! # Usage
//!
//! As an example, the following table will be used:
//!
//! | uuid                                   | base_coin | rel_coin | base_coin_value | started_at |
//! | "c52659d7-4e13-41f5-9c1a-30cc2f646033" | "RICK"    | "MORTY"  | 10              | 1000000029 |
//! | "5acb0e63-8b26-469e-81df-7dd9e4a9ad15" | "RICK"    | "MORTY"  | 13              | 1000000030 |
//! | "9db641f5-4300-4527-9fa6-f1c391d42c35" | "RICK"    | "MORTY"  | 1.2             | 1000000031 |
//!
//! with the `search_index` index created by:
//! ```rust
//! TableUpgrader::create_multi_index(self, "search_index", &["base_coin", "rel_coin", "started_at"]).unwrap();
//! ```
//!
//! If you want to find all `RICK/MORTY` swaps where
//! 1) `10 <= base_coin_value <= 13`
//! 2) `started_at <= 1000000030`, you can use [`WithBound::bound`] along with [`WithOnly::only`]:
//! ```rust
//! let table = open_table_somehow();
//! let all_rick_morty_swaps = table
//!     .cursor_builder()
//!     .only("base_coin", "RICK", "MORTY")?
//!     .bound("base_coin_value", 10, 13)
//!     .bound("started_at", 1000000030.into(), u32::MAX.into())
//!     .open_cursor("search_index")
//!     .await?
//!     .collect()
//!     .await?;
//! ```
//!
//! # Under the hood
//!
//! In the example above, [`CursorOps::collect`] actually creates a JavaScript cursor with the specified key range:
//! ```js
//! var key_range = IDBKeyRange.bound(['RICK', 'MORTY', 10, 1000000030], ['RICK', 'MORTY', 13, 9999999999]);
//! var cursor = table.index('search_index').openCursor(key_range);
//! ```
//!
//! And after that, the database engine compares each record with the specified min and max bounds sequentially from one field to another.
//! Please note `['RICK', 'MORTY', 10, 1000000029]` <= `['RICK', 'MORTY', 11, 2000000000]`.
//!
//! # Important
//!
//! Please make sure all keys of the index are specified
//! by the [`IdbBoundCursorBuilder::only`] or/and [`IdbBoundCursorBuilder::bound`] methods,
//! and they are specified in the same order as they were declared on [`TableUpgrader::create_multi_index`].
//!
//! It's important because if you skip f the `started_at` key, for example, the bounds will be:
//! min = ['RICK', 'MORTY', 10], max = ['RICK', 'MORTY', 13],
//! but an actual record ['RICK', 'MORTY', 13, 1000000030] will not be included in the result,
//! because ['RICK', 'MORTY', 13] < ['RICK', 'MORTY', 13, 1000000030],
//! although it is expected to be within the specified bounds.

use crate::indexed_db::db_driver::cursor::CursorBoundValue;
pub(crate) use crate::indexed_db::db_driver::cursor::{CursorDriver, CursorFilters};
pub use crate::indexed_db::db_driver::cursor::{CursorError, CursorFiltersExt, CursorResult};
use crate::indexed_db::{DbTable, ItemId, TableSignature};
use futures::channel::{mpsc, oneshot};
use futures::{SinkExt, StreamExt};
use mm2_err_handle::prelude::*;
use serde::Serialize;
use serde_json::{self as json, Value as Json};
use std::fmt;
use std::marker::PhantomData;

pub(super) type DbCursorEventTx = mpsc::UnboundedSender<DbCursorEvent>;
pub(super) type DbCursorEventRx = mpsc::UnboundedReceiver<DbCursorEvent>;

pub struct CursorBuilder<'transaction, 'reference, Table: TableSignature> {
    db_table: &'reference DbTable<'transaction, Table>,
    filters: CursorFilters,
    filters_ext: CursorFiltersExt,
}

impl<'transaction, 'reference, Table: TableSignature> CursorBuilder<'transaction, 'reference, Table> {
    pub(crate) fn new(db_table: &'reference DbTable<'transaction, Table>) -> Self {
        CursorBuilder {
            db_table,
            filters: CursorFilters::default(),
            filters_ext: CursorFiltersExt::default(),
        }
    }

    pub fn only<Value>(mut self, field_name: &str, field_value: Value) -> CursorResult<Self>
    where
        Value: Serialize + fmt::Debug,
    {
        let field_value_str = format!("{field_value:?}");
        let field_value = json::to_value(field_value).map_to_mm(|e| CursorError::ErrorSerializingIndexFieldValue {
            field: field_name.to_owned(),
            value: field_value_str,
            description: e.to_string(),
        })?;

        self.filters.only_keys.push((field_name.to_owned(), field_value));
        Ok(self)
    }

    pub fn bound<Value>(mut self, field_name: &str, lower_bound: Value, upper_bound: Value) -> Self
    where
        CursorBoundValue: From<Value>,
    {
        let lower_bound = CursorBoundValue::from(lower_bound);
        let upper_bound = CursorBoundValue::from(upper_bound);
        self.filters
            .bound_keys
            .push((field_name.to_owned(), lower_bound, upper_bound));
        self
    }

    pub fn reverse(mut self) -> Self {
        self.filters.reverse = true;
        self
    }

    /// Sets a filtering condition for the cursor using the provided closure (`f`).
    /// The closure should take a reference to a value and return a boolean indicating whether the
    /// cursor should return this item or none if not found in the store.
    /// ```rust
    /// let cursor_builder = CursorBuilder::new();
    ///
    /// // Define a closure to filter items based on a condition
    /// let condition = |item: Json| -> CursorResult<bool> {
    ///     // Replace this with your actual condition logic
    ///     Ok(item.get("property").is_some())
    /// };
    ///
    /// // Apply the closure to the cursor builder using the where_ method
    /// let updated_cursor_builder = cursor_builder.where_(condition);
    /// ```
    pub fn where_<F>(mut self, f: F) -> CursorBuilder<'transaction, 'reference, Table>
    where
        F: Fn(Json) -> CursorResult<bool> + Send + 'static,
    {
        self.filters_ext.where_ = Some(Box::new(f));
        self
    }

    /// ```rust
    /// let cursor_builder = CursorBuilder::new();
    /// // Apply the default condition to the cursor builder to return the first item
    /// let updated_cursor_builder = cursor_builder.where_first().open_cursor().next();
    /// ```
    pub fn where_first(self) -> CursorBuilder<'transaction, 'reference, Table> {
        self.where_(|_| Ok(true))
    }

    pub fn limit(mut self, limit: usize) -> CursorBuilder<'transaction, 'reference, Table> {
        if limit < 1 {
            return self;
        };

        self.filters_ext.limit = Some(limit);
        self
    }

    pub fn offset(mut self, offset: u32) -> CursorBuilder<'transaction, 'reference, Table> {
        if offset < 1 {
            return self;
        };

        self.filters_ext.offset = Some(offset);
        self
    }

    /// Opens a cursor by the specified `index`.
    /// https://developer.mozilla.org/en-US/docs/Web/API/IDBObjectStore/openCursor
    pub async fn open_cursor(self, index: &str) -> CursorResult<CursorIter<'transaction, Table>> {
        let event_tx = self
            .db_table
            .open_cursor(index, self.filters, self.filters_ext)
            .await
            .mm_err(|e| CursorError::ErrorOpeningCursor {
                description: e.to_string(),
            })?;
        Ok(CursorIter {
            event_tx,
            phantom: PhantomData,
        })
    }
}

pub struct CursorIter<'transaction, Table> {
    event_tx: DbCursorEventTx,
    phantom: PhantomData<&'transaction Table>,
}

impl<Table: TableSignature> CursorIter<'_, Table> {
    /// Advances the iterator and returns the next value.
    /// Please note that the items are sorted by the index keys.
    pub async fn next(&mut self) -> CursorResult<Option<(ItemId, Table)>> {
        let (result_tx, result_rx) = oneshot::channel();
        self.event_tx
            .send(DbCursorEvent::NextItem { result_tx })
            .await
            .map_to_mm(|e| CursorError::UnexpectedState(format!("Error sending cursor event: {e}")))?;
        let maybe_item = result_rx
            .await
            .map_to_mm(|e| CursorError::UnexpectedState(format!("Error receiving cursor item: {e}")))??;
        let (item_id, item) = match maybe_item {
            Some((item_id, item)) => (item_id, item),
            None => return Ok(None),
        };
        let item = json::from_value(item).map_to_mm(|e| CursorError::ErrorDeserializingItem(e.to_string()))?;
        Ok(Some((item_id, item)))
    }

    pub async fn collect(mut self) -> CursorResult<Vec<(ItemId, Table)>> {
        let mut result = Vec::new();
        while let Some((item_id, item)) = self.next().await? {
            result.push((item_id, item));
        }
        Ok(result)
    }
}

pub enum DbCursorEvent {
    NextItem {
        result_tx: oneshot::Sender<CursorResult<Option<(ItemId, Json)>>>,
    },
}

pub(crate) async fn cursor_event_loop(mut rx: DbCursorEventRx, mut cursor: CursorDriver) {
    while let Some(event) = rx.next().await {
        match event {
            DbCursorEvent::NextItem { result_tx } => {
                result_tx.send(cursor.next().await).ok();
            },
        }
    }
}

mod tests {
    use super::*;
    use crate::indexed_db::{BeBigUint, DbIdentifier, DbTable, DbUpgrader, IndexedDbBuilder, OnUpgradeResult};
    use common::log::wasm_log::register_wasm_log;
    use itertools::Itertools;
    use serde::{Deserialize, Serialize};
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    macro_rules! swap_item {
        ($uuid:literal, $base_coin:literal, $rel_coin:literal, $base_coin_value:expr, $rel_coin_value:expr, $started_at:expr) => {
            SwapTable {
                uuid: $uuid.to_owned(),
                base_coin: $base_coin.to_owned(),
                rel_coin: $rel_coin.to_owned(),
                base_coin_value: $base_coin_value,
                rel_coin_value: $rel_coin_value,
                started_at: $started_at,
            }
        };
    }

    #[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
    #[serde(deny_unknown_fields)]
    struct SwapTable {
        uuid: String,
        base_coin: String,
        rel_coin: String,
        base_coin_value: u32,
        rel_coin_value: u32,
        started_at: i32,
    }

    impl TableSignature for SwapTable {
        const TABLE_NAME: &'static str = "swap_test_table";

        fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, _new_version: u32) -> OnUpgradeResult<()> {
            if old_version > 0 {
                // the table is initialized already
                return Ok(());
            }
            let table_upgrader = upgrader.create_table("swap_test_table")?;
            table_upgrader.create_index("base_coin", false)?;
            table_upgrader.create_index("rel_coin_value", false)?;
            table_upgrader.create_multi_index(
                "all_fields_index",
                &[
                    "base_coin",
                    "rel_coin",
                    "base_coin_value",
                    "rel_coin_value",
                    "started_at",
                ],
                false,
            )?;
            table_upgrader.create_multi_index(
                "basecoin_basecoinvalue_startedat_index",
                &["base_coin", "base_coin_value", "started_at"],
                false,
            )
        }
    }

    async fn fill_table<Table>(table: &DbTable<'_, Table>, items: &Vec<Table>)
    where
        Table: TableSignature + std::fmt::Debug,
    {
        for item in items {
            table
                .add_item(item)
                .await
                .unwrap_or_else(|_| panic!("Error adding {item:?} item"));
        }
    }

    async fn next_item<Table: TableSignature>(cursor_iter: &mut CursorIter<'_, Table>) -> Option<Table> {
        cursor_iter
            .next()
            .await
            .expect("!CursorIter::next")
            .map(|(_item_id, item)| item)
    }

    /// The table with `BeBigUint` parameters.
    #[derive(Clone, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
    #[serde(deny_unknown_fields)]
    struct TimestampTable {
        timestamp_x: BeBigUint,
        timestamp_y: u32,
        timestamp_z: BeBigUint,
    }

    impl TimestampTable {
        fn new<X, Z>(timestamp_x: X, timestamp_y: u32, timestamp_z: Z) -> TimestampTable
        where
            BeBigUint: From<X>,
            BeBigUint: From<Z>,
        {
            TimestampTable {
                timestamp_x: BeBigUint::from(timestamp_x),
                timestamp_y,
                timestamp_z: BeBigUint::from(timestamp_z),
            }
        }
    }

    impl TableSignature for TimestampTable {
        const TABLE_NAME: &'static str = "timestamp_table";

        fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, _new_version: u32) -> OnUpgradeResult<()> {
            if old_version > 0 {
                // the table is initialized already
                return Ok(());
            }
            let table_upgrader = upgrader.create_table("timestamp_table")?;
            table_upgrader.create_index("timestamp_x", false)?;
            table_upgrader.create_multi_index("timestamp_xyz", &["timestamp_x", "timestamp_y", "timestamp_z"], false)
        }
    }

    /// Test if `BeBigUint` works properly as an `IndexedDb` index.
    #[wasm_bindgen_test]
    async fn test_be_big_uint_index() {
        const DB_NAME: &str = "TEST_BE_BIG_UINT_INDEX";
        const DB_VERSION: u32 = 1;

        let numbers: Vec<BeBigUint> = vec![
            0u32.into(),
            1u32.into(),
            2u32.into(),
            BeBigUint::from(u8::MAX - 1),
            BeBigUint::from(u8::MAX),
            BeBigUint::from(u8::MAX) + 1u64,
            BeBigUint::from(u16::MAX - 1),
            BeBigUint::from(u16::MAX),
            BeBigUint::from(u16::MAX) + 1u64,
            BeBigUint::from(u32::MAX - 1),
            BeBigUint::from(u32::MAX),
            BeBigUint::from(u32::MAX) + 1u64,
            BeBigUint::from(u64::MAX - 1),
            BeBigUint::from(u64::MAX),
            BeBigUint::from(u64::MAX) + 1u64,
            BeBigUint::from(u128::MAX - 1),
            BeBigUint::from(u128::MAX),
            BeBigUint::from(u128::MAX) + 1u64,
        ];

        // Convert `numbers` into `Vec<TimeStampTable>`.
        let items = numbers
            .iter()
            .cloned()
            .map(|timestamp_x| TimestampTable {
                timestamp_x,
                ..TimestampTable::default()
            })
            .collect();

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<TimestampTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction");
        let table = transaction
            .table::<TimestampTable>()
            .await
            .expect("!DbTransaction::open_table");
        fill_table(&table, &items).await;

        // Test the cursor index for each combination of numbers (lower, upper).
        for num_x in numbers.iter() {
            for num_y in numbers.iter() {
                if num_x > num_y {
                    continue;
                }

                // Get every item that satisfies the following [num_x, num_y] bound.
                let actual_items = table
                    .cursor_builder()
                    .bound("timestamp_x", num_x.clone(), num_y.clone())
                    .open_cursor("timestamp_x")
                    .await
                    .expect("!CursorBuilder::open_cursor")
                    .collect()
                    .await
                    .expect("!CursorIter::collect")
                    .into_iter()
                    // Map `(ItemId, TimestampTable)` into `BeBigUint`.
                    .map(|(_item_id, item)| item.timestamp_x)
                    .sorted()
                    .collect::<Vec<_>>();
                // Get `BeBigUint` numbers that should have been returned by the cursor above.
                let expected = numbers
                    .iter()
                    .filter(|&num| num_x <= num && num <= num_y)
                    .cloned()
                    .sorted()
                    .collect::<Vec<_>>();
                assert_eq!(actual_items, expected);
            }
        }
    }

    #[wasm_bindgen_test]
    async fn test_collect_single_key_cursor() {
        const DB_NAME: &str = "TEST_COLLECT_SINGLE_KEY_CURSOR";
        const DB_VERSION: u32 = 1;

        let items = vec![
            swap_item!("uuid1", "RICK", "MORTY", 10, 1, 700), // +
            swap_item!("uuid2", "MORTY", "KMD", 95000, 1, 721),
            swap_item!("uuid3", "RICK", "XYZ", 7, 6, 721),   // +
            swap_item!("uuid4", "RICK", "MORTY", 8, 6, 721), // +
            swap_item!("uuid5", "KMD", "MORTY", 12, 3, 721),
            swap_item!("uuid6", "QRC20", "RICK", 2, 2, 721),
        ];

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<SwapTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction");
        let table = transaction
            .table::<SwapTable>()
            .await
            .expect("!DbTransaction::open_table");
        fill_table(&table, &items).await;

        let mut actual_items = table
            .cursor_builder()
            .only("base_coin", "RICK")
            .expect("!CursorBuilder::only")
            .open_cursor("base_coin")
            .await
            .expect("!CursorBuilder::open_cursor")
            .collect()
            .await
            .expect("!CursorIter::collect")
            .into_iter()
            .map(|(_item_id, item)| item)
            .collect::<Vec<_>>();
        actual_items.sort();

        let mut expected_items = vec![
            swap_item!("uuid1", "RICK", "MORTY", 10, 1, 700),
            swap_item!("uuid3", "RICK", "XYZ", 7, 6, 721),
            swap_item!("uuid4", "RICK", "MORTY", 8, 6, 721),
        ];
        expected_items.sort();

        assert_eq!(actual_items, expected_items);
    }

    #[wasm_bindgen_test]
    async fn test_collect_single_key_bound_cursor() {
        const DB_NAME: &str = "TEST_COLLECT_SINGLE_KEY_BOUND_CURSOR";
        const DB_VERSION: u32 = 1;

        let items = vec![
            swap_item!("uuid1", "RICK", "MORTY", 10, 3, 700),
            swap_item!("uuid2", "MORTY", "KMD", 95000, 1, 721),
            swap_item!("uuid3", "RICK", "XYZ", 7, u32::MAX, 1281), // +
            swap_item!("uuid4", "RICK", "MORTY", 8, 6, 92),        // +
            swap_item!("uuid5", "QRC20", "RICK", 2, 4, 721),
            swap_item!("uuid6", "KMD", "MORTY", 12, 3124, 214), // +
        ];

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<SwapTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction");
        let table = transaction
            .table::<SwapTable>()
            .await
            .expect("!DbTransaction::open_table");
        fill_table(&table, &items).await;

        let mut actual_items = table
            .cursor_builder()
            .bound("rel_coin_value", 5u32, u32::MAX)
            .open_cursor("rel_coin_value")
            .await
            .expect("!CursorBuilder::open_cursor")
            .collect()
            .await
            .expect("!CursorIter::collect")
            .into_iter()
            .map(|(_item_id, item)| item)
            .collect::<Vec<_>>();
        actual_items.sort();

        let mut expected_items = vec![
            swap_item!("uuid3", "RICK", "XYZ", 7, u32::MAX, 1281),
            swap_item!("uuid4", "RICK", "MORTY", 8, 6, 92),
            swap_item!("uuid6", "KMD", "MORTY", 12, 3124, 214),
        ];
        expected_items.sort();

        assert_eq!(actual_items, expected_items);
    }

    #[wasm_bindgen_test]
    async fn test_collect_multi_key_cursor() {
        const DB_NAME: &str = "TEST_COLLECT_MULTI_KEY_CURSOR";
        const DB_VERSION: u32 = 1;

        let items = vec![
            swap_item!("uuid1", "RICK", "MORTY", 12, 1, 700),
            swap_item!("uuid2", "RICK", "KMD", 95000, 6, 721),
            swap_item!("uuid3", "RICK", "MORTY", 12, 5, 720),
            swap_item!("uuid4", "RICK", "MORTY", 12, 3, 721), // +
            swap_item!("uuid5", "QRC20", "MORTY", 51, 221, 182),
            swap_item!("uuid6", "QRC20", "RICK", 12, 6, 121),
            swap_item!("uuid7", "RICK", "QRC20", 12, 6, 721), // +
            swap_item!("uuid8", "FIRO", "DOGE", 12, 8, 721),
            swap_item!("uuid9", "RICK", "DOGE", 115, 1221, 721),
            swap_item!("uuid10", "RICK", "tQTUM", 12, 6, 721), // +
            swap_item!("uuid11", "MORTY", "RICK", 12, 7, 677),
            swap_item!("uuid12", "tBTC", "RICK", 92, 6, 721),
        ];

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<SwapTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction");
        let table = transaction
            .table::<SwapTable>()
            .await
            .expect("!DbTransaction::open_table");
        fill_table(&table, &items).await;

        let mut actual_items = table
            .cursor_builder()
            .only("base_coin", "RICK")
            .expect("!CursorBuilder::only")
            .only("base_coin_value", 12)
            .expect("!CursorBuilder::only")
            .only("started_at", 721)
            .expect("!CursorBuilder::only")
            .open_cursor("basecoin_basecoinvalue_startedat_index")
            .await
            .expect("!CursorBuilder::open_cursor")
            .collect()
            .await
            .expect("!CursorIter::collect")
            .into_iter()
            .map(|(_item_id, item)| item)
            .collect::<Vec<_>>();
        actual_items.sort();

        let mut expected_items = vec![
            swap_item!("uuid4", "RICK", "MORTY", 12, 3, 721),
            swap_item!("uuid7", "RICK", "QRC20", 12, 6, 721),
            swap_item!("uuid10", "RICK", "tQTUM", 12, 6, 721),
        ];
        expected_items.sort();

        assert_eq!(actual_items, expected_items);
    }

    #[wasm_bindgen_test]
    async fn test_collect_multi_key_bound_cursor() {
        const DB_NAME: &str = "TEST_COLLECT_MULTI_KEY_BOUND_CURSOR";
        const DB_VERSION: u32 = 1;

        let items = vec![
            swap_item!("uuid1", "MORTY", "RICK", 12, 10, 999),
            swap_item!("uuid2", "RICK", "QRC20", 4, 12, 557),
            swap_item!("uuid3", "RICK", "QRC20", 8, 11, 795), // +
            swap_item!("uuid4", "MORTY", "QRC20", 2, 10, 596),
            swap_item!("uuid5", "tQTUM", "MORTY", 1, 8, 709),
            swap_item!("uuid6", "tQTUM", "RICK", 5, 90, 555),
            swap_item!("uuid7", "RICK", "QRC20", 66, 88, 744),
            swap_item!("uuid8", "DOGE", "DOGE", 5, 12, 714),
            swap_item!("uuid9", "RICK", "QRC20", 7, 10, 743), // +
            swap_item!("uuid10", "FIRO", "tQTUM", 7, 11, 777),
            swap_item!("uuid11", "RICK", "MORTY", 91, 11, 1061),
            swap_item!("uuid12", "tBTC", "tQTUM", 4, 771, 745),
            swap_item!("uuid13", "RICK", "QRC20", 3, 11, 759), // +
            swap_item!("uuid14", "DOGE", "tBTC", 4, 6, 895),
            swap_item!("uuid15", "RICK", "QRC20", 723, 19, 558),
            swap_item!("uuid16", "FIRO", "tBTC", 5, 10, 724),
            swap_item!("uuid17", "RICK", "tBTC", 5, 13, 636),
            swap_item!("uuid18", "RICK", "QRC20", 7, 33, 864),
            swap_item!("uuid19", "DOGE", "tBTC", 55, 12, 723),
            swap_item!("uuid20", "RICK", "QRC20", 5, 11, 785), // +
            swap_item!("uuid21", "FIRO", "tBTC", 24, 1, 605),
            swap_item!("uuid22", "RICK", "QRC20", 9, 10, 734),
            swap_item!("uuid23", "tBTC", "tBTC", 7, 99, 834),
            swap_item!("uuid24", "RICK", "QRC20", 8, 12, 849),
            swap_item!("uuid25", "DOGE", "tBTC", 9, 10, 711),
        ];

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<SwapTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction");
        let table = transaction
            .table::<SwapTable>()
            .await
            .expect("!DbTransaction::open_table");
        fill_table(&table, &items).await;

        let actual_items = table
            .cursor_builder()
            .only("base_coin", "RICK")
            .expect("!CursorBuilder::only")
            .only("rel_coin", "QRC20")
            .expect("!CursorBuilder::only")
            .bound("base_coin_value", 3u32, 8u32)
            .bound("rel_coin_value", 10u32, 12u32)
            .bound("started_at", 600i32, 800i32)
            .open_cursor("all_fields_index")
            .await
            .expect("!CursorBuilder::open_cursor")
            .collect()
            .await
            .expect("!CursorIter::collect")
            .into_iter()
            .map(|(_item_id, item)| item)
            .collect::<Vec<_>>();

        // Items are expected to be sorted in the following order.
        let expected_items = vec![
            swap_item!("uuid13", "RICK", "QRC20", 3, 11, 759),
            swap_item!("uuid20", "RICK", "QRC20", 5, 11, 785),
            swap_item!("uuid9", "RICK", "QRC20", 7, 10, 743),
            swap_item!("uuid3", "RICK", "QRC20", 8, 11, 795),
        ];

        assert_eq!(actual_items, expected_items);
    }

    #[wasm_bindgen_test]
    async fn test_collect_multi_key_bound_cursor_big_int() {
        const DB_NAME: &str = "TEST_COLLECT_MULTI_KEY_BOUND_CURSOR_BIG_INT";
        const DB_VERSION: u32 = 1;

        let items = vec![
            TimestampTable::new(u64::MAX, 6, u128::MAX - 3),
            TimestampTable::new(u64::MAX - 1, 0, u128::MAX - 2), // +
            TimestampTable::new(u64::MAX - 2, 1, u128::MAX / 2),
            TimestampTable::new(u128::MAX / 2, 3, 4u64),
            TimestampTable::new(u64::MAX - 1, 1, u128::MAX / 2), // +
            TimestampTable::new(u128::MAX, 5, u64::MAX),         // +
            TimestampTable::new(u64::MAX - 1, 0, u64::MAX - 1),
            TimestampTable::new(u128::MAX, 2, u64::MAX as u128 + 1), // +
        ];

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<TimestampTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction");
        let table = transaction
            .table::<TimestampTable>()
            .await
            .expect("!DbTransaction::open_table");
        fill_table(&table, &items).await;

        let actual_items = table
            .cursor_builder()
            .bound("timestamp_x", BeBigUint::from(u64::MAX - 1), BeBigUint::from(u128::MAX))
            .bound("timestamp_y", 0u32, 5u32)
            .bound("timestamp_z", BeBigUint::from(u64::MAX), BeBigUint::from(u128::MAX - 2))
            .open_cursor("timestamp_xyz")
            .await
            .expect("!CursorBuilder::open_cursor")
            .collect()
            .await
            .expect("!CursorIter::collect")
            .into_iter()
            .map(|(_item_id, item)| item)
            .collect::<Vec<_>>();

        // Items are expected to be sorted in the following order.
        let expected_items = vec![
            TimestampTable::new(u64::MAX - 1, 0, u128::MAX - 2),
            TimestampTable::new(u64::MAX - 1, 1, u128::MAX / 2),
            TimestampTable::new(u128::MAX, 2, u64::MAX as u128 + 1),
            TimestampTable::new(u128::MAX, 5, u64::MAX),
        ];

        assert_eq!(actual_items, expected_items);
    }

    #[wasm_bindgen_test]
    async fn test_iter_without_constraints() {
        const DB_NAME: &str = "TEST_ITER_WITHOUT_CONSTRAINTS";
        const DB_VERSION: u32 = 1;

        let items = vec![
            swap_item!("uuid1", "RICK", "MORTY", 10, 3, 700),
            swap_item!("uuid2", "MORTY", "KMD", 95000, 1, 721),
            swap_item!("uuid3", "RICK", "XYZ", 7, u32::MAX, 1281),
            swap_item!("uuid4", "RICK", "MORTY", 8, 6, 92),
        ];

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<SwapTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction");
        let table = transaction
            .table::<SwapTable>()
            .await
            .expect("!DbTransaction::open_table");
        fill_table(&table, &items).await;

        let mut cursor_iter = table
            .cursor_builder()
            .open_cursor("rel_coin_value")
            .await
            .expect("!CursorBuilder::open_cursor");

        // The items must be sorted by `rel_coin_value`.
        assert_eq!(
            next_item(&mut cursor_iter).await,
            Some(swap_item!("uuid2", "MORTY", "KMD", 95000, 1, 721))
        );
        assert_eq!(
            next_item(&mut cursor_iter).await,
            Some(swap_item!("uuid1", "RICK", "MORTY", 10, 3, 700))
        );
        assert_eq!(
            next_item(&mut cursor_iter).await,
            Some(swap_item!("uuid4", "RICK", "MORTY", 8, 6, 92))
        );
        assert_eq!(
            next_item(&mut cursor_iter).await,
            Some(swap_item!("uuid3", "RICK", "XYZ", 7, u32::MAX, 1281))
        );
        assert!(next_item(&mut cursor_iter).await.is_none());
        // Try to poll one more time. This should not fail but return `None`.
        assert!(next_item(&mut cursor_iter).await.is_none());
    }

    #[wasm_bindgen_test]
    async fn test_rev_iter_without_constraints() {
        const DB_NAME: &str = "TEST_REV_ITER_WITHOUT_CONSTRAINTS";
        const DB_VERSION: u32 = 1;

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<SwapTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction");
        let table = transaction
            .table::<SwapTable>()
            .await
            .expect("!DbTransaction::open_table");

        table
            .cursor_builder()
            .reverse()
            .open_cursor("rel_coin_value")
            .await
            .map(|_| ())
            .expect_err(
                "CursorBuilder::open_cursor should have failed because 'reverse' can be used with key range only",
            );
    }

    #[wasm_bindgen_test]
    async fn test_iter_single_key_bound_cursor() {
        const DB_NAME: &str = "TEST_ITER_SINGLE_KEY_BOUND_CURSOR";
        const DB_VERSION: u32 = 1;

        let items = vec![
            swap_item!("uuid1", "RICK", "MORTY", 10, 3, 700),
            swap_item!("uuid2", "MORTY", "KMD", 95000, 1, 721),
            swap_item!("uuid3", "RICK", "XYZ", 7, u32::MAX, 1281), // +
            swap_item!("uuid4", "RICK", "MORTY", 8, 6, 92),        // +
            swap_item!("uuid5", "QRC20", "RICK", 2, 4, 721),
            swap_item!("uuid6", "KMD", "MORTY", 12, 3124, 214), // +
        ];

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<SwapTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction");
        let table = transaction
            .table::<SwapTable>()
            .await
            .expect("!DbTransaction::open_table");
        fill_table(&table, &items).await;

        let mut cursor_iter = table
            .cursor_builder()
            .bound("rel_coin_value", 5u32, u32::MAX)
            .open_cursor("rel_coin_value")
            .await
            .expect("!CursorBuilder::open_cursor");

        // The items must be sorted by `rel_coin_value`.
        assert_eq!(
            next_item(&mut cursor_iter).await,
            Some(swap_item!("uuid4", "RICK", "MORTY", 8, 6, 92))
        );
        assert_eq!(
            next_item(&mut cursor_iter).await,
            Some(swap_item!("uuid6", "KMD", "MORTY", 12, 3124, 214))
        );
        assert_eq!(
            next_item(&mut cursor_iter).await,
            Some(swap_item!("uuid3", "RICK", "XYZ", 7, u32::MAX, 1281))
        );
        assert!(next_item(&mut cursor_iter).await.is_none());
        // Try to poll one more time. This should not fail but return `None`.
        assert!(next_item(&mut cursor_iter).await.is_none());
    }

    #[wasm_bindgen_test]
    async fn test_rev_iter_single_key_bound_cursor() {
        const DB_NAME: &str = "TEST_REV_ITER_SINGLE_KEY_BOUND_CURSOR";
        const DB_VERSION: u32 = 1;
        let items = vec![
            swap_item!("uuid1", "RICK", "MORTY", 10, 3, 700),
            swap_item!("uuid2", "MORTY", "KMD", 95000, 1, 721),
            swap_item!("uuid3", "RICK", "XYZ", 7, u32::MAX, 1281), // +
            swap_item!("uuid4", "RICK", "MORTY", 8, 6, 92),        // +
            swap_item!("uuid5", "QRC20", "RICK", 2, 4, 721),
            swap_item!("uuid6", "KMD", "MORTY", 12, 3124, 214), // +
        ];

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<SwapTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction");
        let table = transaction
            .table::<SwapTable>()
            .await
            .expect("!DbTransaction::open_table");
        fill_table(&table, &items).await;

        let mut cursor_iter = table
            .cursor_builder()
            .bound("rel_coin_value", 5u32, u32::MAX)
            .reverse()
            .open_cursor("rel_coin_value")
            .await
            .expect("!CursorBuilder::open_cursor");

        // The items must be sorted in reverse order by `rel_coin_value`.
        assert_eq!(
            next_item(&mut cursor_iter).await,
            Some(swap_item!("uuid3", "RICK", "XYZ", 7, u32::MAX, 1281))
        );
        assert_eq!(
            next_item(&mut cursor_iter).await,
            Some(swap_item!("uuid6", "KMD", "MORTY", 12, 3124, 214))
        );
        assert_eq!(
            next_item(&mut cursor_iter).await,
            Some(swap_item!("uuid4", "RICK", "MORTY", 8, 6, 92))
        );
        assert!(next_item(&mut cursor_iter).await.is_none());
        // Try to poll one more time. This should not fail but return `None`.
        assert!(next_item(&mut cursor_iter).await.is_none());
    }

    #[wasm_bindgen_test]
    async fn test_cursor_where_condition() {
        const DB_NAME: &str = "TEST_REV_ITER_SINGLE_KEY_BOUND_CURSOR";
        const DB_VERSION: u32 = 1;

        let items = vec![
            swap_item!("uuid1", "RICK", "MORTY", 10, 3, 700),
            swap_item!("uuid2", "MORTY", "KMD", 95000, 1, 721),
            swap_item!("uuid3", "RICK", "XYZ", 7, u32::MAX, 1281), // +
            swap_item!("uuid4", "RICK", "MORTY", 8, 6, 92),        // +
            swap_item!("uuid5", "QRC20", "RICK", 2, 4, 721),
            swap_item!("uuid6", "KMD", "MORTY", 12, 3124, 214), // +
        ];

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<SwapTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction");
        let table = transaction
            .table::<SwapTable>()
            .await
            .expect("!DbTransaction::open_table");
        fill_table(&table, &items).await;

        // check for first swap where started_at is 1281.
        let condition = move |swap| {
            let swap = serde_json::from_value::<SwapTable>(swap).unwrap();
            Ok(swap.started_at == 1281)
        };
        let maybe_swap = table
            .cursor_builder()
            .bound("rel_coin_value", 5u32, u32::MAX)
            .where_(condition)
            .open_cursor("rel_coin_value")
            .await
            .expect("!CursorBuilder::open_cursor")
            .next()
            .await
            .expect("!Cursor next result")
            .map(|(_, swap)| swap);

        // maybe_swap should return swap with uuid3 since it's swap uuid3 that has started_at to be 1281.
        assert_eq!(maybe_swap, Some(swap_item!("uuid3", "RICK", "XYZ", 7, u32::MAX, 1281)));
    }

    #[wasm_bindgen_test]
    async fn test_cursor_where_first_condition() {
        const DB_NAME: &str = "TEST_REV_ITER_SINGLE_KEY_BOUND_CURSOR";
        const DB_VERSION: u32 = 1;

        let items = vec![
            swap_item!("uuid1", "RICK", "MORTY", 10, 3, 700),
            swap_item!("uuid2", "MORTY", "KMD", 95000, 1, 721),
            swap_item!("uuid3", "RICK", "XYZ", 7, u32::MAX, 1281), // +
            swap_item!("uuid4", "RICK", "MORTY", 8, 6, 92),        // +
            swap_item!("uuid5", "QRC20", "RICK", 2, 4, 721),
            swap_item!("uuid6", "KMD", "MORTY", 12, 3124, 214), // +
        ];

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<SwapTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction");
        let table = transaction
            .table::<SwapTable>()
            .await
            .expect("!DbTransaction::open_table");
        fill_table(&table, &items).await;

        let maybe_swap = table
            .cursor_builder()
            .bound("rel_coin_value", 5u32, u32::MAX)
            .where_first()
            .open_cursor("rel_coin_value")
            .await
            .expect("!CursorBuilder::open_cursor")
            .next()
            .await
            .expect("!Cursor next result")
            .map(|(_, swap)| swap);

        // maybe_swap should return swap with uuid4 since it's the item with the lowest rel_coin_value in the store.
        assert_eq!(maybe_swap, Some(swap_item!("uuid4", "RICK", "MORTY", 8, 6, 92)));
    }

    #[wasm_bindgen_test]
    async fn test_cursor_where_first_but_reversed_condition() {
        const DB_NAME: &str = "TEST_REV_ITER_SINGLE_KEY_BOUND_CURSOR";
        const DB_VERSION: u32 = 1;

        let items = vec![
            swap_item!("uuid1", "RICK", "MORTY", 10, 3, 700),
            swap_item!("uuid2", "MORTY", "KMD", 95000, 1, 721),
            swap_item!("uuid3", "RICK", "XYZ", 7, u32::MAX, 1281), // +
            swap_item!("uuid4", "RICK", "MORTY", 8, 6, 92),        // +
            swap_item!("uuid5", "QRC20", "RICK", 2, 4, 721),
            swap_item!("uuid6", "KMD", "MORTY", 12, 3124, 214), // +
        ];

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<SwapTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction");
        let table = transaction
            .table::<SwapTable>()
            .await
            .expect("!DbTransaction::open_table");
        fill_table(&table, &items).await;

        let maybe_swap = table
            .cursor_builder()
            .bound("rel_coin_value", 5u32, u32::MAX)
            .where_first()
            .reverse()
            .open_cursor("rel_coin_value")
            .await
            .expect("!CursorBuilder::open_cursor")
            .next()
            .await
            .expect("!Cursor next result")
            .map(|(_, swap)| swap);

        // maybe_swap should return swap with uuid4 since it's the item with the highest rel_coin_value in the store.
        assert_eq!(maybe_swap, Some(swap_item!("uuid3", "RICK", "XYZ", 7, u32::MAX, 1281)));
    }

    #[wasm_bindgen_test]
    async fn test_cursor_where_condition_with_limit() {
        const DB_NAME: &str = "TEST_REV_ITER_SINGLE_KEY_BOUND_CURSOR";
        const DB_VERSION: u32 = 1;

        let items = vec![
            swap_item!("uuid1", "RICK", "MORTY", 10, 3, 700),
            swap_item!("uuid2", "MORTY", "KMD", 95000, 1, 721),
            swap_item!("uuid3", "RICK", "XYZ", 7, u32::MAX, 1281), // +
            swap_item!("uuid4", "RICK", "MORTY", 8, 6, 92),        // +
            swap_item!("uuid5", "QRC20", "RICK", 2, 4, 721),
            swap_item!("uuid6", "KMD", "MORTY", 12, 3124, 214), // +
        ];

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<SwapTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction");
        let table = transaction
            .table::<SwapTable>()
            .await
            .expect("!DbTransaction::open_table");
        fill_table(&table, &items).await;

        let maybe_swaps = table
            .cursor_builder()
            .bound("rel_coin_value", 5u32, u32::MAX)
            .where_(|_| Ok(true))
            .limit(1)
            .open_cursor("rel_coin_value")
            .await
            .expect("!CursorBuilder::open_cursor")
            .collect()
            .await
            .expect("!CursorBuilder::open_cursor")
            .into_iter()
            .map(|(_, swap)| swap)
            .collect::<Vec<_>>();

        let expected_swaps = vec![swap_item!("uuid4", "RICK", "MORTY", 8, 6, 92)];
        assert_eq!(expected_swaps, maybe_swaps)
    }

    #[wasm_bindgen_test]
    async fn test_cursor_with_limit() {
        const DB_NAME: &str = "TEST_REV_ITER_SINGLE_KEY_BOUND_CURSOR";
        const DB_VERSION: u32 = 1;

        let items = vec![
            swap_item!("uuid1", "RICK", "MORTY", 10, 3, 700),
            swap_item!("uuid2", "MORTY", "KMD", 95000, 1, 721),
            swap_item!("uuid3", "RICK", "XYZ", 7, u32::MAX, 1281), // +
            swap_item!("uuid4", "RICK", "MORTY", 8, 6, 92),        // +
            swap_item!("uuid5", "QRC20", "RICK", 2, 4, 721),
            swap_item!("uuid6", "KMD", "MORTY", 12, 3124, 214), // +
        ];

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<SwapTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction");
        let table = transaction
            .table::<SwapTable>()
            .await
            .expect("!DbTransaction::open_table");
        fill_table(&table, &items).await;

        let maybe_swaps = table
            .cursor_builder()
            .bound("rel_coin_value", 5u32, u32::MAX)
            .limit(2)
            .open_cursor("rel_coin_value")
            .await
            .expect("!CursorBuilder::open_cursor")
            .collect()
            .await
            .expect("!CursorBuilder::collect")
            .into_iter()
            .map(|(_, swap)| swap)
            .collect::<Vec<_>>();

        let expected_swaps = vec![
            swap_item!("uuid4", "RICK", "MORTY", 8, 6, 92),
            swap_item!("uuid6", "KMD", "MORTY", 12, 3124, 214),
        ];
        assert_eq!(expected_swaps, maybe_swaps)
    }

    #[wasm_bindgen_test]
    async fn test_cursor_with_offset_and_limit() {
        const DB_NAME: &str = "TEST_REV_ITER_SINGLE_KEY_BOUND_CURSOR";
        const DB_VERSION: u32 = 1;

        register_wasm_log();

        let items = vec![
            swap_item!("uuid1", "RICK", "XYZ", 7, u32::MAX, 1281),
            swap_item!("uuid2", "RICK", "MORTY", 8, 6, 92),
            swap_item!("uuid3", "RICK", "FTM", 12, 3124, 214),
        ];

        let db = IndexedDbBuilder::new(DbIdentifier::for_test(DB_NAME))
            .with_version(DB_VERSION)
            .with_table::<SwapTable>()
            .build()
            .await
            .expect("!IndexedDb::init");
        let transaction = db.transaction().await.expect("!IndexedDb::transaction");
        let table = transaction
            .table::<SwapTable>()
            .await
            .expect("!DbTransaction::open_table");
        fill_table(&table, &items).await;

        let maybe_swaps = table
            .cursor_builder()
            .only("base_coin", "RICK")
            .expect("!CursorBuilder::only")
            .offset(1)
            .limit(1)
            .open_cursor("base_coin")
            .await
            .expect("!CursorBuilder::open_cursor")
            .collect()
            .await
            .expect("!CursorBuilder::open_cursor")
            .into_iter()
            .map(|(_, swap)| swap)
            .collect::<Vec<_>>();

        let expected_swaps = vec![swap_item!("uuid2", "RICK", "MORTY", 8, 6, 92)];
        assert_eq!(expected_swaps, maybe_swaps)
    }
}
