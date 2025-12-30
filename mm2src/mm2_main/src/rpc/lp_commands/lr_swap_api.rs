//! RPC implementations for swaps with liquidity routing (LR) of EVM tokens

use super::ext_api::ext_api_errors::ExtApiRpcError;
use super::ext_api::ext_api_types::ClassicSwapDetails;
use crate::lr_swap::lr_helpers::sell_buy_method;
use crate::lr_swap::lr_quote::find_best_swap_path_with_lr;
use lr_api_types::{
    AtomicSwapRpcParams, LrExecuteRoutedTradeRequest, LrExecuteRoutedTradeResponse, LrFindBestQuoteRequest,
    LrFindBestQuoteResponse, LrGetQuotesForTokensRequest, LrGetQuotesForTokensResponse,
};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::MmResultExt;
use mm2_err_handle::{map_mm_error::MapMmError, mm_error::MmResult};

#[cfg(all(test, not(target_arch = "wasm32"), feature = "test-ext-api"))]
mod lr_api_tests;
pub(crate) mod lr_api_types;

/// The "find_best_quote" RPC implementation to find the best swap with liquidity routing (LR), also known as 'aggregated taker swap with LR'.
/// For the provided list of orderbook entries this RPC will find out the most price-effective swap which may include LR steps.
/// There are LR_0 and LR_1 steps supported meaning liquidity routing before or after the atomic swap.
/// Liquidity routing is supported in the EVM chains. This is actually a token swap performed by an LR provider (currently 1inch).
/// Both tokens must be in the same EVM network.
///
/// Use cases:
/// User is interested in buying some coin. There are orders available with the desired coin but User does not have tokens to fill those orders.
/// User has some amount of a user_rel token and would like to use it for buying the desired coin.
/// User calls this RPC to find the best swap with LR: with user_rel token converted into the atomic swap taker coin with LR_0 step.
/// User may also request to convert received maker coin into user_base token via LR_1 swap.
///
/// Similar logic is for the case when User would like to sell a token and use LR step to convert it into an atomic swap taker coin
/// and/or convert a maker coin into the User desired token.
///
/// User calls this RPC with the following params:
/// * user_base and user_rel tickers,
/// * amount to buy or sell,
/// * list of ask and bid orders list.
///
/// If user_base and/or user_rel differs from the coins in the orders the RPC creates LR_0 and/or LR_1 steps and gets LR provider quotes
/// to obtain prices for LR_0 and LR_1 steps. The RPC alsouses order prices to calculate the most price-effective swap path.
/// As the result the RPC returns params for LR_0 LR_1 and atomic swap corresponding the most price effective aggregated swap.
/// If best swap cannot be found a error is returned.
/// TODO: we should also return total fees.
///
/// More info:
/// The GUI should provide this RPC with a list of ask or bid orders to select best swap path from.
/// Currently for that the "best_orders" RPC can be used:
/// The GUI should pick tokens which resides in the same chains with user_base and/or user_rel and query "best_orders" RPC (maybe multiple times).
/// The "best_orders" results are asks or bids which can be passed into this RPC.
///
/// TODO: develop a more convenient RPC to find ask and bid orders for finding the best swap with LR.
/// It should support getting info about most liquid white-listed tokens from the LR provider, to do search for best swap more efficiently.
///
pub async fn lr_find_best_quote_rpc(
    ctx: MmArc,
    req: LrFindBestQuoteRequest,
) -> MmResult<LrFindBestQuoteResponse, ExtApiRpcError> {
    // TODO: add validation:
    // order.base_min_volume << req.amount <= order.base_max_volume - DONE
    // when best order is selected validate against req.rel_max_volume and req.rel_min_volume (?)
    // order coins are supported in 1inch
    // order.price not zero
    // coins in orders should be unique (?)
    // Check user available balance for user_base/user_rel when selecting best swap (?)

    let action = sell_buy_method(&req.method).map_mm_err()?;

    let (lr_data_0, best_order, atomic_swap_volume, lr_data_1, total_price) = find_best_swap_path_with_lr(
        &ctx,
        req.user_base,
        req.user_rel,
        &action,
        req.asks,
        req.bids,
        &req.volume,
    )
    .await
    .map_mm_err()?;

    let lr_data_0 = lr_data_0
        .map(|lr_data| {
            ClassicSwapDetails::from_api_classic_swap_data(
                &ctx,
                lr_data.chain_id,
                lr_data.src_amount,
                lr_data.api_details,
            )
            .mm_err(|err| ExtApiRpcError::OneInchDataError(err.to_string()))
        })
        .transpose()?;
    let lr_data_1 = lr_data_1
        .map(|lr_data| {
            ClassicSwapDetails::from_api_classic_swap_data(
                &ctx,
                lr_data.chain_id,
                lr_data.src_amount,
                lr_data.api_details,
            )
            .mm_err(|err| ExtApiRpcError::OneInchDataError(err.to_string()))
        })
        .transpose()?;
    Ok(LrFindBestQuoteResponse {
        lr_data_0,
        lr_data_1,
        atomic_swap: AtomicSwapRpcParams {
            volume: atomic_swap_volume,
            base: best_order.taker_ticker(),
            rel: best_order.maker_ticker(),
            price: best_order.buy_price(),
            method: "sell".to_owned(), // Always convert to the 'sell' action to simplify LR_0 estimations
            order_uuid: best_order.order().uuid,
            match_by: None,
            order_type: None,
        },
        total_price,
        // TODO: implement later
        // trade_fee: ...
    })
}

/// TODO: Find possible swaps with liquidity routing of several user tokens to fill one order.
/// For the provided single order the RPC searches for the most price-effective swap path with LR for user tokens.
///
/// More info:
/// User is interested in buying some coin. There is an order available the User would like to fill but the User does not have tokens from the order.
/// User calls this RPC with the order, desired coin name, amount to buy or sell and list of User tokens to convert to/from with LR.
/// The RPC calls several 1inch classic swap quotes (to find most efficient token conversions)
/// and return possible LR paths to fill the order, with total swap prices.
/// TODO: should also returns total fees.
///
/// NOTE: this RPC does not select the best quote between User tokens because it finds routes for different tokens (with own value),
/// so returns all of them.
/// That is, it's up to the User to select the most cost effective swap, for e.g. comparing token fiat value.
/// In fact, this could be done even in this RPC as 1inch also can get value in fiat but maybe User evaludation is more prefferable.
pub async fn lr_get_quotes_for_tokens_rpc(
    _ctx: MmArc,
    _req: LrGetQuotesForTokensRequest,
) -> MmResult<LrGetQuotesForTokensResponse, ExtApiRpcError> {
    // TODO: impl later
    todo!()
}

/// Run an aggregated swap with LR to fill a maker order.
/// The `req` parameter is constructed from the result of a "find_best_quote" RPC call
/// It contains parameters for the atomic swap and LR_0 and LR_1 steps
/// LR_0 and LR_1 are liquidity routing steps before and after the atomic swap (see the description for lr_find_best_quote_rpc fn)
pub async fn lr_execute_routed_trade_rpc(
    _ctx: MmArc,
    _req: LrExecuteRoutedTradeRequest,
) -> MmResult<LrExecuteRoutedTradeResponse, ExtApiRpcError> {
    todo!()
}
