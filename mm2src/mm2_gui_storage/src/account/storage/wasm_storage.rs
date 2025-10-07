use crate::account::storage::{AccountStorage, AccountStorageError, AccountStorageResult};
use crate::account::{
    AccountId, AccountInfo, AccountType, AccountWithCoins, AccountWithEnabledFlag, EnabledAccountId,
    EnabledAccountType, HwPubkey,
};
use async_trait::async_trait;
use mm2_core::mm_ctx::MmArc;
use mm2_db::indexed_db::{
    ConstructibleDb, DbIdentifier, DbInstance, DbLocked, DbTransaction, DbTransactionError, DbUpgrader, IndexedDb,
    IndexedDbBuilder, InitDbError, InitDbResult, MultiIndex, OnUpgradeResult, SharedDb, TableSignature,
};
use mm2_err_handle::prelude::*;
use mm2_number::BigDecimal;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

const DB_VERSION: u32 = 1;

type AccountDbLocked<'a> = DbLocked<'a, AccountDb>;

impl From<DbTransactionError> for AccountStorageError {
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
            | DbTransactionError::TransactionAborted => AccountStorageError::Internal(desc),
            DbTransactionError::ErrorDeserializingItem(_) => AccountStorageError::ErrorDeserializing(desc),
            DbTransactionError::ErrorSerializingItem(_) => AccountStorageError::ErrorSerializing(desc),
            DbTransactionError::ErrorGettingItems(_) | DbTransactionError::ErrorCountingItems(_) => {
                AccountStorageError::ErrorLoading(desc)
            },
            DbTransactionError::ErrorUploadingItem(_) | DbTransactionError::ErrorDeletingItems(_) => {
                AccountStorageError::ErrorSaving(desc)
            },
        }
    }
}

impl From<InitDbError> for AccountStorageError {
    fn from(e: InitDbError) -> Self {
        AccountStorageError::Internal(e.to_string())
    }
}

impl AccountId {
    fn try_to_enabled(&self) -> Option<EnabledAccountId> {
        match self {
            AccountId::Iguana => Some(EnabledAccountId::Iguana),
            AccountId::HD { account_idx } => Some(EnabledAccountId::HD {
                account_idx: *account_idx,
            }),
            AccountId::HW { .. } => None,
        }
    }
}

pub(crate) struct WasmAccountStorage {
    account_db: SharedDb<AccountDb>,
}

impl WasmAccountStorage {
    pub fn new(ctx: &MmArc) -> Self {
        WasmAccountStorage {
            account_db: ConstructibleDb::new_shared_db(ctx).into_shared(),
        }
    }

