use crate::hd_wallet::{
    DisplayAddress, ExtendedPublicKeyOps, HDAccount, HDAccountMut, HDAccountOps, HDAccountsMap, HDAccountsMut,
    HDAccountsMutex, HDAddress, HDWallet, HDWalletOps,
};
use crate::siacoin::{Address, PublicKey};

use async_trait::async_trait;
use crypto::{Bip32Error, Bip44Chain, ChildNumber, HDPathToCoin};
use ed25519_dalek_bip32::ExtendedSigningKey;
use std::str::FromStr;
use std::sync::Arc;

// TODO Alright - I began to do the HD wallet implementation, but the decision was made to simply
// use `m/44'/1991'/0'/0'/0` and use this key as PrivKeyBuildPolicy::IguanaPrivKey. This means users
// will not need to do any migration of their seed phrases when HD wallet support is added.
// It was simpler to use PrivKeyBuildPolicy::IguanaPrivKey because all the logic already assumes
// a single address per seed phrease.

impl DisplayAddress for Address {
    fn display_address(&self) -> String {
        self.to_string()
    }
}
pub type SiaHDAddress = HDAddress<Address, PublicKey>;

pub type SiaHDAccount = HDAccount<SiaHDAddress, SiaFauxExtendedPublicKey>;

// See the crypto::GlobalHDAccountCtx dev comment regarding security considerations!
// this will be left unfinished in the RC, but this was intended to be the type of abstraction
// mentioned in the dev comment. The hope was to integrate cleanly into the existing HD wallet traits.
// If this is pattern implemented, this type must be treated securely.
#[allow(dead_code)]
pub struct SiaFauxExtendedPublicKey(Arc<ExtendedSigningKey>);

impl FromStr for SiaFauxExtendedPublicKey {
    type Err = String;

    fn from_str(_: &str) -> Result<Self, Self::Err> {
        todo!()
    }
}

impl ExtendedPublicKeyOps for SiaFauxExtendedPublicKey {
    fn derive_child(&self, _: ChildNumber) -> Result<Self, Bip32Error> {
        todo!()
    }

    fn to_string(&self, _: bip32::Prefix) -> String {
        todo!()
    }
}

#[allow(dead_code)]
pub struct SiaHdWallet(HDWallet<SiaHDAccount>);

#[async_trait]
impl HDWalletOps for SiaHdWallet {
    /// Any type that represents a Hierarchical Deterministic (HD) wallet account.
    type HDAccount = SiaHDAccount;

    /// Returns the coin type associated with this HD Wallet.
    ///
    /// This method should be implemented to fetch the coin type as specified in the wallet's BIP44 derivation path.
    /// For example, in the derivation path `m/44'/0'/0'/0`, the coin type would be the third level `0'`
    /// (representing Bitcoin).
    fn coin_type(&self) -> u32 {
        todo!()
    }

    /// Returns the derivation path associated with this HD Wallet. This is the path used to derive the accounts.
    fn derivation_path(&self) -> &HDPathToCoin {
        todo!()
    }

    /// Fetches the gap limit associated with this HD Wallet.
    /// Gap limit is the maximum number of consecutive unused addresses in an account
    /// that should be checked before considering the wallet as having no more funds.
    fn gap_limit(&self) -> u32 {
        todo!()
    }

    /// Returns the limit on the number of accounts that can be added to the wallet.
    fn account_limit(&self) -> u32 {
        todo!()
    }

    /// Returns the default BIP44 chain for receiver addresses.
    fn default_receiver_chain(&self) -> Bip44Chain {
        todo!()
    }

    /// Returns a mutex that can be used to access the accounts.
    fn get_accounts_mutex(&self) -> &HDAccountsMutex<Self::HDAccount> {
        todo!()
    }

    /// Fetches an account based on its ID. This method will return `None` if the account is not activated.
    async fn get_account(&self, _account_id: u32) -> Option<Self::HDAccount> {
        todo!()
    }

    /// Similar to `get_account`, but provides a mutable reference.
    async fn get_account_mut(&self, _account_id: u32) -> Option<HDAccountMut<'_, Self::HDAccount>> {
        todo!()
    }

    /// Fetches all accounts in the wallet.
    async fn get_accounts(&self) -> HDAccountsMap<Self::HDAccount> {
        todo!()
    }

    /// Similar to `get_accounts`, but provides a mutable reference to the accounts.
    async fn get_accounts_mut(&self) -> HDAccountsMut<'_, Self::HDAccount> {
        todo!()
    }

    /// Attempts to remove an account only if it's the last in the set.
    /// This method will return the removed account if successful or `None` otherwise.
    async fn remove_account_if_last(&self, _account_id: u32) -> Option<Self::HDAccount> {
        todo!()
    }

    /// Returns an address that's currently enabled for single-address operations, such as swaps.
    async fn get_enabled_address(&self) -> Option<<Self::HDAccount as HDAccountOps>::HDAddress> {
        todo!()
    }
}
