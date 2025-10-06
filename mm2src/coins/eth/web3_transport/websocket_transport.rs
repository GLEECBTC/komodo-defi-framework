//! This module offers a transport layer for managing request-response style communication with Ethereum
//! nodes using websockets that can work concurrently.
//!
//! In comparison to HTTP transport, this approach proves to be much quicker (low-latency) and consumes
//! less bandwidth. This efficiency is achieved by avoiding the handling of TCP handshakes (connection reusability)
//! for each request.

use super::http_transport::de_rpc_response;
use crate::eth::web3_transport::Web3SendOut;
use crate::eth::WEB3_REQUEST_TIMEOUT_S;
use crate::eth::{EthCoin, RpcTransportEventHandlerShared};
use crate::{MmCoin, RpcTransportEventHandler};
use common::executor::{AbortSettings, SpawnAbortable, Timer};
use common::log;
use compatible_time::{Duration, Instant};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use futures::channel::oneshot;
use futures::lock::Mutex as AsyncMutex;
use futures_ticker::Ticker;
use futures_util::{FutureExt, SinkExt, StreamExt};
use jsonrpc_core::Call;
use mm2_p2p::Keypair;
use proxy_signature::{ProxySign, RawMessage};
use std::sync::atomic::AtomicBool;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use timed_map::TimedMap;
use tokio_tungstenite_wasm::WebSocketStream;
use web3::error::{Error, TransportError};
use web3::helpers::to_string;
use web3::{helpers::build_request, RequestId, Transport};

const MAX_ATTEMPTS: u32 = 3;
const SLEEP_DURATION: f64 = 1.;
const KEEPALIVE_DURATION: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub(crate) struct WebsocketTransportNode {
    pub(crate) uri: http::Uri,
}

#[derive(Clone, Debug)]
pub struct WebsocketTransport {
    request_id: Arc<AtomicUsize>,
    pub(crate) last_request_failed: Arc<AtomicBool>,
    node: WebsocketTransportNode,
    event_handlers: Vec<RpcTransportEventHandlerShared>,
    pub(crate) proxy_sign_keypair: Option<Keypair>,
    controller_channel: Arc<ControllerChannel>,
    connection_guard: Arc<AsyncMutex<()>>,
}

#[derive(Debug)]
struct ControllerChannel {
    tx: UnboundedSender<ControllerMessage>,
    rx: AsyncMutex<UnboundedReceiver<ControllerMessage>>,
}

enum ControllerMessage {
    Request(WsRequest),
    Close,
}

#[derive(Debug)]
struct WsRequest {
    serialized_request: String,
    request_id: RequestId,
    response_notifier: oneshot::Sender<Vec<u8>>,
}

enum OuterAction {
    None,
    Continue,
    Break,
    Return,
}

impl WebsocketTransport {
    pub(crate) fn with_event_handlers(
        node: WebsocketTransportNode,
        event_handlers: Vec<RpcTransportEventHandlerShared>,
    ) -> Self {
        let (req_tx, req_rx) = futures::channel::mpsc::unbounded();

        WebsocketTransport {
            node,
            event_handlers,
            request_id: Arc::new(AtomicUsize::new(1)),
            controller_channel: Arc::new(ControllerChannel {
                tx: req_tx,
                rx: AsyncMutex::new(req_rx),
            }),
            connection_guard: Arc::new(AsyncMutex::new(())),
            proxy_sign_keypair: None,
            last_request_failed: Arc::new(AtomicBool::new(false)),
        }
    }

    async fn handle_keepalive(&self, wsocket: &mut WebSocketStream, expires_at: Option<Instant>) -> OuterAction {
        const SIMPLE_REQUEST: &str = r#"{"jsonrpc":"2.0","method":"net_version","params":[],"id": 0 }"#;

        if let Some(expires_at) = expires_at {
            if Instant::now() >= expires_at {
                log::debug!("Dropping temporary connection for {:?}", self.node.uri.to_string());
                return OuterAction::Break;
            }
        }

        let mut should_continue = false;
        for _ in 0..MAX_ATTEMPTS {
            match wsocket
                .send(tokio_tungstenite_wasm::Message::Text(SIMPLE_REQUEST.to_string()))
                .await
            {
                Ok(_) => {
                    should_continue = false;
                    break;
                },
                Err(e) => {
                    log::error!("{e}");
                    should_continue = true;
                },
            };

            Timer::sleep(SLEEP_DURATION).await;
        }

        if should_continue {
            return OuterAction::Continue;
        }

        OuterAction::None
    }

