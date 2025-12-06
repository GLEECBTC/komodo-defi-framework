//! Environment helpers for docker tests.
//!
//! This module provides:
//! - Shared MmArc contexts (`MM_CTX`, `MM_CTX1`)
//! - Docker-compose service name constants
//! - Generic docker node helpers and types

use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};
use mm2_test_helpers::for_tests::eth_dev_conf;
use secp256k1::SecretKey;
use std::cell::Cell;
use testcontainers::{Container, GenericImage};

pub use crypto::Secp256k1Secret;

// =============================================================================
// Shared MmArc contexts
// =============================================================================

lazy_static! {
    /// Shared MmArc context for single-instance tests
    pub static ref MM_CTX: MmArc = MmCtxBuilder::new()
        .with_conf(json!({"coins":[eth_dev_conf()],"use_trading_proto_v2": true}))
        .into_mm_arc();

    /// Second MmCtx instance for Maker/Taker tests using same private keys.
    ///
    /// When enabling coins for both Maker and Taker, two distinct coin instances are created.
    /// Different instances of the same coin should have separate global nonce locks.
    /// Using different MmCtx instances assigns Maker and Taker coins to separate CoinsCtx,
    /// addressing the "replacement transaction" issue (same nonce for different transactions).
    pub static ref MM_CTX1: MmArc = MmCtxBuilder::new()
        .with_conf(json!({"use_trading_proto_v2": true}))
        .into_mm_arc();
}

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
pub const KDF_QTUM_SERVICE: &str = "qtum";
/// docker-compose service name for primary UTXO node MYCOIN
pub const KDF_MYCOIN_SERVICE: &str = "mycoin";
/// docker-compose service name for secondary UTXO node MYCOIN1
pub const KDF_MYCOIN1_SERVICE: &str = "mycoin1";
/// docker-compose service name for BCH/SLP node FORSLP
pub const KDF_FORSLP_SERVICE: &str = "forslp";
/// docker-compose service name for Zcash-based Zombie node
pub const KDF_ZOMBIE_SERVICE: &str = "zombie";
/// docker-compose service name for IBC relayer node
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
