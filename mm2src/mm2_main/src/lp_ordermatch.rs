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
//  lp_ordermatch.rs
//  marketmaker
//

use async_trait::async_trait;
use blake2::digest::{Update, VariableOutput};
use blake2::Blake2bVar;
use coins::utxo::{compressed_pub_key_from_priv_raw, ChecksumType, UtxoAddressFormat};
use coins::{
    coin_conf, find_pair, lp_coinfind, BalanceTradeFeeUpdatedHandler, CoinProtocol, CoinsContext, FeeApproxStage,
    MakerCoinSwapOpsV2, MmCoin, MmCoinEnum, TakerCoinSwapOpsV2,
};
use common::executor::{
    simple_map::AbortableSimpleMap, AbortSettings, AbortableSystem, AbortedError, SpawnAbortable, SpawnFuture, Timer,
};
use common::log::{error, warn, LogOnError};
use common::{bits256, log, new_uuid, now_ms, now_sec};
use crypto::privkey::SerializableSecp256k1Keypair;
use crypto::{CryptoCtx, CryptoCtxError};
use derive_more::Display;
use futures::{compat::Future01CompatExt, lock::Mutex as AsyncMutex, TryFutureExt};
use hash256_std_hasher::Hash256StdHasher;
use hash_db::Hasher;
use http::Response;
use keys::{AddressFormat, KeyPair};
use mm2_core::mm_ctx::{from_ctx, MmArc, MmWeak};
use mm2_err_handle::prelude::*;
use mm2_event_stream::{DeriveStreamerId, StreamingManager};
use mm2_libp2p::application::request_response::ordermatch::OrdermatchRequest;
use mm2_libp2p::application::request_response::P2PRequest;
use mm2_libp2p::{
    decode_signed, encode_and_sign, encode_message, pub_sub_topic, PublicKey, TopicHash, TopicPrefix, TOPIC_SEPARATOR,
};
use mm2_metrics::mm_gauge;
use mm2_number::{BigDecimal, BigRational, MmNumber, MmNumberMultiRepr};
use mm2_rpc::data::legacy::{
    MatchBy, Mm2RpcResult, OrderConfirmationsSettings, OrderType, RpcOrderbookEntry, SellBuyRequest, SellBuyResponse,
    TakerAction, TakerRequestForRpc,
};
use mm2_state_machine::prelude::*;
#[cfg(test)]
use mocktopus::macros::*;
use my_orders_storage::{
    delete_my_maker_order, delete_my_taker_order, save_maker_order_on_update, save_my_new_maker_order,
    save_my_new_taker_order, MyActiveOrders, MyOrdersFilteringHistory, MyOrdersHistory, MyOrdersStorage,
};
use num_traits::identities::Zero;
use order_events::{OrderStatusEvent, OrderStatusStreamer};
use orderbook_events::{OrderbookItemChangeEvent, OrderbookStreamer};
use parking_lot::Mutex as PaMutex;
use rpc::v1::types::H256 as H256Json;
use secp256k1::PublicKey as Secp256k1Pubkey;
use serde_json::{self as json, Value as Json};
use sp_trie::{delta_trie_root, MemoryDB, Trie, TrieConfiguration, TrieDB, TrieDBMut, TrieHash, TrieMut};
use std::collections::hash_map::{Entry, HashMap};
use std::collections::{BTreeSet, HashSet};
use std::convert::TryInto;
use std::fmt;
use std::ops::Deref;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use timed_map::{MapKind, TimedMap};
use trie_db::NodeCodec as NodeCodecT;
use uuid::Uuid;

use crate::lp_network::{broadcast_p2p_msg, request_any_relay, request_one_peer, subscribe_to_topic, P2PRequestError};
use crate::lp_swap::maker_swap_v2::{self, MakerSwapStateMachine, MakerSwapStorage};
use crate::lp_swap::taker_swap_v2::{self, TakerSwapStateMachine, TakerSwapStorage};
use crate::lp_swap::{
    calc_max_maker_vol, check_balance_for_maker_swap, check_balance_for_taker_swap, check_other_coin_balance_for_swap,
    detect_secret_hash_algo_v2, generate_secret, get_max_maker_vol, insert_new_swap_to_db, is_pubkey_banned,
    lp_atomic_locktime, p2p_keypair_and_peer_id_to_broadcast, p2p_private_and_peer_id_to_broadcast, run_maker_swap,
    run_taker_swap, swap_v2_topic, AtomicLocktimeVersion, CheckBalanceError, CheckBalanceResult, CoinVolumeInfo,
    MakerSwap, RunMakerSwapInput, RunTakerSwapInput, SwapConfirmationsSettings, TakerSwap, LEGACY_SWAP_TYPE,
};
use crate::swap_versioning::{legacy_swap_version, SwapVersion};

#[cfg(any(test, feature = "run-docker-tests"))]
use crate::lp_swap::taker_swap::FailAt;

#[cfg(feature = "ibc-routing-for-swaps")]
use coins::rpc_command::tendermint::ibc::ChannelId;

pub use best_orders::{best_orders_rpc, best_orders_rpc_v2};
use crypto::secret_hash_algo::SecretHashAlgo;
pub use orderbook_depth::orderbook_depth_rpc;
pub use orderbook_rpc::{orderbook_rpc, orderbook_rpc_v2};

cfg_wasm32! {
    use mm2_db::indexed_db::{ConstructibleDb, DbLocked};
    use ordermatch_wasm_db::{InitDbResult, OrdermatchDb};

    pub type OrdermatchDbLocked<'a> = DbLocked<'a, OrdermatchDb>;
}

mod best_orders;
mod lp_bot;
pub use lp_bot::{
    start_simple_market_maker_bot, stop_simple_market_maker_bot, StartSimpleMakerBotRequest, TradingBotEvent,
};
use primitives::hash::{H256, H264};

mod my_orders_storage;
mod new_protocol;
pub(crate) mod order_events;
mod order_requests_tracker;
mod orderbook_depth;
pub(crate) mod orderbook_events;
mod orderbook_rpc;
#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "ordermatch_tests.rs"]
pub mod ordermatch_tests;

#[cfg(target_arch = "wasm32")]
mod ordermatch_wasm_db;

pub const ORDERBOOK_PREFIX: TopicPrefix = "orbk";
#[cfg(not(any(test, feature = "for-tests")))]
pub const MIN_ORDER_KEEP_ALIVE_INTERVAL: u64 = 30;
#[cfg(any(test, feature = "for-tests"))]
pub const MIN_ORDER_KEEP_ALIVE_INTERVAL: u64 = 5;
const BALANCE_REQUEST_INTERVAL: f64 = 30.;
const MAKER_ORDER_TIMEOUT: u64 = MIN_ORDER_KEEP_ALIVE_INTERVAL * 3;
const TAKER_ORDER_TIMEOUT: u64 = 30;
const ORDER_MATCH_TIMEOUT: u64 = 30;
const ORDERBOOK_REQUESTING_TIMEOUT: u64 = MIN_ORDER_KEEP_ALIVE_INTERVAL * 2;
const MAX_ORDERS_NUMBER_IN_ORDERBOOK_RESPONSE: usize = 1000;
const RECENTLY_CANCELLED_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(not(test))]
const TRIE_STATE_HISTORY_TIMEOUT: u64 = 14400;
#[cfg(test)]
const TRIE_STATE_HISTORY_TIMEOUT: u64 = 3;
#[cfg(not(test))]
const TRIE_ORDER_HISTORY_TIMEOUT: u64 = 300;
#[cfg(test)]
const TRIE_ORDER_HISTORY_TIMEOUT: u64 = 3;
/// Current swap protocol version
const SWAP_VERSION_DEFAULT: u8 = 2;

pub type OrderbookP2PHandlerResult = Result<(), MmError<OrderbookP2PHandlerError>>;

#[derive(Debug, Display)]
pub enum OrderbookP2PHandlerError {
    #[display(fmt = "'{_0}' is an invalid topic for the orderbook handler.")]
    InvalidTopic(String),

    #[display(fmt = "Message decoding was failed. Error: {_0}")]
    DecodeError(String),

    #[display(fmt = "Pubkey '{_0}' is not allowed.")]
    PubkeyNotAllowed(String),

    #[display(fmt = "P2P request error: {_0}")]
    P2PRequestError(String),

    #[display(fmt = "Couldn't find an order {_0}, ignoring, it will be synced upon pubkey keep alive")]
    OrderNotFound(Uuid),

    Internal(String),

    #[display(
        fmt = "Received stale keep alive from pubkey '{from_pubkey}' propagated from '{propagated_from}', will ignore it"
    )]
    StaleKeepAlive {
        from_pubkey: String,
        propagated_from: String,
    },

    #[display(
        fmt = "Sync failure for pubkey '{from_pubkey}' via '{propagated_from}'; unresolved pairs: {unresolved_pairs:?}"
    )]
    SyncFailure {
        from_pubkey: String,
        propagated_from: String,
        unresolved_pairs: Vec<String>,
    },
}

impl OrderbookP2PHandlerError {
    pub(crate) fn is_warning(&self) -> bool {
        // treat SyncFailure as a warning for now due to outdated nodes
        matches!(
            self,
            OrderbookP2PHandlerError::OrderNotFound(_) | OrderbookP2PHandlerError::SyncFailure { .. }
        )
    }
}

impl From<P2PRequestError> for OrderbookP2PHandlerError {
    fn from(e: P2PRequestError) -> Self {
        OrderbookP2PHandlerError::P2PRequestError(e.to_string())
    }
}

/// Alphabetically ordered orderbook pair
type AlbOrderedOrderbookPair = String;
type PubkeyOrders = Vec<(Uuid, OrderbookP2PItem)>;

pub type OrdermatchInitResult<T> = Result<T, MmError<OrdermatchInitError>>;

#[derive(Debug, Deserialize, Display, Serialize)]
pub enum OrdermatchInitError {
    #[display(fmt = "Error deserializing '{field}' config field: {error}")]
    ErrorDeserializingConfig {
        field: String,
        error: String,
    },
    Internal(String),
}

impl From<AbortedError> for OrdermatchInitError {
    fn from(e: AbortedError) -> Self {
        OrdermatchInitError::Internal(e.to_string())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CancelAllOrdersResponse {
    cancelled: Vec<Uuid>,
    currently_matching: Vec<Uuid>,
}

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum CancelAllOrdersError {
    LegacyError(String),
}

impl From<(new_protocol::MakerOrderCreated, String)> for OrderbookItem {
    fn from(tuple: (new_protocol::MakerOrderCreated, String)) -> OrderbookItem {
        let (order, pubkey) = tuple;

        OrderbookItem {
            pubkey,
            base: order.base,
            rel: order.rel,
            price: order.price,
            max_volume: order.max_volume,
            min_volume: order.min_volume,
            uuid: order.uuid.into(),
            created_at: order.created_at,
            base_protocol_info: order.base_protocol_info,
            rel_protocol_info: order.rel_protocol_info,
            conf_settings: Some(order.conf_settings),
        }
    }
}

pub fn addr_format_from_protocol_info(protocol_info: &[u8]) -> AddressFormat {
    match rmp_serde::from_slice::<AddressFormat>(protocol_info) {
        Ok(format) => format,
        Err(_) => AddressFormat::Standard,
    }
}

struct ProcessTrieParams<'a> {
    pubkey: &'a str,
    alb_pair: &'a str,
    protocol_infos: &'a HashMap<Uuid, BaseRelProtocolInfo>,
    conf_infos: &'a HashMap<Uuid, OrderConfirmationsSettings>,
}

fn process_pubkey_full_trie(
    orderbook: &mut Orderbook,
    new_trie_orders: PubkeyOrders,
    params: ProcessTrieParams,
) -> H64 {
    remove_pubkey_pair_orders(orderbook, params.pubkey, params.alb_pair);

    for (uuid, order) in new_trie_orders {
        orderbook.insert_or_update_order_update_trie(OrderbookItem::from_p2p_and_info(
            order,
            params.protocol_infos.get(&uuid).cloned().unwrap_or_default(),
            params.conf_infos.get(&uuid).cloned(),
        ));
    }

    let new_root = pubkey_state_mut(&mut orderbook.pubkeys_state, params.pubkey)
        .trie_roots
        .get(params.alb_pair)
        .copied()
        .unwrap_or_default();
    new_root
}

fn process_trie_delta(
    orderbook: &mut Orderbook,
    delta_orders: HashMap<Uuid, Option<OrderbookP2PItem>>,
    params: ProcessTrieParams,
) -> H64 {
    for (uuid, order) in delta_orders {
        match order {
            Some(order) => orderbook.insert_or_update_order_update_trie(OrderbookItem::from_p2p_and_info(
                order,
                params.protocol_infos.get(&uuid).cloned().unwrap_or_default(),
                params.conf_infos.get(&uuid).cloned(),
            )),
            None => {
                orderbook.remove_order_trie_update(uuid);
            },
        }
    }

    let new_root = match orderbook.pubkeys_state.get(params.pubkey) {
        Some(pubkey_state) => pubkey_state
            .trie_roots
            .get(params.alb_pair)
            .copied()
            .unwrap_or_default(),
        None => H64::default(),
    };
    new_root
}

fn build_pubkey_state_sync_request(
    pubkey: &str,
    pending_pairs: &HashSet<AlbOrderedOrderbookPair>,
    expected_pair_roots: &HashMap<AlbOrderedOrderbookPair, H64>,
) -> OrdermatchRequest {
    let trie_roots = pending_pairs
        .iter()
        .filter_map(|p| expected_pair_roots.get(p).map(|&root| (p.clone(), root)))
        .collect();
    OrdermatchRequest::SyncPubkeyOrderbookState {
        pubkey: pubkey.to_owned(),
        trie_roots,
    }
}

fn apply_pair_orders_diff(
    orderbook: &mut Orderbook,
    from_pubkey: &str,
    pair: &AlbOrderedOrderbookPair,
    diff: DeltaOrFullTrie<Uuid, OrderbookP2PItem>,
    protocol_infos: &HashMap<Uuid, BaseRelProtocolInfo>,
    conf_infos: &HashMap<Uuid, OrderConfirmationsSettings>,
) -> H64 {
    let params = ProcessTrieParams {
        pubkey: from_pubkey,
        alb_pair: pair,
        protocol_infos,
        conf_infos,
    };
    match diff {
        DeltaOrFullTrie::Delta(delta) => process_trie_delta(orderbook, delta, params),
        DeltaOrFullTrie::FullTrie(values) => process_pubkey_full_trie(orderbook, values, params),
    }
}

fn apply_and_validate_pubkey_state_sync_response(
    orderbook_mutex: &PaMutex<Orderbook>,
    from_pubkey: &str,
    peer: &str,
    response: SyncPubkeyOrderbookStateRes,
    expected_pair_roots: &HashMap<AlbOrderedOrderbookPair, H64>,
    pending_pairs: &mut HashSet<AlbOrderedOrderbookPair>,
    keep_alive_timestamp: u64,
) {
    let mut orderbook = orderbook_mutex.lock();
    for (pair, diff) in response.pair_orders_diff {
        // Ignore unsolicited pairs we didn't request to prevent state poisoning.
        if !pending_pairs.contains(&pair) {
            continue;
        }

        let new_root = apply_pair_orders_diff(
            &mut orderbook,
            from_pubkey,
            &pair,
            diff,
            &response.protocol_infos,
            &response.conf_infos,
        );

        if let Some(expected) = expected_pair_roots.get(&pair) {
            if &new_root == expected {
                pending_pairs.remove(&pair);
                // Mark per-pair maker-published timestamp once accepted
                let state = pubkey_state_mut(&mut orderbook.pubkeys_state, from_pubkey);
                state
                    .latest_root_timestamp_by_pair
                    .insert(pair.clone(), keep_alive_timestamp);
                state.pair_last_seen_local.insert(pair.clone(), now_sec());
            } else {
                warn!(
                    "Sync validation failed for pubkey {} pair {} from {}: expected {:?}, got {:?}. Reverting pair.",
                    from_pubkey, pair, peer, expected, new_root
                );
                remove_pubkey_pair_orders(&mut orderbook, from_pubkey, &pair);
            }
        }
    }
}

async fn request_and_apply_pubkey_state_sync_from_peer(
    ctx: &MmArc,
    orderbook: &PaMutex<Orderbook>,
    from_pubkey: &str,
    peer: &str,
    expected_pair_roots: &HashMap<AlbOrderedOrderbookPair, H64>,
    pending_pairs: &mut HashSet<AlbOrderedOrderbookPair>,
    keep_alive_timestamp: u64,
) -> OrderbookP2PHandlerResult {
    let current_req = build_pubkey_state_sync_request(from_pubkey, pending_pairs, expected_pair_roots);

    if let Some(resp) = request_one_peer::<SyncPubkeyOrderbookStateRes>(
        ctx.clone(),
        P2PRequest::Ordermatch(current_req),
        peer.to_string(),
    )
    .await
    .map_mm_err()?
    {
        apply_and_validate_pubkey_state_sync_response(
            orderbook,
            from_pubkey,
            peer,
            resp,
            expected_pair_roots,
            pending_pairs,
            keep_alive_timestamp,
        );
    }

    Ok(())
}

async fn process_orders_keep_alive(
    ctx: MmArc,
    propagated_from: String,
    from_pubkey: String,
    keep_alive: new_protocol::PubkeyKeepAlive,
    i_am_relay: bool,
) -> OrderbookP2PHandlerResult {
    let keep_alive_timestamp = keep_alive.timestamp;
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).expect("from_ctx failed");

    // Update local state and decide whether to sync.
    let trie_roots_to_request =
        ordermatch_ctx
            .orderbook
            .lock()
            .process_keep_alive(&from_pubkey, keep_alive, i_am_relay, &propagated_from)?;

    if trie_roots_to_request.is_empty() {
        // The message was processed, return Ok to forward it
        return Ok(());
    }

    // Query ONLY the keepalive propagator.
    let mut remaining_pairs: HashSet<AlbOrderedOrderbookPair> = trie_roots_to_request.keys().cloned().collect();
    let _ = request_and_apply_pubkey_state_sync_from_peer(
        &ctx,
        &ordermatch_ctx.orderbook,
        &from_pubkey,
        &propagated_from,
        &trie_roots_to_request,
        &mut remaining_pairs,
        keep_alive_timestamp,
    )
    .await;

    // Phase 4: Finalize; if unresolved, mark unsynced and DO NOT FORWARD.
    if !remaining_pairs.is_empty() {
        let unresolved_pairs = remaining_pairs.into_iter().collect::<Vec<_>>();
        return MmError::err(OrderbookP2PHandlerError::SyncFailure {
            from_pubkey,
            propagated_from,
            unresolved_pairs,
        });
    }

    Ok(())
}

#[inline]
fn process_maker_order_created(ctx: &MmArc, from_pubkey: String, created_msg: new_protocol::MakerOrderCreated) {
    let order: OrderbookItem = (created_msg, from_pubkey).into();
    insert_or_update_order(ctx, order);
}

fn process_maker_order_updated(
    ctx: MmArc,
    from_pubkey: String,
    updated_msg: new_protocol::MakerOrderUpdated,
) -> OrderbookP2PHandlerResult {
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).expect("from_ctx failed");
    let uuid = updated_msg.uuid();
    let mut orderbook = ordermatch_ctx.orderbook.lock();

    let mut order = orderbook
        .find_order_by_uuid_and_pubkey(&uuid, &from_pubkey)
        .ok_or_else(|| MmError::new(OrderbookP2PHandlerError::OrderNotFound(uuid)))?;
    order.apply_updated(&updated_msg);
    drop_mutability!(order);
    orderbook.insert_or_update_order_update_trie(order);

    Ok(())
}

fn process_maker_order_cancelled(ctx: &MmArc, from_pubkey: String, cancelled_msg: new_protocol::MakerOrderCancelled) {
    let uuid = Uuid::from(cancelled_msg.uuid);
    let ordermatch_ctx = OrdermatchContext::from_ctx(ctx).expect("from_ctx failed");
    let mut orderbook = ordermatch_ctx.orderbook.lock();
    // Add the order to the recently cancelled list to ignore it if a new order with the same uuid
    // is received within the `RECENTLY_CANCELLED_TIMEOUT` timeframe.
    // We do this even if the order is in the order_set, because it could have been added through
    // means other than the order creation message.
    orderbook
        .recently_cancelled
        .insert_expirable(uuid, from_pubkey.clone(), RECENTLY_CANCELLED_TIMEOUT);

    let maybe_pair = orderbook
        .order_set
        .get(&uuid)
        .filter(|o| o.pubkey == from_pubkey)
        .map(|o| alb_ordered_pair(&o.base, &o.rel));

    if let Some(alb_pair) = maybe_pair {
        orderbook.remove_order_trie_update(uuid);

        // Advance the per-pair maker timestamp floor to the cancel timestamp
        // so any earlier keep-alive for this pair will be rejected as stale.
        //
        // Note: we do NOT refresh liveness (pair_last_seen_local) here.
        // Liveness is refreshed only when we accept state-carrying messages
        // (e.g., a keep-alive that matches the expected root, a validated sync,
        // a MakerOrderCreated/Updated, or a full-trie fill), not on cancel.
        let state = pubkey_state_mut(&mut orderbook.pubkeys_state, &from_pubkey);
        state
            .latest_root_timestamp_by_pair
            .insert(alb_pair.clone(), cancelled_msg.timestamp);

        // If this cancel emptied the pair (root is zero/hashed-null), drop liveness for this pair.
        // This mirrors the zero-root keep-alive path and allows faster GC of empty pairs.
        if let Some(&pair_root) = state.trie_roots.get(&alb_pair) {
            if pair_root == H64::default() || pair_root == hashed_null_node::<Layout>() {
                state.pair_last_seen_local.remove(&alb_pair);
            }
        }
    }
}

// Some coins, for example ZHTLC, have privacy features like random keypair to sign P2P messages per every order.
// So, each order of such coin has unique «pubkey» field that doesn’t match node persistent pubkey derived from passphrase.
// We can compare pubkeys from maker_orders and from asks or bids, to find our order.
#[inline(always)]
fn is_my_order(order_pubkey: &str, my_pub: &Option<String>, my_p2p_pubkeys: &HashSet<String>) -> bool {
    my_pub.as_deref() == Some(order_pubkey) || my_p2p_pubkeys.contains(order_pubkey)
}

/// Request best asks and bids for the given `base` and `rel` coins from relays.
/// Set `asks_num` and/or `bids_num` to get corresponding number of best asks and bids or None to get all of the available orders.
///
/// # Safety
///
/// The function locks [`MmCtx::p2p_ctx`] and [`MmCtx::ordermatch_ctx`]
async fn request_and_fill_orderbook(ctx: &MmArc, base: &str, rel: &str) -> Result<(), String> {
    let request = OrdermatchRequest::GetOrderbook {
        base: base.to_string(),
        rel: rel.to_string(),
    };

    let i_am_seed = ctx.is_seed_node();
    let response = try_s!(request_any_relay::<GetOrderbookRes>(ctx.clone(), P2PRequest::Ordermatch(request)).await);
    let (pubkey_orders, protocol_infos, conf_infos) = match response {
        Some((
            GetOrderbookRes {
                pubkey_orders,
                protocol_infos,
                conf_infos,
            },
            _peer_id,
        )) => (pubkey_orders, protocol_infos, conf_infos),
        None => {
            if i_am_seed {
                warn!("No response received from any peer for GetOrderbook request");
                return Ok(());
            } else {
                return Err("No response received from any peer for GetOrderbook request".to_string());
            }
        },
    };

    let ordermatch_ctx = OrdermatchContext::from_ctx(ctx).unwrap();
    let mut orderbook = ordermatch_ctx.orderbook.lock();

    let my_pubsecp = mm2_internal_pubkey_hex(ctx, String::from).map_err(MmError::into_inner)?;

    let alb_pair = alb_ordered_pair(base, rel);
    for (pubkey, GetOrderbookPubkeyItem { orders, .. }) in pubkey_orders {
        let pubkey_bytes = match hex::decode(&pubkey) {
            Ok(b) => b,
            Err(e) => {
                warn!("Error {} decoding pubkey {}", e, pubkey);
                continue;
            },
        };

        let pubkey_without_prefix: [u8; 32] = match pubkey_bytes.get(1..).map(|slice| slice.try_into()) {
            Some(Ok(arr)) => arr,
            _ => {
                warn!("Invalid pubkey length (not 32 bytes) for {}", pubkey);
                continue;
            },
        };

        if is_my_order(&pubkey, &my_pubsecp, &orderbook.my_p2p_pubkeys) {
            continue;
        }

        if is_pubkey_banned(ctx, &pubkey_without_prefix.into()) {
            warn!("Pubkey {} is banned", pubkey);
            continue;
        }
        let params = ProcessTrieParams {
            pubkey: &pubkey,
            alb_pair: &alb_pair,
            protocol_infos: &protocol_infos,
            conf_infos: &conf_infos,
        };
        let _new_root = process_pubkey_full_trie(&mut orderbook, orders, params);

        let state = pubkey_state_mut(&mut orderbook.pubkeys_state, &pubkey);
        state.pair_last_seen_local.insert(alb_pair.clone(), now_sec());
    }

    let topic = orderbook_topic_from_base_rel(base, rel);
    orderbook
        .topics_subscribed_to
        .insert(topic, OrderbookRequestingState::Requested);

    Ok(())
}

/// Insert or update an order `req`.
/// Note this function locks the [`OrdermatchContext::orderbook`] async mutex.
fn insert_or_update_order(ctx: &MmArc, item: OrderbookItem) {
    let ordermatch_ctx = OrdermatchContext::from_ctx(ctx).expect("from_ctx failed");
    let mut orderbook = ordermatch_ctx.orderbook.lock();
    orderbook.insert_or_update_order_update_trie(item)
}

// use this function when notify maker order created
fn insert_or_update_my_order(ctx: &MmArc, item: OrderbookItem, my_order: &MakerOrder) {
    let ordermatch_ctx = OrdermatchContext::from_ctx(ctx).expect("from_ctx failed");
    let mut orderbook = ordermatch_ctx.orderbook.lock();
    orderbook.insert_or_update_order_update_trie(item);
    if let Some(key) = my_order.p2p_privkey {
        orderbook.my_p2p_pubkeys.insert(hex::encode(key.public_slice()));
    }
}

fn delete_my_order(ctx: &MmArc, uuid: Uuid, p2p_privkey: Option<SerializableSecp256k1Keypair>) {
    let ordermatch_ctx: Arc<OrdermatchContext> = OrdermatchContext::from_ctx(ctx).expect("from_ctx failed");
    let mut orderbook = ordermatch_ctx.orderbook.lock();
    orderbook.remove_order_trie_update(uuid);
    if let Some(key) = p2p_privkey {
        orderbook.my_p2p_pubkeys.remove(&hex::encode(key.public_slice()));
    }
}

pub(crate) fn mm2_internal_pubkey_hex<E, F>(ctx: &MmArc, err_construct: F) -> MmResult<Option<String>, E>
where
    E: NotMmError,
    F: Fn(String) -> E,
{
    match CryptoCtx::from_ctx(ctx).split_mm() {
        Ok(crypto_ctx) => Ok(Some(CryptoCtx::mm2_internal_pubkey_hex(crypto_ctx.as_ref()))),
        Err((CryptoCtxError::NotInitialized, _)) => Ok(None),
        Err((CryptoCtxError::Internal(error), trace)) => MmError::err_with_trace(err_construct(error), trace),
    }
}

fn remove_pubkey_pair_orders(orderbook: &mut Orderbook, pubkey: &str, alb_pair: &str) {
    let pubkey_state = match orderbook.pubkeys_state.get_mut(pubkey) {
        Some(state) => state,
        None => return,
    };

    if !pubkey_state.trie_roots.contains_key(alb_pair) {
        return;
    }

    pubkey_state.order_pairs_trie_state_history.remove(&alb_pair.to_owned());

    let mut orders_to_remove = Vec::with_capacity(pubkey_state.orders_uuids.len());
    pubkey_state.orders_uuids.retain(|(uuid, alb)| {
        if alb == alb_pair {
            orders_to_remove.push(*uuid);
            false
        } else {
            true
        }
    });

    for order in orders_to_remove {
        orderbook.remove_order_trie_update(order);
    }

    let pubkey_state = match orderbook.pubkeys_state.get_mut(pubkey) {
        Some(state) => state,
        None => return,
    };

    pubkey_state.trie_roots.remove(alb_pair);
}

pub async fn handle_orderbook_msg(
    ctx: MmArc,
    topic: &TopicHash,
    from_peer: String,
    msg: &[u8],
    i_am_relay: bool,
) -> OrderbookP2PHandlerResult {
    let topic_str = topic.as_str();
    let mut split = topic_str.split(TOPIC_SEPARATOR);
    if let (Some(ORDERBOOK_PREFIX), Some(_pair)) = (split.next(), split.next()) {
        process_msg(ctx, from_peer, msg, i_am_relay).await?;
        Ok(())
    } else {
        MmError::err(OrderbookP2PHandlerError::InvalidTopic(topic_str.to_owned()))
    }
}

/// Attempts to decode a message and process it returning whether the message is valid and worth rebroadcasting
pub async fn process_msg(ctx: MmArc, from_peer: String, msg: &[u8], i_am_relay: bool) -> OrderbookP2PHandlerResult {
    match decode_signed::<new_protocol::OrdermatchMessage>(msg) {
        Ok((message, _sig, pubkey)) => {
            {
                let my_persistent = mm2_internal_pubkey_hex(&ctx, String::from).ok().flatten();
                let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).expect("from_ctx failed");
                let my_p2p = &ordermatch_ctx.orderbook.lock().my_p2p_pubkeys;
                if is_my_order(&pubkey.to_hex(), &my_persistent, my_p2p) {
                    return Ok(());
                }
            }

            if is_pubkey_banned(&ctx, &pubkey.unprefixed().into()) {
                return MmError::err(OrderbookP2PHandlerError::PubkeyNotAllowed(pubkey.to_hex()));
            }
            log::debug!("received ordermatch message {:?}", message);
            match message {
                new_protocol::OrdermatchMessage::MakerOrderCreated(created_msg) => {
                    process_maker_order_created(&ctx, pubkey.to_hex(), created_msg);
                    Ok(())
                },
                new_protocol::OrdermatchMessage::PubkeyKeepAlive(keep_alive) => {
                    process_orders_keep_alive(ctx, from_peer, pubkey.to_hex(), keep_alive, i_am_relay).await
                },
                new_protocol::OrdermatchMessage::TakerRequest(taker_request) => {
                    let msg = TakerRequest::from_new_proto_and_pubkey(taker_request, pubkey.unprefixed().into());
                    process_taker_request(ctx, msg).await;
                    Ok(())
                },
                new_protocol::OrdermatchMessage::MakerReserved(maker_reserved) => {
                    let msg = MakerReserved::from_new_proto_and_pubkey(maker_reserved, pubkey.unprefixed().into());
                    // spawn because process_maker_reserved may take significant time to run
                    let spawner = ctx.spawner();
                    spawner.spawn(process_maker_reserved(ctx, msg));
                    Ok(())
                },
                new_protocol::OrdermatchMessage::TakerConnect(taker_connect) => {
                    process_taker_connect(ctx, pubkey, taker_connect.into()).await;
                    Ok(())
                },
                new_protocol::OrdermatchMessage::MakerConnected(maker_connected) => {
                    process_maker_connected(ctx, pubkey, maker_connected.into()).await;
                    Ok(())
                },
                new_protocol::OrdermatchMessage::MakerOrderCancelled(cancelled_msg) => {
                    process_maker_order_cancelled(&ctx, pubkey.to_hex(), cancelled_msg);
                    Ok(())
                },
                new_protocol::OrdermatchMessage::MakerOrderUpdated(updated_msg) => {
                    process_maker_order_updated(ctx, pubkey.to_hex(), updated_msg)
                },
            }
        },
        Err(e) => MmError::err(OrderbookP2PHandlerError::DecodeError(e.to_string())),
    }
}

