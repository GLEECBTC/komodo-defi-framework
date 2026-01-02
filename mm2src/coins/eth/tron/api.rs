//! TRON HTTP API client for wallet operations.
//!
//! Implements the minimal TRON API endpoints needed for HD activation and balance queries:
//! - `/wallet/getnowblock` - current block number
//! - `/wallet/getaccount` - account info (balance, existence)
//!
//! # TODO: RPC Pool Trait Refactoring
//!
//! The current structure has node rotation logic duplicated between EVM (`try_rpc_send` in
//! `eth_rpc.rs`) and TRON (`try_clients` here). This should be unified via a common trait:
//!
//! ```ignore
//! #[async_trait]
//! pub trait RpcPool: Send + Sync + Clone {
//!     type Client: Send + Sync + Clone;
//!     type Error;
//!
//!     async fn try_nodes<F, Fut, T>(&self, op: F) -> Result<T, Self::Error>
//!     where
//!         F: Fn(Self::Client) -> Fut + Send + Sync,
//!         Fut: Future<Output = Result<T, Self::Error>> + Send;
//!
//!     fn is_retryable(error: &Self::Error) -> bool;
//! }
//! ```
//!
//! See `docs/plans/chain-rpc-client-refactor.md` for the full refactoring plan.

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

/// Check if a TRON error message indicates a transient condition that should be retried.
///
/// Based on TRON's `Return.response_code` enum:
/// <https://github.com/tronprotocol/java-tron/blob/1e35f79/protocol/src/main/protos/api/api.proto#L1041-L1057>
///
/// Transient codes:
/// - `SERVER_BUSY` (code 9) - node's transaction pending pool is at capacity
/// - `NO_CONNECTION` (code 10) - no active peer connections
/// - `NOT_ENOUGH_EFFECTIVE_CONNECTION` (code 11) - insufficient peer connections
/// - `BLOCK_UNSOLIDIFIED` (code 12) - blockchain not fully synchronized
/// - Rate limiting: "lack of computing resources" message
///
/// These errors become `InvalidResponse` which `Web3RpcError::is_retryable()` treats as permanent.
/// This function provides TRON-specific retry classification.
pub fn is_retryable_tron_error(error_msg: &str) -> bool {
    let lower = error_msg.to_lowercase();

    // TRON-specific transient error codes (from java-tron Return.response_code)
    if lower.contains("server_busy") || lower.contains("server busy") {
        return true;
    }
    if lower.contains("no_connection") || lower.contains("no connection") {
        return true;
    }
    if lower.contains("not_enough_effective_connection") || lower.contains("not enough effective connection") {
        return true;
    }
    if lower.contains("block_unsolidified") || lower.contains("block unsolidified") {
        return true;
    }

    // Rate limiting from RateLimiterServlet
    if lower.contains("lack of computing resources") {
        return true;
    }

    false
}

