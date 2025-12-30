//! Code for swaps with liquidity routing (LR)

use coins::Ticker;
use ethereum_types::{Address as EthAddress, U256};
use lr_errors::LrSwapError;
use mm2_number::MmNumber;
use mm2_rpc::data::legacy::{MatchBy, OrderType, TakerAction};
use trading_api::one_inch_api::classic_swap_types::ClassicSwapData;

pub(crate) mod lr_errors;
pub(crate) mod lr_helpers;
pub(crate) mod lr_quote;

/// Liquidity routing data for the aggregated taker swap state machine.
/// Used for a DEX swap step (via 1inch) before or after an atomic swap.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LrSwapParams {
    /// Source token amount in human-readable coin units (e.g., "1.5" ETH, not wei)
    pub src_amount: MmNumber,
    /// Source token KDF ticker
    pub src: Ticker,
    /// Source token ERC20 contract address (or special 1inch ETH address for native coins)
    pub src_contract: EthAddress,
    /// Source token decimals
    pub src_decimals: u8,
    /// Destination token KDF ticker
    pub dst: Ticker,
    /// Destination token ERC20 contract address
    pub dst_contract: EthAddress,
    /// Destination token decimals
    pub dst_decimals: u8,
    /// User's wallet address that will execute the swap
    pub from: EthAddress,
    /// Maximum acceptable slippage percentage for the DEX swap (0.0 to 50.0)
    pub slippage: f32,
}

/// Atomic swap data for the aggregated taker swap state machine.
/// Represents the P2P atomic swap step in a liquidity-routed swap.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AtomicSwapParams {
    /// Base coin volume in human-readable units. None if not yet calculated.
    pub base_volume: Option<MmNumber>,
    /// Base coin KDF ticker (the coin being bought when action is Buy)
    pub base: Ticker,
    /// Rel coin KDF ticker (the coin being sold when action is Buy)
    pub rel: Ticker,
    /// Price in rel/base units (how much rel per one base)
    pub price: MmNumber,
    /// Whether taker is buying or selling base coin
    pub action: TakerAction,
    /// Order matching strategy: Any, specific Orders, or specific Pubkeys
    #[serde(default)]
    pub match_by: MatchBy,
    /// FillOrKill or GoodTillCancelled
    #[serde(default)]
    pub order_type: OrderType,
}

impl AtomicSwapParams {
    #[allow(unused)]
    pub(crate) fn maker_coin(&self) -> Ticker {
        match self.action {
            TakerAction::Buy => self.base.clone(),
            TakerAction::Sell => self.rel.clone(),
        }
    }

    #[allow(unused)]
    pub(crate) fn taker_coin(&self) -> Ticker {
        match self.action {
            TakerAction::Buy => self.rel.clone(),
            TakerAction::Sell => self.base.clone(),
        }
    }

    #[allow(clippy::result_large_err, unused)]
    pub(crate) fn taker_volume(&self) -> Result<MmNumber, LrSwapError> {
        let Some(ref volume) = self.base_volume else {
            return Err(LrSwapError::InternalError("no atomic swap volume".to_owned()));
        };
        match self.action {
            TakerAction::Buy => Ok(volume * &self.price),
            TakerAction::Sell => Ok(volume.clone()),
        }
    }

    #[allow(clippy::result_large_err, unused)]
    pub(crate) fn maker_volume(&self) -> Result<MmNumber, LrSwapError> {
        let Some(ref volume) = self.base_volume else {
            return Err(LrSwapError::InternalError("no atomic swap volume".to_owned()));
        };
        match self.action {
            TakerAction::Buy => Ok(volume.clone()),
            TakerAction::Sell => Ok(volume * &self.price),
        }
    }
}

/// Struct to return extra data (src_amount) in addition to 1inch swap details
pub(crate) struct ClassicSwapDataExt {
    pub api_details: ClassicSwapData,
    /// Estimated source amount (in wei) for a liquidity routing swap step, includes needed amount to fill the order, plus dex and trade fees (if needed)
    pub src_amount: U256,
    pub chain_id: u64,
}
