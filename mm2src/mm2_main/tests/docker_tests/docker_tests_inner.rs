use crate::docker_tests::helpers::env::{random_secp256k1_secret, MM_CTX};
use crate::docker_tests::helpers::eth::{
    erc20_coin_with_random_privkey, erc20_contract_checksum, fill_eth_erc20_with_private_key, swap_contract,
    swap_contract_checksum, GETH_RPC_URL,
};
use crate::docker_tests::helpers::swap::trade_base_rel;
use crate::docker_tests::helpers::utxo::{
    fill_address, generate_utxo_coin_with_privkey, generate_utxo_coin_with_random_privkey, rmd160_from_priv,
};
use crate::integration_tests_common::*;
use coins::TxFeeDetails;
use coins::{ConfirmPaymentInput, MarketCoinOps, MmCoin, WithdrawRequest};
use common::{block_on, block_on_f01, executor::Timer, get_utc_timestamp, wait_until_sec};
use crypto::privkey::key_pair_from_seed;
use crypto::{CryptoCtx, DerivationPath, KeyPairPolicy};
use http::StatusCode;
use mm2_libp2p::behaviours::atomicdex::MAX_TIME_GAP_FOR_CONNECTED_PEER;
use mm2_number::{BigDecimal, BigRational};
use mm2_test_helpers::for_tests::{
    check_my_swap_status_amounts, disable_coin, disable_coin_err, enable_eth_coin, erc20_dev_conf, eth_dev_conf,
    mm_dump, mycoin1_conf, mycoin_conf, start_swaps, task_enable_eth_with_tokens, wait_for_swap_contract_negotiation,
    wait_for_swap_negotiation_failure, MarketMakerIt, Mm2TestConf, DEFAULT_RPC_PASSWORD,
};
use mm2_test_helpers::{get_passphrase, structs::*};
use serde_json::Value as Json;
use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::env;
use std::iter::FromIterator;
use std::str::FromStr;
use std::thread;
use std::time::Duration;

// =============================================================================
// Test address constants
// =============================================================================

/// Arbitrary address used for swap contract negotiation tests (maker side)
const TEST_ARBITRARY_SWAP_ADDR_1: &str = "0x6c2858f6afac835c43ffda248aea167e1a58436c";
/// Arbitrary address used for swap contract negotiation tests (taker side)
const TEST_ARBITRARY_SWAP_ADDR_2: &str = "0x24abe4c71fc658c01313b6552cd40cd808b3ea80";
/// Valid checksummed ETH address used as withdraw destination in tests
const TEST_WITHDRAW_DEST_ADDR: &str = "0x4b2d0d6c2c785217457B69B922A2A9cEA98f71E9";
/// Invalid checksum variant of the withdraw destination (for checksum validation tests)
const TEST_WITHDRAW_DEST_ADDR_INVALID_CHECKSUM: &str = "0x4b2d0d6c2c785217457b69b922a2A9cEA98f71E9";

#[test]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/554
fn order_should_be_cancelled_when_entire_balance_is_withdrawn() {
    let (_ctx, _, priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);

    let mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "myipaddr": env::var("BOB_TRADE_IP") .ok(),
            "rpcip": env::var("BOB_TRADE_IP") .ok(),
            "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
            "passphrase": format!("0x{}", hex::encode(priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": "999",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    let bob_uuid = json["result"]["uuid"].as_str().unwrap().to_owned();

    log!("Get MYCOIN/MYCOIN1 orderbook");
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let bob_orderbook: Json = serde_json::from_str(&rc.1).unwrap();
    log!("orderbook {:?}", bob_orderbook);
    let asks = bob_orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 1, "MYCOIN/MYCOIN1 orderbook must have exactly 1 ask");

    let withdraw = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "withdraw",
        "coin": "MYCOIN",
        "max": true,
        "to": "R9imXLs1hEcU9KbFDQq2hJEEJ1P5UoekaF",
    })))
    .unwrap();
    assert!(withdraw.0.is_success(), "!withdraw: {}", withdraw.1);

    let withdraw: Json = serde_json::from_str(&withdraw.1).unwrap();

    let send_raw = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "send_raw_transaction",
        "coin": "MYCOIN",
        "tx_hex": withdraw["tx_hex"],
    })))
    .unwrap();
    assert!(send_raw.0.is_success(), "!send_raw: {}", send_raw.1);

    thread::sleep(Duration::from_secs(32));

    log!("Get MYCOIN/MYCOIN1 orderbook");
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let bob_orderbook: Json = serde_json::from_str(&rc.1).unwrap();
    log!("orderbook {}", serde_json::to_string(&bob_orderbook).unwrap());
    let asks = bob_orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 0, "MYCOIN/MYCOIN1 orderbook must have exactly 0 asks");

    log!("Get my orders");
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "my_orders",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!my_orders: {}", rc.1);
    let orders: Json = serde_json::from_str(&rc.1).unwrap();
    log!("my_orders {}", serde_json::to_string(&orders).unwrap());
    assert!(
        orders["result"]["maker_orders"].as_object().unwrap().is_empty(),
        "maker_orders must be empty"
    );

    let rmd160 = rmd160_from_priv(priv_key);
    let order_path = mm_bob.folder.join(format!(
        "DB/{}/ORDERS/MY/MAKER/{}.json",
        hex::encode(rmd160.take()),
        bob_uuid,
    ));
    log!("Order path {}", order_path.display());
    assert!(!order_path.exists());
    block_on(mm_bob.stop()).unwrap();
}

