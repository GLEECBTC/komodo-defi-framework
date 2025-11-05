use async_trait::async_trait;
use common::log::warn;
use crypto::{
    Bip32DerPathOps, Bip32Error, Bip44Chain, ChildNumber, DerivationPath, HDPathToAccount, HDPathToCoin,
    Secp256k1ExtendedPublicKey, StandardHDPath, StandardHDPathError,
};
use futures::lock::{MappedMutexGuard as AsyncMappedMutexGuard, Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard};
use mm2_err_handle::prelude::*;
use primitives::hash::H160;
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Display;
use std::hash::Hash;
use std::str::FromStr;
use std::sync::Arc;

mod account_ops;
pub use account_ops::HDAccountOps;

mod address_ops;
pub use address_ops::{AddrToString, DisplayAddress, HDAddressOps};

mod coin_ops;
pub use coin_ops::{HDAddressId, HDWalletCoinOps};

mod confirm_address;
#[cfg(test)]
pub(crate) use confirm_address::for_tests::MockableConfirmAddress;
pub(crate) use confirm_address::{ConfirmAddressStatus, RpcTaskConfirmAddress};
pub use confirm_address::{HDConfirmAddress, HDConfirmAddressError};

mod errors;
pub use errors::{
    AccountUpdatingError, AddressDerivingError, HDExtractPubkeyError, HDWithdrawError, InvalidBip44ChainError,
    NewAccountCreationError, NewAddressDeriveConfirmError, NewAddressDerivingError, SettingEnabledAddressError,
    TrezorCoinError,
};

mod pubkey;
pub use pubkey::{ExtendedPublicKeyOps, ExtractExtendedPubkey, HDXPubExtractor, RpcTaskXPubExtractor};

mod storage;
#[cfg(target_arch = "wasm32")]
pub(crate) use storage::HDWalletDb;
#[cfg(test)]
pub(crate) use storage::HDWalletMockStorage;
pub use storage::{
    HDAccountStorageItem, HDAccountStorageOps, HDWalletCoinStorage, HDWalletId, HDWalletStorageError,
    HDWalletStorageOps,
};
pub(crate) use storage::{HDWalletStorageInternalOps, HDWalletStorageResult};

mod wallet_ops;
pub use wallet_ops::HDWalletOps;

mod withdraw_ops;
pub use withdraw_ops::{HDCoinWithdrawOps, WithdrawSenderAddress};

pub(crate) type HDAccountsMap<HDAccount> = BTreeMap<u32, HDAccount>;
pub(crate) type HDAccountsMutex<HDAccount> = AsyncMutex<HDAccountsMap<HDAccount>>;
pub(crate) type HDAccountsMut<'a, HDAccount> = AsyncMutexGuard<'a, HDAccountsMap<HDAccount>>;
pub(crate) type HDAccountMut<'a, HDAccount> = AsyncMappedMutexGuard<'a, HDAccountsMap<HDAccount>, HDAccount>;
type HDWalletHDAddress<T> = <<T as HDWalletOps>::HDAccount as HDAccountOps>::HDAddress;
type HDCoinHDAddress<T> = HDWalletHDAddress<<T as HDWalletCoinOps>::HDWallet>;
pub(crate) type HDWalletAddress<T> =
    <<<T as HDWalletOps>::HDAccount as HDAccountOps>::HDAddress as HDAddressOps>::Address;
pub(crate) type HDCoinAddress<T> = HDWalletAddress<<T as HDWalletCoinOps>::HDWallet>;
type HDWalletExtendedPubkey<T> = <<T as HDWalletOps>::HDAccount as HDAccountOps>::ExtendedPublicKey;
pub(crate) type HDCoinExtendedPubkey<T> = HDWalletExtendedPubkey<<T as HDWalletCoinOps>::HDWallet>;
pub(crate) type HDCoinHDAccount<T> = HDWalletHDAccount<<T as HDWalletCoinOps>::HDWallet>;
type HDWalletHDAccount<T> = <T as HDWalletOps>::HDAccount;

pub(crate) const DEFAULT_GAP_LIMIT: u32 = 20;
const DEFAULT_ACCOUNT_LIMIT: u32 = ChildNumber::HARDENED_FLAG;
const DEFAULT_ADDRESS_LIMIT: u32 = ChildNumber::HARDENED_FLAG;
const DEFAULT_RECEIVER_CHAIN: Bip44Chain = Bip44Chain::External;

