use common::custom_futures::timeout::FutureTimerExt;
use common::{block_on, now_ms, wait_until_ms};
use std::any::Any;
use std::env;
use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::Duration;
use test::{test_main, StaticBenchFn, StaticTestFn, TestDescAndFn};
use web3::{transports::Http, Web3};

use crate::docker_tests::docker_env_metadata::{
    get_metadata_file_path, get_or_default_metadata_path, is_docker_compose_mode, should_load_metadata,
    CosmosNodeState, DockerEnvMetadata, GethNodeState, QtumNodeState, SlpNodeState, UtxoNodeState, ZombieNodeState,
};
use crate::docker_tests::helpers::docker_ops::CoinDockerOps;
use crate::docker_tests::helpers::env::{
    KDF_FORSLP_SERVICE, KDF_IBC_RELAYER_SERVICE, KDF_MYCOIN1_SERVICE, KDF_MYCOIN_SERVICE, KDF_QTUM_SERVICE,
    KDF_ZOMBIE_SERVICE,
};
use crate::docker_tests::helpers::eth::{
    erc20_contract, geth_account, geth_docker_node, geth_erc1155_contract, geth_erc721_contract, geth_maker_swap_v2,
    geth_nft_maker_swap_v2, geth_taker_swap_v2, init_geth_node, set_erc20_contract, set_geth_account,
    set_geth_erc1155_contract, set_geth_erc721_contract, set_geth_maker_swap_v2, set_geth_nft_maker_swap_v2,
    set_geth_taker_swap_v2, set_swap_contract, set_watchers_swap_contract, swap_contract, watchers_swap_contract,
    GETH_DOCKER_IMAGE_WITH_TAG, GETH_RPC_URL, GETH_WEB3,
};
use crate::docker_tests::helpers::qrc20::QtumDockerOps;
use crate::docker_tests::helpers::qrc20::{
    qick_token_address, qorty_token_address, qrc20_swap_contract_address, qtum_conf_path, set_qick_token_address,
    set_qorty_token_address, set_qrc20_swap_contract_address, set_qtum_conf_path,
};
use crate::docker_tests::helpers::qrc20::{qtum_docker_node, QTUM_REGTEST_DOCKER_IMAGE_WITH_TAG};
use crate::docker_tests::helpers::tendermint::{
    atom_node, ibc_relayer_node, nucleus_node, prepare_ibc_channels, wait_until_relayer_container_is_ready,
    ATOM_IMAGE_WITH_TAG, IBC_RELAYER_IMAGE_WITH_TAG, NUCLEUS_IMAGE,
};
use crate::docker_tests::helpers::utxo::{
    utxo_asset_docker_node, BchDockerOps, UtxoAssetDockerOps, SLP_TOKEN_ID, SLP_TOKEN_OWNERS,
    UTXO_ASSET_DOCKER_IMAGE_WITH_TAG,
};
use crate::docker_tests::helpers::zcoin::{
    zombie_asset_docker_node, ZCoinAssetDockerOps, ZOMBIE_ASSET_DOCKER_IMAGE_WITH_TAG,
};

#[cfg(feature = "docker-tests-sia")]
use crate::docker_tests::docker_env_metadata::SiaNodeState;
#[cfg(feature = "docker-tests-sia")]
use crate::docker_tests::helpers::sia::{sia_docker_node, SIA_DOCKER_IMAGE_WITH_TAG, SIA_RPC_PARAMS};
#[cfg(feature = "docker-tests-sia")]
use crate::sia_tests::utils::wait_for_dsia_node_ready;

/// Execution mode for docker tests
#[derive(Debug, Clone, Copy, PartialEq)]
enum DockerTestMode {
    /// Default: Start containers via testcontainers, run initialization
    Testcontainers,
    /// Docker-compose mode: Containers already running, run initialization, save metadata
    ComposeInit,
    /// Reuse mode: Load metadata, skip both container start and initialization
    ReuseMetadata,
}

