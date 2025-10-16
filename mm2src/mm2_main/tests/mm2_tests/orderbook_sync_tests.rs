use crate::integration_tests_common::{enable_coins_rick_morty_electrum, enable_electrum, enable_electrum_json};
use common::{block_on, log};
use http::StatusCode;
use mm2_main::lp_ordermatch::MIN_ORDER_KEEP_ALIVE_INTERVAL;
use mm2_number::{BigDecimal, BigRational, MmNumber};
use mm2_rpc::data::legacy::{AggregatedOrderbookEntry, CoinInitResponse, OrderbookResponse};
use mm2_test_helpers::electrums::doc_electrums;
use mm2_test_helpers::for_tests::{
    enable_z_coin_light, get_passphrase, morty_conf, orderbook_v2, rick_conf, set_price, zombie_conf, MarketMakerIt,
    Mm2TestConf, DOC_ELECTRUM_ADDRS, MARTY_ELECTRUM_ADDRS, RICK, ZOMBIE_ELECTRUMS, ZOMBIE_LIGHTWALLETD_URLS,
    ZOMBIE_TICKER,
};
use mm2_test_helpers::get_passphrase;
use mm2_test_helpers::structs::{GetPublicKeyResult, OrderbookV2Response, RpcV2Response, SetPriceResponse};
use serde_json::{self as json, json, Value as Json};
use std::env;
use std::str::FromStr;
use std::thread;
use std::time::Duration;

