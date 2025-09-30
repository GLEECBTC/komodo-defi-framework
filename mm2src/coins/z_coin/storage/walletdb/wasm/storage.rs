use crate::z_coin::storage::walletdb::wasm::tables::{
    WalletDbAccountsTable, WalletDbBlocksTable, WalletDbReceivedNotesTable, WalletDbSaplingWitnessesTable,
    WalletDbSentNotesTable, WalletDbTransactionsTable,
};
use crate::z_coin::storage::wasm::{to_spendable_note, SpendableNoteConstructor};
use crate::z_coin::storage::ZcoinStorageRes;
use crate::z_coin::z_coin_errors::ZcoinStorageError;
use crate::z_coin::{CheckPointBlockInfo, WalletDbShared, ZCoinBuilder, ZcoinConsensusParams};

use async_trait::async_trait;
use common::log::info;
use ff::PrimeField;
use mm2_core::mm_ctx::MmArc;
use mm2_db::indexed_db::{
    ConstructibleDb, DbIdentifier, DbInstance, DbLocked, IndexedDb, IndexedDbBuilder, InitDbResult, MultiIndex,
    SharedDb,
};
use mm2_err_handle::prelude::*;
use mm2_number::num_bigint::ToBigInt;
use mm2_number::BigInt;
use num_traits::{FromPrimitive, ToPrimitive};
use std::collections::HashMap;
use std::convert::TryFrom;
use std::ops::Deref;
use zcash_client_backend::address::RecipientAddress;
use zcash_client_backend::data_api::{PrunedBlock, ReceivedTransaction, SentTransaction};
use zcash_client_backend::encoding::{
    decode_extended_full_viewing_key, decode_payment_address, encode_extended_full_viewing_key, encode_payment_address,
};
use zcash_client_backend::wallet::{AccountId, SpendableNote, WalletTx};
use zcash_client_backend::DecryptedOutput;
use zcash_extras::{NoteId, ShieldedOutput, WalletRead, WalletWrite};
use zcash_primitives::block::BlockHash;
use zcash_primitives::consensus::{BlockHeight, NetworkUpgrade, Parameters};
use zcash_primitives::memo::{Memo, MemoBytes};
use zcash_primitives::merkle_tree::{CommitmentTree, IncrementalWitness};
use zcash_primitives::sapling::{Node, Nullifier, PaymentAddress};
use zcash_primitives::transaction::components::Amount;
use zcash_primitives::transaction::{Transaction, TxId};
use zcash_primitives::zip32::ExtendedFullViewingKey;

const DB_NAME: &str = "wallet_db_cache";
const DB_VERSION: u32 = 1;

pub type WalletDbInnerLocked<'a> = DbLocked<'a, WalletDbInner>;

macro_rules! num_to_bigint {
    ($value: ident) => {
        $value.to_bigint().ok_or_else(|| {
            $crate::z_coin::z_coin_errors::ZcoinStorageError::CorruptedData(
                "Number is too large to fit in a BigInt".to_string(),
            )
        })
    };
}

impl WalletDbShared {
    pub async fn new(
        builder: &ZCoinBuilder<'_>,
        checkpoint_block: Option<CheckPointBlockInfo>,
        continue_from_prev_sync: bool,
    ) -> ZcoinStorageRes<Self> {
        let ticker = builder.ticker;
        let consensus_params = builder.protocol_info.consensus_params.clone();
        let db = WalletIndexedDb::new(builder.ctx, ticker, consensus_params).await?;
        let extrema = db.block_height_extrema().await?;
        let get_evk = db.get_extended_full_viewing_keys().await?;
        let evk = ExtendedFullViewingKey::from(&builder.z_spending_key);
        let min_sync_height = extrema.map(|(min, _)| u32::from(min));
        let init_block_height = checkpoint_block.clone().map(|block| block.height);

        if get_evk.is_empty() || (!continue_from_prev_sync && init_block_height != min_sync_height) {
            // let user know we're clearing cache and resyncing from new provided height.
            if min_sync_height.unwrap_or(0) > 0 {
                info!("Older/Newer sync height detected!, rewinding walletdb to new height: {init_block_height:?}");
            }
            db.rewind_to_height(BlockHeight::from(u32::MIN)).await?;
            if let Some(block) = checkpoint_block {
                db.init_blocks_table(
                    BlockHeight::from_u32(block.height),
                    BlockHash(block.hash.0),
                    block.time,
                    &block.sapling_tree.0,
                )
                .await?;
            }
        }

        if get_evk.is_empty() {
            db.init_accounts_table(&[evk]).await?;
        };

        Ok(Self {
            db,
            ticker: ticker.to_string(),
        })
    }

    pub async fn is_tx_imported(&self, tx_id: TxId) -> MmResult<bool, ZcoinStorageError> {
        self.db.is_tx_imported(tx_id).await
    }
}

pub struct WalletDbInner(pub IndexedDb);

impl WalletDbInner {
    pub fn get_inner(&self) -> &IndexedDb {
        &self.0
    }
}

#[async_trait]
impl DbInstance for WalletDbInner {
    const DB_NAME: &'static str = DB_NAME;

    async fn init(db_id: DbIdentifier) -> InitDbResult<Self> {
        Ok(Self(
            IndexedDbBuilder::new(db_id)
                .with_version(DB_VERSION)
                .with_table::<WalletDbAccountsTable>()
                .with_table::<WalletDbBlocksTable>()
                .with_table::<WalletDbSaplingWitnessesTable>()
                .with_table::<WalletDbSentNotesTable>()
                .with_table::<WalletDbTransactionsTable>()
                .with_table::<WalletDbReceivedNotesTable>()
                .build()
                .await?,
        ))
    }
}

#[derive(Clone)]
pub struct WalletIndexedDb {
    pub db: SharedDb<WalletDbInner>,
    pub ticker: String,
    pub params: ZcoinConsensusParams,
}

impl WalletIndexedDb {
    pub async fn new(
        ctx: &MmArc,
        ticker: &str,
        consensus_params: ZcoinConsensusParams,
    ) -> MmResult<Self, ZcoinStorageError> {
        let db = Self {
            db: ConstructibleDb::new(ctx).into_shared(),
            ticker: ticker.to_string(),
            params: consensus_params,
        };

        Ok(db)
    }

