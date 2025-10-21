use crate::my_tx_history_v2::TxHistoryStorage;
use crate::TransactionType;
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use num_traits::Zero;
use primitives::hash::H160;
use serde::de::Error;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::HashSet;
use std::iter::FromIterator;

#[cfg(target_arch = "wasm32")]
pub mod wasm;

#[cfg(not(target_arch = "wasm32"))]
pub mod sql_tx_history_storage_v2;

#[cfg(any(test, target_arch = "wasm32"))]
mod tx_history_v2_tests;

/// Get `token_id` from the transaction type.
/// Returns an empty `token_id` if the transaction is [`TransactionType::StandardTransfer`].
#[inline]
pub fn token_id_from_tx_type(tx_type: &TransactionType) -> String {
    match tx_type {
        TransactionType::TokenTransfer(token_id)
        | TransactionType::TendermintIBCTransfer {
            token_id: Some(token_id),
        }
        | TransactionType::CustomTendermintMsg {
            token_id: Some(token_id),
            ..
        } => {
            format!("{token_id:02x}")
        },
        _ => String::new(),
    }
}

#[derive(Debug, Display)]
pub enum CreateTxHistoryStorageError {
    Internal(String),
}

/// `TxHistoryStorageBuilder` is used to create an instance that implements the `TxHistoryStorage` trait.
pub struct TxHistoryStorageBuilder<'a> {
    ctx: &'a MmArc,
}

impl TxHistoryStorageBuilder<'_> {
    #[inline]
    pub fn new(ctx: &MmArc) -> TxHistoryStorageBuilder<'_> {
        TxHistoryStorageBuilder { ctx }
    }

    #[inline]
    pub fn build(self) -> MmResult<impl TxHistoryStorage, CreateTxHistoryStorageError> {
        #[cfg(target_arch = "wasm32")]
        return wasm::IndexedDbTxHistoryStorage::new(self.ctx);
        #[cfg(not(target_arch = "wasm32"))]
        sql_tx_history_storage_v2::SqliteTxHistoryStorage::new(self.ctx)
    }
}

/// Whether transaction is unconfirmed or confirmed.
/// Serializes to either `0` or `1` correspondingly.
#[derive(Clone, Copy, Debug)]
pub enum ConfirmationStatus {
    Unconfirmed = 0,
    Confirmed = 1,
}

impl ConfirmationStatus {
    #[inline]
    pub fn from_block_height<Height: Zero>(height: Height) -> ConfirmationStatus {
        if height.is_zero() {
            ConfirmationStatus::Unconfirmed
        } else {
            ConfirmationStatus::Confirmed
        }
    }
}

impl Serialize for ConfirmationStatus {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        (*self as u8).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for ConfirmationStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let code = u8::deserialize(deserializer)?;
        match code {
            0 => Ok(ConfirmationStatus::Unconfirmed),
            1 => Ok(ConfirmationStatus::Confirmed),
            unknown => Err(D::Error::custom(format!(
                "Expected either '0' or '1' confirmation status, found '{unknown}'"
            ))),
        }
    }
}

#[derive(Clone, Debug)]
pub struct WalletId {
    ticker: String,
    hd_wallet_rmd160: Option<H160>,
}

impl WalletId {
    #[inline]
    pub fn new(ticker: String) -> WalletId {
        WalletId {
            ticker: ticker.replace('-', "_"),
            hd_wallet_rmd160: None,
        }
    }

    #[inline]
    pub fn set_hd_wallet_rmd160(&mut self, hd_wallet_rmd160: H160) {
        self.hd_wallet_rmd160 = Some(hd_wallet_rmd160);
    }

    #[inline]
    pub fn with_hd_wallet_rmd160(mut self, hd_wallet_rmd160: H160) -> WalletId {
        self.set_hd_wallet_rmd160(hd_wallet_rmd160);
        self
    }
}

#[derive(Clone, Debug)]
pub struct GetTxHistoryFilters {
    token_id: Option<String>,
    for_addresses: FilteringAddresses,
}

impl GetTxHistoryFilters {
    #[inline]
    pub fn for_address(address: String) -> GetTxHistoryFilters {
        GetTxHistoryFilters {
            token_id: None,
            for_addresses: std::iter::once(address).collect(),
        }
    }

    #[inline]
    pub fn for_addresses<I: IntoIterator<Item = String>>(addresses: I) -> GetTxHistoryFilters {
        GetTxHistoryFilters {
            token_id: None,
            for_addresses: addresses.into_iter().collect(),
        }
    }

    #[inline]
    pub fn with_token_id(mut self, token_id: String) -> GetTxHistoryFilters {
        self.set_token_id(token_id);
        self
    }

    #[inline]
    pub fn set_token_id(&mut self, token_id: String) {
        self.token_id = Some(token_id);
    }

    /// If [`GetTxHistoryFilters::token_id`] is not specified,
    /// we should exclude token's transactions by applying an empty `token_id` filter.
    fn token_id_or_exclude(&self) -> String {
        self.token_id.clone().unwrap_or_default()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct FilteringAddresses(HashSet<String>);

impl FilteringAddresses {
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the containers have the same addresses.
    #[inline]
    pub fn has_intersection(&self, other: &FilteringAddresses) -> bool {
        self.0.intersection(&other.0).next().is_some()
    }
}

impl IntoIterator for FilteringAddresses {
    type Item = String;
    type IntoIter = std::collections::hash_set::IntoIter<String>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl FromIterator<String> for FilteringAddresses {
    fn from_iter<T: IntoIterator<Item = String>>(iter: T) -> Self {
        FilteringAddresses(iter.into_iter().collect())
    }
}
