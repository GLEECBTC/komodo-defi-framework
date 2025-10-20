use crate::integration_tests_common::*;
use common::executor::Timer;
use common::{block_on, log, now_ms, now_sec, wait_until_ms};
use mm2_number::BigDecimal;
use mm2_test_helpers::electrums::doc_electrums;
use mm2_test_helpers::for_tests::{
    disable_coin, enable_z_coin_light, init_withdraw, pirate_conf, rick_conf, send_raw_transaction, withdraw_status,
    z_coin_tx_history, zombie_conf, MarketMakerIt, Mm2TestConf, ARRR, PIRATE_ELECTRUMS, PIRATE_LIGHTWALLETD_URLS, RICK,
    ZOMBIE_ELECTRUMS, ZOMBIE_LIGHTWALLETD_URLS, ZOMBIE_TICKER,
};
use mm2_test_helpers::structs::{
    EnableCoinBalance, InitTaskResult, RpcV2Response, TransactionDetails, WithdrawStatus, ZcoinHistoryRes,
};
use serde_json::{self as json, json, Value as Json};
use std::collections::HashSet;
use std::iter::FromIterator;
use std::num::NonZeroUsize;
use std::str::FromStr;
use std::thread;
use std::time::Duration;

const ARRR_TEST_BIP39_ACTIVATION_SEED: &str = "course flock lucky cereal hamster novel team never metal bean behind cute cruel matrix symptom fault harsh fashion impact prison glove then tree chef";
const ARRR_TEST_BALANCE_SEED: &str = "zombie test seed";
const ARRR_TEST_ACTIVATION_SEED: &str = "arrr test activation seed";
const ZOMBIE_TEST_HISTORY_SEED: &str = "zombie test history seed";
const ZOMBIE_TEST_WITHDRAW_SEED: &str = "zombie withdraw test seed";
const ZOMBIE_TRADE_BOB_SEED: &str = "RICK ZOMBIE BOB";
const ZOMBIE_TRADE_ALICE_SEED: &str = "RICK ZOMBIE ALICE";

async fn withdraw(mm: &MarketMakerIt, coin: &str, to: &str, amount: &str) -> TransactionDetails {
    let init = init_withdraw(mm, coin, to, amount, None).await;
    let init: RpcV2Response<InitTaskResult> = json::from_value(init).unwrap();
    let timeout = wait_until_ms(150000);

    loop {
        if now_ms() > timeout {
            panic!("{} init_withdraw timed out", coin);
        }

        let status = withdraw_status(mm, init.result.task_id).await;
        log!("Withdraw status {}", json::to_string(&status).unwrap());
        let status: RpcV2Response<WithdrawStatus> = json::from_value(status).unwrap();
        match status.result {
            WithdrawStatus::Ok(result) => break result,
            WithdrawStatus::Error(e) => panic!("{} withdraw error {:?}", coin, e),
            _ => Timer::sleep(1.).await,
        }
    }
}

