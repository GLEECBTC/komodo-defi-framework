use super::{z_coin_errors::*, BlockDbImpl, CheckPointBlockInfo, WalletDbShared, ZCoinBuilder, ZcoinConsensusParams};
use crate::utxo::utxo_builder::{UtxoCoinBuilderCommonOps, DAY_IN_SECONDS};
use crate::z_coin::storage::z_locked_notes::LockedNotesStorage;
use crate::z_coin::storage::{BlockProcessingMode, DataConnStmtCacheWrapper};
use crate::z_coin::SyncStartPoint;
use crate::RpcCommonOps;

use async_trait::async_trait;
use common::executor::Timer;
use common::executor::{spawn_abortable, AbortOnDropHandle};
use common::log::LogOnError;
use common::log::{debug, error, info};
use common::now_sec;
use futures::channel::mpsc::channel;
use futures::channel::mpsc::{Receiver as AsyncReceiver, Sender as AsyncSender};
use futures::channel::oneshot::{channel as oneshot_channel, Sender as OneshotSender};
use futures::lock::{Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard};
use futures::StreamExt;
use hex::{FromHex, FromHexError};
use mm2_err_handle::prelude::*;
use mm2_event_stream::StreamingManager;
use parking_lot::Mutex;
use prost::Message;
use rpc::v1::types::{Bytes, H256 as H256Json};
use std::convert::TryFrom;
use std::str::FromStr;
use std::sync::Arc;
use tonic::codec::CompressionEncoding;
use z_coin_grpc::{BlockId, BlockRange, TreeState, TxFilter};
use zcash_extras::{WalletRead, WalletWrite};
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::transaction::TxId;

pub(crate) mod z_coin_grpc {
    tonic::include_proto!("pirate.wallet.sdk.rpc");
}
use z_coin_grpc::compact_tx_streamer_client::CompactTxStreamerClient;
use z_coin_grpc::{ChainSpec, CompactBlock as TonicCompactBlock};

cfg_native!(
    use crate::ZTransaction;
    use crate::utxo::rpc_clients::{UtxoRpcClientOps};
    use crate::z_coin::z_coin_errors::{ZcoinStorageError, ValidateBlocksError};
    use crate::utxo::rpc_clients::NativeClient;

    use futures::compat::Future01CompatExt;
    use http::Uri;
    use group::GroupEncoding;
    use std::convert::TryInto;
    use std::num::TryFromIntError;
    use tonic::transport::{Channel, ClientTlsConfig};
    use tonic::codegen::StdError;

    use z_coin_grpc::{CompactOutput as TonicCompactOutput, CompactSpend as TonicCompactSpend, CompactTx as TonicCompactTx};
);

cfg_wasm32!(
    use futures_util::future::try_join_all;
    use mm2_net::wasm::tonic_client::TonicClient;

    const MAX_CHUNK_SIZE: u64 = 20000;
);

/// ZRpcOps trait provides asynchronous methods for performing various operations related to
/// Zcoin blockchain and wallet synchronization.
#[async_trait]
pub trait ZRpcOps: Send + Sync + 'static {
    /// Asynchronously retrieve the current block height from the Zcoin network.
    async fn get_block_height(&self) -> Result<u64, MmError<UpdateBlocksCacheErr>>;

    /// Asynchronously retrieve the tree state for a specific block height from the Zcoin network.
    async fn get_tree_state(&self, height: u64) -> Result<TreeState, MmError<UpdateBlocksCacheErr>>;

    /// Asynchronously scan and process blocks within a specified block height range.
    ///
    /// This method allows for scanning and processing blocks starting from `start_block` up to
    /// and including `last_block`. It invokes the provided `on_block` function for each compact
    /// block within the specified range.
    async fn scan_blocks(
        &self,
        start_block: u64,
        last_block: u64,
        db: &BlockDbImpl,
        handler: &mut SaplingSyncLoopHandle,
    ) -> Result<(), MmError<UpdateBlocksCacheErr>>;

    async fn check_tx_existence(&self, tx_id: TxId) -> bool;

    /// Retrieves checkpoint block information from the database at a specific height.
    ///
    /// checkpoint_block_from_height retrieves tree state information from rpc corresponding to the given
    /// height and constructs a `CheckPointBlockInfo` struct containing some needed details such as
    /// block height, hash, time, and sapling tree.
    async fn checkpoint_block_from_height(
        &self,
        height: u64,
        ticker: &str,
    ) -> MmResult<Option<CheckPointBlockInfo>, UpdateBlocksCacheErr>;
}

#[cfg(not(target_arch = "wasm32"))]
type RpcClientType = Channel;
#[cfg(target_arch = "wasm32")]
type RpcClientType = TonicClient;

#[derive(Clone)]
pub struct LightRpcClient(pub(crate) Arc<AsyncMutex<Vec<CompactTxStreamerClient<RpcClientType>>>>);

