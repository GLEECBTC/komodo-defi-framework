use super::client::ElectrumClient;
use super::constants::{
    BLOCKCHAIN_HEADERS_SUB_ID, BLOCKCHAIN_SCRIPTHASH_SUB_ID, CUTOFF_TIMEOUT, DEFAULT_CONNECTION_ESTABLISHMENT_TIMEOUT,
};

use crate::{RpcTransportEventHandler, SharableRpcTransportEventHandler};
use common::custom_futures::timeout::FutureTimerExt;
use common::executor::{
    abortable_queue::AbortableQueue, abortable_queue::WeakSpawner, AbortableSystem, SpawnFuture, Timer,
};
use common::jsonrpc_client::{
    JsonRpcBatchResponse, JsonRpcErrorType, JsonRpcId, JsonRpcRequest, JsonRpcResponse, JsonRpcResponseEnum,
};
use common::log::{error, info};
use common::{now_float, now_ms};
use mm2_rpc::data::legacy::ElectrumProtocol;
use timed_map::{MapKind, TimedMap};

use std::io;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use compatible_time::Instant;
use futures::channel::oneshot as async_oneshot;
use futures::compat::{Future01CompatExt, Stream01CompatExt};
use futures::future::FutureExt;
use futures::lock::Mutex as AsyncMutex;
use futures::select;
use futures::stream::StreamExt;
use futures01::sync::mpsc;
use futures01::{Sink, Stream};
use http::Uri;
use serde::Serialize;

cfg_native! {
    use super::tcp_stream::*;

    use std::convert::TryFrom;
    use std::net::ToSocketAddrs;
    use futures::future::{Either, TryFutureExt};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, WriteHalf, ReadHalf};
    use tokio::net::TcpStream;
    use tokio_rustls::{TlsConnector};
    use rustls::{ServerName};
}

cfg_wasm32! {
    use mm2_net::wasm::wasm_ws::{ws_transport,WsOutgoingSender,WsIncomingReceiver};

    use std::sync::atomic::AtomicUsize;
}

pub type JsonRpcPendingRequests = TimedMap<JsonRpcId, async_oneshot::Sender<JsonRpcResponseEnum>>;

macro_rules! disconnect_and_return {
    ($typ:tt, $err:expr, $conn:expr, $handlers:expr) => {{
        let err = ElectrumConnectionErr::$typ(format!("{:?}", $err));
        disconnect_and_return!(err, $conn, $handlers);
    }};
    ($err:expr, $conn:expr, $handlers:expr) => {{
        // Inform the event handlers of the disconnection.
        $handlers.on_disconnected(&$conn.address()).ok();
        // Disconnect the connection.
        $conn.disconnect(Some($err.clone()));
        return Err($err);
    }};
}

macro_rules! disconnect_and_return_if_err {
    ($ex:expr, $typ:tt, $conn:expr, $handlers:expr) => {{
        match $ex {
            Ok(res) => res,
            Err(e) => {
                disconnect_and_return!($typ, e, $conn, $handlers);
            },
        }
    }};
    ($ex:expr, $conn:expr, $handlers:expr) => {{
        match $ex {
            Ok(res) => res,
            Err(e) => {
                disconnect_and_return!(e, $conn, $handlers);
            },
        }
    }};
}

macro_rules! wrap_timeout {
    ($call:expr, $timeout:expr, $conn:expr, $handlers:expr) => {{
        let now = Instant::now();
        let res = match $call.timeout_secs($timeout).await {
            Ok(res) => res,
            Err(_) => {
                disconnect_and_return!(
                    ElectrumConnectionErr::Timeout(stringify!($call), $timeout),
                    $conn,
                    $handlers
                );
            },
        };
        // Remaining timeout after executing `$call`.
        let timeout = ($timeout - now.elapsed().as_secs_f64()).max(0.0);
        (timeout, res)
    }};
}

