use crate::integration_tests_common::{enable_coins_rick_morty_electrum, enable_electrum};
use coins::lightning::ln_events::{
    CHANNEL_READY_LOG, PAYMENT_CLAIMABLE_LOG, SUCCESSFUL_CLAIM_LOG, SUCCESSFUL_SEND_LOG,
};
use common::executor::Timer;
use common::{block_on, log, wait_until_ms};
use gstuff::now_ms;
use http::StatusCode;
use mm2_number::BigDecimal;
use mm2_test_helpers::for_tests::{
    disable_coin, init_lightning, init_lightning_status, my_balance, sign_message, start_swaps, verify_message,
    wait_for_swaps_finish_and_check_status, MarketMakerIt,
};
use mm2_test_helpers::structs::{
    InitLightningStatus, InitTaskResult, LightningActivationResult, RpcV2Response, SignatureResponse,
    VerificationResponse,
};
use serde_json::{self as json, json, Value as Json};
use std::env;
use std::str::FromStr;

const BTC_AVG_BLOCKTIME: u64 = 600;
const T_BTC_ELECTRUMS: &[&str] = &[
    "electrum1.cipig.net:10068",
    "electrum2.cipig.net:10068",
    "electrum3.cipig.net:10068",
];

async fn enable_lightning(mm: &MarketMakerIt, coin: &str, timeout: u64) -> LightningActivationResult {
    let init = init_lightning(mm, coin).await;
    let init: RpcV2Response<InitTaskResult> = json::from_value(init).unwrap();
    let timeout = wait_until_ms(timeout * 1000);

    loop {
        if now_ms() > timeout {
            panic!("{} initialization timed out", coin);
        }

        let status = init_lightning_status(mm, init.result.task_id).await;
        let status: RpcV2Response<InitLightningStatus> = json::from_value(status).unwrap();
        log!("init_lightning_status: {:?}", status);
        match status.result {
            InitLightningStatus::Ok(result) => break result,
            InitLightningStatus::Error(e) => panic!("{} initialization error {:?}", coin, e),
            _ => Timer::sleep(1.).await,
        }
    }
}

fn start_lightning_nodes(enable_0_confs: bool) -> (MarketMakerIt, MarketMakerIt, String, String) {
    let node_1_seed = "become nominee mountain person volume business diet zone govern voice debris hidden";
    let node_2_seed = "february coast tortoise grab shadow vast volcano affair ordinary gesture brass oxygen";

    let coins = json!([
        {
            "coin": "tBTC-TEST-segwit",
            "name": "tbitcoin",
            "fname": "tBitcoin",
            "rpcport": 18332,
            "pubtype": 111,
            "p2shtype": 196,
            "wiftype": 239,
            "segwit": true,
            "bech32_hrp": "tb",
            "address_format":{"format":"segwit"},
            "orderbook_ticker": "tBTC-TEST",
            "txfee": 0,
            "estimate_fee_mode": "ECONOMICAL",
            "mm2": 1,
            "required_confirmations": 0,
            "avg_blocktime": BTC_AVG_BLOCKTIME,
            "protocol": {
              "type": "UTXO"
            }
          },
          {
            "coin": "tBTC-TEST-lightning",
            "mm2": 1,
            "decimals": 11,
            "our_channels_configs": {
              "inbound_channels_confirmations": 1,
              // todo: When this was 100% I got "lightning:channelmanager:2525] ERROR Cannot send value that would put our balance under counterparty-announced channel reserve value (1000000)"
              // todo: This seems to be a bug in rust-lightning for mpp, I informed their team and will revert this to 100 if it was fixed https://github.com/lightningdevkit/rust-lightning/issues/1126#issuecomment-1414308252
              "max_inbound_in_flight_htlc_percent": 90,
              // If this is set to 0 it will default to 1000 sats since it's the min allowed value
              "their_channel_reserve_sats": 1000,
            },
            "counterparty_channel_config_limits": {
              "outbound_channels_confirmations": 1,
              // If true, this enables sending payments between the 2 nodes straight away without waiting for on-chain confirmations
              // if the other node added this node as trusted. It also overrides "outbound_channels_confirmations".
              "allow_outbound_0conf": enable_0_confs
            },
            "protocol": {
              "type": "LIGHTNING",
              "protocol_data":{
                "platform": "tBTC-TEST-segwit",
                "network": "testnet",
                "confirmation_targets": {
                  "background": 12,
                  "normal": 6,
                  "high_priority": 1
                }
              }
            }
          },
        {"coin":"RICK","asset":"RICK","rpcport":8923,"txversion":4,"overwintered":1,"required_confirmations":0,"protocol":{"type":"UTXO"}},
        {"coin":"MORTY","asset":"MORTY","rpcport":11608,"txversion":4,"overwintered":1,"required_confirmations":0,"protocol":{"type":"UTXO"}}
    ]);

    let mm_node_1 = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var ("BOB_TRADE_IP") .ok(),
            "rpcip": env::var ("BOB_TRADE_IP") .ok(),
            "passphrase": node_1_seed.to_string(),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".into(),
        None,
    )
    .unwrap();
    let (_dump_log, _dump_dashboard) = mm_node_1.mm_dump();
    log!("Node 1 log path: {}", mm_node_1.log_path.display());

    let electrum = block_on(enable_electrum(&mm_node_1, "tBTC-TEST-segwit", false, T_BTC_ELECTRUMS));
    log!("Node 1 tBTC address: {}", electrum.address);

    let enable_lightning_1 = block_on(enable_lightning(&mm_node_1, "tBTC-TEST-lightning", 600));
    let node_1_address = enable_lightning_1.address;

    let mm_node_2 = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var ("ALICE_TRADE_IP") .ok(),
            "rpcip": env::var ("ALICE_TRADE_IP") .ok(),
            "passphrase": node_2_seed.to_string(),
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": [mm_node_1.my_seed_addr()],
        }),
        "pass".into(),
        None,
    )
    .unwrap();
    let (_dump_log, _dump_dashboard) = mm_node_2.mm_dump();
    log!("Node 2 log path: {}", mm_node_2.log_path.display());

    let electrum = block_on(enable_electrum(&mm_node_2, "tBTC-TEST-segwit", false, T_BTC_ELECTRUMS));
    log!("Node 2 tBTC address: {}", electrum.address);

    let enable_lightning_2 = block_on(enable_lightning(&mm_node_2, "tBTC-TEST-lightning", 600));
    let node_2_address = enable_lightning_2.address;

    (mm_node_1, mm_node_2, node_1_address, node_2_address)
}

