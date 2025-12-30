//! Structs to call 1inch classic swap api

use super::client::QueryParams;
use super::errors::OneInchError;
use common::{def_with_opt_param, push_if_some};
use ethereum_types::Address;
use mm2_err_handle::mm_error::MmResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use url::Url;

const ONE_INCH_MAX_SLIPPAGE: f32 = 50.0;
const ONE_INCH_MAX_FEE_SHARE: f32 = 3.0;
const ONE_INCH_MAX_GAS: u64 = 11500000;
const ONE_INCH_MAX_PARTS: u32 = 100;
const ONE_INCH_MAX_MAIN_ROUTE_PARTS: u32 = 50;
const ONE_INCH_MAX_COMPLEXITY_LEVEL: u32 = 3;

const BAD_URL_IN_RESPONSE_ERROR: &str = "unsupported url in response";
const ONE_INCH_DOMAIN: &str = "1inch.io";

/// API params builder for swap quote
#[derive(Default)]
pub struct ClassicSwapQuoteCallBuilder {
    /// Source token address
    src: String,
    /// Destination token address
    dst: String,
    /// Source amount, decimal in coin units
    amount: String,
    // Optional fields
    fee: Option<f32>,
    protocols: Option<String>,
    gas_price: Option<String>,
    complexity_level: Option<u32>,
    parts: Option<u32>,
    main_route_parts: Option<u32>,
    gas_limit: Option<u64>, // originally in 1inch was u128 but we made it u64 as serde does not support u128
    include_tokens_info: Option<bool>,
    include_protocols: Option<bool>,
    include_gas: Option<bool>,
    connector_tokens: Option<String>,
}

impl ClassicSwapQuoteCallBuilder {
    pub fn new(src: String, dst: String, amount: String) -> Self {
        Self {
            src,
            dst,
            amount,
            ..Default::default()
        }
    }

    def_with_opt_param!(fee, f32);
    def_with_opt_param!(protocols, String);
    def_with_opt_param!(gas_price, String);
    def_with_opt_param!(complexity_level, u32);
    def_with_opt_param!(parts, u32);
    def_with_opt_param!(main_route_parts, u32);
    def_with_opt_param!(gas_limit, u64);
    def_with_opt_param!(include_tokens_info, bool);
    def_with_opt_param!(include_protocols, bool);
    def_with_opt_param!(include_gas, bool);
    def_with_opt_param!(connector_tokens, String);

    #[allow(clippy::result_large_err)]
    pub fn build_query_params(&self) -> MmResult<QueryParams, OneInchError> {
        self.validate_params()?;

        let mut params = vec![
            ("src", self.src.clone()),
            ("dst", self.dst.clone()),
            ("amount", self.amount.clone()),
        ];

        push_if_some!(params, "fee", self.fee);
        push_if_some!(params, "protocols", &self.protocols);
        push_if_some!(params, "gasPrice", &self.gas_price);
        push_if_some!(params, "complexityLevel", self.complexity_level);
        push_if_some!(params, "parts", self.parts);
        push_if_some!(params, "mainRouteParts", self.main_route_parts);
        push_if_some!(params, "gasLimit", self.gas_limit);
        push_if_some!(params, "includeTokensInfo", self.include_tokens_info);
        push_if_some!(params, "includeProtocols", self.include_protocols);
        push_if_some!(params, "includeGas", self.include_gas);
        push_if_some!(params, "connectorTokens", &self.connector_tokens);
        Ok(params)
    }

    /// Validate params by 1inch rules (to avoid extra requests)
    #[allow(clippy::result_large_err)]
    fn validate_params(&self) -> MmResult<(), OneInchError> {
        validate_fee(&self.fee)?;
        validate_complexity_level(&self.complexity_level)?;
        validate_gas_limit(&self.gas_limit)?;
        validate_parts(&self.parts)?;
        validate_main_route_parts(&self.main_route_parts)?;
        Ok(())
    }
}

