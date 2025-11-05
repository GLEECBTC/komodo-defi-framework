use common::executor::SpawnFuture;
use compatible_time::Duration;
use derive_more::Display;
use futures::channel::mpsc::{channel, Receiver, Sender};
use futures::{
    channel::oneshot,
    future::{join_all, poll_fn},
    Future, FutureExt, SinkExt, StreamExt,
};
use futures_rustls::rustls;
use futures_ticker::Ticker;
use lazy_static::lazy_static;
use libp2p::core::transport::Boxed as BoxedTransport;
use libp2p::core::{ConnectedPoint, Endpoint};
use libp2p::floodsub::{Floodsub, FloodsubEvent, Topic as FloodsubTopic};
use libp2p::gossipsub::{PublishError, SubscriptionError, ValidationMode};
use libp2p::multiaddr::Protocol;
use libp2p::request_response::ResponseChannel;
use libp2p::swarm::{ConnectionDenied, ConnectionId, NetworkBehaviour, SwarmEvent, ToSwarm};
use libp2p::{identity, noise, PeerId, Swarm};
use libp2p::{Multiaddr, Transport};
use log::{debug, error, info};
use mm2_net::ip_addr::is_global_ipv4;
use rand::seq::SliceRandom;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::net::IpAddr;
use std::sync::{Mutex, MutexGuard};
use std::task::{Context, Poll};
use timed_map::{MapKind, TimedMap};

use super::peers_exchange::{PeerAddresses, PeersExchange, PeersExchangeRequest, PeersExchangeResponse};
use super::ping::AdexPing;
use super::request_response::{
    build_request_response_behaviour, PeerRequest, PeerResponse, RequestResponseBehaviour, RequestResponseSender,
};
use crate::application::request_response::network_info::NetworkInfoRequest;
use crate::application::request_response::P2PRequest;
use crate::relay_address::{RelayAddress, RelayAddressError};
use crate::swarm_runtime::SwarmRuntime;
use crate::{decode_message, encode_message, NetworkInfo, NetworkPorts, RequestResponseBehaviourEvent};

pub use libp2p::gossipsub::{Behaviour as Gossipsub, IdentTopic, MessageAuthenticity, MessageId, Topic, TopicHash};
pub use libp2p::gossipsub::{
    ConfigBuilder as GossipsubConfigBuilder, Event as GossipsubEvent, Message as GossipsubMessage,
};

pub type AdexCmdTx = Sender<AdexBehaviourCmd>;
pub type AdexEventRx = Receiver<AdexBehaviourEvent>;

pub const PEERS_TOPIC: &str = "PEERS";

pub(crate) const MAX_BUFFER_SIZE: usize = 1024 * 1024 - 100;

const CONNECTED_RELAYS_CHECK_INTERVAL: Duration = Duration::from_secs(30);
const ANNOUNCE_INTERVAL: Duration = Duration::from_secs(600);
const ANNOUNCE_INITIAL_DELAY: Duration = Duration::from_secs(60);
const CHANNEL_BUF_SIZE: usize = 1024 * 8;
const DEFAULT_NETID: u16 = 8762;

/// Used in time validation logic for each peer which runs immediately  after the
/// `ConnectionEstablished` event.
///
/// Be careful when updating this value, we have some defaults (like for swaps)
/// depending on this.
pub const MAX_TIME_GAP_FOR_CONNECTED_PEER: u64 = 20;

/// Used for storing peers in [`RECENTLY_DIALED_PEERS`].
const DIAL_RETRY_DELAY: Duration = Duration::from_secs(60 * 5);

lazy_static! {
    /// Tracks recently dialed peers to avoid repeated connection attempts.
    static ref RECENTLY_DIALED_PEERS: Mutex<TimedMap<Multiaddr, ()>> = Mutex::new(TimedMap::new_with_map_kind(MapKind::FxHashMap));
}

pub const DEPRECATED_NETID_LIST: &[u16] = &[
    7777, // TODO: keep it inaccessible until Q2 of 2024.
];

/// The structure is the same as `PeerResponse`,
/// but is used to prevent `PeerResponse` from being used outside the network implementation.
#[derive(Debug, Eq, PartialEq)]
pub enum AdexResponse {
    Ok { response: Vec<u8> },
    None,
    Err { error: String },
}

impl From<PeerResponse> for AdexResponse {
    fn from(res: PeerResponse) -> Self {
        match res {
            PeerResponse::Ok { res } => AdexResponse::Ok { response: res },
            PeerResponse::None => AdexResponse::None,
            PeerResponse::Err { err } => AdexResponse::Err { error: err },
        }
    }
}

impl From<AdexResponse> for PeerResponse {
    fn from(res: AdexResponse) -> Self {
        match res {
            AdexResponse::Ok { response } => PeerResponse::Ok { res: response },
            AdexResponse::None => PeerResponse::None,
            AdexResponse::Err { error } => PeerResponse::Err { err: error },
        }
    }
}

#[derive(Debug)]
pub struct AdexResponseChannel(pub ResponseChannel<PeerResponse>);

impl From<ResponseChannel<PeerResponse>> for AdexResponseChannel {
    fn from(res: ResponseChannel<PeerResponse>) -> Self {
        AdexResponseChannel(res)
    }
}

impl From<AdexResponseChannel> for ResponseChannel<PeerResponse> {
    fn from(res: AdexResponseChannel) -> Self {
        res.0
    }
}

