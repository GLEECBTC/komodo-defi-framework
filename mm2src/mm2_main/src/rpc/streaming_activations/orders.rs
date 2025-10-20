//! RPC activation and deactivation of the order status streamer.
use crate::lp_ordermatch::order_events::OrderStatusStreamer;

use super::{EnableStreamingRequest, EnableStreamingResponse};
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::{map_to_mm::MapToMmResult, mm_error::MmResult};
use mm2_event_stream::DeriveStreamerId;

use common::HttpStatusCode;
use http::StatusCode;

#[derive(Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum OrderStatusStreamingRequestError {
    EnableError(String),
}

impl HttpStatusCode for OrderStatusStreamingRequestError {
    fn status_code(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

pub async fn enable_order_status(
    ctx: MmArc,
    req: EnableStreamingRequest<()>,
) -> MmResult<EnableStreamingResponse, OrderStatusStreamingRequestError> {
    let order_status_streamer = OrderStatusStreamer::new(());
    ctx.event_stream_manager
        .add(req.client_id, order_status_streamer, ctx.spawner())
        .await
        .map(EnableStreamingResponse::new)
        .map_to_mm(|e| OrderStatusStreamingRequestError::EnableError(format!("{e:?}")))
}
