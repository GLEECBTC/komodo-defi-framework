mod balance;
mod disable;
mod fee_estimation;
mod heartbeat;
mod network;
mod orderbook;
mod orders;
mod swaps;
mod tx_history;

// Re-exports
pub use balance::*;
pub use disable::*;
pub use fee_estimation::*;
pub use heartbeat::*;
pub use network::*;
pub use orderbook::*;
pub use orders::*;
pub use swaps::*;
pub use tx_history::*;

use mm2_event_stream::StreamerId;

/// The general request for enabling any streamer.
/// `client_id` is common in each request, other data is request-specific.
#[derive(Deserialize)]
pub struct EnableStreamingRequest<T> {
    // If the client ID isn't included, assume it's 0.
    #[serde(default)]
    pub client_id: u64,
    #[serde(flatten)]
    inner: T,
}

/// The success/ok response for any event streaming activation request.
#[derive(Serialize)]
pub struct EnableStreamingResponse {
    pub streamer_id: StreamerId,
    // TODO: If the the streamer was already running, it is probably running with different configuration.
    // We might want to inform the client that the configuration they asked for wasn't applied and return
    // the active configuration instead?
    // pub config: Json,
}

impl EnableStreamingResponse {
    fn new(streamer_id: StreamerId) -> Self {
        Self { streamer_id }
    }
}
