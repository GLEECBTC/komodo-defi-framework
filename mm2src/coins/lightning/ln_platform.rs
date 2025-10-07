use super::*;
use crate::lightning::ln_errors::{SaveChannelClosingError, SaveChannelClosingResult};
use crate::lightning::ln_utils::RpcBestBlock;
use crate::utxo::rpc_clients::{
    BlockHashOrHeight, ConfirmedTransactionInfo, ElectrumBlockHeader, ElectrumClient, ElectrumNonce, EstimateFeeMethod,
    UtxoRpcClientEnum, UtxoRpcResult,
};
use crate::utxo::spv::SimplePaymentVerification;
use crate::utxo::utxo_standard::UtxoStandardCoin;
use crate::utxo::GetConfirmedTxError;
use crate::{MarketCoinOps, MmCoin, WaitForHTLCTxSpendArgs, WeakSpawner};
use bitcoin::blockdata::block::BlockHeader;
use bitcoin::blockdata::script::Script;
use bitcoin::blockdata::transaction::Transaction;
use bitcoin::consensus::encode::{deserialize, serialize_hex};
use bitcoin::hash_types::{BlockHash, TxMerkleNode, Txid};
use bitcoin_hashes::{sha256d, Hash};
use common::executor::{abortable_queue::AbortableQueue, AbortableSystem, SpawnFuture, Timer};
use common::log::{debug, error, info};
use common::{block_on_f01, wait_until_sec};
use futures::compat::Future01CompatExt;
use futures::future::join_all;
use keys::hash::H256;
use lightning::chain::{
    chaininterface::{BroadcasterInterface, ConfirmationTarget, FeeEstimator},
    Confirm, Filter, WatchedOutput,
};
use rpc::v1::types::{Bytes as BytesJson, H256 as H256Json};
use spv_validation::spv_proof::TRY_SPV_PROOF_INTERVAL;
use std::convert::TryInto;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering, Ordering};
use uuid::Uuid;

const CHECK_FOR_NEW_BEST_BLOCK_INTERVAL: f64 = 60.;
const TRY_LOOP_INTERVAL: f64 = 60.;
const TAKER_PAYMENT_SPEND_SEARCH_INTERVAL: f64 = 10.;

#[inline]
pub fn h256_json_from_txid(txid: Txid) -> H256Json {
    H256Json::from(txid.as_hash().into_inner()).reversed()
}

#[inline]
pub fn h256_from_txid(txid: Txid) -> H256 {
    H256::from(txid.as_hash().into_inner())
}

pub async fn get_best_header(best_header_listener: &ElectrumClient) -> EnableLightningResult<ElectrumBlockHeader> {
    best_header_listener
        .blockchain_headers_subscribe()
        .compat()
        .await
        .map_to_mm(|e| EnableLightningError::RpcError(e.to_string()))
}

pub async fn update_best_block(
    chain_monitor: Arc<ChainMonitor>,
    channel_manager: Arc<ChannelManager>,
    best_header: ElectrumBlockHeader,
) {
    {
        let (new_best_header, new_best_height) = match best_header {
            ElectrumBlockHeader::V12(h) => {
                let nonce = match h.nonce {
                    ElectrumNonce::Number(n) => n as u32,
                    ElectrumNonce::Hash(_) => {
                        return;
                    },
                };
                let prev_blockhash = sha256d::Hash::from_inner(h.prev_block_hash.0);
                let merkle_root = sha256d::Hash::from_inner(h.merkle_root.0);
                (
                    BlockHeader {
                        version: h.version as i32,
                        prev_blockhash: BlockHash::from_hash(prev_blockhash),
                        merkle_root: TxMerkleNode::from_hash(merkle_root),
                        time: h.timestamp as u32,
                        bits: h.bits as u32,
                        nonce,
                    },
                    h.block_height as u32,
                )
            },
            ElectrumBlockHeader::V14(h) => {
                let block_header = match deserialize(&h.hex.0) {
                    Ok(header) => header,
                    Err(e) => {
                        error!("Block header deserialization error: {}", e);
                        return;
                    },
                };
                (block_header, h.height as u32)
            },
        };
        async_blocking(move || channel_manager.best_block_updated(&new_best_header, new_best_height)).await;
        async_blocking(move || chain_monitor.best_block_updated(&new_best_header, new_best_height)).await;
    }
}

