use async_trait::async_trait;
use compatible_time::{Duration, Instant};
use futures::channel::{mpsc, oneshot};
use futures::io::{AsyncRead, AsyncWrite};
use futures::task::Poll;
use futures::StreamExt;
use futures_ticker::Ticker;
use libp2p::core::upgrade::{read_length_prefixed, write_length_prefixed};
use libp2p::core::Endpoint;
use libp2p::request_response::{InboundFailure, Message, OutboundFailure, ProtocolSupport};
use libp2p::swarm::{ConnectionDenied, ConnectionId, ToSwarm};
use libp2p::Multiaddr;
use libp2p::{
    request_response::{
        Behaviour as RequestResponse, Config as RequestResponseConfig, Event as RequestResponseEvent, RequestId,
        ResponseChannel,
    },
    swarm::NetworkBehaviour,
    PeerId,
};
use log::{error, warn};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::io;

use super::atomicdex::MAX_BUFFER_SIZE;
use crate::{decode_message, encode_message};

macro_rules! try_io {
    ($e: expr) => {
        match $e {
            Ok(ok) => ok,
            Err(err) => return Err(io::Error::new(io::ErrorKind::InvalidData, err)),
        }
    };
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PeerRequest {
    pub req: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum PeerResponse {
    Ok { res: Vec<u8> },
    None,
    Err { err: String },
}

pub type RequestResponseReceiver = mpsc::UnboundedReceiver<(PeerId, PeerRequest, oneshot::Sender<PeerResponse>)>;
pub type RequestResponseSender = mpsc::UnboundedSender<(PeerId, PeerRequest, oneshot::Sender<PeerResponse>)>;

#[derive(Debug)]
pub enum RequestResponseBehaviourEvent {
    InboundRequest {
        peer_id: PeerId,
        request: PeerRequest,
        response_channel: ResponseChannel<PeerResponse>,
    },
    NoAction,
}

struct PendingRequest {
    tx: oneshot::Sender<PeerResponse>,
    initiated_at: Instant,
}

#[derive(Debug, Clone)]
pub enum Protocol {
    Version1,
    Version2,
}

impl AsRef<str> for Protocol {
    fn as_ref(&self) -> &str {
        match self {
            Protocol::Version1 => "/request-response/1",
            Protocol::Version2 => "/request-response/2",
        }
    }
}

#[derive(Clone)]
pub struct Codec<Proto, Req, Res> {
    phantom: std::marker::PhantomData<(Proto, Req, Res)>,
}

impl<Proto, Req, Res> Default for Codec<Proto, Req, Res> {
    fn default() -> Self {
        Codec {
            phantom: Default::default(),
        }
    }
}

#[async_trait]
impl<
        Proto: Clone + AsRef<str> + Send + Sync,
        Req: DeserializeOwned + Serialize + Send + Sync,
        Res: DeserializeOwned + Serialize + Send + Sync,
    > libp2p::request_response::Codec for Codec<Proto, Req, Res>
{
    type Protocol = Proto;
    type Request = Req;
    type Response = Res;

    async fn read_request<T>(&mut self, _protocol: &Self::Protocol, io: &mut T) -> io::Result<Self::Request>
    where
        T: AsyncRead + Unpin + Send,
    {
        read_to_end(io).await
    }

    async fn read_response<T>(&mut self, _protocol: &Self::Protocol, io: &mut T) -> io::Result<Self::Response>
    where
        T: AsyncRead + Unpin + Send,
    {
        read_to_end(io).await
    }

    async fn write_request<T>(&mut self, _protocol: &Self::Protocol, io: &mut T, req: Self::Request) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        write_all(io, &req).await
    }

    async fn write_response<T>(&mut self, _protocol: &Self::Protocol, io: &mut T, res: Self::Response) -> io::Result<()>
    where
        T: AsyncWrite + Unpin + Send,
    {
        write_all(io, &res).await
    }
}

async fn read_to_end<T, M>(io: &mut T) -> io::Result<M>
where
    T: AsyncRead + Unpin + Send,
    M: DeserializeOwned,
{
    match read_length_prefixed(io, MAX_BUFFER_SIZE).await {
        Ok(data) => Ok(try_io!(decode_message(&data))),
        Err(e) => Err(io::Error::new(io::ErrorKind::InvalidData, e)),
    }
}

async fn write_all<T, M>(io: &mut T, msg: &M) -> io::Result<()>
where
    T: AsyncWrite + Unpin + Send,
    M: Serialize,
{
    let data = try_io!(encode_message(msg));
    if data.len() > MAX_BUFFER_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Try to send data size over maximum",
        ));
    }
    write_length_prefixed(io, data).await
}

