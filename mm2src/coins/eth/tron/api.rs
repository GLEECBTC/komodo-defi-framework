//! TRON HTTP API client for wallet operations.
//!
//! Implements the minimal TRON API endpoints needed for HD activation and balance queries:
//! - `/wallet/getnowblock` - current block number
//! - `/wallet/getaccount` - account info (balance, existence)

use super::TronAddress;
use crate::eth::{Web3RpcError, Web3RpcResult};

use common::{APPLICATION_JSON, PROXY_REQUEST_EXPIRATION_SEC, X_AUTH_PAYLOAD};
use ethereum_types::U256;
use http::header::CONTENT_TYPE;
use http::Uri;
use mm2_p2p::Keypair;
use proxy_signature::RawMessage;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Timeout for individual TRON API requests.
pub const TRON_API_TIMEOUT: Duration = Duration::from_secs(10);

/// Detects TRON API error payloads and extracts the error message.
/// Returns `Some(message)` if the response indicates an error, `None` otherwise.
///
/// TRON API error formats:
/// - `{"Error": "message"}` - Access control / config errors (REST API)
/// - `{"error": "message"}` - General errors (string variant)
/// - `{"error": {"code": ..., "message": ...}}` - JSON-RPC 2.0 errors
/// - `{"result": false, "code": "...", "message": "..."}` - Transaction/contract errors
/// - `{"result": {"result": false, "code": "...", "message": "..."}}` - Nested transaction result
/// - `{"code": "...", "message": "..."}` - Simplified error format
fn tron_error_from_value(v: &serde_json::Value) -> Option<String> {
    let obj = v.as_object()?;

    // Helper to convert code/message values to string
    let value_to_string = |v: &serde_json::Value| -> String {
        v.as_str()
            .map(ToString::to_string)
            .or_else(|| v.as_i64().map(|i| i.to_string()))
            .or_else(|| v.as_u64().map(|u| u.to_string()))
            .unwrap_or_else(|| v.to_string())
    };

    // Helper to format code + message
    let format_error = |code: Option<String>, message: Option<String>, default: &str| -> String {
        match (code, message) {
            (Some(c), Some(m)) => format!("{c}: {m}"),
            (Some(c), None) => c,
            (None, Some(m)) => m,
            (None, None) => default.to_string(),
        }
    };

    // Format: {"Error": "message"} - REST API access control errors
    if let Some(msg) = obj.get("Error").and_then(|v| v.as_str()) {
        return Some(msg.to_string());
    }

    // Format: {"error": "message"} - General string errors
    if let Some(msg) = obj.get("error").and_then(|v| v.as_str()) {
        return Some(msg.to_string());
    }

    // Format: {"error": {"code": ..., "message": ...}} - JSON-RPC 2.0 errors
    if let Some(error_obj) = obj.get("error").and_then(|v| v.as_object()) {
        let code = error_obj.get("code").map(&value_to_string);
        let message = error_obj.get("message").map(&value_to_string);
        return Some(format_error(code, message, "JSON-RPC error"));
    }

    // Format: {"result": {"result": false, "code": "...", "message": "..."}} - Nested transaction result
    // Used by triggerConstantContract, triggerContract, estimateEnergy, broadcastTransaction
    // Note: "result" can be false, null, or missing when there's an error
    if let Some(result_obj) = obj.get("result").and_then(|v| v.as_object()) {
        let inner_result = result_obj.get("result").and_then(|v| v.as_bool());
        let has_error_code = result_obj.get("code").is_some();

        // Error if: inner result is false, OR inner result is null/missing but has error code
        if inner_result == Some(false) || (inner_result.is_none() && has_error_code) {
            let code = result_obj.get("code").map(&value_to_string);
            let message = result_obj.get("message").map(&value_to_string);
            return Some(format_error(code, message, "Transaction failed"));
        }
    }

    // Format: {"result": false, "code": "...", "message": "..."} - Top-level boolean result
    if matches!(obj.get("result").and_then(|v| v.as_bool()), Some(false)) {
        let code = obj.get("code").map(&value_to_string);
        let message = obj.get("message").map(&value_to_string);
        return Some(format_error(code, message, "TRON API returned result=false"));
    }

    // Format: {"code": "...", "message": "..."} without result field
    // Avoid false positives if endpoint returns {"result": true, "code": ..., "message": ...}
    if obj.get("code").is_some()
        && obj.get("message").is_some()
        && !matches!(obj.get("result").and_then(|v| v.as_bool()), Some(true))
    {
        let code = obj.get("code").map(&value_to_string);
        let message = obj.get("message").map(&value_to_string);
        return Some(format_error(code, message, "TRON API error"));
    }

    None
}

