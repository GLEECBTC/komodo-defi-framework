use super::{
    broadcast_p2p_tx_msg, get_payment_locktime, lp_coinfind, taker_payment_spend_deadline, tx_helper_topic, H256Json,
    SwapsContext, TAKER_FEE_VALIDATION_ATTEMPTS, TAKER_FEE_VALIDATION_RETRY_DELAY_SECS, WAIT_CONFIRM_INTERVAL_SEC,
};
use crate::lp_network::{P2PRequestError, P2PRequestResult};

use crate::MmError;
use async_trait::async_trait;
use coins::{
    CanRefundHtlc, ConfirmPaymentInput, FoundSwapTxSpend, MmCoinEnum, RefundPaymentArgs,
    SendMakerPaymentSpendPreimageInput, SwapTxTypeWithSecretHash, WaitForHTLCTxSpendArgs,
    WatcherSearchForSwapTxSpendInput, WatcherValidatePaymentInput, WatcherValidateTakerFeeInput,
};
use common::executor::{AbortSettings, SpawnAbortable, Timer};
use common::log::{debug, error, info};
use common::now_sec;
use compatible_time::Duration;
use futures::compat::Future01CompatExt;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::MapToMmResult;
use mm2_libp2p::{decode_signed, pub_sub_topic, TopicPrefix};
use mm2_state_machine::prelude::*;
use mm2_state_machine::state_machine::StateMachineTrait;
use serde::{Deserialize, Serialize};
use serde_json as json;
use std::cmp::min;
use std::convert::{Infallible, TryInto};
use std::sync::Arc;
use uuid::Uuid;

pub const WATCHER_PREFIX: TopicPrefix = "swpwtchr";
const TAKER_SWAP_CONFIRMATIONS: u64 = 1;
pub const TAKER_SWAP_ENTRY_TIMEOUT_SEC: u64 = 21600;

pub const MAKER_PAYMENT_SPEND_SENT_LOG: &str = "Maker payment spend sent";
pub const MAKER_PAYMENT_SPEND_FOUND_LOG: &str = "Maker payment spend found by watcher";
pub const TAKER_PAYMENT_REFUND_SENT_LOG: &str = "Taker payment refund sent";

struct WatcherStateMachine {
    ctx: MmArc,
    taker_coin: MmCoinEnum,
    maker_coin: MmCoinEnum,
    verified_pub: Vec<u8>,
    data: TakerSwapWatcherData,
    conf: WatcherConf,
    watcher_reward: bool,
}

impl StateMachineTrait for WatcherStateMachine {
    type Result = ();
    type Error = Infallible;
}

impl StandardStateMachine for WatcherStateMachine {}

impl WatcherStateMachine {
    fn taker_locktime(&self) -> u64 {
        self.data.swap_started_at + self.data.lock_duration
    }

    fn wait_for_maker_payment_spend_deadline(&self) -> u64 {
        let factor = self.conf.wait_maker_payment_spend_factor;
        self.data.swap_started_at + (factor * self.data.lock_duration as f64) as u64
    }

