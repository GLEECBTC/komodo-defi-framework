use async_trait::async_trait;
use common::executor::SpawnFuture;
use common::log::{debug, error};
use common::stringify_js_error;
use futures::channel::mpsc::{self, SendError, TrySendError};
use futures::channel::oneshot;
use futures::{FutureExt, SinkExt, Stream, StreamExt};
use mm2_err_handle::prelude::*;
use mm2_state_machine::prelude::*;
use mm2_state_machine::state_machine::{ChangeStateExt, LastState, State, StateMachineTrait, StateResult};
use serde_json::{self as json, Value as Json};
use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use web_sys::{CloseEvent, DomException, MessageEvent, Url, WebSocket};

const NORMAL_CLOSURE_CODE: u16 = 1000;

pub type ConnIdx = usize;

pub type WsOutgoingReceiver = mpsc::Receiver<Vec<u8>>;
pub type WsIncomingSender = mpsc::Sender<(ConnIdx, WebSocketEvent)>;

type WsTransportReceiver = mpsc::Receiver<WsTransportEvent>;
type WsTransportSender = mpsc::Sender<WsTransportEvent>;

type IncomingShutdownTx = oneshot::Sender<()>;
type OutgoingShutdownTx = mpsc::Sender<()>;

type TransportClosure = Closure<dyn FnMut(JsValue)>;

pub type InitWsResult<T> = Result<T, MmError<InitWsError>>;

/// This is just an alias of the `Future<Output = ()> + Send + Unpin + 'static` trait.
/// Unfortunately, the trait type alias is an [unstable feature](https://github.com/rust-lang/rust/issues/41517).
trait ShutdownFut: Future<Output = ()> + Send + Unpin + 'static {}
impl<F: Future<Output = ()> + Send + Unpin + 'static> ShutdownFut for F {}

#[derive(Debug)]
pub enum InitWsError {
    InvalidUrl { url: String, reason: String },
    ConnectionFailed { reason: ClosureReason },
    Unknown(String),
}

impl InitWsError {
    fn from_ws_new_err(e: JsValue, url: &str) -> InitWsError {
        let reason = stringify_js_error(&e);

        // Check for TypeError
        if reason.contains("URL constructor") {
            return InitWsError::InvalidUrl {
                url: url.to_owned(),
                reason,
            };
        };

        match e.dyn_ref::<DomException>().map(DomException::name) {
            Some(ref name) if name == "SyntaxError" => InitWsError::InvalidUrl {
                url: url.to_owned(),
                reason,
            },
            _ => InitWsError::Unknown(reason),
        }
    }
}

/// The `WsEventReceiver` wrapper that filters and maps the incoming `WebSocketEvent` events into `Result<Vec<u8>, WebSocketError>`.
pub struct WsIncomingReceiver {
    inner: WsEventReceiver,
    closed: bool,
}

impl Stream for WsIncomingReceiver {
    type Item = Result<Vec<u8>, WebSocketError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.closed {
            error!("Attempted to poll WsIncomingReceiver after completion");
            return Poll::Ready(None);
        }
        let event = match Stream::poll_next(Pin::new(&mut self.inner), cx) {
            Poll::Ready(Some((_conn_idx, event))) => event,
            Poll::Ready(None) => return Poll::Ready(None),
            Poll::Pending => return Poll::Pending,
        };
        match event {
            WebSocketEvent::Establish => {
                error!("WsIncomingReceiver is expected to be established already");
                Poll::Pending
            },
            WebSocketEvent::Closed { .. } => {
                self.closed = true;
                Poll::Ready(None)
            },
            WebSocketEvent::Error(error) => Poll::Ready(Some(Err(error))),
            WebSocketEvent::Incoming(incoming) => Poll::Ready(Some(Ok(incoming))),
        }
    }
}

#[derive(Debug)]
pub struct WsEventReceiver {
    inner: mpsc::Receiver<(ConnIdx, WebSocketEvent)>,
    /// Is used to determine when the receiver is dropped.
    shutdown_tx: IncomingShutdownTx,
}

impl Unpin for WsEventReceiver {}

impl Stream for WsEventReceiver {
    type Item = (ConnIdx, WebSocketEvent);

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Stream::poll_next(Pin::new(&mut self.inner), cx)
    }
}

