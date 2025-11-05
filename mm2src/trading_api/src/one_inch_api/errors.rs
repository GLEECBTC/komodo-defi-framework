use common::StatusCode;
use derive_more::Display;
use enum_derives::EnumFromStringify;
use ethereum_types::U256;
use mm2_net::transport::SlurpError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Display, Serialize, EnumFromStringify)]
pub enum ApiClientError {
    #[from_stringify("url::ParseError")]
    InvalidParam(String),
    #[display(fmt = "Parameter {param} out of bounds, value: {value}, min: {min} max: {max}")]
    OutOfBounds {
        param: String,
        value: String,
        min: String,
        max: String,
    },
    TransportError(SlurpError),
    ParseBodyError {
        error_msg: String,
    },
    #[display(fmt = "General API error: {error_msg} description: {description}")]
    GeneralApiError {
        error_msg: String,
        description: String,
        status_code: u16,
    },
    #[display(fmt = "Allowance not enough, needed: {amount} allowance: {allowance}")]
    AllowanceNotEnough {
        error_msg: String,
        description: String,
        status_code: u16,
        /// Amount to approve for the API contract
        amount: U256,
        /// Existing allowance for the API contract
        allowance: U256,
    },
}

// API error meta 'type' field known values
const META_TYPE_ALLOWANCE: &str = "allowance";
const META_TYPE_AMOUNT: &str = "amount";

#[derive(Debug, Deserialize)]
pub(crate) struct Error400 {
    pub error: String,
    pub description: Option<String>,
    #[serde(rename = "statusCode")]
    pub status_code: u16,
    pub meta: Option<Vec<Meta>>,
    #[allow(dead_code)]
    #[serde(rename = "requestId")]
    pub request_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Meta {
    #[serde(rename = "type")]
    pub meta_type: String,
    #[serde(rename = "value")]
    pub meta_value: String,
}

#[derive(Debug)]
pub(crate) enum NativeError {
    HttpError { error_msg: String, status_code: u16 },
    HttpError400(Error400),
    ParseError { error_msg: String },
}

impl NativeError {
    pub(crate) fn new(status_code: StatusCode, body: Value) -> Self {
        if status_code == StatusCode::BAD_REQUEST {
            match serde_json::from_value(body) {
                Ok(err) => Self::HttpError400(err),
                Err(err) => Self::ParseError {
                    error_msg: format!("could not parse error response: {err}"),
                },
            }
        } else {
            Self::HttpError {
                error_msg: body["error"].as_str().unwrap_or_default().to_owned(),
                status_code: status_code.into(),
            }
        }
    }
}

impl ApiClientError {
    /// Convert from native API errors to lib errors
    /// Look for known API errors. If none found return as general API error
    pub(crate) fn from_native_error(api_error: NativeError) -> ApiClientError {
        match api_error {
            NativeError::HttpError400(error_400) => {
                if let Some(meta) = error_400.meta {
                    // Try if it's "Not enough allowance" error 'meta' data:
                    if let Some(meta_allowance) = meta.iter().find(|m| m.meta_type == META_TYPE_ALLOWANCE) {
                        // try find 'amount' value
                        let amount = if let Some(meta_amount) = meta.iter().find(|m| m.meta_type == META_TYPE_AMOUNT) {
                            U256::from_dec_str(&meta_amount.meta_value).unwrap_or_default()
                        } else {
                            Default::default()
                        };
                        let allowance = U256::from_dec_str(&meta_allowance.meta_value).unwrap_or_default();
                        return ApiClientError::AllowanceNotEnough {
                            error_msg: error_400.error,
                            status_code: error_400.status_code,
                            description: error_400.description.unwrap_or_default(),
                            amount,
                            allowance,
                        };
                    }
                }
                ApiClientError::GeneralApiError {
                    error_msg: error_400.error,
                    status_code: error_400.status_code,
                    description: error_400.description.unwrap_or_default(),
                }
            },
            NativeError::HttpError { error_msg, status_code } => ApiClientError::GeneralApiError {
                error_msg,
                status_code,
                description: Default::default(),
            },
            NativeError::ParseError { error_msg } => ApiClientError::ParseBodyError { error_msg },
        }
    }
}
