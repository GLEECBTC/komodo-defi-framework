//! Types for LR swaps rpc

use crate::lp_ordermatch::RpcOrderbookEntryV2;
use crate::rpc::lp_commands::ext_api::ext_api_errors::ExtApiRpcError;
use crate::rpc::lp_commands::ext_api::ext_api_types::{ClassicSwapCreateOptParams, ClassicSwapDetails};
use coins::Ticker;
use mm2_number::{MmNumber, MmNumberMultiRepr};
use mm2_rpc::data::legacy::{MatchBy, OrderType};
use uuid::Uuid;

/// Struct to pass maker ask orders into liquidity routing RPCs
#[derive(Debug, Deserialize)]
pub struct AsksForCoin {
    /// Base coin for ask orders
    pub base: Ticker,
    /// Best maker ask orders that could be filled with liquidity routing from the User source_token into ask's rel token
    pub orders: Vec<RpcOrderbookEntryV2>,
}

/// Struct to pass maker bid orders into liquidity routing RPCs
#[derive(Debug, Deserialize)]
pub struct BidsForCoin {
    /// Rel coin for bid orders
    pub rel: Ticker,
    /// Best maker ask orders that could be filled with liquidity routing from the User source_token into ask's rel token
    pub orders: Vec<RpcOrderbookEntryV2>,
}

/// Struct to return the best order from liquidity routing RPCs
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum AskOrBidOrder {
    Ask { base: Ticker, order: RpcOrderbookEntryV2 },
    Bid { rel: Ticker, order: RpcOrderbookEntryV2 },
}

impl AskOrBidOrder {
    pub fn order(&self) -> &RpcOrderbookEntryV2 {
        match self {
            AskOrBidOrder::Ask { order, .. } => order,
            AskOrBidOrder::Bid { order, .. } => order,
        }
    }
    pub fn maker_ticker(&self) -> Ticker {
        match self {
            AskOrBidOrder::Ask { base, .. } => base.clone(),
            AskOrBidOrder::Bid { order, .. } => order.coin.clone(),
        }
    }
    pub fn taker_ticker(&self) -> Ticker {
        match self {
            AskOrBidOrder::Ask { order, .. } => order.coin.clone(),
            AskOrBidOrder::Bid { rel, .. } => rel.clone(),
        }
    }

    /// Convert to maker sell price
    pub fn sell_price(&self) -> MmNumber {
        match self {
            AskOrBidOrder::Ask { order, .. } => order.price.rational.clone().into(),
            AskOrBidOrder::Bid { order, .. } => &MmNumber::from(1) / &order.price.rational.clone().into(),
        }
    }
    /// Convert to maker buy price
    #[allow(unused)]
    pub fn buy_price(&self) -> MmNumber {
        match self {
            AskOrBidOrder::Ask { order, .. } => &MmNumber::from(1) / &order.price.rational.clone().into(),
            AskOrBidOrder::Bid { order, .. } => order.price.rational.clone().into(),
        }
    }

    pub fn max_maker_vol(&self) -> MmNumberMultiRepr {
        match self {
            AskOrBidOrder::Ask { order, .. } => order.base_max_volume.clone(),
            AskOrBidOrder::Bid { order, .. } => order.rel_max_volume.clone(),
        }
    }

    pub fn min_maker_vol(&self) -> MmNumberMultiRepr {
        match self {
            AskOrBidOrder::Ask { order, .. } => order.base_min_volume.clone(),
            AskOrBidOrder::Bid { order, .. } => order.rel_min_volume.clone(),
        }
    }

    pub fn max_taker_vol(&self) -> MmNumberMultiRepr {
        match self {
            AskOrBidOrder::Ask { order, .. } => order.rel_max_volume.clone(),
            AskOrBidOrder::Bid { order, .. } => order.base_max_volume.clone(),
        }
    }

    pub fn min_taker_vol(&self) -> MmNumberMultiRepr {
        match self {
            AskOrBidOrder::Ask { order, .. } => order.rel_min_volume.clone(),
            AskOrBidOrder::Bid { order, .. } => order.base_min_volume.clone(),
        }
    }
}

/// Request to find best swap path with LR to fill an order from list.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LrFindBestQuoteRequest {
    /// Base coin to fill an atomic swap maker order with possible liquidity routing from this coin over a coin/token in an ask/bid
    pub user_base: Ticker,
    /// List of maker atomic swap ask orders, to find best swap path with liquidity routing from user_base or user_rel coin
    pub asks: Vec<AsksForCoin>,
    /// List of maker atomic swap bid orders, to find best swap path with liquidity routing from user_base or user_rel coin
    pub bids: Vec<BidsForCoin>,
    /// Buy or sell volume (in coin units, i.e. with fraction)
    pub volume: MmNumber,
    /// Method buy or sell
    /// TODO: use this field, now we support 'buy' only
    pub method: String,
    /// Rel coin to fill an atomic swap maker order with possible liquidity routing from this coin over a coin/token in an ask/bid
    pub user_rel: Ticker,
}