/// https://github.com/artemii235/SuperNET/issues/241
#[test]
fn alice_can_see_the_active_order_after_connection() {
    let coins = json!([
        {"coin":"RICK","asset":"RICK","rpcport":8923,"txversion":4,"overwintered":1,"protocol":{"type":"UTXO"}},
        {"coin":"MORTY","asset":"MORTY","rpcport":11608,"txversion":4,"overwintered":1,"protocol":{"type":"UTXO"}}
    ]);

    // start bob and immediately place the order
    let mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var("BOB_TRADE_IP") .ok(),
            "rpcip": env::var("BOB_TRADE_IP") .ok(),
            "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
            "passphrase": "bob passphrase",
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".into(),
        None,
    )
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());
    // Enable coins on Bob side. Print the replies in case we need the "address".
    log!(
        "enable_coins (bob): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_bob))
    );
    // issue sell request on Bob side by setting base/rel price
    log!("Issue bob sell request");
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "RICK",
        "rel": "MORTY",
        "price": 0.9,
        "volume": "0.9",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    // Bob orderbook must show the new order
    log!("Get RICK/MORTY orderbook on Bob side");
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "orderbook",
        "base": "RICK",
        "rel": "MORTY",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let bob_orderbook: OrderbookResponse = json::from_str(&rc.1).unwrap();
    log!("Bob orderbook {:?}", bob_orderbook);
    assert!(!bob_orderbook.asks.is_empty(), "Bob RICK/MORTY asks are empty");
    assert_eq!(
        BigDecimal::from_str("0.9").unwrap(),
        bob_orderbook.asks[0].entry.max_volume
    );

    // start eve and immediately place the order
    let mm_eve = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var("BOB_TRADE_IP") .ok(),
            "rpcip": env::var("BOB_TRADE_IP") .ok(),
            "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
            "passphrase": "eve passphrase",
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": [mm_bob.ip.to_string()],
        }),
        "pass".into(),
        None,
    )
    .unwrap();
    let (_eve_dump_log, _eve_dump_dashboard) = mm_eve.mm_dump();
    log!("Eve log path: {}", mm_eve.log_path.display());
    // Enable coins on Eve side. Print the replies in case we need the "address".
    log!(
        "enable_coins (eve): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_eve))
    );
    // issue sell request on Eve side by setting base/rel price
    log!("Issue eve sell request");
    let rc = block_on(mm_eve.rpc(&json!({
        "userpass": mm_eve.userpass,
        "method": "setprice",
        "base": "RICK",
        "rel": "MORTY",
        "price": "1",
        "volume": "0.9",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    // issue sell request on Eve side by setting base/rel price
    log!("Issue eve sell request");
    let rc = block_on(mm_eve.rpc(&json!({
        "userpass": mm_eve.userpass,
        "method": "setprice",
        "base": "MORTY",
        "rel": "RICK",
        "price": "1",
        "volume": "0.9",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    log!("Get RICK/MORTY orderbook on Eve side");
    let rc = block_on(mm_eve.rpc(&json!({
        "userpass": mm_eve.userpass,
        "method": "orderbook",
        "base": "RICK",
        "rel": "MORTY",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let eve_orderbook: OrderbookResponse = json::from_str(&rc.1).unwrap();
    log!("Eve orderbook {:?}", eve_orderbook);
    assert_eq!(
        eve_orderbook.asks.len(),
        2,
        "Eve RICK/MORTY orderbook must have exactly 2 asks"
    );
    assert_eq!(
        eve_orderbook.bids.len(),
        1,
        "Eve RICK/MORTY orderbook must have exactly 1 bid"
    );

    log!("Give Bob 2 seconds to import Eve order");
    thread::sleep(Duration::from_secs(2));
    log!("Get RICK/MORTY orderbook on Bob side");
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "orderbook",
        "base": "RICK",
        "rel": "MORTY",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let bob_orderbook: OrderbookResponse = json::from_str(&rc.1).unwrap();
    log!("Bob orderbook {:?}", bob_orderbook);
    assert_eq!(
        bob_orderbook.asks.len(),
        2,
        "Bob RICK/MORTY orderbook must have exactly 2 asks"
    );
    assert_eq!(
        bob_orderbook.bids.len(),
        1,
        "Bob RICK/MORTY orderbook must have exactly 1 bid"
    );

    let mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var("ALICE_TRADE_IP") .ok(),
            "rpcip": env::var("ALICE_TRADE_IP") .ok(),
            "passphrase": "alice passphrase",
            "coins": coins,
            "seednodes": [mm_bob.ip.to_string()],
            "rpc_password": "pass",
        }),
        "pass".into(),
        None,
    )
    .unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    // Enable coins on Alice side. Print the replies in case we need the "address".
    log!(
        "enable_coins (alice): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_alice))
    );

    log!("Get RICK/MORTY orderbook on Alice side");
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "RICK",
        "rel": "MORTY",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let alice_orderbook: OrderbookResponse = json::from_str(&rc.1).unwrap();
    log!("Alice orderbook {:?}", alice_orderbook);
    assert_eq!(
        alice_orderbook.asks.len(),
        2,
        "Alice RICK/MORTY orderbook must have exactly 2 asks"
    );
    assert_eq!(
        alice_orderbook.bids.len(),
        1,
        "Alice RICK/MORTY orderbook must have exactly 1 bid"
    );

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
    block_on(mm_eve.stop()).unwrap();
}

#[test]
fn alice_can_see_the_active_order_after_orderbook_sync_segwit() {
    let bob_passphrase = get_passphrase(&".env.seed", "BOB_PASSPHRASE").unwrap();

    let bob_coins_config = json!([
        {"coin":"RICK","asset":"RICK","rpcport":8923,"txversion":4,"overwintered":1,"protocol":{"type":"UTXO"}},
        {"coin":"tBTC","name":"tbitcoin","fname":"tBitcoin","rpcport":18332,"pubtype":111,"p2shtype":196,"wiftype":239,"segwit":true,"bech32_hrp":"tb","txfee":1000,"mm2":1,"required_confirmations":0,"protocol":{"type":"UTXO"},"address_format":{"format":"segwit"}}
    ]);

    let alice_coins_config = json!([
        {"coin":"RICK","asset":"RICK","rpcport":8923,"txversion":4,"overwintered":1,"protocol":{"type":"UTXO"}},
        {"coin":"tBTC","name":"tbitcoin","fname":"tBitcoin","rpcport":18332,"pubtype":111,"p2shtype":196,"wiftype":239,"segwit":true,"bech32_hrp":"tb","txfee":1000,"mm2":1,"required_confirmations":0,"protocol":{"type":"UTXO"},"address_format":{"format":"segwit"}}
    ]);

    let mut mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var("BOB_TRADE_IP") .ok(),
            "rpcip": env::var("BOB_TRADE_IP") .ok(),
            "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
            "passphrase": bob_passphrase,
            "coins": bob_coins_config,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".into(),
        None,
    )
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    // Enable coins on Bob side. Print the replies in case we need the "address".
    let electrum = block_on(mm_bob.rpc(&json!({
        "userpass": "pass",
        "method": "electrum",
        "coin": "tBTC",
        "servers": [{"url":"electrum1.cipig.net:10068"},{"url":"electrum2.cipig.net:10068"},{"url":"electrum3.cipig.net:10068"}],
        "address_format":{"format":"segwit"},
        "mm2": 1,
    }))).unwrap();
    assert_eq!(
        electrum.0,
        StatusCode::OK,
        "RPC «electrum» failed with {} {}",
        electrum.0,
        electrum.1
    );
    log!("enable tBTC: {:?}", electrum);
    let enable_tbtc_res: CoinInitResponse = json::from_str(&electrum.1).unwrap();
    let tbtc_segwit_address = enable_tbtc_res.address;

    let enable_rick_res = block_on(enable_electrum(&mm_bob, "RICK", false, DOC_ELECTRUM_ADDRS));
    log!("enable RICK: {:?}", enable_rick_res);
    let rick_address = enable_rick_res.address;

    // issue sell request on Bob side by setting base/rel price
    log!("Issue bob sell requests");

    let bob_orders = [
        // (base, rel, price, volume, min_volume)
        ("tBTC", "RICK", "0.7", "0.0002", Some("0.00015")),
        ("RICK", "tBTC", "0.7", "0.0002", Some("0.00015")),
    ];
    for (base, rel, price, volume, min_volume) in bob_orders.iter() {
        let rc = block_on(mm_bob.rpc(&json!({
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

    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "mmrpc": "2.0",
        "method": "get_public_key",
        "params": {},
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!get_public_key: {}", rc.1);
    let get_public_key_res: RpcV2Response<GetPublicKeyResult> = serde_json::from_str(&rc.1).unwrap();
    let bob_pubkey = get_public_key_res.result.public_key;

    let mut mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var("ALICE_TRADE_IP") .ok(),
            "rpcip": env::var("ALICE_TRADE_IP") .ok(),
            "passphrase": "alice passphrase",
            "coins": alice_coins_config,
            "seednodes": [mm_bob.ip.to_string()],
            "rpc_password": "pass",
        }),
        "pass".into(),
        None,
    )
    .unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    block_on(mm_bob.wait_for_log(22., |log| {
        log.contains("DEBUG Handling IncludedTorelaysMesh message for peer")
    }))
    .unwrap();

    let electrum = block_on(mm_alice.rpc(&json!({
        "userpass": "pass",
        "method": "electrum",
        "coin": "tBTC",
        "servers": [{"url":"electrum1.cipig.net:10068"},{"url":"electrum2.cipig.net:10068"},{"url":"electrum3.cipig.net:10068"}],
        "address_format":{"format":"segwit"},
        "mm2": 1,
    }))).unwrap();
    assert_eq!(
        electrum.0,
        StatusCode::OK,
        "RPC «electrum» failed with {} {}",
        electrum.0,
        electrum.1
    );
    log!("enable Alice tBTC: {:?}", electrum);

    let electrum = block_on(enable_electrum(&mm_alice, "RICK", false, DOC_ELECTRUM_ADDRS));
    log!("enable Alice RICK: {:?}", electrum);

    // setting the price will trigger Alice's subscription to the orderbook topic
    // but won't request the actual orderbook
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "setprice",
        "base": "RICK",
        "rel": "tBTC",
        "price": "1",
        "volume": "0.1",
        "cancel_previous": false,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    block_on(
        mm_alice.wait_for_log((MIN_ORDER_KEEP_ALIVE_INTERVAL * 2) as f64, |log| {
            log.contains(&format!("Inserting order OrderbookItem {{ pubkey: \"{bob_pubkey}\""))
        }),
    )
    .unwrap();

    // checking orderbook on alice side
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "tBTC",
        "rel": "RICK",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);
    let response: OrderbookResponse = json::from_str(&rc.1).unwrap();
    assert_eq!(response.asks[0].entry.address, tbtc_segwit_address);
    assert_eq!(response.bids[0].entry.address, rick_address);

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_orderbook_segwit() {
    let bob_passphrase = get_passphrase(&".env.seed", "BOB_PASSPHRASE").unwrap();

    let bob_coins_config = json!([
        {"coin":"RICK","asset":"RICK","rpcport":8923,"txversion":4,"overwintered":1,"protocol":{"type":"UTXO"}},
        {"coin":"tBTC","name":"tbitcoin","fname":"tBitcoin","rpcport":18332,"pubtype":111,"p2shtype":196,"wiftype":239,"segwit":true,"bech32_hrp":"tb","txfee":1000,"mm2":1,"required_confirmations":0,"protocol":{"type":"UTXO"},"address_format":{"format":"segwit"}}
    ]);

    let alice_coins_config = json!([
        {"coin":"RICK","asset":"RICK","rpcport":8923,"txversion":4,"overwintered":1,"protocol":{"type":"UTXO"}},
        {"coin":"tBTC","name":"tbitcoin","fname":"tBitcoin","rpcport":18332,"pubtype":111,"p2shtype":196,"wiftype":239,"segwit":true,"bech32_hrp":"tb","txfee":1000,"mm2":1,"required_confirmations":0,"protocol":{"type":"UTXO"}}
    ]);

    let mut mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var("BOB_TRADE_IP") .ok(),
            "rpcip": env::var("BOB_TRADE_IP") .ok(),
            "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
            "passphrase": bob_passphrase,
            "coins": bob_coins_config,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".into(),
        None,
    )
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    // Enable coins on Bob side. Print the replies in case we need the "address".
    let electrum = block_on(mm_bob.rpc(&json!({
        "userpass": "pass",
        "method": "electrum",
        "coin": "tBTC",
        "servers": [{"url":"electrum1.cipig.net:10068"},{"url":"electrum2.cipig.net:10068"},{"url":"electrum3.cipig.net:10068"}],
        "address_format":{"format":"segwit"},
        "mm2": 1,
    }))).unwrap();
    assert_eq!(
        electrum.0,
        StatusCode::OK,
        "RPC «electrum» failed with {} {}",
        electrum.0,
        electrum.1
    );
    log!("enable tBTC: {:?}", electrum);
    let enable_tbtc_res: CoinInitResponse = json::from_str(&electrum.1).unwrap();
    let tbtc_segwit_address = enable_tbtc_res.address;

    let enable_rick_res = block_on(enable_electrum(&mm_bob, "RICK", false, DOC_ELECTRUM_ADDRS));
    log!("enable RICK: {:?}", enable_rick_res);
    let rick_address = enable_rick_res.address;

    // issue sell request on Bob side by setting base/rel price
    log!("Issue bob sell requests");

    let bob_orders = [
        // (base, rel, price, volume, min_volume)
        ("tBTC", "RICK", "0.7", "0.0002", Some("0.00015")),
        ("RICK", "tBTC", "0.7", "0.0002", Some("0.00015")),
    ];
    for (base, rel, price, volume, min_volume) in bob_orders.iter() {
        let rc = block_on(mm_bob.rpc(&json!({
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

    let mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var("ALICE_TRADE_IP") .ok(),
            "rpcip": env::var("ALICE_TRADE_IP") .ok(),
            "passphrase": "alice passphrase",
            "coins": alice_coins_config,
            "seednodes": [mm_bob.ip.to_string()],
            "rpc_password": "pass",
        }),
        "pass".into(),
        None,
    )
    .unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    block_on(mm_bob.wait_for_log(22., |log| {
        log.contains("DEBUG Handling IncludedTorelaysMesh message for peer")
    }))
    .unwrap();

    // checking orderbook on alice side
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "tBTC",
        "rel": "RICK",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);
    let response: OrderbookResponse = json::from_str(&rc.1).unwrap();
    assert_eq!(response.asks[0].entry.address, tbtc_segwit_address);
    assert_eq!(response.bids[0].entry.address, rick_address);

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_get_orderbook_with_same_orderbook_ticker() {
    let coins = json!([
        {"coin":"RICK","asset":"RICK","rpcport":8923,"txversion":4,"protocol":{"type":"UTXO"}},
        {"coin":"RICK-Utxo","asset":"RICK","orderbook_ticker":"RICK","rpcport":8923,"txversion":4,"protocol":{"type":"UTXO"}},
        // just a random contract address
        {"coin":"RICK-ERC20","orderbook_ticker":"RICK","decimals": 18,"protocol":{"type":"ERC20","protocol_data":{"platform":"ETH","contract_address":"0x7Fc66500c84A76Ad7e9c93437bFc5Ac33E2DDaE9"}}},
    ]);

    let mm = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "passphrase": "bob passphrase",
            "rpc_password": "password",
            "coins": coins,
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "password".into(),
        None,
    )
    .unwrap();
    let (_dump_log, _dump_dashboard) = mm.mm_dump();
    log!("Log path: {}", mm.log_path.display());

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "orderbook",
        "base": "RICK",
        "rel": "RICK-Utxo",
    })))
    .unwrap();
    assert!(
        rc.0.is_server_error(),
        "orderbook succeed but should have failed {}",
        rc.1
    );

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "orderbook",
        "base": "RICK",
        "rel": "RICK-ERC20",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook {}", rc.1);
}

#[test]
fn test_conf_settings_in_orderbook() {
    let coins = json!([
        {"coin":"RICK","asset":"RICK","rpcport":8923,"txversion":4,"overwintered":1,"required_confirmations":10,"requires_notarization":true,"protocol":{"type":"UTXO"}},
        {"coin":"MORTY","asset":"MORTY","rpcport":11608,"txversion":4,"overwintered":1,"required_confirmations":5,"requires_notarization":false,"protocol":{"type":"UTXO"}},
    ]);

    let mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "passphrase": "bob passphrase",
            "rpc_password": "password",
            "coins": coins,
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "password".into(),
        None,
    )
    .unwrap();
    let (_dump_log, _dump_dashboard) = mm_bob.mm_dump();
    log!("Log path: {}", mm_bob.log_path.display());

    log!(
        "enable_coins (bob): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_bob))
    );

    log!("Issue set_price request for RICK/MORTY on Bob side");
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "RICK",
        "rel": "MORTY",
        "price": 0.9,
        "volume": "0.9",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    log!("Issue set_price request for MORTY/RICK on Bob side");
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MORTY",
        "rel": "RICK",
        "price": 0.9,
        "volume": "0.9",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "passphrase": "alice passphrase",
            "rpc_password": "password",
            "coins": coins,
            "seednodes": [mm_bob.ip.to_string()],
        }),
        "password".into(),
        None,
    )
    .unwrap();
    let (_dump_log, _dump_dashboard) = mm_alice.mm_dump();
    log!("Log path: {}", mm_alice.log_path.display());

    log!(
        "enable_coins (alice): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_alice))
    );

    log!("Get RICK/MORTY orderbook on Alice side");
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "RICK",
        "rel": "MORTY",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let alice_orderbook: OrderbookResponse = json::from_str(&rc.1).unwrap();
    log!("Alice orderbook {:?}", alice_orderbook);

    assert_eq!(
        alice_orderbook.asks.len(),
        1,
        "Alice RICK/MORTY orderbook must have exactly 1 ask"
    );
    assert_eq!(
        alice_orderbook.asks[0].entry.conf_settings.as_ref().unwrap().base_confs,
        10
    );
    assert!(alice_orderbook.asks[0].entry.conf_settings.as_ref().unwrap().base_nota);
    assert_eq!(
        alice_orderbook.asks[0].entry.conf_settings.as_ref().unwrap().rel_confs,
        5
    );
    assert!(!alice_orderbook.asks[0].entry.conf_settings.as_ref().unwrap().rel_nota);

    assert_eq!(
        alice_orderbook.bids.len(),
        1,
        "Alice RICK/MORTY orderbook must have exactly 1 bid"
    );
    assert_eq!(
        alice_orderbook.bids[0].entry.conf_settings.as_ref().unwrap().base_confs,
        10
    );
    assert!(alice_orderbook.bids[0].entry.conf_settings.as_ref().unwrap().base_nota);
    assert_eq!(
        alice_orderbook.bids[0].entry.conf_settings.as_ref().unwrap().rel_confs,
        5
    );
    assert!(!alice_orderbook.bids[0].entry.conf_settings.as_ref().unwrap().rel_nota);

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn alice_can_see_confs_in_orderbook_after_sync() {
    let bob_coins = json!([
        {"coin":"RICK","asset":"RICK","rpcport":8923,"txversion":4,"overwintered":1,"required_confirmations":10,"requires_notarization":true,"protocol":{"type":"UTXO"}},
        {"coin":"MORTY","asset":"MORTY","rpcport":11608,"txversion":4,"overwintered":1,"required_confirmations":5,"requires_notarization":false,"protocol":{"type":"UTXO"}},
    ]);

    let mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "passphrase": "bob passphrase",
            "rpc_password": "password",
            "coins": bob_coins,
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "password".into(),
        None,
    )
    .unwrap();
    // let (_dump_log, _dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    log!(
        "enable_coins (bob): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_bob))
    );

    log!("Issue sell request on Bob side");
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "RICK",
        "rel": "MORTY",
        "price": 0.9,
        "volume": "0.9",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "mmrpc": "2.0",
        "method": "get_public_key",
        "params": {},
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!get_public_key: {}", rc.1);
    let get_public_key_res: RpcV2Response<GetPublicKeyResult> = serde_json::from_str(&rc.1).unwrap();
    let bob_pubkey = get_public_key_res.result.public_key;

    // Alice coins don't have required_confirmations and requires_notarization set
    let alice_coins = json!([
        {"coin":"RICK","asset":"RICK","rpcport":8923,"txversion":4,"overwintered":1,"protocol":{"type":"UTXO"}},
        {"coin":"MORTY","asset":"MORTY","rpcport":11608,"txversion":4,"overwintered":1,"protocol":{"type":"UTXO"}},
    ]);

    let mut mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "passphrase": "alice passphrase",
            "rpc_password": "password",
            "coins": alice_coins,
            "seednodes": [mm_bob.ip.to_string()],
        }),
        "password".into(),
        None,
    )
    .unwrap();
    let (_dump_log, _dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    log!(
        "enable_coins (alice): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_alice))
    );

    // setting the price will trigger Alice's subscription to the orderbook topic
    // but won't request the actual orderbook
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "setprice",
        "base": "RICK",
        "rel": "MORTY",
        "price": "1",
        "volume": "0.1",
        "cancel_previous": false,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    block_on(
        mm_alice.wait_for_log((MIN_ORDER_KEEP_ALIVE_INTERVAL * 2) as f64, |log| {
            log.contains(&format!("Inserting order OrderbookItem {{ pubkey: \"{bob_pubkey}\""))
        }),
    )
    .unwrap();

    log!("Get RICK/MORTY orderbook on Alice side");
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "RICK",
        "rel": "MORTY",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let alice_orderbook: OrderbookResponse = json::from_str(&rc.1).unwrap();
    log!("Alice orderbook {:?}", alice_orderbook);
    assert_eq!(
        alice_orderbook.asks.len(),
        2,
        "Alice RICK/MORTY orderbook must have exactly 2 ask"
    );
    let bob_order_in_orderbook = alice_orderbook
        .asks
        .iter()
        .find(|entry| entry.entry.pubkey == bob_pubkey)
        .unwrap();
    assert_eq!(
        bob_order_in_orderbook.entry.conf_settings.as_ref().unwrap().base_confs,
        10
    );
    assert!(bob_order_in_orderbook.entry.conf_settings.as_ref().unwrap().base_nota);
    assert_eq!(
        bob_order_in_orderbook.entry.conf_settings.as_ref().unwrap().rel_confs,
        5
    );
    assert!(!bob_order_in_orderbook.entry.conf_settings.as_ref().unwrap().rel_nota);

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/859
fn orderbook_extended_data() {
    let coins = json!([
        {"coin":"RICK","asset":"RICK","protocol":{"type":"UTXO"}},
        {"coin":"MORTY","asset":"MORTY","protocol":{"type":"UTXO"}},
    ]);

    let mm = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var("BOB_TRADE_IP") .ok(),
            "rpcip": env::var("BOB_TRADE_IP") .ok(),
            "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
            "passphrase": "bob passphrase",
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".into(),
        None,
    )
    .unwrap();
    let (_dump_log, _dump_dashboard) = &mm.mm_dump();
    log!("Log path: {}", mm.log_path.display());
    block_on(enable_electrum(&mm, "RICK", false, DOC_ELECTRUM_ADDRS));
    block_on(enable_electrum(&mm, "MORTY", false, MARTY_ELECTRUM_ADDRS));

    let bob_orders = &[
        // (base, rel, price, volume)
        ("RICK", "MORTY", "0.9", "0.9"),
        ("RICK", "MORTY", "0.8", "0.9"),
        ("RICK", "MORTY", "0.7", "0.9"),
        ("MORTY", "RICK", "0.8", "0.9"),
        ("MORTY", "RICK", "1", "0.9"),
    ];

    for (base, rel, price, volume) in bob_orders {
        let rc = block_on(mm.rpc(&json!({
            "userpass": mm.userpass,
            "method": "setprice",
            "base": base,
            "rel": rel,
            "price": price,
            "volume": volume,
            "cancel_previous": false,
        })))
        .unwrap();
        assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    }

    thread::sleep(Duration::from_secs(1));
    log!("Get RICK/MORTY orderbook");
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "orderbook",
        "base": "RICK",
        "rel": "MORTY",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let orderbook: OrderbookResponse = json::from_str(&rc.1).unwrap();
    log!("orderbook {:?}", rc.1);
    let expected_total_asks_base_vol = MmNumber::from("2.7");
    assert_eq!(
        expected_total_asks_base_vol.to_decimal(),
        orderbook.total_asks_base.total_asks_base_vol
    );

    let expected_total_bids_base_vol = MmNumber::from("1.62");
    assert_eq!(
        expected_total_bids_base_vol.to_decimal(),
        orderbook.total_bids_base.total_bids_base_vol
    );

    let expected_total_asks_rel_vol = MmNumber::from("2.16");
    assert_eq!(
        expected_total_asks_rel_vol.to_decimal(),
        orderbook.total_asks_rel.total_asks_rel_vol
    );

    let expected_total_bids_rel_vol = MmNumber::from("1.8");
    assert_eq!(
        expected_total_bids_rel_vol.to_decimal(),
        orderbook.total_bids_rel.total_bids_rel_vol
    );

    fn check_price_and_vol_aggr(
        order: &AggregatedOrderbookEntry,
        price: &'static str,
        base_aggr: &'static str,
        rel_aggr: &'static str,
    ) {
        let price = MmNumber::from(price);
        assert_eq!(price.to_decimal(), order.entry.price);

        let base_aggr = MmNumber::from(base_aggr);
        assert_eq!(base_aggr.to_decimal(), order.base_max_volume_aggr.base_max_volume_aggr);

        let rel_aggr = MmNumber::from(rel_aggr);
        assert_eq!(rel_aggr.to_decimal(), order.rel_max_volume_aggr.rel_max_volume_aggr);
    }

    check_price_and_vol_aggr(&orderbook.asks[0], "0.9", "2.7", "2.16");
    check_price_and_vol_aggr(&orderbook.asks[1], "0.8", "1.8", "1.35");
    check_price_and_vol_aggr(&orderbook.asks[2], "0.7", "0.9", "0.63");

    check_price_and_vol_aggr(&orderbook.bids[0], "1.25", "0.72", "0.9");
    check_price_and_vol_aggr(&orderbook.bids[1], "1", "1.62", "1.8");
}

