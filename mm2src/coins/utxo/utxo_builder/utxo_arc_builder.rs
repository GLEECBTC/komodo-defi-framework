use crate::hd_wallet::HDWalletOps;
use crate::utxo::rpc_clients::{
    ElectrumClient, ElectrumClientImpl, UnspentInfo, UtxoJsonRpcClientInfo, UtxoRpcClientEnum,
};

use crate::utxo::utxo_block_header_storage::BlockHeaderStorage;
use crate::utxo::utxo_builder::{UtxoCoinBuildError, UtxoCoinBuilder, UtxoCoinBuilderCommonOps};
use crate::utxo::{
    generate_and_send_tx, generate_tx, output_script, FeePolicy, GetUtxoListOps, UtxoArc, UtxoCommonOps,
    UtxoSyncStatusLoopHandle, UtxoWeak,
};
use crate::{DerivationMethod, PrivKeyBuildPolicy, UtxoActivationParams};
use async_trait::async_trait;
use chain::{BlockHeader, Transaction, TransactionOutput};
use common::executor::{AbortSettings, SpawnAbortable, Timer};
use common::log::{debug, error, info};
use derive_more::Display;
use futures::compat::Future01CompatExt;
use keys::Address;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
#[cfg(test)]
use mocktopus::macros::*;
use rand::Rng;
use script::Script;
use serde_json::Value as Json;
use serialization::Reader;
use spv_validation::conf::SPVConf;
use spv_validation::helpers_validation::{validate_headers, SPVError};
use spv_validation::storage::{BlockHeaderStorageError, BlockHeaderStorageOps};
use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::{Arc, Weak};

const CHUNK_SIZE_REDUCER_VALUE: u64 = 100;
const TRY_TO_RETRIEVE_HEADERS_ATTEMPTS: u8 = 10;

pub struct UtxoArcBuilder<'a, F, T>
where
    F: Fn(UtxoArc) -> T + Send + Sync + 'static,
{
    ctx: &'a MmArc,
    ticker: &'a str,
    conf: &'a Json,
    activation_params: &'a UtxoActivationParams,
    priv_key_policy: PrivKeyBuildPolicy,
    constructor: F,
}

impl<'a, F, T> UtxoArcBuilder<'a, F, T>
where
    F: Fn(UtxoArc) -> T + Send + Sync + 'static,
{
    pub fn new(
        ctx: &'a MmArc,
        ticker: &'a str,
        conf: &'a Json,
        activation_params: &'a UtxoActivationParams,
        priv_key_policy: PrivKeyBuildPolicy,
        constructor: F,
    ) -> UtxoArcBuilder<'a, F, T> {
        UtxoArcBuilder {
            ctx,
            ticker,
            conf,
            activation_params,
            priv_key_policy,
            constructor,
        }
    }
}

#[async_trait]
impl<F, T> UtxoCoinBuilderCommonOps for UtxoArcBuilder<'_, F, T>
where
    F: Fn(UtxoArc) -> T + Send + Sync + 'static,
{
    fn ctx(&self) -> &MmArc {
        self.ctx
    }

    fn conf(&self) -> &Json {
        self.conf
    }

    fn activation_params(&self) -> &UtxoActivationParams {
        self.activation_params
    }

    fn ticker(&self) -> &str {
        self.ticker
    }
}

#[async_trait]
impl<F, T> UtxoCoinBuilder for UtxoArcBuilder<'_, F, T>
where
    F: Fn(UtxoArc) -> T + Clone + Send + Sync + 'static,
    T: UtxoCommonOps + GetUtxoListOps,
{
    type ResultCoin = T;
    type Error = UtxoCoinBuildError;

    fn priv_key_policy(&self) -> PrivKeyBuildPolicy {
        self.priv_key_policy.clone()
    }

    async fn build(self) -> MmResult<Self::ResultCoin, Self::Error> {
        let utxo = self.build_utxo_fields().await?;
        let sync_status_loop_handle = utxo.block_headers_status_notifier.clone();
        let spv_conf = utxo.conf.spv_conf.clone();
        let utxo_arc = UtxoArc::new(utxo);

        self.spawn_merge_utxo_loop_if_required(&utxo_arc, self.constructor.clone());

        let result_coin = (self.constructor)(utxo_arc.clone());

        if let (Some(spv_conf), Some(sync_handle)) = (spv_conf, sync_status_loop_handle) {
            spv_conf.validate(self.ticker).map_to_mm(UtxoCoinBuildError::SPVError)?;
            spawn_block_header_utxo_loop(self.ticker, &utxo_arc, sync_handle, spv_conf);
        }

        Ok(result_coin)
    }
}