pub struct RequestResponseBehaviour {
    /// The inner RequestResponse network behaviour.
    inner: RequestResponse<Codec<Protocol, PeerRequest, PeerResponse>>,
    rx: RequestResponseReceiver,
    tx: RequestResponseSender,
    pending_requests: HashMap<RequestId, PendingRequest>,
    /// Events that need to be yielded to the outside when polling.
    events: VecDeque<RequestResponseBehaviourEvent>,
    /// Timeout for pending requests
    timeout: Duration,
    /// Interval for request timeout check
    timeout_interval: Ticker,
}

impl RequestResponseBehaviour {
    pub fn sender(&self) -> RequestResponseSender {
        self.tx.clone()
    }

    pub fn send_response(&mut self, ch: ResponseChannel<PeerResponse>, rs: PeerResponse) -> Result<(), PeerResponse> {
        self.inner.send_response(ch, rs)
    }

    pub fn send_request(
        &mut self,
        peer_id: &PeerId,
        request: PeerRequest,
        response_tx: oneshot::Sender<PeerResponse>,
    ) -> RequestId {
        let request_id = self.inner.send_request(peer_id, request);
        let pending_request = PendingRequest {
            tx: response_tx,
            initiated_at: Instant::now(),
        };
        assert!(self.pending_requests.insert(request_id, pending_request).is_none());
        request_id
    }

    fn process_request(
        &mut self,
        peer_id: PeerId,
        request: PeerRequest,
        response_channel: ResponseChannel<PeerResponse>,
    ) {
        self.events.push_back(RequestResponseBehaviourEvent::InboundRequest {
            peer_id,
            request,
            response_channel,
        })
    }

    fn process_response(&mut self, request_id: RequestId, response: PeerResponse) {
        match self.pending_requests.remove(&request_id) {
            Some(pending) => {
                if let Err(e) = pending.tx.send(response) {
                    error!("{:?}. Request {:?} is not processed", e, request_id);
                }
            },
            _ => error!("Received unknown request {:?}", request_id),
        }
    }
}

impl NetworkBehaviour for RequestResponseBehaviour {
    type ConnectionHandler =
        <RequestResponse<Codec<Protocol, PeerRequest, PeerResponse>> as NetworkBehaviour>::ConnectionHandler;

    type ToSwarm = RequestResponseBehaviourEvent;

    fn handle_established_inbound_connection(
        &mut self,
        connection_id: libp2p::swarm::ConnectionId,
        peer: PeerId,
        local_addr: &libp2p::Multiaddr,
        remote_addr: &libp2p::Multiaddr,
    ) -> Result<libp2p::swarm::THandler<Self>, libp2p::swarm::ConnectionDenied> {
        self.inner
            .handle_established_inbound_connection(connection_id, peer, local_addr, remote_addr)
    }

    fn handle_established_outbound_connection(
        &mut self,
        connection_id: libp2p::swarm::ConnectionId,
        peer: PeerId,
        addr: &libp2p::Multiaddr,
        role_override: libp2p::core::Endpoint,
    ) -> Result<libp2p::swarm::THandler<Self>, libp2p::swarm::ConnectionDenied> {
        self.inner
            .handle_established_outbound_connection(connection_id, peer, addr, role_override)
    }

