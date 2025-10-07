#![cfg_attr(target_arch = "wasm32", allow(unused_macros))]
#![cfg_attr(target_arch = "wasm32", allow(dead_code))]

mod electrum_rpc;
pub use electrum_rpc::*;

use crate::utxo::{sat_from_big_decimal, GetBlockHeaderError, GetTxError, NumConversError, NumConversResult};
use crate::{big_decimal_from_sat_unsigned, MyAddressError, RpcTransportEventHandlerShared};
use chain::{OutPoint, Transaction as UtxoTx, TransactionInput, TxHashAlgo};
use common::custom_iter::TryIntoGroupMap;
use common::executor::Timer;
use common::jsonrpc_client::{
    JsonRpcBatchClient, JsonRpcClient, JsonRpcError, JsonRpcErrorType, JsonRpcRequest, JsonRpcRequestEnum,
    JsonRpcResponseFut, RpcRes,
};
use common::log::{error, info, warn};
use common::{median, now_sec};
use enum_derives::EnumFromStringify;
use keys::hash::H256;
use keys::Address;
use mm2_err_handle::prelude::*;
use mm2_number::{BigDecimal, MmNumber};
use rpc::v1::types::{Bytes as BytesJson, Transaction as RpcTransaction, H256 as H256Json};
use script::Script;
use serialization::{deserialize, serialize, serialize_with_flags, CoinVariant, SERIALIZE_TRANSACTION_WITNESS};

use std::collections::HashMap;
use std::fmt;
use std::fmt::Debug;
use std::num::NonZeroU64;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;

use async_trait::async_trait;
use derive_more::Display;
use futures::channel::oneshot as async_oneshot;
use futures::compat::Future01CompatExt;
use futures::future::{FutureExt, TryFutureExt};
use futures::lock::Mutex as AsyncMutex;
use futures01::Future;
#[cfg(test)]
use mocktopus::macros::*;
use serde_json::{self as json, Value as Json};

cfg_native! {
    use crate::RpcTransportEventHandler;
    use common::jsonrpc_client::{JsonRpcRemoteAddr, JsonRpcResponseEnum};

    use http::header::AUTHORIZATION;
    use http::{Request, StatusCode};
}

pub const NO_TX_ERROR_CODE: &str = "'code': -5";
const RESPONSE_TOO_LARGE_CODE: i16 = -32600;
const TX_NOT_FOUND_RETRIES: u8 = 10;

pub type AddressesByLabelResult = HashMap<String, AddressPurpose>;
pub type UnspentMap = HashMap<Address, Vec<UnspentInfo>>;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct AddressPurpose {
    purpose: String,
}

#[derive(Clone, Debug)]
pub enum UtxoRpcClientEnum {
    Native(NativeClient),
    Electrum(ElectrumClient),
}

impl std::fmt::Display for UtxoRpcClientEnum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UtxoRpcClientEnum::Native(_) => write!(f, "native"),
            UtxoRpcClientEnum::Electrum(_) => write!(f, "electrum"),
        }
    }
}

impl From<ElectrumClient> for UtxoRpcClientEnum {
    fn from(client: ElectrumClient) -> UtxoRpcClientEnum {
        UtxoRpcClientEnum::Electrum(client)
    }
}

impl From<NativeClient> for UtxoRpcClientEnum {
    fn from(client: NativeClient) -> UtxoRpcClientEnum {
        UtxoRpcClientEnum::Native(client)
    }
}

impl Deref for UtxoRpcClientEnum {
    type Target = dyn UtxoRpcClientOps;
    fn deref(&self) -> &dyn UtxoRpcClientOps {
        match self {
            UtxoRpcClientEnum::Native(ref c) => c,
            UtxoRpcClientEnum::Electrum(ref c) => c,
        }
    }
}

impl UtxoRpcClientEnum {
    pub fn wait_for_confirmations(
        &self,
        tx_hash: H256Json,
        expiry_height: u32,
        confirmations: u32,
        requires_notarization: bool,
        wait_until: u64,
        check_every: u64,
    ) -> Box<dyn Future<Item = (), Error = String> + Send> {
        let selfi = self.clone();
        let mut tx_not_found_retries = TX_NOT_FOUND_RETRIES;
        let fut = async move {
            loop {
                if now_sec() > wait_until {
                    return ERR!(
                        "Waited too long until {} for transaction {:?} to be confirmed {} times",
                        wait_until,
                        tx_hash,
                        confirmations
                    );
                }

                match selfi.get_verbose_transaction(&tx_hash).compat().await {
                    Ok(t) => {
                        let tx_confirmations = if requires_notarization {
                            t.confirmations
                        } else {
                            t.rawconfirmations.unwrap_or(t.confirmations)
                        };
                        if tx_confirmations >= confirmations {
                            return Ok(());
                        } else {
                            info!(
                                "Waiting for tx {:?} confirmations, now {}, required {}, requires_notarization {}",
                                tx_hash, tx_confirmations, confirmations, requires_notarization
                            )
                        }
                    },
                    Err(e) => {
                        if e.get_inner().is_tx_not_found_error() {
                            if tx_not_found_retries == 0 {
                                return ERR!(
                                    "Tx {} was not found on chain after {} tries, error: {}",
                                    tx_hash,
                                    TX_NOT_FOUND_RETRIES,
                                    e,
                                );
                            }
                            error!(
                                "Tx {} not found on chain, error: {}, retrying in {} seconds. Retries left: {}",
                                tx_hash, e, check_every, tx_not_found_retries
                            );
                            tx_not_found_retries -= 1;
                            Timer::sleep(check_every as f64).await;
                            continue;
                        };

                        if expiry_height > 0 {
                            let block = match selfi.get_block_count().compat().await {
                                Ok(b) => b,
                                Err(e) => {
                                    error!("Error {} getting block number, retrying in {} seconds", e, check_every);
                                    Timer::sleep(check_every as f64).await;
                                    continue;
                                },
                            };

                            if block > expiry_height as u64 {
                                return ERR!("The transaction {:?} has expired, current block {}", tx_hash, block);
                            }
                        }
                        error!(
                            "Error {:?} getting the transaction {:?}, retrying in {} seconds",
                            e, tx_hash, check_every
                        )
                    },
                }

                Timer::sleep(check_every as f64).await;
            }
        };
        Box::new(fut.boxed().compat())
    }

