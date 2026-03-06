// ETH Inner Tests
//
// This module contains ETH-only tests that were extracted from docker_tests_inner.rs.
// These tests focus on ETH/ERC20 coin functionality including:
// - ETH/ERC20 activation and disable flows
// - Swap contract address negotiation
// - ETH/ERC20 withdraw and send operations
// - ETH/ERC20 orderbook and order management
// - ERC20 token approval
//
// Gated by: docker-tests-eth

use crate::docker_tests::helpers::env::random_secp256k1_secret;
use crate::docker_tests::helpers::eth::{
    erc20_coin_with_random_privkey, erc20_contract_checksum, fill_eth_erc20_with_private_key, swap_contract,
    swap_contract_checksum, GETH_RPC_URL, MM_CTX,
};
use crate::docker_tests::helpers::swap::trade_base_rel;
use crate::integration_tests_common::rmd160_from_passphrase;
use coins::{MarketCoinOps, TxFeeDetails};
use common::{block_on, get_utc_timestamp};
use crypto::{CryptoCtx, DerivationPath, KeyPairPolicy};
use http::StatusCode;
use mm2_number::BigDecimal;
use mm2_test_helpers::for_tests::{
    disable_coin, disable_coin_err, enable_eth_coin, erc20_dev_conf, eth_dev_conf, start_swaps,
    task_enable_eth_with_tokens, wait_for_swap_contract_negotiation, wait_for_swap_negotiation_failure, MarketMakerIt,
    Mm2TestConf, DEFAULT_RPC_PASSWORD,
};
use mm2_test_helpers::structs::*;
use serde_json::{json, Value as Json};
use std::collections::HashSet;
use std::iter::FromIterator;
use std::str::FromStr;
use std::thread;
use std::time::Duration;

// =============================================================================
// Test address constants
// =============================================================================

/// Arbitrary address used for swap contract negotiation tests (maker side)
const TEST_ARBITRARY_SWAP_ADDR_1: &str = "0x6c2858f6afac835c43ffda248aea167e1a58436c";
/// Arbitrary address used for swap contract negotiation tests (taker side)
const TEST_ARBITRARY_SWAP_ADDR_2: &str = "0x24abe4c71fc658c01313b6552cd40cd808b3ea80";
/// Valid checksummed ETH address used as withdraw destination in tests
const TEST_WITHDRAW_DEST_ADDR: &str = "0x4b2d0d6c2c785217457B69B922A2A9cEA98f71E9";
/// Invalid checksum variant of the withdraw destination (for checksum validation tests)
const TEST_WITHDRAW_DEST_ADDR_INVALID_CHECKSUM: &str = "0x4b2d0d6c2c785217457b69b922a2A9cEA98f71E9";

// =============================================================================
// ETH Activation Helper
// =============================================================================

