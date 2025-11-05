use super::RequestTxHistoryResult;
use crate::hd_wallet::{AddressDerivingError, DisplayAddress};
use crate::my_tx_history_v2::{CoinWithTxHistoryV2, TxHistoryStorage, TxHistoryStorageError};
use crate::tx_history_storage::FilteringAddresses;
use crate::utxo::bch::BchCoin;
use crate::utxo::slp::ParseSlpScriptError;
use crate::utxo::tx_history_events::TxHistoryEventStreamer;
use crate::utxo::{utxo_common, AddrFromStrError, GetBlockHeaderError};
use crate::{
    BalanceError, BalanceResult, BlockHeightAndTime, CoinWithDerivationMethod, HistorySyncState, MarketCoinOps, MmCoin,
    NumConversError, ParseBigDecimalError, TransactionDetails, UnexpectedDerivationMethod, UtxoRpcError, UtxoTx,
};
use async_trait::async_trait;
use common::executor::Timer;
use common::log::{error, info};
use derive_more::Display;
use keys::Address;
use mm2_err_handle::prelude::*;
use mm2_event_stream::{DeriveStreamerId, StreamingManager};
use mm2_metrics::MetricsArc;
use mm2_number::BigDecimal;
use mm2_state_machine::prelude::*;
use mm2_state_machine::state_machine::StateMachineTrait;
use rpc::v1::types::H256 as H256Json;
use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::convert::Infallible;
use std::iter::FromIterator;
use std::str::FromStr;

macro_rules! try_or_stop_unknown {
    ($exp:expr, $fmt:literal) => {
        match $exp {
            Ok(t) => t,
            Err(e) => return Self::change_state(Stopped::unknown(format!("{}: {}", $fmt, e))),
        }
    };
}

#[derive(Debug, Display)]
pub enum UtxoMyAddressesHistoryError {
    AddressDerivingError(AddressDerivingError),
    UnexpectedDerivationMethod(UnexpectedDerivationMethod),
}

impl From<AddressDerivingError> for UtxoMyAddressesHistoryError {
    fn from(e: AddressDerivingError) -> Self {
        UtxoMyAddressesHistoryError::AddressDerivingError(e)
    }
}

impl From<UnexpectedDerivationMethod> for UtxoMyAddressesHistoryError {
    fn from(e: UnexpectedDerivationMethod) -> Self {
        UtxoMyAddressesHistoryError::UnexpectedDerivationMethod(e)
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Display)]
pub enum UtxoTxDetailsError {
    #[display(fmt = "Storage error: {_0}")]
    StorageError(String),
    #[display(fmt = "Transaction deserialization error: {_0}")]
    TxDeserializationError(serialization::Error),
    #[display(fmt = "Invalid transaction: {_0}")]
    InvalidTransaction(String),
    #[display(fmt = "TX Address deserialization error: {_0}")]
    TxAddressDeserializationError(String),
    #[display(fmt = "{_0}")]
    NumConversionErr(NumConversError),
    #[display(fmt = "RPC error: {_0}")]
    RpcError(UtxoRpcError),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl From<serialization::Error> for UtxoTxDetailsError {
    fn from(e: serialization::Error) -> Self {
        UtxoTxDetailsError::TxDeserializationError(e)
    }
}

impl From<UtxoRpcError> for UtxoTxDetailsError {
    fn from(e: UtxoRpcError) -> Self {
        UtxoTxDetailsError::RpcError(e)
    }
}

impl From<NumConversError> for UtxoTxDetailsError {
    fn from(e: NumConversError) -> Self {
        UtxoTxDetailsError::NumConversionErr(e)
    }
}

impl From<ParseBigDecimalError> for UtxoTxDetailsError {
    fn from(e: ParseBigDecimalError) -> Self {
        UtxoTxDetailsError::from(NumConversError::from(e))
    }
}

impl From<ParseSlpScriptError> for UtxoTxDetailsError {
    fn from(err: ParseSlpScriptError) -> Self {
        UtxoTxDetailsError::InvalidTransaction(format!("Error parsing SLP script: {err}"))
    }
}

impl<StorageErr> From<StorageErr> for UtxoTxDetailsError
where
    StorageErr: TxHistoryStorageError,
{
    fn from(e: StorageErr) -> Self {
        UtxoTxDetailsError::StorageError(format!("{e:?}"))
    }
}

pub struct UtxoTxDetailsParams<'a, Storage> {
    pub hash: &'a H256Json,
    pub block_height_and_time: Option<BlockHeightAndTime>,
    pub storage: &'a Storage,
    pub my_addresses: &'a HashSet<Address>,
}

