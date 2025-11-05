use crate::hd_wallet::{AddressDerivingError, DisplayAddress, InvalidBip44ChainError};
use crate::tendermint::{
    BCH_COIN_PROTOCOL_TYPE, BCH_TOKEN_PROTOCOL_TYPE, TENDERMINT_ASSET_PROTOCOL_TYPE, TENDERMINT_COIN_PROTOCOL_TYPE,
};
use crate::tx_history_storage::{
    CreateTxHistoryStorageError, FilteringAddresses, GetTxHistoryFilters, TxHistoryStorageBuilder, WalletId,
};
use crate::utxo::utxo_common::big_decimal_from_sat_unsigned;
use crate::{
    coin_conf, lp_coinfind_or_err, BlockHeightAndTime, CoinFindError, HDPathAccountToAddressId, HistorySyncState,
    MmCoin, MmCoinEnum, MyAddressError, Transaction, TransactionData, TransactionDetails, TransactionType,
    TxFeeDetails, UtxoRpcError,
};
use async_trait::async_trait;
use bitcrypto::sha256;
use common::{calc_total_pages, ten, HttpStatusCode, PagingOptionsEnum, StatusCode};
use derive_more::Display;
use enum_derives::EnumFromStringify;
use futures::compat::Future01CompatExt;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::BigDecimal;
use num_traits::ToPrimitive;
use rpc::v1::types::{Bytes as BytesJson, ToTxHash};
use std::collections::HashSet;

#[derive(Debug)]
pub enum RemoveTxResult {
    TxRemoved,
    TxDidNotExist,
}

impl RemoveTxResult {
    pub fn tx_existed(&self) -> bool {
        matches!(self, RemoveTxResult::TxRemoved)
    }
}

pub struct GetHistoryResult {
    pub transactions: Vec<TransactionDetails>,
    pub skipped: usize,
    pub total: usize,
}

pub trait TxHistoryStorageError: std::fmt::Debug + NotMmError + Send {}

#[async_trait]
pub trait TxHistoryStorage: Send + Sync + 'static {
    type Error: TxHistoryStorageError;

    /// Initializes collection/tables in storage for the specified wallet.
    async fn init(&self, wallet_id: &WalletId) -> Result<(), MmError<Self::Error>>;

    /// Whether collections/tables are initialized for the specified wallet.
    async fn is_initialized_for(&self, wallet_id: &WalletId) -> Result<bool, MmError<Self::Error>>;

    /// Adds multiple transactions to the selected wallet's history.
    /// Also consider adding tx_hex to the cache during this operation.
    async fn add_transactions_to_history<I>(
        &self,
        wallet_id: &WalletId,
        transactions: I,
    ) -> Result<(), MmError<Self::Error>>
    where
        I: IntoIterator<Item = TransactionDetails> + Send + 'static,
        I::IntoIter: Send;

    /// Removes the transaction by internal_id from the selected wallet's history.
    async fn remove_tx_from_history(
        &self,
        wallet_id: &WalletId,
        internal_id: &BytesJson,
    ) -> Result<RemoveTxResult, MmError<Self::Error>>;

    /// Gets the transaction by internal_id from the selected wallet's history
    async fn get_tx_from_history(
        &self,
        wallet_id: &WalletId,
        internal_id: &BytesJson,
    ) -> Result<Option<TransactionDetails>, MmError<Self::Error>>;

    /// Gets the highest block_height from the selected wallet's history
    async fn get_highest_block_height(&self, wallet_id: &WalletId) -> Result<Option<u32>, MmError<Self::Error>>;

    /// Returns whether the history contains unconfirmed transactions.
    async fn history_contains_unconfirmed_txes(
        &self,
        wallet_id: &WalletId,
        for_addresses: FilteringAddresses,
    ) -> Result<bool, MmError<Self::Error>>;

    /// Gets the unconfirmed transactions from the wallet's history.
    async fn get_unconfirmed_txes_from_history(
        &self,
        wallet_id: &WalletId,
        for_addresses: FilteringAddresses,
    ) -> Result<Vec<TransactionDetails>, MmError<Self::Error>>;

    /// Updates transaction in the selected wallet's history
    async fn update_tx_in_history(
        &self,
        wallet_id: &WalletId,
        tx: &TransactionDetails,
    ) -> Result<(), MmError<Self::Error>>;

    /// Whether the selected wallet's history contains a transaction with the given `tx_hash`.
    async fn history_has_tx_hash(&self, wallet_id: &WalletId, tx_hash: &str) -> Result<bool, MmError<Self::Error>>;

    /// Returns the number of unique transaction hashes.
    async fn unique_tx_hashes_num_in_history(
        &self,
        wallet_id: &WalletId,
        for_addresses: FilteringAddresses,
    ) -> Result<usize, MmError<Self::Error>>;

    /// Adds the given `tx_hex` transaction to the selected wallet's cache.
    async fn add_tx_to_cache(
        &self,
        wallet_id: &WalletId,
        tx_hash: &str,
        tx_hex: &BytesJson,
    ) -> Result<(), MmError<Self::Error>>;

    /// Gets transaction hexes from the wallet's cache.
    async fn tx_bytes_from_cache(
        &self,
        wallet_id: &WalletId,
        tx_hash: &str,
    ) -> Result<Option<BytesJson>, MmError<Self::Error>>;

    /// Gets transaction history for the selected wallet according to the specified `filters`.
    async fn get_history(
        &self,
        wallet_id: &WalletId,
        filters: GetTxHistoryFilters,
        paging: PagingOptionsEnum<BytesJson>,
        limit: usize,
    ) -> Result<GetHistoryResult, MmError<Self::Error>>;
}