    pub(crate) async fn lock_db(&self) -> ZcoinStorageRes<WalletDbInnerLocked<'_>> {
        self.db
            .get_or_initialize()
            .await
            .mm_err(|err| ZcoinStorageError::DbError(err.to_string()))
    }

    pub async fn is_tx_imported(&self, tx_id: TxId) -> ZcoinStorageRes<bool> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;

        let tx_table = db_transaction.table::<WalletDbTransactionsTable>().await.map_mm_err()?;
        let index_keys = MultiIndex::new(WalletDbTransactionsTable::TICKER_TXID_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(tx_id.0.to_vec())
            .map_mm_err()?;
        let maybe_tx = tx_table.get_items_by_multi_index(index_keys).await.map_mm_err()?;

        if !maybe_tx.is_empty() {
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn get_update_ops(&self) -> MmResult<DataConnStmtCacheWasm, ZcoinStorageError> {
        Ok(DataConnStmtCacheWasm(self.clone()))
    }

    pub(crate) async fn init_accounts_table(&self, extfvks: &[ExtendedFullViewingKey]) -> ZcoinStorageRes<()> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let walletdb_account_table = db_transaction.table::<WalletDbAccountsTable>().await.map_mm_err()?;

        // check if account exists
        let maybe_min_account = walletdb_account_table
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .bound("height", 0u32, u32::MAX)
            .where_first()
            .open_cursor(WalletDbAccountsTable::TICKER_ACCOUNT_INDEX)
            .await
            .map_mm_err()?
            .next()
            .await
            .map_mm_err()?;
        if maybe_min_account.is_some() {
            return MmError::err(ZcoinStorageError::TableNotEmpty(
                "Account table is not empty".to_string(),
            ));
        }

        // Insert accounts
        for (account, extfvk) in extfvks.iter().enumerate() {
            let account_int = num_to_bigint!(account)?;

            let address = extfvk.default_address().unwrap().1;
            let address = encode_payment_address(self.params.hrp_sapling_payment_address(), &address);

            let account = WalletDbAccountsTable {
                account: account_int.clone(),
                extfvk: encode_extended_full_viewing_key(self.params.hrp_sapling_extended_full_viewing_key(), extfvk),
                address,
                ticker: self.ticker.clone(),
            };

            let index_keys = MultiIndex::new(WalletDbAccountsTable::TICKER_ACCOUNT_INDEX)
                .with_value(&self.ticker)
                .map_mm_err()?
                .with_value(account_int)
                .map_mm_err()?;

            walletdb_account_table
                .replace_item_by_unique_multi_index(index_keys, &account)
                .await
                .map_mm_err()?;
        }

        Ok(())
    }

    pub(crate) async fn init_blocks_table(
        &self,
        height: BlockHeight,
        hash: BlockHash,
        time: u32,
        sapling_tree: &[u8],
    ) -> ZcoinStorageRes<()> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let walletdb_account_table = db_transaction.table::<WalletDbBlocksTable>().await.map_mm_err()?;

        // check if account exists
        let maybe_min_account = walletdb_account_table
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .bound("height", 0u32, u32::MAX)
            .where_first()
            .open_cursor(WalletDbBlocksTable::TICKER_HEIGHT_INDEX)
            .await
            .map_mm_err()?
            .next()
            .await
            .map_mm_err()?;
        if maybe_min_account.is_some() {
            return MmError::err(ZcoinStorageError::TableNotEmpty(
                "Account table is not empty".to_string(),
            ));
        }

        let block = WalletDbBlocksTable {
            height: u32::from(height),
            hash: hash.0.to_vec(),
            time,
            sapling_tree: sapling_tree.to_vec(),
            ticker: self.ticker.clone(),
        };
        let walletdb_blocks_table = db_transaction.table::<WalletDbBlocksTable>().await.map_mm_err()?;
        let height = u32::from(height);
        let index_keys = MultiIndex::new(WalletDbBlocksTable::TICKER_HEIGHT_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(num_to_bigint!(height)?)
            .map_mm_err()?;

        walletdb_blocks_table
            .replace_item_by_unique_multi_index(index_keys, &block)
            .await
            .map_mm_err()?;

        Ok(())
    }
}

impl WalletIndexedDb {
    pub async fn insert_block(
        &self,
        block_height: BlockHeight,
        block_hash: BlockHash,
        block_time: u32,
        commitment_tree: &CommitmentTree<Node>,
    ) -> ZcoinStorageRes<()> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let walletdb_blocks_table = db_transaction.table::<WalletDbBlocksTable>().await.map_mm_err()?;

        let mut encoded_tree = Vec::new();
        commitment_tree.write(&mut encoded_tree).unwrap();

        let hash = &block_hash.0[..];
        let block = WalletDbBlocksTable {
            height: u32::from(block_height),
            hash: hash.to_vec(),
            time: block_time,
            sapling_tree: encoded_tree,
            ticker: self.ticker.clone(),
        };

        let index_keys = MultiIndex::new(WalletDbBlocksTable::TICKER_HEIGHT_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(u32::from(block_height))
            .map_mm_err()?;

        walletdb_blocks_table
            .replace_item_by_unique_multi_index(index_keys, &block)
            .await
            .map(|_| ())
            .map_mm_err()
    }

    pub async fn get_balance(&self, account: AccountId) -> ZcoinStorageRes<Amount> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let rec_note_table = db_transaction
            .table::<WalletDbReceivedNotesTable>()
            .await
            .map_mm_err()?;

        let index_keys = MultiIndex::new(WalletDbReceivedNotesTable::TICKER_ACCOUNT_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(account.0.to_bigint().unwrap())
            .map_mm_err()?;
        let maybe_notes = rec_note_table.get_items_by_multi_index(index_keys).await.map_mm_err()?;

        let tx_table = db_transaction.table::<WalletDbTransactionsTable>().await.map_mm_err()?;
        let txs = tx_table.get_items("ticker", &self.ticker).await.map_mm_err()?;

        let balance: i64 = maybe_notes
            .iter()
            .map(|(_, note)| {
                txs.iter()
                    .filter_map(|(tx_id, tx)| {
                        if *tx_id == note.tx && note.spent.is_none() && tx.block.is_some() {
                            Some(note.value.to_i64().expect("BigInt is too large to fit in an i64"))
                        } else {
                            None
                        }
                    })
                    .sum::<i64>()
            })
            .sum();

        match Amount::from_i64(balance) {
            Ok(amount) if !amount.is_negative() => Ok(amount),
            _ => MmError::err(ZcoinStorageError::CorruptedData(
                "Sum of values in received_notes is out of range".to_string(),
            )),
        }
    }

    pub async fn put_tx_data(&self, tx: &Transaction, created_at: Option<String>) -> ZcoinStorageRes<i64> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let tx_table = db_transaction.table::<WalletDbTransactionsTable>().await.map_mm_err()?;

        let mut raw_tx = vec![];
        tx.write(&mut raw_tx).unwrap();
        let txid = tx.txid().0.to_vec();