#[test]
fn orderbook_should_display_base_rel_volumes() {
    let coins = json!([
        {"coin":"RICK","asset":"RICK","protocol":{"type":"UTXO"}},
        {"coin":"MORTY","asset":"MORTY","protocol":{"type":"UTXO"}},
    ]);

    let mm = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var("BOB_TRADE_IP") .ok(),
            "rpcip": env::var("BOB_TRADE_IP") .ok(),
            "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
            "passphrase": "bob passphrase",
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".into(),
        None,
    )
    .unwrap();
    let (_dump_log, _dump_dashboard) = &mm.mm_dump();
    log!("Log path: {}", mm.log_path.display());
    block_on(enable_electrum(&mm, "RICK", false, DOC_ELECTRUM_ADDRS));
    block_on(enable_electrum(&mm, "MORTY", false, MARTY_ELECTRUM_ADDRS));

    let price = BigRational::new(2.into(), 1.into());
    let volume = BigRational::new(1.into(), 1.into());

    // create order with rational amount and price
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": "RICK",
        "rel": "MORTY",
        "price": price,
        "volume": volume,
        "cancel_previous": false,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    thread::sleep(Duration::from_secs(1));
    log!("Get RICK/MORTY orderbook");
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "orderbook",
        "base": "RICK",
        "rel": "MORTY",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let orderbook: OrderbookResponse = json::from_str(&rc.1).unwrap();
    log!("orderbook {:?}", orderbook);
    assert_eq!(orderbook.asks.len(), 1, "RICK/MORTY orderbook must have exactly 1 ask");
    let min_volume = BigRational::new(1.into(), 10000.into());
    assert_eq!(volume, orderbook.asks[0].entry.base_max_volume.base_max_volume_rat);
    assert_eq!(min_volume, orderbook.asks[0].entry.base_min_volume.base_min_volume_rat);

    assert_eq!(
        &volume * &price,
        orderbook.asks[0].entry.rel_max_volume.rel_max_volume_rat
    );
    assert_eq!(
        &min_volume * &price,
        orderbook.asks[0].entry.rel_min_volume.rel_min_volume_rat
    );

    log!("Get MORTY/RICK orderbook");
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "orderbook",
        "base": "MORTY",
        "rel": "RICK",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let orderbook: OrderbookResponse = json::from_str(&rc.1).unwrap();
    log!("orderbook {:?}", orderbook);
    assert_eq!(orderbook.bids.len(), 1, "MORTY/RICK orderbook must have exactly 1 bid");
    let min_volume = BigRational::new(1.into(), 10000.into());
    assert_eq!(volume, orderbook.bids[0].entry.rel_max_volume.rel_max_volume_rat);
    assert_eq!(min_volume, orderbook.bids[0].entry.rel_min_volume.rel_min_volume_rat);

    assert_eq!(
        &volume * &price,
        orderbook.bids[0].entry.base_max_volume.base_max_volume_rat
    );
    assert_eq!(
        &min_volume * &price,
        orderbook.bids[0].entry.base_min_volume.base_min_volume_rat
    );
}