pub async fn ln_best_block_update_loop(
    platform: Arc<Platform>,
    db: SqliteLightningDB,
    chain_monitor: Arc<ChainMonitor>,
    channel_manager: Arc<ChannelManager>,
    best_header_listener: ElectrumClient,
    best_block: RpcBestBlock,
) {
    let mut current_best_block = best_block;
    loop {
        // Transactions confirmations check can be done at every CHECK_FOR_NEW_BEST_BLOCK_INTERVAL instead of at every new block
        // in case a transaction confirmation fails due to electrums being down. This way there will be no need to wait for a new
        // block to confirm such transaction and causing delays.
        platform
            .process_txs_confirmations(
                &best_header_listener,
                &db,
                Arc::clone(&chain_monitor),
                Arc::clone(&channel_manager),
            )
            .await;
        let best_header = ok_or_continue_after_sleep!(get_best_header(&best_header_listener).await, TRY_LOOP_INTERVAL);
        if current_best_block != best_header.clone().into() {
            platform.update_best_block_height(best_header.block_height());
            platform
                .process_txs_unconfirmations(Arc::clone(&chain_monitor), Arc::clone(&channel_manager))
                .await;
            current_best_block = best_header.clone().into();
            update_best_block(Arc::clone(&chain_monitor), Arc::clone(&channel_manager), best_header).await;
        }
        Timer::sleep(CHECK_FOR_NEW_BEST_BLOCK_INTERVAL).await;
    }
}

async fn get_funding_tx_bytes_loop(rpc_client: &UtxoRpcClientEnum, tx_hash: H256Json) -> BytesJson {
    loop {
        match rpc_client.get_transaction_bytes(&tx_hash).compat().await {
            Ok(res) => break res,
            Err(e) => {
                error!("error {}", e);
                Timer::sleep(TRY_LOOP_INTERVAL).await;
                continue;
            },
        }
    }
}

pub struct LatestFees {
    background: AtomicU64,
    normal: AtomicU64,
    high_priority: AtomicU64,
}

impl LatestFees {
    #[inline]
    fn set_background_fees(&self, fee: u64) {
        self.background.store(fee, Ordering::Release);
    }

    #[inline]
    fn set_normal_fees(&self, fee: u64) {
        self.normal.store(fee, Ordering::Release);
    }

    #[inline]
    fn set_high_priority_fees(&self, fee: u64) {
        self.high_priority.store(fee, Ordering::Release);
    }
}

pub struct Platform {
    pub coin: UtxoStandardCoin,
    /// Main/testnet/signet/regtest Needed for lightning node to know which network to connect to
    pub network: BlockchainNetwork,
    /// The average time in seconds needed to mine a new block for the blockchain network.
    pub avg_blocktime: u64,
    /// The best block height.
    pub best_block_height: AtomicU64,
    /// Number of blocks for every Confirmation target. This is used in the FeeEstimator.
    pub confirmations_targets: PlatformCoinConfirmationTargets,
    /// Latest fees are used when the call for estimate_fee_sat fails.
    pub latest_fees: LatestFees,
    /// This cache stores the transactions that the LN node has interest in.
    pub registered_txs: PaMutex<HashSet<Txid>>,
    /// This cache stores the outputs that the LN node has interest in.
    pub registered_outputs: PaMutex<Vec<WatchedOutput>>,
    /// This cache stores transactions to be broadcasted once the other node accepts the channel
    pub unsigned_funding_txs: PaMutex<HashMap<Uuid, TransactionInputSigner>>,
    /// This spawner is used to spawn coin's related futures that should be aborted on coin deactivation.
    /// and on [`MmArc::stop`].
    pub abortable_system: AbortableQueue,
}