#[test]
fn order_should_be_updated_when_balance_is_decreased_alice_subscribes_after_update() {
    let (_ctx, _, priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);

    let mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "myipaddr": env::var("BOB_TRADE_IP") .ok(),
            "rpcip": env::var("BOB_TRADE_IP") .ok(),
            "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
            "passphrase": format!("0x{}", hex::encode(priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    let mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": "alice passphrase",
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": vec![format!("{}", mm_bob.ip)],
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));

    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": "999",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    log!("Get MYCOIN/MYCOIN1 orderbook");
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let bob_orderbook: Json = serde_json::from_str(&rc.1).unwrap();
    log!("orderbook {:?}", bob_orderbook);
    let asks = bob_orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 1, "MYCOIN/MYCOIN1 orderbook must have exactly 1 ask");

    let withdraw = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "withdraw",
        "coin": "MYCOIN",
        "amount": "499.99999481",
        "to": "R9imXLs1hEcU9KbFDQq2hJEEJ1P5UoekaF",
    })))
    .unwrap();
    assert!(withdraw.0.is_success(), "!withdraw: {}", withdraw.1);

    let withdraw: Json = serde_json::from_str(&withdraw.1).unwrap();

    let send_raw = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "send_raw_transaction",
        "coin": "MYCOIN",
        "tx_hex": withdraw["tx_hex"],
    })))
    .unwrap();
    assert!(send_raw.0.is_success(), "!send_raw: {}", send_raw.1);

    thread::sleep(Duration::from_secs(32));

    log!("Get MYCOIN/MYCOIN1 orderbook on Bob side");
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let bob_orderbook: Json = serde_json::from_str(&rc.1).unwrap();
    log!("orderbook {}", serde_json::to_string(&bob_orderbook).unwrap());
    let asks = bob_orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 1, "Bob MYCOIN/MYCOIN1 orderbook must have exactly 1 ask");

    let order_volume = asks[0]["maxvolume"].as_str().unwrap();
    assert_eq!("500", order_volume); // 1000.0 - (499.99999481 + 0.00000274 txfee) = (500.0 + 0.00000274 txfee)

    log!("Get MYCOIN/MYCOIN1 orderbook on Alice side");
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let alice_orderbook: Json = serde_json::from_str(&rc.1).unwrap();
    log!("orderbook {}", serde_json::to_string(&alice_orderbook).unwrap());
    let asks = alice_orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 1, "Alice MYCOIN/MYCOIN1 orderbook must have exactly 1 ask");

    let order_volume = asks[0]["maxvolume"].as_str().unwrap();
    assert_eq!("500", order_volume);

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn order_should_be_updated_when_balance_is_decreased_alice_subscribes_before_update() {
    let (_ctx, _, priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "myipaddr": env::var("BOB_TRADE_IP") .ok(),
            "rpcip": env::var("BOB_TRADE_IP") .ok(),
            "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
            "passphrase": format!("0x{}", hex::encode(priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    let mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": "alice passphrase",
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": vec![format!("{}", mm_bob.ip)],
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));

    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": "999",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    log!("Get MYCOIN/MYCOIN1 orderbook");
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let bob_orderbook: Json = serde_json::from_str(&rc.1).unwrap();
    log!("orderbook {:?}", bob_orderbook);
    let asks = bob_orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 1, "Bob MYCOIN/MYCOIN1 orderbook must have exactly 1 ask");

    thread::sleep(Duration::from_secs(2));
    log!("Get MYCOIN/MYCOIN1 orderbook on Alice side");
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let alice_orderbook: Json = serde_json::from_str(&rc.1).unwrap();
    log!("orderbook {}", serde_json::to_string(&alice_orderbook).unwrap());
    let asks = alice_orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 1, "Alice MYCOIN/MYCOIN1 orderbook must have exactly 1 ask");

    let withdraw = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "withdraw",
        "coin": "MYCOIN",
        "amount": "499.99999481",
        "to": "R9imXLs1hEcU9KbFDQq2hJEEJ1P5UoekaF",
    })))
    .unwrap();
    assert!(withdraw.0.is_success(), "!withdraw: {}", withdraw.1);

    let withdraw: Json = serde_json::from_str(&withdraw.1).unwrap();

    let send_raw = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "send_raw_transaction",
        "coin": "MYCOIN",
        "tx_hex": withdraw["tx_hex"],
    })))
    .unwrap();
    assert!(send_raw.0.is_success(), "!send_raw: {}", send_raw.1);

    thread::sleep(Duration::from_secs(32));

    log!("Get MYCOIN/MYCOIN1 orderbook on Bob side");
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let bob_orderbook: Json = serde_json::from_str(&rc.1).unwrap();
    log!("orderbook {}", serde_json::to_string(&bob_orderbook).unwrap());
    let asks = bob_orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 1, "Bob MYCOIN/MYCOIN1 orderbook must have exactly 1 ask");

    let order_volume = asks[0]["maxvolume"].as_str().unwrap();
    assert_eq!("500", order_volume); // 1000.0 - (499.99999481 + 0.00000245 txfee) = (500.0 + 0.00000274 txfee)

    log!("Get MYCOIN/MYCOIN1 orderbook on Alice side");
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let alice_orderbook: Json = serde_json::from_str(&rc.1).unwrap();
    log!("orderbook {}", serde_json::to_string(&alice_orderbook).unwrap());
    let asks = alice_orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 1, "Alice MYCOIN/MYCOIN1 orderbook must have exactly 1 ask");

    let order_volume = asks[0]["maxvolume"].as_str().unwrap();
    assert_eq!("500", order_volume);

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_order_should_be_updated_when_matched_partially() {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 2000.into());
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 2000.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mut mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(bob_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    let mut mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(alice_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": vec![format!("{}", mm_bob.ip)],
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));

    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": "1000",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": "500",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    block_on(mm_bob.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop MYCOIN/MYCOIN1"))).unwrap();

    log!("Get MYCOIN/MYCOIN1 orderbook on Bob side");
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let bob_orderbook: Json = serde_json::from_str(&rc.1).unwrap();
    log!("orderbook {}", serde_json::to_string(&bob_orderbook).unwrap());
    let asks = bob_orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 1, "Bob MYCOIN/MYCOIN1 orderbook must have exactly 1 ask");

    let order_volume = asks[0]["maxvolume"].as_str().unwrap();
    assert_eq!("500", order_volume);

    log!("Get MYCOIN/MYCOIN1 orderbook on Alice side");
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let alice_orderbook: Json = serde_json::from_str(&rc.1).unwrap();
    log!("orderbook {}", serde_json::to_string(&alice_orderbook).unwrap());
    let asks = alice_orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 1, "Alice MYCOIN/MYCOIN1 orderbook must have exactly 1 ask");

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_set_price_max() {
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(alice_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);

    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        // the result of equation x + 0.00001 = 1
        "volume": {
            "numer":"99999",
            "denom":"100000"
        },
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        // it is slightly more than previous volume so it should fail
        "volume": {
            "numer":"100000",
            "denom":"100000"
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "setprice success, but should fail: {}", rc.1);
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_maker_order_should_kick_start_and_appear_in_orderbook_on_restart() {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mut bob_conf = json!({
        "gui": "nogui",
        "netid": 9000,
        "dht": "on",  // Enable DHT without delay.
        "passphrase": format!("0x{}", hex::encode(bob_priv_key)),
        "coins": coins,
        "rpc_password": "pass",
        "i_am_seed": true,
        "is_bootstrap_node": true
    });
    let mm_bob = MarketMakerIt::start(bob_conf.clone(), "pass".to_string(), None).unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "max": true,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    // mm_bob using same DB dir that should kick start the order
    bob_conf["dbdir"] = mm_bob.folder.join("DB").to_str().unwrap().into();
    bob_conf["log"] = mm_bob.folder.join("mm2_dup.log").to_str().unwrap().into();
    block_on(mm_bob.stop()).unwrap();

    let mm_bob_dup = MarketMakerIt::start(bob_conf, "pass".to_string(), None).unwrap();
    let (_bob_dup_dump_log, _bob_dup_dump_dashboard) = mm_dump(&mm_bob_dup.log_path);
    log!("{:?}", block_on(enable_native(&mm_bob_dup, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob_dup, "MYCOIN1", &[], None)));

    thread::sleep(Duration::from_secs(2));

    log!("Get MYCOIN/MYCOIN1 orderbook on Bob side");
    let rc = block_on(mm_bob_dup.rpc(&json!({
        "userpass": mm_bob_dup.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let bob_orderbook: Json = serde_json::from_str(&rc.1).unwrap();
    log!("Bob orderbook {:?}", bob_orderbook);
    let asks = bob_orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 1, "Bob MYCOIN/MYCOIN1 orderbook must have exactly 1 asks");
}

#[test]
fn test_maker_order_should_not_kick_start_and_appear_in_orderbook_if_balance_is_withdrawn() {
    let (_ctx, coin, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mut bob_conf = json!({
        "gui": "nogui",
        "netid": 9000,
        "dht": "on",  // Enable DHT without delay.
        "passphrase": format!("0x{}", hex::encode(bob_priv_key)),
        "coins": coins,
        "rpc_password": "pass",
        "i_am_seed": true,
        "is_bootstrap_node": true
    });
    let mm_bob = MarketMakerIt::start(bob_conf.clone(), "pass".to_string(), None).unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "max": true,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    let res: SetPriceResponse = serde_json::from_str(&rc.1).unwrap();
    let uuid = res.result.uuid;

    // mm_bob using same DB dir that should kick start the order
    bob_conf["dbdir"] = mm_bob.folder.join("DB").to_str().unwrap().into();
    bob_conf["log"] = mm_bob.folder.join("mm2_dup.log").to_str().unwrap().into();
    block_on(mm_bob.stop()).unwrap();

    let withdraw = block_on_f01(coin.withdraw(WithdrawRequest::new_max(
        "MYCOIN".to_string(),
        "RRYmiZSDo3UdHHqj1rLKf8cbJroyv9NxXw".to_string(),
    )))
    .unwrap();
    block_on_f01(coin.send_raw_tx(&hex::encode(&withdraw.tx.tx_hex().unwrap().0))).unwrap();
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: withdraw.tx.tx_hex().unwrap().0.to_owned(),
        confirmations: 1,
        requires_nota: false,
        wait_until: wait_until_sec(10),
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let mm_bob_dup = MarketMakerIt::start(bob_conf, "pass".to_string(), None).unwrap();
    let (_bob_dup_dump_log, _bob_dup_dump_dashboard) = mm_dump(&mm_bob_dup.log_path);
    log!("{:?}", block_on(enable_native(&mm_bob_dup, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob_dup, "MYCOIN1", &[], None)));

    thread::sleep(Duration::from_secs(2));

    log!("Get MYCOIN/MYCOIN1 orderbook on Bob side");
    let rc = block_on(mm_bob_dup.rpc(&json!({
        "userpass": mm_bob_dup.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let bob_orderbook: Json = serde_json::from_str(&rc.1).unwrap();
    log!("Bob orderbook {:?}", bob_orderbook);
    let asks = bob_orderbook["asks"].as_array().unwrap();
    assert!(asks.is_empty(), "Bob MYCOIN/MYCOIN1 orderbook must not have asks");

    let rc = block_on(mm_bob_dup.rpc(&json!({
        "userpass": mm_bob_dup.userpass,
        "method": "my_orders",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!my_orders: {}", rc.1);

    let res: MyOrdersRpcResult = serde_json::from_str(&rc.1).unwrap();
    assert!(res.result.maker_orders.is_empty(), "Bob maker orders must be empty");

    let order_path = mm_bob.folder.join(format!(
        "DB/{}/ORDERS/MY/MAKER/{}.json",
        hex::encode(rmd160_from_priv(bob_priv_key).take()),
        uuid
    ));

    log!("Order path {}", order_path.display());
    assert!(!order_path.exists());
}

#[test]
fn test_maker_order_kick_start_should_trigger_subscription_and_match() {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 1000.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);

    let relay_conf = json!({
        "gui": "nogui",
        "netid": 9000,
        "dht": "on",  // Enable DHT without delay.
        "passphrase": "relay",
        "coins": coins,
        "rpc_password": "pass",
        "i_am_seed": true,
        "is_bootstrap_node": true
    });
    let relay = MarketMakerIt::start(relay_conf, "pass".to_string(), None).unwrap();
    let (_relay_dump_log, _relay_dump_dashboard) = mm_dump(&relay.log_path);

    let mut bob_conf = json!({
        "gui": "nogui",
        "netid": 9000,
        "dht": "on",  // Enable DHT without delay.
        "passphrase": format!("0x{}", hex::encode(bob_priv_key)),
        "coins": coins,
        "rpc_password": "pass",
        "seednodes": vec![format!("{}", relay.ip)],
    });
    let mm_bob = MarketMakerIt::start(bob_conf.clone(), "pass".to_string(), None).unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    let mut mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(alice_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": vec![format!("{}", relay.ip)],
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));

    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "max": true,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    // mm_bob using same DB dir that should kick start the order
    bob_conf["dbdir"] = mm_bob.folder.join("DB").to_str().unwrap().into();
    bob_conf["log"] = mm_bob.folder.join("mm2_dup.log").to_str().unwrap().into();
    block_on(mm_bob.stop()).unwrap();

    let mut mm_bob_dup = MarketMakerIt::start(bob_conf, "pass".to_string(), None).unwrap();
    let (_bob_dup_dump_log, _bob_dup_dump_dashboard) = mm_dump(&mm_bob_dup.log_path);
    log!("{:?}", block_on(enable_native(&mm_bob_dup, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob_dup, "MYCOIN1", &[], None)));

    log!("Give restarted Bob 2 seconds to kickstart the order");
    thread::sleep(Duration::from_secs(2));

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": 1,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    block_on(mm_bob_dup.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
}

#[test]
fn test_orders_should_match_on_both_nodes_with_same_priv() {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 2000.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(bob_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    let mut mm_alice_1 = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(alice_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": vec![format!("{}", mm_bob.ip)],
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_1_dump_log, _alice_1_dump_dashboard) = mm_dump(&mm_alice_1.log_path);

    let mut mm_alice_2 = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(alice_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": vec![format!("{}", mm_bob.ip)],
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_2_dump_log, _alice_2_dump_dashboard) = mm_dump(&mm_alice_2.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice_1, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice_1, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice_2, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice_2, "MYCOIN1", &[], None)));

    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "max": true,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let rc = block_on(mm_alice_1.rpc(&json!({
        "userpass": mm_alice_1.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": "1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    block_on(mm_alice_1.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop MYCOIN/MYCOIN1"))).unwrap();

    let rc = block_on(mm_alice_2.rpc(&json!({
        "userpass": mm_alice_2.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": "1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    block_on(mm_alice_2.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop MYCOIN/MYCOIN1"))).unwrap();

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice_1.stop()).unwrap();
    block_on(mm_alice_2.stop()).unwrap();
}

#[test]
fn test_maker_and_taker_order_created_with_same_priv_should_not_match() {
    let (_ctx, coin, priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let (_ctx, coin1, _) = generate_utxo_coin_with_random_privkey("MYCOIN1", 1000.into());
    fill_address(&coin1, &coin.my_address().unwrap(), 1000.into(), 30);
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mut mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    let mut mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": vec![format!("{}", mm_bob.ip)],
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_1_dump_log, _alice_1_dump_dashboard) = mm_dump(&mm_alice.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));

    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "max": true,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": "1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    block_on(mm_bob.wait_for_log(5., |log| log.contains("Entering the maker_swap_loop MYCOIN/MYCOIN1"))).unwrap_err();
    block_on(mm_alice.wait_for_log(5., |log| log.contains("Entering the taker_swap_loop MYCOIN/MYCOIN1"))).unwrap_err();

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_taker_order_converted_to_maker_should_cancel_properly_when_matched() {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 2000.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mut mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(bob_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    let mut mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(alice_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": vec![format!("{}", mm_bob.ip)],
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "sell",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": 1,
        "timeout": 2,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);

    log!("Give Bob 4 seconds to convert order to maker");
    block_on(Timer::sleep(4.));

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": 1,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    block_on(mm_bob.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop MYCOIN/MYCOIN1"))).unwrap();

    log!("Give Bob 2 seconds to cancel the order");
    thread::sleep(Duration::from_secs(2));
    log!("Get my_orders on Bob side");
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "my_orders",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!my_orders: {}", rc.1);
    let my_orders_json: Json = serde_json::from_str(&rc.1).unwrap();
    let maker_orders: HashMap<String, Json> =
        serde_json::from_value(my_orders_json["result"]["maker_orders"].clone()).unwrap();
    assert!(maker_orders.is_empty());

    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let bob_orderbook: Json = serde_json::from_str(&rc.1).unwrap();
    log!("Bob orderbook {:?}", bob_orderbook);
    let asks = bob_orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 0, "Bob MYCOIN/MYCOIN1 orderbook must be empty");

    log!("Get MYCOIN/MYCOIN1 orderbook on Alice side");
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let alice_orderbook: Json = serde_json::from_str(&rc.1).unwrap();
    log!("Alice orderbook {:?}", alice_orderbook);
    let asks = alice_orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 0, "Alice MYCOIN/MYCOIN1 orderbook must be empty");

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

// https://github.com/KomodoPlatform/atomicDEX-API/issues/1053
#[test]
fn test_taker_should_match_with_best_price_buy() {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 2000.into());
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 4000.into());
    let (_ctx, _, eve_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 2000.into());

    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(bob_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    let mut mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(alice_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": vec![format!("{}", mm_bob.ip)],
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);

    let mut mm_eve = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(eve_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": vec![format!("{}", mm_bob.ip)],
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_eve_dump_log, _eve_dump_dashboard) = mm_dump(&mm_alice.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_eve, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_eve, "MYCOIN1", &[], None)));

    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 2,
        "max": true,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let rc = block_on(mm_eve.rpc(&json!({
        "userpass": mm_eve.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "max": true,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    // subscribe alice to the orderbook topic to not miss eve's message
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!alice orderbook: {}", rc.1);
    log!("alice orderbook {}", rc.1);

    thread::sleep(Duration::from_secs(1));

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 3,
        "volume": "1000",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);
    let alice_buy: BuyOrSellRpcResult = serde_json::from_str(&rc.1).unwrap();

    block_on(mm_eve.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop MYCOIN/MYCOIN1"))).unwrap();

    thread::sleep(Duration::from_secs(2));

    block_on(check_my_swap_status_amounts(
        &mm_alice,
        alice_buy.result.uuid,
        1000.into(),
        1000.into(),
    ));
    block_on(check_my_swap_status_amounts(
        &mm_eve,
        alice_buy.result.uuid,
        1000.into(),
        1000.into(),
    ));

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
    block_on(mm_eve.stop()).unwrap();
}

// https://github.com/KomodoPlatform/atomicDEX-API/issues/1053
#[test]
fn test_taker_should_match_with_best_price_sell() {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 2000.into());
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 4000.into());
    let (_ctx, _, eve_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 2000.into());

    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(bob_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    let mut mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(alice_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": vec![format!("{}", mm_bob.ip)],
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);

    let mut mm_eve = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(eve_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": vec![format!("{}", mm_bob.ip)],
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_eve_dump_log, _eve_dump_dashboard) = mm_dump(&mm_alice.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_eve, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_eve, "MYCOIN1", &[], None)));

    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 2,
        "max": true,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let rc = block_on(mm_eve.rpc(&json!({
        "userpass": mm_eve.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "max": true,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    // subscribe alice to the orderbook topic to not miss eve's message
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!alice orderbook: {}", rc.1);
    log!("alice orderbook {}", rc.1);

    thread::sleep(Duration::from_secs(1));

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "sell",
        "base": "MYCOIN1",
        "rel": "MYCOIN",
        "price": "0.1",
        "volume": "1000",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);
    let alice_buy: BuyOrSellRpcResult = serde_json::from_str(&rc.1).unwrap();

    block_on(mm_eve.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop MYCOIN/MYCOIN1"))).unwrap();

    thread::sleep(Duration::from_secs(2));

    block_on(check_my_swap_status_amounts(
        &mm_alice,
        alice_buy.result.uuid,
        1000.into(),
        1000.into(),
    ));
    block_on(check_my_swap_status_amounts(
        &mm_eve,
        alice_buy.result.uuid,
        1000.into(),
        1000.into(),
    ));

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
    block_on(mm_eve.stop()).unwrap();
}

#[test]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/1074
fn test_match_utxo_with_eth_taker_sell() {
    let alice_passphrase = get_passphrase!(".env.client", "ALICE_PASSPHRASE").unwrap();
    let bob_passphrase = get_passphrase!(".env.seed", "BOB_PASSPHRASE").unwrap();
    let alice_priv_key = key_pair_from_seed(&alice_passphrase).unwrap().private().secret;
    let bob_priv_key = key_pair_from_seed(&bob_passphrase).unwrap().private().secret;

    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), alice_priv_key);
    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), bob_priv_key);

    let coins = json!([mycoin_conf(1000), eth_dev_conf()]);

    let mut mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(bob_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    let mut mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(alice_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": vec![format!("{}", mm_bob.ip)],
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    block_on(enable_native(&mm_bob, "ETH", &[GETH_RPC_URL], None));
    block_on(enable_native(&mm_alice, "ETH", &[GETH_RPC_URL], None));

    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "ETH",
        "price": 1,
        "volume": "0.0001",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "sell",
        "base": "ETH",
        "rel": "MYCOIN",
        "price": 1,
        "volume": "0.0001",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);

    block_on(mm_bob.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop MYCOIN/ETH"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop MYCOIN/ETH"))).unwrap();

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/1074
fn test_match_utxo_with_eth_taker_buy() {
    let alice_passphrase = get_passphrase!(".env.client", "ALICE_PASSPHRASE").unwrap();
    let bob_passphrase = get_passphrase!(".env.seed", "BOB_PASSPHRASE").unwrap();
    let alice_priv_key = key_pair_from_seed(&alice_passphrase).unwrap().private().secret;
    let bob_priv_key = key_pair_from_seed(&bob_passphrase).unwrap().private().secret;

    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), alice_priv_key);
    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), bob_priv_key);
    let coins = json!([mycoin_conf(1000), eth_dev_conf()]);
    let mut mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(bob_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    let mut mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(alice_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": vec![format!("{}", mm_bob.ip)],
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    block_on(enable_native(&mm_bob, "ETH", &[GETH_RPC_URL], None));

    block_on(enable_native(&mm_alice, "ETH", &[GETH_RPC_URL], None));

    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "ETH",
        "price": 1,
        "volume": "0.0001",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "ETH",
        "price": 1,
        "volume": "0.0001",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    block_on(mm_bob.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop MYCOIN/ETH"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop MYCOIN/ETH"))).unwrap();

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

async fn enable_eth_with_tokens(
    mm: &MarketMakerIt,
    platform_coin: &str,
    tokens: &[&str],
    swap_contract_address: &str,
    nodes: &[&str],
    balance: bool,
) -> Json {
    let erc20_tokens_requests: Vec<_> = tokens.iter().map(|ticker| json!({ "ticker": ticker })).collect();
    let nodes: Vec<_> = nodes.iter().map(|url| json!({ "url": url })).collect();

    let enable = mm
        .rpc(&json!({
        "userpass": mm.userpass,
        "method": "enable_eth_with_tokens",
        "mmrpc": "2.0",
        "params": {
                "ticker": platform_coin,
                "erc20_tokens_requests": erc20_tokens_requests,
                "swap_contract_address": swap_contract_address,
                "nodes": nodes,
                "tx_history": true,
                "get_balances": balance,
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        enable.0,
        StatusCode::OK,
        "'enable_eth_with_tokens' failed: {}",
        enable.1
    );
    serde_json::from_str(&enable.1).unwrap()
}

#[test]
fn test_enable_eth_coin_with_token_then_disable() {
    let coin = erc20_coin_with_random_privkey(swap_contract());

    let priv_key = coin.display_priv_key().unwrap();
    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);

    let conf = Mm2TestConf::seednode(&priv_key, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_dump_log, _dump_dashboard) = mm.mm_dump();
    log!("log path: {}", mm.log_path.display());

    let swap_contract = swap_contract_checksum();
    block_on(enable_eth_with_tokens(
        &mm,
        "ETH",
        &["ERC20DEV"],
        &swap_contract,
        &[GETH_RPC_URL],
        true,
    ));

    // Create setprice order
    let req = json!({
        "userpass": mm.userpass,
        "method": "buy",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.1,
        "base_confs": 5,
        "base_nota": false,
        "rel_confs": 4,
        "rel_nota": false,
    });
    let make_test_order = block_on(mm.rpc(&req)).unwrap();
    assert_eq!(make_test_order.0, StatusCode::OK);
    let order_uuid = Json::from_str(&make_test_order.1).unwrap();
    let order_uuid = order_uuid.get("result").unwrap().get("uuid").unwrap().as_str().unwrap();

    // Passive ETH while having tokens enabled
    let res = block_on(disable_coin(&mm, "ETH", false));
    assert!(res.passivized);
    assert!(res.cancelled_orders.contains(order_uuid));

    // Try to disable ERC20DEV token.
    // This should work, because platform coin is still in the memory.
    let res = block_on(disable_coin(&mm, "ERC20DEV", false));
    // We expected make_test_order to be cancelled
    assert!(!res.passivized);

    // Because it's currently passive, default `disable_coin` should fail.
    block_on(disable_coin_err(&mm, "ETH", false));
    // And forced `disable_coin` should not fail
    let res = block_on(disable_coin(&mm, "ETH", true));
    assert!(!res.passivized);
}

#[test]
fn test_platform_coin_mismatch() {
    let coin = erc20_coin_with_random_privkey(swap_contract());

    let priv_key = coin.display_priv_key().unwrap();
    let mut erc20_conf = erc20_dev_conf(&erc20_contract_checksum());
    erc20_conf["protocol"]["protocol_data"]["platform"] = "MATIC".into(); // set a different platform coin
    let coins = json!([eth_dev_conf(), erc20_conf]);

    let conf = Mm2TestConf::seednode(&priv_key, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_dump_log, _dump_dashboard) = mm.mm_dump();
    log!("log path: {}", mm.log_path.display());

    let swap_contract = swap_contract_checksum();
    let erc20_tokens_requests = vec![json!({ "ticker": "ERC20DEV" })];
    let nodes = vec![json!({ "url": GETH_RPC_URL })];

    let enable = block_on(mm.rpc(&json!({
    "userpass": mm.userpass,
    "method": "enable_eth_with_tokens",
    "mmrpc": "2.0",
    "params": {
            "ticker": "ETH",
            "erc20_tokens_requests": erc20_tokens_requests,
            "swap_contract_address": swap_contract,
            "nodes": nodes,
            "tx_history": false,
            "get_balances": false,
        }
    })))
    .unwrap();
    assert_eq!(
        enable.0,
        StatusCode::BAD_REQUEST,
        "'enable_eth_with_tokens' must fail with PlatformCoinMismatch",
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&enable.1).unwrap()["error_type"]
            .as_str()
            .unwrap(),
        "PlatformCoinMismatch",
    );
}

#[test]
fn test_enable_eth_coin_with_token_without_balance() {
    let coin = erc20_coin_with_random_privkey(swap_contract());

    let priv_key = coin.display_priv_key().unwrap();
    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);

    let conf = Mm2TestConf::seednode(&priv_key, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_dump_log, _dump_dashboard) = mm.mm_dump();
    log!("log path: {}", mm.log_path.display());

    let swap_contract = swap_contract_checksum();
    let enable_eth_with_tokens = block_on(enable_eth_with_tokens(
        &mm,
        "ETH",
        &["ERC20DEV"],
        &swap_contract,
        &[GETH_RPC_URL],
        false,
    ));

    let enable_eth_with_tokens: RpcV2Response<IguanaEthWithTokensActivationResult> =
        serde_json::from_value(enable_eth_with_tokens).unwrap();

    let (_, eth_balance) = enable_eth_with_tokens
        .result
        .eth_addresses_infos
        .into_iter()
        .next()
        .unwrap();
    log!("{:?}", eth_balance);
    assert!(eth_balance.balances.is_none());
    assert!(eth_balance.tickers.is_none());

    let (_, erc20_balances) = enable_eth_with_tokens
        .result
        .erc20_addresses_infos
        .into_iter()
        .next()
        .unwrap();
    assert!(erc20_balances.balances.is_none());
    assert_eq!(
        erc20_balances.tickers.unwrap(),
        HashSet::from_iter(vec!["ERC20DEV".to_string()])
    );
}

#[test]
fn test_eth_swap_contract_addr_negotiation_same_fallback() {
    let bob_coin = erc20_coin_with_random_privkey(swap_contract());
    let alice_coin = erc20_coin_with_random_privkey(swap_contract());

    let bob_priv_key = bob_coin.display_priv_key().unwrap();
    let alice_priv_key = alice_coin.display_priv_key().unwrap();

    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);

    let bob_conf = Mm2TestConf::seednode(&bob_priv_key, &coins);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();

    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    let alice_conf = Mm2TestConf::light_node(&alice_priv_key, &coins, &[&mm_bob.ip.to_string()]);
    let mut mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    let swap_contract = swap_contract_checksum();

    dbg!(block_on(enable_eth_coin(
        &mm_bob,
        "ETH",
        &[GETH_RPC_URL],
        // using arbitrary address
        TEST_ARBITRARY_SWAP_ADDR_1,
        Some(&swap_contract),
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_bob,
        "ERC20DEV",
        &[GETH_RPC_URL],
        // using arbitrary address
        TEST_ARBITRARY_SWAP_ADDR_1,
        Some(&swap_contract),
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_alice,
        "ETH",
        &[GETH_RPC_URL],
        // using arbitrary address
        TEST_ARBITRARY_SWAP_ADDR_2,
        Some(&swap_contract),
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_alice,
        "ERC20DEV",
        &[GETH_RPC_URL],
        // using arbitrary address
        TEST_ARBITRARY_SWAP_ADDR_2,
        Some(&swap_contract),
        false
    )));

    let uuids = block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[("ETH", "ERC20DEV")],
        1.,
        1.,
        0.0001,
    ));

    // give few seconds for swap statuses to be saved
    thread::sleep(Duration::from_secs(3));

    let wait_until = get_utc_timestamp() + 30;
    let expected_contract = Json::from(swap_contract.trim_start_matches("0x"));

    block_on(wait_for_swap_contract_negotiation(
        &mm_bob,
        &uuids[0],
        expected_contract.clone(),
        wait_until,
    ));
    block_on(wait_for_swap_contract_negotiation(
        &mm_alice,
        &uuids[0],
        expected_contract,
        wait_until,
    ));
}

#[test]
fn test_eth_swap_negotiation_fails_maker_no_fallback() {
    let bob_coin = erc20_coin_with_random_privkey(swap_contract());
    let alice_coin = erc20_coin_with_random_privkey(swap_contract());

    let bob_priv_key = bob_coin.display_priv_key().unwrap();
    let alice_priv_key = alice_coin.display_priv_key().unwrap();

    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);

    let bob_conf = Mm2TestConf::seednode(&bob_priv_key, &coins);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();

    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    let alice_conf = Mm2TestConf::light_node(&alice_priv_key, &coins, &[&mm_bob.ip.to_string()]);
    let mut mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    let swap_contract = swap_contract_checksum();

    dbg!(block_on(enable_eth_coin(
        &mm_bob,
        "ETH",
        &[GETH_RPC_URL],
        // using arbitrary address
        TEST_ARBITRARY_SWAP_ADDR_1,
        None,
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_bob,
        "ERC20DEV",
        &[GETH_RPC_URL],
        // using arbitrary address
        TEST_ARBITRARY_SWAP_ADDR_1,
        None,
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_alice,
        "ETH",
        &[GETH_RPC_URL],
        // using arbitrary address
        TEST_ARBITRARY_SWAP_ADDR_2,
        Some(&swap_contract),
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_alice,
        "ERC20DEV",
        &[GETH_RPC_URL],
        // using arbitrary address
        TEST_ARBITRARY_SWAP_ADDR_2,
        Some(&swap_contract),
        false
    )));

    let uuids = block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[("ETH", "ERC20DEV")],
        1.,
        1.,
        0.0001,
    ));

    // give few seconds for swap statuses to be saved
    thread::sleep(Duration::from_secs(3));

    let wait_until = get_utc_timestamp() + 30;
    block_on(wait_for_swap_negotiation_failure(&mm_bob, &uuids[0], wait_until));
    block_on(wait_for_swap_negotiation_failure(&mm_alice, &uuids[0], wait_until));
}

#[test]
fn test_trade_base_rel_eth_erc20_coins() {
    trade_base_rel(("ETH", "ERC20DEV"));
}

fn withdraw_and_send(
    mm: &MarketMakerIt,
    coin: &str,
    from: Option<HDAccountAddressId>,
    to: &str,
    from_addr: &str,
    expected_bal_change: &str,
    amount: f64,
) {
    let withdraw = block_on(mm.rpc(&json! ({
        "mmrpc": "2.0",
        "userpass": mm.userpass,
        "method": "withdraw",
        "params": {
            "coin": coin,
            "from": from,
            "to": to,
            "amount": amount,
        },
        "id": 0,
    })))
    .unwrap();

    assert!(withdraw.0.is_success(), "!withdraw: {}", withdraw.1);
    let res: RpcSuccessResponse<TransactionDetails> =
        serde_json::from_str(&withdraw.1).expect("Expected 'RpcSuccessResponse<TransactionDetails>'");
    let tx_details = res.result;

    let mut expected_bal_change = BigDecimal::from_str(expected_bal_change).expect("!BigDecimal::from_str");

    let fee_details: TxFeeDetails = serde_json::from_value(tx_details.fee_details).unwrap();

    if let TxFeeDetails::Eth(fee_details) = fee_details {
        if coin == "ETH" {
            expected_bal_change -= fee_details.total_fee;
        }
    }

    assert_eq!(tx_details.to, vec![to.to_owned()]);
    assert_eq!(tx_details.my_balance_change, expected_bal_change);
    // Todo: Should check the from address for withdraws from another HD wallet address when there is an RPC method for addresses
    if from.is_none() {
        assert_eq!(tx_details.from, vec![from_addr.to_owned()]);
    }

    let send = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "send_raw_transaction",
        "coin": coin,
        "tx_hex": tx_details.tx_hex,
    })))
    .unwrap();
    assert!(send.0.is_success(), "!{} send: {}", coin, send.1);
    let send_json: Json = serde_json::from_str(&send.1).unwrap();
    assert_eq!(tx_details.tx_hash, send_json["tx_hash"]);
}

#[test]
fn test_withdraw_and_send_eth_erc20() {
    let privkey = random_secp256k1_secret();
    fill_eth_erc20_with_private_key(privkey);

    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);
    let mm = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(privkey)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("Alice log path: {}", mm.log_path.display());

    let swap_contract = swap_contract_checksum();
    let eth_enable = block_on(enable_eth_coin(
        &mm,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false,
    ));
    let erc20_enable = block_on(enable_eth_coin(
        &mm,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false,
    ));

    withdraw_and_send(
        &mm,
        "ETH",
        None,
        TEST_WITHDRAW_DEST_ADDR,
        eth_enable["address"].as_str().unwrap(),
        "-0.001",
        0.001,
    );

    withdraw_and_send(
        &mm,
        "ERC20DEV",
        None,
        TEST_WITHDRAW_DEST_ADDR,
        erc20_enable["address"].as_str().unwrap(),
        "-0.001",
        0.001,
    );

    // must not allow to withdraw to invalid checksum address
    let withdraw = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "withdraw",
        "params": {
            "coin": "ETH",
            "to": TEST_WITHDRAW_DEST_ADDR_INVALID_CHECKSUM,
            "amount": "0.001",
        },
        "id": 0,
    })))
    .unwrap();

    assert!(withdraw.0.is_client_error(), "ETH withdraw: {}", withdraw.1);
    let res: RpcErrorResponse<String> = serde_json::from_str(&withdraw.1).unwrap();
    assert_eq!(res.error_type, "InvalidAddress");
    assert!(res.error.contains("Invalid address checksum"));
}

