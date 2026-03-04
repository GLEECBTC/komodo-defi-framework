//! Geth/ETH setup for docker tests.

use super::{DockerTestMode, DockerTestRunner};

use crate::docker_tests::helpers::eth::{
    erc20_contract, geth_account, geth_docker_node, geth_erc1155_contract, geth_erc721_contract, geth_maker_swap_v2,
    geth_nft_maker_swap_v2, geth_taker_swap_v2, geth_usdt_contract, init_geth_node, swap_contract,
    wait_for_geth_node_ready, watchers_swap_contract,
};

pub(super) fn setup(runner: &mut DockerTestRunner) {
    match runner.config.mode {
        DockerTestMode::Testcontainers => {
            let node = geth_docker_node("ETH", 8545);
            wait_for_geth_node_ready();
            init_geth_node();
            runner.hold(node);
        },
        DockerTestMode::ComposeInit => {
            wait_for_geth_node_ready();
            init_geth_node();
        },
    }

    // Ensure globals are initialized for test helpers.
    let _ = geth_account();
    let _ = erc20_contract();
    let _ = swap_contract();
    let _ = geth_maker_swap_v2();
    let _ = geth_taker_swap_v2();
    let _ = watchers_swap_contract();
    let _ = geth_erc721_contract();
    let _ = geth_erc1155_contract();
    let _ = geth_nft_maker_swap_v2();
    let _ = geth_usdt_contract();
}
