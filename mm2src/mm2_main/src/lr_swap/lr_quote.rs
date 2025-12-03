//! Implementation for finding the best-priced quote for a Taker swap with liquidity routing (LR).
//! A swap with LR may have interim conversion (LR swap) of a source token into a token needed to send in the atomic swap,
//! or, conversion of a received atomic swap token into a destination token.
//! LR currently is supported for EVM chains.

use super::lr_errors::LrSwapError;
use super::lr_helpers::get_coin_for_one_inch;
use crate::lp_swap::taker_swap::TakerSwapPreparedParams;
use crate::lr_swap::ClassicSwapDataExt;
use crate::rpc::lp_commands::lr_swap_api::lr_api_types::{AskOrBidOrder, AsksForCoin, BidsForCoin};
use coins::eth::eth_utils::wei_to_coins_mm_number;
use coins::eth::{mm_number_from_u256, wei_from_coins_mm_number, EthCoin};
use coins::hd_wallet::AddrToString;
use coins::{lp_coinfind_or_err, MarketCoinOps};
use coins::{DexFee, MmCoin, Ticker};
use common::log;
use ethereum_types::{Address as EthAddress, U256};
use futures::future::{join_all, BoxFuture};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::MmNumber;
use mm2_rpc::data::legacy::TakerAction;
use num_traits::CheckedDiv;
use std::collections::HashMap;
use std::ops::Deref;
use trading_api::one_inch_api::classic_swap_types::{ClassicSwapData, ClassicSwapQuoteCallBuilder};
use trading_api::one_inch_api::client::{
    ApiClient, PortfolioApiMethods, PortfolioUrlBuilder, SwapApiMethods, SwapUrlBuilder,
};
use trading_api::one_inch_api::errors::OneInchError;
use trading_api::one_inch_api::portfolio_types::{CrossPriceParams, CrossPricesSeries, DataGranularity};

/// To estimate src/dst price query price history for every 5 min
const CROSS_PRICES_GRANULARITY: DataGranularity = DataGranularity::FiveMin;
/// Use no more than this number of price history samples to estimate src/dst price
/// NOTE: we need the most actual price for estimation, however for limit = 1 the provider often returns an empty result
#[allow(unused)]
const CROSS_PRICES_LIMIT: u32 = 10;

type ClassicSwapDataResult = MmResult<ClassicSwapData, OneInchError>;

/// Internal struct to collect data for LR step
#[allow(dead_code)] // 'Clone' is detected as dead code in one combinator
#[derive(Clone)]
struct LrStepData {
    /// Source coin or token ticker (to swap from)
    _src_token: Ticker,
    /// Source token contract address
    src_contract: Option<EthAddress>,
    /// Source token amount (estimated) in smallest units
    src_amount: Option<U256>,
    /// Source token decimals
    src_decimals: Option<u8>,
    /// Destination coin or token ticker (to swap into)
    _dst_token: Ticker,
    /// Destination token contract address
    dst_contract: Option<EthAddress>,
    /// Estimated destination token amount from LR step, in smallest units
    dst_amount: Option<U256>,
    /// Destination token decimals
    dst_decimals: Option<u8>,
    /// Chain id where LR swap occurs (obtained from the destination token)
    chain_id: Option<u64>,
    /// Estimated src token / dst token price. NOTE: the price is calculated in smallest units
    lr_price: Option<MmNumber>,
    /// Estimated dex fee and taker fee amounts to include in LR step quote
    /// TODO: return in the rpc result
    taker_swap_params: Option<TakerSwapPreparedParams>,
    /// Dex fee added to the source amount, needed to pay in the atomic swap
    dex_fee: Option<MmNumber>,
    /// A quote from LR provider with destination amount for the LR step
    lr_swap_data: Option<ClassicSwapData>,
}

impl LrStepData {
    fn new(src_ticker: Ticker, dst_ticker: Ticker) -> Self {
        LrStepData {
            _src_token: src_ticker,
            src_contract: None,
            src_decimals: None,
            src_amount: None,
            _dst_token: dst_ticker,
            dst_contract: None,
            dst_amount: None,
            dst_decimals: None,
            chain_id: None,
            lr_price: None,
            taker_swap_params: None,
            dex_fee: None,
            lr_swap_data: None,
        }
    }

    #[allow(clippy::result_large_err)]
    fn get_chain_contract_info(&self) -> MmResult<(String, String, u64), LrSwapError> {
        let src_contract = self
            .src_contract
            .as_ref()
            .ok_or(LrSwapError::InternalError("Source LR contract not set".to_owned()))?
            .addr_to_string();
        let dst_contract = self
            .dst_contract
            .as_ref()
            .ok_or(LrSwapError::InternalError("Destination LR contract not set".to_owned()))?
            .addr_to_string();
        let chain_id = self
            .chain_id
            .ok_or(LrSwapError::InternalError("LR chain id not set".to_owned()))?;
        Ok((src_contract, dst_contract, chain_id))
    }

    #[allow(clippy::result_large_err)]
    fn src_amount(&self) -> Result<U256, LrSwapError> {
        self.src_amount
            .ok_or(LrSwapError::InternalError("no src_amount".to_owned()))
    }

    #[allow(clippy::result_large_err, unused)]
    fn dst_amount(&self) -> Result<U256, LrSwapError> {
        self.dst_amount
            .ok_or(LrSwapError::InternalError("no dst_amount".to_owned()))
    }

    #[allow(clippy::result_large_err)]
    fn src_decimals(&self) -> Result<u8, LrSwapError> {
        self.src_decimals
            .ok_or(LrSwapError::InternalError("no src_decimals".to_owned()))
    }

    #[allow(clippy::result_large_err)]
    fn dst_decimals(&self) -> Result<u8, LrSwapError> {
        self.dst_decimals
            .ok_or(LrSwapError::InternalError("no dst_decimals".to_owned()))
    }
}

struct LrSwapCandidateInfo {
    /// Data for liquidity routing before atomic swap
    lr_data_0: Option<LrStepData>,
    /// Atomic swap order to fill
    maker_order: AskOrBidOrder,
    /// Estimated atomic swap initial sell amount, if LR_0 not present
    atomic_swap_taker_amount: Option<U256>,
    /// Estimated atomic swap final buy amount, if LR_1 not present
    atomic_swap_maker_amount: Option<U256>,
    /// Data for liquidity routing after atomic swap
    lr_data_1: Option<LrStepData>,
}