    #[inline]
    pub fn is_native(&self) -> bool {
        match self {
            UtxoRpcClientEnum::Native(_) => true,
            UtxoRpcClientEnum::Electrum(_) => false,
        }
    }
}

/// Generic unspent info required to build transactions, we need this separate type because native
/// and Electrum provide different list_unspent format.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct UnspentInfo {
    pub outpoint: OutPoint,
    pub value: u64,
    /// The block height transaction mined in.
    /// Note None if the transaction is not mined yet.
    pub height: Option<u64>,
    /// The script pubkey of the UTXO
    pub script: Script,
}

impl UnspentInfo {
    fn from_electrum(unspent: ElectrumUnspent, script: Script) -> UnspentInfo {
        UnspentInfo {
            outpoint: OutPoint {
                hash: unspent.tx_hash.reversed().into(),
                index: unspent.tx_pos,
            },
            value: unspent.value,
            height: unspent.height,
            script,
        }
    }

    fn from_native(unspent: NativeUnspent, decimals: u8, height: Option<u64>) -> NumConversResult<UnspentInfo> {
        Ok(UnspentInfo {
            outpoint: OutPoint {
                hash: unspent.txid.reversed().into(),
                index: unspent.vout,
            },
            value: sat_from_big_decimal(&unspent.amount.to_decimal(), decimals)?,
            height,
            script: unspent.script_pub_key.0.into(),
        })
    }
}

#[derive(Debug, PartialEq)]
pub enum BlockHashOrHeight {
    Height(i64),
    Hash(H256Json),
}

#[derive(Debug, PartialEq)]
pub struct SpentOutputInfo {
    /// The input that spends the output
    pub input: TransactionInput,
    /// The index of spending input
    pub input_index: usize,
    /// The transaction spending the output
    pub spending_tx: UtxoTx,
    /// The block hash or height the includes the spending transaction
    /// For electrum clients the block height will be returned, for native clients the block hash will be returned
    pub spent_in_block: BlockHashOrHeight,
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Debug, Deserialize, Serialize)]
pub enum EstimateFeeMode {
    ECONOMICAL,
    CONSERVATIVE,
    UNSET,
}

pub type UtxoRpcResult<T> = Result<T, MmError<UtxoRpcError>>;
pub type UtxoRpcFut<T> = Box<dyn Future<Item = T, Error = MmError<UtxoRpcError>> + Send + 'static>;

#[derive(Debug, Display, EnumFromStringify)]
pub enum UtxoRpcError {
    Transport(JsonRpcError),
    ResponseParseError(JsonRpcError),
    InvalidResponse(String),
    #[from_stringify("MyAddressError")]
    Internal(String),
}

impl From<JsonRpcError> for UtxoRpcError {
    fn from(e: JsonRpcError) -> Self {
        match e.error {
            JsonRpcErrorType::InvalidRequest(_) | JsonRpcErrorType::Internal(_) => {
                UtxoRpcError::Internal(e.to_string())
            },
            JsonRpcErrorType::Transport(_) => UtxoRpcError::Transport(e),
            JsonRpcErrorType::Parse(_, _) | JsonRpcErrorType::Response(_, _) => UtxoRpcError::ResponseParseError(e),
        }
    }
}

impl From<serialization::Error> for UtxoRpcError {
    fn from(e: serialization::Error) -> Self {
        UtxoRpcError::InvalidResponse(format!("{e:?}"))
    }
}

impl From<NumConversError> for UtxoRpcError {
    fn from(e: NumConversError) -> Self {
        UtxoRpcError::Internal(e.to_string())
    }
}

impl From<keys::Error> for UtxoRpcError {
    fn from(e: keys::Error) -> Self {
        UtxoRpcError::Internal(e.to_string())
    }
}

impl UtxoRpcError {
    pub fn is_tx_not_found_error(&self) -> bool {
        if let UtxoRpcError::ResponseParseError(ref json_err) = self {
            if let JsonRpcErrorType::Response(_, json) = &json_err.error {
                return json["error"]["code"] == -5 // native compatible
                    || json["message"].as_str().unwrap_or_default().contains(NO_TX_ERROR_CODE);
                // electrum compatible;
            }
        };
        false
    }

    pub fn is_response_too_large(&self) -> bool {
        if let UtxoRpcError::ResponseParseError(ref json_err) = self {
            if let JsonRpcErrorType::Response(_, json) = &json_err.error {
                return json["code"] == RESPONSE_TOO_LARGE_CODE;
            }
        };
        false
    }

    pub fn is_network_error(&self) -> bool {
        matches!(self, UtxoRpcError::Transport(_))
    }
}

