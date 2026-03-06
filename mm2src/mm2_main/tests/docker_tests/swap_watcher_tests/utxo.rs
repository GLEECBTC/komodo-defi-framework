//! UTXO-only Watcher Tests
//!
//! Tests for watcher node functionality with UTXO coins only.

use super::*;

#[test]
fn test_taker_saves_the_swap_as_successful_after_restart_panic_at_wait_for_taker_payment_spend() {
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mut mm_bob = run_maker_node(&coins, &[], &[], None);
    let (mut mm_alice, mut alice_conf) = run_taker_node(
        &coins,
        &[("TAKER_FAIL_AT", "wait_for_taker_payment_spend_panic")],
        &[&mm_bob.ip.to_string()],
        None,
    );

    let watcher_conf = WatcherConf {
        wait_taker_payment: 0.,
        wait_maker_payment_spend_factor: 0.,
        refund_start_factor: 1.5,
        search_interval: 1.0,
    };
    let mut mm_watcher = run_watcher_node(&coins, &[], &[&mm_bob.ip.to_string()], watcher_conf, None);

    let uuids = block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[("MYCOIN1", "MYCOIN")],
        25.,
        25.,
        2.,
    ));
    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());
    alice_conf.conf["dbdir"] = mm_alice.folder.join("DB").to_str().unwrap().into();

    block_on(mm_alice.wait_for_log(120., |log| log.contains(WATCHER_MESSAGE_SENT_LOG))).unwrap();
    block_on(mm_bob.wait_for_log(120., |log| log.contains(&format!("[swap uuid={}] Finished", &uuids[0])))).unwrap();
    block_on(mm_watcher.wait_for_log(120., |log| log.contains(MAKER_PAYMENT_SPEND_SENT_LOG))).unwrap();

    block_on(mm_alice.stop()).unwrap();

    let mm_alice = restart_taker_and_wait_until(&alice_conf, &[], &format!("[swap uuid={}] Finished", &uuids[0]));

    let expected_events = [
        "Started",
        "Negotiated",
        "TakerFeeSent",
        "TakerPaymentInstructionsReceived",
        "MakerPaymentReceived",
        "MakerPaymentWaitConfirmStarted",
        "MakerPaymentValidatedAndConfirmed",
        "TakerPaymentSent",
        "WatcherMessageSent",
        "TakerPaymentSpent",
        "MakerPaymentSpentByWatcher",
        "MakerPaymentSpendConfirmed",
        "Finished",
    ];
    check_actual_events(&mm_alice, &uuids[0], &expected_events);

    block_on(mm_alice.stop()).unwrap();
    block_on(mm_watcher.stop()).unwrap();
    block_on(mm_bob.stop()).unwrap();
}

#[test]
fn test_taker_saves_the_swap_as_successful_after_restart_panic_at_maker_payment_spend() {
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mut mm_bob = run_maker_node(&coins, &[], &[], None);
    let (mut mm_alice, mut alice_conf) = run_taker_node(
        &coins,
        &[("TAKER_FAIL_AT", "maker_payment_spend_panic")],
        &[&mm_bob.ip.to_string()],
        None,
    );

    let watcher_conf = WatcherConf {
        wait_taker_payment: 0.,
        wait_maker_payment_spend_factor: 0.,
        refund_start_factor: 1.5,
        search_interval: 1.0,
    };
    let mut mm_watcher = run_watcher_node(&coins, &[], &[&mm_bob.ip.to_string()], watcher_conf, None);

    let uuids = block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[("MYCOIN1", "MYCOIN")],
        25.,
        25.,
        2.,
    ));
    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());
    alice_conf.conf["dbdir"] = mm_alice.folder.join("DB").to_str().unwrap().into();

    block_on(mm_alice.wait_for_log(120., |log| log.contains(WATCHER_MESSAGE_SENT_LOG))).unwrap();
    block_on(mm_bob.wait_for_log(120., |log| log.contains(&format!("[swap uuid={}] Finished", &uuids[0])))).unwrap();
    block_on(mm_watcher.wait_for_log(120., |log| log.contains(MAKER_PAYMENT_SPEND_SENT_LOG))).unwrap();

    block_on(mm_alice.stop()).unwrap();

    let mm_alice = restart_taker_and_wait_until(&alice_conf, &[], &format!("[swap uuid={}] Finished", &uuids[0]));

    let expected_events = [
        "Started",
        "Negotiated",
        "TakerFeeSent",
        "TakerPaymentInstructionsReceived",
        "MakerPaymentReceived",
        "MakerPaymentWaitConfirmStarted",
        "MakerPaymentValidatedAndConfirmed",
        "TakerPaymentSent",
        "WatcherMessageSent",
        "TakerPaymentSpent",
        "MakerPaymentSpentByWatcher",
        "MakerPaymentSpendConfirmed",
        "Finished",
    ];
    check_actual_events(&mm_alice, &uuids[0], &expected_events);
}