#[async_trait]
pub trait UtxoTxHistoryOps:
    CoinWithTxHistoryV2 + CoinWithDerivationMethod + MarketCoinOps + MmCoin + Send + Sync + 'static
{
    /// Returns addresses for those we need to request Transaction history.
    async fn my_addresses(&self) -> MmResult<HashSet<Address>, UtxoMyAddressesHistoryError>;

    /// Returns Transaction details by hash using the coin RPC if required.
    async fn tx_details_by_hash<T>(
        &self,
        params: UtxoTxDetailsParams<'_, T>,
    ) -> MmResult<Vec<TransactionDetails>, UtxoTxDetailsError>
    where
        T: TxHistoryStorage;

    /// Loads transaction from `storage` or requests it using coin RPC.
    async fn tx_from_storage_or_rpc<Storage: TxHistoryStorage>(
        &self,
        tx_hash: &H256Json,
        storage: &Storage,
    ) -> MmResult<UtxoTx, UtxoTxDetailsError>;

    /// Requests transaction history.
    async fn request_tx_history(&self, metrics: MetricsArc, for_addresses: &HashSet<Address>)
        -> RequestTxHistoryResult;

    /// Requests timestamp of the given block.
    async fn get_block_timestamp(&self, height: u64) -> MmResult<u64, GetBlockHeaderError>;

    /// Requests balances of all activated coin's addresses.
    async fn my_addresses_balances(&self) -> BalanceResult<HashMap<String, BigDecimal>>;

    fn address_from_str(&self, address: &str) -> MmResult<Address, AddrFromStrError>;

    /// Sets the history sync state.
    fn set_history_sync_state(&self, new_state: HistorySyncState);
}

struct UtxoTxHistoryStateMachine<Coin: UtxoTxHistoryOps, Storage: TxHistoryStorage> {
    coin: Coin,
    storage: Storage,
    metrics: MetricsArc,
    /// An instance of the streaming manager used for sending TX updates in realtime.
    streaming_manager: StreamingManager,
    /// Last requested balances of the activated coin's addresses.
    /// TODO add a `CoinBalanceState` structure and replace [`HashMap<String, BigDecimal>`] everywhere.
    balances: HashMap<String, BigDecimal>,
}

impl<Coin: UtxoTxHistoryOps, Storage: TxHistoryStorage> StateMachineTrait for UtxoTxHistoryStateMachine<Coin, Storage> {
    type Result = ();
    type Error = Infallible;
}

impl<Coin: UtxoTxHistoryOps, Storage: TxHistoryStorage> StandardStateMachine
    for UtxoTxHistoryStateMachine<Coin, Storage>
{
}

