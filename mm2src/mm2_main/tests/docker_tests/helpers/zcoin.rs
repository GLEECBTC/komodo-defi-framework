//! ZCoin helpers for docker tests.
//!
//! This module provides:
//! - ZCoin docker operations (ZCoinAssetDockerOps)
//! - Zombie asset docker node helpers
//! - ZCoin creation utilities

use crate::docker_tests::helpers::docker_ops::CoinDockerOps;
use crate::docker_tests::helpers::env::DockerNode;
use coins::utxo::rpc_clients::UtxoRpcClientEnum;
use coins::utxo::{coin_daemon_data_dir, zcash_params_path};
use coins::z_coin::ZCoin;
use common::{block_on, now_ms, wait_until_ms};
use mm2_core::mm_ctx::MmArc;
use serde_json::json;
use std::process::Command;
use std::sync::Mutex;
use testcontainers::core::Mount;
use testcontainers::runners::SyncRunner;
use testcontainers::{core::WaitFor, GenericImage, RunnableImage};

// =============================================================================
// Docker image constants
// =============================================================================

/// Zombie asset docker image
pub const ZOMBIE_ASSET_DOCKER_IMAGE: &str = "docker.io/gleec/zombietestrunner";
/// Zombie asset docker image with tag
pub const ZOMBIE_ASSET_DOCKER_IMAGE_WITH_TAG: &str = "docker.io/gleec/zombietestrunner:multiarch";

// =============================================================================
// ZCoinAssetDockerOps
// =============================================================================

/// Docker operations for ZCoin/Zombie assets.
pub struct ZCoinAssetDockerOps {
    #[allow(dead_code)]
    ctx: MmArc,
    coin: ZCoin,
}

impl CoinDockerOps for ZCoinAssetDockerOps {
    fn rpc_client(&self) -> &UtxoRpcClientEnum {
        &self.coin.as_ref().rpc_client
    }
}

impl ZCoinAssetDockerOps {
    /// Create ZCoinAssetDockerOps with default settings.
    pub fn new() -> ZCoinAssetDockerOps {
        let (ctx, coin) = block_on(z_coin_from_spending_key(
            "secret-extended-key-main1q0k2ga2cqqqqpq8m8j6yl0say83cagrqp53zqz54w38ezs8ly9ly5ptamqwfpq85u87w0df4k8t2lwyde3n9v0gcr69nu4ryv60t0kfcsvkr8h83skwqex2nf0vr32794fmzk89cpmjptzc22lgu5wfhhp8lgf3f5vn2l3sge0udvxnm95k6dtxj2jwlfyccnum7nz297ecyhmd5ph526pxndww0rqq0qly84l635mec0x4yedf95hzn6kcgq8yxts26k98j9g32kjc8y83fe",
            "fe",
        ));

        ZCoinAssetDockerOps { ctx, coin }
    }
}

// =============================================================================
// Statics for ZCoin tests
// =============================================================================

lazy_static! {
    /// Temporary directory for ZCoin databases (created once, cleaned up on process exit)
    pub static ref TEMP_DIR: Mutex<tempfile::TempDir> = Mutex::new(tempfile::TempDir::new().unwrap());
}

// =============================================================================
// Docker node helpers
// =============================================================================

/// Start a Zombie asset docker node for testing.
pub fn zombie_asset_docker_node(port: u16) -> DockerNode {
    let image = GenericImage::new(ZOMBIE_ASSET_DOCKER_IMAGE, "multiarch")
        .with_mount(Mount::bind_mount(
            zcash_params_path().display().to_string(),
            "/root/.zcash-params",
        ))
        .with_env_var("COIN_RPC_PORT", port.to_string())
        .with_wait_for(WaitFor::message_on_stdout("config is ready"));

    let image = RunnableImage::from(image).with_mapped_port((port, port));
    let container = image.start().expect("Failed to start Zombie asset docker node");
    let config_ticker = "ZOMBIE";
    let mut conf_path = coin_daemon_data_dir(config_ticker, true);

    std::fs::create_dir_all(&conf_path).unwrap();
    conf_path.push(format!("{config_ticker}.conf"));
    Command::new("docker")
        .arg("cp")
        .arg(format!("{}:/data/node_0/{}.conf", container.id(), config_ticker))
        .arg(&conf_path)
        .status()
        .expect("Failed to execute docker command");

    let timeout = wait_until_ms(3000);
    while !conf_path.exists() {
        assert!(now_ms() < timeout, "Test timed out");
    }

    DockerNode {
        container,
        ticker: config_ticker.into(),
        port,
    }
}

// =============================================================================
// ZCoin creation utilities
// =============================================================================

/// Build asset `ZCoin` from ticker and spending_key.
pub async fn z_coin_from_spending_key(spending_key: &str, path: &str) -> (MmArc, ZCoin) {
    use coins::z_coin::{z_coin_from_conf_and_params_with_docker, ZcoinActivationParams, ZcoinRpcMode};
    use coins::{CoinProtocol, PrivKeyBuildPolicy};
    use mm2_core::mm_ctx::MmCtxBuilder;
    use mm2_test_helpers::for_tests::zombie_conf_for_docker;

    let db_path = {
        let tmp = TEMP_DIR.lock().unwrap();
        let path = tmp.path().join(format!("ZOMBIE_DB_{path}"));
        std::fs::create_dir_all(&path).unwrap();
        path
    };
    let ctx = MmCtxBuilder::new().with_conf(json!({ "dbdir": db_path})).into_mm_arc();

    let mut conf = zombie_conf_for_docker();
    let params = ZcoinActivationParams {
        mode: ZcoinRpcMode::Native,
        ..Default::default()
    };
    let pk_data = [1; 32];

    let protocol_info = match serde_json::from_value::<CoinProtocol>(conf["protocol"].take()).unwrap() {
        CoinProtocol::ZHTLC(protocol_info) => protocol_info,
        other_protocol => panic!("Failed to get protocol from config: {:?}", other_protocol),
    };

    let coin = z_coin_from_conf_and_params_with_docker(
        &ctx,
        "ZOMBIE",
        &conf,
        &params,
        PrivKeyBuildPolicy::IguanaPrivKey(pk_data.into()),
        protocol_info,
        spending_key,
    )
    .await
    .unwrap();

    (ctx, coin)
}