/// Helper function casting mpsc::Receiver as Stream.
fn rx_to_stream(rx: mpsc::Receiver<Vec<u8>>) -> impl Stream<Item = Vec<u8>, Error = io::Error> {
    rx.map_err(|_| panic!("errors not possible on rx"))
}

#[cfg(not(target_arch = "wasm32"))]
/// Helper function to parse a a string DNS name into a ServerName.
fn server_name_from_domain(dns_name: &str) -> Result<ServerName, String> {
    match ServerName::try_from(dns_name) {
        // The `ServerName` must be `DnsName` variant, SSL works with domain names and not IPs.
        Ok(dns_name) if matches!(dns_name, ServerName::DnsName(_)) => Ok(dns_name),
        _ => ERR!("Couldn't parse DNS name from '{}'", dns_name),
    }
}

/// Electrum request RPC representation
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct ElectrumConnectionSettings {
    pub url: String,
    #[serde(default)]
    pub protocol: ElectrumProtocol,
    #[serde(default)]
    pub disable_cert_verification: bool,
    pub timeout_sec: Option<f64>,
}

/// Possible connection errors when connection to an Electrum server.
#[derive(Clone, Debug)]
pub enum ElectrumConnectionErr {
    /// Couldn't connect to the server within the provided timeout.
    /// The first argument is the call (stringified) that timed out.
    /// The second argument is the time limit it had to finish within, in seconds.
    Timeout(&'static str, f64),
    /// A temporary error that might be resolved later on.
    Temporary(String),
    /// An error that can't be resolved by retrying.
    Irrecoverable(String),
    /// The server's version doesn't match the client's version.
    VersionMismatch(String),
}

impl ElectrumConnectionErr {
    pub fn is_recoverable(&self) -> bool {
        match self {
            ElectrumConnectionErr::Irrecoverable(_) | ElectrumConnectionErr::VersionMismatch(_) => false,
            ElectrumConnectionErr::Timeout(_, _) | ElectrumConnectionErr::Temporary(_) => true,
        }
    }
}

/// Represents the active Electrum connection to selected address
#[derive(Debug)]
pub struct ElectrumConnection {
    /// The client connected to this SocketAddr
    settings: ElectrumConnectionSettings,
    /// The Sender forwarding requests to writing part of underlying stream
    tx: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    /// A lock to prevent multiple connection establishments happening concurrently.
    establishing_connection: AsyncMutex<()>,
    /// Responses are stored here
    responses: Mutex<JsonRpcPendingRequests>,
    /// Selected protocol version. The value is initialized after the server.version RPC call.
    protocol_version: Mutex<Option<f32>>,
    /// Why was the connection disconnected the last time?
    last_error: Mutex<Option<ElectrumConnectionErr>>,
    /// An abortable system for connection specific tasks to run on.
    abortable_system: AbortableQueue,
}

impl ElectrumConnection {
    pub fn new(settings: ElectrumConnectionSettings, abortable_system: AbortableQueue) -> Self {
        ElectrumConnection {
            settings,
            tx: Mutex::new(None),
            establishing_connection: AsyncMutex::new(()),
            responses: Mutex::new(JsonRpcPendingRequests::new_with_map_kind(MapKind::BTreeMap).expiration_tick_cap(50)),
            protocol_version: Mutex::new(None),
            last_error: Mutex::new(None),
            abortable_system,
        }
    }

    pub fn address(&self) -> &str {
        &self.settings.url
    }

    fn weak_spawner(&self) -> WeakSpawner {
        self.abortable_system.weak_spawner()
    }

    fn is_connected(&self) -> bool {
        self.tx.lock().unwrap().is_some()
    }

    fn set_protocol_version(&self, version: f32) {
        let mut protocol_version = self.protocol_version.lock().unwrap();
        if protocol_version.is_none() {
            *protocol_version = Some(version);
        }
    }

    fn clear_protocol_version(&self) {
        self.protocol_version.lock().unwrap().take();
    }