impl LightRpcClient {
    pub async fn new(lightwalletd_urls: Vec<String>) -> Result<Self, MmError<ZcoinClientInitError>> {
        let mut rpc_clients = Vec::new();
        if lightwalletd_urls.is_empty() {
            return MmError::err(ZcoinClientInitError::EmptyLightwalletdUris);
        }

        #[cfg(not(target_arch = "wasm32"))]
        let mut errors = Vec::new();
        for url in &lightwalletd_urls {
            #[cfg(not(target_arch = "wasm32"))]
            let uri = match Uri::from_str(url) {
                Ok(uri) => uri,
                Err(err) => {
                    errors.push(UrlIterError::InvalidUri(err));
                    continue;
                },
            };
            #[cfg(not(target_arch = "wasm32"))]
            let endpoint = match Channel::builder(uri).tls_config(ClientTlsConfig::new()) {
                Ok(endpoint) => endpoint,
                Err(err) => {
                    errors.push(UrlIterError::TlsConfigFailure(err));
                    continue;
                },
            };
            #[cfg(not(target_arch = "wasm32"))]
            let client = match Self::connect_endpoint(endpoint).await {
                Ok(tonic_channel) => tonic_channel.accept_compressed(CompressionEncoding::Gzip),
                Err(err) => {
                    errors.push(UrlIterError::ConnectionFailure(err));
                    continue;
                },
            };

            cfg_wasm32!(
                let client = CompactTxStreamerClient::new(TonicClient::new(url.to_string())).accept_compressed(CompressionEncoding::Gzip);
            );

            rpc_clients.push(client);
        }

        #[cfg(not(target_arch = "wasm32"))]
        drop_mutability!(errors);
        drop_mutability!(rpc_clients);
        // check if rpc_clients is empty, then for loop wasn't successful
        #[cfg(not(target_arch = "wasm32"))]
        if rpc_clients.is_empty() {
            return MmError::err(ZcoinClientInitError::UrlIterFailure(errors));
        }

        Ok(LightRpcClient(AsyncMutex::new(rpc_clients).into()))
    }

    /// Attempt to create a new client by connecting to a given endpoint.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn connect_endpoint<D>(dst: D) -> Result<CompactTxStreamerClient<Channel>, tonic::transport::Error>
    where
        D: TryInto<tonic::transport::Endpoint>,
        D::Error: Into<StdError>,
    {
        let conn = tonic::transport::Endpoint::new(dst)?.connect().await?;
        Ok(CompactTxStreamerClient::new(conn))
    }
}

#[async_trait]
impl RpcCommonOps for LightRpcClient {
    type RpcClient = CompactTxStreamerClient<RpcClientType>;
    type Error = MmError<UpdateBlocksCacheErr>;

    async fn get_live_client(&self) -> Result<Self::RpcClient, Self::Error> {
        let mut clients = self.0.lock().await;
        for (i, mut client) in clients.clone().into_iter().enumerate() {
            let request = tonic::Request::new(ChainSpec {});
            // use get_latest_block method as a health check
            let latest = client.get_latest_block(request).await;
            if latest.is_ok() {
                clients.rotate_left(i);
                return Ok(client);
            }
        }
        return Err(MmError::new(UpdateBlocksCacheErr::GetLiveLightClientError(
            "All the current light clients are unavailable.".to_string(),
        )));
    }
}

async fn handle_block_cache_update(
    db: &BlockDbImpl,
    handler: &mut SaplingSyncLoopHandle,
    block: Result<TonicCompactBlock, tonic::Status>,
    last_block: u64,
) -> Result<(), MmError<UpdateBlocksCacheErr>> {
    let block = block.map_err(|_| UpdateBlocksCacheErr::DecodeError("Error getting block".to_string()))?;
    debug!("Got block {}", block.height);
    let height = u32::try_from(block.height)
        .map_err(|_| UpdateBlocksCacheErr::DecodeError("Block height too large".to_string()))?;
    db.insert_block(height, block.encode_to_vec())
        .await
        .map_err(|err| UpdateBlocksCacheErr::ZcashDBError(err.to_string()))?;

    handler.notify_blocks_cache_status(block.height, last_block);
    Ok(())
}

#[async_trait]
impl ZRpcOps for LightRpcClient {
    async fn get_block_height(&self) -> Result<u64, MmError<UpdateBlocksCacheErr>> {
        let request = tonic::Request::new(ChainSpec {});
        let block = self
            .get_live_client()
            .await?
            .get_latest_block(request)
            .await
            .map_to_mm(UpdateBlocksCacheErr::GrpcError)?
            // return the message
            .into_inner();
        Ok(block.height)
    }

    async fn get_tree_state(&self, height: u64) -> Result<TreeState, MmError<UpdateBlocksCacheErr>> {
        let request = tonic::Request::new(BlockId { height, hash: vec![] });

        Ok(self
            .get_live_client()
            .await?
            .get_tree_state(request)
            .await
            .map_to_mm(UpdateBlocksCacheErr::GrpcError)?
            .into_inner())
    }

