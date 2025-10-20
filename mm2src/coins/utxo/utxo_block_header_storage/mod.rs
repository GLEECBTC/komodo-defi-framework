#[cfg(not(target_arch = "wasm32"))]
mod sql_block_header_storage;
#[cfg(not(target_arch = "wasm32"))]
pub use sql_block_header_storage::SqliteBlockHeadersStorage;

#[cfg(target_arch = "wasm32")]
mod wasm;
#[cfg(target_arch = "wasm32")]
pub use wasm::IDBBlockHeadersStorage;

use async_trait::async_trait;
use chain::BlockHeader;
use mm2_core::mm_ctx::MmArc;
#[cfg(all(test, not(target_arch = "wasm32")))]
use mocktopus::macros::*;
use primitives::hash::H256;
use spv_validation::storage::{BlockHeaderStorageError, BlockHeaderStorageOps};
use std::collections::HashMap;
use std::fmt::{Debug, Formatter};

pub struct BlockHeaderStorage {
    pub inner: Box<dyn BlockHeaderStorageOps>,
}

impl Debug for BlockHeaderStorage {
    fn fmt(&self, _f: &mut Formatter<'_>) -> std::fmt::Result {
        Ok(())
    }
}

impl BlockHeaderStorage {
    #[cfg(all(not(test), not(target_arch = "wasm32")))]
    pub(crate) fn new_from_ctx(ctx: MmArc, ticker: String) -> Result<Self, BlockHeaderStorageError> {
        #[cfg(not(feature = "new-db-arch"))]
        let maybe_sqlite_connection = ctx.sqlite_connection.get();
        #[cfg(feature = "new-db-arch")]
        let maybe_sqlite_connection = ctx.global_db_conn.get();
        let sqlite_connection = maybe_sqlite_connection.ok_or(BlockHeaderStorageError::Internal(
            "BlockHeaderStorage's SQL DB is not initialized".to_owned(),
        ))?;
        Ok(BlockHeaderStorage {
            inner: Box::new(SqliteBlockHeadersStorage {
                ticker,
                conn: sqlite_connection.clone(),
            }),
        })
    }

    #[cfg(target_arch = "wasm32")]
    pub(crate) fn new_from_ctx(ctx: MmArc, ticker: String) -> Result<Self, BlockHeaderStorageError> {
        Ok(BlockHeaderStorage {
            inner: Box::new(IDBBlockHeadersStorage::new(&ctx, ticker)),
        })
    }

    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn new_from_ctx(ctx: MmArc, ticker: String) -> Result<Self, BlockHeaderStorageError> {
        use db_common::sqlite::rusqlite::Connection;
        use std::sync::{Arc, Mutex};

        let conn = ctx
            .sqlite_connection
            .get()
            .cloned()
            .unwrap_or_else(|| Arc::new(Mutex::new(Connection::open_in_memory().unwrap())));

        Ok(BlockHeaderStorage {
            inner: Box::new(SqliteBlockHeadersStorage { ticker, conn }),
        })
    }

    #[allow(dead_code)]
    pub(crate) fn into_inner(self) -> Box<dyn BlockHeaderStorageOps> {
        self.inner
    }
}

#[async_trait]
#[cfg_attr(all(test, not(target_arch = "wasm32")), mockable)]
impl BlockHeaderStorageOps for BlockHeaderStorage {
    async fn init(&self) -> Result<(), BlockHeaderStorageError> {
        self.inner.init().await
    }

    async fn is_initialized_for(&self) -> Result<bool, BlockHeaderStorageError> {
        self.inner.is_initialized_for().await
    }

    async fn add_block_headers_to_storage(
        &self,
        headers: HashMap<u64, BlockHeader>,
    ) -> Result<(), BlockHeaderStorageError> {
        self.inner.add_block_headers_to_storage(headers).await
    }

    async fn get_block_header(&self, height: u64) -> Result<Option<BlockHeader>, BlockHeaderStorageError> {
        self.inner.get_block_header(height).await
    }

    async fn get_block_header_raw(&self, height: u64) -> Result<Option<String>, BlockHeaderStorageError> {
        self.inner.get_block_header_raw(height).await
    }

    async fn get_last_block_height(&self) -> Result<Option<u64>, BlockHeaderStorageError> {
        self.inner.get_last_block_height().await
    }

    async fn get_last_block_header_with_non_max_bits(
        &self,
        max_bits: u32,
    ) -> Result<Option<BlockHeader>, BlockHeaderStorageError> {
        self.inner.get_last_block_header_with_non_max_bits(max_bits).await
    }

    async fn get_block_height_by_hash(&self, hash: H256) -> Result<Option<i64>, BlockHeaderStorageError> {
        self.inner.get_block_height_by_hash(hash).await
    }

