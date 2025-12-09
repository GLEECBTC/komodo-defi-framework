//! Non-CI tests for 'aggregated taker swaps with liquidity routing' (LR).

use coins::RawTransactionRes;
use common::executor::Timer;
use common::{block_on, log};
use ethereum_types::H256;
use lazy_static::lazy_static;
use mm2_number::bigdecimal::ToPrimitive;
use mm2_number::BigDecimal;
use mm2_rpc::data::legacy::MatchBy;
use mm2_test_helpers::electrums::doc_electrums;
use mm2_test_helpers::for_tests::{
    active_swaps, disable_coin, doc_conf, enable_electrum_json, mm_dump, my_balance, my_swap_status, polygon_conf,
    wait_for_swap_finished, MarketMakerIt, Mm2TestConf, SwapV2TestContracts, TestNode, ARBITRUM_MAINNET_NODES,
    ARBITRUM_MAINNET_SWAP_CONTRACT, ARBITRUM_MAINNET_SWAP_V2_MAKER_CONTRACT, ARBITRUM_MAINNET_SWAP_V2_NFT_CONTRACT,
    ARBITRUM_MAINNET_SWAP_V2_TAKER_CONTRACT, DOC, POLYGON_MAINNET_NODES, POLYGON_MAINNET_SWAP_CONTRACT,
    POLYGON_MAINNET_SWAP_V2_MAKER_CONTRACT, POLYGON_MAINNET_SWAP_V2_NFT_CONTRACT,
    POLYGON_MAINNET_SWAP_V2_TAKER_CONTRACT,
};
use mm2_test_helpers::for_tests::{best_orders_v2_by_number, enable_eth_coin_with_tokens_v2, orderbook_v2};
use mm2_test_helpers::structs::lr_test_structs::{
    ClassicSwapResponse, LrExecuteRoutedTradeResponse, LrFindBestQuoteResponse,
};
use mm2_test_helpers::structs::SetPriceResult;
use mm2_test_helpers::structs::{OrderbookV2Response, RpcOrderbookEntryV2, RpcV2Response, SetPriceResponse};
use serde_json::{json, Value};
use std::str::FromStr;
use uuid::Uuid;
use web3::transports::Http;
use web3::Web3;

const MATIC: &str = "MATIC";
lazy_static! {
    pub static ref POLYGON_WEB3: Web3<Http> = Web3::new(Http::new(POLYGON_MAINNET_NODES[0]).unwrap());
}

