use crate::lp_dispatcher::{dispatch_lp_event, DispatcherContext};
use crate::lp_ordermatch::lp_bot::{RunningState, StoppedState, StoppingState, TradingBotStarted, TradingBotStopped,
                                   TradingBotStopping, VolumeSettings};
use crate::lp_ordermatch::{cancel_all_orders, CancelBy, TradingBotEvent};
use crate::lp_swap::SavedSwap;
use crate::{lp_ordermatch::{cancel_order, create_maker_order,
                            lp_bot::{SimpleCoinMarketMakerCfg, SimpleMakerBotRegistry, TradingBotContext,
                                     TradingBotState},
                            update_maker_order, CancelOrderReq, MakerOrder, MakerOrderUpdateReq, OrdermatchContext,
                            SetPriceReq},
            lp_swap::{latest_swaps_for_pair, LatestSwapsErr}};
use coins::lp_price::{fetch_price_tickers, Provider, RateInfos, PRICE_ENDPOINTS};
use coins::{lp_coinfind, GetNonZeroBalance};
use common::{executor::{SpawnFuture, Timer},
             log::{debug, error, info, warn},
             Future01CompatExt, HttpStatusCode, StatusCode};
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::MmNumber;
use serde_json::Value as Json;
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

// !< constants
pub const BOT_DEFAULT_REFRESH_RATE: f64 = 30.0;
pub const PRECISION_FOR_NOTIFICATION: u64 = 8;
const LATEST_SWAPS_LIMIT: usize = 1000;

// !< Type definitions
pub type StartSimpleMakerBotResult = Result<StartSimpleMakerBotRes, MmError<StartSimpleMakerBotError>>;
pub type StopSimpleMakerBotResult = Result<StopSimpleMakerBotRes, MmError<StopSimpleMakerBotError>>;
pub type OrderProcessingResult = Result<bool, MmError<OrderProcessingError>>;
pub type VwapProcessingResult = Result<MmNumber, MmError<OrderProcessingError>>;
pub type OrderPreparationResult = Result<(Option<MmNumber>, MmNumber, MmNumber, bool), MmError<OrderProcessingError>>;

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum OrderProcessingError {
    #[display(fmt = "Rates from provider are Unknown - skipping for {}", key_trade_pair)]
    ProviderUnknown { key_trade_pair: String },
    #[display(fmt = "Price from provider is zero - skipping for {}", key_trade_pair)]
    PriceIsZero { key_trade_pair: String },
    #[display(fmt = "Last updated price timestamp is invalid - skipping for {}", key_trade_pair)]
    LastUpdatedTimestampInvalid { key_trade_pair: String },
    #[display(
        fmt = "Last updated price timestamp elapsed {} is more than the elapsed validity {} - skipping for {}",
        elapsed,
        elapsed_validity,
        key_trade_pair
    )]
    PriceElapsedValidityExpired {
        elapsed: f64,
        elapsed_validity: f64,
        key_trade_pair: String,
    },
    #[display(fmt = "Unable to parse/treat elapsed time {} - skipping", _0)]
    PriceElapsedValidityUntreatable(String),
    #[display(fmt = "Price of base coin {} is below min_base_price {}", base_price, min_base_price)]
    PriceBelowMinBasePrice { base_price: String, min_base_price: String },
    #[display(fmt = "Price of rel coin {} is below min_rel_price {}", rel_price, min_rel_price)]
    PriceBelowMinRelPrice { rel_price: String, min_rel_price: String },
    #[display(
        fmt = "Price of pair {} ({}) is below min_pair_price {}",
        pair,
        pair_price,
        min_pair_price
    )]
    PriceBelowPairPrice {
        pair: String,
        pair_price: String,
        min_pair_price: String,
    },

    #[display(fmt = "Asset not enabled - skipping")]
    AssetNotEnabled,
    #[display(fmt = "Internal coin find error - skipping")]
    InternalCoinFindError,
    #[display(fmt = "Internal error when retrieving balance - skipping")]
    BalanceInternalError,
    #[display(fmt = "Balance is zero - skipping")]
    BalanceIsZero,
    #[display(fmt = "{}", _0)]
    OrderCreationError(String),
    #[display(fmt = "{}", _0)]
    OrderUpdateError(String),
    #[display(fmt = "Error when querying swap history: {}", _0)]
    MyRecentSwapsError(String),
    #[display(fmt = "Base balance is less than the min_vol_usd - skipping")]
    MinVolUsdAboveBalanceUsd,
    #[display(fmt = "Legacy error - skipping")]
    LegacyError(String),
}