    async fn remove_headers_from_storage(
        &self,
        from_height: u64,
        to_height: u64,
    ) -> Result<(), BlockHeaderStorageError> {
        self.inner.remove_headers_from_storage(from_height, to_height).await
    }

    async fn is_table_empty(&self) -> Result<(), BlockHeaderStorageError> {
        self.inner.is_table_empty().await
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
mod block_headers_storage_tests {
    use super::*;
    use chain::BlockHeaderBits;
    use mm2_test_helpers::for_tests::mm_ctx_with_custom_db;

    cfg_wasm32! {
        use wasm_bindgen_test::*;
        use spv_validation::work::MAX_BITS_BTC;

        wasm_bindgen_test_configure!(run_in_browser);
    }

    cfg_native! {
        use spv_validation::work::MAX_BITS_BTC;

    }

    pub(crate) async fn test_add_block_headers_impl(for_coin: &str) {
        let ctx = mm_ctx_with_custom_db();
        let storage = BlockHeaderStorage::new_from_ctx(ctx, for_coin.to_string())
            .unwrap()
            .into_inner();
        storage.init().await.unwrap();

        let mut headers = HashMap::with_capacity(1);
        let block_header: BlockHeader = "0000002076d41d3e4b0bfd4c0d3b30aa69fdff3ed35d85829efd04000000000000000000b386498b583390959d9bac72346986e3015e83ac0b54bc7747a11a494ac35c94bb3ce65a53fb45177f7e311c".into();
        headers.insert(520481, block_header);
        storage.add_block_headers_to_storage(headers).await.unwrap();
        assert!(storage.is_table_empty().await.is_err());
    }

    pub(crate) async fn test_get_block_header_impl(for_coin: &str) {
        let ctx = mm_ctx_with_custom_db();
        let storage = BlockHeaderStorage::new_from_ctx(ctx, for_coin.to_string())
            .unwrap()
            .into_inner();
        storage.init().await.unwrap();

        let mut headers = HashMap::with_capacity(1);
        let block_header: BlockHeader = "0000002076d41d3e4b0bfd4c0d3b30aa69fdff3ed35d85829efd04000000000000000000b386498b583390959d9bac72346986e3015e83ac0b54bc7747a11a494ac35c94bb3ce65a53fb45177f7e311c".into();
        headers.insert(520481, block_header);

        storage.add_block_headers_to_storage(headers).await.unwrap();
        assert!(storage.is_table_empty().await.is_err());

        let hex = storage.get_block_header_raw(520481).await.unwrap().unwrap();
        assert_eq!(hex, "0000002076d41d3e4b0bfd4c0d3b30aa69fdff3ed35d85829efd04000000000000000000b386498b583390959d9bac72346986e3015e83ac0b54bc7747a11a494ac35c94bb3ce65a53fb45177f7e311c".to_string());

        let block_header = storage.get_block_header(520481).await.unwrap().unwrap();
        let block_hash: H256 = "0000000000000000002e31d0714a5ab23100945ff87ba2d856cd566a3c9344ec".into();
        assert_eq!(block_header.hash(), block_hash.reversed());

        let height = storage.get_block_height_by_hash(block_hash).await.unwrap().unwrap();
        assert_eq!(height, 520481);
    }

    pub(crate) async fn test_get_last_block_header_with_non_max_bits_impl(for_coin: &str) {
        let ctx = mm_ctx_with_custom_db();
        let storage = BlockHeaderStorage::new_from_ctx(ctx, for_coin.to_string())
            .unwrap()
            .into_inner();
        storage.init().await.unwrap();

        let mut headers = HashMap::with_capacity(2);

        // This block has max difficulty
        // https://live.blockcypher.com/btc-testnet/block/00000000961a9d117feb57e516e17217207a849bf6cdfce529f31d9a96053530/
        let block_header: BlockHeader = "02000000ea01a61a2d7420a1b23875e40eb5eb4ca18b378902c8e6384514ad0000000000c0c5a1ae80582b3fe319d8543307fa67befc2a734b8eddb84b1780dfdf11fa2b20e71353ffff001d00805fe0".into();
        headers.insert(201595, block_header);

        // https://live.blockcypher.com/btc-testnet/block/0000000000ad144538e6c80289378ba14cebb50ee47538b2a120742d1aa601ea/
        let expected_block_header: BlockHeader = "02000000cbed7fd98f1f06e85c47e13ff956533642056be45e7e6b532d4d768f00000000f2680982f333fcc9afa7f9a5e2a84dc54b7fe10605cd187362980b3aa882e9683be21353ab80011c813e1fc0".into();
        headers.insert(201594, expected_block_header.clone());

        // This block has max difficulty
        // https://live.blockcypher.com/btc-testnet/block/0000000000ad144538e6c80289378ba14cebb50ee47538b2a120742d1aa601ea/
        let block_header: BlockHeader = "020000001f38c8e30b30af912fbd4c3e781506713cfb43e73dff6250348e060000000000afa8f3eede276ccb4c4ee649ad9823fc181632f262848ca330733e7e7e541beb9be51353ffff001d00a63037".into();
        headers.insert(201593, block_header);

        storage.add_block_headers_to_storage(headers).await.unwrap();
        assert!(storage.is_table_empty().await.is_err());

        let actual_block_header = storage
            .get_last_block_header_with_non_max_bits(MAX_BITS_BTC)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(actual_block_header.bits, BlockHeaderBits::Compact(MAX_BITS_BTC.into()));
        assert_eq!(actual_block_header, expected_block_header);
    }

    pub(crate) async fn test_get_last_block_height_impl(for_coin: &str) {
        let ctx = mm_ctx_with_custom_db();
        let storage = BlockHeaderStorage::new_from_ctx(ctx, for_coin.to_string())
            .unwrap()
            .into_inner();
        storage.init().await.unwrap();

        let mut headers = HashMap::with_capacity(2);

        // https://live.blockcypher.com/btc-testnet/block/00000000961a9d117feb57e516e17217207a849bf6cdfce529f31d9a96053530/
        let block_header: BlockHeader = "02000000ea01a61a2d7420a1b23875e40eb5eb4ca18b378902c8e6384514ad0000000000c0c5a1ae80582b3fe319d8543307fa67befc2a734b8eddb84b1780dfdf11fa2b20e71353ffff001d00805fe0".into();
        headers.insert(201595, block_header);

        // https://live.blockcypher.com/btc-testnet/block/0000000000ad144538e6c80289378ba14cebb50ee47538b2a120742d1aa601ea/
        let block_header: BlockHeader = "02000000cbed7fd98f1f06e85c47e13ff956533642056be45e7e6b532d4d768f00000000f2680982f333fcc9afa7f9a5e2a84dc54b7fe10605cd187362980b3aa882e9683be21353ab80011c813e1fc0".into();
        headers.insert(201594, block_header);

        // https://live.blockcypher.com/btc-testnet/block/0000000000ad144538e6c80289378ba14cebb50ee47538b2a120742d1aa601ea/
        let block_header: BlockHeader = "020000001f38c8e30b30af912fbd4c3e781506713cfb43e73dff6250348e060000000000afa8f3eede276ccb4c4ee649ad9823fc181632f262848ca330733e7e7e541beb9be51353ffff001d00a63037".into();
        headers.insert(201593, block_header);

        storage.add_block_headers_to_storage(headers).await.unwrap();
        assert!(storage.is_table_empty().await.is_err());

        let last_block_height = storage.get_last_block_height().await.unwrap();
        assert_eq!(last_block_height.unwrap(), 201595);
    }

    pub(crate) async fn test_remove_headers_from_storage_impl(for_coin: &str) {
        let ctx = mm_ctx_with_custom_db();
        let storage = BlockHeaderStorage::new_from_ctx(ctx, for_coin.to_string())
            .unwrap()
            .into_inner();
        storage.init().await.unwrap();

        // test remove headers from height.
        let mut headers = HashMap::with_capacity(5);

        // https://live.blockcypher.com/btc-testnet/block/00000000016a2f4a57ff9b9422ddc09adb753b689324899fcdc56172f55480f7/
        let block_header: BlockHeader = "02000000f2a57f6b614df598ff8dff068292bd862c2bb0c12e4e380638db5700000000002349f389569e582d42cb51aa584f5a74f977c3cf86d2c2ac63b1bbde7fc95dc7f1e61353ab80011c8585405d".into();
        headers.insert(201597, block_header);

        // https://live.blockcypher.com/btc-testnet/block/000000000057db3806384e2ec1b02b2c86bd928206ff8dff98f54d616b7fa5f2/
        let block_header: BlockHeader = "02000000303505969a1df329e5fccdf69b847a201772e116e557eb7f119d1a9600000000469267f52f43b8799e72f0726ba2e56432059a8ad02b84d4fff84b9476e95f7716e41353ab80011c168cb471".into();
        headers.insert(201596, block_header);

        // https://live.blockcypher.com/btc-testnet/block/00000000961a9d117feb57e516e17217207a849bf6cdfce529f31d9a96053530/
        let block_header: BlockHeader = "02000000ea01a61a2d7420a1b23875e40eb5eb4ca18b378902c8e6384514ad0000000000c0c5a1ae80582b3fe319d8543307fa67befc2a734b8eddb84b1780dfdf11fa2b20e71353ffff001d00805fe0".into();
        headers.insert(201595, block_header);

        // https://live.blockcypher.com/btc-testnet/block/0000000000ad144538e6c80289378ba14cebb50ee47538b2a120742d1aa601ea/
        let block_header: BlockHeader = "02000000cbed7fd98f1f06e85c47e13ff956533642056be45e7e6b532d4d768f00000000f2680982f333fcc9afa7f9a5e2a84dc54b7fe10605cd187362980b3aa882e9683be21353ab80011c813e1fc0".into();
        headers.insert(201594, block_header);

        // https://live.blockcypher.com/btc-testnet/block/0000000000ad144538e6c80289378ba14cebb50ee47538b2a120742d1aa601ea/
        let block_header: BlockHeader = "020000001f38c8e30b30af912fbd4c3e781506713cfb43e73dff6250348e060000000000afa8f3eede276ccb4c4ee649ad9823fc181632f262848ca330733e7e7e541beb9be51353ffff001d00a63037".into();
        headers.insert(201593, block_header);

        storage.add_block_headers_to_storage(headers).await.unwrap();
        assert!(storage.is_table_empty().await.is_err());

        // Remove 4 headers from storage
        storage.remove_headers_from_storage(201593, 201596).await.unwrap();

        // Validate that block headers 201593 to 201596 are removed from storage
        // Note that 201593..201597 is exclusive meaning it includes the first value 201593 but excludes the last value 201597
        for h in 201593..201597 {
            let block_header = storage.get_block_header(h).await.unwrap();
            assert!(block_header.is_none());
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod native_tests {
    use super::*;
    use crate::utxo::utxo_block_header_storage::block_headers_storage_tests::*;
    use common::block_on;
    use mm2_test_helpers::for_tests::mm_ctx_with_custom_db;

    #[test]
    fn test_init_collection() {
        let for_coin = "init_collection";
        let ctx = mm_ctx_with_custom_db();
        let storage = BlockHeaderStorage::new_from_ctx(ctx, for_coin.to_string())
            .unwrap()
            .into_inner();

        let initialized = block_on(storage.is_initialized_for()).unwrap();
        assert!(!initialized);

        block_on(storage.init()).unwrap();
        // repetitive init must not fail
        block_on(storage.init()).unwrap();

        let initialized = block_on(storage.is_initialized_for()).unwrap();
        assert!(initialized);
    }

    const FOR_COIN_GET: &str = "get";
    const FOR_COIN_INSERT: &str = "insert";
    #[test]
    fn test_add_block_headers() {
        block_on(test_add_block_headers_impl(FOR_COIN_INSERT))
    }

    #[test]
    fn test_test_get_block_header() {
        block_on(test_get_block_header_impl(FOR_COIN_GET))
    }

    #[test]
    fn test_get_last_block_header_with_non_max_bits() {
        block_on(test_get_last_block_header_with_non_max_bits_impl(FOR_COIN_GET))
    }

    #[test]
    fn test_get_last_block_height() {
        block_on(test_get_last_block_height_impl(FOR_COIN_GET))
    }

    #[test]
    fn test_remove_headers_from_storage() {
        block_on(test_remove_headers_from_storage_impl(FOR_COIN_GET))
    }
}

#[cfg(target_arch = "wasm32")]
mod wasm_test {
    use super::*;
    use crate::utxo::utxo_block_header_storage::block_headers_storage_tests::*;
    use common::log::wasm_log::register_wasm_log;
    use mm2_test_helpers::for_tests::mm_ctx_with_custom_db;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    const FOR_COIN: &str = "tBTC";

    #[wasm_bindgen_test]
    async fn test_storage_init() {
        let ctx = mm_ctx_with_custom_db();
        let storage = IDBBlockHeadersStorage::new(&ctx, "RICK".to_string());

        register_wasm_log();

        let initialized = storage.is_initialized_for().await.unwrap();
        assert!(initialized);

        // repetitive init must not fail
        storage.init().await.unwrap();

        let initialized = storage.is_initialized_for().await.unwrap();
        assert!(initialized);
    }

    #[wasm_bindgen_test]
    async fn test_add_block_headers() {
        test_add_block_headers_impl(FOR_COIN).await
    }

    #[wasm_bindgen_test]
    async fn test_test_get_block_header() {
        test_get_block_header_impl(FOR_COIN).await
    }

    #[wasm_bindgen_test]
    async fn test_get_last_block_header_with_non_max_bits() {
        test_get_last_block_header_with_non_max_bits_impl(FOR_COIN).await
    }

    #[wasm_bindgen_test]
    async fn test_get_last_block_height() {
        test_get_last_block_height_impl(FOR_COIN).await
    }

    #[wasm_bindgen_test]
    async fn test_remove_headers_from_storage() {
        test_remove_headers_from_storage_impl(FOR_COIN).await
    }
}