    #[cfg(target_arch = "wasm32")]
    async fn scan_blocks(
        &self,
        start_block: u64,
        last_block: u64,
        db: &BlockDbImpl,
        handler: &mut SaplingSyncLoopHandle,
    ) -> Result<(), MmError<UpdateBlocksCacheErr>> {
        let mut requests = Vec::new();
        let mut current_start = start_block;
        let selfi = self.get_live_client().await?;

        /// Wraps the client (`selfi`) in an `Arc` to enable safe cloning and sharing across futures.
        /// This is necessary to avoid the error "cannot return reference to local variable `selfi`."
        async fn get_block_range_wrapper(
            mut selfi: CompactTxStreamerClient<RpcClientType>,
            request: BlockRange,
        ) -> Result<tonic::Response<tonic::Streaming<TonicCompactBlock>>, tonic::Status> {
            selfi.get_block_range(tonic::Request::new(request)).await
        }

        // Generate multiple gRPC requests to fetch block ranges efficiently.
        // It takes the starting block height and the last block height as input and constructs requests for fetching
        // consecutive block ranges within the specified limits.
        while current_start <= last_block {
            let current_end = if current_start + MAX_CHUNK_SIZE - 1 <= last_block {
                current_start + MAX_CHUNK_SIZE - 1
            } else {
                last_block
            };

            let block_range = BlockRange {
                start: Some(BlockId {
                    height: current_start,
                    hash: Vec::new(),
                }),
                end: Some(BlockId {
                    height: current_end,
                    hash: Vec::new(),
                }),
            };

            requests.push(get_block_range_wrapper(selfi.clone(), block_range));
            current_start = current_end + 1;
        }

        let responses = try_join_all(requests).await?;
        for response in responses {
            let mut response = response.into_inner();
            while let Some(block) = response.next().await {
                handle_block_cache_update(db, handler, block, last_block).await?;
            }
        }

        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn scan_blocks(
        &self,
        start_block: u64,
        last_block: u64,
        db: &BlockDbImpl,
        handler: &mut SaplingSyncLoopHandle,
    ) -> Result<(), MmError<UpdateBlocksCacheErr>> {
        let mut selfi = self.get_live_client().await?;
        let request = tonic::Request::new(BlockRange {
            start: Some(BlockId {
                height: start_block,
                hash: Vec::new(),
            }),
            end: Some(BlockId {
                height: last_block,
                hash: Vec::new(),
            }),
        });
        let mut response = selfi
            .get_block_range(request)
            .await
            .map_to_mm(UpdateBlocksCacheErr::GrpcError)?
            .into_inner();
        while let Some(block) = response.next().await {
            handle_block_cache_update(db, handler, block, last_block).await?;
        }

        Ok(())
    }

    async fn check_tx_existence(&self, tx_id: TxId) -> bool {
        let mut attempts = 0;
        loop {
            if let Ok(mut client) = self.get_live_client().await {
                let request = tonic::Request::new(TxFilter {
                    block: None,
                    index: 0,
                    hash: tx_id.0.into(),
                });
                match client.get_transaction(request).await {
                    Ok(_) => break,
                    Err(e) => {
                        error!("Error on getting tx {}: err: {}", tx_id, e);
                        if attempts >= 5 {
                            return false;
                        }
                        attempts += 1;
                        Timer::sleep(30.).await;
                    },
                }
            }
        }
        true
    }

    async fn checkpoint_block_from_height(
        &self,
        height: u64,
        ticker: &str,
    ) -> MmResult<Option<CheckPointBlockInfo>, UpdateBlocksCacheErr> {
        let tree_state = self.get_tree_state(height).await?;
        let hash = H256Json::from_str(&tree_state.hash)
            .map_err(|err| UpdateBlocksCacheErr::DecodeError(err.to_string()))?
            .reversed();
        let sapling_tree = Bytes::new(
            FromHex::from_hex(&tree_state.tree)
                .map_err(|err: FromHexError| UpdateBlocksCacheErr::DecodeError(err.to_string()))?,
        );

        info!("Final Derived Sync Height for {ticker} is: {height}");
        Ok(Some(CheckPointBlockInfo {
            height: tree_state.height as u32,
            hash,
            time: tree_state.time,
            sapling_tree,
        }))
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl ZRpcOps for NativeClient {
    async fn get_block_height(&self) -> Result<u64, MmError<UpdateBlocksCacheErr>> {
        Ok(self.get_block_count().compat().await.map_mm_err()?)
    }

    async fn get_tree_state(&self, _height: u64) -> Result<TreeState, MmError<UpdateBlocksCacheErr>> {
        todo!()
    }

    async fn scan_blocks(
        &self,
        start_block: u64,
        last_block: u64,
        db: &BlockDbImpl,
        handler: &mut SaplingSyncLoopHandle,
    ) -> Result<(), MmError<UpdateBlocksCacheErr>> {
        for height in start_block..=last_block {
            let block = self.get_block_by_height(height).await.map_mm_err()?;
            debug!("Got block {:?}", block);
            let mut compact_txs = Vec::with_capacity(block.tx.len());
            // By default, CompactBlocks only contain CompactTxs for transactions that contain Sapling spends or outputs.
            // Create and push compact_tx during iteration.
            for (tx_id, hash_tx) in block.tx.iter().enumerate() {
                let tx_bytes = self.get_transaction_bytes(hash_tx).compat().await.map_mm_err()?;
                let tx = ZTransaction::read(tx_bytes.as_slice()).unwrap();
                let mut spends = Vec::new();
                let mut outputs = Vec::new();
                if !tx.shielded_spends.is_empty() || !tx.shielded_outputs.is_empty() {
                    // Create and push spends with outs for compact_tx during iterations.
                    for spend in &tx.shielded_spends {
                        let compact_spend = TonicCompactSpend {
                            nf: spend.nullifier.to_vec(),
                        };
                        spends.push(compact_spend);
                    }
                    for out in &tx.shielded_outputs {
                        let compact_out = TonicCompactOutput {
                            cmu: out.cmu.to_bytes().to_vec(),
                            epk: out.ephemeral_key.to_bytes().to_vec(),
                            // https://zips.z.cash/zip-0307#output-compression
                            // The first 52 bytes of the ciphertext contain the contents and opening of the note commitment,
                            // which is all of the data needed to spend the note and to verify that the note is spendable.
                            ciphertext: out.enc_ciphertext[0..52].to_vec(),
                        };
                        outputs.push(compact_out);
                    }
                    // Shadowing mut variables as immutable. No longer need to update them.
                    drop_mutability!(spends);
                    drop_mutability!(outputs);
                    let mut hash_tx_vec = hash_tx.0.to_vec();
                    hash_tx_vec.reverse();

                    let compact_tx = TonicCompactTx {
                        index: tx_id as u64,
                        hash: hash_tx_vec,
                        fee: 0,
                        spends,
                        outputs,
                    };
                    compact_txs.push(compact_tx);
                }
            }
            let mut hash = block.hash.0.to_vec();
            hash.reverse();
            // Set 0 in vector in the case of genesis block.
            let mut prev_hash = block.previousblockhash.unwrap_or_default().0.to_vec();
            prev_hash.reverse();
            // Shadowing mut variables as immutable.
            drop_mutability!(hash);
            drop_mutability!(prev_hash);
            drop_mutability!(compact_txs);

            let compact_block = TonicCompactBlock {
                proto_version: 0,
                height,
                hash,
                prev_hash,
                time: block.time,
                // (hash, prevHash, and time) OR (full header)
                header: Vec::new(),
                vtx: compact_txs,
            };
            let height: u32 = compact_block
                .height
                .try_into()
                .map_to_mm(|err: TryFromIntError| UpdateBlocksCacheErr::DecodeError(err.to_string()))?;
            db.insert_block(height, compact_block.encode_to_vec())
                .await
                .map_err(|err| UpdateBlocksCacheErr::ZcashDBError(err.to_string()))?;
            handler.notify_blocks_cache_status(compact_block.height, last_block);
        }

        Ok(())
    }

    async fn check_tx_existence(&self, tx_id: TxId) -> bool {
        let mut attempts = 0;
        loop {
            let tx_hash = H256Json::from(tx_id.0).reversed();
            let tx = self.get_raw_transaction_bytes(&tx_hash).compat().await;
            match tx {
                Ok(_) => break,
                Err(e) => {
                    error!("Error on getting tx {}: err: {}", tx_id, e);
                    if attempts >= 5 {
                        return false;
                    }
                    attempts += 1;
                    Timer::sleep(30.).await;
                },
            }
        }
        true
    }

    async fn checkpoint_block_from_height(
        &self,
        _height: u64,
        _ticker: &str,
    ) -> MmResult<Option<CheckPointBlockInfo>, UpdateBlocksCacheErr> {
        todo!()
    }
}

pub(super) async fn init_light_client(
    builder: &ZCoinBuilder<'_>,
    lightwalletd_urls: Vec<String>,
    blocks_db: BlockDbImpl,
    sync_params: &Option<SyncStartPoint>,
    skip_sync_params: bool,
    locked_notes_db: LockedNotesStorage,
) -> Result<(AsyncMutex<SaplingSyncConnector>, WalletDbShared), MmError<ZcoinClientInitError>> {
    let coin = builder.ticker.to_string();
    let (sync_status_notifier, sync_watcher) = channel(1);
    let (on_tx_gen_notifier, on_tx_gen_watcher) = channel(1);

    let light_rpc_clients = LightRpcClient::new(lightwalletd_urls).await?;

    let min_height = blocks_db.get_earliest_block().await.map_mm_err()? as u64;
    let current_block_height = light_rpc_clients
        .get_block_height()
        .await
        .mm_err(ZcoinClientInitError::UpdateBlocksCacheErr)?;
    let sapling_activation_height = builder.protocol_info.consensus_params.sapling_activation_height as u64;
    let sync_height = match *sync_params {
        Some(SyncStartPoint::Date(date)) => builder
            .calculate_starting_height_from_date(date, current_block_height)
            .mm_err(ZcoinClientInitError::UtxoCoinBuildError)?
            .unwrap_or(sapling_activation_height),
        Some(SyncStartPoint::Height(height)) => height,
        Some(SyncStartPoint::Earliest) => sapling_activation_height,
        None => builder
            .calculate_starting_height_from_date(now_sec() - DAY_IN_SECONDS, current_block_height)
            .mm_err(ZcoinClientInitError::UtxoCoinBuildError)?
            .unwrap_or(sapling_activation_height),
    };
    let maybe_checkpoint_block = light_rpc_clients
        .checkpoint_block_from_height(sync_height.max(sapling_activation_height), &coin)
        .await
        .map_mm_err()?;

    // check if no sync_params was provided and continue syncing from last height in db if it's > 0 or skip_sync_params is true.
    let continue_from_prev_sync =
        (min_height > 0 && sync_params.is_none()) || (skip_sync_params && min_height < sapling_activation_height);

    let wallet_db = WalletDbShared::new(builder, maybe_checkpoint_block, continue_from_prev_sync)
        .await
        .map_mm_err()?;

    // Check min_height in blocks_db and rewind blocks_db to 0 if sync_height != min_height
    if !continue_from_prev_sync && (sync_height != min_height) {
        // let user know we're clearing cache and re-syncing from new provided height.
        if min_height > 0 {
            info!("Older/Newer sync height detected!, rewinding blocks_db to new height: {sync_height:?}");
        }
        blocks_db.rewind_to_height(u32::MIN.into()).await.map_mm_err()?;
    };

    let first_sync_block = FirstSyncBlock {
        requested: sync_height,
        is_pre_sapling: sync_height < sapling_activation_height,
        actual: sync_height.max(sapling_activation_height),
    };
    let sync_handle = SaplingSyncLoopHandle {
        coin,
        current_block: BlockHeight::from_u32(0),
        blocks_db,
        wallet_db: wallet_db.clone(),
        consensus_params: builder.protocol_info.consensus_params.clone(),
        sync_status_notifier,
        main_sync_state_finished: false,
        on_tx_gen_watcher,
        watch_for_tx: None,
        scan_blocks_per_iteration: builder.z_coin_params.scan_blocks_per_iteration.into(),
        scan_interval_ms: builder.z_coin_params.scan_interval_ms,
        first_sync_block: first_sync_block.clone(),
        streaming_manager: builder.ctx.event_stream_manager.clone(),
        locked_notes_db,
    };

    let abort_handle = spawn_abortable(light_wallet_db_sync_loop(sync_handle, Box::new(light_rpc_clients)));

    Ok((
        SaplingSyncConnector::new_mutex_wrapped(sync_watcher, on_tx_gen_notifier, abort_handle, first_sync_block),
        wallet_db,
    ))
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) async fn init_native_client(
    builder: &ZCoinBuilder<'_>,
    native_client: NativeClient,
    blocks_db: BlockDbImpl,
    locked_notes_db: LockedNotesStorage,
) -> Result<(AsyncMutex<SaplingSyncConnector>, WalletDbShared), MmError<ZcoinClientInitError>> {
    let coin = builder.ticker.to_string();
    let (sync_status_notifier, sync_watcher) = channel(1);
    let (on_tx_gen_notifier, on_tx_gen_watcher) = channel(1);

    let checkpoint_block = builder.protocol_info.check_point_block.clone();
    let sapling_height = builder.protocol_info.consensus_params.sapling_activation_height;
    let checkpoint_height = checkpoint_block.clone().map(|b| b.height).unwrap_or(sapling_height) as u64;
    let first_sync_block = FirstSyncBlock {
        requested: checkpoint_height,
        is_pre_sapling: false,
        actual: checkpoint_height,
    };
    let wallet_db = WalletDbShared::new(builder, checkpoint_block, true)
        .await
        .mm_err(|err| ZcoinClientInitError::ZcoinStorageError(err.to_string()))?;

    let sync_handle = SaplingSyncLoopHandle {
        coin,
        current_block: BlockHeight::from_u32(0),
        blocks_db,
        wallet_db: wallet_db.clone(),
        consensus_params: builder.protocol_info.consensus_params.clone(),
        sync_status_notifier,
        main_sync_state_finished: false,
        on_tx_gen_watcher,
        watch_for_tx: None,
        scan_blocks_per_iteration: builder.z_coin_params.scan_blocks_per_iteration.into(),
        scan_interval_ms: builder.z_coin_params.scan_interval_ms,
        first_sync_block: first_sync_block.clone(),
        streaming_manager: builder.ctx.event_stream_manager.clone(),
        locked_notes_db,
    };
    let abort_handle = spawn_abortable(light_wallet_db_sync_loop(sync_handle, Box::new(native_client)));

    Ok((
        SaplingSyncConnector::new_mutex_wrapped(sync_watcher, on_tx_gen_notifier, abort_handle, first_sync_block),
        wallet_db,
    ))
}

pub struct SaplingSyncRespawnGuard {
    pub(super) sync_handle: Option<(SaplingSyncLoopHandle, Box<dyn ZRpcOps>)>,
    pub(super) abort_handle: Arc<Mutex<AbortOnDropHandle>>,
}

impl Drop for SaplingSyncRespawnGuard {
    fn drop(&mut self) {
        if let Some((handle, rpc)) = self.sync_handle.take() {
            *self.abort_handle.lock() = spawn_abortable(light_wallet_db_sync_loop(handle, rpc));
        }
    }
}

#[allow(unused)]
impl SaplingSyncRespawnGuard {
    pub(super) fn watch_for_tx(&mut self, tx_id: TxId) {
        if let Some(ref mut handle) = self.sync_handle {
            handle.0.watch_for_tx = Some(tx_id);
        }
    }

    #[inline]
    pub(super) fn current_block(&self) -> BlockHeight {
        self.sync_handle.as_ref().expect("always Some").0.current_block
    }
}

/// `SyncStatus` enumerates different states that may occur during the execution of
/// Zcoin-related operations during block sync.
///
/// - `UpdatingBlocksCache`: Represents the state of updating the blocks cache, with associated data
///   about the first synchronization block, the current scanned block, and the latest block.
/// - `BuildingWalletDb`: Denotes the state of building the wallet db, with associated data about
///   the first synchronization block, the current scanned block, and the latest block.
/// - `TemporaryError(String)`: Represents a temporary error state, with an associated error message
///   providing details about the error.
/// - `Finishing`: Represents the finishing state of an operation.
#[derive(Debug)]
pub enum SyncStatus {
    UpdatingBlocksCache {
        current_scanned_block: u64,
        latest_block: u64,
    },
    BuildingWalletDb {
        current_scanned_block: u64,
        latest_block: u64,
    },
    TemporaryError(String),
    Finished {
        block_number: u64,
    },
}

/// The `FirstSyncBlock` struct contains details about the block block that is used to start the synchronization
/// process.
/// It includes information about the requested block height, whether it predates the Sapling activation, and the
/// actual starting block height used during synchronization.
///
/// - `requested`: The requested block height during synchronization.
/// - `is_pre_sapling`: Indicates whether the block predates the Sapling activation.
/// - `actual`: The actual block height used for synchronization(may be altered).
#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FirstSyncBlock {
    pub requested: u64,
    pub is_pre_sapling: bool,
    pub actual: u64,
}

/// The `SaplingSyncLoopHandle` struct is used to manage and control Zcoin synchronization loop.
/// It includes information about the coin being synchronized, the current block height, database access, etc.
#[allow(unused)]
pub struct SaplingSyncLoopHandle {
    coin: String,
    current_block: BlockHeight,
    blocks_db: BlockDbImpl,
    wallet_db: WalletDbShared,
    locked_notes_db: LockedNotesStorage,
    consensus_params: ZcoinConsensusParams,
    /// Notifies about sync status without stopping the loop, e.g. on coin activation
    sync_status_notifier: AsyncSender<SyncStatus>,
    /// Signal to determine if main sync state is finished.
    main_sync_state_finished: bool,
    /// If new tx is required to be generated, we stop the sync and respawn it after tx is sent
    /// This watcher waits for such notification
    on_tx_gen_watcher: AsyncReceiver<OneshotSender<(Self, Box<dyn ZRpcOps>)>>,
    watch_for_tx: Option<TxId>,
    scan_blocks_per_iteration: u32,
    scan_interval_ms: u64,
    first_sync_block: FirstSyncBlock,
    /// A copy of the streaming manager to send notifications to the streamers upon new txs, balance change, etc...
    streaming_manager: StreamingManager,
}

impl SaplingSyncLoopHandle {
    #[inline]
    fn notify_blocks_cache_status(&mut self, current_scanned_block: u64, latest_block: u64) {
        if self.main_sync_state_finished {
            return;
        }
        self.sync_status_notifier
            .try_send(SyncStatus::UpdatingBlocksCache {
                current_scanned_block,
                latest_block,
            })
            .debug_log_with_msg("No one seems interested in SyncStatus");
    }

    fn notify_building_wallet_db(&mut self, current_scanned_block: u64, latest_block: u64) {
        if self.main_sync_state_finished {
            return;
        }
        self.sync_status_notifier
            .try_send(SyncStatus::BuildingWalletDb {
                current_scanned_block,
                latest_block,
            })
            .debug_log_with_msg("No one seems interested in SyncStatus");
    }

    fn notify_on_error(&mut self, error: String) {
        if self.main_sync_state_finished {
            return;
        }
        self.sync_status_notifier
            .try_send(SyncStatus::TemporaryError(error))
            .debug_log_with_msg("No one seems interested in SyncStatus");
    }

    fn notify_sync_finished(&mut self) {
        if self.main_sync_state_finished {
            return;
        } else {
            self.main_sync_state_finished = true
        }
        self.sync_status_notifier
            .try_send(SyncStatus::Finished {
                block_number: self.current_block.into(),
            })
            .debug_log_with_msg("No one seems interested in SyncStatus");
    }

    async fn update_blocks_cache(&mut self, rpc: &dyn ZRpcOps) -> Result<(), MmError<UpdateBlocksCacheErr>> {
        let current_block = rpc.get_block_height().await?;
        let block_db = self.blocks_db.clone();
        let current_block_in_db = &self.blocks_db.get_latest_block().await.map_mm_err()?;
        let wallet_db = self.wallet_db.clone();
        let extrema = wallet_db
            .db
            .block_height_extrema()
            .await
            .map_err(|err| MmError::new(UpdateBlocksCacheErr::ZcashDBError(err.to_string())))?;
        let mut from_block = self
            .consensus_params
            .sapling_activation_height
            .max(current_block_in_db + 1) as u64;

        if let Some((_, max_in_wallet)) = extrema {
            from_block = from_block.max(max_in_wallet.into());
        }

        if current_block >= from_block {
            rpc.scan_blocks(from_block, current_block, &block_db, self)
                .await
                .map_mm_err()?;
        }

        self.current_block = BlockHeight::from_u32(current_block as u32);
        Ok(())
    }

    /// Scans cached blocks, validates the chain and updates WalletDb.
    /// For more notes on the process, check https://github.com/zcash/librustzcash/blob/master/zcash_client_backend/src/data_api/chain.rs#L2
    async fn scan_validate_and_update_blocks(&mut self) -> Result<(), MmError<ZcoinStorageError>> {
        let blocks_db = self.blocks_db.clone();
        let wallet_db = self.wallet_db.db.clone();
        let mut wallet_ops = wallet_db.get_update_ops().expect("get_update_ops always returns Ok");

        if let Err(e) = blocks_db
            .process_blocks_with_mode(
                self.consensus_params.clone(),
                BlockProcessingMode::Validate,
                wallet_ops.get_max_height_hash().await?,
                None,
                &self.locked_notes_db,
            )
            .await
        {
            match e.into_inner() {
                ZcoinStorageError::ValidateBlocksError(ValidateBlocksError::ChainInvalid {
                    height: lower_bound,
                    ..
                }) => {
                    let rewind_height = if lower_bound > BlockHeight::from_u32(10) {
                        lower_bound - 10
                    } else {
                        BlockHeight::from_u32(0)
                    };
                    wallet_ops.rewind_to_height(rewind_height).await?;
                    blocks_db.rewind_to_height(rewind_height).await?;
                },
                e => return MmError::err(e),
            }
        }

        let current_block = BlockHeight::from_u32(blocks_db.get_latest_block().await?);
        loop {
            match wallet_ops.block_height_extrema().await? {
                Some((_, max_in_wallet)) => {
                    if max_in_wallet >= current_block {
                        break;
                    } else {
                        debug!("Updating wallet.db from block {} to {}", max_in_wallet, current_block);
                        self.notify_building_wallet_db(max_in_wallet.into(), current_block.into());
                    }
                },
                None => {
                    debug!("Updating wallet.db from block {} to {}", 0, current_block);
                    self.notify_building_wallet_db(0, current_block.into())
                },
            }

            let scan = DataConnStmtCacheWrapper::new(wallet_ops.clone());
            blocks_db
                .process_blocks_with_mode(
                    self.consensus_params.clone(),
                    BlockProcessingMode::Scan(scan, self.streaming_manager.clone()),
                    None,
                    Some(self.scan_blocks_per_iteration),
                    &self.locked_notes_db,
                )
                .await?;

            if self.scan_interval_ms > 0 {
                Timer::sleep_ms(self.scan_interval_ms).await;
            }
        }

        Ok(())
    }

    async fn check_watch_for_tx_existence(&mut self, rpc: &dyn ZRpcOps) {
        if let Some(tx_id) = self.watch_for_tx {
            if !rpc.check_tx_existence(tx_id).await {
                self.watch_for_tx = None;
            }
        }
    }
}

/// For more info on shielded light client protocol, please check the https://zips.z.cash/zip-0307
///
/// It's important to note that unlike standard UTXOs, shielded outputs are not spendable until the transaction is confirmed.
///
/// For AtomicDEX, we have additional requirements for the sync process:
/// 1. Coin should not be usable until initial sync is finished.
/// 2. During concurrent transaction generation (several simultaneous swaps using the same coin), we should prevent the same input usage.
/// 3. Once the transaction is sent, we have to wait until it's confirmed for the change to become spendable.
///
/// So the following was implemented:
/// 1. On the coin initialization, `init_light_client` creates `SaplingSyncLoopHandle`, spawns sync loop
///   and returns mutex-wrapped `SaplingSyncConnector` to interact with it.
/// 2. During sync process, the `SaplingSyncLoopHandle` notifies external code about status using `sync_status_notifier`.
/// 3. Once the sync completes, the coin becomes usable.
/// 4. When transaction is about to be generated, the external code locks the `SaplingSyncConnector` mutex,
///   and calls `SaplingSyncConnector::wait_for_gen_tx_blockchain_sync`.
///   This actually stops the loop and returns `SaplingSyncGuard`, which contains MutexGuard<SaplingSyncConnector> and `SaplingSyncRespawnGuard`.
/// 5. `SaplingSyncRespawnGuard` in its turn contains `SaplingSyncLoopHandle` that is used to respawn the sync when the guard is dropped.
/// 6. Once the transaction is generated and sent, `SaplingSyncRespawnGuard::watch_for_tx` is called to update `SaplingSyncLoopHandle` state.
/// 7. Once the loop is respawned, it will check that broadcast tx is imported (or not available anymore) before stopping in favor of
///   next wait_for_gen_tx_blockchain_sync call.
async fn light_wallet_db_sync_loop(mut sync_handle: SaplingSyncLoopHandle, mut client: Box<dyn ZRpcOps>) {
    info!(
        "(Re)starting light_wallet_db_sync_loop for {}, blocks per iteration {}, interval in ms {}",
        sync_handle.coin, sync_handle.scan_blocks_per_iteration, sync_handle.scan_interval_ms
    );

    loop {
        if let Err(e) = sync_handle.update_blocks_cache(client.as_ref()).await {
            error!("Error {} on blocks cache update", e);
            sync_handle.notify_on_error(e.to_string());
            Timer::sleep(10.).await;
            continue;
        }

        if let Err(e) = sync_handle.scan_validate_and_update_blocks().await {
            error!("Error {} on scan_blocks", e);
            sync_handle.notify_on_error(e.to_string());
            Timer::sleep(10.).await;
            continue;
        }

        sync_handle.notify_sync_finished();

        sync_handle.check_watch_for_tx_existence(client.as_ref()).await;

        if let Some(tx_id) = sync_handle.watch_for_tx {
            let walletdb = &sync_handle.wallet_db;
            if let Ok(is_tx_imported) = walletdb.is_tx_imported(tx_id).await {
                if !is_tx_imported {
                    error!("Tx {} is not imported yet", tx_id);
                    Timer::sleep(10.).await;
                    continue;
                }
            }
            sync_handle.watch_for_tx = None;
        }

        if let Ok(Some(sender)) = sync_handle.on_tx_gen_watcher.try_next() {
            match sender.send((sync_handle, client)) {
                Ok(_) => break,
                Err((handle_from_channel, rpc_from_channel)) => {
                    sync_handle = handle_from_channel;
                    client = rpc_from_channel;
                },
            }
        }

        Timer::sleep(10.).await;
    }
}

type SyncWatcher = AsyncReceiver<SyncStatus>;
type NewTxNotifier = AsyncSender<OneshotSender<(SaplingSyncLoopHandle, Box<dyn ZRpcOps>)>>;

pub(super) struct SaplingSyncConnector {
    pub(super) sync_watcher: SyncWatcher,
    on_tx_gen_notifier: NewTxNotifier,
    abort_handle: Arc<Mutex<AbortOnDropHandle>>,
    first_sync_block: FirstSyncBlock,
}

impl SaplingSyncConnector {
    #[allow(unused)]
    #[inline]
    pub(super) fn new_mutex_wrapped(
        sync_watcher: SyncWatcher,
        on_tx_gen_notifier: NewTxNotifier,
        abort_handle: AbortOnDropHandle,
        first_sync_block: FirstSyncBlock,
    ) -> AsyncMutex<Self> {
        AsyncMutex::new(SaplingSyncConnector {
            sync_watcher,
            on_tx_gen_notifier,
            abort_handle: Arc::new(Mutex::new(abort_handle)),
            first_sync_block,
        })
    }

    #[inline]
    pub(super) async fn first_sync_block(&self) -> Result<FirstSyncBlock, MmError<BlockchainScanStopped>> {
        Ok(self.first_sync_block.clone())
    }

    #[inline]
    pub(super) async fn current_sync_status(&mut self) -> Result<SyncStatus, MmError<BlockchainScanStopped>> {
        self.sync_watcher.next().await.or_mm_err(|| BlockchainScanStopped {})
    }

    pub(super) async fn wait_for_gen_tx_blockchain_sync(
        &mut self,
    ) -> Result<SaplingSyncRespawnGuard, MmError<BlockchainScanStopped>> {
        let (sender, receiver) = oneshot_channel();
        self.on_tx_gen_notifier
            .try_send(sender)
            .map_to_mm(|_| BlockchainScanStopped {})?;
        receiver
            .await
            .map(|(handle, rpc)| SaplingSyncRespawnGuard {
                sync_handle: Some((handle, rpc)),
                abort_handle: self.abort_handle.clone(),
            })
            .map_to_mm(|_| BlockchainScanStopped {})
    }
}

pub(super) struct SaplingSyncGuard<'a> {
    pub(super) _connector_guard: AsyncMutexGuard<'a, SaplingSyncConnector>,
    pub(super) respawn_guard: SaplingSyncRespawnGuard,
}
