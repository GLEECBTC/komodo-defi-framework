use super::super::{
    BlockHashOrHeight, EstimateFeeMethod, EstimateFeeMode, SpentOutputInfo, UnspentInfo, UnspentMap,
    UtxoJsonRpcClientInfo, UtxoRpcClientOps, UtxoRpcError, UtxoRpcFut,
};
use super::connection::{ElectrumConnection, ElectrumConnectionErr, ElectrumConnectionSettings};
use super::connection_manager::ConnectionManager;
use super::constants::{
    BLOCKCHAIN_HEADERS_SUB_ID, BLOCKCHAIN_SCRIPTHASH_SUB_ID, ELECTRUM_REQUEST_TIMEOUT, NO_FORCE_CONNECT_METHODS,
    SEND_TO_ALL_METHODS,
};
use super::electrum_script_hash;
use super::event_handlers::ElectrumConnectionManagerNotifier;
use super::rpc_responses::*;

use crate::utxo::rpc_clients::ConcurrentRequestMap;
use crate::utxo::utxo_block_header_storage::BlockHeaderStorage;
use crate::utxo::{
    output_script, output_script_p2pk, GetBlockHeaderError, GetConfirmedTxError, GetTxHeightError,
    ScripthashNotification,
};
use crate::RpcTransportEventHandler;
use crate::SharableRpcTransportEventHandler;
use chain::{BlockHeader, Transaction as UtxoTx, TxHashAlgo};
use common::executor::abortable_queue::{AbortableQueue, WeakSpawner};
use common::jsonrpc_client::{
    JsonRpcBatchClient, JsonRpcClient, JsonRpcError, JsonRpcErrorType, JsonRpcId, JsonRpcMultiClient,
    JsonRpcRemoteAddr, JsonRpcRequest, JsonRpcRequestEnum, JsonRpcResponseEnum, JsonRpcResponseFut, RpcRes,
};
use common::log::warn;
use common::{median, OrdRange};
use keys::hash::H256;
use keys::Address;
use mm2_err_handle::prelude::*;
use mm2_number::BigDecimal;
#[cfg(test)]
use mocktopus::macros::*;
use rpc::v1::types::{Bytes as BytesJson, Transaction as RpcTransaction, H256 as H256Json};
use serialization::{
    deserialize, serialize, serialize_with_flags, CoinVariant, CompactInteger, Reader, SERIALIZE_TRANSACTION_WITNESS,
};
use spv_validation::helpers_validation::SPVError;
use spv_validation::storage::BlockHeaderStorageOps;

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::collections::HashSet;
use std::convert::TryInto;
use std::fmt::Debug;
use std::iter::FromIterator;
use std::num::NonZeroU64;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;

use crate::utxo::utxo_balance_events::UtxoBalanceEventStreamer;
use async_trait::async_trait;
use futures::compat::Future01CompatExt;
use futures::future::{join_all, FutureExt, TryFutureExt};
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use futures01::Future;
use itertools::Itertools;
use mm2_event_stream::{DeriveStreamerId, StreamingManager, StreamingManagerError};
use serde_json::{self as json, Value as Json};

type ElectrumTxHistory = Vec<ElectrumTxHistoryItem>;
type ElectrumScriptHash = String;
type ScriptHashUnspents = Vec<ElectrumUnspent>;

#[derive(Debug)]
pub struct ElectrumClientSettings {
    pub client_name: String,
    pub servers: Vec<ElectrumConnectionSettings>,
    pub coin_ticker: String,
    pub negotiate_version: bool,
    pub spawn_ping: bool,
    /// Minimum number of connections to keep alive at all times (best effort).
    pub min_connected: usize,
    /// Maximum number of connections to keep alive at any time.
    pub max_connected: usize,
}

#[derive(Debug)]
pub struct ElectrumClientImpl {
    client_name: String,
    coin_ticker: String,
    pub connection_manager: ConnectionManager,
    next_id: AtomicU64,
    negotiate_version: bool,
    protocol_version: OrdRange<f32>,
    get_balance_concurrent_map: ConcurrentRequestMap<String, ElectrumBalance>,
    list_unspent_concurrent_map: ConcurrentRequestMap<String, Vec<ElectrumUnspent>>,
    block_headers_storage: BlockHeaderStorage,
    /// Event handlers that are triggered on (dis)connection & transport events. They are wrapped
    /// in an `Arc` since they are shared outside `ElectrumClientImpl`. They are handed to each active
    /// `ElectrumConnection` to notify them about the events.
    event_handlers: Arc<Vec<Box<SharableRpcTransportEventHandler>>>,
    /// A streaming manager instance used to notify for Utxo balance events streamer.
    streaming_manager: StreamingManager,
    abortable_system: AbortableQueue,
}

