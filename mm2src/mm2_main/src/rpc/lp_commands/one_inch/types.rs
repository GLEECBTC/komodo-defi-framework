use super::errors::FromApiValueError;
use coins::eth::erc20::{get_erc20_ticker_by_contract_address, get_platform_ticker};
use coins::eth::{u256_to_big_decimal, wei_to_eth_decimal, wei_to_gwei_decimal};
use coins::Ticker;
use common::true_f;
use ethereum_types::{Address, U256};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::{construct_detailed, BigDecimal, MmNumber};
use rpc::v1::types::Bytes as BytesJson;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::convert::TryInto;
use std::str::FromStr;
use trading_api::one_inch_api::{
    self,
    classic_swap_types::{ProtocolImage, ProtocolInfo, TokenInfo as LrTokenInfo},
    client::ApiClient,
};

construct_detailed!(DetailedAmount, amount);

#[derive(Clone, Debug, Deserialize)]
pub struct AggregationContractRequest {}

/// Request to get quote for 1inch classic swap.
/// See 1inch docs for more details: https://portal.1inch.dev/documentation/apis/swap/classic-swap/Parameter%20Descriptions/quote_params
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassicSwapQuoteRequest {
    /// Base coin ticker
    pub base: Ticker,
    /// Rel coin ticker
    pub rel: Ticker,
    /// Swap amount in coins (with fraction)
    pub amount: MmNumber,
    /// Partner fee, percentage of src token amount will be sent to referrer address, min: 0; max: 3.
    /// Should be the same for quote and swap rpc. Default is 0
    pub fee: Option<f32>,
    /// Specify liquidity sources
    /// e.g.: &protocols=WETH,CURVE,BALANCER,...,ZRX
    /// (by default - all used)
    pub protocols: Option<String>,
    /// Network price per gas, in Gwei for this rpc.
    /// 1inch takes in account gas expenses to determine exchange route. Should be the same for a quote and swap.
    /// If not set the 'fast' network gas price will be used
    pub gas_price: Option<String>,
    /// Maximum number of token-connectors to be used in a transaction, min: 0; max: 3; default: 2
    pub complexity_level: Option<u32>,
    /// Limit maximum number of parts each main route parts can be split into.
    /// Should be the same for a quote and swap. Default: 20; max: 100
    pub parts: Option<u32>,
    /// Limit maximum number of main route parts. Should be the same for a quote and swap. Default: 20; max: 50;
    pub main_route_parts: Option<u32>,
    /// Maximum amount of gas for a swap.
    /// Should be the same for a quote and swap. Default: 11500000; max: 11500000
    pub gas_limit: Option<u128>,
    /// Return fromToken and toToken info in response (default is true)
    #[serde(default = "true_f")]
    pub include_tokens_info: bool,
    /// Return used swap protocols in response (default is true)
    #[serde(default = "true_f")]
    pub include_protocols: bool,
    /// Include estimated gas in return value (default is true)
    #[serde(default = "true_f")]
    pub include_gas: bool,
    /// Token-connectors can be specified via this parameter. If not set, default token-connectors will be used
    pub connector_tokens: Option<String>,
}

/// Request to create transaction for 1inch classic swap.
/// See 1inch docs for more details: https://portal.1inch.dev/documentation/apis/swap/classic-swap/Parameter%20Descriptions/swap_params
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClassicSwapCreateRequest {
    /// Base coin ticker
    pub base: Ticker,
    /// Rel coin ticker
    pub rel: Ticker,
    /// Swap amount in coins (with fraction)
    pub amount: MmNumber,
    /// Allowed slippage, min: 0; max: 50
    pub slippage: f32,
    /// Partner fee, percentage of src token amount will be sent to referrer address, min: 0; max: 3.
    /// Should be the same for quote and swap rpc. Default is 0
    pub fee: Option<f32>,
    /// Specify liquidity sources
    /// e.g.: &protocols=WETH,CURVE,BALANCER,...,ZRX
    /// (by default - all used)
    pub protocols: Option<String>,
    /// Network price per gas, in Gwei for this rpc.
    /// 1inch takes in account gas expenses to determine exchange route. Should be the same for a quote and swap.
    /// If not set the 'fast' network gas price will be used
    pub gas_price: Option<String>,
    /// Maximum number of token-connectors to be used in a transaction, min: 0; max: 3; default: 2
    pub complexity_level: Option<u32>,
    /// Limit maximum number of parts each main route parts can be split into.
    /// Should be the same for a quote and swap. Default: 20; max: 100
    pub parts: Option<u32>,
    /// Limit maximum number of main route parts. Should be the same for a quote and swap. Default: 20; max: 50;
    pub main_route_parts: Option<u32>,
    /// Maximum amount of gas for a swap.
    /// Should be the same for a quote and swap. Default: 11500000; max: 11500000
    pub gas_limit: Option<u128>,
    /// Return fromToken and toToken info in response (default is true)
    #[serde(default = "true_f")]
    pub include_tokens_info: bool,
    /// Return used swap protocols in response (default is true)
    #[serde(default = "true_f")]
    pub include_protocols: bool,
    /// Include estimated gas in response (default is true)
    #[serde(default = "true_f")]
    pub include_gas: bool,
    /// Token-connectors can be specified via this parameter. If not set, default token-connectors will be used
    pub connector_tokens: Option<String>,
    /// Excluded supported liquidity sources. Should be the same for a quote and swap, max: 5
    pub excluded_protocols: Option<String>,
    /// Used according https://eips.ethereum.org/EIPS/eip-2612
    pub permit: Option<String>,
    /// Exclude the Unoswap method
    pub compatibility: Option<bool>,
    /// This address will receive funds after the swap. By default same address as 'my address'
    pub receiver: Option<String>,
    /// Address to receive the partner fee. Must be set explicitly if fee is also set
    pub referrer: Option<String>,
    /// if true, disable most of the checks, default: false
    pub disable_estimate: Option<bool>,
    /// if true, the algorithm can cancel part of the route, if the rate has become less attractive.
    /// Unswapped tokens will return to 'my address'. Default: true
    pub allow_partial_fill: Option<bool>,
    /// Enable this flag for auto approval by Permit2 contract if you did an approval to Uniswap Permit2 smart contract for this token.
    /// Default is false
    pub use_permit2: Option<bool>,
}

