use crate::hd_wallet::{
    HDAccountStorageItem, HDWalletId, HDWalletStorageError, HDWalletStorageInternalOps, HDWalletStorageResult,
};
use crate::CoinsContext;
use async_trait::async_trait;
use crypto::XPub;
use mm2_core::mm_ctx::MmArc;
use mm2_db::indexed_db::cursor_prelude::*;
use mm2_db::indexed_db::{
    DbIdentifier, DbInstance, DbLocked, DbTable, DbTransactionError, DbUpgrader, IndexedDb, IndexedDbBuilder,
    InitDbError, InitDbResult, ItemId, MultiIndex, OnUpgradeResult, SharedDb, TableSignature, WeakDb,
};
use mm2_err_handle::prelude::*;

const DB_VERSION: u32 = 1;
/// An index of the `HDAccountTable` table that consists of the following properties:
/// * coin - coin ticker
/// * hd_wallet_rmd160 - RIPEMD160(SHA256(x)) where x is a pubkey extracted from a Hardware Wallet device or passphrase.
const WALLET_ID_INDEX: &str = "wallet_id";
/// A **unique** index of the `HDAccountTable` table that consists of the following properties:
/// * coin - coin ticker
/// * hd_wallet_rmd160 - RIPEMD160(SHA256(x)) where x is a pubkey extracted from a Hardware Wallet device or passphrase.
/// * account_id - HD account id
const WALLET_ACCOUNT_ID_INDEX: &str = "wallet_account_id";

type HDWalletDbLocked<'a> = DbLocked<'a, HDWalletDb>;

impl From<DbTransactionError> for HDWalletStorageError {
    fn from(e: DbTransactionError) -> Self {
        let desc = e.to_string();
        match e {
            DbTransactionError::NoSuchTable { .. }
            | DbTransactionError::ErrorCreatingTransaction(_)
            | DbTransactionError::ErrorOpeningTable { .. }
            | DbTransactionError::ErrorSerializingIndex { .. }
            | DbTransactionError::MultipleItemsByUniqueIndex { .. }
            | DbTransactionError::NoSuchIndex { .. }
            | DbTransactionError::InvalidIndex { .. }
            | DbTransactionError::UnexpectedState(_)
            | DbTransactionError::TransactionAborted => HDWalletStorageError::Internal(desc),
            DbTransactionError::ErrorDeserializingItem(_) => HDWalletStorageError::ErrorDeserializing(desc),
            DbTransactionError::ErrorSerializingItem(_) => HDWalletStorageError::ErrorSerializing(desc),
            DbTransactionError::ErrorGettingItems(_) | DbTransactionError::ErrorCountingItems(_) => {
                HDWalletStorageError::ErrorLoading(desc)
            },
            DbTransactionError::ErrorUploadingItem(_) | DbTransactionError::ErrorDeletingItems(_) => {
                HDWalletStorageError::ErrorSaving(desc)
            },
        }
    }
}

impl From<CursorError> for HDWalletStorageError {
    fn from(e: CursorError) -> Self {
        let stringified_error = e.to_string();
        match e {
            // We don't expect that the `String` and `u32` types serialization to fail.
            CursorError::ErrorSerializingIndexFieldValue {..}
            // We don't expect that the `String` and `u32` types deserialization to fail.
            | CursorError::ErrorDeserializingIndexValue {..}
            | CursorError::ErrorOpeningCursor {..}
            | CursorError::AdvanceError {..}
            | CursorError::InvalidKeyRange {..}
            | CursorError::TypeMismatch {..}
            | CursorError::IncorrectNumberOfKeysPerIndex {..}
            | CursorError::UnexpectedState(..)
            | CursorError::IncorrectUsage {..} => HDWalletStorageError::Internal(stringified_error),
            CursorError::ErrorDeserializingItem {..} => HDWalletStorageError::ErrorDeserializing(stringified_error),
        }
    }
}

impl From<InitDbError> for HDWalletStorageError {
    fn from(e: InitDbError) -> Self {
        HDWalletStorageError::Internal(e.to_string())
    }
}

/// The table has the following individually non-unique indexes: `coin`, `hd_wallet_rmd160`, `account_id`,
/// one non-unique multi-index `wallet_id` that consists of `coin`, `hd_wallet_rmd160`,
/// and one unique multi-index `wallet_account_id` that consists of these four indexes in a row.
/// See [`HDAccountTable::on_update_needed`].
#[derive(Deserialize, Serialize)]
struct HDAccountTable {
    /// [`HDWalletId::coin`].
    /// Non-unique index that is used to fetch/remove items from the storage.
    coin: String,
    /// [`HDWalletId::hd_wallet_rmd160`].
    /// Non-unique index that is used to fetch/remove items from the storage.
    hd_wallet_rmd160: String,
    /// HD Account ID.
    /// Non-unique index that is used to fetch/remove items from the storage.
    account_id: u32,
    account_xpub: XPub,
    /// The number of addresses that we know have been used by the user.
    external_addresses_number: u32,
    internal_addresses_number: u32,
}

