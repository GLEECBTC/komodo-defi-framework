// UTXO Swaps V1 Tests
//
// This module contains UTXO-only swap tests that were extracted from docker_tests_inner.rs
// These tests focus on UTXO swap mechanics, payment lifecycle, and related functionality.
// They do NOT require ETH/ERC20 containers - only MYCOIN/MYCOIN1 UTXO containers.
//
// Gated by: docker-tests-swaps

use crate::docker_tests::helpers::env::random_secp256k1_secret;
use crate::docker_tests::helpers::swap::trade_base_rel;
use crate::docker_tests::helpers::utxo::{
    fill_address, generate_utxo_coin_with_privkey, generate_utxo_coin_with_random_privkey, rmd160_from_priv,
    utxo_coin_from_privkey,
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
use serde_json::{json, Value as Json};
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
        other_pubkey: &my_public_key,
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
        other_pubkey: &my_public_key,
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

    let pubkey = coin.my_public_key().unwrap();
    let search_input = SearchForSwapTxSpendInput {
        time_lock,
        other_pub: &pubkey,
        secret_hash: &[0; 20],
        tx: &tx.tx_hex(),
        search_from_block: 0,
        swap_contract_address: &None,
        swap_unique_data: &[],
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
        other_pubkey: &my_public_key,
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
        other_pubkey: &my_public_key,
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

    let pubkey = coin.my_public_key().unwrap();
    let search_input = SearchForSwapTxSpendInput {
        time_lock,
        other_pub: &pubkey,
        secret_hash: &[0; 20],
        tx: &tx.tx_hex(),
        search_from_block: 0,
        swap_contract_address: &None,
        swap_unique_data: &[],
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
        other_pubkey: &my_pubkey,
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
        other_pubkey: &my_pubkey,
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

    let pubkey = coin.my_public_key().unwrap();
    let search_input = SearchForSwapTxSpendInput {
        time_lock,
        other_pub: &pubkey,
        secret_hash: &*dhash160(&secret),
        tx: &tx.tx_hex(),
        search_from_block: 0,
        swap_contract_address: &None,
        swap_unique_data: &[],
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
        other_pubkey: &my_pubkey,
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
        other_pubkey: &my_pubkey,
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

    let pubkey = coin.my_public_key().unwrap();
    let search_input = SearchForSwapTxSpendInput {
        time_lock,
        other_pub: &pubkey,
        secret_hash: &*dhash160(&secret),
        tx: &tx.tx_hex(),
        search_from_block: 0,
        swap_contract_address: &None,
        swap_unique_data: &[],
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
            other_pubkey: &my_pubkey,
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
    // With 2% fee rate: max_vol = (balance - tx_fee) / 1.02
    // balance = 1, tx_fee varies based on UTXO, so max_vol ≈ 0.98
    let expected = MmNumber::from((99999481u64, 102000000u64)).to_fraction();
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
    // With 2% fee rate: max_vol = (balance - tx_fee) / 1.02
    // balance = 0.00532845, tx_fee varies based on UTXO
    assert_eq!(json["result"]["numer"], Json::from("35177"));
    assert_eq!(json["result"]["denom"], Json::from("6800000"));

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
    // With 2% fee rate (KMD discount removed): max_vol = (balance - tx_fee) / 1.02
    // balance = 1, tx_fee varies based on UTXO, so max_vol ≈ 0.9999
    assert_eq!(json["result"]["numer"], Json::from("9999481"));
    assert_eq!(json["result"]["denom"], Json::from("10200000"));

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

    // With 2% fee rate: locked = volume + dex_fee + tx_fees
    // = 777 + (777 * 0.02) + 0.00000519 = 777 + 15.54 + 0.00000519 = 792.54000519
    let expected_result: MmNumberMultiRepr = MmNumber::from("792.54000519").into();
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

    // With 2% fee rate: max_vol = (balance - tx_fee) / 1.02 = 49999863/51000000
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": {
            "numer":"49999863",
            "denom":"51000000"
        },
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    block_on(mm_bob.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
    thread::sleep(Duration::from_secs(6));

    // Second buy should fail because coins are locked by first swap
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": {
            "numer":"49999864",
            "denom":"51000000"
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

    // With 2% fee rate: max_vol = (balance - tx_fee) / 1.02 = 49999863/51000000
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "sell",
        "base": "MYCOIN1",
        "rel": "MYCOIN",
        "price": 1,
        "volume": {
            "numer":"49999863",
            "denom":"51000000"
        },
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);

    block_on(mm_bob.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
    thread::sleep(Duration::from_secs(6));

    // Second sell should fail because coins are locked by first swap
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "sell",
        "base": "MYCOIN1",
        "rel": "MYCOIN",
        "price": 1,
        "volume": {
            "numer":"49999864",
            "denom":"51000000"
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
    // With 2% fee rate: max_vol = (balance - tx_fee) / 1.02 = 99999481/102000000
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": {
            "numer":"99999481",
            "denom":"102000000"
        },
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    // Slightly more than max should fail
    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": {
            "numer":"99999482",
            "denom":"102000000"
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "buy success, but should fail: {}", rc.1);
    block_on(mm_alice.stop()).unwrap();
}

// =============================================================================
// Setprice Max Volume Tests
// Tests for setprice with max parameter and volume calculations
// =============================================================================

#[test]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/471
fn test_match_and_trade_setprice_max() {
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
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "max": true,
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
    assert_eq!(asks[0]["maxvolume"], Json::from("999.99999726"));

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": 1,
        "volume": "999.99999",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    block_on(mm_bob.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop MYCOIN/MYCOIN1"))).unwrap();

    thread::sleep(Duration::from_secs(3));

    let rmd160 = rmd160_from_priv(bob_priv_key);
    let order_path = mm_bob.folder.join(format!(
        "DB/{}/ORDERS/MY/MAKER/{}.json",
        hex::encode(rmd160.take()),
        bob_uuid,
    ));
    log!("Order path {}", order_path.display());
    assert!(!order_path.exists());
    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
// https://github.com/KomodoPlatform/atomicDEX-API/issues/888
fn test_max_taker_vol_swap() {
    let (_ctx, _, bob_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000.into());
    let (_ctx, _, alice_priv_key) = generate_utxo_coin_with_random_privkey("MYCOIN1", 50.into());
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mut mm_bob = block_on(MarketMakerIt::start_with_envs(
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
        &[("MYCOIN_FEE_DISCOUNT", "")],
    ))
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);
    block_on(mm_bob.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();

    let mut mm_alice = block_on(MarketMakerIt::start_with_envs(
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
        &[("MYCOIN_FEE_DISCOUNT", "")],
    ))
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);
    block_on(mm_alice.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();

    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));
    let price = MmNumber::from((100, 1620));
    let rc = block_on(mm_bob.rpc(&json!({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "price": price,
        "max": true,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": "MYCOIN1",
        "rel": "MYCOIN",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);
    log!("{}", rc.1);
    thread::sleep(Duration::from_secs(3));

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "max_taker_vol",
        "coin": "MYCOIN1",
        "trade_with": "MYCOIN",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!max_taker_vol: {}", rc.1);
    let vol: MaxTakerVolResponse = serde_json::from_str(&rc.1).unwrap();
    // With 1% fee rate (MYCOIN_FEE_DISCOUNT): max_vol = (balance - tx_fee) / 1.01
    // balance = 50, tx_fee varies, so max_vol ≈ 49.50
    let expected_vol = MmNumber::from((4999999481u64, 101000000u64));

    let actual_vol = MmNumber::from(vol.result.clone());
    log!("actual vol {}", actual_vol.to_decimal());
    log!("expected vol {}", expected_vol.to_decimal());

    assert_eq!(expected_vol, actual_vol);

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "sell",
        "base": "MYCOIN1",
        "rel": "MYCOIN",
        "price": "16",
        "volume": vol.result,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);
    let sell_res: BuyOrSellRpcResult = serde_json::from_str(&rc.1).unwrap();

    block_on(mm_bob.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop MYCOIN/MYCOIN1"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop MYCOIN/MYCOIN1"))).unwrap();

    thread::sleep(Duration::from_secs(3));

    let rc = block_on(mm_alice.rpc(&json!({
        "userpass": mm_alice.userpass,
        "method": "my_swap_status",
        "params": {
            "uuid": sell_res.result.uuid
        }
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!my_swap_status: {}", rc.1);

    let status_response: Json = serde_json::from_str(&rc.1).unwrap();
    let events_array = status_response["result"]["events"].as_array().unwrap();
    let first_event_type = events_array[0]["event"]["type"].as_str().unwrap();
    assert_eq!("Started", first_event_type);
    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

// =============================================================================
// Trade Preimage Tests
// Tests for trade_preimage RPC - fee estimation before swap execution
// =============================================================================

#[test]
fn test_maker_trade_preimage() {
    let priv_key = random_secp256k1_secret();

    let (_ctx, mycoin) = utxo_coin_from_privkey("MYCOIN", priv_key);
    let my_address = mycoin.my_address().expect("!my_address");
    fill_address(&mycoin, &my_address, 10.into(), 30);

    let (_ctx, mycoin1) = utxo_coin_from_privkey("MYCOIN1", priv_key);
    let my_address = mycoin1.my_address().expect("!my_address");
    fill_address(&mycoin1, &my_address, 20.into(), 30);

    let coins = json!([mycoin_conf(1000), mycoin1_conf(2000)]);
    let mm = MarketMakerIt::start(
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
    let (_dump_log, _dump_dashboard) = mm_dump(&mm.log_path);

    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "MYCOIN",
            "rel": "MYCOIN1",
            "swap_method": "setprice",
            "price": 1,
            "max": true,
        },
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!trade_preimage: {}", rc.1);
    let base_coin_fee = TradeFeeForTest::new("MYCOIN", "0.00000274", false); // txfee from get_sender_trade_fee
    let rel_coin_fee = TradeFeeForTest::new("MYCOIN1", "0.00000992", true);
    let volume = MmNumber::from("9.99999726"); // 1.0 - 0.00000274 from calc_max_maker_vol

    let my_coin_total = TotalTradeFeeForTest::new("MYCOIN", "0.00000274", "0.00000274");
    let my_coin1_total = TotalTradeFeeForTest::new("MYCOIN1", "0.00000992", "0");

    let expected = TradePreimageResult::MakerPreimage(MakerPreimage {
        base_coin_fee,
        rel_coin_fee,
        volume: Some(volume.to_decimal()),
        volume_rat: Some(volume.to_ratio()),
        volume_fraction: Some(volume.to_fraction()),
        total_fees: vec![my_coin_total, my_coin1_total],
    });

    let mut actual: RpcSuccessResponse<TradePreimageResult> = serde_json::from_str(&rc.1).unwrap();
    actual.result.sort_total_fees();
    assert_eq!(expected, actual.result);

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "MYCOIN1",
            "rel": "MYCOIN",
            "swap_method": "setprice",
            "price": 1,
            "max": true,
        },
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!trade_preimage: {}", rc.1);
    let mut actual: RpcSuccessResponse<TradePreimageResult> = serde_json::from_str(&rc.1).unwrap();
    actual.result.sort_total_fees();

    let base_coin_fee = TradeFeeForTest::new("MYCOIN1", "0.00000548", false);
    let rel_coin_fee = TradeFeeForTest::new("MYCOIN", "0.00000496", true);
    let volume = MmNumber::from("19.99999452");

    let my_coin_total = TotalTradeFeeForTest::new("MYCOIN", "0.00000496", "0");
    let my_coin1_total = TotalTradeFeeForTest::new("MYCOIN1", "0.00000548", "0.00000548");
    let expected = TradePreimageResult::MakerPreimage(MakerPreimage {
        base_coin_fee,
        rel_coin_fee,
        volume: Some(volume.to_decimal()),
        volume_rat: Some(volume.to_ratio()),
        volume_fraction: Some(volume.to_fraction()),
        total_fees: vec![my_coin_total, my_coin1_total],
    });

    actual.result.sort_total_fees();
    assert_eq!(expected, actual.result);

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "MYCOIN1",
            "rel": "MYCOIN",
            "swap_method": "setprice",
            "price": 1,
            "volume": "19.99999109", // actually try max value (balance - txfee = 20.0 - 0.00000823)
        },
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!trade_preimage: {}", rc.1);
    let mut actual: RpcSuccessResponse<TradePreimageResult> = serde_json::from_str(&rc.1).unwrap();
    actual.result.sort_total_fees();

    let base_coin_fee = TradeFeeForTest::new("MYCOIN1", "0.00000891", false); // txfee updated for calculated max volume (not 616)
    let rel_coin_fee = TradeFeeForTest::new("MYCOIN", "0.00000496", true);

    let total_my_coin = TotalTradeFeeForTest::new("MYCOIN", "0.00000496", "0");
    let total_my_coin1 = TotalTradeFeeForTest::new("MYCOIN1", "0.00000891", "0.00000891");

    let expected = TradePreimageResult::MakerPreimage(MakerPreimage {
        base_coin_fee,
        rel_coin_fee,
        volume: None,
        volume_rat: None,
        volume_fraction: None,
        total_fees: vec![total_my_coin, total_my_coin1],
    });

    actual.result.sort_total_fees();
    assert_eq!(expected, actual.result);
}

#[test]
fn test_taker_trade_preimage() {
    let priv_key = random_secp256k1_secret();

    let (_ctx, mycoin) = utxo_coin_from_privkey("MYCOIN", priv_key);
    let my_address = mycoin.my_address().expect("!my_address");
    fill_address(&mycoin, &my_address, 10.into(), 30);

    let (_ctx, mycoin1) = utxo_coin_from_privkey("MYCOIN1", priv_key);
    let my_address = mycoin1.my_address().expect("!my_address");
    fill_address(&mycoin1, &my_address, 20.into(), 30);

    let coins = json!([mycoin_conf(1000), mycoin1_conf(2000)]);
    let mm = MarketMakerIt::start(
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
    let (_dump_log, _dump_dashboard) = mm_dump(&mm.log_path);

    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));

    // `max` field is not supported for `buy/sell` swap methods
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "MYCOIN",
            "rel": "MYCOIN1",
            "swap_method": "sell",
            "max": true,
            "price": 1,
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "trade_preimage success, but should fail: {}", rc.1);

    let actual: RpcErrorResponse<trade_preimage_error::InvalidParam> = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(actual.error_type, "InvalidParam", "Unexpected error_type: {}", rc.1);
    let expected = trade_preimage_error::InvalidParam {
        param: "max".to_owned(),
        reason: "'max' cannot be used with 'sell' or 'buy' method".to_owned(),
    };
    assert_eq!(actual.error_data, Some(expected));

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "MYCOIN",
            "rel": "MYCOIN1",
            "swap_method": "sell",
            "volume": "7.77",
            "price": "2",
        },
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!trade_preimage: {}", rc.1);

    let mut actual: RpcSuccessResponse<TradePreimageResult> = serde_json::from_str(&rc.1).unwrap();
    actual.result.sort_total_fees();

    let base_coin_fee = TradeFeeForTest::new("MYCOIN", "0.00000274", false);
    let rel_coin_fee = TradeFeeForTest::new("MYCOIN1", "0.00000992", true);
    // With 2% fee rate: dex_fee = 7.77 * 0.02 = 0.1554
    let taker_fee = TradeFeeForTest::new("MYCOIN", "0.1554", false);
    let fee_to_send_taker_fee = TradeFeeForTest::new("MYCOIN", "0.00000245", false);

    // total = taker_fee + base_coin_fee + fee_to_send_taker_fee = 0.1554 + 0.00000274 + 0.00000245 = 0.15540519
    let my_coin_total_fee = TotalTradeFeeForTest::new("MYCOIN", "0.15540519", "0.15540519");
    let my_coin1_total_fee = TotalTradeFeeForTest::new("MYCOIN1", "0.00000992", "0");

    let expected = TradePreimageResult::TakerPreimage(TakerPreimage {
        base_coin_fee,
        rel_coin_fee,
        taker_fee,
        fee_to_send_taker_fee,
        total_fees: vec![my_coin_total_fee, my_coin1_total_fee],
    });
    assert_eq!(expected, actual.result);

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "MYCOIN",
            "rel": "MYCOIN1",
            "swap_method": "buy",
            "volume": "7.77",
            "price": "2",
        },
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!trade_preimage: {}", rc.1);
    let mut actual: RpcSuccessResponse<TradePreimageResult> = serde_json::from_str(&rc.1).unwrap();
    actual.result.sort_total_fees();

    let base_coin_fee = TradeFeeForTest::new("MYCOIN", "0.00000496", true);
    let rel_coin_fee = TradeFeeForTest::new("MYCOIN1", "0.00000548", false); // fee to send taker payment     // With 2% fee rate: buy 7.77 MYCOIN at price 2 = spend 15.54 MYCOIN1, dex_fee = 15.54 * 0.02 = 0.3108
    let taker_fee = TradeFeeForTest::new("MYCOIN1", "0.3108", false);
    let fee_to_send_taker_fee = TradeFeeForTest::new("MYCOIN1", "0.0000049", false);

    let my_coin_total_fee = TotalTradeFeeForTest::new("MYCOIN", "0.00000496", "0");
    // total = taker_fee + rel_coin_fee + fee_to_send_taker_fee = 0.3108 + 0.00000548 + 0.0000049 = 0.31081038
    let my_coin1_total_fee = TotalTradeFeeForTest::new("MYCOIN1", "0.31081038", "0.31081038");

    let expected = TradePreimageResult::TakerPreimage(TakerPreimage {
        base_coin_fee,
        rel_coin_fee,
        taker_fee,
        fee_to_send_taker_fee,
        total_fees: vec![my_coin_total_fee, my_coin1_total_fee],
    });
    assert_eq!(expected, actual.result);
}

#[test]
fn test_trade_preimage_not_sufficient_balance() {
    #[track_caller]
    fn expect_not_sufficient_balance(
        res: &str,
        available: BigDecimal,
        required: BigDecimal,
        locked_by_swaps: Option<BigDecimal>,
    ) {
        let actual: RpcErrorResponse<trade_preimage_error::NotSufficientBalance> = serde_json::from_str(res).unwrap();
        assert_eq!(actual.error_type, "NotSufficientBalance");
        let expected = trade_preimage_error::NotSufficientBalance {
            coin: "MYCOIN".to_owned(),
            available,
            required,
            locked_by_swaps,
        };
        assert_eq!(actual.error_data, Some(expected));
    }

    let priv_key = random_secp256k1_secret();
    let fill_balance_functor = |amount: BigDecimal| {
        let (_ctx, mycoin) = utxo_coin_from_privkey("MYCOIN", priv_key);
        let my_address = mycoin.my_address().expect("!my_address");
        fill_address(&mycoin, &my_address, amount, 30);
    };

    let coins = json!([mycoin_conf(1000), mycoin1_conf(2000)]);
    let mm = MarketMakerIt::start(
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
    let (_dump_log, _dump_dashboard) = mm_dump(&mm.log_path);

    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));

    fill_balance_functor(MmNumber::from("0.00001273").to_decimal()); // volume < txfee + dust = 274 + 1000
                                                                     // Try sell the max amount with the zero balance.
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "MYCOIN",
            "rel": "MYCOIN1",
            "swap_method": "setprice",
            "price": 1,
            "max": true,
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "trade_preimage success, but should fail: {}", rc.1);
    let available = MmNumber::from("0.00001273").to_decimal();
    // Required at least 0.00001274 MYCOIN to pay the transaction_fee(0.00000274) and to send a value not less than dust(0.00001) and not less than min_trading_vol (10 * dust).
    let required = MmNumber::from("0.00001274").to_decimal(); // TODO: this is not true actually: we can't create orders less that min_trading_vol = 10 * dust
    expect_not_sufficient_balance(&rc.1, available, required, Some(MmNumber::from("0").to_decimal()));

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "MYCOIN",
            "rel": "MYCOIN1",
            "swap_method": "setprice",
            "price": 1,
            "volume": 0.1,
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "trade_preimage success, but should fail: {}", rc.1);
    // Required 0.00001 MYCOIN to pay the transaction fee and the specified 0.1 volume.
    let available = MmNumber::from("0.00001273").to_decimal();
    let required = MmNumber::from("0.1000024").to_decimal();
    expect_not_sufficient_balance(&rc.1, available, required, None);

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "MYCOIN",
            "rel": "MYCOIN1",
            "swap_method": "setprice",
            "price": 1,
            "max": true,
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "trade_preimage success, but should fail: {}", rc.1);
    // balance(0.00001273)
    let available = MmNumber::from("0.00001273").to_decimal();
    // required min_tx_amount(0.00001) + transaction_fee(0.00000274)
    let required = MmNumber::from("0.00001274").to_decimal();
    expect_not_sufficient_balance(&rc.1, available, required, Some(MmNumber::from("0").to_decimal()));

    fill_balance_functor(MmNumber::from("7.770085").to_decimal());
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "MYCOIN",
            "rel": "MYCOIN1",
            "swap_method": "sell",
            "price": 1,
            "volume": 7.77,
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "trade_preimage success, but should fail: {}", rc.1);
    let available = MmNumber::from("7.77009773").to_decimal();
    // `required = volume + fee_to_send_taker_payment + dex_fee + fee_to_send_dex_fee`,
    // where `volume = 7.77`, `fee_to_send_taker_payment = 0.00000393, fee_to_send_dex_fee = 0.00000422`.
    // With 2% fee rate: dex_fee = 7.77 * 0.02 = 0.1554
    // required = 7.77 + 0.1554 (dex_fee) + (0.00000393 + 0.00000422) = 7.92540815
    let required = MmNumber::from("7.92540815");
    expect_not_sufficient_balance(&rc.1, available, required.to_decimal(), Some(BigDecimal::from(0)));
}

