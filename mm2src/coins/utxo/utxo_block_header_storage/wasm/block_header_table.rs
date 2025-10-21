use mm2_db::indexed_db::{BeBigUint, DbUpgrader, OnUpgradeResult, TableSignature};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BlockHeaderStorageTable {
    pub height: BeBigUint,
    pub bits: u32,
    pub hash: String,
    pub raw_header: String,
    pub ticker: String,
}

impl BlockHeaderStorageTable {
    pub const TICKER_HEIGHT_INDEX: &'static str = "block_height_ticker_index";
    pub const HASH_TICKER_INDEX: &'static str = "block_hash_ticker_index";
}

impl TableSignature for BlockHeaderStorageTable {
    const TABLE_NAME: &'static str = "block_header_storage_cache_table";

    fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
        if let (0, 1) = (old_version, new_version) {
            let table = upgrader.create_table(Self::TABLE_NAME)?;
            table.create_multi_index(Self::TICKER_HEIGHT_INDEX, &["ticker", "height"], true)?;
            table.create_multi_index(Self::HASH_TICKER_INDEX, &["hash", "ticker"], true)?;
            table.create_index("ticker", false)?;
        }
        Ok(())
    }
}
