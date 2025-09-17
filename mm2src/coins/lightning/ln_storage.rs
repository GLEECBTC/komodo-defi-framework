use async_trait::async_trait;
use bitcoin::{BlockHash, Network, Txid};
use common::log::LogState;
use lightning::chain::channelmonitor::ChannelMonitor;
use lightning::routing::gossip;
use lightning::routing::scoring::{ProbabilisticScorer, ProbabilisticScoringDecayParameters};
use lightning::sign::ecdsa::WriteableEcdsaChannelSigner as Sign;
use lightning::sign::SignerProvider;
use lightning::util::persist::KVStore;
use lightning::util::ser::ReadableArgs;
use mm2_io::fs::invalid_data_err;
use parking_lot::Mutex as PaMutex;
use secp256k1::PublicKey;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::ops::Deref;
use std::str::FromStr;
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
    // async fn init_fs(&self) -> Result<(), Self::Error>;

    // async fn is_fs_initialized(&self) -> Result<bool, Self::Error>;

    async fn get_nodes_addresses(&self) -> Result<NodesAddressesMap, Self::Error>;

    async fn save_nodes_addresses(&self, nodes_addresses: NodesAddressesMapShared) -> Result<(), Self::Error>;

    async fn get_network_graph(&self, network: Network, logger: Arc<LogState>) -> Result<NetworkGraph, Self::Error>;

    async fn get_scorer(&self, network_graph: Arc<NetworkGraph>, logger: Arc<LogState>) -> Result<Scorer, Self::Error>;

    async fn get_trusted_nodes(&self) -> Result<HashSet<PublicKey>, Self::Error>;

    async fn save_trusted_nodes(&self, trusted_nodes: TrustedNodesShared) -> Result<(), Self::Error>;

    fn read_channelmonitors<Signer: Sign, K: Deref>(
        &self,
        keys_manager: K,
    ) -> Result<Vec<(BlockHash, ChannelMonitor<Signer>)>, std::io::Error>
    where
        K::Target: SignerProvider<EcdsaSigner = Signer> + Sized;
}

#[async_trait]
impl<Store: KVStore + Sync> LightningStorage for Store {
    type Error = std::io::Error;

    async fn get_nodes_addresses(&self) -> Result<NodesAddressesMap, Self::Error> {
        let content = self.read("", "", "channel_nodes_data")?;
        serde_json::from_slice(&content).map_err(|err| invalid_data_err("Error", err))
    }

    async fn save_nodes_addresses(&self, nodes_addresses: NodesAddressesMapShared) -> Result<(), Self::Error> {
        let nodes_addresses: HashMap<String, SocketAddr> = nodes_addresses
            .lock()
            .iter()
            .map(|(pubkey, addr)| (pubkey.to_string(), *addr))
            .collect();
        let content = serde_json::to_vec(&nodes_addresses).map_err(|err| invalid_data_err("Error", err))?;
        self.write("", "", "channel_nodes_data", &content)
    }

    async fn get_network_graph(&self, network: Network, logger: Arc<LogState>) -> Result<NetworkGraph, Self::Error> {
        common::log::info!("Reading the saved lightning network graph from file, this can take some time!");
        let content = self.read("", "", "network_graph")?;
        NetworkGraph::read(&mut content, logger).map_err(|e| invalid_data_err("Error", e))
    }

    async fn get_scorer(&self, network_graph: Arc<NetworkGraph>, logger: Arc<LogState>) -> Result<Scorer, Self::Error> {
        let content = self.read("", "", "scorer")?;
        let scorer = ProbabilisticScorer::read(
            &mut content.as_slice(),
            (ProbabilisticScoringDecayParameters::default(), network_graph, logger),
        )
        .map_err(|e| invalid_data_err("Error", e))?;
        Ok(Mutex::new(scorer))
    }

    async fn get_trusted_nodes(&self) -> Result<HashSet<PublicKey>, Self::Error> {
        let content = self.read("", "", "trusted_nodes")?;
        let trusted_nodes: HashSet<String> =
            serde_json::from_slice(&content).map_err(|err| invalid_data_err("Error", err))?;
        trusted_nodes
            .iter()
            .map(|pubkey_str| {
                let pubkey = PublicKey::from_str(pubkey_str).map_err(|e| invalid_data_err("Error", e))?;
                Ok(pubkey)
            })
            .collect()
    }

    async fn save_trusted_nodes(&self, trusted_nodes: TrustedNodesShared) -> Result<(), Self::Error> {
        let trusted_nodes: HashSet<String> = trusted_nodes.lock().iter().map(|pubkey| pubkey.to_string()).collect();
        let content = serde_json::to_vec(&trusted_nodes).map_err(|err| invalid_data_err("Error", err))?;
        self.write("", "", "trusted_nodes", &content)
    }

    /// Read `ChannelMonitor`s from disk.
    fn read_channelmonitors<Signer: Sign, K: Deref>(
        &self,
        keys_manager: K,
    ) -> Result<Vec<(BlockHash, ChannelMonitor<Signer>)>, std::io::Error>
    where
        K::Target: SignerProvider<EcdsaSigner = Signer> + Sized,
    {
        let mut res = Vec::new();
        for filename in self.list("monitors", "")? {
            if filename.len() < 65 {
                return Err(invalid_data_err("Invalid ChannelMonitor file name", filename));
            }
            if filename.ends_with(".tmp") {
                // If we were in the middle of committing an new update and crashed, it should be
                // safe to ignore the update - we should never have returned to the caller and
                // irrevocably committed to the new state in any way.
                continue;
            }

            let txid = Txid::from_hex(filename.split_at(64).0)
                .map_err(|e| invalid_data_err("Invalid tx ID in filename error", e))?;

            let index = filename
                .split_at(65)
                .1
                .parse::<u16>()
                .map_err(|e| invalid_data_err("Invalid tx index in filename error", e))?;

            let content = self.read("monitors", "", &filename)?;
            let (blockhash, channel_monitor) =
                <(BlockHash, ChannelMonitor<Signer>)>::read(&mut content, &*keys_manager)
                    .map_err(|e| invalid_data_err("Failed to deserialize ChannelMonito", e))?;

            if channel_monitor.get_funding_txo().0.txid != txid || channel_monitor.get_funding_txo().0.index != index {
                return Err(invalid_data_err(
                    "ChannelMonitor was stored in the wrong file",
                    filename,
                ));
            }

            res.push((blockhash, channel_monitor));
        }
        Ok(res)
    }
}