        let index_keys = MultiIndex::new(WalletDbTransactionsTable::TICKER_TXID_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(&txid)
            .map_mm_err()?;
        let single_tx = tx_table.get_item_by_unique_multi_index(index_keys).await.map_mm_err()?;
        if let Some((id_tx, some_tx)) = single_tx {
            let updated_tx = WalletDbTransactionsTable {
                txid: txid.clone(),
                created: some_tx.created,
                block: some_tx.block,
                tx_index: some_tx.tx_index,
                expiry_height: Some(u32::from(tx.expiry_height)),
                raw: Some(raw_tx),
                ticker: self.ticker.clone(),
            };
            tx_table.replace_item(id_tx, &updated_tx).await.map_mm_err()?;

            return Ok(id_tx as i64);
        };

        let new_tx = WalletDbTransactionsTable {
            txid: txid.clone(),
            created: created_at,
            block: None,
            tx_index: None,
            expiry_height: Some(u32::from(tx.expiry_height)),
            raw: Some(raw_tx),
            ticker: self.ticker.clone(),
        };
        let index_keys = MultiIndex::new(WalletDbTransactionsTable::TICKER_TXID_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(txid)
            .map_mm_err()?;

        Ok(tx_table
            .replace_item_by_unique_multi_index(index_keys, &new_tx)
            .await
            .map_mm_err()?
            .into())
    }

    pub async fn put_tx_meta<N>(&self, tx: &WalletTx<N>, height: BlockHeight) -> ZcoinStorageRes<i64> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let tx_table = db_transaction.table::<WalletDbTransactionsTable>().await.map_mm_err()?;

        let txid = tx.txid.0.to_vec();
        let index_keys = MultiIndex::new(WalletDbTransactionsTable::TICKER_TXID_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(&txid)
            .map_mm_err()?;
        let single_tx = tx_table.get_item_by_unique_multi_index(index_keys).await.map_mm_err()?;

        if let Some((id_tx, some_tx)) = single_tx {
            let updated_tx = WalletDbTransactionsTable {
                txid: some_tx.txid.clone(),
                created: some_tx.created,
                block: Some(u32::from(height)),
                tx_index: Some(tx.index as i64),
                expiry_height: some_tx.expiry_height,
                raw: some_tx.raw,
                ticker: self.ticker.clone(),
            };
            tx_table.replace_item(id_tx, &updated_tx).await.map_mm_err()?;

            return Ok(id_tx as i64);
        };

        let new_tx = WalletDbTransactionsTable {
            txid: txid.clone(),
            created: None,
            block: Some(u32::from(height)),
            tx_index: Some(tx.index as i64),
            expiry_height: None,
            raw: None,
            ticker: self.ticker.clone(),
        };
        let index_keys = MultiIndex::new(WalletDbTransactionsTable::TICKER_TXID_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(txid)
            .map_mm_err()?;

        Ok(tx_table
            .replace_item_by_unique_multi_index(index_keys, &new_tx)
            .await
            .map_mm_err()?
            .into())
    }

    pub async fn mark_spent(&self, tx_ref: i64, nf: &Nullifier) -> ZcoinStorageRes<()> {
        let ticker = self.ticker.clone();
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let received_notes_table = db_transaction
            .table::<WalletDbReceivedNotesTable>()
            .await
            .map_mm_err()?;

        let index_keys = MultiIndex::new(WalletDbReceivedNotesTable::TICKER_NF_INDEX)
            .with_value(&ticker)
            .map_mm_err()?
            .with_value(nf.0.to_vec())
            .map_mm_err()?;
        let maybe_note = received_notes_table
            .get_item_by_unique_multi_index(index_keys)
            .await
            .map_mm_err()?;

        if let Some((id, note)) = maybe_note {
            let new_received_note = WalletDbReceivedNotesTable {
                tx: note.tx,
                output_index: note.output_index,
                account: note.account,
                diversifier: note.diversifier,
                value: note.value,
                rcm: note.rcm,
                nf: note.nf,
                is_change: note.is_change,
                memo: note.memo,
                spent: Some(num_to_bigint!(tx_ref)?),
                ticker,
            };
            received_notes_table
                .replace_item(id, &new_received_note)
                .await
                .map_mm_err()?;

            return Ok(());
        }

        MmError::err(ZcoinStorageError::GetFromStorageError("note not found".to_string()))
    }

    pub async fn put_received_note<T: ShieldedOutput>(&self, output: &T, tx_ref: i64) -> ZcoinStorageRes<NoteId> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;

        let rcm = output.note().rcm().to_repr();
        let account = BigInt::from(output.account().0);
        let diversifier = output.to().diversifier().0.to_vec();
        let value = output.note().value.into();
        let rcm = rcm.to_vec();
        let memo = output.memo().map(|m| m.as_slice().to_vec());
        let is_change = output.is_change();
        let tx = tx_ref as u32;
        let output_index = output.index() as u32;
        let nf_bytes = output.nullifier().map(|nf| nf.0.to_vec());

        let received_note_table = db_transaction
            .table::<WalletDbReceivedNotesTable>()
            .await
            .map_mm_err()?;
        let index_keys = MultiIndex::new(WalletDbReceivedNotesTable::TICKER_TX_OUTPUT_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(tx)
            .map_mm_err()?
            .with_value(output_index)
            .map_mm_err()?;
        let current_note = received_note_table
            .get_item_by_unique_multi_index(index_keys)
            .await
            .map_mm_err()?;

        let id = if let Some((id, note)) = current_note {
            let temp_note = WalletDbReceivedNotesTable {
                tx,
                output_index,
                account: note.account,
                diversifier,
                value,
                rcm,
                nf: note.nf.or(nf_bytes),
                is_change: note.is_change.or(is_change),
                memo: note.memo.or(memo),
                spent: note.spent,
                ticker: self.ticker.clone(),
            };
            received_note_table.replace_item(id, &temp_note).await.map_mm_err()?
        } else {
            let new_note = WalletDbReceivedNotesTable {
                tx,
                output_index,
                account,
                diversifier,
                value,
                rcm,
                nf: nf_bytes,
                is_change,
                memo,
                spent: None,
                ticker: self.ticker.clone(),
            };

            let index_keys = MultiIndex::new(WalletDbReceivedNotesTable::TICKER_TX_OUTPUT_INDEX)
                .with_value(&self.ticker)
                .map_mm_err()?
                .with_value(tx)
                .map_mm_err()?
                .with_value(num_to_bigint!(output_index)?)
                .map_mm_err()?;
            received_note_table
                .replace_item_by_unique_multi_index(index_keys, &new_note)
                .await
                .map_mm_err()?
        };

        Ok(NoteId::ReceivedNoteId(id.into()))
    }

    pub async fn insert_witness(
        &self,
        note_id: i64,
        witness: &IncrementalWitness<Node>,
        height: BlockHeight,
    ) -> ZcoinStorageRes<()> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let witness_table = db_transaction
            .table::<WalletDbSaplingWitnessesTable>()
            .await
            .map_mm_err()?;

        let mut encoded = Vec::new();
        witness.write(&mut encoded).unwrap();

        let note_id_int = BigInt::from_i64(note_id).unwrap();
        let witness = WalletDbSaplingWitnessesTable {
            note: note_id_int,
            block: u32::from(height),
            witness: encoded,
            ticker: self.ticker.clone(),
        };

        witness_table.add_item(&witness).await.map(|_| ()).map_mm_err()
    }

    pub async fn prune_witnesses(&self, below_height: BlockHeight) -> ZcoinStorageRes<()> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let witness_table = db_transaction
            .table::<WalletDbSaplingWitnessesTable>()
            .await
            .map_mm_err()?;

        let mut maybe_witness = witness_table
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .bound("block", 0u32, (below_height - 1).into())
            .open_cursor(WalletDbSaplingWitnessesTable::TICKER_BLOCK_INDEX)
            .await
            .map_mm_err()?;

        while let Some((id, _)) = maybe_witness.next().await.map_mm_err()? {
            witness_table.delete_item(id).await.map_mm_err()?;
        }

        Ok(())
    }