#[derive(Debug)]
pub enum AdexBehaviourCmd {
    Subscribe {
        /// Subscribe to this topic
        topic: String,
    },
    Unsubscribe {
        /// Unsubscribe from this topic
        topic: String,
    },
    PublishMsg {
        topic: String,
        msg: Vec<u8>,
    },
    PublishMsgFrom {
        topic: String,
        msg: Vec<u8>,
        from: PeerId,
    },
    /// Request relays sequential until a response is received.
    RequestAnyRelay {
        req: Vec<u8>,
        response_tx: oneshot::Sender<Option<(PeerId, Vec<u8>)>>,
    },
    /// Request given peers and collect all their responses.
    RequestPeers {
        req: Vec<u8>,
        peers: Vec<String>,
        response_tx: oneshot::Sender<Vec<(PeerId, AdexResponse)>>,
    },
    /// Request relays and collect all their responses.
    RequestRelays {
        req: Vec<u8>,
        response_tx: oneshot::Sender<Vec<(PeerId, AdexResponse)>>,
    },
    /// Send a response using a `response_channel`.
    SendResponse {
        /// Response to a request.
        res: AdexResponse,
        /// Pass the same `response_channel` as that was obtained from [`AdexBehaviourEvent::PeerRequest`].
        response_channel: AdexResponseChannel,
    },
    GetPeersInfo {
        result_tx: oneshot::Sender<HashMap<String, Vec<String>>>,
    },
    GetGossipMesh {
        result_tx: oneshot::Sender<HashMap<String, Vec<String>>>,
    },
    GetGossipPeerTopics {
        result_tx: oneshot::Sender<HashMap<String, Vec<String>>>,
    },
    GetGossipTopicPeers {
        result_tx: oneshot::Sender<HashMap<String, Vec<String>>>,
    },
    GetRelayMesh {
        result_tx: oneshot::Sender<Vec<String>>,
    },
    /// Add a reserved peer to the peer exchange.
    AddReservedPeer {
        peer: PeerId,
        addresses: PeerAddresses,
    },
    PropagateMessage {
        message_id: MessageId,
        propagation_source: PeerId,
    },
}

/// Determines if a dial attempt to the remote should be made.
///
/// Returns `false` if a dial attempt to the given address has already been made,
/// in which case the caller must skip the dial attempt.
fn check_and_mark_dialed(recently_dialed_peers: &mut MutexGuard<TimedMap<Multiaddr, ()>>, addr: &Multiaddr) -> bool {
    if recently_dialed_peers.get(addr).is_some() {
        info!("Connection attempt was already made recently to '{addr}'.");
        return false;
    }

    recently_dialed_peers.insert_expirable(addr.clone(), (), DIAL_RETRY_DELAY);

    true
}

/// Returns info about directly connected peers.
pub async fn get_directly_connected_peers(mut cmd_tx: AdexCmdTx) -> HashMap<String, Vec<String>> {
    let (result_tx, rx) = oneshot::channel();
    let cmd = AdexBehaviourCmd::GetPeersInfo { result_tx };
    cmd_tx.send(cmd).await.expect("Rx should be present");
    rx.await.expect("Tx should be present")
}

/// Returns current gossipsub mesh state
pub async fn get_gossip_mesh(mut cmd_tx: AdexCmdTx) -> HashMap<String, Vec<String>> {
    let (result_tx, rx) = oneshot::channel();
    let cmd = AdexBehaviourCmd::GetGossipMesh { result_tx };
    cmd_tx.send(cmd).await.expect("Rx should be present");
    rx.await.expect("Tx should be present")
}

pub async fn get_gossip_peer_topics(mut cmd_tx: AdexCmdTx) -> HashMap<String, Vec<String>> {
    let (result_tx, rx) = oneshot::channel();
    let cmd = AdexBehaviourCmd::GetGossipPeerTopics { result_tx };
    cmd_tx.send(cmd).await.expect("Rx should be present");
    rx.await.expect("Tx should be present")
}

pub async fn get_gossip_topic_peers(mut cmd_tx: AdexCmdTx) -> HashMap<String, Vec<String>> {
    let (result_tx, rx) = oneshot::channel();
    let cmd = AdexBehaviourCmd::GetGossipTopicPeers { result_tx };
    cmd_tx.send(cmd).await.expect("Rx should be present");
    rx.await.expect("Tx should be present")
}

pub async fn get_relay_mesh(mut cmd_tx: AdexCmdTx) -> Vec<String> {
    let (result_tx, rx) = oneshot::channel();
    let cmd = AdexBehaviourCmd::GetRelayMesh { result_tx };
    cmd_tx.send(cmd).await.expect("Rx should be present");
    rx.await.expect("Tx should be present")
}

async fn validate_peer_time(peer: PeerId, mut response_tx: Sender<PeerId>, rp_sender: RequestResponseSender) {
    let request = P2PRequest::NetworkInfo(NetworkInfoRequest::GetPeerUtcTimestamp);
    let encoded_request = encode_message(&request)
        .expect("Static type `PeerInfoRequest::GetPeerUtcTimestamp` should never fail in serialization.");

    match request_one_peer(peer, encoded_request, rp_sender).await {
        PeerResponse::Ok { res } => {
            if let Ok(timestamp) = decode_message::<u64>(&res) {
                let now = common::get_utc_timestamp();
                let now: u64 = now
                    .try_into()
                    .unwrap_or_else(|_| panic!("`common::get_utc_timestamp` returned invalid data: {now}"));

                let diff = now.abs_diff(timestamp);

                // If time diff is in the acceptable gap, end the validation here.
                if diff <= MAX_TIME_GAP_FOR_CONNECTED_PEER {
                    debug!(
                        "Peer '{peer}' is within the acceptable time gap ({MAX_TIME_GAP_FOR_CONNECTED_PEER} seconds); time difference is {diff} seconds."
                    );
                    return;
                }
            };
        },
        other => {
            error!("Unexpected response `{other:?}` from peer `{peer}`");
            // TODO: Ideally, we should send `peer` to end the connection,
            // but we don't want to cause a breaking change yet.
            return;
        },
    }

    // If the function reaches this point, this means validation has failed.
    // Send the peer ID to disconnect from it.
    error!("Failed to validate the time for peer `{peer}`; disconnecting.");
    response_tx.send(peer).await.unwrap();
}

