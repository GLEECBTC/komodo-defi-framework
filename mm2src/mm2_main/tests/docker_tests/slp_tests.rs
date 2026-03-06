use crate::docker_tests::helpers::slp::{get_prefilled_slp_privkey, get_slp_token_id};
use crate::integration_tests_common::enable_native;
use bitcrypto::ChecksumType;
use coins::utxo::UtxoAddressFormat;
use common::block_on;
use http::StatusCode;
use keys::{Address, AddressBuilder, AddressHashEnum, AddressPrefix, NetworkAddressPrefixes};
use mm2_number::BigDecimal;
use mm2_rpc::data::legacy::{BalanceResponse, CoinInitResponse};
use mm2_test_helpers::for_tests::{
    assert_coin_not_found_on_balance, disable_coin, enable_bch_with_tokens, enable_native_bch, enable_slp, my_balance,
    MarketMakerIt, UtxoRpcMode,
};
use mm2_test_helpers::structs::{EnableBchWithTokensResponse, EnableSlpResponse, RpcV2Response, TransactionDetails};
use serde_json::{self as json, json, Value as Json};
use std::collections::HashSet;
use std::thread;
use std::time::Duration;

// ============================================================================
// SLP-specific helper functions
// ============================================================================

fn slp_supplied_node() -> MarketMakerIt {
    let coins = json! ([
        {"coin":"FORSLP","asset":"FORSLP","required_confirmations":0,"txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"BCH","protocol_data":{"slp_prefix":"slptest"}}},
        {"coin":"ADEXSLP","protocol":{"type":"SLPTOKEN","protocol_data":{"decimals":8,"token_id":get_slp_token_id(),"platform":"FORSLP"}}}
    ]);

    let priv_key = get_prefilled_slp_privkey();
    MarketMakerIt::start(
        json! ({
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
    .unwrap()
}

fn get_balance(mm: &MarketMakerIt, coin: &str) -> BalanceResponse {
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "my_balance",
        "coin": coin,
    })))
    .unwrap();
    assert_eq!(rc.0, StatusCode::OK, "my_balance request failed {}", rc.1);
    json::from_str(&rc.1).unwrap()
}

fn utxo_burn_address() -> Address {
    AddressBuilder::new(
        UtxoAddressFormat::Standard,
        ChecksumType::DSHA256,
        NetworkAddressPrefixes {
            p2pkh: [60].into(),
            p2sh: AddressPrefix::default(),
        },
        None,
    )
    .as_pkh(AddressHashEnum::default_address_hash())
    .build()
    .expect("valid address props")
}

fn withdraw_max_and_send_v1(mm: &MarketMakerIt, coin: &str, to: &str) -> TransactionDetails {
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "withdraw",
        "coin": coin,
        "max": true,
        "to": to,
    })))
    .unwrap();
    assert_eq!(rc.0, StatusCode::OK, "withdraw request failed {}", rc.1);
    let tx_details: TransactionDetails = json::from_str(&rc.1).unwrap();

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "send_raw_transaction",
        "tx_hex": tx_details.tx_hex,
        "coin": coin,
    })))
    .unwrap();
    assert_eq!(rc.0, StatusCode::OK, "send_raw_transaction request failed {}", rc.1);

    tx_details
}

async fn enable_bch_with_tokens_without_balance(
    mm: &MarketMakerIt,
    platform_coin: &str,
    tokens: &[&str],
    mode: UtxoRpcMode,
    tx_history: bool,
) -> Json {
    let slp_requests: Vec<_> = tokens.iter().map(|ticker| json!({ "ticker": ticker })).collect();

    let enable = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "enable_bch_with_tokens",
            "mmrpc": "2.0",
            "params": {
                "ticker": platform_coin,
                "allow_slp_unsafe_conf": true,
                "bchd_urls": [],
                "mode": mode,
                "tx_history": tx_history,
                "slp_tokens_requests": slp_requests,
                "get_balances": false,
            }
        }))
        .await
        .unwrap();
    assert_eq!(
        enable.0,
        StatusCode::OK,
        "'enable_bch_with_tokens' failed: {}",
        enable.1
    );
    json::from_str(&enable.1).unwrap()
}