/// TRON HTTP transport node configuration.
#[derive(Clone, Debug)]
pub struct TronHttpNode {
    pub uri: Uri,
    pub komodo_proxy: bool,
}

/// TRON HTTP client for a single node.
#[derive(Clone, Debug)]
pub struct TronHttpClient {
    pub node: TronHttpNode,
    /// Keypair for signing requests to komodo proxy nodes.
    pub proxy_sign_keypair: Option<Arc<Keypair>>,
}

impl TronHttpClient {
    pub fn new(node: TronHttpNode, proxy_sign_keypair: Option<Arc<Keypair>>) -> Self {
        Self {
            node,
            proxy_sign_keypair,
        }
    }

    /// Send a POST request to the TRON API.
    pub async fn post<T: Serialize, R: DeserializeOwned>(&self, path: &str, body: &T) -> Web3RpcResult<R> {
        // Build URI, avoiding double slashes
        let base = self.node.uri.to_string();
        let base = base.trim_end_matches('/');
        let path = path.trim_start_matches('/');
        let uri_str = format!("{}/{}", base, path);
        let uri: Uri = uri_str
            .parse()
            .map_err(|e| Web3RpcError::Internal(format!("Invalid URI: {e}")))?;

        let body_bytes = serde_json::to_vec(body).map_err(|e| Web3RpcError::Internal(e.to_string()))?;
        let response_bytes = self.send_request(&uri, body_bytes).await?;

        // Parse JSON once
        let response_json: serde_json::Value = serde_json::from_slice(&response_bytes)
            .map_err(|e| Web3RpcError::InvalidResponse(format!("Failed to parse JSON response: {e}")))?;

        // Check for TRON error payloads (200 OK but error content)
        if let Some(error_msg) = tron_error_from_value(&response_json) {
            return Err(Web3RpcError::InvalidResponse(format!("TRON API error: {error_msg}")).into());
        }

        // Convert Value to typed response (no re-parsing of bytes)
        serde_json::from_value(response_json)
            .map_err(|e| Web3RpcError::InvalidResponse(format!("Failed to parse response: {e}")).into())
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn send_request(&self, uri: &Uri, body: Vec<u8>) -> Web3RpcResult<Vec<u8>> {
        use common::custom_futures::timeout::FutureTimerExt;
        use http::header::HeaderValue;
        use mm2_net::transport::slurp_req;

        let mut req = http::Request::new(body.clone());
        *req.method_mut() = http::Method::POST;
        *req.uri_mut() = uri.clone();
        req.headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static(APPLICATION_JSON));

        // Add proxy signature if using komodo proxy
        if self.node.komodo_proxy {
            let keypair = self.proxy_sign_keypair.as_ref().ok_or_else(|| {
                Web3RpcError::Internal("Proxy node requires signing keypair but none provided".to_string())
            })?;

            let proxy_sign = RawMessage::sign(keypair, uri, body.len(), PROXY_REQUEST_EXPIRATION_SEC)
                .map_err(|e| Web3RpcError::Internal(format!("Proxy signing failed: {e}")))?;

            let proxy_sign_json =
                serde_json::to_string(&proxy_sign).map_err(|e| Web3RpcError::Internal(e.to_string()))?;

            let header_value = proxy_sign_json
                .parse()
                .map_err(|e| Web3RpcError::Internal(format!("Invalid proxy header value: {e}")))?;

            req.headers_mut().insert(X_AUTH_PAYLOAD, header_value);
        }

        match Box::pin(slurp_req(req)).timeout(TRON_API_TIMEOUT).await {
            Ok(Ok((status, _headers, response_body))) => {
                if !status.is_success() {
                    return Err(Web3RpcError::Transport(format!(
                        "TRON API returned status {}: {}",
                        status,
                        String::from_utf8_lossy(&response_body)
                    ))
                    .into());
                }
                Ok(response_body)
            },
            Ok(Err(e)) => Err(Web3RpcError::Transport(format!("Request failed: {e}")).into()),
            Err(_timeout) => Err(Web3RpcError::Timeout(format!("Request to {} timed out", uri)).into()),
        }
    }

