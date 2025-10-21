use common::jsonrpc_client::JsonRpcErrorType;
use derive_more::Display;
use http::{HeaderMap, StatusCode};
use mm2_err_handle::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{Error, Value as Json};

#[cfg(not(target_arch = "wasm32"))]
pub use crate::native_http::{slurp_post_json, slurp_req, slurp_req_body, slurp_url, slurp_url_with_headers};

#[cfg(target_arch = "wasm32")]
pub use crate::wasm::http::{slurp_post_json, slurp_url, slurp_url_with_headers};

pub type SlurpResult = Result<(StatusCode, HeaderMap, Vec<u8>), MmError<SlurpError>>;

pub type SlurpResultJson = Result<(StatusCode, HeaderMap, Json), MmError<SlurpError>>;

#[derive(Debug, Deserialize, Display, Serialize)]
pub enum SlurpError {
    #[display(fmt = "Error deserializing '{uri}' response: {error}")]
    ErrorDeserializing { uri: String, error: String },
    #[display(fmt = "Invalid request: {_0}")]
    InvalidRequest(String),
    #[display(fmt = "Request '{uri}' timeout: {error}")]
    Timeout { uri: String, error: String },
    #[display(fmt = "Transport '{uri}' error: {error}")]
    Transport { uri: String, error: String },
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl From<serde_json::Error> for SlurpError {
    fn from(e: Error) -> Self {
        SlurpError::Internal(e.to_string())
    }
}

impl From<SlurpError> for JsonRpcErrorType {
    fn from(err: SlurpError) -> Self {
        match err {
            SlurpError::InvalidRequest(err) => Self::InvalidRequest(err),
            SlurpError::Transport { .. } | SlurpError::Timeout { .. } => Self::Transport(err.to_string()),
            SlurpError::ErrorDeserializing { uri, error } => Self::Parse(uri.into(), error),
            SlurpError::Internal(_) => Self::Internal(err.to_string()),
        }
    }
}

/// Send POST JSON HTTPS request and parse response
pub async fn post_json<T>(url: &str, json: String) -> Result<T, MmError<SlurpError>>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    let result = slurp_post_json(url, json).await?;
    serde_json::from_slice(&result.2).map_to_mm(|e| SlurpError::ErrorDeserializing {
        uri: url.to_owned(),
        error: e.to_string(),
    })
}

/// Fetch URL by HTTPS and parse JSON response
pub async fn fetch_json<T>(url: &str) -> Result<T, MmError<SlurpError>>
where
    T: serde::de::DeserializeOwned + Send + 'static,
{
    let result = slurp_url(url).await?;
    serde_json::from_slice(&result.2).map_to_mm(|e| SlurpError::ErrorDeserializing {
        uri: url.to_owned(),
        error: e.to_string(),
    })
}

/// Errors encountered when making HTTP requests to fetch information from a URI.
#[derive(Clone, Debug, Deserialize, Display, PartialEq, Serialize)]
pub enum GetInfoFromUriError {
    #[display(fmt = "Invalid request: {_0}")]
    InvalidRequest(String),
    #[display(fmt = "Transport: {_0}")]
    Transport(String),
    #[display(fmt = "Invalid response: {_0}")]
    InvalidResponse(String),
    #[display(fmt = "Internal: {_0}")]
    Internal(String),
}

/// `http::Error` can appear on an HTTP request [`http::Builder::build`] building.
impl From<http::Error> for GetInfoFromUriError {
    fn from(e: http::Error) -> Self {
        GetInfoFromUriError::InvalidRequest(e.to_string())
    }
}

impl From<serde_json::Error> for GetInfoFromUriError {
    fn from(e: serde_json::Error) -> Self {
        GetInfoFromUriError::InvalidRequest(e.to_string())
    }
}

impl From<SlurpError> for GetInfoFromUriError {
    fn from(e: SlurpError) -> Self {
        let error_str = e.to_string();
        match e {
            SlurpError::ErrorDeserializing { .. } => GetInfoFromUriError::InvalidResponse(error_str),
            SlurpError::Transport { .. } | SlurpError::Timeout { .. } => GetInfoFromUriError::Transport(error_str),
            SlurpError::InvalidRequest(_) => GetInfoFromUriError::InvalidRequest(error_str),
            SlurpError::Internal(_) => GetInfoFromUriError::Internal(error_str),
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<hyper::header::InvalidHeaderValue> for GetInfoFromUriError {
    fn from(e: hyper::header::InvalidHeaderValue) -> Self {
        GetInfoFromUriError::Internal(e.to_string())
    }
}

/// Sends a POST request to the given URI and expects a 2xx status code in response.
///
/// # Errors
///
/// Returns an error if the HTTP status code of the response is not in the 2xx range.
pub async fn send_post_request_to_uri(uri: &str, body: String) -> MmResult<Vec<u8>, GetInfoFromUriError> {
    let (status, _header, body) = slurp_post_json(uri, body).await.map_mm_err()?;
    if !status.is_success() {
        return Err(MmError::new(GetInfoFromUriError::Transport(format!(
            "Status code not in 2xx range from {uri}: {status}",
        ))));
    }
    Ok(body)
}
