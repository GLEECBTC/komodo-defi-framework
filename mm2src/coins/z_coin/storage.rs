use crate::z_coin::{ValidateBlocksError, ZcoinConsensusParams, ZcoinStorageError};
use mm2_event_stream::StreamingManager;

pub(crate) mod blockdb;
pub(crate) use blockdb::*;

pub(crate) mod walletdb;
pub(crate) use walletdb::*;

pub(crate) mod z_locked_notes;
pub(crate) use z_locked_notes::{LockedNotesStorage, LockedNotesStorageError};

#[cfg(target_arch = "wasm32")]
mod z_params;
#[cfg(target_arch = "wasm32")]
pub(crate) use z_params::ZcashParamsWasmImpl;

use mm2_err_handle::mm_error::MmResult;
#[cfg(target_arch = "wasm32")]
use walletdb::wasm::storage::DataConnStmtCacheWasm;
#[cfg(debug_assertions)]
use zcash_client_backend::data_api::error::Error;
use zcash_client_backend::data_api::PrunedBlock;
use zcash_client_backend::proto::compact_formats::CompactBlock;
use zcash_client_backend::wallet::{AccountId, WalletTx};
use zcash_client_backend::welding_rig::scan_block;
#[cfg(not(target_arch = "wasm32"))]
use zcash_client_sqlite::for_async::DataConnStmtCacheAsync;
use zcash_extras::{WalletRead, WalletWrite};
use zcash_primitives::block::BlockHash;
use zcash_primitives::consensus::BlockHeight;
use zcash_primitives::merkle_tree::CommitmentTree;
use zcash_primitives::sapling::Nullifier;
use zcash_primitives::zip32::ExtendedFullViewingKey;

pub type ZcoinStorageRes<T> = MmResult<T, ZcoinStorageError>;

#[derive(Clone)]
pub struct DataConnStmtCacheWrapper {
    #[cfg(not(target_arch = "wasm32"))]
    cache: DataConnStmtCacheAsync<ZcoinConsensusParams>,
    #[cfg(target_arch = "wasm32")]
    cache: DataConnStmtCacheWasm,
}

impl DataConnStmtCacheWrapper {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(cache: DataConnStmtCacheAsync<ZcoinConsensusParams>) -> Self {
        Self { cache }
    }
    #[cfg(target_arch = "wasm32")]
    pub fn new(cache: DataConnStmtCacheWasm) -> Self {
        Self { cache }
    }
    #[cfg(not(target_arch = "wasm32"))]
    #[inline]
    pub fn inner(&self) -> &DataConnStmtCacheAsync<ZcoinConsensusParams> {
        &self.cache
    }
    #[cfg(target_arch = "wasm32")]
    #[inline]
    pub fn inner(&self) -> &DataConnStmtCacheWasm {
        &self.cache
    }
}

pub struct CompactBlockRow {
    pub(crate) height: BlockHeight,
    pub(crate) data: Vec<u8>,
}

#[derive(Clone)]
pub enum BlockProcessingMode {
    Validate,
    Scan(DataConnStmtCacheWrapper, StreamingManager),
}

/// Checks that the scanned blocks in the data database, when combined with the recent
/// `CompactBlock`s in the cache database, form a valid chain.
///
/// This function is built on the core assumption that the information provided in the
/// cache database is more likely to be accurate than the previously-scanned information.
/// This follows from the design (and trust) assumption that the `lightwalletd` server
/// provides accurate block information as of the time it was requested.
///
pub async fn validate_chain(
    block: CompactBlock,
    prev_height: &mut BlockHeight,
    prev_hash: &mut Option<BlockHash>,
) -> Result<(), ValidateBlocksError> {
    let current_height = block.height();
    if current_height != *prev_height + 1 {
        Err(ValidateBlocksError::block_height_discontinuity(
            *prev_height + 1,
            current_height,
        ))
    } else if prev_hash.is_none() || (prev_hash.as_ref() == Some(&block.prev_hash())) {
        Ok(())
    } else {
        Err(ValidateBlocksError::prev_hash_mismatch(current_height))
    }?;

    *prev_height = current_height;
    *prev_hash = Some(block.hash());

    Ok(())
}