impl<Coin, Storage> UtxoTxHistoryStateMachine<Coin, Storage>
where
    Coin: UtxoTxHistoryOps,
    Storage: TxHistoryStorage,
{
    /// Requests balances for every activated address, updates the balances in [`UtxoTxHistoryStateMachine::balances`]
    /// and returns the addresses whose balance has changed.
    ///
    /// # Note
    ///
    /// [`UtxoTxHistoryStateMachine::balances`] is changed if we successfully handled all balances **only**.
    async fn updated_addresses(&mut self) -> BalanceResult<HashSet<Address>> {
        let current_balances = self.coin.my_addresses_balances().await?;

        // Create a copy of the CTX balances state.
        // We must not to save any change of `ctx.balances` if an error occurs while processing `current_balances` collection.
        let mut ctx_balances = self.balances.clone();

        let mut updated_addresses = HashSet::with_capacity(ctx_balances.len());
        for (address, current_balance) in current_balances {
            let updated_address = match ctx_balances.entry(address.clone()) {
                // Do nothing if the balance hasn't been changed.
                Entry::Occupied(entry) if *entry.get() == current_balance => continue,
                Entry::Occupied(mut entry) => {
                    entry.insert(current_balance);
                    address
                },
                Entry::Vacant(entry) => {
                    entry.insert(current_balance);
                    address
                },
            };

            // Currently, it's easier to convert `Address` from stringified address
            // than to refactor `CoinBalanceReport` by replacing stringified addresses with a type parameter.
            // Such refactoring will lead to huge code changes, complex and nested trait bounds.
            // I personally think that it's overhead since, a least for now,
            // we need to parse `CoinBalanceReport` within the transaction history only.
            match self.coin.address_from_str(&updated_address) {
                Ok(addr) => updated_addresses.insert(addr),
                Err(e) => {
                    let (kind, trace) = e.split();
                    let error =
                        format!("Error on converting address from 'UtxoTxHistoryOps::my_addresses_balances': {kind}");
                    return MmError::err_with_trace(BalanceError::Internal(error), trace);
                },
            };
        }

        // Save the changes in the context.
        self.balances = ctx_balances;

        Ok(updated_addresses)
    }
}

// States have to be generic over storage type because BchAndSlpHistoryCtx is generic over it
struct Init<Coin, Storage> {
    phantom: std::marker::PhantomData<(Coin, Storage)>,
}

impl<Coin, Storage> Init<Coin, Storage> {
    fn new() -> Self {
        Init {
            phantom: Default::default(),
        }
    }
}

impl<Coin, Storage> TransitionFrom<Init<Coin, Storage>> for Stopped<Coin, Storage> {}

#[async_trait]
impl<Coin, Storage> State for Init<Coin, Storage>
where
    Coin: UtxoTxHistoryOps,
    Storage: TxHistoryStorage,
{
    type StateMachine = UtxoTxHistoryStateMachine<Coin, Storage>;

    async fn on_changed(
        self: Box<Self>,
        ctx: &mut UtxoTxHistoryStateMachine<Coin, Storage>,
    ) -> StateResult<UtxoTxHistoryStateMachine<Coin, Storage>> {
        ctx.coin.set_history_sync_state(HistorySyncState::NotStarted);

        if let Err(e) = ctx.storage.init(&ctx.coin.history_wallet_id()).await {
            return Self::change_state(Stopped::storage_error(e));
        }

        Self::change_state(FetchingTxHashes::for_all_addresses())
    }
}

// States have to be generic over storage type because BchAndSlpHistoryCtx is generic over it
struct FetchingTxHashes<Coin, Storage> {
    fetch_for_addresses: Option<HashSet<Address>>,
    phantom: std::marker::PhantomData<(Coin, Storage)>,
}

impl<Coin, Storage> FetchingTxHashes<Coin, Storage> {
    fn for_all_addresses() -> Self {
        FetchingTxHashes {
            fetch_for_addresses: None,
            phantom: Default::default(),
        }
    }

    fn for_addresses(fetch_for_addresses: HashSet<Address>) -> Self {
        FetchingTxHashes {
            fetch_for_addresses: Some(fetch_for_addresses),
            phantom: Default::default(),
        }
    }
}

impl<Coin, Storage> TransitionFrom<Init<Coin, Storage>> for FetchingTxHashes<Coin, Storage> {}
impl<Coin, Storage> TransitionFrom<OnIoErrorCooldown<Coin, Storage>> for FetchingTxHashes<Coin, Storage> {}
impl<Coin, Storage> TransitionFrom<WaitForHistoryUpdateTrigger<Coin, Storage>> for FetchingTxHashes<Coin, Storage> {}

