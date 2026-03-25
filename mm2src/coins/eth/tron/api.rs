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

use super::fee::{TronAccountResources, TronChainPrices};
use super::{trc20_transfer_tokens, TronAddress};
use crate::eth::{Web3RpcError, Web3RpcResult};

use common::{APPLICATION_JSON, PROXY_REQUEST_EXPIRATION_SEC, X_AUTH_PAYLOAD};
use ethereum_types::U256;
use http::header::CONTENT_TYPE;
use http::Uri;
use mm2_p2p::Keypair;
use proxy_signature::RawMessage;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value as Json;
use serde_json::{self as json};
use std::convert::TryInto;
use std::sync::Arc;
use std::time::Duration;

/// Timeout for individual TRON API requests.
pub const TRON_API_TIMEOUT: Duration = Duration::from_secs(10);

// ============================================================================
// TRON Error Classification
// ============================================================================

/// Structured TRON API error payload with code and message.
///
/// Separating code and message enables proper retry classification at the source.
#[derive(Debug)]
struct TronErrorPayload {
    code: Option<String>,
    message: String,
}

impl TronErrorPayload {
    /// Check if this error indicates a transient condition that should be retried.
    ///
    /// Based on TRON's `Return.response_code` enum:
    /// <https://github.com/tronprotocol/java-tron/blob/1e35f79/protocol/src/main/protos/api/api.proto#L1041-L1057>
    ///
    /// Transient codes (retryable):
    /// - `SERVER_BUSY` (code 9) - node's transaction pending pool is at capacity
    /// - `NO_CONNECTION` (code 10) - no active peer connections
    /// - `NOT_ENOUGH_EFFECTIVE_CONNECTION` (code 11) - insufficient peer connections
    /// - `BLOCK_UNSOLIDIFIED` (code 12) - blockchain not fully synchronized
    /// - Rate limiting: "lack of computing resources" message
    ///
    /// All other codes are permanent (not retryable): SIGERROR, CONTRACT_VALIDATE_ERROR,
    /// CONTRACT_EXE_ERROR, BANDWITH_ERROR, DUP_TRANSACTION_ERROR, TAPOS_ERROR,
    /// TOO_BIG_TRANSACTION_ERROR, TRANSACTION_EXPIRATION_ERROR, OTHER_ERROR.
    ///
    /// # Why string codes instead of numeric codes
    ///
    /// TRON's HTTP API serializes enum values as string names via `JsonFormat.printToString`.
    /// Example response: `{"code": "SERVER_BUSY", "message": "..."}`.
    /// See: <https://github.com/tronprotocol/java-tron/blob/1e35f79/framework/src/main/java/org/tron/core/services/http/JsonFormat.java#L378-L382>
    fn is_retryable(&self) -> bool {
        const RETRYABLE_CODES: &[&str] = &[
            "SERVER_BUSY",
            "NO_CONNECTION",
            "NOT_ENOUGH_EFFECTIVE_CONNECTION",
            "BLOCK_UNSOLIDIFIED",
        ];

        // Rate limiting message from RateLimiterServlet (not a response_code, but a servlet error).
        // See: https://github.com/tronprotocol/java-tron/blob/1e35f79/framework/src/main/java/org/tron/core/services/http/RateLimiterServlet.java#L114
        const RATE_LIMIT_MSG: &str = "lack of computing resources";

        if let Some(c) = &self.code {
            if RETRYABLE_CODES.contains(&c.as_str()) {
                return true;
            }
        }

        self.message.contains(RATE_LIMIT_MSG)
    }
}

impl std::fmt::Display for TronErrorPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.code {
            Some(code) => write!(f, "{}: {}", code, self.message),
            None => write!(f, "{}", self.message),
        }
    }
}

/// Detects TRON API error payloads and extracts structured error information.
/// Returns `Some(TronErrorPayload)` if the response indicates an error, `None` otherwise.
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
fn tron_error_from_value(v: &Json) -> Option<TronErrorPayload> {
    let obj = v.as_object()?;

    // Helper to convert JSON values to string (handles string, number, or fallback to JSON repr)
    let value_to_string = |v: &Json| -> String {
        match v {
            Json::String(s) => s.clone(),
            Json::Number(n) => n.to_string(),
            other => other.to_string(),
        }
    };

    // Format: {"Error": "message"} - Generic servlet errors (Util.printErrorMsg, JsonFormat.printErrorMsg)
    if let Some(msg) = obj.get("Error").and_then(|v| v.as_str()) {
        return Some(TronErrorPayload {
            code: None,
            message: msg.to_string(),
        });
    }

    // Format: {"error": {"code": ..., "message": ...}} - JSON-RPC 2.0 errors (for future use)
    if let Some(error_obj) = obj.get("error").and_then(|v| v.as_object()) {
        let code = error_obj.get("code").map(&value_to_string);
        let message = error_obj
            .get("message")
            .map(&value_to_string)
            .unwrap_or_else(|| "JSON-RPC error".to_string());
        return Some(TronErrorPayload { code, message });
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
            let message = result_obj
                .get("message")
                .map(&value_to_string)
                .unwrap_or_else(|| "Transaction failed".to_string());
            return Some(TronErrorPayload { code, message });
        }
    }

    // Format: {"result": false, "code": "...", "message": "..."} - Top-level Return (broadcasttransaction)
    if matches!(obj.get("result").and_then(|v| v.as_bool()), Some(false)) {
        let code = obj.get("code").map(&value_to_string);
        let message = obj
            .get("message")
            .map(&value_to_string)
            .unwrap_or_else(|| "TRON API returned result=false".to_string());
        return Some(TronErrorPayload { code, message });
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
    proxy_sign_keypair: Option<Arc<Keypair>>,
}

impl TronHttpClient {
    pub fn new(node: TronHttpNode, proxy_sign_keypair: Option<Arc<Keypair>>) -> Self {
        Self {
            node,
            proxy_sign_keypair,
        }
    }

