//! Docker tests custom runner (split from the old monolithic `runner.rs`).
//!
//! Public API is preserved:
//! - `docker_tests::runner::docker_tests_runner_impl()`
//!
//! Internals are split into per-chain setup submodules to push cfg gates to module boundaries.

use std::any::Any;
use std::env;
use std::io::{BufRead, BufReader};
use std::process::Command;
use test::{test_main, StaticBenchFn, StaticTestFn, TestDescAndFn};

// =============================================================================
// Per-chain setup submodules
// =============================================================================

#[cfg(any(
    feature = "docker-tests-swaps",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia",
    feature = "docker-tests-integration"
))]
mod utxo;

#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
mod slp;

#[cfg(feature = "docker-tests-qrc20")]
mod qtum;

#[cfg(any(
    feature = "docker-tests-eth",
    feature = "docker-tests-watchers-eth",
    feature = "docker-tests-integration"
))]
mod geth;

#[cfg(feature = "docker-tests-zcoin")]
mod zcoin;

#[cfg(any(feature = "docker-tests-tendermint", feature = "docker-tests-integration"))]
mod tendermint;

#[cfg(feature = "docker-tests-sia")]
mod sia;

// =============================================================================
// Core runner types
// =============================================================================

/// Execution mode for docker tests.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum DockerTestMode {
    /// Default: Start containers via testcontainers, run initialization.
    Testcontainers,
    /// Docker-compose mode: Containers already running, run initialization.
    ComposeInit,
}

/// Environment variable to indicate docker-compose mode (containers already running).
const ENV_DOCKER_COMPOSE_MODE: &str = "KDF_DOCKER_COMPOSE_ENV";

/// Determine which execution mode to use based on environment variables.
fn determine_test_mode() -> DockerTestMode {
    if env::var(ENV_DOCKER_COMPOSE_MODE).is_ok() {
        DockerTestMode::ComposeInit
    } else {
        DockerTestMode::Testcontainers
    }
}

/// Parses runner config from env once.
pub(crate) struct DockerTestConfig {
    pub(crate) mode: DockerTestMode,
    /// When `_MM2_TEST_CONF` is set, the runner must skip docker setup entirely.
    pub(crate) skip_setup: bool,
}

impl DockerTestConfig {
    fn from_env() -> Self {
        DockerTestConfig {
            mode: determine_test_mode(),
            skip_setup: env::var("_MM2_TEST_CONF").is_ok(),
        }
    }
}

/// Stateful docker test runner holding container keep-alives.
///
/// Keep-alives are stored as `Box<dyn Any>` to ensure RAII drop only happens
/// after `test_main` returns.
pub(crate) struct DockerTestRunner {
    pub(crate) config: DockerTestConfig,
    pub(crate) keep_alive: Vec<Box<dyn Any>>,
}

impl DockerTestRunner {
    fn new(config: DockerTestConfig) -> Self {
        DockerTestRunner {
            config,
            keep_alive: Vec::new(),
        }
    }

    pub(crate) fn hold<T: Any>(&mut self, container: T) {
        self.keep_alive.push(Box::new(container));
    }

    pub(crate) fn is_testcontainers(&self) -> bool {
        self.config.mode == DockerTestMode::Testcontainers
    }

    fn setup_or_reuse_nodes(&mut self) {
        if self.is_testcontainers() {
            for image in required_images() {
                pull_docker_image(image);
                remove_docker_containers(image);
            }
        }

        #[cfg(any(
            feature = "docker-tests-swaps",
            feature = "docker-tests-ordermatch",
            feature = "docker-tests-watchers",
            feature = "docker-tests-qrc20",
            feature = "docker-tests-sia",
            feature = "docker-tests-integration"
        ))]
        utxo::setup(self);

        #[cfg(feature = "docker-tests-qrc20")]
        qtum::setup(self);

        #[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
        slp::setup(self);

        #[cfg(any(
            feature = "docker-tests-eth",
            feature = "docker-tests-watchers-eth",
            feature = "docker-tests-integration"
        ))]
        geth::setup(self);

        #[cfg(feature = "docker-tests-zcoin")]
        zcoin::setup(self);

        #[cfg(any(feature = "docker-tests-tendermint", feature = "docker-tests-integration"))]
        tendermint::setup(self);

        #[cfg(feature = "docker-tests-sia")]
        sia::setup(self);
    }

    fn run_tests(&mut self, tests: &[&TestDescAndFn]) {
        let owned_tests: Vec<_> = tests
            .iter()
            .map(|t| match t.testfn {
                StaticTestFn(f) => TestDescAndFn {
                    testfn: StaticTestFn(f),
                    desc: t.desc.clone(),
                },
                StaticBenchFn(f) => TestDescAndFn {
                    testfn: StaticBenchFn(f),
                    desc: t.desc.clone(),
                },
                _ => panic!("non-static tests passed to lp_coins test runner"),
            })
            .collect();

        let args: Vec<String> = env::args().collect();
        test_main(&args, owned_tests, None);
    }
}

