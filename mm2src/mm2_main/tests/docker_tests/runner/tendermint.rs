//! Tendermint/Cosmos/IBC setup for docker tests.

use super::{DockerTestMode, DockerTestRunner};

use crate::docker_tests::helpers::tendermint::{
    atom_node, ibc_relayer_node, nucleus_node, prepare_ibc_channels, prepare_ibc_channels_compose,
    wait_until_relayer_container_is_ready, wait_until_relayer_container_is_ready_compose,
};

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

pub(super) fn setup(runner: &mut DockerTestRunner) {
    match runner.config.mode {
        DockerTestMode::Testcontainers => {
            let runtime_dir = prepare_runtime_dir().unwrap();

            let nucleus_node_instance = nucleus_node(runtime_dir.clone());
            let atom_node_instance = atom_node(runtime_dir.clone());
            let ibc_relayer_node_instance = ibc_relayer_node(runtime_dir.clone());

            prepare_ibc_channels(ibc_relayer_node_instance.container.id());
            thread::sleep(Duration::from_secs(10));
            wait_until_relayer_container_is_ready(ibc_relayer_node_instance.container.id());

            runner.hold(nucleus_node_instance);
            runner.hold(atom_node_instance);
            runner.hold(ibc_relayer_node_instance);
        },
        DockerTestMode::ComposeInit => {
            let _runtime_dir = get_runtime_dir();

            prepare_ibc_channels_compose();
            thread::sleep(Duration::from_secs(10));
            wait_until_relayer_container_is_ready_compose();
        },
    }
}

/// Get the runtime directory path.
fn get_runtime_dir() -> PathBuf {
    let project_root = {
        let mut current_dir = std::env::current_dir().unwrap();
        current_dir.pop();
        current_dir.pop();
        current_dir
    };
    project_root.join(".docker/container-runtime")
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