#[test]
fn test_bch_and_slp_balance() {
    // MM2 should mark the SLP-related and other UTXOs as unspendable BCH balance
    let mm = slp_supplied_node();

    let enable_bch = block_on(enable_native_bch(&mm, "FORSLP", &[]));
    let enable_bch: CoinInitResponse = json::from_value(enable_bch).unwrap();

    let expected_spendable = BigDecimal::from(1000);
    assert_eq!(expected_spendable, enable_bch.balance);

    let expected_unspendable: BigDecimal = "0.00001".parse().unwrap();
    assert_eq!(expected_unspendable, enable_bch.unspendable_balance);

    let bch_balance = get_balance(&mm, "FORSLP");

    assert_eq!(expected_spendable, bch_balance.balance);
    assert_eq!(expected_unspendable, bch_balance.unspendable_balance);

    let enable_slp = block_on(enable_native(&mm, "ADEXSLP", &[], None));

    let expected_spendable = BigDecimal::from(1000);
    assert_eq!(expected_spendable, enable_slp.balance);

    let expected_unspendable: BigDecimal = 0.into();
    assert_eq!(expected_unspendable, enable_slp.unspendable_balance);

    let slp_balance = get_balance(&mm, "ADEXSLP");

    assert_eq!(expected_spendable, slp_balance.balance);
    assert_eq!(expected_unspendable, slp_balance.unspendable_balance);
}

#[test]
fn test_bch_and_slp_balance_enable_slp_v2() {
    // MM2 should mark the SLP-related and other UTXOs as unspendable BCH balance
    let mm = slp_supplied_node();

    let enable_bch = block_on(enable_native_bch(&mm, "FORSLP", &[]));
    let enable_bch: CoinInitResponse = json::from_value(enable_bch).unwrap();

    let expected_spendable = BigDecimal::from(1000);
    assert_eq!(expected_spendable, enable_bch.balance);

    let expected_unspendable: BigDecimal = "0.00001".parse().unwrap();
    assert_eq!(expected_unspendable, enable_bch.unspendable_balance);

    let bch_balance = get_balance(&mm, "FORSLP");

    assert_eq!(expected_spendable, bch_balance.balance);
    assert_eq!(expected_unspendable, bch_balance.unspendable_balance);

    let enable_slp = block_on(enable_slp(&mm, "ADEXSLP"));
    let enable_slp: RpcV2Response<EnableSlpResponse> = json::from_value(enable_slp).unwrap();
    assert_eq!(1, enable_slp.result.balances.len());

    let (_, balance) = enable_slp.result.balances.into_iter().next().unwrap();

    let expected_spendable = BigDecimal::from(1000);
    assert_eq!(expected_spendable, balance.spendable);

    let expected_unspendable: BigDecimal = 0.into();
    assert_eq!(expected_unspendable, balance.unspendable);

    let slp_balance = get_balance(&mm, "ADEXSLP");

    assert_eq!(expected_spendable, slp_balance.balance);
    assert_eq!(expected_unspendable, slp_balance.unspendable_balance);
}

#[test]
fn test_bch_and_slp_balance_enable_bch_with_tokens_v2() {
    let mm = slp_supplied_node();

    let tx_history = false;
    let enable_bch_with_tokens = block_on(enable_bch_with_tokens(
        &mm,
        "FORSLP",
        &["ADEXSLP"],
        UtxoRpcMode::Native,
        tx_history,
        None,
    ));
    let enable_bch_with_tokens: RpcV2Response<EnableBchWithTokensResponse> =
        json::from_value(enable_bch_with_tokens).unwrap();

    let expected_bch_spendable = BigDecimal::from(1000);
    let expected_bch_unspendable: BigDecimal = "0.00001".parse().unwrap();

    let bch_balance = enable_bch_with_tokens
        .result
        .bch_addresses_infos
        .into_iter()
        .next()
        .unwrap()
        .1
        .balances
        .unwrap();

    assert_eq!(expected_bch_spendable, bch_balance.spendable);
    assert_eq!(expected_bch_unspendable, bch_balance.unspendable);

    let slp_balances = enable_bch_with_tokens
        .result
        .slp_addresses_infos
        .into_iter()
        .next()
        .unwrap()
        .1
        .balances
        .unwrap();

    let expected_slp_spendable = BigDecimal::from(1000);
    let expected_slp_unspendable: BigDecimal = 0.into();

    let actual_slp = slp_balances.get("ADEXSLP").unwrap();
    assert_eq!(expected_slp_spendable, actual_slp.spendable);
    assert_eq!(expected_slp_unspendable, actual_slp.unspendable);
}

