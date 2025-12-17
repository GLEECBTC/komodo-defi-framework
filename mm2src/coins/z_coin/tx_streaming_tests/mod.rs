#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod wasm;

use common::now_sec;
use mm2_test_helpers::for_tests::{PIRATE_ELECTRUMS, PIRATE_LIGHTWALLETD_URLS};

use crate::utxo::rpc_clients::ElectrumConnectionSettings;
use crate::z_coin::{ZcoinActivationParams, ZcoinRpcMode};

#[allow(dead_code)]
fn light_zcoin_activation_params() -> ZcoinActivationParams {
    ZcoinActivationParams {
        mode: ZcoinRpcMode::Light {
            electrum_servers: PIRATE_ELECTRUMS
                .iter()
                .map(|s| ElectrumConnectionSettings {
                    url: s.to_string(),
                    protocol: Default::default(),
                    disable_cert_verification: Default::default(),
                    timeout_sec: None,
                })
                .collect(),
            min_connected: None,
            max_connected: None,
            light_wallet_d_servers: PIRATE_LIGHTWALLETD_URLS.iter().map(|s| s.to_string()).collect(),
            sync_params: Some(crate::z_coin::SyncStartPoint::Date(now_sec() - 24 * 60 * 60)),
            skip_sync_params: None,
        },
        ..Default::default()
    }
}
