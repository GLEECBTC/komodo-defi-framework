use super::{HDAccountMut, HDAccountOps, HDAccountsMap, HDAccountsMut, HDAccountsMutex};
use async_trait::async_trait;
use crypto::{Bip44Chain, HDPathToCoin};

/// `HDWalletOps`: Operations that should be implemented for Structs or any type that represents HD wallets.
#[async_trait]
pub trait HDWalletOps {
    /// Any type that represents a Hierarchical Deterministic (HD) wallet account.
    type HDAccount: HDAccountOps + Send + Sync;

    /// Returns the coin type associated with this HD Wallet.
    ///
    /// This method should be implemented to fetch the coin type as specified in the wallet's BIP44 derivation path.
    /// For example, in the derivation path `m/44'/0'/0'/0`, the coin type would be the third level `0'`
    /// (representing Bitcoin).
    fn coin_type(&self) -> u32;

    /// Returns the derivation path associated with this HD Wallet. This is the path used to derive the accounts.
    fn derivation_path(&self) -> &HDPathToCoin;

    /// Fetches the gap limit associated with this HD Wallet.
    /// Gap limit is the maximum number of consecutive unused addresses in an account
    /// that should be checked before considering the wallet as having no more funds.
    fn gap_limit(&self) -> u32;

    /// Returns the limit on the number of accounts that can be added to the wallet.
    fn account_limit(&self) -> u32;

    /// Returns the default BIP44 chain for receiver addresses.
    fn default_receiver_chain(&self) -> Bip44Chain;

    /// Returns a mutex that can be used to access the accounts.
    fn get_accounts_mutex(&self) -> &HDAccountsMutex<Self::HDAccount>;

    /// Fetches an account based on its ID. This method will return `None` if the account is not activated.
    async fn get_account(&self, account_id: u32) -> Option<Self::HDAccount>;

    /// Similar to `get_account`, but provides a mutable reference.
    async fn get_account_mut(&self, account_id: u32) -> Option<HDAccountMut<'_, Self::HDAccount>>;

    /// Fetches all accounts in the wallet.
    async fn get_accounts(&self) -> HDAccountsMap<Self::HDAccount>;

    /// Similar to `get_accounts`, but provides a mutable reference to the accounts.
    async fn get_accounts_mut(&self) -> HDAccountsMut<'_, Self::HDAccount>;

    /// Attempts to remove an account only if it's the last in the set.
    /// This method will return the removed account if successful or `None` otherwise.
    async fn remove_account_if_last(&self, account_id: u32) -> Option<Self::HDAccount>;

    /// Returns an address that's currently enabled for single-address operations, such as swaps.
    async fn get_enabled_address(&self) -> Option<<Self::HDAccount as HDAccountOps>::HDAddress>;
}
