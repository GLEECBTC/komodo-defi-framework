//! ETH/ERC20 Watcher Tests
//!
//! These tests are disabled by default because ETH watchers are unstable
//! and not completed yet. Enable with feature `docker-tests-watchers-eth`.

use super::*;

#[test]
fn test_watcher_spends_maker_payment_utxo_eth() {
    let alice_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let bob_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let watcher_coin = eth_coin_with_random_privkey(watchers_swap_contract());

    let balances = start_swaps_and_get_balances(
        "ETH",
        "MYCOIN",
        0.01,
        0.01,
        1.,
        &[("USE_WATCHER_REWARD", "")],
        SwapFlow::WatcherSpendsMakerPayment,
        &alice_coin.display_priv_key().unwrap()[2..],
        &bob_coin.display_priv_key().unwrap()[2..],
        &watcher_coin.display_priv_key().unwrap()[2..],
        None,
    );

    let mycoin_volume = BigDecimal::from_str("1").unwrap();

    assert_eq!(
        balances.alice_bcoin_balance_after.round(0),
        balances.alice_bcoin_balance_before + mycoin_volume
    );
    assert!(balances.bob_acoin_balance_after > balances.bob_acoin_balance_before);
    assert!(balances.alice_acoin_balance_after > balances.alice_acoin_balance_middle);
}

#[test]
fn test_watcher_spends_maker_payment_eth_utxo() {
    let alice_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let bob_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let watcher_coin = eth_coin_with_random_privkey(watchers_swap_contract());

    let balances = start_swaps_and_get_balances(
        "MYCOIN",
        "ETH",
        100.,
        100.,
        0.01,
        &[("TEST_COIN_PRICE", "0.01"), ("USE_WATCHER_REWARD", "")],
        SwapFlow::WatcherSpendsMakerPayment,
        &alice_coin.display_priv_key().unwrap()[2..],
        &bob_coin.display_priv_key().unwrap()[2..],
        &watcher_coin.display_priv_key().unwrap()[2..],
        None,
    );

    let eth_volume = BigDecimal::from_str("0.01").unwrap();
    let mycoin_volume = BigDecimal::from_str("1").unwrap();
    let min_tx_amount = BigDecimal::from_str("0.00001").unwrap();

    let coin = TestCoin::new("MYCOIN");
    TestCoin::min_tx_amount.mock_safe(move |_| MockResult::Return(min_tx_amount.clone()));
    let dex_fee: BigDecimal = DexFee::new_from_taker_coin(&coin, "ETH", &MmNumber::from(mycoin_volume.clone()))
        .fee_amount() // returns Standard fee (default for TestCoin)
        .into();
    let alice_mycoin_reward_sent = balances.alice_acoin_balance_before
        - balances.alice_acoin_balance_after.clone()
        - mycoin_volume.clone()
        - dex_fee.with_scale(8);

    assert_eq!(
        balances.alice_bcoin_balance_after,
        balances.alice_bcoin_balance_middle + eth_volume
    );
    assert_eq!(
        balances.bob_acoin_balance_after.round(2),
        balances.bob_acoin_balance_before + mycoin_volume + alice_mycoin_reward_sent.round(2)
    );
    assert!(balances.watcher_bcoin_balance_after > balances.watcher_bcoin_balance_before);
}

#[test]
fn test_watcher_spends_maker_payment_eth_erc20() {
    let alice_coin = erc20_coin_with_random_privkey(watchers_swap_contract());
    let bob_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let watcher_coin = eth_coin_with_random_privkey(watchers_swap_contract());

    let balances = start_swaps_and_get_balances(
        "ERC20DEV",
        "ETH",
        100.,
        100.,
        0.01,
        &[("TEST_COIN_PRICE", "0.01"), ("USE_WATCHER_REWARD", "")],
        SwapFlow::WatcherSpendsMakerPayment,
        &alice_coin.display_priv_key().unwrap()[2..],
        &bob_coin.display_priv_key().unwrap()[2..],
        &watcher_coin.display_priv_key().unwrap()[2..],
        None,
    );

    let eth_volume = BigDecimal::from_str("0.01").unwrap();
    let jst_volume = BigDecimal::from_str("1").unwrap();

    assert_eq!(
        balances.alice_bcoin_balance_after,
        balances.alice_bcoin_balance_middle + eth_volume
    );
    assert_eq!(
        balances.bob_acoin_balance_after,
        balances.bob_acoin_balance_before + jst_volume
    );
    assert!(balances.watcher_bcoin_balance_after > balances.watcher_bcoin_balance_before);
}

#[test]
fn test_watcher_spends_maker_payment_erc20_eth() {
    let alice_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let bob_coin = erc20_coin_with_random_privkey(watchers_swap_contract());
    let watcher_coin = eth_coin_with_random_privkey(watchers_swap_contract());

    let balances = start_swaps_and_get_balances(
        "ETH",
        "ERC20DEV",
        0.01,
        0.01,
        1.,
        &[("USE_WATCHER_REWARD", "")],
        SwapFlow::WatcherSpendsMakerPayment,
        &alice_coin.display_priv_key().unwrap()[2..],
        &bob_coin.display_priv_key().unwrap()[2..],
        &watcher_coin.display_priv_key().unwrap()[2..],
        None,
    );

    let jst_volume = BigDecimal::from_str("1").unwrap();

    assert_eq!(
        balances.alice_bcoin_balance_after,
        balances.alice_bcoin_balance_before + jst_volume
    );
    assert!(balances.bob_acoin_balance_after > balances.bob_acoin_balance_before);
    // TODO watcher likely pays the fee that is higher than received reward
    // assert!(balances.watcher_acoin_balance_after > balances.watcher_acoin_balance_before);
}

#[test]
fn test_watcher_spends_maker_payment_utxo_erc20() {
    let alice_coin = erc20_coin_with_random_privkey(watchers_swap_contract());
    let bob_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let watcher_coin = eth_coin_with_random_privkey(watchers_swap_contract());

    let balances = start_swaps_and_get_balances(
        "ERC20DEV",
        "MYCOIN",
        1.,
        1.,
        1.,
        &[("TEST_COIN_PRICE", "0.01"), ("USE_WATCHER_REWARD", "")],
        SwapFlow::WatcherSpendsMakerPayment,
        &alice_coin.display_priv_key().unwrap()[2..],
        &bob_coin.display_priv_key().unwrap()[2..],
        &watcher_coin.display_priv_key().unwrap()[2..],
        None,
    );

    let mycoin_volume = BigDecimal::from_str("1").unwrap();
    let jst_volume = BigDecimal::from_str("1").unwrap();

    assert_eq!(
        balances.alice_bcoin_balance_after.round(0),
        balances.alice_bcoin_balance_before + mycoin_volume
    );
    assert_eq!(
        balances.bob_acoin_balance_after,
        balances.bob_acoin_balance_before + jst_volume
    );
    assert!(balances.alice_eth_balance_after > balances.alice_eth_balance_middle);
}

