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
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// Timeout for individual TRON API requests.
pub const TRON_API_TIMEOUT: Duration = Duration::from_secs(10);

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
    pub async fn post<T: Serialize, R: for<'de> Deserialize<'de>>(&self, path: &str, body: &T) -> Web3RpcResult<R> {
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

        // Check for TronGrid error responses (200 OK but error payload)
        if let Ok(error_check) = serde_json::from_slice::<serde_json::Value>(&response_bytes) {
            // TronGrid can return {"Error": "..."} or {"error": "..."} or {"code": ..., "message": ...}
            if let Some(error_msg) = error_check.get("Error").and_then(|v| v.as_str()) {
                return Err(Web3RpcError::InvalidResponse(format!("TRON API error: {error_msg}")).into());
            }
            if let Some(error_msg) = error_check.get("error").and_then(|v| v.as_str()) {
                return Err(Web3RpcError::InvalidResponse(format!("TRON API error: {error_msg}")).into());
            }
            if error_check.get("code").is_some() {
                if let Some(msg) = error_check.get("message").and_then(|v| v.as_str()) {
                    return Err(Web3RpcError::InvalidResponse(format!("TRON API error: {msg}")).into());
                }
            }
        }

        serde_json::from_slice(&response_bytes)
            .map_err(|e| Web3RpcError::InvalidResponse(format!("Failed to parse response: {e}")).into())
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn send_request(&self, uri: &Uri, body: Vec<u8>) -> Web3RpcResult<Vec<u8>> {
        use common::executor::Timer;
        use futures::future::{select, Either};
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

        let timeout = Timer::sleep(TRON_API_TIMEOUT.as_secs_f64());
        let req_fut = Box::pin(slurp_req(req));

        match select(req_fut, timeout).await {
            Either::Left((Ok((status, _headers, response_body)), _)) => {
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
            Either::Left((Err(e), _)) => Err(Web3RpcError::Transport(format!("Request failed: {e}")).into()),
            Either::Right(_) => Err(Web3RpcError::Timeout(format!("Request to {} timed out", uri)).into()),
        }
    }

    #[cfg(target_arch = "wasm32")]
    async fn send_request(&self, uri: &Uri, body: Vec<u8>) -> Web3RpcResult<Vec<u8>> {
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

        let (status_code, response_str) = request
            .request_str()
            .await
            .map_err(|e| Web3RpcError::Transport(format!("WASM fetch failed: {e:?}")))?;

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
                    last_error = e.into_inner();
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