impl Platform {
    #[inline]
    pub fn new(
        coin: UtxoStandardCoin,
        network: BlockchainNetwork,
        confirmations_targets: PlatformCoinConfirmationTargets,
    ) -> EnableLightningResult<Self> {
        // This should never return an error since it's validated that avg_blocktime is in platform coin config in a previous step of lightning activation.
        // But an error is returned here just in case.
        let avg_blocktime = coin
            .as_ref()
            .conf
            .avg_blocktime
            .ok_or_else(|| EnableLightningError::Internal("`avg_blocktime` can't be None!".into()))?;

        // Create an abortable system linked to the base `coin` so if the base coin is disabled,
        // all spawned futures related to `LightCoin` will be aborted as well.
        let abortable_system = coin.as_ref().abortable_system.create_subsystem()?;

        Ok(Platform {
            coin,
            network,
            avg_blocktime,
            best_block_height: AtomicU64::new(0),
            confirmations_targets,
            latest_fees: LatestFees {
                background: AtomicU64::new(0),
                normal: AtomicU64::new(0),
                high_priority: AtomicU64::new(0),
            },
            registered_txs: PaMutex::new(HashSet::new()),
            registered_outputs: PaMutex::new(Vec::new()),
            unsigned_funding_txs: PaMutex::new(HashMap::new()),
            abortable_system,
        })
    }

    #[inline]
    fn rpc_client(&self) -> &UtxoRpcClientEnum {
        &self.coin.as_ref().rpc_client
    }

    pub fn spawner(&self) -> WeakSpawner {
        self.abortable_system.weak_spawner()
    }

    pub async fn set_latest_fees(&self) -> UtxoRpcResult<()> {
        let platform_coin = &self.coin;
        let conf = &platform_coin.as_ref().conf;

        let latest_background_fees = self
            .rpc_client()
            .estimate_fee_sat(
                platform_coin.decimals(),
                // Todo: when implementing Native client detect_fee_method should be used for Native and EstimateFeeMethod::Standard for Electrum
                &EstimateFeeMethod::Standard,
                &conf.estimate_fee_mode,
                self.confirmations_targets.background,
            )
            .compat()
            .await?;
        self.latest_fees.set_background_fees(latest_background_fees);

        let latest_normal_fees = self
            .rpc_client()
            .estimate_fee_sat(
                platform_coin.decimals(),
                // Todo: when implementing Native client detect_fee_method should be used for Native and EstimateFeeMethod::Standard for Electrum
                &EstimateFeeMethod::Standard,
                &conf.estimate_fee_mode,
                self.confirmations_targets.normal,
            )
            .compat()
            .await?;
        self.latest_fees.set_normal_fees(latest_normal_fees);

        let latest_high_priority_fees = self
            .rpc_client()
            .estimate_fee_sat(
                platform_coin.decimals(),
                // Todo: when implementing Native client detect_fee_method should be used for Native and EstimateFeeMethod::Standard for Electrum
                &EstimateFeeMethod::Standard,
                &conf.estimate_fee_mode,
                self.confirmations_targets.high_priority,
            )
            .compat()
            .await?;
        self.latest_fees.set_high_priority_fees(latest_high_priority_fees);

        Ok(())
    }

    #[inline]
    pub fn update_best_block_height(&self, new_height: u64) {
        self.best_block_height.store(new_height, AtomicOrdering::Release);
    }

    #[inline]
    pub fn best_block_height(&self) -> u64 {
        self.best_block_height.load(AtomicOrdering::Acquire)
    }

    pub fn add_tx(&self, txid: Txid) {
        let mut registered_txs = self.registered_txs.lock();
        registered_txs.insert(txid);
    }

    pub fn add_output(&self, output: WatchedOutput) {
        let mut registered_outputs = self.registered_outputs.lock();
        registered_outputs.push(output);
    }

    async fn process_tx_for_unconfirmation<T>(&self, txid: Txid, monitor: Arc<T>)
    where
        T: Confirm + Send + Sync + 'static,
    {
        let rpc_txid = h256_json_from_txid(txid);
        match self.rpc_client().get_tx_if_onchain(&rpc_txid).await {
            Ok(Some(_)) => {},
            Ok(None) => {
                info!(
                    "Transaction {} is not found on chain. The transaction will be re-broadcasted.",
                    txid,
                );
                let monitor = monitor.clone();
                async_blocking(move || monitor.transaction_unconfirmed(&txid)).await;
                // If a transaction is unconfirmed due to a block reorganization; LDK will rebroadcast it.
                // In this case, this transaction needs to be added again to the registered transactions
                // to start watching for it on the chain again.
                self.add_tx(txid);
            },
            Err(e) => error!(
                "Error while trying to check if the transaction {} is discarded or not :{:?}",
                txid, e
            ),
        }
    }

