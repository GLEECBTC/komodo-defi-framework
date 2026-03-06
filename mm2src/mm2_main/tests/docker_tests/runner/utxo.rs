//! UTXO (MYCOIN, MYCOIN1) setup for docker tests.

use super::{DockerTestMode, DockerTestRunner};

use crate::docker_tests::helpers::docker_ops::setup_utxo_conf_for_compose;
use crate::docker_tests::helpers::env::KDF_MYCOIN_SERVICE;
use crate::docker_tests::helpers::utxo::{utxo_asset_docker_node, UtxoAssetDockerOps};

#[cfg(any(
    feature = "docker-tests-swaps",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-integration"
))]
use crate::docker_tests::helpers::env::KDF_MYCOIN1_SERVICE;

pub(super) fn setup(runner: &mut DockerTestRunner) {
    // MYCOIN
    match runner.config.mode {
        DockerTestMode::Testcontainers => {
            let node = utxo_asset_docker_node("MYCOIN", 8000);
            let utxo_ops = UtxoAssetDockerOps::from_ticker("MYCOIN");
            crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&utxo_ops, 4);
            runner.hold(node);
        },
        DockerTestMode::ComposeInit => {
            setup_utxo_conf_for_compose("MYCOIN", KDF_MYCOIN_SERVICE);
            let utxo_ops = UtxoAssetDockerOps::from_ticker("MYCOIN");
            crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&utxo_ops, 4);
        },
    }

    // MYCOIN1 (only for utxo pair tests - not needed by Sia)
    #[cfg(any(
        feature = "docker-tests-swaps",
        feature = "docker-tests-ordermatch",
        feature = "docker-tests-watchers",
        feature = "docker-tests-qrc20",
        feature = "docker-tests-integration"
    ))]
    {
        match runner.config.mode {
            DockerTestMode::Testcontainers => {
                let node = utxo_asset_docker_node("MYCOIN1", 8001);
                let utxo_ops1 = UtxoAssetDockerOps::from_ticker("MYCOIN1");
                crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&utxo_ops1, 4);
                runner.hold(node);
            },
            DockerTestMode::ComposeInit => {
                setup_utxo_conf_for_compose("MYCOIN1", KDF_MYCOIN1_SERVICE);
                let utxo_ops1 = UtxoAssetDockerOps::from_ticker("MYCOIN1");
                crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&utxo_ops1, 4);
            },
        }
    }
}