impl TableSignature for HDAccountTable {
    const TABLE_NAME: &'static str = "hd_account";

    fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
        if let (0, 1) = (old_version, new_version) {
            let table = upgrader.create_table(Self::TABLE_NAME)?;
            table.create_multi_index(WALLET_ID_INDEX, &["coin", "hd_wallet_rmd160"], false)?;
            table.create_multi_index(
                WALLET_ACCOUNT_ID_INDEX,
                &["coin", "hd_wallet_rmd160", "account_id"],
                true,
            )?;
        }

        Ok(())
    }
}

impl HDAccountTable {
    fn new(wallet_id: HDWalletId, account_info: HDAccountStorageItem) -> HDAccountTable {
        HDAccountTable {
            coin: wallet_id.coin,
            hd_wallet_rmd160: wallet_id.hd_wallet_rmd160,
            account_id: account_info.account_id,
            account_xpub: account_info.account_xpub,
            external_addresses_number: account_info.external_addresses_number,
            internal_addresses_number: account_info.internal_addresses_number,
        }
    }
}

impl From<HDAccountTable> for HDAccountStorageItem {
    fn from(account: HDAccountTable) -> Self {
        HDAccountStorageItem {
            account_id: account.account_id,
            account_xpub: account.account_xpub,
            external_addresses_number: account.external_addresses_number,
            internal_addresses_number: account.internal_addresses_number,
        }
    }
}

pub(crate) struct HDWalletDb {
    pub(crate) inner: IndexedDb,
}

#[async_trait]
impl DbInstance for HDWalletDb {
    const DB_NAME: &'static str = "hd_wallet";

    async fn init(db_id: DbIdentifier) -> InitDbResult<Self> {
        let inner = IndexedDbBuilder::new(db_id)
            .with_version(DB_VERSION)
            .with_table::<HDAccountTable>()
            .build()
            .await?;
        Ok(HDWalletDb { inner })
    }
}

/// The wrapper over the [`CoinsContext::hd_wallet_db`] weak pointer.
pub(super) struct HDWalletIndexedDbStorage {
    db: WeakDb<HDWalletDb>,
}

#[async_trait]
impl HDWalletStorageInternalOps for HDWalletIndexedDbStorage {
    async fn init(ctx: &MmArc) -> HDWalletStorageResult<Self>
    where
        Self: Sized,
    {
        let coins_ctx = CoinsContext::from_ctx(ctx).map_to_mm(HDWalletStorageError::Internal)?;
        let db = SharedDb::downgrade(&coins_ctx.hd_wallet_db);
        Ok(HDWalletIndexedDbStorage { db })
    }

    async fn load_accounts(&self, wallet_id: HDWalletId) -> HDWalletStorageResult<Vec<HDAccountStorageItem>> {
        let shared_db = self.get_shared_db()?;
        let locked_db = Self::lock_db_mutex(&shared_db).await?;

        let transaction = locked_db.inner.transaction().await.map_mm_err()?;
        let table = transaction.table::<HDAccountTable>().await.map_mm_err()?;

        let index_keys = MultiIndex::new(WALLET_ID_INDEX)
            .with_value(wallet_id.coin)
            .map_mm_err()?
            .with_value(wallet_id.hd_wallet_rmd160)
            .map_mm_err()?;
        Ok(table
            .get_items_by_multi_index(index_keys)
            .await
            .map_mm_err()?
            .into_iter()
            .map(|(_item_id, item)| HDAccountStorageItem::from(item))
            .collect())
    }

    async fn load_account(
        &self,
        wallet_id: HDWalletId,
        account_id: u32,
    ) -> HDWalletStorageResult<Option<HDAccountStorageItem>> {
        let shared_db = self.get_shared_db()?;
        let locked_db = Self::lock_db_mutex(&shared_db).await?;

        let transaction = locked_db.inner.transaction().await.map_mm_err()?;
        let table = transaction.table::<HDAccountTable>().await.map_mm_err()?;

        let maybe_account = Self::find_account(&table, wallet_id, account_id).await?;
        match maybe_account {
            Some((_account_item_id, account_item)) => Ok(Some(HDAccountStorageItem::from(account_item))),
            None => Ok(None),
        }
    }

    async fn update_external_addresses_number(
        &self,
        wallet_id: HDWalletId,
        account_id: u32,
        new_external_addresses_number: u32,
    ) -> HDWalletStorageResult<()> {
        self.update_account(wallet_id, account_id, |account| {
            account.external_addresses_number = new_external_addresses_number;
        })
        .await
    }

