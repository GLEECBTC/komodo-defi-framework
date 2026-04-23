use super::api_types::{
    GasfreeAccountInfo, GasfreeSubmitRequest, GasfreeSubmitResponse, GasfreeSupportedToken, GasfreeTraceResponse,
};
use super::config::{ResolvedTronGaslessProvider, TronGaslessProviderConfig};
use super::error::TronGasfreeError;
use crate::eth::tron::TronAddress;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use common::custom_futures::timeout::FutureTimerExt;
use common::now_sec;
use hmac::{Hmac, Mac};
use http::StatusCode;
use mm2_err_handle::prelude::*;
use mm2_net::transport::{slurp_post_json_with_headers, slurp_url_with_headers};
use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value as Json;
use sha2::Sha256;
use std::fmt;
use std::time::Duration;
use url::Url;
use uuid::Uuid;

const API_PREFIX: &str = "api/v1";
const HEADER_TIMESTAMP: &str = "Timestamp";
const KOMODO_PROXY_COMMIT_MESSAGE: &str =
    "KomodoProxy GasFree authentication is reserved for Commit 12 and is not implemented yet";

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub enum TronGasfreeTransport {
    DirectHmac {
        base_url: Url,
        api_key: String,
        api_secret: String,
    },
    KomodoProxy {
        base_url: Url,
    },
}

impl TronGasfreeTransport {
    pub fn from_config(config: &TronGaslessProviderConfig) -> Self {
        TronGasfreeTransport::DirectHmac {
            base_url: config.base_url.clone(),
            api_key: config.api_key.clone(),
            api_secret: config.api_secret.clone(),
        }
    }

    pub fn from_resolved_provider(provider: &ResolvedTronGaslessProvider) -> Self {
        TronGasfreeTransport::from_config(provider.config())
    }

    fn base_url(&self) -> &Url {
        match self {
            TronGasfreeTransport::DirectHmac { base_url, .. } | TronGasfreeTransport::KomodoProxy { base_url } => {
                base_url
            },
        }
    }
}

impl fmt::Debug for TronGasfreeTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TronGasfreeTransport::DirectHmac { base_url, .. } => f
                .debug_struct("TronGasfreeTransport::DirectHmac")
                .field("base_url", base_url)
                .field("api_key", &"<redacted>")
                .field("api_secret", &"<redacted>")
                .finish(),
            TronGasfreeTransport::KomodoProxy { base_url } => f
                .debug_struct("TronGasfreeTransport::KomodoProxy")
                .field("base_url", base_url)
                .finish(),
        }
    }
}

pub struct TronGasfreeClient {
    transport: TronGasfreeTransport,
    request_timeout: Duration,
    supported_tokens_cache: Mutex<Option<Vec<GasfreeSupportedToken>>>,
}

impl TronGasfreeClient {
    pub fn new(transport: TronGasfreeTransport, request_timeout_ms: u64) -> Self {
        TronGasfreeClient {
            transport,
            request_timeout: Duration::from_millis(request_timeout_ms),
            supported_tokens_cache: Mutex::new(None),
        }
    }

    pub fn from_config(config: &TronGaslessProviderConfig) -> Self {
        TronGasfreeClient::new(TronGasfreeTransport::from_config(config), config.request_timeout_ms)
    }

    pub fn from_resolved_provider(provider: &ResolvedTronGaslessProvider) -> Self {
        TronGasfreeClient::from_config(provider.config())
    }

    pub fn cached_supported_tokens(&self) -> Option<Vec<GasfreeSupportedToken>> {
        self.supported_tokens_cache.lock().clone()
    }

    pub async fn get_account_info(&self, account: &TronAddress) -> MmResult<GasfreeAccountInfo, TronGasfreeError> {
        let path = format!("{API_PREFIX}/address/{account}");
        self.get_json(&path).await
    }

    pub async fn submit_transfer(
        &self,
        req: &GasfreeSubmitRequest,
    ) -> MmResult<GasfreeSubmitResponse, TronGasfreeError> {
        req.validate().map_to_mm(TronGasfreeError::InvalidRequest)?;
        self.post_json(&format!("{API_PREFIX}/gasfree/submit"), req).await
    }

