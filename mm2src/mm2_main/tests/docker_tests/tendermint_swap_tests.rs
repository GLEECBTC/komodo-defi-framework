// Tendermint Cross-Chain Swap Tests
//
// This module contains tests that require Tendermint AND other chain types (ETH, Electrum).
// These tests cannot be placed in tendermint_tests.rs because they require additional
// infrastructure beyond Tendermint nodes.
//
// Tests:
// - swap_nucleus_with_doc: NUCLEUS <-> DOC (Tendermint + Electrum)
// - swap_nucleus_with_eth: NUCLEUS <-> ETH (Tendermint + Geth)
// - swap_doc_with_iris_ibc_nucleus: DOC <-> IRIS-IBC-NUCLEUS (Tendermint + Electrum)
//
// Gated by: docker-tests-tendermint + docker-tests-eth (cross-chain Tendermint swaps)

use crate::docker_tests::helpers::eth::{fill_eth, swap_contract_checksum, GETH_RPC_URL};
use crate::integration_tests_common::enable_electrum;
use common::executor::Timer;
use common::{block_on, log};
use compatible_time::Duration;
use ethereum_types::{Address, U256};
use mm2_number::BigDecimal;
use mm2_rpc::data::legacy::OrderbookResponse;
use mm2_test_helpers::for_tests::{
    check_my_swap_status, check_recent_swaps, doc_conf, enable_eth_coin, enable_tendermint, eth_dev_conf,
    iris_ibc_nucleus_testnet_conf, nucleus_testnet_conf, wait_check_stats_swap_status, MarketMakerIt,
    DOC_ELECTRUM_ADDRS,
};
use serde_json::json;
use std::convert::TryFrom;
use std::env;
use std::str::FromStr;
use std::sync::Mutex;
use std::thread;

const NUCLEUS_TESTNET_RPC_URLS: &[&str] = &["http://localhost:26657"];

const BOB_PASSPHRASE: &str = "iris test seed";
const ALICE_PASSPHRASE: &str = "iris test2 seed";

lazy_static! {
    /// Makes sure that tests sending transactions run sequentially to prevent account sequence
    /// mismatches as some addresses are used in multiple tests.
    static ref SEQUENCE_LOCK: Mutex<()> = Mutex::new(());
}

#[test]
fn swap_nucleus_with_doc() {
    let _lock = SEQUENCE_LOCK.lock().unwrap();
    let bob_passphrase = String::from(BOB_PASSPHRASE);
    let alice_passphrase = String::from(ALICE_PASSPHRASE);

    let coins = json!([nucleus_testnet_conf(), doc_conf()]);

    let mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 8999,
            "dht": "on",
            "myipaddr": env::var("BOB_TRADE_IP") .ok(),
            "rpcip": env::var("BOB_TRADE_IP") .ok(),
            "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
            "passphrase": bob_passphrase,
            "coins": coins,
            "rpc_password": "password",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "password".into(),
        None,
    )
    .unwrap();

    thread::sleep(Duration::from_secs(1));

    let mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 8999,
            "dht": "on",
            "myipaddr": env::var("ALICE_TRADE_IP") .ok(),
            "rpcip": env::var("ALICE_TRADE_IP") .ok(),
            "passphrase": alice_passphrase,
            "coins": coins,
            "seednodes": [mm_bob.my_seed_addr()],
            "rpc_password": "password",
            "skip_startup_checks": true,
        }),
        "password".into(),
        None,
    )
    .unwrap();

    thread::sleep(Duration::from_secs(1));

    dbg!(block_on(enable_tendermint(
        &mm_bob,
        "NUCLEUS-TEST",
        &[],
        NUCLEUS_TESTNET_RPC_URLS,
        false
    )));

    dbg!(block_on(enable_tendermint(
        &mm_alice,
        "NUCLEUS-TEST",
        &[],
        NUCLEUS_TESTNET_RPC_URLS,
        false
    )));

    dbg!(block_on(enable_electrum(&mm_bob, "DOC", false, DOC_ELECTRUM_ADDRS,)));

    dbg!(block_on(enable_electrum(&mm_alice, "DOC", false, DOC_ELECTRUM_ADDRS,)));

    block_on(trade_base_rel_tendermint(
        mm_bob,
        mm_alice,
        "NUCLEUS-TEST",
        "DOC",
        1,
        2,
        0.008,
    ));
}

