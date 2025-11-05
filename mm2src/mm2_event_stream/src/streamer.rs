use std::any::{self, Any};

use crate::{Event, StreamerId, StreamingManager};
use common::executor::{abortable_queue::WeakSpawner, AbortSettings, SpawnAbortable};
use common::log::{error, info};

use async_trait::async_trait;
use futures::channel::{mpsc, oneshot};
use futures::{future, select, FutureExt, Stream, StreamExt};

/// A marker to indicate that the event streamer doesn't take any input data.
pub struct NoDataIn;

/// A mixture trait combining `Stream`, `Send` & `Unpin` together (to avoid confusing annotation).
pub trait StreamHandlerInput<D>: Stream<Item = D> + Send + Unpin {}
/// Implement the trait for all types `T` that implement `Stream<Item = D> + Send + Unpin` for any `D`.
impl<T, D> StreamHandlerInput<D> for T where T: Stream<Item = D> + Send + Unpin {}

#[async_trait]
pub trait EventStreamer
where
    Self: Sized + Send + 'static,
{
    type DataInType: Send;

    /// Returns a human readable unique identifier for the event streamer.
    /// No other event streamer should have the same identifier.
    fn streamer_id(&self) -> StreamerId;

    /// Event handler that is responsible for broadcasting event data to the streaming channels.
    ///
    /// `ready_tx` is a oneshot sender that is used to send the initialization status of the event.
    /// `data_rx` is a receiver that the streamer *could* use to receive data from the outside world.
    async fn handle(
        self,
        broadcaster: Broadcaster,
        ready_tx: oneshot::Sender<Result<(), String>>,
        data_rx: impl StreamHandlerInput<Self::DataInType>,
    );
}

/// Trait for types that can produce a unique [`StreamerId`] for event streaming.
///
/// Used to standardize initialization and ID derivation for various streamers.
///
/// - `'a`: Lifetime for borrowed derive parameters.
pub trait DeriveStreamerId<'a> {
    /// Type used to create the instance.
    type InitParam;

    /// Borrowed type used to derive the [`StreamerId`].
    type DeriveParam: 'a;

    /// Creates a new instance using the specified initialization parameter.
    fn new(param: Self::InitParam) -> Self;

    /// Derives a unique [`StreamerId`] based on the provided parameter.
    fn derive_streamer_id(param: Self::DeriveParam) -> StreamerId;
}

/// Spawns the [`EventStreamer::handle`] in a separate task using [`WeakSpawner`].
///
/// Returns a [`oneshot::Sender`] to shutdown the handler and an optional [`mpsc::UnboundedSender`]
/// to send data to the handler.
pub(crate) async fn spawn<S>(
    streamer: S,
    spawner: WeakSpawner,
    streaming_manager: StreamingManager,
) -> Result<(oneshot::Sender<()>, Option<mpsc::UnboundedSender<Box<dyn Any + Send>>>), String>
where
    S: EventStreamer,
{
    let streamer_id = streamer.streamer_id();
    info!("Spawning event streamer: {streamer_id}");

    // A oneshot channel to receive the initialization status of the handler through.
    let (tx_ready, ready_rx) = oneshot::channel();
    // A oneshot channel to shutdown the handler.
    let (tx_shutdown, rx_shutdown) = oneshot::channel::<()>();
    // An unbounded channel to send data to the handler.
    let (any_data_sender, any_data_receiver) = mpsc::unbounded::<Box<dyn Any + Send>>();
    // A middleware to cast the data of type `Box<dyn Any>` to the actual input datatype of this streamer.
    let data_receiver = any_data_receiver.filter_map({
        let streamer_id = streamer_id.clone();
        move |any_input_data| {
            let streamer_id = streamer_id.clone();
            future::ready(
                any_input_data
                    .downcast()
                    .map(|input_data| *input_data)
                    .map_err(|_| {
                        error!("Couldn't downcast a received message to {}. This message wasn't intended to be sent to this streamer ({streamer_id}).", any::type_name::<S::DataInType>());
                    })
                    .ok(),
            )
        }
    });

    let handler_with_shutdown = {
        let streamer_id = streamer_id.clone();
        async move {
            select! {
                _ = rx_shutdown.fuse() => {
                    info!("Manually shutting down event streamer: {streamer_id}.")
                }
                _ = streamer.handle(Broadcaster::new(streaming_manager), tx_ready, data_receiver).fuse() => {}
            }
        }
    };
    let settings = AbortSettings::info_on_abort(format!("{streamer_id} streamer has stopped."));
    spawner.spawn_with_settings(handler_with_shutdown, settings);

    ready_rx.await.unwrap_or_else(|e| {
        Err(format!(
            "The handler was aborted before sending event initialization status: {e}"
        ))
    })?;

    // If the handler takes no input data, return `None` for the data sender.
    if any::TypeId::of::<S::DataInType>() == any::TypeId::of::<NoDataIn>() {
        Ok((tx_shutdown, None))
    } else {
        Ok((tx_shutdown, Some(any_data_sender)))
    }
}

