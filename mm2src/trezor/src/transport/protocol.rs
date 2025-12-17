//! This file is inspired by https://github.com/tezedge/tezedge-client/blob/master/trezor_api/src/transport/protocol.rs

use std::convert::TryFrom;

use crate::proto::messages::MessageType;
use crate::proto::ProtoMessage;
use crate::{TrezorError, TrezorResult};
use async_trait::async_trait;
use byteorder::{BigEndian, ByteOrder};
use mm2_err_handle::prelude::*;

const CHUNK_LEN: u32 = 64;
const CHUNK_HEADER_LEN: usize = 9;

/// A link represents a serial connection to send and receive byte chunks from and to a Trezor device.
#[async_trait]
pub trait Link {
    async fn write_chunk(&mut self, chunk: Vec<u8>) -> TrezorResult<()>;
    async fn read_chunk(&mut self, chunk_len: u32) -> TrezorResult<Vec<u8>>;
}

/// A protocol is used to encode messages in chunks that can be sent to the device and to parse
/// chunks into messages.
#[async_trait]
pub trait Protocol {
    async fn session_begin(&mut self) -> TrezorResult<()>;
    async fn session_end(&mut self) -> TrezorResult<()>;
    async fn write(&mut self, message: ProtoMessage) -> TrezorResult<()>;
    async fn read(&mut self) -> TrezorResult<ProtoMessage>;
}

/// The original binary protocol.
pub struct ProtocolV1<L: Link> {
    pub link: L,
}

#[cfg(target_arch = "wasm32")]
impl<L: Link> ProtocolV1<L> {
    pub(crate) fn link(&self) -> &L {
        &self.link
    }
}

#[async_trait]
impl<L: Link + Send> Protocol for ProtocolV1<L> {
    /// Protocol V1 doesn't support sessions.
    async fn session_begin(&mut self) -> TrezorResult<()> {
        Ok(())
    }

    /// Protocol V1 doesn't support sessions.
    async fn session_end(&mut self) -> TrezorResult<()> {
        Ok(())
    }

    async fn write(&mut self, message: ProtoMessage) -> TrezorResult<()> {
        // First generate the total payload, then write it to the transport in chunks.
        let mut data = vec![0; 8];
        data[0] = 0x23;
        data[1] = 0x23;
        BigEndian::write_u16(&mut data[2..4], message.message_type() as u16);
        BigEndian::write_u32(&mut data[4..8], message.payload().len() as u32);
        data.extend(message.into_payload());

        let mut cur: usize = 0;
        while cur < data.len() {
            let mut chunk = vec![0x3f];
            let end = std::cmp::min(cur + (CHUNK_LEN - 1) as usize, data.len());
            chunk.extend(&data[cur..end]);
            cur = end;
            debug_assert!(chunk.len() <= CHUNK_LEN as usize);
            chunk.resize(CHUNK_LEN as usize, 0);

            self.link.write_chunk(chunk).await?;
        }

        Ok(())
    }

    async fn read(&mut self) -> TrezorResult<ProtoMessage> {
        let chunk = self.link.read_chunk(CHUNK_LEN).await?;
        if chunk.len() < CHUNK_HEADER_LEN {
            return MmError::err(TrezorError::ProtocolError(format!(
                "Invalid chunk length '{}', expected at least '{}'",
                chunk.len(),
                CHUNK_HEADER_LEN
            )));
        }
        if chunk[0] != 0x3f || chunk[1] != 0x23 || chunk[2] != 0x23 {
            let error = format!(
                "bad magic in v1 read: 0x{:x}{:x}{:x} instead of 0x3f2323",
                chunk[0], chunk[1], chunk[2]
            );
            return MmError::err(TrezorError::ProtocolError(error));
        }
        let message_type_id = BigEndian::read_u16(&chunk[3..5]) as i32;
        let message_type = MessageType::try_from(message_type_id)
            .map_err(|e| TrezorError::ProtocolError(format!("Invalid message type: {message_type_id}, Error: {e}")))?;
        let data_length = BigEndian::read_u32(&chunk[5..9]) as usize;
        let mut data: Vec<u8> = chunk[9..].into();

        while data.len() < data_length {
            let chunk = self.link.read_chunk(CHUNK_LEN).await?;
            if chunk[0] != 0x3f {
                let error = format!("bad magic in v1 read: {:x} instead of 0x3f", chunk[0]);
                return MmError::err(TrezorError::ProtocolError(error));
            }

            data.extend(&chunk[1..]);
        }

        Ok(ProtoMessage::new(message_type, data[0..data_length].into()))
    }
}
