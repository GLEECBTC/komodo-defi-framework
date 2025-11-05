use crate::utxo::rpc_clients::UtxoRpcError;
use crate::PrivKeyPolicyNotAllowed;
use common::executor::AbortedError;
use common::HttpStatusCode;
use db_common::sqlite::rusqlite::Error as SqlError;
use derive_more::Display;
use http::StatusCode;
use mm2_err_handle::prelude::*;
use rpc_task::RpcTaskError;
use std::num::TryFromIntError;
use uuid::Uuid;

pub type EnableLightningResult<T> = Result<T, MmError<EnableLightningError>>;
pub type SaveChannelClosingResult<T> = Result<T, MmError<SaveChannelClosingError>>;

#[derive(Clone, Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum EnableLightningError {
    #[display(fmt = "Invalid request: {_0}")]
    InvalidRequest(String),
    #[display(fmt = "Invalid configuration: {_0}")]
    InvalidConfiguration(String),
    #[display(fmt = "{_0} is only supported in {_1} mode")]
    UnsupportedMode(String, String),
    #[display(fmt = "I/O error {_0}")]
    IOError(String),
    #[display(fmt = "Invalid address: {_0}")]
    InvalidAddress(String),
    #[display(fmt = "Invalid path: {_0}")]
    InvalidPath(String),
    #[display(fmt = "Private key policy is not allowed: {_0}")]
    PrivKeyPolicyNotAllowed(PrivKeyPolicyNotAllowed),
    #[display(fmt = "System time error {_0}")]
    SystemTimeError(String),
    #[display(fmt = "RPC error {_0}")]
    RpcError(String),
    #[display(fmt = "DB error {_0}")]
    DbError(String),
    #[display(fmt = "Rpc task error: {_0}")]
    RpcTaskError(String),
    ConnectToNodeError(String),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl HttpStatusCode for EnableLightningError {
    fn status_code(&self) -> StatusCode {
        match self {
            EnableLightningError::InvalidRequest(_)
            | EnableLightningError::RpcError(_)
            | EnableLightningError::PrivKeyPolicyNotAllowed(_) => StatusCode::BAD_REQUEST,
            EnableLightningError::UnsupportedMode(_, _) => StatusCode::NOT_IMPLEMENTED,
            EnableLightningError::InvalidAddress(_)
            | EnableLightningError::InvalidPath(_)
            | EnableLightningError::SystemTimeError(_)
            | EnableLightningError::IOError(_)
            | EnableLightningError::ConnectToNodeError(_)
            | EnableLightningError::InvalidConfiguration(_)
            | EnableLightningError::DbError(_)
            | EnableLightningError::RpcTaskError(_)
            | EnableLightningError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<std::io::Error> for EnableLightningError {
    fn from(err: std::io::Error) -> EnableLightningError {
        EnableLightningError::IOError(err.to_string())
    }
}

impl From<SqlError> for EnableLightningError {
    fn from(err: SqlError) -> EnableLightningError {
        EnableLightningError::DbError(err.to_string())
    }
}

impl From<UtxoRpcError> for EnableLightningError {
    fn from(e: UtxoRpcError) -> Self {
        EnableLightningError::RpcError(e.to_string())
    }
}

impl From<PrivKeyPolicyNotAllowed> for EnableLightningError {
    fn from(e: PrivKeyPolicyNotAllowed) -> Self {
        EnableLightningError::PrivKeyPolicyNotAllowed(e)
    }
}

impl From<RpcTaskError> for EnableLightningError {
    fn from(e: RpcTaskError) -> Self {
        EnableLightningError::RpcTaskError(e.to_string())
    }
}

impl From<AbortedError> for EnableLightningError {
    fn from(e: AbortedError) -> Self {
        EnableLightningError::Internal(e.to_string())
    }
}

#[derive(Display, PartialEq)]
pub enum SaveChannelClosingError {
    #[display(fmt = "DB error: {_0}")]
    DbError(String),
    #[display(fmt = "Channel with uuid {_0} not found in DB")]
    ChannelNotFound(Uuid),
    #[display(fmt = "Funding transaction hash is Null in DB")]
    FundingTxNull,
    #[display(fmt = "Error parsing funding transaction hash: {_0}")]
    FundingTxParseError(String),
    #[display(fmt = "Error while waiting for the funding transaction to be spent: {_0}")]
    WaitForFundingTxSpendError(String),
    #[display(fmt = "Error while converting types: {_0}")]
    ConversionError(TryFromIntError),
}

impl From<SqlError> for SaveChannelClosingError {
    fn from(err: SqlError) -> SaveChannelClosingError {
        SaveChannelClosingError::DbError(err.to_string())
    }
}

impl From<TryFromIntError> for SaveChannelClosingError {
    fn from(err: TryFromIntError) -> SaveChannelClosingError {
        SaveChannelClosingError::ConversionError(err)
    }
}