async fn request_one_peer(peer: PeerId, req: Vec<u8>, mut request_response_tx: RequestResponseSender) -> PeerResponse {
    // Use the internal receiver to receive a response to this request.
    let (internal_response_tx, internal_response_rx) = oneshot::channel();
    let request = PeerRequest { req };
    request_response_tx
        .send((peer, request, internal_response_tx))
        .await
        .unwrap();

    match internal_response_rx.await {
        Ok(response) => response,
        Err(e) => PeerResponse::Err {
            err: format!("Error on request the peer {peer:?}: \"{e:?}\". Request next peer"),
        },
    }
}

/// Request the peers sequential until a `PeerResponse::Ok()` will not be received.
async fn request_any_peer(
    peers: Vec<PeerId>,
    request_data: Vec<u8>,
    request_response_tx: RequestResponseSender,
    response_tx: oneshot::Sender<Option<(PeerId, Vec<u8>)>>,
) {
    debug!("start request_any_peer loop: peers {}", peers.len());
    for peer in peers {
        match request_one_peer(peer, request_data.clone(), request_response_tx.clone()).await {
            PeerResponse::Ok { res } => {
                debug!("Received a response from peer {:?}, stop the request loop", peer);
                if response_tx.send(Some((peer, res))).is_err() {
                    error!("Response oneshot channel was closed");
                }
                return;
            },
            PeerResponse::None => {
                debug!("Received None from peer {:?}, request next peer", peer);
            },
            PeerResponse::Err { err } => {
                error!("Error on request {:?} peer: {:?}. Request next peer", peer, err);
            },
        };
    }

    debug!("None of the peers responded to the request");
    if response_tx.send(None).is_err() {
        error!("Response oneshot channel was closed");
    };
}

/// Request the peers and collect all their responses.
async fn request_peers(
    peers: Vec<PeerId>,
    request_data: Vec<u8>,
    request_response_tx: RequestResponseSender,
    response_tx: oneshot::Sender<Vec<(PeerId, AdexResponse)>>,
) {
    debug!("start request_any_peer loop: peers {}", peers.len());
    let mut futures = Vec::with_capacity(peers.len());
    for peer in peers {
        let request_data = request_data.clone();
        let request_response_tx = request_response_tx.clone();
        futures.push(async move {
            let response = request_one_peer(peer, request_data, request_response_tx).await;
            (peer, response)
        })
    }

    let responses = join_all(futures)
        .await
        .into_iter()
        .map(|(peer_id, res)| {
            let res: AdexResponse = res.into();
            (peer_id, res)
        })
        .collect();

    if response_tx.send(responses).is_err() {
        error!("Response oneshot channel was closed");
    };
}

pub struct AtomicDexBehaviour {
    core: CoreBehaviour,
    event_tx: Sender<AdexBehaviourEvent>,
    runtime: SwarmRuntime,
    cmd_rx: Receiver<AdexBehaviourCmd>,
    netid: u16,
}

#[derive(NetworkBehaviour)]
pub struct CoreBehaviour {
    gossipsub: Gossipsub,
    floodsub: Floodsub,
    peers_exchange: PeersExchange,
    ping: AdexPing,
    request_response: RequestResponseBehaviour,
}

#[derive(Debug)]
pub enum AdexBehaviourEvent {
    Gossipsub(libp2p::gossipsub::Event),
    Floodsub(FloodsubEvent),
    PeersExchange(libp2p::request_response::Event<PeersExchangeRequest, PeersExchangeResponse>),
    Ping(libp2p::ping::Event),
    RequestResponse(RequestResponseBehaviourEvent),
}

impl From<CoreBehaviourEvent> for AdexBehaviourEvent {
    fn from(event: CoreBehaviourEvent) -> Self {
        match event {
            CoreBehaviourEvent::Gossipsub(event) => AdexBehaviourEvent::Gossipsub(event),
            CoreBehaviourEvent::Floodsub(event) => AdexBehaviourEvent::Floodsub(event),
            CoreBehaviourEvent::PeersExchange(event) => AdexBehaviourEvent::PeersExchange(event),
            CoreBehaviourEvent::Ping(event) => AdexBehaviourEvent::Ping(event),
            CoreBehaviourEvent::RequestResponse(event) => AdexBehaviourEvent::RequestResponse(event),
        }
    }
}

impl AtomicDexBehaviour {
    fn notify_on_adex_event(&mut self, event: AdexBehaviourEvent) {
        if let Err(e) = self.event_tx.try_send(event) {
            error!("notify_on_adex_event error {}", e);
        }
    }

    fn spawn(&self, fut: impl Future<Output = ()> + Send + 'static) {
        self.runtime.spawn(fut)
    }