/// Test for an aggregated taker swap of MATIC for DOC with interim routing MATIC via a POL token
#[test]
fn test_aggregated_swap_mainnet_polygon_utxo() {
    let bob_passphrase = std::env::var("BOB_MAINNET").expect("BOB_MAINNET env must be set");
    let alice_passphrase = std::env::var("ALICE_MAINNET").expect("ALICE_MAINNET env must be set");

    let user_base = DOC.to_owned(); // 0.0011111 USD
    let user_rel = MATIC.to_owned(); // 0.24 USD
    let swap_amount: BigDecimal = "1".parse().unwrap();
    let method = "buy"; // Sell MATIC buy 1 DOC
    let receive_token = match method {
        "buy" => &user_base,
        "sell" => &user_rel,
        _ => panic!("method must be buy or sell"),
    };

    let dai_conf = dai_plg20_conf(); // 1 USD
    let oneinch_conf = oneinch_plg20_conf(); // Note: crossprices API always returns 'token not in whitelist, unsupported token'
    let agix_conf = agix_plg20_conf(); // Note: crossprices API frequently returns 'token not in whitelist, unsupported token'
    let aave_conf = aave_plg20_conf(); // 289 USD

    let dai_ticker = dai_conf["coin"].as_str().unwrap().to_owned();
    let oneinch_ticker = oneinch_conf["coin"].as_str().unwrap().to_owned();
    let agix_ticker = agix_conf["coin"].as_str().unwrap().to_owned();
    let aave_ticker = aave_conf["coin"].as_str().unwrap().to_owned();

    let bob_coins = json!([doc_conf(), polygon_conf(), dai_conf, oneinch_conf, agix_conf, aave_conf,]);
    let bob_conf = Mm2TestConf::seednode(&bob_passphrase, &bob_coins); // Using legacy swaps until TPU contracts deployed on POLYGON
    let mut mm_bob = block_on(MarketMakerIt::start_async(bob_conf.conf, bob_conf.rpc_password, None)).unwrap();

    let alice_coins = json!([doc_conf(), polygon_conf(), dai_conf, oneinch_conf, agix_conf, aave_conf,]);
    let mut alice_conf = Mm2TestConf::light_node(&alice_passphrase, &alice_coins, &[&mm_bob.ip.to_string()]);
    alice_conf.conf["1inch_api"] = "https://api.1inch.dev".into();
    let mut mm_alice = block_on(MarketMakerIt::start_async(
        alice_conf.conf,
        alice_conf.rpc_password,
        None,
    ))
    .unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);
    log!("Alice log path: {}", mm_alice.log_path.display());
    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    let _bob_enable_pol_tokens = block_on(enable_pol_tokens(
        &mm_bob,
        &[&dai_ticker, &oneinch_ticker, &agix_ticker, &aave_ticker],
    ));
    let _bob_enable_doc = block_on(enable_electrum_json(&mm_bob, DOC, false, doc_electrums()));
    print_balances(&mm_bob, "Bob balance before swap:", &[MATIC, DOC]);

    let _alice_enable_pol_tokens = block_on(enable_pol_tokens(
        &mm_alice,
        &[&dai_ticker, &oneinch_ticker, &agix_ticker, &aave_ticker],
    ));
    let _alice_enable_doc = block_on(enable_electrum_json(&mm_alice, DOC, false, doc_electrums()));
    print_balances(
        &mm_alice,
        "Alice balance before swap:",
        &[DOC, MATIC, &dai_ticker, &oneinch_ticker, &agix_ticker, &aave_ticker],
    );

    let alice_balance_before = block_on(my_balance(&mm_alice, receive_token));

    if let Err(err) = block_on(set_swap_gas_fee_policy(&mm_alice, MATIC, "Medium")) {
        log!("set_swap_transaction_fee_policy on {MATIC} error={}", err);
    }

    let dai_order = block_on(create_maker_order(&mut mm_bob, DOC, &dai_ticker, 0.0011111, 1.0)).unwrap();
    let oneinch_order = block_on(create_maker_order(&mut mm_bob, DOC, &oneinch_ticker, 0.00105, 1.0)).unwrap();
    let agix_order = block_on(create_maker_order(&mut mm_bob, DOC, &agix_ticker, 0.000980009090, 1.0)).unwrap();
    let aave_order = block_on(create_maker_order(
        &mut mm_bob,
        DOC,
        &aave_ticker,
        0.0000037313793103,
        1.0,
    ))
    .unwrap();

    let dai_order = block_on(wait_for_orderbook(&mut mm_alice, DOC, &dai_ticker, &dai_order.uuid, 60)).unwrap();
    let oneinch_order = block_on(wait_for_orderbook(
        &mut mm_alice,
        DOC,
        &oneinch_ticker,
        &oneinch_order.uuid,
        60,
    ))
    .unwrap();
    let agix_order = block_on(wait_for_orderbook(
        &mut mm_alice,
        DOC,
        &agix_ticker,
        &agix_order.uuid,
        60,
    ))
    .unwrap();
    let aave_order = block_on(wait_for_orderbook(
        &mut mm_alice,
        DOC,
        &aave_ticker,
        &aave_order.uuid,
        60,
    ))
    .unwrap();

    let best_orders_res = block_on(best_orders_v2_by_number(&mm_alice, DOC, "buy", 10, true)); // This is the taker action. To get all DOC orders taker uses the "buy" param
    let best_orders = best_orders_res
        .result
        .orders
        .values()
        .flatten()
        .cloned()
        .collect::<Vec<_>>(); // As taker used 'sell' actions, orders are returned as asks

    const LR_SLIPPAGE: f32 = 0.0;
    let best_quote = block_on(find_best_lr_swap(
        &mut mm_alice,
        &user_base,
        &json!([{
            "base": DOC, // pass as ask orders
            "orders": &best_orders,
        }]),
        &json!([]),
        &swap_amount,
        method,
        &user_rel,
    ))
    .expect("best quote should be found");
    print_quote_resp(&best_quote);
    let agg_uuid = block_on(create_and_start_agg_taker_swap(&mut mm_alice, LR_SLIPPAGE, best_quote)).unwrap();

    log!("Aggregated taker swap uuid {:?} started", agg_uuid);
    block_on(Timer::sleep(1.0));

    let active_swaps_alice = block_on(active_swaps(&mm_alice));
    assert_eq!(active_swaps_alice.uuids, vec![agg_uuid]);
    block_on(wait_for_swap_finished(&mm_alice, &agg_uuid.to_string(), 180)); // Only taker has the aggregated swap
    log!("Aggregated taker swap uuid {:?} finished", agg_uuid);

    let taker_swap_status = block_on(my_swap_status(&mm_alice, &agg_uuid.to_string())).unwrap();
    log!(
        "Aggregated taker swap final status {}",
        serde_json::to_string(&taker_swap_status).unwrap()
    );
    check_my_agg_swap_final_status(&taker_swap_status);
    let alice_balance_after = block_on(my_balance(&mm_alice, receive_token));
    print_balances(
        &mm_alice,
        "Alice balance after swap:",
        &[DOC, MATIC, &dai_ticker, &oneinch_ticker, &agix_ticker, &aave_ticker],
    );

    let status = block_on(my_swap_status(&mm_alice, &agg_uuid.to_string())).unwrap();
    if let Some(atomic_swap_uuid) = status["result"]["atomic_swap_uuid"].as_str() {
        log!("Waiting for Maker to finish atomic_swap_uuid={atomic_swap_uuid}");
        // This may take long time on MATIC mainnet due to the Maker spending a PLG20 token from the HTLC with a low gas fee
        block_on(wait_for_swap_finished(&mm_bob, atomic_swap_uuid, 60));
    }

    block_on(cancel_order(&mm_bob, &dai_order.uuid));
    block_on(cancel_order(&mm_bob, &oneinch_order.uuid));
    block_on(cancel_order(&mm_bob, &agix_order.uuid));
    block_on(cancel_order(&mm_bob, &aave_order.uuid));
    block_on(Timer::sleep(10.0)); // wait for orderbook update

    block_on(disable_coin(&mm_bob, MATIC, false));
    block_on(disable_coin(&mm_bob, DOC, false));
    block_on(disable_coin(&mm_alice, MATIC, false));
    block_on(disable_coin(&mm_alice, DOC, false));

    // Rough check taker swap amount received
    let alice_bal_diff = &alice_balance_after.balance - &alice_balance_before.balance;
    log!("Alice received amount {}: {}", receive_token, alice_bal_diff);
    assert!(
        alice_bal_diff > &swap_amount * "0.80".parse::<BigDecimal>().unwrap(),
        "too much received {}",
        alice_bal_diff
    );
    assert!(
        alice_bal_diff < &swap_amount * "1.20".parse::<BigDecimal>().unwrap(),
        "too low received {}",
        alice_bal_diff
    );
}

