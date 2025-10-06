use super::check_balance::{
    check_my_coin_balance_for_swap, CheckBalanceError, CheckBalanceResult, TakerFeeAdditionalInfo,
};
use super::pubkey_banning::ban_pubkey_on_failed_swap;
use super::swap_lock::{SwapLock, SwapLockOps};
use super::swap_watcher::{watcher_topic, SwapWatcherMsg};
use super::trade_preimage::{TradePreimageRequest, TradePreimageRpcError, TradePreimageRpcResult};
use super::{
    broadcast_my_swap_status, broadcast_swap_message, broadcast_swap_msg_every, check_other_coin_balance_for_swap,
    get_locked_amount, recv_swap_msg, swap_topic, wait_for_maker_payment_conf_until, AtomicSwap, LockedAmount,
    MySwapInfo, NegotiationDataMsg, NegotiationDataV2, NegotiationDataV3, RecoveredSwap, RecoveredSwapAction,
    SavedSwap, SavedSwapIo, SavedTradeFee, SwapConfirmationsSettings, SwapError, SwapMsg, SwapPubkeys, SwapTxDataMsg,
    SwapsContext, TransactionIdentifier, WAIT_CONFIRM_INTERVAL_SEC,
};
use crate::lp_network::subscribe_to_topic;
use crate::lp_ordermatch::TakerOrderBuilder;
use crate::lp_swap::swap_events::{SwapStatusEvent, SwapStatusStreamer};
use crate::lp_swap::swap_v2_common::mark_swap_as_finished;
use crate::lp_swap::taker_restart::get_command_based_on_maker_or_watcher_activity;
use crate::lp_swap::{
    broadcast_p2p_tx_msg, broadcast_swap_msg_every_delayed, tx_helper_topic, wait_for_maker_payment_conf_duration,
    TakerSwapWatcherData, MAX_STARTED_AT_DIFF,
};
use coins::lp_price::fetch_swap_coins_price;
use coins::{
    lp_coinfind, CanRefundHtlc, CheckIfMyPaymentSentArgs, ConfirmPaymentInput, DexFee, FeeApproxStage,
    FoundSwapTxSpend, MmCoin, MmCoinEnum, PaymentInstructionArgs, PaymentInstructions, PaymentInstructionsErr,
    RefundPaymentArgs, SearchForSwapTxSpendInput, SendPaymentArgs, SpendPaymentArgs, SwapTxTypeWithSecretHash,
    TradeFee, TradePreimageValue, TransactionEnum, ValidatePaymentInput, WaitForHTLCTxSpendArgs, WatcherReward,
};
use common::executor::Timer;
use common::log::{debug, error, info, warn};
use common::{bits256, now_ms, now_sec, wait_until_sec};
use crypto::{privkey::SerializableSecp256k1Keypair, CryptoCtx};
use futures::{compat::Future01CompatExt, future::try_join, select, FutureExt};
use http::Response;
use keys::KeyPair;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_event_stream::DeriveStreamerId;
use mm2_number::{BigDecimal, MmNumber};
use mm2_rpc::data::legacy::{MatchBy, OrderConfirmationsSettings, TakerAction};
use parking_lot::Mutex as PaMutex;
use primitives::hash::H264;
use rpc::v1::types::{Bytes as BytesJson, H256 as H256Json, H264 as H264Json};
use serde_json::{self as json, Value as Json};
use std::convert::TryInto;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use uuid::Uuid;

const TAKER_PAYMENT_SPEND_SEARCH_INTERVAL: f64 = 10.;

pub const TAKER_SUCCESS_EVENTS: [&str; 12] = [
    "Started",
    "Negotiated",
    "TakerFeeSent",
    "TakerPaymentInstructionsReceived",
    "MakerPaymentReceived",
    "MakerPaymentWaitConfirmStarted",
    "MakerPaymentValidatedAndConfirmed",
    "TakerPaymentSent",
    "TakerPaymentSpent",
    "MakerPaymentSpent",
    "MakerPaymentSpendConfirmed",
    "Finished",
];

pub const TAKER_USING_WATCHERS_SUCCESS_EVENTS: [&str; 14] = [
    "Started",
    "Negotiated",
    "TakerFeeSent",
    "TakerPaymentInstructionsReceived",
    "MakerPaymentReceived",
    "MakerPaymentWaitConfirmStarted",
    "MakerPaymentValidatedAndConfirmed",
    "TakerPaymentSent",
    "WatcherMessageSent",
    "TakerPaymentSpent",
    "MakerPaymentSpent",
    "MakerPaymentSpentByWatcher",
    "MakerPaymentSpendConfirmed",
    "Finished",
];

pub const TAKER_ERROR_EVENTS: [&str; 17] = [
    "StartFailed",
    "NegotiateFailed",
    "TakerFeeSendFailed",
    "MakerPaymentValidateFailed",
    "MakerPaymentWaitConfirmFailed",
    "TakerPaymentTransactionFailed",
    "TakerPaymentWaitConfirmFailed",
    "TakerPaymentDataSendFailed",
    "TakerPaymentWaitForSpendFailed",
    "MakerPaymentSpendFailed",
    "MakerPaymentSpendConfirmFailed",
    "TakerPaymentWaitRefundStarted",
    "TakerPaymentRefundStarted",
    "TakerPaymentRefunded",
    "TakerPaymentRefundedByWatcher",
    "TakerPaymentRefundFailed",
    "TakerPaymentRefundFinished",
];

pub const REFUND_TEST_FAILURE_LOG: &str = "Explicit refund test failure...";
pub const WATCHER_MESSAGE_SENT_LOG: &str = "Watcher message sent...";
pub const MAKER_PAYMENT_SPENT_BY_WATCHER_LOG: &str = "Maker payment is spent by the watcher...";

#[cfg(not(target_arch = "wasm32"))]
pub fn stats_taker_swap_dir(ctx: &MmArc) -> PathBuf {
    ctx.global_dir().join("SWAPS").join("STATS").join("TAKER")
}

#[cfg(not(target_arch = "wasm32"))]
pub fn stats_taker_swap_file_path(ctx: &MmArc, uuid: &Uuid) -> PathBuf {
    stats_taker_swap_dir(ctx).join(format!("{uuid}.json"))
}