/// A generic HD address that can be used with any HD wallet.
#[derive(Clone)]
pub struct HDAddress<Address, Pubkey> {
    pub address: Address,
    pub pubkey: Pubkey,
    pub derivation_path: DerivationPath,
}

impl<Address, Pubkey> HDAddressOps for HDAddress<Address, Pubkey>
where
    Address: Clone + DisplayAddress + Eq + Hash + Send + Sync,
    Pubkey: Clone,
{
    type Address = Address;
    type Pubkey = Pubkey;

    fn address(&self) -> Self::Address {
        self.address.clone()
    }

    fn pubkey(&self) -> Self::Pubkey {
        self.pubkey.clone()
    }

    fn derivation_path(&self) -> &DerivationPath {
        &self.derivation_path
    }
}

/// A generic HD address that can be used with any HD wallet.
#[derive(Clone, Debug)]
pub struct HDAddressesCache<HDAddress> {
    cache: Arc<AsyncMutex<HashMap<HDAddressId, HDAddress>>>,
}

impl<HDAddress> Default for HDAddressesCache<HDAddress> {
    fn default() -> Self {
        HDAddressesCache {
            cache: Arc::new(AsyncMutex::new(HashMap::new())),
        }
    }
}

impl<HDAddress> HDAddressesCache<HDAddress> {
    pub fn with_capacity(capacity: usize) -> Self {
        HDAddressesCache {
            cache: Arc::new(AsyncMutex::new(HashMap::with_capacity(capacity))),
        }
    }

    pub async fn lock(&self) -> AsyncMutexGuard<'_, HashMap<HDAddressId, HDAddress>> {
        self.cache.lock().await
    }
}

/// A generic HD account that can be used with any HD wallet.
#[derive(Clone, Debug)]
pub struct HDAccount<HDAddress, ExtendedPublicKey>
where
    HDAddress: HDAddressOps + Send,
    ExtendedPublicKey: ExtendedPublicKeyOps,
{
    pub account_id: u32,
    /// [Extended public key](https://learnmeabitcoin.com/technical/extended-keys) that corresponds to the derivation path:
    /// `m/purpose'/coin_type'/account'`.
    pub extended_pubkey: ExtendedPublicKey,
    /// [`HDWallet::derivation_path`] derived by [`HDAccount::account_id`].
    pub account_derivation_path: HDPathToAccount,
    /// The number of addresses that we know have been used by the user.
    /// This is used in order not to check the transaction history for each address,
    /// but to request the balance of addresses whose index is less than `address_number`.
    pub external_addresses_number: u32,
    /// The number of internal addresses that we know have been used by the user.
    /// This is used in order not to check the transaction history for each address,
    /// but to request the balance of addresses whose index is less than `address_number`.
    pub internal_addresses_number: u32,
    /// The cache of derived addresses.
    /// This is used at [`HDWalletCoinOps::derive_address`].
    pub derived_addresses: HDAddressesCache<HDAddress>,
}

impl<HDAddress, ExtendedPublicKey> HDAccountOps for HDAccount<HDAddress, ExtendedPublicKey>
where
    HDAddress: HDAddressOps + Clone + Send,
    ExtendedPublicKey: ExtendedPublicKeyOps,
{
    type HDAddress = HDAddress;
    type ExtendedPublicKey = ExtendedPublicKey;

    fn new(
        account_id: u32,
        extended_pubkey: Self::ExtendedPublicKey,
        account_derivation_path: HDPathToAccount,
    ) -> Self {
        HDAccount {
            account_id,
            extended_pubkey,
            account_derivation_path,
            external_addresses_number: 0,
            internal_addresses_number: 0,
            derived_addresses: HDAddressesCache::default(),
        }
    }

    fn address_limit(&self) -> u32 {
        DEFAULT_ADDRESS_LIMIT
    }

    fn known_addresses_number(&self, chain: Bip44Chain) -> MmResult<u32, InvalidBip44ChainError> {
        match chain {
            Bip44Chain::External => Ok(self.external_addresses_number),
            Bip44Chain::Internal => Ok(self.internal_addresses_number),
        }
    }

    fn set_known_addresses_number(&mut self, chain: Bip44Chain, num: u32) {
        match chain {
            Bip44Chain::External => {
                self.external_addresses_number = num;
            },
            Bip44Chain::Internal => {
                self.internal_addresses_number = num;
            },
        }
    }

    fn account_derivation_path(&self) -> DerivationPath {
        self.account_derivation_path.to_derivation_path()
    }

    fn account_id(&self) -> u32 {
        self.account_id
    }

    fn is_address_activated(&self, chain: Bip44Chain, address_id: u32) -> MmResult<bool, InvalidBip44ChainError> {
        let is_activated = address_id < self.known_addresses_number(chain)?;
        Ok(is_activated)
    }

    fn derived_addresses(&self) -> &HDAddressesCache<Self::HDAddress> {
        &self.derived_addresses
    }

    fn extended_pubkey(&self) -> &Self::ExtendedPublicKey {
        &self.extended_pubkey
    }
}

