use crate::utxo::tx_cache::{TxCacheError, TxCacheResult, UtxoVerboseCacheOps};
use async_trait::async_trait;
use common::log::LogOnError;
use futures::lock::Mutex as AsyncMutex;
use futures::FutureExt;
use mm2_err_handle::prelude::*;
use mm2_io::fs::{read_json, write_json, FsJsonError};
use parking_lot::Mutex as PaMutex;
use rpc::v1::types::{Transaction as RpcTransaction, H256 as H256Json};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

lazy_static! {
    static ref TX_CACHE_LOCK: TxCacheLock = TxCacheLock::default();
}

impl From<FsJsonError> for TxCacheError {
    fn from(e: FsJsonError) -> Self {
        match e {
            FsJsonError::IoReading(loading) => TxCacheError::ErrorLoading(loading.to_string()),
            FsJsonError::IoWriting(writing) => TxCacheError::ErrorSaving(writing.to_string()),
            FsJsonError::Serializing(ser) => TxCacheError::ErrorSerializing(ser.to_string()),
            FsJsonError::Deserializing(de) => TxCacheError::ErrorDeserializing(de.to_string()),
        }
    }
}

/// The cache lock is used to avoid reading and writing the same files at the same time.
#[derive(Default)]
struct TxCacheLock {
    /// The collection of `Ticker -> Mutex` pairs.
    mutexes: PaMutex<HashMap<String, Arc<AsyncMutex<()>>>>,
}

impl TxCacheLock {
    /// Get the mutex corresponding to the specified `ticker`.
    pub fn mutex_by_ticker(&self, ticker: &str) -> Arc<AsyncMutex<()>> {
        self.mutexes
            .lock()
            .entry(ticker.to_owned())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }
}

/// The cache instance that assigned to a specified coin.
///
/// Please note [`UtxoVerboseCache::ticker`] may not equal to [`Coin::ticker`].
/// In particular, `QRC20` tokens have the same transactions as `Qtum` coin,
/// so [`Qrc20Coin::platform_ticker`] is used as [`UtxoVerboseCache::ticker`].
#[derive(Debug)]
pub struct FsVerboseCache {
    ticker: String,
    tx_cache_path: PathBuf,
}

#[async_trait]
impl UtxoVerboseCacheOps for FsVerboseCache {
    async fn load_transactions_from_cache_concurrently(
        &self,
        tx_ids: HashSet<H256Json>,
    ) -> HashMap<H256Json, TxCacheResult<Option<RpcTransaction>>> {
        let mutex = TX_CACHE_LOCK.mutex_by_ticker(&self.ticker);
        let _lock = mutex.lock().await;

        let it = tx_ids
            .into_iter()
            .map(|txid| self.load_transaction_from_cache(txid).map(move |res| (txid, res)));
        futures::future::join_all(it).await.into_iter().collect()
    }

    async fn cache_transactions_concurrently(&self, txs: &HashMap<H256Json, RpcTransaction>) {
        let mutex = TX_CACHE_LOCK.mutex_by_ticker(&self.ticker);
        let _lock = mutex.lock().await;

        let it = txs.values().map(|tx| self.cache_transaction(tx));
        futures::future::join_all(it)
            .await
            .into_iter()
            .for_each(|tx| tx.error_log());
    }
}

impl FsVerboseCache {
    #[inline]
    pub fn new(ticker: String, tx_cache_path: PathBuf) -> FsVerboseCache {
        FsVerboseCache { ticker, tx_cache_path }
    }

    /// Tries to load transaction from cache.
    /// Note: `tx.confirmations` can be out-of-date.
    async fn load_transaction_from_cache(&self, txid: H256Json) -> TxCacheResult<Option<RpcTransaction>> {
        let path = self.cached_transaction_path(&txid);
        read_json(&path).await.mm_err(TxCacheError::from)
    }

    /// Uploads transaction to cache.
    async fn cache_transaction(&self, tx: &RpcTransaction) -> TxCacheResult<()> {
        const USE_TMP_FILE: bool = true;

        let path = self.cached_transaction_path(&tx.txid);
        write_json(tx, &path, USE_TMP_FILE).await.mm_err(TxCacheError::from)
    }

    #[inline]
    fn cached_transaction_path(&self, txid: &H256Json) -> PathBuf {
        self.tx_cache_path.join(format!("{txid:?}"))
    }
}
