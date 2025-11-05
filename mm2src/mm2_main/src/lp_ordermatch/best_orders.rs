use coins::{address_by_coin_conf_and_pubkey_str, coin_conf, is_wallet_only_conf, is_wallet_only_ticker};
use common::{log, HttpStatusCode};
use derive_more::Display;
use http::{Response, StatusCode};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_libp2p::application::request_response::{
    ordermatch::{BestOrdersAction, OrdermatchRequest},
    P2PRequest,
};
use mm2_number::{BigRational, MmNumber};
use mm2_rpc::data::legacy::OrderConfirmationsSettings;
use num_traits::Zero;
use serde_json::{self as json, Value as Json};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use super::{
    addr_format_from_protocol_info, is_my_order, mm2_internal_pubkey_hex, orderbook_address, BaseRelProtocolInfo,
    OrderbookP2PItemWithProof, OrdermatchContext, RpcOrderbookEntryV2,
};
use crate::lp_network::request_any_relay;

#[derive(Debug, Deserialize)]
pub struct BestOrdersRequest {
    coin: String,
    action: BestOrdersAction,
    volume: MmNumber,
}

#[derive(Debug, Deserialize, Serialize)]
struct BestOrdersP2PRes {
    orders: HashMap<String, Vec<OrderbookP2PItemWithProof>>,
    #[serde(default)]
    protocol_infos: HashMap<Uuid, BaseRelProtocolInfo>,
    #[serde(default)]
    conf_infos: HashMap<Uuid, OrderConfirmationsSettings>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum RequestBestOrdersBy {
    #[serde(rename = "volume")]
    Volume(MmNumber),
    #[serde(rename = "number")]
    Number(usize),
}

#[derive(Debug, Deserialize)]
pub struct BestOrdersRequestV2 {
    coin: String,
    action: BestOrdersAction,
    request_by: RequestBestOrdersBy,
    #[serde(default)]
    exclude_mine: bool,
}

pub fn process_best_orders_p2p_request(
    ctx: MmArc,
    coin: String,
    action: BestOrdersAction,
    required_volume: BigRational,
) -> Result<Option<Vec<u8>>, String> {
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).expect("ordermatch_ctx must exist at this point");
    let orderbook = ordermatch_ctx.orderbook.lock();
    let search_pairs_in = match action {
        BestOrdersAction::Buy => &orderbook.pairs_existing_for_base,
        BestOrdersAction::Sell => &orderbook.pairs_existing_for_rel,
    };
    let tickers = some_or_return_ok_none!(search_pairs_in.get(&coin));
    let mut result = HashMap::new();
    let pairs = tickers.iter().map(|ticker| match action {
        BestOrdersAction::Buy => (coin.clone(), ticker.clone()),
        BestOrdersAction::Sell => (ticker.clone(), coin.clone()),
    });

    let mut protocol_infos = HashMap::new();
    let mut conf_infos = HashMap::new();

    for pair in pairs {
        let orders = match orderbook.ordered.get(&pair) {
            Some(orders) => orders,
            None => {
                log::debug!("No orders for pair {:?}", pair);
                continue;
            },
        };
        let mut best_orders = vec![];
        let mut collected_volume = BigRational::zero();
        for ordered in orders {
            match orderbook.order_set.get(&ordered.uuid) {
                Some(o) => {
                    let min_volume = match action {
                        BestOrdersAction::Buy => o.min_volume.clone(),
                        BestOrdersAction::Sell => &o.min_volume * &o.price,
                    };
                    if min_volume > required_volume {
                        log::debug!("Order {} min_vol {:?} > {:?}", o.uuid, min_volume, required_volume);
                        continue;
                    }

                    let max_volume = match action {
                        BestOrdersAction::Buy => o.max_volume.clone(),
                        BestOrdersAction::Sell => &o.max_volume * &o.price,
                    };
                    let order_w_proof = orderbook.orderbook_item_with_proof(o.clone());
                    protocol_infos.insert(order_w_proof.order.uuid, order_w_proof.order.base_rel_proto_info());
                    if let Some(ref info) = order_w_proof.order.conf_settings {
                        conf_infos.insert(order_w_proof.order.uuid, info.clone());
                    }
                    best_orders.push(order_w_proof.into());

                    collected_volume += max_volume;
                    if collected_volume >= required_volume {
                        break;
                    }
                },
                None => {
                    log::debug!("No order with uuid {:?}", ordered.uuid);
                    continue;
                },
            };
        }
        match action {
            BestOrdersAction::Buy => result.insert(pair.1, best_orders),
            BestOrdersAction::Sell => result.insert(pair.0, best_orders),
        };
    }

