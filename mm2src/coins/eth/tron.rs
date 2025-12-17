//! Minimal Tron placeholders for EthCoin integration.
//! These types will be expanded with full TRON logic in later steps.

mod address;
pub use address::Address as TronAddress;

use ethereum_types::U256;

pub const TRX_DECIMALS: u8 = 6;
const ONE_TRX: u64 = 1_000_000; // 1 TRX = 1,000,000 SUN

/// Represents TRON chain/network.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Network {
    Mainnet,
    Shasta,
    Nile,
    // TODO: Add more networks as needed.
}

/// Draft TRON clients structure.
#[derive(Clone, Debug)]
pub struct TronClients {
    pub clients: Vec<TronClient>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct TronClient {
    pub endpoint: String,
    pub network: Network,
    #[serde(default)]
    pub komodo_proxy: bool, // should be true for any net which requires api key
}

/// Placeholder for TRON fee params.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TronFeeParams {
    // TODO: Add TRON-specific fields in future steps.
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
