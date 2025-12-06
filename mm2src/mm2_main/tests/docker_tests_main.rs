#![cfg(feature = "run-docker-tests")]
#![cfg(not(target_arch = "wasm32"))]
#![feature(custom_test_frameworks)]
#![feature(test)]
#![test_runner(docker_tests_runner)]

#[cfg(test)]
#[macro_use]
extern crate common;
#[cfg(test)]
#[macro_use]
extern crate gstuff;
#[cfg(test)]
#[macro_use]
extern crate lazy_static;
#[cfg(test)]
#[macro_use]
extern crate serde_json;
#[cfg(test)]
extern crate ser_error_derive;
#[cfg(test)]
extern crate test;

use common::custom_futures::timeout::FutureTimerExt;
use std::env;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use test::{test_main, StaticBenchFn, StaticTestFn, TestDescAndFn};
use web3::{transports::Http, Web3};

mod docker_tests;
mod sia_tests;
use docker_tests::docker_env_metadata::{
    get_metadata_file_path, is_docker_compose_mode, should_load_metadata, CosmosNodeState, DockerEnvMetadata,
    GethNodeState, QtumNodeState, SiaNodeState, SlpNodeState, UtxoNodeState, ZombieNodeState,
};
use docker_tests::docker_tests_common::*;
use docker_tests::qrc20_tests::{qtum_docker_node, QtumDockerOps, QTUM_REGTEST_DOCKER_IMAGE_WITH_TAG};
use sia_tests::utils::wait_for_dsia_node_ready;

#[allow(dead_code)]
mod integration_tests_common;

const ENV_VAR_NO_UTXO_DOCKER: &str = "_KDF_NO_UTXO_DOCKER";
const ENV_VAR_NO_QTUM_DOCKER: &str = "_KDF_NO_QTUM_DOCKER";
const ENV_VAR_NO_SLP_DOCKER: &str = "_KDF_NO_SLP_DOCKER";
const ENV_VAR_NO_ETH_DOCKER: &str = "_KDF_NO_ETH_DOCKER";
const ENV_VAR_NO_COSMOS_DOCKER: &str = "_KDF_NO_COSMOS_DOCKER";
const ENV_VAR_NO_ZOMBIE_DOCKER: &str = "_KDF_NO_ZOMBIE_DOCKER";
const ENV_VAR_NO_SIA_DOCKER: &str = "_KDF_NO_SIA_DOCKER";

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