    pub async fn update_expired_notes(&self, height: BlockHeight) -> ZcoinStorageRes<()> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        // fetch received_notes.
        let received_notes_table = db_transaction
            .table::<WalletDbReceivedNotesTable>()
            .await
            .map_mm_err()?;
        let maybe_notes = received_notes_table
            .get_items("ticker", &self.ticker)
            .await
            .map_mm_err()?;

        // fetch transactions with block < height .
        let txs_table = db_transaction.table::<WalletDbTransactionsTable>().await.map_mm_err()?;
        let mut maybe_txs = txs_table
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .bound("expiry_height", 0u32, u32::from(height - 1))
            .reverse()
            .open_cursor(WalletDbTransactionsTable::TICKER_EXP_HEIGHT_INDEX)
            .await
            .map_mm_err()?;

        while let Some((id, note)) = maybe_txs.next().await.map_mm_err()? {
            if note.block.is_none() {
                if let Some(curr) = maybe_notes.iter().find(|(_, n)| n.spent == id.to_bigint()) {
                    let temp_note = WalletDbReceivedNotesTable {
                        tx: curr.1.tx,
                        output_index: curr.1.output_index,
                        account: curr.1.account.clone(),
                        diversifier: curr.1.diversifier.clone(),
                        value: curr.1.value.clone(),
                        rcm: curr.1.rcm.clone(),
                        nf: curr.1.nf.clone(),
                        is_change: curr.1.is_change,
                        memo: curr.1.memo.clone(),
                        spent: None,
                        ticker: self.ticker.clone(),
                    };

                    received_notes_table
                        .replace_item(curr.0, &temp_note)
                        .await
                        .map_mm_err()?;
                }
            };
        }

        Ok(())
    }

    pub async fn put_sent_note(&self, output: &DecryptedOutput, tx_ref: i64) -> ZcoinStorageRes<()> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;

        let tx_ref = num_to_bigint!(tx_ref)?;
        let output_index = output.index;
        let output_index = num_to_bigint!(output_index)?;
        let from_account = output.account.0;
        let from_account = num_to_bigint!(from_account)?;
        let value = output.note.value;
        let value = num_to_bigint!(value)?;
        let address = encode_payment_address(self.params.hrp_sapling_payment_address(), &output.to);

        let sent_note_table = db_transaction.table::<WalletDbSentNotesTable>().await.map_mm_err()?;
        let index_keys = MultiIndex::new(WalletDbSentNotesTable::TICKER_TX_OUTPUT_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(&tx_ref)
            .map_mm_err()?
            .with_value(&output_index)
            .map_mm_err()?;
        let maybe_note = sent_note_table
            .get_item_by_unique_multi_index(index_keys)
            .await
            .map_mm_err()?;

        let update_note = WalletDbSentNotesTable {
            tx: tx_ref.clone(),
            output_index: output_index.clone(),
            from_account,
            address,
            value,
            memo: Some(output.memo.as_slice().to_vec()),
            ticker: self.ticker.clone(),
        };
        if let Some((id, _)) = maybe_note {
            sent_note_table.replace_item(id, &update_note).await.map_mm_err()?;
        } else {
            let index_keys = MultiIndex::new(WalletDbReceivedNotesTable::TICKER_TX_OUTPUT_INDEX)
                .with_value(&self.ticker)
                .map_mm_err()?
                .with_value(tx_ref)
                .map_mm_err()?
                .with_value(output_index)
                .map_mm_err()?;
            sent_note_table
                .replace_item_by_unique_multi_index(index_keys, &update_note)
                .await
                .map_mm_err()?;
        }

        Ok(())
    }

    pub async fn insert_sent_note(
        &self,
        tx_ref: i64,
        output_index: usize,
        account: AccountId,
        to: &RecipientAddress,
        value: Amount,
        memo: Option<&MemoBytes>,
    ) -> ZcoinStorageRes<()> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let sent_note_table = db_transaction.table::<WalletDbSentNotesTable>().await.map_mm_err()?;

        let tx_ref = num_to_bigint!(tx_ref)?;
        let output_index = num_to_bigint!(output_index)?;
        let from_account = account.0;
        let from_account = num_to_bigint!(from_account)?;
        let value = i64::from(value);
        let value = num_to_bigint!(value)?;
        let address = to.encode(&self.params);
        let new_note = WalletDbSentNotesTable {
            tx: tx_ref.clone(),
            output_index: output_index.clone(),
            from_account,
            address,
            value,
            memo: memo.map(|m| m.as_slice().to_vec()),
            ticker: self.ticker.clone(),
        };
        let index_keys = MultiIndex::new(WalletDbReceivedNotesTable::TICKER_TX_OUTPUT_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(tx_ref)
            .map_mm_err()?
            .with_value(output_index)
            .map_mm_err()?;

        sent_note_table
            .replace_item_by_unique_multi_index(index_keys, &new_note)
            .await
            .map(|_| ())
            .map_mm_err()
    }

    /// Asynchronously rewinds the storage to a specified block height, effectively
    /// removing data beyond the specified height from the storage.    
    pub async fn rewind_to_height(&self, block_height: BlockHeight) -> ZcoinStorageRes<()> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;

        let block_height = u32::from(block_height);

        // Recall where we synced up to previously.
        let blocks_table = db_transaction.table::<WalletDbBlocksTable>().await.map_mm_err()?;
        let maybe_height = blocks_table
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .bound("height", 0u32, u32::MAX)
            .reverse()
            .where_first()
            .open_cursor(WalletDbBlocksTable::TICKER_HEIGHT_INDEX)
            .await
            .map_mm_err()?
            .next()
            .await
            .map_mm_err()?
            .map(|(_, item)| {
                item.height
                    .to_u32()
                    .ok_or_else(|| ZcoinStorageError::GetFromStorageError("height is too large".to_string()))
            })
            .transpose()?;
        let sapling_activation_height = self
            .params
            .activation_height(NetworkUpgrade::Sapling)
            .ok_or_else(|| ZcoinStorageError::BackendError("Sapling not active".to_string()))?;
        let maybe_height = maybe_height.unwrap_or_else(|| (sapling_activation_height - 1).into());

        if block_height >= maybe_height {
            return Ok(());
        };

        // Decrement witnesses.
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let witnesses_table = db_transaction
            .table::<WalletDbSaplingWitnessesTable>()
            .await
            .map_mm_err()?;
        let maybe_witnesses_cursor = witnesses_table
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .bound("block", block_height + 1, u32::MAX)
            .open_cursor(WalletDbSaplingWitnessesTable::TICKER_BLOCK_INDEX)
            .await
            .map_mm_err()?
            .collect()
            .await
            .map_mm_err()?;

        for (id, _witness) in maybe_witnesses_cursor {
            witnesses_table.delete_item(id).await.map_mm_err()?;
        }

        // Un-mine transactions.
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let transactions_table = db_transaction.table::<WalletDbTransactionsTable>().await.map_mm_err()?;
        let mut maybe_txs_cursor = transactions_table
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .bound("block", block_height + 1, u32::MAX)
            .open_cursor(WalletDbTransactionsTable::TICKER_BLOCK_INDEX)
            .await
            .map_mm_err()?;
        while let Some((_, tx)) = maybe_txs_cursor.next().await.map_mm_err()? {
            let modified_tx = WalletDbTransactionsTable {
                txid: tx.txid.clone(),
                created: tx.created.clone(),
                block: None,
                tx_index: None,
                expiry_height: tx.expiry_height,
                raw: tx.raw.clone(),
                ticker: self.ticker.clone(),
            };
            let index_keys = MultiIndex::new(WalletDbTransactionsTable::TICKER_TXID_INDEX)
                .with_value(&self.ticker)
                .map_mm_err()?
                .with_value(tx.txid)
                .map_mm_err()?;
            transactions_table
                .replace_item_by_unique_multi_index(index_keys, &modified_tx)
                .await
                .map_mm_err()?;
        }

        // Now that they aren't depended on, delete scanned blocks.
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let blocks_table = db_transaction.table::<WalletDbBlocksTable>().await.map_mm_err()?;
        let maybe_blocks = blocks_table
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .bound("height", block_height + 1, u32::MAX)
            .open_cursor(WalletDbBlocksTable::TICKER_HEIGHT_INDEX)
            .await
            .map_mm_err()?
            .collect()
            .await
            .map_mm_err()?;

        for (_, block) in maybe_blocks {
            let index_keys = MultiIndex::new(WalletDbBlocksTable::TICKER_HEIGHT_INDEX)
                .with_value(&self.ticker)
                .map_mm_err()?
                .with_value(block.height)
                .map_mm_err()?;
            blocks_table
                .delete_item_by_unique_multi_index(index_keys)
                .await
                .map_mm_err()?;
        }

        Ok(())
    }
}

