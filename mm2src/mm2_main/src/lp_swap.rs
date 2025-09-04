//! Atomic swap loops and states
//!
//! # A note on the terminology used
//!
//! Alice = Buyer = Liquidity receiver = Taker
//! ("*The process of an atomic swap begins with the person who makes the initial request — this is the liquidity receiver*" - Komodo Whitepaper).
//!
//! Bob = Seller = Liquidity provider = Market maker
//! ("*On the other side of the atomic swap, we have the liquidity provider — we call this person, Bob*" - Komodo Whitepaper).
//!
//! # Algorithm updates
//!
//! At the end of 2018 most UTXO coins have BIP65 (https://github.com/bitcoin/bips/blob/master/bip-0065.mediawiki).
//! The previous swap protocol discussions took place at 2015-2016 when there were just a few
//! projects that implemented CLTV opcode support:
//! https://bitcointalk.org/index.php?topic=1340621.msg13828271#msg13828271
//! https://bitcointalk.org/index.php?topic=1364951
//! So the Tier Nolan approach is a bit outdated, the main purpose was to allow swapping of a coin
//! that doesn't have CLTV at least as Alice side (as APayment is 2of2 multisig).
//! Nowadays the protocol can be simplified to the following (UTXO coins, BTC and forks):
//!
//! 1. AFee: OP_DUP OP_HASH160 FEE_RMD160 OP_EQUALVERIFY OP_CHECKSIG
//!
//! 2. BPayment:
//!
//! ```
//! OP_IF
//!   <now + LOCKTIME*2> OP_CLTV OP_DROP <bob_pub> OP_CHECKSIG
//! OP_ELSE
//!   OP_SIZE 32 OP_EQUALVERIFY OP_HASH160 <hash(bob_privN)> OP_EQUALVERIFY <alice_pub> OP_CHECKSIG
//! OP_ENDIF
//! ```
//!
//! 3. APayment:
//!
//! ```
//! OP_IF
//!   <now + LOCKTIME> OP_CLTV OP_DROP <alice_pub> OP_CHECKSIG
//! OP_ELSE
//!   OP_SIZE 32 OP_EQUALVERIFY OP_HASH160 <hash(bob_privN)> OP_EQUALVERIFY <bob_pub> OP_CHECKSIG
//! OP_ENDIF
//! ```

/******************************************************************************
 * Copyright © 2023 Pampex LTD and TillyHK LTD                                *
 *                                                                            *
 * See the CONTRIBUTOR-LICENSE-AGREEMENT, COPYING, LICENSE-COPYRIGHT-NOTICE   *
 * and DEVELOPER-CERTIFICATE-OF-ORIGIN files in the LEGAL directory in        *
 * the top-level directory of this distribution for the individual copyright  *
 * holder information and the developer policies on copyright and licensing.  *
 *                                                                            *
 * Unless otherwise agreed in a custom licensing agreement, no part of the    *
 * Komodo DeFi Framework software, including this file may be copied, modified, propagated*
 * or distributed except according to the terms contained in the              *
 * LICENSE-COPYRIGHT-NOTICE file.                                             *
 *                                                                            *
 * Removal or modification of this copyright notice is prohibited.            *
 *                                                                            *
 ******************************************************************************/
//
//  lp_swap.rs
//  marketmaker
//

use super::lp_network::P2PRequestResult;
use crate::lp_network::{broadcast_p2p_msg, Libp2pPeerId, P2PProcessError, P2PProcessResult, P2PRequestError};
use crate::lp_swap::maker_swap_v2::MakerSwapStorage;
use crate::lp_swap::taker_swap_v2::TakerSwapStorage;
use bitcrypto::sha256;
use coins::{lp_coinfind, lp_coinfind_or_err, CoinFindError, MmCoinEnum, TradeFee, TransactionEnum};
use common::log::{debug, warn};
use common::now_sec;
use common::{
    bits256, calc_total_pages,
    executor::{spawn_abortable, AbortOnDropHandle, SpawnFuture, Timer},
    log::{error, info},
    HttpStatusCode, PagingOptions, StatusCode,
};
use derive_more::Display;
use http::Response;
use mm2_core::mm_ctx::{from_ctx, MmArc};
use mm2_err_handle::prelude::*;
use mm2_libp2p::behaviours::atomicdex::MAX_TIME_GAP_FOR_CONNECTED_PEER;
use mm2_libp2p::{decode_signed, encode_and_sign, pub_sub_topic, PeerId, TopicPrefix};
use mm2_number::{BigDecimal, MmNumber, MmNumberMultiRepr};
use mm2_state_machine::storable_state_machine::StateMachineStorage;
use parking_lot::Mutex as PaMutex;
use rpc::v1::types::{Bytes as BytesJson, H256 as H256Json, H264};
use secp256k1::{PublicKey, SecretKey, Signature};
use serde::Serialize;
use serde_json::{self as json, Value as Json};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use timed_map::{MapKind, TimedMap};
use uuid::Uuid;

#[cfg(any(feature = "custom-swap-locktime", test, feature = "run-docker-tests"))]
use std::sync::atomic::{AtomicU64, Ordering};

mod check_balance;
mod maker_swap;
pub mod maker_swap_v2;
mod max_maker_vol_rpc;
mod my_swaps_storage;
mod pubkey_banning;
mod recreate_swap_data;
mod saved_swap;
mod swap_lock;
#[path = "lp_swap/komodefi.swap_v2.pb.rs"]
#[rustfmt::skip]
mod swap_v2_pb;
pub(crate) mod swap_events;
mod swap_v2_common;
pub(crate) mod swap_v2_rpcs;
pub(crate) mod swap_watcher;
pub(crate) mod taker_restart;
pub(crate) mod taker_swap;
pub mod taker_swap_v2;
mod trade_preimage;

#[cfg(target_arch = "wasm32")]
mod swap_wasm_db;

pub use check_balance::{check_other_coin_balance_for_swap, CheckBalanceError, CheckBalanceResult};
use crypto::secret_hash_algo::SecretHashAlgo;
use crypto::CryptoCtx;
use keys::{KeyPair, SECP_SIGN, SECP_VERIFY};
use maker_swap::MakerSwapEvent;
pub use maker_swap::{
    calc_max_maker_vol, check_balance_for_maker_swap, get_max_maker_vol, maker_swap_trade_preimage, run_maker_swap,
    CoinVolumeInfo, MakerSavedEvent, MakerSavedSwap, MakerSwap, MakerSwapStatusChanged, MakerTradePreimage,
    RunMakerSwapInput, MAKER_PAYMENT_SENT_LOG,
};
pub use max_maker_vol_rpc::max_maker_vol;
use my_swaps_storage::{MySwapsOps, MySwapsStorage};
use pubkey_banning::BanReason;
pub use pubkey_banning::{ban_pubkey_rpc, is_pubkey_banned, list_banned_pubkeys_rpc, unban_pubkeys_rpc};
pub use recreate_swap_data::recreate_swap_data;
pub use saved_swap::{SavedSwap, SavedSwapError, SavedSwapIo, SavedSwapResult};
use swap_v2_common::{
    get_unfinished_swaps_uuids, swap_kickstart_handler_for_maker, swap_kickstart_handler_for_taker, ActiveSwapV2Info,
};
use swap_v2_pb::*;
use swap_v2_rpcs::{get_maker_swap_data_for_rpc, get_swap_type, get_taker_swap_data_for_rpc};
pub use swap_watcher::{
    process_watcher_msg, watcher_topic, TakerSwapWatcherData, MAKER_PAYMENT_SPEND_FOUND_LOG,
    MAKER_PAYMENT_SPEND_SENT_LOG, TAKER_PAYMENT_REFUND_SENT_LOG, TAKER_SWAP_ENTRY_TIMEOUT_SEC, WATCHER_PREFIX,
};
use taker_swap::TakerSwapEvent;
pub use taker_swap::{
    calc_max_taker_vol, check_balance_for_taker_swap, create_taker_swap_default_params, max_taker_vol,
    max_taker_vol_from_available, run_taker_swap, taker_swap_trade_preimage, RunTakerSwapInput, TakerSavedSwap,
    TakerSwap, TakerSwapData, TakerSwapPreparedParams, TakerTradePreimage, MAKER_PAYMENT_SPENT_BY_WATCHER_LOG,
    REFUND_TEST_FAILURE_LOG, WATCHER_MESSAGE_SENT_LOG,
};
pub use trade_preimage::trade_preimage_rpc;

pub const SWAP_PREFIX: TopicPrefix = "swap";
pub const SWAP_V2_PREFIX: TopicPrefix = "swapv2";
pub const SWAP_FINISHED_LOG: &str = "Swap finished: ";
pub const TX_HELPER_PREFIX: TopicPrefix = "txhlp";

pub(crate) const LEGACY_SWAP_TYPE: u8 = 0;
pub(crate) const MAKER_SWAP_V2_TYPE: u8 = 1;
pub(crate) const TAKER_SWAP_V2_TYPE: u8 = 2;

pub(crate) const TAKER_FEE_VALIDATION_ATTEMPTS: usize = 6;
pub(crate) const TAKER_FEE_VALIDATION_RETRY_DELAY_SECS: f64 = 10.;

const NEGOTIATE_SEND_INTERVAL: f64 = 30.;

/// If a certain P2P message is not received, swap will be aborted after this time expires.
const NEGOTIATION_TIMEOUT_SEC: u64 = 90;

const MAX_STARTED_AT_DIFF: u64 = MAX_TIME_GAP_FOR_CONNECTED_PEER * 3;

cfg_wasm32! {
    use mm2_db::indexed_db::{ConstructibleDb, DbLocked};
    use saved_swap::migrate_swaps_data;
    use swap_wasm_db::{InitDbResult, InitDbError, SwapDb};

    pub type SwapDbLocked<'a> = DbLocked<'a, SwapDb>;
}

#[derive(Clone, Debug, Eq, Deserialize, PartialEq, Serialize)]
pub enum SwapMsg {
    Negotiation(NegotiationDataMsg),
    NegotiationReply(NegotiationDataMsg),
    Negotiated(bool),
    TakerFee(SwapTxDataMsg),
    MakerPayment(SwapTxDataMsg),
    TakerPayment(Vec<u8>),
}

#[derive(Debug, Default)]
pub struct SwapMsgStore {
    negotiation: Option<NegotiationDataMsg>,
    negotiation_reply: Option<NegotiationDataMsg>,
    negotiated: Option<bool>,
    taker_fee: Option<SwapTxDataMsg>,
    maker_payment: Option<SwapTxDataMsg>,
    taker_payment: Option<Vec<u8>>,
    accept_only_from: bits256,
}

impl SwapMsgStore {
    pub fn new(accept_only_from: bits256) -> Self {
        SwapMsgStore {
            accept_only_from,
            ..Default::default()
        }
    }
}

/// Storage for P2P messages, which are exchanged during SwapV2 protocol execution.
#[derive(Debug)]
pub struct SwapV2MsgStore {
    maker_negotiation: Option<MakerNegotiation>,
    taker_negotiation: Option<TakerNegotiation>,
    maker_negotiated: Option<MakerNegotiated>,
    taker_funding: Option<TakerFundingInfo>,
    maker_payment: Option<MakerPaymentInfo>,
    taker_payment: Option<TakerPaymentInfo>,
    taker_payment_spend_preimage: Option<TakerPaymentSpendPreimage>,
    accept_only_from: PublicKey,
}

impl SwapV2MsgStore {
    /// Creates new SwapV2MsgStore
    pub fn new(accept_only_from: PublicKey) -> Self {
        SwapV2MsgStore {
            maker_negotiation: None,
            taker_negotiation: None,
            maker_negotiated: None,
            taker_funding: None,
            maker_payment: None,
            taker_payment: None,
            taker_payment_spend_preimage: None,
            accept_only_from,
        }
    }
}

/// Returns key-pair for signing P2P messages and an optional `PeerId` if it should be used forcibly
/// instead of local peer ID.
///
/// # Panic
///
/// This function panics if `CryptoCtx` hasn't been initialized yet.
pub fn p2p_keypair_and_peer_id_to_broadcast(ctx: &MmArc, p2p_privkey: Option<&KeyPair>) -> (KeyPair, Option<PeerId>) {
    match p2p_privkey {
        Some(keypair) => (*keypair, Some(keypair.libp2p_peer_id())),
        None => {
            let crypto_ctx = CryptoCtx::from_ctx(ctx).expect("CryptoCtx must be initialized already");
            (*crypto_ctx.mm2_internal_key_pair(), None)
        },
    }
}

