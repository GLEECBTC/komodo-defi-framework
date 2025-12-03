use super::lr_errors::LrSwapError;
use crate::rpc::lp_commands::one_inch::types::ClassicSwapCreateOptParams;
use coins::eth::{EthCoin, EthCoinType};
use coins::hd_wallet::DisplayAddress;
use coins::{lp_coinfind_or_err, MmCoinEnum, Ticker};
use ethereum_types::{Address as EthAddress, U256};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::MmNumber;
use mm2_rpc::data::legacy::{MatchBy, OrderType, SellBuyRequest, TakerAction};
use std::str::FromStr;
use trading_api::one_inch_api::{classic_swap_types::ClassicSwapCreateCallBuilder, client::ApiClient};

pub(crate) async fn get_coin_for_one_inch(
    ctx: &MmArc,
    ticker: &Ticker,
) -> MmResult<(EthCoin, EthAddress), LrSwapError> {
    let coin = match lp_coinfind_or_err(ctx, ticker).await.map_mm_err()? {
        MmCoinEnum::EthCoinVariant(coin) => coin,
        _ => return Err(MmError::new(LrSwapError::CoinTypeError)),
    };
    let contract = match coin.coin_type {
        EthCoinType::Eth => EthAddress::from_str(ApiClient::eth_special_contract())
            .map_to_mm(|_| LrSwapError::InternalError("invalid address".to_owned()))?,
        EthCoinType::Erc20 { token_addr, .. } => token_addr,
        EthCoinType::Nft { .. } => return Err(MmError::new(LrSwapError::NftProtocolNotSupported)),
    };
    Ok((coin, contract))
}

#[allow(clippy::result_large_err)]
pub(crate) fn check_if_one_inch_supports_pair(base_chain_id: u64, rel_chain_id: u64) -> MmResult<(), LrSwapError> {
    if !ApiClient::is_chain_supported(base_chain_id) {
        return MmError::err(LrSwapError::ChainNotSupported);
    }
    if base_chain_id != rel_chain_id {
        return MmError::err(LrSwapError::DifferentChains);
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
pub(crate) fn sell_buy_method(method: &str) -> MmResult<TakerAction, LrSwapError> {
    match method {
        "buy" => Ok(TakerAction::Buy),
        "sell" => Ok(TakerAction::Sell),
        _ => MmError::err(LrSwapError::InvalidParam(
            "invalid method in sell/buy request".to_owned(),
        )),
    }
}

pub(crate) fn make_classic_swap_create_params(
    base_contract: EthAddress,
    rel_contract: EthAddress,
    sell_amount: U256,
    my_address: EthAddress,
    _slippage: f32,
    opt_params: ClassicSwapCreateOptParams,
) -> ClassicSwapCreateCallBuilder {
    ClassicSwapCreateCallBuilder::new(
        base_contract.display_address(),
        rel_contract.display_address(),
        sell_amount.to_string(),
        my_address.display_address(),
        0.0, // TODO: enable slippage
    )
    .with_fee(opt_params.fee)
    .with_protocols(opt_params.protocols)
    .with_gas_price(opt_params.gas_price)
    .with_complexity_level(opt_params.complexity_level)
    .with_parts(opt_params.parts)
    .with_main_route_parts(opt_params.main_route_parts)
    .with_gas_limit(opt_params.gas_limit)
    .with_include_tokens_info(Some(opt_params.include_tokens_info))
    .with_include_protocols(Some(opt_params.include_protocols))
    .with_include_gas(Some(opt_params.include_gas))
    .with_connector_tokens(opt_params.connector_tokens)
    .with_excluded_protocols(opt_params.excluded_protocols)
    .with_permit(opt_params.permit)
    .with_compatibility(opt_params.compatibility)
    .with_receiver(opt_params.receiver)
    .with_referrer(opt_params.referrer)
    .with_disable_estimate(opt_params.disable_estimate)
    .with_allow_partial_fill(opt_params.allow_partial_fill)
    .with_use_permit2(opt_params.use_permit2)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn make_atomic_swap_request(
    base: Ticker,
    rel: Ticker,
    price: MmNumber,
    volume: MmNumber,
    action: TakerAction,
    match_by: MatchBy,
    order_type: OrderType,
    rel_confs: Option<u64>,
) -> SellBuyRequest {
    SellBuyRequest {
        base,
        rel,
        price,
        volume,
        timeout: None,
        duration: None,
        method: match action {
            TakerAction::Buy => "buy".to_string(),
            TakerAction::Sell => "sell".to_string(),
        },
        gui: None,
        dest_pub_key: Default::default(),
        match_by,
        order_type,
        base_confs: None,
        base_nota: None,
        rel_confs,
        rel_nota: None,
        min_volume: None,
        save_in_history: true,
    }
}
