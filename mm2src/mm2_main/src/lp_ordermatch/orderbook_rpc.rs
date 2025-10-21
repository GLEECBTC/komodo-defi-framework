use coins::{address_by_coin_conf_and_pubkey_str, coin_conf, is_wallet_only_conf};
use common::log::warn;
use common::{now_sec, HttpStatusCode};
use derive_more::Display;
use http::{Response, StatusCode};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::{BigRational, MmNumber, MmNumberMultiRepr};
use mm2_rpc::data::legacy::{AggregatedOrderbookEntry, OrderbookRequest, OrderbookResponse, RpcOrderbookEntry};
use num_traits::Zero;
use serde_json::{self as json, Value as Json};

use super::{
    addr_format_from_protocol_info, is_my_order, mm2_internal_pubkey_hex, orderbook_address,
    subscribe_to_orderbook_topic, OrdermatchContext, RpcOrderbookEntryV2,
};

#[derive(Debug, Serialize)]
pub struct AggregatedOrderbookEntryV2 {
    #[serde(flatten)]
    entry: RpcOrderbookEntryV2,
    base_max_volume_aggr: MmNumberMultiRepr,
    rel_max_volume_aggr: MmNumberMultiRepr,
}

fn build_aggregated_entries(entries: Vec<RpcOrderbookEntry>) -> (Vec<AggregatedOrderbookEntry>, MmNumber, MmNumber) {
    let mut total_base = BigRational::zero();
    let mut total_rel = BigRational::zero();
    let aggregated = entries
        .into_iter()
        .map(|entry| {
            total_base += entry.base_max_volume.as_ratio();
            total_rel += entry.rel_max_volume.as_ratio();
            AggregatedOrderbookEntry {
                entry,
                base_max_volume_aggr: MmNumber::from(total_base.clone()).into(),
                rel_max_volume_aggr: MmNumber::from(total_rel.clone()).into(),
            }
        })
        .collect();
    (aggregated, total_base.into(), total_rel.into())
}

fn build_aggregated_entries_v2(
    entries: Vec<RpcOrderbookEntryV2>,
) -> (Vec<AggregatedOrderbookEntryV2>, MmNumberMultiRepr, MmNumberMultiRepr) {
    let mut total_base = BigRational::zero();
    let mut total_rel = BigRational::zero();
    let aggregated = entries
        .into_iter()
        .map(|entry| {
            total_base += &entry.base_max_volume.rational;
            total_rel += &entry.rel_max_volume.rational;
            AggregatedOrderbookEntryV2 {
                entry,
                base_max_volume_aggr: MmNumber::from(total_base.clone()).into(),
                rel_max_volume_aggr: MmNumber::from(total_rel.clone()).into(),
            }
        })
        .collect();
    (aggregated, total_base.into(), total_rel.into())
}