/// Returns private key for signing P2P messages and an optional `PeerId` if it should be used forcibly
/// instead of local peer ID.
///
/// # Panic
///
/// This function panics if `CryptoCtx` hasn't been initialized yet.
pub fn p2p_private_and_peer_id_to_broadcast(ctx: &MmArc, p2p_privkey: Option<&KeyPair>) -> ([u8; 32], Option<PeerId>) {
    match p2p_privkey {
        Some(keypair) => (keypair.private_bytes(), Some(keypair.libp2p_peer_id())),
        None => {
            let crypto_ctx = CryptoCtx::from_ctx(ctx).expect("CryptoCtx must be initialized already");
            (crypto_ctx.mm2_internal_privkey_secret().take(), None)
        },
    }
}

/// Spawns the loop that broadcasts message every `interval` seconds returning the AbortOnDropHandle
/// to stop it
pub fn broadcast_swap_msg_every<T: 'static + Serialize + Clone + Send>(
    ctx: MmArc,
    topic: String,
    msg: T,
    interval_sec: f64,
    p2p_privkey: Option<KeyPair>,
) -> AbortOnDropHandle {
    let fut = async move {
        loop {
            broadcast_swap_message(&ctx, topic.clone(), msg.clone(), &p2p_privkey);
            Timer::sleep(interval_sec).await;
        }
    };
    spawn_abortable(fut)
}

/// Spawns the loop that broadcasts message every `interval` seconds returning the AbortOnDropHandle
/// to stop it. This function waits for interval seconds first before starting the broadcast.
pub fn broadcast_swap_msg_every_delayed<T: 'static + Serialize + Clone + Send>(
    ctx: MmArc,
    topic: String,
    msg: T,
    interval_sec: f64,
    p2p_privkey: Option<KeyPair>,
) -> AbortOnDropHandle {
    let fut = async move {
        loop {
            Timer::sleep(interval_sec).await;
            broadcast_swap_message(&ctx, topic.clone(), msg.clone(), &p2p_privkey);
        }
    };
    spawn_abortable(fut)
}

/// Broadcast the swap message once
pub fn broadcast_swap_message<T: Serialize>(ctx: &MmArc, topic: String, msg: T, p2p_privkey: &Option<KeyPair>) {
    let (p2p_private, from) = p2p_private_and_peer_id_to_broadcast(ctx, p2p_privkey.as_ref());
    let encoded_msg = match encode_and_sign(&msg, &p2p_private) {
        Ok(m) => m,
        Err(e) => {
            error!("Error encoding and signing swap message: {}", e);
            return;
        },
    };
    broadcast_p2p_msg(ctx, topic, encoded_msg, from);
}

/// Broadcast the tx message once
pub fn broadcast_p2p_tx_msg(ctx: &MmArc, topic: String, msg: &TransactionEnum, p2p_privkey: &Option<KeyPair>) {
    if !msg.supports_tx_helper() {
        return;
    }

    let (p2p_private, from) = p2p_private_and_peer_id_to_broadcast(ctx, p2p_privkey.as_ref());
    let encoded_msg = match encode_and_sign(&msg.tx_hex(), &p2p_private) {
        Ok(m) => m,
        Err(e) => {
            error!("Error encoding and signing tx message: {}", e);
            return;
        },
    };
    broadcast_p2p_msg(ctx, topic, encoded_msg, from);
}

impl SwapMsg {
    fn swap_msg_to_store(self, msg_store: &mut SwapMsgStore) {
        match self {
            SwapMsg::Negotiation(data) => msg_store.negotiation = Some(data),
            SwapMsg::NegotiationReply(data) => msg_store.negotiation_reply = Some(data),
            SwapMsg::Negotiated(negotiated) => msg_store.negotiated = Some(negotiated),
            SwapMsg::TakerFee(data) => msg_store.taker_fee = Some(data),
            SwapMsg::MakerPayment(data) => msg_store.maker_payment = Some(data),
            SwapMsg::TakerPayment(taker_payment) => msg_store.taker_payment = Some(taker_payment),
        }
    }
}

pub async fn process_swap_msg(ctx: MmArc, topic: &str, msg: &[u8]) -> P2PRequestResult<()> {
    let uuid = Uuid::from_str(topic).map_to_mm(|e| P2PRequestError::DecodeError(e.to_string()))?;

    let msg = match decode_signed::<SwapMsg>(msg) {
        Ok(m) => m,
        Err(swap_msg_err) => {
            #[cfg(not(target_arch = "wasm32"))]
            return match json::from_slice::<SwapStatus>(msg) {
                Ok(mut status) => {
                    status.data.fetch_and_set_usd_prices().await;
                    if let Err(e) = save_stats_swap(&ctx, &status.data).await {
                        error!("Error saving the swap {} status: {}", status.data.uuid(), e);
                    }
                    Ok(())
                },
                Err(swap_status_err) => {
                    let error = format!(
                        "Couldn't deserialize swap msg to either 'SwapMsg': {swap_msg_err} or to 'SwapStatus': {swap_status_err}"
                    );
                    MmError::err(P2PRequestError::DecodeError(error))
                },
            };

            #[cfg(target_arch = "wasm32")]
            return MmError::err(P2PRequestError::DecodeError(format!(
                "Couldn't deserialize 'SwapMsg': {swap_msg_err}"
            )));
        },
    };

    debug!("Processing swap msg {msg:?} for uuid {uuid}");
    let swap_ctx = SwapsContext::from_ctx(&ctx).unwrap();
    let mut msgs = swap_ctx.swap_msgs.lock().unwrap();
    if let Some(msg_store) = msgs.get_mut(&uuid) {
        if msg_store.accept_only_from.bytes == msg.2.unprefixed() {
            msg.0.swap_msg_to_store(msg_store);
        } else {
            warn!("Received message from unexpected sender for swap {uuid}");
        }
    };

    Ok(())
}

pub fn swap_topic(uuid: &Uuid) -> String {
    pub_sub_topic(SWAP_PREFIX, &uuid.to_string())
}

/// Formats and returns a topic format for `txhlp`.
///
/// # Usage
/// ```ignore
/// let topic = tx_helper_topic("BTC");
/// // Returns topic format `txhlp/BTC` as String type.
/// ```
#[inline(always)]
pub fn tx_helper_topic(coin: &str) -> String {
    pub_sub_topic(TX_HELPER_PREFIX, coin)
}

async fn recv_swap_msg<T>(
    ctx: MmArc,
    mut getter: impl FnMut(&mut SwapMsgStore) -> Option<T>,
    uuid: &Uuid,
    timeout: u64,
) -> Result<T, String> {
    let started = now_sec();
    let timeout = BASIC_COMM_TIMEOUT + timeout;
    let wait_until = started + timeout;
    loop {
        Timer::sleep(1.).await;
        let swap_ctx = SwapsContext::from_ctx(&ctx).unwrap();
        let mut msgs = swap_ctx.swap_msgs.lock().unwrap();
        if let Some(msg_store) = msgs.get_mut(uuid) {
            if let Some(msg) = getter(msg_store) {
                return Ok(msg);
            }
        }
        let now = now_sec();
        if now > wait_until {
            return ERR!("Timeout ({} > {})", now - started, timeout);
        }
    }
}

/// Includes the grace time we add to the "normal" timeouts
/// in order to give different and/or heavy communication channels a chance.
const BASIC_COMM_TIMEOUT: u64 = 90;

#[cfg(not(any(feature = "custom-swap-locktime", test, feature = "run-docker-tests")))]
/// Default atomic swap payment locktime, in seconds.
/// Maker sends payment with LOCKTIME * 2
/// Taker sends payment with LOCKTIME
const PAYMENT_LOCKTIME: u64 = 3600 * 2 + 300 * 2;

#[cfg(any(feature = "custom-swap-locktime", test, feature = "run-docker-tests"))]
/// Default atomic swap payment locktime, in seconds.
/// Maker sends payment with LOCKTIME * 2
/// Taker sends payment with LOCKTIME
pub(crate) static PAYMENT_LOCKTIME: AtomicU64 = AtomicU64::new(super::CUSTOM_PAYMENT_LOCKTIME_DEFAULT);

#[inline]
/// Returns `PAYMENT_LOCKTIME`
pub fn get_payment_locktime() -> u64 {
    #[cfg(not(any(feature = "custom-swap-locktime", test, feature = "run-docker-tests")))]
    return PAYMENT_LOCKTIME;
    #[cfg(any(feature = "custom-swap-locktime", test, feature = "run-docker-tests"))]
    PAYMENT_LOCKTIME.load(Ordering::Relaxed)
}

#[inline]
pub fn taker_payment_spend_duration(locktime: u64) -> u64 {
    (locktime * 4) / 5
}

#[inline]
pub fn taker_payment_spend_deadline(swap_started_at: u64, locktime: u64) -> u64 {
    swap_started_at + taker_payment_spend_duration(locktime)
}

#[inline]
pub fn wait_for_maker_payment_conf_duration(locktime: u64) -> u64 {
    (locktime * 2) / 5
}

#[inline]
pub fn wait_for_maker_payment_conf_until(swap_started_at: u64, locktime: u64) -> u64 {
    swap_started_at + wait_for_maker_payment_conf_duration(locktime)
}

const _SWAP_DEFAULT_NUM_CONFIRMS: u32 = 1;
const _SWAP_DEFAULT_MAX_CONFIRMS: u32 = 6;
/// MM2 checks that swap payment is confirmed every WAIT_CONFIRM_INTERVAL seconds
const WAIT_CONFIRM_INTERVAL_SEC: u64 = 15;

#[derive(Debug, PartialEq, Serialize)]
pub enum RecoveredSwapAction {
    RefundedMyPayment,
    SpentOtherPayment,
}

#[derive(Debug, PartialEq)]
pub struct RecoveredSwap {
    action: RecoveredSwapAction,
    coin: String,
    transaction: TransactionEnum,
}

/// Represents the amount of a coin locked by ongoing swap
#[derive(Debug)]
pub struct LockedAmount {
    coin: String,
    amount: MmNumber,
    trade_fee: Option<TradeFee>,
}

pub trait AtomicSwap: Send + Sync {
    fn locked_amount(&self) -> Vec<LockedAmount>;

    fn uuid(&self) -> &Uuid;

    fn maker_coin(&self) -> &str;

    fn taker_coin(&self) -> &str;

    fn unique_swap_data(&self) -> Vec<u8>;
}

#[derive(Serialize)]
#[serde(tag = "type", content = "event")]
pub enum SwapEvent {
    Maker(MakerSwapEvent),
    Taker(TakerSwapEvent),
}

impl From<MakerSwapEvent> for SwapEvent {
    fn from(maker_event: MakerSwapEvent) -> Self {
        SwapEvent::Maker(maker_event)
    }
}

impl From<TakerSwapEvent> for SwapEvent {
    fn from(taker_event: TakerSwapEvent) -> Self {
        SwapEvent::Taker(taker_event)
    }
}

struct LockedAmountInfo {
    swap_uuid: Uuid,
    locked_amount: LockedAmount,
}

struct SwapsContext {
    running_swaps: Mutex<HashMap<Uuid, Arc<dyn AtomicSwap>>>,
    active_swaps_v2_infos: Mutex<HashMap<Uuid, ActiveSwapV2Info>>,
    banned_pubkeys: Mutex<TimedMap<H256Json, BanReason>>,
    swap_msgs: Mutex<HashMap<Uuid, SwapMsgStore>>,
    swap_v2_msgs: Mutex<HashMap<Uuid, SwapV2MsgStore>>,
    taker_swap_watchers: PaMutex<TimedMap<Vec<u8>, ()>>,
    locked_amounts: Mutex<HashMap<String, Vec<LockedAmountInfo>>>,
    #[cfg(target_arch = "wasm32")]
    swap_db: ConstructibleDb<SwapDb>,
}

