use super::check_balance::CheckBalanceError;
use super::{maker_swap_trade_preimage, taker_swap_trade_preimage, MakerTradePreimage, TakerTradePreimage};
use crate::lp_ordermatch::{MakerOrderBuildError, TakerOrderBuildError};
use coins::{is_wallet_only_ticker, lp_coinfind_or_err, BalanceError, CoinFindError, TradeFee, TradePreimageError};
use common::HttpStatusCode;
use crypto::CryptoCtxError;
use derive_more::Display;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::{construct_detailed, BigDecimal, MmNumber};
use mm2_rpc::data::legacy::TakerAction;
use std::collections::HashMap;

construct_detailed!(DetailedAmount, amount);
construct_detailed!(DetailedVolume, volume);
construct_detailed!(DetailedRequiredBalance, required_balance);

pub type TradePreimageRpcResult<T> = Result<T, MmError<TradePreimageRpcError>>;

/// Calculate all fees for the swap and max volume (for maker)
pub async fn trade_preimage_rpc(
    ctx: MmArc,
    req: TradePreimageRequest,
) -> TradePreimageRpcResult<TradePreimageResponse> {
    if is_wallet_only_ticker(&ctx, &req.base) {
        return MmError::err(TradePreimageRpcError::CoinIsWalletOnly { coin: req.base });
    }
    if is_wallet_only_ticker(&ctx, &req.rel) {
        return MmError::err(TradePreimageRpcError::CoinIsWalletOnly { coin: req.rel });
    }

    let base_coin = lp_coinfind_or_err(&ctx, &req.base).await.map_mm_err()?;
    let rel_coin = lp_coinfind_or_err(&ctx, &req.rel).await.map_mm_err()?;

    match req.swap_method {
        TradePreimageMethod::SetPrice => maker_swap_trade_preimage(&ctx, req, base_coin, rel_coin)
            .await
            .map(TradePreimageResponse::from),
        TradePreimageMethod::Buy | TradePreimageMethod::Sell => {
            taker_swap_trade_preimage(&ctx, req, base_coin, rel_coin)
                .await
                .map(TradePreimageResponse::from)
        },
    }
}

#[derive(Deserialize)]
pub struct TradePreimageRequest {
    /// The base currency of the request.
    pub base: String,
    /// The rel currency of the request.
    pub rel: String,
    /// The name of the method whose preimage is requested.
    pub swap_method: TradePreimageMethod,
    /// The price in `rel` the user is willing to receive per one unit of the `base` coin.
    pub price: MmNumber,
    /// The amount the user is willing to trade.
    /// Ignored if `max = true`.
    #[serde(default)]
    pub volume: MmNumber,
    /// Whether to return the maximum available volume for setprice method
    #[serde(default)]
    pub max: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TradePreimageMethod {
    SetPrice,
    Buy,
    Sell,
}

impl TradePreimageMethod {
    pub fn to_taker_action(&self) -> Result<TakerAction, String> {
        match self {
            TradePreimageMethod::SetPrice => Err("Expected 'sell' or 'buy' method".to_owned()),
            TradePreimageMethod::Sell => Ok(TakerAction::Sell),
            TradePreimageMethod::Buy => Ok(TakerAction::Buy),
        }
    }
}

#[derive(Serialize)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub enum TradePreimageResponse {
    MakerPreimage {
        base_coin_fee: TradeFeeResponse,
        rel_coin_fee: TradeFeeResponse,
        #[serde(skip_serializing_if = "Option::is_none")]
        #[serde(flatten)]
        volume: Option<DetailedVolume>,
        total_fees: Vec<TotalTradeFeeResponse>,
    },
    TakerPreimage {
        base_coin_fee: TradeFeeResponse,
        rel_coin_fee: TradeFeeResponse,
        taker_fee: TradeFeeResponse,
        fee_to_send_taker_fee: TradeFeeResponse,
        total_fees: Vec<TotalTradeFeeResponse>,
    },
}