/// Array to store data (possible swap route candidated, with prices for each step) needed for estimation
/// of the aggregated swap with liquidity routing, with the best total price
struct LrSwapCandidates {
    // The array of swaps with LR candidates
    inner: Vec<LrSwapCandidateInfo>,
}

/// Mutable iterator over lr_data_0 field
struct LrStepDataMut0<'a> {
    inner: std::slice::IterMut<'a, LrSwapCandidateInfo>,
}

impl<'a> Iterator for LrStepDataMut0<'a> {
    type Item = &'a mut LrStepData;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().and_then(|item| item.lr_data_0.as_mut())
    }
}

/// Mutable iterator over lr_data_1 field
struct LrStepDataMut1<'a> {
    inner: std::slice::IterMut<'a, LrSwapCandidateInfo>,
}

impl<'a> Iterator for LrStepDataMut1<'a> {
    type Item = &'a mut LrStepData;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().and_then(|item| item.lr_data_1.as_mut())
    }
}

impl LrSwapCandidates {
    fn iter_mut_lr_data_0(&mut self) -> LrStepDataMut0<'_> {
        LrStepDataMut0 {
            inner: self.inner.iter_mut(),
        }
    }

    fn iter_mut_lr_data_1(&mut self) -> LrStepDataMut1<'_> {
        LrStepDataMut1 {
            inner: self.inner.iter_mut(),
        }
    }

    /// Init LR data map from the source token (mytoken) and tokens from orders
    async fn new_with_orders(
        ctx: &MmArc,
        user_base_ticker: Ticker,
        user_rel_ticker: Ticker,
        action: &TakerAction,
        asks_coins: Vec<AsksForCoin>,
        bids_coins: Vec<BidsForCoin>,
    ) -> Self {
        async fn tokens_in_same_chain(ctx: &MmArc, coin: &EthCoin, other_ticker: &Ticker) -> bool {
            if let Some(chain_id) = coin.chain_id() {
                if let Ok(other_coin) = get_coin_for_one_inch(ctx, other_ticker).await {
                    if let Some(other_chain_id) = other_coin.0.chain_id() {
                        if chain_id == other_chain_id {
                            return true;
                        }
                    }
                }
            }
            false
        }

        async fn create_candidate(
            ctx: &MmArc,
            maker_order: AskOrBidOrder,
            user_src_ticker: &Ticker,
            user_src_eth_coin: Option<&EthCoin>,
            user_dst_ticker: &Ticker,
            user_dst_eth_coin: Option<&EthCoin>,
        ) -> Option<LrSwapCandidateInfo> {
            let mut lr_data_0 = None;
            let mut lr_data_1 = None;
            // Add as a LR_0 candidate if the user source token and maker ask rel are in the same EVM chain, but different tokens
            if let Some(user_src_eth_coin) = user_src_eth_coin {
                log::debug!(
                    "checking asks for same chain for LR_0 user_src_eth_coin={} taker_ticker={}",
                    user_src_eth_coin.ticker(),
                    maker_order.taker_ticker()
                );
                if tokens_in_same_chain(ctx, user_src_eth_coin, &maker_order.taker_ticker()).await
                    && user_src_eth_coin.ticker() != maker_order.taker_ticker().as_str()
                {
                    // Route from source token to maker ask rel
                    lr_data_0 = Some(LrStepData::new(
                        user_src_eth_coin.ticker().to_owned(),
                        maker_order.taker_ticker(),
                    ));
                } else {
                    log::debug!("checking same chain - NOT");
                }
            }

            // Add as a LR_1 candidate if the ask base and the user destination token are in the same EVM chain, but different tokens
            if let Some(user_dst_eth_coin) = user_dst_eth_coin {
                log::debug!(
                    "checking asks for same chain for LR_1 user_dst_eth_coin={} maker_ticker={}",
                    user_dst_eth_coin.ticker(),
                    maker_order.maker_ticker()
                );
                if tokens_in_same_chain(ctx, user_dst_eth_coin, &maker_order.maker_ticker()).await
                    && maker_order.maker_ticker() != user_dst_eth_coin.ticker()
                {
                    // Route from source token to maker ask rel
                    lr_data_1 = Some(LrStepData::new(
                        maker_order.maker_ticker(),
                        user_dst_eth_coin.ticker().to_owned(),
                    ));
                } else {
                    log::debug!("checking same chain - NOT");
                }
            }
            // do not add orders w/o any LR or w/o our coin
            if (lr_data_0.is_some() || &maker_order.taker_ticker() == user_src_ticker)
                && (lr_data_1.is_some() || &maker_order.maker_ticker() == user_dst_ticker)
            {
                log::debug!(
                    "ask candidate added: LR_0: {:?}/{:?} taker/maker: {}/{} LR_1 {:?}/{:?}",
                    lr_data_0.as_ref().map(|d| &d._src_token),
                    lr_data_0.as_ref().map(|d| &d._dst_token),
                    maker_order.taker_ticker(),
                    maker_order.maker_ticker(),
                    lr_data_1.as_ref().map(|d| &d._src_token),
                    lr_data_1.as_ref().map(|d| &d._dst_token)
                );

                let candidate = LrSwapCandidateInfo {
                    lr_data_0,
                    maker_order,
                    atomic_swap_taker_amount: None,
                    atomic_swap_maker_amount: None,
                    lr_data_1,
                };
                Some(candidate)
            } else {
                log::debug!("ask candidate not added: lr_data_0.is_some: {} taker_ticker == user_src_ticker: {} lr_data_1.is_some: {} maker_ticker == user_dst_ticker: {}", 
                    lr_data_0.is_some(), &maker_order.taker_ticker() == user_src_ticker, lr_data_1.is_some(), &maker_order.maker_ticker() == user_dst_ticker);
                None
            }
        }

        let user_base = get_coin_for_one_inch(ctx, &user_base_ticker).await.ok();
        let user_rel = get_coin_for_one_inch(ctx, &user_rel_ticker).await.ok();
        let (user_src_ticker, user_src_eth_coin, user_dst_ticker, user_dst_eth_coin) = match action {
            TakerAction::Buy => (
                user_rel_ticker,
                user_rel.map(|tupl| tupl.0),
                user_base_ticker,
                user_base.map(|tupl| tupl.0),
            ),
            TakerAction::Sell => (
                user_base_ticker,
                user_base.map(|tupl| tupl.0),
                user_rel_ticker,
                user_rel.map(|tupl| tupl.0),
            ),
        };

        let mut inner = vec![];
        for asks_for_coin in asks_coins {
            for order in asks_for_coin.orders {
                let maker_order = AskOrBidOrder::Ask {
                    base: asks_for_coin.base.clone(),
                    order,
                };
                if let Some(candidate) = create_candidate(
                    ctx,
                    maker_order,
                    &user_src_ticker,
                    user_src_eth_coin.as_ref(),
                    &user_dst_ticker,
                    user_dst_eth_coin.as_ref(),
                )
                .await
                {
                    inner.push(candidate);
                }
            }
        }
        for bids_for_coin in bids_coins {
            for order in bids_for_coin.orders {
                let maker_order = AskOrBidOrder::Bid {
                    rel: bids_for_coin.rel.clone(),
                    order,
                };
                if let Some(candidate) = create_candidate(
                    ctx,
                    maker_order,
                    &user_src_ticker,
                    user_src_eth_coin.as_ref(),
                    &user_dst_ticker,
                    user_dst_eth_coin.as_ref(),
                )
                .await
                {
                    inner.push(candidate);
                }
            }
        }
        Self { inner }
    }

    /// Estimate LR_0 destination tokens amounts required to fill maker orders for the required maker_amount.
    /// The maker_amount is the source_amount of the LR_1 (if present) or the one provided in the params.
    /// The function multiplies the maker_amount by the order price.
    /// The maker_amount must be in coin units (with decimals)
    async fn calc_lr_0_destination_amounts(
        &mut self,
        ctx: &MmArc,
        user_buy_amount: &MmNumber,
    ) -> MmResult<(), LrSwapError> {
        for candidate in self.inner.iter_mut() {
            let taker_ticker = &candidate.maker_order.taker_ticker();
            let taker_coin = lp_coinfind_or_err(ctx, taker_ticker).await.map_mm_err()?;
            let maker_sell_price = candidate.maker_order.sell_price();
            let maker_amount = if let Some(ref lr_data_1) = candidate.lr_data_1 {
                let Some(src_amount) = lr_data_1.src_amount else {
                    continue; // No src_amount means LR provider did not send us cross prices, skipping this candidate
                };
                wei_to_coins_mm_number(src_amount, lr_data_1.src_decimals()?).map_mm_err()?
            } else {
                user_buy_amount.clone()
            };
            let dst_amount = &maker_amount * &maker_sell_price;
            let dst_amount = wei_from_coins_mm_number(&dst_amount, taker_coin.decimals()).map_mm_err()?;
            if let Some(ref mut lr_data_0) = candidate.lr_data_0 {
                lr_data_0.dst_amount = Some(dst_amount);
            } else {
                candidate.atomic_swap_taker_amount = Some(dst_amount);
            }

            log::debug!(
                "calc_lr_0_destination_amounts taker_ticker={} taker_coin.decimals()={} lr_data_0.dst_amount={:?}",
                taker_ticker,
                taker_coin.decimals(),
                dst_amount
            );
        }
        Ok(())
    }

    fn update_lr_prices(lr_data_refs: Vec<&mut LrStepData>, lr_prices: HashMap<(Ticker, Ticker), Option<MmNumber>>) {
        for item in lr_data_refs {
            if let Some(prices) = lr_prices.get(&(item._src_token.clone(), item._dst_token.clone())) {
                item.lr_price = prices.clone();
            }
        }
    }

    /// Set LR_0 src_amount to user sell amount, or if LR_0 not present, set atomic_swap_src_amount
    #[allow(clippy::result_large_err)]
    async fn set_lr_0_src_amount(&mut self, ctx: &MmArc, mm_amount: &MmNumber) -> MmResult<(), LrSwapError> {
        for candidate in self.inner.iter_mut() {
            if let Some(ref mut lr_data_0) = candidate.lr_data_0 {
                let amount = wei_from_coins_mm_number(mm_amount, lr_data_0.src_decimals()?).map_mm_err()?;
                lr_data_0.src_amount = Some(amount);
            } else {
                let taker_coin = lp_coinfind_or_err(ctx, &candidate.maker_order.taker_ticker())
                    .await
                    .map_mm_err()?;
                let taker_amount = wei_from_coins_mm_number(mm_amount, taker_coin.decimals()).map_mm_err()?;
                candidate.atomic_swap_taker_amount = Some(taker_amount);
            }
        }
        Ok(())
    }

    fn update_lr_0_swap_data(&mut self, lr_swap_data: Vec<(usize, Option<ClassicSwapData>)>) {
        for item in lr_swap_data {
            if let Some(candidate) = self.inner.get_mut(item.0) {
                if let Some(lr_data_0) = candidate.lr_data_0.as_mut() {
                    lr_data_0.lr_swap_data = item.1;
                }
            }
        }
    }

    fn update_lr_1_swap_data(&mut self, lr_swap_data: Vec<(usize, Option<ClassicSwapData>)>) {
        for item in lr_swap_data {
            if let Some(candidate) = self.inner.get_mut(item.0) {
                if let Some(lr_data_1) = candidate.lr_data_1.as_mut() {
                    lr_data_1.lr_swap_data = item.1;
                }
            }
        }
    }

    async fn set_contracts(&mut self, ctx: &MmArc) -> MmResult<(), LrSwapError> {
        for candidate in self.inner.iter_mut() {
            if let Some(ref mut lr_data_0) = candidate.lr_data_0 {
                let (src_coin, src_contract) = get_coin_for_one_inch(ctx, &lr_data_0._src_token).await?;
                let (dst_coin, dst_contract) = get_coin_for_one_inch(ctx, &lr_data_0._dst_token).await?;
                let src_decimals = src_coin.decimals();
                let dst_decimals = dst_coin.decimals();
                log::debug!(
                    "src_coin={} src_decimals={} dst_coin={} dst_decimals={}",
                    src_coin.ticker(),
                    src_decimals,
                    dst_coin.ticker(),
                    dst_decimals
                );

                #[cfg(feature = "for-tests")]
                {
                    assert_ne!(src_decimals, 0);
                    assert_ne!(dst_decimals, 0);
                }

                lr_data_0.src_contract = Some(src_contract);
                lr_data_0.dst_contract = Some(dst_contract);
                lr_data_0.src_decimals = Some(src_decimals);
                lr_data_0.dst_decimals = Some(dst_decimals);
                lr_data_0.chain_id = dst_coin.chain_id();
            }
            if let Some(ref mut lr_data_1) = candidate.lr_data_1 {
                let (src_coin, src_contract) = get_coin_for_one_inch(ctx, &lr_data_1._src_token).await?;
                let (dst_coin, dst_contract) = get_coin_for_one_inch(ctx, &lr_data_1._dst_token).await?;
                let src_decimals = src_coin.decimals();
                let dst_decimals = dst_coin.decimals();

                #[cfg(feature = "for-tests")]
                {
                    assert_ne!(src_decimals, 0);
                    assert_ne!(dst_decimals, 0);
                }

                lr_data_1.src_contract = Some(src_contract);
                lr_data_1.dst_contract = Some(dst_contract);
                lr_data_1.src_decimals = Some(src_decimals);
                lr_data_1.dst_decimals = Some(dst_decimals);
                lr_data_1.chain_id = dst_coin.chain_id();
            }
        }
        Ok(())
    }

    /// Query 1inch token_0/token_1 prices in series and estimate token_0/token_1 average price
    /// Assuming the calling code ensures that relation src_tokens : dst_tokens will never be M:N (but only 1:M or M:1)
    async fn query_lr_prices(ctx: &MmArc, lr_data_refs: Vec<&mut LrStepData>) -> MmResult<(), LrSwapError> {
        let mut prices_futs = vec![];
        let mut src_dst = vec![]; // TODO: use hash map
        for lr_data in lr_data_refs.iter() {
            let (src_contract, dst_contract, chain_id) = lr_data.get_chain_contract_info()?;
            // Run src / dst token price query:
            let query_params = CrossPriceParams::new(chain_id, src_contract, dst_contract)
                .with_granularity(Some(CROSS_PRICES_GRANULARITY))
                // We do not use the limit parameter to get multiple samples, because some periods may be missing (if there were no trades)
                .build_query_params()
                .map_mm_err()?;
            let url = PortfolioUrlBuilder::create_api_url_builder(ctx, PortfolioApiMethods::CrossPrices)
                .map_mm_err()?
                .with_query_params(query_params)
                .build()
                .map_mm_err()?;
            let fut = ApiClient::call_api::<CrossPricesSeries>(url);
            prices_futs.push(fut);
            src_dst.push((lr_data._src_token.clone(), lr_data._dst_token.clone()));
        }
        let prices_in_series = join_all(prices_futs).await.into_iter().map(|res| res.ok()); // set bad results to None to preserve prices_in_series length

        let quotes = src_dst
            .into_iter()
            .zip(prices_in_series)
            .map(|((src, dst), series)| {
                // Get src/dst price. NOTE: cross_prices return prices in ETH coins or token units (not in smallest units)
                let dst_price = cross_prices_close(series);
                ((src, dst), dst_price)
            })
            .collect::<HashMap<_, _>>();

        log_cross_prices(&quotes);
        LrSwapCandidates::update_lr_prices(lr_data_refs, quotes);
        Ok(())
    }

    /// Estimate the needed source amount for LR_0 step, by dividing the dst amount by the src/dst price.
    /// The dst_amount should be set at this point
    #[allow(clippy::result_large_err)]
    fn estimate_lr_0_source_amounts(&mut self) -> MmResult<(), LrSwapError> {
        for candidate in self.inner.iter_mut() {
            let Some(ref mut lr_data_0) = candidate.lr_data_0 else {
                log::debug!("estimate_lr_0_source_amounts no lr_data_0 skipping candidate");
                continue;
            };
            let Some(dst_amount) = lr_data_0.dst_amount else {
                log::debug!(
                    "estimate_lr_0_source_amounts no lr_data_0.dst_amount for {}/{} skipping candidate",
                    lr_data_0._src_token,
                    lr_data_0._dst_token
                );
                continue; // We may not calculate dst_amount if no cross prices were received at LR_1
            };
            let Some(ref lr_price) = lr_data_0.lr_price else {
                log::debug!(
                    "estimate_lr_0_source_amounts no lr_data_0.lr_price for {}/{} skipping candidate",
                    lr_data_0._src_token,
                    lr_data_0._dst_token
                );
                continue;
            };
            // Get in coin units
            // Note: cross_prices API price is returned in src_coin / dst_coin units
            let dst_amount = wei_to_coins_mm_number(dst_amount, lr_data_0.dst_decimals()?).map_mm_err()?;
            if let Some(src_amount) = &dst_amount.checked_div(lr_price) {
                lr_data_0.src_amount =
                    Some(wei_from_coins_mm_number(src_amount, lr_data_0.src_decimals()?).map_mm_err()?);
                log::debug!(
                    "estimate_lr_0_source_amounts maker_order.taker_ticker={} lr_price={} lr_data_0.src_amount={:?}",
                    candidate.maker_order.taker_ticker(),
                    lr_price.to_decimal(),
                    lr_data_0.src_amount.unwrap_or_default()
                );
            } else {
                log::debug!(
                    "estimate_lr_0_source_amounts bad division dst_amount {} on lr_price {}, skipping candidate",
                    dst_amount.to_decimal(),
                    lr_price.to_decimal()
                );
            }
        }
        Ok(())
    }

    /// Estimate the needed source amount for LR_1 step, by dividing the known dst amount by the src/dst price
    /// The dst amount is set by the User in the 'buy' coin units
    #[allow(clippy::result_large_err)]
    async fn estimate_lr_1_source_amounts_from_dest(
        &mut self,
        ctx: &MmArc,
        user_dst_amount: &MmNumber,
    ) -> MmResult<(), LrSwapError> {
        for candidate in self.inner.iter_mut() {
            if let Some(ref mut lr_data_1) = candidate.lr_data_1 {
                let Some(ref lr_price) = lr_data_1.lr_price else {
                    continue; // No LR provider price - skipping
                };
                // Get in coin units
                // Note: cross prices API price is returned in coin / coin units (not in smallest units))
                let dst_amount = wei_from_coins_mm_number(user_dst_amount, lr_data_1.dst_decimals()?).map_mm_err()?;
                let dst_amount = mm_number_from_u256(dst_amount);
                if let Some(src_amount) = &dst_amount.checked_div(lr_price) {
                    lr_data_1.src_amount =
                        Some(wei_from_coins_mm_number(src_amount, lr_data_1.src_decimals()?).map_mm_err()?);
                    log::debug!(
                        "estimate_lr_1_source_amounts_from_dest lr_data_1._src_token={} lr_price={} lr_data_1.src_amount={:?}",
                        lr_data_1._src_token,
                        lr_price.to_decimal(),
                        lr_data_1.src_amount
                    );
                }
            } else {
                let maker_coin = lp_coinfind_or_err(ctx, &candidate.maker_order.maker_ticker())
                    .await
                    .map_mm_err()?;
                let dst_amount = wei_from_coins_mm_number(user_dst_amount, maker_coin.decimals()).map_mm_err()?;
                candidate.atomic_swap_maker_amount = Some(dst_amount);
            }
        }
        Ok(())
    }

    /// Estimate the LR_1 source amount either from the destination amount from the LR_0 quote (if the LR_0 exists)
    /// or from the User's provided sell amount (in coins)
    /// For LR_0 we need to deduct dex fee
    async fn estimate_lr_1_source_amounts_from_lr_0(
        &mut self,
        ctx: &MmArc,
        user_sell_amount: &MmNumber,
    ) -> MmResult<(), LrSwapError> {
        for candidate in self.inner.iter_mut() {
            let taker_amount = if let Some(ref lr_data_0) = candidate.lr_data_0 {
                let Some(ref lr_swap_data) = lr_data_0.lr_swap_data else {
                    log::debug!(
                        "estimate_lr_1_source_amounts_from_lr_0 lr_data_0 {}/{} no swap_data skipping",
                        lr_data_0._src_token,
                        lr_data_0._dst_token
                    );
                    continue; // No LR provider quote - skip this candidate
                };
                let quote_dst_amount = U256::from_dec_str(&lr_swap_data.dst_amount)?; // Get the 'real' destination amount from the LR quote (not estimated)
                let est_dst_amount = candidate.lr_data_0.as_ref().and_then(|lr_data_0| lr_data_0.dst_amount);
                log::debug!("estimate_lr_1_source_amounts_from_lr_0 lr_data_0 {}/{} quote_dst_amount={quote_dst_amount} est_dst_amount={est_dst_amount:?}", 
                    lr_data_0._src_token, lr_data_0._dst_token);
                let volume_with_fees =
                    wei_to_coins_mm_number(quote_dst_amount, lr_data_0.dst_decimals()?).map_mm_err()?;
                let maker_ticker = candidate.maker_order.maker_ticker();
                let taker_ticker = candidate.maker_order.taker_ticker();
                let dex_fee_rate = DexFee::dex_fee_rate(&taker_ticker, &maker_ticker);
                volume_with_fees / (MmNumber::from("1") + dex_fee_rate) // Deduct dex fee to get the atomic swap taker amount
            } else {
                user_sell_amount.clone() // TODO: use atomic_swap_taker_amount
            };
            // Get maker amount from taker amount
            let maker_ticker = &candidate.maker_order.maker_ticker();
            let maker_coin = lp_coinfind_or_err(ctx, maker_ticker).await.map_mm_err()?;
            let maker_sell_price: MmNumber = candidate.maker_order.sell_price(); // In maker/taker units
            let Some(maker_amount) = &taker_amount.checked_div(&maker_sell_price) else {
                continue;
            };
            let maker_amount = wei_from_coins_mm_number(maker_amount, maker_coin.decimals()).map_mm_err()?;
            if let Some(ref mut lr_data_1) = candidate.lr_data_1 {
                lr_data_1.src_amount = Some(maker_amount);
            } else {
                candidate.atomic_swap_maker_amount = Some(maker_amount); // if no LR_1, store as atomic_swap maker_amount for total price calc
            }
            log::debug!(
                "estimate_lr_1_source_amounts_from_lr_0 maker_ticker={} lr_data_1.src_token={:?} taker_amount={} maker_sell_price={} maker_coin.decimals()={} maker_amount={:?}",
                maker_ticker,
                candidate.lr_data_1.as_ref().map(|lr_data| &lr_data._src_token),
                taker_amount,
                maker_sell_price,
                maker_coin.decimals(),
                maker_amount
            );
        }
        Ok(())
    }

    /// Estimate dex and trade fees to do the atomic swap and add them to the source amount.
    /// The dex fee will be deducted from the destination amount (in proportion) when the atomic swap is running.
    #[allow(clippy::result_large_err)]
    async fn estimate_lr_0_fee_amounts(&mut self, ctx: &MmArc) -> MmResult<(), LrSwapError> {
        for candidate in self.inner.iter_mut() {
            let Some(ref lr_data_0) = candidate.lr_data_0 else {
                continue;
            };
            let Some(src_amount) = lr_data_0.src_amount else {
                continue; // No src_amount means LR provider did not send us cross prices, skipping this candidate
            };
            let atomic_swap_maker_token = candidate.maker_order.maker_ticker();
            let src_amount_mm_num = wei_to_coins_mm_number(src_amount, lr_data_0.src_decimals()?).map_mm_err()?;
            let src_coin = lp_coinfind_or_err(ctx, &lr_data_0._src_token).await.map_mm_err()?; // TODO: when I used get_coin_for_one_inch(), throwing a error if the order coin not EVM, 'lr_quote.rs' is lost in the error path. Why? dedup()?
            let dex_fee = DexFee::new_from_taker_coin(src_coin.deref(), &atomic_swap_maker_token, &src_amount_mm_num)
                .fee_amount(); // TODO: use simply DexFee::rate?
            let Some(ref mut lr_data_0) = candidate.lr_data_0 else {
                continue;
            };
            // Add dex fee to the amount
            //let src_amount_with_fees = LrStepData::add_fees_to_amount(src_amount_mm_num, src_token, &taker_swap_params)?;
            let src_amount_with_fees = &src_amount_mm_num + &dex_fee;
            log::debug!("estimate_lr_0_fee_amounts src_token={} src_amount_mm_num={src_amount_mm_num} dex_fee={dex_fee} src_amount_with_fees={src_amount_with_fees}", lr_data_0._src_token);
            lr_data_0.src_amount =
                Some(wei_from_coins_mm_number(&src_amount_with_fees, lr_data_0.src_decimals()?).map_mm_err()?);
            //lr_data_0.taker_swap_params = Some(taker_swap_params);
            lr_data_0.dex_fee = Some(dex_fee);
        }
        Ok(())
    }

    /// Run 1inch requests to get LR quotes to convert source tokens to tokens in orders
    async fn run_lr_0_quotes(&mut self, ctx: &MmArc) -> MmResult<(), LrSwapError> {
        let mut idx = vec![];
        let mut quote_futs = vec![];
        for candidate in self.inner.iter().enumerate() {
            let Some(ref lr_data_0) = candidate.1.lr_data_0 else {
                log::debug!("run_lr_0_quotes skipping candidate={} no lr_data_0", candidate.0);
                continue;
            };
            let Some(quote_fut) = create_quote_call(ctx, lr_data_0)? else {
                log::debug!(
                    "run_lr_0_quotes skipping candidate={} could not create quote",
                    candidate.0
                );
                continue;
            };
            // TODO: do not repeat 1inch calls for same pair:
            // let indexed_fut = async {
            //    (candidate.0, quote_fut.await.ok())
            // };
            quote_futs.push(quote_fut);
            idx.push(candidate.0);
        }
        let lr_quotes = join_all(quote_futs).await.into_iter().map(|res| res.ok()); // if a bad result received (for e.g. low liquidity) set to None to preserve swap_data length
        let lr_quotes_indexed = idx.into_iter().zip(lr_quotes).collect();
        self.update_lr_0_swap_data(lr_quotes_indexed);
        Ok(())
    }

    /// Run 1inch requests to get LR quotes to convert source tokens to tokens in orders
    async fn run_lr_1_quotes(&mut self, ctx: &MmArc) -> MmResult<(), LrSwapError> {
        let mut idx = vec![];
        let mut quote_futs = vec![];
        for candidate in self.inner.iter().enumerate() {
            let Some(ref lr_data_1) = candidate.1.lr_data_1 else {
                continue;
            };
            let Some(fut) = create_quote_call(ctx, lr_data_1)? else {
                continue;
            };
            // TODO: combine index in a future
            quote_futs.push(fut);
            idx.push(candidate.0);
        }
        let lr_quotes = join_all(quote_futs).await.into_iter().map(|res| res.ok()); // if a bad result received (for e.g. low liquidity) set to None to preserve swap_data length
        let lr_quotes_indexed = idx.into_iter().zip(lr_quotes).collect();
        self.update_lr_1_swap_data(lr_quotes_indexed);
        Ok(())
    }

    async fn check_order_limits(ctx: &MmArc, candidate: &LrSwapCandidateInfo) -> MmResult<bool, LrSwapError> {
        let atomic_swap_taker_amount = if let Some(ref lr_data_0) = candidate.lr_data_0 {
            let Some(ref lr_swap_data) = lr_data_0.lr_swap_data else {
                log::debug!(
                    "check_order_limits: {}/{} no LR_0 quote, skipping candidate",
                    lr_data_0._src_token,
                    lr_data_0._dst_token
                );
                return Ok(false); // No LR provider quote - skip this candidate
            };
            let quote_dst_amount = U256::from_dec_str(&lr_swap_data.dst_amount)?; // Get the 'real' destination amount from the LR quote (not estimated)
            wei_to_coins_mm_number(quote_dst_amount, lr_data_0.dst_decimals()?).map_mm_err()?
        } else {
            let atomic_swap_taker_amount = candidate
                .atomic_swap_taker_amount
                .ok_or(LrSwapError::InternalError("no atomic swap taker amount".to_owned()))?;
            let taker_coin = lp_coinfind_or_err(ctx, &candidate.maker_order.taker_ticker())
                .await
                .map_mm_err()?;
            wei_to_coins_mm_number(atomic_swap_taker_amount, taker_coin.decimals()).map_mm_err()?
        };
        log::debug!(
            "check_order_limits: {}/{} atomic_swap_taker_amount={} max_taker_vol={} min_taker_vol={}",
            candidate.maker_order.maker_ticker(),
            candidate.maker_order.taker_ticker(),
            atomic_swap_taker_amount.to_decimal(),
            candidate.maker_order.max_taker_vol().decimal,
            candidate.maker_order.min_taker_vol().decimal,
        );
        if atomic_swap_taker_amount.to_ratio() > candidate.maker_order.max_taker_vol().rational
            || atomic_swap_taker_amount.to_ratio() < candidate.maker_order.min_taker_vol().rational
        {
            log::debug!(
                "check_order_limits: {}/{} out of order min/max, skipping candidate",
                candidate.maker_order.maker_ticker(),
                candidate.maker_order.taker_ticker()
            );
            return Ok(false);
        }

        let atomic_swap_maker_amount = if let Some(ref lr_data_1) = candidate.lr_data_1 {
            wei_to_coins_mm_number(lr_data_1.src_amount()?, lr_data_1.src_decimals()?).map_mm_err()?
        } else {
            let atomic_swap_maker_amount = candidate
                .atomic_swap_maker_amount
                .ok_or(LrSwapError::InternalError("no atomic swap maker amount".to_owned()))?;
            let maker_coin = lp_coinfind_or_err(ctx, &candidate.maker_order.maker_ticker())
                .await
                .map_mm_err()?;
            wei_to_coins_mm_number(atomic_swap_maker_amount, maker_coin.decimals()).map_mm_err()?
        };
        log::debug!(
            "check_order_limits: {}/{} atomic_swap_maker_amount={} max_maker_vol={} min_maker_vol={}",
            candidate.maker_order.maker_ticker(),
            candidate.maker_order.taker_ticker(),
            atomic_swap_maker_amount.to_decimal(),
            candidate.maker_order.max_maker_vol().decimal,
            candidate.maker_order.min_maker_vol().decimal,
        );
        if atomic_swap_maker_amount.to_ratio() > candidate.maker_order.max_maker_vol().rational
            || atomic_swap_maker_amount.to_ratio() < candidate.maker_order.min_maker_vol().rational
        {
            log::debug!(
                "check_order_limits: {}/{} out of order min/max, skipping candidate",
                candidate.maker_order.maker_ticker(),
                candidate.maker_order.taker_ticker()
            );
            return Ok(false);
        }
        Ok(true)
    }

    /// Select the best swap path, by minimum of total swap price, including LR steps and atomic swap)
    #[allow(clippy::result_large_err)]
    async fn select_best_swap(self, ctx: &MmArc) -> MmResult<(LrSwapCandidateInfo, MmNumber), LrSwapError> {
        let mut best_price = None;
        let mut best_candidate = None;
        let candidate_len = self.inner.len();
        for candidate in self.inner {
            // For debug printing;
            let mut lr_0_src_token_print = None;
            let mut lr_0_dst_token_print = None;
            let taker_ticker_print = candidate.maker_order.taker_ticker();
            let maker_ticker_print = candidate.maker_order.maker_ticker();
            let mut lr_1_src_token_print = None;
            let mut lr_1_dst_token_print = None;

            if !LrSwapCandidates::check_order_limits(ctx, &candidate).await? {
                continue;
            }

            let sell_amount = if let Some(ref lr_data_0) = candidate.lr_data_0 {
                lr_0_src_token_print = Some(lr_data_0._src_token.clone());
                lr_0_dst_token_print = Some(lr_data_0._dst_token.clone());
                if lr_data_0.lr_swap_data.is_none() {
                    log::debug!("select_best_swap: no LR_0 quote for {lr_0_src_token_print:?}/{lr_0_dst_token_print:?}, skipping candidate");
                    continue; // No LR provider quote received - skip this candidate
                }
                // Exclude dex fee from the LR_0 src_amount for total_price calculation
                let volume_with_fees =
                    wei_to_coins_mm_number(lr_data_0.src_amount()?, lr_data_0.src_decimals()?).map_mm_err()?;
                let maker_ticker = candidate.maker_order.maker_ticker();
                let taker_ticker = candidate.maker_order.taker_ticker();
                let dex_fee_rate = DexFee::dex_fee_rate(&taker_ticker, &maker_ticker);
                volume_with_fees / (MmNumber::from("1") + dex_fee_rate)
            } else {
                let atomic_swap_taker_amount = candidate
                    .atomic_swap_taker_amount
                    .ok_or(LrSwapError::InternalError("no atomic swap taker amount".to_owned()))?;
                let taker_coin = lp_coinfind_or_err(ctx, &candidate.maker_order.taker_ticker())
                    .await
                    .map_mm_err()?;
                wei_to_coins_mm_number(atomic_swap_taker_amount, taker_coin.decimals()).map_mm_err()?
            };

            let buy_amount = if let Some(ref lr_data_1) = candidate.lr_data_1 {
                lr_1_src_token_print = Some(lr_data_1._src_token.clone());
                lr_1_dst_token_print = Some(lr_data_1._dst_token.clone());
                let Some(ref lr_swap_data) = lr_data_1.lr_swap_data else {
                    log::debug!("select_best_swap: no LR_1 quote for {lr_1_src_token_print:?}/{lr_1_dst_token_print:?}, skipping candidate");
                    continue; // No LR provider quote - skip this candidate
                };
                let quote_dst_amount = U256::from_dec_str(&lr_swap_data.dst_amount)?;
                wei_to_coins_mm_number(quote_dst_amount, lr_data_1.dst_decimals()?).map_mm_err()?
            } else {
                let atomic_swap_maker_amount = candidate
                    .atomic_swap_maker_amount
                    .ok_or(LrSwapError::InternalError("no atomic swap maker amount".to_owned()))?;
                let maker_coin = lp_coinfind_or_err(ctx, &candidate.maker_order.maker_ticker())
                    .await
                    .map_mm_err()?;
                wei_to_coins_mm_number(atomic_swap_maker_amount, maker_coin.decimals()).map_mm_err()?
            };

            log::debug!("select_best_swap: candidate: LR_0: {lr_0_src_token_print:?}/{lr_0_dst_token_print:?} Atomic swap: {taker_ticker_print}/{maker_ticker_print} LR_1: {lr_1_src_token_print:?}/{lr_1_dst_token_print:?} sell_amount={} buy_amount={} total_price={:?}",
                sell_amount.to_decimal(), buy_amount.to_decimal(), sell_amount.checked_div(&buy_amount).map(|amount| amount.to_decimal()));
            let Some(total_price) = sell_amount.checked_div(&buy_amount) else {
                log::debug!("select_best_swap: total_price is None, skipping candidate");
                continue;
            };
            if let Some(best_price_unwrapped) = best_price.as_ref() {
                if &total_price < best_price_unwrapped {
                    best_price = Some(total_price);
                    best_candidate = Some(candidate);
                }
            } else {
                best_price = Some(total_price);
                best_candidate = Some(candidate);
            }
        }

        if let Some(best_price) = best_price {
            let best_candidate = best_candidate.ok_or(LrSwapError::InternalError("no best_candidate".to_owned()))?;
            Ok((best_candidate, best_price))
        } else {
            MmError::err(LrSwapError::BestLrSwapNotFound {
                candidates: candidate_len as u32,
            })
        }
    }
}