#[allow(dead_code)]
#[derive(Debug)]
struct TryFromBytesError(String);

impl From<String> for TryFromBytesError {
    fn from(string: String) -> Self {
        TryFromBytesError(string)
    }
}

trait TryFromBytes {
    fn try_from_bytes(bytes: Vec<u8>) -> Result<Self, TryFromBytesError>
    where
        Self: Sized;
}

impl TryFromBytes for String {
    fn try_from_bytes(bytes: Vec<u8>) -> Result<Self, TryFromBytesError> {
        String::from_utf8(bytes).map_err(|e| ERRL!("{}", e).into())
    }
}

impl TryFromBytes for OrderbookP2PItem {
    fn try_from_bytes(bytes: Vec<u8>) -> Result<Self, TryFromBytesError> {
        rmp_serde::from_read(bytes.as_slice()).map_err(|e| ERRL!("{}", e).into())
    }
}

impl TryFromBytes for H64 {
    fn try_from_bytes(bytes: Vec<u8>) -> Result<Self, TryFromBytesError> {
        bytes.try_into().map_err(|e| ERRL!("{:?}", e).into())
    }
}

impl TryFromBytes for Uuid {
    fn try_from_bytes(bytes: Vec<u8>) -> Result<Self, TryFromBytesError> {
        Uuid::from_slice(&bytes).map_err(|e| ERRL!("{}", e).into())
    }
}

pub fn process_peer_request(ctx: MmArc, request: OrdermatchRequest) -> Result<Option<Vec<u8>>, String> {
    match request {
        OrdermatchRequest::GetOrderbook { base, rel } => process_get_orderbook_request(ctx, base, rel),
        OrdermatchRequest::SyncPubkeyOrderbookState { pubkey, trie_roots } => {
            let response = process_sync_pubkey_orderbook_state(ctx, pubkey, trie_roots);
            response.map(|res| res.map(|r| encode_message(&r).expect("Serialization failed")))
        },
        OrdermatchRequest::BestOrders { coin, action, volume } => {
            best_orders::process_best_orders_p2p_request(ctx, coin, action, volume)
        },
        OrdermatchRequest::BestOrdersByNumber { coin, action, number } => {
            best_orders::process_best_orders_p2p_request_by_number(ctx, coin, action, number)
        },
        OrdermatchRequest::OrderbookDepth { pairs } => orderbook_depth::process_orderbook_depth_p2p_request(ctx, pairs),
    }
}

type TrieProof = Vec<Vec<u8>>;

#[derive(Debug, Deserialize, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
struct GetOrderbookPubkeyItem {
    /// Timestamp of the latest keep alive message received.
    last_keep_alive: u64,
    /// last signed OrdermatchMessage payload
    last_signed_pubkey_payload: Vec<u8>,
    /// Requested orders.
    orders: PubkeyOrders,
}

/// Do not change this struct as it will break backward compatibility
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[cfg_attr(test, derive(PartialEq))]
struct BaseRelProtocolInfo {
    base: Vec<u8>,
    rel: Vec<u8>,
}

#[derive(Debug, Deserialize, Serialize)]
struct GetOrderbookRes {
    /// Asks and bids grouped by pubkey.
    pubkey_orders: HashMap<String, GetOrderbookPubkeyItem>,
    #[serde(default)]
    protocol_infos: HashMap<Uuid, BaseRelProtocolInfo>,
    #[serde(default)]
    conf_infos: HashMap<Uuid, OrderConfirmationsSettings>,
}

struct GetPubkeysOrdersRes {
    total_number_of_orders: usize,
    uuids_by_pubkey: HashMap<String, PubkeyOrders>,
    protocol_infos: HashMap<Uuid, BaseRelProtocolInfo>,
    conf_infos: HashMap<Uuid, OrderConfirmationsSettings>,
}

fn get_pubkeys_orders(orderbook: &Orderbook, base: String, rel: String) -> GetPubkeysOrdersRes {
    let asks = orderbook.unordered.get(&(base.clone(), rel.clone()));
    let bids = orderbook.unordered.get(&(rel, base));

    let asks_num = asks.map(|x| x.len()).unwrap_or(0);
    let bids_num = bids.map(|x| x.len()).unwrap_or(0);
    let total_number_of_orders = asks_num + bids_num;

    // flatten Option(asks) and Option(bids) to avoid cloning
    let orders = asks.iter().chain(bids.iter()).copied().flatten();

    let mut uuids_by_pubkey = HashMap::new();
    let mut protocol_infos = HashMap::new();
    let mut conf_infos = HashMap::new();
    for uuid in orders {
        let order = match orderbook.order_set.get(uuid) {
            Some(o) => o,
            None => {
                warn!(
                    "Orderbook::ordered contains uuid {} that is not in Orderbook::order_set",
                    uuid
                );
                continue;
            },
        };
        let uuids = uuids_by_pubkey.entry(order.pubkey.clone()).or_insert_with(Vec::new);
        protocol_infos.insert(order.uuid, order.base_rel_proto_info());
        if let Some(ref info) = order.conf_settings {
            conf_infos.insert(order.uuid, info.clone());
        }
        uuids.push((*uuid, order.clone().into()))
    }

    GetPubkeysOrdersRes {
        total_number_of_orders,
        uuids_by_pubkey,
        protocol_infos,
        conf_infos,
    }
}

fn process_get_orderbook_request(ctx: MmArc, base: String, rel: String) -> Result<Option<Vec<u8>>, String> {
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    let orderbook = ordermatch_ctx.orderbook.lock();

    let pubkeys_orders = get_pubkeys_orders(&orderbook, base, rel);
    if pubkeys_orders.total_number_of_orders > MAX_ORDERS_NUMBER_IN_ORDERBOOK_RESPONSE {
        return ERR!("Orderbook too large");
    }

    let orders_to_send = pubkeys_orders
        .uuids_by_pubkey
        .into_iter()
        .map(|(pubkey, orders)| {
            let pubkey_state = orderbook.pubkeys_state.get(&pubkey).ok_or(ERRL!(
                "Orderbook::pubkeys_state is expected to contain the {:?} pubkey",
                pubkey
            ))?;

            let pubkey_last_seen = pubkey_state.pair_last_seen_local.values().max().copied().unwrap_or(0);

            let item = GetOrderbookPubkeyItem {
                last_keep_alive: pubkey_last_seen,
                orders,
                // TODO save last signed payload to pubkey state
                last_signed_pubkey_payload: vec![],
            };

            Ok((pubkey, item))
        })
        .collect::<Result<HashMap<_, _>, String>>()?;

    let response = GetOrderbookRes {
        pubkey_orders: orders_to_send,
        protocol_infos: pubkeys_orders.protocol_infos,
        conf_infos: pubkeys_orders.conf_infos,
    };
    let encoded = try_s!(encode_message(&response));
    Ok(Some(encoded))
}

#[derive(Debug, Deserialize, Serialize)]
enum DeltaOrFullTrie<Key: Eq + std::hash::Hash, Value> {
    Delta(HashMap<Key, Option<Value>>),
    FullTrie(Vec<(Key, Value)>),
}

impl<Key: Eq + std::hash::Hash, V1> DeltaOrFullTrie<Key, V1> {
    pub fn map_to<V2: From<V1>>(self, mut on_each: impl FnMut(&Key, Option<&V1>)) -> DeltaOrFullTrie<Key, V2> {
        match self {
            DeltaOrFullTrie::Delta(delta) => {
                delta.iter().for_each(|(key, val)| on_each(key, val.as_ref()));
                let new_map = delta
                    .into_iter()
                    .map(|(key, value)| (key, value.map(From::from)))
                    .collect();
                DeltaOrFullTrie::Delta(new_map)
            },
            DeltaOrFullTrie::FullTrie(trie) => {
                trie.iter().for_each(|(key, val)| on_each(key, Some(val)));
                let new_trie = trie.into_iter().map(|(key, value)| (key, value.into())).collect();
                DeltaOrFullTrie::FullTrie(new_trie)
            },
        }
    }
}

#[expect(dead_code)]
#[derive(Debug)]
enum TrieDiffHistoryError {
    TrieDbError(Box<trie_db::TrieError<H64, sp_trie::Error>>),
    TryFromBytesError(TryFromBytesError),
    GetterNoneForKeyFromTrie,
}

impl std::fmt::Display for TrieDiffHistoryError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "({self:?})")
    }
}

impl From<TryFromBytesError> for TrieDiffHistoryError {
    fn from(error: TryFromBytesError) -> TrieDiffHistoryError {
        TrieDiffHistoryError::TryFromBytesError(error)
    }
}

impl From<Box<trie_db::TrieError<H64, sp_trie::Error>>> for TrieDiffHistoryError {
    fn from(error: Box<trie_db::TrieError<H64, sp_trie::Error>>) -> TrieDiffHistoryError {
        TrieDiffHistoryError::TrieDbError(error)
    }
}

fn get_full_trie<Key, Value>(
    trie_root: &H64,
    db: &MemoryDB<Blake2Hasher64>,
    getter: impl Fn(&Key) -> Option<Value>,
) -> Result<Vec<(Key, Value)>, TrieDiffHistoryError>
where
    Key: Clone + Eq + std::hash::Hash + TryFromBytes,
{
    let trie = TrieDB::<Layout>::new(db, trie_root)?;
    let trie: Result<Vec<_>, TrieDiffHistoryError> = trie
        .iter()?
        .map(|key_value| {
            let (key, _) = key_value?;
            let key = TryFromBytes::try_from_bytes(key)?;
            let val = getter(&key).ok_or(TrieDiffHistoryError::GetterNoneForKeyFromTrie)?;
            Ok((key, val))
        })
        .collect();
    trie
}

impl<Key: Clone + Eq + std::hash::Hash + TryFromBytes, Value: Clone> DeltaOrFullTrie<Key, Value> {
    fn from_history(
        history: &TrieDiffHistory<Key, Value>,
        from_hash: H64,
        actual_trie_root: H64,
        db: &MemoryDB<Blake2Hasher64>,
        getter: impl Fn(&Key) -> Option<Value>,
    ) -> Result<DeltaOrFullTrie<Key, Value>, TrieDiffHistoryError> {
        if let Some(delta) = history.get(&from_hash) {
            let mut current_delta = delta;
            let mut total_delta = HashMap::new();
            total_delta.extend(delta.delta.iter().cloned());
            while let Some(cur) = history.get(&current_delta.next_root) {
                current_delta = cur;
                total_delta.extend(current_delta.delta.iter().cloned());
            }
            if current_delta.next_root == actual_trie_root {
                return Ok(DeltaOrFullTrie::Delta(total_delta));
            }

            warn!(
                "History started from {:?} ends with not up-to-date trie root {:?}",
                from_hash, actual_trie_root
            );
        }

        let trie = get_full_trie(&actual_trie_root, db, getter)?;
        Ok(DeltaOrFullTrie::FullTrie(trie))
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct SyncPubkeyOrderbookStateRes {
    /// last signed OrdermatchMessage payload from pubkey
    last_signed_pubkey_payload: Vec<u8>,
    pair_orders_diff: HashMap<AlbOrderedOrderbookPair, DeltaOrFullTrie<Uuid, OrderbookP2PItem>>,
    #[serde(default)]
    protocol_infos: HashMap<Uuid, BaseRelProtocolInfo>,
    #[serde(default)]
    conf_infos: HashMap<Uuid, OrderConfirmationsSettings>,
}

fn process_sync_pubkey_orderbook_state(
    ctx: MmArc,
    pubkey: String,
    trie_roots: HashMap<AlbOrderedOrderbookPair, H64>,
) -> Result<Option<SyncPubkeyOrderbookStateRes>, String> {
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    let orderbook = ordermatch_ctx.orderbook.lock();
    let pubkey_state = some_or_return_ok_none!(orderbook.pubkeys_state.get(&pubkey));

    let order_getter = |uuid: &Uuid| orderbook.order_set.get(uuid).cloned();
    let pair_orders_diff: Result<HashMap<_, _>, _> = trie_roots
        .into_iter()
        .map(|(pair, root)| {
            let actual_pair_root = pubkey_state
                .trie_roots
                .get(&pair)
                .ok_or(ERRL!("No pair trie root for {}", pair))?;

            let delta_result = match pubkey_state.order_pairs_trie_state_history.get(&pair) {
                Some(history) => {
                    DeltaOrFullTrie::from_history(history, root, *actual_pair_root, &orderbook.memory_db, order_getter)
                },
                None => {
                    get_full_trie(actual_pair_root, &orderbook.memory_db, order_getter).map(DeltaOrFullTrie::FullTrie)
                },
            };

            let delta = try_s!(delta_result);
            Ok((pair, delta))
        })
        .collect();

    let pair_orders_diff = try_s!(pair_orders_diff);
    let mut protocol_infos = HashMap::new();
    let mut conf_infos = HashMap::new();
    let pair_orders_diff = pair_orders_diff
        .into_iter()
        .map(|(pair, trie)| {
            let new_trie = trie.map_to(|uuid, order| match order {
                Some(o) => {
                    protocol_infos.insert(
                        o.uuid,
                        BaseRelProtocolInfo {
                            base: o.base_protocol_info.clone(),
                            rel: o.rel_protocol_info.clone(),
                        },
                    );
                    if let Some(ref info) = o.conf_settings {
                        conf_infos.insert(o.uuid, info.clone());
                    }
                },
                None => {
                    protocol_infos.remove(uuid);
                    conf_infos.remove(uuid);
                },
            });
            (pair, new_trie)
        })
        .collect();
    let last_signed_pubkey_payload = vec![];
    let result = SyncPubkeyOrderbookStateRes {
        last_signed_pubkey_payload,
        pair_orders_diff,
        protocol_infos,
        conf_infos,
    };
    Ok(Some(result))
}

fn alb_ordered_pair(base: &str, rel: &str) -> AlbOrderedOrderbookPair {
    let (first, second) = if base < rel { (base, rel) } else { (rel, base) };
    let mut res = first.to_owned();
    res.push(':');
    res.push_str(second);
    res
}

fn orderbook_topic_from_base_rel(base: &str, rel: &str) -> String {
    pub_sub_topic(ORDERBOOK_PREFIX, &alb_ordered_pair(base, rel))
}

fn orderbook_topic_from_ordered_pair(pair: &str) -> String {
    pub_sub_topic(ORDERBOOK_PREFIX, pair)
}

#[test]
fn test_alb_ordered_pair() {
    assert_eq!("BTC:KMD", alb_ordered_pair("KMD", "BTC"));
    assert_eq!("BTCH:KMD", alb_ordered_pair("KMD", "BTCH"));
    assert_eq!("KMD:QTUM", alb_ordered_pair("QTUM", "KMD"));
}

#[allow(dead_code)]
pub fn parse_orderbook_pair_from_topic(topic: &str) -> Option<(&str, &str)> {
    let mut split = topic.split(TOPIC_SEPARATOR);
    match split.next() {
        Some(ORDERBOOK_PREFIX) => match split.next() {
            Some(maybe_pair) => {
                let colon = maybe_pair.find(':');
                match colon {
                    Some(index) => {
                        if index + 1 < maybe_pair.len() {
                            Some((&maybe_pair[..index], &maybe_pair[index + 1..]))
                        } else {
                            None
                        }
                    },
                    None => None,
                }
            },
            None => None,
        },
        _ => None,
    }
}

#[test]
fn test_parse_orderbook_pair_from_topic() {
    assert_eq!(Some(("BTC", "KMD")), parse_orderbook_pair_from_topic("orbk/BTC:KMD"));
    assert_eq!(None, parse_orderbook_pair_from_topic("orbk/BTC:"));
}

fn maker_order_created_p2p_notify(
    ctx: MmArc,
    order: &MakerOrder,
    base_protocol_info: Vec<u8>,
    rel_protocol_info: Vec<u8>,
) {
    let topic = order.orderbook_topic();
    let message = new_protocol::MakerOrderCreated {
        uuid: order.uuid.into(),
        base: order.base_orderbook_ticker().to_owned(),
        rel: order.rel_orderbook_ticker().to_owned(),
        price: order.price.to_ratio(),
        max_volume: order.available_amount().to_ratio(),
        min_volume: order.min_base_vol.to_ratio(),
        conf_settings: order.conf_settings.clone().unwrap(),
        created_at: now_sec(),
        timestamp: now_sec(),
        pair_trie_root: H64::default(),
        base_protocol_info,
        rel_protocol_info,
    };

    let to_broadcast = new_protocol::OrdermatchMessage::MakerOrderCreated(message.clone());
    let (key_pair, peer_id) = p2p_keypair_and_peer_id_to_broadcast(&ctx, order.p2p_keypair());

    let encoded_msg = match encode_and_sign(&to_broadcast, key_pair.private_ref()) {
        Ok(msg) => msg,
        Err(e) => {
            error!("Couldn't encode and sign the 'maker_order_created' message: {}", e);
            return;
        },
    };
    let item: OrderbookItem = (message, hex::encode(key_pair.public_slice())).into();
    insert_or_update_my_order(&ctx, item, order);
    broadcast_p2p_msg(&ctx, topic, encoded_msg, peer_id);
}

fn process_my_maker_order_updated(ctx: &MmArc, message: &new_protocol::MakerOrderUpdated) {
    let ordermatch_ctx = OrdermatchContext::from_ctx(ctx).expect("from_ctx failed");
    let mut orderbook = ordermatch_ctx.orderbook.lock();

    let uuid = message.uuid();
    if let Some(mut order) = orderbook.find_order_by_uuid(&uuid) {
        order.apply_updated(message);
        orderbook.insert_or_update_order_update_trie(order);
    }
}

fn maker_order_updated_p2p_notify(
    ctx: MmArc,
    topic: String,
    message: new_protocol::MakerOrderUpdated,
    p2p_privkey: Option<&KeyPair>,
) {
    let msg: new_protocol::OrdermatchMessage = message.clone().into();
    let (secret, peer_id) = p2p_private_and_peer_id_to_broadcast(&ctx, p2p_privkey);
    let encoded_msg = match encode_and_sign(&msg, &secret) {
        Ok(msg) => msg,
        Err(e) => {
            error!("Couldn't encode and sign the 'maker_order_updated' message: {}", e);
            return;
        },
    };
    process_my_maker_order_updated(&ctx, &message);
    broadcast_p2p_msg(&ctx, topic, encoded_msg, peer_id);
}

fn maker_order_cancelled_p2p_notify(ctx: &MmArc, order: &MakerOrder) {
    let message = new_protocol::OrdermatchMessage::MakerOrderCancelled(new_protocol::MakerOrderCancelled {
        uuid: order.uuid.into(),
        timestamp: now_sec(),
        pair_trie_root: H64::default(),
    });
    delete_my_order(ctx, order.uuid, order.p2p_privkey);
    log::debug!("maker_order_cancelled_p2p_notify called, message {:?}", message);
    broadcast_ordermatch_message(ctx, order.orderbook_topic(), message, order.p2p_keypair());
}

pub struct BalanceUpdateOrdermatchHandler {
    ctx: MmWeak,
}

impl BalanceUpdateOrdermatchHandler {
    pub fn new(ctx: MmArc) -> Self {
        BalanceUpdateOrdermatchHandler { ctx: ctx.weak() }
    }
}

#[async_trait]
impl BalanceTradeFeeUpdatedHandler for BalanceUpdateOrdermatchHandler {
    async fn balance_updated(&self, coin: &MmCoinEnum, new_balance: &BigDecimal) {
        let ctx = match MmArc::from_weak(&self.ctx) {
            Some(ctx) => ctx,
            None => return,
        };
        if coin.wallet_only(&ctx) {
            warn!(
                "coin: {} is wallet only, skip BalanceTradeFeeUpdatedHandler",
                coin.ticker()
            );
            return;
        }
        // Get the max maker available volume to check if the wallet balances are sufficient for the issued maker orders.
        // Note although the maker orders are issued already, but they are not matched yet, so pass the `OrderIssue` stage.
        let new_volume = match calc_max_maker_vol(&ctx, coin, new_balance, FeeApproxStage::OrderIssueMax).await {
            Ok(vol_info) => vol_info.volume,
            Err(e) if e.get_inner().not_sufficient_balance() => MmNumber::from(0),
            Err(e) => {
                warn!("Couldn't handle the 'balance_updated' event: {}", e);
                return;
            },
        };

        let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
        let my_maker_orders = ordermatch_ctx.maker_orders_ctx.lock().orders.clone();

        for (uuid, order_mutex) in my_maker_orders {
            let mut order = order_mutex.lock().await;
            if order.base != coin.ticker() {
                continue;
            }

            if new_volume < order.min_base_vol {
                let removed_order_mutex = ordermatch_ctx.maker_orders_ctx.lock().remove_order(&uuid);
                // This checks that the order hasn't been removed by another process
                if removed_order_mutex.is_some() {
                    // cancel the order
                    maker_order_cancelled_p2p_notify(&ctx, &order);
                    delete_my_maker_order(
                        ctx.clone(),
                        order.clone(),
                        MakerOrderCancellationReason::InsufficientBalance,
                    )
                    .compat()
                    .await
                    .ok();
                    continue;
                }
            }

            if new_volume < order.available_amount() {
                order.max_base_vol = &order.reserved_amount() + &new_volume;
                let mut update_msg = new_protocol::MakerOrderUpdated::new(order.uuid);
                update_msg.with_new_max_volume(order.available_amount().into());
                maker_order_updated_p2p_notify(ctx.clone(), order.orderbook_topic(), update_msg, order.p2p_keypair());
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TakerRequest {
    pub base: String,
    pub rel: String,
    pub base_amount: MmNumber,
    pub rel_amount: MmNumber,
    pub action: TakerAction,
    uuid: Uuid,
    sender_pubkey: H256Json,
    dest_pub_key: H256Json,
    #[serde(default)]
    match_by: MatchBy,
    conf_settings: Option<OrderConfirmationsSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_protocol_info: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel_protocol_info: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "SwapVersion::is_legacy")]
    pub swap_version: SwapVersion,
    #[cfg(feature = "ibc-routing-for-swaps")]
    order_metadata: OrderMetadata,
}

impl TakerRequest {
    fn from_new_proto_and_pubkey(message: new_protocol::TakerRequest, sender_pubkey: H256Json) -> Self {
        let base_amount = MmNumber::from(message.base_amount);
        let rel_amount = MmNumber::from(message.rel_amount);

        TakerRequest {
            base: message.base,
            rel: message.rel,
            base_amount,
            rel_amount,
            action: message.action,
            uuid: message.uuid.into(),
            sender_pubkey,
            dest_pub_key: Default::default(),
            match_by: message.match_by.into(),
            conf_settings: Some(message.conf_settings),
            base_protocol_info: message.base_protocol_info,
            rel_protocol_info: message.rel_protocol_info,
            swap_version: message.swap_version,
            // TODO: Support the new protocol types.
            #[cfg(feature = "ibc-routing-for-swaps")]
            order_metadata: OrderMetadata::default(),
        }
    }

    fn can_match_with_maker_pubkey(&self, maker_pubkey: &H256Json) -> bool {
        match &self.match_by {
            MatchBy::Pubkeys(pubkeys) => pubkeys.contains(maker_pubkey),
            _ => true,
        }
    }

    fn can_match_with_uuid(&self, uuid: &Uuid) -> bool {
        match &self.match_by {
            MatchBy::Orders(uuids) => uuids.contains(uuid),
            _ => true,
        }
    }

    fn base_protocol_info_for_maker(&self) -> &Option<Vec<u8>> {
        match &self.action {
            TakerAction::Buy => &self.base_protocol_info,
            TakerAction::Sell => &self.rel_protocol_info,
        }
    }

    fn rel_protocol_info_for_maker(&self) -> &Option<Vec<u8>> {
        match &self.action {
            TakerAction::Buy => &self.rel_protocol_info,
            TakerAction::Sell => &self.base_protocol_info,
        }
    }
}

impl From<TakerOrder> for new_protocol::OrdermatchMessage {
    fn from(taker_order: TakerOrder) -> Self {
        new_protocol::OrdermatchMessage::TakerRequest(new_protocol::TakerRequest {
            base_amount: taker_order.request.get_base_amount().to_ratio(),
            rel_amount: taker_order.request.get_rel_amount().to_ratio(),
            base: taker_order.base_orderbook_ticker().to_owned(),
            rel: taker_order.rel_orderbook_ticker().to_owned(),
            action: taker_order.request.action,
            uuid: taker_order.request.uuid.into(),
            match_by: taker_order.request.match_by.into(),
            conf_settings: taker_order.request.conf_settings.unwrap(),
            base_protocol_info: taker_order.request.base_protocol_info,
            rel_protocol_info: taker_order.request.rel_protocol_info,
            swap_version: taker_order.request.swap_version,
        })
    }
}

impl TakerRequest {
    fn get_base_amount(&self) -> &MmNumber {
        &self.base_amount
    }

    fn get_rel_amount(&self) -> &MmNumber {
        &self.rel_amount
    }
}

pub struct TakerOrderBuilder<'a> {
    base_coin: &'a MmCoinEnum,
    rel_coin: &'a MmCoinEnum,
    base_orderbook_ticker: Option<String>,
    rel_orderbook_ticker: Option<String>,
    base_amount: MmNumber,
    rel_amount: MmNumber,
    sender_pubkey: H256Json,
    action: TakerAction,
    match_by: MatchBy,
    order_type: OrderType,
    conf_settings: Option<OrderConfirmationsSettings>,
    min_volume: Option<MmNumber>,
    timeout: u64,
    save_in_history: bool,
    swap_version: u8,
    #[cfg(feature = "ibc-routing-for-swaps")]
    order_metadata: OrderMetadata,
}

pub enum TakerOrderBuildError {
    BaseEqualRel,
    /// Base amount too low with threshold
    BaseAmountTooLow {
        actual: MmNumber,
        threshold: MmNumber,
    },
    /// Rel amount too low with threshold
    RelAmountTooLow {
        actual: MmNumber,
        threshold: MmNumber,
    },
    /// Min volume too low with threshold
    MinVolumeTooLow {
        actual: MmNumber,
        threshold: MmNumber,
    },
    /// Max vol below min base vol
    MaxBaseVolBelowMinBaseVol {
        max: MmNumber,
        min: MmNumber,
    },
    SenderPubkeyIsZero,
    ConfsSettingsNotSet,
}

impl fmt::Display for TakerOrderBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TakerOrderBuildError::BaseEqualRel => write!(f, "Rel coin can not be same as base"),
            TakerOrderBuildError::BaseAmountTooLow { actual, threshold } => write!(
                f,
                "Base amount {} is too low, required: {}",
                actual.to_decimal(),
                threshold.to_decimal()
            ),
            TakerOrderBuildError::RelAmountTooLow { actual, threshold } => write!(
                f,
                "Rel amount {} is too low, required: {}",
                actual.to_decimal(),
                threshold.to_decimal()
            ),
            TakerOrderBuildError::MinVolumeTooLow { actual, threshold } => write!(
                f,
                "Min volume {} is too low, required: {}",
                actual.to_decimal(),
                threshold.to_decimal()
            ),
            TakerOrderBuildError::MaxBaseVolBelowMinBaseVol { min, max } => write!(
                f,
                "Max base vol {} is below min base vol: {}",
                max.to_decimal(),
                min.to_decimal()
            ),
            TakerOrderBuildError::SenderPubkeyIsZero => write!(f, "Sender pubkey can not be zero"),
            TakerOrderBuildError::ConfsSettingsNotSet => write!(f, "Confirmation settings must be set"),
        }
    }
}