/// Details to create classic swap calls
#[derive(Serialize, Deserialize, Debug)]
pub struct ClassicSwapDetails {
    /// Destination token amount, in coins (with fraction)
    pub dst_amount: DetailedAmount,
    /// Source (base) token info
    #[serde(skip_serializing_if = "Option::is_none")]
    pub src_token: Option<LrTokenInfo>,
    /// Source (base) token name as it is defined in the coins file
    pub src_token_kdf: Option<Ticker>,
    /// Destination (rel) token info
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dst_token: Option<LrTokenInfo>,
    /// Destination (rel) token name as it is defined in the coins file.
    /// This is used to show route tokens in the GUI, like they are in the coin file.
    /// However, route tokens can be missed in the coins file and therefore cannot be filled.
    /// In this case GUI may use LrTokenInfo::Address or LrTokenInfo::Symbol
    pub dst_token_kdf: Option<Ticker>,
    /// Used liquidity sources
    #[serde(skip_serializing_if = "Option::is_none")]
    pub protocols: Option<Vec<Vec<Vec<ProtocolInfo>>>>,
    /// Swap tx fields (returned only for create swap rpc)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tx: Option<TxFields>,
    /// Estimated (returned only for quote rpc)
    pub gas: Option<u128>,
}

/// Response for both classic swap quote or create swap calls
pub type ClassicSwapResponse = ClassicSwapDetails;

impl ClassicSwapDetails {
    /// Get token name as it is defined in the coins file by contract address
    async fn token_name_kdf(ctx: &MmArc, chain_id: u64, token_info: &LrTokenInfo) -> Option<Ticker> {
        let special_contract =
            Address::from_str(ApiClient::eth_special_contract()).expect("1inch special address must be valid"); // TODO: must call 1inch to get it, instead of burned consts

        let platform_ticker = get_platform_ticker(ctx, chain_id)?;
        if token_info.address == special_contract {
            Some(platform_ticker)
        } else {
            get_erc20_ticker_by_contract_address(ctx, &platform_ticker, &token_info.address)
        }
    }

    pub(crate) async fn from_api_classic_swap_data(
        ctx: &MmArc,
        chain_id: u64,
        data: one_inch_api::classic_swap_types::ClassicSwapData,
    ) -> MmResult<Self, FromApiValueError> {
        let src_token_info = data
            .src_token
            .ok_or(FromApiValueError::new("Missing source TokenInfo".to_owned()))?;
        let dst_token_info = data
            .dst_token
            .ok_or(FromApiValueError::new("Missing destination TokenInfo".to_owned()))?;
        let dst_decimals: u8 = dst_token_info
            .decimals
            .try_into()
            .map_to_mm(|_| FromApiValueError::new("invalid decimals in destination TokenInfo".to_owned()))?;
        Ok(Self {
            dst_amount: MmNumber::from(
                u256_to_big_decimal(U256::from_dec_str(&data.dst_amount)?, dst_decimals).map_mm_err()?,
            )
            .into(),
            src_token_kdf: Self::token_name_kdf(ctx, chain_id, &src_token_info).await,
            src_token: Some(src_token_info),
            dst_token_kdf: Self::token_name_kdf(ctx, chain_id, &dst_token_info).await,
            dst_token: Some(dst_token_info),
            protocols: data.protocols,
            tx: data.tx.map(TxFields::from_api_tx_fields).transpose()?,
            gas: data.gas,
        })
    }
}

#[derive(Deserialize, Serialize, Debug)]
pub struct TxFields {
    pub from: Address,
    pub to: Address,
    pub data: BytesJson,
    pub value: BigDecimal,
    /// Estimated gas price in gwei
    pub gas_price: BigDecimal,
    pub gas: u128, // TODO: in eth EthTxFeeDetails rpc we use u64. Better have identical u128 everywhere
}

impl TxFields {
    pub(crate) fn from_api_tx_fields(
        tx_fields: one_inch_api::classic_swap_types::TxFields,
    ) -> MmResult<Self, FromApiValueError> {
        Ok(Self {
            from: tx_fields.from,
            to: tx_fields.to,
            data: BytesJson::from(hex::decode(str_strip_0x!(tx_fields.data.as_str()))?),
            value: wei_to_eth_decimal(U256::from_dec_str(&tx_fields.value)?).map_mm_err()?,
            gas_price: wei_to_gwei_decimal(U256::from_dec_str(&tx_fields.gas_price)?).map_mm_err()?,
            gas: tx_fields.gas,
        })
    }
}

#[derive(Deserialize)]
pub struct ClassicSwapLiquiditySourcesRequest {
    pub chain_id: u64,
}

#[derive(Serialize)]
pub struct ClassicSwapLiquiditySourcesResponse {
    pub protocols: Vec<ProtocolImage>,
}

#[derive(Deserialize)]
pub struct ClassicSwapTokensRequest {
    pub chain_id: u64,
}

#[derive(Serialize)]
pub struct ClassicSwapTokensResponse {
    pub tokens: HashMap<Ticker, LrTokenInfo>,
}
