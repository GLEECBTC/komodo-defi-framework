use super::rpc_clients::{ElectrumClient, ElectrumConnectionSettings, UtxoRpcClientOps};
use super::utxo_builder::{UtxoArcBuilder, UtxoCoinBuilderCommonOps};
use super::utxo_standard::UtxoStandardCoin;
use super::*;
use crate::utxo::utxo_common_tests;
use crate::{IguanaPrivKey, PrivKeyBuildPolicy};
use hex::FromHex;
use mm2_core::mm_ctx::MmCtxBuilder;
use mm2_test_helpers::for_tests::DOC_ELECTRUM_ADDRS;
use serialization::{deserialize, ChainVariant};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

const TEST_COIN_NAME: &str = "DOC";

pub async fn electrum_client_for_test(servers: &[&str]) -> ElectrumClient {
    let ctx = MmCtxBuilder::default().into_mm_arc();
    let servers: Vec<_> = servers
        .iter()
        .map(|server| json!({ "url": server, "protocol": "WSS" }))
        .collect();
    let req = json!({
        "method": "electrum",
        "servers": servers,
    });
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let priv_key_policy = PrivKeyBuildPolicy::IguanaPrivKey(IguanaPrivKey::default());
    let builder = UtxoArcBuilder::new(
        &ctx,
        TEST_COIN_NAME,
        &Json::Null,
        &params,
        priv_key_policy,
        UtxoStandardCoin::from,
    );
    let args = ElectrumBuilderArgs {
        spawn_ping: false,
        negotiate_version: true,
        collect_metrics: false,
    };

    let servers: Vec<ElectrumConnectionSettings> = servers.into_iter().map(|s| json::from_value(s).unwrap()).collect();
    let abortable_system = AbortableQueue::default();
    builder
        .electrum_client(abortable_system, args, ChainVariant::Standard, servers, (None, None))
        .await
        .unwrap()
}

#[wasm_bindgen_test]
async fn test_electrum_rpc_client() {
    let client = electrum_client_for_test(DOC_ELECTRUM_ADDRS).await;

    let tx_hash: H256Json = <[u8; 32]>::from_hex("a3ebedbe20f82e43708f276152cf7dfb03a6050921c8f266e48c00ab66e891fb")
        .unwrap()
        .into();
    let verbose_tx = client
        .get_verbose_transaction(&tx_hash)
        .compat()
        .await
        .expect("!get_verbose_transaction");
    let actual: UtxoTx = deserialize(verbose_tx.hex.as_slice()).unwrap();
    let expected = UtxoTx::from("0400008085202f8901e15182af2c252bcfbd58884f3bdbd4d85ed036e53cfe2fd1f904ecfea10cb9f2010000006b483045022100d2435e0c9211114271ac452dc47fd08d3d2dc4bdd484d5750ee6bbda41056d520220408bfb236b7028b6fde0e59a1b6522949131a611584cce36c3df1e934c1748630121022d7424c741213a2b9b49aebdaa10e84419e642a8db0a09e359a3d4c850834846ffffffff02a09ba104000000001976a914054407d1a2224268037cfc7ca3bc438d082bedf488acdd28ce9157ba11001976a914046922483fab8ca76b23e55e9d338605e2dbab6088ac03d63665000000000000000000000000000000");
    assert_eq!(actual, expected);
}

#[wasm_bindgen_test]
async fn test_electrum_display_balances() {
    let rpc_client = electrum_client_for_test(DOC_ELECTRUM_ADDRS).await;
    utxo_common_tests::test_electrum_display_balances(&rpc_client).await;
}

#[wasm_bindgen_test]
async fn test_hd_utxo_tx_history() {
    let rpc_client = electrum_client_for_test(DOC_ELECTRUM_ADDRS).await;
    utxo_common_tests::test_hd_utxo_tx_history_impl(rpc_client).await;
}
