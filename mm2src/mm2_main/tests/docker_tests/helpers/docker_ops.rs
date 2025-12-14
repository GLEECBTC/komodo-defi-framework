//! Docker operations and funding locks for docker tests.
//!
//! This module provides shared infrastructure for docker test helpers:
//! - `CoinDockerOps` trait for coins running in docker containers
//! - Funding locks to prevent concurrent operations causing RPC failures
//!
//! ## Funding Locks
//!
//! The locks prevent concurrent funding operations that would cause RPC failures
//! (insufficient funds, nonce reuse, transaction confirmation race conditions).

use coins::utxo::rpc_clients::{NativeClient, UtxoRpcClientEnum, UtxoRpcClientOps};
use common::{block_on_f01, now_ms, wait_until_ms};
use std::process::Command;
use std::thread;
use std::time::Duration;
use tokio::sync::Mutex as AsyncMutex;

// =============================================================================
// CoinDockerOps trait
// =============================================================================

/// Trait for docker coin operations.
///
/// Provides common functionality for coins running in docker containers,
/// including RPC client access and readiness waiting.
///
/// Implemented by:
/// - `UtxoAssetDockerOps` (in `helpers::utxo`)
/// - `BchDockerOps` (in `helpers::utxo`)
/// - `ZCoinAssetDockerOps` (in `helpers::zcoin`)
pub trait CoinDockerOps {
    /// Get the RPC client for this coin.
    fn rpc_client(&self) -> &UtxoRpcClientEnum;

    /// Get the native RPC client, panicking if not native.
    fn native_client(&self) -> &NativeClient {
        match self.rpc_client() {
            UtxoRpcClientEnum::Native(native) => native,
            _ => panic!("UtxoRpcClientEnum::Native is expected"),
        }
    }

    /// Wait until the coin node is ready with expected transaction version.
    fn wait_ready(&self, expected_tx_version: i32) {
        let timeout = wait_until_ms(120000);
        loop {
            match block_on_f01(self.rpc_client().get_block_count()) {
                Ok(n) => {
                    if n > 1 {
                        if let UtxoRpcClientEnum::Native(client) = self.rpc_client() {
                            let hash = block_on_f01(client.get_block_hash(n)).unwrap();
                            let block = block_on_f01(client.get_block(hash)).unwrap();
                            let coinbase = block_on_f01(client.get_verbose_transaction(&block.tx[0])).unwrap();
                            log!("Coinbase tx {:?} in block {}", coinbase, n);
                            if coinbase.version == expected_tx_version {
                                break;
                            }
                        }
                    }
                },
                Err(e) => log!("{:?}", e),
            }
            assert!(now_ms() < timeout, "Test timed out");
            thread::sleep(Duration::from_secs(1));
        }
    }
}

// =============================================================================
// Funding Locks
// =============================================================================

lazy_static! {
    // -------------------------------------------------------------------------
    // UTXO coin locks
    // -------------------------------------------------------------------------

    /// Lock for MYCOIN funding operations
    pub static ref MYCOIN_LOCK: AsyncMutex<()> = AsyncMutex::new(());

    /// Lock for MYCOIN1 funding operations
    pub static ref MYCOIN1_LOCK: AsyncMutex<()> = AsyncMutex::new(());

    /// Lock for FORSLP (BCH/SLP) funding operations
    pub static ref FORSLP_LOCK: AsyncMutex<()> = AsyncMutex::new(());

    // -------------------------------------------------------------------------
    // Qtum/QRC20 lock
    // -------------------------------------------------------------------------

    /// Lock for Qtum/QRC20 funding operations.
    /// Shared by QTUM, QICK, and QORTY coins since they all run on the same Qtum node.
    pub static ref QTUM_LOCK: AsyncMutex<()> = AsyncMutex::new(());

    // -------------------------------------------------------------------------
    // ZCoin locks
    // -------------------------------------------------------------------------

    /// Lock for ZCoin generation TX (address 1)
    pub static ref ZCOIN_GEN_TX_LOCK: AsyncMutex<()> = AsyncMutex::new(());

    /// Lock for ZCoin generation TX (address 2)
    pub static ref ZCOIN_GEN_TX_LOCK_ADDR2: AsyncMutex<()> = AsyncMutex::new(());
}

/// Get the appropriate funding lock for a given ticker.
///
/// This centralizes the ticker-to-lock mapping and provides a clear error
/// message when an unknown ticker is used.
pub fn get_funding_lock(ticker: &str) -> &'static AsyncMutex<()> {
    match ticker {
        "MYCOIN" => &MYCOIN_LOCK,
        "MYCOIN1" => &MYCOIN1_LOCK,
        "FORSLP" => &FORSLP_LOCK,
        "QTUM" | "QICK" | "QORTY" => &QTUM_LOCK,
        _ => panic!("No funding lock defined for ticker: {}", ticker),
    }
}

// =============================================================================
// Docker Compose Utilities
// =============================================================================

/// Find the container ID for a docker-compose service, independent of project name.
///
/// Uses label-based lookup (`com.docker.compose.service=<service>`) which works
/// regardless of project name or container_name settings.
pub fn resolve_compose_container_id(service_name: &str) -> String {
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
    if let Some(container_id) = stdout.lines().next().map(str::trim).filter(|s| !s.is_empty()) {
        return container_id.to_string();
    }

    // Fallback: try by container name pattern
    let fallback_name = format!("kdf-{}", service_name);
    let output = Command::new("docker")
        .args(["ps", "-q", "--filter", &format!("name={}", fallback_name)])
        .output()
        .expect("failed to execute `docker ps` (name filter)");

    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Some(container_id) = stdout.lines().next().map(str::trim).filter(|s| !s.is_empty()) {
        return container_id.to_string();
    }

    panic!(
        "No running container found for docker-compose service '{}'. \
         Make sure `.docker/test-nodes.yml` is up and containers are started.",
        service_name
    );
}

/// Copy a file from a compose container to the host.
pub fn docker_cp_from_container(container_id: &str, src: &str, dst: &std::path::Path) {
    Command::new("docker")
        .arg("cp")
        .arg(format!("{}:{}", container_id, src))
        .arg(dst)
        .status()
        .expect("Failed to copy file from compose container");
}

/// Wait for a file to exist on the filesystem.
pub fn wait_for_file(path: &std::path::Path, timeout_ms: u64) {
    let timeout = wait_until_ms(timeout_ms);
    loop {
        if path.exists() {
            break;
        }
        assert!(now_ms() < timeout, "Timed out waiting for {:?}", path);
        thread::sleep(Duration::from_millis(100));
    }
}