#[test]
fn test_taker_saves_the_swap_as_finished_after_restart_taker_payment_refunded_panic_at_wait_for_taker_payment_spend() {
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mm_seednode = run_maker_node(&coins, &[], &[], Some(60));
    let mut mm_bob = run_maker_node(&coins, &[], &[&mm_seednode.ip.to_string()], Some(60));
    let (mut mm_alice, mut alice_conf) = run_taker_node(
        &coins,
        &[("TAKER_FAIL_AT", "wait_for_taker_payment_spend_panic")],
        &[&mm_seednode.ip.to_string()],
        Some(60),
    );

    let watcher_conf = WatcherConf {
        wait_taker_payment: 0.,
        wait_maker_payment_spend_factor: 1.,
        refund_start_factor: 0.,
        search_interval: 1.,
    };
    let mut mm_watcher = run_watcher_node(&coins, &[], &[&mm_seednode.ip.to_string()], watcher_conf, Some(60));

    let uuids = block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[("MYCOIN1", "MYCOIN")],
        25.,
        25.,
        2.,
    ));
    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());
    alice_conf.conf["dbdir"] = mm_alice.folder.join("DB").to_str().unwrap().into();

    block_on(mm_bob.wait_for_log(120., |log| log.contains(MAKER_PAYMENT_SENT_LOG))).unwrap();
    block_on(mm_bob.stop()).unwrap();

    block_on(mm_alice.wait_for_log(120., |log| log.contains(WATCHER_MESSAGE_SENT_LOG))).unwrap();
    block_on(mm_watcher.wait_for_log(120., |log| log.contains(TAKER_PAYMENT_REFUND_SENT_LOG))).unwrap();

    block_on(mm_alice.stop()).unwrap();

    let mm_alice = restart_taker_and_wait_until(&alice_conf, &[], &format!("[swap uuid={}] Finished", &uuids[0]));

    let expected_events = [
        "Started",
        "Negotiated",
        "TakerFeeSent",
        "TakerPaymentInstructionsReceived",
        "MakerPaymentReceived",
        "MakerPaymentWaitConfirmStarted",
        "MakerPaymentValidatedAndConfirmed",
        "TakerPaymentSent",
        "WatcherMessageSent",
        "TakerPaymentRefundedByWatcher",
        "Finished",
    ];
    check_actual_events(&mm_alice, &uuids[0], &expected_events);

    block_on(mm_alice.stop()).unwrap();
    block_on(mm_watcher.stop()).unwrap();
    block_on(mm_seednode.stop()).unwrap();
}