/// Common operations that both types of UTXO clients have but implement them differently
#[async_trait]
pub trait UtxoRpcClientOps: fmt::Debug + Send + Sync + 'static {
    /// Returns available unspents for the given `address`.
    fn list_unspent(&self, address: &Address, decimals: u8) -> UtxoRpcFut<Vec<UnspentInfo>>;

    /// Returns available unspents for every given `addresses`.
    fn list_unspent_group(&self, addresses: Vec<Address>, decimals: u8) -> UtxoRpcFut<UnspentMap>;

    /// Submits the given `tx` transaction to blockchain network.
    fn send_transaction(&self, tx: &UtxoTx) -> UtxoRpcFut<H256Json>;

    /// Submits the raw `tx` transaction (serialized, hex-encoded) to blockchain network.
    fn send_raw_transaction(&self, tx: BytesJson) -> UtxoRpcFut<H256Json>;

    /// Subscribe to scripthash notifications from `server_address` for the given `scripthash`.
    fn blockchain_scripthash_subscribe_using(&self, server_address: &str, scripthash: String) -> UtxoRpcFut<Json>;

    /// Returns raw transaction (serialized, hex-encoded) by the given `txid`.
    fn get_transaction_bytes(&self, txid: &H256Json) -> UtxoRpcFut<BytesJson>;

    /// Returns verbose transaction by the given `txid`.
    fn get_verbose_transaction(&self, txid: &H256Json) -> UtxoRpcFut<RpcTransaction>;

    /// Returns verbose transactions in the same order they were requested.
    fn get_verbose_transactions(&self, tx_ids: &[H256Json]) -> UtxoRpcFut<Vec<RpcTransaction>>;

    /// Returns the height of the most-work fully-validated chain.
    fn get_block_count(&self) -> UtxoRpcFut<u64>;

    /// Requests balance of the given `address`.
    fn display_balance(&self, address: Address, decimals: u8) -> RpcRes<BigDecimal>;

    /// Requests balances of the given `addresses`.
    /// The pairs `(Address, BigDecimal)` are guaranteed to be in the same order in which they were requested.
    fn display_balances(&self, addresses: Vec<Address>, decimals: u8) -> UtxoRpcFut<Vec<(Address, BigDecimal)>>;

    /// Returns fee estimation per KByte in satoshis.
    fn estimate_fee_sat(
        &self,
        decimals: u8,
        fee_method: &EstimateFeeMethod,
        mode: &Option<EstimateFeeMode>,
        n_blocks: u32,
    ) -> UtxoRpcFut<u64>;

    /// Returns the minimum fee a low-priority transaction must pay in order to be accepted to the daemon’s memory pool.
    fn get_relay_fee(&self) -> RpcRes<BigDecimal>;

    /// Tries to find a transaction that spends the specified `vout` output of the `tx_hash` transaction.
    fn find_output_spend(
        &self,
        tx_hash: H256,
        script_pubkey: &[u8],
        vout: usize,
        from_block: BlockHashOrHeight,
        tx_hash_algo: TxHashAlgo,
    ) -> Box<dyn Future<Item = Option<SpentOutputInfo>, Error = String> + Send>;

    /// Get median time past for `count` blocks in the past including `starting_block`
    fn get_median_time_past(
        &self,
        starting_block: u64,
        count: NonZeroU64,
        coin_variant: CoinVariant,
    ) -> UtxoRpcFut<u32>;

    /// Returns block time in seconds since epoch (Jan 1 1970 GMT).
    async fn get_block_timestamp(&self, height: u64) -> Result<u64, MmError<GetBlockHeaderError>>;

    /// Returns verbose transaction by the given `txid` if it's on-chain or None if it's not.
    async fn get_tx_if_onchain(&self, tx_hash: &H256Json) -> Result<Option<UtxoTx>, MmError<GetTxError>> {
        match self
            .get_transaction_bytes(tx_hash)
            .compat()
            .await
            .map_err(|e| e.into_inner())
        {
            Ok(bytes) => Ok(Some(deserialize(bytes.as_slice())?)),
            Err(err) => {
                if err.is_tx_not_found_error() {
                    return Ok(None);
                }
                Err(err.into())
            },
        }
    }
}

#[derive(Clone, Deserialize, Debug)]
#[cfg_attr(test, derive(Default))]
pub struct NativeUnspent {
    pub txid: H256Json,
    pub vout: u32,
    pub address: String,
    pub account: Option<String>,
    #[serde(rename = "scriptPubKey")]
    pub script_pub_key: BytesJson,
    pub amount: MmNumber,
    pub confirmations: u64,
    pub spendable: bool,
}