    async fn lock_db_mutex(&self) -> AccountStorageResult<AccountDbLocked<'_>> {
        self.account_db
            .get_or_initialize()
            .await
            .mm_err(AccountStorageError::from)
    }

    /// Loads accounts sorted by `AccountId`.
    /// This method takes `db_transaction` to ensure data coherence.
    async fn load_accounts(
        db_transaction: &DbTransaction<'_>,
    ) -> AccountStorageResult<BTreeMap<AccountId, AccountInfo>> {
        let table = db_transaction.table::<AccountTable>().await.map_mm_err()?;
        table
            .get_all_items()
            .await
            .map_mm_err()?
            .into_iter()
            .map(|(_item_id, account)| {
                let account_info = AccountInfo::try_from(account)?;
                Ok((account_info.account_id.clone(), account_info))
            })
            .collect()
    }

    /// Loads `AccountId` of an enabled account.
    /// This method takes `db_transaction` to ensure data coherence.
    async fn load_enabled_account_id(
        db_transaction: &DbTransaction<'_>,
    ) -> AccountStorageResult<Option<EnabledAccountId>> {
        let enabled_table = db_transaction.table::<EnabledAccountTable>().await.map_mm_err()?;
        let enabled_accounts = enabled_table.get_all_items().await.map_mm_err()?;
        if enabled_accounts.len() > 1 {
            let error = format!("Expected only one enabled account, found {}", enabled_accounts.len());
            return MmError::err(AccountStorageError::Internal(error));
        }
        match enabled_accounts.into_iter().next() {
            Some((_item_id, enabled_account)) => EnabledAccountId::try_from(enabled_account).map(Some),
            None => Ok(None),
        }
    }

    /// Loads `AccountId` of an enabled account or returns an error if there is no enabled account yet.
    /// This method takes `db_transaction` to ensure data coherence.
    async fn load_enabled_account_id_or_err(
        db_transaction: &DbTransaction<'_>,
    ) -> AccountStorageResult<EnabledAccountId> {
        let enabled_acc_id = Self::load_enabled_account_id(db_transaction)
            .await?
            .or_mm_err(|| AccountStorageError::NoEnabledAccount)?;
        Ok(enabled_acc_id)
    }

    /// Loads `AccountWithCoins`.
    /// This method takes `db_transaction` to ensure data coherence.
    async fn load_account_with_coins(
        db_transaction: &DbTransaction<'_>,
        account_id: &AccountId,
    ) -> AccountStorageResult<Option<AccountWithCoins>> {
        let table = db_transaction.table::<AccountTable>().await.map_mm_err()?;

        let index_keys = AccountTable::account_id_to_index(account_id)?;
        table
            .get_item_by_unique_multi_index(index_keys)
            .await
            .map_mm_err()?
            .map(|(_item_id, account)| AccountWithCoins::try_from(account))
            .transpose()
    }

    /// Checks whether an account with the given `account_id` exists.
    /// This method takes `db_transaction` to ensure data coherence.
    async fn account_exists(db_transaction: &DbTransaction<'_>, account_id: &AccountId) -> AccountStorageResult<bool> {
        let table = db_transaction.table::<AccountTable>().await.map_mm_err()?;

        let index_keys = AccountTable::account_id_to_index(account_id)?;
        let count = table.count_by_multi_index(index_keys).await.map_mm_err()?;
        Ok(count > 0)
    }

    /// Disable the given account if it's enabled.
    /// This method takes `db_transaction` to ensure data coherence.
    async fn disable_account_if_enabled(
        db_transaction: &DbTransaction<'_>,
        enabled_account_id: EnabledAccountId,
    ) -> AccountStorageResult<()> {
        match Self::load_enabled_account_id(db_transaction).await? {
            // If there is an enabled account **and** its ID is the same as `enabled_account_id`.
            Some(actual_enabled) if actual_enabled == enabled_account_id => (),
            _ => return Ok(()),
        }

        let table = db_transaction.table::<EnabledAccountTable>().await.map_mm_err()?;
        // Remove the account by clearing the table.
        table.clear().await.map_mm_err()?;
        Ok(())
    }

    /// Loads an account by `AccountId`, applies the given `f` function to it,
    /// and uploads changes to the storage.
    async fn update_account<F>(&self, account_id: AccountId, f: F) -> AccountStorageResult<()>
    where
        F: FnOnce(&mut AccountTable),
    {
        let locked_db = self.lock_db_mutex().await?;
        let transaction = locked_db.inner.transaction().await.map_mm_err()?;
        let table = transaction.table::<AccountTable>().await.map_mm_err()?;

        let index_keys = AccountTable::account_id_to_index(&account_id)?;
        let (item_id, mut account) = table
            .get_item_by_unique_multi_index(index_keys)
            .await
            .map_mm_err()?
            .or_mm_err(|| AccountStorageError::NoSuchAccount(account_id))?;
        f(&mut account);
        table.replace_item(item_id, &account).await.map_mm_err()?;
        Ok(())
    }
}

#[async_trait]
impl AccountStorage for WasmAccountStorage {
    /// [`WasmAccountStorage::lock_db_mutex`] initializes the database on the first call.
    async fn init(&self) -> AccountStorageResult<()> {
        self.lock_db_mutex().await.map(|_locked_db| ())
    }

    async fn load_account_coins(&self, account_id: AccountId) -> AccountStorageResult<BTreeSet<String>> {
        let locked_db = self.lock_db_mutex().await?;
        let transaction = locked_db.inner.transaction().await.map_mm_err()?;

        let account = Self::load_account_with_coins(&transaction, &account_id)
            .await?
            .or_mm_err(|| AccountStorageError::NoSuchAccount(account_id))?;
        Ok(account.coins)
    }

    async fn load_accounts(&self) -> AccountStorageResult<BTreeMap<AccountId, AccountInfo>> {
        let locked_db = self.lock_db_mutex().await?;
        let transaction = locked_db.inner.transaction().await.map_mm_err()?;

        Self::load_accounts(&transaction).await
    }

    async fn load_accounts_with_enabled_flag(
        &self,
    ) -> AccountStorageResult<BTreeMap<AccountId, AccountWithEnabledFlag>> {
        let locked_db = self.lock_db_mutex().await?;
        let transaction = locked_db.inner.transaction().await.map_mm_err()?;

        let enabled_account_id = AccountId::from(Self::load_enabled_account_id_or_err(&transaction).await?);

        let mut found_enabled = false;
        let accounts = Self::load_accounts(&transaction)
            .await?
            .into_iter()
            .map(|(account_id, account_info)| {
                let enabled = account_id == enabled_account_id;
                found_enabled |= enabled;
                Ok((account_id, AccountWithEnabledFlag { account_info, enabled }))
            })
            .collect::<AccountStorageResult<BTreeMap<_, _>>>()?;

        // If `AccountStorage::load_enabled_account_id` returns an `AccountId`,
        // then corresponding account must be in `AccountTable`.
        if !found_enabled {
            return MmError::err(AccountStorageError::unknown_account_in_enabled_table(
                enabled_account_id,
            ));
        }
        Ok(accounts)
    }