#[test]
fn test_taker_saves_the_swap_as_finished_after_restart_taker_payment_refunded_panic_at_taker_payment_refund() {
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mm_seednode = run_maker_node(&coins, &[], &[], Some(60));
    let mut mm_bob = run_maker_node(&coins, &[], &[&mm_seednode.ip.to_string()], Some(60));
    let (mut mm_alice, mut alice_conf) = run_taker_node(
        &coins,
        &[("TAKER_FAIL_AT", "taker_payment_refund_panic")],
        &[&mm_seednode.ip.to_string()],
        Some(60),
    );

    let watcher_conf = WatcherConf {
        wait_taker_payment: 0.,
        wait_maker_payment_spend_factor: 1.,
        refund_start_factor: 0.,
        search_interval: 1.,
    };
    let mut mm_watcher = run_watcher_node(&coins, &[], &[&mm_seednode.ip.to_string()], watcher_conf, Some(60));

    let uuids = block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[("MYCOIN1", "MYCOIN")],
        25.,
        25.,
        2.,
    ));
    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());
    alice_conf.conf["dbdir"] = mm_alice.folder.join("DB").to_str().unwrap().into();

    block_on(mm_bob.wait_for_log(120., |log| log.contains(MAKER_PAYMENT_SENT_LOG))).unwrap();
    block_on(mm_bob.stop()).unwrap();

    block_on(mm_alice.wait_for_log(120., |log| log.contains(REFUND_TEST_FAILURE_LOG))).unwrap();
    block_on(mm_watcher.wait_for_log(120., |log| log.contains(TAKER_PAYMENT_REFUND_SENT_LOG))).unwrap();

    block_on(mm_alice.stop()).unwrap();

    let mm_alice = restart_taker_and_wait_until(&alice_conf, &[], &format!("[swap uuid={}] Finished", &uuids[0]));

    let expected_events = [
        "Started",
        "Negotiated",
        "TakerFeeSent",
        "TakerPaymentInstructionsReceived",
        "MakerPaymentReceived",
        "MakerPaymentWaitConfirmStarted",
        "MakerPaymentValidatedAndConfirmed",
        "TakerPaymentSent",
        "WatcherMessageSent",
        "TakerPaymentWaitForSpendFailed",
        "TakerPaymentWaitRefundStarted",
        "TakerPaymentRefundStarted",
        "TakerPaymentRefundedByWatcher",
        "Finished",
    ];
    check_actual_events(&mm_alice, &uuids[0], &expected_events);

    block_on(mm_alice.stop()).unwrap();
    block_on(mm_watcher.stop()).unwrap();
    block_on(mm_seednode.stop()).unwrap();
}

#[test]
fn test_taker_completes_swap_after_restart() {
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mut mm_bob = run_maker_node(&coins, &[], &[], None);
    let (mut mm_alice, mut alice_conf) = run_taker_node(&coins, &[], &[&mm_bob.ip.to_string()], None);

    let uuids = block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[("MYCOIN1", "MYCOIN")],
        25.,
        25.,
        2.,
    ));

    block_on(mm_alice.wait_for_log(120., |log| log.contains(WATCHER_MESSAGE_SENT_LOG))).unwrap();
    alice_conf.conf["dbdir"] = mm_alice.folder.join("DB").to_str().unwrap().into();
    block_on(mm_alice.stop()).unwrap();

    let mut mm_alice = block_on(MarketMakerIt::start_with_envs(
        alice_conf.conf,
        alice_conf.rpc_password.clone(),
        None,
        &[],
    ))
    .unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());
    enable_coin(&mm_alice, "MYCOIN");
    enable_coin(&mm_alice, "MYCOIN1");

    block_on(wait_for_swaps_finish_and_check_status(
        &mut mm_bob,
        &mut mm_alice,
        &uuids,
        2.,
        25.,
    ));

    block_on(mm_alice.stop()).unwrap();
    block_on(mm_bob.stop()).unwrap();
}