/// Determine which execution mode to use based on environment variables
fn determine_test_mode() -> DockerTestMode {
    if should_load_metadata() {
        DockerTestMode::ReuseMetadata
    } else if is_docker_compose_mode() {
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

/// Stateful docker test runner holding metadata and container keep-alives.
///
/// Keep-alives are stored as `Box<dyn Any>` to ensure RAII drop only happens
/// after `test_main` returns.
struct DockerTestRunner {
    config: DockerTestConfig,
    metadata: DockerEnvMetadata,
    keep_alive: Vec<Box<dyn Any>>,
}

impl DockerTestRunner {
    fn new(config: DockerTestConfig) -> Self {
        DockerTestRunner {
            config,
            metadata: DockerEnvMetadata::new(),
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
        match self.config.mode {
            DockerTestMode::ReuseMetadata => {
                let metadata_path = get_metadata_file_path().expect("KDF_DOCKER_ENV_STATE_FILE must be set");
                let metadata =
                    DockerEnvMetadata::load(&metadata_path).expect("Failed to load docker environment metadata");

                if let Err(e) = validate_nodes_health(&metadata) {
                    panic!(
                        "Node health check failed: {}. Ensure containers are running or remove KDF_DOCKER_ENV_STATE_FILE to start fresh.",
                        e
                    );
                }

                load_metadata_into_globals(&metadata);
                self.metadata = metadata;
                log!("Loaded environment state from metadata, skipping container startup and initialization");
            },
            DockerTestMode::ComposeInit | DockerTestMode::Testcontainers => {
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
                    feature = "docker-tests-sia"
                ))]
                self.setup_utxo();
                #[cfg(feature = "docker-tests-qrc20")]
                self.setup_qtum();
                #[cfg(feature = "docker-tests-slp")]
                self.setup_slp();
                #[cfg(any(
                    feature = "docker-tests-eth",
                    feature = "docker-tests-ordermatch",
                    feature = "docker-tests-watchers-eth"
                ))]
                self.setup_geth();
                #[cfg(feature = "docker-tests-zcoin")]
                self.setup_zombie();
                #[cfg(feature = "docker-tests-tendermint")]
                self.setup_cosmos();
                #[cfg(feature = "docker-tests-sia")]
                self.setup_sia();

                if self.config.mode == DockerTestMode::ComposeInit {
                    let metadata_path = get_or_default_metadata_path();
                    if let Some(parent) = metadata_path.parent() {
                        std::fs::create_dir_all(parent).ok();
                    }
                    if let Err(e) = self.metadata.save(&metadata_path) {
                        log!("Warning: Failed to save docker environment metadata: {}", e);
                    } else {
                        log!("Saved docker environment metadata to {:?}", metadata_path);
                    }
                }
            },
        }
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
        feature = "docker-tests-sia"
    ))]
    fn setup_utxo(&mut self) {
        // MYCOIN
        match self.config.mode {
            DockerTestMode::Testcontainers => {
                let node = utxo_asset_docker_node("MYCOIN", 8000);
                let utxo_ops = UtxoAssetDockerOps::from_ticker("MYCOIN");
                utxo_ops.wait_ready(4);
                self.hold(node);
            },
            DockerTestMode::ComposeInit => {
                setup_utxo_conf_for_compose("MYCOIN", KDF_MYCOIN_SERVICE);
                let utxo_ops = UtxoAssetDockerOps::from_ticker("MYCOIN");
                utxo_ops.wait_ready(4);
            },
            DockerTestMode::ReuseMetadata => return,
        }

        // MYCOIN1 (only for utxo pair tests)
        #[cfg(any(
            feature = "docker-tests-swaps-utxo",
            feature = "docker-tests-ordermatch",
            feature = "docker-tests-watchers",
            feature = "docker-tests-qrc20"
        ))]
        {
            match self.config.mode {
                DockerTestMode::Testcontainers => {
                    let node = utxo_asset_docker_node("MYCOIN1", 8001);
                    let utxo_ops1 = UtxoAssetDockerOps::from_ticker("MYCOIN1");
                    utxo_ops1.wait_ready(4);
                    self.hold(node);
                },
                DockerTestMode::ComposeInit => {
                    setup_utxo_conf_for_compose("MYCOIN1", KDF_MYCOIN1_SERVICE);
                    let utxo_ops1 = UtxoAssetDockerOps::from_ticker("MYCOIN1");
                    utxo_ops1.wait_ready(4);
                },
                DockerTestMode::ReuseMetadata => {},
            }
        }

        self.metadata.initialized.utxo = true;

        // Store ports consistently for both modes (compose uses same ports)
        let mycoin_port = 8000;

        #[cfg(any(
            feature = "docker-tests-swaps-utxo",
            feature = "docker-tests-ordermatch",
            feature = "docker-tests-watchers",
            feature = "docker-tests-qrc20"
        ))]
        let mycoin1_port = 8001;
        #[cfg(not(any(
            feature = "docker-tests-swaps-utxo",
            feature = "docker-tests-ordermatch",
            feature = "docker-tests-watchers",
            feature = "docker-tests-qrc20"
        )))]
        let mycoin1_port = 0;

        self.metadata.utxo = Some(UtxoNodeState {
            mycoin_port,
            mycoin1_port,
        });
    }

    #[cfg(feature = "docker-tests-qrc20")]
    fn setup_qtum(&mut self) {
        match self.config.mode {
            DockerTestMode::Testcontainers => {
                let node = qtum_docker_node(9000);
                let qtum_ops = QtumDockerOps::new();
                qtum_ops.wait_ready(2);
                qtum_ops.initialize_contracts();
                self.hold(node);
            },
            DockerTestMode::ComposeInit => {
                setup_qtum_conf_for_compose();
                let qtum_ops = QtumDockerOps::new();
                qtum_ops.wait_ready(2);
                qtum_ops.initialize_contracts();
            },
            DockerTestMode::ReuseMetadata => return,
        }

        self.metadata.qtum = Some(QtumNodeState {
            port: 9000,
            conf_path: qtum_conf_path().clone(),
            qick_token_address: qick_token_address(),
            qorty_token_address: qorty_token_address(),
            swap_contract_address: qrc20_swap_contract_address(),
        });
        self.metadata.initialized.qtum = true;
    }

    #[cfg(feature = "docker-tests-slp")]
    fn setup_slp(&mut self) {
        match self.config.mode {
            DockerTestMode::Testcontainers => {
                let node = utxo_asset_docker_node("FORSLP", 10000);
                let for_slp_ops = BchDockerOps::from_ticker("FORSLP");
                for_slp_ops.wait_ready(4);
                for_slp_ops.initialize_slp();
                self.hold(node);
            },
            DockerTestMode::ComposeInit => {
                setup_utxo_conf_for_compose("FORSLP", KDF_FORSLP_SERVICE);
                let for_slp_ops = BchDockerOps::from_ticker("FORSLP");
                for_slp_ops.wait_ready(4);
                for_slp_ops.initialize_slp();
            },
            DockerTestMode::ReuseMetadata => return,
        }

        let token_id = *SLP_TOKEN_ID.lock().unwrap();
        let token_owners = SLP_TOKEN_OWNERS.lock().unwrap().clone();
        self.metadata.slp = Some(SlpNodeState {
            port: 10000,
            token_id,
            token_owners,
        });
        self.metadata.initialized.slp = true;
    }

    #[cfg(any(
        feature = "docker-tests-eth",
        feature = "docker-tests-ordermatch",
        feature = "docker-tests-watchers-eth"
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
            DockerTestMode::ReuseMetadata => return,
        }

        self.metadata.geth = Some(GethNodeState {
            rpc_url: GETH_RPC_URL.to_string(),
            account: geth_account(),
            erc20_contract: erc20_contract(),
            swap_contract: swap_contract(),
            maker_swap_v2: geth_maker_swap_v2(),
            taker_swap_v2: geth_taker_swap_v2(),
            watchers_swap_contract: watchers_swap_contract(),
            erc721_contract: geth_erc721_contract(),
            erc1155_contract: geth_erc1155_contract(),
            nft_maker_swap_v2: geth_nft_maker_swap_v2(),
        });
        self.metadata.initialized.geth = true;
    }

    #[cfg(feature = "docker-tests-zcoin")]
    fn setup_zombie(&mut self) {
        match self.config.mode {
            DockerTestMode::Testcontainers => {
                let node = zombie_asset_docker_node(7090);
                let zombie_ops = ZCoinAssetDockerOps::new();
                zombie_ops.wait_ready(4);
                self.hold(node);
            },
            DockerTestMode::ComposeInit => {
                setup_utxo_conf_for_compose("ZOMBIE", KDF_ZOMBIE_SERVICE);
                let zombie_ops = ZCoinAssetDockerOps::new();
                zombie_ops.wait_ready(4);
            },
            DockerTestMode::ReuseMetadata => return,
        }

        self.metadata.zombie = Some(ZombieNodeState {
            port: 7090,
            conf_path: coins::utxo::coin_daemon_data_dir("ZOMBIE", true).join("ZOMBIE.conf"),
        });
        self.metadata.initialized.zombie = true;
    }

    #[cfg(feature = "docker-tests-tendermint")]
    fn setup_cosmos(&mut self) {
        match self.config.mode {
            DockerTestMode::Testcontainers => {
                let runtime_dir = prepare_runtime_dir().unwrap();

                let nucleus_node_instance = nucleus_node(runtime_dir.clone());
                let atom_node_instance = atom_node(runtime_dir.clone());
                let ibc_relayer_node_instance = ibc_relayer_node(runtime_dir.clone());

                self.metadata.cosmos = Some(CosmosNodeState {
                    nucleus_rpc_url: "http://localhost:26657".to_string(),
                    atom_rpc_url: "http://localhost:26658".to_string(),
                    runtime_dir,
                    ibc_channels_ready: false,
                });

                prepare_ibc_channels(ibc_relayer_node_instance.container.id());
                thread::sleep(Duration::from_secs(10));
                wait_until_relayer_container_is_ready(ibc_relayer_node_instance.container.id());

                self.hold(nucleus_node_instance);
                self.hold(atom_node_instance);
                self.hold(ibc_relayer_node_instance);
            },
            DockerTestMode::ComposeInit => {
                let runtime_dir = get_runtime_dir();

                self.metadata.cosmos = Some(CosmosNodeState {
                    nucleus_rpc_url: "http://localhost:26657".to_string(),
                    atom_rpc_url: "http://localhost:26658".to_string(),
                    runtime_dir,
                    ibc_channels_ready: false,
                });

                prepare_ibc_channels_compose();
                thread::sleep(Duration::from_secs(10));
                wait_until_relayer_container_is_ready_compose();
            },
            DockerTestMode::ReuseMetadata => return,
        }

        if let Some(ref mut cosmos) = self.metadata.cosmos {
            cosmos.ibc_channels_ready = true;
        }
        self.metadata.initialized.cosmos = true;
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
            DockerTestMode::ReuseMetadata => return,
        }

        self.metadata.sia = Some(SiaNodeState {
            rpc_host: SIA_RPC_PARAMS.0.to_string(),
            rpc_port: SIA_RPC_PARAMS.1,
            rpc_password: SIA_RPC_PARAMS.2.to_string(),
            initialized: true,
        });
        self.metadata.initialized.sia = true;
    }
}