#[test]
fn swap_nucleus_with_eth() {
    let _lock = SEQUENCE_LOCK.lock().unwrap();
    let bob_passphrase = String::from(BOB_PASSPHRASE);
    let alice_passphrase = String::from(ALICE_PASSPHRASE);
    const BOB_ETH_ADDRESS: &str = "0x7b338250f990954E3Ab034ccD32a917c2F607C2d";
    const ALICE_ETH_ADDRESS: &str = "0x37602b7a648b207ACFD19E67253f57669bEA4Ad8";

    fill_eth(
        Address::from_str(BOB_ETH_ADDRESS).unwrap(),
        U256::from(10).pow(U256::from(20)),
    );

    fill_eth(
        Address::from_str(ALICE_ETH_ADDRESS).unwrap(),
        U256::from(10).pow(U256::from(20)),
    );

    let coins = json!([nucleus_testnet_conf(), eth_dev_conf()]);

    let mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 8999,
            "dht": "on",
            "myipaddr": env::var("BOB_TRADE_IP") .ok(),
            "rpcip": env::var("BOB_TRADE_IP") .ok(),
            "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
            "passphrase": bob_passphrase,
            "coins": coins,
            "rpc_password": "password",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "password".into(),
        None,
    )
    .unwrap();

    thread::sleep(Duration::from_secs(1));

    let mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 8999,
            "dht": "on",
            "myipaddr": env::var("ALICE_TRADE_IP") .ok(),
            "rpcip": env::var("ALICE_TRADE_IP") .ok(),
            "passphrase": alice_passphrase,
            "coins": coins,
            "seednodes": [mm_bob.my_seed_addr()],
            "rpc_password": "password",
            "skip_startup_checks": true,
        }),
        "password".into(),
        None,
    )
    .unwrap();

    thread::sleep(Duration::from_secs(1));

    dbg!(block_on(enable_tendermint(
        &mm_bob,
        "NUCLEUS-TEST",
        &[],
        NUCLEUS_TESTNET_RPC_URLS,
        false
    )));

    dbg!(block_on(enable_tendermint(
        &mm_alice,
        "NUCLEUS-TEST",
        &[],
        NUCLEUS_TESTNET_RPC_URLS,
        false
    )));

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

    block_on(trade_base_rel_tendermint(
        mm_bob,
        mm_alice,
        "NUCLEUS-TEST",
        "ETH",
        1,
        2,
        0.008,
    ));
}

#[test]
fn swap_doc_with_iris_ibc_nucleus() {
    let _lock = SEQUENCE_LOCK.lock().unwrap();
    let bob_passphrase = String::from(BOB_PASSPHRASE);
    let alice_passphrase = String::from(ALICE_PASSPHRASE);

    let coins = json!([nucleus_testnet_conf(), iris_ibc_nucleus_testnet_conf(), doc_conf()]);

    let mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 8999,
            "dht": "on",
            "myipaddr": env::var("BOB_TRADE_IP") .ok(),
            "rpcip": env::var("BOB_TRADE_IP") .ok(),
            "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
            "passphrase": bob_passphrase,
            "coins": coins,
            "rpc_password": "password",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "password".into(),
        None,
    )
    .unwrap();

    thread::sleep(Duration::from_secs(1));

    let mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 8999,
            "dht": "on",
            "myipaddr": env::var("ALICE_TRADE_IP") .ok(),
            "rpcip": env::var("ALICE_TRADE_IP") .ok(),
            "passphrase": alice_passphrase,
            "coins": coins,
            "seednodes": [mm_bob.my_seed_addr()],
            "rpc_password": "password",
            "skip_startup_checks": true,
        }),
        "password".into(),
        None,
    )
    .unwrap();

    thread::sleep(Duration::from_secs(1));

    dbg!(block_on(enable_tendermint(
        &mm_bob,
        "NUCLEUS-TEST",
        &["IRIS-IBC-NUCLEUS-TEST"],
        NUCLEUS_TESTNET_RPC_URLS,
        false
    )));

    dbg!(block_on(enable_tendermint(
        &mm_alice,
        "NUCLEUS-TEST",
        &["IRIS-IBC-NUCLEUS-TEST"],
        NUCLEUS_TESTNET_RPC_URLS,
        false
    )));

    dbg!(block_on(enable_electrum(&mm_bob, "DOC", false, DOC_ELECTRUM_ADDRS)));

    dbg!(block_on(enable_electrum(&mm_alice, "DOC", false, DOC_ELECTRUM_ADDRS)));

    block_on(trade_base_rel_tendermint(
        mm_bob,
        mm_alice,
        "DOC",
        "IRIS-IBC-NUCLEUS-TEST",
        1,
        2,
        0.008,
    ));
}

