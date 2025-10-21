use super::htlc_proto::{NucleusClaimHtlcProto, NucleusCreateHtlcProto};

use cosmrs::proto::traits::Name;
use cosmrs::{tx::Msg, AccountId, Coin, ErrorReport};
use std::convert::TryFrom;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct NucleusCreateHtlcMsg {
    /// Sender's address.
    pub(crate) to: AccountId,

    /// Recipient's address.
    pub(crate) sender: AccountId,

    /// Amount to send.
    pub(crate) amount: Vec<Coin>,

    /// The sha256 hash generated from secret and timestamp.
    pub(crate) hash_lock: String,

    /// The number of blocks to wait before the asset may be returned to.
    pub(crate) time_lock: u64,

    /// The timestamp in seconds for generating hash lock if provided.
    pub(crate) timestamp: u64,
}

impl Msg for NucleusCreateHtlcMsg {
    type Proto = NucleusCreateHtlcProto;
}

impl TryFrom<NucleusCreateHtlcProto> for NucleusCreateHtlcMsg {
    type Error = ErrorReport;

    fn try_from(proto: NucleusCreateHtlcProto) -> Result<NucleusCreateHtlcMsg, Self::Error> {
        NucleusCreateHtlcMsg::try_from(&proto)
    }
}

impl TryFrom<&NucleusCreateHtlcProto> for NucleusCreateHtlcMsg {
    type Error = ErrorReport;

    fn try_from(proto: &NucleusCreateHtlcProto) -> Result<NucleusCreateHtlcMsg, Self::Error> {
        Ok(NucleusCreateHtlcMsg {
            sender: proto.sender.parse()?,
            to: proto.to.parse()?,
            amount: proto.amount.iter().map(TryFrom::try_from).collect::<Result<_, _>>()?,
            hash_lock: proto.hash_lock.clone(),
            timestamp: proto.timestamp,
            time_lock: proto.time_lock,
        })
    }
}

impl From<NucleusCreateHtlcMsg> for NucleusCreateHtlcProto {
    fn from(coin: NucleusCreateHtlcMsg) -> NucleusCreateHtlcProto {
        NucleusCreateHtlcProto::from(&coin)
    }
}

impl From<&NucleusCreateHtlcMsg> for NucleusCreateHtlcProto {
    fn from(msg: &NucleusCreateHtlcMsg) -> NucleusCreateHtlcProto {
        NucleusCreateHtlcProto {
            sender: msg.sender.to_string(),
            to: msg.to.to_string(),
            amount: msg.amount.iter().map(Into::into).collect(),
            hash_lock: msg.hash_lock.clone(),
            timestamp: msg.timestamp,
            time_lock: msg.time_lock,
        }
    }
}

impl Name for NucleusCreateHtlcProto {
    const NAME: &'static str = "MsgCreateHTLC";
    const PACKAGE: &'static str = "nucleus.htlc";
}

#[derive(Clone)]
pub(crate) struct NucleusClaimHtlcMsg {
    /// Sender's address.
    pub(crate) sender: AccountId,

    /// Generated HTLC ID
    pub(crate) id: String,

    /// Secret that has been used for generating hash_lock
    pub(crate) secret: String,
}

impl Msg for NucleusClaimHtlcMsg {
    type Proto = NucleusClaimHtlcProto;
}

impl TryFrom<NucleusClaimHtlcProto> for NucleusClaimHtlcMsg {
    type Error = ErrorReport;

    fn try_from(proto: NucleusClaimHtlcProto) -> Result<NucleusClaimHtlcMsg, Self::Error> {
        NucleusClaimHtlcMsg::try_from(&proto)
    }
}

impl TryFrom<&NucleusClaimHtlcProto> for NucleusClaimHtlcMsg {
    type Error = ErrorReport;

    fn try_from(proto: &NucleusClaimHtlcProto) -> Result<NucleusClaimHtlcMsg, Self::Error> {
        Ok(NucleusClaimHtlcMsg {
            sender: proto.sender.parse()?,
            id: proto.id.clone(),
            secret: proto.secret.clone(),
        })
    }
}

impl From<NucleusClaimHtlcMsg> for NucleusClaimHtlcProto {
    fn from(coin: NucleusClaimHtlcMsg) -> NucleusClaimHtlcProto {
        NucleusClaimHtlcProto::from(&coin)
    }
}

impl From<&NucleusClaimHtlcMsg> for NucleusClaimHtlcProto {
    fn from(msg: &NucleusClaimHtlcMsg) -> NucleusClaimHtlcProto {
        NucleusClaimHtlcProto {
            sender: msg.sender.to_string(),
            id: msg.id.clone(),
            secret: msg.secret.clone(),
        }
    }
}

impl Name for NucleusClaimHtlcProto {
    const NAME: &'static str = "MsgClaimHTLC";
    const PACKAGE: &'static str = "nucleus.htlc";
}