#[test]
fn test_aggregated_swap_mainnet_polygon_arbitrum_sell() {
    let user_base = MATIC.to_owned(); // 0.24 USD
    let user_rel = crv_arb20_conf()["coin"].as_str().unwrap().to_owned(); // 0.63 USD
    let swap_amount: BigDecimal = "0.3".parse().unwrap();
    let method = "sell"; // Sell 0.3 MATIC, buy CRV-ARB20
    test_aggregated_swap_mainnet_polygon_arbitrum_impl(&user_base, &user_rel, swap_amount, method, false, false);
}

#[test]
fn test_aggregated_swap_mainnet_polygon_arbitrum_buy() {
    let user_base = crv_arb20_conf()["coin"].as_str().unwrap().to_owned(); // 0.63 USD
    let user_rel = MATIC.to_owned(); // 0.24 USD
    let swap_amount: BigDecimal = "0.1".parse().unwrap();
    let method = "buy"; // Buy 0.1 CRV-ARB20, sell MATIC
    test_aggregated_swap_mainnet_polygon_arbitrum_impl(&user_base, &user_rel, swap_amount, method, false, true);
}

fn test_aggregated_swap_mainnet_polygon_arbitrum_impl(
    user_base: &str,
    user_rel: &str,
    swap_amount: BigDecimal,
    method: &str,
    run_swap: bool,
    use_asks: bool,
) {
    let bob_passphrase = std::env::var("BOB_MAINNET").expect("BOB_MAINNET env must be set");
    let alice_passphrase = std::env::var("ALICE_MAINNET").expect("ALICE_MAINNET env must be set");

    let receive_token = match method {
        "buy" => user_base,
        "sell" => user_rel,
        _ => panic!("method must be buy or sell"),
    };

    let dai_conf = dai_plg20_conf(); // 1 USD
    let oneinch_conf = oneinch_plg20_conf(); // Note: crossprices API always returns 'token not in whitelist, unsupported token'
    let agix_conf = agix_plg20_conf(); // Note: crossprices API frequently returns 'token not in whitelist, unsupported token'
    let aave_conf = aave_plg20_conf(); // 289 USD
    let eth_arb_conf = eth_arb_conf(); // Ethereum in Arb One 2,500 USD
    let arb_conf = arb_arb20_conf(); // 0.41 USD
    let grt_conf = grt_arb20_conf(); // 0.0098 USD
    let crv_conf = crv_arb20_conf(); // 0.63 USD

    let dai_ticker = dai_conf["coin"].as_str().unwrap().to_owned();
    let oneinch_ticker = oneinch_conf["coin"].as_str().unwrap().to_owned();
    let agix_ticker = agix_conf["coin"].as_str().unwrap().to_owned();
    let aave_ticker = aave_conf["coin"].as_str().unwrap().to_owned();
    let eth_arb_ticker = eth_arb_conf["coin"].as_str().unwrap().to_owned();
    let arb_ticker = arb_conf["coin"].as_str().unwrap().to_owned();
    let grt_ticker = grt_conf["coin"].as_str().unwrap().to_owned();
    let crv_ticker = crv_conf["coin"].as_str().unwrap().to_owned();

    let bob_coins = json!([
        doc_conf(),
        polygon_conf(),
        dai_conf,
        oneinch_conf,
        agix_conf,
        aave_conf,
        eth_arb_conf,
        arb_conf,
        grt_conf,
        crv_conf
    ]);
    let bob_conf = Mm2TestConf::seednode(&bob_passphrase, &bob_coins); // Using legacy swaps until TPU contracts deployed on POLYGON
    let mut mm_bob = block_on(MarketMakerIt::start_async(bob_conf.conf, bob_conf.rpc_password, None)).unwrap();

    let alice_coins = json!([
        doc_conf(),
        polygon_conf(),
        dai_conf,
        oneinch_conf,
        agix_conf,
        aave_conf,
        eth_arb_conf,
        arb_conf,
        grt_conf,
        crv_conf
    ]);
    let mut alice_conf = Mm2TestConf::light_node(&alice_passphrase, &alice_coins, &[&mm_bob.ip.to_string()]);
    alice_conf.conf["1inch_api"] = "https://api.1inch.dev".into();
    let mut mm_alice = block_on(MarketMakerIt::start_async(
        alice_conf.conf,
        alice_conf.rpc_password,
        None,
    ))
    .unwrap();

    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);
    log!("Alice log path: {}", mm_alice.log_path.display());
    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    let _bob_enable_pol_tokens = block_on(enable_pol_tokens(
        &mm_bob,
        &[&dai_ticker, &oneinch_ticker, &agix_ticker, &aave_ticker],
    ));
    let _bob_enable_arb_tokens = block_on(enable_arb_tokens(&mm_bob, &[&arb_ticker, &crv_ticker, &grt_ticker]));
    print_balances(
        &mm_bob,
        "Bob balance before swap:",
        &[MATIC, &eth_arb_ticker, &arb_ticker, &grt_ticker],
    );

    let _alice_enable_pol_tokens = block_on(enable_pol_tokens(
        &mm_alice,
        &[&dai_ticker, &oneinch_ticker, &agix_ticker, &aave_ticker],
    ));
    let _alice_enable_arb_tokens = block_on(enable_arb_tokens(&mm_alice, &[&arb_ticker, &crv_ticker, &grt_ticker]));
    print_balances(
        &mm_alice,
        "Alice balance before swap:",
        &[
            MATIC,
            &dai_ticker,
            &oneinch_ticker,
            &agix_ticker,
            &aave_ticker,
            &eth_arb_ticker,
            &arb_ticker,
            &grt_ticker,
            &crv_ticker,
        ],
    );
    let alice_balance_before = block_on(my_balance(&mm_alice, receive_token));

    if let Err(err) = block_on(set_swap_gas_fee_policy(&mm_alice, MATIC, "Medium")) {
        log!("set_swap_transaction_fee_policy on {MATIC} error={}", err);
    }

    let arb_order = block_on(create_maker_order(&mut mm_bob, &arb_ticker, &aave_ticker, 0.00141, 1.0)).unwrap();
    let grt_order = block_on(create_maker_order(
        &mut mm_bob,
        &grt_ticker,
        &aave_ticker,
        0.00003401384,
        7.0,
    ))
    .unwrap();
    let arb_order = block_on(wait_for_orderbook(
        &mut mm_alice,
        &arb_ticker,
        &aave_ticker,
        &arb_order.uuid,
        60,
    ))
    .unwrap();
    let grt_order = block_on(wait_for_orderbook(
        &mut mm_alice,
        &grt_ticker,
        &aave_ticker,
        &grt_order.uuid,
        60,
    ))
    .unwrap();

    // The best_orders RPC accepts "buy" or "sell" as the action from the taker perspective.
    // If taker calls with "sell" the best_orders RPC returns the orders as bids.
    // If taker calls with "buy" the best_orders RPC returns the orders as asks.
    let mut json_asks = vec![];
    let mut json_bids = vec![];
    if use_asks {
        let best_orders_res = block_on(best_orders_v2_by_number(&mm_alice, &grt_ticker, "buy", 10, true));
        let best_asks_0 = best_orders_res
            .result
            .orders
            .values()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        json_asks.push(json!({ "base": &grt_ticker, "orders": &best_asks_0 }));

        let best_orders_res = block_on(best_orders_v2_by_number(&mm_alice, &arb_ticker, "buy", 10, true)); // This is the taker action. To get all aave orders taker uses "sell"
        let best_asks_1 = best_orders_res
            .result
            .orders
            .values()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        json_asks.push(json!({ "base": &arb_ticker, "orders": &best_asks_1 }));
    } else {
        // using "sell" we get all aave orders as bids:
        let best_orders_res = block_on(best_orders_v2_by_number(&mm_alice, &aave_ticker, "sell", 10, true));
        let best_bids = best_orders_res
            .result
            .orders
            .values()
            .flatten()
            .cloned()
            .collect::<Vec<_>>();
        json_bids.push(json!({ "rel": &aave_ticker, "orders": &best_bids }));
    }

    const LR_SLIPPAGE: f32 = 0.0;
    let best_quote = block_on(find_best_lr_swap(
        &mut mm_alice,
        user_base,
        &json!(json_asks),
        &json!(json_bids),
        &swap_amount,
        method,
        user_rel,
    ))
    .expect("best quote should be found");
    print_quote_resp(&best_quote);

    if !run_swap {
        return;
    }
    let agg_uuid = block_on(create_and_start_agg_taker_swap(&mut mm_alice, LR_SLIPPAGE, best_quote)).unwrap();

    log!("Aggregated taker swap uuid {:?} started", agg_uuid);
    block_on(Timer::sleep(1.0));

    let active_swaps_alice = block_on(active_swaps(&mm_alice));
    assert_eq!(active_swaps_alice.uuids, vec![agg_uuid]);
    block_on(wait_for_swap_finished(&mm_alice, &agg_uuid.to_string(), 600)); // Only taker has the aggregated swap
    log!("Aggregated taker swap uuid {:?} finished", agg_uuid);

    let taker_swap_status = block_on(my_swap_status(&mm_alice, &agg_uuid.to_string())).unwrap();
    log!(
        "Aggregated taker swap final status {}",
        serde_json::to_string(&taker_swap_status).unwrap()
    );
    check_my_agg_swap_final_status(&taker_swap_status);
    let alice_balance_after = block_on(my_balance(&mm_alice, receive_token));
    print_balances(
        &mm_alice,
        "Alice balance after swap:",
        &[
            MATIC,
            &dai_ticker,
            &oneinch_ticker,
            &agix_ticker,
            &aave_ticker,
            &eth_arb_ticker,
            &arb_ticker,
            &grt_ticker,
            &crv_ticker,
        ],
    );

    let status = block_on(my_swap_status(&mm_alice, &agg_uuid.to_string())).unwrap();
    if let Some(atomic_swap_uuid) = status["result"]["atomic_swap_uuid"].as_str() {
        log!("Waiting for Maker to finish atomic_swap_uuid={atomic_swap_uuid}");
        // This may take long time on MATIC mainnet if Maker has spent a PLG20 token from HTLC with a low gas fee
        block_on(wait_for_swap_finished(&mm_bob, atomic_swap_uuid, 60));
    }
    block_on(cancel_order(&mm_bob, &arb_order.uuid));
    block_on(cancel_order(&mm_bob, &grt_order.uuid));
    block_on(Timer::sleep(10.0)); // wait for orderbook update

    block_on(disable_coin(&mm_bob, MATIC, false));
    block_on(disable_coin(&mm_bob, &eth_arb_ticker, false));
    block_on(disable_coin(&mm_alice, MATIC, false));
    block_on(disable_coin(&mm_alice, &eth_arb_ticker, false));

    // Rough check taker swap amount received
    let alice_bal_diff = &alice_balance_after.balance - &alice_balance_before.balance;
    log!("Alice received amount {}: {}", receive_token, alice_bal_diff);
    assert!(
        alice_bal_diff > &swap_amount * "0.80".parse::<BigDecimal>().unwrap(),
        "too much received {}",
        alice_bal_diff
    );
    assert!(
        alice_bal_diff < &swap_amount * &"1.20".parse::<BigDecimal>().unwrap(),
        "too low received {}",
        alice_bal_diff
    );
}

