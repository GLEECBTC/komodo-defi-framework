//! Finding best quote to do swaps with liquidity routing (LR) support
//! Swaps with LR run additional interim swaps in EVM chains to convert one token into another token suitable to do a normal atomic swap.

use crate::lp_ordermatch::RpcOrderbookEntryV2;
use crate::rpc::lp_commands::lr_swap::types::{AskOrBidOrder, AsksForCoin, BidsForCoin};
use crate::rpc::lp_commands::one_inch::errors::ApiIntegrationRpcError;
use crate::rpc::lp_commands::one_inch::rpcs::get_coin_for_one_inch;
use coins::eth::{mm_number_from_u256, mm_number_to_u256, wei_from_coins_mm_number};
use coins::hd_wallet::DisplayAddress;
use coins::lp_coinfind_or_err;
use coins::MmCoin;
use coins::Ticker;
use common::log;
use ethereum_types::Address as EthAddress;
use ethereum_types::U256;
use futures::future::join_all;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::MmNumber;
use num_traits::CheckedDiv;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use trading_api::one_inch_api::classic_swap_types::{ClassicSwapData, ClassicSwapQuoteParams};
use trading_api::one_inch_api::client::{
    ApiClient, PortfolioApiMethods, PortfolioUrlBuilder, SwapApiMethods, SwapUrlBuilder,
};
use trading_api::one_inch_api::portfolio_types::{CrossPriceParams, CrossPricesSeries, DataGranularity};

/// To estimate src/dst price query price history for last 5 min
const CROSS_PRICES_GRANULARITY: DataGranularity = DataGranularity::FiveMin;
/// Use no more than 1 price history samples to estimate src/dst price
const CROSS_PRICES_LIMIT: u32 = 1;

/// Internal struct to collect data for LR swap step
#[allow(dead_code)] // 'Clone' is detected as dead code in one combinator
#[derive(Clone)]
struct LrStepData {
    /// Source coin or token ticker (to swap from)
    _src_token: Ticker,
    /// Source token contract address
    src_contract: Option<EthAddress>,
    /// Source token amount in wei
    src_amount: Option<U256>,
    /// Source token decimals
    src_decimals: Option<u8>,
    /// Destination coin or token ticker (to swap into)
    _dst_token: Ticker,
    /// Destination token contract address
    dst_contract: Option<EthAddress>,
    /// Destination token amount in wei
    dst_amount: Option<U256>,
    /// Destination token decimals
    dst_decimals: Option<u8>,
    /// Chain id where LR swap occurs (obtained from the destination token)
    chain_id: Option<u64>,
    /// Estimated src token / dst token price
    lr_price: Option<MmNumber>,
    /// A quote from LR provider with destination amount for LR swap step
    lr_swap_data: Option<ClassicSwapData>,
}

impl LrStepData {
    #[allow(clippy::result_large_err)]
    fn get_chain_contract_info(&self) -> MmResult<(String, String, u64), ApiIntegrationRpcError> {
        let src_contract = self
            .src_contract
            .as_ref()
            .ok_or(ApiIntegrationRpcError::InternalError(
                "Source LR contract not set".to_owned(),
            ))?
            .display_address();
        let dst_contract = self
            .dst_contract
            .as_ref()
            .ok_or(ApiIntegrationRpcError::InternalError(
                "Destination LR contract not set".to_owned(),
            ))?
            .display_address();
        let chain_id = self
            .chain_id
            .ok_or(ApiIntegrationRpcError::InternalError("LR chain id not set".to_owned()))?;
        Ok((src_contract, dst_contract, chain_id))
    }
}

struct LrSwapCandidateInfo {
    /// Data for liquidity routing before atomic swap
    lr_data_0: Option<LrStepData>,
    /// Atomic swap order to fill
    atomic_swap_order: AskOrBidOrder,
    /// Data for liquidity routing after atomic swap
    _lr_data_1: Option<LrStepData>,
}

/// Array to store data (possible swap route candidated, with prices for each step) needed for estimation
/// of the aggregated swap with liquidity routing, with the best total price
struct LrSwapCandidates {
    // The array of swaps with LR candidated is indexed by HashMaps with LR_0 and LR_1 base/rel pairs (to easily access and updated)
    // TODO: maybe this is overcomplicated and just a vector of candidates would be sufficicent
    inner0: HashMap<(Ticker, Ticker), Arc<RwLock<LrSwapCandidateInfo>>>,
    _inner1: HashMap<(Ticker, Ticker), Arc<RwLock<LrSwapCandidateInfo>>>,
}