#[test]
fn test_withdraw_and_send_hd_eth_erc20() {
    const PASSPHRASE: &str = "tank abandon bind salon remove wisdom net size aspect direct source fossil";

    let KeyPairPolicy::GlobalHDAccount(hd_acc) = CryptoCtx::init_with_global_hd_account(MM_CTX.clone(), PASSPHRASE)
        .unwrap()
        .key_pair_policy()
        .clone()
    else {
        panic!("Expected 'KeyPairPolicy::GlobalHDAccount'");
    };

    let swap_contract = swap_contract_checksum();

    // Withdraw from HD account 0, change address 0, index 1
    let mut path_to_address = HDAccountAddressId {
        account_id: 0,
        chain: Bip44Chain::External,
        address_id: 1,
    };
    let path_to_addr_str = "/0'/0/1";
    let path_to_coin: String = serde_json::from_value(eth_dev_conf()["derivation_path"].clone()).unwrap();
    let derivation_path = path_to_coin.clone() + path_to_addr_str;
    let derivation_path = DerivationPath::from_str(&derivation_path).unwrap();
    // Get the private key associated with this account and fill it with eth and erc20 token.
    let priv_key = hd_acc.derive_secp256k1_secret(&derivation_path).unwrap();
    fill_eth_erc20_with_private_key(priv_key);

    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);

    let conf = Mm2TestConf::seednode_with_hd_account(PASSPHRASE, &coins);
    let mm_hd = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm_hd.mm_dump();
    log!("Alice log path: {}", mm_hd.log_path.display());

    let eth_enable = block_on(task_enable_eth_with_tokens(
        &mm_hd,
        "ETH",
        &["ERC20DEV"],
        &swap_contract,
        &[GETH_RPC_URL],
        60,
        Some(path_to_address.clone()),
    ));
    let activation_result = match eth_enable {
        EthWithTokensActivationResult::HD(hd) => hd,
        _ => panic!("Expected EthWithTokensActivationResult::HD"),
    };
    let balance = match activation_result.wallet_balance {
        EnableCoinBalanceMap::HD(hd) => hd,
        _ => panic!("Expected EnableCoinBalance::HD"),
    };
    let account = balance.accounts.first().expect("Expected account at index 0");
    assert_eq!(
        account.addresses[1].address,
        "0xDe841899aB4A22E23dB21634e54920aDec402397"
    );
    assert_eq!(account.addresses[1].balance.len(), 2);
    assert_eq!(account.addresses[1].balance.get("ETH").unwrap().spendable, 100.into());
    assert_eq!(
        account.addresses[1].balance.get("ERC20DEV").unwrap().spendable,
        100.into()
    );

    withdraw_and_send(
        &mm_hd,
        "ETH",
        Some(path_to_address.clone()),
        TEST_WITHDRAW_DEST_ADDR,
        &account.addresses[1].address,
        "-0.001",
        0.001,
    );

    withdraw_and_send(
        &mm_hd,
        "ERC20DEV",
        Some(path_to_address.clone()),
        TEST_WITHDRAW_DEST_ADDR,
        &account.addresses[1].address,
        "-0.001",
        0.001,
    );

    // Change the address index, the withdrawal should fail.
    path_to_address.address_id = 0;

    let withdraw = block_on(mm_hd.rpc(&json! ({
        "mmrpc": "2.0",
        "userpass": mm_hd.userpass,
        "method": "withdraw",
        "params": {
            "coin": "ETH",
            "from": path_to_address,
            "to": TEST_WITHDRAW_DEST_ADDR,
            "amount": 0.001,
        },
        "id": 0,
    })))
    .unwrap();
    assert!(!withdraw.0.is_success(), "!withdraw: {}", withdraw.1);

    // But if we fill it, we should be able to withdraw.
    let path_to_addr_str = "/0'/0/0";
    let derivation_path = path_to_coin + path_to_addr_str;
    let derivation_path = DerivationPath::from_str(&derivation_path).unwrap();
    let priv_key = hd_acc.derive_secp256k1_secret(&derivation_path).unwrap();
    fill_eth_erc20_with_private_key(priv_key);

    let withdraw = block_on(mm_hd.rpc(&json! ({
        "mmrpc": "2.0",
        "userpass": mm_hd.userpass,
        "method": "withdraw",
        "params": {
            "coin": "ETH",
            "from": path_to_address,
            "to": TEST_WITHDRAW_DEST_ADDR,
            "amount": 0.001,
        },
        "id": 0,
    })))
    .unwrap();
    assert!(withdraw.0.is_success(), "!withdraw: {}", withdraw.1);

    block_on(mm_hd.stop()).unwrap();
}