    /// Builds the proxy signature JSON string for komodo proxy nodes.
    /// Returns `None` when this node is not a proxy.
    fn proxy_sign_json(&self, uri: &Uri, body_len: usize) -> Web3RpcResult<Option<String>> {
        if !self.node.komodo_proxy {
            return Ok(None);
        }
        let keypair = self.proxy_sign_keypair.as_ref().ok_or_else(|| {
            Web3RpcError::Internal("Proxy node requires signing keypair but none provided".to_string())
        })?;
        let proxy_sign = RawMessage::sign(keypair, uri, body_len, PROXY_REQUEST_EXPIRATION_SEC)
            .map_err(|e| Web3RpcError::Internal(format!("Proxy signing failed: {e}")))?;
        let json_str = json::to_string(&proxy_sign).map_err(|e| Web3RpcError::Internal(e.to_string()))?;
        Ok(Some(json_str))
    }

    /// Send a POST request to the TRON API.
    ///
    /// Error classification at source:
    /// - **Retryable**: malformed JSON, unexpected payload structure, transient TRON errors
    ///   (SERVER_BUSY, NO_CONNECTION, etc.), rate limiting. These trigger node rotation.
    /// - **Non-retryable**: permanent TRON errors like CONTRACT_VALIDATE_ERROR, SIGERROR, etc.
    ///   These would fail on any node.
    /// - **Internal**: programming errors (invalid URI, serialization bugs). Not retryable.
    pub async fn post<T: Serialize, R: DeserializeOwned>(&self, path: &str, body: &T) -> Web3RpcResult<R> {
        // Build URI, avoiding double slashes
        let base = self.node.uri.to_string();
        let base = base.trim_end_matches('/');
        let path = path.trim_start_matches('/');
        let uri_str = format!("{}/{}", base, path);
        let uri: Uri = uri_str
            .parse()
            .map_err(|e| Web3RpcError::Internal(format!("Invalid URI: {e}")))?;

        let body_bytes = json::to_vec(body).map_err(|e| Web3RpcError::Internal(e.to_string()))?;
        let response_bytes = self.send_request(&uri, body_bytes).await?;

        // Parse JSON once. Malformed JSON = faulty node, try another.
        let response_json: Json = json::from_slice(&response_bytes)
            .map_err(|e| Web3RpcError::BadResponse(format!("TRON node returned malformed JSON: {e}")))?;

        // Check for TRON error payloads (200 OK but error content).
        // Classify transient errors as retryable; permanent rejections as non-retryable.
        if let Some(tron_err) = tron_error_from_value(&response_json) {
            if tron_err.is_retryable() {
                return Err(Web3RpcError::Transport(format!("TRON API transient error: {tron_err}")).into());
            } else {
                return Err(Web3RpcError::RemoteError {
                    code: tron_err.code,
                    message: tron_err.message,
                }
                .into());
            }
        }

        // Convert Json to typed response. Unexpected structure = faulty node, try another.
        json::from_value(response_json)
            .map_err(|e| Web3RpcError::BadResponse(format!("TRON node returned unexpected payload: {e}")).into())
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

        if let Some(proxy_json) = self.proxy_sign_json(uri, body.len())? {
            let header_value = proxy_json
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

        if let Some(proxy_json) = self.proxy_sign_json(uri, body.len())? {
            request = request.header(X_AUTH_PAYLOAD, &proxy_json);
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
    /// Computed block identifier (not in protobuf, added by the HTTP servlet layer).
    /// First 8 bytes duplicate the block number (big-endian) for sortability, remaining 24 bytes
    /// are from SHA256 of `block_header.raw_data`. We only need bytes `[8..16]` for TAPOS
    /// (`ref_block_hash`). The block number itself comes from `block_header.raw_data.number`.
    /// Deserialized from a 64-char hex string.
    /// See [`generateBlockId`](https://github.com/tronprotocol/java-tron/blob/1e35f79/common/src/main/java/org/tron/common/utils/Sha256Hash.java#L252-L258).
    #[serde(rename = "blockID", deserialize_with = "deserialize_block_id")]
    pub block_id: [u8; 32],
    /// Block header containing raw block data (number, timestamp, etc.).
    pub block_header: BlockHeader,
}

/// Block header from `/wallet/getnowblock` response.
#[derive(Deserialize, Debug)]
pub struct BlockHeader {
    pub raw_data: BlockRawData,
}

/// Raw block data from a TRON block header.
#[derive(Deserialize, Debug)]
pub struct BlockRawData {
    /// Block height.
    pub number: i64,
    /// Block timestamp in milliseconds since Unix epoch.
    #[serde(default)]
    pub timestamp: i64,
}

impl GetNowBlockResponse {
    /// Validate block number and timestamp are sane, return the header with block number as `u64`.
    ///
    /// A negative block number or non-positive timestamp means the node returned bad data.
    /// Returns `BadResponse` (retryable) to trigger rotation to another node.
    fn validated_header(&self) -> Web3RpcResult<(&BlockHeader, u64)> {
        let number = self.block_header.raw_data.number;
        if number < 0 {
            return Err(Web3RpcError::BadResponse(format!(
                "TRON node returned invalid negative block number: {number}"
            ))
            .into());
        }
        let timestamp = self.block_header.raw_data.timestamp;
        if timestamp <= 0 {
            return Err(
                Web3RpcError::BadResponse(format!("TRON node returned invalid block timestamp: {timestamp}")).into(),
            );
        }
        Ok((&self.block_header, number as u64))
    }
}

/// Deserialize a hex string into `[u8; 32]`.
///
/// Handles TRON's `blockID` field: a 64-char hex string (no `0x` prefix).
/// Returns an error if the hex is invalid or not exactly 32 bytes.
fn deserialize_block_id<'de, D>(deserializer: D) -> Result<[u8; 32], D::Error>
where
    D: Deserializer<'de>,
{
    let hex_str = String::deserialize(deserializer)?;
    let hex_str = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
    let bytes = hex::decode(hex_str).map_err(serde::de::Error::custom)?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|v: Vec<u8>| serde::de::Error::custom(format!("blockID must be 32 bytes, got {}", v.len())))?;
    Ok(arr)
}

/// Validated block data needed for TAPOS (Transaction as Proof of Stake) reference.
///
/// TRON transactions include a reference to a recent block (TAPOS) for replay protection:
/// - `ref_block_bytes`: last 2 bytes of `number` (big-endian) → `number.to_be_bytes()[6..8]`
/// - `ref_block_hash`: bytes 8..16 of `block_id` (the SHA256 portion)
///
/// TAPOS validity window is 65,536 blocks (~54 hours).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaposBlockData {
    /// Block height.
    pub number: u64,
    /// Full 32-byte block identifier.
    pub block_id: [u8; 32],
    /// Block timestamp in milliseconds since Unix epoch.
    pub timestamp: i64,
}

/// Request body for `/wallet/getaccount`.
#[derive(Serialize)]
struct GetAccountRequest<'a> {
    address: &'a TronAddress,
    /// When `true`, addresses in request/response use Base58Check format (`T...`);
    /// when `false`, hex format (`41...`).
    visible: bool,
}

