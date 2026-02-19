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

/// Integration tests using real TRON testnet (Nile).
/// These tests require network access and are gated behind the `tron-network-tests` feature.
/// Run with: `cargo test -p coins --features tron-network-tests --lib tron_nile`
#[cfg(all(test, feature = "tron-network-tests"))]
mod api_integration_tests;

pub use address::Address as TronAddress;
pub use api::{TaposBlockData, TronApiClient, TronHttpClient, TronHttpNode};

use serde::{Deserialize, Serialize};

pub const TRX_DECIMALS: u8 = 6;

/// Represents TRON chain/network.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Network {
    Mainnet,
    Shasta,
    Nile,
}