#[derive(Debug, Clone)]
pub struct WsOutgoingSender {
    inner: mpsc::Sender<Vec<u8>>,
    /// Is used to determine when all senders are dropped.
    #[allow(dead_code)]
    shutdown_tx: OutgoingShutdownTx,
}

/// Consider implementing the `Sink` trait.
/// Please note `WsOutgoingSender` must not provide a way to close the [`WsOutgoingSender::inner`] channel,
/// because the shutdown_tx wouldn't be closed properly.
impl WsOutgoingSender {
    pub async fn send(&mut self, msg: Vec<u8>) -> Result<(), SendError> {
        self.inner.send(msg).await
    }

    pub fn try_send(&mut self, msg: Vec<u8>) -> Result<(), TrySendError<Vec<u8>>> {
        self.inner.try_send(msg)
    }
}

#[derive(Debug)]
pub enum WebSocketEvent {
    /// A WebSocket connection is established.
    Establish,
    /// A WebSocket connection has been closed.
    Closed { reason: ClosureReason },
    /// An error has occurred.
    /// Please note some of the errors lead to the connection close.
    Error(WebSocketError),
    /// A message has been received through a WebSocket connection.
    Incoming(Vec<u8>),
}

#[derive(Debug)]
pub enum WebSocketError {
    OutgoingError { reason: OutgoingError, outgoing: Vec<u8> },
    InvalidIncoming { description: String },
}

#[derive(Debug)]
pub enum OutgoingError {
    IsNotConnected,
    SerializingError(String),
    UnderlyingError(String),
}

/// The [status codes](https://datatracker.ietf.org/doc/html/rfc6455#section-7.4.1) representation.
#[derive(Clone, Debug, PartialEq)]
pub enum ClosureReason {
    /// 1000 indicates a normal closure, meaning that the purpose for
    /// which the connection was established has been fulfilled.
    NormalClosure,
    /// 1001 indicates that an endpoint is "going away", such as a server
    /// going down or a browser having navigated away from a page.
    ///
    /// 1011 indicates that a server is terminating the connection because
    /// it encountered an unexpected condition that prevented it from
    /// fulfilling the request.
    GoingAway,
    /// 1006 is a reserved value and MUST NOT be set as a status code in a
    /// Close control frame by an endpoint.  It is designated for use in
    /// applications expecting a status code to indicate that the
    /// connection was closed abnormally, e.g., without sending or
    /// receiving a Close control frame.
    ClientReachedUnexpectedState,
    /// 1002, 1003, 1007, 1008, 1009, 1010
    ProtocolError,
    /// 1015 indicates that the connection was closed due to a failure to perform a TLS handshake
    /// (e.g., the server certificate can't be verified).
    TlsError,
    /// The client closed on a `WsTransportError` error.
    ClientClosedOnUnderlyingError,
    Other(u16),
}

impl ClosureReason {
    // https://datatracker.ietf.org/doc/html/rfc6455#section-7.4.1
    fn from_status_code(code: u16) -> ClosureReason {
        match code {
            1000 => ClosureReason::NormalClosure,
            1001 | 1011 => ClosureReason::GoingAway,
            1002 | 1003 | 1007 | 1008 | 1009 | 1010 => ClosureReason::ProtocolError,
            1015 => ClosureReason::TlsError,
            code => ClosureReason::Other(code),
        }
    }
}

fn spawn_ws_transport<Spawner: SpawnFuture>(
    idx: ConnIdx,
    url: &str,
    spawner: &Spawner,
) -> InitWsResult<(WsOutgoingSender, WsEventReceiver)> {
    let (ws, ws_transport_rx) = WebSocketImpl::init(url)?;
    let (incoming_tx, incoming_rx, incoming_shutdown) = incoming_channel(1024);

    let (outgoing_tx, outgoing_rx, outgoing_shutdown) = outgoing_channel(1024);

    let user_shutdown = into_one_shutdown(incoming_shutdown, outgoing_shutdown);

    let state_event_rx = StateEventListener::new(outgoing_rx, ws_transport_rx, user_shutdown);
    let mut state_machine = WsStateMachine {
        idx,
        ws,
        event_tx: incoming_tx,
        state_event_rx,
    };

    let fut = async move {
        state_machine
            .run(Box::new(ConnectingState))
            .await
            .expect("The error of this machine is Infallible");
    };
    spawner.spawn(fut);

    Ok((outgoing_tx, incoming_rx))
}