fn check_too_low_volume_order_creation_fails(mm: &MarketMakerIt, base: &str, rel: &str) {
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": base,
        "rel": rel,
        "price": "1",
        "volume": "0.00000099",
        "cancel_previous": false,
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "setprice success, but should be error {}", rc.1);

    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": base,
        "rel": rel,
        "price": "0.00000000000000000099",
        "volume": "1",
        "cancel_previous": false,
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "setprice success, but should be error {}", rc.1);

    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "sell",
        "base": base,
        "rel": rel,
        "price": "1",
        "volume": "0.00000099",
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "sell success, but should be error {}", rc.1);

    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "buy",
        "base": base,
        "rel": rel,
        "price": "1",
        "volume": "0.00000099",
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "buy success, but should be error {}", rc.1);
}

#[test]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/481
fn test_setprice_buy_sell_too_low_volume() {
    let privkey = random_secp256k1_secret();

    // Fill the addresses with coins.
    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), privkey);
    generate_utxo_coin_with_privkey("MYCOIN1", 1000.into(), privkey);
    fill_eth_erc20_with_private_key(privkey);

    let coins = json!([
        mycoin_conf(1000),
        mycoin1_conf(1000),
        eth_dev_conf(),
        erc20_dev_conf(&erc20_contract_checksum())
    ]);
    let conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(privkey)), &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_dump_log, _dump_dashboard) = mm.mm_dump();
    log!("Log path: {}", mm.log_path.display());

    // Enable all the coins
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));

    let swap_contract = swap_contract_checksum();
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    check_too_low_volume_order_creation_fails(&mm, "MYCOIN", "ETH");
    check_too_low_volume_order_creation_fails(&mm, "ETH", "MYCOIN");
    check_too_low_volume_order_creation_fails(&mm, "ERC20DEV", "MYCOIN1");
}