impl From<LatestSwapsErr> for OrderProcessingError {
    fn from(e: LatestSwapsErr) -> Self { OrderProcessingError::MyRecentSwapsError(format!("{}", e)) }
}

impl From<GetNonZeroBalance> for OrderProcessingError {
    fn from(err: GetNonZeroBalance) -> Self {
        match err {
            GetNonZeroBalance::MyBalanceError(_) => OrderProcessingError::BalanceInternalError,
            GetNonZeroBalance::BalanceIsZero => OrderProcessingError::BalanceIsZero,
        }
    }
}

impl From<std::string::String> for OrderProcessingError {
    fn from(error: std::string::String) -> Self { OrderProcessingError::LegacyError(error) }
}

#[derive(Deserialize)]
enum PriceSources {
    #[serde(rename = "price_url")]
    Singular(String),
    #[serde(rename = "price_urls")]
    Multiple(Vec<String>),
}

impl Default for PriceSources {
    fn default() -> Self { PriceSources::Multiple(PRICE_ENDPOINTS.iter().map(ToString::to_string).collect()) }
}

impl PriceSources {
    /// # Important
    ///
    /// Always use this to get the data
    fn get_urls(&self) -> Vec<String> {
        match self {
            // TODO: deprecate price_url soon and inform the users
            PriceSources::Singular(url) => vec![url.clone()],
            PriceSources::Multiple(urls) => urls.clone(),
        }
    }
}

#[derive(Deserialize)]
pub struct StartSimpleMakerBotRequest {
    cfg: SimpleMakerBotRegistry,
    // TODO: This is marked as an `Option` for now so we can be able to provide a default value for it since
    // `flatten` & `default` don't work together: https://github.com/serde-rs/serde/issues/1626.
    #[serde(flatten)]
    price_sources: Option<PriceSources>,
    bot_refresh_rate: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StopSimpleMakerBotRes {
    result: String,
}

impl StopSimpleMakerBotRes {
    #[allow(dead_code)]
    pub fn get_result(&self) -> String { self.result.clone() }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StartSimpleMakerBotRes {
    result: String,
}

impl StartSimpleMakerBotRes {
    #[allow(dead_code)]
    pub fn get_result(&self) -> String { self.result.clone() }
}

enum VwapSide {
    Base,
    Rel,
}

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum StopSimpleMakerBotError {
    #[display(fmt = "The bot is already stopped")]
    AlreadyStopped,
    #[display(fmt = "The bot is already stopping")]
    AlreadyStopping,
    #[display(fmt = "Transport error: {}", _0)]
    Transport(String),
    #[display(fmt = "Internal error: {}", _0)]
    InternalError(String),
}

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum StartSimpleMakerBotError {
    #[display(fmt = "The bot is already started")]
    AlreadyStarted,
    #[display(fmt = "Invalid bot configuration")]
    InvalidBotConfiguration,
    #[display(fmt = "Transport error: {}", _0)]
    Transport(String),
    #[display(fmt = "Cannot start the bot if it's currently stopping")]
    CannotStartFromStopping,
    #[display(fmt = "Internal error: {}", _0)]
    InternalError(String),
}

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum SwapUpdateNotificationError {
    #[display(fmt = "{}", _0)]
    MyRecentSwapsError(LatestSwapsErr),
    #[display(fmt = "Swap info not available")]
    SwapInfoNotAvailable,
}

impl From<LatestSwapsErr> for SwapUpdateNotificationError {
    fn from(e: LatestSwapsErr) -> Self { SwapUpdateNotificationError::MyRecentSwapsError(e) }
}

impl HttpStatusCode for StartSimpleMakerBotError {
    fn status_code(&self) -> StatusCode {
        match self {
            StartSimpleMakerBotError::AlreadyStarted
            | StartSimpleMakerBotError::InvalidBotConfiguration
            | StartSimpleMakerBotError::CannotStartFromStopping => StatusCode::BAD_REQUEST,
            StartSimpleMakerBotError::Transport(_) | StartSimpleMakerBotError::InternalError(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            },
        }
    }
}

impl HttpStatusCode for StopSimpleMakerBotError {
    fn status_code(&self) -> StatusCode {
        match self {
            // maybe bad request is not adapted for the first errors.
            StopSimpleMakerBotError::AlreadyStopped | StopSimpleMakerBotError::AlreadyStopping => {
                StatusCode::BAD_REQUEST
            },
            StopSimpleMakerBotError::Transport(_) | StopSimpleMakerBotError::InternalError(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            },
        }
    }
}

#[derive(Clone)]
struct TradingPair {
    base: String,
    rel: String,
}

impl TradingPair {
    pub fn new(base: String, rel: String) -> TradingPair { TradingPair { base, rel } }

