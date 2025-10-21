use crate::my_tx_history_v2::{GetHistoryResult, RemoveTxResult, TxHistoryStorage};
use crate::tx_history_storage::wasm::tx_history_db::{TxHistoryDb, TxHistoryDbLocked};
use crate::tx_history_storage::wasm::{WasmTxHistoryError, WasmTxHistoryResult};
use crate::tx_history_storage::{
    token_id_from_tx_type, ConfirmationStatus, CreateTxHistoryStorageError, FilteringAddresses, GetTxHistoryFilters,
    WalletId,
};
use crate::{compare_transaction_details, CoinsContext, TransactionDetails};
use async_trait::async_trait;
use common::PagingOptionsEnum;
use itertools::Itertools;
use mm2_core::mm_ctx::MmArc;
use mm2_db::indexed_db::{BeBigUint, DbUpgrader, MultiIndex, OnUpgradeResult, SharedDb, TableSignature};
use mm2_err_handle::prelude::*;
use rpc::v1::types::Bytes as BytesJson;
use serde_json::{self as json, Value as Json};

impl WalletId {
    /// If [`WalletId::hd_wallet_rmd160`] is not specified,
    /// we need to exclude transactions of each HD wallet by specifying an empty `hd_wallet_rmd160`.
    fn hd_wallet_rmd160_or_exclude(&self) -> String {
        self.hd_wallet_rmd160.map(|hash| hash.to_string()).unwrap_or_default()
    }
}

#[derive(Clone)]
pub struct IndexedDbTxHistoryStorage {
    db: SharedDb<TxHistoryDb>,
}

impl IndexedDbTxHistoryStorage {
    pub fn new(ctx: &MmArc) -> MmResult<Self, CreateTxHistoryStorageError>
    where
        Self: Sized,
    {
        let coins_ctx = CoinsContext::from_ctx(ctx).map_to_mm(CreateTxHistoryStorageError::Internal)?;
        Ok(IndexedDbTxHistoryStorage {
            db: coins_ctx.tx_history_db.clone(),
        })
    }
}

#[async_trait]
impl TxHistoryStorage for IndexedDbTxHistoryStorage {
    type Error = WasmTxHistoryError;

    async fn init(&self, _wallet_id: &WalletId) -> MmResult<(), Self::Error> {
        Ok(())
    }

    async fn is_initialized_for(&self, _wallet_id: &WalletId) -> MmResult<bool, Self::Error> {
        Ok(true)
    }

    /// Adds multiple transactions to the selected coin's history
    /// Also consider adding tx_hex to the cache during this operation
    async fn add_transactions_to_history<I>(&self, wallet_id: &WalletId, transactions: I) -> MmResult<(), Self::Error>
    where
        I: IntoIterator<Item = TransactionDetails> + Send + 'static,
        I::IntoIter: Send,
    {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let history_table = db_transaction.table::<TxHistoryTableV2>().await.map_mm_err()?;
        let cache_table = db_transaction.table::<TxCacheTableV2>().await.map_mm_err()?;

        for tx in transactions {
            let Some(tx_hash) = tx.tx.tx_hash() else { continue };

            let history_item = TxHistoryTableV2::from_tx_details(wallet_id.clone(), &tx)?;
            history_table.add_item(&history_item).await.map_mm_err()?;

            let cache_item = TxCacheTableV2::from_tx_details(wallet_id.clone(), &tx)?;
            let index_keys = MultiIndex::new(TxCacheTableV2::COIN_TX_HASH_INDEX)
                .with_value(&wallet_id.ticker)
                .map_mm_err()?
                .with_value(tx_hash)
                .map_mm_err()?;
            // `TxHistoryTableV2::tx_hash` is not a unique field, but `TxCacheTableV2::tx_hash` is unique.
            // So we use `DbTable::add_item_or_ignore_by_unique_multi_index` instead of `DbTable::add_item`
            // since `transactions` may contain txs with same `tx_hash` but different `internal_id`.
            cache_table
                .add_item_or_ignore_by_unique_multi_index(index_keys, &cache_item)
                .await
                .map_mm_err()?;
        }
        Ok(())
    }

    async fn remove_tx_from_history(
        &self,
        wallet_id: &WalletId,
        internal_id: &BytesJson,
    ) -> MmResult<RemoveTxResult, Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let table = db_transaction.table::<TxHistoryTableV2>().await.map_mm_err()?;