    #[cfg(target_arch = "wasm32")]
    async fn send_request(&self, uri: &Uri, body: Vec<u8>) -> Web3RpcResult<Vec<u8>> {
        use common::custom_futures::timeout::FutureTimerExt;
        use http::header::ACCEPT;
        use mm2_net::wasm::http::FetchRequest;

        let body_str =
            String::from_utf8(body.clone()).map_err(|e| Web3RpcError::Internal(format!("Invalid UTF-8 body: {e}")))?;

        let mut request = FetchRequest::post(&uri.to_string());
        request = request
            .cors()
            .body_utf8(body_str)
            .header(ACCEPT.as_str(), APPLICATION_JSON)
            .header(CONTENT_TYPE.as_str(), APPLICATION_JSON);

        // Add proxy signature if using komodo proxy
        if self.node.komodo_proxy {
            let keypair = self.proxy_sign_keypair.as_ref().ok_or_else(|| {
                Web3RpcError::Internal("Proxy node requires signing keypair but none provided".to_string())
            })?;

            let proxy_sign = RawMessage::sign(keypair, uri, body.len(), PROXY_REQUEST_EXPIRATION_SEC)
                .map_err(|e| Web3RpcError::Internal(format!("Proxy signing failed: {e}")))?;

            let proxy_sign_json =
                serde_json::to_string(&proxy_sign).map_err(|e| Web3RpcError::Internal(e.to_string()))?;

            request = request.header(X_AUTH_PAYLOAD, &proxy_sign_json);
        }

        let (status_code, response_str) = match Box::pin(request.request_str()).timeout(TRON_API_TIMEOUT).await {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => return Err(Web3RpcError::Transport(format!("WASM fetch failed: {e:?}")).into()),
            Err(_timeout) => return Err(Web3RpcError::Timeout(format!("Request to {} timed out", uri)).into()),
        };

        if !status_code.is_success() {
            return Err(
                Web3RpcError::Transport(format!("TRON API returned status {}: {}", status_code, response_str)).into(),
            );
        }

        Ok(response_str.into_bytes())
    }
}

// ============================================================================
// TRON API Request/Response Types
// ============================================================================

/// Request body for `/wallet/getnowblock`.
#[derive(Serialize)]
struct GetNowBlockRequest {}

/// Response from `/wallet/getnowblock`.
#[derive(Deserialize, Debug)]
pub struct GetNowBlockResponse {
    #[serde(default)]
    pub block_header: Option<BlockHeader>,
}

#[derive(Deserialize, Debug)]
pub struct BlockHeader {
    pub raw_data: BlockRawData,
}

#[derive(Deserialize, Debug)]
pub struct BlockRawData {
    pub number: i64,
}

/// Request body for `/wallet/getaccount`.
#[derive(Serialize)]
struct GetAccountRequest {
    address: String,
    visible: bool,
}

