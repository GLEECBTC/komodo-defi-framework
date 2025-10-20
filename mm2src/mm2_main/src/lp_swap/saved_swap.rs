use crate::lp_swap::maker_swap::{MakerSavedSwap, MakerSwap, MakerSwapEvent};
use crate::lp_swap::taker_swap::{TakerSavedSwap, TakerSwap, TakerSwapEvent};
use crate::lp_swap::{MySwapInfo, RecoveredSwap};
use async_trait::async_trait;
use coins::lp_coinfind;
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use rpc::v1::types::H256 as H256Json;
use uuid::Uuid;
#[cfg(target_arch = "wasm32")]
pub use wasm_impl::migrate_swaps_data;

pub type SavedSwapResult<T> = Result<T, MmError<SavedSwapError>>;

#[derive(Debug, Display, Deserialize, Serialize)]
pub enum SavedSwapError {
    #[display(fmt = "Error saving the a swap: {_0}")]
    ErrorSaving(String),
    #[display(fmt = "Error loading a swap: {_0}")]
    ErrorLoading(String),
    #[display(fmt = "Error deserializing a swap: {_0}")]
    ErrorDeserializing(String),
    #[display(fmt = "Error serializing a swap: {_0}")]
    ErrorSerializing(String),
    CursorError(String),
    #[allow(dead_code)]
    #[display(fmt = "Internal error: {_0}")]
    InternalError(String),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum SavedSwap {
    Maker(MakerSavedSwap),
    Taker(TakerSavedSwap),
}

impl From<MakerSavedSwap> for SavedSwap {
    fn from(maker: MakerSavedSwap) -> Self {
        SavedSwap::Maker(maker)
    }
}

impl From<TakerSavedSwap> for SavedSwap {
    fn from(taker: TakerSavedSwap) -> Self {
        SavedSwap::Taker(taker)
    }
}

impl SavedSwap {
    pub fn is_success(&self) -> Result<bool, String> {
        match self {
            SavedSwap::Maker(swap) => swap.is_success(),
            SavedSwap::Taker(swap) => swap.is_success(),
        }
    }

    pub fn is_finished_and_success(&self) -> bool {
        self.is_success().unwrap_or(false)
    }

    pub fn is_finished(&self) -> bool {
        match self {
            SavedSwap::Maker(swap) => swap.is_finished(),
            SavedSwap::Taker(swap) => swap.is_finished(),
        }
    }

    pub fn watcher_message_sent(&self) -> bool {
        match &self {
            SavedSwap::Taker(taker_swap) => taker_swap.watcher_message_sent(),
            _ => false,
        }
    }

    pub fn uuid(&self) -> &Uuid {
        match self {
            SavedSwap::Maker(swap) => &swap.uuid,
            SavedSwap::Taker(swap) => &swap.uuid,
        }
    }

    #[cfg(all(not(target_arch = "wasm32"), feature = "new-db-arch"))]
    pub fn maker_address(&self) -> &str {
        match self {
            SavedSwap::Maker(swap) => &swap.maker_address,
            SavedSwap::Taker(swap) => &swap.maker_address,
        }
    }

    pub fn maker_coin_ticker(&self) -> Result<String, String> {
        match self {
            SavedSwap::Maker(swap) => swap.maker_coin(),
            SavedSwap::Taker(swap) => swap.maker_coin(),
        }
    }

    pub fn taker_coin_ticker(&self) -> Result<String, String> {
        match self {
            SavedSwap::Maker(swap) => swap.taker_coin(),
            SavedSwap::Taker(swap) => swap.taker_coin(),
        }
    }

    pub fn get_my_info(&self) -> Option<MySwapInfo> {
        match self {
            SavedSwap::Maker(swap) => swap.get_my_info(),
            SavedSwap::Taker(swap) => swap.get_my_info(),
        }
    }

