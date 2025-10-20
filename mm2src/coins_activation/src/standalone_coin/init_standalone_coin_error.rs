use crate::prelude::CoinConfWithProtocolError;
use common::{HttpStatusCode, StatusCode};
use crypto::HwRpcError;
use derive_more::Display;
use rpc_task::rpc_common::{CancelRpcTaskError, RpcTaskStatusError, RpcTaskUserActionError};
use rpc_task::{RpcTaskError, TaskId};
use ser_error_derive::SerializeErrorType;
use serde_derive::Serialize;
use serde_json::Value as Json;
use std::time::Duration;

pub type InitStandaloneCoinStatusError = RpcTaskStatusError;
pub type InitStandaloneCoinUserActionError = RpcTaskUserActionError;
pub type CancelInitStandaloneCoinError = CancelRpcTaskError;

#[derive(Clone, Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum InitStandaloneCoinError {
    #[display(fmt = "No such task '{_0}'")]
    NoSuchTask(TaskId),
    #[display(fmt = "Initialization task has timed out {duration:?}")]
    TaskTimedOut { duration: Duration },
    #[display(fmt = "Coin {ticker} is activated already")]
    CoinIsAlreadyActivated { ticker: String },
    #[display(fmt = "Coin {_0} config is not found")]
    CoinConfigIsNotFound(String),
    #[display(fmt = "Coin {ticker} protocol parsing failed: {error}")]
    CoinProtocolParseError { ticker: String, error: String },
    #[display(fmt = "Unexpected platform protocol {protocol} for {ticker}")]
    UnexpectedCoinProtocol { ticker: String, protocol: Json },
    #[display(fmt = "Error on platform coin {ticker} creation: {error}")]
    CoinCreationError { ticker: String, error: String },
    #[display(fmt = "{_0}")]
    HwError(HwRpcError),
    #[display(fmt = "Transport error: {_0}")]
    Transport(String),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl From<CoinConfWithProtocolError> for InitStandaloneCoinError {
    fn from(e: CoinConfWithProtocolError) -> Self {
        match e {
            CoinConfWithProtocolError::ConfigIsNotFound(error) => InitStandaloneCoinError::CoinConfigIsNotFound(error),
            CoinConfWithProtocolError::CoinProtocolParseError { ticker, err } => {
                InitStandaloneCoinError::CoinProtocolParseError {
                    ticker,
                    error: err.to_string(),
                }
            },
            CoinConfWithProtocolError::UnexpectedProtocol { ticker, protocol } => {
                InitStandaloneCoinError::UnexpectedCoinProtocol { ticker, protocol }
            },
            CoinConfWithProtocolError::CustomTokenError(e) => {
                InitStandaloneCoinError::Internal(format!("Custom tokens are not supported for standalone coins: {e}"))
            },
        }
    }
}

impl From<RpcTaskError> for InitStandaloneCoinError {
    fn from(e: RpcTaskError) -> Self {
        match e {
            RpcTaskError::NoSuchTask(task_id) => InitStandaloneCoinError::NoSuchTask(task_id),
            RpcTaskError::Timeout(duration) => InitStandaloneCoinError::TaskTimedOut { duration },
            rpc_internal => InitStandaloneCoinError::Internal(rpc_internal.to_string()),
        }
    }
}

impl HttpStatusCode for InitStandaloneCoinError {
    fn status_code(&self) -> StatusCode {
        match self {
            InitStandaloneCoinError::NoSuchTask(_)
            | InitStandaloneCoinError::CoinIsAlreadyActivated { .. }
            | InitStandaloneCoinError::CoinConfigIsNotFound { .. }
            | InitStandaloneCoinError::CoinProtocolParseError { .. }
            | InitStandaloneCoinError::UnexpectedCoinProtocol { .. }
            | InitStandaloneCoinError::CoinCreationError { .. } => StatusCode::BAD_REQUEST,
            InitStandaloneCoinError::TaskTimedOut { .. } => StatusCode::REQUEST_TIMEOUT,
            InitStandaloneCoinError::HwError(_) => StatusCode::GONE,
            InitStandaloneCoinError::Transport(_) | InitStandaloneCoinError::Internal(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            },
        }
    }
}