    pub async fn process_txs_unconfirmations(
        &self,
        chain_monitor: Arc<ChainMonitor>,
        channel_manager: Arc<ChannelManager>,
    ) {
        // Retrieve channel manager transaction IDs to check the chain for un-confirmations
        let channel_manager_relevant_txids = channel_manager.get_relevant_txids();
        for (txid, _) in channel_manager_relevant_txids {
            self.process_tx_for_unconfirmation(txid, Arc::clone(&channel_manager))
                .await;
        }

        // Retrieve chain monitor transaction IDs to check the chain for un-confirmations
        let chain_monitor_relevant_txids = chain_monitor.get_relevant_txids();
        for (txid, _) in chain_monitor_relevant_txids {
            self.process_tx_for_unconfirmation(txid, Arc::clone(&chain_monitor))
                .await;
        }
    }

    async fn get_confirmed_registered_txs(&self, client: &ElectrumClient) -> Vec<ConfirmedTransactionInfo> {
        let registered_txs = self.registered_txs.lock().clone();

        let on_chain_txs_futs = registered_txs
            .into_iter()
            .map(|txid| async move {
                let rpc_txid = h256_json_from_txid(txid);
                self.rpc_client().get_tx_if_onchain(&rpc_txid).await
            })
            .collect::<Vec<_>>();
        let on_chain_txs = join_all(on_chain_txs_futs)
            .await
            .into_iter()
            .filter_map(|maybe_tx| match maybe_tx {
                Ok(maybe_tx) => maybe_tx,
                Err(e) => {
                    error!(
                        "Error while trying to figure if transaction is on-chain or not: {:?}",
                        e
                    );
                    None
                },
            });

        let is_spv_enabled = self.coin.as_ref().conf.spv_conf.is_some();
        let confirmed_transactions_futs = on_chain_txs
            .map(|transaction| async move {
                if is_spv_enabled {
                    client
                        // TODO: Should log the spv error if height > 0
                        .validate_spv_proof(&transaction, wait_until_sec(TRY_SPV_PROOF_INTERVAL))
                        .await
                        .map_err(GetConfirmedTxError::SPVError)
                } else {
                    client.get_confirmed_tx_info_from_rpc(&transaction).await
                }
            })
            .collect::<Vec<_>>();
        join_all(confirmed_transactions_futs)
            .await
            .into_iter()
            .filter_map(|confirmed_transaction| match confirmed_transaction {
                Ok(confirmed_tx) => {
                    let txid = Txid::from_hash(confirmed_tx.tx.hash().reversed().to_sha256d());
                    self.registered_txs.lock().remove(&txid);
                    Some(confirmed_tx)
                },
                Err(e) => {
                    error!("Error verifying transaction: {:?}", e);
                    None
                },
            })
            .collect()
    }

    async fn append_spent_registered_output_txs(
        &self,
        transactions_to_confirm: &mut Vec<ConfirmedTransactionInfo>,
        client: &ElectrumClient,
    ) {
        let registered_outputs = self.registered_outputs.lock().clone();

        let spent_outputs_info_fut = registered_outputs
            .into_iter()
            .map(|output| async move {
                self.rpc_client()
                    .find_output_spend(
                        h256_from_txid(output.outpoint.txid),
                        output.script_pubkey.as_ref(),
                        output.outpoint.index.into(),
                        BlockHashOrHeight::Hash(Default::default()),
                        self.coin.as_ref().tx_hash_algo,
                    )
                    .compat()
                    .await
            })
            .collect::<Vec<_>>();
        let mut spent_outputs_info = join_all(spent_outputs_info_fut)
            .await
            .into_iter()
            .filter_map(|maybe_spent| match maybe_spent {
                Ok(maybe_spent) => maybe_spent,
                Err(e) => {
                    error!("Error while trying to figure if output is spent or not: {:?}", e);
                    None
                },
            })
            .collect::<Vec<_>>();
        spent_outputs_info.retain(|output| {
            !transactions_to_confirm
                .iter()
                .any(|info| info.tx.hash() == output.spending_tx.hash())
        });

        let is_spv_enabled = self.coin.as_ref().conf.spv_conf.is_some();
        let confirmed_transactions_futs = spent_outputs_info
            .into_iter()
            .map(|output| async move {
                if is_spv_enabled {
                    client
                        // TODO: Should log the spv error if height > 0
                        .validate_spv_proof(&output.spending_tx, wait_until_sec(TRY_SPV_PROOF_INTERVAL))
                        .await
                        .map_err(GetConfirmedTxError::SPVError)
                } else {
                    client.get_confirmed_tx_info_from_rpc(&output.spending_tx).await
                }
            })
            .collect::<Vec<_>>();
        let mut confirmed_transaction_info = join_all(confirmed_transactions_futs)
            .await
            .into_iter()
            .filter_map(|confirmed_transaction| match confirmed_transaction {
                Ok(confirmed_tx) => {
                    self.registered_outputs.lock().retain(|output| {
                        !confirmed_tx
                            .tx
                            .clone()
                            .inputs
                            .into_iter()
                            .any(|txin| txin.previous_output.hash == h256_from_txid(output.outpoint.txid))
                    });
                    Some(confirmed_tx)
                },
                Err(e) => {
                    error!("Error verifying transaction: {:?}", e);
                    None
                },
            })
            .collect();

        transactions_to_confirm.append(&mut confirmed_transaction_info);
    }

