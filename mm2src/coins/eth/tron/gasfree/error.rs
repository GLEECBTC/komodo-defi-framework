use common::HttpStatusCode;
use derive_more::Display;
use http::StatusCode;
use mm2_net::transport::SlurpError;
use ser_error_derive::SerializeErrorType;
use serde::Serialize;

/// Errors during GasFree activation-time configuration validation.
#[derive(Debug, Display)]
pub enum TronGaslessConfigError {
    #[display(fmt = "GasFree is only supported on TRON chains, got {chain}")]
    UnsupportedChain { chain: String },
    #[display(fmt = "Invalid GasFree base_url: {reason}")]
    InvalidBaseUrl { reason: String },
    #[display(fmt = "Invalid GasFree service_provider address: {reason}")]
    InvalidServiceProvider { reason: String },
}

/// Withdraw-level errors specific to TRON GasFree routing.
#[derive(Clone, Debug, Display, PartialEq, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum GaslessWithdrawError {
    #[display(fmt = "Gasless withdraw is unavailable for this coin or request")]
    Unavailable,
    #[display(fmt = "A gasless transfer is already pending; wait for settlement and retry")]
    PendingTransfer,
    #[display(fmt = "Gasless provider fee exceeds the requested maximum")]
    MaxFeeExceeded,
    #[display(fmt = "Gasless provider rejected the transfer")]
    ProviderRejected,
    #[display(fmt = "Gasless provider returned an invalid response")]
    InvalidProviderResponse,
    #[display(fmt = "Gasless transfer trace was not found")]
    TraceNotFound,
    #[display(fmt = "Gasless quote expired")]
    QuoteExpired,
}

impl HttpStatusCode for GaslessWithdrawError {
    fn status_code(&self) -> StatusCode {
        match self {
            GaslessWithdrawError::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
            GaslessWithdrawError::PendingTransfer => StatusCode::CONFLICT,
            GaslessWithdrawError::MaxFeeExceeded | GaslessWithdrawError::QuoteExpired => StatusCode::BAD_REQUEST,
            GaslessWithdrawError::ProviderRejected | GaslessWithdrawError::InvalidProviderResponse => {
                StatusCode::BAD_GATEWAY
            },
            GaslessWithdrawError::TraceNotFound => StatusCode::NOT_FOUND,
        }
    }
}

/// Errors during GasFree runtime HTTP/API interactions.
#[derive(Clone, Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum TronGasfreeError {
    #[display(fmt = "Invalid request: {_0}")]
    InvalidRequest(String),
    #[display(fmt = "Invalid response: {_0}")]
    InvalidResponse(String),
    #[display(fmt = "Transport: {_0}")]
    Transport(String),
    #[display(fmt = "Timeout: {_0}")]
    Timeout(String),
    #[display(fmt = "GasFree provider rejected the request: {_0}")]
    ProviderBadRequest(String),
    #[display(fmt = "GasFree authentication failed: {_0}")]
    Unauthorized(String),
    #[display(fmt = "GasFree access forbidden: {_0}")]
    Forbidden(String),
    #[display(fmt = "GasFree rate limit exceeded: {_0}")]
    RateLimited(String),
    #[display(fmt = "GasFree upstream error: {_0}")]
    Upstream(String),
    #[display(fmt = "Not implemented: {_0}")]
    NotImplemented(String),
    #[display(fmt = "Internal: {_0}")]
    Internal(String),
}

impl HttpStatusCode for TronGasfreeError {
    fn status_code(&self) -> StatusCode {
        match self {
            TronGasfreeError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            TronGasfreeError::Timeout(_) => StatusCode::GATEWAY_TIMEOUT,
            TronGasfreeError::Transport(_) | TronGasfreeError::InvalidResponse(_) | TronGasfreeError::Upstream(_) => {
                StatusCode::BAD_GATEWAY
            },
            TronGasfreeError::ProviderBadRequest(_) => StatusCode::BAD_REQUEST,
            TronGasfreeError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            TronGasfreeError::Forbidden(_) => StatusCode::FORBIDDEN,
            TronGasfreeError::RateLimited(_) => StatusCode::TOO_MANY_REQUESTS,
            TronGasfreeError::NotImplemented(_) => StatusCode::NOT_IMPLEMENTED,
            TronGasfreeError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<SlurpError> for TronGasfreeError {
    fn from(err: SlurpError) -> Self {
        let message = err.to_string();
        match err {
            SlurpError::InvalidRequest(_) => TronGasfreeError::InvalidRequest(message),
            SlurpError::ErrorDeserializing { .. } => TronGasfreeError::InvalidResponse(message),
            SlurpError::Timeout { .. } => TronGasfreeError::Timeout(message),
            SlurpError::Transport { .. } => TronGasfreeError::Transport(message),
            SlurpError::Internal(_) => TronGasfreeError::Internal(message),
        }
    }
}
