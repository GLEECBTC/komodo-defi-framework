use derive_more::Display;
use enum_derives::EnumFromStringify;

cfg_native!(
    pub(crate) mod sqlite;

    use db_common::async_sql_conn::{AsyncConnError, AsyncConnection};
    use futures::lock::Mutex;
    use std::sync::Arc;
);

cfg_wasm32!(
    pub(crate) mod wasm;

    use self::wasm::LockedNoteDbInner;
    use mm2_db::indexed_db::{DbTransactionError, InitDbError, SharedDb};
);

/// Represents a shielded note temporarily locked due to a pending transaction.
/// Locked notes are excluded from the spendable balance until confirmed or cleared.
#[derive(Debug, Clone)]
pub(crate) enum LockedNote {
    /// A note being spent by a pending shielded transaction (`rseed` is the note's randomness).
    Spent { rseed: String },

    /// A pending change output from an unconfirmed shielded transaction (`value` is the expected amount).
    Change { value: u64 },
}

/// A wrapper for the db connection to the change note cache database in native and browser.
#[derive(Clone)]
pub struct LockedNotesStorage {
    #[cfg(not(target_arch = "wasm32"))]
    pub db: Arc<Mutex<AsyncConnection>>,
    #[cfg(target_arch = "wasm32")]
    pub db: SharedDb<LockedNoteDbInner>,
    #[allow(unused)]
    address: String,
}

#[derive(Clone, Debug, Display, Eq, PartialEq, EnumFromStringify)]
pub(crate) enum LockedNotesStorageError {
    #[cfg(not(target_arch = "wasm32"))]
    #[display(fmt = "Sqlite Error: {_0}")]
    #[from_stringify("AsyncConnError", "db_common::sqlite::rusqlite::Error")]
    SqliteError(String),
    #[cfg(target_arch = "wasm32")]
    #[display(fmt = "IndexedDb Error: {_0}")]
    #[from_stringify("InitDbError", "DbTransactionError")]
    IndexedDbError(String),
}

#[cfg(any(test, target_arch = "wasm32"))]
pub(super) mod locked_notes_test {
    use crate::z_coin::storage::z_locked_notes::{LockedNote, LockedNotesStorage};
    use common::cross_test;
    use mm2_test_helpers::for_tests::mm_ctx_with_custom_db;

    common::cfg_wasm32! {
        use wasm_bindgen_test::*;
        wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);
    }

    const MY_ADDRESS: &str = "my_address";

    cross_test!(test_insert_and_remove_note, {
        let ctx = mm_ctx_with_custom_db();
        let db = LockedNotesStorage::new(&ctx, MY_ADDRESS.to_string()).await.unwrap();

        // Insert a pending spent note
        let spent_txid = "0x18b1acd8ceae8d71a2ae8b7e4a3e48ceb39dc237f0aa38c468425b88dc8d5f3e".to_string();
        let spent_rseed = "0xcfec34a81e67e85aa1ce1a6666f92f9bc5606f0795be555bb3c9f9ac089aa4f7".to_string();
        db.insert_spent_note(spent_txid.clone(), spent_rseed.clone())
            .await
            .unwrap();

        // Insert a pending change note
        let change_txid = "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef".to_string();
        let change_value = 123456;
        db.insert_change_note(change_txid.clone(), change_value).await.unwrap();

        // Remove by txid
        db.remove_notes_for_txid(spent_txid.clone()).await.unwrap();
        db.remove_notes_for_txid(change_txid.clone()).await.unwrap();

        let notes = db.load_all_notes().await.unwrap();
        assert!(notes.is_empty());

        // Insert both again but using same txid
        db.insert_spent_note(spent_txid.clone(), spent_rseed.clone())
            .await
            .unwrap();
        db.insert_change_note(spent_txid.clone(), change_value).await.unwrap();

        // Remove by txid (removes both input and output if same txid)
        db.remove_notes_for_txid(spent_txid.clone()).await.unwrap();

        let notes = db.load_all_notes().await.unwrap();
        assert!(notes.is_empty());
    });

    cross_test!(test_load_all_notes, {
        let ctx = mm_ctx_with_custom_db();
        let db = LockedNotesStorage::new(&ctx, MY_ADDRESS.to_string()).await.unwrap();

        let spent_txid = "0x01".to_string();
        let spent_rseed = "0xcafe000000000000000000000000000000000000000000000000000000000000".to_string();
        let change_txid = "0x02".to_string();
        let change_value = 123456789;

        db.insert_spent_note(spent_txid.clone(), spent_rseed.clone())
            .await
            .unwrap();
        db.insert_change_note(change_txid.clone(), change_value).await.unwrap();

        let notes = db.load_all_notes().await.unwrap();

        assert_eq!(notes.len(), 2);

        match &notes[0] {
            LockedNote::Spent { rseed } => {
                assert_eq!(rseed, &spent_rseed);
            },
            _ => panic!("First note should be a Spent note"),
        }
        match &notes[1] {
            LockedNote::Change { value } => {
                assert_eq!(*value, change_value);
            },
            _ => panic!("Second note should be a Change note"),
        }
    });

    cross_test!(test_sum_changes, {
        let ctx = mm_ctx_with_custom_db();
        let db = LockedNotesStorage::new(&ctx, MY_ADDRESS.to_string()).await.unwrap();

        db.insert_change_note("txid1".to_string(), 1000).await.unwrap();
        db.insert_change_note("txid2".to_string(), 2000).await.unwrap();
        db.insert_spent_note("0xinputrseed".to_string(), "txid3".to_string())
            .await
            .unwrap();

        let notes = db.load_all_notes().await.unwrap();

        // Only sum Output note values
        let sum: u64 = notes
            .iter()
            .filter_map(|n| match n {
                LockedNote::Change { value, .. } => Some(*value),
                _ => None,
            })
            .sum();
        assert_eq!(sum, 3000);
    });
}
