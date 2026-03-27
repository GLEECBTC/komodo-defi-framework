//! Integration tests for TRON API client using Nile testnet.
//!
//! These tests make real network calls to the TRON Nile testnet.
//! They are gated behind the `tron-network-tests` feature to avoid running
//! during regular test runs.
//!
//! # Running the tests
//!
//! ```bash
//! # Run all TRON Nile integration tests (native)
//! cargo test -p coins --features tron-network-tests --lib tron_nile
//!
//! # Run a specific test
//! cargo test -p coins --features tron-network-tests --lib tron_nile_current_block
//!
//! # Override API nodes (optional, native only)
//! TRON_NILE_API_URLS="https://nile.trongrid.io" cargo test -p coins --features tron-network-tests --lib tron_nile
//! ```
//!
//! # WASM tests
//!
//! WASM tests require a browser runner because `mm2_net`'s WASM HTTP transport uses
//! `Window`/`Worker` fetch and doesn't support Node.js. Run with:
//!
//! ```bash
//! wasm-pack test --headless --firefox mm2src/coins --features tron-network-tests -- tron_nile
//! ```
//!
//! See `docs/DEV_ENVIRONMENT.md` for browser driver setup (geckodriver, environment variables).

use super::api::{TronApiClient, TronHttpClient, TronHttpNode};
use super::TronAddress;
use crate::eth::chain_rpc::ChainRpcOps;
use crate::eth::Web3RpcError;
use common::executor::Timer;
use common::{cross_test, small_rng};
use ethereum_types::Address as EthAddress;
use http::Uri;
use mm2_test_helpers::for_tests::{TRON_NILE_NODES, TRON_TESTNET_KNOWN_ADDRESS};
use rand::RngCore;
use std::convert::TryInto;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen_test::*;

#[cfg(target_arch = "wasm32")]
wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_browser);

/// Get TRON Nile API URLs from environment or use defaults.
fn tron_nile_urls() -> Vec<Uri> {
    #[cfg(not(target_arch = "wasm32"))]
    let from_env = std::env::var("TRON_NILE_API_URLS").ok();
    #[cfg(target_arch = "wasm32")]
    let from_env: Option<String> = None;

    let raw_urls: Vec<String> = if let Some(s) = from_env {
        s.split([',', ' '])
            .map(str::trim)
            .filter(|x| !x.is_empty())
            .map(ToOwned::to_owned)
            .collect()
    } else {
        TRON_NILE_NODES.iter().map(|s| s.to_string()).collect()
    };

    raw_urls
        .into_iter()
        .map(|url| url.parse().expect("Invalid TRON API URL"))
        .collect()
}

/// Create a TronApiClient for Nile testnet.
fn tron_nile_api_client() -> TronApiClient {
    let uris = tron_nile_urls();
    let clients = uris
        .into_iter()
        .map(|uri| {
            TronHttpClient::new(
                TronHttpNode {
                    uri,
                    komodo_proxy: false,
                },
                None,
            )
        })
        .collect();
    TronApiClient::new(clients)
}

/// Parse a TRON base58 address to TronAddress.
fn parse_tron_address(base58: &str) -> TronAddress {
    TronAddress::from_base58(base58).expect("Invalid TRON address")
}

/// Generate a random TRON address for testing unused address scenarios.
fn random_tron_address() -> TronAddress {
    let mut rng = small_rng();
    let mut addr_bytes = [0u8; 20];
    rng.fill_bytes(&mut addr_bytes);
    let eth_addr = EthAddress::from_slice(&addr_bytes);
    TronAddress::from(&eth_addr)
}

/// Create a TronApiClient with a failing node first, then working nodes.
/// This tests the retry/failover behavior.
fn tron_nile_api_client_with_failing_node_first() -> TronApiClient {
    let bad_uri: Uri = "http://127.0.0.1:1".parse().expect("Invalid bad URI");
    let good_uris = tron_nile_urls();

    // Put the bad node first, so the client must retry on good nodes
    let mut all_clients = vec![TronHttpClient::new(
        TronHttpNode {
            uri: bad_uri,
            komodo_proxy: false,
        },
        None,
    )];

    all_clients.extend(good_uris.into_iter().map(|uri| {
        TronHttpClient::new(
            TronHttpNode {
                uri,
                komodo_proxy: false,
            },
            None,
        )
    }));

    TronApiClient::new(all_clients)
}

