//! RPC implementations for swaps with liquidity routing (LR) of EVM tokens

use super::one_inch::types::ClassicSwapDetails;
use crate::lp_swap::{
    check_balance_for_taker_swap, check_my_coin_balance_for_swap, check_other_coin_balance_for_swap, CheckBalanceError,
};
use crate::lr_swap::lr_helpers::{check_if_one_inch_supports_pair, get_coin_for_one_inch, sell_buy_method};
use crate::lr_swap::lr_impl::find_best_swap_path_with_lr;
use crate::lr_swap::lr_swap_state_machine::lp_start_agg_taker_swap;
use crate::lr_swap::{AtomicSwapParams, LrSwapParams};
use crate::rpc::lp_commands::one_inch::errors::ApiIntegrationRpcError;
use coins::{
    lp_coinfind_or_err, CoinWithDerivationMethod, FeeApproxStage, MarketCoinOps, MmCoin, MmCoinEnum, TradeFee,
};
use common::log::debug;
use futures::compat::Future01CompatExt;
use lr_api_types::{
    LrExecuteRoutedTradeRequest, LrExecuteRoutedTradeResponse, LrFindBestQuoteRequest, LrFindBestQuoteResponse,
    LrGetQuotesForTokensRequest,
};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::map_mm_error::MapMmError;
use mm2_err_handle::mm_error::MmResult;
use mm2_err_handle::prelude::{MmError, MmResultExt};
use mm2_rpc::data::legacy::TakerAction;
use std::ops::Deref;

pub(crate) mod lr_api_types;

#[cfg(all(test, not(target_arch = "wasm32"), feature = "test-ext-api"))]
mod lr_api_tests;

/// Finds the most cost-effective swap path using liquidity routing (LR) for EVM-compatible tokens (`Aggregated taker swap` path),
/// by selecting the best option from a list of orderbook entries (ask/bid orders).
/// This RPC returns the data needed for actual execution of the swap with LR .
///
/// # Overview
/// This RPC helps users execute token swaps even if they do not directly hold the tokens required
/// by the maker orders. It uses external liquidity routing (e.g., via 1inch provider) to perform necessary conversions, currently for EVM networks
///
/// A swap path may consist of:
///     - A liquidity routing (LR) step before or after the atomic swap.
///     - An atomic swap step to fill the selected maker order (ask or bid).
///
/// Use Case
/// The user wants to buy a specific amount of a token `user_base`, but only holds a different token `user_rel`.
/// This RPC evaluates possible swap paths by combining:
///     - Converting `user_rel` (`user_base`) to the token required by a maker order via LR.
///     - Filling the order through an atomic swap.
///     - Converting the token required by a maker order to `user_base` (`user_rel`) via LR.
/// It then selects and returns the most price-effective path, taking into account:
///     - prices of orders (provided in the params)
///     - 1inch LR quotes
///     - (TODO) Total swap and routing fees
/// Sell requests are processed in a similar way.
///
/// Example
/// A user wants to buy 1 BTC with their USDT, but the best available order sells 1 BTC for DAI.
/// This RPC calculates the total cost of liquidity routing the user's USDT into DAI and then using the
/// acquired DAI to take the BTC order. It compares this path against other potential candidates
/// (e.g., a order selling BTC for USDC and routing the user's USDT into USDC via LR) to find the cheapest option.
///
/// Inputs
/// - A list of maker ask or bid orders (orderbook entries)
/// - Trade method (`buy` or `sell`)
/// - Target or source amount to buy/sell
/// - User’s tokens `user_rel` and `user_base` to be used for the swap
///
/// Outputs
/// - The best swap path including any required LR steps
///
/// Current Limitations
/// - Only supports filling ask orders with:
///   - `user_rel` (sell request)
///   - Liquidity routing before the atomic swap: `user_rel` -> maker `rel`
/// - Does not yet support:
///   - User's buy request
///   - Filling bid orders
///   - Liquidity routing after the atomic swap
///
/// TODO:
/// - Return full trade fee breakdown (e.g., DEX fees, LR fees)
/// - Support the following additional aggregated swap configurations:
///   - Filling ask orders with LR after the atomic swap
///   - Filling bid orders with LR before and after the atomic swap
/// - Support user's buy request
///
/// Notes:
/// - This function relies on external quote APIs (currently 1inch) and may incur latency.
/// - Use this RPC when a direct atomic swap is not available or optimal, and pre/post-routing is needed.
pub async fn lr_find_best_quote_rpc(
    ctx: MmArc,
    req: LrFindBestQuoteRequest,
) -> MmResult<LrFindBestQuoteResponse, ApiIntegrationRpcError> {
    // TODO: add validation:
    // order.base_min_volume << req.amount <= order.base_max_volume
    // order.coin is supported in 1inch
    // order.price not zero
    // when best order is selected validate against req.rel_max_volume and req.rel_min_volume
    // coins in orders should be unique

    let (user_rel_coin, _) = get_coin_for_one_inch(&ctx, &req.user_rel).await.map_mm_err()?;
    let user_rel_chain = user_rel_coin
        .chain_id()
        .ok_or(ApiIntegrationRpcError::ChainNotSupported)?;
    let (swap_data, best_order, total_price) =
        find_best_swap_path_with_lr(&ctx, req.user_base, req.user_rel, req.asks, req.bids, &req.volume).await?;
    let lr_swap_details = ClassicSwapDetails::from_api_classic_swap_data(&ctx, user_rel_chain, swap_data)
        .await
        .mm_err(|err| ApiIntegrationRpcError::ApiDataError(err.to_string()))?;
    Ok(LrFindBestQuoteResponse {
        lr_swap_details,
        best_order,
        total_price,
        // TODO: implement later
        // trade_fee: ...
    })
}