#[async_trait]
impl<Coin, Storage> State for FetchingTxHashes<Coin, Storage>
where
    Coin: UtxoTxHistoryOps,
    Storage: TxHistoryStorage,
{
    type StateMachine = UtxoTxHistoryStateMachine<Coin, Storage>;

    async fn on_changed(
        self: Box<Self>,
        ctx: &mut UtxoTxHistoryStateMachine<Coin, Storage>,
    ) -> StateResult<UtxoTxHistoryStateMachine<Coin, Storage>> {
        let wallet_id = ctx.coin.history_wallet_id();
        if let Err(e) = ctx.storage.init(&wallet_id).await {
            return Self::change_state(Stopped::storage_error(e));
        }

        let fetch_for_addresses = match self.fetch_for_addresses {
            Some(for_addresses) => for_addresses,
            // `fetch_for_addresses` hasn't been specified. Fetch TX hashses for all addresses.
            None => try_or_stop_unknown!(ctx.coin.my_addresses().await, "Error on getting my addresses"),
        };

        let maybe_tx_ids = ctx
            .coin
            .request_tx_history(ctx.metrics.clone(), &fetch_for_addresses)
            .await;
        match maybe_tx_ids {
            RequestTxHistoryResult::Ok(all_tx_ids_with_height) => {
                let filtering_addresses =
                    FilteringAddresses::from_iter(fetch_for_addresses.iter().map(DisplayAddress::display_address));

                let in_storage = match ctx
                    .storage
                    .unique_tx_hashes_num_in_history(&wallet_id, filtering_addresses)
                    .await
                {
                    Ok(num) => num,
                    Err(e) => return Self::change_state(Stopped::storage_error(e)),
                };
                if all_tx_ids_with_height.len() > in_storage {
                    let txes_left = all_tx_ids_with_height.len() - in_storage;
                    let new_state_json = json!({ "transactions_left": txes_left });
                    ctx.coin
                        .set_history_sync_state(HistorySyncState::InProgress(new_state_json));
                }

                Self::change_state(UpdatingUnconfirmedTxes::new(
                    fetch_for_addresses,
                    all_tx_ids_with_height,
                ))
            },
            RequestTxHistoryResult::HistoryTooLarge => Self::change_state(Stopped::history_too_large()),
            RequestTxHistoryResult::Retry { error } => {
                error!("Error {} on requesting tx history for {}", error, ctx.coin.ticker());
                Self::change_state(OnIoErrorCooldown::new(fetch_for_addresses))
            },
            RequestTxHistoryResult::CriticalError(e) => {
                error!(
                    "Critical error {} on requesting tx history for {}",
                    e,
                    ctx.coin.ticker()
                );
                Self::change_state(Stopped::unknown(e))
            },
        }
    }
}

/// An I/O cooldown before `FetchingTxHashes` state.
/// States have to be generic over storage type because `UtxoTxHistoryStateMachine` is generic over it.
struct OnIoErrorCooldown<Coin, Storage> {
    /// The list of addresses of those we need to fetch TX hashes at the upcoming `FetchingTxHashses` state.
    fetch_for_addresses: HashSet<Address>,
    phantom: std::marker::PhantomData<(Coin, Storage)>,
}

impl<Coin, Storage> OnIoErrorCooldown<Coin, Storage> {
    fn new(fetch_for_addresses: HashSet<Address>) -> Self {
        OnIoErrorCooldown {
            fetch_for_addresses,
            phantom: Default::default(),
        }
    }
}