/// Implementation code to find the optimal swap path (with the lowest total price) from the `user_base` coin to the `user_rel` coin
/// (`Aggregated taker swap` path).
/// This path includes:
/// - An atomic swap step: used to fill a specific ask or bid order provided in the parameters.
/// - A liquidity routing (LR) step before and/or after the atomic swap: converts `user_base` or `user_sell` into the coin in the order.
///
/// TODO: Note that in this function we request 1inch quotas (not swap details) so no slippage is applied for this.
/// When the actual aggregated swap is running we would create a new 1inch request for swap detail for these tokens
/// but the new price for them may be different and the estimated amount after the liquidity routing may deviate much
/// from the value needed to fill the atomic maker order (like User wanted this).
/// Maybe we should request for swap details here and this will allow to ensure slippage for the LR amount which we return here.
///
/// TODO: it's not only the slippage problem though. We try to estimate the needed source amount by querying the OHLC price and
/// this may also add error to the error from the slippage. We should take this error into account too.
///
/// TODO: if we do LR_0 for a platform coin we should reserve some platform coin amount for fees on other swap steps.
pub async fn find_best_swap_path_with_lr(
    ctx: &MmArc,
    user_base: Ticker,
    user_rel: Ticker,
    action: &TakerAction,
    asks: Vec<AsksForCoin>,
    bids: Vec<BidsForCoin>,
    amount: &MmNumber,
) -> MmResult<
    (
        Option<ClassicSwapDataExt>,
        AskOrBidOrder,
        Option<MmNumber>,
        Option<ClassicSwapDataExt>,
        MmNumber,
    ),
    LrSwapError,