/// Detects TRON API error payloads and extracts the error message.
/// Returns `Some(message)` if the response indicates an error, `None` otherwise.
///
/// TRON API error formats (HTTP API):
/// - `{"Error": "message"}` - Generic servlet errors
///   <https://github.com/tronprotocol/java-tron/blob/1e35f79/framework/src/main/java/org/tron/core/services/http/Util.java#L90-L94>
/// - `{"error": {"code": ..., "message": ...}}` - JSON-RPC 2.0 errors (for future use)
/// - `{"result": false, "code": "...", "message": "..."}` - Return message (broadcasttransaction)
///   <https://github.com/tronprotocol/java-tron/blob/1e35f79/protocol/src/main/protos/api/api.proto#L1040-L1062>
/// - `{"result": {"result": false, "code": "...", "message": "..."}}` - Nested Return (triggersmartcontract, estimateenergy)
///   <https://developers.tron.network/reference/estimateenergy>
///
/// Non-error: `{}` for non-existent accounts (<https://developers.tron.network/reference/getaccount-1>)
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

    // Format: {"Error": "message"} - Generic servlet errors (Util.printErrorMsg, JsonFormat.printErrorMsg)
    if let Some(msg) = obj.get("Error").and_then(|v| v.as_str()) {
        return Some(msg.to_string());
    }

    // Format: {"error": {"code": ..., "message": ...}} - JSON-RPC 2.0 errors (for future use)
    if let Some(error_obj) = obj.get("error").and_then(|v| v.as_object()) {
        let code = error_obj.get("code").map(&value_to_string);
        let message = error_obj.get("message").map(&value_to_string);
        return Some(format_error(code, message, "JSON-RPC error"));
    }

    // Format: {"result": {"result": false, "code": "...", "message": "..."}} - Nested Return
    // Used by: TransactionExtention (triggersmartcontract), EstimateEnergyMessage (estimateenergy)
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

    // Format: {"result": false, "code": "...", "message": "..."} - Top-level Return (broadcasttransaction)
    if matches!(obj.get("result").and_then(|v| v.as_bool()), Some(false)) {
        let code = obj.get("code").map(&value_to_string);
        let message = obj.get("message").map(&value_to_string);
        return Some(format_error(code, message, "TRON API returned result=false"));
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
    /// Get the current block.
    pub async fn get_now_block(&self) -> Web3RpcResult<GetNowBlockResponse> {
        self.post("/wallet/getnowblock", &GetNowBlockRequest {}).await
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
    /// Only retryable errors (transport failures, timeouts, TRON transient errors) trigger
    /// fallback to the next node. Non-retryable errors (invalid responses, API errors like
    /// "contract doesn't exist") return immediately since they would fail on any node.
    ///
    /// Note: Holds mutex across await for consistency with EVM's `try_rpc_send` pattern.
    async fn try_clients<F, Fut, T>(&self, op: F) -> Web3RpcResult<T>
    where
        F: Fn(TronHttpClient) -> Fut,
        Fut: std::future::Future<Output = Web3RpcResult<T>>,
    {
        let mut clients = self.clients.lock().await;

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
                    // Retry on transport/timeout errors (generic transient issues)
                    // OR on TRON-specific transient errors (server busy, no connection, etc.).
                    let is_retryable = inner.is_retryable()
                        || matches!(&inner, Web3RpcError::InvalidResponse(msg) if is_retryable_tron_error(msg));
                    if !is_retryable {
                        return Err(inner.into());
                    }
                    last_error = inner;
                },
            }
        }

        Err(last_error.into())
    }

    /// Get account information with node rotation.
    pub async fn get_account(&self, address: &TronAddress) -> Web3RpcResult<GetAccountResponse> {
        self.try_clients(|client| {
            let addr = *address;
            async move { client.get_account(&addr).await }
        })
        .await
    }
}

// ============================================================================
// ChainRpcOps implementation for TronApiClient
// ============================================================================

use crate::eth::chain_rpc::ChainRpcOps;
use async_trait::async_trait;
use mm2_err_handle::prelude::MmError;

#[async_trait]
impl ChainRpcOps for TronApiClient {
    type Error = MmError<Web3RpcError>;
    type Address = TronAddress;
    type Balance = U256;

    async fn current_block(&self) -> Result<u64, Self::Error> {
        self.try_clients(|client| async move {
            let response = client.get_now_block().await?;
            let block_header = response.block_header.ok_or_else(|| {
                Web3RpcError::InvalidResponse("Missing block_header in getnowblock response".to_string())
            })?;
            let block_number = block_header.raw_data.number;
            if block_number < 0 {
                return Err(
                    Web3RpcError::InvalidResponse(format!("Invalid negative block number: {block_number}")).into(),
                );
            }
            Ok(block_number as u64)
        })
        .await
    }

    async fn balance_native(&self, address: Self::Address) -> Result<Self::Balance, Self::Error> {
        self.try_clients(|client| {
            let addr = address;
            async move {
                let account = client.get_account(&addr).await?;
                let balance = account.balance.unwrap_or(0).max(0) as u64;
                Ok(U256::from(balance))
            }
        })
        .await
    }

    async fn is_address_used_basic(&self, address: Self::Address) -> Result<bool, Self::Error> {
        self.try_clients(|client| {
            let addr = address;
            async move {
                let account = client.get_account(&addr).await?;
                Ok(account.exists_meaningfully())
            }
        })
        .await
    }
}

impl std::fmt::Debug for TronApiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TronApiClient").finish_non_exhaustive()
    }
}