/// This test ensures that `trade_preimage` will not succeed on input that will fail on `buy/sell/setprice`.
/// https://github.com/KomodoPlatform/atomicDEX-API/issues/902
#[test]
fn test_trade_preimage_additional_validation() {
    let priv_key = random_secp256k1_secret();

    let (_ctx, mycoin1) = utxo_coin_from_privkey("MYCOIN1", priv_key);
    let my_address = mycoin1.my_address().expect("!my_address");
    fill_address(&mycoin1, &my_address, 20.into(), 30);

    let (_ctx, mycoin) = utxo_coin_from_privkey("MYCOIN", priv_key);
    let my_address = mycoin.my_address().expect("!my_address");
    fill_address(&mycoin, &my_address, 10.into(), 30);

    let coins = json!([mycoin_conf(1000), mycoin1_conf(2000)]);

    let mm = MarketMakerIt::start(
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
    let (_dump_log, _dump_dashboard) = mm_dump(&mm.log_path);

    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));

    // Price is too low
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "MYCOIN",
            "rel": "MYCOIN1",
            "swap_method": "setprice",
            "price": 0,
            "volume": 0.1,
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "trade_preimage success, but should fail: {}", rc.1);
    let actual: RpcErrorResponse<trade_preimage_error::PriceTooLow> = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(actual.error_type, "PriceTooLow");
    // currently the minimum price is any value above 0
    let expected = trade_preimage_error::PriceTooLow {
        price: BigDecimal::from(0),
        threshold: BigDecimal::from(0),
    };
    assert_eq!(actual.error_data, Some(expected));

    // volume 0.00001 is too low, min trading volume 0.0001
    let low_volume = BigDecimal::from(1) / BigDecimal::from(100_000);

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "MYCOIN",
            "rel": "MYCOIN1",
            "swap_method": "setprice",
            "price": 1,
            "volume": low_volume,
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "trade_preimage success, but should fail: {}", rc.1);
    let actual: RpcErrorResponse<trade_preimage_error::VolumeTooLow> = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(actual.error_type, "VolumeTooLow");
    // Min MYCOIN trading volume is 0.0001.
    let volume_threshold = BigDecimal::from(1) / BigDecimal::from(10_000);
    let expected = trade_preimage_error::VolumeTooLow {
        coin: "MYCOIN".to_owned(),
        volume: low_volume.clone(),
        threshold: volume_threshold,
    };
    assert_eq!(actual.error_data, Some(expected));

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "MYCOIN",
            "rel": "MYCOIN1",
            "swap_method": "sell",
            "price": 1,
            "volume": low_volume,
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "trade_preimage success, but should fail: {}", rc.1);
    let actual: RpcErrorResponse<trade_preimage_error::VolumeTooLow> = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(actual.error_type, "VolumeTooLow");
    // Min MYCOIN trading volume is 0.0001.
    let volume_threshold = BigDecimal::from(1) / BigDecimal::from(10_000);
    let expected = trade_preimage_error::VolumeTooLow {
        coin: "MYCOIN".to_owned(),
        volume: low_volume,
        threshold: volume_threshold,
    };
    assert_eq!(actual.error_data, Some(expected));

    // rel volume is too low
    // Min MYCOIN trading volume is 0.0001.
    let volume = BigDecimal::from(1) / BigDecimal::from(10_000);
    let low_price = BigDecimal::from(1) / BigDecimal::from(10);
    // Min MYCOIN1 trading volume is 0.0001, but the actual volume is 0.00001
    let low_rel_volume = &volume * &low_price;
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "trade_preimage",
        "params": {
            "base": "MYCOIN",
            "rel": "MYCOIN1",
            "swap_method": "sell",
            "price": low_price,
            "volume": volume,
        },
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "trade_preimage success, but should fail: {}", rc.1);
    let actual: RpcErrorResponse<trade_preimage_error::VolumeTooLow> = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(actual.error_type, "VolumeTooLow");
    // Min MYCOIN1 trading volume is 0.0001.
    let volume_threshold = BigDecimal::from(1) / BigDecimal::from(10_000);
    let expected = trade_preimage_error::VolumeTooLow {
        coin: "MYCOIN1".to_owned(),
        volume: low_rel_volume,
        threshold: volume_threshold,
    };
    assert_eq!(actual.error_data, Some(expected));
}