        let index_keys = MultiIndex::new(TxHistoryTableV2::WALLET_ID_INTERNAL_ID_INDEX)
            .with_value(&wallet_id.ticker)
            .map_mm_err()?
            .with_value(wallet_id.hd_wallet_rmd160_or_exclude())
            .map_mm_err()?
            .with_value(internal_id)
            .map_mm_err()?;

        if table
            .delete_item_by_unique_multi_index(index_keys)
            .await
            .map_mm_err()?
            .is_some()
        {
            Ok(RemoveTxResult::TxRemoved)
        } else {
            Ok(RemoveTxResult::TxDidNotExist)
        }
    }

    async fn get_tx_from_history(
        &self,
        wallet_id: &WalletId,
        internal_id: &BytesJson,
    ) -> MmResult<Option<TransactionDetails>, Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let table = db_transaction.table::<TxHistoryTableV2>().await.map_mm_err()?;

        let index_keys = MultiIndex::new(TxHistoryTableV2::WALLET_ID_INTERNAL_ID_INDEX)
            .with_value(&wallet_id.ticker)
            .map_mm_err()?
            .with_value(wallet_id.hd_wallet_rmd160_or_exclude())
            .map_mm_err()?
            .with_value(internal_id)
            .map_mm_err()?;

        let details_json = match table.get_item_by_unique_multi_index(index_keys).await.map_mm_err()? {
            Some((_item_id, item)) => item.details_json,
            None => return Ok(None),
        };
        json::from_value(details_json).map_to_mm(|e| WasmTxHistoryError::ErrorDeserializing(e.to_string()))
    }

    async fn get_highest_block_height(&self, _wallet_id: &WalletId) -> Result<Option<u32>, MmError<Self::Error>> {
        // TODO
        Ok(None)
    }

    /// Since we need to filter the transactions by the given `for_addresses`,
    /// we can't use [`DbTable::count_by_multi_index`].
    /// TODO consider one of the solutions described at [`IndexedDbTxHistoryStorage::get_history`].
    async fn history_contains_unconfirmed_txes(
        &self,
        wallet_id: &WalletId,
        for_addresses: FilteringAddresses,
    ) -> Result<bool, MmError<Self::Error>> {
        let txs = self
            .get_unconfirmed_txes_from_history(wallet_id, for_addresses)
            .await
            .map_mm_err()?;
        Ok(!txs.is_empty())
    }

    /// Gets the unconfirmed transactions from the history
    async fn get_unconfirmed_txes_from_history(
        &self,
        wallet_id: &WalletId,
        for_addresses: FilteringAddresses,
    ) -> MmResult<Vec<TransactionDetails>, Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let table = db_transaction.table::<TxHistoryTableV2>().await.map_mm_err()?;

        let index_keys = MultiIndex::new(TxHistoryTableV2::WALLET_ID_CONFIRMATION_STATUS_INDEX)
            .with_value(&wallet_id.ticker)
            .map_mm_err()?
            .with_value(wallet_id.hd_wallet_rmd160_or_exclude())
            .map_mm_err()?
            .with_value(ConfirmationStatus::Unconfirmed)
            .map_mm_err()?;

        let transactions = table
            .get_items_by_multi_index(index_keys)
            .await
            .map_mm_err()?
            .into_iter()
            .map(|(_item_id, item)| item);

        Self::take_according_to_filtering_addresses(transactions, &for_addresses)
    }

    /// Updates transaction in the selected coin's history
    async fn update_tx_in_history(&self, wallet_id: &WalletId, tx: &TransactionDetails) -> MmResult<(), Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let table = db_transaction.table::<TxHistoryTableV2>().await.map_mm_err()?;

        let index_keys = MultiIndex::new(TxHistoryTableV2::WALLET_ID_INTERNAL_ID_INDEX)
            .with_value(&wallet_id.ticker)
            .map_mm_err()?
            .with_value(wallet_id.hd_wallet_rmd160_or_exclude())
            .map_mm_err()?
            .with_value(&tx.internal_id)
            .map_mm_err()?;
        let item = TxHistoryTableV2::from_tx_details(wallet_id.clone(), tx)?;
        table
            .replace_item_by_unique_multi_index(index_keys, &item)
            .await
            .map_mm_err()?;
        Ok(())
    }

    async fn history_has_tx_hash(&self, wallet_id: &WalletId, tx_hash: &str) -> Result<bool, MmError<Self::Error>> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let table = db_transaction.table::<TxHistoryTableV2>().await.map_mm_err()?;

        let index_keys = MultiIndex::new(TxHistoryTableV2::WALLET_ID_TX_HASH_INDEX)
            .with_value(&wallet_id.ticker)
            .map_mm_err()?
            .with_value(wallet_id.hd_wallet_rmd160_or_exclude())
            .map_mm_err()?
            .with_value(tx_hash)
            .map_mm_err()?;
        let count_txs = table.count_by_multi_index(index_keys).await.map_mm_err()?;
        Ok(count_txs > 0)
    }

    /// TODO consider refactoring this method to avoid fetching all transactions.
    async fn unique_tx_hashes_num_in_history(
        &self,
        wallet_id: &WalletId,
        for_addresses: FilteringAddresses,
    ) -> Result<usize, MmError<Self::Error>> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let table = db_transaction.table::<TxHistoryTableV2>().await.map_mm_err()?;

        let index_keys = MultiIndex::new(TxHistoryTableV2::WALLET_ID_INDEX)
            .with_value(&wallet_id.ticker)
            .map_mm_err()?
            .with_value(wallet_id.hd_wallet_rmd160_or_exclude())
            .map_mm_err()?;

        // `IndexedDb` doesn't provide an elegant way to count records applying custom filters to index properties like `tx_hash`,
        // so currently fetch all records with `coin,hd_wallet_rmd160=wallet_id` and apply the `unique_by(|tx| tx.tx_hash)` to them.
        let transactions = table
            .get_items_by_multi_index(index_keys)
            .await
            .map_mm_err()?
            .into_iter()
            .map(|(_item_id, tx)| tx)
            .unique_by(|tx| tx.tx_hash.clone());

        let filtered_transactions = Self::take_according_to_filtering_addresses(transactions, &for_addresses)?;
        Ok(filtered_transactions.len())
    }

    async fn add_tx_to_cache(
        &self,
        wallet_id: &WalletId,
        tx_hash: &str,
        tx_hex: &BytesJson,
    ) -> Result<(), MmError<Self::Error>> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let table = db_transaction.table::<TxCacheTableV2>().await.map_mm_err()?;

        table
            .add_item(&TxCacheTableV2 {
                coin: wallet_id.ticker.clone(),
                tx_hash: tx_hash.to_owned(),
                tx_hex: tx_hex.clone(),
            })
            .await
            .map_mm_err()?;
        Ok(())
    }

    async fn tx_bytes_from_cache(
        &self,
        wallet_id: &WalletId,
        tx_hash: &str,
    ) -> MmResult<Option<BytesJson>, Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let table = db_transaction.table::<TxCacheTableV2>().await.map_mm_err()?;

        let index_keys = MultiIndex::new(TxCacheTableV2::COIN_TX_HASH_INDEX)
            .with_value(&wallet_id.ticker)
            .map_mm_err()?
            .with_value(tx_hash)
            .map_mm_err()?;
        match table.get_item_by_unique_multi_index(index_keys).await.map_mm_err()? {
            Some((_item_id, item)) => Ok(Some(item.tx_hex)),
            None => Ok(None),
        }
    }

    /// This is totally inefficient due to we query all items from the storage
    /// and then checks whether it were sent from/to one of the specified `for_addresses`.
    ///
    /// TODO One of the possible solutions is to do the following:
    /// 1) Add `TxFromAddressTable` and `TxToAddressTable` tables;
    /// 2) Add [`CursorBoundValue::BTreeSet`] that iterates items over its index values;
    /// 3) Query transaction internal IDs from the `TxFromAddressTable` and `TxToAddressTable` tables
    ///    by using a cursor with the specified `ticker`, `hd_wallet_rmd160`, `token_id` constant indexes
    ///    and the iterable [`CursorBoundValue::BTreeMap = for_addresses`] index;
    /// 4) Query transaction details from the `TxHistoryTableV2` table by using a cursor with the specified `ticker`, `hd_wallet_rmd160`, `token_id` constant indexes
    ///    and the iterable [`CursorBoundValue::BTreeMap = expected_internal_ids`].
    async fn get_history(
        &self,
        wallet_id: &WalletId,
        filters: GetTxHistoryFilters,
        paging: PagingOptionsEnum<BytesJson>,
        limit: usize,
    ) -> MmResult<GetHistoryResult, Self::Error> {
        // Check if [`GetTxHistoryFilters::for_addresses`] is empty.
        // If it is, it's much more efficient to return an empty result before we do any query.
        if filters.for_addresses.is_empty() {
            return Ok(GetHistoryResult {
                transactions: Vec::new(),
                skipped: 0,
                total: 0,
            });
        }

        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let table = db_transaction.table::<TxHistoryTableV2>().await.map_mm_err()?;

        let index_keys = MultiIndex::new(TxHistoryTableV2::WALLET_ID_TOKEN_ID_INDEX)
            .with_value(&wallet_id.ticker)
            .map_mm_err()?
            .with_value(wallet_id.hd_wallet_rmd160_or_exclude())
            .map_mm_err()?
            .with_value(filters.token_id_or_exclude())
            .map_mm_err()?;

        let transactions = table
            .get_items_by_multi_index(index_keys)
            .await
            .map_mm_err()?
            .into_iter()
            .map(|(_item_id, tx)| tx);

        let transactions = Self::take_according_to_filtering_addresses(transactions, &filters.for_addresses)?;
        Self::take_according_to_paging_opts(transactions, paging, limit)
    }
}