async fn save_my_taker_swap_event(ctx: &MmArc, swap: &TakerSwap, event: TakerSavedEvent) -> Result<(), String> {
    let maker_coin_pub = swap.my_maker_coin_htlc_pub();
    let maker_coin_address = try_s!(swap.maker_coin.address_from_pubkey(&maker_coin_pub));
    let swap = match SavedSwap::load_my_swap_from_db(ctx, Some(&maker_coin_address), swap.uuid).await {
        Ok(Some(swap)) => swap,
        Ok(None) => SavedSwap::Taker(TakerSavedSwap {
            uuid: swap.uuid,
            #[cfg(all(not(target_arch = "wasm32"), feature = "new-db-arch"))]
            maker_address: maker_coin_address,
            my_order_uuid: swap.my_order_uuid,
            maker_amount: Some(swap.maker_amount.to_decimal()),
            maker_coin: Some(swap.maker_coin.ticker().to_owned()),
            maker_coin_usd_price: None,
            taker_amount: Some(swap.taker_amount.to_decimal()),
            taker_coin: Some(swap.taker_coin.ticker().to_owned()),
            taker_coin_usd_price: None,
            gui: ctx.gui().map(|g| g.to_owned()),
            mm_version: Some(ctx.mm_version.to_owned()),
            events: vec![],
            success_events: if !ctx.disable_watchers_globally()
                && swap.taker_coin.is_supported_by_watchers()
                && swap.maker_coin.is_supported_by_watchers()
            {
                TAKER_USING_WATCHERS_SUCCESS_EVENTS
                    .iter()
                    .map(<&str>::to_string)
                    .collect()
            } else {
                TAKER_SUCCESS_EVENTS.iter().map(<&str>::to_string).collect()
            },
            error_events: TAKER_ERROR_EVENTS.iter().map(<&str>::to_string).collect(),
        }),
        Err(e) => return ERR!("{}", e),
    };

    if let SavedSwap::Taker(mut taker_swap) = swap {
        taker_swap.events.push(event);
        if taker_swap.is_success().unwrap_or(false) {
            taker_swap.fetch_and_set_usd_prices().await;
        }
        let new_swap = SavedSwap::Taker(taker_swap);
        try_s!(new_swap.save_to_db(ctx).await);
        Ok(())
    } else {
        ERR!("Expected SavedSwap::Taker, got {:?}", swap)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TakerSavedEvent {
    pub timestamp: u64,
    pub event: TakerSwapEvent,
}

impl TakerSavedEvent {
    /// get the next swap command that must be executed after swap restore
    fn get_command(&self) -> Option<TakerSwapCommand> {
        match self.event {
            TakerSwapEvent::Started(_) => Some(TakerSwapCommand::Negotiate),
            TakerSwapEvent::StartFailed(_) => Some(TakerSwapCommand::Finish),
            TakerSwapEvent::Negotiated(_) => Some(TakerSwapCommand::SendTakerFee),
            TakerSwapEvent::NegotiateFailed(_) => Some(TakerSwapCommand::Finish),
            TakerSwapEvent::TakerFeeSent(_) => Some(TakerSwapCommand::WaitForMakerPayment),
            TakerSwapEvent::TakerFeeSendFailed(_) => Some(TakerSwapCommand::Finish),
            TakerSwapEvent::TakerPaymentInstructionsReceived(_) => Some(TakerSwapCommand::ValidateMakerPayment),
            TakerSwapEvent::MakerPaymentReceived(_) => Some(TakerSwapCommand::ValidateMakerPayment),
            TakerSwapEvent::MakerPaymentWaitConfirmStarted => Some(TakerSwapCommand::ValidateMakerPayment),
            TakerSwapEvent::MakerPaymentValidatedAndConfirmed => Some(TakerSwapCommand::SendTakerPayment),
            TakerSwapEvent::MakerPaymentValidateFailed(_) => Some(TakerSwapCommand::Finish),
            TakerSwapEvent::MakerPaymentWaitConfirmFailed(_) => Some(TakerSwapCommand::Finish),
            TakerSwapEvent::TakerPaymentSent(_) => Some(TakerSwapCommand::WaitForTakerPaymentSpend),
            TakerSwapEvent::WatcherMessageSent(_, _) => Some(TakerSwapCommand::WaitForTakerPaymentSpend),
            TakerSwapEvent::TakerPaymentTransactionFailed(_) => Some(TakerSwapCommand::Finish),
            TakerSwapEvent::TakerPaymentDataSendFailed(_) => Some(TakerSwapCommand::PrepareForTakerPaymentRefund),
            TakerSwapEvent::TakerPaymentSpent(_) => Some(TakerSwapCommand::SpendMakerPayment),
            TakerSwapEvent::TakerPaymentWaitForSpendFailed(_) => Some(TakerSwapCommand::PrepareForTakerPaymentRefund),
            TakerSwapEvent::TakerPaymentWaitConfirmFailed(_) => Some(TakerSwapCommand::PrepareForTakerPaymentRefund),
            TakerSwapEvent::MakerPaymentSpent(_) => Some(TakerSwapCommand::ConfirmMakerPaymentSpend),
            TakerSwapEvent::MakerPaymentSpendConfirmed => Some(TakerSwapCommand::Finish),
            TakerSwapEvent::MakerPaymentSpendConfirmFailed(_) => Some(TakerSwapCommand::PrepareForTakerPaymentRefund),
            TakerSwapEvent::MakerPaymentSpentByWatcher(_) => Some(TakerSwapCommand::ConfirmMakerPaymentSpend),
            TakerSwapEvent::MakerPaymentSpendFailed(_) => Some(TakerSwapCommand::PrepareForTakerPaymentRefund),
            TakerSwapEvent::TakerPaymentWaitRefundStarted { .. } => {
                Some(TakerSwapCommand::PrepareForTakerPaymentRefund)
            },
            TakerSwapEvent::TakerPaymentRefundStarted => Some(TakerSwapCommand::RefundTakerPayment),
            TakerSwapEvent::TakerPaymentRefunded(_) => Some(TakerSwapCommand::FinalizeTakerPaymentRefund),
            TakerSwapEvent::TakerPaymentRefundFailed(_) => Some(TakerSwapCommand::Finish),
            TakerSwapEvent::TakerPaymentRefundFinished => Some(TakerSwapCommand::Finish),
            TakerSwapEvent::TakerPaymentRefundedByWatcher(_) => Some(TakerSwapCommand::Finish),
            TakerSwapEvent::Finished => None,
        }
    }
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
pub struct TakerSavedSwap {
    pub uuid: Uuid,
    #[cfg(all(not(target_arch = "wasm32"), feature = "new-db-arch"))]
    pub maker_address: String,
    pub my_order_uuid: Option<Uuid>,
    pub events: Vec<TakerSavedEvent>,
    pub maker_amount: Option<BigDecimal>,
    pub maker_coin: Option<String>,
    pub maker_coin_usd_price: Option<BigDecimal>,
    pub taker_amount: Option<BigDecimal>,
    pub taker_coin: Option<String>,
    pub taker_coin_usd_price: Option<BigDecimal>,
    pub gui: Option<String>,
    pub mm_version: Option<String>,
    pub success_events: Vec<String>,
    pub error_events: Vec<String>,
}

impl TakerSavedSwap {
    pub fn maker_coin(&self) -> Result<String, String> {
        match self.events.first() {
            Some(event) => match &event.event {
                TakerSwapEvent::Started(data) => Ok(data.maker_coin.clone()),
                _ => ERR!("First swap event must be Started"),
            },
            None => ERR!("Can't get maker coin, events are empty"),
        }
    }

    pub fn taker_coin(&self) -> Result<String, String> {
        match self.events.first() {
            Some(event) => match &event.event {
                TakerSwapEvent::Started(data) => Ok(data.taker_coin.clone()),
                _ => ERR!("First swap event must be Started"),
            },
            None => ERR!("Can't get maker coin, events are empty"),
        }
    }

    pub fn is_finished(&self) -> bool {
        match self.events.last() {
            Some(event) => event.event == TakerSwapEvent::Finished,
            None => false,
        }
    }

    pub fn get_my_info(&self) -> Option<MySwapInfo> {
        match self.events.first() {
            Some(event) => match &event.event {
                TakerSwapEvent::Started(data) => Some(MySwapInfo {
                    my_coin: data.taker_coin.clone(),
                    other_coin: data.maker_coin.clone(),
                    my_amount: data.taker_amount.clone(),
                    other_amount: data.maker_amount.clone(),
                    started_at: data.started_at,
                }),
                _ => None,
            },
            None => None,
        }
    }

    pub fn is_recoverable(&self) -> bool {
        if !self.is_finished() {
            return false;
        };
        let mut maker_payment_spent = false;
        let mut maker_payment_spent_by_watcher = false;
        let mut maker_payment_spend_confirmed_failed = false;
        for event in self.events.iter() {
            match event.event {
                TakerSwapEvent::StartFailed(_)
                | TakerSwapEvent::NegotiateFailed(_)
                | TakerSwapEvent::TakerFeeSendFailed(_)
                | TakerSwapEvent::MakerPaymentValidateFailed(_)
                | TakerSwapEvent::TakerPaymentRefunded(_)
                | TakerSwapEvent::TakerPaymentRefundedByWatcher(_)
                | TakerSwapEvent::MakerPaymentSpendConfirmed
                | TakerSwapEvent::MakerPaymentWaitConfirmFailed(_) => {
                    return false;
                },
                TakerSwapEvent::MakerPaymentSpent(_) => {
                    maker_payment_spent = true;
                },
                TakerSwapEvent::MakerPaymentSpentByWatcher(_) => {
                    maker_payment_spent_by_watcher = true;
                },
                TakerSwapEvent::MakerPaymentSpendConfirmFailed(_) => {
                    maker_payment_spend_confirmed_failed = true;
                },
                _ => (),
            }
        }
        // MakerPaymentSpent or MakerPaymentSpentByWatcher were the last success events but a new step `MakerPaymentSpendConfirmed` was added after them.
        // For backward compatibility (old saved swaps) we need to check for MakerPaymentSpent or MakerPaymentSpentByWatcher
        // and if there is no MakerPaymentSpendConfirmFailed.
        maker_payment_spend_confirmed_failed || (!maker_payment_spent && !maker_payment_spent_by_watcher)
    }

    pub fn swap_data(&self) -> Result<&TakerSwapData, String> {
        match self.events.first() {
            Some(event) => match &event.event {
                TakerSwapEvent::Started(data) => Ok(data),
                _ => ERR!("First swap event must be Started"),
            },
            None => ERR!("Can't get swap_data, events are empty"),
        }
    }

    pub fn finished_at(&self) -> Result<u64, String> {
        match self.events.last() {
            Some(event) => match &event.event {
                TakerSwapEvent::Finished => Ok(event.timestamp / 1000),
                _ => ERR!("Last swap event must be Finished"),
            },
            None => ERR!("Can't get finished_at, events are empty"),
        }
    }

    pub fn is_success(&self) -> Result<bool, String> {
        if !self.is_finished() {
            return ERR!("Can not determine is_success state for not finished swap");
        }
        for event in self.events.iter() {
            if event.event.is_error() {
                return Ok(false);
            }
        }
        Ok(true)
    }

    pub fn watcher_message_sent(&self) -> bool {
        self.events
            .iter()
            .any(|e| matches!(e.event, TakerSwapEvent::WatcherMessageSent(_, _)))
    }

    pub async fn fetch_and_set_usd_prices(&mut self) {
        if let Some(rates) = fetch_swap_coins_price(self.maker_coin.clone(), self.taker_coin.clone()).await {
            self.maker_coin_usd_price = Some(rates.base);
            self.taker_coin_usd_price = Some(rates.rel);
        }
    }

    // TODO: Adjust for private coins when/if they are braodcasted
    // TODO: Adjust for HD wallet when completed
    pub fn swap_pubkeys(&self) -> Result<SwapPubkeys, String> {
        let taker = match &self.events.first() {
            Some(event) => match &event.event {
                TakerSwapEvent::Started(started) => started.my_persistent_pub.to_string(),
                _ => return ERR!("First swap event must be Started"),
            },
            None => return ERR!("Can't get taker's pubkey while events are empty"),
        };

        let maker = match self.events.get(1) {
            Some(event) => match &event.event {
                TakerSwapEvent::Negotiated(neg) => {
                    let Some(key) = neg.maker_coin_htlc_pubkey else {
                        return ERR!("maker's pubkey is empty");
                    };
                    key.to_string()
                },
                _ => return ERR!("Swap must be negotiated to get maker's pubkey"),
            },
            None => return ERR!("Can't get maker's pubkey while there's no Negotiated event"),
        };

        Ok(SwapPubkeys { maker, taker })
    }
}

#[allow(clippy::large_enum_variant)]
pub enum RunTakerSwapInput {
    StartNew(TakerSwap),
    KickStart {
        maker_coin: MmCoinEnum,
        taker_coin: MmCoinEnum,
        swap_uuid: Uuid,
    },
}

impl RunTakerSwapInput {
    fn uuid(&self) -> &Uuid {
        match self {
            RunTakerSwapInput::StartNew(swap) => &swap.uuid,
            RunTakerSwapInput::KickStart { swap_uuid, .. } => swap_uuid,
        }
    }
}

/// Starts the taker swap and drives it to completion (until None next command received).
/// Panics in case of command or event apply fails, not sure yet how to handle such situations
/// because it's usually means that swap is in invalid state which is possible only if there's developer error
/// Every produced event is saved to local DB. Swap status is broadcast to P2P network after completion.
pub async fn run_taker_swap(swap: RunTakerSwapInput, ctx: MmArc) {
    let uuid = swap.uuid().to_owned();
    let mut attempts = 0;
    let swap_lock = loop {
        match SwapLock::lock(&ctx, uuid, 40.).await {
            Ok(Some(l)) => break l,
            Ok(None) => {
                if attempts >= 1 {
                    warn!(
                        "Swap {} file lock is acquired by another process/thread, aborting",
                        uuid
                    );
                    return;
                } else {
                    attempts += 1;
                    Timer::sleep(40.).await;
                }
            },
            Err(e) => {
                error!("Swap {} file lock error: {}", uuid, e);
                return;
            },
        }
    };

    let (swap, mut command) = match swap {
        RunTakerSwapInput::StartNew(swap) => (swap, TakerSwapCommand::Start),
        RunTakerSwapInput::KickStart {
            maker_coin,
            taker_coin,
            swap_uuid,
        } => match TakerSwap::load_from_db_by_uuid(ctx, maker_coin, taker_coin, &swap_uuid).await {
            Ok((swap, command)) => match command {
                Some(c) => {
                    info!("Swap {} kick started.", uuid);
                    (swap, c)
                },
                None => {
                    warn!("Swap {} has been finished already, aborting.", uuid);
                    return;
                },
            },
            Err(e) => {
                error!("Error loading swap {}: {}", uuid, e);
                return;
            },
        },
    };

    let mut touch_loop = Box::pin(
        async move {
            loop {
                match swap_lock.touch().await {
                    Ok(_) => (),
                    Err(e) => warn!("Swap {} file lock error: {}", uuid, e),
                };
                Timer::sleep(30.).await;
            }
        }
        .fuse(),
    );

    let ctx = swap.ctx.clone();
    subscribe_to_topic(&ctx, swap_topic(&swap.uuid));
    let mut status = ctx.log.status_handle();
    let uuid_str = uuid.to_string();
    let to_broadcast = !(swap.maker_coin.is_privacy() || swap.taker_coin.is_privacy());
    let running_swap = Arc::new(swap);
    let swap_ctx = SwapsContext::from_ctx(&ctx).unwrap();
    swap_ctx.init_msg_store(running_swap.uuid, running_swap.maker_pubkey);
    // Register the swap in the running swaps map.
    swap_ctx
        .running_swaps
        .lock()
        .unwrap()
        .insert(uuid, running_swap.clone());
    let mut swap_fut = Box::pin(
        async move {
            let mut events;
            loop {
                let res = running_swap.handle_command(command).await.expect("!handle_command");
                events = res.1;
                for event in events {
                    let to_save = TakerSavedEvent {
                        timestamp: now_ms(),
                        event: event.clone(),
                    };

                    // Send a notification to the swap status streamer about a new event.
                    ctx.event_stream_manager
                        .send_fn(&SwapStatusStreamer::derive_streamer_id(()), || {
                            SwapStatusEvent::TakerV1 {
                                uuid: running_swap.uuid,
                                event: to_save.clone(),
                            }
                        })
                        .ok();
                    save_my_taker_swap_event(&ctx, &running_swap, to_save)
                        .await
                        .expect("!save_my_taker_swap_event");
                    if event.should_ban_maker() {
                        ban_pubkey_on_failed_swap(
                            &ctx,
                            running_swap.maker_pubkey.bytes.into(),
                            &running_swap.uuid,
                            event.clone().into(),
                        )
                    }

                    if event.is_error() {
                        error!("[swap uuid={uuid_str}] {event:?}");
                    }

                    status.status(&[&"swap", &("uuid", uuid_str.as_str())], &event.status_str());
                    running_swap.apply_event(event);
                }
                match res.0 {
                    Some(c) => {
                        command = c;
                    },
                    None => {
                        if let Err(e) = mark_swap_as_finished(ctx.clone(), running_swap.uuid).await {
                            error!("!mark_swap_finished({}): {}", uuid_str, e);
                        }

                        if to_broadcast {
                            if let Err(e) = broadcast_my_swap_status(&ctx, running_swap.uuid).await {
                                error!("!broadcast_my_swap_status({}): {}", uuid_str, e);
                            }
                        }
                        break;
                    },
                }
            }
        }
        .fuse(),
    );
    select! {
        _swap = swap_fut => (), // swap finished normally
        _touch = touch_loop => unreachable!("Touch loop can not stop!"),
    };
    // Remove the swap from the running swaps map.
    swap_ctx.running_swaps.lock().unwrap().remove(&uuid);
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TakerSwapData {
    pub taker_coin: String,
    pub maker_coin: String,
    #[serde(rename = "maker")]
    pub maker_pubkey: H256Json,
    pub my_persistent_pub: H264Json,
    pub lock_duration: u64,
    pub maker_amount: BigDecimal,
    pub taker_amount: BigDecimal,
    pub maker_payment_confirmations: u64,
    pub maker_payment_requires_nota: Option<bool>,
    pub taker_payment_confirmations: u64,
    pub taker_payment_requires_nota: Option<bool>,
    pub taker_payment_lock: u64,
    /// Allows to recognize one SWAP from the other in the logs. #274.
    pub uuid: Uuid,
    pub started_at: u64,
    pub maker_payment_wait: u64,
    pub maker_coin_start_block: u64,
    pub taker_coin_start_block: u64,
    /// A transaction fee that should be paid to send a `TakerFee`.
    /// Note this value is used to calculate locked amount only.
    pub fee_to_send_taker_fee: Option<SavedTradeFee>,
    /// A `TakerPayment` transaction fee.
    /// Note this value is used to calculate locked amount only.
    pub taker_payment_trade_fee: Option<SavedTradeFee>,
    /// A transaction fee that should be paid to spend a `MakerPayment`.
    /// Note this value is used to calculate locked amount only.
    pub maker_payment_spend_trade_fee: Option<SavedTradeFee>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub maker_coin_swap_contract_address: Option<BytesJson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub taker_coin_swap_contract_address: Option<BytesJson>,
    /// Temporary pubkey used in HTLC redeem script when applicable for maker coin
    /// Note: it's temporary for zcoin. For other coins it's currently obtained from iguana key or HD wallet activated key
    pub maker_coin_htlc_pubkey: Option<H264Json>,
    /// Temporary pubkey used in HTLC redeem script when applicable for taker coin
    /// Note: it's temporary for zcoin. For other coins it's currently obtained from iguana key or HD wallet activated key
    pub taker_coin_htlc_pubkey: Option<H264Json>,
    /// Temporary privkey used to sign P2P messages when applicable
    pub p2p_privkey: Option<SerializableSecp256k1Keypair>,
}

pub struct TakerSwapMut {
    pub data: TakerSwapData,
    pub other_maker_coin_htlc_pub: H264,
    pub other_taker_coin_htlc_pub: H264,
    taker_fee: Option<TransactionIdentifier>,
    pub maker_payment: Option<TransactionIdentifier>,
    pub taker_payment: Option<TransactionIdentifier>,
    maker_payment_spend: Option<TransactionIdentifier>,
    maker_payment_spend_confirmed: bool,
    taker_payment_spend: Option<TransactionIdentifier>,
    maker_payment_spend_preimage: Option<Vec<u8>>,
    taker_payment_refund_preimage: Option<Vec<u8>>,
    taker_payment_refund: Option<TransactionIdentifier>,
    pub secret_hash: BytesJson,
    secret: H256Json,
    pub watcher_reward: bool,
    reward_amount: Option<BigDecimal>,
    payment_instructions: Option<PaymentInstructions>,
}

#[cfg(any(test, feature = "run-docker-tests"))]
#[derive(Eq, PartialEq, Debug)]
pub enum FailAt {
    TakerPayment,
    WaitForTakerPaymentSpendPanic,
    MakerPaymentSpend,
    MakerPaymentSpendPanic,
    TakerPaymentRefund,
    TakerPaymentRefundPanic,
}

#[cfg(any(test, feature = "run-docker-tests"))]
impl From<String> for FailAt {
    fn from(str: String) -> Self {
        match str.as_str() {
            "taker_payment" => FailAt::TakerPayment,
            "wait_for_taker_payment_spend_panic" => FailAt::WaitForTakerPaymentSpendPanic,
            "maker_payment_spend" => FailAt::MakerPaymentSpend,
            "maker_payment_spend_panic" => FailAt::MakerPaymentSpendPanic,
            "taker_payment_refund" => FailAt::TakerPaymentRefund,
            "taker_payment_refund_panic" => FailAt::TakerPaymentRefundPanic,
            _ => panic!("Invalid TAKER_FAIL_AT value"),
        }
    }
}

pub struct TakerSwap {
    ctx: MmArc,
    pub maker_coin: MmCoinEnum,
    pub taker_coin: MmCoinEnum,
    pub maker_amount: MmNumber,
    pub taker_amount: MmNumber,
    my_persistent_pub: H264,
    maker_pubkey: bits256,
    uuid: Uuid,
    my_order_uuid: Option<Uuid>,
    pub maker_payment_lock: AtomicU64,
    maker_payment_confirmed: AtomicBool,
    errors: PaMutex<Vec<SwapError>>,
    finished_at: AtomicU64,
    mutable: RwLock<TakerSwapMut>,
    conf_settings: SwapConfirmationsSettings,
    payment_locktime: u64,
    p2p_privkey: Option<KeyPair>,
    #[cfg(any(test, feature = "run-docker-tests"))]
    pub(super) fail_at: Option<FailAt>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TakerPaymentSpentData {
    pub transaction: TransactionIdentifier,
    pub secret: H256Json,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct MakerNegotiationData {
    pub maker_payment_locktime: u64,
    pub maker_pubkey: H264Json,
    pub secret_hash: BytesJson,
    pub maker_coin_swap_contract_addr: Option<BytesJson>,
    pub taker_coin_swap_contract_addr: Option<BytesJson>,
    pub maker_coin_htlc_pubkey: Option<H264Json>,
    pub taker_coin_htlc_pubkey: Option<H264Json>,
}

impl MakerNegotiationData {
    fn other_maker_coin_htlc_pub(&self) -> H264 {
        self.maker_coin_htlc_pubkey.unwrap_or(self.maker_pubkey).into()
    }

    fn other_taker_coin_htlc_pub(&self) -> H264 {
        self.taker_coin_htlc_pubkey.unwrap_or(self.maker_pubkey).into()
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", content = "data")]
#[allow(clippy::large_enum_variant)]
pub enum TakerSwapEvent {
    Started(TakerSwapData),
    StartFailed(SwapError),
    Negotiated(MakerNegotiationData),
    NegotiateFailed(SwapError),
    TakerFeeSent(TransactionIdentifier),
    TakerFeeSendFailed(SwapError),
    TakerPaymentInstructionsReceived(Option<PaymentInstructions>),
    MakerPaymentReceived(TransactionIdentifier),
    MakerPaymentWaitConfirmStarted,
    MakerPaymentValidatedAndConfirmed,
    MakerPaymentValidateFailed(SwapError),
    MakerPaymentWaitConfirmFailed(SwapError),
    TakerPaymentSent(TransactionIdentifier),
    WatcherMessageSent(Option<Vec<u8>>, Option<Vec<u8>>),
    TakerPaymentTransactionFailed(SwapError),
    TakerPaymentDataSendFailed(SwapError),
    TakerPaymentWaitConfirmFailed(SwapError),
    TakerPaymentSpent(TakerPaymentSpentData),
    TakerPaymentWaitForSpendFailed(SwapError),
    MakerPaymentSpent(TransactionIdentifier),
    MakerPaymentSpendConfirmed,
    MakerPaymentSpendConfirmFailed(SwapError),
    MakerPaymentSpentByWatcher(TransactionIdentifier),
    MakerPaymentSpendFailed(SwapError),
    TakerPaymentWaitRefundStarted { wait_until: u64 },
    TakerPaymentRefundStarted,
    TakerPaymentRefunded(Option<TransactionIdentifier>),
    TakerPaymentRefundFailed(SwapError),
    TakerPaymentRefundFinished,
    TakerPaymentRefundedByWatcher(Option<TransactionIdentifier>),
    Finished,
}

impl TakerSwapEvent {
    pub fn status_str(&self) -> String {
        match self {
            TakerSwapEvent::Started(_) => "Started...".to_owned(),
            TakerSwapEvent::StartFailed(_) => "Start failed...".to_owned(),
            TakerSwapEvent::Negotiated(_) => "Negotiated...".to_owned(),
            TakerSwapEvent::NegotiateFailed(_) => "Negotiate failed...".to_owned(),
            TakerSwapEvent::TakerFeeSent(_) => "Taker fee sent...".to_owned(),
            TakerSwapEvent::TakerFeeSendFailed(_) => "Taker fee send failed...".to_owned(),
            TakerSwapEvent::TakerPaymentInstructionsReceived(_) => "Taker payment instructions received...".to_owned(),
            TakerSwapEvent::MakerPaymentReceived(_) => "Maker payment received...".to_owned(),
            TakerSwapEvent::MakerPaymentWaitConfirmStarted => "Maker payment wait confirm started...".to_owned(),
            TakerSwapEvent::MakerPaymentValidatedAndConfirmed => "Maker payment validated and confirmed...".to_owned(),
            TakerSwapEvent::MakerPaymentValidateFailed(_) => "Maker payment validate failed...".to_owned(),
            TakerSwapEvent::MakerPaymentWaitConfirmFailed(_) => {
                "Maker payment wait for confirmation failed...".to_owned()
            },
            TakerSwapEvent::TakerPaymentSent(_) => "Taker payment sent...".to_owned(),
            TakerSwapEvent::WatcherMessageSent(_, _) => WATCHER_MESSAGE_SENT_LOG.to_owned(),
            TakerSwapEvent::TakerPaymentTransactionFailed(_) => "Taker payment transaction failed...".to_owned(),
            TakerSwapEvent::TakerPaymentDataSendFailed(_) => "Taker payment data send failed...".to_owned(),
            TakerSwapEvent::TakerPaymentWaitConfirmFailed(_) => {
                "Taker payment wait for confirmation failed...".to_owned()
            },
            TakerSwapEvent::TakerPaymentSpent(_) => "Taker payment spent...".to_owned(),
            TakerSwapEvent::TakerPaymentWaitForSpendFailed(_) => "Taker payment wait for spend failed...".to_owned(),
            TakerSwapEvent::MakerPaymentSpent(_) => "Maker payment spent...".to_owned(),
            TakerSwapEvent::MakerPaymentSpendConfirmed => "Maker payment spent confirmed...".to_owned(),
            TakerSwapEvent::MakerPaymentSpendConfirmFailed(_) => "Maker payment spend confirm failed...".to_owned(),
            TakerSwapEvent::MakerPaymentSpentByWatcher(_) => "Maker payment spent by watcher...".to_owned(),
            TakerSwapEvent::MakerPaymentSpendFailed(_) => "Maker payment spend failed...".to_owned(),
            TakerSwapEvent::TakerPaymentWaitRefundStarted { wait_until } => {
                format!("Taker payment wait refund till {wait_until} started...")
            },
            TakerSwapEvent::TakerPaymentRefundStarted => "Taker payment refund started...".to_owned(),
            TakerSwapEvent::TakerPaymentRefunded(_) => "Taker payment refunded...".to_owned(),
            TakerSwapEvent::TakerPaymentRefundFailed(_) => "Taker payment refund failed...".to_owned(),
            TakerSwapEvent::TakerPaymentRefundFinished => "Taker payment refund finished...".to_owned(),
            TakerSwapEvent::TakerPaymentRefundedByWatcher(_) => "Taker payment refunded by watcher...".to_owned(),
            TakerSwapEvent::Finished => "Finished".to_owned(),
        }
    }

    fn should_ban_maker(&self) -> bool {
        matches!(
            self,
            TakerSwapEvent::MakerPaymentValidateFailed(_) | TakerSwapEvent::TakerPaymentWaitForSpendFailed(_)
        )
    }

    fn is_success(&self) -> bool {
        matches!(
            self,
            TakerSwapEvent::Started(_)
                | TakerSwapEvent::Negotiated(_)
                | TakerSwapEvent::TakerFeeSent(_)
                | TakerSwapEvent::TakerPaymentInstructionsReceived(_)
                | TakerSwapEvent::MakerPaymentReceived(_)
                | TakerSwapEvent::MakerPaymentWaitConfirmStarted
                | TakerSwapEvent::WatcherMessageSent(_, _)
                | TakerSwapEvent::MakerPaymentValidatedAndConfirmed
                | TakerSwapEvent::TakerPaymentSent(_)
                | TakerSwapEvent::TakerPaymentSpent(_)
                | TakerSwapEvent::MakerPaymentSpent(_)
                | TakerSwapEvent::MakerPaymentSpendConfirmed
                | TakerSwapEvent::MakerPaymentSpentByWatcher(_)
                | TakerSwapEvent::Finished
        )
    }

    fn is_error(&self) -> bool {
        !self.is_success()
    }
}

#[derive(Debug)]
pub enum TakerSwapCommand {
    Start,
    Negotiate,
    SendTakerFee,
    WaitForMakerPayment,
    ValidateMakerPayment,
    SendTakerPayment,
    WaitForTakerPaymentSpend,
    SpendMakerPayment,
    ConfirmMakerPaymentSpend,
    PrepareForTakerPaymentRefund,
    RefundTakerPayment,
    FinalizeTakerPaymentRefund,
    Finish,
}

impl TakerSwap {
    #[inline]
    fn w(&self) -> RwLockWriteGuard<'_, TakerSwapMut> {
        self.mutable.write().unwrap()
    }

    #[inline]
    pub fn r(&self) -> RwLockReadGuard<'_, TakerSwapMut> {
        self.mutable.read().unwrap()
    }

    #[inline]
    fn my_maker_coin_htlc_pub(&self) -> H264Json {
        self.r()
            .data
            .maker_coin_htlc_pubkey
            .unwrap_or_else(|| self.my_persistent_pub.into())
    }

    #[inline]
    fn my_taker_coin_htlc_pub(&self) -> H264Json {
        self.r()
            .data
            .taker_coin_htlc_pubkey
            .unwrap_or_else(|| self.my_persistent_pub.into())
    }

    #[inline]
    fn wait_refund_until(&self) -> u64 {
        self.r().data.taker_payment_lock + 3700
    }

    pub(crate) fn apply_event(&self, event: TakerSwapEvent) {
        match event {
            TakerSwapEvent::Started(data) => {
                self.w().data = data;
                log_tag!(
                    self.ctx,
                    "";
                    fmt = "Taker swap {} has successfully started",
                    self.uuid
                );
            },
            TakerSwapEvent::StartFailed(err) => self.errors.lock().push(err),
            TakerSwapEvent::Negotiated(data) => {
                self.maker_payment_lock
                    .store(data.maker_payment_locktime, Ordering::Relaxed);
                self.w().other_maker_coin_htlc_pub = data.other_maker_coin_htlc_pub();
                self.w().other_taker_coin_htlc_pub = data.other_taker_coin_htlc_pub();
                self.w().secret_hash = data.secret_hash;

                if data.maker_coin_swap_contract_addr.is_some() {
                    self.w().data.maker_coin_swap_contract_address = data.maker_coin_swap_contract_addr;
                }

                if data.taker_coin_swap_contract_addr.is_some() {
                    self.w().data.taker_coin_swap_contract_address = data.taker_coin_swap_contract_addr;
                }
            },
            TakerSwapEvent::NegotiateFailed(err) => self.errors.lock().push(err),
            TakerSwapEvent::TakerFeeSent(tx) => self.w().taker_fee = Some(tx),
            TakerSwapEvent::TakerFeeSendFailed(err) => self.errors.lock().push(err),
            TakerSwapEvent::TakerPaymentInstructionsReceived(instructions) => {
                self.w().payment_instructions = instructions
            },
            TakerSwapEvent::MakerPaymentReceived(tx) => self.w().maker_payment = Some(tx),
            TakerSwapEvent::MakerPaymentWaitConfirmStarted => (),
            TakerSwapEvent::MakerPaymentValidatedAndConfirmed => {
                self.maker_payment_confirmed.store(true, Ordering::Relaxed)
            },
            TakerSwapEvent::MakerPaymentValidateFailed(err) => self.errors.lock().push(err),
            TakerSwapEvent::MakerPaymentWaitConfirmFailed(err) => self.errors.lock().push(err),
            TakerSwapEvent::TakerPaymentSent(tx) => self.w().taker_payment = Some(tx),
            TakerSwapEvent::WatcherMessageSent(maker_payment_spend_preimage, taker_payment_refund_preimage) => {
                self.w().maker_payment_spend_preimage = maker_payment_spend_preimage;
                self.w().taker_payment_refund_preimage = taker_payment_refund_preimage;
            },
            TakerSwapEvent::TakerPaymentTransactionFailed(err) => self.errors.lock().push(err),
            TakerSwapEvent::TakerPaymentDataSendFailed(err) => self.errors.lock().push(err),
            TakerSwapEvent::TakerPaymentWaitConfirmFailed(err) => self.errors.lock().push(err),
            TakerSwapEvent::TakerPaymentSpent(data) => {
                self.w().taker_payment_spend = Some(data.transaction);
                self.w().secret = data.secret;
            },
            TakerSwapEvent::TakerPaymentWaitForSpendFailed(err) => self.errors.lock().push(err),
            TakerSwapEvent::MakerPaymentSpent(tx) => self.w().maker_payment_spend = Some(tx),
            TakerSwapEvent::MakerPaymentSpendConfirmed => self.w().maker_payment_spend_confirmed = true,
            TakerSwapEvent::MakerPaymentSpendConfirmFailed(err) => self.errors.lock().push(err),
            TakerSwapEvent::MakerPaymentSpentByWatcher(tx) => self.w().maker_payment_spend = Some(tx),
            TakerSwapEvent::MakerPaymentSpendFailed(err) => self.errors.lock().push(err),
            TakerSwapEvent::TakerPaymentWaitRefundStarted { .. } => (),
            TakerSwapEvent::TakerPaymentRefundStarted => (),
            TakerSwapEvent::TakerPaymentRefunded(tx) => self.w().taker_payment_refund = tx,
            TakerSwapEvent::TakerPaymentRefundFailed(err) => self.errors.lock().push(err),
            TakerSwapEvent::TakerPaymentRefundFinished => (),
            TakerSwapEvent::TakerPaymentRefundedByWatcher(tx) => self.w().taker_payment_refund = tx,
            TakerSwapEvent::Finished => self.finished_at.store(now_sec(), Ordering::Relaxed),
        }
    }

    async fn handle_command(
        &self,
        command: TakerSwapCommand,
    ) -> Result<(Option<TakerSwapCommand>, Vec<TakerSwapEvent>), String> {
        match command {
            TakerSwapCommand::Start => self.start().await,
            TakerSwapCommand::Negotiate => self.negotiate().await,
            TakerSwapCommand::SendTakerFee => self.send_taker_fee().await,
            TakerSwapCommand::WaitForMakerPayment => self.wait_for_maker_payment().await,
            TakerSwapCommand::ValidateMakerPayment => self.validate_maker_payment().await,
            TakerSwapCommand::SendTakerPayment => self.send_taker_payment().await,
            TakerSwapCommand::WaitForTakerPaymentSpend => self.wait_for_taker_payment_spend().await,
            TakerSwapCommand::SpendMakerPayment => self.spend_maker_payment().await,
            TakerSwapCommand::ConfirmMakerPaymentSpend => self.confirm_maker_payment_spend().await,
            TakerSwapCommand::PrepareForTakerPaymentRefund => self.prepare_for_taker_payment_refund().await,
            TakerSwapCommand::RefundTakerPayment => self.refund_taker_payment().await,
            TakerSwapCommand::FinalizeTakerPaymentRefund => self.finalize_taker_payment_refund().await,
            TakerSwapCommand::Finish => Ok((None, vec![TakerSwapEvent::Finished])),
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        ctx: MmArc,
        maker_pubkey: bits256,
        maker_amount: MmNumber,
        taker_amount: MmNumber,
        my_persistent_pub: H264,
        uuid: Uuid,
        my_order_uuid: Option<Uuid>,
        conf_settings: SwapConfirmationsSettings,
        maker_coin: MmCoinEnum,
        taker_coin: MmCoinEnum,
        payment_locktime: u64,
        p2p_privkey: Option<KeyPair>,
        #[cfg(any(test, feature = "run-docker-tests"))] fail_at: Option<FailAt>,
    ) -> Self {
        TakerSwap {
            maker_coin,
            taker_coin,
            maker_amount,
            taker_amount,
            my_persistent_pub,
            maker_pubkey,
            uuid,
            my_order_uuid,
            maker_payment_confirmed: AtomicBool::new(false),
            finished_at: AtomicU64::new(0),
            maker_payment_lock: AtomicU64::new(0),
            errors: PaMutex::new(Vec::new()),
            conf_settings,
            payment_locktime,
            p2p_privkey,
            mutable: RwLock::new(TakerSwapMut {
                data: TakerSwapData::default(),
                other_maker_coin_htlc_pub: H264::default(),
                other_taker_coin_htlc_pub: H264::default(),
                taker_fee: None,
                maker_payment: None,
                maker_payment_spend_confirmed: false,
                taker_payment: None,
                taker_payment_spend: None,
                maker_payment_spend_preimage: None,
                taker_payment_refund_preimage: None,
                maker_payment_spend: None,
                taker_payment_refund: None,
                secret_hash: BytesJson::default(),
                secret: H256Json::default(),
                watcher_reward: false,
                reward_amount: None,
                payment_instructions: None,
            }),
            ctx,
            #[cfg(any(test, feature = "run-docker-tests"))]
            fail_at,
        }
    }

    fn get_my_negotiation_data(
        &self,
        secret_hash: Vec<u8>,
        maker_coin_swap_contract: Vec<u8>,
        taker_coin_swap_contract: Vec<u8>,
    ) -> NegotiationDataMsg {
        let r = self.r();

        let equal = r.data.maker_coin_htlc_pubkey == r.data.taker_coin_htlc_pubkey;
        let same_as_persistent = r.data.maker_coin_htlc_pubkey == Some(r.data.my_persistent_pub);
        if equal && same_as_persistent {
            NegotiationDataMsg::V2(NegotiationDataV2 {
                started_at: r.data.started_at,
                secret_hash,
                payment_locktime: r.data.taker_payment_lock,
                persistent_pubkey: self.my_persistent_pub.into(),
                maker_coin_swap_contract,
                taker_coin_swap_contract,
            })
        } else {
            NegotiationDataMsg::V3(NegotiationDataV3 {
                started_at: r.data.started_at,
                payment_locktime: r.data.taker_payment_lock,
                secret_hash,
                maker_coin_swap_contract,
                taker_coin_swap_contract,
                maker_coin_htlc_pub: self.my_maker_coin_htlc_pub(),
                taker_coin_htlc_pub: self.my_taker_coin_htlc_pub(),
            })
        }
    }

    /// # Panic
    ///
    /// Panic if taker_fee of [`TakerSwapMut`] is [`Option::None`].
    async fn get_taker_fee_data(&self) -> Result<SwapTxDataMsg, MmError<PaymentInstructionsErr>> {
        // If taker fee is a lightning payment the payment hash will be sent in the message
        let taker_fee_data = self
            .r()
            .taker_fee
            .as_ref()
            .expect("TakerSwapMut::taker_fee must be some value to use get_taker_fee_data")
            .tx_hex
            .0
            .clone();
        let secret_hash = self.r().secret_hash.0.clone();
        let maker_amount = self.maker_amount.clone().into();
        let maker_lock_duration =
            (self.r().data.lock_duration as f64 * self.taker_coin.maker_locktime_multiplier()).ceil() as u64;
        let expires_in = wait_for_maker_payment_conf_duration(self.r().data.lock_duration);

        let watcher_reward = self.r().watcher_reward && self.taker_coin.is_eth();
        let wait_until = wait_for_maker_payment_conf_until(self.r().data.started_at, self.r().data.lock_duration);
        let instructions = self
            .maker_coin
            .maker_payment_instructions(PaymentInstructionArgs {
                secret_hash: &secret_hash,
                amount: maker_amount,
                maker_lock_duration,
                expires_in,
                watcher_reward,
                wait_until,
            })
            .await?;
        Ok(SwapTxDataMsg::new(taker_fee_data, instructions))
    }

    async fn start(&self) -> Result<(Option<TakerSwapCommand>, Vec<TakerSwapEvent>), String> {
        // do not use self.r().data here as it is not initialized at this step yet
        let stage = FeeApproxStage::StartSwap;
        let dex_fee = DexFee::new_with_taker_pubkey(
            self.taker_coin.deref(),
            self.maker_coin.ticker(),
            &self.taker_amount,
            &self.my_taker_coin_htlc_pub().0,
        );
        let preimage_value = TradePreimageValue::Exact(self.taker_amount.to_decimal());

        let fee_to_send_dex_fee = if matches!(dex_fee, DexFee::NoFee) {
            TradeFee {
                coin: self.taker_coin.ticker().to_owned(),
                amount: MmNumber::from(0),
                paid_from_trading_vol: false,
            }
        } else {
            let fee_to_send_dex_fee_fut = self.taker_coin.get_fee_to_send_taker_fee(dex_fee.clone(), stage);
            match fee_to_send_dex_fee_fut.await {
                Ok(fee) => fee,
                Err(e) => {
                    return Ok((
                        Some(TakerSwapCommand::Finish),
                        vec![TakerSwapEvent::StartFailed(
                            ERRL!("!taker_coin.get_fee_to_send_taker_fee {}", e).into(),
                        )],
                    ))
                },
            }
        };
        let get_sender_trade_fee_fut = self.taker_coin.get_sender_trade_fee(preimage_value, stage);
        let taker_payment_trade_fee = match get_sender_trade_fee_fut.await {
            Ok(fee) => fee,
            Err(e) => {
                return Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::StartFailed(
                        ERRL!("!taker_coin.get_sender_trade_fee {}", e).into(),
                    )],
                ))
            },
        };
        let maker_payment_spend_trade_fee_fut = self.maker_coin.get_receiver_trade_fee(stage);
        let maker_payment_spend_trade_fee = match maker_payment_spend_trade_fee_fut.compat().await {
            Ok(fee) => fee,
            Err(e) => {
                return Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::StartFailed(
                        ERRL!("!maker_coin.get_receiver_trade_fee {}", e).into(),
                    )],
                ))
            },
        };

        let params = TakerSwapPreparedParams {
            dex_fee: dex_fee.total_spend_amount(),
            fee_to_send_dex_fee: fee_to_send_dex_fee.clone(),
            taker_payment_trade_fee: taker_payment_trade_fee.clone(),
            maker_payment_spend_trade_fee: maker_payment_spend_trade_fee.clone(),
        };
        let check_balance_f = check_balance_for_taker_swap(
            &self.ctx,
            self.taker_coin.deref(),
            self.maker_coin.deref(),
            self.taker_amount.clone(),
            Some(&self.uuid),
            Some(params),
            stage,
        );
        if let Err(e) = check_balance_f.await {
            return Ok((
                Some(TakerSwapCommand::Finish),
                vec![TakerSwapEvent::StartFailed(
                    ERRL!("!check_balance_for_taker_swap {}", e).into(),
                )],
            ));
        }

        let started_at = now_sec();

        let maker_coin_start_block = match self.maker_coin.current_block().compat().await {
            Ok(b) => b,
            Err(e) => {
                return Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::StartFailed(
                        ERRL!("!maker_coin.current_block {}", e).into(),
                    )],
                ))
            },
        };

        let taker_coin_start_block = match self.taker_coin.current_block().compat().await {
            Ok(b) => b,
            Err(e) => {
                return Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::StartFailed(
                        ERRL!("!taker_coin.current_block {}", e).into(),
                    )],
                ))
            },
        };

        let maker_coin_swap_contract_address = self.maker_coin.swap_contract_address();
        let taker_coin_swap_contract_address = self.taker_coin.swap_contract_address();

        let unique_data = self.unique_swap_data();
        let maker_coin_htlc_pubkey = self.maker_coin.derive_htlc_pubkey(&unique_data);
        let taker_coin_htlc_pubkey = self.taker_coin.derive_htlc_pubkey(&unique_data);

        let data = TakerSwapData {
            taker_coin: self.taker_coin.ticker().to_owned(),
            maker_coin: self.maker_coin.ticker().to_owned(),
            maker_pubkey: self.maker_pubkey.bytes.into(),
            started_at,
            lock_duration: self.payment_locktime,
            maker_amount: self.maker_amount.to_decimal(),
            taker_amount: self.taker_amount.to_decimal(),
            maker_payment_confirmations: self.conf_settings.maker_coin_confs,
            maker_payment_requires_nota: Some(self.conf_settings.maker_coin_nota),
            taker_payment_confirmations: self.conf_settings.taker_coin_confs,
            taker_payment_requires_nota: Some(self.conf_settings.taker_coin_nota),
            taker_payment_lock: started_at + self.payment_locktime,
            my_persistent_pub: self.my_persistent_pub.into(),
            uuid: self.uuid,
            maker_payment_wait: wait_for_maker_payment_conf_until(started_at, self.payment_locktime),
            maker_coin_start_block,
            taker_coin_start_block,
            fee_to_send_taker_fee: Some(SavedTradeFee::from(fee_to_send_dex_fee)),
            taker_payment_trade_fee: Some(SavedTradeFee::from(taker_payment_trade_fee)),
            maker_payment_spend_trade_fee: Some(SavedTradeFee::from(maker_payment_spend_trade_fee)),
            maker_coin_swap_contract_address,
            taker_coin_swap_contract_address,
            maker_coin_htlc_pubkey: Some(maker_coin_htlc_pubkey.into()),
            taker_coin_htlc_pubkey: Some(taker_coin_htlc_pubkey.into()),
            p2p_privkey: self.p2p_privkey.map(SerializableSecp256k1Keypair::from),
        };

        // This will be done during order match
        self.w().watcher_reward = std::env::var("USE_WATCHER_REWARD").is_ok();

        Ok((Some(TakerSwapCommand::Negotiate), vec![TakerSwapEvent::Started(data)]))
    }

    async fn negotiate(&self) -> Result<(Option<TakerSwapCommand>, Vec<TakerSwapEvent>), String> {
        const NEGOTIATE_TIMEOUT_SEC: u64 = 90;

        let recv_fut = recv_swap_msg(
            self.ctx.clone(),
            |store| store.negotiation.take(),
            &self.uuid,
            NEGOTIATE_TIMEOUT_SEC,
        );
        let maker_data = match recv_fut.await {
            Ok(d) => d,
            Err(e) => {
                return Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::NegotiateFailed(ERRL!("{:?}", e).into())],
                ))
            },
        };

        debug!("Received maker negotiation data {:?}", maker_data);
        let time_dif = self.r().data.started_at.abs_diff(maker_data.started_at());
        if time_dif > MAX_STARTED_AT_DIFF {
            return Ok((Some(TakerSwapCommand::Finish), vec![TakerSwapEvent::NegotiateFailed(
                ERRL!("The time difference between you and the maker cannot be longer than 60 seconds. Current difference: {}. Please make sure that your system clock is synced to the correct time before starting another swap!", time_dif).into(),
            )]));
        }

        let customized_lock_duration =
            (self.r().data.lock_duration as f64 * self.taker_coin.maker_locktime_multiplier()).ceil() as u64;
        let expected_lock_time = maker_data.started_at().checked_add(customized_lock_duration);
        if Some(maker_data.payment_locktime()) != expected_lock_time {
            return Ok((
                Some(TakerSwapCommand::Finish),
                vec![TakerSwapEvent::NegotiateFailed(
                    ERRL!(
                        "maker_data.payment_locktime {:?} not equal to expected {:?}",
                        maker_data.payment_locktime(),
                        expected_lock_time
                    )
                    .into(),
                )],
            ));
        }

        let maker_coin_swap_contract_addr = match self
            .maker_coin
            .negotiate_swap_contract_addr(maker_data.maker_coin_swap_contract())
        {
            Ok(addr) => addr,
            Err(e) => match self.maker_coin.fallback_swap_contract() {
                // try to negotiate using fallback
                Some(addr) => Some(addr),
                None => {
                    return Ok((
                        Some(TakerSwapCommand::Finish),
                        vec![TakerSwapEvent::NegotiateFailed(
                            ERRL!("!maker_coin.negotiate_swap_contract_addr {}", e).into(),
                        )],
                    ))
                },
            },
        };

        let taker_coin_swap_contract_addr = match self
            .taker_coin
            .negotiate_swap_contract_addr(maker_data.taker_coin_swap_contract())
        {
            Ok(addr) => addr,
            Err(e) => match self.taker_coin.fallback_swap_contract() {
                // try to negotiate using fallback
                Some(addr) => Some(addr),
                None => {
                    return Ok((
                        Some(TakerSwapCommand::Finish),
                        vec![TakerSwapEvent::NegotiateFailed(
                            ERRL!("!taker_coin.negotiate_swap_contract_addr {}", e).into(),
                        )],
                    ))
                },
            },
        };

        // Validate maker_coin_htlc_pubkey realness
        if let Err(err) = self
            .maker_coin
            .validate_other_pubkey(&maker_data.maker_coin_htlc_pub().0)
        {
            return Ok((
                Some(TakerSwapCommand::Finish),
                vec![TakerSwapEvent::NegotiateFailed(
                    ERRL!("!maker_data.maker_coin_htlc_pub {}", err).into(),
                )],
            ));
        };

        // Validate taker_coin_htlc_pubkey realness
        if let Err(err) = self
            .taker_coin
            .validate_other_pubkey(&maker_data.taker_coin_htlc_pub().0)
        {
            return Ok((
                Some(TakerSwapCommand::Finish),
                vec![TakerSwapEvent::NegotiateFailed(
                    ERRL!("!maker_data.taker_coin_htlc_pub {}", err).into(),
                )],
            ));
        };

        if !(maker_data.secret_hash().len() == 20 || maker_data.secret_hash().len() == 32) {
            return Ok((
                Some(TakerSwapCommand::Finish),
                vec![TakerSwapEvent::NegotiateFailed(
                    ERRL!("!maker_data.secret_hash: secret_hash validation failed").into(),
                )],
            ));
        }

        let maker_coin_swap_contract_bytes = maker_coin_swap_contract_addr
            .clone()
            .map_or_else(Vec::new, |bytes| bytes.0);
        let taker_coin_swap_contract_bytes = taker_coin_swap_contract_addr
            .clone()
            .map_or_else(Vec::new, |bytes| bytes.0);

        let my_negotiation_data = self.get_my_negotiation_data(
            maker_data.secret_hash().to_vec(),
            maker_coin_swap_contract_bytes,
            taker_coin_swap_contract_bytes,
        );

        let (topic, taker_data) = (swap_topic(&self.uuid), SwapMsg::NegotiationReply(my_negotiation_data));

        debug!("Sending taker negotiation data {:?}", taker_data);
        let send_abort_handle = broadcast_swap_msg_every(
            self.ctx.clone(),
            topic,
            taker_data,
            NEGOTIATE_TIMEOUT_SEC as f64 / 6.,
            self.p2p_privkey,
        );

        let recv_fut = recv_swap_msg(
            self.ctx.clone(),
            |store| store.negotiated.take(),
            &self.uuid,
            NEGOTIATE_TIMEOUT_SEC,
        );
        let negotiated = match recv_fut.await {
            Ok(d) => d,
            Err(e) => {
                return Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::NegotiateFailed(ERRL!("{:?}", e).into())],
                ))
            },
        };
        drop(send_abort_handle);

        if !negotiated {
            return Ok((
                Some(TakerSwapCommand::Finish),
                vec![TakerSwapEvent::NegotiateFailed(
                    ERRL!("Maker sent negotiated = false").into(),
                )],
            ));
        }

        Ok((
            Some(TakerSwapCommand::SendTakerFee),
            vec![TakerSwapEvent::Negotiated(MakerNegotiationData {
                maker_payment_locktime: maker_data.payment_locktime(),
                // using default to avoid misuse of this field
                // maker_coin_htlc_pubkey and taker_coin_htlc_pubkey must be used instead
                maker_pubkey: H264Json::default(),
                secret_hash: maker_data.secret_hash().into(),
                maker_coin_swap_contract_addr,
                taker_coin_swap_contract_addr,
                maker_coin_htlc_pubkey: Some(*maker_data.maker_coin_htlc_pub()),
                taker_coin_htlc_pubkey: Some(*maker_data.taker_coin_htlc_pub()),
            })],
        ))
    }

    async fn send_taker_fee(&self) -> Result<(Option<TakerSwapCommand>, Vec<TakerSwapEvent>), String> {
        let expire_at = self.r().data.started_at + self.r().data.lock_duration / 3;
        let now = now_sec();
        if now > expire_at {
            return Ok((
                Some(TakerSwapCommand::Finish),
                vec![TakerSwapEvent::TakerFeeSendFailed(
                    ERRL!("Timeout {} > {}", now, expire_at).into(),
                )],
            ));
        }
        let dex_fee = DexFee::new_with_taker_pubkey(
            self.taker_coin.deref(),
            &self.r().data.maker_coin,
            &self.taker_amount,
            &self.my_taker_coin_htlc_pub().0,
        );
        if matches!(dex_fee, DexFee::NoFee) {
            info!("Taker fee tx not sent for dex taker");
            let empty_tx_ident = TransactionIdentifier {
                tx_hex: BytesJson::from(vec![]),
                tx_hash: BytesJson::from(vec![]),
            };
            return Ok((
                Some(TakerSwapCommand::WaitForMakerPayment),
                vec![TakerSwapEvent::TakerFeeSent(empty_tx_ident)],
            ));
        }
        let fee_tx = self
            .taker_coin
            .send_taker_fee(dex_fee, self.uuid.as_bytes(), expire_at)
            .await;
        let transaction = match fee_tx {
            Ok(t) => t,
            Err(err) => {
                return Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::TakerFeeSendFailed(
                        ERRL!("{}", err.get_plain_text_format()).into(),
                    )],
                ));
            },
        };

        let tx_hash = transaction.tx_hash_as_bytes();
        info!("Taker fee tx hash {:02x}", tx_hash);
        let tx_ident = TransactionIdentifier {
            tx_hex: BytesJson::from(transaction.tx_hex()),
            tx_hash,
        };

        Ok((
            Some(TakerSwapCommand::WaitForMakerPayment),
            vec![TakerSwapEvent::TakerFeeSent(tx_ident)],
        ))
    }

    async fn wait_for_maker_payment(&self) -> Result<(Option<TakerSwapCommand>, Vec<TakerSwapEvent>), String> {
        const MAKER_PAYMENT_WAIT_TIMEOUT_SEC: u64 = 600;

        let payment_data_msg = match self.get_taker_fee_data().await {
            Ok(data) => data,
            Err(e) => {
                return Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::MakerPaymentValidateFailed(e.to_string().into())],
                ))
            },
        };

        let msg = SwapMsg::TakerFee(payment_data_msg);
        let abort_send_handle = broadcast_swap_msg_every(
            self.ctx.clone(),
            swap_topic(&self.uuid),
            msg,
            MAKER_PAYMENT_WAIT_TIMEOUT_SEC as f64 / 6.,
            self.p2p_privkey,
        );

        let recv_fut = recv_swap_msg(
            self.ctx.clone(),
            |store| store.maker_payment.take(),
            &self.uuid,
            MAKER_PAYMENT_WAIT_TIMEOUT_SEC,
        );
        let payload = match recv_fut.await {
            Ok(p) => p,
            Err(e) => {
                return Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::MakerPaymentValidateFailed(
                        ERRL!("Error waiting for 'maker-payment' data: {}", e).into(),
                    )],
                ))
            },
        };
        drop(abort_send_handle);

        let mut swap_events = Vec::with_capacity(3);
        let instructions = match payload.instructions() {
            Some(instructions) => {
                match self.taker_coin.validate_taker_payment_instructions(
                    instructions,
                    PaymentInstructionArgs {
                        secret_hash: &self.r().secret_hash.0,
                        amount: self.taker_amount.to_decimal(),
                        ..Default::default()
                    },
                ) {
                    Ok(instructions) => Some(instructions),
                    Err(e) => {
                        return Ok((
                            Some(TakerSwapCommand::Finish),
                            vec![TakerSwapEvent::MakerPaymentValidateFailed(e.to_string().into())],
                        ))
                    },
                }
            },
            None => None,
        };
        swap_events.push(TakerSwapEvent::TakerPaymentInstructionsReceived(instructions));

        let maker_payment = match self.maker_coin.tx_enum_from_bytes(payload.data()) {
            Ok(p) => p,
            Err(e) => {
                return Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::MakerPaymentValidateFailed(
                        ERRL!("Error parsing the 'maker-payment': {:?}", e).into(),
                    )],
                ))
            },
        };

        let tx_hash = maker_payment.tx_hash_as_bytes();
        info!("Got maker payment {:02x}", tx_hash);
        let tx_ident = TransactionIdentifier {
            tx_hex: BytesJson::from(maker_payment.tx_hex()),
            tx_hash,
        };

        swap_events.push(TakerSwapEvent::MakerPaymentReceived(tx_ident));
        swap_events.push(TakerSwapEvent::MakerPaymentWaitConfirmStarted);

        Ok((Some(TakerSwapCommand::ValidateMakerPayment), swap_events))
    }

    async fn validate_maker_payment(&self) -> Result<(Option<TakerSwapCommand>, Vec<TakerSwapEvent>), String> {
        info!("Before wait confirm");
        let confirmations = self.r().data.maker_payment_confirmations;
        let confirm_maker_payment_input = ConfirmPaymentInput {
            payment_tx: self.r().maker_payment.clone().unwrap().tx_hex.0,
            confirmations,
            requires_nota: self.r().data.maker_payment_requires_nota.unwrap_or(false),
            wait_until: self.r().data.maker_payment_wait,
            check_every: WAIT_CONFIRM_INTERVAL_SEC,
        };

        let f = self.maker_coin.wait_for_confirmations(confirm_maker_payment_input);
        if let Err(err) = f.compat().await {
            return Ok((
                Some(TakerSwapCommand::Finish),
                vec![TakerSwapEvent::MakerPaymentWaitConfirmFailed(
                    ERRL!("!wait for maker payment confirmations: {}", err).into(),
                )],
            ));
        }
        info!("After wait confirm");

        let reward_amount = self.r().reward_amount.clone();
        let wait_maker_payment_until = self.r().data.maker_payment_wait;
        let watcher_reward = if self.r().watcher_reward {
            match self
                .maker_coin
                .get_maker_watcher_reward(&self.taker_coin, reward_amount, wait_maker_payment_until)
                .await
            {
                Ok(reward) => reward,
                Err(err) => {
                    return Ok((
                        Some(TakerSwapCommand::Finish),
                        vec![TakerSwapEvent::TakerPaymentTransactionFailed(
                            err.into_inner().to_string().into(),
                        )],
                    ))
                },
            }
        } else {
            None
        };

        let validate_input = ValidatePaymentInput {
            payment_tx: self.r().maker_payment.clone().unwrap().tx_hex.0,
            time_lock_duration: self.r().data.lock_duration,
            time_lock: self.maker_payment_lock.load(Ordering::Relaxed),
            other_pub: self.r().other_maker_coin_htlc_pub.to_vec(),
            secret_hash: self.r().secret_hash.0.to_vec(),
            amount: self.maker_amount.to_decimal(),
            swap_contract_address: self.r().data.maker_coin_swap_contract_address.clone(),
            try_spv_proof_until: self.r().data.maker_payment_wait,
            confirmations,
            unique_swap_data: self.unique_swap_data(),
            watcher_reward,
        };
        let validated = self.maker_coin.validate_maker_payment(validate_input).await;

        if let Err(e) = validated {
            return Ok((
                Some(TakerSwapCommand::Finish),
                vec![TakerSwapEvent::MakerPaymentValidateFailed(
                    ERRL!("!validate maker payment: {}", e).into(),
                )],
            ));
        }

        Ok((
            Some(TakerSwapCommand::SendTakerPayment),
            vec![TakerSwapEvent::MakerPaymentValidatedAndConfirmed],
        ))
    }

    fn create_watcher_data(
        &self,
        taker_payment_hash: Vec<u8>,
        maker_payment_spend_preimage: Vec<u8>,
        taker_payment_refund_preimage: Vec<u8>,
    ) -> TakerSwapWatcherData {
        TakerSwapWatcherData {
            uuid: self.uuid,
            secret_hash: self.r().secret_hash.clone().into(),
            maker_payment_spend_preimage,
            taker_payment_refund_preimage,
            swap_started_at: self.r().data.started_at,
            lock_duration: self.r().data.lock_duration,
            taker_coin: self.r().data.taker_coin.clone(),
            taker_fee_hash: self.r().taker_fee.as_ref().unwrap().tx_hash.0.clone(),
            taker_payment_hash,
            taker_coin_start_block: self.r().data.taker_coin_start_block,
            taker_payment_confirmations: self.r().data.taker_payment_confirmations,
            taker_payment_requires_nota: self.r().data.taker_payment_requires_nota,
            maker_coin: self.r().data.maker_coin.clone(),
            maker_pub: self.r().other_maker_coin_htlc_pub.to_vec(),
            maker_payment_hash: self.r().maker_payment.as_ref().unwrap().tx_hash.0.clone(),
            maker_coin_start_block: self.r().data.maker_coin_start_block,
        }
    }

    /// Sets up the watcher reward for the taker's payment in the swap.
    ///
    /// The reward mainly serves as compensation to watchers for the mining fees
    /// paid to execute the transactions.
    ///
    /// The reward configuration depends on the specific requirements of the coins
    /// involved in the swap.
    /// Some coins may not support watcher rewards at all.
    async fn setup_watcher_reward(&self, taker_payment_lock: u64) -> Result<Option<WatcherReward>, String> {
        if !self.r().watcher_reward {
            return Ok(None);
        }

        let reward_amount = self.r().reward_amount.clone();
        self.taker_coin
            .get_taker_watcher_reward(
                &self.maker_coin,
                Some(self.taker_amount.clone().into()),
                Some(self.maker_amount.clone().into()),
                reward_amount,
                taker_payment_lock,
            )
            .await
            .map(Some)
            .map_err(|err| ERRL!("Watcher reward error: {}", err.to_string()))
    }

    /// Processes watcher-related logic for the swap by preparing and broadcasting necessary data.
    ///
    /// This function creates spend/refund preimages and broadcasts them to watchers if both coins
    /// support watcher functionality and watchers are enabled.
    ///
    /// The preimages allow watchers to either complete the swap by spending the maker payment
    /// or refund the taker payment if needed.
    async fn process_watcher_logic(&self, transaction: &TransactionEnum) -> Option<TakerSwapEvent> {
        let watchers_enabled_and_supported = !self.ctx.disable_watchers_globally()
            && self.taker_coin.is_supported_by_watchers()
            && self.maker_coin.is_supported_by_watchers();

        if !watchers_enabled_and_supported {
            return None;
        }

        let maker_payment_spend_preimage_fut = self.maker_coin.create_maker_payment_spend_preimage(
            &self.r().maker_payment.as_ref().unwrap().tx_hex,
            self.maker_payment_lock.load(Ordering::Relaxed),
            self.r().other_maker_coin_htlc_pub.as_slice(),
            &self.r().secret_hash.0,
            &self.unique_swap_data()[..],
        );

        let taker_payment_refund_preimage_fut = self.taker_coin.create_taker_payment_refund_preimage(
            &transaction.tx_hex(),
            self.r().data.taker_payment_lock,
            &*self.r().other_taker_coin_htlc_pub,
            &self.r().secret_hash.0,
            &self.r().data.taker_coin_swap_contract_address,
            &self.unique_swap_data(),
        );

        match try_join(
            maker_payment_spend_preimage_fut.compat(),
            taker_payment_refund_preimage_fut.compat(),
        )
        .await
        {
            Ok((maker_payment_spend, taker_payment_refund)) => {
                let watcher_data = self.create_watcher_data(
                    transaction.tx_hash_as_bytes().into_vec(),
                    maker_payment_spend.tx_hex(),
                    taker_payment_refund.tx_hex(),
                );
                let swpmsg_watcher = SwapWatcherMsg::TakerSwapWatcherMsg(watcher_data);

                let htlc_keypair = self.taker_coin.derive_htlc_key_pair(&self.unique_swap_data());
                broadcast_swap_message(
                    &self.ctx,
                    watcher_topic(&self.r().data.taker_coin),
                    swpmsg_watcher,
                    &Some(htlc_keypair),
                );

                info!("{}", WATCHER_MESSAGE_SENT_LOG);
                Some(TakerSwapEvent::WatcherMessageSent(
                    Some(maker_payment_spend.tx_hex()),
                    Some(taker_payment_refund.tx_hex()),
                ))
            },
            Err(e) => {
                error!(
                    "The watcher message could not be sent, error creating at least one of the preimages: {}",
                    e.get_plain_text_format()
                );
                None
            },
        }
    }

    async fn send_taker_payment(&self) -> Result<(Option<TakerSwapCommand>, Vec<TakerSwapEvent>), String> {
        #[cfg(test)]
        if self.fail_at == Some(FailAt::TakerPayment) {
            return Ok((
                Some(TakerSwapCommand::Finish),
                vec![TakerSwapEvent::TakerPaymentTransactionFailed(
                    "Explicit test failure".into(),
                )],
            ));
        }

        // Extract values from the lock before async operations
        let taker_payment_lock = self.r().data.taker_payment_lock;
        let other_taker_coin_htlc_pub = self.r().other_taker_coin_htlc_pub;
        let secret_hash = self.r().secret_hash.clone();
        let taker_coin_start_block = self.r().data.taker_coin_start_block;
        let taker_coin_swap_contract_address = self.r().data.taker_coin_swap_contract_address.clone();
        let unique_data = self.unique_swap_data();
        let taker_amount_decimal = self.taker_amount.to_decimal();
        let payment_instructions = self.r().payment_instructions.clone();

        // Look for previously sent taker payment in case of restart
        let maybe_existing_payment = match self
            .taker_coin
            .check_if_my_payment_sent(CheckIfMyPaymentSentArgs {
                time_lock: taker_payment_lock,
                other_pub: other_taker_coin_htlc_pub.as_slice(),
                secret_hash: &secret_hash.0,
                search_from_block: taker_coin_start_block,
                swap_contract_address: &taker_coin_swap_contract_address,
                swap_unique_data: &unique_data,
                amount: &taker_amount_decimal,
                payment_instructions: &payment_instructions,
            })
            .await
        {
            Ok(Some(tx)) => Some(tx),
            Ok(None) => None,
            Err(e) => {
                return Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::TakerPaymentTransactionFailed(ERRL!("{}", e).into())],
                ))
            },
        };

        // If the payment is not yet sent, make sure we didn't miss the deadline for sending it.
        if maybe_existing_payment.is_none() {
            let timeout = self.r().data.maker_payment_wait;
            let now = now_sec();
            if now > timeout {
                return Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::TakerPaymentTransactionFailed(
                        ERRL!("Timeout {} > {}", now, timeout).into(),
                    )],
                ));
            }
        }

        // Set up watcher reward if enable
        let watcher_reward = match self.setup_watcher_reward(taker_payment_lock).await {
            Ok(reward) => reward,
            Err(err) => {
                return Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::TakerPaymentTransactionFailed(err.into())],
                ));
            },
        };

        // Use existing payment or create new one
        let transaction = match maybe_existing_payment {
            Some(tx) => tx,
            None => {
                let lock_duration = self.r().data.lock_duration;
                match self
                    .taker_coin
                    .send_taker_payment(SendPaymentArgs {
                        time_lock_duration: lock_duration,
                        time_lock: taker_payment_lock,
                        other_pubkey: &*other_taker_coin_htlc_pub,
                        secret_hash: &secret_hash.0,
                        amount: taker_amount_decimal,
                        swap_contract_address: &taker_coin_swap_contract_address,
                        swap_unique_data: &unique_data,
                        payment_instructions: &payment_instructions,
                        watcher_reward,
                        wait_for_confirmation_until: taker_payment_lock,
                    })
                    .await
                {
                    Ok(t) => t,
                    Err(err) => {
                        return Ok((
                            Some(TakerSwapCommand::Finish),
                            vec![TakerSwapEvent::TakerPaymentTransactionFailed(
                                ERRL!("{}", err.get_plain_text_format()).into(),
                            )],
                        ))
                    },
                }
            },
        };

        // Create transaction identifier and prepare `TakerPaymentSent` success event
        let tx_hash = transaction.tx_hash_as_bytes();
        let tx_hex = BytesJson::from(transaction.tx_hex());
        info!("Taker payment tx hash {:02x}", tx_hash);
        let tx_ident = TransactionIdentifier {
            tx_hex: tx_hex.clone(),
            tx_hash,
        };
        let mut swap_events = vec![TakerSwapEvent::TakerPaymentSent(tx_ident)];

        // Process watcher logic if enabled and supported by both coins
        if let Some(watcher_event) = self.process_watcher_logic(&transaction).await {
            swap_events.push(watcher_event);
        }

        Ok((Some(TakerSwapCommand::WaitForTakerPaymentSpend), swap_events))
    }

    async fn wait_for_taker_payment_spend(&self) -> Result<(Option<TakerSwapCommand>, Vec<TakerSwapEvent>), String> {
        const BROADCAST_MSG_INTERVAL_SEC: f64 = 600.;

        let tx_hex = self.r().taker_payment.as_ref().unwrap().tx_hex.clone();
        let mut watcher_broadcast_abort_handle = None;
        // Watchers cannot be used for lightning swaps for now
        // Todo: Check if watchers can work in some cases with lightning and implement it if it's possible, this part will probably work if only the taker is lightning since the preimage is available
        if !self.ctx.disable_watchers_globally()
            && self.taker_coin.is_supported_by_watchers()
            && self.maker_coin.is_supported_by_watchers()
        {
            if let (Some(maker_payment_spend), Some(taker_payment_refund)) = (
                self.r().maker_payment_spend_preimage.clone(),
                self.r().taker_payment_refund_preimage.clone(),
            ) {
                let watcher_data = self.create_watcher_data(
                    self.r().taker_payment.as_ref().unwrap().tx_hash.0.clone(),
                    maker_payment_spend,
                    taker_payment_refund,
                );
                let swpmsg_watcher = SwapWatcherMsg::TakerSwapWatcherMsg(watcher_data);
                let htlc_keypair = self.taker_coin.derive_htlc_key_pair(&self.unique_swap_data());
                watcher_broadcast_abort_handle = Some(broadcast_swap_msg_every_delayed(
                    self.ctx.clone(),
                    watcher_topic(&self.r().data.taker_coin),
                    swpmsg_watcher,
                    BROADCAST_MSG_INTERVAL_SEC,
                    Some(htlc_keypair),
                ));
            }
        }

        // Todo: taker_payment should be a message on lightning network not a swap message
        let msg = SwapMsg::TakerPayment(tx_hex.0.clone());
        let send_abort_handle = broadcast_swap_msg_every(
            self.ctx.clone(),
            swap_topic(&self.uuid),
            msg,
            BROADCAST_MSG_INTERVAL_SEC,
            self.p2p_privkey,
        );

        #[cfg(any(test, feature = "run-docker-tests"))]
        if self.fail_at == Some(FailAt::WaitForTakerPaymentSpendPanic) {
            // Wait for 5 seconds before panicking to ensure the message is sent
            Timer::sleep(5.).await;
            panic!("Taker panicked unexpectedly at wait for taker payment spend");
        }

        info!("Waiting for maker to spend taker payment!");

        let wait_until = self.r().data.taker_payment_lock;
        let secret_hash = self.r().secret_hash.clone();
        let taker_coin_start_block = self.r().data.taker_coin_start_block;
        let taker_coin_swap_contract_address = self.r().data.taker_coin_swap_contract_address.clone();
        let watcher_reward = self.r().watcher_reward;
        let f = self.taker_coin.wait_for_htlc_tx_spend(WaitForHTLCTxSpendArgs {
            tx_bytes: &tx_hex,
            secret_hash: &secret_hash.0,
            wait_until,
            from_block: taker_coin_start_block,
            swap_contract_address: &taker_coin_swap_contract_address,
            check_every: TAKER_PAYMENT_SPEND_SEARCH_INTERVAL,
            watcher_reward,
        });
        let tx = match f.await {
            Ok(t) => t,
            Err(err) => {
                return Ok((
                    Some(TakerSwapCommand::PrepareForTakerPaymentRefund),
                    vec![
                        TakerSwapEvent::TakerPaymentWaitForSpendFailed(err.get_plain_text_format().into()),
                        TakerSwapEvent::TakerPaymentWaitRefundStarted {
                            wait_until: self.wait_refund_until(),
                        },
                    ],
                ));
            },
        };
        drop(send_abort_handle);
        drop(watcher_broadcast_abort_handle);
        let tx_hash = tx.tx_hash_as_bytes();
        info!("Taker payment spend tx {:02x}", tx_hash);
        let tx_ident = TransactionIdentifier {
            tx_hex: BytesJson::from(tx.tx_hex()),
            tx_hash,
        };

        let secret = match self
            .taker_coin
            .extract_secret(&secret_hash.0, &tx_ident.tx_hex, watcher_reward)
            .await
        {
            Ok(secret) => H256Json::from(secret),
            Err(e) => {
                return Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::TakerPaymentWaitForSpendFailed(ERRL!("{}", e).into())],
                ))
            },
        };

        Ok((
            Some(TakerSwapCommand::SpendMakerPayment),
            vec![TakerSwapEvent::TakerPaymentSpent(TakerPaymentSpentData {
                transaction: tx_ident,
                secret,
            })],
        ))
    }

    async fn spend_maker_payment(&self) -> Result<(Option<TakerSwapCommand>, Vec<TakerSwapEvent>), String> {
        #[cfg(any(test, feature = "run-docker-tests"))]
        if self.fail_at == Some(FailAt::MakerPaymentSpend) {
            return Ok((
                Some(TakerSwapCommand::Finish),
                vec![TakerSwapEvent::MakerPaymentSpendFailed("Explicit test failure".into())],
            ));
        } else if self.fail_at == Some(FailAt::MakerPaymentSpendPanic) {
            panic!("Taker panicked unexpectedly at maker payment spend");
        }

        let other_maker_coin_htlc_pub = self.r().other_maker_coin_htlc_pub;
        let secret = self.r().secret;
        let taker_spends_payment_args = SpendPaymentArgs {
            other_payment_tx: &self.r().maker_payment.clone().unwrap().tx_hex,
            time_lock: self.maker_payment_lock.load(Ordering::Relaxed),
            other_pubkey: &*other_maker_coin_htlc_pub,
            secret: &secret.0,
            secret_hash: &self.r().secret_hash.clone().0,
            swap_contract_address: &self.r().data.maker_coin_swap_contract_address.clone(),
            swap_unique_data: &self.unique_swap_data(),
            watcher_reward: self.r().watcher_reward,
        };
        let maybe_spend_tx = self
            .maker_coin
            .send_taker_spends_maker_payment(taker_spends_payment_args)
            .await;
        let transaction = match maybe_spend_tx {
            Ok(t) => t,
            Err(err) => {
                if let Some(tx) = err.get_tx() {
                    broadcast_p2p_tx_msg(
                        &self.ctx,
                        tx_helper_topic(self.maker_coin.ticker()),
                        &tx,
                        &self.p2p_privkey,
                    );
                };

                return Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::MakerPaymentSpendFailed(
                        ERRL!("{}", err.get_plain_text_format()).into(),
                    )],
                ));
            },
        };

        broadcast_p2p_tx_msg(
            &self.ctx,
            tx_helper_topic(self.maker_coin.ticker()),
            &transaction,
            &self.p2p_privkey,
        );

        let tx_hash = transaction.tx_hash_as_bytes();
        info!("Maker payment spend tx {:02x}", tx_hash);
        let tx_ident = TransactionIdentifier {
            tx_hex: BytesJson::from(transaction.tx_hex()),
            tx_hash,
        };

        Ok((
            Some(TakerSwapCommand::ConfirmMakerPaymentSpend),
            vec![TakerSwapEvent::MakerPaymentSpent(tx_ident)],
        ))
    }

    async fn confirm_maker_payment_spend(&self) -> Result<(Option<TakerSwapCommand>, Vec<TakerSwapEvent>), String> {
        let confirm_maker_payment_spend_input = ConfirmPaymentInput {
            payment_tx: self.r().maker_payment_spend.clone().unwrap().tx_hex.0,
            confirmations: self.r().data.maker_payment_confirmations,
            requires_nota: false,
            wait_until: self.wait_refund_until(),
            check_every: WAIT_CONFIRM_INTERVAL_SEC,
        };
        let wait_fut = self
            .maker_coin
            .wait_for_confirmations(confirm_maker_payment_spend_input);
        if let Err(err) = wait_fut.compat().await {
            return Ok((
                Some(TakerSwapCommand::PrepareForTakerPaymentRefund),
                vec![
                    TakerSwapEvent::MakerPaymentSpendConfirmFailed(
                        ERRL!("!wait for maker payment spend confirmations: {}", err).into(),
                    ),
                    TakerSwapEvent::TakerPaymentWaitRefundStarted {
                        wait_until: self.wait_refund_until(),
                    },
                ],
            ));
        }
        info!("Maker payment spend confirmed");
        Ok((
            Some(TakerSwapCommand::Finish),
            vec![TakerSwapEvent::MakerPaymentSpendConfirmed],
        ))
    }

    async fn prepare_for_taker_payment_refund(
        &self,
    ) -> Result<(Option<TakerSwapCommand>, Vec<TakerSwapEvent>), String> {
        let maker_payment = self.r().maker_payment.clone().unwrap().tx_hex;
        if let Err(e) = self.maker_coin.on_taker_payment_refund_start(&maker_payment).await {
            error!("Error {} on calling on_taker_payment_refund_start!", e)
        }

        Ok((
            Some(TakerSwapCommand::RefundTakerPayment),
            vec![TakerSwapEvent::TakerPaymentRefundStarted],
        ))
    }

    async fn refund_taker_payment(&self) -> Result<(Option<TakerSwapCommand>, Vec<TakerSwapEvent>), String> {
        #[cfg(any(test, feature = "run-docker-tests"))]
        if self.fail_at == Some(FailAt::TakerPaymentRefund) {
            return Ok((
                Some(TakerSwapCommand::Finish),
                vec![TakerSwapEvent::TakerPaymentRefundFailed("Explicit test failure".into())],
            ));
        } else if self.fail_at == Some(FailAt::TakerPaymentRefundPanic) {
            panic!("{}", REFUND_TEST_FAILURE_LOG);
        }

        let taker_payment = self.r().taker_payment.clone().unwrap().tx_hex;
        let locktime = self.r().data.taker_payment_lock;
        if self.taker_coin.is_auto_refundable() {
            return match self.taker_coin.wait_for_htlc_refund(&taker_payment, locktime).await {
                Ok(()) => Ok((
                    Some(TakerSwapCommand::FinalizeTakerPaymentRefund),
                    vec![TakerSwapEvent::TakerPaymentRefunded(None)],
                )),
                Err(err) => Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::TakerPaymentRefundFailed(
                        ERRL!("!taker_coin.wait_for_htlc_refund: {}", err.to_string()).into(),
                    )],
                )),
            };
        }

        loop {
            match self.taker_coin.can_refund_htlc(locktime).await {
                Ok(CanRefundHtlc::CanRefundNow) => break,
                Ok(CanRefundHtlc::HaveToWait(to_sleep)) => Timer::sleep(to_sleep as f64).await,
                Err(e) => {
                    error!("Error {} on can_refund_htlc, retrying in 30 seconds", e);
                    Timer::sleep(30.).await;
                },
            }
        }

        let other_taker_coin_htlc_pub = self.r().other_taker_coin_htlc_pub;
        let secret_hash = self.r().secret_hash.clone();
        let swap_contract_address = self.r().data.taker_coin_swap_contract_address.clone();
        let watcher_reward = self.r().watcher_reward;
        let refund_result = self
            .taker_coin
            .send_taker_refunds_payment(RefundPaymentArgs {
                payment_tx: &taker_payment,
                time_lock: locktime,
                other_pubkey: other_taker_coin_htlc_pub.as_slice(),
                tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
                    maker_secret_hash: &secret_hash,
                },
                swap_contract_address: &swap_contract_address,
                swap_unique_data: &self.unique_swap_data(),
                watcher_reward,
            })
            .await;

        let transaction = match refund_result {
            Ok(t) => t,
            Err(err) => {
                if let Some(tx) = err.get_tx() {
                    broadcast_p2p_tx_msg(
                        &self.ctx,
                        tx_helper_topic(self.taker_coin.ticker()),
                        &tx,
                        &self.p2p_privkey,
                    );
                }

                return Ok((
                    Some(TakerSwapCommand::Finish),
                    vec![TakerSwapEvent::TakerPaymentRefundFailed(
                        ERRL!("{:?}", err.get_plain_text_format()).into(),
                    )],
                ));
            },
        };

        broadcast_p2p_tx_msg(
            &self.ctx,
            tx_helper_topic(self.taker_coin.ticker()),
            &transaction,
            &self.p2p_privkey,
        );

        let tx_hash = transaction.tx_hash_as_bytes();
        info!("Taker refund tx hash {:02x}", tx_hash);
        let tx_ident = TransactionIdentifier {
            tx_hex: BytesJson::from(transaction.tx_hex()),
            tx_hash,
        };

        Ok((
            Some(TakerSwapCommand::FinalizeTakerPaymentRefund),
            vec![TakerSwapEvent::TakerPaymentRefunded(Some(tx_ident))],
        ))
    }

    async fn finalize_taker_payment_refund(&self) -> Result<(Option<TakerSwapCommand>, Vec<TakerSwapEvent>), String> {
        let maker_payment = self.r().maker_payment.clone().unwrap().tx_hex;
        if let Err(e) = self.maker_coin.on_taker_payment_refund_success(&maker_payment).await {
            error!("Error {} on calling on_taker_payment_refund_success!", e)
        }

        Ok((
            Some(TakerSwapCommand::Finish),
            vec![TakerSwapEvent::TakerPaymentRefundFinished],
        ))
    }

    pub async fn load_from_db_by_uuid(
        ctx: MmArc,
        maker_coin: MmCoinEnum,
        taker_coin: MmCoinEnum,
        swap_uuid: &Uuid,
    ) -> Result<(Self, Option<TakerSwapCommand>), String> {
        let saved = match SavedSwap::load_my_swap_from_db(&ctx, None, *swap_uuid).await {
            Ok(Some(saved)) => saved,
            Ok(None) => return ERR!("Couldn't find a swap with the uuid '{}'", swap_uuid),
            Err(e) => return ERR!("{}", e),
        };
        let saved = match saved {
            SavedSwap::Taker(swap) => swap,
            SavedSwap::Maker(_) => return ERR!("Can not load TakerSwap from SavedSwap::Maker uuid: {}", swap_uuid),
        };
        Self::load_from_saved(ctx, maker_coin, taker_coin, saved).await
    }

    pub async fn load_from_saved(
        ctx: MmArc,
        maker_coin: MmCoinEnum,
        taker_coin: MmCoinEnum,
        mut saved: TakerSavedSwap,
    ) -> Result<(Self, Option<TakerSwapCommand>), String> {
        if saved.events.is_empty() {
            return ERR!("Can't restore swap from empty events set");
        };

        let data = match saved.events[0].event {
            TakerSwapEvent::Started(ref mut data) => data,
            _ => return ERR!("First swap event must be Started"),
        };

        // refresh swap contract addresses if the swap file is out-dated (doesn't contain the fields yet)
        if data.maker_coin_swap_contract_address.is_none() {
            data.maker_coin_swap_contract_address = maker_coin.swap_contract_address();
        }
        if data.taker_coin_swap_contract_address.is_none() {
            data.taker_coin_swap_contract_address = taker_coin.swap_contract_address();
        }

        let crypto_ctx = try_s!(CryptoCtx::from_ctx(&ctx));
        let my_persistent_pub = {
            let my_persistent_pub: [u8; 33] = try_s!(crypto_ctx.mm2_internal_key_pair().public_slice().try_into());
            my_persistent_pub.into()
        };

        let mut maker = bits256::from([0; 32]);
        maker.bytes = data.maker_pubkey.0;
        let conf_settings = SwapConfirmationsSettings {
            maker_coin_confs: data.maker_payment_confirmations,
            maker_coin_nota: data
                .maker_payment_requires_nota
                .unwrap_or_else(|| maker_coin.requires_notarization()),
            taker_coin_confs: data.taker_payment_confirmations,
            taker_coin_nota: data
                .taker_payment_requires_nota
                .unwrap_or_else(|| taker_coin.requires_notarization()),
        };

        #[cfg(any(test, feature = "run-docker-tests"))]
        let fail_at = std::env::var("TAKER_FAIL_AT").map(FailAt::from).ok();

        let swap = TakerSwap::new(
            ctx.clone(),
            maker,
            data.maker_amount.clone().into(),
            data.taker_amount.clone().into(),
            my_persistent_pub,
            saved.uuid,
            Some(saved.uuid),
            conf_settings,
            maker_coin.clone(),
            taker_coin.clone(),
            data.lock_duration,
            data.p2p_privkey.map(SerializableSecp256k1Keypair::into_inner),
            #[cfg(any(test, feature = "run-docker-tests"))]
            fail_at,
        );

        for saved_event in &saved.events {
            swap.apply_event(saved_event.event.clone());
        }

        let mut command = match saved.events.last().unwrap().get_command() {
            Some(command) => command,
            None => return Ok((swap, None)),
        };

        if taker_coin.is_supported_by_watchers()
            && maker_coin.is_supported_by_watchers()
            && saved.watcher_message_sent()
        {
            // discover events occurred while taker was offline, due to maker or watcher activity
            command = get_command_based_on_maker_or_watcher_activity(&ctx, &swap, saved, command).await?;
        }
        drop_mutability!(command);

        Ok((swap, Some(command)))
    }

    pub async fn recover_funds(&self) -> Result<RecoveredSwap, String> {
        if self.finished_at.load(Ordering::Relaxed) == 0 {
            return ERR!("Swap must be finished before recover funds attempt");
        }

        if self.r().taker_payment_refund.is_some() {
            return ERR!("Taker payment is refunded, swap is not recoverable");
        }

        if self.r().maker_payment_spend.is_some() && self.r().maker_payment_spend_confirmed {
            return ERR!("Maker payment is spent and confirmed, swap is not recoverable");
        }

        let maker_payment = match &self.r().maker_payment {
            Some(tx) => tx.tx_hex.0.clone(),
            None => return ERR!("No info about maker payment, swap is not recoverable"),
        };

        // have to do this because std::sync::RwLockReadGuard returned by r() is not Send,
        // so it can't be used across await
        let other_maker_coin_htlc_pub = self.r().other_maker_coin_htlc_pub;
        let other_taker_coin_htlc_pub = self.r().other_taker_coin_htlc_pub;
        let secret_hash = self.r().secret_hash.0.clone();
        let maker_coin_start_block = self.r().data.maker_coin_start_block;
        let maker_coin_swap_contract_address = self.r().data.maker_coin_swap_contract_address.clone();

        let taker_payment_lock = self.r().data.taker_payment_lock;
        let taker_coin_start_block = self.r().data.taker_coin_start_block;
        let taker_coin_swap_contract_address = self.r().data.taker_coin_swap_contract_address.clone();
        let watcher_reward = self.r().watcher_reward;

        let unique_data = self.unique_swap_data();
        macro_rules! check_maker_payment_is_not_spent {
            // validate that maker payment is not spent
            () => {
                let search_input = SearchForSwapTxSpendInput {
                    time_lock: self.maker_payment_lock.load(Ordering::Relaxed),
                    other_pub: other_maker_coin_htlc_pub.as_slice(),
                    secret_hash: &secret_hash,
                    tx: &maker_payment,
                    search_from_block: maker_coin_start_block,
                    swap_contract_address: &maker_coin_swap_contract_address,
                    swap_unique_data: &unique_data,
                    watcher_reward,
                };

                match self.maker_coin.search_for_swap_tx_spend_other(search_input).await {
                    Ok(Some(FoundSwapTxSpend::Spent(tx))) => {
                        return ERR!(
                            "Maker payment was already spent by {} tx {:02x}",
                            self.maker_coin.ticker(),
                            tx.tx_hash_as_bytes()
                        )
                    },
                    Ok(Some(FoundSwapTxSpend::Refunded(tx))) => {
                        return ERR!(
                            "Maker payment was already refunded by {} tx {:02x}",
                            self.maker_coin.ticker(),
                            tx.tx_hash_as_bytes()
                        )
                    },
                    Err(e) => return ERR!("Error {} when trying to find maker payment spend", e),
                    Ok(None) => (), // payment is not spent, continue
                }
            };
        }

        let maybe_taker_payment = self.r().taker_payment.clone();
        let payment_instructions = self.r().payment_instructions.clone();

        let taker_payment = match maybe_taker_payment {
            Some(tx) => tx.tx_hex.0,
            None => {
                let maybe_sent = try_s!(
                    self.taker_coin
                        .check_if_my_payment_sent(CheckIfMyPaymentSentArgs {
                            time_lock: taker_payment_lock,
                            other_pub: other_taker_coin_htlc_pub.as_slice(),
                            secret_hash: &secret_hash,
                            search_from_block: taker_coin_start_block,
                            swap_contract_address: &taker_coin_swap_contract_address,
                            swap_unique_data: &unique_data,
                            amount: &self.taker_amount.to_decimal(),
                            payment_instructions: &payment_instructions,
                        })
                        .await
                );
                match maybe_sent {
                    Some(tx) => tx.tx_hex(),
                    None => return ERR!("Taker payment is not found, swap is not recoverable"),
                }
            },
        };

        if self.r().taker_payment_spend.is_some() {
            check_maker_payment_is_not_spent!();
            // has to do this because std::sync::RwLockReadGuard returned by r() is not Send,
            // so it can't be used across await
            let other_maker_coin_htlc_pub = self.r().other_maker_coin_htlc_pub;
            let secret = self.r().secret.0;
            let maker_coin_swap_contract_address = self.r().data.maker_coin_swap_contract_address.clone();
            let watcher_reward = self.r().watcher_reward;

            let maybe_spend_tx = self
                .maker_coin
                .send_taker_spends_maker_payment(SpendPaymentArgs {
                    other_payment_tx: &maker_payment,
                    time_lock: self.maker_payment_lock.load(Ordering::Relaxed),
                    other_pubkey: other_maker_coin_htlc_pub.as_slice(),
                    secret: &secret,
                    secret_hash: &secret_hash,
                    swap_contract_address: &maker_coin_swap_contract_address,
                    swap_unique_data: &unique_data,
                    watcher_reward,
                })
                .await;

            let transaction = match maybe_spend_tx {
                Ok(t) => t,
                Err(err) => {
                    if let Some(tx) = err.get_tx() {
                        broadcast_p2p_tx_msg(
                            &self.ctx,
                            tx_helper_topic(self.maker_coin.ticker()),
                            &tx,
                            &self.p2p_privkey,
                        );
                    }

                    return ERR!("{}", err.get_plain_text_format());
                },
            };

            return Ok(RecoveredSwap {
                action: RecoveredSwapAction::SpentOtherPayment,
                coin: self.maker_coin.ticker().to_string(),
                transaction,
            });
        }

        let search_input = SearchForSwapTxSpendInput {
            time_lock: taker_payment_lock,
            other_pub: other_taker_coin_htlc_pub.as_slice(),
            secret_hash: &secret_hash,
            tx: &taker_payment,
            search_from_block: taker_coin_start_block,
            swap_contract_address: &taker_coin_swap_contract_address,
            swap_unique_data: &unique_data,
            watcher_reward,
        };
        let taker_payment_spend = try_s!(self.taker_coin.search_for_swap_tx_spend_my(search_input).await);

        match taker_payment_spend {
            Some(spend) => match spend {
                FoundSwapTxSpend::Spent(tx) => {
                    check_maker_payment_is_not_spent!();
                    let secret_hash = self.r().secret_hash.clone();
                    let tx_hex = tx.tx_hex();
                    let secret = try_s!(
                        self.taker_coin
                            .extract_secret(&secret_hash.0, &tx_hex, watcher_reward)
                            .await
                    );

                    let taker_spends_payment_args = SpendPaymentArgs {
                        other_payment_tx: &maker_payment,
                        time_lock: self.maker_payment_lock.load(Ordering::Relaxed),
                        other_pubkey: other_maker_coin_htlc_pub.as_slice(),
                        secret: &secret,
                        secret_hash: &secret_hash,
                        swap_contract_address: &maker_coin_swap_contract_address,
                        swap_unique_data: &unique_data,
                        watcher_reward: self.r().watcher_reward,
                    };
                    let maybe_spend_tx = self
                        .maker_coin
                        .send_taker_spends_maker_payment(taker_spends_payment_args)
                        .await;

                    let transaction = match maybe_spend_tx {
                        Ok(t) => t,
                        Err(err) => {
                            if let Some(tx) = err.get_tx() {
                                broadcast_p2p_tx_msg(
                                    &self.ctx,
                                    tx_helper_topic(self.maker_coin.ticker()),
                                    &tx,
                                    &self.p2p_privkey,
                                );
                            }

                            return ERR!("{}", err.get_plain_text_format());
                        },
                    };

                    Ok(RecoveredSwap {
                        action: RecoveredSwapAction::SpentOtherPayment,
                        coin: self.maker_coin.ticker().to_string(),
                        transaction,
                    })
                },
                FoundSwapTxSpend::Refunded(tx) => ERR!(
                    "Taker payment has been refunded already by transaction {:02x}",
                    tx.tx_hash_as_bytes()
                ),
            },
            None => {
                if self.taker_coin.is_auto_refundable() {
                    return ERR!("Taker payment will be refunded automatically!");
                }

                let can_refund = try_s!(self.taker_coin.can_refund_htlc(taker_payment_lock).await);
                if let CanRefundHtlc::HaveToWait(seconds_to_wait) = can_refund {
                    return ERR!("Too early to refund, wait until {}", wait_until_sec(seconds_to_wait));
                }

                let fut = self.taker_coin.send_taker_refunds_payment(RefundPaymentArgs {
                    payment_tx: &taker_payment,
                    time_lock: taker_payment_lock,
                    other_pubkey: other_taker_coin_htlc_pub.as_slice(),
                    tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
                        maker_secret_hash: secret_hash.as_slice(),
                    },
                    swap_contract_address: &taker_coin_swap_contract_address,
                    swap_unique_data: &unique_data,
                    watcher_reward,
                });

                let transaction = match fut.await {
                    Ok(t) => t,
                    Err(err) => {
                        if let Some(tx) = err.get_tx() {
                            broadcast_p2p_tx_msg(
                                &self.ctx,
                                tx_helper_topic(self.taker_coin.ticker()),
                                &tx,
                                &self.p2p_privkey,
                            );
                        }

                        return ERR!("{:?}", err.get_plain_text_format());
                    },
                };

                Ok(RecoveredSwap {
                    action: RecoveredSwapAction::RefundedMyPayment,
                    coin: self.taker_coin.ticker().to_string(),
                    transaction,
                })
            },
        }
    }
}