impl SwapsContext {
    /// Obtains a reference to this crate context, creating it if necessary.
    fn from_ctx(ctx: &MmArc) -> Result<Arc<SwapsContext>, String> {
        Ok(try_s!(from_ctx(&ctx.swaps_ctx, move || {
            Ok(SwapsContext {
                running_swaps: Mutex::new(HashMap::new()),
                active_swaps_v2_infos: Mutex::new(HashMap::new()),
                banned_pubkeys: Mutex::new(TimedMap::new_with_map_kind(MapKind::FxHashMap)),
                swap_msgs: Mutex::new(HashMap::new()),
                swap_v2_msgs: Mutex::new(HashMap::new()),
                taker_swap_watchers: PaMutex::new(TimedMap::new_with_map_kind(MapKind::FxHashMap)),
                locked_amounts: Mutex::new(HashMap::new()),
                #[cfg(target_arch = "wasm32")]
                swap_db: ConstructibleDb::new(ctx),
            })
        })))
    }

    pub fn init_msg_store(&self, uuid: Uuid, accept_only_from: bits256) {
        let store = SwapMsgStore::new(accept_only_from);
        self.swap_msgs.lock().unwrap().insert(uuid, store);
    }

    /// Initializes storage for the swap with specific uuid.
    pub fn init_msg_v2_store(&self, uuid: Uuid, accept_only_from: PublicKey) {
        let store = SwapV2MsgStore::new(accept_only_from);
        self.swap_v2_msgs.lock().unwrap().insert(uuid, store);
    }

    /// Removes storage for the swap with specific uuid.
    pub fn remove_msg_v2_store(&self, uuid: &Uuid) {
        self.swap_v2_msgs.lock().unwrap().remove(uuid);
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn swap_db(&self) -> InitDbResult<SwapDbLocked<'_>> {
        self.swap_db.get_or_initialize().await
    }
}

#[derive(Debug, Deserialize)]
pub struct GetLockedAmountReq {
    coin: String,
}

#[derive(Serialize)]
pub struct GetLockedAmountResp {
    coin: String,
    locked_amount: MmNumberMultiRepr,
}

#[derive(Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum GetLockedAmountRpcError {
    #[display(fmt = "No such coin: {coin}")]
    NoSuchCoin { coin: String },
}

impl HttpStatusCode for GetLockedAmountRpcError {
    fn status_code(&self) -> StatusCode {
        match self {
            GetLockedAmountRpcError::NoSuchCoin { .. } => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<CoinFindError> for GetLockedAmountRpcError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => GetLockedAmountRpcError::NoSuchCoin { coin },
        }
    }
}

pub async fn get_locked_amount_rpc(
    ctx: MmArc,
    req: GetLockedAmountReq,
) -> Result<GetLockedAmountResp, MmError<GetLockedAmountRpcError>> {
    lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;
    let locked_amount = get_locked_amount(&ctx, &req.coin);

    Ok(GetLockedAmountResp {
        coin: req.coin,
        locked_amount: locked_amount.into(),
    })
}

/// Get total amount of selected coin locked by all currently ongoing swaps
pub fn get_locked_amount(ctx: &MmArc, coin: &str) -> MmNumber {
    let swap_ctx = SwapsContext::from_ctx(ctx).unwrap();
    let swap_lock = swap_ctx.running_swaps.lock().unwrap();

    let mut locked =
        swap_lock
            .values()
            .flat_map(|swap| swap.locked_amount())
            .fold(MmNumber::from(0), |mut total_amount, locked| {
                if locked.coin == coin {
                    total_amount += locked.amount;
                }
                if let Some(trade_fee) = locked.trade_fee {
                    if trade_fee.coin == coin && !trade_fee.paid_from_trading_vol {
                        total_amount += trade_fee.amount;
                    }
                }
                total_amount
            });
    drop(swap_lock);

    let locked_amounts = swap_ctx.locked_amounts.lock().unwrap();
    if let Some(locked_for_coin) = locked_amounts.get(coin) {
        locked += locked_for_coin
            .iter()
            .fold(MmNumber::from(0), |mut total_amount, locked| {
                total_amount += &locked.locked_amount.amount;
                if let Some(trade_fee) = &locked.locked_amount.trade_fee {
                    if trade_fee.coin == coin && !trade_fee.paid_from_trading_vol {
                        total_amount += &trade_fee.amount;
                    }
                }
                total_amount
            });
    }

    locked
}

/// Clear up all the running swaps.
///
/// This doesn't mean that these swaps will be stopped. They can only be stopped from the abortable systems they are running on top of.
pub fn clear_running_swaps(ctx: &MmArc) {
    let swap_ctx = SwapsContext::from_ctx(ctx).unwrap();
    swap_ctx.running_swaps.lock().unwrap().clear();
}

/// Get total amount of selected coin locked by all currently ongoing swaps except the one with selected uuid
fn get_locked_amount_by_other_swaps(ctx: &MmArc, except_uuid: &Uuid, coin: &str) -> MmNumber {
    let swap_ctx = SwapsContext::from_ctx(ctx).unwrap();
    let swap_lock = swap_ctx.running_swaps.lock().unwrap();

    swap_lock
        .values()
        .filter(|swap| swap.uuid() != except_uuid)
        .flat_map(|swap| swap.locked_amount())
        .fold(MmNumber::from(0), |mut total_amount, locked| {
            if locked.coin == coin {
                total_amount += locked.amount;
            }
            if let Some(trade_fee) = locked.trade_fee {
                if trade_fee.coin == coin && !trade_fee.paid_from_trading_vol {
                    total_amount += trade_fee.amount;
                }
            }
            total_amount
        })
}

pub fn active_swaps_using_coins(ctx: &MmArc, coins: &HashSet<String>) -> Result<Vec<Uuid>, String> {
    let swap_ctx = try_s!(SwapsContext::from_ctx(ctx));
    let swaps = try_s!(swap_ctx.running_swaps.lock());
    let mut uuids = vec![];
    for swap in swaps.values() {
        if coins.contains(&swap.maker_coin().to_string()) || coins.contains(&swap.taker_coin().to_string()) {
            uuids.push(*swap.uuid())
        }
    }
    drop(swaps);

    let swaps_v2 = try_s!(swap_ctx.active_swaps_v2_infos.lock());
    for (uuid, info) in swaps_v2.iter() {
        if coins.contains(&info.maker_coin) || coins.contains(&info.taker_coin) {
            uuids.push(*uuid);
        }
    }
    Ok(uuids)
}

pub fn active_swaps(ctx: &MmArc) -> Result<Vec<(Uuid, u8)>, String> {
    let swap_ctx = try_s!(SwapsContext::from_ctx(ctx));
    let mut uuids: Vec<_> = swap_ctx
        .running_swaps
        .lock()
        .unwrap()
        .keys()
        .map(|uuid| (*uuid, LEGACY_SWAP_TYPE))
        .collect();

    let swaps_v2 = swap_ctx.active_swaps_v2_infos.lock().unwrap();
    uuids.extend(swaps_v2.iter().map(|(uuid, info)| (*uuid, info.swap_type)));
    Ok(uuids)
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
pub struct SwapConfirmationsSettings {
    pub maker_coin_confs: u64,
    pub maker_coin_nota: bool,
    pub taker_coin_confs: u64,
    pub taker_coin_nota: bool,
}

impl SwapConfirmationsSettings {
    pub fn requires_notarization(&self) -> bool {
        self.maker_coin_nota || self.taker_coin_nota
    }
}

fn coin_with_4x_locktime(ticker: &str) -> bool {
    matches!(ticker, "BCH" | "BTG" | "SBTC")
}

#[derive(Debug)]
pub enum AtomicLocktimeVersion {
    V1,
    V2 {
        my_conf_settings: SwapConfirmationsSettings,
        other_conf_settings: SwapConfirmationsSettings,
    },
}

pub fn lp_atomic_locktime_v1(maker_coin: &str, taker_coin: &str) -> u64 {
    if maker_coin == "BTC" || taker_coin == "BTC" {
        get_payment_locktime() * 10
    } else if coin_with_4x_locktime(maker_coin) || coin_with_4x_locktime(taker_coin) {
        get_payment_locktime() * 4
    } else {
        get_payment_locktime()
    }
}

pub fn lp_atomic_locktime_v2(
    maker_coin: &str,
    taker_coin: &str,
    my_conf_settings: &SwapConfirmationsSettings,
    other_conf_settings: &SwapConfirmationsSettings,
) -> u64 {
    if taker_coin.contains("-lightning") {
        // A good value for lightning taker locktime is about 24 hours to find a good 3 hop or less path for the payment
        get_payment_locktime() * 12
    } else if maker_coin == "BTC"
        || taker_coin == "BTC"
        || coin_with_4x_locktime(maker_coin)
        || coin_with_4x_locktime(taker_coin)
        || my_conf_settings.requires_notarization()
        || other_conf_settings.requires_notarization()
    {
        get_payment_locktime() * 4
    } else {
        get_payment_locktime()
    }
}

/// Some coins are "slow" (block time is high - e.g. BTC average block time is ~10 minutes).
/// https://bitinfocharts.com/comparison/bitcoin-confirmationtime.html
/// We need to increase payment locktime accordingly when at least 1 side of swap uses "slow" coin.
pub fn lp_atomic_locktime(maker_coin: &str, taker_coin: &str, version: AtomicLocktimeVersion) -> u64 {
    match version {
        AtomicLocktimeVersion::V1 => lp_atomic_locktime_v1(maker_coin, taker_coin),
        AtomicLocktimeVersion::V2 {
            my_conf_settings,
            other_conf_settings,
        } => lp_atomic_locktime_v2(maker_coin, taker_coin, &my_conf_settings, &other_conf_settings),
    }
}

#[derive(Clone, Debug, Eq, Deserialize, PartialEq, Serialize)]
pub struct NegotiationDataV1 {
    started_at: u64,
    payment_locktime: u64,
    secret_hash: [u8; 20],
    #[serde(
        deserialize_with = "H264::deserialize_from_bytes",
        serialize_with = "H264::serialize_to_byte_seq"
    )]
    persistent_pubkey: H264,
}

#[derive(Clone, Debug, Eq, Deserialize, PartialEq, Serialize)]
pub struct NegotiationDataV2 {
    started_at: u64,
    payment_locktime: u64,
    secret_hash: Vec<u8>,
    #[serde(
        deserialize_with = "H264::deserialize_from_bytes",
        serialize_with = "H264::serialize_to_byte_seq"
    )]
    persistent_pubkey: H264,
    maker_coin_swap_contract: Vec<u8>,
    taker_coin_swap_contract: Vec<u8>,
}

#[derive(Clone, Debug, Eq, Deserialize, PartialEq, Serialize)]
pub struct NegotiationDataV3 {
    started_at: u64,
    payment_locktime: u64,
    secret_hash: Vec<u8>,
    maker_coin_swap_contract: Vec<u8>,
    taker_coin_swap_contract: Vec<u8>,
    #[serde(
        deserialize_with = "H264::deserialize_from_bytes",
        serialize_with = "H264::serialize_to_byte_seq"
    )]
    maker_coin_htlc_pub: H264,
    #[serde(
        deserialize_with = "H264::deserialize_from_bytes",
        serialize_with = "H264::serialize_to_byte_seq"
    )]
    taker_coin_htlc_pub: H264,
}

#[derive(Clone, Debug, Eq, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum NegotiationDataMsg {
    V1(NegotiationDataV1),
    V2(NegotiationDataV2),
    V3(NegotiationDataV3),
}

impl NegotiationDataMsg {
    pub fn started_at(&self) -> u64 {
        match self {
            NegotiationDataMsg::V1(v1) => v1.started_at,
            NegotiationDataMsg::V2(v2) => v2.started_at,
            NegotiationDataMsg::V3(v3) => v3.started_at,
        }
    }

    pub fn payment_locktime(&self) -> u64 {
        match self {
            NegotiationDataMsg::V1(v1) => v1.payment_locktime,
            NegotiationDataMsg::V2(v2) => v2.payment_locktime,
            NegotiationDataMsg::V3(v3) => v3.payment_locktime,
        }
    }