    pub async fn get_trace(&self, trace_id: &str) -> MmResult<GasfreeTraceResponse, TronGasfreeError> {
        let trace_id = Uuid::parse_str(trace_id)
            .map_err(|e| MmError::new(TronGasfreeError::InvalidRequest(format!("Invalid trace id: {e}"))))?;
        self.get_json(&format!("{API_PREFIX}/gasfree/{trace_id}")).await
    }

    pub async fn get_supported_tokens(&self) -> MmResult<Vec<GasfreeSupportedToken>, TronGasfreeError> {
        if let Some(cached) = self.cached_supported_tokens() {
            return Ok(cached);
        }

        let tokens: Vec<GasfreeSupportedToken> = self.get_json(&format!("{API_PREFIX}/config/token/all")).await?;
        *self.supported_tokens_cache.lock() = Some(tokens.clone());
        Ok(tokens)
    }

    async fn get_json<T>(&self, path: &str) -> MmResult<T, TronGasfreeError>
    where
        T: DeserializeOwned,
    {
        self.execute_get(path).await
    }

    async fn post_json<T, R>(&self, path: &str, body: &T) -> MmResult<R, TronGasfreeError>
    where
        T: serde::Serialize,
        R: DeserializeOwned,
    {
        let body = serde_json::to_string(body)
            .map_to_mm(|e| TronGasfreeError::Internal(format!("Failed to serialize GasFree request: {e}")))?;
        self.execute_post(path, body).await
    }

    async fn execute_get<T>(&self, endpoint_path: &str) -> MmResult<T, TronGasfreeError>
    where
        T: DeserializeOwned,
    {
        let url = join_endpoint_url(self.transport.base_url(), endpoint_path)?;
        let headers = build_auth_headers(&self.transport, "GET", url.path(), now_sec())?;
        let borrowed_headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let response = Box::pin(slurp_url_with_headers(url.as_str(), borrowed_headers))
            .timeout(self.request_timeout)
            .await
            .map_err(|_| MmError::new(TronGasfreeError::Timeout(format!("Request to {url} timed out"))))?;

        let (status, _headers, body) = match response {
            Ok(ok) => ok,
            Err(err) => return MmError::err(TronGasfreeError::from(err.into_inner())),
        };
        decode_provider_response(status, &body)
    }

    async fn execute_post<T>(&self, endpoint_path: &str, body: String) -> MmResult<T, TronGasfreeError>
    where
        T: DeserializeOwned,
    {
        let url = join_endpoint_url(self.transport.base_url(), endpoint_path)?;
        let headers = build_auth_headers(&self.transport, "POST", url.path(), now_sec())?;
        let borrowed_headers: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        let response = Box::pin(slurp_post_json_with_headers(url.as_str(), body, borrowed_headers))
            .timeout(self.request_timeout)
            .await
            .map_err(|_| MmError::new(TronGasfreeError::Timeout(format!("Request to {url} timed out"))))?;

        let (status, _headers, body) = match response {
            Ok(ok) => ok,
            Err(err) => return MmError::err(TronGasfreeError::from(err.into_inner())),
        };
        decode_provider_response(status, &body)
    }
}

#[derive(Debug, Deserialize)]
struct ProviderEnvelope<T> {
    code: u16,
    reason: Option<String>,
    #[serde(default)]
    message: String,
    data: Option<T>,
}

fn join_endpoint_url(base_url: &Url, endpoint_path: &str) -> MmResult<Url, TronGasfreeError> {
    let mut normalized = base_url.clone();
    if !normalized.path().ends_with('/') {
        let new_path = format!("{}/", normalized.path());
        normalized.set_path(&new_path);
    }

    normalized
        .join(endpoint_path.trim_start_matches('/'))
        .map_to_mm(|e| TronGasfreeError::InvalidRequest(format!("Invalid GasFree base_url '{}': {e}", base_url)))
}