    fn process_cmd(&mut self, cmd: AdexBehaviourCmd) -> Result<(), AdexBehaviourError> {
        match cmd {
            AdexBehaviourCmd::Subscribe { topic } => {
                self.core.gossipsub.subscribe(&IdentTopic::new(topic))?;
            },
            AdexBehaviourCmd::Unsubscribe { topic } => {
                self.core.gossipsub.unsubscribe(&IdentTopic::new(topic))?;
            },
            AdexBehaviourCmd::PublishMsg { topic, msg } => {
                self.core.gossipsub.publish(TopicHash::from_raw(topic), msg)?;
            },
            AdexBehaviourCmd::PublishMsgFrom { topic, msg, from } => {
                self.core
                    .gossipsub
                    .publish_from(TopicHash::from_raw(topic), msg, from)?;
            },
            AdexBehaviourCmd::RequestAnyRelay { req, response_tx } => {
                let relays = self.core.gossipsub.get_relay_mesh();
                // spawn the `request_any_peer` future
                let future = request_any_peer(relays, req, self.core.request_response.sender(), response_tx);
                self.spawn(future);
            },
            AdexBehaviourCmd::RequestPeers {
                req,
                peers,
                response_tx,
            } => {
                let peers = peers
                    .into_iter()
                    .filter_map(|peer| match peer.parse() {
                        Ok(p) => Some(p),
                        Err(e) => {
                            error!("Error on parse peer id {:?}: {:?}", peer, e);
                            None
                        },
                    })
                    .collect();
                let future = request_peers(peers, req, self.core.request_response.sender(), response_tx);
                self.spawn(future);
            },
            AdexBehaviourCmd::RequestRelays { req, response_tx } => {
                let relays = self.core.gossipsub.get_relay_mesh();
                // spawn the `request_peers` future
                let future = request_peers(relays, req, self.core.request_response.sender(), response_tx);
                self.spawn(future);
            },
            AdexBehaviourCmd::SendResponse { res, response_channel } => {
                if let Err(response) = self
                    .core
                    .request_response
                    .send_response(response_channel.into(), res.into())
                {
                    error!("Error sending response: {:?}", response);
                }
            },
            AdexBehaviourCmd::GetPeersInfo { result_tx } => {
                let result = self
                    .core
                    .gossipsub
                    .get_peers_connections()
                    .into_iter()
                    .map(|(peer_id, connected_points)| {
                        let peer_id = peer_id.to_base58();
                        let connected_points = connected_points
                            .into_iter()
                            .map(|(_conn_id, point)| match point {
                                ConnectedPoint::Dialer { address, .. } => address.to_string(),
                                ConnectedPoint::Listener { send_back_addr, .. } => send_back_addr.to_string(),
                            })
                            .collect();
                        (peer_id, connected_points)
                    })
                    .collect();
                if result_tx.send(result).is_err() {
                    debug!("Result rx is dropped");
                }
            },
            AdexBehaviourCmd::GetGossipMesh { result_tx } => {
                let result = self
                    .core
                    .gossipsub
                    .get_mesh()
                    .iter()
                    .map(|(topic, peers)| {
                        let topic = topic.to_string();
                        let peers = peers.iter().map(|peer| peer.to_string()).collect();
                        (topic, peers)
                    })
                    .collect();
                if result_tx.send(result).is_err() {
                    debug!("Result rx is dropped");
                }
            },
            AdexBehaviourCmd::GetGossipPeerTopics { result_tx } => {
                let result = self
                    .core
                    .gossipsub
                    .get_all_peer_topics()
                    .iter()
                    .map(|(peer, topics)| {
                        let peer = peer.to_string();
                        let topics = topics.iter().map(|topic| topic.to_string()).collect();
                        (peer, topics)
                    })
                    .collect();
                if result_tx.send(result).is_err() {
                    error!("Result rx is dropped");
                }
            },
            AdexBehaviourCmd::GetGossipTopicPeers { result_tx } => {
                let result = self
                    .core
                    .gossipsub
                    .get_all_topic_peers()
                    .iter()
                    .map(|(topic, peers)| {
                        let topic = topic.to_string();
                        let peers = peers.iter().map(|peer| peer.to_string()).collect();
                        (topic, peers)
                    })
                    .collect();
                if result_tx.send(result).is_err() {
                    error!("Result rx is dropped");
                }
            },
            AdexBehaviourCmd::GetRelayMesh { result_tx } => {
                let result = self
                    .core
                    .gossipsub
                    .get_relay_mesh()
                    .into_iter()
                    .map(|peer| peer.to_string())
                    .collect();
                if result_tx.send(result).is_err() {
                    error!("Result rx is dropped");
                }
            },
            AdexBehaviourCmd::AddReservedPeer { peer, addresses } => {
                self.core
                    .peers_exchange
                    .add_peer_addresses_to_reserved_peers(&peer, addresses);
            },
            AdexBehaviourCmd::PropagateMessage {
                message_id,
                propagation_source,
            } => {
                self.core
                    .gossipsub
                    .propagate_message(&message_id, &propagation_source)?;
            },
        }

        Ok(())
    }

    fn announce_listeners(&mut self, listeners: PeerAddresses) {
        let serialized = rmp_serde::to_vec(&listeners).expect("PeerAddresses serialization should never fail");
        self.core.floodsub.publish(FloodsubTopic::new(PEERS_TOPIC), serialized);
    }

    pub fn connected_relays_len(&self) -> usize {
        self.core.gossipsub.connected_relays_len()
    }

    pub fn relay_mesh_len(&self) -> usize {
        self.core.gossipsub.relay_mesh_len()
    }

    pub fn received_messages_in_period(&self) -> (Duration, usize) {
        self.core.gossipsub.get_received_messages_in_period()
    }

    pub fn connected_peers_len(&self) -> usize {
        self.core.gossipsub.get_num_peers()
    }
}

pub enum NodeType {
    Light {
        network_ports: NetworkPorts,
    },
    LightInMemory,
    Relay {
        ip: IpAddr,
        network_ports: NetworkPorts,
        wss_certs: Option<WssCerts>,
    },
    RelayInMemory {
        port: u64,
    },
}

pub struct WssCerts {
    pub server_priv_key: rustls::PrivateKey,
    pub certs: Vec<rustls::Certificate>,
}

impl NodeType {
    pub fn to_network_info(&self) -> NetworkInfo {
        match self {
            NodeType::Light { network_ports } | NodeType::Relay { network_ports, .. } => NetworkInfo::Distributed {
                network_ports: *network_ports,
            },
            NodeType::LightInMemory | NodeType::RelayInMemory { .. } => NetworkInfo::InMemory,
        }
    }

    pub fn is_relay(&self) -> bool {
        matches!(self, NodeType::Relay { .. } | NodeType::RelayInMemory { .. })
    }

    pub fn wss_certs(&self) -> Option<&WssCerts> {
        match self {
            NodeType::Relay { wss_certs, .. } => wss_certs.as_ref(),
            _ => None,
        }
    }
}

