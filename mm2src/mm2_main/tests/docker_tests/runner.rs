// block_on - only used in setup_sia
#[cfg(feature = "docker-tests-sia")]
use common::block_on;
use std::any::Any;
use std::env;
use std::io::{BufRead, BufReader};
#[cfg(any(feature = "docker-tests-tendermint", feature = "docker-tests-integration"))]
use std::path::PathBuf;
use std::process::Command;
// thread and Duration - only used in setup_cosmos for thread::sleep
#[cfg(any(feature = "docker-tests-tendermint", feature = "docker-tests-integration"))]
use std::thread;
#[cfg(any(feature = "docker-tests-tendermint", feature = "docker-tests-integration"))]
use std::time::Duration;
use test::{test_main, StaticBenchFn, StaticTestFn, TestDescAndFn};

// UTXO imports - needed for UTXO-based test features
// Note: CoinDockerOps trait is accessed via UFCS to avoid unused import warnings

// KDF_MYCOIN_SERVICE - needed by setup_utxo for compose mode
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia",
    feature = "docker-tests-integration"
))]
use crate::docker_tests::helpers::env::KDF_MYCOIN_SERVICE;

// KDF_MYCOIN1_SERVICE - only needed by features that use MYCOIN1 (not Sia)
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-integration"
))]
use crate::docker_tests::helpers::env::KDF_MYCOIN1_SERVICE;

// UTXO docker image and utxo_asset_docker_node - used for MYCOIN/MYCOIN1/FORSLP setup
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia",
    feature = "docker-tests-slp",
    feature = "docker-tests-integration"
))]
use crate::docker_tests::helpers::utxo::{utxo_asset_docker_node, UTXO_ASSET_DOCKER_IMAGE_WITH_TAG};

// setup_utxo_conf_for_compose - used by setup_utxo and setup_slp in compose mode
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia",
    feature = "docker-tests-slp",
    feature = "docker-tests-zcoin",
    feature = "docker-tests-integration"
))]
use crate::docker_tests::helpers::docker_ops::setup_utxo_conf_for_compose;

// UtxoAssetDockerOps - only needed by features that call setup_utxo
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia",
    feature = "docker-tests-integration"
))]
use crate::docker_tests::helpers::utxo::UtxoAssetDockerOps;

// SLP imports
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
use crate::docker_tests::helpers::env::KDF_FORSLP_SERVICE;
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
use crate::docker_tests::helpers::utxo::BchDockerOps;

// QRC20 imports
#[cfg(feature = "docker-tests-qrc20")]
use crate::docker_tests::helpers::qrc20::{
    qick_token_address, qorty_token_address, qrc20_swap_contract_address, qtum_conf_path, qtum_docker_node,
    setup_qtum_conf_for_compose, QtumDockerOps, QTUM_REGTEST_DOCKER_IMAGE_WITH_TAG,
};

// ETH imports
#[cfg(any(
    feature = "docker-tests-eth",
    feature = "docker-tests-watchers-eth",
    feature = "docker-tests-integration"
))]
use crate::docker_tests::helpers::eth::{
    erc20_contract, geth_account, geth_docker_node, geth_erc1155_contract, geth_erc721_contract, geth_maker_swap_v2,
    geth_nft_maker_swap_v2, geth_taker_swap_v2, init_geth_node, swap_contract, wait_for_geth_node_ready,
    watchers_swap_contract, GETH_DOCKER_IMAGE_WITH_TAG,
};

// Tendermint imports
#[cfg(any(feature = "docker-tests-tendermint", feature = "docker-tests-integration"))]
use crate::docker_tests::helpers::tendermint::{
    atom_node, ibc_relayer_node, nucleus_node, prepare_ibc_channels, prepare_ibc_channels_compose,
    wait_until_relayer_container_is_ready, wait_until_relayer_container_is_ready_compose, ATOM_IMAGE_WITH_TAG,
    IBC_RELAYER_IMAGE_WITH_TAG, NUCLEUS_IMAGE,
};

// ZCoin imports
#[cfg(feature = "docker-tests-zcoin")]
use crate::docker_tests::helpers::env::KDF_ZOMBIE_SERVICE;
#[cfg(feature = "docker-tests-zcoin")]
use crate::docker_tests::helpers::zcoin::{
    zombie_asset_docker_node, ZCoinAssetDockerOps, ZOMBIE_ASSET_DOCKER_IMAGE_WITH_TAG,
};

