use super::{MyRecentSwapsUuids, MySwapsFilter};
use async_trait::async_trait;
use common::PagingOptions;
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use uuid::Uuid;

pub type MySwapsResult<T> = Result<T, MmError<MySwapsError>>;

#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
#[derive(Debug, Display, PartialEq)]
pub enum MySwapsError {
    #[display(fmt = "Error serializing swap: {_0}")]
    ErrorSerializingItem(String),
    #[display(fmt = "Error deserializing swap: {_0}")]
    ErrorDeserializingItem(String),
    #[display(fmt = "Invalid timestamp range")]
    InvalidTimestampRange,
    #[display(fmt = "Error saving swap: {_0}")]
    ErrorSavingSwap(String),
    #[display(fmt = "'from_uuid' not found: {_0}")]
    FromUuidNotFound(Uuid),
    #[display(fmt = "Error parsing uuid: {_0}")]
    UuidParse(uuid::Error),
    #[display(fmt = "Unknown SQL error: {_0}")]
    UnknownSqlError(String),
    #[display(fmt = "Internal error: {_0}")]
    InternalError(String),
}

#[async_trait]
pub trait MySwapsOps {
    async fn save_new_swap(
        &self,
        my_coin: &str,
        other_coin: &str,
        uuid: Uuid,
        started_at: u64,
        swap_type: u8,
    ) -> MySwapsResult<()>;

    async fn my_recent_swaps_with_filters(
        &self,
        filter: &MySwapsFilter,
        paging_options: Option<&PagingOptions>,
    ) -> MySwapsResult<MyRecentSwapsUuids>;
}

pub struct MySwapsStorage {
    ctx: MmArc,
}