#[cfg_attr(test, mockable)]
impl ElectrumClientImpl {
    /// Returns a new instance of `ElectrumClientImpl`.
    ///
    /// This doesn't initialize the connection manager contained within `ElectrumClientImpl`.
    /// Use `try_new_arc` to create an Arc-wrapped instance with an initialized connection manager.
    fn try_new(
        client_settings: ElectrumClientSettings,
        block_headers_storage: BlockHeaderStorage,
        streaming_manager: StreamingManager,
        abortable_system: AbortableQueue,
        mut event_handlers: Vec<Box<SharableRpcTransportEventHandler>>,
    ) -> Result<ElectrumClientImpl, String> {
        let connection_manager = ConnectionManager::try_new(
            client_settings.servers,
            client_settings.spawn_ping,
            (client_settings.min_connected, client_settings.max_connected),
            &abortable_system,
        )?;

        event_handlers.push(Box::new(ElectrumConnectionManagerNotifier {
            connection_manager: connection_manager.clone(),
        }));

        Ok(ElectrumClientImpl {
            client_name: client_settings.client_name,
            coin_ticker: client_settings.coin_ticker,
            connection_manager,
            next_id: 0.into(),
            negotiate_version: client_settings.negotiate_version,
            protocol_version: OrdRange::new(1.2, 1.4).unwrap(),
            get_balance_concurrent_map: ConcurrentRequestMap::new(),
            list_unspent_concurrent_map: ConcurrentRequestMap::new(),
            block_headers_storage,
            abortable_system,
            streaming_manager,
            event_handlers: Arc::new(event_handlers),
        })
    }

    /// Create a new Electrum client instance.
    /// This function initializes the connection manager and starts the connection process.
    pub fn try_new_arc(
        client_settings: ElectrumClientSettings,
        block_headers_storage: BlockHeaderStorage,
        streaming_manager: StreamingManager,
        abortable_system: AbortableQueue,
        event_handlers: Vec<Box<SharableRpcTransportEventHandler>>,
    ) -> Result<Arc<ElectrumClientImpl>, String> {
        let client_impl = Arc::new(ElectrumClientImpl::try_new(
            client_settings,
            block_headers_storage,
            streaming_manager,
            abortable_system,
            event_handlers,
        )?);
        // Initialize the connection manager.
        client_impl
            .connection_manager
            .initialize(Arc::downgrade(&client_impl))
            .map_err(|e| e.to_string())?;

        Ok(client_impl)
    }

    /// Remove an Electrum connection and stop corresponding spawned actor.
    pub fn remove_server(&self, server_addr: &str) -> Result<Arc<ElectrumConnection>, String> {
        self.connection_manager
            .remove_connection(server_addr)
            .map_err(|err| err.to_string())
    }

    /// Check if all connections have been removed.
    pub fn is_connections_pool_empty(&self) -> bool {
        self.connection_manager.is_connections_pool_empty()
    }

    /// Get available protocol versions.
    pub fn protocol_version(&self) -> &OrdRange<f32> {
        &self.protocol_version
    }

    pub fn coin_ticker(&self) -> &str {
        &self.coin_ticker
    }

    /// Whether to negotiate the protocol version.
    pub fn negotiate_version(&self) -> bool {
        self.negotiate_version
    }

    /// Get the event handlers.
    pub fn event_handlers(&self) -> Arc<Vec<Box<SharableRpcTransportEventHandler>>> {
        self.event_handlers.clone()
    }

    /// Sends a list of addresses through the scripthash notification sender to subscribe to their scripthash notifications.
    pub fn subscribe_addresses(&self, addresses: HashSet<Address>) -> Result<(), String> {
        match self.streaming_manager.send(
            &UtxoBalanceEventStreamer::derive_streamer_id(&self.coin_ticker),
            ScripthashNotification::SubscribeToAddresses(addresses),
        ) {
            // Don't error if the streamer isn't found/enabled.
            Err(StreamingManagerError::StreamerNotFound) | Ok(()) => Ok(()),
            Err(e) => Err(format!("Failed sending scripthash message. {e:?}")),
        }
    }

    /// Notifies the Utxo balance streamer of a new script hash balance change.
    ///
    /// The streamer will figure out which address this scripthash belongs to and will broadcast an notification to clients.
    pub fn notify_triggered_hash(&self, script_hash: String) -> Result<(), String> {
        match self.streaming_manager.send(
            &UtxoBalanceEventStreamer::derive_streamer_id(&self.coin_ticker),
            ScripthashNotification::Triggered(script_hash),
        ) {
            // Don't error if the streamer isn't found/enabled.
            Err(StreamingManagerError::StreamerNotFound) | Ok(()) => Ok(()),
            Err(e) => Err(format!("Failed sending scripthash message. {e:?}")),
        }
    }

    /// Get block headers storage.
    pub fn block_headers_storage(&self) -> &BlockHeaderStorage {
        &self.block_headers_storage
    }

    pub fn weak_spawner(&self) -> WeakSpawner {
        self.abortable_system.weak_spawner()
    }

    #[cfg(test)]
    pub fn with_protocol_version(
        client_settings: ElectrumClientSettings,
        block_headers_storage: BlockHeaderStorage,
        streaming_manager: StreamingManager,
        abortable_system: AbortableQueue,
        event_handlers: Vec<Box<SharableRpcTransportEventHandler>>,
        protocol_version: OrdRange<f32>,
    ) -> Result<Arc<ElectrumClientImpl>, String> {
        let client_impl = Arc::new(ElectrumClientImpl {
            protocol_version,
            ..ElectrumClientImpl::try_new(
                client_settings,
                block_headers_storage,
                streaming_manager,
                abortable_system,
                event_handlers,
            )?
        });
        // Initialize the connection manager.
        client_impl
            .connection_manager
            .initialize(Arc::downgrade(&client_impl))
            .map_err(|e| e.to_string())?;

        Ok(client_impl)
    }
}

#[derive(Clone, Debug)]
pub struct ElectrumClient(pub Arc<ElectrumClientImpl>);

impl Deref for ElectrumClient {
    type Target = ElectrumClientImpl;
    fn deref(&self) -> &ElectrumClientImpl {
        &self.0
    }
}

impl UtxoJsonRpcClientInfo for ElectrumClient {
    fn coin_name(&self) -> &str {
        self.coin_ticker.as_str()
    }
}

