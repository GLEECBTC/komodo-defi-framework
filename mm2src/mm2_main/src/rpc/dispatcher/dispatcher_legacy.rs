use super::PUBLIC_METHODS;
use common::HyRes;
use derive_more::Display;
use futures::compat::Future01CompatExt;
use futures::{Future as Future03, FutureExt, TryFutureExt};
use http::Response;
use mm2_core::mm_ctx::MmArc;
use serde_json::{self as json, Value as Json};
use std::net::SocketAddr;

use super::lp_commands::legacy::*;
use crate::lp_ordermatch::{
    best_orders_rpc, buy, cancel_all_orders_rpc, cancel_order_rpc, my_orders, order_status, orderbook_depth_rpc,
    orderbook_rpc, orders_history_by_filter, sell, set_price, update_maker_order_rpc,
};
use crate::lp_swap::{
    active_swaps_rpc, all_swaps_uuids_by_filter, ban_pubkey_rpc, coins_needed_for_kick_start, import_swaps,
    list_banned_pubkeys_rpc, max_taker_vol, my_recent_swaps_rpc, my_swap_status, recover_funds_of_swap,
    stats_swap_status, unban_pubkeys_rpc,
};
use crate::rpc::rate_limiter::{process_rate_limit, RateLimitContext};
use coins::{
    convert_address, convert_utxo_address, get_enabled_coins, get_trade_fee, kmd_rewards_info, my_tx_history,
    send_raw_transaction, set_required_confirmations, set_requires_notarization, show_priv_key, validate_address,
};

/// Result of `fn dispatcher`.
pub enum DispatcherRes {
    /// `fn dispatcher` has found a Rust handler for the RPC "method".
    Match(HyRes),
    /// No handler found by `fn dispatcher`. Returning the `Json` request in order for it to be handled elsewhere.
    NoMatch,
}

async fn auth(json: &Json, ctx: &MmArc, client: &SocketAddr) -> Result<(), String> {
    if !PUBLIC_METHODS.contains(&json["method"].as_str()) {
        if !json["userpass"].is_string() {
            return Err("Userpass is not set!".to_string());
        }

        if json["userpass"] != ctx.conf["rpc_password"] {
            return Err(format!("{}", process_rate_limit(ctx, client).await));
        }
    }
    Ok(())
}

/// Using async/await (futures 0.3) in `dispatcher`
/// will pave the way for porting the remaining system threading code to async/await green threads.
fn hyres(handler: impl Future03<Output = Result<Response<Vec<u8>>, String>> + Send + 'static) -> HyRes {
    Box::new(handler.boxed().compat())
}