#[test]
fn activate_z_coin_light() {
    let coins = json!([pirate_conf()]);

    let conf = Mm2TestConf::seednode(ARRR_TEST_BALANCE_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_result = block_on(enable_z_coin_light(
        &mm,
        ARRR,
        PIRATE_ELECTRUMS,
        PIRATE_LIGHTWALLETD_URLS,
        None,
        None,
    ));

    let balance = match activation_result.wallet_balance {
        EnableCoinBalance::Iguana(iguana) => iguana,
        _ => panic!("Expected EnableCoinBalance::Iguana"),
    };
    assert_eq!(balance.balance.spendable, BigDecimal::default());
}

#[test]
fn activate_z_coin_light_with_changing_height() {
    let coins = json!([pirate_conf()]);

    let conf = Mm2TestConf::seednode_with_hd_account(ARRR_TEST_BIP39_ACTIVATION_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_result = block_on(enable_z_coin_light(
        &mm,
        ARRR,
        PIRATE_ELECTRUMS,
        PIRATE_LIGHTWALLETD_URLS,
        None,
        None,
    ));

    let old_first_sync_block = activation_result.first_sync_block;
    let balance = match activation_result.wallet_balance {
        EnableCoinBalance::Iguana(iguana) => iguana,
        _ => panic!("Expected EnableCoinBalance::Iguana"),
    };
    assert_eq!(balance.balance.spendable, BigDecimal::default());

    // disable coin
    block_on(disable_coin(&mm, ARRR, true));

    // Perform activation with changed height
    // Calculate timestamp for 2 days ago
    let two_day_seconds = 2 * 24 * 60 * 60;
    let two_days_ago = now_sec() - two_day_seconds;
    log!(
        "Re-running enable_z_coin_light_with_changing_height with new starting date {}",
        two_days_ago
    );

    let activation_result = block_on(enable_z_coin_light(
        &mm,
        ARRR,
        PIRATE_ELECTRUMS,
        PIRATE_LIGHTWALLETD_URLS,
        None,
        Some(two_days_ago),
    ));

    let new_first_sync_block = activation_result.first_sync_block;
    let balance = match activation_result.wallet_balance {
        EnableCoinBalance::Iguana(iguana) => iguana,
        _ => panic!("Expected EnableCoinBalance::Iguana"),
    };
    assert_eq!(balance.balance.spendable, BigDecimal::default());

    // let's check to make sure first activation starting height is different from current one
    assert_ne!(
        old_first_sync_block.as_ref().unwrap().actual,
        new_first_sync_block.as_ref().unwrap().actual
    );
    // let's check to make sure first activation starting height is greater than current one since we used date later
    // than current date
    assert!(old_first_sync_block.as_ref().unwrap().actual > new_first_sync_block.as_ref().unwrap().actual);
}

#[test]
fn activate_z_coin_with_hd_account() {
    let coins = json!([pirate_conf()]);

    let hd_account_id = 0;
    let conf = Mm2TestConf::seednode_with_hd_account(ARRR_TEST_BIP39_ACTIVATION_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_result = block_on(enable_z_coin_light(
        &mm,
        ARRR,
        PIRATE_ELECTRUMS,
        PIRATE_LIGHTWALLETD_URLS,
        Some(hd_account_id),
        None,
    ));

    let actual = match activation_result.wallet_balance {
        EnableCoinBalance::Iguana(iguana) => iguana.address,
        EnableCoinBalance::HD(_) => panic!("Expected 'Iguana' wallet balance, found HD"),
    };
    assert_eq!(
        actual,
        "zs1p4xfnqmqa4aq5rrnfldafxcggsqhg0wph3elzrzwls9ks95lrg0gtlktjr0t5gg9lj657jyr8m6"
    );
}

// ignored because it requires a long-running Zcoin initialization process
#[test]
#[ignore]
fn test_z_coin_tx_history() {
    let coins = json!([zombie_conf()]);

    let conf = Mm2TestConf::seednode(ZOMBIE_TEST_HISTORY_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    block_on(enable_z_coin_light(
        &mm,
        ZOMBIE_TICKER,
        ZOMBIE_ELECTRUMS,
        ZOMBIE_LIGHTWALLETD_URLS,
        None,
        None,
    ));

    let tx_history = block_on(z_coin_tx_history(&mm, ZOMBIE_TICKER, 5, None));
    log!("History {}", json::to_string(&tx_history).unwrap());

    let response: RpcV2Response<ZcoinHistoryRes> = json::from_value(tx_history).unwrap();

    // all transactions have default fee
    let expected_fee = BigDecimal::from_str("0.00001").unwrap();
    for tx in response.result.transactions.iter() {
        assert_eq!(tx.transaction_fee, expected_fee, "Invalid fee for tx {tx:?}");
    }

    // withdraw transaction to the shielded address
    let withdraw_tx = &response.result.transactions[0];
    assert_eq!(withdraw_tx.internal_id, 5);
    assert_eq!(withdraw_tx.block_height, 169530);
    assert_eq!(withdraw_tx.timestamp, 1656939208);
    assert_eq!(
        withdraw_tx.tx_hash,
        "893b648d2d46de09e24ec7b8b2a13958505656410d1b8ca7838f57a23ae66e4e"
    );

    let from = HashSet::from_iter([
        "zs1xa4lt5w85x4a7awhm5sfm2jz0raz3yqw4ttpyn2ekjcyya00j8l09llz6wpwazcgas6ekj3m054".to_owned(),
    ]);
    assert_eq!(withdraw_tx.from, from);

    let to = HashSet::from_iter([
        "zs1g6z7dcfp5wg085fuzqlauf8d85ct4hke7xmwxe0djnq48909yfsj66hzj0fjgfgzynddud8n04g".to_owned(),
        "zs1xa4lt5w85x4a7awhm5sfm2jz0raz3yqw4ttpyn2ekjcyya00j8l09llz6wpwazcgas6ekj3m054".to_owned(),
    ]);
    assert_eq!(withdraw_tx.to, to);

    // transaction spent 2 inputs: 89985130 and 9999000 sats
    let spent_by_me = BigDecimal::from_str("0.9998413").unwrap();
    assert_eq!(withdraw_tx.spent_by_me, spent_by_me);
    // change output 89983130 sats
    let received_by_me = BigDecimal::from_str("0.8998313").unwrap();
    assert_eq!(withdraw_tx.received_by_me, received_by_me);

    // withdrew 0.1 and paid tx fee
    let my_balance_change = BigDecimal::from_str("-0.1000100").unwrap();
    assert_eq!(withdraw_tx.my_balance_change, my_balance_change);

    // swap payment spent to our address
    let htlc_spend_tx = &response.result.transactions[1];
    assert_eq!(htlc_spend_tx.internal_id, 4);
    assert_eq!(htlc_spend_tx.block_height, 169526);
    assert_eq!(htlc_spend_tx.timestamp, 1656938980);
    assert_eq!(
        htlc_spend_tx.tx_hash,
        "9fb1a1690ddcc37b67d61af509343b0b7d147fc59fe58f4e7f7ca68f0d12f6a2"
    );
    let from = HashSet::from_iter(["bVJG4mSRn5sBSwFWCyeE9EuiogQJtDy3NF".to_owned()]);
    assert_eq!(htlc_spend_tx.from, from);

    let to = HashSet::from_iter([
        "zs1xa4lt5w85x4a7awhm5sfm2jz0raz3yqw4ttpyn2ekjcyya00j8l09llz6wpwazcgas6ekj3m054".to_owned(),
    ]);
    assert_eq!(htlc_spend_tx.to, to);

    // our address didn't spend any inputs
    assert_eq!(htlc_spend_tx.spent_by_me, BigDecimal::default());
    // received 9999000 sats
    let received_by_me = BigDecimal::from_str("0.09999").unwrap();
    assert_eq!(htlc_spend_tx.received_by_me, received_by_me);

    let my_balance_change = BigDecimal::from_str("0.09999").unwrap();
    assert_eq!(htlc_spend_tx.my_balance_change, my_balance_change);

    // swap payment sent from our address
    let htlc_send_tx = &response.result.transactions[2];
    assert_eq!(htlc_send_tx.internal_id, 3);
    assert_eq!(htlc_send_tx.block_height, 169521);
    assert_eq!(htlc_send_tx.timestamp, 1656938572);
    assert_eq!(
        htlc_send_tx.tx_hash,
        "9e7146c77419f8db6975180328cf85c1eac6b18e4e4190293e4c7104c837477d"
    );

    let from = HashSet::from_iter([
        "zs1xa4lt5w85x4a7awhm5sfm2jz0raz3yqw4ttpyn2ekjcyya00j8l09llz6wpwazcgas6ekj3m054".to_owned(),
    ]);
    assert_eq!(htlc_send_tx.from, from);

    let to = HashSet::from_iter([
        "bNyQ2YiiMo1H4GEum3nhymmTRZ8bmkVCFA".to_owned(),
        "zs1xa4lt5w85x4a7awhm5sfm2jz0raz3yqw4ttpyn2ekjcyya00j8l09llz6wpwazcgas6ekj3m054".to_owned(),
    ]);
    assert_eq!(htlc_send_tx.to, to);

    // spent a single 99986130 sats output
    let spent_by_me = BigDecimal::from_str("0.9998613").unwrap();
    assert_eq!(htlc_send_tx.spent_by_me, spent_by_me);
    // change output
    let received_by_me = BigDecimal::from_str("0.8998513").unwrap();
    assert_eq!(htlc_send_tx.received_by_me, received_by_me);

    // 0.1 swap payment amount and transaction fee
    let my_balance_change = BigDecimal::from_str("-0.1000100").unwrap();
    assert_eq!(htlc_send_tx.my_balance_change, my_balance_change);

    // dex fee sent from our address
    let dex_fee_tx = &response.result.transactions[3];
    assert_eq!(dex_fee_tx.internal_id, 2);
    assert_eq!(dex_fee_tx.block_height, 169519);
    assert_eq!(dex_fee_tx.timestamp, 1656938512);
    assert_eq!(
        dex_fee_tx.tx_hash,
        "7d45e5941e8701a030b0c1c0786995e0f638ce9b82cd71cb614a4a1957f1a3ab"
    );

    let from = HashSet::from_iter([
        "zs1xa4lt5w85x4a7awhm5sfm2jz0raz3yqw4ttpyn2ekjcyya00j8l09llz6wpwazcgas6ekj3m054".to_owned(),
    ]);
    assert_eq!(dex_fee_tx.from, from);

    let to = HashSet::from_iter([
        "zs1rp6426e9r6jkq2nsanl66tkd34enewrmr0uvj0zelhkcwmsy0uvxz2fhm9eu9rl3ukxvgzy2v9f".to_owned(),
        "zs1xa4lt5w85x4a7awhm5sfm2jz0raz3yqw4ttpyn2ekjcyya00j8l09llz6wpwazcgas6ekj3m054".to_owned(),
    ]);
    assert_eq!(dex_fee_tx.to, to);

    // spent a single 100000000 sats output
    let spent_by_me = BigDecimal::from(1);
    assert_eq!(dex_fee_tx.spent_by_me, spent_by_me);
    // change output
    let received_by_me = BigDecimal::from_str("0.9998613").unwrap();
    assert_eq!(dex_fee_tx.received_by_me, received_by_me);

    // 0.0001287 dex fee amount and transaction fee
    let my_balance_change = BigDecimal::from_str("-0.0001387").unwrap();
    assert_eq!(dex_fee_tx.my_balance_change, my_balance_change);

    // incoming tx, 2 ZOMBIE received
    let incoming_tx = &response.result.transactions[4];
    assert_eq!(incoming_tx.internal_id, 1);
    assert_eq!(incoming_tx.block_height, 169514);
    assert_eq!(incoming_tx.timestamp, 1656938072);
    assert_eq!(
        incoming_tx.tx_hash,
        "a18a991738fe8568feb7578201250daee7abb9567c247e2241b62149674238c2"
    );

    // from doesn't seem to be detectable for transactions from other shielded addresses
    assert_eq!(incoming_tx.from, HashSet::new());

    let to = HashSet::from_iter([
        "zs1xa4lt5w85x4a7awhm5sfm2jz0raz3yqw4ttpyn2ekjcyya00j8l09llz6wpwazcgas6ekj3m054".to_owned(),
    ]);
    assert_eq!(incoming_tx.to, to);

    assert_eq!(incoming_tx.spent_by_me, BigDecimal::default());

    let received_by_me = BigDecimal::from(2);
    assert_eq!(incoming_tx.received_by_me, received_by_me);

    let my_balance_change = BigDecimal::from(2);
    assert_eq!(incoming_tx.my_balance_change, my_balance_change);

    // check paging by page number
    let page = Some(common::PagingOptionsEnum::PageNumber(NonZeroUsize::new(2).unwrap()));
    let tx_history = block_on(z_coin_tx_history(&mm, ZOMBIE_TICKER, 2, page));
    log!("History {}", json::to_string(&tx_history).unwrap());

    let response: RpcV2Response<ZcoinHistoryRes> = json::from_value(tx_history).unwrap();
    assert_eq!(response.result.transactions.len(), 2);

    assert_eq!(
        response.result.transactions[0].tx_hash,
        "9e7146c77419f8db6975180328cf85c1eac6b18e4e4190293e4c7104c837477d"
    );
    assert_eq!(
        response.result.transactions[1].tx_hash,
        "7d45e5941e8701a030b0c1c0786995e0f638ce9b82cd71cb614a4a1957f1a3ab"
    );
    assert_eq!(response.result.skipped, 2);
    assert_eq!(response.result.total, 5);
    assert_eq!(response.result.limit, 2);
    assert_eq!(response.result.total_pages, 3);

    // check paging by from_id 3
    let page = Some(common::PagingOptionsEnum::FromId(3));
    let tx_history = block_on(z_coin_tx_history(&mm, ZOMBIE_TICKER, 3, page));
    log!("History {}", json::to_string(&tx_history).unwrap());

    let response: RpcV2Response<ZcoinHistoryRes> = json::from_value(tx_history).unwrap();
    assert_eq!(response.result.transactions.len(), 2);

    assert_eq!(
        response.result.transactions[0].tx_hash,
        "7d45e5941e8701a030b0c1c0786995e0f638ce9b82cd71cb614a4a1957f1a3ab"
    );
    assert_eq!(
        response.result.transactions[1].tx_hash,
        "a18a991738fe8568feb7578201250daee7abb9567c247e2241b62149674238c2"
    );
    assert_eq!(response.result.skipped, 3);
    assert_eq!(response.result.total, 5);
    assert_eq!(response.result.limit, 3);
    assert_eq!(response.result.total_pages, 2);

    // check paging by from_id 5
    let page = Some(common::PagingOptionsEnum::FromId(5));
    let tx_history = block_on(z_coin_tx_history(&mm, ZOMBIE_TICKER, 3, page));
    log!("History {}", json::to_string(&tx_history).unwrap());

    let response: RpcV2Response<ZcoinHistoryRes> = json::from_value(tx_history).unwrap();
    assert_eq!(response.result.transactions.len(), 3);

    assert_eq!(
        response.result.transactions[0].tx_hash,
        "9fb1a1690ddcc37b67d61af509343b0b7d147fc59fe58f4e7f7ca68f0d12f6a2"
    );
    assert_eq!(
        response.result.transactions[1].tx_hash,
        "9e7146c77419f8db6975180328cf85c1eac6b18e4e4190293e4c7104c837477d"
    );
    assert_eq!(
        response.result.transactions[2].tx_hash,
        "7d45e5941e8701a030b0c1c0786995e0f638ce9b82cd71cb614a4a1957f1a3ab"
    );
    assert_eq!(response.result.skipped, 1);
    assert_eq!(response.result.total, 5);
    assert_eq!(response.result.limit, 3);
    assert_eq!(response.result.total_pages, 2);
}

// ignored because it requires a long-running Zcoin initialization process
#[test]
#[ignore]
fn withdraw_z_coin_light() {
    let coins = json!([zombie_conf()]);

    let conf = Mm2TestConf::seednode(ZOMBIE_TEST_WITHDRAW_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_result = block_on(enable_z_coin_light(
        &mm,
        ZOMBIE_TICKER,
        ZOMBIE_ELECTRUMS,
        ZOMBIE_LIGHTWALLETD_URLS,
        None,
        None,
    ));

    log!("{:?}", activation_result);

    let withdraw_res = block_on(withdraw(
        &mm,
        ZOMBIE_TICKER,
        "zs1hs0p406y5tntz6wlp7sc3qe4g6ycnnd46leeyt6nyxr42dfvf0dwjkhmjdveukem0x72kkx0tup",
        "0.1",
    ));
    log!("{:?}", withdraw_res);

    // withdrawing to myself, balance change is the fee
    assert_eq!(
        withdraw_res.my_balance_change,
        BigDecimal::from_str("-0.00001").unwrap()
    );

    let send_raw_tx = block_on(send_raw_transaction(&mm, ZOMBIE_TICKER, &withdraw_res.tx_hex));
    log!("{:?}", send_raw_tx);
}

// ignored because it requires a long-running Zcoin initialization process
#[test]
#[ignore]
fn trade_rick_zombie_light() {
    let coins = json!([zombie_conf(), rick_conf()]);
    let bob_passphrase = ZOMBIE_TRADE_BOB_SEED;
    let alice_passphrase = ZOMBIE_TRADE_ALICE_SEED;

    let bob_conf = Mm2TestConf::seednode(bob_passphrase, &coins);
    let mut mm_bob = MarketMakerIt::start(bob_conf.conf, bob_conf.rpc_password, None).unwrap();

    let (_bob_dump_log, _bob_dump_dashboard) = mm_bob.mm_dump();
    log!("Bob log path: {}", mm_bob.log_path.display());

    let zombie_activation = block_on(enable_z_coin_light(
        &mm_bob,
        ZOMBIE_TICKER,
        ZOMBIE_ELECTRUMS,
        ZOMBIE_LIGHTWALLETD_URLS,
        None,
        None,
    ));

    log!("Bob ZOMBIE activation {:?}", zombie_activation);

    let rick_activation = block_on(enable_electrum_json(&mm_bob, RICK, false, doc_electrums()));

    log!("Bob RICK activation {:?}", rick_activation);

    let rc = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": RICK,
        "rel": ZOMBIE_TICKER,
        "price": 1,
        "volume": "0.1"
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    let alice_conf = Mm2TestConf::light_node(alice_passphrase, &coins, &[&mm_bob.ip.to_string()]);
    let mut mm_alice = MarketMakerIt::start(alice_conf.conf, alice_conf.rpc_password, None).unwrap();

    thread::sleep(Duration::from_secs(1));

    let (_alice_dump_log, _alice_dump_dashboard) = mm_alice.mm_dump();
    log!("Alice log path: {}", mm_alice.log_path.display());

    let zombie_activation = block_on(enable_z_coin_light(
        &mm_alice,
        ZOMBIE_TICKER,
        ZOMBIE_ELECTRUMS,
        ZOMBIE_LIGHTWALLETD_URLS,
        None,
        None,
    ));

    log!("Alice ZOMBIE activation {:?}", zombie_activation);

    let rick_activation = block_on(enable_electrum_json(&mm_alice, RICK, false, doc_electrums()));

    log!("Alice RICK activation {:?}", rick_activation);

    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "orderbook",
        "base": RICK,
        "rel": ZOMBIE_TICKER,
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

    thread::sleep(Duration::from_secs(1));

    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": RICK,
        "rel": ZOMBIE_TICKER,
        "volume": "0.1",
        "price": 1
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);

    let buy_json: Json = serde_json::from_str(&rc.1).unwrap();
    let uuid = buy_json["result"]["uuid"].as_str().unwrap().to_owned();

    block_on(mm_alice.wait_for_log(5., |log| log.contains("Entering the taker_swap_loop RICK/ZOMBIE"))).unwrap();

    block_on(mm_bob.wait_for_log(5., |log| log.contains("Entering the maker_swap_loop RICK/ZOMBIE"))).unwrap();

    block_on(mm_bob.wait_for_log(900., |log| log.contains(&format!("[swap uuid={uuid}] Finished")))).unwrap();

    block_on(mm_alice.wait_for_log(900., |log| log.contains(&format!("[swap uuid={uuid}] Finished")))).unwrap();
}

// ignored because it requires a long-running Zcoin initialization process
#[test]
#[ignore]
fn activate_pirate_light() {
    let coins = json!([pirate_conf()]);

    let conf = Mm2TestConf::seednode(ARRR_TEST_ACTIVATION_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_result = block_on(enable_z_coin_light(
        &mm,
        ARRR,
        PIRATE_ELECTRUMS,
        PIRATE_LIGHTWALLETD_URLS,
        None,
        None,
    ));

    let balance = match activation_result.wallet_balance {
        EnableCoinBalance::Iguana(iguana) => iguana,
        _ => panic!("Expected EnableCoinBalance::Iguana"),
    };
    log!("{:?}", balance);
    assert_eq!(balance.balance.spendable, BigDecimal::default());
}
