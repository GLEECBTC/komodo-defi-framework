//! TRON blockchain support for EthCoin integration.
//!
//! TRON uses a 21-byte address format (0x41 prefix + 20 bytes) displayed as Base58Check.
//! Native currency is TRX with 6 decimals (1 TRX = 1,000,000 SUN).

mod address;
pub mod api;
pub mod fee;
pub(crate) mod proto;
pub(crate) mod sign;
pub mod tx_builder;
pub mod withdraw;

/// Integration tests using real TRON testnet (Nile).
/// These tests require network access and are gated behind the `tron-network-tests` feature.
/// Run with: `cargo test -p coins --features tron-network-tests --lib tron_nile`
#[cfg(all(test, feature = "tron-network-tests"))]
mod api_integration_tests;

pub use address::Address as TronAddress;
pub use api::{BroadcastHexResponse, TaposBlockData, TronApiClient, TronHttpClient, TronHttpNode};

use ethabi::Token;
use ethereum_types::U256;
use serde::{Deserialize, Serialize};

pub const TRX_DECIMALS: u8 = 6;

/// Build ABI tokens for a TRC20 `transfer(address,uint256)` call.
///
/// Shared by `tx_builder` (full ABI encoding with selector) and `api` (parameter-only encoding).
pub(crate) fn trc20_transfer_tokens(recipient: &TronAddress, amount: U256) -> [Token; 2] {
    [Token::Address(recipient.to_evm_address()), Token::Uint(amount)]
}

/// Represents TRON chain/network.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Network {
    Mainnet,
    Shasta,
    Nile,
}

/// Hard cap on TRON raw transaction size to prevent oversized-input DoS.
/// Typical TRON transactions are a few hundred bytes; 256 KiB is generous.
pub const MAX_TRON_RAW_TX_BYTES: usize = 256 * 1024;

/// Strips optional `0x`/`0X` prefix and validates the hex string for TRON broadcast.
///
/// Checks: non-empty, bounded length, even character count, ASCII hex digits only.
pub fn normalize_tron_raw_tx_hex(input: &str) -> Result<String, String> {
    let s = input
        .strip_prefix("0x")
        .or_else(|| input.strip_prefix("0X"))
        .unwrap_or(input);

    if s.is_empty() {
        return Err("TRON raw transaction hex is empty".to_owned());
    }
    if s.len() > MAX_TRON_RAW_TX_BYTES * 2 {
        return Err(format!(
            "TRON raw transaction hex too large: {} chars (max {})",
            s.len(),
            MAX_TRON_RAW_TX_BYTES * 2,
        ));
    }
    if !s.len().is_multiple_of(2) {
        return Err("TRON raw transaction hex has odd length".to_owned());
    }
    if !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("TRON raw transaction hex contains non-hex characters".to_owned());
    }
    Ok(s.to_owned())
}

/// Validates that TRON raw transaction bytes are non-empty and within the size limit.
pub fn validate_tron_raw_tx_len(len: usize) -> Result<(), String> {
    if len == 0 {
        return Err("TRON raw transaction bytes are empty".to_owned());
    }
    if len > MAX_TRON_RAW_TX_BYTES {
        return Err(format!(
            "TRON raw transaction too large: {} bytes (max {})",
            len, MAX_TRON_RAW_TX_BYTES,
        ));
    }
    Ok(())
}

/// Shared test fixtures for TRON unit tests.
#[cfg(test)]
pub(super) mod test_fixtures {
    use super::api::TaposBlockData;
    use super::fee::{DestAccountState, TronChainPrices};

    pub const TEST_FROM_HEX: &str = "4123b00d15c601b30613bf5a3b2f72527c79cc08b6";
    pub const TEST_TO_HEX: &str = "418840e6c55b9ada326d211d818c34a994aeced808";

    /// Nile testnet block 64,687,673 — used as TAPOS source in golden vector tests.
    pub fn nile_block_64687673() -> TaposBlockData {
        TaposBlockData {
            number: 64_687_673,
            block_id: {
                let bytes = hex::decode("0000000003db0e39901ce5715271b601b1c57055f5d8fa6a9fe3505eee560308").unwrap();
                let mut arr = [0u8; 32];
                arr.copy_from_slice(&bytes);
                arr
            },
            timestamp: 1_770_522_369_000,
        }
    }

    /// Mainnet-like chain prices for tests.
    pub fn mainnet_prices() -> TronChainPrices {
        TronChainPrices {
            bandwidth_price_sun: 1_000,
            energy_price_sun: 420,
            create_new_account_fee_sun: 1_000_000,     // 1 TRX
            create_account_bandwidth_fee_sun: 100_000, // 0.1 TRX
            create_new_account_bandwidth_rate: 1,
        }
    }

    /// `DestAccountState` for an unactivated address with mainnet-like fees.
    pub fn new_account_state() -> DestAccountState {
        DestAccountState::NewAccount {
            creation_fee_sun: 1_000_000,     // 1 TRX
            bandwidth_fallback_sun: 100_000, // 0.1 TRX
            bandwidth_rate: 1,
        }
    }
}

#[cfg(test)]
mod raw_tx_validation_tests {
    use super::*;

    #[test]
    fn normalize_tron_raw_tx_hex_validates_input() {
        // Valid inputs: strips 0x/0X prefix, accepts bare hex
        assert_eq!(normalize_tron_raw_tx_hex("0xabcd").unwrap(), "abcd");
        assert_eq!(normalize_tron_raw_tx_hex("0Xabcd").unwrap(), "abcd");
        assert_eq!(normalize_tron_raw_tx_hex("abcd1234").unwrap(), "abcd1234");

        // Rejections: empty, prefix-only, odd length, non-hex, oversized
        assert!(normalize_tron_raw_tx_hex("").is_err());
        assert!(normalize_tron_raw_tx_hex("0x").is_err());
        assert!(normalize_tron_raw_tx_hex("abc").is_err());
        assert!(normalize_tron_raw_tx_hex("abcg").is_err());
        let oversized = "ab".repeat(MAX_TRON_RAW_TX_BYTES + 1);
        assert!(normalize_tron_raw_tx_hex(&oversized).is_err());
    }

    #[test]
    fn validate_tron_raw_tx_len_validates_bounds() {
        assert!(validate_tron_raw_tx_len(0).is_err());
        assert!(validate_tron_raw_tx_len(1000).is_ok());
        assert!(validate_tron_raw_tx_len(MAX_TRON_RAW_TX_BYTES + 1).is_err());
    }
}
