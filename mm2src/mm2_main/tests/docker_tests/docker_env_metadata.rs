//! Docker test environment metadata for state persistence and reuse.
//!
//! This module enables sharing docker test nodes across multiple test runs by:
//! 1. Serializing initialization state (contract addresses, token IDs, config paths) to JSON
//! 2. Loading metadata to skip re-initialization when nodes are already running
//!
//! Environment variables:
//! - `KDF_DOCKER_COMPOSE_ENV=1`: Skip container startup, run initialization, save metadata
//! - `KDF_DOCKER_ENV_STATE_FILE=<path>`: Load metadata, skip both startup and initialization

use ethereum_types::H160 as H160Eth;
use primitives::hash::H256;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Environment variable to indicate docker-compose mode (containers already running)
pub const ENV_DOCKER_COMPOSE_MODE: &str = "KDF_DOCKER_COMPOSE_ENV";

/// Environment variable pointing to metadata file for state reuse
pub const ENV_DOCKER_STATE_FILE: &str = "KDF_DOCKER_ENV_STATE_FILE";

/// Default metadata file path relative to project root
pub const DEFAULT_METADATA_PATH: &str = ".docker/container-runtime/docker_env_state.json";

/// Metadata capturing all initialization state for docker test nodes.
///
/// This struct is serialized to JSON after initialization and can be loaded
/// to skip re-initialization on subsequent test runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockerEnvMetadata {
    /// Version for forward compatibility
    pub version: u32,
    /// Timestamp when metadata was created
    pub created_at: u64,
    /// Which node subsystems were initialized
    pub initialized: InitializedNodes,
    /// UTXO node state (MYCOIN, MYCOIN1)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub utxo: Option<UtxoNodeState>,
    /// Qtum/QRC20 node state
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qtum: Option<QtumNodeState>,
    /// BCH/SLP node state
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slp: Option<SlpNodeState>,
    /// Geth/Ethereum node state
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geth: Option<GethNodeState>,
    /// Zombie (Zcash) node state
    #[serde(skip_serializing_if = "Option::is_none")]
    pub zombie: Option<ZombieNodeState>,
    /// Cosmos/Tendermint nodes state
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cosmos: Option<CosmosNodeState>,
    /// Sia node state
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sia: Option<SiaNodeState>,
}

/// Tracks which node subsystems were initialized
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InitializedNodes {
    pub utxo: bool,
    pub qtum: bool,
    pub slp: bool,
    pub geth: bool,
    pub zombie: bool,
    pub cosmos: bool,
    pub sia: bool,
}

/// UTXO test nodes state (MYCOIN, MYCOIN1)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UtxoNodeState {
    pub mycoin_port: u16,
    pub mycoin1_port: u16,
}

/// Qtum/QRC20 node state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QtumNodeState {
    pub port: u16,
    pub conf_path: PathBuf,
    /// QICK token contract address
    #[serde(with = "h160_hex")]
    pub qick_token_address: H160Eth,
    /// QORTY token contract address
    #[serde(with = "h160_hex")]
    pub qorty_token_address: H160Eth,
    /// QRC20 swap contract address
    #[serde(with = "h160_hex")]
    pub swap_contract_address: H160Eth,
}

/// BCH/SLP node state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlpNodeState {
    pub port: u16,
    /// SLP token ID (genesis tx hash)
    #[serde(with = "h256_hex")]
    pub token_id: H256,
    /// Private keys of wallets funded with SLP tokens
    #[serde(with = "vec_bytes32_hex")]
    pub token_owners: Vec<[u8; 32]>,
}

/// Geth/Ethereum node state with all deployed contracts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GethNodeState {
    pub rpc_url: String,
    /// The dev account funded on node creation
    #[serde(with = "h160_hex")]
    pub account: H160Eth,
    /// ERC20 test token contract
    #[serde(with = "h160_hex")]
    pub erc20_contract: H160Eth,
    /// Legacy swap contract
    #[serde(with = "h160_hex")]
    pub swap_contract: H160Eth,
    /// Maker swap V2 contract
    #[serde(with = "h160_hex")]
    pub maker_swap_v2: H160Eth,
    /// Taker swap V2 contract
    #[serde(with = "h160_hex")]
    pub taker_swap_v2: H160Eth,
    /// Watchers swap contract
    #[serde(with = "h160_hex")]
    pub watchers_swap_contract: H160Eth,
    /// ERC721 NFT contract
    #[serde(with = "h160_hex")]
    pub erc721_contract: H160Eth,
    /// ERC1155 NFT contract
    #[serde(with = "h160_hex")]
    pub erc1155_contract: H160Eth,
    /// NFT Maker swap V2 contract
    #[serde(with = "h160_hex")]
    pub nft_maker_swap_v2: H160Eth,
}

