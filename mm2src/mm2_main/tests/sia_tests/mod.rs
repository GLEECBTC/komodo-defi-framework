/// These modules are feature gated behind "run-sia-functional-tests" feature.
/// Each module must be run individually.
/// Eg,
/// cargo test -p mm2_main --features enable-sia,run-sia-functional-tests -- sia_tests::docker_functional_tests
/// cargo test -p mm2_main --features enable-sia,run-sia-functional-tests -- sia_tests::short_locktime_tests
mod docker_functional_tests;

/// This module is a temporary hack to allow grouping the relevant tests together via `cargo test` commands.
/// See doc comment inside short_locktime_tests.rs for more details.
mod short_locktime_tests;

pub(crate) mod utils;
