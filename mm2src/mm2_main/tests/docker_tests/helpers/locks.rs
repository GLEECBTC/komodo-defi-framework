//! Coin funding locks for docker tests.
//!
//! These locks prevent concurrent funding operations that would cause RPC failures
//! (insufficient funds, nonce reuse, transaction confirmation race conditions).
//!
//! All coin-specific locks are centralized here to:
//! - Remove cross-module coupling between helper modules
//! - Make it clear which coins share locks
//! - Provide a single location for lock documentation

use tokio::sync::Mutex as AsyncMutex;

lazy_static! {
    // =========================================================================
    // UTXO coin locks
    // =========================================================================

    /// Lock for MYCOIN funding operations
    pub static ref MYCOIN_LOCK: AsyncMutex<()> = AsyncMutex::new(());

    /// Lock for MYCOIN1 funding operations
    pub static ref MYCOIN1_LOCK: AsyncMutex<()> = AsyncMutex::new(());

    /// Lock for FORSLP (BCH/SLP) funding operations
    pub static ref FORSLP_LOCK: AsyncMutex<()> = AsyncMutex::new(());

    // =========================================================================
    // Qtum/QRC20 lock
    // =========================================================================

    /// Lock for Qtum/QRC20 funding operations.
    /// Shared by QTUM, QICK, and QORTY coins since they all run on the same Qtum node.
    pub static ref QTUM_LOCK: AsyncMutex<()> = AsyncMutex::new(());

    // =========================================================================
    // ZCoin locks
    // =========================================================================

    /// Lock for ZCoin generation TX (address 1)
    pub static ref ZCOIN_GEN_TX_LOCK: AsyncMutex<()> = AsyncMutex::new(());

    /// Lock for ZCoin generation TX (address 2)
    pub static ref ZCOIN_GEN_TX_LOCK_ADDR2: AsyncMutex<()> = AsyncMutex::new(());
}

/// Get the appropriate funding lock for a given ticker.
///
/// This centralizes the ticker-to-lock mapping and provides a clear error
/// message when an unknown ticker is used.
pub fn get_funding_lock(ticker: &str) -> &'static AsyncMutex<()> {
    match ticker {
        "MYCOIN" => &MYCOIN_LOCK,
        "MYCOIN1" => &MYCOIN1_LOCK,
        "FORSLP" => &FORSLP_LOCK,
        "QTUM" | "QICK" | "QORTY" => &QTUM_LOCK,
        _ => panic!("No funding lock defined for ticker: {}", ticker),
    }
}
