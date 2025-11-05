use super::z_tx_history::fetch_txs_from_db;
use super::{NoInfoAboutTx, ZCoin, ZTxHistoryError, ZcoinTxDetails};
use crate::utxo::rpc_clients::UtxoRpcError;
use crate::MarketCoinOps;
use common::log;
use mm2_err_handle::prelude::MmError;
use mm2_event_stream::{Broadcaster, DeriveStreamerId, Event, EventStreamer, StreamHandlerInput, StreamerId};
use rpc::v1::types::H256 as H256Json;

use async_trait::async_trait;
use futures::channel::oneshot;
use futures::compat::Future01CompatExt;
use futures::StreamExt;
use zcash_client_backend::wallet::WalletTx;
use zcash_primitives::sapling::Nullifier;

pub struct ZCoinTxHistoryEventStreamer {
    coin: ZCoin,
}

impl<'a> DeriveStreamerId<'a> for ZCoinTxHistoryEventStreamer {
    type InitParam = ZCoin;
    type DeriveParam = &'a str;

    fn new(coin: Self::InitParam) -> Self {
        Self { coin }
    }

    #[inline(always)]
    fn derive_streamer_id(coin: Self::DeriveParam) -> StreamerId {
        StreamerId::TxHistory { coin: coin.to_string() }
    }
}

#[async_trait]
impl EventStreamer for ZCoinTxHistoryEventStreamer {
    type DataInType = Vec<WalletTx<Nullifier>>;

    fn streamer_id(&self) -> StreamerId {
        Self::derive_streamer_id(self.coin.ticker())
    }

    async fn handle(
        self,
        broadcaster: Broadcaster,
        ready_tx: oneshot::Sender<Result<(), String>>,
        mut data_rx: impl StreamHandlerInput<Self::DataInType>,
    ) {
        ready_tx
            .send(Ok(()))
            .expect("Receiver is dropped, which should never happen.");

        while let Some(new_txs) = data_rx.next().await {
            let new_txs_details = match get_tx_details(&self.coin, new_txs).await {
                Ok(tx_details) => tx_details,
                Err(e) => {
                    broadcaster.broadcast(Event::err(self.streamer_id(), json!({ "error": e.to_string() })));
                    log::error!("Failed to get tx details in streamer {}: {e:?}", self.streamer_id());
                    continue;
                },
            };
            for tx_details in new_txs_details {
                let tx_details = serde_json::to_value(tx_details).expect("Serialization should't fail.");
                broadcaster.broadcast(Event::new(self.streamer_id(), tx_details));
            }
        }
    }
}

/// Errors that can occur while getting transaction details for some tx hashes.
///
/// The error implements `Display` trait, so it can be easily converted `.to_string`.
#[derive(Debug, derive_more::Display)]
enum GetTxDetailsError {
    #[display(fmt = "RPC Error: {_0:?}")]
    UtxoRpcError(UtxoRpcError),
    #[display(fmt = "DB Error: {_0:?}")]
    DbError(String),
    #[display(fmt = "Internal Error: {_0:?}")]
    Internal(NoInfoAboutTx),
}

impl From<MmError<UtxoRpcError>> for GetTxDetailsError {
    fn from(e: MmError<UtxoRpcError>) -> Self {
        GetTxDetailsError::UtxoRpcError(e.into_inner())
    }
}

impl From<MmError<ZTxHistoryError>> for GetTxDetailsError {
    fn from(e: MmError<ZTxHistoryError>) -> Self {
        GetTxDetailsError::DbError(e.to_string())
    }
}

impl From<MmError<NoInfoAboutTx>> for GetTxDetailsError {
    fn from(e: MmError<NoInfoAboutTx>) -> Self {
        GetTxDetailsError::Internal(e.into_inner())
    }
}

async fn get_tx_details(coin: &ZCoin, txs: Vec<WalletTx<Nullifier>>) -> Result<Vec<ZcoinTxDetails>, GetTxDetailsError> {
    let current_block = coin.utxo_rpc_client().get_block_count().compat().await?;
    let txs_from_db = {
        let tx_ids = txs.iter().map(|tx| tx.txid).collect();
        fetch_txs_from_db(coin, tx_ids).await?
    };

    let hashes_for_verbose = txs_from_db
        .iter()
        .map(|item| H256Json::from(item.tx_hash.take()))
        .collect();
    let transactions = coin.z_transactions_from_cache_or_rpc(hashes_for_verbose).await?;

    let prev_tx_hashes = transactions
        .iter()
        .flat_map(|(_, tx)| {
            tx.vin.iter().map(|vin| {
                let mut hash = *vin.prevout.hash();
                hash.reverse();
                H256Json::from(hash)
            })
        })
        .collect();
    let prev_transactions = coin.z_transactions_from_cache_or_rpc(prev_tx_hashes).await?;

    let txs_details = txs_from_db
        .into_iter()
        .map(|tx_item| coin.tx_details_from_db_item(tx_item, &transactions, &prev_transactions, current_block))
        .collect::<Result<_, _>>()?;

    Ok(txs_details)
}