/// Create a TronApiClient with only failing nodes.
/// This tests the transport error handling when all nodes fail.
fn tron_nile_api_client_all_failing() -> TronApiClient {
    let bad_uris: Vec<Uri> = vec![
        "http://127.0.0.1:1".parse().unwrap(),
        "http://127.0.0.1:2".parse().unwrap(),
    ];

    let clients = bad_uris
        .into_iter()
        .map(|uri| {
            TronHttpClient::new(
                TronHttpNode {
                    uri,
                    komodo_proxy: false,
                },
                None,
            )
        })
        .collect();

    TronApiClient::new(clients)
}

// ============================================================================
// Test Implementation Functions
// ============================================================================

async fn test_get_now_block_number_impl() {
    let client = tron_nile_api_client();
    let block_number = client.current_block().await.unwrap();

    // Nile testnet should have millions of blocks by now
    assert!(
        block_number > 0,
        "Block number should be positive, got {}",
        block_number
    );
    assert!(
        block_number > 1_000_000,
        "Nile testnet should have more than 1M blocks, got {}",
        block_number
    );
}

async fn test_block_number_non_decreasing_impl() {
    let client = tron_nile_api_client();

    let block1 = client.current_block().await.unwrap();
    // Small delay between calls (cross-platform)
    Timer::sleep(0.1).await;
    let block2 = client.current_block().await.unwrap();

    assert!(
        block2 >= block1,
        "Block number should not decrease: {} -> {}",
        block1,
        block2
    );
}

async fn test_is_address_used_known_impl() {
    let client = tron_nile_api_client();
    let address = parse_tron_address(TRON_TESTNET_KNOWN_ADDRESS);

    let is_used = client.is_address_used_basic(address).await.unwrap();

    assert!(
        is_used,
        "Known testnet address {} should be marked as used",
        TRON_TESTNET_KNOWN_ADDRESS
    );
}

async fn test_is_address_used_unused_impl() {
    let client = tron_nile_api_client();
    let address = random_tron_address();

    let is_used = client.is_address_used_basic(address).await.unwrap();

    assert!(!is_used, "Random address should not be marked as used");
}

async fn test_balance_native_impl() {
    let client = tron_nile_api_client();
    let address = parse_tron_address(TRON_TESTNET_KNOWN_ADDRESS);

    let balance = client.balance_native(address).await.unwrap();

    assert!(
        balance > ethereum_types::U256::zero(),
        "Known testnet address {} should have non-zero TRX balance",
        TRON_TESTNET_KNOWN_ADDRESS
    );
}

async fn test_get_block_for_tapos_impl() {
    let client = tron_nile_api_client();
    let tapos = client.get_block_for_tapos().await.unwrap();

    assert!(
        tapos.number > 1_000_000,
        "Nile testnet should have more than 1M blocks, got {}",
        tapos.number
    );
    assert!(
        tapos.timestamp > 0,
        "Block timestamp should be positive, got {}",
        tapos.timestamp
    );

    // blockID first 8 bytes encode the block number in big-endian
    let number_from_id = u64::from_be_bytes(tapos.block_id[..8].try_into().unwrap());
    assert_eq!(
        number_from_id, tapos.number,
        "Block number in blockID should match block_header.raw_data.number"
    );
}

// ============================================================================
// Cross-Platform Integration Tests
// ============================================================================

cross_test!(tron_nile_current_block, {
    test_get_now_block_number_impl().await;
});

cross_test!(tron_nile_block_number_non_decreasing, {
    test_block_number_non_decreasing_impl().await;
});

cross_test!(tron_nile_is_address_used_known, {
    test_is_address_used_known_impl().await;
});