async fn enable_eth_with_tokens(
    mm: &MarketMakerIt,
    platform_coin: &str,
    tokens: &[&str],
    swap_contract_address: &str,
    nodes: &[&str],
    balance: bool,
) -> Json {
    let erc20_tokens_requests: Vec<_> = tokens.iter().map(|ticker| json!({ "ticker": ticker })).collect();
    let nodes: Vec<_> = nodes.iter().map(|url| json!({ "url": url })).collect();

    let enable = mm
        .rpc(&json!({
        "userpass": mm.userpass,
        "method": "enable_eth_with_tokens",
        "mmrpc": "2.0",
        "params": {
                "ticker": platform_coin,
                "erc20_tokens_requests": erc20_tokens_requests,
                "swap_contract_address": swap_contract_address,
                "nodes": nodes,
                "tx_history": true,
                "get_balances": balance,
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        enable.0,
        StatusCode::OK,
        "'enable_eth_with_tokens' failed: {}",
        enable.1
    );
    serde_json::from_str(&enable.1).unwrap()
}

// =============================================================================
// ETH/ERC20 Activation and Disable Tests
// =============================================================================

#[test]
fn test_enable_eth_coin_with_token_then_disable() {
    let coin = erc20_coin_with_random_privkey(swap_contract());

    let priv_key = coin.display_priv_key().unwrap();
    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);

    let conf = Mm2TestConf::seednode(&priv_key, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_dump_log, _dump_dashboard) = mm.mm_dump();
    log!("log path: {}", mm.log_path.display());

    let swap_contract = swap_contract_checksum();
    block_on(enable_eth_with_tokens(
        &mm,
        "ETH",
        &["ERC20DEV"],
        &swap_contract,
        &[GETH_RPC_URL],
        true,
    ));

    // Create setprice order
    let req = json!({
        "userpass": mm.userpass,
        "method": "buy",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.1,
        "base_confs": 5,
        "base_nota": false,
        "rel_confs": 4,
        "rel_nota": false,
    });
    let make_test_order = block_on(mm.rpc(&req)).unwrap();
    assert_eq!(make_test_order.0, StatusCode::OK);
    let order_uuid = Json::from_str(&make_test_order.1).unwrap();
    let order_uuid = order_uuid.get("result").unwrap().get("uuid").unwrap().as_str().unwrap();

    // Passive ETH while having tokens enabled
    let res = block_on(disable_coin(&mm, "ETH", false));
    assert!(res.passivized);
    assert!(res.cancelled_orders.contains(order_uuid));

    // Try to disable ERC20DEV token.
    // This should work, because platform coin is still in the memory.
    let res = block_on(disable_coin(&mm, "ERC20DEV", false));
    // We expected make_test_order to be cancelled
    assert!(!res.passivized);

    // Because it's currently passive, default `disable_coin` should fail.
    block_on(disable_coin_err(&mm, "ETH", false));
    // And forced `disable_coin` should not fail
    let res = block_on(disable_coin(&mm, "ETH", true));
    assert!(!res.passivized);
}

#[test]
fn test_platform_coin_mismatch() {
    let coin = erc20_coin_with_random_privkey(swap_contract());

    let priv_key = coin.display_priv_key().unwrap();
    let mut erc20_conf = erc20_dev_conf(&erc20_contract_checksum());
    erc20_conf["protocol"]["protocol_data"]["platform"] = "MATIC".into(); // set a different platform coin
    let coins = json!([eth_dev_conf(), erc20_conf]);

    let conf = Mm2TestConf::seednode(&priv_key, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_dump_log, _dump_dashboard) = mm.mm_dump();
    log!("log path: {}", mm.log_path.display());

    let swap_contract = swap_contract_checksum();
    let erc20_tokens_requests = vec![json!({ "ticker": "ERC20DEV" })];
    let nodes = vec![json!({ "url": GETH_RPC_URL })];

    let enable = block_on(mm.rpc(&json!({
    "userpass": mm.userpass,
    "method": "enable_eth_with_tokens",
    "mmrpc": "2.0",
    "params": {
            "ticker": "ETH",
            "erc20_tokens_requests": erc20_tokens_requests,
            "swap_contract_address": swap_contract,
            "nodes": nodes,
            "tx_history": false,
            "get_balances": false,
        }
    })))
    .unwrap();
    assert_eq!(
        enable.0,
        StatusCode::BAD_REQUEST,
        "'enable_eth_with_tokens' must fail with PlatformCoinMismatch",
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&enable.1).unwrap()["error_type"]
            .as_str()
            .unwrap(),
        "PlatformCoinMismatch",
    );
}

#[test]
fn test_enable_eth_coin_with_token_without_balance() {
    let coin = erc20_coin_with_random_privkey(swap_contract());

    let priv_key = coin.display_priv_key().unwrap();
    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);

    let conf = Mm2TestConf::seednode(&priv_key, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_dump_log, _dump_dashboard) = mm.mm_dump();
    log!("log path: {}", mm.log_path.display());

    let swap_contract = swap_contract_checksum();
    let enable_eth_with_tokens = block_on(enable_eth_with_tokens(
        &mm,
        "ETH",
        &["ERC20DEV"],
        &swap_contract,
        &[GETH_RPC_URL],
        false,
    ));

    let enable_eth_with_tokens: RpcV2Response<IguanaEthWithTokensActivationResult> =
        serde_json::from_value(enable_eth_with_tokens).unwrap();

    let (_, eth_balance) = enable_eth_with_tokens
        .result
        .eth_addresses_infos
        .into_iter()
        .next()
        .unwrap();
    log!("{:?}", eth_balance);
    assert!(eth_balance.balances.is_none());
    assert!(eth_balance.tickers.is_none());

    let (_, erc20_balances) = enable_eth_with_tokens
        .result
        .erc20_addresses_infos
        .into_iter()
        .next()
        .unwrap();
    assert!(erc20_balances.balances.is_none());
    assert_eq!(
        erc20_balances.tickers.unwrap(),
        HashSet::from_iter(vec!["ERC20DEV".to_string()])
    );
}

