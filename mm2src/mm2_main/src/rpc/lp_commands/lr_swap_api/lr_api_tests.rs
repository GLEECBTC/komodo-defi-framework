use super::lr_api_types::{AsksForCoin, LrFindBestQuoteRequest};
use crate::lp_ordermatch::{OrderbookAddress, RpcOrderbookEntryV2};
use crate::rpc::lp_commands::legacy::electrum;
use coins::eth::EthCoin;
use coins_activation::platform_for_tests::init_platform_coin_with_tokens_loop;
use crypto::CryptoCtx;
use mm2_number::{MmNumber, MmNumberMultiRepr};
use mm2_test_helpers::for_tests::{btc_with_spv_conf, mm_ctx_with_custom_db_with_conf, ETH_MAINNET_NODES};
use std::str::FromStr;
use uuid::Uuid;

/// Test to find best swap with LR.
/// checks how to find an order from an utxo/token ask order list, which is the most price efficient if route from my token into the token in the order.
/// With this test use --features test-ext-api and set ONE_INCH_API_TEST_AUTH env to the 1inch dev auth key
/// TODO: make it mockable to run within CI
#[tokio::test]
async fn test_find_best_lr_swap_for_order_list() {
    let _ = env_logger::try_init(); // to print log::info! log::debug! etc messages from the impl code (also use RUST_LOG)

    let platform_coin = "ETH".to_owned();
    let base_conf = btc_with_spv_conf();
    let platform_coin_conf = json!({
        "coin": platform_coin.clone(),
        "name": "ethereum",
        "derivation_path": "m/44'/1'",
        "protocol": {
            "type": "ETH",
            "protocol_data": {
                "chain_id": 1
            }
        }
    });

    // WETH = 2696.90 USD
    let weth_conf = json!({
        "coin": "WETH-ERC20",
        "name": "WETH-ERC20",
        "derivation_path": "m/44'/1'",
        "decimals": 18,
        "protocol": {
            "type": "ERC20",
            "protocol_data": {
                "platform": "ETH",
                "contract_address": "0xC02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"
            }
        }
    });

    // BNB = 612.36 USD
    let bnb_conf = json!({
        "coin": "BNB-ERC20",
        "name": "BNB token",
        "derivation_path": "m/44'/1'",
        "decimals": 18,
        "protocol": {
            "type": "ERC20",
            "protocol_data": {
                "platform": "ETH",
                "contract_address": "0xB8c77482e45F1F44dE1745F52C74426C631bDD52"
            }
        }
    });
    // AAVE 258.75 USD
    let aave_conf = json!({
        "coin": "AAVE-ERC20",
        "name": "AAVE token",
        "derivation_path": "m/44'/1'",
        "decimals": 18,
        "protocol": {
            "type": "ERC20",
            "protocol_data": {
                "platform": "ETH",
                "contract_address": "0x7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DDaE9"
            }
        }
    });
    // CNC 0.136968 USD USD
    let cnc_conf = json!({
        "coin": "CNC-ERC20",
        "name": "CNC token",
        "derivation_path": "m/44'/1'",
        "decimals": 18,
        "protocol": {
            "type": "ERC20",
            "protocol_data": {
                "platform": "ETH",
                "contract_address": "0x9aE380F0272E2162340a5bB646c354271c0F5cFC"
            }
        }
    });

    let base_ticker = base_conf["coin"].as_str().unwrap().to_owned();
    let weth_ticker = weth_conf["coin"].as_str().unwrap().to_owned();
    let bnb_ticker = bnb_conf["coin"].as_str().unwrap().to_owned();
    let aave_ticker = aave_conf["coin"].as_str().unwrap().to_owned();
    let cnc_ticker = cnc_conf["coin"].as_str().unwrap().to_owned();

    let conf = json!({
        "coins": [base_conf, platform_coin_conf, weth_conf, bnb_conf, aave_conf, cnc_conf],
        "1inch_api": "https://api.1inch.dev"
    });
    let ctx = mm_ctx_with_custom_db_with_conf(Some(conf));
    CryptoCtx::init_with_iguana_passphrase(ctx.clone(), "123").unwrap();

    electrum(
        ctx.clone(),
        json!({
            "coin": base_ticker,
            "mm2": 1,
            "method": "electrum",
            "servers": [
                {"url": "electrum1.cipig.net:10001"},
                {"url": "electrum2.cipig.net:10001"},
                {"url": "electrum3.cipig.net:10001"}
            ],
            "tx_history": false
        }),
    )
    .await
    .unwrap();
    init_platform_coin_with_tokens_loop::<EthCoin>(
        ctx.clone(),
        serde_json::from_value(json!({
            "ticker": platform_coin.clone(),
            "rpc_mode": "Default",
            "nodes": ETH_MAINNET_NODES.iter().map(|u| json!({"url": u})).collect::<Vec<_>>(),
            "swap_contract_address": "0xeA6D65434A15377081495a9E7C5893543E7c32cB",
            "erc20_tokens_requests": [
                {"ticker": weth_ticker.clone()},
                {"ticker": bnb_ticker.clone()},
                {"ticker": aave_ticker.clone()},
                {"ticker": cnc_ticker.clone()}
            ],
            "priv_key_policy": { "type": "ContextPrivKey" }
        }))
        .unwrap(),
    )
    .await
    .unwrap();

    let asks = vec![AsksForCoin {
        base: base_ticker.clone(),
        orders: vec![
            RpcOrderbookEntryV2 {
                coin: bnb_ticker,
                address: OrderbookAddress::Transparent("RLL6n4ayAv1haokcEd1QUEYniyeoiYkn7W".into()),
                price: MmNumberMultiRepr::from(MmNumber::from("145.69")),
                pubkey: "02f3578fbc0fc76056eae34180a71e9190ee08ad05d40947aab7a286666e2ce798".to_owned(),
                uuid: Uuid::from_str("7f26dc6a-39ab-4685-b5f1-55f12268ea50").unwrap(),
                is_mine: false,
                base_max_volume: MmNumberMultiRepr::from(MmNumber::from("1")),
                base_min_volume: MmNumberMultiRepr::from(MmNumber::from("0.1")),
                rel_max_volume: MmNumberMultiRepr::from(MmNumber::from("145.69")),
                rel_min_volume: MmNumberMultiRepr::from(MmNumber::from("14.569")),
                conf_settings: Default::default(),
            },
            RpcOrderbookEntryV2 {
                coin: aave_ticker,
                address: OrderbookAddress::Transparent("RK1JDwZ1LvH47Tvqm6pQM7aSqC2Zo6JwRF".into()),
                price: MmNumberMultiRepr::from(MmNumber::from("370.334")),
                pubkey: "02470bfb8e7710be4a7c2b8e9ba4bcfc5362a71643e64fc2e33b0d64c844ee9123".to_owned(),
                uuid: Uuid::from_str("2aadf450-6a8e-4e4e-8b89-19ca10f23cc3").unwrap(),
                is_mine: false,
                base_max_volume: MmNumberMultiRepr::from(MmNumber::from("1")),
                base_min_volume: MmNumberMultiRepr::from(MmNumber::from("0.1")),
                rel_max_volume: MmNumberMultiRepr::from(MmNumber::from("370.334")),
                rel_min_volume: MmNumberMultiRepr::from(MmNumber::from("37.0334")),
                conf_settings: Default::default(),
            },
            RpcOrderbookEntryV2 {
                coin: cnc_ticker,
                address: OrderbookAddress::Transparent("RK1JDwZ1LvH47Tvqm6pQM7aSqC2Zo6JwRF".into()),
                price: MmNumberMultiRepr::from(MmNumber::from("699300.69")),
                pubkey: "03de96cb66dcfaceaa8b3d4993ce8914cd5fe84e3fd53cefdae45add8032792a12".to_owned(),
                uuid: Uuid::from_str("89ab019f-b2fe-4d89-9764-96ac4a3fbf8e").unwrap(),
                is_mine: false,
                base_max_volume: MmNumberMultiRepr::from(MmNumber::from("1")),
                base_min_volume: MmNumberMultiRepr::from(MmNumber::from("0.1")),
                rel_max_volume: MmNumberMultiRepr::from(MmNumber::from("699300.69")),
                rel_min_volume: MmNumberMultiRepr::from(MmNumber::from("69930.069")),
                conf_settings: Default::default(),
            },
        ],
    }];
    let bids = vec![];

    let req = LrFindBestQuoteRequest {
        asks,
        user_base: base_ticker,
        bids,
        volume: "0.123".into(),
        method: "find_best_quote".into(),
        user_rel: weth_ticker,
    };

    let response = super::lr_find_best_quote_rpc(ctx, req).await;
    log!("response={:?}", response);
    assert!(response.is_ok(), "response={:?}", response);

    // BTC / WETH price around 35.0
    log!("response total_price={}", response.unwrap().total_price.to_decimal());
}