impl<HDAddress, ExtendedPublicKey> HDAccountStorageOps for HDAccount<HDAddress, ExtendedPublicKey>
where
    HDAddress: HDAddressOps + Send,
    ExtendedPublicKey: ExtendedPublicKeyOps,
    <ExtendedPublicKey as FromStr>::Err: Display,
{
    fn try_from_storage_item(
        wallet_der_path: &HDPathToCoin,
        account_info: &HDAccountStorageItem,
    ) -> HDWalletStorageResult<Self>
    where
        Self: Sized,
    {
        const ACCOUNT_CHILD_HARDENED: bool = true;

        let account_child = ChildNumber::new(account_info.account_id, ACCOUNT_CHILD_HARDENED)?;
        let account_derivation_path = wallet_der_path
            .derive(account_child)
            .map_to_mm(StandardHDPathError::from)
            .map_mm_err()?;
        let extended_pubkey = ExtendedPublicKey::from_str(&account_info.account_xpub)
            .map_err(|e| HDWalletStorageError::ErrorDeserializing(e.to_string()))?;
        let capacity =
            account_info.external_addresses_number + account_info.internal_addresses_number + DEFAULT_GAP_LIMIT;
        Ok(HDAccount {
            account_id: account_info.account_id,
            extended_pubkey,
            account_derivation_path,
            external_addresses_number: account_info.external_addresses_number,
            internal_addresses_number: account_info.internal_addresses_number,
            derived_addresses: HDAddressesCache::with_capacity(capacity as usize),
        })
    }

    fn to_storage_item(&self) -> HDAccountStorageItem {
        HDAccountStorageItem {
            account_id: self.account_id,
            account_xpub: self.extended_pubkey.to_string(bip32::Prefix::XPUB),
            external_addresses_number: self.external_addresses_number,
            internal_addresses_number: self.internal_addresses_number,
        }
    }
}

pub async fn load_hd_accounts_from_storage<HDAddress, ExtendedPublicKey>(
    hd_wallet_storage: &HDWalletCoinStorage,
    derivation_path: &HDPathToCoin,
) -> HDWalletStorageResult<HDAccountsMap<HDAccount<HDAddress, ExtendedPublicKey>>>
where
    HDAddress: HDAddressOps + Send,
    ExtendedPublicKey: ExtendedPublicKeyOps,
    <ExtendedPublicKey as FromStr>::Err: Display,
{
    let accounts = hd_wallet_storage.load_all_accounts().await?;
    let res: HDWalletStorageResult<HDAccountsMap<HDAccount<HDAddress, ExtendedPublicKey>>> = accounts
        .iter()
        .map(|account_info| {
            let account = HDAccount::try_from_storage_item(derivation_path, account_info)?;
            Ok((account.account_id, account))
        })
        .collect();
    match res {
        Ok(accounts) => Ok(accounts),
        Err(e) if e.get_inner().is_deserializing_err() => {
            warn!("Error loading HD accounts from the storage: '{}'. Clear accounts", e);
            hd_wallet_storage.clear_accounts().await?;
            Ok(HDAccountsMap::new())
        },
        Err(e) => Err(e),
    }
}

