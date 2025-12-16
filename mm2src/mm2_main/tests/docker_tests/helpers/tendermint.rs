//! Tendermint/Cosmos helpers for docker tests.
//!
//! This module provides:
//! - Docker node helpers for Nucleus, Atom, and IBC relayer
//! - IBC channel preparation utilities

use crate::docker_tests::helpers::env::{resolve_compose_container_id, DockerNode, KDF_IBC_RELAYER_SERVICE};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;
use testcontainers::core::Mount;
use testcontainers::runners::SyncRunner;
use testcontainers::{GenericImage, RunnableImage};

// =============================================================================
// Docker image constants
// =============================================================================

/// Nucleus docker image
pub const NUCLEUS_IMAGE: &str = "docker.io/gleec/nucleusd";
/// Atom (Gaia) docker image with tag
pub const ATOM_IMAGE_WITH_TAG: &str = "docker.io/gleec/gaiad:kdf-ci";
/// IBC relayer docker image with tag
pub const IBC_RELAYER_IMAGE_WITH_TAG: &str = "docker.io/gleec/ibc-relayer:kdf-ci";

// =============================================================================
// Docker node helpers
// =============================================================================

/// Start a Nucleus docker node for testing.
pub fn nucleus_node(runtime_dir: PathBuf) -> DockerNode {
    let nucleus_node_runtime_dir = runtime_dir.join("nucleus-testnet-data");
    assert!(nucleus_node_runtime_dir.exists());

    let image = GenericImage::new(NUCLEUS_IMAGE, "latest").with_mount(Mount::bind_mount(
        nucleus_node_runtime_dir.to_str().unwrap(),
        "/root/.nucleus",
    ));
    let image = RunnableImage::from((image, vec![])).with_network("host");
    let container = image.start().expect("Failed to start Nucleus docker node");

    DockerNode {
        container,
        ticker: "NUCLEUS-TEST".to_owned(),
        port: Default::default(), // This doesn't need to be the correct value as we are using the host network.
    }
}

/// Start an Atom (Gaia) docker node for testing.
pub fn atom_node(runtime_dir: PathBuf) -> DockerNode {
    let atom_node_runtime_dir = runtime_dir.join("atom-testnet-data");
    assert!(atom_node_runtime_dir.exists());

    let (image, tag) = ATOM_IMAGE_WITH_TAG.rsplit_once(':').unwrap();
    let image = GenericImage::new(image, tag).with_mount(Mount::bind_mount(
        atom_node_runtime_dir.to_str().unwrap(),
        "/root/.gaia",
    ));
    let image = RunnableImage::from((image, vec![])).with_network("host");
    let container = image.start().expect("Failed to start Atom docker node");

    DockerNode {
        container,
        ticker: "ATOM-TEST".to_owned(),
        port: Default::default(), // This doesn't need to be the correct value as we are using the host network.
    }
}

/// Start an IBC relayer docker node for testing.
pub fn ibc_relayer_node(runtime_dir: PathBuf) -> DockerNode {
    let relayer_node_runtime_dir = runtime_dir.join("ibc-relayer-data");
    assert!(relayer_node_runtime_dir.exists());

    let (image, tag) = IBC_RELAYER_IMAGE_WITH_TAG.rsplit_once(':').unwrap();
    let image = GenericImage::new(image, tag).with_mount(Mount::bind_mount(
        relayer_node_runtime_dir.to_str().unwrap(),
        "/root/.relayer",
    ));
    let image = RunnableImage::from((image, vec![])).with_network("host");
    let container = image.start().expect("Failed to start IBC Relayer docker node");

    DockerNode {
        container,
        ticker: Default::default(), // This isn't an asset node.
        port: Default::default(),   // This doesn't need to be the correct value as we are using the host network.
    }
}

// =============================================================================
// IBC utilities
// =============================================================================

/// Prepare IBC channels between Nucleus and Atom.
pub fn prepare_ibc_channels(container_id: &str) {
    let exec = |args: &[&str]| {
        Command::new("docker")
            .args(["exec", container_id])
            .args(args)
            .output()
            .unwrap();
    };

    exec(&["rly", "transact", "clients", "nucleus-atom", "--override"]);
    // It takes a couple of seconds for nodes to get into the right state after updating clients.
    // Wait for 5 just to make sure.
    thread::sleep(Duration::from_secs(5));

    exec(&["rly", "transact", "link", "nucleus-atom"]);
}

/// Wait until the IBC relayer container is ready.
pub fn wait_until_relayer_container_is_ready(container_id: &str) {
    const Q_RESULT: &str = "0: nucleus-atom         -> chns(✔) clnts(✔) conn(✔) (nucleus-testnet<>cosmoshub-testnet)";

    let mut attempts = 0;
    loop {
        let mut docker = Command::new("docker");
        docker.arg("exec").arg(container_id).args(["rly", "paths", "list"]);

        log!("Running <<{docker:?}>>.");

        let output = docker.stderr(Stdio::inherit()).output().unwrap();
        let output = String::from_utf8(output.stdout).unwrap();
        let output = output.trim();

        if output == Q_RESULT {
            break;
        }
        attempts += 1;

        log!("Expected output {Q_RESULT}, received {output}.");
        if attempts > 10 {
            panic!("Reached max attempts for <<{:?}>>.", docker);
        } else {
            log!("Asking for relayer node status again..");
        }

        thread::sleep(Duration::from_secs(2));
    }
}

// =============================================================================
// Compose mode utilities
// =============================================================================

/// Prepare IBC channels for compose mode.
///
/// Resolves the IBC relayer container ID from docker-compose and prepares channels.
pub fn prepare_ibc_channels_compose() {
    let container_id = resolve_compose_container_id(KDF_IBC_RELAYER_SERVICE);
    prepare_ibc_channels(&container_id);
}

/// Wait for IBC relayer to be ready in compose mode.
///
/// Resolves the IBC relayer container ID from docker-compose and waits for readiness.
pub fn wait_until_relayer_container_is_ready_compose() {
    let container_id = resolve_compose_container_id(KDF_IBC_RELAYER_SERVICE);
    wait_until_relayer_container_is_ready(&container_id);
}
