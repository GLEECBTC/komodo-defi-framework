//! RPC activation and deactivation for Tx history event streamers.
use super::{EnableStreamingRequest, EnableStreamingResponse};

use coins::utxo::tx_history_events::TxHistoryEventStreamer;
use coins::z_coin::tx_history_events::ZCoinTxHistoryEventStreamer;
use coins::{lp_coinfind, MmCoin, MmCoinEnum};
use common::HttpStatusCode;
use derive_more::Display;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::{map_to_mm::MapToMmResult, mm_error::MmResult};
use mm2_event_stream::DeriveStreamerId;

#[derive(Deserialize)]
pub struct EnableTxHistoryStreamingRequest {
    pub coin: String,
}

#[derive(Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum TxHistoryStreamingRequestError {
    EnableError(String),
    CoinNotFound,
    CoinNotSupported,
    Internal(String),
}

impl HttpStatusCode for TxHistoryStreamingRequestError {
    fn status_code(&self) -> StatusCode {
        match self {
            TxHistoryStreamingRequestError::EnableError(_) => StatusCode::BAD_REQUEST,
            TxHistoryStreamingRequestError::CoinNotFound => StatusCode::NOT_FOUND,
            TxHistoryStreamingRequestError::CoinNotSupported => StatusCode::NOT_IMPLEMENTED,
            TxHistoryStreamingRequestError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

pub async fn enable_tx_history(
    ctx: MmArc,
    req: EnableStreamingRequest<EnableTxHistoryStreamingRequest>,
) -> MmResult<EnableStreamingResponse, TxHistoryStreamingRequestError> {
    let (client_id, req) = (req.client_id, req.inner);
    let coin = lp_coinfind(&ctx, &req.coin)
        .await
        .map_err(TxHistoryStreamingRequestError::Internal)?
        .ok_or(TxHistoryStreamingRequestError::CoinNotFound)?;

    let enable_result = match coin {
        MmCoinEnum::UtxoCoin(coin) => {
            let streamer = TxHistoryEventStreamer::new(req.coin);
            ctx.event_stream_manager.add(client_id, streamer, coin.spawner()).await
        },
        MmCoinEnum::Bch(coin) => {
            let streamer = TxHistoryEventStreamer::new(req.coin);
            ctx.event_stream_manager.add(client_id, streamer, coin.spawner()).await
        },
        MmCoinEnum::QtumCoin(coin) => {
            let streamer = TxHistoryEventStreamer::new(req.coin);
            ctx.event_stream_manager.add(client_id, streamer, coin.spawner()).await
        },
        MmCoinEnum::Tendermint(coin) => {
            // The tx history streamer is very primitive reactive streamer that only emits new txs.
            // it's logic is exactly the same for utxo coins and tendermint coins as well.
            let streamer = TxHistoryEventStreamer::new(req.coin);
            ctx.event_stream_manager.add(client_id, streamer, coin.spawner()).await
        },
        MmCoinEnum::ZCoin(coin) => {
            let streamer = ZCoinTxHistoryEventStreamer::new(coin.clone());
            ctx.event_stream_manager.add(client_id, streamer, coin.spawner()).await
        },
        _ => Err(TxHistoryStreamingRequestError::CoinNotSupported)?,
    };

    enable_result
        .map(EnableStreamingResponse::new)
        .map_to_mm(|e| TxHistoryStreamingRequestError::EnableError(format!("{e:?}")))
}
