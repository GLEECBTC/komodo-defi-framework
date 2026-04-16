pub mod address;
pub mod config;
pub mod error;

pub use address::{compute_gasfree_address_for_network, controller_for_network};
pub use config::{ResolvedTronGaslessProvider, TronGaslessProviderConfig};
pub use error::TronGaslessConfigError;

use crate::eth::ChainSpec;

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

    Ok(Some(ResolvedTronGaslessProvider::new(
        config.clone(),
        service_provider,
        verifying_contract,
    )))
}
