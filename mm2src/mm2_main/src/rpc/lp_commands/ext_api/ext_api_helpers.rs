use super::ext_api_types::{ClassicSwapCreateOptParams, ClassicSwapQuoteOptParams};
use coins::hd_wallet::DisplayAddress;
use coins::Ticker;
use ethereum_types::{Address as EthAddress, U256};
use mm2_number::MmNumber;
use mm2_rpc::data::legacy::{MatchBy, OrderType, SellBuyRequest, TakerAction};
use trading_api::one_inch_api::classic_swap_types::{ClassicSwapCreateCallBuilder, ClassicSwapQuoteCallBuilder};

pub(crate) fn make_classic_swap_quote_params(
    base_contract: EthAddress,
    rel_contract: EthAddress,
    sell_amount: U256,
    opt_params: ClassicSwapQuoteOptParams,
) -> ClassicSwapQuoteCallBuilder {
    ClassicSwapQuoteCallBuilder::new(
        base_contract.display_address(),
        rel_contract.display_address(),
        sell_amount.to_string(),
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

#[allow(unused)]
pub(crate) fn make_atomic_swap_request(
    base: Ticker,
    rel: Ticker,
    price: MmNumber,
    volume: MmNumber,
    action: TakerAction,
    match_by: MatchBy,
    order_type: OrderType,
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
        rel_confs: None,
        rel_nota: None,
        min_volume: None,
        save_in_history: true,
    }
}
