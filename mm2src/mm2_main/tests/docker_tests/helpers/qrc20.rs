//! Qtum/QRC20 helpers for docker tests.
//!
//! This module provides:
//! - QRC20 coin creation and funding utilities
//! - Qtum docker node helpers
//! - QRC20 contract initialization

use crate::docker_tests::helpers::docker_ops::{docker_cp_from_container, wait_for_file};
use crate::docker_tests::helpers::env::{
    random_secp256k1_secret, resolve_compose_container_id, DockerNode, KDF_QTUM_SERVICE,
};
use crate::docker_tests::helpers::utxo::fill_address;
use crate::docker_tests::helpers::utxo::QTUM_LOCK;
use coins::qrc20::rpc_clients::for_tests::Qrc20NativeWalletOps;
use coins::qrc20::{qrc20_coin_with_priv_key, Qrc20ActivationParams, Qrc20Coin};
use coins::utxo::qtum::QtumBasedCoin;
use coins::utxo::qtum::{qtum_coin_with_priv_key, QtumCoin};
use coins::utxo::rpc_clients::{UtxoRpcClientEnum, UtxoRpcClientOps};
use coins::utxo::{sat_from_big_decimal, UtxoActivationParams, UtxoCoinFields};
use coins::{ConfirmPaymentInput, MarketCoinOps};
use common::{block_on, block_on_f01, now_ms, now_sec, temp_dir, wait_until_ms, wait_until_sec};
use crypto::Secp256k1Secret;
use ethereum_types::H160 as H160Eth;
use http::StatusCode;
use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};
use mm2_number::BigDecimal;
use mm2_test_helpers::for_tests::MarketMakerIt;
use serde_json::{self as json, json, Value as Json};
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use testcontainers::core::WaitFor;
use testcontainers::runners::SyncRunner;
use testcontainers::{GenericImage, RunnableImage};

// =============================================================================
// Docker image constants
// =============================================================================

/// Qtum regtest docker image
pub const QTUM_REGTEST_DOCKER_IMAGE: &str = "docker.io/gleec/qtumregtest";
/// Qtum regtest docker image with tag
pub const QTUM_REGTEST_DOCKER_IMAGE_WITH_TAG: &str = "docker.io/gleec/qtumregtest:latest";

// =============================================================================
// Global state (OnceLock for contract addresses)
// =============================================================================

/// QICK token contract address
static QICK_TOKEN_ADDRESS: OnceLock<H160Eth> = OnceLock::new();
/// QORTY token contract address
static QORTY_TOKEN_ADDRESS: OnceLock<H160Eth> = OnceLock::new();
/// QRC20 swap contract address
static QRC20_SWAP_CONTRACT_ADDRESS: OnceLock<H160Eth> = OnceLock::new();
/// Path to Qtum config file
static QTUM_CONF_PATH: OnceLock<PathBuf> = OnceLock::new();

/// Get the QICK token contract address.
/// Panics if called before initialization.
pub fn qick_token_address() -> H160Eth {
    *QICK_TOKEN_ADDRESS
        .get()
        .expect("QICK_TOKEN_ADDRESS not initialized - ensure QRC20 init has run")
}

/// Get the QORTY token contract address.
/// Panics if called before initialization.
pub fn qorty_token_address() -> H160Eth {
    *QORTY_TOKEN_ADDRESS
        .get()
        .expect("QORTY_TOKEN_ADDRESS not initialized - ensure QRC20 init has run")
}

/// Get the QRC20 swap contract address.
/// Panics if called before initialization.
pub fn qrc20_swap_contract_address() -> H160Eth {
    *QRC20_SWAP_CONTRACT_ADDRESS
        .get()
        .expect("QRC20_SWAP_CONTRACT_ADDRESS not initialized - ensure QRC20 init has run")
}

/// Get the Qtum config file path.
/// Panics if called before initialization.
pub fn qtum_conf_path() -> &'static PathBuf {
    QTUM_CONF_PATH
        .get()
        .expect("QTUM_CONF_PATH not initialized - ensure QRC20 init has run")
}

/// Set the QICK token contract address (for initialization).
pub fn set_qick_token_address(addr: H160Eth) {
    QICK_TOKEN_ADDRESS
        .set(addr)
        .expect("QICK_TOKEN_ADDRESS already initialized");
}