pub async fn ws_transport<Spawner: SpawnFuture>(
    idx: ConnIdx,
    url: &str,
    spawner: &Spawner,
) -> InitWsResult<(WsOutgoingSender, WsIncomingReceiver)> {
    let (sender, mut receiver) = spawn_ws_transport(idx, url, spawner)?;
    while let Some((_conn_idx, event)) = receiver.next().await {
        match event {
            WebSocketEvent::Establish => break,
            WebSocketEvent::Closed { reason } => {
                return MmError::err(InitWsError::ConnectionFailed { reason });
            },
            // if the error is an underlying error, the connection will close immediately
            WebSocketEvent::Error(e) => error!("{:?}", e),
            WebSocketEvent::Incoming(incoming) => error!(
                "Unexpected incoming while the connection is not established: {:?}",
                incoming
            ),
        }
    }
    // here we have the established connection
    let receiver = WsIncomingReceiver {
        inner: receiver,
        closed: false,
    };
    Ok((sender, receiver))
}

fn incoming_channel(capacity: usize) -> (WsIncomingSender, WsEventReceiver, impl ShutdownFut) {
    let (event_tx, event_rx) = mpsc::channel(capacity);
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    let incoming_rx = WsEventReceiver {
        inner: event_rx,
        shutdown_tx,
    };

    // convert the `oneshot::Receiver<()>` into `impl Future<Output=()>`
    let shutdown_rx = shutdown_rx.map(|_| ());
    (event_tx, incoming_rx, shutdown_rx)
}

fn outgoing_channel(capacity: usize) -> (WsOutgoingSender, WsOutgoingReceiver, impl ShutdownFut) {
    let (outgoing_tx, outgoing_rx) = mpsc::channel(capacity);
    let (shutdown_tx, shutdown_rx) = mpsc::channel(1);
    let outgoing_tx = WsOutgoingSender {
        inner: outgoing_tx,
        shutdown_tx,
    };

    // convert the `mpsc::Receiver<()>` into `impl Future<Output=()>`
    let shutdown_rx = shutdown_rx.collect::<Vec<_>>().map(|_| ());
    (outgoing_tx, outgoing_rx, shutdown_rx)
}

fn into_one_shutdown(left: impl ShutdownFut, right: impl ShutdownFut) -> impl ShutdownFut {
    use futures::future::{select, Either};

    async move {
        match select(left, right).await {
            Either::Left((_left_output, right_fut)) => right_fut.await,
            Either::Right((_right_output, left_fut)) => left_fut.await,
        }
    }
    .boxed()
}

/// The JS closures that have to be alive until the corresponding WebSocket exists.
struct WsClosures {
    _closures: Vec<TransportClosure>,
}

/// This is a wrapper over `WebSocket` that holds the closures passed as [`WebSocket::onopen`], [`WebSocket::onclose`] etc.
///
/// # Drop
///
/// Once and instance of `WebSocketImpl` is dropped, `WebSocket` will be closed via [`WebSocket::close_with_code`].
/// Please note that [`WebSocket::drop`] will be fired once the spawned state machine future is aborted.
struct WebSocketImpl {
    ws: WebSocket,
    /// It's not used directly, but we need to hold the closures in memory till `ws` exists.
    #[allow(dead_code)]
    closures: WsClosures,
}

impl WebSocketImpl {
    fn init(url: &str) -> InitWsResult<(WebSocketImpl, WsTransportReceiver)> {
        Self::validate_websocket_url(url)?;
        let ws = WebSocket::new(url).map_to_mm(|e| InitWsError::from_ws_new_err(e, url))?;

        let (tx, rx) = mpsc::channel(1024);

        let onopen_closure = construct_ws_event_closure(|_: JsValue| WsTransportEvent::Establish, tx.clone());
        let onclose_closure = construct_ws_event_closure::<CloseEvent, _>(WsTransportEvent::from, tx.clone());
        let onerror_closure = construct_ws_event_closure(
            |_: JsValue| WsTransportEvent::Error(WsTransportError::UnderlyingError),
            tx.clone(),
        );
        let onmessage_closure = construct_ws_event_closure(
            |message: MessageEvent| match decode_incoming(message) {
                Ok(response) => WsTransportEvent::Incoming(response),
                Err(e) => WsTransportEvent::Error(WsTransportError::ErrorDecodingIncoming(e)),
            },
            tx,
        );

        ws.set_onopen(Some(onopen_closure.as_ref().unchecked_ref()));
        ws.set_onclose(Some(onclose_closure.as_ref().unchecked_ref()));
        ws.set_onerror(Some(onerror_closure.as_ref().unchecked_ref()));
        ws.set_onmessage(Some(onmessage_closure.as_ref().unchecked_ref()));

        // keep the closures in the memory until the `ws` exists
        let closures = WsClosures {
            _closures: vec![onopen_closure, onclose_closure, onerror_closure, onmessage_closure],
        };

        Ok((WebSocketImpl { ws, closures }, rx))
    }

