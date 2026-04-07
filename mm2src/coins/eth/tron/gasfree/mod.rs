pub mod config;
pub mod error;

pub use config::TronGaslessProviderConfig;
pub use error::TronGaslessConfigError;

use crate::eth::ChainSpec;

/// Validate and resolve a TRON GasFree provider config at platform activation time.
pub fn resolve_tron_gasless_provider(
    chain_spec: &ChainSpec,
    provider_config: Option<&TronGaslessProviderConfig>,
) -> Result<Option<TronGaslessProviderConfig>, TronGaslessConfigError> {
    let config = match provider_config {
        Some(cfg) => cfg,
        None => return Ok(None),
    };

    // GasFree is TRON-only
    if !matches!(chain_spec, ChainSpec::Tron { .. }) {
        return Err(TronGaslessConfigError::UnsupportedChain {
            chain: chain_spec.kind().to_string(),
        });
    }

    Ok(Some(config.clone()))
}