pub async fn trade_base_rel_tendermint(
    mut mm_bob: MarketMakerIt,
    mut mm_alice: MarketMakerIt,
    base: &str,
    rel: &str,
    maker_price: i32,
    taker_price: i32,
    volume: f64,
) {
    log!("Issue bob {}/{} sell request", base, rel);
    let rc = mm_bob
        .rpc(&json!({
            "userpass": mm_bob.userpass,
            "method": "setprice",
            "base": base,
            "rel": rel,
            "price": maker_price,
            "volume": volume
        }))
        .await
        .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let mut uuids = vec![];

    common::log::info!(
        "Trigger alice subscription to {}/{} orderbook topic first and sleep for 1 second",
        base,
        rel
    );
    let rc = mm_alice
        .rpc(&json!({
            "userpass": mm_alice.userpass,
            "method": "orderbook",
            "base": base,
            "rel": rel,
        }))
        .await
        .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);
    Timer::sleep(1.).await;
    common::log::info!("Issue alice {}/{} buy request", base, rel);
    let rc = mm_alice
        .rpc(&json!({
            "userpass": mm_alice.userpass,
            "method": "buy",
            "base": base,
            "rel": rel,
            "volume": volume,
            "price": taker_price
        }))
        .await
        .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);
    let buy_json: serde_json::Value = serde_json::from_str(&rc.1).unwrap();
    uuids.push(buy_json["result"]["uuid"].as_str().unwrap().to_owned());

    // ensure the swaps are started
    let expected_log = format!("Entering the taker_swap_loop {base}/{rel}");
    mm_alice
        .wait_for_log(5., |log| log.contains(&expected_log))
        .await
        .unwrap();
    let expected_log = format!("Entering the maker_swap_loop {base}/{rel}");
    mm_bob
        .wait_for_log(5., |log| log.contains(&expected_log))
        .await
        .unwrap();

    for uuid in uuids.iter() {
        // ensure the swaps are indexed to the SQLite database
        let expected_log = format!("Inserting new swap {uuid} to the SQLite database");
        mm_alice
            .wait_for_log(5., |log| log.contains(&expected_log))
            .await
            .unwrap();
        mm_bob
            .wait_for_log(5., |log| log.contains(&expected_log))
            .await
            .unwrap()
    }

    for uuid in uuids.iter() {
        match mm_bob
            .wait_for_log(180., |log| log.contains(&format!("[swap uuid={uuid}] Finished")))
            .await
        {
            Ok(_) => (),
            Err(_) => {
                log!("{}", mm_bob.log_as_utf8().unwrap());
            },
        }

        match mm_alice
            .wait_for_log(180., |log| log.contains(&format!("[swap uuid={uuid}] Finished")))
            .await
        {
            Ok(_) => (),
            Err(_) => {
                log!("{}", mm_alice.log_as_utf8().unwrap());
            },
        }

        log!("Waiting a few second for the fresh swap status to be saved..");
        Timer::sleep(5.).await;

        log!("{}", mm_alice.log_as_utf8().unwrap());
        log!("Checking alice/taker status..");
        check_my_swap_status(
            &mm_alice,
            uuid,
            BigDecimal::try_from(volume).unwrap(),
            BigDecimal::try_from(volume).unwrap(),
        )
        .await;

        log!("{}", mm_bob.log_as_utf8().unwrap());
        log!("Checking bob/maker status..");
        check_my_swap_status(
            &mm_bob,
            uuid,
            BigDecimal::try_from(volume).unwrap(),
            BigDecimal::try_from(volume).unwrap(),
        )
        .await;
    }

    log!("Waiting 3 seconds for nodes to broadcast their swaps data..");
    Timer::sleep(3.).await;

    for uuid in uuids.iter() {
        log!("Checking alice status..");
        wait_check_stats_swap_status(&mm_alice, uuid, 240).await;

        log!("Checking bob status..");
        wait_check_stats_swap_status(&mm_bob, uuid, 240).await;
    }

    log!("Checking alice recent swaps..");
    check_recent_swaps(&mm_alice, uuids.len()).await;
    log!("Checking bob recent swaps..");
    check_recent_swaps(&mm_bob, uuids.len()).await;
    log!("Get {}/{} orderbook", base, rel);
    let rc = mm_bob
        .rpc(&json!({
            "userpass": mm_bob.userpass,
            "method": "orderbook",
            "base": base,
            "rel": rel,
        }))
        .await
        .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let bob_orderbook: OrderbookResponse = serde_json::from_str(&rc.1).unwrap();
    log!("{}/{} orderbook {:?}", base, rel, bob_orderbook);

    assert_eq!(0, bob_orderbook.bids.len(), "{base} {rel} bids must be empty");
    assert_eq!(0, bob_orderbook.asks.len(), "{base} {rel} asks must be empty");

    mm_bob.stop().await.unwrap();
    mm_alice.stop().await.unwrap();
}
