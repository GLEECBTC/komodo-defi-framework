use std::convert::TryFrom;

use crate::proto::messages::MessageType;
use crate::proto::messages_common::{failure::FailureType, Failure};
use crate::user_interaction::TrezorUserInteraction;
use derive_more::Display;
use mm2_err_handle::prelude::*;
use prost::{DecodeError, EncodeError};

pub type TrezorResult<T> = Result<T, MmError<TrezorError>>;

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "ios")))]
use hw_common::transport::UsbError;
#[cfg(target_arch = "wasm32")]
use hw_common::transport::WebUsbError;

#[derive(Debug, Display)]
pub enum TrezorError {
    #[display(fmt = "'{transport}' transport is not available on this platform")]
    TransportNotSupported {
        transport: String,
    },
    /// Please note it's not the same as `PermissionDenied`.
    /// This error may appear in a browser when the user didn't allow the app to get the list of devices.
    ErrorRequestingAccessPermission(String),
    /// TODO put a device info
    DeviceDisconnected,
    /// The error depends on transport implementation.
    UnderlyingError(String),
    ProtocolError(String),
    #[display(fmt = "Received unexpected message type: {_0:?}")]
    UnexpectedMessageType(MessageType),
    Failure(OperationFailure),
    #[display(fmt = "Unexpected interaction request: {_0:?}")]
    UnexpectedInteractionRequest(TrezorUserInteraction),
    Internal(String),
    PongMessageMismatch,
    #[display(fmt = "no processor for trezor response")]
    InternalNoProcessor,
}

#[derive(Clone, Debug, Display)]
pub enum OperationFailure {
    InvalidPin,
    UnexpectedMessage,
    ButtonExpected,
    DataError,
    PinExpected,
    InvalidSignature,
    ProcessError,
    NotEnoughFunds,
    NotInitialized,
    WipeCodeMismatch,
    InvalidSession,
    FirmwareError,
    FailureMessageNotFound,
    UserCancelled,
}

impl From<Failure> for OperationFailure {
    fn from(failure: Failure) -> Self {
        match failure.code.and_then(|t| FailureType::try_from(t).ok()) {
            Some(FailureType::FailurePinInvalid) | Some(FailureType::FailurePinMismatch) => {
                OperationFailure::InvalidPin
            },
            Some(FailureType::FailureActionCancelled) | Some(FailureType::FailurePinCancelled) => {
                OperationFailure::UserCancelled
            },
            Some(FailureType::FailureUnexpectedMessage) => OperationFailure::UnexpectedMessage,
            Some(FailureType::FailureButtonExpected) => OperationFailure::ButtonExpected,
            Some(FailureType::FailureDataError) => OperationFailure::DataError,
            Some(FailureType::FailurePinExpected) => OperationFailure::PinExpected,
            Some(FailureType::FailureInvalidSignature) => OperationFailure::InvalidSignature,
            Some(FailureType::FailureProcessError) => OperationFailure::ProcessError,
            Some(FailureType::FailureNotEnoughFunds) => OperationFailure::NotEnoughFunds,
            Some(FailureType::FailureNotInitialized) => OperationFailure::NotInitialized,
            Some(FailureType::FailureWipeCodeMismatch) => OperationFailure::WipeCodeMismatch,
            Some(FailureType::FailureInvalidSession) => OperationFailure::InvalidSession,
            Some(FailureType::FailureFirmwareError) => OperationFailure::FirmwareError,
            None => OperationFailure::FailureMessageNotFound,
        }
    }
}

impl From<DecodeError> for TrezorError {
    fn from(e: DecodeError) -> Self {
        TrezorError::ProtocolError(e.to_string())
    }
}

impl From<EncodeError> for TrezorError {
    fn from(e: EncodeError) -> Self {
        TrezorError::Internal(e.to_string())
    }
}

#[cfg(target_arch = "wasm32")]
impl From<WebUsbError> for TrezorError {
    fn from(e: WebUsbError) -> Self {
        match e {
            WebUsbError::NotSupported => TrezorError::TransportNotSupported {
                transport: "WebUSB".to_owned(),
            },
            WebUsbError::ErrorRequestingDevice(e) => TrezorError::ErrorRequestingAccessPermission(e),
            WebUsbError::Internal(e) => TrezorError::Internal(e),
            e => TrezorError::UnderlyingError(e.to_string()),
        }
    }
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "ios")))]
impl From<UsbError> for TrezorError {
    fn from(e: UsbError) -> Self {
        match e {
            UsbError::DeviceDisconnected => TrezorError::DeviceDisconnected,
            UsbError::Internal(e) => TrezorError::Internal(e),
            e => TrezorError::UnderlyingError(e.to_string()),
        }
    }
}

#[cfg(all(not(target_arch = "wasm32"), not(target_os = "ios")))]
impl From<std::io::Error> for TrezorError {
    fn from(e: std::io::Error) -> Self {
        TrezorError::UnderlyingError(e.to_string())
    }
}
