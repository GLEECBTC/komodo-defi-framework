//! ZCoin/Zombie setup for docker tests.

use super::{DockerTestMode, DockerTestRunner};

use crate::docker_tests::helpers::docker_ops::setup_utxo_conf_for_compose;
use crate::docker_tests::helpers::env::KDF_ZOMBIE_SERVICE;
use crate::docker_tests::helpers::zcoin::{zombie_asset_docker_node, ZCoinAssetDockerOps};

pub(super) fn setup(runner: &mut DockerTestRunner) {
    match runner.config.mode {
        DockerTestMode::Testcontainers => {
            let node = zombie_asset_docker_node(7090);
            let zombie_ops = ZCoinAssetDockerOps::new();
            crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&zombie_ops, 4);
            runner.hold(node);
        },
        DockerTestMode::ComposeInit => {
            setup_utxo_conf_for_compose("ZOMBIE", KDF_ZOMBIE_SERVICE);
            let zombie_ops = ZCoinAssetDockerOps::new();
            crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&zombie_ops, 4);
        },
    }
}