impl<Coin, Storage> TransitionFrom<FetchingTxHashes<Coin, Storage>> for OnIoErrorCooldown<Coin, Storage> {}
impl<Coin, Storage> TransitionFrom<FetchingTransactionsData<Coin, Storage>> for OnIoErrorCooldown<Coin, Storage> {}
impl<Coin, Storage> TransitionFrom<UpdatingUnconfirmedTxes<Coin, Storage>> for OnIoErrorCooldown<Coin, Storage> {}

#[async_trait]
impl<Coin, Storage> State for OnIoErrorCooldown<Coin, Storage>
where
    Coin: UtxoTxHistoryOps,
    Storage: TxHistoryStorage,
{
    type StateMachine = UtxoTxHistoryStateMachine<Coin, Storage>;

    async fn on_changed(
        mut self: Box<Self>,
        ctx: &mut UtxoTxHistoryStateMachine<Coin, Storage>,
    ) -> StateResult<UtxoTxHistoryStateMachine<Coin, Storage>> {
        loop {
            Timer::sleep(30.).await;

            // We need to check whose balance has changed in these 30 seconds.
            let updated_addresses = match ctx.updated_addresses().await {
                Ok(updated) => updated,
                Err(e) => {
                    error!("Error {e:?} on balance fetching for the coin {}", ctx.coin.ticker());
                    continue;
                },
            };

            // We still need to fetch TX hashes for [`OnIoErrorCooldown::fetch_for_addresses`],
            // but now we also need to fetch TX hashes for new `updated_addresses`.
            // Merge these two containers.
            self.fetch_for_addresses.extend(updated_addresses);

            return Self::change_state(FetchingTxHashes::for_addresses(self.fetch_for_addresses));
        }
    }
}

// States have to be generic over storage type because BchAndSlpHistoryCtx is generic over it
struct WaitForHistoryUpdateTrigger<Coin, Storage> {
    phantom: std::marker::PhantomData<(Coin, Storage)>,
}

impl<Coin, Storage> WaitForHistoryUpdateTrigger<Coin, Storage> {
    fn new() -> Self {
        WaitForHistoryUpdateTrigger {
            phantom: Default::default(),
        }
    }
}

impl<Coin, Storage> TransitionFrom<FetchingTransactionsData<Coin, Storage>>
    for WaitForHistoryUpdateTrigger<Coin, Storage>
{
}

#[async_trait]
impl<Coin, Storage> State for WaitForHistoryUpdateTrigger<Coin, Storage>
where
    Coin: UtxoTxHistoryOps,
    Storage: TxHistoryStorage,
{
    type StateMachine = UtxoTxHistoryStateMachine<Coin, Storage>;

    async fn on_changed(
        self: Box<Self>,
        ctx: &mut UtxoTxHistoryStateMachine<Coin, Storage>,
    ) -> StateResult<UtxoTxHistoryStateMachine<Coin, Storage>> {
        let wallet_id = ctx.coin.history_wallet_id();
        loop {
            Timer::sleep(30.).await;

            let my_addresses = try_or_stop_unknown!(ctx.coin.my_addresses().await, "Error on getting my addresses");
            let for_addresses = to_filtering_addresses(&my_addresses);

            match ctx
                .storage
                .history_contains_unconfirmed_txes(&wallet_id, for_addresses)
                .await
            {
                // Fetch TX hashses for all addresses.
                Ok(true) => return Self::change_state(FetchingTxHashes::for_addresses(my_addresses)),
                Ok(false) => (),
                Err(e) => return Self::change_state(Stopped::storage_error(e)),
            }

            let updated_addresses = match ctx.updated_addresses().await {
                Ok(updated) => updated,
                Err(e) => {
                    error!("Error {e:?} on balance fetching for the coin {}", ctx.coin.ticker());
                    continue;
                },
            };

            if !updated_addresses.is_empty() {
                // Fetch TX hashes for those addresses whose balance has changed only.
                return Self::change_state(FetchingTxHashes::for_addresses(updated_addresses));
            }
        }
    }
}

