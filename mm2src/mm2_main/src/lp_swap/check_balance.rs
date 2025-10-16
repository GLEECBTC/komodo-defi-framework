use std::ops::Deref;

use super::maker_swap::LegacyMakerSwapTotalFeeHelper;
use super::maker_swap_v2::MakerSwapV2TotalFeeHelper;
use super::taker_swap::{LegacyTakerSwapTotalFeeHelper, MaxTakerVolumeLessThanDust};
use super::taker_swap_v2::TakerSwapV2TotalFeeHelper;
use super::{get_locked_amount, get_locked_amount_by_other_swaps};
use async_trait::async_trait;
use coins::{BalanceError, DexFee, FeeApproxStage, MarketCoinOps, MmCoin, MmCoinEnum, TradeFee, TradePreimageError};
use common::log::debug;
use derive_more::Display;
use futures::compat::Future01CompatExt;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::{BigDecimal, MmNumber};
use uuid::Uuid;

pub type CheckBalanceResult<T> = Result<T, MmError<CheckBalanceError>>;

/// Check the coin balance before the swap has started.
///
/// `swap_uuid` is used if our swap is running already and we should except this swap locked amount from the following calculations.
pub async fn check_my_coin_balance_for_swap(
    ctx: &MmArc,
    coin: &dyn MmCoin,
    swap_uuid: Option<&Uuid>,
    volume: Option<MmNumber>,
    dex_fee: Option<MmNumber>,
    total_trade_fee: Option<MmNumber>,
) -> CheckBalanceResult<BigDecimal> {
    let ticker = coin.ticker();
    debug!("Checking coin '{}' balance for swap...", ticker);
    let balance: MmNumber = coin.my_spendable_balance().compat().await.map_mm_err()?.into();

    let locked = match swap_uuid {
        Some(u) => get_locked_amount_by_other_swaps(ctx, u, ticker),
        None => get_locked_amount(ctx, ticker),
    };

    debug!(
        "coin {} balance {:?} ({:?}), locked {:?} ({:?}), volume {:?} ({:?}), total_trade_fee {:?} ({:?}), dex_fee {:?} ({:?})",
        ticker,
        balance.to_fraction(),
        balance.to_decimal(),
        locked.to_fraction(),
        locked.to_decimal(),
        volume.as_ref().map(|val| val.to_fraction()),
        volume.as_ref().map(|val| val.to_decimal()),
        total_trade_fee.as_ref().map(|val| val.to_fraction()),
        total_trade_fee.as_ref().map(|val| val.to_decimal()),
        dex_fee.as_ref().map(|val| val.to_fraction()),
        dex_fee.as_ref().map(|val| val.to_decimal()),
    );

    let required = volume.unwrap_or_default() + total_trade_fee.unwrap_or_default() + dex_fee.unwrap_or_default();
    let available = &balance - &locked;

    if available < required {
        return MmError::err(CheckBalanceError::NotSufficientBalance {
            coin: ticker.to_owned(),
            available: available.to_decimal(),
            required: required.to_decimal(),
            locked_by_swaps: Some(locked.to_decimal()),
        });
    }
    Ok(balance.into())
}

pub async fn check_other_coin_balance_for_swap(
    ctx: &MmArc,
    coin: &dyn MmCoin,
    swap_uuid: Option<&Uuid>,
    trade_fee: TradeFee,
) -> CheckBalanceResult<()> {
    macro_rules! is_platform_fee {
        ($ticker: expr, $trade_fee: expr) => {
            $ticker != $trade_fee.coin
        };
    }

    let ticker = coin.ticker();
    debug!(
        "Check other_coin '{}' balance for swap to pay trade fee, trade_fee coin {}",
        ticker, trade_fee.coin
    );
    let balance: MmNumber = coin.my_spendable_balance().compat().await.map_mm_err()?.into();

    let locked = match swap_uuid {
        Some(u) => get_locked_amount_by_other_swaps(ctx, u, ticker),
        None => get_locked_amount(ctx, ticker),
    };

    if !is_platform_fee!(ticker, trade_fee) {
        if trade_fee.paid_from_trading_vol {
            return Ok(());
        }
        let available = &balance - &locked;
        let required = trade_fee.amount;
        debug!(
            "other coin: {} balance: {:?} ({}), locked: {:?} ({}), required: {:?} ({})",
            ticker,
            balance.to_fraction(),
            balance.to_decimal(),
            locked.to_fraction(),
            locked.to_decimal(),
            required.to_fraction(),
            required.to_decimal(),
        );
        if available < required {
            return MmError::err(CheckBalanceError::NotSufficientBalance {
                coin: ticker.to_owned(),
                available: available.to_decimal(),
                required: required.to_decimal(),
                locked_by_swaps: Some(locked.to_decimal()),
            });
        }
    } else {
        check_platform_coin_balance_for_swap(ctx, coin, trade_fee, swap_uuid)
            .await
            .map_mm_err()?;
    }

    Ok(())
}

