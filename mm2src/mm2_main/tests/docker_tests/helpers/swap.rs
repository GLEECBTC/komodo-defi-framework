//! Swap orchestration helpers for docker tests.
//!
//! This module provides high-level cross-chain atomic swap test scenarios.
//! For chain-specific helpers, import directly from the other `helpers` submodules.
//!
//! ## Feature gating
//!
//! This module is compiled for all docker tests (gated by `run-docker-tests`), but
//! chain-specific imports and code blocks are gated by their respective feature flags:
//!
//! - ETH: `docker-tests-eth`, `docker-tests-ordermatch`
//! - QRC20: `docker-tests-qrc20`
//! - UTXO: `docker-tests-swaps`, `docker-tests-ordermatch`, `docker-tests-watchers`, `docker-tests-qrc20`, `docker-tests-sia`, `docker-tests-slp`
//! - SLP: `docker-tests-slp`

use common::block_on;
use crypto::privkey::key_pair_from_secret;
use mm2_test_helpers::for_tests::{
    check_my_swap_status, check_recent_swaps, mm_dump, wait_check_stats_swap_status, MarketMakerIt,
};
use serde_json::{json, Value as Json};
use std::thread;
use std::time::Duration;

use crypto::Secp256k1Secret;

// random_secp256k1_secret - used by non-SLP swap paths
#[cfg(any(
    feature = "docker-tests-swaps",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-eth",
    feature = "docker-tests-sia"
))]
use super::env::random_secp256k1_secret;

// SET_BURN_PUBKEY_TO_ALICE - used by trade_base_rel
#[cfg(any(
    feature = "docker-tests-swaps",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-slp",
    feature = "docker-tests-eth",
    feature = "docker-tests-integration"
))]
use super::env::SET_BURN_PUBKEY_TO_ALICE;

/// Timeout in seconds for wallet funding operations during test setup.
#[cfg(any(
    feature = "docker-tests-qrc20",
    feature = "docker-tests-swaps",
    feature = "docker-tests-sia"
))]
const WALLET_FUNDING_TIMEOUT_SEC: u64 = 30;

// ETH imports
#[cfg(feature = "docker-tests-eth")]
use super::eth::{erc20_contract_checksum, fill_eth_erc20_with_private_key, swap_contract_checksum, GETH_RPC_URL};
#[cfg(feature = "docker-tests-eth")]
use mm2_test_helpers::for_tests::{enable_eth_coin, erc20_dev_conf, eth_dev_conf};

// QRC20 imports
#[cfg(feature = "docker-tests-qrc20")]
use super::qrc20::{
    enable_qrc20_native, fill_qrc20_address, generate_segwit_qtum_coin_with_random_privkey, qrc20_coin_conf_item,
    qrc20_coin_from_privkey, qtum_conf_path, wait_for_estimate_smart_fee,
};
#[cfg(feature = "docker-tests-qrc20")]
use super::utxo::fill_address as fill_utxo_address_qrc20;
#[cfg(feature = "docker-tests-qrc20")]
use coins::MarketCoinOps;
#[cfg(feature = "docker-tests-qrc20")]
use mm2_test_helpers::for_tests::enable_native as enable_native_qrc20;

// UTXO imports (non-QRC20 paths)
#[cfg(all(
    any(feature = "docker-tests-swaps", feature = "docker-tests-sia"),
    not(feature = "docker-tests-qrc20")
))]
use super::utxo::{fill_address, utxo_coin_from_privkey};
#[cfg(all(
    any(feature = "docker-tests-swaps", feature = "docker-tests-sia"),
    not(feature = "docker-tests-qrc20")
))]
use coins::MarketCoinOps;
#[cfg(all(
    any(feature = "docker-tests-swaps", feature = "docker-tests-sia"),
    not(feature = "docker-tests-qrc20")
))]
use mm2_test_helpers::for_tests::enable_native;

// UTXO imports (QRC20 path - already imports fill_address as fill_utxo_address_qrc20)
#[cfg(feature = "docker-tests-qrc20")]
use super::utxo::utxo_coin_from_privkey as utxo_coin_from_privkey_qrc20;