    fn send_to_ws(&self, outgoing: Vec<u8>) -> Result<(), WebSocketError> {
        self.ws.send_with_u8_array(&outgoing).map_err(|error| {
            let reason = OutgoingError::UnderlyingError(stringify_js_error(&error));
            WebSocketError::OutgoingError { reason, outgoing }
        })
    }

    fn validate_websocket_url(url: &str) -> Result<(), MmError<InitWsError>> {
        let parsed_url = Url::new(url).map_to_mm(|e| InitWsError::from_ws_new_err(e, url))?;

        let scheme = parsed_url.protocol();
        if scheme != "ws:" && scheme != "wss:" {
            return MmError::err(InitWsError::InvalidUrl {
                url: url.to_string(),
                reason: "URL must use 'ws' or 'wss' scheme".to_string(),
            });
        }

        Ok(())
    }
}

impl Drop for WebSocketImpl {
    fn drop(&mut self) {
        // Reset all closures as they will not exist after `WebSocketImpl` is dropped.
        self.ws.set_onopen(None);
        self.ws.set_onclose(None);
        self.ws.set_onerror(None);
        self.ws.set_onmessage(None);

        if let Err(e) = self.ws.close_with_code(NORMAL_CLOSURE_CODE) {
            error!("Unexpected error when closing WebSocket: {e:?}");
        }
    }
}

struct WsStateMachine {
    idx: ConnIdx,
    ws: WebSocketImpl,
    /// The sender is used to send the transport events outside (to the userspace).
    event_tx: WsIncomingSender,
    /// The stream of internal events that may come from either WebSocket transport or outside (userspace, such as outgoing messages).
    state_event_rx: StateEventListener,
}

impl StateMachineTrait for WsStateMachine {
    type Result = ();
    type Error = Infallible;
}

impl StandardStateMachine for WsStateMachine {}

impl WsStateMachine {
    /// Send the `event` to the corresponding `WebSocketReceiver` instance.
    fn notify_listener(&mut self, event: WebSocketEvent) {
        if !self.event_tx.is_closed() {
            if let Err(e) = self.event_tx.try_send((self.idx, event)) {
                let error = e.to_string();
                let event = e.into_inner();
                error!("Error sending WebSocketEvent {:?}: {}", event, error);
            }
        }
    }

    fn send_unexpected_outgoing_back(&mut self, outgoing: Vec<u8>, current_state: &str) {
        error!(
            "Unexpected outgoing message while the socket idx={} state is {}",
            self.idx, current_state
        );
        let error = WebSocketEvent::Error(WebSocketError::OutgoingError {
            reason: OutgoingError::IsNotConnected,
            outgoing,
        });
        self.notify_listener(error);
    }

    fn notify_about_invalid_incoming(&mut self, description: String) {
        let error = WebSocketEvent::Error(WebSocketError::InvalidIncoming { description });
        self.notify_listener(error);
    }
}

/// `WsStateMachine` is not thread-safety `Send` because [`WebSocket::ws`] is not `Send` by default.
/// Although wasm is currently single-threaded, we can implement the `Send` trait for `WsStateMachine`,
/// but it won't be safe when wasm becomes multi-threaded.
unsafe impl Send for WsStateMachine {}

struct StateEventListener {
    rx: Box<dyn Stream<Item = StateEvent> + Unpin + Send>,
}