/// Empty object marker for TRON API responses.
///
/// MUST use `deny_unknown_fields`; otherwise arbitrary error payloads
/// (e.g. `{ "code": "...", "message": "..." }`) could deserialize as `NoAccount` and silently
/// bypass retry/rotation logic.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct TronEmptyObject {}

/// Response from `/wallet/getaccount`.
///
/// TRON returns `{}` for non-existent accounts, or account data for existing ones.
/// Using untagged enum: `Account` (with required `address`) is tried first; if it fails,
/// `NoAccount` matches the empty object.
///
/// # Proto3 serialization
///
/// TRON uses proto3 where default values (0, empty) are omitted from JSON.
/// - `address`: Always non-empty for existing accounts (used as DB key), so always serialized.
/// - `balance`: Could be 0 for new accounts, so might be omitted. We use `#[serde(default)]`.
/// - `create_time`: Set on creation, but omitted if 0. We use `#[serde(default)]`.
///
/// # Extensibility
///
/// `Account` does NOT use `deny_unknown_fields` so additional fields returned by TRON
/// (like `net_usage`, `assetV2`, `frozenV2`, `owner_permission`, etc.) are silently ignored.
/// Add fields here as needed for future functionality.
///
/// See: <https://developers.tron.network/reference/getaccount-1>
#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum GetAccountResponse {
    /// Existing account. `address` is always present (non-empty bytes in protobuf).
    Account {
        /// Account address in hex format.
        address: String,
        /// Balance in SUN (1 TRX = 1,000,000 SUN). Defaults to 0 if omitted (proto3).
        #[serde(default)]
        balance: u64,
        /// Account creation timestamp in milliseconds. Defaults to 0 if omitted.
        #[serde(default)]
        create_time: i64,
    },
    /// Empty object `{}` - account doesn't exist on chain.
    NoAccount(TronEmptyObject),
}

impl GetAccountResponse {
    /// Returns true if the account exists on chain (used for HD gap-limit scanning).
    pub fn exists_meaningfully(&self) -> bool {
        matches!(self, GetAccountResponse::Account { .. })
    }
}

/// Request body for `/wallet/triggerconstantcontract`.
///
/// Used to call constant (view/pure) functions on smart contracts without broadcasting.
/// For TRC20: `balanceOf(address)`, `decimals()`, `name()`, `symbol()`.
///
/// Note: Uses `visible: true` so addresses serialize as Base58 (TronAddress default).
#[derive(Serialize)]
struct TriggerConstantContractRequest<'a> {
    /// Caller address (required by TRON even for constant calls).
    owner_address: &'a TronAddress,
    /// Contract address to call.
    contract_address: &'a TronAddress,
    /// Function signature, e.g. "balanceOf(address)" or "decimals()".
    function_selector: &'a str,
    /// ABI-encoded parameters (hex string, no 0x prefix, excludes 4-byte selector).
    parameter: &'a str,
    /// If true, addresses are Base58 format; if false, hex with 0x41 prefix.
    /// Must be true since TronAddress serializes to Base58.
    visible: bool,
}

/// Partial response from TRON's `triggerconstantcontract` endpoint.
///
/// The endpoint returns a `TransactionExtention` protobuf message containing the
/// full simulated transaction, logs, and execution results. We only deserialize the
/// fields needed for fee estimation (`energy_used`) and balance queries (`constant_result`)
/// and omit:
/// - `transaction`: The simulated transaction object (not broadcast for constant calls)
/// - `energy_penalty`: Additional energy penalty for certain contract patterns
/// - `result`: Success/failure indicator (handled by `tron_error_from_value()` before deserialization)
/// - `txid`: Transaction hash of the simulated transaction
///
/// Error responses are handled by `tron_error_from_value()` before deserialization.
#[derive(Deserialize, Debug)]
pub struct TriggerConstantContractResponse {
    /// ABI-encoded return values (protobuf: `repeated bytes`).
    /// TRON serializes as hex strings (no 0x prefix). For single return value functions,
    /// this contains one element. Decoded via `hex::decode` in `parse_constant_result_u256`.
    #[serde(default)]
    pub constant_result: Vec<String>,

    /// Energy consumed by the TVM simulation (constant calls execute in a sandbox
    /// without broadcasting, so this is precise, not a rough estimate).
    /// Used to predict the fee for the actual TRC20 `transfer()` transaction.
    #[serde(default)]
    pub energy_used: Option<u64>,
}

/// Request body for `/wallet/getchainparameters`.
#[derive(Serialize)]
struct GetChainParametersRequest {}