impl<F, T> MergeUtxoArcOps<T> for UtxoArcBuilder<'_, F, T>
where
    F: Fn(UtxoArc) -> T + Send + Sync + 'static,
    T: UtxoCommonOps + GetUtxoListOps,
{
}

#[derive(Deserialize)]
#[serde(default)]
pub struct MergeConditions {
    /// The minimum number of UTXOs to merge. If the number of UTXOs is less than this, the merge will not be performed.
    pub merge_at: usize,
    /// The maximum number of UTXOs to merge at once in a single transaction.
    pub max_merge_at_once: usize,
}

impl Default for MergeConditions {
    fn default() -> Self {
        MergeConditions {
            merge_at: 50,
            max_merge_at_once: 50,
        }
    }
}

pub enum UtxoMergeError {
    BadMergeConditions(String),
    InternalError(String),
}

/// Merges unspent UTXOs from `from_address` address to `to_script_pubkey` script.
pub async fn merge_utxos<Coin>(
    coin: &Coin,
    from_address: &Address,
    to_script_pubkey: &Script,
    merge_conditions: &MergeConditions,
    broadcast: bool,
) -> MmResult<(Transaction, Vec<UnspentInfo>), UtxoMergeError>
where
    Coin: UtxoCommonOps + GetUtxoListOps,
{
    let ticker = &coin.as_ref().conf.ticker;
    let (unspents, recently_spent) = coin.get_unspent_ordered_list(from_address).await.mm_err(|e| {
        UtxoMergeError::InternalError(format!("Error in get_unspent_ordered_list for coin={ticker}: {e}"))
    })?;

    if unspents.len() < merge_conditions.merge_at {
        return Err(UtxoMergeError::BadMergeConditions(format!(
            "Not enough unspent UTXOs to merge for coin={ticker}, found={}, required={}",
            unspents.len(),
            merge_conditions.merge_at
        ))
        .into());
    }
    let unspents: Vec<_> = unspents.into_iter().take(merge_conditions.max_merge_at_once).collect();
    if unspents.len() < 2 {
        return Err(UtxoMergeError::BadMergeConditions(format!(
            "No point of merging only a single UTXO (coin={ticker})"
        ))
        .into());
    }

    let value = unspents.iter().fold(0, |sum, unspent| sum + unspent.value);
    let output = TransactionOutput {
        value,
        script_pubkey: to_script_pubkey.to_bytes(),
    };

    let tx = if broadcast {
        generate_and_send_tx(
            coin,
            unspents.clone(),
            None,
            FeePolicy::DeductFromOutput(0),
            recently_spent,
            vec![output],
        )
        .await
        .map_to_mm(|e| UtxoMergeError::InternalError(format!("Error in generate_and_send_tx for coin={ticker}: {e}")))?
    } else {
        let (tx, _) = generate_tx(
            coin,
            unspents.clone(),
            None,
            FeePolicy::DeductFromOutput(0),
            vec![output],
        )
        .await
        .map_to_mm(|e| UtxoMergeError::InternalError(format!("Error in generate_tx for coin={ticker}: {e}")))?;
        tx
    };

    Ok((tx, unspents))
}