    pub fn secret_hash(&self) -> &[u8] {
        match self {
            NegotiationDataMsg::V1(v1) => &v1.secret_hash,
            NegotiationDataMsg::V2(v2) => &v2.secret_hash,
            NegotiationDataMsg::V3(v3) => &v3.secret_hash,
        }
    }

    pub fn maker_coin_htlc_pub(&self) -> &H264 {
        match self {
            NegotiationDataMsg::V1(v1) => &v1.persistent_pubkey,
            NegotiationDataMsg::V2(v2) => &v2.persistent_pubkey,
            NegotiationDataMsg::V3(v3) => &v3.maker_coin_htlc_pub,
        }
    }

    pub fn taker_coin_htlc_pub(&self) -> &H264 {
        match self {
            NegotiationDataMsg::V1(v1) => &v1.persistent_pubkey,
            NegotiationDataMsg::V2(v2) => &v2.persistent_pubkey,
            NegotiationDataMsg::V3(v3) => &v3.taker_coin_htlc_pub,
        }
    }

    pub fn maker_coin_swap_contract(&self) -> Option<&[u8]> {
        match self {
            NegotiationDataMsg::V1(_) => None,
            NegotiationDataMsg::V2(v2) => Some(&v2.maker_coin_swap_contract),
            NegotiationDataMsg::V3(v3) => Some(&v3.maker_coin_swap_contract),
        }
    }

    pub fn taker_coin_swap_contract(&self) -> Option<&[u8]> {
        match self {
            NegotiationDataMsg::V1(_) => None,
            NegotiationDataMsg::V2(v2) => Some(&v2.taker_coin_swap_contract),
            NegotiationDataMsg::V3(v3) => Some(&v3.taker_coin_swap_contract),
        }
    }
}

#[derive(Clone, Debug, Eq, Deserialize, PartialEq, Serialize)]
pub struct PaymentWithInstructions {
    data: Vec<u8>,
    // Next step instructions for the other side whether taker or maker.
    // An example for this is a maker/taker sending the taker/maker a lightning invoice to be payed.
    next_step_instructions: Vec<u8>,
}

#[derive(Clone, Debug, Eq, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum SwapTxDataMsg {
    Regular(Vec<u8>),
    WithInstructions(PaymentWithInstructions),
}

impl SwapTxDataMsg {
    #[inline]
    pub fn data(&self) -> &[u8] {
        match self {
            SwapTxDataMsg::Regular(data) => data,
            SwapTxDataMsg::WithInstructions(p) => &p.data,
        }
    }

    #[inline]
    pub fn instructions(&self) -> Option<&[u8]> {
        match self {
            SwapTxDataMsg::Regular(_) => None,
            SwapTxDataMsg::WithInstructions(p) => Some(&p.next_step_instructions),
        }
    }