// Verifies https://github.com/KomodoPlatform/komodo-defi-framework/issues/2111
#[test]
fn test_taker_completes_swap_after_taker_payment_spent_while_offline() {
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);
    let mut mm_bob = run_maker_node(&coins, &[], &[], None);
    let (mut mm_alice, mut alice_conf) = run_taker_node(&coins, &[], &[&mm_bob.ip.to_string()], None);

    let uuids = block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[("MYCOIN1", "MYCOIN")],
        25.,
        25.,
        2.,
    ));

    // stop taker after taker payment sent
    let taker_payment_msg = "Taker payment tx hash ";
    block_on(mm_alice.wait_for_log(120., |log| log.contains(taker_payment_msg))).unwrap();
    // ensure p2p message is sent to the maker, this happens before this message:
    block_on(mm_alice.wait_for_log(120., |log| log.contains("Waiting for maker to spend taker payment!"))).unwrap();
    alice_conf.conf["dbdir"] = mm_alice.folder.join("DB").to_str().unwrap().into();
    block_on(mm_alice.stop()).unwrap();

    // wait for taker payment spent by maker
    block_on(mm_bob.wait_for_log(120., |log| log.contains("Taker payment spend tx"))).unwrap();
    // and restart taker
    let mut mm_alice = block_on(MarketMakerIt::start_with_envs(
        alice_conf.conf,
        alice_conf.rpc_password.clone(),
        None,
        &[],
    ))
    .unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());
    enable_coin(&mm_alice, "MYCOIN");
    enable_coin(&mm_alice, "MYCOIN1");

    block_on(wait_for_swaps_finish_and_check_status(
        &mut mm_bob,
        &mut mm_alice,
        &uuids,
        2.,
        25.,
    ));

    block_on(mm_alice.stop()).unwrap();
    block_on(mm_bob.stop()).unwrap();
}

#[test]
fn test_watcher_spends_maker_payment_utxo_utxo() {
    let alice_privkey = hex::encode(random_secp256k1_secret());
    let bob_privkey = hex::encode(random_secp256k1_secret());
    let watcher_privkey = hex::encode(random_secp256k1_secret());

    let balances = start_swaps_and_get_balances(
        "MYCOIN",
        "MYCOIN1",
        25.,
        25.,
        2.,
        &[],
        SwapFlow::WatcherSpendsMakerPayment,
        &alice_privkey,
        &bob_privkey,
        &watcher_privkey,
        None,
    );

    let acoin_volume = BigDecimal::from_str("50").unwrap();
    let bcoin_volume = BigDecimal::from_str("2").unwrap();
    // DEX fee is 2% of taker volume (acoin_volume), paid by Alice (taker)
    let dex_fee = &acoin_volume * BigDecimal::from_str("0.02").unwrap();

    // Alice spends acoin_volume + dex_fee (as taker, she pays the DEX fee in the taker coin)
    assert_eq!(
        balances.alice_acoin_balance_after.round(0),
        balances.alice_acoin_balance_before.clone() - acoin_volume.clone() - dex_fee
    );
    assert_eq!(
        balances.alice_bcoin_balance_after.round(0),
        balances.alice_bcoin_balance_before + bcoin_volume.clone()
    );
    // Bob receives acoin_volume (no fee on his side)
    assert_eq!(
        balances.bob_acoin_balance_after.round(0),
        balances.bob_acoin_balance_before + acoin_volume
    );
    assert_eq!(
        balances.bob_bcoin_balance_after.round(0),
        balances.bob_bcoin_balance_before - bcoin_volume
    );
}

#[test]
fn test_watcher_refunds_taker_payment_utxo() {
    let alice_privkey = &hex::encode(random_secp256k1_secret());
    let bob_privkey = &hex::encode(random_secp256k1_secret());
    let watcher_privkey = &hex::encode(random_secp256k1_secret());

    let balances = start_swaps_and_get_balances(
        "MYCOIN1",
        "MYCOIN",
        25.,
        25.,
        2.,
        &[],
        SwapFlow::WatcherRefundsTakerPayment,
        alice_privkey,
        bob_privkey,
        watcher_privkey,
        Some(60),
    );

    // Alice's a_coin (MYCOIN1) balance is reduced by the DEX fee.
    // The taker fee is non-refundable even when the swap is refunded.
    // DEX fee is 2% of acoin_volume (50 MYCOIN1) = 1 MYCOIN1
    let acoin_volume = BigDecimal::from_str("50").unwrap();
    let dex_fee = &acoin_volume * BigDecimal::from_str("0.02").unwrap();
    assert_eq!(
        balances.alice_acoin_balance_after.round(0),
        balances.alice_acoin_balance_before.clone() - dex_fee
    );
    // Alice's b_coin (MYCOIN) balance should be unchanged - she got her taker payment refunded
    assert_eq!(balances.alice_bcoin_balance_after, balances.alice_bcoin_balance_before);
}