async fn merge_utxo_loop<T>(
    weak: UtxoWeak,
    merge_at: usize,
    check_every: f64,
    max_merge_at_once: usize,
    constructor: impl Fn(UtxoArc) -> T,
) where
    T: UtxoCommonOps + GetUtxoListOps,
{
    let (my_address, script_pubkey) = match weak.upgrade() {
        Some(arc) => {
            let coin = constructor(arc);
            let my_address = match &coin.as_ref().derivation_method {
                DerivationMethod::SingleAddress(my_address) => my_address.clone(),
                DerivationMethod::HDWallet(hd_wallet) => match hd_wallet.get_enabled_address().await {
                    Some(hd_address) => hd_address.address,
                    None => {
                        // When this happens, this might be due to using HW wallet that its addresses hasn't been polled yet.
                        // Anyway we don't really need a consolidation loop for HW wallets since they can't swap and
                        // such a loop will keep asking for consolidation signatures every now and then.
                        covered_error!(
                            "No enabled address found in HD wallet for coin {}",
                            coin.as_ref().conf.ticker
                        );
                        return;
                    },
                },
            };
            let script_pubkey = match output_script(&my_address) {
                Ok(script) => script,
                Err(e) => {
                    covered_error!("Error {} on output_script for coin {}", e, coin.as_ref().conf.ticker);
                    return;
                },
            };
            (my_address, script_pubkey)
        },
        None => return,
    };

    let merge_conditions = MergeConditions {
        merge_at,
        max_merge_at_once,
    };

    loop {
        Timer::sleep(check_every).await;

        let coin = match weak.upgrade() {
            Some(arc) => constructor(arc),
            None => break,
        };

        let ticker = &coin.as_ref().conf.ticker;
        match merge_utxos(&coin, &my_address, &script_pubkey, &merge_conditions, true).await {
            Ok((tx, spents)) => info!(
                "UTXO merge of {} outputs successful for coin={ticker}, tx_hash={}",
                spents.len(),
                tx.hash().reversed()
            ),
            Err(e) => match e.into_inner() {
                UtxoMergeError::BadMergeConditions(_) => (), // We don't gotta log any errors here since we provided a bad merge conditions.ƒ
                UtxoMergeError::InternalError(e) => error!("Error on UTXO merge attempt for coin={ticker}: {e}"),
            },
        }
    }
}

pub trait MergeUtxoArcOps<T: UtxoCommonOps + GetUtxoListOps>: UtxoCoinBuilderCommonOps {
    fn spawn_merge_utxo_loop_if_required<F>(&self, utxo_arc: &UtxoArc, constructor: F)
    where
        F: Fn(UtxoArc) -> T + Send + Sync + 'static,
    {
        let merge_params = match self.activation_params().utxo_merge_params {
            Some(ref merge_params) => merge_params,
            None => return,
        };

        let ticker = self.ticker();
        info!("Starting UTXO merge loop for coin {ticker}");

        let utxo_weak = utxo_arc.downgrade();
        let fut = merge_utxo_loop(
            utxo_weak,
            merge_params.merge_at,
            merge_params.check_every,
            merge_params.max_merge_at_once,
            constructor,
        );

        let settings = AbortSettings::info_on_abort(format!("spawn_merge_utxo_loop_if_required stopped for {ticker}"));
        utxo_arc
            .abortable_system
            .weak_spawner()
            .spawn_with_settings(fut, settings);
    }
}

pub(crate) struct BlockHeaderUtxoLoopExtraArgs {
    pub(crate) chunk_size: u64,
    pub(crate) error_sleep: f64,
    pub(crate) success_sleep: f64,
}

#[cfg_attr(test, mockable)]
impl Default for BlockHeaderUtxoLoopExtraArgs {
    fn default() -> Self {
        Self {
            chunk_size: 2016,
            error_sleep: 10.,
            success_sleep: 60.,
        }
    }
}

