// UTXO Swaps V1 Tests
//
// This module contains UTXO-only swap tests that were extracted from docker_tests_inner.rs
// These tests focus on UTXO swap mechanics, payment lifecycle, and related functionality.
// They do NOT require ETH/ERC20 containers - only MYCOIN/MYCOIN1 UTXO containers.
//
// Gated by: docker-tests-swaps-utxo

use crate::docker_tests::helpers::env::random_secp256k1_secret;
use crate::docker_tests::helpers::swap::trade_base_rel;
use crate::docker_tests::helpers::utxo::{
    fill_address, generate_utxo_coin_with_privkey, generate_utxo_coin_with_random_privkey, utxo_coin_from_privkey,
};
use crate::integration_tests_common::*;
use bitcrypto::dhash160;
use chain::OutPoint;
use coins::utxo::rpc_clients::UnspentInfo;
use coins::utxo::{GetUtxoListOps, UtxoCommonOps};
use coins::{
    ConfirmPaymentInput, FoundSwapTxSpend, MarketCoinOps, MmCoin, RefundPaymentArgs, SearchForSwapTxSpendInput,
    SendPaymentArgs, SpendPaymentArgs, SwapOps, SwapTxTypeWithSecretHash, TransactionEnum,
};
use common::{block_on, block_on_f01, executor::Timer, now_sec, wait_until_sec};
use mm2_number::{BigDecimal, MmNumber};
use mm2_test_helpers::for_tests::{
    get_locked_amount, kmd_conf, max_maker_vol, mm_dump, mycoin1_conf, mycoin_conf, set_price, start_swaps,
    MarketMakerIt, Mm2TestConf,
};
use mm2_test_helpers::structs::*;
use serde_json::Value as Json;
use std::collections::HashMap;
use std::str::FromStr;
use std::thread;
use std::time::Duration;

// =============================================================================
// UTXO Swap Spend/Refund Mechanics Tests
// Tests for searching swap tx spend status (refunded vs spent)
// =============================================================================

#[test]
fn test_search_for_swap_tx_spend_native_was_refunded_taker() {
    let timeout = wait_until_sec(120);
    let (_ctx, coin, _) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000u64.into());
    let my_public_key = coin.my_public_key().unwrap();

    let time_lock = now_sec() - 3600;
    let taker_payment_args = SendPaymentArgs {
        time_lock_duration: 0,
        time_lock,
        other_pubkey: my_public_key,
        secret_hash: &[0; 20],
        amount: 1u64.into(),
        swap_contract_address: &None,
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let tx = block_on(coin.send_taker_payment(taker_payment_args)).unwrap();

    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: tx.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();
    let maker_refunds_payment_args = RefundPaymentArgs {
        payment_tx: &tx.tx_hex(),
        time_lock,
        other_pubkey: my_public_key,
        tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
            maker_secret_hash: &[0; 20],
        },
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let refund_tx = block_on(coin.send_maker_refunds_payment(maker_refunds_payment_args)).unwrap();

    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: refund_tx.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let search_input = SearchForSwapTxSpendInput {
        time_lock,
        other_pub: coin.my_public_key().unwrap(),
        secret_hash: &[0; 20],
        tx: &tx.tx_hex(),
        search_from_block: 0,
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let found = block_on(coin.search_for_swap_tx_spend_my(search_input))
        .unwrap()
        .unwrap();
    assert_eq!(FoundSwapTxSpend::Refunded(refund_tx), found);
}

#[test]
fn test_for_non_existent_tx_hex_utxo() {
    // This test shouldn't wait till timeout!
    let timeout = wait_until_sec(120);
    let (_ctx, coin, _) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000u64.into());
    // bad transaction hex
    let tx = hex::decode("0400008085202f8902bf17bf7d1daace52e08f732a6b8771743ca4b1cb765a187e72fd091a0aabfd52000000006a47304402203eaaa3c4da101240f80f9c5e9de716a22b1ec6d66080de6a0cca32011cd77223022040d9082b6242d6acf9a1a8e658779e1c655d708379862f235e8ba7b8ca4e69c6012102031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3ffffffffff023ca13c0e9e085dd13f481f193e8a3e8fd609020936e98b5587342d994f4d020000006b483045022100c0ba56adb8de923975052312467347d83238bd8d480ce66e8b709a7997373994022048507bcac921fdb2302fa5224ce86e41b7efc1a2e20ae63aa738dfa99b7be826012102031d4256c4bc9f99ac88bf3dba21773132281f65f9bf23a59928bce08961e2f3ffffffff0300e1f5050000000017a9141ee6d4c38a3c078eab87ad1a5e4b00f21259b10d87000000000000000016611400000000000000000000000000000000000000001b94d736000000001976a91405aab5342166f8594baf17a7d9bef5d56744332788ac2d08e35e000000000000000000000000000000").unwrap();
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: tx,
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    let actual = block_on_f01(coin.wait_for_confirmations(confirm_payment_input))
        .err()
        .unwrap();
    assert!(actual.contains(
        "Tx d342ff9da528a2e262bddf2b6f9a27d1beb7aeb03f0fc8d9eac2987266447e44 was not found on chain after 10 tries"
    ));
}

