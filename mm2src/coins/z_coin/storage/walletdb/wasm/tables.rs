use mm2_db::indexed_db::{DbUpgrader, OnUpgradeResult, TableSignature};
use mm2_number::BigInt;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WalletDbAccountsTable {
    pub account: BigInt,
    pub extfvk: String,
    pub address: String,
    pub ticker: String,
}

impl WalletDbAccountsTable {
    /// A **unique** index that consists of the following properties:
    /// * ticker
    /// * account
    pub const TICKER_ACCOUNT_INDEX: &'static str = "ticker_account_index";
    /// A **unique** index that consists of the following properties:
    /// * ticker
    /// * account
    /// * extfvk
    pub const TICKER_ACCOUNT_EXTFVK_INDEX: &'static str = "ticker_account_extfvk_index";
}

impl TableSignature for WalletDbAccountsTable {
    const TABLE_NAME: &'static str = "walletdb_accounts";

    fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
        if let (0, 1) = (old_version, new_version) {
            let table = upgrader.create_table(Self::TABLE_NAME)?;
            table.create_multi_index(Self::TICKER_ACCOUNT_INDEX, &["ticker", "account"], true)?;
            table.create_multi_index(
                Self::TICKER_ACCOUNT_EXTFVK_INDEX,
                &["ticker", "account", "extfvk"],
                false,
            )?;
            table.create_index("ticker", false)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WalletDbBlocksTable {
    pub height: u32,
    pub hash: Vec<u8>,
    pub time: u32,
    pub sapling_tree: Vec<u8>,
    pub ticker: String,
}

impl WalletDbBlocksTable {
    /// A **unique** index that consists of the following properties:
    /// * ticker
    /// * height
    pub const TICKER_HEIGHT_INDEX: &'static str = "ticker_height_index";
    /// A **unique** index that consists of the following properties:
    /// * ticker
    /// * hash
    pub const TICKER_HASH_INDEX: &'static str = "ticker_hash_index";
}

impl TableSignature for WalletDbBlocksTable {
    const TABLE_NAME: &'static str = "walletdb_blocks";

    fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
        if let (0, 1) = (old_version, new_version) {
            let table = upgrader.create_table(Self::TABLE_NAME)?;
            table.create_multi_index(Self::TICKER_HEIGHT_INDEX, &["ticker", "height"], true)?;
            table.create_multi_index(Self::TICKER_HASH_INDEX, &["ticker", "hash"], true)?;
            table.create_index("ticker", false)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WalletDbTransactionsTable {
    /// Unique field
    pub txid: Vec<u8>,
    pub created: Option<String>,
    pub block: Option<u32>,
    pub tx_index: Option<i64>,
    pub expiry_height: Option<u32>,
    pub raw: Option<Vec<u8>>,
    pub ticker: String,
}

impl WalletDbTransactionsTable {
    /// A **unique** index that consists of the following properties:
    /// * ticker
    /// * txid
    pub const TICKER_TXID_INDEX: &'static str = "ticker_txid_index";
    /// A **unique** index that consists of the following properties:
    /// * ticker
    /// * block
    pub const TICKER_BLOCK_INDEX: &'static str = "ticker_block_index";
    /// A **unique** index that consists of the following properties:
    /// * ticker
    /// * expiry_height
    pub const TICKER_EXP_HEIGHT_INDEX: &'static str = "ticker_expiry_height_index";
}

impl TableSignature for WalletDbTransactionsTable {
    const TABLE_NAME: &'static str = "walletdb_transactions";

    fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
        if let (0, 1) = (old_version, new_version) {
            let table = upgrader.create_table(Self::TABLE_NAME)?;
            table.create_multi_index(Self::TICKER_TXID_INDEX, &["ticker", "txid"], true)?;
            table.create_multi_index(Self::TICKER_BLOCK_INDEX, &["ticker", "block"], false)?;
            table.create_multi_index(Self::TICKER_EXP_HEIGHT_INDEX, &["ticker", "expiry_height"], false)?;
            table.create_index("ticker", false)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WalletDbReceivedNotesTable {
    /// references transactions(id_tx)
    pub tx: u32,
    pub output_index: u32,
    /// references accounts(account)
    pub account: BigInt,
    pub diversifier: Vec<u8>,
    pub value: BigInt,
    pub rcm: Vec<u8>,
    /// Unique field
    pub nf: Option<Vec<u8>>,
    pub is_change: Option<bool>,
    pub memo: Option<Vec<u8>>,
    /// references transactions(id_tx)
    pub spent: Option<BigInt>,
    pub ticker: String,
}

impl WalletDbReceivedNotesTable {
    /// A **unique** index that consists of the following properties:
    /// * ticker
    /// * account
    pub const TICKER_ACCOUNT_INDEX: &'static str = "ticker_account_index";
    /// A **unique** index that consists of the following properties:
    /// * ticker
    /// * tx
    /// * output_index
    pub const TICKER_TX_OUTPUT_INDEX: &'static str = "ticker_tx_output_index";
    /// A **unique** index that consists of the following properties:
    /// * ticker
    /// * tx
    /// * output_index
    pub const TICKER_NF_INDEX: &'static str = "ticker_nf_index";
}

impl TableSignature for WalletDbReceivedNotesTable {
    const TABLE_NAME: &'static str = "walletdb_received_notes";

    fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
        if let (0, 1) = (old_version, new_version) {
            let table = upgrader.create_table(Self::TABLE_NAME)?;
            table.create_multi_index(Self::TICKER_NF_INDEX, &["ticker", "nf"], true)?;
            table.create_multi_index(Self::TICKER_ACCOUNT_INDEX, &["ticker", "account"], false)?;
            table.create_multi_index(Self::TICKER_TX_OUTPUT_INDEX, &["ticker", "tx", "output_index"], false)?;
            table.create_index("ticker", false)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WalletDbSaplingWitnessesTable {
    /// REFERENCES received_notes(id_note)
    pub note: BigInt,
    /// REFERENCES blocks(height)
    pub block: u32,
    pub witness: Vec<u8>,
    pub ticker: String,
}

impl WalletDbSaplingWitnessesTable {
    /// A **unique** index that consists of the following properties:
    /// * ticker
    /// * block
    pub const TICKER_BLOCK_INDEX: &'static str = "ticker_block_index";
    pub const TICKER_NOTE_BLOCK_INDEX: &'static str = "ticker_note_block_index";
}

impl TableSignature for WalletDbSaplingWitnessesTable {
    const TABLE_NAME: &'static str = "walletdb_sapling_witness";

    fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
        if let (0, 1) = (old_version, new_version) {
            let table = upgrader.create_table(Self::TABLE_NAME)?;
            table.create_multi_index(Self::TICKER_NOTE_BLOCK_INDEX, &["ticker", "note", "block"], true)?;
            table.create_multi_index(Self::TICKER_BLOCK_INDEX, &["ticker", "block"], false)?;
            table.create_index("ticker", false)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct WalletDbSentNotesTable {
    /// REFERENCES transactions(id_tx)
    pub tx: BigInt,
    pub output_index: BigInt,
    /// REFERENCES accounts(account)
    pub from_account: BigInt,
    pub address: String,
    pub value: BigInt,
    pub memo: Option<Vec<u8>>,
    pub ticker: String,
}

impl WalletDbSentNotesTable {
    /// A **unique** index that consists of the following properties:
    /// * ticker
    /// * tx
    /// * output_index
    pub const TICKER_TX_OUTPUT_INDEX: &'static str = "ticker_tx_output_index";
}

impl TableSignature for WalletDbSentNotesTable {
    const TABLE_NAME: &'static str = "walletdb_sent_notes";

    fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
        if let (0, 1) = (old_version, new_version) {
            let table = upgrader.create_table(Self::TABLE_NAME)?;
            table.create_multi_index(Self::TICKER_TX_OUTPUT_INDEX, &["ticker", "tx", "output_index"], false)?;
            table.create_index("ticker", false)?;
        }
        Ok(())
    }
}