#[async_trait]
impl WalletRead for WalletIndexedDb {
    type Error = MmError<ZcoinStorageError>;
    type NoteRef = NoteId;
    type TxRef = i64;

    async fn block_height_extrema(&self) -> Result<Option<(BlockHeight, BlockHeight)>, Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let block_headers_db = db_transaction.table::<WalletDbBlocksTable>().await.map_mm_err()?;
        let earliest_block = block_headers_db
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .bound("height", 0u32, u32::MAX)
            .where_first()
            .open_cursor(WalletDbBlocksTable::TICKER_HEIGHT_INDEX)
            .await
            .map_mm_err()?
            .next()
            .await
            .map_mm_err()?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let block_headers_db = db_transaction.table::<WalletDbBlocksTable>().await.map_mm_err()?;
        let latest_block = block_headers_db
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .bound("height", 0u32, u32::MAX)
            .reverse()
            .where_first()
            .open_cursor(WalletDbBlocksTable::TICKER_HEIGHT_INDEX)
            .await
            .map_mm_err()?
            .next()
            .await
            .map_mm_err()?;

        if let (Some(min), Some(max)) = (earliest_block, latest_block) {
            Ok(Some((BlockHeight::from(min.1.height), BlockHeight::from(max.1.height))))
        } else {
            Ok(None)
        }
    }

    async fn get_block_hash(&self, block_height: BlockHeight) -> Result<Option<BlockHash>, Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let block_headers_db = db_transaction.table::<WalletDbBlocksTable>().await.map_mm_err()?;
        let index_keys = MultiIndex::new(WalletDbBlocksTable::TICKER_HEIGHT_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(u32::from(block_height))
            .map_mm_err()?;

        Ok(block_headers_db
            .get_item_by_unique_multi_index(index_keys)
            .await
            .map_mm_err()?
            .map(|(_, block)| BlockHash::from_slice(&block.hash[..])))
    }

    async fn get_tx_height(&self, txid: TxId) -> Result<Option<BlockHeight>, Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let block_headers_db = db_transaction.table::<WalletDbTransactionsTable>().await.map_mm_err()?;
        let index_keys = MultiIndex::new(WalletDbTransactionsTable::TICKER_TXID_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(txid.0.to_vec())
            .map_mm_err()?;

        Ok(block_headers_db
            .get_item_by_unique_multi_index(index_keys)
            .await
            .map_mm_err()?
            .and_then(|(_, tx)| tx.block.map(BlockHeight::from)))
    }

    async fn get_address(&self, account: AccountId) -> Result<Option<PaymentAddress>, Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let block_headers_db = db_transaction.table::<WalletDbAccountsTable>().await.map_mm_err()?;
        let account_num = account.0;
        let index_keys = MultiIndex::new(WalletDbAccountsTable::TICKER_ACCOUNT_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(num_to_bigint!(account_num)?)
            .map_mm_err()?;

        let address = block_headers_db
            .get_item_by_unique_multi_index(index_keys)
            .await
            .map_mm_err()?
            .map(|(_, account)| account.address)
            .ok_or_else(|| ZcoinStorageError::GetFromStorageError("Invalid account/not found".to_string()))?;

        decode_payment_address(self.params.hrp_sapling_payment_address(), &address).map_to_mm(|err| {
            ZcoinStorageError::DecodingError(format!(
                "Error occurred while decoding account address: {err:?} - ticker: {}",
                self.ticker
            ))
        })
    }

    async fn get_extended_full_viewing_keys(&self) -> Result<HashMap<AccountId, ExtendedFullViewingKey>, Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let accounts_table = db_transaction.table::<WalletDbAccountsTable>().await.map_mm_err()?;
        let maybe_accounts = accounts_table.get_items("ticker", &self.ticker).await.map_mm_err()?;

        let mut res_accounts: HashMap<AccountId, ExtendedFullViewingKey> = HashMap::with_capacity(maybe_accounts.len());
        for (_, account) in maybe_accounts {
            let extfvk =
                decode_extended_full_viewing_key(self.params.hrp_sapling_extended_full_viewing_key(), &account.extfvk)
                    .map_to_mm(|err| ZcoinStorageError::DecodingError(format!("{err:?} - ticker: {}", self.ticker)))
                    .and_then(|k| k.ok_or_else(|| MmError::new(ZcoinStorageError::IncorrectHrpExtFvk)));
            let acc_id = account
                .account
                .to_u32()
                .ok_or_else(|| ZcoinStorageError::GetFromStorageError("Invalid account id".to_string()))?;

            res_accounts.insert(AccountId(acc_id), extfvk?);
        }

        Ok(res_accounts)
    }

    async fn is_valid_account_extfvk(
        &self,
        account: AccountId,
        extfvk: &ExtendedFullViewingKey,
    ) -> Result<bool, Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let accounts_table = db_transaction.table::<WalletDbAccountsTable>().await.map_mm_err()?;
        let index_keys = MultiIndex::new(WalletDbAccountsTable::TICKER_ACCOUNT_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(account.0.to_bigint())
            .map_mm_err()?;

        let account = accounts_table
            .get_item_by_unique_multi_index(index_keys)
            .await
            .map_mm_err()?;

        if let Some((_, account)) = account {
            let expected =
                decode_extended_full_viewing_key(self.params.hrp_sapling_extended_full_viewing_key(), &account.extfvk)
                    .map_to_mm(|err| ZcoinStorageError::DecodingError(format!("{err:?} - ticker: {}", self.ticker)))
                    .and_then(|k| k.ok_or_else(|| MmError::new(ZcoinStorageError::IncorrectHrpExtFvk)))?;

            return Ok(&expected == extfvk);
        }

        Ok(false)
    }

    async fn get_balance_at(&self, account: AccountId, anchor_height: BlockHeight) -> Result<Amount, Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;

        let tx_table = db_transaction.table::<WalletDbTransactionsTable>().await.map_mm_err()?;
        // Retrieves a list of transaction IDs (txid) from the transactions table
        // that match the provided account ID.
        let txids = tx_table
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .bound("block", 0u32, u32::from(anchor_height))
            .open_cursor(WalletDbTransactionsTable::TICKER_BLOCK_INDEX)
            .await
            .map_mm_err()?
            .collect()
            .await
            .map_mm_err()?
            .into_iter()
            .map(|(id, _)| id)
            .collect::<Vec<_>>();

        let received_notes_table = db_transaction
            .table::<WalletDbReceivedNotesTable>()
            .await
            .map_mm_err()?;
        let index_keys = MultiIndex::new(WalletDbReceivedNotesTable::TICKER_ACCOUNT_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(account.0.to_bigint().unwrap())
            .map_mm_err()?;
        let maybe_notes = received_notes_table
            .get_items_by_multi_index(index_keys)
            .await
            .map_mm_err()?;

        let mut value: i64 = 0;
        for (_, note) in maybe_notes {
            if txids.contains(&note.tx) && note.spent.is_none() {
                value += note.value.to_i64().ok_or_else(|| {
                    MmError::new(ZcoinStorageError::GetFromStorageError("price is too large".to_string()))
                })?
            }
        }

        match Amount::from_i64(value) {
            Ok(amount) if !amount.is_negative() => Ok(amount),
            _ => MmError::err(ZcoinStorageError::CorruptedData(
                "Sum of values in received_notes is out of range".to_string(),
            )),
        }
    }

    async fn get_memo(&self, id_note: Self::NoteRef) -> Result<Memo, Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;

        let memo = match id_note {
            NoteId::SentNoteId(id_note) => {
                let sent_notes_table = db_transaction.table::<WalletDbSentNotesTable>().await.map_mm_err()?;
                let notes = sent_notes_table.get_items("ticker", &self.ticker).await.map_mm_err()?;
                notes
                    .into_iter()
                    .find(|(id, _)| *id as i64 == id_note)
                    .map(|(_, n)| n.memo)
            },
            NoteId::ReceivedNoteId(id_note) => {
                let received_notes_table = db_transaction.table::<WalletDbSentNotesTable>().await.map_mm_err()?;
                let notes = received_notes_table
                    .get_items("ticker", &self.ticker)
                    .await
                    .map_mm_err()?;
                notes
                    .into_iter()
                    .find(|(id, _)| *id as i64 == id_note)
                    .map(|(_, n)| n.memo)
            },
        };

        if let Some(Some(memo)) = memo {
            return MemoBytes::from_bytes(&memo)
                .and_then(Memo::try_from)
                .map_to_mm(|err| ZcoinStorageError::InvalidMemo(err.to_string()));
        };

        MmError::err(ZcoinStorageError::GetFromStorageError("Memo not found".to_string()))
    }

    async fn get_commitment_tree(
        &self,
        block_height: BlockHeight,
    ) -> Result<Option<CommitmentTree<Node>>, Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let blocks_table = db_transaction.table::<WalletDbBlocksTable>().await.map_mm_err()?;
        let index_keys = MultiIndex::new(WalletDbBlocksTable::TICKER_HEIGHT_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(u32::from(block_height))
            .map_mm_err()?;

        let block = blocks_table
            .get_item_by_unique_multi_index(index_keys)
            .await
            .map_mm_err()?
            .map(|(_, account)| account);

        if let Some(block) = block {
            return Ok(Some(
                CommitmentTree::read(&block.sapling_tree[..])
                    .map_to_mm(|e| ZcoinStorageError::DecodingError(e.to_string()))?,
            ));
        }

        Ok(None)
    }

    async fn get_witnesses(
        &self,
        block_height: BlockHeight,
    ) -> Result<Vec<(Self::NoteRef, IncrementalWitness<Node>)>, Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;

        let sapling_witness_table = db_transaction
            .table::<WalletDbSaplingWitnessesTable>()
            .await
            .map_mm_err()?;

        let index_keys = MultiIndex::new(WalletDbSaplingWitnessesTable::TICKER_BLOCK_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(u32::from(block_height))
            .map_mm_err()?;
        let maybe_witnesses = sapling_witness_table
            .get_items_by_multi_index(index_keys)
            .await
            .map_mm_err()?;

        // Retrieves a list of transaction IDs (id_tx) from the transactions table
        // that match the provided account ID and have not been spent (spent IS NULL).
        let mut witnesses = Vec::with_capacity(maybe_witnesses.len());
        for (_, witness) in maybe_witnesses {
            let id_note = witness.note.to_i64().unwrap();
            let id_note = NoteId::ReceivedNoteId(id_note.to_i64().expect("invalid value"));
            let witness = IncrementalWitness::read(witness.witness.as_slice())
                .map(|witness| (id_note, witness))
                .map_to_mm(|err| ZcoinStorageError::DecodingError(err.to_string()))?;
            witnesses.push(witness)
        }

        Ok(witnesses)
    }

    async fn get_nullifiers(&self) -> Result<Vec<(AccountId, Nullifier)>, Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;

        // Received notes
        let received_notes_table = db_transaction
            .table::<WalletDbReceivedNotesTable>()
            .await
            .map_mm_err()?;
        let maybe_notes = received_notes_table
            .get_items("ticker", &self.ticker)
            .await
            .map_mm_err()?;

        // Transactions
        let txs_table = db_transaction.table::<WalletDbTransactionsTable>().await.map_mm_err()?;
        let maybe_txs = txs_table.get_items("ticker", &self.ticker).await.map_mm_err()?;

        let mut nullifiers = vec![];
        for (_, note) in maybe_notes {
            let matching_tx = maybe_txs.iter().find(|(id_tx, _tx)| id_tx.to_bigint() == note.spent);

            if let Some((_, tx)) = matching_tx {
                if tx.block.is_some() {
                    nullifiers.push((
                        AccountId(
                            note.account
                                .to_u32()
                                .ok_or_else(|| ZcoinStorageError::GetFromStorageError("Invalid amount".to_string()))?,
                        ),
                        Nullifier::from_slice(&note.nf.clone().ok_or_else(|| {
                            ZcoinStorageError::GetFromStorageError("Error while putting tx_meta".to_string())
                        })?)
                        .unwrap(),
                    ));
                }
            }
        }

        Ok(nullifiers)
    }

    async fn get_spendable_notes(
        &self,
        account: AccountId,
        anchor_height: BlockHeight,
    ) -> Result<Vec<SpendableNote>, Self::Error> {
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;

        // Received notes
        let received_notes_table = db_transaction
            .table::<WalletDbReceivedNotesTable>()
            .await
            .map_mm_err()?;
        let index_keys = MultiIndex::new(WalletDbReceivedNotesTable::TICKER_ACCOUNT_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(account.0.to_bigint())
            .map_mm_err()?;
        let maybe_notes = received_notes_table
            .get_items_by_multi_index(index_keys)
            .await
            .map_mm_err()?;

        // Transactions
        let txs_table = db_transaction.table::<WalletDbTransactionsTable>().await.map_mm_err()?;
        let txs = txs_table
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .bound("block", 0u32, u32::from(anchor_height + 1))
            .open_cursor(WalletDbTransactionsTable::TICKER_BLOCK_INDEX)
            .await
            .map_mm_err()?
            .collect()
            .await
            .map_mm_err()?
            .into_iter()
            .collect::<Vec<_>>();
        // Witnesses
        let witnesses_table = db_transaction
            .table::<WalletDbSaplingWitnessesTable>()
            .await
            .map_mm_err()?;
        let witnesses = witnesses_table
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .only("block", u32::from(anchor_height))
            .map_mm_err()?
            .open_cursor(WalletDbSaplingWitnessesTable::TICKER_BLOCK_INDEX)
            .await
            .map_mm_err()?
            .collect()
            .await
            .map_mm_err()?
            .into_iter()
            .map(|(_, item)| item)
            .collect::<Vec<_>>();

        let mut spendable_notes = vec![];
        for (id_note, note) in maybe_notes {
            let id_note = num_to_bigint!(id_note)?;
            let witness = witnesses.iter().find(|wit| wit.note == id_note);
            let tx = txs.iter().find(|(id, _tx)| *id == note.tx);

            if let (Some(witness), Some(_)) = (witness, tx) {
                if note.spent.is_none() {
                    let spend = SpendableNoteConstructor {
                        diversifier: note.diversifier.clone(),
                        value: note.value.clone(),
                        rcm: note.rcm.to_owned(),
                        witness: witness.witness.clone(),
                    };
                    spendable_notes.push(to_spendable_note(spend)?);
                }
            }
        }

        Ok(spendable_notes)
    }

    async fn select_spendable_notes(
        &self,
        account: AccountId,
        target_value: Amount,
        anchor_height: BlockHeight,
    ) -> Result<Vec<SpendableNote>, Self::Error> {
        // The goal of this SQL statement is to select the oldest notes until the required
        // value has been reached, and then fetch the witnesses at the desired height for the
        // selected notes. This is achieved in several steps:
        //
        // 1) Use a window function to create a view of all notes, ordered from oldest to
        //    newest, with an additional column containing a running sum:
        //    - Unspent notes accumulate the values of all unspent notes in that note's
        //      account, up to itself.
        //    - Spent notes accumulate the values of all notes in the transaction they were
        //      spent in, up to itself.
        //
        // 2) Select all unspent notes in the desired account, along with their running sum.
        //
        // 3) Select all notes for which the running sum was less than the required value, as
        //    well as a single note for which the sum was greater than or equal to the
        //    required value, bringing the sum of all selected notes across the threshold.
        //
        // 4) Match the selected notes against the witnesses at the desired height.
        let locked_db = self.lock_db().await?;
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;

        // Received notes
        let received_notes_table = db_transaction
            .table::<WalletDbReceivedNotesTable>()
            .await
            .map_mm_err()?;
        let index_keys = MultiIndex::new(WalletDbReceivedNotesTable::TICKER_ACCOUNT_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(account.0.to_bigint().unwrap())
            .map_mm_err()?;
        let maybe_notes = received_notes_table
            .get_items_by_multi_index(index_keys)
            .await
            .map_mm_err()?;

        // Transactions
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let txs_table = db_transaction.table::<WalletDbTransactionsTable>().await.map_mm_err()?;
        let txs = txs_table
            .cursor_builder()
            .only("ticker", &self.ticker)
            .map_mm_err()?
            .bound("block", 0u32, u32::from(anchor_height))
            .open_cursor(WalletDbTransactionsTable::TICKER_BLOCK_INDEX)
            .await
            .map_mm_err()?
            .collect()
            .await
            .map_mm_err()?;

        // Sapling Witness
        let db_transaction = locked_db.get_inner().transaction().await.map_mm_err()?;
        let witness_table = db_transaction
            .table::<WalletDbSaplingWitnessesTable>()
            .await
            .map_mm_err()?;
        let index_keys = MultiIndex::new(WalletDbSaplingWitnessesTable::TICKER_BLOCK_INDEX)
            .with_value(&self.ticker)
            .map_mm_err()?
            .with_value(u32::from(anchor_height))
            .map_mm_err()?;
        let witnesses = witness_table.get_items_by_multi_index(index_keys).await.map_mm_err()?;

        let mut running_sum = 0;
        let mut notes = vec![];
        for (id_note, note) in &maybe_notes {
            let value = note.value.clone().to_i64().expect("price is too large");
            if note.spent.is_none() {
                running_sum += value;
                notes.push((id_note, value, note, running_sum));
            }
        }

        let final_notes: Vec<_> = notes
            .iter()
            .filter_map(|(id_note, value, note, running_sum)| {
                txs.iter()
                    .find(|(id_tx, _tx)| *id_tx == note.tx)
                    .map(|_| (id_note, value, note, running_sum))
            })
            .collect();
        let mut unspent_notes: Vec<_> = final_notes
            .iter()
            .filter(|(_, _, _, sum)| **sum < i64::from(target_value))
            .cloned()
            .collect();

        if let Some(note) = final_notes.iter().find(|(_, _, _, sum)| **sum >= target_value.into()) {
            unspent_notes.push(*note);
        };

        // Step 4: Get witnesses for selected notes
        let mut spendable_notes = Vec::new();
        for (id_note, _, note, _) in &unspent_notes {
            let noteid_bigint = num_to_bigint!(id_note)?;
            if let Some((_, witness)) = witnesses.iter().find(|(_, w)| w.note == noteid_bigint) {
                let spendable = to_spendable_note(SpendableNoteConstructor {
                    diversifier: note.diversifier.clone(),
                    value: note.value.clone(),
                    rcm: note.rcm.clone(),
                    witness: witness.witness.clone(),
                })?;
                spendable_notes.push(spendable);
            }
        }

        Ok(spendable_notes)
    }
}

#[async_trait]
impl WalletWrite for WalletIndexedDb {
    async fn advance_by_block(
        &mut self,
        block: &PrunedBlock,
        updated_witnesses: &[(Self::NoteRef, IncrementalWitness<Node>)],
    ) -> Result<Vec<(Self::NoteRef, IncrementalWitness<Node>)>, Self::Error> {
        let selfi = self.deref();
        selfi
            .insert_block(
                block.block_height,
                block.block_hash,
                block.block_time,
                block.commitment_tree,
            )
            .await?;

        let mut new_witnesses = vec![];
        for tx in block.transactions {
            let tx_row = selfi.put_tx_meta(tx, block.block_height).await?;

            // Mark notes as spent and remove them from the scanning cache
            for spend in &tx.shielded_spends {
                selfi.mark_spent(tx_row, &spend.nf).await?;
            }

            for output in &tx.shielded_outputs {
                let received_note_id = selfi.put_received_note(output, tx_row).await?;

                // Save witness for note.
                new_witnesses.push((received_note_id, output.witness.clone()));
            }
        }

        // Insert current new_witnesses into the database.
        for (received_note_id, witness) in updated_witnesses.iter().chain(new_witnesses.iter()) {
            if let NoteId::ReceivedNoteId(rnid) = *received_note_id {
                selfi.insert_witness(rnid, witness, block.block_height).await?;
            } else {
                return MmError::err(ZcoinStorageError::InvalidNoteId);
            }
        }

        // Prune the stored witnesses (we only expect rollbacks of at most 100 blocks).
        let below_height = if block.block_height < BlockHeight::from(100) {
            BlockHeight::from(0)
        } else {
            block.block_height - 100
        };
        selfi.prune_witnesses(below_height).await?;

        // Update now-expired transactions that didn't get mined.
        selfi.update_expired_notes(block.block_height).await?;

        Ok(new_witnesses)
    }

    async fn store_received_tx(&mut self, received_tx: &ReceivedTransaction) -> Result<Self::TxRef, Self::Error> {
        let selfi = self.deref();
        let tx_ref = selfi.put_tx_data(received_tx.tx, None).await?;

        for output in received_tx.outputs {
            if output.outgoing {
                selfi.put_sent_note(output, tx_ref).await?;
            } else {
                selfi.put_received_note(output, tx_ref).await?;
            }
        }

        Ok(tx_ref)
    }

    async fn store_sent_tx(&mut self, sent_tx: &SentTransaction) -> Result<Self::TxRef, Self::Error> {
        let selfi = self.deref();
        let tx_ref = selfi.put_tx_data(sent_tx.tx, Some(sent_tx.created.to_string())).await?;

        // Mark notes as spent.
        for spend in &sent_tx.tx.shielded_spends {
            selfi.mark_spent(tx_ref, &spend.nullifier).await?;
        }

        selfi
            .insert_sent_note(
                tx_ref,
                sent_tx.output_index,
                sent_tx.account,
                sent_tx.recipient_address,
                sent_tx.value,
                sent_tx.memo.as_ref(),
            )
            .await?;

        // Return the row number of the transaction, so the caller can fetch it for sending.
        Ok(tx_ref)
    }

    async fn rewind_to_height(&mut self, block_height: BlockHeight) -> Result<(), Self::Error> {
        let selfi = self.deref();
        selfi.rewind_to_height(block_height).await
    }
}

#[derive(Clone)]
pub struct DataConnStmtCacheWasm(pub WalletIndexedDb);

#[async_trait]
impl WalletRead for DataConnStmtCacheWasm {
    type Error = MmError<ZcoinStorageError>;
    type NoteRef = NoteId;
    type TxRef = i64;

    async fn block_height_extrema(&self) -> Result<Option<(BlockHeight, BlockHeight)>, Self::Error> {
        self.0.block_height_extrema().await
    }

    async fn get_block_hash(&self, block_height: BlockHeight) -> Result<Option<BlockHash>, Self::Error> {
        self.0.get_block_hash(block_height).await
    }

    async fn get_tx_height(&self, txid: TxId) -> Result<Option<BlockHeight>, Self::Error> {
        self.0.get_tx_height(txid).await
    }

    async fn get_address(&self, account: AccountId) -> Result<Option<PaymentAddress>, Self::Error> {
        self.0.get_address(account).await
    }

    async fn get_extended_full_viewing_keys(&self) -> Result<HashMap<AccountId, ExtendedFullViewingKey>, Self::Error> {
        self.0.get_extended_full_viewing_keys().await
    }

    async fn is_valid_account_extfvk(
        &self,
        account: AccountId,
        extfvk: &ExtendedFullViewingKey,
    ) -> Result<bool, Self::Error> {
        self.0.is_valid_account_extfvk(account, extfvk).await
    }

    async fn get_balance_at(&self, account: AccountId, anchor_height: BlockHeight) -> Result<Amount, Self::Error> {
        self.0.get_balance_at(account, anchor_height).await
    }

    async fn get_memo(&self, id_note: Self::NoteRef) -> Result<Memo, Self::Error> {
        self.0.get_memo(id_note).await
    }

    async fn get_commitment_tree(
        &self,
        block_height: BlockHeight,
    ) -> Result<Option<CommitmentTree<Node>>, Self::Error> {
        self.0.get_commitment_tree(block_height).await
    }

    async fn get_witnesses(
        &self,
        block_height: BlockHeight,
    ) -> Result<Vec<(Self::NoteRef, IncrementalWitness<Node>)>, Self::Error> {
        self.0.get_witnesses(block_height).await
    }

    async fn get_nullifiers(&self) -> Result<Vec<(AccountId, Nullifier)>, Self::Error> {
        self.0.get_nullifiers().await
    }

    async fn get_spendable_notes(
        &self,
        account: AccountId,
        anchor_height: BlockHeight,
    ) -> Result<Vec<SpendableNote>, Self::Error> {
        self.0.get_spendable_notes(account, anchor_height).await
    }

    async fn select_spendable_notes(
        &self,
        account: AccountId,
        target_value: Amount,
        anchor_height: BlockHeight,
    ) -> Result<Vec<SpendableNote>, Self::Error> {
        self.0
            .select_spendable_notes(account, target_value, anchor_height)
            .await
    }
}

#[async_trait]
impl WalletWrite for DataConnStmtCacheWasm {
    async fn advance_by_block(
        &mut self,
        block: &PrunedBlock,
        updated_witnesses: &[(Self::NoteRef, IncrementalWitness<Node>)],
    ) -> Result<Vec<(Self::NoteRef, IncrementalWitness<Node>)>, Self::Error> {
        self.0.advance_by_block(block, updated_witnesses).await
    }

    async fn store_received_tx(&mut self, received_tx: &ReceivedTransaction) -> Result<Self::TxRef, Self::Error> {
        self.0.store_received_tx(received_tx).await
    }

    async fn store_sent_tx(&mut self, sent_tx: &SentTransaction) -> Result<Self::TxRef, Self::Error> {
        self.0.store_sent_tx(sent_tx).await
    }

    async fn rewind_to_height(&mut self, block_height: BlockHeight) -> Result<(), Self::Error> {
        self.0.rewind_to_height(block_height).await
    }
}