#[test]
fn test_watcher_spends_maker_payment_erc20_utxo() {
    let alice_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let bob_coin = erc20_coin_with_random_privkey(watchers_swap_contract());
    let watcher_coin = eth_coin_with_random_privkey(watchers_swap_contract());

    let balances = start_swaps_and_get_balances(
        "MYCOIN",
        "ERC20DEV",
        1.,
        1.,
        1.,
        &[("TEST_COIN_PRICE", "0.01"), ("USE_WATCHER_REWARD", "")],
        SwapFlow::WatcherSpendsMakerPayment,
        &alice_coin.display_priv_key().unwrap()[2..],
        &bob_coin.display_priv_key().unwrap()[2..],
        &watcher_coin.display_priv_key().unwrap()[2..],
        None,
    );

    let mycoin_volume = BigDecimal::from_str("1").unwrap();
    let jst_volume = BigDecimal::from_str("1").unwrap();

    let min_tx_amount = BigDecimal::from_str("0.00001").unwrap();
    let coin = TestCoin::new("MYCOIN");
    TestCoin::min_tx_amount.mock_safe(move |_| MockResult::Return(min_tx_amount.clone()));
    let dex_fee: BigDecimal = DexFee::new_from_taker_coin(&coin, "ERC20DEV", &MmNumber::from(mycoin_volume.clone()))
        .fee_amount() // returns Standard fee (default for TestCoin)
        .into();
    let alice_mycoin_reward_sent = balances.alice_acoin_balance_before
        - balances.alice_acoin_balance_after.clone()
        - mycoin_volume.clone()
        - dex_fee.with_scale(8);

    let bob_jst_reward_sent =
        balances.bob_bcoin_balance_before - jst_volume.clone() - balances.bob_bcoin_balance_after.clone();

    assert_eq!(
        balances.alice_bcoin_balance_after,
        balances.alice_bcoin_balance_before + jst_volume
    );
    assert_eq!(
        balances.bob_acoin_balance_after.round(2),
        balances.bob_acoin_balance_before + mycoin_volume + alice_mycoin_reward_sent.round(2)
    );
    assert_eq!(
        balances.watcher_bcoin_balance_after,
        balances.watcher_bcoin_balance_before + bob_jst_reward_sent
    );
}

#[test]
fn test_watcher_refunds_taker_payment_eth() {
    let alice_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let bob_coin = erc20_coin_with_random_privkey(watchers_swap_contract());
    let watcher_coin = eth_coin_with_random_privkey(watchers_swap_contract());

    let balances = start_swaps_and_get_balances(
        "ETH",
        "ERC20DEV",
        0.01,
        0.01,
        1.,
        &[("USE_WATCHER_REWARD", "")],
        SwapFlow::WatcherRefundsTakerPayment,
        &alice_coin.display_priv_key().unwrap()[2..],
        &bob_coin.display_priv_key().unwrap()[2..],
        &watcher_coin.display_priv_key().unwrap()[2..],
        Some(60),
    );

    assert_eq!(balances.alice_bcoin_balance_after, balances.alice_bcoin_balance_before);
    assert!(balances.watcher_acoin_balance_after > balances.watcher_acoin_balance_before);
}

#[test]
fn test_watcher_refunds_taker_payment_erc20() {
    let alice_coin = erc20_coin_with_random_privkey(watchers_swap_contract());
    let bob_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let watcher_coin = eth_coin_with_random_privkey(watchers_swap_contract());

    let balances = start_swaps_and_get_balances(
        "ERC20DEV",
        "ETH",
        100.,
        100.,
        0.01,
        &[("TEST_COIN_PRICE", "0.01"), ("USE_WATCHER_REWARD", "")],
        SwapFlow::WatcherRefundsTakerPayment,
        &alice_coin.display_priv_key().unwrap()[2..],
        &bob_coin.display_priv_key().unwrap()[2..],
        &watcher_coin.display_priv_key().unwrap()[2..],
        Some(60),
    );
    let erc20_volume = BigDecimal::from_str("1").unwrap();

    assert_eq!(
        balances.alice_acoin_balance_after,
        balances.alice_acoin_balance_middle + erc20_volume
    );

    log!("watcher_bcoin_balance_before {}", balances.watcher_bcoin_balance_before);
    log!("watcher_bcoin_balance_after {}", balances.watcher_bcoin_balance_after);

    assert!(balances.watcher_bcoin_balance_after > balances.watcher_bcoin_balance_before);
}

#[test]
fn test_watcher_waits_for_taker_eth() {
    let alice_coin = erc20_coin_with_random_privkey(watchers_swap_contract());
    let bob_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let watcher_coin = eth_coin_with_random_privkey(watchers_swap_contract());

    start_swaps_and_get_balances(
        "ERC20DEV",
        "ETH",
        100.,
        100.,
        0.01,
        &[("TEST_COIN_PRICE", "0.01"), ("USE_WATCHER_REWARD", "")],
        SwapFlow::TakerSpendsMakerPayment,
        &alice_coin.display_priv_key().unwrap()[2..],
        &bob_coin.display_priv_key().unwrap()[2..],
        &watcher_coin.display_priv_key().unwrap()[2..],
        None,
    );
}

