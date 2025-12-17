use crate::client::TrezorSession;
use crate::proto::messages_common as proto_common;
use crate::result_handler::ResultHandler;
use crate::user_interaction::TrezorUserInteraction;
use crate::{TrezorError, TrezorResult};
use async_trait::async_trait;
use mm2_err_handle::prelude::*;
use rpc_task::RpcTaskError;
use std::convert::TryFrom;
use std::fmt;
use std::sync::Arc;

pub use crate::proto::messages_common::button_request::ButtonRequestType;
pub use crate::proto::messages_common::pin_matrix_request::PinMatrixRequestType;
use crate::response_processor::{ProcessTrezorResponse, TrezorProcessingError, TrezorRequestProcessor};

/// The different types of user interactions the Trezor device can request.
#[derive(PartialEq, Eq, Clone, Debug)]
pub enum InteractionType {
    Button,
    PinMatrix,
    Passphrase,
    PassphraseState,
}

/// A response from a Trezor device.
///
/// On every message exchange, instead of the expected/desired response,
/// the Trezor can ask for some user interaction, or can send a failure.
#[derive(Debug)]
pub enum TrezorResponse<'a, 'b, T> {
    Ready(T),
    ButtonRequest(ButtonRequest<'a, 'b, T>),
    PinMatrixRequest(PinMatrixRequest<'a, 'b, T>),
    PassphraseRequest(PassphraseRequest<'a, 'b, T>),
}

impl<'a, 'b, T: 'static> TrezorResponse<'a, 'b, T> {
    /// Get the actual `Ok` response value or an error if not `Ok`.
    pub fn ok(self) -> TrezorResult<T> {
        match self {
            TrezorResponse::Ready(m) => Ok(m),
            TrezorResponse::ButtonRequest(_) => MmError::err(TrezorError::UnexpectedInteractionRequest(
                TrezorUserInteraction::ButtonRequest,
            )),
            TrezorResponse::PinMatrixRequest(_) => MmError::err(TrezorError::UnexpectedInteractionRequest(
                TrezorUserInteraction::PinMatrix3x3,
            )),
            TrezorResponse::PassphraseRequest(_) => MmError::err(TrezorError::UnexpectedInteractionRequest(
                TrezorUserInteraction::PassphraseRequest,
            )),
        }
    }

    /// Returns `Some(T)` if the result is ready, otherwise cancels the request.
    pub async fn cancel_if_not_ready(self) -> Option<T> {
        match self {
            TrezorResponse::Ready(val) => {
                return Some(val);
            },
            TrezorResponse::ButtonRequest(button) => button.cancel().await,
            TrezorResponse::PinMatrixRequest(pin) => pin.cancel().await,
            TrezorResponse::PassphraseRequest(pass) => pass.cancel().await,
        }
        None
    }

    pub(crate) fn new_button_request(
        session: &'b mut TrezorSession<'a>,
        message: proto_common::ButtonRequest,
        result_handler: ResultHandler<T>,
    ) -> Self {
        TrezorResponse::ButtonRequest(ButtonRequest {
            session,
            message,
            result_handler,
        })
    }

    pub(crate) fn new_pin_matrix_request(
        session: &'b mut TrezorSession<'a>,
        message: proto_common::PinMatrixRequest,
        result_handler: ResultHandler<T>,
    ) -> Self {
        TrezorResponse::PinMatrixRequest(PinMatrixRequest {
            session,
            message,
            result_handler,
        })
    }

    pub(crate) fn new_passphrase_request(
        session: &'b mut TrezorSession<'a>,
        message: proto_common::PassphraseRequest,
        result_handler: ResultHandler<T>,
    ) -> Self {
        TrezorResponse::PassphraseRequest(PassphraseRequest {
            session,
            message,
            result_handler,
        })
    }
}