/// Response from `/wallet/getchainparameters`.
///
/// The HTTP API uses camelCase `chainParameter` in live responses.
#[derive(Clone, Debug, Deserialize)]
struct GetChainParametersResponse {
    #[serde(rename = "chainParameter", alias = "chain_parameter")]
    chain_parameter: Vec<ChainParameterEntry>,
}

#[derive(Clone, Debug, Deserialize)]
struct ChainParameterEntry {
    key: String,
    /// Some chain parameters are boolean-like flags and omit `value` in JSON.
    value: Option<i64>,
}

/// Request body for `/wallet/getaccountresource`.
#[derive(Serialize)]
struct GetAccountResourceRequest<'a> {
    address: &'a TronAddress,
    visible: bool,
}

/// Request body for `/wallet/broadcasthex`.
#[derive(Serialize)]
struct BroadcastHexRequest<'a> {
    transaction: &'a str,
}

/// Response from `/wallet/broadcasthex` on success.
///
/// Error responses (`result: false`) are intercepted by `tron_error_from_value()` before
/// deserialization, so this struct only captures the success-path field.
/// `txid` is always present — it is computed from the transaction hash before broadcast.
#[derive(Debug, Deserialize)]
pub struct BroadcastHexResponse {
    pub txid: String,
}

/// Request body for transaction lookup by hash.
#[derive(Serialize)]
struct TxByIdRequest<'a> {
    value: &'a str,
}

/// Response from `/wallet/gettransactionbyid`.
#[derive(Debug, Deserialize)]
pub struct GetTransactionByIdResponse {
    #[serde(rename = "txID")]
    pub tx_id: String,
    pub raw_data: TronTxRawData,
}

#[derive(Debug, Deserialize)]
pub struct TronTxRawData {
    pub contract: Vec<TronTxContract>,
}

#[derive(Debug, Deserialize)]
pub struct TronTxContract {
    #[serde(rename = "type")]
    pub contract_type: String,
    pub parameter: TronTxContractParameter,
}

#[derive(Debug, Deserialize)]
pub struct TronTxContractParameter {
    pub value: TronTxContractValue,
}

#[derive(Debug, Deserialize)]
pub struct TronTxContractValue {
    pub contract_address: Option<String>,
    pub data: Option<String>,
}

/// Response from `/wallet/gettransactioninfobyid`.
/// Request struct: [`TxByIdRequest`] (shared with `gettransactionbyid`).
#[derive(Debug, Deserialize)]
pub struct GetTransactionInfoByIdResponse {
    pub id: String,
    #[serde(rename = "blockNumber")]
    pub block_number: u64,
    pub receipt: TronTxReceipt,
    #[serde(default)]
    pub fee: Option<u64>,
}

#[derive(Debug, Deserialize)]
pub struct TronTxReceipt {
    #[serde(default)]
    pub energy_usage_total: u64,
    #[serde(default)]
    pub net_fee: u64,
    #[serde(default)]
    pub energy_fee: u64,
    pub result: String,
}

fn parse_chain_prices_sun(chain_params: &GetChainParametersResponse) -> Web3RpcResult<TronChainPrices> {
    let mut bandwidth_price_sun = None;
    let mut energy_price_sun = None;
    let mut create_new_account_fee_sun = None;
    let mut create_account_bandwidth_fee_sun = None;
    let mut create_new_account_bandwidth_rate = None;

    for param in &chain_params.chain_parameter {
        match param.key.as_str() {
            "getTransactionFee" => bandwidth_price_sun = param.value.and_then(|v| v.try_into().ok()),
            "getEnergyFee" => energy_price_sun = param.value.and_then(|v| v.try_into().ok()),
            "getCreateNewAccountFeeInSystemContract" => {
                create_new_account_fee_sun = param.value.and_then(|v| v.try_into().ok())
            },
            "getCreateAccountFee" => create_account_bandwidth_fee_sun = param.value.and_then(|v| v.try_into().ok()),
            "getCreateNewAccountBandwidthRate" => {
                create_new_account_bandwidth_rate = param.value.and_then(|v| v.try_into().ok())
            },
            _ => {},
        }
    }

    let bandwidth_price_sun = bandwidth_price_sun.ok_or_else(|| {
        Web3RpcError::BadResponse("Missing or invalid getTransactionFee in getchainparameters response".to_owned())
    })?;
    let energy_price_sun = energy_price_sun.ok_or_else(|| {
        Web3RpcError::BadResponse("Missing or invalid getEnergyFee in getchainparameters response".to_owned())
    })?;
    // Account creation fees default to 0 if missing — some TRON RPC providers may
    // not return these parameters, and they are only needed for transfers to new addresses.
    let create_new_account_fee_sun = create_new_account_fee_sun.unwrap_or(0);
    let create_account_bandwidth_fee_sun = create_account_bandwidth_fee_sun.unwrap_or(0);
    // Default to 1 when missing/invalid to match java-tron behavior.
    let create_new_account_bandwidth_rate = create_new_account_bandwidth_rate.filter(|rate| *rate > 0).unwrap_or(1);

    if bandwidth_price_sun == 0 || energy_price_sun == 0 {
        return Err(Web3RpcError::BadResponse(
            "Invalid chain prices: getTransactionFee/getEnergyFee must be greater than zero".to_owned(),
        )
        .into());
    }

    Ok(TronChainPrices {
        bandwidth_price_sun,
        energy_price_sun,
        create_new_account_fee_sun,
        create_account_bandwidth_fee_sun,
        create_new_account_bandwidth_rate,
    })
}

// ============================================================================
// High-level TRON API methods
// ============================================================================

impl TronHttpClient {
    /// Get the current block.
    pub async fn get_now_block(&self) -> Web3RpcResult<GetNowBlockResponse> {
        self.post("/wallet/getnowblock", &GetNowBlockRequest {}).await
    }