pub async fn check_platform_coin_balance_for_swap(
    ctx: &MmArc,
    coin: &(dyn MarketCoinOps + Sync),
    trade_fee: TradeFee,
    swap_uuid: Option<&Uuid>,
) -> CheckBalanceResult<()> {
    let balance = coin.platform_coin_balance().compat().await.map_mm_err()?;
    let balance = MmNumber::from(balance);
    let ticker = trade_fee.coin.as_str();
    debug!(
        "Check if the platform coin '{}' has sufficient balance to pay the trade fee {:?} ({})",
        ticker,
        trade_fee.amount.to_fraction(),
        trade_fee.amount.to_decimal()
    );

    let required = trade_fee.amount;
    let locked = match swap_uuid {
        Some(uuid) => get_locked_amount_by_other_swaps(ctx, uuid, ticker),
        None => get_locked_amount(ctx, ticker),
    };
    let available = &balance - &locked;

    debug!(
        "Platform coin: {} balance: {:?} ({}), locked: {:?} ({})",
        ticker,
        balance.to_fraction(),
        balance.to_decimal(),
        locked.to_fraction(),
        locked.to_decimal()
    );
    if available < required {
        MmError::err(CheckBalanceError::NotSufficientBaseCoinBalance {
            coin: ticker.to_owned(),
            available: available.to_decimal(),
            required: required.to_decimal(),
            locked_by_swaps: Some(locked.to_decimal()),
        })
    } else {
        Ok(())
    }
}

#[derive(Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum CheckBalanceError {
    #[display(
        fmt = "Not enough {coin} for swap: available {available}, required at least {required}, locked by swaps {locked_by_swaps:?}"
    )]
    NotSufficientBalance {
        coin: String,
        available: BigDecimal,
        required: BigDecimal,
        locked_by_swaps: Option<BigDecimal>,
    },
    #[display(
        fmt = "Not enough platform coin {coin} balance for swap: available {available}, required at least {required}, locked by swaps {locked_by_swaps:?}"
    )]
    NotSufficientBaseCoinBalance {
        coin: String,
        available: BigDecimal,
        required: BigDecimal,
        locked_by_swaps: Option<BigDecimal>,
    },
    #[display(fmt = "The volume {volume} of the {coin} coin less than minimum transaction amount {threshold}")]
    VolumeTooLow {
        coin: String,
        volume: BigDecimal,
        threshold: BigDecimal,
    },
    #[display(fmt = "Transport error: {_0}")]
    Transport(String),
    #[display(fmt = "Internal error: {_0}")]
    InternalError(String),
}

impl From<BalanceError> for CheckBalanceError {
    fn from(e: BalanceError) -> Self {
        match e {
            BalanceError::Transport(transport) | BalanceError::InvalidResponse(transport) => {
                CheckBalanceError::Transport(transport)
            },
            BalanceError::UnexpectedDerivationMethod(_) | BalanceError::WalletStorageError(_) => {
                CheckBalanceError::InternalError(e.to_string())
            },
            BalanceError::Internal(internal) => CheckBalanceError::InternalError(internal),
            BalanceError::NoSuchCoin { .. } => CheckBalanceError::InternalError(e.to_string()),
        }
    }
}

impl CheckBalanceError {
    pub fn not_sufficient_balance(&self) -> bool {
        matches!(
            self,
            CheckBalanceError::NotSufficientBalance { .. } | CheckBalanceError::NotSufficientBaseCoinBalance { .. }
        )
    }