#[test]
fn test_set_price_must_save_order_to_db() {
    let private_key_str = erc20_coin_with_random_privkey(swap_contract())
        .display_priv_key()
        .unwrap();
    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);

    let conf = Mm2TestConf::seednode(&private_key_str, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    // Enable coins
    let swap_contract = swap_contract_checksum();
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    log!("Issue bob ETH/ERC20DEV sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.1
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    let rc_json: Json = serde_json::from_str(&rc.1).unwrap();
    let uuid: String = serde_json::from_value(rc_json["result"]["uuid"].clone()).unwrap();
    let order_path = mm.folder.join(format!(
        "DB/{}/ORDERS/MY/MAKER/{}.json",
        hex::encode(rmd160_from_passphrase(&private_key_str)),
        uuid
    ));
    assert!(order_path.exists());
}

#[test]
fn test_set_price_response_format() {
    let privkey = random_secp256k1_secret();
    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), privkey);

    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000),]);

    let conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(privkey)), &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    // Enable coins
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));

    log!("Issue bob MYCOIN/MYCOIN1 sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": 0.1
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    let rc_json: Json = serde_json::from_str(&rc.1).unwrap();
    let _: BigDecimal = serde_json::from_value(rc_json["result"]["max_base_vol"].clone()).unwrap();
    let _: BigDecimal = serde_json::from_value(rc_json["result"]["min_base_vol"].clone()).unwrap();
    let _: BigDecimal = serde_json::from_value(rc_json["result"]["price"].clone()).unwrap();

    let _: BigRational = serde_json::from_value(rc_json["result"]["max_base_vol_rat"].clone()).unwrap();
    let _: BigRational = serde_json::from_value(rc_json["result"]["min_base_vol_rat"].clone()).unwrap();
    let _: BigRational = serde_json::from_value(rc_json["result"]["price_rat"].clone()).unwrap();
}

