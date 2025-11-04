//! Trading Protocol Upgrade (“swap v2”) policy: confirms, 0‑conf windows, visibility, and caps
//!
//! Canonical reference for confirmation and visibility policy used by the swap v2 state machines.
//! Other modules should link here rather than restating details.
//!
//! What this covers
//! - Default gating policy (who waits for what, and where)
//! - Visibility grace windows and polling
//! - Where and how the gates are enforced in code
//! - High‑level rationale (EVM vs UTXO)
//! - Future Design considerations
//!
//! At‑a‑glance defaults:
//! - Maker side (on taker funding):
//!   - Proceed on mempool visibility (0‑conf) by default.
//!     Flag: [`MakerSwapStateMachine::require_taker_funding_confirm_before_maker_payment`] = `false`.
//!     Guarded by the visibility window: [`SWAP_TX_VISIBILITY_GRACE_SECS`] with polling [`SWAP_TX_VISIBILITY_POLL_SECS`].
//!
//! - Taker side (on maker payment):
//!   - Require on‑chain confirmation of maker payment before broadcasting the taker funding spend.
//!     Flag: [`TakerSwapStateMachine::require_maker_payment_confirm_before_funding_spend`] = `true`.
//!
//! - Post‑spend confirmation gates:
//!   - Maker requires taker payment spend confirmation before completing:
//!     Flag: [`MakerSwapStateMachine::require_taker_payment_spend_confirm`] = `true`.
//!   - Taker requires maker payment spend confirmation before completing:
//!     Flag: [`TakerSwapStateMachine::require_maker_payment_spend_confirm`] = `true`.
//!
//! - Visibility grace (best‑effort mempool discovery):
//!   - Window: [`SWAP_TX_VISIBILITY_GRACE_SECS`] seconds, polled every [`SWAP_TX_VISIBILITY_POLL_SECS`] seconds.
//!   - Used to avoid failing early when a tx is temporarily invisible but broadcastable (rebroadcast fallback included).
//!
//! Rationale
//! - UTXO chains often have 1‑conf > typical taker waiting windows (BTC ~10m, LTC ~2.5m, KMD ~60s).
//!   Proceeding on 0‑conf taker funding keeps swaps fast, at the cost of occasional maker locks if funding is dropped/replaced.
//! - Taker must not risk a replaced maker payment (e.g., RBF), hence the taker’s confirmation gate before spending the funding.
//! - EVM chains have short blocks (~12–15s), but we keep consistent policy across families; 0‑conf visibility gating is still used.
//!
//! Where this is enforced (high level)
//! - Maker (taker funding gate):
//!   - In maker state “taker funding received”, the maker first validates funding, then:
//!     - If `require_taker_funding_confirm_before_maker_payment == false` (default): ensure mempool visibility within
//!       [`SWAP_TX_VISIBILITY_GRACE_SECS`], else abort to refund path.
//!     - If `true`: require 1‑conf (capped by the taker time window), else abort to refund path.
//!   - On success, maker sends maker payment and shares the taker‑funding spend preimage.
//!
//! - Taker (maker payment gate):
//!   - In taker state “maker payment and funding spend preimage received”, the taker validates maker payment,
//!     ensures mempool visibility within [`SWAP_TX_VISIBILITY_GRACE_SECS`], then:
//!     - If `require_maker_payment_confirm_before_funding_spend == true` (default): wait confirmations before spending funding.
//!     - If `false` (non‑default, opt‑in): may spend after visibility; still re‑check before any spend is ever broadcast.
//!
//! - Post‑spend gates (completion):
//!   - Maker waits for taker payment spend confirmation if
//!     [`MakerSwapStateMachine::require_taker_payment_spend_confirm`] is `true` (default).
//!   - Taker waits for maker payment spend confirmation if
//!     [`TakerSwapStateMachine::require_maker_payment_spend_confirm`] is `true` (default).
//!
//! Constants (this module)
//! - [`SWAP_TX_VISIBILITY_GRACE_SECS`]: best‑effort mempool visibility window (seconds).
//! - [`SWAP_TX_VISIBILITY_POLL_SECS`]: poll interval (seconds) while waiting for visibility.
//!
//! Related timeouts
//! - Negotiation phase: see `NEGOTIATION_TIMEOUT_SEC`.
//! - Chain confirmation waits: use per‑coin confirmations/notarizations via `ConfirmPaymentInput` at call sites.
//! - Lock‑time policy: see [`crate::lp_swap::lp_atomic_locktime_v2`] and helpers in `lp_swap.rs`.
//!
//! Notes for maintainers
//! - If you change defaults above, update:
//!   - This doc,
//!   - The default field values on the state machines,
//!   - Any unit/integration tests relying on the gates/timings.
//! - Visibility gating uses a rebroadcast‑and‑poll loop; keep values conservative for public RPCs.
//!
//! Future design considerations (non‑normative; suggestions, not commitments)
//!
//! - Use 0‑conf to enable maker competition:
//!   Makers can compete on speed as well as price by offering fast success path swaps to build a track record.
//!   Risk comes from pre‑confirmation drops/replacements, which can lead to maker funds being locked.
//!
//! - Client trust annotations:
//!   Clients can keep local trust annotations (allowlists/overrides) informed by the current stats DB:
//!   successful swap counts, time‑weighted confirmed volume, and failures attributable to the maker.
//!   This is a stopgap until reputation, stake/slashing, or better solutions are in place.
//!   “Trusted” makers may benefit from confirmations, as larger‑volume takers will tend to select them even if swaps
//!   take longer; they can still require 0‑conf, so the choice remains with makers.
//!   New makers can compete on speed, not only pricing, to earn trust.
//!
//! - Removing the funding transaction:
//!   If explored, this would reduce fees and latency by skipping taker funding, but it introduces a backout risk:
//!   maker funds can remain locked while the taker sends nothing, but it's worth considering as it's' not less risky
//!   than requiring 0‑conf for funding.
//!   This would also be up to makers, who choose allowed volumes and parameters to compete for volume and trust/reputation.

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