#[test]
#[ignore]
fn test_two_watchers_spend_maker_payment_eth_erc20() {
    let coins = json!([eth_dev_conf(), eth_jst_testnet_conf()]);

    let alice_passphrase =
        String::from("spice describe gravity federal blast come thank unfair canal monkey style afraid");
    let alice_conf = Mm2TestConf::seednode(&alice_passphrase, &coins);
    let mut mm_alice = MarketMakerIt::start(alice_conf.conf.clone(), alice_conf.rpc_password.clone(), None).unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    let bob_passphrase = String::from("also shoot benefit prefer juice shell elder veteran woman mimic image kidney");
    let bob_conf = Mm2TestConf::light_node(&bob_passphrase, &coins, &[&mm_alice.ip.to_string()]);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    let watcher1_passphrase =
        String::from("also shoot benefit prefer juice shell thank unfair canal monkey style afraid");
    let watcher1_conf = Mm2TestConf::watcher_light_node(
        &watcher1_passphrase,
        &coins,
        &[&mm_alice.ip.to_string()],
        WatcherConf {
            wait_taker_payment: 0.,
            wait_maker_payment_spend_factor: 0.,
            refund_start_factor: 1.5,
            search_interval: 1.0,
        },
    )
    .conf;
    let mut mm_watcher1 = MarketMakerIt::start(watcher1_conf, DEFAULT_RPC_PASSWORD.to_string(), None).unwrap();
    let (_watcher_dump_log, _watcher_dump_dashboard) = mm_dump(&mm_watcher1.log_path);

    let watcher2_passphrase =
        String::from("also shoot benefit shell thank prefer juice unfair canal monkey style afraid");
    let watcher2_conf = Mm2TestConf::watcher_light_node(
        &watcher2_passphrase,
        &coins,
        &[&mm_alice.ip.to_string()],
        WatcherConf {
            wait_taker_payment: 0.,
            wait_maker_payment_spend_factor: 0.,
            refund_start_factor: 1.5,
            search_interval: 1.0,
        },
    )
    .conf;
    let mut mm_watcher2 = MarketMakerIt::start(watcher2_conf, DEFAULT_RPC_PASSWORD.to_string(), None).unwrap();
    let (_watcher_dump_log, _watcher_dump_dashboard) = mm_dump(&mm_watcher1.log_path);

    enable_eth(&mm_alice, "ETH");
    enable_eth(&mm_alice, "JST");
    enable_eth(&mm_bob, "ETH");
    enable_eth(&mm_bob, "JST");
    enable_eth(&mm_watcher1, "ETH");
    enable_eth(&mm_watcher1, "JST");
    enable_eth(&mm_watcher2, "ETH");
    enable_eth(&mm_watcher2, "JST");

    let alice_eth_balance_before = block_on(my_balance(&mm_alice, "ETH")).balance.with_scale(2);
    let alice_jst_balance_before = block_on(my_balance(&mm_alice, "JST")).balance.with_scale(2);
    let bob_eth_balance_before = block_on(my_balance(&mm_bob, "ETH")).balance.with_scale(2);
    let bob_jst_balance_before = block_on(my_balance(&mm_bob, "JST")).balance.with_scale(2);
    let watcher1_eth_balance_before = block_on(my_balance(&mm_watcher1, "ETH")).balance;
    let watcher2_eth_balance_before = block_on(my_balance(&mm_watcher2, "ETH")).balance;

    block_on(start_swaps(&mut mm_bob, &mut mm_alice, &[("ETH", "JST")], 1., 1., 0.01));

    block_on(mm_alice.wait_for_log(180., |log| log.contains(WATCHER_MESSAGE_SENT_LOG))).unwrap();
    block_on(mm_alice.stop()).unwrap();
    block_on(mm_watcher1.wait_for_log(180., |log| log.contains(MAKER_PAYMENT_SPEND_SENT_LOG))).unwrap();
    block_on(mm_watcher2.wait_for_log(180., |log| log.contains(MAKER_PAYMENT_SPEND_SENT_LOG))).unwrap();
    thread::sleep(Duration::from_secs(25));

    let mm_alice = MarketMakerIt::start(alice_conf.conf.clone(), alice_conf.rpc_password, None).unwrap();
    enable_eth(&mm_alice, "ETH");
    enable_eth(&mm_alice, "JST");

    let alice_eth_balance_after = block_on(my_balance(&mm_alice, "ETH")).balance.with_scale(2);
    let alice_jst_balance_after = block_on(my_balance(&mm_alice, "JST")).balance.with_scale(2);
    let bob_eth_balance_after = block_on(my_balance(&mm_bob, "ETH")).balance.with_scale(2);
    let bob_jst_balance_after = block_on(my_balance(&mm_bob, "JST")).balance.with_scale(2);
    let watcher1_eth_balance_after = block_on(my_balance(&mm_watcher1, "ETH")).balance;
    let watcher2_eth_balance_after = block_on(my_balance(&mm_watcher2, "ETH")).balance;

    let volume = BigDecimal::from_str("0.01").unwrap();
    assert_eq!(alice_jst_balance_before - volume.clone(), alice_jst_balance_after);
    assert_eq!(bob_jst_balance_before + volume.clone(), bob_jst_balance_after);
    assert_eq!(alice_eth_balance_before + volume.clone(), alice_eth_balance_after);
    assert_eq!(bob_eth_balance_before - volume, bob_eth_balance_after);
    let w1_gain = watcher1_eth_balance_after > watcher1_eth_balance_before;
    let w2_gain = watcher2_eth_balance_after > watcher2_eth_balance_before;
    assert_ne!(w1_gain, w2_gain, "exactly one watcher must receive the reward");
}