#[derive(Debug, Display)]
pub enum AdexBehaviourError {
    #[display(fmt = "{_0}")]
    ParsingRelayAddress(RelayAddressError),
    #[display(fmt = "{_0}")]
    SubscriptionError(SubscriptionError),
    #[display(fmt = "{_0}")]
    PublishError(PublishError),
    #[display(fmt = "{_0}")]
    InitializationError(String),
}

impl From<RelayAddressError> for AdexBehaviourError {
    fn from(e: RelayAddressError) -> Self {
        AdexBehaviourError::ParsingRelayAddress(e)
    }
}

impl From<SubscriptionError> for AdexBehaviourError {
    fn from(e: SubscriptionError) -> Self {
        AdexBehaviourError::SubscriptionError(e)
    }
}

impl From<PublishError> for AdexBehaviourError {
    fn from(e: PublishError) -> Self {
        AdexBehaviourError::PublishError(e)
    }
}

pub fn generate_ed25519_keypair(mut p2p_key: [u8; 32]) -> identity::Keypair {
    let secret = identity::ed25519::SecretKey::try_from_bytes(&mut p2p_key).expect("Secret length is 32 bytes");
    let keypair = identity::ed25519::Keypair::from(secret);
    identity::Keypair::from(keypair)
}

/// Custom types mapping the complex associated types of AtomicDexBehaviour to the ExpandedSwarm
type AtomicDexSwarm = Swarm<AtomicDexBehaviour>;

