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

// ============================================================================
// Error Response Tests
// ============================================================================
// These tests verify that our error detection handles real TRON API error responses.

use serde::Serialize;

/// Request for /wallet/triggerconstantcontract endpoint.
#[derive(Serialize)]
struct TriggerConstantContractRequest {
    owner_address: String,
    contract_address: String,
    function_selector: String,
    parameter: String,
    visible: bool,
}

/// Create a single TronHttpClient for error tests (no rotation needed).
fn tron_nile_single_client() -> TronHttpClient {
    let uri: Uri = TRON_NILE_NODES[0].parse().expect("Invalid TRON API URL");
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

    let request = TriggerConstantContractRequest {
        // Use a valid but non-contract address as owner
        owner_address: TRON_TESTNET_KNOWN_ADDRESS.to_string(),
        // Use a random address that is definitely not a contract
        contract_address: random_tron_address().to_base58(),
        function_selector: "balanceOf(address)".to_string(),
        parameter: "0000000000000000000000000000000000000000000000000000000000000001".to_string(),
        visible: true,
    };

    // Our error detection should now catch the nested result error
    let result: Result<serde_json::Value, _> = client.post("/wallet/triggerconstantcontract", &request).await;

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

    #[derive(Deserialize)]
    struct AnyResponse {}

    let result: Result<AnyResponse, _> = client
        .post("/wallet/nonexistent_endpoint_12345", &EmptyRequest {})
        .await;

    // Should return an error (either HTTP 404 or API error)
    assert!(result.is_err(), "Non-existent endpoint should return error");
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
}

cross_test!(tron_nile_retry_on_transport_failure, {
    test_retry_on_transport_failure_impl().await;
});

cross_test!(tron_nile_all_nodes_failing_returns_transport_error, {
    test_all_nodes_failing_returns_transport_error_impl().await;
});