impl AtomicSwap for TakerSwap {
    fn locked_amount(&self) -> Vec<LockedAmount> {
        let mut result = Vec::new();

        // if taker fee is not sent yet it must be virtually locked
        let taker_fee = DexFee::new_with_taker_pubkey(
            self.taker_coin.deref(),
            &self.r().data.maker_coin,
            &self.taker_amount,
            &self.my_taker_coin_htlc_pub().0,
        );
        let trade_fee = self.r().data.fee_to_send_taker_fee.clone().map(TradeFee::from);
        if self.r().taker_fee.is_none() {
            result.push(LockedAmount {
                coin: self.taker_coin.ticker().to_owned(),
                amount: taker_fee.total_spend_amount(),
                trade_fee,
            });
        }

        // if taker payment is not sent yet it must be virtually locked
        if self.r().taker_payment.is_none() {
            let trade_fee = self.r().data.taker_payment_trade_fee.clone().map(TradeFee::from);
            result.push(LockedAmount {
                coin: self.taker_coin.ticker().to_owned(),
                amount: self.taker_amount.clone(),
                trade_fee,
            });
        }

        // if maker payment is not spent yet the `MakerPaymentSpend` tx fee must be virtually locked
        if self.r().maker_payment_spend.is_none() {
            let trade_fee = self.r().data.maker_payment_spend_trade_fee.clone().map(TradeFee::from);
            result.push(LockedAmount {
                coin: self.maker_coin.ticker().to_owned(),
                amount: 0.into(),
                trade_fee,
            });
        }

        result
    }