#[test]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/670
fn orderbook_should_work_without_coins_activation() {
    let bob_passphrase = get_passphrase(&".env.seed", "BOB_PASSPHRASE").unwrap();

    let coins = json!([rick_conf(), morty_conf()]);

    let mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var("BOB_TRADE_IP") .ok(),
            "rpcip": env::var("BOB_TRADE_IP") .ok(),
            "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
            "passphrase": bob_passphrase,
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".into(),
        None,
    )
    .unwrap();

    let (_dump_log, _dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    let mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "dht": "on",  // Enable DHT without delay.
            "myipaddr": env::var("ALICE_TRADE_IP") .ok(),
            "rpcip": env::var("ALICE_TRADE_IP") .ok(),
            "passphrase": "alice passphrase",
            "coins": coins,
            "seednodes": [mm_bob.ip.to_string()],
            "rpc_password": "pass",
        }),
        "pass".into(),
        None,
    )
    .unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    let electrum = block_on(enable_electrum(&mm_bob, "RICK", false, DOC_ELECTRUM_ADDRS));
    log!("enable RICK on Bob: {:?}", electrum);
    let electrum = block_on(enable_electrum(&mm_bob, "MORTY", false, MARTY_ELECTRUM_ADDRS));
    log!("enable MORTY on Bob: {:?}", electrum);

    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MORTY",
        "rel": "RICK",
        "price": "1",
        "volume": "5",
        "min_volume": "1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    log!("Get MORTY/RICK orderbook on Alice side");
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "MORTY",
        "rel": "RICK",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let orderbook: Json = json::from_str(&rc.1).unwrap();
    log!("orderbook {:?}", orderbook);
    let asks = orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 1, "Alice MORTY/RICK orderbook must have exactly 1 ask");
}

