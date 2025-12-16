// UTXO Ordermatching V1 Tests
//
// This module contains UTXO-only ordermatching tests that were extracted from docker_tests_inner.rs
// These tests focus on orderbook behavior, order lifecycle, balance-driven updates, and matching logic.
// They do NOT require ETH/ERC20 containers - only MYCOIN/MYCOIN1 UTXO containers.
//
// Gated by: docker-tests-ordermatch

use crate::docker_tests::helpers::env::random_secp256k1_secret;
use crate::docker_tests::helpers::utxo::{
    fill_address, generate_utxo_coin_with_privkey, generate_utxo_coin_with_random_privkey, rmd160_from_priv,
};
use crate::integration_tests_common::*;
use coins::{ConfirmPaymentInput, MarketCoinOps, MmCoin, WithdrawRequest};
use common::{block_on, block_on_f01, executor::Timer, wait_until_sec};
use mm2_libp2p::behaviours::atomicdex::MAX_TIME_GAP_FOR_CONNECTED_PEER;
use mm2_number::{BigDecimal, BigRational};
use mm2_test_helpers::for_tests::{
    check_my_swap_status_amounts, mm_dump, mycoin1_conf, mycoin_conf, MarketMakerIt, Mm2TestConf,
};
use mm2_test_helpers::structs::*;
use serde_json::{json, Value as Json};
use std::collections::HashMap;
use std::convert::TryInto;
use std::env;
use std::thread;
use std::time::Duration;

// =============================================================================
// Order Lifecycle Tests
// Tests for order creation, cancellation, and balance-driven updates
// =============================================================================

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

// =============================================================================
// Partial Fill Tests
// Tests for order updates when partially matched
// =============================================================================

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

// =============================================================================
// Order Volume Tests
// Tests for setprice max volume and volume constraints
// =============================================================================

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

// =============================================================================
// Order Restart/Persistence Tests
// Tests for maker order kickstart on MM restart
// =============================================================================

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

// =============================================================================
// Same Private Key Edge Cases
// Tests for edge cases when using the same private key across nodes
// =============================================================================

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

// =============================================================================
// Order Conversion and Cancellation Tests
// Tests for taker-to-maker order conversion and proper cleanup
// =============================================================================

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

// =============================================================================
// Best Price Matching Tests
// Tests for ensuring taker matches with best available price
// =============================================================================

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
    let (_eve_dump_log, _eve_dump_dashboard) = mm_dump(&mm_eve.log_path);

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
    let (_eve_dump_log, _eve_dump_dashboard) = mm_dump(&mm_eve.log_path);

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
    assert!(rc.0.is_success(), "!sell: {}", rc.1);
    let alice_sell: BuyOrSellRpcResult = serde_json::from_str(&rc.1).unwrap();

    block_on(mm_eve.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop MYCOIN/MYCOIN1"))).unwrap();

    thread::sleep(Duration::from_secs(2));

    block_on(check_my_swap_status_amounts(
        &mm_alice,
        alice_sell.result.uuid,
        1000.into(),
        1000.into(),
    ));
    block_on(check_my_swap_status_amounts(
        &mm_eve,
        alice_sell.result.uuid,
        1000.into(),
        1000.into(),
    ));

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
    block_on(mm_eve.stop()).unwrap();
}

// =============================================================================
// RPC Response Format Tests
// Tests for validating RPC response formats (UTXO-only variants)
// =============================================================================

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

// =============================================================================
// Min Volume and Dust Tests
// Tests for order min_volume constraints and dust thresholds
// =============================================================================

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

// =============================================================================
// P2P Infrastructure Tests
// These tests verify P2P networking behavior (UTXO-based, coin-agnostic)
// =============================================================================

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