#[test]
fn test_watcher_waits_for_taker_utxo() {
    let alice_privkey = &hex::encode(random_secp256k1_secret());
    let bob_privkey = &hex::encode(random_secp256k1_secret());
    let watcher_privkey = &hex::encode(random_secp256k1_secret());

    start_swaps_and_get_balances(
        "MYCOIN1",
        "MYCOIN",
        25.,
        25.,
        2.,
        &[],
        SwapFlow::TakerSpendsMakerPayment,
        alice_privkey,
        bob_privkey,
        watcher_privkey,
        None,
    );
}

#[test]
fn test_watcher_validate_taker_fee_utxo() {
    let timeout = wait_until_sec(120); // timeout if test takes more than 120 seconds to run
    let lock_duration = get_payment_locktime();
    let (_ctx, taker_coin, _) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000u64.into());
    let (_ctx, maker_coin, _) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000u64.into());
    let taker_pubkey = taker_coin.my_public_key().unwrap();

    let taker_amount = MmNumber::from((10, 1));
    let dex_fee = DexFee::new_from_taker_coin(&taker_coin, maker_coin.ticker(), &taker_amount);

    let taker_fee = block_on(taker_coin.send_taker_fee(dex_fee, Uuid::new_v4().as_bytes(), lock_duration)).unwrap();

    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: taker_fee.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };

    block_on_f01(taker_coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let validate_taker_fee_res = block_on_f01(taker_coin.watcher_validate_taker_fee(WatcherValidateTakerFeeInput {
        taker_fee_hash: taker_fee.tx_hash_as_bytes().into_vec(),
        sender_pubkey: taker_pubkey.to_vec(),
        min_block_number: 0,
        lock_duration,
    }));
    assert!(validate_taker_fee_res.is_ok());

    let error = block_on_f01(taker_coin.watcher_validate_taker_fee(WatcherValidateTakerFeeInput {
        taker_fee_hash: taker_fee.tx_hash_as_bytes().into_vec(),
        sender_pubkey: maker_coin.my_public_key().unwrap().to_vec(),
        min_block_number: 0,
        lock_duration,
    }))
    .unwrap_err()
    .into_inner();

    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains(INVALID_SENDER_ERR_LOG))
        },
        _ => panic!("Expected `WrongPaymentTx` invalid public key, found {:?}", error),
    }

    let error = block_on_f01(taker_coin.watcher_validate_taker_fee(WatcherValidateTakerFeeInput {
        taker_fee_hash: taker_fee.tx_hash_as_bytes().into_vec(),
        sender_pubkey: taker_pubkey.to_vec(),
        min_block_number: u64::MAX,
        lock_duration,
    }))
    .unwrap_err()
    .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains(EARLY_CONFIRMATION_ERR_LOG))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` confirmed before min_block, found {:?}",
            error
        ),
    }

    let error = block_on_f01(taker_coin.watcher_validate_taker_fee(WatcherValidateTakerFeeInput {
        taker_fee_hash: taker_fee.tx_hash_as_bytes().into_vec(),
        sender_pubkey: taker_pubkey.to_vec(),
        min_block_number: 0,
        lock_duration: 0,
    }))
    .unwrap_err()
    .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains(OLD_TRANSACTION_ERR_LOG))
        },
        _ => panic!("Expected `WrongPaymentTx` transaction too old, found {:?}", error),
    }

    let mock_pubkey = taker_pubkey.to_vec();
    <UtxoStandardCoin as SwapOps>::dex_pubkey
        .mock_safe(move |_| MockResult::Return(Box::leak(Box::new(mock_pubkey.clone()))));

    let error = block_on_f01(taker_coin.watcher_validate_taker_fee(WatcherValidateTakerFeeInput {
        taker_fee_hash: taker_fee.tx_hash_as_bytes().into_vec(),
        sender_pubkey: taker_pubkey.to_vec(),
        min_block_number: 0,
        lock_duration,
    }))
    .unwrap_err()
    .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains(INVALID_RECEIVER_ERR_LOG))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` tx output script_pubkey doesn't match expected, found {:?}",
            error
        ),
    }
}