cross_test!(tron_nile_is_address_used_unused, {
    test_is_address_used_unused_impl().await;
});

cross_test!(tron_nile_balance_native, {
    test_balance_native_impl().await;
});

cross_test!(tron_nile_get_block_for_tapos, {
    test_get_block_for_tapos_impl().await;
});

// ============================================================================
// Error Response Tests
// ============================================================================
// These tests verify that our error detection handles real TRON API error responses.

use serde::{Deserialize, Serialize};

/// Create a single TronHttpClient for error tests (no rotation needed).
fn tron_nile_single_client() -> TronHttpClient {
    let uri = tron_nile_urls()
        .into_iter()
        .next()
        .expect("At least one TRON node expected");
    TronHttpClient::new(
        TronHttpNode {
            uri,
            komodo_proxy: false,
        },
        None,
    )
}

async fn test_error_nested_result_detection_impl() {
    // Call triggerconstantcontract with a non-existent contract address
    // This tests our nested result error detection:
    // {"result": {"result": false, "code": "CONTRACT_VALIDATE_ERROR", "message": "..."}}
    let client = tron_nile_single_client();

    let owner = parse_tron_address(TRON_TESTNET_KNOWN_ADDRESS);
    // Use a random address that is definitely not a contract
    let non_contract = random_tron_address();

    // Uses the public trigger_constant_contract method (same post() + error detection path)
    let result = client
        .trigger_constant_contract(
            &owner,
            &non_contract,
            "balanceOf(address)",
            "0000000000000000000000000000000000000000000000000000000000000001",
        )
        .await;

    // Should be an error because our error detection catches nested {"result": {"result": false, ...}}
    assert!(result.is_err(), "Expected error for non-existent contract");

    let err = result.unwrap_err().into_inner();

    // Verify the error is a RemoteError with CONTRACT_VALIDATE_ERROR code
    match err {
        Web3RpcError::RemoteError { code, message } => {
            assert_eq!(code.as_deref(), Some("CONTRACT_VALIDATE_ERROR"));
            assert!(
                message.contains("Smart contract"),
                "Expected message to contain 'Smart contract', got: {}",
                message
            );
        },
        other => panic!("Expected RemoteError, got {:?}", other),
    }
}

async fn test_error_invalid_endpoint_impl() {
    // Call a non-existent endpoint to test HTTP error handling
    let client = tron_nile_single_client();

    #[derive(Serialize)]
    struct EmptyRequest {}

    #[derive(Deserialize, Debug)]
    struct AnyResponse {}

    let result: Result<AnyResponse, _> = client
        .post("/wallet/nonexistent_endpoint_12345", &EmptyRequest {})
        .await;

    let err = result.unwrap_err().into_inner();
    match err {
        Web3RpcError::Transport(msg) => {
            // 405 Method Not Allowed are returned for non-existent endpoints
            assert!(
                msg.starts_with("TRON API returned status 405"),
                "Expected HTTP 405 status error, got: {}",
                msg
            );
        },
        other => panic!("Expected Web3RpcError::Transport, got {:?}", other),
    }
}

async fn test_error_empty_response_handling_impl() {
    // Verify that an empty account response (non-existent account) is handled correctly
    // and does NOT trigger our error detection (since {} is a valid "account not found" response)
    let client = tron_nile_api_client();
    let address = random_tron_address();

    // This should succeed and return an empty account, not an error
    let result = client.get_account(&address).await;
    assert!(result.is_ok(), "Empty account response should not be treated as error");

    let account = result.unwrap();
    assert!(
        !account.exists_meaningfully(),
        "Random address should return non-existent account"
    );
}

cross_test!(tron_nile_error_nested_result_detection, {
    test_error_nested_result_detection_impl().await;
});

cross_test!(tron_nile_error_invalid_endpoint, {
    test_error_invalid_endpoint_impl().await;
});

cross_test!(tron_nile_error_empty_response_handling, {
    test_error_empty_response_handling_impl().await;
});