#[test]
fn test_search_for_swap_tx_spend_native_was_refunded_maker() {
    let timeout = wait_until_sec(120);
    let (_ctx, coin, _) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000u64.into());
    let my_public_key = coin.my_public_key().unwrap();

    let time_lock = now_sec() - 3600;
    let maker_payment_args = SendPaymentArgs {
        time_lock_duration: 0,
        time_lock,
        other_pubkey: my_public_key,
        secret_hash: &[0; 20],
        amount: 1u64.into(),
        swap_contract_address: &None,
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let tx = block_on(coin.send_maker_payment(maker_payment_args)).unwrap();

    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: tx.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();
    let maker_refunds_payment_args = RefundPaymentArgs {
        payment_tx: &tx.tx_hex(),
        time_lock,
        other_pubkey: my_public_key,
        tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
            maker_secret_hash: &[0; 20],
        },
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let refund_tx = block_on(coin.send_maker_refunds_payment(maker_refunds_payment_args)).unwrap();

    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: refund_tx.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let search_input = SearchForSwapTxSpendInput {
        time_lock,
        other_pub: coin.my_public_key().unwrap(),
        secret_hash: &[0; 20],
        tx: &tx.tx_hex(),
        search_from_block: 0,
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let found = block_on(coin.search_for_swap_tx_spend_my(search_input))
        .unwrap()
        .unwrap();
    assert_eq!(FoundSwapTxSpend::Refunded(refund_tx), found);
}

#[test]
fn test_search_for_taker_swap_tx_spend_native_was_spent_by_maker() {
    let timeout = wait_until_sec(120);
    let (_ctx, coin, _) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000u64.into());
    let secret = [0; 32];
    let my_pubkey = coin.my_public_key().unwrap();

    let secret_hash = dhash160(&secret);
    let time_lock = now_sec() - 3600;
    let taker_payment_args = SendPaymentArgs {
        time_lock_duration: 0,
        time_lock,
        other_pubkey: my_pubkey,
        secret_hash: secret_hash.as_slice(),
        amount: 1u64.into(),
        swap_contract_address: &None,
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let tx = block_on(coin.send_taker_payment(taker_payment_args)).unwrap();

    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: tx.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();
    let maker_spends_payment_args = SpendPaymentArgs {
        other_payment_tx: &tx.tx_hex(),
        time_lock,
        other_pubkey: my_pubkey,
        secret: &secret,
        secret_hash: secret_hash.as_slice(),
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let spend_tx = block_on(coin.send_maker_spends_taker_payment(maker_spends_payment_args)).unwrap();

    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: spend_tx.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let search_input = SearchForSwapTxSpendInput {
        time_lock,
        other_pub: coin.my_public_key().unwrap(),
        secret_hash: &*dhash160(&secret),
        tx: &tx.tx_hex(),
        search_from_block: 0,
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let found = block_on(coin.search_for_swap_tx_spend_my(search_input))
        .unwrap()
        .unwrap();
    assert_eq!(FoundSwapTxSpend::Spent(spend_tx), found);
}

#[test]
fn test_search_for_maker_swap_tx_spend_native_was_spent_by_taker() {
    let timeout = wait_until_sec(120);
    let (_ctx, coin, _) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000u64.into());
    let secret = [0; 32];
    let my_pubkey = coin.my_public_key().unwrap();

    let time_lock = now_sec() - 3600;
    let secret_hash = dhash160(&secret);
    let maker_payment_args = SendPaymentArgs {
        time_lock_duration: 0,
        time_lock,
        other_pubkey: my_pubkey,
        secret_hash: secret_hash.as_slice(),
        amount: 1u64.into(),
        swap_contract_address: &None,
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until: 0,
    };
    let tx = block_on(coin.send_maker_payment(maker_payment_args)).unwrap();

    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: tx.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();
    let taker_spends_payment_args = SpendPaymentArgs {
        other_payment_tx: &tx.tx_hex(),
        time_lock,
        other_pubkey: my_pubkey,
        secret: &secret,
        secret_hash: secret_hash.as_slice(),
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let spend_tx = block_on(coin.send_taker_spends_maker_payment(taker_spends_payment_args)).unwrap();

    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: spend_tx.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let search_input = SearchForSwapTxSpendInput {
        time_lock,
        other_pub: coin.my_public_key().unwrap(),
        secret_hash: &*dhash160(&secret),
        tx: &tx.tx_hex(),
        search_from_block: 0,
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    };
    let found = block_on(coin.search_for_swap_tx_spend_my(search_input))
        .unwrap()
        .unwrap();
    assert_eq!(FoundSwapTxSpend::Spent(spend_tx), found);
}

#[test]
fn test_one_hundred_maker_payments_in_a_row_native() {
    let timeout = 30;
    let (_ctx, coin, _) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    fill_address(&coin, &coin.my_address().unwrap(), 2.into(), timeout);
    let secret = [0; 32];
    let my_pubkey = coin.my_public_key().unwrap();

    let time_lock = now_sec() - 3600;
    let mut unspents = vec![];
    let mut sent_tx = vec![];
    for i in 0..100 {
        let maker_payment_args = SendPaymentArgs {
            time_lock_duration: 0,
            time_lock: time_lock + i,
            other_pubkey: my_pubkey,
            secret_hash: &*dhash160(&secret),
            amount: 1.into(),
            swap_contract_address: &coin.swap_contract_address(),
            swap_unique_data: &[],
            payment_instructions: &None,
            watcher_reward: None,
            wait_for_confirmation_until: 0,
        };
        let tx = block_on(coin.send_maker_payment(maker_payment_args)).unwrap();
        if let TransactionEnum::UtxoTx(tx) = tx {
            unspents.push(UnspentInfo {
                outpoint: OutPoint {
                    hash: tx.hash(),
                    index: 2,
                },
                value: tx.outputs[2].value,
                height: None,
                script: coin
                    .script_for_address(&block_on(coin.as_ref().derivation_method.unwrap_single_addr()))
                    .unwrap(),
            });
            sent_tx.push(tx);
        }
    }

    let recently_sent = block_on(coin.as_ref().recently_spent_outpoints.lock());

    unspents = recently_sent
        .replace_spent_outputs_with_cache(unspents.into_iter().collect())
        .into_iter()
        .collect();

    let last_tx = sent_tx.last().unwrap();
    let expected_unspent = UnspentInfo {
        outpoint: OutPoint {
            hash: last_tx.hash(),
            index: 2,
        },
        value: last_tx.outputs[2].value,
        height: None,
        script: last_tx.outputs[2].script_pubkey.clone().into(),
    };
    assert_eq!(vec![expected_unspent], unspents);
}

// =============================================================================
// UTXO-only Swap and Trade Tests
// Tests for complete swap flows using only MYCOIN/MYCOIN1
// =============================================================================

#[test]
fn test_trade_base_rel_mycoin_mycoin1_coins() {
    trade_base_rel(("MYCOIN", "MYCOIN1"));
}

#[test]
fn test_trade_base_rel_mycoin_mycoin1_coins_burnkey_as_alice() {
    // Trade with burn pubkey set as Alice's pubkey (for testing purposes)
    // Uses the SET_BURN_PUBKEY_TO_ALICE flag via trade_base_rel
    use crate::docker_tests::helpers::env::SET_BURN_PUBKEY_TO_ALICE;
    SET_BURN_PUBKEY_TO_ALICE.set(true);
    trade_base_rel(("MYCOIN", "MYCOIN1"));
    SET_BURN_PUBKEY_TO_ALICE.set(false);
}

// =============================================================================
// Max Volume Tests
// Tests for max_taker_vol and max_maker_vol RPCs
// =============================================================================

#[test]
fn test_get_max_taker_vol() {
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 1.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",
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

    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "max_taker_vol",
        "coin": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!max_taker_vol: {}", rc.1);
    let json: MaxTakerVolResponse = serde_json::from_str(&rc.1).unwrap();
    let expected = MmNumber::from((77699596737u64, 77800000000u64)).to_fraction();
    assert_eq!(json.result, expected);
    assert_eq!(json.coin, "MYCOIN1");

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "sell",
        "base": "MYCOIN1",
        "rel": "MYCOIN",
        "price": 1,
        "volume": json.result,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);

    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_get_max_taker_vol_dex_fee_min_tx_amount() {
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", "0.00532845".parse().unwrap());
    let coins = json!([mycoin_conf(10000), mycoin1_conf(10000)]);
    let mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",
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

    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "max_taker_vol",
        "coin": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!max_taker_vol: {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(json["result"]["numer"], Json::from("105331"));
    assert_eq!(json["result"]["denom"], Json::from("20000000"));

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "sell",
        "base": "MYCOIN1",
        "rel": "MYCOIN",
        "price": 1,
        "volume": {
            "numer": json["result"]["numer"],
            "denom": json["result"]["denom"],
        }
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);

    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_get_max_taker_vol_dust_threshold() {
    let (_ctx, coin, priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", "0.0014041".parse().unwrap());
    let coins = json!([
    mycoin_conf(10000),
    {"coin":"MYCOIN1","asset":"MYCOIN1","txversion":4,"overwintered":1,"txfee":10000,"protocol":{"type":"UTXO"},"dust":72800}
    ]);
    let mm = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",
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
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm.log_path);

    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "max_taker_vol",
        "coin": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!max_taker_vol {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    let result: MmNumber = serde_json::from_value(json["result"].clone()).unwrap();
    assert!(result.is_zero());

    fill_address(&coin, &coin.my_address().unwrap(), "0.0002".parse().unwrap(), 30);

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "max_taker_vol",
        "coin": "MYCOIN1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!max_taker_vol: {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(json["result"]["numer"], Json::from("3973"));
    assert_eq!(json["result"]["denom"], Json::from("5000000"));

    block_on(mm.stop()).unwrap();
}

#[test]
fn test_get_max_taker_vol_with_kmd() {
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 1.into());
    let coins = json!([mycoin_conf(10000), mycoin1_conf(10000), kmd_conf(10000)]);
    let mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",
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

    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    let electrum = block_on(enable_electrum(
        &mm_alice,
        "KMD",
        false,
        &[
            "electrum1.cipig.net:10001",
            "electrum2.cipig.net:10001",
            "electrum3.cipig.net:10001",
        ],
    ));
    log!("{:?}", electrum);
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "max_taker_vol",
        "coin": "MYCOIN1",
        "trade_with": "KMD",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!max_taker_vol: {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(json["result"]["numer"], Json::from("2589865579"));
    assert_eq!(json["result"]["denom"], Json::from("2593000000"));

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "sell",
        "base": "MYCOIN1",
        "rel": "KMD",
        "price": 1,
        "volume": {
            "numer": json["result"]["numer"],
            "denom": json["result"]["denom"],
        }
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);

    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_get_max_maker_vol() {
    let (_ctx, _, priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 1.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(priv_key)), &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();
    let (_dump_log, _dump_dashboard) = mm_dump(&mm.log_path);

    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));

    let expected_volume = MmNumber::from("0.99999726");
    let expected = MaxMakerVolResponse {
        coin: "MYCOIN1".to_string(),
        volume: MmNumberMultiRepr::from(expected_volume.clone()),
        balance: MmNumberMultiRepr::from(1),
        locked_by_swaps: MmNumberMultiRepr::from(0),
    };
    let actual = block_on(max_maker_vol(&mm, "MYCOIN1")).unwrap::<MaxMakerVolResponse>();
    assert_eq!(actual, expected);

    let res = block_on(set_price(&mm, "MYCOIN1", "MYCOIN", "1", "0", true, None));
    assert_eq!(res.result.max_base_vol, expected_volume.to_decimal());
}

#[test]
fn test_get_max_maker_vol_error() {
    let priv_key = random_secp256k1_secret();
    let coins = json!([mycoin_conf(1000)]);
    let conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(priv_key)), &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();
    let (_dump_log, _dump_dashboard) = mm_dump(&mm.log_path);

    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));

    let actual_error = block_on(max_maker_vol(&mm, "MYCOIN")).unwrap_err::<max_maker_vol_error::NotSufficientBalance>();
    let expected_error = max_maker_vol_error::NotSufficientBalance {
        coin: "MYCOIN".to_owned(),
        available: 0.into(),
        required: BigDecimal::from(1000) / BigDecimal::from(100_000_000),
        locked_by_swaps: None,
    };
    assert_eq!(actual_error.error_type, "NotSufficientBalance");
    assert_eq!(actual_error.error_data, Some(expected_error));
}

