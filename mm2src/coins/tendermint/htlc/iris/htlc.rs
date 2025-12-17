use super::htlc_proto::{IrisClaimHtlcProto, IrisCreateHtlcProto};

use cosmrs::proto::traits::Name;
use cosmrs::{tx::Msg, AccountId, Coin, ErrorReport};
use std::convert::TryFrom;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct IrisCreateHtlcMsg {
    /// Sender's address.
    pub(crate) to: AccountId,

    /// Recipient's address.
    pub(crate) sender: AccountId,

    /// The claim receiving address on the other chain.
    pub(crate) receiver_on_other_chain: String,

    /// The counterparty creator address on the other chain.
    pub(crate) sender_on_other_chain: String,

    /// Amount to send.
    pub(crate) amount: Vec<Coin>,

    /// The sha256 hash generated from secret and timestamp.
    pub(crate) hash_lock: String,

    /// The number of blocks to wait before the asset may be returned to.
    pub(crate) time_lock: u64,

    /// The timestamp in seconds for generating hash lock if provided.
    pub(crate) timestamp: u64,

    /// Whether it is an HTLT transaction.
    pub(crate) transfer: bool,
}

impl Msg for IrisCreateHtlcMsg {
    type Proto = IrisCreateHtlcProto;
}

impl TryFrom<IrisCreateHtlcProto> for IrisCreateHtlcMsg {
    type Error = ErrorReport;

    fn try_from(proto: IrisCreateHtlcProto) -> Result<IrisCreateHtlcMsg, Self::Error> {
        IrisCreateHtlcMsg::try_from(&proto)
    }
}

impl TryFrom<&IrisCreateHtlcProto> for IrisCreateHtlcMsg {
    type Error = ErrorReport;

    fn try_from(proto: &IrisCreateHtlcProto) -> Result<IrisCreateHtlcMsg, Self::Error> {
        Ok(IrisCreateHtlcMsg {
            sender: proto.sender.parse()?,
            to: proto.to.parse()?,
            amount: proto.amount.iter().map(TryFrom::try_from).collect::<Result<_, _>>()?,
            receiver_on_other_chain: proto.receiver_on_other_chain.clone(),
            sender_on_other_chain: proto.sender_on_other_chain.clone(),
            hash_lock: proto.hash_lock.clone(),
            timestamp: proto.timestamp,
            time_lock: proto.time_lock,
            transfer: proto.transfer,
        })
    }
}

impl From<IrisCreateHtlcMsg> for IrisCreateHtlcProto {
    fn from(t: IrisCreateHtlcMsg) -> IrisCreateHtlcProto {
        IrisCreateHtlcProto::from(&t)
    }
}

impl From<&IrisCreateHtlcMsg> for IrisCreateHtlcProto {
    fn from(msg: &IrisCreateHtlcMsg) -> IrisCreateHtlcProto {
        IrisCreateHtlcProto {
            sender: msg.sender.to_string(),
            to: msg.to.to_string(),
            amount: msg.amount.iter().map(Into::into).collect(),
            receiver_on_other_chain: msg.receiver_on_other_chain.clone(),
            sender_on_other_chain: msg.sender_on_other_chain.clone(),
            hash_lock: msg.hash_lock.clone(),
            timestamp: msg.timestamp,
            time_lock: msg.time_lock,
            transfer: msg.transfer,
        }
    }
}

impl Name for IrisCreateHtlcProto {
    const NAME: &'static str = "MsgCreateHTLC";
    const PACKAGE: &'static str = "irismod.htlc";
}

#[derive(Clone)]
pub(crate) struct IrisClaimHtlcMsg {
    /// Sender's address.
    pub(crate) sender: AccountId,

    /// Generated HTLC ID
    pub(crate) id: String,

    /// Secret that has been used for generating hash_lock
    pub(crate) secret: String,
}

impl Msg for IrisClaimHtlcMsg {
    type Proto = IrisClaimHtlcProto;
}

impl TryFrom<IrisClaimHtlcProto> for IrisClaimHtlcMsg {
    type Error = ErrorReport;

    fn try_from(proto: IrisClaimHtlcProto) -> Result<IrisClaimHtlcMsg, Self::Error> {
        IrisClaimHtlcMsg::try_from(&proto)
    }
}

impl TryFrom<&IrisClaimHtlcProto> for IrisClaimHtlcMsg {
    type Error = ErrorReport;

    fn try_from(proto: &IrisClaimHtlcProto) -> Result<IrisClaimHtlcMsg, Self::Error> {
        Ok(IrisClaimHtlcMsg {
            sender: proto.sender.parse()?,
            id: proto.id.clone(),
            secret: proto.secret.clone(),
        })
    }
}

impl From<IrisClaimHtlcMsg> for IrisClaimHtlcProto {
    fn from(coin: IrisClaimHtlcMsg) -> IrisClaimHtlcProto {
        IrisClaimHtlcProto::from(&coin)
    }
}

impl From<&IrisClaimHtlcMsg> for IrisClaimHtlcProto {
    fn from(msg: &IrisClaimHtlcMsg) -> IrisClaimHtlcProto {
        IrisClaimHtlcProto {
            sender: msg.sender.to_string(),
            id: msg.id.clone(),
            secret: msg.secret.clone(),
        }
    }
}

impl Name for IrisClaimHtlcProto {
    const NAME: &'static str = "MsgClaimHTLC";
    const PACKAGE: &'static str = "irismod.htlc";
}
