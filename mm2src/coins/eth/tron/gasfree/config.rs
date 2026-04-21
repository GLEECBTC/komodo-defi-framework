use crate::eth::tron::TronAddress;
use serde::Deserialize;
use std::str::FromStr;
use url::Url;

use super::error::TronGaslessConfigError;

const DEFAULT_REQUEST_TIMEOUT_MS: u64 = 15_000;
const DEFAULT_STATUS_POLL_INTERVAL_MS: u64 = 3_000;

/// Configuration for a TRON GasFree provider, supplied at platform activation time.
///
/// Contains API credentials that MUST NOT appear in Debug output or serialized responses.
#[derive(Clone, Deserialize)]
pub struct TronGaslessProviderConfig {
    /// Host-only GasFree API URL (e.g. `https://open.gasfree.io`).
    /// The resolver appends `/tron/`, `/nile/`, or `/shasta/` based on the activated TRON network.
    /// A user-supplied path segment is rejected as `InvalidBaseUrl` at activation.
    pub base_url: Url,
    /// API key for authentication.
    pub api_key: String,
    /// API secret for HMAC-SHA256 signing.
    pub api_secret: String,
    /// Service provider TRON address for TIP-712 PermitTransferMessage.
    /// Validated as a parseable TRON address at activation time.
    // TODO: Remove once the API-fetch path is implemented — fetch from
    // `/api/v1/config/provider/all` at activation time instead of requiring user input.
    pub service_provider: String,
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

impl TronGaslessProviderConfig {
    /// Parse `service_provider` as a TRON address.
    pub fn service_provider_address(&self) -> Result<TronAddress, TronGaslessConfigError> {
        TronAddress::from_str(&self.service_provider)
            .map_err(|e| TronGaslessConfigError::InvalidServiceProvider { reason: e.to_string() })
    }
}

/// Fully-validated GasFree provider state stored on `EthCoinImpl` after activation.
///
/// `service_provider` is parsed from the user-supplied config.
/// `verifying_contract` is derived from the hardcoded per-network controller.
/// Construction is only possible via
/// [`resolve_tron_gasless_provider`](super::resolve_tron_gasless_provider).
#[derive(Clone)]
pub struct ResolvedTronGaslessProvider {
    raw: TronGaslessProviderConfig,
    service_provider: TronAddress,
    verifying_contract: TronAddress,
}

impl ResolvedTronGaslessProvider {
    /// Construct a resolved provider. Only callable from the gasfree module.
    pub(super) fn new(
        raw: TronGaslessProviderConfig,
        service_provider: TronAddress,
        verifying_contract: TronAddress,
    ) -> Self {
        ResolvedTronGaslessProvider {
            raw,
            service_provider,
            verifying_contract,
        }
    }

    /// TODO(Commit 7, Commit 8): Consumed by Commit 7 (`GasfreeSubmitRequest.service_provider`)
    /// and Commit 8 (TIP-712 `serviceProvider` field). Pre-parsed at activation by design.
    pub fn service_provider(&self) -> &TronAddress {
        &self.service_provider
    }

    /// TODO(Commit 8): Consumed by TIP-712 `PermitTransferMessage` signing
    /// (`verifyingContract` domain field). Pre-parsed at activation by design;
    /// see the Commit 3 plan section for rationale.
    pub fn verifying_contract(&self) -> &TronAddress {
        &self.verifying_contract
    }

    pub fn config(&self) -> &TronGaslessProviderConfig {
        &self.raw
    }
}

/// Redacted Debug — delegates to the raw config's redacted Debug.
impl std::fmt::Debug for ResolvedTronGaslessProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedTronGaslessProvider")
            .field("raw", &self.raw)
            .field("service_provider", &self.service_provider)
            .finish_non_exhaustive()
    }
}