/// A wrapper around `StreamingManager` to only expose the `broadcast` method.
pub struct Broadcaster(StreamingManager);

impl Broadcaster {
    pub fn new(inner: StreamingManager) -> Self {
        Self(inner)
    }

    pub fn broadcast(&self, event: Event) {
        self.0.broadcast(event);
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
pub mod test_utils {
    use super::*;

    use common::executor::Timer;
    use serde_json::json;

    /// A test event streamer that broadcasts an event periodically.
    /// Broadcasts `json!("hello")` every tenth of a second.
    pub struct PeriodicStreamer;

    #[async_trait]
    impl EventStreamer for PeriodicStreamer {
        type DataInType = NoDataIn;

        fn streamer_id(&self) -> StreamerId {
            StreamerId::ForTesting {
                test_streamer: "periodic_streamer".to_string(),
            }
        }

        async fn handle(
            self,
            broadcaster: Broadcaster,
            ready_tx: oneshot::Sender<Result<(), String>>,
            _: impl StreamHandlerInput<Self::DataInType>,
        ) {
            ready_tx.send(Ok(())).unwrap();
            loop {
                broadcaster.broadcast(Event::new(self.streamer_id(), json!("hello")));
                Timer::sleep(0.1).await;
            }
        }
    }

    /// A test event streamer that broadcasts an event whenever it receives a new message through `data_rx`.
    pub struct ReactiveStreamer;

    #[async_trait]
    impl EventStreamer for ReactiveStreamer {
        type DataInType = String;

        fn streamer_id(&self) -> StreamerId {
            StreamerId::ForTesting {
                test_streamer: "reactive_streamer".to_string(),
            }
        }

        async fn handle(
            self,
            broadcaster: Broadcaster,
            ready_tx: oneshot::Sender<Result<(), String>>,
            mut data_rx: impl StreamHandlerInput<Self::DataInType>,
        ) {
            ready_tx.send(Ok(())).unwrap();
            while let Some(msg) = data_rx.next().await {
                // Just echo back whatever we receive.
                broadcaster.broadcast(Event::new(self.streamer_id(), json!(msg)));
            }
        }
    }

    /// A test event streamer that fails upon initialization.
    pub struct InitErrorStreamer;

    #[async_trait]
    impl EventStreamer for InitErrorStreamer {
        type DataInType = NoDataIn;

        fn streamer_id(&self) -> StreamerId {
            StreamerId::ForTesting {
                test_streamer: "init_error_streamer".to_string(),
            }
        }

        async fn handle(
            self,
            _: Broadcaster,
            ready_tx: oneshot::Sender<Result<(), String>>,
            _: impl StreamHandlerInput<Self::DataInType>,
        ) {
            // Fail the initialization and stop.
            ready_tx.send(Err("error".to_string())).unwrap();
        }
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
mod tests {
    use super::test_utils::{InitErrorStreamer, PeriodicStreamer, ReactiveStreamer};
    use super::*;

    use common::executor::abortable_queue::AbortableQueue;
    use common::{cfg_wasm32, cross_test};
    cfg_wasm32! {
        use wasm_bindgen_test::*;
        wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
    }

    cross_test!(test_spawn_periodic_streamer, {
        let system = AbortableQueue::default();
        // Spawn the periodic streamer.
        let (_, data_in) = spawn(PeriodicStreamer, system.weak_spawner(), StreamingManager::default())
            .await
            .unwrap();
        // Periodic streamer shouldn't be ingesting any input.
        assert!(data_in.is_none());
    });

    cross_test!(test_spawn_reactive_streamer, {
        let system = AbortableQueue::default();
        // Spawn the reactive streamer.
        let (_, data_in) = spawn(ReactiveStreamer, system.weak_spawner(), StreamingManager::default())
            .await
            .unwrap();
        // Reactive streamer should be ingesting some input.
        assert!(data_in.is_some());
    });

    cross_test!(test_spawn_erroring_streamer, {
        let system = AbortableQueue::default();
        // Try to spawn the erroring streamer.
        let err = spawn(InitErrorStreamer, system.weak_spawner(), StreamingManager::default())
            .await
            .unwrap_err();
        // The streamer should return an error.
        assert_eq!(err, "error");
    });
}