impl LrSwapCandidates {
    /// Init LR data map from the source token (mytoken) and tokens from orders
    fn new_with_orders(src_token: Ticker, asks_coins: Vec<AsksForCoin>, _bids_coins: Vec<BidsForCoin>) -> Self {
        let mut inner0 = HashMap::new();
        let inner1 = HashMap::new();
        for asks_for_coin in asks_coins {
            for order in asks_for_coin.orders {
                let candidate = LrSwapCandidateInfo {
                    lr_data_0: Some(LrStepData {
                        _src_token: src_token.clone(),
                        src_contract: None,
                        src_decimals: None,
                        src_amount: None,
                        _dst_token: order.coin.clone(),
                        dst_contract: None,
                        dst_amount: None,
                        dst_decimals: None,
                        chain_id: None,
                        lr_price: None,
                        lr_swap_data: None,
                    }),
                    atomic_swap_order: AskOrBidOrder::Ask {
                        base: asks_for_coin.base.clone(),
                        order: order.clone(),
                    },
                    _lr_data_1: None, // TODO: add support for LR 1
                };
                let candidate = Arc::new(RwLock::new(candidate));
                inner0.insert((src_token.clone(), order.coin.clone()), candidate);
                // TODO: add support for inner1
            }
        }
        Self {
            inner0,
            _inner1: inner1,
        }
    }

    /// Calculate amounts of destination tokens required to fill ask orders for the requested base_amount:
    /// multiplies base_amount by the order price. Base_amount must be in coin units (with decimals)
    async fn calc_destination_token_amounts(
        &mut self,
        ctx: &MmArc,
        base_amount: &MmNumber,
    ) -> MmResult<(), ApiIntegrationRpcError> {
        for candidate in self.inner0.values_mut() {
            let order_ticker = candidate.read().unwrap().atomic_swap_order.order().coin.clone();
            let coin = lp_coinfind_or_err(ctx, &order_ticker).await.map_mm_err()?;
            let mut candidate_write = candidate.write().unwrap();
            let price: MmNumber = candidate_write.atomic_swap_order.order().price.rational.clone().into();
            let dst_amount = base_amount * &price;
            let Some(ref mut lr_data_0) = candidate_write.lr_data_0 else {
                continue;
            };
            let dst_amount = wei_from_coins_mm_number(&dst_amount, coin.decimals()).map_mm_err()?;
            lr_data_0.dst_amount = Some(dst_amount);
            log::debug!(
                "calc_destination_token_amounts atomic_swap_order.order.coin={} coin.decimals()={} lr_data_0.dst_amount={:?}",
                order_ticker,
                coin.decimals(),
                dst_amount
            );
        }
        Ok(())
    }

    fn update_with_lr_prices(&mut self, mut lr_prices: HashMap<(Ticker, Ticker), Option<MmNumber>>) {
        for (key, val) in self.inner0.iter_mut() {
            if let Some(ref mut lr_data_0) = val.write().unwrap().lr_data_0 {
                lr_data_0.lr_price = lr_prices.remove(key).flatten();
            }
        }
    }

    fn update_with_lr_swap_data(&mut self, mut lr_swap_data: HashMap<(Ticker, Ticker), Option<ClassicSwapData>>) {
        for (key, val) in self.inner0.iter_mut() {
            if let Some(ref mut lr_data_0) = val.write().unwrap().lr_data_0 {
                lr_data_0.lr_swap_data = lr_swap_data.remove(key).flatten();
            }
        }
    }