/// Public API: custom test runner implementation called by `docker_tests_main.rs`.
pub fn docker_tests_runner_impl(tests: &[&TestDescAndFn]) {
    // pretty_env_logger::try_init();
    let config = DockerTestConfig::from_env();
    log!("Docker test mode: {:?}", config.mode);

    let mut runner = DockerTestRunner::new(config);

    // Allow metadata reuse even when skip_setup is set (it only loads state, doesn't start containers)
    if !runner.config.skip_setup || runner.config.mode == DockerTestMode::ReuseMetadata {
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
        feature = "docker-tests-sia"
    ))]
    images.push(UTXO_ASSET_DOCKER_IMAGE_WITH_TAG);

    #[cfg(feature = "docker-tests-qrc20")]
    images.push(QTUM_REGTEST_DOCKER_IMAGE_WITH_TAG);

    #[cfg(any(
        feature = "docker-tests-eth",
        feature = "docker-tests-ordermatch",
        feature = "docker-tests-watchers-eth"
    ))]
    images.push(GETH_DOCKER_IMAGE_WITH_TAG);

    #[cfg(feature = "docker-tests-tendermint")]
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

/// Check that a Geth contract has deployed code at the given address.
///
/// This semantic check validates that the metadata's contract addresses actually
/// have bytecode deployed, catching stale metadata where containers were recreated
/// but contracts weren't re-deployed.
fn check_geth_contract_code(web3: &Web3<Http>, name: &str, address: ethereum_types::H160) -> Result<(), String> {
    match block_on(web3.eth().code(address, None).timeout(Duration::from_secs(3))) {
        Ok(Ok(code)) => {
            if code.0.is_empty() {
                return Err(format!(
                    "GETH {} contract has no deployed code at {:?}; metadata is stale. Re-run docker env init.",
                    name, address
                ));
            }
            log!("{} contract OK at {:?}", name, address);
            Ok(())
        },
        Ok(Err(e)) => Err(format!(
            "GETH {} contract code fetch failed at {:?}: {}",
            name, address, e
        )),
        Err(_) => Err(format!("GETH {} contract code fetch timed out at {:?}", name, address)),
    }
}