#[test]
fn test_watcher_validate_taker_fee_eth() {
    let timeout = wait_until_sec(120); // timeout if test takes more than 120 seconds to run
    let lock_duration = get_payment_locktime();

    let taker_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let taker_keypair = taker_coin.derive_htlc_key_pair(&[]);
    let taker_pubkey = taker_keypair.public();

    let taker_amount = MmNumber::from((1, 1));
    let dex_fee = DexFee::new_from_taker_coin(&taker_coin, "ETH", &taker_amount);
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

    let wrong_keypair = key_pair_from_secret(&random_secp256k1_secret().take()).unwrap();
    let error = block_on_f01(taker_coin.watcher_validate_taker_fee(WatcherValidateTakerFeeInput {
        taker_fee_hash: taker_fee.tx_hash_as_bytes().into_vec(),
        sender_pubkey: wrong_keypair.public().to_vec(),
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

    let mock_pubkey = taker_pubkey.to_vec();
    <EthCoin as SwapOps>::dex_pubkey.mock_safe(move |_| MockResult::Return(Box::leak(Box::new(mock_pubkey.clone()))));

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
    <EthCoin as SwapOps>::dex_pubkey.clear_mock();
}

#[test]
fn test_watcher_validate_taker_fee_erc20() {
    let timeout = wait_until_sec(120); // timeout if test takes more than 120 seconds to run
    let lock_duration = get_payment_locktime();

    let taker_coin = erc20_coin_with_random_privkey(watchers_swap_contract());
    let taker_keypair = taker_coin.derive_htlc_key_pair(&[]);
    let taker_pubkey = taker_keypair.public();

    let taker_amount = MmNumber::from((1, 1));
    let dex_fee = DexFee::new_from_taker_coin(&taker_coin, "ETH", &taker_amount);
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

    let wrong_keypair = key_pair_from_secret(&random_secp256k1_secret().take()).unwrap();
    let error = block_on_f01(taker_coin.watcher_validate_taker_fee(WatcherValidateTakerFeeInput {
        taker_fee_hash: taker_fee.tx_hash_as_bytes().into_vec(),
        sender_pubkey: wrong_keypair.public().to_vec(),
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

    let mock_pubkey = taker_pubkey.to_vec();
    <EthCoin as SwapOps>::dex_pubkey.mock_safe(move |_| MockResult::Return(Box::leak(Box::new(mock_pubkey.clone()))));

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
    <EthCoin as SwapOps>::dex_pubkey.clear_mock();
}

#[test]
fn test_watcher_validate_taker_payment_eth() {
    let timeout = wait_until_sec(120); // timeout if test takes more than 120 seconds to run

    let taker_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let taker_keypair = taker_coin.derive_htlc_key_pair(&[]);
    let taker_pub = taker_keypair.public();

    let maker_seed = get_passphrase!(".env.seed", "BOB_PASSPHRASE").unwrap();
    let maker_keypair = key_pair_from_seed(&maker_seed).unwrap();
    let maker_pub = maker_keypair.public();

    let time_lock_duration = get_payment_locktime();
    let wait_for_confirmation_until = wait_until_sec(time_lock_duration);
    let time_lock = wait_for_confirmation_until;
    let taker_amount = BigDecimal::from_str("0.01").unwrap();
    let maker_amount = BigDecimal::from_str("0.01").unwrap();
    let secret_hash = dhash160(&generate_secret().unwrap());
    let watcher_reward = Some(
        block_on(taker_coin.get_taker_watcher_reward(
            &MmCoinEnum::from(taker_coin.clone()),
            Some(taker_amount.clone()),
            Some(maker_amount),
            None,
            wait_for_confirmation_until,
        ))
        .unwrap(),
    );

    let taker_payment = block_on(taker_coin.send_taker_payment(SendPaymentArgs {
        time_lock_duration,
        time_lock,
        other_pubkey: maker_pub,
        secret_hash: secret_hash.as_slice(),
        amount: taker_amount.clone(),
        swap_contract_address: &taker_coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: watcher_reward.clone(),
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

    let validate_taker_payment_res = block_on_f01(taker_coin.watcher_validate_taker_payment(
        coins::WatcherValidatePaymentInput {
            payment_tx: taker_payment.tx_hex(),
            taker_payment_refund_preimage: Vec::new(),
            time_lock,
            taker_pub: taker_pub.to_vec(),
            maker_pub: maker_pub.to_vec(),
            secret_hash: secret_hash.to_vec(),
            wait_until: timeout,
            confirmations: 1,
            maker_coin: MmCoinEnum::EthCoinVariant(taker_coin.clone()),
        },
    ));
    assert!(validate_taker_payment_res.is_ok());

    let error = block_on_f01(
        taker_coin.watcher_validate_taker_payment(coins::WatcherValidatePaymentInput {
            payment_tx: taker_payment.tx_hex(),
            taker_payment_refund_preimage: Vec::new(),
            time_lock,
            taker_pub: maker_pub.to_vec(),
            maker_pub: maker_pub.to_vec(),
            secret_hash: secret_hash.to_vec(),
            wait_until: timeout,
            confirmations: 1,
            maker_coin: MmCoinEnum::EthCoinVariant(taker_coin.clone()),
        }),
    )
    .unwrap_err()
    .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains(INVALID_SENDER_ERR_LOG))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            INVALID_SENDER_ERR_LOG, error
        ),
    }

    let taker_payment_wrong_contract = block_on(taker_coin.send_taker_payment(SendPaymentArgs {
        time_lock_duration,
        time_lock,
        other_pubkey: maker_pub,
        secret_hash: secret_hash.as_slice(),
        amount: taker_amount.clone(),
        swap_contract_address: &Some("9130b257d37a52e52f21054c4da3450c72f595ce".into()),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: watcher_reward.clone(),
        wait_for_confirmation_until,
    }))
    .unwrap();

    let error = block_on_f01(
        taker_coin.watcher_validate_taker_payment(coins::WatcherValidatePaymentInput {
            payment_tx: taker_payment_wrong_contract.tx_hex(),
            taker_payment_refund_preimage: Vec::new(),
            time_lock,
            taker_pub: taker_pub.to_vec(),
            maker_pub: maker_pub.to_vec(),
            secret_hash: secret_hash.to_vec(),
            wait_until: timeout,
            confirmations: 1,
            maker_coin: MmCoinEnum::EthCoinVariant(taker_coin.clone()),
        }),
    )
    .unwrap_err()
    .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains(INVALID_CONTRACT_ADDRESS_ERR_LOG))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            INVALID_CONTRACT_ADDRESS_ERR_LOG, error
        ),
    }

    // Used to get wrong swap id
    let wrong_secret_hash = dhash160(&generate_secret().unwrap());
    let error = block_on_f01(
        taker_coin.watcher_validate_taker_payment(coins::WatcherValidatePaymentInput {
            payment_tx: taker_payment.tx_hex(),
            taker_payment_refund_preimage: Vec::new(),
            time_lock,
            taker_pub: taker_pub.to_vec(),
            maker_pub: maker_pub.to_vec(),
            secret_hash: wrong_secret_hash.to_vec(),
            wait_until: timeout,
            confirmations: 1,
            maker_coin: MmCoinEnum::EthCoinVariant(taker_coin.clone()),
        }),
    )
    .unwrap_err()
    .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::UnexpectedPaymentState(err) => {
            assert!(err.contains(INVALID_PAYMENT_STATE_ERR_LOG))
        },
        _ => panic!(
            "Expected `UnexpectedPaymentState` {}, found {:?}",
            INVALID_PAYMENT_STATE_ERR_LOG, error
        ),
    }

    let taker_payment_wrong_secret = block_on(taker_coin.send_taker_payment(SendPaymentArgs {
        time_lock_duration,
        time_lock,
        other_pubkey: maker_pub,
        secret_hash: wrong_secret_hash.as_slice(),
        amount: taker_amount,
        swap_contract_address: &taker_coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward,
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
        taker_payment_refund_preimage: Vec::new(),
        time_lock,
        taker_pub: taker_pub.to_vec(),
        maker_pub: maker_pub.to_vec(),
        secret_hash: wrong_secret_hash.to_vec(),
        wait_until: timeout,
        confirmations: 1,
        maker_coin: MmCoinEnum::EthCoinVariant(taker_coin.clone()),
    }))
    .unwrap_err()
    .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains(INVALID_SWAP_ID_ERR_LOG))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            INVALID_SWAP_ID_ERR_LOG, error
        ),
    }

    let error = block_on_f01(taker_coin.watcher_validate_taker_payment(WatcherValidatePaymentInput {
        payment_tx: taker_payment.tx_hex(),
        taker_payment_refund_preimage: Vec::new(),
        time_lock,
        taker_pub: taker_pub.to_vec(),
        maker_pub: taker_pub.to_vec(),
        secret_hash: secret_hash.to_vec(),
        wait_until: timeout,
        confirmations: 1,
        maker_coin: MmCoinEnum::EthCoinVariant(taker_coin.clone()),
    }))
    .unwrap_err()
    .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains(INVALID_RECEIVER_ERR_LOG))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            INVALID_RECEIVER_ERR_LOG, error
        ),
    }
}