// =============================================================================
// UTXO Merge and Consolidation Tests
// Tests for UTXO merge functionality and consolidate_utxos RPC
// =============================================================================

#[test]
fn test_utxo_merge() {
    let timeout = 30;
    let (_ctx, coin, privkey) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    fill_address(&coin, &coin.my_address().unwrap(), 2.into(), timeout);
    fill_address(&coin, &coin.my_address().unwrap(), 2.into(), timeout);
    fill_address(&coin, &coin.my_address().unwrap(), 2.into(), timeout);
    fill_address(&coin, &coin.my_address().unwrap(), 2.into(), timeout);

    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mut mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",
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
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    let native = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "enable",
        "coin": "MYCOIN",
        "mm2": 1,
        "utxo_merge_params": {
            "merge_at": 2,
            "check_every": 1,
        }
    })))
    .unwrap();
    assert!(native.0.is_success(), "'enable' failed: {}", native.1);
    log!("Enable result {}", native.1);

    block_on(mm_bob.wait_for_log(4., |log| log.contains("Starting UTXO merge loop for coin MYCOIN"))).unwrap();

    block_on(mm_bob.wait_for_log(4., |log| {
        log.contains("UTXO merge of 5 outputs successful for coin=MYCOIN, tx_hash")
    }))
    .unwrap();

    thread::sleep(Duration::from_secs(2));
    let address = block_on(coin.as_ref().derivation_method.unwrap_single_addr());
    let (unspents, _) = block_on(coin.get_unspent_ordered_list(&address)).unwrap();
    assert_eq!(unspents.len(), 1);
}

