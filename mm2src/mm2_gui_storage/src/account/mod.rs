use crate::account::storage::AccountStorageError;
use mm2_err_handle::prelude::*;
use mm2_number::BigDecimal;
use rpc::v1::types::H160 as H160Json;
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::collections::BTreeSet;

pub(crate) mod storage;

pub const MAX_ACCOUNT_NAME_LENGTH: usize = 255;
pub const MAX_ACCOUNT_DESCRIPTION_LENGTH: usize = 600;
pub const MAX_TICKER_LENGTH: usize = 255;

pub(crate) type HwPubkey = H160Json;

#[derive(Clone, Copy, Debug, Deserialize_repr, Serialize_repr)]
#[repr(u8)]
pub(crate) enum AccountType {
    Iguana = 0,
    HD = 1,
    HW = 2,
}

impl TryFrom<i64> for AccountType {
    type Error = MmError<AccountStorageError>;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(AccountType::Iguana),
            1 => Ok(AccountType::HD),
            2 => Ok(AccountType::HW),
            other => {
                let error = format!("Unknown 'account_type' value: {other}");
                MmError::err(AccountStorageError::ErrorDeserializing(error))
            },
        }
    }
}

/// An enable account type.
/// We should not allow the user to enable [`AccountType::HW`].
#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[repr(u8)]
pub(crate) enum EnabledAccountType {
    Iguana = 0,
    HD = 1,
}

impl TryFrom<i64> for EnabledAccountType {
    type Error = MmError<AccountStorageError>;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        match AccountType::try_from(value)? {
            AccountType::Iguana => Ok(EnabledAccountType::Iguana),
            AccountType::HD => Ok(EnabledAccountType::HD),
            AccountType::HW => MmError::err(AccountStorageError::ErrorDeserializing(
                "HW account cannot be enabled".to_string(),
            )),
        }
    }
}

impl From<EnabledAccountType> for AccountType {
    fn from(t: EnabledAccountType) -> Self {
        match t {
            EnabledAccountType::Iguana => AccountType::Iguana,
            EnabledAccountType::HD => AccountType::HD,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "lowercase")]
pub enum AccountId {
    Iguana,
    HD { account_idx: u32 },
    HW { device_pubkey: HwPubkey },
}

#[derive(Copy, Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "lowercase")]
pub enum EnabledAccountId {
    Iguana,
    HD { account_idx: u32 },
}

impl From<EnabledAccountId> for AccountId {
    fn from(id: EnabledAccountId) -> Self {
        match id {
            EnabledAccountId::Iguana => AccountId::Iguana,
            EnabledAccountId::HD { account_idx } => AccountId::HD { account_idx },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct AccountInfo {
    pub(crate) account_id: AccountId,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) balance_usd: BigDecimal,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct AccountWithEnabledFlag {
    #[serde(flatten)]
    account_info: AccountInfo,
    /// Whether this account is enabled.
    /// This flag is expected to be `true` for one account **only**.
    enabled: bool,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct AccountWithCoins {
    #[serde(flatten)]
    account_info: AccountInfo,
    coins: BTreeSet<String>,
}
