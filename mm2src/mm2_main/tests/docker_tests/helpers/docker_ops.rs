//! Docker operations trait for docker tests.
//!
//! This module provides the `CoinDockerOps` trait which defines common
//! functionality for coins running in docker containers.

use coins::utxo::rpc_clients::{NativeClient, UtxoRpcClientEnum, UtxoRpcClientOps};
use common::{block_on_f01, now_ms, wait_until_ms};
use std::thread;
use std::time::Duration;

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
