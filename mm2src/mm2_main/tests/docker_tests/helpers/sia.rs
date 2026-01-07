//! Sia helpers for docker tests.
//!
//! This module provides:
//! - Sia docker node configuration and startup
//! - Sia RPC connection parameters

use super::env::DockerNode;
use testcontainers::core::Mount;
use testcontainers::runners::SyncRunner;
use testcontainers::{core::WaitFor, GenericImage, RunnableImage};

// =============================================================================
// Sia node configuration
// =============================================================================

/// SIA daemon RPC connection parameters: (host, port, password)
pub static SIA_RPC_PARAMS: (&str, u16, &str) = ("127.0.0.1", 9980, "password");

/// SIA docker image name
pub const SIA_DOCKER_IMAGE: &str = "ghcr.io/siafoundation/walletd";
/// SIA docker image with tag
pub const SIA_DOCKER_IMAGE_WITH_TAG: &str = "ghcr.io/siafoundation/walletd:latest";

// =============================================================================
// Docker node helpers
// =============================================================================

/// Start a Sia docker node for testing.
///
/// This helper creates the necessary config files and starts the walletd container.
pub fn sia_docker_node(ticker: &'static str, port: u16) -> DockerNode {
    use crate::sia_tests::utils::{WALLETD_CONFIG, WALLETD_NETWORK_CONFIG};

    let config_dir = std::env::temp_dir()
        .join(format!(
            "sia-docker-tests-temp-{}",
            chrono::Local::now().format("%Y-%m-%d_%H-%M-%S-%3f")
        ))
        .join("walletd_config");
    std::fs::create_dir_all(&config_dir).unwrap();

    // Write walletd.yml
    std::fs::write(config_dir.join("walletd.yml"), WALLETD_CONFIG).expect("failed to write walletd.yml");

    // Write ci_network.json
    std::fs::write(config_dir.join("ci_network.json"), WALLETD_NETWORK_CONFIG)
        .expect("failed to write ci_network.json");

    let image = GenericImage::new(SIA_DOCKER_IMAGE, "latest")
        .with_env_var("WALLETD_CONFIG_FILE", "/config/walletd.yml")
        .with_wait_for(WaitFor::message_on_stdout("node started"))
        .with_mount(Mount::bind_mount(
            config_dir.to_str().expect("config path is invalid"),
            "/config",
        ));

    let args = vec!["-network=/config/ci_network.json".to_string(), "-debug".to_string()];
    let image = RunnableImage::from(image)
        .with_mapped_port((port, port))
        .with_args(args);

    let container = image.start().expect("Failed to start Sia docker node");
    DockerNode {
        container,
        ticker: ticker.into(),
        port,
    }
}
