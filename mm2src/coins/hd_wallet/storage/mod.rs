use async_trait::async_trait;
use crypto::{Bip32Error, CryptoCtx, CryptoCtxError, HDPathToCoin, StandardHDPathError, XPub};
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
#[cfg(test)]
use mocktopus::macros::*;
use primitives::hash::H160;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::Formatter;
use std::ops::Deref;

#[cfg(not(target_arch = "wasm32"))]
mod sqlite_storage;
#[cfg(target_arch = "wasm32")]
mod wasm_storage;

#[cfg(any(test, target_arch = "wasm32"))]
mod mock_storage;
#[cfg(any(test, target_arch = "wasm32"))]
pub(crate) use mock_storage::HDWalletMockStorage;

cfg_wasm32! {
    use wasm_storage::HDWalletIndexedDbStorage as HDWalletStorageInstance;
    pub(crate) use wasm_storage::HDWalletDb;
}

cfg_native! {
    use sqlite_storage::HDWalletSqliteStorage as HDWalletStorageInstance;
}

pub(crate) type HDWalletStorageResult<T> = MmResult<T, HDWalletStorageError>;
type HDWalletStorageBoxed = Box<dyn HDWalletStorageInternalOps + Send + Sync>;

#[derive(Debug, Display)]
pub enum HDWalletStorageError {
    #[display(fmt = "HD wallet not allowed")]
    HDWalletUnavailable,
    #[display(fmt = "HD account '{wallet_id:?}':{account_id} not found")]
    HDAccountNotFound { wallet_id: HDWalletId, account_id: u32 },
    #[display(fmt = "Error saving changes in HD wallet storage: {_0}")]
    ErrorSaving(String),
    #[display(fmt = "Error loading from HD wallet storage: {_0}")]
    ErrorLoading(String),
    #[display(fmt = "Error deserializing a swap: {_0}")]
    ErrorDeserializing(String),
    #[display(fmt = "Error serializing a swap: {_0}")]
    ErrorSerializing(String),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl From<Bip32Error> for HDWalletStorageError {
    fn from(e: Bip32Error) -> Self {
        HDWalletStorageError::ErrorDeserializing(e.to_string())
    }
}

impl From<CryptoCtxError> for HDWalletStorageError {
    fn from(e: CryptoCtxError) -> Self {
        HDWalletStorageError::Internal(e.to_string())
    }
}

impl From<StandardHDPathError> for HDWalletStorageError {
    fn from(e: StandardHDPathError) -> Self {
        HDWalletStorageError::ErrorDeserializing(e.to_string())
    }
}

impl HDWalletStorageError {
    pub fn is_deserializing_err(&self) -> bool {
        matches!(self, HDWalletStorageError::ErrorDeserializing(_))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HDWalletId {
    coin: String,
    /// RIPEMD160(SHA256(x)) where x is a pubkey extracted from a Hardware Wallet device or passphrase.
    /// This property allows us to store DB items that are unique to each Hardware Wallet device.
    hd_wallet_rmd160: String,
}

impl HDWalletId {
    pub fn new(coin: String, hd_wallet_rmd160: &H160) -> HDWalletId {
        HDWalletId {
            coin,
            hd_wallet_rmd160: display_rmd160(hd_wallet_rmd160),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct HDAccountStorageItem {
    pub account_id: u32,
    pub account_xpub: XPub,
    /// The number of addresses that we know have been used by the user.
    pub external_addresses_number: u32,
    pub internal_addresses_number: u32,
}

#[async_trait]
#[cfg_attr(test, mockable)]
pub(crate) trait HDWalletStorageInternalOps {
    async fn init(ctx: &MmArc) -> HDWalletStorageResult<Self>
    where
        Self: Sized;

    async fn load_accounts(&self, wallet_id: HDWalletId) -> HDWalletStorageResult<Vec<HDAccountStorageItem>>;

    async fn load_account(
        &self,
        wallet_id: HDWalletId,
        account_id: u32,
    ) -> HDWalletStorageResult<Option<HDAccountStorageItem>>;

    async fn update_external_addresses_number(
        &self,
        wallet_id: HDWalletId,
        account_id: u32,
        new_external_addresses_number: u32,
    ) -> HDWalletStorageResult<()>;

    async fn update_internal_addresses_number(
        &self,
        wallet_id: HDWalletId,
        account_id: u32,
        new_internal_addresses_number: u32,
    ) -> HDWalletStorageResult<()>;

    async fn upload_new_account(
        &self,
        wallet_id: HDWalletId,
        account: HDAccountStorageItem,
    ) -> HDWalletStorageResult<()>;

    async fn clear_accounts(&self, wallet_id: HDWalletId) -> HDWalletStorageResult<()>;
}

/// `HDWalletStorageOps` is a trait that allows us to interact with the storage implementation of the HD wallet.
#[async_trait]
pub trait HDWalletStorageOps {
    /// Getter for the HD wallet storage.
    fn hd_wallet_storage(&self) -> &HDWalletCoinStorage;

    /// Loads all accounts from the HD wallet storage.
    async fn load_all_accounts(&self) -> HDWalletStorageResult<Vec<HDAccountStorageItem>> {
        let storage = self.hd_wallet_storage();
        storage.load_all_accounts().await
    }

    /// Loads a specific account from the HD wallet storage.
    async fn load_account(&self, account_id: u32) -> HDWalletStorageResult<Option<HDAccountStorageItem>> {
        let storage = self.hd_wallet_storage();
        storage.load_account(account_id).await
    }

    /// Updates the number of external addresses for a specific account.
    async fn update_external_addresses_number(
        &self,
        account_id: u32,
        new_external_addresses_number: u32,
    ) -> HDWalletStorageResult<()> {
        let storage = self.hd_wallet_storage();
        storage
            .update_external_addresses_number(account_id, new_external_addresses_number)
            .await
    }

    /// Updates the number of internal addresses for a specific account.
    async fn update_internal_addresses_number(
        &self,
        account_id: u32,
        new_internal_addresses_number: u32,
    ) -> HDWalletStorageResult<()> {
        let storage = self.hd_wallet_storage();
        storage
            .update_internal_addresses_number(account_id, new_internal_addresses_number)
            .await
    }

    /// Saves new account details to the HD wallet storage.
    async fn upload_new_account(&self, account_info: HDAccountStorageItem) -> HDWalletStorageResult<()> {
        let storage = self.hd_wallet_storage();
        storage.upload_new_account(account_info).await
    }

    /// Deletes all accounts from the HD wallet storage.
    async fn clear_accounts(&self) -> HDWalletStorageResult<()> {
        let storage = self.hd_wallet_storage();
        storage.clear_accounts().await
    }
}

/// `HDAccountStorageOps` is a trait that allows us to convert `HDAccountStorageItem` to whatever implements this trait and vice versa.
pub trait HDAccountStorageOps {
    /// Converts `HDAccountStorageItem` to whatever implements this trait.
    fn try_from_storage_item(
        wallet_der_path: &HDPathToCoin,
        account_info: &HDAccountStorageItem,
    ) -> HDWalletStorageResult<Self>
    where
        Self: Sized;

    /// Converts whatever implements this trait to `HDAccountStorageItem`.
    fn to_storage_item(&self) -> HDAccountStorageItem;
}

/// The wrapper over the [`HDWalletStorage::inner`] database implementation.
/// It's associated with a specific mm2 user, HD wallet and coin.
pub struct HDWalletCoinStorage {
    coin: String,
    /// RIPEMD160(SHA256(x)) where x is a pubkey extracted from a Hardware Wallet device or passphrase.
    /// This property allows us to store DB items that are unique to each Hardware Wallet device or HD wallet.
    hd_wallet_rmd160: H160,
    inner: HDWalletStorageBoxed,
}

impl fmt::Debug for HDWalletCoinStorage {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("HDWalletCoinStorage")
            .field("coin", &self.coin)
            .field("hd_wallet_rmd160", &self.hd_wallet_rmd160)
            .finish()
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
impl Default for HDWalletCoinStorage {
    fn default() -> Self {
        HDWalletCoinStorage {
            coin: String::default(),
            hd_wallet_rmd160: H160::default(),
            inner: Box::new(HDWalletMockStorage),
        }
    }
}

impl HDWalletCoinStorage {
    pub async fn init(ctx: &MmArc, coin: String) -> HDWalletStorageResult<HDWalletCoinStorage> {
        let inner = Box::new(HDWalletStorageInstance::init(ctx).await?);
        let crypto_ctx = CryptoCtx::from_ctx(ctx).map_mm_err()?;
        let hd_wallet_rmd160 = crypto_ctx
            .hw_wallet_rmd160()
            .or_mm_err(|| HDWalletStorageError::HDWalletUnavailable)?;
        Ok(HDWalletCoinStorage {
            coin,
            hd_wallet_rmd160,
            inner,
        })
    }

    pub async fn init_with_rmd160(
        ctx: &MmArc,
        coin: String,
        hd_wallet_rmd160: H160,
    ) -> HDWalletStorageResult<HDWalletCoinStorage> {
        let inner = Box::new(HDWalletStorageInstance::init(ctx).await?);
        Ok(HDWalletCoinStorage {
            coin,
            hd_wallet_rmd160,
            inner,
        })
    }

    pub fn wallet_id(&self) -> HDWalletId {
        HDWalletId::new(self.coin.clone(), &self.hd_wallet_rmd160)
    }

    pub async fn load_all_accounts(&self) -> HDWalletStorageResult<Vec<HDAccountStorageItem>> {
        let wallet_id = self.wallet_id();
        self.inner.load_accounts(wallet_id).await
    }

    async fn load_account(&self, account_id: u32) -> HDWalletStorageResult<Option<HDAccountStorageItem>> {
        let wallet_id = self.wallet_id();
        self.inner.load_account(wallet_id, account_id).await
    }

    async fn update_external_addresses_number(
        &self,
        account_id: u32,
        new_external_addresses_number: u32,
    ) -> HDWalletStorageResult<()> {
        let wallet_id = self.wallet_id();
        self.inner
            .update_external_addresses_number(wallet_id, account_id, new_external_addresses_number)
            .await
    }

    async fn update_internal_addresses_number(
        &self,
        account_id: u32,
        new_internal_addresses_number: u32,
    ) -> HDWalletStorageResult<()> {
        let wallet_id = self.wallet_id();
        self.inner
            .update_internal_addresses_number(wallet_id, account_id, new_internal_addresses_number)
            .await
    }

    async fn upload_new_account(&self, account_info: HDAccountStorageItem) -> HDWalletStorageResult<()> {
        let wallet_id = self.wallet_id();
        self.inner.upload_new_account(wallet_id, account_info).await
    }

    pub async fn clear_accounts(&self) -> HDWalletStorageResult<()> {
        let wallet_id = self.wallet_id();
        self.inner.clear_accounts(wallet_id).await
    }
}

fn display_rmd160(rmd160: &H160) -> String {
    hex::encode(rmd160.deref())
}

#[cfg(any(test, target_arch = "wasm32"))]
mod tests {
    use super::*;
    use itertools::Itertools;
    use mm2_test_helpers::for_tests::mm_ctx_with_custom_db;
    use primitives::hash::H160;

    cfg_wasm32! {
        use wasm_storage::get_all_storage_items;
        use wasm_bindgen_test::*;

        wasm_bindgen_test_configure!(run_in_browser);
    }

    cfg_native! {
        use sqlite_storage::get_all_storage_items;
        use common::block_on;
    }

    async fn test_unique_wallets_impl() {
        let rick_device0_account0 = HDAccountStorageItem {
            account_id: 0,
            account_xpub: "xpub6DEHSksajpRPM59RPw7Eg6PKdU7E2ehxJWtYdrfQ6JFmMGBsrR6jA78ANCLgzKYm4s5UqQ4ydLEYPbh3TRVvn5oAZVtWfi4qJLMntpZ8uGJ".to_owned(),
            external_addresses_number: 1,
            internal_addresses_number: 2,
        };
        let rick_device0_account1 = HDAccountStorageItem {
            account_id: 1,
            account_xpub: "xpub6DEHSksajpRPQq2FdGT6JoieiQZUpTZ3WZn8fcuLJhFVmtCpXbuXxp5aPzaokwcLV2V9LE55Dwt8JYkpuMv7jXKwmyD28WbHYjBH2zhbW2p".to_owned(),
            external_addresses_number: 1,
            internal_addresses_number: 2,
        };
        let rick_device1_account0 = HDAccountStorageItem {
            account_id: 0,
            account_xpub: "xpub6EuV33a2DXxAhoJTRTnr8qnysu81AA4YHpLY6o8NiGkEJ8KADJ35T64eJsStWsmRf1xXkEANVjXFXnaUKbRtFwuSPCLfDdZwYNZToh4LBCd".to_owned(),
            external_addresses_number: 3,
            internal_addresses_number: 4,
        };
        let morty_device0_account0 = HDAccountStorageItem {
            account_id: 0,
            account_xpub: "xpub6AHA9hZDN11k2ijHMeS5QqHx2KP9aMBRhTDqANMnwVtdyw2TDYRmF8PjpvwUFcL1Et8Hj59S3gTSMcUQ5gAqTz3Wd8EsMTmF3DChhqPQBnU".to_owned(),
            external_addresses_number: 7,
            internal_addresses_number: 8,
        };

        let ctx = mm_ctx_with_custom_db();
        let device0_rmd160 = H160::from("0000000000000000000000000000000000000020");
        let device1_rmd160 = H160::from("0000000000000000000000000000000000000030");

        let rick_device0_db = HDWalletCoinStorage::init_with_rmd160(&ctx, "RICK".to_owned(), device0_rmd160)
            .await
            .expect("!HDWalletCoinStorage::new");
        let rick_device1_db = HDWalletCoinStorage::init_with_rmd160(&ctx, "RICK".to_owned(), device1_rmd160)
            .await
            .expect("!HDWalletCoinStorage::new");
        let morty_device0_db = HDWalletCoinStorage::init_with_rmd160(&ctx, "MORTY".to_owned(), device0_rmd160)
            .await
            .expect("!HDWalletCoinStorage::new");

        rick_device0_db
            .upload_new_account(rick_device0_account0.clone())
            .await
            .expect("!HDWalletCoinStorage::upload_new_account: RICK device=0 account=0");
        rick_device0_db
            .upload_new_account(rick_device0_account1.clone())
            .await
            .expect("!HDWalletCoinStorage::upload_new_account: RICK device=0 account=1");
        rick_device1_db
            .upload_new_account(rick_device1_account0.clone())
            .await
            .expect("!HDWalletCoinStorage::upload_new_account: RICK device=1 account=0");
        morty_device0_db
            .upload_new_account(morty_device0_account0.clone())
            .await
            .expect("!HDWalletCoinStorage::upload_new_account: MORTY device=0 account=0");

        // All accounts must be in the only one database.
        // Rows in the database must differ by only `coin`, `hd_wallet_rmd160` and `account_id` values.
        let all_accounts: Vec<_> = get_all_storage_items(&ctx)
            .await
            .into_iter()
            .sorted_by(|x, y| x.external_addresses_number.cmp(&y.external_addresses_number))
            .collect();
        assert_eq!(
            all_accounts,
            vec![
                rick_device0_account0.clone(),
                rick_device0_account1.clone(),
                rick_device1_account0.clone(),
                morty_device0_account0.clone()
            ]
        );

        let mut actual = rick_device0_db
            .load_all_accounts()
            .await
            .expect("HDWalletCoinStorage::load_all_accounts: RICK device=0");
        actual.sort_by(|x, y| x.account_id.cmp(&y.account_id));
        assert_eq!(actual, vec![rick_device0_account0, rick_device0_account1]);

        let actual = rick_device1_db
            .load_all_accounts()
            .await
            .expect("HDWalletCoinStorage::load_all_accounts: RICK device=1");
        assert_eq!(actual, vec![rick_device1_account0]);

        let actual = morty_device0_db
            .load_all_accounts()
            .await
            .expect("HDWalletCoinStorage::load_all_accounts: MORTY device=0");
        assert_eq!(actual, vec![morty_device0_account0]);
    }

    async fn test_delete_accounts_impl() {
        let wallet0_account0 = HDAccountStorageItem {
            account_id: 0,
            account_xpub: "xpub6DEHSksajpRPM59RPw7Eg6PKdU7E2ehxJWtYdrfQ6JFmMGBsrR6jA78ANCLgzKYm4s5UqQ4ydLEYPbh3TRVvn5oAZVtWfi4qJLMntpZ8uGJ".to_owned(),
            external_addresses_number: 1,
            internal_addresses_number: 2,
        };
        let wallet0_account1 = HDAccountStorageItem {
            account_id: 1,
            account_xpub: "xpub6DEHSksajpRPQq2FdGT6JoieiQZUpTZ3WZn8fcuLJhFVmtCpXbuXxp5aPzaokwcLV2V9LE55Dwt8JYkpuMv7jXKwmyD28WbHYjBH2zhbW2p".to_owned(),
            external_addresses_number: 1,
            internal_addresses_number: 2,
        };
        let wallet1_account0 = HDAccountStorageItem {
            account_id: 0,
            account_xpub: "xpub6EuV33a2DXxAhoJTRTnr8qnysu81AA4YHpLY6o8NiGkEJ8KADJ35T64eJsStWsmRf1xXkEANVjXFXnaUKbRtFwuSPCLfDdZwYNZToh4LBCd".to_owned(),
            external_addresses_number: 3,
            internal_addresses_number: 4,
        };
        let wallet2_account0 = HDAccountStorageItem {
            account_id: 0,
            account_xpub: "xpub6CUGRUonZSQ4TWtTMmzXdrXDtypWKiKrhko4egpiMZbpiaQL2jkwSB1icqYh2cfDfVxdx4df189oLKnC5fSwqPfgyP3hooxujYzAu3fDVmz".to_owned(),
            external_addresses_number: 5,
            internal_addresses_number: 6,
        };

        let ctx = mm_ctx_with_custom_db();
        let device0_rmd160 = H160::from("0000000000000000000000000000000000000010");
        let device1_rmd160 = H160::from("0000000000000000000000000000000000000020");
        let device2_rmd160 = H160::from("0000000000000000000000000000000000000030");

        let wallet0_db = HDWalletCoinStorage::init_with_rmd160(&ctx, "RICK".to_owned(), device0_rmd160)
            .await
            .expect("!HDWalletCoinStorage::new");
        let wallet1_db = HDWalletCoinStorage::init_with_rmd160(&ctx, "RICK".to_owned(), device1_rmd160)
            .await
            .expect("!HDWalletCoinStorage::new");
        let wallet2_db = HDWalletCoinStorage::init_with_rmd160(&ctx, "RICK".to_owned(), device2_rmd160)
            .await
            .expect("!HDWalletCoinStorage::new");

        wallet0_db
            .upload_new_account(wallet0_account0.clone())
            .await
            .expect("!HDWalletCoinStorage::upload_new_account: RICK wallet=0 account=0");
        wallet0_db
            .upload_new_account(wallet0_account1.clone())
            .await
            .expect("!HDWalletCoinStorage::upload_new_account: RICK wallet=0 account=1");
        wallet1_db
            .upload_new_account(wallet1_account0.clone())
            .await
            .expect("!HDWalletCoinStorage::upload_new_account: RICK wallet=1 account=0");
        wallet2_db
            .upload_new_account(wallet2_account0.clone())
            .await
            .expect("!HDWalletCoinStorage::upload_new_account: RICK wallet=2 account=0");

        wallet0_db
            .clear_accounts()
            .await
            .expect("HDWalletCoinStorage::clear_accounts: RICK wallet=0");

        // All accounts must be in the only one database.
        // Rows in the database must differ by only `coin`, `hd_wallet_rmd160` and `account_id` values.
        let all_accounts: Vec<_> = get_all_storage_items(&ctx)
            .await
            .into_iter()
            .sorted_by(|x, y| x.external_addresses_number.cmp(&y.external_addresses_number))
            .collect();
        assert_eq!(all_accounts, vec![wallet1_account0, wallet2_account0]);
    }

    async fn test_update_account_impl() {
        let mut account0 = HDAccountStorageItem {
            account_id: 0,
            account_xpub: "xpub6DEHSksajpRPM59RPw7Eg6PKdU7E2ehxJWtYdrfQ6JFmMGBsrR6jA78ANCLgzKYm4s5UqQ4ydLEYPbh3TRVvn5oAZVtWfi4qJLMntpZ8uGJ".to_owned(),
            external_addresses_number: 1,
            internal_addresses_number: 2,
        };
        let mut account1 = HDAccountStorageItem {
            account_id: 1,
            account_xpub: "xpub6DEHSksajpRPQq2FdGT6JoieiQZUpTZ3WZn8fcuLJhFVmtCpXbuXxp5aPzaokwcLV2V9LE55Dwt8JYkpuMv7jXKwmyD28WbHYjBH2zhbW2p".to_owned(),
            external_addresses_number: 3,
            internal_addresses_number: 4,
        };

        let ctx = mm_ctx_with_custom_db();
        let device_rmd160 = H160::from("0000000000000000000000000000000000000010");

        let db = HDWalletCoinStorage::init_with_rmd160(&ctx, "RICK".to_owned(), device_rmd160)
            .await
            .expect("!HDWalletCoinStorage::new");

        db.upload_new_account(account0.clone())
            .await
            .expect("!HDWalletCoinStorage::upload_new_account: RICK wallet=0 account=0");
        db.upload_new_account(account1.clone())
            .await
            .expect("!HDWalletCoinStorage::upload_new_account: RICK wallet=0 account=1");

        db.update_internal_addresses_number(0, 5)
            .await
            .expect("!HDWalletCoinStorage::update_internal_addresses_number");
        db.update_external_addresses_number(1, 10)
            .await
            .expect("!HDWalletCoinStorage::update_external_addresses_number");

        let actual: Vec<_> = db
            .load_all_accounts()
            .await
            .expect("!HDWalletCoinStorage::load_all_accounts")
            .into_iter()
            .sorted_by(|x, y| x.external_addresses_number.cmp(&y.external_addresses_number))
            .collect();

        account0.internal_addresses_number = 5;
        account1.external_addresses_number = 10;
        assert_eq!(actual, vec![account0, account1]);
    }

    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen_test]
    async fn test_unique_wallets() {
        test_unique_wallets_impl().await
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn test_unique_wallets() {
        block_on(test_unique_wallets_impl())
    }

    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen_test]
    async fn test_delete_accounts() {
        test_delete_accounts_impl().await
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn test_delete_accounts() {
        block_on(test_delete_accounts_impl())
    }

    #[cfg(target_arch = "wasm32")]
    #[wasm_bindgen_test]
    async fn test_update_account() {
        test_update_account_impl().await
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn test_update_account() {
        block_on(test_update_account_impl())
    }
}