#[test]
fn test_watcher_validate_taker_payment_utxo() {
    let timeout = wait_until_sec(120); // timeout if test takes more than 120 seconds to run
    let time_lock_duration = get_payment_locktime();
    let wait_for_confirmation_until = wait_until_sec(time_lock_duration);
    let time_lock = wait_for_confirmation_until;

    let (_ctx, taker_coin, _) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000u64.into());
    let taker_pubkey = taker_coin.my_public_key().unwrap();

    let (_ctx, maker_coin, _) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000u64.into());
    let maker_pubkey = maker_coin.my_public_key().unwrap();

    let secret_hash = dhash160(&generate_secret().unwrap());

    let taker_payment = block_on(taker_coin.send_taker_payment(SendPaymentArgs {
        time_lock_duration,
        time_lock,
        other_pubkey: &maker_pubkey,
        secret_hash: secret_hash.as_slice(),
        amount: BigDecimal::from(10),
        swap_contract_address: &None,
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until,
    }))
    .unwrap();

    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: taker_payment.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    block_on_f01(taker_coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let taker_payment_refund_preimage = block_on_f01(taker_coin.create_taker_payment_refund_preimage(
        &taker_payment.tx_hex(),
        time_lock,
        &maker_pubkey,
        secret_hash.as_slice(),
        &None,
        &[],
    ))
    .unwrap();
    let validate_taker_payment_res =
        block_on_f01(taker_coin.watcher_validate_taker_payment(WatcherValidatePaymentInput {
            payment_tx: taker_payment.tx_hex(),
            taker_payment_refund_preimage: taker_payment_refund_preimage.tx_hex(),
            time_lock,
            taker_pub: taker_pubkey.to_vec(),
            maker_pub: maker_pubkey.to_vec(),
            secret_hash: secret_hash.to_vec(),
            wait_until: timeout,
            confirmations: 1,
            maker_coin: MmCoinEnum::UtxoCoinVariant(maker_coin.clone()),
        }));
    assert!(validate_taker_payment_res.is_ok());

    let error = block_on_f01(taker_coin.watcher_validate_taker_payment(WatcherValidatePaymentInput {
        payment_tx: taker_payment.tx_hex(),
        taker_payment_refund_preimage: taker_payment_refund_preimage.tx_hex(),
        time_lock,
        taker_pub: maker_pubkey.to_vec(),
        maker_pub: maker_pubkey.to_vec(),
        secret_hash: secret_hash.to_vec(),
        wait_until: timeout,
        confirmations: 1,
        maker_coin: MmCoinEnum::UtxoCoinVariant(maker_coin.clone()),
    }))
    .unwrap_err()
    .into_inner();

    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains(INVALID_SENDER_ERR_LOG))
        },
        _ => panic!("Expected `WrongPaymentTx` {INVALID_SENDER_ERR_LOG}, found {:?}", error),
    }

    // Used to get wrong swap id
    let wrong_secret_hash = dhash160(&generate_secret().unwrap());
    let error = block_on_f01(taker_coin.watcher_validate_taker_payment(WatcherValidatePaymentInput {
        payment_tx: taker_payment.tx_hex(),
        taker_payment_refund_preimage: taker_payment_refund_preimage.tx_hex(),
        time_lock,
        taker_pub: taker_pubkey.to_vec(),
        maker_pub: maker_pubkey.to_vec(),
        secret_hash: wrong_secret_hash.to_vec(),
        wait_until: timeout,
        confirmations: 1,
        maker_coin: MmCoinEnum::UtxoCoinVariant(maker_coin.clone()),
    }))
    .unwrap_err()
    .into_inner();

    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains(INVALID_SCRIPT_ERR_LOG))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            INVALID_SCRIPT_ERR_LOG, error
        ),
    }

    let taker_payment_wrong_secret = block_on(taker_coin.send_taker_payment(SendPaymentArgs {
        time_lock_duration,
        time_lock,
        other_pubkey: &maker_pubkey,
        secret_hash: wrong_secret_hash.as_slice(),
        amount: BigDecimal::from(10),
        swap_contract_address: &taker_coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until,
    }))
    .unwrap();

    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: taker_payment_wrong_secret.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    block_on_f01(taker_coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let error = block_on_f01(taker_coin.watcher_validate_taker_payment(WatcherValidatePaymentInput {
        payment_tx: taker_payment.tx_hex(),
        taker_payment_refund_preimage: taker_payment_refund_preimage.tx_hex(),
        time_lock: 500,
        taker_pub: taker_pubkey.to_vec(),
        maker_pub: maker_pubkey.to_vec(),
        secret_hash: wrong_secret_hash.to_vec(),
        wait_until: timeout,
        confirmations: 1,
        maker_coin: MmCoinEnum::UtxoCoinVariant(maker_coin.clone()),
    }))
    .unwrap_err()
    .into_inner();

    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains(INVALID_SCRIPT_ERR_LOG))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            INVALID_SCRIPT_ERR_LOG, error
        ),
    }

    let wrong_taker_payment_refund_preimage = block_on_f01(taker_coin.create_taker_payment_refund_preimage(
        &taker_payment.tx_hex(),
        time_lock,
        &maker_pubkey,
        wrong_secret_hash.as_slice(),
        &None,
        &[],
    ))
    .unwrap();

    let error = block_on_f01(taker_coin.watcher_validate_taker_payment(WatcherValidatePaymentInput {
        payment_tx: taker_payment.tx_hex(),
        taker_payment_refund_preimage: wrong_taker_payment_refund_preimage.tx_hex(),
        time_lock,
        taker_pub: taker_pubkey.to_vec(),
        maker_pub: maker_pubkey.to_vec(),
        secret_hash: secret_hash.to_vec(),
        wait_until: timeout,
        confirmations: 1,
        maker_coin: MmCoinEnum::UtxoCoinVariant(maker_coin),
    }))
    .unwrap_err()
    .into_inner();

    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains(INVALID_REFUND_TX_ERR_LOG))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            INVALID_REFUND_TX_ERR_LOG, error
        ),
    }
}

