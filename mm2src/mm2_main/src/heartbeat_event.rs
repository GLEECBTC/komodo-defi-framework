use async_trait::async_trait;
use common::executor::Timer;
use futures::channel::oneshot;
use mm2_event_stream::{Broadcaster, Event, EventStreamer, NoDataIn, StreamHandlerInput, StreamerId};
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct HeartbeatEventConfig {
    /// The time in seconds to wait before sending another ping event.
    pub stream_interval_seconds: f64,
}

impl Default for HeartbeatEventConfig {
    fn default() -> Self {
        Self {
            stream_interval_seconds: 5.0,
        }
    }
}

pub struct HeartbeatEvent {
    config: HeartbeatEventConfig,
}

impl HeartbeatEvent {
    pub fn new(config: HeartbeatEventConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl EventStreamer for HeartbeatEvent {
    type DataInType = NoDataIn;

    fn streamer_id(&self) -> StreamerId {
        StreamerId::Heartbeat
    }

    async fn handle(
        self,
        broadcaster: Broadcaster,
        ready_tx: oneshot::Sender<Result<(), String>>,
        _: impl StreamHandlerInput<NoDataIn>,
    ) {
        ready_tx.send(Ok(())).unwrap();

        loop {
            broadcaster.broadcast(Event::new(self.streamer_id(), json!({})));

            Timer::sleep(self.config.stream_interval_seconds).await;
        }
    }
}
