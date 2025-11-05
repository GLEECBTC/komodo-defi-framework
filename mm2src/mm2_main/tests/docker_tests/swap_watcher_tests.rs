use crate::docker_tests::docker_tests_common::GETH_RPC_URL;
use crate::docker_tests::eth_docker_tests::{
    erc20_coin_with_random_privkey, erc20_contract_checksum, eth_coin_with_random_privkey, watchers_swap_contract,
};
use crate::integration_tests_common::*;
use crate::{generate_utxo_coin_with_privkey, generate_utxo_coin_with_random_privkey, random_secp256k1_secret};
use coins::coin_errors::ValidatePaymentError;
use coins::eth::{checksum_address, EthCoin};
use coins::utxo::utxo_standard::UtxoStandardCoin;
use coins::utxo::{dhash160, UtxoCommonOps};
use coins::{
    ConfirmPaymentInput, DexFee, FoundSwapTxSpend, MarketCoinOps, MmCoin, MmCoinEnum, RefundPaymentArgs, RewardTarget,
    SearchForSwapTxSpendInput, SendMakerPaymentSpendPreimageInput, SendPaymentArgs, SwapOps, SwapTxTypeWithSecretHash,
    TestCoin, ValidateWatcherSpendInput, WatcherOps, WatcherSpendType, WatcherValidatePaymentInput,
    WatcherValidateTakerFeeInput, EARLY_CONFIRMATION_ERR_LOG, INVALID_CONTRACT_ADDRESS_ERR_LOG,
    INVALID_PAYMENT_STATE_ERR_LOG, INVALID_RECEIVER_ERR_LOG, INVALID_REFUND_TX_ERR_LOG, INVALID_SCRIPT_ERR_LOG,
    INVALID_SENDER_ERR_LOG, INVALID_SWAP_ID_ERR_LOG, OLD_TRANSACTION_ERR_LOG,
};
use common::{block_on, block_on_f01, now_sec, wait_until_sec};
use crypto::privkey::{key_pair_from_secret, key_pair_from_seed};
use mm2_main::lp_swap::{
    generate_secret, get_payment_locktime, MAKER_PAYMENT_SENT_LOG, MAKER_PAYMENT_SPEND_FOUND_LOG,
    MAKER_PAYMENT_SPEND_SENT_LOG, REFUND_TEST_FAILURE_LOG, TAKER_PAYMENT_REFUND_SENT_LOG, WATCHER_MESSAGE_SENT_LOG,
};
use mm2_number::BigDecimal;
use mm2_number::MmNumber;
use mm2_test_helpers::for_tests::{
    enable_eth_coin, erc20_dev_conf, eth_dev_conf, eth_jst_testnet_conf, mm_dump, my_balance, my_swap_status,
    mycoin1_conf, mycoin_conf, start_swaps, wait_for_swaps_finish_and_check_status, MarketMakerIt, Mm2TestConf,
    DEFAULT_RPC_PASSWORD,
};
use mm2_test_helpers::get_passphrase;
use mm2_test_helpers::structs::WatcherConf;
use mocktopus::mocking::*;
use num_traits::{One, Zero};
use primitives::hash::H256;
use serde_json::Value;
use std::str::FromStr;
use std::thread;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone)]
struct BalanceResult {
    alice_acoin_balance_before: BigDecimal,
    alice_acoin_balance_middle: BigDecimal,
    alice_acoin_balance_after: BigDecimal,
    alice_bcoin_balance_before: BigDecimal,
    alice_bcoin_balance_middle: BigDecimal,
    alice_bcoin_balance_after: BigDecimal,
    alice_eth_balance_middle: BigDecimal,
    alice_eth_balance_after: BigDecimal,
    bob_acoin_balance_before: BigDecimal,
    bob_acoin_balance_after: BigDecimal,
    bob_bcoin_balance_before: BigDecimal,
    bob_bcoin_balance_after: BigDecimal,
    watcher_acoin_balance_before: BigDecimal,
    watcher_acoin_balance_after: BigDecimal,
    watcher_bcoin_balance_before: BigDecimal,
    watcher_bcoin_balance_after: BigDecimal,
}