/// Creates and spawns new AdexBehaviour Swarm returning:
/// 1. tx to send control commands
/// 2. rx emitting gossip events to processing side
/// 3. our peer_id
/// 4. abort handle to stop the P2P processing fut
///
/// Prefer using [`spawn_gossipsub`] to make sure the Swarm is initialized and spawned on the same runtime.
/// Otherwise, you can face the following error:
/// `panicked at 'there is no reactor running, must be called from the context of a Tokio 1.x runtime'`.
#[allow(clippy::too_many_arguments)]
fn start_gossipsub(
    config: GossipsubConfig,
    on_poll: impl Fn(&AtomicDexSwarm) + Send + 'static,
) -> Result<(Sender<AdexBehaviourCmd>, AdexEventRx, PeerId), AdexBehaviourError> {
    let i_am_relay = config.node_type.is_relay();
    let mut rng = rand::thread_rng();
    let local_key = generate_ed25519_keypair(config.p2p_key);
    let local_peer_id = PeerId::from(local_key.public());
    info!("Local peer id: {:?}", local_peer_id);

    let noise_config = noise::Config::new(&local_key).expect("Signing libp2p-noise static DH keypair failed.");

    let network_info = config.node_type.to_network_info();
    info!("Network information: {:?}", network_info);

    let transport = match network_info {
        NetworkInfo::InMemory => build_memory_transport(noise_config, config.max_num_streams),
        NetworkInfo::Distributed { .. } => {
            build_dns_ws_transport(noise_config, config.node_type.wss_certs(), config.max_num_streams)
        },
    };

    let (cmd_tx, cmd_rx) = channel(CHANNEL_BUF_SIZE);
    let (event_tx, event_rx) = channel(CHANNEL_BUF_SIZE);

    let bootstrap = config
        .to_dial
        .into_iter()
        .map(|addr| addr.try_to_multiaddr(network_info))
        .collect::<Result<Vec<Multiaddr>, _>>()?;

    let (mesh_n_low, mesh_n, mesh_n_high) = if i_am_relay { (4, 8, 12) } else { (2, 4, 6) };

    // Create a Swarm to manage peers and events
    let mut swarm = {
        // to set default parameters for gossipsub use:
        // let gossipsub_config = gossipsub::GossipsubConfig::default();

        // To content-address message, we can take the hash of message and use it as an ID.
        let message_id_fn = |message: &GossipsubMessage| {
            let mut s = DefaultHasher::new();
            message.data.hash(&mut s);
            message.sequence_number.hash(&mut s);
            MessageId(s.finish().to_be_bytes().to_vec())
        };

        // set custom gossipsub
        let gossipsub_config = GossipsubConfigBuilder::default()
            .message_id_fn(message_id_fn)
            .mesh_n_low(mesh_n_low)
            .i_am_relay(i_am_relay)
            .mesh_n(mesh_n)
            .mesh_n_high(mesh_n_high)
            .validate_messages()
            .validation_mode(ValidationMode::Permissive)
            .max_transmit_size(MAX_BUFFER_SIZE)
            .build()
            .map_err(|e| AdexBehaviourError::InitializationError(e.to_owned()))?;

        // build a gossipsub network behaviour
        let gossipsub = Gossipsub::new(MessageAuthenticity::Author(local_peer_id), gossipsub_config)
            .map_err(|e| AdexBehaviourError::InitializationError(e.to_owned()))?;

        let floodsub = Floodsub::new(local_peer_id, config.netid != DEFAULT_NETID);

        let peers_exchange = PeersExchange::new(network_info);

        // build a request-response network behaviour
        let request_response = build_request_response_behaviour();

        // use default ping config with 15s interval, 20s timeout and 1 max failure
        let ping = AdexPing::new();

        let core_behaviour = CoreBehaviour {
            gossipsub,
            floodsub,
            peers_exchange,
            request_response,
            ping,
        };

        let adex_behavior = AtomicDexBehaviour {
            core: core_behaviour,
            event_tx,
            runtime: config.runtime.clone(),
            cmd_rx,
            netid: config.netid,
        };

        libp2p::swarm::SwarmBuilder::with_executor(transport, adex_behavior, local_peer_id, config.runtime.clone())
            .build()
    };

    swarm
        .behaviour_mut()
        .core
        .floodsub
        .subscribe(FloodsubTopic::new(PEERS_TOPIC.to_owned()));

    match config.node_type {
        NodeType::Relay {
            ip,
            network_ports,
            wss_certs,
        } => {
            let dns_addr: Multiaddr = format!("/ip4/{}/tcp/{}", ip, network_ports.tcp).parse().unwrap();
            libp2p::Swarm::listen_on(&mut swarm, dns_addr).unwrap();
            if wss_certs.is_some() {
                let wss_addr: Multiaddr = format!("/ip4/{}/tcp/{}/wss", ip, network_ports.wss).parse().unwrap();
                libp2p::Swarm::listen_on(&mut swarm, wss_addr).unwrap();
            }
        },
        NodeType::RelayInMemory { port } => {
            let memory_addr: Multiaddr = format!("/memory/{port}").parse().unwrap();
            libp2p::Swarm::listen_on(&mut swarm, memory_addr).unwrap();
        },
        _ => (),
    }

    let mut recently_dialed_peers = RECENTLY_DIALED_PEERS.lock().unwrap();
    for relay in bootstrap.choose_multiple(&mut rng, mesh_n) {
        if !check_and_mark_dialed(&mut recently_dialed_peers, relay) {
            continue;
        }

        match libp2p::Swarm::dial(&mut swarm, relay.clone()) {
            Ok(_) => info!("Dialed {}", relay),
            Err(e) => error!("Dial {:?} failed: {:?}", relay, e),
        }
    }

    // All currently connected peers come from the config file (because we didn't connect any other
    // ones yet), so it's safe to treat them as trusted nodes.
    let peers: Vec<_> = libp2p::Swarm::connected_peers(&swarm).cloned().collect();
    for peer in peers {
        swarm.behaviour_mut().core.gossipsub.add_explicit_peer(&peer);
    }

    drop(recently_dialed_peers);

    let mut check_connected_relays_interval =
        Ticker::new_with_next(CONNECTED_RELAYS_CHECK_INTERVAL, CONNECTED_RELAYS_CHECK_INTERVAL);

    let mut announce_interval = Ticker::new_with_next(ANNOUNCE_INTERVAL, ANNOUNCE_INITIAL_DELAY);
    let mut listening = false;

    let (timestamp_tx, mut timestamp_rx) = futures::channel::mpsc::channel(mesh_n_high);
    let polling_fut = poll_fn(move |cx: &mut Context| {
        loop {
            match swarm.behaviour_mut().cmd_rx.poll_next_unpin(cx) {
                Poll::Ready(Some(cmd)) => swarm.behaviour_mut().process_cmd(cmd).unwrap(),
                Poll::Ready(None) => return Poll::Ready(()),
                Poll::Pending => break,
            }
        }

        while let Poll::Ready(Some(peer_id)) = timestamp_rx.poll_next_unpin(cx) {
            if swarm.disconnect_peer_id(peer_id).is_err() {
                error!("Disconnection from `{peer_id}` failed unexpectedly, which should never happen.");
            }
            swarm.behaviour_mut().core.gossipsub.remove_explicit_peer(&peer_id);
        }

        loop {
            match swarm.poll_next_unpin(cx) {
                Poll::Ready(Some(event)) => {
                    debug!("Swarm event {:?}", event);

                    if let SwarmEvent::ConnectionEstablished { peer_id, .. } = &event {
                        info!("Validating time data for peer `{peer_id}`.");
                        let future = validate_peer_time(
                            *peer_id,
                            timestamp_tx.clone(),
                            swarm.behaviour().core.request_response.sender(),
                        );
                        swarm.behaviour().spawn(future);
                    }

                    if let SwarmEvent::Behaviour(event) = event {
                        if swarm.behaviour_mut().netid != DEFAULT_NETID {
                            if let AdexBehaviourEvent::Floodsub(FloodsubEvent::Message(message)) = &event {
                                for topic in &message.topics {
                                    if topic == &FloodsubTopic::new(PEERS_TOPIC) {
                                        let addresses: PeerAddresses = match rmp_serde::from_slice(&message.data) {
                                            Ok(a) => a,
                                            Err(_) => break,
                                        };
                                        swarm
                                            .behaviour_mut()
                                            .core
                                            .peers_exchange
                                            .add_peer_addresses_to_known_peers(&message.source, addresses);
                                    }
                                }
                            }
                        }
                        swarm.behaviour_mut().notify_on_adex_event(event);
                    }
                },
                Poll::Ready(None) => return Poll::Ready(()),
                Poll::Pending => break,
            }
        }

        if swarm.behaviour().core.gossipsub.is_relay() {
            while let Poll::Ready(Some(_)) = announce_interval.poll_next_unpin(cx) {
                announce_my_addresses(&mut swarm);
            }
        }

        while let Poll::Ready(Some(_)) = check_connected_relays_interval.poll_next_unpin(cx) {
            maintain_connection_to_relays(&mut swarm, &bootstrap);
        }

        if !listening && i_am_relay {
            for listener in Swarm::listeners(&swarm) {
                info!("Listening on {}", listener);
                listening = true;
            }
        }
        on_poll(&swarm);
        Poll::Pending
    });

    config.runtime.spawn(polling_fut.then(|_| futures::future::ready(())));
    Ok((cmd_tx, event_rx, local_peer_id))
}