pub struct TxDetailsBuilder<'a, Addr: DisplayAddress, Tx: Transaction> {
    coin: String,
    tx: &'a Tx,
    my_addresses: HashSet<Addr>,
    total_amount: BigDecimal,
    received_by_me: BigDecimal,
    spent_by_me: BigDecimal,
    from_addresses: HashSet<Addr>,
    to_addresses: HashSet<Addr>,
    transaction_type: TransactionType,
    block_height_and_time: Option<BlockHeightAndTime>,
    tx_fee: Option<TxFeeDetails>,
}

impl<'a, Addr: Clone + DisplayAddress + Eq + std::hash::Hash, Tx: Transaction> TxDetailsBuilder<'a, Addr, Tx> {
    pub fn new(
        coin: String,
        tx: &'a Tx,
        block_height_and_time: Option<BlockHeightAndTime>,
        my_addresses: impl IntoIterator<Item = Addr>,
    ) -> Self {
        TxDetailsBuilder {
            coin,
            tx,
            my_addresses: my_addresses.into_iter().collect(),
            total_amount: Default::default(),
            received_by_me: Default::default(),
            spent_by_me: Default::default(),
            from_addresses: Default::default(),
            to_addresses: Default::default(),
            block_height_and_time,
            transaction_type: TransactionType::StandardTransfer,
            tx_fee: None,
        }
    }

    pub fn set_tx_fee(&mut self, tx_fee: Option<TxFeeDetails>) {
        self.tx_fee = tx_fee;
    }

    pub fn set_transaction_type(&mut self, tx_type: TransactionType) {
        self.transaction_type = tx_type;
    }

    pub fn transferred_to(&mut self, address: Addr, amount: &BigDecimal) {
        if self.my_addresses.contains(&address) {
            self.received_by_me += amount;
        }
        self.to_addresses.insert(address);
    }

    pub fn transferred_from(&mut self, address: Addr, amount: &BigDecimal) {
        if self.my_addresses.contains(&address) {
            self.spent_by_me += amount;
        }
        self.total_amount += amount;
        self.from_addresses.insert(address);
    }