#[test]
fn test_watcher_validate_taker_payment_erc20() {
    let timeout = wait_until_sec(120); // timeout if test takes more than 120 seconds to run

    let taker_coin = erc20_coin_with_random_privkey(watchers_swap_contract());
    let taker_keypair = taker_coin.derive_htlc_key_pair(&[]);
    let taker_pub = taker_keypair.public();

    let maker_seed = get_passphrase!(".env.seed", "BOB_PASSPHRASE").unwrap();
    let maker_keypair = key_pair_from_seed(&maker_seed).unwrap();
    let maker_pub = maker_keypair.public();

    let time_lock_duration = get_payment_locktime();
    let wait_for_confirmation_until = wait_until_sec(time_lock_duration);
    let time_lock = wait_for_confirmation_until;

    let secret_hash = dhash160(&generate_secret().unwrap());

    let taker_amount = BigDecimal::from_str("0.01").unwrap();
    let maker_amount = BigDecimal::from_str("0.01").unwrap();

    let watcher_reward = Some(
        block_on(taker_coin.get_taker_watcher_reward(
            &MmCoinEnum::from(taker_coin.clone()),
            Some(taker_amount.clone()),
            Some(maker_amount),
            None,
            wait_for_confirmation_until,
        ))
        .unwrap(),
    );

    let taker_payment = block_on(taker_coin.send_taker_payment(SendPaymentArgs {
        time_lock_duration,
        time_lock,
        other_pubkey: maker_pub,
        secret_hash: secret_hash.as_slice(),
        amount: taker_amount.clone(),
        swap_contract_address: &taker_coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: watcher_reward.clone(),
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

    let validate_taker_payment_res =
        block_on_f01(taker_coin.watcher_validate_taker_payment(WatcherValidatePaymentInput {
            payment_tx: taker_payment.tx_hex(),
            taker_payment_refund_preimage: Vec::new(),
            time_lock,
            taker_pub: taker_pub.to_vec(),
            maker_pub: maker_pub.to_vec(),
            secret_hash: secret_hash.to_vec(),
            wait_until: timeout,
            confirmations: 1,
            maker_coin: MmCoinEnum::EthCoinVariant(taker_coin.clone()),
        }));
    assert!(validate_taker_payment_res.is_ok());

    let error = block_on_f01(taker_coin.watcher_validate_taker_payment(WatcherValidatePaymentInput {
        payment_tx: taker_payment.tx_hex(),
        taker_payment_refund_preimage: Vec::new(),
        time_lock,
        taker_pub: maker_pub.to_vec(),
        maker_pub: maker_pub.to_vec(),
        secret_hash: secret_hash.to_vec(),
        wait_until: timeout,
        confirmations: 1,
        maker_coin: MmCoinEnum::EthCoinVariant(taker_coin.clone()),
    }))
    .unwrap_err()
    .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains(INVALID_SENDER_ERR_LOG))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            INVALID_SENDER_ERR_LOG, error
        ),
    }

    let taker_payment_wrong_contract = block_on(taker_coin.send_taker_payment(SendPaymentArgs {
        time_lock_duration,
        time_lock,
        other_pubkey: maker_pub,
        secret_hash: secret_hash.as_slice(),
        amount: taker_amount.clone(),
        swap_contract_address: &Some("9130b257d37a52e52f21054c4da3450c72f595ce".into()),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: watcher_reward.clone(),
        wait_for_confirmation_until,
    }))
    .unwrap();

    let error = block_on_f01(taker_coin.watcher_validate_taker_payment(WatcherValidatePaymentInput {
        payment_tx: taker_payment_wrong_contract.tx_hex(),
        taker_payment_refund_preimage: Vec::new(),
        time_lock,
        taker_pub: taker_pub.to_vec(),
        maker_pub: maker_pub.to_vec(),
        secret_hash: secret_hash.to_vec(),
        wait_until: timeout,
        confirmations: 1,
        maker_coin: MmCoinEnum::EthCoinVariant(taker_coin.clone()),
    }))
    .unwrap_err()
    .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains(INVALID_CONTRACT_ADDRESS_ERR_LOG))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            INVALID_CONTRACT_ADDRESS_ERR_LOG, error
        ),
    }

    // Used to get wrong swap id
    let wrong_secret_hash = dhash160(&generate_secret().unwrap());
    let error = block_on_f01(taker_coin.watcher_validate_taker_payment(WatcherValidatePaymentInput {
        payment_tx: taker_payment.tx_hex(),
        taker_payment_refund_preimage: Vec::new(),
        time_lock,
        taker_pub: taker_pub.to_vec(),
        maker_pub: maker_pub.to_vec(),
        secret_hash: wrong_secret_hash.to_vec(),
        wait_until: timeout,
        confirmations: 1,
        maker_coin: MmCoinEnum::EthCoinVariant(taker_coin.clone()),
    }))
    .unwrap_err()
    .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::UnexpectedPaymentState(err) => {
            assert!(err.contains(INVALID_PAYMENT_STATE_ERR_LOG))
        },
        _ => panic!(
            "Expected `UnexpectedPaymentState` {}, found {:?}",
            INVALID_PAYMENT_STATE_ERR_LOG, error
        ),
    }

    let taker_payment_wrong_secret = block_on(taker_coin.send_taker_payment(SendPaymentArgs {
        time_lock_duration,
        time_lock,
        other_pubkey: maker_pub,
        secret_hash: wrong_secret_hash.as_slice(),
        amount: taker_amount,
        swap_contract_address: &taker_coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward,
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
        taker_payment_refund_preimage: Vec::new(),
        time_lock,
        taker_pub: taker_pub.to_vec(),
        maker_pub: maker_pub.to_vec(),
        secret_hash: wrong_secret_hash.to_vec(),
        wait_until: timeout,
        confirmations: 1,
        maker_coin: MmCoinEnum::EthCoinVariant(taker_coin.clone()),
    }))
    .unwrap_err()
    .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains(INVALID_SWAP_ID_ERR_LOG))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            INVALID_SWAP_ID_ERR_LOG, error
        ),
    }

    let error = block_on_f01(taker_coin.watcher_validate_taker_payment(WatcherValidatePaymentInput {
        payment_tx: taker_payment.tx_hex(),
        taker_payment_refund_preimage: Vec::new(),
        time_lock,
        taker_pub: taker_pub.to_vec(),
        maker_pub: taker_pub.to_vec(),
        secret_hash: secret_hash.to_vec(),
        wait_until: timeout,
        confirmations: 1,
        maker_coin: MmCoinEnum::EthCoinVariant(taker_coin.clone()),
    }))
    .unwrap_err()
    .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains(INVALID_RECEIVER_ERR_LOG))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            INVALID_RECEIVER_ERR_LOG, error
        ),
    }
}

