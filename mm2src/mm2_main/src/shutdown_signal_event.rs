use async_trait::async_trait;
use futures::channel::oneshot;
use futures::StreamExt;
use mm2_event_stream::{Broadcaster, Event, EventStreamer, StreamHandlerInput, StreamerId};

pub struct ShutdownSignalEvent;

#[async_trait]
impl EventStreamer for ShutdownSignalEvent {
    type DataInType = String;

    fn streamer_id(&self) -> StreamerId {
        StreamerId::ShutdownSignal
    }

    async fn handle(
        self,
        broadcaster: Broadcaster,
        ready_tx: oneshot::Sender<Result<(), String>>,
        mut data_rx: impl StreamHandlerInput<Self::DataInType>,
    ) {
        ready_tx
            .send(Ok(()))
            .expect("Receiver is dropped, which should never happen.");

        while let Some(signal) = data_rx.next().await {
            broadcaster.broadcast(Event::new(self.streamer_id(), json!(signal)));
        }
    }
}