/// This function executes a loop to fetch, validate and store block headers from the connected electrum servers.
/// sync_status_loop_handle notifies the coin activation function of errors and if the error is temporary or not.
/// spv_conf is passed from the coin configuration and it determines how headers are validated and stored.
pub(crate) async fn block_header_utxo_loop(
    weak: Weak<ElectrumClientImpl>,
    mut sync_status_loop_handle: UtxoSyncStatusLoopHandle,
    spv_conf: SPVConf,
) {
    macro_rules! remove_server_and_break_if_no_servers_left {
        ($client:expr, $server_address:expr, $ticker:expr, $sync_status_loop_handle:expr) => {
            if let Err(e) = $client.remove_server($server_address) {
                let msg = format!("Error {} on removing server {}!", e, $server_address);
                // Todo: Permanent error notification should lead to deactivation of coin after applying some fail-safe measures if there are on-going swaps
                $sync_status_loop_handle.notify_on_permanent_error(msg);
                break;
            }

            if $client.is_connections_pool_empty() {
                // Todo: Permanent error notification should lead to deactivation of coin after applying some fail-safe measures if there are on-going swaps
                let msg = format!("All servers are removed for {}!", $ticker);
                $sync_status_loop_handle.notify_on_permanent_error(msg);
                break;
            }
        };
    }

    let (mut electrum_addresses, mut block_count) = match weak.upgrade() {
        Some(client) => {
            let client = ElectrumClient(client);
            match client.get_servers_with_latest_block_count().compat().await {
                Ok((electrum_addresses, block_count)) => (electrum_addresses, block_count),
                Err(err) => {
                    sync_status_loop_handle.notify_on_permanent_error(err);
                    return;
                },
            }
        },
        None => {
            sync_status_loop_handle.notify_on_permanent_error("Electrum client dropped!".to_string());
            return;
        },
    };
    let mut args = BlockHeaderUtxoLoopExtraArgs::default();
    while let Some(client) = weak.upgrade() {
        let client = ElectrumClient(client);
        let ticker = client.coin_name();

        let storage = client.block_headers_storage();
        let last_height_in_storage = match storage.get_last_block_height().await {
            Ok(Some(height)) => height,
            Ok(None) => {
                if let Err(err) = validate_and_store_starting_header(&client, ticker, storage, &spv_conf).await {
                    sync_status_loop_handle.notify_on_permanent_error(err);
                    break;
                }
                spv_conf.starting_block_header.height
            },
            Err(err) => {
                error!(
                    "Error {} on getting the height of the last stored {} header in DB!",
                    err, ticker
                );
                sync_status_loop_handle.notify_on_temp_error(err);
                Timer::sleep(args.error_sleep).await;
                continue;
            },
        };

        let mut retrieve_to = last_height_in_storage + args.chunk_size;
        if retrieve_to > block_count {
            (electrum_addresses, block_count) = match client.get_servers_with_latest_block_count().compat().await {
                Ok((electrum_addresses, block_count)) => (electrum_addresses, block_count),
                Err(e) => {
                    let msg = format!("Error {e} on getting the height of the latest {ticker} block from rpc!");
                    error!("{}", msg);
                    sync_status_loop_handle.notify_on_temp_error(msg);
                    Timer::sleep(args.error_sleep).await;
                    continue;
                },
            };

            if retrieve_to > block_count {
                retrieve_to = block_count;
            }
        }
        drop_mutability!(retrieve_to);

        if last_height_in_storage == block_count {
            sync_status_loop_handle.notify_sync_finished(block_count);
            Timer::sleep(args.success_sleep).await;
            continue;
        }

        // Check if there should be a limit on the number of headers stored in storage.
        if let Some(max_stored_block_headers) = spv_conf.max_stored_block_headers {
            if let Err(err) =
                remove_excessive_headers_from_storage(storage, retrieve_to, max_stored_block_headers).await
            {
                error!("Error {} on removing excessive {} headers from storage!", err, ticker);
                sync_status_loop_handle.notify_on_temp_error(err);
                Timer::sleep(args.error_sleep).await;
            };
        }

        sync_status_loop_handle.notify_blocks_headers_sync_status(last_height_in_storage + 1, retrieve_to);

        let index = rand::thread_rng().gen_range(0, electrum_addresses.len());
        let server_address = match electrum_addresses.get(index) {
            Some(address) => address,
            None => {
                let msg = "Electrum addresses are empty when there should be at least one electrum returned from get_servers_with_latest_block_count!";
                error!("{}", msg);
                sync_status_loop_handle.notify_on_temp_error(msg.to_string());
                Timer::sleep(args.error_sleep).await;
                continue;
            },
        };
        let (block_registry, block_headers) = match try_to_retrieve_headers_until_success(
            &mut args,
            &client,
            server_address,
            last_height_in_storage + 1,
            retrieve_to,
        )
        .await
        {
            Ok((block_registry, block_headers)) => (block_registry, block_headers),
            Err(err) => match err.get_inner() {
                TryToRetrieveHeadersUntilSuccessError::NetworkError { .. } => {
                    error!("{}", err);
                    sync_status_loop_handle.notify_on_temp_error(err.to_string());
                    continue;
                },
                TryToRetrieveHeadersUntilSuccessError::PermanentError { .. } => {
                    error!("{}", err);
                    remove_server_and_break_if_no_servers_left!(
                        client,
                        server_address,
                        ticker,
                        sync_status_loop_handle
                    );
                    continue;
                },
            },
        };

        // Validate retrieved block headers.
        if let Err(err) = validate_headers(ticker, last_height_in_storage, &block_headers, storage, &spv_conf).await {
            error!("Error {} on validating the latest headers for {}!", err, ticker);
            // This code block handles a specific error scenario where a parent hash mismatch(chain re-org) is
            // detected in the SPV client.
            // If this error occurs, the code retrieves and revalidates the mismatching header from the SPV client..
            if let SPVError::ParentHashMismatch {
                coin,
                mismatched_block_height,
            } = &err
            {
                match resolve_possible_chain_reorg(
                    &client,
                    server_address,
                    &mut args,
                    last_height_in_storage,
                    *mismatched_block_height,
                    storage,
                    &spv_conf,
                )
                .await
                {
                    Ok(()) => {
                        info!(
                            "Chain reorg detected and resolved for coin: {}, re-syncing reorganized headers!",
                            coin
                        );
                        continue;
                    },
                    Err(err) => {
                        error!("Error {} on resolving chain reorg for coin: {}!", err, coin);
                        if err.get_inner().is_network_error() {
                            sync_status_loop_handle.notify_on_temp_error(err.to_string());
                        } else {
                            remove_server_and_break_if_no_servers_left!(
                                client,
                                server_address,
                                ticker,
                                sync_status_loop_handle
                            );
                        }
                        continue;
                    },
                }
            }
            remove_server_and_break_if_no_servers_left!(client, server_address, ticker, sync_status_loop_handle);
            continue;
        }

        let sleep = args.error_sleep;
        ok_or_continue_after_sleep!(storage.add_block_headers_to_storage(block_registry).await, sleep);
    }
}