/// Set the QORTY token contract address (for initialization).
pub fn set_qorty_token_address(addr: H160Eth) {
    QORTY_TOKEN_ADDRESS
        .set(addr)
        .expect("QORTY_TOKEN_ADDRESS already initialized");
}

/// Set the QRC20 swap contract address (for initialization).
pub fn set_qrc20_swap_contract_address(addr: H160Eth) {
    QRC20_SWAP_CONTRACT_ADDRESS
        .set(addr)
        .expect("QRC20_SWAP_CONTRACT_ADDRESS already initialized");
}

/// Set the Qtum config file path (for initialization).
pub fn set_qtum_conf_path(path: PathBuf) {
    QTUM_CONF_PATH.set(path).expect("QTUM_CONF_PATH already initialized");
}

/// Setup Qtum configuration from a docker-compose container.
///
/// Copies the Qtum configuration file from the compose container to the local
/// daemon data directory. Used when tests run against pre-started compose nodes.
pub fn setup_qtum_conf_for_compose() {
    use coins::utxo::coin_daemon_data_dir;

    let mut conf_path = coin_daemon_data_dir("qtum", false);
    std::fs::create_dir_all(&conf_path).unwrap();
    conf_path.push("qtum.conf");

    let container_id = resolve_compose_container_id(KDF_QTUM_SERVICE);
    docker_cp_from_container(&container_id, "/data/node_0/qtum.conf", &conf_path);
    wait_for_file(&conf_path, 3000);

    set_qtum_conf_path(conf_path);
}

// =============================================================================
// Constants
// =============================================================================

/// Qtum address label used in tests
pub const QTUM_ADDRESS_LABEL: &str = "MM2_ADDRESS_LABEL";

// =============================================================================
// Utility functions
// =============================================================================

/// Get only one address assigned the specified label.
pub fn get_address_by_label<T>(coin: T, label: &str) -> String
where
    T: AsRef<UtxoCoinFields>,
{
    let native = match coin.as_ref().rpc_client {
        UtxoRpcClientEnum::Native(ref native) => native,
        UtxoRpcClientEnum::Electrum(_) => panic!("NativeClient expected"),
    };
    let mut addresses = block_on_f01(native.get_addresses_by_label(label))
        .expect("!getaddressesbylabel")
        .into_iter();
    match addresses.next() {
        Some((addr, _purpose)) if addresses.next().is_none() => addr,
        Some(_) => panic!("Expected only one address by {:?}", label),
        None => panic!("Expected one address by {:?}", label),
    }
}

/// Build `Qrc20Coin` from ticker and privkey without filling the balance.
pub fn qrc20_coin_from_privkey(ticker: &str, priv_key: Secp256k1Secret) -> (MmArc, Qrc20Coin) {
    use crate::docker_tests::helpers::utxo::import_address;

    let contract_address = match ticker {
        "QICK" => qick_token_address(),
        "QORTY" => qorty_token_address(),
        _ => panic!("Expected QICK or QORTY ticker"),
    };
    let swap_contract_address = qrc20_swap_contract_address();
    let platform = "QTUM";
    let ctx = MmCtxBuilder::new().into_mm_arc();
    let confpath = qtum_conf_path();
    let conf = json!({
        "coin":ticker,
        "decimals": 8,
        "required_confirmations":0,
        "pubtype":120,
        "p2shtype":110,
        "wiftype":128,
        "mm2":1,
        "mature_confirmations":500,
        "network":"regtest",
        "confpath": confpath,
        "dust": 72800,
    });
    let req = json!({
        "method": "enable",
        "swap_contract_address": format!("{:#02x}", swap_contract_address),
    });
    let params = Qrc20ActivationParams::from_legacy_req(&req).unwrap();

    let coin = block_on(qrc20_coin_with_priv_key(
        &ctx,
        ticker,
        platform,
        &conf,
        &params,
        priv_key,
        contract_address,
    ))
    .unwrap();

    block_on(import_address(&coin));
    (ctx, coin)
}

/// Get the QRC20 coin config item for MM2 config.
pub fn qrc20_coin_conf_item(ticker: &str) -> Json {
    let contract_address = match ticker {
        "QICK" => qick_token_address(),
        "QORTY" => qorty_token_address(),
        _ => panic!("Expected either QICK or QORTY ticker, found {}", ticker),
    };
    let contract_address = format!("{contract_address:#02x}");

    let confpath = qtum_conf_path();
    json!({
        "coin":ticker,
        "required_confirmations":1,
        "pubtype":120,
        "p2shtype":110,
        "wiftype":128,
        "mature_confirmations":500,
        "confpath":confpath,
        "network":"regtest",
        "protocol":{"type":"QRC20","protocol_data":{"platform":"QTUM","contract_address":contract_address}}})
}

