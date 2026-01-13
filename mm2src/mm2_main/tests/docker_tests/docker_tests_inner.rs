// Docker Tests Inner - Cross-Chain Ordermatching Tests
//
// This module contains tests that require BOTH ETH and UTXO chains for ordermatching.
// These tests cannot be placed in either eth_inner_tests.rs or utxo_ordermatch_v1_tests.rs
// because they require cross-chain functionality.
//
// ETH-only tests have been extracted to: eth_inner_tests.rs
// UTXO-only ordermatching tests have been extracted to: utxo_ordermatch_v1_tests.rs
//
// Gated by: docker-tests-ordermatch (cross-chain ordermatching tests)

use crate::docker_tests::helpers::env::random_secp256k1_secret;
use crate::docker_tests::helpers::eth::{
    erc20_contract_checksum, fill_eth_erc20_with_private_key, swap_contract_checksum, GETH_RPC_URL,
};
use crate::docker_tests::helpers::utxo::generate_utxo_coin_with_privkey;
use crate::integration_tests_common::*;
use common::block_on;
use crypto::privkey::key_pair_from_seed;
use mm2_number::BigDecimal;
use mm2_test_helpers::for_tests::{
    best_orders_v2, best_orders_v2_by_number, enable_eth_coin, erc20_dev_conf, eth_dev_conf, mm_dump, my_balance,
    mycoin1_conf, mycoin_conf, MarketMakerIt, Mm2TestConf,
};
use mm2_test_helpers::structs::BestOrdersResponse;
use mm2_test_helpers::{get_passphrase, structs::*};
use serde_json::json;

// =============================================================================
// Cross-Chain Matching Tests (UTXO + ETH)
// These tests verify order matching between different chain types
// =============================================================================

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

// =============================================================================
// Cross-Chain Volume Validation Tests
// These tests check order volume constraints across ETH and UTXO coins
// =============================================================================

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

// =============================================================================
// Cross-Chain Orderbook Depth Tests
// These tests verify orderbook depth calculation across multiple chain types
// =============================================================================

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

// =============================================================================
// Cross-Chain Best Orders Tests
// These tests verify best_orders RPC across ETH and UTXO coins
// =============================================================================

fn get_bob_alice() -> (MarketMakerIt, MarketMakerIt) {
    let bob_priv_key = random_secp256k1_secret();
    let alice_priv_key = random_secp256k1_secret();

    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), bob_priv_key);
    generate_utxo_coin_with_privkey("MYCOIN1", 1000.into(), bob_priv_key);
    fill_eth_erc20_with_private_key(bob_priv_key);

    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), alice_priv_key);
    generate_utxo_coin_with_privkey("MYCOIN1", 1000.into(), alice_priv_key);
    fill_eth_erc20_with_private_key(alice_priv_key);

    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000), eth_dev_conf(),]);

    let bob_conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(bob_priv_key)), &coins);
    let mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();

    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    let alice_conf = Mm2TestConf::light_node(
        &format!("0x{}", hex::encode(alice_priv_key)),
        &coins,
        &[&mm_bob.ip.to_string()],
    );
    let mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));

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
        &mm_alice,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    (mm_bob, mm_alice)
}

