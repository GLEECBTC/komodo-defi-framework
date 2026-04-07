use serde::Deserialize;
use url::Url;

const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 15_000;
const DEFAULT_STATUS_POLL_INTERVAL_MS: u64 = 3_000;

/// Configuration for a TRON GasFree provider, supplied at platform activation time.
///
/// Contains API credentials that MUST NOT appear in Debug output or serialized responses.
#[derive(Clone, Deserialize)]
pub struct TronGaslessProviderConfig {
    /// Base URL for the GasFree API.
    pub base_url: Url,
    /// API key for authentication.
    pub api_key: String,
    /// API secret for HMAC-SHA256 signing.
    pub api_secret: String,
    /// Service provider TRON address for TIP-712 PermitTransferMessage.
    /// Per-network constant (same for all providers on a given network), but kept here
    /// rather than hardcoded because GasFree protocol upgrades could deploy new addresses.
    /// Can be obtained from the GasFree API (`/api/v1/config/provider/all`).
    // TODO: Maybe obtain this on activation and cache it, rather than requiring users to manually input it.
    pub service_provider: String,
    /// Verifying contract TRON address for TIP-712 domain and CREATE2 address computation.
    /// Per-network constant, but kept here rather than hardcoded because GasFree protocol
    /// upgrades could deploy new addresses.
    pub verifying_contract: String,
    /// Request timeout in milliseconds for GasFree API calls.
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
    /// Polling interval in milliseconds for transfer status checks.
    #[serde(default = "default_status_poll_interval_ms")]
    pub status_poll_interval_ms: u64,
}

fn default_request_timeout_ms() -> u64 {
    DEFAULT_REQUEST_TIMEOUT_MS
}
fn default_status_poll_interval_ms() -> u64 {
    DEFAULT_STATUS_POLL_INTERVAL_MS
}

/// Redacted Debug — secrets MUST NOT be logged.
impl std::fmt::Debug for TronGaslessProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TronGaslessProviderConfig")
            .field("base_url", &self.base_url)
            .finish_non_exhaustive()
    }
}