impl From<MakerTradePreimage> for TradePreimageResponse {
    fn from(maker: MakerTradePreimage) -> Self {
        let mut total_fees = HashMap::new();

        TradePreimageResponse::accumulate_total_fees(&mut total_fees, maker.base_coin_fee.clone());
        let base_coin_fee = TradeFeeResponse::from(maker.base_coin_fee);

        TradePreimageResponse::accumulate_total_fees(&mut total_fees, maker.rel_coin_fee.clone());
        let rel_coin_fee = TradeFeeResponse::from(maker.rel_coin_fee);

        let total_fees = total_fees
            .into_iter()
            .filter_map(TradePreimageResponse::filter_zero_total_fees)
            .collect();
        let volume = maker.volume.map(DetailedVolume::from);
        TradePreimageResponse::MakerPreimage {
            base_coin_fee,
            rel_coin_fee,
            volume,
            total_fees,
        }
    }
}

impl From<TakerTradePreimage> for TradePreimageResponse {
    fn from(taker: TakerTradePreimage) -> Self {
        let mut total_fees = HashMap::new();

        TradePreimageResponse::accumulate_total_fees(&mut total_fees, taker.base_coin_fee.clone());
        let base_coin_fee = TradeFeeResponse::from(taker.base_coin_fee);

        TradePreimageResponse::accumulate_total_fees(&mut total_fees, taker.rel_coin_fee.clone());
        let rel_coin_fee = TradeFeeResponse::from(taker.rel_coin_fee);

        TradePreimageResponse::accumulate_total_fees(&mut total_fees, taker.taker_fee.clone());
        let taker_fee = TradeFeeResponse::from(taker.taker_fee);

        TradePreimageResponse::accumulate_total_fees(&mut total_fees, taker.fee_to_send_taker_fee.clone());
        let fee_to_send_taker_fee = TradeFeeResponse::from(taker.fee_to_send_taker_fee);

        let total_fees = total_fees
            .into_iter()
            .filter_map(TradePreimageResponse::filter_zero_total_fees)
            .collect();
        TradePreimageResponse::TakerPreimage {
            base_coin_fee,
            rel_coin_fee,
            taker_fee,
            fee_to_send_taker_fee,
            total_fees,
        }
    }
}

impl TradePreimageResponse {
    fn accumulate_total_fees(total_fees: &mut HashMap<String, TotalTradeFee>, fee: TradeFee) {
        use std::collections::hash_map::Entry;
        match total_fees.entry(fee.coin.clone()) {
            Entry::Occupied(mut entry) => {
                entry.get_mut().add_trade_fee(fee.amount, fee.paid_from_trading_vol);
            },
            Entry::Vacant(entry) => {
                entry.insert(fee.into());
            },
        }
    }

    fn filter_zero_total_fees((_coin, fee): (String, TotalTradeFee)) -> Option<TotalTradeFeeResponse> {
        if fee.amount.is_zero() {
            None
        } else {
            Some(TotalTradeFeeResponse::from(fee))
        }
    }
}

/// The extended `coins::TradePreimageError` error.
#[derive(Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum TradePreimageRpcError {
    #[display(
        fmt = "Not enough {coin} for swap: available {available}, required at least {required}, locked by swaps {locked_by_swaps:?}"
    )]
    NotSufficientBalance {
        coin: String,
        available: BigDecimal,
        required: BigDecimal,
        #[serde(skip_serializing_if = "Option::is_none")]
        locked_by_swaps: Option<BigDecimal>,
    },
    #[display(
        fmt = "Not enough platform coin {coin} balance for swap: available {available}, required at least {required}, locked by swaps {locked_by_swaps:?}"
    )]
    NotSufficientBaseCoinBalance {
        coin: String,
        available: BigDecimal,
        required: BigDecimal,
        #[serde(skip_serializing_if = "Option::is_none")]
        locked_by_swaps: Option<BigDecimal>,
    },
    #[display(fmt = "The volume {volume} of the {coin} coin less than minimum transaction amount {threshold}")]
    VolumeTooLow {
        coin: String,
        volume: BigDecimal,
        threshold: BigDecimal,
    },
    #[display(fmt = "No such coin {coin}")]
    NoSuchCoin { coin: String },
    #[display(fmt = "Coin {coin} is wallet only")]
    CoinIsWalletOnly { coin: String },
    #[display(fmt = "Rel coin can not be same as base")]
    BaseEqualRel,
    #[display(fmt = "Incorrect use of the '{param}' parameter: {reason}")]
    InvalidParam { param: String, reason: String },
    #[display(fmt = "Price {price} is too low, required at least {threshold}")]
    PriceTooLow { price: BigDecimal, threshold: BigDecimal },
    #[display(fmt = "Transport error: {_0}")]
    Transport(String),
    #[display(fmt = "Internal error: {_0}")]
    InternalError(String),
}