#[test]
fn test_utxo_merge_max_merge_at_once() {
    let timeout = 30;
    let (_ctx, coin, privkey) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    fill_address(&coin, &coin.my_address().unwrap(), 2.into(), timeout);
    fill_address(&coin, &coin.my_address().unwrap(), 2.into(), timeout);
    fill_address(&coin, &coin.my_address().unwrap(), 2.into(), timeout);
    fill_address(&coin, &coin.my_address().unwrap(), 2.into(), timeout);

    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mut mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",
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
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    let native = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "enable",
        "coin": "MYCOIN",
        "mm2": 1,
        "utxo_merge_params": {
            "merge_at": 3,
            "check_every": 1,
            "max_merge_at_once": 4,
        }
    })))
    .unwrap();
    assert!(native.0.is_success(), "'enable' failed: {}", native.1);
    log!("Enable result {}", native.1);

    block_on(mm_bob.wait_for_log(4., |log| log.contains("Starting UTXO merge loop for coin MYCOIN"))).unwrap();

    block_on(mm_bob.wait_for_log(4., |log| {
        log.contains("UTXO merge of 4 outputs successful for coin=MYCOIN, tx_hash")
    }))
    .unwrap();

    thread::sleep(Duration::from_secs(2));
    let address = block_on(coin.as_ref().derivation_method.unwrap_single_addr());
    let (unspents, _) = block_on(coin.get_unspent_ordered_list(&address)).unwrap();
    assert_eq!(unspents.len(), 2);
}

