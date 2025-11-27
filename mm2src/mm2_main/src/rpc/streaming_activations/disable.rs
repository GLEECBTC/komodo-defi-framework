//! The module for handling any event streaming deactivation requests.
//!
//! All event streamers are deactivated using the streamer ID only.

use common::HttpStatusCode;
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::{map_to_mm::MapToMmResult, mm_error::MmResult};
use mm2_event_stream::StreamerId;

use http::StatusCode;

/// The request used for any event streaming deactivation.
#[derive(Deserialize)]
pub struct DisableStreamingRequest {
    pub client_id: u64,
    pub streamer_id: StreamerId,
}

/// The success/ok response for any event streaming deactivation request.
#[derive(Serialize)]
pub struct DisableStreamingResponse {
    result: &'static str,
}

impl DisableStreamingResponse {
    fn new() -> Self {
        Self { result: "Success" }
    }
}

#[derive(Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
/// The error response for any event streaming deactivation request.
pub enum DisableStreamingRequestError {
    DisableError(String),
}

impl HttpStatusCode for DisableStreamingRequestError {
    fn status_code(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

/// Disables a streamer.
///
/// This works for any streamer regarding of their type/usage.
pub async fn disable_streamer(
    ctx: MmArc,
    req: DisableStreamingRequest,
) -> MmResult<DisableStreamingResponse, DisableStreamingRequestError> {
    ctx.event_stream_manager
        .stop(req.client_id, &req.streamer_id)
        .map_to_mm(|e| DisableStreamingRequestError::DisableError(format!("{e:?}")))?;
    Ok(DisableStreamingResponse::new())
}