    async fn update_internal_addresses_number(
        &self,
        wallet_id: HDWalletId,
        account_id: u32,
        new_internal_addresses_number: u32,
    ) -> HDWalletStorageResult<()> {
        self.update_account(wallet_id, account_id, |account| {
            account.internal_addresses_number = new_internal_addresses_number;
        })
        .await
    }

    async fn upload_new_account(
        &self,
        wallet_id: HDWalletId,
        account: HDAccountStorageItem,
    ) -> HDWalletStorageResult<()> {
        let shared_db = self.get_shared_db()?;
        let locked_db = Self::lock_db_mutex(&shared_db).await?;

        let transaction = locked_db.inner.transaction().await.map_mm_err()?;
        let table = transaction.table::<HDAccountTable>().await.map_mm_err()?;

        let new_account = HDAccountTable::new(wallet_id, account);
        table
            .add_item(&new_account)
            .await
            .map(|_| ())
            .mm_err(HDWalletStorageError::from)
    }

    async fn clear_accounts(&self, wallet_id: HDWalletId) -> HDWalletStorageResult<()> {
        let shared_db = self.get_shared_db()?;
        let locked_db = Self::lock_db_mutex(&shared_db).await?;

        let transaction = locked_db.inner.transaction().await.map_mm_err()?;
        let table = transaction.table::<HDAccountTable>().await.map_mm_err()?;

        let index_keys = MultiIndex::new(WALLET_ID_INDEX)
            .with_value(wallet_id.coin)
            .map_mm_err()?
            .with_value(wallet_id.hd_wallet_rmd160)
            .map_mm_err()?;
        table.delete_items_by_multi_index(index_keys).await.map_mm_err()?;
        Ok(())
    }
}

impl HDWalletIndexedDbStorage {
    fn get_shared_db(&self) -> HDWalletStorageResult<SharedDb<HDWalletDb>> {
        self.db
            .upgrade()
            .or_mm_err(|| HDWalletStorageError::Internal("'HDWalletIndexedDbStorage::db' doesn't exist".to_owned()))
    }

    async fn lock_db_mutex(db: &SharedDb<HDWalletDb>) -> HDWalletStorageResult<HDWalletDbLocked<'_>> {
        db.get_or_initialize().await.mm_err(HDWalletStorageError::from)
    }

    async fn find_account(
        table: &DbTable<'_, HDAccountTable>,
        wallet_id: HDWalletId,
        account_id: u32,
    ) -> HDWalletStorageResult<Option<(ItemId, HDAccountTable)>> {
        let index_keys = MultiIndex::new(WALLET_ACCOUNT_ID_INDEX)
            .with_value(wallet_id.coin)
            .map_mm_err()?
            .with_value(wallet_id.hd_wallet_rmd160)
            .map_mm_err()?
            .with_value(account_id)
            .map_mm_err()?;
        table
            .get_item_by_unique_multi_index(index_keys)
            .await
            .mm_err(HDWalletStorageError::from)
    }

    async fn update_account<F>(&self, wallet_id: HDWalletId, account_id: u32, f: F) -> HDWalletStorageResult<()>
    where
        F: FnOnce(&mut HDAccountTable),
    {
        let shared_db = self.get_shared_db()?;
        let locked_db = Self::lock_db_mutex(&shared_db).await?;

        let transaction = locked_db.inner.transaction().await.map_mm_err()?;
        let table = transaction.table::<HDAccountTable>().await.map_mm_err()?;

        let (account_item_id, mut account) = Self::find_account(&table, wallet_id.clone(), account_id)
            .await?
            .or_mm_err(|| HDWalletStorageError::HDAccountNotFound { wallet_id, account_id })?;

        // Apply `f` to `account` and upload the changes to the storage.
        f(&mut account);
        table
            .replace_item(account_item_id, &account)
            .await
            .map(|_| ())
            .mm_err(HDWalletStorageError::from)
    }
}

/// This function is used in `hd_wallet_storage::tests`.
pub(super) async fn get_all_storage_items(ctx: &MmArc) -> Vec<HDAccountStorageItem> {
    let coins_ctx = CoinsContext::from_ctx(ctx).unwrap();
    let db = coins_ctx.hd_wallet_db.get_or_initialize().await.unwrap();
    let transaction = db.inner.transaction().await.unwrap();
    let table = transaction.table::<HDAccountTable>().await.unwrap();
    table
        .get_all_items()
        .await
        .expect("Error getting items")
        .into_iter()
        .map(|(_item_id, item)| HDAccountStorageItem::from(item))
        .collect()
}
