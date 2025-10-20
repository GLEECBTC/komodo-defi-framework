use crate::hd_wallet::{
    DisplayAddress, HDAccount, HDAccountMut, HDAccountOps, HDAccountsMap, HDAccountsMut, HDAccountsMutex, HDAddress,
    HDWallet, HDWalletCoinStorage, HDWalletOps, HDWalletStorageOps, WithdrawSenderAddress,
};
use async_trait::async_trait;
use crypto::{Bip44Chain, HDPathToCoin, Secp256k1ExtendedPublicKey};
use keys::{Address, AddressFormat as UtxoAddressFormat, CashAddress, Public};

pub type UtxoHDAddress = HDAddress<Address, Public>;
pub type UtxoHDAccount = HDAccount<UtxoHDAddress, Secp256k1ExtendedPublicKey>;
pub type UtxoWithdrawSender = WithdrawSenderAddress<Address, Public>;

/// A struct to encapsulate the types needed for a UTXO HD wallet.
pub struct UtxoHDWallet {
    /// The inner HD wallet field that makes use of the generic `HDWallet` struct.
    pub inner: HDWallet<UtxoHDAccount>,
    /// Specifies the UTXO address format for all addresses in the wallet.
    pub address_format: UtxoAddressFormat,
}

#[async_trait]
impl HDWalletStorageOps for UtxoHDWallet {
    fn hd_wallet_storage(&self) -> &HDWalletCoinStorage {
        self.inner.hd_wallet_storage()
    }
}

#[async_trait]
impl HDWalletOps for UtxoHDWallet {
    type HDAccount = UtxoHDAccount;

    fn coin_type(&self) -> u32 {
        self.inner.coin_type()
    }

    fn derivation_path(&self) -> &HDPathToCoin {
        self.inner.derivation_path()
    }

    fn gap_limit(&self) -> u32 {
        self.inner.gap_limit()
    }

    fn account_limit(&self) -> u32 {
        self.inner.account_limit()
    }

    fn default_receiver_chain(&self) -> Bip44Chain {
        self.inner.default_receiver_chain()
    }

    fn get_accounts_mutex(&self) -> &HDAccountsMutex<Self::HDAccount> {
        self.inner.get_accounts_mutex()
    }

    async fn get_account(&self, account_id: u32) -> Option<Self::HDAccount> {
        self.inner.get_account(account_id).await
    }

    async fn get_account_mut(&self, account_id: u32) -> Option<HDAccountMut<'_, Self::HDAccount>> {
        self.inner.get_account_mut(account_id).await
    }

    async fn get_accounts(&self) -> HDAccountsMap<Self::HDAccount> {
        self.inner.get_accounts().await
    }

    async fn get_accounts_mut(&self) -> HDAccountsMut<'_, Self::HDAccount> {
        self.inner.get_accounts_mut().await
    }

    async fn remove_account_if_last(&self, account_id: u32) -> Option<Self::HDAccount> {
        self.inner.remove_account_if_last(account_id).await
    }

    async fn get_enabled_address(&self) -> Option<<Self::HDAccount as HDAccountOps>::HDAddress> {
        self.inner.get_enabled_address().await
    }
}

impl DisplayAddress for Address {
    fn display_address(&self) -> String {
        self.to_string()
    }
}

impl DisplayAddress for CashAddress {
    fn display_address(&self) -> String {
        self.encode().expect("A valid cash address")
    }
}