fn maintain_connection_to_relays(swarm: &mut AtomicDexSwarm, bootstrap_addresses: &[Multiaddr]) {
    let behaviour = swarm.behaviour();
    let connected_relays = behaviour.core.gossipsub.connected_relays();
    let mesh_n_low = behaviour.core.gossipsub.get_config().mesh_n_low();
    let mesh_n = behaviour.core.gossipsub.get_config().mesh_n();
    // allow 2 * mesh_n_high connections to other nodes
    let max_n = behaviour.core.gossipsub.get_config().mesh_n_high() * 2;

    let mut rng = rand::thread_rng();
    if connected_relays.len() < mesh_n_low {
        let mut recently_dialed_peers = RECENTLY_DIALED_PEERS.lock().unwrap();
        let to_connect_num = mesh_n - connected_relays.len();
        let to_connect =
            swarm
                .behaviour_mut()
                .core
                .peers_exchange
                .get_random_peers(to_connect_num, |peer, addresses| {
                    !connected_relays.contains(peer)
                        && addresses
                            .iter()
                            .any(|addr| check_and_mark_dialed(&mut recently_dialed_peers, addr))
                });

        // choose some random bootstrap addresses to connect if peers exchange returned not enough peers
        if to_connect.len() < to_connect_num {
            let connect_bootstrap_num = to_connect_num - to_connect.len();
            for addr in bootstrap_addresses
                .iter()
                .filter(|addr| {
                    !swarm.behaviour().core.gossipsub.is_connected_to_addr(addr)
                        && check_and_mark_dialed(&mut recently_dialed_peers, addr)
                })
                .collect::<Vec<_>>()
                .choose_multiple(&mut rng, connect_bootstrap_num)
            {
                if let Err(e) = libp2p::Swarm::dial(swarm, (*addr).clone()) {
                    error!("Bootstrap addr {} dial error {}", addr, e);
                }
            }
        }
        for (peer, addresses) in to_connect {
            for addr in addresses {
                if swarm.behaviour().core.gossipsub.is_connected_to_addr(&addr) {
                    continue;
                }

                if let Err(e) = libp2p::Swarm::dial(swarm, addr.clone()) {
                    error!("Peer {} address {} dial error {}", peer, addr, e);
                }
            }
        }
        drop(recently_dialed_peers);
    }

    if connected_relays.len() > max_n {
        let to_disconnect_num = connected_relays.len() - max_n;
        let relays_mesh = swarm.behaviour().core.gossipsub.get_relay_mesh();
        let not_in_mesh: Vec<_> = connected_relays
            .iter()
            .filter(|peer| !relays_mesh.contains(peer))
            .collect();
        for peer in not_in_mesh.choose_multiple(&mut rng, to_disconnect_num) {
            if !swarm.behaviour().core.peers_exchange.is_reserved_peer(peer) {
                info!("Disconnecting peer {}", peer);
                if Swarm::disconnect_peer_id(swarm, **peer).is_err() {
                    error!("Peer {} disconnect error", peer);
                }
            }
        }
    }

    for relay in connected_relays {
        if !swarm.behaviour().core.peers_exchange.is_known_peer(&relay) {
            swarm.behaviour_mut().core.peers_exchange.add_known_peer(relay);
        }
    }
}

fn announce_my_addresses(swarm: &mut AtomicDexSwarm) {
    let global_listeners: PeerAddresses = Swarm::listeners(swarm)
        .filter(|listener| {
            for protocol in listener.iter() {
                if let Protocol::Ip4(ip) = protocol {
                    return is_global_ipv4(&ip);
                }
            }
            false
        })
        .take(1)
        .cloned()
        .collect();
    if !global_listeners.is_empty() {
        swarm.behaviour_mut().announce_listeners(global_listeners);
    }
}

#[cfg(target_arch = "wasm32")]
fn build_dns_ws_transport(
    noise_keys: noise::Config,
    _wss_certs: Option<&WssCerts>,
    max_num_streams: usize,
) -> BoxedTransport<(PeerId, libp2p::core::muxing::StreamMuxerBox)> {
    let websocket = libp2p::wasm_ext::ffi::websocket_transport();
    let transport = libp2p::wasm_ext::ExtTransport::new(websocket);

    upgrade_transport(transport, noise_keys, max_num_streams)
}

#[cfg(not(target_arch = "wasm32"))]
fn build_dns_ws_transport(
    noise_keys: noise::Config,
    wss_certs: Option<&WssCerts>,
    max_num_streams: usize,
) -> BoxedTransport<(PeerId, libp2p::core::muxing::StreamMuxerBox)> {
    use libp2p::websocket::tls as libp2p_tls;

    let ws_tcp = libp2p::dns::TokioDnsConfig::custom(
        libp2p::tcp::tokio::Transport::new(libp2p::tcp::Config::new().nodelay(true)),
        libp2p::dns::ResolverConfig::google(),
        Default::default(),
    )
    .unwrap();

    let mut ws_dns_tcp = libp2p::websocket::WsConfig::new(ws_tcp);

    if let Some(certs) = wss_certs {
        let server_priv_key = libp2p_tls::PrivateKey::new(certs.server_priv_key.0.clone());
        let certs = certs
            .certs
            .iter()
            .map(|cert| libp2p_tls::Certificate::new(cert.0.clone()));
        let wss_config = libp2p_tls::Config::new(server_priv_key, certs).unwrap();
        ws_dns_tcp.set_tls_config(wss_config);
    }

    // This is for preventing port reuse of dns/tcp instead of
    // websocket ports.
    let dns_tcp = libp2p::dns::TokioDnsConfig::custom(
        libp2p::tcp::tokio::Transport::new(libp2p::tcp::Config::new().nodelay(true)),
        libp2p::dns::ResolverConfig::google(),
        Default::default(),
    )
    .unwrap();

    let transport = dns_tcp.or_transport(ws_dns_tcp);
    upgrade_transport(transport, noise_keys, max_num_streams)
}

fn build_memory_transport(
    noise_keys: noise::Config,
    max_num_streams: usize,
) -> BoxedTransport<(PeerId, libp2p::core::muxing::StreamMuxerBox)> {
    let transport = libp2p::core::transport::MemoryTransport::default();
    upgrade_transport(transport, noise_keys, max_num_streams)
}