    #[inline]
    fn uuid(&self) -> &Uuid {
        &self.uuid
    }

    #[inline]
    fn maker_coin(&self) -> &str {
        self.maker_coin.ticker()
    }

    #[inline]
    fn taker_coin(&self) -> &str {
        self.taker_coin.ticker()
    }

    #[inline]
    fn unique_swap_data(&self) -> Vec<u8> {
        // Taker generates swap UUID so it's safe for him to use it for privkey derivation
        self.uuid.as_bytes().to_vec()
    }
}

pub struct TakerSwapPreparedParams {
    pub(super) dex_fee: MmNumber,
    pub(super) fee_to_send_dex_fee: TradeFee,
    pub(super) taker_payment_trade_fee: TradeFee,
    pub(super) maker_payment_spend_trade_fee: TradeFee,
}

pub async fn check_balance_for_taker_swap(
    ctx: &MmArc,
    my_coin: &dyn MmCoin,
    other_coin: &dyn MmCoin,
    volume: MmNumber,
    swap_uuid: Option<&Uuid>,
    prepared_params: Option<TakerSwapPreparedParams>,
    stage: FeeApproxStage,
) -> CheckBalanceResult<()> {
    let fee_params = match prepared_params {
        Some(params) => params,
        None => create_taker_swap_default_params(my_coin, other_coin, volume.clone(), stage).await?,
    };

    let taker_fee = TakerFeeAdditionalInfo {
        dex_fee: fee_params.dex_fee,
        fee_to_send_dex_fee: fee_params.fee_to_send_dex_fee,
    };

    check_my_coin_balance_for_swap(
        ctx,
        my_coin,
        swap_uuid,
        volume,
        fee_params.taker_payment_trade_fee,
        Some(taker_fee),
    )
    .await?;
    if !fee_params.maker_payment_spend_trade_fee.paid_from_trading_vol {
        check_other_coin_balance_for_swap(ctx, other_coin, swap_uuid, fee_params.maker_payment_spend_trade_fee).await?;
    }
    Ok(())
}

