use async_trait::async_trait;
use derive_more::Display;
use mm2_err_handle::prelude::*;
use rpc::v1::types::{Transaction as RpcTransaction, H256 as H256Json};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;

pub mod dummy_tx_cache;
#[cfg(not(target_arch = "wasm32"))]
pub mod fs_tx_cache;

#[cfg(target_arch = "wasm32")]
pub mod wasm_tx_cache {
    pub type WasmVerboseCache = crate::utxo::tx_cache::dummy_tx_cache::DummyVerboseCache;
}

pub type TxCacheResult<T> = MmResult<T, TxCacheError>;
pub type UtxoVerboseCacheShared = Arc<dyn UtxoVerboseCacheOps + Send + Sync + 'static>;

#[derive(Debug, Display)]
pub enum TxCacheError {
    ErrorLoading(String),
    ErrorSaving(String),
    ErrorDeserializing(String),
    ErrorSerializing(String),
}

#[async_trait]
pub trait UtxoVerboseCacheOps: fmt::Debug {
    #[inline]
    fn into_shared(self) -> UtxoVerboseCacheShared
    where
        Self: Sized + Send + Sync + 'static,
    {
        Arc::new(self)
    }

    /// Tries to load transactions from cache concurrently.
    /// Please note `tx.confirmations` can be out-of-date.
    async fn load_transactions_from_cache_concurrently(
        &self,
        tx_ids: HashSet<H256Json>,
    ) -> HashMap<H256Json, TxCacheResult<Option<RpcTransaction>>>;

    /// Uploads transactions to cache concurrently.
    async fn cache_transactions_concurrently(&self, txs: &HashMap<H256Json, RpcTransaction>);
}