/// Not used yet
/// We may use this to top-up a token before an agg swap test,
/// for e.g. if we want to do liquidity routing from this token
#[allow(unused)]
async fn top_up_token(mm: &mut MarketMakerIt, base: &str, rel: &str, amount: f64) -> Result<(), String> {
    make_1inch_swap(mm, base, rel, amount).await?;
    Ok(())
}

/// Not used yet
/// Make 1inch classic swap to top-up tokens for the test
/// Use default tx fee set by the 1inch rpc (should be medium)
#[allow(unused)]
async fn make_1inch_swap(mm: &mut MarketMakerIt, base: &str, rel: &str, amount: f64) -> Result<String, String> {
    common::log::info!("Issue {}/{} 1inch swap request", base, rel);
    let rc = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "experimental::1inch_v6_0::classic_swap_create",
            "mmrpc": "2.0",
            "params": {
                "base": base,
                "rel": rel,
                "amount": amount,
                "slippage": 1
            }
        }))
        .await
        .unwrap();
    if !rc.0.is_success() {
        return Err(format!("cannot create 1inch swap: {}", rc.1));
    }
    let swap_resp: RpcV2Response<ClassicSwapResponse> = serde_json::from_str(&rc.1).unwrap();
    let tx_fields = swap_resp.result.tx.unwrap();

    let rc = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "sign_raw_transaction",
            "mmrpc": "2.0",
            "params": {
                "coin": base,
                "type": "ETH",
                "tx": {
                    "to": tx_fields.to,
                    "value": tx_fields.value,
                    "gas_limit": tx_fields.gas,
                    "pay_for_gas": {
                        "tx_type": "Legacy",
                        "gas_price": tx_fields.gas_price
                    },
                    "data": tx_fields.data
                }
            }
        }))
        .await
        .unwrap();
    if !rc.0.is_success() {
        return Err(format!("cannot sign 1inch swap tx: {}", rc.1));
    }
    let sign_resp: RpcV2Response<RawTransactionRes> = serde_json::from_str(&rc.1).unwrap();

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "send_raw_transaction",
        "coin": base,
        "tx_hex": sign_resp.result.tx_hex
    })))
    .unwrap();
    if !rc.0.is_success() {
        return Err(format!("could not do 1inch send_raw_transaction: {}", rc.1));
    }
    let send_resp: Value = serde_json::from_str(&rc.1).unwrap();
    Ok(send_resp["tx_hash"].to_string())
}