    async fn handle_send_request(
        &self,
        request: Option<ControllerMessage>,
        wsocket: &mut WebSocketStream,
        response_notifiers: &mut TimedMap<usize, oneshot::Sender<Vec<u8>>>,
    ) -> OuterAction {
        match request {
            Some(ControllerMessage::Request(WsRequest {
                request_id,
                serialized_request,
                response_notifier,
            })) => {
                response_notifiers.insert_expirable(
                    request_id,
                    response_notifier,
                    // Since request will be cancelled when timeout occurs, we are free to drop its state.
                    WEB3_REQUEST_TIMEOUT_S,
                );

                let mut should_continue = Default::default();
                for _ in 0..MAX_ATTEMPTS {
                    match wsocket
                        .send(tokio_tungstenite_wasm::Message::Text(serialized_request.clone()))
                        .await
                    {
                        Ok(_) => {
                            should_continue = false;
                            break;
                        },
                        Err(e) => {
                            log::error!("{e}");
                            should_continue = true;
                        },
                    }

                    Timer::sleep(SLEEP_DURATION).await;
                }

                if should_continue {
                    let _ = response_notifiers.remove(&request_id);
                    return OuterAction::Continue;
                }
            },
            Some(ControllerMessage::Close) => {
                return OuterAction::Break;
            },
            _ => {},
        }

        OuterAction::None
    }

    async fn handle_response(
        &self,
        message: Option<Result<tokio_tungstenite_wasm::Message, tokio_tungstenite_wasm::Error>>,
        response_notifiers: &mut TimedMap<usize, oneshot::Sender<Vec<u8>>>,
    ) -> OuterAction {
        match message {
            Some(Ok(tokio_tungstenite_wasm::Message::Text(inc_event))) => {
                if let Ok(inc_event) = serde_json::from_str::<serde_json::Value>(&inc_event) {
                    if !inc_event.is_object() {
                        return OuterAction::Continue;
                    }

                    if let Some(id) = inc_event.get("id") {
                        let request_id = id.as_u64().unwrap_or_default() as usize;

                        if let Some(notifier) = response_notifiers.remove(&request_id) {
                            let mut res_bytes: Vec<u8> = Vec::new();
                            if serde_json::to_writer(&mut res_bytes, &inc_event).is_ok() {
                                notifier.send(res_bytes).expect("receiver channel must be alive");
                            }
                        }
                    }
                }
            },
            Some(Ok(tokio_tungstenite_wasm::Message::Binary(_))) => return OuterAction::Continue,
            Some(Ok(tokio_tungstenite_wasm::Message::Close(_))) => return OuterAction::Break,
            Some(Err(e)) => {
                log::error!("{e}");
                return OuterAction::Return;
            },
            None => return OuterAction::Continue,
        };

        OuterAction::None
    }

    async fn attempt_to_establish_socket_connection(
        &self,
        max_attempts: u32,
        mut sleep_duration_on_failure: f64,
    ) -> tokio_tungstenite_wasm::Result<WebSocketStream> {
        const MAX_SLEEP_DURATION: f64 = 32.0;
        let mut attempts = 0;

        loop {
            match tokio_tungstenite_wasm::connect(self.node.uri.to_string()).await {
                Ok(ws) => return Ok(ws),
                Err(e) => {
                    attempts += 1;
                    if attempts > max_attempts {
                        return Err(e);
                    }

                    Timer::sleep(sleep_duration_on_failure).await;
                    sleep_duration_on_failure = (sleep_duration_on_failure * 2.0).min(MAX_SLEEP_DURATION);
                },
            };
        }
    }