/// Validate that nodes are reachable before loading metadata
fn validate_nodes_health(metadata: &DockerEnvMetadata) -> Result<(), String> {
    log!("Validating node health from metadata...");

    // Check UTXO nodes (MYCOIN, MYCOIN1)
    if metadata.initialized.utxo {
        let utxo = metadata.utxo.as_ref().ok_or_else(|| {
            "UTXO marked initialized but UTXO state missing in metadata; re-run docker env init.".to_string()
        })?;

        for (name, port) in [("MYCOIN", utxo.mycoin_port), ("MYCOIN1", utxo.mycoin1_port)] {
            if port == 0 {
                continue;
            }
            let addr = format!("127.0.0.1:{}", port);
            if TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_secs(2)).is_err() {
                return Err(format!("{} node not reachable at {}", name, addr));
            }
            log!("  {} node OK at port {}", name, port);
        }
    }

    // Check Qtum node
    if metadata.initialized.qtum {
        if let Some(ref qtum) = metadata.qtum {
            let addr = format!("127.0.0.1:{}", qtum.port);
            if TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_secs(2)).is_err() {
                return Err(format!("QTUM node not reachable at {}", addr));
            }
            if !qtum.conf_path.exists() {
                return Err(format!(
                    "Qtum config missing at {}; metadata is stale. Re-run docker env init.",
                    qtum.conf_path.display()
                ));
            }
            log!("  QTUM node OK at port {}", qtum.port);
        }
    }

    // Check SLP node
    if metadata.initialized.slp {
        if let Some(ref slp) = metadata.slp {
            let addr = format!("127.0.0.1:{}", slp.port);
            if TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_secs(2)).is_err() {
                return Err(format!("FORSLP node not reachable at {}", addr));
            }
            log!("  FORSLP node OK at port {}", slp.port);
        }
    }

    // Check Geth node via web3 RPC
    if metadata.initialized.geth {
        let geth = metadata
            .geth
            .as_ref()
            .ok_or_else(|| "Geth RPC URL missing in metadata; re-run docker env init.".to_string())?;
        let transport = Http::new(&geth.rpc_url).map_err(|e| {
            format!(
                "Failed to create HTTP transport for Geth RPC URL '{}': {}",
                geth.rpc_url, e
            )
        })?;
        let web3 = Web3::new(transport);
        match block_on(web3.eth().block_number().timeout(Duration::from_secs(3))) {
            Ok(Ok(_)) => log!("  GETH node OK at {}", geth.rpc_url),
            _ => return Err(format!("GETH node not reachable at {}", geth.rpc_url)),
        }

        // Semantic checks: verify all contracts have deployed bytecode
        log!("  Verifying GETH contract deployments...");
        check_geth_contract_code(&web3, "erc20_contract", geth.erc20_contract)?;
        check_geth_contract_code(&web3, "swap_contract", geth.swap_contract)?;
        check_geth_contract_code(&web3, "maker_swap_v2", geth.maker_swap_v2)?;
        check_geth_contract_code(&web3, "taker_swap_v2", geth.taker_swap_v2)?;
        check_geth_contract_code(&web3, "watchers_swap_contract", geth.watchers_swap_contract)?;
        check_geth_contract_code(&web3, "erc721_contract", geth.erc721_contract)?;
        check_geth_contract_code(&web3, "erc1155_contract", geth.erc1155_contract)?;
        check_geth_contract_code(&web3, "nft_maker_swap_v2", geth.nft_maker_swap_v2)?;
    }

    // Check Zombie node
    if metadata.initialized.zombie {
        if let Some(ref zombie) = metadata.zombie {
            let addr = format!("127.0.0.1:{}", zombie.port);
            if TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_secs(2)).is_err() {
                return Err(format!("ZOMBIE node not reachable at {}", addr));
            }
            log!("  ZOMBIE node OK at port {}", zombie.port);
        }
    }

    // Check Cosmos nodes
    if metadata.initialized.cosmos {
        if let Some(ref cosmos) = metadata.cosmos {
            let addr = "127.0.0.1:26657";
            if TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_secs(2)).is_err() {
                return Err(format!("NUCLEUS node not reachable at {}", addr));
            }
            log!("  NUCLEUS node OK at {}", cosmos.nucleus_rpc_url);

            let addr = "127.0.0.1:26658";
            if TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_secs(2)).is_err() {
                return Err(format!("ATOM node not reachable at {}", addr));
            }
            log!("  ATOM node OK at {}", cosmos.atom_rpc_url);
        }
    }

    // Check Sia node (only when docker-tests-sia feature is enabled)
    #[cfg(feature = "docker-tests-sia")]
    if metadata.initialized.sia {
        if let Some(ref sia) = metadata.sia {
            let addr = format!("{}:{}", sia.rpc_host, sia.rpc_port);
            if TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_secs(2)).is_err() {
                return Err(format!("SIA node not reachable at {}", addr));
            }
            log!("  SIA node OK at {}:{}", sia.rpc_host, sia.rpc_port);
        }
    }

    log!("All nodes healthy!");
    Ok(())
}