impl StateEventListener {
    /// Combine the `outgoing_stream` and `ws_stream` into one stream of the internal events.
    /// `ws_stream` - is a stream of the `WebSocket` events.
    /// `outgoing_stream` - is a stream of the outgoing messages came from outside (userspace).
    fn new(outgoing_stream: WsOutgoingReceiver, ws_stream: WsTransportReceiver, shutdown_rx: impl ShutdownFut) -> Self {
        use futures::stream::select;

        let mapperd_outgoing = outgoing_stream.map(StateEvent::OutgoingMessage);
        let mapped_ws_transport = ws_stream.map(StateEvent::WsTransportEvent);
        let mapped_shutdown = shutdown_rx.map(|_| StateEvent::UserSideClosed).into_stream();

        // combine the streams into one
        let internal_stream = select(select(mapperd_outgoing, mapped_ws_transport), mapped_shutdown);
        StateEventListener {
            rx: Box::new(internal_stream),
        }
    }

    async fn receive_one(&mut self) -> Option<StateEvent> {
        self.rx.next().await
    }
}

/// The combination of `WsTransportEvent` and `OutgoingEvent`
/// obtained by merging `WsTransportReceiver` and `WsOutgoingReceiver` listeners.
#[derive(Debug)]
enum StateEvent {
    /// All instances of `WsOutgoingSender` and `WsIncomingReceiver` were dropped.
    UserSideClosed,
    /// Received an outgoing message. It should be forwarded to `WebSocket`.
    OutgoingMessage(Vec<u8>),
    /// Received a `WsTransportEvent` event. It might be an incoming message from `WebSocket` or something else.
    WsTransportEvent(WsTransportEvent),
}

#[derive(Debug)]
enum WsTransportEvent {
    Establish,
    Close {
        /// https://datatracker.ietf.org/doc/html/rfc6455#section-7.4.1
        code: u16,
    },
    Error(WsTransportError),
    Incoming(Vec<u8>),
}

#[derive(Debug)]
enum WsTransportError {
    ErrorDecodingIncoming(String),
    /// An error happened on the connection. For more information about when this event
    /// occurs, see the [HTML Living Standard](https://html.spec.whatwg.org/multipage/web-sockets.html).
    /// Since the browser is not allowed to convey any information to the client code as to why an error
    /// happened (for security reasons), as described in the HTML specification, there usually is no extra
    /// information available. That's why this event has no data attached to it.
    ///
    /// This comment is copied from https://github.com/najamelan/ws_stream_wasm
    UnderlyingError,
}

impl From<CloseEvent> for WsTransportEvent {
    fn from(close: CloseEvent) -> Self {
        WsTransportEvent::Close { code: close.code() }
    }
}

struct ConnectingState;
struct OpenState;
struct ClosedState {
    reason: ClosureReason,
}

impl TransitionFrom<ConnectingState> for OpenState {}
impl TransitionFrom<ConnectingState> for ClosedState {}
impl TransitionFrom<OpenState> for ClosedState {}

#[async_trait]
impl LastState for ClosedState {
    type StateMachine = WsStateMachine;

    async fn on_changed(self: Box<Self>, ctx: &mut WsStateMachine) -> () {
        debug!("WebScoket idx={} => ClosedState", ctx.idx);
        // Notify the listener that the connection has been closed to prevent new outgoing messages.
        ctx.notify_listener(WebSocketEvent::Closed {
            reason: self.reason.clone(),
        });

        // Please note that we don't need to close websocket via `ctx.ws.close_with_code()`.
        // It will be closed on [`WsStateMachine::drop`] right after the state machine is finished.
    }
}

#[async_trait]
impl State for ConnectingState {
    type StateMachine = WsStateMachine;