pub struct TakerTradePreimage {
    /// The fee is paid per swap concerning the `base` coin.
    pub base_coin_fee: TradeFee,
    /// The fee is paid per swap concerning the `rel` coin.
    pub rel_coin_fee: TradeFee,
    /// The dex fee to be paid by taker coin.
    pub taker_fee: TradeFee,
    /// The miner fee is paid to send the dex fee.
    pub fee_to_send_taker_fee: TradeFee,
}

pub async fn taker_swap_trade_preimage(
    ctx: &MmArc,
    req: TradePreimageRequest,
    base_coin: MmCoinEnum,
    rel_coin: MmCoinEnum,
) -> TradePreimageRpcResult<TakerTradePreimage> {
    let action = req
        .swap_method
        .to_taker_action()
        .map_to_mm(TradePreimageRpcError::InternalError)?;
    let (my_coin, other_coin) = match action {
        TakerAction::Sell => (base_coin.clone(), rel_coin.clone()),
        TakerAction::Buy => (rel_coin.clone(), base_coin.clone()),
    };
    let my_coin_ticker = my_coin.ticker();
    let other_coin_ticker = other_coin.ticker();

    if req.max {
        return MmError::err(TradePreimageRpcError::InvalidParam {
            param: "max".to_owned(),
            reason: "'max' cannot be used with 'sell' or 'buy' method".to_owned(),
        });
    }

    let base_amount = req.volume.clone();
    let rel_amount = &req.price * &req.volume;

    let stage = FeeApproxStage::TradePreimage;
    let my_coin_volume = match action {
        TakerAction::Sell => base_amount.clone(),
        TakerAction::Buy => rel_amount.clone(),
    };

    let dex_fee = DexFee::new_with_taker_pubkey(
        my_coin.deref(),
        other_coin_ticker,
        &my_coin_volume,
        &my_coin.derive_htlc_pubkey(&[]), // passing empty unique_data because we need only the permanent pubkey here (not derived from the unique data)
    );
    let taker_fee = TradeFee {
        coin: my_coin_ticker.to_owned(),
        amount: dex_fee.total_spend_amount(),
        paid_from_trading_vol: false,
    };

    let fee_to_send_taker_fee = my_coin
        .get_fee_to_send_taker_fee(dex_fee.clone(), stage)
        .await
        .mm_err(|e| TradePreimageRpcError::from_trade_preimage_error(e, my_coin_ticker))?;

    let preimage_value = TradePreimageValue::Exact(my_coin_volume.to_decimal());
    let my_coin_trade_fee = my_coin
        .get_sender_trade_fee(preimage_value, stage)
        .await
        .mm_err(|e| TradePreimageRpcError::from_trade_preimage_error(e, my_coin_ticker))?;
    let other_coin_trade_fee = other_coin
        .get_receiver_trade_fee(stage)
        .compat()
        .await
        .mm_err(|e| TradePreimageRpcError::from_trade_preimage_error(e, other_coin_ticker))?;

    let prepared_params = TakerSwapPreparedParams {
        dex_fee: dex_fee.total_spend_amount(),
        fee_to_send_dex_fee: fee_to_send_taker_fee.clone(),
        taker_payment_trade_fee: my_coin_trade_fee.clone(),
        maker_payment_spend_trade_fee: other_coin_trade_fee.clone(),
    };
    check_balance_for_taker_swap(
        ctx,
        my_coin.deref(),
        other_coin.deref(),
        my_coin_volume.clone(),
        None,
        Some(prepared_params),
        stage,
    )
    .await
    .map_mm_err()?;

    let conf_settings = OrderConfirmationsSettings {
        base_confs: base_coin.required_confirmations(),
        base_nota: base_coin.requires_notarization(),
        rel_confs: rel_coin.required_confirmations(),
        rel_nota: rel_coin.requires_notarization(),
    };
    let our_public_id = CryptoCtx::from_ctx(ctx).map_mm_err()?.mm2_internal_public_id();
    let order_builder = TakerOrderBuilder::new(&base_coin, &rel_coin)
        .with_base_amount(base_amount)
        .with_rel_amount(rel_amount)
        .with_action(action.clone())
        .with_match_by(MatchBy::Any)
        .with_conf_settings(conf_settings)
        .with_sender_pubkey(H256Json::from(our_public_id.bytes));

    // perform an additional validation
    let _ = order_builder
        .build()
        .map_to_mm(|e| TradePreimageRpcError::from_taker_order_build_error(e, &req.base, &req.rel))?;

    let (base_coin_fee, rel_coin_fee) = match action {
        TakerAction::Sell => (my_coin_trade_fee, other_coin_trade_fee),
        TakerAction::Buy => (other_coin_trade_fee, my_coin_trade_fee),
    };
    Ok(TakerTradePreimage {
        base_coin_fee,
        rel_coin_fee,
        taker_fee,
        fee_to_send_taker_fee,
    })
}

#[derive(Deserialize)]
struct MaxTakerVolRequest {
    coin: String,
    trade_with: Option<String>,
}

pub async fn max_taker_vol(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let req: MaxTakerVolRequest = try_s!(json::from_value(req));
    let coin = match lp_coinfind(&ctx, &req.coin).await {
        Ok(Some(t)) => t,
        Ok(None) => return ERR!("No such coin: {}", req.coin),
        Err(err) => return ERR!("!lp_coinfind({}): {}", req.coin, err),
    };
    let other_coin = req.trade_with.as_ref().unwrap_or(&req.coin);
    let fut = calc_max_taker_vol(&ctx, &coin, other_coin, FeeApproxStage::TradePreimageMax);
    let max_vol = match fut.await {
        Ok(max_vol) => max_vol,
        Err(e) if e.get_inner().not_sufficient_balance() => {
            warn!("{}", e);
            MmNumber::from(0)
        },
        Err(err) => {
            return ERR!("{}", err);
        },
    };

    let res = try_s!(json::to_vec(&json!({
        "result": max_vol.to_fraction(),
        "coin": coin.ticker(),
    })));
    Ok(try_s!(Response::builder().body(res)))
}

/// If we want to calculate the maximum taker volume, we should solve the following equation:
///
/// ```rust
/// max_vol = balance - locked_amount
///         - trade_fee(max_vol)
///         - fee_to_send_taker_fee(dex_fee(max_vol))
///         - dex_fee(max_vol)
/// ```
///
/// 1. If the `trade_fee` and `fee_to_send_taker_fee` should be paid in base coin, the equation can be simplified:
///
/// ```rust
/// max_vol = balance - locked_amount - dex_fee(max_vol)
/// ```
///
/// where we can calculate the exact `max_vol` since the inverse of `dex_fee(x)` is obtainable.
///
/// 2. Otherwise we cannot express `max_vol` from the equation above, but we can find the smallest of the largest `max_vol`. That means if we find the largest `trade_fee` and `fee_to_send_taker_fee` values and plug them in:
///
/// ```rust
/// min_max_vol = balance - locked_amount
///             - max_trade_fee
///             - max_fee_to_send_taker_fee
///             - dex_fee(max_vol)
/// ```
///
/// then `min_max_vol` can be calculated as in the first case.
///
/// Please note that for any `x` and `y`, if `x < y` then
/// `trade_fee(x) <= trade_fee(y)`, `fee_to_send_taker_fee(x) <= fee_to_send_taker_fee(y)`,
/// and `dex_fee(x) <= dex_fee(y)`.
///
/// Let `real_max_vol` be the actual desired volume. Performing the following steps yields
/// an approximate maximum volume:
///
/// - `max_possible = balance - locked_amount`  
///   The largest possible max volume, replacing unknown fees with zero.
/// - `max_trade_fee = trade_fee(max_possible)`  
///   The largest possible `trade_fee`.
/// - `max_possible_2 = balance - locked_amount - max_trade_fee`  
///   A more accurate upper bound (`real_max_vol <= max_possible_2 <= max_possible`).
/// - `max_dex_fee = dex_fee(max_possible_2)`  
///   Passed into `fee_to_send_taker_fee`.
/// - `max_fee_to_send_taker_fee = fee_to_send_taker_fee(max_dex_fee)`
///
/// After that,  
/// ```rust
/// min_max_vol = balance - locked_amount
///             - max_trade_fee
///             - max_fee_to_send_taker_fee
///             - dex_fee(max_vol)
/// ```  
/// can be solved as in the first case.
///
pub async fn calc_max_taker_vol(
    ctx: &MmArc,
    coin: &MmCoinEnum,
    other_coin: &str,
    stage: FeeApproxStage,
) -> CheckBalanceResult<MmNumber> {
    let my_coin = coin.ticker();
    let balance: MmNumber = coin.my_spendable_balance().compat().await.map_mm_err()?.into();
    let locked = get_locked_amount(ctx, my_coin);
    let min_tx_amount = MmNumber::from(coin.min_tx_amount());

    let max_possible = &balance - &locked;
    let preimage_value = TradePreimageValue::UpperBound(max_possible.to_decimal());
    let max_trade_fee = coin
        .get_sender_trade_fee(preimage_value, stage)
        .await
        .mm_err(|e| CheckBalanceError::from_trade_preimage_error(e, my_coin))?;

    let max_vol = if my_coin == max_trade_fee.coin {
        // second case
        let max_possible_2 = &max_possible - &max_trade_fee.amount;
        let max_dex_fee = DexFee::new_from_taker_coin(coin.deref(), other_coin, &max_possible_2); // taker_pubkey is not known yet so we get max dex fee to calc max volume
        let max_fee_to_send_taker_fee = coin
            .get_fee_to_send_taker_fee(max_dex_fee.clone(), stage)
            .await
            .mm_err(|e| CheckBalanceError::from_trade_preimage_error(e, my_coin))?;
        let min_max_possible = &max_possible_2 - &max_fee_to_send_taker_fee.amount;

        debug!(
            "max_taker_vol case 2: min_max_possible {:?}, balance {:?}, locked {:?}, max_trade_fee {:?}, max_dex_fee {:?}, max_fee_to_send_taker_fee {:?}",
            min_max_possible.to_fraction(),
            balance.to_fraction(),
            locked.to_fraction(),
            max_trade_fee.amount.to_fraction(),
            max_dex_fee.total_spend_amount().to_fraction(),
            max_fee_to_send_taker_fee.amount.to_fraction()
        );
        max_taker_vol_from_available(min_max_possible, my_coin, other_coin, &min_tx_amount)
            .mm_err(|e| CheckBalanceError::from_max_taker_vol_error(e, my_coin.to_owned(), locked.to_decimal()))?
    } else {
        // first case
        debug!(
            "max_taker_vol case 1: balance {:?}, locked {:?}",
            balance.to_fraction(),
            locked.to_fraction()
        );
        max_taker_vol_from_available(max_possible, my_coin, other_coin, &min_tx_amount)
            .mm_err(|e| CheckBalanceError::from_max_taker_vol_error(e, my_coin.to_owned(), locked.to_decimal()))?
    };
    // do not check if `max_vol < min_tx_amount`, because it is checked within `max_taker_vol_from_available` already
    Ok(max_vol)
}

#[derive(Debug)]
pub struct MaxTakerVolumeLessThanDust {
    pub max_vol: MmNumber,
    pub min_tx_amount: MmNumber,
}

#[allow(clippy::result_large_err)]
pub fn max_taker_vol_from_available(
    available: MmNumber,
    base: &str,
    rel: &str,
    min_tx_amount: &MmNumber,
) -> Result<MmNumber, MmError<MaxTakerVolumeLessThanDust>> {
    let dex_fee_rate = DexFee::dex_fee_rate(base, rel);
    let threshold_coef = &(&MmNumber::from(1) + &dex_fee_rate) / &dex_fee_rate;
    let max_vol = if available > min_tx_amount * &threshold_coef {
        available / (MmNumber::from(1) + dex_fee_rate)
    } else {
        &available - min_tx_amount
    };

    if &max_vol <= min_tx_amount {
        return MmError::err(MaxTakerVolumeLessThanDust {
            max_vol,
            min_tx_amount: min_tx_amount.clone(),
        });
    }
    Ok(max_vol)
}