// Sia imports
#[cfg(feature = "docker-tests-sia")]
use crate::docker_tests::helpers::sia::{sia_docker_node, SIA_DOCKER_IMAGE_WITH_TAG};
#[cfg(feature = "docker-tests-sia")]
use crate::sia_tests::utils::wait_for_dsia_node_ready;

/// Execution mode for docker tests
#[derive(Debug, Clone, Copy, PartialEq)]
enum DockerTestMode {
    /// Default: Start containers via testcontainers, run initialization
    Testcontainers,
    /// Docker-compose mode: Containers already running, run initialization
    ComposeInit,
}

/// Environment variable to indicate docker-compose mode (containers already running)
const ENV_DOCKER_COMPOSE_MODE: &str = "KDF_DOCKER_COMPOSE_ENV";

/// Determine which execution mode to use based on environment variables
fn determine_test_mode() -> DockerTestMode {
    if env::var(ENV_DOCKER_COMPOSE_MODE).is_ok() {
        DockerTestMode::ComposeInit
    } else {
        DockerTestMode::Testcontainers
    }
}

/// Parses runner config from env once.
struct DockerTestConfig {
    mode: DockerTestMode,
    /// When `_MM2_TEST_CONF` is set, the runner must skip docker setup entirely.
    skip_setup: bool,
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
struct DockerTestRunner {
    config: DockerTestConfig,
    keep_alive: Vec<Box<dyn Any>>,
}

impl DockerTestRunner {
    fn new(config: DockerTestConfig) -> Self {
        DockerTestRunner {
            config,
            keep_alive: Vec::new(),
        }
    }

    fn hold<T: Any>(&mut self, container: T) {
        self.keep_alive.push(Box::new(container));
    }

