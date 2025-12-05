#![allow(static_mut_refs)]
pub mod docker_env_metadata;
pub mod docker_tests_common;
pub mod helpers;

mod docker_ordermatch_tests;
mod docker_tests_inner;
#[cfg(feature = "docker-tests-eth")]
mod eth_docker_tests;
pub mod qrc20_tests;
#[cfg(feature = "docker-tests-sia")]
mod sia_docker_tests;
#[cfg(feature = "docker-tests-slp")]
mod slp_tests;
// Cross-chain swap tests - run only in main docker-tests job
// Excluded from chain-specific jobs to avoid running with insufficient nodes
mod swap_proto_v2_tests;
#[cfg(all(
    feature = "run-docker-tests",
    not(feature = "docker-tests-slp"),
    not(feature = "docker-tests-sia"),
    not(feature = "docker-tests-eth")
))]
mod swap_tests;
mod swap_watcher_tests;
mod swaps_confs_settings_sync_tests;
mod swaps_file_lock_tests;
mod tendermint_tests;
mod z_coin_docker_tests;

// dummy test helping IDE to recognize this as test module
#[test]
#[allow(clippy::assertions_on_constants)]
fn dummy() {
    assert!(true)
}