// SLP imports
#[cfg(feature = "docker-tests-slp")]
use super::slp::{get_prefilled_slp_privkey, get_slp_token_id};
#[cfg(feature = "docker-tests-slp")]
use mm2_test_helpers::for_tests::{enable_native as enable_native_slp, enable_native_bch};

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
///
/// ## Feature requirements
///
/// Different coin pairs require different feature flags:
/// - ETH/ERC20DEV: `docker-tests-eth` or `docker-tests-ordermatch`
/// - QTUM/QICK/QORTY: `docker-tests-qrc20`
/// - MYCOIN/MYCOIN1: `docker-tests-swaps`, `docker-tests-ordermatch`, `docker-tests-watchers`, `docker-tests-qrc20`, `docker-tests-sia`
/// - FORSLP/ADEXSLP: `docker-tests-slp`
pub fn trade_base_rel((base, rel): (&str, &str)) {
    /// Generate a wallet with the random private key and fill the wallet with funds.
    fn generate_and_fill_priv_key(ticker: &str) -> Secp256k1Secret {
        match ticker {
            #[cfg(feature = "docker-tests-qrc20")]
            "QTUM" => {
                wait_for_estimate_smart_fee(WALLET_FUNDING_TIMEOUT_SEC).expect("!wait_for_estimate_smart_fee");
                let (_ctx, _coin, priv_key) = generate_segwit_qtum_coin_with_random_privkey("QTUM", 10.into(), Some(0));
                priv_key
            },
            #[cfg(feature = "docker-tests-qrc20")]
            "QICK" | "QORTY" => {
                let priv_key = random_secp256k1_secret();
                let (_ctx, coin) = qrc20_coin_from_privkey(ticker, priv_key);
                let my_address = coin.my_address().expect("!my_address");
                fill_utxo_address_qrc20(&coin, &my_address, 10.into(), WALLET_FUNDING_TIMEOUT_SEC);
                fill_qrc20_address(&coin, 10.into(), WALLET_FUNDING_TIMEOUT_SEC);
                priv_key
            },
            #[cfg(feature = "docker-tests-qrc20")]
            "MYCOIN" | "MYCOIN1" => {
                let priv_key = random_secp256k1_secret();
                let (_ctx, coin) = utxo_coin_from_privkey_qrc20(ticker, priv_key);
                let my_address = coin.my_address().expect("!my_address");
                fill_utxo_address_qrc20(&coin, &my_address, 10.into(), WALLET_FUNDING_TIMEOUT_SEC);
                priv_key
            },
            #[cfg(all(
                any(
                    feature = "docker-tests-swaps",
                    feature = "docker-tests-ordermatch",
                    feature = "docker-tests-sia"
                ),
                not(feature = "docker-tests-qrc20")
            ))]
            "MYCOIN" | "MYCOIN1" => {
                let priv_key = random_secp256k1_secret();
                let (_ctx, coin) = utxo_coin_from_privkey(ticker, priv_key);
                let my_address = coin.my_address().expect("!my_address");
                fill_address(&coin, &my_address, 10.into(), WALLET_FUNDING_TIMEOUT_SEC);
                priv_key
            },
            #[cfg(feature = "docker-tests-slp")]
            "ADEXSLP" | "FORSLP" => Secp256k1Secret::from(get_prefilled_slp_privkey()),
            #[cfg(feature = "docker-tests-eth")]
            "ETH" | "ERC20DEV" => {
                let priv_key = random_secp256k1_secret();
                fill_eth_erc20_with_private_key(priv_key);
                priv_key
            },
            _ => panic!(
                "Unsupported ticker: {}. Check that the required feature flag is enabled. \
                 ETH/ERC20DEV: docker-tests-eth or docker-tests-ordermatch. \
                 QTUM/QICK/QORTY/MYCOIN: docker-tests-qrc20. \
                 MYCOIN/MYCOIN1: docker-tests-swaps, docker-tests-ordermatch, docker-tests-watchers, docker-tests-sia. \
                 FORSLP/ADEXSLP: docker-tests-slp.",
                ticker
            ),
        }
    }

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

    // Build coins config based on enabled features
    let mut coins_vec: Vec<Json> = Vec::new();

    #[cfg(feature = "docker-tests-eth")]
    {
        coins_vec.push(eth_dev_conf());
        coins_vec.push(erc20_dev_conf(&erc20_contract_checksum()));
    }

    #[cfg(feature = "docker-tests-qrc20")]
    {
        let confpath = qtum_conf_path();
        coins_vec.push(qrc20_coin_conf_item("QICK"));
        coins_vec.push(qrc20_coin_conf_item("QORTY"));
        coins_vec.push(json!({
            "coin": "QTUM", "asset": "QTUM", "required_confirmations": 0, "decimals": 8,
            "pubtype": 120, "p2shtype": 110, "wiftype": 128, "segwit": true, "txfee": 0,
            "txfee_volatility_percent": 0.1, "dust": 72800, "mm2": 1, "network": "regtest",
            "confpath": confpath, "protocol": {"type": "UTXO"}, "bech32_hrp": "qcrt",
            "address_format": {"format": "segwit"}
        }));
        coins_vec.push(json!({
            "coin": "MYCOIN", "asset": "MYCOIN", "required_confirmations": 0,
            "txversion": 4, "overwintered": 1, "txfee": 1000, "protocol": {"type": "UTXO"}
        }));
        coins_vec.push(json!({
            "coin": "MYCOIN1", "asset": "MYCOIN1", "required_confirmations": 0,
            "txversion": 4, "overwintered": 1, "txfee": 1000, "protocol": {"type": "UTXO"}
        }));
    }

    #[cfg(all(
        any(
            feature = "docker-tests-swaps",
            feature = "docker-tests-ordermatch",
            feature = "docker-tests-sia"
        ),
        not(feature = "docker-tests-qrc20")
    ))]
    {
        coins_vec.push(json!({
            "coin": "MYCOIN", "asset": "MYCOIN", "required_confirmations": 0,
            "txversion": 4, "overwintered": 1, "txfee": 1000, "protocol": {"type": "UTXO"}
        }));
        coins_vec.push(json!({
            "coin": "MYCOIN1", "asset": "MYCOIN1", "required_confirmations": 0,
            "txversion": 4, "overwintered": 1, "txfee": 1000, "protocol": {"type": "UTXO"}
        }));
    }

    #[cfg(feature = "docker-tests-slp")]
    {
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
            "dht": "on",
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
            "dht": "on",
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

    // Enable coins for Bob based on enabled features
    #[cfg(feature = "docker-tests-qrc20")]
    {
        log!("{:?}", block_on(enable_qrc20_native(&mm_bob, "QICK")));
        log!("{:?}", block_on(enable_qrc20_native(&mm_bob, "QORTY")));
        log!("{:?}", block_on(enable_native_qrc20(&mm_bob, "QTUM", &[], None)));
        log!("{:?}", block_on(enable_native_qrc20(&mm_bob, "MYCOIN", &[], None)));
        log!("{:?}", block_on(enable_native_qrc20(&mm_bob, "MYCOIN1", &[], None)));
    }

    #[cfg(all(
        any(
            feature = "docker-tests-swaps",
            feature = "docker-tests-ordermatch",
            feature = "docker-tests-sia"
        ),
        not(feature = "docker-tests-qrc20")
    ))]
    {
        log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
        log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    }

    #[cfg(feature = "docker-tests-slp")]
    {
        log!("{:?}", block_on(enable_native_bch(&mm_bob, "FORSLP", &[])));
        log!("{:?}", block_on(enable_native_slp(&mm_bob, "ADEXSLP", &[], None)));
    }

    #[cfg(feature = "docker-tests-eth")]
    {
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

    // Enable coins for Alice based on enabled features
    #[cfg(feature = "docker-tests-qrc20")]
    {
        log!("{:?}", block_on(enable_qrc20_native(&mm_alice, "QICK")));
        log!("{:?}", block_on(enable_qrc20_native(&mm_alice, "QORTY")));
        log!("{:?}", block_on(enable_native_qrc20(&mm_alice, "QTUM", &[], None)));
        log!("{:?}", block_on(enable_native_qrc20(&mm_alice, "MYCOIN", &[], None)));
        log!("{:?}", block_on(enable_native_qrc20(&mm_alice, "MYCOIN1", &[], None)));
    }

    #[cfg(all(
        any(
            feature = "docker-tests-swaps",
            feature = "docker-tests-ordermatch",
            feature = "docker-tests-sia"
        ),
        not(feature = "docker-tests-qrc20")
    ))]
    {
        log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
        log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));
    }

    #[cfg(feature = "docker-tests-slp")]
    {
        log!("{:?}", block_on(enable_native_bch(&mm_alice, "FORSLP", &[])));
        log!("{:?}", block_on(enable_native_slp(&mm_alice, "ADEXSLP", &[], None)));
    }

    #[cfg(feature = "docker-tests-eth")]
    {
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