/// Response from `/wallet/getaccount`.
///
/// Note: TRON returns an empty object `{}` if the account doesn't exist.
/// All fields are optional to handle this case.
#[derive(Deserialize, Debug, Default)]
pub struct GetAccountResponse {
    /// Account address (only present for existing accounts on some nodes).
    #[serde(default)]
    pub address: Option<String>,
    /// Balance in SUN (1 TRX = 1,000,000 SUN).
    #[serde(default)]
    pub balance: Option<i64>,
    /// Account creation timestamp.
    #[serde(default)]
    pub create_time: Option<i64>,
    /// Owner permission structure (present for activated accounts).
    #[serde(default)]
    pub owner_permission: Option<serde_json::Value>,
}

impl GetAccountResponse {
    /// Returns true if the account exists and has meaningful on-chain presence.
    ///
    /// An account is considered "used" (for HD gap-limit scanning) if any of these are true:
    /// - Has address field (some nodes only return this for existing accounts)
    /// - Has positive balance
    /// - Has a creation timestamp (was explicitly activated)
    /// - Has owner permission structure
    ///
    /// Based on java-tron behavior: empty object = account doesn't exist.
    pub fn exists_meaningfully(&self) -> bool {
        self.address.is_some()
            || self.balance.unwrap_or(0) > 0
            || self.create_time.is_some()
            || self.owner_permission.is_some()
    }
}

// ============================================================================
// High-level TRON API methods
// ============================================================================

impl TronHttpClient {
    /// Get the current block number.
    pub async fn get_now_block_number(&self) -> Web3RpcResult<u64> {
        let response: GetNowBlockResponse = self.post("/wallet/getnowblock", &GetNowBlockRequest {}).await?;

        let block_header = response
            .block_header
            .ok_or_else(|| Web3RpcError::InvalidResponse("Missing block_header in getnowblock response".to_string()))?;

        let block_number = block_header.raw_data.number;
        if block_number < 0 {
            return Err(Web3RpcError::InvalidResponse(format!("Invalid negative block number: {block_number}")).into());
        }
        Ok(block_number as u64)
    }

    /// Get account information for a TRON address.
    pub async fn get_account(&self, address: &TronAddress) -> Web3RpcResult<GetAccountResponse> {
        let request = GetAccountRequest {
            // Use hex format with 0x41 prefix for the API
            address: address.to_hex(),
            visible: false,
        };

        self.post("/wallet/getaccount", &request).await
    }

    /// Get account balance in SUN (smallest unit, 1 TRX = 1,000,000 SUN).
    pub async fn get_account_balance_sun(&self, address: &TronAddress) -> Web3RpcResult<U256> {
        let account = self.get_account(address).await?;
        let balance = account.balance.unwrap_or(0).max(0) as u64;
        Ok(U256::from(balance))
    }

    /// Check if an address has been used (for HD wallet gap-limit scanning).
    pub async fn is_address_used(&self, address: &TronAddress) -> Web3RpcResult<bool> {
        let account = self.get_account(address).await?;
        Ok(account.exists_meaningfully())
    }
}

// ============================================================================
// TRON API Client (node rotation)
// ============================================================================

use futures::lock::Mutex as AsyncMutex;

/// Pool of TRON HTTP clients with rotation on success.
#[derive(Clone)]
pub struct TronApiClient {
    clients: Arc<AsyncMutex<Vec<TronHttpClient>>>,
}

impl TronApiClient {
    pub fn new(clients: Vec<TronHttpClient>) -> Self {
        Self {
            clients: Arc::new(AsyncMutex::new(clients)),
        }
    }

