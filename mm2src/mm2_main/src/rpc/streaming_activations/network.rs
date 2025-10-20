//! RPC activation and deactivation for the network event streamer.
use super::{EnableStreamingRequest, EnableStreamingResponse};

use common::HttpStatusCode;
use derive_more::Display;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::{map_to_mm::MapToMmResult, mm_error::MmResult};
use mm2_libp2p::application::network_event::{NetworkEvent, NetworkEventConfig};

#[derive(Deserialize)]
pub struct EnableNetworkStreamingRequest {
    pub config: NetworkEventConfig,
}

#[derive(Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum NetworkStreamingRequestError {
    EnableError(String),
}

impl HttpStatusCode for NetworkStreamingRequestError {
    fn status_code(&self) -> StatusCode {
        StatusCode::BAD_REQUEST
    }
}

pub async fn enable_network(
    ctx: MmArc,
    req: EnableStreamingRequest<EnableNetworkStreamingRequest>,
) -> MmResult<EnableStreamingResponse, NetworkStreamingRequestError> {
    let (client_id, req) = (req.client_id, req.inner);
    let network_steamer = NetworkEvent::new(req.config, ctx.clone());
    ctx.event_stream_manager
        .add(client_id, network_steamer, ctx.spawner())
        .await
        .map(EnableStreamingResponse::new)
        .map_to_mm(|e| NetworkStreamingRequestError::EnableError(format!("{e:?}")))
}