/// Set up an encrypted Transport over the Mplex protocol.
fn upgrade_transport<T>(
    transport: T,
    noise_config: noise::Config,
    max_num_streams: usize,
) -> BoxedTransport<(PeerId, libp2p::core::muxing::StreamMuxerBox)>
where
    T: Transport + Send + Sync + 'static + std::marker::Unpin,
    T::Output: futures::AsyncRead + futures::AsyncWrite + Unpin + Send + 'static,
    T::ListenerUpgrade: Send,
    T::Dial: Send,
    T::Error: Send + Sync + 'static,
{
    let mut yamux_cfg = libp2p::yamux::Config::default();
    yamux_cfg.set_max_num_streams(max_num_streams);

    transport
        .upgrade(libp2p::core::upgrade::Version::V1)
        .authenticate(noise_config)
        .multiplex(yamux_cfg)
        .timeout(std::time::Duration::from_secs(20))
        .map(|(peer, muxer), _| (peer, libp2p::core::muxing::StreamMuxerBox::new(muxer)))
        .boxed()
}

impl NetworkBehaviour for AtomicDexBehaviour {
    type ConnectionHandler = <CoreBehaviour as NetworkBehaviour>::ConnectionHandler;

    type ToSwarm = AdexBehaviourEvent;

    fn handle_established_inbound_connection(
        &mut self,
        connection_id: libp2p::swarm::ConnectionId,
        peer: PeerId,
        local_addr: &Multiaddr,
        remote_addr: &Multiaddr,
    ) -> Result<libp2p::swarm::THandler<Self>, libp2p::swarm::ConnectionDenied> {
        self.core
            .handle_established_inbound_connection(connection_id, peer, local_addr, remote_addr)
    }

    fn handle_established_outbound_connection(
        &mut self,
        connection_id: libp2p::swarm::ConnectionId,
        peer: PeerId,
        addr: &Multiaddr,
        role_override: libp2p::core::Endpoint,
    ) -> Result<libp2p::swarm::THandler<Self>, libp2p::swarm::ConnectionDenied> {
        self.core
            .handle_established_outbound_connection(connection_id, peer, addr, role_override)
    }

    fn handle_pending_outbound_connection(
        &mut self,
        connection_id: ConnectionId,
        maybe_peer: Option<PeerId>,
        addresses: &[Multiaddr],
        effective_role: Endpoint,
    ) -> Result<Vec<Multiaddr>, ConnectionDenied> {
        let mut found_addresses = self.core.request_response.handle_pending_outbound_connection(
            connection_id,
            maybe_peer,
            addresses,
            effective_role,
        )?;

        found_addresses.extend(self.core.peers_exchange.handle_pending_outbound_connection(
            connection_id,
            maybe_peer,
            addresses,
            effective_role,
        )?);

        found_addresses.extend(self.core.gossipsub.handle_pending_outbound_connection(
            connection_id,
            maybe_peer,
            addresses,
            effective_role,
        )?);

        found_addresses.extend(self.core.floodsub.handle_pending_outbound_connection(
            connection_id,
            maybe_peer,
            addresses,
            effective_role,
        )?);

        found_addresses.extend(self.core.ping.handle_pending_outbound_connection(
            connection_id,
            maybe_peer,
            addresses,
            effective_role,
        )?);

        Ok(found_addresses)
    }

    fn on_swarm_event(&mut self, event: libp2p::swarm::FromSwarm<Self::ConnectionHandler>) {
        self.core.on_swarm_event(event)
    }

    fn on_connection_handler_event(
        &mut self,
        peer_id: PeerId,
        connection_id: libp2p::swarm::ConnectionId,
        event: libp2p::swarm::THandlerOutEvent<Self>,
    ) {
        self.core.on_connection_handler_event(peer_id, connection_id, event)
    }

    fn poll(
        &mut self,
        cx: &mut std::task::Context<'_>,
        params: &mut impl libp2p::swarm::PollParameters,
    ) -> std::task::Poll<libp2p::swarm::ToSwarm<Self::ToSwarm, libp2p::swarm::THandlerInEvent<Self>>> {
        self.core.poll(cx, params).map(|to_swarm| match to_swarm {
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

pub struct GossipsubConfig {
    netid: u16,
    p2p_key: [u8; 32],
    runtime: SwarmRuntime,
    to_dial: Vec<RelayAddress>,
    node_type: NodeType,
    max_num_streams: usize,
}

impl GossipsubConfig {
    #[cfg(test)]
    pub(crate) fn new_for_tests(runtime: SwarmRuntime, to_dial: Vec<RelayAddress>, node_type: NodeType) -> Self {
        let mut p2p_key = [0u8; 32];
        common::os_rng(&mut p2p_key).unwrap();

        GossipsubConfig {
            netid: 333,
            p2p_key,
            runtime,
            to_dial,
            node_type,
            max_num_streams: 128,
        }
    }

    pub fn new(netid: u16, runtime: SwarmRuntime, node_type: NodeType, p2p_key: [u8; 32]) -> Self {
        GossipsubConfig {
            netid,
            p2p_key,
            runtime,
            to_dial: vec![],
            node_type,
            max_num_streams: 512,
        }
    }

    pub fn to_dial(&mut self, to_dial: Vec<RelayAddress>) -> &mut Self {
        self.to_dial = to_dial;
        self
    }

    pub fn max_num_streams(&mut self, max_num_streams: usize) -> &mut Self {
        self.max_num_streams = max_num_streams;
        self
    }
}

/// Creates and spawns new AdexBehaviour Swarm returning:
/// 1. tx to send control commands
/// 2. rx emitting gossip events to processing side
/// 3. our peer_id
/// 4. abort handle to stop the P2P processing fut.
pub async fn spawn_gossipsub(
    config: GossipsubConfig,
    on_poll: impl Fn(&AtomicDexSwarm) + Send + 'static,
) -> Result<(Sender<AdexBehaviourCmd>, AdexEventRx, PeerId), AdexBehaviourError> {
    let (result_tx, result_rx) = oneshot::channel();

    let runtime_c = config.runtime.clone();
    let fut = async move {
        let result = start_gossipsub(config, on_poll);
        result_tx.send(result).unwrap();
    };

    // `Libp2p` must be spawned on the tokio runtime
    runtime_c.spawn(fut);
    result_rx.await.expect("Fatal error on starting gossipsub")
}