#[test]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/511
fn test_all_orders_per_pair_per_node_must_be_displayed_in_orderbook() {
    let coins = json!([
        {"coin":"RICK","asset":"RICK","protocol":{"type":"UTXO"}},
        {"coin":"MORTY","asset":"MORTY","protocol":{"type":"UTXO"}},
    ]);

    let mm = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var("BOB_TRADE_IP") .ok(),
            "rpcip": env::var("BOB_TRADE_IP") .ok(),
            "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
            "passphrase": "bob passphrase",
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".into(),
        None,
    )
    .unwrap();
    let (_dump_log, _dump_dashboard) = mm.mm_dump();
    log!("Log path: {}", mm.log_path.display());
    block_on(enable_electrum(&mm, "RICK", false, DOC_ELECTRUM_ADDRS));
    block_on(enable_electrum(&mm, "MORTY", false, MARTY_ELECTRUM_ADDRS));

    // set 2 orders with different prices
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": "RICK",
        "rel": "MORTY",
        "price": 0.9,
        "volume": "0.9",
        "cancel_previous": false,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": "RICK",
        "rel": "MORTY",
        "price": 1,
        "volume": "0.9",
        "cancel_previous": false,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    thread::sleep(Duration::from_secs(2));

    log!("Get RICK/MORTY orderbook");
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "orderbook",
        "base": "RICK",
        "rel": "MORTY",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let orderbook: Json = json::from_str(&rc.1).unwrap();
    log!("orderbook {:?}", orderbook);
    let asks = orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 2, "RICK/MORTY orderbook must have exactly 2 asks");
}

