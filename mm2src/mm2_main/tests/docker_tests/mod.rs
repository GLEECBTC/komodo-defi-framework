#![allow(static_mut_refs)]
pub mod docker_tests_common;

mod docker_ordermatch_tests;
mod docker_tests_inner;
mod eth_docker_tests;
pub mod qrc20_tests;
#[cfg(feature = "enable-sia")]
mod sia_docker_tests;
mod slp_tests;
mod swap_proto_v2_tests;
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