    async fn update_with_contracts(&mut self, ctx: &MmArc) -> MmResult<(), ApiIntegrationRpcError> {
        for ((src_token, dst_token), candidate) in self.inner0.iter_mut() {
            let (src_coin, src_contract) = get_coin_for_one_inch(ctx, src_token).await?;
            let (dst_coin, dst_contract) = get_coin_for_one_inch(ctx, dst_token).await?;
            let mut candidate_write = candidate.write().unwrap();
            let Some(ref mut lr_data_0) = candidate_write.lr_data_0 else {
                continue;
            };
            let src_decimals = src_coin.decimals();
            let dst_decimals = dst_coin.decimals();

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
        Ok(())
    }

    /// Query 1inch token_0/token_1 price in series and calc average price
    /// Assuming the outer RPC-level code ensures that relation src_tokens : dst_tokens will never be M:N (but only 1:M or M:1)
    async fn query_destination_token_prices(&mut self, ctx: &MmArc) -> MmResult<(), ApiIntegrationRpcError> {
        let mut prices_futs = vec![];
        let mut src_dst = vec![];
        for ((src_token, dst_token), candidate) in self.inner0.iter() {
            let candidate_read = candidate.read().unwrap();
            let Some(ref lr_data_0) = candidate_read.lr_data_0 else {
                continue;
            };
            let (src_contract, dst_contract, chain_id) = lr_data_0.get_chain_contract_info()?;
            // Run src / dst token price query:
            let query_params = CrossPriceParams::new(chain_id, src_contract, dst_contract)
                .with_granularity(Some(CROSS_PRICES_GRANULARITY))
                .with_limit(Some(CROSS_PRICES_LIMIT))
                .build_query_params()
                .map_mm_err()?;
            let url = PortfolioUrlBuilder::create_api_url_builder(ctx, PortfolioApiMethods::CrossPrices)
                .map_mm_err()?
                .with_query_params(query_params)
                .build()
                .map_mm_err()?;
            let fut = ApiClient::call_api::<CrossPricesSeries>(url);
            prices_futs.push(fut);
            src_dst.push((src_token.clone(), dst_token.clone()));
        }
        let prices_in_series = join_all(prices_futs).await.into_iter().map(|res| res.ok()); // set bad results to None to preserve prices_in_series length

        let quotes = src_dst
            .into_iter()
            .zip(prices_in_series)
            .map(|((src, dst), series)| {
                let dst_price = cross_prices_average(series);
                ((src, dst), dst_price)
            })
            .collect::<HashMap<_, _>>();

        log_cross_prices(&quotes);
        self.update_with_lr_prices(quotes);
        Ok(())
    }

    /// Estimate the needed source amount for LR swap, by dividing the known dst amount by the src/dst price
    #[allow(clippy::result_large_err)]
    fn estimate_source_token_amounts(&mut self) -> MmResult<(), ApiIntegrationRpcError> {
        for candidate in self.inner0.values_mut() {
            let order_ticker = candidate.read().unwrap().atomic_swap_order.order().coin.clone();
            let mut candidate_write = candidate.write().unwrap();
            let Some(ref mut lr_data_0) = candidate_write.lr_data_0 else {
                continue;
            };
            let Some(ref dst_price) = lr_data_0.lr_price else {
                continue;
            };
            let dst_amount = lr_data_0
                .dst_amount
                .ok_or(ApiIntegrationRpcError::InternalError("no dst_amount".to_owned()))?;
            let dst_amount = mm_number_from_u256(dst_amount);
            if let Some(src_amount) = &dst_amount.checked_div(dst_price) {
                lr_data_0.src_amount = Some(mm_number_to_u256(src_amount)?);
                log::debug!(
                    "estimate_source_token_amounts lr_data.order.coin={} dst_price={} lr_data.src_amount={:?}",
                    order_ticker,
                    dst_price.to_decimal(),
                    src_amount
                );
            }
        }
        Ok(())
    }

    /// Run 1inch requests to get LR quotes to convert source tokens to tokens in orders
    async fn run_lr_quotes(&mut self, ctx: &MmArc) -> MmResult<(), ApiIntegrationRpcError> {
        let mut src_dst = vec![];
        let mut quote_futs = vec![];
        for ((src_token, dst_token), candidate) in self.inner0.iter() {
            let candidate_read = candidate.read().unwrap();
            let Some(ref lr_data_0) = candidate_read.lr_data_0 else {
                continue;
            };
            let Some(src_amount) = lr_data_0.src_amount else {
                continue;
            };
            let (src_contract, dst_contract, chain_id) = lr_data_0.get_chain_contract_info()?;
            let query_params = ClassicSwapQuoteParams::new(src_contract, dst_contract, src_amount.to_string())
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
            quote_futs.push(fut);
            src_dst.push((src_token.clone(), dst_token.clone()));
        }
        let swap_data = join_all(quote_futs).await.into_iter().map(|res| res.ok()); // if a bad result received (for e.g. low liguidity) set to None to preserve swap_data length
        let swap_data_map = src_dst.into_iter().zip(swap_data).collect();
        self.update_with_lr_swap_data(swap_data_map);
        Ok(())
    }

    /// Select the best swap path, by minimum of total swap price (including order and LR swap)
    #[allow(clippy::result_large_err)]
    fn select_best_swap(&self) -> MmResult<(ClassicSwapData, AskOrBidOrder, MmNumber), ApiIntegrationRpcError> {
        // Calculate swap's total_price (filling the order plus LR swap) as src_amount / order_amount
        // where src_amount is user tokens to pay for the swap with LR, 'order_amount' is amount which will fill the order
        // Tx fee is not accounted here because it is in the platform coin, not token, so we can't compare LR swap tx fee directly here.
        // Instead, GUI may calculate and show to the user the total spendings for LR swap, including fees, in USD or other fiat currency
        let calc_total_price = |src_amount: U256, lr_swap: &ClassicSwapData, order: &RpcOrderbookEntryV2| {
            let src_amount = mm_number_from_u256(src_amount);
            let order_price = MmNumber::from(order.price.rational.clone());
            let dst_amount = MmNumber::from(lr_swap.dst_amount.as_str());
            let order_amount = dst_amount.checked_div(&order_price)?;
            let total_price = src_amount.checked_div(&order_amount);
            log::debug!("select_best_swap order.coin={} lr_swap.dst_amount(wei)={} order_amount(to fill order, wei)={} total_price(with LR)={}", 
                order.coin, lr_swap.dst_amount, order_amount.to_decimal(), total_price.clone().unwrap_or(MmNumber::from(0)).to_decimal());
            total_price
        };

        self.inner0
            .values()
            .filter_map(|candidate| {
                let candidate_read = candidate.read().unwrap();
                let atomic_swap_order = candidate_read.atomic_swap_order.clone();
                candidate_read
                    .lr_data_0
                    .as_ref()
                    .map(|lr_data_0| (atomic_swap_order, lr_data_0.clone()))
            })
            // filter out orders for which we did not get LR swap quotes and were not able to estimate needed source amount
            .filter_map(
                |(atomic_swap_order, lr_data_0)| match (lr_data_0.src_amount, lr_data_0.lr_swap_data) {
                    (Some(src_amount), Some(lr_swap_data)) => Some((src_amount, lr_swap_data, atomic_swap_order)),
                    (_, _) => None,
                },
            )
            // calculate total price and filter out orders for which we could not calculate the total price
            .filter_map(|(src_amount, lr_swap_data, order)| {
                calc_total_price(src_amount, &lr_swap_data, order.order())
                    .map(|total_price| (lr_swap_data, order, total_price))
            })
            .min_by(|(_, _, price_0), (_, _, price_1)| price_0.cmp(price_1))
            .ok_or(MmError::new(ApiIntegrationRpcError::BestLrSwapNotFound))
    }
}

/// Implementation code to find the optimal swap path (with the lowest total price) from the `user_base` coin to the `user_rel` coin
/// (`Aggregated taker swap` path).
/// This path includes:
/// - An atomic swap step: used to fill a specific ask (or, in future, bid) order provided in the parameters.
/// - A liquidity routing (LR) step before and/or after (todo) the atomic swap: converts `user_base` or `user_sell` into the coin in the order.
///
/// This function currently supports only:
/// - Ask orders and User 'sell' requests.
/// - Liquidity routing before the atomic swap.
///
/// TODO:
/// - Support bid orders and User 'buy' requests.
/// - Support liquidity routing after the atomic swap (e.g., to convert the output coin into `user_rel`).
pub async fn find_best_swap_path_with_lr(
    ctx: &MmArc,
    _user_base: Ticker,
    user_rel: Ticker,
    asks: Vec<AsksForCoin>,
    bids: Vec<BidsForCoin>,
    base_amount: &MmNumber,
) -> MmResult<(ClassicSwapData, AskOrBidOrder, MmNumber), ApiIntegrationRpcError> {
    let mut candidates = LrSwapCandidates::new_with_orders(user_rel, asks, bids);
    candidates.update_with_contracts(ctx).await?;
    candidates.calc_destination_token_amounts(ctx, base_amount).await?;
    candidates.query_destination_token_prices(ctx).await?;
    candidates.estimate_source_token_amounts()?;
    candidates.run_lr_quotes(ctx).await?;

    candidates.select_best_swap()
}

/// Helper to process 1inch token cross prices data and return average price
fn cross_prices_average(series: Option<CrossPricesSeries>) -> Option<MmNumber> {
    let series = series?;

    if series.is_empty() {
        return None;
    }

    let total: MmNumber = series.iter().fold(MmNumber::from(0), |acc, price_data| {
        acc + MmNumber::from(price_data.avg.clone())
    });
    Some(total / MmNumber::from(series.len() as u64))
}

fn log_cross_prices(prices: &HashMap<(Ticker, Ticker), Option<MmNumber>>) {
    for p in prices {
        log::debug!(
            "cross prices api src/dst price={:?} {:?}",
            p,
            p.1.clone().map(|v| v.to_decimal())
        );
    }
}
