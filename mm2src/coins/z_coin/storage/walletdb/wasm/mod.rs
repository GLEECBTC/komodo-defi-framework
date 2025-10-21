pub mod storage;
pub mod tables;

use crate::z_coin::ZcoinStorageError;

use ff::PrimeField;
use mm2_err_handle::prelude::*;
use mm2_number::BigInt;
use num_traits::ToPrimitive;
use std::convert::TryInto;
use zcash_client_backend::wallet::SpendableNote;
use zcash_primitives::merkle_tree::IncrementalWitness;
use zcash_primitives::sapling::Diversifier;
use zcash_primitives::sapling::Rseed;
use zcash_primitives::transaction::components::Amount;

struct SpendableNoteConstructor {
    diversifier: Vec<u8>,
    value: BigInt,
    rcm: Vec<u8>,
    witness: Vec<u8>,
}

fn to_spendable_note(note: SpendableNoteConstructor) -> MmResult<SpendableNote, ZcoinStorageError> {
    let diversifier = {
        let d = note.diversifier;
        if d.len() != 11 {
            return MmError::err(ZcoinStorageError::CorruptedData(
                "Invalid diversifier length".to_string(),
            ));
        }
        let mut tmp = [0; 11];
        tmp.copy_from_slice(&d);
        Diversifier(tmp)
    };

    let note_value = Amount::from_i64(note.value.to_i64().expect("BigInt is too large to fit in an i64")).unwrap();

    let rseed = {
        let rcm_bytes = note.rcm;

        // We store rcm directly in the data DB, regardless of whether the note
        // used a v1 or v2 note plaintext, so for the purposes of spending let's
        // pretend this is a pre-ZIP 212 note.
        let rcm = jubjub::Fr::from_repr(
            rcm_bytes[..]
                .try_into()
                .map_to_mm(|_| ZcoinStorageError::InvalidNote("Invalid note".to_string()))?,
        )
        .ok_or_else(|| MmError::new(ZcoinStorageError::InvalidNote("Invalid note".to_string())))?;
        Rseed::BeforeZip212(rcm)
    };

    let witness = {
        let d = note.witness;
        IncrementalWitness::read(&d[..]).map_to_mm(|err| ZcoinStorageError::IoError(err.to_string()))?
    };

    Ok(SpendableNote {
        diversifier,
        note_value,
        rseed,
        witness,
    })
}

#[cfg(test)]
mod wasm_test {
    use crate::z_coin::storage::walletdb::WalletIndexedDb;
    use crate::z_coin::storage::{BlockDbImpl, BlockProcessingMode, DataConnStmtCacheWasm, DataConnStmtCacheWrapper};
    use crate::z_coin::LockedNotesStorage;
    use crate::z_coin::{ValidateBlocksError, ZcoinConsensusParams, ZcoinStorageError};
    use crate::ZcoinProtocolInfo;
    use mm2_core::mm_ctx::MmArc;
    use mm2_event_stream::StreamingManager;
    use mm2_test_helpers::for_tests::mm_ctx_with_custom_db;
    use protobuf::Message;
    use std::path::PathBuf;
    use wasm_bindgen_test::*;
    use zcash_client_backend::wallet::{AccountId, OvkPolicy};
    use zcash_extras::fake_compact_block;
    use zcash_extras::fake_compact_block_spending;
    use zcash_extras::wallet::create_spend_to_address;
    use zcash_extras::WalletRead;
    use zcash_primitives::block::BlockHash;
    use zcash_primitives::consensus::{BlockHeight, Network, NetworkUpgrade, Parameters};
    use zcash_primitives::transaction::components::Amount;
    use zcash_primitives::zip32::{ExtendedFullViewingKey, ExtendedSpendingKey};
    use zcash_proofs::prover::LocalTxProver;

    wasm_bindgen_test_configure!(run_in_browser);

    const TICKER: &str = "ARRR";
    const MY_ADDRESS: &str = " RSiVR1jAnu95MJMdrZDLhsQacwAJ6aUmd9";

    async fn test_prover() -> LocalTxProver {
        let (spend_buf, output_buf) = wagyu_zcash_parameters::load_sapling_parameters();
        LocalTxProver::from_bytes(&spend_buf[..], &output_buf[..])
    }

    fn consensus_params() -> ZcoinConsensusParams {
        let protocol_info = serde_json::from_value::<ZcoinProtocolInfo>(json!({
            "consensus_params": {
              "overwinter_activation_height": 152855,
              "sapling_activation_height": u32::from(sapling_activation_height()),
              "blossom_activation_height": null,
              "heartwood_activation_height": null,
              "canopy_activation_height": null,
              "coin_type": 133,
              "hrp_sapling_extended_spending_key": "secret-extended-key-main",
              "hrp_sapling_extended_full_viewing_key": "zxviews",
              "hrp_sapling_payment_address": "zs",
              "b58_pubkey_address_prefix": [
                28,
                184
              ],
              "b58_script_address_prefix": [
                28,
                189
              ]
            }
        }))
        .unwrap();

        protocol_info.consensus_params
    }

    pub fn sapling_activation_height() -> BlockHeight {
        Network::TestNetwork.activation_height(NetworkUpgrade::Sapling).unwrap()
    }

    async fn wallet_db_from_zcoin_builder_for_test(ctx: &MmArc, ticker: &str) -> WalletIndexedDb {
        WalletIndexedDb::new(ctx, ticker, consensus_params()).await.unwrap()
    }