/// Best‑effort mempool visibility grace period (seconds).
/// Set to ~2× the average Ethereum block time (~15s → 30s).
/// Rationale: avoid failing early when propagation is temporarily slow (e.g., private/MEV relays,
/// node lag), while still keeping swaps fast by proceeding as soon as the tx is reasonably
/// expected to be discoverable by its txid. This is not a confirmation wait, only a visibility window.
pub(super) const SWAP_TX_VISIBILITY_GRACE_SECS: f64 = 30.0;
/// Poll interval (seconds) while waiting for tx visibility within the grace window.
/// Default 1s balances responsiveness and RPC load for public mempools.
/// DISCUSS: On private/MEV‑protected relays the tx may remain invisible until inclusion,
/// consider a longer interval (e.g., 2–3s) or adaptive backoff to reduce unnecessary requests.
pub(super) const SWAP_TX_VISIBILITY_POLL_SECS: f64 = 1.0;

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
            (MmCoinEnum::UtxoCoinVariant(m), MmCoinEnum::UtxoCoinVariant(t)) => {
                swap_kickstart_handler::<MakerSwapStateMachine<_, _>, _, _>(swap_repr, storage, uuid, m, t).await
            },
            (MmCoinEnum::EthCoinVariant(m), MmCoinEnum::EthCoinVariant(t)) => {
                swap_kickstart_handler::<MakerSwapStateMachine<_, _>, _, _>(swap_repr, storage, uuid, m, t).await
            },
            (MmCoinEnum::UtxoCoinVariant(m), MmCoinEnum::EthCoinVariant(t)) => {
                swap_kickstart_handler::<MakerSwapStateMachine<_, _>, _, _>(swap_repr, storage, uuid, m, t).await
            },
            (MmCoinEnum::EthCoinVariant(m), MmCoinEnum::UtxoCoinVariant(t)) => {
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
            (MmCoinEnum::UtxoCoinVariant(m), MmCoinEnum::UtxoCoinVariant(t)) => {
                swap_kickstart_handler::<TakerSwapStateMachine<_, _>, _, _>(swap_repr, storage, uuid, m, t).await
            },
            (MmCoinEnum::EthCoinVariant(m), MmCoinEnum::EthCoinVariant(t)) => {
                swap_kickstart_handler::<TakerSwapStateMachine<_, _>, _, _>(swap_repr, storage, uuid, m, t).await
            },
            (MmCoinEnum::UtxoCoinVariant(m), MmCoinEnum::EthCoinVariant(t)) => {
                swap_kickstart_handler::<TakerSwapStateMachine<_, _>, _, _>(swap_repr, storage, uuid, m, t).await
            },
            (MmCoinEnum::EthCoinVariant(m), MmCoinEnum::UtxoCoinVariant(t)) => {
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