#[test]
fn test_consolidate_utxos_rpc() {
    let timeout = 30;
    let utxos = 50;
    let (_ctx, coin, privkey) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());

    for i in 1..=utxos {
        fill_address(&coin, &coin.my_address().unwrap(), i.into(), timeout);
    }

    let coins = json!([mycoin_conf(1000)]);
    let mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",
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
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));

    let consolidate_rpc = |merge_at: u32, merge_at_once: u32| {
        block_on(mm_bob.rpc(&json!({
            "mmrpc": "2.0",
            "userpass": mm_bob.userpass,
            "method": "consolidate_utxos",
            "params": {
                "coin": "MYCOIN",
                "merge_conditions": {
                    "merge_at": merge_at,
                    "max_merge_at_once": merge_at_once,
                },
                "broadcast": true
            }
        })))
        .unwrap()
    };

    let res = consolidate_rpc(52, 4);
    assert!(!res.0.is_success(), "Expected error for merge_at > utxos: {}", res.1);

    let res = consolidate_rpc(30, 4);
    assert!(res.0.is_success(), "Consolidate utxos failed: {}", res.1);

    let res: RpcSuccessResponse<ConsolidateUtxoResponse> =
        serde_json::from_str(&res.1).expect("Expected 'RpcSuccessResponse<ConsolidateUtxoResponse>'");
    assert_eq!(res.result.consolidated_utxos.len(), 4);
    for i in 1..=4 {
        assert_eq!(res.result.consolidated_utxos[i - 1].value, (i as u32).into());
    }

    thread::sleep(Duration::from_secs(2));
    let address = block_on(coin.as_ref().derivation_method.unwrap_single_addr());
    let (unspents, _) = block_on(coin.get_unspent_ordered_list(&address)).unwrap();
    assert_eq!(unspents.len(), 51 - 4 + 1);
}