impl<'a> TakerOrderBuilder<'a> {
    pub fn new(base_coin: &'a MmCoinEnum, rel_coin: &'a MmCoinEnum) -> TakerOrderBuilder<'a> {
        TakerOrderBuilder {
            base_coin,
            rel_coin,
            base_orderbook_ticker: None,
            rel_orderbook_ticker: None,
            base_amount: MmNumber::from(0),
            rel_amount: MmNumber::from(0),
            sender_pubkey: H256Json::default(),
            action: TakerAction::Buy,
            match_by: MatchBy::Any,
            conf_settings: None,
            min_volume: None,
            order_type: OrderType::GoodTillCancelled,
            timeout: TAKER_ORDER_TIMEOUT,
            save_in_history: true,
            swap_version: SWAP_VERSION_DEFAULT,
            #[cfg(feature = "ibc-routing-for-swaps")]
            order_metadata: OrderMetadata::default(),
        }
    }

    pub fn with_base_amount(mut self, vol: MmNumber) -> Self {
        self.base_amount = vol;
        self
    }

    pub fn with_rel_amount(mut self, vol: MmNumber) -> Self {
        self.rel_amount = vol;
        self
    }

    pub fn with_min_volume(mut self, vol: Option<MmNumber>) -> Self {
        self.min_volume = vol;
        self
    }

    pub fn with_action(mut self, action: TakerAction) -> Self {
        self.action = action;
        self
    }

    pub fn with_match_by(mut self, match_by: MatchBy) -> Self {
        self.match_by = match_by;
        self
    }

    fn with_order_type(mut self, order_type: OrderType) -> Self {
        self.order_type = order_type;
        self
    }

    pub fn with_conf_settings(mut self, settings: OrderConfirmationsSettings) -> Self {
        self.conf_settings = Some(settings);
        self
    }

    pub fn with_sender_pubkey(mut self, sender_pubkey: H256Json) -> Self {
        self.sender_pubkey = sender_pubkey;
        self
    }

    pub fn with_timeout(mut self, timeout: u64) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_save_in_history(mut self, save_in_history: bool) -> Self {
        self.save_in_history = save_in_history;
        self
    }

    pub fn with_base_orderbook_ticker(mut self, ticker: Option<String>) -> Self {
        self.base_orderbook_ticker = ticker;
        self
    }

    pub fn with_rel_orderbook_ticker(mut self, ticker: Option<String>) -> Self {
        self.rel_orderbook_ticker = ticker;
        self
    }

    /// When a new [TakerOrderBuilder::new] is created, it sets [SWAP_VERSION_DEFAULT].
    /// However, if user has not specified in the config to use TPU V2,
    /// the TakerOrderBuilder's swap_version is changed to legacy.
    /// In the future alls users will be using TPU V2 by default without "use_trading_proto_v2" configuration.
    pub fn set_legacy_swap_v(&mut self) {
        self.swap_version = legacy_swap_version()
    }

    /// Validate fields and build
    #[allow(clippy::result_large_err)]
    pub fn build(self) -> Result<TakerOrder, TakerOrderBuildError> {
        let min_base_amount = self.base_coin.min_trading_vol();
        let min_rel_amount = self.rel_coin.min_trading_vol();

        if self.base_coin.ticker() == self.rel_coin.ticker() {
            return Err(TakerOrderBuildError::BaseEqualRel);
        }

        if self.base_amount < min_base_amount {
            return Err(TakerOrderBuildError::BaseAmountTooLow {
                actual: self.base_amount,
                threshold: min_base_amount,
            });
        }

        if self.rel_amount < min_rel_amount {
            return Err(TakerOrderBuildError::RelAmountTooLow {
                actual: self.rel_amount,
                threshold: min_rel_amount,
            });
        }

        if self.sender_pubkey == H256Json::default() {
            return Err(TakerOrderBuildError::SenderPubkeyIsZero);
        }

        if self.conf_settings.is_none() {
            return Err(TakerOrderBuildError::ConfsSettingsNotSet);
        }

        let price = &self.rel_amount / &self.base_amount;
        let base_min_by_rel = &min_rel_amount / &price;
        let base_min_vol_threshold = min_base_amount.max(base_min_by_rel);

        let min_volume = self.min_volume.unwrap_or_else(|| base_min_vol_threshold.clone());

        if min_volume < base_min_vol_threshold {
            return Err(TakerOrderBuildError::MinVolumeTooLow {
                actual: min_volume,
                threshold: base_min_vol_threshold,
            });
        }

        if self.base_amount < min_volume {
            return Err(TakerOrderBuildError::MaxBaseVolBelowMinBaseVol {
                max: self.base_amount,
                min: min_volume,
            });
        }

        let my_coin = match &self.action {
            TakerAction::Buy => &self.rel_coin,
            TakerAction::Sell => &self.base_coin,
        };

        let p2p_privkey = if my_coin.is_privacy() {
            Some(SerializableSecp256k1Keypair::random())
        } else {
            None
        };

        let base_protocol_info = match &self.action {
            TakerAction::Buy => self.base_coin.coin_protocol_info(Some(self.base_amount.clone())),
            TakerAction::Sell => self.base_coin.coin_protocol_info(None),
        };

        let rel_protocol_info = match &self.action {
            TakerAction::Buy => self.rel_coin.coin_protocol_info(None),
            TakerAction::Sell => self.rel_coin.coin_protocol_info(Some(self.rel_amount.clone())),
        };

        Ok(TakerOrder {
            created_at: now_ms(),
            request: TakerRequest {
                base: self.base_coin.ticker().into(),
                rel: self.rel_coin.ticker().into(),
                base_amount: self.base_amount,
                rel_amount: self.rel_amount,
                action: self.action,
                uuid: new_uuid(),
                sender_pubkey: self.sender_pubkey,
                dest_pub_key: Default::default(),
                match_by: self.match_by,
                conf_settings: self.conf_settings,
                base_protocol_info: Some(base_protocol_info),
                rel_protocol_info: Some(rel_protocol_info),
                swap_version: SwapVersion::from(self.swap_version),
                #[cfg(feature = "ibc-routing-for-swaps")]
                order_metadata: self.order_metadata,
            },
            matches: Default::default(),
            min_volume,
            order_type: self.order_type,
            timeout: self.timeout,
            save_in_history: self.save_in_history,
            base_orderbook_ticker: self.base_orderbook_ticker,
            rel_orderbook_ticker: self.rel_orderbook_ticker,
            p2p_privkey,
        })
    }

    #[cfg(test)]
    /// skip validation for tests
    fn build_unchecked(self) -> TakerOrder {
        let base_protocol_info = match &self.action {
            TakerAction::Buy => self.base_coin.coin_protocol_info(Some(self.base_amount.clone())),
            TakerAction::Sell => self.base_coin.coin_protocol_info(None),
        };

        let rel_protocol_info = match &self.action {
            TakerAction::Buy => self.rel_coin.coin_protocol_info(None),
            TakerAction::Sell => self.rel_coin.coin_protocol_info(Some(self.rel_amount.clone())),
        };

        TakerOrder {
            created_at: now_ms(),
            request: TakerRequest {
                base: self.base_coin.ticker().to_owned(),
                rel: self.rel_coin.ticker().to_owned(),
                base_amount: self.base_amount,
                rel_amount: self.rel_amount,
                action: self.action,
                uuid: new_uuid(),
                sender_pubkey: self.sender_pubkey,
                dest_pub_key: Default::default(),
                match_by: self.match_by,
                conf_settings: self.conf_settings,
                base_protocol_info: Some(base_protocol_info),
                rel_protocol_info: Some(rel_protocol_info),
                swap_version: SwapVersion::from(self.swap_version),
                #[cfg(feature = "ibc-routing-for-swaps")]
                order_metadata: self.order_metadata,
            },
            matches: HashMap::new(),
            min_volume: Default::default(),
            order_type: Default::default(),
            timeout: self.timeout,
            save_in_history: false,
            base_orderbook_ticker: None,
            rel_orderbook_ticker: None,
            p2p_privkey: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TakerOrder {
    pub created_at: u64,
    pub request: TakerRequest,
    matches: HashMap<Uuid, TakerMatch>,
    min_volume: MmNumber,
    order_type: OrderType,
    timeout: u64,
    #[serde(default = "get_true")]
    save_in_history: bool,
    #[serde(default)]
    base_orderbook_ticker: Option<String>,
    #[serde(default)]
    rel_orderbook_ticker: Option<String>,
    /// A custom priv key for more privacy to prevent linking orders of the same node between each other
    /// Commonly used with privacy coins (ARRR, ZCash, etc.)
    p2p_privkey: Option<SerializableSecp256k1Keypair>,
}

/// Result of match_reserved function
#[derive(Debug, PartialEq)]
enum MatchReservedResult {
    /// Order and reserved message matched,
    Matched,
    /// Order and reserved didn't match
    NotMatched,
}

impl TakerOrder {
    fn is_cancellable(&self) -> bool {
        self.matches.is_empty()
    }

    fn match_reserved(&self, reserved: &MakerReserved) -> MatchReservedResult {
        match &self.request.match_by {
            MatchBy::Any => (),
            MatchBy::Orders(uuids) => {
                if !uuids.contains(&reserved.maker_order_uuid) {
                    return MatchReservedResult::NotMatched;
                }
            },
            MatchBy::Pubkeys(pubkeys) => {
                if !pubkeys.contains(&reserved.sender_pubkey) {
                    return MatchReservedResult::NotMatched;
                }
            },
        }

        let my_base_amount = self.request.get_base_amount();
        let my_rel_amount = self.request.get_rel_amount();
        let other_base_amount = reserved.get_base_amount();
        let other_rel_amount = reserved.get_rel_amount();

        match self.request.action {
            TakerAction::Buy => {
                let match_ticker = (self.request.base == reserved.base
                    || self.base_orderbook_ticker.as_ref() == Some(&reserved.base))
                    && (self.request.rel == reserved.rel || self.rel_orderbook_ticker.as_ref() == Some(&reserved.rel));
                if match_ticker && my_base_amount == other_base_amount && other_rel_amount <= my_rel_amount {
                    MatchReservedResult::Matched
                } else {
                    MatchReservedResult::NotMatched
                }
            },
            TakerAction::Sell => {
                let match_ticker = (self.request.base == reserved.rel
                    || self.base_orderbook_ticker.as_ref() == Some(&reserved.rel))
                    && (self.request.rel == reserved.base
                        || self.rel_orderbook_ticker.as_ref() == Some(&reserved.base));
                if match_ticker && my_base_amount == other_rel_amount && my_rel_amount <= other_base_amount {
                    MatchReservedResult::Matched
                } else {
                    MatchReservedResult::NotMatched
                }
            },
        }
    }

    /// Returns the ticker of the taker coin
    fn taker_coin_ticker(&self) -> &str {
        match &self.request.action {
            TakerAction::Buy => &self.request.rel,
            TakerAction::Sell => &self.request.base,
        }
    }

    /// Returns the ticker of the maker coin
    fn maker_coin_ticker(&self) -> &str {
        match &self.request.action {
            TakerAction::Buy => &self.request.base,
            TakerAction::Sell => &self.request.rel,
        }
    }

    fn base_orderbook_ticker(&self) -> &str {
        self.base_orderbook_ticker.as_deref().unwrap_or(&self.request.base)
    }

    fn rel_orderbook_ticker(&self) -> &str {
        self.rel_orderbook_ticker.as_deref().unwrap_or(&self.request.rel)
    }

    /// Returns the orderbook ticker of the taker coin
    fn taker_orderbook_ticker(&self) -> &str {
        match self.request.action {
            TakerAction::Buy => self.rel_orderbook_ticker(),
            TakerAction::Sell => self.base_orderbook_ticker(),
        }
    }

    /// Returns the orderbook ticker of the maker coin
    fn maker_orderbook_ticker(&self) -> &str {
        match self.request.action {
            TakerAction::Buy => self.base_orderbook_ticker(),
            TakerAction::Sell => self.rel_orderbook_ticker(),
        }
    }

    fn orderbook_topic(&self) -> String {
        orderbook_topic_from_base_rel(self.base_orderbook_ticker(), self.rel_orderbook_ticker())
    }

    fn p2p_keypair(&self) -> Option<&KeyPair> {
        self.p2p_privkey.as_ref().map(|key| key.key_pair())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
/// Market maker order
/// The "action" is missing here because it's easier to always consider maker order as "sell"
/// So upon ordermatch with request we have only 2 combinations "sell":"sell" and "sell":"buy"
/// Adding "action" to maker order will just double possible combinations making order match more complex.
pub struct MakerOrder {
    pub max_base_vol: MmNumber,
    pub min_base_vol: MmNumber,
    pub price: MmNumber,
    pub created_at: u64,
    pub updated_at: Option<u64>,
    pub base: String,
    pub rel: String,
    matches: HashMap<Uuid, MakerMatch>,
    started_swaps: Vec<Uuid>,
    uuid: Uuid,
    conf_settings: Option<OrderConfirmationsSettings>,
    // Keeping this for now for backward compatibility when kickstarting maker orders
    #[serde(skip_serializing_if = "Option::is_none")]
    changes_history: Option<Vec<HistoricalOrder>>,
    #[serde(default = "get_true")]
    save_in_history: bool,
    #[serde(default)]
    base_orderbook_ticker: Option<String>,
    #[serde(default)]
    rel_orderbook_ticker: Option<String>,
    /// A custom priv key for more privacy to prevent linking orders of the same node between each other
    /// Commonly used with privacy coins (ARRR, ZCash, etc.)
    p2p_privkey: Option<SerializableSecp256k1Keypair>,
    /// TODO: Move this into the `OrderMetadata` type when we are doing BC
    /// on orders already.
    #[serde(default, skip_serializing_if = "SwapVersion::is_legacy")]
    pub swap_version: SwapVersion,
    #[cfg(feature = "ibc-routing-for-swaps")]
    order_metadata: OrderMetadata,
    timeout_in_minutes: Option<u16>,
}

pub struct MakerOrderBuilder<'a> {
    max_base_vol: MmNumber,
    min_base_vol: Option<MmNumber>,
    price: MmNumber,
    base_coin: &'a MmCoinEnum,
    rel_coin: &'a MmCoinEnum,
    base_orderbook_ticker: Option<String>,
    rel_orderbook_ticker: Option<String>,
    conf_settings: Option<OrderConfirmationsSettings>,
    save_in_history: bool,
    swap_version: u8,
    #[cfg(feature = "ibc-routing-for-swaps")]
    order_metadata: OrderMetadata,
    timeout_in_minutes: Option<u16>,
}

/// Contains extra and/or optional metadata (e.g., protocol-specific information) that can
/// be used for both taker and maker orders.
///
/// TODO: `swap_version` should likely be moved into this type.
#[cfg(feature = "ibc-routing-for-swaps")]
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct OrderMetadata {
    channel_id_if_ibc_routing: Option<ChannelId>,
}

pub enum MakerOrderBuildError {
    BaseEqualRel,
    /// Max base vol too low with threshold
    MaxBaseVolTooLow {
        actual: MmNumber,
        threshold: MmNumber,
    },
    /// Min base vol too low with threshold
    MinBaseVolTooLow {
        actual: MmNumber,
        threshold: MmNumber,
    },
    /// Price too low with threshold
    PriceTooLow {
        actual: MmNumber,
        threshold: MmNumber,
    },
    /// Rel vol too low with threshold
    RelVolTooLow {
        actual: MmNumber,
        threshold: MmNumber,
    },
    ConfSettingsNotSet,
    MaxBaseVolBelowMinBaseVol {
        min: MmNumber,
        max: MmNumber,
    },
}

impl fmt::Display for MakerOrderBuildError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MakerOrderBuildError::BaseEqualRel => write!(f, "Rel coin can not be same as base"),
            MakerOrderBuildError::MaxBaseVolTooLow { actual, threshold } => write!(
                f,
                "Max base vol {} is too low, required: {}",
                actual.to_decimal(),
                threshold.to_decimal()
            ),
            MakerOrderBuildError::MinBaseVolTooLow { actual, threshold } => write!(
                f,
                "Min base vol {} is too low, required: {}",
                actual.to_decimal(),
                threshold.to_decimal()
            ),
            MakerOrderBuildError::PriceTooLow { actual, threshold } => write!(
                f,
                "Price {} is too low, required: {}",
                actual.to_decimal(),
                threshold.to_decimal()
            ),
            MakerOrderBuildError::RelVolTooLow { actual, threshold } => write!(
                f,
                "Max rel vol {} is too low, required: {}",
                actual.to_decimal(),
                threshold.to_decimal()
            ),
            MakerOrderBuildError::ConfSettingsNotSet => write!(f, "Confirmation settings must be set"),
            MakerOrderBuildError::MaxBaseVolBelowMinBaseVol { min, max } => write!(
                f,
                "Max base vol {} is below min base vol: {}",
                max.to_decimal(),
                min.to_decimal()
            ),
        }
    }
}

#[allow(clippy::result_large_err)]
fn validate_price(price: MmNumber) -> Result<(), MakerOrderBuildError> {
    let min_price = 0.into();

    if price <= min_price {
        return Err(MakerOrderBuildError::PriceTooLow {
            actual: price,
            threshold: min_price,
        });
    }

    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_and_get_min_vol(
    min_base_amount: MmNumber,
    min_rel_amount: MmNumber,
    min_base_vol: Option<MmNumber>,
    price: MmNumber,
) -> Result<MmNumber, MakerOrderBuildError> {
    let base_min_by_rel = min_rel_amount / price;
    let base_min_vol_threshold = min_base_amount.max(base_min_by_rel);
    let actual_min_base_vol = min_base_vol.unwrap_or_else(|| base_min_vol_threshold.clone());

    if actual_min_base_vol < base_min_vol_threshold {
        return Err(MakerOrderBuildError::MinBaseVolTooLow {
            actual: actual_min_base_vol,
            threshold: base_min_vol_threshold,
        });
    }

    Ok(actual_min_base_vol)
}

#[allow(clippy::result_large_err)]
fn validate_max_vol(
    min_base_amount: MmNumber,
    min_rel_amount: MmNumber,
    max_base_vol: MmNumber,
    min_base_vol: Option<MmNumber>,
    price: MmNumber,
) -> Result<(), MakerOrderBuildError> {
    if let Some(min) = min_base_vol {
        if max_base_vol < min {
            return Err(MakerOrderBuildError::MaxBaseVolBelowMinBaseVol { min, max: max_base_vol });
        }
    }

    if max_base_vol < min_base_amount {
        return Err(MakerOrderBuildError::MaxBaseVolTooLow {
            actual: max_base_vol,
            threshold: min_base_amount,
        });
    }

    let rel_vol = max_base_vol * price;
    if rel_vol < min_rel_amount {
        return Err(MakerOrderBuildError::RelVolTooLow {
            actual: rel_vol,
            threshold: min_rel_amount,
        });
    }

    Ok(())
}

impl<'a> MakerOrderBuilder<'a> {
    pub fn new(base_coin: &'a MmCoinEnum, rel_coin: &'a MmCoinEnum) -> MakerOrderBuilder<'a> {
        MakerOrderBuilder {
            base_coin,
            rel_coin,
            base_orderbook_ticker: None,
            rel_orderbook_ticker: None,
            max_base_vol: 0.into(),
            min_base_vol: None,
            price: 0.into(),
            conf_settings: None,
            save_in_history: true,
            swap_version: SWAP_VERSION_DEFAULT,
            #[cfg(feature = "ibc-routing-for-swaps")]
            order_metadata: OrderMetadata::default(),
            timeout_in_minutes: None,
        }
    }

    pub fn with_max_base_vol(mut self, vol: MmNumber) -> Self {
        self.max_base_vol = vol;
        self
    }

    pub fn with_min_base_vol(mut self, vol: Option<MmNumber>) -> Self {
        self.min_base_vol = vol;
        self
    }

    pub fn with_price(mut self, price: MmNumber) -> Self {
        self.price = price;
        self
    }

    pub fn with_conf_settings(mut self, conf_settings: OrderConfirmationsSettings) -> Self {
        self.conf_settings = Some(conf_settings);
        self
    }

    pub fn with_save_in_history(mut self, save_in_history: bool) -> Self {
        self.save_in_history = save_in_history;
        self
    }

    pub fn with_base_orderbook_ticker(mut self, base_orderbook_ticker: Option<String>) -> Self {
        self.base_orderbook_ticker = base_orderbook_ticker;
        self
    }

    pub fn with_rel_orderbook_ticker(mut self, rel_orderbook_ticker: Option<String>) -> Self {
        self.rel_orderbook_ticker = rel_orderbook_ticker;
        self
    }

    pub fn set_timeout(&mut self, timeout_in_minutes: u16) {
        self.timeout_in_minutes = Some(timeout_in_minutes);
    }

    /// When a new [MakerOrderBuilder::new] is created, it sets [SWAP_VERSION_DEFAULT].
    /// However, if user has not specified in the config to use TPU V2,
    /// the MakerOrderBuilder's swap_version is changed to legacy.
    /// In the future alls users will be using TPU V2 by default without "use_trading_proto_v2" configuration.
    pub fn set_legacy_swap_v(&mut self) {
        self.swap_version = legacy_swap_version()
    }

    /// Build MakerOrder
    #[allow(clippy::result_large_err)]
    pub fn build(self) -> Result<MakerOrder, MakerOrderBuildError> {
        if self.base_coin.ticker() == self.rel_coin.ticker() {
            return Err(MakerOrderBuildError::BaseEqualRel);
        }

        if self.conf_settings.is_none() {
            return Err(MakerOrderBuildError::ConfSettingsNotSet);
        }

        let min_base_amount = self.base_coin.min_trading_vol();
        let min_rel_amount = self.rel_coin.min_trading_vol();

        validate_price(self.price.clone())?;

        let actual_min_base_vol = validate_and_get_min_vol(
            min_base_amount.clone(),
            min_rel_amount.clone(),
            self.min_base_vol.clone(),
            self.price.clone(),
        )?;

        validate_max_vol(
            min_base_amount,
            min_rel_amount,
            self.max_base_vol.clone(),
            self.min_base_vol.clone(),
            self.price.clone(),
        )?;

        let created_at = now_ms();

        let p2p_privkey = if self.base_coin.is_privacy() {
            Some(SerializableSecp256k1Keypair::random())
        } else {
            None
        };

        Ok(MakerOrder {
            base: self.base_coin.ticker().to_owned(),
            rel: self.rel_coin.ticker().to_owned(),
            created_at,
            updated_at: Some(created_at),
            max_base_vol: self.max_base_vol,
            min_base_vol: actual_min_base_vol,
            price: self.price,
            matches: HashMap::new(),
            started_swaps: Vec::new(),
            uuid: new_uuid(),
            conf_settings: self.conf_settings,
            changes_history: None,
            save_in_history: self.save_in_history,
            base_orderbook_ticker: self.base_orderbook_ticker,
            rel_orderbook_ticker: self.rel_orderbook_ticker,
            p2p_privkey,
            swap_version: SwapVersion::from(self.swap_version),
            #[cfg(feature = "ibc-routing-for-swaps")]
            order_metadata: self.order_metadata,
            timeout_in_minutes: self.timeout_in_minutes,
        })
    }

    #[cfg(test)]
    fn build_unchecked(self) -> MakerOrder {
        let created_at = now_ms();
        #[allow(clippy::or_fun_call)]
        MakerOrder {
            base: self.base_coin.ticker().to_owned(),
            rel: self.rel_coin.ticker().to_owned(),
            created_at,
            updated_at: Some(created_at),
            max_base_vol: self.max_base_vol,
            min_base_vol: self.min_base_vol.unwrap_or(self.base_coin.min_trading_vol()),
            price: self.price,
            matches: HashMap::new(),
            started_swaps: Vec::new(),
            uuid: new_uuid(),
            conf_settings: self.conf_settings,
            changes_history: None,
            save_in_history: false,
            base_orderbook_ticker: None,
            rel_orderbook_ticker: None,
            p2p_privkey: None,
            swap_version: SwapVersion::from(self.swap_version),
            #[cfg(feature = "ibc-routing-for-swaps")]
            order_metadata: self.order_metadata,
            timeout_in_minutes: self.timeout_in_minutes,
        }
    }
}

#[allow(dead_code)]
fn zero_rat() -> BigRational {
    BigRational::zero()
}

impl MakerOrder {
    fn available_amount(&self) -> MmNumber {
        &self.max_base_vol - &self.reserved_amount()
    }

    fn reserved_amount(&self) -> MmNumber {
        self.matches.iter().fold(
            MmNumber::from(BigRational::from_integer(0.into())),
            |reserved, (_, order_match)| &reserved + order_match.reserved.get_base_amount(),
        )
    }

    fn is_cancellable(&self) -> bool {
        !self.has_ongoing_matches()
    }

    fn has_ongoing_matches(&self) -> bool {
        for (_, order_match) in self.matches.iter() {
            // if there's at least 1 ongoing match the order is not cancellable
            if order_match.connected.is_none() && order_match.connect.is_none() {
                return true;
            }
        }
        false
    }

    fn match_with_request(&self, taker: &TakerRequest) -> OrderMatchResult {
        let taker_base_amount = taker.get_base_amount();
        let taker_rel_amount = taker.get_rel_amount();

        let zero = MmNumber::from(0);
        if taker_base_amount <= &zero || taker_rel_amount <= &zero {
            return OrderMatchResult::NotMatched;
        }

        match taker.action {
            TakerAction::Buy => {
                let ticker_match = (self.base == taker.base
                    || self.base_orderbook_ticker.as_ref() == Some(&taker.base))
                    && (self.rel == taker.rel || self.rel_orderbook_ticker.as_ref() == Some(&taker.rel));
                let taker_price = taker_rel_amount / taker_base_amount;
                if ticker_match
                    && taker_base_amount <= &self.available_amount()
                    && taker_base_amount >= &self.min_base_vol
                    && taker_price >= self.price
                {
                    OrderMatchResult::Matched((taker_base_amount.clone(), taker_base_amount * &self.price))
                } else {
                    OrderMatchResult::NotMatched
                }
            },
            TakerAction::Sell => {
                let ticker_match = (self.base == taker.rel || self.base_orderbook_ticker.as_ref() == Some(&taker.rel))
                    && (self.rel == taker.base || self.rel_orderbook_ticker.as_ref() == Some(&taker.base));
                let taker_price = taker_base_amount / taker_rel_amount;

                // Calculate the resulting base amount using the Maker's price instead of the Taker's.
                let matched_base_amount = taker_base_amount / &self.price;
                let matched_rel_amount = taker_base_amount.clone();

                if ticker_match
                    && matched_base_amount <= self.available_amount()
                    && matched_base_amount >= self.min_base_vol
                    && taker_price >= self.price
                {
                    OrderMatchResult::Matched((matched_base_amount, matched_rel_amount))
                } else {
                    OrderMatchResult::NotMatched
                }
            },
        }
    }

    fn apply_updated(&mut self, msg: &new_protocol::MakerOrderUpdated) {
        if let Some(new_price) = msg.new_price() {
            self.price = new_price;
        }

        if let Some(new_max_volume) = msg.new_max_volume() {
            self.max_base_vol = new_max_volume;
        }

        if let Some(new_min_volume) = msg.new_min_volume() {
            self.min_base_vol = new_min_volume;
        }

        if let Some(conf_settings) = msg.new_conf_settings() {
            self.conf_settings = conf_settings.into();
        }

        self.updated_at = Some(now_ms());
    }

    fn base_orderbook_ticker(&self) -> &str {
        self.base_orderbook_ticker.as_deref().unwrap_or(&self.base)
    }

    fn rel_orderbook_ticker(&self) -> &str {
        self.rel_orderbook_ticker.as_deref().unwrap_or(&self.rel)
    }

    fn orderbook_topic(&self) -> String {
        orderbook_topic_from_base_rel(self.base_orderbook_ticker(), self.rel_orderbook_ticker())
    }

    fn was_updated(&self) -> bool {
        self.updated_at != Some(self.created_at)
    }

    fn p2p_keypair(&self) -> Option<&KeyPair> {
        self.p2p_privkey.as_ref().map(|key| key.key_pair())
    }
}