/// Find possible swaps with liquidity routing of several user tokens to fill one order.
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
/// Again, it's a TODO.
pub async fn lr_get_quotes_for_tokens_rpc(
    _ctx: MmArc,
    _req: LrGetQuotesForTokensRequest,
) -> MmResult<LrFindBestQuoteResponse, ApiIntegrationRpcError> {
    // TODO: impl later
    todo!()
}

/// Run an aggregated swap with LR to fill a maker order.
/// The `req` parameter is constructed from the result of a "find_best_quote" RPC call
/// It contains parameters for the atomic swap and LR_0 and LR_1 steps
/// LR_0 and LR_1 are liquidity routing steps before and after the atomic swap (see the description for lr_find_best_quote_rpc fn)
pub async fn lr_execute_routed_trade_rpc(
    ctx: MmArc,
    req: LrExecuteRoutedTradeRequest,
) -> MmResult<LrExecuteRoutedTradeResponse, ApiIntegrationRpcError> {
    debug_print_routed_trade_req(&req);

    if req.lr_swap_0.is_none() && req.lr_swap_1.is_none() {
        return MmError::err(ApiIntegrationRpcError::InvalidParam("no LR params".to_owned()));
    }
    let action = sell_buy_method(&req.atomic_swap.method).map_mm_err()?;
    if action != TakerAction::Sell {
        return MmError::err(ApiIntegrationRpcError::InvalidParam("action must be sell".to_owned()));
    }

    let atomic_swap_params = AtomicSwapParams {
        action,
        base: req.atomic_swap.base.clone(),
        rel: req.atomic_swap.rel.clone(),
        price: req.atomic_swap.price,
        match_by: req.atomic_swap.match_by.unwrap_or_default(),
        order_type: req.atomic_swap.order_type.unwrap_or_default(),
        base_volume: req.atomic_swap.volume,
    };

    // Validate LR step 0 (before the atomic swap):
    let lr_swap_params_0 = if let Some(ref lr_swap_0) = req.lr_swap_0 {
        let (src_coin, src_contract) = get_coin_for_one_inch(&ctx, &lr_swap_0.get_source_token()?)
            .await
            .map_mm_err()?;
        let (dst_coin, dst_contract) = get_coin_for_one_inch(&ctx, &lr_swap_0.get_destination_token()?)
            .await
            .map_mm_err()?;
        let src_chain_id = src_coin.chain_id().ok_or(ApiIntegrationRpcError::ChainNotSupported)?;
        let dst_chain_id = dst_coin.chain_id().ok_or(ApiIntegrationRpcError::ChainNotSupported)?;
        check_if_one_inch_supports_pair(src_chain_id, dst_chain_id).map_mm_err()?;

        // Ensure correct token routing from LR-0 to atomic swap
        if lr_swap_0.get_destination_token()? != atomic_swap_params.taker_coin() {
            return MmError::err(ApiIntegrationRpcError::InvalidParam(format!(
                "Connecting tokens must be same: LR_0 dst_token: {}, taker_coin: {}",
                lr_swap_0.get_destination_token()?,
                atomic_swap_params.taker_coin()
            )));
        }
        let single_address = src_coin.derivation_method().single_addr_or_err().await.map_mm_err()?;
        debug!(
            "Calling check source coin '{}' balance for LR step 0...",
            src_coin.ticker()
        );
        // Check LR swap step source token balance:
        check_my_coin_balance_for_swap(
            &ctx,
            &src_coin,
            None,
            lr_swap_0.swap_details.src_amount.clone(),
            TradeFee {
                coin: src_coin.platform_ticker().to_owned(),
                amount: Default::default(),   // TODO: fix (add to the LR quote)
                paid_from_trading_vol: false, // false as LR is done for EVM tokens
            },
            None,
        )
        .await
        .map_mm_err()?;
        Some(LrSwapParams {
            src_amount: lr_swap_0.swap_details.src_amount.clone(),
            src_decimals: src_coin.decimals(),
            dst_decimals: dst_coin.decimals(),
            src: lr_swap_0.get_source_token()?,
            src_contract,
            dst: lr_swap_0.get_destination_token()?,
            dst_contract,
            from: single_address,
            slippage: lr_swap_0.slippage,
            // If no gas limit provided, we may fail estimations in LR_0 rollback (and if it's the platform coin to rollback).
            // I guess this is not a reason to fail
            gas: lr_swap_0.swap_details.gas.unwrap_or_default(),
        })
    } else {
        None
    };

    let atomic_swap_base_coin = lp_coinfind_or_err(&ctx, &req.atomic_swap.base).await.map_mm_err()?;
    let atomic_swap_rel_coin = lp_coinfind_or_err(&ctx, &req.atomic_swap.rel).await.map_mm_err()?;
    let (taker_coin, maker_coin) = match atomic_swap_params.action {
        TakerAction::Sell => (atomic_swap_base_coin, atomic_swap_rel_coin),
        TakerAction::Buy => (atomic_swap_rel_coin, atomic_swap_base_coin),
    };
    if req.lr_swap_0.is_none() {
        // Check token balance for the atomic swap, if it is the first step:
        debug!(
            "Calling check taker coin '{}' balance for atomic swap...",
            taker_coin.deref().ticker()
        );
        // TODO: no need as start_lp_auto_buy in the state machine does this
        check_balance_for_taker_swap(
            &ctx,
            taker_coin.deref(),
            maker_coin.deref(),
            atomic_swap_params.taker_volume()?,
            None,
            None,
            FeeApproxStage::OrderIssue,
        )
        .await
        .map_mm_err()?
    }

    // Validate needed trade fee in maker platform coin, total for atomic swap and LR_1
    // For the atomic swap Taker may need some amount to spend the Maker payment:
    let mut maker_coin_trade_fee = maker_coin
        .get_receiver_trade_fee(FeeApproxStage::OrderIssue)
        .compat()
        .await
        .mm_err(|e| CheckBalanceError::from_trade_preimage_error(e, maker_coin.ticker()))
        .map_mm_err()?;

    // Add tx fee for LR_1:
    if let (Some(lr_swap_1), MmCoinEnum::EthCoinVariant(eth_coin)) = (&req.lr_swap_1, &maker_coin) {
        let lr_trade_fee = eth_coin
            .estimate_trade_fee(
                lr_swap_1.swap_details.gas.unwrap_or_default().into(),
                FeeApproxStage::OrderIssue,
            )
            .await
            .map_mm_err()?;
        maker_coin_trade_fee.amount += lr_trade_fee.amount;
        maker_coin_trade_fee.paid_from_trading_vol = false;
    }
    debug!(
        "Check other_coin '{}' balance for atomic swap, paid_from_trading_vol={}",
        maker_coin.ticker(),
        maker_coin_trade_fee.paid_from_trading_vol
    );
    if !maker_coin_trade_fee.paid_from_trading_vol {
        debug!(
            "Calling check other_coin '{}' balance for atomic swap...",
            maker_coin.ticker()
        );
        check_other_coin_balance_for_swap(&ctx, maker_coin.deref(), None, maker_coin_trade_fee)
            .await
            .map_mm_err()?;
    }

    // Validate LR step 1 (after the atomic swap):
    let lr_swap_params_1 = if let Some(lr_swap_1) = req.lr_swap_1 {
        let (src_coin, src_contract) = get_coin_for_one_inch(&ctx, &lr_swap_1.get_source_token()?)
            .await
            .map_mm_err()?;
        let (dst_coin, dst_contract) = get_coin_for_one_inch(&ctx, &lr_swap_1.get_destination_token()?)
            .await
            .map_mm_err()?;
        let src_chain_id = src_coin.chain_id().ok_or(ApiIntegrationRpcError::ChainNotSupported)?;
        let dst_chain_id = dst_coin.chain_id().ok_or(ApiIntegrationRpcError::ChainNotSupported)?;
        check_if_one_inch_supports_pair(src_chain_id, dst_chain_id).map_mm_err()?;

        // Ensure correct token routing from atomic swap to LR_1
        if atomic_swap_params.maker_coin() != lr_swap_1.get_source_token()? {
            return MmError::err(ApiIntegrationRpcError::InvalidParam(format!(
                "Connecting tokens must be same: maker coin: {}, LR_1 src_token: {}",
                atomic_swap_params.maker_coin(),
                lr_swap_1.get_source_token()?
            )));
        }
        let single_address = src_coin.derivation_method().single_addr_or_err().await.map_mm_err()?;
        Some(LrSwapParams {
            src_amount: lr_swap_1.swap_details.src_amount.clone(),
            src_decimals: src_coin.decimals(),
            dst_decimals: dst_coin.decimals(),
            src: lr_swap_1.get_source_token()?,
            src_contract,
            dst: lr_swap_1.get_destination_token()?,
            dst_contract,
            from: single_address,
            slippage: lr_swap_1.slippage,
            gas: lr_swap_1.swap_details.gas.unwrap_or_default(), // this field not used yet in LR_1 step
        })
    } else {
        None
    };
    let swap_uuid = lp_start_agg_taker_swap(ctx, lr_swap_params_0, lr_swap_params_1, atomic_swap_params)
        .await
        .map_mm_err()?;
    Ok(LrExecuteRoutedTradeResponse { uuid: swap_uuid })
    // TODO: add different reasons for AbortReason instead SomeReason
    // TODO: ExtApiError - reduce included big OneInchApi err
    // TODO: replace LrSwapError::InternalError with concrete errors
    // TODO: add LR_1 txfee check - added
    // TODO: return 'allowance' rpc result in MmNumber coin units
    // TODO: return 'allowance not enough' error in MmNumber coin units
    // TODO: add set swap tx policy to enable_coin rpc - done
}