#[test]
fn test_fetch_utxos_rpc() {
    let timeout = 30;
    let (_ctx, coin, privkey) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());

    for i in 1..=10 {
        fill_address(&coin, &coin.my_address().unwrap(), i.into(), timeout);
    }

    let coins = json!([mycoin_conf(1000)]);
    let mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",
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
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));

    let fetch_utxo_rpc = || {
        let res = block_on(mm_bob.rpc(&json!({
            "mmrpc": "2.0",
            "userpass": mm_bob.userpass,
            "method": "fetch_utxos",
            "params": {
                "coin": "MYCOIN"
            }
        })))
        .unwrap();
        assert!(res.0.is_success(), "Fetch UTXOs failed: {}", res.1);
        let res: RpcSuccessResponse<FetchUtxosResponse> =
            serde_json::from_str(&res.1).expect("Expected 'RpcSuccessResponse<FetchUtxosResponse>'");
        res.result
    };

    let res = fetch_utxo_rpc();
    assert!(res.total_count == 11);

    fill_address(&coin, &coin.my_address().unwrap(), 100.into(), timeout);
    thread::sleep(Duration::from_secs(2));

    let res = fetch_utxo_rpc();
    assert!(res.total_count == 12);
    assert!(res.addresses[0].utxos.iter().any(|utxo| utxo.value == 100.into()));
}