#[allow(unused)]
async fn wait_for_confirmations(tx_hashes: &[&str], timeout: u32) -> Result<(), String> {
    const RETRY_DELAY: u32 = 5;
    let mut waited = 0;
    let mut unconfirmed = tx_hashes
        .iter()
        .map(|hash_str| H256::from_str(hash_str).unwrap())
        .collect::<Vec<_>>();
    loop {
        let mut unconfirmed_new = vec![];
        for hash in unconfirmed {
            if let Ok(receipt) = POLYGON_WEB3.eth().transaction_receipt(hash).await {
                if receipt.is_some() {
                    continue;
                }
            }
            unconfirmed_new.push(hash);
        }
        if unconfirmed_new.is_empty() {
            return Ok(());
        }
        if waited > timeout {
            break;
        }
        Timer::sleep(RETRY_DELAY as f64).await;
        waited += RETRY_DELAY;
        unconfirmed = unconfirmed_new;
    }
    Err("timeout waiting for confirmations".to_owned())
}

/// issue setprice request on Bob side by setting base/rel price
async fn create_maker_order(
    maker: &mut MarketMakerIt,
    base: &str,
    rel: &str,
    price: f64,
    volume: f64,
) -> Result<SetPriceResult, String> {
    common::log::info!("Issue maker {}/{} setprice request", base, rel);
    let rc = maker
        .rpc(&json!({
            "userpass": maker.userpass,
            "method": "setprice",
            "base": base,
            "rel": rel,
            "price": price,
            "volume": volume
        }))
        .await
        .unwrap();
    if rc.0.is_success() {
        let resp: SetPriceResponse = serde_json::from_str(&rc.1).unwrap();
        Ok(resp.result)
    } else {
        Err(rc.1)
    }
}