    /// TODO: This implementation is messy. We should do all the calculations before storing them
    /// to the database. We shouldn’t need these on-demand calculations in this module; it's better
    /// to remove this function entirely but some coins like UTXOs still depend on it.
    pub fn build(self) -> TransactionDetails {
        let (block_height, timestamp) = match self.block_height_and_time {
            Some(height_with_time) => (height_with_time.height, height_with_time.timestamp),
            None => (0, 0),
        };

        let mut from: Vec<_> = self
            .from_addresses
            .iter()
            .map(DisplayAddress::display_address)
            .collect();
        from.sort();

        let mut to: Vec<_> = self.to_addresses.iter().map(DisplayAddress::display_address).collect();
        to.sort();

        let tx_hash = self.tx.tx_hash_as_bytes();
        let internal_id = match &self.transaction_type {
            TransactionType::TokenTransfer(token_id) => {
                let mut bytes_for_hash = tx_hash.0.clone();
                bytes_for_hash.extend_from_slice(&token_id.0);
                sha256(&bytes_for_hash).to_vec().into()
            },
            TransactionType::TendermintIBCTransfer { .. } | TransactionType::CustomTendermintMsg { .. } => {
                unreachable!("Tendermint never invokes this function.")
            },
            TransactionType::StakingDelegation
            | TransactionType::RemoveDelegation
            | TransactionType::ClaimDelegationRewards
            | TransactionType::FeeForTokenTx
            | TransactionType::StandardTransfer
            | TransactionType::NftTransfer => tx_hash.clone(),
        };

        TransactionDetails {
            coin: self.coin,
            tx: TransactionData::new_signed(self.tx.tx_hex().into(), tx_hash.to_tx_hash()),
            from,
            to,
            total_amount: self.total_amount,
            my_balance_change: &self.received_by_me - &self.spent_by_me,
            spent_by_me: self.spent_by_me,
            received_by_me: self.received_by_me,
            block_height,
            timestamp,
            fee_details: self.tx_fee,
            internal_id,
            kmd_rewards: None,
            transaction_type: self.transaction_type,
            memo: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum MyTxHistoryTarget {
    #[default]
    Iguana,
    AccountId {
        account_id: u32,
    },
    AddressId(HDPathAccountToAddressId),
}

#[derive(Clone, Deserialize)]
pub struct MyTxHistoryRequestV2<T> {
    pub(crate) coin: String,
    #[serde(default = "ten")]
    pub(crate) limit: usize,
    #[serde(default)]
    pub(crate) paging_options: PagingOptionsEnum<T>,
    #[serde(default)]
    pub(crate) target: MyTxHistoryTarget,
}

#[derive(Serialize)]
pub struct MyTxHistoryDetails {
    #[serde(flatten)]
    pub(crate) details: TransactionDetails,
    pub(crate) confirmations: u64,
}

#[derive(Serialize)]
pub struct MyTxHistoryResponseV2<Tx, Id> {
    pub(crate) coin: String,
    pub(crate) target: MyTxHistoryTarget,
    pub(crate) current_block: u64,
    pub(crate) transactions: Vec<Tx>,
    pub(crate) sync_status: HistorySyncState,
    pub(crate) limit: usize,
    pub(crate) skipped: usize,
    pub(crate) total: usize,
    pub(crate) total_pages: usize,
    pub(crate) paging_options: PagingOptionsEnum<Id>,
}

#[derive(Debug, Display, EnumFromStringify, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum MyTxHistoryErrorV2 {
    CoinIsNotActive(String),
    #[from_stringify("InvalidBip44ChainError")]
    InvalidTarget(String),
    StorageIsNotInitialized(String),
    #[from_stringify("CreateTxHistoryStorageError")]
    StorageError(String),
    #[from_stringify("UtxoRpcError")]
    RpcError(String),
    NotSupportedFor(String),
    #[from_stringify("MyAddressError")]
    Internal(String),
}

impl MyTxHistoryErrorV2 {
    pub fn with_expected_target(actual: MyTxHistoryTarget, expected: &str) -> MyTxHistoryErrorV2 {
        MyTxHistoryErrorV2::InvalidTarget(format!("Expected {expected:?} target, found: {actual:?}"))
    }
}

impl HttpStatusCode for MyTxHistoryErrorV2 {
    fn status_code(&self) -> StatusCode {
        match self {
            MyTxHistoryErrorV2::CoinIsNotActive(_) => StatusCode::NOT_FOUND,
            MyTxHistoryErrorV2::StorageIsNotInitialized(_)
            | MyTxHistoryErrorV2::StorageError(_)
            | MyTxHistoryErrorV2::RpcError(_)
            | MyTxHistoryErrorV2::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            MyTxHistoryErrorV2::NotSupportedFor(_) | MyTxHistoryErrorV2::InvalidTarget(_) => StatusCode::BAD_REQUEST,
        }
    }
}

impl From<CoinFindError> for MyTxHistoryErrorV2 {
    fn from(err: CoinFindError) -> Self {
        match err {
            CoinFindError::NoSuchCoin { coin } => MyTxHistoryErrorV2::CoinIsNotActive(coin),
        }
    }
}

impl<T: TxHistoryStorageError> From<T> for MyTxHistoryErrorV2 {
    fn from(err: T) -> Self {
        let msg = format!("{err:?}");
        MyTxHistoryErrorV2::StorageError(msg)
    }
}

impl From<AddressDerivingError> for MyTxHistoryErrorV2 {
    fn from(e: AddressDerivingError) -> Self {
        match e {
            AddressDerivingError::InvalidBip44Chain { .. } => MyTxHistoryErrorV2::InvalidTarget(e.to_string()),
            AddressDerivingError::Bip32Error(_) => MyTxHistoryErrorV2::Internal(e.to_string()),
            AddressDerivingError::Internal(internal) => MyTxHistoryErrorV2::Internal(internal),
        }
    }
}

#[async_trait]
pub trait CoinWithTxHistoryV2 {
    fn history_wallet_id(&self) -> WalletId;

    async fn get_tx_history_filters(
        &self,
        target: MyTxHistoryTarget,
    ) -> MmResult<GetTxHistoryFilters, MyTxHistoryErrorV2>;
}

/// According to the [comment](https://github.com/KomodoPlatform/atomicDEX-API/pull/1285#discussion_r888410390),
/// it's worth to add [`MmCoin::my_tx_history_v2`] when most coins support transaction history V2.
pub async fn my_tx_history_v2_rpc(
    ctx: MmArc,
    request: MyTxHistoryRequestV2<BytesJson>,
) -> Result<MyTxHistoryResponseV2<MyTxHistoryDetails, BytesJson>, MmError<MyTxHistoryErrorV2>> {
    match lp_coinfind_or_err(&ctx, &request.coin).await.map_mm_err()? {
        MmCoinEnum::Bch(bch) => my_tx_history_v2_impl(ctx, &bch, request).await,
        MmCoinEnum::SlpToken(slp_token) => my_tx_history_v2_impl(ctx, &slp_token, request).await,
        MmCoinEnum::UtxoCoin(utxo) => my_tx_history_v2_impl(ctx, &utxo, request).await,
        MmCoinEnum::QtumCoin(qtum) => my_tx_history_v2_impl(ctx, &qtum, request).await,
        MmCoinEnum::Tendermint(tendermint) => my_tx_history_v2_impl(ctx, &tendermint, request).await,
        MmCoinEnum::TendermintToken(tendermint_token) => my_tx_history_v2_impl(ctx, &tendermint_token, request).await,
        other => MmError::err(MyTxHistoryErrorV2::NotSupportedFor(other.ticker().to_owned())),
    }
}

pub(crate) async fn my_tx_history_v2_impl<Coin>(
    ctx: MmArc,
    coin: &Coin,
    request: MyTxHistoryRequestV2<BytesJson>,
) -> Result<MyTxHistoryResponseV2<MyTxHistoryDetails, BytesJson>, MmError<MyTxHistoryErrorV2>>
where
    Coin: CoinWithTxHistoryV2 + MmCoin,
{
    let tx_history_storage = TxHistoryStorageBuilder::new(&ctx).build().map_mm_err()?;

    let wallet_id = coin.history_wallet_id();
    let is_storage_init = tx_history_storage.is_initialized_for(&wallet_id).await.map_mm_err()?;
    if !is_storage_init {
        let msg = format!("Storage is not initialized for {wallet_id:?}");
        return MmError::err(MyTxHistoryErrorV2::StorageIsNotInitialized(msg));
    }
    let current_block = coin
        .current_block()
        .compat()
        .await
        .map_to_mm(MyTxHistoryErrorV2::RpcError)?;

    let filters = coin.get_tx_history_filters(request.target.clone()).await?;
    let history = tx_history_storage
        .get_history(&wallet_id, filters, request.paging_options.clone(), request.limit)
        .await
        .map_mm_err()?;

    let coin_conf = coin_conf(&ctx, coin.ticker());
    let protocol_type = coin_conf["protocol"]["type"].as_str().unwrap_or_default();
    let decimals = coin.decimals();

    let transactions = history
        .transactions
        .into_iter()
        .map(|mut details| {
            // TODO
            // !! temporary solution !!
            // for tendermint, tx_history_v2 implementation doesn't include amount parsing logic.
            // therefore, re-mapping is required
            match protocol_type {
                TENDERMINT_COIN_PROTOCOL_TYPE | TENDERMINT_ASSET_PROTOCOL_TYPE => {
                    // TODO
                    // see this https://github.com/KomodoPlatform/atomicDEX-API/pull/1526#discussion_r1037001780
                    if let Some(TxFeeDetails::Utxo(fee)) = &mut details.fee_details {
                        let mapped_fee = crate::tendermint::TendermintFeeDetails {
                            // We make sure this is filled in `tendermint_tx_history_v2`
                            coin: fee.coin.as_ref().expect("can't be empty").to_owned(),
                            amount: fee.amount.clone(),
                            gas_limit: crate::tendermint::GAS_LIMIT_DEFAULT,
                            // ignored anyway
                            uamount: 0,
                        };
                        details.fee_details = Some(TxFeeDetails::Tendermint(mapped_fee));
                    }

                    match &details.transaction_type {
                        // Amount mappings are by-passed when `TransactionType` is `FeeForTokenTx`
                        TransactionType::FeeForTokenTx => {},
                        _ => {
                            // In order to use error result instead of panicking, we should do an extra iteration above this map.
                            // Because all the values are inserted by u64 convertion in tx_history_v2 implementation, using `panic`
                            // shouldn't harm.

                            let u_total_amount = details.total_amount.to_u64().unwrap_or_else(|| {
                                panic!("Parsing '{}' into u64 should not fail", details.total_amount)
                            });
                            details.total_amount = big_decimal_from_sat_unsigned(u_total_amount, decimals);

                            let u_spent_by_me = details.spent_by_me.to_u64().unwrap_or_else(|| {
                                panic!("Parsing '{}' into u64 should not fail", details.spent_by_me)
                            });
                            details.spent_by_me = big_decimal_from_sat_unsigned(u_spent_by_me, decimals);

                            let u_received_by_me = details.received_by_me.to_u64().unwrap_or_else(|| {
                                panic!("Parsing '{}' into u64 should not fail", details.received_by_me)
                            });
                            details.received_by_me = big_decimal_from_sat_unsigned(u_received_by_me, decimals);

                            // Because this can be negative values, no need to read and parse
                            // this since it's always 0 from tx_history_v2 implementation.
                            details.my_balance_change = &details.received_by_me - &details.spent_by_me;
                        },
                    }
                },
                BCH_COIN_PROTOCOL_TYPE | BCH_TOKEN_PROTOCOL_TYPE => {
                    // SLP tokens are part of BCH transactions and SLP transactions might be stored with the BCH ticker.
                    // Ideally, we should avoid this workaround and instead fix the incorrect ticker logic when inserting
                    // transactions with the wrong ticker.
                    //
                    // Original PR: https://github.com/KomodoPlatform/komodo-defi-framework/pull/1175.
                    if details.coin != request.coin {
                        details.coin = request.coin.clone();
                    }
                },
                _ => {},
            };

            let confirmations = if details.block_height > current_block {
                0
            } else {
                current_block + 1 - details.block_height
            };
            MyTxHistoryDetails { confirmations, details }
        })
        .collect();

    Ok(MyTxHistoryResponseV2 {
        coin: request.coin,
        target: request.target,
        current_block,
        transactions,
        sync_status: coin.history_sync_status(),
        limit: request.limit,
        skipped: history.skipped,
        total: history.total,
        total_pages: calc_total_pages(history.total, request.limit),
        paging_options: request.paging_options,
    })
}

pub async fn z_coin_tx_history_rpc(
    ctx: MmArc,
    request: MyTxHistoryRequestV2<i64>,
) -> Result<MyTxHistoryResponseV2<crate::z_coin::ZcoinTxDetails, i64>, MmError<MyTxHistoryErrorV2>> {
    match lp_coinfind_or_err(&ctx, &request.coin).await.map_mm_err()? {
        MmCoinEnum::ZCoin(z_coin) => z_coin.tx_history(request).await,
        other => MmError::err(MyTxHistoryErrorV2::NotSupportedFor(other.ticker().to_owned())),
    }
}

#[cfg(test)]
pub(crate) mod for_tests {
    use super::{CoinWithTxHistoryV2, TxHistoryStorage};
    use crate::tx_history_storage::TxHistoryStorageBuilder;
    use common::block_on;
    use mm2_core::mm_ctx::MmArc;
    use mm2_test_helpers::for_tests::mm_ctx_with_custom_db;

    pub fn init_storage_for<Coin: CoinWithTxHistoryV2>(coin: &Coin) -> (MmArc, impl TxHistoryStorage) {
        let ctx = mm_ctx_with_custom_db();
        let storage = TxHistoryStorageBuilder::new(&ctx).build().unwrap();
        block_on(storage.init(&coin.history_wallet_id())).unwrap();
        (ctx, storage)
    }
}