    pub async fn recover_funds(self, ctx: MmArc) -> Result<RecoveredSwap, String> {
        let maker_ticker = try_s!(self.maker_coin_ticker());
        let maker_coin = match lp_coinfind(&ctx, &maker_ticker).await {
            Ok(Some(c)) => c,
            Ok(None) => return ERR!("Coin {} is not activated", maker_ticker),
            Err(e) => return ERR!("Error {} on {} coin find attempt", e, maker_ticker),
        };

        let taker_ticker = try_s!(self.taker_coin_ticker());
        let taker_coin = match lp_coinfind(&ctx, &taker_ticker).await {
            Ok(Some(c)) => c,
            Ok(None) => return ERR!("Coin {} is not activated", taker_ticker),
            Err(e) => return ERR!("Error {} on {} coin find attempt", e, taker_ticker),
        };
        match self {
            SavedSwap::Maker(saved) => {
                let (maker_swap, _) = try_s!(MakerSwap::load_from_saved(ctx, maker_coin, taker_coin, saved));
                Ok(try_s!(maker_swap.recover_funds().await))
            },
            SavedSwap::Taker(saved) => {
                let (taker_swap, _) = try_s!(TakerSwap::load_from_saved(ctx, maker_coin, taker_coin, saved).await);
                Ok(try_s!(taker_swap.recover_funds().await))
            },
        }
    }

    pub fn is_recoverable(&self) -> bool {
        match self {
            SavedSwap::Maker(saved) => saved.is_recoverable(),
            SavedSwap::Taker(saved) => saved.is_recoverable(),
        }
    }

    pub fn hide_secrets(&mut self) {
        match self {
            SavedSwap::Maker(swap) => {
                if let Some(ref mut event) = swap.events.first_mut() {
                    if let MakerSwapEvent::Started(ref mut data) = event.event {
                        data.secret = H256Json::default();
                        data.p2p_privkey = None;
                    }
                }
            },
            SavedSwap::Taker(swap) => {
                if let Some(ref mut event) = swap.events.first_mut() {
                    if let TakerSwapEvent::Started(ref mut data) = event.event {
                        data.p2p_privkey = None;
                    }
                }
            },
        };
    }

    pub async fn fetch_and_set_usd_prices(&mut self) {
        match self {
            SavedSwap::Maker(maker) => maker.fetch_and_set_usd_prices().await,
            SavedSwap::Taker(taker) => taker.fetch_and_set_usd_prices().await,
        }
    }
}

#[async_trait]
pub trait SavedSwapIo {
    async fn load_my_swap_from_db(
        ctx: &MmArc,
        address_dir: Option<&str>,
        uuid: Uuid,
    ) -> SavedSwapResult<Option<SavedSwap>>;

    async fn load_all_my_swaps_from_db(ctx: &MmArc) -> SavedSwapResult<Vec<SavedSwap>>;

    #[cfg(not(target_arch = "wasm32"))]
    async fn load_from_maker_stats_db(ctx: &MmArc, uuid: Uuid) -> SavedSwapResult<Option<MakerSavedSwap>>;

    #[cfg(not(target_arch = "wasm32"))]
    async fn load_all_from_maker_stats_db(ctx: &MmArc) -> SavedSwapResult<Vec<MakerSavedSwap>>;

    #[cfg(not(target_arch = "wasm32"))]
    async fn load_from_taker_stats_db(ctx: &MmArc, uuid: Uuid) -> SavedSwapResult<Option<TakerSavedSwap>>;

    #[cfg(not(target_arch = "wasm32"))]
    async fn load_all_from_taker_stats_db(ctx: &MmArc) -> SavedSwapResult<Vec<TakerSavedSwap>>;

    /// Save the serialized `SavedSwap` to the swaps db.
    async fn save_to_db(&self, ctx: &MmArc) -> SavedSwapResult<()>;

