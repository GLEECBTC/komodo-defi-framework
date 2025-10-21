use super::{ibc_proto::IBCTransferV1Proto, IBC_OUT_SOURCE_PORT, IBC_OUT_TIMEOUT_IN_NANOS};
use cosmrs::proto::traits::Name;
use cosmrs::{tx::Msg, AccountId, Coin, ErrorReport};
use std::convert::TryFrom;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MsgTransfer {
    /// the port on which the packet will be sent
    pub(crate) source_port: String,
    /// the channel by which the packet will be sent
    pub(crate) source_channel: String,
    /// the tokens to be transferred
    pub(crate) token: Coin,
    /// the sender address
    pub(crate) sender: AccountId,
    /// the recipient address on the destination chain
    pub(crate) receiver: AccountId,
    /// Timeout height relative to the current block height.
    /// The timeout is disabled when set to 0.
    pub(crate) timeout_height: Option<cosmrs::tendermint::block::Height>,
    /// Timeout timestamp in absolute nanoseconds since unix epoch.
    /// The timeout is disabled when set to 0.
    pub(crate) timeout_timestamp: u64,
    // Not supported by some of the cosmos chains like IRIS
    // pub(crate) memo: Option<String>,
}

impl MsgTransfer {
    pub(crate) fn new_with_default_timeout(
        source_channel: String,
        sender: AccountId,
        receiver: AccountId,
        token: Coin,
    ) -> Result<Self, String> {
        let timestamp_as_nanos = common::get_utc_timestamp_nanos()? as u64;

        Ok(Self {
            source_port: IBC_OUT_SOURCE_PORT.to_owned(),
            source_channel,
            sender,
            receiver,
            token,
            timeout_height: None,
            timeout_timestamp: timestamp_as_nanos + IBC_OUT_TIMEOUT_IN_NANOS,
            // memo: Some(memo.clone()),
        })
    }
}

impl Msg for MsgTransfer {
    type Proto = IBCTransferV1Proto;
}

impl TryFrom<IBCTransferV1Proto> for MsgTransfer {
    type Error = ErrorReport;

    #[inline(always)]
    fn try_from(proto: IBCTransferV1Proto) -> Result<MsgTransfer, Self::Error> {
        MsgTransfer::try_from(&proto)
    }
}

impl TryFrom<&IBCTransferV1Proto> for MsgTransfer {
    type Error = ErrorReport;

    fn try_from(proto: &IBCTransferV1Proto) -> Result<MsgTransfer, Self::Error> {
        Ok(MsgTransfer {
            source_port: proto.source_port.to_owned(),
            source_channel: proto.source_channel.to_owned(),
            token: proto
                .token
                .to_owned()
                .map(TryFrom::try_from)
                .ok_or_else(|| ErrorReport::msg("token can't be empty"))??,
            sender: proto.sender.parse()?,
            receiver: proto.receiver.parse()?,
            timeout_height: None,
            timeout_timestamp: proto.timeout_timestamp,
            // memo: proto.memo.to_owned(),
        })
    }
}

impl From<MsgTransfer> for IBCTransferV1Proto {
    fn from(coin: MsgTransfer) -> IBCTransferV1Proto {
        IBCTransferV1Proto::from(&coin)
    }
}

impl From<&MsgTransfer> for IBCTransferV1Proto {
    fn from(msg: &MsgTransfer) -> IBCTransferV1Proto {
        IBCTransferV1Proto {
            source_port: msg.source_port.to_owned(),
            source_channel: msg.source_channel.to_owned(),
            token: Some(msg.token.to_owned().into()),
            sender: msg.sender.to_string(),
            receiver: msg.receiver.to_string(),
            timeout_height: None,
            timeout_timestamp: msg.timeout_timestamp,
            // memo: msg.memo.to_owned(),
        }
    }
}

impl Name for IBCTransferV1Proto {
    const NAME: &'static str = "MsgTransfer";
    const PACKAGE: &'static str = "ibc.applications.transfer.v1";
}
