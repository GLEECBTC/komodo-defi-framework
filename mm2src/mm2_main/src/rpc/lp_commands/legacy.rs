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
//  rpc_commands.rs
//  marketmaker
//

use coins::utxo::rpc_clients::ELECTRUM_REQUEST_TIMEOUT;
use coins::{lp_coinfind, lp_coinfind_any, lp_coininit, CoinsContext, MmCoinEnum};
use common::custom_futures::timeout::FutureTimerExt;
use common::executor::Timer;
use common::{rpc_err_response, rpc_response, HyRes};
use futures::compat::Future01CompatExt;
use http::Response;
use mm2_core::mm_ctx::MmArc;
use mm2_libp2p::p2p_ctx::P2PContext;
use mm2_metrics::MetricsOps;
use mm2_number::construct_detailed;
use mm2_rpc::data::legacy::{BalanceResponse, CoinInitResponse, Mm2RpcResult, MmVersionResponse, Status};
use serde_json::{self as json, Value as Json};
use std::collections::HashSet;
use uuid::Uuid;

use crate::lp_dispatcher::{dispatch_lp_event, StopCtxEvent};
use crate::lp_network::subscribe_to_topic;
use crate::lp_ordermatch::{cancel_orders_by, get_matching_orders, CancelBy};
use crate::lp_swap::{active_swaps_using_coins, tx_helper_topic, watcher_topic};

const INTERNAL_SERVER_ERROR_CODE: u16 = 500;
const RESPONSE_OK_STATUS_CODE: u16 = 200;

pub fn disable_coin_err(
    error: String,
    matching: &[Uuid],
    cancelled: &[Uuid],
    active_swaps: &[Uuid],
) -> Result<Response<Vec<u8>>, String> {
    let err = json!({
        "error": error,
        "orders": {
            "matching": matching,
            "cancelled": cancelled
        },
        "active_swaps": active_swaps
    });
    Response::builder()
        .status(INTERNAL_SERVER_ERROR_CODE)
        .body(json::to_vec(&err).unwrap())
        .map_err(|e| ERRL!("{}", e))
}

/// Attempts to disable the coin
pub async fn disable_coin(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let ticker = try_s!(req["coin"].as_str().ok_or("No 'coin' field")).to_owned();
    let force_disable = req["force_disable"].as_bool().unwrap_or_default();

    let coin = match lp_coinfind_any(&ctx, &ticker).await {
        Ok(Some(t)) if t.is_available() => t,
        Ok(Some(t)) if !t.is_available() && force_disable => t,
        Err(err) => {
            return disable_coin_err(format!("!lp_coinfind({err}): "), &[], &[], &[]);
        },
        _ => {
            return disable_coin_err(format!("No such coin: {ticker}"), &[], &[], &[]);
        },
    };

    // Get all matching orders and active swaps.
    let coins_to_disable: HashSet<_> = std::iter::once(ticker.clone()).collect();
    let active_swaps = try_s!(active_swaps_using_coins(&ctx, &coins_to_disable));
    let still_matching_orders = try_s!(get_matching_orders(&ctx, &coins_to_disable).await);

    let coins_ctx = try_s!(CoinsContext::from_ctx(&ctx));

    // Return an error if:
    // 1. There are matching orders or active swaps.
    // 2. A platform coin is to be disabled and there are tokens dependent on it.
    if !active_swaps.is_empty() || !still_matching_orders.is_empty() {
        return disable_coin_err(
            String::from("There are currently matching orders, active swaps"),
            &still_matching_orders,
            &[],
            &active_swaps,
        );
    }

    let response = |ticker: &str, cancelled_orders: Vec<Uuid>, passivized: bool| {
        let res = json!({
            "result": {
                "coin": ticker,
                "cancelled_orders": cancelled_orders,
                "passivized": passivized,
            }
        });

        Response::builder()
            .body(json::to_vec(&res).unwrap())
            .map_err(|e| ERRL!("{}", e))
    };

    // Proceed with disabling the coin/tokens.
    log!("disabling {ticker} coin");
    let cancelled_and_matching_orders = cancel_orders_by(
        &ctx,
        CancelBy::Coin {
            ticker: ticker.to_string(),
        },
    )
    .await;
    let cancelled_orders = match cancelled_and_matching_orders {
        Ok((cancelled, _)) => cancelled,
        Err(err) => {
            return disable_coin_err(err, &still_matching_orders, &[], &active_swaps);
        },
    };

    if !coins_ctx.get_dependent_tokens(&ticker).await.is_empty() && !force_disable {
        coin.update_is_available(false);
        return response(&ticker, cancelled_orders, true);
    }

    coins_ctx.remove_coin(coin.inner).await;

    response(&ticker, cancelled_orders, false)
}

