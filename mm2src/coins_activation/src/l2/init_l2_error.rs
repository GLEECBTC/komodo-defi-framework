use crate::prelude::CoinConfWithProtocolError;
use common::{HttpStatusCode, StatusCode};
use derive_more::Display;
use rpc_task::rpc_common::{CancelRpcTaskError, RpcTaskStatusError, RpcTaskUserActionError};
use rpc_task::RpcTaskError;
use ser_error_derive::SerializeErrorType;
use serde_derive::Serialize;
use serde_json::Value as Json;
use std::time::Duration;

pub type InitL2StatusError = RpcTaskStatusError;
pub type InitL2UserActionError = RpcTaskUserActionError;
pub type CancelInitL2Error = CancelRpcTaskError;

#[derive(Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum InitL2Error {
    #[display(fmt = "Layer 2 {_0} is already activated")]
    L2IsAlreadyActivated(String),
    #[display(fmt = "Layer 2 {_0} config is not found")]
    L2ConfigIsNotFound(String),
    #[display(fmt = "Layer 2 {ticker} protocol parsing failed: {error}")]
    L2ProtocolParseError {
        ticker: String,
        error: String,
    },
    #[display(fmt = "Unexpected layer 2 protocol {protocol} for {ticker}")]
    UnexpectedL2Protocol {
        ticker: String,
        protocol: Json,
    },
    #[display(fmt = "Platform coin {_0} is not activated")]
    PlatformCoinIsNotActivated(String),
    #[display(fmt = "{platform_coin_ticker} is not a platform coin for layer 2 {l2_ticker}")]
    UnsupportedPlatformCoin {
        platform_coin_ticker: String,
        l2_ticker: String,
    },
    #[display(fmt = "Invalid config for platform coin: {platform_coin_ticker}, error: {err}")]
    InvalidPlatformConfiguration {
        platform_coin_ticker: String,
        err: String,
    },
    #[display(fmt = "Layer 2 configuration parsing failed: {_0}")]
    L2ConfigParseError(String),
    #[display(fmt = "Initialization task has timed out {duration:?}")]
    TaskTimedOut {
        duration: Duration,
    },
    Transport(String),
    Internal(String),
}

impl From<CoinConfWithProtocolError> for InitL2Error {
    fn from(err: CoinConfWithProtocolError) -> Self {
        match err {
            CoinConfWithProtocolError::ConfigIsNotFound(ticker) => InitL2Error::L2ConfigIsNotFound(ticker),
            CoinConfWithProtocolError::CoinProtocolParseError { ticker, err } => InitL2Error::L2ProtocolParseError {
                ticker,
                error: err.to_string(),
            },
            CoinConfWithProtocolError::UnexpectedProtocol { ticker, protocol } => {
                InitL2Error::UnexpectedL2Protocol { ticker, protocol }
            },
            CoinConfWithProtocolError::CustomTokenError(e) => {
                InitL2Error::Internal(format!("Custom tokens are not supported for L2: {e}"))
            },
        }
    }
}

impl From<RpcTaskError> for InitL2Error {
    fn from(rpc_err: RpcTaskError) -> Self {
        match rpc_err {
            RpcTaskError::Timeout(duration) => InitL2Error::TaskTimedOut { duration },
            internal_error => InitL2Error::Internal(internal_error.to_string()),
        }
    }
}

impl HttpStatusCode for InitL2Error {
    fn status_code(&self) -> StatusCode {
        match self {
            InitL2Error::L2IsAlreadyActivated(_)
            | InitL2Error::PlatformCoinIsNotActivated(_)
            | InitL2Error::L2ConfigIsNotFound { .. }
            | InitL2Error::UnexpectedL2Protocol { .. } => StatusCode::BAD_REQUEST,
            InitL2Error::TaskTimedOut { .. } => StatusCode::REQUEST_TIMEOUT,
            InitL2Error::L2ProtocolParseError { .. }
            | InitL2Error::UnsupportedPlatformCoin { .. }
            | InitL2Error::InvalidPlatformConfiguration { .. }
            | InitL2Error::L2ConfigParseError(_)
            | InitL2Error::Transport(_)
            | InitL2Error::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}