// ============================================================================
// Fee Validation Tests (TRC20)
// ============================================================================

const NILE_KNOWN_TRC20_TX_HASH: &str = "b4eaf9c10802e20ad757c701fca45616c71fa68c84dea4110f6772005a480fa4";
const NILE_KNOWN_TRC20_BLOCK_NUMBER: u64 = 64_844_180;
const NILE_KNOWN_TRC20_CONTRACT_ADDRESS: &str = "41eca9bc828a3005b9a3b909f2cc5c2a54794de05f";
const NILE_KNOWN_TRC20_TRANSFER_SELECTOR: &str = "a9059cbb";
const NILE_KNOWN_TRC20_FEE_SUN: u64 = 345_000;

async fn test_known_trc20_tx_fee_receipt_impl() {
    let client = tron_nile_single_client();

    let tx = client
        .get_transaction_by_id(NILE_KNOWN_TRC20_TX_HASH)
        .await
        .expect("Known TRC20 transaction should be available on Nile");
    assert_eq!(tx.tx_id, NILE_KNOWN_TRC20_TX_HASH);

    let first_contract = tx
        .raw_data
        .contract
        .first()
        .expect("Known TRC20 transaction should contain at least one contract");

    assert_eq!(first_contract.contract_type, "TriggerSmartContract");
    assert_eq!(
        first_contract.parameter.value.contract_address.as_deref(),
        Some(NILE_KNOWN_TRC20_CONTRACT_ADDRESS)
    );

    let data = first_contract
        .parameter
        .value
        .data
        .as_deref()
        .expect("TriggerSmartContract data must be present");
    assert!(
        data.starts_with(NILE_KNOWN_TRC20_TRANSFER_SELECTOR),
        "Expected TRC20 transfer selector prefix {}, got {}",
        NILE_KNOWN_TRC20_TRANSFER_SELECTOR,
        data
    );

    let tx_info = client
        .get_transaction_info_by_id(NILE_KNOWN_TRC20_TX_HASH)
        .await
        .expect("Known TRC20 transaction info should be available on Nile");
    assert_eq!(tx_info.id, NILE_KNOWN_TRC20_TX_HASH);
    assert_eq!(tx_info.block_number, NILE_KNOWN_TRC20_BLOCK_NUMBER);
    assert_eq!(tx_info.receipt.result, "SUCCESS");
    assert!(tx_info.receipt.energy_usage_total > 0);
    assert_eq!(tx_info.receipt.energy_fee, 0);
    assert_eq!(tx_info.receipt.net_fee, NILE_KNOWN_TRC20_FEE_SUN);
    assert_eq!(tx_info.fee.unwrap_or_default(), NILE_KNOWN_TRC20_FEE_SUN);
}

async fn test_chain_fee_parameters_are_present_and_valid_impl() {
    let client = tron_nile_api_client();
    let chain_prices = client
        .get_chain_prices()
        .await
        .expect("getchainparameters should be available and valid on Nile");

    assert!(chain_prices.bandwidth_price_sun > 0);
    assert!(chain_prices.energy_price_sun > 0);
    // Account creation fees should be present on Nile testnet.
    // These are governance-set params; on Nile they mirror mainnet values.
    assert!(
        chain_prices.create_new_account_fee_sun > 0,
        "Nile should have non-zero CreateNewAccountFeeInSystemContract"
    );
    assert!(
        chain_prices.create_account_bandwidth_fee_sun > 0,
        "Nile should have non-zero CreateAccountFee"
    );
    assert!(
        chain_prices.create_new_account_bandwidth_rate > 0,
        "Nile should have non-zero CreateNewAccountBandwidthRate"
    );
}

cross_test!(tron_nile_known_trc20_tx_fee_receipt, {
    test_known_trc20_tx_fee_receipt_impl().await;
});

cross_test!(tron_nile_chain_fee_parameters_are_present_and_valid, {
    test_chain_fee_parameters_are_present_and_valid_impl().await;
});

// ============================================================================
// Node Rotation and Retry Tests
// ============================================================================
// These tests verify the retry/failover behavior when nodes fail.