    pub async fn process_txs_confirmations(
        &self,
        client: &ElectrumClient,
        db: &SqliteLightningDB,
        chain_monitor: Arc<ChainMonitor>,
        channel_manager: Arc<ChannelManager>,
    ) {
        let mut transactions_to_confirm = self.get_confirmed_registered_txs(client).await;
        self.append_spent_registered_output_txs(&mut transactions_to_confirm, client)
            .await;

        transactions_to_confirm.sort_by(|a, b| (a.height, a.index).cmp(&(b.height, b.index)));

        for confirmed_transaction_info in transactions_to_confirm {
            let best_block_height = self.best_block_height() as i64;
            if let Err(e) = db
                .update_funding_tx_block_height(
                    confirmed_transaction_info.tx.hash().reversed().to_string(),
                    best_block_height,
                )
                .await
            {
                error!("Unable to update the funding tx block height in DB: {}", e);
            }
            let channel_manager = channel_manager.clone();
            let confirmed_transaction_info_cloned = confirmed_transaction_info.clone();
            async_blocking(move || {
                channel_manager.transactions_confirmed(
                    &confirmed_transaction_info_cloned.header.clone().into(),
                    &[(
                        confirmed_transaction_info_cloned.index as usize,
                        &confirmed_transaction_info_cloned.tx.clone().into(),
                    )],
                    confirmed_transaction_info_cloned.height as u32,
                )
            })
            .await;
            let chain_monitor = chain_monitor.clone();
            async_blocking(move || {
                chain_monitor.transactions_confirmed(
                    &confirmed_transaction_info.header.into(),
                    &[(
                        confirmed_transaction_info.index as usize,
                        &confirmed_transaction_info.tx.into(),
                    )],
                    confirmed_transaction_info.height as u32,
                )
            })
            .await;
        }
    }

    pub async fn get_channel_closing_tx(&self, channel_details: DBChannelDetails) -> SaveChannelClosingResult<String> {
        let from_block = channel_details
            .funding_generated_in_block
            .map(|b| b.try_into())
            .transpose()?
            .unwrap_or_else(|| self.best_block_height());

        let tx_id = channel_details
            .funding_tx
            .ok_or_else(|| MmError::new(SaveChannelClosingError::FundingTxNull))?;

        let tx_hash =
            H256Json::from_str(&tx_id).map_to_mm(|e| SaveChannelClosingError::FundingTxParseError(e.to_string()))?;

        let funding_tx_bytes = get_funding_tx_bytes_loop(self.rpc_client(), tx_hash).await;

        let closing_tx = self
            .coin
            // TODO add fn with old wait_for_tx_spend name
            .wait_for_htlc_tx_spend(WaitForHTLCTxSpendArgs {
                tx_bytes: &funding_tx_bytes.into_vec(),
                secret_hash: &[],
                wait_until: wait_until_sec(3600),
                from_block,
                swap_contract_address: &None,
                check_every: TAKER_PAYMENT_SPEND_SEARCH_INTERVAL,
                watcher_reward: false,
            })
            .await
            .map_to_mm(|e| SaveChannelClosingError::WaitForFundingTxSpendError(e.get_plain_text_format()))?;

        let closing_tx_hash = format!("{:02x}", closing_tx.tx_hash_as_bytes());

        Ok(closing_tx_hash)
    }
}