// =============================================================================
// Swap Contract Negotiation Tests
// =============================================================================

#[test]
fn test_eth_swap_contract_addr_negotiation_same_fallback() {
    let bob_coin = erc20_coin_with_random_privkey(swap_contract());
    let alice_coin = erc20_coin_with_random_privkey(swap_contract());

    let bob_priv_key = bob_coin.display_priv_key().unwrap();
    let alice_priv_key = alice_coin.display_priv_key().unwrap();

    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);

    let bob_conf = Mm2TestConf::seednode(&bob_priv_key, &coins);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();

    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    let alice_conf = Mm2TestConf::light_node(&alice_priv_key, &coins, &[&mm_bob.ip.to_string()]);
    let mut mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    let swap_contract = swap_contract_checksum();

    dbg!(block_on(enable_eth_coin(
        &mm_bob,
        "ETH",
        &[GETH_RPC_URL],
        // using arbitrary address
        TEST_ARBITRARY_SWAP_ADDR_1,
        Some(&swap_contract),
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_bob,
        "ERC20DEV",
        &[GETH_RPC_URL],
        // using arbitrary address
        TEST_ARBITRARY_SWAP_ADDR_1,
        Some(&swap_contract),
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_alice,
        "ETH",
        &[GETH_RPC_URL],
        // using arbitrary address
        TEST_ARBITRARY_SWAP_ADDR_2,
        Some(&swap_contract),
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_alice,
        "ERC20DEV",
        &[GETH_RPC_URL],
        // using arbitrary address
        TEST_ARBITRARY_SWAP_ADDR_2,
        Some(&swap_contract),
        false
    )));

    let uuids = block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[("ETH", "ERC20DEV")],
        1.,
        1.,
        0.0001,
    ));

    // give few seconds for swap statuses to be saved
    thread::sleep(Duration::from_secs(3));

    let wait_until = get_utc_timestamp() + 30;
    // Expected contract should be lowercase since swap status stores addresses in lowercase format
    let expected_contract = Json::from(swap_contract.trim_start_matches("0x").to_lowercase());

    block_on(wait_for_swap_contract_negotiation(
        &mm_bob,
        &uuids[0],
        expected_contract.clone(),
        wait_until,
    ));
    block_on(wait_for_swap_contract_negotiation(
        &mm_alice,
        &uuids[0],
        expected_contract,
        wait_until,
    ));
}

#[test]
fn test_eth_swap_negotiation_fails_maker_no_fallback() {
    let bob_coin = erc20_coin_with_random_privkey(swap_contract());
    let alice_coin = erc20_coin_with_random_privkey(swap_contract());

    let bob_priv_key = bob_coin.display_priv_key().unwrap();
    let alice_priv_key = alice_coin.display_priv_key().unwrap();

    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);

    let bob_conf = Mm2TestConf::seednode(&bob_priv_key, &coins);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();

    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    let alice_conf = Mm2TestConf::light_node(&alice_priv_key, &coins, &[&mm_bob.ip.to_string()]);
    let mut mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    let swap_contract = swap_contract_checksum();

    dbg!(block_on(enable_eth_coin(
        &mm_bob,
        "ETH",
        &[GETH_RPC_URL],
        // using arbitrary address
        TEST_ARBITRARY_SWAP_ADDR_1,
        None,
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_bob,
        "ERC20DEV",
        &[GETH_RPC_URL],
        // using arbitrary address
        TEST_ARBITRARY_SWAP_ADDR_1,
        None,
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_alice,
        "ETH",
        &[GETH_RPC_URL],
        // using arbitrary address
        TEST_ARBITRARY_SWAP_ADDR_2,
        Some(&swap_contract),
        false
    )));

    dbg!(block_on(enable_eth_coin(
        &mm_alice,
        "ERC20DEV",
        &[GETH_RPC_URL],
        // using arbitrary address
        TEST_ARBITRARY_SWAP_ADDR_2,
        Some(&swap_contract),
        false
    )));

    let uuids = block_on(start_swaps(
        &mut mm_bob,
        &mut mm_alice,
        &[("ETH", "ERC20DEV")],
        1.,
        1.,
        0.0001,
    ));

    // give few seconds for swap statuses to be saved
    thread::sleep(Duration::from_secs(3));

    let wait_until = get_utc_timestamp() + 30;
    block_on(wait_for_swap_negotiation_failure(&mm_bob, &uuids[0], wait_until));
    block_on(wait_for_swap_negotiation_failure(&mm_alice, &uuids[0], wait_until));
}

