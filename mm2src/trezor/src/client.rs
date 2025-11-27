//! This file is inspired by https://github.com/tezedge/tezedge-client/blob/master/trezor_api/src/client.rs

use crate::device_info::TrezorDeviceInfo;
use crate::error::OperationFailure;
use crate::proto::messages::MessageType;
use crate::proto::messages_common as proto_common;
use crate::proto::messages_management as proto_management;
use crate::proto::{ProtoMessage, TrezorMessage};
use crate::response::TrezorResponse;
use crate::result_handler::ResultHandler;
use crate::transport::Transport;
use crate::TrezorRequestProcessor;
use crate::{TrezorError, TrezorResult};
use common::now_ms;
use futures::lock::{Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard};
use mm2_err_handle::prelude::*;
use rpc_task::RpcTaskError;
use std::sync::Arc;

#[derive(Clone)]
pub struct TrezorClient {
    inner: Arc<AsyncMutex<TrezorClientImpl>>,
}

impl TrezorClient {
    pub fn from_transport<T>(transport: T) -> TrezorClient
    where
        T: Transport + Send + Sync + 'static,
    {
        let transport = Box::new(transport);
        let inner = Arc::new(AsyncMutex::new(TrezorClientImpl { transport }));
        TrezorClient { inner }
    }

    /// Initialize a Trezor session by sending
    /// [Initialize](https://docs.trezor.io/trezor-firmware/common/communication/sessions.html#examples).
    /// Returns `TrezorDeviceInfo` and `TrezorSession`.
    pub async fn init_new_session(
        &mut self,
        processor: Arc<dyn TrezorRequestProcessor<Error = RpcTaskError>>,
    ) -> TrezorResult<(TrezorDeviceInfo, TrezorSession<'_>)> {
        let mut session = TrezorSession {
            inner: self.inner.lock().await,
            processor: Some(processor.clone()),
        };
        let features = session.initialize_device().await?;
        Ok((TrezorDeviceInfo::from(features), session))
    }

    /// Occupies the Trezor device for further interactions by locking a mutex.
    pub async fn session(&self, processor: Arc<dyn TrezorRequestProcessor<Error = RpcTaskError>>) -> TrezorSession<'_> {
        TrezorSession {
            inner: self.inner.lock().await,
            processor: Some(processor.clone()),
        }
    }

    /// Checks if the Trezor device is vacant (not occupied).
    /// Returns `None` if it is occupied already.
    /// Note: does not return processor and should be used to check connections only
    pub fn try_session_if_not_occupied(&self) -> Option<TrezorSession<'_>> {
        self.inner
            .try_lock()
            .map(|inner| TrezorSession { inner, processor: None })
    }
}

pub struct TrezorClientImpl {
    transport: Box<dyn Transport + Send + Sync + 'static>,
}

pub struct TrezorSession<'a> {
    inner: AsyncMutexGuard<'a, TrezorClientImpl>,
    pub processor: Option<Arc<dyn TrezorRequestProcessor<Error = RpcTaskError>>>,
}

impl<'a> TrezorSession<'a> {
    #[cfg(target_arch = "wasm32")]
    pub async fn is_connected(&mut self) -> TrezorResult<bool> {
        self.inner.transport.is_connected().await
    }

    /// Sends a message and returns a TrezorResponse with either the
    /// expected response message, a failure or an interaction request.
    pub async fn call<'b, T: 'static, S: TrezorMessage>(
        &'b mut self,
        message: S,
        result_handler: ResultHandler<T>,
    ) -> TrezorResult<TrezorResponse<'a, 'b, T>> {
        let resp = self.call_raw(message).await?;
        match resp.message_type() {
            mt if mt == result_handler.message_type() => Ok(TrezorResponse::Ready(result_handler.handle_raw(resp)?)),
            MessageType::Failure => {
                let fail_msg: proto_common::Failure = resp.into_message()?;
                MmError::err(TrezorError::Failure(OperationFailure::from(fail_msg)))
            },
            MessageType::ButtonRequest => {
                let req_msg = resp.into_message()?;
                Ok(TrezorResponse::new_button_request(self, req_msg, result_handler))
            },
            MessageType::PinMatrixRequest => {
                let req_msg = resp.into_message()?;
                Ok(TrezorResponse::new_pin_matrix_request(self, req_msg, result_handler))
            },
            MessageType::PassphraseRequest => {
                let req_msg = resp.into_message()?;
                Ok(TrezorResponse::new_passphrase_request(self, req_msg, result_handler))
            },
            mtype => MmError::err(TrezorError::UnexpectedMessageType(mtype)),
        }
    }

    /// Sends a message and returns the raw ProtoMessage struct that was
    /// responded by the device.
    async fn call_raw<S: TrezorMessage>(&mut self, message: S) -> TrezorResult<ProtoMessage> {
        let mut buf = Vec::with_capacity(message.encoded_len());
        message.encode(&mut buf)?;

        let proto_msg = ProtoMessage::new(S::message_type(), buf);
        self.inner.transport.write_message(proto_msg).await?;
        self.inner.transport.read_message().await
    }

    pub async fn ping<'b>(&'b mut self) -> TrezorResult<TrezorResponse<'a, 'b, ()>> {
        let ping_message = format!("With love, {}", now_ms());
        let req = proto_management::Ping {
            message: Some(ping_message.clone()),
            button_protection: None,
        };

        let result_handler = ResultHandler::<()>::new(move |pong: proto_common::Success| {
            if pong.message == Some(ping_message) {
                Ok(())
            } else {
                MmError::err(TrezorError::PongMessageMismatch)
            }
        });
        self.call(req, result_handler).await
    }

    /// Initialize the device.
    ///
    /// The Initialize packet will cause the device to stop what it is currently doing
    /// and should work at any time.
    /// Thus, it can also be used to recover from previous errors.
    ///
    /// # Usage
    ///
    /// Must be called before sending requests to Trezor.
    async fn initialize_device(&mut self) -> TrezorResult<proto_management::Features> {
        // Don't set the session_id since currently there is no need to restore the previous session.
        // https://docs.trezor.io/trezor-firmware/common/communication/sessions.html#session-lifecycle
        let req = proto_management::Initialize { session_id: None };

        let result_handler = ResultHandler::<proto_management::Features>::new(Ok);
        let features = self.call(req, result_handler).await?.ok()?;

        Ok(features)
    }

    pub(crate) async fn cancel_last_op(&mut self) {
        let req = proto_management::Cancel {};
        let result_handler = ResultHandler::new(|_m: proto_common::Failure| Ok(()));
        // Ignore result.
        self.call(req, result_handler).await.ok();
    }
}
