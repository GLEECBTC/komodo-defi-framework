use common::{block_on, log};
use mm2_number::BigDecimal;
use mm2_rpc::data::legacy::OrderbookResponse;
use mm2_test_helpers::for_tests::{
    atom_testnet_conf, disable_coin, disable_coin_err, enable_tendermint, enable_tendermint_token,
    enable_tendermint_without_balance, get_tendermint_my_tx_history, ibc_withdraw, iris_ibc_nucleus_testnet_conf,
    my_balance, nucleus_testnet_conf, orderbook, orderbook_v2, send_raw_transaction, set_price,
    tendermint_add_delegation, tendermint_delegations, tendermint_ongoing_undelegations, tendermint_remove_delegation,
    tendermint_remove_delegation_raw, tendermint_validators, withdraw_v1, MarketMakerIt, Mm2TestConf,
};
use mm2_test_helpers::structs::{
    Bip44Chain, HDAccountAddressId, OrderbookAddress, OrderbookV2Response, RpcV2Response, TendermintActivationResult,
    TransactionDetails, TransactionType,
};
use serde_json::json;
use std::collections::HashSet;
use std::iter::FromIterator;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

const TENDERMINT_TEST_SEED: &str = "tendermint test seed";
const TENDERMINT_CONSTANT_BALANCE_SEED: &str = "tendermint constant balance seed";

const ATOM_TENDERMINT_RPC_URLS: &[&str] = &["http://localhost:26658"];
const NUCLEUS_TESTNET_RPC_URLS: &[&str] = &["http://localhost:26657"];

const TENDERMINT_TEST_BIP39_SEED: &str =
    "emerge canoe salmon dolphin glow priority random become gasp sell blade argue";

lazy_static! {
    /// Makes sure that tests sending transactions run sequentially to prevent account sequence
    /// mismatches as some addresses are used in multiple tests.
    static ref SEQUENCE_LOCK: Mutex<()> = Mutex::new(());
}