    // Drop mutability of result, protocol_infos and conf_infos
    let result = result;
    let protocol_infos = protocol_infos;
    let conf_infos = conf_infos;

    let response = BestOrdersP2PRes {
        orders: result,
        protocol_infos,
        conf_infos,
    };
    let encoded = rmp_serde::to_vec(&response).expect("rmp_serde::to_vec should not fail here");
    Ok(Some(encoded))
}

pub fn process_best_orders_p2p_request_by_number(
    ctx: MmArc,
    coin: String,
    action: BestOrdersAction,
    number: usize,
) -> Result<Option<Vec<u8>>, String> {
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).expect("ordermatch_ctx must exist at this point");
    let orderbook = ordermatch_ctx.orderbook.lock();
    let search_pairs_in = match action {
        BestOrdersAction::Buy => &orderbook.pairs_existing_for_base,
        BestOrdersAction::Sell => &orderbook.pairs_existing_for_rel,
    };
    let tickers = some_or_return_ok_none!(search_pairs_in.get(&coin));
    let mut result = HashMap::new();
    let pairs = tickers.iter().map(|ticker| match action {
        BestOrdersAction::Buy => (coin.clone(), ticker.clone()),
        BestOrdersAction::Sell => (ticker.clone(), coin.clone()),
    });

    let mut protocol_infos = HashMap::new();
    let mut conf_infos = HashMap::new();

    for pair in pairs {
        let orders = match orderbook.ordered.get(&pair) {
            Some(orders) => orders.clone(),
            None => {
                log::debug!("No orders for pair {:?}", pair);
                continue;
            },
        };
        let mut best_orders = vec![];
        for ordered in orders.iter().take(number) {
            match orderbook.order_set.get(&ordered.uuid) {
                Some(o) => {
                    let order_w_proof = orderbook.orderbook_item_with_proof(o.clone());
                    protocol_infos.insert(order_w_proof.order.uuid, order_w_proof.order.base_rel_proto_info());
                    if let Some(ref info) = order_w_proof.order.conf_settings {
                        conf_infos.insert(order_w_proof.order.uuid, info.clone());
                    }
                    best_orders.push(order_w_proof.into());
                },
                None => {
                    log::debug!("No order with uuid {:?}", ordered.uuid);
                    continue;
                },
            };
        }
        match action {
            BestOrdersAction::Buy => result.insert(pair.1, best_orders),
            BestOrdersAction::Sell => result.insert(pair.0, best_orders),
        };
    }

    // Drop mutability of result, protocol_infos and conf_infos
    let result = result;
    let protocol_infos = protocol_infos;
    let conf_infos = conf_infos;

    let response = BestOrdersP2PRes {
        orders: result,
        protocol_infos,
        conf_infos,
    };
    let encoded = rmp_serde::to_vec(&response).expect("rmp_serde::to_vec should not fail here");
    Ok(Some(encoded))
}