/// Load metadata into global state variables
fn load_metadata_into_globals(metadata: &DockerEnvMetadata) {
    // Load Qtum state
    if let Some(ref qtum) = metadata.qtum {
        set_qtum_conf_path(qtum.conf_path.clone());
        set_qick_token_address(qtum.qick_token_address);
        set_qorty_token_address(qtum.qorty_token_address);
        set_qrc20_swap_contract_address(qtum.swap_contract_address);
    }

    // Load SLP state
    if let Some(ref slp) = metadata.slp {
        *SLP_TOKEN_ID.lock().unwrap() = slp.token_id;
        *SLP_TOKEN_OWNERS.lock().unwrap() = slp.token_owners.clone();
    }

    // Load Geth state
    if let Some(ref geth) = metadata.geth {
        set_geth_account(geth.account);
        set_erc20_contract(geth.erc20_contract);
        set_swap_contract(geth.swap_contract);
        set_geth_maker_swap_v2(geth.maker_swap_v2);
        set_geth_taker_swap_v2(geth.taker_swap_v2);
        set_watchers_swap_contract(geth.watchers_swap_contract);
        set_geth_erc721_contract(geth.erc721_contract);
        set_geth_erc1155_contract(geth.erc1155_contract);
        set_geth_nft_maker_swap_v2(geth.nft_maker_swap_v2);
    }

    log!("Loaded global state from metadata");
}