    pub fn as_combination(&self) -> String { self.base.clone() + "/" + self.rel.clone().as_str() }
}

pub async fn tear_down_bot(ctx: MmArc) {
    let simple_market_maker_bot_ctx = TradingBotContext::from_ctx(&ctx).unwrap();
    let mut state = simple_market_maker_bot_ctx.trading_bot_states.lock().await;
    if let TradingBotState::Stopped(ref mut stopped_state) = *state {
        let nb_orders = cancel_pending_orders(&ctx, &stopped_state.trading_bot_cfg.clone()).await;
        let event: TradingBotEvent = TradingBotStopped { nb_orders }.into();
        dispatch_lp_event(ctx.clone(), event.into()).await;
        stopped_state.trading_bot_cfg.clear();
    }
}

fn sum_vwap(base_amount: &MmNumber, rel_amount: &MmNumber, total_volume: &mut MmNumber) -> MmNumber {
    let cur_price = base_amount / rel_amount;
    let cur_sum_price_volume = &cur_price * rel_amount;
    *total_volume += rel_amount;
    cur_sum_price_volume
}

fn vwap_calculation(kind: VwapSide, swaps: Vec<SavedSwap>, calculated_price: MmNumber) -> (MmNumber, i32) {
    let mut nb_trades_treated = 0;
    let mut total_sum_price_volume = MmNumber::default();
    let mut total_vol = MmNumber::default();
    for swap in swaps.iter() {
        if !swap.is_finished_and_success() {
            continue;
        }
        let (my_amount, other_amount) = match swap.get_my_info() {
            Some(x) => {
                nb_trades_treated += 1;
                (MmNumber::from(x.my_amount), MmNumber::from(x.other_amount))
            },
            None => continue,
        };
        let cur_sum_price_volume = match kind {
            VwapSide::Base => sum_vwap(&my_amount, &other_amount, &mut total_vol),
            VwapSide::Rel => sum_vwap(&other_amount, &my_amount, &mut total_vol),
        };
        total_sum_price_volume += cur_sum_price_volume;
    }
    if total_sum_price_volume.is_zero() {
        debug!("Unable to get average price from last trades - stick with calculated price");
        return (calculated_price, nb_trades_treated);
    }
    (total_sum_price_volume / total_vol, nb_trades_treated)
}

async fn vwap_logic(
    base_swaps: Vec<SavedSwap>,
    rel_swaps: Vec<SavedSwap>,
    calculated_price: MmNumber,
    cfg: &SimpleCoinMarketMakerCfg,
) -> MmNumber {
    let base_swaps_empty = base_swaps.is_empty();
    let rel_swaps_empty = rel_swaps.is_empty();
    let (base_vwap, nb_base_trades) = vwap_calculation(VwapSide::Rel, base_swaps, calculated_price.clone());
    let (rel_vwap, nb_rel_trades) = vwap_calculation(VwapSide::Base, rel_swaps, calculated_price.clone());
    let total_trades_treated = nb_base_trades + nb_rel_trades;
    if base_vwap == calculated_price && rel_vwap == calculated_price {
        return calculated_price;
    }
    let mut to_divide = 0;
    let mut total_vwap = MmNumber::default();
    if !base_swaps_empty {
        to_divide += 1;
        total_vwap += base_vwap;
    }
    if !rel_swaps_empty {
        to_divide += 1;
        total_vwap += rel_vwap;
    }
    // here divide cannot be 0 anymore because if both swaps history are empty we do not pass through this function.
    let vwap_price = total_vwap / MmNumber::from(to_divide);
    if vwap_price > calculated_price {
        debug!(
            "[{}/{}]: price: {} is less than average trading price ({} swaps): - using vwap price: {}",
            cfg.base, cfg.rel, calculated_price, total_trades_treated, vwap_price
        );
        return vwap_price;
    }
    debug!("price calculated by the CEX rates {} is above the vwap price ({} swaps) {} - skipping threshold readjustment for pair: [{}/{}]",
    calculated_price, total_trades_treated, vwap_price, cfg.base, cfg.rel);
    calculated_price
}

pub async fn vwap(
    base_swaps: Vec<SavedSwap>,
    rel_swaps: Vec<SavedSwap>,
    calculated_price: MmNumber,
    cfg: &SimpleCoinMarketMakerCfg,
) -> MmNumber {
    let is_equal_history_len = rel_swaps.len() == base_swaps.len();
    let have_precedent_swaps = !rel_swaps.is_empty() && !base_swaps.is_empty();
    if is_equal_history_len && !have_precedent_swaps {
        debug!(
            "No last trade for trading pair: [{}/{}] - keeping calculated price: {}",
            cfg.base, cfg.rel, calculated_price
        );
        return calculated_price;
    }
    vwap_logic(base_swaps, rel_swaps, calculated_price, cfg).await
}

async fn vwap_calculator(
    calculated_price: MmNumber,
    ctx: &MmArc,
    cfg: &SimpleCoinMarketMakerCfg,
) -> VwapProcessingResult {
    let base_swaps = latest_swaps_for_pair(ctx.clone(), cfg.base.clone(), cfg.rel.clone(), LATEST_SWAPS_LIMIT).await?;
    let rel_swaps = latest_swaps_for_pair(ctx.clone(), cfg.rel.clone(), cfg.base.clone(), LATEST_SWAPS_LIMIT).await?;
    Ok(vwap(base_swaps, rel_swaps, calculated_price, cfg).await)
}

async fn cancel_pending_orders(ctx: &MmArc, cfg_registry: &HashMap<String, SimpleCoinMarketMakerCfg>) -> usize {
    let mut nb_orders = 0;
    for (trading_pair, cfg) in cfg_registry.iter() {
        match cancel_all_orders(ctx.clone(), CancelBy::Pair {
            base: cfg.base.clone(),
            rel: cfg.rel.clone(),
        })
        .await
        {
            Ok(resp) => {
                info!(
                    "Successfully deleted orders: {:?} for pair: {}",
                    resp.cancelled, trading_pair
                );
                nb_orders += resp.cancelled.len();
            },
            Err(err) => error!("Couldn't cancel pending orders: {} for pair: {}", err, trading_pair),
        }
    }
    nb_orders
}

async fn cancel_single_order(ctx: &MmArc, uuid: Uuid) {
    match cancel_order(ctx.clone(), CancelOrderReq { uuid }).await {
        Ok(_) => info!("Order with uuid: {} successfully cancelled", uuid),
        Err(err) => warn!("Couldn't cancel the order with uuid: {} - err: {}", uuid, err),
    };
}

async fn checks_order_prerequisites(
    rates: &RateInfos,
    cfg: &SimpleCoinMarketMakerCfg,
    key_trade_pair: &str,
) -> OrderProcessingResult {
    if rates.base_provider == Provider::Unknown || rates.rel_provider == Provider::Unknown {
        return MmError::err(OrderProcessingError::ProviderUnknown {
            key_trade_pair: key_trade_pair.to_string(),
        });
    }

    if rates.price.is_zero() {
        return MmError::err(OrderProcessingError::PriceIsZero {
            key_trade_pair: key_trade_pair.to_string(),
        });
    }

    if let Some(min_base_price) = &cfg.min_base_price {
        if &rates.base_price < min_base_price {
            return MmError::err(OrderProcessingError::PriceBelowMinBasePrice {
                base_price: rates.base_price.to_string(),
                min_base_price: min_base_price.to_string(),
            });
        }
    }

    if let Some(rel_min_price) = &cfg.min_rel_price {
        if &rates.rel_price < rel_min_price {
            return MmError::err(OrderProcessingError::PriceBelowMinRelPrice {
                rel_price: rates.rel_price.to_string(),
                min_rel_price: rel_min_price.to_string(),
            });
        }
    }

    if let Some(min_pair_price) = &cfg.min_pair_price {
        if &rates.price < min_pair_price {
            return MmError::err(OrderProcessingError::PriceBelowPairPrice {
                pair: key_trade_pair.to_string(),
                pair_price: rates.price.to_string(),
                min_pair_price: min_pair_price.to_string(),
            });
        }
    }

    if rates.last_updated_timestamp.is_none() {
        return MmError::err(OrderProcessingError::LastUpdatedTimestampInvalid {
            key_trade_pair: key_trade_pair.to_string(),
        });
    }

    // Elapsed validity is the field defined in the cfg or 5 min by default (300 sec)
    let elapsed = rates.retrieve_elapsed_times();
    let elapsed_validity = cfg.price_elapsed_validity.unwrap_or(300.0);

    if elapsed > elapsed_validity {
        return MmError::err(OrderProcessingError::PriceElapsedValidityExpired {
            elapsed,
            elapsed_validity,
            key_trade_pair: key_trade_pair.to_string(),
        });
    }
    debug!("elapsed since last price update: {} secs", elapsed);
    Ok(true)
}

async fn prepare_order(
    rates: &RateInfos,
    cfg: &SimpleCoinMarketMakerCfg,
    key_trade_pair: &str,
    ctx: &MmArc,
) -> OrderPreparationResult {
    checks_order_prerequisites(rates, cfg, key_trade_pair).await?;
    let base_coin = lp_coinfind(ctx, cfg.base.as_str())
        .await?
        .ok_or_else(|| MmError::new(OrderProcessingError::AssetNotEnabled))?;
    let base_balance = base_coin.get_non_zero_balance().compat().await?;
    lp_coinfind(ctx, cfg.rel.as_str())
        .await?
        .ok_or_else(|| MmError::new(OrderProcessingError::AssetNotEnabled))?;

    debug!("balance for {} is {}", cfg.base, base_balance);

    let mut calculated_price = &rates.price * &cfg.spread;
    debug!("calculated price is: {}", calculated_price);
    if cfg.check_last_bidirectional_trade_thresh_hold.unwrap_or(false) {
        calculated_price = vwap_calculator(calculated_price.clone(), ctx, cfg).await?;
    }
    let mut is_max = cfg.max.unwrap_or(false);

    let volume = match &cfg.max_volume {
        Some(VolumeSettings::Percentage(balance_percent)) => {
            if *balance_percent >= MmNumber::from(1) {
                is_max = true;
                MmNumber::default()
            } else {
                balance_percent * &base_balance
            }
        },
        Some(VolumeSettings::Usd(max_volume_usd)) => {
            if &base_balance * &rates.base_price < *max_volume_usd {
                is_max = true;
                MmNumber::default()
            } else {
                max_volume_usd / &rates.base_price
            }
        },
        _ => MmNumber::default(),
    };

    let min_vol = match &cfg.min_volume {
        Some(VolumeSettings::Percentage(min_volume_percentage)) => {
            if is_max {
                Some(min_volume_percentage * &base_balance)
            } else {
                Some(min_volume_percentage * &volume)
            }
        },
        Some(VolumeSettings::Usd(min_volume_usd)) => {
            if &base_balance * &rates.base_price < *min_volume_usd {
                return MmError::err(OrderProcessingError::MinVolUsdAboveBalanceUsd);
            }
            Some(min_volume_usd / &rates.base_price)
        },
        None => None,
    };

    Ok((min_vol, volume, calculated_price, is_max))
}

async fn update_single_order(
    rates: &RateInfos,
    cfg: SimpleCoinMarketMakerCfg,
    uuid: Uuid,
    key_trade_pair: String,
    ctx: &MmArc,
) -> OrderProcessingResult {
    let (min_vol, volume, calculated_price, is_max) = prepare_order(rates, &cfg, &key_trade_pair, ctx).await?;
    let req = MakerOrderUpdateReq {
        uuid,
        new_price: Some(calculated_price.clone()),
        max: is_max.into(),
        volume_delta: None,
        min_volume: min_vol,
        base_confs: cfg.base_confs,
        base_nota: cfg.base_nota,
        rel_confs: cfg.rel_confs,
        rel_nota: cfg.rel_nota,
    };

    let resp = update_maker_order(ctx, req)
        .await
        .map_to_mm(OrderProcessingError::OrderUpdateError)?;

    let vol_info = if is_max {
        "max volume".to_string()
    } else {
        format!("volume: {:.8}", volume.to_decimal())
    };

    info!(
        "Successfully update order for {key_trade_pair} - uuid: {} - rate: ({:.8} {key_trade_pair}) - {vol_info}",
        resp.uuid,
        calculated_price.to_decimal()
    );

    Ok(true)
}

async fn execute_update_order(
    uuid: Uuid,
    order: MakerOrder,
    cloned_infos: (MmArc, RateInfos, TradingPair, SimpleCoinMarketMakerCfg),
) -> bool {
    let (ctx, rates, key_trade_pair, cfg) = cloned_infos;
    match update_single_order(&rates, cfg, uuid, key_trade_pair.as_combination(), &ctx).await {
        Ok(resp) => resp,
        Err(err) => {
            let pair = key_trade_pair.as_combination();
            error!(
                "Order with uuid: {} for {pair} cannot be updated - rate: ({:.8} {pair}) - err: {err:?}",
                order.uuid,
                rates.price.to_decimal(),
            );
            cancel_single_order(&ctx, order.uuid).await;
            false
        },
    }
}

async fn create_single_order(
    rates: &RateInfos,
    cfg: SimpleCoinMarketMakerCfg,
    key_trade_pair: String,
    ctx: MmArc,
) -> OrderProcessingResult {
    let (min_vol, volume, calculated_price, is_max) = prepare_order(rates, &cfg, &key_trade_pair, &ctx).await?;

    let req = SetPriceReq {
        base: cfg.base.clone(),
        rel: cfg.rel.clone(),
        price: calculated_price.clone(),
        max: is_max,
        volume: volume.clone(),
        min_volume: min_vol,
        cancel_previous: true,
        base_confs: cfg.base_confs,
        base_nota: cfg.base_nota,
        rel_confs: cfg.rel_confs,
        rel_nota: cfg.rel_nota,
        save_in_history: true,
    };

    let resp = create_maker_order(&ctx, req)
        .await
        .map_to_mm(OrderProcessingError::OrderUpdateError)?;
    let vol_info = if is_max {
        "max volume".to_string()
    } else {
        format!("volume: {:.8}", volume.to_decimal())
    };

    info!(
        "Successfully update order for {key_trade_pair} - uuid: {} - rate: ({:.8} {key_trade_pair}) - {vol_info}",
        resp.uuid,
        calculated_price.to_decimal()
    );

    Ok(true)
}

async fn execute_create_single_order(
    rates: RateInfos,
    cfg: SimpleCoinMarketMakerCfg,
    key_trade_pair: String,
    ctx: &MmArc,
) -> bool {
    match create_single_order(&rates, cfg, key_trade_pair.clone(), ctx.clone()).await {
        Ok(resp) => resp,
        Err(err) => {
            error!(
                "{err} - order cannot be created for: {key_trade_pair} - rate: ({:.8} {key_trade_pair}).",
                rates.price.to_decimal(),
            );
            false
        },
    }
}

async fn process_bot_logic(ctx: &MmArc) {
    let simple_market_maker_bot_ctx = TradingBotContext::from_ctx(ctx).unwrap();
    let mut state = simple_market_maker_bot_ctx.trading_bot_states.lock().await;
    let running_state = match &mut *state {
        TradingBotState::Running(running_state) => running_state,
        TradingBotState::Stopping(_) | TradingBotState::Stopped(_) => return,
    };

    let cfg = running_state.trading_bot_cfg.clone();
    let rates_registry = match fetch_price_tickers(&mut running_state.price_urls).await {
        Ok(model) => model,
        Err(err) => {
            let nb_orders = cancel_pending_orders(ctx, &cfg).await;
            error!("error fetching price: {err:?} - cancel {nb_orders} orders");
            return;
        },
    };

    drop(state);

    let mut memoization_pair_registry: HashSet<String> = HashSet::new();
    let ordermatch_ctx = OrdermatchContext::from_ctx(ctx).unwrap();
    let maker_orders = ordermatch_ctx.maker_orders_ctx.lock().orders.clone();
    let mut futures_order_update = Vec::with_capacity(maker_orders.len());
    for (uuid, order_mutex) in maker_orders {
        let order = order_mutex.lock().await;
        let key_trade_pair = TradingPair::new(order.base.clone(), order.rel.clone());

        if let Some(coin_cfg) = cfg.get(&key_trade_pair.as_combination()) {
            if !coin_cfg.enable {
                continue;
            }
            let cloned_infos = (
                ctx.clone(),
                rates_registry
                    .get_cex_rates(&coin_cfg.base, &coin_cfg.rel)
                    .unwrap_or_default(),
                key_trade_pair.clone(),
                coin_cfg.clone(),
            );
            futures_order_update.push(execute_update_order(uuid, order.clone(), cloned_infos));
            memoization_pair_registry.insert(key_trade_pair.as_combination());
        }
    }

    let _results_order_updates = futures::future::join_all(futures_order_update).await;

    let mut futures_order_creation = Vec::with_capacity(cfg.len());
    // Now iterate over the registry and for every pairs that are not hit let's create an order
    for (trading_pair, cur_cfg) in cfg {
        if memoization_pair_registry.get(&trading_pair).is_some() || !cur_cfg.enable {
            continue;
        }
        let rates_infos = rates_registry
            .get_cex_rates(&cur_cfg.base, &cur_cfg.rel)
            .unwrap_or_default();
        futures_order_creation.push(execute_create_single_order(
            rates_infos,
            cur_cfg,
            trading_pair.clone(),
            ctx,
        ));
    }
    let _results_order_creations = futures::future::join_all(futures_order_creation).await;
}

pub async fn lp_bot_loop(ctx: MmArc) {
    info!("lp_bot_loop successfully started");
    loop {
        debug!("tick lp_bot_loop");
        if ctx.is_stopping() {
            break;
        }
        let simple_market_maker_bot_ctx = TradingBotContext::from_ctx(&ctx).unwrap();
        let mut states = simple_market_maker_bot_ctx.trading_bot_states.lock().await;
        if let TradingBotState::Stopping(stopping_state) = &*states {
            *states = StoppedState {
                trading_bot_cfg: stopping_state.trading_bot_cfg.clone(),
            }
            .into();
            drop(states);
            tear_down_bot(ctx).await;
            break;
        }
        drop(states);
        let started = common::now_float();
        process_bot_logic(&ctx).await;
        let elapsed = common::now_float() - started;
        info!("bot logic processed in {} seconds", elapsed);
        let refresh_rate = simple_market_maker_bot_ctx.get_refresh_rate().await;
        Timer::sleep(refresh_rate).await;
    }
    info!("lp_bot_loop successfully stopped");
}

pub async fn start_simple_market_maker_bot(ctx: MmArc, req: StartSimpleMakerBotRequest) -> StartSimpleMakerBotResult {
    let simple_market_maker_bot_ctx = TradingBotContext::from_ctx(&ctx).unwrap();
    let mut state = simple_market_maker_bot_ctx.trading_bot_states.lock().await;
    match *state {
        TradingBotState::Running { .. } => MmError::err(StartSimpleMakerBotError::AlreadyStarted),
        TradingBotState::Stopping(_) => MmError::err(StartSimpleMakerBotError::CannotStartFromStopping),
        TradingBotState::Stopped(_) => {
            let dispatcher_ctx = DispatcherContext::from_ctx(&ctx).unwrap();
            let mut dispatcher = dispatcher_ctx.dispatcher.write().await;
            dispatcher.add_listener(simple_market_maker_bot_ctx.clone());
            let mut refresh_rate = req.bot_refresh_rate.unwrap_or(BOT_DEFAULT_REFRESH_RATE);
            if refresh_rate < BOT_DEFAULT_REFRESH_RATE {
                refresh_rate = BOT_DEFAULT_REFRESH_RATE;
            }
            let nb_pairs = req.cfg.len();
            *state = RunningState {
                trading_bot_cfg: req.cfg,
                bot_refresh_rate: refresh_rate,
                price_urls: req.price_sources.unwrap_or_default().get_urls(),
            }
            .into();
            drop(state);
            let event: TradingBotEvent = TradingBotStarted { nb_pairs }.into();
            dispatcher.dispatch_async(ctx.clone(), event.into()).await;
            ctx.spawner().spawn(lp_bot_loop(ctx.clone()));
            Ok(StartSimpleMakerBotRes {
                result: "Success".to_string(),
            })
        },
    }
}

pub async fn stop_simple_market_maker_bot(ctx: MmArc, _req: Json) -> StopSimpleMakerBotResult {
    let simple_market_maker_bot_ctx = TradingBotContext::from_ctx(&ctx).unwrap();
    let mut state = simple_market_maker_bot_ctx.trading_bot_states.lock().await;
    match &*state {
        TradingBotState::Stopped(_) => MmError::err(StopSimpleMakerBotError::AlreadyStopped),
        TradingBotState::Stopping(_) => MmError::err(StopSimpleMakerBotError::AlreadyStopping),
        TradingBotState::Running(running_state) => {
            let event: TradingBotEvent = TradingBotStopping {
                bot_refresh_rate: running_state.bot_refresh_rate,
            }
            .into();
            *state = StoppingState {
                trading_bot_cfg: running_state.trading_bot_cfg.clone(),
            }
            .into();
            drop(state);
            dispatch_lp_event(ctx.clone(), event.into()).await;
            Ok(StopSimpleMakerBotRes {
                result: "Success".to_string(),
            })
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{start_simple_market_maker_bot, stop_simple_market_maker_bot, StartSimpleMakerBotRequest};
    use common::block_on;
    use mm2_test_helpers::for_tests::mm_ctx_with_iguana;
    use serde_json::Value as Json;

    #[test]
    fn test_start_and_stop_simple_market_maker_bot_from_ctx() {
        let ctx = mm_ctx_with_iguana(Some(
            "also shoot benefit prefer juice shell elder veteran woman mimic image kidney",
        ));

        let cloned_ctx = ctx.clone();
        let another_cloned_ctx = ctx.clone();
        let req = StartSimpleMakerBotRequest {
            cfg: Default::default(),
            price_sources: Default::default(),
            bot_refresh_rate: None,
        };
        let answer = block_on(start_simple_market_maker_bot(ctx, req)).unwrap();
        assert_eq!(answer.get_result(), "Success");

        let req = StartSimpleMakerBotRequest {
            cfg: Default::default(),
            price_sources: Default::default(),
            bot_refresh_rate: None,
        };
        let answer = block_on(start_simple_market_maker_bot(cloned_ctx, req));
        assert!(answer.is_err());
        let answer = block_on(stop_simple_market_maker_bot(another_cloned_ctx, Json::default())).unwrap();
        assert_eq!(answer.get_result(), "Success");
    }
}