// AP: custom test runner is intended to initialize the required environment (e.g. coin daemons in the docker containers)
// and then gracefully clear it by dropping the RAII docker container handlers
// I've tried to use static for such singleton initialization but it turned out that despite
// rustc allows to use Drop as static the drop fn won't ever be called
// NB: https://github.com/rust-lang/rfcs/issues/1111
// the only preparation step required is Zcash params files downloading:
// Windows - https://github.com/KomodoPlatform/komodo/blob/master/zcutil/fetch-params.bat
// Linux and MacOS - https://github.com/KomodoPlatform/komodo/blob/master/zcutil/fetch-params.sh
pub fn docker_tests_runner(tests: &[&TestDescAndFn]) {
    // pretty_env_logger::try_init();
    let mut containers = vec![];

    // Determine execution mode
    let mode = determine_test_mode();
    log!("Docker test mode: {:?}", mode);

    // skip Docker containers initialization if we are intended to run test_mm_start only
    if env::var("_MM2_TEST_CONF").is_err() {
        match mode {
            DockerTestMode::ReuseMetadata => {
                // Load metadata and set global state without starting containers or initialization
                let metadata_path = get_metadata_file_path().expect("KDF_DOCKER_ENV_STATE_FILE must be set");
                let metadata =
                    DockerEnvMetadata::load(&metadata_path).expect("Failed to load docker environment metadata");

                // Validate that nodes are healthy before proceeding
                if let Err(e) = validate_nodes_health(&metadata) {
                    panic!("Node health check failed: {}. Ensure containers are running or remove KDF_DOCKER_ENV_STATE_FILE to start fresh.", e);
                }

                load_metadata_into_globals(&metadata);
                log!("Loaded environment state from metadata, skipping container startup and initialization");
            },
            DockerTestMode::ComposeInit | DockerTestMode::Testcontainers => {
                // For both modes, we may need to track metadata
                let mut metadata = DockerEnvMetadata::new();

                let disable_utxo: bool = env::var(ENV_VAR_NO_UTXO_DOCKER).is_ok();
                let disable_slp: bool = env::var(ENV_VAR_NO_SLP_DOCKER).is_ok();
                let disable_qtum: bool = env::var(ENV_VAR_NO_QTUM_DOCKER).is_ok();
                let disable_eth: bool = env::var(ENV_VAR_NO_ETH_DOCKER).is_ok();
                let disable_cosmos: bool = env::var(ENV_VAR_NO_COSMOS_DOCKER).is_ok();
                let disable_zombie: bool = env::var(ENV_VAR_NO_ZOMBIE_DOCKER).is_ok();
                let disable_sia: bool = env::var(ENV_VAR_NO_SIA_DOCKER).is_ok();

                // Only pull images and start containers in Testcontainers mode
                if mode == DockerTestMode::Testcontainers {
                    let mut images = vec![];

                    if !disable_utxo || !disable_slp {
                        images.push(UTXO_ASSET_DOCKER_IMAGE_WITH_TAG)
                    }
                    if !disable_qtum {
                        images.push(QTUM_REGTEST_DOCKER_IMAGE_WITH_TAG);
                    }
                    if !disable_eth {
                        images.push(GETH_DOCKER_IMAGE_WITH_TAG);
                    }
                    if !disable_cosmos {
                        images.push(NUCLEUS_IMAGE);
                        images.push(ATOM_IMAGE_WITH_TAG);
                        images.push(IBC_RELAYER_IMAGE_WITH_TAG);
                    }
                    if !disable_zombie {
                        images.push(ZOMBIE_ASSET_DOCKER_IMAGE_WITH_TAG);
                    }
                    if !disable_sia {
                        images.push(SIA_DOCKER_IMAGE_WITH_TAG);
                    }

                    for image in images {
                        pull_docker_image(image);
                        remove_docker_containers(image);
                    }
                }

                // Start containers (testcontainers mode) or assume they're running (compose mode)
                let (nucleus_node, atom_node, ibc_relayer_node) = if !disable_cosmos {
                    if mode == DockerTestMode::Testcontainers {
                        let runtime_dir = prepare_runtime_dir().unwrap();
                        let nucleus_node = nucleus_node(runtime_dir.clone());
                        let atom_node = atom_node(runtime_dir.clone());
                        let ibc_relayer_node = ibc_relayer_node(runtime_dir.clone());
                        metadata.cosmos = Some(CosmosNodeState {
                            nucleus_rpc_url: "http://localhost:26657".to_string(),
                            atom_rpc_url: "http://localhost:26658".to_string(),
                            runtime_dir,
                            ibc_channels_ready: false,
                        });
                        (Some(nucleus_node), Some(atom_node), Some(ibc_relayer_node))
                    } else {
                        // Compose mode: containers already running, just record metadata
                        let runtime_dir = get_runtime_dir();
                        metadata.cosmos = Some(CosmosNodeState {
                            nucleus_rpc_url: "http://localhost:26657".to_string(),
                            atom_rpc_url: "http://localhost:26658".to_string(),
                            runtime_dir,
                            ibc_channels_ready: false,
                        });
                        (None, None, None)
                    }
                } else {
                    (None, None, None)
                };

                let (utxo_node, utxo_node1) = if !disable_utxo {
                    if mode == DockerTestMode::Testcontainers {
                        let utxo_node = utxo_asset_docker_node("MYCOIN", 8000);
                        let utxo_node1 = utxo_asset_docker_node("MYCOIN1", 8001);
                        (Some(utxo_node), Some(utxo_node1))
                    } else {
                        (None, None)
                    }
                } else {
                    (None, None)
                };
                if !disable_utxo {
                    metadata.utxo = Some(UtxoNodeState {
                        mycoin_port: 8000,
                        mycoin1_port: 8001,
                    });
                }

                let qtum_node = if !disable_qtum {
                    if mode == DockerTestMode::Testcontainers {
                        Some(qtum_docker_node(9000))
                    } else {
                        None
                    }
                } else {
                    None
                };

                let for_slp_node = if !disable_slp {
                    if mode == DockerTestMode::Testcontainers {
                        Some(utxo_asset_docker_node("FORSLP", 10000))
                    } else {
                        None
                    }
                } else {
                    None
                };

                let geth_node = if !disable_eth {
                    if mode == DockerTestMode::Testcontainers {
                        Some(geth_docker_node("ETH", 8545))
                    } else {
                        None
                    }
                } else {
                    None
                };

                let zombie_node = if !disable_zombie {
                    if mode == DockerTestMode::Testcontainers {
                        Some(zombie_asset_docker_node(7090))
                    } else {
                        None
                    }
                } else {
                    None
                };

                let sia_node = if !disable_sia {
                    if mode == DockerTestMode::Testcontainers {
                        Some(sia_docker_node("SIA", 9980))
                    } else {
                        None
                    }
                } else {
                    None
                };

                // Initialize UTXO nodes
                if !disable_utxo {
                    if let (Some(utxo_node), Some(utxo_node1)) = (utxo_node, utxo_node1) {
                        let utxo_ops = UtxoAssetDockerOps::from_ticker("MYCOIN");
                        let utxo_ops1 = UtxoAssetDockerOps::from_ticker("MYCOIN1");
                        utxo_ops.wait_ready(4);
                        utxo_ops1.wait_ready(4);
                        containers.push(utxo_node);
                        containers.push(utxo_node1);
                    } else if mode == DockerTestMode::ComposeInit {
                        // Copy configs from containers before initializing
                        setup_utxo_conf_for_compose("MYCOIN", "kdf-mycoin");
                        setup_utxo_conf_for_compose("MYCOIN1", "kdf-mycoin1");
                        let utxo_ops = UtxoAssetDockerOps::from_ticker("MYCOIN");
                        let utxo_ops1 = UtxoAssetDockerOps::from_ticker("MYCOIN1");
                        utxo_ops.wait_ready(4);
                        utxo_ops1.wait_ready(4);
                    }
                    metadata.initialized.utxo = true;
                }

                // Initialize Qtum/QRC20
                if !disable_qtum {
                    if let Some(qtum_node) = qtum_node {
                        let qtum_ops = QtumDockerOps::new();
                        qtum_ops.wait_ready(2);
                        qtum_ops.initialize_contracts();
                        containers.push(qtum_node);
                    } else if mode == DockerTestMode::ComposeInit {
                        // In compose mode, we need to set up QTUM_CONF_PATH first
                        setup_qtum_conf_for_compose();
                        let qtum_ops = QtumDockerOps::new();
                        qtum_ops.wait_ready(2);
                        qtum_ops.initialize_contracts();
                    }
                    // Record Qtum state in metadata
                    #[allow(static_mut_refs)]
                    unsafe {
                        if let (Some(conf_path), Some(qick), Some(qorty), Some(swap)) = (
                            QTUM_CONF_PATH.as_ref(),
                            QICK_TOKEN_ADDRESS,
                            QORTY_TOKEN_ADDRESS,
                            QRC20_SWAP_CONTRACT_ADDRESS,
                        ) {
                            metadata.qtum = Some(QtumNodeState {
                                port: 9000,
                                conf_path: conf_path.clone(),
                                qick_token_address: qick,
                                qorty_token_address: qorty,
                                swap_contract_address: swap,
                            });
                        }
                    }
                    metadata.initialized.qtum = true;
                }

                // Initialize SLP
                if !disable_slp {
                    if let Some(for_slp_node) = for_slp_node {
                        let for_slp_ops = BchDockerOps::from_ticker("FORSLP");
                        for_slp_ops.wait_ready(4);
                        for_slp_ops.initialize_slp();
                        containers.push(for_slp_node);
                    } else if mode == DockerTestMode::ComposeInit {
                        // Copy config from container before initializing
                        setup_utxo_conf_for_compose("FORSLP", "kdf-forslp");
                        let for_slp_ops = BchDockerOps::from_ticker("FORSLP");
                        for_slp_ops.wait_ready(4);
                        for_slp_ops.initialize_slp();
                    }
                    // Record SLP state in metadata
                    let token_id = *SLP_TOKEN_ID.lock().unwrap();
                    let token_owners = SLP_TOKEN_OWNERS.lock().unwrap().clone();
                    metadata.slp = Some(SlpNodeState {
                        port: 10000,
                        token_id,
                        token_owners,
                    });
                    metadata.initialized.slp = true;
                }

                // Initialize Geth/Ethereum
                if !disable_eth {
                    if let Some(geth_node) = geth_node {
                        wait_for_geth_node_ready();
                        init_geth_node();
                        containers.push(geth_node);
                    } else if mode == DockerTestMode::ComposeInit {
                        wait_for_geth_node_ready();
                        init_geth_node();
                    }
                    // Record Geth state in metadata
                    unsafe {
                        metadata.geth = Some(GethNodeState {
                            rpc_url: GETH_RPC_URL.to_string(),
                            account: GETH_ACCOUNT,
                            erc20_contract: GETH_ERC20_CONTRACT,
                            swap_contract: GETH_SWAP_CONTRACT,
                            maker_swap_v2: GETH_MAKER_SWAP_V2,
                            taker_swap_v2: GETH_TAKER_SWAP_V2,
                            watchers_swap_contract: GETH_WATCHERS_SWAP_CONTRACT,
                            erc721_contract: GETH_ERC721_CONTRACT,
                            erc1155_contract: GETH_ERC1155_CONTRACT,
                            nft_maker_swap_v2: GETH_NFT_MAKER_SWAP_V2,
                        });
                    }
                    metadata.initialized.geth = true;
                }

                // Initialize Zombie
                if !disable_zombie {
                    if let Some(zombie_node) = zombie_node {
                        let zombie_ops = ZCoinAssetDockerOps::new();
                        zombie_ops.wait_ready(4);
                        containers.push(zombie_node);
                    } else if mode == DockerTestMode::ComposeInit {
                        // Copy config from container before initializing
                        setup_utxo_conf_for_compose("ZOMBIE", "kdf-zombie");
                        let zombie_ops = ZCoinAssetDockerOps::new();
                        zombie_ops.wait_ready(4);
                    }
                    metadata.zombie = Some(ZombieNodeState {
                        port: 7090,
                        conf_path: coins::utxo::coin_daemon_data_dir("ZOMBIE", true).join("ZOMBIE.conf"),
                    });
                    metadata.initialized.zombie = true;
                }

                // Initialize Cosmos/IBC
                if !disable_cosmos {
                    if let (Some(nucleus_node), Some(atom_node), Some(ibc_relayer_node)) =
                        (nucleus_node, atom_node, ibc_relayer_node)
                    {
                        prepare_ibc_channels(ibc_relayer_node.container.id());
                        thread::sleep(Duration::from_secs(10));
                        wait_until_relayer_container_is_ready(ibc_relayer_node.container.id());
                        containers.push(nucleus_node);
                        containers.push(atom_node);
                        containers.push(ibc_relayer_node);
                    } else if mode == DockerTestMode::ComposeInit {
                        // In compose mode, prepare IBC channels using the kdf-ibc-relayer container
                        prepare_ibc_channels_compose();
                        thread::sleep(Duration::from_secs(10));
                        wait_until_relayer_container_is_ready_compose();
                    }
                    if let Some(ref mut cosmos) = metadata.cosmos {
                        cosmos.ibc_channels_ready = true;
                    }
                    metadata.initialized.cosmos = true;
                }

                // Initialize Sia
                if !disable_sia {
                    if let Some(sia_node) = sia_node {
                        block_on(wait_for_dsia_node_ready());
                        containers.push(sia_node);
                    } else if mode == DockerTestMode::ComposeInit {
                        block_on(wait_for_dsia_node_ready());
                    }
                    metadata.sia = Some(SiaNodeState {
                        rpc_host: SIA_RPC_PARAMS.0.to_string(),
                        rpc_port: SIA_RPC_PARAMS.1,
                        rpc_password: SIA_RPC_PARAMS.2.to_string(),
                        initialized: true,
                    });
                    metadata.initialized.sia = true;
                }

                // Save metadata in compose mode for future reuse
                if mode == DockerTestMode::ComposeInit {
                    let metadata_path = DockerEnvMetadata::default_path();
                    if let Some(parent) = metadata_path.parent() {
                        std::fs::create_dir_all(parent).ok();
                    }
                    if let Err(e) = metadata.save(&metadata_path) {
                        log!("Warning: Failed to save docker environment metadata: {}", e);
                    } else {
                        log!("Saved docker environment metadata to {:?}", metadata_path);
                    }
                }
            },
        }
    }

    // Run tests
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

/// Validate that nodes are reachable before loading metadata
fn validate_nodes_health(metadata: &DockerEnvMetadata) -> Result<(), String> {
    use std::net::TcpStream;
    use std::time::Duration;

    log!("Validating node health from metadata...");

    // Check UTXO nodes (MYCOIN, MYCOIN1)
    if metadata.initialized.utxo {
        if let Some(ref utxo) = metadata.utxo {
            for (name, port) in [("MYCOIN", utxo.mycoin_port), ("MYCOIN1", utxo.mycoin1_port)] {
                let addr = format!("127.0.0.1:{}", port);
                if TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_secs(2)).is_err() {
                    return Err(format!("{} node not reachable at {}", name, addr));
                }
                log!("  {} node OK at port {}", name, port);
            }
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
            // Check Nucleus RPC (port 26657)
            let addr = "127.0.0.1:26657";
            if TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_secs(2)).is_err() {
                return Err(format!("NUCLEUS node not reachable at {}", addr));
            }
            log!("  NUCLEUS node OK at {}", cosmos.nucleus_rpc_url);

            // Check Atom RPC (port 26658)
            let addr = "127.0.0.1:26658";
            if TcpStream::connect_timeout(&addr.parse().unwrap(), Duration::from_secs(2)).is_err() {
                return Err(format!("ATOM node not reachable at {}", addr));
            }
            log!("  ATOM node OK at {}", cosmos.atom_rpc_url);
        }
    }

    // Check Sia node
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
    unsafe {
        // Load Qtum state
        if let Some(ref qtum) = metadata.qtum {
            QTUM_CONF_PATH = Some(qtum.conf_path.clone());
            QICK_TOKEN_ADDRESS = Some(qtum.qick_token_address);
            QORTY_TOKEN_ADDRESS = Some(qtum.qorty_token_address);
            QRC20_SWAP_CONTRACT_ADDRESS = Some(qtum.swap_contract_address);
        }

        // Load SLP state
        if let Some(ref slp) = metadata.slp {
            *SLP_TOKEN_ID.lock().unwrap() = slp.token_id;
            *SLP_TOKEN_OWNERS.lock().unwrap() = slp.token_owners.clone();
        }

        // Load Geth state
        if let Some(ref geth) = metadata.geth {
            GETH_ACCOUNT = geth.account;
            GETH_ERC20_CONTRACT = geth.erc20_contract;
            GETH_SWAP_CONTRACT = geth.swap_contract;
            GETH_MAKER_SWAP_V2 = geth.maker_swap_v2;
            GETH_TAKER_SWAP_V2 = geth.taker_swap_v2;
            GETH_WATCHERS_SWAP_CONTRACT = geth.watchers_swap_contract;
            GETH_ERC721_CONTRACT = geth.erc721_contract;
            GETH_ERC1155_CONTRACT = geth.erc1155_contract;
            GETH_NFT_MAKER_SWAP_V2 = geth.nft_maker_swap_v2;
        }
    }

    log!("Loaded global state from metadata");
}