#[test]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/473
fn setprice_min_volume_should_be_displayed_in_orderbook() {
    let bob_passphrase = get_passphrase(&".env.seed", "BOB_PASSPHRASE").unwrap();

    let coins = json!([rick_conf(), morty_conf()]);

    let mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var("BOB_TRADE_IP") .ok(),
            "rpcip": env::var("BOB_TRADE_IP") .ok(),
            "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
            "passphrase": bob_passphrase,
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".into(),
        None,
    )
    .unwrap();

    let (_dump_log, _dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    let mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "dht": "on",  // Enable DHT without delay.
            "myipaddr": env::var("ALICE_TRADE_IP") .ok(),
            "rpcip": env::var("ALICE_TRADE_IP") .ok(),
            "passphrase": "alice passphrase",
            "coins": coins,
            "seednodes": [mm_bob.ip.to_string()],
            "rpc_password": "pass",
        }),
        "pass".into(),
        None,
    )
    .unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    let electrum = block_on(enable_electrum(&mm_bob, "RICK", false, DOC_ELECTRUM_ADDRS));
    log!("enable RICK on Bob: {:?}", electrum);
    let electrum = block_on(enable_electrum(&mm_bob, "MORTY", false, MARTY_ELECTRUM_ADDRS));
    log!("enable MORTY on Bob: {:?}", electrum);

    let electrum = block_on(enable_electrum(&mm_alice, "RICK", false, DOC_ELECTRUM_ADDRS));
    log!("enable RICK on Alice: {:?}", electrum);
    let electrum = block_on(enable_electrum(&mm_alice, "MORTY", false, MARTY_ELECTRUM_ADDRS));
    log!("enable MORTY on Alice: {:?}", electrum);

    // issue orderbook call on Alice side to trigger subscription to a topic
    block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "MORTY",
        "rel": "RICK",
    })))
    .unwrap();

    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MORTY",
        "rel": "RICK",
        "price": "1",
        "volume": "5",
        "min_volume": "1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    thread::sleep(Duration::from_secs(2));
    log!("Get MORTY/RICK orderbook on Bob side");
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "orderbook",
        "base": "MORTY",
        "rel": "RICK",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let orderbook: Json = json::from_str(&rc.1).unwrap();
    log!("orderbook {:?}", orderbook);
    let asks = orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 1, "Bob MORTY/RICK orderbook must have exactly 1 ask");

    let min_volume = asks[0]["min_volume"].as_str().unwrap();
    assert_eq!(min_volume, "1", "Bob MORTY/RICK ask must display correct min_volume");

    log!("Get MORTY/RICK orderbook on Alice side");
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "MORTY",
        "rel": "RICK",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    let orderbook: Json = json::from_str(&rc.1).unwrap();
    log!("orderbook {:?}", orderbook);
    let asks = orderbook["asks"].as_array().unwrap();
    assert_eq!(asks.len(), 1, "Alice MORTY/RICK orderbook must have exactly 1 ask");

    let min_volume = asks[0]["min_volume"].as_str().unwrap();
    assert_eq!(min_volume, "1", "Alice MORTY/RICK ask must display correct min_volume");
}