    fn set_last_error(&self, reason: ElectrumConnectionErr) {
        let mut last_error = self.last_error.lock().unwrap();
        if last_error.is_none() {
            *last_error = Some(reason);
        }
    }

    fn clear_last_error(&self) {
        self.last_error.lock().unwrap().take();
    }

    fn last_error(&self) -> Option<ElectrumConnectionErr> {
        self.last_error.lock().unwrap().clone()
    }

    /// Connects to the electrum server by setting the `tx` sender channel.
    ///
    /// # Safety:
    /// For this to be atomic, the caller must have acquired the lock to `establishing_connection`.
    fn connect(&self, tx: mpsc::Sender<Vec<u8>>) {
        self.tx.lock().unwrap().replace(tx);
        self.clear_last_error();
    }

    /// Disconnect and clear the connection state.
    pub fn disconnect(&self, reason: Option<ElectrumConnectionErr>) {
        self.tx.lock().unwrap().take();
        self.responses.lock().unwrap().clear();
        self.clear_protocol_version();
        if let Some(reason) = reason {
            self.set_last_error(reason);
        }
        self.abortable_system.abort_all_and_reset().ok();
    }

    /// Sends a request to the electrum server and waits for the response.
    ///
    /// ## Important: This should always return [`JsonRpcErrorType::Transport`] error.
    pub async fn electrum_request(
        &self,
        req_json: String,
        rpc_id: JsonRpcId,
        timeout: f64,
    ) -> Result<JsonRpcResponseEnum, JsonRpcErrorType> {
        #[cfg(not(target_arch = "wasm32"))]
        let req_json = {
            // Electrum request and responses must end with \n
            // https://electrumx.readthedocs.io/en/latest/protocol-basics.html#message-stream
            let mut req_json = req_json;
            req_json.push('\n');
            req_json
        };

        // Create a oneshot channel to receive the response in.
        let (req_tx, res_rx) = async_oneshot::channel();
        self.responses
            .lock()
            .unwrap()
            .insert_expirable(rpc_id, req_tx, Duration::from_secs_f64(timeout));
        let tx = self
            .tx
            .lock()
            .unwrap()
            // Clone to not to hold the lock while sending the request.
            .clone()
            .ok_or_else(|| JsonRpcErrorType::Transport("Connection is not established".to_string()))?;

        // Send the request to the electrum server.
        tx.send(req_json.into_bytes())
            .compat()
            .await
            .map_err(|e| JsonRpcErrorType::Transport(e.to_string()))?;

        // Wait for the response to be processed and sent back to us.
        res_rx
            .timeout_secs(timeout)
            .await
            .map_err(|e| JsonRpcErrorType::Transport(e.to_string()))?
            .map_err(|_e| JsonRpcErrorType::Transport("The sender didn't send".to_string()))
    }

    /// Process an incoming JSONRPC response from the electrum server.
    fn process_electrum_response(&self, bytes: &[u8], client: &ElectrumClient) {
        // Inform the event handlers.
        client.event_handlers().on_incoming_response(bytes);

        // detect if we got standard JSONRPC response or subscription response as JSONRPC request
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ElectrumRpcResponseEnum {
            /// The subscription response as JSONRPC request.
            ///
            /// NOTE Because JsonRpcResponse uses default values for each of its field,
            /// this variant has to stay at top in this enumeration to be properly deserialized
            /// from serde.
            SubscriptionNotification(JsonRpcRequest),
            /// The standard JSONRPC single response.
            SingleResponse(JsonRpcResponse),
            /// The batch of standard JSONRPC responses.
            BatchResponses(JsonRpcBatchResponse),
        }

        let response: ElectrumRpcResponseEnum = match serde_json::from_slice(bytes) {
            Ok(res) => res,
            Err(e) => {
                error!("{}", e);
                return;
            },
        };

        let response = match response {
            ElectrumRpcResponseEnum::SingleResponse(single) => JsonRpcResponseEnum::Single(single),
            ElectrumRpcResponseEnum::BatchResponses(batch) => JsonRpcResponseEnum::Batch(batch),
            ElectrumRpcResponseEnum::SubscriptionNotification(req) => {
                match req.method.as_str() {
                    BLOCKCHAIN_SCRIPTHASH_SUB_ID => {
                        if let Some(scripthash) = req.params.first().and_then(|s| s.as_str()) {
                            client.notify_triggered_hash(scripthash.to_string()).ok();
                        } else {
                            error!("Notification must contain the script hash value, got: {req:?}");
                        }
                    },
                    BLOCKCHAIN_HEADERS_SUB_ID => {},
                    _ => {
                        error!("Unexpected notification method: {}", req.method);
                    },
                }
                return;
            },
        };

        // the corresponding sender may not exist, receiver may be dropped
        // these situations are not considered as errors so we just silently skip them
        let pending = self.responses.lock().unwrap().remove(&response.rpc_id());
        if let Some(tx) = pending {
            tx.send(response).ok();
        }
    }

