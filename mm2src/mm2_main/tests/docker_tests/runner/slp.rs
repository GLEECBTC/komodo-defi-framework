//! SLP/BCH (FORSLP) setup for docker tests.

use super::{DockerTestMode, DockerTestRunner};

use crate::docker_tests::helpers::docker_ops::setup_utxo_conf_for_compose;
use crate::docker_tests::helpers::env::KDF_FORSLP_SERVICE;
use crate::docker_tests::helpers::slp::{forslp_docker_node, BchDockerOps};

pub(super) fn setup(runner: &mut DockerTestRunner) {
    match runner.config.mode {
        DockerTestMode::Testcontainers => {
            let node = forslp_docker_node(10000);
            let for_slp_ops = BchDockerOps::from_ticker("FORSLP");
            crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&for_slp_ops, 4);
            for_slp_ops.initialize_slp();
            runner.hold(node);
        },
        DockerTestMode::ComposeInit => {
            setup_utxo_conf_for_compose("FORSLP", KDF_FORSLP_SERVICE);
            let for_slp_ops = BchDockerOps::from_ticker("FORSLP");
            crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&for_slp_ops, 4);
            for_slp_ops.initialize_slp();
        },
    }
}