/// Fill a QRC20 address with tokens.
pub fn fill_qrc20_address(coin: &Qrc20Coin, amount: BigDecimal, timeout: u64) {
    // prevent concurrent fill since daemon RPC returns errors if send_to_address
    // is called concurrently (insufficient funds) and it also may return other errors
    // if previous transaction is not confirmed yet
    let _lock = block_on(QTUM_LOCK.lock());
    let timeout = wait_until_sec(timeout);
    let client = match coin.as_ref().rpc_client {
        UtxoRpcClientEnum::Native(ref client) => client,
        UtxoRpcClientEnum::Electrum(_) => panic!("Expected NativeClient"),
    };

    use futures::TryFutureExt;
    let from_addr = get_address_by_label(coin, QTUM_ADDRESS_LABEL);
    let to_addr = block_on_f01(coin.my_addr_as_contract_addr().compat()).unwrap();
    let satoshis = sat_from_big_decimal(&amount, coin.as_ref().decimals).expect("!sat_from_big_decimal");

    let hash = block_on_f01(client.transfer_tokens(
        &coin.contract_address,
        &from_addr,
        to_addr,
        satoshis.into(),
        coin.as_ref().decimals,
    ))
    .expect("!transfer_tokens")
    .txid;

    let tx_bytes = block_on_f01(client.get_transaction_bytes(&hash)).unwrap();
    log!("{:02x}", tx_bytes);
    let confirm_payment_input = ConfirmPaymentInput {
        payment_tx: tx_bytes.0,
        confirmations: 1,
        requires_nota: false,
        wait_until: timeout,
        check_every: 1,
    };
    block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();
}

/// Generate random privkey, create a QRC20 coin and fill its address with the specified balance.
pub fn generate_qrc20_coin_with_random_privkey(
    ticker: &str,
    qtum_balance: BigDecimal,
    qrc20_balance: BigDecimal,
) -> (MmArc, Qrc20Coin, Secp256k1Secret) {
    let priv_key = random_secp256k1_secret();
    let (ctx, coin) = qrc20_coin_from_privkey(ticker, priv_key);

    let timeout = 30; // timeout if test takes more than 30 seconds to run
    let my_address = coin.my_address().expect("!my_address");
    fill_address(&coin, &my_address, qtum_balance, timeout);
    fill_qrc20_address(&coin, qrc20_balance, timeout);
    (ctx, coin, priv_key)
}

/// Generate a Qtum coin with random privkey.
pub fn generate_qtum_coin_with_random_privkey(
    ticker: &str,
    balance: BigDecimal,
    txfee: Option<u64>,
) -> (MmArc, QtumCoin, [u8; 32]) {
    let confpath = qtum_conf_path();
    let conf = json!({
        "coin":ticker,
        "decimals":8,
        "required_confirmations":0,
        "pubtype":120,
        "p2shtype": 110,
        "wiftype":128,
        "txfee": txfee,
        "txfee_volatility_percent":0.1,
        "mm2":1,
        "mature_confirmations":500,
        "network":"regtest",
        "confpath": confpath,
        "dust": 72800,
    });
    let req = json!({"method": "enable"});
    let priv_key = random_secp256k1_secret();
    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let coin = block_on(qtum_coin_with_priv_key(&ctx, ticker, &conf, &params, priv_key)).unwrap();

    let timeout = 30; // timeout if test takes more than 30 seconds to run
    let my_address = coin.my_address().expect("!my_address");
    fill_address(&coin, &my_address, balance, timeout);
    (ctx, coin, priv_key.take())
}