// =============================================================================
// ETH/ERC20 Swap Tests
// =============================================================================

#[test]
fn test_trade_base_rel_eth_erc20_coins() {
    trade_base_rel(("ETH", "ERC20DEV"));
}

// =============================================================================
// ETH/ERC20 Withdraw and Send Tests
// =============================================================================

fn withdraw_and_send(
    mm: &MarketMakerIt,
    coin: &str,
    from: Option<HDAccountAddressId>,
    to: &str,
    from_addr: &str,
    expected_bal_change: &str,
    amount: f64,
) {
    let withdraw = block_on(mm.rpc(&json! ({
        "mmrpc": "2.0",
        "userpass": mm.userpass,
        "method": "withdraw",
        "params": {
            "coin": coin,
            "from": from,
            "to": to,
            "amount": amount,
        },
        "id": 0,
    })))
    .unwrap();

    assert!(withdraw.0.is_success(), "!withdraw: {}", withdraw.1);
    let res: RpcSuccessResponse<TransactionDetails> =
        serde_json::from_str(&withdraw.1).expect("Expected 'RpcSuccessResponse<TransactionDetails>'");
    let tx_details = res.result;

    let mut expected_bal_change = BigDecimal::from_str(expected_bal_change).expect("!BigDecimal::from_str");

    let fee_details: TxFeeDetails = serde_json::from_value(tx_details.fee_details).unwrap();

    if let TxFeeDetails::Eth(fee_details) = fee_details {
        if coin == "ETH" {
            expected_bal_change -= fee_details.total_fee;
        }
    }

    assert_eq!(tx_details.to, vec![to.to_owned()]);
    assert_eq!(tx_details.my_balance_change, expected_bal_change);
    // Todo: Should check the from address for withdraws from another HD wallet address when there is an RPC method for addresses
    if from.is_none() {
        assert_eq!(tx_details.from, vec![from_addr.to_owned()]);
    }

    let send = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "send_raw_transaction",
        "coin": coin,
        "tx_hex": tx_details.tx_hex,
    })))
    .unwrap();
    assert!(send.0.is_success(), "!{} send: {}", coin, send.1);
    let send_json: Json = serde_json::from_str(&send.1).unwrap();
    assert_eq!(tx_details.tx_hash, send_json["tx_hash"]);
}

#[test]
fn test_withdraw_and_send_eth_erc20() {
    let privkey = random_secp256k1_secret();
    fill_eth_erc20_with_private_key(privkey);

    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);
    let mm = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
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

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("Alice log path: {}", mm.log_path.display());

    let swap_contract = swap_contract_checksum();
    let eth_enable = block_on(enable_eth_coin(
        &mm,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false,
    ));
    let erc20_enable = block_on(enable_eth_coin(
        &mm,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false,
    ));

    withdraw_and_send(
        &mm,
        "ETH",
        None,
        TEST_WITHDRAW_DEST_ADDR,
        eth_enable["address"].as_str().unwrap(),
        "-0.001",
        0.001,
    );

    withdraw_and_send(
        &mm,
        "ERC20DEV",
        None,
        TEST_WITHDRAW_DEST_ADDR,
        erc20_enable["address"].as_str().unwrap(),
        "-0.001",
        0.001,
    );

    // must not allow to withdraw to invalid checksum address
    let withdraw = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "mmrpc": "2.0",
        "method": "withdraw",
        "params": {
            "coin": "ETH",
            "to": TEST_WITHDRAW_DEST_ADDR_INVALID_CHECKSUM,
            "amount": "0.001",
        },
        "id": 0,
    })))
    .unwrap();

    assert!(withdraw.0.is_client_error(), "ETH withdraw: {}", withdraw.1);
    let res: RpcErrorResponse<String> = serde_json::from_str(&withdraw.1).unwrap();
    assert_eq!(res.error_type, "InvalidAddress");
    assert!(res.error.contains("Invalid address checksum"));
}