    /// Get validated block data for TAPOS transaction references.
    ///
    /// Calls `/wallet/getnowblock` and validates that `blockID`, block number, and timestamp
    /// are all present and sane. Returns `BadResponse` (retryable) for missing/invalid data.
    pub async fn get_block_for_tapos(&self) -> Web3RpcResult<TaposBlockData> {
        let response = self.get_now_block().await?;
        let (header, number) = response.validated_header()?;
        Ok(TaposBlockData {
            number,
            block_id: response.block_id,
            timestamp: header.raw_data.timestamp,
        })
    }

    /// Get account information for a TRON address.
    pub async fn get_account(&self, address: &TronAddress) -> Web3RpcResult<GetAccountResponse> {
        let request = GetAccountRequest { address, visible: true };
        self.post("/wallet/getaccount", &request).await
    }

    /// Call a constant (view/pure) function on a smart contract.
    ///
    /// This is the low-level method; for TRC20-specific calls, use `TronApiClient::trc20_balance_of`
    /// or `TronApiClient::trc20_decimals` which handle ABI encoding and node rotation.
    pub async fn trigger_constant_contract(
        &self,
        owner_address: &TronAddress,
        contract_address: &TronAddress,
        function_selector: &str,
        parameter: &str,
    ) -> Web3RpcResult<TriggerConstantContractResponse> {
        let request = TriggerConstantContractRequest {
            owner_address,
            contract_address,
            function_selector,
            parameter,
            visible: true,
        };
        self.post("/wallet/triggerconstantcontract", &request).await
    }

    /// Fetch TRON chain fee prices from `/wallet/getchainparameters`.
    ///
    /// Returns `BadResponse` (retryable) when required fee parameters are missing, malformed,
    /// negative, or zero so callers can rotate to the next node.
    pub async fn get_chain_prices(&self) -> Web3RpcResult<TronChainPrices> {
        let response: GetChainParametersResponse = self
            .post("/wallet/getchainparameters", &GetChainParametersRequest {})
            .await?;
        parse_chain_prices_sun(&response)
    }

    /// Get account resource usage and limits.
    ///
    /// Returns `TronAccountResources` with bandwidth and energy quotas.
    /// Empty `{}` responses (unactivated accounts) produce all-zero resources.
    pub async fn get_account_resource(&self, address: &TronAddress) -> Web3RpcResult<TronAccountResources> {
        let request = GetAccountResourceRequest { address, visible: true };
        self.post("/wallet/getaccountresource", &request).await
    }

    /// Broadcast a signed transaction (hex-encoded protobuf bytes).
    ///
    /// Error responses (`result: false`) are handled by `tron_error_from_value()` in `post()`.
    pub async fn broadcast_hex(&self, tx_hex: &str) -> Web3RpcResult<BroadcastHexResponse> {
        self.post("/wallet/broadcasthex", &BroadcastHexRequest { transaction: tx_hex })
            .await
    }

    /// Get raw transaction by hash.
    pub async fn get_transaction_by_id(&self, tx_id: &str) -> Web3RpcResult<GetTransactionByIdResponse> {
        self.post("/wallet/gettransactionbyid", &TxByIdRequest { value: tx_id })
            .await
    }

    /// Get transaction execution info/receipt by hash.
    pub async fn get_transaction_info_by_id(&self, tx_id: &str) -> Web3RpcResult<GetTransactionInfoByIdResponse> {
        self.post("/wallet/gettransactioninfobyid", &TxByIdRequest { value: tx_id })
            .await
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
    /// Retryability is determined by `Web3RpcError::is_retryable()`:
    /// - **Retryable** (`Transport`, `Timeout`, `BadResponse`): try next node. Includes network failures/timeouts,
    ///   malformed JSON/unexpected payloads, and transient TRON conditions (SERVER_BUSY, etc.).
    /// - **Non-retryable** (`RemoteError`, `InvalidResponse`, `Internal`, etc.): fail immediately. Includes
    ///   deterministic TRON rejections (CONTRACT_VALIDATE_ERROR, SIGERROR, etc.) and programming errors.
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

        let mut last_retryable: Option<Web3RpcError> = None;

        for (i, client) in clients.clone().into_iter().enumerate() {
            match op(client).await {
                Ok(result) => {
                    // Rotate successful client to front
                    clients.rotate_left(i);
                    return Ok(result);
                },
                Err(e) => {
                    let inner = e.into_inner();
                    if inner.is_retryable() {
                        last_retryable = Some(inner);
                        continue;
                    }
                    // Non-retryable error, fail fast
                    return Err(inner.into());
                },
            }
        }

        Err(last_retryable
            .unwrap_or_else(|| Web3RpcError::Transport("All TRON nodes unreachable".to_string()))
            .into())
    }

    /// Get validated block data for TAPOS with node rotation.
    pub async fn get_block_for_tapos(&self) -> Web3RpcResult<TaposBlockData> {
        self.try_clients(|client| async move { client.get_block_for_tapos().await })
            .await
    }

    /// Get account information with node rotation.
    pub async fn get_account(&self, address: &TronAddress) -> Web3RpcResult<GetAccountResponse> {
        self.try_clients(|client| {
            let addr = *address;
            async move { client.get_account(&addr).await }
        })
        .await
    }

    /// Get TRC20 token balance for an account with node rotation.
    ///
    /// Calls `balanceOf(address)` on the TRC20 contract.
    /// Returns balance as U256 (raw token units, not adjusted for decimals).
    pub async fn trc20_balance_of(&self, contract: &TronAddress, account: &TronAddress) -> Web3RpcResult<U256> {
        let parameter = abi_encode_address_param(account);
        self.try_clients(|client| {
            let contract = *contract;
            let account = *account;
            let param = parameter.clone();
            async move {
                let response = client
                    .trigger_constant_contract(&account, &contract, "balanceOf(address)", &param)
                    .await?;
                parse_constant_result_u256(&response)
            }
        })
        .await
    }