/// Generate a SegWit Qtum coin with random privkey.
pub fn generate_segwit_qtum_coin_with_random_privkey(
    ticker: &str,
    balance: BigDecimal,
    txfee: Option<u64>,
) -> (MmArc, QtumCoin, Secp256k1Secret) {
    let confpath = qtum_conf_path();
    let conf = json!({
        "coin":ticker,
        "decimals":8,
        "required_confirmations":0,
        "pubtype":120,
        "p2shtype": 110,
        "wiftype":128,
        "segwit":true,
        "txfee": txfee,
        "txfee_volatility_percent":0.1,
        "mm2":1,
        "mature_confirmations":500,
        "network":"regtest",
        "confpath": confpath,
        "dust": 72800,
        "bech32_hrp":"qcrt",
        "address_format": {
            "format": "segwit",
        },
    });
    let req = json!({"method": "enable"});
    let priv_key = random_secp256k1_secret();
    let ctx = MmCtxBuilder::new().into_mm_arc();
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let coin = block_on(qtum_coin_with_priv_key(&ctx, ticker, &conf, &params, priv_key)).unwrap();

    let timeout = 30; // timeout if test takes more than 30 seconds to run
    let my_address = coin.my_address().expect("!my_address");
    fill_address(&coin, &my_address, balance, timeout);
    (ctx, coin, priv_key)
}

/// Wait for the `estimatesmartfee` returns no errors.
pub fn wait_for_estimate_smart_fee(timeout: u64) -> Result<(), String> {
    enum EstimateSmartFeeState {
        Idle,
        Ok,
        NotAvailable,
    }
    lazy_static! {
        static ref LOCK: Mutex<EstimateSmartFeeState> = Mutex::new(EstimateSmartFeeState::Idle);
    }

    let state = &mut *LOCK.lock().unwrap();
    match state {
        EstimateSmartFeeState::Ok => return Ok(()),
        EstimateSmartFeeState::NotAvailable => return ERR!("estimatesmartfee not available"),
        EstimateSmartFeeState::Idle => log!("Start wait_for_estimate_smart_fee"),
    }

    let priv_key = random_secp256k1_secret();
    let (_ctx, coin) = qrc20_coin_from_privkey("QICK", priv_key);
    let timeout = wait_until_sec(timeout);
    let client = match coin.as_ref().rpc_client {
        UtxoRpcClientEnum::Native(ref client) => client,
        UtxoRpcClientEnum::Electrum(_) => panic!("Expected NativeClient"),
    };
    while now_sec() < timeout {
        if let Ok(res) = block_on_f01(client.estimate_smart_fee(&None, 1)) {
            if res.errors.is_empty() {
                *state = EstimateSmartFeeState::Ok;
                return Ok(());
            }
        }
        thread::sleep(Duration::from_secs(1));
    }

    *state = EstimateSmartFeeState::NotAvailable;
    ERR!("Waited too long for estimate_smart_fee to work")
}

/// Enable QRC20 coin in MarketMaker.
pub async fn enable_qrc20_native(mm: &MarketMakerIt, coin: &str) -> Json {
    let swap_contract_address = qrc20_swap_contract_address();

    let native = mm
        .rpc(&json! ({
            "userpass": mm.userpass,
            "method": "enable",
            "coin": coin,
            "swap_contract_address": format!("{:#02x}", swap_contract_address),
            "mm2": 1,
        }))
        .await
        .unwrap();
    assert_eq!(native.0, StatusCode::OK, "'enable' failed: {}", native.1);
    json::from_str(&native.1).unwrap()
}

// =============================================================================
// Docker node setup
// =============================================================================

// =============================================================================
// QtumDockerOps - Docker ops for Qtum initialization
// =============================================================================

use crate::docker_tests::helpers::docker_ops::CoinDockerOps;

