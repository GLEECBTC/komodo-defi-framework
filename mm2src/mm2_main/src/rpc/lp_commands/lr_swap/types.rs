//! Types for LR swaps rpc

// Most of the code in this module fails on clippy.
#![allow(dead_code)]

use crate::lp_ordermatch::RpcOrderbookEntryV2;
use crate::rpc::lp_commands::one_inch::types::ClassicSwapDetails;
use coins::Ticker;
use mm2_number::MmNumber;
use mm2_rpc::data::legacy::{SellBuyRequest, SellBuyResponse};

#[derive(Debug, Deserialize)]
pub struct AsksForCoin {
    /// Base coin for ask orders
    pub base: Ticker,
    /// Best maker ask orders that could be filled with liquidity routing from the User source_token into ask's rel token
    pub orders: Vec<RpcOrderbookEntryV2>,
}

#[derive(Debug, Deserialize)]
pub struct BidsForCoin {
    /// Rel coin for bid orders
    pub rel: Ticker,
    /// Best maker ask orders that could be filled with liquidity routing from the User source_token into ask's rel token
    pub orders: Vec<RpcOrderbookEntryV2>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
pub enum AskOrBidOrder {
    Ask { base: Ticker, order: RpcOrderbookEntryV2 },
    Bid { rel: Ticker, order: RpcOrderbookEntryV2 },
}

impl AskOrBidOrder {
    pub fn order(&self) -> &RpcOrderbookEntryV2 {
        match self {
            AskOrBidOrder::Ask { base: _, order } => order,
            AskOrBidOrder::Bid { rel: _, order } => order,
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
    /// Swap tx data (from 1inch quote)
    pub lr_swap_details: ClassicSwapDetails,
    /// found best order which can be filled with LR swap
    pub best_order: AskOrBidOrder,
    /// base/rel price including the price of the LR swap part
    pub total_price: MmNumber,
    // /// Fees to pay, including LR swap fee
    // pub trade_fee: TradePreimageResponse, // TODO: implement when trade_preimage implemented for TPU
}

/// Request to get quotes with possible swap paths to fill order with multiple tokens with LR
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

/// Details with swap with LR
#[derive(Debug, Serialize)]
pub struct QuotesDetails {
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
    pub quotes: Vec<QuotesDetails>,
}

/// Request to sell or buy order with LR
/// TODO: this struct will be changed in the next PR
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LrExecuteRoutedTradeRequest {
    /// Original sell or buy request (but only MatchBy::Orders could be used to fill the maker swap found in )
    #[serde(flatten)]
    pub fill_req: SellBuyRequest,

    /// Tx data to create one inch swap (from 1inch quote)
    /// TODO: make this an enum to allow other LR providers
    pub lr_swap_details: ClassicSwapDetails,
}

/// Response to sell or buy order with LR
#[derive(Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LrExecuteRoutedTradeResponse {
    /// Original sell or buy response
    #[serde(flatten)]
    pub fill_response: SellBuyResponse,
}