/// Zombie (Zcash-based) node state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZombieNodeState {
    pub port: u16,
    pub conf_path: PathBuf,
}

/// Cosmos/Tendermint nodes state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CosmosNodeState {
    pub nucleus_rpc_url: String,
    pub atom_rpc_url: String,
    pub runtime_dir: PathBuf,
    pub ibc_channels_ready: bool,
}

/// Sia node state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiaNodeState {
    pub rpc_host: String,
    pub rpc_port: u16,
    pub rpc_password: String,
    pub initialized: bool,
}

impl DockerEnvMetadata {
    /// Create new empty metadata
    pub fn new() -> Self {
        Self {
            version: 1,
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            initialized: InitializedNodes::default(),
            utxo: None,
            qtum: None,
            slp: None,
            geth: None,
            zombie: None,
            cosmos: None,
            sia: None,
        }
    }

    /// Save metadata to file
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        let json =
            serde_json::to_string_pretty(self).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        // Write to temp file first, then rename for atomicity
        let temp_path = path.with_extension("json.tmp");
        std::fs::write(&temp_path, json)?;
        std::fs::rename(&temp_path, path)?;

        log!("Saved docker environment metadata to {:?}", path);
        Ok(())
    }

    /// Load metadata from file
    pub fn load(path: &std::path::Path) -> std::io::Result<Self> {
        let json = std::fs::read_to_string(path)?;
        let metadata: Self =
            serde_json::from_str(&json).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        log!(
            "Loaded docker environment metadata from {:?} (created at {})",
            path,
            metadata.created_at
        );
        Ok(metadata)
    }

    /// Get the default metadata path for the project
    pub fn default_path() -> PathBuf {
        let project_root = {
            let mut current_dir = std::env::current_dir().unwrap();
            // Navigate from mm2src/mm2_main to project root
            current_dir.pop();
            current_dir.pop();
            current_dir
        };
        project_root.join(DEFAULT_METADATA_PATH)
    }
}

impl Default for DockerEnvMetadata {
    fn default() -> Self {
        Self::new()
    }
}

// Serde helpers for H160Eth (20 bytes)
mod h160_hex {
    use ethereum_types::H160;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &H160, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&format!("{:?}", value))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<H160, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

// Serde helpers for H256 (32 bytes)
mod h256_hex {
    use primitives::hash::H256;
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &H256, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let bytes: &[u8] = value.as_ref();
        serializer.serialize_str(&hex::encode(bytes))
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<H256, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
        if bytes.len() != 32 {
            return Err(serde::de::Error::custom("expected 32 bytes"));
        }
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&bytes);
        Ok(H256::from(arr))
    }
}

// Serde helpers for Vec<[u8; 32]>
mod vec_bytes32_hex {
    use serde::{self, Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &Vec<[u8; 32]>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        use serde::ser::SerializeSeq;
        let mut seq = serializer.serialize_seq(Some(value.len()))?;
        for bytes in value {
            seq.serialize_element(&hex::encode(bytes))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Vec<[u8; 32]>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let strings: Vec<String> = Vec::deserialize(deserializer)?;
        strings
            .into_iter()
            .map(|s| {
                let bytes = hex::decode(&s).map_err(serde::de::Error::custom)?;
                if bytes.len() != 32 {
                    return Err(serde::de::Error::custom("expected 32 bytes"));
                }
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                Ok(arr)
            })
            .collect()
    }
}

/// Check if we're running in docker-compose mode (containers pre-started)
pub fn is_docker_compose_mode() -> bool {
    std::env::var(ENV_DOCKER_COMPOSE_MODE).is_ok()
}

/// Get the metadata file path if set
pub fn get_metadata_file_path() -> Option<PathBuf> {
    std::env::var(ENV_DOCKER_STATE_FILE).ok().map(PathBuf::from)
}

/// Check if we should load metadata and skip initialization
pub fn should_load_metadata() -> bool {
    get_metadata_file_path().is_some()
}