#[test]
fn test_buy_response_format() {
    let privkey = random_secp256k1_secret();
    generate_utxo_coin_with_privkey("MYCOIN1", 1000.into(), privkey);

    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000),]);

    let conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(privkey)), &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    // Enable coins
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));

    log!("Issue bob buy request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": 0.1,
        "base_confs": 5,
        "base_nota": true,
        "rel_confs": 4,
        "rel_nota": false,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);
    let _: BuyOrSellRpcResult = serde_json::from_str(&rc.1).unwrap();
}

#[test]
fn test_sell_response_format() {
    let privkey = random_secp256k1_secret();
    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), privkey);

    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000),]);

    let conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(privkey)), &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    // Enable coins
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));

    log!("Issue bob sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "sell",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": 0.1,
        "base_confs": 5,
        "base_nota": true,
        "rel_confs": 4,
        "rel_nota": false,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);
    let _: BuyOrSellRpcResult = serde_json::from_str(&rc.1).unwrap();
}

#[test]
fn test_set_price_conf_settings() {
    let private_key_str = erc20_coin_with_random_privkey(swap_contract())
        .display_priv_key()
        .unwrap();

    let coins = json!([eth_dev_conf(),{"coin":"ERC20DEV","name":"erc20dev","protocol":{"type":"ERC20","protocol_data":{"platform":"ETH","contract_address":erc20_contract_checksum()}},"required_confirmations":2},]);

    let conf = Mm2TestConf::seednode(&private_key_str, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    // Enable coins
    let swap_contract = swap_contract_checksum();
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    log!("Issue bob sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.1,
        "base_confs": 5,
        "base_nota": true,
        "rel_confs": 4,
        "rel_nota": false,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(json["result"]["conf_settings"]["base_confs"], Json::from(5));
    assert_eq!(json["result"]["conf_settings"]["base_nota"], Json::from(true));
    assert_eq!(json["result"]["conf_settings"]["rel_confs"], Json::from(4));
    assert_eq!(json["result"]["conf_settings"]["rel_nota"], Json::from(false));

    // must use coin config as defaults if not set in request
    log!("Issue bob sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.1,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(json["result"]["conf_settings"]["base_confs"], Json::from(1));
    assert_eq!(json["result"]["conf_settings"]["base_nota"], Json::from(false));
    assert_eq!(json["result"]["conf_settings"]["rel_confs"], Json::from(2));
    assert_eq!(json["result"]["conf_settings"]["rel_nota"], Json::from(false));
}

#[test]
fn test_buy_conf_settings() {
    let private_key_str = erc20_coin_with_random_privkey(swap_contract())
        .display_priv_key()
        .unwrap();

    let coins = json!([eth_dev_conf(),{"coin":"ERC20DEV","name":"erc20dev","protocol":{"type":"ERC20","protocol_data":{"platform":"ETH","contract_address":erc20_contract_checksum()}},"required_confirmations":2},]);

    let conf = Mm2TestConf::seednode(&private_key_str, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    // Enable coins
    let swap_contract = swap_contract_checksum();
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    log!("Issue bob buy request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "buy",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.1,
        "base_confs": 5,
        "base_nota": true,
        "rel_confs": 4,
        "rel_nota": false,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(json["result"]["conf_settings"]["base_confs"], Json::from(5));
    assert_eq!(json["result"]["conf_settings"]["base_nota"], Json::from(true));
    assert_eq!(json["result"]["conf_settings"]["rel_confs"], Json::from(4));
    assert_eq!(json["result"]["conf_settings"]["rel_nota"], Json::from(false));

    // must use coin config as defaults if not set in request
    log!("Issue bob buy request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "buy",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.1,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(json["result"]["conf_settings"]["base_confs"], Json::from(1));
    assert_eq!(json["result"]["conf_settings"]["base_nota"], Json::from(false));
    assert_eq!(json["result"]["conf_settings"]["rel_confs"], Json::from(2));
    assert_eq!(json["result"]["conf_settings"]["rel_nota"], Json::from(false));
}

#[test]
fn test_sell_conf_settings() {
    let private_key_str = erc20_coin_with_random_privkey(swap_contract())
        .display_priv_key()
        .unwrap();

    let coins = json!([eth_dev_conf(),{"coin":"ERC20DEV","name":"erc20dev","protocol":{"type":"ERC20","protocol_data":{"platform":"ETH","contract_address":erc20_contract_checksum()}},"required_confirmations":2},]);

    let conf = Mm2TestConf::seednode(&private_key_str, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    // Enable coins
    let swap_contract = swap_contract_checksum();
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    log!("Issue bob sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "sell",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.1,
        "base_confs": 5,
        "base_nota": true,
        "rel_confs": 4,
        "rel_nota": false,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(json["result"]["conf_settings"]["base_confs"], Json::from(5));
    assert_eq!(json["result"]["conf_settings"]["base_nota"], Json::from(true));
    assert_eq!(json["result"]["conf_settings"]["rel_confs"], Json::from(4));
    assert_eq!(json["result"]["conf_settings"]["rel_nota"], Json::from(false));

    // must use coin config as defaults if not set in request
    log!("Issue bob sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "sell",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.1,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(json["result"]["conf_settings"]["base_confs"], Json::from(1));
    assert_eq!(json["result"]["conf_settings"]["base_nota"], Json::from(false));
    assert_eq!(json["result"]["conf_settings"]["rel_confs"], Json::from(2));
    assert_eq!(json["result"]["conf_settings"]["rel_nota"], Json::from(false));
}

#[test]
fn test_my_orders_response_format() {
    let privkey = random_secp256k1_secret();
    generate_utxo_coin_with_privkey("MYCOIN1", 10000.into(), privkey);
    generate_utxo_coin_with_privkey("MYCOIN", 10000.into(), privkey);

    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000),]);

    let conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(privkey)), &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    // Enable coins
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));

    log!("Issue bob buy request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": 0.1,
        "base_confs": 5,
        "base_nota": true,
        "rel_confs": 4,
        "rel_nota": false,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    log!("Issue bob setprice request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": 0.1,
        "base_confs": 5,
        "base_nota": true,
        "rel_confs": 4,
        "rel_nota": false,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    log!("Issue bob my_orders request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "my_orders",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!my_orders: {}", rc.1);

    let _: MyOrdersRpcResult = serde_json::from_str(&rc.1).unwrap();
}

#[test]
fn test_my_orders_after_matched() {
    let bob_coin = erc20_coin_with_random_privkey(swap_contract());
    let alice_coin = erc20_coin_with_random_privkey(swap_contract());

    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);

    let bob_conf = Mm2TestConf::seednode(&bob_coin.display_priv_key().unwrap(), &coins);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();

    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    let alice_conf = Mm2TestConf::light_node(
        &alice_coin.display_priv_key().unwrap(),
        &coins,
        &[&mm_bob.ip.to_string()],
    );
    let mut mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    let swap_contract = swap_contract_checksum();
    dbg!(block_on(enable_eth_coin(
        &mm_bob,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_bob,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_alice,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_alice,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    let rc = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.000001,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.000001,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    block_on(mm_bob.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop ETH/ERC20DEV"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop ETH/ERC20DEV"))).unwrap();

    log!("Issue bob my_orders request");
    let rc = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "my_orders",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!my_orders: {}", rc.1);

    let _: MyOrdersRpcResult = serde_json::from_str(&rc.1).unwrap();
    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_update_maker_order_after_matched() {
    let bob_coin = erc20_coin_with_random_privkey(swap_contract());
    let alice_coin = erc20_coin_with_random_privkey(swap_contract());

    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);

    let bob_conf = Mm2TestConf::seednode(&bob_coin.display_priv_key().unwrap(), &coins);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();

    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    let alice_conf = Mm2TestConf::light_node(
        &alice_coin.display_priv_key().unwrap(),
        &coins,
        &[&mm_bob.ip.to_string()],
    );
    let mut mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    let swap_contract = swap_contract_checksum();
    dbg!(block_on(enable_eth_coin(
        &mm_bob,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_bob,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_alice,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_alice,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    let rc = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.00002,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    let setprice_json: Json = serde_json::from_str(&rc.1).unwrap();
    let uuid: String = serde_json::from_value(setprice_json["result"]["uuid"].clone()).unwrap();

    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.00001,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    block_on(mm_bob.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop ETH/ERC20DEV"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop ETH/ERC20DEV"))).unwrap();

    log!("Issue bob update maker order request that should fail because new volume is less than reserved amount");
    let update_maker_order = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "update_maker_order",
        "uuid": uuid,
        "volume_delta": -0.00002,
    })))
    .unwrap();
    assert!(
        !update_maker_order.0.is_success(),
        "update_maker_order success, but should be error {}",
        update_maker_order.1
    );

    log!("Issue another bob update maker order request");
    let update_maker_order = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "update_maker_order",
        "uuid": uuid,
        "volume_delta": 0.00001,
    })))
    .unwrap();
    assert!(
        update_maker_order.0.is_success(),
        "!update_maker_order: {}",
        update_maker_order.1
    );
    let update_maker_order_json: Json = serde_json::from_str(&update_maker_order.1).unwrap();
    log!("{}", update_maker_order.1);
    assert_eq!(update_maker_order_json["result"]["max_base_vol"], Json::from("0.00003"));

    log!("Issue bob my_orders request");
    let rc = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "my_orders",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!my_orders: {}", rc.1);

    let _: MyOrdersRpcResult = serde_json::from_str(&rc.1).unwrap();
    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_buy_min_volume() {
    let privkey = random_secp256k1_secret();
    generate_utxo_coin_with_privkey("MYCOIN1", 1000.into(), privkey);

    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000),]);

    let conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(privkey)), &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    // Enable coins
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));

    let min_volume: BigDecimal = "0.1".parse().unwrap();
    log!("Issue bob MYCOIN/MYCOIN1 buy request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": "2",
        "volume": "1",
        "min_volume": min_volume,
        "order_type": {
            "type": "GoodTillCancelled"
        },
        "timeout": 2,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);
    let response: BuyOrSellRpcResult = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(min_volume, response.result.min_volume);

    log!("Wait for 4 seconds for Bob order to be converted to maker");
    block_on(Timer::sleep(4.));

    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "my_orders",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!my_orders: {}", rc.1);
    let my_orders: MyOrdersRpcResult = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(
        1,
        my_orders.result.maker_orders.len(),
        "maker_orders must have exactly 1 order"
    );
    assert!(my_orders.result.taker_orders.is_empty(), "taker_orders must be empty");
    let maker_order = my_orders.result.maker_orders.get(&response.result.uuid).unwrap();

    let expected_min_volume: BigDecimal = "0.2".parse().unwrap();
    assert_eq!(expected_min_volume, maker_order.min_base_vol);
}