    pub(crate) async fn start_connection_loop(self, expires_at: Option<Instant>) {
        let _guard = self.connection_guard.lock().await;

        // List of awaiting requests
        let mut response_notifiers: TimedMap<RequestId, oneshot::Sender<Vec<u8>>> =
            TimedMap::new_with_map_kind(timed_map::MapKind::FxHashMap).expiration_tick_cap(30);

        let mut wsocket = match self
            .attempt_to_establish_socket_connection(MAX_ATTEMPTS, SLEEP_DURATION)
            .await
        {
            Ok(ws) => ws,
            Err(e) => {
                log::error!("Connection could not established for {}. Error {e}", self.node.uri);
                return;
            },
        };

        let mut keepalive_interval = Ticker::new(KEEPALIVE_DURATION);
        let mut req_rx = self.controller_channel.rx.lock().await;

        loop {
            futures_util::select! {
                _ = keepalive_interval.next().fuse() => {
                    match self.handle_keepalive(&mut wsocket, expires_at).await {
                        OuterAction::None => {},
                        OuterAction::Continue => continue,
                        OuterAction::Break => break,
                        OuterAction::Return => return,
                    }
                }

                request = req_rx.next().fuse() => {
                    match self.handle_send_request(request, &mut wsocket, &mut response_notifiers).await {
                        OuterAction::None => {},
                        OuterAction::Continue => continue,
                        OuterAction::Break => break,
                        OuterAction::Return => return,
                    }
                }

                message = wsocket.next().fuse() => {
                    match self.handle_response(message, &mut response_notifiers).await {
                        OuterAction::None => {},
                        OuterAction::Continue => continue,
                        OuterAction::Break => break,
                        OuterAction::Return => return,
                    }
                }
            }
        }
    }

    pub(crate) async fn stop_connection_loop(&self) {
        let mut tx = self.controller_channel.tx.clone();
        tx.send(ControllerMessage::Close)
            .await
            .expect("receiver channel must be alive");
    }

    pub(crate) fn maybe_spawn_connection_loop(&self, coin: EthCoin) {
        self.maybe_spawn_connection_loop_inner(coin, None)
    }

    pub(crate) fn maybe_spawn_temporary_connection_loop(&self, coin: EthCoin, expires_at: Instant) {
        self.maybe_spawn_connection_loop_inner(coin, Some(expires_at))
    }

    fn maybe_spawn_connection_loop_inner(&self, coin: EthCoin, expires_at: Option<Instant>) {
        // if we can acquire the lock here, it means connection loop is not alive
        if self.connection_guard.try_lock().is_some() {
            let fut = self.clone().start_connection_loop(expires_at);
            let settings = AbortSettings::info_on_abort(format!("connection loop stopped for {:?}", self.node.uri));
            coin.spawner().spawn_with_settings(fut, settings);
        }
    }
}

async fn send_request(
    transport: WebsocketTransport,
    request: Call,
    request_id: RequestId,
    event_handlers: Vec<RpcTransportEventHandlerShared>,
) -> Result<serde_json::Value, Error> {
    /// komodo-defi-proxy expects proxy signatures in the socket messages as they
    /// cannot be provided through headers.
    #[derive(Serialize)]
    struct ProxyWrapper<'a> {
        #[serde(flatten)]
        pub request: &'a Call,
        pub proxy_sign: ProxySign,
    }

    let mut serialized_request = to_string(&request);
    let request_bytes = serialized_request.as_bytes().to_owned();

    if let Some(proxy_sign_keypair) = &transport.proxy_sign_keypair {
        let proxy_sign = RawMessage::sign(
            proxy_sign_keypair,
            &transport.node.uri,
            request_bytes.len(),
            common::PROXY_REQUEST_EXPIRATION_SEC,
        )
        .map_err(|e| Error::Transport(TransportError::Message(e.to_string())))?;

        let wrapper = ProxyWrapper {
            request: &request,
            proxy_sign,
        };

        serialized_request = serde_json::to_string(&wrapper)?;
    }

    let (notification_sender, notification_receiver) = oneshot::channel::<Vec<u8>>();

    event_handlers.on_outgoing_request(&request_bytes);

    let mut tx = transport.controller_channel.tx.clone();
    tx.send(ControllerMessage::Request(WsRequest {
        request_id,
        serialized_request,
        response_notifier: notification_sender,
    }))
    .await
    .map_err(|e| Error::Transport(TransportError::Message(e.to_string())))?;

    if let Ok(response) = notification_receiver.await {
        event_handlers.on_incoming_response(&response);
        return de_rpc_response(response, &transport.node.uri.to_string());
    };

    Err(Error::Transport(TransportError::Message(format!(
        "Sending {request:?} failed."
    ))))
}

impl Transport for WebsocketTransport {
    type Out = Web3SendOut;

    fn prepare(&self, method: &str, params: Vec<serde_json::Value>) -> (RequestId, Call) {
        let request_id = self.request_id.fetch_add(1, Ordering::SeqCst);
        let request = build_request(request_id, method, params);

        (request_id, request)
    }

    fn send(&self, id: RequestId, request: Call) -> Self::Out {
        Box::pin(send_request(self.clone(), request, id, self.event_handlers.clone()))
    }
}
