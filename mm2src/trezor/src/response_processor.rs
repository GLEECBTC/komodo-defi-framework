use std::sync::Arc;

use crate::user_interaction::TrezorPassphraseResponse;
use crate::{TrezorError, TrezorPinMatrix3x3Response};
use async_trait::async_trait;
use derive_more::Display;
use mm2_err_handle::prelude::*;
use rpc_task::RpcTaskError;

#[derive(Display)]
pub enum TrezorProcessingError<E> {
    TrezorError(TrezorError),
    ProcessorError(E),
}

impl<E> From<TrezorError> for TrezorProcessingError<E> {
    fn from(e: TrezorError) -> Self {
        TrezorProcessingError::TrezorError(e)
    }
}

#[async_trait]
pub trait TrezorRequestProcessor
where
    Self: Send + Sync,
{
    type Error: NotMmError + Send;

    async fn on_button_request(&self) -> MmResult<(), TrezorProcessingError<Self::Error>>;

    async fn on_pin_request(&self) -> MmResult<TrezorPinMatrix3x3Response, TrezorProcessingError<Self::Error>>;

    async fn on_passphrase_request(&self) -> MmResult<TrezorPassphraseResponse, TrezorProcessingError<Self::Error>>;

    async fn on_ready(&self) -> MmResult<(), TrezorProcessingError<Self::Error>>;
}

#[async_trait]
pub trait ProcessTrezorResponse<T>
where
    T: Send + Sync + 'static,
{
    async fn process(
        self,
        processor: Arc<dyn TrezorRequestProcessor<Error = RpcTaskError>>,
    ) -> MmResult<T, TrezorProcessingError<RpcTaskError>>;
}