/// The dispatcher, with full control over the HTTP result and the way we run the `Future` producing it.
///
/// Invoked both directly from the HTTP endpoint handler below and in a delayed fashion from `lp_command_q_loop`.
///
/// Returns `None` if the requested "method" wasn't found among the ported RPC methods and has to be handled elsewhere.
pub fn dispatcher(req: Json, ctx: MmArc) -> DispatcherRes {
    let method = match req["method"].clone() {
        Json::String(method) => method,
        _ => return DispatcherRes::NoMatch,
    };
    DispatcherRes::Match(match &method[..] {
        // Sorted alphanumerically (on the first latter) for readability.
        // "autoprice" => lp_autoprice (ctx, req),
        "active_swaps" => hyres(active_swaps_rpc(ctx, req)),
        "all_swaps_uuids_by_filter" => hyres(all_swaps_uuids_by_filter(ctx, req)),
        "ban_pubkey" => hyres(ban_pubkey_rpc(ctx, req)),
        "best_orders" => hyres(best_orders_rpc(ctx, req)),
        "buy" => hyres(buy(ctx, req)),
        "cancel_all_orders" => hyres(cancel_all_orders_rpc(ctx, req)),
        "cancel_order" => hyres(cancel_order_rpc(ctx, req)),
        "coins_needed_for_kick_start" => hyres(coins_needed_for_kick_start(ctx)),
        "convertaddress" => hyres(convert_address(ctx, req)),
        "convert_utxo_address" => hyres(convert_utxo_address(ctx, req)),
        "disable_coin" => hyres(disable_coin(ctx, req)),
        "electrum" => hyres(electrum(ctx, req)),
        "enable" => hyres(enable(ctx, req)),
        "get_enabled_coins" => hyres(get_enabled_coins(ctx)),
        "get_directly_connected_peers" => hyres(get_directly_connected_peers(ctx)),
        "get_gossip_mesh" => hyres(get_gossip_mesh(ctx)),
        "get_gossip_peer_topics" => hyres(get_gossip_peer_topics(ctx)),
        "get_gossip_topic_peers" => hyres(get_gossip_topic_peers(ctx)),
        "get_my_peer_id" => hyres(get_my_peer_id(ctx)),
        "get_relay_mesh" => hyres(get_relay_mesh(ctx)),
        "get_trade_fee" => hyres(get_trade_fee(ctx, req)),
        // "fundvalue" => lp_fundvalue (ctx, req, false),
        "help" => help(),
        "import_swaps" => hyres(import_swaps(ctx, req)),
        "kmd_rewards_info" => hyres(kmd_rewards_info(ctx)),
        // "inventory" => inventory (ctx, req),
        "list_banned_pubkeys" => hyres(list_banned_pubkeys_rpc(ctx)),
        "max_taker_vol" => hyres(max_taker_vol(ctx, req)),
        "metrics" => metrics(ctx),
        "min_trading_vol" => hyres(min_trading_vol(ctx, req)),
        "my_balance" => hyres(my_balance(ctx, req)),
        "my_orders" => hyres(my_orders(ctx)),
        "my_recent_swaps" => hyres(my_recent_swaps_rpc(ctx, req)),
        "my_swap_status" => hyres(my_swap_status(ctx, req)),
        "my_tx_history" => hyres(my_tx_history(ctx, req)),
        "orders_history_by_filter" => hyres(orders_history_by_filter(ctx, req)),
        "order_status" => hyres(order_status(ctx, req)),
        "orderbook" => hyres(orderbook_rpc(ctx, req)),
        "orderbook_depth" => hyres(orderbook_depth_rpc(ctx, req)),
        "recover_funds_of_swap" => hyres(recover_funds_of_swap(ctx, req)),
        "sell" => hyres(sell(ctx, req)),
        "show_priv_key" => hyres(show_priv_key(ctx, req)),
        "send_raw_transaction" => hyres(send_raw_transaction(ctx, req)),
        "set_required_confirmations" => hyres(set_required_confirmations(ctx, req)),
        "set_requires_notarization" => hyres(set_requires_notarization(ctx, req)),
        "setprice" => hyres(set_price(ctx, req)),
        "stats_swap_status" => hyres(stats_swap_status(ctx, req)),
        "stop" => hyres(stop(ctx)),
        "trade_preimage" => hyres(into_legacy::trade_preimage(ctx, req)),
        "unban_pubkeys" => hyres(unban_pubkeys_rpc(ctx, req)),
        "update_maker_order" => hyres(update_maker_order_rpc(ctx, req)),
        "validateaddress" => hyres(validate_address(ctx, req)),
        "version" => version(ctx),
        "withdraw" => hyres(into_legacy::withdraw(ctx, req)),
        _ => return DispatcherRes::NoMatch,
    })
}

#[derive(Debug, Display)]
pub enum LegacyRequestProcessError {
    #[display(fmt = "Selected method is not allowed: {reason}")]
    NotAllowed { reason: String },
    #[display(fmt = "No such method")]
    NoMatch,
    #[display(fmt = "RPC call failed: {reason}")]
    Failed { reason: String },
}

pub async fn process_single_request(
    ctx: MmArc,
    req: Json,
    client: SocketAddr,
    local_only: bool,
) -> Result<Response<Vec<u8>>, LegacyRequestProcessError> {
    // https://github.com/artemii235/SuperNET/issues/368
    if local_only && !client.ip().is_loopback() && !PUBLIC_METHODS.contains(&req["method"].as_str()) {
        return Err(LegacyRequestProcessError::NotAllowed {
            reason: "Selected method can only be called from localhost.".to_owned(),
        });
    }
    let rate_limit_ctx = RateLimitContext::from_ctx(&ctx).unwrap();
    if rate_limit_ctx.is_banned(client.ip()).await {
        return Err(LegacyRequestProcessError::NotAllowed {
            reason: "Your IP is banned.".to_owned(),
        });
    }
    auth(&req, &ctx, &client)
        .await
        .map_err(|reason| LegacyRequestProcessError::Failed { reason })?;

    let handler = match dispatcher(req, ctx.clone()) {
        DispatcherRes::Match(handler) => handler,
        DispatcherRes::NoMatch => {
            return Err(LegacyRequestProcessError::NoMatch);
        },
    };

    handler
        .compat()
        .await
        .map_err(|reason| LegacyRequestProcessError::Failed { reason })
}

/// The set of functions that convert the result of the updated handlers into the legacy format.
mod into_legacy {
    use super::*;
    use crate::lp_swap;

    pub async fn withdraw(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
        let params = try_s!(json::from_value(req));
        let result = try_s!(coins::withdraw(ctx, params).await);
        let body = try_s!(json::to_vec(&result));
        Ok(try_s!(Response::builder().body(body)))
    }

    pub async fn trade_preimage(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
        let params = try_s!(json::from_value(req));
        let result = try_s!(lp_swap::trade_preimage_rpc(ctx, params).await);
        let res = json!({ "result": result });
        let body = try_s!(json::to_vec(&res));
        Ok(try_s!(Response::builder().body(body)))
    }
}
