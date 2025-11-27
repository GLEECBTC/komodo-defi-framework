use crate::tx_history_storage::wasm::tx_history_db::TxHistoryDb;
use crate::tx_history_storage::wasm::WasmTxHistoryResult;
use crate::TransactionDetails;
use mm2_db::indexed_db::{DbIdentifier, DbInstance, DbUpgrader, OnUpgradeResult, TableSignature};
use mm2_err_handle::prelude::MmResultExt;

pub async fn load_tx_history(
    db: &TxHistoryDb,
    ticker: &str,
    wallet_address: &str,
) -> WasmTxHistoryResult<Vec<TransactionDetails>> {
    let history_id = HistoryId::new(ticker, wallet_address);

    let transaction = db.get_inner().transaction().await.map_mm_err()?;
    let table = transaction.table::<TxHistoryTableV1>().await.map_mm_err()?;

    let item_opt = table
        .get_item_by_unique_index("history_id", history_id.as_str())
        .await
        .map_mm_err()?;
    match item_opt {
        Some((_item_id, TxHistoryTableV1 { txs, .. })) => Ok(txs),
        None => Ok(Vec::new()),
    }
}

pub async fn save_tx_history(
    db: &TxHistoryDb,
    ticker: &str,
    wallet_address: &str,
    txs: Vec<TransactionDetails>,
) -> WasmTxHistoryResult<()> {
    let history_id = HistoryId::new(ticker, wallet_address);
    let history_id_value = history_id.to_string();
    let tx_history_item = TxHistoryTableV1 { history_id, txs };

    let transaction = db.get_inner().transaction().await.map_mm_err()?;
    let table = transaction.table::<TxHistoryTableV1>().await.map_mm_err()?;

    table
        .replace_item_by_unique_index("history_id", &history_id_value, &tx_history_item)
        .await
        .map_mm_err()?;
    Ok(())
}

pub async fn clear_tx_history(db: &TxHistoryDb, ticker: &str, wallet_address: &str) -> WasmTxHistoryResult<()> {
    let history_id = HistoryId::new(ticker, wallet_address);

    let transaction = db.get_inner().transaction().await.map_mm_err()?;
    let table = transaction.table::<TxHistoryTableV1>().await.map_mm_err()?;

    table
        .delete_item_by_unique_index("history_id", history_id.as_str())
        .await
        .map_mm_err()?;
    Ok(())
}

#[derive(Debug, Deserialize, Serialize)]
struct HistoryId(String);