fn build_auth_headers(
    transport: &TronGasfreeTransport,
    method: &str,
    request_path: &str,
    timestamp: u64,
) -> MmResult<Vec<(String, String)>, TronGasfreeError> {
    match transport {
        TronGasfreeTransport::DirectHmac {
            api_key, api_secret, ..
        } => {
            let authorization = build_hmac_authorization(api_key, api_secret, method, request_path, timestamp);
            Ok(vec![
                (HEADER_TIMESTAMP.into(), timestamp.to_string()),
                (http::header::AUTHORIZATION.as_str().into(), authorization),
            ])
        },
        TronGasfreeTransport::KomodoProxy { .. } => {
            MmError::err(TronGasfreeError::NotImplemented(KOMODO_PROXY_COMMIT_MESSAGE.into()))
        },
    }
}

fn build_hmac_authorization(
    api_key: &str,
    api_secret: &str,
    method: &str,
    request_path: &str,
    timestamp: u64,
) -> String {
    let string_to_sign = format!("{method}{request_path}{timestamp}");
    let mut mac = HmacSha256::new_from_slice(api_secret.as_bytes()).expect("HMAC-SHA256 accepts arbitrary-length keys");
    mac.update(string_to_sign.as_bytes());
    let signature = BASE64_STANDARD.encode(mac.finalize().into_bytes());
    format!("ApiKey {api_key}:{signature}")
}

fn decode_provider_response<T>(status: StatusCode, body: &[u8]) -> MmResult<T, TronGasfreeError>
where
    T: DeserializeOwned,
{
    if !status.is_success() {
        return MmError::err(provider_error_from_http_status(status, body));
    }

    let envelope: ProviderEnvelope<T> = serde_json::from_slice(body)
        .map_to_mm(|e| TronGasfreeError::InvalidResponse(format!("GasFree API returned unexpected payload: {e}")))?;

    if envelope.code != 200 {
        let status_code = if (100..=599).contains(&envelope.code) {
            envelope.code
        } else {
            StatusCode::BAD_GATEWAY.as_u16()
        };
        return MmError::err(provider_error_from_status(
            status_code,
            sanitize_provider_message(envelope.reason.as_deref(), Some(envelope.message.as_str()), None),
        ));
    }

    envelope
        .data
        .or_mm_err(|| TronGasfreeError::InvalidResponse("GasFree API returned a success envelope without data".into()))
}

fn provider_error_from_http_status(status: StatusCode, body: &[u8]) -> TronGasfreeError {
    if let Ok(envelope) = serde_json::from_slice::<ProviderEnvelope<Json>>(body) {
        let status_code = if (100..=599).contains(&envelope.code) {
            envelope.code
        } else {
            status.as_u16()
        };
        return provider_error_from_status(
            status_code,
            sanitize_provider_message(
                envelope.reason.as_deref(),
                Some(envelope.message.as_str()),
                Some(&String::from_utf8_lossy(body)),
            ),
        );
    }

    provider_error_from_status(
        status.as_u16(),
        sanitize_provider_message(None, None, Some(&String::from_utf8_lossy(body))),
    )
}

fn provider_error_from_status(status_code: u16, message: String) -> TronGasfreeError {
    match status_code {
        401 => TronGasfreeError::Unauthorized(message),
        403 => TronGasfreeError::Forbidden(message),
        429 => TronGasfreeError::RateLimited(message),
        400..=499 => TronGasfreeError::ProviderBadRequest(message),
        500..=599 => TronGasfreeError::Upstream(message),
        _ => TronGasfreeError::Upstream(message),
    }
}

fn sanitize_provider_message(reason: Option<&str>, message: Option<&str>, fallback_body: Option<&str>) -> String {
    let reason = sanitize_text(reason.unwrap_or_default());
    let message = sanitize_text(message.unwrap_or_default());

    if !reason.is_empty() && !message.is_empty() {
        return format!("{reason}: {message}");
    }
    if !message.is_empty() {
        return message;
    }
    if !reason.is_empty() {
        return reason;
    }

    let fallback = sanitize_text(fallback_body.unwrap_or_default());
    if fallback.is_empty() {
        "GasFree provider returned an error".into()
    } else {
        fallback
    }
}