    async fn on_changed(self: Box<Self>, ctx: &mut WsStateMachine) -> StateResult<Self::StateMachine> {
        debug!("WebSocket idx={} => ConnectingState", ctx.idx);
        while let Some(event) = ctx.state_event_rx.receive_one().await {
            match event {
                // there is no need to keep the connection, so close the socket and change the state into `ClosedState`
                StateEvent::UserSideClosed => return Self::change_state(ClosedState::normal_closure()),
                StateEvent::OutgoingMessage(outgoing) => ctx.send_unexpected_outgoing_back(outgoing, "ConnectingState"),
                StateEvent::WsTransportEvent(WsTransportEvent::Establish) => return Self::change_state(OpenState),
                StateEvent::WsTransportEvent(WsTransportEvent::Close { code }) => {
                    return Self::change_state(ClosedState::from_status_code(code))
                },
                StateEvent::WsTransportEvent(WsTransportEvent::Error(error)) => {
                    match error {
                        // if an underlying error has occurred, it's better to close the socket
                        WsTransportError::UnderlyingError => {
                            return Self::change_state(ClosedState::on_underlying_error())
                        },
                        WsTransportError::ErrorDecodingIncoming(_error) => error!(
                            "Unexpected incoming message while the socket idx={} state is ConnectingState",
                            ctx.idx
                        ),
                    }
                },
                StateEvent::WsTransportEvent(WsTransportEvent::Incoming(incoming)) => error!(
                    "Unexpected incoming message {:?} while the socket idx={} state is ConnectingState",
                    incoming, ctx.idx
                ),
            }
        }
        error!("StateEventListener is closed unexpectedly");
        Self::change_state(ClosedState::unexpected_closure())
    }
}

#[async_trait]
impl State for OpenState {
    type StateMachine = WsStateMachine;

    async fn on_changed(self: Box<Self>, ctx: &mut WsStateMachine) -> StateResult<Self::StateMachine> {
        debug!("WebSocket idx={} => OpenState", ctx.idx);
        // notify the listener about the changed state
        ctx.notify_listener(WebSocketEvent::Establish);

        // wait for the `WsTransportEvent::Established` event or another one
        while let Some(event) = ctx.state_event_rx.receive_one().await {
            match event {
                // there is no need to keep the connection, so close the socket and change the state into `ClosedState`
                StateEvent::UserSideClosed => return Self::change_state(ClosedState::normal_closure()),
                StateEvent::OutgoingMessage(outgoing) => {
                    if let Err(e) = ctx.ws.send_to_ws(outgoing) {
                        error!("{:?}", e);
                        ctx.notify_listener(WebSocketEvent::Error(e));
                    }
                },
                StateEvent::WsTransportEvent(WsTransportEvent::Establish) => {
                    error!("Unexpected WsTransport::Establish event")
                },
                StateEvent::WsTransportEvent(WsTransportEvent::Close { code }) => {
                    return Self::change_state(ClosedState::from_status_code(code))
                },
                StateEvent::WsTransportEvent(WsTransportEvent::Error(error)) => {
                    match error {
                        // if an underlying error has occurred, it's better to close the socket
                        WsTransportError::UnderlyingError => {
                            return Self::change_state(ClosedState::on_underlying_error())
                        },
                        WsTransportError::ErrorDecodingIncoming(error) => ctx.notify_about_invalid_incoming(error),
                    }
                },
                StateEvent::WsTransportEvent(WsTransportEvent::Incoming(incoming)) => {
                    ctx.notify_listener(WebSocketEvent::Incoming(incoming))
                },
            }
        }

        error!("StateEventListener is closed unexpectedly");
        Self::change_state(ClosedState::unexpected_closure())
    }
}

impl ClosedState {
    fn normal_closure() -> ClosedState {
        ClosedState {
            reason: ClosureReason::NormalClosure,
        }
    }

    fn on_underlying_error() -> ClosedState {
        ClosedState {
            reason: ClosureReason::ClientClosedOnUnderlyingError,
        }
    }

    fn unexpected_closure() -> ClosedState {
        ClosedState {
            reason: ClosureReason::ClientReachedUnexpectedState,
        }
    }

    fn from_status_code(code: u16) -> ClosedState {
        ClosedState {
            reason: ClosureReason::from_status_code(code),
        }
    }
}

fn decode_incoming(incoming: MessageEvent) -> Result<Vec<u8>, String> {
    match incoming.data().dyn_into::<js_sys::JsString>() {
        Ok(txt) => {
            let txt = String::from(txt);
            Ok(txt.into_bytes())
        },
        Err(e) => Err(format!("Unknown MessageEvent {e:?}")),
    }
}

