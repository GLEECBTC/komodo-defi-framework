//! Docker operations for docker tests.
//!
//! This module provides shared infrastructure for docker test helpers:
//! - `CoinDockerOps` trait for coins running in docker containers
//! - Docker compose utilities for container management

use coins::utxo::coin_daemon_data_dir;
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
use coins::utxo::rpc_clients::NativeClient;
use coins::utxo::rpc_clients::{UtxoRpcClientEnum, UtxoRpcClientOps};
use common::{block_on_f01, now_ms, wait_until_ms};
use std::process::Command;
use std::thread;
use std::time::Duration;

use super::env::resolve_compose_container_id;

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
    /// Only used by BchDockerOps::initialize_slp for SLP token setup.
    #[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
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
// Docker Compose Utilities
// =============================================================================

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

/// Setup UTXO coin configuration from a docker-compose container.
///
/// Copies the coin configuration file from the compose container to the local
/// daemon data directory. Used when tests run against pre-started compose nodes
/// rather than testcontainers.
pub fn setup_utxo_conf_for_compose(ticker: &str, service_name: &str) {
    let mut conf_path = coin_daemon_data_dir(ticker, true);
    std::fs::create_dir_all(&conf_path).unwrap();
    conf_path.push(format!("{ticker}.conf"));

    let container_id = resolve_compose_container_id(service_name);
    let src = format!("/data/node_0/{ticker}.conf");
    docker_cp_from_container(&container_id, &src, &conf_path);
    wait_for_file(&conf_path, 3000);
}
