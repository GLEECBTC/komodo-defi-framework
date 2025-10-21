use super::*;
use crate::lp_coininit;
use crypto::CryptoCtx;
use mm2_core::mm_ctx::MmCtxBuilder;
use mm2_test_helpers::for_tests::{ETH_SEPOLIA_CHAIN_ID, ETH_SEPOLIA_NODES, ETH_SEPOLIA_SWAP_CONTRACT};
use wasm_bindgen_test::*;
use web_sys::console;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn pass() {
    let ctx = MmCtxBuilder::default().into_mm_arc();
    let _coins_context = CoinsContext::from_ctx(&ctx).unwrap();
}

async fn init_eth_coin_helper() -> Result<(MmArc, MmCoinEnum), String> {
    let conf = json!({
        "coins": [{
            "coin": "ETH",
            "name": "ethereum",
            "fname": "Ethereum",
            "protocol":{
                "type": "ETH",
                "protocol_data": {
                    "chain_id": ETH_SEPOLIA_CHAIN_ID,
                }
            },
            "rpcport": 80,
            "mm2": 1,
            "max_eth_tx_type": 2
        }]
    });

    let ctx = MmCtxBuilder::new().with_conf(conf).into_mm_arc();
    CryptoCtx::init_with_iguana_passphrase(
        ctx.clone(),
        "spice describe gravity federal blast come thank unfair canal monkey style afraid",
    )
    .unwrap();

    let req = json!({
        "urls":ETH_SEPOLIA_NODES,
        "swap_contract_address":ETH_SEPOLIA_SWAP_CONTRACT,
    });
    Ok((ctx.clone(), lp_coininit(&ctx, "ETH", &req).await?))
}

#[wasm_bindgen_test]
async fn test_init_eth_coin() {
    let (_ctx, _coin) = init_eth_coin_helper().await.unwrap();
}

#[wasm_bindgen_test]
async fn wasm_test_sign_eth_tx() {
    // we need to hold ref to _ctx until the end of the test (because of the weak ref to MmCtx in EthCoinImpl)
    let (_ctx, coin) = init_eth_coin_helper().await.unwrap();
    let sign_req = json::from_value(json!({
        "coin": "ETH",
        "type": "ETH",
        "tx": {
            "to": "0x7Bc1bBDD6A0a722fC9bffC49c921B685ECB84b94".to_string(),
            "value": "1.234",
            "gas_limit": "21000"
        }
    }))
    .unwrap();
    let res = coin.sign_raw_tx(&sign_req).await;
    console::log_1(&format!("res={res:?}").into());
    assert!(res.is_ok());
}

#[wasm_bindgen_test]
async fn wasm_test_sign_eth_tx_with_priority_fee() {
    // we need to hold ref to _ctx until the end of the test (because of the weak ref to MmCtx in EthCoinImpl)
    let (_ctx, coin) = init_eth_coin_helper().await.unwrap();
    let sign_req = json::from_value(json!({
        "coin": "ETH",
        "type": "ETH",
        "tx": {
            "to": "0x7Bc1bBDD6A0a722fC9bffC49c921B685ECB84b94".to_string(),
            "value": "1.234",
            "gas_limit": "21000",
            "pay_for_gas": {
                "tx_type": "Eip1559",
                "max_fee_per_gas": "1234.567",
                "max_priority_fee_per_gas": "1.2",
            }
        }
    }))
    .unwrap();
    let res = coin.sign_raw_tx(&sign_req).await;
    console::log_1(&format!("res={res:?}").into());
    assert!(res.is_ok());
    let tx: UnverifiedTransactionWrapper = rlp::decode(&res.unwrap().tx_hex).expect("decoding signed tx okay");
    if !matches!(tx, UnverifiedTransactionWrapper::Eip1559(..)) {
        panic!("expected eip1559 tx");
    }
}
