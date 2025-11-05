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
use testcontainers::clients::Cli;

mod docker_tests;
use docker_tests::docker_tests_common::*;
use docker_tests::qrc20_tests::{qtum_docker_node, QtumDockerOps, QTUM_REGTEST_DOCKER_IMAGE_WITH_TAG};

#[allow(dead_code)]
mod integration_tests_common;

const ENV_VAR_NO_UTXO_DOCKER: &str = "_KDF_NO_UTXO_DOCKER";
const ENV_VAR_NO_QTUM_DOCKER: &str = "_KDF_NO_QTUM_DOCKER";
const ENV_VAR_NO_SLP_DOCKER: &str = "_KDF_NO_SLP_DOCKER";
const ENV_VAR_NO_ETH_DOCKER: &str = "_KDF_NO_ETH_DOCKER";
const ENV_VAR_NO_COSMOS_DOCKER: &str = "_KDF_NO_COSMOS_DOCKER";
const ENV_VAR_NO_ZOMBIE_DOCKER: &str = "_KDF_NO_ZOMBIE_DOCKER";

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
    let docker = Cli::default();
    let mut containers = vec![];
    // skip Docker containers initialization if we are intended to run test_mm_start only
    if env::var("_MM2_TEST_CONF").is_err() {
        let mut images = vec![];

        let disable_utxo: bool = env::var(ENV_VAR_NO_UTXO_DOCKER).is_ok();
        let disable_slp: bool = env::var(ENV_VAR_NO_SLP_DOCKER).is_ok();
        let disable_qtum: bool = env::var(ENV_VAR_NO_QTUM_DOCKER).is_ok();
        let disable_eth: bool = env::var(ENV_VAR_NO_ETH_DOCKER).is_ok();
        let disable_cosmos: bool = env::var(ENV_VAR_NO_COSMOS_DOCKER).is_ok();
        let disable_zombie: bool = env::var(ENV_VAR_NO_ZOMBIE_DOCKER).is_ok();

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

        for image in images {
            pull_docker_image(image);
            remove_docker_containers(image);
        }

        let (nucleus_node, atom_node, ibc_relayer_node) = if !disable_cosmos {
            let runtime_dir = prepare_runtime_dir().unwrap();
            let nucleus_node = nucleus_node(&docker, runtime_dir.clone());
            let atom_node = atom_node(&docker, runtime_dir.clone());
            let ibc_relayer_node = ibc_relayer_node(&docker, runtime_dir);
            (Some(nucleus_node), Some(atom_node), Some(ibc_relayer_node))
        } else {
            (None, None, None)
        };
        let (utxo_node, utxo_node1) = if !disable_utxo {
            let utxo_node = utxo_asset_docker_node(&docker, "MYCOIN", 7000);
            let utxo_node1 = utxo_asset_docker_node(&docker, "MYCOIN1", 8000);
            (Some(utxo_node), Some(utxo_node1))
        } else {
            (None, None)
        };
        let qtum_node = if !disable_qtum {
            let qtum_node = qtum_docker_node(&docker, 9000);
            Some(qtum_node)
        } else {
            None
        };
        let for_slp_node = if !disable_slp {
            let for_slp_node = utxo_asset_docker_node(&docker, "FORSLP", 10000);
            Some(for_slp_node)
        } else {
            None
        };
        let geth_node = if !disable_eth {
            let geth_node = geth_docker_node(&docker, "ETH", 8545);
            Some(geth_node)
        } else {
            None
        };
        let zombie_node = if !disable_zombie {
            let zombie_node = zombie_asset_docker_node(&docker, 7090);
            Some(zombie_node)
        } else {
            None
        };

        if let (Some(utxo_node), Some(utxo_node1)) = (utxo_node, utxo_node1) {
            let utxo_ops = UtxoAssetDockerOps::from_ticker("MYCOIN");
            let utxo_ops1 = UtxoAssetDockerOps::from_ticker("MYCOIN1");
            utxo_ops.wait_ready(4);
            utxo_ops1.wait_ready(4);
            containers.push(utxo_node);
            containers.push(utxo_node1);
        }
        if let Some(qtum_node) = qtum_node {
            let qtum_ops = QtumDockerOps::new();
            qtum_ops.wait_ready(2);
            qtum_ops.initialize_contracts();
            containers.push(qtum_node);
        }
        if let Some(for_slp_node) = for_slp_node {
            let for_slp_ops = BchDockerOps::from_ticker("FORSLP");
            for_slp_ops.wait_ready(4);
            for_slp_ops.initialize_slp();
            containers.push(for_slp_node);
        }
        if let Some(geth_node) = geth_node {
            wait_for_geth_node_ready();
            init_geth_node();
            containers.push(geth_node);
        }
        if let Some(zombie_node) = zombie_node {
            let zombie_ops = ZCoinAssetDockerOps::new();
            zombie_ops.wait_ready(4);
            containers.push(zombie_node);
        }
        if let (Some(nucleus_node), Some(atom_node), Some(ibc_relayer_node)) =
            (nucleus_node, atom_node, ibc_relayer_node)
        {
            prepare_ibc_channels(ibc_relayer_node.container.id());
            thread::sleep(Duration::from_secs(10));
            wait_until_relayer_container_is_ready(ibc_relayer_node.container.id());
            containers.push(nucleus_node);
            containers.push(atom_node);
            containers.push(ibc_relayer_node);
        }
    }
    // detect if docker is installed
    // skip the tests that use docker if not installed
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