async fn wait_for_orderbook(
    mm: &mut MarketMakerIt,
    base: &str,
    rel: &str,
    uuid: &Uuid,
    timeout: u32,
) -> Result<RpcOrderbookEntryV2, String> {
    let mut waited = 0;
    const RETRY_DELAY: u32 = 5;
    loop {
        let orderbook_v2 = orderbook_v2(mm, base, rel).await;
        let orderbook_v2: RpcV2Response<OrderbookV2Response> = serde_json::from_value(orderbook_v2).unwrap();
        if let Some(e) = orderbook_v2.result.asks.iter().find(|ask| &ask.entry.uuid == uuid) {
            return Ok(e.entry.clone());
        }
        if let Some(e) = orderbook_v2.result.bids.iter().find(|bid| &bid.entry.uuid == uuid) {
            return Ok(e.entry.clone());
        }
        if waited > timeout {
            break;
        }
        Timer::sleep(RETRY_DELAY as f64).await;
        waited += RETRY_DELAY;
    }
    Err(format!("no uuid {uuid} in orderbook"))
}

async fn find_best_lr_swap(
    taker: &mut MarketMakerIt,
    user_base: &str,
    asks: &Value,
    bids: &Value,
    volume: &BigDecimal,
    method: &str,
    user_rel: &str,
) -> Result<LrFindBestQuoteResponse, String> {
    log!("Issue find_best_quote {}/{} request", user_base, user_rel);
    let rc = taker
        .rpc(&json!({
            "userpass": taker.userpass,
            "method": "experimental::liquidity_routing::find_best_quote",
            "mmrpc": "2.0",
            "params": {
                "user_base": user_base,
                "volume": volume,
                "asks": asks,
                "bids": bids,
                "method": method,
                "user_rel": user_rel
            }
        }))
        .await
        .unwrap();
    if rc.0.is_success() {
        let resp: RpcV2Response<LrFindBestQuoteResponse> = serde_json::from_str(&rc.1).unwrap();
        Ok(resp.result)
    } else {
        Err(rc.1)
    }
}

async fn create_and_start_agg_taker_swap(
    taker: &mut MarketMakerIt,
    slippage: f32,
    best_quote: LrFindBestQuoteResponse,
) -> Result<Uuid, String> {
    let lr_swap_0 = best_quote.lr_data_0.map(|swap_details| {
        json!({
            "slippage": slippage,
            "swap_details": swap_details
        })
    });
    let lr_swap_1 = best_quote.lr_data_1.map(|swap_details| {
        json!({
            "slippage": slippage,
            "swap_details": swap_details
        })
    });
    let mut atomic_swap = best_quote.atomic_swap;
    // We discussed that we should not match a specific order.
    // I set this for now but we need to think about this again:
    // if a different order is taken the LR calculations may be wrong for it.
    atomic_swap.match_by = Some(MatchBy::Orders([atomic_swap.order_uuid].into()));
    log!(
        "Issue execute_routed_trade {} {}/{} request",
        atomic_swap.method,
        atomic_swap.base,
        atomic_swap.rel
    );
    let rc = taker
        .rpc(&json!({
            "userpass": taker.userpass,
            "method": "experimental::liquidity_routing::execute_routed_trade",
            "mmrpc": "2.0",
            "params": {
                "lr_swap_0": lr_swap_0,
                "atomic_swap": atomic_swap,
                "lr_swap_1": lr_swap_1,
            }
        }))
        .await
        .unwrap();
    if rc.0.is_success() {
        let resp: RpcV2Response<LrExecuteRoutedTradeResponse> = serde_json::from_str(&rc.1).unwrap();
        Ok(resp.result.uuid)
    } else {
        Err(rc.1)
    }
}

/// Helper function requesting my swap status and checking it's events
fn check_my_agg_swap_final_status(status_response: &Value) {
    assert!(status_response["result"]["is_finished"].as_bool().unwrap());

    let events = status_response["result"]["events"].as_array().unwrap();
    assert!(!events.is_empty());
    assert!(
        events
            .iter()
            .any(|item| item["event_type"].as_str() == Some("Completed")),
        "swap not completed successfully"
    );
    assert!(
        !events.iter().any(|item| item["event_type"].as_str() == Some("Aborted")),
        "swap was aborted"
    );
}