/// API params builder to create a tx for swap
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct ClassicSwapCreateCallBuilder {
    pub src: String,
    pub dst: String,
    /// Amount in token smallest units
    pub amount: String,
    from: String,
    slippage: f32,
    fee: Option<f32>,
    protocols: Option<String>,
    gas_price: Option<String>,
    complexity_level: Option<u32>,
    parts: Option<u32>,
    main_route_parts: Option<u32>,
    gas_limit: Option<u64>, // originally in 1inch was u128 but we made it u64 as serde does not support u128
    include_tokens_info: Option<bool>,
    include_protocols: Option<bool>,
    include_gas: Option<bool>,
    connector_tokens: Option<String>,
    excluded_protocols: Option<String>,
    permit: Option<String>,
    compatibility: Option<bool>,
    receiver: Option<String>,
    referrer: Option<String>,
    disable_estimate: Option<bool>,
    allow_partial_fill: Option<bool>,
    use_permit2: Option<bool>,
}

impl ClassicSwapCreateCallBuilder {
    pub fn new(src: String, dst: String, amount: String, from: String, slippage: f32) -> Self {
        Self {
            src,
            dst,
            amount,
            from,
            slippage,
            ..Default::default()
        }
    }

    def_with_opt_param!(fee, f32);
    def_with_opt_param!(protocols, String);
    def_with_opt_param!(gas_price, String);
    def_with_opt_param!(complexity_level, u32);
    def_with_opt_param!(parts, u32);
    def_with_opt_param!(main_route_parts, u32);
    def_with_opt_param!(gas_limit, u64);
    def_with_opt_param!(include_tokens_info, bool);
    def_with_opt_param!(include_protocols, bool);
    def_with_opt_param!(include_gas, bool);
    def_with_opt_param!(connector_tokens, String);
    def_with_opt_param!(excluded_protocols, String);
    def_with_opt_param!(permit, String);
    def_with_opt_param!(compatibility, bool);
    def_with_opt_param!(receiver, String);
    def_with_opt_param!(referrer, String);
    def_with_opt_param!(disable_estimate, bool);
    def_with_opt_param!(allow_partial_fill, bool);
    def_with_opt_param!(use_permit2, bool);

    #[allow(clippy::result_large_err)]
    pub fn build_query_params(&self) -> MmResult<QueryParams, OneInchError> {
        self.validate_params()?;

        let mut params = vec![
            ("src", self.src.clone()),
            ("dst", self.dst.clone()),
            ("amount", self.amount.clone()),
            ("from", self.from.clone()),
            ("slippage", self.slippage.to_string()),
        ];

        push_if_some!(params, "fee", self.fee);
        push_if_some!(params, "protocols", &self.protocols);
        push_if_some!(params, "gasPrice", &self.gas_price);
        push_if_some!(params, "complexityLevel", self.complexity_level);
        push_if_some!(params, "parts", self.parts);
        push_if_some!(params, "mainRouteParts", self.main_route_parts);
        push_if_some!(params, "gasLimit", self.gas_limit);
        push_if_some!(params, "includeTokensInfo", self.include_tokens_info);
        push_if_some!(params, "includeProtocols", self.include_protocols);
        push_if_some!(params, "includeGas", self.include_gas);
        push_if_some!(params, "connectorTokens", &self.connector_tokens);
        push_if_some!(params, "excludedProtocols", &self.excluded_protocols);
        push_if_some!(params, "permit", &self.permit);
        push_if_some!(params, "compatibility", &self.compatibility);
        push_if_some!(params, "receiver", &self.receiver);
        push_if_some!(params, "referrer", &self.referrer);
        push_if_some!(params, "disableEstimate", self.disable_estimate);
        push_if_some!(params, "allowPartialFill", self.allow_partial_fill);
        push_if_some!(params, "usePermit2", self.use_permit2);

        Ok(params)
    }

