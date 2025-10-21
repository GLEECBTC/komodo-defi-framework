cfg_native!(
    use crate::z_coin::ZcoinConsensusParams;

    pub mod wallet_sql_storage;

    use zcash_client_sqlite::for_async::WalletDbAsync;
);

#[cfg(target_arch = "wasm32")]
pub mod wasm;
#[cfg(target_arch = "wasm32")]
use wasm::storage::WalletIndexedDb;

#[derive(Clone)]
pub struct WalletDbShared {
    #[cfg(not(target_arch = "wasm32"))]
    pub db: WalletDbAsync<ZcoinConsensusParams>,
    #[cfg(target_arch = "wasm32")]
    pub db: WalletIndexedDb,
    #[allow(unused)]
    ticker: String,
}