impl JsonRpcClient for ElectrumClient {
    fn version(&self) -> &'static str {
        "2.0"
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, AtomicOrdering::Relaxed)
    }

    fn client_info(&self) -> String {
        UtxoJsonRpcClientInfo::client_info(self)
    }

    fn transport(&self, request: JsonRpcRequestEnum) -> JsonRpcResponseFut {
        Box::new(self.clone().electrum_request_multi(request).boxed().compat())
    }
}

impl JsonRpcBatchClient for ElectrumClient {}

impl JsonRpcMultiClient for ElectrumClient {
    fn transport_exact(&self, to_addr: String, request: JsonRpcRequestEnum) -> JsonRpcResponseFut {
        Box::new(
            self.clone()
                .electrum_request_to(to_addr.clone(), request)
                .map_ok(|response| (JsonRpcRemoteAddr(to_addr), response))
                .boxed()
                .compat(),
        )
    }
}

#[cfg_attr(test, mockable)]
impl ElectrumClient {
    pub fn try_new(
        client_settings: ElectrumClientSettings,
        event_handlers: Vec<Box<SharableRpcTransportEventHandler>>,
        block_headers_storage: BlockHeaderStorage,
        streaming_manager: StreamingManager,
        abortable_system: AbortableQueue,
    ) -> Result<ElectrumClient, String> {
        let client = ElectrumClient(ElectrumClientImpl::try_new_arc(
            client_settings,
            block_headers_storage,
            streaming_manager,
            abortable_system,
            event_handlers,
        )?);

        Ok(client)
    }

    /// Sends a JSONRPC request to all the connected servers.
    ///
    /// This method will block until a response is received from at least one server.
    async fn electrum_request_multi(
        self,
        request: JsonRpcRequestEnum,
    ) -> Result<(JsonRpcRemoteAddr, JsonRpcResponseEnum), JsonRpcErrorType> {
        // Whether to send the request to all active connections or not.
        let send_to_all = matches!(request, JsonRpcRequestEnum::Single(ref req) if SEND_TO_ALL_METHODS.contains(&req.method.as_str()));
        // Request id and serialized request.
        let req_id = request.rpc_id();
        let request = json::to_string(&request).map_err(|e| JsonRpcErrorType::InvalidRequest(e.to_string()))?;
        let request = (req_id, request);
        // Use the active connections for this request.
        let connections = self.connection_manager.get_active_connections();
        // Maximum number of connections to establish or use in request concurrently. Could be up to connections.len().
        let concurrency = if send_to_all { connections.len() } else { 1 };
        match self
            .send_request_using(&request, connections, send_to_all, concurrency)
            .await
        {
            Ok(response) => Ok(response),
            // If we failed the request using only the active connections, try again using all connections.
            Err(_) if !send_to_all => {
                warn!(
                    "[coin={}] Failed to send the request using active connections, trying all connections.",
                    self.coin_ticker()
                );
                let connections = self.connection_manager.get_all_connections();
                // At this point we should have all the connections disconnected since all
                // the active connections failed (and we disconnected them in the process).
                // So use a higher concurrency to speed up the response time.
                //
                // Note that a side effect of this is that we might break the `max_connected` threshold for
                // a short time since the connection manager's background task will be trying to establish
                // connections at the same time. This is not as bad though since the manager's background task
                // tries connections sequentially and we are expected for finish much quicker due to parallelizing.
                let concurrency = self.connection_manager.config().max_connected;
                match self.send_request_using(&request, connections, false, concurrency).await {
                    Ok(response) => Ok(response),
                    Err(err_vec) => Err(JsonRpcErrorType::Internal(format!("All servers errored: {err_vec:?}"))),
                }
            },
            Err(e) => Err(JsonRpcErrorType::Internal(format!("All servers errored: {e:?}"))),
        }
    }

    /// Sends a JSONRPC request to a specific electrum server.
    ///
    /// This will try to wake up the server connection if it's not connected.
    async fn electrum_request_to(
        self,
        to_addr: String,
        request: JsonRpcRequestEnum,
    ) -> Result<JsonRpcResponseEnum, JsonRpcErrorType> {
        // Whether to force the connection to be established (if not) before sending the request.
        let force_connect = !matches!(request, JsonRpcRequestEnum::Single(ref req) if NO_FORCE_CONNECT_METHODS.contains(&req.method.as_str()));
        let json = json::to_string(&request).map_err(|err| JsonRpcErrorType::InvalidRequest(err.to_string()))?;

        let connection = self
            .connection_manager
            .get_connection_by_address(&to_addr, force_connect)
            .await
            .map_err(|err| JsonRpcErrorType::Internal(err.to_string()))?;

        let response = connection
            .electrum_request(json, request.rpc_id(), ELECTRUM_REQUEST_TIMEOUT)
            .await;
        // If the request was not forcefully connected, we shouldn't inform the connection manager that it's
        // not needed anymore, as we didn't force spawn it in the first place.
        // This fixes dropping the connection after the version check request, as we don't mark the connection
        // maintained till after the version is checked.
        if force_connect {
            // Inform the connection manager that the connection was queried and no longer needed now.
            self.connection_manager.not_needed(&to_addr);
        }

        response
    }

