//! Qtum/QRC20 setup for docker tests.

use super::{DockerTestMode, DockerTestRunner};

use crate::docker_tests::helpers::qrc20::{
    qick_token_address, qorty_token_address, qrc20_swap_contract_address, qtum_conf_path, qtum_docker_node,
    setup_qtum_conf_for_compose, QtumDockerOps,
};

pub(super) fn setup(runner: &mut DockerTestRunner) {
    match runner.config.mode {
        DockerTestMode::Testcontainers => {
            let node = qtum_docker_node(9000);
            let qtum_ops = QtumDockerOps::new();
            crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&qtum_ops, 2);
            qtum_ops.initialize_contracts();
            runner.hold(node);
        },
        DockerTestMode::ComposeInit => {
            setup_qtum_conf_for_compose();
            let qtum_ops = QtumDockerOps::new();
            crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&qtum_ops, 2);
            qtum_ops.initialize_contracts();
        },
    }

    // Ensure globals are initialized for test helpers.
    let _ = qtum_conf_path().clone();
    let _ = qick_token_address();
    let _ = qorty_token_address();
    let _ = qrc20_swap_contract_address();
}
