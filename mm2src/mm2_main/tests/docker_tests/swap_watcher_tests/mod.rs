//! Swap Watcher Tests
//!
//! Shared helpers for watcher tests. UTXO tests are always enabled,
//! ETH/ERC20 tests require the `docker-tests-watchers-eth` feature.

// UTXO watcher tests - always enabled with docker-tests-watchers
mod utxo;

// ETH/ERC20 watcher tests - disabled by default (unstable, not completed yet)
#[cfg(feature = "docker-tests-watchers-eth")]
mod eth;

// Common imports (used by UTXO watcher tests)
use crate::docker_tests::helpers::env::random_secp256k1_secret;
use crate::docker_tests::helpers::utxo::{generate_utxo_coin_with_privkey, generate_utxo_coin_with_random_privkey};
use crate::integration_tests_common::*;
use coins::coin_errors::ValidatePaymentError;
use coins::utxo::utxo_standard::UtxoStandardCoin;
use coins::utxo::{dhash160, UtxoCommonOps};
use coins::{
    ConfirmPaymentInput, DexFee, FoundSwapTxSpend, MarketCoinOps, MmCoin, MmCoinEnum, RefundPaymentArgs,
    SearchForSwapTxSpendInput, SendMakerPaymentSpendPreimageInput, SendPaymentArgs, SwapOps, SwapTxTypeWithSecretHash,
    ValidateWatcherSpendInput, WatcherOps, WatcherSpendType, WatcherValidatePaymentInput, WatcherValidateTakerFeeInput,
    EARLY_CONFIRMATION_ERR_LOG, INVALID_RECEIVER_ERR_LOG, INVALID_REFUND_TX_ERR_LOG, INVALID_SCRIPT_ERR_LOG,
    INVALID_SENDER_ERR_LOG, OLD_TRANSACTION_ERR_LOG,
};
use common::{block_on, block_on_f01, now_sec, wait_until_sec};
use mm2_main::lp_swap::{
    generate_secret, get_payment_locktime, MAKER_PAYMENT_SENT_LOG, MAKER_PAYMENT_SPEND_FOUND_LOG,
    MAKER_PAYMENT_SPEND_SENT_LOG, REFUND_TEST_FAILURE_LOG, TAKER_PAYMENT_REFUND_SENT_LOG, WATCHER_MESSAGE_SENT_LOG,
};
use mm2_number::BigDecimal;
use mm2_number::MmNumber;
use mm2_test_helpers::for_tests::{
    mm_dump, my_balance, my_swap_status, mycoin1_conf, mycoin_conf, start_swaps,
    wait_for_swaps_finish_and_check_status, MarketMakerIt, Mm2TestConf, DEFAULT_RPC_PASSWORD,
};
use mm2_test_helpers::structs::WatcherConf;
use mocktopus::mocking::*;
use num_traits::Zero;
use serde_json::json;

// ETH-only imports (used only by ETH watcher tests)
#[cfg(feature = "docker-tests-watchers-eth")]
use crate::docker_tests::helpers::eth::{
    erc20_coin_with_random_privkey, erc20_contract_checksum, eth_coin_with_random_privkey, watchers_swap_contract,
    watchers_swap_contract_checksum, GETH_RPC_URL,
};
#[cfg(feature = "docker-tests-watchers-eth")]
use coins::eth::EthCoin;
#[cfg(feature = "docker-tests-watchers-eth")]
use coins::{
    RewardTarget, TestCoin, INVALID_CONTRACT_ADDRESS_ERR_LOG, INVALID_PAYMENT_STATE_ERR_LOG, INVALID_SWAP_ID_ERR_LOG,
};
#[cfg(feature = "docker-tests-watchers-eth")]
use crypto::privkey::{key_pair_from_secret, key_pair_from_seed};
#[cfg(feature = "docker-tests-watchers-eth")]
use mm2_test_helpers::for_tests::{enable_eth_coin, erc20_dev_conf, eth_dev_conf, eth_jst_testnet_conf};
#[cfg(feature = "docker-tests-watchers-eth")]
use mm2_test_helpers::get_passphrase;
#[cfg(feature = "docker-tests-watchers-eth")]
use num_traits::One;
use primitives::hash::H256;
use serde_json::Value;
use std::str::FromStr;
use std::thread;
use std::time::Duration;
use uuid::Uuid;

#[derive(Debug, Clone)]
struct BalanceResult {
    alice_acoin_balance_before: BigDecimal,
    #[cfg(feature = "docker-tests-watchers-eth")]
    alice_acoin_balance_middle: BigDecimal,
    alice_acoin_balance_after: BigDecimal,
    alice_bcoin_balance_before: BigDecimal,
    #[cfg(feature = "docker-tests-watchers-eth")]
    alice_bcoin_balance_middle: BigDecimal,
    alice_bcoin_balance_after: BigDecimal,
    #[cfg(feature = "docker-tests-watchers-eth")]
    alice_eth_balance_middle: BigDecimal,
    #[cfg(feature = "docker-tests-watchers-eth")]
    alice_eth_balance_after: BigDecimal,
    bob_acoin_balance_before: BigDecimal,
    bob_acoin_balance_after: BigDecimal,
    bob_bcoin_balance_before: BigDecimal,
    bob_bcoin_balance_after: BigDecimal,
    #[cfg(feature = "docker-tests-watchers-eth")]
    watcher_acoin_balance_before: BigDecimal,
    #[cfg(feature = "docker-tests-watchers-eth")]
    watcher_acoin_balance_after: BigDecimal,
    #[cfg(feature = "docker-tests-watchers-eth")]
    watcher_bcoin_balance_before: BigDecimal,
    #[cfg(feature = "docker-tests-watchers-eth")]
    watcher_bcoin_balance_after: BigDecimal,
}

