use common::block_on;
use sia_rust::http_client::{SiaApiClient, SiaApiClientError, SiaHttpConf};
use sia_rust::http_endpoints::{
    AddressBalanceRequest, AddressUtxosRequest, ConsensusTipRequest, TxpoolBroadcastRequest,
};
use sia_rust::spend_policy::SpendPolicy;
use sia_rust::transaction::{SiacoinOutput, V2TransactionBuilder};
use sia_rust::types::{Address, Currency};
use sia_rust::{Keypair, PublicKey, SecretKey};
use std::process::Command;
use std::str::FromStr;
use url::Url;

#[cfg(test)]
fn mine_blocks(n: u64, addr: &Address) {
    Command::new("docker")
        .arg("exec")
        .arg("sia-docker")
        .arg("walletd")
        .arg("mine")
        .arg(format!("-addr={addr}"))
        .arg(format!("-n={n}"))
        .status()
        .expect("Failed to execute docker command");
}

#[test]
fn test_sia_new_client() {
    let conf = SiaHttpConf {
        url: Url::parse("http://localhost:9980/").unwrap(),
        password: "password".to_string(),
    };
    let _api_client = block_on(SiaApiClient::new(conf)).unwrap();
}

#[test]
fn test_sia_client_bad_auth() {
    let conf = SiaHttpConf {
        url: Url::parse("http://localhost:9980/").unwrap(),
        password: "foo".to_string(),
    };
    let result = block_on(SiaApiClient::new(conf));
    assert!(matches!(result, Err(SiaApiClientError::UnexpectedHttpStatus(401))));
}

#[test]
fn test_sia_client_consensus_tip() {
    let conf = SiaHttpConf {
        url: Url::parse("http://localhost:9980/").unwrap(),
        password: "password".to_string(),
    };
    let api_client = block_on(SiaApiClient::new(conf)).unwrap();
    let _response = block_on(api_client.dispatcher(ConsensusTipRequest)).unwrap();
}

// This test likely needs to be removed because mine_blocks has possibility of interfering with other async tests
// related to block height
#[test]
fn test_sia_client_address_balance() {
    let conf = SiaHttpConf {
        url: Url::parse("http://localhost:9980/").unwrap(),
        password: "password".to_string(),
    };
    let api_client = block_on(SiaApiClient::new(conf)).unwrap();

    let address =
        Address::from_str("addr:591fcf237f8854b5653d1ac84ae4c107b37f148c3c7b413f292d48db0c25a8840be0653e411f").unwrap();
    mine_blocks(10, &address);

    let request = AddressBalanceRequest { address };
    let response = block_on(api_client.dispatcher(request)).unwrap();

    let expected = Currency::new(12919594847110692864, 54210108624275221);
    assert_eq!(response.siacoins, expected);
    assert_eq!(expected.to_u128(), 1000000000000000000000000000000000000);
}

#[test]
fn test_sia_client_build_tx() {
    let conf = SiaHttpConf {
        url: Url::parse("http://localhost:9980/").unwrap(),
        password: "password".to_string(),
    };
    let api_client = block_on(SiaApiClient::new(conf)).unwrap();
    let sk: SecretKey = SecretKey::from_bytes(
        &hex::decode("0100000000000000000000000000000000000000000000000000000000000000").unwrap(),
    )
    .unwrap();
    let pk: PublicKey = (&sk).into();
    let keypair = Keypair { public: pk, secret: sk };
    let spend_policy = SpendPolicy::PublicKey(pk);

    let address = spend_policy.address();

    mine_blocks(201, &address);

    let utxos = block_on(api_client.dispatcher(AddressUtxosRequest {
        address: address.clone(),
    }))
    .unwrap();
    let spend_this = utxos[0].clone();
    let vin = spend_this.clone();
    println!("utxo[0]: {spend_this:?}");
    let vout = SiacoinOutput {
        value: spend_this.siacoin_output.value,
        address,
    };
    let tx = V2TransactionBuilder::new(0u64.into())
        .add_siacoin_input(vin, spend_policy)
        .add_siacoin_output(vout)
        .sign_simple(vec![&keypair])
        .unwrap()
        .build();

    let req = TxpoolBroadcastRequest {
        transactions: vec![],
        v2transactions: vec![tx],
    };
    let _response = block_on(api_client.dispatcher(req)).unwrap();
}