fn sanitize_text(text: &str) -> String {
    const MAX_CHARS: usize = 256;

    let sanitized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if sanitized.chars().count() > MAX_CHARS {
        let truncated: String = sanitized.chars().take(MAX_CHARS).collect();
        format!("{truncated}…")
    } else {
        sanitized
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn hmac_authorization_matches_known_vector() {
        let authorization = build_hmac_authorization(
            "test-key",
            "test-secret",
            "GET",
            "/nile/api/v1/config/token/all",
            1_731_912_286,
        );

        assert_eq!(
            authorization,
            "ApiKey test-key:sYfwaLSnifcL/MTLFDmfORdCVceFLckEPL1mMKEjTHQ="
        );
    }

    #[test]
    fn transport_variants_preserve_routing_and_redaction() {
        let err = build_auth_headers(
            &TronGasfreeTransport::KomodoProxy {
                base_url: Url::parse("https://proxy.komodo.test/").unwrap(),
            },
            "GET",
            "/api/v1/config/token/all",
            123,
        )
        .unwrap_err();
        match err.into_inner() {
            TronGasfreeError::NotImplemented(message) => assert!(message.contains("Commit 12")),
            other => panic!("unexpected error: {:?}", other),
        }

        let transport = TronGasfreeTransport::DirectHmac {
            base_url: Url::parse("https://open-test.gasfree.io/nile/").unwrap(),
            api_key: "super-secret-key".into(),
            api_secret: "super-secret-value".into(),
        };
        let debug = format!("{transport:?}");
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("super-secret-key"));
        assert!(!debug.contains("super-secret-value"));
    }

    #[test]
    fn http_200_with_provider_error_envelope_is_rejected() {
        let err = decode_provider_response::<GasfreeSubmitResponse>(
            StatusCode::OK,
            br#"{"code":429,"reason":"TooManyRequests","message":" slow down ","data":null}"#,
        )
        .unwrap_err();

        match err.into_inner() {
            TronGasfreeError::RateLimited(message) => assert_eq!(message, "TooManyRequests: slow down"),
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[test]
    fn decode_submit_response_accepts_estimate_transfer_fee_alias_only() {
        let body = serde_json::to_vec(&json!({
            "code": 200,
            "reason": null,
            "message": "",
            "data": {
                "amount": 100000,
                "providerAddress": "TKtWbdzEq5ss9vTS9kwRhBp5mXmBfBns3E",
                "accountAddress": "TUUSMd58eC3fKx3fn7whxJyr1FR56tgaP8",
                "signature": "",
                "targetAddress": "TEkj3ndMVEmFLYaFrATMwMjBRZ1EAZkucT",
                "maxFee": 2000000,
                "version": 1,
                "nonce": 8,
                "tokenAddress": "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf",
                "expiredAt": 1747909695000u64,
                "estimateTransferFee": 2000,
                "id": "6c3ff67e-0bf4-4c09-91ca-0c7c254b01a0",
                "state": "WAITING",
                "estimatedActivateFee": 0,
                "gasFreeAddress": "TNER12mMVWruqopsW9FQtKxCGfZcEtb3ER"
            }
        }))
        .unwrap();

        let response: GasfreeSubmitResponse = decode_provider_response(StatusCode::OK, &body).unwrap();
        assert_eq!(response.estimated_transfer_fee, 2000u64.into());
    }

    fn timed_out_slurp_error() -> mm2_net::transport::SlurpError {
        mm2_net::transport::SlurpError::Timeout {
            uri: "https://open-test.gasfree.io/nile/api/v1/config/token/all".into(),
            error: "timed out".into(),
        }
    }

    #[test]
    fn timeout_slurp_error_maps_to_timeout_variant() {
        let err = TronGasfreeError::from(timed_out_slurp_error());

        match err {
            TronGasfreeError::Timeout(message) => assert!(message.contains("timeout")),
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[test]
    fn sanitize_text_truncates_on_utf8_char_boundary() {
        let input = format!("{}€€", "a".repeat(255));
        let sanitized = sanitize_text(&input);
        assert_eq!(sanitized, format!("{}€…", "a".repeat(255)));
    }
}