#[test]
fn test_enable_bch_with_tokens_v2_without_balance() {
    let mm = slp_supplied_node();

    let tx_history = false;
    let enable_bch_with_tokens = block_on(enable_bch_with_tokens_without_balance(
        &mm,
        "FORSLP",
        &["ADEXSLP"],
        UtxoRpcMode::Native,
        tx_history,
    ));

    let enable_bch_with_tokens: RpcV2Response<EnableBchWithTokensResponse> =
        json::from_value(enable_bch_with_tokens).unwrap();

    let (_, bch_balance) = enable_bch_with_tokens
        .result
        .bch_addresses_infos
        .into_iter()
        .next()
        .unwrap();
    assert!(bch_balance.balances.is_none());
    assert!(bch_balance.tickers.is_none());

    let (_, slp_balances) = enable_bch_with_tokens
        .result
        .slp_addresses_infos
        .into_iter()
        .next()
        .unwrap();
    assert!(slp_balances.balances.is_none());
    assert_eq!(slp_balances.tickers.unwrap(), HashSet::from(["ADEXSLP".to_string()]));
}

#[test]
fn test_withdraw_bch_max_must_not_spend_slp() {
    let mm = slp_supplied_node();

    block_on(enable_native_bch(&mm, "FORSLP", &[]));
    block_on(enable_native(&mm, "ADEXSLP", &[], None));

    withdraw_max_and_send_v1(&mm, "FORSLP", &utxo_burn_address().to_string());
    thread::sleep(Duration::from_secs(1));

    let bch_balance = get_balance(&mm, "FORSLP");
    let expected_spendable = BigDecimal::from(0);
    let expected_unspendable: BigDecimal = "0.00001".parse().unwrap();

    assert_eq!(expected_spendable, bch_balance.balance);
    assert_eq!(expected_unspendable, bch_balance.unspendable_balance);

    let slp_balance = get_balance(&mm, "ADEXSLP");
    let expected_spendable = BigDecimal::from(1000);

    assert_eq!(expected_spendable, slp_balance.balance);
}

#[test]
fn test_disable_platform_coin_with_tokens() {
    let mm = slp_supplied_node();
    let _ = block_on(enable_bch_with_tokens(
        &mm,
        "FORSLP",
        &["ADEXSLP"],
        UtxoRpcMode::Native,
        false,
        None,
    ));
    // Try to disable ADEXSLP token.
    let res = block_on(disable_coin(&mm, "ADEXSLP", false));
    assert!(!res.passivized);
    // Check if platform_coin FORSLP is still enabled.
    block_on(my_balance(&mm, "FORSLP"));
    // Check if ADEXSLP token still enabled.
    block_on(assert_coin_not_found_on_balance(&mm, "ADEXSLP"));
    // Try to disable patform_coin.
    let res = block_on(disable_coin(&mm, "FORSLP", false));
    assert!(!res.passivized);

    // Enable enable_bch_with_tokens again to restart the process
    let _ = block_on(enable_bch_with_tokens(
        &mm,
        "FORSLP",
        &["ADEXSLP"],
        UtxoRpcMode::Native,
        false,
        None,
    ));

    // Try to force disable platform coin, FORSLP.
    let res = block_on(disable_coin(&mm, "FORSLP", true));
    assert!(!res.passivized);
}
