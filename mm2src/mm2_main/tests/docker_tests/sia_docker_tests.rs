//! TODO: These tests have nothing to do with SiaCoin and should rather be in `sia-rust` repo instead.

use common::block_on;
use sia_rust::transport::client::{
    error::ClientError as SiaApiClientError, ApiClient, ApiClientHelpers, Client as SiaApiClient, Conf as SiaHttpConf,
};
use sia_rust::transport::endpoints::{
    AddressBalanceRequest, ConsensusTipRequest, GetAddressUtxosRequest, TxpoolBroadcastRequest,
};
use sia_rust::types::{Address, Currency, Keypair, SiacoinOutput, SpendPolicy};
use sia_rust::utils::V2TransactionBuilder;
use std::str::FromStr;
use url::Url;

#[test]
fn test_sia_new_client() {
    let conf = SiaHttpConf {
        server_url: Url::parse("http://localhost:9980/").unwrap(),
        password: Some("password".to_string()),
        timeout: None,
    };
    let _api_client = block_on(SiaApiClient::new(conf)).unwrap();
}

#[test]
fn test_sia_client_bad_auth() {
    let conf = SiaHttpConf {
        server_url: Url::parse("http://localhost:9980/").unwrap(),
        password: Some("foo".to_string()),
        timeout: None,
    };
    let result = block_on(SiaApiClient::new(conf));
    let Err(error) = result else {
        panic!("Expected error but got success");
    };

    match error {
        SiaApiClientError::PingServer(nested_error) => match *nested_error {
            SiaApiClientError::DispatcherUnexpectedStatus { status, .. } => {
                assert_eq!(status, http::StatusCode::UNAUTHORIZED);
            },
            different_error => panic!(
                "Unexpected DispatcherUnexpectedStatus error, got: {:?}",
                different_error
            ),
        },
        different_error => panic!("Expected PingServer error, got: {:?}", different_error),
    }
}

#[test]
fn test_sia_client_consensus_tip() {
    let conf = SiaHttpConf {
        server_url: Url::parse("http://localhost:9980/").unwrap(),
        password: Some("password".to_string()),
        timeout: None,
    };
    let api_client = block_on(SiaApiClient::new(conf)).unwrap();
    let _response = block_on(api_client.dispatcher(ConsensusTipRequest)).unwrap();
}

// This test likely needs to be removed because mine_blocks has possibility of interfering with other async tests
// related to block height
#[test]
fn test_sia_client_address_balance() {
    let conf = SiaHttpConf {
        server_url: Url::parse("http://localhost:9980/").unwrap(),
        password: Some("password".to_string()),
        timeout: None,
    };
    let api_client = block_on(SiaApiClient::new(conf)).unwrap();

    let address =
        Address::from_str("591fcf237f8854b5653d1ac84ae4c107b37f148c3c7b413f292d48db0c25a8840be0653e411f").unwrap();
    block_on(api_client.mine_blocks(10, &address)).unwrap();

    let request = AddressBalanceRequest { address };
    let response = block_on(api_client.dispatcher(request)).unwrap();

    // It's hard to predict how much was mined to this address while other tests are also mining in the same network.
    // Looks like the halving happens so quickly and the sum of mined coins change between different test runs.
    // Just make sure we at least mined something.
    assert!(response.immature_siacoins + response.siacoins > Currency(0));
}

#[test]
fn test_sia_client_build_tx() {
    let conf = SiaHttpConf {
        server_url: Url::parse("http://localhost:9980/").unwrap(),
        password: Some("password".to_string()),
        timeout: None,
    };
    let api_client = block_on(SiaApiClient::new(conf)).unwrap();
    let keypair = Keypair::from_private_bytes(
        &hex::decode("0100000000000000000000000000000000000000000000000000000000000000").unwrap(),
    )
    .unwrap();
    let spend_policy = SpendPolicy::PublicKey(keypair.public());

    let address = spend_policy.address();

    block_on(api_client.mine_blocks(201, &address)).unwrap();

    let utxos = block_on(api_client.dispatcher(GetAddressUtxosRequest {
        address: address.clone(),
        limit: None,
        offset: None,
        include_mempool: true,
    }))
    .unwrap();
    let spend_this = utxos.outputs[0].clone();
    let vin = spend_this.clone();
    println!("utxo[0]: {spend_this:?}");
    let vout = SiacoinOutput {
        value: spend_this.siacoin_output.value,
        address,
    };
    let tx = V2TransactionBuilder::new()
        .add_siacoin_input(vin, spend_policy)
        .add_siacoin_output(vout)
        .sign_simple(vec![&keypair])
        .update_basis(utxos.basis.clone())
        .build();

    let req = TxpoolBroadcastRequest {
        basis: utxos.basis,
        transactions: vec![],
        v2transactions: vec![tx],
    };

    let _response = block_on(api_client.dispatcher(req)).unwrap();
}
