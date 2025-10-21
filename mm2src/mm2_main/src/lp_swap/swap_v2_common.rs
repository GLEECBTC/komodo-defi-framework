use crate::lp_network::{subscribe_to_topic, unsubscribe_from_topic};
use crate::lp_swap::maker_swap_v2::{MakerSwapDbRepr, MakerSwapStateMachine, MakerSwapStorage};
use crate::lp_swap::swap_lock::{SwapLock, SwapLockError, SwapLockOps};
use crate::lp_swap::taker_swap_v2::{TakerSwapDbRepr, TakerSwapStateMachine, TakerSwapStorage};
use crate::lp_swap::{swap_v2_topic, SwapsContext};
use coins::{lp_coinfind, MakerCoinSwapOpsV2, MmCoin, MmCoinEnum, TakerCoinSwapOpsV2};
use common::executor::abortable_queue::AbortableQueue;
use common::executor::{SpawnFuture, Timer};
use common::log::{error, info, warn};
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_state_machine::storable_state_machine::{StateMachineDbRepr, StateMachineStorage, StorableStateMachine};
use rpc::v1::types::Bytes as BytesJson;
use secp256k1::PublicKey;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Error;
use uuid::Uuid;

cfg_native!(
    use common::async_blocking;
    use crate::database::my_swaps::{
        does_swap_exist, get_swap_events, update_swap_events, select_unfinished_swaps_uuids, set_swap_is_finished,
    };
);

cfg_wasm32!(
    use common::bool_as_int::BoolAsInt;
    use crate::lp_swap::swap_wasm_db::{IS_FINISHED_SWAP_TYPE_INDEX, MySwapsFiltersTable, SavedSwapTable};
    use mm2_db::indexed_db::{DbTransactionError, InitDbError, MultiIndex};
);

/// Information about active swap to be stored in swaps context
pub struct ActiveSwapV2Info {
    pub uuid: Uuid,
    pub maker_coin: String,
    pub taker_coin: String,
    pub swap_type: u8,
}

/// DB representation of tx preimage with signature
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct StoredTxPreimage {
    pub preimage: BytesJson,
    pub signature: BytesJson,
}

/// Represents error variants, which can happen on swaps re-creation
#[derive(Debug, Display)]
pub enum SwapRecreateError {
    /// DB representation has empty events
    ReprEventsEmpty,
    /// Failed to parse some data from DB representation (e.g. transactions, pubkeys, etc.)
    FailedToParseData(String),
    /// Swap has been aborted
    SwapAborted,
    /// Swap has been completed
    SwapCompleted,
    /// Swap has been finished with refund
    SwapFinishedWithRefund,
}

/// Represents errors that can be produced by [`MakerSwapStateMachine`] or [`TakerSwapStateMachine`] run.
#[derive(Debug, Display)]
pub enum SwapStateMachineError {
    StorageError(String),
    SerdeError(String),
    SwapLockAlreadyAcquired,
    SwapLock(SwapLockError),
    #[cfg(target_arch = "wasm32")]
    NoSwapWithUuid(Uuid),
}