const QRC20_TOKEN_BYTES: &str = "6080604052600860ff16600a0a633b9aca000260005534801561002157600080fd5b50600054600160003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002081905550610c69806100776000396000f3006080604052600436106100a4576000357c0100000000000000000000000000000000000000000000000000000000900463ffffffff16806306fdde03146100a9578063095ea7b31461013957806318160ddd1461019e57806323b872dd146101c9578063313ce5671461024e5780635a3b7e421461027f57806370a082311461030f57806395d89b4114610366578063a9059cbb146103f6578063dd62ed3e1461045b575b600080fd5b3480156100b557600080fd5b506100be6104d2565b6040518080602001828103825283818151815260200191508051906020019080838360005b838110156100fe5780820151818401526020810190506100e3565b50505050905090810190601f16801561012b5780820380516001836020036101000a031916815260200191505b509250505060405180910390f35b34801561014557600080fd5b50610184600480360381019080803573ffffffffffffffffffffffffffffffffffffffff1690602001909291908035906020019092919050505061050b565b604051808215151515815260200191505060405180910390f35b3480156101aa57600080fd5b506101b36106bb565b6040518082815260200191505060405180910390f35b3480156101d557600080fd5b50610234600480360381019080803573ffffffffffffffffffffffffffffffffffffffff169060200190929190803573ffffffffffffffffffffffffffffffffffffffff169060200190929190803590602001909291905050506106c1565b604051808215151515815260200191505060405180910390f35b34801561025a57600080fd5b506102636109a1565b604051808260ff1660ff16815260200191505060405180910390f35b34801561028b57600080fd5b506102946109a6565b6040518080602001828103825283818151815260200191508051906020019080838360005b838110156102d45780820151818401526020810190506102b9565b50505050905090810190601f1680156103015780820380516001836020036101000a031916815260200191505b509250505060405180910390f35b34801561031b57600080fd5b50610350600480360381019080803573ffffffffffffffffffffffffffffffffffffffff1690602001909291905050506109df565b6040518082815260200191505060405180910390f35b34801561037257600080fd5b5061037b6109f7565b6040518080602001828103825283818151815260200191508051906020019080838360005b838110156103bb5780820151818401526020810190506103a0565b50505050905090810190601f1680156103e85780820380516001836020036101000a031916815260200191505b509250505060405180910390f35b34801561040257600080fd5b50610441600480360381019080803573ffffffffffffffffffffffffffffffffffffffff16906020019092919080359060200190929190505050610a30565b604051808215151515815260200191505060405180910390f35b34801561046757600080fd5b506104bc600480360381019080803573ffffffffffffffffffffffffffffffffffffffff169060200190929190803573ffffffffffffffffffffffffffffffffffffffff169060200190929190505050610be1565b6040518082815260200191505060405180910390f35b6040805190810160405280600881526020017f515243205445535400000000000000000000000000000000000000000000000081525081565b60008260008173ffffffffffffffffffffffffffffffffffffffff161415151561053457600080fd5b60008314806105bf57506000600260003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060008673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002054145b15156105ca57600080fd5b82600260003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060008673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055508373ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff167f8c5be1e5ebec7d5bd14f71427d1e84f3dd0314c0f7b2291e5b200ac8c7c3b925856040518082815260200191505060405180910390a3600191505092915050565b60005481565b60008360008173ffffffffffffffffffffffffffffffffffffffff16141515156106ea57600080fd5b8360008173ffffffffffffffffffffffffffffffffffffffff161415151561071157600080fd5b610797600260008873ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205485610c06565b600260008873ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002060003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002081905550610860600160008873ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205485610c06565b600160008873ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055506108ec600160008773ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205485610c1f565b600160008773ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055508473ffffffffffffffffffffffffffffffffffffffff168673ffffffffffffffffffffffffffffffffffffffff167fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef866040518082815260200191505060405180910390a36001925050509392505050565b600881565b6040805190810160405280600981526020017f546f6b656e20302e31000000000000000000000000000000000000000000000081525081565b60016020528060005260406000206000915090505481565b6040805190810160405280600381526020017f515443000000000000000000000000000000000000000000000000000000000081525081565b60008260008173ffffffffffffffffffffffffffffffffffffffff1614151515610a5957600080fd5b610aa2600160003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205484610c06565b600160003373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200190815260200160002081905550610b2e600160008673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020019081526020016000205484610c1f565b600160008673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff168152602001908152602001600020819055508373ffffffffffffffffffffffffffffffffffffffff163373ffffffffffffffffffffffffffffffffffffffff167fddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef856040518082815260200191505060405180910390a3600191505092915050565b6002602052816000526040600020602052806000526040600020600091509150505481565b6000818310151515610c1457fe5b818303905092915050565b6000808284019050838110151515610c3357fe5b80915050929150505600a165627a7a723058207f2e5248b61b80365ea08a0f6d11ac0b47374c4dfd538de76bc2f19591bbbba40029";
const QRC20_SWAP_CONTRACT_BYTES: &str = "608060405234801561001057600080fd5b50611437806100206000396000f3fe60806040526004361061004a5760003560e01c806302ed292b1461004f5780630716326d146100de578063152cf3af1461017b57806346fc0294146101f65780639b415b2a14610294575b600080fd5b34801561005b57600080fd5b506100dc600480360360a081101561007257600080fd5b81019080803590602001909291908035906020019092919080359060200190929190803573ffffffffffffffffffffffffffffffffffffffff169060200190929190803573ffffffffffffffffffffffffffffffffffffffff169060200190929190505050610339565b005b3480156100ea57600080fd5b506101176004803603602081101561010157600080fd5b8101908080359060200190929190505050610867565b60405180846bffffffffffffffffffffffff19166bffffffffffffffffffffffff191681526020018367ffffffffffffffff1667ffffffffffffffff16815260200182600381111561016557fe5b60ff168152602001935050505060405180910390f35b6101f46004803603608081101561019157600080fd5b8101908080359060200190929190803573ffffffffffffffffffffffffffffffffffffffff16906020019092919080356bffffffffffffffffffffffff19169060200190929190803567ffffffffffffffff1690602001909291905050506108bf565b005b34801561020257600080fd5b50610292600480360360a081101561021957600080fd5b81019080803590602001909291908035906020019092919080356bffffffffffffffffffffffff19169060200190929190803573ffffffffffffffffffffffffffffffffffffffff169060200190929190803573ffffffffffffffffffffffffffffffffffffffff169060200190929190505050610bd9565b005b610337600480360360c08110156102aa57600080fd5b810190808035906020019092919080359060200190929190803573ffffffffffffffffffffffffffffffffffffffff169060200190929190803573ffffffffffffffffffffffffffffffffffffffff16906020019092919080356bffffffffffffffffffffffff19169060200190929190803567ffffffffffffffff169060200190929190505050610fe2565b005b6001600381111561034657fe5b600080878152602001908152602001600020600001601c9054906101000a900460ff16600381111561037457fe5b1461037e57600080fd5b6000600333836003600288604051602001808281526020019150506040516020818303038152906040526040518082805190602001908083835b602083106103db57805182526020820191506020810190506020830392506103b8565b6001836020036101000a038019825116818451168082178552505050505050905001915050602060405180830381855afa15801561041d573d6000803e3d6000fd5b5050506040513d602081101561043257600080fd5b8101908080519060200190929190505050604051602001808281526020019150506040516020818303038152906040526040518082805190602001908083835b602083106104955780518252602082019150602081019050602083039250610472565b6001836020036101000a038019825116818451168082178552505050505050905001915050602060405180830381855afa1580156104d7573d6000803e3d6000fd5b5050506040515160601b8689604051602001808673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b81526014018573ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b8152601401846bffffffffffffffffffffffff19166bffffffffffffffffffffffff191681526014018373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b8152601401828152602001955050505050506040516020818303038152906040526040518082805190602001908083835b602083106105fc57805182526020820191506020810190506020830392506105d9565b6001836020036101000a038019825116818451168082178552505050505050905001915050602060405180830381855afa15801561063e573d6000803e3d6000fd5b5050506040515160601b905060008087815260200190815260200160002060000160009054906101000a900460601b6bffffffffffffffffffffffff1916816bffffffffffffffffffffffff19161461069657600080fd5b6002600080888152602001908152602001600020600001601c6101000a81548160ff021916908360038111156106c857fe5b0217905550600073ffffffffffffffffffffffffffffffffffffffff168373ffffffffffffffffffffffffffffffffffffffff16141561074e573373ffffffffffffffffffffffffffffffffffffffff166108fc869081150290604051600060405180830381858888f19350505050158015610748573d6000803e3d6000fd5b50610820565b60008390508073ffffffffffffffffffffffffffffffffffffffff1663a9059cbb33886040518363ffffffff1660e01b8152600401808373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200182815260200192505050602060405180830381600087803b1580156107da57600080fd5b505af11580156107ee573d6000803e3d6000fd5b505050506040513d602081101561080457600080fd5b810190808051906020019092919050505061081e57600080fd5b505b7f36c177bcb01c6d568244f05261e2946c8c977fa50822f3fa098c470770ee1f3e8685604051808381526020018281526020019250505060405180910390a1505050505050565b60006020528060005260406000206000915090508060000160009054906101000a900460601b908060000160149054906101000a900467ffffffffffffffff169080600001601c9054906101000a900460ff16905083565b600073ffffffffffffffffffffffffffffffffffffffff168373ffffffffffffffffffffffffffffffffffffffff16141580156108fc5750600034115b801561094057506000600381111561091057fe5b600080868152602001908152602001600020600001601c9054906101000a900460ff16600381111561093e57fe5b145b61094957600080fd5b60006003843385600034604051602001808673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b81526014018573ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b8152601401846bffffffffffffffffffffffff19166bffffffffffffffffffffffff191681526014018373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b8152601401828152602001955050505050506040516020818303038152906040526040518082805190602001908083835b60208310610a6c5780518252602082019150602081019050602083039250610a49565b6001836020036101000a038019825116818451168082178552505050505050905001915050602060405180830381855afa158015610aae573d6000803e3d6000fd5b5050506040515160601b90506040518060600160405280826bffffffffffffffffffffffff191681526020018367ffffffffffffffff16815260200160016003811115610af757fe5b81525060008087815260200190815260200160002060008201518160000160006101000a81548173ffffffffffffffffffffffffffffffffffffffff021916908360601c021790555060208201518160000160146101000a81548167ffffffffffffffff021916908367ffffffffffffffff160217905550604082015181600001601c6101000a81548160ff02191690836003811115610b9357fe5b02179055509050507fccc9c05183599bd3135da606eaaf535daffe256e9de33c048014cffcccd4ad57856040518082815260200191505060405180910390a15050505050565b60016003811115610be657fe5b600080878152602001908152602001600020600001601c9054906101000a900460ff166003811115610c1457fe5b14610c1e57600080fd5b600060038233868689604051602001808673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b81526014018573ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b8152601401846bffffffffffffffffffffffff19166bffffffffffffffffffffffff191681526014018373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b8152601401828152602001955050505050506040516020818303038152906040526040518082805190602001908083835b60208310610d405780518252602082019150602081019050602083039250610d1d565b6001836020036101000a038019825116818451168082178552505050505050905001915050602060405180830381855afa158015610d82573d6000803e3d6000fd5b5050506040515160601b905060008087815260200190815260200160002060000160009054906101000a900460601b6bffffffffffffffffffffffff1916816bffffffffffffffffffffffff1916148015610e10575060008087815260200190815260200160002060000160149054906101000a900467ffffffffffffffff1667ffffffffffffffff164210155b610e1957600080fd5b6003600080888152602001908152602001600020600001601c6101000a81548160ff02191690836003811115610e4b57fe5b0217905550600073ffffffffffffffffffffffffffffffffffffffff168373ffffffffffffffffffffffffffffffffffffffff161415610ed1573373ffffffffffffffffffffffffffffffffffffffff166108fc869081150290604051600060405180830381858888f19350505050158015610ecb573d6000803e3d6000fd5b50610fa3565b60008390508073ffffffffffffffffffffffffffffffffffffffff1663a9059cbb33886040518363ffffffff1660e01b8152600401808373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff16815260200182815260200192505050602060405180830381600087803b158015610f5d57600080fd5b505af1158015610f71573d6000803e3d6000fd5b505050506040513d6020811015610f8757600080fd5b8101908080519060200190929190505050610fa157600080fd5b505b7f1797d500133f8e427eb9da9523aa4a25cb40f50ebc7dbda3c7c81778973f35ba866040518082815260200191505060405180910390a1505050505050565b600073ffffffffffffffffffffffffffffffffffffffff168373ffffffffffffffffffffffffffffffffffffffff161415801561101f5750600085115b801561106357506000600381111561103357fe5b600080888152602001908152602001600020600001601c9054906101000a900460ff16600381111561106157fe5b145b61106c57600080fd5b60006003843385888a604051602001808673ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b81526014018573ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b8152601401846bffffffffffffffffffffffff19166bffffffffffffffffffffffff191681526014018373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1660601b8152601401828152602001955050505050506040516020818303038152906040526040518082805190602001908083835b6020831061118e578051825260208201915060208101905060208303925061116b565b6001836020036101000a038019825116818451168082178552505050505050905001915050602060405180830381855afa1580156111d0573d6000803e3d6000fd5b5050506040515160601b90506040518060600160405280826bffffffffffffffffffffffff191681526020018367ffffffffffffffff1681526020016001600381111561121957fe5b81525060008089815260200190815260200160002060008201518160000160006101000a81548173ffffffffffffffffffffffffffffffffffffffff021916908360601c021790555060208201518160000160146101000a81548167ffffffffffffffff021916908367ffffffffffffffff160217905550604082015181600001601c6101000a81548160ff021916908360038111156112b557fe5b021790555090505060008590508073ffffffffffffffffffffffffffffffffffffffff166323b872dd33308a6040518463ffffffff1660e01b8152600401808473ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020018373ffffffffffffffffffffffffffffffffffffffff1673ffffffffffffffffffffffffffffffffffffffff1681526020018281526020019350505050602060405180830381600087803b15801561137d57600080fd5b505af1158015611391573d6000803e3d6000fd5b505050506040513d60208110156113a757600080fd5b81019080805190602001909291905050506113c157600080fd5b7fccc9c05183599bd3135da606eaaf535daffe256e9de33c048014cffcccd4ad57886040518082815260200191505060405180910390a1505050505050505056fea265627a7a723158208c83db436905afce0b7be1012be64818c49323c12d451fe2ab6bce76ff6421c964736f6c63430005110032";