fn enable_coin(mm_node: &MarketMakerIt, coin: &str) {
    if coin == "MYCOIN" {
        log!("{:?}", block_on(enable_native(mm_node, coin, &[], None)));
    } else {
        enable_eth(mm_node, coin);
    }
}

fn enable_eth(mm_node: &MarketMakerIt, coin: &str) {
    dbg!(block_on(enable_eth_coin(
        mm_node,
        coin,
        &[GETH_RPC_URL],
        &checksum_address(&format!("{:02x}", watchers_swap_contract())),
        Some(&checksum_address(&format!("{:02x}", watchers_swap_contract()))),
        true
    )));
}

#[allow(clippy::enum_variant_names)]
enum SwapFlow {
    WatcherSpendsMakerPayment,
    WatcherRefundsTakerPayment,
    TakerSpendsMakerPayment,
}

#[allow(clippy::too_many_arguments)]
fn start_swaps_and_get_balances(
    a_coin: &'static str,
    b_coin: &'static str,
    maker_price: f64,
    taker_price: f64,
    volume: f64,
    envs: &[(&str, &str)],
    swap_flow: SwapFlow,
    alice_privkey: &str,
    bob_privkey: &str,
    watcher_privkey: &str,
    custom_locktime: Option<u64>,
) -> BalanceResult {
    let coins = json!([
        eth_dev_conf(),
        erc20_dev_conf(&erc20_contract_checksum()),
        mycoin_conf(1000),
        mycoin1_conf(1000)
    ]);

    let mut alice_conf = Mm2TestConf::seednode(&format!("0x{alice_privkey}"), &coins);
    if let Some(locktime) = custom_locktime {
        alice_conf.conf["payment_locktime"] = locktime.into();
    }
    let mut mm_alice = block_on(MarketMakerIt::start_with_envs(
        alice_conf.conf.clone(),
        alice_conf.rpc_password.clone(),
        None,
        envs,
    ))
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    let mut bob_conf = Mm2TestConf::light_node(&format!("0x{bob_privkey}"), &coins, &[&mm_alice.ip.to_string()]);
    if let Some(locktime) = custom_locktime {
        bob_conf.conf["payment_locktime"] = locktime.into();
    }
    let mut mm_bob = block_on(MarketMakerIt::start_with_envs(
        bob_conf.conf.clone(),
        bob_conf.rpc_password,
        None,
        envs,
    ))
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    generate_utxo_coin_with_privkey("MYCOIN", 100.into(), H256::from_str(bob_privkey).unwrap());
    generate_utxo_coin_with_privkey("MYCOIN", 100.into(), H256::from_str(alice_privkey).unwrap());
    generate_utxo_coin_with_privkey("MYCOIN1", 100.into(), H256::from_str(bob_privkey).unwrap());
    generate_utxo_coin_with_privkey("MYCOIN1", 100.into(), H256::from_str(alice_privkey).unwrap());

    let (watcher_conf, watcher_log_to_wait) = match swap_flow {
        SwapFlow::WatcherSpendsMakerPayment => (
            WatcherConf {
                wait_taker_payment: 0.,
                wait_maker_payment_spend_factor: 0.,
                refund_start_factor: 1.5,
                search_interval: 1.0,
            },
            MAKER_PAYMENT_SPEND_SENT_LOG,
        ),
        SwapFlow::WatcherRefundsTakerPayment => (
            WatcherConf {
                wait_taker_payment: 0.,
                wait_maker_payment_spend_factor: 1.,
                refund_start_factor: 0.,
                search_interval: 1.,
            },
            TAKER_PAYMENT_REFUND_SENT_LOG,
        ),
        SwapFlow::TakerSpendsMakerPayment => (
            WatcherConf {
                wait_taker_payment: 0.,
                wait_maker_payment_spend_factor: 1.,
                refund_start_factor: 1.5,
                search_interval: 1.0,
            },
            MAKER_PAYMENT_SPEND_FOUND_LOG,
        ),
    };

    let mut watcher_conf = Mm2TestConf::watcher_light_node(
        &format!("0x{watcher_privkey}"),
        &coins,
        &[&mm_alice.ip.to_string()],
        watcher_conf,
    )
    .conf;
    if let Some(locktime) = custom_locktime {
        watcher_conf["payment_locktime"] = locktime.into();
    }

    let mut mm_watcher = block_on(MarketMakerIt::start_with_envs(
        watcher_conf,
        DEFAULT_RPC_PASSWORD.to_string(),
        None,
        envs,
    ))
    .unwrap();
    let (_watcher_dump_log, _watcher_dump_dashboard) = mm_dump(&mm_watcher.log_path);
    log!("Watcher log path: {}", mm_watcher.log_path.display());

    enable_coin(&mm_alice, a_coin);
    enable_coin(&mm_alice, b_coin);
    enable_coin(&mm_bob, a_coin);
    enable_coin(&mm_bob, b_coin);
    enable_coin(&mm_watcher, a_coin);
    enable_coin(&mm_watcher, b_coin);

    if a_coin != "ETH" && b_coin != "ETH" {
        enable_coin(&mm_alice, "ETH");
    }

    let alice_acoin_balance_before = block_on(my_balance(&mm_alice, a_coin)).balance;
    let alice_bcoin_balance_before = block_on(my_balance(&mm_alice, b_coin)).balance;
    let bob_acoin_balance_before = block_on(my_balance(&mm_bob, a_coin)).balance;
    let bob_bcoin_balance_before = block_on(my_balance(&mm_bob, b_coin)).balance;
    let watcher_acoin_balance_before = block_on(my_balance(&mm_watcher, a_coin)).balance;
    let watcher_bcoin_balance_before = block_on(my_balance(&mm_watcher, b_coin)).balance;

    let mut alice_acoin_balance_middle = BigDecimal::zero();
    let mut alice_bcoin_balance_middle = BigDecimal::zero();
    let mut alice_eth_balance_middle = BigDecimal::zero();
    let mut bob_acoin_balance_after = BigDecimal::zero();
    let mut bob_bcoin_balance_after = BigDecimal::zero();

    block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[(b_coin, a_coin)],
        maker_price,
        taker_price,
        volume,
    ));

    if matches!(swap_flow, SwapFlow::WatcherRefundsTakerPayment) {
        block_on(mm_bob.wait_for_log(120., |log| log.contains(MAKER_PAYMENT_SENT_LOG))).unwrap();
        block_on(mm_bob.stop()).unwrap();
    }
    if !matches!(swap_flow, SwapFlow::TakerSpendsMakerPayment) {
        block_on(mm_alice.wait_for_log(120., |log| log.contains(WATCHER_MESSAGE_SENT_LOG))).unwrap();
        alice_acoin_balance_middle = block_on(my_balance(&mm_alice, a_coin)).balance;
        alice_bcoin_balance_middle = block_on(my_balance(&mm_alice, b_coin)).balance;
        alice_eth_balance_middle = block_on(my_balance(&mm_alice, "ETH")).balance;
        block_on(mm_alice.stop()).unwrap();
    }

    block_on(mm_watcher.wait_for_log(120., |log| log.contains(watcher_log_to_wait))).unwrap();
    thread::sleep(Duration::from_secs(20));

    let mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();
    enable_coin(&mm_alice, a_coin);
    enable_coin(&mm_alice, b_coin);

    if a_coin != "ETH" && b_coin != "ETH" {
        enable_coin(&mm_alice, "ETH");
    }

    let alice_acoin_balance_after = block_on(my_balance(&mm_alice, a_coin)).balance;
    let alice_bcoin_balance_after = block_on(my_balance(&mm_alice, b_coin)).balance;
    let alice_eth_balance_after = block_on(my_balance(&mm_alice, "ETH")).balance;
    if !matches!(swap_flow, SwapFlow::WatcherRefundsTakerPayment) {
        bob_acoin_balance_after = block_on(my_balance(&mm_bob, a_coin)).balance;
        bob_bcoin_balance_after = block_on(my_balance(&mm_bob, b_coin)).balance;
    }
    let watcher_acoin_balance_after = block_on(my_balance(&mm_watcher, a_coin)).balance;
    let watcher_bcoin_balance_after = block_on(my_balance(&mm_watcher, b_coin)).balance;

    BalanceResult {
        alice_acoin_balance_before,
        alice_acoin_balance_middle,
        alice_acoin_balance_after,
        alice_bcoin_balance_before,
        alice_bcoin_balance_middle,
        alice_bcoin_balance_after,
        alice_eth_balance_middle,
        alice_eth_balance_after,
        bob_acoin_balance_before,
        bob_acoin_balance_after,
        bob_bcoin_balance_before,
        bob_bcoin_balance_after,
        watcher_acoin_balance_before,
        watcher_acoin_balance_after,
        watcher_bcoin_balance_before,
        watcher_bcoin_balance_after,
    }
}

