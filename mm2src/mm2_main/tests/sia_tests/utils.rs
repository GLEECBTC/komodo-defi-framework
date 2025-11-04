pub use coins::siacoin::sia_rust::types::{Address, Currency, Keypair};
pub use coins::siacoin::sia_rust::utils::V2TransactionBuilder;

use coins::siacoin::{ApiClientHelpers, SiaApiClient, SiaClient, SiaClientConf};
use keys::hash::H256;

use crate::docker_tests::docker_tests_common::SIA_RPC_PARAMS;
use common::custom_futures::timeout::FutureTimerExt;
use common::executor::Timer;
use mm2_rpc::data::legacy::CoinInitResponse;
use mm2_test_helpers::for_tests::{MarketMakerIt, Mm2TestConf};

use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::collections::HashMap;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use url::Url; // for read_line()

mod komodod_client;
pub use komodod_client::*;

pub const WALLETD_CONFIG: &str = r#"
http:
  address: :9980
  password: password
  publicEndpoints: false
index:
  mode: full
log:
  stdout:
    enabled: true
    level: debug
    format: human
"#;

// FIXME Alright - Nate provided a simplified version of this... use that after testing this works at all
pub const WALLETD_NETWORK_CONFIG: &str = r#"{
    "network": {
        "name": "komodo-ci",
        "initialCoinbase": "300000000000000000000000000000",
        "minimumCoinbase": "30000000000000000000000000000",
        "initialTarget": "0100000000000000000000000000000000000000000000000000000000000000",
        "blockInterval": 60000000000,
        "maturityDelay": 10,
        "hardforkDevAddr": {
            "height": 1,
            "oldAddress": "000000000000000000000000000000000000000000000000000000000000000089eb0d6a8a69",
            "newAddress": "000000000000000000000000000000000000000000000000000000000000000089eb0d6a8a69"
        },
        "hardforkTax": {
            "height": 2
        },
        "hardforkStorageProof": {
            "height": 5
        },
        "hardforkOak": {
            "height": 10,
            "fixHeight": 12,
            "genesisTimestamp": "2023-01-13T00:53:20-08:00"
        },
        "hardforkASIC": {
            "height": 20,
            "oakTime": 600000000000,
            "oakTarget": "0100000000000000000000000000000000000000000000000000000000000000",
            "nonceFactor": 1009
        },
        "hardforkFoundation": {
            "height": 30,
            "primaryAddress": "053b2def3cbdd078c19d62ce2b4f0b1a3c5e0ffbeeff01280efb1f8969b2f5bb4fdc680f0807",
            "failsafeAddress": "000000000000000000000000000000000000000000000000000000000000000089eb0d6a8a69"
        },
        "hardforkV2": {
            "allowHeight": 0,
            "requireHeight": 7777777,
            "finalCutHeight": 8888888
        }
    },
    "genesis": {
        "parentID": "0000000000000000000000000000000000000000000000000000000000000000",
        "nonce": 0,
        "timestamp": "2023-01-13T00:53:20-08:00",
        "minerPayouts": null,
        "transactions": [
            {
                "id": "268ef8627241b3eb505cea69b21379c4b91c21dfc4b3f3f58c66316249058cfd",
                "siacoinOutputs": [
                    {
                        "value": "1000000000000000000000000000000000000",
                        "address": "a0cfbc1089d129f52d00bc0b0fac190d4d87976a1d7f34da7ca0c295c99a628de344d19ad469"
                    }
                ],
                "siafundOutputs": [
                    {
                        "value": 10000,
                        "address": "053b2def3cbdd078c19d62ce2b4f0b1a3c5e0ffbeeff01280efb1f8969b2f5bb4fdc680f0807"
                    }
                ]
            }
        ]
    }
}"#;

pub const ALICE_SIA_ADDRESS_STR: &str = "a0cfbc1089d129f52d00bc0b0fac190d4d87976a1d7f34da7ca0c295c99a628de344d19ad469";
pub const ALICE_KMD_KEY: TestKeyPair = TestKeyPair {
    address: "RNa3bJJC2L3UUCGQ9WY5fhCSzSd5ExiAWr",
    pubkey: "033ca097f047603318d7191ecb8e75b96a15b6bfac97853c4f25619177c5992427",
    wif: "UqubgosgQT3cjt488P2qLoqP3oMGgNccXHTGeVQBSUFsMwCA459Q",
};