/// Test does not work as Alice receives order with PubkeyKeepAlive message (not with MakerOrderCreated)
#[ignore = "not working due to protocol change"]
#[test]
fn test_order_cancellation_received_before_creation() {
    let coins = json!([rick_conf(), morty_conf()]);

    let bob_passphrase = "bob passphrase";
    let mm_bob_conf = Mm2TestConf::seednode(bob_passphrase, &coins);
    let mm_bob = MarketMakerIt::start(mm_bob_conf.conf, mm_bob_conf.rpc_password, None).unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());
    log!(
        "enable_coins (bob): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_bob))
    );

    // Bob places maker order before Alice connects to the network so that Alice receives the order creation through IHAVE/IWANT messages.
    let set_price = block_on(set_price(&mm_bob, "RICK", "MORTY", "0.9", "0.9", false, None));

    let mm_alice_conf = Mm2TestConf::light_node("alice passphrase", &coins, &[&mm_bob.ip.to_string()]);
    let mut mm_alice = MarketMakerIt::start(mm_alice_conf.conf, mm_alice_conf.rpc_password, None).unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());
    log!(
        "enable_coins (alice): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_alice))
    );

    // Alice requests the orderbook to subscribe to the topic
    let orderbook = block_on(orderbook_v2(&mm_alice, "RICK", "MORTY"));
    let asks = orderbook["result"]["asks"].as_array().unwrap();
    // Alice should see the order in the orderbook as she got it through `GetOrderbook` p2p request.
    assert_eq!(asks.len(), 1, "Alice RICK/MORTY orderbook must have exactly 1 ask");

    // Bob cancels the order straight after Alice subscribes to the orderbook topic
    // so that Alice receives the cancellation message faster than the creation message
    // that she will receive later through IHAVE/IWANT messages.
    let cancel_orders = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "cancel_all_orders",
        "cancel_by": {
            "type": "All",
        }
    })))
    .unwrap();
    assert!(cancel_orders.0.is_success(), "!cancel_all_orders: {}", cancel_orders.1);

    // Make sure Alice receives the order cancellation message.
    block_on(mm_alice.wait_for_log(10., |log| {
        log.contains("received ordermatch message MakerOrderCancelled")
    }))
    .unwrap();

    // Make sure Alice receives the order creation message.
    block_on(mm_alice.wait_for_log(10., |log| log.contains("received ordermatch message MakerOrderCreated"))).unwrap();

    // Make sure Alice ignores inserting of the order into the orderbook.
    block_on(mm_alice.wait_for_log(10., |log| {
        log.contains(&format!(
            "Maker order {} was recently cancelled, ignoring",
            set_price.result.uuid
        ))
    }))
    .unwrap();

    // Check Alice's orderbook again to make sure the order wasn't re-inserted after the cancellation.
    let orderbook = block_on(orderbook_v2(&mm_alice, "RICK", "MORTY"));
    let asks = orderbook["result"]["asks"].as_array().unwrap();
    // Alice shouldn't find the order in the orderbook.
    assert_eq!(asks.len(), 0, "Alice RICK/MORTY orderbook must have exactly 0 ask");

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

