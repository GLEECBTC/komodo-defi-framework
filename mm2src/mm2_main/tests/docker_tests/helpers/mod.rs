//! Shared helper functions for docker tests.
//!
//! These helpers are organized by chain type and are available to all test modules
//! regardless of feature flags.
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

pub mod docker_ops;
pub mod env;
pub mod eth;
pub mod locks;
pub mod qrc20;
pub mod sia;
pub mod swap;
pub mod tendermint;
pub mod utxo;
pub mod zcoin;
