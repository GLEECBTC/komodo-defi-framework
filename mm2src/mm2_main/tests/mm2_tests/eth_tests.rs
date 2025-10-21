use common::block_on;
use ethcore_transaction::UnverifiedTransactionWrapper;
use http::StatusCode;
use mm2_test_helpers::for_tests::{
    eth_sepolia_conf, get_passphrase, MarketMakerIt, Mm2TestConf, ETH_SEPOLIA_NODES, ETH_SEPOLIA_SWAP_CONTRACT,
};
use serde_json::{json, Value as Json};
use std::str::FromStr;

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_sign_eth_transaction() {
    let passphrase = get_passphrase(&".env.client", "BOB_PASSPHRASE").unwrap();
    let coins = json!([eth_sepolia_conf()]);
    let conf = Mm2TestConf::seednode(&passphrase, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();
    block_on(enable_eth(&mm, "ETH", ETH_SEPOLIA_NODES));
    let signed_tx = block_on(call_sign_eth_transaction(
        &mm,
        "ETH",
        "0x7Bc1bBDD6A0a722fC9bffC49c921B685ECB84b94",
        "1.234",
        "21000",
        None,
        Some("ABCD1234"),
    ));
    assert!(signed_tx["result"]["tx_hex"].is_string());
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_sign_eth_transaction_eip1559() {
    let passphrase = get_passphrase(&".env.client", "BOB_PASSPHRASE").unwrap();
    let coins = json!([eth_sepolia_conf()]);
    let conf = Mm2TestConf::seednode(&passphrase, &coins);
    let mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();
    block_on(enable_eth(&mm, "ETH", ETH_SEPOLIA_NODES));
    let signed_tx = block_on(call_sign_eth_transaction(
        &mm,
        "ETH",
        "0x7Bc1bBDD6A0a722fC9bffC49c921B685ECB84b94",
        "1.234",
        "21000",
        Some(json!({
            "tx_type": "Eip1559",
            "max_fee_per_gas": "1234.567", // in gwei
            "max_priority_fee_per_gas": "1.2", // in gwei
        })),
        Some("ABCD1234"),
    ));
    let bytes: Vec<u8> = ::rustc_hex::FromHex::from_hex(
        signed_tx["result"]["tx_hex"]
            .as_str()
            .expect("returned signed tx in hex"),
    )
    .unwrap();
    let tx: UnverifiedTransactionWrapper = rlp::decode(&bytes).expect("decoding signed tx okay");
    if !matches!(tx, UnverifiedTransactionWrapper::Eip1559(..)) {
        panic!("expected eip1559 tx");
    }
}

#[cfg(not(target_arch = "wasm32"))]
async fn enable_eth(mm: &MarketMakerIt, platform_coin: &str, nodes: &[&str]) -> Json {
    let enable = mm
        .rpc(&json!({
        "userpass": mm.userpass,
        "method": "enable",
        "coin": platform_coin,
        "urls": nodes,
        "swap_contract_address": ETH_SEPOLIA_SWAP_CONTRACT,
        "mm2": 1,
        }))
        .await
        .unwrap();
    assert_eq!(
        enable.0,
        StatusCode::OK,
        "'enable {platform_coin:?}' failed: {}",
        enable.1
    );
    Json::from_str(&enable.1).unwrap()
}

/// helper to call sign_raw_transaction rpc
/// params: marketmaker, coin, value in eth, gas_limit, optional contract data in hex
#[cfg(not(target_arch = "wasm32"))]
async fn call_sign_eth_transaction(
    mm: &MarketMakerIt,
    platform_coin: &str,
    to: &str,
    value: &str,
    gas_limit: &str,
    pay_for_gas: Option<Json>,
    data: Option<&str>,
) -> Json {
    let signed_tx = mm
        .rpc(&json!({
        "userpass": mm.userpass,
        "method": "sign_raw_transaction",
        "mmrpc": "2.0",
        "params": {
                "coin": platform_coin,
                "type": "ETH",
                "tx": {
                    "to": to,
                    "value": value,
                    "gas_limit": gas_limit,
                    "pay_for_gas": pay_for_gas,
                    "data": data
                }
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        signed_tx.0,
        StatusCode::OK,
        "'sign_raw_transaction' failed: {}",
        signed_tx.1
    );
    Json::from_str(&signed_tx.1).unwrap()
}