/// Docker ops for Qtum initialization.
/// Used to create contracts and configure the Qtum node.
pub struct QtumDockerOps {
    #[allow(dead_code)]
    ctx: MmArc,
    coin: QtumCoin,
}

impl CoinDockerOps for QtumDockerOps {
    fn rpc_client(&self) -> &UtxoRpcClientEnum {
        &self.coin.as_ref().rpc_client
    }
}

impl QtumDockerOps {
    pub fn new() -> QtumDockerOps {
        let ctx = MmCtxBuilder::new().into_mm_arc();
        let confpath = qtum_conf_path();
        let conf = json!({"coin":"QTUM","decimals":8,"network":"regtest","confpath":confpath});
        let req = json!({
            "method": "enable",
        });
        let priv_key = Secp256k1Secret::from("809465b17d0a4ddb3e4c69e8f23c2cabad868f51f8bed5c765ad1d6516c3306f");
        let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
        let coin = block_on(qtum_coin_with_priv_key(&ctx, "QTUM", &conf, &params, priv_key)).unwrap();
        QtumDockerOps { ctx, coin }
    }

    pub fn initialize_contracts(&self) {
        let sender = get_address_by_label(&self.coin, QTUM_ADDRESS_LABEL);
        set_qick_token_address(self.create_contract(&sender, QRC20_TOKEN_BYTES));
        set_qorty_token_address(self.create_contract(&sender, QRC20_TOKEN_BYTES));
        set_qrc20_swap_contract_address(self.create_contract(&sender, QRC20_SWAP_CONTRACT_BYTES));
    }

