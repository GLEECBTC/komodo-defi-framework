//! TRON blockchain support for EthCoin integration.
//!
//! TRON uses a 21-byte address format (0x41 prefix + 20 bytes) displayed as Base58Check.
//! Native currency is TRX with 6 decimals (1 TRX = 1,000,000 SUN).

mod address;
pub mod api;

/// Integration tests using real TRON testnet (Nile).
/// These tests require network access and are gated behind the `tron-network-tests` feature.
/// Run with: `cargo test -p coins --features tron-network-tests --lib tron_nile`
#[cfg(all(test, feature = "tron-network-tests"))]
mod api_integration_tests;

pub use address::Address as TronAddress;
pub use api::{TronApiClient, TronHttpClient, TronHttpNode};

use ethereum_types::U256;
use serde::{Deserialize, Serialize};

pub const TRX_DECIMALS: u8 = 6;
const ONE_TRX: u64 = 1_000_000; // 1 TRX = 1,000,000 SUN

/// Represents TRON chain/network.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Network {
    Mainnet,
    Shasta,
    Nile,
}

/// Convert TRX to SUN using U256 type.
#[allow(dead_code)]
fn trx_to_sun_u256(trx: u64) -> U256 {
    U256::from(trx) * U256::from(ONE_TRX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trx_to_sun_conversion() {
        // Zero TRX
        assert_eq!(trx_to_sun_u256(0), U256::zero());
        // 1 TRX = 1,000,000 SUN
        assert_eq!(trx_to_sun_u256(1), U256::from(1_000_000u64));
        // 100 TRX
        assert_eq!(trx_to_sun_u256(100), U256::from(100_000_000u64));
        // Total TRX supply is ~95 billion, but we test u64::MAX for extra safety
        // u64::MAX * 1_000_000 = 18,446,744,073,709,551,615,000,000
        let max_sun = trx_to_sun_u256(u64::MAX);
        assert_eq!(max_sun, U256::from_dec_str("18446744073709551615000000").unwrap());
    }
}