    /// Save the inner maker/taker swap to the corresponding stats db.
    #[cfg(not(target_arch = "wasm32"))]
    async fn save_to_stats_db(&self, ctx: &MmArc) -> SavedSwapResult<()>;
}

#[cfg(not(target_arch = "wasm32"))]
mod native_impl {
    use super::*;
    use crate::lp_swap::maker_swap::{stats_maker_swap_dir, stats_maker_swap_file_path};
    use crate::lp_swap::taker_swap::{stats_taker_swap_dir, stats_taker_swap_file_path};
    use crate::lp_swap::{my_swap_file_path, my_swaps_dir};
    use mm2_io::fs::{read_dir_json, read_json, write_json, FsJsonError};

    const USE_TMP_FILE: bool = true;

    impl From<FsJsonError> for SavedSwapError {
        fn from(fs: FsJsonError) -> Self {
            match fs {
                FsJsonError::IoReading(reading) => SavedSwapError::ErrorLoading(reading.to_string()),
                FsJsonError::IoWriting(writing) => SavedSwapError::ErrorSaving(writing.to_string()),
                FsJsonError::Serializing(serializing) => SavedSwapError::ErrorSerializing(serializing.to_string()),
                FsJsonError::Deserializing(deserializing) => {
                    SavedSwapError::ErrorDeserializing(deserializing.to_string())
                },
            }
        }
    }

    #[async_trait]
    impl SavedSwapIo for SavedSwap {
        async fn load_my_swap_from_db(
            ctx: &MmArc,
            address_dir: Option<&str>,
            uuid: Uuid,
        ) -> SavedSwapResult<Option<SavedSwap>> {
            // TODO(new-db-arch): Set the correct address directory for the new db arch branch (via a query to the global DB).
            #[cfg(feature = "new-db-arch")]
            let address_dir = address_dir.unwrap_or("Fetch the address directory from the global DB given the UUID.");
            #[cfg(not(feature = "new-db-arch"))]
            let address_dir = address_dir.unwrap_or("no address directory for old DB architecture (has no effect)");
            let path = my_swap_file_path(ctx, address_dir, &uuid);
            Ok(read_json(&path).await.map_mm_err()?)
        }

        #[cfg_attr(feature = "new-db-arch", allow(unreachable_code, unused_variables))]
        async fn load_all_my_swaps_from_db(ctx: &MmArc) -> SavedSwapResult<Vec<SavedSwap>> {
            #[cfg(feature = "new-db-arch")]
            {
                // This method is solely used for migrations. Which we should ditch or refactor with the new DB architecture.
                // If we ditch the old migrations, this method should never be called (and should be deleted when we are
                // done with the incremental architecture change).
                todo!("Fix the dummy address directory in `my_swaps_dir` below or remove this method all together");
            }
            let path = my_swaps_dir(ctx, "has no effect in not(feature = 'new-db-arch')");
            Ok(read_dir_json(&path).await.map_mm_err()?)
        }

        async fn load_from_maker_stats_db(ctx: &MmArc, uuid: Uuid) -> SavedSwapResult<Option<MakerSavedSwap>> {
            let path = stats_maker_swap_file_path(ctx, &uuid);
            Ok(read_json(&path).await.map_mm_err()?)
        }

        async fn load_all_from_maker_stats_db(ctx: &MmArc) -> SavedSwapResult<Vec<MakerSavedSwap>> {
            let path = stats_maker_swap_dir(ctx);
            Ok(read_dir_json(&path).await.map_mm_err()?)
        }

        async fn load_from_taker_stats_db(ctx: &MmArc, uuid: Uuid) -> SavedSwapResult<Option<TakerSavedSwap>> {
            let path = stats_taker_swap_file_path(ctx, &uuid);
            Ok(read_json(&path).await.map_mm_err()?)
        }

        async fn load_all_from_taker_stats_db(ctx: &MmArc) -> SavedSwapResult<Vec<TakerSavedSwap>> {
            let path = stats_taker_swap_dir(ctx);
            Ok(read_dir_json(&path).await.map_mm_err()?)
        }

