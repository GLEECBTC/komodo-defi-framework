use crate::lp_swap::{get_max_maker_vol, CheckBalanceError, CoinVolumeInfo};
use coins::{lp_coinfind_or_err, CoinFindError};
use common::HttpStatusCode;
use derive_more::Display;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::{BigDecimal, MmNumberMultiRepr};
use ser_error_derive::SerializeErrorType;

#[derive(Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum MaxMakerVolRpcError {
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
    #[display(fmt = "Transport error: {_0}")]
    Transport(String),
    #[display(fmt = "Internal error: {_0}")]
    InternalError(String),
}

impl From<CoinFindError> for MaxMakerVolRpcError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => MaxMakerVolRpcError::NoSuchCoin { coin },
        }
    }
}

impl From<CheckBalanceError> for MaxMakerVolRpcError {
    fn from(e: CheckBalanceError) -> Self {
        match e {
            CheckBalanceError::NotSufficientBalance {
                coin,
                available,
                required,
                locked_by_swaps,
            } => MaxMakerVolRpcError::NotSufficientBalance {
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
            } => MaxMakerVolRpcError::NotSufficientBaseCoinBalance {
                coin,
                available,
                required,
                locked_by_swaps,
            },
            CheckBalanceError::VolumeTooLow {
                coin,
                volume,
                threshold,
            } => MaxMakerVolRpcError::VolumeTooLow {
                coin,
                volume,
                threshold,
            },
            CheckBalanceError::Transport(transport) => MaxMakerVolRpcError::Transport(transport),
            CheckBalanceError::InternalError(internal) => MaxMakerVolRpcError::InternalError(internal),
        }
    }
}

impl HttpStatusCode for MaxMakerVolRpcError {
    fn status_code(&self) -> StatusCode {
        match self {
            MaxMakerVolRpcError::NoSuchCoin { .. } => StatusCode::NOT_FOUND,
            MaxMakerVolRpcError::NotSufficientBalance { .. }
            | MaxMakerVolRpcError::NotSufficientBaseCoinBalance { .. }
            | MaxMakerVolRpcError::VolumeTooLow { .. } => StatusCode::BAD_REQUEST,
            MaxMakerVolRpcError::Transport(_) | MaxMakerVolRpcError::InternalError(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            },
        }
    }
}

#[derive(Deserialize)]
pub struct MaxMakerVolRequest {
    coin: String,
}

#[derive(Debug, Serialize)]
pub struct MaxMakerVolResponse {
    coin: String,
    volume: MmNumberMultiRepr,
    balance: MmNumberMultiRepr,
    locked_by_swaps: MmNumberMultiRepr,
}

pub async fn max_maker_vol(ctx: MmArc, req: MaxMakerVolRequest) -> MmResult<MaxMakerVolResponse, MaxMakerVolRpcError> {
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;
    let CoinVolumeInfo {
        volume,
        balance,
        locked_by_swaps,
    } = get_max_maker_vol(&ctx, &coin).await.map_mm_err()?;

    Ok(MaxMakerVolResponse {
        coin: req.coin,
        volume: MmNumberMultiRepr::from(volume),
        balance: MmNumberMultiRepr::from(balance),
        locked_by_swaps: MmNumberMultiRepr::from(locked_by_swaps),
    })
}