async fn cancel_order(mm: &MarketMakerIt, uuid: &Uuid) {
    mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "cancel_order",
        "uuid": uuid,
    }))
    .await
    .unwrap();
}

async fn set_swap_gas_fee_policy(mm: &MarketMakerIt, coin: &str, fee_policy: &str) -> Result<(), String> {
    let rc = mm
        .rpc(&json! ({
            "userpass": mm.userpass,
            "mmrpc": "2.0",
            "method": "set_swap_gas_fee_policy",
            "params": {
                "coin": coin,
                "swap_gas_fee_policy": fee_policy
            }
        }))
        .await
        .unwrap();
    if rc.0.is_success() {
        Ok(())
    } else {
        Err(rc.1)
    }
}

#[allow(unused)]
fn check_results<T: std::fmt::Debug>(results: &[Result<T, String>], msg: &str) {
    let mut err_msg = "".to_owned();
    for (i, r) in results.iter().enumerate() {
        if r.is_err() {
            err_msg += format!(" result[{i}]={}", r.as_ref().unwrap_err()).as_str();
        }
    }
    if !err_msg.is_empty() {
        log!("{},{}", msg, err_msg);
        panic!("{}", msg);
    }
}

fn print_balances(mm: &MarketMakerIt, msg: &str, tickers: &[&str]) {
    for ticker in tickers {
        let balance = block_on(my_balance(mm, ticker));
        log!(
            "{} {}: address={:?} balance={:?} unspendable_balance={:?}",
            msg,
            ticker,
            balance.address,
            balance.balance,
            balance.unspendable_balance
        );
    }
}

fn print_quote_resp(quote: &LrFindBestQuoteResponse) {
    log!(
        "Found best quote for swap with LR:
        LR_0: src_token={:?} src_amount={:?} dst_token={:?} dst_amount={:?}
        atomic swap params: base={} rel={} method={} price={} volume={:?}
        LR_1: src_token={:?} src_amount={:?} dst_token={:?} dst_amount={:?}",
        quote
            .lr_data_0
            .as_ref()
            .and_then(|data| data.src_token.as_ref().unwrap().symbol_kdf.as_ref()),
        quote.lr_data_0.as_ref().map(|data| data.src_amount.to_decimal()),
        quote
            .lr_data_0
            .as_ref()
            .and_then(|data| data.dst_token.as_ref().unwrap().symbol_kdf.as_ref()),
        quote.lr_data_0.as_ref().map(|data| data.dst_amount.as_ratio().to_f64()),
        quote.atomic_swap.base,
        quote.atomic_swap.rel,
        quote.atomic_swap.method,
        quote.atomic_swap.price.to_decimal(),
        quote.atomic_swap.volume.as_ref().map(|v| v.to_decimal()),
        quote
            .lr_data_1
            .as_ref()
            .and_then(|data| data.src_token.as_ref().unwrap().symbol_kdf.as_ref()),
        quote.lr_data_1.as_ref().map(|data| data.src_amount.to_decimal()),
        quote
            .lr_data_1
            .as_ref()
            .and_then(|data| data.dst_token.as_ref().unwrap().symbol_kdf.as_ref()),
        quote.lr_data_1.as_ref().map(|data| data.dst_amount.as_ratio().to_f64()),
    );
}

fn dai_plg20_conf() -> Value {
    // 1 USD
    json!({
        "coin": "DAI-PLG20",
        "name": "dai_plg20",
        "derivation_path": "m/44'/966'",
        "decimals": 18,
        "protocol": {
            "type": "ERC20",
            "protocol_data": {
                "platform": MATIC,
                "contract_address": "0x8f3Cf7ad23Cd3CaDbD9735AFf958023239c6A063"
            }
        }
    })
}

fn oneinch_plg20_conf() -> Value {
    json!({
        "coin": "1INCH-PLG20",
        "name": "1inch_plg20",
        "derivation_path": "m/44'/966'",
        "decimals": 18,
        "protocol": {
            "type": "ERC20",
            "protocol_data": {
                "platform": MATIC,
                "contract_address": "0x9c2C5fd7b07E95EE044DDeba0E97a665F142394f"
            }
        }
    })
}

fn agix_plg20_conf() -> Value {
    json!({
        "coin": "AGIX-PLG20",
        "name": "agix_plg20",
        "derivation_path": "m/44'/966'",
        "decimals": 18,
        "protocol": {
            "type": "ERC20",
            "protocol_data": {
                "platform": MATIC,
                "contract_address": "0x190Eb8a183D22a4bdf278c6791b152228857c033"
            }
        }
    })
}

fn aave_plg20_conf() -> Value {
    json!({
        "coin": "AAVE-PLG20",
        "name": "aave_plg20",
        "derivation_path": "m/44'/966'",
        "decimals": 18,
        "protocol": {
            "type": "ERC20",
            "protocol_data": {
                "platform": MATIC,
                "contract_address": "0xD6DF932A45C0f255f85145f286eA0b292B21C90B"
            }
        }
    })
}