    #[inline]
    pub fn new(data: Vec<u8>, instructions: Option<Vec<u8>>) -> Self {
        match instructions {
            Some(next_step_instructions) => SwapTxDataMsg::WithInstructions(PaymentWithInstructions {
                data,
                next_step_instructions,
            }),
            None => SwapTxDataMsg::Regular(data),
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TransactionIdentifier {
    /// Raw bytes of signed transaction in hexadecimal string, this should be sent as is to send_raw_transaction RPC to broadcast the transaction.
    /// Some payments like lightning payments don't have a tx_hex, for such payments tx_hex will be equal to tx_hash.
    tx_hex: BytesJson,
    /// Transaction hash in hexadecimal format
    tx_hash: BytesJson,
}

#[cfg(not(target_arch = "wasm32"))]
pub fn my_swaps_dir(ctx: &MmArc, address: &str) -> PathBuf {
    ctx.address_dir(address).join("SWAPS").join("MY")
}

#[cfg(not(target_arch = "wasm32"))]
pub fn my_swap_file_path(ctx: &MmArc, address: &str, uuid: &Uuid) -> PathBuf {
    my_swaps_dir(ctx, address).join(format!("{uuid}.json"))
}

pub async fn insert_new_swap_to_db(
    ctx: MmArc,
    my_coin: &str,
    other_coin: &str,
    uuid: Uuid,
    started_at: u64,
    swap_type: u8,
) -> Result<(), String> {
    MySwapsStorage::new(ctx)
        .save_new_swap(my_coin, other_coin, uuid, started_at, swap_type)
        .await
        .map_err(|e| ERRL!("{}", e))
}

#[cfg(not(target_arch = "wasm32"))]
fn add_swap_to_db_index(ctx: &MmArc, swap: &SavedSwap) {
    if let Some(conn) = ctx.sqlite_conn_opt() {
        crate::database::stats_swaps::add_swap_to_index(&conn, swap)
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn save_stats_swap(ctx: &MmArc, swap: &SavedSwap) -> Result<(), String> {
    try_s!(swap.save_to_stats_db(ctx).await);
    add_swap_to_db_index(ctx, swap);
    Ok(())
}

/// The helper structure that makes easier to parse the response for GUI devs
/// They won't have to parse the events themselves handling possible errors, index out of bounds etc.
#[derive(Debug, Serialize, Deserialize)]
pub struct MySwapInfo {
    pub my_coin: String,
    pub other_coin: String,
    pub my_amount: BigDecimal,
    pub other_amount: BigDecimal,
    pub started_at: u64,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
pub struct SavedTradeFee {
    coin: String,
    amount: BigDecimal,
    #[serde(default)]
    paid_from_trading_vol: bool,
}

impl From<SavedTradeFee> for TradeFee {
    fn from(orig: SavedTradeFee) -> Self {
        // used to calculate locked amount so paid_from_trading_vol doesn't matter here
        TradeFee {
            coin: orig.coin,
            amount: orig.amount.into(),
            paid_from_trading_vol: orig.paid_from_trading_vol,
        }
    }
}

impl From<TradeFee> for SavedTradeFee {
    fn from(orig: TradeFee) -> Self {
        SavedTradeFee {
            coin: orig.coin,
            amount: orig.amount.into(),
            paid_from_trading_vol: orig.paid_from_trading_vol,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub struct SwapError {
    error: String,
}

impl From<String> for SwapError {
    fn from(error: String) -> Self {
        SwapError { error }
    }
}

impl From<&str> for SwapError {
    fn from(e: &str) -> Self {
        SwapError { error: e.to_owned() }
    }
}

#[derive(Serialize)]
struct MySwapStatusResponse {
    #[serde(flatten)]
    swap: SavedSwap,
    my_info: Option<MySwapInfo>,
    recoverable: bool,
    is_finished: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_success: Option<bool>,
}

impl From<SavedSwap> for MySwapStatusResponse {
    fn from(mut swap: SavedSwap) -> MySwapStatusResponse {
        swap.hide_secrets();
        MySwapStatusResponse {
            my_info: swap.get_my_info(),
            recoverable: swap.is_recoverable(),
            is_finished: swap.is_finished(),
            // only serialize is_success field if swap is successful
            is_success: swap.is_success().ok(),
            swap,
        }
    }
}

/// Returns the status of swap performed on `my` node
pub async fn my_swap_status(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let uuid: Uuid = try_s!(json::from_value(req["params"]["uuid"].clone()));
    let swap_type = try_s!(get_swap_type(&ctx, &uuid).await);

    match swap_type {
        Some(LEGACY_SWAP_TYPE) => {
            let status = match SavedSwap::load_my_swap_from_db(&ctx, None, uuid).await {
                Ok(Some(status)) => status,
                Ok(None) => return Err("swap data is not found".to_owned()),
                Err(e) => return ERR!("{}", e),
            };

            let res_js = json!({ "result": MySwapStatusResponse::from(status) });
            let res = try_s!(json::to_vec(&res_js));
            Ok(try_s!(Response::builder().body(res)))
        },
        Some(MAKER_SWAP_V2_TYPE) => {
            let swap_data = try_s!(get_maker_swap_data_for_rpc(&ctx, &uuid).await);
            let res_js = json!({ "result": swap_data });
            let res = try_s!(json::to_vec(&res_js));
            Ok(try_s!(Response::builder().body(res)))
        },
        Some(TAKER_SWAP_V2_TYPE) => {
            let swap_data = try_s!(get_taker_swap_data_for_rpc(&ctx, &uuid).await);
            let res_js = json!({ "result": swap_data });
            let res = try_s!(json::to_vec(&res_js));
            Ok(try_s!(Response::builder().body(res)))
        },
        Some(unsupported_type) => ERR!("Got unsupported swap type from DB: {}", unsupported_type),
        None => ERR!("No swap with uuid {}", uuid),
    }
}

#[cfg(target_arch = "wasm32")]
pub async fn stats_swap_status(_ctx: MmArc, _req: Json) -> Result<Response<Vec<u8>>, String> {
    ERR!("'stats_swap_status' is only supported in native mode")
}

/// Returns the status of requested swap, typically performed by other nodes and saved by `save_stats_swap_status`
#[cfg(not(target_arch = "wasm32"))]
pub async fn stats_swap_status(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let uuid: Uuid = try_s!(json::from_value(req["params"]["uuid"].clone()));

    let maker_status = try_s!(SavedSwap::load_from_maker_stats_db(&ctx, uuid).await);
    let taker_status = try_s!(SavedSwap::load_from_taker_stats_db(&ctx, uuid).await);

    if maker_status.is_none() && taker_status.is_none() {
        return ERR!("swap data is not found");
    }

    let res_js = json!({
        "result": {
            "maker": maker_status,
            "taker": taker_status,
        }
    });
    let res = try_s!(json::to_vec(&res_js));
    Ok(try_s!(Response::builder().body(res)))
}

#[derive(Debug, Deserialize, Serialize)]
struct SwapStatus {
    method: String,
    data: SavedSwap,
}

/// Broadcasts `my` swap status to P2P network
async fn broadcast_my_swap_status(ctx: &MmArc, uuid: Uuid) -> Result<(), String> {
    let mut status = match try_s!(SavedSwap::load_my_swap_from_db(ctx, None, uuid).await) {
        Some(status) => status,
        None => return ERR!("swap data is not found"),
    };
    status.hide_secrets();

    #[cfg(not(target_arch = "wasm32"))]
    try_s!(save_stats_swap(ctx, &status).await);

    let status = SwapStatus {
        method: "swapstatus".into(),
        data: status,
    };
    let msg = json::to_vec(&status).expect("Swap status ser should never fail");
    broadcast_p2p_msg(ctx, swap_topic(&uuid), msg, None);
    Ok(())
}

#[derive(Debug, Deserialize)]
pub struct MySwapsFilter {
    pub my_coin: Option<String>,
    pub other_coin: Option<String>,
    pub from_timestamp: Option<u64>,
    pub to_timestamp: Option<u64>,
}

// TODO: Should return the result from SQL like in order history. So it can be clear the exact started_at time
// and the coins if they are not included in the filter request
/// Returns *all* uuids of swaps, which match the selected filter.
pub async fn all_swaps_uuids_by_filter(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let filter: MySwapsFilter = try_s!(json::from_value(req));
    let db_result = try_s!(
        MySwapsStorage::new(ctx)
            .my_recent_swaps_with_filters(&filter, None)
            .await
    );

    let res_js = json!({
        "result": {
            "found_records": db_result.uuids_and_types.len(),
            "uuids": db_result.uuids_and_types.into_iter().map(|(uuid, _)| uuid).collect::<Vec<_>>(),
            "my_coin": filter.my_coin,
            "other_coin": filter.other_coin,
            "from_timestamp": filter.from_timestamp,
            "to_timestamp": filter.to_timestamp,
        },
    });
    let res = try_s!(json::to_vec(&res_js));
    Ok(try_s!(Response::builder().body(res)))
}

#[derive(Debug, Deserialize)]
pub struct MyRecentSwapsReq {
    #[serde(flatten)]
    pub paging_options: PagingOptions,
    #[serde(flatten)]
    pub filter: MySwapsFilter,
}

#[derive(Debug, Default, PartialEq)]
pub struct MyRecentSwapsUuids {
    /// UUIDs and types of swaps matching the query
    pub uuids_and_types: Vec<(Uuid, u8)>,
    /// Total count of swaps matching the query
    pub total_count: usize,
    /// The number of skipped UUIDs
    pub skipped: usize,
}

#[derive(Debug, Display, Deserialize, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum LatestSwapsErr {
    #[display(fmt = "No such swap with the uuid '{_0}'")]
    UUIDNotPresentInDb(Uuid),
    UnableToLoadSavedSwaps(SavedSwapError),
    #[display(fmt = "Unable to query swaps storage")]
    UnableToQuerySwapStorage,
}

pub async fn latest_swaps_for_pair(
    ctx: MmArc,
    my_coin: String,
    other_coin: String,
    limit: usize,
) -> Result<Vec<SavedSwap>, MmError<LatestSwapsErr>> {
    let filter = MySwapsFilter {
        my_coin: Some(my_coin),
        other_coin: Some(other_coin),
        from_timestamp: None,
        to_timestamp: None,
    };

    let paging_options = PagingOptions {
        limit,
        page_number: NonZeroUsize::new(1).expect("1 > 0"),
        from_uuid: None,
    };

    let db_result = match MySwapsStorage::new(ctx.clone())
        .my_recent_swaps_with_filters(&filter, Some(&paging_options))
        .await
    {
        Ok(x) => x,
        Err(_) => return Err(MmError::new(LatestSwapsErr::UnableToQuerySwapStorage)),
    };

    let mut swaps = Vec::with_capacity(db_result.uuids_and_types.len());
    // TODO this is needed for trading bot, which seems not used as of now. Remove the code?
    for (uuid, _) in db_result.uuids_and_types.iter() {
        let swap = match SavedSwap::load_my_swap_from_db(&ctx, None, *uuid).await {
            Ok(Some(swap)) => swap,
            Ok(None) => {
                error!("No such swap with the uuid '{}'", uuid);
                continue;
            },
            Err(e) => return Err(MmError::new(LatestSwapsErr::UnableToLoadSavedSwaps(e.into_inner()))),
        };
        swaps.push(swap);
    }

    Ok(swaps)
}

/// Returns the data of recent swaps of `my` node.
pub async fn my_recent_swaps_rpc(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let req: MyRecentSwapsReq = try_s!(json::from_value(req));
    let db_result = try_s!(
        MySwapsStorage::new(ctx.clone())
            .my_recent_swaps_with_filters(&req.filter, Some(&req.paging_options))
            .await
    );

    // iterate over uuids trying to parse the corresponding files content and add to result vector
    let mut swaps = Vec::with_capacity(db_result.uuids_and_types.len());
    for (uuid, swap_type) in db_result.uuids_and_types.iter() {
        match *swap_type {
            LEGACY_SWAP_TYPE => match SavedSwap::load_my_swap_from_db(&ctx, None, *uuid).await {
                Ok(Some(swap)) => {
                    let swap_json = try_s!(json::to_value(MySwapStatusResponse::from(swap)));
                    swaps.push(swap_json)
                },
                Ok(None) => warn!("No such swap with the uuid '{}'", uuid),
                Err(e) => error!("Error loading a swap with the uuid '{}': {}", uuid, e),
            },
            MAKER_SWAP_V2_TYPE => match get_maker_swap_data_for_rpc(&ctx, uuid).await {
                Ok(data) => {
                    let swap_json = try_s!(json::to_value(data));
                    swaps.push(swap_json);
                },
                Err(e) => error!("Error loading a swap with the uuid '{}': {}", uuid, e),
            },
            TAKER_SWAP_V2_TYPE => match get_taker_swap_data_for_rpc(&ctx, uuid).await {
                Ok(data) => {
                    let swap_json = try_s!(json::to_value(data));
                    swaps.push(swap_json);
                },
                Err(e) => error!("Error loading a swap with the uuid '{}': {}", uuid, e),
            },
            unknown_type => error!("Swap with the uuid '{}' has unknown type {}", uuid, unknown_type),
        }
    }

    let res_js = json!({
        "result": {
            "swaps": swaps,
            "from_uuid": req.paging_options.from_uuid,
            "skipped": db_result.skipped,
            "limit": req.paging_options.limit,
            "total": db_result.total_count,
            "page_number": req.paging_options.page_number,
            "total_pages": calc_total_pages(db_result.total_count, req.paging_options.limit),
            "found_records": db_result.uuids_and_types.len(),
        },
    });
    let res = try_s!(json::to_vec(&res_js));
    Ok(try_s!(Response::builder().body(res)))
}

/// Find out the swaps that need to be kick-started, continue from the point where swap was interrupted
/// Return the tickers of coins that must be enabled for swaps to continue
pub async fn swap_kick_starts(ctx: MmArc) -> Result<HashSet<String>, String> {
    #[cfg(target_arch = "wasm32")]
    try_s!(migrate_swaps_data(&ctx).await);

    let mut coins = HashSet::new();
    let legacy_unfinished_uuids = try_s!(get_unfinished_swaps_uuids(ctx.clone(), LEGACY_SWAP_TYPE).await);
    for uuid in legacy_unfinished_uuids {
        let swap = match SavedSwap::load_my_swap_from_db(&ctx, None, uuid).await {
            Ok(Some(s)) => s,
            Ok(None) => {
                warn!("Swap {} is indexed, but doesn't exist in DB", uuid);
                continue;
            },
            Err(e) => {
                error!("Error {} on getting swap {} data from DB", e, uuid);
                continue;
            },
        };
        info!("Kick starting the swap {}", swap.uuid());
        let maker_coin_ticker = match swap.maker_coin_ticker() {
            Ok(t) => t,
            Err(e) => {
                error!("Error {} getting maker coin of swap: {}", e, swap.uuid());
                continue;
            },
        };
        let taker_coin_ticker = match swap.taker_coin_ticker() {
            Ok(t) => t,
            Err(e) => {
                error!("Error {} getting taker coin of swap {}", e, swap.uuid());
                continue;
            },
        };
        coins.insert(maker_coin_ticker.clone());
        coins.insert(taker_coin_ticker.clone());

        let fut = kickstart_thread_handler(ctx.clone(), swap, maker_coin_ticker, taker_coin_ticker);
        ctx.spawner().spawn(fut);
    }

    let maker_swap_storage = MakerSwapStorage::new(ctx.clone());
    let unfinished_maker_uuids = try_s!(maker_swap_storage.get_unfinished().await);
    for maker_uuid in unfinished_maker_uuids {
        info!("Trying to kickstart maker swap {}", maker_uuid);
        let maker_swap_repr = match maker_swap_storage.get_repr(maker_uuid).await {
            Ok(repr) => repr,
            Err(e) => {
                error!("Error {} getting DB repr of maker swap {}", e, maker_uuid);
                continue;
            },
        };
        debug!("Got maker swap repr {:?}", maker_swap_repr);

        coins.insert(maker_swap_repr.maker_coin.clone());
        coins.insert(maker_swap_repr.taker_coin.clone());

        let fut =
            swap_kickstart_handler_for_maker(ctx.clone(), maker_swap_repr, maker_swap_storage.clone(), maker_uuid);
        ctx.spawner().spawn(fut);
    }

    let taker_swap_storage = TakerSwapStorage::new(ctx.clone());
    let unfinished_taker_uuids = try_s!(taker_swap_storage.get_unfinished().await);
    for taker_uuid in unfinished_taker_uuids {
        info!("Trying to kickstart taker swap {}", taker_uuid);
        let taker_swap_repr = match taker_swap_storage.get_repr(taker_uuid).await {
            Ok(repr) => repr,
            Err(e) => {
                error!("Error {} getting DB repr of taker swap {}", e, taker_uuid);
                continue;
            },
        };
        debug!("Got taker swap repr {:?}", taker_swap_repr);

        coins.insert(taker_swap_repr.maker_coin.clone());
        coins.insert(taker_swap_repr.taker_coin.clone());

        let fut =
            swap_kickstart_handler_for_taker(ctx.clone(), taker_swap_repr, taker_swap_storage.clone(), taker_uuid);
        ctx.spawner().spawn(fut);
    }
    Ok(coins)
}

async fn kickstart_thread_handler(ctx: MmArc, swap: SavedSwap, maker_coin_ticker: String, taker_coin_ticker: String) {
    let taker_coin = loop {
        match lp_coinfind(&ctx, &taker_coin_ticker).await {
            Ok(Some(c)) => break c,
            Ok(None) => {
                info!(
                    "Can't kickstart the swap {} until the coin {} is activated",
                    swap.uuid(),
                    taker_coin_ticker
                );
                Timer::sleep(5.).await;
            },
            Err(e) => {
                error!("Error {} on {} find attempt", e, taker_coin_ticker);
                return;
            },
        };
    };

    let maker_coin = loop {
        match lp_coinfind(&ctx, &maker_coin_ticker).await {
            Ok(Some(c)) => break c,
            Ok(None) => {
                info!(
                    "Can't kickstart the swap {} until the coin {} is activated",
                    swap.uuid(),
                    maker_coin_ticker
                );
                Timer::sleep(5.).await;
            },
            Err(e) => {
                error!("Error {} on {} find attempt", e, maker_coin_ticker);
                return;
            },
        };
    };
    match swap {
        SavedSwap::Maker(saved_swap) => {
            run_maker_swap(
                RunMakerSwapInput::KickStart {
                    maker_coin,
                    taker_coin,
                    swap_uuid: saved_swap.uuid,
                },
                ctx,
            )
            .await;
        },
        SavedSwap::Taker(saved_swap) => {
            run_taker_swap(
                RunTakerSwapInput::KickStart {
                    maker_coin,
                    taker_coin,
                    swap_uuid: saved_swap.uuid,
                },
                ctx,
            )
            .await;
        },
    }
}

pub async fn coins_needed_for_kick_start(ctx: MmArc) -> Result<Response<Vec<u8>>, String> {
    let res = try_s!(json::to_vec(&json!({
        "result": *(try_s!(ctx.coins_needed_for_kick_start.lock()))
    })));
    Ok(try_s!(Response::builder().body(res)))
}

pub async fn recover_funds_of_swap(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let uuid: Uuid = try_s!(json::from_value(req["params"]["uuid"].clone()));
    let swap = match SavedSwap::load_my_swap_from_db(&ctx, None, uuid).await {
        Ok(Some(swap)) => swap,
        Ok(None) => return ERR!("swap data is not found"),
        Err(e) => return ERR!("{}", e),
    };

    let recover_data = try_s!(swap.recover_funds(ctx).await);
    let res = try_s!(json::to_vec(&json!({
        "result": {
            "action": recover_data.action,
            "coin": recover_data.coin,
            "tx_hash": recover_data.transaction.tx_hash_as_bytes(),
            "tx_hex": BytesJson::from(recover_data.transaction.tx_hex()),
        }
    })));
    Ok(try_s!(Response::builder().body(res)))
}

pub async fn import_swaps(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let swaps: Vec<SavedSwap> = try_s!(json::from_value(req["swaps"].clone()));
    let mut imported = vec![];
    let mut skipped = HashMap::new();
    for swap in swaps {
        match swap.save_to_db(&ctx).await {
            Ok(_) => {
                if let Some(info) = swap.get_my_info() {
                    if let Err(e) = insert_new_swap_to_db(
                        ctx.clone(),
                        &info.my_coin,
                        &info.other_coin,
                        *swap.uuid(),
                        info.started_at,
                        LEGACY_SWAP_TYPE,
                    )
                    .await
                    {
                        error!("Error {} on new swap insertion", e);
                    }
                }
                imported.push(swap.uuid().to_owned());
            },
            Err(e) => {
                skipped.insert(swap.uuid().to_owned(), ERRL!("{}", e));
            },
        }
    }
    let res = try_s!(json::to_vec(&json!({
        "result": {
            "imported": imported,
            "skipped": skipped,
        }
    })));
    Ok(try_s!(Response::builder().body(res)))
}

#[derive(Deserialize)]
struct ActiveSwapsReq {
    #[serde(default)]
    include_status: bool,
}

#[derive(Serialize)]
struct ActiveSwapsRes {
    uuids: Vec<Uuid>,
    statuses: Option<HashMap<Uuid, SavedSwap>>,
}

/// This RPC does not support including statuses of v2 (Trading Protocol Upgrade) swaps.
/// It returns only uuids for these.
pub async fn active_swaps_rpc(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let req: ActiveSwapsReq = try_s!(json::from_value(req));
    let uuids_with_types = try_s!(active_swaps(&ctx));
    let statuses = if req.include_status {
        let mut map = HashMap::new();
        for (uuid, swap_type) in uuids_with_types.iter() {
            match *swap_type {
                LEGACY_SWAP_TYPE => {
                    let status = match SavedSwap::load_my_swap_from_db(&ctx, None, *uuid).await {
                        Ok(Some(status)) => status,
                        Ok(None) => continue,
                        Err(e) => {
                            error!("Error on loading_from_db: {}", e);
                            continue;
                        },
                    };
                    map.insert(*uuid, status);
                },
                unsupported_type => {
                    error!("active_swaps_rpc doesn't support swap type {}", unsupported_type);
                    continue;
                },
            }
        }
        Some(map)
    } else {
        None
    };
    let result = ActiveSwapsRes {
        uuids: uuids_with_types
            .into_iter()
            .map(|uuid_with_type| uuid_with_type.0)
            .collect(),
        statuses,
    };
    let res = try_s!(json::to_vec(&result));
    Ok(try_s!(Response::builder().body(res)))
}

// Todo: Maybe add a secret_hash_algo method to the SwapOps trait instead
/// Selects secret hash algorithm depending on types of coins being swapped
#[cfg(not(target_arch = "wasm32"))]
pub fn detect_secret_hash_algo(maker_coin: &MmCoinEnum, taker_coin: &MmCoinEnum) -> SecretHashAlgo {
    match (maker_coin, taker_coin) {
        (MmCoinEnum::Tendermint(_) | MmCoinEnum::TendermintToken(_) | MmCoinEnum::LightningCoin(_), _) => {
            SecretHashAlgo::SHA256
        },
        // If taker is lightning coin the SHA256 of the secret will be sent as part of the maker signed invoice
        (_, MmCoinEnum::Tendermint(_) | MmCoinEnum::TendermintToken(_)) => SecretHashAlgo::SHA256,
        (_, _) => SecretHashAlgo::DHASH160,
    }
}

/// Selects secret hash algorithm depending on types of coins being swapped
#[cfg(target_arch = "wasm32")]
pub fn detect_secret_hash_algo(maker_coin: &MmCoinEnum, taker_coin: &MmCoinEnum) -> SecretHashAlgo {
    match (maker_coin, taker_coin) {
        (MmCoinEnum::Tendermint(_) | MmCoinEnum::TendermintToken(_), _) => SecretHashAlgo::SHA256,
        (_, MmCoinEnum::Tendermint(_) | MmCoinEnum::TendermintToken(_)) => SecretHashAlgo::SHA256,
        (_, _) => SecretHashAlgo::DHASH160,
    }
}

/// Determines the secret hash algorithm for TPU, prioritizing SHA256 if either coin supports it.
/// # Attention
/// When adding new coins support, ensure their `secret_hash_algo_v2` implementation returns correct secret hash algorithm.
pub fn detect_secret_hash_algo_v2(maker_coin: &MmCoinEnum, taker_coin: &MmCoinEnum) -> SecretHashAlgo {
    let maker_algo = maker_coin.secret_hash_algo_v2();
    let taker_algo = taker_coin.secret_hash_algo_v2();
    if maker_algo == SecretHashAlgo::SHA256 || taker_algo == SecretHashAlgo::SHA256 {
        SecretHashAlgo::SHA256
    } else {
        SecretHashAlgo::DHASH160
    }
}

pub struct SwapPubkeys {
    pub maker: String,
    pub taker: String,
}

/// P2P topic used to broadcast messages during execution of the upgraded swap protocol.
pub fn swap_v2_topic(uuid: &Uuid) -> String {
    pub_sub_topic(SWAP_V2_PREFIX, &uuid.to_string())
}

/// Broadcast the swap v2 message once
pub fn broadcast_swap_v2_message<T: prost::Message>(
    ctx: &MmArc,
    topic: String,
    msg: &T,
    p2p_privkey: &Option<KeyPair>,
) {
    use prost::Message;

    let (p2p_private, from) = p2p_private_and_peer_id_to_broadcast(ctx, p2p_privkey.as_ref());
    let encoded_msg = msg.encode_to_vec();

    let secp_secret = SecretKey::from_slice(&p2p_private).expect("valid secret key");
    let secp_message =
        secp256k1::Message::from_slice(sha256(&encoded_msg).as_slice()).expect("sha256 is 32 bytes hash");
    let signature = SECP_SIGN.sign(&secp_message, &secp_secret);

    let signed_message = SignedMessage {
        from: PublicKey::from_secret_key(&*SECP_SIGN, &secp_secret).serialize().into(),
        signature: signature.serialize_compact().into(),
        payload: encoded_msg,
    };
    broadcast_p2p_msg(ctx, topic, signed_message.encode_to_vec(), from);
}

/// Spawns the loop that broadcasts message every `interval` seconds returning the AbortOnDropHandle
/// to stop it
pub fn broadcast_swap_v2_msg_every<T: prost::Message + 'static>(
    ctx: MmArc,
    topic: String,
    msg: T,
    interval_sec: f64,
    p2p_privkey: Option<KeyPair>,
) -> AbortOnDropHandle {
    let fut = async move {
        loop {
            broadcast_swap_v2_message(&ctx, topic.clone(), &msg, &p2p_privkey);
            Timer::sleep(interval_sec).await;
        }
    };
    spawn_abortable(fut)
}

/// Processes messages received during execution of the upgraded swap protocol.
pub fn process_swap_v2_msg(ctx: MmArc, topic: &str, msg: &[u8]) -> P2PProcessResult<()> {
    use prost::Message;

    let uuid = Uuid::from_str(topic).map_to_mm(|e| P2PProcessError::DecodeError(e.to_string()))?;

    let swap_ctx = SwapsContext::from_ctx(&ctx).unwrap();
    let mut msgs = swap_ctx.swap_v2_msgs.lock().unwrap();
    if let Some(msg_store) = msgs.get_mut(&uuid) {
        let signed_message = SignedMessage::decode(msg).map_to_mm(|e| P2PProcessError::DecodeError(e.to_string()))?;

        let pubkey =
            PublicKey::from_slice(&signed_message.from).map_to_mm(|e| P2PProcessError::DecodeError(e.to_string()))?;
        if pubkey != msg_store.accept_only_from {
            return MmError::err(P2PProcessError::UnexpectedSender(pubkey.to_string()));
        }

        let signature = Signature::from_compact(&signed_message.signature)
            .map_to_mm(|e| P2PProcessError::DecodeError(e.to_string()))?;
        let secp_message = secp256k1::Message::from_slice(sha256(&signed_message.payload).as_slice())
            .expect("sha256 is 32 bytes hash");

        SECP_VERIFY
            .verify(&secp_message, &signature, &pubkey)
            .map_to_mm(|e| P2PProcessError::InvalidSignature(e.to_string()))?;

        let swap_message = SwapMessage::decode(signed_message.payload.as_slice())
            .map_to_mm(|e| P2PProcessError::DecodeError(e.to_string()))?;

        let uuid_from_message =
            Uuid::from_slice(&swap_message.swap_uuid).map_to_mm(|e| P2PProcessError::DecodeError(e.to_string()))?;

        if uuid_from_message != uuid {
            return MmError::err(P2PProcessError::ValidationFailed(format!(
                "uuid from message {uuid_from_message} doesn't match uuid from topic {uuid}",
            )));
        }

        debug!("Processing swap v2 msg {:?} for uuid {}", swap_message, uuid);
        match swap_message.inner {
            Some(swap_message::Inner::MakerNegotiation(maker_negotiation)) => {
                msg_store.maker_negotiation = Some(maker_negotiation)
            },
            Some(swap_message::Inner::TakerNegotiation(taker_negotiation)) => {
                msg_store.taker_negotiation = Some(taker_negotiation)
            },
            Some(swap_message::Inner::MakerNegotiated(maker_negotiated)) => {
                msg_store.maker_negotiated = Some(maker_negotiated)
            },
            Some(swap_message::Inner::TakerFundingInfo(taker_funding)) => msg_store.taker_funding = Some(taker_funding),
            Some(swap_message::Inner::MakerPaymentInfo(maker_payment)) => msg_store.maker_payment = Some(maker_payment),
            Some(swap_message::Inner::TakerPaymentInfo(taker_payment)) => msg_store.taker_payment = Some(taker_payment),
            Some(swap_message::Inner::TakerPaymentSpendPreimage(preimage)) => {
                msg_store.taker_payment_spend_preimage = Some(preimage)
            },
            None => return MmError::err(P2PProcessError::DecodeError("swap_message.inner is None".into())),
        }
    }
    Ok(())
}

async fn recv_swap_v2_msg<T>(
    ctx: MmArc,
    mut getter: impl FnMut(&mut SwapV2MsgStore) -> Option<T>,
    uuid: &Uuid,
    timeout: u64,
) -> Result<T, String> {
    let started = now_sec();
    let timeout = BASIC_COMM_TIMEOUT + timeout;
    let wait_until = started + timeout;
    loop {
        Timer::sleep(1.).await;
        let swap_ctx = SwapsContext::from_ctx(&ctx).unwrap();
        let mut msgs = swap_ctx.swap_v2_msgs.lock().unwrap();
        if let Some(msg_store) = msgs.get_mut(uuid) {
            if let Some(msg) = getter(msg_store) {
                return Ok(msg);
            }
        }
        let now = now_sec();
        if now > wait_until {
            return ERR!("Timeout ({} > {})", now - started, timeout);
        }
    }
}

pub fn generate_secret() -> Result<[u8; 32], rand::Error> {
    let mut sec = [0u8; 32];
    common::os_rng(&mut sec)?;
    Ok(sec)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod lp_swap_tests {
    use super::*;
    use crate::lp_native_dex::{fix_directories, init_p2p};
    use coins::hd_wallet::HDPathAccountToAddressId;
    use coins::utxo::rpc_clients::ElectrumConnectionSettings;
    use coins::utxo::utxo_standard::utxo_standard_coin_with_priv_key;
    use coins::utxo::{UtxoActivationParams, UtxoRpcMode};
    use coins::PrivKeyActivationPolicy;
    use coins::{DexFee, MarketCoinOps, TestCoin};
    use common::{block_on, new_uuid};
    use mm2_core::mm_ctx::MmCtxBuilder;
    use mm2_test_helpers::for_tests::{morty_conf, rick_conf, MORTY_ELECTRUM_ADDRS, RICK_ELECTRUM_ADDRS};
    use mocktopus::mocking::*;
    use std::convert::TryFrom;

    #[test]
    fn test_lp_atomic_locktime() {
        let maker_coin = "KMD";
        let taker_coin = "DEX";
        let my_conf_settings = SwapConfirmationsSettings {
            maker_coin_confs: 2,
            maker_coin_nota: true,
            taker_coin_confs: 2,
            taker_coin_nota: true,
        };
        let other_conf_settings = SwapConfirmationsSettings {
            maker_coin_confs: 1,
            maker_coin_nota: false,
            taker_coin_confs: 1,
            taker_coin_nota: false,
        };
        let expected = get_payment_locktime() * 4;
        let version = AtomicLocktimeVersion::V2 {
            my_conf_settings,
            other_conf_settings,
        };
        let actual = lp_atomic_locktime(maker_coin, taker_coin, version);
        assert_eq!(expected, actual);

        let maker_coin = "KMD";
        let taker_coin = "DEX";
        let my_conf_settings = SwapConfirmationsSettings {
            maker_coin_confs: 2,
            maker_coin_nota: true,
            taker_coin_confs: 2,
            taker_coin_nota: false,
        };
        let other_conf_settings = SwapConfirmationsSettings {
            maker_coin_confs: 1,
            maker_coin_nota: false,
            taker_coin_confs: 1,
            taker_coin_nota: false,
        };
        let expected = get_payment_locktime() * 4;
        let version = AtomicLocktimeVersion::V2 {
            my_conf_settings,
            other_conf_settings,
        };
        let actual = lp_atomic_locktime(maker_coin, taker_coin, version);
        assert_eq!(expected, actual);

        let maker_coin = "KMD";
        let taker_coin = "DEX";
        let my_conf_settings = SwapConfirmationsSettings {
            maker_coin_confs: 2,
            maker_coin_nota: false,
            taker_coin_confs: 2,
            taker_coin_nota: true,
        };
        let other_conf_settings = SwapConfirmationsSettings {
            maker_coin_confs: 1,
            maker_coin_nota: false,
            taker_coin_confs: 1,
            taker_coin_nota: false,
        };
        let expected = get_payment_locktime() * 4;
        let version = AtomicLocktimeVersion::V2 {
            my_conf_settings,
            other_conf_settings,
        };
        let actual = lp_atomic_locktime(maker_coin, taker_coin, version);
        assert_eq!(expected, actual);

        let maker_coin = "KMD";
        let taker_coin = "DEX";
        let my_conf_settings = SwapConfirmationsSettings {
            maker_coin_confs: 2,
            maker_coin_nota: false,
            taker_coin_confs: 2,
            taker_coin_nota: false,
        };
        let other_conf_settings = SwapConfirmationsSettings {
            maker_coin_confs: 1,
            maker_coin_nota: false,
            taker_coin_confs: 1,
            taker_coin_nota: false,
        };
        let expected = get_payment_locktime();
        let version = AtomicLocktimeVersion::V2 {
            my_conf_settings,
            other_conf_settings,
        };
        let actual = lp_atomic_locktime(maker_coin, taker_coin, version);
        assert_eq!(expected, actual);

        let maker_coin = "BTC";
        let taker_coin = "DEX";
        let my_conf_settings = SwapConfirmationsSettings {
            maker_coin_confs: 2,
            maker_coin_nota: false,
            taker_coin_confs: 2,
            taker_coin_nota: false,
        };
        let other_conf_settings = SwapConfirmationsSettings {
            maker_coin_confs: 1,
            maker_coin_nota: false,
            taker_coin_confs: 1,
            taker_coin_nota: false,
        };
        let expected = get_payment_locktime() * 4;
        let version = AtomicLocktimeVersion::V2 {
            my_conf_settings,
            other_conf_settings,
        };
        let actual = lp_atomic_locktime(maker_coin, taker_coin, version);
        assert_eq!(expected, actual);

        let maker_coin = "KMD";
        let taker_coin = "BTC";
        let my_conf_settings = SwapConfirmationsSettings {
            maker_coin_confs: 2,
            maker_coin_nota: false,
            taker_coin_confs: 2,
            taker_coin_nota: false,
        };
        let other_conf_settings = SwapConfirmationsSettings {
            maker_coin_confs: 1,
            maker_coin_nota: false,
            taker_coin_confs: 1,
            taker_coin_nota: false,
        };
        let expected = get_payment_locktime() * 4;
        let version = AtomicLocktimeVersion::V2 {
            my_conf_settings,
            other_conf_settings,
        };
        let actual = lp_atomic_locktime(maker_coin, taker_coin, version);
        assert_eq!(expected, actual);

        let maker_coin = "KMD";
        let taker_coin = "DEX";
        let expected = get_payment_locktime();
        let actual = lp_atomic_locktime(maker_coin, taker_coin, AtomicLocktimeVersion::V1);
        assert_eq!(expected, actual);

        let maker_coin = "KMD";
        let taker_coin = "DEX";
        let expected = get_payment_locktime();
        let actual = lp_atomic_locktime(maker_coin, taker_coin, AtomicLocktimeVersion::V1);
        assert_eq!(expected, actual);

        let maker_coin = "KMD";
        let taker_coin = "DEX";
        let expected = get_payment_locktime();
        let actual = lp_atomic_locktime(maker_coin, taker_coin, AtomicLocktimeVersion::V1);
        assert_eq!(expected, actual);

        let maker_coin = "KMD";
        let taker_coin = "DEX";
        let expected = get_payment_locktime();
        let actual = lp_atomic_locktime(maker_coin, taker_coin, AtomicLocktimeVersion::V1);
        assert_eq!(expected, actual);

        let maker_coin = "BTC";
        let taker_coin = "DEX";
        let expected = get_payment_locktime() * 10;
        let actual = lp_atomic_locktime(maker_coin, taker_coin, AtomicLocktimeVersion::V1);
        assert_eq!(expected, actual);

        let maker_coin = "KMD";
        let taker_coin = "BTC";
        let expected = get_payment_locktime() * 10;
        let actual = lp_atomic_locktime(maker_coin, taker_coin, AtomicLocktimeVersion::V1);
        assert_eq!(expected, actual);
    }

    #[test]
    fn check_negotiation_data_serde() {
        // old message format should be deserialized to NegotiationDataMsg::V1
        let v1 = NegotiationDataV1 {
            started_at: 0,
            payment_locktime: 0,
            secret_hash: [0; 20],
            persistent_pubkey: [1; 33].into(),
        };

        let expected = NegotiationDataMsg::V1(NegotiationDataV1 {
            started_at: 0,
            payment_locktime: 0,
            secret_hash: [0; 20],
            persistent_pubkey: [1; 33].into(),
        });

        let serialized = rmp_serde::to_vec_named(&v1).unwrap();

        let deserialized: NegotiationDataMsg = rmp_serde::from_slice(serialized.as_slice()).unwrap();

        assert_eq!(deserialized, expected);

        // new message format should be deserialized to old
        let v2 = NegotiationDataMsg::V2(NegotiationDataV2 {
            started_at: 0,
            payment_locktime: 0,
            secret_hash: vec![0; 20],
            persistent_pubkey: [1; 33].into(),
            maker_coin_swap_contract: vec![1; 20],
            taker_coin_swap_contract: vec![1; 20],
        });

        let expected = NegotiationDataV1 {
            started_at: 0,
            payment_locktime: 0,
            secret_hash: [0; 20],
            persistent_pubkey: [1; 33].into(),
        };

        let serialized = rmp_serde::to_vec_named(&v2).unwrap();

        let deserialized: NegotiationDataV1 = rmp_serde::from_slice(serialized.as_slice()).unwrap();

        assert_eq!(deserialized, expected);

        // new message format should be deserialized to new
        let v2 = NegotiationDataMsg::V2(NegotiationDataV2 {
            started_at: 0,
            payment_locktime: 0,
            secret_hash: vec![0; 20],
            persistent_pubkey: [1; 33].into(),
            maker_coin_swap_contract: vec![1; 20],
            taker_coin_swap_contract: vec![1; 20],
        });

        let serialized = rmp_serde::to_vec(&v2).unwrap();

        let deserialized: NegotiationDataMsg = rmp_serde::from_slice(serialized.as_slice()).unwrap();

        assert_eq!(deserialized, v2);

        let v3 = NegotiationDataMsg::V3(NegotiationDataV3 {
            started_at: 0,
            payment_locktime: 0,
            secret_hash: vec![0; 20],
            maker_coin_swap_contract: vec![1; 20],
            taker_coin_swap_contract: vec![1; 20],
            maker_coin_htlc_pub: [1; 33].into(),
            taker_coin_htlc_pub: [1; 33].into(),
        });

        // v3 must be deserialized to v3, backward compatibility is not required
        let serialized = rmp_serde::to_vec(&v3).unwrap();

        let deserialized: NegotiationDataMsg = rmp_serde::from_slice(serialized.as_slice()).unwrap();

        assert_eq!(deserialized, v3);
    }

    #[test]
    fn check_payment_data_serde() {
        const MSG_DATA_INSTRUCTIONS: [u8; 300] = [1; 300];

        #[derive(Clone, Debug, Eq, Deserialize, PartialEq, Serialize)]
        enum SwapMsgOld {
            Negotiation(NegotiationDataMsg),
            NegotiationReply(NegotiationDataMsg),
            Negotiated(bool),
            TakerFee(Vec<u8>),
            MakerPayment(Vec<u8>),
            TakerPayment(Vec<u8>),
        }

        // old message format should be deserialized to PaymentDataMsg::Regular
        let old = SwapMsgOld::MakerPayment(MSG_DATA_INSTRUCTIONS.to_vec());

        let expected = SwapMsg::MakerPayment(SwapTxDataMsg::Regular(MSG_DATA_INSTRUCTIONS.to_vec()));

        let serialized = rmp_serde::to_vec_named(&old).unwrap();

        let deserialized: SwapMsg = rmp_serde::from_slice(serialized.as_slice()).unwrap();

        assert_eq!(deserialized, expected);

        // PaymentDataMsg::Regular should be deserialized to old message format
        let v1 = SwapMsg::MakerPayment(SwapTxDataMsg::Regular(MSG_DATA_INSTRUCTIONS.to_vec()));

        let expected = old;

        let serialized = rmp_serde::to_vec_named(&v1).unwrap();

        let deserialized: SwapMsgOld = rmp_serde::from_slice(serialized.as_slice()).unwrap();

        assert_eq!(deserialized, expected);

        // PaymentDataMsg::Regular should be deserialized to PaymentDataMsg::Regular
        let v1 = SwapMsg::MakerPayment(SwapTxDataMsg::Regular(MSG_DATA_INSTRUCTIONS.to_vec()));

        let serialized = rmp_serde::to_vec_named(&v1).unwrap();

        let deserialized: SwapMsg = rmp_serde::from_slice(serialized.as_slice()).unwrap();

        assert_eq!(deserialized, v1);

        // PaymentDataMsg::WithInstructions should be deserialized to PaymentDataMsg::WithInstructions
        let v2 = SwapMsg::MakerPayment(SwapTxDataMsg::WithInstructions(PaymentWithInstructions {
            data: MSG_DATA_INSTRUCTIONS.to_vec(),
            next_step_instructions: MSG_DATA_INSTRUCTIONS.to_vec(),
        }));

        let serialized = rmp_serde::to_vec_named(&v2).unwrap();

        let deserialized: SwapMsg = rmp_serde::from_slice(serialized.as_slice()).unwrap();

        assert_eq!(deserialized, v2);

        // PaymentDataMsg::WithInstructions shouldn't be deserialized to old message format, new nodes with payment instructions can't swap with old nodes without it.
        let v2 = SwapMsg::MakerPayment(SwapTxDataMsg::WithInstructions(PaymentWithInstructions {
            data: MSG_DATA_INSTRUCTIONS.to_vec(),
            next_step_instructions: MSG_DATA_INSTRUCTIONS.to_vec(),
        }));

        let serialized = rmp_serde::to_vec_named(&v2).unwrap();

        let deserialized: Result<SwapMsgOld, rmp_serde::decode::Error> = rmp_serde::from_slice(serialized.as_slice());

        assert!(deserialized.is_err());
    }

    fn utxo_activation_params(electrums: &[&str]) -> UtxoActivationParams {
        UtxoActivationParams {
            mode: UtxoRpcMode::Electrum {
                servers: electrums
                    .iter()
                    .map(|url| ElectrumConnectionSettings {
                        url: url.to_string(),
                        protocol: Default::default(),
                        disable_cert_verification: false,
                        timeout_sec: None,
                    })
                    .collect(),
                min_connected: None,
                max_connected: None,
            },
            utxo_merge_params: None,
            tx_history: false,
            required_confirmations: Some(0),
            requires_notarization: None,
            address_format: None,
            gap_limit: None,
            enable_params: Default::default(),
            priv_key_policy: PrivKeyActivationPolicy::ContextPrivKey,
            check_utxo_maturity: None,
            path_to_address: HDPathAccountToAddressId::default(),
        }
    }

    #[test]
    #[ignore]
    fn gen_recoverable_swap() {
        let maker_passphrase = std::env::var("BOB_PASSPHRASE").expect("BOB_PASSPHRASE env must be set");
        let maker_fail_at = std::env::var("MAKER_FAIL_AT").map(maker_swap::FailAt::from).ok();
        let taker_passphrase = std::env::var("ALICE_PASSPHRASE").expect("ALICE_PASSPHRASE env must be set");
        let taker_fail_at = std::env::var("TAKER_FAIL_AT").map(taker_swap::FailAt::from).ok();
        let lock_duration = match std::env::var("LOCK_DURATION") {
            Ok(maybe_num) => maybe_num.parse().expect("LOCK_DURATION must be a number of seconds"),
            Err(_) => 30,
        };

        if maker_fail_at.is_none() && taker_fail_at.is_none() {
            panic!("At least one of MAKER_FAIL_AT/TAKER_FAIL_AT must be provided");
        }

        let maker_ctx_conf = json!({
            "netid": 1234,
            "p2p_in_memory": true,
            "p2p_in_memory_port": 777,
            "i_am_seed": true,
            "is_bootstrap_node": true
        });

        let maker_ctx = MmCtxBuilder::default().with_conf(maker_ctx_conf).into_mm_arc();
        let maker_key_pair = *CryptoCtx::init_with_iguana_passphrase(maker_ctx.clone(), &maker_passphrase)
            .unwrap()
            .mm2_internal_key_pair();

        fix_directories(&maker_ctx).unwrap();
        block_on(init_p2p(maker_ctx.clone())).unwrap();
        maker_ctx.init_sqlite_connection().unwrap();

        let rick_activation_params = utxo_activation_params(RICK_ELECTRUM_ADDRS);
        let morty_activation_params = utxo_activation_params(MORTY_ELECTRUM_ADDRS);

        let rick_maker = block_on(utxo_standard_coin_with_priv_key(
            &maker_ctx,
            "RICK",
            &rick_conf(),
            &rick_activation_params,
            maker_key_pair.private().secret,
        ))
        .unwrap();

        println!("Maker address {}", rick_maker.my_address().unwrap());

        let morty_maker = block_on(utxo_standard_coin_with_priv_key(
            &maker_ctx,
            "MORTY",
            &morty_conf(),
            &morty_activation_params,
            maker_key_pair.private().secret,
        ))
        .unwrap();

        let taker_ctx_conf = json!({
            "netid": 1234,
            "p2p_in_memory": true,
            "seednodes": vec!["/memory/777"]
        });

        let taker_ctx = MmCtxBuilder::default().with_conf(taker_ctx_conf).into_mm_arc();
        let taker_key_pair = *CryptoCtx::init_with_iguana_passphrase(taker_ctx.clone(), &taker_passphrase)
            .unwrap()
            .mm2_internal_key_pair();

        fix_directories(&taker_ctx).unwrap();
        block_on(init_p2p(taker_ctx.clone())).unwrap();
        taker_ctx.init_sqlite_connection().unwrap();

        let rick_taker = block_on(utxo_standard_coin_with_priv_key(
            &taker_ctx,
            "RICK",
            &rick_conf(),
            &rick_activation_params,
            taker_key_pair.private().secret,
        ))
        .unwrap();

        let morty_taker = block_on(utxo_standard_coin_with_priv_key(
            &taker_ctx,
            "MORTY",
            &morty_conf(),
            &morty_activation_params,
            taker_key_pair.private().secret,
        ))
        .unwrap();

        println!("Taker address {}", rick_taker.my_address().unwrap());

        let uuid = new_uuid();
        let maker_amount = BigDecimal::from_str("0.1").unwrap();
        let taker_amount = BigDecimal::from_str("0.1").unwrap();
        let conf_settings = SwapConfirmationsSettings {
            maker_coin_confs: 0,
            maker_coin_nota: false,
            taker_coin_confs: 0,
            taker_coin_nota: false,
        };

        let mut maker_swap = MakerSwap::new(
            maker_ctx.clone(),
            taker_key_pair.public().compressed_unprefixed().unwrap().into(),
            maker_amount.clone(),
            taker_amount.clone(),
            <[u8; 33]>::try_from(maker_key_pair.public_slice()).unwrap().into(),
            uuid,
            None,
            conf_settings,
            rick_maker.clone().into(),
            morty_maker.into(),
            lock_duration,
            None,
            Default::default(),
        );

        maker_swap.fail_at = maker_fail_at;

        #[cfg(any(test, feature = "run-docker-tests"))]
        let fail_at = std::env::var("TAKER_FAIL_AT").map(taker_swap::FailAt::from).ok();

        let taker_swap = TakerSwap::new(
            taker_ctx.clone(),
            maker_key_pair.public().compressed_unprefixed().unwrap().into(),
            maker_amount.into(),
            taker_amount.into(),
            <[u8; 33]>::try_from(taker_key_pair.public_slice()).unwrap().into(),
            uuid,
            None,
            conf_settings,
            rick_taker.clone().into(),
            morty_taker.into(),
            lock_duration,
            None,
            #[cfg(any(test, feature = "run-docker-tests"))]
            fail_at,
        );

        block_on(futures::future::join(
            run_maker_swap(RunMakerSwapInput::StartNew(maker_swap), maker_ctx.clone()),
            run_taker_swap(RunTakerSwapInput::StartNew(taker_swap), taker_ctx.clone()),
        ));

        let makers_maker_coin_address = rick_maker.my_address().unwrap();
        let takers_maker_coin_address = rick_taker.my_address().unwrap();

        println!(
            "Maker swap path {}",
            std::fs::canonicalize(my_swap_file_path(&maker_ctx, &makers_maker_coin_address, &uuid))
                .unwrap()
                .display()
        );
        println!(
            "Taker swap path {}",
            std::fs::canonicalize(my_swap_file_path(&taker_ctx, &takers_maker_coin_address, &uuid))
                .unwrap()
                .display()
        );
    }

    #[test]
    fn test_deserialize_iris_swap_status() {
        let _: SavedSwap = json::from_str(include_str!("for_tests/iris_nimda_rick_taker_swap.json")).unwrap();
        let _: SavedSwap = json::from_str(include_str!("for_tests/iris_nimda_rick_maker_swap.json")).unwrap();
    }

    #[test]
    fn test_kmd_taker_dex_fee_calculation() {
        std::env::set_var("MYCOIN_FEE_DISCOUNT", "");

        let kmd = coins::TestCoin::new("KMD");
        TestCoin::should_burn_dex_fee.mock_safe(|_| MockResult::Return(true));
        let (kmd_fee_amount, kmd_burn_amount) = match DexFee::new_from_taker_coin(&kmd, "ETH", &MmNumber::from(6150)) {
            DexFee::Standard(_) | DexFee::NoFee => {
                panic!("Wrong variant returned for KMD from `DexFee::new_from_taker_coin`.")
            },
            DexFee::WithBurn {
                fee_amount,
                burn_amount,
                ..
            } => (fee_amount, burn_amount),
        };
        TestCoin::should_burn_dex_fee.clear_mock();

        let mycoin = coins::TestCoin::new("MYCOIN");
        TestCoin::should_burn_dex_fee.mock_safe(|_| MockResult::Return(true));
        let (mycoin_fee_amount, mycoin_burn_amount) =
            match DexFee::new_from_taker_coin(&mycoin, "ETH", &MmNumber::from(6150)) {
                DexFee::Standard(_) | DexFee::NoFee => {
                    panic!("Wrong variant returned for MYCOIN from `DexFee::new_from_taker_coin`.")
                },
                DexFee::WithBurn {
                    fee_amount,
                    burn_amount,
                    ..
                } => (fee_amount, burn_amount),
            };
        TestCoin::should_burn_dex_fee.clear_mock();

        let expected_mycoin_total_fee = &kmd_fee_amount / &MmNumber::from("0.75");
        let expected_kmd_burn_amount = &expected_mycoin_total_fee - &kmd_fee_amount;

        assert_eq!(kmd_fee_amount, mycoin_fee_amount);
        assert_eq!(expected_kmd_burn_amount, kmd_burn_amount);
        // assuming for TestCoin dust is zero
        assert_eq!(mycoin_burn_amount, kmd_burn_amount);
    }

    #[test]
    fn test_dex_fee_from_taker_coin_discount() {
        std::env::set_var("MYCOIN_FEE_DISCOUNT", "");

        let mycoin = coins::TestCoin::new("MYCOIN");
        TestCoin::should_burn_dex_fee.mock_safe(|_| MockResult::Return(true));
        let (mycoin_taker_fee, mycoin_burn_amount) =
            match DexFee::new_from_taker_coin(&mycoin, "", &MmNumber::from(6150)) {
                DexFee::Standard(_) | DexFee::NoFee => {
                    panic!("Wrong variant returned for MYCOIN from `DexFee::new_from_taker_coin`.")
                },
                DexFee::WithBurn {
                    fee_amount,
                    burn_amount,
                    ..
                } => (fee_amount, burn_amount),
            };
        TestCoin::should_burn_dex_fee.clear_mock();

        let testcoin = coins::TestCoin::default();
        TestCoin::should_burn_dex_fee.mock_safe(|_| MockResult::Return(true));
        let (testcoin_taker_fee, testcoin_burn_amount) =
            match DexFee::new_from_taker_coin(&testcoin, "", &MmNumber::from(6150)) {
                DexFee::Standard(_) | DexFee::NoFee => {
                    panic!("Wrong variant returned for TEST coin from `DexFee::new_from_taker_coin`.")
                },
                DexFee::WithBurn {
                    fee_amount,
                    burn_amount,
                    ..
                } => (fee_amount, burn_amount),
            };
        TestCoin::should_burn_dex_fee.clear_mock();
        assert_eq!(testcoin_taker_fee * MmNumber::from("0.90"), mycoin_taker_fee);
        assert_eq!(testcoin_burn_amount * MmNumber::from("0.90"), mycoin_burn_amount);
    }

    #[test]
    fn test_legacy_new_negotiation_rmp() {
        // In legacy messages, persistent_pubkey was represented as Vec<u8> instead of H264.
        #[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
        struct LegacyNegotiationDataV2 {
            started_at: u64,
            payment_locktime: u64,
            secret_hash: Vec<u8>,
            persistent_pubkey: Vec<u8>,
            maker_coin_swap_contract: Vec<u8>,
            taker_coin_swap_contract: Vec<u8>,
        }

        let legacy_instance = LegacyNegotiationDataV2 {
            started_at: 1620000000,
            payment_locktime: 1620003600,
            secret_hash: vec![0u8; 20],
            persistent_pubkey: vec![1u8; 33],
            maker_coin_swap_contract: vec![1u8; 20],
            taker_coin_swap_contract: vec![1u8; 20],
        };

        // ------------------------------------------
        // Step 1: Test Deserialization from Legacy Format
        // ------------------------------------------
        let legacy_serialized =
            rmp_serde::to_vec_named(&legacy_instance).expect("Legacy MessagePack serialization failed");
        let new_instance: NegotiationDataV2 =
            rmp_serde::from_slice(&legacy_serialized).expect("Deserialization into new NegotiationDataV2 failed");

        assert_eq!(new_instance.started_at, legacy_instance.started_at);
        assert_eq!(new_instance.payment_locktime, legacy_instance.payment_locktime);
        assert_eq!(new_instance.secret_hash, legacy_instance.secret_hash);
        assert_eq!(
            new_instance.persistent_pubkey.0.to_vec(),
            legacy_instance.persistent_pubkey
        );
        assert_eq!(
            new_instance.maker_coin_swap_contract,
            legacy_instance.maker_coin_swap_contract
        );
        assert_eq!(
            new_instance.taker_coin_swap_contract,
            legacy_instance.taker_coin_swap_contract
        );

        // ------------------------------------------
        // Step 2: Test Serialization from New Format to Legacy Format
        // ------------------------------------------
        let new_serialized = rmp_serde::to_vec_named(&new_instance).expect("Serialization of new type failed");
        let legacy_from_new: LegacyNegotiationDataV2 =
            rmp_serde::from_slice(&new_serialized).expect("Legacy deserialization from new serialization failed");

        assert_eq!(legacy_from_new.started_at, new_instance.started_at);
        assert_eq!(legacy_from_new.payment_locktime, new_instance.payment_locktime);
        assert_eq!(legacy_from_new.secret_hash, new_instance.secret_hash);
        assert_eq!(
            legacy_from_new.persistent_pubkey,
            new_instance.persistent_pubkey.0.to_vec()
        );
        assert_eq!(
            legacy_from_new.maker_coin_swap_contract,
            new_instance.maker_coin_swap_contract
        );
        assert_eq!(
            legacy_from_new.taker_coin_swap_contract,
            new_instance.taker_coin_swap_contract
        );

        // ------------------------------------------
        // Step 3: Round-Trip Test of the New Format
        // ------------------------------------------
        let rt_serialized = rmp_serde::to_vec_named(&new_instance).expect("Round-trip serialization failed");
        let round_trip: NegotiationDataV2 =
            rmp_serde::from_slice(&rt_serialized).expect("Round-trip deserialization failed");
        assert_eq!(round_trip, new_instance);
    }
}