pub const BOB_SIA_ADDRESS_STR: &str = "c34caa97740668de2bbdb7174572ed64c861342bf27e80313cbfa02e9251f52e30aad3892533";
pub const BOB_KMD_KEY: TestKeyPair = TestKeyPair {
    address: "RLHqXM7q689D1PZvt9nH5nmouSPMG9sopG",
    pubkey: "02f5e06a51ac7723d8d07792b6b2f36e7953264ce0756006c3859baaad4c016266",
    wif: "UvU3bn2bucriZVDaSSB51aGGu9emUbmf9ZK72sdRjrD2Vb4smQ8T",
};

lazy_static! {
    pub static ref COINS: Json = json!(
        [
            // Dockerized Sia coin
            {
                "coin": "DSIA",
                "mm2": 1,
                "required_confirmations": 1,
                "protocol": {
                "type": "SIA"
                }
            },
            // Dockerized UTXO coin
            {
                "coin":"MYCOIN",
                "asset":"MYCOIN",
                "mm2": 1,
                "txversion":4,
                "overwintered":1,
                "protocol":{
                    "type":"UTXO"
                }
            }
        ]
    );

    /// Sia Address from the iguana seed "buyer buyer buyer buyer buyer buyer buyer buyer buyer buyer buyer cabin"
    pub static ref ALICE_SIA_ADDRESS: Address = Address::from_str(ALICE_SIA_ADDRESS_STR).unwrap();

    /// Sia Address from the iguana seed "sell sell sell sell sell sell sell sell sell sell sell sell"
    pub static ref BOB_SIA_ADDRESS: Address = Address::from_str(BOB_SIA_ADDRESS_STR).unwrap();

    /// A Sia Address that is not Alice's or Bob's. Global walletd container will mine to this address.
    /// iguana seed "neutral neutral neutral neutral neutral neutral neutral neutral neutral neutral neutral noise"
    pub static ref CHARLIE_SIA_KEYPAIR: Keypair = Keypair::from_private_bytes(&[
        0x38, 0x9d, 0xd4, 0xd0, 0x09, 0xe6, 0xb1, 0x1d,
        0xb0, 0xf1, 0x55, 0x16, 0xbc, 0x29, 0x0e, 0x7b,
        0xa0, 0xcc, 0x58, 0x09, 0x30, 0xac, 0xe2, 0x00,
        0x5d, 0x39, 0xd0, 0xe4, 0x97, 0xb4, 0xa6, 0x67
    ]).unwrap();

    /// Sia Address of CHARLIE_SIA_KEYPAIR
    pub static ref CHARLIE_SIA_ADDRESS: Address = CHARLIE_SIA_KEYPAIR.public().address();
}

/// A container running a Sia walletd instance.
/// The container will run until the `Container` falls out of scope. It will then be stopped and removed.
/// It is sometimes useful while debugging to leave a container running after a test executes.
/// This can be done by leaking the `Container` or the `SiaTestnetContainer` itself.
/// eg,
/// let _leaked = Box::leak(Box::new(container));
pub struct SiaTestnetContainer {
    /// SiaClient to interact with the walletd API within the container
    pub client: SiaClient,
    /// Port on the host that walletd API is bound to
    pub host_port: u16,
}

/// Send coins from Charlie to the given address.
/// Assumes Charlie has enough coins to send.
pub async fn fund_address(address: &Address, amount: Currency) {
    lazy_static! {
        static ref SIA_FUNDING_LOCK: Mutex<()> = Mutex::new(());
    }
    // Lock the funding operation so to not let multiple tests fund address from the same utxos and double spend them.
    let _lock = SIA_FUNDING_LOCK.lock().await;

    let client = init_sia_client().await.unwrap();

    let tx = V2TransactionBuilder::new()
        .miner_fee(Currency::DEFAULT_FEE)
        .add_siacoin_output((address.clone(), amount).into())
        .fund_tx_single_source(&client, &CHARLIE_SIA_KEYPAIR.public())
        .await
        .expect("fund_address helper failed at fund_tx_single_source")
        .add_change_output(&CHARLIE_SIA_KEYPAIR.public().address())
        .sign_simple(vec![&CHARLIE_SIA_KEYPAIR])
        .build();

    // Broadcast the transaction
    client.broadcast_transaction(&tx).await.unwrap();
    // Mine some blocks to confirm the transaction
    client.mine_blocks(10, &CHARLIE_SIA_ADDRESS).await.unwrap();
}