#[derive(Debug, Display)]
enum TryToRetrieveHeadersUntilSuccessError {
    #[display(fmt = "Network error: {error}, on retrieving headers from server {server_address}")]
    NetworkError { error: String, server_address: String },
    #[display(fmt = "Permanent Error: {error}, on retrieving headers from server {server_address}")]
    PermanentError { error: String, server_address: String },
}

/// Loops until the headers are retrieved successfully.
async fn try_to_retrieve_headers_until_success(
    args: &mut BlockHeaderUtxoLoopExtraArgs,
    client: &ElectrumClient,
    server_address: &str,
    retrieve_from: u64,
    retrieve_to: u64,
) -> Result<(HashMap<u64, BlockHeader>, Vec<BlockHeader>), MmError<TryToRetrieveHeadersUntilSuccessError>> {
    let mut attempts: u8 = TRY_TO_RETRIEVE_HEADERS_ATTEMPTS;
    loop {
        match client
            .retrieve_headers_from(server_address, retrieve_from, retrieve_to)
            .compat()
            .await
        {
            Ok(res) => break Ok(res),
            Err(err) => {
                let err_inner = err.get_inner();
                if err_inner.is_network_error() {
                    if attempts == 0 {
                        break Err(MmError::new(TryToRetrieveHeadersUntilSuccessError::NetworkError {
                            error: format!(
                                "Max attempts of {TRY_TO_RETRIEVE_HEADERS_ATTEMPTS} reached, will try to retrieve headers from a random server again!"
                            ),
                            server_address: server_address.to_string(),
                        }));
                    }
                    attempts -= 1;
                    error!(
                        "Network Error: {}, Will try fetching block headers again from {} after 10 secs",
                        err, server_address,
                    );
                    Timer::sleep(args.error_sleep).await;
                    continue;
                };

                // If electrum returns response too large error, we will reduce the requested headers by CHUNK_SIZE_REDUCER_VALUE in every loop until we arrive at a reasonable value.
                if err_inner.is_response_too_large() && args.chunk_size > CHUNK_SIZE_REDUCER_VALUE {
                    args.chunk_size -= CHUNK_SIZE_REDUCER_VALUE;
                    continue;
                }

                break Err(MmError::new(TryToRetrieveHeadersUntilSuccessError::PermanentError {
                    error: err.to_string(),
                    server_address: server_address.to_string(),
                }));
            },
        }
    }
}