/// Enable a coin in the Electrum mode.
pub async fn electrum(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let ticker = try_s!(req["coin"].as_str().ok_or("No 'coin' field")).to_owned();
    let coin: MmCoinEnum = try_s!(lp_coininit(&ctx, &ticker, &req).await);
    let balance = match coin.my_balance().compat().timeout_secs(ELECTRUM_REQUEST_TIMEOUT).await {
        Ok(Ok(balance)) => balance,
        // If the coin was activated successfully but the balance query failed (most probably due to faulty
        // electrum servers), remove the coin as the whole request is a failure now from the POV of the GUI.
        err => {
            let coins_ctx = try_s!(CoinsContext::from_ctx(&ctx));
            coins_ctx.remove_coin(coin).await;
            return Err(ERRL!("Deactivated coin due to error in balance querying: {:?}", err));
        },
    };
    let res = CoinInitResponse {
        result: "success".into(),
        address: try_s!(coin.my_address()),
        balance: balance.spendable,
        unspendable_balance: balance.unspendable,
        coin: coin.ticker().into(),
        required_confirmations: coin.required_confirmations(),
        requires_notarization: coin.requires_notarization(),
        mature_confirmations: coin.mature_confirmations(),
    };
    let res = try_s!(json::to_vec(&res));
    Ok(try_s!(Response::builder().body(res)))
}

/// Enable a coin in the local wallet mode.
pub async fn enable(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let ticker = try_s!(req["coin"].as_str().ok_or("No 'coin' field")).to_owned();
    let coin: MmCoinEnum = try_s!(lp_coininit(&ctx, &ticker, &req).await);
    let balance = try_s!(coin.my_balance().compat().await);
    let res = CoinInitResponse {
        result: "success".to_string(),
        address: try_s!(coin.my_address()),
        balance: balance.spendable,
        unspendable_balance: balance.unspendable,
        coin: coin.ticker().to_string(),
        required_confirmations: coin.required_confirmations(),
        requires_notarization: coin.requires_notarization(),
        mature_confirmations: coin.mature_confirmations(),
    };
    let res = try_s!(json::to_vec(&res));
    let res = try_s!(Response::builder().body(res));

    if coin.is_utxo_in_native_mode() {
        subscribe_to_topic(&ctx, tx_helper_topic(coin.ticker()));
    }
    if ctx.is_watcher() {
        subscribe_to_topic(&ctx, watcher_topic(coin.ticker()));
    }

    Ok(res)
}

#[cfg(target_arch = "wasm32")]
pub fn help() -> HyRes {
    rpc_response(
        INTERNAL_SERVER_ERROR_CODE,
        json!({
            "error":"'help' is only supported in native mode"
        })
        .to_string(),
    )
}

#[cfg(not(target_arch = "wasm32"))]
pub fn help() -> HyRes {
    rpc_response(
        RESPONSE_OK_STATUS_CODE,
        "
        buy(base, rel, price, relvolume, timeout=10, duration=3600)
        electrum(coin, urls)
        enable(coin, urls, swap_contract_address)
        myprice(base, rel)
        my_balance(coin)
        my_swap_status(params/uuid)
        orderbook(base, rel, duration=3600)
        sell(base, rel, price, basevolume, timeout=10, duration=3600)
        send_raw_transaction(coin, tx_hex)
        setprice(base, rel, price, broadcast=1)
        stop()
        version
        withdraw(coin, amount, to)
    ",
    )
}

/// Get MarketMaker session metrics
pub fn metrics(ctx: MmArc) -> HyRes {
    match ctx.metrics.collect_json().map(|value| value.to_string()) {
        Ok(response) => rpc_response(RESPONSE_OK_STATUS_CODE, response),
        Err(err) => rpc_err_response(INTERNAL_SERVER_ERROR_CODE, &err.to_string()),
    }
}

/// Get my_balance of a coin
pub async fn my_balance(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let ticker = try_s!(req["coin"].as_str().ok_or("No 'coin' field")).to_owned();
    let coin = match lp_coinfind(&ctx, &ticker).await {
        Ok(Some(t)) => t,
        Ok(None) => return ERR!("No such coin: {}", ticker),
        Err(err) => return ERR!("!lp_coinfind({}): {}", ticker, err),
    };
    let my_balance = try_s!(coin.my_balance().compat().await);

    let res = try_s!(json::to_vec(&BalanceResponse {
        coin: ticker,
        balance: my_balance.spendable,
        unspendable_balance: my_balance.unspendable,
        address: try_s!(coin.my_address())
    }));
    Ok(try_s!(Response::builder().body(res)))
}