/// Set up QTUM_CONF_PATH for compose mode by copying config from the container
fn setup_qtum_conf_for_compose() {
    let mut conf_path = coins::utxo::coin_daemon_data_dir("qtum", false);
    std::fs::create_dir_all(&conf_path).unwrap();
    conf_path.push("qtum.conf");

    let container_id = resolve_compose_container_id(KDF_QTUM_SERVICE);

    Command::new("docker")
        .arg("cp")
        .arg(format!("{}:/data/node_0/qtum.conf", container_id))
        .arg(&conf_path)
        .status()
        .expect("Failed to copy Qtum config from compose container");

    let timeout = wait_until_ms(3000);
    loop {
        if conf_path.exists() {
            break;
        }
        assert!(now_ms() < timeout, "Timed out waiting for Qtum config");
    }

    set_qtum_conf_path(conf_path);
}

/// Set up UTXO coin config for compose mode by copying config from the container.
///
/// `service_name` is the docker-compose service name (e.g., "mycoin"), not the container name.
fn setup_utxo_conf_for_compose(ticker: &str, service_name: &str) {
    let mut conf_path = coins::utxo::coin_daemon_data_dir(ticker, true);
    std::fs::create_dir_all(&conf_path).unwrap();
    conf_path.push(format!("{ticker}.conf"));

    let container_id = resolve_compose_container_id(service_name);

    Command::new("docker")
        .arg("cp")
        .arg(format!("{container_id}:/data/node_0/{ticker}.conf"))
        .arg(&conf_path)
        .status()
        .expect("Failed to copy UTXO config from compose container");

    let timeout = wait_until_ms(3000);
    loop {
        if conf_path.exists() {
            break;
        }
        assert!(now_ms() < timeout, "Timed out waiting for {} config", ticker);
    }
}