pub async fn orderbook_rpc(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let req: OrderbookRequest = try_s!(json::from_value(req));
    if req.base == req.rel {
        return ERR!("Base and rel must be different coins");
    }
    let base_coin_conf = coin_conf(&ctx, &req.base);
    if base_coin_conf.is_null() {
        return ERR!("Coin {} is not found in config", req.base);
    }
    if is_wallet_only_conf(&base_coin_conf) {
        return ERR!("Base Coin {} is wallet only", req.base);
    }
    let rel_coin_conf = coin_conf(&ctx, &req.rel);
    if rel_coin_conf.is_null() {
        return ERR!("Coin {} is not found in config", req.rel);
    }
    if is_wallet_only_conf(&rel_coin_conf) {
        return ERR!("Base Coin {} is wallet only", req.rel);
    }
    let ordermatch_ctx = try_s!(OrdermatchContext::from_ctx(&ctx));
    let request_orderbook = true;
    let base_ticker = ordermatch_ctx.orderbook_ticker_bypass(&req.base);
    let rel_ticker = ordermatch_ctx.orderbook_ticker_bypass(&req.rel);
    if base_ticker == rel_ticker && base_coin_conf["protocol"] == rel_coin_conf["protocol"] {
        return ERR!("Base and rel coins have the same orderbook tickers and protocols.");
    }

    try_s!(subscribe_to_orderbook_topic(&ctx, &base_ticker, &rel_ticker, request_orderbook).await);

    let my_pubsecp = mm2_internal_pubkey_hex(&ctx, String::from).map_err(MmError::into_inner)?;

    let orderbook = ordermatch_ctx.orderbook.lock();
    let my_p2p_pubkeys = &orderbook.my_p2p_pubkeys;

    let mut asks = match orderbook.unordered.get(&(base_ticker.clone(), rel_ticker.clone())) {
        Some(uuids) => {
            let mut orderbook_entries = Vec::with_capacity(uuids.len());
            for uuid in uuids {
                let ask = orderbook.order_set.get(uuid).ok_or(ERRL!(
                    "Orderbook::unordered contains {:?} uuid that is not in Orderbook::order_set",
                    uuid
                ))?;
                let address_format = addr_format_from_protocol_info(&ask.base_protocol_info);
                let address = try_s!(address_by_coin_conf_and_pubkey_str(
                    &ctx,
                    &req.base,
                    &base_coin_conf,
                    &ask.pubkey,
                    address_format,
                ));
                let is_mine = is_my_order(&ask.pubkey, &my_pubsecp, my_p2p_pubkeys);
                orderbook_entries.push(ask.as_rpc_entry_ask(address, is_mine));
            }
            orderbook_entries
        },
        None => vec![],
    };
    asks.sort_unstable_by(|ask1, ask2| ask1.price_rat.cmp(&ask2.price_rat));
    let (mut asks, total_asks_base_vol, total_asks_rel_vol) = build_aggregated_entries(asks);
    asks.reverse();

    let mut bids = match orderbook.unordered.get(&(rel_ticker, base_ticker)) {
        Some(uuids) => {
            let mut orderbook_entries = Vec::with_capacity(uuids.len());
            for uuid in uuids {
                let bid = orderbook.order_set.get(uuid).ok_or(ERRL!(
                    "Orderbook::unordered contains {:?} uuid that is not in Orderbook::order_set",
                    uuid
                ))?;
                let address_format = addr_format_from_protocol_info(&bid.base_protocol_info);
                let address = try_s!(address_by_coin_conf_and_pubkey_str(
                    &ctx,
                    &req.rel,
                    &rel_coin_conf,
                    &bid.pubkey,
                    address_format,
                ));
                let is_mine = is_my_order(&bid.pubkey, &my_pubsecp, my_p2p_pubkeys);
                orderbook_entries.push(bid.as_rpc_entry_bid(address, is_mine));
            }
            orderbook_entries
        },
        None => vec![],
    };
    bids.sort_unstable_by(|bid1, bid2| bid2.price_rat.cmp(&bid1.price_rat));
    let (bids, total_bids_base_vol, total_bids_rel_vol) = build_aggregated_entries(bids);

    let response = OrderbookResponse {
        num_asks: asks.len(),
        num_bids: bids.len(),
        ask_depth: 0,
        asks,
        base: req.base,
        bid_depth: 0,
        bids,
        netid: ctx.netid(),
        rel: req.rel,
        timestamp: now_sec(),
        total_asks_base: total_asks_base_vol.into(),
        total_asks_rel: total_asks_rel_vol.into(),
        total_bids_base: total_bids_base_vol.into(),
        total_bids_rel: total_bids_rel_vol.into(),
    };
    let response = try_s!(json::to_vec(&response));
    Ok(try_s!(Response::builder().body(response)))
}

#[derive(Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum OrderbookRpcError {
    BaseRelSame,
    BaseRelSameOrderbookTickersAndProtocols,
    CoinConfigNotFound(String),
    CoinIsWalletOnly(String),
    P2PSubscribeError(String),
    Internal(String),
}

impl HttpStatusCode for OrderbookRpcError {
    fn status_code(&self) -> StatusCode {
        match self {
            OrderbookRpcError::BaseRelSame
            | OrderbookRpcError::BaseRelSameOrderbookTickersAndProtocols
            | OrderbookRpcError::CoinConfigNotFound(_)
            | OrderbookRpcError::CoinIsWalletOnly(_) => StatusCode::BAD_REQUEST,
            OrderbookRpcError::P2PSubscribeError(_) | OrderbookRpcError::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            },
        }
    }
}

#[derive(Serialize)]
pub struct OrderbookV2Response {
    asks: Vec<AggregatedOrderbookEntryV2>,
    base: String,
    bids: Vec<AggregatedOrderbookEntryV2>,
    net_id: u16,
    num_asks: usize,
    num_bids: usize,
    rel: String,
    timestamp: u64,
    total_asks_base_vol: MmNumberMultiRepr,
    total_asks_rel_vol: MmNumberMultiRepr,
    total_bids_base_vol: MmNumberMultiRepr,
    total_bids_rel_vol: MmNumberMultiRepr,
}

enum GetTradeableCoinConfErr {
    CoinConfigNotFound(String),
    CoinIsWalletOnly(String),
}

fn get_tradeable_coin_conf(ctx: &MmArc, ticker: &str) -> MmResult<Json, GetTradeableCoinConfErr> {
    let conf = coin_conf(ctx, ticker);
    if conf.is_null() {
        return MmError::err(GetTradeableCoinConfErr::CoinConfigNotFound(ticker.to_owned()));
    }
    if is_wallet_only_conf(&conf) {
        return MmError::err(GetTradeableCoinConfErr::CoinIsWalletOnly(ticker.to_owned()));
    }
    Ok(conf)
}