#[test]
fn test_taker_validates_taker_payment_refund_utxo() {
    let timeout = wait_until_sec(120); // timeout if test takes more than 120 seconds to run
    let time_lock_duration = get_payment_locktime();
    let wait_for_confirmation_until = wait_until_sec(time_lock_duration);
    let time_lock = now_sec() - 10;

    let (_ctx, taker_coin, _) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000u64.into());
    let (_ctx, maker_coin, _) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000u64.into());
    let maker_pubkey = maker_coin.my_public_key().unwrap();

    let secret_hash = dhash160(&generate_secret().unwrap());

    let taker_payment = block_on(taker_coin.send_taker_payment(SendPaymentArgs {
        time_lock_duration,
        time_lock,
        other_pubkey: &maker_pubkey,
        secret_hash: secret_hash.as_slice(),
        amount: BigDecimal::from(10),
        swap_contract_address: &None,
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until,
    }))
    .unwrap();

    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: taker_payment.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    block_on_f01(taker_coin.wait_for_confirmations(confirm_payment_input)).unwrap();

    let taker_payment_refund_preimage = block_on_f01(taker_coin.create_taker_payment_refund_preimage(
        &taker_payment.tx_hex(),
        time_lock,
        &maker_pubkey,
        secret_hash.as_slice(),
        &None,
        &[],
    ))
    .unwrap();

    let taker_payment_refund = block_on_f01(taker_coin.send_taker_payment_refund_preimage(RefundPaymentArgs {
        payment_tx: &taker_payment_refund_preimage.tx_hex(),
        other_pubkey: &maker_pubkey,
        tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
            maker_secret_hash: secret_hash.as_slice(),
        },
        time_lock,
        swap_contract_address: &None,
        swap_unique_data: &[],
        watcher_reward: false,
    }))
    .unwrap();

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: taker_payment_refund.tx_hex(),
        maker_pub: maker_pubkey.to_vec(),
        swap_contract_address: None,
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: BigDecimal::from(10),
        watcher_reward: None,
        spend_type: WatcherSpendType::TakerPaymentRefund,
    };

    let validate_watcher_refund = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input));
    assert!(validate_watcher_refund.is_ok());
}