#[test]
fn test_withdraw_and_send_hd_eth_erc20() {
    const PASSPHRASE: &str = "tank abandon bind salon remove wisdom net size aspect direct source fossil";

    let KeyPairPolicy::GlobalHDAccount(hd_acc) = CryptoCtx::init_with_global_hd_account(MM_CTX.clone(), PASSPHRASE)
        .unwrap()
        .key_pair_policy()
        .clone()
    else {
        panic!("Expected 'KeyPairPolicy::GlobalHDAccount'");
    };

    let swap_contract = swap_contract_checksum();

    // Withdraw from HD account 0, change address 0, index 1
    let mut path_to_address = HDAccountAddressId {
        account_id: 0,
        chain: Bip44Chain::External,
        address_id: 1,
    };
    let path_to_addr_str = "/0'/0/1";
    let path_to_coin: String = serde_json::from_value(eth_dev_conf()["derivation_path"].clone()).unwrap();
    let derivation_path = path_to_coin.clone() + path_to_addr_str;
    let derivation_path = DerivationPath::from_str(&derivation_path).unwrap();
    // Get the private key associated with this account and fill it with eth and erc20 token.
    let priv_key = hd_acc.derive_secp256k1_secret(&derivation_path).unwrap();
    fill_eth_erc20_with_private_key(priv_key);

    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);

    let conf = Mm2TestConf::seednode_with_hd_account(PASSPHRASE, &coins);
    let mm_hd = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm_hd.mm_dump();
    log!("Alice log path: {}", mm_hd.log_path.display());

    let eth_enable = block_on(task_enable_eth_with_tokens(
        &mm_hd,
        "ETH",
        &["ERC20DEV"],
        Some(&swap_contract),
        &[GETH_RPC_URL],
        60,
        Some(path_to_address.clone()),
    ));
    let activation_result = match eth_enable {
        EthWithTokensActivationResult::HD(hd) => hd,
        _ => panic!("Expected EthWithTokensActivationResult::HD"),
    };
    let balance = match activation_result.wallet_balance {
        EnableCoinBalanceMap::HD(hd) => hd,
        _ => panic!("Expected EnableCoinBalance::HD"),
    };
    let account = balance.accounts.first().expect("Expected account at index 0");
    assert_eq!(
        account.addresses[1].address,
        "0xDe841899aB4A22E23dB21634e54920aDec402397"
    );
    assert_eq!(account.addresses[1].balance.len(), 2);
    assert_eq!(account.addresses[1].balance.get("ETH").unwrap().spendable, 100.into());
    assert_eq!(
        account.addresses[1].balance.get("ERC20DEV").unwrap().spendable,
        100.into()
    );

    withdraw_and_send(
        &mm_hd,
        "ETH",
        Some(path_to_address.clone()),
        TEST_WITHDRAW_DEST_ADDR,
        &account.addresses[1].address,
        "-0.001",
        0.001,
    );

    withdraw_and_send(
        &mm_hd,
        "ERC20DEV",
        Some(path_to_address.clone()),
        TEST_WITHDRAW_DEST_ADDR,
        &account.addresses[1].address,
        "-0.001",
        0.001,
    );

    // Change the address index, the withdrawal should fail.
    path_to_address.address_id = 0;

    let withdraw = block_on(mm_hd.rpc(&json! ({
        "mmrpc": "2.0",
        "userpass": mm_hd.userpass,
        "method": "withdraw",
        "params": {
            "coin": "ETH",
            "from": path_to_address,
            "to": TEST_WITHDRAW_DEST_ADDR,
            "amount": 0.001,
        },
        "id": 0,
    })))
    .unwrap();
    assert!(!withdraw.0.is_success(), "!withdraw: {}", withdraw.1);

    // But if we fill it, we should be able to withdraw.
    let path_to_addr_str = "/0'/0/0";
    let derivation_path = path_to_coin + path_to_addr_str;
    let derivation_path = DerivationPath::from_str(&derivation_path).unwrap();
    let priv_key = hd_acc.derive_secp256k1_secret(&derivation_path).unwrap();
    fill_eth_erc20_with_private_key(priv_key);

    let withdraw = block_on(mm_hd.rpc(&json! ({
        "mmrpc": "2.0",
        "userpass": mm_hd.userpass,
        "method": "withdraw",
        "params": {
            "coin": "ETH",
            "from": path_to_address,
            "to": TEST_WITHDRAW_DEST_ADDR,
            "amount": 0.001,
        },
        "id": 0,
    })))
    .unwrap();
    assert!(withdraw.0.is_success(), "!withdraw: {}", withdraw.1);

    block_on(mm_hd.stop()).unwrap();
}

