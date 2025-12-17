//! This file is inspired by https://github.com/tezedge/tezedge-client/blob/master/trezor_api/src/client.rs

use crate::proto::messages::MessageType;
use crate::proto::{ProtoMessage, TrezorMessage};
use crate::{TrezorError, TrezorResult};
use mm2_err_handle::prelude::*;

/// Function to be passed to the [`TrezorClient::call`] method
/// to process the Trezor response message into a general-purpose type.
pub(crate) type RawResultHandler<T> = dyn FnOnce(ProtoMessage) -> TrezorResult<T> + Send;

pub struct ResultHandler<T> {
    result_message_type: MessageType,
    handler: Box<RawResultHandler<T>>,
}

impl<T> ResultHandler<T> {
    pub fn new<H, R>(result_handler: H) -> ResultHandler<T>
    where
        H: FnOnce(R) -> TrezorResult<T> + 'static + Send,
        R: TrezorMessage,
    {
        let handler = move |proto: ProtoMessage| {
            if R::message_type() != proto.message_type() {
                return MmError::err(TrezorError::UnexpectedMessageType(proto.message_type()));
            }
            let resp_msg = proto.into_message()?;
            result_handler(resp_msg)
        };
        ResultHandler {
            result_message_type: R::message_type(),
            handler: Box::new(handler),
        }
    }

    pub fn message_type(&self) -> MessageType {
        self.result_message_type
    }

    pub fn handle_raw(self, proto: ProtoMessage) -> TrezorResult<T> {
        (self.handler)(proto)
    }
}