    /// Validate params by 1inch rules (to avoid extra requests)
    #[allow(clippy::result_large_err)]
    fn validate_params(&self) -> MmResult<(), OneInchError> {
        validate_slippage(self.slippage)?;
        validate_fee(&self.fee)?;
        validate_complexity_level(&self.complexity_level)?;
        validate_gas_limit(&self.gas_limit)?;
        validate_parts(&self.parts)?;
        validate_main_route_parts(&self.main_route_parts)?;
        Ok(())
    }
}

#[derive(Clone, Deserialize, Debug, Serialize)]
pub struct TokenInfo {
    pub address: Address,
    pub symbol: String,
    pub name: String,
    pub decimals: u32,
    pub eip2612: bool,
    #[serde(rename = "isFoT", default)]
    pub is_fot: bool,
    #[serde(
        rename = "logoURI",
        default,
        deserialize_with = "serde_one_inch_link::deserialize_opt_string"
    )] // Note: needed to use 'default' with 'deserialize_with' to allow optional 'logoURI'
    pub logo_uri: Option<String>,
    pub tags: Vec<String>,
    /// Token name as it is defined in the coins file.
    /// This is used to show route tokens in the GUI, like they are in the coin file.
    /// However, route tokens can be missed in the coins file and therefore cannot be filled.
    /// In this case GUI may use LrTokenInfo::Address or LrTokenInfo::Symbol
    pub symbol_kdf: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ProtocolInfo {
    pub name: String,
    pub part: f64,
    #[serde(rename = "fromTokenAddress")]
    pub from_token_address: Address,
    #[serde(rename = "toTokenAddress")]
    pub to_token_address: Address,
}

/// Returned data from an API call to get quote or create swap
#[derive(Clone, Deserialize, Debug)]
pub struct ClassicSwapData {
    /// dst token amount to receive, integer number as string (1inch API format)
    #[serde(rename = "dstAmount")]
    pub dst_amount: String,
    #[serde(rename = "srcToken")]
    pub src_token: Option<TokenInfo>,
    #[serde(rename = "dstToken")]
    pub dst_token: Option<TokenInfo>,
    pub protocols: Option<Vec<Vec<Vec<ProtocolInfo>>>>,
    /// Returned from create swap call
    pub tx: Option<TxFields>,
    /// Returned from quote call
    /// NOTE: in the 1inch API this field is u128 but this type is not supported by serde. u64 should be enough
    pub gas: Option<u64>,
}

#[derive(Clone, Deserialize, Debug)]
pub struct TxFields {
    pub from: Address,
    pub to: Address,
    pub data: String,
    /// tx value, integer number as string, in smallest units (1inch API format)
    pub value: String,
    /// gas price, integer number as string, in wei (1inch API format)
    #[serde(rename = "gasPrice")]
    pub gas_price: String,
    /// gas limit
    pub gas: u64,
}

#[derive(Deserialize, Serialize)]
pub struct ProtocolImage {
    pub id: String,
    pub title: String,
    #[serde(deserialize_with = "serde_one_inch_link::deserialize_opt_string")]
    pub img: Option<String>,
    #[serde(deserialize_with = "serde_one_inch_link::deserialize_opt_string")]
    pub img_color: Option<String>,
}

#[derive(Deserialize)]
pub struct ProtocolsResponse {
    pub protocols: Vec<ProtocolImage>,
}

#[derive(Deserialize)]
pub struct TokensResponse {
    pub tokens: HashMap<String, TokenInfo>,
}

mod serde_one_inch_link {
    use super::validate_one_inch_link;
    use serde::{Deserialize, Deserializer};

    /// Deserialise Option<String> with checking links
    pub(super) fn deserialize_opt_string<'a, D>(deserializer: D) -> Result<Option<String>, D::Error>
    where
        D: Deserializer<'a>,
    {
        <Option<String> as Deserialize>::deserialize(deserializer)
            .map(|opt_value| opt_value.map(|value| validate_one_inch_link(&value).unwrap_or_default()))
    }
}