#[test]
fn test_best_orders() {
    let (mut mm_bob, mm_alice) = get_bob_alice();

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
        ("MYCOIN1", "ETH", "0.8", "0.8", None),
        ("MYCOIN1", "ETH", "0.7", "0.8", Some("0.8")),
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

    block_on(mm_bob.wait_for_log(22., |log| {
        log.contains("DEBUG Handling IncludedTorelaysMesh message for peer")
    }))
    .unwrap();

    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "best_orders",
        "coin": "MYCOIN",
        "action": "buy",
        "volume": "0.1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!best_orders: {}", rc.1);
    let response: BestOrdersResponse = serde_json::from_str(&rc.1).unwrap();
    let best_mycoin1_orders = response.result.get("MYCOIN1").unwrap();
    assert_eq!(1, best_mycoin1_orders.len());
    let expected_price: BigDecimal = "0.8".parse().unwrap();
    assert_eq!(expected_price, best_mycoin1_orders[0].price);

    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "best_orders",
        "coin": "MYCOIN",
        "action": "buy",
        "volume": "1.7",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!best_orders: {}", rc.1);
    let response: BestOrdersResponse = serde_json::from_str(&rc.1).unwrap();
    // MYCOIN1
    let best_mycoin1_orders = response.result.get("MYCOIN1").unwrap();
    let expected_price: BigDecimal = "0.7".parse().unwrap();
    let bob_mycoin1_addr = block_on(my_balance(&mm_bob, "MYCOIN1")).address;
    // let bob_mycoin1_addr = mm_bob.display_address("MYCOIN1").unwrap();
    assert_eq!(expected_price, best_mycoin1_orders[0].price);
    assert_eq!(bob_mycoin1_addr, best_mycoin1_orders[0].address);
    let expected_price: BigDecimal = "0.8".parse().unwrap();
    assert_eq!(expected_price, best_mycoin1_orders[1].price);
    assert_eq!(bob_mycoin1_addr, best_mycoin1_orders[1].address);
    // ETH
    let expected_price: BigDecimal = "0.8".parse().unwrap();
    let best_eth_orders = response.result.get("ETH").unwrap();
    assert_eq!(expected_price, best_eth_orders[0].price);

    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "best_orders",
        "coin": "MYCOIN",
        "action": "sell",
        "volume": "0.1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!best_orders: {}", rc.1);
    let response: BestOrdersResponse = serde_json::from_str(&rc.1).unwrap();

    let expected_price: BigDecimal = "1.25".parse().unwrap();

    let best_mycoin1_orders = response.result.get("MYCOIN1").unwrap();
    assert_eq!(expected_price, best_mycoin1_orders[0].price);
    assert_eq!(1, best_mycoin1_orders.len());

    let best_eth_orders = response.result.get("ETH").unwrap();
    assert_eq!(expected_price, best_eth_orders[0].price);

    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "best_orders",
        "coin": "ETH",
        "action": "sell",
        "volume": "0.1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!best_orders: {}", rc.1);
    let response: BestOrdersResponse = serde_json::from_str(&rc.1).unwrap();

    let expected_price: BigDecimal = "1.25".parse().unwrap();

    let best_mycoin1_orders = response.result.get("MYCOIN1").unwrap();
    assert_eq!(expected_price, best_mycoin1_orders[0].price);
    assert_eq!("MYCOIN1", best_mycoin1_orders[0].coin);
    assert_eq!(1, best_mycoin1_orders.len());

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_best_orders_v2_by_number() {
    let (mut mm_bob, mm_alice) = get_bob_alice();

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
        ("MYCOIN1", "ETH", "0.8", "0.8", None),
        ("MYCOIN1", "ETH", "0.7", "0.8", Some("0.8")),
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

    block_on(mm_bob.wait_for_log(22., |log| {
        log.contains("DEBUG Handling IncludedTorelaysMesh message for peer")
    }))
    .unwrap();

    let response = block_on(best_orders_v2_by_number(&mm_alice, "MYCOIN", "buy", 1, false));
    log!("response {response:?}");
    let best_mycoin1_orders = response.result.orders.get("MYCOIN1").unwrap();
    log!("Best MYCOIN1 orders when buy MYCOIN {:?}", [best_mycoin1_orders]);
    assert_eq!(1, best_mycoin1_orders.len());
    let expected_price: BigDecimal = "0.7".parse().unwrap();
    assert_eq!(expected_price, best_mycoin1_orders[0].price.decimal);

    let response = block_on(best_orders_v2_by_number(&mm_alice, "MYCOIN", "buy", 2, false));
    log!("response {response:?}");
    let best_mycoin1_orders = response.result.orders.get("MYCOIN1").unwrap();
    log!("Best MYCOIN1 orders when buy MYCOIN {:?}", [best_mycoin1_orders]);
    assert_eq!(2, best_mycoin1_orders.len());
    let expected_price: BigDecimal = "0.7".parse().unwrap();
    assert_eq!(expected_price, best_mycoin1_orders[0].price.decimal);
    let expected_price: BigDecimal = "0.8".parse().unwrap();
    assert_eq!(expected_price, best_mycoin1_orders[1].price.decimal);

    let response = block_on(best_orders_v2_by_number(&mm_alice, "MYCOIN", "sell", 1, false));
    log!("response {response:?}");
    let expected_price: BigDecimal = "1.25".parse().unwrap();
    let best_mycoin1_orders = response.result.orders.get("MYCOIN1").unwrap();
    log!("Best MYCOIN1 orders when sell MYCOIN {:?}", [best_mycoin1_orders]);
    assert_eq!(1, best_mycoin1_orders.len());
    assert_eq!(expected_price, best_mycoin1_orders[0].price.decimal);
    let best_eth_orders = response.result.orders.get("ETH").unwrap();
    log!("Best ETH orders when sell MYCOIN {:?}", [best_eth_orders]);
    assert_eq!(1, best_eth_orders.len());
    assert_eq!(expected_price, best_eth_orders[0].price.decimal);

    let response = block_on(best_orders_v2_by_number(&mm_alice, "ETH", "sell", 1, false));
    log!("response {response:?}");
    let best_mycoin_orders = response.result.orders.get("MYCOIN").unwrap();
    log!("Best MYCOIN orders when sell ETH {:?}", [best_mycoin_orders]);
    assert_eq!(1, best_mycoin_orders.len());
    let expected_price: BigDecimal = "1.25".parse().unwrap();
    assert_eq!(expected_price, best_mycoin_orders[0].price.decimal);

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_best_orders_v2_by_volume() {
    let (mut mm_bob, mm_alice) = get_bob_alice();

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
        ("MYCOIN1", "ETH", "0.8", "0.8", None),
        ("MYCOIN1", "ETH", "0.7", "0.8", Some("0.8")),
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

    block_on(mm_bob.wait_for_log(22., |log| {
        log.contains("DEBUG Handling IncludedTorelaysMesh message for peer")
    }))
    .unwrap();

    let response = block_on(best_orders_v2(&mm_alice, "MYCOIN", "buy", "1.7"));
    log!("response {response:?}");
    // MYCOIN1
    let best_mycoin1_orders = response.result.orders.get("MYCOIN1").unwrap();
    log!("Best MYCOIN1 orders when buy MYCOIN {:?}", [best_mycoin1_orders]);
    let expected_price: BigDecimal = "0.7".parse().unwrap();
    assert_eq!(expected_price, best_mycoin1_orders[0].price.decimal);
    let expected_price: BigDecimal = "0.8".parse().unwrap();
    assert_eq!(expected_price, best_mycoin1_orders[1].price.decimal);
    // ETH
    let expected_price: BigDecimal = "0.8".parse().unwrap();
    let best_eth_orders = response.result.orders.get("ETH").unwrap();
    log!("Best ETH orders when buy MYCOIN {:?}", [best_eth_orders]);
    assert_eq!(expected_price, best_eth_orders[0].price.decimal);

    let response = block_on(best_orders_v2(&mm_alice, "MYCOIN", "sell", "0.1"));
    log!("response {response:?}");
    let expected_price: BigDecimal = "1.25".parse().unwrap();
    let best_mycoin1_orders = response.result.orders.get("MYCOIN1").unwrap();
    log!("Best MYCOIN1 orders when sell MYCOIN {:?}", [best_mycoin1_orders]);
    assert_eq!(expected_price, best_mycoin1_orders[0].price.decimal);
    assert_eq!(1, best_mycoin1_orders.len());
    let best_eth_orders = response.result.orders.get("ETH").unwrap();
    log!("Best ETH orders when sell MYCOIN {:?}", [best_mycoin1_orders]);
    assert_eq!(expected_price, best_eth_orders[0].price.decimal);

    let response = block_on(best_orders_v2(&mm_alice, "ETH", "sell", "0.1"));
    log!("response {response:?}");
    let expected_price: BigDecimal = "1.25".parse().unwrap();
    let best_mycoin1_orders = response.result.orders.get("MYCOIN1").unwrap();
    log!("Best MYCOIN1 orders when sell ETH {:?}", [best_mycoin1_orders]);
    assert_eq!(expected_price, best_mycoin1_orders[0].price.decimal);
    assert_eq!("MYCOIN1", best_mycoin1_orders[0].coin);
    assert_eq!(1, best_mycoin1_orders.len());

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_best_orders_filter_response() {
    // alice defined MYCOIN1 as "wallet_only" in config
    let alice_coins = json!([
        mycoin_conf(1000),
        {"coin":"MYCOIN1","asset":"MYCOIN1","rpcport":11608,"wallet_only": true,"txversion":4,"overwintered":1,"protocol":{"type":"UTXO"}},
        eth_dev_conf(),
    ]);

    let bob_priv_key = random_secp256k1_secret();
    let alice_priv_key = random_secp256k1_secret();

    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), bob_priv_key);
    generate_utxo_coin_with_privkey("MYCOIN1", 1000.into(), bob_priv_key);
    fill_eth_erc20_with_private_key(bob_priv_key);

    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), alice_priv_key);
    generate_utxo_coin_with_privkey("MYCOIN1", 1000.into(), alice_priv_key);
    fill_eth_erc20_with_private_key(alice_priv_key);

    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000), eth_dev_conf(),]);

    let bob_conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(bob_priv_key)), &coins);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();

    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    let swap_contract = swap_contract_checksum();
    dbg!(block_on(enable_eth_coin(
        &mm_bob,
        "ETH",
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
        ("MYCOIN1", "ETH", "0.8", "0.8", None),
        ("MYCOIN1", "ETH", "0.7", "0.8", Some("0.8")),
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
        &alice_coins,
        &[&mm_bob.ip.to_string()],
    );
    let mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    block_on(mm_bob.wait_for_log(22., |log| {
        log.contains("DEBUG Handling IncludedTorelaysMesh message for peer")
    }))
    .unwrap();

    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "best_orders",
        "coin": "MYCOIN",
        "action": "buy",
        "volume": "0.1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!best_orders: {}", rc.1);
    let response: BestOrdersResponse = serde_json::from_str(&rc.1).unwrap();
    let empty_vec = Vec::new();
    let best_mycoin1_orders = response.result.get("MYCOIN1").unwrap_or(&empty_vec);
    assert_eq!(0, best_mycoin1_orders.len());
    let best_eth_orders = response.result.get("ETH").unwrap();
    assert_eq!(1, best_eth_orders.len());

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}
