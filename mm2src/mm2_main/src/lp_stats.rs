/// The module is responsible for mm2 network stats collection
///
use common::executor::{SpawnFuture, Timer};
use common::{log, HttpStatusCode};
use derive_more::Display;
use futures::lock::Mutex as AsyncMutex;
use http::StatusCode;
use mm2_core::mm_ctx::{from_ctx, MmArc};
use mm2_err_handle::prelude::*;
use mm2_libp2p::application::request_response::network_info::NetworkInfoRequest;
use mm2_libp2p::{encode_message, NetworkInfo, PeerId, RelayAddress, RelayAddressError};
use mm2_net::ip_addr::ParseAddressError;
use serde_json::{self as json, Value as Json};
use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::sync::Arc;

use crate::lp_network::{add_reserved_peer_addresses, lp_network_ports, request_peers, NetIdError, PeerDecodedResponse};
use std::str::FromStr;

pub type NodeVersionResult<T> = Result<T, MmError<NodeVersionError>>;

#[derive(Debug, Deserialize, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum NodeVersionError {
    #[display(fmt = "Invalid request: {}", _0)]
    InvalidRequest(String),
    #[display(fmt = "Database error: {}", _0)]
    DatabaseError(String),
    #[display(fmt = "Invalid address: {}", _0)]
    InvalidAddress(String),
    #[display(fmt = "Error on parse peer id {}: {}", _0, _1)]
    PeerIdParseError(String, String),
    #[display(fmt = "{} is only supported in native mode", _0)]
    UnsupportedMode(String),
    #[display(fmt = "start_version_stat_collection is already running")]
    AlreadyRunning,
    #[display(fmt = "Version stat collection is currently stopping")]
    CurrentlyStopping,
    #[display(fmt = "start_version_stat_collection is not running")]
    NotRunning,
}