// =============================================================================
// Withdraw Tests (UTXO-only)
// Tests for withdraw RPC with insufficient balance
// =============================================================================

#[test]
fn test_withdraw_not_sufficient_balance() {
    let privkey = random_secp256k1_secret();
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mm = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",
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
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm.log_path);
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));

    let amount = BigDecimal::from(1);
    let withdraw = block_on(mm.rpc(&json!({
        "mmrpc": "2.0",
        "userpass": mm.userpass,
        "method": "withdraw",
        "params": {
            "coin": "MYCOIN",
            "to": "RJTYiYeJ8eVvJ53n2YbrVmxWNNMVZjDGLh",
            "amount": amount,
        },
        "id": 0,
    })))
    .unwrap();

    assert!(withdraw.0.is_client_error(), "MYCOIN withdraw: {}", withdraw.1);
    log!("error: {:?}", withdraw.1);
    let error: RpcErrorResponse<withdraw_error::NotSufficientBalance> =
        serde_json::from_str(&withdraw.1).expect("Expected 'RpcErrorResponse<NotSufficientBalance>'");
    let expected_error = withdraw_error::NotSufficientBalance {
        coin: "MYCOIN".to_owned(),
        available: 0.into(),
        required: amount,
    };
    assert_eq!(error.error_type, "NotSufficientBalance");
    assert_eq!(error.error_data, Some(expected_error));

    let balance = BigDecimal::from(1) / BigDecimal::from(2);
    let (_ctx, coin) = utxo_coin_from_privkey("MYCOIN", privkey);
    fill_address(&coin, &coin.my_address().unwrap(), balance.clone(), 30);

    let txfee = BigDecimal::from_str("0.00000211").unwrap();
    let withdraw = block_on(mm.rpc(&json!({
        "mmrpc": "2.0",
        "userpass": mm.userpass,
        "method": "withdraw",
        "params": {
            "coin": "MYCOIN",
            "to": "RJTYiYeJ8eVvJ53n2YbrVmxWNNMVZjDGLh",
            "amount": balance,
        },
        "id": 0,
    })))
    .unwrap();

    assert!(withdraw.0.is_client_error(), "MYCOIN withdraw: {}", withdraw.1);
    log!("error: {:?}", withdraw.1);
    let error: RpcErrorResponse<withdraw_error::NotSufficientBalance> =
        serde_json::from_str(&withdraw.1).expect("Expected 'RpcErrorResponse<NotSufficientBalance>'");
    let expected_error = withdraw_error::NotSufficientBalance {
        coin: "MYCOIN".to_owned(),
        available: balance.clone(),
        required: balance + txfee,
    };
    assert_eq!(error.error_type, "NotSufficientBalance");
    assert_eq!(error.error_data, Some(expected_error));
}

// =============================================================================
// Locked Amount Tests
// Tests for locked_amount RPC during swaps
// =============================================================================

#[test]
fn test_locked_amount() {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 1000.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let bob_conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(bob_priv_key)), &coins);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    let alice_conf = Mm2TestConf::light_node(
        &format!("0x{}", hex::encode(alice_priv_key)),
        &coins,
        &[&mm_bob.ip.to_string()],
    );
    let mut mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));

    block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[("MYCOIN", "MYCOIN1")],
        1.,
        1.,
        777.,
    ));

    let locked_bob = block_on(get_locked_amount(&mm_bob, "MYCOIN"));
    assert_eq!(locked_bob.coin, "MYCOIN");

    let expected_result: MmNumberMultiRepr = MmNumber::from("777.00000274").into();
    assert_eq!(expected_result, locked_bob.locked_amount);

    let locked_alice = block_on(get_locked_amount(&mm_alice, "MYCOIN1"));
    assert_eq!(locked_alice.coin, "MYCOIN1");

    let expected_result: MmNumberMultiRepr = MmNumber::from("778.00000519").into();
    assert_eq!(expected_result, locked_alice.locked_amount);
}

// =============================================================================
// Swap Lifecycle Tests
// Tests for swap stopping, order transformation, etc.
// =============================================================================