    /// Get TRC20 token decimals with node rotation.
    ///
    /// Calls `decimals()` on the TRC20 contract.
    /// Returns decimals as u8 (typically 6 for USDT, 18 for most tokens).
    pub async fn trc20_decimals(&self, contract: &TronAddress, caller: &TronAddress) -> Web3RpcResult<u8> {
        self.try_clients(|client| {
            let contract = *contract;
            let caller = *caller;
            async move {
                let response = client
                    .trigger_constant_contract(&caller, &contract, "decimals()", "")
                    .await?;
                let value = parse_constant_result_u256(&response)?;

                // Decimals must fit in u8 (0-255)
                if value > U256::from(255u8) {
                    return Err(Web3RpcError::InvalidResponse(format!(
                        "TRC20 decimals value {} exceeds u8 range",
                        value
                    ))
                    .into());
                }
                Ok(value.as_u32() as u8)
            }
        })
        .await
    }

    /// Fetch validated TRON chain fee prices with node rotation.
    ///
    /// Invalid fee parameter payloads are treated as retryable (`BadResponse`) and trigger
    /// fallback to the next node.
    pub async fn get_chain_prices(&self) -> Web3RpcResult<TronChainPrices> {
        self.try_clients(|client| async move { client.get_chain_prices().await })
            .await
    }

    /// Get account resource usage and limits with node rotation.
    pub async fn get_account_resource(&self, address: &TronAddress) -> Web3RpcResult<TronAccountResources> {
        self.try_clients(|client| {
            let addr = *address;
            async move { client.get_account_resource(&addr).await }
        })
        .await
    }

    /// Broadcast a signed transaction (hex-encoded protobuf bytes) with node rotation.
    pub async fn broadcast_hex(&self, tx_hex: &str) -> Web3RpcResult<BroadcastHexResponse> {
        let tx_hex = tx_hex.to_owned();
        self.try_clients(|client| {
            let hex = tx_hex.clone();
            async move { client.broadcast_hex(&hex).await }
        })
        .await
    }

    /// Get raw transaction by hash with node rotation.
    pub async fn get_transaction_by_id(&self, tx_id: &str) -> Web3RpcResult<GetTransactionByIdResponse> {
        let tx_id = tx_id.to_owned();
        self.try_clients(|client| {
            let tx_id = tx_id.clone();
            async move { client.get_transaction_by_id(&tx_id).await }
        })
        .await
    }

    /// Get transaction execution info/receipt by hash with node rotation.
    pub async fn get_transaction_info_by_id(&self, tx_id: &str) -> Web3RpcResult<GetTransactionInfoByIdResponse> {
        let tx_id = tx_id.to_owned();
        self.try_clients(|client| {
            let tx_id = tx_id.clone();
            async move { client.get_transaction_info_by_id(&tx_id).await }
        })
        .await
    }

    /// Call a constant (view/pure) function on a smart contract with node rotation.
    pub async fn trigger_constant_contract(
        &self,
        owner_address: &TronAddress,
        contract_address: &TronAddress,
        function_selector: &str,
        parameter: &str,
    ) -> Web3RpcResult<TriggerConstantContractResponse> {
        let owner = *owner_address;
        let contract = *contract_address;
        let selector = function_selector.to_owned();
        let param = parameter.to_owned();
        self.try_clients(move |client| {
            let selector = selector.clone();
            let param = param.clone();
            async move {
                client
                    .trigger_constant_contract(&owner, &contract, &selector, &param)
                    .await
            }
        })
        .await
    }

    /// Estimate energy required for a TRC20 `transfer(address,uint256)` call.
    ///
    /// ABI-encodes the transfer parameters and calls `trigger_constant_contract`.
    /// Returns the `energy_used` from the response. If `energy_used` is missing or zero,
    /// returns `BadResponse` to trigger node rotation.
    pub async fn estimate_trc20_transfer_energy(
        &self,
        owner: &TronAddress,
        contract: &TronAddress,
        recipient: &TronAddress,
        amount: U256,
    ) -> Web3RpcResult<u64> {
        let params_hex = abi_encode_trc20_transfer_params(recipient, amount);
        let response = self
            .trigger_constant_contract(owner, contract, "transfer(address,uint256)", &params_hex)
            .await?;
        match response.energy_used {
            Some(energy) if energy > 0 => Ok(energy),
            _ => Err(Web3RpcError::BadResponse(
                "trigger_constant_contract returned no energy_used for TRC20 transfer estimation".to_owned(),
            )
            .into()),
        }
    }
}

// ============================================================================
// TRC20 ABI Helpers
// ============================================================================

/// Encode an address as a 32-byte ABI parameter (hex string, no 0x prefix).
///
/// For `balanceOf(address)`, the parameter is the account address encoded as:
/// - 12 zero bytes (left padding)
/// - 20-byte raw EVM address (from `TronAddress::to_evm_address()`)
///
/// Uses standard 20-byte EVM ABI encoding, NOT TRON's 21-byte format with 0x41 prefix.
fn abi_encode_address_param(addr: &TronAddress) -> String {
    let evm_addr = addr.to_evm_address();
    let mut padded = [0u8; 32];
    padded[12..32].copy_from_slice(evm_addr.as_bytes());
    hex::encode(padded)
}

/// Encode TRC20 `transfer(address,uint256)` parameters as a hex string (no 0x prefix).
///
/// The parameters are ABI-encoded as two 32-byte slots:
/// - recipient address (left-padded to 32 bytes, 20-byte EVM format)
/// - amount as uint256
///
/// The function selector is NOT included — TRON's `function_selector` field handles that.
fn abi_encode_trc20_transfer_params(recipient: &TronAddress, amount: U256) -> String {
    let tokens = trc20_transfer_tokens(recipient, amount);
    hex::encode(ethabi::encode(&tokens))
}