impl HistoryId {
    fn new(ticker: &str, wallet_address: &str) -> HistoryId {
        HistoryId(format!("{ticker}_{wallet_address}"))
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for HistoryId {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", &self.0)
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct TxHistoryTableV1 {
    history_id: HistoryId,
    txs: Vec<TransactionDetails>,
}

impl TableSignature for TxHistoryTableV1 {
    const TABLE_NAME: &'static str = "tx_history";

    fn on_upgrade_needed(upgrader: &DbUpgrader, old_version: u32, new_version: u32) -> OnUpgradeResult<()> {
        if let (0, 1) = (old_version, new_version) {
            let table = upgrader.create_table(Self::TABLE_NAME)?;
            table.create_index("history_id", true)?;
        }

        Ok(())
    }
}

mod tests {
    use super::*;
    use serde_json as json;
    use wasm_bindgen_test::*;

    wasm_bindgen_test_configure!(run_in_browser);

    #[wasm_bindgen_test]
    async fn test_tx_history() {
        const DB_NAME: &str = "TEST_TX_HISTORY";
        let db = TxHistoryDb::init(DbIdentifier::for_test(DB_NAME))
            .await
            .expect("!TxHistoryDb::init_with_fs_path");

        let history = load_tx_history(&db, "RICK", "RRnMcSeKiLrNdbp91qNVQwwXx5azD4S4CD")
            .await
            .expect("!TxHistoryDb::load_history");
        assert!(history.is_empty());

        let history_str = r#"[{"tx_hex":"0400008085202f89018ec8f6f02e008ebd57bbf94c0d8297c1825f3af204490c43f5652b002a2c8b17010000006b483045022100e625a8b77beac5ec891e7d44b18fc4d780ef8456847eb2c6fa2f765e3379a9c102200f323612189fa44ee16f5deb39809b71c4811545b36ee4d6cc622d01aab10ef3012102043663e9c5af8275771809b3889d437f559e49e8df79b6ba19ade4cc5d8eb3e0ffffffff03809698000000000017a9142ffaf6694c6b441790546eefd277e430e08e47a6870000000000000000166a14094ab490fffa9939544545a656b345bf21920a90f6b35714000000001976a9145ed376ce9faa63cb2fef5862e1a5cc811c17316588acf9d29260000000000000000000000000000000","tx_hash":"f05d786bd4b647a5720094bf0a2c6f23b5e131c451d750a96102898f7b5458e8","from":["RHvavL8j683JwrN2ygk9Bg495DvPu5QVN3"],"to":["RHvavL8j683JwrN2ygk9Bg495DvPu5QVN3","bH6y6RtvbLToqSUNtLA5rQRjSwyNNzUSNc"],"total_amount":"3.51293022","spent_by_me":"3.51293022","received_by_me":"3.41292022","my_balance_change":"-0.10001","block_height":916940,"timestamp":1620235027,"fee_details":{"type":"Utxo","amount":"0.00001"},"coin":"RICK","internal_id":"f05d786bd4b647a5720094bf0a2c6f23b5e131c451d750a96102898f7b5458e8"}]"#;
        let history: Vec<TransactionDetails> = json::from_str(history_str).unwrap();
        save_tx_history(&db, "RICK", "RRnMcSeKiLrNdbp91qNVQwwXx5azD4S4CD", history)
            .await
            .expect("!TxHistoryDb::save_history");

        let updated_history_str = r#"[{"tx_hex":"0400008085202f89018ec8f6f02e008ebd57bbf94c0d8297c1825f3af204490c43f5652b002a2c8b17010000006b483045022100e625a8b77beac5ec891e7d44b18fc4d780ef8456847eb2c6fa2f765e3379a9c102200f323612189fa44ee16f5deb39809b71c4811545b36ee4d6cc622d01aab10ef3012102043663e9c5af8275771809b3889d437f559e49e8df79b6ba19ade4cc5d8eb3e0ffffffff03809698000000000017a9142ffaf6694c6b441790546eefd277e430e08e47a6870000000000000000166a14094ab490fffa9939544545a656b345bf21920a90f6b35714000000001976a9145ed376ce9faa63cb2fef5862e1a5cc811c17316588acf9d29260000000000000000000000000000000","tx_hash":"f05d786bd4b647a5720094bf0a2c6f23b5e131c451d750a96102898f7b5458e8","from":["RHvavL8j683JwrN2ygk9Bg495DvPu5QVN3"],"to":["RHvavL8j683JwrN2ygk9Bg495DvPu5QVN3","bH6y6RtvbLToqSUNtLA5rQRjSwyNNzUSNc"],"total_amount":"3.51293022","spent_by_me":"3.51293022","received_by_me":"3.41292022","my_balance_change":"-0.10001","block_height":916940,"timestamp":1620235027,"fee_details":{"type":"Utxo","amount":"0.00001"},"coin":"RICK","internal_id":"f05d786bd4b647a5720094bf0a2c6f23b5e131c451d750a96102898f7b5458e8"},{"tx_hex":"0400008085202f8901a5620f30001e5e31bcbffacee7687fd84490fa1f8625ddd1e098f0bc530d673e020000006b483045022100a9864707855307681b81d94ae17328f6feccb2a9439d27378dfeae2df0220cc102207476f8304af14794a9cd2fe6287e07a1c802c05ec134b789ddb22ab51f7d2238012102043663e9c5af8275771809b3889d437f559e49e8df79b6ba19ade4cc5d8eb3e0ffffffff0246320000000000001976a914ca1e04745e8ca0c60d8c5881531d51bec470743f88ac5e4ef014000000001976a9145ed376ce9faa63cb2fef5862e1a5cc811c17316588acacd29260000000000000000000000000000000","tx_hash":"178b2c2a002b65f5430c4904f23a5f82c197820d4cf9bb57bd8e002ef0f6c88e","from":["RHvavL8j683JwrN2ygk9Bg495DvPu5QVN3"],"to":["RHvavL8j683JwrN2ygk9Bg495DvPu5QVN3","RThtXup6Zo7LZAi8kRWgjAyi1s4u6U9Cpf"],"total_amount":"3.51306892","spent_by_me":"3.51306892","received_by_me":"3.51293022","my_balance_change":"-0.0001387","block_height":916939,"timestamp":1620234997,"fee_details":{"type":"Utxo","amount":"0.00001"},"coin":"RICK","internal_id":"178b2c2a002b65f5430c4904f23a5f82c197820d4cf9bb57bd8e002ef0f6c88e"}]"#;
        let updated_history: Vec<TransactionDetails> = json::from_str(updated_history_str).unwrap();
        save_tx_history(
            &db,
            "RICK",
            "RRnMcSeKiLrNdbp91qNVQwwXx5azD4S4CD",
            updated_history.clone(),
        )
        .await
        .expect("!TxHistoryDb::save_history");

        let actual_history = load_tx_history(&db, "RICK", "RRnMcSeKiLrNdbp91qNVQwwXx5azD4S4CD")
            .await
            .expect("!TxHistoryDb::load_history");
        assert_eq!(actual_history, updated_history);

        clear_tx_history(&db, "RICK", "RRnMcSeKiLrNdbp91qNVQwwXx5azD4S4CD")
            .await
            .expect("!TxHistoryDb::clear");

        let history = load_tx_history(&db, "RICK", "RRnMcSeKiLrNdbp91qNVQwwXx5azD4S4CD")
            .await
            .expect("!TxHistoryDb::load_history");
        assert!(history.is_empty());
    }
}