    /// Process a bulk response from the electrum server.
    ///
    /// A bulk response is a response that contains multiple JSONRPC responses.
    fn process_electrum_bulk_response(&self, bulk_response: &[u8], client: &ElectrumClient) {
        // We should split the received response because we can get several responses in bulk.
        let responses = bulk_response.split(|item| *item == b'\n');

        for response in responses {
            // `split` returns empty slice if it ends with separator which is our case.
            if !response.is_empty() {
                self.process_electrum_response(response, client)
            }
        }
    }
}

// Connection loop establishment methods.
impl ElectrumConnection {
    /// Tries to establish a connection to the server.
    ///
    /// Returns the tokio stream with the server and the remaining timeout
    /// left from the input timeout.
    #[cfg(not(target_arch = "wasm32"))]
    async fn establish_connection(connection: &ElectrumConnection) -> Result<ElectrumStream, ElectrumConnectionErr> {
        let address = connection.address();

        let socket_addr = match address.to_socket_addrs() {
            Err(e) if matches!(e.kind(), std::io::ErrorKind::InvalidInput) => {
                return Err(ElectrumConnectionErr::Irrecoverable(format!(
                    "Invalid address format: {e:?}"
                )));
            },
            Err(e) => {
                return Err(ElectrumConnectionErr::Temporary(format!(
                    "Resolve error in address: {e:?}"
                )));
            },
            Ok(mut addr) => match addr.next() {
                None => {
                    return Err(ElectrumConnectionErr::Temporary("Address resolved to None".to_string()));
                },
                Some(addr) => addr,
            },
        };

        let connect_f = match connection.settings.protocol {
            ElectrumProtocol::TCP => Either::Left(TcpStream::connect(&socket_addr).map_ok(ElectrumStream::Tcp)),
            ElectrumProtocol::SSL => {
                let uri: Uri = match address.parse() {
                    Ok(uri) => uri,
                    Err(e) => {
                        return Err(ElectrumConnectionErr::Irrecoverable(format!("URL parse error: {e:?}")));
                    },
                };

                let Some(dns_name) = uri.host().map(String::from) else {
                    return Err(ElectrumConnectionErr::Irrecoverable(
                        "Couldn't retrieve host from address".to_string(),
                    ));
                };

                let Ok(dns) = server_name_from_domain(dns_name.as_str()) else {
                    return Err(ElectrumConnectionErr::Irrecoverable(
                        "Address isn't a valid domain name".to_string(),
                    ));
                };

                let tls_connector = if connection.settings.disable_cert_verification {
                    TlsConnector::from(UNSAFE_TLS_CONFIG.clone())
                } else {
                    TlsConnector::from(SAFE_TLS_CONFIG.clone())
                };

                Either::Right(
                    TcpStream::connect(&socket_addr)
                        .and_then(move |stream| tls_connector.connect(dns, stream).map_ok(ElectrumStream::Tls)),
                )
            },
            ElectrumProtocol::WS | ElectrumProtocol::WSS => {
                return Err(ElectrumConnectionErr::Irrecoverable(
                    "Incorrect protocol for native connection ('WS'/'WSS'). Use 'TCP' or 'SSL' instead.".to_string(),
                ));
            },
        };

        // Try to connect to the server.
        let stream = match connect_f.await {
            Ok(stream) => stream,
            Err(e) => {
                return Err(ElectrumConnectionErr::Temporary(format!(
                    "Couldn't connect to the electrum server: {e:?}"
                )))
            },
        };
        if let Err(e) = stream.as_ref().set_nodelay(true) {
            return Err(ElectrumConnectionErr::Temporary(format!(
                "Setting TCP_NODELAY failed: {e:?}"
            )));
        };

        Ok(stream)
    }