    async fn load_enabled_account_id(&self) -> AccountStorageResult<EnabledAccountId> {
        let locked_db = self.lock_db_mutex().await?;
        let transaction = locked_db.inner.transaction().await.map_mm_err()?;
        Self::load_enabled_account_id_or_err(&transaction).await
    }

    async fn load_enabled_account_with_coins(&self) -> AccountStorageResult<AccountWithCoins> {
        let locked_db = self.lock_db_mutex().await?;
        let transaction = locked_db.inner.transaction().await.map_mm_err()?;

        let account_id = AccountId::from(Self::load_enabled_account_id_or_err(&transaction).await?);

        Self::load_account_with_coins(&transaction, &account_id)
            .await?
            .or_mm_err(|| AccountStorageError::unknown_account_in_enabled_table(account_id))
    }

    async fn enable_account(&self, enabled_account_id: EnabledAccountId) -> AccountStorageResult<()> {
        let locked_db = self.lock_db_mutex().await?;
        let transaction = locked_db.inner.transaction().await.map_mm_err()?;

        let account_id = AccountId::from(enabled_account_id);

        // First, check if the account exists.
        if !Self::account_exists(&transaction, &account_id).await? {
            return MmError::err(AccountStorageError::NoSuchAccount(account_id));
        }

        let table = transaction.table::<EnabledAccountTable>().await.map_mm_err()?;
        // Remove the previous enabled account by clearing the table.
        table.clear().await.map_mm_err()?;

        table
            .add_item(&EnabledAccountTable::from(enabled_account_id))
            .await
            .map_mm_err()?;
        Ok(())
    }

    async fn upload_account(&self, account_info: AccountInfo) -> AccountStorageResult<()> {
        let locked_db = self.lock_db_mutex().await?;
        let transaction = locked_db.inner.transaction().await.map_mm_err()?;

        // First, check if the account doesn't exist.
        if Self::account_exists(&transaction, &account_info.account_id).await? {
            return MmError::err(AccountStorageError::AccountExistsAlready(account_info.account_id));
        }

        let table = transaction.table::<AccountTable>().await.map_mm_err()?;
        table.add_item(&AccountTable::from(account_info)).await.map_mm_err()?;
        Ok(())
    }

    async fn delete_account(&self, account_id: AccountId) -> AccountStorageResult<()> {
        let locked_db = self.lock_db_mutex().await?;
        let transaction = locked_db.inner.transaction().await.map_mm_err()?;

        // First, check if the account exists already.
        if !Self::account_exists(&transaction, &account_id).await? {
            return MmError::err(AccountStorageError::NoSuchAccount(account_id));
        }

        // Check if the account can be enabled.
        if let Some(enabled_account_id) = account_id.try_to_enabled() {
            Self::disable_account_if_enabled(&transaction, enabled_account_id).await?;
        }

        // Remove the account info.
        let table = transaction.table::<AccountTable>().await.map_mm_err()?;
        let index_keys = AccountTable::account_id_to_index(&account_id)?;
        table.delete_item_by_unique_multi_index(index_keys).await.map_mm_err()?;
        Ok(())
    }

    async fn set_name(&self, account_id: AccountId, name: String) -> AccountStorageResult<()> {
        self.update_account(account_id, |account| account.name = name).await
    }

    async fn set_description(&self, account_id: AccountId, description: String) -> AccountStorageResult<()> {
        self.update_account(account_id, |account| account.description = description)
            .await
    }

    async fn set_balance(&self, account_id: AccountId, balance_usd: BigDecimal) -> AccountStorageResult<()> {
        self.update_account(account_id, |account| account.balance_usd = balance_usd)
            .await
    }

    async fn activate_coins(&self, account_id: AccountId, tickers: Vec<String>) -> AccountStorageResult<()> {
        self.update_account(account_id, |account| account.activated_coins.extend(tickers))
            .await
    }

    async fn deactivate_coins(&self, account_id: AccountId, tickers: Vec<String>) -> AccountStorageResult<()> {
        self.update_account(account_id, |account| {
            for ticker in tickers.iter() {
                account.activated_coins.remove(ticker);
            }
        })
        .await
    }
}

struct AccountDb {
    inner: IndexedDb,
}

