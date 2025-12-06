//! Shared ETH/ERC20 helper functions for docker tests.
//!
//! This module provides address getters, funding utilities, and coin creation helpers
//! that are used across multiple test modules. These helpers wrap the GETH global statics
//! and provide safe, convenient access to test infrastructure.

use super::super::docker_tests_common::{
    random_secp256k1_secret, GETH_ACCOUNT, GETH_ERC20_CONTRACT, GETH_NONCE_LOCK, GETH_RPC_URL, GETH_SWAP_CONTRACT,
    GETH_WATCHERS_SWAP_CONTRACT, GETH_WEB3, MM_CTX,
};
use coins::eth::{checksum_address, eth_coin_from_conf_and_request, EthCoin, ERC20_ABI};
use coins::{CoinProtocol, CoinWithDerivationMethod, DerivationMethod, PrivKeyBuildPolicy};
use common::block_on;
use crypto::Secp256k1Secret;
use ethereum_types::U256;
use mm2_test_helpers::for_tests::{erc20_dev_conf, eth_dev_conf};
use std::thread;
use std::time::Duration;
use web3::contract::{Contract, Options};
use web3::ethabi::Token;
use web3::types::{Address, TransactionRequest, H256};

/// Geth dev chain ID used for testing
pub const GETH_DEV_CHAIN_ID: u64 = 1337;

// =============================================================================
// Address getters - wrap unsafe statics for safe access
// =============================================================================

/// # Safety
///
/// GETH_ACCOUNT is set once during initialization before tests start
pub fn geth_account() -> Address {
    unsafe { GETH_ACCOUNT }
}

/// # Safety
///
/// GETH_SWAP_CONTRACT is set once during initialization before tests start
pub fn swap_contract() -> Address {
    unsafe { GETH_SWAP_CONTRACT }
}

/// # Safety
///
/// GETH_WATCHERS_SWAP_CONTRACT is set once during initialization before tests start
pub fn watchers_swap_contract() -> Address {
    unsafe { GETH_WATCHERS_SWAP_CONTRACT }
}

/// # Safety
///
/// GETH_ERC20_CONTRACT is set once during initialization before tests start
pub fn erc20_contract() -> Address {
    unsafe { GETH_ERC20_CONTRACT }
}

/// Return ERC20 dev token contract address in checksum format
pub fn erc20_contract_checksum() -> String {
    checksum_address(&format!("{:02x}", erc20_contract()))
}

/// Return swap contract address in checksum format (with 0x prefix)
pub fn swap_contract_checksum() -> String {
    checksum_address(&format!("{:02x}", swap_contract()))
}

/// Return watchers swap contract address in checksum format (with 0x prefix)
pub fn watchers_swap_contract_checksum() -> String {
    checksum_address(&format!("{:02x}", watchers_swap_contract()))
}

// =============================================================================
// Funding utilities - fill test wallets with ETH and tokens
// =============================================================================

fn wait_for_confirmation(tx_hash: H256) {
    thread::sleep(Duration::from_millis(2000));
    loop {
        match block_on(GETH_WEB3.eth().transaction_receipt(tx_hash)) {
            Ok(Some(r)) => match r.block_hash {
                Some(_) => break,
                None => thread::sleep(Duration::from_millis(100)),
            },
            _ => {
                thread::sleep(Duration::from_millis(100));
            },
        }
    }
}

/// Fill an address with ETH from the Geth coinbase account
pub fn fill_eth(to_addr: Address, amount: U256) {
    let _guard = GETH_NONCE_LOCK.lock().unwrap();
    let tx_request = TransactionRequest {
        from: geth_account(),
        to: Some(to_addr),
        gas: None,
        gas_price: None,
        value: Some(amount),
        data: None,
        nonce: None,
        condition: None,
        transaction_type: None,
        access_list: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
    };
    let tx_hash = block_on(GETH_WEB3.eth().send_transaction(tx_request)).unwrap();
    wait_for_confirmation(tx_hash);
}

/// Fill an address with ERC20 tokens from the Geth coinbase account
pub fn fill_erc20(to_addr: Address, amount: U256) {
    let _guard = GETH_NONCE_LOCK.lock().unwrap();
    let erc20 = Contract::from_json(GETH_WEB3.eth(), erc20_contract(), ERC20_ABI.as_bytes()).unwrap();

    let tx_hash = block_on(erc20.call(
        "transfer",
        (Token::Address(to_addr), Token::Uint(amount)),
        geth_account(),
        Options::default(),
    ))
    .unwrap();
    wait_for_confirmation(tx_hash);
}