// States have to be generic over storage type because BchAndSlpHistoryCtx is generic over it
struct UpdatingUnconfirmedTxes<Coin, Storage> {
    /// The list of addresses for those we have requested [`UpdatingUnconfirmedTxes::all_tx_ids_with_height`] TX hashses
    /// at the `FetchingTxHashes` state.
    requested_for_addresses: HashSet<Address>,
    all_tx_ids_with_height: Vec<(H256Json, u64)>,
    phantom: std::marker::PhantomData<(Coin, Storage)>,
}

impl<Coin, Storage> UpdatingUnconfirmedTxes<Coin, Storage> {
    fn new(requested_for_addresses: HashSet<Address>, all_tx_ids_with_height: Vec<(H256Json, u64)>) -> Self {
        UpdatingUnconfirmedTxes {
            requested_for_addresses,
            all_tx_ids_with_height,
            phantom: Default::default(),
        }
    }
}

impl<Coin, Storage> TransitionFrom<FetchingTxHashes<Coin, Storage>> for UpdatingUnconfirmedTxes<Coin, Storage> {}

#[async_trait]
impl<Coin, Storage> State for UpdatingUnconfirmedTxes<Coin, Storage>
where
    Coin: UtxoTxHistoryOps,
    Storage: TxHistoryStorage,
{
    type StateMachine = UtxoTxHistoryStateMachine<Coin, Storage>;

    async fn on_changed(
        self: Box<Self>,
        ctx: &mut UtxoTxHistoryStateMachine<Coin, Storage>,
    ) -> StateResult<UtxoTxHistoryStateMachine<Coin, Storage>> {
        let wallet_id = ctx.coin.history_wallet_id();

        let for_addresses = to_filtering_addresses(&self.requested_for_addresses);
        let unconfirmed = match ctx
            .storage
            .get_unconfirmed_txes_from_history(&wallet_id, for_addresses)
            .await
        {
            Ok(unconfirmed) => unconfirmed,
            Err(e) => return Self::change_state(Stopped::storage_error(e)),
        };

        let txs_with_height: HashMap<H256Json, u64> = self.all_tx_ids_with_height.clone().into_iter().collect();
        for mut tx in unconfirmed {
            let Some(tx_hash) = tx.tx.tx_hash() else { continue };

            let found = match H256Json::from_str(tx_hash) {
                Ok(unconfirmed_tx_hash) => txs_with_height.get(&unconfirmed_tx_hash),
                Err(_) => None,
            };

            match found {
                Some(height) => {
                    if *height > 0 {
                        match ctx.coin.get_block_timestamp(*height).await {
                            Ok(time) => tx.timestamp = time,
                            Err(_) => return Self::change_state(OnIoErrorCooldown::new(self.requested_for_addresses)),
                        };
                        tx.block_height = *height;
                        if let Err(e) = ctx.storage.update_tx_in_history(&wallet_id, &tx).await {
                            return Self::change_state(Stopped::storage_error(e));
                        }
                    }
                },
                None => {
                    // This can potentially happen when unconfirmed tx is removed from mempool for some reason.
                    // Or if the hash is undecodable. We should remove it from storage too.
                    if let Err(e) = ctx.storage.remove_tx_from_history(&wallet_id, &tx.internal_id).await {
                        return Self::change_state(Stopped::storage_error(e));
                    }
                },
            }
        }

        Self::change_state(FetchingTransactionsData::new(
            self.requested_for_addresses,
            self.all_tx_ids_with_height,
        ))
    }
}

// States have to be generic over storage type because BchAndSlpHistoryCtx is generic over it
struct FetchingTransactionsData<Coin, Storage> {
    /// The list of addresses for those we have requested [`UpdatingUnconfirmedTxes::all_tx_ids_with_height`] TX hashses
    /// at the `FetchingTxHashes` state.
    requested_for_addresses: HashSet<Address>,
    all_tx_ids_with_height: Vec<(H256Json, u64)>,
    phantom: std::marker::PhantomData<(Coin, Storage)>,
}