    fn create_contract(&self, sender: &str, hexbytes: &str) -> H160Eth {
        let bytecode = hex::decode(hexbytes).expect("Hex encoded bytes expected");
        let gas_limit = 2_500_000u64;
        let gas_price = BigDecimal::from_str("0.0000004").unwrap();

        match self.coin.as_ref().rpc_client {
            UtxoRpcClientEnum::Native(ref native) => {
                let result = block_on_f01(native.create_contract(&bytecode.into(), gas_limit, gas_price, sender))
                    .expect("!createcontract");
                result.address.0.into()
            },
            UtxoRpcClientEnum::Electrum(_) => panic!("Native client expected"),
        }
    }
}

// =============================================================================
// Docker node setup
// =============================================================================

/// Start a Qtum regtest docker node and initialize configuration.
pub fn qtum_docker_node(port: u16) -> DockerNode {
    let image = GenericImage::new(QTUM_REGTEST_DOCKER_IMAGE, "latest")
        .with_env_var("CLIENTS", "2")
        .with_env_var("COIN_RPC_PORT", port.to_string())
        .with_env_var("ADDRESS_LABEL", QTUM_ADDRESS_LABEL)
        .with_env_var("FILL_MEMPOOL", "true")
        .with_wait_for(WaitFor::message_on_stdout("config is ready"));
    let image = RunnableImage::from(image).with_mapped_port((port, port));
    let container = image.start().expect("Failed to start Qtum regtest docker container");

    let name = "qtum";
    let mut conf_path = temp_dir().join("qtum-regtest");
    std::fs::create_dir_all(&conf_path).unwrap();
    conf_path.push(format!("{name}.conf"));
    Command::new("docker")
        .arg("cp")
        .arg(format!("{}:/data/node_0/{}.conf", container.id(), name))
        .arg(&conf_path)
        .status()
        .expect("Failed to execute docker command");
    let timeout = wait_until_ms(3000);
    loop {
        if conf_path.exists() {
            break;
        };
        assert!(now_ms() < timeout, "Test timed out");
    }

    set_qtum_conf_path(conf_path);
    DockerNode {
        container,
        ticker: name.to_owned(),
        port,
    }
}