        async fn save_to_db(&self, ctx: &MmArc) -> SavedSwapResult<()> {
            #[cfg(feature = "new-db-arch")]
            let address_dir = self.maker_address();
            #[cfg(not(feature = "new-db-arch"))]
            let address_dir = "no address directory for old DB architecture (has no effect)";
            let path = my_swap_file_path(ctx, address_dir, self.uuid());
            write_json(self, &path, USE_TMP_FILE).await.map_mm_err()?;
            Ok(())
        }

        /// Save the inner maker/taker swap to the corresponding stats db.
        async fn save_to_stats_db(&self, ctx: &MmArc) -> SavedSwapResult<()> {
            match self {
                SavedSwap::Maker(maker) => {
                    let path = stats_maker_swap_file_path(ctx, &maker.uuid);
                    write_json(self, &path, USE_TMP_FILE).await.map_mm_err()?;
                },
                SavedSwap::Taker(taker) => {
                    let path = stats_taker_swap_file_path(ctx, &taker.uuid);
                    write_json(self, &path, USE_TMP_FILE).await.map_mm_err()?;
                },
            }
            Ok(())
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_impl {
    use super::*;
    use crate::lp_swap::swap_wasm_db::{
        DbTransactionError, InitDbError, MySwapsFiltersTable, SavedSwapTable, SwapsMigrationTable,
    };
    use crate::lp_swap::{SwapsContext, LEGACY_SWAP_TYPE};
    use bytes::Buf;
    use common::log::{info, warn};
    use mm2_db::indexed_db::cursor_prelude::CursorError;
    use mm2_db::indexed_db::DbTable;
    use serde_json as json;

    pub(super) async fn get_current_migration(
        migration_table: &DbTable<'_, SwapsMigrationTable>,
    ) -> MmResult<u32, SavedSwapError> {
        let migrations = migration_table
            .cursor_builder()
            .bound("migration", 0, u32::MAX)
            .reverse()
            .where_first()
            .open_cursor("migration")
            .await
            .map_mm_err()?
            .next()
            .await
            .map_mm_err()?;

        Ok(migrations.map(|(_, m)| m.migration).unwrap_or_default())
    }

    pub async fn migrate_swaps_data(ctx: &MmArc) -> MmResult<(), SavedSwapError> {
        let swaps_ctx = SwapsContext::from_ctx(ctx).map_to_mm(SavedSwapError::InternalError)?;
        let db = swaps_ctx.swap_db().await.map_mm_err()?;
        let transaction = db.transaction().await.map_mm_err()?;
        let migration_table = transaction.table::<SwapsMigrationTable>().await.map_mm_err()?;

        let mut migration = get_current_migration(&migration_table).await?;
        info!("Current swaps data migration {}", migration);
        loop {
            match migration {
                0 => {
                    let filters_table = transaction.table::<MySwapsFiltersTable>().await.map_mm_err()?;
                    let swaps_table = transaction.table::<SavedSwapTable>().await.map_mm_err()?;
                    let swaps = swaps_table.get_all_items().await.map_mm_err()?;
                    let swaps = swaps
                        .into_iter()
                        .map(|(_item_id, SavedSwapTable { saved_swap, .. })| saved_swap)
                        .map(json::from_value)
                        .map(|res: Result<SavedSwap, _>| {
                            res.map_to_mm(|e| SavedSwapError::ErrorDeserializing(e.to_string()))
                        })
                        .collect::<Result<Vec<SavedSwap>, _>>()?;
                    for swap in swaps {
                        let (filter_id, mut filter_record) = match filters_table
                            .get_item_by_unique_index("uuid", swap.uuid())
                            .await
                            .map_mm_err()?
                        {
                            Some(f) => f,
                            None => {
                                warn!("No MySwapsFiltersTable for {}", swap.uuid());
                                continue;
                            },
                        };
                        filter_record.swap_type = LEGACY_SWAP_TYPE;
                        filter_record.is_finished = swap.is_finished().into();
                        filters_table
                            .replace_item(filter_id, &filter_record)
                            .await
                            .map_mm_err()?;
                    }
                },
                1 => break,
                unsupported => {
                    return MmError::err(SavedSwapError::InternalError(format!(
                        "Unsupported migration {unsupported}"
                    )))
                },
            }
            migration += 1;
            migration_table
                .add_item(&SwapsMigrationTable { migration })
                .await
                .map_mm_err()?;
        }

        info!("Swaps data migration is completed, new version {}", migration);
        Ok(())
    }

    impl From<CursorError> for SavedSwapError {
        fn from(e: CursorError) -> Self {
            SavedSwapError::CursorError(e.to_string())
        }
    }

    impl From<DbTransactionError> for SavedSwapError {
        fn from(e: DbTransactionError) -> Self {
            let desc = e.to_string();
            match e {
                DbTransactionError::NoSuchTable { .. }
                | DbTransactionError::ErrorCreatingTransaction(_)
                | DbTransactionError::ErrorOpeningTable { .. }
                | DbTransactionError::ErrorSerializingIndex { .. }
                | DbTransactionError::MultipleItemsByUniqueIndex { .. }
                | DbTransactionError::NoSuchIndex { .. }
                | DbTransactionError::InvalidIndex { .. }
                | DbTransactionError::UnexpectedState(_)
                | DbTransactionError::TransactionAborted => SavedSwapError::InternalError(desc),
                DbTransactionError::ErrorDeserializingItem(_) => SavedSwapError::ErrorDeserializing(desc),
                DbTransactionError::ErrorSerializingItem(_) => SavedSwapError::ErrorSerializing(desc),
                DbTransactionError::ErrorGettingItems(_) | DbTransactionError::ErrorCountingItems(_) => {
                    SavedSwapError::ErrorLoading(desc)
                },
                DbTransactionError::ErrorUploadingItem(_) | DbTransactionError::ErrorDeletingItems(_) => {
                    SavedSwapError::ErrorSaving(desc)
                },
            }
        }
    }

    impl From<InitDbError> for SavedSwapError {
        fn from(e: InitDbError) -> Self {
            SavedSwapError::InternalError(e.to_string())
        }
    }

    #[async_trait]
    impl SavedSwapIo for SavedSwap {
        async fn load_my_swap_from_db(
            ctx: &MmArc,
            _address_dir: Option<&str>,
            uuid: Uuid,
        ) -> SavedSwapResult<Option<SavedSwap>> {
            let swaps_ctx = SwapsContext::from_ctx(ctx).map_to_mm(SavedSwapError::InternalError)?;
            let db = swaps_ctx.swap_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<SavedSwapTable>().await.map_mm_err()?;

            let saved_swap_json = match table.get_item_by_unique_index("uuid", uuid).await.map_mm_err()? {
                Some((_item_id, SavedSwapTable { saved_swap, .. })) => saved_swap,
                None => return Ok(None),
            };

            json::from_value(saved_swap_json).map_to_mm(|e| SavedSwapError::ErrorDeserializing(e.to_string()))
        }

        async fn load_all_my_swaps_from_db(ctx: &MmArc) -> SavedSwapResult<Vec<SavedSwap>> {
            let swaps_ctx = SwapsContext::from_ctx(ctx).map_to_mm(SavedSwapError::InternalError)?;
            let db = swaps_ctx.swap_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<SavedSwapTable>().await.map_mm_err()?;

            let swaps = table.get_all_items().await.map_mm_err()?;
            swaps
                .into_iter()
                .map(|(_item_id, SavedSwapTable { saved_swap, .. })| saved_swap)
                .map(json::from_value)
                .map(|res: Result<SavedSwap, _>| res.map_to_mm(|e| SavedSwapError::ErrorDeserializing(e.to_string())))
                .collect()
        }

        async fn save_to_db(&self, ctx: &MmArc) -> SavedSwapResult<()> {
            let saved_swap = json::to_value(self).map_to_mm(|e| SavedSwapError::ErrorSerializing(e.to_string()))?;
            let saved_swap_item = SavedSwapTable {
                uuid: *self.uuid(),
                saved_swap,
            };

            let swaps_ctx = SwapsContext::from_ctx(ctx).map_to_mm(SavedSwapError::InternalError)?;
            let db = swaps_ctx.swap_db().await.map_mm_err()?;
            let transaction = db.transaction().await.map_mm_err()?;
            let table = transaction.table::<SavedSwapTable>().await.map_mm_err()?;

            table
                .replace_item_by_unique_index("uuid", *self.uuid(), &saved_swap_item)
                .await
                .map_mm_err()?;
            Ok(())
        }
    }
}

#[cfg(target_arch = "wasm32")]
mod tests {
    use super::*;
    use crate::lp_swap::swap_wasm_db::{ItemId, MySwapsFiltersTable, SavedSwapTable, SwapsMigrationTable};
    use crate::lp_swap::{SwapsContext, LEGACY_SWAP_TYPE};
    use mm2_core::mm_ctx::MmCtxBuilder;
    use serde_json as json;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    async fn get_all_items(ctx: &MmArc) -> Vec<(ItemId, SavedSwapTable)> {
        let swaps_ctx = SwapsContext::from_ctx(ctx).unwrap();
        let db = swaps_ctx.swap_db().await.expect("Error getting SwapDb");
        let transaction = db.transaction().await.expect("Error creating transaction");
        let table = transaction
            .table::<SavedSwapTable>()
            .await
            .expect("Error opening table");
        table.get_all_items().await.expect("Error getting items")
    }

    #[wasm_bindgen_test]
    async fn test_saved_swap_table() {
        let ctx = MmCtxBuilder::new().with_test_db_namespace().into_mm_arc();

        let saved_swap_str = r#"{"type":"Maker","error_events":["StartFailed","NegotiateFailed","TakerFeeValidateFailed","MakerPaymentTransactionFailed","MakerPaymentDataSendFailed","TakerPaymentValidateFailed","TakerPaymentSpendFailed","TakerPaymentSpendConfirmFailed","MakerPaymentRefunded","MakerPaymentRefundFailed"],"events":[{"event":{"data":{"lock_duration":7800,"maker_amount":"3.54932734","maker_coin":"KMD","maker_coin_start_block":1452970,"maker_payment_confirmations":1,"maker_payment_lock":1563759539,"my_persistent_pub":"031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","secret":"e1c9bd12a83f810813dc078ac398069b63d56bf1e94657def995c43cd1975302","started_at":1563743939,"taker":"101ace6b08605b9424b0582b5cce044b70a3c8d8d10cb2965e039b0967ae92b9","taker_amount":"0.02004833998671660000000000","taker_coin":"ETH","taker_coin_start_block":8196380,"taker_payment_confirmations":1,"uuid":"3447b727-fe93-4357-8e5a-8cf2699b7e86"},"type":"Started"},"timestamp":1563743939211},{"event":{"data":{"taker_payment_locktime":1563751737,"taker_pubkey":"03101ace6b08605b9424b0582b5cce044b70a3c8d8d10cb2965e039b0967ae92b9"},"type":"Negotiated"},"timestamp":1563743979835},{"event":{"data":{"tx_hash":"a59203eb2328827de00bed699a29389792906e4f39fdea145eb40dc6b3821bd6","tx_hex":"f8690284ee6b280082520894d8997941dd1346e9231118d5685d866294f59e5b865af3107a4000801ca0743d2b7c9fad65805d882179062012261be328d7628ae12ee08eff8d7657d993a07eecbd051f49d35279416778faa4664962726d516ce65e18755c9b9406a9c2fd"},"type":"TakerFeeValidated"},"timestamp":1563744052878},{"event":{"data":{"error":"lp_swap:1888] eth:654] RPC error: Error { code: ServerError(-32010), message: \"Transaction with the same hash was already imported.\", data: None }"},"type":"MakerPaymentTransactionFailed"},"timestamp":1563744118577},{"event":{"type":"Finished"},"timestamp":1563763243350}],"success_events":["Started","Negotiated","TakerFeeValidated","MakerPaymentSent","TakerPaymentReceived","TakerPaymentWaitConfirmStarted","TakerPaymentValidatedAndConfirmed","TakerPaymentSpent","TakerPaymentSpendConfirmStarted","TakerPaymentSpendConfirmed","TakerPaymentSpendConfirmStarted","TakerPaymentSpendConfirmed","Finished"],"uuid":"3447b727-fe93-4357-8e5a-8cf2699b7e86"}"#;
        let mut saved_swap: SavedSwap = json::from_str(saved_swap_str).unwrap();
        let first_saved_item = SavedSwapTable {
            uuid: *saved_swap.uuid(),
            saved_swap: json::to_value(&saved_swap).unwrap(),
        };

        saved_swap.save_to_db(&ctx).await.expect("!save_to_db");

        let first_item_id = {
            let items = get_all_items(&ctx).await;
            assert_eq!(items.len(), 1);
            let (first_item_id, item) = items.into_iter().next().unwrap();
            assert_eq!(item, first_saved_item);
            first_item_id
        };

        saved_swap.hide_secrets();

        let second_saved_item = SavedSwapTable {
            uuid: *saved_swap.uuid(),
            saved_swap: json::to_value(&saved_swap).unwrap(),
        };
        assert_ne!(first_saved_item, second_saved_item);

        saved_swap.save_to_db(&ctx).await.expect("!save_to_db");

        {
            let items = get_all_items(&ctx).await;
            assert_eq!(items.len(), 1);
            let (second_item_id, item) = items.into_iter().next().unwrap();
            assert_eq!(first_item_id, second_item_id);
            assert_eq!(item, second_saved_item);
        }

        let actual_saved_swap = SavedSwap::load_my_swap_from_db(&ctx, None, *saved_swap.uuid())
            .await
            .expect("!load_from_db")
            .expect("Swap not found");
        let actual_saved_item = SavedSwapTable {
            uuid: *actual_saved_swap.uuid(),
            saved_swap: json::to_value(actual_saved_swap).unwrap(),
        };
        assert_eq!(actual_saved_item, second_saved_item);
    }

    #[wasm_bindgen_test]
    async fn test_get_current_migration() {
        let ctx = MmCtxBuilder::new().with_test_db_namespace().into_mm_arc();

        let swaps_ctx = SwapsContext::from_ctx(&ctx).unwrap();
        let db = swaps_ctx.swap_db().await.expect("Error getting SwapDb");
        let transaction = db.transaction().await.expect("Error creating transaction");
        let table = transaction
            .table::<SwapsMigrationTable>()
            .await
            .expect("Error opening table");

        table.add_item(&SwapsMigrationTable { migration: 1 }).await.unwrap();
        table.add_item(&SwapsMigrationTable { migration: 2 }).await.unwrap();
        table.add_item(&SwapsMigrationTable { migration: 3 }).await.unwrap();

        let migration = wasm_impl::get_current_migration(&table).await.unwrap();
        assert_eq!(migration, 3);
    }

    #[wasm_bindgen_test]
    async fn test_migrate_swaps_data() {
        let ctx = MmCtxBuilder::new().with_test_db_namespace().into_mm_arc();

        let saved_swap_str = r#"{"type":"Maker","error_events":["StartFailed","NegotiateFailed","TakerFeeValidateFailed","MakerPaymentTransactionFailed","MakerPaymentDataSendFailed","TakerPaymentValidateFailed","TakerPaymentSpendFailed","TakerPaymentSpendConfirmFailed","MakerPaymentRefunded","MakerPaymentRefundFailed"],"events":[{"event":{"data":{"lock_duration":7800,"maker_amount":"3.54932734","maker_coin":"KMD","maker_coin_start_block":1452970,"maker_payment_confirmations":1,"maker_payment_lock":1563759539,"my_persistent_pub":"031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","secret":"e1c9bd12a83f810813dc078ac398069b63d56bf1e94657def995c43cd1975302","started_at":1563743939,"taker":"101ace6b08605b9424b0582b5cce044b70a3c8d8d10cb2965e039b0967ae92b9","taker_amount":"0.02004833998671660000000000","taker_coin":"ETH","taker_coin_start_block":8196380,"taker_payment_confirmations":1,"uuid":"3447b727-fe93-4357-8e5a-8cf2699b7e86"},"type":"Started"},"timestamp":1563743939211},{"event":{"data":{"taker_payment_locktime":1563751737,"taker_pubkey":"03101ace6b08605b9424b0582b5cce044b70a3c8d8d10cb2965e039b0967ae92b9"},"type":"Negotiated"},"timestamp":1563743979835},{"event":{"data":{"tx_hash":"a59203eb2328827de00bed699a29389792906e4f39fdea145eb40dc6b3821bd6","tx_hex":"f8690284ee6b280082520894d8997941dd1346e9231118d5685d866294f59e5b865af3107a4000801ca0743d2b7c9fad65805d882179062012261be328d7628ae12ee08eff8d7657d993a07eecbd051f49d35279416778faa4664962726d516ce65e18755c9b9406a9c2fd"},"type":"TakerFeeValidated"},"timestamp":1563744052878},{"event":{"data":{"error":"lp_swap:1888] eth:654] RPC error: Error { code: ServerError(-32010), message: \"Transaction with the same hash was already imported.\", data: None }"},"type":"MakerPaymentTransactionFailed"},"timestamp":1563744118577},{"event":{"type":"Finished"},"timestamp":1563763243350}],"success_events":["Started","Negotiated","TakerFeeValidated","MakerPaymentSent","TakerPaymentReceived","TakerPaymentWaitConfirmStarted","TakerPaymentValidatedAndConfirmed","TakerPaymentSpent","TakerPaymentSpendConfirmStarted","TakerPaymentSpendConfirmed","TakerPaymentSpendConfirmStarted","TakerPaymentSpendConfirmed","Finished"],"uuid":"3447b727-fe93-4357-8e5a-8cf2699b7e86"}"#;
        let saved_swap: SavedSwap = json::from_str(saved_swap_str).unwrap();
        saved_swap.save_to_db(&ctx).await.expect("!save_to_db");

        let swaps_ctx = SwapsContext::from_ctx(&ctx).unwrap();
        {
            let db = swaps_ctx.swap_db().await.expect("Error getting SwapDb");
            let transaction = db.transaction().await.expect("Error creating transaction");
            let table = transaction
                .table::<MySwapsFiltersTable>()
                .await
                .expect("Error opening table");

            let filter = MySwapsFiltersTable {
                uuid: *saved_swap.uuid(),
                my_coin: saved_swap.maker_coin_ticker().unwrap(),
                other_coin: saved_swap.taker_coin_ticker().unwrap(),
                started_at: 0,
                is_finished: false.into(),
                swap_type: 0,
            };
            table.add_item(&filter).await.unwrap();
        }

        wasm_impl::migrate_swaps_data(&ctx).await.unwrap();

        let db = swaps_ctx.swap_db().await.expect("Error getting SwapDb");
        let transaction = db.transaction().await.expect("Error creating transaction");
        let table = transaction
            .table::<MySwapsFiltersTable>()
            .await
            .expect("Error opening table");

        let (_, item) = table
            .get_item_by_unique_index("uuid", saved_swap.uuid())
            .await
            .unwrap()
            .unwrap();

        assert!(item.is_finished.as_bool());

        let migration_table = transaction
            .table::<SwapsMigrationTable>()
            .await
            .expect("Error opening table");
        let current_migration = wasm_impl::get_current_migration(&migration_table).await.unwrap();
        assert_eq!(current_migration, 1);
    }
}
