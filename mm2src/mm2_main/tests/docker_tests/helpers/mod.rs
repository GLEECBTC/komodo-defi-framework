//! Shared helper functions for docker tests.
//!
//! These helpers are organized by chain type. Most are gated on `run-docker-tests`,
//! while some (env, eth) are also available for sepolia tests.
//!
//! ## Module organization
//!
//! - `docker_ops` - Docker operations trait (`CoinDockerOps`) for coins in containers
//! - `env` - Environment setup: shared contexts, service constants, metadata loading
//! - `eth` - Ethereum/ERC20: Geth initialization, contract deployment, funding
//! - `utxo` - UTXO coins: MYCOIN, MYCOIN1, BCH/SLP helpers
//! - `qrc20` - Qtum/QRC20: contract initialization, coin creation
//! - `sia` - Sia: node setup, RPC configuration
//! - `swap` - Cross-chain swap orchestration helpers
//! - `tendermint` - Cosmos/Tendermint: node setup, IBC channels
//! - `zcoin` - ZCoin/Zombie: sapling cache, node setup
//! - `locks` - Simple lock helpers used by UTXO/QRC20 helpers

// Docker-specific helpers, only needed when docker tests are enabled.
#[cfg(feature = "run-docker-tests")]
pub mod docker_ops;

// Environment helpers - also used by sepolia tests
#[cfg(any(
    feature = "run-docker-tests",
    feature = "sepolia-maker-swap-v2-tests",
    feature = "sepolia-taker-swap-v2-tests",
))]
pub mod env;

// ETH helpers - also used by sepolia tests
#[cfg(any(
    feature = "run-docker-tests",
    feature = "sepolia-maker-swap-v2-tests",
    feature = "sepolia-taker-swap-v2-tests",
))]
pub mod eth;

// Simple lock helpers used by UTXO/QRC20 helpers.
#[cfg(feature = "run-docker-tests")]
pub mod locks;

// QRC20 helpers (Qtum/QRC20 docker nodes & contracts).
#[cfg(feature = "run-docker-tests")]
pub mod qrc20;

// Sia helpers (Sia docker nodes).
#[cfg(feature = "run-docker-tests")]
pub mod sia;

// Cross-chain swap orchestration helpers.
#[cfg(feature = "run-docker-tests")]
pub mod swap;

// Tendermint / IBC helpers.
#[cfg(feature = "run-docker-tests")]
pub mod tendermint;

// UTXO (incl. SLP) helpers.
#[cfg(feature = "run-docker-tests")]
pub mod utxo;

// ZCoin/Zombie helpers.
#[cfg(feature = "run-docker-tests")]
pub mod zcoin;
