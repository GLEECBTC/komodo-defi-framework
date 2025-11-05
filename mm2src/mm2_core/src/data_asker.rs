use common::custom_futures::timeout::FutureTimerExt;
use common::{HttpStatusCode, StatusCode};
use compatible_time::Duration;
use derive_more::Display;
use futures::channel::oneshot;
use futures::lock::Mutex as AsyncMutex;
use mm2_err_handle::prelude::*;
use mm2_event_stream::{Event, StreamerId};
use ser_error_derive::SerializeErrorType;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::atomic::{self, AtomicUsize};
use std::sync::Arc;
use timed_map::{MapKind, TimedMap};

use crate::mm_ctx::{MmArc, MmCtx};

#[derive(Clone, Debug)]
pub struct DataAsker {
    data_id: Arc<AtomicUsize>,
    awaiting_asks: Arc<AsyncMutex<TimedMap<usize, oneshot::Sender<serde_json::Value>>>>,
}

impl Default for DataAsker {
    fn default() -> Self {
        Self {
            data_id: Default::default(),
            awaiting_asks: Arc::new(AsyncMutex::new(
                TimedMap::new_with_map_kind(MapKind::FxHashMap).expiration_tick_cap(5),
            )),
        }
    }
}

#[derive(Debug, Display)]
pub enum AskForDataError {
    #[display(fmt = "Expected JSON data, but the received data (from data provider) was not deserializable: {_0:?}")]
    DeserializationError(serde_json::Error),
    Internal(String),
    Timeout,
}

impl MmCtx {
    pub async fn ask_for_data<Input, Output>(
        &self,
        data_type: &str,
        data: Input,
        timeout: Duration,
    ) -> Result<Output, MmError<AskForDataError>>
    where
        Input: Serialize,
        Output: DeserializeOwned,
    {
        if data_type.contains(char::is_whitespace) {
            return MmError::err(AskForDataError::Internal(format!(
                "data_type can not contain whitespace, but got {data_type}"
            )));
        }

        let data_id = self.data_asker.data_id.fetch_add(1, atomic::Ordering::SeqCst);
        let (sender, receiver) = futures::channel::oneshot::channel::<serde_json::Value>();

        // We don't want to hold the lock, so call this in an inner-scope.
        {
            self.data_asker
                .awaiting_asks
                .lock()
                .await
                .insert_expirable(data_id, sender, timeout);
        }

        let input = json!({
            "data_id": data_id,
            "timeout_secs": timeout.as_secs(),
            "data": data
        });

        self.event_stream_manager.broadcast_all(Event::new(
            StreamerId::DataNeeded {
                data_type: data_type.to_string(),
            },
            input,
        ));

        match receiver.timeout(timeout).await {
            Ok(Ok(response)) => match serde_json::from_value::<Output>(response) {
                Ok(value) => Ok(value),
                Err(error) => MmError::err(AskForDataError::DeserializationError(error)),
            },
            Ok(Err(error)) => MmError::err(AskForDataError::Internal(format!(
                "Receiver channel is not alive. {error}"
            ))),
            Err(_) => MmError::err(AskForDataError::Timeout),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct SendAskedDataRequest {
    data_id: usize,
    data: serde_json::Value,
}

#[derive(Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum SendAskedDataError {
    #[display(fmt = "No data was asked for id={_0}")]
    NotFound(usize),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl HttpStatusCode for SendAskedDataError {
    fn status_code(&self) -> StatusCode {
        match self {
            SendAskedDataError::NotFound(_) => StatusCode::NOT_FOUND,
            SendAskedDataError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

pub async fn send_asked_data_rpc(
    ctx: MmArc,
    asked_data: SendAskedDataRequest,
) -> Result<bool, MmError<SendAskedDataError>> {
    let mut awaiting_asks = ctx.data_asker.awaiting_asks.lock().await;
    match awaiting_asks.remove(&asked_data.data_id) {
        Some(sender) => {
            sender.send(asked_data.data).map_to_mm(|_| {
                SendAskedDataError::Internal("Receiver channel is not alive. Most likely timed out.".to_owned())
            })?;
            Ok(true)
        },
        None => MmError::err(SendAskedDataError::NotFound(asked_data.data_id)),
    }
}

#[cfg(test)]
mod tests {
    use crate::mm_ctx::MmCtxBuilder;
    use common::block_on;
    use common::executor::Timer;
    use compatible_time::Duration;
    use serde::Deserialize;
    use serde_json::json;
    use std::thread;

    #[test]
    fn simulate_ask_and_send_data() {
        let ctx = MmCtxBuilder::new().into_mm_arc();
        let ctx_clone = ctx.clone();

        #[derive(Clone, Debug, Deserialize)]
        struct DummyType {
            name: String,
        }

        thread::scope(|scope| {
            scope.spawn(move || {
                let output: DummyType =
                    block_on(ctx.ask_for_data("TEST", serde_json::Value::Null, Duration::from_secs(3))).unwrap();
                let output2: DummyType =
                    block_on(ctx.ask_for_data("TEST", serde_json::Value::Null, Duration::from_secs(3))).unwrap();

                // Assert values sent from the other thread.
                assert_eq!(&output.name, "Onur");
                assert_eq!(&output2.name, "Reggi");
            });

            scope.spawn(move || {
                // Wait until we ask for data from the other thread.
                common::block_on(Timer::sleep(1.));

                let data = super::SendAskedDataRequest {
                    data_id: 0,
                    data: json!({
                        "name": "Onur".to_owned()
                    }),
                };

                block_on(super::send_asked_data_rpc(ctx_clone.clone(), data)).unwrap();

                // Wait until we ask for data from the other thread.
                common::block_on(Timer::sleep(1.));

                let data = super::SendAskedDataRequest {
                    data_id: 1,
                    data: json!({
                        "name": "Reggi".to_owned()
                    }),
                };

                block_on(super::send_asked_data_rpc(ctx_clone, data)).unwrap();
            });
        });
    }
}
