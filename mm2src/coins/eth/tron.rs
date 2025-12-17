//! Minimal Tron placeholders for EthCoin integration.
//! These types will be expanded with full TRON logic in later steps.

mod address;
pub use address::Address as TronAddress;

use ethereum_types::U256;

#[expect(dead_code)]
const TRX_DECIMALS: u32 = 6;
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

#[expect(dead_code)]
// Helper function to convert TRX to SUN using U256 type
// Returns None if multiplication would overflow
fn trx_to_sun_u256(trx: u64) -> Option<U256> {
    trx.checked_mul(ONE_TRX).map(U256::from)
}