#[allow(clippy::result_large_err)]
fn validate_slippage(slippage: f32) -> MmResult<(), OneInchError> {
    if !(0.0..=ONE_INCH_MAX_SLIPPAGE).contains(&slippage) {
        return Err(OneInchError::OutOfBounds {
            param: "slippage".to_owned(),
            value: slippage.to_string(),
            min: 0.0.to_string(),
            max: ONE_INCH_MAX_SLIPPAGE.to_string(),
        }
        .into());
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_fee(fee: &Option<f32>) -> MmResult<(), OneInchError> {
    if let Some(fee) = fee {
        if !(0.0..=ONE_INCH_MAX_FEE_SHARE).contains(fee) {
            return Err(OneInchError::OutOfBounds {
                param: "fee".to_owned(),
                value: fee.to_string(),
                min: 0.0.to_string(),
                max: ONE_INCH_MAX_FEE_SHARE.to_string(),
            }
            .into());
        }
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_gas_limit(gas_limit: &Option<u64>) -> MmResult<(), OneInchError> {
    if let Some(gas_limit) = gas_limit {
        if gas_limit > &ONE_INCH_MAX_GAS {
            return Err(OneInchError::OutOfBounds {
                param: "gas_limit".to_owned(),
                value: gas_limit.to_string(),
                min: 0.to_string(),
                max: ONE_INCH_MAX_GAS.to_string(),
            }
            .into());
        }
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_parts(parts: &Option<u32>) -> MmResult<(), OneInchError> {
    if let Some(parts) = parts {
        if parts > &ONE_INCH_MAX_PARTS {
            return Err(OneInchError::OutOfBounds {
                param: "parts".to_owned(),
                value: parts.to_string(),
                min: 0.to_string(),
                max: ONE_INCH_MAX_PARTS.to_string(),
            }
            .into());
        }
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_main_route_parts(main_route_parts: &Option<u32>) -> MmResult<(), OneInchError> {
    if let Some(main_route_parts) = main_route_parts {
        if main_route_parts > &ONE_INCH_MAX_MAIN_ROUTE_PARTS {
            return Err(OneInchError::OutOfBounds {
                param: "main route parts".to_owned(),
                value: main_route_parts.to_string(),
                min: 0.to_string(),
                max: ONE_INCH_MAX_MAIN_ROUTE_PARTS.to_string(),
            }
            .into());
        }
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
fn validate_complexity_level(complexity_level: &Option<u32>) -> MmResult<(), OneInchError> {
    if let Some(complexity_level) = complexity_level {
        if complexity_level > &ONE_INCH_MAX_COMPLEXITY_LEVEL {
            return Err(OneInchError::OutOfBounds {
                param: "complexity level".to_owned(),
                value: complexity_level.to_string(),
                min: 0.to_string(),
                max: ONE_INCH_MAX_COMPLEXITY_LEVEL.to_string(),
            }
            .into());
        }
    }
    Ok(())
}

/// Check if url is valid and is a subdomain of 1inch domain (simple anti-phishing check)
fn validate_one_inch_link(s: &str) -> Result<String, OneInchError> {
    let url = Url::parse(s).map_err(|_err| OneInchError::ParseBodyError {
        error_msg: BAD_URL_IN_RESPONSE_ERROR.to_owned(),
    })?;
    if let Some(host) = url.host() {
        if host.to_string().ends_with(ONE_INCH_DOMAIN) {
            return Ok(s.to_owned());
        }
    }
    Err(OneInchError::ParseBodyError {
        error_msg: BAD_URL_IN_RESPONSE_ERROR.to_owned(),
    })
}

#[test]
fn test_validate_one_inch_link() {
    assert!(validate_one_inch_link("https://cdn.1inch.io/liquidity-sources-logo/wmatic_color.png").is_ok());
    assert!(validate_one_inch_link("https://CDN.1INCH.IO/liquidity-sources-logo/wmatic_color.png").is_ok());
    assert!(validate_one_inch_link("https://example.org/somepath/somefile.png").is_err());
    assert!(validate_one_inch_link("https://inch.io/somepath/somefile.png").is_err());
    assert!(validate_one_inch_link("127.0.0.1").is_err());
}
