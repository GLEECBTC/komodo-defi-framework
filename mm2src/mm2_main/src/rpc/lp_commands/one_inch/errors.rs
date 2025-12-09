use crate::lp_swap::CheckBalanceError;
use crate::lr_swap::lr_errors::LrSwapError;
use coins::{CoinFindError, NumConversError, TradePreimageError, UnexpectedDerivationMethod};
use common::{HttpStatusCode, StatusCode};
use derive_more::Display;
use ethereum_types::U256;
use mm2_number::BigDecimal;
use ser_error_derive::SerializeErrorType;
use serde::Serialize;
use trading_api::one_inch_api::errors::ApiClientError;

#[derive(Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum ApiIntegrationRpcError {
    NoSuchCoin {
        coin: String,
    },
    #[display(fmt = "EVM token needed")]
    CoinTypeError,
    #[display(fmt = "NFT not supported")]
    NftProtocolNotSupported,
    #[display(fmt = "Chain not supported")]
    ChainNotSupported,
    #[display(fmt = "Must be same chain")]
    DifferentChains,
    MyAddressError(String),
    ConversionError(String),
    #[allow(dead_code)] // TODO: disable when the find best quote PR merged
    NumberError(String),
    InvalidParam(String),
    #[display(fmt = "Internal error: no token info in params for liquidity routing")]
    NoLrTokenInfo,
    #[display(fmt = "Parameter {param} out of bounds, value: {value}, min: {min} max: {max}")]
    OutOfBounds {
        param: String,
        value: String,
        min: String,
        max: String,
    },
    #[display(fmt = "allowance not enough for 1inch contract, available: {allowance}, needed: {amount}")]
    OneInchAllowanceNotEnough {
        allowance: U256,
        amount: U256,
    },
    #[display(fmt = "1inch API error: {_0}")]
    OneInchError(ApiClientError),
    ApiDataError(String),
    InternalError(String),
    ResponseParseError(String),
    #[display(fmt = "Transaction error {_0}")]
    TransactionError(String),
    #[display(fmt = "Sign transaction error {_0}")]
    SignTransactionError(String),
    #[display(fmt = "best liquidity routing swap not found, candidates: {candidates}")]
    #[allow(dead_code)] // TODO: remove when PR 2545 merged
    BestLrSwapNotFound {
        candidates: u32,
    },
    #[display(
        fmt = "Not enough {coin} for swap: available {available}, required at least {required}, locked by swaps {locked_by_swaps:?}"
    )]
    NotSufficientBalance {
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
    #[display(fmt = "Transport error {_0}")]
    TransportError(String),
}

