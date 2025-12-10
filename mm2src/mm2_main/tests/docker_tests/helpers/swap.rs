//! Swap orchestration helpers for docker tests.
//!
//! This module provides high-level cross-chain atomic swap test scenarios.
//! For chain-specific helpers, import directly from the other `helpers` submodules.

use coins::MarketCoinOps;
use common::block_on;
use crypto::privkey::key_pair_from_secret;
use mm2_test_helpers::for_tests::{
    check_my_swap_status, check_recent_swaps, enable_eth_coin, enable_native, enable_native_bch, erc20_dev_conf,
    eth_dev_conf, mm_dump, wait_check_stats_swap_status, MarketMakerIt,
};
use serde_json::Value as Json;
use std::thread;
use std::time::Duration;

use super::env::{random_secp256k1_secret, Secp256k1Secret, SET_BURN_PUBKEY_TO_ALICE};
use super::eth::{erc20_contract_checksum, fill_eth_erc20_with_private_key, swap_contract_checksum, GETH_RPC_URL};
use super::qrc20::{
    enable_qrc20_native, fill_qrc20_address, generate_segwit_qtum_coin_with_random_privkey, qrc20_coin_conf_item,
    qrc20_coin_from_privkey, qtum_conf_path, wait_for_estimate_smart_fee,
};
use super::utxo::{fill_address, get_prefilled_slp_privkey, get_slp_token_id, utxo_coin_from_privkey};

// =============================================================================
// Cross-chain swap test scenarios
// =============================================================================

