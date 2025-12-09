//! Code for swaps with liquidity routing (LR)

use coins::Ticker;
use ethereum_types::{Address as EthAddress, U256};
use lr_errors::LrSwapError;
use mm2_number::MmNumber;
use mm2_rpc::data::legacy::{MatchBy, OrderType, TakerAction};
use trading_api::one_inch_api::classic_swap_types::ClassicSwapData;

pub(crate) mod lr_errors;
pub(crate) mod lr_helpers;
pub(crate) mod lr_impl;
pub(crate) mod lr_swap_state_machine;

/// Liquidity routing data for the aggregated taker swap state machine
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LrSwapParams {
    pub src_amount: MmNumber,
    pub src: String,
    pub src_contract: EthAddress,
    pub src_decimals: u8,
    pub dst: String,
    pub dst_contract: EthAddress,
    pub dst_decimals: u8,
    pub from: EthAddress,
    pub slippage: f32,
    pub gas: u64,
}

/// Atomic swap data for the aggregated taker swap state machine
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AtomicSwapParams {
    pub base_volume: Option<MmNumber>,
    pub base: String,
    pub rel: String,
    pub price: MmNumber,
    pub action: TakerAction,
    #[serde(default)]
    pub match_by: MatchBy,
    #[serde(default)]
    pub order_type: OrderType,
}

impl AtomicSwapParams {
    pub(crate) fn maker_coin(&self) -> Ticker {
        match self.action {
            TakerAction::Buy => self.base.clone(),
            TakerAction::Sell => self.rel.clone(),
        }
    }

    pub(crate) fn taker_coin(&self) -> Ticker {
        match self.action {
            TakerAction::Buy => self.rel.clone(),
            TakerAction::Sell => self.base.clone(),
        }
    }

    #[allow(clippy::result_large_err)]
    pub(crate) fn taker_volume(&self) -> Result<MmNumber, LrSwapError> {
        let Some(ref volume) = self.base_volume else {
            return Err(LrSwapError::InternalError("no atomic swp volume".to_owned()));
        };
        match self.action {
            TakerAction::Buy => Ok(volume * &self.price),
            TakerAction::Sell => Ok(volume.clone()),
        }
    }

    #[allow(clippy::result_large_err, unused)]
    pub(crate) fn maker_volume(&self) -> Result<MmNumber, LrSwapError> {
        let Some(ref volume) = self.base_volume else {
            return Err(LrSwapError::InternalError("no atomic swp volume".to_owned()));
        };
        match self.action {
            TakerAction::Buy => Ok(volume.clone()),
            TakerAction::Sell => Ok(volume * &self.price),
        }
    }
}

/// Struct to return extra data (src_amount) in addition to 1inch swap details
pub struct ClassicSwapDataExt {
    pub api_details: ClassicSwapData,
    /// Estimated source amount for a liquidity routing swap step, includes needed amount to fill the order, plus dex and trade fees (if needed)
    pub src_amount: U256,
    pub chain_id: u64,
}