// ignored because it requires a long-running ZOMBIE initialization process
#[test]
#[ignore]
fn zhtlc_orders_sync_alice_connected_before_creation() {
    let bob_passphrase = get_passphrase!(".env.seed", "BOB_PASSPHRASE").unwrap();
    let alice_passphrase = get_passphrase!(".env.client", "ALICE_PASSPHRASE").unwrap();

    let coins = json!([rick_conf(), zombie_conf()]);

    let bob_conf = Mm2TestConf::seednode(&bob_passphrase, &coins);
    let mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();

    let (_dump_log, _dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    let alice_conf = Mm2TestConf::light_node(&alice_passphrase, &coins, &[&mm_bob.ip.to_string()]);
    let mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    block_on(enable_electrum_json(&mm_bob, RICK, false, doc_electrums()));
    block_on(enable_z_coin_light(
        &mm_bob,
        ZOMBIE_TICKER,
        ZOMBIE_ELECTRUMS,
        ZOMBIE_LIGHTWALLETD_URLS,
        None,
        None,
    ));

    let set_price_json = json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": ZOMBIE_TICKER,
        "rel": RICK,
        "price": 1,
        "volume": "1",
    });
    log!("Issue sell request on Bob side by setting base/rel price…");
    let rc = block_on(mm_bob.rpc(&set_price_json)).unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let set_price_res: SetPriceResponse = json::from_str(&rc.1).unwrap();

    let orderbook = block_on(orderbook_v2(&mm_alice, ZOMBIE_TICKER, RICK));
    let orderbook: RpcV2Response<OrderbookV2Response> = json::from_value(orderbook).unwrap();
    let orderbook = orderbook.result;

    assert_eq!(1, orderbook.asks.len());
    orderbook
        .asks
        .iter()
        .find(|ask| ask.entry.uuid == set_price_res.result.uuid)
        .unwrap();

    thread::sleep(Duration::from_secs(MIN_ORDER_KEEP_ALIVE_INTERVAL * 3));

    let orderbook = block_on(orderbook_v2(&mm_alice, ZOMBIE_TICKER, RICK));
    let orderbook: RpcV2Response<OrderbookV2Response> = json::from_value(orderbook).unwrap();
    let orderbook = orderbook.result;

    assert_eq!(1, orderbook.asks.len());
    orderbook
        .asks
        .iter()
        .find(|ask| ask.entry.uuid == set_price_res.result.uuid)
        .unwrap();
}

// ignored because it requires a long-running ZOMBIE initialization process
#[test]
#[ignore]
fn zhtlc_orders_sync_alice_connected_after_creation() {
    let bob_passphrase = get_passphrase!(".env.seed", "BOB_PASSPHRASE").unwrap();
    let alice_passphrase = get_passphrase!(".env.client", "ALICE_PASSPHRASE").unwrap();

    let coins = json!([rick_conf(), zombie_conf()]);

    let bob_conf = Mm2TestConf::seednode(&bob_passphrase, &coins);
    let mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();

    let (_dump_log, _dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    block_on(enable_electrum_json(&mm_bob, "RICK", false, doc_electrums()));
    block_on(enable_z_coin_light(
        &mm_bob,
        ZOMBIE_TICKER,
        ZOMBIE_ELECTRUMS,
        ZOMBIE_LIGHTWALLETD_URLS,
        None,
        None,
    ));

    let set_price_json = json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "ZOMBIE",
        "rel": "RICK",
        "price": 1,
        "volume": "1",
    });
    log!("Issue sell request on Bob side by setting base/rel price…");
    let rc = block_on(mm_bob.rpc(&set_price_json)).unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let bob_set_price_res: SetPriceResponse = json::from_str(&rc.1).unwrap();

    let alice_conf = Mm2TestConf::light_node(&alice_passphrase, &coins, &[&mm_bob.ip.to_string()]);
    let mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    block_on(enable_electrum_json(&mm_alice, RICK, false, doc_electrums()));
    block_on(enable_z_coin_light(
        &mm_alice,
        ZOMBIE_TICKER,
        ZOMBIE_ELECTRUMS,
        ZOMBIE_LIGHTWALLETD_URLS,
        None,
        None,
    ));

    let set_price_json = json!({
        "userpass": mm_alice.userpass,
        "method": "setprice",
        "base": "RICK",
        "rel": "ZOMBIE",
        "price": 1,
        "volume": "1",
    });
    log!("Issue sell request on Alice side to trigger subscription on orderbook topic");
    let rc = block_on(mm_alice.rpc(&set_price_json)).unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    thread::sleep(Duration::from_secs(MIN_ORDER_KEEP_ALIVE_INTERVAL));

    let orderbook = block_on(orderbook_v2(&mm_alice, ZOMBIE_TICKER, RICK));
    let orderbook: RpcV2Response<OrderbookV2Response> = json::from_value(orderbook).unwrap();
    let orderbook = orderbook.result;

    assert_eq!(1, orderbook.asks.len());
    orderbook
        .asks
        .iter()
        .find(|ask| ask.entry.uuid == bob_set_price_res.result.uuid)
        .unwrap();
}

#[test]
fn test_expirable_order() {
    let coins = json!([rick_conf(), morty_conf()]);

    let bob_passphrase = "bob passphrase";
    let mm_bob_conf = Mm2TestConf::seednode(bob_passphrase, &coins);
    let mm_bob = MarketMakerIt::start(mm_bob_conf.conf, mm_bob_conf.rpc_password, None).unwrap();

    block_on(enable_electrum(&mm_bob, "RICK", false, DOC_ELECTRUM_ADDRS));
    block_on(enable_electrum(&mm_bob, "MORTY", false, MARTY_ELECTRUM_ADDRS));

    let expiration_min = 1;
    let _ = block_on(set_price(
        &mm_bob,
        "RICK",
        "MORTY",
        "0.1",
        "0.1",
        false,
        Some(expiration_min),
    ));

    let mm_alice_conf = Mm2TestConf::light_node("alice passphrase", &coins, &[&mm_bob.ip.to_string()]);
    let mut mm_alice = MarketMakerIt::start(mm_alice_conf.conf, mm_alice_conf.rpc_password, None).unwrap();

    let orderbook = block_on(orderbook_v2(&mm_alice, "RICK", "MORTY"));
    let asks = orderbook["result"]["asks"].as_array().unwrap();
    // Alice should see the order in the orderbook as she got it through `GetOrderbook` p2p request.
    assert_eq!(asks.len(), 1, "Alice RICK/MORTY orderbook must have exactly 1 ask");

    // Sleep until order expires
    thread::sleep(Duration::from_secs(expiration_min as u64 * 60 + 1));

    // Make sure Alice receives the order cancellation message.
    block_on(mm_alice.wait_for_log(10., |log| {
        log.contains("received ordermatch message MakerOrderCancelled")
    }))
    .unwrap();

    let orderbook = block_on(orderbook_v2(&mm_alice, "RICK", "MORTY"));
    let asks = orderbook["result"]["asks"].as_array().unwrap();
    // Alice shouldn't find the order in the orderbook as it was expired just recently.
    assert_eq!(asks.len(), 0, "Alice RICK/MORTY orderbook must have exactly 0 ask");
}