#[test]
fn test_tendermint_balance() {
    let coins = json!([atom_testnet_conf()]);
    let coin = coins[0]["coin"].as_str().unwrap();
    let expected_address = "cosmos10tfc28dmn2m5qdrmg5ycjyqq7lyu7y8ledc8tc";

    let conf = Mm2TestConf::seednode(TENDERMINT_CONSTANT_BALANCE_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_result = block_on(enable_tendermint(&mm, coin, &[], ATOM_TENDERMINT_RPC_URLS, false));

    let result: RpcV2Response<TendermintActivationResult> = serde_json::from_value(activation_result).unwrap();
    assert_eq!(result.result.address, expected_address);
    let expected_balance: BigDecimal = "0.012345".parse().unwrap();
    assert_eq!(result.result.balance.unwrap().spendable, expected_balance);

    let my_balance = block_on(my_balance(&mm, coin));
    assert_eq!(my_balance.balance, expected_balance);
    assert_eq!(my_balance.unspendable_balance, BigDecimal::default());
    assert_eq!(my_balance.address, expected_address);
    assert_eq!(my_balance.coin, coin);
}

#[test]
fn test_tendermint_activation_without_balance() {
    let coins = json!([atom_testnet_conf()]);
    let coin = coins[0]["coin"].as_str().unwrap();
    let conf = Mm2TestConf::seednode(TENDERMINT_CONSTANT_BALANCE_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_result = block_on(enable_tendermint_without_balance(
        &mm,
        coin,
        &[],
        ATOM_TENDERMINT_RPC_URLS,
        false,
    ));

    let result: RpcV2Response<TendermintActivationResult> = serde_json::from_value(activation_result).unwrap();

    assert!(result.result.balance.is_none());
    assert!(result.result.tokens_balances.is_none());
    assert!(result.result.tokens_tickers.unwrap().is_empty());
}

#[test]
fn test_iris_ibc_nucleus_without_balance() {
    let coins = json!([nucleus_testnet_conf(), iris_ibc_nucleus_testnet_conf()]);

    let conf = Mm2TestConf::seednode(TENDERMINT_TEST_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();
    let platform_coin = coins[0]["coin"].as_str().unwrap();
    let token = coins[1]["coin"].as_str().unwrap();

    let activation_result = block_on(enable_tendermint_without_balance(
        &mm,
        platform_coin,
        &[token],
        NUCLEUS_TESTNET_RPC_URLS,
        false,
    ));

    let result: RpcV2Response<TendermintActivationResult> = serde_json::from_value(activation_result).unwrap();

    assert!(result.result.balance.is_none());
    assert!(result.result.tokens_balances.is_none());
    assert_eq!(
        result.result.tokens_tickers.unwrap(),
        HashSet::from_iter(vec![token.to_string()])
    );
}

#[test]
fn test_iris_ibc_nucleus_orderbook() {
    let coins = json!([nucleus_testnet_conf(), iris_ibc_nucleus_testnet_conf()]);
    let platform_coin = coins[0]["coin"].as_str().unwrap();
    let token = coins[1]["coin"].as_str().unwrap();

    let conf = Mm2TestConf::seednode(TENDERMINT_TEST_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_result = block_on(enable_tendermint(
        &mm,
        platform_coin,
        &[token],
        NUCLEUS_TESTNET_RPC_URLS,
        false,
    ));

    let response: RpcV2Response<TendermintActivationResult> = serde_json::from_value(activation_result).unwrap();

    let expected_address = "nuc150evuj4j7k9kgu38e453jdv9m3u0ft2n4fgzfr";
    assert_eq!(response.result.address, expected_address);

    let set_price_res = block_on(set_price(&mm, token, platform_coin, "1", "0.1", false, None));
    log!("{:?}", set_price_res);

    let set_price_res = block_on(set_price(&mm, platform_coin, token, "1", "0.1", false, None));
    log!("{:?}", set_price_res);

    let orderbook = block_on(orderbook(&mm, token, platform_coin));
    let orderbook: OrderbookResponse = serde_json::from_value(orderbook).unwrap();

    let first_ask = orderbook.asks.first().unwrap();
    assert_eq!(first_ask.entry.address, expected_address);

    let first_bid = orderbook.bids.first().unwrap();
    assert_eq!(first_bid.entry.address, expected_address);

    let orderbook_v2 = block_on(orderbook_v2(&mm, token, platform_coin));
    let orderbook_v2: RpcV2Response<OrderbookV2Response> = serde_json::from_value(orderbook_v2).unwrap();

    let expected_address = OrderbookAddress::Transparent(expected_address.into());
    let first_ask = orderbook_v2.result.asks.first().unwrap();
    assert_eq!(first_ask.entry.address, expected_address);

    let first_bid = orderbook_v2.result.bids.first().unwrap();
    assert_eq!(first_bid.entry.address, expected_address);
}

#[test]
fn test_tendermint_hd_address() {
    let coins = json!([atom_testnet_conf()]);
    let coin = coins[0]["coin"].as_str().unwrap();
    // Default address m/44'/118'/0'/0/0 when no path_to_address is specified in activation request
    let expected_address = "cosmos1nv4mqaky7n7rqjhch7829kgypx5s8fh62wdtr8";

    let conf = Mm2TestConf::seednode_with_hd_account(TENDERMINT_TEST_BIP39_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_result = block_on(enable_tendermint(&mm, coin, &[], ATOM_TENDERMINT_RPC_URLS, false));

    let result: RpcV2Response<TendermintActivationResult> = serde_json::from_value(activation_result).unwrap();
    assert_eq!(result.result.address, expected_address);
}

#[test]
fn test_tendermint_withdraw() {
    let _lock = SEQUENCE_LOCK.lock().unwrap();
    const MY_ADDRESS: &str = "cosmos150evuj4j7k9kgu38e453jdv9m3u0ft2n53flg6";

    let coins = json!([atom_testnet_conf()]);
    let coin = coins[0]["coin"].as_str().unwrap();

    let conf = Mm2TestConf::seednode(TENDERMINT_TEST_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_res = block_on(enable_tendermint(&mm, coin, &[], ATOM_TENDERMINT_RPC_URLS, false));
    log!("Activation {}", serde_json::to_string(&activation_res).unwrap());

    // just call withdraw without sending to check response correctness
    let tx_details = block_on(withdraw_v1(
        &mm,
        coin,
        "cosmos1svaw0aqc4584x825ju7ua03g5xtxwd0ahl86hz",
        "0.1",
        None,
    ));
    log!("Withdraw to other {}", serde_json::to_string(&tx_details).unwrap());
    // TODO how to check it if the fee is dynamic?
    /*
    let expected_total: BigDecimal = "0.15".parse().unwrap();
    assert_eq!(tx_details.total_amount, expected_total);
    assert_eq!(tx_details.spent_by_me, expected_total);
    assert_eq!(tx_details.my_balance_change, expected_total * BigDecimal::from(-1));
    */
    assert_eq!(tx_details.received_by_me, BigDecimal::default());
    assert_eq!(
        tx_details.to,
        vec!["cosmos1svaw0aqc4584x825ju7ua03g5xtxwd0ahl86hz".to_owned()]
    );
    assert_eq!(tx_details.from, vec![MY_ADDRESS.to_owned()]);

    // withdraw and send transaction to ourselves
    let tx_details = block_on(withdraw_v1(&mm, coin, MY_ADDRESS, "0.1", None));
    log!("Withdraw to self {}", serde_json::to_string(&tx_details).unwrap());

    // TODO how to check it if the fee is dynamic?
    /*
    let expected_total: BigDecimal = "0.15".parse().unwrap();
    let expected_balance_change: BigDecimal = "-0.05".parse().unwrap();
    assert_eq!(tx_details.total_amount, expected_total);
    assert_eq!(tx_details.spent_by_me, expected_total);
    assert_eq!(tx_details.my_balance_change, expected_balance_change);
     */
    let expected_received: BigDecimal = "0.1".parse().unwrap();
    assert_eq!(tx_details.received_by_me, expected_received);

    assert_eq!(tx_details.to, vec![MY_ADDRESS.to_owned()]);
    assert_eq!(tx_details.from, vec![MY_ADDRESS.to_owned()]);

    let send_raw_tx = block_on(send_raw_transaction(&mm, coin, &tx_details.tx_hex));
    log!("Send raw tx {}", serde_json::to_string(&send_raw_tx).unwrap());
}

#[test]
fn test_tendermint_withdraw_hd() {
    let _lock = SEQUENCE_LOCK.lock().unwrap();
    const MY_ADDRESS: &str = "cosmos134h9tv7866jcuw708w5w76lcfx7s3x2ysyalxy";

    let coins = json!([atom_testnet_conf()]);
    let coin = coins[0]["coin"].as_str().unwrap();

    let conf = Mm2TestConf::seednode_with_hd_account(TENDERMINT_TEST_BIP39_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_res = block_on(enable_tendermint(&mm, coin, &[], ATOM_TENDERMINT_RPC_URLS, false));
    log!(
        "Activation with assets {}",
        serde_json::to_string(&activation_res).unwrap()
    );

    // We will withdraw from HD account 0 and change 0 and address_index 1
    let path_to_address = HDAccountAddressId {
        account_id: 0,
        chain: Bip44Chain::External,
        address_id: 1,
    };

    // just call withdraw without sending to check response correctness
    let tx_details = block_on(withdraw_v1(
        &mm,
        coin,
        "cosmos1g3ufk7awmktp6kr2kgzfvlhm4ujzq3ekk9j3n3",
        "0.1",
        Some(path_to_address.clone()),
    ));
    log!("Withdraw to other {}", serde_json::to_string(&tx_details).unwrap());
    // TODO how to check it if the fee is dynamic?
    /*
    let expected_total: BigDecimal = "0.15".parse().unwrap();
    assert_eq!(tx_details.total_amount, expected_total);
    assert_eq!(tx_details.spent_by_me, expected_total);
    assert_eq!(tx_details.my_balance_change, expected_total * BigDecimal::from(-1));
    */
    assert_eq!(tx_details.received_by_me, BigDecimal::default());
    assert_eq!(
        tx_details.to,
        vec!["cosmos1g3ufk7awmktp6kr2kgzfvlhm4ujzq3ekk9j3n3".to_owned()]
    );
    assert_eq!(tx_details.from, vec![MY_ADDRESS.to_owned()]);

    // withdraw and send transaction to ourselves
    let tx_details = block_on(withdraw_v1(&mm, coin, MY_ADDRESS, "0.1", Some(path_to_address)));
    log!("Withdraw to self {}", serde_json::to_string(&tx_details).unwrap());

    // TODO how to check it if the fee is dynamic?
    /*
    let expected_total: BigDecimal = "0.15".parse().unwrap();
    let expected_balance_change: BigDecimal = "-0.05".parse().unwrap();
    assert_eq!(tx_details.total_amount, expected_total);
    assert_eq!(tx_details.spent_by_me, expected_total);
    assert_eq!(tx_details.my_balance_change, expected_balance_change);
     */
    let expected_received: BigDecimal = "0.1".parse().unwrap();
    assert_eq!(tx_details.received_by_me, expected_received);

    assert_eq!(tx_details.to, vec![MY_ADDRESS.to_owned()]);
    assert_eq!(tx_details.from, vec![MY_ADDRESS.to_owned()]);

    let send_raw_tx = block_on(send_raw_transaction(&mm, coin, &tx_details.tx_hex));
    log!("Send raw tx {}", serde_json::to_string(&send_raw_tx).unwrap());
}

#[test]
fn test_custom_gas_limit_on_tendermint_withdraw() {
    let coins = json!([atom_testnet_conf()]);
    let coin = coins[0]["coin"].as_str().unwrap();

    let conf = Mm2TestConf::seednode(TENDERMINT_TEST_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_res = block_on(enable_tendermint(&mm, coin, &[], ATOM_TENDERMINT_RPC_URLS, false));
    log!("Activation {}", serde_json::to_string(&activation_res).unwrap());

    let request = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "withdraw",
        "coin": coin,
        "to": "cosmos1w5h6wud7a8zpa539rc99ehgl9gwkad3wjsjq8v",
        "amount": "0.1",
        "fee": {
            "type": "CosmosGas",
            "gas_limit": 150000,
            "gas_price": 0.1
        }
    })))
    .unwrap();
    assert_eq!(request.0, common::StatusCode::OK, "'withdraw' failed: {}", request.1);
    let tx_details: TransactionDetails = serde_json::from_str(&request.1).unwrap();

    assert_eq!(tx_details.fee_details["gas_limit"], 150000);
}

#[test]
fn test_tendermint_ibc_withdraw() {
    let _lock = SEQUENCE_LOCK.lock().unwrap();
    // visit `{swagger_address}/ibc/core/channel/v1/channels?pagination.limit=10000` to see the full list of ibc channels
    const IBC_SOURCE_CHANNEL: u16 = 3;

    const IBC_TARGET_ADDRESS: &str = "cosmos1r5v5srda7xfth3hn2s26txvrcrntldjumt8mhl";
    const MY_ADDRESS: &str = "nuc150evuj4j7k9kgu38e453jdv9m3u0ft2n4fgzfr";

    let coins = json!([nucleus_testnet_conf()]);
    let platform_coin = coins[0]["coin"].as_str().unwrap();

    let conf = Mm2TestConf::seednode(TENDERMINT_TEST_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_res = block_on(enable_tendermint(
        &mm,
        platform_coin,
        &[],
        NUCLEUS_TESTNET_RPC_URLS,
        false,
    ));
    log!(
        "Activation with assets {}",
        serde_json::to_string(&activation_res).unwrap()
    );

    let tx_details = block_on(ibc_withdraw(
        &mm,
        IBC_SOURCE_CHANNEL,
        platform_coin,
        IBC_TARGET_ADDRESS,
        "0.1",
        None,
    ));
    log!(
        "IBC transfer to atom address {}",
        serde_json::to_string(&tx_details).unwrap()
    );

    assert_eq!(tx_details.to, vec![IBC_TARGET_ADDRESS.to_owned()]);
    assert_eq!(tx_details.from, vec![MY_ADDRESS.to_owned()]);

    let send_raw_tx = block_on(send_raw_transaction(&mm, platform_coin, &tx_details.tx_hex));
    log!("Send raw tx {}", serde_json::to_string(&send_raw_tx).unwrap());
}

#[test]
fn test_tendermint_ibc_withdraw_hd() {
    let _lock = SEQUENCE_LOCK.lock().unwrap();
    // visit `{swagger_address}/ibc/core/channel/v1/channels?pagination.limit=10000` to see the full list of ibc channels
    const IBC_SOURCE_CHANNEL: u16 = 3;

    const IBC_TARGET_ADDRESS: &str = "nuc150evuj4j7k9kgu38e453jdv9m3u0ft2n4fgzfr";
    const MY_ADDRESS: &str = "cosmos134h9tv7866jcuw708w5w76lcfx7s3x2ysyalxy";

    let coins = json!([atom_testnet_conf()]);
    let coin = coins[0]["coin"].as_str().unwrap();

    let conf = Mm2TestConf::seednode_with_hd_account(TENDERMINT_TEST_BIP39_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_res = block_on(enable_tendermint(&mm, coin, &[], ATOM_TENDERMINT_RPC_URLS, false));
    log!(
        "Activation with assets {}",
        serde_json::to_string(&activation_res).unwrap()
    );

    // We will withdraw from HD account 0 and change 0 and address_index 1
    let path_to_address = HDAccountAddressId {
        account_id: 0,
        chain: Bip44Chain::External,
        address_id: 1,
    };

    let tx_details = block_on(ibc_withdraw(
        &mm,
        IBC_SOURCE_CHANNEL,
        coin,
        IBC_TARGET_ADDRESS,
        "0.061",
        Some(path_to_address),
    ));
    log!(
        "IBC transfer to nucleus address {}",
        serde_json::to_string(&tx_details).unwrap()
    );

    assert_eq!(tx_details.to, vec![IBC_TARGET_ADDRESS.to_owned()]);
    assert_eq!(tx_details.from, vec![MY_ADDRESS.to_owned()]);

    let send_raw_tx = block_on(send_raw_transaction(&mm, coin, &tx_details.tx_hex));
    log!("Send raw tx {}", serde_json::to_string(&send_raw_tx).unwrap());
}

#[test]
fn test_tendermint_token_withdraw() {
    let _lock = SEQUENCE_LOCK.lock().unwrap();
    const MY_ADDRESS: &str = "nuc150evuj4j7k9kgu38e453jdv9m3u0ft2n4fgzfr";

    let coins = json!([nucleus_testnet_conf(), iris_ibc_nucleus_testnet_conf()]);
    let platform_coin = coins[0]["coin"].as_str().unwrap();
    let token = coins[1]["coin"].as_str().unwrap();

    let conf = Mm2TestConf::seednode(TENDERMINT_TEST_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_res = block_on(enable_tendermint(
        &mm,
        platform_coin,
        &[],
        NUCLEUS_TESTNET_RPC_URLS,
        false,
    ));
    log!(
        "Activation with assets {}",
        serde_json::to_string(&activation_res).unwrap()
    );

    let activation_res = block_on(enable_tendermint_token(&mm, token));
    log!("Token activation {}", serde_json::to_string(&activation_res).unwrap());

    // just call withdraw without sending to check response correctness
    let tx_details = block_on(withdraw_v1(
        &mm,
        token,
        "nuc1k2zmvy4kyxdfxv085kjlrygz2d78g78ew365gq",
        "0.1",
        None,
    ));

    log!("Withdraw to other {}", serde_json::to_string(&tx_details).unwrap());

    let expected_total: BigDecimal = "0.1".parse().unwrap();
    assert_eq!(tx_details.total_amount, expected_total);

    // TODO How to check it if the fee is dynamic?
    /*
    let expected_fee: BigDecimal = "0.05".parse().unwrap();
    let actual_fee: BigDecimal = tx_details.fee_details["amount"].as_str().unwrap().parse().unwrap();
    assert_eq!(actual_fee, expected_fee);
    */

    assert_eq!(tx_details.spent_by_me, expected_total);
    assert_eq!(tx_details.my_balance_change, expected_total * BigDecimal::from(-1));
    assert_eq!(tx_details.received_by_me, BigDecimal::default());
    assert_eq!(
        tx_details.to,
        vec!["nuc1k2zmvy4kyxdfxv085kjlrygz2d78g78ew365gq".to_owned()]
    );
    assert_eq!(tx_details.from, vec![MY_ADDRESS.to_owned()]);

    // withdraw and send transaction to ourselves
    let tx_details = block_on(withdraw_v1(&mm, token, MY_ADDRESS, "0.1", None));
    log!("Withdraw to self {}", serde_json::to_string(&tx_details).unwrap());

    let expected_total: BigDecimal = "0.1".parse().unwrap();
    let expected_received: BigDecimal = "0.1".parse().unwrap();

    assert_eq!(tx_details.total_amount, expected_total);

    // TODO How to check it if the fee is dynamic?
    /*
    let expected_fee: BigDecimal = "0.05".parse().unwrap();
    let actual_fee: BigDecimal = tx_details.fee_details["amount"].as_str().unwrap().parse().unwrap();
    assert_eq!(actual_fee, expected_fee);
    */

    assert_eq!(tx_details.spent_by_me, expected_total);
    assert_eq!(tx_details.received_by_me, expected_received);
    assert_eq!(tx_details.to, vec![MY_ADDRESS.to_owned()]);
    assert_eq!(tx_details.from, vec![MY_ADDRESS.to_owned()]);

    let send_raw_tx = block_on(send_raw_transaction(&mm, token, &tx_details.tx_hex));
    log!("Send raw tx {}", serde_json::to_string(&send_raw_tx).unwrap());
}

#[test]
fn test_tendermint_tx_history() {
    const TEST_SEED: &str = "Vdo8Xt8pTAetRlMq3kV0LzE393eVYbPSn5Mhtw4p";
    const TX_FINISHED_LOG: &str = "Tx history fetching finished for NUCLEUS-TEST.";
    const TX_HISTORY_PAGE_LIMIT: usize = 50;
    const NUCLEUS_EXPECTED_TX_COUNT: u64 = 9;
    const IRIS_IBC_EXPECTED_TX_COUNT: u64 = 1;

    let nucleus_constant_history_txs = include_str!("../../../mm2_test_helpers/dummy_files/nucleus-history.json");
    let nucleus_constant_history_txs: Vec<TransactionDetails> =
        serde_json::from_str(nucleus_constant_history_txs).unwrap();

    let iris_ibc_constant_history_txs =
        include_str!("../../../mm2_test_helpers/dummy_files/iris-ibc-nucleus-history.json");
    let iris_ibc_constant_history_txs: Vec<TransactionDetails> =
        serde_json::from_str(iris_ibc_constant_history_txs).unwrap();

    let coins = json!([nucleus_testnet_conf(), iris_ibc_nucleus_testnet_conf()]);
    let platform_coin = coins[0]["coin"].as_str().unwrap();
    let token = coins[1]["coin"].as_str().unwrap();

    let conf = Mm2TestConf::seednode(TEST_SEED, &coins);
    let mut mm = block_on(MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)).unwrap();

    block_on(enable_tendermint(
        &mm,
        platform_coin,
        &[token],
        NUCLEUS_TESTNET_RPC_URLS,
        true,
    ));

    if block_on(mm.wait_for_log(60., |log| log.contains(TX_FINISHED_LOG))).is_err() {
        log!("{}", mm.log_as_utf8().unwrap());
        panic!("Tx history didn't finish which is not expected");
    }

    // testing NUCLEUS-TEST history
    let nucleus_history_response = block_on(get_tendermint_my_tx_history(
        &mm,
        platform_coin,
        TX_HISTORY_PAGE_LIMIT,
        1,
    ));
    let total_txs = nucleus_history_response["result"]["total"].as_u64().unwrap();
    assert_eq!(total_txs, NUCLEUS_EXPECTED_TX_COUNT);

    let mut nucleus_txs_from_request = nucleus_history_response["result"]["transactions"].clone();
    for i in 0..NUCLEUS_EXPECTED_TX_COUNT {
        nucleus_txs_from_request[i as usize]
            .as_object_mut()
            .unwrap()
            .remove("confirmations");
    }
    let nucleus_txs_from_request: Vec<TransactionDetails> = serde_json::from_value(nucleus_txs_from_request).unwrap();
    assert_eq!(nucleus_constant_history_txs, nucleus_txs_from_request);

    // testing IRIS-IBC-NUCLEUS-TEST history
    let iris_ibc_tx_history_response = block_on(get_tendermint_my_tx_history(&mm, token, TX_HISTORY_PAGE_LIMIT, 1));
    let total_txs = iris_ibc_tx_history_response["result"]["total"].as_u64().unwrap();
    assert_eq!(total_txs, IRIS_IBC_EXPECTED_TX_COUNT);

    let mut iris_ibc_txs_from_request = iris_ibc_tx_history_response["result"]["transactions"].clone();
    for i in 0..IRIS_IBC_EXPECTED_TX_COUNT {
        iris_ibc_txs_from_request[i as usize]
            .as_object_mut()
            .unwrap()
            .remove("confirmations");
    }
    let iris_ibc_txs_from_request: Vec<TransactionDetails> = serde_json::from_value(iris_ibc_txs_from_request).unwrap();

    assert_eq!(iris_ibc_constant_history_txs, iris_ibc_txs_from_request);

    block_on(mm.stop()).unwrap();
}

#[test]
fn test_disable_tendermint_platform_coin_with_token() {
    let coins = json!([nucleus_testnet_conf(), iris_ibc_nucleus_testnet_conf()]);
    let platform_coin = coins[0]["coin"].as_str().unwrap();
    let token = coins[1]["coin"].as_str().unwrap();

    let conf = Mm2TestConf::seednode(TENDERMINT_TEST_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();
    // Enable platform coin NUCLEUS-TEST
    let activation_res = block_on(enable_tendermint(
        &mm,
        platform_coin,
        &[],
        NUCLEUS_TESTNET_RPC_URLS,
        false,
    ));
    assert!(&activation_res.get("result").unwrap().get("address").is_some());

    // Enable platform coin token IRIS-IBC-NUCLEUS-TEST
    let activation_res = block_on(enable_tendermint_token(&mm, token));
    assert!(&activation_res.get("result").unwrap().get("balances").is_some());

    // Try to passive platform coin
    let res = block_on(disable_coin(&mm, platform_coin, false));
    assert!(res.passivized);

    // Try to disable token when platform coin is passived.
    // This should work, because platform coin is still in the memory.
    let res = block_on(disable_coin(&mm, token, false));
    assert!(!res.passivized);

    // Then try to force disable platform coin.
    let res = block_on(disable_coin(&mm, platform_coin, true));
    assert!(!res.passivized);
}

#[test]
fn test_passive_coin_and_force_disable() {
    let coins = json!([nucleus_testnet_conf(), iris_ibc_nucleus_testnet_conf()]);
    let platform_coin = coins[0]["coin"].as_str().unwrap();
    let token = coins[1]["coin"].as_str().unwrap();

    let conf = Mm2TestConf::seednode(TENDERMINT_TEST_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    // Enable platform coin NUCLEUS-TEST
    let activation_res = block_on(enable_tendermint(
        &mm,
        platform_coin,
        &[],
        NUCLEUS_TESTNET_RPC_URLS,
        false,
    ));
    assert!(&activation_res.get("result").unwrap().get("address").is_some());

    // Enable platform coin token IRIS-IBC-NUCLEUS-TEST
    let activation_res = block_on(enable_tendermint_token(&mm, token));
    assert!(&activation_res.get("result").unwrap().get("balances").is_some());

    // Try to passive platform coin
    let res = block_on(disable_coin(&mm, platform_coin, false));
    assert!(res.passivized);

    // Try to disable token when platform coin is passived.
    // This should work, because platform coin is still in the memory.
    let res = block_on(disable_coin(&mm, token, false));
    assert!(!res.passivized);

    // Re-activate passive coin
    let activation_res = block_on(enable_tendermint(
        &mm,
        platform_coin,
        &[],
        NUCLEUS_TESTNET_RPC_URLS,
        false,
    ));
    assert!(&activation_res.get("result").unwrap().get("address").is_some());

    // Enable platform coin token
    let activation_res = block_on(enable_tendermint_token(&mm, token));
    assert!(&activation_res.get("result").unwrap().get("balances").is_some());

    // Try to force disable platform coin
    let res = block_on(disable_coin(&mm, platform_coin, true));
    assert!(!res.passivized);

    // Try to disable token when platform coin force disabled.
    // This should failed, because platform coin was purged with its tokens.
    block_on(disable_coin_err(&mm, token, false));
}

#[test]
fn test_tendermint_validators_rpc() {
    let coins = json!([nucleus_testnet_conf()]);
    let platform_coin = coins[0]["coin"].as_str().unwrap();

    let conf = Mm2TestConf::seednode(TENDERMINT_TEST_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_res = block_on(enable_tendermint(
        &mm,
        platform_coin,
        &[],
        NUCLEUS_TESTNET_RPC_URLS,
        false,
    ));
    assert!(&activation_res.get("result").unwrap().get("address").is_some());

    let validators_raw_response = block_on(tendermint_validators(&mm, platform_coin, "All", 10, 1));

    assert_eq!(
        validators_raw_response["result"]["validators"][0]["operator_address"],
        "nucvaloper15d4sf4z6y0vk9dnum8yzkvr9c3wq4q897vefpu"
    );
    assert_eq!(validators_raw_response["result"]["validators"][0]["jailed"], false);
}

#[test]
fn test_tendermint_add_delegation() {
    let _lock = SEQUENCE_LOCK.lock().unwrap();
    const MY_ADDRESS: &str = "nuc150evuj4j7k9kgu38e453jdv9m3u0ft2n4fgzfr";
    const VALIDATOR_ADDRESS: &str = "nucvaloper15d4sf4z6y0vk9dnum8yzkvr9c3wq4q897vefpu";

    let coins = json!([nucleus_testnet_conf()]);
    let coin_ticker = coins[0]["coin"].as_str().unwrap();

    let conf = Mm2TestConf::seednode(TENDERMINT_TEST_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_res = block_on(enable_tendermint(
        &mm,
        coin_ticker,
        &[],
        NUCLEUS_TESTNET_RPC_URLS,
        false,
    ));

    log!(
        "Activation with assets {}",
        serde_json::to_string(&activation_res).unwrap()
    );

    let tx_details = block_on(tendermint_add_delegation(&mm, coin_ticker, VALIDATOR_ADDRESS, "0.5"));

    assert_eq!(tx_details.to, vec![VALIDATOR_ADDRESS.to_owned()]);
    assert_eq!(tx_details.from, vec![MY_ADDRESS.to_owned()]);
    assert_eq!(tx_details.transaction_type, TransactionType::StakingDelegation);

    let send_raw_tx = block_on(send_raw_transaction(&mm, coin_ticker, &tx_details.tx_hex));
    log!("Send raw tx {}", serde_json::to_string(&send_raw_tx).unwrap());
}

#[test]
fn test_tendermint_remove_delegation() {
    let _lock = SEQUENCE_LOCK.lock().unwrap();
    const MY_ADDRESS: &str = "nuc150evuj4j7k9kgu38e453jdv9m3u0ft2n4fgzfr";
    const VALIDATOR_ADDRESS: &str = "nucvaloper15d4sf4z6y0vk9dnum8yzkvr9c3wq4q897vefpu";

    let coins = json!([nucleus_testnet_conf()]);
    let coin_ticker = coins[0]["coin"].as_str().unwrap();

    let conf = Mm2TestConf::seednode(TENDERMINT_TEST_SEED, &coins);
    let mm = MarketMakerIt::start(conf.conf, conf.rpc_password, None).unwrap();

    let activation_res = block_on(enable_tendermint(
        &mm,
        coin_ticker,
        &[],
        NUCLEUS_TESTNET_RPC_URLS,
        false,
    ));

    log!(
        "Activation with assets {}",
        serde_json::to_string(&activation_res).unwrap()
    );

    let tx_details = block_on(tendermint_add_delegation(&mm, coin_ticker, VALIDATOR_ADDRESS, "0.5"));

    assert_eq!(tx_details.to, vec![VALIDATOR_ADDRESS.to_owned()]);
    assert_eq!(tx_details.from, vec![MY_ADDRESS.to_owned()]);
    assert_eq!(tx_details.transaction_type, TransactionType::StakingDelegation);

    let send_raw_tx = block_on(send_raw_transaction(&mm, coin_ticker, &tx_details.tx_hex));
    log!("Send raw tx {}", serde_json::to_string(&send_raw_tx).unwrap());

    thread::sleep(Duration::from_secs(1));

    let r = block_on(tendermint_delegations(&mm, coin_ticker));
    let delegation_info = r["result"]["delegations"].as_array().unwrap().last().unwrap();
    assert_eq!(delegation_info["validator_address"], VALIDATOR_ADDRESS);

    // Try to undelegate more than the total delegated amount
    let raw_response = block_on(tendermint_remove_delegation_raw(
        &mm,
        coin_ticker,
        VALIDATOR_ADDRESS,
        "3.4",
    ));
    assert_eq!(raw_response.0, http::StatusCode::BAD_REQUEST);

    // Track this type here to enforce compiler to help us to update this test coverage
    // whenever this type is removed/renamed.
    let _ = coins::DelegationError::TooMuchToUndelegate {
        available: BigDecimal::default(),
        requested: BigDecimal::default(),
    };
    assert!(raw_response.1.contains("TooMuchToUndelegate"));

    let tx_details = block_on(tendermint_remove_delegation(
        &mm,
        coin_ticker,
        VALIDATOR_ADDRESS,
        "0.15",
    ));

    assert_eq!(tx_details.from, vec![MY_ADDRESS.to_owned()]);
    assert!(tx_details.to.is_empty());
    assert_eq!(tx_details.transaction_type, TransactionType::RemoveDelegation);

    let send_raw_tx = block_on(send_raw_transaction(&mm, coin_ticker, &tx_details.tx_hex));
    log!("Send raw tx {}", serde_json::to_string(&send_raw_tx).unwrap());

    thread::sleep(Duration::from_secs(1));

    let r = block_on(tendermint_ongoing_undelegations(&mm, coin_ticker));
    let undelegation_info = r["result"]["ongoing_undelegations"].as_array().unwrap().last().unwrap();
    assert_eq!(undelegation_info["validator_address"], VALIDATOR_ADDRESS);
    let undelegation_entry = undelegation_info["entries"].as_array().unwrap().last().unwrap();
    assert_eq!(undelegation_entry["balance"], "0.15");
}

mod swap {
    use super::*;

    use crate::docker_tests::eth_docker_tests::fill_eth;
    use crate::docker_tests::eth_docker_tests::swap_contract;
    use crate::integration_tests_common::enable_electrum;
    use common::executor::Timer;
    use common::log;
    use compatible_time::Duration;
    use ethereum_types::{Address, U256};
    use mm2_rpc::data::legacy::OrderbookResponse;
    use mm2_test_helpers::for_tests::{
        check_my_swap_status, check_recent_swaps, doc_conf, enable_eth_coin, iris_ibc_nucleus_testnet_conf,
        nucleus_testnet_conf, wait_check_stats_swap_status, DOC_ELECTRUM_ADDRS,
    };
    use std::convert::TryFrom;
    use std::env;
    use std::str::FromStr;

    const BOB_PASSPHRASE: &str = "iris test seed";
    const ALICE_PASSPHRASE: &str = "iris test2 seed";

    #[test]
    fn swap_nucleus_with_doc() {
        let _lock = SEQUENCE_LOCK.lock().unwrap();
        let bob_passphrase = String::from(BOB_PASSPHRASE);
        let alice_passphrase = String::from(ALICE_PASSPHRASE);

        let coins = json!([nucleus_testnet_conf(), doc_conf()]);

        let mm_bob = MarketMakerIt::start(
            json!({
                "gui": "nogui",
                "netid": 8999,
                "dht": "on",
                "myipaddr": env::var("BOB_TRADE_IP") .ok(),
                "rpcip": env::var("BOB_TRADE_IP") .ok(),
                "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
                "passphrase": bob_passphrase,
                "coins": coins,
                "rpc_password": "password",
                "i_am_seed": true,
                "is_bootstrap_node": true
            }),
            "password".into(),
            None,
        )
        .unwrap();

        thread::sleep(Duration::from_secs(1));

        let mm_alice = MarketMakerIt::start(
            json!({
                "gui": "nogui",
                "netid": 8999,
                "dht": "on",
                "myipaddr": env::var("ALICE_TRADE_IP") .ok(),
                "rpcip": env::var("ALICE_TRADE_IP") .ok(),
                "passphrase": alice_passphrase,
                "coins": coins,
                "seednodes": [mm_bob.my_seed_addr()],
                "rpc_password": "password",
                "skip_startup_checks": true,
            }),
            "password".into(),
            None,
        )
        .unwrap();

        thread::sleep(Duration::from_secs(1));

        dbg!(block_on(enable_tendermint(
            &mm_bob,
            "NUCLEUS-TEST",
            &[],
            NUCLEUS_TESTNET_RPC_URLS,
            false
        )));

        dbg!(block_on(enable_tendermint(
            &mm_alice,
            "NUCLEUS-TEST",
            &[],
            NUCLEUS_TESTNET_RPC_URLS,
            false
        )));

        dbg!(block_on(enable_electrum(&mm_bob, "DOC", false, DOC_ELECTRUM_ADDRS,)));

        dbg!(block_on(enable_electrum(&mm_alice, "DOC", false, DOC_ELECTRUM_ADDRS,)));

        block_on(trade_base_rel_tendermint(
            mm_bob,
            mm_alice,
            "NUCLEUS-TEST",
            "DOC",
            1,
            2,
            0.008,
        ));
    }

    #[test]
    fn swap_nucleus_with_eth() {
        let _lock = SEQUENCE_LOCK.lock().unwrap();
        let bob_passphrase = String::from(BOB_PASSPHRASE);
        let alice_passphrase = String::from(ALICE_PASSPHRASE);
        const BOB_ETH_ADDRESS: &str = "0x7b338250f990954E3Ab034ccD32a917c2F607C2d";
        const ALICE_ETH_ADDRESS: &str = "0x37602b7a648b207ACFD19E67253f57669bEA4Ad8";

        fill_eth(
            Address::from_str(BOB_ETH_ADDRESS).unwrap(),
            U256::from(10).pow(U256::from(20)),
        );

        fill_eth(
            Address::from_str(ALICE_ETH_ADDRESS).unwrap(),
            U256::from(10).pow(U256::from(20)),
        );

        let coins = json!([nucleus_testnet_conf(), crate::eth_dev_conf()]);

        let mm_bob = MarketMakerIt::start(
            json!({
                "gui": "nogui",
                "netid": 8999,
                "dht": "on",
                "myipaddr": env::var("BOB_TRADE_IP") .ok(),
                "rpcip": env::var("BOB_TRADE_IP") .ok(),
                "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
                "passphrase": bob_passphrase,
                "coins": coins,
                "rpc_password": "password",
                "i_am_seed": true,
                "is_bootstrap_node": true
            }),
            "password".into(),
            None,
        )
        .unwrap();

        thread::sleep(Duration::from_secs(1));

        let mm_alice = MarketMakerIt::start(
            json!({
                "gui": "nogui",
                "netid": 8999,
                "dht": "on",
                "myipaddr": env::var("ALICE_TRADE_IP") .ok(),
                "rpcip": env::var("ALICE_TRADE_IP") .ok(),
                "passphrase": alice_passphrase,
                "coins": coins,
                "seednodes": [mm_bob.my_seed_addr()],
                "rpc_password": "password",
                "skip_startup_checks": true,
            }),
            "password".into(),
            None,
        )
        .unwrap();

        thread::sleep(Duration::from_secs(1));

        dbg!(block_on(enable_tendermint(
            &mm_bob,
            "NUCLEUS-TEST",
            &[],
            NUCLEUS_TESTNET_RPC_URLS,
            false
        )));

        dbg!(block_on(enable_tendermint(
            &mm_alice,
            "NUCLEUS-TEST",
            &[],
            NUCLEUS_TESTNET_RPC_URLS,
            false
        )));

        let swap_contract = format!("0x{}", hex::encode(swap_contract()));

        dbg!(block_on(enable_eth_coin(
            &mm_bob,
            "ETH",
            &[crate::GETH_RPC_URL],
            &swap_contract,
            None,
            false
        )));

        dbg!(block_on(enable_eth_coin(
            &mm_alice,
            "ETH",
            &[crate::GETH_RPC_URL],
            &swap_contract,
            None,
            false
        )));

        block_on(trade_base_rel_tendermint(
            mm_bob,
            mm_alice,
            "NUCLEUS-TEST",
            "ETH",
            1,
            2,
            0.008,
        ));
    }

    #[test]
    fn swap_doc_with_iris_ibc_nucleus() {
        let _lock = SEQUENCE_LOCK.lock().unwrap();
        let bob_passphrase = String::from(BOB_PASSPHRASE);
        let alice_passphrase = String::from(ALICE_PASSPHRASE);

        let coins = json!([nucleus_testnet_conf(), iris_ibc_nucleus_testnet_conf(), doc_conf()]);

        let mm_bob = MarketMakerIt::start(
            json!({
                "gui": "nogui",
                "netid": 8999,
                "dht": "on",
                "myipaddr": env::var("BOB_TRADE_IP") .ok(),
                "rpcip": env::var("BOB_TRADE_IP") .ok(),
                "canbind": env::var("BOB_TRADE_PORT") .ok().map (|s| s.parse::<i64>().unwrap()),
                "passphrase": bob_passphrase,
                "coins": coins,
                "rpc_password": "password",
                "i_am_seed": true,
                "is_bootstrap_node": true
            }),
            "password".into(),
            None,
        )
        .unwrap();

        thread::sleep(Duration::from_secs(1));

        let mm_alice = MarketMakerIt::start(
            json!({
                "gui": "nogui",
                "netid": 8999,
                "dht": "on",
                "myipaddr": env::var("ALICE_TRADE_IP") .ok(),
                "rpcip": env::var("ALICE_TRADE_IP") .ok(),
                "passphrase": alice_passphrase,
                "coins": coins,
                "seednodes": [mm_bob.my_seed_addr()],
                "rpc_password": "password",
                "skip_startup_checks": true,
            }),
            "password".into(),
            None,
        )
        .unwrap();

        thread::sleep(Duration::from_secs(1));

        dbg!(block_on(enable_tendermint(
            &mm_bob,
            "NUCLEUS-TEST",
            &["IRIS-IBC-NUCLEUS-TEST"],
            NUCLEUS_TESTNET_RPC_URLS,
            false
        )));

        dbg!(block_on(enable_tendermint(
            &mm_alice,
            "NUCLEUS-TEST",
            &["IRIS-IBC-NUCLEUS-TEST"],
            NUCLEUS_TESTNET_RPC_URLS,
            false
        )));

        dbg!(block_on(enable_electrum(&mm_bob, "DOC", false, DOC_ELECTRUM_ADDRS)));

        dbg!(block_on(enable_electrum(&mm_alice, "DOC", false, DOC_ELECTRUM_ADDRS)));

        block_on(trade_base_rel_tendermint(
            mm_bob,
            mm_alice,
            "DOC",
            "IRIS-IBC-NUCLEUS-TEST",
            1,
            2,
            0.008,
        ));
    }

    pub async fn trade_base_rel_tendermint(
        mut mm_bob: MarketMakerIt,
        mut mm_alice: MarketMakerIt,
        base: &str,
        rel: &str,
        maker_price: i32,
        taker_price: i32,
        volume: f64,
    ) {
        log!("Issue bob {}/{} sell request", base, rel);
        let rc = mm_bob
            .rpc(&json!({
                "userpass": mm_bob.userpass,
                "method": "setprice",
                "base": base,
                "rel": rel,
                "price": maker_price,
                "volume": volume
            }))
            .await
            .unwrap();
        assert!(rc.0.is_success(), "!setprice: {}", rc.1);

        let mut uuids = vec![];

        common::log::info!(
            "Trigger alice subscription to {}/{} orderbook topic first and sleep for 1 second",
            base,
            rel
        );
        let rc = mm_alice
            .rpc(&json!({
                "userpass": mm_alice.userpass,
                "method": "orderbook",
                "base": base,
                "rel": rel,
            }))
            .await
            .unwrap();
        assert!(rc.0.is_success(), "!orderbook: {}", rc.1);
        Timer::sleep(1.).await;
        common::log::info!("Issue alice {}/{} buy request", base, rel);
        let rc = mm_alice
            .rpc(&json!({
                "userpass": mm_alice.userpass,
                "method": "buy",
                "base": base,
                "rel": rel,
                "volume": volume,
                "price": taker_price
            }))
            .await
            .unwrap();
        assert!(rc.0.is_success(), "!buy: {}", rc.1);
        let buy_json: serde_json::Value = serde_json::from_str(&rc.1).unwrap();
        uuids.push(buy_json["result"]["uuid"].as_str().unwrap().to_owned());

        // ensure the swaps are started
        let expected_log = format!("Entering the taker_swap_loop {base}/{rel}");
        mm_alice
            .wait_for_log(5., |log| log.contains(&expected_log))
            .await
            .unwrap();
        let expected_log = format!("Entering the maker_swap_loop {base}/{rel}");
        mm_bob
            .wait_for_log(5., |log| log.contains(&expected_log))
            .await
            .unwrap();

        for uuid in uuids.iter() {
            // ensure the swaps are indexed to the SQLite database
            let expected_log = format!("Inserting new swap {uuid} to the SQLite database");
            mm_alice
                .wait_for_log(5., |log| log.contains(&expected_log))
                .await
                .unwrap();
            mm_bob
                .wait_for_log(5., |log| log.contains(&expected_log))
                .await
                .unwrap()
        }

        for uuid in uuids.iter() {
            match mm_bob
                .wait_for_log(180., |log| log.contains(&format!("[swap uuid={uuid}] Finished")))
                .await
            {
                Ok(_) => (),
                Err(_) => {
                    log!("{}", mm_bob.log_as_utf8().unwrap());
                },
            }

            match mm_alice
                .wait_for_log(180., |log| log.contains(&format!("[swap uuid={uuid}] Finished")))
                .await
            {
                Ok(_) => (),
                Err(_) => {
                    log!("{}", mm_alice.log_as_utf8().unwrap());
                },
            }

            log!("Waiting a few second for the fresh swap status to be saved..");
            Timer::sleep(5.).await;

            log!("{}", mm_alice.log_as_utf8().unwrap());
            log!("Checking alice/taker status..");
            check_my_swap_status(
                &mm_alice,
                uuid,
                BigDecimal::try_from(volume).unwrap(),
                BigDecimal::try_from(volume).unwrap(),
            )
            .await;

            log!("{}", mm_bob.log_as_utf8().unwrap());
            log!("Checking bob/maker status..");
            check_my_swap_status(
                &mm_bob,
                uuid,
                BigDecimal::try_from(volume).unwrap(),
                BigDecimal::try_from(volume).unwrap(),
            )
            .await;
        }

        log!("Waiting 3 seconds for nodes to broadcast their swaps data..");
        Timer::sleep(3.).await;

        for uuid in uuids.iter() {
            log!("Checking alice status..");
            wait_check_stats_swap_status(&mm_alice, uuid, 240).await;

            log!("Checking bob status..");
            wait_check_stats_swap_status(&mm_bob, uuid, 240).await;
        }

        log!("Checking alice recent swaps..");
        check_recent_swaps(&mm_alice, uuids.len()).await;
        log!("Checking bob recent swaps..");
        check_recent_swaps(&mm_bob, uuids.len()).await;
        log!("Get {}/{} orderbook", base, rel);
        let rc = mm_bob
            .rpc(&json!({
                "userpass": mm_bob.userpass,
                "method": "orderbook",
                "base": base,
                "rel": rel,
            }))
            .await
            .unwrap();
        assert!(rc.0.is_success(), "!orderbook: {}", rc.1);

        let bob_orderbook: OrderbookResponse = serde_json::from_str(&rc.1).unwrap();
        log!("{}/{} orderbook {:?}", base, rel, bob_orderbook);

        assert_eq!(0, bob_orderbook.bids.len(), "{base} {rel} bids must be empty");
        assert_eq!(0, bob_orderbook.asks.len(), "{base} {rel} asks must be empty");

        mm_bob.stop().await.unwrap();
        mm_alice.stop().await.unwrap();
    }
}