// =============================================================================
// ETH/ERC20 Order DB Persistence and Conf Settings Tests
// =============================================================================

#[test]
fn test_set_price_must_save_order_to_db() {
    let private_key_str = erc20_coin_with_random_privkey(swap_contract())
        .display_priv_key()
        .unwrap();
    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);

    let conf = Mm2TestConf::seednode(&private_key_str, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    // Enable coins
    let swap_contract = swap_contract_checksum();
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    log!("Issue bob ETH/ERC20DEV sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.1
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    let rc_json: Json = serde_json::from_str(&rc.1).unwrap();
    let uuid: String = serde_json::from_value(rc_json["result"]["uuid"].clone()).unwrap();
    let order_path = mm.folder.join(format!(
        "DB/{}/ORDERS/MY/MAKER/{}.json",
        hex::encode(rmd160_from_passphrase(&private_key_str)),
        uuid
    ));
    assert!(order_path.exists());
}

#[test]
fn test_set_price_conf_settings() {
    let private_key_str = erc20_coin_with_random_privkey(swap_contract())
        .display_priv_key()
        .unwrap();

    let coins = json!([eth_dev_conf(),{"coin":"ERC20DEV","name":"erc20dev","protocol":{"type":"ERC20","protocol_data":{"platform":"ETH","contract_address":erc20_contract_checksum()}},"required_confirmations":2},]);

    let conf = Mm2TestConf::seednode(&private_key_str, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    // Enable coins
    let swap_contract = swap_contract_checksum();
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    log!("Issue bob sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.1,
        "base_confs": 5,
        "base_nota": true,
        "rel_confs": 4,
        "rel_nota": false,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(json["result"]["conf_settings"]["base_confs"], Json::from(5));
    assert_eq!(json["result"]["conf_settings"]["base_nota"], Json::from(true));
    assert_eq!(json["result"]["conf_settings"]["rel_confs"], Json::from(4));
    assert_eq!(json["result"]["conf_settings"]["rel_nota"], Json::from(false));

    // must use coin config as defaults if not set in request
    log!("Issue bob sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "setprice",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.1,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(json["result"]["conf_settings"]["base_confs"], Json::from(1));
    assert_eq!(json["result"]["conf_settings"]["base_nota"], Json::from(false));
    assert_eq!(json["result"]["conf_settings"]["rel_confs"], Json::from(2));
    assert_eq!(json["result"]["conf_settings"]["rel_nota"], Json::from(false));
}

#[test]
fn test_buy_conf_settings() {
    let private_key_str = erc20_coin_with_random_privkey(swap_contract())
        .display_priv_key()
        .unwrap();

    let coins = json!([eth_dev_conf(),{"coin":"ERC20DEV","name":"erc20dev","protocol":{"type":"ERC20","protocol_data":{"platform":"ETH","contract_address":erc20_contract_checksum()}},"required_confirmations":2},]);

    let conf = Mm2TestConf::seednode(&private_key_str, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    // Enable coins
    let swap_contract = swap_contract_checksum();
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    log!("Issue bob buy request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "buy",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.1,
        "base_confs": 5,
        "base_nota": true,
        "rel_confs": 4,
        "rel_nota": false,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(json["result"]["conf_settings"]["base_confs"], Json::from(5));
    assert_eq!(json["result"]["conf_settings"]["base_nota"], Json::from(true));
    assert_eq!(json["result"]["conf_settings"]["rel_confs"], Json::from(4));
    assert_eq!(json["result"]["conf_settings"]["rel_nota"], Json::from(false));

    // must use coin config as defaults if not set in request
    log!("Issue bob buy request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "buy",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.1,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(json["result"]["conf_settings"]["base_confs"], Json::from(1));
    assert_eq!(json["result"]["conf_settings"]["base_nota"], Json::from(false));
    assert_eq!(json["result"]["conf_settings"]["rel_confs"], Json::from(2));
    assert_eq!(json["result"]["conf_settings"]["rel_nota"], Json::from(false));
}

#[test]
fn test_sell_conf_settings() {
    let private_key_str = erc20_coin_with_random_privkey(swap_contract())
        .display_priv_key()
        .unwrap();

    let coins = json!([eth_dev_conf(),{"coin":"ERC20DEV","name":"erc20dev","protocol":{"type":"ERC20","protocol_data":{"platform":"ETH","contract_address":erc20_contract_checksum()}},"required_confirmations":2},]);

    let conf = Mm2TestConf::seednode(&private_key_str, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("MM log path: {}", mm.log_path.display());

    // Enable coins
    let swap_contract = swap_contract_checksum();
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));
    dbg!(block_on(enable_eth_coin(
        &mm,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    log!("Issue bob sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "sell",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.1,
        "base_confs": 5,
        "base_nota": true,
        "rel_confs": 4,
        "rel_nota": false,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(json["result"]["conf_settings"]["base_confs"], Json::from(5));
    assert_eq!(json["result"]["conf_settings"]["base_nota"], Json::from(true));
    assert_eq!(json["result"]["conf_settings"]["rel_confs"], Json::from(4));
    assert_eq!(json["result"]["conf_settings"]["rel_nota"], Json::from(false));

    // must use coin config as defaults if not set in request
    log!("Issue bob sell request");
    let rc = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "sell",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.1,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!sell: {}", rc.1);
    let json: Json = serde_json::from_str(&rc.1).unwrap();
    assert_eq!(json["result"]["conf_settings"]["base_confs"], Json::from(1));
    assert_eq!(json["result"]["conf_settings"]["base_nota"], Json::from(false));
    assert_eq!(json["result"]["conf_settings"]["rel_confs"], Json::from(2));
    assert_eq!(json["result"]["conf_settings"]["rel_nota"], Json::from(false));
}

// =============================================================================
// ETH/ERC20 Order Matching and my_orders Tests
// =============================================================================

#[test]
fn test_my_orders_after_matched() {
    let bob_coin = erc20_coin_with_random_privkey(swap_contract());
    let alice_coin = erc20_coin_with_random_privkey(swap_contract());

    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);

    let bob_conf = Mm2TestConf::seednode(&bob_coin.display_priv_key().unwrap(), &coins);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();

    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    let alice_conf = Mm2TestConf::light_node(
        &alice_coin.display_priv_key().unwrap(),
        &coins,
        &[&mm_bob.ip.to_string()],
    );
    let mut mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

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
        &mm_bob,
        "ERC20DEV",
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

    dbg!(block_on(enable_eth_coin(
        &mm_alice,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    let rc = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.000001,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.000001,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    block_on(mm_bob.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop ETH/ERC20DEV"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop ETH/ERC20DEV"))).unwrap();

    log!("Issue bob my_orders request");
    let rc = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "my_orders",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!my_orders: {}", rc.1);

    let _: MyOrdersRpcResult = serde_json::from_str(&rc.1).unwrap();
    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

#[test]
fn test_update_maker_order_after_matched() {
    let bob_coin = erc20_coin_with_random_privkey(swap_contract());
    let alice_coin = erc20_coin_with_random_privkey(swap_contract());

    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);

    let bob_conf = Mm2TestConf::seednode(&bob_coin.display_priv_key().unwrap(), &coins);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();

    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    let alice_conf = Mm2TestConf::light_node(
        &alice_coin.display_priv_key().unwrap(),
        &coins,
        &[&mm_bob.ip.to_string()],
    );
    let mut mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

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
        &mm_bob,
        "ERC20DEV",
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

    dbg!(block_on(enable_eth_coin(
        &mm_alice,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false
    )));

    let rc = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.00002,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);
    let setprice_json: Json = serde_json::from_str(&rc.1).unwrap();
    let uuid: String = serde_json::from_value(setprice_json["result"]["uuid"].clone()).unwrap();

    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": "ETH",
        "rel": "ERC20DEV",
        "price": 1,
        "volume": 0.00001,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    block_on(mm_bob.wait_for_log(22., |log| log.contains("Entering the maker_swap_loop ETH/ERC20DEV"))).unwrap();
    block_on(mm_alice.wait_for_log(22., |log| log.contains("Entering the taker_swap_loop ETH/ERC20DEV"))).unwrap();

    log!("Issue bob update maker order request that should fail because new volume is less than reserved amount");
    let update_maker_order = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "update_maker_order",
        "uuid": uuid,
        "volume_delta": -0.00002,
    })))
    .unwrap();
    assert!(
        !update_maker_order.0.is_success(),
        "update_maker_order success, but should be error {}",
        update_maker_order.1
    );

    log!("Issue another bob update maker order request");
    let update_maker_order = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "update_maker_order",
        "uuid": uuid,
        "volume_delta": 0.00001,
    })))
    .unwrap();
    assert!(
        update_maker_order.0.is_success(),
        "!update_maker_order: {}",
        update_maker_order.1
    );
    let update_maker_order_json: Json = serde_json::from_str(&update_maker_order.1).unwrap();
    log!("{}", update_maker_order.1);
    assert_eq!(update_maker_order_json["result"]["max_base_vol"], Json::from("0.00003"));

    log!("Issue bob my_orders request");
    let rc = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "my_orders",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!my_orders: {}", rc.1);

    let _: MyOrdersRpcResult = serde_json::from_str(&rc.1).unwrap();
    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

// =============================================================================
// ERC20 Token Approval Tests
// =============================================================================

#[test]
fn test_approve_erc20() {
    let privkey = random_secp256k1_secret();
    fill_eth_erc20_with_private_key(privkey);

    let coins = json!([eth_dev_conf(), erc20_dev_conf(&erc20_contract_checksum())]);
    let mm = MarketMakerIt::start(
        Mm2TestConf::seednode(&format!("0x{}", hex::encode(privkey)), &coins).conf,
        DEFAULT_RPC_PASSWORD.to_string(),
        None,
    )
    .unwrap();

    let (_mm_dump_log, _mm_dump_dashboard) = mm.mm_dump();
    log!("Node log path: {}", mm.log_path.display());

    let swap_contract = swap_contract_checksum();
    let _eth_enable = block_on(enable_eth_coin(
        &mm,
        "ETH",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false,
    ));
    let _erc20_enable = block_on(enable_eth_coin(
        &mm,
        "ERC20DEV",
        &[GETH_RPC_URL],
        &swap_contract,
        None,
        false,
    ));

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method":"approve_token",
        "mmrpc":"2.0",
        "id": 0,
        "params":{
          "coin": "ERC20DEV",
          "spender": swap_contract,
          "amount": BigDecimal::from_str("11.0").unwrap(),
        }
    })))
    .unwrap();
    assert!(rc.0.is_success(), "approve_token error: {}", rc.1);
    let res = serde_json::from_str::<Json>(&rc.1).unwrap();
    assert!(
        hex::decode(str_strip_0x!(res["result"].as_str().unwrap())).is_ok(),
        "approve_token result incorrect"
    );
    thread::sleep(Duration::from_secs(5));
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method":"get_token_allowance",
        "mmrpc":"2.0",
        "id": 0,
        "params":{
          "coin": "ERC20DEV",
          "spender": swap_contract,
        }
    })))
    .unwrap();
    assert!(rc.0.is_success(), "get_token_allowance error: {}", rc.1);
    let res = serde_json::from_str::<Json>(&rc.1).unwrap();
    assert_eq!(
        BigDecimal::from_str(res["result"].as_str().unwrap()).unwrap(),
        BigDecimal::from_str("11.0").unwrap(),
        "get_token_allowance result incorrect"
    );

    block_on(mm.stop()).unwrap();
}