impl From<TakerOrder> for MakerOrder {
    fn from(taker_order: TakerOrder) -> Self {
        let created_at = now_ms();
        match taker_order.request.action {
            TakerAction::Sell => MakerOrder {
                price: (taker_order.request.get_rel_amount() / taker_order.request.get_base_amount()),
                max_base_vol: taker_order.request.get_base_amount().clone(),
                min_base_vol: taker_order.min_volume,
                created_at,
                updated_at: Some(created_at),
                base: taker_order.request.base,
                rel: taker_order.request.rel,
                matches: HashMap::new(),
                started_swaps: Vec::new(),
                uuid: taker_order.request.uuid,
                conf_settings: taker_order.request.conf_settings,
                changes_history: None,
                save_in_history: taker_order.save_in_history,
                base_orderbook_ticker: taker_order.base_orderbook_ticker,
                rel_orderbook_ticker: taker_order.rel_orderbook_ticker,
                p2p_privkey: taker_order.p2p_privkey,
                swap_version: taker_order.request.swap_version,
                // TODO: Add test coverage for this once we have an integration test for this feature.
                #[cfg(feature = "ibc-routing-for-swaps")]
                order_metadata: taker_order.request.order_metadata,
                timeout_in_minutes: None,
            },
            // The "buy" taker order is recreated with reversed pair as Maker order is always considered as "sell"
            TakerAction::Buy => {
                let price = taker_order.request.get_base_amount() / taker_order.request.get_rel_amount();
                let min_base_vol = &taker_order.min_volume / &price;
                MakerOrder {
                    price,
                    max_base_vol: taker_order.request.get_rel_amount().clone(),
                    min_base_vol,
                    created_at,
                    updated_at: Some(created_at),
                    base: taker_order.request.rel,
                    rel: taker_order.request.base,
                    matches: HashMap::new(),
                    started_swaps: Vec::new(),
                    uuid: taker_order.request.uuid,
                    conf_settings: taker_order.request.conf_settings.map(|s| s.reversed()),
                    changes_history: None,
                    save_in_history: taker_order.save_in_history,
                    base_orderbook_ticker: taker_order.rel_orderbook_ticker,
                    rel_orderbook_ticker: taker_order.base_orderbook_ticker,
                    p2p_privkey: taker_order.p2p_privkey,
                    swap_version: taker_order.request.swap_version,
                    // TODO: Add test coverage for this once we have an integration test for this feature.
                    #[cfg(feature = "ibc-routing-for-swaps")]
                    order_metadata: taker_order.request.order_metadata,
                    timeout_in_minutes: None,
                }
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TakerConnect {
    taker_order_uuid: Uuid,
    maker_order_uuid: Uuid,
    sender_pubkey: H256Json,
    dest_pub_key: H256Json,
}

impl From<new_protocol::TakerConnect> for TakerConnect {
    fn from(message: new_protocol::TakerConnect) -> TakerConnect {
        TakerConnect {
            taker_order_uuid: message.taker_order_uuid.into(),
            maker_order_uuid: message.maker_order_uuid.into(),
            sender_pubkey: Default::default(),
            dest_pub_key: Default::default(),
        }
    }
}

impl From<TakerConnect> for new_protocol::OrdermatchMessage {
    fn from(taker_connect: TakerConnect) -> Self {
        new_protocol::OrdermatchMessage::TakerConnect(new_protocol::TakerConnect {
            taker_order_uuid: taker_connect.taker_order_uuid.into(),
            maker_order_uuid: taker_connect.maker_order_uuid.into(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[cfg_attr(test, derive(Default))]
pub struct MakerReserved {
    base: String,
    rel: String,
    base_amount: MmNumber,
    rel_amount: MmNumber,
    taker_order_uuid: Uuid,
    maker_order_uuid: Uuid,
    sender_pubkey: H256Json,
    dest_pub_key: H256Json,
    conf_settings: Option<OrderConfirmationsSettings>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_protocol_info: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rel_protocol_info: Option<Vec<u8>>,
    #[serde(default, skip_serializing_if = "SwapVersion::is_legacy")]
    pub swap_version: SwapVersion,
    #[cfg(feature = "ibc-routing-for-swaps")]
    order_metadata: OrderMetadata,
}

impl MakerReserved {
    fn get_base_amount(&self) -> &MmNumber {
        &self.base_amount
    }

    fn get_rel_amount(&self) -> &MmNumber {
        &self.rel_amount
    }

    fn price(&self) -> MmNumber {
        &self.rel_amount / &self.base_amount
    }
}

impl MakerReserved {
    fn from_new_proto_and_pubkey(message: new_protocol::MakerReserved, sender_pubkey: H256Json) -> Self {
        let base_amount = MmNumber::from(message.base_amount);
        let rel_amount = MmNumber::from(message.rel_amount);

        MakerReserved {
            base: message.base,
            rel: message.rel,
            base_amount,
            rel_amount,
            taker_order_uuid: message.taker_order_uuid.into(),
            maker_order_uuid: message.maker_order_uuid.into(),
            sender_pubkey,
            dest_pub_key: Default::default(),
            conf_settings: Some(message.conf_settings),
            base_protocol_info: message.base_protocol_info,
            rel_protocol_info: message.rel_protocol_info,
            swap_version: message.swap_version,
            // TODO: Support the new protocol types.
            #[cfg(feature = "ibc-routing-for-swaps")]
            order_metadata: OrderMetadata::default(),
        }
    }
}

impl From<MakerReserved> for new_protocol::OrdermatchMessage {
    fn from(maker_reserved: MakerReserved) -> Self {
        new_protocol::OrdermatchMessage::MakerReserved(new_protocol::MakerReserved {
            base_amount: maker_reserved.get_base_amount().to_ratio(),
            rel_amount: maker_reserved.get_rel_amount().to_ratio(),
            base: maker_reserved.base,
            rel: maker_reserved.rel,
            taker_order_uuid: maker_reserved.taker_order_uuid.into(),
            maker_order_uuid: maker_reserved.maker_order_uuid.into(),
            conf_settings: maker_reserved.conf_settings.unwrap(),
            base_protocol_info: maker_reserved.base_protocol_info,
            rel_protocol_info: maker_reserved.rel_protocol_info,
            swap_version: maker_reserved.swap_version,
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerConnected {
    taker_order_uuid: Uuid,
    maker_order_uuid: Uuid,
    method: String,
    sender_pubkey: H256Json,
    dest_pub_key: H256Json,
}

impl From<new_protocol::MakerConnected> for MakerConnected {
    fn from(message: new_protocol::MakerConnected) -> MakerConnected {
        MakerConnected {
            taker_order_uuid: message.taker_order_uuid.into(),
            maker_order_uuid: message.maker_order_uuid.into(),
            method: "".to_string(),
            sender_pubkey: Default::default(),
            dest_pub_key: Default::default(),
        }
    }
}

impl From<MakerConnected> for new_protocol::OrdermatchMessage {
    fn from(maker_connected: MakerConnected) -> Self {
        new_protocol::OrdermatchMessage::MakerConnected(new_protocol::MakerConnected {
            taker_order_uuid: maker_connected.taker_order_uuid.into(),
            maker_order_uuid: maker_connected.maker_order_uuid.into(),
        })
    }
}

fn broadcast_keep_alive_for_pub(ctx: &MmArc, pubkey: &str, orderbook: &Orderbook, p2p_privkey: Option<&KeyPair>) {
    let state = match orderbook.pubkeys_state.get(pubkey) {
        Some(s) => s,
        None => return,
    };

    for (alb_pair, root) in state.trie_roots.iter() {
        let message = new_protocol::PubkeyKeepAlive {
            trie_roots: HashMap::from([(alb_pair.clone(), *root)]),
            timestamp: now_sec(),
        };

        broadcast_ordermatch_message(
            ctx,
            orderbook_topic_from_ordered_pair(alb_pair),
            message.clone().into(),
            p2p_privkey,
        );
    }
}

pub async fn broadcast_maker_orders_keep_alive_loop(ctx: MmArc) {
    // broadcast_maker_orders_keep_alive_loop is spawned only if CryptoCtx is initialized.
    let persistent_pubsecp = CryptoCtx::from_ctx(&ctx)
        .expect("CryptoCtx not available")
        .mm2_internal_pubkey_hex();

    while !ctx.is_stopping() {
        Timer::sleep(MIN_ORDER_KEEP_ALIVE_INTERVAL as f64).await;
        let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).expect("from_ctx failed");
        let my_orders = ordermatch_ctx.maker_orders_ctx.lock().orders.clone();

        for (_, order_mutex) in my_orders {
            let order = order_mutex.lock().await;
            if let Some(p2p_privkey) = order.p2p_privkey {
                // Artem Vitae
                // I tried if let Some(p2p_privkey) = order_mutex.lock().await.p2p_privkey
                // but it seems to keep holding the guard
                drop(order);
                let pubsecp = hex::encode(p2p_privkey.public_slice());
                let orderbook = ordermatch_ctx.orderbook.lock();
                broadcast_keep_alive_for_pub(&ctx, &pubsecp, &orderbook, Some(p2p_privkey.key_pair()));
            }
        }

        let orderbook = ordermatch_ctx.orderbook.lock();
        broadcast_keep_alive_for_pub(&ctx, &persistent_pubsecp, &orderbook, None);
    }
}

fn broadcast_ordermatch_message(
    ctx: &MmArc,
    topic: String,
    msg: new_protocol::OrdermatchMessage,
    p2p_privkey: Option<&KeyPair>,
) {
    let (secret, peer_id) = p2p_private_and_peer_id_to_broadcast(ctx, p2p_privkey);
    let encoded_msg = match encode_and_sign(&msg, &secret) {
        Ok(encoded_msg) => encoded_msg,
        Err(e) => {
            error!("Failed to encode and sign ordermatch message: {}", e);
            return;
        },
    };
    broadcast_p2p_msg(ctx, topic, encoded_msg, peer_id);
}

/// The order is ordered by [`OrderbookItem::price`] and [`OrderbookItem::uuid`].
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct OrderedByPriceOrder {
    price: MmNumber,
    uuid: Uuid,
}

#[derive(Clone, Debug, PartialEq)]
enum OrderbookRequestingState {
    /// The orderbook was requested from relays.
    #[allow(dead_code)]
    Requested,
    /// We subscribed to a topic at `subscribed_at` time, but the orderbook was not requested.
    NotRequested { subscribed_at: u64 },
}

type H64 = [u8; 8];

#[derive(Debug, Clone, Eq, PartialEq)]
struct TrieDiff<Key, Value> {
    delta: Vec<(Key, Option<Value>)>,
    next_root: H64,
}

#[derive(Debug)]
struct TrieDiffHistory<Key, Value> {
    inner: TimedMap<H64, TrieDiff<Key, Value>>,
}

impl<Key, Value> TrieDiffHistory<Key, Value> {
    fn insert_new_diff(&mut self, insert_at: H64, diff: TrieDiff<Key, Value>) {
        if insert_at == diff.next_root {
            // do nothing to avoid cycles in diff history
            return;
        }

        match self.inner.remove(&diff.next_root) {
            Some(mut diff) => {
                // we reached a state that was already reached previously
                // history can be cleaned up to this state hash
                while let Some(next_diff) = self.inner.remove(&diff.next_root) {
                    diff = next_diff;
                }
            },
            None => {
                self.inner
                    .insert_expirable(insert_at, diff, Duration::from_secs(TRIE_ORDER_HISTORY_TIMEOUT));
            },
        };
    }

    #[allow(dead_code)]
    fn remove_key(&mut self, key: H64) {
        self.inner.remove(&key);
    }

    #[allow(dead_code)]
    fn contains_key(&self, key: &H64) -> bool {
        self.inner.get(key).is_some()
    }

    fn get(&self, key: &H64) -> Option<&TrieDiff<Key, Value>> {
        self.inner.get(key)
    }

    #[allow(dead_code)]
    fn len(&self) -> usize {
        self.inner.len_unchecked()
    }
}

type TrieOrderHistory = TrieDiffHistory<Uuid, OrderbookItem>;

struct OrderbookPubkeyState {
    /// Monotonic maker-published timestamp for the last accepted root per pair.
    /// Used to ignore out-of-order or replayed keep-alive roots for specific pairs.
    ///
    /// Retention policy:
    /// We intentionally retain entries even after a pair is pruned by inactivity GC to defend against
    /// stale replays resurrecting old state. These entries are dropped only when the entire pubkey
    /// state is removed.
    latest_root_timestamp_by_pair: HashMap<AlbOrderedOrderbookPair, u64>,
    /// Local receive time (seconds) of the last accepted keep‑alive per alphabetically ordered pair
    /// owned by this pubkey. This is the authoritative liveness signal used by inactivity GC and
    /// trie‑state pruning. Pairs not updated within the timeout are purged; if all pairs are stale,
    /// the entire pubkey state is removed.
    /// Key: `AlbOrderedOrderbookPair` ("BASE:REL"), Value: local Unix time in seconds.
    pair_last_seen_local: HashMap<AlbOrderedOrderbookPair, u64>,
    /// The map storing historical data about specific pair subtrie changes
    /// Used to get diffs of orders of pair between specific root hashes
    order_pairs_trie_state_history: TimedMap<AlbOrderedOrderbookPair, TrieOrderHistory>,
    /// The known UUIDs owned by pubkey with alphabetically ordered pair to ease the lookup during pubkey orderbook requests
    orders_uuids: HashSet<(Uuid, AlbOrderedOrderbookPair)>,
    /// The map storing alphabetically ordered pair with trie root hash of orders owned by pubkey.
    trie_roots: HashMap<AlbOrderedOrderbookPair, H64>,
}

impl OrderbookPubkeyState {
    pub fn new() -> OrderbookPubkeyState {
        OrderbookPubkeyState {
            latest_root_timestamp_by_pair: HashMap::default(),
            pair_last_seen_local: HashMap::default(),
            order_pairs_trie_state_history: TimedMap::new_with_map_kind(MapKind::FxHashMap),
            orders_uuids: HashSet::default(),
            trie_roots: HashMap::default(),
        }
    }
}

fn get_trie_mut<'a>(
    mem_db: &'a mut MemoryDB<Blake2Hasher64>,
    root: &'a mut H64,
) -> Result<TrieDBMut<'a, Layout>, String> {
    if *root == H64::default() {
        Ok(TrieDBMut::new(mem_db, root))
    } else {
        TrieDBMut::from_existing(mem_db, root).map_err(|e| ERRL!("{:?}", e))
    }
}

fn pubkey_state_mut<'a>(
    state: &'a mut HashMap<String, OrderbookPubkeyState>,
    from_pubkey: &str,
) -> &'a mut OrderbookPubkeyState {
    state
        .entry(from_pubkey.to_owned())
        .or_insert_with(OrderbookPubkeyState::new)
}

fn order_pair_root_mut<'a>(state: &'a mut HashMap<AlbOrderedOrderbookPair, H64>, pair: &str) -> &'a mut H64 {
    #[allow(clippy::unwrap_or_default)]
    state.entry(pair.to_owned()).or_insert_with(Default::default)
}

/// `parity_util_mem::malloc_size` crushes for some reason on wasm32
#[cfg(target_arch = "wasm32")]
fn collect_orderbook_metrics(_ctx: &MmArc, _orderbook: &Orderbook) {}

#[cfg(not(target_arch = "wasm32"))]
fn collect_orderbook_metrics(ctx: &MmArc, orderbook: &Orderbook) {
    use parity_util_mem::malloc_size;

    let memory_db_size = malloc_size(&orderbook.memory_db);
    mm_gauge!(ctx.metrics, "orderbook.len", orderbook.order_set.len() as f64);
    mm_gauge!(ctx.metrics, "orderbook.memory_db", memory_db_size as f64);
}

struct Orderbook {
    /// A map from (base, rel).
    ordered: HashMap<(String, String), BTreeSet<OrderedByPriceOrder>>,
    /// A map from base ticker to the set of another tickers to track the existing pairs
    pairs_existing_for_base: HashMap<String, HashSet<String>>,
    /// A map from rel ticker to the set of another tickers to track the existing pairs
    pairs_existing_for_rel: HashMap<String, HashSet<String>>,
    /// A map from (base, rel).
    unordered: HashMap<(String, String), HashSet<Uuid>>,
    order_set: HashMap<Uuid, OrderbookItem>,
    /// a map of orderbook states of known maker pubkeys
    pubkeys_state: HashMap<String, OrderbookPubkeyState>,
    /// `TimedMap` of recently canceled orders, mapping `Uuid` to the maker pubkey as `String`,
    /// used to avoid order recreation in case of out-of-order p2p messages,
    /// e.g., when receiving the order cancellation message before the order is created.
    /// Entries are kept for `RECENTLY_CANCELLED_TIMEOUT` seconds.
    recently_cancelled: TimedMap<Uuid, String>,
    topics_subscribed_to: HashMap<String, OrderbookRequestingState>,
    /// MemoryDB instance to store Patricia Tries data
    memory_db: MemoryDB<Blake2Hasher64>,
    my_p2p_pubkeys: HashSet<String>,
    /// A copy of the streaming manager to stream orderbook events out.
    streaming_manager: StreamingManager,
}

impl Default for Orderbook {
    fn default() -> Self {
        Orderbook {
            ordered: HashMap::default(),
            pairs_existing_for_base: HashMap::default(),
            pairs_existing_for_rel: HashMap::default(),
            unordered: HashMap::default(),
            order_set: HashMap::default(),
            pubkeys_state: HashMap::default(),
            recently_cancelled: TimedMap::new_with_map_kind(MapKind::FxHashMap),
            topics_subscribed_to: HashMap::default(),
            memory_db: MemoryDB::default(),
            my_p2p_pubkeys: HashSet::default(),
            streaming_manager: Default::default(),
        }
    }
}

fn hashed_null_node<T: TrieConfiguration>() -> TrieHash<T> {
    <T::Codec as NodeCodecT>::hashed_null_node()
}

impl Orderbook {
    fn new(streaming_manager: StreamingManager) -> Orderbook {
        Orderbook {
            streaming_manager,
            ..Default::default()
        }
    }

    fn find_order_by_uuid_and_pubkey(&self, uuid: &Uuid, from_pubkey: &str) -> Option<OrderbookItem> {
        self.order_set.get(uuid).and_then(|order| {
            if order.pubkey == from_pubkey {
                Some(order.clone())
            } else {
                None
            }
        })
    }

    fn find_order_by_uuid(&self, uuid: &Uuid) -> Option<OrderbookItem> {
        self.order_set.get(uuid).cloned()
    }

    fn insert_or_update_order_update_trie(&mut self, order: OrderbookItem) {
        // Ignore the order if it was recently cancelled
        if self.recently_cancelled.get(&order.uuid) == Some(&order.pubkey) {
            warn!("Maker order {} was recently cancelled, ignoring", order.uuid);
            return;
        }

        let zero = BigRational::from_integer(0.into());
        if order.max_volume <= zero || order.price <= zero || order.min_volume < zero {
            self.remove_order_trie_update(order.uuid);
            return;
        } // else insert the order

        self.insert_or_update_order(order.clone());

        let pubkey_state = pubkey_state_mut(&mut self.pubkeys_state, &order.pubkey);

        let alb_ordered = alb_ordered_pair(&order.base, &order.rel);
        let pair_root = order_pair_root_mut(&mut pubkey_state.trie_roots, &alb_ordered);
        let prev_root = *pair_root;

        pubkey_state.pair_last_seen_local.insert(alb_ordered.clone(), now_sec());

        pubkey_state.orders_uuids.insert((order.uuid, alb_ordered.clone()));

        {
            let mut pair_trie = match get_trie_mut(&mut self.memory_db, pair_root) {
                Ok(trie) => trie,
                Err(e) => {
                    error!("Error getting {} trie with root {:?}", e, prev_root);
                    return;
                },
            };
            let order_bytes = order.trie_state_bytes();
            if let Err(e) = pair_trie.insert(order.uuid.as_bytes(), &order_bytes) {
                error!(
                    "Error {:?} on insertion to trie. Key {}, value {:?}",
                    e, order.uuid, order_bytes
                );
                return;
            };
        }

        if prev_root != H64::default() {
            let _ = pubkey_state
                .order_pairs_trie_state_history
                .update_expiration_status(alb_ordered.clone(), Duration::from_secs(TRIE_STATE_HISTORY_TIMEOUT));

            let history = match pubkey_state
                .order_pairs_trie_state_history
                .get_mut_unchecked(&alb_ordered)
            {
                Some(t) => t,
                None => {
                    pubkey_state.order_pairs_trie_state_history.insert_expirable(
                        alb_ordered.clone(),
                        TrieOrderHistory {
                            inner: TimedMap::new_with_map_kind(MapKind::FxHashMap),
                        },
                        Duration::from_secs(TRIE_STATE_HISTORY_TIMEOUT),
                    );

                    pubkey_state
                        .order_pairs_trie_state_history
                        .get_mut_unchecked(&alb_ordered)
                        .expect("must exist")
                },
            };

            history.insert_new_diff(
                prev_root,
                TrieDiff {
                    delta: vec![(order.uuid, Some(order.clone()))],
                    next_root: *pair_root,
                },
            );
        }
    }

    fn insert_or_update_order(&mut self, order: OrderbookItem) {
        log::debug!("Inserting order {:?}", order);
        let zero = BigRational::from_integer(0.into());
        if order.max_volume <= zero || order.price <= zero || order.min_volume < zero {
            self.remove_order_trie_update(order.uuid);
            return;
        } // else insert the order

        let base_rel = (order.base.clone(), order.rel.clone());

        let ordered = self.ordered.entry(base_rel.clone()).or_default();

        // have to clone to drop immutable ordered borrow
        let existing = ordered
            .iter()
            .find(|maybe_existing| maybe_existing.uuid == order.uuid)
            .cloned();

        if let Some(exists) = existing {
            ordered.remove(&exists);
        }
        ordered.insert(OrderedByPriceOrder {
            uuid: order.uuid,
            price: order.price.clone().into(),
        });

        self.pairs_existing_for_base
            .entry(order.base.clone())
            .or_default()
            .insert(order.rel.clone());

        self.pairs_existing_for_rel
            .entry(order.rel.clone())
            .or_default()
            .insert(order.base.clone());

        self.unordered.entry(base_rel).or_default().insert(order.uuid);

        self.streaming_manager
            .send_fn(
                &OrderbookStreamer::derive_streamer_id((&order.base, &order.rel)),
                || OrderbookItemChangeEvent::NewOrUpdatedItem(Box::new(order.clone().into())),
            )
            .ok();
        self.order_set.insert(order.uuid, order);
    }

    fn remove_order_trie_update(&mut self, uuid: Uuid) -> Option<OrderbookItem> {
        let order = self.order_set.remove(&uuid)?;
        let base_rel = (order.base.clone(), order.rel.clone());

        // create an `order_to_delete` that allows to find and remove an element from `self.ordered` by hash
        let order_to_delete = OrderedByPriceOrder {
            price: order.price.clone().into(),
            uuid,
        };

        if let Some(orders) = self.ordered.get_mut(&base_rel) {
            orders.remove(&order_to_delete);
            if orders.is_empty() {
                self.ordered.remove(&base_rel);
            }
        }

        if let Some(orders) = self.unordered.get_mut(&base_rel) {
            // use the same uuid to remove an order
            orders.remove(&order_to_delete.uuid);
            if orders.is_empty() {
                self.unordered.remove(&base_rel);
            }
        }

        let alb_ordered = alb_ordered_pair(&order.base, &order.rel);
        let pubkey_state = pubkey_state_mut(&mut self.pubkeys_state, &order.pubkey);
        let pair_state = order_pair_root_mut(&mut pubkey_state.trie_roots, &alb_ordered);
        let old_state = *pair_state;

        let to_remove = &(uuid, alb_ordered.clone());
        pubkey_state.orders_uuids.remove(to_remove);

        *pair_state = match delta_trie_root::<Layout, _, _, _, _, _>(
            &mut self.memory_db,
            *pair_state,
            vec![(*order.uuid.as_bytes(), None::<Vec<u8>>)],
        ) {
            Ok(root) => root,
            Err(_) => {
                error!("Failed to get existing trie with root {:?}", pair_state);
                return Some(order);
            },
        };

        let _ = pubkey_state
            .order_pairs_trie_state_history
            .update_expiration_status(alb_ordered.clone(), Duration::from_secs(TRIE_STATE_HISTORY_TIMEOUT));

        if let Some(history) = pubkey_state
            .order_pairs_trie_state_history
            .get_mut_unchecked(&alb_ordered)
        {
            history.insert_new_diff(
                old_state,
                TrieDiff {
                    delta: vec![(uuid, None)],
                    next_root: *pair_state,
                },
            );
        }

        self.streaming_manager
            .send_fn(
                &OrderbookStreamer::derive_streamer_id((&order.base, &order.rel)),
                || OrderbookItemChangeEvent::RemovedItem(order.uuid),
            )
            .ok();
        Some(order)
    }

    fn is_subscribed_to(&self, topic: &str) -> bool {
        self.topics_subscribed_to.contains_key(topic)
    }

    /// Processes a [`new_protocol::PubkeyKeepAlive`] for `from_pubkey`, updating per‑pair state and
    /// deciding whether a state sync is required.
    ///
    /// This function performs only local state transitions; it does no I/O.
    /// It may:
    /// - Accept the announcement (per pair) and refresh timestamps if the announced root matches the
    ///   local root.
    /// - Clear a pair if the announced root is null/empty (remove all orders, remember the null root,
    ///   update the maker timestamp floor, and drop the local “last seen” so GC can prune it).
    /// - Detect divergence and return the set of pairs that must be synced to the announced roots.
    ///
    /// Invariants and validation:
    /// - Maker timestamps are checked per pair and must be strictly increasing. If any pair is stale
    ///   (announced timestamp ≤ the last accepted maker timestamp for that pair), the function returns
    ///   [`OrderbookP2PHandlerError::StaleKeepAlive`] and no state is advanced for that message. Callers
    ///   must not propagate such a message.
    /// - We update `pair_last_seen_local` only for pairs that are accepted/in‑sync. Divergent pairs do
    ///   not refresh liveness until validated to the expected root.
    ///
    /// Subscription policy:
    /// - Pairs we are not subscribed to are ignored unless this node acts as a relay (`i_am_relay`).
    ///
    /// Returns:
    /// - A map of `"BASE:REL"` to the expected roots (as announced) for which local state diverges and
    ///   a sync is required. An empty map means no sync is needed and the message may be propagated.
    ///
    /// Caller responsibilities:
    /// - If the returned map is non‑empty, fetch trie diffs from the peer that propagated the keep‑alive
    ///   (`propagated_from`), validate them to the expected roots, apply, and only then consider forwarding
    ///   the keep‑alive.
    ///
    /// Errors:
    /// - [`OrderbookP2PHandlerError::StaleKeepAlive`] if any per‑pair maker timestamp regresses or repeats.
    /// - Other variants for malformed or otherwise invalid data.
    ///
    /// Notes:
    /// - Maker timestamps are used for monotonicity/anti‑replay; GC uses local receive times.
    /// - The caller must ignore Unsolicited pairs in sync responses when applying the response.
    fn process_keep_alive(
        &mut self,
        from_pubkey: &str,
        message: new_protocol::PubkeyKeepAlive,
        i_am_relay: bool,
        propagated_from: &str,
    ) -> Result<HashMap<AlbOrderedOrderbookPair, H64>, MmError<OrderbookP2PHandlerError>> {
        // TODO(rate-limit):
        // Add a single, shared in‑process rate limiter for OrdermatchMessage
        // types (e.g., a module in mm2_p2p or lp_network).
        // For keep-alive, enforce a minimum interval per pubkey (>= 20–30s) or X messages/minute.
        // When throttled, early‑return without syncing/propagating, before any state mutation.

        // Pre-scan: if any single pair is stale => treat the whole message as stale.
        // Note: We currently send keep-alives per pair (trie_roots has a single entry),
        // so this is equivalent to checking that one pair. If multi-pair keep-alives return,
        // demote only stale pairs instead of rejecting the whole message.
        {
            let pubkey_state = pubkey_state_mut(&mut self.pubkeys_state, from_pubkey);
            for (alb_pair, _) in message.trie_roots.iter() {
                let subscribed = self
                    .topics_subscribed_to
                    .contains_key(&orderbook_topic_from_ordered_pair(alb_pair));
                if !subscribed && !i_am_relay {
                    continue;
                }

                let last_pair_timestamp = *pubkey_state.latest_root_timestamp_by_pair.get(alb_pair).unwrap_or(&0);

                if message.timestamp <= last_pair_timestamp {
                    log::debug!(
                    "Ignoring a stale PubkeyKeepAlive from {} for pair {}: message.timestamp={} <= last_pair_timestamp={}",
                    from_pubkey, alb_pair, message.timestamp, last_pair_timestamp
                    );
                    return MmError::err(OrderbookP2PHandlerError::StaleKeepAlive {
                        from_pubkey: from_pubkey.to_owned(),
                        propagated_from: propagated_from.to_owned(),
                    });
                }
            }
        }

        let mut trie_roots_to_request = HashMap::new();

        for (alb_pair, trie_root) in message.trie_roots {
            let subscribed = self
                .topics_subscribed_to
                .contains_key(&orderbook_topic_from_ordered_pair(&alb_pair));
            if !subscribed && !i_am_relay {
                continue;
            }

            // Empty/null root: clear local orders for (pubkey, pair),
            // remember the null root, and don't request a sync.
            if trie_root == H64::default() || trie_root == hashed_null_node::<Layout>() {
                log::debug!(
                    "Received zero or hashed_null_node pair {} trie root from pub {}",
                    alb_pair,
                    from_pubkey
                );

                // Clear local orders for this pubkey/pair.
                remove_pubkey_pair_orders(self, from_pubkey, &alb_pair);

                // Remember that the latest known root for this pair is null/empty.
                {
                    let pubkey_state = pubkey_state_mut(&mut self.pubkeys_state, from_pubkey);
                    pubkey_state.trie_roots.insert(alb_pair.clone(), trie_root);
                    pubkey_state
                        .latest_root_timestamp_by_pair
                        .insert(alb_pair.clone(), message.timestamp);
                    pubkey_state.pair_last_seen_local.remove(&alb_pair);
                }

                continue;
            }

            // For non-null roots, compare with our current view and request sync if it differs.
            let current_root = {
                let pubkey_state = pubkey_state_mut(&mut self.pubkeys_state, from_pubkey);
                *order_pair_root_mut(&mut pubkey_state.trie_roots, &alb_pair)
            };
            if current_root != trie_root {
                trie_roots_to_request.insert(alb_pair, trie_root);
                continue;
            }
            let state = pubkey_state_mut(&mut self.pubkeys_state, from_pubkey);
            state
                .latest_root_timestamp_by_pair
                .insert(alb_pair.clone(), message.timestamp);
            state.pair_last_seen_local.insert(alb_pair, now_sec());
        }

        Ok(trie_roots_to_request)
    }

    fn orderbook_item_with_proof(&self, order: OrderbookItem) -> OrderbookItemWithProof {
        OrderbookItemWithProof {
            order,
            last_message_payload: vec![],
            proof: vec![],
        }
    }
}

struct OrdermatchContext {
    pub maker_orders_ctx: PaMutex<MakerOrdersContext>,
    pub my_taker_orders: AsyncMutex<HashMap<Uuid, TakerOrder>>,
    pub orderbook: PaMutex<Orderbook>,
    /// The map from coin original ticker to the orderbook ticker
    /// It is used to share the same orderbooks for concurrently activated coins with different protocols
    /// E.g. BTC and BTC-Segwit
    pub orderbook_tickers: HashMap<String, String>,
    /// The map from orderbook ticker to original tickers having it in the config
    pub original_tickers: HashMap<String, HashSet<String>>,
    /// Pending MakerReserved messages for a specific TakerOrder UUID
    /// Used to select a trade with the best price upon matching
    pending_maker_reserved: AsyncMutex<HashMap<Uuid, Vec<MakerReserved>>>,
    #[cfg(target_arch = "wasm32")]
    ordermatch_db: ConstructibleDb<OrdermatchDb>,
}

pub fn init_ordermatch_context(ctx: &MmArc) -> OrdermatchInitResult<()> {
    // Helper
    #[derive(Deserialize)]
    struct CoinConf {
        coin: String,
        orderbook_ticker: Option<String>,
    }

    let coins: Vec<CoinConf> =
        json::from_value(ctx.conf["coins"].clone()).map_to_mm(|e| OrdermatchInitError::ErrorDeserializingConfig {
            field: "coins".to_owned(),
            error: e.to_string(),
        })?;
    let mut orderbook_tickers = HashMap::new();
    let mut original_tickers = HashMap::new();
    for coin in coins {
        if let Some(orderbook_ticker) = coin.orderbook_ticker {
            orderbook_tickers.insert(coin.coin.clone(), orderbook_ticker.clone());
            original_tickers
                .entry(orderbook_ticker)
                .or_insert_with(HashSet::new)
                .insert(coin.coin);
        }
    }

    let ordermatch_context = OrdermatchContext {
        maker_orders_ctx: PaMutex::new(MakerOrdersContext::new(ctx)?),
        my_taker_orders: Default::default(),
        orderbook: PaMutex::new(Orderbook::new(ctx.event_stream_manager.clone())),
        pending_maker_reserved: Default::default(),
        orderbook_tickers,
        original_tickers,
        #[cfg(target_arch = "wasm32")]
        ordermatch_db: ConstructibleDb::new(ctx),
    };

    from_ctx(&ctx.ordermatch_ctx, move || Ok(ordermatch_context))
        .map(|_| ())
        .map_to_mm(OrdermatchInitError::Internal)
}

#[cfg_attr(all(test, not(target_arch = "wasm32")), mockable)]
impl OrdermatchContext {
    /// Obtains a reference to this crate context, creating it if necessary.
    #[cfg(not(test))]
    fn from_ctx(ctx: &MmArc) -> Result<Arc<OrdermatchContext>, String> {
        Ok(try_s!(from_ctx(&ctx.ordermatch_ctx, move || {
            ERR!("'OrdermatchContext' is not initialized")
        })))
    }

    /// Obtains a reference to this crate context, creating it if necessary.
    #[cfg(test)]
    fn from_ctx(ctx: &MmArc) -> Result<Arc<OrdermatchContext>, String> {
        Ok(try_s!(from_ctx(&ctx.ordermatch_ctx, move || {
            Ok(OrdermatchContext {
                maker_orders_ctx: PaMutex::new(try_s!(MakerOrdersContext::new(ctx))),
                my_taker_orders: Default::default(),
                orderbook: PaMutex::new(Orderbook::new(ctx.event_stream_manager.clone())),
                pending_maker_reserved: Default::default(),
                orderbook_tickers: Default::default(),
                original_tickers: Default::default(),
                #[cfg(target_arch = "wasm32")]
                ordermatch_db: ConstructibleDb::new(ctx),
            })
        })))
    }

    fn orderbook_ticker(&self, ticker: &str) -> Option<String> {
        self.orderbook_tickers.get(ticker).cloned()
    }

    fn orderbook_ticker_bypass(&self, ticker: &str) -> String {
        self.orderbook_ticker(ticker).unwrap_or_else(|| ticker.to_owned())
    }

    fn orderbook_pair_bypass(&self, pair: &(String, String)) -> (String, String) {
        (
            self.orderbook_ticker(&pair.0).unwrap_or_else(|| pair.0.clone()),
            self.orderbook_ticker(&pair.1).unwrap_or_else(|| pair.1.clone()),
        )
    }

    #[cfg(target_arch = "wasm32")]
    pub async fn ordermatch_db(&self) -> InitDbResult<OrdermatchDbLocked<'_>> {
        self.ordermatch_db.get_or_initialize().await
    }
}

pub struct MakerOrdersContext {
    orders: TimedMap<Uuid, Arc<AsyncMutex<MakerOrder>>>,
    order_tickers: HashMap<Uuid, String>,
    count_by_tickers: HashMap<String, usize>,
    /// The `check_balance_update_loop` future abort handles associated stored by corresponding tickers.
    balance_loops: AbortableSimpleMap<String>,
}

impl MakerOrdersContext {
    fn new(ctx: &MmArc) -> OrdermatchInitResult<MakerOrdersContext> {
        // Create an abortable system linked to the `MmCtx` so if the context is stopped via `MmArc::stop`,
        // all spawned `check_balance_update_loop` futures will be aborted as well.
        let balance_loops = ctx.abortable_system.create_subsystem()?;

        Ok(MakerOrdersContext {
            orders: TimedMap::new_with_map_kind(timed_map::MapKind::FxHashMap),
            order_tickers: HashMap::new(),
            count_by_tickers: HashMap::new(),
            balance_loops,
        })
    }

    fn add_order(&mut self, ctx: MmWeak, order: MakerOrder, balance: Option<BigDecimal>) {
        self.spawn_balance_loop_if_not_spawned(ctx, order.base.clone(), balance);

        self.order_tickers.insert(order.uuid, order.base.clone());
        *self.count_by_tickers.entry(order.base.clone()).or_insert(0) += 1;

        if let Some(t) = order.timeout_in_minutes {
            // Use unchecked write to skip automatic cleanup as we need to handle
            // expired orders manually.
            self.orders.insert_expirable_unchecked(
                order.uuid,
                Arc::new(AsyncMutex::new(order)),
                Duration::from_secs(t as u64 * 60),
            );
        } else {
            // Use unchecked write to skip automatic cleanup as we need to handle
            // expired orders manually.
            self.orders
                .insert_constant_unchecked(order.uuid, Arc::new(AsyncMutex::new(order)));
        }
    }

    fn get_order(&self, uuid: &Uuid) -> Option<&Arc<AsyncMutex<MakerOrder>>> {
        self.orders.get(uuid)
    }

    fn remove_order(&mut self, uuid: &Uuid) -> Option<Arc<AsyncMutex<MakerOrder>>> {
        let order = self.orders.remove(uuid)?;
        let ticker = self.order_tickers.remove(uuid)?;
        if let Some(count) = self.count_by_tickers.get_mut(&ticker) {
            if *count > 0 {
                *count -= 1;
            }
        }

        if !self.coin_has_active_maker_orders(&ticker) {
            self.stop_balance_loop(&ticker);
        }

        Some(order)
    }

    fn coin_has_active_maker_orders(&self, ticker: &str) -> bool {
        self.count_by_tickers.get(ticker).copied() > Some(0)
    }

    fn spawn_balance_loop_if_not_spawned(&mut self, ctx: MmWeak, order_base: String, balance: Option<BigDecimal>) {
        let ticker = order_base.clone();
        let mut balance_loops = self.balance_loops.lock();

        let fut = check_balance_update_loop(ctx, ticker, balance);
        // `SimpleMapImpl::spawn_or_ignore` won't spawn the future
        // if the `check_balance_update_loop` loop has been spawned already.
        balance_loops.spawn_or_ignore(order_base, fut).warn_log();
    }

    fn stop_balance_loop(&mut self, ticker: &str) {
        self.balance_loops.lock().abort_future(ticker).warn_log();
    }

    #[cfg(test)]
    fn balance_loop_exists(&mut self, ticker: &str) -> bool {
        self.balance_loops.lock().contains(ticker).unwrap()
    }
}

struct LegacySwapParams<'a> {
    maker_coin: &'a MmCoinEnum,
    taker_coin: &'a MmCoinEnum,
    uuid: &'a Uuid,
    my_conf_settings: &'a SwapConfirmationsSettings,
    my_persistent_pub: &'a H264,
    maker_amount: &'a MmNumber,
    taker_amount: &'a MmNumber,
    locktime: &'a u64,
}
struct StateMachineParams<'a> {
    secret_hash_algo: &'a SecretHashAlgo,
    uuid: &'a Uuid,
    my_conf_settings: &'a SwapConfirmationsSettings,
    locktime: &'a u64,
    maker_amount: &'a MmNumber,
    taker_amount: &'a MmNumber,
}

#[allow(unreachable_code, unused_variables)] // TODO: remove with `ibc-routing-for-swaps` feature removal.
#[cfg_attr(test, mockable)]
fn lp_connect_start_bob(ctx: MmArc, maker_match: MakerMatch, maker_order: MakerOrder, taker_p2p_pubkey: PublicKey) {
    let spawner = ctx.spawner();
    let uuid = maker_match.request.uuid;

    let fut = async move {
        // aka "maker_loop"
        let taker_coin = match lp_coinfind(&ctx, &maker_order.rel).await {
            Ok(Some(c)) => c,
            Ok(None) => {
                error!("Coin {} is not found/enabled", maker_order.rel);
                return;
            },
            Err(e) => {
                error!("!lp_coinfind({}): {}", maker_order.rel, e);
                return;
            },
        };

        let maker_coin = match lp_coinfind(&ctx, &maker_order.base).await {
            Ok(Some(c)) => c,
            Ok(None) => {
                error!("Coin {} is not found/enabled", maker_order.base);
                return;
            },
            Err(e) => {
                error!("!lp_coinfind({}): {}", maker_order.base, e);
                return;
            },
        };
        let taker_pubkey = bits256::from(maker_match.request.sender_pubkey.0);
        let maker_amount = maker_match.reserved.get_base_amount().clone();
        let taker_amount = maker_match.reserved.get_rel_amount().clone();

        #[cfg(feature = "ibc-routing-for-swaps")]
        {
            let _taker_order_metadata = &maker_match.request.order_metadata;
            let _maker_order_metadata = &maker_order.order_metadata;

            // TODO
            //   - If this is non-HTLC tendermint swap, cross-check IBC channels for routing before start.
            //   - Could malformed orders trick us by intentionally modfying channel IDs?
            //   - Unify this logic with `lp_connected_alice`.
            unreachable!();
        }

        // lp_connect_start_bob is called only from process_taker_connect, which returns if CryptoCtx is not initialized
        let crypto_ctx = CryptoCtx::from_ctx(&ctx).expect("'CryptoCtx' must be initialized already");
        let raw_priv = crypto_ctx.mm2_internal_privkey_secret();
        let my_persistent_pub = compressed_pub_key_from_priv_raw(&raw_priv.take(), ChecksumType::DSHA256).unwrap();

        let my_conf_settings = choose_maker_confs_and_notas(
            maker_order.conf_settings.clone(),
            &maker_match.request,
            &maker_coin,
            &taker_coin,
        );
        // detect atomic lock time version implicitly by conf_settings existence in taker request
        let atomic_locktime_v = match maker_match.request.conf_settings {
            Some(_) => {
                let other_conf_settings = choose_taker_confs_and_notas(
                    &maker_match.request,
                    &maker_match.reserved.conf_settings,
                    &maker_coin,
                    &taker_coin,
                );
                AtomicLocktimeVersion::V2 {
                    my_conf_settings,
                    other_conf_settings,
                }
            },
            None => AtomicLocktimeVersion::V1,
        };
        let lock_time = lp_atomic_locktime(
            maker_order.base_orderbook_ticker(),
            maker_order.rel_orderbook_ticker(),
            atomic_locktime_v,
        );
        log_tag!(
            ctx,
            "";
            fmt = "Entering the maker_swap_loop {}/{} with uuid: {}",
            maker_coin.ticker(),
            taker_coin.ticker(),
            uuid
        );

        let secret = match generate_secret() {
            Ok(s) => s.into(),
            Err(e) => {
                error!("Error {} on secret generation", e);
                return;
            },
        };

        let alice_swap_v = maker_match.request.swap_version;
        let bob_swap_v = maker_order.swap_version;

        // Start a legacy swap if either the taker or maker uses the legacy swap protocol (version 1)
        if alice_swap_v.is_legacy() || bob_swap_v.is_legacy() {
            let params = LegacySwapParams {
                maker_coin: &maker_coin,
                taker_coin: &taker_coin,
                uuid: &uuid,
                my_conf_settings: &my_conf_settings,
                my_persistent_pub: &my_persistent_pub,
                maker_amount: &maker_amount,
                taker_amount: &taker_amount,
                locktime: &lock_time,
            };
            start_maker_legacy_swap(&ctx, maker_order, taker_pubkey, secret, params).await;
            return;
        }

        // Ensure detect_secret_hash_algo_v2 returns the correct secret hash algorithm when adding new coin support in TPU.
        let params = StateMachineParams {
            secret_hash_algo: &detect_secret_hash_algo_v2(&maker_coin, &taker_coin),
            uuid: &uuid,
            my_conf_settings: &my_conf_settings,
            locktime: &lock_time,
            maker_amount: &maker_amount,
            taker_amount: &taker_amount,
        };
        let taker_p2p_pubkey = match taker_p2p_pubkey {
            PublicKey::Secp256k1(pubkey) => pubkey.into(),
        };

        // TODO try to handle it more gracefully during project redesign
        match (&maker_coin, &taker_coin) {
            (MmCoinEnum::UtxoCoin(m), MmCoinEnum::UtxoCoin(t)) => {
                start_maker_swap_state_machine(&ctx, &maker_order, &taker_p2p_pubkey, &secret, m, t, &params).await;
            },
            (MmCoinEnum::EthCoin(m), MmCoinEnum::EthCoin(t)) => {
                start_maker_swap_state_machine(&ctx, &maker_order, &taker_p2p_pubkey, &secret, m, t, &params).await;
            },
            (MmCoinEnum::UtxoCoin(m), MmCoinEnum::EthCoin(t)) => {
                start_maker_swap_state_machine(&ctx, &maker_order, &taker_p2p_pubkey, &secret, m, t, &params).await;
            },
            (MmCoinEnum::EthCoin(m), MmCoinEnum::UtxoCoin(t)) => {
                start_maker_swap_state_machine(&ctx, &maker_order, &taker_p2p_pubkey, &secret, m, t, &params).await;
            },
            _ => {
                let params = LegacySwapParams {
                    maker_coin: &maker_coin,
                    taker_coin: &taker_coin,
                    uuid: &uuid,
                    my_conf_settings: &my_conf_settings,
                    my_persistent_pub: &my_persistent_pub,
                    maker_amount: &maker_amount,
                    taker_amount: &taker_amount,
                    locktime: &lock_time,
                };
                start_maker_legacy_swap(&ctx, maker_order, taker_pubkey, secret, params).await
            },
        }
    };

    let settings = AbortSettings::info_on_abort(format!("swap {uuid} stopped!"));
    spawner.spawn_with_settings(fut, settings);
}

async fn start_maker_legacy_swap(
    ctx: &MmArc,
    maker_order: MakerOrder,
    taker_pubkey: bits256,
    secret: H256,
    params: LegacySwapParams<'_>,
) {
    if let Err(e) = insert_new_swap_to_db(
        ctx.clone(),
        params.maker_coin.ticker(),
        params.taker_coin.ticker(),
        *params.uuid,
        now_sec(),
        LEGACY_SWAP_TYPE,
    )
    .await
    {
        error!("Error {} on new swap insertion", e);
    }

    let maker_swap = MakerSwap::new(
        ctx.clone(),
        taker_pubkey,
        params.maker_amount.to_decimal(),
        params.taker_amount.to_decimal(),
        *params.my_persistent_pub,
        *params.uuid,
        Some(maker_order.uuid),
        *params.my_conf_settings,
        params.maker_coin.clone(),
        params.taker_coin.clone(),
        *params.locktime,
        maker_order.p2p_privkey.map(SerializableSecp256k1Keypair::into_inner),
        secret,
    );
    run_maker_swap(RunMakerSwapInput::StartNew(maker_swap), ctx.clone()).await;
}

async fn start_maker_swap_state_machine<
    MakerCoin: MmCoin + MakerCoinSwapOpsV2 + Clone,
    TakerCoin: MmCoin + TakerCoinSwapOpsV2 + Clone,
>(
    ctx: &MmArc,
    maker_order: &MakerOrder,
    taker_p2p_pubkey: &Secp256k1Pubkey,
    secret: &H256,
    maker_coin: &MakerCoin,
    taker_coin: &TakerCoin,
    params: &StateMachineParams<'_>,
) {
    let mut maker_swap_state_machine = MakerSwapStateMachine {
        storage: MakerSwapStorage::new(ctx.clone()),
        abortable_system: ctx
            .abortable_system
            .create_subsystem()
            .expect("create_subsystem should not fail"),
        ctx: ctx.clone(),
        started_at: now_sec(),
        maker_coin: maker_coin.clone(),
        maker_volume: params.maker_amount.clone(),
        secret: *secret,
        taker_coin: taker_coin.clone(),
        taker_volume: params.taker_amount.clone(),
        taker_premium: Default::default(),
        conf_settings: *params.my_conf_settings,
        p2p_topic: swap_v2_topic(params.uuid),
        uuid: *params.uuid,
        p2p_keypair: maker_order.p2p_privkey.map(SerializableSecp256k1Keypair::into_inner),
        secret_hash_algo: *params.secret_hash_algo,
        lock_duration: *params.locktime,
        taker_p2p_pubkey: *taker_p2p_pubkey,
        require_taker_payment_spend_confirm: true,
        swap_version: maker_order.swap_version.version,
    };
    #[allow(clippy::box_default)]
    maker_swap_state_machine
        .run(Box::new(maker_swap_v2::Initialize::default()))
        .await
        .error_log();
}

#[allow(unreachable_code, unused_variables)] // TODO: remove with `ibc-routing-for-swaps` feature removal.
fn lp_connected_alice(ctx: MmArc, taker_order: TakerOrder, taker_match: TakerMatch, maker_p2p_pubkey: PublicKey) {
    let spawner = ctx.spawner();
    let uuid = taker_match.reserved.taker_order_uuid;

    let fut = async move {
        // aka "taker_loop"
        let maker_pubkey = bits256::from(taker_match.reserved.sender_pubkey.0);
        let taker_coin_ticker = taker_order.taker_coin_ticker();
        let taker_coin = match lp_coinfind(&ctx, taker_coin_ticker).await {
            Ok(Some(c)) => c,
            Ok(None) => {
                error!("Coin {} is not found/enabled", taker_coin_ticker);
                return;
            },
            Err(e) => {
                error!("!lp_coinfind({}): {}", taker_coin_ticker, e);
                return;
            },
        };

        let maker_coin_ticker = taker_order.maker_coin_ticker();
        let maker_coin = match lp_coinfind(&ctx, maker_coin_ticker).await {
            Ok(Some(c)) => c,
            Ok(None) => {
                error!("Coin {} is not found/enabled", maker_coin_ticker);
                return;
            },
            Err(e) => {
                error!("!lp_coinfind({}): {}", maker_coin_ticker, e);
                return;
            },
        };

        // lp_connected_alice is called only from process_maker_connected, which returns if CryptoCtx is not initialized
        let crypto_ctx = CryptoCtx::from_ctx(&ctx).expect("'CryptoCtx' must be initialized already");
        let raw_priv = crypto_ctx.mm2_internal_privkey_secret();
        let my_persistent_pub = compressed_pub_key_from_priv_raw(&raw_priv.take(), ChecksumType::DSHA256).unwrap();

        #[cfg(feature = "ibc-routing-for-swaps")]
        {
            let _taker_order_metadata = &taker_order.request.order_metadata;
            let _maker_order_metadata = &taker_match.reserved.order_metadata;

            // TODO
            //   - If this is non-HTLC tendermint swap, cross-check IBC channels for routing before start.
            //   - Could malformed orders trick us by intentionally modfying channel IDs?
            //   - Unify this logic with `lp_connect_start_bob`.
            unreachable!();
        }

        #[allow(unreachable_code)]
        let maker_amount = taker_match.reserved.get_base_amount().clone();
        let taker_amount = taker_match.reserved.get_rel_amount().clone();

        let my_conf_settings = choose_taker_confs_and_notas(
            &taker_order.request,
            &taker_match.reserved.conf_settings,
            &maker_coin,
            &taker_coin,
        );
        // detect atomic lock time version implicitly by conf_settings existence in maker reserved
        let atomic_locktime_v = match taker_match.reserved.conf_settings {
            Some(_) => {
                let other_conf_settings = choose_maker_confs_and_notas(
                    taker_match.reserved.conf_settings,
                    &taker_order.request,
                    &maker_coin,
                    &taker_coin,
                );
                AtomicLocktimeVersion::V2 {
                    my_conf_settings,
                    other_conf_settings,
                }
            },
            None => AtomicLocktimeVersion::V1,
        };
        let locktime = lp_atomic_locktime(
            taker_order.maker_orderbook_ticker(),
            taker_order.taker_orderbook_ticker(),
            atomic_locktime_v,
        );
        log_tag!(
            ctx,
            "";
            fmt = "Entering the taker_swap_loop {}/{} with uuid: {}",
            maker_coin.ticker(),
            taker_coin.ticker(),
            uuid
        );

        let bob_swap_v = taker_match.reserved.swap_version;
        let alice_swap_v = taker_order.request.swap_version;

        // Start a legacy swap if either the maker or taker uses the legacy swap protocol (version 1)
        if bob_swap_v.is_legacy() || alice_swap_v.is_legacy() {
            let params = LegacySwapParams {
                maker_coin: &maker_coin,
                taker_coin: &taker_coin,
                uuid: &uuid,
                my_conf_settings: &my_conf_settings,
                my_persistent_pub: &my_persistent_pub,
                maker_amount: &maker_amount,
                taker_amount: &taker_amount,
                locktime: &locktime,
            };
            start_taker_legacy_swap(&ctx, taker_order, maker_pubkey, params).await;
            return;
        }

        let taker_secret = match generate_secret() {
            Ok(s) => s.into(),
            Err(e) => {
                error!("Error {} on secret generation", e);
                return;
            },
        };

        // Ensure detect_secret_hash_algo_v2 returns the correct secret hash algorithm when adding new coin support in TPU.
        let params = StateMachineParams {
            secret_hash_algo: &detect_secret_hash_algo_v2(&maker_coin, &taker_coin),
            uuid: &uuid,
            my_conf_settings: &my_conf_settings,
            locktime: &locktime,
            maker_amount: &maker_amount,
            taker_amount: &taker_amount,
        };
        let maker_p2p_pubkey = match maker_p2p_pubkey {
            PublicKey::Secp256k1(pubkey) => pubkey.into(),
        };

        // TODO try to handle it more gracefully during project redesign
        match (&maker_coin, &taker_coin) {
            (MmCoinEnum::UtxoCoin(m), MmCoinEnum::UtxoCoin(t)) => {
                start_taker_swap_state_machine(&ctx, &taker_order, &maker_p2p_pubkey, &taker_secret, m, t, &params)
                    .await;
            },
            (MmCoinEnum::EthCoin(m), MmCoinEnum::EthCoin(t)) => {
                start_taker_swap_state_machine(&ctx, &taker_order, &maker_p2p_pubkey, &taker_secret, m, t, &params)
                    .await;
            },
            (MmCoinEnum::UtxoCoin(m), MmCoinEnum::EthCoin(t)) => {
                start_taker_swap_state_machine(&ctx, &taker_order, &maker_p2p_pubkey, &taker_secret, m, t, &params)
                    .await;
            },
            (MmCoinEnum::EthCoin(m), MmCoinEnum::UtxoCoin(t)) => {
                start_taker_swap_state_machine(&ctx, &taker_order, &maker_p2p_pubkey, &taker_secret, m, t, &params)
                    .await;
            },
            _ => {
                let params = LegacySwapParams {
                    maker_coin: &maker_coin,
                    taker_coin: &taker_coin,
                    uuid: &uuid,
                    my_conf_settings: &my_conf_settings,
                    my_persistent_pub: &my_persistent_pub,
                    maker_amount: &maker_amount,
                    taker_amount: &taker_amount,
                    locktime: &locktime,
                };
                start_taker_legacy_swap(&ctx, taker_order, maker_pubkey, params).await;
            },
        }
    };

    let settings = AbortSettings::info_on_abort(format!("swap {uuid} stopped!"));
    spawner.spawn_with_settings(fut, settings)
}

async fn start_taker_legacy_swap(
    ctx: &MmArc,
    taker_order: TakerOrder,
    maker_pubkey: bits256,
    params: LegacySwapParams<'_>,
) {
    #[cfg(any(test, feature = "run-docker-tests"))]
    let fail_at = std::env::var("TAKER_FAIL_AT").map(FailAt::from).ok();

    if let Err(e) = insert_new_swap_to_db(
        ctx.clone(),
        params.taker_coin.ticker(),
        params.maker_coin.ticker(),
        *params.uuid,
        now_sec(),
        LEGACY_SWAP_TYPE,
    )
    .await
    {
        error!("Error {} on new swap insertion", e);
    }

    let taker_swap = TakerSwap::new(
        ctx.clone(),
        maker_pubkey,
        params.maker_amount.clone(),
        params.taker_amount.clone(),
        *params.my_persistent_pub,
        *params.uuid,
        Some(*params.uuid),
        *params.my_conf_settings,
        params.maker_coin.clone(),
        params.taker_coin.clone(),
        *params.locktime,
        taker_order.p2p_privkey.map(SerializableSecp256k1Keypair::into_inner),
        #[cfg(any(test, feature = "run-docker-tests"))]
        fail_at,
    );
    run_taker_swap(RunTakerSwapInput::StartNew(taker_swap), ctx.clone()).await
}

async fn start_taker_swap_state_machine<
    MakerCoin: MmCoin + MakerCoinSwapOpsV2 + Clone,
    TakerCoin: MmCoin + TakerCoinSwapOpsV2 + Clone,
>(
    ctx: &MmArc,
    taker_order: &TakerOrder,
    maker_p2p_pubkey: &Secp256k1Pubkey,
    taker_secret: &H256,
    maker_coin: &MakerCoin,
    taker_coin: &TakerCoin,
    params: &StateMachineParams<'_>,
) {
    let mut taker_swap_state_machine = TakerSwapStateMachine {
        storage: TakerSwapStorage::new(ctx.clone()),
        abortable_system: ctx
            .abortable_system
            .create_subsystem()
            .expect("create_subsystem should not fail"),
        ctx: ctx.clone(),
        started_at: now_sec(),
        lock_duration: *params.locktime,
        maker_coin: maker_coin.clone(),
        maker_volume: params.maker_amount.clone(),
        taker_coin: taker_coin.clone(),
        taker_volume: params.taker_amount.clone(),
        taker_premium: Default::default(),
        secret_hash_algo: *params.secret_hash_algo,
        conf_settings: *params.my_conf_settings,
        p2p_topic: swap_v2_topic(params.uuid),
        uuid: *params.uuid,
        p2p_keypair: taker_order.p2p_privkey.map(SerializableSecp256k1Keypair::into_inner),
        taker_secret: *taker_secret,
        maker_p2p_pubkey: *maker_p2p_pubkey,
        require_maker_payment_confirm_before_funding_spend: true,
        require_maker_payment_spend_confirm: true,
        swap_version: taker_order.request.swap_version.version,
    };
    #[allow(clippy::box_default)]
    taker_swap_state_machine
        .run(Box::new(taker_swap_v2::Initialize::default()))
        .await
        .error_log();
}

pub async fn lp_ordermatch_loop(ctx: MmArc) {
    // lp_ordermatch_loop is spawned only if CryptoCtx is initialized
    let my_pubsecp = CryptoCtx::from_ctx(&ctx)
        .expect("CryptoCtx not available")
        .mm2_internal_pubkey_hex();

    loop {
        if ctx.is_stopping() {
            break;
        }
        let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();

        handle_timed_out_taker_orders(ctx.clone(), &ordermatch_ctx).await;
        handle_timed_out_maker_matches(ctx.clone(), &ordermatch_ctx).await;
        check_balance_for_maker_orders(ctx.clone(), &ordermatch_ctx).await;

        let expired_orders = ordermatch_ctx.maker_orders_ctx.lock().orders.drop_expired_entries();

        for (uuid, order_mutex) in expired_orders {
            log::info!("Order '{uuid}' is expired, cancelling");

            let order = order_mutex.lock().await;
            maker_order_cancelled_p2p_notify(&ctx, &order);
            delete_my_maker_order(ctx.clone(), order.clone(), MakerOrderCancellationReason::Expired)
                .compat()
                .await
                .ok();
        }

        {
            let mut orderbook = ordermatch_ctx.orderbook.lock();

            let deadline = now_sec().saturating_sub(MAKER_ORDER_TIMEOUT);

            let mut pairs_to_prune = Vec::new();
            let mut pubkeys_to_remove = Vec::new();

            for (pubkey, state) in orderbook.pubkeys_state.iter() {
                // Skip our own pubkeys; local order lifecycle manages them.
                if pubkey == &my_pubsecp || orderbook.my_p2p_pubkeys.contains(pubkey) {
                    continue;
                }

                // Remove the pubkey if no pair has been seen recently.
                let any_recent_pair = state
                    .trie_roots
                    .keys()
                    .any(|pair| state.pair_last_seen_local.get(pair).copied().unwrap_or(0) > deadline);

                if !any_recent_pair {
                    pubkeys_to_remove.push(pubkey.clone());
                    continue;
                }

                // Otherwise keep the pubkey and prune only its stale pairs.
                let stale_pairs: Vec<_> = state
                    .trie_roots
                    .keys()
                    .filter_map(|pair| {
                        let last_seen = state.pair_last_seen_local.get(pair).copied().unwrap_or(0);
                        (last_seen <= deadline).then(|| pair.clone())
                    })
                    .collect();

                if !stale_pairs.is_empty() {
                    pairs_to_prune.push((pubkey.clone(), stale_pairs));
                }
            }

            // Prune stale pairs for pubkeys we keep.
            for (pubkey, pairs) in pairs_to_prune {
                for pair in pairs {
                    remove_pubkey_pair_orders(&mut orderbook, &pubkey, &pair);
                    if let Some(state) = orderbook.pubkeys_state.get_mut(&pubkey) {
                        state.pair_last_seen_local.remove(&pair);
                    }
                }
            }

            // Remove fully expired pubkeys: delete their remaining orders first, then the state.
            // GC: remove orders while the pubkey state still exists to avoid lazy re-creation inside `remove_order_trie_update`.
            for pubkey in pubkeys_to_remove {
                if let Some(orders) = orderbook
                    .pubkeys_state
                    .get_mut(&pubkey)
                    .map(|state| std::mem::take(&mut state.orders_uuids))
                {
                    for (uuid, _) in orders {
                        orderbook.remove_order_trie_update(uuid);
                    }
                }

                // TODO(pubkey_replay_guard):
                // Block stale replays after full pubkey timeout (incl. restarts).
                // - On full pubkey removal: store pubkey -> max maker_ts (write-once HashMap<String,u64>).
                // - On keep-alive intake: before creating state, drop if maker_ts <= stored floor.
                // - Restarts: persist this map (and ideally per-pair last_maker_ts); until persisted, enforce
                //   sanity bounds: maker_ts > now + MAX_MAKER_FUTURE_SKEW or now - maker_ts > MAX_MAKER_PAST_AGE => reject.
                // - Liveness: keep GC driven by local time; optionally prune when maker_stale || local_stale;
                //   do not rely solely on maker clock.
                // - Cleanup: pre-scan without creating state; centralize timestamp checks/logging; never sync
                //   origin for messages rejected by guards; add unit tests.
                orderbook.pubkeys_state.remove(&pubkey);
            }

            collect_orderbook_metrics(&ctx, &orderbook);
        }

        {
            let mut missing_uuids = Vec::new();
            let mut to_cancel = Vec::new();
            {
                let orderbook = ordermatch_ctx.orderbook.lock();
                for uuid in ordermatch_ctx.maker_orders_ctx.lock().orders.keys() {
                    if !orderbook.order_set.contains_key(&uuid) {
                        missing_uuids.push(uuid);
                    }
                }
            }

            for uuid in missing_uuids {
                let order_mutex = match ordermatch_ctx.maker_orders_ctx.lock().get_order(&uuid) {
                    Some(o) => o.clone(),
                    None => continue,
                };

                let mut order = order_mutex.lock().await;
                let (base, rel) = match find_pair(&ctx, &order.base, &order.rel).await {
                    Ok(Some(pair)) => pair,
                    _ => continue,
                };
                let current_balance = match base.my_spendable_balance().compat().await {
                    Ok(b) => b,
                    Err(e) => {
                        log::info!("Error {} on balance check to kickstart order {}, cancelling", e, uuid);
                        to_cancel.push(uuid);
                        continue;
                    },
                };
                let max_vol =
                    match calc_max_maker_vol(&ctx, &base, &current_balance, FeeApproxStage::OrderIssueMax).await {
                        Ok(vol_info) => vol_info.volume,
                        Err(e) => {
                            log::info!("Error {} on balance check to kickstart order {}, cancelling", e, uuid);
                            to_cancel.push(uuid);
                            continue;
                        },
                    };
                if max_vol < order.available_amount() {
                    order.max_base_vol = order.reserved_amount() + max_vol;
                }
                if order.available_amount() < order.min_base_vol {
                    log::info!("Insufficient volume available for order {}, cancelling", uuid);
                    to_cancel.push(uuid);
                    continue;
                }

                let maker_orders = ordermatch_ctx.maker_orders_ctx.lock().orders.clone();

                // notify other nodes only if maker order is still there keeping maker_orders locked during the operation
                if maker_orders.contains_key(&uuid) {
                    if let Err(err) = subscribe_to_orderbook_topic(
                        &ctx,
                        order.base_orderbook_ticker(),
                        order.rel_orderbook_ticker(),
                        false,
                    )
                    .await
                    {
                        // TODO: centralized-P2P-failure-policy
                        // Introduce a single, centralized handler for P2P/network failures that affect a specific (base, rel) pair.
                        // The handler should, on critical/persistent failures, cancel local maker orders for that pair and clear
                        // in-memory pair state to avoid stale/half-synced state. Apply this consistently across all call sites,
                        // including but not limited to:
                        // - subscribe_to_orderbook_topic
                        // - maker_order_created_p2p_notify
                        // - re-subscribe on reconnect if needed
                        // - keep-alive initiated sync requests
                        // - background refresh / re-announce paths
                        warn!(
                            "Failed to subscribe to orderbook topic {}:{}: {}",
                            order.base_orderbook_ticker(),
                            order.rel_orderbook_ticker(),
                            err
                        );
                    }
                    maker_order_created_p2p_notify(
                        ctx.clone(),
                        &order,
                        base.coin_protocol_info(None),
                        rel.coin_protocol_info(Some(order.max_base_vol.clone() * order.price.clone())),
                    );
                }
            }

            for uuid in to_cancel {
                let removed_order_mutex = ordermatch_ctx.maker_orders_ctx.lock().remove_order(&uuid);
                // This checks that the order hasn't been removed by another process
                if let Some(order_mutex) = removed_order_mutex {
                    let order = order_mutex.lock().await;
                    maker_order_cancelled_p2p_notify(&ctx, &order);
                    delete_my_maker_order(
                        ctx.clone(),
                        order.clone(),
                        MakerOrderCancellationReason::InsufficientBalance,
                    )
                    .compat()
                    .await
                    .ok();
                }
            }
        }

        Timer::sleep(0.777).await;
    }
}

pub async fn clean_memory_loop(ctx_weak: MmWeak) {
    loop {
        {
            let ctx = match MmArc::from_weak(&ctx_weak) {
                Some(ctx) => ctx,
                None => return,
            };
            if ctx.is_stopping() {
                break;
            }

            let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
            let mut orderbook = ordermatch_ctx.orderbook.lock();
            orderbook.memory_db.purge();
        }
        Timer::sleep(600.).await;
    }
}

/// Transforms the timed out and unmatched GTC taker orders to maker.
///
/// # Safety
///
/// The function locks the [`OrdermatchContext::my_maker_orders`] and [`OrdermatchContext::my_taker_orders`] mutexes.
async fn handle_timed_out_taker_orders(ctx: MmArc, ordermatch_ctx: &OrdermatchContext) {
    let mut my_taker_orders = ordermatch_ctx.my_taker_orders.lock().await;
    let storage = MyOrdersStorage::new(ctx.clone());
    let mut my_actual_taker_orders = HashMap::with_capacity(my_taker_orders.len());

    for (uuid, order) in my_taker_orders.drain() {
        if order.created_at + order.timeout * 1000 >= now_ms() {
            my_actual_taker_orders.insert(uuid, order);
            continue;
        }

        if !order.matches.is_empty() || order.order_type != OrderType::GoodTillCancelled {
            delete_my_taker_order(ctx.clone(), order, TakerOrderCancellationReason::TimedOut)
                .compat()
                .await
                .ok();
            continue;
        }

        // transform the timed out taker order to maker

        delete_my_taker_order(ctx.clone(), order.clone(), TakerOrderCancellationReason::ToMaker)
            .compat()
            .await
            .ok();
        let maker_order: MakerOrder = order.into();
        ordermatch_ctx
            .maker_orders_ctx
            .lock()
            .add_order(ctx.weak(), maker_order.clone(), None);

        storage
            .save_new_active_maker_order(&maker_order)
            .await
            .error_log_with_msg("!save_new_active_maker_order");
        if maker_order.save_in_history {
            storage
                .update_was_taker_in_filtering_history(uuid)
                .await
                .error_log_with_msg("!update_was_taker_in_filtering_history");
        }

        // notify other peers
        if let Ok(Some((base, rel))) = find_pair(&ctx, &maker_order.base, &maker_order.rel).await {
            maker_order_created_p2p_notify(
                ctx.clone(),
                &maker_order,
                base.coin_protocol_info(None),
                rel.coin_protocol_info(Some(maker_order.max_base_vol.clone() * maker_order.price.clone())),
            );
        }
    }

    *my_taker_orders = my_actual_taker_orders;
}

/// # Safety
///
/// The function locks the [`OrdermatchContext::my_maker_orders`] mutex.
async fn check_balance_for_maker_orders(ctx: MmArc, ordermatch_ctx: &OrdermatchContext) {
    let my_maker_orders = ordermatch_ctx.maker_orders_ctx.lock().orders.clone();

    for (uuid, order) in my_maker_orders {
        let order = order.lock().await;
        if order.available_amount() >= order.min_base_vol || order.has_ongoing_matches() {
            continue;
        }

        let reason = if order.matches.is_empty() {
            MakerOrderCancellationReason::InsufficientBalance
        } else {
            MakerOrderCancellationReason::Fulfilled
        };
        let removed_order_mutex = ordermatch_ctx.maker_orders_ctx.lock().remove_order(&uuid);
        // This checks that the order hasn't been removed by another process
        if removed_order_mutex.is_some() {
            maker_order_cancelled_p2p_notify(&ctx, &order);
            delete_my_maker_order(ctx.clone(), order.clone(), reason)
                .compat()
                .await
                .ok();
        }
    }
}

/// Removes timed out unfinished matches to unlock the reserved amount.
///
/// # Safety
///
/// The function locks the [`OrdermatchContext::my_maker_orders`] mutex.
async fn handle_timed_out_maker_matches(ctx: MmArc, ordermatch_ctx: &OrdermatchContext) {
    let now = now_ms();
    let storage = MyOrdersStorage::new(ctx.clone());
    let my_maker_orders = ordermatch_ctx.maker_orders_ctx.lock().orders.clone();

    for (_, order) in my_maker_orders.iter() {
        let mut order = order.lock().await;
        let old_len = order.matches.len();
        order.matches.retain(|_, order_match| {
            order_match.last_updated + ORDER_MATCH_TIMEOUT * 1000 > now || order_match.connected.is_some()
        });
        if old_len != order.matches.len() {
            storage
                .update_active_maker_order(&order)
                .await
                .error_log_with_msg("!update_active_maker_order");
        }
    }
}

async fn process_maker_reserved(ctx: MmArc, reserved_msg: MakerReserved) {
    log::debug!("Processing MakerReserved {:?}", reserved_msg);
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    {
        let my_taker_orders = ordermatch_ctx.my_taker_orders.lock().await;
        if !my_taker_orders.contains_key(&reserved_msg.taker_order_uuid) {
            return;
        }
    }

    let uuid = reserved_msg.taker_order_uuid;
    {
        let mut pending_map = ordermatch_ctx.pending_maker_reserved.lock().await;
        let pending_for_order = pending_map
            .entry(reserved_msg.taker_order_uuid)
            .or_insert_with(Vec::new);
        pending_for_order.push(reserved_msg);
        if pending_for_order.len() > 1 {
            // messages will be sorted by price and processed in the first called handler
            return;
        }
    }

    Timer::sleep(3.).await;

    let mut my_taker_orders = ordermatch_ctx.my_taker_orders.lock().await;
    let my_order = match my_taker_orders.entry(uuid) {
        Entry::Vacant(_) => return,
        Entry::Occupied(entry) => entry.into_mut(),
    };

    // our base and rel coins should match maker's side tickers for a proper is_coin_protocol_supported check
    let (base_coin, rel_coin) = match find_pair(&ctx, my_order.maker_coin_ticker(), my_order.taker_coin_ticker()).await
    {
        Ok(Some(c)) => c,
        _ => return, // attempt to match with deactivated coin
    };
    let mut pending_map = ordermatch_ctx.pending_maker_reserved.lock().await;
    if let Some(mut reserved_messages) = pending_map.remove(&uuid) {
        reserved_messages.sort_unstable_by_key(|r| r.price());

        for reserved_msg in reserved_messages {
            let my_conf_settings = choose_maker_confs_and_notas(
                reserved_msg.conf_settings.clone(),
                &my_order.request,
                &base_coin,
                &rel_coin,
            );
            let other_conf_settings =
                choose_taker_confs_and_notas(&my_order.request, &reserved_msg.conf_settings, &base_coin, &rel_coin);
            let atomic_locktime_v = AtomicLocktimeVersion::V2 {
                my_conf_settings,
                other_conf_settings,
            };
            let lock_time = lp_atomic_locktime(
                my_order.maker_orderbook_ticker(),
                my_order.taker_orderbook_ticker(),
                atomic_locktime_v,
            );
            // send "connect" message if reserved message targets our pubkey AND
            // reserved amounts match our order AND order is NOT reserved by someone else (empty matches)
            if (my_order.match_reserved(&reserved_msg) == MatchReservedResult::Matched && my_order.matches.is_empty())
                && base_coin.is_coin_protocol_supported(&reserved_msg.base_protocol_info, None, lock_time, false)
                && rel_coin.is_coin_protocol_supported(
                    &reserved_msg.rel_protocol_info,
                    Some(reserved_msg.rel_amount.clone()),
                    lock_time,
                    false,
                )
            {
                let our_public_id = CryptoCtx::from_ctx(&ctx)
                    .expect("'CryptoCtx' must be initialized already")
                    .mm2_internal_public_id();
                let connect = TakerConnect {
                    sender_pubkey: H256Json::from(our_public_id.bytes),
                    dest_pub_key: reserved_msg.sender_pubkey,
                    taker_order_uuid: reserved_msg.taker_order_uuid,
                    maker_order_uuid: reserved_msg.maker_order_uuid,
                };
                let topic = my_order.orderbook_topic();
                broadcast_ordermatch_message(&ctx, topic, connect.clone().into(), my_order.p2p_keypair());
                let taker_match = TakerMatch {
                    reserved: reserved_msg,
                    connect,
                    connected: None,
                    last_updated: now_ms(),
                };

                ctx.event_stream_manager
                    .send_fn(&OrderStatusStreamer::derive_streamer_id(()), || {
                        OrderStatusEvent::TakerMatch(taker_match.clone())
                    })
                    .ok();

                my_order
                    .matches
                    .insert(taker_match.reserved.maker_order_uuid, taker_match);
                MyOrdersStorage::new(ctx)
                    .update_active_taker_order(my_order)
                    .await
                    .error_log_with_msg("!update_active_taker_order");
                return;
            }
        }
    }
}

async fn process_maker_connected(ctx: MmArc, from_pubkey: PublicKey, connected: MakerConnected) {
    log::debug!("Processing MakerConnected {:?}", connected);
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();

    let mut my_taker_orders = ordermatch_ctx.my_taker_orders.lock().await;
    let my_order_entry = match my_taker_orders.entry(connected.taker_order_uuid) {
        Entry::Occupied(e) => e,
        Entry::Vacant(_) => return,
    };
    let order_match = match my_order_entry.get().matches.get(&connected.maker_order_uuid) {
        Some(o) => o,
        None => {
            warn!(
                "Our node doesn't have the match with uuid {}",
                connected.maker_order_uuid
            );
            return;
        },
    };

    if order_match.reserved.sender_pubkey != from_pubkey.unprefixed().into() {
        error!("Connected message sender pubkey != reserved message sender pubkey");
        return;
    }

    ctx.event_stream_manager
        .send_fn(&OrderStatusStreamer::derive_streamer_id(()), || {
            OrderStatusEvent::TakerConnected(order_match.clone())
        })
        .ok();

    // alice
    lp_connected_alice(
        ctx.clone(),
        my_order_entry.get().clone(),
        order_match.clone(),
        from_pubkey,
    );
    // remove the matched order immediately
    let order = my_order_entry.remove();
    delete_my_taker_order(ctx, order, TakerOrderCancellationReason::Fulfilled)
        .compat()
        .await
        .ok();
}

async fn process_taker_request(ctx: MmArc, taker_request: TakerRequest) {
    log::debug!("Processing request {:?}", taker_request);

    let our_public_id: H256Json = match CryptoCtx::from_ctx(&ctx) {
        Ok(ctx) => ctx.mm2_internal_public_id().bytes.into(),
        Err(_) => return,
    };

    if !taker_request.can_match_with_maker_pubkey(&our_public_id) {
        return;
    }

    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    let storage = MyOrdersStorage::new(ctx.clone());
    let mut my_orders = ordermatch_ctx.maker_orders_ctx.lock().orders.clone();
    let filtered = my_orders
        .iter_mut()
        .filter(|(uuid, _)| taker_request.can_match_with_uuid(uuid));

    for (uuid, order) in filtered {
        let mut order = order.lock().await;
        if let OrderMatchResult::Matched((base_amount, rel_amount)) = order.match_with_request(&taker_request) {
            let (base_coin, rel_coin) = match find_pair(&ctx, &order.base, &order.rel).await {
                Ok(Some(c)) => c,
                _ => return, // attempt to match with deactivated coin
            };

            let my_conf_settings =
                choose_maker_confs_and_notas(order.conf_settings.clone(), &taker_request, &base_coin, &rel_coin);
            let other_conf_settings =
                choose_taker_confs_and_notas(&taker_request, &order.conf_settings, &base_coin, &rel_coin);
            let atomic_locktime_v = AtomicLocktimeVersion::V2 {
                my_conf_settings,
                other_conf_settings,
            };
            let maker_lock_duration = (lp_atomic_locktime(
                order.base_orderbook_ticker(),
                order.rel_orderbook_ticker(),
                atomic_locktime_v,
            ) as f64
                * rel_coin.maker_locktime_multiplier())
            .ceil() as u64;

            if !order.matches.contains_key(&taker_request.uuid)
                && base_coin.is_coin_protocol_supported(
                    taker_request.base_protocol_info_for_maker(),
                    Some(base_amount.clone()),
                    maker_lock_duration,
                    true,
                )
                && rel_coin.is_coin_protocol_supported(
                    taker_request.rel_protocol_info_for_maker(),
                    None,
                    maker_lock_duration,
                    true,
                )
            {
                let reserved = MakerReserved {
                    dest_pub_key: taker_request.sender_pubkey,
                    sender_pubkey: our_public_id,
                    base: order.base_orderbook_ticker().to_owned(),
                    base_amount: base_amount.clone(),
                    rel_amount: rel_amount.clone(),
                    rel: order.rel_orderbook_ticker().to_owned(),
                    taker_order_uuid: taker_request.uuid,
                    maker_order_uuid: *uuid,
                    conf_settings: order.conf_settings.clone().or_else(|| {
                        Some(OrderConfirmationsSettings {
                            base_confs: base_coin.required_confirmations(),
                            base_nota: base_coin.requires_notarization(),
                            rel_confs: rel_coin.required_confirmations(),
                            rel_nota: rel_coin.requires_notarization(),
                        })
                    }),
                    base_protocol_info: Some(base_coin.coin_protocol_info(None)),
                    rel_protocol_info: Some(rel_coin.coin_protocol_info(Some(rel_amount.clone()))),
                    swap_version: order.swap_version,
                    #[cfg(feature = "ibc-routing-for-swaps")]
                    order_metadata: order.order_metadata.clone(),
                };
                let topic = order.orderbook_topic();
                log::debug!("Request matched sending reserved {:?}", reserved);
                broadcast_ordermatch_message(&ctx, topic, reserved.clone().into(), order.p2p_keypair());
                let maker_match = MakerMatch {
                    request: taker_request,
                    reserved,
                    connect: None,
                    connected: None,
                    last_updated: now_ms(),
                };

                ctx.event_stream_manager
                    .send_fn(&OrderStatusStreamer::derive_streamer_id(()), || {
                        OrderStatusEvent::MakerMatch(maker_match.clone())
                    })
                    .ok();

                order.matches.insert(maker_match.request.uuid, maker_match);
                storage
                    .update_active_maker_order(&order)
                    .await
                    .error_log_with_msg("!update_active_maker_order");
            }
            return;
        }
    }
}

async fn process_taker_connect(ctx: MmArc, sender_pubkey: PublicKey, connect_msg: TakerConnect) {
    log::debug!("Processing TakerConnect {:?}", connect_msg);
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();

    let order_mutex = {
        match ordermatch_ctx
            .maker_orders_ctx
            .lock()
            .orders
            .get(&connect_msg.maker_order_uuid)
        {
            Some(o) => o.clone(),
            None => return,
        }
    };
    let mut my_order = order_mutex.lock().await;
    let order_match = match my_order.matches.get_mut(&connect_msg.taker_order_uuid) {
        Some(o) => o,
        None => {
            warn!(
                "Our node doesn't have the match with uuid {}",
                connect_msg.taker_order_uuid
            );
            return;
        },
    };
    if order_match.request.sender_pubkey != sender_pubkey.unprefixed().into() {
        warn!("Connect message sender pubkey != request message sender pubkey");
        return;
    }

    if order_match.connected.is_none() && order_match.connect.is_none() {
        // Taker order existence is checked previously - it can't be created if CryptoCtx is not initialized
        let our_public_id = CryptoCtx::from_ctx(&ctx)
            .expect("'CryptoCtx' must be initialized already")
            .mm2_internal_public_id();
        let connected = MakerConnected {
            sender_pubkey: our_public_id.bytes.into(),
            dest_pub_key: connect_msg.sender_pubkey,
            taker_order_uuid: connect_msg.taker_order_uuid,
            maker_order_uuid: connect_msg.maker_order_uuid,
            method: "connected".into(),
        };
        order_match.connect = Some(connect_msg);
        order_match.connected = Some(connected.clone());
        let order_match = order_match.clone();

        ctx.event_stream_manager
            .send_fn(&OrderStatusStreamer::derive_streamer_id(()), || {
                OrderStatusEvent::MakerConnected(order_match.clone())
            })
            .ok();

        my_order.started_swaps.push(order_match.request.uuid);
        lp_connect_start_bob(ctx.clone(), order_match, my_order.clone(), sender_pubkey);
        let topic = my_order.orderbook_topic();
        broadcast_ordermatch_message(&ctx, topic.clone(), connected.into(), my_order.p2p_keypair());

        // If volume is less order will be cancelled a bit later
        if my_order.available_amount() >= my_order.min_base_vol {
            let mut updated_msg = new_protocol::MakerOrderUpdated::new(my_order.uuid);
            updated_msg.with_new_max_volume(my_order.available_amount().into());
            maker_order_updated_p2p_notify(ctx.clone(), topic, updated_msg, my_order.p2p_keypair());
        }
        MyOrdersStorage::new(ctx)
            .update_active_maker_order(&my_order)
            .await
            .error_log_with_msg("!update_active_maker_order");
    }
}

pub async fn buy(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let input: SellBuyRequest = try_s!(json::from_value(req));
    if input.base == input.rel {
        return ERR!("Base and rel must be different coins");
    }
    let rel_coin = try_s!(lp_coinfind(&ctx, &input.rel).await);
    let rel_coin = try_s!(rel_coin.ok_or("Rel coin is not found or inactive"));
    let base_coin = try_s!(lp_coinfind(&ctx, &input.base).await);
    let base_coin: MmCoinEnum = try_s!(base_coin.ok_or("Base coin is not found or inactive"));

    try_s!(base_coin.pre_check_for_order_creation(&ctx, &rel_coin).await);

    let my_amount = &input.volume * &input.price;
    try_s!(
        check_balance_for_taker_swap(
            &ctx,
            rel_coin.deref(),
            base_coin.deref(),
            my_amount,
            None,
            None,
            FeeApproxStage::OrderIssue
        )
        .await
    );
    let res = try_s!(lp_auto_buy(&ctx, &base_coin, &rel_coin, input).await);
    Ok(try_s!(Response::builder().body(res)))
}

pub async fn sell(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let input: SellBuyRequest = try_s!(json::from_value(req));
    if input.base == input.rel {
        return ERR!("Base and rel must be different coins");
    }
    let base_coin = try_s!(lp_coinfind(&ctx, &input.base).await);
    let base_coin = try_s!(base_coin.ok_or("Base coin is not found or inactive"));
    let rel_coin = try_s!(lp_coinfind(&ctx, &input.rel).await);
    let rel_coin = try_s!(rel_coin.ok_or("Rel coin is not found or inactive"));

    try_s!(base_coin.pre_check_for_order_creation(&ctx, &rel_coin).await);

    try_s!(
        check_balance_for_taker_swap(
            &ctx,
            base_coin.deref(),
            rel_coin.deref(),
            input.volume.clone(),
            None,
            None,
            FeeApproxStage::OrderIssue
        )
        .await
    );

    let res = try_s!(lp_auto_buy(&ctx, &base_coin, &rel_coin, input).await);
    Ok(try_s!(Response::builder().body(res)))
}

/// Created when maker order is matched with taker request
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MakerMatch {
    request: TakerRequest,
    reserved: MakerReserved,
    connect: Option<TakerConnect>,
    connected: Option<MakerConnected>,
    last_updated: u64,
}

/// Created upon taker request broadcast
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct TakerMatch {
    reserved: MakerReserved,
    connect: TakerConnect,
    connected: Option<MakerConnected>,
    last_updated: u64,
}

impl<'a> From<&'a TakerRequest> for TakerRequestForRpc {
    fn from(request: &'a TakerRequest) -> TakerRequestForRpc {
        TakerRequestForRpc {
            base: request.base.clone(),
            rel: request.rel.clone(),
            base_amount: request.base_amount.to_decimal(),
            base_amount_rat: request.base_amount.to_ratio(),
            rel_amount: request.rel_amount.to_decimal(),
            rel_amount_rat: request.rel_amount.to_ratio(),
            action: request.action.clone(),
            uuid: request.uuid,
            method: "request".to_string(),
            sender_pubkey: request.sender_pubkey,
            dest_pub_key: request.dest_pub_key,
            match_by: request.match_by.clone(),
            conf_settings: request.conf_settings.clone(),
        }
    }
}

#[allow(clippy::needless_borrow)]
pub async fn lp_auto_buy(
    ctx: &MmArc,
    base_coin: &MmCoinEnum,
    rel_coin: &MmCoinEnum,
    input: SellBuyRequest,
) -> Result<Vec<u8>, String> {
    if input.price < MmNumber::from(BigRational::new(1.into(), 100_000_000.into())) {
        return ERR!("Price is too low, minimum is 0.00000001");
    }

    let action = match Some(input.method.as_ref()) {
        Some("buy") => TakerAction::Buy,
        Some("sell") => TakerAction::Sell,
        _ => return ERR!("Auto buy must be called only from buy/sell RPC methods"),
    };
    let ordermatch_ctx = try_s!(OrdermatchContext::from_ctx(ctx));
    let mut my_taker_orders = ordermatch_ctx.my_taker_orders.lock().await;
    let our_public_id = try_s!(CryptoCtx::from_ctx(&ctx)).mm2_internal_public_id();
    let rel_volume = &input.volume * &input.price;
    let conf_settings = OrderConfirmationsSettings {
        base_confs: input.base_confs.unwrap_or_else(|| base_coin.required_confirmations()),
        base_nota: input.base_nota.unwrap_or_else(|| base_coin.requires_notarization()),
        rel_confs: input.rel_confs.unwrap_or_else(|| rel_coin.required_confirmations()),
        rel_nota: input.rel_nota.unwrap_or_else(|| rel_coin.requires_notarization()),
    };

    let mut order_builder = TakerOrderBuilder::new(base_coin, rel_coin)
        .with_base_amount(input.volume)
        .with_rel_amount(rel_volume)
        .with_action(action)
        .with_match_by(input.match_by)
        .with_min_volume(input.min_volume)
        .with_order_type(input.order_type)
        .with_conf_settings(conf_settings)
        .with_sender_pubkey(H256Json::from(our_public_id.bytes))
        .with_save_in_history(input.save_in_history)
        .with_base_orderbook_ticker(ordermatch_ctx.orderbook_ticker(base_coin.ticker()))
        .with_rel_orderbook_ticker(ordermatch_ctx.orderbook_ticker(rel_coin.ticker()));

    if !ctx.use_trading_proto_v2() {
        order_builder.set_legacy_swap_v();
    }

    // For non-HTLC Tendermint orders, include the channel information which will be used
    // later from the other pair.
    #[cfg(feature = "ibc-routing-for-swaps")]
    if let MmCoinEnum::Tendermint(tendermint_coin) = &base_coin {
        if !tendermint_coin.supports_htlc() {
            let channel_id = try_s!(tendermint_coin.get_healthy_ibc_channel_to_htlc_chain().await);
            order_builder.order_metadata.channel_id_if_ibc_routing = Some(channel_id);
        }
    }

    if let Some(timeout) = input.timeout {
        order_builder = order_builder.with_timeout(timeout);
    }
    let order = try_s!(order_builder.build());

    let request_orderbook = false;
    try_s!(
        subscribe_to_orderbook_topic(
            ctx,
            order.base_orderbook_ticker(),
            order.rel_orderbook_ticker(),
            request_orderbook
        )
        .await
    );
    broadcast_ordermatch_message(ctx, order.orderbook_topic(), order.clone().into(), order.p2p_keypair());

    let res = try_s!(json::to_vec(&Mm2RpcResult::new(SellBuyResponse {
        request: (&order.request).into(),
        order_type: order.order_type.clone(),
        min_volume: order.min_volume.clone().into(),
        base_orderbook_ticker: order.base_orderbook_ticker.clone(),
        rel_orderbook_ticker: order.rel_orderbook_ticker.clone(),
    })));

    save_my_new_taker_order(ctx.clone(), &order)
        .await
        .map_err(|e| ERRL!("{}", e))?;
    my_taker_orders.insert(order.request.uuid, order);

    Ok(res)
}

/// Orderbook Item P2P message
/// DO NOT CHANGE - it will break backwards compatibility
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct OrderbookP2PItem {
    pubkey: String,
    base: String,
    rel: String,
    price: BigRational,
    max_volume: BigRational,
    min_volume: BigRational,
    uuid: Uuid,
    created_at: u64,
}

macro_rules! try_get_age_or_default {
    ($created_at: expr) => {{
        let now = now_sec();
        now.checked_sub($created_at).unwrap_or_else(|| {
            warn!("now - created_at: ({} - {}) caused a u64 underflow", now, $created_at);
            Default::default()
        })
    }};
}

impl OrderbookP2PItem {
    fn as_rpc_best_orders_buy(
        &self,
        address: String,
        conf_settings: Option<&OrderConfirmationsSettings>,
        is_mine: bool,
    ) -> RpcOrderbookEntry {
        let price_mm = MmNumber::from(self.price.clone());
        let max_vol_mm = MmNumber::from(self.max_volume.clone());
        let min_vol_mm = MmNumber::from(self.min_volume.clone());

        let base_max_volume = max_vol_mm.clone().into();
        let base_min_volume = min_vol_mm.clone().into();
        let rel_max_volume = (&max_vol_mm * &price_mm).into();
        let rel_min_volume = (&min_vol_mm * &price_mm).into();

        RpcOrderbookEntry {
            coin: self.rel.clone(),
            address,
            price: price_mm.to_decimal(),
            price_rat: price_mm.to_ratio(),
            price_fraction: price_mm.to_fraction(),
            max_volume: max_vol_mm.to_decimal(),
            max_volume_rat: max_vol_mm.to_ratio(),
            max_volume_fraction: max_vol_mm.to_fraction(),
            min_volume: min_vol_mm.to_decimal(),
            min_volume_rat: min_vol_mm.to_ratio(),
            min_volume_fraction: min_vol_mm.to_fraction(),
            pubkey: self.pubkey.clone(),
            age: try_get_age_or_default!(self.created_at),
            uuid: self.uuid,
            is_mine,
            base_max_volume,
            base_min_volume,
            rel_max_volume,
            rel_min_volume,
            conf_settings: conf_settings.cloned(),
        }
    }

    fn as_rpc_best_orders_buy_v2(
        &self,
        address: OrderbookAddress,
        conf_settings: Option<&OrderConfirmationsSettings>,
        is_mine: bool,
    ) -> RpcOrderbookEntryV2 {
        let price_mm = MmNumber::from(self.price.clone());
        let max_vol_mm = MmNumber::from(self.max_volume.clone());
        let min_vol_mm = MmNumber::from(self.min_volume.clone());

        RpcOrderbookEntryV2 {
            coin: self.rel.clone(),
            address,
            rel_max_volume: (&max_vol_mm * &price_mm).into(),
            rel_min_volume: (&min_vol_mm * &price_mm).into(),
            price: price_mm.into(),
            pubkey: self.pubkey.clone(),
            uuid: self.uuid,
            is_mine,
            base_max_volume: max_vol_mm.into(),
            base_min_volume: min_vol_mm.into(),
            conf_settings: conf_settings.cloned(),
        }
    }

    fn as_rpc_best_orders_sell(
        &self,
        address: String,
        conf_settings: Option<&OrderConfirmationsSettings>,
        is_mine: bool,
    ) -> RpcOrderbookEntry {
        let price_mm = MmNumber::from(1i32) / self.price.clone().into();
        let max_vol_mm = MmNumber::from(self.max_volume.clone());
        let min_vol_mm = MmNumber::from(self.min_volume.clone());

        let base_max_volume = (&max_vol_mm / &price_mm).into();
        let base_min_volume = (&min_vol_mm / &price_mm).into();
        let rel_max_volume = max_vol_mm.clone().into();
        let rel_min_volume = min_vol_mm.clone().into();
        let conf_settings = conf_settings.map(|conf| conf.reversed());

        RpcOrderbookEntry {
            coin: self.base.clone(),
            address,
            price: price_mm.to_decimal(),
            price_rat: price_mm.to_ratio(),
            price_fraction: price_mm.to_fraction(),
            max_volume: max_vol_mm.to_decimal(),
            max_volume_rat: max_vol_mm.to_ratio(),
            max_volume_fraction: max_vol_mm.to_fraction(),
            min_volume: min_vol_mm.to_decimal(),
            min_volume_rat: min_vol_mm.to_ratio(),
            min_volume_fraction: min_vol_mm.to_fraction(),
            pubkey: self.pubkey.clone(),
            age: try_get_age_or_default!(self.created_at),
            uuid: self.uuid,
            is_mine,
            base_max_volume,
            base_min_volume,
            rel_max_volume,
            rel_min_volume,
            conf_settings,
        }
    }

    fn as_rpc_best_orders_sell_v2(
        &self,
        address: OrderbookAddress,
        conf_settings: Option<&OrderConfirmationsSettings>,
        is_mine: bool,
    ) -> RpcOrderbookEntryV2 {
        let price_mm = MmNumber::from(1i32) / self.price.clone().into();
        let max_vol_mm = MmNumber::from(self.max_volume.clone());
        let min_vol_mm = MmNumber::from(self.min_volume.clone());

        let conf_settings = conf_settings.map(|conf| conf.reversed());

        RpcOrderbookEntryV2 {
            coin: self.base.clone(),
            address,
            base_max_volume: (&max_vol_mm / &price_mm).into(),
            base_min_volume: (&min_vol_mm / &price_mm).into(),
            price: price_mm.into(),
            pubkey: self.pubkey.clone(),
            uuid: self.uuid,
            is_mine,
            rel_max_volume: max_vol_mm.into(),
            rel_min_volume: min_vol_mm.into(),
            conf_settings,
        }
    }
}

/// Despite it looks the same as OrderbookItemWithProof it's better to have a separate struct to avoid compatibility
/// breakage if we need to add more fields to the OrderbookItemWithProof
/// DO NOT ADD more fields in this struct as it will break backward compatibility.
/// Add them to the BestOrdersRes instead
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
struct OrderbookP2PItemWithProof {
    /// Orderbook item
    order: OrderbookP2PItem,
    /// Last pubkey message payload that contains most recent pair trie root
    last_message_payload: Vec<u8>,
    /// Proof confirming that orderbook item is in the pair trie
    proof: TrieProof,
}

impl From<OrderbookItemWithProof> for OrderbookP2PItemWithProof {
    fn from(o: OrderbookItemWithProof) -> Self {
        OrderbookP2PItemWithProof {
            order: o.order.into(),
            last_message_payload: o.last_message_payload,
            proof: o.proof,
        }
    }
}

impl From<OrderbookItem> for OrderbookP2PItem {
    fn from(o: OrderbookItem) -> OrderbookP2PItem {
        OrderbookP2PItem {
            pubkey: o.pubkey,
            base: o.base,
            rel: o.rel,
            price: o.price,
            max_volume: o.max_volume,
            min_volume: o.min_volume,
            uuid: o.uuid,
            created_at: o.created_at,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct OrderbookItem {
    pubkey: String,
    base: String,
    rel: String,
    price: BigRational,
    max_volume: BigRational,
    min_volume: BigRational,
    uuid: Uuid,
    created_at: u64,
    base_protocol_info: Vec<u8>,
    rel_protocol_info: Vec<u8>,
    conf_settings: Option<OrderConfirmationsSettings>,
}

#[derive(Clone, Debug, PartialEq)]
struct OrderbookItemWithProof {
    /// Orderbook item
    order: OrderbookItem,
    /// Last pubkey message payload that contains most recent pair trie root
    last_message_payload: Vec<u8>,
    /// Proof confirming that orderbook item is in the pair trie
    proof: TrieProof,
}

/// Concrete implementation of Hasher using Blake2b 64-bit hashes
#[derive(Debug)]
pub struct Blake2Hasher64;

impl Hasher for Blake2Hasher64 {
    type Out = [u8; 8];
    type StdHasher = Hash256StdHasher;
    const LENGTH: usize = 8;

    fn hash(x: &[u8]) -> Self::Out {
        let mut hasher = Blake2bVar::new(8).expect("8 is valid VarBlake2b output_size");
        hasher.update(x);
        let mut res: [u8; 8] = Default::default();
        hasher.finalize_variable(&mut res).expect("hashing to succeed");
        res
    }
}

type Layout = sp_trie::LayoutV0<Blake2Hasher64>;

impl OrderbookItem {
    fn apply_updated(&mut self, msg: &new_protocol::MakerOrderUpdated) {
        if let Some(new_price) = msg.new_price() {
            self.price = new_price.into();
        }

        if let Some(new_max_volume) = msg.new_max_volume() {
            self.max_volume = new_max_volume.into();
        }

        if let Some(new_min_volume) = msg.new_min_volume() {
            self.min_volume = new_min_volume.into();
        }
    }

    fn as_rpc_entry_ask(&self, address: String, is_mine: bool) -> RpcOrderbookEntry {
        let price_mm = MmNumber::from(self.price.clone());
        let max_vol_mm = MmNumber::from(self.max_volume.clone());
        let min_vol_mm = MmNumber::from(self.min_volume.clone());

        let base_max_volume = max_vol_mm.clone().into();
        let base_min_volume = min_vol_mm.clone().into();
        let rel_max_volume = (&max_vol_mm * &price_mm).into();
        let rel_min_volume = (&min_vol_mm * &price_mm).into();

        RpcOrderbookEntry {
            coin: self.base.clone(),
            address,
            price: price_mm.to_decimal(),
            price_rat: price_mm.to_ratio(),
            price_fraction: price_mm.to_fraction(),
            max_volume: max_vol_mm.to_decimal(),
            max_volume_rat: max_vol_mm.to_ratio(),
            max_volume_fraction: max_vol_mm.to_fraction(),
            min_volume: min_vol_mm.to_decimal(),
            min_volume_rat: min_vol_mm.to_ratio(),
            min_volume_fraction: min_vol_mm.to_fraction(),
            pubkey: self.pubkey.clone(),
            age: try_get_age_or_default!(self.created_at),
            uuid: self.uuid,
            is_mine,
            base_max_volume,
            base_min_volume,
            rel_max_volume,
            rel_min_volume,
            conf_settings: self.conf_settings.clone(),
        }
    }

    fn as_rpc_entry_bid(&self, address: String, is_mine: bool) -> RpcOrderbookEntry {
        let price_mm = MmNumber::from(1i32) / self.price.clone().into();
        let max_vol_mm = MmNumber::from(self.max_volume.clone());
        let min_vol_mm = MmNumber::from(self.min_volume.clone());

        let base_max_volume = (&max_vol_mm / &price_mm).into();
        let base_min_volume = (&min_vol_mm / &price_mm).into();
        let rel_max_volume = max_vol_mm.clone().into();
        let rel_min_volume = min_vol_mm.clone().into();
        let conf_settings = self.conf_settings.as_ref().map(|conf| conf.reversed());

        RpcOrderbookEntry {
            coin: self.base.clone(),
            address,
            price: price_mm.to_decimal(),
            price_rat: price_mm.to_ratio(),
            price_fraction: price_mm.to_fraction(),
            max_volume: max_vol_mm.to_decimal(),
            max_volume_rat: max_vol_mm.to_ratio(),
            max_volume_fraction: max_vol_mm.to_fraction(),
            min_volume: min_vol_mm.to_decimal(),
            min_volume_rat: min_vol_mm.to_ratio(),
            min_volume_fraction: min_vol_mm.to_fraction(),
            pubkey: self.pubkey.clone(),
            age: try_get_age_or_default!(self.created_at),
            uuid: self.uuid,
            is_mine,
            base_max_volume,
            base_min_volume,
            rel_max_volume,
            rel_min_volume,
            conf_settings,
        }
    }

    fn as_rpc_v2_entry_ask(&self, address: OrderbookAddress, is_mine: bool) -> RpcOrderbookEntryV2 {
        let price_mm = MmNumber::from(self.price.clone());
        let max_vol_mm = MmNumber::from(self.max_volume.clone());
        let min_vol_mm = MmNumber::from(self.min_volume.clone());

        RpcOrderbookEntryV2 {
            coin: self.base.clone(),
            address,
            rel_max_volume: (&max_vol_mm * &price_mm).into(),
            rel_min_volume: (&min_vol_mm * &price_mm).into(),
            price: price_mm.into(),
            pubkey: self.pubkey.clone(),
            uuid: self.uuid,
            is_mine,
            base_max_volume: max_vol_mm.into(),
            base_min_volume: min_vol_mm.into(),
            conf_settings: self.conf_settings.clone(),
        }
    }

    fn as_rpc_v2_entry_bid(&self, address: OrderbookAddress, is_mine: bool) -> RpcOrderbookEntryV2 {
        let price_mm = MmNumber::from(1i32) / self.price.clone().into();
        let max_vol_mm = MmNumber::from(self.max_volume.clone());
        let min_vol_mm = MmNumber::from(self.min_volume.clone());

        let conf_settings = self.conf_settings.as_ref().map(|conf| conf.reversed());

        RpcOrderbookEntryV2 {
            coin: self.base.clone(),
            address,
            base_max_volume: (&max_vol_mm / &price_mm).into(),
            base_min_volume: (&min_vol_mm / &price_mm).into(),
            price: price_mm.into(),
            pubkey: self.pubkey.clone(),
            uuid: self.uuid,
            is_mine,
            rel_max_volume: max_vol_mm.into(),
            rel_min_volume: min_vol_mm.into(),
            conf_settings,
        }
    }

    fn from_p2p_and_info(
        o: OrderbookP2PItem,
        proto_info: BaseRelProtocolInfo,
        conf_info: Option<OrderConfirmationsSettings>,
    ) -> Self {
        OrderbookItem {
            pubkey: o.pubkey,
            base: o.base,
            rel: o.rel,
            price: o.price,
            max_volume: o.max_volume,
            min_volume: o.min_volume,
            uuid: o.uuid,
            created_at: o.created_at,
            base_protocol_info: proto_info.base,
            rel_protocol_info: proto_info.rel,
            conf_settings: conf_info,
        }
    }

    fn base_rel_proto_info(&self) -> BaseRelProtocolInfo {
        BaseRelProtocolInfo {
            base: self.base_protocol_info.clone(),
            rel: self.rel_protocol_info.clone(),
        }
    }

    /// Serialize order partially to store in the trie
    /// AVOID CHANGING THIS as much as possible because it will cause a kind of "hard fork"
    fn trie_state_bytes(&self) -> Vec<u8> {
        #[derive(Serialize)]
        struct OrderbookItemHelper<'a> {
            pubkey: &'a str,
            base: &'a str,
            rel: &'a str,
            price: &'a BigRational,
            max_volume: &'a BigRational,
            min_volume: &'a BigRational,
            uuid: &'a Uuid,
            created_at: &'a u64,
        }

        let helper = OrderbookItemHelper {
            pubkey: &self.pubkey,
            base: &self.base,
            rel: &self.rel,
            price: &self.price,
            max_volume: &self.max_volume,
            min_volume: &self.min_volume,
            uuid: &self.uuid,
            created_at: &self.created_at,
        };

        rmp_serde::to_vec(&helper).expect("Serialization should never fail")
    }
}

fn get_true() -> bool {
    true
}

#[derive(Deserialize)]
pub struct SetPriceReq {
    base: String,
    rel: String,
    price: MmNumber,
    #[serde(default)]
    max: bool,
    #[serde(default)]
    volume: MmNumber,
    min_volume: Option<MmNumber>,
    #[serde(default = "get_true")]
    cancel_previous: bool,
    base_confs: Option<u64>,
    base_nota: Option<bool>,
    rel_confs: Option<u64>,
    rel_nota: Option<bool>,
    #[serde(default = "get_true")]
    save_in_history: bool,
    timeout_in_minutes: Option<u16>,
}

#[derive(Deserialize)]
pub struct MakerOrderUpdateReq {
    uuid: Uuid,
    new_price: Option<MmNumber>,
    max: Option<bool>,
    volume_delta: Option<MmNumber>,
    min_volume: Option<MmNumber>,
    base_confs: Option<u64>,
    base_nota: Option<bool>,
    rel_confs: Option<u64>,
    rel_nota: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct MakerReservedForRpc<'a> {
    base: &'a str,
    rel: &'a str,
    base_amount: BigDecimal,
    base_amount_rat: BigRational,
    rel_amount: BigDecimal,
    rel_amount_rat: BigRational,
    taker_order_uuid: &'a Uuid,
    maker_order_uuid: &'a Uuid,
    sender_pubkey: &'a H256Json,
    dest_pub_key: &'a H256Json,
    conf_settings: &'a Option<OrderConfirmationsSettings>,
    method: String,
}

#[derive(Debug, Serialize)]
pub struct TakerConnectForRpc<'a> {
    taker_order_uuid: &'a Uuid,
    maker_order_uuid: &'a Uuid,
    method: String,
    sender_pubkey: &'a H256Json,
    dest_pub_key: &'a H256Json,
}

impl<'a> From<&'a TakerConnect> for TakerConnectForRpc<'a> {
    fn from(connect: &'a TakerConnect) -> TakerConnectForRpc<'a> {
        TakerConnectForRpc {
            taker_order_uuid: &connect.taker_order_uuid,
            maker_order_uuid: &connect.maker_order_uuid,
            method: "connect".to_string(),
            sender_pubkey: &connect.sender_pubkey,
            dest_pub_key: &connect.dest_pub_key,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct MakerConnectedForRpc<'a> {
    taker_order_uuid: &'a Uuid,
    maker_order_uuid: &'a Uuid,
    method: String,
    sender_pubkey: &'a H256Json,
    dest_pub_key: &'a H256Json,
}

impl<'a> From<&'a MakerConnected> for MakerConnectedForRpc<'a> {
    fn from(connected: &'a MakerConnected) -> MakerConnectedForRpc<'a> {
        MakerConnectedForRpc {
            taker_order_uuid: &connected.taker_order_uuid,
            maker_order_uuid: &connected.maker_order_uuid,
            method: "connected".to_string(),
            sender_pubkey: &connected.sender_pubkey,
            dest_pub_key: &connected.dest_pub_key,
        }
    }
}

impl<'a> From<&'a MakerReserved> for MakerReservedForRpc<'a> {
    fn from(reserved: &'a MakerReserved) -> MakerReservedForRpc<'a> {
        MakerReservedForRpc {
            base: &reserved.base,
            rel: &reserved.rel,
            base_amount: reserved.base_amount.to_decimal(),
            base_amount_rat: reserved.base_amount.to_ratio(),
            rel_amount: reserved.rel_amount.to_decimal(),
            rel_amount_rat: reserved.rel_amount.to_ratio(),
            taker_order_uuid: &reserved.taker_order_uuid,
            maker_order_uuid: &reserved.maker_order_uuid,
            sender_pubkey: &reserved.sender_pubkey,
            dest_pub_key: &reserved.dest_pub_key,
            conf_settings: &reserved.conf_settings,
            method: "reserved".to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
struct MakerMatchForRpc<'a> {
    request: TakerRequestForRpc,
    reserved: MakerReservedForRpc<'a>,
    connect: Option<TakerConnectForRpc<'a>>,
    connected: Option<MakerConnectedForRpc<'a>>,
    last_updated: u64,
}

#[allow(clippy::needless_borrow)]
impl<'a> From<&'a MakerMatch> for MakerMatchForRpc<'a> {
    fn from(maker_match: &'a MakerMatch) -> MakerMatchForRpc<'a> {
        MakerMatchForRpc {
            request: (&maker_match.request).into(),
            reserved: (&maker_match.reserved).into(),
            connect: maker_match.connect.as_ref().map(Into::into),
            connected: maker_match.connected.as_ref().map(Into::into),
            last_updated: maker_match.last_updated,
        }
    }
}

#[derive(Serialize)]
struct MakerOrderForRpc<'a> {
    base: &'a str,
    rel: &'a str,
    price: BigDecimal,
    price_rat: &'a MmNumber,
    max_base_vol: BigDecimal,
    max_base_vol_rat: &'a MmNumber,
    min_base_vol: BigDecimal,
    min_base_vol_rat: &'a MmNumber,
    created_at: u64,
    updated_at: Option<u64>,
    matches: HashMap<Uuid, MakerMatchForRpc<'a>>,
    started_swaps: &'a [Uuid],
    uuid: Uuid,
    conf_settings: &'a Option<OrderConfirmationsSettings>,
    #[serde(skip_serializing_if = "Option::is_none")]
    changes_history: &'a Option<Vec<HistoricalOrder>>,
    base_orderbook_ticker: &'a Option<String>,
    rel_orderbook_ticker: &'a Option<String>,
}

impl<'a> From<&'a MakerOrder> for MakerOrderForRpc<'a> {
    fn from(order: &'a MakerOrder) -> MakerOrderForRpc<'a> {
        MakerOrderForRpc {
            base: &order.base,
            rel: &order.rel,
            price: order.price.to_decimal(),
            price_rat: &order.price,
            max_base_vol: order.max_base_vol.to_decimal(),
            max_base_vol_rat: &order.max_base_vol,
            min_base_vol: order.min_base_vol.to_decimal(),
            min_base_vol_rat: &order.min_base_vol,
            created_at: order.created_at,
            updated_at: order.updated_at,
            matches: order
                .matches
                .iter()
                .map(|(uuid, order_match)| (*uuid, order_match.into()))
                .collect(),
            started_swaps: &order.started_swaps,
            uuid: order.uuid,
            conf_settings: &order.conf_settings,
            changes_history: &order.changes_history,
            base_orderbook_ticker: &order.base_orderbook_ticker,
            rel_orderbook_ticker: &order.rel_orderbook_ticker,
        }
    }
}

/// Cancels the orders in case of error on different checks
/// https://github.com/KomodoPlatform/atomicDEX-API/issues/794
async fn cancel_orders_on_error<T, E>(ctx: &MmArc, req: &SetPriceReq, error: E) -> Result<T, E> {
    if req.cancel_previous {
        let ordermatch_ctx = OrdermatchContext::from_ctx(ctx).unwrap();
        cancel_previous_maker_orders(ctx, &ordermatch_ctx, &req.base, &req.rel).await;
    }
    Err(error)
}

pub async fn check_other_coin_balance_for_order_issue(ctx: &MmArc, other_coin: &MmCoinEnum) -> CheckBalanceResult<()> {
    let trade_fee = other_coin
        .get_receiver_trade_fee(FeeApproxStage::OrderIssue)
        .compat()
        .await
        .mm_err(|e| CheckBalanceError::from_trade_preimage_error(e, other_coin.ticker()))?;
    check_other_coin_balance_for_swap(ctx, other_coin.deref(), None, trade_fee).await
}

pub async fn check_balance_update_loop(ctx: MmWeak, ticker: String, balance: Option<BigDecimal>) {
    let mut current_balance = balance;
    loop {
        Timer::sleep(BALANCE_REQUEST_INTERVAL).await;
        let ctx = match MmArc::from_weak(&ctx) {
            Some(ctx) => ctx,
            None => return,
        };

        if let Ok(Some(coin)) = lp_coinfind(&ctx, &ticker).await {
            let balance = match coin.my_spendable_balance().compat().await {
                Ok(balance) => balance,
                Err(_) => continue,
            };
            if Some(&balance) != current_balance.as_ref() {
                let coins_ctx = CoinsContext::from_ctx(&ctx).unwrap();
                coins_ctx.balance_updated(&coin, &balance).await;
                current_balance = Some(balance);
            }
        }
    }
}

pub async fn create_maker_order(ctx: &MmArc, req: SetPriceReq) -> Result<MakerOrder, String> {
    let base_coin: MmCoinEnum = match try_s!(lp_coinfind(ctx, &req.base).await) {
        Some(coin) => coin,
        None => return ERR!("Base coin {} is not found", req.base),
    };

    let rel_coin: MmCoinEnum = match try_s!(lp_coinfind(ctx, &req.rel).await) {
        Some(coin) => coin,
        None => return ERR!("Rel coin {} is not found", req.rel),
    };

    try_s!(base_coin.pre_check_for_order_creation(ctx, &rel_coin).await);

    let (volume, balance) = if req.max {
        let CoinVolumeInfo { volume, balance, .. } = try_s!(
            get_max_maker_vol(ctx, &base_coin)
                .or_else(|e| cancel_orders_on_error(ctx, &req, e))
                .await
        );
        try_s!(check_other_coin_balance_for_order_issue(ctx, &rel_coin).await);
        (volume, balance.to_decimal())
    } else {
        let balance = try_s!(
            check_balance_for_maker_swap(
                ctx,
                base_coin.deref(),
                rel_coin.deref(),
                req.volume.clone(),
                None,
                None,
                FeeApproxStage::OrderIssue
            )
            .or_else(|e| cancel_orders_on_error(ctx, &req, e))
            .await
        );
        (req.volume.clone(), balance)
    };

    let ordermatch_ctx = try_s!(OrdermatchContext::from_ctx(ctx));
    if req.cancel_previous {
        cancel_previous_maker_orders(ctx, &ordermatch_ctx, &req.base, &req.rel).await;
    }

    let conf_settings = OrderConfirmationsSettings {
        base_confs: req.base_confs.unwrap_or_else(|| base_coin.required_confirmations()),
        base_nota: req.base_nota.unwrap_or_else(|| base_coin.requires_notarization()),
        rel_confs: req.rel_confs.unwrap_or_else(|| rel_coin.required_confirmations()),
        rel_nota: req.rel_nota.unwrap_or_else(|| rel_coin.requires_notarization()),
    };

    let mut builder = MakerOrderBuilder::new(&base_coin, &rel_coin)
        .with_max_base_vol(volume.clone())
        .with_min_base_vol(req.min_volume)
        .with_price(req.price.clone())
        .with_conf_settings(conf_settings)
        .with_save_in_history(req.save_in_history)
        .with_base_orderbook_ticker(ordermatch_ctx.orderbook_ticker(base_coin.ticker()))
        .with_rel_orderbook_ticker(ordermatch_ctx.orderbook_ticker(rel_coin.ticker()));

    if let Some(t) = req.timeout_in_minutes {
        builder.set_timeout(t);
    }

    if !ctx.use_trading_proto_v2() {
        builder.set_legacy_swap_v();
    }

    // For non-HTLC Tendermint orders, include the channel information which will be used
    // later from the other pair.
    #[cfg(feature = "ibc-routing-for-swaps")]
    if let MmCoinEnum::Tendermint(tendermint_coin) = &base_coin {
        if !tendermint_coin.supports_htlc() {
            let channel_id = try_s!(tendermint_coin.get_healthy_ibc_channel_to_htlc_chain().await);
            builder.order_metadata.channel_id_if_ibc_routing = Some(channel_id);
        }
    }

    let new_order = try_s!(builder.build());

    let request_orderbook = false;
    try_s!(
        subscribe_to_orderbook_topic(
            ctx,
            new_order.base_orderbook_ticker(),
            new_order.rel_orderbook_ticker(),
            request_orderbook
        )
        .await
    );
    save_my_new_maker_order(ctx.clone(), &new_order)
        .await
        .map_err(|e| ERRL!("{}", e))?;
    maker_order_created_p2p_notify(
        ctx.clone(),
        &new_order,
        base_coin.coin_protocol_info(None),
        rel_coin.coin_protocol_info(Some(volume * req.price)),
    );

    ordermatch_ctx
        .maker_orders_ctx
        .lock()
        .add_order(ctx.weak(), new_order.clone(), Some(balance));
    Ok(new_order)
}

pub async fn set_price(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let req: SetPriceReq = try_s!(json::from_value(req));
    let maker_order = create_maker_order(&ctx, req).await?;
    let rpc_result = MakerOrderForRpc::from(&maker_order);
    let res = try_s!(json::to_vec(&Mm2RpcResult::new(rpc_result)));
    Ok(try_s!(Response::builder().body(res)))
}

/// Removes the previous orders if there're some to allow multiple setprice call per pair.
/// It's common use case now as `autoprice` doesn't work with new ordermatching and
/// MM2 users request the coins price from aggregators by their own scripts issuing
/// repetitive setprice calls with new price
///
/// # Safety
///
/// The function locks the [`OrdermatchContext::my_maker_orders`] mutex.
async fn cancel_previous_maker_orders(
    ctx: &MmArc,
    ordermatch_ctx: &OrdermatchContext,
    base_to_delete: &str,
    rel_to_delete: &str,
) {
    let my_maker_orders = ordermatch_ctx.maker_orders_ctx.lock().orders.clone();

    for (uuid, order) in my_maker_orders {
        let order = order.lock().await;
        let to_delete = order.base == base_to_delete && order.rel == rel_to_delete;
        if to_delete {
            let removed_order_mutex = ordermatch_ctx.maker_orders_ctx.lock().remove_order(&uuid);
            // This checks that the uuid, &order.base hasn't been removed by another process
            if removed_order_mutex.is_some() {
                maker_order_cancelled_p2p_notify(ctx, &order);
                delete_my_maker_order(ctx.clone(), order.clone(), MakerOrderCancellationReason::Cancelled)
                    .compat()
                    .await
                    .ok();
            }
        }
    }
}

pub async fn update_maker_order(ctx: &MmArc, req: MakerOrderUpdateReq) -> Result<MakerOrder, String> {
    let ordermatch_ctx = try_s!(OrdermatchContext::from_ctx(ctx));
    let order_mutex = {
        let maker_orders_ctx = ordermatch_ctx.maker_orders_ctx.lock();
        match maker_orders_ctx.get_order(&req.uuid) {
            Some(order) => order.clone(),
            None => return ERR!("There is no order with UUID {}", req.uuid),
        }
    };

    let order_before_update = order_mutex.lock().await.clone();
    if order_before_update.has_ongoing_matches() {
        return ERR!("Can't update an order that has ongoing matches");
    }

    let base = order_before_update.base.as_str();
    let rel = order_before_update.rel.as_str();
    let (base_coin, rel_coin) = match find_pair(ctx, base, rel).await {
        Ok(Some(c)) => c,
        _ => return ERR!("Base coin {} and/or rel coin {} are not activated", base, rel),
    };

    let original_conf_settings = order_before_update.conf_settings.clone().unwrap();
    let updated_conf_settings = OrderConfirmationsSettings {
        base_confs: req.base_confs.unwrap_or(original_conf_settings.base_confs),
        base_nota: req.base_nota.unwrap_or(original_conf_settings.base_nota),
        rel_confs: req.rel_confs.unwrap_or(original_conf_settings.rel_confs),
        rel_nota: req.rel_nota.unwrap_or(original_conf_settings.rel_nota),
    };

    let original_volume = order_before_update.max_base_vol.clone();
    let reserved_amount = order_before_update.reserved_amount();

    let mut update_msg = new_protocol::MakerOrderUpdated::new(req.uuid);
    update_msg.with_new_conf_settings(updated_conf_settings);

    // Validate and Add new_price to update_msg if new_price is found in the request
    let new_price = match req.new_price {
        Some(new_price) => {
            try_s!(validate_price(new_price.clone()));
            update_msg.with_new_price(new_price.clone().into());
            new_price
        },
        None => order_before_update.price.clone(),
    };

    let min_base_amount = base_coin.min_trading_vol();
    // Todo: Here min_trading_vol for lightning depends on inbound liquidity not outbound, will require to split min_trading_vol to two functions
    let min_rel_amount = rel_coin.min_trading_vol();

    // Add min_volume to update_msg if min_volume is found in the request
    if let Some(min_volume) = req.min_volume.clone() {
        // Validate and Calculate Minimum Volume
        let actual_min_vol = try_s!(validate_and_get_min_vol(
            min_base_amount.clone(),
            min_rel_amount.clone(),
            Some(min_volume),
            new_price.clone()
        ));
        update_msg.with_new_min_volume(actual_min_vol.into());
    }

    // Calculate order volume and add to update_msg if new_volume is found in the request
    let new_volume = if req.max.unwrap_or(false) {
        let max_volume = try_s!(get_max_maker_vol(ctx, &base_coin).await).volume + reserved_amount.clone();
        try_s!(check_other_coin_balance_for_order_issue(ctx, &rel_coin).await);
        update_msg.with_new_max_volume(max_volume.clone().into());
        max_volume
    } else if Option::is_some(&req.volume_delta) {
        let volume = original_volume + req.volume_delta.unwrap();
        if volume <= MmNumber::from("0") {
            return ERR!("New volume {} should be more than zero", volume);
        }
        try_s!(
            check_balance_for_maker_swap(
                ctx,
                base_coin.deref(),
                rel_coin.deref(),
                volume.clone(),
                None,
                None,
                FeeApproxStage::OrderIssue
            )
            .await
        );
        update_msg.with_new_max_volume(volume.clone().into());
        volume
    } else {
        original_volume
    };

    if new_volume <= reserved_amount {
        return ERR!(
            "New volume {} should be more than reserved amount for order matches {}",
            new_volume,
            reserved_amount
        );
    }

    // Validate Order Volume
    try_s!(validate_max_vol(
        min_base_amount.clone(),
        min_rel_amount.clone(),
        new_volume.clone() - reserved_amount.clone(),
        req.min_volume.clone(),
        new_price
    ));

    let order_mutex = {
        let maker_orders_ctx = ordermatch_ctx.maker_orders_ctx.lock();
        match maker_orders_ctx.get_order(&req.uuid) {
            Some(order) => order.clone(),
            None => return ERR!("Order with UUID: {} has been deleted", req.uuid),
        }
    };

    let mut order = order_mutex.lock().await;
    if *order != order_before_update {
        return ERR!("Order state has changed after price/volume/balance checks. Please try to update the order again if it's still needed.");
    }
    order.apply_updated(&update_msg);
    if let Err(e) = save_maker_order_on_update(ctx.clone(), &order).await {
        *order = order_before_update;
        return ERR!("Error on saving updated order state to database:{}", e);
    }
    update_msg.with_new_max_volume((new_volume - reserved_amount).into());
    maker_order_updated_p2p_notify(ctx.clone(), order.orderbook_topic(), update_msg, order.p2p_keypair());
    Ok(order.clone())
}

pub async fn update_maker_order_rpc(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let req: MakerOrderUpdateReq = try_s!(json::from_value(req));
    let order = try_s!(update_maker_order(&ctx, req).await);
    let rpc_result = MakerOrderForRpc::from(&order);
    let res = try_s!(json::to_vec(&Mm2RpcResult::new(rpc_result)));

    Ok(try_s!(Response::builder().body(res)))
}

/// Result of match_order_and_request function
#[derive(Debug, PartialEq)]
enum OrderMatchResult {
    /// Order and request matched, contains base and rel resulting amounts
    Matched((MmNumber, MmNumber)),
    /// Orders didn't match
    NotMatched,
}

#[derive(Deserialize)]
struct OrderStatusReq {
    uuid: Uuid,
}

#[derive(Serialize)]
struct OrderForRpcWithCancellationReason<'a> {
    #[serde(flatten)]
    order: OrderForRpc<'a>,
    cancellation_reason: &'a str,
}

pub async fn order_status(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let req: OrderStatusReq = try_s!(json::from_value(req));

    let ordermatch_ctx = try_s!(OrdermatchContext::from_ctx(&ctx));
    let storage = MyOrdersStorage::new(ctx.clone());

    let maybe_order_mutex = ordermatch_ctx.maker_orders_ctx.lock().get_order(&req.uuid).cloned();
    if let Some(order_mutex) = maybe_order_mutex {
        let order = order_mutex.lock().await.clone();
        let res = json!({
            "type": "Maker",
            "order": MakerOrderForMyOrdersRpc::from(&order),
        });
        return Response::builder()
            .body(json::to_vec(&res).expect("Serialization failed"))
            .map_err(|e| ERRL!("{}", e));
    }

    let taker_orders = ordermatch_ctx.my_taker_orders.lock().await;
    if let Some(order) = taker_orders.get(&req.uuid) {
        let res = json!({
            "type": "Taker",
            "order": TakerOrderForRpc::from(order),
        });
        return Response::builder()
            .body(json::to_vec(&res).expect("Serialization failed"))
            .map_err(|e| ERRL!("{}", e));
    }

    let order = try_s!(storage.load_order_from_history(req.uuid).await);
    let cancellation_reason = &try_s!(storage.select_order_status(req.uuid).await);

    let res = json!(OrderForRpcWithCancellationReason {
        order: OrderForRpc::from(&order),
        cancellation_reason,
    });
    Response::builder()
        .body(json::to_vec(&res).expect("Serialization failed"))
        .map_err(|e| ERRL!("{}", e))
}

#[derive(Display)]
pub enum MakerOrderCancellationReason {
    Fulfilled,
    InsufficientBalance,
    Cancelled,
    Expired,
}

#[derive(Display)]
pub enum TakerOrderCancellationReason {
    Fulfilled,
    ToMaker,
    TimedOut,
    Cancelled,
}

#[derive(Debug, Deserialize)]
pub struct MyOrdersFilter {
    pub order_type: Option<String>,
    pub initial_action: Option<String>,
    pub base: Option<String>,
    pub rel: Option<String>,
    pub from_price: Option<MmNumber>,
    pub to_price: Option<MmNumber>,
    pub from_volume: Option<MmNumber>,
    pub to_volume: Option<MmNumber>,
    pub from_timestamp: Option<u64>,
    pub to_timestamp: Option<u64>,
    pub was_taker: Option<bool>,
    pub status: Option<String>,
    #[serde(default)]
    pub include_details: bool,
}

#[derive(Debug, Clone, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "order")]
pub enum Order {
    Maker(MakerOrder),
    Taker(TakerOrder),
}

impl<'a> From<&'a Order> for OrderForRpc<'a> {
    fn from(order: &'a Order) -> OrderForRpc<'a> {
        match order {
            Order::Maker(o) => OrderForRpc::Maker(MakerOrderForRpc::from(o)),
            Order::Taker(o) => OrderForRpc::Taker(TakerOrderForRpc::from(o)),
        }
    }
}

impl Order {
    pub fn uuid(&self) -> Uuid {
        match self {
            Order::Maker(maker) => maker.uuid,
            Order::Taker(taker) => taker.request.uuid,
        }
    }
}

#[derive(Serialize)]
struct UuidParseError {
    uuid: String,
    warning: String,
}

#[derive(Debug, Default)]
pub struct RecentOrdersSelectResult {
    /// Orders matching the query
    pub orders: Vec<FilteringOrder>,
    /// Total count of orders matching the query
    pub total_count: usize,
    /// The number of skipped orders
    pub skipped: usize,
}

#[derive(Debug, Serialize)]
pub struct FilteringOrder {
    pub uuid: String,
    pub order_type: String,
    pub initial_action: String,
    pub base: String,
    pub rel: String,
    pub price: f64,
    pub volume: f64,
    pub created_at: i64,
    pub last_updated: i64,
    pub was_taker: i8,
    pub status: String,
}

/// Returns *all* uuids of swaps, which match the selected filter.
pub async fn orders_history_by_filter(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let storage = MyOrdersStorage::new(ctx.clone());

    let filter: MyOrdersFilter = try_s!(json::from_value(req));
    let db_result = try_s!(storage.select_orders_by_filter(&filter, None).await);

    let mut warnings = vec![];
    let rpc_orders = if filter.include_details {
        let mut vec = Vec::with_capacity(db_result.orders.len());
        for order in db_result.orders.iter() {
            let uuid = match Uuid::parse_str(order.uuid.as_str()) {
                Ok(uuid) => uuid,
                Err(e) => {
                    let warning = format!(
                        "Order details for Uuid {} were skipped because uuid could not be parsed",
                        order.uuid
                    );
                    warn!("{}, error {}", warning, e);
                    warnings.push(UuidParseError {
                        uuid: order.uuid.clone(),
                        warning,
                    });
                    continue;
                },
            };

            if let Ok(order) = storage.load_order_from_history(uuid).await {
                vec.push(order);
                continue;
            }

            let ordermatch_ctx = try_s!(OrdermatchContext::from_ctx(&ctx));
            if order.order_type == "Maker" {
                let maybe_order_mutex = ordermatch_ctx.maker_orders_ctx.lock().get_order(&uuid).cloned();
                if let Some(maker_order_mutex) = maybe_order_mutex {
                    let maker_order = maker_order_mutex.lock().await.clone();
                    vec.push(Order::Maker(maker_order));
                }
                continue;
            }

            let taker_orders = ordermatch_ctx.my_taker_orders.lock().await;
            if let Some(taker_order) = taker_orders.get(&uuid) {
                vec.push(Order::Taker(taker_order.to_owned()));
            }
        }
        vec
    } else {
        vec![]
    };

    let details: Vec<_> = rpc_orders.iter().map(OrderForRpc::from).collect();

    let json = json!({
    "result": {
        "orders": db_result.orders,
        "details": details,
        "found_records": db_result.total_count,
        "warnings": warnings,
    }});

    let res = try_s!(json::to_vec(&json));

    Ok(try_s!(Response::builder().body(res)))
}

#[derive(Deserialize)]
pub struct CancelOrderReq {
    uuid: Uuid,
}

#[derive(Debug, Deserialize, Serialize, SerializeErrorType, Display)]
#[serde(tag = "error_type", content = "error_data")]
pub enum CancelOrderError {
    #[display(fmt = "Cannot retrieve order match context.")]
    CannotRetrieveOrderMatchContext,
    #[display(fmt = "Order {uuid} is being matched now, can't cancel")]
    OrderBeingMatched { uuid: Uuid },
    #[display(fmt = "Order {uuid} not found")]
    UUIDNotFound { uuid: Uuid },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct CancelOrderResponse {
    result: String,
}

// TODO: This is a near copy of the function below, `cancel_order_rpc`.
pub async fn cancel_order(ctx: MmArc, req: CancelOrderReq) -> Result<CancelOrderResponse, MmError<CancelOrderError>> {
    let ordermatch_ctx = match OrdermatchContext::from_ctx(&ctx) {
        Ok(x) => x,
        Err(_) => return MmError::err(CancelOrderError::CannotRetrieveOrderMatchContext),
    };
    let maybe_order_mutex = ordermatch_ctx.maker_orders_ctx.lock().get_order(&req.uuid).cloned();
    if let Some(order_mutex) = maybe_order_mutex {
        let order = order_mutex.lock().await;
        if !order.is_cancellable() {
            return MmError::err(CancelOrderError::OrderBeingMatched { uuid: req.uuid });
        }
        let removed_order_mutex = ordermatch_ctx.maker_orders_ctx.lock().remove_order(&order.uuid);
        // This checks that the order hasn't been removed by another process
        if removed_order_mutex.is_some() {
            maker_order_cancelled_p2p_notify(&ctx, &order);
            delete_my_maker_order(ctx, order.clone(), MakerOrderCancellationReason::Cancelled)
                .compat()
                .await
                .ok();
        }
        return Ok(CancelOrderResponse {
            result: "success".to_string(),
        });
    }

    let mut taker_orders = ordermatch_ctx.my_taker_orders.lock().await;
    match taker_orders.entry(req.uuid) {
        Entry::Occupied(order) => {
            if !order.get().is_cancellable() {
                return MmError::err(CancelOrderError::UUIDNotFound { uuid: req.uuid });
            }
            let order = order.remove();
            delete_my_taker_order(ctx, order, TakerOrderCancellationReason::Cancelled)
                .compat()
                .await
                .ok();
            return Ok(CancelOrderResponse {
                result: "success".to_string(),
            });
        },
        // error is returned
        Entry::Vacant(_) => (),
    }
    MmError::err(CancelOrderError::UUIDNotFound { uuid: req.uuid })
}

pub async fn cancel_order_rpc(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let req: CancelOrderReq = try_s!(json::from_value(req));

    let ordermatch_ctx = try_s!(OrdermatchContext::from_ctx(&ctx));
    let maybe_order_mutex = ordermatch_ctx.maker_orders_ctx.lock().get_order(&req.uuid).cloned();
    if let Some(order_mutex) = maybe_order_mutex {
        let order = order_mutex.lock().await;
        if !order.is_cancellable() {
            return ERR!("Order {} is being matched now, can't cancel", req.uuid);
        }
        let removed_order_mutex = ordermatch_ctx.maker_orders_ctx.lock().remove_order(&order.uuid);
        // This checks that the order hasn't been removed by another process
        if removed_order_mutex.is_some() {
            maker_order_cancelled_p2p_notify(&ctx, &order);
            delete_my_maker_order(ctx, order.clone(), MakerOrderCancellationReason::Cancelled)
                .compat()
                .await
                .ok();
        }
        let res = json!({
            "result": "success"
        });
        return Response::builder()
            .body(json::to_vec(&res).expect("Serialization failed"))
            .map_err(|e| ERRL!("{}", e));
    }

    let mut taker_orders = ordermatch_ctx.my_taker_orders.lock().await;
    match taker_orders.entry(req.uuid) {
        Entry::Occupied(order) => {
            if !order.get().is_cancellable() {
                return ERR!("Order {} is being matched now, can't cancel", req.uuid);
            }
            let order = order.remove();
            delete_my_taker_order(ctx, order, TakerOrderCancellationReason::Cancelled)
                .compat()
                .await
                .ok();
            let res = json!({
                "result": "success"
            });
            return Response::builder()
                .body(json::to_vec(&res).expect("Serialization failed"))
                .map_err(|e| ERRL!("{}", e));
        },
        // error is returned
        Entry::Vacant(_) => (),
    }

    let res = json!({
        "error": format!("Order with uuid {} is not found", req.uuid),
    });
    Response::builder()
        .status(404)
        .body(json::to_vec(&res).expect("Serialization failed"))
        .map_err(|e| ERRL!("{}", e))
}

#[derive(Serialize)]
struct MakerOrderForMyOrdersRpc<'a> {
    #[serde(flatten)]
    order: MakerOrderForRpc<'a>,
    cancellable: bool,
    available_amount: BigDecimal,
}

impl<'a> From<&'a MakerOrder> for MakerOrderForMyOrdersRpc<'a> {
    fn from(order: &'a MakerOrder) -> MakerOrderForMyOrdersRpc<'a> {
        MakerOrderForMyOrdersRpc {
            order: order.into(),
            cancellable: order.is_cancellable(),
            available_amount: order.available_amount().into(),
        }
    }
}

#[derive(Serialize)]
struct TakerMatchForRpc<'a> {
    reserved: MakerReservedForRpc<'a>,
    connect: TakerConnectForRpc<'a>,
    connected: Option<MakerConnectedForRpc<'a>>,
    last_updated: u64,
}

#[allow(clippy::needless_borrow)]
impl<'a> From<&'a TakerMatch> for TakerMatchForRpc<'a> {
    fn from(taker_match: &'a TakerMatch) -> TakerMatchForRpc<'a> {
        TakerMatchForRpc {
            reserved: (&taker_match.reserved).into(),
            connect: (&taker_match.connect).into(),
            connected: taker_match.connected.as_ref().map(|connected| connected.into()),
            last_updated: taker_match.last_updated,
        }
    }
}

#[derive(Serialize)]
struct TakerOrderForRpc<'a> {
    created_at: u64,
    request: TakerRequestForRpc,
    matches: HashMap<Uuid, TakerMatchForRpc<'a>>,
    order_type: &'a OrderType,
    cancellable: bool,
    base_orderbook_ticker: &'a Option<String>,
    rel_orderbook_ticker: &'a Option<String>,
}

#[allow(clippy::needless_borrow)]
impl<'a> From<&'a TakerOrder> for TakerOrderForRpc<'a> {
    fn from(order: &'a TakerOrder) -> TakerOrderForRpc<'a> {
        TakerOrderForRpc {
            created_at: order.created_at,
            request: (&order.request).into(),
            matches: order
                .matches
                .iter()
                .map(|(uuid, taker_match)| (*uuid, taker_match.into()))
                .collect(),
            cancellable: order.is_cancellable(),
            order_type: &order.order_type,
            base_orderbook_ticker: &order.base_orderbook_ticker,
            rel_orderbook_ticker: &order.rel_orderbook_ticker,
        }
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Serialize)]
#[serde(tag = "type", content = "order")]
enum OrderForRpc<'a> {
    Maker(MakerOrderForRpc<'a>),
    Taker(TakerOrderForRpc<'a>),
}

pub async fn my_orders(ctx: MmArc) -> Result<Response<Vec<u8>>, String> {
    let ordermatch_ctx = try_s!(OrdermatchContext::from_ctx(&ctx));
    let my_maker_orders = ordermatch_ctx.maker_orders_ctx.lock().orders.clone();
    let mut maker_orders_map = HashMap::with_capacity(my_maker_orders.len());
    for (uuid, order_mutex) in my_maker_orders.iter() {
        let order = order_mutex.lock().await.clone();
        maker_orders_map.insert(uuid, order);
    }
    let maker_orders_for_rpc: HashMap<_, _> = maker_orders_map
        .iter()
        .map(|(uuid, order)| (uuid, MakerOrderForMyOrdersRpc::from(order)))
        .collect();

    let taker_orders = ordermatch_ctx.my_taker_orders.lock().await;
    let taker_orders_for_rpc: HashMap<_, _> = taker_orders
        .iter()
        .map(|(uuid, order)| (uuid, TakerOrderForRpc::from(order)))
        .collect();
    let res = json!({
        "result": {
            "maker_orders": maker_orders_for_rpc,
            "taker_orders": taker_orders_for_rpc,
        }
    });
    Response::builder()
        .body(json::to_vec(&res).expect("Serialization failed"))
        .map_err(|e| ERRL!("{}", e))
}

#[cfg(not(target_arch = "wasm32"))]
pub fn my_maker_orders_dir(ctx: &MmArc) -> PathBuf {
    ctx.dbdir().join("ORDERS").join("MY").join("MAKER")
}

#[cfg(not(target_arch = "wasm32"))]
fn my_taker_orders_dir(ctx: &MmArc) -> PathBuf {
    ctx.dbdir().join("ORDERS").join("MY").join("TAKER")
}

#[cfg(not(target_arch = "wasm32"))]
fn my_orders_history_dir(ctx: &MmArc) -> PathBuf {
    ctx.dbdir().join("ORDERS").join("MY").join("HISTORY")
}

#[cfg(not(target_arch = "wasm32"))]
pub fn my_maker_order_file_path(ctx: &MmArc, uuid: &Uuid) -> PathBuf {
    my_maker_orders_dir(ctx).join(format!("{uuid}.json"))
}

#[cfg(not(target_arch = "wasm32"))]
fn my_taker_order_file_path(ctx: &MmArc, uuid: &Uuid) -> PathBuf {
    my_taker_orders_dir(ctx).join(format!("{uuid}.json"))
}

#[cfg(not(target_arch = "wasm32"))]
fn my_order_history_file_path(ctx: &MmArc, uuid: &Uuid) -> PathBuf {
    my_orders_history_dir(ctx).join(format!("{uuid}.json"))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HistoricalOrder {
    #[serde(skip_serializing_if = "Option::is_none")]
    max_base_vol: Option<MmNumber>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min_base_vol: Option<MmNumber>,
    #[serde(skip_serializing_if = "Option::is_none")]
    price: Option<MmNumber>,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    conf_settings: Option<OrderConfirmationsSettings>,
}

pub async fn orders_kick_start(ctx: &MmArc) -> Result<HashSet<String>, String> {
    let ordermatch_ctx = try_s!(OrdermatchContext::from_ctx(ctx));

    let storage = MyOrdersStorage::new(ctx.clone());
    let saved_maker_orders = try_s!(storage.load_active_maker_orders().await);
    let saved_taker_orders = try_s!(storage.load_active_taker_orders().await);
    let mut coins = HashSet::with_capacity((saved_maker_orders.len() * 2) + (saved_taker_orders.len() * 2));

    {
        let mut maker_orders_ctx = ordermatch_ctx.maker_orders_ctx.lock();
        for order in saved_maker_orders {
            coins.insert(order.base.clone());
            coins.insert(order.rel.clone());
            maker_orders_ctx.add_order(ctx.weak(), order.clone(), None);
        }
    }

    let mut taker_orders = ordermatch_ctx.my_taker_orders.lock().await;
    for order in saved_taker_orders {
        coins.insert(order.request.base.clone());
        coins.insert(order.request.rel.clone());
        taker_orders.insert(order.request.uuid, order);
    }
    Ok(coins)
}

#[derive(Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum CancelBy {
    /// All orders of current node
    All,
    /// All orders of specific pair
    Pair { base: String, rel: String },
    /// All orders using the coin ticker as base or rel
    Coin { ticker: String },
}

pub async fn get_matching_orders(ctx: &MmArc, coins: &HashSet<String>) -> Result<Vec<Uuid>, String> {
    let ordermatch_ctx = try_s!(OrdermatchContext::from_ctx(ctx));
    let mut matching_orders = vec![];

    let maker_orders = ordermatch_ctx.maker_orders_ctx.lock().orders.clone();
    let taker_orders = ordermatch_ctx.my_taker_orders.lock().await;

    for (uuid, order) in maker_orders.iter() {
        let order = order.lock().await.clone();
        if (coins.contains(&order.base) || coins.contains(&order.rel)) && !order.is_cancellable() {
            matching_orders.push(*uuid);
        }
    }

    taker_orders.iter().for_each(|(uuid, order)| {
        if (coins.contains(&order.request.base) || coins.contains(&order.request.rel)) && !order.is_cancellable() {
            matching_orders.push(*uuid);
        };
    });

    Ok(matching_orders)
}

pub async fn cancel_orders_by(ctx: &MmArc, cancel_by: CancelBy) -> Result<(Vec<Uuid>, Vec<Uuid>), String> {
    let mut cancelled = vec![];
    let mut cancelled_maker_orders = vec![];
    let mut cancelled_taker_orders = vec![];
    let mut currently_matching = vec![];

    let ordermatch_ctx = try_s!(OrdermatchContext::from_ctx(ctx));
    let maker_orders = ordermatch_ctx.maker_orders_ctx.lock().orders.clone();
    let mut taker_orders = ordermatch_ctx.my_taker_orders.lock().await;

    macro_rules! cancel_maker_if_true {
        ($e: expr, $uuid: ident, $order: ident) => {
            if $e {
                if $order.is_cancellable() {
                    cancelled_maker_orders.push($order);
                    cancelled.push($uuid);
                    true
                } else {
                    currently_matching.push($uuid);
                    false
                }
            } else {
                false
            }
        };
    }

    macro_rules! cancel_taker_if_true {
        ($e: expr, $uuid: ident, $order: ident) => {
            if $e {
                if $order.is_cancellable() {
                    cancelled_taker_orders.push($order);
                    cancelled.push($uuid);
                    None
                } else {
                    currently_matching.push($uuid);
                    Some(($uuid, $order))
                }
            } else {
                Some(($uuid, $order))
            }
        };
    }

    match cancel_by {
        CancelBy::All => {
            let mut to_remove = Vec::new();
            for (uuid, order) in maker_orders.iter() {
                let uuid = *uuid;
                let order = order.lock().await.clone();
                if cancel_maker_if_true!(true, uuid, order) {
                    to_remove.push(uuid);
                }
            }
            let mut maker_order_ctx = ordermatch_ctx.maker_orders_ctx.lock();
            for uuid in to_remove.iter() {
                maker_order_ctx.remove_order(uuid);
            }
            *taker_orders = taker_orders
                .drain()
                .filter_map(|(uuid, order)| cancel_taker_if_true!(true, uuid, order))
                .collect();
        },
        CancelBy::Pair { base, rel } => {
            let mut to_remove = Vec::new();
            for (uuid, order) in maker_orders.iter() {
                let uuid = *uuid;
                let order = order.lock().await.clone();
                if cancel_maker_if_true!(order.base == base && order.rel == rel, uuid, order) {
                    to_remove.push(uuid);
                }
            }
            let mut maker_orders_ctx = ordermatch_ctx.maker_orders_ctx.lock();
            for uuid in to_remove.iter() {
                maker_orders_ctx.remove_order(uuid);
            }
            *taker_orders = taker_orders
                .drain()
                .filter_map(|(uuid, order)| {
                    cancel_taker_if_true!(order.request.base == base && order.request.rel == rel, uuid, order)
                })
                .collect();
        },
        CancelBy::Coin { ticker } => {
            let mut to_remove = Vec::new();
            for (uuid, order) in maker_orders.iter() {
                let uuid = *uuid;
                let order = order.lock().await.clone();
                if cancel_maker_if_true!(order.base == ticker || order.rel == ticker, uuid, order) {
                    to_remove.push(uuid);
                }
            }
            let mut maker_orders_ctx = ordermatch_ctx.maker_orders_ctx.lock();
            for uuid in to_remove.iter() {
                maker_orders_ctx.remove_order(uuid);
            }
            *taker_orders = taker_orders
                .drain()
                .filter_map(|(uuid, order)| {
                    cancel_taker_if_true!(order.request.base == ticker || order.request.rel == ticker, uuid, order)
                })
                .collect();
        },
    };
    for order in cancelled_maker_orders {
        maker_order_cancelled_p2p_notify(ctx, &order);
        delete_my_maker_order(ctx.clone(), order.clone(), MakerOrderCancellationReason::Cancelled)
            .compat()
            .await
            .ok();
    }
    for order in cancelled_taker_orders {
        delete_my_taker_order(ctx.clone(), order, TakerOrderCancellationReason::Cancelled)
            .compat()
            .await
            .ok();
    }
    Ok((cancelled, currently_matching))
}

pub async fn cancel_all_orders(
    ctx: MmArc,
    cancel_by: CancelBy,
) -> Result<CancelAllOrdersResponse, MmError<CancelAllOrdersError>> {
    cancel_orders_by(&ctx, cancel_by)
        .await
        .map(|(cancelled, currently_matching)| CancelAllOrdersResponse {
            cancelled,
            currently_matching,
        })
        .map_to_mm(CancelAllOrdersError::LegacyError)
}

pub async fn cancel_all_orders_rpc(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let cancel_by: CancelBy = try_s!(json::from_value(req["cancel_by"].clone()));

    let (cancelled, currently_matching) = try_s!(cancel_orders_by(&ctx, cancel_by).await);

    let res = json!({
        "result": {
            "cancelled": cancelled,
            "currently_matching": currently_matching,
        }
    });
    Response::builder()
        .body(json::to_vec(&res).expect("Serialization failed"))
        .map_err(|e| ERRL!("{}", e))
}

/// Subscribe to an orderbook topic (see [`orderbook_topic`]).
/// If the `request_orderbook` is true and the orderbook for the given pair of coins is not requested yet (or is not filled up yet),
/// request and fill the orderbook.
///
/// # Safety
///
/// The function locks [`MmCtx::p2p_ctx`] and [`MmCtx::ordermatch_ctx`]
async fn subscribe_to_orderbook_topic(
    ctx: &MmArc,
    base: &str,
    rel: &str,
    request_orderbook: bool,
) -> Result<(), String> {
    let current_timestamp = now_sec();
    let topic = orderbook_topic_from_base_rel(base, rel);
    let is_orderbook_filled = {
        let ordermatch_ctx = try_s!(OrdermatchContext::from_ctx(ctx));
        let mut orderbook = ordermatch_ctx.orderbook.lock();

        match orderbook.topics_subscribed_to.entry(topic.clone()) {
            Entry::Vacant(e) => {
                // we weren't subscribed to the topic yet
                e.insert(OrderbookRequestingState::NotRequested {
                    subscribed_at: current_timestamp,
                });
                subscribe_to_topic(ctx, topic.clone());
                // orderbook is not filled
                false
            },
            Entry::Occupied(e) => match e.get() {
                OrderbookRequestingState::Requested => {
                    // We are subscribed to the topic and the orderbook was requested already
                    true
                },
                OrderbookRequestingState::NotRequested { subscribed_at } => {
                    // We are subscribed to the topic. Also we didn't request the orderbook,
                    // True if enough time has passed for the orderbook to fill by OrdermatchRequest::SyncPubkeyOrderbookState.
                    *subscribed_at + ORDERBOOK_REQUESTING_TIMEOUT < current_timestamp
                },
            },
        }
    };

    if !is_orderbook_filled && request_orderbook {
        try_s!(request_and_fill_orderbook(ctx, base, rel).await);
    }

    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RpcOrderbookEntryV2 {
    pub coin: String,
    pub address: OrderbookAddress,
    pub price: MmNumberMultiRepr,
    pub pubkey: String,
    pub uuid: Uuid,
    pub is_mine: bool,
    pub base_max_volume: MmNumberMultiRepr,
    pub base_min_volume: MmNumberMultiRepr,
    pub rel_max_volume: MmNumberMultiRepr,
    pub rel_min_volume: MmNumberMultiRepr,
    pub conf_settings: Option<OrderConfirmationsSettings>,
}

fn choose_maker_confs_and_notas(
    maker_confs: Option<OrderConfirmationsSettings>,
    taker_req: &TakerRequest,
    maker_coin: &MmCoinEnum,
    taker_coin: &MmCoinEnum,
) -> SwapConfirmationsSettings {
    let maker_settings = maker_confs.unwrap_or(OrderConfirmationsSettings {
        base_confs: maker_coin.required_confirmations(),
        base_nota: maker_coin.requires_notarization(),
        rel_confs: taker_coin.required_confirmations(),
        rel_nota: taker_coin.requires_notarization(),
    });

    let (maker_coin_confs, maker_coin_nota, taker_coin_confs, taker_coin_nota) = match taker_req.conf_settings {
        Some(ref taker_settings) => match taker_req.action {
            TakerAction::Sell => {
                let maker_coin_confs = if taker_settings.rel_confs < maker_settings.base_confs {
                    taker_settings.rel_confs
                } else {
                    maker_settings.base_confs
                };
                let maker_coin_nota = if !taker_settings.rel_nota {
                    taker_settings.rel_nota
                } else {
                    maker_settings.base_nota
                };
                (
                    maker_coin_confs,
                    maker_coin_nota,
                    maker_settings.rel_confs,
                    maker_settings.rel_nota,
                )
            },
            TakerAction::Buy => {
                let maker_coin_confs = if taker_settings.base_confs < maker_settings.base_confs {
                    taker_settings.base_confs
                } else {
                    maker_settings.base_confs
                };
                let maker_coin_nota = if !taker_settings.base_nota {
                    taker_settings.base_nota
                } else {
                    maker_settings.base_nota
                };
                (
                    maker_coin_confs,
                    maker_coin_nota,
                    maker_settings.rel_confs,
                    maker_settings.rel_nota,
                )
            },
        },
        None => (
            maker_settings.base_confs,
            maker_settings.base_nota,
            maker_settings.rel_confs,
            maker_settings.rel_nota,
        ),
    };

    SwapConfirmationsSettings {
        maker_coin_confs,
        maker_coin_nota,
        taker_coin_confs,
        taker_coin_nota,
    }
}

fn choose_taker_confs_and_notas(
    taker_req: &TakerRequest,
    maker_conf_settings: &Option<OrderConfirmationsSettings>,
    maker_coin: &MmCoinEnum,
    taker_coin: &MmCoinEnum,
) -> SwapConfirmationsSettings {
    let (mut taker_coin_confs, mut taker_coin_nota, maker_coin_confs, maker_coin_nota) = match taker_req.action {
        TakerAction::Buy => match taker_req.conf_settings {
            Some(ref s) => (s.rel_confs, s.rel_nota, s.base_confs, s.base_nota),
            None => (
                taker_coin.required_confirmations(),
                taker_coin.requires_notarization(),
                maker_coin.required_confirmations(),
                maker_coin.requires_notarization(),
            ),
        },
        TakerAction::Sell => match taker_req.conf_settings {
            Some(ref s) => (s.base_confs, s.base_nota, s.rel_confs, s.rel_nota),
            None => (
                taker_coin.required_confirmations(),
                taker_coin.requires_notarization(),
                maker_coin.required_confirmations(),
                maker_coin.requires_notarization(),
            ),
        },
    };
    if let Some(settings_from_maker) = maker_conf_settings {
        if settings_from_maker.rel_confs < taker_coin_confs {
            taker_coin_confs = settings_from_maker.rel_confs;
        }
        if !settings_from_maker.rel_nota {
            taker_coin_nota = settings_from_maker.rel_nota;
        }
    }
    SwapConfirmationsSettings {
        maker_coin_confs,
        maker_coin_nota,
        taker_coin_confs,
        taker_coin_nota,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "address_type", content = "address_data")]
pub enum OrderbookAddress {
    Transparent(String),
    Shielded,
}

#[derive(Debug, Display)]
enum OrderbookAddrErr {
    AddrFromPubkeyError(String),
    CoinIsNotSupported(String),
    DeserializationError(json::Error),
    InvalidPlatformCoinProtocol(String),
    PlatformCoinConfIsNull(String),
}

impl From<json::Error> for OrderbookAddrErr {
    fn from(err: json::Error) -> Self {
        OrderbookAddrErr::DeserializationError(err)
    }
}

impl From<coins::tendermint::AccountIdFromPubkeyHexErr> for OrderbookAddrErr {
    fn from(err: coins::tendermint::AccountIdFromPubkeyHexErr) -> Self {
        OrderbookAddrErr::AddrFromPubkeyError(err.to_string())
    }
}

fn orderbook_address(
    ctx: &MmArc,
    coin: &str,
    conf: &Json,
    pubkey: &str,
    addr_format: UtxoAddressFormat,
) -> Result<OrderbookAddress, MmError<OrderbookAddrErr>> {
    let protocol: CoinProtocol = json::from_value(conf["protocol"].clone())?;
    match protocol {
        CoinProtocol::ERC20 { .. } | CoinProtocol::ETH { .. } | CoinProtocol::NFT { .. } => {
            coins::eth::addr_from_pubkey_str(pubkey)
                .map(OrderbookAddress::Transparent)
                .map_to_mm(OrderbookAddrErr::AddrFromPubkeyError)
        },
        // Todo: implement TRX address generation
        CoinProtocol::TRX { .. } => MmError::err(OrderbookAddrErr::CoinIsNotSupported(coin.to_owned())),
        CoinProtocol::UTXO { .. } | CoinProtocol::QTUM | CoinProtocol::QRC20 { .. } | CoinProtocol::BCH { .. } => {
            coins::utxo::address_by_conf_and_pubkey_str(coin, conf, pubkey, addr_format)
                .map(OrderbookAddress::Transparent)
                .map_to_mm(OrderbookAddrErr::AddrFromPubkeyError)
        },
        CoinProtocol::SLPTOKEN { platform, .. } => {
            let platform_conf = coin_conf(ctx, &platform);
            if platform_conf.is_null() {
                return MmError::err(OrderbookAddrErr::PlatformCoinConfIsNull(platform));
            }
            // TODO is there any way to make it better without duplicating the prefix in the SLP conf?
            let platform_protocol: CoinProtocol = json::from_value(platform_conf["protocol"].clone())?;
            match platform_protocol {
                CoinProtocol::BCH { slp_prefix } => coins::utxo::slp::slp_addr_from_pubkey_str(pubkey, &slp_prefix)
                    .map(OrderbookAddress::Transparent)
                    .mm_err(|e| OrderbookAddrErr::AddrFromPubkeyError(e.to_string())),
                _ => MmError::err(OrderbookAddrErr::InvalidPlatformCoinProtocol(platform)),
            }
        },
        CoinProtocol::TENDERMINT(protocol) => Ok(coins::tendermint::account_id_from_pubkey_hex(
            &protocol.account_prefix,
            pubkey,
        )
        .map(|id| OrderbookAddress::Transparent(id.to_string()))?),
        CoinProtocol::TENDERMINTTOKEN(proto) => {
            let platform_conf = coin_conf(ctx, &proto.platform);
            if platform_conf.is_null() {
                return MmError::err(OrderbookAddrErr::PlatformCoinConfIsNull(proto.platform));
            }
            // TODO is there any way to make it better without duplicating the prefix in the IBC conf?
            let platform_protocol: CoinProtocol = json::from_value(platform_conf["protocol"].clone())?;
            match platform_protocol {
                CoinProtocol::TENDERMINT(platform) => Ok(coins::tendermint::account_id_from_pubkey_hex(
                    &platform.account_prefix,
                    pubkey,
                )
                .map(|id| OrderbookAddress::Transparent(id.to_string()))?),
                _ => MmError::err(OrderbookAddrErr::InvalidPlatformCoinProtocol(format!(
                    "Platform protocol {platform_protocol:?} is not TENDERMINT"
                ))),
            }
        },
        CoinProtocol::ZHTLC { .. } => Ok(OrderbookAddress::Shielded),
        #[cfg(not(target_arch = "wasm32"))]
        // Todo: Shielded address is used for lightning for now, the lightning node public key can be used for the orderbook entry pubkey
        // Todo: instead of the platform coin pubkey which is used right now. But lightning payments are supposed to be private,
        // Todo: so maybe we should hide the node address in the orderbook, only the sending node and the receiving node should know about a payment,
        // Todo: a routing node will know about a payment it routed but not the sender or the receiver. This will require using a new keypair for every order/swap
        // Todo: similar to how it's done for zcoin.
        CoinProtocol::LIGHTNING { .. } => Ok(OrderbookAddress::Shielded),
        // TODO implement for SIA "this is needed to show the address in the orderbook"
        #[cfg(feature = "enable-sia")]
        CoinProtocol::SIA => MmError::err(OrderbookAddrErr::CoinIsNotSupported(coin.to_owned())),
        #[cfg(feature = "enable-solana")]
        CoinProtocol::SOLANA(_) => MmError::err(OrderbookAddrErr::CoinIsNotSupported(coin.to_owned())),
        #[cfg(feature = "enable-solana")]
        CoinProtocol::SOLANATOKEN(_) => MmError::err(OrderbookAddrErr::CoinIsNotSupported(coin.to_owned())),
    }
}