    /// Execute an operation with node rotation.
    /// Tries each node until one succeeds, rotating successful nodes to front.
    ///
    /// Only retryable errors (transport failures, timeouts) trigger fallback to the next node.
    /// Non-retryable errors (invalid responses, API errors like "contract doesn't exist")
    /// return immediately since they would fail on any node.
    async fn try_clients<F, Fut, T>(&self, op: F) -> Web3RpcResult<T>
    where
        F: Fn(TronHttpClient) -> Fut,
        Fut: std::future::Future<Output = Web3RpcResult<T>>,
    {
        let mut clients: futures::lock::MutexGuard<'_, Vec<TronHttpClient>> = self.clients.lock().await;

        if clients.is_empty() {
            return Err(Web3RpcError::Transport("No TRON API nodes configured".to_string()).into());
        }

        let mut last_error = Web3RpcError::Transport("All TRON nodes unreachable".to_string());

        for (i, client) in clients.clone().into_iter().enumerate() {
            match op(client).await {
                Ok(result) => {
                    // Rotate successful client to front
                    clients.rotate_left(i);
                    return Ok(result);
                },
                Err(e) => {
                    let inner = e.into_inner();
                    // Only retry on transport/timeout errors (transient issues).
                    // InvalidResponse, Internal, etc. are permanent - the same request
                    // would fail on any node (e.g., "contract doesn't exist").
                    if !inner.is_retryable() {
                        return Err(inner.into());
                    }
                    last_error = inner;
                },
            }
        }

        Err(last_error.into())
    }

    /// Get the current block number.
    pub async fn get_now_block_number(&self) -> Web3RpcResult<u64> {
        self.try_clients(|client| async move { client.get_now_block_number().await })
            .await
    }

    /// Get account information.
    pub async fn get_account(&self, address: &TronAddress) -> Web3RpcResult<GetAccountResponse> {
        let address = *address;
        self.try_clients(|client| {
            let addr = address;
            async move { client.get_account(&addr).await }
        })
        .await
    }

    /// Get account balance in SUN.
    pub async fn get_account_balance_sun(&self, address: &TronAddress) -> Web3RpcResult<U256> {
        let address = *address;
        self.try_clients(|client| {
            let addr = address;
            async move { client.get_account_balance_sun(&addr).await }
        })
        .await
    }

    /// Check if an address has been used.
    pub async fn is_address_used(&self, address: &TronAddress) -> Web3RpcResult<bool> {
        let address = *address;
        self.try_clients(|client| {
            let addr = address;
            async move { client.is_address_used(&addr).await }
        })
        .await
    }
}