/// Set up QTUM_CONF_PATH for compose mode by copying config from the container
fn setup_qtum_conf_for_compose() {
    let mut conf_path = coins::utxo::coin_daemon_data_dir("qtum", false);
    std::fs::create_dir_all(&conf_path).unwrap();
    conf_path.push("qtum.conf");

    // Copy config from the running compose container
    Command::new("docker")
        .arg("cp")
        .arg("kdf-qtum:/data/node_0/qtum.conf")
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

    unsafe { QTUM_CONF_PATH = Some(conf_path) };
}

/// Set up UTXO coin config for compose mode by copying config from the container
fn setup_utxo_conf_for_compose(ticker: &str, container_name: &str) {
    let mut conf_path = coins::utxo::coin_daemon_data_dir(ticker, true);
    std::fs::create_dir_all(&conf_path).unwrap();
    conf_path.push(format!("{ticker}.conf"));

    // Copy config from the running compose container
    Command::new("docker")
        .arg("cp")
        .arg(format!("{container_name}:/data/node_0/{ticker}.conf"))
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

/// Prepare IBC channels for compose mode
fn prepare_ibc_channels_compose() {
    let exec = |args: &[&str]| {
        Command::new("docker")
            .args(["exec", "kdf-ibc-relayer"])
            .args(args)
            .output()
            .unwrap();
    };

    exec(&["rly", "transact", "clients", "nucleus-atom", "--override"]);
    thread::sleep(Duration::from_secs(5));
    exec(&["rly", "transact", "link", "nucleus-atom"]);
}

/// Wait for IBC relayer to be ready in compose mode
fn wait_until_relayer_container_is_ready_compose() {
    const Q_RESULT: &str = "0: nucleus-atom         -> chns(✔) clnts(✔) conn(✔) (nucleus-testnet<>cosmoshub-testnet)";

    let mut attempts = 0;
    loop {
        let mut docker = Command::new("docker");
        docker.arg("exec").arg("kdf-ibc-relayer").args(["rly", "paths", "list"]);

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

    // Remove runtime directory if it exists to copy containers files to a clean directory
    if containers_runtime_dir.exists() {
        std::fs::remove_dir_all(&containers_runtime_dir).unwrap();
    }

    // Copy container files to runtime directory
    mm2_io::fs::copy_dir_all(&containers_state_dir, &containers_runtime_dir).unwrap();

    Ok(containers_runtime_dir)
}
