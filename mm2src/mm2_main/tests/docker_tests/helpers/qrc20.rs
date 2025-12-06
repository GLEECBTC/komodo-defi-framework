//! Qtum/QRC20 helpers for docker tests.
//!
//! This module provides:
//! - QRC20 coin creation and funding utilities
//! - Qtum docker node helpers
//! - QRC20 contract initialization

use crate::docker_tests::helpers::env::{random_secp256k1_secret, Secp256k1Secret};
use crate::docker_tests::helpers::locks::QTUM_LOCK;
use crate::docker_tests::helpers::utxo::fill_address;
use coins::qrc20::rpc_clients::for_tests::Qrc20NativeWalletOps;
use coins::qrc20::{qrc20_coin_with_priv_key, Qrc20ActivationParams, Qrc20Coin};
use coins::utxo::qtum::QtumBasedCoin;
use coins::utxo::qtum::{qtum_coin_with_priv_key, QtumCoin};
use coins::utxo::rpc_clients::{UtxoRpcClientEnum, UtxoRpcClientOps};
use coins::utxo::{sat_from_big_decimal, UtxoActivationParams, UtxoCoinFields};
use coins::{ConfirmPaymentInput, MarketCoinOps};
use common::{block_on, block_on_f01, now_sec, wait_until_sec};
use ethereum_types::H160 as H160Eth;
use http::StatusCode;
use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};
use mm2_number::BigDecimal;
use mm2_test_helpers::for_tests::MarketMakerIt;
use serde_json::{self as json, Value as Json};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

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