#[test]
fn test_taker_validates_taker_payment_refund_eth() {
    let timeout = wait_until_sec(120); // timeout if test takes more than 120 seconds to run

    let taker_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let taker_keypair = taker_coin.derive_htlc_key_pair(&[]);
    let taker_pub = taker_keypair.public();

    let maker_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let maker_keypair = maker_coin.derive_htlc_key_pair(&[]);
    let maker_pub = maker_keypair.public();

    let time_lock_duration = get_payment_locktime();
    let wait_for_confirmation_until = wait_until_sec(time_lock_duration);
    let time_lock = now_sec() - 10;
    let taker_amount = BigDecimal::from_str("0.001").unwrap();
    let maker_amount = BigDecimal::from_str("0.001").unwrap();
    let secret_hash = dhash160(&generate_secret().unwrap());

    let watcher_reward = block_on(taker_coin.get_taker_watcher_reward(
        &MmCoinEnum::from(taker_coin.clone()),
        Some(taker_amount.clone()),
        Some(maker_amount),
        None,
        wait_for_confirmation_until,
    ))
    .unwrap();

    let taker_payment = block_on(taker_coin.send_taker_payment(SendPaymentArgs {
        time_lock_duration,
        time_lock,
        other_pubkey: maker_pub,
        secret_hash: secret_hash.as_slice(),
        amount: taker_amount.clone(),
        swap_contract_address: &taker_coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: Some(watcher_reward.clone()),
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
        taker_pub,
        secret_hash.as_slice(),
        &taker_coin.swap_contract_address(),
        &[],
    ))
    .unwrap();

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: taker_payment_refund_preimage.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: taker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: taker_amount.clone(),
        watcher_reward: Some(watcher_reward.clone()),
        spend_type: WatcherSpendType::TakerPaymentRefund,
    };

    let error = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::UnexpectedPaymentState(err) => {
            assert!(err.contains("Payment state is not"))
        },
        _ => panic!(
            "Expected `UnexpectedPaymentState` {}, found {:?}",
            "Payment state is not 3", error
        ),
    }

    let taker_payment_refund = block_on_f01(taker_coin.send_taker_payment_refund_preimage(RefundPaymentArgs {
        payment_tx: &taker_payment_refund_preimage.tx_hex(),
        other_pubkey: taker_pub,
        tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
            maker_secret_hash: secret_hash.as_slice(),
        },
        time_lock,
        swap_contract_address: &taker_coin.swap_contract_address(),
        swap_unique_data: &[],
        watcher_reward: true,
    }))
    .unwrap();

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: taker_payment_refund.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: taker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: taker_amount.clone(),
        watcher_reward: Some(watcher_reward.clone()),
        spend_type: WatcherSpendType::TakerPaymentRefund,
    };

    let validate_watcher_refund = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input));
    assert!(validate_watcher_refund.is_ok());

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: taker_payment_refund.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: Some("9130b257d37a52e52f21054c4da3450c72f595ce".into()),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: taker_amount.clone(),
        watcher_reward: Some(watcher_reward.clone()),
        spend_type: WatcherSpendType::TakerPaymentRefund,
    };
    let error = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("was sent to wrong address"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            "invalid contract address", error
        ),
    }

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: taker_payment_refund.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: taker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: taker_amount.clone(),
        watcher_reward: Some(watcher_reward.clone()),
        spend_type: WatcherSpendType::TakerPaymentRefund,
    };

    let error = block_on_f01(maker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("Transaction sender arg"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            "invalid refund tx sender arg", error
        ),
    }

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: taker_payment_refund.tx_hex(),
        maker_pub: taker_pub.to_vec(),
        swap_contract_address: taker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: taker_amount.clone(),
        watcher_reward: Some(watcher_reward.clone()),
        spend_type: WatcherSpendType::TakerPaymentRefund,
    };

    let error = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("Transaction receiver arg"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            "invalid refund tx receiver arg", error
        ),
    }

    let mut wrong_watcher_reward = watcher_reward.clone();
    wrong_watcher_reward.reward_target = RewardTarget::PaymentReceiver;

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: taker_payment_refund.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: taker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: taker_amount.clone(),
        watcher_reward: Some(wrong_watcher_reward),
        spend_type: WatcherSpendType::TakerPaymentRefund,
    };

    let error = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("Transaction reward target arg"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            "invalid refund tx reward target arg", error
        ),
    }

    let mut wrong_watcher_reward = watcher_reward.clone();
    wrong_watcher_reward.send_contract_reward_on_spend = true;

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: taker_payment_refund.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: taker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: taker_amount.clone(),
        watcher_reward: Some(wrong_watcher_reward),
        spend_type: WatcherSpendType::TakerPaymentRefund,
    };

    let error = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("Transaction sends contract reward on spend arg"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            "invalid refund tx sends contract reward on spend arg", error
        ),
    }

    let mut wrong_watcher_reward = watcher_reward.clone();
    wrong_watcher_reward.amount = BigDecimal::one();

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: taker_payment_refund.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: taker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: taker_amount,
        watcher_reward: Some(wrong_watcher_reward),
        spend_type: WatcherSpendType::TakerPaymentRefund,
    };

    let error = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("Transaction watcher reward amount arg"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            "invalid refund tx watcher reward amount arg", error
        ),
    }

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: taker_payment_refund.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: taker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: BigDecimal::one(),
        watcher_reward: Some(watcher_reward),
        spend_type: WatcherSpendType::TakerPaymentRefund,
    };

    let error = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("Transaction amount arg"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            "invalid refund tx amount arg", error
        ),
    }
}

#[test]
fn test_taker_validates_taker_payment_refund_erc20() {
    let timeout = wait_until_sec(120); // timeout if test takes more than 120 seconds to run

    let taker_coin = erc20_coin_with_random_privkey(watchers_swap_contract());
    let taker_keypair = taker_coin.derive_htlc_key_pair(&[]);
    let taker_pub = taker_keypair.public();

    let maker_seed = get_passphrase!(".env.client", "BOB_PASSPHRASE").unwrap();
    let maker_keypair = key_pair_from_seed(&maker_seed).unwrap();
    let maker_pub = maker_keypair.public();

    let time_lock_duration = get_payment_locktime();
    let wait_for_confirmation_until = wait_until_sec(time_lock_duration);
    let time_lock = now_sec() - 10;

    let secret_hash = dhash160(&generate_secret().unwrap());

    let taker_amount = BigDecimal::from_str("0.001").unwrap();
    let maker_amount = BigDecimal::from_str("0.001").unwrap();

    let watcher_reward = Some(
        block_on(taker_coin.get_taker_watcher_reward(
            &MmCoinEnum::from(taker_coin.clone()),
            Some(taker_amount.clone()),
            Some(maker_amount),
            None,
            wait_for_confirmation_until,
        ))
        .unwrap(),
    );

    let taker_payment = block_on(taker_coin.send_taker_payment(SendPaymentArgs {
        time_lock_duration,
        time_lock,
        other_pubkey: maker_pub,
        secret_hash: secret_hash.as_slice(),
        amount: taker_amount.clone(),
        swap_contract_address: &taker_coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: watcher_reward.clone(),
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
        taker_pub,
        secret_hash.as_slice(),
        &taker_coin.swap_contract_address(),
        &[],
    ))
    .unwrap();

    let taker_payment_refund = block_on_f01(taker_coin.send_taker_payment_refund_preimage(RefundPaymentArgs {
        payment_tx: &taker_payment_refund_preimage.tx_hex(),
        other_pubkey: taker_pub,
        tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
            maker_secret_hash: secret_hash.as_slice(),
        },
        time_lock,
        swap_contract_address: &taker_coin.swap_contract_address(),
        swap_unique_data: &[],
        watcher_reward: true,
    }))
    .unwrap();

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: taker_payment_refund.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: taker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: taker_amount,
        watcher_reward: watcher_reward.clone(),
        spend_type: WatcherSpendType::TakerPaymentRefund,
    };

    let validate_watcher_refund = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input));
    assert!(validate_watcher_refund.is_ok());

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: taker_payment_refund.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: taker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: BigDecimal::one(),
        watcher_reward,
        spend_type: WatcherSpendType::TakerPaymentRefund,
    };

    let error = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("Transaction amount arg"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            "invalid refund tx amount arg", error
        ),
    }
}

