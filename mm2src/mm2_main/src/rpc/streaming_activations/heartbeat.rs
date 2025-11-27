//! RPC activation and deactivation for the heartbeats.
use super::{EnableStreamingRequest, EnableStreamingResponse};

use crate::heartbeat_event::{HeartbeatEvent, HeartbeatEventConfig};
use common::HttpStatusCode;
use derive_more::Display;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::{map_to_mm::MapToMmResult, mm_error::MmResult};

#[derive(Deserialize)]
pub struct EnableHeartbeatRequest {
    pub config: HeartbeatEventConfig,
}

#[derive(Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum HeartbeatRequestError {
    EnableError(String),
}

impl HttpStatusCode for HeartbeatRequestError {
    fn status_code(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

pub async fn enable_heartbeat(
    ctx: MmArc,
    req: EnableStreamingRequest<EnableHeartbeatRequest>,
) -> MmResult<EnableStreamingResponse, HeartbeatRequestError> {
    let (client_id, req) = (req.client_id, req.inner);
    let heartbeat_streamer = HeartbeatEvent::new(req.config);
    ctx.event_stream_manager
        .add(client_id, heartbeat_streamer, ctx.spawner())
        .await
        .map(EnableStreamingResponse::new)
        .map_to_mm(|e| HeartbeatRequestError::EnableError(format!("{e:?}")))
}