pub async fn best_orders_rpc(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let req: BestOrdersRequest = try_s!(json::from_value(req));
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    if is_wallet_only_ticker(&ctx, &req.coin) {
        return ERR!("Coin {} is wallet only", &req.coin);
    }
    let p2p_request = OrdermatchRequest::BestOrders {
        coin: ordermatch_ctx.orderbook_ticker_bypass(&req.coin),
        action: req.action,
        volume: req.volume.into(),
    };

    let best_orders_res =
        try_s!(request_any_relay::<BestOrdersP2PRes>(ctx.clone(), P2PRequest::Ordermatch(p2p_request)).await);
    let mut response = HashMap::new();
    if let Some((p2p_response, peer_id)) = best_orders_res {
        log::debug!("Got best orders {:?} from peer {}", p2p_response, peer_id);
        let my_pubsecp = mm2_internal_pubkey_hex(&ctx, String::from).map_err(MmError::into_inner)?;
        let my_p2p_pubkeys = ordermatch_ctx.orderbook.lock().my_p2p_pubkeys.clone();
        for (coin, orders_w_proofs) in p2p_response.orders {
            let coin_conf = coin_conf(&ctx, &coin);
            if coin_conf.is_null() {
                log::warn!("Coin {} is not found in config", coin);
                continue;
            }
            if is_wallet_only_conf(&coin_conf) {
                log::warn!(
                    "Coin {} was removed from best orders because it's defined as wallet only in config",
                    coin
                );
                continue;
            }
            for order_w_proof in orders_w_proofs {
                let order = order_w_proof.order;
                let empty_proto_info = BaseRelProtocolInfo::default();
                let proto_infos = p2p_response
                    .protocol_infos
                    .get(&order.uuid)
                    .unwrap_or(&empty_proto_info);
                let addr_format = match req.action {
                    BestOrdersAction::Buy => addr_format_from_protocol_info(&proto_infos.rel),
                    BestOrdersAction::Sell => addr_format_from_protocol_info(&proto_infos.base),
                };
                let address =
                    match address_by_coin_conf_and_pubkey_str(&ctx, &coin, &coin_conf, &order.pubkey, addr_format) {
                        Ok(a) => a,
                        Err(e) => {
                            log::error!("Error {} getting coin {} address from pubkey {}", e, coin, order.pubkey);
                            continue;
                        },
                    };
                let conf_settings = p2p_response.conf_infos.get(&order.uuid);
                let is_mine = is_my_order(&order.pubkey, &my_pubsecp, &my_p2p_pubkeys);
                let entry = match req.action {
                    BestOrdersAction::Buy => order.as_rpc_best_orders_buy(address, conf_settings, is_mine),
                    BestOrdersAction::Sell => order.as_rpc_best_orders_sell(address, conf_settings, is_mine),
                };
                if let Some(original_tickers) = ordermatch_ctx.original_tickers.get(&coin) {
                    for ticker in original_tickers {
                        let mut original_entry = entry.clone();
                        original_entry.coin = ticker.to_owned();
                        response
                            .entry(ticker.to_owned())
                            .or_insert_with(Vec::new)
                            .push(original_entry);
                    }
                }
                response.entry(coin.clone()).or_insert_with(Vec::new).push(entry);
            }
        }
    } else {
        return Err("No response from any peer".to_string());
    }

    let res = json!({ "result": response, "original_tickers": &ordermatch_ctx.original_tickers });
    Response::builder()
        .body(json::to_vec(&res).expect("Serialization failed"))
        .map_err(|e| ERRL!("{}", e))
}

#[derive(Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum BestOrdersRpcError {
    CoinIsWalletOnly(String),
    P2PError(String),
    CtxError(String),
}