/// Represents a Hierarchical Deterministic (HD) wallet for UTXO coins.
/// This struct encapsulates all the necessary data for HD wallet operations
/// and is initialized whenever a utxo coin is activated in HD wallet mode.
#[derive(Debug)]
pub struct HDWallet<HDAccount>
where
    HDAccount: HDAccountOps + Clone + Send + Sync,
{
    /// A unique identifier for the HD wallet derived from the master public key.
    /// Specifically, it's the RIPEMD160 hash of the SHA256 hash of the master pubkey.
    /// This property aids in storing database items uniquely for each HD wallet.
    pub hd_wallet_rmd160: H160,
    /// Provides a means to access database operations for a specific user, HD wallet, and coin.
    /// The storage wrapper associates with the `coin` and `hd_wallet_rmd160` to provide unique storage access.
    pub hd_wallet_storage: HDWalletCoinStorage,
    /// Derivation path of the coin.
    /// This derivation path consists of `purpose` and `coin_type` only
    /// where the full `BIP44` address has the following structure:
    /// `m/purpose'/coin_type'/account'/change/address_index`.
    pub derivation_path: HDPathToCoin,
    /// Contains information about the accounts enabled for this HD wallet.
    pub accounts: HDAccountsMutex<HDAccount>,
    // Todo: This should be removed in the future to enable simultaneous swaps from multiple addresses
    /// The address that's specifically enabled for certain operations, e.g. swaps.
    pub enabled_address: HDPathAccountToAddressId,
    /// Defines the maximum number of consecutive addresses that can be generated
    /// without any associated transactions. If an address outside this limit
    /// receives transactions, they won't be identified.
    pub gap_limit: u32,
}

#[async_trait]
impl<HDAccount> HDWalletOps for HDWallet<HDAccount>
where
    HDAccount: HDAccountOps + Clone + Send + Sync,
{
    type HDAccount = HDAccount;

    fn coin_type(&self) -> u32 {
        self.derivation_path.coin_type()
    }

    fn derivation_path(&self) -> &HDPathToCoin {
        &self.derivation_path
    }

    fn gap_limit(&self) -> u32 {
        self.gap_limit
    }

    fn account_limit(&self) -> u32 {
        DEFAULT_ACCOUNT_LIMIT
    }

    fn default_receiver_chain(&self) -> Bip44Chain {
        DEFAULT_RECEIVER_CHAIN
    }

    fn get_accounts_mutex(&self) -> &HDAccountsMutex<Self::HDAccount> {
        &self.accounts
    }

    async fn get_account(&self, account_id: u32) -> Option<Self::HDAccount> {
        let accounts = self.get_accounts_mutex().lock().await;
        accounts.get(&account_id).cloned()
    }

    async fn get_account_mut(&self, account_id: u32) -> Option<HDAccountMut<'_, Self::HDAccount>> {
        let accounts = self.get_accounts_mutex().lock().await;
        if !accounts.contains_key(&account_id) {
            return None;
        }

        Some(AsyncMutexGuard::map(accounts, |accounts| {
            accounts
                .get_mut(&account_id)
                .expect("getting an element should never fail due to the checks above")
        }))
    }

    async fn get_accounts(&self) -> HDAccountsMap<Self::HDAccount> {
        self.get_accounts_mutex().lock().await.clone()
    }

    async fn get_accounts_mut(&self) -> HDAccountsMut<'_, Self::HDAccount> {
        self.get_accounts_mutex().lock().await
    }

    async fn remove_account_if_last(&self, account_id: u32) -> Option<Self::HDAccount> {
        let mut x = self.get_accounts_mutex().lock().await;
        // `BTreeMap::last_entry` is still unstable.
        let (last_account_id, _) = x.iter().last()?;
        if *last_account_id == account_id {
            x.remove(&account_id)
        } else {
            None
        }
    }

    async fn get_enabled_address(&self) -> Option<<Self::HDAccount as HDAccountOps>::HDAddress> {
        let enabled_address = self.enabled_address;
        let account = self.get_account(enabled_address.account_id).await?;
        let hd_address_id = HDAddressId {
            chain: enabled_address.chain,
            address_id: enabled_address.address_id,
        };
        let derived = account.derived_addresses().lock().await;

        let address = derived.get(&hd_address_id);
        address.cloned()
    }
}