/// Please note the `Event` type can be `JsValue`. It doesn't lead to a runtime error, because [`JsValue::dyn_into<JsValue>()`] returns itself.
fn construct_ws_event_closure<Event, F>(mut f: F, mut event_tx: WsTransportSender) -> Closure<dyn FnMut(JsValue)>
where
    F: FnMut(Event) -> WsTransportEvent + 'static,
    Event: JsCast + 'static,
{
    Closure::new(move |event: JsValue| {
        let transport_event = match event.dyn_into::<Event>() {
            Ok(event) => f(event),
            // the `Err` variant contains untouched source `JsValue`
            Err(e) => {
                // consider using another way to obtain the `Event` type name
                let expected = std::any::type_name::<Event>();
                error!("Expected {}, found: {:?}", expected, e);
                WsTransportEvent::Error(WsTransportError::UnderlyingError)
            },
        };
        if let Err(e) = event_tx.try_send(transport_event) {
            let error = e.to_string();
            let event = e.into_inner();
            error!("Error sending WebSocketEvent {:?}: {}", event, error);
        }
    })
}

mod tests {
    use super::*;
    use common::custom_futures::timeout::FutureTimerExt;
    use common::executor::abortable_queue::AbortableQueue;
    use common::log::{debug, wasm_log::register_wasm_log};
    use common::{WasmUnwrapErrExt, WasmUnwrapExt};
    use lazy_static::lazy_static;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    lazy_static! {
        static ref CONN_IDX: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
    }

    #[wasm_bindgen_test]
    async fn test_websocket() {
        register_wasm_log();
        let conn_idx = CONN_IDX.fetch_add(1, Ordering::Relaxed);

        let abortable_system = AbortableQueue::default();

        let (mut outgoing_tx, mut incoming_rx) = spawn_ws_transport(
            conn_idx,
            "wss://electrum1.cipig.net:30020",
            &abortable_system.weak_spawner(),
        )
        .expect("!spawn_ws_transport");

        match incoming_rx.next().timeout_secs(5.).await.unwrap_w() {
            Some((_conn_idx, WebSocketEvent::Establish)) => (),
            other => panic!("Expected 'Establish' event, found: {other:?}"),
        }

        let get_version = json!({
            "jsonrpc": "2.0",
            "id": "0",
            "method": "server.version",
            "params": ["1.2", "1.4"],
        });
        let get_version = json::to_vec(&get_version).expect("Vec serialization won't fail");
        outgoing_tx.send(get_version).await.expect("!outgoing_tx.send");

        match incoming_rx.next().timeout_secs(5.).await.unwrap_w() {
            Some((_conn_idx, WebSocketEvent::Incoming(response))) => {
                let response: Json = json::from_slice(&response).expect("Failed to parse incoming message");
                debug!("Response: {:?}", response);
                assert!(response.get("result").is_some());
            },
            other => panic!("Expected 'Incoming' event, found: {other:?}"),
        }

        drop(outgoing_tx);
        // Even if the `WsOutgoingSender` is closed, the transport must not close.
        incoming_rx
            .next()
            .timeout_secs(1.)
            .await
            .expect_err_w("Expected the future to time out, received an event");

        // It's possible for `wasm_ws` submodules ONLY.
        // Generally, the shutdown channel has to close on the `WsIncomingReceiver` instance drop.
        incoming_rx
            .shutdown_tx
            .send(())
            .expect("shutdown_rx must not be dropped");
        let mut incoming_rx = incoming_rx.inner;

        match incoming_rx.next().timeout_secs(0.5).await.unwrap_w() {
            Some((
                _conn_idx,
                WebSocketEvent::Closed {
                    reason: ClosureReason::NormalClosure,
                },
            )) => (),
            other => panic!("Expected 'Closed' event with 'ClientClosed' reason, found: {other:?}"),
        }
    }

    #[wasm_bindgen_test]
    async fn test_websocket_unreachable_url() {
        register_wasm_log();
        let conn_idx = CONN_IDX.fetch_add(1, Ordering::Relaxed);

        let abortable_system = AbortableQueue::default();

        // TODO check if outgoing messages are ignored non-open states
        let (_outgoing_tx, mut incoming_rx) = spawn_ws_transport(
            conn_idx,
            "ws://electrum1.cipig.net:10017",
            &abortable_system.weak_spawner(),
        )
        .expect("!spawn_ws_transport");

        match incoming_rx.next().timeout_secs(5.).await.unwrap_w() {
            Some((
                _conn_idx,
                WebSocketEvent::Closed {
                    reason: _reason @ ClosureReason::ClientClosedOnUnderlyingError,
                },
            )) => (),
            other => panic!("Expected 'Closed' event with 'ClosedOnUnderlyingError' reason, found: {other:?}"),
        }
    }
}