impl HttpStatusCode for BestOrdersRpcError {
    fn status_code(&self) -> StatusCode {
        match self {
            BestOrdersRpcError::CoinIsWalletOnly(_) => StatusCode::BAD_REQUEST,
            BestOrdersRpcError::P2PError(_) | BestOrdersRpcError::CtxError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[derive(Serialize)]
pub struct BestOrdersV2Response {
    orders: HashMap<String, Vec<RpcOrderbookEntryV2>>,
    original_tickers: HashMap<String, HashSet<String>>,
}

pub async fn best_orders_rpc_v2(
    ctx: MmArc,
    req: BestOrdersRequestV2,
) -> Result<BestOrdersV2Response, MmError<BestOrdersRpcError>> {
    if is_wallet_only_ticker(&ctx, &req.coin) {
        return MmError::err(BestOrdersRpcError::CoinIsWalletOnly(req.coin));
    }
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    let p2p_request = match req.request_by {
        RequestBestOrdersBy::Volume(mm_number) => OrdermatchRequest::BestOrders {
            coin: ordermatch_ctx.orderbook_ticker_bypass(&req.coin),
            action: req.action,
            volume: mm_number.into(),
        },
        RequestBestOrdersBy::Number(size) => OrdermatchRequest::BestOrdersByNumber {
            coin: ordermatch_ctx.orderbook_ticker_bypass(&req.coin),
            action: req.action,
            number: size,
        },
    };

    let best_orders_res = request_any_relay::<BestOrdersP2PRes>(ctx.clone(), P2PRequest::Ordermatch(p2p_request))
        .await
        .mm_err(|e| BestOrdersRpcError::P2PError(format!("{e:?}")))?;
    let mut orders = HashMap::new();
    if let Some((p2p_response, peer_id)) = best_orders_res {
        log::debug!("Got best orders {:?} from peer {}", p2p_response, peer_id);
        let my_pubsecp = mm2_internal_pubkey_hex(&ctx, BestOrdersRpcError::CtxError)?;
        let my_p2p_pubkeys = ordermatch_ctx.orderbook.lock().my_p2p_pubkeys.clone();

        for (coin, orders_w_proofs) in p2p_response.orders {
            let coin_conf = coin_conf(&ctx, &coin);
            if coin_conf.is_null() {
                log::warn!("Coin {} is not found in config", coin);
                continue;
            }
            if is_wallet_only_conf(&coin_conf) {
                log::warn!(
                    "Coin {} was removed from best orders because it's defined as wallet only in config",
                    coin
                );
                continue;
            }
            for order_w_proof in orders_w_proofs {
                let order = order_w_proof.order;
                let is_mine = is_my_order(&order.pubkey, &my_pubsecp, &my_p2p_pubkeys);
                if req.exclude_mine && is_mine {
                    continue;
                }
                let empty_proto_info = BaseRelProtocolInfo::default();
                let proto_infos = p2p_response
                    .protocol_infos
                    .get(&order.uuid)
                    .unwrap_or(&empty_proto_info);
                let addr_format = match req.action {
                    BestOrdersAction::Buy => addr_format_from_protocol_info(&proto_infos.rel),
                    BestOrdersAction::Sell => addr_format_from_protocol_info(&proto_infos.base),
                };
                let address = match orderbook_address(&ctx, &coin, &coin_conf, &order.pubkey, addr_format) {
                    Ok(a) => a,
                    Err(e) => {
                        log::error!("Error {} getting coin {} address from pubkey {}", e, coin, order.pubkey);
                        continue;
                    },
                };
                let conf_settings = p2p_response.conf_infos.get(&order.uuid);
                let entry = match req.action {
                    BestOrdersAction::Buy => order.as_rpc_best_orders_buy_v2(address, conf_settings, is_mine),
                    BestOrdersAction::Sell => order.as_rpc_best_orders_sell_v2(address, conf_settings, is_mine),
                };
                if let Some(original_tickers) = ordermatch_ctx.original_tickers.get(&coin) {
                    for ticker in original_tickers {
                        let mut original_entry = entry.clone();
                        original_entry.coin = ticker.to_owned();
                        orders
                            .entry(ticker.to_owned())
                            .or_insert_with(Vec::new)
                            .push(original_entry);
                    }
                }
                orders.entry(coin.clone()).or_insert_with(Vec::new).push(entry);
            }
        }
    } else {
        return MmError::err(BestOrdersRpcError::P2PError("No response from any peer".to_string()));
    }

    Ok(BestOrdersV2Response {
        orders,
        original_tickers: ordermatch_ctx.original_tickers.clone(),
    })
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod best_orders_test {
    use super::*;
    use crate::lp_ordermatch::ordermatch_tests::make_random_orders;
    use crate::lp_ordermatch::{OrderbookItem, TrieProof};
    use common::new_uuid;
    use std::iter::FromIterator;

    #[test]
    fn check_best_orders_p2p_res_serde() {
        #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
        struct BestOrderV1 {
            pubkey: String,
            base: String,
            rel: String,
            price: BigRational,
            max_volume: BigRational,
            min_volume: BigRational,
            uuid: Uuid,
            created_at: u64,
        }

        impl From<OrderbookItem> for BestOrderV1 {
            fn from(o: OrderbookItem) -> Self {
                BestOrderV1 {
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

        #[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
        struct BestOrderWithProofV1 {
            /// Orderbook item
            order: BestOrderV1,
            /// Last pubkey message payload that contains most recent pair trie root
            last_message_payload: Vec<u8>,
            /// Proof confirming that orderbook item is in the pair trie
            proof: TrieProof,
        }

        #[derive(Debug, Deserialize, PartialEq, Serialize)]
        struct BestOrdersResV1 {
            orders: HashMap<String, Vec<BestOrderWithProofV1>>,
        }

        let orders = make_random_orders("".into(), &[1; 32], "RICK".into(), "MORTY".into(), 10);
        let v1_orders: Vec<_> = orders
            .clone()
            .into_iter()
            .map(|order| BestOrderWithProofV1 {
                order: order.into(),
                last_message_payload: vec![],
                proof: vec![],
            })
            .collect();

        let v1 = BestOrdersResV1 {
            orders: HashMap::from_iter(std::iter::once(("RICK".into(), v1_orders))),
        };

        let v1_serialized = rmp_serde::to_vec_named(&v1).unwrap();

        let mut new: BestOrdersP2PRes = rmp_serde::from_slice(&v1_serialized).unwrap();
        new.protocol_infos.insert(
            new_uuid(),
            BaseRelProtocolInfo {
                base: vec![1],
                rel: vec![2],
            },
        );
        new.conf_infos.insert(new_uuid(), OrderConfirmationsSettings::default());

        let new_serialized = rmp_serde::to_vec_named(&new).unwrap();

        let v1_from_new: BestOrdersResV1 = rmp_serde::from_slice(&new_serialized).unwrap();
        assert_eq!(v1, v1_from_new);

        #[derive(Debug, Deserialize, PartialEq, Serialize)]
        struct BestOrdersResV2 {
            orders: HashMap<String, Vec<OrderbookP2PItemWithProof>>,
            #[serde(default)]
            protocol_infos: HashMap<Uuid, BaseRelProtocolInfo>,
        }
        let v2_orders: Vec<_> = orders
            .into_iter()
            .map(|order| OrderbookP2PItemWithProof {
                order: order.into(),
                last_message_payload: vec![],
                proof: vec![],
            })
            .collect();

        let v2 = BestOrdersResV2 {
            orders: HashMap::from_iter(std::iter::once(("RICK".into(), v2_orders))),
            protocol_infos: HashMap::from_iter(std::iter::once((
                new_uuid(),
                BaseRelProtocolInfo {
                    base: vec![1],
                    rel: vec![2],
                },
            ))),
        };

        let v2_serialized = rmp_serde::to_vec_named(&v2).unwrap();

        let mut new: BestOrdersP2PRes = rmp_serde::from_slice(&v2_serialized).unwrap();
        new.conf_infos.insert(new_uuid(), OrderConfirmationsSettings::default());

        let new_serialized = rmp_serde::to_vec_named(&new).unwrap();

        let v2_from_new: BestOrdersResV2 = rmp_serde::from_slice(&new_serialized).unwrap();
        assert_eq!(v2, v2_from_new);
    }
}