> {
    let mut candidates = LrSwapCandidates::new_with_orders(ctx, user_base, user_rel, action, asks, bids).await;
    candidates.set_contracts(ctx).await?;
    match action {
        TakerAction::Buy => {
            // Calculate amounts from the destination coin 'buy' amount (backwards)
            // Query src/dst price for LR_1 step (to estimate the source amount).
            LrSwapCandidates::query_lr_prices(ctx, candidates.iter_mut_lr_data_1().collect::<Vec<_>>()).await?;
            candidates.estimate_lr_1_source_amounts_from_dest(ctx, amount).await?;
            // Query src/dst price for LR_0 step (to estimate the source amount).
            // TODO: good to query prices for LR_0 and LR_1 in one join
            LrSwapCandidates::query_lr_prices(ctx, candidates.iter_mut_lr_data_0().collect::<Vec<_>>()).await?;
            candidates.calc_lr_0_destination_amounts(ctx, amount).await?;
            candidates.estimate_lr_0_source_amounts()?;
            candidates.estimate_lr_0_fee_amounts(ctx).await?;
            candidates.run_lr_0_quotes(ctx).await?;
            candidates
                .estimate_lr_1_source_amounts_from_lr_0(ctx, &MmNumber::from("0"))
                .await?; // Recalculate LR_1 src amount based on LR provider's quote
            candidates.run_lr_1_quotes(ctx).await?;
        },
        TakerAction::Sell => {
            // Calculate amounts starting from the source coin 'sell' amount (forwards)
            candidates.set_lr_0_src_amount(ctx, amount).await?;
            candidates.estimate_lr_0_fee_amounts(ctx).await?;
            candidates.run_lr_0_quotes(ctx).await?;
            candidates.estimate_lr_1_source_amounts_from_lr_0(ctx, amount).await?;
            candidates.run_lr_1_quotes(ctx).await?;
        },
    }
    let (best_candidate, best_price) = candidates.select_best_swap(ctx).await?;
    let atomic_swap_sell_volume = {
        if best_candidate.lr_data_0.is_none() && best_candidate.atomic_swap_taker_amount.is_none() {
            return MmError::err(LrSwapError::InternalError("no taker amount".to_owned()));
        }
        let taker_coin = lp_coinfind_or_err(ctx, &best_candidate.maker_order.taker_ticker())
            .await
            .map_mm_err()?;
        best_candidate
            .atomic_swap_taker_amount
            .map(|amount| wei_to_coins_mm_number(amount, taker_coin.decimals()))
            .transpose()
            .map_mm_err()?
    };
    let lr_data_ext_0 = best_candidate
        .lr_data_0
        .map(|lr_data| -> Result<_, LrSwapError> {
            Ok(ClassicSwapDataExt {
                src_amount: lr_data
                    .src_amount
                    .ok_or(LrSwapError::InternalError("no src_amount".to_owned()))?,
                api_details: lr_data
                    .lr_swap_data
                    .ok_or(LrSwapError::InternalError("no lr_swap_data".to_owned()))?,
                chain_id: lr_data
                    .chain_id
                    .ok_or(LrSwapError::InternalError("no chain_id".to_owned()))?,
            })
        })
        .transpose()?;
    let lr_data_ext_1 = best_candidate
        .lr_data_1
        .map(|lr_data| -> Result<_, LrSwapError> {
            Ok(ClassicSwapDataExt {
                src_amount: lr_data
                    .src_amount
                    .ok_or(LrSwapError::InternalError("no src_amount".to_owned()))?,
                api_details: lr_data
                    .lr_swap_data
                    .ok_or(LrSwapError::InternalError("no lr_swap_data".to_owned()))?,
                chain_id: lr_data
                    .chain_id
                    .ok_or(LrSwapError::InternalError("no chain_id".to_owned()))?,
            })
        })
        .transpose()?;
    Ok((
        lr_data_ext_0,
        best_candidate.maker_order,
        atomic_swap_sell_volume,
        lr_data_ext_1,
        best_price,
    ))
}