impl<Coin, Storage> TransitionFrom<UpdatingUnconfirmedTxes<Coin, Storage>> for FetchingTransactionsData<Coin, Storage> {}

impl<Coin, Storage> FetchingTransactionsData<Coin, Storage> {
    fn new(requested_for_addresses: HashSet<Address>, all_tx_ids_with_height: Vec<(H256Json, u64)>) -> Self {
        FetchingTransactionsData {
            requested_for_addresses,
            all_tx_ids_with_height,
            phantom: Default::default(),
        }
    }
}

#[async_trait]
impl<Coin, Storage> State for FetchingTransactionsData<Coin, Storage>
where
    Coin: UtxoTxHistoryOps,
    Storage: TxHistoryStorage,
{
    type StateMachine = UtxoTxHistoryStateMachine<Coin, Storage>;

    async fn on_changed(
        self: Box<Self>,
        ctx: &mut UtxoTxHistoryStateMachine<Coin, Storage>,
    ) -> StateResult<UtxoTxHistoryStateMachine<Coin, Storage>> {
        let ticker = ctx.coin.ticker();
        let wallet_id = ctx.coin.history_wallet_id();

        let my_addresses = try_or_stop_unknown!(ctx.coin.my_addresses().await, "Error on getting my addresses");

        for (tx_hash, height) in self.all_tx_ids_with_height {
            let tx_hash_string = format!("{tx_hash:02x}");
            match ctx.storage.history_has_tx_hash(&wallet_id, &tx_hash_string).await {
                Ok(true) => continue,
                Ok(false) => (),
                Err(e) => return Self::change_state(Stopped::storage_error(e)),
            }

            let block_height_and_time = if height > 0 {
                let timestamp = match ctx.coin.get_block_timestamp(height).await {
                    Ok(time) => time,
                    Err(_) => return Self::change_state(OnIoErrorCooldown::new(self.requested_for_addresses)),
                };
                Some(BlockHeightAndTime { height, timestamp })
            } else {
                None
            };
            let params = UtxoTxDetailsParams {
                hash: &tx_hash,
                block_height_and_time,
                storage: &ctx.storage,
                my_addresses: &my_addresses,
            };
            let tx_details = match ctx.coin.tx_details_by_hash(params).await {
                Ok(tx) => tx,
                Err(e) => {
                    error!("Error on getting {ticker} tx details for hash {tx_hash:02x}: {e}");
                    return Self::change_state(OnIoErrorCooldown::new(self.requested_for_addresses));
                },
            };

            ctx.streaming_manager
                .send_fn(&TxHistoryEventStreamer::derive_streamer_id(ctx.coin.ticker()), || {
                    tx_details.clone()
                })
                .ok();

            if let Err(e) = ctx.storage.add_transactions_to_history(&wallet_id, tx_details).await {
                return Self::change_state(Stopped::storage_error(e));
            }

            // wait for for one second to reduce the number of requests to electrum servers
            Timer::sleep(1.).await;
        }
        info!("Tx history fetching finished for {ticker}");
        ctx.coin.set_history_sync_state(HistorySyncState::Finished);
        Self::change_state(WaitForHistoryUpdateTrigger::new())
    }
}

#[expect(dead_code)]
#[derive(Debug)]
enum StopReason {
    HistoryTooLarge,
    StorageError(String),
    UnknownError(String),
}

struct Stopped<Coin, Storage> {
    phantom: std::marker::PhantomData<(Coin, Storage)>,
    stop_reason: StopReason,
}

impl<Coin, Storage> Stopped<Coin, Storage> {
    fn history_too_large() -> Self {
        Stopped {
            phantom: Default::default(),
            stop_reason: StopReason::HistoryTooLarge,
        }
    }

    fn storage_error<E>(e: E) -> Self
    where
        E: std::fmt::Debug,
    {
        Stopped {
            phantom: Default::default(),
            stop_reason: StopReason::StorageError(format!("{e:?}")),
        }
    }

