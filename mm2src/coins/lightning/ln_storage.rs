use async_trait::async_trait;
use bitcoin::Network;
use common::log::LogState;
use lightning::routing::gossip;
use lightning::routing::scoring::ProbabilisticScorer;
use parking_lot::Mutex as PaMutex;
use secp256k1::PublicKey;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

pub type NodesAddressesMap = HashMap<PublicKey, SocketAddr>;
pub type NodesAddressesMapShared = Arc<PaMutex<NodesAddressesMap>>;
pub type TrustedNodesShared = Arc<PaMutex<HashSet<PublicKey>>>;

pub type NetworkGraph = gossip::NetworkGraph<Arc<LogState>>;
pub type Scorer = Mutex<ProbabilisticScorer<Arc<NetworkGraph>, Arc<LogState>>>;

#[async_trait]
pub trait LightningStorage {
    type Error;

    /// Initializes dirs/collection/tables in storage for a specified coin
    async fn init_fs(&self) -> Result<(), Self::Error>;

    async fn is_fs_initialized(&self) -> Result<bool, Self::Error>;

    async fn get_nodes_addresses(&self) -> Result<NodesAddressesMap, Self::Error>;

    async fn save_nodes_addresses(&self, nodes_addresses: NodesAddressesMapShared) -> Result<(), Self::Error>;

    async fn get_network_graph(&self, network: Network, logger: Arc<LogState>) -> Result<NetworkGraph, Self::Error>;

    async fn get_scorer(&self, network_graph: Arc<NetworkGraph>, logger: Arc<LogState>) -> Result<Scorer, Self::Error>;

    async fn get_trusted_nodes(&self) -> Result<HashSet<PublicKey>, Self::Error>;

    async fn save_trusted_nodes(&self, trusted_nodes: TrustedNodesShared) -> Result<(), Self::Error>;
}