async fn test_retry_on_transport_failure_impl() {
    // Create a client with a failing node first, then working nodes.
    // The request should succeed by retrying on the working nodes.
    let client = tron_nile_api_client_with_failing_node_first();

    // Should succeed by retrying on working nodes after transport failure
    let result = client.current_block().await;

    assert!(
        result.is_ok(),
        "Request should succeed by retrying on working nodes after transport failure: {:?}",
        result.err()
    );

    let block_number = result.unwrap();
    assert!(
        block_number > 1_000_000,
        "Should get a valid block number from the working node"
    );
}

async fn test_all_nodes_failing_returns_transport_error_impl() {
    // Create a client with only failing nodes.
    // The request should fail with a transport error after trying all nodes.
    let client = tron_nile_api_client_all_failing();

    let result = client.current_block().await;

    assert!(result.is_err(), "Request should fail when all nodes are unreachable");

    let error = result.unwrap_err();
    let inner = error.into_inner();

    // The last error should be a transport error (retryable)
    // since all failures were connection failures.
    assert!(
        inner.is_retryable(),
        "Final error should be Transport (retryable) when all nodes fail: {:?}",
        inner
    );

    assert!(
        matches!(inner, Web3RpcError::Transport(_)),
        "Expected Web3RpcError::Transport, got {:?}",
        inner
    );
}

cross_test!(tron_nile_retry_on_transport_failure, {
    test_retry_on_transport_failure_impl().await;
});

cross_test!(tron_nile_all_nodes_failing_returns_transport_error, {
    test_all_nodes_failing_returns_transport_error_impl().await;
});

// ============================================================================
// Account Resource Tests
// ============================================================================

async fn test_get_account_resource_known_address_impl() {
    let client = tron_nile_api_client();
    let address = parse_tron_address(TRON_TESTNET_KNOWN_ADDRESS);

    let resources = client
        .get_account_resource(&address)
        .await
        .expect("getaccountresource should succeed for known address");

    // Known testnet address should have at least the free bandwidth limit
    assert!(
        resources.free_net_limit > 0,
        "Known address should have non-zero freeNetLimit, got {}",
        resources.free_net_limit
    );
}

async fn test_get_account_resource_unactivated_address_impl() {
    let client = tron_nile_api_client();
    let address = random_tron_address();

    let resources = client
        .get_account_resource(&address)
        .await
        .expect("getaccountresource should succeed for unactivated address (empty {} response)");

    // Unactivated address returns empty {} which deserializes to all zeros
    assert_eq!(resources.free_net_used, 0);
    assert_eq!(resources.free_net_limit, 0);
    assert_eq!(resources.net_used, 0);
    assert_eq!(resources.net_limit, 0);
    assert_eq!(resources.energy_used, 0);
    assert_eq!(resources.energy_limit, 0);
}

cross_test!(tron_nile_get_account_resource_known_address, {
    test_get_account_resource_known_address_impl().await;
});

cross_test!(tron_nile_get_account_resource_unactivated_address, {
    test_get_account_resource_unactivated_address_impl().await;
});

// ============================================================================
// Account Creation Fee Integration Tests
// ============================================================================

async fn test_unactivated_address_detected_for_fee_estimation_impl() {
    let client = tron_nile_api_client();
    let random_addr = random_tron_address();

    // Random address should be unactivated
    let account = client
        .get_account(&random_addr)
        .await
        .expect("getaccount should succeed for random address");
    assert!(
        !account.exists_meaningfully(),
        "random address should not be activated on Nile"
    );

    // Chain prices should include account creation fee params
    let prices = client
        .get_chain_prices()
        .await
        .expect("getchainparameters should succeed");
    assert!(
        prices.create_new_account_fee_sun > 0,
        "CreateNewAccountFeeInSystemContract should be set on Nile"
    );
}

cross_test!(tron_nile_unactivated_address_detected_for_fee_estimation, {
    test_unactivated_address_detected_for_fee_estimation_impl().await;
});