impl MySwapsStorage {
    pub fn new(ctx: MmArc) -> MySwapsStorage {
        MySwapsStorage { ctx }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod native_impl {
    use super::*;
    use crate::database::my_swaps::{insert_new_swap, select_uuids_by_my_swaps_filter, SelectSwapsUuidsErr};
    use db_common::sqlite::rusqlite::Error as SqlError;

    impl From<SelectSwapsUuidsErr> for MySwapsError {
        fn from(e: SelectSwapsUuidsErr) -> Self {
            match e {
                SelectSwapsUuidsErr::Sql(db) => MySwapsError::UnknownSqlError(db.to_string()),
                SelectSwapsUuidsErr::Parse(uuid) => MySwapsError::UuidParse(uuid),
            }
        }
    }

    impl From<SqlError> for MySwapsError {
        fn from(e: SqlError) -> Self {
            MySwapsError::UnknownSqlError(e.to_string())
        }
    }

    #[async_trait]
    impl MySwapsOps for MySwapsStorage {
        async fn save_new_swap(
            &self,
            my_coin: &str,
            other_coin: &str,
            uuid: Uuid,
            started_at: u64,
            swap_type: u8,
        ) -> MySwapsResult<()> {
            Ok(insert_new_swap(
                &self.ctx,
                my_coin,
                other_coin,
                &uuid.to_string(),
                &started_at.to_string(),
                swap_type,
            )?)
        }

        async fn my_recent_swaps_with_filters(
            &self,
            filter: &MySwapsFilter,
            paging_options: Option<&PagingOptions>,
        ) -> MySwapsResult<MyRecentSwapsUuids> {
            Ok(select_uuids_by_my_swaps_filter(
                &self.ctx.sqlite_connection(),
                filter,
                paging_options,
            )?)
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_impl {
    use super::*;
    use crate::lp_swap::swap_wasm_db::cursor_prelude::*;
    use crate::lp_swap::swap_wasm_db::{DbTransactionError, InitDbError, MySwapsFiltersTable};
    use crate::lp_swap::{SwapsContext, LEGACY_SWAP_TYPE};
    use std::collections::BTreeSet;
    use uuid::Uuid;

    impl From<DbTransactionError> for MySwapsError {
        fn from(e: DbTransactionError) -> Self {
            let stringified_error = e.to_string();
            match e {
                DbTransactionError::NoSuchTable { .. }
                | DbTransactionError::ErrorCreatingTransaction(_)
                | DbTransactionError::ErrorOpeningTable { .. }
                // We don't expect that the `String` and `u32` types serialization to fail.
                | DbTransactionError::ErrorSerializingIndex { .. }
                | DbTransactionError::ErrorGettingItems(_)
                | DbTransactionError::ErrorCountingItems(_)
                // We don't delete items from the `my_swaps` table
                | DbTransactionError::ErrorDeletingItems(_)
                | DbTransactionError::MultipleItemsByUniqueIndex { .. }
                | DbTransactionError::NoSuchIndex { .. }
                | DbTransactionError::InvalidIndex { .. }
                | DbTransactionError::UnexpectedState(_)
                | DbTransactionError::TransactionAborted => MySwapsError::InternalError(stringified_error),
                DbTransactionError::ErrorSerializingItem(_) => MySwapsError::ErrorSerializingItem(stringified_error),
                DbTransactionError::ErrorDeserializingItem(_) => MySwapsError::ErrorDeserializingItem(stringified_error),
                DbTransactionError::ErrorUploadingItem(_) => MySwapsError::ErrorSavingSwap(stringified_error),
            }
        }
    }

    impl From<CursorError> for MySwapsError {
        fn from(e: CursorError) -> Self {
            let stringified_error = e.to_string();
            match e {
                // We don't expect that the `String` and `u32` types serialization to fail.
                CursorError::ErrorSerializingIndexFieldValue {..}
                // We don't expect that the `String` and `u32` types deserialization to fail.
                | CursorError::ErrorDeserializingIndexValue {..}
                | CursorError::ErrorOpeningCursor {..}
                | CursorError::AdvanceError {..}
                | CursorError::InvalidKeyRange {..}
                | CursorError::TypeMismatch {..}
                | CursorError::IncorrectNumberOfKeysPerIndex {..}
                | CursorError::UnexpectedState(..)
                | CursorError::IncorrectUsage {..} => MySwapsError::InternalError(stringified_error),
                CursorError::ErrorDeserializingItem {..} => MySwapsError::ErrorDeserializingItem(stringified_error),
            }
        }
    }

    impl From<InitDbError> for MySwapsError {
        fn from(e: InitDbError) -> Self {
            MySwapsError::InternalError(e.to_string())
        }
    }

    #[async_trait]
    impl MySwapsOps for MySwapsStorage {
        async fn save_new_swap(
            &self,
            my_coin: &str,
            other_coin: &str,
            uuid: Uuid,
            started_at: u64,
            swap_type: u8,
        ) -> MySwapsResult<()> {
            let swap_ctx = SwapsContext::from_ctx(&self.ctx).map_to_mm(MySwapsError::InternalError)?;
            let db = swap_ctx.swap_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let my_swaps_table = transaction.table::<MySwapsFiltersTable>().await.map_mm_err()?;

            let item = MySwapsFiltersTable {
                uuid,
                my_coin: my_coin.to_owned(),
                other_coin: other_coin.to_owned(),
                started_at: started_at as u32,
                is_finished: false.into(),
                swap_type,
            };
            my_swaps_table.add_item(&item).await.map_mm_err()?;
            Ok(())
        }

        async fn my_recent_swaps_with_filters(
            &self,
            filter: &MySwapsFilter,
            paging_options: Option<&PagingOptions>,
        ) -> MySwapsResult<MyRecentSwapsUuids> {
            let swap_ctx = SwapsContext::from_ctx(&self.ctx).map_to_mm(MySwapsError::InternalError)?;
            let db = swap_ctx.swap_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let my_swaps_table = transaction.table::<MySwapsFiltersTable>().await.map_mm_err()?;

            let from_timestamp = filter.from_timestamp.map(|t| t as u32).unwrap_or_default();
            let to_timestamp = filter.to_timestamp.map(|t| t as u32).unwrap_or(u32::MAX);
            if from_timestamp > to_timestamp {
                return MmError::err(MySwapsError::InvalidTimestampRange);
            }

            let items = match (&filter.my_coin, &filter.other_coin) {
                (Some(my_coin), Some(other_coin)) => my_swaps_table
                    .cursor_builder()
                    .only("my_coin", my_coin)
                    .map_mm_err()?
                    .only("other_coin", other_coin)
                    .map_mm_err()?
                    .bound("started_at", from_timestamp, to_timestamp)
                    .open_cursor("with_my_other_coins")
                    .await
                    .map_mm_err()?
                    .collect()
                    .await
                    .map_mm_err()?,
                (Some(my_coin), None) => my_swaps_table
                    .cursor_builder()
                    .only("my_coin", my_coin)
                    .map_mm_err()?
                    .bound("started_at", from_timestamp, to_timestamp)
                    .open_cursor("with_my_coin")
                    .await
                    .map_mm_err()?
                    .collect()
                    .await
                    .map_mm_err()?,
                (None, Some(other_coin)) => my_swaps_table
                    .cursor_builder()
                    .only("other_coin", other_coin)
                    .map_mm_err()?
                    .bound("started_at", from_timestamp, to_timestamp)
                    .open_cursor("with_other_coin")
                    .await
                    .map_mm_err()?
                    .collect()
                    .await
                    .map_mm_err()?,
                (None, None) => my_swaps_table
                    .cursor_builder()
                    .bound("started_at", from_timestamp, to_timestamp)
                    .open_cursor("started_at")
                    .await
                    .map_mm_err()?
                    .collect()
                    .await
                    .map_mm_err()?,
            };

            let uuids: BTreeSet<OrderedUuid> = items
                .into_iter()
                .map(|(_item_id, item)| OrderedUuid::from(item))
                .collect();
            match paging_options {
                Some(paging) => take_according_to_paging_opts(uuids, paging),
                None => {
                    let total_count = uuids.len();
                    Ok(MyRecentSwapsUuids {
                        uuids_and_types: uuids
                            .into_iter()
                            .map(|ordered| (ordered.uuid, ordered.swap_type))
                            .collect(),
                        total_count,
                        skipped: 0,
                    })
                },
            }
        }
    }

    pub(super) fn take_according_to_paging_opts(
        uuids: BTreeSet<OrderedUuid>,
        paging: &PagingOptions,
    ) -> MySwapsResult<MyRecentSwapsUuids> {
        let total_count = uuids.len();

        let skip = match paging.from_uuid {
            // `page_number` is ignored if from_uuid is set
            Some(expected_uuid) => {
                uuids
                    .iter()
                    .position(|ordered_uuid| ordered_uuid.uuid == expected_uuid)
                    .or_mm_err(|| MySwapsError::FromUuidNotFound(expected_uuid))?
                    + 1
            },
            None => (paging.page_number.get() - 1) * paging.limit,
        };

        let uuids_and_types = uuids
            .into_iter()
            .map(|ordered| (ordered.uuid, ordered.swap_type))
            .skip(skip)
            .take(paging.limit)
            .collect();

        Ok(MyRecentSwapsUuids {
            uuids_and_types,
            total_count,
            skipped: skip,
        })
    }

    /// A swap identifier is ordered first by `started_at` and then by `uuid`.
    #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
    pub(super) struct OrderedUuid {
        pub started_at: u32,
        pub uuid: Uuid,
        pub swap_type: u8,
    }

    impl From<MySwapsFiltersTable> for OrderedUuid {
        fn from(item: MySwapsFiltersTable) -> OrderedUuid {
            OrderedUuid {
                started_at: item.started_at,
                uuid: item.uuid,
                swap_type: item.swap_type,
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_tests {
    use super::wasm_impl::*;
    use super::*;
    use crate::lp_swap::{LEGACY_SWAP_TYPE, MAKER_SWAP_V2_TYPE, TAKER_SWAP_V2_TYPE};
    use common::log::wasm_log::register_wasm_log;
    use common::new_uuid;
    use mm2_core::mm_ctx::MmCtxBuilder;
    use rand::seq::SliceRandom;
    use rand::Rng;
    use std::collections::BTreeSet;
    use std::num::NonZeroUsize;
    use std::ops::Range;
    use uuid::Uuid;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    fn is_applied(filters: &MySwapsFilter, my_coin: &str, other_coin: &str, started_at: u64) -> bool {
        if let Some(ref expected_my_coin) = filters.my_coin {
            if expected_my_coin != my_coin {
                return false;
            }
        }
        if let Some(ref expected_other_coin) = filters.other_coin {
            if expected_other_coin != other_coin {
                return false;
            }
        }
        let from_timestamp = filters.from_timestamp.unwrap_or_default();
        let to_timestamp = filters.to_timestamp.unwrap_or(u64::MAX);
        from_timestamp <= started_at && started_at <= to_timestamp
    }

    async fn test_my_recent_swaps_impl(
        total_count: usize,
        coins: &[&str],
        timestamp_range: Range<u64>,
        filters: MySwapsFilter,
    ) {
        let ctx = MmCtxBuilder::new().with_test_db_namespace().into_mm_arc();
        let my_swaps = MySwapsStorage::new(ctx);

        let mut expected_uuids = BTreeSet::new();
        let mut rng = rand::thread_rng();

        for _ in 0..total_count {
            let uuid = new_uuid();
            let my_coin = *coins.choose(&mut rng).unwrap();
            let other_coin = *coins.choose(&mut rng).unwrap();
            let started_at = rng.gen_range(timestamp_range.start, timestamp_range.end);
            let swap_type = rng.gen_range(0, 3);

            if is_applied(&filters, my_coin, other_coin, started_at) {
                expected_uuids.insert(OrderedUuid {
                    started_at: started_at as u32,
                    uuid,
                    swap_type,
                });
            }
            my_swaps
                .save_new_swap(my_coin, other_coin, uuid, started_at, swap_type)
                .await
                .expect("!MySwapsStorage::save_new_swap");
        }

        let actual = my_swaps
            .my_recent_swaps_with_filters(&filters, None)
            .await
            .expect("!MySwapsStorage::my_recent_swaps_with_filters");

        let expected_total_count = expected_uuids.len();
        let expected = MyRecentSwapsUuids {
            uuids_and_types: expected_uuids
                .into_iter()
                .map(|ordered| (ordered.uuid, ordered.swap_type))
                .collect(),
            total_count: expected_total_count,
            skipped: 0,
        };
        assert_eq!(actual, expected);
    }

    #[wasm_bindgen_test]
    fn test_take_according_to_paging_opts() {
        register_wasm_log();

        let uuids: BTreeSet<OrderedUuid> = [
            (1, "49c79ea4-e1eb-4fb2-a0ef-265bded0b77f", TAKER_SWAP_V2_TYPE),
            (2, "2f9afe84-7a89-4194-8947-45fba563118f", MAKER_SWAP_V2_TYPE),
            (3, "41383f43-46a5-478c-9386-3b2cce0aca20", LEGACY_SWAP_TYPE),
            (4, "5acb0e63-8b26-469e-81df-7dd9e4a9ad15", TAKER_SWAP_V2_TYPE),
            (5, "3447b727-fe93-4357-8e5a-8cf2699b7e86", MAKER_SWAP_V2_TYPE),
            // ordered by uuid
            (6, "8f5b267a-efa8-49d6-a92d-ec0523cca891", LEGACY_SWAP_TYPE),
            (6, "983ce732-62a8-4a44-b4ac-7e4271adc977", TAKER_SWAP_V2_TYPE),
            (7, "c52659d7-4e13-41f5-9c1a-30cc2f646033", MAKER_SWAP_V2_TYPE),
            (8, "af5e0383-97f6-4408-8c03-a8eb8d17e46d", LEGACY_SWAP_TYPE),
        ]
        .iter()
        .map(|(started_at, uuid, swap_type)| OrderedUuid {
            started_at: *started_at,
            uuid: Uuid::parse_str(uuid).unwrap(),
            swap_type: *swap_type,
        })
        .collect();

        let paging = PagingOptions {
            limit: 2,
            // has to be ignored
            page_number: NonZeroUsize::new(10).unwrap(),
            from_uuid: Some(Uuid::parse_str("8f5b267a-efa8-49d6-a92d-ec0523cca891").unwrap()),
        };
        let actual = take_according_to_paging_opts(uuids.clone(), &paging).unwrap();
        let expected = MyRecentSwapsUuids {
            uuids_and_types: vec![
                (
                    "983ce732-62a8-4a44-b4ac-7e4271adc977".parse().unwrap(),
                    TAKER_SWAP_V2_TYPE,
                ),
                (
                    "c52659d7-4e13-41f5-9c1a-30cc2f646033".parse().unwrap(),
                    MAKER_SWAP_V2_TYPE,
                ),
            ],
            total_count: uuids.len(),
            skipped: 6,
        };
        assert_eq!(actual, expected);

        let paging = PagingOptions {
            limit: 3,
            page_number: NonZeroUsize::new(2).unwrap(),
            from_uuid: None,
        };
        let actual = take_according_to_paging_opts(uuids.clone(), &paging).unwrap();
        let expected = MyRecentSwapsUuids {
            uuids_and_types: vec![
                (
                    "5acb0e63-8b26-469e-81df-7dd9e4a9ad15".parse().unwrap(),
                    TAKER_SWAP_V2_TYPE,
                ),
                (
                    "3447b727-fe93-4357-8e5a-8cf2699b7e86".parse().unwrap(),
                    MAKER_SWAP_V2_TYPE,
                ),
                (
                    "8f5b267a-efa8-49d6-a92d-ec0523cca891".parse().unwrap(),
                    LEGACY_SWAP_TYPE,
                ),
            ],
            total_count: uuids.len(),
            skipped: 3,
        };

        assert_eq!(actual, expected);

        let from_uuid = Uuid::parse_str("f87fa9ce-0820-4675-b85d-db18c7bc9fb4").unwrap();
        let paging = PagingOptions {
            limit: 3,
            // has to be ignored
            page_number: NonZeroUsize::new(10).unwrap(),
            // unknown UUID
            from_uuid: Some(from_uuid),
        };
        let actual = take_according_to_paging_opts(uuids, &paging)
            .expect_err("'take_according_to_paging_opts' must return an error");
        assert_eq!(actual.into_inner(), MySwapsError::FromUuidNotFound(from_uuid));
    }

    #[wasm_bindgen_test]
    async fn test_my_recent_swaps() {
        const COINS: [&str; 3] = ["RICK", "MORTY", "KMD"];

        register_wasm_log();

        let filters = MySwapsFilter {
            my_coin: Some("RICK".to_owned()),
            other_coin: Some("MORTY".to_owned()),
            from_timestamp: Some(2000),
            to_timestamp: Some(3000),
        };
        test_my_recent_swaps_impl(1000, &COINS, 1000..5000, filters).await;

        let filters = MySwapsFilter {
            my_coin: Some("RICK".to_owned()),
            other_coin: None,
            from_timestamp: Some(2000),
            to_timestamp: Some(3000),
        };
        test_my_recent_swaps_impl(100, &COINS, 1000..5000, filters).await;

        let filters = MySwapsFilter {
            my_coin: Some("RICK".to_owned()),
            other_coin: None,
            from_timestamp: Some(2000),
            to_timestamp: None,
        };
        test_my_recent_swaps_impl(100, &COINS, 1000..5000, filters).await;

        let filters = MySwapsFilter {
            my_coin: None,
            other_coin: None,
            from_timestamp: None,
            to_timestamp: Some(1000),
        };
        test_my_recent_swaps_impl(10, &COINS, 1001..3000, filters).await;

        let filters = MySwapsFilter {
            my_coin: None,
            other_coin: None,
            from_timestamp: Some(3000),
            to_timestamp: None,
        };
        test_my_recent_swaps_impl(10, &COINS, 1000..2000, filters).await;
    }
}