    /// Construct [`CheckBalanceError`] from [`coins::TradePreimageError`] using the additional `ticker` argument.
    /// `ticker` is used to identify whether the `NotSufficientBalance` or `NotSufficientBaseCoinBalance` has occurred.
    pub fn from_trade_preimage_error(trade_preimage_err: TradePreimageError, ticker: &str) -> CheckBalanceError {
        match trade_preimage_err {
            TradePreimageError::NotSufficientBalance {
                coin,
                available,
                required,
            } => {
                if coin == ticker {
                    CheckBalanceError::NotSufficientBalance {
                        coin,
                        available,
                        locked_by_swaps: None,
                        required,
                    }
                } else {
                    CheckBalanceError::NotSufficientBaseCoinBalance {
                        coin,
                        available,
                        locked_by_swaps: None,
                        required,
                    }
                }
            },
            TradePreimageError::AmountIsTooSmall { amount, threshold } => CheckBalanceError::VolumeTooLow {
                coin: ticker.to_owned(),
                volume: amount,
                threshold,
            },
            TradePreimageError::Transport(transport) => CheckBalanceError::Transport(transport),
            TradePreimageError::InternalError(internal) => CheckBalanceError::InternalError(internal),
            TradePreimageError::NftProtocolNotSupported => {
                CheckBalanceError::InternalError("Nft Protocol is not supported yet!".to_string())
            },
            TradePreimageError::NoSuchCoin { .. } => CheckBalanceError::InternalError(trade_preimage_err.to_string()),
        }
    }

    pub fn from_max_taker_vol_error(
        max_vol_err: MaxTakerVolumeLessThanDust,
        coin: String,
        locked_by_swaps: BigDecimal,
    ) -> CheckBalanceError {
        CheckBalanceError::NotSufficientBalance {
            coin,
            available: max_vol_err.max_vol.to_decimal(),
            required: max_vol_err.min_tx_amount.to_decimal(),
            locked_by_swaps: Some(locked_by_swaps),
        }
    }
}

pub async fn check_coin_balances_for_swap(
    ctx: &MmArc,
    swap_uuid: Option<&Uuid>,
    total_fee_helper: &(dyn SwapTotalFeeHelper + Sync),
    upper_bound_volume: bool,
) -> CheckBalanceResult<BigDecimal> {
    let my_coin_fees = total_fee_helper.get_my_coin_fees(upper_bound_volume).await?;
    let other_coin_fees = total_fee_helper.get_other_coin_fees().await?;
    let balance = check_my_coin_balance_for_swap(
        ctx,
        total_fee_helper.get_my_coin(),
        swap_uuid,
        Some(total_fee_helper.get_my_coin_volume()),
        total_fee_helper.get_dex_fee(),
        if !total_fee_helper.is_my_platform_fee(&my_coin_fees) {
            Some(my_coin_fees.clone().amount)
        } else {
            None
        },
    )
    .await?;
    if total_fee_helper.is_my_platform_fee(&my_coin_fees) {
        check_platform_coin_balance_for_swap(ctx, total_fee_helper.get_my_coin(), my_coin_fees.clone(), swap_uuid)
            .await?;
    }
    check_other_coin_balance_for_swap(ctx, total_fee_helper.get_other_coin(), swap_uuid, other_coin_fees).await?;
    Ok(balance)
}

/// Helper to return total fees for legacy and TPU swaps, either for maker or taker,
/// to balance checking or calculating function
#[async_trait]
pub trait SwapTotalFeeHelper {
    /// Returns my coin object
    fn get_my_coin(&self) -> &dyn MmCoin;

    /// Returns my coin trade volume
    fn get_my_coin_volume(&self) -> MmNumber;

    /// Returns dex fee for taker or None
    fn get_dex_fee(&self) -> Option<MmNumber>;

    /// Estimates fees for the party trading my_coin
    async fn get_my_coin_fees(&self, upper_bound_amount: bool) -> CheckBalanceResult<TradeFee>;

    /// Returns for other coin object
    fn get_other_coin(&self) -> &dyn MmCoin;

    /// Estimates fees for the party trading other_coin
    async fn get_other_coin_fees(&self) -> CheckBalanceResult<TradeFee>;

    /// Returns true if fees for my_coin are paid in platfrom coin (different from my_coin)
    fn is_my_platform_fee(&self, fees: &TradeFee) -> bool {
        self.get_my_coin().ticker() != fees.coin
    }

    /// Returns true if fees for other_coin are paid in platfrom coin (different from other__coin)
    fn is_other_platform_fee(&self, fees: &TradeFee) -> bool {
        self.get_other_coin().ticker() != fees.coin
    }
}