#[test]
fn test_taker_validates_maker_payment_spend_eth() {
    let timeout = wait_until_sec(120); // timeout if test takes more than 120 seconds to run

    let taker_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let taker_keypair = taker_coin.derive_htlc_key_pair(&[]);
    let taker_pub = taker_keypair.public();

    let maker_coin = eth_coin_with_random_privkey(watchers_swap_contract());
    let maker_keypair = maker_coin.derive_htlc_key_pair(&[]);
    let maker_pub = maker_keypair.public();

    let time_lock_duration = get_payment_locktime();
    let wait_for_confirmation_until = wait_until_sec(time_lock_duration);
    let time_lock = wait_for_confirmation_until;
    let maker_amount = BigDecimal::from_str("0.001").unwrap();

    let secret = generate_secret().unwrap();
    let secret_hash = dhash160(&secret);

    let watcher_reward = block_on(maker_coin.get_maker_watcher_reward(
        &MmCoinEnum::from(taker_coin.clone()),
        None,
        wait_for_confirmation_until,
    ))
    .unwrap()
    .unwrap();

    let maker_payment = block_on(maker_coin.send_maker_payment(SendPaymentArgs {
        time_lock_duration,
        time_lock,
        other_pubkey: taker_pub,
        secret_hash: secret_hash.as_slice(),
        amount: maker_amount.clone(),
        swap_contract_address: &maker_coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: Some(watcher_reward.clone()),
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
        maker_pub,
        secret_hash.as_slice(),
        &[],
    ))
    .unwrap();

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: maker_payment_spend_preimage.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: maker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: maker_amount.clone(),
        watcher_reward: Some(watcher_reward.clone()),
        spend_type: WatcherSpendType::MakerPaymentSpend,
    };

    let error = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::UnexpectedPaymentState(err) => {
            assert!(err.contains("Payment state is not"))
        },
        _ => panic!(
            "Expected `UnexpectedPaymentState` {}, found {:?}",
            "invalid payment state", error
        ),
    }

    let maker_payment_spend = block_on_f01(taker_coin.send_maker_payment_spend_preimage(
        SendMakerPaymentSpendPreimageInput {
            preimage: &maker_payment_spend_preimage.tx_hex(),
            secret_hash: secret_hash.as_slice(),
            secret: secret.as_slice(),
            taker_pub,
            watcher_reward: true,
        },
    ))
    .unwrap();

    block_on_f01(maker_coin.wait_for_confirmations(ConfirmPaymentInput {
        payment_tx: maker_payment_spend.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    }))
    .unwrap();

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: maker_payment_spend.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: maker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: maker_amount.clone(),
        watcher_reward: Some(watcher_reward.clone()),
        spend_type: WatcherSpendType::MakerPaymentSpend,
    };

    block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input)).unwrap();

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: maker_payment_spend.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: Some("9130b257d37a52e52f21054c4da3450c72f595ce".into()),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: maker_amount.clone(),
        watcher_reward: Some(watcher_reward.clone()),
        spend_type: WatcherSpendType::MakerPaymentSpend,
    };

    let error = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("was sent to wrong address"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            "invalid contract address", error
        ),
    };

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: maker_payment_spend.tx_hex(),
        maker_pub: taker_pub.to_vec(),
        swap_contract_address: maker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: maker_amount.clone(),
        watcher_reward: Some(watcher_reward.clone()),
        spend_type: WatcherSpendType::MakerPaymentSpend,
    };

    let error = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("Transaction sender arg"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            "invalid payment spend tx sender arg", error
        ),
    };

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: maker_payment_spend.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: maker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: maker_amount.clone(),
        watcher_reward: Some(watcher_reward.clone()),
        spend_type: WatcherSpendType::MakerPaymentSpend,
    };

    let error = block_on_f01(maker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("Transaction receiver arg"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            "invalid payment spend tx receiver arg", error
        ),
    };

    let mut wrong_watcher_reward = watcher_reward.clone();
    wrong_watcher_reward.reward_target = RewardTarget::Contract;

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: maker_payment_spend.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: maker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: maker_amount.clone(),
        watcher_reward: Some(wrong_watcher_reward),
        spend_type: WatcherSpendType::MakerPaymentSpend,
    };

    let error = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("Transaction reward target arg"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            "invalid payment spend tx reward target arg", error
        ),
    };

    let mut wrong_watcher_reward = watcher_reward.clone();
    wrong_watcher_reward.send_contract_reward_on_spend = false;

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: maker_payment_spend.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: maker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: maker_amount.clone(),
        watcher_reward: Some(wrong_watcher_reward),
        spend_type: WatcherSpendType::MakerPaymentSpend,
    };

    let error = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("Transaction sends contract reward on spend arg"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            "invalid payment spend tx sends contract reward on spend arg", error
        ),
    };

    let mut wrong_watcher_reward = watcher_reward.clone();
    wrong_watcher_reward.amount = BigDecimal::one();

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: maker_payment_spend.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: maker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: maker_amount,
        watcher_reward: Some(wrong_watcher_reward),
        spend_type: WatcherSpendType::MakerPaymentSpend,
    };

    let error = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("Transaction watcher reward amount arg"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            "invalid payment spend tx watcher reward amount arg", error
        ),
    };

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: maker_payment_spend.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: maker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: BigDecimal::one(),
        watcher_reward: Some(watcher_reward),
        spend_type: WatcherSpendType::MakerPaymentSpend,
    };

    let error = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("Transaction amount arg"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            "invalid payment spend tx amount arg", error
        ),
    };
}