impl IndexedDbTxHistoryStorage {
    fn take_according_to_filtering_addresses<I>(
        txs: I,
        for_addresses: &FilteringAddresses,
    ) -> WasmTxHistoryResult<Vec<TransactionDetails>>
    where
        I: Iterator<Item = TxHistoryTableV2>,
    {
        txs.filter(|tx| {
            tx.from_addresses.has_intersection(for_addresses) || tx.to_addresses.has_intersection(for_addresses)
        })
        .map(tx_details_from_item)
        .collect()
    }

    pub(super) fn take_according_to_paging_opts(
        mut txs: Vec<TransactionDetails>,
        paging: PagingOptionsEnum<BytesJson>,
        limit: usize,
    ) -> WasmTxHistoryResult<GetHistoryResult> {
        let total_count = txs.len();

        // This is super inefficient to fetch the whole transaction history, sort it on the client side.
        // It's required to implement `DESC` order for `IdbCursor` in order to sort the transactions
        // the same way as `compare_transaction_details` does.
        // But it's difficult to implement, and I think it can be postponed for a while.
        txs.sort_by(compare_transaction_details);

        let skip = match paging {
            // `page_number` is ignored if from_uuid is set
            PagingOptionsEnum::FromId(from_internal_id) => {
                let maybe_skip = txs
                    .iter()
                    .position(|tx| tx.internal_id == from_internal_id)
                    .map(|pos| pos + 1);
                match maybe_skip {
                    Some(skip) => skip,
                    None => {
                        return Ok(GetHistoryResult {
                            transactions: Vec::new(),
                            skipped: 0,
                            total: total_count,
                        })
                    },
                }
            },
            PagingOptionsEnum::PageNumber(page_number) => (page_number.get() - 1) * limit,
        };

        Ok(GetHistoryResult {
            transactions: txs.into_iter().skip(skip).take(limit).collect(),
            skipped: skip,
            total: total_count,
        })
    }

