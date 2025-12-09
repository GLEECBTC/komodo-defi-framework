//! State machine for taker aggregated swap: liquidity routing swap (opt) + atomic swap + liquidity routing swap (opt)

use super::lr_errors::LrSwapError;
use super::lr_helpers;
use super::{AtomicSwapParams, LrSwapParams};
use crate::common::executor::SpawnAbortable;
use crate::common::log::LogOnError;
use crate::lp_ordermatch::lp_auto_buy;
use crate::lp_swap::swap_lock::SwapLock;
use crate::lp_swap::swap_v2_common::*;
use crate::lp_swap::swap_v2_rpcs::{my_swap_status_rpc, MySwapStatusError, MySwapStatusRequest, SwapRpcData};
use crate::lp_swap::taker_swap_v2;
use crate::lp_swap::{
    check_my_coin_balance_for_swap, check_other_coin_balance_for_swap, create_taker_swap_default_params,
    get_locked_amount, CheckBalanceError, CheckBalanceResult, TakerFeeAdditionalInfo, AGG_TAKER_SWAP_TYPE,
};
use crate::lr_swap::lr_helpers::{make_atomic_swap_request, make_classic_swap_create_params};
use async_trait::async_trait;
use coins::eth::{u256_to_big_decimal, wei_from_coins_mm_number, EthCoin, EthCoinType};
use coins::hd_wallet::DisplayAddress;
use coins::{
    is_eth_platform_coin, lp_coinfind_or_err, ConfirmPaymentInput, FeeApproxStage, MarketCoinOps, RawTransactionRes,
    SignEthTransactionParams, SignRawTransactionEnum, SignRawTransactionRequest,
};
use coins::{DexFee, MmCoin};
use coins::{MmCoinEnum, Ticker};
use common::executor::abortable_queue::AbortableQueue;
use common::executor::AbortSettings;
use common::executor::AbortableSystem;
use common::executor::Timer;
use common::log::{debug, info, warn};
use common::Future01CompatExt;
use common::{new_uuid, now_sec};
use derive_more::Display;
use ethereum_types::{Address as EthAddress, U256};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::map_mm_error::MapMmError;
use mm2_err_handle::prelude::*;
use mm2_number::{BigDecimal, MmNumber};
use mm2_rpc::data::legacy::{Mm2RpcResult, SellBuyResponse, TakerAction};
use mm2_state_machine::prelude::*;
use mm2_state_machine::storable_state_machine::*;
use num_traits::{CheckedDiv, FromPrimitive};
use rpc::v1::types::Bytes as BytesJson;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::OnceLock;
use trading_api::one_inch_api::classic_swap_types::ClassicSwapData;
use trading_api::one_inch_api::client::{ApiClient, SwapApiMethods, SwapUrlBuilder};
use uuid::Uuid;

const LR_SWAP_CONFIRMATIONS: u64 = 1;
const LR_SWAP_WAIT_CONFIRM_TIMEOUT_SEC: u64 = 60;
const LR_SWAP_WAIT_CONFIRM_INTERVAL_SEC: u64 = 15;

cfg_native!(
    use common::async_blocking;
    use db_common::sqlite::rusqlite::{named_params, Error as SqlError, Result as SqlResult, Row};
    use db_common::sqlite::rusqlite::types::Type as SqlType;
    use crate::database::my_lr_swaps::insert_new_lr_swap;
    use crate::database::my_lr_swaps::SELECT_LR_SWAP_BY_UUID;
);

cfg_wasm32!(
    use crate::lp_swap::SwapsContext;
    use crate::lp_swap::swap_wasm_db::{MySwapsFiltersTable, SavedSwapTable};
    use crate::swap_versioning::legacy_swap_version;
);

const LOG_SWAP_LR_NAME: &str = "Taker swap with LR";

/// Represents events produced by aggregated taker swap states.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "event_type", content = "event_data")]
pub enum AggTakerSwapEvent {
    /// Run LR-swap step before atomic swap
    RunLrSwap0 {},
    /// Waiting for LR_0 tx to confirm
    WaitingForLrTxConfirmation0 {
        coin: Ticker,
        tx_bytes: BytesJson,
        dst_amount: MmNumber,
        slippage: f32,
    },
    /// Atomic swap has been successfully started
    StartAtomicSwap {
        taker_volume: MmNumber,
        action: TakerAction,
        slippage: Option<f32>,
    },
    /// Waiting for running atomic swap
    WaitingForAtomicSwap {
        atomic_swap_uuid: Uuid,
        maker_volume: MmNumber,
    },
    /// Run LR-swap step after atomic swap
    RunLrSwap1 {},
    /// Waiting for LR_1 tx to confirm
    WaitingForLrTxConfirmation1 { coin: Ticker, tx_bytes: BytesJson },
    /// Waiting for LR_0 rollback tx to confirm
    WaitForLrRollbackTxConfirmation0 {
        coin: Ticker,
        tx_bytes: BytesJson,
        atomic_swap_aborted: bool,
        atomic_swap_abort_reason: Option<taker_swap_v2::AbortReason>,
    },
    /// Try to send back funds received on LR_0, if atomic swap did not complete
    TryRefundLr0 {
        atomic_swap_aborted: bool,
        atomic_swap_abort_reason: Option<taker_swap_v2::AbortReason>,
    },
    /// Taker swap with LR has been aborted before any payment was sent.
    Aborted { reason: states::AbortReason },
    /// Taker swap with LR completed successfully.
    Completed,
    /// Taker swap with LR finished w/o errors, funds refunded.
    FinishedRefunded,
}

/// Storage for aggregated swaps
#[derive(Clone)]
pub struct AggTakerSwapStorage {
    ctx: MmArc,
}

impl AggTakerSwapStorage {
    pub fn new(ctx: MmArc) -> Self {
        AggTakerSwapStorage { ctx }
    }
}

#[async_trait]
impl StateMachineStorage for AggTakerSwapStorage {
    type MachineId = Uuid;
    type DbRepr = AggTakerSwapDbRepr;
    type Error = MmError<SwapStateMachineError>;

    /// NOTE: the state machine state is saved only once, on-init.
    /// If we modify some properties in progress, we need to restore them from events, when recreating the state machine
    #[cfg(not(target_arch = "wasm32"))]
    async fn store_repr(&mut self, _id: Self::MachineId, repr: Self::DbRepr) -> Result<(), Self::Error> {
        let ctx = self.ctx.clone();

        async_blocking(move || {
            let lr_swap_0_ser = if let Some(lr_swap_0) = repr.lr_swap_0 {
                serde_json::to_string(&lr_swap_0)
                    .map_to_mm(|ser_err| SwapStateMachineError::SerdeError(ser_err.to_string()))?
            } else {
                Default::default() // We should insert empty string to let sqlite know about the field type
            };
            let sell_buy_req_ser = serde_json::to_string(&repr.atomic_swap)
                .map_to_mm(|ser_err| SwapStateMachineError::SerdeError(ser_err.to_string()))?;
            let lr_swap_1_ser = if let Some(lr_swap_1) = repr.lr_swap_1 {
                serde_json::to_string(&lr_swap_1)
                    .map_to_mm(|ser_err| SwapStateMachineError::SerdeError(ser_err.to_string()))?
            } else {
                Default::default() // We should insert empty string to let sqlite know about the field type
            };
            let sql_params = named_params! {
                ":my_coin": repr.taker_coin,
                ":other_coin": repr.maker_coin,
                ":uuid": repr.uuid.to_string(),
                ":started_at": repr.started_at,
                ":swap_type": AGG_TAKER_SWAP_TYPE,
                ":swap_version": repr.swap_version,
                ":taker_volume": repr.source_volume.to_fraction_string(),
                ":maker_volume": repr.destination_volume.to_fraction_string(),
                ":lr_swap_0": lr_swap_0_ser,
                ":sell_buy_req": sell_buy_req_ser,
                ":lr_swap_1": lr_swap_1_ser,
            };
            insert_new_lr_swap(&ctx, sql_params)?;
            Ok(())
        })
        .await
    }