impl From<GetTradeableCoinConfErr> for OrderbookRpcError {
    fn from(err: GetTradeableCoinConfErr) -> Self {
        match err {
            GetTradeableCoinConfErr::CoinConfigNotFound(ticker) => OrderbookRpcError::CoinConfigNotFound(ticker),
            GetTradeableCoinConfErr::CoinIsWalletOnly(ticker) => OrderbookRpcError::CoinIsWalletOnly(ticker),
        }
    }
}

pub async fn orderbook_rpc_v2(
    ctx: MmArc,
    req: OrderbookRequest,
) -> Result<OrderbookV2Response, MmError<OrderbookRpcError>> {
    if req.base == req.rel {
        return MmError::err(OrderbookRpcError::BaseRelSame);
    }
    let base_coin_conf = get_tradeable_coin_conf(&ctx, &req.base).map_mm_err()?;

    let rel_coin_conf = get_tradeable_coin_conf(&ctx, &req.rel).map_mm_err()?;

    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).expect("ctx is available");
    let base_ticker = ordermatch_ctx.orderbook_ticker_bypass(&req.base);
    let rel_ticker = ordermatch_ctx.orderbook_ticker_bypass(&req.rel);
    if base_ticker == rel_ticker && base_coin_conf["protocol"] == rel_coin_conf["protocol"] {
        return MmError::err(OrderbookRpcError::BaseRelSameOrderbookTickersAndProtocols);
    }

    let request_orderbook = true;
    subscribe_to_orderbook_topic(&ctx, &base_ticker, &rel_ticker, request_orderbook)
        .await
        .map_to_mm(OrderbookRpcError::P2PSubscribeError)?;

    let my_pubsecp = mm2_internal_pubkey_hex(&ctx, OrderbookRpcError::Internal)?;
    let orderbook = ordermatch_ctx.orderbook.lock();
    let my_p2p_pubkeys = &orderbook.my_p2p_pubkeys;

    let mut asks = match orderbook.unordered.get(&(base_ticker.clone(), rel_ticker.clone())) {
        Some(uuids) => {
            let mut orderbook_entries = Vec::new();
            for uuid in uuids {
                let ask = match orderbook.order_set.get(uuid) {
                    Some(a) => a,
                    None => {
                        warn!("unordered contains {:?} uuid that is not in order_set", uuid);
                        continue;
                    },
                };
                let address_format = addr_format_from_protocol_info(&ask.base_protocol_info);
                let address = match orderbook_address(&ctx, &req.base, &base_coin_conf, &ask.pubkey, address_format) {
                    Ok(a) => a,
                    Err(e) => {
                        warn!("Error {} on getting address for order {}", e, ask.uuid);
                        continue;
                    },
                };
                let is_mine = is_my_order(&ask.pubkey, &my_pubsecp, my_p2p_pubkeys);
                orderbook_entries.push(ask.as_rpc_v2_entry_ask(address, is_mine));
            }
            orderbook_entries
        },
        None => Vec::new(),
    };
    asks.sort_unstable_by(|ask1, ask2| ask1.price.rational.cmp(&ask2.price.rational));
    let (mut asks, total_asks_base_vol, total_asks_rel_vol) = build_aggregated_entries_v2(asks);
    asks.reverse();

    let mut bids = match orderbook.unordered.get(&(rel_ticker, base_ticker)) {
        Some(uuids) => {
            let mut orderbook_entries = vec![];
            for uuid in uuids {
                let bid = match orderbook.order_set.get(uuid) {
                    Some(b) => b,
                    None => {
                        warn!("unordered contains {:?} uuid that is not in order_set", uuid);
                        continue;
                    },
                };
                let address_format = addr_format_from_protocol_info(&bid.base_protocol_info);
                let address = match orderbook_address(&ctx, &req.rel, &rel_coin_conf, &bid.pubkey, address_format) {
                    Ok(a) => a,
                    Err(e) => {
                        warn!("Error {} on getting address for order {}", e, bid.uuid);
                        continue;
                    },
                };
                let is_mine = is_my_order(&bid.pubkey, &my_pubsecp, my_p2p_pubkeys);
                orderbook_entries.push(bid.as_rpc_v2_entry_bid(address, is_mine));
            }
            orderbook_entries
        },
        None => vec![],
    };
    bids.sort_unstable_by(|bid1, bid2| bid2.price.rational.cmp(&bid1.price.rational));
    let (bids, total_bids_base_vol, total_bids_rel_vol) = build_aggregated_entries_v2(bids);

    Ok(OrderbookV2Response {
        num_asks: asks.len(),
        num_bids: bids.len(),
        asks,
        bids,
        net_id: ctx.netid(),
        base: req.base,
        rel: req.rel,
        timestamp: now_sec(),
        total_asks_base_vol,
        total_asks_rel_vol,
        total_bids_base_vol,
        total_bids_rel_vol,
    })
}
