//! RPC implementation for use API of external trading service providers (1inch etc).

use crate::lr_swap::lr_helpers::{check_if_one_inch_supports_pair, get_coin_for_one_inch};
use coins::eth::u256_from_big_decimal;
use coins::{CoinWithDerivationMethod, MmCoin};
use ext_api_errors::ExtApiRpcError;
use ext_api_helpers::{make_classic_swap_create_params, make_classic_swap_quote_params};
use ext_api_types::{
    AggregationContractRequest, ClassicSwapCreateRequest, ClassicSwapDetails, ClassicSwapLiquiditySourcesRequest,
    ClassicSwapLiquiditySourcesResponse, ClassicSwapQuoteRequest, ClassicSwapResponse, ClassicSwapTokensRequest,
    ClassicSwapTokensResponse,
};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use trading_api::one_inch_api::classic_swap_types::{ProtocolsResponse, TokensResponse};
use trading_api::one_inch_api::client::{ApiClient, SwapApiMethods, SwapUrlBuilder};

pub(crate) mod ext_api_errors;
pub(crate) mod ext_api_helpers;
#[cfg(test)]
mod ext_api_tests;
pub(crate) mod ext_api_types;

/// "1inch_v6_0_classic_swap_contract" rpc impl
/// used to get contract address (for e.g. to approve funds)
pub async fn one_inch_v6_0_classic_swap_contract_rpc(
    _ctx: MmArc,
    _req: AggregationContractRequest,
) -> MmResult<String, ExtApiRpcError> {
    Ok(ApiClient::classic_swap_contract().to_owned())
}

/// "1inch_classic_swap_quote" rpc impl
pub async fn one_inch_v6_0_classic_swap_quote_rpc(
    ctx: MmArc,
    req: ClassicSwapQuoteRequest,
) -> MmResult<ClassicSwapResponse, ExtApiRpcError> {
    let (base, base_contract) = get_coin_for_one_inch(&ctx, &req.base).await.map_mm_err()?;
    let (rel, rel_contract) = get_coin_for_one_inch(&ctx, &req.rel).await.map_mm_err()?;
    let base_chain_id = base.chain_id().ok_or(ExtApiRpcError::ChainNotSupported)?;
    let rel_chain_id = rel.chain_id().ok_or(ExtApiRpcError::ChainNotSupported)?;
    check_if_one_inch_supports_pair(base_chain_id, rel_chain_id).map_mm_err()?;
    let sell_amount = u256_from_big_decimal(&req.amount.to_decimal(), base.decimals())
        .mm_err(|err| ExtApiRpcError::InvalidParam(err.to_string()))?;
    let query_params = make_classic_swap_quote_params(base_contract, rel_contract, sell_amount, req.opt_params)
        .build_query_params()
        .map_mm_err()?;
    let url = SwapUrlBuilder::create_api_url_builder(&ctx, base_chain_id, SwapApiMethods::ClassicSwapQuote)
        .map_mm_err()?
        .with_query_params(query_params)
        .build()
        .map_mm_err()?;
    let quote = ApiClient::call_api(url).await.map_mm_err()?;
    ClassicSwapResponse::from_api_classic_swap_data(&ctx, base_chain_id, sell_amount, quote) // use 'base' as amount in errors is in the src coin
        .mm_err(|err| ExtApiRpcError::OneInchDataError(err.to_string()))
}

/// "1inch_classic_swap_create" rpc implementation
/// This rpc actually returns a transaction to call the 1inch swap aggregation contract. GUI should sign it and send to the chain.
/// We don't verify the transaction in any way and trust the 1inch api.
pub async fn one_inch_v6_0_classic_swap_create_rpc(
    ctx: MmArc,
    req: ClassicSwapCreateRequest,
) -> MmResult<ClassicSwapResponse, ExtApiRpcError> {
    let (base, base_contract) = get_coin_for_one_inch(&ctx, &req.base).await.map_mm_err()?;
    let (rel, rel_contract) = get_coin_for_one_inch(&ctx, &req.rel).await.map_mm_err()?;
    let base_chain_id = base.chain_id().ok_or(ExtApiRpcError::ChainNotSupported)?;
    let rel_chain_id = rel.chain_id().ok_or(ExtApiRpcError::ChainNotSupported)?;
    check_if_one_inch_supports_pair(base_chain_id, rel_chain_id).map_mm_err()?;
    let sell_amount = u256_from_big_decimal(&req.amount.to_decimal(), base.decimals())
        .mm_err(|err| ExtApiRpcError::InvalidParam(err.to_string()))?;
    let single_address = base.derivation_method().single_addr_or_err().await.map_mm_err()?;

    let query_params = make_classic_swap_create_params(
        base_contract,
        rel_contract,
        sell_amount,
        single_address,
        req.slippage,
        req.opt_params,
    )
    .build_query_params()
    .map_mm_err()?;
    let url = SwapUrlBuilder::create_api_url_builder(&ctx, base_chain_id, SwapApiMethods::ClassicSwapCreate)
        .map_mm_err()?
        .with_query_params(query_params)
        .build()
        .map_mm_err()?;
    let swap_with_tx = ApiClient::call_api(url).await.map_mm_err()?;
    ClassicSwapResponse::from_api_classic_swap_data(&ctx, base_chain_id, sell_amount, swap_with_tx)
        .mm_err(|err| ExtApiRpcError::OneInchDataError(err.to_string()))
}

/// "1inch_v6_0_classic_swap_liquidity_sources" rpc implementation.
/// Returns list of DEX available for routing with the 1inch Aggregation contract
pub async fn one_inch_v6_0_classic_swap_liquidity_sources_rpc(
    ctx: MmArc,
    req: ClassicSwapLiquiditySourcesRequest,
) -> MmResult<ClassicSwapLiquiditySourcesResponse, ExtApiRpcError> {
    let url = SwapUrlBuilder::create_api_url_builder(&ctx, req.chain_id, SwapApiMethods::LiquiditySources)
        .map_mm_err()?
        .build()
        .map_mm_err()?;
    let response: ProtocolsResponse = ApiClient::call_api(url).await.map_mm_err()?;
    Ok(ClassicSwapLiquiditySourcesResponse {
        protocols: response.protocols,
    })
}

/// "1inch_classic_swap_tokens" rpc implementation.
/// Returns list of tokens available for 1inch classic swaps
pub async fn one_inch_v6_0_classic_swap_tokens_rpc(
    ctx: MmArc,
    req: ClassicSwapTokensRequest,
) -> MmResult<ClassicSwapTokensResponse, ExtApiRpcError> {
    let url = SwapUrlBuilder::create_api_url_builder(&ctx, req.chain_id, SwapApiMethods::Tokens)
        .map_mm_err()?
        .build()
        .map_mm_err()?;
    let mut response: TokensResponse = ApiClient::call_api(url).await.map_mm_err()?;
    for (_, token_info) in response.tokens.iter_mut() {
        token_info.symbol_kdf = ClassicSwapDetails::token_name_kdf(&ctx, req.chain_id, token_info);
    }
    Ok(ClassicSwapTokensResponse {
        tokens: response.tokens,
    })
}