/// Scans new blocks added to the cache for any transactions received by
/// the tracked accounts.
///
/// This function returns without error after scanning new blocks, allowing
/// the caller to update their UI with scanning progress. Repeatedly calling this
/// function will process sequential ranges of blocks.
///
/// The function focuses on cached blocks with heights greater than the
/// highest scanned block in `data`. Cached blocks with lower heights are not
/// verified against previously-scanned blocks. This function **assumes** that
/// the caller is handling rollbacks.
///
/// For brand-new light client databases, the function starts scanning from the
/// Sapling activation height. This height can be fast-forwarded to a more recent
/// block by initializing the client database with a starting block (e.g., calling
/// `init_blocks_table` before this function if using `zcash_client_sqlite`).
///
/// Scanned blocks are required to be height-sequential. If a block is missing from
/// the cache, an error will be returned with kind [`ChainInvalid::BlockHeightDiscontinuity`].
///
pub async fn scan_cached_block(
    data: &DataConnStmtCacheWrapper,
    params: &ZcoinConsensusParams,
    block: &CompactBlock,
    locked_notes_db: &LockedNotesStorage,
    last_height: &mut BlockHeight,
) -> Result<Vec<WalletTx<Nullifier>>, ValidateBlocksError> {
    let mut data_guard = data.inner().clone();
    // Fetch the ExtendedFullViewingKeys we are tracking
    let extfvks = data_guard.get_extended_full_viewing_keys().await?;
    let extfvks: Vec<(&AccountId, &ExtendedFullViewingKey)> = extfvks.iter().collect();

    // Get the most recent CommitmentTree
    let mut tree = data_guard
        .get_commitment_tree(*last_height)
        .await
        .map(|t| t.unwrap_or_else(CommitmentTree::empty))?;
    // Get most recent incremental witnesses for the notes we are tracking
    let mut witnesses = data_guard.get_witnesses(*last_height).await?;

    // Get the nullifiers for the notes we are tracking
    let mut nullifiers = data_guard.get_nullifiers().await?;

    let current_height = block.height();
    // Scanned blocks MUST be height-sequential.
    if current_height != (*last_height + 1) {
        return Err(ValidateBlocksError::block_height_discontinuity(
            *last_height + 1,
            current_height,
        ));
    }

    let txs: Vec<WalletTx<Nullifier>> = {
        let mut witness_refs: Vec<_> = witnesses.iter_mut().map(|w| &mut w.1).collect();
        scan_block(
            params,
            block.clone(),
            &extfvks,
            &nullifiers,
            &mut tree,
            &mut witness_refs[..],
        )
    };

    // To enforce that all roots match,
    // see -> https://github.com/KomodoPlatform/librustzcash/blob/e92443a7bbd1c5e92e00e6deb45b5a33af14cea4/zcash_client_backend/src/data_api/chain.rs#L304-L326
    let new_witnesses = data_guard
        .advance_by_block(
            &(PrunedBlock {
                block_height: current_height,
                block_hash: BlockHash::from_slice(&block.hash),
                block_time: block.time,
                commitment_tree: &tree,
                transactions: &txs,
            }),
            &witnesses,
        )
        .await?;

    let spent_nf: Vec<Nullifier> = txs
        .iter()
        .flat_map(|tx| tx.shielded_spends.iter().map(|spend| spend.nf))
        .collect();
    nullifiers.retain(|(_, nf)| !spent_nf.contains(nf));
    nullifiers.extend(
        txs.iter()
            .flat_map(|tx| tx.shielded_outputs.iter().map(|out| (out.account, out.nf))),
    );

    witnesses.extend(new_witnesses);
    *last_height = current_height;

    // TODO: Execute updates to `locked_notes_db` and `wallet_db` in a single transaction.
    // This will be possible with a newer librustzcash that supports both spent notes and unconfirmed change tracking.
    // See: https://github.com/KomodoPlatform/komodo-defi-framework/pull/2331#pullrequestreview-2883773336
    for tx in &txs {
        locked_notes_db
            .remove_notes_for_txid(tx.txid.to_string())
            .await
            .map_err(|err| ValidateBlocksError::DbError(err.to_string()))?;
    }

    Ok(txs)
}
