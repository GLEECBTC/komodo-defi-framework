pub mod address;
pub mod api_types;
pub mod client;
pub mod config;
pub mod error;

use crate::eth::ChainSpec;
pub use address::{api_path_segment_for_network, compute_gasfree_address_for_network, controller_for_network};
pub use api_types::{
    GasfreeAccountAsset, GasfreeAccountInfo, GasfreeRequestId, GasfreeSubmitRequest, GasfreeSubmitResponse,
    GasfreeSupportedToken, GasfreeTraceResponse, GasfreeTransactionState, GasfreeTransferState,
};
pub use client::{TronGasfreeClient, TronGasfreeTransport};
pub use config::{ResolvedTronGaslessProvider, TronGaslessProviderConfig};
pub use error::{TronGasfreeError, TronGaslessConfigError};
use url::Url;

/// Validate and resolve a TRON GasFree provider config at platform activation time.
pub fn resolve_tron_gasless_provider(
    chain_spec: &ChainSpec,
    provider_config: Option<&TronGaslessProviderConfig>,
) -> Result<Option<ResolvedTronGaslessProvider>, TronGaslessConfigError> {
    let config = match provider_config {
        Some(cfg) => cfg,
        None => return Ok(None),
    };

    // GasFree is TRON-only
    let network = match chain_spec {
        ChainSpec::Tron { network } => network,
        _ => {
            return Err(TronGaslessConfigError::UnsupportedChain {
                chain: chain_spec.kind().to_string(),
            });
        },
    };

    let service_provider = config.service_provider_address()?;
    let verifying_contract = controller_for_network(network);
    let mut resolved_config = config.clone();
    resolved_config.base_url = resolve_gasfree_base_url(&config.base_url, network)?;

    Ok(Some(ResolvedTronGaslessProvider::new(
        resolved_config,
        service_provider,
        verifying_contract,
    )))
}

fn resolve_gasfree_base_url(
    base_url: &Url,
    network: &crate::eth::tron::Network,
) -> Result<Url, TronGaslessConfigError> {
    let path = base_url.path();
    if path != "/" {
        return Err(TronGaslessConfigError::InvalidBaseUrl {
            reason: format!("expected a host-only URL, got path '{path}'"),
        });
    }

    let mut resolved = base_url.clone();
    resolved.set_path(&format!("/{}/", api_path_segment_for_network(network)));
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eth::tron::Network;

    fn provider_config(base_url: &str) -> TronGaslessProviderConfig {
        TronGaslessProviderConfig {
            base_url: Url::parse(base_url).unwrap(),
            api_key: "key".into(),
            api_secret: "secret".into(),
            service_provider: "TKtWbdzEq5ss9vTS9kwRhBp5mXmBfBns3E".into(),
            request_timeout_ms: 15_000,
            status_poll_interval_ms: 3_000,
        }
    }

    fn resolved_path(network: Network, base_url: &str) -> String {
        let provider = resolve_tron_gasless_provider(&ChainSpec::Tron { network }, Some(&provider_config(base_url)))
            .unwrap()
            .unwrap();

        provider
            .config()
            .base_url
            .join("api/v1/config/token/all")
            .unwrap()
            .path()
            .to_string()
    }

    #[test]
    fn resolver_derives_network_base_path_for_each_network() {
        assert_eq!(
            resolved_path(Network::Mainnet, "https://open.gasfree.io"),
            "/tron/api/v1/config/token/all"
        );
        assert_eq!(
            resolved_path(Network::Nile, "https://open-test.gasfree.io"),
            "/nile/api/v1/config/token/all"
        );
        assert_eq!(
            resolved_path(Network::Shasta, "https://open-test.gasfree.io"),
            "/shasta/api/v1/config/token/all"
        );
    }

    #[test]
    fn resolver_rejects_user_supplied_base_url_path() {
        let err = resolve_tron_gasless_provider(
            &ChainSpec::Tron {
                network: Network::Mainnet,
            },
            Some(&provider_config("https://open.gasfree.io/tron/")),
        )
        .unwrap_err();

        match err {
            TronGaslessConfigError::InvalidBaseUrl { reason } => assert!(reason.contains("host-only")),
            other => panic!("unexpected error: {:?}", other),
        }
    }

    #[test]
    fn resolver_tolerates_trailing_slash_on_host_only_base_url() {
        assert_eq!(
            resolved_path(Network::Mainnet, "https://open.gasfree.io"),
            resolved_path(Network::Mainnet, "https://open.gasfree.io/")
        );
    }
}