#[test]
fn test_sell_min_volume() {
    let privkey = random_secp256k1_secret();
    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), privkey);

    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000),]);

    let conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(privkey)), &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    // Enable coins
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));

    let min_volume: BigDecimal = "0.1".parse().unwrap();
    log!("Issue bob MYCOIN/MYCOIN1 sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "sell",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": "1",
        "volume": "1",
        "min_volume": min_volume,
        "order_type": {
            "type": "GoodTillCancelled"
        },
        "timeout": 2,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);
    let rc_json: Json = serde_json::from_str(&rc.1).unwrap();
    let uuid: String = serde_json::from_value(rc_json["result"]["uuid"].clone()).unwrap();
    let min_volume_response: BigDecimal = serde_json::from_value(rc_json["result"]["min_volume"].clone()).unwrap();
    assert_eq!(min_volume, min_volume_response);

    log!("Wait for 4 seconds for Bob order to be converted to maker");
    block_on(Timer::sleep(4.));

    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "my_orders",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!my_orders: {}", rc.1);
    let my_orders: Json = serde_json::from_str(&rc.1).unwrap();
    let my_maker_orders: HashMap<String, Json> =
        serde_json::from_value(my_orders["result"]["maker_orders"].clone()).unwrap();
    let my_taker_orders: HashMap<String, Json> =
        serde_json::from_value(my_orders["result"]["taker_orders"].clone()).unwrap();
    assert_eq!(1, my_maker_orders.len(), "maker_orders must have exactly 1 order");
    assert!(my_taker_orders.is_empty(), "taker_orders must be empty");
    let maker_order = my_maker_orders.get(&uuid).unwrap();
    let min_volume_maker: BigDecimal = serde_json::from_value(maker_order["min_base_vol"].clone()).unwrap();
    assert_eq!(min_volume, min_volume_maker);
}

#[test]
fn test_setprice_min_volume_dust() {
    let privkey = random_secp256k1_secret();
    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), privkey);

    let coins = json! ([
        {"coin":"MYCOIN","asset":"MYCOIN","txversion":4,"overwintered":1,"txfee":1000,"dust":10000000,"protocol":{"type":"UTXO"}},
        mycoin1_conf(1000),
    ]);

    let conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(privkey)), &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    // Enable coins
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));

    log!("Issue bob MYCOIN/MYCOIN1 sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": "1",
        "volume": "1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    let response: SetPriceResponse = serde_json::from_str(&rc.1).unwrap();
    let expected_min = BigDecimal::from(1);
    assert_eq!(expected_min, response.result.min_base_vol);

    log!("Issue bob MYCOIN/MYCOIN1 sell request less than dust");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": "1",
        // Less than dust, should fial
        "volume": 0.01,
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "!setprice: {}", rc.1);
}

#[test]
fn test_sell_min_volume_dust() {
    let privkey = random_secp256k1_secret();
    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), privkey);

    let coins = json! ([
        {"coin":"MYCOIN","asset":"MYCOIN","txversion":4,"overwintered":1,"txfee":1000,"dust":10000000,"protocol":{"type":"UTXO"}},
        mycoin1_conf(1000),
    ]);

    let conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(privkey)), &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    // Enable coins
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));

    log!("Issue bob MYCOIN/MYCOIN1 sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "sell",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": "1",
        "volume": "1",
        "order_type": {
            "type": "FillOrKill"
        }
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);
    let response: BuyOrSellRpcResult = serde_json::from_str(&rc.1).unwrap();
    let expected_min = BigDecimal::from(1);
    assert_eq!(response.result.min_volume, expected_min);

    log!("Issue bob MYCOIN/MYCOIN1 sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "sell",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": "1",
        // Less than dust
        "volume": 0.01,
        "order_type": {
            "type": "FillOrKill"
        }
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "!sell: {}", rc.1);
}