    #[cfg(target_arch = "wasm32")]
    async fn establish_connection(
        connection: &ElectrumConnection,
    ) -> Result<(WsIncomingReceiver, WsOutgoingSender), ElectrumConnectionErr> {
        lazy_static! {
            static ref CONN_IDX: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
        }

        let address = connection.address();
        let uri: Uri = match address.parse() {
            Ok(uri) => uri,
            Err(e) => {
                return Err(ElectrumConnectionErr::Irrecoverable(format!(
                    "Failed to parse the address: {e:?}"
                )));
            },
        };
        if uri.scheme().is_some() {
            return Err(ElectrumConnectionErr::Irrecoverable(
                "There has not to be a scheme in the url. 'ws://' scheme is used by default.  Consider using 'protocol: \"WSS\"' in the electrum request to switch to the 'wss://' scheme.".to_string(),
            )
            );
        }

        let protocol_prefixed_address = match connection.settings.protocol {
            ElectrumProtocol::WS => {
                format!("ws://{address}")
            },
            ElectrumProtocol::WSS => {
                format!("wss://{address}")
            },
            ElectrumProtocol::TCP | ElectrumProtocol::SSL => {
                return Err(ElectrumConnectionErr::Irrecoverable(
                    "'TCP' and 'SSL' are not supported in a browser. Please use 'WS' or 'WSS' protocols".to_string(),
                ));
            },
        };

        let spawner = connection.weak_spawner();
        let connect_f = ws_transport(
            CONN_IDX.fetch_add(1, AtomicOrdering::Relaxed),
            &protocol_prefixed_address,
            &spawner,
        );

        // Try to connect to the server.
        let (transport_tx, transport_rx) = match connect_f.await {
            Ok(stream) => stream,
            Err(e) => {
                return Err(ElectrumConnectionErr::Temporary(format!(
                    "Couldn't connect to the electrum server: {e:?}"
                )))
            },
        };

        Ok((transport_rx, transport_tx))
    }

    /// Waits until `last_response` time is too old in the past then returns a temporary error.
    async fn timeout_loop(last_response: Arc<AtomicU64>) -> ElectrumConnectionErr {
        loop {
            Timer::sleep(CUTOFF_TIMEOUT).await;
            let last_sec = (last_response.load(AtomicOrdering::Relaxed) / 1000) as f64;
            if now_float() - last_sec > CUTOFF_TIMEOUT {
                break ElectrumConnectionErr::Temporary(format!(
                    "Server didn't respond for too long ({}s).",
                    now_float() - last_sec
                ));
            }
        }
    }

    /// Runs the send loop that sends outgoing requests to the server.
    ///
    /// This runs until the sender is disconnected.
    async fn send_loop(
        address: String,
        event_handlers: Arc<Vec<Box<SharableRpcTransportEventHandler>>>,
        #[cfg(not(target_arch = "wasm32"))] mut write: WriteHalf<ElectrumStream>,
        #[cfg(target_arch = "wasm32")] mut write: WsOutgoingSender,
        rx: mpsc::Receiver<Vec<u8>>,
    ) -> ElectrumConnectionErr {
        let mut rx = rx_to_stream(rx).compat();
        while let Some(Ok(bytes)) = rx.next().await {
            // NOTE: We shouldn't really notify on going request yet since we don't know
            // if sending will error. We do that early though to avoid cloning the bytes on wasm.
            event_handlers.on_outgoing_request(&bytes);

            #[cfg(not(target_arch = "wasm32"))]
            let send_result = write.write_all(&bytes).await;
            #[cfg(target_arch = "wasm32")]
            let send_result = write.send(bytes).await;

            if let Err(e) = send_result {
                error!("Write error {e} to {address}");
            }
        }
        ElectrumConnectionErr::Temporary("Sender disconnected".to_string())
    }

