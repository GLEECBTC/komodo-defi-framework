//! RPC activation and deactivation for different balance event streamers.
use super::{EnableStreamingRequest, EnableStreamingResponse};

use coins::eth::eth_balance_events::EthBalanceEventStreamer;
use coins::tendermint::tendermint_balance_events::TendermintBalanceEventStreamer;
use coins::utxo::utxo_balance_events::UtxoBalanceEventStreamer;
use coins::z_coin::z_balance_streaming::ZCoinBalanceEventStreamer;
use coins::{lp_coinfind, MmCoin, MmCoinEnum};
use common::HttpStatusCode;
use derive_more::Display;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::{map_to_mm::MapToMmResult, mm_error::MmResult};
use mm2_event_stream::DeriveStreamerId;

use serde_json::Value as Json;

#[derive(Deserialize)]
pub struct EnableBalanceStreamingRequest {
    pub coin: String,
    pub config: Option<Json>,
}

#[derive(Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum BalanceStreamingRequestError {
    EnableError(String),
    CoinNotFound,
    CoinNotSupported,
    Internal(String),
}

impl HttpStatusCode for BalanceStreamingRequestError {
    fn status_code(&self) -> StatusCode {
        match self {
            BalanceStreamingRequestError::EnableError(_) => StatusCode::BAD_REQUEST,
            BalanceStreamingRequestError::CoinNotFound => StatusCode::NOT_FOUND,
            BalanceStreamingRequestError::CoinNotSupported => StatusCode::NOT_IMPLEMENTED,
            BalanceStreamingRequestError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

pub async fn enable_balance(
    ctx: MmArc,
    req: EnableStreamingRequest<EnableBalanceStreamingRequest>,
) -> MmResult<EnableStreamingResponse, BalanceStreamingRequestError> {
    let (client_id, req) = (req.client_id, req.inner);
    let coin = lp_coinfind(&ctx, &req.coin)
        .await
        .map_err(BalanceStreamingRequestError::Internal)?
        .ok_or(BalanceStreamingRequestError::CoinNotFound)?;

    match coin {
        MmCoinEnum::EthCoinVariant(_) => (),
        MmCoinEnum::ZCoinVariant(_)
        | MmCoinEnum::UtxoCoinVariant(_)
        | MmCoinEnum::BchVariant(_)
        | MmCoinEnum::QtumCoinVariant(_)
        | MmCoinEnum::TendermintVariant(_) => {
            if req.config.is_some() {
                Err(BalanceStreamingRequestError::EnableError(
                    "Invalid config provided. No config needed".to_string(),
                ))?
            }
        },
        _ => Err(BalanceStreamingRequestError::CoinNotSupported)?,
    }

    let enable_result = match coin {
        MmCoinEnum::UtxoCoinVariant(coin) => {
            let streamer = UtxoBalanceEventStreamer::new(coin.clone().into());
            ctx.event_stream_manager.add(client_id, streamer, coin.spawner()).await
        },
        MmCoinEnum::BchVariant(coin) => {
            let streamer = UtxoBalanceEventStreamer::new(coin.clone().into());
            ctx.event_stream_manager.add(client_id, streamer, coin.spawner()).await
        },
        MmCoinEnum::QtumCoinVariant(coin) => {
            let streamer = UtxoBalanceEventStreamer::new(coin.clone().into());
            ctx.event_stream_manager.add(client_id, streamer, coin.spawner()).await
        },
        MmCoinEnum::EthCoinVariant(coin) => {
            let streamer = EthBalanceEventStreamer::try_new(req.config, coin.clone())
                .map_to_mm(|e| BalanceStreamingRequestError::EnableError(format!("{e:?}")))?;
            ctx.event_stream_manager.add(client_id, streamer, coin.spawner()).await
        },
        MmCoinEnum::ZCoinVariant(coin) => {
            let streamer = ZCoinBalanceEventStreamer::new(coin.clone());
            ctx.event_stream_manager.add(client_id, streamer, coin.spawner()).await
        },
        MmCoinEnum::TendermintVariant(coin) => {
            let streamer = TendermintBalanceEventStreamer::new(coin.clone());
            ctx.event_stream_manager.add(client_id, streamer, coin.spawner()).await
        },
        _ => Err(BalanceStreamingRequestError::CoinNotSupported)?,
    };

    enable_result
        .map(EnableStreamingResponse::new)
        .map_to_mm(|e| BalanceStreamingRequestError::EnableError(format!("{e:?}")))
}