impl HttpStatusCode for TradePreimageRpcError {
    fn status_code(&self) -> StatusCode {
        match self {
            TradePreimageRpcError::NotSufficientBalance { .. }
            | TradePreimageRpcError::NotSufficientBaseCoinBalance { .. }
            | TradePreimageRpcError::VolumeTooLow { .. }
            | TradePreimageRpcError::NoSuchCoin { .. }
            | TradePreimageRpcError::CoinIsWalletOnly { .. }
            | TradePreimageRpcError::BaseEqualRel
            | TradePreimageRpcError::InvalidParam { .. }
            | TradePreimageRpcError::PriceTooLow { .. } => StatusCode::BAD_REQUEST,
            TradePreimageRpcError::Transport(_) | TradePreimageRpcError::InternalError(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            },
        }
    }
}

impl From<BalanceError> for TradePreimageRpcError {
    fn from(e: BalanceError) -> Self {
        match e {
            BalanceError::Transport(transport) | BalanceError::InvalidResponse(transport) => {
                TradePreimageRpcError::Transport(transport)
            },
            BalanceError::UnexpectedDerivationMethod(_) => TradePreimageRpcError::InternalError(e.to_string()),
            BalanceError::WalletStorageError(_) => TradePreimageRpcError::InternalError(e.to_string()),
            BalanceError::Internal(internal) => TradePreimageRpcError::InternalError(internal),
            BalanceError::NoSuchCoin { coin } => TradePreimageRpcError::NoSuchCoin { coin },
        }
    }
}

impl From<CheckBalanceError> for TradePreimageRpcError {
    fn from(e: CheckBalanceError) -> Self {
        match e {
            CheckBalanceError::NotSufficientBalance {
                coin,
                available,
                required,
                locked_by_swaps,
            } => TradePreimageRpcError::NotSufficientBalance {
                coin,
                available,
                required,
                locked_by_swaps,
            },
            CheckBalanceError::NotSufficientBaseCoinBalance {
                coin,
                available,
                required,
                locked_by_swaps,
            } => TradePreimageRpcError::NotSufficientBaseCoinBalance {
                coin,
                available,
                required,
                locked_by_swaps,
            },
            CheckBalanceError::VolumeTooLow {
                coin,
                volume,
                threshold,
            } => TradePreimageRpcError::VolumeTooLow {
                coin,
                volume,
                threshold,
            },
            CheckBalanceError::Transport(transport) => TradePreimageRpcError::Transport(transport),
            CheckBalanceError::InternalError(internal) => TradePreimageRpcError::InternalError(internal),
        }
    }
}

impl From<CoinFindError> for TradePreimageRpcError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => TradePreimageRpcError::NoSuchCoin { coin },
        }
    }
}

impl From<CryptoCtxError> for TradePreimageRpcError {
    fn from(e: CryptoCtxError) -> Self {
        TradePreimageRpcError::InternalError(e.to_string())
    }
}

impl TradePreimageRpcError {
    /// Construct [`TradePreimageRpcError`] from [`coins::TradePreimageError`] using the additional `ticker` argument.
    /// `ticker` is used to identify whether the `NotSufficientBalance` or `NotSufficientBaseCoinBalance` has occurred.
    pub fn from_trade_preimage_error(trade_preimage_err: TradePreimageError, ticker: &str) -> TradePreimageRpcError {
        // `CheckBalanceError` has similar variants as `TradePreimageRpcError` and can be obtained from `TradePreimageError`,
        // so avoid unnecessary boilerplate code.
        TradePreimageRpcError::from(CheckBalanceError::from_trade_preimage_error(trade_preimage_err, ticker))
    }