// Represents the different types of errors that can occur while retrieving block headers from the Electrum client.
#[derive(Debug, Display)]
enum PossibleChainReorgError {
    #[display(fmt = "Preconfigured starting_block_header is bad or invalid. Please reconfigure.")]
    BadStartingHeaderChain,
    #[display(fmt = "Validation Error: {_0}")]
    ValidationError(String),
    #[display(fmt = "Error retrieving headers: {_0}")]
    HeadersRetrievalError(TryToRetrieveHeadersUntilSuccessError),
}

impl PossibleChainReorgError {
    fn is_network_error(&self) -> bool {
        matches!(
            self,
            PossibleChainReorgError::HeadersRetrievalError(TryToRetrieveHeadersUntilSuccessError::NetworkError { .. })
        )
    }
}

/// Retrieves block headers from the specified client within the given height range and revalidate against [`SPVError::ParentHashMismatch`] .
#[allow(clippy::unit_arg)]
async fn resolve_possible_chain_reorg(
    client: &ElectrumClient,
    server_address: &str,
    args: &mut BlockHeaderUtxoLoopExtraArgs,
    last_height_in_storage: u64,
    mismatched_block_height: u64,
    storage: &dyn BlockHeaderStorageOps,
    spv_conf: &SPVConf,
) -> Result<(), MmError<PossibleChainReorgError>> {
    let ticker = client.coin_name();
    let mut retrieve_from = mismatched_block_height;
    let mut retrieve_to = retrieve_from + args.chunk_size;

    loop {
        debug!(
            "Possible chain reorganization for coin:{} at block height {}!",
            ticker, retrieve_from
        );
        // Attempt to retrieve the headers and validate them.
        let (_, headers_to_validate) =
            match try_to_retrieve_headers_until_success(args, client, server_address, retrieve_from, retrieve_to).await
            {
                Ok(res) => res,
                Err(err) => {
                    break Err(MmError::new(PossibleChainReorgError::HeadersRetrievalError(
                        err.into_inner(),
                    )))
                },
            };
        // If the headers are successfully retrieved and validated, remove the headers from storage and continue the outer loop.
        match validate_headers(ticker, retrieve_from - 1, &headers_to_validate, storage, spv_conf).await {
            Ok(_) => {
                // Headers are valid, remove saved headers and continue outer loop
                let sleep = args.error_sleep;
                return Ok(ok_or_continue_after_sleep!(
                    storage
                        .remove_headers_from_storage(retrieve_from, last_height_in_storage)
                        .await,
                    sleep
                ));
            },
            Err(err) => {
                if let SPVError::ParentHashMismatch {
                    mismatched_block_height,
                    ..
                } = err
                {
                    // There is another parent hash mismatch, retrieve the chunk right before this mismatched block height.
                    retrieve_to = mismatched_block_height - 1;
                    // Check if the height to retrieve up to is equal to the height of the preconfigured starting block header.
                    // If it is, it indicates a bad chain, and we return an error of type `RetrieveHeadersError::BadStartingHeaderChain`.
                    if retrieve_to == spv_conf.starting_block_header.height {
                        // Bad chain for preconfigured starting header detected, reconfigure.
                        return Err(MmError::new(PossibleChainReorgError::BadStartingHeaderChain));
                    };
                    // Calculate the height to retrieve from on next iteration based on the the height we will retrieve up to and the chunk size.
                    // If the current height is below or equal to the starting block header height, use the block header
                    // height after the starting one.
                    retrieve_from = retrieve_to
                        .saturating_sub(args.chunk_size)
                        .max(spv_conf.starting_block_header.height + 1);
                } else {
                    return Err(MmError::new(PossibleChainReorgError::ValidationError(err.to_string())));
                }
            },
        }
    }
}

