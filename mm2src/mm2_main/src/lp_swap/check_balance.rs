use super::taker_swap::MaxTakerVolumeLessThanDust;
use super::{get_locked_amount, get_locked_amount_by_other_swaps};
use coins::{BalanceError, MmCoin, TradeFee, TradePreimageError};
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
    volume: MmNumber,
    mut trade_fee: TradeFee,
    taker_fee: Option<TakerFeeAdditionalInfo>,
) -> CheckBalanceResult<BigDecimal> {
    let ticker = coin.ticker();
    debug!("Check my_coin '{}' balance for swap", ticker);
    let balance: MmNumber = coin.my_spendable_balance().compat().await.map_mm_err()?.into();

    let locked = match swap_uuid {
        Some(u) => get_locked_amount_by_other_swaps(ctx, u, ticker),
        None => get_locked_amount(ctx, ticker),
    };

    let dex_fee = match taker_fee {
        Some(TakerFeeAdditionalInfo {
            dex_fee,
            fee_to_send_dex_fee,
        }) => {
            if fee_to_send_dex_fee.coin != trade_fee.coin {
                let err = format!(
                    "trade_fee {:?} and fee_to_send_dex_fee {:?} coins are expected to be the same",
                    trade_fee.coin, fee_to_send_dex_fee.coin
                );
                return MmError::err(CheckBalanceError::InternalError(err));
            }
            // increase `trade_fee` by the `fee_to_send_dex_fee`
            trade_fee.amount += fee_to_send_dex_fee.amount;
            dex_fee
        },
        None => MmNumber::from(0),
    };

    let total_trade_fee = if ticker == trade_fee.coin {
        trade_fee.amount
    } else {
        let platform_coin_balance: MmNumber = coin.platform_coin_balance().compat().await.map_mm_err()?.into();
        check_platform_coin_balance_for_swap(ctx, &platform_coin_balance, trade_fee, swap_uuid)
            .await
            .map_mm_err()?;
        MmNumber::from(0)
    };

    debug!(
        "my_coin: {} balance: {:?} ({}), locked: {:?} ({}), volume: {:?} ({}), total_trade_fee: {:?} ({}), dex_fee: {:?} ({}",
        ticker,
        balance.to_fraction(),
        balance.to_decimal(),
        locked.to_fraction(),
        locked.to_decimal(),
        volume.to_fraction(),
        volume.to_decimal(),
        total_trade_fee.to_fraction(),
        total_trade_fee.to_decimal(),
        dex_fee.to_fraction(),
        dex_fee.to_decimal()
    );

    let required = volume + total_trade_fee + dex_fee;
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
    if trade_fee.paid_from_trading_vol {
        return Ok(());
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

    if ticker == trade_fee.coin {
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
        let platform_coin_balance: MmNumber = coin.platform_coin_balance().compat().await.map_mm_err()?.into();
        check_platform_coin_balance_for_swap(ctx, &platform_coin_balance, trade_fee, swap_uuid)
            .await
            .map_mm_err()?;
    }

    Ok(())
}

pub async fn check_platform_coin_balance_for_swap(
    ctx: &MmArc,
    balance: &MmNumber,
    trade_fee: TradeFee,
    swap_uuid: Option<&Uuid>,
) -> CheckBalanceResult<()> {
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
    let available = balance - &locked;

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

pub struct TakerFeeAdditionalInfo {
    pub dex_fee: MmNumber,
    pub fee_to_send_dex_fee: TradeFee,
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
