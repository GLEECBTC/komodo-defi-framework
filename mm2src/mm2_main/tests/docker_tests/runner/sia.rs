//! Sia setup for docker tests.

use super::{DockerTestMode, DockerTestRunner};

use common::block_on;

use crate::docker_tests::helpers::sia::sia_docker_node;
use crate::sia_tests::utils::wait_for_dsia_node_ready;

pub(super) fn setup(runner: &mut DockerTestRunner) {
    match runner.config.mode {
        DockerTestMode::Testcontainers => {
            let node = sia_docker_node("SIA", 9980);
            block_on(wait_for_dsia_node_ready());
            runner.hold(node);
        },
        DockerTestMode::ComposeInit => {
            block_on(wait_for_dsia_node_ready());
        },
    }
}
