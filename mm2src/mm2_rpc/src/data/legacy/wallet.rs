use serde::{Deserialize, Serialize};

use mm2_number::BigDecimal;

#[derive(Serialize, Deserialize)]
pub struct BalanceResponse {
    pub coin: String,
    pub balance: BigDecimal,
    pub unspendable_balance: BigDecimal,
    pub address: String,
    /// TRON GasFree receive address derived locally via CREATE2.
    /// Only populated when this is a TRON coin with a configured GasFree provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gasfree_address: Option<String>,
}