#[test]
fn swaps_should_stop_on_stop_rpc() {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 1000.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let bob_conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(bob_priv_key)), &coins);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);

    let alice_conf = Mm2TestConf::light_node(
        &format!("0x{}", hex::encode(alice_priv_key)),
        &coins,
        &[&mm_bob.ip.to_string()],
    );
    let mut mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));

    block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[("MYCOIN", "MYCOIN1")],
        1.,
        1.,
        0.0001,
    ));

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_fill_or_kill_taker_order_should_not_transform_to_maker() {
    let privkey = random_secp256k1_secret();
    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), privkey);

    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000),]);

    let conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(privkey)), &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));

    log!("Issue bob MYCOIN/MYCOIN1 sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "sell",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": 0.1,
        "order_type": {
            "type": "FillOrKill"
        },
        "timeout": 2,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);
    let sell_json: Json = serde_json::from_str(&rc.1).unwrap();
    let order_type = sell_json["result"]["order_type"]["type"].as_str();
    assert_eq!(order_type, Some("FillOrKill"));

    log!("Wait for 4 seconds for Bob order to be cancelled");
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
    assert!(my_maker_orders.is_empty(), "maker_orders must be empty");
    assert!(my_taker_orders.is_empty(), "taker_orders must be empty");
}

#[test]
fn test_gtc_taker_order_should_transform_to_maker() {
    let privkey = random_secp256k1_secret();
    generate_utxo_coin_with_privkey("MYCOIN", 1000.into(), privkey);

    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000),]);

    let conf = Mm2TestConf::seednode(&format!("0x{}", hex::encode(privkey)), &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));

    log!("Issue bob MYCOIN/MYCOIN1 sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "sell",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": 0.1,
        "order_type": {
            "type": "GoodTillCancelled"
        },
        "timeout": 2,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    let rc_json: Json = serde_json::from_str(&rc.1).unwrap();
    let uuid: String = serde_json::from_value(rc_json["result"]["uuid"].clone()).unwrap();

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
    assert_eq!(
        1,
        my_maker_orders.len(),
        "maker_orders must have exactly 1 order, but has {:?}",
        my_maker_orders
    );
    assert!(my_taker_orders.is_empty(), "taker_orders must be empty");
    assert!(my_maker_orders.contains_key(&uuid));
}

// =============================================================================
// Buy/Sell with Locked Coins Tests
// Tests for order placement when coins are locked by other swaps
// =============================================================================

#[test]
fn test_buy_when_coins_locked_by_other_swap() {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 2.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mut mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",
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
            "dht": "on",
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
        "volume": {
            "numer":"77699596737",
            "denom":"77800000000"
        },
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    block_on(mm_bob.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
    thread::sleep(Duration::from_secs(6));

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": {
            "numer":"77699599999",
            "denom":"77800000000"
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "buy success, but should fail: {}", rc.1);
    assert!(rc.1.contains("Not enough MYCOIN1 for swap"), "{}", rc.1);
    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_sell_when_coins_locked_by_other_swap() {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 2.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mut mm_bob = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",
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
            "dht": "on",
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
        "max": true,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "sell",
        "base": "MYCOIN1",
        "rel": "MYCOIN",
        "price": 1,
        "volume": {
            "numer":"77699596737",
            "denom":"77800000000"
        },
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);

    block_on(mm_bob.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
    thread::sleep(Duration::from_secs(6));

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "sell",
        "base": "MYCOIN1",
        "rel": "MYCOIN",
        "price": 1,
        "volume": {
            "numer":"77699599999",
            "denom":"77800000000"
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "sell success, but should fail: {}", rc.1);
    assert!(rc.1.contains("Not enough MYCOIN1 for swap"), "{}", rc.1);
    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_buy_max() {
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 1.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mm_alice = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",
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
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": {
            "numer":"77699596737",
            "denom":"77800000000"
        },
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": {
            "numer":"77699596738",
            "denom":"77800000000"
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "buy success, but should fail: {}", rc.1);
    block_on(mm_alice.stop()).unwrap();
}