/// Public API: custom test runner implementation called by `docker_tests_main.rs`.
pub fn docker_tests_runner_impl(tests: &[&TestDescAndFn]) {
    let config = DockerTestConfig::from_env();
    log!("Docker test mode: {:?}", config.mode);

    let mut runner = DockerTestRunner::new(config);

    if !runner.config.skip_setup {
        runner.setup_or_reuse_nodes();
    }

    runner.run_tests(tests);
}

// =============================================================================
// Images + docker utility functions
// =============================================================================

fn required_images() -> Vec<&'static str> {
    let mut images = Vec::new();

    #[cfg(any(
        feature = "docker-tests-swaps",
        feature = "docker-tests-ordermatch",
        feature = "docker-tests-watchers",
        feature = "docker-tests-qrc20",
        feature = "docker-tests-sia",
        feature = "docker-tests-integration"
    ))]
    {
        use crate::docker_tests::helpers::utxo::UTXO_ASSET_DOCKER_IMAGE_WITH_TAG;
        images.push(UTXO_ASSET_DOCKER_IMAGE_WITH_TAG);
    }

    #[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
    {
        use crate::docker_tests::helpers::slp::FORSLP_IMAGE_WITH_TAG;
        images.push(FORSLP_IMAGE_WITH_TAG);
    }

    #[cfg(feature = "docker-tests-qrc20")]
    {
        use crate::docker_tests::helpers::qrc20::QTUM_REGTEST_DOCKER_IMAGE_WITH_TAG;
        images.push(QTUM_REGTEST_DOCKER_IMAGE_WITH_TAG);
    }

    #[cfg(any(
        feature = "docker-tests-eth",
        feature = "docker-tests-watchers-eth",
        feature = "docker-tests-integration"
    ))]
    {
        use crate::docker_tests::helpers::eth::GETH_DOCKER_IMAGE_WITH_TAG;
        images.push(GETH_DOCKER_IMAGE_WITH_TAG);
    }

    #[cfg(any(feature = "docker-tests-tendermint", feature = "docker-tests-integration"))]
    {
        use crate::docker_tests::helpers::tendermint::{
            ATOM_IMAGE_WITH_TAG, IBC_RELAYER_IMAGE_WITH_TAG, NUCLEUS_IMAGE,
        };
        images.push(NUCLEUS_IMAGE);
        images.push(ATOM_IMAGE_WITH_TAG);
        images.push(IBC_RELAYER_IMAGE_WITH_TAG);
    }

    #[cfg(feature = "docker-tests-zcoin")]
    {
        use crate::docker_tests::helpers::zcoin::ZOMBIE_ASSET_DOCKER_IMAGE_WITH_TAG;
        images.push(ZOMBIE_ASSET_DOCKER_IMAGE_WITH_TAG);
    }

    #[cfg(feature = "docker-tests-sia")]
    {
        use crate::docker_tests::helpers::sia::SIA_DOCKER_IMAGE_WITH_TAG;
        images.push(SIA_DOCKER_IMAGE_WITH_TAG);
    }

    images.sort_unstable();
    images.dedup();
    images
}

fn pull_docker_image(name: &str) {
    Command::new("docker")
        .arg("pull")
        .arg(name)
        .status()
        .expect("Failed to execute docker command");
}

fn remove_docker_containers(name: &str) {
    let stdout = Command::new("docker")
        .arg("ps")
        .arg("-f")
        .arg(format!("ancestor={name}"))
        .arg("-q")
        .output()
        .expect("Failed to execute docker command");

    let reader = BufReader::new(stdout.stdout.as_slice());
    let ids: Vec<_> = reader.lines().map(|line| line.unwrap()).collect();
    if !ids.is_empty() {
        Command::new("docker")
            .arg("rm")
            .arg("-f")
            .args(ids)
            .status()
            .expect("Failed to execute docker command");
    }
}