/// Response for find best swap path with LR
#[derive(Debug, Serialize)]
pub struct LrFindBestQuoteResponse {
    /// LR_0 tx data (from 1inch quote)
    pub lr_data_0: Option<ClassicSwapDetails>,
    /// LR_1 tx data (from 1inch quote)
    pub lr_data_1: Option<ClassicSwapDetails>,
    /// found best order which can be filled with LR swap
    pub atomic_swap: AtomicSwapRpcParams,
    /// base/rel price including the price of the LR swap part
    pub total_price: MmNumber,
    // /// Fees to pay, including LR swap fee
    // pub trade_fee: TradePreimageResponse, // TODO: implement when trade_preimage implemented for TPU
}

/// Request to get quotes with possible swap paths to fill order with multiple tokens with LR
#[allow(dead_code)]
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LrGetQuotesForTokensRequest {
    /// Order base coin ticker (from the orderbook).
    pub base: Ticker,
    /// Swap amount in base coins to sell (with fraction)
    pub amount: MmNumber,
    /// Maker order to find possible swap path with LR
    pub orderbook_entry: RpcOrderbookEntryV2,
    /// List of user tokens to trade with LR
    pub my_tokens: Vec<Ticker>,
}

/// Details for best swap with LR
#[derive(Debug, Serialize)]
pub struct QuoteDetails {
    /// interim token to route to/from
    pub dest_token: Ticker,
    /// Swap tx data (from 1inch quote)
    pub lr_swap_details: ClassicSwapDetails,
    /// total swap price with LR
    pub total_price: MmNumber,
    // /// Fees to pay, including LR swap fee
    // pub trade_fee: TradePreimageResponse, // TODO: implement when trade_preimage implemented for TPU
}

/// Response for quotes to fill order with LR
#[derive(Debug, Serialize)]
pub struct LrGetQuotesForTokensResponse {
    pub quotes: Vec<QuoteDetails>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LrSwapRpcParams {
    pub slippage: f32,
    pub swap_details: ClassicSwapDetails,
    pub opt_params: Option<ClassicSwapCreateOptParams>,
}

impl LrSwapRpcParams {
    #[allow(clippy::result_large_err, unused)]
    pub fn get_source_token(&self) -> Result<Ticker, ExtApiRpcError> {
        Ok(self
            .swap_details
            .src_token
            .as_ref()
            .ok_or(ExtApiRpcError::NoLrTokenInfo)?
            .symbol_kdf
            .as_ref()
            .ok_or(ExtApiRpcError::NoLrTokenInfo)?
            .clone())
    }

    #[allow(clippy::result_large_err, unused)]
    pub fn get_destination_token(&self) -> Result<Ticker, ExtApiRpcError> {
        Ok(self
            .swap_details
            .dst_token
            .as_ref()
            .ok_or(ExtApiRpcError::NoLrTokenInfo)?
            .symbol_kdf
            .as_ref()
            .ok_or(ExtApiRpcError::NoLrTokenInfo)?
            .clone())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AtomicSwapRpcParams {
    pub volume: Option<MmNumber>,
    pub base: Ticker,
    pub rel: Ticker,
    pub price: MmNumber,
    pub method: String,
    pub order_uuid: Uuid,
    #[serde(default)]
    pub match_by: Option<MatchBy>,
    #[serde(default)]
    pub order_type: Option<OrderType>,
    // TODO: add opt params
}

/// Request to fill a maker order with atomic swap with LR steps
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LrExecuteRoutedTradeRequest {
    /// Sell or buy params for the atomic swap step
    #[allow(unused)]
    pub atomic_swap: AtomicSwapRpcParams,

    /// Params to create 1inch LR swap (from 1inch quote)
    /// TODO: make this an enum to allow other LR providers
    #[allow(unused)]
    pub lr_swap_0: Option<LrSwapRpcParams>,

    /// Params to create 1inch LR swap (from 1inch quote)
    #[allow(unused)]
    pub lr_swap_1: Option<LrSwapRpcParams>,
}

/// Response to sell or buy order with LR
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LrExecuteRoutedTradeResponse {
    /// Created aggregated swap uuid for tracking the swap
    pub uuid: Uuid,
}