fn request_and_check_orderbook_depth(mm_alice: &MarketMakerIt) {
    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "orderbook_depth",
        "pairs": [("MYCOIN", "MYCOIN1"), ("MYCOIN", "ETH"), ("MYCOIN1", "ETH")],
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook_depth: {}", rc.1);
    let response: OrderbookDepthResponse = serde_json::from_str(&rc.1).unwrap();
    let mycoin_mycoin1 = response
        .result
        .iter()
        .find(|pair_depth| pair_depth.pair.0 == "MYCOIN" && pair_depth.pair.1 == "MYCOIN1")
        .unwrap();
    assert_eq!(3, mycoin_mycoin1.depth.asks);
    assert_eq!(2, mycoin_mycoin1.depth.bids);

    let mycoin_eth = response
        .result
        .iter()
        .find(|pair_depth| pair_depth.pair.0 == "MYCOIN" && pair_depth.pair.1 == "ETH")
        .unwrap();
    assert_eq!(1, mycoin_eth.depth.asks);
    assert_eq!(1, mycoin_eth.depth.bids);

    let mycoin1_eth = response
        .result
        .iter()
        .find(|pair_depth| pair_depth.pair.0 == "MYCOIN1" && pair_depth.pair.1 == "ETH")
        .unwrap();
    assert_eq!(0, mycoin1_eth.depth.asks);
    assert_eq!(0, mycoin1_eth.depth.bids);
}

#[test]
fn test_orderbook_depth() {
    let bob_priv_key = random_secp256k1_secret();
    let alice_priv_key = random_secp256k1_secret();
    let swap_contract = swap_contract_checksum();

    // Fill bob's addresses with coins.
    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), bob_priv_key);
    generate_utxo_coin_with_privkey("MYCOIN1", 1000.into(), bob_priv_key);
    fill_eth_erc20_with_private_key(bob_priv_key);

    // Fill alice's addresses with coins.
    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), alice_priv_key);
    generate_utxo_coin_with_privkey("MYCOIN1", 1000.into(), alice_priv_key);
    fill_eth_erc20_with_private_key(alice_priv_key);

    let coins = json!([
        mycoin_conf(1000),
        mycoin1_conf(1000),
        eth_dev_conf(),
        erc20_dev_conf(&erc20_contract_checksum())
    ]);

    let bob_conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(bob_priv_key)), &coins);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();

    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    // Enable all the coins for bob
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    dbg!(block_on(enable_eth_coin(
        &mm_bob,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));
    dbg!(block_on(enable_eth_coin(
        &mm_bob,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    // issue sell request on Bob side by setting base/rel price
    log!("Issue bob sell requests");
    let bob_orders = [
        // (base, rel, price, volume, min_volume)
        ("MYCOIN", "MYCOIN1", "0.9", "0.9", None),
        ("MYCOIN", "MYCOIN1", "0.8", "0.9", None),
        ("MYCOIN", "MYCOIN1", "0.7", "0.9", Some("0.9")),
        ("MYCOIN", "ETH", "0.8", "0.9", None),
        ("MYCOIN1", "MYCOIN", "0.8", "0.9", None),
        ("MYCOIN1", "MYCOIN", "0.9", "0.9", None),
        ("ETH", "MYCOIN", "0.8", "0.9", None),
    ];
    for (base, rel, price, volume, min_volume) in bob_orders.iter() {
        let rc = block_on(mm_bob.rpc(&json! ({
            "userpass": mm_bob.userpass,
            "method": "setprice",
            "base": base,
            "rel": rel,
            "price": price,
            "volume": volume,
            "min_volume": min_volume.unwrap_or("0.00777"),
            "cancel_previous": false,
        })))
        .unwrap();
        assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    }

    let alice_conf = Mm2TestConf::light_node(
        &format!("0x{}", hex::encode(alice_priv_key)),
        &coins,
        &[&mm_bob.ip.to_string()],
    );
    let mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    block_on(mm_bob.wait_for_log(22., |log| {
        log.contains("DEBUG Handling IncludedTorelaysMesh message for peer")
    }))
    .unwrap();

    request_and_check_orderbook_depth(&mm_alice);
    // request MYCOIN/MYCOIN1 orderbook to subscribe Alice
    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    request_and_check_orderbook_depth(&mm_alice);

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_approve_erc20() {
    let privkey = random_secp256k1_secret();
    fill_eth_erc20_with_private_key(privkey);

    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);
    let mm = MarketMakerIt::start(
        Mm2TestConf::seednode(&format!("0x{}", hex::encode(privkey)), &coins).conf,
        DEFAULT_RPC_PASSWORD.to_string(),
        None,
    )
    .unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("Node log path: {}", mm.log_path.display());

    let swap_contract = swap_contract_checksum();
    let _eth_enable = block_on(enable_eth_coin(
        &mm,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false,
    ));
    let _erc20_enable = block_on(enable_eth_coin(
        &mm,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false,
    ));

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method":"approve_token",
        "mmrpc":"2.0",
        "id": 0,
        "params":{
          "coin": "ERC20DEV",
          "spender": swap_contract,
          "amount": BigDecimal::from_str("11.0").unwrap(),
        }
    })))
    .unwrap();
    assert!(rc.0.is_success(), "approve_token error: {}", rc.1);
    let res = serde_json::from_str::<Json>(&rc.1).unwrap();
    assert!(
        hex::decode(str_strip_0x!(res["result"].as_str().unwrap())).is_ok(),
        "approve_token result incorrect"
    );
    thread::sleep(Duration::from_secs(5));
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method":"get_token_allowance",
        "mmrpc":"2.0",
        "id": 0,
        "params":{
          "coin": "ERC20DEV",
          "spender": swap_contract,
        }
    })))
    .unwrap();
    assert!(rc.0.is_success(), "get_token_allowance error: {}", rc.1);
    let res = serde_json::from_str::<Json>(&rc.1).unwrap();
    assert_eq!(
        BigDecimal::from_str(res["result"].as_str().unwrap()).unwrap(),
        BigDecimal::from_str("11.0").unwrap(),
        "get_token_allowance result incorrect"
    );

    block_on(mm.stop()).unwrap();
}

#[test]
fn test_peer_time_sync_validation() {
    let timeoffset_tolerable = TryInto::<i64>::try_into(MAX_TIME_GAP_FOR_CONNECTED_PEER).unwrap() - 1;
    let timeoffset_too_big = TryInto::<i64>::try_into(MAX_TIME_GAP_FOR_CONNECTED_PEER).unwrap() + 1;

    let start_peers_with_time_offset = |offset: i64| -> (Json, Json) {
        let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 10.into());
        let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 10.into());
        let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
        let bob_conf = Mm2TestConf::seednode(&hex::encode(bob_priv_key), &coins);
        let mut mm_bob = block_on(MarketMakerIt::start_with_envs(
            bob_conf.conf,
            bob_conf.rpc_password,
            None,
            &[],
        ))
        .unwrap();
        let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);
        block_on(mm_bob.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();
        let alice_conf =
            Mm2TestConf::light_node(&hex::encode(alice_priv_key), &coins, &[mm_bob.ip.to_string().as_str()]);
        let mut mm_alice = block_on(MarketMakerIt::start_with_envs(
            alice_conf.conf,
            alice_conf.rpc_password,
            None,
            &[("TEST_TIMESTAMP_OFFSET", offset.to_string().as_str())],
        ))
        .unwrap();
        let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);
        block_on(mm_alice.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();

        let res_bob = block_on(mm_bob.rpc(&json!({
            "userpass": mm_bob.userpass,
            "method": "get_directly_connected_peers",
        })))
        .unwrap();
        assert!(res_bob.0.is_success(), "!get_directly_connected_peers: {}", res_bob.1);
        let bob_peers = serde_json::from_str::<Json>(&res_bob.1).unwrap();

        let res_alice = block_on(mm_alice.rpc(&json!({
            "userpass": mm_alice.userpass,
            "method": "get_directly_connected_peers",
        })))
        .unwrap();
        assert!(
            res_alice.0.is_success(),
            "!get_directly_connected_peers: {}",
            res_alice.1
        );
        let alice_peers = serde_json::from_str::<Json>(&res_alice.1).unwrap();

        block_on(mm_bob.stop()).unwrap();
        block_on(mm_alice.stop()).unwrap();
        (bob_peers, alice_peers)
    };

    // check with small time offset:
    let (bob_peers, alice_peers) = start_peers_with_time_offset(timeoffset_tolerable);
    assert!(
        bob_peers["result"].as_object().unwrap().len() == 1,
        "bob must have one peer"
    );
    assert!(
        alice_peers["result"].as_object().unwrap().len() == 1,
        "alice must have one peer"
    );

    // check with too big time offset:
    let (bob_peers, alice_peers) = start_peers_with_time_offset(timeoffset_too_big);
    assert!(
        bob_peers["result"].as_object().unwrap().is_empty(),
        "bob must have no peers"
    );
    assert!(
        alice_peers["result"].as_object().unwrap().is_empty(),
        "alice must have no peers"
    );
}