    fn unknown(e: String) -> Self {
        Stopped {
            phantom: Default::default(),
            stop_reason: StopReason::UnknownError(e),
        }
    }
}

impl<Coin, Storage> TransitionFrom<FetchingTxHashes<Coin, Storage>> for Stopped<Coin, Storage> {}
impl<Coin, Storage> TransitionFrom<UpdatingUnconfirmedTxes<Coin, Storage>> for Stopped<Coin, Storage> {}
impl<Coin, Storage> TransitionFrom<WaitForHistoryUpdateTrigger<Coin, Storage>> for Stopped<Coin, Storage> {}
impl<Coin, Storage> TransitionFrom<FetchingTransactionsData<Coin, Storage>> for Stopped<Coin, Storage> {}

#[async_trait]
impl<Coin, Storage> LastState for Stopped<Coin, Storage>
where
    Coin: UtxoTxHistoryOps,
    Storage: TxHistoryStorage,
{
    type StateMachine = UtxoTxHistoryStateMachine<Coin, Storage>;

    async fn on_changed(self: Box<Self>, ctx: &mut UtxoTxHistoryStateMachine<Coin, Storage>) -> () {
        info!(
            "Stopping tx history fetching for {}. Reason: {:?}",
            ctx.coin.ticker(),
            self.stop_reason
        );
        let new_state_json = match self.stop_reason {
            StopReason::HistoryTooLarge => json!({
                "code": utxo_common::HISTORY_TOO_LARGE_ERR_CODE,
                "message": "Got `history too large` error from Electrum server. History is not available",
            }),
            reason => json!({
                "message": format!("{:?}", reason),
            }),
        };
        ctx.coin.set_history_sync_state(HistorySyncState::Error(new_state_json));
    }
}

pub async fn bch_and_slp_history_loop(
    coin: BchCoin,
    storage: impl TxHistoryStorage,
    metrics: MetricsArc,
    streaming_manager: StreamingManager,
    current_balance: Option<BigDecimal>,
) {
    let balances = match current_balance {
        Some(current_balance) => {
            let my_address = match coin.my_address() {
                Ok(my_address) => my_address,
                Err(e) => {
                    error!("{}", e);
                    return;
                },
            };
            HashMap::from([(my_address, current_balance)])
        },
        None => {
            let ticker = coin.ticker().to_string();
            let addr_bal = retry_on_err!(async { coin.my_addresses_balances().await })
                .until_ready()
                .repeat_every_secs(30.)
                .inspect_err(move |e| {
                    error!("Error {e:?} on balance fetching for the coin {}", ticker);
                })
                .await;

            match addr_bal {
                Ok(addresses_balances) => addresses_balances,
                Err(e) => {
                    error!("{}", e);
                    return;
                },
            }
        },
    };

    let mut state_machine = UtxoTxHistoryStateMachine {
        coin,
        storage,
        metrics,
        streaming_manager,
        balances,
    };
    state_machine
        .run(Box::new(Init::new()))
        .await
        .expect("The error of this machine is Infallible");
}

pub async fn utxo_history_loop<Coin, Storage>(
    coin: Coin,
    storage: Storage,
    metrics: MetricsArc,
    streaming_manager: StreamingManager,
    current_balances: HashMap<String, BigDecimal>,
) where
    Coin: UtxoTxHistoryOps,
    Storage: TxHistoryStorage,
{
    let mut state_machine = UtxoTxHistoryStateMachine {
        coin,
        storage,
        metrics,
        streaming_manager,
        balances: current_balances,
    };
    state_machine
        .run(Box::new(Init::new()))
        .await
        .expect("The error of this machine is Infallible");
}

fn to_filtering_addresses(addresses: &HashSet<Address>) -> FilteringAddresses {
    FilteringAddresses::from_iter(addresses.iter().map(DisplayAddress::display_address))
}