/// Get dex fee and trade fee, including fee to spend maker coin (if requested)
pub async fn create_taker_swap_default_params(
    my_coin: &dyn MmCoin,
    other_coin: &dyn MmCoin,
    volume: MmNumber,
    stage: FeeApproxStage,
) -> CheckBalanceResult<TakerSwapPreparedParams> {
    let dex_fee = DexFee::new_from_taker_coin(my_coin, other_coin.ticker(), &volume); // taker_pubkey is not known yet so we get max dexfee to estimate max swap amount
    let fee_to_send_dex_fee = my_coin
        .get_fee_to_send_taker_fee(dex_fee.clone(), stage)
        .await
        .mm_err(|e| CheckBalanceError::from_trade_preimage_error(e, my_coin.ticker()))?;
    let preimage_value = TradePreimageValue::Exact(volume.to_decimal());
    let taker_payment_trade_fee = my_coin
        .get_sender_trade_fee(preimage_value, stage)
        .await
        .mm_err(|e| CheckBalanceError::from_trade_preimage_error(e, my_coin.ticker()))?;
    let maker_payment_spend_trade_fee = other_coin
        .get_receiver_trade_fee(stage)
        .compat()
        .await
        .mm_err(|e| CheckBalanceError::from_trade_preimage_error(e, other_coin.ticker()))?;
    Ok(TakerSwapPreparedParams {
        dex_fee: dex_fee.total_spend_amount(),
        fee_to_send_dex_fee,
        taker_payment_trade_fee,
        maker_payment_spend_trade_fee,
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod taker_swap_tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;
    use crate::lp_swap::get_locked_amount_by_other_swaps;
    use coins::eth::{addr_from_str, signed_eth_tx_from_bytes, SignedEthTx};
    use coins::utxo::UtxoTx;
    use coins::{FoundSwapTxSpend, MarketCoinOps, MmCoin, SwapOps, TestCoin};
    use common::{block_on, new_uuid};
    use mm2_test_helpers::for_tests::{mm_ctx_with_iguana, ETH_SEPOLIA_SWAP_CONTRACT};
    use mocktopus::mocking::*;

    const PASSPHRASE: Option<&str> =
        Some("spice describe gravity federal blast come thank unfair canal monkey style afraid");

    fn eth_tx_for_test() -> SignedEthTx {
        // raw transaction bytes of https://etherscan.io/tx/0x0869be3e5d4456a29d488a533ad6c118620fef450f36778aecf31d356ff8b41f
        let tx_bytes = [
            248, 240, 3, 133, 1, 42, 5, 242, 0, 131, 2, 73, 240, 148, 133, 0, 175, 192, 188, 82, 20, 114, 128, 130, 22,
            51, 38, 194, 255, 12, 115, 244, 168, 113, 135, 110, 205, 245, 24, 127, 34, 254, 184, 132, 21, 44, 243, 175,
            73, 33, 143, 82, 117, 16, 110, 27, 133, 82, 200, 114, 233, 42, 140, 198, 35, 21, 201, 249, 187, 180, 20,
            46, 148, 40, 9, 228, 193, 130, 71, 199, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 152, 41, 132, 9, 201, 73, 19,
            94, 237, 137, 35, 61, 4, 194, 207, 239, 152, 75, 175, 245, 157, 174, 10, 214, 161, 207, 67, 70, 87, 246,
            231, 212, 47, 216, 119, 68, 237, 197, 125, 141, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 93, 72, 125, 102, 28, 159, 180, 237, 198, 97,
            87, 80, 82, 200, 104, 40, 245, 221, 7, 28, 122, 104, 91, 99, 1, 159, 140, 25, 131, 101, 74, 87, 50, 168,
            146, 187, 90, 160, 51, 1, 123, 247, 6, 108, 165, 181, 188, 40, 56, 47, 211, 229, 221, 73, 5, 15, 89, 81,
            117, 225, 216, 108, 98, 226, 119, 232, 94, 184, 42, 106,
        ];
        signed_eth_tx_from_bytes(&tx_bytes).unwrap()
    }

    #[test]
    fn test_recover_funds_taker_swap_maker_payment_spend_errored() {
        let ctx = mm_ctx_with_iguana(PASSPHRASE);

        let taker_saved_json = r#"{"error_events":["StartFailed","NegotiateFailed","TakerFeeSendFailed","MakerPaymentValidateFailed","TakerPaymentTransactionFailed","TakerPaymentDataSendFailed","TakerPaymentWaitForSpendFailed","MakerPaymentSpendFailed","MakerPaymentSpendConfirmFailed","TakerPaymentRefunded","TakerPaymentRefundedByWatcher","TakerPaymentRefundFailed"],"events":[{"event":{"data":{"lock_duration":7800,"maker":"1bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","maker_amount":"0.12596566232185483","maker_coin":"KMD","maker_coin_start_block":1458035,"maker_payment_confirmations":1,"maker_payment_wait":1564053079,"my_persistent_pub":"0326846707a52a233cfc49a61ef51b1698bbe6aa78fa8b8d411c02743c09688f0a","started_at":1564050479,"taker_amount":"50.000000000000001504212457800000","taker_coin":"DOGE","taker_coin_start_block":2823448,"taker_payment_confirmations":1,"taker_payment_lock":1564058279,"uuid":"41383f43-46a5-478c-9386-3b2cce0aca20"},"type":"Started"},"timestamp":1564050480269},{"event":{"data":{"maker_payment_locktime":1564066080,"maker_pubkey":"031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","secret_hash":"3669eb83a007a3c507448d79f45a9f06ec2f36a8"},"type":"Negotiated"},"timestamp":1564050540991},{"event":{"data":{"tx_hash":"bdde828b492d6d1cc25cd2322fd592dafd722fcc7d8b0fedce4d3bb4a1a8c8ff","tx_hex":"0100000002c7efa995c8b7be0a8b6c2d526c6c444c1634d65584e9ee89904e9d8675eac88c010000006a473044022051f34d5e3b7d0b9098d5e35333f3550f9cb9e57df83d5e4635b7a8d2986d6d5602200288c98da05de6950e01229a637110a1800ba643e75cfec59d4eb1021ad9b40801210326846707a52a233cfc49a61ef51b1698bbe6aa78fa8b8d411c02743c09688f0affffffffae6c233989efa7c7d2aa6534adc96078917ff395b7f09f734a147b2f44ade164000000006a4730440220393a784c2da74d0e2a28ec4f7df6c8f9d8b2af6ae6957f1e68346d744223a8fd02201b7a96954ac06815a43a6c7668d829ae9cbb5de76fa77189ddfd9e3038df662c01210326846707a52a233cfc49a61ef51b1698bbe6aa78fa8b8d411c02743c09688f0affffffff02115f5800000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ac41a84641020000001976a914444f0e1099709ba4d742454a7d98a5c9c162ceab88ac6d84395d"},"type":"TakerFeeSent"},"timestamp":1564050545296},{"event":{"data":{"tx_hash":"0a0f11fa82802c2c30862c50ab2162185dae8de7f7235f32c506f814c142b382","tx_hex":"0400008085202f8902ace337db2dd4c56b0697f58fb8cfb6bd1cd6f469d925fc0376d1dcfb7581bf82000000006b483045022100d1f95be235c5c8880f5d703ace287e2768548792c58c5dbd27f5578881b30ea70220030596106e21c7e0057ee0dab283f9a1fe273f15208cba80870c447bd559ef0d0121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffff9f339752567c404427fd77f2b35cecdb4c21489edc64e25e729fdb281785e423000000006a47304402203179e95877dbc107123a417f1e648e3ff13d384890f1e4a67b6dd5087235152e0220102a8ab799fadb26b5d89ceb9c7bc721a7e0c2a0d0d7e46bbe0cf3d130010d430121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffff025635c0000000000017a91480a95d366d65e34a465ab17b0c9eb1d5a33bae08876cbfce05000000001976a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac8d7c395d000000000000000000000000000000"},"type":"MakerPaymentReceived"},"timestamp":1564050588176},{"event":{"type":"MakerPaymentWaitConfirmStarted"},"timestamp":1564050588178},{"event":{"type":"MakerPaymentValidatedAndConfirmed"},"timestamp":1564050693585},{"event":{"data":{"tx_hash":"539cb6dbdc25465bbccc575554f05d1bb04c70efce4316e41194e747375c3659","tx_hex":"0100000001ffc8a8a1b43b4dceed0f8b7dcc2f72fdda92d52f32d25cc21c6d2d498b82debd010000006a47304402203967b7f9f5532fa47116585c7d1bcba51861ea2059cca00409f34660db18e33a0220640991911852533a12fdfeb039fb9c8ca2c45482c6993bd84636af3670d49c1501210326846707a52a233cfc49a61ef51b1698bbe6aa78fa8b8d411c02743c09688f0affffffff0200f2052a0100000017a914f2fa08ae416b576779ae5da975e5442663215fce87415173f9000000001976a914444f0e1099709ba4d742454a7d98a5c9c162ceab88ac0585395d"},"type":"TakerPaymentSent"},"timestamp":1564050695611},{"event":{"data":{"secret":"1b8886b8a2cdb62505699400b694ac20f04d7bd4abd80e1ab154aa8d861fc093","transaction":{"tx_hash":"cc5af1cf68d246419fee49c3d74c0cd173599d115b86efe274368a614951bc47","tx_hex":"010000000159365c3747e79411e41643ceef704cb01b5df0545557ccbc5b4625dcdbb69c5300000000d747304402200e78e27d2f1c18676f98ca3dfa4e4a9eeaa8209b55f57b4dd5d9e1abdf034cfa0220623b5c22b62234cec230342aa306c497e43494b44ec2425b84e236b1bf01257001201b8886b8a2cdb62505699400b694ac20f04d7bd4abd80e1ab154aa8d861fc093004c6b6304a7a2395db175210326846707a52a233cfc49a61ef51b1698bbe6aa78fa8b8d411c02743c09688f0aac6782012088a9143669eb83a007a3c507448d79f45a9f06ec2f36a88821031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ac68ffffffff01008d380c010000001976a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac8c77395d"}},"type":"TakerPaymentSpent"},"timestamp":1564051092890},{"event":{"data":{"error":"lp_swap:1981] utxo:891] rpc_clients:738] JsonRpcError { request: JsonRpcRequest { jsonrpc: \"2.0\", id: \"67\", method: \"blockchain.transaction.broadcast\", params: [String(\"0400008085202f890182b342c114f806c5325f23f7e78dae5d186221ab502c86302c2c8082fa110f0a00000000d7473044022035791ea5548f87484065c9e1f0bdca9ebc699f2c7f51182c84f360102e32dc3d02200612ed53bca52d9c2568437f087598531534badf26229fe0f652ea72ddf03ca501201b8886b8a2cdb62505699400b694ac20f04d7bd4abd80e1ab154aa8d861fc093004c6b630420c1395db17521031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ac6782012088a9143669eb83a007a3c507448d79f45a9f06ec2f36a888210326846707a52a233cfc49a61ef51b1698bbe6aa78fa8b8d411c02743c09688f0aac68ffffffff01460ec000000000001976a914444f0e1099709ba4d742454a7d98a5c9c162ceab88ac967e395d000000000000000000000000000000\")] }, error: Transport(\"rpc_clients:668] All electrums are currently disconnected\") }"},"type":"MakerPaymentSpendFailed"},"timestamp":1564051092897},{"event":{"type":"Finished"},"timestamp":1564051092900}],"success_events":["Started","Negotiated","TakerFeeSent","MakerPaymentReceived","MakerPaymentWaitConfirmStarted","MakerPaymentValidatedAndConfirmed","TakerPaymentSent","TakerPaymentSpent","MakerPaymentSpent","MakerPaymentSpendConfirmed","Finished"],"uuid":"41383f43-46a5-478c-9386-3b2cce0aca20"}"#;
        let taker_saved_swap: TakerSavedSwap = json::from_str(taker_saved_json).unwrap();

        TestCoin::ticker.mock_safe(|_| MockResult::Return("ticker"));
        TestCoin::swap_contract_address.mock_safe(|_| MockResult::Return(None));

        static MAKER_PAYMENT_SPEND_CALLED: AtomicBool = AtomicBool::new(false);
        TestCoin::send_taker_spends_maker_payment.mock_safe(|_, _| {
            MAKER_PAYMENT_SPEND_CALLED.store(true, Ordering::Relaxed);
            MockResult::Return(Box::pin(futures::future::ready(Ok(eth_tx_for_test().into()))))
        });
        TestCoin::search_for_swap_tx_spend_other
            .mock_safe(|_, _| MockResult::Return(Box::pin(futures::future::ready(Ok(None)))));
        let maker_coin = MmCoinEnum::Test(TestCoin::default());
        let taker_coin = MmCoinEnum::Test(TestCoin::default());
        let (taker_swap, _) = block_on(TakerSwap::load_from_saved(
            ctx,
            maker_coin,
            taker_coin,
            taker_saved_swap,
        ))
        .unwrap();
        let actual = block_on(taker_swap.recover_funds()).unwrap();
        let expected = RecoveredSwap {
            action: RecoveredSwapAction::SpentOtherPayment,
            coin: "ticker".to_string(),
            transaction: eth_tx_for_test().into(),
        };
        assert_eq!(expected, actual);
        assert!(MAKER_PAYMENT_SPEND_CALLED.load(Ordering::Relaxed));
    }

    #[test]
    fn test_recover_funds_taker_swap_taker_payment_errored_but_sent_not_spent() {
        let ctx = mm_ctx_with_iguana(PASSPHRASE);

        let taker_saved_json = r#"{"error_events":["StartFailed","NegotiateFailed","TakerFeeSendFailed","MakerPaymentValidateFailed","TakerPaymentTransactionFailed","TakerPaymentDataSendFailed","TakerPaymentWaitForSpendFailed","MakerPaymentSpendFailed","MakerPaymentSpendConfirmFailed","TakerPaymentRefunded","TakerPaymentRefundedByWatcher","TakerPaymentRefundFailed"],"events":[{"event":{"data":{"lock_duration":7800,"maker":"1bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","maker_amount":"3.54932734","maker_coin":"KMD","maker_coin_start_block":1452970,"maker_payment_confirmations":1,"maker_payment_wait":1563746537,"my_persistent_pub":"03101ace6b08605b9424b0582b5cce044b70a3c8d8d10cb2965e039b0967ae92b9","started_at":1563743937,"taker_amount":"0.02004833998671660000000000","taker_coin":"ETH","taker_coin_start_block":8196380,"taker_payment_confirmations":1,"taker_payment_lock":1563751737,"uuid":"3447b727-fe93-4357-8e5a-8cf2699b7e86"},"type":"Started"},"timestamp":1563743937741},{"event":{"data":{"maker_payment_locktime":1563759539,"maker_pubkey":"031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","secret_hash":"432c8272ac59b47dea2d299b5cf1ee64ea1917b9"},"type":"Negotiated"},"timestamp":1563744003530},{"event":{"data":{"tx_hash":"a59203eb2328827de00bed699a29389792906e4f39fdea145eb40dc6b3821bd6","tx_hex":"f8690284ee6b280082520894d8997941dd1346e9231118d5685d866294f59e5b865af3107a4000801ca0743d2b7c9fad65805d882179062012261be328d7628ae12ee08eff8d7657d993a07eecbd051f49d35279416778faa4664962726d516ce65e18755c9b9406a9c2fd"},"type":"TakerFeeSent"},"timestamp":1563744020598},{"event":{"data":{"tx_hash":"0cf4acbcefde53645851c5c6053ea61fe0cbb5f828a906d69eb809e0b071a03b","tx_hex":"0400008085202f89025d5ae3e8c87418c9b735f8f2f7d29e26820c33c9f30d53f2d31f8b99ea9b1490010000006a47304402201185c06ca575261c539b287175751b7de642eb7466c59128639a19b4c2dd2f9b02201c8c4167d581864bedd4d1deb5596472e6e3ce29fe9e7996907a7b59c905d5490121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffff06dbf9971c8dfd4a0c8c49f4f15c51de59ba13b2efa702682e26869843af9a87000000006a473044022012b47c12c7f6ad7d8b778fc4b5dcfd56a39325daf302f56e7b84753ba5216cfa022076bf571cf9e20facf70d2f134e8ed2de67aa08581a27ff3128bf93a9b594ac770121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffff02fed727150000000017a914d5268b31131a652f9b6ddf57db62f02285cdfad1874e1d7835000000001976a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac37cf345d000000000000000000000000000000"},"type":"MakerPaymentReceived"},"timestamp":1563744071778},{"event":{"type":"MakerPaymentWaitConfirmStarted"},"timestamp":1563744071781},{"event":{"type":"MakerPaymentValidatedAndConfirmed"},"timestamp":1563744118073},{"event":{"data":{"error":"lp_swap:1888] eth:654] RPC error: Error { code: ServerError(-32010), message: \"Transaction with the same hash was already imported.\", data: None }"},"type":"TakerPaymentTransactionFailed"},"timestamp":1563744118577},{"event":{"type":"Finished"},"timestamp":1563744118580}],"success_events":["Started","Negotiated","TakerFeeSent","MakerPaymentReceived","MakerPaymentWaitConfirmStarted","MakerPaymentValidatedAndConfirmed","TakerPaymentSent","TakerPaymentSpent","MakerPaymentSpent","MakerPaymentSpendConfirmed","Finished"],"uuid":"3447b727-fe93-4357-8e5a-8cf2699b7e86"}"#;
        let taker_saved_swap: TakerSavedSwap = json::from_str(taker_saved_json).unwrap();

        TestCoin::ticker.mock_safe(|_| MockResult::Return("ticker"));
        TestCoin::swap_contract_address.mock_safe(|_| MockResult::Return(None));
        TestCoin::can_refund_htlc
            .mock_safe(|_, _| MockResult::Return(Box::pin(futures::future::ok(CanRefundHtlc::CanRefundNow))));

        static MY_PAYMENT_SENT_CALLED: AtomicBool = AtomicBool::new(false);
        TestCoin::check_if_my_payment_sent.mock_safe(|_, _| {
            MY_PAYMENT_SENT_CALLED.store(true, Ordering::Relaxed);
            MockResult::Return(Box::pin(futures::future::ok(Some(eth_tx_for_test().into()))))
        });

        static TX_SPEND_CALLED: AtomicBool = AtomicBool::new(false);
        TestCoin::search_for_swap_tx_spend_my.mock_safe(|_, _| {
            TX_SPEND_CALLED.store(true, Ordering::Relaxed);
            MockResult::Return(Box::pin(futures::future::ready(Ok(None))))
        });

        static TAKER_PAYMENT_REFUND_CALLED: AtomicBool = AtomicBool::new(false);
        TestCoin::send_taker_refunds_payment.mock_safe(|_, _| {
            TAKER_PAYMENT_REFUND_CALLED.store(true, Ordering::Relaxed);
            MockResult::Return(Box::pin(futures::future::ok(eth_tx_for_test().into())))
        });
        let maker_coin = MmCoinEnum::Test(TestCoin::default());
        let taker_coin = MmCoinEnum::Test(TestCoin::default());
        let (taker_swap, _) = block_on(TakerSwap::load_from_saved(
            ctx,
            maker_coin,
            taker_coin,
            taker_saved_swap,
        ))
        .unwrap();
        let actual = block_on(taker_swap.recover_funds()).unwrap();
        let expected = RecoveredSwap {
            action: RecoveredSwapAction::RefundedMyPayment,
            coin: "ticker".to_string(),
            transaction: eth_tx_for_test().into(),
        };
        assert_eq!(expected, actual);
        assert!(MY_PAYMENT_SENT_CALLED.load(Ordering::Relaxed));
        assert!(TX_SPEND_CALLED.load(Ordering::Relaxed));
        assert!(TAKER_PAYMENT_REFUND_CALLED.load(Ordering::Relaxed));
    }

    #[test]
    fn test_recover_funds_taker_swap_taker_payment_errored_but_sent_and_spent_by_maker() {
        let ctx = mm_ctx_with_iguana(PASSPHRASE);

        let taker_saved_json = r#"{"error_events":["StartFailed","NegotiateFailed","TakerFeeSendFailed","MakerPaymentValidateFailed","TakerPaymentTransactionFailed","TakerPaymentDataSendFailed","TakerPaymentWaitForSpendFailed","MakerPaymentSpendFailed","TakerPaymentRefunded","TakerPaymentRefundedByWatcher","TakerPaymentRefundFailed"],"events":[{"event":{"data":{"lock_duration":7800,"maker":"1bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","maker_amount":"3.54932734","maker_coin":"KMD","maker_coin_start_block":1452970,"maker_payment_confirmations":1,"maker_payment_wait":1563746537,"my_persistent_pub":"03101ace6b08605b9424b0582b5cce044b70a3c8d8d10cb2965e039b0967ae92b9","started_at":1563743937,"taker_amount":"0.02004833998671660000000000","taker_coin":"ETH","taker_coin_start_block":8196380,"taker_payment_confirmations":1,"taker_payment_lock":1563751737,"uuid":"3447b727-fe93-4357-8e5a-8cf2699b7e86"},"type":"Started"},"timestamp":1563743937741},{"event":{"data":{"maker_payment_locktime":1563759539,"maker_pubkey":"031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","secret_hash":"432c8272ac59b47dea2d299b5cf1ee64ea1917b9"},"type":"Negotiated"},"timestamp":1563744003530},{"event":{"data":{"tx_hash":"a59203eb2328827de00bed699a29389792906e4f39fdea145eb40dc6b3821bd6","tx_hex":"f8690284ee6b280082520894d8997941dd1346e9231118d5685d866294f59e5b865af3107a4000801ca0743d2b7c9fad65805d882179062012261be328d7628ae12ee08eff8d7657d993a07eecbd051f49d35279416778faa4664962726d516ce65e18755c9b9406a9c2fd"},"type":"TakerFeeSent"},"timestamp":1563744020598},{"event":{"data":{"tx_hash":"0cf4acbcefde53645851c5c6053ea61fe0cbb5f828a906d69eb809e0b071a03b","tx_hex":"0400008085202f89025d5ae3e8c87418c9b735f8f2f7d29e26820c33c9f30d53f2d31f8b99ea9b1490010000006a47304402201185c06ca575261c539b287175751b7de642eb7466c59128639a19b4c2dd2f9b02201c8c4167d581864bedd4d1deb5596472e6e3ce29fe9e7996907a7b59c905d5490121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffff06dbf9971c8dfd4a0c8c49f4f15c51de59ba13b2efa702682e26869843af9a87000000006a473044022012b47c12c7f6ad7d8b778fc4b5dcfd56a39325daf302f56e7b84753ba5216cfa022076bf571cf9e20facf70d2f134e8ed2de67aa08581a27ff3128bf93a9b594ac770121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffff02fed727150000000017a914d5268b31131a652f9b6ddf57db62f02285cdfad1874e1d7835000000001976a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac37cf345d000000000000000000000000000000"},"type":"MakerPaymentReceived"},"timestamp":1563744071778},{"event":{"type":"MakerPaymentWaitConfirmStarted"},"timestamp":1563744071781},{"event":{"type":"MakerPaymentValidatedAndConfirmed"},"timestamp":1563744118073},{"event":{"data":{"error":"lp_swap:1888] eth:654] RPC error: Error { code: ServerError(-32010), message: \"Transaction with the same hash was already imported.\", data: None }"},"type":"TakerPaymentTransactionFailed"},"timestamp":1563744118577},{"event":{"type":"Finished"},"timestamp":1563744118580}],"success_events":["Started","Negotiated","TakerFeeSent","MakerPaymentReceived","MakerPaymentWaitConfirmStarted","MakerPaymentValidatedAndConfirmed","TakerPaymentSent","TakerPaymentSpent","MakerPaymentSpent","MakerPaymentSpendConfirmed","Finished"],"uuid":"3447b727-fe93-4357-8e5a-8cf2699b7e86"}"#;
        let taker_saved_swap: TakerSavedSwap = json::from_str(taker_saved_json).unwrap();

        TestCoin::ticker.mock_safe(|_| MockResult::Return("ticker"));
        TestCoin::swap_contract_address.mock_safe(|_| MockResult::Return(None));
        TestCoin::extract_secret.mock_safe(|_, _, _, _| MockResult::Return(Box::pin(async move { Ok([0; 32]) })));

        static MY_PAYMENT_SENT_CALLED: AtomicBool = AtomicBool::new(false);
        TestCoin::check_if_my_payment_sent.mock_safe(|_, _| {
            MY_PAYMENT_SENT_CALLED.store(true, Ordering::Relaxed);
            MockResult::Return(Box::pin(futures::future::ok(Some(eth_tx_for_test().into()))))
        });

        static SEARCH_TX_SPEND_CALLED: AtomicBool = AtomicBool::new(false);
        TestCoin::search_for_swap_tx_spend_my.mock_safe(|_, _| {
            SEARCH_TX_SPEND_CALLED.store(true, Ordering::Relaxed);
            let tx: UtxoTx = "0100000001de7aa8d29524906b2b54ee2e0281f3607f75662cbc9080df81d1047b78e21dbc00000000d7473044022079b6c50820040b1fbbe9251ced32ab334d33830f6f8d0bf0a40c7f1336b67d5b0220142ccf723ddabb34e542ed65c395abc1fbf5b6c3e730396f15d25c49b668a1a401209da937e5609680cb30bff4a7661364ca1d1851c2506fa80c443f00a3d3bf7365004c6b6304f62b0e5cb175210270e75970bb20029b3879ec76c4acd320a8d0589e003636264d01a7d566504bfbac6782012088a9142fb610d856c19fd57f2d0cffe8dff689074b3d8a882103f368228456c940ac113e53dad5c104cf209f2f102a409207269383b6ab9b03deac68ffffffff01d0dc9800000000001976a9146d9d2b554d768232320587df75c4338ecc8bf37d88ac40280e5c".into();
            MockResult::Return(Box::pin(futures::future::ready(Ok(Some(FoundSwapTxSpend::Spent(tx.into()))))))
        });

        TestCoin::search_for_swap_tx_spend_other
            .mock_safe(|_, _| MockResult::Return(Box::pin(futures::future::ready(Ok(None)))));

        static MAKER_PAYMENT_SPEND_CALLED: AtomicBool = AtomicBool::new(false);
        TestCoin::send_taker_spends_maker_payment.mock_safe(|_, _| {
            MAKER_PAYMENT_SPEND_CALLED.store(true, Ordering::Relaxed);
            MockResult::Return(Box::pin(futures::future::ready(Ok(eth_tx_for_test().into()))))
        });
        let maker_coin = MmCoinEnum::Test(TestCoin::default());
        let taker_coin = MmCoinEnum::Test(TestCoin::default());
        let (taker_swap, _) = block_on(TakerSwap::load_from_saved(
            ctx,
            maker_coin,
            taker_coin,
            taker_saved_swap,
        ))
        .unwrap();
        let actual = block_on(taker_swap.recover_funds()).unwrap();
        let expected = RecoveredSwap {
            action: RecoveredSwapAction::SpentOtherPayment,
            coin: "ticker".to_string(),
            transaction: eth_tx_for_test().into(),
        };
        assert_eq!(expected, actual);
        assert!(MY_PAYMENT_SENT_CALLED.load(Ordering::Relaxed));
        assert!(SEARCH_TX_SPEND_CALLED.load(Ordering::Relaxed));
        assert!(MAKER_PAYMENT_SPEND_CALLED.load(Ordering::Relaxed));
    }

    #[test]
    fn test_recover_funds_taker_swap_taker_payment_refund_failed_not_spent() {
        let ctx = mm_ctx_with_iguana(PASSPHRASE);

        let taker_saved_json = r#"{"error_events":["StartFailed","NegotiateFailed","TakerFeeSendFailed","MakerPaymentValidateFailed","TakerPaymentTransactionFailed","TakerPaymentDataSendFailed","TakerPaymentWaitForSpendFailed","MakerPaymentSpendFailed","MakerPaymentSpendConfirmFailed","TakerPaymentRefunded","TakerPaymentRefundedByWatcher","TakerPaymentRefundFailed"],"events":[{"event":{"data":{"lock_duration":7800,"maker":"1bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","maker_amount":"0.58610590","maker_coin":"KMD","maker_coin_start_block":1450923,"maker_payment_confirmations":1,"maker_payment_wait":1563623475,"my_persistent_pub":"02713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91","started_at":1563620875,"taker_amount":"0.0077700000552410000000000","taker_coin":"LTC","taker_coin_start_block":1670837,"taker_payment_confirmations":1,"taker_payment_lock":1563628675,"uuid":"9db641f5-4300-4527-9fa6-f1c391d42c35"},"type":"Started"},"timestamp":1563620875766},{"event":{"data":{"maker_payment_locktime":1563636475,"maker_pubkey":"031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","secret_hash":"7ed38daab6085c1a1e4426e61dc87a3c2c081a95"},"type":"Negotiated"},"timestamp":1563620955014},{"event":{"data":{"tx_hash":"6740136eaaa615d9d231969e3a9599d0fc59e53989237a8d31cd6fc86c160013","tx_hex":"0100000001a2586ea8294cedc55741bef625ba72c646399903391a7f6c604a58c6263135f2000000006b4830450221009c78c8ba4a7accab6b09f9a95da5bc59c81f4fc1e60b288ec3c5462b4d02ef01022056b63be1629cf17751d3cc5ffec51bcb1d7f9396e9ce9ca254d0f34104f7263a012102713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91ffffffff0210270000000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ac78aa1900000000001976a91406ccabfd5f9075ecd5e8d0d31c0e973a54d51e8288ac5bf6325d"},"type":"TakerFeeSent"},"timestamp":1563620958220},{"event":{"data":{"tx_hash":"d0f6e664cea9d89fe7b5cf8005fdca070d1ab1d05a482aaef95c08cdaecddf0a","tx_hex":"0400008085202f89019f1cbda354342cdf982046b331bbd3791f53b692efc6e4becc36be495b2977d9000000006b483045022100fa9d4557394141f6a8b9bfb8cd594a521fd8bcd1965dbf8bc4e04abc849ac66e0220589f521814c10a7561abfd5e432f7a2ee60d4875fe4604618af3207dae531ac00121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffff029e537e030000000017a9145534898009f1467191065f6890b96914b39a1c018791857702000000001976a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac72ee325d000000000000000000000000000000"},"type":"MakerPaymentReceived"},"timestamp":1563620999307},{"event":{"type":"MakerPaymentWaitConfirmStarted"},"timestamp":1563620999310},{"event":{"type":"MakerPaymentValidatedAndConfirmed"},"timestamp":1563621244153},{"event":{"data":{"tx_hash":"1e883eb2f3991e84ba27f53651f89b7dda708678a5b9813d043577f222b9ca30","tx_hex":"01000000011300166cc86fcd318d7a238939e559fcd099953a9e9631d2d915a6aa6e134067010000006a47304402206781d5f2db2ff13d2ec7e266f774ea5630cc2dba4019e18e9716131b8b026051022006ebb33857b6d180f13aa6be2fc532f9734abde9d00ae14757e7d7ba3741c08c012102713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91ffffffff0228db0b000000000017a91483818667161bf94adda3964a81a231cbf6f5338187b0480c00000000001976a91406ccabfd5f9075ecd5e8d0d31c0e973a54d51e8288ac7cf7325d"},"type":"TakerPaymentSent"},"timestamp":1563621246370},{"event":{"data":{"error":"utxo:1145] rpc_clients:782] Waited too long until 1563628675 for output TransactionOutput { value: 777000, script_pubkey: a91483818667161bf94adda3964a81a231cbf6f5338187 } to be spent "},"type":"TakerPaymentWaitForSpendFailed"},"timestamp":1563638060370},{"event":{"data":{"error":"lp_swap:2025] utxo:938] rpc_clients:719] JsonRpcError { request: JsonRpcRequest { jsonrpc: \"2.0\", id: \"9\", method: \"blockchain.transaction.broadcast\", params: [String(\"010000000130cab922f27735043d81b9a5788670da7d9bf85136f527ba841e99f3b23e881e00000000b6473044022058a0c1da6bcf8c1418899ff8475f3ab6dddbff918528451c1fe71c2f7dad176302204c2e0bcf8f9b5f09e02ccfeb9256e9b34fb355ea655a5704a8a3fa920079b91501514c6b63048314335db1752102713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91ac6782012088a9147ed38daab6085c1a1e4426e61dc87a3c2c081a958821031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ac68feffffff0188540a00000000001976a91406ccabfd5f9075ecd5e8d0d31c0e973a54d51e8288ac1c2b335d\")] }, error: Response(Object({\"code\": Number(1), \"message\": String(\"the transaction was rejected by network rules.\\n\\nMissing inputs\\n[010000000130cab922f27735043d81b9a5788670da7d9bf85136f527ba841e99f3b23e881e00000000b6473044022058a0c1da6bcf8c1418899ff8475f3ab6dddbff918528451c1fe71c2f7dad176302204c2e0bcf8f9b5f09e02ccfeb9256e9b34fb355ea655a5704a8a3fa920079b91501514c6b63048314335db1752102713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91ac6782012088a9147ed38daab6085c1a1e4426e61dc87a3c2c081a958821031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ac68feffffff0188540a00000000001976a91406ccabfd5f9075ecd5e8d0d31c0e973a54d51e8288ac1c2b335d]\")})) }"},"type":"TakerPaymentRefundFailed"},"timestamp":1563638060583},{"event":{"type":"Finished"},"timestamp":1563638060585}],"success_events":["Started","Negotiated","TakerFeeSent","MakerPaymentReceived","MakerPaymentWaitConfirmStarted","MakerPaymentValidatedAndConfirmed","TakerPaymentSent","TakerPaymentSpent","MakerPaymentSpent","MakerPaymentSpendConfirmed","Finished"],"uuid":"9db641f5-4300-4527-9fa6-f1c391d42c35"}"#;
        let taker_saved_swap: TakerSavedSwap = json::from_str(taker_saved_json).unwrap();

        TestCoin::ticker.mock_safe(|_| MockResult::Return("ticker"));
        TestCoin::swap_contract_address.mock_safe(|_| MockResult::Return(None));
        TestCoin::can_refund_htlc
            .mock_safe(|_, _| MockResult::Return(Box::pin(futures::future::ok(CanRefundHtlc::CanRefundNow))));

        static SEARCH_TX_SPEND_CALLED: AtomicBool = AtomicBool::new(false);
        TestCoin::search_for_swap_tx_spend_my.mock_safe(|_, _| {
            SEARCH_TX_SPEND_CALLED.store(true, Ordering::Relaxed);
            MockResult::Return(Box::pin(futures::future::ready(Ok(None))))
        });

        static REFUND_CALLED: AtomicBool = AtomicBool::new(false);
        TestCoin::send_taker_refunds_payment.mock_safe(|_, _| {
            REFUND_CALLED.store(true, Ordering::Relaxed);
            MockResult::Return(Box::pin(futures::future::ok(eth_tx_for_test().into())))
        });
        let maker_coin = MmCoinEnum::Test(TestCoin::default());
        let taker_coin = MmCoinEnum::Test(TestCoin::default());
        let (taker_swap, _) = block_on(TakerSwap::load_from_saved(
            ctx,
            maker_coin,
            taker_coin,
            taker_saved_swap,
        ))
        .unwrap();
        let actual = block_on(taker_swap.recover_funds()).unwrap();
        let expected = RecoveredSwap {
            action: RecoveredSwapAction::RefundedMyPayment,
            coin: "ticker".to_string(),
            transaction: eth_tx_for_test().into(),
        };
        assert_eq!(expected, actual);
        assert!(SEARCH_TX_SPEND_CALLED.load(Ordering::Relaxed));
        assert!(REFUND_CALLED.load(Ordering::Relaxed));
    }

    #[test]
    fn test_recover_funds_taker_swap_taker_payment_refund_failed_not_spent_too_early_to_refund() {
        let ctx = mm_ctx_with_iguana(PASSPHRASE);

        let taker_saved_json = r#"{"error_events":["StartFailed","NegotiateFailed","TakerFeeSendFailed","MakerPaymentValidateFailed","TakerPaymentTransactionFailed","TakerPaymentDataSendFailed","TakerPaymentWaitForSpendFailed","MakerPaymentSpendFailed","MakerPaymentSpendConfirmFailed","TakerPaymentRefunded","TakerPaymentRefundedByWatcher","TakerPaymentRefundFailed"],"events":[{"event":{"data":{"lock_duration":7800,"maker":"1bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","maker_amount":"0.58610590","maker_coin":"KMD","maker_coin_start_block":1450923,"maker_payment_confirmations":1,"maker_payment_wait":1563623475,"my_persistent_pub":"02713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91","started_at":1563620875,"taker_amount":"0.0077700000552410000000000","taker_coin":"LTC","taker_coin_start_block":1670837,"taker_payment_confirmations":1,"taker_payment_lock":1563628675,"uuid":"9db641f5-4300-4527-9fa6-f1c391d42c35"},"type":"Started"},"timestamp":1563620875766},{"event":{"data":{"maker_payment_locktime":1563636475,"maker_pubkey":"031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","secret_hash":"7ed38daab6085c1a1e4426e61dc87a3c2c081a95"},"type":"Negotiated"},"timestamp":1563620955014},{"event":{"data":{"tx_hash":"6740136eaaa615d9d231969e3a9599d0fc59e53989237a8d31cd6fc86c160013","tx_hex":"0100000001a2586ea8294cedc55741bef625ba72c646399903391a7f6c604a58c6263135f2000000006b4830450221009c78c8ba4a7accab6b09f9a95da5bc59c81f4fc1e60b288ec3c5462b4d02ef01022056b63be1629cf17751d3cc5ffec51bcb1d7f9396e9ce9ca254d0f34104f7263a012102713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91ffffffff0210270000000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ac78aa1900000000001976a91406ccabfd5f9075ecd5e8d0d31c0e973a54d51e8288ac5bf6325d"},"type":"TakerFeeSent"},"timestamp":1563620958220},{"event":{"data":{"tx_hash":"d0f6e664cea9d89fe7b5cf8005fdca070d1ab1d05a482aaef95c08cdaecddf0a","tx_hex":"0400008085202f89019f1cbda354342cdf982046b331bbd3791f53b692efc6e4becc36be495b2977d9000000006b483045022100fa9d4557394141f6a8b9bfb8cd594a521fd8bcd1965dbf8bc4e04abc849ac66e0220589f521814c10a7561abfd5e432f7a2ee60d4875fe4604618af3207dae531ac00121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffff029e537e030000000017a9145534898009f1467191065f6890b96914b39a1c018791857702000000001976a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac72ee325d000000000000000000000000000000"},"type":"MakerPaymentReceived"},"timestamp":1563620999307},{"event":{"type":"MakerPaymentWaitConfirmStarted"},"timestamp":1563620999310},{"event":{"type":"MakerPaymentValidatedAndConfirmed"},"timestamp":1563621244153},{"event":{"data":{"tx_hash":"1e883eb2f3991e84ba27f53651f89b7dda708678a5b9813d043577f222b9ca30","tx_hex":"01000000011300166cc86fcd318d7a238939e559fcd099953a9e9631d2d915a6aa6e134067010000006a47304402206781d5f2db2ff13d2ec7e266f774ea5630cc2dba4019e18e9716131b8b026051022006ebb33857b6d180f13aa6be2fc532f9734abde9d00ae14757e7d7ba3741c08c012102713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91ffffffff0228db0b000000000017a91483818667161bf94adda3964a81a231cbf6f5338187b0480c00000000001976a91406ccabfd5f9075ecd5e8d0d31c0e973a54d51e8288ac7cf7325d"},"type":"TakerPaymentSent"},"timestamp":1563621246370},{"event":{"data":{"error":"utxo:1145] rpc_clients:782] Waited too long until 1563628675 for output TransactionOutput { value: 777000, script_pubkey: a91483818667161bf94adda3964a81a231cbf6f5338187 } to be spent "},"type":"TakerPaymentWaitForSpendFailed"},"timestamp":1563638060370},{"event":{"data":{"error":"lp_swap:2025] utxo:938] rpc_clients:719] JsonRpcError { request: JsonRpcRequest { jsonrpc: \"2.0\", id: \"9\", method: \"blockchain.transaction.broadcast\", params: [String(\"010000000130cab922f27735043d81b9a5788670da7d9bf85136f527ba841e99f3b23e881e00000000b6473044022058a0c1da6bcf8c1418899ff8475f3ab6dddbff918528451c1fe71c2f7dad176302204c2e0bcf8f9b5f09e02ccfeb9256e9b34fb355ea655a5704a8a3fa920079b91501514c6b63048314335db1752102713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91ac6782012088a9147ed38daab6085c1a1e4426e61dc87a3c2c081a958821031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ac68feffffff0188540a00000000001976a91406ccabfd5f9075ecd5e8d0d31c0e973a54d51e8288ac1c2b335d\")] }, error: Response(Object({\"code\": Number(1), \"message\": String(\"the transaction was rejected by network rules.\\n\\nMissing inputs\\n[010000000130cab922f27735043d81b9a5788670da7d9bf85136f527ba841e99f3b23e881e00000000b6473044022058a0c1da6bcf8c1418899ff8475f3ab6dddbff918528451c1fe71c2f7dad176302204c2e0bcf8f9b5f09e02ccfeb9256e9b34fb355ea655a5704a8a3fa920079b91501514c6b63048314335db1752102713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91ac6782012088a9147ed38daab6085c1a1e4426e61dc87a3c2c081a958821031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ac68feffffff0188540a00000000001976a91406ccabfd5f9075ecd5e8d0d31c0e973a54d51e8288ac1c2b335d]\")})) }"},"type":"TakerPaymentRefundFailed"},"timestamp":1563638060583},{"event":{"type":"Finished"},"timestamp":1563638060585}],"success_events":["Started","Negotiated","TakerFeeSent","MakerPaymentReceived","MakerPaymentWaitConfirmStarted","MakerPaymentValidatedAndConfirmed","TakerPaymentSent","TakerPaymentSpent","MakerPaymentSpent","MakerPaymentSpendConfirmed","Finished"],"uuid":"9db641f5-4300-4527-9fa6-f1c391d42c35"}"#;
        let taker_saved_swap: TakerSavedSwap = json::from_str(taker_saved_json).unwrap();

        TestCoin::ticker.mock_safe(|_| MockResult::Return("ticker"));
        TestCoin::swap_contract_address.mock_safe(|_| MockResult::Return(None));
        TestCoin::can_refund_htlc
            .mock_safe(|_, _| MockResult::Return(Box::pin(futures::future::ok(CanRefundHtlc::HaveToWait(1000)))));

        static SEARCH_TX_SPEND_CALLED: AtomicBool = AtomicBool::new(false);
        TestCoin::search_for_swap_tx_spend_my.mock_safe(|_, _| {
            SEARCH_TX_SPEND_CALLED.store(true, Ordering::Relaxed);
            MockResult::Return(Box::pin(futures::future::ready(Ok(None))))
        });
        let maker_coin = MmCoinEnum::Test(TestCoin::default());
        let taker_coin = MmCoinEnum::Test(TestCoin::default());
        let (taker_swap, _) = block_on(TakerSwap::load_from_saved(
            ctx,
            maker_coin,
            taker_coin,
            taker_saved_swap,
        ))
        .unwrap();
        let error = block_on(taker_swap.recover_funds()).unwrap_err();
        assert!(error.contains("Too early to refund"));
        assert!(SEARCH_TX_SPEND_CALLED.load(Ordering::Relaxed));
    }

    #[test]
    fn test_recover_funds_taker_swap_taker_payment_refund_failed_spent_by_maker() {
        let ctx = mm_ctx_with_iguana(PASSPHRASE);

        let taker_saved_json = r#"{"error_events":["StartFailed","NegotiateFailed","TakerFeeSendFailed","MakerPaymentValidateFailed","TakerPaymentTransactionFailed","TakerPaymentDataSendFailed","TakerPaymentWaitForSpendFailed","MakerPaymentSpendFailed","MakerPaymentSpendConfirmFailed","TakerPaymentRefunded","TakerPaymentRefundedByWatcher","TakerPaymentRefundFailed"],"events":[{"event":{"data":{"lock_duration":7800,"maker":"1bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","maker_amount":"0.58610590","maker_coin":"KMD","maker_coin_start_block":1450923,"maker_payment_confirmations":1,"maker_payment_wait":1563623475,"my_persistent_pub":"02713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91","started_at":1563620875,"taker_amount":"0.0077700000552410000000000","taker_coin":"LTC","taker_coin_start_block":1670837,"taker_payment_confirmations":1,"taker_payment_lock":1563628675,"uuid":"9db641f5-4300-4527-9fa6-f1c391d42c35"},"type":"Started"},"timestamp":1563620875766},{"event":{"data":{"maker_payment_locktime":1563636475,"maker_pubkey":"031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","secret_hash":"7ed38daab6085c1a1e4426e61dc87a3c2c081a95"},"type":"Negotiated"},"timestamp":1563620955014},{"event":{"data":{"tx_hash":"6740136eaaa615d9d231969e3a9599d0fc59e53989237a8d31cd6fc86c160013","tx_hex":"0100000001a2586ea8294cedc55741bef625ba72c646399903391a7f6c604a58c6263135f2000000006b4830450221009c78c8ba4a7accab6b09f9a95da5bc59c81f4fc1e60b288ec3c5462b4d02ef01022056b63be1629cf17751d3cc5ffec51bcb1d7f9396e9ce9ca254d0f34104f7263a012102713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91ffffffff0210270000000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ac78aa1900000000001976a91406ccabfd5f9075ecd5e8d0d31c0e973a54d51e8288ac5bf6325d"},"type":"TakerFeeSent"},"timestamp":1563620958220},{"event":{"data":{"tx_hash":"d0f6e664cea9d89fe7b5cf8005fdca070d1ab1d05a482aaef95c08cdaecddf0a","tx_hex":"0400008085202f89019f1cbda354342cdf982046b331bbd3791f53b692efc6e4becc36be495b2977d9000000006b483045022100fa9d4557394141f6a8b9bfb8cd594a521fd8bcd1965dbf8bc4e04abc849ac66e0220589f521814c10a7561abfd5e432f7a2ee60d4875fe4604618af3207dae531ac00121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffff029e537e030000000017a9145534898009f1467191065f6890b96914b39a1c018791857702000000001976a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac72ee325d000000000000000000000000000000"},"type":"MakerPaymentReceived"},"timestamp":1563620999307},{"event":{"type":"MakerPaymentWaitConfirmStarted"},"timestamp":1563620999310},{"event":{"type":"MakerPaymentValidatedAndConfirmed"},"timestamp":1563621244153},{"event":{"data":{"tx_hash":"1e883eb2f3991e84ba27f53651f89b7dda708678a5b9813d043577f222b9ca30","tx_hex":"01000000011300166cc86fcd318d7a238939e559fcd099953a9e9631d2d915a6aa6e134067010000006a47304402206781d5f2db2ff13d2ec7e266f774ea5630cc2dba4019e18e9716131b8b026051022006ebb33857b6d180f13aa6be2fc532f9734abde9d00ae14757e7d7ba3741c08c012102713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91ffffffff0228db0b000000000017a91483818667161bf94adda3964a81a231cbf6f5338187b0480c00000000001976a91406ccabfd5f9075ecd5e8d0d31c0e973a54d51e8288ac7cf7325d"},"type":"TakerPaymentSent"},"timestamp":1563621246370},{"event":{"data":{"error":"utxo:1145] rpc_clients:782] Waited too long until 1563628675 for output TransactionOutput { value: 777000, script_pubkey: a91483818667161bf94adda3964a81a231cbf6f5338187 } to be spent "},"type":"TakerPaymentWaitForSpendFailed"},"timestamp":1563638060370},{"event":{"data":{"error":"lp_swap:2025] utxo:938] rpc_clients:719] JsonRpcError { request: JsonRpcRequest { jsonrpc: \"2.0\", id: \"9\", method: \"blockchain.transaction.broadcast\", params: [String(\"010000000130cab922f27735043d81b9a5788670da7d9bf85136f527ba841e99f3b23e881e00000000b6473044022058a0c1da6bcf8c1418899ff8475f3ab6dddbff918528451c1fe71c2f7dad176302204c2e0bcf8f9b5f09e02ccfeb9256e9b34fb355ea655a5704a8a3fa920079b91501514c6b63048314335db1752102713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91ac6782012088a9147ed38daab6085c1a1e4426e61dc87a3c2c081a958821031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ac68feffffff0188540a00000000001976a91406ccabfd5f9075ecd5e8d0d31c0e973a54d51e8288ac1c2b335d\")] }, error: Response(Object({\"code\": Number(1), \"message\": String(\"the transaction was rejected by network rules.\\n\\nMissing inputs\\n[010000000130cab922f27735043d81b9a5788670da7d9bf85136f527ba841e99f3b23e881e00000000b6473044022058a0c1da6bcf8c1418899ff8475f3ab6dddbff918528451c1fe71c2f7dad176302204c2e0bcf8f9b5f09e02ccfeb9256e9b34fb355ea655a5704a8a3fa920079b91501514c6b63048314335db1752102713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91ac6782012088a9147ed38daab6085c1a1e4426e61dc87a3c2c081a958821031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ac68feffffff0188540a00000000001976a91406ccabfd5f9075ecd5e8d0d31c0e973a54d51e8288ac1c2b335d]\")})) }"},"type":"TakerPaymentRefundFailed"},"timestamp":1563638060583},{"event":{"type":"Finished"},"timestamp":1563638060585}],"success_events":["Started","Negotiated","TakerFeeSent","MakerPaymentReceived","MakerPaymentWaitConfirmStarted","MakerPaymentValidatedAndConfirmed","TakerPaymentSent","TakerPaymentSpent","MakerPaymentSpent","MakerPaymentSpendConfirmed","Finished"],"uuid":"9db641f5-4300-4527-9fa6-f1c391d42c35"}"#;
        let taker_saved_swap: TakerSavedSwap = json::from_str(taker_saved_json).unwrap();

        TestCoin::ticker.mock_safe(|_| MockResult::Return("ticker"));
        TestCoin::swap_contract_address.mock_safe(|_| MockResult::Return(None));
        TestCoin::extract_secret.mock_safe(|_, _, _, _| MockResult::Return(Box::pin(async move { Ok([0; 32]) })));

        static SEARCH_TX_SPEND_CALLED: AtomicBool = AtomicBool::new(false);
        TestCoin::search_for_swap_tx_spend_my.mock_safe(|_, _| {
            SEARCH_TX_SPEND_CALLED.store(true, Ordering::Relaxed);
            let tx: UtxoTx = "0100000001de7aa8d29524906b2b54ee2e0281f3607f75662cbc9080df81d1047b78e21dbc00000000d7473044022079b6c50820040b1fbbe9251ced32ab334d33830f6f8d0bf0a40c7f1336b67d5b0220142ccf723ddabb34e542ed65c395abc1fbf5b6c3e730396f15d25c49b668a1a401209da937e5609680cb30bff4a7661364ca1d1851c2506fa80c443f00a3d3bf7365004c6b6304f62b0e5cb175210270e75970bb20029b3879ec76c4acd320a8d0589e003636264d01a7d566504bfbac6782012088a9142fb610d856c19fd57f2d0cffe8dff689074b3d8a882103f368228456c940ac113e53dad5c104cf209f2f102a409207269383b6ab9b03deac68ffffffff01d0dc9800000000001976a9146d9d2b554d768232320587df75c4338ecc8bf37d88ac40280e5c".into();
            MockResult::Return(Box::pin(futures::future::ready(Ok(Some(FoundSwapTxSpend::Spent(tx.into()))))))
        });

        TestCoin::search_for_swap_tx_spend_other
            .mock_safe(|_, _| MockResult::Return(Box::pin(futures::future::ready(Ok(None)))));

        static MAKER_PAYMENT_SPEND_CALLED: AtomicBool = AtomicBool::new(false);
        TestCoin::send_taker_spends_maker_payment.mock_safe(|_, _| {
            MAKER_PAYMENT_SPEND_CALLED.store(true, Ordering::Relaxed);
            MockResult::Return(Box::pin(futures::future::ready(Ok(eth_tx_for_test().into()))))
        });
        let maker_coin = MmCoinEnum::Test(TestCoin::default());
        let taker_coin = MmCoinEnum::Test(TestCoin::default());
        let (taker_swap, _) = block_on(TakerSwap::load_from_saved(
            ctx,
            maker_coin,
            taker_coin,
            taker_saved_swap,
        ))
        .unwrap();
        let actual = block_on(taker_swap.recover_funds()).unwrap();
        let expected = RecoveredSwap {
            action: RecoveredSwapAction::SpentOtherPayment,
            coin: "ticker".to_string(),
            transaction: eth_tx_for_test().into(),
        };
        assert_eq!(expected, actual);
        assert!(SEARCH_TX_SPEND_CALLED.load(Ordering::Relaxed));
        assert!(MAKER_PAYMENT_SPEND_CALLED.load(Ordering::Relaxed));
    }

    #[test]
    fn test_recover_funds_taker_swap_not_finished() {
        let ctx = mm_ctx_with_iguana(PASSPHRASE);

        // the json doesn't have Finished event at the end
        let taker_saved_json = r#"{"error_events":["StartFailed","NegotiateFailed","TakerFeeSendFailed","MakerPaymentValidateFailed","TakerPaymentTransactionFailed","TakerPaymentDataSendFailed","TakerPaymentWaitForSpendFailed","MakerPaymentSpendFailed","MakerPaymentSpendConfirmFailed","TakerPaymentRefunded","TakerPaymentRefundedByWatcher","TakerPaymentRefundFailed"],"events":[{"event":{"data":{"lock_duration":7800,"maker":"1bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","maker_amount":"0.12596566232185483","maker_coin":"KMD","maker_coin_start_block":1458035,"maker_payment_confirmations":1,"maker_payment_wait":1564053079,"my_persistent_pub":"0326846707a52a233cfc49a61ef51b1698bbe6aa78fa8b8d411c02743c09688f0a","started_at":1564050479,"taker_amount":"50.000000000000001504212457800000","taker_coin":"DOGE","taker_coin_start_block":2823448,"taker_payment_confirmations":1,"taker_payment_lock":1564058279,"uuid":"41383f43-46a5-478c-9386-3b2cce0aca20"},"type":"Started"},"timestamp":1564050480269},{"event":{"data":{"maker_payment_locktime":1564066080,"maker_pubkey":"031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","secret_hash":"3669eb83a007a3c507448d79f45a9f06ec2f36a8"},"type":"Negotiated"},"timestamp":1564050540991},{"event":{"data":{"tx_hash":"bdde828b492d6d1cc25cd2322fd592dafd722fcc7d8b0fedce4d3bb4a1a8c8ff","tx_hex":"0100000002c7efa995c8b7be0a8b6c2d526c6c444c1634d65584e9ee89904e9d8675eac88c010000006a473044022051f34d5e3b7d0b9098d5e35333f3550f9cb9e57df83d5e4635b7a8d2986d6d5602200288c98da05de6950e01229a637110a1800ba643e75cfec59d4eb1021ad9b40801210326846707a52a233cfc49a61ef51b1698bbe6aa78fa8b8d411c02743c09688f0affffffffae6c233989efa7c7d2aa6534adc96078917ff395b7f09f734a147b2f44ade164000000006a4730440220393a784c2da74d0e2a28ec4f7df6c8f9d8b2af6ae6957f1e68346d744223a8fd02201b7a96954ac06815a43a6c7668d829ae9cbb5de76fa77189ddfd9e3038df662c01210326846707a52a233cfc49a61ef51b1698bbe6aa78fa8b8d411c02743c09688f0affffffff02115f5800000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ac41a84641020000001976a914444f0e1099709ba4d742454a7d98a5c9c162ceab88ac6d84395d"},"type":"TakerFeeSent"},"timestamp":1564050545296},{"event":{"data":{"tx_hash":"0a0f11fa82802c2c30862c50ab2162185dae8de7f7235f32c506f814c142b382","tx_hex":"0400008085202f8902ace337db2dd4c56b0697f58fb8cfb6bd1cd6f469d925fc0376d1dcfb7581bf82000000006b483045022100d1f95be235c5c8880f5d703ace287e2768548792c58c5dbd27f5578881b30ea70220030596106e21c7e0057ee0dab283f9a1fe273f15208cba80870c447bd559ef0d0121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffff9f339752567c404427fd77f2b35cecdb4c21489edc64e25e729fdb281785e423000000006a47304402203179e95877dbc107123a417f1e648e3ff13d384890f1e4a67b6dd5087235152e0220102a8ab799fadb26b5d89ceb9c7bc721a7e0c2a0d0d7e46bbe0cf3d130010d430121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffff025635c0000000000017a91480a95d366d65e34a465ab17b0c9eb1d5a33bae08876cbfce05000000001976a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac8d7c395d000000000000000000000000000000"},"type":"MakerPaymentReceived"},"timestamp":1564050588176},{"event":{"type":"MakerPaymentWaitConfirmStarted"},"timestamp":1564050588178},{"event":{"type":"MakerPaymentValidatedAndConfirmed"},"timestamp":1564050693585},{"event":{"data":{"tx_hash":"539cb6dbdc25465bbccc575554f05d1bb04c70efce4316e41194e747375c3659","tx_hex":"0100000001ffc8a8a1b43b4dceed0f8b7dcc2f72fdda92d52f32d25cc21c6d2d498b82debd010000006a47304402203967b7f9f5532fa47116585c7d1bcba51861ea2059cca00409f34660db18e33a0220640991911852533a12fdfeb039fb9c8ca2c45482c6993bd84636af3670d49c1501210326846707a52a233cfc49a61ef51b1698bbe6aa78fa8b8d411c02743c09688f0affffffff0200f2052a0100000017a914f2fa08ae416b576779ae5da975e5442663215fce87415173f9000000001976a914444f0e1099709ba4d742454a7d98a5c9c162ceab88ac0585395d"},"type":"TakerPaymentSent"},"timestamp":1564050695611},{"event":{"data":{"secret":"1b8886b8a2cdb62505699400b694ac20f04d7bd4abd80e1ab154aa8d861fc093","transaction":{"tx_hash":"cc5af1cf68d246419fee49c3d74c0cd173599d115b86efe274368a614951bc47","tx_hex":"010000000159365c3747e79411e41643ceef704cb01b5df0545557ccbc5b4625dcdbb69c5300000000d747304402200e78e27d2f1c18676f98ca3dfa4e4a9eeaa8209b55f57b4dd5d9e1abdf034cfa0220623b5c22b62234cec230342aa306c497e43494b44ec2425b84e236b1bf01257001201b8886b8a2cdb62505699400b694ac20f04d7bd4abd80e1ab154aa8d861fc093004c6b6304a7a2395db175210326846707a52a233cfc49a61ef51b1698bbe6aa78fa8b8d411c02743c09688f0aac6782012088a9143669eb83a007a3c507448d79f45a9f06ec2f36a88821031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ac68ffffffff01008d380c010000001976a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac8c77395d"}},"type":"TakerPaymentSpent"},"timestamp":1564051092890},{"event":{"data":{"error":"lp_swap:1981] utxo:891] rpc_clients:738] JsonRpcError { request: JsonRpcRequest { jsonrpc: \"2.0\", id: \"67\", method: \"blockchain.transaction.broadcast\", params: [String(\"0400008085202f890182b342c114f806c5325f23f7e78dae5d186221ab502c86302c2c8082fa110f0a00000000d7473044022035791ea5548f87484065c9e1f0bdca9ebc699f2c7f51182c84f360102e32dc3d02200612ed53bca52d9c2568437f087598531534badf26229fe0f652ea72ddf03ca501201b8886b8a2cdb62505699400b694ac20f04d7bd4abd80e1ab154aa8d861fc093004c6b630420c1395db17521031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ac6782012088a9143669eb83a007a3c507448d79f45a9f06ec2f36a888210326846707a52a233cfc49a61ef51b1698bbe6aa78fa8b8d411c02743c09688f0aac68ffffffff01460ec000000000001976a914444f0e1099709ba4d742454a7d98a5c9c162ceab88ac967e395d000000000000000000000000000000\")] }, error: Transport(\"rpc_clients:668] All electrums are currently disconnected\") }"},"type":"MakerPaymentSpendFailed"},"timestamp":1564051092897}],"success_events":["Started","Negotiated","TakerFeeSent","MakerPaymentReceived","MakerPaymentWaitConfirmStarted","MakerPaymentValidatedAndConfirmed","TakerPaymentSent","TakerPaymentSpent","MakerPaymentSpent","MakerPaymentSpendConfirmed","Finished"],"uuid":"41383f43-46a5-478c-9386-3b2cce0aca20"}"#;
        let taker_saved_swap: TakerSavedSwap = json::from_str(taker_saved_json).unwrap();

        TestCoin::ticker.mock_safe(|_| MockResult::Return("ticker"));
        TestCoin::swap_contract_address.mock_safe(|_| MockResult::Return(None));
        let maker_coin = MmCoinEnum::Test(TestCoin::default());
        let taker_coin = MmCoinEnum::Test(TestCoin::default());
        let (taker_swap, _) = block_on(TakerSwap::load_from_saved(
            ctx,
            maker_coin,
            taker_coin,
            taker_saved_swap,
        ))
        .unwrap();
        assert!(block_on(taker_swap.recover_funds()).is_err());
    }

    #[test]
    fn test_taker_swap_event_should_ban() {
        let event = TakerSwapEvent::TakerPaymentWaitConfirmFailed("err".into());
        assert!(!event.should_ban_maker());

        let event = TakerSwapEvent::MakerPaymentWaitConfirmFailed("err".into());
        assert!(!event.should_ban_maker());

        let event = TakerSwapEvent::MakerPaymentValidateFailed("err".into());
        assert!(event.should_ban_maker());

        let event = TakerSwapEvent::TakerPaymentWaitForSpendFailed("err".into());
        assert!(event.should_ban_maker());
    }

    #[test]
    fn test_recheck_swap_contract_address_if_none() {
        let ctx = mm_ctx_with_iguana(PASSPHRASE);

        // swap file contains neither maker_coin_swap_contract_address nor taker_coin_swap_contract_address
        let taker_saved_json = r#"{"error_events":["StartFailed","NegotiateFailed","TakerFeeSendFailed","MakerPaymentValidateFailed","TakerPaymentTransactionFailed","TakerPaymentDataSendFailed","TakerPaymentWaitForSpendFailed","MakerPaymentSpendFailed","MakerPaymentSpendConfirmFailed","TakerPaymentRefunded","TakerPaymentRefundedByWatcher","TakerPaymentRefundFailed"],"events":[{"event":{"data":{"lock_duration":7800,"maker":"1bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","maker_amount":"0.58610590","maker_coin":"KMD","maker_coin_start_block":1450923,"maker_payment_confirmations":1,"maker_payment_wait":1563623475,"my_persistent_pub":"02713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91","started_at":1563620875,"taker_amount":"0.0077700000552410000000000","taker_coin":"LTC","taker_coin_start_block":1670837,"taker_payment_confirmations":1,"taker_payment_lock":1563628675,"uuid":"9db641f5-4300-4527-9fa6-f1c391d42c35"},"type":"Started"},"timestamp":1563620875766},{"event":{"data":{"maker_payment_locktime":1563636475,"maker_pubkey":"031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","secret_hash":"7ed38daab6085c1a1e4426e61dc87a3c2c081a95"},"type":"Negotiated"},"timestamp":1563620955014},{"event":{"data":{"tx_hash":"6740136eaaa615d9d231969e3a9599d0fc59e53989237a8d31cd6fc86c160013","tx_hex":"0100000001a2586ea8294cedc55741bef625ba72c646399903391a7f6c604a58c6263135f2000000006b4830450221009c78c8ba4a7accab6b09f9a95da5bc59c81f4fc1e60b288ec3c5462b4d02ef01022056b63be1629cf17751d3cc5ffec51bcb1d7f9396e9ce9ca254d0f34104f7263a012102713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91ffffffff0210270000000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ac78aa1900000000001976a91406ccabfd5f9075ecd5e8d0d31c0e973a54d51e8288ac5bf6325d"},"type":"TakerFeeSent"},"timestamp":1563620958220},{"event":{"data":{"tx_hash":"d0f6e664cea9d89fe7b5cf8005fdca070d1ab1d05a482aaef95c08cdaecddf0a","tx_hex":"0400008085202f89019f1cbda354342cdf982046b331bbd3791f53b692efc6e4becc36be495b2977d9000000006b483045022100fa9d4557394141f6a8b9bfb8cd594a521fd8bcd1965dbf8bc4e04abc849ac66e0220589f521814c10a7561abfd5e432f7a2ee60d4875fe4604618af3207dae531ac00121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffff029e537e030000000017a9145534898009f1467191065f6890b96914b39a1c018791857702000000001976a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac72ee325d000000000000000000000000000000"},"type":"MakerPaymentReceived"},"timestamp":1563620999307},{"event":{"type":"MakerPaymentWaitConfirmStarted"},"timestamp":1563620999310},{"event":{"type":"MakerPaymentValidatedAndConfirmed"},"timestamp":1563621244153},{"event":{"data":{"tx_hash":"1e883eb2f3991e84ba27f53651f89b7dda708678a5b9813d043577f222b9ca30","tx_hex":"01000000011300166cc86fcd318d7a238939e559fcd099953a9e9631d2d915a6aa6e134067010000006a47304402206781d5f2db2ff13d2ec7e266f774ea5630cc2dba4019e18e9716131b8b026051022006ebb33857b6d180f13aa6be2fc532f9734abde9d00ae14757e7d7ba3741c08c012102713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91ffffffff0228db0b000000000017a91483818667161bf94adda3964a81a231cbf6f5338187b0480c00000000001976a91406ccabfd5f9075ecd5e8d0d31c0e973a54d51e8288ac7cf7325d"},"type":"TakerPaymentSent"},"timestamp":1563621246370},{"event":{"data":{"error":"utxo:1145] rpc_clients:782] Waited too long until 1563628675 for output TransactionOutput { value: 777000, script_pubkey: a91483818667161bf94adda3964a81a231cbf6f5338187 } to be spent "},"type":"TakerPaymentWaitForSpendFailed"},"timestamp":1563638060370},{"event":{"data":{"error":"lp_swap:2025] utxo:938] rpc_clients:719] JsonRpcError { request: JsonRpcRequest { jsonrpc: \"2.0\", id: \"9\", method: \"blockchain.transaction.broadcast\", params: [String(\"010000000130cab922f27735043d81b9a5788670da7d9bf85136f527ba841e99f3b23e881e00000000b6473044022058a0c1da6bcf8c1418899ff8475f3ab6dddbff918528451c1fe71c2f7dad176302204c2e0bcf8f9b5f09e02ccfeb9256e9b34fb355ea655a5704a8a3fa920079b91501514c6b63048314335db1752102713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91ac6782012088a9147ed38daab6085c1a1e4426e61dc87a3c2c081a958821031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ac68feffffff0188540a00000000001976a91406ccabfd5f9075ecd5e8d0d31c0e973a54d51e8288ac1c2b335d\")] }, error: Response(Object({\"code\": Number(1), \"message\": String(\"the transaction was rejected by network rules.\\n\\nMissing inputs\\n[010000000130cab922f27735043d81b9a5788670da7d9bf85136f527ba841e99f3b23e881e00000000b6473044022058a0c1da6bcf8c1418899ff8475f3ab6dddbff918528451c1fe71c2f7dad176302204c2e0bcf8f9b5f09e02ccfeb9256e9b34fb355ea655a5704a8a3fa920079b91501514c6b63048314335db1752102713015d3fa4d30259e90be5f131beb593bf0131f3af2dcdb304e3322d8d52b91ac6782012088a9147ed38daab6085c1a1e4426e61dc87a3c2c081a958821031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ac68feffffff0188540a00000000001976a91406ccabfd5f9075ecd5e8d0d31c0e973a54d51e8288ac1c2b335d]\")})) }"},"type":"TakerPaymentRefundFailed"},"timestamp":1563638060583},{"event":{"type":"Finished"},"timestamp":1563638060585}],"success_events":["Started","Negotiated","TakerFeeSent","MakerPaymentReceived","MakerPaymentWaitConfirmStarted","MakerPaymentValidatedAndConfirmed","TakerPaymentSent","TakerPaymentSpent","MakerPaymentSpent","MakerPaymentSpendConfirmed","Finished"],"uuid":"9db641f5-4300-4527-9fa6-f1c391d42c35"}"#;
        let taker_saved_swap: TakerSavedSwap = json::from_str(taker_saved_json).unwrap();

        TestCoin::ticker.mock_safe(|_| MockResult::Return("ticker"));
        static SWAP_CONTRACT_ADDRESS_CALLED: AtomicUsize = AtomicUsize::new(0);
        TestCoin::swap_contract_address.mock_safe(|_| {
            SWAP_CONTRACT_ADDRESS_CALLED.fetch_add(1, Ordering::Relaxed);
            MockResult::Return(Some(BytesJson::default()))
        });
        let maker_coin = MmCoinEnum::Test(TestCoin::default());
        let taker_coin = MmCoinEnum::Test(TestCoin::default());
        let (taker_swap, _) = block_on(TakerSwap::load_from_saved(
            ctx,
            maker_coin,
            taker_coin,
            taker_saved_swap,
        ))
        .unwrap();

        assert_eq!(SWAP_CONTRACT_ADDRESS_CALLED.load(Ordering::Relaxed), 2);
        assert_eq!(
            taker_swap.r().data.maker_coin_swap_contract_address,
            Some(BytesJson::default())
        );
        assert_eq!(
            taker_swap.r().data.taker_coin_swap_contract_address,
            Some(BytesJson::default())
        );
    }

    #[test]
    fn test_recheck_only_one_swap_contract_address() {
        let ctx = mm_ctx_with_iguana(PASSPHRASE);

        // swap file contains only maker_coin_swap_contract_address
        let taker_saved_json = r#"{"type":"Taker","uuid":"49c79ea4-e1eb-4fb2-a0ef-265bded0b77f","events":[{"timestamp":1608542326909,"event":{"type":"Started","data":{"taker_coin":"RICK","maker_coin":"ETH","maker":"c6a78589e18b482aea046975e6d0acbdea7bf7dbf04d9d5bd67fda917815e3ed","my_persistent_pub":"02031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3","lock_duration":7800,"maker_amount":"0.1","taker_amount":"0.1","maker_payment_confirmations":1,"maker_payment_requires_nota":false,"taker_payment_confirmations":0,"taker_payment_requires_nota":false,"taker_payment_lock":1608550126,"uuid":"49c79ea4-e1eb-4fb2-a0ef-265bded0b77f","started_at":1608542326,"maker_payment_wait":1608545446,"maker_coin_start_block":14360,"taker_coin_start_block":723123,"maker_coin_swap_contract_address":"eA6D65434A15377081495a9E7C5893543E7c32cB"}}},{"timestamp":1608542327416,"event":{"type":"Negotiated","data":{"maker_payment_locktime":1608557926,"maker_pubkey":"03c6a78589e18b482aea046975e6d0acbdea7bf7dbf04d9d5bd67fda917815e3ed","secret_hash":"8b0221f3b977c1c65dddf17c1c28e2bbced9e7b4"}}},{"timestamp":1608542332604,"event":{"type":"TakerFeeSent","data":{"tx_hex":"0400008085202f89011ca964f77200b73d64b481f47de84098041d3470d6256e44f2741f080e2b11cf020000006b4830450221008a064f5e51ef8281d43eb7bcd016fed7e560ea1eb7b0713ec977602c96d8f79b02205bfaa6655b849b9922c03276b938273f2edb8fb9ffcaa2a9212d7220560f6060012102031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3ffffffff0246320000000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ac62752e27000000001976a91405aab5342166f8594baf17a7d9bef5d56744332788ac7768e05f000000000000000000000000000000","tx_hash":"3793df28ed2aac6188d2c48ec65eff12eea301089d60da655fc96f598326d708"}}},{"timestamp":1608542334018,"event":{"type":"MakerPaymentReceived","data":{"tx_hex":"f8ef82021c80830249f094a09ad3cd7e96586ebd05a2607ee56b56fb2db8fd88016345785d8a0000b884152cf3af50aebafeaf827c62c2eed09e265fa5aa9e013c0f27f0a88259f1aaa1279f0c32000000000000000000000000bab36286672fbdc7b250804bf6d14be0df69fa298b0221f3b977c1c65dddf17c1c28e2bbced9e7b4000000000000000000000000000000000000000000000000000000000000000000000000000000005fe0a5661ba0f18a0c5c349462b51dacd1a0761e4997d4572a01e48480c4e310d69a40308ad3a04510513f01a79c59f22c9cb79952547c8dfc4c74785b630f512d64369323e0c1","tx_hash":"6782323490584a2bc768cd5199506bfa1ed91e7515b35bb72fa269604b7dc0aa"}}},{"timestamp":1608542334019,"event":{"type":"MakerPaymentWaitConfirmStarted"}},{"timestamp":1608542334825,"event":{"type":"MakerPaymentValidatedAndConfirmed"}},{"timestamp":1608542337671,"event":{"type":"TakerPaymentSent","data":{"tx_hex":"0400008085202f890108d72683596fc95f65da609d0801a3ee12ff5ec68ec4d28861ac2aed28df9337010000006b48304502210086a03db599438b243bee2b02af56e23447f85d09854416b51305536b9ca5890e02204b288acdea4cdc7ab1ffbd9766a7bdf95f5bd02d2917dfb7089dbf29032591b0012102031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3ffffffff03809698000000000017a914888e9e1816214c3960eac7b55e35521ca4426b0c870000000000000000166a148b0221f3b977c1c65dddf17c1c28e2bbced9e7b4fada9526000000001976a91405aab5342166f8594baf17a7d9bef5d56744332788ac7f68e05f000000000000000000000000000000","tx_hash":"44fa493757df5fdca823bbac05a8b8feb5862d799d4947fd544abcd129feceea"}}},{"timestamp":1608542348271,"event":{"type":"TakerPaymentSpent","data":{"transaction":{"tx_hex":"0400008085202f8901eacefe29d1bc4a54fd47499d792d86b5feb8a805acbb23a8dc5fdf573749fa4400000000d74730440220508c853cc4f1fcb9e6aa00e704eef99adaee9a4ea63a1fd6393bb7ff18da02c802200396bb5d52157bd77ff26ac521ed75aca388d3ec1e5e3ebb7b3aed73c3d33ec50120df871242dcbcc4fe9ed4d3413e21b2f8ce606a3ee7128c9b2d2e31fcedc1848e004c6b6304ee86e05fb1752102031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3ac6782012088a9148b0221f3b977c1c65dddf17c1c28e2bbced9e7b4882103c6a78589e18b482aea046975e6d0acbdea7bf7dbf04d9d5bd67fda917815e3edac68ffffffff0198929800000000001976a9146d9d2b554d768232320587df75c4338ecc8bf37d88ac725ae05f000000000000000000000000000000","tx_hash":"9376dde62249802a0aba8259f51def9bb2e509af85a5ec7df04b479a9da28a29"},"secret":"df871242dcbcc4fe9ed4d3413e21b2f8ce606a3ee7128c9b2d2e31fcedc1848e"}}},{"timestamp":1608542349372,"event":{"type":"MakerPaymentSpent","data":{"tx_hex":"f90107821fb980830249f094a09ad3cd7e96586ebd05a2607ee56b56fb2db8fd80b8a402ed292b50aebafeaf827c62c2eed09e265fa5aa9e013c0f27f0a88259f1aaa1279f0c32000000000000000000000000000000000000000000000000016345785d8a0000df871242dcbcc4fe9ed4d3413e21b2f8ce606a3ee7128c9b2d2e31fcedc1848e00000000000000000000000000000000000000000000000000000000000000000000000000000000000000004b2d0d6c2c785217457b69b922a2a9cea98f71e91ca0ed6a4942a78c7ae6eb3c9dec496459a9ef68b34cb389acd939d13d3ecaf7e4aca021bb77e80fc60acf25a7a01cc1272b1b76594a521fb1abe1322d650e58a672c2","tx_hash":"c2d206e665aee159a5ab9aff60f76444e97bdad8f9152eccb6ca07d9204974ca"}}},{"timestamp":1608542349373,"event":{"type":"Finished"}}],"maker_amount":"0.1","maker_coin":"ETH","taker_amount":"0.1","taker_coin":"RICK","gui":"nogui","mm_version":"1a6082121","success_events":["Started","Negotiated","TakerFeeSent","MakerPaymentReceived","MakerPaymentWaitConfirmStarted","MakerPaymentValidatedAndConfirmed","TakerPaymentSent","TakerPaymentSpent","MakerPaymentSpent","MakerPaymentSpendConfirmed","Finished"],"error_events":["StartFailed","NegotiateFailed","TakerFeeSendFailed","MakerPaymentValidateFailed","MakerPaymentWaitConfirmFailed","TakerPaymentTransactionFailed","TakerPaymentWaitConfirmFailed","TakerPaymentDataSendFailed","TakerPaymentWaitForSpendFailed","MakerPaymentSpendFailed","MakerPaymentSpendConfirmFailed","TakerPaymentWaitRefundStarted","TakerPaymentRefunded","TakerPaymentRefundedByWatcher","TakerPaymentRefundFailed"]}"#;
        let taker_saved_swap: TakerSavedSwap = json::from_str(taker_saved_json).unwrap();

        TestCoin::ticker.mock_safe(|_| MockResult::Return("ticker"));
        static SWAP_CONTRACT_ADDRESS_CALLED: AtomicUsize = AtomicUsize::new(0);
        TestCoin::swap_contract_address.mock_safe(|_| {
            SWAP_CONTRACT_ADDRESS_CALLED.fetch_add(1, Ordering::Relaxed);
            MockResult::Return(Some(BytesJson::default()))
        });
        let maker_coin = MmCoinEnum::Test(TestCoin::default());
        let taker_coin = MmCoinEnum::Test(TestCoin::default());
        let (taker_swap, _) = block_on(TakerSwap::load_from_saved(
            ctx,
            maker_coin,
            taker_coin,
            taker_saved_swap,
        ))
        .unwrap();

        assert_eq!(SWAP_CONTRACT_ADDRESS_CALLED.load(Ordering::Relaxed), 1);
        let expected_addr = addr_from_str(ETH_SEPOLIA_SWAP_CONTRACT).unwrap();
        let expected = BytesJson::from(expected_addr.0.as_ref());
        assert_eq!(taker_swap.r().data.maker_coin_swap_contract_address, Some(expected));
        assert_eq!(
            taker_swap.r().data.taker_coin_swap_contract_address,
            Some(BytesJson::default())
        );
    }

    #[test]
    // https://github.com/KomodoPlatform/atomicDEX-API/issues/647
    fn test_recoverable() {
        // Swap ended with MakerPaymentWaitConfirmFailed event.
        // MM2 did not attempt to send the payment in this case so swap is not recoverable.
        let swap: TakerSavedSwap = json::from_str(r#"{"error_events":["StartFailed","NegotiateFailed","TakerFeeSendFailed","MakerPaymentValidateFailed","MakerPaymentWaitConfirmFailed","TakerPaymentTransactionFailed","TakerPaymentWaitConfirmFailed","TakerPaymentDataSendFailed","TakerPaymentWaitForSpendFailed","MakerPaymentSpendFailed","MakerPaymentSpendConfirmFailed","TakerPaymentWaitRefundStarted","TakerPaymentRefunded","TakerPaymentRefundedByWatcher","TakerPaymentRefundFailed"],"events":[{"event":{"data":{"lock_duration":7800,"maker":"1bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","maker_amount":"0.12596566232185483","maker_coin":"KMD","maker_coin_start_block":1458035,"maker_payment_confirmations":1,"maker_payment_wait":1564053079,"my_persistent_pub":"0326846707a52a233cfc49a61ef51b1698bbe6aa78fa8b8d411c02743c09688f0a","started_at":1564050479,"taker_amount":"50.000000000000001504212457800000","taker_coin":"DOGE","taker_coin_start_block":2823448,"taker_payment_confirmations":1,"taker_payment_lock":1564058279,"uuid":"41383f43-46a5-478c-9386-3b2cce0aca20"},"type":"Started"},"timestamp":1564050480269},{"event":{"data":{"maker_payment_locktime":1564066080,"maker_pubkey":"031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8","secret_hash":"3669eb83a007a3c507448d79f45a9f06ec2f36a8"},"type":"Negotiated"},"timestamp":1564050540991},{"event":{"data":{"tx_hash":"bdde828b492d6d1cc25cd2322fd592dafd722fcc7d8b0fedce4d3bb4a1a8c8ff","tx_hex":"0100000002c7efa995c8b7be0a8b6c2d526c6c444c1634d65584e9ee89904e9d8675eac88c010000006a473044022051f34d5e3b7d0b9098d5e35333f3550f9cb9e57df83d5e4635b7a8d2986d6d5602200288c98da05de6950e01229a637110a1800ba643e75cfec59d4eb1021ad9b40801210326846707a52a233cfc49a61ef51b1698bbe6aa78fa8b8d411c02743c09688f0affffffffae6c233989efa7c7d2aa6534adc96078917ff395b7f09f734a147b2f44ade164000000006a4730440220393a784c2da74d0e2a28ec4f7df6c8f9d8b2af6ae6957f1e68346d744223a8fd02201b7a96954ac06815a43a6c7668d829ae9cbb5de76fa77189ddfd9e3038df662c01210326846707a52a233cfc49a61ef51b1698bbe6aa78fa8b8d411c02743c09688f0affffffff02115f5800000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ac41a84641020000001976a914444f0e1099709ba4d742454a7d98a5c9c162ceab88ac6d84395d"},"type":"TakerFeeSent"},"timestamp":1564050545296},{"event":{"data":{"tx_hash":"0a0f11fa82802c2c30862c50ab2162185dae8de7f7235f32c506f814c142b382","tx_hex":"0400008085202f8902ace337db2dd4c56b0697f58fb8cfb6bd1cd6f469d925fc0376d1dcfb7581bf82000000006b483045022100d1f95be235c5c8880f5d703ace287e2768548792c58c5dbd27f5578881b30ea70220030596106e21c7e0057ee0dab283f9a1fe273f15208cba80870c447bd559ef0d0121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffff9f339752567c404427fd77f2b35cecdb4c21489edc64e25e729fdb281785e423000000006a47304402203179e95877dbc107123a417f1e648e3ff13d384890f1e4a67b6dd5087235152e0220102a8ab799fadb26b5d89ceb9c7bc721a7e0c2a0d0d7e46bbe0cf3d130010d430121031bb83b58ec130e28e0a6d5d2acf2eb01b0d3f1670e021d47d31db8a858219da8ffffffff025635c0000000000017a91480a95d366d65e34a465ab17b0c9eb1d5a33bae08876cbfce05000000001976a914c3f710deb7320b0efa6edb14e3ebeeb9155fa90d88ac8d7c395d000000000000000000000000000000"},"type":"MakerPaymentReceived"},"timestamp":1564050588176},{"event":{"type":"MakerPaymentWaitConfirmStarted"},"timestamp":1564050588178},{"event":{"data":{"error":"error"},"type":"MakerPaymentWaitConfirmFailed"},"timestamp":1564051092897},{"event":{"type":"Finished"},"timestamp":1564051092900}],"success_events":["Started","Negotiated","TakerFeeSent","MakerPaymentReceived","MakerPaymentWaitConfirmStarted","MakerPaymentValidatedAndConfirmed","TakerPaymentSent","TakerPaymentSpent","MakerPaymentSpent","MakerPaymentSpendConfirmed","Finished"],"uuid":"41383f43-46a5-478c-9386-3b2cce0aca20"}"#).unwrap();
        assert!(!swap.is_recoverable());
    }

    #[test]
    fn test_max_taker_vol_from_available() {
        let min_tx_amount = MmNumber::from("0.00001");

        // For these `availables` the dex_fee must be greater than min_tx_amount
        let source = vec![
            ("0.00779", false),
            ("0.01", false),
            ("0.0135", false),
            ("1.2000001", false),
            ("99999999999999999999999999999999999999999999999999999", false),
            ("0.00778000000000000000000000000000000000000000000000002", false),
            ("0.00778000000000000000000000000000000000000000000000001", false),
            ("0.00863333333333333333333333333333333333333333333333334", true),
            ("0.00863333333333333333333333333333333333333333333333333", true),
        ];
        for (available, is_kmd) in source {
            let available = MmNumber::from(available);
            // no matter base or rel is KMD
            let base = if is_kmd { "RICK" } else { "MORTY" };
            let max_taker_vol = max_taker_vol_from_available(available.clone(), "RICK", "MORTY", &min_tx_amount)
                .expect("!max_taker_vol_from_available");

            let coin = TestCoin::new(base);
            let mock_min_tx_amount = min_tx_amount.clone();
            TestCoin::min_tx_amount.mock_safe(move |_| MockResult::Return(mock_min_tx_amount.clone().into()));
            let dex_fee = DexFee::new_from_taker_coin(&coin, "MORTY", &max_taker_vol).total_spend_amount();
            assert!(min_tx_amount < dex_fee);
            assert!(min_tx_amount <= max_taker_vol);
            assert_eq!(max_taker_vol + dex_fee, available);
        }

        // for these `availables` the dex_fee must be the same as min_tx_amount
        let source = vec![
            ("0.00863333333333333333333333333333333333333333333333332", true),
            ("0.00863333333333333333333333333333333333333333333333331", true),
            ("0.00777999999999999999999999999999999999999999999999999", false),
            ("0.00777", false),
            ("0.00002001", false),
        ];
        for (available, is_kmd) in source {
            let available = MmNumber::from(available);
            // no matter base or rel is KMD
            let base = if is_kmd { "KMD" } else { "RICK" };
            let max_taker_vol = max_taker_vol_from_available(available.clone(), base, "MORTY", &min_tx_amount)
                .expect("!max_taker_vol_from_available");

            let coin = TestCoin::new(base);
            let mock_min_tx_amount = min_tx_amount.clone();
            TestCoin::min_tx_amount.mock_safe(move |_| MockResult::Return(mock_min_tx_amount.clone().into()));
            let dex_fee = DexFee::new_from_taker_coin(&coin, "MORTY", &max_taker_vol).fee_amount(); // returns Standard dex_fee (default for TestCoin)
            println!(
                "available={:?} max_taker_vol={:?} dex_fee={:?}",
                available.to_decimal(),
                max_taker_vol.to_decimal(),
                dex_fee.to_decimal()
            );
            assert_eq!(min_tx_amount, dex_fee);
            assert!(min_tx_amount <= max_taker_vol);
            assert_eq!(max_taker_vol + dex_fee, available);
        }

        // these `availables` must return an error
        let availables = vec![
            "0.00002",
            "0.000011",
            "0.00001000000000000000000000000000000000000000000000001",
            "0.00001",
            "0.00000999999999999999999999999999999999999999999999999",
            "0.00000000000000000000000000000000000000000000000000001",
            "0",
            "-2",
        ];
        for available in availables {
            let available = MmNumber::from(available);
            max_taker_vol_from_available(available.clone(), "KMD", "MORTY", &min_tx_amount)
                .expect_err("!max_taker_vol_from_available success but should be error");
        }
    }

    #[test]
    fn locked_amount_should_not_use_paid_from_trading_vol_fee() {
        use crate::lp_swap::get_locked_amount;

        let taker_saved_json = r#"{
            "type": "Taker",
            "uuid": "af5e0383-97f6-4408-8c03-a8eb8d17e46d",
            "my_order_uuid": "af5e0383-97f6-4408-8c03-a8eb8d17e46d",
            "events": [
                {
                    "timestamp": 1617096259172,
                    "event": {
                        "type": "Started",
                        "data": {
                            "taker_coin": "MORTY",
                            "maker_coin": "RICK",
                            "maker": "15d9c51c657ab1be4ae9d3ab6e76a619d3bccfe830d5363fa168424c0d044732",
                            "my_persistent_pub": "03ad6f89abc2e5beaa8a3ac28e22170659b3209fe2ddf439681b4b8f31508c36fa",
                            "lock_duration": 7800,
                            "maker_amount": "0.1",
                            "taker_amount": "0.11",
                            "maker_payment_confirmations": 1,
                            "maker_payment_requires_nota": false,
                            "taker_payment_confirmations": 1,
                            "taker_payment_requires_nota": false,
                            "taker_payment_lock": 1617104058,
                            "uuid": "af5e0383-97f6-4408-8c03-a8eb8d17e46d",
                            "started_at": 1617096258,
                            "maker_payment_wait": 1617099378,
                            "maker_coin_start_block": 865240,
                            "taker_coin_start_block": 869167,
                            "fee_to_send_taker_fee": {
                                "coin": "MORTY",
                                "amount": "0.00001",
                                "paid_from_trading_vol": false
                            },
                            "taker_payment_trade_fee": {
                                "coin": "MORTY",
                                "amount": "0.00001",
                                "paid_from_trading_vol": false
                            },
                            "maker_payment_spend_trade_fee": {
                                "coin": "RICK",
                                "amount": "0.00001",
                                "paid_from_trading_vol": true
                            }
                        }
                    }
                }
            ],
            "maker_amount": "0.1",
            "maker_coin": "RICK",
            "taker_amount": "0.11",
            "taker_coin": "MORTY",
            "gui": null,
            "mm_version": "21867da64",
            "success_events": [],
            "error_events": []
        }"#;
        let taker_saved_swap: TakerSavedSwap = json::from_str(taker_saved_json).unwrap();

        let ctx = mm_ctx_with_iguana(PASSPHRASE);

        let maker_coin = MmCoinEnum::Test(TestCoin::new("RICK"));
        let taker_coin = MmCoinEnum::Test(TestCoin::new("MORTY"));

        TestCoin::swap_contract_address.mock_safe(|_| MockResult::Return(None));
        TestCoin::min_tx_amount.mock_safe(|_| MockResult::Return(BigDecimal::from(0)));

        let (swap, _) = block_on(TakerSwap::load_from_saved(
            ctx.clone(),
            maker_coin,
            taker_coin,
            taker_saved_swap,
        ))
        .unwrap();
        let swaps_ctx = SwapsContext::from_ctx(&ctx).unwrap();
        let arc = Arc::new(swap);
        swaps_ctx.running_swaps.lock().unwrap().insert(arc.uuid, arc);

        let actual = get_locked_amount(&ctx, "RICK");
        assert_eq!(actual, MmNumber::from(0));

        let actual = get_locked_amount_by_other_swaps(&ctx, &new_uuid(), "RICK");
        assert_eq!(actual, MmNumber::from(0));
    }
}