    fn refund_start_time(&self) -> u64 {
        let factor = self.conf.refund_start_factor;
        self.data.swap_started_at + (factor * self.data.lock_duration as f64) as u64
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WatcherConf {
    #[serde(default = "common::sixty_f64")]
    wait_taker_payment: f64,
    #[serde(default = "default_watcher_maker_payment_spend_factor")]
    wait_maker_payment_spend_factor: f64,
    #[serde(default = "default_watcher_refund_factor")]
    refund_start_factor: f64,
    #[serde(default = "common::three_hundred_f64")]
    search_interval: f64,
}

impl Default for WatcherConf {
    fn default() -> Self {
        WatcherConf {
            wait_taker_payment: common::sixty_f64(),
            wait_maker_payment_spend_factor: default_watcher_maker_payment_spend_factor(),
            refund_start_factor: default_watcher_refund_factor(),
            search_interval: common::three_hundred_f64(),
        }
    }
}

pub fn default_watcher_maker_payment_spend_factor() -> f64 {
    common::one_f64()
}

pub fn default_watcher_refund_factor() -> f64 {
    common::one_and_half_f64()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum SwapWatcherMsg {
    TakerSwapWatcherMsg(TakerSwapWatcherData),
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct TakerSwapWatcherData {
    pub uuid: Uuid,
    pub secret_hash: Vec<u8>,
    pub maker_payment_spend_preimage: Vec<u8>,
    pub taker_payment_refund_preimage: Vec<u8>,
    pub swap_started_at: u64,
    pub lock_duration: u64,
    pub taker_coin: String,
    pub taker_fee_hash: Vec<u8>,
    pub taker_payment_hash: Vec<u8>,
    pub taker_coin_start_block: u64,
    pub taker_payment_confirmations: u64,
    pub taker_payment_requires_nota: Option<bool>,
    pub maker_coin: String,
    pub maker_pub: Vec<u8>,
    pub maker_payment_hash: Vec<u8>,
    pub maker_coin_start_block: u64,
}

#[allow(dead_code)]
struct ValidatePublicKeys {}

#[allow(dead_code)]
struct ValidateTakerFee {}

#[allow(dead_code)]
struct ValidateTakerPayment {}

struct WaitForTakerPaymentSpend {
    taker_payment_hex: Vec<u8>,
}

struct RefundTakerPayment {}

struct SpendMakerPayment {
    secret: H256Json,
}

impl SpendMakerPayment {
    fn new(secret: H256Json) -> Self {
        SpendMakerPayment { secret }
    }
}

struct Stopped {
    stop_reason: StopReason,
}

#[expect(dead_code)]
#[derive(Debug)]
enum StopReason {
    Finished(WatcherSuccess),
    Error(MmError<WatcherError>),
}

#[derive(Debug)]
enum WatcherSuccess {
    MakerPaymentSpent,
    TakerPaymentRefunded,
    MakerPaymentSpentByTaker,
    TakerPaymentRefundedByTaker,
}

#[expect(dead_code)]
#[derive(Debug)]
enum WatcherError {
    InvalidTakerFee(String),
    TakerPaymentNotConfirmed(String),
    InvalidTakerPayment(String),
    UnableToExtractSecret(String),
    MakerPaymentSpendFailed(String),
    MakerPaymentCouldNotBeFound(String),
    TakerPaymentRefundFailed(String),
}

impl Stopped {
    fn from_reason(stop_reason: StopReason) -> Stopped {
        Stopped { stop_reason }
    }
}

impl TransitionFrom<ValidateTakerFee> for ValidateTakerPayment {}
impl TransitionFrom<ValidateTakerPayment> for WaitForTakerPaymentSpend {}
impl TransitionFrom<WaitForTakerPaymentSpend> for SpendMakerPayment {}
impl TransitionFrom<WaitForTakerPaymentSpend> for RefundTakerPayment {}
impl TransitionFrom<ValidatePublicKeys> for Stopped {}
impl TransitionFrom<ValidateTakerFee> for Stopped {}
impl TransitionFrom<ValidateTakerPayment> for Stopped {}
impl TransitionFrom<WaitForTakerPaymentSpend> for Stopped {}
impl TransitionFrom<RefundTakerPayment> for Stopped {}
impl TransitionFrom<SpendMakerPayment> for Stopped {}

#[async_trait]
impl State for ValidateTakerFee {
    type StateMachine = WatcherStateMachine;

    async fn on_changed(self: Box<Self>, watcher_ctx: &mut WatcherStateMachine) -> StateResult<WatcherStateMachine> {
        debug!("Watcher validate taker fee");

        let validation_result = retry_on_err!(async {
            watcher_ctx
                .taker_coin
                .watcher_validate_taker_fee(WatcherValidateTakerFeeInput {
                    taker_fee_hash: watcher_ctx.data.taker_fee_hash.clone(),
                    sender_pubkey: watcher_ctx.verified_pub.clone(),
                    min_block_number: watcher_ctx.data.taker_coin_start_block,
                    lock_duration: watcher_ctx.data.lock_duration,
                })
                .compat()
                .await
        })
        .repeat_every_secs(TAKER_FEE_VALIDATION_RETRY_DELAY_SECS)
        .attempts(TAKER_FEE_VALIDATION_ATTEMPTS)
        .inspect_err(|e| error!("Error validating taker fee: {}", e))
        .await;

        match validation_result {
            Ok(_) => Self::change_state(ValidateTakerPayment {}),
            Err(repeat_err) => Self::change_state(Stopped::from_reason(StopReason::Error(
                WatcherError::InvalidTakerFee(repeat_err.to_string()).into(),
            ))),
        }
    }
}

// TODO: Validate also maker payment
#[async_trait]
impl State for ValidateTakerPayment {
    type StateMachine = WatcherStateMachine;

    async fn on_changed(self: Box<Self>, watcher_ctx: &mut WatcherStateMachine) -> StateResult<WatcherStateMachine> {
        debug!("Watcher validate taker payment");
        let taker_payment_spend_deadline =
            taker_payment_spend_deadline(watcher_ctx.data.swap_started_at, watcher_ctx.data.lock_duration);

        let sleep_duration = watcher_ctx.conf.wait_taker_payment;
        Timer::sleep(sleep_duration).await;

        let taker_payment_hex_fut = watcher_ctx
            .taker_coin
            .get_tx_hex_by_hash(watcher_ctx.data.taker_payment_hash.clone());

        let taker_payment_hex = match taker_payment_hex_fut.compat().await {
            Ok(tx_res) => tx_res.tx_hex.into_vec(),
            Err(err) => {
                return Self::change_state(Stopped::from_reason(StopReason::Error(
                    WatcherError::InvalidTakerPayment(err.to_string()).into(),
                )));
            },
        };

        let confirmations = min(watcher_ctx.data.taker_payment_confirmations, TAKER_SWAP_CONFIRMATIONS);
        let confirm_taker_payment_input = ConfirmPaymentInput {
            payment_tx: taker_payment_hex.clone(),
            confirmations,
            requires_nota: watcher_ctx.data.taker_payment_requires_nota.unwrap_or(false),
            wait_until: taker_payment_spend_deadline,
            check_every: WAIT_CONFIRM_INTERVAL_SEC,
        };

        let wait_fut = watcher_ctx
            .taker_coin
            .wait_for_confirmations(confirm_taker_payment_input)
            .compat();
        if let Err(err) = wait_fut.await {
            return Self::change_state(Stopped::from_reason(StopReason::Error(
                WatcherError::TakerPaymentNotConfirmed(err).into(),
            )));
        }

        let validate_input = WatcherValidatePaymentInput {
            payment_tx: taker_payment_hex.clone(),
            taker_payment_refund_preimage: watcher_ctx.data.taker_payment_refund_preimage.clone(),
            time_lock: watcher_ctx.taker_locktime(),
            taker_pub: watcher_ctx.verified_pub.clone(),
            maker_pub: watcher_ctx.data.maker_pub.clone(),
            secret_hash: watcher_ctx.data.secret_hash.clone(),
            wait_until: taker_payment_spend_deadline,
            confirmations,
            maker_coin: watcher_ctx.maker_coin.clone(),
        };

        let validated_f = watcher_ctx
            .taker_coin
            .watcher_validate_taker_payment(validate_input)
            .compat();

        if let Err(err) = validated_f.await {
            return Self::change_state(Stopped::from_reason(StopReason::Error(
                WatcherError::InvalidTakerPayment(err.to_string()).into(),
            )));
        }

        Self::change_state(WaitForTakerPaymentSpend { taker_payment_hex })
    }
}

#[async_trait]
impl State for WaitForTakerPaymentSpend {
    type StateMachine = WatcherStateMachine;

    async fn on_changed(self: Box<Self>, watcher_ctx: &mut WatcherStateMachine) -> StateResult<WatcherStateMachine> {
        debug!("Watcher wait for taker payment spend");
        let payment_search_interval = watcher_ctx.conf.search_interval;
        let wait_until = watcher_ctx.refund_start_time();
        let search_input = WatcherSearchForSwapTxSpendInput {
            time_lock: watcher_ctx.taker_locktime() as u32,
            taker_pub: &watcher_ctx.verified_pub,
            maker_pub: &watcher_ctx.data.maker_pub,
            secret_hash: &watcher_ctx.data.secret_hash,
            tx: &self.taker_payment_hex,
            search_from_block: watcher_ctx.data.taker_coin_start_block,
            watcher_reward: watcher_ctx.watcher_reward,
        };

        loop {
            if now_sec() > wait_until {
                info!(
                    "Waited too long until {} for transaction {:?} to be spent",
                    wait_until, self.taker_payment_hex
                );
                return Self::change_state(RefundTakerPayment {});
            }

            let f = watcher_ctx
                .taker_coin
                .watcher_search_for_swap_tx_spend(search_input.clone())
                .await;

            let tx = match f {
                Ok(Some(FoundSwapTxSpend::Spent(tx))) => tx,
                Ok(Some(FoundSwapTxSpend::Refunded(_))) => {
                    return Self::change_state(Stopped::from_reason(StopReason::Finished(
                        WatcherSuccess::TakerPaymentRefundedByTaker,
                    )))
                },
                Ok(None) => {
                    debug!(
                        "Spend or refund for taker payment tx {:?} was not found",
                        &self.taker_payment_hex
                    );
                    Timer::sleep(payment_search_interval).await;
                    continue;
                },
                Err(err) => {
                    error!("{}", err);
                    Timer::sleep(payment_search_interval).await;
                    continue;
                },
            };

            let now = now_sec();
            if now < watcher_ctx.taker_locktime() {
                let wait_until = watcher_ctx.wait_for_maker_payment_spend_deadline();
                let maker_payment_hex_fut = watcher_ctx
                    .maker_coin
                    .get_tx_hex_by_hash(watcher_ctx.data.maker_payment_hash.clone());
                let maker_payment_hex = match maker_payment_hex_fut.compat().await {
                    Ok(tx_res) => tx_res.tx_hex.into_vec(),
                    Err(err) => {
                        return Self::change_state(Stopped::from_reason(StopReason::Error(
                            WatcherError::MakerPaymentCouldNotBeFound(err.to_string()).into(),
                        )))
                    },
                };

                if watcher_ctx
                    .maker_coin
                    .wait_for_htlc_tx_spend(WaitForHTLCTxSpendArgs {
                        tx_bytes: &maker_payment_hex,
                        secret_hash: &watcher_ctx.data.secret_hash,
                        wait_until,
                        from_block: watcher_ctx.data.maker_coin_start_block,
                        swap_contract_address: &None,
                        check_every: payment_search_interval,
                        watcher_reward: watcher_ctx.watcher_reward,
                    })
                    .await
                    .is_ok()
                {
                    info!("{}", MAKER_PAYMENT_SPEND_FOUND_LOG);
                    return Self::change_state(Stopped::from_reason(StopReason::Finished(
                        WatcherSuccess::MakerPaymentSpentByTaker,
                    )));
                }
            }

            let tx_hex = tx.tx_hex();
            let secret = match watcher_ctx
                .taker_coin
                .extract_secret(&watcher_ctx.data.secret_hash, &tx_hex, true)
                .await
            {
                Ok(secret) => H256Json::from(secret),
                Err(err) => {
                    return Self::change_state(Stopped::from_reason(StopReason::Error(
                        WatcherError::UnableToExtractSecret(err).into(),
                    )))
                },
            };
            return Self::change_state(SpendMakerPayment::new(secret));
        }
    }
}

#[async_trait]
impl State for SpendMakerPayment {
    type StateMachine = WatcherStateMachine;

    async fn on_changed(self: Box<Self>, watcher_ctx: &mut WatcherStateMachine) -> StateResult<WatcherStateMachine> {
        debug!("Watcher spend maker payment");
        let spend_fut = watcher_ctx
            .maker_coin
            .send_maker_payment_spend_preimage(SendMakerPaymentSpendPreimageInput {
                preimage: &watcher_ctx.data.maker_payment_spend_preimage,
                secret: &self.secret.0,
                secret_hash: &watcher_ctx.data.secret_hash,
                taker_pub: &watcher_ctx.verified_pub,
                watcher_reward: watcher_ctx.watcher_reward,
            });

        let transaction = match spend_fut.compat().await {
            Ok(t) => t,
            Err(err) => {
                if let Some(tx) = err.get_tx() {
                    broadcast_p2p_tx_msg(
                        &watcher_ctx.ctx,
                        tx_helper_topic(watcher_ctx.maker_coin.ticker()),
                        &tx,
                        &None,
                    );
                };
                return Self::change_state(Stopped::from_reason(StopReason::Error(
                    WatcherError::MakerPaymentSpendFailed(err.get_plain_text_format()).into(),
                )));
            },
        };

        broadcast_p2p_tx_msg(
            &watcher_ctx.ctx,
            tx_helper_topic(watcher_ctx.maker_coin.ticker()),
            &transaction,
            &None,
        );

        let tx_hash = transaction.tx_hash_as_bytes();
        info!(
            "{}: Maker payment spend tx {:02x} sent by watcher",
            MAKER_PAYMENT_SPEND_SENT_LOG, tx_hash
        );

        Self::change_state(Stopped::from_reason(StopReason::Finished(
            WatcherSuccess::MakerPaymentSpent,
        )))
    }
}

#[async_trait]
impl State for RefundTakerPayment {
    type StateMachine = WatcherStateMachine;

    async fn on_changed(self: Box<Self>, watcher_ctx: &mut WatcherStateMachine) -> StateResult<WatcherStateMachine> {
        debug!("Watcher refund taker payment");
        loop {
            match watcher_ctx
                .taker_coin
                .can_refund_htlc(watcher_ctx.taker_locktime())
                .await
            {
                Ok(CanRefundHtlc::CanRefundNow) => break,
                Ok(CanRefundHtlc::HaveToWait(to_sleep)) => Timer::sleep(to_sleep as f64).await,
                Err(e) => {
                    error!("Error {} on can_refund_htlc, retrying in 30 seconds", e);
                    Timer::sleep(30.).await;
                },
            }
        }

        let refund_fut = watcher_ctx
            .taker_coin
            .send_taker_payment_refund_preimage(RefundPaymentArgs {
                payment_tx: &watcher_ctx.data.taker_payment_refund_preimage,
                swap_contract_address: &None,
                tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
                    maker_secret_hash: &watcher_ctx.data.secret_hash,
                },
                other_pubkey: &watcher_ctx.verified_pub,
                time_lock: watcher_ctx.taker_locktime(),
                swap_unique_data: &[],
                watcher_reward: watcher_ctx.watcher_reward,
            });
        let transaction = match refund_fut.compat().await {
            Ok(t) => t,
            Err(err) => {
                if let Some(tx) = err.get_tx() {
                    broadcast_p2p_tx_msg(
                        &watcher_ctx.ctx,
                        tx_helper_topic(watcher_ctx.taker_coin.ticker()),
                        &tx,
                        &None,
                    );
                }

                return Self::change_state(Stopped::from_reason(StopReason::Error(
                    WatcherError::TakerPaymentRefundFailed(err.get_plain_text_format()).into(),
                )));
            },
        };

        broadcast_p2p_tx_msg(
            &watcher_ctx.ctx,
            tx_helper_topic(watcher_ctx.taker_coin.ticker()),
            &transaction,
            &None,
        );

        let tx_hash = transaction.tx_hash_as_bytes();
        info!(
            "{}: Taker payment refund tx {:02x} sent by watcher",
            TAKER_PAYMENT_REFUND_SENT_LOG, tx_hash
        );
        Self::change_state(Stopped::from_reason(StopReason::Finished(
            WatcherSuccess::TakerPaymentRefunded,
        )))
    }
}

#[async_trait]
impl LastState for Stopped {
    type StateMachine = WatcherStateMachine;

    async fn on_changed(self: Box<Self>, watcher_ctx: &mut WatcherStateMachine) -> () {
        info!(
            "Watcher loop for swap {} stopped with reason {:?}",
            watcher_ctx.data.uuid, self.stop_reason
        )
    }
}

pub fn process_watcher_msg(ctx: MmArc, msg: &[u8]) -> P2PRequestResult<()> {
    let msg = decode_signed::<SwapWatcherMsg>(msg).map_to_mm(|e| P2PRequestError::DecodeError(e.to_string()))?;

    let watcher_data = msg.0;
    let verified_pubkey = msg.2;
    match watcher_data {
        SwapWatcherMsg::TakerSwapWatcherMsg(watcher_data) => {
            spawn_taker_swap_watcher(ctx, watcher_data, verified_pubkey.to_bytes())
        },
    };

    Ok(())
}

/// Currently, Taker Swap Watcher is supported only.
enum WatcherType {
    Taker,
}

/// The `SwapWatcherLock` is used to lock the given taker fee hash as the running Swap Watcher,
/// (i.e. insert the fee hash into either [`SwapsContext::taker_swap_watchers`] or [`SwapsContext::maker_swap_watchers`]),
/// and to unlock it (i.e remove the hash from corresponding watcher collection) once `SwapWatcherLock` is dropped.
struct SwapWatcherLock {
    swap_ctx: Arc<SwapsContext>,
    fee_hash: Vec<u8>,
    watcher_type: WatcherType,
}

impl SwapWatcherLock {
    /// Locks the given taker fee hash as the running Swap Watcher,
    /// so inserts the hash into the [`SwapsContext::taker_swap_watchers`] collection.
    ///
    /// Returns `None` if there is an ongoing Taker Swap Watcher already.
    fn lock_taker(swap_ctx: Arc<SwapsContext>, fee_hash: Vec<u8>) -> Option<Self> {
        {
            let mut guard = swap_ctx.taker_swap_watchers.lock();
            if guard
                .insert_expirable(fee_hash.clone(), (), Duration::from_secs(TAKER_SWAP_ENTRY_TIMEOUT_SEC))
                .is_some()
            {
                // There is the same hash already.
                return None;
            }
        }

        Some(SwapWatcherLock {
            swap_ctx,
            fee_hash,
            watcher_type: WatcherType::Taker,
        })
    }
}

impl Drop for SwapWatcherLock {
    fn drop(&mut self) {
        match self.watcher_type {
            WatcherType::Taker => self.swap_ctx.taker_swap_watchers.lock().remove(&self.fee_hash.clone()),
        };
    }
}

fn spawn_taker_swap_watcher(ctx: MmArc, watcher_data: TakerSwapWatcherData, verified_pub: Vec<u8>) {
    // TODO: See if more data validations can be added here
    if watcher_data.lock_duration != get_payment_locktime()
        && watcher_data.lock_duration != get_payment_locktime() * 4
        && watcher_data.lock_duration != get_payment_locktime() * 10
    {
        error!("Invalid lock duration");
        return;
    }

    let swap_ctx = SwapsContext::from_ctx(&ctx).unwrap();
    if swap_ctx.swap_msgs.lock().unwrap().contains_key(&watcher_data.uuid) {
        return;
    }
    let taker_watcher_lock = match SwapWatcherLock::lock_taker(swap_ctx, watcher_data.taker_fee_hash.clone()) {
        Some(lock) => lock,
        // There is an ongoing Taker Swap Watcher already.
        None => return,
    };

    let spawner = ctx.spawner();
    let taker_fee_bytes: [u8; 32] = match watcher_data.taker_fee_hash.as_slice().try_into() {
        Ok(bytes) => bytes,
        Err(_) => {
            error!(
                "Invalid taker fee hash length for {}",
                hex::encode(&watcher_data.taker_fee_hash)
            );
            return;
        },
    };
    let fee_hash = H256Json::from(taker_fee_bytes);

    let fut = async move {
        let taker_coin = match lp_coinfind(&ctx, &watcher_data.taker_coin).await {
            Ok(Some(c)) => c,
            Ok(None) => {
                error!("Coin {} is not found/enabled", watcher_data.taker_coin);
                return;
            },
            Err(e) => {
                error!("!lp_coinfind({}): {}", watcher_data.taker_coin, e);
                return;
            },
        };

        let maker_coin = match lp_coinfind(&ctx, &watcher_data.maker_coin).await {
            Ok(Some(c)) => c,
            Ok(None) => {
                error!("Coin {} is not found/enabled", watcher_data.maker_coin);
                return;
            },
            Err(e) => {
                error!("!lp_coinfind({}): {}", watcher_data.maker_coin, e);
                return;
            },
        };

        if !taker_coin.is_supported_by_watchers() || !maker_coin.is_supported_by_watchers() {
            log!("One of the coins or their contracts does not support watchers");
            return;
        }

        log_tag!(
            ctx,
            "";
            fmt = "Entering the taker swap watcher loop {}/{} with taker fee hash: {}",
            maker_coin.ticker(),
            taker_coin.ticker(),
            fee_hash
        );

        let conf = json::from_value::<WatcherConf>(ctx.conf["watcher_conf"].clone()).unwrap_or_default();
        let watcher_reward = maker_coin.is_eth();
        let mut state_machine = WatcherStateMachine {
            ctx,
            maker_coin,
            taker_coin,
            verified_pub,
            data: watcher_data,
            conf,
            watcher_reward,
        };
        state_machine
            .run(Box::new(ValidateTakerFee {}))
            .await
            .expect("The error of this machine is Infallible");

        // This allows to move the `taker_watcher_lock` value into this async block to keep it alive
        // until the Swap Watcher finishes.
        drop(taker_watcher_lock);
    };

    let settings = AbortSettings::info_on_abort(format!("taker swap watcher {fee_hash} stopped!"));
    // Please note that `taker_watcher_lock` will be dropped once `MmCtx` is stopped
    // since this `fut` will be aborted.
    spawner.spawn_with_settings(fut, settings);
}

pub fn watcher_topic(ticker: &str) -> String {
    pub_sub_topic(WATCHER_PREFIX, ticker)
}