fn debug_print_routed_trade_req(req: &LrExecuteRoutedTradeRequest) {
    let (lr_0_src, lr_0_dst, lr_0_src_amount) = req
        .lr_swap_0
        .as_ref()
        .map(|lr| {
            (
                lr.swap_details.src_token.as_ref().and_then(|t| t.symbol_kdf.as_ref()),
                lr.swap_details.dst_token.as_ref().and_then(|t| t.symbol_kdf.as_ref()),
                Some(&lr.swap_details.src_amount),
            )
        })
        .unwrap_or_default();
    let (base, rel, method, volume) = (
        &req.atomic_swap.base,
        &req.atomic_swap.rel,
        &req.atomic_swap.method,
        &req.atomic_swap.volume,
    );
    let (lr_1_src, lr_1_dst, lr_1_src_amount) = req
        .lr_swap_1
        .as_ref()
        .map(|lr| {
            (
                lr.swap_details.src_token.as_ref().and_then(|t| t.symbol_kdf.as_ref()),
                lr.swap_details.dst_token.as_ref().and_then(|t| t.symbol_kdf.as_ref()),
                Some(&lr.swap_details.src_amount),
            )
        })
        .unwrap_or_default();
    debug!("RPC execute_routed_trade entered with lr_0: [src: {:?} dst: {:?} src_amount: {:?}] atomic swap: [base: {:?} rel: {:?} method: {:?} volume: {:?}] lr_1 [src: {:?} dst: {:?} src_amount: {:?}] ", 
        lr_0_src, lr_0_dst, lr_0_src_amount.map(|v| v.to_decimal()),
        base, rel, method, volume.as_ref().map(|v|v.to_decimal()),
        lr_1_src, lr_1_dst, lr_1_src_amount.map(|v| v.to_decimal()),
    );
}