    async fn lock_db(&self) -> WasmTxHistoryResult<TxHistoryDbLocked<'_>> {
        self.db.get_or_initialize().await.mm_err(WasmTxHistoryError::from)
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct TxHistoryTableV2 {
    coin: String,
    hd_wallet_rmd160: String,
    tx_hash: String,
    internal_id: BytesJson,
    block_height: BeBigUint,
    confirmation_status: ConfirmationStatus,
    token_id: String,
    from_addresses: FilteringAddresses,
    to_addresses: FilteringAddresses,
    details_json: Json,
}

impl TxHistoryTableV2 {
    /// An index that consists of the only one `coin` property.
    const WALLET_ID_INDEX: &'static str = "wallet_id";
    /// A **unique** index that consists of the following properties:
    /// * coin - coin ticker
    /// * internal_id - transaction internal ID
    const WALLET_ID_INTERNAL_ID_INDEX: &'static str = "wallet_id_internal_id";
    /// An index that consists of the following properties:
    /// * coin - coin ticker
    /// * tx_hash - transaction hash
    const WALLET_ID_TX_HASH_INDEX: &'static str = "wallet_id_tx_hash";
    /// An index that consists of the following properties:
    /// * coin - coin ticker
    /// * confirmation_status - whether transaction is confirmed or unconfirmed
    const WALLET_ID_CONFIRMATION_STATUS_INDEX: &'static str = "wallet_id_confirmation_status";
    /// An index that consists of the following properties:
    /// * coin - coin ticker
    /// * token_id - token ID (can be an empty string)
    const WALLET_ID_TOKEN_ID_INDEX: &'static str = "wallet_id_token_id";