pub async fn stop(ctx: MmArc) -> Result<Response<Vec<u8>>, String> {
    dispatch_lp_event(ctx.clone(), StopCtxEvent.into()).await;
    // Should delay the shutdown a bit in order not to trip the "stop" RPC call in unit tests.
    // Stopping immediately leads to the "stop" RPC call failing with the "errno 10054" sometimes.
    let fut = async move {
        Timer::sleep(0.05).await;
        ctx.stop().await.expect("Couldn't stop the KDF runtime.");
    };

    // Please note we shouldn't use `MmCtx::spawner` to spawn this future,
    // as all spawned futures will be dropped on `MmArc::stop`, so this future will be dropped as well,
    // and it may lead to an unexpected behaviour.
    common::executor::spawn(fut);

    let res = try_s!(json::to_vec(&Mm2RpcResult::new(Status::Success)));
    Ok(try_s!(Response::builder().body(res)))
}

pub fn version(ctx: MmArc) -> HyRes {
    match json::to_vec(&MmVersionResponse {
        result: ctx.mm_version.clone(),
        datetime: ctx.datetime.clone(),
    }) {
        Ok(response) => rpc_response(RESPONSE_OK_STATUS_CODE, response),
        Err(err) => rpc_err_response(INTERNAL_SERVER_ERROR_CODE, ERRL!("{}", err).as_str()),
    }
}

pub async fn get_directly_connected_peers(ctx: MmArc) -> Result<Response<Vec<u8>>, String> {
    let ctx = P2PContext::fetch_from_mm_arc(&ctx);
    let cmd_tx = ctx.cmd_tx.lock().clone();
    let result = mm2_libp2p::get_directly_connected_peers(cmd_tx).await;
    let result = json!({
        "result": result,
    });
    let res = try_s!(json::to_vec(&result));
    Ok(try_s!(Response::builder().body(res)))
}

pub async fn get_gossip_mesh(ctx: MmArc) -> Result<Response<Vec<u8>>, String> {
    let ctx = P2PContext::fetch_from_mm_arc(&ctx);
    let cmd_tx = ctx.cmd_tx.lock().clone();
    let result = mm2_libp2p::get_gossip_mesh(cmd_tx).await;
    let result = json!({
        "result": result,
    });
    let res = try_s!(json::to_vec(&result));
    Ok(try_s!(Response::builder().body(res)))
}

pub async fn get_gossip_peer_topics(ctx: MmArc) -> Result<Response<Vec<u8>>, String> {
    let ctx = P2PContext::fetch_from_mm_arc(&ctx);
    let cmd_tx = ctx.cmd_tx.lock().clone();
    let result = mm2_libp2p::get_gossip_peer_topics(cmd_tx).await;
    let result = json!({
        "result": result,
    });
    let res = try_s!(json::to_vec(&result));
    Ok(try_s!(Response::builder().body(res)))
}

pub async fn get_gossip_topic_peers(ctx: MmArc) -> Result<Response<Vec<u8>>, String> {
    let ctx = P2PContext::fetch_from_mm_arc(&ctx);
    let cmd_tx = ctx.cmd_tx.lock().clone();
    let result = mm2_libp2p::get_gossip_topic_peers(cmd_tx).await;
    let result = json!({
        "result": result,
    });
    let res = try_s!(json::to_vec(&result));
    Ok(try_s!(Response::builder().body(res)))
}

pub async fn get_relay_mesh(ctx: MmArc) -> Result<Response<Vec<u8>>, String> {
    let ctx = P2PContext::fetch_from_mm_arc(&ctx);
    let cmd_tx = ctx.cmd_tx.lock().clone();
    let result = mm2_libp2p::get_relay_mesh(cmd_tx).await;
    let result = json!({
        "result": result,
    });
    let res = try_s!(json::to_vec(&result));
    Ok(try_s!(Response::builder().body(res)))
}

pub async fn get_my_peer_id(ctx: MmArc) -> Result<Response<Vec<u8>>, String> {
    let p2p_ctx = P2PContext::fetch_from_mm_arc(&ctx);
    let peer_id = p2p_ctx.peer_id().to_string();

    let result = json!({
        "result": peer_id,
    });
    let res = try_s!(json::to_vec(&result));
    Ok(try_s!(Response::builder().body(res)))
}

construct_detailed!(DetailedMinTradingVol, min_trading_vol);

#[derive(Serialize)]
struct MinTradingVolResponse<'a> {
    coin: &'a str,
    #[serde(flatten)]
    volume: DetailedMinTradingVol,
}

/// Get min_trading_vol of a coin
pub async fn min_trading_vol(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let ticker = try_s!(req["coin"].as_str().ok_or("No 'coin' field")).to_owned();
    let coin = match lp_coinfind(&ctx, &ticker).await {
        Ok(Some(t)) => t,
        Ok(None) => return ERR!("No such coin: {}", ticker),
        Err(err) => return ERR!("!lp_coinfind({}): {}", ticker, err),
    };
    let min_trading_vol = coin.min_trading_vol();
    let response = MinTradingVolResponse {
        coin: &ticker,
        volume: min_trading_vol.into(),
    };
    let res = json!({
        "result": response,
    });
    let res = try_s!(json::to_vec(&res));
    Ok(try_s!(Response::builder().body(res)))
}