pub async fn fund_privkey_sia(priv_key: &H256, amount: Currency) {
    let keypair = Keypair::from_private_bytes(priv_key.as_slice()).unwrap();
    let address = Address::from_public_key(&keypair.public());
    fund_address(&address, amount).await;
}

/// Get the global walletd container
pub async fn get_global_walletd_container() -> Arc<SiaTestnetContainer> {
    let client = init_sia_client().await.unwrap();
    Arc::new(SiaTestnetContainer {
        host_port: client.base_url.port().unwrap(),
        client,
    })
}

pub struct TestKeyPair<'a> {
    pub address: &'a str,
    #[allow(dead_code)]
    pub pubkey: &'a str,
    #[allow(dead_code)]
    pub wif: &'a str,
}

/// Response from `get_directly_connected_peers` RPC endpoint.
/// eg, {"<PeerId>": ["<Multiaddr>", "<Multiaddr>"], "<PeerId>": ["<Multiaddr>"]}}
/// TODO: Should technically be HashMap<Peerid, Vec<Multiaddr>> but not needed for current use cases.
#[derive(Debug, Serialize, Deserialize)]
#[serde(transparent, rename = "result")]
pub struct GetDirectlyConnectedPeersResponse(pub HashMap<String, Vec<String>>);

pub async fn enable_dsia(mm: &MarketMakerIt) -> CoinInitResponse {
    let (ip, port, password) = SIA_RPC_PARAMS;
    let url = format!("http://{ip}:{port}/");
    mm.rpc_typed::<CoinInitResponse>(&json!({
        "method": "enable",
        "coin": "DSIA",
        "tx_history": true,
        "client_conf": {
            "server_url": url,
            "password": password,
        }
    }))
    .await
    .unwrap()
}

pub async fn enable_mycoin(mm: &MarketMakerIt) -> CoinInitResponse {
    mm.rpc_typed::<CoinInitResponse>(&json!({
        "method": "enable",
        "coin": "MYCOIN",
        "tx_history": true
    }))
    .await
    .unwrap()
}

/**
Initialize a MarketMaker instance with a configuration suitable for the taker aka Alice.

Intended to be used in conjunction with `init_bob` to create a taker/maker setup.

This node will not act as a seed node and will not listen on the p2p port.
**/
pub async fn init_alice(priv_key: &H256, seednode_ip: &IpAddr, custom_locktime: Option<u64>) -> MarketMakerIt {
    let seed = format!("0x{}", hex::encode(priv_key));
    let mut conf = Mm2TestConf::light_node(&seed, &COINS, &[&seednode_ip.to_string()]);
    if let Some(lt) = custom_locktime {
        conf.conf["payment_locktime"] = lt.into();
    }
    let mm = MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)
        .await
        .unwrap();

    //let (_dump_log, _dump_dashboard) = mm.mm_dump();
    log!("alice's log path: {}", mm.log_path.display());

    mm
}

/**
Initialize a MarketMaker instance with a configuration suitable for the maker aka Bob.

Intended to be used in conjunction with `init_alice` to create a taker/maker setup.

This node will act as a seed node and will listen on the p2p port.
**/
pub async fn init_bob(priv_key: &H256, custom_locktime: Option<u64>) -> MarketMakerIt {
    let seed = format!("0x{}", hex::encode(priv_key));
    let mut conf = Mm2TestConf::seednode(&seed, &COINS);
    if let Some(lt) = custom_locktime {
        conf.conf["payment_locktime"] = lt.into();
    }
    let mm = MarketMakerIt::start_async(conf.conf, conf.rpc_password, None)
        .await
        .unwrap();

    //let (_dump_log, _dump_dashboard) = mm.mm_dump();
    log!("bob's log path: {}", mm.log_path.display());

    mm
}