#[allow(clippy::result_large_err)]
pub(crate) fn create_taker_total_fee_helper<'a>(
    ctx: &MmArc,
    my_coin: &'a MmCoinEnum,
    other_coin: &'a MmCoinEnum,
    volume: MmNumber,
    dex_fee: Option<DexFee>,
    stage: FeeApproxStage,
) -> CheckBalanceResult<Box<dyn SwapTotalFeeHelper + 'a + Sync + Send>> {
    let dex_fee = dex_fee.unwrap_or_else(|| DexFee::new_from_taker_coin(my_coin.deref(), other_coin.ticker(), &volume));
    // Note that if the remote side does not support TPU the swap will downgrade to legacy swap
    // so the total fee calculated by TakerSwapV2TotalFeeHelper may be not accurate
    if ctx.conf["use_trading_proto_v2"].as_bool().unwrap_or_default() {
        match (my_coin, other_coin) {
            (MmCoinEnum::UtxoCoin(my_coin), MmCoinEnum::UtxoCoin(other_coin)) => {
                Ok(Box::new(TakerSwapV2TotalFeeHelper {
                    my_coin: my_coin.clone(),
                    other_coin: other_coin.clone(),
                    volume,
                    dex_fee,
                    stage,
                }))
            },
            (MmCoinEnum::EthCoin(my_coin), MmCoinEnum::EthCoin(other_coin)) => {
                Ok(Box::new(TakerSwapV2TotalFeeHelper {
                    my_coin: my_coin.clone(),
                    other_coin: other_coin.clone(),
                    volume,
                    dex_fee,
                    stage,
                }))
            },
            (MmCoinEnum::UtxoCoin(my_coin), MmCoinEnum::EthCoin(other_coin)) => {
                Ok(Box::new(TakerSwapV2TotalFeeHelper {
                    my_coin: my_coin.clone(),
                    other_coin: other_coin.clone(),
                    volume,
                    dex_fee,
                    stage,
                }))
            },
            (MmCoinEnum::EthCoin(my_coin), MmCoinEnum::UtxoCoin(other_coin)) => {
                Ok(Box::new(TakerSwapV2TotalFeeHelper {
                    my_coin: my_coin.clone(),
                    other_coin: other_coin.clone(),
                    volume,
                    dex_fee,
                    stage,
                }))
            },
            (_, _) => MmError::err(CheckBalanceError::InternalError(
                "swap v2 not supported for this coin".to_string(),
            )),
        }
    } else {
        Ok(Box::new(LegacyTakerSwapTotalFeeHelper {
            my_coin: my_coin.deref(),
            other_coin: other_coin.deref(),
            volume,
            dex_fee,
            stage,
        }))
    }
}

#[allow(clippy::result_large_err)]
pub(crate) fn create_maker_total_fee_helper<'a>(
    ctx: &MmArc,
    my_coin: &'a MmCoinEnum,
    other_coin: &'a MmCoinEnum,
    volume: MmNumber,
    stage: FeeApproxStage,
) -> CheckBalanceResult<Box<dyn SwapTotalFeeHelper + 'a + Sync + Send>> {
    // Note that if the remote side does not support TPU the swap will downgrade to legacy swap
    // so the total fee calculated by TakerSwapV2TotalFeeHelper may be not accurate
    if ctx.conf["use_trading_proto_v2"].as_bool().unwrap_or_default() {
        match (my_coin, other_coin) {
            (MmCoinEnum::UtxoCoin(my_coin), MmCoinEnum::UtxoCoin(other_coin)) => {
                Ok(Box::new(MakerSwapV2TotalFeeHelper {
                    my_coin: my_coin.clone(),
                    other_coin: other_coin.clone(),
                    volume,
                    stage,
                }))
            },
            (MmCoinEnum::EthCoin(my_coin), MmCoinEnum::EthCoin(other_coin)) => {
                Ok(Box::new(MakerSwapV2TotalFeeHelper {
                    my_coin: my_coin.clone(),
                    other_coin: other_coin.clone(),
                    volume,
                    stage,
                }))
            },
            (MmCoinEnum::UtxoCoin(my_coin), MmCoinEnum::EthCoin(other_coin)) => {
                Ok(Box::new(MakerSwapV2TotalFeeHelper {
                    my_coin: my_coin.clone(),
                    other_coin: other_coin.clone(),
                    volume,
                    stage,
                }))
            },
            (MmCoinEnum::EthCoin(my_coin), MmCoinEnum::UtxoCoin(other_coin)) => {
                Ok(Box::new(MakerSwapV2TotalFeeHelper {
                    my_coin: my_coin.clone(),
                    other_coin: other_coin.clone(),
                    volume,
                    stage,
                }))
            },
            (_, _) => MmError::err(CheckBalanceError::InternalError(
                "swap v2 not supported for this coin".to_string(),
            )),
        }
    } else {
        Ok(Box::new(LegacyMakerSwapTotalFeeHelper {
            my_coin: my_coin.deref(),
            other_coin: other_coin.deref(),
            volume,
            stage,
        }))
    }
}