    /// Sends a JSONRPC request to all the given connections in parallel and returns
    /// the first successful response if there is any, or a vector of errors otherwise.
    ///
    /// If `send_to_all` is set to `true`, we won't return on first successful response but
    /// wait for all responses to come back first.
    async fn send_request_using(
        &self,
        request: &(JsonRpcId, String),
        connections: Vec<Arc<ElectrumConnection>>,
        send_to_all: bool,
        max_concurrency: usize,
    ) -> Result<(JsonRpcRemoteAddr, JsonRpcResponseEnum), Vec<(JsonRpcRemoteAddr, JsonRpcErrorType)>> {
        let max_concurrency = max_concurrency.max(1);
        // Create the request
        let chunked_requests = connections.chunks(max_concurrency).map(|chunk| {
            FuturesUnordered::from_iter(chunk.iter().map(|connection| {
                let client = self.clone();
                let req_id = request.0;
                let req_json = request.1.clone();
                async move {
                    let connection_is_established = connection
                        // We first make sure that the connection loop is established before sending the request.
                        .establish_connection_loop(client)
                        .await
                        .map_err(|e| JsonRpcErrorType::Transport(format!("Failed to establish connection: {e:?}")));
                    let response = match connection_is_established {
                        Ok(_) => {
                            // Perform the request.
                            connection
                                .electrum_request(req_json, req_id, ELECTRUM_REQUEST_TIMEOUT)
                                .await
                        },
                        Err(e) => Err(e),
                    };
                    (response, connection.clone())
                }
            }))
        });
        let client = self.clone();
        let mut final_response = None;
        let mut errors = Vec::new();
        // Iterate over the request chunks sequentially.
        for mut requests in chunked_requests {
            // For each chunk, iterate over the requests in parallel.
            while let Some((response, connection)) = requests.next().await {
                let address = JsonRpcRemoteAddr(connection.address().to_string());
                match response {
                    Ok(response) => {
                        if final_response.is_none() {
                            final_response = Some((address, response));
                        }
                        client.connection_manager.not_needed(connection.address());
                        if !send_to_all {
                            if let Some(response) = final_response {
                                return Ok(response);
                            }
                        }
                    },
                    Err(e) => {
                        warn!(
                            "[coin={}], Error while sending request to {address:?}: {e:?}",
                            client.coin_ticker()
                        );
                        connection.disconnect(Some(ElectrumConnectionErr::Temporary(format!(
                            "Forcefully disconnected for erroring: {e:?}."
                        ))));
                        client.event_handlers.on_disconnected(connection.address()).ok();
                        errors.push((address, e))
                    },
                }
            }
        }
        final_response.ok_or(errors)
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#server-ping
    pub fn server_ping(&self) -> RpcRes<()> {
        rpc_func!(self, "server.ping")
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#server-version
    pub fn server_version(&self, server_address: &str, version: &OrdRange<f32>) -> RpcRes<ElectrumProtocolVersion> {
        let protocol_version: Vec<String> = version.flatten().into_iter().map(|v| format!("{v}")).collect();
        rpc_func_from!(
            self,
            server_address,
            "server.version",
            &self.client_name,
            protocol_version
        )
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-headers-subscribe
    pub fn get_block_count_from(&self, server_address: &str) -> RpcRes<u64> {
        Box::new(
            rpc_func_from!(self, server_address, BLOCKCHAIN_HEADERS_SUB_ID)
                .map(|r: ElectrumBlockHeader| r.block_height()),
        )
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-block-headers
    pub fn get_block_headers_from(
        &self,
        server_address: &str,
        start_height: u64,
        count: NonZeroU64,
    ) -> RpcRes<ElectrumBlockHeadersRes> {
        rpc_func_from!(self, server_address, "blockchain.block.headers", start_height, count)
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-scripthash-listunspent
    /// It can return duplicates sometimes: https://github.com/artemii235/SuperNET/issues/269
    /// We should remove them to build valid transactions
    pub fn scripthash_list_unspent(&self, hash: &str) -> RpcRes<Vec<ElectrumUnspent>> {
        let request_fut = Box::new(rpc_func!(self, "blockchain.scripthash.listunspent", hash).and_then(
            move |unspents: Vec<ElectrumUnspent>| {
                let mut map: HashMap<(H256Json, u32), bool> = HashMap::new();
                let unspents = unspents
                    .into_iter()
                    .filter(|unspent| match map.entry((unspent.tx_hash, unspent.tx_pos)) {
                        Entry::Occupied(_) => false,
                        Entry::Vacant(e) => {
                            e.insert(true);
                            true
                        },
                    })
                    .collect();
                Ok(unspents)
            },
        ));
        let arc = self.clone();
        let hash = hash.to_owned();
        let fut = async move { arc.list_unspent_concurrent_map.wrap_request(hash, request_fut).await };
        Box::new(fut.boxed().compat())
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-scripthash-listunspent
    /// It can return duplicates sometimes: https://github.com/artemii235/SuperNET/issues/269
    /// We should remove them to build valid transactions.
    /// Please note the function returns `ScriptHashUnspents` elements in the same order in which they were requested.
    pub fn scripthash_list_unspent_batch(&self, hashes: Vec<ElectrumScriptHash>) -> RpcRes<Vec<ScriptHashUnspents>> {
        let requests = hashes
            .iter()
            .map(|hash| rpc_req!(self, "blockchain.scripthash.listunspent", hash));
        Box::new(self.batch_rpc(requests).map(move |unspents: Vec<ScriptHashUnspents>| {
            unspents
                .into_iter()
                .map(|hash_unspents| {
                    hash_unspents
                        .into_iter()
                        .unique_by(|unspent| (unspent.tx_hash, unspent.tx_pos))
                        .collect::<Vec<_>>()
                })
                .collect()
        }))
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-scripthash-get-history
    pub fn scripthash_get_history(&self, hash: &str) -> RpcRes<ElectrumTxHistory> {
        rpc_func!(self, "blockchain.scripthash.get_history", hash)
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-scripthash-get-history
    /// Requests history of the `hashes` in a batch and returns them in the same order they were requested.
    pub fn scripthash_get_history_batch<I>(&self, hashes: I) -> RpcRes<Vec<ElectrumTxHistory>>
    where
        I: IntoIterator<Item = String>,
    {
        let requests = hashes
            .into_iter()
            .map(|hash| rpc_req!(self, "blockchain.scripthash.get_history", hash));
        self.batch_rpc(requests)
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-scripthash-gethistory
    pub fn scripthash_get_balance(&self, hash: &str) -> RpcRes<ElectrumBalance> {
        let arc = self.clone();
        let hash = hash.to_owned();
        let fut = async move {
            let request = rpc_func!(arc, "blockchain.scripthash.get_balance", &hash);
            arc.get_balance_concurrent_map.wrap_request(hash, request).await
        };
        Box::new(fut.boxed().compat())
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-scripthash-gethistory
    /// Requests balances in a batch and returns them in the same order they were requested.
    pub fn scripthash_get_balances<I>(&self, hashes: I) -> RpcRes<Vec<ElectrumBalance>>
    where
        I: IntoIterator<Item = String>,
    {
        let requests = hashes
            .into_iter()
            .map(|hash| rpc_req!(self, "blockchain.scripthash.get_balance", &hash));
        self.batch_rpc(requests)
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-headers-subscribe
    pub fn blockchain_headers_subscribe(&self) -> RpcRes<ElectrumBlockHeader> {
        rpc_func!(self, BLOCKCHAIN_HEADERS_SUB_ID)
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-transaction-broadcast
    pub fn blockchain_transaction_broadcast(&self, tx: BytesJson) -> RpcRes<H256Json> {
        rpc_func!(self, "blockchain.transaction.broadcast", tx)
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-estimatefee
    /// It is recommended to set n_blocks as low as possible.
    /// However, in some cases, n_blocks = 1 leads to an unreasonably high fee estimation.
    /// https://github.com/KomodoPlatform/atomicDEX-API/issues/656#issuecomment-743759659
    pub fn estimate_fee(&self, mode: &Option<EstimateFeeMode>, n_blocks: u32) -> UtxoRpcFut<f64> {
        match mode {
            Some(m) => {
                Box::new(rpc_func!(self, "blockchain.estimatefee", n_blocks, m).map_to_mm_fut(UtxoRpcError::from))
            },
            None => Box::new(rpc_func!(self, "blockchain.estimatefee", n_blocks).map_to_mm_fut(UtxoRpcError::from)),
        }
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-block-header
    pub fn blockchain_block_header(&self, height: u64) -> RpcRes<BytesJson> {
        rpc_func!(self, "blockchain.block.header", height)
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-block-headers
    pub fn blockchain_block_headers(&self, start_height: u64, count: NonZeroU64) -> RpcRes<ElectrumBlockHeadersRes> {
        rpc_func!(self, "blockchain.block.headers", start_height, count)
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-transaction-get-merkle
    pub fn blockchain_transaction_get_merkle(&self, txid: H256Json, height: u64) -> RpcRes<TxMerkleBranch> {
        rpc_func!(self, "blockchain.transaction.get_merkle", txid, height)
    }

    // get_tx_height_from_rpc is costly since it loops through history after requesting the whole history of the script pubkey
    // This method should always be used if the block headers are saved to the DB
    async fn get_tx_height_from_storage(&self, tx: &UtxoTx) -> Result<u64, MmError<GetTxHeightError>> {
        let tx_hash = tx.hash().reversed();
        let blockhash = self
            .get_verbose_transaction(&tx_hash.into())
            .compat()
            .await
            .map_mm_err()?
            .blockhash;
        Ok(self
            .block_headers_storage()
            .get_block_height_by_hash(blockhash.into())
            .await?
            .ok_or_else(|| {
                GetTxHeightError::HeightNotFound(format!(
                    "Transaction block header is not found in storage for {}",
                    self.coin_ticker()
                ))
            })?
            .try_into()?)
    }

    // get_tx_height_from_storage is always preferred to be used instead of this, but if there is no headers in storage (storing headers is not enabled)
    // this function can be used instead
    async fn get_tx_height_from_rpc(&self, tx: &UtxoTx) -> Result<u64, GetTxHeightError> {
        let selfi = self;
        for output in tx.outputs.clone() {
            let script_pubkey_str = hex::encode(electrum_script_hash(&output.script_pubkey));
            if let Ok(history) = selfi.scripthash_get_history(script_pubkey_str.as_str()).compat().await {
                if let Some(item) = history
                    .into_iter()
                    .find(|item| item.tx_hash.reversed() == H256Json(*tx.hash()) && item.height > 0)
                {
                    return Ok(item.height as u64);
                }
            }
        }
        Err(GetTxHeightError::HeightNotFound(format!(
            "Couldn't find height through electrum for {}",
            selfi.coin_ticker
        )))
    }

    async fn block_header_from_storage(&self, height: u64) -> Result<BlockHeader, MmError<GetBlockHeaderError>> {
        self.block_headers_storage()
            .get_block_header(height)
            .await?
            .ok_or_else(|| {
                GetBlockHeaderError::Internal(format!("Header not found in storage for {}", self.coin_ticker)).into()
            })
    }

    async fn block_header_from_storage_or_rpc(&self, height: u64) -> Result<BlockHeader, MmError<GetBlockHeaderError>> {
        match self.block_header_from_storage(height).await {
            Ok(h) => Ok(h),
            Err(_) => Ok(deserialize(
                self.blockchain_block_header(height).compat().await?.as_slice(),
            )?),
        }
    }

    pub async fn get_confirmed_tx_info_from_rpc(
        &self,
        tx: &UtxoTx,
    ) -> Result<ConfirmedTransactionInfo, GetConfirmedTxError> {
        let height = self.get_tx_height_from_rpc(tx).await?;

        let merkle_branch = self
            .blockchain_transaction_get_merkle(tx.hash().reversed().into(), height)
            .compat()
            .await?;

        let header = deserialize(self.blockchain_block_header(height).compat().await?.as_slice())?;

        Ok(ConfirmedTransactionInfo {
            tx: tx.clone(),
            header,
            index: merkle_branch.pos as u64,
            height,
        })
    }

    pub async fn get_merkle_and_validated_header(
        &self,
        tx: &UtxoTx,
    ) -> Result<(TxMerkleBranch, BlockHeader, u64), MmError<SPVError>> {
        let height = self.get_tx_height_from_storage(tx).await.map_mm_err()?;

        let merkle_branch = self
            .blockchain_transaction_get_merkle(tx.hash().reversed().into(), height)
            .compat()
            .await
            .map_to_mm(|err| SPVError::UnableToGetMerkle {
                coin: self.coin_ticker.clone(),
                err: err.to_string(),
            })?;

        let header = self.block_header_from_storage(height).await.map_mm_err()?;

        Ok((merkle_branch, header, height))
    }

    pub fn retrieve_headers_from(
        &self,
        server_address: &str,
        from_height: u64,
        to_height: u64,
    ) -> UtxoRpcFut<(HashMap<u64, BlockHeader>, Vec<BlockHeader>)> {
        let coin_name = self.coin_ticker.clone();
        if from_height == 0 || to_height < from_height {
            return Box::new(futures01::future::err(
                UtxoRpcError::Internal("Invalid values for from/to parameters".to_string()).into(),
            ));
        }
        let count: NonZeroU64 = match (to_height - from_height + 1).try_into() {
            Ok(c) => c,
            Err(e) => return Box::new(futures01::future::err(UtxoRpcError::Internal(e.to_string()).into())),
        };
        Box::new(
            self.get_block_headers_from(server_address, from_height, count)
                .map_to_mm_fut(UtxoRpcError::from)
                .and_then(move |headers| {
                    let (block_registry, block_headers) = {
                        if headers.count == 0 {
                            return MmError::err(UtxoRpcError::Internal("No headers available".to_string()));
                        }
                        let len = CompactInteger::from(headers.count);
                        let mut serialized = serialize(&len).take();
                        serialized.extend(headers.hex.0);
                        drop_mutability!(serialized);
                        let mut reader =
                            Reader::new_with_coin_variant(serialized.as_slice(), coin_name.as_str().into());
                        let maybe_block_headers = reader.read_list::<BlockHeader>();
                        let block_headers = match maybe_block_headers {
                            Ok(headers) => headers,
                            Err(e) => return MmError::err(UtxoRpcError::InvalidResponse(format!("{e:?}"))),
                        };
                        let mut block_registry: HashMap<u64, BlockHeader> = HashMap::new();
                        let mut starting_height = from_height;
                        for block_header in &block_headers {
                            block_registry.insert(starting_height, block_header.clone());
                            starting_height += 1;
                        }
                        (block_registry, block_headers)
                    };
                    Ok((block_registry, block_headers))
                }),
        )
    }

    pub(crate) fn get_servers_with_latest_block_count(&self) -> UtxoRpcFut<(Vec<String>, u64)> {
        let selfi = self.clone();
        let fut = async move {
            let addresses = selfi.connection_manager.get_all_server_addresses();
            let futures = addresses
                .into_iter()
                .map(|address| {
                    selfi
                        .get_block_count_from(&address)
                        .map(|response| (address, response))
                        .compat()
                })
                .collect::<Vec<_>>();

            let responses = join_all(futures).await;

            // First, we use filter_map to get rid of any errors and collect the
            // server addresses and block counts into two vectors
            let (responding_servers, block_counts_from_all_servers): (Vec<_>, Vec<_>) =
                responses.clone().into_iter().filter_map(|res| res.ok()).unzip();

            // Next, we use max to find the maximum block count from all servers
            if let Some(max_block_count) = block_counts_from_all_servers.clone().iter().max() {
                // Then, we use filter and collect to get the servers that have the maximum block count
                let servers_with_max_count: Vec<_> = responding_servers
                    .into_iter()
                    .zip(block_counts_from_all_servers)
                    .filter(|(_, count)| count == max_block_count)
                    .map(|(addr, _)| addr)
                    .collect();

                // Finally, we return a tuple of servers with max count and the max count
                return Ok((servers_with_max_count, *max_block_count));
            }

            Err(MmError::new(UtxoRpcError::Internal(format!(
                "Couldn't get block count from any server for {}, responses: {:?}",
                &selfi.coin_ticker, responses
            ))))
        };

        Box::new(fut.boxed().compat())
    }
}

// if mockable is placed before async_trait there is `munmap_chunk(): invalid pointer` error on async fn mocking attempt
#[async_trait]
#[cfg_attr(test, mockable)]
impl UtxoRpcClientOps for ElectrumClient {
    fn list_unspent(&self, address: &Address, _decimals: u8) -> UtxoRpcFut<Vec<UnspentInfo>> {
        let mut output_scripts = vec![try_f!(output_script(address))];

        // If the plain pubkey is available, fetch the UTXOs found in P2PK outputs as well (if any).
        if let Some(pubkey) = address.pubkey() {
            // We don't want to show P2PK outputs along with segwit ones (P2WPKH).
            // Allow listing the P2PK outputs only if the address is not segwit (i.e. show P2PK outputs along with P2PKH).
            if !address.addr_format().is_segwit() {
                let p2pk_output_script = output_script_p2pk(pubkey);
                output_scripts.push(p2pk_output_script);
            }
        }

        let this = self.clone();
        let fut = async move {
            let hashes = output_scripts
                .iter()
                .map(|s| hex::encode(electrum_script_hash(s)))
                .collect();
            let unspents = this.scripthash_list_unspent_batch(hashes).compat().await?;

            let unspents = unspents
                .into_iter()
                .zip(output_scripts)
                .flat_map(|(unspents, output_script)| {
                    unspents
                        .into_iter()
                        .map(move |unspent| UnspentInfo::from_electrum(unspent, output_script.clone()))
                })
                .collect();
            Ok(unspents)
        };

        Box::new(fut.boxed().compat())
    }

    fn list_unspent_group(&self, addresses: Vec<Address>, _decimals: u8) -> UtxoRpcFut<UnspentMap> {
        let output_scripts = try_f!(addresses
            .iter()
            .map(output_script)
            .collect::<Result<Vec<_>, keys::Error>>());

        let this = self.clone();
        let fut = async move {
            let hashes = output_scripts
                .iter()
                .map(|s| hex::encode(electrum_script_hash(s)))
                .collect();
            let unspents = this.scripthash_list_unspent_batch(hashes).compat().await?;

            let unspents: Vec<Vec<UnspentInfo>> = unspents
                .into_iter()
                .zip(output_scripts)
                .map(|(unspents, output_script)| {
                    unspents
                        .into_iter()
                        .map(|unspent| UnspentInfo::from_electrum(unspent, output_script.clone()))
                        .collect()
                })
                .collect();

            let unspent_map = addresses
                .into_iter()
                // `scripthash_list_unspent_batch` returns `ScriptHashUnspents` elements in the same order in which they were requested.
                // So we can zip `addresses` and `unspents` into one iterator.
                .zip(unspents)
                .collect();
            Ok(unspent_map)
        };
        Box::new(fut.boxed().compat())
    }

    fn send_transaction(&self, tx: &UtxoTx) -> UtxoRpcFut<H256Json> {
        let bytes = if tx.has_witness() {
            BytesJson::from(serialize_with_flags(tx, SERIALIZE_TRANSACTION_WITNESS))
        } else {
            BytesJson::from(serialize(tx))
        };
        Box::new(
            self.blockchain_transaction_broadcast(bytes)
                .map_to_mm_fut(UtxoRpcError::from),
        )
    }

    fn send_raw_transaction(&self, tx: BytesJson) -> UtxoRpcFut<H256Json> {
        Box::new(
            self.blockchain_transaction_broadcast(tx)
                .map_to_mm_fut(UtxoRpcError::from),
        )
    }

    fn blockchain_scripthash_subscribe_using(&self, server_address: &str, scripthash: String) -> UtxoRpcFut<Json> {
        Box::new(
            rpc_func_from!(self, server_address, BLOCKCHAIN_SCRIPTHASH_SUB_ID, scripthash)
                .map_to_mm_fut(UtxoRpcError::from),
        )
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-transaction-get
    /// returns transaction bytes by default
    fn get_transaction_bytes(&self, txid: &H256Json) -> UtxoRpcFut<BytesJson> {
        let verbose = false;
        Box::new(rpc_func!(self, "blockchain.transaction.get", txid, verbose).map_to_mm_fut(UtxoRpcError::from))
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-transaction-get
    /// returns verbose transaction by default
    fn get_verbose_transaction(&self, txid: &H256Json) -> UtxoRpcFut<RpcTransaction> {
        let verbose = true;
        Box::new(rpc_func!(self, "blockchain.transaction.get", txid, verbose).map_to_mm_fut(UtxoRpcError::from))
    }

    /// https://electrumx.readthedocs.io/en/latest/protocol-methods.html#blockchain-transaction-get
    /// Returns verbose transactions in a batch.
    fn get_verbose_transactions(&self, tx_ids: &[H256Json]) -> UtxoRpcFut<Vec<RpcTransaction>> {
        let verbose = true;
        let requests = tx_ids
            .iter()
            .map(|txid| rpc_req!(self, "blockchain.transaction.get", txid, verbose));
        Box::new(self.batch_rpc(requests).map_to_mm_fut(UtxoRpcError::from))
    }

    fn get_block_count(&self) -> UtxoRpcFut<u64> {
        Box::new(
            self.blockchain_headers_subscribe()
                .map(|r| r.block_height())
                .map_to_mm_fut(UtxoRpcError::from),
        )
    }

    fn display_balance(&self, address: Address, decimals: u8) -> RpcRes<BigDecimal> {
        let output_script = try_f!(output_script(&address).map_err(|err| JsonRpcError::new(
            UtxoJsonRpcClientInfo::client_info(self),
            rpc_req!(self, "blockchain.scripthash.get_balance").into(),
            JsonRpcErrorType::Internal(err.to_string())
        )));
        let mut hashes = vec![hex::encode(electrum_script_hash(&output_script))];

        // If the plain pubkey is available, fetch the balance found in P2PK output as well (if any).
        if let Some(pubkey) = address.pubkey() {
            // Show the balance in P2PK outputs only for the non-segwit legacy addresses (P2PKH).
            if !address.addr_format().is_segwit() {
                let p2pk_output_script = output_script_p2pk(pubkey);
                hashes.push(hex::encode(electrum_script_hash(&p2pk_output_script)));
            }
        }

        let this = self.clone();
        let fut = async move {
            Ok(this
                .scripthash_get_balances(hashes)
                .compat()
                .await?
                .into_iter()
                .fold(BigDecimal::from(0), |sum, electrum_balance| {
                    sum + electrum_balance.to_big_decimal(decimals)
                }))
        };
        Box::new(fut.boxed().compat())
    }

    fn display_balances(&self, addresses: Vec<Address>, decimals: u8) -> UtxoRpcFut<Vec<(Address, BigDecimal)>> {
        let this = self.clone();
        let fut = async move {
            let hashes = addresses
                .iter()
                .map(|address| {
                    let output_script = output_script(address)?;
                    let hash = electrum_script_hash(&output_script);

                    Ok(hex::encode(hash))
                })
                .collect::<Result<Vec<_>, keys::Error>>()?;

            let electrum_balances = this.scripthash_get_balances(hashes).compat().await?;
            let balances = electrum_balances
                .into_iter()
                // `scripthash_get_balances` returns `ElectrumBalance` elements in the same order in which they were requested.
                // So we can zip `addresses` and the balances into one iterator.
                .zip(addresses)
                .map(|(electrum_balance, address)| (address, electrum_balance.to_big_decimal(decimals)))
                .collect();
            Ok(balances)
        };

        Box::new(fut.boxed().compat())
    }

    fn estimate_fee_sat(
        &self,
        decimals: u8,
        _fee_method: &EstimateFeeMethod,
        mode: &Option<EstimateFeeMode>,
        n_blocks: u32,
    ) -> UtxoRpcFut<u64> {
        Box::new(self.estimate_fee(mode, n_blocks).map(move |fee| {
            if fee > 0.00001 {
                (fee * 10.0_f64.powf(decimals as f64)) as u64
            } else {
                1000
            }
        }))
    }

    fn get_relay_fee(&self) -> RpcRes<BigDecimal> {
        rpc_func!(self, "blockchain.relayfee")
    }

    fn find_output_spend(
        &self,
        tx_hash: H256,
        script_pubkey: &[u8],
        vout: usize,
        _from_block: BlockHashOrHeight,
        tx_hash_algo: TxHashAlgo,
    ) -> Box<dyn Future<Item = Option<SpentOutputInfo>, Error = String> + Send> {
        let selfi = self.clone();
        let script_hash = hex::encode(electrum_script_hash(script_pubkey));
        let fut = async move {
            let history = try_s!(selfi.scripthash_get_history(&script_hash).compat().await);

            if history.len() < 2 {
                return Ok(None);
            }

            for item in history.iter() {
                let transaction = try_s!(selfi.get_transaction_bytes(&item.tx_hash).compat().await);

                let mut maybe_spend_tx: UtxoTx =
                    try_s!(deserialize(transaction.as_slice()).map_err(|e| ERRL!("{:?}", e)));
                maybe_spend_tx.tx_hash_algo = tx_hash_algo;
                drop_mutability!(maybe_spend_tx);

                for (index, input) in maybe_spend_tx.inputs.iter().enumerate() {
                    if input.previous_output.hash == tx_hash && input.previous_output.index == vout as u32 {
                        return Ok(Some(SpentOutputInfo {
                            input: input.clone(),
                            input_index: index,
                            spending_tx: maybe_spend_tx,
                            spent_in_block: BlockHashOrHeight::Height(item.height),
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
        coin_variant: CoinVariant,
    ) -> UtxoRpcFut<u32> {
        let from = if starting_block <= count.get() {
            0
        } else {
            starting_block - count.get() + 1
        };

        let coin_name = self.coin_ticker.clone();
        let requested_count = count.get();

        Box::new(
            self.blockchain_block_headers(from, count)
                .map_to_mm_fut(UtxoRpcError::from)
                .and_then(move |res| {
                    if res.count == 0 {
                        return MmError::err(UtxoRpcError::InvalidResponse("Server returned zero count".to_owned()));
                    }
                    let res_count = res.count;
                    let len = CompactInteger::from(res_count);
                    let mut serialized = serialize(&len).take();
                    serialized.extend(res.hex.0);
                    let mut reader = Reader::new_with_coin_variant(serialized.as_slice(), coin_variant);

                    let headers = reader.read_list::<BlockHeader>().map_to_mm(|e| UtxoRpcError::InvalidResponse(format!(
                            "blockchain.block.headers: failed to parse list of {} headers (coin={}, from={}, requested_count={}): {}",
                            res_count, coin_name, from, requested_count, e,
                        )))?;

                    let mut timestamps: Vec<_> = headers.into_iter().map(|block| block.time).collect();
                    // can unwrap because count is non zero
                    Ok(median(timestamps.as_mut_slice()).unwrap())
                }),
        )
    }

    async fn get_block_timestamp(&self, height: u64) -> Result<u64, MmError<GetBlockHeaderError>> {
        Ok(self.block_header_from_storage_or_rpc(height).await?.time as u64)
    }
}