#[derive(Display)]
enum StartingHeaderValidationError {
    #[display(fmt = "Can't decode/deserialize from storage for {coin} - reason: {reason}")]
    DecodeErr {
        coin: String,
        reason: String,
    },
    RpcError(String),
    StorageError(String),
    #[display(fmt = "Error validating starting header for {coin} - reason: {reason}")]
    ValidationError {
        coin: String,
        reason: String,
    },
}

async fn validate_and_store_starting_header(
    client: &ElectrumClient,
    ticker: &str,
    storage: &dyn BlockHeaderStorageOps,
    spv_conf: &SPVConf,
) -> MmResult<(), StartingHeaderValidationError> {
    let height = spv_conf.starting_block_header.height;
    let header_bytes = client
        .blockchain_block_header(height)
        .compat()
        .await
        .map_to_mm(|err| StartingHeaderValidationError::RpcError(err.to_string()))?;

    let mut reader = Reader::new_with_coin_variant(&header_bytes, ticker.into());
    let header = reader
        .read()
        .map_to_mm(|err| StartingHeaderValidationError::DecodeErr {
            coin: ticker.to_string(),
            reason: err.to_string(),
        })?;

    spv_conf
        .validate_rpc_starting_header(height, &header)
        .map_to_mm(|err| StartingHeaderValidationError::ValidationError {
            coin: ticker.to_string(),
            reason: err.to_string(),
        })?;

    storage
        .add_block_headers_to_storage(HashMap::from([(height, header)]))
        .await
        .map_to_mm(|err| StartingHeaderValidationError::StorageError(err.to_string()))
}

async fn remove_excessive_headers_from_storage(
    storage: &BlockHeaderStorage,
    last_height_to_be_added: u64,
    max_allowed_headers: NonZeroU64,
) -> Result<(), BlockHeaderStorageError> {
    let max_allowed_headers = max_allowed_headers.get();
    if last_height_to_be_added > max_allowed_headers {
        return storage
            .remove_headers_from_storage(0, last_height_to_be_added - max_allowed_headers)
            .await;
    }

    Ok(())
}

fn spawn_block_header_utxo_loop(
    ticker: &str,
    utxo_arc: &UtxoArc,
    sync_status_loop_handle: UtxoSyncStatusLoopHandle,
    spv_conf: SPVConf,
) {
    let client = match &utxo_arc.rpc_client {
        UtxoRpcClientEnum::Native(_) => return,
        UtxoRpcClientEnum::Electrum(client) => client,
    };
    info!("Starting UTXO block header loop for coin {ticker}");

    let electrum_weak = Arc::downgrade(&client.0);
    let fut = block_header_utxo_loop(electrum_weak, sync_status_loop_handle, spv_conf);

    let settings = AbortSettings::info_on_abort(format!("spawn_block_header_utxo_loop stopped for {ticker}"));
    utxo_arc
        .abortable_system
        .weak_spawner()
        .spawn_with_settings(fut, settings);
}