    fn is_testcontainers(&self) -> bool {
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
            feature = "docker-tests-swaps-utxo",
            feature = "docker-tests-ordermatch",
            feature = "docker-tests-watchers",
            feature = "docker-tests-qrc20",
            feature = "docker-tests-sia",
            feature = "docker-tests-integration"
        ))]
        self.setup_utxo();
        #[cfg(feature = "docker-tests-qrc20")]
        self.setup_qtum();
        #[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
        self.setup_slp();
        #[cfg(any(
            feature = "docker-tests-eth",
            feature = "docker-tests-watchers-eth",
            feature = "docker-tests-integration"
        ))]
        self.setup_geth();
        #[cfg(feature = "docker-tests-zcoin")]
        self.setup_zombie();
        #[cfg(any(feature = "docker-tests-tendermint", feature = "docker-tests-integration"))]
        self.setup_cosmos();
        #[cfg(feature = "docker-tests-sia")]
        self.setup_sia();
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

    #[cfg(any(
        feature = "docker-tests-swaps-utxo",
        feature = "docker-tests-ordermatch",
        feature = "docker-tests-watchers",
        feature = "docker-tests-qrc20",
        feature = "docker-tests-sia",
        feature = "docker-tests-integration"
    ))]
    fn setup_utxo(&mut self) {
        // MYCOIN
        match self.config.mode {
            DockerTestMode::Testcontainers => {
                let node = utxo_asset_docker_node("MYCOIN", 8000);
                let utxo_ops = UtxoAssetDockerOps::from_ticker("MYCOIN");
                crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&utxo_ops, 4);
                self.hold(node);
            },
            DockerTestMode::ComposeInit => {
                setup_utxo_conf_for_compose("MYCOIN", KDF_MYCOIN_SERVICE);
                let utxo_ops = UtxoAssetDockerOps::from_ticker("MYCOIN");
                crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&utxo_ops, 4);
            },
        }

        // MYCOIN1 (only for utxo pair tests)
        #[cfg(any(
            feature = "docker-tests-swaps-utxo",
            feature = "docker-tests-ordermatch",
            feature = "docker-tests-watchers",
            feature = "docker-tests-qrc20",
            feature = "docker-tests-integration"
        ))]
        {
            match self.config.mode {
                DockerTestMode::Testcontainers => {
                    let node = utxo_asset_docker_node("MYCOIN1", 8001);
                    let utxo_ops1 = UtxoAssetDockerOps::from_ticker("MYCOIN1");
                    crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&utxo_ops1, 4);
                    self.hold(node);
                },
                DockerTestMode::ComposeInit => {
                    setup_utxo_conf_for_compose("MYCOIN1", KDF_MYCOIN1_SERVICE);
                    let utxo_ops1 = UtxoAssetDockerOps::from_ticker("MYCOIN1");
                    crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&utxo_ops1, 4);
                },
            }
        }
    }

    #[cfg(feature = "docker-tests-qrc20")]
    fn setup_qtum(&mut self) {
        match self.config.mode {
            DockerTestMode::Testcontainers => {
                let node = qtum_docker_node(9000);
                let qtum_ops = QtumDockerOps::new();
                crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&qtum_ops, 2);
                qtum_ops.initialize_contracts();
                self.hold(node);
            },
            DockerTestMode::ComposeInit => {
                setup_qtum_conf_for_compose();
                let qtum_ops = QtumDockerOps::new();
                crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&qtum_ops, 2);
                qtum_ops.initialize_contracts();
            },
        }

        // Ensure globals are initialized for test helpers
        let _ = qtum_conf_path().clone();
        let _ = qick_token_address();
        let _ = qorty_token_address();
        let _ = qrc20_swap_contract_address();
    }

    #[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
    fn setup_slp(&mut self) {
        match self.config.mode {
            DockerTestMode::Testcontainers => {
                let node = utxo_asset_docker_node("FORSLP", 10000);
                let for_slp_ops = BchDockerOps::from_ticker("FORSLP");
                crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&for_slp_ops, 4);
                for_slp_ops.initialize_slp();
                self.hold(node);
            },
            DockerTestMode::ComposeInit => {
                setup_utxo_conf_for_compose("FORSLP", KDF_FORSLP_SERVICE);
                let for_slp_ops = BchDockerOps::from_ticker("FORSLP");
                crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&for_slp_ops, 4);
                for_slp_ops.initialize_slp();
            },
        }
    }

    #[cfg(any(
        feature = "docker-tests-eth",
        feature = "docker-tests-watchers-eth",
        feature = "docker-tests-integration"
    ))]
    fn setup_geth(&mut self) {
        match self.config.mode {
            DockerTestMode::Testcontainers => {
                let node = geth_docker_node("ETH", 8545);
                wait_for_geth_node_ready();
                init_geth_node();
                self.hold(node);
            },
            DockerTestMode::ComposeInit => {
                wait_for_geth_node_ready();
                init_geth_node();
            },
        }

        // Ensure globals are initialized for test helpers
        let _ = geth_account();
        let _ = erc20_contract();
        let _ = swap_contract();
        let _ = geth_maker_swap_v2();
        let _ = geth_taker_swap_v2();
        let _ = watchers_swap_contract();
        let _ = geth_erc721_contract();
        let _ = geth_erc1155_contract();
        let _ = geth_nft_maker_swap_v2();
    }

    #[cfg(feature = "docker-tests-zcoin")]
    fn setup_zombie(&mut self) {
        match self.config.mode {
            DockerTestMode::Testcontainers => {
                let node = zombie_asset_docker_node(7090);
                let zombie_ops = ZCoinAssetDockerOps::new();
                crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&zombie_ops, 4);
                self.hold(node);
            },
            DockerTestMode::ComposeInit => {
                setup_utxo_conf_for_compose("ZOMBIE", KDF_ZOMBIE_SERVICE);
                let zombie_ops = ZCoinAssetDockerOps::new();
                crate::docker_tests::helpers::docker_ops::CoinDockerOps::wait_ready(&zombie_ops, 4);
            },
        }
    }

    #[cfg(any(feature = "docker-tests-tendermint", feature = "docker-tests-integration"))]
    fn setup_cosmos(&mut self) {
        match self.config.mode {
            DockerTestMode::Testcontainers => {
                let runtime_dir = prepare_runtime_dir().unwrap();

                let nucleus_node_instance = nucleus_node(runtime_dir.clone());
                let atom_node_instance = atom_node(runtime_dir.clone());
                let ibc_relayer_node_instance = ibc_relayer_node(runtime_dir.clone());

                prepare_ibc_channels(ibc_relayer_node_instance.container.id());
                thread::sleep(Duration::from_secs(10));
                wait_until_relayer_container_is_ready(ibc_relayer_node_instance.container.id());

                self.hold(nucleus_node_instance);
                self.hold(atom_node_instance);
                self.hold(ibc_relayer_node_instance);
            },
            DockerTestMode::ComposeInit => {
                let _runtime_dir = get_runtime_dir();

                prepare_ibc_channels_compose();
                thread::sleep(Duration::from_secs(10));
                wait_until_relayer_container_is_ready_compose();
            },
        }
    }

    #[cfg(feature = "docker-tests-sia")]
    fn setup_sia(&mut self) {
        match self.config.mode {
            DockerTestMode::Testcontainers => {
                let node = sia_docker_node("SIA", 9980);
                block_on(wait_for_dsia_node_ready());
                self.hold(node);
            },
            DockerTestMode::ComposeInit => {
                block_on(wait_for_dsia_node_ready());
            },
        }
    }
}