async fn open_channel(
    mm: &mut MarketMakerIt,
    coin: &str,
    address: &str,
    amount: f64,
    wait_for_ready_signal: bool,
) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "lightning::channels::open_channel",
            "params": {
                "coin": coin,
                "node_address": address,
                "amount": {
                    "type": "Exact",
                    "value": amount,
                },
            },
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'lightning::channels::open_channel' failed: {}",
        request.1
    );

    let res: Json = json::from_str(&request.1).unwrap();
    let uuid = res["result"]["uuid"].as_str().unwrap();
    if wait_for_ready_signal {
        mm.wait_for_log(3600., |log| log.contains(&format!("{CHANNEL_READY_LOG}: {uuid}")))
            .await
            .unwrap();
    }
    res
}

async fn close_channel(mm: &MarketMakerIt, uuid: &str, force_close: bool) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "lightning::channels::close_channel",
            "params": {
                "coin": "tBTC-TEST-lightning",
                "uuid": uuid,
                "force_close": force_close,
            },
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'lightning::channels::close_channel' failed: {}",
        request.1
    );

    json::from_str(&request.1).unwrap()
}

async fn add_trusted_node(mm: &MarketMakerIt, node_id: &str) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "lightning::nodes::add_trusted_node",
            "params": {
                "coin": "tBTC-TEST-lightning",
                "node_id": node_id
            },
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'lightning::nodes::add_trusted_node' failed: {}",
        request.1
    );
    json::from_str(&request.1).unwrap()
}

async fn generate_invoice(mm: &MarketMakerIt, amount_in_msat: u64) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "lightning::payments::generate_invoice",
            "params": {
                "coin": "tBTC-TEST-lightning",
                "description": "test invoice",
                "amount_in_msat": amount_in_msat
            },
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'lightning::payments::generate_invoice' failed: {}",
        request.1
    );

    json::from_str(&request.1).unwrap()
}