impl std::fmt::Debug for TronApiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TronApiClient").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // =========================================================================
    // tron_error_from_value unit tests
    // =========================================================================

    #[test]
    fn test_error_uppercase_error() {
        // Format: {"Error": "message"} - REST API access control errors
        let v = json!({"Error": "this API is unavailable due to config"});
        assert_eq!(
            tron_error_from_value(&v),
            Some("this API is unavailable due to config".to_string())
        );
    }

    #[test]
    fn test_error_lowercase_string() {
        // Format: {"error": "message"} - General string errors
        let v = json!({"error": "Invalid address format"});
        assert_eq!(tron_error_from_value(&v), Some("Invalid address format".to_string()));
    }

    #[test]
    fn test_error_jsonrpc_object() {
        // Format: {"error": {"code": ..., "message": ...}} - JSON-RPC 2.0
        let v = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": {
                "code": -32602,
                "message": "Invalid params"
            }
        });
        assert_eq!(tron_error_from_value(&v), Some("-32602: Invalid params".to_string()));
    }

    #[test]
    fn test_error_jsonrpc_code_only() {
        let v = json!({"error": {"code": -32000}});
        assert_eq!(tron_error_from_value(&v), Some("-32000".to_string()));
    }

    #[test]
    fn test_error_nested_result_object() {
        // Format: {"result": {"result": false, "code": "...", "message": "..."}}
        // Used by triggerConstantContract, broadcastTransaction, etc.
        let v = json!({
            "result": {
                "result": false,
                "code": "CONTRACT_VALIDATE_ERROR",
                "message": "Smart contract is not exist."
            }
        });
        assert_eq!(
            tron_error_from_value(&v),
            Some("CONTRACT_VALIDATE_ERROR: Smart contract is not exist.".to_string())
        );
    }

    #[test]
    fn test_error_nested_result_code_only() {
        let v = json!({"result": {"result": false, "code": "SIGERROR"}});
        assert_eq!(tron_error_from_value(&v), Some("SIGERROR".to_string()));
    }

    #[test]
    fn test_error_nested_result_message_only() {
        let v = json!({"result": {"result": false, "message": "Unknown error"}});
        assert_eq!(tron_error_from_value(&v), Some("Unknown error".to_string()));
    }

    #[test]
    fn test_error_nested_result_no_details() {
        let v = json!({"result": {"result": false}});
        assert_eq!(tron_error_from_value(&v), Some("Transaction failed".to_string()));
    }

    #[test]
    fn test_error_nested_result_null_with_code() {
        // Real TRON API response: result is null but has error code
        // This is what triggerConstantContract actually returns
        let v = json!({
            "result": {
                "result": null,
                "code": "CONTRACT_VALIDATE_ERROR",
                "message": "Smart contract is not exist."
            }
        });
        assert_eq!(
            tron_error_from_value(&v),
            Some("CONTRACT_VALIDATE_ERROR: Smart contract is not exist.".to_string())
        );
    }

    #[test]
    fn test_error_nested_result_missing_with_code() {
        // result field is missing entirely but has error code
        let v = json!({
            "result": {
                "code": "SIGERROR",
                "message": "Signature verification failed"
            }
        });
        assert_eq!(
            tron_error_from_value(&v),
            Some("SIGERROR: Signature verification failed".to_string())
        );
    }

    #[test]
    fn test_non_error_nested_result_object_without_code() {
        // result is an object but no code field - not an error
        // This could be a valid response structure from some endpoint
        let v = json!({
            "result": {
                "transaction_id": "abc123",
                "block_number": 12345
            }
        });
        assert_eq!(tron_error_from_value(&v), None);
    }

    #[test]
    fn test_error_toplevel_result_false() {
        // Format: {"result": false, "code": "...", "message": "..."}
        let v = json!({
            "result": false,
            "code": "BANDWIDTH_ERROR",
            "message": "account has insufficient bandwidth"
        });
        assert_eq!(
            tron_error_from_value(&v),
            Some("BANDWIDTH_ERROR: account has insufficient bandwidth".to_string())
        );
    }

    #[test]
    fn test_error_code_message_without_result() {
        // Format: {"code": "...", "message": "..."} without result field
        let v = json!({"code": "TAPOS_ERROR", "message": "Invalid block reference"});
        assert_eq!(
            tron_error_from_value(&v),
            Some("TAPOS_ERROR: Invalid block reference".to_string())
        );
    }

    #[test]
    fn test_error_numeric_code() {
        let v = json!({"code": 1002, "message": "TOO_MANY_REQUESTS"});
        assert_eq!(tron_error_from_value(&v), Some("1002: TOO_MANY_REQUESTS".to_string()));
    }

    // =========================================================================
    // Non-error responses (should return None)
    // =========================================================================

    #[test]
    fn test_non_error_result_true() {
        // {"result": true, ...} is success, not error
        let v = json!({
            "result": true,
            "code": "SUCCESS",
            "message": "Transaction submitted"
        });
        assert_eq!(tron_error_from_value(&v), None);
    }

    #[test]
    fn test_non_error_nested_result_true() {
        // {"result": {"result": true, ...}} is success
        let v = json!({
            "result": {
                "result": true,
                "code": "SUCCESS"
            }
        });
        assert_eq!(tron_error_from_value(&v), None);
    }

    #[test]
    fn test_non_error_account_response() {
        // Normal account response
        let v = json!({
            "address": "41abc123...",
            "balance": 1000000,
            "create_time": 1234567890000i64
        });
        assert_eq!(tron_error_from_value(&v), None);
    }

    #[test]
    fn test_non_error_empty_object() {
        // Empty object (non-existent account)
        let v = json!({});
        assert_eq!(tron_error_from_value(&v), None);
    }

    #[test]
    fn test_non_error_block_response() {
        let v = json!({
            "block_header": {
                "raw_data": {
                    "number": 12345678
                }
            }
        });
        assert_eq!(tron_error_from_value(&v), None);
    }

    #[test]
    fn test_non_error_primitives() {
        // Non-object values return None
        assert_eq!(tron_error_from_value(&json!(null)), None);
        assert_eq!(tron_error_from_value(&json!(123)), None);
        assert_eq!(tron_error_from_value(&json!("string")), None);
        assert_eq!(tron_error_from_value(&json!([1, 2, 3])), None);
    }

    // =========================================================================
    // GetAccountResponse tests
    // =========================================================================

    #[test]
    fn test_account_exists_with_address() {
        let account = GetAccountResponse {
            address: Some("41abc...".to_string()),
            balance: None,
            create_time: None,
            owner_permission: None,
        };
        assert!(account.exists_meaningfully());
    }

    #[test]
    fn test_account_exists_with_balance() {
        let account = GetAccountResponse {
            address: None,
            balance: Some(1000),
            create_time: None,
            owner_permission: None,
        };
        assert!(account.exists_meaningfully());
    }

    #[test]
    fn test_account_exists_with_create_time() {
        let account = GetAccountResponse {
            address: None,
            balance: None,
            create_time: Some(1234567890000),
            owner_permission: None,
        };
        assert!(account.exists_meaningfully());
    }

    #[test]
    fn test_account_not_exists_empty() {
        let account = GetAccountResponse::default();
        assert!(!account.exists_meaningfully());
    }

    #[test]
    fn test_account_not_exists_zero_balance() {
        let account = GetAccountResponse {
            address: None,
            balance: Some(0),
            create_time: None,
            owner_permission: None,
        };
        assert!(!account.exists_meaningfully());
    }

    #[test]
    fn test_account_not_exists_negative_balance() {
        // Edge case: negative balance should be treated as non-existent
        let account = GetAccountResponse {
            address: None,
            balance: Some(-100),
            create_time: None,
            owner_permission: None,
        };
        assert!(!account.exists_meaningfully());
    }

    // =========================================================================
    // Web3RpcError::is_retryable() tests
    // =========================================================================

    #[test]
    fn test_retryable_transport_error() {
        let error = Web3RpcError::Transport("connection refused".to_string());
        assert!(error.is_retryable(), "Transport errors should be retryable");
    }

    #[test]
    fn test_retryable_timeout_error() {
        let error = Web3RpcError::Timeout("request timed out".to_string());
        assert!(error.is_retryable(), "Timeout errors should be retryable");
    }

    #[test]
    fn test_non_retryable_invalid_response() {
        let error = Web3RpcError::InvalidResponse("CONTRACT_VALIDATE_ERROR".to_string());
        assert!(!error.is_retryable(), "InvalidResponse errors should NOT be retryable");
    }

    #[test]
    fn test_non_retryable_internal_error() {
        let error = Web3RpcError::Internal("serialization failed".to_string());
        assert!(!error.is_retryable(), "Internal errors should NOT be retryable");
    }

    #[test]
    fn test_non_retryable_invalid_gas_config() {
        let error = Web3RpcError::InvalidGasApiConfig("missing url".to_string());
        assert!(
            !error.is_retryable(),
            "InvalidGasApiConfig errors should NOT be retryable"
        );
    }

    #[test]
    fn test_non_retryable_protocol_not_supported() {
        let error = Web3RpcError::ProtocolNotSupported("NFT not supported".to_string());
        assert!(
            !error.is_retryable(),
            "ProtocolNotSupported errors should NOT be retryable"
        );
    }

    #[test]
    fn test_non_retryable_num_conversion_error() {
        let error = Web3RpcError::NumConversError("overflow".to_string());
        assert!(!error.is_retryable(), "NumConversError errors should NOT be retryable");
    }

    #[test]
    fn test_non_retryable_no_such_coin() {
        let error = Web3RpcError::NoSuchCoin {
            coin: "INVALID".to_string(),
        };
        assert!(!error.is_retryable(), "NoSuchCoin errors should NOT be retryable");
    }
}