    #[wasm_bindgen_test]
    async fn test_empty_database_has_no_balance() {
        let ctx = mm_ctx_with_custom_db();
        let db = wallet_db_from_zcoin_builder_for_test(&ctx, TICKER).await;

        // Add an account to the wallet
        let extsk = ExtendedSpendingKey::master(&[]);
        let extfvks = [ExtendedFullViewingKey::from(&extsk)];
        assert!(db.init_accounts_table(&extfvks).await.is_ok());

        // The account should be empty
        assert_eq!(db.get_balance(AccountId(0)).await.unwrap(), Amount::zero());

        // We can't get an anchor height, as we have not scanned any blocks.
        assert_eq!(db.get_target_and_anchor_heights().await.unwrap(), None);

        // An invalid account has zero balance
        assert!(db.get_address(AccountId(1)).await.is_err());
        assert_eq!(db.get_balance(AccountId(0)).await.unwrap(), Amount::zero());
    }

    #[wasm_bindgen_test]
    async fn test_init_accounts_table_only_works_once() {
        let ctx = mm_ctx_with_custom_db();
        let db = wallet_db_from_zcoin_builder_for_test(&ctx, TICKER).await;

        // We can call the function as many times as we want with no data
        assert!(db.init_accounts_table(&[]).await.is_ok());
        assert!(db.init_accounts_table(&[]).await.is_ok());

        // First call with data should initialise the accounts table.
        let extfvks = [ExtendedFullViewingKey::from(&ExtendedSpendingKey::master(&[]))];
        assert!(db.init_accounts_table(&extfvks).await.is_ok());

        // Subsequent calls should return an error
        assert!(db.init_accounts_table(&extfvks).await.is_ok());
    }

    #[wasm_bindgen_test]
    async fn test_init_blocks_table_only_works_once() {
        let ctx = mm_ctx_with_custom_db();
        let db = wallet_db_from_zcoin_builder_for_test(&ctx, TICKER).await;

        // First call with data should initialise the blocks table
        assert!(db
            .init_blocks_table(BlockHeight::from(1), BlockHash([1; 32]), 1, &[])
            .await
            .is_ok());

        // Subsequent calls should return an error
        assert!(db
            .init_blocks_table(BlockHeight::from(2), BlockHash([2; 32]), 2, &[])
            .await
            .is_err());
    }

    #[wasm_bindgen_test]
    async fn init_accounts_table_stores_correct_address() {
        let ctx = mm_ctx_with_custom_db();
        let db = wallet_db_from_zcoin_builder_for_test(&ctx, TICKER).await;

        // Add an account to the wallet
        let extsk = ExtendedSpendingKey::master(&[]);
        let extfvks = [ExtendedFullViewingKey::from(&extsk)];
        assert!(db.init_accounts_table(&extfvks).await.is_ok());

        // The account's address should be in the data DB.
        let pa = db.get_address(AccountId(0)).await.unwrap();
        assert_eq!(pa.unwrap(), extsk.default_address().unwrap().1);
    }

    #[wasm_bindgen_test]
    async fn test_valid_chain_state() {
        // init blocks_db
        let ctx = mm_ctx_with_custom_db();
        let locked_notes_db = LockedNotesStorage::new(ctx.clone(), MY_ADDRESS.to_string())
            .await
            .unwrap();
        let blockdb = BlockDbImpl::new(&ctx, TICKER.to_string(), PathBuf::new())
            .await
            .unwrap();

        // init walletdb.
        let walletdb = wallet_db_from_zcoin_builder_for_test(&ctx, TICKER).await;

        // Add an account to the wallet
        let extsk = ExtendedSpendingKey::master(&[]);
        let extfvk = ExtendedFullViewingKey::from(&extsk);
        assert!(walletdb.init_accounts_table(&[extfvk.clone()]).await.is_ok());

        // Empty chain should be valid
        let consensus_params = consensus_params();
        blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Validate,
                walletdb.get_max_height_hash().await.unwrap(),
                None,
                &locked_notes_db,
            )
            .await
            .unwrap();

        // create a fake compactBlock sending value to the address
        let (cb, _) = fake_compact_block(
            sapling_activation_height(),
            BlockHash([0; 32]),
            extfvk.clone(),
            Amount::from_u64(5).unwrap(),
        );
        let cb_bytes = cb.write_to_bytes().unwrap();
        blockdb.insert_block(cb.height as u32, cb_bytes).await.unwrap();