/// Creates and registers a new HD account for a HDWallet.
///
/// # Parameters
/// - `coin`: A coin that implements [`ExtractExtendedPubkey`].
/// - `hd_wallet`: The specified HD wallet.
/// - `xpub_extractor`: Optional method for extracting the extended public key.
///   This is especially useful when dealing with hardware wallets. It can
///   allow for the extraction of the extended public key directly from the
///   wallet when needed.
/// - `account_id`: Optional account identifier.
///
/// # Returns
/// A result containing a mutable reference to the created `HDAccount` if successful.
pub async fn create_new_account<'a, Coin, XPubExtractor, HDWallet, HDAccount>(
    coin: &Coin,
    hd_wallet: &'a HDWallet,
    xpub_extractor: Option<XPubExtractor>,
    account_id: Option<u32>,
) -> MmResult<HDAccountMut<'a, HDWalletHDAccount<HDWallet>>, NewAccountCreationError>
where
    Coin: ExtractExtendedPubkey<ExtendedPublicKey = HDWalletExtendedPubkey<HDWallet>> + Sync,
    HDWallet: HDWalletOps<HDAccount = HDAccount> + HDWalletStorageOps + Sync,
    XPubExtractor: HDXPubExtractor + Send,
    HDAccount: 'a + HDAccountOps + HDAccountStorageOps,
{
    const INIT_ACCOUNT_ID: u32 = 0;
    let new_account_id = match account_id {
        Some(account_id) => account_id,
        None => {
            let accounts = hd_wallet.get_accounts_mut().await;
            let last_account_id = accounts.iter().last().map(|(account_id, _account)| *account_id);
            last_account_id.map_or(INIT_ACCOUNT_ID, |last_id| {
                (INIT_ACCOUNT_ID..=last_id)
                    .find(|id| !accounts.contains_key(id))
                    .unwrap_or(last_id + 1)
            })
        },
    };
    let max_accounts_number = hd_wallet.account_limit();
    if new_account_id >= max_accounts_number {
        return MmError::err(NewAccountCreationError::AccountLimitReached { max_accounts_number });
    }

    let account_child_hardened = true;
    let account_child = ChildNumber::new(new_account_id, account_child_hardened)
        .map_to_mm(|e| NewAccountCreationError::Internal(e.to_string()))?;

    let account_derivation_path: HDPathToAccount = hd_wallet.derivation_path().derive(account_child)?;
    let account_pubkey = coin
        .extract_extended_pubkey(xpub_extractor, account_derivation_path.to_derivation_path())
        .await
        .map_mm_err()?;

    let new_account = HDAccount::new(new_account_id, account_pubkey, account_derivation_path);

    let accounts = hd_wallet.get_accounts_mut().await;
    if accounts.contains_key(&new_account_id) {
        let error =
            format!("Account '{new_account_id}' has been activated while we proceed the 'create_new_account' function");
        return MmError::err(NewAccountCreationError::Internal(error));
    }

    hd_wallet
        .upload_new_account(new_account.to_storage_item())
        .await
        .map_mm_err()?;

    Ok(AsyncMutexGuard::map(accounts, |accounts| {
        accounts
            .entry(new_account_id)
            // the `entry` method should return [`Entry::Vacant`] due to the checks above
            .or_insert(new_account)
    }))
}

#[async_trait]
impl<HDAccount> HDWalletStorageOps for HDWallet<HDAccount>
where
    HDAccount: HDAccountOps + HDAccountStorageOps + Clone + Send + Sync,
{
    fn hd_wallet_storage(&self) -> &HDWalletCoinStorage {
        &self.hd_wallet_storage
    }
}

/// Unique identifier for an HD wallet address within the whole wallet context.
#[derive(Copy, Clone, Debug, Deserialize, Serialize)]
pub struct HDPathAccountToAddressId {
    pub account_id: u32,
    pub chain: Bip44Chain,
    pub address_id: u32,
}

impl Default for HDPathAccountToAddressId {
    fn default() -> Self {
        HDPathAccountToAddressId {
            account_id: 0,
            chain: Bip44Chain::External,
            address_id: 0,
        }
    }
}

impl From<StandardHDPath> for HDPathAccountToAddressId {
    fn from(der_path: StandardHDPath) -> Self {
        HDPathAccountToAddressId {
            account_id: der_path.account_id(),
            chain: der_path.chain(),
            address_id: der_path.address_id(),
        }
    }
}

