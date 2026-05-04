pub mod address;
pub mod api_types;
pub mod authorization;
pub mod client;
pub mod config;
pub mod error;
pub mod relay_payload;
pub mod service;
pub mod typed_data;
pub(crate) mod withdraw;

use crate::eth::ChainSpec;
pub use address::{api_path_segment_for_network, compute_gasfree_address_for_network, controller_for_network};
pub use api_types::{
    GasfreeAccountAsset, GasfreeAccountInfo, GasfreeRequestId, GasfreeSubmitRequest, GasfreeSubmitResponse,
    GasfreeSupportedToken, GasfreeTraceResponse, GasfreeTransactionState, GasfreeTransferState,
};
pub use authorization::{sign_permit_transfer, GasfreeSignedAuthorization};
pub use client::{TronGasfreeClient, TronGasfreeTransport};
pub use config::{ResolvedTronGaslessProvider, ResolvedTronGaslessTokenConfig, TronGaslessProviderConfig};
pub use error::{GaslessWithdrawError, TronGasfreeError, TronGaslessConfigError};
pub use relay_payload::{TronGasfreeRelayPayload, TRON_GASFREE_RELAY_TYPE};
pub use service::{
    DisabledReason, GasfreeAccountService, GasfreeAvailability, GasfreeTransferPreflight, GasfreeTransferRequest,
    OnChainBalanceFetcher, TronOnChainBalanceFetcher,
};
pub use typed_data::{
    build_permit_transfer_typed_data, hash_permit_transfer_typed_data, GasfreeDomain, PermitTransferData,
    PermitTransferMessage,
};
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
        network.clone(),
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
mod test_helpers {
    use super::config::{ResolvedTronGaslessProvider, TronGaslessProviderConfig};
    use super::controller_for_network;
    use crate::eth::tron::{Network, TronAddress};
    use serde_json::{json, Value};
    use std::str::FromStr;
    use url::Url;

    pub(super) const TEST_BASE_URL: &str = "https://open-test.gasfree.io";
    pub(super) const DEFAULT_SERVICE_PROVIDER: &str = "TKtWbdzEq5ss9vTS9kwRhBp5mXmBfBns3E";

    pub(super) fn provider_config(base_url: &str, service_provider: &str) -> TronGaslessProviderConfig {
        TronGaslessProviderConfig {
            base_url: Url::parse(base_url).unwrap(),
            api_key: "key".into(),
            api_secret: "secret".into(),
            service_provider: service_provider.into(),
            request_timeout_ms: 15_000,
            status_poll_interval_ms: 3_000,
        }
    }

    pub(super) fn test_provider(network: Network, service_provider: &str) -> ResolvedTronGaslessProvider {
        let raw = provider_config(TEST_BASE_URL, service_provider);
        let parsed = TronAddress::from_str(service_provider).unwrap();
        let verifying_contract = controller_for_network(&network);
        ResolvedTronGaslessProvider::new(raw, network, parsed, verifying_contract)
    }

    pub(super) fn base_submit_response_payload() -> Value {
        json!({
            "id": "6c3ff67e-0bf4-4c09-91ca-0c7c254b01a0",
            "accountAddress": "TUUSMd58eC3fKx3fn7whxJyr1FR56tgaP8",
            "gasFreeAddress": "TNER12mMVWruqopsW9FQtKxCGfZcEtb3ER",
            "providerAddress": "TKtWbdzEq5ss9vTS9kwRhBp5mXmBfBns3E",
            "targetAddress": "TEkj3ndMVEmFLYaFrATMwMjBRZ1EAZkucT",
            "tokenAddress": "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf",
            "amount": 100000,
            "maxFee": 2000000,
            "signature": "",
            "version": 1,
            "nonce": 8,
            "expiredAt": 1747909695000u64,
            "state": "WAITING",
            "estimatedActivateFee": 0,
            "estimatedTransferFee": 2000,
            "createdAt": 1747909635678u64,
            "updatedAt": 1747909635678u64
        })
    }

    pub(super) fn base_trace_response_payload() -> Value {
        json!({
            "id": "6c3ff67e-0bf4-4c09-91ca-0c7c254b01a0",
            "accountAddress": "TUUSMd58eC3fKx3fn7whxJyr1FR56tgaP8",
            "gasFreeAddress": "TNER12mMVWruqopsW9FQtKxCGfZcEtb3ER",
            "providerAddress": "TKtWbdzEq5ss9vTS9kwRhBp5mXmBfBns3E",
            "targetAddress": "TEkj3ndMVEmFLYaFrATMwMjBRZ1EAZkucT",
            "tokenAddress": "TXYZopYRdj2D9XRtbG411XZZ3kM5VkAeBf",
            "amount": 100000,
            "state": "CONFIRMING",
            "expiredAt": 1747909695000u64,
            "estimatedActivateFee": 0,
            "estimatedTransferFee": 2000,
            "estimatedTotalFee": 2000,
            "estimatedTotalCost": 102000,
            "txnHash": "22".repeat(32),
            "txnBlockNum": 57175988,
            "txnBlockTimestamp": 1747909638000u64,
            "txnState": "ON_CHAIN",
            "txnActivateFee": 0,
            "txnTransferFee": 2000,
            "txnTotalFee": 2000,
            "txnAmount": 100000,
            "txnTotalCost": 102000,
            "nonce": 8,
            "version": 1,
            "signature": "33".repeat(65)
        })
    }

    /// Wrap a payload in the GasFree provider's success envelope (`{code, reason, message, data}`).
    pub(super) fn provider_envelope(data: Value) -> Value {
        json!({
            "code": 200,
            "reason": null,
            "message": "",
            "data": data,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::{provider_config, DEFAULT_SERVICE_PROVIDER};
    use super::*;
    use crate::eth::tron::Network;

    fn resolved_path(network: Network, base_url: &str) -> String {
        let provider = resolve_tron_gasless_provider(
            &ChainSpec::Tron { network },
            Some(&provider_config(base_url, DEFAULT_SERVICE_PROVIDER)),
        )
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
            Some(&provider_config(
                "https://open.gasfree.io/tron/",
                DEFAULT_SERVICE_PROVIDER,
            )),
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