#[derive(Clone, Deserialize, Debug)]
pub struct ValidateAddressRes {
    #[serde(rename = "isvalid")]
    pub is_valid: bool,
    pub address: String,
    #[serde(rename = "scriptPubKey")]
    pub script_pub_key: BytesJson,
    #[serde(rename = "segid")]
    pub seg_id: Option<u32>,
    #[serde(rename = "ismine")]
    pub is_mine: Option<bool>,
    #[serde(rename = "iswatchonly")]
    pub is_watch_only: Option<bool>,
    #[serde(rename = "isscript")]
    pub is_script: bool,
    pub account: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[cfg_attr(test, derive(Default))]
pub struct ListTransactionsItem {
    pub account: Option<String>,
    #[serde(default)]
    pub address: String,
    pub category: String,
    pub amount: f64,
    pub vout: u64,
    #[serde(default)]
    pub fee: f64,
    #[serde(default)]
    pub confirmations: i64,
    #[serde(default)]
    pub blockhash: H256Json,
    #[serde(default)]
    pub blockindex: u64,
    #[serde(default)]
    pub txid: H256Json,
    pub timereceived: u64,
    #[serde(default)]
    pub walletconflicts: Vec<String>,
}

impl ListTransactionsItem {
    /// Checks if the transaction is conflicting.
    /// It means the transaction has conflicts or has negative confirmations.
    pub fn is_conflicting(&self) -> bool {
        self.confirmations < 0 || !self.walletconflicts.is_empty()
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct ReceivedByAddressItem {
    #[serde(default)]
    pub account: String,
    pub address: String,
    pub txids: Vec<H256Json>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct EstimateSmartFeeRes {
    #[serde(rename = "feerate")]
    #[serde(default)]
    pub fee_rate: f64,
    #[serde(default)]
    pub errors: Vec<String>,
    pub blocks: i64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ListSinceBlockRes {
    transactions: Vec<ListTransactionsItem>,
    #[serde(rename = "lastblock")]
    #[allow(dead_code)]
    last_block: H256Json,
}

#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
pub struct NetworkInfoLocalAddress {
    address: String,
    port: u16,
    score: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
pub struct NetworkInfoNetwork {
    name: String,
    limited: bool,
    reachable: bool,
    proxy: String,
    proxy_randomize_credentials: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[allow(dead_code)]
pub struct NetworkInfo {
    connections: u64,
    #[serde(rename = "localaddresses")]
    local_addresses: Vec<NetworkInfoLocalAddress>,
    #[serde(rename = "localservices")]
    local_services: String,
    networks: Vec<NetworkInfoNetwork>,
    #[serde(rename = "protocolversion")]
    protocol_version: u64,
    #[serde(rename = "relayfee")]
    relay_fee: BigDecimal,
    subversion: String,
    #[serde(rename = "timeoffset")]
    time_offset: i64,
    version: u64,
    warnings: String,
}

#[derive(Clone, Debug, Deserialize)]
pub struct GetAddressInfoRes {
    // as of now we are interested in ismine and iswatchonly fields only, but this response contains much more info
    #[serde(rename = "ismine")]
    pub is_mine: bool,
    #[serde(rename = "iswatchonly")]
    pub is_watch_only: bool,
}

#[derive(Debug)]
pub enum EstimateFeeMethod {
    /// estimatefee, deprecated in many coins: https://bitcoincore.org/en/doc/0.16.0/rpc/util/estimatefee/
    Standard,
    /// estimatesmartfee added since 0.16.0 bitcoind RPC: https://bitcoincore.org/en/doc/0.16.0/rpc/util/estimatesmartfee/
    SmartFee,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum BlockNonce {
    String(String),
    U64(u64),
}

#[derive(Debug, Deserialize, Serialize)]
pub struct VerboseBlock {
    /// Block hash
    pub hash: H256Json,
    /// Number of confirmations. -1 if block is on the side chain
    pub confirmations: i64,
    /// Block size
    pub size: u32,
    /// Block size, excluding witness data
    pub strippedsize: Option<u32>,
    /// Block weight
    pub weight: Option<u32>,
    /// Block height
    pub height: Option<u32>,
    /// Block version
    pub version: u32,
    /// Block version as hex
    #[serde(rename = "versionHex")]
    pub version_hex: Option<String>,
    /// Merkle root of this block
    pub merkleroot: H256Json,
    /// Transactions ids
    pub tx: Vec<H256Json>,
    /// Block time in seconds since epoch (Jan 1 1970 GMT)
    pub time: u32,
    /// Median block time in seconds since epoch (Jan 1 1970 GMT)
    pub mediantime: Option<u32>,
    /// Block nonce
    pub nonce: BlockNonce,
    /// Block nbits
    pub bits: String,
    /// Block difficulty
    pub difficulty: f64,
    /// Expected number of hashes required to produce the chain up to this block (in hex)
    pub chainwork: H256Json,
    /// Hash of previous block
    pub previousblockhash: Option<H256Json>,
    /// Hash of next block
    pub nextblockhash: Option<H256Json>,
    #[serde(rename = "finalsaplingroot")]
    pub final_sapling_root: Option<H256Json>,
}

pub type RpcReqSub<T> = async_oneshot::Sender<Result<T, JsonRpcError>>;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ListUnspentArgs {
    min_conf: i32,
    max_conf: i32,
    addresses: Vec<String>,
}

#[derive(Debug)]
struct ConcurrentRequestState<V> {
    is_running: bool,
    subscribers: Vec<RpcReqSub<V>>,
}

impl<V> ConcurrentRequestState<V> {
    fn new() -> Self {
        ConcurrentRequestState {
            is_running: false,
            subscribers: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct ConcurrentRequestMap<K, V> {
    inner: AsyncMutex<HashMap<K, ConcurrentRequestState<V>>>,
}

impl<K, V> Default for ConcurrentRequestMap<K, V> {
    fn default() -> Self {
        ConcurrentRequestMap {
            inner: AsyncMutex::new(HashMap::new()),
        }
    }
}

impl<K: Clone + Eq + std::hash::Hash, V: Clone> ConcurrentRequestMap<K, V> {
    pub fn new() -> ConcurrentRequestMap<K, V> {
        ConcurrentRequestMap::default()
    }

    async fn wrap_request(&self, request_arg: K, request_fut: RpcRes<V>) -> Result<V, JsonRpcError> {
        let mut map = self.inner.lock().await;
        let state = map
            .entry(request_arg.clone())
            .or_insert_with(ConcurrentRequestState::new);
        if state.is_running {
            let (tx, rx) = async_oneshot::channel();
            state.subscribers.push(tx);
            // drop here to avoid holding the lock during await
            drop(map);
            rx.await.unwrap()
        } else {
            state.is_running = true;
            // drop here to avoid holding the lock during await
            drop(map);
            let request_res = request_fut.compat().await;
            let mut map = self.inner.lock().await;
            let state = map.get_mut(&request_arg).unwrap();
            for sub in state.subscribers.drain(..) {
                if sub.send(request_res.clone()).is_err() {
                    warn!("subscriber is dropped");
                }
            }
            state.is_running = false;
            request_res
        }
    }
}

/// RPC client for UTXO based coins
/// https://developer.bitcoin.org/reference/rpc/index.html - Bitcoin RPC API reference
/// Other coins have additional methods or miss some of these
/// This description will be updated with more info
#[derive(Debug)]
pub struct NativeClientImpl {
    /// Name of coin the rpc client is intended to work with
    pub coin_ticker: String,
    /// The uri to send requests to
    pub uri: String,
    /// Value of Authorization header, e.g. "Basic base64(user:password)"
    pub auth: String,
    /// Transport event handlers
    pub event_handlers: Vec<RpcTransportEventHandlerShared>,
    pub request_id: AtomicU64,
    pub list_unspent_concurrent_map: ConcurrentRequestMap<ListUnspentArgs, Vec<NativeUnspent>>,
}

#[cfg(test)]
impl Default for NativeClientImpl {
    fn default() -> Self {
        NativeClientImpl {
            coin_ticker: "TEST".to_string(),
            uri: "".to_string(),
            auth: "".to_string(),
            event_handlers: vec![],
            request_id: Default::default(),
            list_unspent_concurrent_map: ConcurrentRequestMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct NativeClient(pub Arc<NativeClientImpl>);
impl Deref for NativeClient {
    type Target = NativeClientImpl;
    fn deref(&self) -> &NativeClientImpl {
        &self.0
    }
}

/// The trait provides methods to generate the JsonRpcClient instance info such as name of coin.
pub trait UtxoJsonRpcClientInfo: JsonRpcClient {
    /// Name of coin the rpc client is intended to work with
    fn coin_name(&self) -> &str;

    /// Generate client info from coin name
    fn client_info(&self) -> String {
        format!("coin: {}", self.coin_name())
    }
}

impl UtxoJsonRpcClientInfo for NativeClientImpl {
    fn coin_name(&self) -> &str {
        self.coin_ticker.as_str()
    }
}

impl JsonRpcClient for NativeClientImpl {
    fn version(&self) -> &'static str {
        "1.0"
    }

    fn next_id(&self) -> u64 {
        self.request_id.fetch_add(1, AtomicOrdering::Relaxed)
    }

    fn client_info(&self) -> String {
        UtxoJsonRpcClientInfo::client_info(self)
    }

    #[cfg(target_arch = "wasm32")]
    fn transport(&self, _request: JsonRpcRequestEnum) -> JsonRpcResponseFut {
        Box::new(futures01::future::err(JsonRpcErrorType::Internal(
            "'NativeClientImpl' must be used in native mode only".to_string(),
        )))
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn transport(&self, request: JsonRpcRequestEnum) -> JsonRpcResponseFut {
        use mm2_net::transport::slurp_req;

        let request_body =
            try_f!(json::to_string(&request).map_err(|e| JsonRpcErrorType::InvalidRequest(e.to_string())));
        // measure now only body length, because the `hyper` crate doesn't allow to get total HTTP packet length
        self.event_handlers.on_outgoing_request(request_body.as_bytes());

        let uri = self.uri.clone();
        let auth = self.auth.clone();
        let http_request = try_f!(Request::builder()
            .method("POST")
            .header(AUTHORIZATION, auth)
            .uri(uri.clone())
            .body(Vec::from(request_body))
            .map_err(|e| JsonRpcErrorType::InvalidRequest(e.to_string())));

        let event_handlers = self.event_handlers.clone();
        Box::new(slurp_req(http_request).boxed().compat().then(
            move |result| -> Result<(JsonRpcRemoteAddr, JsonRpcResponseEnum), JsonRpcErrorType> {
                let res = result.map_err(|e| e.into_inner())?;
                // measure now only body length, because the `hyper` crate doesn't allow to get total HTTP packet length
                event_handlers.on_incoming_response(&res.2);

                let body =
                    std::str::from_utf8(&res.2).map_err(|e| JsonRpcErrorType::parse_error(&uri, e.to_string()))?;

                if res.0 != StatusCode::OK {
                    let res_value = serde_json::from_slice(&res.2)
                        .map_err(|e| JsonRpcErrorType::parse_error(&uri, e.to_string()))?;
                    return Err(JsonRpcErrorType::Response(uri.into(), res_value));
                }

                let response = json::from_str(body).map_err(|e| JsonRpcErrorType::parse_error(&uri, e.to_string()))?;
                Ok((uri.into(), response))
            },
        ))
    }
}

impl JsonRpcBatchClient for NativeClientImpl {}

// if mockable is placed before async_trait there is `munmap_chunk(): invalid pointer` error on async fn mocking attempt
#[async_trait]
#[cfg_attr(test, mockable)]
impl UtxoRpcClientOps for NativeClient {
    fn list_unspent(&self, address: &Address, decimals: u8) -> UtxoRpcFut<Vec<UnspentInfo>> {
        let fut = self
            .list_unspent_impl(0, i32::MAX, vec![address.to_string()])
            .map_to_mm_fut(UtxoRpcError::from)
            .and_then(move |unspents| {
                unspents
                    .into_iter()
                    .map(|unspent| UnspentInfo::from_native(unspent, decimals, None).map_mm_err())
                    .collect::<UtxoRpcResult<_>>()
            });
        Box::new(fut)
    }

    fn list_unspent_group(&self, addresses: Vec<Address>, decimals: u8) -> UtxoRpcFut<UnspentMap> {
        let mut addresses_str = Vec::with_capacity(addresses.len());
        let mut addresses_map = HashMap::with_capacity(addresses.len());
        for addr in addresses {
            let addr_str = addr.to_string();
            addresses_str.push(addr_str.clone());
            addresses_map.insert(addr_str, addr);
        }

        let fut = self
            .list_unspent_impl(0, i32::MAX, addresses_str)
            .map_to_mm_fut(UtxoRpcError::from)
            .and_then(move |unspents| {
                unspents
                    .into_iter()
                    // Convert `Vec<NativeUnspent>` into `UnspentMap`.
                    .map(|unspent| {
                        let orig_address = addresses_map
                            .get(&unspent.address)
                            .or_mm_err(|| {
                                UtxoRpcError::InvalidResponse(format!("Unexpected address '{}'", unspent.address))
                            })?
                            .clone();
                        let unspent_info = UnspentInfo::from_native(unspent, decimals, None).map_mm_err()?;
                        Ok((orig_address, unspent_info))
                    })
                    // Collect `(Address, UnspentInfo)` items into `HashMap<Address, Vec<UnspentInfo>>` grouped by the addresses.
                    .try_into_group_map()
            });
        Box::new(fut)
    }

    fn send_transaction(&self, tx: &UtxoTx) -> UtxoRpcFut<H256Json> {
        let tx_bytes = if tx.has_witness() {
            BytesJson::from(serialize_with_flags(tx, SERIALIZE_TRANSACTION_WITNESS))
        } else {
            BytesJson::from(serialize(tx))
        };
        Box::new(self.send_raw_transaction(tx_bytes))
    }

    /// https://developer.bitcoin.org/reference/rpc/sendrawtransaction
    fn send_raw_transaction(&self, tx: BytesJson) -> UtxoRpcFut<H256Json> {
        Box::new(rpc_func!(self, "sendrawtransaction", tx).map_to_mm_fut(UtxoRpcError::from))
    }

    fn blockchain_scripthash_subscribe_using(&self, _: &str, _scripthash: String) -> UtxoRpcFut<Json> {
        Box::new(futures01::future::err(
            UtxoRpcError::Internal("blockchain_scripthash_subscribe` is not supported for Native Clients".to_owned())
                .into(),
        ))
    }

    fn get_transaction_bytes(&self, txid: &H256Json) -> UtxoRpcFut<BytesJson> {
        Box::new(self.get_raw_transaction_bytes(txid).map_to_mm_fut(UtxoRpcError::from))
    }

    fn get_verbose_transaction(&self, txid: &H256Json) -> UtxoRpcFut<RpcTransaction> {
        Box::new(self.get_raw_transaction_verbose(txid).map_to_mm_fut(UtxoRpcError::from))
    }

    fn get_verbose_transactions(&self, tx_ids: &[H256Json]) -> UtxoRpcFut<Vec<RpcTransaction>> {
        Box::new(
            self.get_raw_transaction_verbose_batch(tx_ids)
                .map_to_mm_fut(UtxoRpcError::from),
        )
    }

    fn get_block_count(&self) -> UtxoRpcFut<u64> {
        Box::new(self.0.get_block_count().map_to_mm_fut(UtxoRpcError::from))
    }

    fn display_balance(&self, address: Address, _decimals: u8) -> RpcRes<BigDecimal> {
        Box::new(
            self.list_unspent_impl(0, i32::MAX, vec![address.to_string()])
                .map(|unspents| {
                    unspents
                        .iter()
                        .fold(BigDecimal::from(0), |sum, unspent| sum + unspent.amount.to_decimal())
                }),
        )
    }

    fn display_balances(&self, addresses: Vec<Address>, decimals: u8) -> UtxoRpcFut<Vec<(Address, BigDecimal)>> {
        let this = self.clone();
        let fut = async move {
            let unspent_map = this.list_unspent_group(addresses.clone(), decimals).compat().await?;
            let balances = addresses
                .into_iter()
                .map(|address| {
                    let balance = address_balance_from_unspent_map(&address, &unspent_map, decimals);
                    (address, balance)
                })
                .collect();
            Ok(balances)
        };
        Box::new(fut.boxed().compat())
    }

    fn estimate_fee_sat(
        &self,
        decimals: u8,
        fee_method: &EstimateFeeMethod,
        mode: &Option<EstimateFeeMode>,
        n_blocks: u32,
    ) -> UtxoRpcFut<u64> {
        match fee_method {
            EstimateFeeMethod::Standard => Box::new(self.estimate_fee(n_blocks).map(move |fee| {
                if fee > 0.00001 {
                    (fee * 10.0_f64.powf(decimals as f64)) as u64
                } else {
                    1000
                }
            })),
            EstimateFeeMethod::SmartFee => Box::new(self.estimate_smart_fee(mode, n_blocks).map(move |res| {
                if res.fee_rate > 0.00001 {
                    (res.fee_rate * 10.0_f64.powf(decimals as f64)) as u64
                } else {
                    1000
                }
            })),
        }
    }

    fn get_relay_fee(&self) -> RpcRes<BigDecimal> {
        Box::new(self.get_network_info().map(|info| info.relay_fee))
    }

    fn find_output_spend(
        &self,
        tx_hash: H256,
        _script_pubkey: &[u8],
        vout: usize,
        from_block: BlockHashOrHeight,
        tx_hash_algo: TxHashAlgo,
    ) -> Box<dyn Future<Item = Option<SpentOutputInfo>, Error = String> + Send> {
        let selfi = self.clone();
        let fut = async move {
            let from_block_hash = match from_block {
                BlockHashOrHeight::Height(h) => try_s!(selfi.get_block_hash(h as u64).compat().await),
                BlockHashOrHeight::Hash(h) => h,
            };
            let list_since_block: ListSinceBlockRes = try_s!(selfi.list_since_block(from_block_hash).compat().await);
            for transaction in list_since_block
                .transactions
                .into_iter()
                .filter(|tx| !tx.is_conflicting())
            {
                let maybe_spend_tx_bytes = try_s!(selfi.get_raw_transaction_bytes(&transaction.txid).compat().await);
                let mut maybe_spend_tx: UtxoTx =
                    try_s!(deserialize(maybe_spend_tx_bytes.as_slice()).map_err(|e| ERRL!("{:?}", e)));
                maybe_spend_tx.tx_hash_algo = tx_hash_algo;
                drop_mutability!(maybe_spend_tx);

                for (index, input) in maybe_spend_tx.inputs.iter().enumerate() {
                    if input.previous_output.hash == tx_hash && input.previous_output.index == vout as u32 {
                        return Ok(Some(SpentOutputInfo {
                            input: input.clone(),
                            input_index: index,
                            spending_tx: maybe_spend_tx,
                            spent_in_block: BlockHashOrHeight::Hash(transaction.blockhash),
                        }));
                    }
                }
            }
            Ok(None)
        };
        Box::new(fut.boxed().compat())
    }

    fn get_median_time_past(
        &self,
        starting_block: u64,
        count: NonZeroU64,
        _coin_variant: CoinVariant,
    ) -> UtxoRpcFut<u32> {
        let selfi = self.clone();
        let fut = async move {
            let starting_block_hash = selfi.get_block_hash(starting_block).compat().await?;
            let starting_block_data = selfi.get_block(starting_block_hash).compat().await?;
            if let Some(median) = starting_block_data.mediantime {
                return Ok(median);
            }

            let mut block_timestamps = vec![starting_block_data.time];
            let from = if starting_block <= count.get() {
                0
            } else {
                starting_block - count.get() + 1
            };
            for block_n in from..starting_block {
                let block_hash = selfi.get_block_hash(block_n).compat().await?;
                let block_data = selfi.get_block(block_hash).compat().await?;
                block_timestamps.push(block_data.time);
            }
            // can unwrap because count is non zero
            Ok(median(block_timestamps.as_mut_slice()).unwrap())
        };
        Box::new(fut.boxed().compat())
    }

    async fn get_block_timestamp(&self, height: u64) -> Result<u64, MmError<GetBlockHeaderError>> {
        let block = self.get_block_by_height(height).await.map_mm_err()?;
        Ok(block.time as u64)
    }
}

#[cfg_attr(test, mockable)]
impl NativeClient {
    /// https://developer.bitcoin.org/reference/rpc/listunspent
    pub fn list_unspent_impl(
        &self,
        min_conf: i32,
        max_conf: i32,
        addresses: Vec<String>,
    ) -> RpcRes<Vec<NativeUnspent>> {
        let request_fut = rpc_func!(self, "listunspent", &min_conf, &max_conf, &addresses);
        let arc = self.clone();
        let args = ListUnspentArgs {
            min_conf,
            max_conf,
            addresses,
        };
        let fut = async move { arc.list_unspent_concurrent_map.wrap_request(args, request_fut).await };
        Box::new(fut.boxed().compat())
    }

    pub fn list_all_transactions(&self, step: u64) -> RpcRes<Vec<ListTransactionsItem>> {
        let selfi = self.clone();
        let fut = async move {
            let mut from = 0;
            let mut transaction_list = Vec::new();

            loop {
                let transactions = selfi.list_transactions(step, from).compat().await?;
                if transactions.is_empty() {
                    return Ok(transaction_list);
                }

                transaction_list.extend(transactions);
                from += step;
            }
        };
        Box::new(fut.boxed().compat())
    }
}

impl NativeClient {
    pub async fn get_block_by_height(&self, height: u64) -> UtxoRpcResult<VerboseBlock> {
        let block_hash = self.get_block_hash(height).compat().await?;
        self.get_block(block_hash).compat().await
    }
}

#[cfg_attr(test, mockable)]
impl NativeClientImpl {
    /// https://developer.bitcoin.org/reference/rpc/importaddress
    pub fn import_address(&self, address: &str, label: &str, rescan: bool) -> RpcRes<()> {
        rpc_func!(self, "importaddress", address, label, rescan)
    }

    /// https://developer.bitcoin.org/reference/rpc/validateaddress
    pub fn validate_address(&self, address: &str) -> RpcRes<ValidateAddressRes> {
        rpc_func!(self, "validateaddress", address)
    }

    pub fn output_amount(
        &self,
        txid: H256Json,
        index: usize,
    ) -> Box<dyn Future<Item = u64, Error = String> + Send + 'static> {
        let fut = self.get_raw_transaction_bytes(&txid).map_err(|e| ERRL!("{}", e));
        Box::new(fut.and_then(move |bytes| {
            let tx: UtxoTx = try_s!(deserialize(bytes.as_slice()).map_err(|e| ERRL!(
                "Error {:?} trying to deserialize the transaction {:?}",
                e,
                bytes
            )));
            Ok(tx.outputs[index].value)
        }))
    }

    /// https://developer.bitcoin.org/reference/rpc/getblock.html
    /// Always returns verbose block
    pub fn get_block(&self, hash: H256Json) -> UtxoRpcFut<VerboseBlock> {
        let verbose = true;
        Box::new(rpc_func!(self, "getblock", hash, verbose).map_to_mm_fut(UtxoRpcError::from))
    }

    /// https://developer.bitcoin.org/reference/rpc/getblockhash.html
    pub fn get_block_hash(&self, height: u64) -> UtxoRpcFut<H256Json> {
        Box::new(rpc_func!(self, "getblockhash", height).map_to_mm_fut(UtxoRpcError::from))
    }

    /// https://developer.bitcoin.org/reference/rpc/getblockcount.html
    pub fn get_block_count(&self) -> RpcRes<u64> {
        rpc_func!(self, "getblockcount")
    }

    /// https://developer.bitcoin.org/reference/rpc/getrawtransaction.html
    /// Always returns verbose transaction
    fn get_raw_transaction_verbose(&self, txid: &H256Json) -> RpcRes<RpcTransaction> {
        let verbose = 1;
        rpc_func!(self, "getrawtransaction", txid, verbose)
    }

    /// https://developer.bitcoin.org/reference/rpc/getrawtransaction.html
    /// Always returns verbose transactions in the same order they were requested.
    fn get_raw_transaction_verbose_batch(&self, tx_ids: &[H256Json]) -> RpcRes<Vec<RpcTransaction>> {
        let verbose = 1;
        let requests = tx_ids
            .iter()
            .map(|txid| rpc_req!(self, "getrawtransaction", txid, verbose));
        self.batch_rpc(requests)
    }

    /// https://developer.bitcoin.org/reference/rpc/getrawtransaction.html
    /// Always returns transaction bytes
    pub fn get_raw_transaction_bytes(&self, txid: &H256Json) -> RpcRes<BytesJson> {
        let verbose = 0;
        rpc_func!(self, "getrawtransaction", txid, verbose)
    }

    /// https://developer.bitcoin.org/reference/rpc/estimatefee.html
    /// It is recommended to set n_blocks as low as possible.
    /// However, in some cases, n_blocks = 1 leads to an unreasonably high fee estimation.
    /// https://github.com/KomodoPlatform/atomicDEX-API/issues/656#issuecomment-743759659
    pub fn estimate_fee(&self, n_blocks: u32) -> UtxoRpcFut<f64> {
        Box::new(rpc_func!(self, "estimatefee", n_blocks).map_to_mm_fut(UtxoRpcError::from))
    }

    /// https://developer.bitcoin.org/reference/rpc/estimatesmartfee.html
    /// It is recommended to set n_blocks as low as possible.
    /// However, in some cases, n_blocks = 1 leads to an unreasonably high fee estimation.
    /// https://github.com/KomodoPlatform/atomicDEX-API/issues/656#issuecomment-743759659
    pub fn estimate_smart_fee(&self, mode: &Option<EstimateFeeMode>, n_blocks: u32) -> UtxoRpcFut<EstimateSmartFeeRes> {
        match mode {
            Some(m) => Box::new(rpc_func!(self, "estimatesmartfee", n_blocks, m).map_to_mm_fut(UtxoRpcError::from)),
            None => Box::new(rpc_func!(self, "estimatesmartfee", n_blocks).map_to_mm_fut(UtxoRpcError::from)),
        }
    }

    /// https://developer.bitcoin.org/reference/rpc/listtransactions.html
    pub fn list_transactions(&self, count: u64, from: u64) -> RpcRes<Vec<ListTransactionsItem>> {
        let account = "*";
        let watch_only = true;
        rpc_func!(self, "listtransactions", account, count, from, watch_only)
    }

    /// https://developer.bitcoin.org/reference/rpc/listreceivedbyaddress.html
    pub fn list_received_by_address(
        &self,
        min_conf: u64,
        include_empty: bool,
        include_watch_only: bool,
    ) -> RpcRes<Vec<ReceivedByAddressItem>> {
        rpc_func!(
            self,
            "listreceivedbyaddress",
            min_conf,
            include_empty,
            include_watch_only
        )
    }

    pub fn detect_fee_method(&self) -> impl Future<Item = EstimateFeeMethod, Error = String> + Send {
        let estimate_fee_fut = self.estimate_fee(1);
        self.estimate_smart_fee(&None, 1).then(move |res| -> Box<dyn Future<Item=EstimateFeeMethod, Error=String> + Send> {
            match res {
                Ok(smart_fee) => if smart_fee.fee_rate > 0. {
                    Box::new(futures01::future::ok(EstimateFeeMethod::SmartFee))
                } else {
                    info!("fee_rate from smart fee should be above zero, but got {:?}, trying estimatefee", smart_fee);
                    Box::new(estimate_fee_fut.map_err(|e| ERRL!("{}", e)).and_then(|res| if res > 0. {
                        Ok(EstimateFeeMethod::Standard)
                    } else {
                        ERR!("Estimate fee result should be above zero, but got {}, consider setting txfee in config", res)
                    }))
                },
                Err(e) => {
                    error!("Error {} on estimate smart fee, trying estimatefee", e);
                    Box::new(estimate_fee_fut.map_err(|e| ERRL!("{}", e)).and_then(|res| if res > 0. {
                        Ok(EstimateFeeMethod::Standard)
                    } else {
                        ERR!("Estimate fee result should be above zero, but got {}, consider setting txfee in config", res)
                    }))
                }
            }
        })
    }

    /// https://developer.bitcoin.org/reference/rpc/listsinceblock.html
    /// uses default target confirmations 1 and always includes watch_only addresses
    pub fn list_since_block(&self, block_hash: H256Json) -> RpcRes<ListSinceBlockRes> {
        let target_confirmations = 1;
        let include_watch_only = true;
        rpc_func!(
            self,
            "listsinceblock",
            block_hash,
            target_confirmations,
            include_watch_only
        )
    }

    /// https://developer.bitcoin.org/reference/rpc/sendtoaddress.html
    pub fn send_to_address(&self, addr: &str, amount: &BigDecimal) -> RpcRes<H256Json> {
        rpc_func!(self, "sendtoaddress", addr, amount)
    }

    /// Returns the list of addresses assigned the specified label.
    /// https://developer.bitcoin.org/reference/rpc/getaddressesbylabel.html
    pub fn get_addresses_by_label(&self, label: &str) -> RpcRes<AddressesByLabelResult> {
        rpc_func!(self, "getaddressesbylabel", label)
    }

    /// https://developer.bitcoin.org/reference/rpc/getnetworkinfo.html
    pub fn get_network_info(&self) -> RpcRes<NetworkInfo> {
        rpc_func!(self, "getnetworkinfo")
    }

    /// https://developer.bitcoin.org/reference/rpc/getaddressinfo.html
    pub fn get_address_info(&self, address: &str) -> RpcRes<GetAddressInfoRes> {
        rpc_func!(self, "getaddressinfo", address)
    }

    /// https://developer.bitcoin.org/reference/rpc/getblockheader.html
    pub fn get_block_header_bytes(&self, block_hash: H256Json) -> RpcRes<BytesJson> {
        let verbose = false;
        rpc_func!(self, "getblockheader", block_hash, verbose)
    }
}

impl NativeClientImpl {
    /// Check whether input address is imported to daemon
    pub async fn is_address_imported(&self, address: &str) -> Result<bool, String> {
        let validate_res = try_s!(self.validate_address(address).compat().await);
        match (validate_res.is_mine, validate_res.is_watch_only) {
            (Some(is_mine), Some(is_watch_only)) => Ok(is_mine || is_watch_only),
            // ignoring (Some(_), None) and (None, Some(_)) variants, there seem to be no known daemons that return is_mine,
            // but do not return is_watch_only, so it's ok to fallback to getaddressinfo
            _ => {
                let address_info = try_s!(self.get_address_info(address).compat().await);
                Ok(address_info.is_mine || address_info.is_watch_only)
            },
        }
    }
}

fn address_balance_from_unspent_map(address: &Address, unspent_map: &UnspentMap, decimals: u8) -> BigDecimal {
    let unspents = match unspent_map.get(address) {
        Some(unspents) => unspents,
        // If `balances` doesn't contain `address`, there are no unspents related to the address.
        // Consider the balance of that address equal to 0.
        None => return BigDecimal::from(0),
    };
    unspents.iter().fold(BigDecimal::from(0), |sum, unspent| {
        sum + big_decimal_from_sat_unsigned(unspent.value, decimals)
    })
}