/// Process 1inch token cross_prices history and return average price for all history
#[allow(unused)]
fn cross_prices_average(series: Option<CrossPricesSeries>) -> Option<MmNumber> {
    let series = series?;
    if series.is_empty() {
        return None;
    }
    let total: MmNumber = series.iter().fold(MmNumber::from(0), |acc, price_data| {
        acc + MmNumber::from(&price_data.avg)
    });
    Some(total / MmNumber::from(series.len() as u64))
}

/// Get the latest close price from cross_prices history
fn cross_prices_close(series: Option<CrossPricesSeries>) -> Option<MmNumber> {
    let series = series?;
    if series.is_empty() {
        return None;
    }
    series.first().map(|p| p.close.clone().into())
}

fn log_cross_prices(prices: &HashMap<(Ticker, Ticker), Option<MmNumber>>) {
    for p in prices {
        log::debug!(
            "1inch cross_prices result(close)={:?} {:?}",
            p,
            p.1.clone().map(|v| v.to_decimal())
        );
    }
}

#[allow(clippy::result_large_err)]
fn create_quote_call<'a>(
    ctx: &MmArc,
    lr_data: &'a LrStepData,
) -> MmResult<Option<BoxFuture<'a, ClassicSwapDataResult>>, LrSwapError> {
    let Some(src_amount) = lr_data.src_amount else {
        return Ok(None);
    };
    let (src_contract, dst_contract, chain_id) = lr_data.get_chain_contract_info()?;
    let query_params = ClassicSwapQuoteCallBuilder::new(src_contract, dst_contract, src_amount.to_string())
        .with_include_tokens_info(Some(true))
        .with_include_gas(Some(true))
        .build_query_params()
        .map_mm_err()?;
    let url = SwapUrlBuilder::create_api_url_builder(ctx, chain_id, SwapApiMethods::ClassicSwapQuote)
        .map_mm_err()?
        .with_query_params(query_params)
        .build()
        .map_mm_err()?;
    let fut = ApiClient::call_api::<ClassicSwapData>(url);
    Ok(Some(Box::pin(fut)))
}