    fn from_tx_details(wallet_id: WalletId, tx: &TransactionDetails) -> WasmTxHistoryResult<TxHistoryTableV2> {
        let tx_hash = tx
            .tx
            .tx_hash()
            .ok_or_else(|| WasmTxHistoryError::NotSupported("Unsupported type of TransactionDetails".to_string()))?;

        let details_json = json::to_value(tx).map_to_mm(|e| WasmTxHistoryError::ErrorSerializing(e.to_string()))?;
        let hd_wallet_rmd160 = wallet_id.hd_wallet_rmd160_or_exclude();
        Ok(TxHistoryTableV2 {
            coin: wallet_id.ticker,
            hd_wallet_rmd160,
            tx_hash: tx_hash.to_string(),
            internal_id: tx.internal_id.clone(),
            block_height: BeBigUint::from(tx.block_height),
            confirmation_status: ConfirmationStatus::from_block_height(tx.block_height),
            token_id: token_id_from_tx_type(&tx.transaction_type),
            from_addresses: tx.from.clone().into_iter().collect(),
            to_addresses: tx.to.clone().into_iter().collect(),
            details_json,
        })
    }
}

impl TableSignature for TxHistoryTableV2 {
    const TABLE_NAME: &'static str = "tx_history_v2";

    fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
        if let (0, 1) = (old_version, new_version) {
            let table = upgrader.create_table(Self::TABLE_NAME)?;
            table.create_multi_index(TxHistoryTableV2::WALLET_ID_INDEX, &["coin", "hd_wallet_rmd160"], false)?;
            table.create_multi_index(
                TxHistoryTableV2::WALLET_ID_INTERNAL_ID_INDEX,
                &["coin", "hd_wallet_rmd160", "internal_id"],
                true,
            )?;
            table.create_multi_index(
                TxHistoryTableV2::WALLET_ID_TX_HASH_INDEX,
                &["coin", "hd_wallet_rmd160", "tx_hash"],
                false,
            )?;
            table.create_multi_index(
                TxHistoryTableV2::WALLET_ID_CONFIRMATION_STATUS_INDEX,
                &["coin", "hd_wallet_rmd160", "confirmation_status"],
                false,
            )?;
            table.create_multi_index(
                TxHistoryTableV2::WALLET_ID_TOKEN_ID_INDEX,
                &["coin", "hd_wallet_rmd160", "token_id"],
                false,
            )?;
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct TxCacheTableV2 {
    coin: String,
    tx_hash: String,
    tx_hex: BytesJson,
}

impl TxCacheTableV2 {
    /// A **unique** index that consists of the following properties:
    /// * coin - coin ticker
    /// * tx_hash - transaction hash
    const COIN_TX_HASH_INDEX: &'static str = "coin_tx_hash";

    fn from_tx_details(wallet_id: WalletId, tx: &TransactionDetails) -> WasmTxHistoryResult<TxCacheTableV2> {
        if let (Some(tx_hash), Some(tx_hex)) = (tx.tx.tx_hash(), tx.tx.tx_hex()) {
            return Ok(TxCacheTableV2 {
                coin: wallet_id.ticker,
                tx_hash: tx_hash.to_string(),
                tx_hex: tx_hex.clone(),
            });
        }

        MmError::err(WasmTxHistoryError::NotSupported(
            "Unsupported type of TransactionDetails".to_string(),
        ))
    }
}

impl TableSignature for TxCacheTableV2 {
    const TABLE_NAME: &'static str = "tx_cache_v2";

    fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
        if let (0, 1) = (old_version, new_version) {
            let table = upgrader.create_table(Self::TABLE_NAME)?;
            table.create_multi_index(TxCacheTableV2::COIN_TX_HASH_INDEX, &["coin", "tx_hash"], true)?;
        }
        Ok(())
    }
}

fn tx_details_from_item(item: TxHistoryTableV2) -> WasmTxHistoryResult<TransactionDetails> {
    json::from_value(item.details_json).map_to_mm(|e| WasmTxHistoryError::ErrorDeserializing(e.to_string()))
}