impl HttpStatusCode for NodeVersionError {
    fn status_code(&self) -> StatusCode {
        match self {
            NodeVersionError::InvalidRequest(_)
            | NodeVersionError::InvalidAddress(_)
            | NodeVersionError::PeerIdParseError(_, _) => StatusCode::BAD_REQUEST,
            NodeVersionError::UnsupportedMode(_)
            | NodeVersionError::AlreadyRunning
            | NodeVersionError::CurrentlyStopping
            | NodeVersionError::NotRunning => StatusCode::METHOD_NOT_ALLOWED,
            NodeVersionError::DatabaseError(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<serde_json::Error> for NodeVersionError {
    fn from(e: serde_json::Error) -> Self { NodeVersionError::InvalidRequest(e.to_string()) }
}

impl From<NetIdError> for NodeVersionError {
    fn from(e: NetIdError) -> Self { NodeVersionError::InvalidAddress(e.to_string()) }
}

impl From<ParseAddressError> for NodeVersionError {
    fn from(e: ParseAddressError) -> Self { NodeVersionError::InvalidAddress(e.to_string()) }
}

impl From<RelayAddressError> for NodeVersionError {
    fn from(e: RelayAddressError) -> Self { NodeVersionError::InvalidAddress(e.to_string()) }
}

#[derive(Serialize, Deserialize)]
pub struct NodeInfo {
    pub name: String,
    pub address: String,
    pub peer_id: String,
}

#[derive(Serialize, Deserialize)]
pub struct NodeVersionStat {
    pub name: String,
    pub version: Option<String>,
    pub timestamp: u64,
    pub error: Option<String>,
}

#[cfg(target_arch = "wasm32")]
fn insert_node_info_to_db(_ctx: &MmArc, _node_info: &NodeInfo) -> Result<(), String> { Ok(()) }

#[cfg(not(target_arch = "wasm32"))]
fn insert_node_info_to_db(ctx: &MmArc, node_info: &NodeInfo) -> Result<(), String> {
    crate::database::stats_nodes::insert_node_info(ctx, node_info).map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
fn insert_node_version_stat_to_db(_ctx: &MmArc, _node_version_stat: NodeVersionStat) -> Result<(), String> { Ok(()) }

#[cfg(not(target_arch = "wasm32"))]
fn insert_node_version_stat_to_db(ctx: &MmArc, node_version_stat: NodeVersionStat) -> Result<(), String> {
    crate::database::stats_nodes::insert_node_version_stat(ctx, node_version_stat).map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
fn delete_node_info_from_db(_ctx: &MmArc, _name: String) -> Result<(), String> { Ok(()) }

#[cfg(not(target_arch = "wasm32"))]
fn delete_node_info_from_db(ctx: &MmArc, name: String) -> Result<(), String> {
    crate::database::stats_nodes::delete_node_info(ctx, name).map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
fn select_peers_addresses_from_db(_ctx: &MmArc) -> Result<Vec<(String, String)>, String> { Ok(Vec::new()) }

#[cfg(not(target_arch = "wasm32"))]
fn select_peers_addresses_from_db(ctx: &MmArc) -> Result<Vec<(String, String)>, String> {
    crate::database::stats_nodes::select_peers_addresses(ctx).map_err(|e| e.to_string())
}

#[cfg(target_arch = "wasm32")]
pub async fn add_node_to_version_stat(_ctx: MmArc, _req: Json) -> NodeVersionResult<String> {
    MmError::err(NodeVersionError::UnsupportedMode("'add_node_to_version_stat'".into()))
}

/// Adds node info. to db to be used later for stats collection
#[cfg(not(target_arch = "wasm32"))]
pub async fn add_node_to_version_stat(ctx: MmArc, req: Json) -> NodeVersionResult<String> {
    let node_info: NodeInfo = json::from_value(req)?;

    // Check that the entered peer_id is valid
    let _peer_id = node_info
        .peer_id
        .parse::<PeerId>()
        .map_to_mm(|e| NodeVersionError::PeerIdParseError(node_info.peer_id.clone(), e.to_string()))?;

    let ipv4_addr = mm2_net::ip_addr::addr_to_ipv4_string(&node_info.address)?;
    let node_info_with_ipv4_addr = NodeInfo {
        name: node_info.name,
        address: ipv4_addr,
        peer_id: node_info.peer_id,
    };

    insert_node_info_to_db(&ctx, &node_info_with_ipv4_addr).map_to_mm(NodeVersionError::DatabaseError)?;

    Ok("success".into())
}

#[cfg(target_arch = "wasm32")]
pub async fn remove_node_from_version_stat(_ctx: MmArc, _req: Json) -> NodeVersionResult<String> {
    MmError::err(NodeVersionError::UnsupportedMode(
        "'remove_node_from_version_stat'".into(),
    ))
}

/// Removes node info. from db to skip collecting stats for this node
#[cfg(not(target_arch = "wasm32"))]
pub async fn remove_node_from_version_stat(ctx: MmArc, req: Json) -> NodeVersionResult<String> {
    let node_name: String = json::from_value(req["name"].clone())?;

    delete_node_info_from_db(&ctx, node_name).map_to_mm(NodeVersionError::DatabaseError)?;

    Ok("success".into())
}

#[derive(Debug, Deserialize, Serialize)]
struct Mm2VersionRes {
    nodes: HashMap<String, String>,
}

fn process_get_version_request(ctx: MmArc) -> Result<Vec<u8>, String> {
    let response = ctx.mm_version().to_string();
    encode_message(&response).map_err(|e| e.to_string())
}

fn process_get_peer_utc_timestamp_request() -> Result<Vec<u8>, String> {
    let timestamp = common::get_utc_timestamp();
    let timestamp: u64 = timestamp
        .try_into()
        .unwrap_or_else(|_| panic!("`common::get_utc_timestamp` returned invalid data: {}", timestamp));

    encode_message(&timestamp).map_err(|e| e.to_string())
}

pub fn process_info_request(ctx: MmArc, request: NetworkInfoRequest) -> Result<Vec<u8>, String> {
    match request {
        NetworkInfoRequest::GetMm2Version => process_get_version_request(ctx),
        NetworkInfoRequest::GetPeerUtcTimestamp => process_get_peer_utc_timestamp_request(),
    }
}

#[derive(PartialEq, Default)]
enum StatsCollectionStatus {
    Running,
    Updating(f64),
    Stopping,
    #[default]
    Stopped,
}

#[cfg_attr(not(target_arch = "wasm32"), derive(Default))]
struct StatsContext {
    pub status: AsyncMutex<StatsCollectionStatus>,
}

#[cfg(not(target_arch = "wasm32"))]
impl StatsContext {
    fn from_ctx(ctx: &MmArc) -> Result<Arc<StatsContext>, String> {
        Ok(try_s!(from_ctx(&ctx.stats_ctx, move || {
            Ok(StatsContext::default())
        })))
    }
}

#[cfg(target_arch = "wasm32")]
pub async fn start_version_stat_collection(_ctx: MmArc, _req: Json) -> NodeVersionResult<String> {
    MmError::err(NodeVersionError::UnsupportedMode(
        "'start_version_stat_collection'".into(),
    ))
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn start_version_stat_collection(ctx: MmArc, req: Json) -> NodeVersionResult<String> {
    let stats_ctx = StatsContext::from_ctx(&ctx).unwrap();
    {
        let state = stats_ctx.status.lock().await;
        if *state == StatsCollectionStatus::Stopping {
            return MmError::err(NodeVersionError::CurrentlyStopping);
        }
        if *state != StatsCollectionStatus::Stopped {
            return MmError::err(NodeVersionError::AlreadyRunning);
        }
    }

    let interval: f64 = json::from_value(req["interval"].clone())?;

    let peers_addresses = select_peers_addresses_from_db(&ctx).map_to_mm(NodeVersionError::DatabaseError)?;

    let netid = ctx.conf["netid"].as_u64().unwrap_or(0) as u16;
    let network_info = if ctx.p2p_in_memory() {
        NetworkInfo::InMemory
    } else {
        let network_ports = lp_network_ports(netid)?;
        NetworkInfo::Distributed { network_ports }
    };

    for (peer_id, address) in peers_addresses {
        let peer_id = peer_id
            .parse::<PeerId>()
            .map_to_mm(|e| NodeVersionError::PeerIdParseError(peer_id, e.to_string()))?;

        let relay_addr = RelayAddress::from_str(&address)?;
        let multi_address = relay_addr.try_to_multiaddr(network_info)?;

        let addresses = HashSet::from([multi_address]);
        add_reserved_peer_addresses(&ctx, peer_id, addresses);
    }

    let spawner = ctx.spawner();
    spawner.spawn(stat_collection_loop(ctx, interval));

    Ok("success".into())
}

#[cfg(not(target_arch = "wasm32"))]
async fn stat_collection_loop(ctx: MmArc, interval: f64) {
    use common::now_sec;
    use mm2_libp2p::application::request_response::P2PRequest;

    use crate::database::stats_nodes::select_peers_names;

    let mut interval = interval;
    loop {
        if ctx.is_stopping() {
            break;
        };
        {
            let stats_ctx = StatsContext::from_ctx(&ctx).unwrap();
            {
                let mut state = stats_ctx.status.lock().await;
                match *state {
                    StatsCollectionStatus::Running => (),
                    StatsCollectionStatus::Updating(i) => {
                        interval = i;
                        *state = StatsCollectionStatus::Running;
                    },
                    StatsCollectionStatus::Stopping => {
                        *state = StatsCollectionStatus::Stopped;
                        break;
                    },
                    StatsCollectionStatus::Stopped => *state = StatsCollectionStatus::Running,
                }
            }

            let peers_names = match select_peers_names(&ctx) {
                Ok(n) => n,
                Err(e) => {
                    log::error!("Error selecting peers names from db: {}", e);
                    Timer::sleep(10.).await;
                    continue;
                },
            };

            let peers: Vec<String> = peers_names.keys().cloned().collect();

            let timestamp = now_sec();
            let get_versions_res = match request_peers::<String>(
                ctx.clone(),
                P2PRequest::NetworkInfo(NetworkInfoRequest::GetMm2Version),
                peers,
            )
            .await
            {
                Ok(res) => res,
                Err(e) => {
                    log::error!("Error getting nodes versions from peers: {}", e);
                    Timer::sleep(10.).await;
                    continue;
                },
            };

            for (peer_id, response) in get_versions_res {
                let name = match peers_names.get(&peer_id.to_string()) {
                    Some(n) => n.clone(),
                    None => continue,
                };

                match response {
                    PeerDecodedResponse::Ok(v) => {
                        let node_version_stat = NodeVersionStat {
                            name: name.clone(),
                            version: Some(v.clone()),
                            timestamp,
                            error: None,
                        };
                        if let Err(e) = insert_node_version_stat_to_db(&ctx, node_version_stat) {
                            log::error!("Error inserting node {} version {} into db: {}", name, v, e);
                        };
                    },
                    PeerDecodedResponse::Err(e) => {
                        log::error!(
                            "Node {} responded to version request with error: {}",
                            name.clone(),
                            e.clone()
                        );
                        let node_version_stat = NodeVersionStat {
                            name: name.clone(),
                            version: None,
                            timestamp,
                            error: Some(e.clone()),
                        };
                        if let Err(e) = insert_node_version_stat_to_db(&ctx, node_version_stat) {
                            log::error!("Error inserting node {} error into db: {}", name, e);
                        };
                    },
                    PeerDecodedResponse::None => {
                        log::debug!("Node {} did not respond to version request", name.clone());
                        let node_version_stat = NodeVersionStat {
                            name: name.clone(),
                            version: None,
                            timestamp,
                            error: None,
                        };
                        if let Err(e) = insert_node_version_stat_to_db(&ctx, node_version_stat) {
                            log::error!("Error inserting no response for node {} into db: {}", name, e);
                        };
                    },
                }
            }
        }
        Timer::sleep(interval).await;
    }
}

#[cfg(target_arch = "wasm32")]
pub async fn update_version_stat_collection(_ctx: MmArc, _req: Json) -> NodeVersionResult<String> {
    MmError::err(NodeVersionError::UnsupportedMode(
        "'update_version_stat_collection'".into(),
    ))
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn update_version_stat_collection(ctx: MmArc, req: Json) -> NodeVersionResult<String> {
    let stats_ctx = StatsContext::from_ctx(&ctx).unwrap();
    let mut state = stats_ctx.status.lock().await;
    if *state == StatsCollectionStatus::Stopped || *state == StatsCollectionStatus::Stopping {
        return MmError::err(NodeVersionError::NotRunning);
    }

    let interval: f64 = json::from_value(req["interval"].clone())?;
    *state = StatsCollectionStatus::Updating(interval);

    Ok("success".into())
}

#[cfg(target_arch = "wasm32")]
pub async fn stop_version_stat_collection(_ctx: MmArc, _req: Json) -> NodeVersionResult<String> {
    MmError::err(NodeVersionError::UnsupportedMode(
        "'stop_version_stat_collection'".into(),
    ))
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn stop_version_stat_collection(ctx: MmArc, _req: Json) -> NodeVersionResult<String> {
    let stats_ctx = StatsContext::from_ctx(&ctx).unwrap();
    let mut state = stats_ctx.status.lock().await;
    if *state == StatsCollectionStatus::Stopped || *state == StatsCollectionStatus::Stopping {
        return MmError::err(NodeVersionError::NotRunning);
    }

    *state = StatsCollectionStatus::Stopping;

    Ok("success".into())
}