impl HDPathAccountToAddressId {
    pub fn to_derivation_path(&self, path_to_coin: &HDPathToCoin) -> Result<DerivationPath, MmError<Bip32Error>> {
        let mut account_der_path = path_to_coin.to_derivation_path();
        account_der_path.push(ChildNumber::new(self.account_id, true)?);
        account_der_path.push(self.chain.to_child_number());
        account_der_path.push(ChildNumber::new(self.address_id, false)?);

        Ok(account_der_path)
    }
}
/// Represents how a hierarchical deterministic (HD) address is selected.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(untagged)]
pub enum HDAddressSelector {
    /// Specifies the HD address using its structured account, chain, and address ID.
    AddressId(HDPathAccountToAddressId),
    /// Specifies the HD address directly using a BIP-44,84 and other compliant derivation path.
    ///
    /// IMPORTANT: Don't use `Bip44DerivationPath` or `RpcDerivationPath` because if there is an error in the path,
    /// `serde::Deserialize` returns "data did not match any variant of untagged enum HDAddressSelector".
    /// It's better to show the user an informative error.
    DerivationPath { derivation_path: String },
}

impl HDAddressSelector {
    pub fn to_address_path(&self, expected_coin_type: u32) -> MmResult<HDPathAccountToAddressId, StandardHDPathError> {
        match self {
            HDAddressSelector::AddressId(address_id) => Ok(*address_id),
            HDAddressSelector::DerivationPath { derivation_path } => {
                let derivation_path = StandardHDPath::from_str(derivation_path).map_to_mm(StandardHDPathError::from)?;
                let coin_type = derivation_path.coin_type();

                if coin_type != expected_coin_type {
                    return MmError::err(StandardHDPathError::InvalidCoinType {
                        expected: expected_coin_type,
                        found: coin_type,
                    });
                }

                Ok(HDPathAccountToAddressId::from(derivation_path))
            },
        }
    }

    pub fn valid_derivation_path(self, path_to_coin: &HDPathToCoin) -> MmResult<DerivationPath, StandardHDPathError> {
        match self {
            HDAddressSelector::AddressId(id) => id
                .to_derivation_path(path_to_coin)
                .mm_err(StandardHDPathError::Bip32Error),
            HDAddressSelector::DerivationPath { derivation_path } => {
                let standard_hd_path = StandardHDPath::from_str(&derivation_path)
                    .map_to_mm(|_| StandardHDPathError::Bip32Error(Bip32Error::Decode))?;
                let rpc_path_to_coin = standard_hd_path.path_to_coin();

                // validate rpc path_to_coin against activated coin.
                if &rpc_path_to_coin != path_to_coin {
                    return MmError::err(StandardHDPathError::InvalidPathToCoin {
                        expected: rpc_path_to_coin.to_string(),
                        found: path_to_coin.to_string(),
                    });
                };

                Ok(standard_hd_path.to_derivation_path())
            },
        }
    }
}

pub(crate) mod inner_impl {
    use super::*;
    use coin_ops::HDWalletCoinOps;

    pub struct NewAddress<HDAddress>
    where
        HDAddress: HDAddressOps,
    {
        pub hd_address: HDAddress,
        pub new_known_addresses_number: u32,
    }

    /// Generates a new address without updating a corresponding number of used `hd_account` addresses.
    pub async fn generate_new_address_immutable<Coin>(
        coin: &Coin,
        hd_account: &HDCoinHDAccount<Coin>,
        chain: Bip44Chain,
    ) -> MmResult<NewAddress<HDCoinHDAddress<Coin>>, NewAddressDerivingError>
    where
        Coin: HDWalletCoinOps + ?Sized + Sync,
    {
        let known_addresses_number = hd_account.known_addresses_number(chain).map_mm_err()?;
        // Address IDs start from 0, so the `known_addresses_number = last_known_address_id + 1`.
        let new_address_id = known_addresses_number;
        let max_addresses_number = hd_account.address_limit();
        if new_address_id >= max_addresses_number {
            return MmError::err(NewAddressDerivingError::AddressLimitReached { max_addresses_number });
        }
        let address = coin
            .derive_address(hd_account, chain, new_address_id)
            .await
            .map_mm_err()?;
        Ok(NewAddress {
            hd_address: address,
            new_known_addresses_number: known_addresses_number + 1,
        })
    }
}