/// Public API: custom test runner implementation called by `docker_tests_main.rs`.
pub fn docker_tests_runner_impl(tests: &[&TestDescAndFn]) {
    // pretty_env_logger::try_init();
    let config = DockerTestConfig::from_env();
    log!("Docker test mode: {:?}", config.mode);

    let mut runner = DockerTestRunner::new(config);

    if !runner.config.skip_setup {
        runner.setup_or_reuse_nodes();
    }

    runner.run_tests(tests);
}

fn required_images() -> Vec<&'static str> {
    let mut images = Vec::new();

    #[cfg(any(
        feature = "docker-tests-swaps-utxo",
        feature = "docker-tests-ordermatch",
        feature = "docker-tests-watchers",
        feature = "docker-tests-qrc20",
        feature = "docker-tests-sia",
        feature = "docker-tests-slp",
        feature = "docker-tests-integration"
    ))]
    images.push(UTXO_ASSET_DOCKER_IMAGE_WITH_TAG);

    #[cfg(feature = "docker-tests-qrc20")]
    images.push(QTUM_REGTEST_DOCKER_IMAGE_WITH_TAG);

    #[cfg(any(
        feature = "docker-tests-eth",
        feature = "docker-tests-watchers-eth",
        feature = "docker-tests-integration"
    ))]
    images.push(GETH_DOCKER_IMAGE_WITH_TAG);

    #[cfg(any(feature = "docker-tests-tendermint", feature = "docker-tests-integration"))]
    {
        images.push(NUCLEUS_IMAGE);
        images.push(ATOM_IMAGE_WITH_TAG);
        images.push(IBC_RELAYER_IMAGE_WITH_TAG);
    }

    #[cfg(feature = "docker-tests-zcoin")]
    images.push(ZOMBIE_ASSET_DOCKER_IMAGE_WITH_TAG);

    #[cfg(feature = "docker-tests-sia")]
    images.push(SIA_DOCKER_IMAGE_WITH_TAG);

    images.sort_unstable();
    images.dedup();
    images
}

/// Get the runtime directory path
#[cfg(any(feature = "docker-tests-tendermint", feature = "docker-tests-integration"))]
fn get_runtime_dir() -> PathBuf {
    let project_root = {
        let mut current_dir = std::env::current_dir().unwrap();
        current_dir.pop();
        current_dir.pop();
        current_dir
    };
    project_root.join(".docker/container-runtime")
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

#[cfg(any(feature = "docker-tests-tendermint", feature = "docker-tests-integration"))]
fn prepare_runtime_dir() -> std::io::Result<PathBuf> {
    let project_root = {
        let mut current_dir = std::env::current_dir().unwrap();
        current_dir.pop();
        current_dir.pop();
        current_dir
    };

    let containers_state_dir = project_root.join(".docker/container-state");
    assert!(containers_state_dir.exists());
    let containers_runtime_dir = project_root.join(".docker/container-runtime");

    if containers_runtime_dir.exists() {
        std::fs::remove_dir_all(&containers_runtime_dir).unwrap();
    }

    mm2_io::fs::copy_dir_all(&containers_state_dir, &containers_runtime_dir).unwrap();

    Ok(containers_runtime_dir)
}