#[async_trait]
impl DbInstance for AccountDb {
    const DB_NAME: &'static str = "gui_account_storage";

    async fn init(db_id: DbIdentifier) -> InitDbResult<Self> {
        let inner = IndexedDbBuilder::new(db_id)
            .with_version(DB_VERSION)
            .with_table::<AccountTable>()
            .with_table::<EnabledAccountTable>()
            .build()
            .await?;
        Ok(AccountDb { inner })
    }
}

#[derive(Deserialize, Serialize)]
struct AccountTable {
    account_type: AccountType,
    /// `None` if [`AccountTable::account_type`] is [`AccountType::Iguana`] or [`AccountType::HW`].
    account_idx: u32,
    /// `None` if [`AccountTable::account_type`] is [`AccountType::Iguana`] or [`AccountType::HD`].
    device_pubkey: HwPubkey,
    name: String,
    description: String,
    balance_usd: BigDecimal,
    activated_coins: BTreeSet<String>,
}

impl AccountTable {
    /// An **unique** index that consists of the following properties:
    /// * account_type
    /// * account_idx
    /// * device_pubkey
    const ACCOUNT_ID_INDEX: &'static str = "account_id";

    fn account_id_to_index(account_id: &AccountId) -> AccountStorageResult<MultiIndex> {
        let (account_type, account_idx, device_pubkey) = account_id.to_tuple();

        let multi_index = MultiIndex::new(AccountTable::ACCOUNT_ID_INDEX)
            .with_value(account_type)
            .map_mm_err()?
            .with_value(account_idx)
            .map_mm_err()?
            .with_value(device_pubkey)
            .map_mm_err()?;
        Ok(multi_index)
    }
}

impl TableSignature for AccountTable {
    const TABLE_NAME: &'static str = "gui_account";

    fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
        if let (0, 1) = (old_version, new_version) {
            let table = upgrader.create_table(Self::TABLE_NAME)?;
            table.create_multi_index(
                AccountTable::ACCOUNT_ID_INDEX,
                &["account_type", "account_idx", "device_pubkey"],
                true,
            )?;
        }

        Ok(())
    }
}

impl From<AccountInfo> for AccountTable {
    fn from(orig: AccountInfo) -> Self {
        let (account_type, account_idx, device_pubkey) = orig.account_id.to_tuple();
        AccountTable {
            account_type,
            account_idx,
            device_pubkey,
            name: orig.name,
            description: orig.description,
            balance_usd: orig.balance_usd,
            activated_coins: BTreeSet::new(),
        }
    }
}

impl TryFrom<AccountTable> for AccountInfo {
    type Error = MmError<AccountStorageError>;

    fn try_from(value: AccountTable) -> Result<Self, Self::Error> {
        Ok(AccountInfo {
            account_id: AccountId::try_from_tuple(value.account_type, value.account_idx, value.device_pubkey)?,
            name: value.name,
            description: value.description,
            balance_usd: value.balance_usd,
        })
    }
}

impl TryFrom<AccountTable> for AccountWithCoins {
    type Error = MmError<AccountStorageError>;

    fn try_from(value: AccountTable) -> Result<Self, Self::Error> {
        let coins = value.activated_coins.clone();
        Ok(AccountWithCoins {
            account_info: AccountInfo::try_from(value)?,
            coins,
        })
    }
}

/// The table consists of one item that points to the enabled account,
/// or the table is empty.
#[derive(Deserialize, Serialize)]
struct EnabledAccountTable {
    account_type: EnabledAccountType,
    /// `None` if [`EnabledAccountTable::account_type`] is [`EnabledAccountTable::Iguana`].
    account_idx: u32,
}

impl From<EnabledAccountId> for EnabledAccountTable {
    fn from(account_id: EnabledAccountId) -> Self {
        let (account_type, account_idx, _device_pubkey) = account_id.to_tuple();
        EnabledAccountTable {
            account_type,
            account_idx,
        }
    }
}

impl TryFrom<EnabledAccountTable> for EnabledAccountId {
    type Error = MmError<AccountStorageError>;

    fn try_from(value: EnabledAccountTable) -> Result<Self, Self::Error> {
        EnabledAccountId::try_from_pair(value.account_type, value.account_idx)
    }
}

impl TableSignature for EnabledAccountTable {
    const TABLE_NAME: &'static str = "gui_enabled_account";

    fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
        if let (0, 1) = (old_version, new_version) {
            let table = upgrader.create_table(Self::TABLE_NAME)?;
            table.create_multi_index(
                AccountTable::ACCOUNT_ID_INDEX,
                &["account_type", "account_idx", "device_pubkey"],
                true,
            )?;
        }

        Ok(())
    }
}
