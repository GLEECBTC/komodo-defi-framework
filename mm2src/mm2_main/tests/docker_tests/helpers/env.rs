//! Environment helpers for docker tests.
//!
//! This module provides:
//! - Docker-compose service name constants
//! - Generic docker node helpers and types

use testcontainers::{Container, GenericImage};

#[cfg(any(
    feature = "docker-tests-swaps",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-watchers-eth",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-eth",
    feature = "docker-tests-sia",
    feature = "docker-tests-integration"
))]
use crypto::Secp256k1Secret;
#[cfg(any(
    feature = "docker-tests-swaps",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-watchers-eth",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-eth",
    feature = "docker-tests-sia",
    feature = "docker-tests-integration"
))]
use secp256k1::SecretKey;

// Cell import only needed for SET_BURN_PUBKEY_TO_ALICE
#[cfg(any(
    feature = "docker-tests-swaps",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-slp",
    feature = "docker-tests-eth",
    feature = "docker-tests-integration"
))]
use std::cell::Cell;

// =============================================================================
// Thread-local test flags
// =============================================================================

#[cfg(any(
    feature = "docker-tests-swaps",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-slp",
    feature = "docker-tests-eth",
    feature = "docker-tests-integration"
))]
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
    feature = "docker-tests-swaps",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia",
    feature = "docker-tests-integration"
))]
pub const KDF_MYCOIN_SERVICE: &str = "mycoin";

/// docker-compose service name for secondary UTXO node MYCOIN1
#[cfg(any(
    feature = "docker-tests-swaps",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-integration"
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
#[cfg(any(
    feature = "docker-tests-swaps",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-watchers-eth",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-eth",
    feature = "docker-tests-sia",
    feature = "docker-tests-integration"
))]
pub fn random_secp256k1_secret() -> Secp256k1Secret {
    let priv_key = SecretKey::new(&mut rand6::thread_rng());
    Secp256k1Secret::from(*priv_key.as_ref())
}

// =============================================================================
// Docker Compose Utilities
// =============================================================================

/// Find the container ID for a docker-compose service, independent of project name.
///
/// Uses label-based lookup (`com.docker.compose.service=<service>`) which works
/// regardless of project name or container_name settings.
///
/// Note: kept in `helpers::env` so both docker-compose setup helpers and Tendermint helpers
/// can reuse it without extra dependencies.
#[cfg(any(
    feature = "docker-tests-tendermint",
    feature = "docker-tests-integration",
    feature = "docker-tests-swaps",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia",
    feature = "docker-tests-slp",
    feature = "docker-tests-zcoin"
))]
pub fn resolve_compose_container_id(service_name: &str) -> String {
    use std::process::Command;

    let output = Command::new("docker")
        .args([
            "ps",
            "-q",
            "--filter",
            &format!("label=com.docker.compose.service={}", service_name),
            "--filter",
            "status=running",
        ])
        .output()
        .expect("failed to execute `docker ps`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout
        .lines()
        .next()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .unwrap_or_else(|| {
            panic!(
                "No running container found for docker-compose service '{}'. \
                 Make sure `.docker/test-nodes.yml` is up and containers are started.",
                service_name
            )
        })
}
