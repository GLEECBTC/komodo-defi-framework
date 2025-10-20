//! Provides estimations of base and priority fee per gas or fetch estimations from a gas api provider
pub mod block_native;
pub mod infura;
pub mod simple;

use ethereum_types::U256;
use url::Url;

pub(crate) const FEE_PRIORITY_LEVEL_N: usize = 3;

/// Indicates which provider was used to get fee per gas estimations
#[derive(Clone, Debug)]
pub enum EstimationSource {
    /// filled by default values
    Empty,
    /// internal simple estimator
    Simple,
    Infura,
    Blocknative,
}

impl std::fmt::Display for EstimationSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EstimationSource::Empty => write!(f, "empty"),
            EstimationSource::Simple => write!(f, "simple"),
            EstimationSource::Infura => write!(f, "infura"),
            EstimationSource::Blocknative => write!(f, "blocknative"),
        }
    }
}

impl Default for EstimationSource {
    fn default() -> Self {
        Self::Empty
    }
}

enum PriorityLevelId {
    Low = 0,
    Medium = 1,
    High = 2,
}

/// Supported gas api providers
#[derive(Clone, Deserialize)]
pub enum GasApiProvider {
    Infura,
    Blocknative,
}

#[derive(Clone, Deserialize)]
pub struct GasApiConfig {
    /// gas api provider name to use
    pub provider: GasApiProvider,
    /// gas api provider or proxy base url (scheme, host and port without the relative part)
    pub url: Url,
}

/// Priority level estimated max fee per gas
#[derive(Clone, Debug, Default)]
pub struct FeePerGasLevel {
    /// estimated max priority tip fee per gas in wei
    pub max_priority_fee_per_gas: U256,
    /// estimated max fee per gas in wei
    pub max_fee_per_gas: U256,
    /// estimated transaction min wait time in mempool in ms for this priority level
    pub min_wait_time: Option<u32>,
    /// estimated transaction max wait time in mempool in ms for this priority level
    pub max_wait_time: Option<u32>,
}

/// Internal struct for estimated fee per gas for several priority levels, in wei
/// low/medium/high levels are supported
#[derive(Default, Debug, Clone)]
pub struct FeePerGasEstimated {
    /// base fee for the next block in wei
    pub base_fee: U256,
    /// estimated low priority fee
    pub low: FeePerGasLevel,
    /// estimated medium priority fee
    pub medium: FeePerGasLevel,
    /// estimated high priority fee
    pub high: FeePerGasLevel,
    /// which estimator used
    pub source: EstimationSource,
    /// base trend (up or down)
    pub base_fee_trend: String,
    /// priority trend (up or down)
    pub priority_fee_trend: String,
}