async fn pay_invoice(mm: &MarketMakerIt, invoice: &str) -> Json {
    let request = mm
        .rpc(&json!({
            "userpass": mm.userpass,
            "method": "lightning::payments::send_payment",
            "params": {
                "coin": "tBTC-TEST-lightning",
                "payment": {
                    "type": "invoice",
                    "invoice": invoice
                }
            },
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'lightning::payments::send_payment' failed: {}",
        request.1
    );

    json::from_str(&request.1).unwrap()
}

async fn get_payment_details(mm: &MarketMakerIt, payment_hash: &str) -> Json {
    let request = mm
        .rpc(&json!({
          "userpass": mm.userpass,
          "method": "lightning::payments::get_payment_details",
          "params": {
              "coin": "tBTC-TEST-lightning",
              "payment_hash": payment_hash
          },
        }))
        .await
        .unwrap();
    assert_eq!(
        request.0,
        StatusCode::OK,
        "'lightning::payments::get_payment_details' failed: {}",
        request.1
    );

    json::from_str(&request.1).unwrap()
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_enable_lightning() {
    let seed = "valley embody about obey never adapt gesture trust screen tube glide bread";

    let coins = json!([
        {
            "coin": "tBTC-TEST-segwit",
            "name": "tbitcoin",
            "fname": "tBitcoin",
            "rpcport": 18332,
            "pubtype": 111,
            "p2shtype": 196,
            "wiftype": 239,
            "segwit": true,
            "bech32_hrp": "tb",
            "address_format":{"format":"segwit"},
            "orderbook_ticker": "tBTC-TEST",
            "txfee": 0,
            "estimate_fee_mode": "ECONOMICAL",
            "mm2": 1,
            "required_confirmations": 0,
            "avg_blocktime": BTC_AVG_BLOCKTIME,
            "protocol": {
              "type": "UTXO"
            }
          },
          {
            "coin": "tBTC-TEST-lightning",
            "mm2": 1,
            "decimals": 11,
            "protocol": {
              "type": "LIGHTNING",
              "protocol_data":{
                "platform": "tBTC-TEST-segwit",
                "network": "testnet",
                "confirmation_targets": {
                  "background": 12,
                  "normal": 6,
                  "high_priority": 1
                }
              }
            }
          }
    ]);

    let mm = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var ("BOB_TRADE_IP") .ok(),
            "rpcip": env::var ("BOB_TRADE_IP") .ok(),
            "passphrase": seed.to_string(),
            "coins": coins,
            "i_am_seed": true,
            "rpc_password": "pass",
            "is_bootstrap_node": true
        }),
        "pass".into(),
        None,
    )
    .unwrap();
    let (_dump_log, _dump_dashboard) = mm.mm_dump();
    log!("log path: {}", mm.log_path.display());

    let _electrum = block_on(enable_electrum(&mm, "tBTC-TEST-segwit", false, T_BTC_ELECTRUMS));

    let enable_lightning_coin = block_on(enable_lightning(&mm, "tBTC-TEST-lightning", 600));
    assert_eq!(&enable_lightning_coin.platform_coin, "tBTC-TEST-segwit");
    assert_eq!(
        &enable_lightning_coin.address,
        "02ce55b18d617bf4ac27b0f045301a0bb4e71669ae45cb5f2529f2f217520ffca1"
    );
    assert_eq!(enable_lightning_coin.balance.spendable, BigDecimal::from(0));
    assert_eq!(enable_lightning_coin.balance.unspendable, BigDecimal::from(0));

    // Disable tBTC-TEST-lightning
    let disabled = block_on(mm.rpc(&json! ({
        "userpass": mm.userpass,
        "method": "disable_coin",
        "coin": "tBTC-TEST-lightning",
    })))
    .unwrap();
    assert_eq!(disabled.0, StatusCode::OK);

    // Enable tBTC-TEST-lightning
    let enable_lightning_coin = block_on(enable_lightning(&mm, "tBTC-TEST-lightning", 600));
    assert_eq!(&enable_lightning_coin.platform_coin, "tBTC-TEST-segwit");
    assert_eq!(
        &enable_lightning_coin.address,
        "02ce55b18d617bf4ac27b0f045301a0bb4e71669ae45cb5f2529f2f217520ffca1"
    );

    // Try to passive tBTC-TEST-segwit platform coin.
    let res = block_on(disable_coin(&mm, "tBTC-TEST-segwit", false));
    assert!(res.passivized);

    // Try to disable tBTC-TEST-lightning token
    // This should work, because platform coin is still in the memory.
    let res = block_on(disable_coin(&mm, "tBTC-TEST-lightning", false));
    assert!(!res.passivized);
    // Try to force disable tBTC-TEST-segwit platform coin.
    let res = block_on(disable_coin(&mm, "tBTC-TEST-segwit", true));
    assert!(!res.passivized);

    // Stop mm2
    block_on(mm.stop()).unwrap();
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_connect_to_node() {
    let (mm_node_1, mm_node_2, node_1_id, _) = start_lightning_nodes(false);
    let node_1_address = format!("{}@{}:9735", node_1_id, mm_node_1.ip);

    let connect = block_on(mm_node_2.rpc(&json!({
        "userpass": mm_node_2.userpass,
        "method": "lightning::nodes::connect_to_node",
        "params": {
            "coin": "tBTC-TEST-lightning",
            "node_address": node_1_address,
        },
    })))
    .unwrap();
    assert!(
        connect.0.is_success(),
        "!lightning::nodes::connect_to_node: {}",
        connect.1
    );
    let connect_res: Json = json::from_str(&connect.1).unwrap();
    let expected = format!("Connected successfully to node : {node_1_address}");
    assert_eq!(connect_res["result"], expected);

    block_on(mm_node_1.stop()).unwrap();
    block_on(mm_node_2.stop()).unwrap();
}

#[test]
// This test is ignored because it requires refilling the tBTC addresses with test coins periodically.
#[ignore]
#[cfg(not(target_arch = "wasm32"))]
fn test_open_channel() {
    let (mm_node_1, mut mm_node_2, node_1_id, node_2_id) = start_lightning_nodes(false);
    let node_1_address = format!("{}@{}:9735", node_1_id, mm_node_1.ip);

    block_on(open_channel(
        &mut mm_node_2,
        "tBTC-TEST-lightning",
        &node_1_address,
        0.0002,
        false,
    ));
    block_on(mm_node_2.wait_for_log(60., |log| log.contains("Transaction broadcasted successfully"))).unwrap();

    let list_channels_node_1 = block_on(mm_node_1.rpc(&json!({
        "userpass": mm_node_1.userpass,
        "method": "lightning::channels::list_open_channels_by_filter",
        "params": {
            "coin": "tBTC-TEST-lightning",
        },
    })))
    .unwrap();
    assert!(
        list_channels_node_1.0.is_success(),
        "!lightning::channels::list_open_channels_by_filter: {}",
        list_channels_node_1.1
    );
    let list_channels_node_1_res: Json = json::from_str(&list_channels_node_1.1).unwrap();
    log!("list_channels_node_1_res {:?}", list_channels_node_1_res);
    assert_eq!(
        list_channels_node_1_res["result"]["open_channels"][0]["counterparty_node_id"],
        node_2_id
    );
    assert_eq!(
        list_channels_node_1_res["result"]["open_channels"][0]["is_outbound"],
        false
    );
    assert_eq!(
        list_channels_node_1_res["result"]["open_channels"][0]["balance_msat"],
        0
    );

    let list_channels_node_2 = block_on(mm_node_2.rpc(&json!({
      "userpass": mm_node_2.userpass,
      "method": "lightning::channels::list_open_channels_by_filter",
      "params": {
          "coin": "tBTC-TEST-lightning",
      },
    })))
    .unwrap();
    assert!(
        list_channels_node_2.0.is_success(),
        "!lightning::channels::list_open_channels_by_filter: {}",
        list_channels_node_2.1
    );
    let list_channels_node_2_res: Json = json::from_str(&list_channels_node_2.1).unwrap();
    assert_eq!(
        list_channels_node_2_res["result"]["open_channels"][0]["counterparty_node_id"],
        node_1_id
    );
    assert_eq!(
        list_channels_node_2_res["result"]["open_channels"][0]["is_outbound"],
        true
    );
    assert_eq!(
        list_channels_node_2_res["result"]["open_channels"][0]["balance_msat"],
        20000000
    );

    block_on(mm_node_1.stop()).unwrap();
    block_on(mm_node_2.stop()).unwrap();
}

#[test]
// This test is ignored because it requires refilling the tBTC addresses with test coins periodically.
#[ignore]
#[cfg(not(target_arch = "wasm32"))]
// This also tests 0_confs_channels
fn test_send_payment() {
    let (mut mm_node_2, mut mm_node_1, node_2_id, node_1_id) = start_lightning_nodes(true);
    let node_1_address = format!("{}@{}:9735", node_1_id, mm_node_1.ip);

    block_on(add_trusted_node(&mm_node_1, &node_2_id));
    block_on(open_channel(
        &mut mm_node_2,
        "tBTC-TEST-lightning",
        &node_1_address,
        0.0002,
        true,
    ));

    let send_payment = block_on(mm_node_2.rpc(&json!({
        "userpass": mm_node_2.userpass,
        "method": "lightning::payments::send_payment",
        "params": {
            "coin": "tBTC-TEST-lightning",
            "payment": {
                "type": "keysend",
                "destination": node_1_id,
                "amount_in_msat": 1000,
                "expiry": 24
            }
        },
    })))
    .unwrap();
    assert!(
        send_payment.0.is_success(),
        "!lightning::payments::send_payment: {}",
        send_payment.1
    );

    let send_payment_res: Json = json::from_str(&send_payment.1).unwrap();
    log!("send_payment_res {:?}", send_payment_res);
    let payment_hash = send_payment_res["result"]["payment_hash"].as_str().unwrap();

    block_on(mm_node_2.wait_for_log(60., |log| log.contains(SUCCESSFUL_SEND_LOG))).unwrap();

    // Check payment on the sending node side
    let sender_payment_details = block_on(get_payment_details(&mm_node_2, payment_hash));
    let payment = &sender_payment_details["result"]["payment_details"];
    assert_eq!(payment["status"], "succeeded");
    assert_eq!(payment["amount_in_msat"], 1000);
    assert_eq!(payment["payment_type"]["type"], "Outbound Payment");

    // Check payment on the receiving node side
    let receiver_payment_details = block_on(get_payment_details(&mm_node_1, payment_hash));
    let payment = &receiver_payment_details["result"]["payment_details"];
    assert_eq!(payment["status"], "succeeded");
    assert_eq!(payment["amount_in_msat"], 1000);
    assert_eq!(payment["payment_type"]["type"], "Inbound Payment");

    // Test generate and pay invoice
    let generate_invoice = block_on(generate_invoice(&mm_node_1, 10000));
    let invoice = generate_invoice["result"]["invoice"].as_str().unwrap();
    let invoice_payment_hash = generate_invoice["result"]["payment_hash"].as_str().unwrap();

    let pay_invoice = block_on(pay_invoice(&mm_node_2, invoice));
    let payment_hash = pay_invoice["result"]["payment_hash"].as_str().unwrap();

    block_on(mm_node_1.wait_for_log(60., |log| log.contains(SUCCESSFUL_CLAIM_LOG))).unwrap();
    block_on(mm_node_2.wait_for_log(60., |log| {
        log.contains(&format!("{SUCCESSFUL_SEND_LOG} with payment hash {payment_hash}"))
    }))
    .unwrap();

    // Check payment on the sending node side
    let sender_payment_details = block_on(get_payment_details(&mm_node_2, invoice_payment_hash));
    let payment = &sender_payment_details["result"]["payment_details"];
    assert_eq!(payment["status"], "succeeded");
    assert_eq!(payment["amount_in_msat"], 10000);
    assert_eq!(payment["payment_type"]["type"], "Outbound Payment");
    assert_eq!(payment["description"], "test invoice");

    // Check payment on the receiving node side
    let receiver_payment_details = block_on(get_payment_details(&mm_node_1, invoice_payment_hash));
    let payment = &receiver_payment_details["result"]["payment_details"];
    assert_eq!(payment["status"], "succeeded");
    assert_eq!(payment["amount_in_msat"], 10000);
    assert_eq!(payment["payment_type"]["type"], "Inbound Payment");
    assert_eq!(payment["description"], "test invoice");

    block_on(mm_node_1.stop()).unwrap();
    block_on(mm_node_2.stop()).unwrap();
}

#[test]
// This test is ignored because it requires refilling the tBTC addresses with test coins periodically.
#[ignore]
#[cfg(not(target_arch = "wasm32"))]
fn test_mpp() {
    let (mut mm_node_2, mut mm_node_1, node_2_id, node_1_id) = start_lightning_nodes(true);
    let node_1_address = format!("{}@{}:9735", node_1_id, mm_node_1.ip);

    block_on(add_trusted_node(&mm_node_1, &node_2_id));

    block_on(open_channel(
        &mut mm_node_2,
        "tBTC-TEST-lightning",
        &node_1_address,
        0.0002,
        true,
    ));
    block_on(open_channel(
        &mut mm_node_2,
        "tBTC-TEST-lightning",
        &node_1_address,
        0.0002,
        true,
    ));

    // Wait a few seconds for both channels to be included in the invoice routing hints, since both channels are private.
    block_on(Timer::sleep(3.));

    // Test generate and pay invoice, invoice amount is larger than one channel so payment will use the 2 channels
    let generate_invoice = block_on(generate_invoice(&mm_node_1, 30000000));
    let invoice = generate_invoice["result"]["invoice"].as_str().unwrap();
    let invoice_payment_hash = generate_invoice["result"]["payment_hash"].as_str().unwrap();

    let pay_invoice = block_on(pay_invoice(&mm_node_2, invoice));
    let payment_hash = pay_invoice["result"]["payment_hash"].as_str().unwrap();

    block_on(mm_node_1.wait_for_log(60., |log| log.contains(SUCCESSFUL_CLAIM_LOG))).unwrap();
    block_on(mm_node_2.wait_for_log(60., |log| {
        log.contains(&format!("{SUCCESSFUL_SEND_LOG} with payment hash {payment_hash}"))
    }))
    .unwrap();

    // Check payment on the sending node side
    let sender_payment_details = block_on(get_payment_details(&mm_node_2, invoice_payment_hash));
    let payment = &sender_payment_details["result"]["payment_details"];
    assert_eq!(payment["status"], "succeeded");
    assert_eq!(payment["amount_in_msat"], 30000000);
    assert_eq!(payment["payment_type"]["type"], "Outbound Payment");
    assert_eq!(payment["description"], "test invoice");

    // Check payment on the receiving node side
    let receiver_payment_details = block_on(get_payment_details(&mm_node_1, invoice_payment_hash));
    let payment = &receiver_payment_details["result"]["payment_details"];
    assert_eq!(payment["status"], "succeeded");
    assert_eq!(payment["amount_in_msat"], 30000000);
    assert_eq!(payment["payment_type"]["type"], "Inbound Payment");
    assert_eq!(payment["description"], "test invoice");

    block_on(mm_node_1.stop()).unwrap();
    block_on(mm_node_2.stop()).unwrap();
}

#[test]
// This test is ignored because it requires refilling the tBTC and RICK addresses with test coins periodically.
#[ignore]
#[cfg(not(target_arch = "wasm32"))]
fn test_lightning_swaps() {
    let (mut mm_node_1, mut mm_node_2, node_1_id, node_2_id) = start_lightning_nodes(true);
    let node_1_address = format!("{}@{}:9735", node_1_id, mm_node_1.ip);

    block_on(add_trusted_node(&mm_node_1, &node_2_id));

    block_on(open_channel(
        &mut mm_node_2,
        "tBTC-TEST-lightning",
        &node_1_address,
        0.0002,
        true,
    ));

    // Enable coins on mm_node_1 side. Print the replies in case we need the "address".
    log!(
        "enable_coins (mm_node_1): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_node_1))
    );

    // Enable coins on mm_node_2 side. Print the replies in case we need the "address".
    log!(
        "enable_coins (mm_node_2): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_node_2))
    );

    // -------------------- Test Lightning Taker Swap --------------------
    let price = 0.0005;
    let volume = 0.1;
    let uuids = block_on(start_swaps(
        &mut mm_node_1,
        &mut mm_node_2,
        &[("RICK", "tBTC-TEST-lightning")],
        price,
        price,
        volume,
    ));
    block_on(wait_for_swaps_finish_and_check_status(
        &mut mm_node_1,
        &mut mm_node_2,
        &uuids,
        volume,
        price,
    ));

    // Check node 1 lightning balance after swap
    let node_1_lightning_balance = block_on(my_balance(&mm_node_1, "tBTC-TEST-lightning")).balance;
    // Channel reserve balance, which is non-spendable, is 1000 sats or 0.00001 BTC.
    // Note: A channel reserve balance is the amount that is set aside by each channel participant which ensures neither have 'nothing at stake' if a cheating attempt occurs.
    assert_eq!(node_1_lightning_balance, BigDecimal::from_str("0.00004").unwrap());

    // -------------------- Test Lightning Maker Swap --------------------
    let price = 10.;
    let volume = 0.00004;
    let uuids = block_on(start_swaps(
        &mut mm_node_1,
        &mut mm_node_2,
        &[("tBTC-TEST-lightning", "RICK")],
        price,
        price,
        volume,
    ));
    block_on(wait_for_swaps_finish_and_check_status(
        &mut mm_node_1,
        &mut mm_node_2,
        &uuids,
        volume,
        price,
    ));

    block_on(mm_node_1.stop()).unwrap();
    block_on(mm_node_2.stop()).unwrap();
}

#[test]
// This test is ignored because it requires refilling the tBTC and RICK addresses with test coins periodically.
// This test also takes a lot of time so it should always be ignored.
#[ignore]
#[cfg(not(target_arch = "wasm32"))]
fn test_lightning_taker_swap_mpp() {
    let (mut mm_node_1, mut mm_node_2, node_1_id, node_2_id) = start_lightning_nodes(true);
    let node_1_address = format!("{}@{}:9735", node_1_id, mm_node_1.ip);

    block_on(add_trusted_node(&mm_node_1, &node_2_id));

    block_on(open_channel(
        &mut mm_node_2,
        "tBTC-TEST-lightning",
        &node_1_address,
        0.0002,
        true,
    ));
    block_on(open_channel(
        &mut mm_node_2,
        "tBTC-TEST-lightning",
        &node_1_address,
        0.0002,
        true,
    ));

    // Enable coins on mm_node_1 side. Print the replies in case we need the "address".
    log!(
        "enable_coins (mm_node_1): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_node_1))
    );

    // Enable coins on mm_node_2 side. Print the replies in case we need the "address".
    log!(
        "enable_coins (mm_node_2): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_node_2))
    );

    let price = 0.0025;
    let volume = 0.1;
    let uuids = block_on(start_swaps(
        &mut mm_node_1,
        &mut mm_node_2,
        &[("RICK", "tBTC-TEST-lightning")],
        price,
        price,
        volume,
    ));
    block_on(wait_for_swaps_finish_and_check_status(
        &mut mm_node_1,
        &mut mm_node_2,
        &uuids,
        volume,
        price,
    ));
    block_on(mm_node_1.stop()).unwrap();
    block_on(mm_node_2.stop()).unwrap();
}

#[test]
// This test is ignored because it requires refilling the tBTC and RICK addresses with test coins periodically.
// This test also takes a lot of time so it should always be ignored.
#[ignore]
#[cfg(not(target_arch = "wasm32"))]
fn test_lightning_maker_swap_mpp() {
    let (mut mm_node_1, mut mm_node_2, node_1_id, node_2_id) = start_lightning_nodes(true);
    let node_1_address = format!("{}@{}:9735", node_1_id, mm_node_1.ip);

    block_on(add_trusted_node(&mm_node_1, &node_2_id));

    block_on(open_channel(
        &mut mm_node_2,
        "tBTC-TEST-lightning",
        &node_1_address,
        0.0002,
        true,
    ));
    block_on(open_channel(
        &mut mm_node_2,
        "tBTC-TEST-lightning",
        &node_1_address,
        0.0002,
        true,
    ));

    // Enable coins on mm_node_1 side. Print the replies in case we need the "address".
    log!(
        "enable_coins (mm_node_1): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_node_1))
    );

    // Enable coins on mm_node_2 side. Print the replies in case we need the "address".
    log!(
        "enable_coins (mm_node_2): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_node_2))
    );

    let price = 10.;
    let volume = 0.00025;
    let uuids = block_on(start_swaps(
        &mut mm_node_2,
        &mut mm_node_1,
        &[("tBTC-TEST-lightning", "RICK")],
        price,
        price,
        volume,
    ));
    block_on(wait_for_swaps_finish_and_check_status(
        &mut mm_node_2,
        &mut mm_node_1,
        &uuids,
        volume,
        price,
    ));
    block_on(mm_node_1.stop()).unwrap();
    block_on(mm_node_2.stop()).unwrap();
}

// Todo: not working for now, should work once on-chain claiming is implemented https://github.com/lightningdevkit/rust-lightning/issues/2017
// Todo: watchtowers implementation is needed for such cases, if taker is offline
#[test]
// This test is ignored because it requires refilling the tBTC and RICK addresses with test coins periodically.
// This test also takes a lot of time so it should always be ignored.
#[ignore]
#[cfg(not(target_arch = "wasm32"))]
fn test_lightning_taker_gets_swap_preimage_onchain() {
    let (mut mm_node_1, mut mm_node_2, node_1_id, _) = start_lightning_nodes(false);
    let node_1_address = format!("{}@{}:9735", node_1_id, mm_node_1.ip);

    let open_channel = block_on(open_channel(
        &mut mm_node_2,
        "tBTC-TEST-lightning",
        &node_1_address,
        0.0002,
        true,
    ));
    let uuid = open_channel["result"]["uuid"].as_str().unwrap();

    // Enable coins on mm_node_1 side. Print the replies in case we need the "address".
    log!(
        "enable_coins (mm_node_1): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_node_1))
    );

    // Enable coins on mm_node_2 side. Print the replies in case we need the "address".
    log!(
        "enable_coins (mm_node_2): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_node_2))
    );

    let price = 0.0005;
    let volume = 0.1;
    let uuids = block_on(start_swaps(
        &mut mm_node_1,
        &mut mm_node_2,
        &[("RICK", "tBTC-TEST-lightning")],
        price,
        price,
        volume,
    ));
    block_on(mm_node_1.wait_for_log(60., |log| log.contains(PAYMENT_CLAIMABLE_LOG))).unwrap();

    // Taker node force closes the channel after the maker receives the payment but before the maker claims the payment and releases the preimage
    block_on(close_channel(&mm_node_2, uuid, true));

    block_on(mm_node_1.wait_for_log(7200., |log| log.contains(&format!("[swap uuid={}] Finished", uuids[0])))).unwrap();
    block_on(mm_node_2.wait_for_log(7200., |log| log.contains(&format!("[swap uuid={}] Finished", uuids[0])))).unwrap();

    // Todo: If the test passes the payment will be added to the tBTC balance, add a check here, find a way to inform the user of this.

    block_on(mm_node_1.stop()).unwrap();
    block_on(mm_node_2.stop()).unwrap();
}

// Todo: not working for now, should work once on-chain claiming is implemented https://github.com/lightningdevkit/rust-lightning/issues/2017
// Todo: watchtowers implementation is needed for such cases, if taker is offline
#[test]
// This test is ignored because it requires refilling the tBTC and RICK addresses with test coins periodically.
// This test also takes a lot of time so it should always be ignored.
#[ignore]
#[cfg(not(target_arch = "wasm32"))]
fn test_lightning_taker_claims_mpp() {
    let (mut mm_node_1, mut mm_node_2, node_1_id, _) = start_lightning_nodes(false);
    let node_1_address = format!("{}@{}:9735", node_1_id, mm_node_1.ip);

    let open_channel_1 = block_on(open_channel(
        &mut mm_node_2,
        "tBTC-TEST-lightning",
        &node_1_address,
        0.0002,
        true,
    ));
    let uuid = open_channel_1["result"]["uuid"].as_str().unwrap();
    block_on(open_channel(
        &mut mm_node_2,
        "tBTC-TEST-lightning",
        &node_1_address,
        0.0002,
        true,
    ));

    // Enable coins on mm_node_1 side. Print the replies in case we need the "address".
    log!(
        "enable_coins (mm_node_1): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_node_1))
    );

    // Enable coins on mm_node_2 side. Print the replies in case we need the "address".
    log!(
        "enable_coins (mm_node_2): {:?}",
        block_on(enable_coins_rick_morty_electrum(&mm_node_2))
    );

    let price = 0.0025;
    let volume = 0.1;
    let uuids = block_on(start_swaps(
        &mut mm_node_1,
        &mut mm_node_2,
        &[("RICK", "tBTC-TEST-lightning")],
        price,
        price,
        volume,
    ));

    block_on(mm_node_1.wait_for_log(60., |log| log.contains(PAYMENT_CLAIMABLE_LOG))).unwrap();

    // Taker node force closes the channel after the maker receives the payment but before the maker claims the payment and releases the preimage
    block_on(close_channel(&mm_node_2, uuid, true));

    block_on(mm_node_1.wait_for_log(7200., |log| log.contains(&format!("[swap uuid={}] Finished", uuids[0])))).unwrap();
    block_on(mm_node_2.wait_for_log(7200., |log| log.contains(&format!("[swap uuid={}] Finished", uuids[0])))).unwrap();

    // Todo: If the test passes the payment will be added to the tBTC balance, add a check here, find a way to inform the user of this.

    block_on(mm_node_1.stop()).unwrap();
    block_on(mm_node_2.stop()).unwrap();
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn test_sign_verify_message_lightning() {
    let seed = "spice describe gravity federal blast come thank unfair canal monkey style afraid";

    let coins = json!([
      {
        "coin": "tBTC-TEST-segwit",
        "name": "tbitcoin",
        "fname": "tBitcoin",
        "rpcport": 18332,
        "pubtype": 111,
        "p2shtype": 196,
        "wiftype": 239,
        "segwit": true,
        "bech32_hrp": "tb",
        "address_format":{"format":"segwit"},
        "orderbook_ticker": "tBTC-TEST",
        "txfee": 0,
        "estimate_fee_mode": "ECONOMICAL",
        "mm2": 1,
        "required_confirmations": 0,
        "avg_blocktime": BTC_AVG_BLOCKTIME,
        "protocol": {
          "type": "UTXO"
        }
      },
      {
        "coin": "tBTC-TEST-lightning",
        "mm2": 1,
        "decimals": 11,
        "sign_message_prefix": "Lightning Signed Message:",
        "protocol": {
          "type": "LIGHTNING",
          "protocol_data":{
            "platform": "tBTC-TEST-segwit",
            "network": "testnet",
            "confirmation_targets": {
              "background": 12,
              "normal": 6,
              "high_priority": 1
            }
          }
        }
      }
    ]);

    let mm = MarketMakerIt::start(
        json!({
            "gui": "nogui",
            "netid": 9998,
            "myipaddr": env::var ("BOB_TRADE_IP") .ok(),
            "rpcip": env::var ("BOB_TRADE_IP") .ok(),
            "passphrase": seed.to_string(),
            "coins": coins,
            "i_am_seed": true,
            "rpc_password": "pass",
            "is_bootstrap_node": true
        }),
        "pass".into(),
        None,
    )
    .unwrap();
    let (_dump_log, _dump_dashboard) = mm.mm_dump();
    log!("log path: {}", mm.log_path.display());

    block_on(enable_electrum(&mm, "tBTC-TEST-segwit", false, T_BTC_ELECTRUMS));
    block_on(enable_lightning(&mm, "tBTC-TEST-lightning", 600));

    let response = block_on(sign_message(&mm, "tBTC-TEST-lightning", None));
    let response: RpcV2Response<SignatureResponse> = json::from_value(response).unwrap();
    let response = response.result;

    assert_eq!(
        response.signature,
        "dhmbgykwzy53uycr6u8mpp3us6poikc5qh7wgex5qn54msq7cs3ygebj3h9swaocboqzi89jazwo7i3mmqou15w4dcty666sq3yqhzhr"
    );

    let response = block_on(verify_message(
        &mm,
        "tBTC-TEST-lightning",
        "dhmbgykwzy53uycr6u8mpp3us6poikc5qh7wgex5qn54msq7cs3ygebj3h9swaocboqzi89jazwo7i3mmqou15w4dcty666sq3yqhzhr",
        "0367c7b9f42eb15205de39454ddf9fcfce70a129b01049d9fe1b3b34eac1d6b933",
    ));
    let response: RpcV2Response<VerificationResponse> = json::from_value(response).unwrap();
    let response = response.result;

    assert!(response.is_valid);
}