impl HttpStatusCode for ApiIntegrationRpcError {
    fn status_code(&self) -> StatusCode {
        match self {
            ApiIntegrationRpcError::NoSuchCoin { .. } => StatusCode::NOT_FOUND,
            ApiIntegrationRpcError::CoinTypeError
            | ApiIntegrationRpcError::NftProtocolNotSupported
            | ApiIntegrationRpcError::ChainNotSupported
            | ApiIntegrationRpcError::DifferentChains
            | ApiIntegrationRpcError::MyAddressError(_)
            | ApiIntegrationRpcError::ConversionError(_)
            | ApiIntegrationRpcError::NoLrTokenInfo
            | ApiIntegrationRpcError::InvalidParam(_)
            | ApiIntegrationRpcError::OutOfBounds { .. }
            | ApiIntegrationRpcError::OneInchAllowanceNotEnough { .. }
            | ApiIntegrationRpcError::NumberError(_)
            | ApiIntegrationRpcError::BestLrSwapNotFound { .. }
            | ApiIntegrationRpcError::NotSufficientBalance { .. }
            | ApiIntegrationRpcError::VolumeTooLow { .. } => StatusCode::BAD_REQUEST,
            ApiIntegrationRpcError::OneInchError(_)
            | ApiIntegrationRpcError::ApiDataError(_)
            | ApiIntegrationRpcError::TransportError(_)
            | ApiIntegrationRpcError::TransactionError(_) => StatusCode::BAD_GATEWAY,
            ApiIntegrationRpcError::ResponseParseError(_)
            | ApiIntegrationRpcError::InternalError { .. }
            | ApiIntegrationRpcError::SignTransactionError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<ApiClientError> for ApiIntegrationRpcError {
    fn from(error: ApiClientError) -> Self {
        match error {
            ApiClientError::InvalidParam(error) => ApiIntegrationRpcError::InvalidParam(error),
            ApiClientError::OutOfBounds { param, value, min, max } => {
                ApiIntegrationRpcError::OutOfBounds { param, value, min, max }
            },
            ApiClientError::TransportError(_)
            | ApiClientError::ParseBodyError { .. }
            | ApiClientError::GeneralApiError { .. } => ApiIntegrationRpcError::OneInchError(error),
            ApiClientError::AllowanceNotEnough { allowance, amount, .. } => {
                ApiIntegrationRpcError::OneInchAllowanceNotEnough { allowance, amount }
            },
        }
    }
}

impl From<CoinFindError> for ApiIntegrationRpcError {
    fn from(err: CoinFindError) -> Self {
        match err {
            CoinFindError::NoSuchCoin { coin } => ApiIntegrationRpcError::NoSuchCoin { coin },
        }
    }
}

impl From<CheckBalanceError> for ApiIntegrationRpcError {
    fn from(err: CheckBalanceError) -> Self {
        match err {
            CheckBalanceError::NotSufficientBalance {
                coin,
                available,
                required,
                locked_by_swaps,
            } => Self::NotSufficientBalance {
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
            } => Self::NotSufficientBalance {
                coin,
                available,
                required,
                locked_by_swaps,
            },
            CheckBalanceError::VolumeTooLow {
                coin,
                volume,
                threshold,
            } => Self::VolumeTooLow {
                coin,
                volume,
                threshold,
            },
            CheckBalanceError::Transport(nested_err) => Self::TransportError(nested_err),
            CheckBalanceError::InternalError(nested_err) => Self::InternalError(nested_err),
        }
    }
}

impl From<TradePreimageError> for ApiIntegrationRpcError {
    fn from(err: TradePreimageError) -> Self {
        match err {
            TradePreimageError::AmountIsTooSmall { amount, threshold } => Self::VolumeTooLow {
                coin: "".to_owned(),
                volume: amount,
                threshold,
            },
            TradePreimageError::NotSufficientBalance {
                coin,
                available,
                required,
            } => Self::NotSufficientBalance {
                coin,
                available,
                required,
                locked_by_swaps: Default::default(),
            },
            TradePreimageError::Transport(nested_err) => Self::TransportError(nested_err),
            TradePreimageError::InternalError(nested_err) => Self::InternalError(nested_err),
            TradePreimageError::NftProtocolNotSupported => Self::NftProtocolNotSupported,
            TradePreimageError::NoSuchCoin { coin } => Self::NoSuchCoin { coin },
        }
    }
}

impl From<UnexpectedDerivationMethod> for ApiIntegrationRpcError {
    fn from(err: UnexpectedDerivationMethod) -> Self {
        Self::MyAddressError(err.to_string())
    }
}

impl From<NumConversError> for ApiIntegrationRpcError {
    fn from(err: NumConversError) -> Self {
        Self::ConversionError(err.to_string())
    }
}

impl From<ethereum_types::FromDecStrErr> for ApiIntegrationRpcError {
    fn from(err: ethereum_types::FromDecStrErr) -> Self {
        Self::ConversionError(err.to_string())
    }
}

/// Error aggregator for errors of conversion of api returned values
#[derive(Debug, Display, Serialize)]
pub(crate) struct FromApiValueError(String);

impl FromApiValueError {
    pub(crate) fn new(msg: String) -> Self {
        Self(msg)
    }
}

impl From<NumConversError> for FromApiValueError {
    fn from(err: NumConversError) -> Self {
        Self(err.to_string())
    }
}

impl From<primitive_types::Error> for FromApiValueError {
    fn from(err: primitive_types::Error) -> Self {
        Self(format!("{err:?}"))
    }
}

impl From<hex::FromHexError> for FromApiValueError {
    fn from(err: hex::FromHexError) -> Self {
        Self(err.to_string())
    }
}

impl From<ethereum_types::FromDecStrErr> for FromApiValueError {
    fn from(err: ethereum_types::FromDecStrErr) -> Self {
        Self(err.to_string())
    }
}

impl From<LrSwapError> for ApiIntegrationRpcError {
    fn from(err: LrSwapError) -> Self {
        match err {
            LrSwapError::NoSuchCoin { coin } => ApiIntegrationRpcError::NoSuchCoin { coin },
            LrSwapError::StateError(msg) | LrSwapError::AtomicSwapError(msg) | LrSwapError::InternalError(msg) => {
                ApiIntegrationRpcError::InternalError(msg)
            },
            LrSwapError::AtomicSwapAborted(abort_reason) => {
                ApiIntegrationRpcError::InternalError(abort_reason.to_string())
            },
            LrSwapError::CoinTypeError => ApiIntegrationRpcError::CoinTypeError,
            LrSwapError::NftProtocolNotSupported => ApiIntegrationRpcError::NftProtocolNotSupported,
            LrSwapError::ChainNotSupported => ApiIntegrationRpcError::ChainNotSupported,
            LrSwapError::DifferentChains => ApiIntegrationRpcError::DifferentChains,
            LrSwapError::InvalidParam(msg) => ApiIntegrationRpcError::InvalidParam(msg),
            LrSwapError::MyAddressError(msg) => ApiIntegrationRpcError::MyAddressError(msg),
            LrSwapError::BestLrSwapNotFound { candidates } => ApiIntegrationRpcError::BestLrSwapNotFound { candidates },
            LrSwapError::OutOfBounds { param, value, min, max } => {
                ApiIntegrationRpcError::OutOfBounds { param, value, min, max }
            },
            LrSwapError::OneInchAllowanceNotEnough { allowance, amount } => {
                ApiIntegrationRpcError::OneInchAllowanceNotEnough { allowance, amount }
            },
            LrSwapError::OneInchError(msg) => ApiIntegrationRpcError::OneInchError(msg),
            LrSwapError::ConversionError(msg) => ApiIntegrationRpcError::ConversionError(msg),
            LrSwapError::ResponseParseError(msg) => ApiIntegrationRpcError::ResponseParseError(msg),
            LrSwapError::TransactionError(msg) => ApiIntegrationRpcError::TransactionError(msg),
            LrSwapError::SignTransactionError(msg) => ApiIntegrationRpcError::SignTransactionError(msg),
            LrSwapError::NotSufficientBalance {
                coin,
                available,
                required,
                locked_by_swaps,
            } => ApiIntegrationRpcError::NotSufficientBalance {
                coin,
                available,
                required,
                locked_by_swaps,
            },
            LrSwapError::VolumeTooLow {
                coin,
                volume,
                threshold,
            } => ApiIntegrationRpcError::VolumeTooLow {
                coin,
                volume,
                threshold,
            },
            LrSwapError::TransportError(nested_err) => ApiIntegrationRpcError::TransportError(nested_err),
        }
    }
}