// =============================================================================
// Coin creation utilities - create test coins with random keys
// =============================================================================

/// Creates ETH protocol coin supplied with 100 ETH
pub fn eth_coin_with_random_privkey_using_urls(swap_contract_address: Address, urls: &[&str]) -> EthCoin {
    let eth_conf = eth_dev_conf();
    let req = json!({
        "method": "enable",
        "coin": "ETH",
        "swap_contract_address": swap_contract_address,
        "urls": urls,
    });

    let secret = random_secp256k1_secret();
    let eth_coin = block_on(eth_coin_from_conf_and_request(
        &MM_CTX,
        "ETH",
        &eth_conf,
        &req,
        CoinProtocol::ETH {
            chain_id: GETH_DEV_CHAIN_ID,
        },
        PrivKeyBuildPolicy::IguanaPrivKey(secret),
    ))
    .unwrap();

    let my_address = match eth_coin.derivation_method() {
        DerivationMethod::SingleAddress(addr) => *addr,
        _ => panic!("Expected single address"),
    };

    // 100 ETH
    fill_eth(my_address, U256::from(10).pow(U256::from(20)));

    eth_coin
}

/// Creates ETH protocol coin supplied with 100 ETH, using the default GETH_RPC_URL
pub fn eth_coin_with_random_privkey(swap_contract_address: Address) -> EthCoin {
    eth_coin_with_random_privkey_using_urls(swap_contract_address, &[GETH_RPC_URL])
}

/// Creates ERC20 protocol coin supplied with 1 ETH and 100 tokens
pub fn erc20_coin_with_random_privkey(swap_contract_address: Address) -> EthCoin {
    let erc20_conf = erc20_dev_conf(&erc20_contract_checksum());
    let req = json!({
        "method": "enable",
        "coin": "ERC20DEV",
        "swap_contract_address": swap_contract_address,
        "urls": [GETH_RPC_URL],
    });

    let erc20_coin = block_on(eth_coin_from_conf_and_request(
        &MM_CTX,
        "ERC20DEV",
        &erc20_conf,
        &req,
        CoinProtocol::ERC20 {
            platform: "ETH".to_string(),
            contract_address: checksum_address(&format!("{:02x}", erc20_contract())),
        },
        PrivKeyBuildPolicy::IguanaPrivKey(random_secp256k1_secret()),
    ))
    .unwrap();

    let my_address = match erc20_coin.derivation_method() {
        DerivationMethod::SingleAddress(addr) => *addr,
        _ => panic!("Expected single address"),
    };

    // 1 ETH
    fill_eth(my_address, U256::from(10).pow(U256::from(18)));
    // 100 tokens (it has 8 decimals)
    fill_erc20(my_address, U256::from(10000000000u64));

    erc20_coin
}

/// Fills the private key's public address with ETH and ERC20 tokens
pub fn fill_eth_erc20_with_private_key(priv_key: Secp256k1Secret) {
    let eth_conf = eth_dev_conf();
    let req = json!({
        "coin": "ETH",
        "urls": [GETH_RPC_URL],
        "swap_contract_address": swap_contract(),
    });

    let eth_coin = block_on(eth_coin_from_conf_and_request(
        &MM_CTX,
        "ETH",
        &eth_conf,
        &req,
        CoinProtocol::ETH {
            chain_id: GETH_DEV_CHAIN_ID,
        },
        PrivKeyBuildPolicy::IguanaPrivKey(priv_key),
    ))
    .unwrap();
    let my_address = block_on(eth_coin.derivation_method().single_addr_or_err()).unwrap();

    // 100 ETH
    fill_eth(my_address, U256::from(10).pow(U256::from(20)));

    let erc20_conf = erc20_dev_conf(&erc20_contract_checksum());
    let req = json!({
        "method": "enable",
        "coin": "ERC20DEV",
        "urls": [GETH_RPC_URL],
        "swap_contract_address": swap_contract(),
    });

    let _erc20_coin = block_on(eth_coin_from_conf_and_request(
        &MM_CTX,
        "ERC20DEV",
        &erc20_conf,
        &req,
        CoinProtocol::ERC20 {
            platform: "ETH".to_string(),
            contract_address: erc20_contract_checksum(),
        },
        PrivKeyBuildPolicy::IguanaPrivKey(priv_key),
    ))
    .unwrap();

    // 100 tokens (it has 8 decimals)
    fill_erc20(my_address, U256::from(10000000000u64));
}