    #[cfg(target_arch = "wasm32")]
    async fn store_repr(&mut self, uuid: Self::MachineId, repr: Self::DbRepr) -> Result<(), Self::Error> {
        let swaps_ctx = SwapsContext::from_ctx(&self.ctx).expect("SwapsContext::from_ctx should not fail");
        let db = swaps_ctx.swap_db().await.map_mm_err()?;
        let transaction = db.transaction().await.map_mm_err()?;
        let filters_table = transaction.table::<MySwapsFiltersTable>().await.map_mm_err()?;

        let item = MySwapsFiltersTable {
            uuid,
            my_coin: repr.taker_coin.clone(),
            other_coin: repr.maker_coin.clone(),
            started_at: repr.started_at as u32,
            is_finished: false.into(),
            swap_type: AGG_TAKER_SWAP_TYPE,
        };
        filters_table.add_item(&item).await.map_mm_err()?;

        let table = transaction.table::<SavedSwapTable>().await.map_mm_err()?;
        let item = SavedSwapTable {
            uuid,
            saved_swap: serde_json::to_value(repr)?,
        };
        table.add_item(&item).await.map_mm_err()?;
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn get_repr(&self, id: Self::MachineId) -> Result<Self::DbRepr, Self::Error> {
        let ctx = self.ctx.clone();
        let id_str = id.to_string();

        async_blocking(move || {
            Ok(ctx.sqlite_connection().query_row(
                SELECT_LR_SWAP_BY_UUID,
                &[(":uuid", &id_str)],
                Self::DbRepr::from_sql_row,
            )?)
        })
        .await
    }

    #[cfg(target_arch = "wasm32")]
    async fn get_repr(&self, id: Self::MachineId) -> Result<Self::DbRepr, Self::Error> {
        get_swap_repr(&self.ctx, id).await
    }

    async fn has_record_for(&mut self, id: &Self::MachineId) -> Result<bool, Self::Error> {
        has_db_record_for(self.ctx.clone(), id).await
    }

    async fn store_event(&mut self, id: Self::MachineId, event: AggTakerSwapEvent) -> Result<(), Self::Error> {
        store_swap_event::<AggTakerSwapDbRepr>(self.ctx.clone(), id, event).await
    }

    async fn get_unfinished(&self) -> Result<Vec<Self::MachineId>, Self::Error> {
        get_unfinished_swaps_uuids(self.ctx.clone(), AGG_TAKER_SWAP_TYPE).await
    }

    async fn mark_finished(&mut self, id: Self::MachineId) -> Result<(), Self::Error> {
        mark_swap_as_finished(self.ctx.clone(), id).await
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AggTakerSwapDbRepr {
    pub maker_coin: String,
    pub taker_coin: String,
    pub uuid: Uuid,
    pub started_at: u64,
    /// Is swap finished, the value is set only from db
    pub is_finished: bool,
    pub source_volume: MmNumber,
    pub destination_volume: MmNumber,
    pub lr_swap_0: Option<LrSwapParams>,
    pub lr_swap_1: Option<LrSwapParams>,
    pub atomic_swap: AtomicSwapParams,
    pub events: Vec<AggTakerSwapEvent>,
    pub swap_version: u8,
}

impl StateMachineDbRepr for AggTakerSwapDbRepr {
    type Event = AggTakerSwapEvent;

    fn add_event(&mut self, event: Self::Event) {
        self.events.push(event)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl AggTakerSwapDbRepr {
    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) async fn get_repr_impl(ctx: &MmArc, id: &Uuid) -> SqlResult<Self> {
        let id_str = id.to_string();
        let ctx = ctx.clone();

        async_blocking(move || {
            ctx.sqlite_connection()
                .query_row(SELECT_LR_SWAP_BY_UUID, &[(":uuid", &id_str)], Self::from_sql_row)
        })
        .await
    }

    fn from_sql_row(row: &Row) -> SqlResult<Self> {
        Ok(AggTakerSwapDbRepr {
            taker_coin: row.get(0)?,
            maker_coin: row.get(1)?,
            uuid: row
                .get::<_, String>(2)?
                .parse()
                .map_err(|e| SqlError::FromSqlConversionFailure(2, SqlType::Text, Box::new(e)))?,
            started_at: row.get(3)?,
            is_finished: row.get(4)?,
            events: serde_json::from_str(&row.get::<_, String>(5)?)
                .map_err(|e| SqlError::FromSqlConversionFailure(5, SqlType::Text, Box::new(e)))?,
            swap_version: row.get(6)?,
            source_volume: MmNumber::from_fraction_string(&row.get::<_, String>(7)?)
                .map_err(|e| SqlError::FromSqlConversionFailure(7, SqlType::Text, Box::new(e)))?,
            destination_volume: MmNumber::from_fraction_string(&row.get::<_, String>(8)?)
                .map_err(|e| SqlError::FromSqlConversionFailure(8, SqlType::Text, Box::new(e)))?,
            lr_swap_0: serde_json::from_str::<LrSwapParams>(&row.get::<_, String>(9)?).ok(),
            atomic_swap: serde_json::from_str::<AtomicSwapParams>(&row.get::<_, String>(10)?)
                .map_err(|e| SqlError::FromSqlConversionFailure(10, SqlType::Text, Box::new(e)))?,
            lr_swap_1: serde_json::from_str::<LrSwapParams>(&row.get::<_, String>(11)?).ok(),
        })
    }
}

impl AggTakerSwapDbRepr {
    pub(crate) fn source_coin(&self) -> Ticker {
        if let Some(ref lr_swap_0) = self.lr_swap_0 {
            lr_swap_0.src.clone()
        } else {
            self.atomic_swap.taker_coin()
        }
    }

    pub(crate) fn destination_coin(&self) -> Ticker {
        if let Some(ref lr_swap_1) = self.lr_swap_1 {
            lr_swap_1.dst.clone()
        } else {
            self.atomic_swap.maker_coin()
        }
    }

    pub(crate) fn routing_coin_0(&self) -> Option<Ticker> {
        self.lr_swap_0.as_ref().map(|p| p.dst.clone())
    }

    pub(crate) fn routing_coin_1(&self) -> Option<Ticker> {
        self.lr_swap_1.as_ref().map(|p| p.dst.clone())
    }
}

/// Represents the state machine for maker's side of the Trading Protocol Upgrade swap (v2).
pub struct AggTakerSwapStateMachine {
    /// MM2 context
    ctx: MmArc,
    /// Storage
    storage: AggTakerSwapStorage,
    /// Abortable queue used to spawn related activities
    abortable_system: AbortableQueue,
    /// Params for the LR swap step running before the atomic swap (optional)
    lr_swap_0: Option<LrSwapParams>,
    /// Params for the LR swap step running after the atomic swap (optional)
    lr_swap_1: Option<LrSwapParams>,
    /// Sell or buy params for the atomic swap
    atomic_swap: AtomicSwapParams,
    /// LR_0 step destination volume (used for rollback)
    lr_0_dst_amount: OnceLock<MmNumber>,
    /// The UUID of the atomic swap
    atomic_swap_uuid: OnceLock<Uuid>,
    /// unique ID of the aggregated swap for stopping or querying status.
    uuid: Uuid,
    /// The timestamp when the aggregated swap was started.
    started_at: u64,
    /// Swap impl version
    swap_version: u8,
}

#[async_trait]
impl StorableStateMachine for AggTakerSwapStateMachine {
    type Storage = AggTakerSwapStorage;
    type Result = ();
    type Error = MmError<SwapStateMachineError>;
    type ReentrancyLock = SwapLock;
    type RecreateCtx = ();
    type RecreateError = MmError<SwapRecreateError>;

    fn to_db_repr(&self) -> AggTakerSwapDbRepr {
        AggTakerSwapDbRepr {
            maker_coin: self.atomic_swap_maker_coin(), // TODO: maybe we don't need these two fields and rely on lr_swap_0 lr_swap_1 and sell_buy_req
            taker_coin: self.atomic_swap_taker_coin(),
            uuid: self.uuid,
            started_at: self.started_at,
            source_volume: self.source_volume().unwrap_or_default(), // Safe as swap must be aborted if no source_volume
            destination_volume: self.destination_volume(),
            is_finished: false,
            lr_swap_0: self.lr_swap_0.clone(),
            lr_swap_1: self.lr_swap_1.clone(),
            atomic_swap: self.atomic_swap.clone(),
            events: Vec::new(),
            swap_version: self.swap_version,
        }
    }

    fn storage(&mut self) -> &mut Self::Storage {
        &mut self.storage
    }

    fn id(&self) -> <Self::Storage as StateMachineStorage>::MachineId {
        self.uuid
    }

    async fn recreate_machine(
        uuid: Uuid,
        storage: AggTakerSwapStorage,
        mut repr: AggTakerSwapDbRepr,
        _recreate_ctx: Self::RecreateCtx,
    ) -> Result<(RestoredMachine<Self>, Box<dyn RestoredState<StateMachine = Self>>), Self::RecreateError> {
        let current_state: Box<dyn RestoredState<StateMachine = Self>> = match repr
            .events
            .pop()
            .ok_or(MmError::new(SwapRecreateError::ReprEventsEmpty))?
        {
            AggTakerSwapEvent::StartAtomicSwap {
                taker_volume,
                action,
                slippage,
            } => Box::new(states::StartAtomicSwap {
                taker_volume,
                action,
                slippage,
            }),
            AggTakerSwapEvent::RunLrSwap0 {} => Box::new(states::RunLrSwap0 {
                lr_swap_params: repr
                    .lr_swap_0
                    .as_ref()
                    .ok_or(SwapRecreateError::FailedToParseData("LR data empty".to_string()))?
                    .clone(),
            }),
            AggTakerSwapEvent::RunLrSwap1 {} => Box::new(states::RunLrSwap1 {
                lr_swap_params: repr
                    .lr_swap_1
                    .as_ref()
                    .ok_or(SwapRecreateError::FailedToParseData("LR data empty".to_string()))?
                    .clone(),
                src_amount: Default::default(), // TODO:
            }),

            AggTakerSwapEvent::Aborted { .. } => return MmError::err(SwapRecreateError::SwapAborted),
            AggTakerSwapEvent::Completed => return MmError::err(SwapRecreateError::SwapCompleted),
            _ => return MmError::err(SwapRecreateError::FailedToParseData("".to_string())),
        };

        let machine = AggTakerSwapStateMachine {
            ctx: storage.ctx.clone(),
            abortable_system: storage
                .ctx
                .abortable_system
                .create_subsystem()
                .expect("create_subsystem should not fail"),
            storage,
            lr_swap_0: repr.lr_swap_0.clone(),
            lr_swap_1: repr.lr_swap_1.clone(),
            atomic_swap: repr.atomic_swap.clone(),
            atomic_swap_uuid: OnceLock::default(),
            lr_0_dst_amount: OnceLock::default(),
            uuid,
            swap_version: AggTakerSwapStateMachine::AGG_SWAP_VERSION,
            started_at: 0,
        };

        if let Some(atomic_swap_uuid) = Self::find_atomic_swap_uuid_in_events(&repr.events) {
            let _ = machine.atomic_swap_uuid.set(atomic_swap_uuid);
        }
        if let Some(lr_0_dst_amount) = Self::find_lr_0_dst_amount_in_events(&repr.events) {
            let _ = machine.lr_0_dst_amount.set(lr_0_dst_amount);
        }
        Ok((RestoredMachine::new(machine), current_state))
    }

    async fn acquire_reentrancy_lock(&self) -> Result<Self::ReentrancyLock, Self::Error> {
        acquire_reentrancy_lock_impl(&self.ctx, self.uuid).await
    }

    fn spawn_reentrancy_lock_renew(&mut self, guard: Self::ReentrancyLock) {
        spawn_reentrancy_lock_renew_impl(&self.abortable_system, self.uuid, guard)
    }

    fn init_additional_context(&mut self) {
        let swap_info = ActiveSwapV2Info {
            uuid: self.uuid,
            maker_coin: self.atomic_swap_maker_coin(),
            taker_coin: self.atomic_swap_taker_coin(),
            swap_type: AGG_TAKER_SWAP_TYPE,
        };
        init_agg_swap_context_impl(&self.ctx, swap_info);
    }

    fn clean_up_context(&mut self) {
        clean_up_agg_swap_context_impl(&self.ctx, &self.uuid);
    }

    fn on_event(&mut self, event: &AggTakerSwapEvent) {
        match event {
            AggTakerSwapEvent::StartAtomicSwap { .. } => {
                // TODO: No need to lock LR swap amounts?
            },
            AggTakerSwapEvent::Aborted { .. } | AggTakerSwapEvent::Completed => (),
            _ => (),
        }

        // TODO: add streamer call
        // Send a notification to the swap status streamer about a new event.
        /*self.ctx
        .event_stream_manager
        .send_fn(SwapStatusStreamer::derive_streamer_id(), || SwapStatusEvent::AggTaker {
            uuid: self.uuid,
            event: event.clone(),
        })
        .ok();*/
    }

    fn on_kickstart_event(&mut self, _event: AggTakerSwapEvent) {}
}

mod states {
    use super::*;

    /// Represents a state used to start a new aggregated taker swap.
    #[derive(Default)]
    pub struct Initialize {}

    impl InitialState for Initialize {
        type StateMachine = AggTakerSwapStateMachine;
    }

    #[async_trait]
    impl State for Initialize {
        type StateMachine = AggTakerSwapStateMachine;

        async fn on_changed(
            self: Box<Self>,
            state_machine: &mut Self::StateMachine,
        ) -> StateResult<Self::StateMachine> {
            info!(
                "{LOG_SWAP_LR_NAME} uuid: {} for {}/{} with amount {} starting...",
                state_machine.uuid,
                state_machine.source_coin(),
                state_machine.destination_coin(),
                state_machine.source_volume().unwrap_or_default().to_decimal()
            );
            if let Some(ref lr_swap_params) = state_machine.lr_swap_0 {
                let run_lr_swap = RunLrSwap0 {
                    lr_swap_params: lr_swap_params.clone(),
                };
                return Self::change_state(run_lr_swap, state_machine).await;
            }
            let Ok(taker_volume) = state_machine.source_volume() else {
                info!("{LOG_SWAP_LR_NAME} failed: {}", "internal error: no source volume");
                let next_state = Aborted {
                    reason: AbortReason::SomeReason("internal error: no source volume".to_owned()),
                };
                return Self::change_state(next_state, state_machine).await;
            };
            let run_atomic_swap = StartAtomicSwap {
                taker_volume,
                action: state_machine.atomic_swap.action.clone(),
                // TODO: currently slippage must be None, if no LR_0 step.
                // Maybe we will want to have slippage for atomic swaps as well, then this may change
                slippage: None,
            };
            Self::change_state(run_atomic_swap, state_machine).await
        }
    }

    /// State to start atomic swap step
    pub(super) struct StartAtomicSwap {
        pub(super) taker_volume: MmNumber,
        pub(super) action: TakerAction,
        pub(super) slippage: Option<f32>,
    }

    #[async_trait]
    impl State for StartAtomicSwap {
        type StateMachine = AggTakerSwapStateMachine;

        async fn on_changed(
            self: Box<Self>,
            state_machine: &mut Self::StateMachine,
        ) -> StateResult<Self::StateMachine> {
            let atomic_start_result = state_machine
                .start_lp_auto_buy(self.taker_volume.clone(), self.slippage)
                .await;
            match atomic_start_result {
                Ok(resp) => {
                    let _ = state_machine
                        .atomic_swap_uuid
                        .set(resp.request.uuid)
                        .expect("Atomic swap UUID should be empty");
                    // Get other party maker volume
                    let maker_volume = match resp.request.action {
                        TakerAction::Buy => resp.request.base_amount_rat.into(),
                        TakerAction::Sell => resp.request.rel_amount_rat.into(),
                    };
                    let next_state = WaitingForAtomicSwap {
                        atomic_swap_uuid: resp.request.uuid,
                        maker_volume,
                    };
                    Self::change_state(next_state, state_machine).await
                },
                Err(err) => {
                    info!("{LOG_SWAP_LR_NAME} failed: {}", err);
                    let next_state = Aborted {
                        reason: AbortReason::SomeReason(err.get_inner().to_string()),
                    };
                    Self::change_state(next_state, state_machine).await
                },
            }
        }
    }

    impl StorableState for StartAtomicSwap {
        type StateMachine = AggTakerSwapStateMachine;

        fn get_event(&self) -> AggTakerSwapEvent {
            AggTakerSwapEvent::StartAtomicSwap {
                taker_volume: self.taker_volume.clone(),
                action: self.action.clone(),
                slippage: self.slippage,
            }
        }
    }

    impl TransitionFrom<Initialize> for StartAtomicSwap {}
    impl TransitionFrom<WaitForLrTxConfirmation0> for StartAtomicSwap {}

    /// State to wait for the atomic swap step to finish
    pub(super) struct WaitingForAtomicSwap {
        atomic_swap_uuid: Uuid,
        maker_volume: MmNumber,
    }

    #[async_trait]
    impl State for WaitingForAtomicSwap {
        type StateMachine = AggTakerSwapStateMachine;

        async fn on_changed(
            self: Box<Self>,
            state_machine: &mut Self::StateMachine,
        ) -> StateResult<Self::StateMachine> {
            let result = state_machine.wait_for_atomic_swap_finished().await;
            if let Ok(is_completed) = result {
                info!(
                    "{LOG_SWAP_LR_NAME} {}: interim atomic swap {} finished: {}",
                    state_machine.uuid,
                    state_machine.atomic_swap_uuid.get().unwrap_or(&Uuid::nil()),
                    if is_completed { "success" } else { "refunded" }
                );
            }
            match result {
                Ok(true) => {
                    if let Some(ref lr_swap) = state_machine.lr_swap_1 {
                        let run_lr_swap = RunLrSwap1 {
                            lr_swap_params: lr_swap.clone(),
                            src_amount: self.maker_volume,
                        };
                        return Self::change_state(run_lr_swap, state_machine).await;
                    }
                    let completed = Completed {};
                    Self::change_state(completed, state_machine).await
                },
                Ok(false) => {
                    if state_machine.lr_swap_0.is_some() {
                        let try_refund = TryRefundLr0 {
                            atomic_swap_aborted: false,
                            atomic_swap_abort_reason: None,
                        };
                        return Self::change_state(try_refund, state_machine).await;
                    }
                    let finished = FinishedRefunded {};
                    Self::change_state(finished, state_machine).await
                },
                Err(err) => {
                    info!(
                        "{LOG_SWAP_LR_NAME}: interim atomic swap {} failed: {}",
                        state_machine.atomic_swap_uuid.get().unwrap_or(&Uuid::nil()),
                        err
                    );
                    if state_machine.lr_swap_0.is_some() {
                        let try_refund = TryRefundLr0 {
                            atomic_swap_aborted: true,
                            atomic_swap_abort_reason: if let LrSwapError::AtomicSwapAborted(reason) = err.get_inner() {
                                Some(reason.clone())
                            } else {
                                None
                            },
                        };
                        return Self::change_state(try_refund, state_machine).await;
                    }
                    let next_state = Aborted {
                        reason: AbortReason::SomeReason(err.get_inner().to_string()),
                    };
                    return Self::change_state(next_state, state_machine).await;
                },
            }
        }
    }

    impl StorableState for WaitingForAtomicSwap {
        type StateMachine = AggTakerSwapStateMachine;

        fn get_event(&self) -> AggTakerSwapEvent {
            AggTakerSwapEvent::WaitingForAtomicSwap {
                atomic_swap_uuid: self.atomic_swap_uuid,
                maker_volume: self.maker_volume.clone(),
            }
        }
    }

    impl TransitionFrom<StartAtomicSwap> for WaitingForAtomicSwap {}

    /// State to create and send a LR swap tx (before atomic swap)
    pub(super) struct RunLrSwap0 {
        pub lr_swap_params: LrSwapParams,
    }

    #[async_trait]
    impl State for RunLrSwap0 {
        type StateMachine = AggTakerSwapStateMachine;

        async fn on_changed(
            self: Box<Self>,
            state_machine: &mut Self::StateMachine,
        ) -> StateResult<Self::StateMachine> {
            let (base_ticker, tx_bytes, dst_amount) = match state_machine.run_lr_swap(&self.lr_swap_params, None).await
            {
                // No src_amount to be set on LR swap 0
                Ok((base_ticker, tx_bytes, dst_amount)) => (base_ticker, tx_bytes, dst_amount),
                Err(err) => {
                    info!("{LOG_SWAP_LR_NAME}: LR_0 swap failed: {}", err);
                    let aborted = Aborted {
                        reason: AbortReason::SomeReason(err.get_inner().to_string()),
                    };
                    return Self::change_state(aborted, state_machine).await;
                },
            };
            info!("{LOG_SWAP_LR_NAME}: LR_0 swap tx sent");
            let _ = state_machine.lr_0_dst_amount.set(dst_amount.clone());
            let next_state = WaitForLrTxConfirmation0 {
                coin: base_ticker,
                tx_bytes,
                dst_amount,
                slippage: self.lr_swap_params.slippage,
            };
            Self::change_state(next_state, state_machine).await
        }
    }

    impl StorableState for RunLrSwap0 {
        type StateMachine = AggTakerSwapStateMachine;

        fn get_event(&self) -> AggTakerSwapEvent {
            AggTakerSwapEvent::RunLrSwap0 {}
        }
    }

    impl TransitionFrom<Initialize> for RunLrSwap0 {}

    /// State to wait for confirmation of LR swap tx (before atomic swap)
    pub(super) struct WaitForLrTxConfirmation0 {
        coin: Ticker,
        tx_bytes: BytesJson,
        dst_amount: MmNumber,
        slippage: f32,
    }

    #[async_trait]
    impl State for WaitForLrTxConfirmation0 {
        type StateMachine = AggTakerSwapStateMachine;

        async fn on_changed(
            self: Box<Self>,
            state_machine: &mut Self::StateMachine,
        ) -> StateResult<Self::StateMachine> {
            if let Err(err) = state_machine
                .wait_for_lr_tx_confirmation(&self.coin, self.tx_bytes.clone())
                .await
            {
                info!("{LOG_SWAP_LR_NAME}: LR_0 tx confirmation failed: {}", err);
                let aborted = Aborted {
                    reason: AbortReason::SomeReason(format!("LR_0 tx confirmation failed: {}", err.get_inner())),
                };
                return Self::change_state(aborted, state_machine).await;
            }

            info!("{LOG_SWAP_LR_NAME}: LR_0 swap tx confirmed");
            let atomic_swap_volume = match state_machine.deduct_fees(self.dst_amount.clone()).await {
                Ok(atomic_swap_volume) => atomic_swap_volume,
                Err(err) => {
                    info!("{LOG_SWAP_LR_NAME}: deduct fees failed: {}", err);
                    let aborted = Aborted {
                        reason: AbortReason::SomeReason(err.get_inner().to_string()),
                    };
                    return Self::change_state(aborted, state_machine).await;
                },
            };

            let run_atomic_swap = StartAtomicSwap {
                taker_volume: atomic_swap_volume,
                action: state_machine.atomic_swap.action.clone(),
                slippage: Some(self.slippage),
            };
            Self::change_state(run_atomic_swap, state_machine).await
        }
    }

    impl StorableState for WaitForLrTxConfirmation0 {
        type StateMachine = AggTakerSwapStateMachine;

        fn get_event(&self) -> AggTakerSwapEvent {
            AggTakerSwapEvent::WaitingForLrTxConfirmation0 {
                coin: self.coin.clone(),
                tx_bytes: self.tx_bytes.clone(),
                dst_amount: self.dst_amount.clone(),
                slippage: self.slippage,
            }
        }
    }

    impl TransitionFrom<RunLrSwap0> for WaitForLrTxConfirmation0 {}

    /// State to create and send a LR swap tx (after atomic swap)
    pub(super) struct RunLrSwap1 {
        pub lr_swap_params: LrSwapParams,
        pub src_amount: MmNumber,
    }

    #[async_trait]
    impl State for RunLrSwap1 {
        type StateMachine = AggTakerSwapStateMachine;

        async fn on_changed(
            self: Box<Self>,
            state_machine: &mut Self::StateMachine,
        ) -> StateResult<Self::StateMachine> {
            let (base_ticker, tx_bytes, _) = match state_machine
                .run_lr_swap(&self.lr_swap_params, Some(self.src_amount))
                .await
            {
                // Set LR_swap_1 src amount as the atomic swap maker_volume
                Ok((base_ticker, tx_bytes, dst_amount)) => (base_ticker, tx_bytes, dst_amount),
                Err(err) => {
                    info!("{LOG_SWAP_LR_NAME}: LR_1 swap failed: {}", err);
                    let aborted = Aborted {
                        reason: AbortReason::SomeReason(err.get_inner().to_string()),
                    };
                    return Self::change_state(aborted, state_machine).await;
                },
            };
            info!("{LOG_SWAP_LR_NAME}: LR_1 swap tx sent");
            let wait_for_conf = WaitForLrTxConfirmation1 {
                coin: base_ticker,
                tx_bytes,
            };
            Self::change_state(wait_for_conf, state_machine).await
        }
    }

    impl StorableState for RunLrSwap1 {
        type StateMachine = AggTakerSwapStateMachine;

        fn get_event(&self) -> AggTakerSwapEvent {
            AggTakerSwapEvent::RunLrSwap1 {}
        }
    }

    impl TransitionFrom<WaitingForAtomicSwap> for RunLrSwap1 {}

    /// State to wait for confirmation of LR swap tx (after atomic swap)
    pub(super) struct WaitForLrTxConfirmation1 {
        coin: Ticker,
        tx_bytes: BytesJson,
    }

    #[async_trait]
    impl State for WaitForLrTxConfirmation1 {
        type StateMachine = AggTakerSwapStateMachine;

        async fn on_changed(
            self: Box<Self>,
            state_machine: &mut Self::StateMachine,
        ) -> StateResult<Self::StateMachine> {
            if let Err(err) = state_machine
                .wait_for_lr_tx_confirmation(&self.coin, self.tx_bytes.clone())
                .await
            {
                info!("{LOG_SWAP_LR_NAME} LR_1 tx confirmation failed: {}", err);
                let aborted = Aborted {
                    reason: AbortReason::SomeReason(err.get_inner().to_string()),
                };
                return Self::change_state(aborted, state_machine).await;
            }
            info!("{LOG_SWAP_LR_NAME}: LR_1 swap tx confirmed");
            let completed = Completed {};
            Self::change_state(completed, state_machine).await
        }
    }

    impl StorableState for WaitForLrTxConfirmation1 {
        type StateMachine = AggTakerSwapStateMachine;

        fn get_event(&self) -> AggTakerSwapEvent {
            AggTakerSwapEvent::WaitingForLrTxConfirmation1 {
                coin: self.coin.clone(),
                tx_bytes: self.tx_bytes.clone(),
            }
        }
    }

    impl TransitionFrom<RunLrSwap1> for WaitForLrTxConfirmation1 {}

    /// State to try to send back funds received on LR_0, if atomic swap did not complete
    pub(super) struct TryRefundLr0 {
        pub atomic_swap_aborted: bool,
        pub atomic_swap_abort_reason: Option<taker_swap_v2::AbortReason>,
    }

    #[async_trait]
    impl State for TryRefundLr0 {
        type StateMachine = AggTakerSwapStateMachine;

        async fn on_changed(
            self: Box<Self>,
            state_machine: &mut Self::StateMachine,
        ) -> StateResult<Self::StateMachine> {
            let Some(ref lr_swap_0) = state_machine.lr_swap_0 else {
                let aborted = Aborted {
                    reason: AbortReason::SomeReason("No LR_0".to_string()),
                };
                return Self::change_state(aborted, state_machine).await;
            };
            let Some(lr_0_dst_amount) = state_machine.lr_0_dst_amount.get() else {
                let aborted = Aborted {
                    reason: AbortReason::SomeReason("No LR_0 destination amount set".to_string()),
                };
                return Self::change_state(aborted, state_machine).await;
            };
            let (ticker, tx_bytes) = match state_machine.run_lr_rollback(lr_swap_0, lr_0_dst_amount.clone()).await {
                Ok((ticker, tx_bytes)) => (ticker, tx_bytes),
                Err(err) => {
                    info!("{LOG_SWAP_LR_NAME} LR_0 rollback failed: {}", err);
                    let aborted = Aborted {
                        reason: AbortReason::SomeReason(err.get_inner().to_string()),
                    };
                    return Self::change_state(aborted, state_machine).await;
                },
            };
            let wait_for_conf = WaitForLrRollbackTxConfirmation0 {
                coin: ticker,
                tx_bytes,
                atomic_swap_aborted: self.atomic_swap_aborted,
                atomic_swap_abort_reason: self.atomic_swap_abort_reason,
            };
            Self::change_state(wait_for_conf, state_machine).await
        }
    }

    impl StorableState for TryRefundLr0 {
        type StateMachine = AggTakerSwapStateMachine;

        fn get_event(&self) -> AggTakerSwapEvent {
            AggTakerSwapEvent::TryRefundLr0 {
                atomic_swap_aborted: self.atomic_swap_aborted,
                atomic_swap_abort_reason: self.atomic_swap_abort_reason.clone(),
            }
        }
    }

    impl TransitionFrom<WaitingForAtomicSwap> for TryRefundLr0 {}

    /// State to wait for confirmation of LR_0 roll back tx
    pub(super) struct WaitForLrRollbackTxConfirmation0 {
        coin: Ticker,
        tx_bytes: BytesJson,
        atomic_swap_aborted: bool,
        atomic_swap_abort_reason: Option<taker_swap_v2::AbortReason>,
    }

    #[async_trait]
    impl State for WaitForLrRollbackTxConfirmation0 {
        type StateMachine = AggTakerSwapStateMachine;

        async fn on_changed(
            self: Box<Self>,
            state_machine: &mut Self::StateMachine,
        ) -> StateResult<Self::StateMachine> {
            if let Err(err) = state_machine
                .wait_for_lr_tx_confirmation(&self.coin, self.tx_bytes.clone())
                .await
            {
                info!("{LOG_SWAP_LR_NAME} LR_0 rollback tx confirmation failed: {}", err);
                let aborted = Aborted {
                    reason: AbortReason::SomeReason(err.get_inner().to_string()),
                };
                return Self::change_state(aborted, state_machine).await;
            }
            let completed = Completed {};
            Self::change_state(completed, state_machine).await
        }
    }

    impl StorableState for WaitForLrRollbackTxConfirmation0 {
        type StateMachine = AggTakerSwapStateMachine;

        fn get_event(&self) -> AggTakerSwapEvent {
            AggTakerSwapEvent::WaitForLrRollbackTxConfirmation0 {
                coin: self.coin.clone(),
                tx_bytes: self.tx_bytes.clone(),
                atomic_swap_aborted: self.atomic_swap_aborted,
                atomic_swap_abort_reason: self.atomic_swap_abort_reason.clone(),
            }
        }
    }

    impl TransitionFrom<TryRefundLr0> for WaitForLrRollbackTxConfirmation0 {}

    /// Aggregated taker swap with LR completed state
    pub(super) struct Completed {}

    impl StorableState for Completed {
        type StateMachine = AggTakerSwapStateMachine;

        fn get_event(&self) -> AggTakerSwapEvent {
            AggTakerSwapEvent::Completed
        }
    }

    #[async_trait]
    impl LastState for Completed {
        type StateMachine = AggTakerSwapStateMachine;

        async fn on_changed(
            self: Box<Self>,
            state_machine: &mut Self::StateMachine,
        ) -> <Self::StateMachine as StateMachineTrait>::Result {
            info!("{LOG_SWAP_LR_NAME} {} has been completed", state_machine.uuid);
        }
    }

    impl TransitionFrom<WaitForLrTxConfirmation1> for Completed {}
    impl TransitionFrom<WaitingForAtomicSwap> for Completed {}
    impl TransitionFrom<WaitForLrRollbackTxConfirmation0> for Completed {}

    /// Aggregated taker swap with LR finished w/o error, with both atomic swap and LR_0 funds refunded
    pub(super) struct FinishedRefunded {}

    impl StorableState for FinishedRefunded {
        type StateMachine = AggTakerSwapStateMachine;

        fn get_event(&self) -> AggTakerSwapEvent {
            AggTakerSwapEvent::FinishedRefunded
        }
    }

    #[async_trait]
    impl LastState for FinishedRefunded {
        type StateMachine = AggTakerSwapStateMachine;

        async fn on_changed(
            self: Box<Self>,
            state_machine: &mut Self::StateMachine,
        ) -> <Self::StateMachine as StateMachineTrait>::Result {
            info!("{LOG_SWAP_LR_NAME} {} has been finished refunded", state_machine.uuid);
        }
    }

    impl TransitionFrom<WaitForLrRollbackTxConfirmation0> for FinishedRefunded {}
    impl TransitionFrom<WaitingForAtomicSwap> for FinishedRefunded {}

    /// Represents possible reasons of taker swap being aborted
    /// TODO: add reasons
    #[derive(Clone, Debug, Deserialize, Display, Serialize)]
    pub enum AbortReason {
        SomeReason(String),
    }

    /// Aggregated taker swap with LR aborted state
    struct Aborted {
        reason: AbortReason,
    }

    #[async_trait]
    impl LastState for Aborted {
        type StateMachine = AggTakerSwapStateMachine;

        async fn on_changed(
            self: Box<Self>,
            state_machine: &mut Self::StateMachine,
        ) -> <Self::StateMachine as StateMachineTrait>::Result {
            warn!(
                "{LOG_SWAP_LR_NAME} {} was aborted with reason {}",
                state_machine.uuid, self.reason
            );
        }
    }

    impl StorableState for Aborted {
        type StateMachine = AggTakerSwapStateMachine;

        fn get_event(&self) -> AggTakerSwapEvent {
            AggTakerSwapEvent::Aborted {
                reason: self.reason.clone(),
            }
        }
    }

    impl TransitionFrom<Initialize> for Aborted {}
    impl TransitionFrom<RunLrSwap0> for Aborted {}
    impl TransitionFrom<WaitForLrTxConfirmation0> for Aborted {}
    impl TransitionFrom<RunLrSwap1> for Aborted {}
    impl TransitionFrom<WaitForLrTxConfirmation1> for Aborted {}
    impl TransitionFrom<WaitingForAtomicSwap> for Aborted {}
    impl TransitionFrom<StartAtomicSwap> for Aborted {}
    impl TransitionFrom<TryRefundLr0> for Aborted {}
    impl TransitionFrom<WaitForLrRollbackTxConfirmation0> for Aborted {}
}

impl AggTakerSwapStateMachine {
    /// Current agg swap version
    const AGG_SWAP_VERSION: u8 = 0;

    #[allow(clippy::result_large_err)]
    pub(crate) fn source_volume(&self) -> Result<MmNumber, LrSwapError> {
        if let Some(ref lr_swap_0) = self.lr_swap_0 {
            Ok(lr_swap_0.src_amount.clone())
        } else {
            self.atomic_swap.taker_volume()
        }
    }

    pub(crate) fn destination_volume(&self) -> MmNumber {
        Default::default() // TODO: estimate
    }

    /// Source coin or token ticker in aggregated swap (before the first liquidity routing)
    #[allow(unused)]
    fn source_coin(&self) -> Ticker {
        if let Some(ref lr_swap_0) = self.lr_swap_0 {
            lr_swap_0.src.clone()
        } else {
            self.atomic_swap.taker_coin()
        }
    }

    /// Destination coin or token ticker in aggregated swap (after the final liquidity routing)
    #[allow(unused)]
    fn destination_coin(&self) -> Ticker {
        if let Some(ref lr_swap_1) = self.lr_swap_1 {
            lr_swap_1.dst.clone()
        } else {
            self.atomic_swap.maker_coin()
        }
    }

    fn atomic_swap_maker_coin(&self) -> Ticker {
        self.atomic_swap.maker_coin()
    }

    fn atomic_swap_taker_coin(&self) -> Ticker {
        self.atomic_swap.taker_coin()
    }

    pub(crate) fn find_atomic_swap_uuid_in_events(events: &[AggTakerSwapEvent]) -> Option<Uuid> {
        events.iter().find_map(|event| {
            if let AggTakerSwapEvent::WaitingForAtomicSwap { atomic_swap_uuid, .. } = event {
                Some(*atomic_swap_uuid)
            } else {
                None
            }
        })
    }

    fn find_lr_0_dst_amount_in_events(events: &[AggTakerSwapEvent]) -> Option<MmNumber> {
        events.iter().find_map(|event| {
            if let AggTakerSwapEvent::WaitingForLrTxConfirmation0 { dst_amount, .. } = event {
                Some(dst_amount.clone())
            } else {
                None
            }
        })
    }

    /// Execute liquidity routing interim swap.
    /// The source amount of the LR swap may be set from the previous step by the volume_from_prev_step param.
    async fn run_lr_swap(
        &self,
        lr_swap_params: &LrSwapParams,
        src_amount_from_prev_step: Option<MmNumber>,
    ) -> MmResult<(Ticker, BytesJson, MmNumber), LrSwapError> {
        let (src_coin, _) = lr_helpers::get_coin_for_one_inch(&self.ctx, &lr_swap_params.src).await?;
        let src_amount = if let Some(src_amount_from_prev_step) = src_amount_from_prev_step {
            src_amount_from_prev_step
        } else {
            lr_swap_params.src_amount.clone()
        };
        info!(
            "{LOG_SWAP_LR_NAME} starting liquidity routing step for {}/{} for amount {}",
            lr_swap_params.src,
            lr_swap_params.dst,
            src_amount.to_decimal()
        );
        let src_amount = wei_from_coins_mm_number(&src_amount, lr_swap_params.src_decimals)
            .mm_err(|err| LrSwapError::InvalidParam(err.to_string()))?;

        let (dst_amount, tx_bytes) = self
            .create_and_send_classic_swap_tx(
                &src_coin,
                lr_swap_params.src_contract,
                lr_swap_params.dst_contract,
                src_amount,
                lr_swap_params.from,
                lr_swap_params.slippage,
            )
            .await?;

        let dst_amount = u256_to_big_decimal(dst_amount, lr_swap_params.dst_decimals).map_mm_err()?;
        Ok((lr_swap_params.src.clone(), tx_bytes.tx_hex, dst_amount.into()))
    }

    /// Execute rollback of the previously routed dst_amount (on the LR_0 step) to return the source amount to the User,
    /// in case of something wrong was with the atomic swap.
    async fn run_lr_rollback(
        &self,
        lr_swap_params: &LrSwapParams,
        mut dst_amount: MmNumber,
    ) -> MmResult<(Ticker, BytesJson), LrSwapError> {
        let (dst_coin, _) = lr_helpers::get_coin_for_one_inch(&self.ctx, &lr_swap_params.dst).await?;
        info!(
            "{LOG_SWAP_LR_NAME} starting liquidity routing rollback for {}/{} for amount {}",
            lr_swap_params.dst,
            lr_swap_params.src,
            dst_amount.to_decimal()
        );
        // Check if available balance is insufficient for the LR_0 dst_amount (this may be due to slippage),
        // then spend avalable
        let balance: MmNumber = dst_coin.my_spendable_balance().compat().await.map_mm_err()?.into();
        let locked = get_locked_amount(&self.ctx, &lr_swap_params.dst);
        let mut avail = balance - locked;
        if is_eth_platform_coin!(dst_coin) {
            // We could also get gas_limit from ClassicSwapData::tx
            let swap_tx_fee = dst_coin
                .estimate_trade_fee(lr_swap_params.gas.into(), FeeApproxStage::StartSwap)
                .await
                .map_mm_err()?;
            avail -= swap_tx_fee.amount;
        }
        if avail < dst_amount {
            dst_amount = avail;
        }
        let dst_amount = wei_from_coins_mm_number(&dst_amount, lr_swap_params.dst_decimals).map_mm_err()?;
        let (_, tx_bytes) = self
            .create_and_send_classic_swap_tx(
                &dst_coin,
                lr_swap_params.dst_contract,
                lr_swap_params.src_contract,
                dst_amount,
                lr_swap_params.from,
                lr_swap_params.slippage,
            )
            .await?;
        Ok((lr_swap_params.dst.clone(), tx_bytes.tx_hex))
    }

    async fn create_and_send_classic_swap_tx(
        &self,
        src_coin: &EthCoin,
        src_contract: EthAddress,
        dst_contract: EthAddress,
        src_amount: U256,
        from: EthAddress,
        slippage: f32,
    ) -> MmResult<(U256, RawTransactionRes), LrSwapError> {
        let src_chain_id = src_coin.chain_id().ok_or(LrSwapError::ChainNotSupported)?;
        if let EthCoinType::Erc20 { .. } = src_coin.coin_type {
            let classic_swap_contract =
                EthAddress::from_str(ApiClient::classic_swap_contract()).expect("valid eth address");
            src_coin
                .handle_allowance(
                    classic_swap_contract,
                    src_amount,
                    now_sec() + LR_SWAP_WAIT_CONFIRM_TIMEOUT_SEC,
                ) // TODO: Refactor as std::time::Instant;
                .await?
        }

        let lr_swap_request = make_classic_swap_create_params(
            src_contract,
            dst_contract,
            src_amount,
            from,
            slippage,
            Default::default(),
        );

        let query_params = lr_swap_request.build_query_params().map_mm_err()?;
        let url = SwapUrlBuilder::create_api_url_builder(&self.ctx, src_chain_id, SwapApiMethods::ClassicSwapCreate)
            .map_mm_err()?
            .with_query_params(query_params)
            .build()
            .map_mm_err()?;
        let swap_with_tx: ClassicSwapData = ApiClient::call_api(url).await.map_mm_err()?;
        let tx_fields = swap_with_tx
            .tx
            .ok_or(LrSwapError::InternalError("TxFields empty".to_string()))?;

        let sign_params = SignRawTransactionEnum::ETH(SignEthTransactionParams {
            value: Some(u256_to_big_decimal(U256::from_dec_str(&tx_fields.value)?, src_coin.decimals()).map_mm_err()?),
            to: Some(tx_fields.to.display_address()),
            data: Some(tx_fields.data),
            gas_limit: U256::from(tx_fields.gas),
            pay_for_gas: None, // TODO: enable when the gas policy refactoring PR merged
        });

        // TODO: maybe add another sign and send tx impl in trading_api?
        // actually I use sign_raw_tx instead of eth.rs's sign_and_send_transaction to avoid bringing eth types here
        // TODO: refactor: use SignRawTransactionEnum as the param instead of SignRawTransactionRequest (coin unneeded)
        let tx_bytes = src_coin
            .sign_raw_tx(&SignRawTransactionRequest {
                coin: src_coin.ticker().to_owned(),
                tx: sign_params,
            })
            .await
            .map_mm_err()?;
        let txid = src_coin
            .send_raw_tx_bytes(&tx_bytes.tx_hex)
            .compat()
            .await
            .map_to_mm(LrSwapError::TransactionError)?;
        let dst_amount = U256::from_dec_str(&swap_with_tx.dst_amount)?;
        info!("{LOG_SWAP_LR_NAME}: liquidity routing tx {} sent okay", txid);
        Ok((dst_amount, tx_bytes))
    }

    /// Start nested atomic swap by calling lp_auto_buy. The actual volume is determined on previous states
    /// For swaps with LR we always use taker 'sell' action and the taker_volume is always in the taker sell coins.
    async fn start_lp_auto_buy(
        &self,
        taker_volume: MmNumber,
        slippage: Option<f32>,
    ) -> MmResult<SellBuyResponse, LrSwapError> {
        // For swaps with LR we always use taker 'sell':
        let taker_action = TakerAction::Sell;

        let base_coin = lp_coinfind_or_err(&self.ctx, &self.atomic_swap.base)
            .await
            .map_mm_err()?;
        let rel_coin = lp_coinfind_or_err(&self.ctx, &self.atomic_swap.rel)
            .await
            .map_mm_err()?;
        if base_coin.wallet_only(&self.ctx) {
            return MmError::err(LrSwapError::InvalidParam(format!(
                "Base coin {} is wallet only",
                self.atomic_swap.base
            )));
        }
        if rel_coin.wallet_only(&self.ctx) {
            return MmError::err(LrSwapError::InvalidParam(format!(
                "Rel coin {} is wallet only",
                self.atomic_swap.rel
            )));
        }
        let (taker_sell_volume, sell_base, sell_rel) = match taker_action {
            TakerAction::Buy => (
                &taker_volume * &self.atomic_swap.price,
                rel_coin.clone(),
                base_coin.clone(),
            ),
            TakerAction::Sell => (taker_volume.clone(), base_coin.clone(), rel_coin.clone()),
        };
        debug!(
            "{LOG_SWAP_LR_NAME} checking taker balance for atomic swap for {}/{} taker_sell_volume: {} action={:?} atomic_swap.price={}",
            sell_base.ticker(),
            sell_rel.ticker(),
            taker_sell_volume.to_decimal(),
            taker_action,
            self.atomic_swap.price.to_decimal(),
        );

        let taker_volume_with_slippage = check_balance_with_slippage(
            &self.ctx,
            sell_base.deref(),
            sell_rel.deref(),
            taker_sell_volume,
            self.lr_0_dst_amount.get().cloned(),
            slippage,
        )
        .await
        .map_mm_err()?;

        info!(
            "{LOG_SWAP_LR_NAME}: starting atomic swap for {}/{}, action {:?}, taker_volume {}",
            self.atomic_swap.base,
            self.atomic_swap.rel,
            taker_action,
            taker_volume_with_slippage.to_decimal()
        );
        let sell_buy_req = make_atomic_swap_request(
            self.atomic_swap.base.clone(),
            self.atomic_swap.rel.clone(),
            self.atomic_swap.price.clone(),
            taker_volume_with_slippage,
            taker_action,
            self.atomic_swap.match_by.clone(),
            self.atomic_swap.order_type.clone(),
            // TODO:
            // if we have LR_1, we need assured spending maker payment confirmation (to ensure LR_1 could run)
            // if no LR_1, we should use atomic swap optional params:
            // `if self.lr_swap_1.is_some() { Some(1) } else { opt_param.conf }`
            // For now set conf=1 to ensure tests to show correct balance
            Some(1),
        );
        let res_bytes = lp_auto_buy(&self.ctx, &base_coin, &rel_coin, sell_buy_req)
            .await
            .map_to_mm(LrSwapError::InternalError)?;
        let rpc_res: Mm2RpcResult<SellBuyResponse> = serde_json::from_slice(res_bytes.as_slice())?;
        Ok(rpc_res.result)
    }

    /// This fn returns None if atomic swap is not finished yet, true if it completed okay and false if funds were refunded
    #[allow(clippy::result_large_err)]
    fn check_atomic_swap_status(
        swap_result: &MmResult<SwapRpcData, MySwapStatusError>,
    ) -> MmResult<Option<bool>, LrSwapError> {
        let swap_status = match swap_result {
            Ok(swap_status) => swap_status,
            Err(mm_err) => {
                match mm_err.get_inner() {
                    // TODO: now considering that swap has not been started yet and we don't have non-existant uuids,
                    // but maybe we could throw an error after some time
                    MySwapStatusError::NoSwapWithUuid(_) => return Ok(None),
                    other_err => {
                        return MmError::err(LrSwapError::InternalError(format!(
                            "Failed to get swap status: {}",
                            other_err
                        )))
                    },
                }
            },
        };
        match swap_status {
            SwapRpcData::TakerV1(swap_status) => {
                if !swap_status.is_finished() {
                    return Ok(None);
                }
                match swap_status.is_success() {
                    Ok(true) => Ok(Some(true)),
                    Ok(false) | Err(_) => MmError::err(LrSwapError::AtomicSwapError("Atomic swap failed".to_string())),
                }
            },
            SwapRpcData::TakerV2(swap_status) => {
                if let Some(reason) = swap_status.is_aborted() {
                    return MmError::err(LrSwapError::AtomicSwapAborted(reason));
                }
                if swap_status.is_completed() {
                    return Ok(Some(true));
                }
                if swap_status.is_refunded() {
                    return Ok(Some(false));
                }
                Ok(None)
            },
            SwapRpcData::AggTaker(_) | SwapRpcData::MakerV1(_) | SwapRpcData::MakerV2(_) => {
                MmError::err(LrSwapError::InternalError("incorrect atomic swap type".to_string()))
            },
        }
    }

    /// This fn returns true if the atomic swap has completed successfully,
    /// false is returned when the atomic swap has finished w/o errors and taker payment was refunded
    async fn wait_for_atomic_swap_finished(&self) -> MmResult<bool, LrSwapError> {
        let atomic_swap_uuid = self
            .atomic_swap_uuid
            .get()
            .ok_or(LrSwapError::InternalError("atomic swap uuid not set".to_string()))?;
        info!(
            "{LOG_SWAP_LR_NAME} {}: waiting for interim atomic swap uuid {} to finish",
            self.uuid, atomic_swap_uuid
        );
        // We do not have any time limits for waiting the atomic swap to finish,
        // assuming the atomic swap code ensures it won't hang.
        loop {
            let swap_result = my_swap_status_rpc(
                self.ctx.clone(),
                MySwapStatusRequest {
                    uuid: *atomic_swap_uuid,
                },
            )
            .await;
            if let Some(result) = Self::check_atomic_swap_status(&swap_result)? {
                return Ok(result);
            }
            Timer::sleep(5.).await;
        }
    }

    async fn wait_for_lr_tx_confirmation(&self, coin: &Ticker, tx_bytes: BytesJson) -> MmResult<(), LrSwapError> {
        match lp_coinfind_or_err(&self.ctx, coin).await.map_mm_err()? {
            MmCoinEnum::EthCoinVariant(eth_coin) => {
                info!("{LOG_SWAP_LR_NAME}: waiting for liquidity routing tx to confirm");
                let confirm_lr_swap_input = ConfirmPaymentInput {
                    payment_tx: tx_bytes.0,
                    confirmations: LR_SWAP_CONFIRMATIONS,
                    requires_nota: false,
                    wait_until: now_sec() + LR_SWAP_WAIT_CONFIRM_TIMEOUT_SEC,
                    check_every: LR_SWAP_WAIT_CONFIRM_INTERVAL_SEC,
                };
                let _ = eth_coin
                    .wait_for_confirmations(confirm_lr_swap_input)
                    .compat()
                    .await
                    .map_to_mm(LrSwapError::TransactionError)?;
                info!("{LOG_SWAP_LR_NAME}: liquidity routing tx confirmed");
                Ok(())
            },
            _ => MmError::err(LrSwapError::CoinTypeError),
        }
    }

    /// Deduct fees from the input volume (which is the LR swap destination amount)
    /// and convert it to the atomic swap buy or sell volume
    async fn deduct_fees(&self, volume_with_fees: MmNumber) -> MmResult<MmNumber, LrSwapError> {
        let taker_ticker = self.atomic_swap.taker_coin();
        let maker_ticker = self.atomic_swap.maker_coin();
        // Try to estimate the original taker volume. This works for EVM as there no min tx amount
        let dex_fee_rate = DexFee::dex_fee_rate(&taker_ticker, &maker_ticker);
        // Recalculate swap_volume with the tarde fees removed:
        let taker_volume = volume_with_fees.clone() / (MmNumber::from("1") + dex_fee_rate);
        debug!(
            "{maker_ticker}/{taker_ticker} deduct_fees volume_with_fees={volume_with_fees} taker_volume={taker_volume} dex_fee_diff={}",
            &volume_with_fees - &taker_volume
        );

        let swap_volume = match self.atomic_swap.action {
            TakerAction::Buy => taker_volume
                .checked_div(&self.atomic_swap.price) // make the amount how it was in the buy order
                .ok_or(MmError::new(LrSwapError::ConversionError(
                    "Could not calculate swap amount".to_string(),
                )))?,
            TakerAction::Sell => taker_volume,
        };
        Ok(swap_volume)
    }
}

async fn agg_taker_swap_state_machine_runner(
    ctx: &MmArc,
    lr_swap_0: Option<LrSwapParams>,
    lr_swap_1: Option<LrSwapParams>,
    atomic_swap: AtomicSwapParams,
    uuid: Uuid,
) {
    let mut state_machine = AggTakerSwapStateMachine {
        storage: AggTakerSwapStorage::new(ctx.clone()),
        abortable_system: ctx
            .abortable_system
            .create_subsystem()
            .expect("create_subsystem should not fail"),
        ctx: ctx.clone(),
        lr_swap_0,
        lr_swap_1,
        atomic_swap,
        lr_0_dst_amount: OnceLock::new(),
        uuid,
        started_at: now_sec(),
        swap_version: AggTakerSwapStateMachine::AGG_SWAP_VERSION,
        atomic_swap_uuid: OnceLock::default(),
    };
    #[allow(clippy::box_default)]
    state_machine
        .run(Box::new(states::Initialize::default()))
        .await
        .error_log();
}

pub(crate) async fn lp_start_agg_taker_swap(
    ctx: MmArc,
    lr_swap_0: Option<LrSwapParams>,
    lr_swap_1: Option<LrSwapParams>,
    atomic_swap: AtomicSwapParams,
) -> MmResult<Uuid, LrSwapError> {
    let spawner = ctx.spawner();
    let uuid = new_uuid(); // For a aggregated swap we need a new uuid, different from the atomic swap uuid, to distinguish the aggregated swap as dedicated in rpcs, statuses etc

    let fut = async move {
        agg_taker_swap_state_machine_runner(&ctx, lr_swap_0, lr_swap_1, atomic_swap, uuid).await;
    };

    let settings = AbortSettings::info_on_abort(format!("swap {uuid} stopped!"));
    spawner.spawn_with_settings(fut, settings);
    Ok(uuid)
}

// Checks balance for the taker volume and decreases it within the slippage
// The slippage value is percentage.
pub async fn check_balance_with_slippage(
    ctx: &MmArc,
    my_coin: &dyn MmCoin,
    other_coin: &dyn MmCoin,
    mut volume: MmNumber,
    lr_0_dst_amount: Option<MmNumber>,
    slippage: Option<f32>,
) -> CheckBalanceResult<MmNumber> {
    let fee_params =
        create_taker_swap_default_params(my_coin, other_coin, volume.clone(), FeeApproxStage::OrderIssue).await?;
    let taker_fee = TakerFeeAdditionalInfo {
        dex_fee: fee_params.clone().dex_fee,
        fee_to_send_dex_fee: fee_params.clone().fee_to_send_dex_fee,
    };

    if let Err(err) = check_my_coin_balance_for_swap(
        ctx,
        my_coin,
        None,
        volume.clone(),
        fee_params.clone().taker_payment_trade_fee,
        Some(taker_fee),
    )
    .await
    {
        let lr_0_dst_amount = lr_0_dst_amount.ok_or(err.clone())?;
        let slippage = slippage.ok_or(err.clone())?;
        let CheckBalanceError::NotSufficientBalance {
            coin,
            available,
            locked_by_swaps,
            ..
        } = err.get_inner()
        else {
            return Err(err);
        };
        // Convert to BigDecimal to round to token decimals and compare with 'required'
        let available = MmNumber::from(available).to_decimal().round(my_coin.decimals() as i64);

        // Check if available volume is within the LR_0 slippage.
        // Note about how we consider the available volume:
        // We do not know the exact amount received from the LR_0 and how it is different from the original dst_amount returned by 1inch due to slippage.
        // we just assume we could spend available volume, provided it is within slippage (from the 1inch dst_amount).
        // If it's not within the slippage, we assume something wrong has happened.
        let required_with_slippage =
            &lr_0_dst_amount * &BigDecimal::from_f32(1.0 - slippage / 100.0).unwrap_or_default().into();
        // Convert to BigDecimal to round to token decimals and compare with 'available'
        let required_with_slippage = required_with_slippage.to_decimal().round(my_coin.decimals() as i64);
        debug!(
            "{LOG_SWAP_LR_NAME} received NotSufficientBalance error, checking slippage: my_coin={} lr_0_dst_amount={} volume={} available={} slippage={} required_with_slippage={}",
            my_coin.ticker(),
            lr_0_dst_amount.to_decimal(),
            volume.to_decimal(),
            available,
            slippage,
            required_with_slippage
        );

        if available < required_with_slippage {
            info!(
                "{LOG_SWAP_LR_NAME} too low available {}, required with slippage {}, aborting atomic swap",
                available, required_with_slippage,
            );
            return MmError::err(CheckBalanceError::NotSufficientBalance {
                coin: coin.clone(),
                available,
                required: required_with_slippage,
                locked_by_swaps: locked_by_swaps.clone(),
            });
        }
        // Estimate new taker_volume for atomic swap (with slippage), by deducting fees:
        let mut volume_from_avail: MmNumber = available.into();
        if my_coin.ticker() == fee_params.taker_payment_trade_fee.coin {
            volume_from_avail -= fee_params.taker_payment_trade_fee.amount; // TODO: remove funding tx txfee as well
        }
        volume = {
            let dex_fee_rate = DexFee::dex_fee_rate(my_coin.ticker(), other_coin.ticker());
            volume_from_avail / (MmNumber::from("1") + dex_fee_rate)
        };
        debug!("{LOG_SWAP_LR_NAME} new volume with slippage={}", volume.to_decimal());
    }

    if !fee_params.maker_payment_spend_trade_fee.paid_from_trading_vol {
        check_other_coin_balance_for_swap(ctx, other_coin, None, fee_params.maker_payment_spend_trade_fee).await?;
    }
    Ok(volume)
}