#[async_trait]
impl<T> ProcessTrezorResponse<T> for TrezorResponse<'_, '_, T>
where
    T: Send + Sync + 'static,
{
    async fn process(
        self,
        processor: Arc<dyn TrezorRequestProcessor<Error = RpcTaskError>>,
    ) -> MmResult<T, TrezorProcessingError<RpcTaskError>> {
        let processor_req = processor.clone();
        let fut = async move {
            let mut response = self;
            loop {
                response = match response {
                    TrezorResponse::Ready(result) => return Ok(result),
                    TrezorResponse::ButtonRequest(button_req) => {
                        processor_req.on_button_request().await.map_mm_err()?;
                        button_req.ack().await.map_mm_err()?
                    },
                    TrezorResponse::PinMatrixRequest(pin_req) => {
                        let pin_response = processor_req.on_pin_request().await.map_mm_err()?;
                        pin_req.ack_pin(pin_response.pin).await.map_mm_err()?
                    },
                    TrezorResponse::PassphraseRequest(passphrase_req) => {
                        let passphrase_response = processor_req.on_passphrase_request().await.map_mm_err()?;
                        passphrase_req
                            .ack_passphrase(passphrase_response.passphrase)
                            .await
                            .map_mm_err()?
                    },
                };
            }
        };
        let res = fut.await;
        processor.on_ready().await.map_mm_err()?;
        res
    }
}

/// A button request message sent by the device.
pub struct ButtonRequest<'a, 'b, T> {
    session: &'b mut TrezorSession<'a>,
    message: proto_common::ButtonRequest,
    result_handler: ResultHandler<T>,
}

impl<T> fmt::Debug for ButtonRequest<'_, '_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.message)
    }
}

impl<'a, 'b, T: 'static> ButtonRequest<'a, 'b, T> {
    /// The type of button request.
    #[inline(always)]
    pub fn request_type(&self) -> Option<ButtonRequestType> {
        self.message.code.and_then(|t| ButtonRequestType::try_from(t).ok())
    }

    /// Ack the request and get the next message from the device.
    pub async fn ack(self) -> TrezorResult<TrezorResponse<'a, 'b, T>> {
        let req = proto_common::ButtonAck {};
        self.session.call(req, self.result_handler).await
    }

    pub async fn cancel(self) {
        self.session.cancel_last_op().await
    }
}

/// A PIN matrix request message sent by the device.
pub struct PinMatrixRequest<'a, 'b, T> {
    session: &'b mut TrezorSession<'a>,
    message: proto_common::PinMatrixRequest,
    result_handler: ResultHandler<T>,
}

impl<T> fmt::Debug for PinMatrixRequest<'_, '_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.message)
    }
}

impl<'a, 'b, T: 'static> PinMatrixRequest<'a, 'b, T> {
    /// The type of PIN matrix request.
    #[inline(always)]
    pub fn request_type(&self) -> Option<PinMatrixRequestType> {
        self.message.r#type.and_then(|t| PinMatrixRequestType::try_from(t).ok())
    }

    /// Ack the request with a PIN and get the next message from the device.
    pub async fn ack_pin(self, pin: String) -> TrezorResult<TrezorResponse<'a, 'b, T>> {
        let req = proto_common::PinMatrixAck { pin };
        self.session.call(req, self.result_handler).await
    }

    pub async fn cancel(self) {
        self.session.cancel_last_op().await
    }
}

/// A Passphrase request message sent by the device.
pub struct PassphraseRequest<'a, 'b, T> {
    session: &'b mut TrezorSession<'a>,
    message: proto_common::PassphraseRequest,
    result_handler: ResultHandler<T>,
}

impl<T> fmt::Debug for PassphraseRequest<'_, '_, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.message)
    }
}

impl<'a, 'b, T: 'static> PassphraseRequest<'a, 'b, T> {
    /// Ack the request with a Passphrase and get the next message from the device.
    pub async fn ack_passphrase(self, passphrase: String) -> TrezorResult<TrezorResponse<'a, 'b, T>> {
        #[allow(deprecated)]
        let req = proto_common::PassphraseAck {
            passphrase: Some(passphrase),
            state: None,
            on_device: None,
        };
        self.session.call(req, self.result_handler).await
    }

    pub async fn cancel(self) {
        self.session.cancel_last_op().await
    }
}
