//! RPC activation and deactivation for the shutdown signals.
use super::{EnableStreamingRequest, EnableStreamingResponse};

use crate::shutdown_signal_event::ShutdownSignalEvent;
use common::HttpStatusCode;
use derive_more::Display;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::{map_to_mm::MapToMmResult, mm_error::MmResult};

#[derive(Deserialize)]
pub struct EnableShutdownSignalRequest;

#[derive(Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum ShutdownSignalRequestError {
    EnableError(String),
}

impl HttpStatusCode for ShutdownSignalRequestError {
    fn status_code(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

pub async fn enable_shutdown_signal(
    ctx: MmArc,
    req: EnableStreamingRequest<EnableShutdownSignalRequest>,
) -> MmResult<EnableStreamingResponse, ShutdownSignalRequestError> {
    ctx.event_stream_manager
        .add(req.client_id, ShutdownSignalEvent, ctx.spawner())
        .await
        .map(EnableStreamingResponse::new)
        .map_to_mm(|e| ShutdownSignalRequestError::EnableError(format!("{e:?}")))
}