    /// Runs the receive loop that reads incoming responses from the server.
    ///
    /// This runs until the electrum server sends an empty response (signaling disconnection),
    /// or if we encounter an error while reading from the stream.
    #[cfg(not(target_arch = "wasm32"))]
    async fn recv_loop(
        connection: Arc<ElectrumConnection>,
        client: ElectrumClient,
        read: ReadHalf<ElectrumStream>,
        last_response: Arc<AtomicU64>,
    ) -> ElectrumConnectionErr {
        let mut buffer = String::with_capacity(1024);
        let mut buf_reader = BufReader::new(read);
        loop {
            match buf_reader.read_line(&mut buffer).await {
                Ok(c) => {
                    if c == 0 {
                        break ElectrumConnectionErr::Temporary("EOF".to_string());
                    }
                },
                Err(e) => {
                    break ElectrumConnectionErr::Temporary(format!("Error on read {e:?}"));
                },
            };

            last_response.store(now_ms(), AtomicOrdering::Relaxed);
            connection.process_electrum_bulk_response(buffer.as_bytes(), &client);
            buffer.clear();
        }
    }

    #[cfg(target_arch = "wasm32")]
    async fn recv_loop(
        connection: Arc<ElectrumConnection>,
        client: ElectrumClient,
        mut read: WsIncomingReceiver,
        last_response: Arc<AtomicU64>,
    ) -> ElectrumConnectionErr {
        let address = connection.address();
        while let Some(response) = read.next().await {
            match response {
                Ok(bytes) => {
                    last_response.store(now_ms(), AtomicOrdering::Relaxed);
                    connection.process_electrum_response(&bytes, &client);
                },
                Err(e) => {
                    error!("{address} error: {e:?}");
                },
            }
        }
        ElectrumConnectionErr::Temporary("Receiver disconnected".to_string())
    }

    /// Checks the server version against the range of accepted versions and disconnects the server
    /// if the version is not supported.
    async fn check_server_version(
        connection: &ElectrumConnection,
        client: &ElectrumClient,
    ) -> Result<(), ElectrumConnectionErr> {
        let address = connection.address();

        // Don't query for the version if the client doesn't care about it, as querying for the version might
        // fail with the protocol range we will provide.
        if !client.negotiate_version() {
            return Ok(());
        }

        match client.server_version(address, client.protocol_version()).compat().await {
            Ok(version_str) => match version_str.protocol_version.parse::<f32>() {
                Ok(version_f32) => {
                    connection.set_protocol_version(version_f32);
                    Ok(())
                },
                Err(e) => Err(ElectrumConnectionErr::Temporary(format!(
                    "Failed to parse electrum server version {e:?}"
                ))),
            },
            // If the version we provided isn't supported by the server, it returns a JSONRPC response error.
            Err(e) if matches!(e.error, JsonRpcErrorType::Response(..)) => {
                Err(ElectrumConnectionErr::VersionMismatch(format!("{e:?}")))
            },
            Err(e) => Err(ElectrumConnectionErr::Temporary(format!(
                "Failed to get electrum server version {e:?}"
            ))),
        }
    }