#[test]
fn test_trade_preimage_legacy() {
    let priv_key = random_secp256k1_secret();
    let (_ctx, mycoin) = utxo_coin_from_privkey("MYCOIN", priv_key);
    let my_address = mycoin.my_address().expect("!my_address");
    fill_address(&mycoin, &my_address, 10.into(), 30);
    let (_ctx, mycoin1) = utxo_coin_from_privkey("MYCOIN1", priv_key);
    let my_address = mycoin1.my_address().expect("!my_address");
    fill_address(&mycoin1, &my_address, 20.into(), 30);

    let coins = json!([mycoin_conf(1000), mycoin1_conf(2000)]);
    let mm = MarketMakerIt::start(
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
    let (_dump_log, _dump_dashboard) = mm_dump(&mm.log_path);

    log!("{:?}", block_on(enable_native(&mm, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm, "MYCOIN", &[], None)));

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "trade_preimage",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "swap_method": "setprice",
        "max": true,
        "price": "1",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!trade_preimage: {}", rc.1);
    let _: TradePreimageResponse = serde_json::from_str(&rc.1).unwrap();

    // vvv test a taker method vvv

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "trade_preimage",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "swap_method": "sell",
        "volume": "7.77",
        "price": "2",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!trade_preimage: {}", rc.1);
    let _: TradePreimageResponse = serde_json::from_str(&rc.1).unwrap();

    // vvv test the error response vvv

    // `max` field is not supported for `buy/sell` swap methods
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "trade_preimage",
        "base": "MYCOIN",
        "rel": "MYCOIN1",
        "swap_method": "sell",
        "max": true,
        "price": "1",
    })))
    .unwrap();
    assert!(!rc.0.is_success(), "trade_preimage success, but should fail: {}", rc.1);
    assert!(rc
        .1
        .contains("Incorrect use of the 'max' parameter: 'max' cannot be used with 'sell' or 'buy' method"));
}