fn check_actual_events(mm_alice: &MarketMakerIt, uuid: &str, expected_events: &[&'static str]) -> Value {
    let status_response = block_on(my_swap_status(mm_alice, uuid)).unwrap();
    let events_array = status_response["result"]["events"].as_array().unwrap();
    let actual_events = events_array.iter().map(|item| item["event"]["type"].as_str().unwrap());
    let actual_events: Vec<&str> = actual_events.collect();
    assert_eq!(expected_events, actual_events.as_slice());
    status_response
}

fn run_taker_node(
    coins: &Value,
    envs: &[(&str, &str)],
    seednodes: &[&str],
    custom_locktime: Option<u64>,
) -> (MarketMakerIt, Mm2TestConf) {
    let privkey = hex::encode(random_secp256k1_secret());
    let mut conf = Mm2TestConf::light_node(&format!("0x{privkey}"), coins, seednodes);
    if let Some(locktime) = custom_locktime {
        conf.conf["payment_locktime"] = locktime.into();
    }
    let mm = block_on(MarketMakerIt::start_with_envs(
        conf.conf.clone(),
        conf.rpc_password.clone(),
        None,
        envs,
    ))
    .unwrap();
    let (_dump_log, _dump_dashboard) = mm.mm_dump();
    log!("Log path: {}", mm.log_path.display());

    generate_utxo_coin_with_privkey("MYCOIN", 100.into(), H256::from_str(&privkey).unwrap());
    generate_utxo_coin_with_privkey("MYCOIN1", 100.into(), H256::from_str(&privkey).unwrap());
    enable_coin(&mm, "MYCOIN");
    enable_coin(&mm, "MYCOIN1");

    (mm, conf)
}

fn restart_taker_and_wait_until(conf: &Mm2TestConf, envs: &[(&str, &str)], wait_until: &str) -> MarketMakerIt {
    let mut mm_alice = block_on(MarketMakerIt::start_with_envs(
        conf.conf.clone(),
        conf.rpc_password.clone(),
        None,
        envs,
    ))
    .unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());
    enable_coin(&mm_alice, "MYCOIN");
    enable_coin(&mm_alice, "MYCOIN1");

    block_on(mm_alice.wait_for_log(120., |log| log.contains(wait_until))).unwrap();
    mm_alice
}

