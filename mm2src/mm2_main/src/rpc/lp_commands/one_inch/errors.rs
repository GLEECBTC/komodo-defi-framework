use coins::{CoinFindError, NumConversError};
use common::{HttpStatusCode, StatusCode};
use derive_more::Display;
use enum_derives::EnumFromStringify;
use ethereum_types::U256;
use ser_error_derive::SerializeErrorType;
use serde::Serialize;
use trading_api::one_inch_api::errors::ApiClientError;

#[derive(Debug, Display, Serialize, SerializeErrorType, EnumFromStringify)]
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
    #[from_stringify("coins::UnexpectedDerivationMethod")]
    MyAddressError(String),
    #[from_stringify("ethereum_types::FromDecStrErr", "coins::NumConversError")]
    NumberError(String),
    InvalidParam(String),
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
    #[display(fmt = "liquidity routing swap not found")]
    BestLrSwapNotFound,
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
            | ApiIntegrationRpcError::InvalidParam(_)
            | ApiIntegrationRpcError::OutOfBounds { .. }
            | ApiIntegrationRpcError::OneInchAllowanceNotEnough { .. }
            | ApiIntegrationRpcError::NumberError(_)
            | ApiIntegrationRpcError::BestLrSwapNotFound => StatusCode::BAD_REQUEST,
            ApiIntegrationRpcError::OneInchError(_) | ApiIntegrationRpcError::ApiDataError(_) => {
                StatusCode::BAD_GATEWAY
            },
            ApiIntegrationRpcError::InternalError { .. } => StatusCode::INTERNAL_SERVER_ERROR,
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