// Ethereum in Arb One
fn eth_arb_conf() -> Value {
    json!({
        "coin": "ETH-ARB20",
        "name": "eth_arb20",
        "fname": "Ethereum",
        "rpcport": 80,
        "mm2": 1,
        "chain_id": 42161,
        "required_confirmations": 10,
        "avg_blocktime": 0.25,
        "protocol": {
            "type": "ETH",
            "protocol_data": {
                "chain_id": 42161
            }
        },
        "derivation_path": "m/44'/60'",
        "use_access_list": true,
        "max_eth_tx_type": 2,
        "gas_limit": {
            "eth_send_coins": 300000,
            "eth_payment": 700000,
            "eth_receiver_spend": 600000,
            "eth_sender_refund": 600000
        }
    })
}

fn arb_arb20_conf() -> Value {
    json!({
        "coin": "ARB-ARB20",
        "name": "arb_arb20",
        "fname": "Arbitrum",
        "rpcport": 80,
        "mm2": 1,
        "chain_id": 42161,
        "decimals": 18,
        "avg_blocktime": 0.25,
        "required_confirmations": 10,
        "protocol": {
        "type": "ERC20",
        "protocol_data": {
            "platform": "ETH-ARB20",
            "contract_address": "0x912CE59144191C1204E64559FE8253a0e49E6548"
        }
        },
        "derivation_path": "m/44'/60'",
        "use_access_list": true,
        "max_eth_tx_type": 2,
        "gas_limit": {
            "eth_send_erc20": 400000,
            "erc20_payment": 800000,
            "erc20_receiver_spend": 700000,
            "erc20_sender_refund": 700000
        }
    })
}

fn grt_arb20_conf() -> Value {
    json!({
        "coin": "GRT-ARB20",
        "name": "grt_arb20",
        "fname": "The Graph",
        "rpcport": 80,
        "mm2": 1,
        "chain_id": 42161,
        "decimals": 18,
        "avg_blocktime": 0.25,
        "required_confirmations": 10,
        "protocol": {
            "type": "ERC20",
            "protocol_data": {
                "platform": "ETH-ARB20",
                "contract_address": "0x9623063377AD1B27544C965cCd7342f7EA7e88C7"
            }
        },
        "derivation_path": "m/44'/60'",
        "use_access_list": true,
        "max_eth_tx_type": 2,
        "gas_limit": {
            "eth_send_erc20": 400000,
            "erc20_payment": 800000,
            "erc20_receiver_spend": 700000,
            "erc20_sender_refund": 700000
        }
    })
}

fn crv_arb20_conf() -> Value {
    json!({
        "coin": "CRV-ARB20",
        "name": "crv_arb20",
        "fname": "Curve DAO",
        "rpcport": 80,
        "mm2": 1,
        "chain_id": 42161,
        "decimals": 18,
        "avg_blocktime": 0.25,
        "required_confirmations": 10,
        "protocol": {
            "type": "ERC20",
            "protocol_data": {
                "platform": "ETH-ARB20",
                "contract_address": "0x11cDb42B0EB46D95f990BeDD4695A6e3fA034978"
            }
        },
        "derivation_path": "m/44'/60'",
        "use_access_list": true,
        "max_eth_tx_type": 2,
        "gas_limit": {
            "eth_send_erc20": 400000,
            "erc20_payment": 800000,
            "erc20_receiver_spend": 700000,
            "erc20_sender_refund": 700000
        }
    })
}

async fn enable_pol_tokens(mm: &MarketMakerIt, tickers: &[&str]) -> Value {
    let pol_contracts_v2 = SwapV2TestContracts {
        maker_swap_v2_contract: POLYGON_MAINNET_SWAP_V2_MAKER_CONTRACT.to_owned(),
        taker_swap_v2_contract: POLYGON_MAINNET_SWAP_V2_TAKER_CONTRACT.to_owned(),
        nft_maker_swap_v2_contract: POLYGON_MAINNET_SWAP_V2_NFT_CONTRACT.to_owned(),
    };
    let polygon_nodes = POLYGON_MAINNET_NODES
        .iter()
        .map(|ip| TestNode { url: (*ip).to_owned() })
        .collect::<Vec<_>>();

    enable_eth_coin_with_tokens_v2(
        mm,
        MATIC,
        tickers,
        POLYGON_MAINNET_SWAP_CONTRACT,
        pol_contracts_v2,
        None,
        &polygon_nodes,
    )
    .await
}

async fn enable_arb_tokens(mm: &MarketMakerIt, tickers: &[&str]) -> Value {
    let arb_contracts_v2 = SwapV2TestContracts {
        maker_swap_v2_contract: ARBITRUM_MAINNET_SWAP_V2_MAKER_CONTRACT.to_owned(),
        taker_swap_v2_contract: ARBITRUM_MAINNET_SWAP_V2_TAKER_CONTRACT.to_owned(),
        nft_maker_swap_v2_contract: ARBITRUM_MAINNET_SWAP_V2_NFT_CONTRACT.to_owned(),
    };
    let arb_nodes = ARBITRUM_MAINNET_NODES
        .iter()
        .map(|ip| TestNode { url: (*ip).to_owned() })
        .collect::<Vec<_>>();

    enable_eth_coin_with_tokens_v2(
        mm,
        eth_arb_conf()["coin"].as_str().unwrap(),
        tickers,
        ARBITRUM_MAINNET_SWAP_CONTRACT,
        arb_contracts_v2,
        None,
        &arb_nodes,
    )
    .await
}