impl From<SwapLockError> for SwapStateMachineError {
    fn from(e: SwapLockError) -> Self {
        SwapStateMachineError::SwapLock(e)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<db_common::sqlite::rusqlite::Error> for SwapStateMachineError {
    fn from(e: db_common::sqlite::rusqlite::Error) -> Self {
        SwapStateMachineError::StorageError(e.to_string())
    }
}

impl From<serde_json::Error> for SwapStateMachineError {
    fn from(e: Error) -> Self {
        SwapStateMachineError::SerdeError(e.to_string())
    }
}

#[cfg(target_arch = "wasm32")]
impl From<InitDbError> for SwapStateMachineError {
    fn from(e: InitDbError) -> Self {
        SwapStateMachineError::StorageError(e.to_string())
    }
}

#[cfg(target_arch = "wasm32")]
impl From<DbTransactionError> for SwapStateMachineError {
    fn from(e: DbTransactionError) -> Self {
        SwapStateMachineError::StorageError(e.to_string())
    }
}

pub struct SwapRecreateCtx<MakerCoin, TakerCoin> {
    pub maker_coin: MakerCoin,
    pub taker_coin: TakerCoin,
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn has_db_record_for(ctx: MmArc, id: &Uuid) -> MmResult<bool, SwapStateMachineError> {
    let id_str = id.to_string();
    Ok(async_blocking(move || does_swap_exist(&ctx.sqlite_connection(), &id_str)).await?)
}

#[cfg(target_arch = "wasm32")]
pub(super) async fn has_db_record_for(ctx: MmArc, id: &Uuid) -> MmResult<bool, SwapStateMachineError> {
    let swaps_ctx = SwapsContext::from_ctx(&ctx).expect("SwapsContext::from_ctx should not fail");
    let db = swaps_ctx.swap_db().await.map_mm_err()?;
    let transaction = db.transaction().await.map_mm_err()?;
    let table = transaction.table::<MySwapsFiltersTable>().await.map_mm_err()?;
    let maybe_item = table.get_item_by_unique_index("uuid", id).await.map_mm_err()?;
    Ok(maybe_item.is_some())
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn store_swap_event<T: StateMachineDbRepr>(
    ctx: MmArc,
    id: Uuid,
    event: T::Event,
) -> MmResult<(), SwapStateMachineError>
where
    T::Event: DeserializeOwned + Serialize + Send + 'static,
{
    let id_str = id.to_string();
    async_blocking(move || {
        let events_json = get_swap_events(&ctx.sqlite_connection(), &id_str)?;
        let mut events: Vec<T::Event> = serde_json::from_str(&events_json)?;
        events.push(event);
        drop_mutability!(events);
        let serialized_events = serde_json::to_string(&events)?;
        update_swap_events(&ctx.sqlite_connection(), &id_str, &serialized_events)?;
        Ok(())
    })
    .await
}

#[cfg(target_arch = "wasm32")]
pub(super) async fn store_swap_event<T: StateMachineDbRepr + DeserializeOwned + Serialize + Send + 'static>(
    ctx: MmArc,
    id: Uuid,
    event: T::Event,
) -> MmResult<(), SwapStateMachineError> {
    let swaps_ctx = SwapsContext::from_ctx(&ctx).expect("SwapsContext::from_ctx should not fail");
    let db = swaps_ctx.swap_db().await.map_mm_err()?;
    let transaction = db.transaction().await.map_mm_err()?;
    let table = transaction.table::<SavedSwapTable>().await.map_mm_err()?;

    let saved_swap_json = match table.get_item_by_unique_index("uuid", id).await.map_mm_err()? {
        Some((_item_id, SavedSwapTable { saved_swap, .. })) => saved_swap,
        None => return MmError::err(SwapStateMachineError::NoSwapWithUuid(id)),
    };

    let mut swap_repr: T = serde_json::from_value(saved_swap_json)?;
    swap_repr.add_event(event);

    let new_item = SavedSwapTable {
        uuid: id,
        saved_swap: serde_json::to_value(swap_repr)?,
    };
    table
        .replace_item_by_unique_index("uuid", id, &new_item)
        .await
        .map_mm_err()?;
    Ok(())
}

#[cfg(target_arch = "wasm32")]
pub(super) async fn get_swap_repr<T: DeserializeOwned>(ctx: &MmArc, id: Uuid) -> MmResult<T, SwapStateMachineError> {
    let swaps_ctx = SwapsContext::from_ctx(ctx).expect("SwapsContext::from_ctx should not fail");
    let db = swaps_ctx.swap_db().await.map_mm_err()?;
    let transaction = db.transaction().await.map_mm_err()?;

    let table = transaction.table::<SavedSwapTable>().await.map_mm_err()?;
    let saved_swap_json = match table.get_item_by_unique_index("uuid", id).await.map_mm_err()? {
        Some((_item_id, SavedSwapTable { saved_swap, .. })) => saved_swap,
        None => return MmError::err(SwapStateMachineError::NoSwapWithUuid(id)),
    };

    let swap_repr = serde_json::from_value(saved_swap_json)?;
    Ok(swap_repr)
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn get_unfinished_swaps_uuids(
    ctx: MmArc,
    swap_type: u8,
) -> MmResult<Vec<Uuid>, SwapStateMachineError> {
    async_blocking(move || {
        select_unfinished_swaps_uuids(&ctx.sqlite_connection(), swap_type)
            .map_to_mm(|e| SwapStateMachineError::StorageError(e.to_string()))
    })
    .await
}

#[cfg(target_arch = "wasm32")]
pub(super) async fn get_unfinished_swaps_uuids(
    ctx: MmArc,
    swap_type: u8,
) -> MmResult<Vec<Uuid>, SwapStateMachineError> {
    let index = MultiIndex::new(IS_FINISHED_SWAP_TYPE_INDEX)
        .with_value(BoolAsInt::new(false))
        .map_mm_err()?
        .with_value(swap_type)
        .map_mm_err()?;

    let swaps_ctx = SwapsContext::from_ctx(&ctx).expect("SwapsContext::from_ctx should not fail");
    let db = swaps_ctx.swap_db().await.map_mm_err()?;
    let transaction = db.transaction().await.map_mm_err()?;
    let table = transaction.table::<MySwapsFiltersTable>().await.map_mm_err()?;
    let table_items = table.get_items_by_multi_index(index).await.map_mm_err()?;

    Ok(table_items.into_iter().map(|(_item_id, item)| item.uuid).collect())
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn mark_swap_as_finished(ctx: MmArc, id: Uuid) -> MmResult<(), SwapStateMachineError> {
    async_blocking(move || Ok(set_swap_is_finished(&ctx.sqlite_connection(), &id.to_string())?)).await
}

#[cfg(target_arch = "wasm32")]
pub(super) async fn mark_swap_as_finished(ctx: MmArc, id: Uuid) -> MmResult<(), SwapStateMachineError> {
    let swaps_ctx = SwapsContext::from_ctx(&ctx).expect("SwapsContext::from_ctx should not fail");
    let db = swaps_ctx.swap_db().await.map_mm_err()?;
    let transaction = db.transaction().await.map_mm_err()?;
    let table = transaction.table::<MySwapsFiltersTable>().await.map_mm_err()?;
    let mut item = match table.get_item_by_unique_index("uuid", id).await.map_mm_err()? {
        Some((_item_id, item)) => item,
        None => return MmError::err(SwapStateMachineError::NoSwapWithUuid(id)),
    };
    item.is_finished = true.into();
    table
        .replace_item_by_unique_index("uuid", id, &item)
        .await
        .map_mm_err()?;
    Ok(())
}

pub(super) fn init_additional_context_impl(ctx: &MmArc, swap_info: ActiveSwapV2Info, other_p2p_pubkey: PublicKey) {
    subscribe_to_topic(ctx, swap_v2_topic(&swap_info.uuid));
    let swap_ctx = SwapsContext::from_ctx(ctx).expect("SwapsContext::from_ctx should not fail");
    swap_ctx.init_msg_v2_store(swap_info.uuid, other_p2p_pubkey);
    swap_ctx
        .active_swaps_v2_infos
        .lock()
        .unwrap()
        .insert(swap_info.uuid, swap_info);
}

pub(super) fn clean_up_context_impl(ctx: &MmArc, uuid: &Uuid, maker_coin: &str, taker_coin: &str) {
    unsubscribe_from_topic(ctx, swap_v2_topic(uuid));
    let swap_ctx = SwapsContext::from_ctx(ctx).expect("SwapsContext::from_ctx should not fail");
    swap_ctx.remove_msg_v2_store(uuid);
    swap_ctx.active_swaps_v2_infos.lock().unwrap().remove(uuid);

    let mut locked_amounts = swap_ctx.locked_amounts.lock().unwrap();
    if let Some(maker_coin_locked) = locked_amounts.get_mut(maker_coin) {
        maker_coin_locked.retain(|locked| locked.swap_uuid != *uuid);
    }

    if let Some(taker_coin_locked) = locked_amounts.get_mut(taker_coin) {
        taker_coin_locked.retain(|locked| locked.swap_uuid != *uuid);
    }
}

pub(super) async fn acquire_reentrancy_lock_impl(ctx: &MmArc, uuid: Uuid) -> MmResult<SwapLock, SwapStateMachineError> {
    let mut attempts = 0;
    loop {
        match SwapLock::lock(ctx, uuid, 40.).await.map_mm_err()? {
            Some(l) => break Ok(l),
            None => {
                if attempts >= 1 {
                    break MmError::err(SwapStateMachineError::SwapLockAlreadyAcquired);
                } else {
                    warn!("Swap {} file lock already acquired, retrying in 40 seconds", uuid);
                    attempts += 1;
                    Timer::sleep(40.).await;
                }
            },
        }
    }
}

pub(super) fn spawn_reentrancy_lock_renew_impl(abortable_system: &AbortableQueue, uuid: Uuid, guard: SwapLock) {
    let fut = async move {
        loop {
            match guard.touch().await {
                Ok(_) => (),
                Err(e) => warn!("Swap {} file lock error: {}", uuid, e),
            };
            Timer::sleep(30.).await;
        }
    };
    abortable_system.weak_spawner().spawn(fut);
}

pub(super) trait GetSwapCoins {
    fn maker_coin(&self) -> &str;

    fn taker_coin(&self) -> &str;
}

/// Attempts to find and return the maker and taker coins required for the swap to proceed.
/// If a coin is not activated, it logs the information and retries until the coin is found.
/// If an unexpected issue occurs, function logs the error and returns `None`.
pub(super) async fn swap_kickstart_coins<T: GetSwapCoins>(
    ctx: &MmArc,
    swap_repr: &T,
    uuid: &Uuid,
) -> Option<(MmCoinEnum, MmCoinEnum)> {
    let taker_coin_ticker = swap_repr.taker_coin();

    let taker_coin = loop {
        match lp_coinfind(ctx, taker_coin_ticker).await {
            Ok(Some(c)) => break c,
            Ok(None) => {
                info!(
                    "Can't kickstart the swap {} until the coin {} is activated",
                    uuid, taker_coin_ticker,
                );
                Timer::sleep(1.).await;
            },
            Err(e) => {
                error!("Error {} on {} find attempt", e, taker_coin_ticker);
                return None;
            },
        };
    };

    let maker_coin_ticker = swap_repr.maker_coin();

    let maker_coin = loop {
        match lp_coinfind(ctx, maker_coin_ticker).await {
            Ok(Some(c)) => break c,
            Ok(None) => {
                info!(
                    "Can't kickstart the swap {} until the coin {} is activated",
                    uuid, maker_coin_ticker,
                );
                Timer::sleep(1.).await;
            },
            Err(e) => {
                error!("Error {} on {} find attempt", e, maker_coin_ticker);
                return None;
            },
        };
    };

    Some((maker_coin, taker_coin))
}

/// Handles the recreation and kickstart of a swap state machine.
pub(super) async fn swap_kickstart_handler<
    T: StorableStateMachine<RecreateCtx = SwapRecreateCtx<MakerCoin, TakerCoin>>,
    MakerCoin: MmCoin + MakerCoinSwapOpsV2,
    TakerCoin: MmCoin + TakerCoinSwapOpsV2,
>(
    swap_repr: <T::Storage as StateMachineStorage>::DbRepr,
    storage: T::Storage,
    uuid: <T::Storage as StateMachineStorage>::MachineId,
    maker_coin: MakerCoin,
    taker_coin: TakerCoin,
) where
    <T::Storage as StateMachineStorage>::MachineId: Copy + std::fmt::Display,
    T::Error: std::fmt::Display,
    T::RecreateError: std::fmt::Display,
{
    let recreate_context = SwapRecreateCtx { maker_coin, taker_coin };

    let (mut state_machine, state) = match T::recreate_machine(uuid, storage, swap_repr, recreate_context).await {
        Ok((machine, from_state)) => (machine, from_state),
        Err(e) => {
            error!("Error {} on trying to recreate the swap {}", e, uuid);
            return;
        },
    };

    if let Err(e) = state_machine.kickstart(state).await {
        error!("Error {} on trying to run the swap {}", e, uuid);
    }
}

pub(super) async fn swap_kickstart_handler_for_maker(
    ctx: MmArc,
    swap_repr: MakerSwapDbRepr,
    storage: MakerSwapStorage,
    uuid: Uuid,
) {
    if let Some((maker_coin, taker_coin)) = swap_kickstart_coins(&ctx, &swap_repr, &uuid).await {
        match (maker_coin, taker_coin) {
            (MmCoinEnum::UtxoCoin(m), MmCoinEnum::UtxoCoin(t)) => {
                swap_kickstart_handler::<MakerSwapStateMachine<_, _>, _, _>(swap_repr, storage, uuid, m, t).await
            },
            (MmCoinEnum::EthCoin(m), MmCoinEnum::EthCoin(t)) => {
                swap_kickstart_handler::<MakerSwapStateMachine<_, _>, _, _>(swap_repr, storage, uuid, m, t).await
            },
            (MmCoinEnum::UtxoCoin(m), MmCoinEnum::EthCoin(t)) => {
                swap_kickstart_handler::<MakerSwapStateMachine<_, _>, _, _>(swap_repr, storage, uuid, m, t).await
            },
            (MmCoinEnum::EthCoin(m), MmCoinEnum::UtxoCoin(t)) => {
                swap_kickstart_handler::<MakerSwapStateMachine<_, _>, _, _>(swap_repr, storage, uuid, m, t).await
            },
            _ => {
                error!(
                    "V2 swaps are not currently supported for {}/{} pair",
                    swap_repr.maker_coin(),
                    swap_repr.taker_coin()
                );
            },
        }
    }
}

pub(super) async fn swap_kickstart_handler_for_taker(
    ctx: MmArc,
    swap_repr: TakerSwapDbRepr,
    storage: TakerSwapStorage,
    uuid: Uuid,
) {
    if let Some((maker_coin, taker_coin)) = swap_kickstart_coins(&ctx, &swap_repr, &uuid).await {
        match (maker_coin, taker_coin) {
            (MmCoinEnum::UtxoCoin(m), MmCoinEnum::UtxoCoin(t)) => {
                swap_kickstart_handler::<TakerSwapStateMachine<_, _>, _, _>(swap_repr, storage, uuid, m, t).await
            },
            (MmCoinEnum::EthCoin(m), MmCoinEnum::EthCoin(t)) => {
                swap_kickstart_handler::<TakerSwapStateMachine<_, _>, _, _>(swap_repr, storage, uuid, m, t).await
            },
            (MmCoinEnum::UtxoCoin(m), MmCoinEnum::EthCoin(t)) => {
                swap_kickstart_handler::<TakerSwapStateMachine<_, _>, _, _>(swap_repr, storage, uuid, m, t).await
            },
            (MmCoinEnum::EthCoin(m), MmCoinEnum::UtxoCoin(t)) => {
                swap_kickstart_handler::<TakerSwapStateMachine<_, _>, _, _>(swap_repr, storage, uuid, m, t).await
            },
            _ => {
                error!(
                    "V2 swaps are not currently supported for {}/{} pair",
                    swap_repr.maker_coin(),
                    swap_repr.taker_coin()
                );
            },
        }
    }
}