impl FeeEstimator for Platform {
    // Gets estimated satoshis of fee required per 1000 Weight-Units.
    fn get_est_sat_per_1000_weight(&self, confirmation_target: ConfirmationTarget) -> u32 {
        let platform_coin = &self.coin;

        let latest_fees = match confirmation_target {
            ConfirmationTarget::Background => self.latest_fees.background.load(Ordering::Acquire),
            ConfirmationTarget::Normal => self.latest_fees.normal.load(Ordering::Acquire),
            ConfirmationTarget::HighPriority => self.latest_fees.high_priority.load(Ordering::Acquire),
        };

        let conf = &platform_coin.as_ref().conf;
        let n_blocks = match confirmation_target {
            ConfirmationTarget::Background => self.confirmations_targets.background,
            ConfirmationTarget::Normal => self.confirmations_targets.normal,
            ConfirmationTarget::HighPriority => self.confirmations_targets.high_priority,
        };
        let fee_rate = tokio::task::block_in_place(move || {
            block_on_f01(self.rpc_client().estimate_fee_sat(
                platform_coin.decimals(),
                // Todo: when implementing Native client detect_fee_method should be used for Native and
                // EstimateFeeMethod::Standard for Electrum
                &EstimateFeeMethod::Standard,
                &conf.estimate_fee_mode,
                n_blocks,
            ))
            .unwrap_or(latest_fees)
        });

        // Set default fee to last known fee for the corresponding confirmation target
        match confirmation_target {
            ConfirmationTarget::Background => self.latest_fees.set_background_fees(fee_rate),
            ConfirmationTarget::Normal => self.latest_fees.set_normal_fees(fee_rate),
            ConfirmationTarget::HighPriority => self.latest_fees.set_high_priority_fees(fee_rate),
        };

        // Must be no smaller than 253 (ie 1 satoshi-per-byte rounded up to ensure later round-downs don’t put us below 1 satoshi-per-byte).
        // https://docs.rs/lightning/0.0.101/lightning/chain/chaininterface/trait.FeeEstimator.html#tymethod.get_est_sat_per_1000_weight
        // This has changed in rust-lightning v0.0.110 as LDK currently wraps get_est_sat_per_1000_weight to ensure that the value returned is
        // no smaller than 253. https://github.com/lightningdevkit/rust-lightning/pull/1552
        (fee_rate as f64 / 4.0).ceil() as u32
    }
}

impl BroadcasterInterface for Platform {
    fn broadcast_transaction(&self, tx: &Transaction) {
        let txid = tx.txid();
        let tx_hex = serialize_hex(tx);
        debug!("Trying to broadcast transaction: {}", tx_hex);
        let tx_bytes = match hex::decode(&tx_hex) {
            Ok(b) => b,
            Err(e) => {
                error!("Converting transaction to bytes error:{}", e);
                return;
            },
        };

        let platform_coin = self.coin.clone();
        let fut = async move {
            loop {
                match platform_coin
                    .as_ref()
                    .rpc_client
                    .send_raw_transaction(tx_bytes.clone().into())
                    .compat()
                    .await
                {
                    Ok(id) => {
                        info!("Transaction broadcasted successfully: {:?} ", id);
                        break;
                    },
                    // Todo: broadcast transaction through p2p network instead in case of error
                    // Todo: I don't want to rely on p2p broadcasting for now since there is no way to know if there are nodes running bitcoin in native mode or not
                    // Todo: Also we need to make sure that the transaction was broadcasted after relying on the p2p network
                    Err(e) => {
                        error!("Broadcast transaction {} failed: {}", txid, e);
                        if !e.get_inner().is_network_error() {
                            break;
                        }
                        Timer::sleep(TRY_LOOP_INTERVAL).await;
                    },
                }
            }
        };
        self.spawner().spawn(fut);
    }
}

impl Filter for Platform {
    // Watches for this transaction on-chain
    #[inline]
    fn register_tx(&self, txid: &Txid, _script_pubkey: &Script) {
        self.add_tx(*txid);
    }

    // Watches for any transactions that spend this output on-chain
    fn register_output(&self, output: WatchedOutput) {
        self.add_output(output);
    }
}
