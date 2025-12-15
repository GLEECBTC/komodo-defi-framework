#![cfg(feature = "run-docker-tests")]
#![cfg(not(target_arch = "wasm32"))]
#![feature(custom_test_frameworks)]
#![feature(test)]
#![test_runner(docker_tests_runner)]

#[cfg(test)]
#[macro_use]
extern crate common;
#[cfg(all(test, feature = "docker-tests-qrc20"))]
#[macro_use]
extern crate gstuff;
#[cfg(test)]
#[macro_use]
extern crate lazy_static;
// serde_json macro_use: only for features whose test files don't have explicit `use serde_json::json`
// tendermint tests have explicit imports so don't need this
#[cfg(all(
    test,
    any(
        feature = "docker-tests-swaps-utxo",
        feature = "docker-tests-ordermatch",
        feature = "docker-tests-watchers",
        feature = "docker-tests-qrc20",
        feature = "docker-tests-eth",
        feature = "docker-tests-slp",
        feature = "docker-tests-zcoin",
        feature = "docker-tests-sia",
        feature = "docker-tests-integration"
    )
))]
#[macro_use]
extern crate serde_json;
#[cfg(test)]
extern crate ser_error_derive;
#[cfg(test)]
extern crate test;

use test::TestDescAndFn;

mod docker_tests;

// Sia tests are gated on docker-tests-sia feature to prevent them from running in other docker test jobs
#[cfg(feature = "docker-tests-sia")]
mod sia_tests;

#[allow(dead_code)]
mod integration_tests_common;

// AP: custom test runner is intended to initialize the required environment (e.g. coin daemons in the docker containers)
// and then gracefully clear it by dropping the RAII docker container handlers
// I've tried to use static for such singleton initialization but it turned out that despite
// rustc allows to use Drop as static the drop fn won't ever be called
// NB: https://github.com/rust-lang/rfcs/issues/1111
// the only preparation step required is Zcash params files downloading:
// Windows - https://github.com/KomodoPlatform/komodo/blob/master/zcutil/fetch-params.bat
// Linux and MacOS - https://github.com/KomodoPlatform/komodo/blob/master/zcutil/fetch-params.sh
pub fn docker_tests_runner(tests: &[&TestDescAndFn]) {
    docker_tests::runner::docker_tests_runner_impl(tests)
}