fn run_maker_node(
    coins: &Value,
    envs: &[(&str, &str)],
    seednodes: &[&str],
    custom_locktime: Option<u64>,
) -> MarketMakerIt {
    let privkey = hex::encode(random_secp256k1_secret());
    let mut conf = if seednodes.is_empty() {
        Mm2TestConf::seednode(&format!("0x{privkey}"), coins)
    } else {
        Mm2TestConf::light_node(&format!("0x{privkey}"), coins, seednodes)
    };
    if let Some(locktime) = custom_locktime {
        conf.conf["payment_locktime"] = locktime.into();
    }
    let mm = block_on(MarketMakerIt::start_with_envs(
        conf.conf.clone(),
        conf.rpc_password,
        None,
        envs,
    ))
    .unwrap();
    let (_dump_log, _dump_dashboard) = mm.mm_dump();
    log!("Log path: {}", mm.log_path.display());

    generate_utxo_coin_with_privkey("MYCOIN", 100.into(), H256::from_str(&privkey).unwrap());
    generate_utxo_coin_with_privkey("MYCOIN1", 100.into(), H256::from_str(&privkey).unwrap());
    enable_coin(&mm, "MYCOIN");
    enable_coin(&mm, "MYCOIN1");

    mm
}

fn run_watcher_node(
    coins: &Value,
    envs: &[(&str, &str)],
    seednodes: &[&str],
    watcher_conf: WatcherConf,
    custom_locktime: Option<u64>,
) -> MarketMakerIt {
    let privkey = hex::encode(random_secp256k1_secret());
    let mut conf = Mm2TestConf::watcher_light_node(&format!("0x{privkey}"), coins, seednodes, watcher_conf).conf;
    if let Some(locktime) = custom_locktime {
        conf["payment_locktime"] = locktime.into();
    }
    let mm = block_on(MarketMakerIt::start_with_envs(
        conf,
        DEFAULT_RPC_PASSWORD.to_string(),
        None,
        envs,
    ))
    .unwrap();
    let (_dump_log, _dump_dashboard) = mm.mm_dump();
    log!("Log path: {}", mm.log_path.display());

    generate_utxo_coin_with_privkey("MYCOIN", 100.into(), H256::from_str(&privkey).unwrap());
    generate_utxo_coin_with_privkey("MYCOIN1", 100.into(), H256::from_str(&privkey).unwrap());
    enable_coin(&mm, "MYCOIN");
    enable_coin(&mm, "MYCOIN1");

    mm
}

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

    assert_eq!(
        balances.alice_acoin_balance_after.round(0),
        balances.alice_acoin_balance_before - acoin_volume.clone()
    );
    assert_eq!(
        balances.alice_bcoin_balance_after.round(0),
        balances.alice_bcoin_balance_before + bcoin_volume.clone()
    );
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

    assert_eq!(
        balances.alice_acoin_balance_after.round(0),
        balances.alice_acoin_balance_before
    );
    assert_eq!(balances.alice_bcoin_balance_after, balances.alice_bcoin_balance_before);
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
    if watcher1_eth_balance_after > watcher1_eth_balance_before {
        assert_eq!(watcher2_eth_balance_after, watcher2_eth_balance_after);
    }
    if watcher2_eth_balance_after > watcher2_eth_balance_before {
        assert_eq!(watcher1_eth_balance_after, watcher1_eth_balance_after);
    }
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
        other_pubkey: maker_pubkey,
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
        maker_pubkey,
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
            maker_coin: MmCoinEnum::UtxoCoin(maker_coin.clone()),
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
        maker_coin: MmCoinEnum::UtxoCoin(maker_coin.clone()),
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
        maker_coin: MmCoinEnum::UtxoCoin(maker_coin.clone()),
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
        other_pubkey: maker_pubkey,
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
        maker_coin: MmCoinEnum::UtxoCoin(maker_coin.clone()),
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
        maker_pubkey,
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
        maker_coin: MmCoinEnum::UtxoCoin(maker_coin.clone()),
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
            maker_coin: MmCoinEnum::EthCoin(taker_coin.clone()),
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
            maker_coin: MmCoinEnum::EthCoin(taker_coin.clone()),
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
            maker_coin: MmCoinEnum::EthCoin(taker_coin.clone()),
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
            maker_coin: MmCoinEnum::EthCoin(taker_coin.clone()),
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
        maker_coin: MmCoinEnum::EthCoin(taker_coin.clone()),
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
        maker_coin: MmCoinEnum::EthCoin(taker_coin.clone()),
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
            maker_coin: MmCoinEnum::EthCoin(taker_coin.clone()),
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
        maker_coin: MmCoinEnum::EthCoin(taker_coin.clone()),
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
        maker_coin: MmCoinEnum::EthCoin(taker_coin.clone()),
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
        maker_coin: MmCoinEnum::EthCoin(taker_coin.clone()),
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
        maker_coin: MmCoinEnum::EthCoin(taker_coin.clone()),
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
        maker_coin: MmCoinEnum::EthCoin(taker_coin.clone()),
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
        other_pubkey: maker_pubkey,
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
        maker_pubkey,
        secret_hash.as_slice(),
        &None,
        &[],
    ))
    .unwrap();

    let taker_payment_refund = block_on_f01(taker_coin.send_taker_payment_refund_preimage(RefundPaymentArgs {
        payment_tx: &taker_payment_refund_preimage.tx_hex(),
        other_pubkey: maker_pubkey,
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
        other_pubkey: taker_pubkey,
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
        maker_pubkey,
        secret_hash.as_slice(),
        &[],
    ))
    .unwrap();

    let maker_payment_spend = block_on_f01(taker_coin.send_maker_payment_spend_preimage(
        SendMakerPaymentSpendPreimageInput {
            preimage: &maker_payment_spend_preimage.tx_hex(),
            secret_hash: secret_hash.as_slice(),
            secret: secret.as_slice(),
            taker_pub: taker_pubkey,
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
fn test_send_taker_payment_refund_preimage_utxo() {
    let timeout = wait_until_sec(120); // timeout if test takes more than 120 seconds to run
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

    let refund_tx = block_on_f01(coin.create_taker_payment_refund_preimage(
        &tx.tx_hex(),
        time_lock,
        my_public_key,
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
        other_pubkey: my_public_key,
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
fn test_watcher_reward() {
    let timeout = wait_until_sec(300); // timeout if test takes more than 300 seconds to run
    let (_ctx, utxo_coin, _) = generate_utxo_coin_with_random_privkey("MYCOIN", 1000u64.into());
    let eth_coin = eth_coin_with_random_privkey(watchers_swap_contract());

    let watcher_reward =
        block_on(eth_coin.get_taker_watcher_reward(&MmCoinEnum::EthCoin(eth_coin.clone()), None, None, None, timeout))
            .unwrap();
    assert!(!watcher_reward.is_exact_amount);
    assert!(matches!(watcher_reward.reward_target, RewardTarget::Contract));
    assert!(!watcher_reward.send_contract_reward_on_spend);

    let watcher_reward = block_on(eth_coin.get_taker_watcher_reward(
        &MmCoinEnum::EthCoin(eth_coin.clone()),
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
        &MmCoinEnum::UtxoCoin(utxo_coin.clone()),
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
        block_on(eth_coin.get_maker_watcher_reward(&MmCoinEnum::EthCoin(eth_coin.clone()), None, timeout))
            .unwrap()
            .unwrap();
    assert!(!watcher_reward.is_exact_amount);
    assert!(matches!(watcher_reward.reward_target, RewardTarget::None));
    assert!(watcher_reward.send_contract_reward_on_spend);

    let watcher_reward = block_on(eth_coin.get_maker_watcher_reward(
        &MmCoinEnum::EthCoin(eth_coin.clone()),
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
        block_on(eth_coin.get_maker_watcher_reward(&MmCoinEnum::UtxoCoin(utxo_coin.clone()), None, timeout))
            .unwrap()
            .unwrap();
    assert!(!watcher_reward.is_exact_amount);
    assert!(matches!(watcher_reward.reward_target, RewardTarget::PaymentSpender));
    assert!(!watcher_reward.send_contract_reward_on_spend);

    let watcher_reward = block_on(utxo_coin.get_taker_watcher_reward(
        &MmCoinEnum::EthCoin(eth_coin),
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
        block_on(utxo_coin.get_maker_watcher_reward(&MmCoinEnum::UtxoCoin(utxo_coin.clone()), None, timeout)).unwrap();
    assert!(watcher_reward.is_none());
}