/// Get the runtime directory path
fn get_runtime_dir() -> PathBuf {
    let project_root = {
        let mut current_dir = std::env::current_dir().unwrap();
        current_dir.pop();
        current_dir.pop();
        current_dir
    };
    project_root.join(".docker/container-runtime")
}

/// Find the container ID for a docker-compose service, independent of project name.
///
/// Uses label-based lookup (`com.docker.compose.service=<service>`) which works
/// regardless of project name or container_name settings.
fn resolve_compose_container_id(service_name: &str) -> String {
    let output = Command::new("docker")
        .args([
            "ps",
            "-q",
            "--filter",
            &format!("label=com.docker.compose.service={}", service_name),
            "--filter",
            "status=running",
        ])
        .output()
        .expect("failed to execute `docker ps`");

    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Some(container_id) = stdout.lines().next().map(str::trim).filter(|s| !s.is_empty()) {
        return container_id.to_string();
    }

    let fallback_name = format!("kdf-{}", service_name);
    let output = Command::new("docker")
        .args(["ps", "-q", "--filter", &format!("name={}", fallback_name)])
        .output()
        .expect("failed to execute `docker ps` (name filter)");

    let stdout = String::from_utf8_lossy(&output.stdout);
    if let Some(container_id) = stdout.lines().next().map(str::trim).filter(|s| !s.is_empty()) {
        return container_id.to_string();
    }

    panic!(
        "No running container found for docker-compose service '{}'. \
         Make sure `.docker/test-nodes.yml` is up and containers are started.",
        service_name
    );
}

/// Prepare IBC channels for compose mode
fn prepare_ibc_channels_compose() {
    let container_id = resolve_compose_container_id(KDF_IBC_RELAYER_SERVICE);

    let exec = |container: &str, args: &[&str]| {
        Command::new("docker")
            .args(["exec", container])
            .args(args)
            .output()
            .unwrap();
    };

    exec(
        &container_id,
        &["rly", "transact", "clients", "nucleus-atom", "--override"],
    );
    thread::sleep(Duration::from_secs(5));
    exec(&container_id, &["rly", "transact", "link", "nucleus-atom"]);
}

/// Wait for IBC relayer to be ready in compose mode
fn wait_until_relayer_container_is_ready_compose() {
    const Q_RESULT: &str = "0: nucleus-atom         -> chns(✔) clnts(✔) conn(✔) (nucleus-testnet<>cosmoshub-testnet)";

    let container_id = resolve_compose_container_id(KDF_IBC_RELAYER_SERVICE);

    let mut attempts = 0;
    loop {
        let mut docker = Command::new("docker");
        docker.arg("exec").arg(&container_id).args(["rly", "paths", "list"]);

        log!("Running <<{docker:?}>>.");

        let output = docker.output().unwrap();
        let output = String::from_utf8(output.stdout).unwrap();
        let output = output.trim();

        if output == Q_RESULT {
            break;
        }
        attempts += 1;

        log!("Expected output {Q_RESULT}, received {output}.");
        if attempts > 10 {
            panic!("Reached max attempts for IBC relayer readiness check.");
        } else {
            log!("Asking for relayer node status again..");
        }

        thread::sleep(Duration::from_secs(2));
    }
}

fn wait_for_geth_node_ready() {
    let mut attempts = 0;
    loop {
        if attempts >= 5 {
            panic!("Failed to connect to Geth node after several attempts.");
        }
        match block_on(GETH_WEB3.eth().block_number().timeout(Duration::from_secs(6))) {
            Ok(Ok(block_number)) => {
                log!("Geth node is ready, latest block number: {:?}", block_number);
                break;
            },
            Ok(Err(e)) => {
                log!("Failed to connect to Geth node: {:?}, retrying...", e);
            },
            Err(_) => {
                log!("Connection to Geth node timed out, retrying...");
            },
        }
        attempts += 1;
        thread::sleep(Duration::from_secs(1));
    }
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