    fn handle_pending_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        maybe_peer: Option<PeerId>,
        addresses: &[Multiaddr],
        effective_role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        self.inner
            .handle_pending_outbound_connection(connection_id, maybe_peer, addresses, effective_role)
    }

    fn on_swarm_event(&mut self, event: libp2p::swarm::FromSwarm<Self::ConnectionHandler>) {
        self.inner.on_swarm_event(event)
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        _connection_id: libp2p::swarm::ConnectionId,
        event: libp2p::swarm::THandlerOutEvent<Self>,
    ) {
        let (peer_id, message) = match event {
            libp2p::request_response::HandlerEvent::Request {
                request_id,
                request,
                sender,
            } => {
                let channel = ResponseChannel { sender };
                let message: Message<PeerRequest, PeerResponse> = Message::Request {
                    request_id,
                    request,
                    channel,
                };

                (peer_id, message)
            },
            libp2p::request_response::HandlerEvent::Response { request_id, response } => {
                let message = Message::Response { request_id, response };
                (peer_id, message)
            },
            libp2p::request_response::HandlerEvent::ResponseSent(_) => return,
            libp2p::request_response::HandlerEvent::ResponseOmission(_) => {
                error!("Error on receive a request: {:?}", InboundFailure::ResponseOmission);
                return;
            },
            libp2p::request_response::HandlerEvent::OutboundTimeout(request_id) => {
                let error = OutboundFailure::Timeout;
                error!(
                    "Error on send request {:?} to peer {:?}: {:?}",
                    request_id, peer_id, error
                );
                let err_response = PeerResponse::Err {
                    err: format!("{error:?}"),
                };
                self.process_response(request_id, err_response);
                return;
            },
            libp2p::request_response::HandlerEvent::OutboundUnsupportedProtocols(request_id) => {
                let error = OutboundFailure::UnsupportedProtocols;
                error!(
                    "Error on send request {:?} to peer {:?}: {:?}",
                    request_id, peer_id, error
                );
                let err_response = PeerResponse::Err {
                    err: format!("{error:?}"),
                };
                self.process_response(request_id, err_response);
                return;
            },
        };

        match message {
            Message::Request { request, channel, .. } => {
                log::debug!("Received a request from {:?} peer", peer_id);
                self.process_request(peer_id, request, channel)
            },
            Message::Response { request_id, response } => {
                log::debug!(
                    "Received a response to the {:?} request from peer {:?}",
                    request_id,
                    peer_id
                );
                self.process_response(request_id, response)
            },
        }
    }

    fn poll(
        &mut self,
        cx: &mut std::task::Context<'_>,
        params: &mut impl libp2p::swarm::PollParameters,
    ) -> std::task::Poll<libp2p::swarm::ToSwarm<Self::ToSwarm, libp2p::swarm::THandlerInEvent<Self>>> {
        // poll the `rx`
        match self.rx.poll_next_unpin(cx) {
            // received a request, forward it through the network and put to the `pending_requests`
            Poll::Ready(Some((peer_id, request, response_tx))) => {
                let _request_id = self.send_request(&peer_id, request, response_tx);
            },
            // the channel was closed
            Poll::Ready(None) => panic!("request-response channel has been closed"),
            Poll::Pending => (),
        }

        if let Some(event) = self.events.pop_front() {
            // forward a pending event to the top
            return Poll::Ready(ToSwarm::GenerateEvent(event));
        }

        while let Poll::Ready(Some(_)) = self.timeout_interval.poll_next_unpin(cx) {
            let now = Instant::now();
            let timeout = self.timeout;
            self.pending_requests.retain(|request_id, pending_request| {
                let retain = now.duration_since(pending_request.initiated_at) < timeout;
                if !retain {
                    warn!("Request {} timed out", request_id);
                }
                retain
            });
        }

        self.inner.poll(cx, params).map(|to_swarm| match to_swarm {
            ToSwarm::GenerateEvent(event) => ToSwarm::GenerateEvent(event.into()),
            ToSwarm::Dial { opts } => ToSwarm::Dial { opts },
            ToSwarm::ListenOn { opts } => ToSwarm::ListenOn { opts },
            ToSwarm::RemoveListener { id } => ToSwarm::RemoveListener { id },
            ToSwarm::NotifyHandler {
                peer_id,
                handler,
                event,
            } => ToSwarm::NotifyHandler {
                peer_id,
                handler,
                event,
            },
            ToSwarm::NewExternalAddrCandidate(multiaddr) => ToSwarm::NewExternalAddrCandidate(multiaddr),
            ToSwarm::ExternalAddrConfirmed(multiaddr) => ToSwarm::ExternalAddrConfirmed(multiaddr),
            ToSwarm::ExternalAddrExpired(multiaddr) => ToSwarm::ExternalAddrExpired(multiaddr),
            ToSwarm::CloseConnection { peer_id, connection } => ToSwarm::CloseConnection { peer_id, connection },
        })
    }
}

impl From<libp2p::request_response::Event<PeerRequest, PeerResponse>> for RequestResponseBehaviourEvent {
    fn from(event: libp2p::request_response::Event<PeerRequest, PeerResponse>) -> Self {
        match event {
            RequestResponseEvent::Message { peer, message } => match message {
                Message::Request { request, channel, .. } => Self::InboundRequest {
                    peer_id: peer,
                    request,
                    response_channel: channel,
                },
                Message::Response { .. } => RequestResponseBehaviourEvent::NoAction,
            },
            RequestResponseEvent::OutboundFailure { .. } => RequestResponseBehaviourEvent::NoAction,
            RequestResponseEvent::InboundFailure { .. } => RequestResponseBehaviourEvent::NoAction,
            RequestResponseEvent::ResponseSent { .. } => RequestResponseBehaviourEvent::NoAction,
        }
    }
}

/// Build a request-response network behaviour.
pub fn build_request_response_behaviour() -> RequestResponseBehaviour {
    let config = RequestResponseConfig::default();
    // We don't want to support V1 since it was only used in 7777 old layer.
    let protocol = core::iter::once((Protocol::Version2, ProtocolSupport::Full));
    let inner = RequestResponse::new(protocol, config);

    let (tx, rx) = mpsc::unbounded();
    let pending_requests = HashMap::new();
    let events = VecDeque::new();
    let timeout = Duration::from_secs(10);
    let timeout_interval = Ticker::new(Duration::from_secs(3));

    RequestResponseBehaviour {
        inner,
        rx,
        tx,
        pending_requests,
        events,
        timeout,
        timeout_interval,
    }
}