#[test]
fn test_taker_validates_maker_payment_spend_erc20() {
    let timeout = wait_until_sec(120); // timeout if test takes more than 120 seconds to run

    let taker_coin = erc20_coin_with_random_privkey(watchers_swap_contract());
    let taker_keypair = taker_coin.derive_htlc_key_pair(&[]);
    let taker_pub = taker_keypair.public();

    let maker_coin = erc20_coin_with_random_privkey(watchers_swap_contract());
    let maker_keypair = maker_coin.derive_htlc_key_pair(&[]);
    let maker_pub = maker_keypair.public();

    let time_lock_duration = get_payment_locktime();
    let wait_for_confirmation_until = wait_until_sec(time_lock_duration);
    let time_lock = wait_for_confirmation_until;
    let maker_amount = BigDecimal::from_str("0.001").unwrap();

    let secret = generate_secret().unwrap();
    let secret_hash = dhash160(&secret);

    let watcher_reward = block_on(maker_coin.get_maker_watcher_reward(
        &MmCoinEnum::from(taker_coin.clone()),
        None,
        wait_for_confirmation_until,
    ))
    .unwrap();

    let maker_payment = block_on(maker_coin.send_maker_payment(SendPaymentArgs {
        time_lock_duration,
        time_lock,
        other_pubkey: taker_pub,
        secret_hash: secret_hash.as_slice(),
        amount: maker_amount.clone(),
        swap_contract_address: &maker_coin.swap_contract_address(),
        swap_unique_data: &[],
        payment_instructions: &None,
        watcher_reward: watcher_reward.clone(),
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
        maker_pub,
        secret_hash.as_slice(),
        &[],
    ))
    .unwrap();

    let maker_payment_spend = block_on_f01(taker_coin.send_maker_payment_spend_preimage(
        SendMakerPaymentSpendPreimageInput {
            preimage: &maker_payment_spend_preimage.tx_hex(),
            secret_hash: secret_hash.as_slice(),
            secret: secret.as_slice(),
            taker_pub,
            watcher_reward: true,
        },
    ))
    .unwrap();

    block_on_f01(maker_coin.wait_for_confirmations(ConfirmPaymentInput {
        payment_tx: maker_payment_spend.tx_hex(),
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    }))
    .unwrap();

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: maker_payment_spend.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: maker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: maker_amount,
        watcher_reward: watcher_reward.clone(),
        spend_type: WatcherSpendType::MakerPaymentSpend,
    };

    block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input)).unwrap();

    let validate_input = ValidateWatcherSpendInput {
        payment_tx: maker_payment_spend.tx_hex(),
        maker_pub: maker_pub.to_vec(),
        swap_contract_address: maker_coin.swap_contract_address(),
        time_lock,
        secret_hash: secret_hash.to_vec(),
        amount: BigDecimal::one(),
        watcher_reward,
        spend_type: WatcherSpendType::MakerPaymentSpend,
    };

    let error = block_on_f01(taker_coin.taker_validates_payment_spend_or_refund(validate_input))
        .unwrap_err()
        .into_inner();
    log!("error: {:?}", error);
    match error {
        ValidatePaymentError::WrongPaymentTx(err) => {
            assert!(err.contains("Transaction amount arg"))
        },
        _ => panic!(
            "Expected `WrongPaymentTx` {}, found {:?}",
            "invalid payment spend tx amount arg", error
        ),
    };
}

#[test]
fn test_watcher_reward() {
    let timeout = wait_until_sec(300); // timeout if test takes more than 300 seconds to run
    let (_ctx, utxo_coin, _) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000u64.into());
    let eth_coin = eth_coin_with_random_privkey(watchers_swap_contract());

    let watcher_reward = block_on(eth_coin.get_taker_watcher_reward(
        &MmCoinEnum::EthCoinVariant(eth_coin.clone()),
        None,
        None,
        None,
        timeout,
    ))
    .unwrap();
    assert!(!watcher_reward.is_exact_amount);
    assert!(matches!(watcher_reward.reward_target, RewardTarget::Contract));
    assert!(!watcher_reward.send_contract_reward_on_spend);

    let watcher_reward = block_on(eth_coin.get_taker_watcher_reward(
        &MmCoinEnum::EthCoinVariant(eth_coin.clone()),
        None,
        None,
        Some(BigDecimal::one()),
        timeout,
    ))
    .unwrap();
    // assert!(watcher_reward.is_exact_amount);
    assert!(matches!(watcher_reward.reward_target, RewardTarget::Contract));
    assert!(!watcher_reward.send_contract_reward_on_spend);
    assert_eq!(watcher_reward.amount, BigDecimal::one());

    let watcher_reward = block_on(eth_coin.get_taker_watcher_reward(
        &MmCoinEnum::UtxoCoinVariant(utxo_coin.clone()),
        None,
        None,
        None,
        timeout,
    ))
    .unwrap();
    assert!(!watcher_reward.is_exact_amount);
    assert!(matches!(watcher_reward.reward_target, RewardTarget::PaymentSender));
    assert!(!watcher_reward.send_contract_reward_on_spend);

    let watcher_reward =
        block_on(eth_coin.get_maker_watcher_reward(&MmCoinEnum::EthCoinVariant(eth_coin.clone()), None, timeout))
            .unwrap()
            .unwrap();
    assert!(!watcher_reward.is_exact_amount);
    assert!(matches!(watcher_reward.reward_target, RewardTarget::None));
    assert!(watcher_reward.send_contract_reward_on_spend);

    let watcher_reward = block_on(eth_coin.get_maker_watcher_reward(
        &MmCoinEnum::EthCoinVariant(eth_coin.clone()),
        Some(BigDecimal::one()),
        timeout,
    ))
    .unwrap()
    .unwrap();
    assert!(watcher_reward.is_exact_amount);
    assert!(matches!(watcher_reward.reward_target, RewardTarget::None));
    assert!(watcher_reward.send_contract_reward_on_spend);
    assert_eq!(watcher_reward.amount, BigDecimal::one());

    let watcher_reward =
        block_on(eth_coin.get_maker_watcher_reward(&MmCoinEnum::UtxoCoinVariant(utxo_coin.clone()), None, timeout))
            .unwrap()
            .unwrap();
    assert!(!watcher_reward.is_exact_amount);
    assert!(matches!(watcher_reward.reward_target, RewardTarget::PaymentSpender));
    assert!(!watcher_reward.send_contract_reward_on_spend);

    let watcher_reward = block_on(utxo_coin.get_taker_watcher_reward(
        &MmCoinEnum::EthCoinVariant(eth_coin),
        Some(BigDecimal::from_str("0.01").unwrap()),
        Some(BigDecimal::from_str("1").unwrap()),
        None,
        timeout,
    ))
    .unwrap();
    assert!(!watcher_reward.is_exact_amount);
    assert!(matches!(watcher_reward.reward_target, RewardTarget::PaymentReceiver));
    assert!(!watcher_reward.send_contract_reward_on_spend);

    let watcher_reward =
        block_on(utxo_coin.get_maker_watcher_reward(&MmCoinEnum::UtxoCoinVariant(utxo_coin.clone()), None, timeout))
            .unwrap();
    assert!(watcher_reward.is_none());
}