fn enable_coin(mm_node: &MarketMakerIt, coin: &str) {
    if coin == "MYCOIN" || coin == "MYCOIN1" {
        log!("{:?}", block_on(enable_native(mm_node, coin, &[], None)));
    } else {
        #[cfg(feature = "docker-tests-watchers-eth")]
        enable_eth(mm_node, coin);
        #[cfg(not(feature = "docker-tests-watchers-eth"))]
        panic!("ETH coin {} requires docker-tests-watchers-eth feature", coin);
    }
}

#[cfg(feature = "docker-tests-watchers-eth")]
fn enable_eth(mm_node: &MarketMakerIt, coin: &str) {
    dbg!(block_on(enable_eth_coin(
        mm_node,
        coin,
        &[GETH_RPC_URL],
        &watchers_swap_contract_checksum(),
        Some(&watchers_swap_contract_checksum()),
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
    #[cfg(feature = "docker-tests-watchers-eth")]
    let coins = json!([
        eth_dev_conf(),
        erc20_dev_conf(&erc20_contract_checksum()),
        mycoin_conf(1000),
        mycoin1_conf(1000)
    ]);
    #[cfg(not(feature = "docker-tests-watchers-eth"))]
    let coins = json!([mycoin_conf(1000), mycoin1_conf(1000)]);

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

    #[cfg(feature = "docker-tests-watchers-eth")]
    if a_coin != "ETH" && b_coin != "ETH" {
        enable_coin(&mm_alice, "ETH");
    }

    let alice_acoin_balance_before = block_on(my_balance(&mm_alice, a_coin)).balance;
    let alice_bcoin_balance_before = block_on(my_balance(&mm_alice, b_coin)).balance;
    let bob_acoin_balance_before = block_on(my_balance(&mm_bob, a_coin)).balance;
    let bob_bcoin_balance_before = block_on(my_balance(&mm_bob, b_coin)).balance;
    #[cfg(feature = "docker-tests-watchers-eth")]
    let watcher_acoin_balance_before = block_on(my_balance(&mm_watcher, a_coin)).balance;
    #[cfg(feature = "docker-tests-watchers-eth")]
    let watcher_bcoin_balance_before = block_on(my_balance(&mm_watcher, b_coin)).balance;

    #[cfg(feature = "docker-tests-watchers-eth")]
    let mut alice_acoin_balance_middle = BigDecimal::zero();
    #[cfg(feature = "docker-tests-watchers-eth")]
    let mut alice_bcoin_balance_middle = BigDecimal::zero();
    #[cfg(feature = "docker-tests-watchers-eth")]
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
        #[cfg(feature = "docker-tests-watchers-eth")]
        {
            alice_acoin_balance_middle = block_on(my_balance(&mm_alice, a_coin)).balance;
            alice_bcoin_balance_middle = block_on(my_balance(&mm_alice, b_coin)).balance;
            alice_eth_balance_middle = block_on(my_balance(&mm_alice, "ETH")).balance;
        }
        block_on(mm_alice.stop()).unwrap();
    }

    block_on(mm_watcher.wait_for_log(120., |log| log.contains(watcher_log_to_wait))).unwrap();
    thread::sleep(Duration::from_secs(20));

    let mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();
    enable_coin(&mm_alice, a_coin);
    enable_coin(&mm_alice, b_coin);

    #[cfg(feature = "docker-tests-watchers-eth")]
    if a_coin != "ETH" && b_coin != "ETH" {
        enable_coin(&mm_alice, "ETH");
    }

    let alice_acoin_balance_after = block_on(my_balance(&mm_alice, a_coin)).balance;
    let alice_bcoin_balance_after = block_on(my_balance(&mm_alice, b_coin)).balance;
    #[cfg(feature = "docker-tests-watchers-eth")]
    let alice_eth_balance_after = block_on(my_balance(&mm_alice, "ETH")).balance;
    if !matches!(swap_flow, SwapFlow::WatcherRefundsTakerPayment) {
        bob_acoin_balance_after = block_on(my_balance(&mm_bob, a_coin)).balance;
        bob_bcoin_balance_after = block_on(my_balance(&mm_bob, b_coin)).balance;
    }
    #[cfg(feature = "docker-tests-watchers-eth")]
    let watcher_acoin_balance_after = block_on(my_balance(&mm_watcher, a_coin)).balance;
    #[cfg(feature = "docker-tests-watchers-eth")]
    let watcher_bcoin_balance_after = block_on(my_balance(&mm_watcher, b_coin)).balance;

    BalanceResult {
        alice_acoin_balance_before,
        #[cfg(feature = "docker-tests-watchers-eth")]
        alice_acoin_balance_middle,
        alice_acoin_balance_after,
        alice_bcoin_balance_before,
        #[cfg(feature = "docker-tests-watchers-eth")]
        alice_bcoin_balance_middle,
        alice_bcoin_balance_after,
        #[cfg(feature = "docker-tests-watchers-eth")]
        alice_eth_balance_middle,
        #[cfg(feature = "docker-tests-watchers-eth")]
        alice_eth_balance_after,
        bob_acoin_balance_before,
        bob_acoin_balance_after,
        bob_bcoin_balance_before,
        bob_bcoin_balance_after,
        #[cfg(feature = "docker-tests-watchers-eth")]
        watcher_acoin_balance_before,
        #[cfg(feature = "docker-tests-watchers-eth")]
        watcher_acoin_balance_after,
        #[cfg(feature = "docker-tests-watchers-eth")]
        watcher_bcoin_balance_before,
        #[cfg(feature = "docker-tests-watchers-eth")]
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