/// Initialize a Sia standalone SiaClient.
/// This is useful to interact with a Sia testnet container for commands that are not from Alice or
/// Bob. Eg, mining blocks to progress the chain.
pub async fn init_sia_client() -> Result<SiaClient, String> {
    let (ip, port, password) = SIA_RPC_PARAMS;
    let conf = SiaClientConf {
        server_url: Url::parse(&format!("http://{}:{}/", ip, port)).unwrap(),
        password: Some(password.to_string()),
        timeout: Some(10),
    };
    SiaClient::new(conf).await.map_err(|e| e.to_string())
}

/// Wait for the global Dsia node to be ready by polling the current_height endpoint.
/// Panics if the node is not ready after several attempts.
/// Spawns a mining thread that will mine blocks every 10 seconds to advance the chain.
pub async fn wait_for_dsia_node_ready() {
    let mut attempts = 0;
    loop {
        if attempts >= 5 {
            panic!("Failed to connect to Dsia node after several attempts.");
        }

        match init_sia_client().await {
            Ok(client) => match client.current_height().timeout(Duration::from_secs(6)).await {
                Ok(Ok(block_number)) => {
                    log!("Dsia node is ready, latest block number: {:?}", block_number);
                    break;
                },
                Ok(Err(e)) => {
                    log!("Failed to connect to Dsia node: {:?}, retrying...", e);
                },
                Err(_) => {
                    log!("Connection to Dsia node timed out, retrying...");
                },
            },
            Err(e) => {
                log!("Failed to create Sia client: {:?}, retrying...", e);
            },
        };

        attempts += 1;
        Timer::sleep(1.).await;
    }

    let client = init_sia_client().await.unwrap();
    // Mine 155 blocks to begin because coinbase maturity is 150
    client.mine_blocks(155, &CHARLIE_SIA_ADDRESS).await.unwrap();

    // Spawn a loop that will keep mining blocks every 10 seconds to advance the chain
    // and get the swap tests running.
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(10)).await;
            client.mine_blocks(1, &CHARLIE_SIA_ADDRESS).await.unwrap();
            common::log::debug!("Mined 1 block on global DSIA container");
        }
    });
}

/// Connects to the the already initilized komodod container (running MYCOIN) and funds `funded_key` with some coins.
/// Also imports both `funded_key` and `unfunded_key` addresses into the node.
pub async fn get_komodod_client(funded_key: TestKeyPair<'_>, unfunded_key: TestKeyPair<'_>) -> KomododClient {
    let conf = KomododClientConf {
        // This is where MYCOIN node runs.
        // TODO: make a global constant for this.
        ip: IpAddr::from([127, 0, 0, 1]),
        port: 7000,
        rpcuser: "test".to_string(),
        rpcpassword: "test".to_string(),
        timeout: None,
    };
    let client = KomododClient::new(conf).await;

    // Send 1,000,000 coins to funded_key.address
    let _ = client.rpc("sendtoaddress", json!([funded_key.address, 1000000])).await;

    // Import both addresses to our node.
    let _ = client.rpc("importaddress", json!([funded_key.address])).await;
    let _ = client.rpc("importaddress", json!([unfunded_key.address])).await;

    client
}

// Wait until Alice connects to Bob as a peer or timeout
pub async fn wait_for_peers_connected(
    alice: &MarketMakerIt,
    bob: &MarketMakerIt,
    timeout_duration: Duration,
) -> Result<(), ()> {
    let start_time = tokio::time::Instant::now();

    // fetch Bob's PeerId
    let bob_peer_id = bob
        .rpc_typed::<String>(&json!({"method": "get_my_peer_id"}))
        .await
        .unwrap();

    loop {
        // fetch Alice's connected peers
        let alice_peers = alice
            .rpc_typed::<GetDirectlyConnectedPeersResponse>(&json!({"method": "get_directly_connected_peers"}))
            .await
            .unwrap();

        // Check if Bob's PeerId is in Alice's connected peers
        if alice_peers.0.contains_key(&bob_peer_id) {
            return Ok(());
        }

        // Check if we've reached the timeout
        if start_time.elapsed() >= timeout_duration {
            return Err(()); // Timed out
        }

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}