        // Cache-only chain should be valid
        blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Validate,
                walletdb.get_max_height_hash().await.unwrap(),
                None,
                &locked_notes_db,
            )
            .await
            .unwrap();

        // scan the cache
        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
        blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Scan(scan, StreamingManager::default()),
                None,
                None,
                &locked_notes_db,
            )
            .await
            .unwrap();

        // Data-only chain should be valid
        let max_height_hash = walletdb.get_max_height_hash().await.unwrap();
        blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Validate,
                max_height_hash,
                None,
                &locked_notes_db,
            )
            .await
            .unwrap();

        // Create a second fake CompactBlock sending more value to the address
        let (cb2, _) = fake_compact_block(
            sapling_activation_height() + 1,
            cb.hash(),
            extfvk,
            Amount::from_u64(7).unwrap(),
        );
        let cb_bytes = cb2.write_to_bytes().unwrap();
        blockdb.insert_block(cb2.height as u32, cb_bytes).await.unwrap();

        // Data+cache chain should be valid
        blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Validate,
                walletdb.get_max_height_hash().await.unwrap(),
                None,
                &locked_notes_db,
            )
            .await
            .unwrap();

        // Scan the cache again
        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
        blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Scan(scan, StreamingManager::default()),
                None,
                None,
                &locked_notes_db,
            )
            .await
            .unwrap();

        // Data+cache chain should be valid
        blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Validate,
                walletdb.get_max_height_hash().await.unwrap(),
                None,
                &locked_notes_db,
            )
            .await
            .unwrap();
    }

    #[wasm_bindgen_test]
    async fn invalid_chain_cache_disconnected() {
        // init blocks_db
        let ctx = mm_ctx_with_custom_db();
        let locked_notes_db = LockedNotesStorage::new(ctx.clone(), MY_ADDRESS.to_string())
            .await
            .unwrap();
        let blockdb = BlockDbImpl::new(&ctx, TICKER.to_string(), PathBuf::new())
            .await
            .unwrap();

        // init walletdb.
        let walletdb = wallet_db_from_zcoin_builder_for_test(&ctx, TICKER).await;
        let consensus_params = consensus_params();

        // Add an account to the wallet
        let extsk = ExtendedSpendingKey::master(&[]);
        let extfvk = ExtendedFullViewingKey::from(&extsk);
        assert!(walletdb.init_accounts_table(&[extfvk.clone()]).await.is_ok());

        // Create some fake compactBlocks
        let (cb, _) = fake_compact_block(
            sapling_activation_height(),
            BlockHash([0; 32]),
            extfvk.clone(),
            Amount::from_u64(5).unwrap(),
        );
        let (cb2, _) = fake_compact_block(
            sapling_activation_height() + 1,
            cb.hash(),
            extfvk.clone(),
            Amount::from_u64(7).unwrap(),
        );
        let cb_bytes = cb.write_to_bytes().unwrap();
        blockdb.insert_block(cb.height as u32, cb_bytes).await.unwrap();
        let cb2_bytes = cb2.write_to_bytes().unwrap();
        blockdb.insert_block(cb2.height as u32, cb2_bytes).await.unwrap();

        // Scan the cache again
        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
        blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Scan(scan, StreamingManager::default()),
                None,
                None,
                &locked_notes_db,
            )
            .await
            .unwrap();

        // Data-only chain should be valid
        blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Validate,
                walletdb.get_max_height_hash().await.unwrap(),
                None,
                &locked_notes_db,
            )
            .await
            .unwrap();

        // Create more fake CompactBlocks that don't connect to the scanned ones
        let (cb3, _) = fake_compact_block(
            sapling_activation_height() + 2,
            BlockHash([1; 32]),
            extfvk.clone(),
            Amount::from_u64(8).unwrap(),
        );
        let (cb4, _) = fake_compact_block(
            sapling_activation_height() + 3,
            cb3.hash(),
            extfvk,
            Amount::from_u64(3).unwrap(),
        );
        let cb3_bytes = cb3.write_to_bytes().unwrap();
        blockdb.insert_block(cb3.height as u32, cb3_bytes).await.unwrap();
        let cb4_bytes = cb4.write_to_bytes().unwrap();
        blockdb.insert_block(cb4.height as u32, cb4_bytes).await.unwrap();

        // Data+cache chain should be invalid at the data/cache boundary
        let validate_chain = blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Validate,
                walletdb.get_max_height_hash().await.unwrap(),
                None,
                &locked_notes_db,
            )
            .await
            .unwrap_err();
        match validate_chain.get_inner() {
            ZcoinStorageError::ValidateBlocksError(ValidateBlocksError::ChainInvalid { height, .. }) => {
                assert_eq!(*height, sapling_activation_height() + 2)
            },
            _ => panic!(),
        }
    }

    #[wasm_bindgen_test]
    async fn test_invalid_chain_reorg() {
        // init blocks_db
        let ctx = mm_ctx_with_custom_db();
        let locked_notes_db = LockedNotesStorage::new(ctx.clone(), MY_ADDRESS.to_string())
            .await
            .unwrap();
        let blockdb = BlockDbImpl::new(&ctx, TICKER.to_string(), PathBuf::new())
            .await
            .unwrap();

        // init walletdb.
        let walletdb = wallet_db_from_zcoin_builder_for_test(&ctx, TICKER).await;
        let consensus_params = consensus_params();

        // Add an account to the wallet
        let extsk = ExtendedSpendingKey::master(&[]);
        let extfvk = ExtendedFullViewingKey::from(&extsk);
        assert!(walletdb.init_accounts_table(&[extfvk.clone()]).await.is_ok());

        // Create some fake compactBlocks
        let (cb, _) = fake_compact_block(
            sapling_activation_height(),
            BlockHash([0; 32]),
            extfvk.clone(),
            Amount::from_u64(5).unwrap(),
        );
        let (cb2, _) = fake_compact_block(
            sapling_activation_height() + 1,
            cb.hash(),
            extfvk.clone(),
            Amount::from_u64(7).unwrap(),
        );
        let cb_bytes = cb.write_to_bytes().unwrap();
        blockdb.insert_block(cb.height as u32, cb_bytes).await.unwrap();
        let cb2_bytes = cb2.write_to_bytes().unwrap();
        blockdb.insert_block(cb2.height as u32, cb2_bytes).await.unwrap();

        // Scan the cache again
        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
        blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Scan(scan, StreamingManager::default()),
                None,
                None,
                &locked_notes_db,
            )
            .await
            .unwrap();

        // Data-only chain should be valid
        blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Validate,
                walletdb.get_max_height_hash().await.unwrap(),
                None,
                &locked_notes_db,
            )
            .await
            .unwrap();

        // Create more fake CompactBlocks that that contains a reorg
        let (cb3, _) = fake_compact_block(
            sapling_activation_height() + 2,
            cb2.hash(),
            extfvk.clone(),
            Amount::from_u64(8).unwrap(),
        );
        let (cb4, _) = fake_compact_block(
            sapling_activation_height() + 3,
            BlockHash([1; 32]),
            extfvk,
            Amount::from_u64(3).unwrap(),
        );
        let cb3_bytes = cb3.write_to_bytes().unwrap();
        blockdb.insert_block(cb3.height as u32, cb3_bytes).await.unwrap();
        let cb4_bytes = cb4.write_to_bytes().unwrap();
        blockdb.insert_block(cb4.height as u32, cb4_bytes).await.unwrap();

        // Data+cache chain should be invalid at the data/cache boundary
        let validate_chain = blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Validate,
                walletdb.get_max_height_hash().await.unwrap(),
                None,
                &locked_notes_db,
            )
            .await
            .unwrap_err();
        match validate_chain.get_inner() {
            ZcoinStorageError::ValidateBlocksError(ValidateBlocksError::ChainInvalid { height, .. }) => {
                assert_eq!(*height, sapling_activation_height() + 3)
            },
            _ => panic!(),
        }
    }

    #[wasm_bindgen_test]
    async fn test_data_db_rewinding() {
        // init blocks_db
        let ctx = mm_ctx_with_custom_db();
        let locked_notes_db = LockedNotesStorage::new(ctx.clone(), MY_ADDRESS.to_string())
            .await
            .unwrap();
        let blockdb = BlockDbImpl::new(&ctx, TICKER.to_string(), PathBuf::new())
            .await
            .unwrap();

        // init walletdb.
        let walletdb = wallet_db_from_zcoin_builder_for_test(&ctx, TICKER).await;
        let consensus_params = consensus_params();

        // Add an account to the wallet
        let extsk = ExtendedSpendingKey::master(&[]);
        let extfvk = ExtendedFullViewingKey::from(&extsk);
        assert!(walletdb.init_accounts_table(&[extfvk.clone()]).await.is_ok());

        // Account balance should be zero
        assert_eq!(walletdb.get_balance(AccountId(0)).await.unwrap(), Amount::zero());

        // Create some fake compactBlocks sending value to the address
        let value = Amount::from_u64(5).unwrap();
        let value2 = Amount::from_u64(7).unwrap();
        let (cb, _) = fake_compact_block(sapling_activation_height(), BlockHash([0; 32]), extfvk.clone(), value);
        let (cb2, _) = fake_compact_block(sapling_activation_height() + 1, cb.hash(), extfvk, value2);
        let cb_bytes = cb.write_to_bytes().unwrap();
        blockdb.insert_block(cb.height as u32, cb_bytes).await.unwrap();
        let cb2_bytes = cb2.write_to_bytes().unwrap();
        blockdb.insert_block(cb2.height as u32, cb2_bytes).await.unwrap();

        // Scan the cache
        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
        blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Scan(scan, StreamingManager::default()),
                None,
                None,
                &locked_notes_db,
            )
            .await
            .unwrap();

        // Account balance should reflect both received notes
        assert_eq!(walletdb.get_balance(AccountId(0)).await.unwrap(), value + value2);

        // Rewind to height of last scanned block
        walletdb
            .rewind_to_height(sapling_activation_height() + 1)
            .await
            .unwrap();

        // Account balance should should be unaltered
        assert_eq!(walletdb.get_balance(AccountId(0)).await.unwrap(), value + value2);

        // Rewind so one block is dropped.
        walletdb.rewind_to_height(sapling_activation_height()).await.unwrap();

        // Account balance should only contain the first received note
        assert_eq!(walletdb.get_balance(AccountId(0)).await.unwrap(), value);

        // Scan the cache again
        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
        blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Scan(scan, StreamingManager::default()),
                None,
                None,
                &locked_notes_db,
            )
            .await
            .unwrap();

        // Account balance should again reflect both received notes
        assert_eq!(walletdb.get_balance(AccountId(0)).await.unwrap(), value + value2);
    }

    #[wasm_bindgen_test]
    async fn test_scan_cached_blocks_requires_sequential_blocks() {
        // init blocks_db
        let ctx = mm_ctx_with_custom_db();
        let locked_notes_db = LockedNotesStorage::new(ctx.clone(), MY_ADDRESS.to_string())
            .await
            .unwrap();
        let blockdb = BlockDbImpl::new(&ctx, TICKER.to_string(), PathBuf::new())
            .await
            .unwrap();

        // init walletdb.
        let walletdb = wallet_db_from_zcoin_builder_for_test(&ctx, TICKER).await;
        let consensus_params = consensus_params();

        // Add an account to the wallet
        let extsk = ExtendedSpendingKey::master(&[]);
        let extfvk = ExtendedFullViewingKey::from(&extsk);
        assert!(walletdb.init_accounts_table(&[extfvk.clone()]).await.is_ok());

        // Create a block with height SAPLING_ACTIVATION_HEIGHT
        let value = Amount::from_u64(50000).unwrap();
        let (cb1, _) = fake_compact_block(sapling_activation_height(), BlockHash([0; 32]), extfvk.clone(), value);
        let cb1_bytes = cb1.write_to_bytes().unwrap();
        blockdb.insert_block(cb1.height as u32, cb1_bytes).await.unwrap();

        // Scan cache
        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
        blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Scan(scan, StreamingManager::default()),
                None,
                None,
                &locked_notes_db,
            )
            .await
            .unwrap();

        // We cannot scan a block of height SAPLING_ACTIVATION_HEIGHT + 2 next
        let (cb2, _) = fake_compact_block(sapling_activation_height() + 1, cb1.hash(), extfvk.clone(), value);
        let cb2_bytes = cb2.write_to_bytes().unwrap();
        let (cb3, _) = fake_compact_block(sapling_activation_height() + 2, cb2.hash(), extfvk.clone(), value);
        let cb3_bytes = cb3.write_to_bytes().unwrap();
        blockdb.insert_block(cb3.height as u32, cb3_bytes).await.unwrap();
        // Scan the cache again
        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
        let scan = blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Scan(scan, StreamingManager::default()),
                None,
                None,
                &locked_notes_db,
            )
            .await
            .unwrap_err();
        match scan.get_inner() {
            ZcoinStorageError::ValidateBlocksError(err) => {
                let actual = err.to_string();
                let expected = ValidateBlocksError::block_height_discontinuity(
                    sapling_activation_height() + 1,
                    sapling_activation_height() + 2,
                );
                assert_eq!(expected.to_string(), actual)
            },
            _ => panic!("Should have failed"),
        }

        // if we add a block of height SPALING_ACTIVATION_HEIGHT +!, we can now scan both;
        blockdb.insert_block(cb2.height as u32, cb2_bytes).await.unwrap();
        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
        assert!(blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Scan(scan, StreamingManager::default()),
                None,
                None,
                &locked_notes_db
            )
            .await
            .is_ok());

        assert_eq!(
            walletdb.get_balance(AccountId(0)).await.unwrap(),
            Amount::from_u64(150_000).unwrap()
        );
    }

    #[wasm_bindgen_test]
    async fn test_scan_cached_blokcs_finds_received_notes() {
        // init blocks_db
        let ctx = mm_ctx_with_custom_db();
        let locked_notes_db = LockedNotesStorage::new(ctx.clone(), MY_ADDRESS.to_string())
            .await
            .unwrap();
        let blockdb = BlockDbImpl::new(&ctx, TICKER.to_string(), PathBuf::new())
            .await
            .unwrap();

        // init walletdb.
        let walletdb = wallet_db_from_zcoin_builder_for_test(&ctx, TICKER).await;
        let consensus_params = consensus_params();

        // Add an account to the wallet
        let extsk = ExtendedSpendingKey::master(&[]);
        let extfvk = ExtendedFullViewingKey::from(&extsk);
        assert!(walletdb.init_accounts_table(&[extfvk.clone()]).await.is_ok());

        // Account balance should be zero
        assert_eq!(walletdb.get_balance(AccountId(0)).await.unwrap(), Amount::zero());

        // Create a fake compactblock sending value to the address
        let value = Amount::from_u64(5).unwrap();
        let (cb1, _) = fake_compact_block(sapling_activation_height(), BlockHash([0; 32]), extfvk.clone(), value);
        let cb1_bytes = cb1.write_to_bytes().unwrap();
        blockdb.insert_block(cb1.height as u32, cb1_bytes).await.unwrap();

        // Scan the cache
        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
        assert!(blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Scan(scan, StreamingManager::default()),
                None,
                None,
                &locked_notes_db
            )
            .await
            .is_ok());

        // Account balance should reflect the received note
        assert_eq!(walletdb.get_balance(AccountId(0)).await.unwrap(), value);

        // Create a second fake Compactblock sending more value to the address
        let value2 = Amount::from_u64(7).unwrap();
        let (cb2, _) = fake_compact_block(sapling_activation_height() + 1, cb1.hash(), extfvk.clone(), value2);
        let cb2_bytes = cb2.write_to_bytes().unwrap();
        blockdb.insert_block(cb2.height as u32, cb2_bytes).await.unwrap();

        // Scan the cache again
        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
        assert!(blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Scan(scan, StreamingManager::default()),
                None,
                None,
                &locked_notes_db
            )
            .await
            .is_ok());

        // Account balance should reflect the received note
        assert_eq!(walletdb.get_balance(AccountId(0)).await.unwrap(), value + value2);
    }

    #[wasm_bindgen_test]
    async fn test_scan_cached_blocks_finds_change_notes() {
        // init blocks_db
        let ctx = mm_ctx_with_custom_db();
        let locked_notes_db = LockedNotesStorage::new(ctx.clone(), MY_ADDRESS.to_string())
            .await
            .unwrap();
        let blockdb = BlockDbImpl::new(&ctx, TICKER.to_string(), PathBuf::new())
            .await
            .unwrap();

        // init walletdb.
        let walletdb = wallet_db_from_zcoin_builder_for_test(&ctx, TICKER).await;
        let consensus_params = consensus_params();

        // Add an account to the wallet
        let extsk = ExtendedSpendingKey::master(&[]);
        let extfvk = ExtendedFullViewingKey::from(&extsk);
        assert!(walletdb.init_accounts_table(&[extfvk.clone()]).await.is_ok());

        // Account balance should be zero
        assert_eq!(walletdb.get_balance(AccountId(0)).await.unwrap(), Amount::zero());

        // Create a fake compactblock sending value to the address
        let value = Amount::from_u64(5).unwrap();
        let (cb1, nf) = fake_compact_block(sapling_activation_height(), BlockHash([0; 32]), extfvk.clone(), value);
        let cb1_bytes = cb1.write_to_bytes().unwrap();
        blockdb.insert_block(cb1.height as u32, cb1_bytes).await.unwrap();

        // Scan the cache
        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
        assert!(blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Scan(scan, StreamingManager::default()),
                None,
                None,
                &locked_notes_db
            )
            .await
            .is_ok());

        // Account balance should reflect the received note
        assert_eq!(walletdb.get_balance(AccountId(0)).await.unwrap(), value);

        // Create a second fake Compactblock spending value from the address
        let extsk2 = ExtendedSpendingKey::master(&[0]);
        let to2 = extsk2.default_address().unwrap().1;
        let value2 = Amount::from_u64(2).unwrap();
        let cb2 = fake_compact_block_spending(
            sapling_activation_height() + 1,
            cb1.hash(),
            (nf, value),
            extfvk,
            to2,
            value2,
        );
        let cb2_bytes = cb2.write_to_bytes().unwrap();
        blockdb.insert_block(cb2.height as u32, cb2_bytes).await.unwrap();

        // Scan the cache again
        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
        let scan = blockdb
            .process_blocks_with_mode(
                consensus_params.clone(),
                BlockProcessingMode::Scan(scan, StreamingManager::default()),
                None,
                None,
                &locked_notes_db,
            )
            .await;
        assert!(scan.is_ok());

        // Account balance should equal the change
        assert_eq!(walletdb.get_balance(AccountId(0)).await.unwrap(), value - value2);
    }

    fn network() -> Network {
        Network::TestNetwork
    }

    // Todo: Uncomment after improving tx creation time
    // https://github.com/KomodoPlatform/komodo-defi-framework/issues/2000
    //    #[wasm_bindgen_test]
    //    async fn create_to_address_fails_on_unverified_notes() {
    //        // init blocks_db
    //        let ctx = mm_ctx_with_custom_db();
    //        let locked_notes_db = LockedNotesStorage::new(ctx.clone(), MY_ADDRESS.to_string()).await.unwrap();
    //        let blockdb = BlockDbImpl::new(&ctx, TICKER.to_string(), PathBuf::new()).await.unwrap();
    //
    //        // init walletdb.
    //        let mut walletdb = wallet_db_from_zcoin_builder_for_test(&ctx, TICKER).await;
    //        let consensus_params = consensus_params();
    //
    //        // Add an account to the wallet
    //        let extsk = ExtendedSpendingKey::master(&[]);
    //        let extfvk = ExtendedFullViewingKey::from(&extsk);
    //        assert!(walletdb.init_accounts_table(&[extfvk.clone()]).await.is_ok());
    //
    //        // Account balance should be zero
    //        assert_eq!(walletdb.get_balance(AccountId(0)).await.unwrap(), Amount::zero());
    //
    //        // Add funds to the wallet in a single note
    //        let value = Amount::from_u64(50000).unwrap();
    //        let (cb, _) = fake_compact_block(sapling_activation_height(), BlockHash([0; 32]), extfvk.clone(), value);
    //        let cb_bytes = cb.write_to_bytes().unwrap();
    //        blockdb.insert_block(cb.height as u32, cb_bytes).await.unwrap();
    //
    //        // Scan the cache
    //        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
    //        assert!(blockdb
    //            .process_blocks_with_mode(consensus_params.clone(), BlockProcessingMode::Scan(scan, StreamingManager::default()), None, None)
    //            .await
    //            .is_ok());
    //
    //        // Verified balance matches total balance
    //        let (_, anchor_height) = walletdb.get_target_and_anchor_heights().await.unwrap().unwrap();
    //        assert_eq!(walletdb.get_balance(AccountId(0)).await.unwrap(), value);
    //        assert_eq!(
    //            walletdb.get_balance_at(AccountId(0), anchor_height).await.unwrap(),
    //            value
    //        );
    //
    //        // Add more funds to the wallet in a second note
    //        let (cb, _) = fake_compact_block(sapling_activation_height() + 1, cb.hash(), extfvk.clone(), value);
    //        let cb_bytes = cb.write_to_bytes().unwrap();
    //        blockdb.insert_block(cb.height as u32, cb_bytes).await.unwrap();
    //
    //        // Scan the cache
    //        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
    //        assert!(blockdb
    //            .process_blocks_with_mode(consensus_params.clone(), BlockProcessingMode::Scan(scan, StreamingManager::default()), None, None, &locked_notes_db)
    //            .await
    //            .is_ok());
    //
    //        // Verified balance does not include the second note
    //        let (_, anchor_height2) = walletdb.get_target_and_anchor_heights().await.unwrap().unwrap();
    //        assert_eq!(walletdb.get_balance(AccountId(0)).await.unwrap(), value + value);
    //        assert_eq!(
    //            walletdb.get_balance_at(AccountId(0), anchor_height2).await.unwrap(),
    //            value
    //        );
    //
    //        // Spend fails because there are insufficient verified notes
    //        let extsk2 = ExtendedSpendingKey::master(&[]);
    //        let to = extsk2.default_address().unwrap().1.into();
    //        match create_spend_to_address(
    //            &mut walletdb,
    //            &network(),
    //            test_prover().await,
    //            AccountId(0),
    //            &extsk,
    //            &to,
    //            Amount::from_u64(70000).unwrap(),
    //            None,
    //            OvkPolicy::Sender,
    //        )
    //        .await
    //        {
    //            Ok(_) => panic!("Should have failed"),
    //            Err(e) => assert!(e
    //                .to_string()
    //                .contains("Insufficient balance (have 50000, need 71000 including fee)")),
    //        }
    //
    //        // Mine blocks SAPLING_ACTIVATION_HEIGHT + 2 to 9 until just before the second
    //        // note is verified
    //        for i in 2..10 {
    //            let (cb, _) = fake_compact_block(sapling_activation_height() + i, cb.hash(), extfvk.clone(), value);
    //            let cb_bytes = cb.write_to_bytes().unwrap();
    //            blockdb.insert_block(cb.height as u32, cb_bytes).await.unwrap();
    //        }
    //
    //        // Scan the cache
    //        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
    //        assert!(blockdb
    //            .process_blocks_with_mode(consensus_params.clone(), BlockProcessingMode::Scan(scan, StreamingManager::default()), None, None, &locked_notes_db)
    //            .await
    //            .is_ok());
    //
    //        // Second spend still fails
    //        match create_spend_to_address(
    //            &mut walletdb,
    //            &network(),
    //            test_prover().await,
    //            AccountId(0),
    //            &extsk,
    //            &to,
    //            Amount::from_u64(70000).unwrap(),
    //            None,
    //            OvkPolicy::Sender,
    //        )
    //        .await
    //        {
    //            Ok(_) => panic!("Should have failed"),
    //            Err(e) => assert!(e
    //                .to_string()
    //                .contains("Insufficient balance (have 50000, need 71000 including fee)")),
    //        }
    //
    //        // Mine block 11 so that the second note becomes verified
    //        let (cb, _) = fake_compact_block(sapling_activation_height() + 10, cb.hash(), extfvk, value);
    //        let cb_bytes = cb.write_to_bytes().unwrap();
    //        blockdb.insert_block(cb.height as u32, cb_bytes).await.unwrap();
    //        // Scan the cache
    //        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
    //        assert!(blockdb
    //            .process_blocks_with_mode(consensus_params.clone(), BlockProcessingMode::Scan(scan, StreamingManager::default()), None, None, &locked_notes_db)
    //            .await
    //            .is_ok());
    //
    //        // Second spend should now succeed
    //        create_spend_to_address(
    //            &mut walletdb,
    //            &network(),
    //            test_prover().await,
    //            AccountId(0),
    //            &extsk,
    //            &to,
    //            Amount::from_u64(70000).unwrap(),
    //            None,
    //            OvkPolicy::Sender,
    //        )
    //        .await
    //        .unwrap();
    //    }

    #[wasm_bindgen_test]
    async fn test_create_to_address_fails_on_incorrect_extsk() {
        // init walletdb.
        let ctx = mm_ctx_with_custom_db();
        let mut walletdb = wallet_db_from_zcoin_builder_for_test(&ctx, TICKER).await;

        // Add two accounts to the wallet
        let extsk0 = ExtendedSpendingKey::master(&[]);
        let extsk1 = ExtendedSpendingKey::master(&[0]);
        let extfvks = [
            ExtendedFullViewingKey::from(&extsk0),
            ExtendedFullViewingKey::from(&extsk1),
        ];
        assert!(walletdb.init_accounts_table(&extfvks).await.is_ok());

        let to = extsk0.default_address().unwrap().1.into();
        match create_spend_to_address(
            &mut walletdb,
            &network(),
            test_prover().await,
            AccountId(0),
            &extsk1,
            &to,
            Amount::from_u64(1).unwrap(),
            None,
            OvkPolicy::Sender,
        )
        .await
        {
            Ok(_) => panic!("Should have failed"),
            Err(e) => assert!(e.to_string().contains("Incorrect ExtendedSpendingKey for account 0")),
        }

        match create_spend_to_address(
            &mut walletdb,
            &network(),
            test_prover().await,
            AccountId(1),
            &extsk0,
            &to,
            Amount::from_u64(1).unwrap(),
            None,
            OvkPolicy::Sender,
        )
        .await
        {
            Ok(_) => panic!("Should have failed"),
            Err(e) => assert!(e.to_string().contains("Incorrect ExtendedSpendingKey for account 1")),
        }
    }

    #[wasm_bindgen_test]
    async fn test_create_to_address_fails_with_no_blocks() {
        // init walletdb.
        let ctx = mm_ctx_with_custom_db();
        let mut walletdb = wallet_db_from_zcoin_builder_for_test(&ctx, TICKER).await;

        // Add two accounts to the wallet
        let extsk = ExtendedSpendingKey::master(&[]);
        let extfvks = [ExtendedFullViewingKey::from(&extsk)];
        assert!(walletdb.init_accounts_table(&extfvks).await.is_ok());

        let to = extsk.default_address().unwrap().1.into();
        match create_spend_to_address(
            &mut walletdb,
            &network(),
            test_prover().await,
            AccountId(0),
            &extsk,
            &to,
            Amount::from_u64(1).unwrap(),
            None,
            OvkPolicy::Sender,
        )
        .await
        {
            Ok(_) => panic!("Should have failed"),
            Err(e) => assert!(e.to_string().contains("Must scan blocks first")),
        }
    }

    #[wasm_bindgen_test]
    async fn test_create_to_address_fails_on_insufficient_balance() {
        // init walletdb.
        let ctx = mm_ctx_with_custom_db();
        let mut walletdb = wallet_db_from_zcoin_builder_for_test(&ctx, TICKER).await;

        assert!(walletdb
            .init_blocks_table(BlockHeight::from(1), BlockHash([1; 32]), 1, &[])
            .await
            .is_ok());

        // Add an account to the wallet
        let extsk = ExtendedSpendingKey::master(&[]);
        let extfvks = [ExtendedFullViewingKey::from(&extsk)];
        assert!(walletdb.init_accounts_table(&extfvks).await.is_ok());
        let to = extsk.default_address().unwrap().1.into();

        // Account balance should be zero
        assert_eq!(walletdb.get_balance(AccountId(0)).await.unwrap(), Amount::zero());

        // We cannot spend anything
        match create_spend_to_address(
            &mut walletdb,
            &network(),
            test_prover().await,
            AccountId(0),
            &extsk,
            &to,
            Amount::from_u64(1).unwrap(),
            None,
            OvkPolicy::Sender,
        )
        .await
        {
            Ok(_) => panic!("Should have failed"),
            Err(e) => assert!(e
                .to_string()
                .contains("Insufficient balance (have 0, need 1001 including fee)")),
        }
    }

    // Todo: Uncomment after improving tx creation time
    // https://github.com/KomodoPlatform/komodo-defi-framework/issues/2000
    //    #[wasm_bindgen_test]
    //    async fn test_create_to_address_fails_on_locked_notes() {
    //        register_wasm_log();
    //
    //        // init blocks_db
    //        let ctx = mm_ctx_with_custom_db();
    //        let locked_notes_db = LockedNotesStorage::new(ctx.clone(), MY_ADDRESS.to_string()).await.unwrap();
    //        let blockdb = BlockDbImpl::new(&ctx, TICKER.to_string(), PathBuf::new()).await.unwrap();
    //
    //        // init walletdb.
    //        let mut walletdb = wallet_db_from_zcoin_builder_for_test(&ctx, TICKER).await;
    //        let consensus_params = consensus_params();
    //
    //        // Add an account to the wallet
    //        let extsk = ExtendedSpendingKey::master(&[]);
    //        let extfvk = ExtendedFullViewingKey::from(&extsk);
    //        assert!(walletdb.init_accounts_table(&[extfvk.clone()]).await.is_ok());
    //
    //        // Add funds to the wallet in a single note
    //        let value = Amount::from_u64(50000).unwrap();
    //        let (cb, _) = fake_compact_block(sapling_activation_height(), BlockHash([0; 32]), extfvk, value);
    //        let cb_bytes = cb.write_to_bytes().unwrap();
    //        blockdb.insert_block(cb.height as u32, cb_bytes).await.unwrap();
    //
    //        // Scan the cache
    //        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
    //        blockdb
    //            .process_blocks_with_mode(consensus_params.clone(), BlockProcessingMode::Scan(scan, StreamingManager::default()), None, None, &locked_notes_db)
    //            .await
    //            .unwrap();
    //        assert_eq!(walletdb.get_balance(AccountId(0)).await.unwrap(), value);
    //
    //        // Send some of the funds to another address
    //        let extsk2 = ExtendedSpendingKey::master(&[]);
    //        let to = extsk2.default_address().unwrap().1.into();
    //        create_spend_to_address(
    //            &mut walletdb,
    //            &network(),
    //            test_prover().await,
    //            AccountId(0),
    //            &extsk,
    //            &to,
    //            Amount::from_u64(15000).unwrap(),
    //            None,
    //            OvkPolicy::Sender,
    //        )
    //        .await
    //        .unwrap();
    //
    //        // A second spend fails because there are no usable notes
    //        match create_spend_to_address(
    //            &mut walletdb,
    //            &network(),
    //            test_prover().await,
    //            AccountId(0),
    //            &extsk,
    //            &to,
    //            Amount::from_u64(2000).unwrap(),
    //            None,
    //            OvkPolicy::Sender,
    //        )
    //        .await
    //        {
    //            Ok(_) => panic!("Should have failed"),
    //            Err(e) => assert!(e
    //                .to_string()
    //                .contains("Insufficient balance (have 0, need 3000 including fee)")),
    //        }
    //
    //        // Mine blocks SAPLING_ACTIVATION_HEIGHT + 1 to 21 (that don't send us funds)
    //        // until just before the first transaction expires
    //        for i in 1..22 {
    //            let (cb, _) = fake_compact_block(
    //                sapling_activation_height() + i,
    //                cb.hash(),
    //                ExtendedFullViewingKey::from(&ExtendedSpendingKey::master(&[i as u8])),
    //                value,
    //            );
    //            let cb_bytes = cb.write_to_bytes().unwrap();
    //            blockdb.insert_block(cb.height as u32, cb_bytes).await.unwrap();
    //        }
    //        // Scan the cache
    //        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
    //        blockdb
    //            .process_blocks_with_mode(consensus_params.clone(), BlockProcessingMode::Scan(scan, StreamingManager::default()), None, None, &locked_notes_db)
    //            .await
    //            .unwrap();
    //
    //        // Second spend still fails
    //        match create_spend_to_address(
    //            &mut walletdb,
    //            &network(),
    //            test_prover().await,
    //            AccountId(0),
    //            &extsk,
    //            &to,
    //            Amount::from_u64(2000).unwrap(),
    //            None,
    //            OvkPolicy::Sender,
    //        )
    //        .await
    //        {
    //            Ok(_) => panic!("Should have failed"),
    //            Err(e) => assert!(e
    //                .to_string()
    //                .contains("Insufficient balance (have 0, need 3000 including fee)")),
    //        }
    //
    //        // Mine block SAPLING_ACTIVATION_HEIGHT + 22 so that the first transaction expires
    //        let (cb, _) = fake_compact_block(
    //            sapling_activation_height() + 22,
    //            cb.hash(),
    //            ExtendedFullViewingKey::from(&ExtendedSpendingKey::master(&[22])),
    //            value,
    //        );
    //        let cb_bytes = cb.write_to_bytes().unwrap();
    //        blockdb.insert_block(cb.height as u32, cb_bytes).await.unwrap();
    //        // Scan the cache
    //        let scan = DataConnStmtCacheWrapper::new(DataConnStmtCacheWasm(walletdb.clone()));
    //        blockdb
    //            .process_blocks_with_mode(consensus_params.clone(), BlockProcessingMode::Scan(scan, StreamingManager::default()), None, None, &locked_notes_db)
    //            .await
    //            .unwrap();
    //
    //        // Second spend should now succeed
    //        create_spend_to_address(
    //            &mut walletdb,
    //            &network(),
    //            test_prover().await,
    //            AccountId(0),
    //            &extsk,
    //            &to,
    //            Amount::from_u64(2000).unwrap(),
    //            None,
    //            OvkPolicy::Sender,
    //        )
    //        .await
    //        .unwrap();
    //    }
}
