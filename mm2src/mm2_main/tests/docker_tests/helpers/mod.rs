//! Shared helper functions for docker tests.
//!
//! These helpers are organized by chain type. Most are gated on `run-docker-tests`,
//! while some (env, eth) are also available for sepolia tests.
//!
//! ## Module organization
//!
//! - `docker_ops` - Docker operations trait and funding locks for coins in containers
//! - `env` - Environment setup: shared contexts, service constants
//! - `eth` - Ethereum/ERC20: Geth initialization, contract deployment, funding
//! - `utxo` - UTXO coins: MYCOIN, MYCOIN1, BCH/SLP helpers
//! - `qrc20` - Qtum/QRC20: contract initialization, coin creation
//! - `sia` - Sia: node setup, RPC configuration
//! - `swap` - Cross-chain swap orchestration helpers
//! - `tendermint` - Cosmos/Tendermint: node setup, IBC channels
//! - `zcoin` - ZCoin/Zombie: sapling cache, node setup

// Docker-specific helpers, only needed when docker tests are enabled.
// Gated on specific features to avoid unused code warnings.

// docker_ops - CoinDockerOps trait and UTXO compose utilities
// (tendermint uses resolve_compose_container_id from env.rs instead)
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia",
    feature = "docker-tests-slp",
    feature = "docker-tests-zcoin",
    feature = "docker-tests-integration"
))]
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
    feature = "docker-tests-eth",
    feature = "docker-tests-watchers-eth",
    feature = "docker-tests-integration",
    feature = "sepolia-maker-swap-v2-tests",
    feature = "sepolia-taker-swap-v2-tests",
))]
pub mod eth;

// QRC20 helpers (Qtum/QRC20 docker nodes & contracts).
#[cfg(feature = "docker-tests-qrc20")]
pub mod qrc20;

// Sia helpers (Sia docker nodes).
#[cfg(feature = "docker-tests-sia")]
pub mod sia;

// Cross-chain swap orchestration helpers.
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-eth",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-slp"
))]
pub mod swap;

// Tendermint / IBC helpers.
#[cfg(any(feature = "docker-tests-tendermint", feature = "docker-tests-integration"))]
pub mod tendermint;

// UTXO (incl. SLP) helpers.
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia",
    feature = "docker-tests-slp",
    feature = "docker-tests-integration"
))]
pub mod utxo;

// ZCoin/Zombie helpers.
#[cfg(feature = "docker-tests-zcoin")]
pub mod zcoin;
