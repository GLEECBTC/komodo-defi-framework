use super::{orderbook_topic_from_base_rel, OrdermatchContext};
use crate::lp_network::request_any_relay;
use coins::is_wallet_only_ticker;
use common::log;
use http::Response;
use mm2_core::mm_ctx::MmArc;
use mm2_libp2p::application::request_response::{ordermatch::OrdermatchRequest, P2PRequest};
use serde_json::{self as json, Value as Json};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct OrderbookDepthReq {
    pairs: Vec<(String, String)>,
}

#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
struct PairDepth {
    asks: usize,
    bids: usize,
}

#[derive(Debug, Deserialize, Serialize)]
struct OrderbookDepthP2PResponse {
    depth: HashMap<(String, String), PairDepth>,
}

#[derive(Debug, Deserialize, Serialize)]
struct PairWithDepth {
    pair: (String, String),
    depth: PairDepth,
}

pub async fn orderbook_depth_rpc(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).unwrap();
    let req: OrderbookDepthReq = try_s!(json::from_value(req));

    let wallet_only_pairs: Vec<_> = req
        .pairs
        .iter()
        .filter(|pair| is_wallet_only_ticker(&ctx, &pair.0) || is_wallet_only_ticker(&ctx, &pair.1))
        .collect();

    if !wallet_only_pairs.is_empty() {
        return ERR!("Pairs {:?} have wallet only coins", wallet_only_pairs);
    }

    let mut result = Vec::with_capacity(req.pairs.len());

    // the Iter::filter uses &Self::Item, which is undesirable, we need owned pair
    #[allow(clippy::unnecessary_filter_map)]
    let mut to_request_from_relay: Vec<_> = {
        let orderbook = ordermatch_ctx.orderbook.lock();
        let subs = ordermatch_ctx.orderbook_subscriptions.read();
        req.pairs
            .into_iter()
            .filter_map(|original_pair| {
                let orderbook_pair = ordermatch_ctx.orderbook_pair_bypass(&original_pair);
                let topic = orderbook_topic_from_base_rel(&orderbook_pair.0, &orderbook_pair.1);
                if subs.contains_key(&topic) {
                    let asks = orderbook
                        .unordered
                        .get(&orderbook_pair)
                        .map_or(0, |orders| orders.len());
                    let reversed = (orderbook_pair.1, orderbook_pair.0);
                    let bids = orderbook.unordered.get(&reversed).map_or(0, |orders| orders.len());
                    result.push(PairWithDepth {
                        pair: original_pair,
                        depth: PairDepth { asks, bids },
                    });
                    None
                } else {
                    Some((orderbook_pair, original_pair))
                }
            })
            .collect()
    };

    if !to_request_from_relay.is_empty() {
        let p2p_request = OrdermatchRequest::OrderbookDepth {
            pairs: to_request_from_relay
                .iter()
                .map(|(orderbook_pair, _)| orderbook_pair.clone())
                .collect(),
        };
        log::debug!("Sending request_any_relay({:?})", p2p_request);
        let p2p_response = try_s!(
            request_any_relay::<OrderbookDepthP2PResponse>(ctx.clone(), P2PRequest::Ordermatch(p2p_request)).await
        );
        log::debug!("Received response {:?}", p2p_response);
        if let Some((response, _)) = p2p_response {
            for (pair, depth) in response.depth {
                to_request_from_relay = to_request_from_relay
                    .into_iter()
                    .filter_map(|(orderbook_pair, original_pair)| {
                        if orderbook_pair == pair && original_pair != pair {
                            result.push(PairWithDepth {
                                pair: original_pair,
                                depth,
                            });
                            None
                        } else {
                            Some((orderbook_pair, original_pair))
                        }
                    })
                    .collect();
                result.push(PairWithDepth { pair, depth });
            }
        }
    }

    let res = json!({ "result": result });
    Response::builder()
        .body(json::to_vec(&res).expect("Serialization failed"))
        .map_err(|e| ERRL!("{}", e))
}

pub fn process_orderbook_depth_p2p_request(
    ctx: MmArc,
    pairs: Vec<(String, String)>,
) -> Result<Option<Vec<u8>>, String> {
    let ordermatch_ctx = OrdermatchContext::from_ctx(&ctx).expect("ordermatch_ctx must exist at this point");
    let orderbook = ordermatch_ctx.orderbook.lock();
    let depth = pairs
        .into_iter()
        .map(|pair| {
            let asks = orderbook.unordered.get(&pair).map_or(0, |orders| orders.len());
            let reversed = (pair.1.clone(), pair.0.clone());
            let bids = orderbook.unordered.get(&reversed).map_or(0, |orders| orders.len());
            (pair, PairDepth { asks, bids })
        })
        .collect();
    let response = OrderbookDepthP2PResponse { depth };
    let encoded = rmp_serde::to_vec(&response).expect("rmp_serde::to_vec should not fail here");
    Ok(Some(encoded))
}