/// End-to-end atomic swap test between two coins.
///
/// This function:
/// 1. Generates and funds wallets for both maker (base) and taker (rel) coins
/// 2. Starts two MarketMaker instances (Bob as maker, Alice as taker)
/// 3. Enables all required coins on both instances
/// 4. Places a setprice order and matches with a buy order
/// 5. Waits for swap completion and verifies both sides
pub fn trade_base_rel((base, rel): (&str, &str)) {
    /// Generate a wallet with the random private key and fill the wallet with Qtum (required by gas_fee) and specified in `ticker` coin.
    fn generate_and_fill_priv_key(ticker: &str) -> Secp256k1Secret {
        let timeout = 30; // timeout if test takes more than 30 seconds to run

        match ticker {
            "QTUM" => {
                //Segwit QTUM
                wait_for_estimate_smart_fee(timeout).expect("!wait_for_estimate_smart_fee");
                let (_ctx, _coin, priv_key) = generate_segwit_qtum_coin_with_random_privkey("QTUM", 10.into(), Some(0));

                priv_key
            },
            "QICK" | "QORTY" => {
                let priv_key = random_secp256k1_secret();
                let (_ctx, coin) = qrc20_coin_from_privkey(ticker, priv_key);
                let my_address = coin.my_address().expect("!my_address");
                fill_address(&coin, &my_address, 10.into(), timeout);
                fill_qrc20_address(&coin, 10.into(), timeout);

                priv_key
            },
            "MYCOIN" | "MYCOIN1" => {
                let priv_key = random_secp256k1_secret();
                let (_ctx, coin) = utxo_coin_from_privkey(ticker, priv_key);
                let my_address = coin.my_address().expect("!my_address");
                fill_address(&coin, &my_address, 10.into(), timeout);
                priv_key
            },
            "ADEXSLP" | "FORSLP" => Secp256k1Secret::from(get_prefilled_slp_privkey()),
            "ETH" | "ERC20DEV" => {
                let priv_key = random_secp256k1_secret();
                fill_eth_erc20_with_private_key(priv_key);
                priv_key
            },
            _ => panic!(
                "Unsupported ticker: {}. Expected one of: QTUM, QICK, QORTY, MYCOIN, MYCOIN1, ETH, ERC20DEV, FORSLP, ADEXSLP",
                ticker
            ),
        }
    }

    // Determine which chain families are needed for this trade pair
    let uses_eth = matches!(base, "ETH" | "ERC20DEV") || matches!(rel, "ETH" | "ERC20DEV");
    let uses_qrc20 = matches!(base, "QICK" | "QORTY" | "QTUM") || matches!(rel, "QICK" | "QORTY" | "QTUM");
    let uses_utxo = matches!(base, "MYCOIN" | "MYCOIN1") || matches!(rel, "MYCOIN" | "MYCOIN1");
    let uses_slp = matches!(base, "FORSLP" | "ADEXSLP") || matches!(rel, "FORSLP" | "ADEXSLP");

    let bob_priv_key = generate_and_fill_priv_key(base);
    let alice_priv_key = generate_and_fill_priv_key(rel);
    let alice_pubkey_str = hex::encode(
        key_pair_from_secret(&alice_priv_key)
            .expect("valid test key pair")
            .public()
            .to_vec(),
    );

    let mut envs = vec![];
    if SET_BURN_PUBKEY_TO_ALICE.get() {
        envs.push(("TEST_BURN_ADDR_RAW_PUBKEY", alice_pubkey_str.as_str()));
    }

    // Build coins config dynamically based on which chains are needed
    let mut coins_vec: Vec<Json> = Vec::new();

    if uses_eth {
        coins_vec.push(eth_dev_conf());
        coins_vec.push(erc20_dev_conf(&erc20_contract_checksum()));
    }

    if uses_qrc20 {
        let confpath = qtum_conf_path();
        coins_vec.push(qrc20_coin_conf_item("QICK"));
        coins_vec.push(qrc20_coin_conf_item("QORTY"));
        // TODO: check if we should fix protocol "type":"UTXO" to "QTUM" for this and other QTUM coin tests.
        // Maybe we should use a different coin for "UTXO" protocol and make new tests for "QTUM" protocol
        coins_vec.push(json!({
            "coin": "QTUM", "asset": "QTUM", "required_confirmations": 0, "decimals": 8,
            "pubtype": 120, "p2shtype": 110, "wiftype": 128, "segwit": true, "txfee": 0,
            "txfee_volatility_percent": 0.1, "dust": 72800, "mm2": 1, "network": "regtest",
            "confpath": confpath, "protocol": {"type": "UTXO"}, "bech32_hrp": "qcrt",
            "address_format": {"format": "segwit"}
        }));
    }

    if uses_utxo {
        coins_vec.push(json!({
            "coin": "MYCOIN", "asset": "MYCOIN", "required_confirmations": 0,
            "txversion": 4, "overwintered": 1, "txfee": 1000, "protocol": {"type": "UTXO"}
        }));
        coins_vec.push(json!({
            "coin": "MYCOIN1", "asset": "MYCOIN1", "required_confirmations": 0,
            "txversion": 4, "overwintered": 1, "txfee": 1000, "protocol": {"type": "UTXO"}
        }));
    }

    if uses_slp {
        coins_vec.push(json!({
            "coin": "FORSLP", "asset": "FORSLP", "required_confirmations": 0,
            "txversion": 4, "overwintered": 1, "txfee": 1000,
            "protocol": {"type": "BCH", "protocol_data": {"slp_prefix": "slptest"}}
        }));
        coins_vec.push(json!({
            "coin": "ADEXSLP",
            "protocol": {"type": "SLPTOKEN", "protocol_data": {"decimals": 8, "token_id": get_slp_token_id(), "platform": "FORSLP"}}
        }));
    }

    let coins = Json::Array(coins_vec);
    let mut mm_bob = block_on(MarketMakerIt::start_with_envs(
        json! ({
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
        envs.as_slice(),
    ))
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);
    block_on(mm_bob.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();

    let mut mm_alice = block_on(MarketMakerIt::start_with_envs(
        json! ({
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
        envs.as_slice(),
    ))
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);
    block_on(mm_alice.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();

    // Enable coins based on what's needed for this trade (Bob)
    if uses_qrc20 {
        log!("{:?}", block_on(enable_qrc20_native(&mm_bob, "QICK")));
        log!("{:?}", block_on(enable_qrc20_native(&mm_bob, "QORTY")));
        log!("{:?}", block_on(enable_native(&mm_bob, "QTUM", &[], None)));
    }
    if uses_utxo {
        log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
        log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    }
    if uses_slp {
        log!("{:?}", block_on(enable_native_bch(&mm_bob, "FORSLP", &[])));
        log!("{:?}", block_on(enable_native(&mm_bob, "ADEXSLP", &[], None)));
    }
    if uses_eth {
        let swap_contract = swap_contract_checksum();
        log!(
            "{:?}",
            block_on(enable_eth_coin(
                &mm_bob,
                "ETH",
                &[GETH_RPC_URL],
                &swap_contract,
                None,
                false
            ))
        );
        log!(
            "{:?}",
            block_on(enable_eth_coin(
                &mm_bob,
                "ERC20DEV",
                &[GETH_RPC_URL],
                &swap_contract,
                None,
                false
            ))
        );
    }

    // Enable coins based on what's needed for this trade (Alice)
    if uses_qrc20 {
        log!("{:?}", block_on(enable_qrc20_native(&mm_alice, "QICK")));
        log!("{:?}", block_on(enable_qrc20_native(&mm_alice, "QORTY")));
        log!("{:?}", block_on(enable_native(&mm_alice, "QTUM", &[], None)));
    }
    if uses_utxo {
        log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
        log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));
    }
    if uses_slp {
        log!("{:?}", block_on(enable_native_bch(&mm_alice, "FORSLP", &[])));
        log!("{:?}", block_on(enable_native(&mm_alice, "ADEXSLP", &[], None)));
    }
    if uses_eth {
        let swap_contract = swap_contract_checksum();
        log!(
            "{:?}",
            block_on(enable_eth_coin(
                &mm_alice,
                "ETH",
                &[GETH_RPC_URL],
                &swap_contract,
                None,
                false
            ))
        );
        log!(
            "{:?}",
            block_on(enable_eth_coin(
                &mm_alice,
                "ERC20DEV",
                &[GETH_RPC_URL],
                &swap_contract,
                None,
                false
            ))
        );
    }

    let rc = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": base,
        "rel": rel,
        "price": 1,
        "volume": "3",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    thread::sleep(Duration::from_secs(1));

    log!("Issue alice {}/{} buy request", base, rel);
    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": base,
        "rel": rel,
        "price": 1,
        "volume": "2",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);
    let buy_json: Json = serde_json::from_str(&rc.1).unwrap();
    let uuid = buy_json["result"]["uuid"].as_str().unwrap().to_owned();

    // ensure the swaps are started
    block_on(mm_bob.wait_for_log(22., |log| {
        log.contains(&format!("Entering the maker_swap_loop {base}/{rel}"))
    }))
    .unwrap();
    block_on(mm_alice.wait_for_log(22., |log| {
        log.contains(&format!("Entering the taker_swap_loop {base}/{rel}"))
    }))
    .unwrap();

    // ensure the swaps are finished
    block_on(mm_bob.wait_for_log(600., |log| log.contains(&format!("[swap uuid={uuid}] Finished")))).unwrap();
    block_on(mm_alice.wait_for_log(600., |log| log.contains(&format!("[swap uuid={uuid}] Finished")))).unwrap();

    log!("Checking alice/taker status..");
    block_on(check_my_swap_status(
        &mm_alice,
        &uuid,
        "2".parse().unwrap(),
        "2".parse().unwrap(),
    ));

    log!("Checking bob/maker status..");
    block_on(check_my_swap_status(
        &mm_bob,
        &uuid,
        "2".parse().unwrap(),
        "2".parse().unwrap(),
    ));

    log!("Checking alice status..");
    block_on(wait_check_stats_swap_status(&mm_alice, &uuid, 240));

    log!("Checking bob status..");
    block_on(wait_check_stats_swap_status(&mm_bob, &uuid, 240));

    log!("Checking alice recent swaps..");
    block_on(check_recent_swaps(&mm_alice, 1));
    log!("Checking bob recent swaps..");
    block_on(check_recent_swaps(&mm_bob, 1));

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}
