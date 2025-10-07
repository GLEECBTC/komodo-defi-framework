//! This file is inspired by https://github.com/tezedge/tezedge-client/blob/master/trezor_api/src/messages.rs
//! In this module we implement the `message_type` getter for all protobuf message types.

use prost::bytes::BytesMut;

pub mod messages;
pub mod messages_bitcoin;
pub mod messages_common;
pub mod messages_ethereum;
pub mod messages_ethereum_definitions;
pub mod messages_management;

/// This is needed by generated protobuf modules.
pub(crate) use messages_common as common;

use messages::MessageType;
use messages_bitcoin::*;
use messages_common::*;
use messages_ethereum::*;
use messages_management::*;

/// This macro provides the TrezorMessage trait for a protobuf message.
macro_rules! trezor_message_impl {
    ($struct:ident, $mtype:expr) => {
        impl TrezorMessage for $struct {
            fn message_type() -> MessageType {
                $mtype
            }
        }
    };
}

/// A protobuf message accompanied by the message type.
/// This type is used to pass messages over the transport
/// and used to contain messages received from the transport.
pub struct ProtoMessage {
    message_type: MessageType,
    payload: Vec<u8>,
}

impl ProtoMessage {
    pub fn new(message_type: MessageType, payload: Vec<u8>) -> ProtoMessage {
        ProtoMessage { message_type, payload }
    }

    pub fn message_type(&self) -> MessageType {
        self.message_type
    }

    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub fn into_payload(self) -> Vec<u8> {
        self.payload
    }

    /// Take the payload from the ProtoMessage and parse it to a protobuf message.
    pub fn into_message<M: prost::Message + Default>(self) -> Result<M, prost::DecodeError> {
        let bytes = BytesMut::from(self.payload.as_slice());
        prost::Message::decode(bytes)
    }
}

/// This trait extends the protobuf Message trait to also have a static getter for the message
/// type code.
pub trait TrezorMessage: prost::Message + Default + 'static {
    fn message_type() -> MessageType;
}

// Management
trezor_message_impl!(Initialize, MessageType::Initialize);
trezor_message_impl!(Ping, MessageType::Ping);
trezor_message_impl!(ChangePin, MessageType::ChangePin);
trezor_message_impl!(WipeDevice, MessageType::WipeDevice);
trezor_message_impl!(GetEntropy, MessageType::GetEntropy);
trezor_message_impl!(Entropy, MessageType::Entropy);
trezor_message_impl!(LoadDevice, MessageType::LoadDevice);
trezor_message_impl!(ResetDevice, MessageType::ResetDevice);
trezor_message_impl!(Features, MessageType::Features);
trezor_message_impl!(Cancel, MessageType::Cancel);
trezor_message_impl!(EndSession, MessageType::EndSession);
trezor_message_impl!(ApplySettings, MessageType::ApplySettings);
trezor_message_impl!(ApplyFlags, MessageType::ApplyFlags);
trezor_message_impl!(BackupDevice, MessageType::BackupDevice);
trezor_message_impl!(EntropyRequest, MessageType::EntropyRequest);
trezor_message_impl!(EntropyAck, MessageType::EntropyAck);
trezor_message_impl!(RecoveryDevice, MessageType::RecoveryDevice);
trezor_message_impl!(WordRequest, MessageType::WordRequest);
trezor_message_impl!(WordAck, MessageType::WordAck);
trezor_message_impl!(GetFeatures, MessageType::GetFeatures);
// Common
trezor_message_impl!(Success, MessageType::Success);
trezor_message_impl!(Failure, MessageType::Failure);
trezor_message_impl!(PinMatrixRequest, MessageType::PinMatrixRequest);
trezor_message_impl!(PinMatrixAck, MessageType::PinMatrixAck);
trezor_message_impl!(ButtonRequest, MessageType::ButtonRequest);
trezor_message_impl!(ButtonAck, MessageType::ButtonAck);
trezor_message_impl!(PassphraseRequest, MessageType::PassphraseRequest);
trezor_message_impl!(PassphraseAck, MessageType::PassphraseAck);
// Bitcoin
trezor_message_impl!(GetAddress, MessageType::GetAddress);
trezor_message_impl!(Address, MessageType::Address);
trezor_message_impl!(GetPublicKey, MessageType::GetPublicKey);
trezor_message_impl!(PublicKey, MessageType::PublicKey);
trezor_message_impl!(SignTx, MessageType::SignTx);
trezor_message_impl!(TxRequest, MessageType::TxRequest);

// Bitcoin (compatible)
trezor_message_impl!(TxAckOutput, MessageType::TxAck);
trezor_message_impl!(TxAckInput, MessageType::TxAck);
trezor_message_impl!(TxAckPrevMeta, MessageType::TxAck);
trezor_message_impl!(TxAckPrevInput, MessageType::TxAck);
trezor_message_impl!(TxAckPrevOutput, MessageType::TxAck);
trezor_message_impl!(TxAckPrevExtraData, MessageType::TxAck);

// Ethereum
trezor_message_impl!(EthereumSignTx, MessageType::EthereumSignTx);
trezor_message_impl!(EthereumSignTxEIP1559, MessageType::EthereumSignTxEip1559);
trezor_message_impl!(EthereumTxRequest, MessageType::EthereumTxRequest);
trezor_message_impl!(EthereumTxAck, MessageType::EthereumTxAck);
trezor_message_impl!(EthereumGetAddress, MessageType::EthereumGetAddress);
trezor_message_impl!(EthereumAddress, MessageType::EthereumAddress);
trezor_message_impl!(EthereumGetPublicKey, MessageType::EthereumGetPublicKey);
trezor_message_impl!(EthereumPublicKey, MessageType::EthereumPublicKey);