/// Parse the first constant_result element as U256.
///
/// TRON returns `constant_result` as `repeated bytes` (protobuf), serialized as hex strings
/// without 0x prefix. This function decodes the hex, validates, and converts to U256.
fn parse_constant_result_u256(response: &TriggerConstantContractResponse) -> Web3RpcResult<U256> {
    // Empty constant_result can occur due to node-specific issues:
    // - Node out of sync (different latest block state)
    // - Resource limits (OutOfTimeException on overloaded nodes)
    // - TVM configuration differences between nodes
    // BadResponse is used to trigger rotation - another node may succeed.
    let hex_str = response.constant_result.first().ok_or_else(|| {
        Web3RpcError::BadResponse(
            "TRON constant_result is empty - node may be out of sync or resource-constrained".to_string(),
        )
    })?;

    // Decode hex string to bytes. Invalid hex from a node should trigger rotation.
    let bytes = hex::decode(hex_str).map_err(|e| {
        Web3RpcError::BadResponse(format!(
            "TRON constant_result contains invalid hex '{}': {}",
            hex_str, e
        ))
    })?;

    // Oversized result (>32 bytes) indicates wrong return type from contract.
    // This is probably deterministic and would fail on all nodes - InvalidResponse is used.
    if bytes.len() > 32 {
        return Err(Web3RpcError::InvalidResponse(format!(
            "constant_result too large: {} bytes (max 32) - contract may return wrong type",
            bytes.len()
        ))
        .into());
    }

    // Left-pad to 32 bytes and convert to U256
    let mut padded = [0u8; 32];
    padded[32 - bytes.len()..].copy_from_slice(&bytes[..]);
    Ok(U256::from_big_endian(&padded))
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
            let (_header, number) = response.validated_header()?;
            Ok(number)
        })
        .await
    }

    async fn balance_native(&self, address: Self::Address) -> Result<Self::Balance, Self::Error> {
        self.try_clients(|client| {
            let addr = address;
            async move {
                let account = client.get_account(&addr).await?;
                let balance = match account {
                    GetAccountResponse::Account { balance, .. } => balance,
                    // Address might have been created by KDF and not used on-chain yet. Return 0.
                    GetAccountResponse::NoAccount(_) => 0,
                };
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the custom `blockID` hex deserializer parses correctly and that the
    /// block number embedded in `blockID[0..8]` matches `block_header.raw_data.number`.
    /// Test data: Nile testnet block [54242114](https://nile.tronscan.org/#/block/54242114).
    #[test]
    fn parse_getnowblock_and_tapos_derivation() {
        let json = r#"{
            "blockID": "00000000033bab42567444cc8af3dbaeb5cf26b514b7e90b9a23424ea8392641",
            "block_header": {
                "raw_data": {
                    "number": 54242114,
                    "timestamp": 1738799040000
                }
            }
        }"#;
        let resp: GetNowBlockResponse = serde_json::from_str(json).unwrap();

        assert_eq!(resp.block_header.raw_data.number, 54_242_114);
        assert_eq!(resp.block_header.raw_data.timestamp, 1_738_799_040_000);

        // Block number embedded in blockID[0..8] matches block_header
        let number_from_id = u64::from_be_bytes(resp.block_id[..8].try_into().unwrap());
        assert_eq!(number_from_id, 54_242_114);
    }

    /// Non-hex `blockID` must fail deserialization (triggers `BadResponse` → node rotation).
    #[test]
    fn parse_getnowblock_rejects_invalid_block_id_hex() {
        let json = r#"{ "blockID": "zz00000000000000000000000000000000000000000000000000000000000000", "block_header": { "raw_data": { "number": 1 } } }"#;
        let err = serde_json::from_str::<GetNowBlockResponse>(json).unwrap_err();
        assert!(err.is_data(), "Expected data error for invalid hex, got: {}", err);
        assert!(
            err.to_string().contains("Invalid character"),
            "Expected hex parse error, got: {}",
            err
        );
    }

    /// `blockID` that isn't exactly 32 bytes must fail deserialization.
    #[test]
    fn parse_getnowblock_rejects_wrong_length_block_id() {
        // 31 bytes (62 hex chars) — too short
        let json = r#"{ "blockID": "00000000033bab42e37d025dc14e9ebc26e8f6cb6b6e26e08d2bf2db29c3b4", "block_header": { "raw_data": { "number": 1 } } }"#;
        let err = serde_json::from_str::<GetNowBlockResponse>(json).unwrap_err();
        assert!(err.is_data(), "Expected data error for wrong length, got: {}", err);
        assert!(
            err.to_string().contains("blockID must be 32 bytes"),
            "Expected length error, got: {}",
            err
        );
    }

    fn sample_chain_params() -> Vec<ChainParameterEntry> {
        vec![
            ChainParameterEntry {
                key: "getTransactionFee".to_owned(),
                value: Some(1000),
            },
            ChainParameterEntry {
                key: "getEnergyFee".to_owned(),
                value: Some(100),
            },
            ChainParameterEntry {
                key: "getCreateNewAccountFeeInSystemContract".to_owned(),
                value: Some(1_000_000),
            },
            ChainParameterEntry {
                key: "getCreateAccountFee".to_owned(),
                value: Some(100_000),
            },
            ChainParameterEntry {
                key: "getCreateNewAccountBandwidthRate".to_owned(),
                value: Some(1),
            },
        ]
    }

    #[test]
    fn parse_chain_prices_accepts_valid_parameters() {
        let response = GetChainParametersResponse {
            chain_parameter: sample_chain_params(),
        };

        let prices = parse_chain_prices_sun(&response).unwrap();
        assert_eq!(prices.bandwidth_price_sun, 1000);
        assert_eq!(prices.energy_price_sun, 100);
        assert_eq!(prices.create_new_account_fee_sun, 1_000_000);
        assert_eq!(prices.create_account_bandwidth_fee_sun, 100_000);
        assert_eq!(prices.create_new_account_bandwidth_rate, 1);
    }

    #[test]
    fn parse_chain_prices_rejects_zero_values_as_retryable_bad_response() {
        let mut params = sample_chain_params();
        // Set getTransactionFee to 0
        params[0].value = Some(0);
        let response = GetChainParametersResponse {
            chain_parameter: params,
        };

        let err = parse_chain_prices_sun(&response).unwrap_err().into_inner();
        assert!(matches!(err, Web3RpcError::BadResponse(_)));
        assert!(err.is_retryable());
    }

    #[test]
    fn parse_chain_prices_allows_zero_account_creation_fees() {
        let mut params = sample_chain_params();
        // Governance could set account creation fees to 0
        params[2].value = Some(0); // getCreateNewAccountFeeInSystemContract
        params[3].value = Some(0); // getCreateAccountFee
        let response = GetChainParametersResponse {
            chain_parameter: params,
        };

        let prices = parse_chain_prices_sun(&response).unwrap();
        assert_eq!(prices.create_new_account_fee_sun, 0);
        assert_eq!(prices.create_account_bandwidth_fee_sun, 0);
        assert_eq!(prices.create_new_account_bandwidth_rate, 1);
    }

    #[test]
    fn parse_chain_prices_defaults_missing_account_creation_params_to_zero() {
        // Only bandwidth/energy params, missing account creation params — should not fail
        let response = GetChainParametersResponse {
            chain_parameter: vec![
                ChainParameterEntry {
                    key: "getTransactionFee".to_owned(),
                    value: Some(1000),
                },
                ChainParameterEntry {
                    key: "getEnergyFee".to_owned(),
                    value: Some(100),
                },
            ],
        };

        let prices = parse_chain_prices_sun(&response).unwrap();
        assert_eq!(prices.create_new_account_fee_sun, 0);
        assert_eq!(prices.create_account_bandwidth_fee_sun, 0);
        assert_eq!(prices.create_new_account_bandwidth_rate, 1);
    }

    #[test]
    fn parse_getchainparameters_handles_entries_without_value_field() {
        let json = r#"{
            "chainParameter": [
                { "key": "getAllowUpdateAccountName" },
                { "key": "getTransactionFee", "value": 1000 },
                { "key": "getEnergyFee", "value": 100 },
                { "key": "getCreateNewAccountFeeInSystemContract", "value": 1000000 },
                { "key": "getCreateAccountFee", "value": 100000 },
                { "key": "getCreateNewAccountBandwidthRate", "value": 1 }
            ]
        }"#;

        let response: GetChainParametersResponse = serde_json::from_str(json).unwrap();
        let prices = parse_chain_prices_sun(&response).unwrap();
        assert_eq!(prices.bandwidth_price_sun, 1000);
        assert_eq!(prices.energy_price_sun, 100);
        assert_eq!(prices.create_new_account_fee_sun, 1_000_000);
        assert_eq!(prices.create_account_bandwidth_fee_sun, 100_000);
        assert_eq!(prices.create_new_account_bandwidth_rate, 1);
    }

    #[test]
    fn parse_chain_prices_defaults_missing_bandwidth_rate_to_one() {
        let response = GetChainParametersResponse {
            chain_parameter: vec![
                ChainParameterEntry {
                    key: "getTransactionFee".to_owned(),
                    value: Some(1000),
                },
                ChainParameterEntry {
                    key: "getEnergyFee".to_owned(),
                    value: Some(100),
                },
                ChainParameterEntry {
                    key: "getCreateNewAccountFeeInSystemContract".to_owned(),
                    value: Some(1_000_000),
                },
                ChainParameterEntry {
                    key: "getCreateAccountFee".to_owned(),
                    value: Some(100_000),
                },
            ],
        };

        let prices = parse_chain_prices_sun(&response).unwrap();
        assert_eq!(prices.create_new_account_bandwidth_rate, 1);
    }

    /// Verifies that all 6 mixed-case field renames map correctly to `TronAccountResources`.
    /// Extra fields (TotalNetLimit, etc.) are silently ignored as expected.
    #[test]
    fn parse_account_resource_canonical_response() {
        let json = r#"{
            "freeNetUsed": 100,
            "freeNetLimit": 600,
            "NetUsed": 30,
            "NetLimit": 500,
            "EnergyUsed": 200,
            "EnergyLimit": 1000,
            "TotalNetLimit": 43200000000,
            "TotalNetWeight": 84593524300,
            "TotalEnergyCurrentLimit": 50000000000,
            "TotalEnergyWeight": 12345678
        }"#;

        let resources: TronAccountResources = serde_json::from_str(json).unwrap();
        assert_eq!(resources.free_net_used, 100);
        assert_eq!(resources.free_net_limit, 600);
        assert_eq!(resources.net_used, 30);
        assert_eq!(resources.net_limit, 500);
        assert_eq!(resources.energy_used, 200);
        assert_eq!(resources.energy_limit, 1000);
    }

    /// Empty `{}` (unactivated account / proto3 zero-omission) produces all-zero resources.
    #[test]
    fn parse_account_resource_empty_response() {
        let resources: TronAccountResources = serde_json::from_str("{}").unwrap();
        assert_eq!(resources, TronAccountResources::default());
    }

    /// `tron_error_from_value` catches `result: false` before deserialization,
    /// so `BroadcastHexResponse` only needs to handle success responses.
    #[test]
    fn broadcast_hex_error_response_caught_by_tron_error_from_value() {
        let json = r#"{
            "result": false,
            "code": "SIGERROR",
            "message": "Validate signature error",
            "txid": "a1b2c3d4e5f60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90"
        }"#;

        let value: Json = serde_json::from_str(json).unwrap();
        let error = tron_error_from_value(&value);
        assert!(error.is_some(), "Error should be detected");
        let error = error.unwrap();
        assert_eq!(error.code.as_deref(), Some("SIGERROR"));
        assert!(!error.is_retryable());
    }
}
