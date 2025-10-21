//! RPC activation and deactivation of the orderbook streamer.
use super::EnableStreamingResponse;
use crate::lp_ordermatch::orderbook_events::OrderbookStreamer;
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::{map_to_mm::MapToMmResult, mm_error::MmResult};
use mm2_event_stream::DeriveStreamerId;

use common::HttpStatusCode;
use http::StatusCode;

#[derive(Deserialize)]
pub struct EnableOrderbookStreamingRequest {
    pub client_id: u64,
    pub base: String,
    pub rel: String,
}

#[derive(Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum OrderbookStreamingRequestError {
    EnableError(String),
}

impl HttpStatusCode for OrderbookStreamingRequestError {
    fn status_code(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

pub async fn enable_orderbook(
    ctx: MmArc,
    req: EnableOrderbookStreamingRequest,
) -> MmResult<EnableStreamingResponse, OrderbookStreamingRequestError> {
    let order_status_streamer = OrderbookStreamer::new((ctx.clone(), req.base, req.rel));
    ctx.event_stream_manager
        .add(req.client_id, order_status_streamer, ctx.spawner())
        .await
        .map(EnableStreamingResponse::new)
        .map_to_mm(|e| OrderbookStreamingRequestError::EnableError(format!("{e:?}")))
}