    pub fn from_maker_order_build_error(
        maker_order_err: MakerOrderBuildError,
        base: &str,
        rel: &str,
    ) -> TradePreimageRpcError {
        match maker_order_err {
            MakerOrderBuildError::BaseEqualRel => TradePreimageRpcError::BaseEqualRel,
            // TODO: this error is displayed as 'The volume .. of the .. coin less than minimum transaction amount <threshold>'
            // what may be thought as 'dust' but this is 'min order amount'
            // better fix display as 'minimum order amount'
            MakerOrderBuildError::MaxBaseVolTooLow { actual, threshold } => TradePreimageRpcError::VolumeTooLow {
                coin: base.to_owned(),
                volume: actual.to_decimal(),
                threshold: threshold.to_decimal(), // TODO: this is reported as 'The volume .. of the .. coin less than minimum transaction amount' what may be thought as dust ut this is 'min volume'
            },
            MakerOrderBuildError::PriceTooLow { actual, threshold } => TradePreimageRpcError::PriceTooLow {
                price: actual.to_decimal(),
                threshold: threshold.to_decimal(),
            },
            // TODO: this error is displayed as 'The volume .. of the .. coin less than minimum transaction amount <threshold>'
            // what may be thought as 'dust' but this is 'min order amount'
            // better fix display as 'minimum order amount'
            MakerOrderBuildError::RelVolTooLow { actual, threshold } => TradePreimageRpcError::VolumeTooLow {
                coin: rel.to_owned(),
                volume: actual.to_decimal(),
                threshold: threshold.to_decimal(),
            },
            // The errors below may occur due to invalid dummy params.
            error @ MakerOrderBuildError::MinBaseVolTooLow { .. }
            | error @ MakerOrderBuildError::ConfSettingsNotSet
            | error @ MakerOrderBuildError::MaxBaseVolBelowMinBaseVol { .. } => {
                TradePreimageRpcError::InternalError(format!("Unexpected MakerOrderBuildError: {error}"))
            },
        }
    }

    pub fn from_taker_order_build_error(
        taker_order_err: TakerOrderBuildError,
        base: &str,
        rel: &str,
    ) -> TradePreimageRpcError {
        match taker_order_err {
            TakerOrderBuildError::BaseEqualRel => TradePreimageRpcError::BaseEqualRel,
            TakerOrderBuildError::BaseAmountTooLow { actual, threshold } => TradePreimageRpcError::VolumeTooLow {
                coin: base.to_owned(),
                volume: actual.to_decimal(),
                threshold: threshold.to_decimal(),
            },
            TakerOrderBuildError::RelAmountTooLow { actual, threshold } => TradePreimageRpcError::VolumeTooLow {
                coin: rel.to_owned(),
                volume: actual.to_decimal(),
                threshold: threshold.to_decimal(),
            },
            // The errors below may occur due to invalid dummy params.
            error @ TakerOrderBuildError::MinVolumeTooLow { .. }
            | error @ TakerOrderBuildError::MaxBaseVolBelowMinBaseVol { .. }
            | error @ TakerOrderBuildError::SenderPubkeyIsZero
            | error @ TakerOrderBuildError::ConfsSettingsNotSet => {
                TradePreimageRpcError::InternalError(format!("Unexpected TakerOrderBuildError: {error}"))
            },
        }
    }
}

#[derive(Clone, Serialize)]
pub struct TradeFeeResponse {
    coin: String,
    #[serde(flatten)]
    amount: DetailedAmount,
    paid_from_trading_vol: bool,
}

impl From<TradeFee> for TradeFeeResponse {
    fn from(orig: TradeFee) -> Self {
        TradeFeeResponse {
            coin: orig.coin,
            amount: DetailedAmount::from(orig.amount),
            paid_from_trading_vol: orig.paid_from_trading_vol,
        }
    }
}

#[derive(Clone)]
struct TotalTradeFee {
    coin: String,
    amount: MmNumber,
    required_balance: MmNumber,
}

impl TotalTradeFee {
    fn add_trade_fee(&mut self, amount: MmNumber, paid_from_trading_vol: bool) {
        self.amount += &amount;
        if !paid_from_trading_vol {
            self.required_balance += amount;
        }
    }
}

impl From<TradeFee> for TotalTradeFee {
    fn from(orig: TradeFee) -> TotalTradeFee {
        let required_balance = if orig.paid_from_trading_vol {
            0.into()
        } else {
            orig.amount.clone()
        };
        TotalTradeFee {
            coin: orig.coin,
            amount: orig.amount,
            required_balance,
        }
    }
}

#[derive(Clone, Serialize)]
pub struct TotalTradeFeeResponse {
    coin: String,
    #[serde(flatten)]
    amount: DetailedAmount,
    #[serde(flatten)]
    required_balance: DetailedRequiredBalance,
}

impl From<TotalTradeFee> for TotalTradeFeeResponse {
    fn from(orig: TotalTradeFee) -> Self {
        TotalTradeFeeResponse {
            coin: orig.coin,
            amount: orig.amount.into(),
            required_balance: orig.required_balance.into(),
        }
    }
}