#[test]
fn test_taker_validates_maker_payment_spend_utxo() {
    let timeout = wait_until_sec(120); // timeout if test takes more than 120 seconds to run
    let time_lock_duration = get_payment_locktime();
    let wait_for_confirmation_until = wait_until_sec(time_lock_duration);
    let time_lock = wait_for_confirmation_until;

    let (_ctx, taker_coin, _) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000u64.into());
    let (_ctx, maker_coin, _) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000u64.into());
    let taker_pubkey = taker_coin.my_public_key().unwrap();
    let maker_pubkey = maker_coin.my_public_key().unwrap();

    let secret = generate_secret().unwrap();
    let secret_hash = dhash160(&secret);

    let maker_payment = block_on(maker_coin.send_maker_payment(SendPaymentArgs {
        time_lock_duration,
        time_lock,
        other_pubkey: &taker_pubkey,
        secret_hash: secret_hash.as_slice(),
        amount: BigDecimal::from(10),
        swap_contract_address: &None,
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: None,
        wait_for_confirmation_until,
    }))
    .unwrap();

    block_on_f01(maker_coin.wait_for_confirmations(ConfirmPaymentInput {
        payment_tx: maker_payment.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    }))
    .unwrap();

    let maker_payment_spend_preimage = block_on_f01(taker_coin.create_maker_payment_spend_preimage(
        &maker_payment.tx_hex(),
        time_lock,
        &maker_pubkey,
        secret_hash.as_slice(),
        &[],
    ))
    .unwrap();

    let maker_payment_spend = block_on_f01(taker_coin.send_maker_payment_spend_preimage(
        SendMakerPaymentSpendPreimageInput {
            preimage: &maker_payment_spend_preimage.tx_hex(),
            secret_hash: secret_hash.as_slice(),
            secret: secret.as_slice(),
            taker_pub: &taker_pubkey,
            watcher_reward: false,
        },
    ))
    .unwrap();

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: maker_payment_spend.tx_hex(),
        maker_pub: maker_pubkey.to_vec(),
        swap_contract_address: None,
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: BigDecimal::from(10),
        watcher_reward: None,
        spend_type: WatcherSpendType::TakerPaymentRefund,
    };

    let validate_watcher_spend = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input));
    assert!(validate_watcher_spend.is_ok());
}

#[test]
fn test_send_taker_payment_refund_preimage_utxo() {
    let timeout = wait_until_sec(120); // timeout if test takes more than 120 seconds to run
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

    let refund_tx = block_on_f01(coin.create_taker_payment_refund_preimage(
        &tx.tx_hex(),
        time_lock,
        &my_public_key,
        &[0; 20],
        &None,
        &[],
    ))
    .unwrap();

    let refund_tx = block_on_f01(coin.send_taker_payment_refund_preimage(RefundPaymentArgs {
        payment_tx: &refund_tx.tx_hex(),
        swap_contract_address: &None,
        tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
            maker_secret_hash: &[0; 20],
        },
        other_pubkey: &my_public_key,
        time_lock,
        swap_unique_data: &[],
        watcher_reward: false,
    }))
    .unwrap();

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
