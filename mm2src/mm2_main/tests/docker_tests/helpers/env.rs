//! Environment helpers for docker tests.
//!
//! This module provides:
//! - Docker-compose service name constants
//! - Generic docker node helpers and types

use secp256k1::SecretKey;
use std::cell::Cell;
use testcontainers::{Container, GenericImage};

pub use crypto::Secp256k1Secret;

// =============================================================================
// Thread-local test flags
// =============================================================================

thread_local! {
    /// Set test dex pubkey as Taker (to check DexFee::NoFee)
    pub static SET_BURN_PUBKEY_TO_ALICE: Cell<bool> = const { Cell::new(false) };
}

// =============================================================================
// Docker-compose service name constants
// =============================================================================

// Docker-compose service names (see `.docker/test-nodes.yml`).
// Use service names rather than container names to enable label-based lookup,
// making the code resilient to compose project name changes.

/// docker-compose service name for Qtum/QRC20 node
#[cfg(feature = "docker-tests-qrc20")]
pub const KDF_QTUM_SERVICE: &str = "qtum";

/// docker-compose service name for primary UTXO node MYCOIN
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia"
))]
pub const KDF_MYCOIN_SERVICE: &str = "mycoin";

/// docker-compose service name for secondary UTXO node MYCOIN1
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia"
))]
pub const KDF_MYCOIN1_SERVICE: &str = "mycoin1";

/// docker-compose service name for BCH/SLP node FORSLP
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
pub const KDF_FORSLP_SERVICE: &str = "forslp";

/// docker-compose service name for Zcash-based Zombie node
#[cfg(feature = "docker-tests-zcoin")]
pub const KDF_ZOMBIE_SERVICE: &str = "zombie";

/// docker-compose service name for IBC relayer node
#[cfg(any(feature = "docker-tests-tendermint", feature = "docker-tests-integration"))]
pub const KDF_IBC_RELAYER_SERVICE: &str = "ibc-relayer";

// =============================================================================
// Generic docker node struct
// =============================================================================

/// A running docker container for testing.
pub struct DockerNode {
    #[allow(dead_code)]
    pub container: Container<GenericImage>,
    #[allow(dead_code)]
    pub ticker: String,
    #[allow(dead_code)]
    pub port: u16,
}

// =============================================================================
// Utility functions
// =============================================================================

/// Generate a random secp256k1 secret key for testing.
pub fn random_secp256k1_secret() -> Secp256k1Secret {
    let priv_key = SecretKey::new(&mut rand6::thread_rng());
    Secp256k1Secret::from(*priv_key.as_ref())
}