    /// Starts the connection loop that keeps an active connection to the electrum server.
    /// If this connection is already connected, nothing is performed and `Ok(())` is returned.
    ///
    /// This will first try to connect to the server and use that connection to query its version.
    /// If version checks succeed, the connection will be kept alive, otherwise, it will be terminated.
    pub async fn establish_connection_loop(
        self: &Arc<ElectrumConnection>,
        client: ElectrumClient,
    ) -> Result<(), ElectrumConnectionErr> {
        let connection = self.clone();
        let address = connection.address().to_string();
        let event_handlers = client.event_handlers();
        // This is the timeout for connection establishment and version querying (i.e. the whole method).
        // The caller is guaranteed that the method will return within this time.
        let timeout = connection
            .settings
            .timeout_sec
            .unwrap_or(DEFAULT_CONNECTION_ESTABLISHMENT_TIMEOUT);

        // Locking `establishing_connection` will prevent other threads from establishing a connection concurrently.
        let (timeout, _establishing_connection) = wrap_timeout!(
            connection.establishing_connection.lock(),
            timeout,
            connection,
            event_handlers
        );

        // Check if we are already connected.
        if connection.is_connected() {
            return Ok(());
        }

        // Check why we errored the last time, don't try to reconnect if it was an irrecoverable error.
        if let Some(last_error) = connection.last_error() {
            if !last_error.is_recoverable() {
                return Err(last_error);
            }
        }

        let (timeout, stream_res) = wrap_timeout!(
            Self::establish_connection(&connection).boxed(),
            timeout,
            connection,
            event_handlers
        );
        let stream = disconnect_and_return_if_err!(stream_res, connection, event_handlers);

        let (connection_ready_signal, wait_for_connection_ready) = async_oneshot::channel();
        let connection_loop = {
            // Branch 1: Disconnect after not receiving responses for too long.
            let last_response = Arc::new(AtomicU64::new(now_ms()));
            let timeout_branch = Self::timeout_loop(last_response.clone()).boxed();

            // Branch 2: Read incoming responses from the server.
            #[cfg(not(target_arch = "wasm32"))]
            let (read, write) = tokio::io::split(stream);
            #[cfg(target_arch = "wasm32")]
            let (read, write) = stream;
            let recv_branch = Self::recv_loop(connection.clone(), client.clone(), read, last_response).boxed();

            // Branch 3: Send outgoing requests to the server.
            let (tx, rx) = mpsc::channel(0);
            let send_branch = Self::send_loop(address.clone(), event_handlers.clone(), write, rx).boxed();

            let connection = connection.clone();
            let event_handlers = event_handlers.clone();
            async move {
                connection.connect(tx);
                // Signal that the connection is up and ready so to start the version querying.
                connection_ready_signal.send(()).ok();
                event_handlers.on_connected(&address).ok();
                let via = match connection.settings.protocol {
                    ElectrumProtocol::TCP => "via TCP",
                    ElectrumProtocol::SSL if connection.settings.disable_cert_verification => {
                        "via SSL *with disabled certificate verification*"
                    },
                    ElectrumProtocol::SSL => "via SSL",
                    ElectrumProtocol::WS => "via WS",
                    ElectrumProtocol::WSS => "via WSS",
                };
                info!("{address} is now connected {via}.");

                let err = select! {
                    e = timeout_branch.fuse() => e,
                    e = recv_branch.fuse() => e,
                    e = send_branch.fuse() => e,
                };

                error!("{address} connection dropped due to: {err:?}");
                event_handlers.on_disconnected(&address).ok();
                connection.disconnect(Some(err));
            }
        };
        // Start the connection loop on a weak spawner.
        connection.weak_spawner().spawn(connection_loop);

        // Wait for the connection to be ready before querying the version.
        let (timeout, connection_ready_res) =
            wrap_timeout!(wait_for_connection_ready, timeout, connection, event_handlers);
        disconnect_and_return_if_err!(connection_ready_res, Temporary, connection, event_handlers);

        let (_, version_res) = wrap_timeout!(
            Self::check_server_version(&connection, &client).boxed(),
            timeout,
            connection,
            event_handlers
        );
        disconnect_and_return_if_err!(version_res, connection, event_handlers);

        Ok(())
    }
}
