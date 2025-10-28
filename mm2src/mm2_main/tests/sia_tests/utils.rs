pub use coins::siacoin::sia_rust::types::{Address, Currency, Keypair};
pub use coins::siacoin::sia_rust::utils::V2TransactionBuilder;
use mm2_main::lp_native_dex::lp_init;
use mm2_main::lp_network::MAX_NETID;

use coins::siacoin::{ApiClientHelpers, SiaApiClient, SiaClient, SiaClientConf};
use coins::utxo::zcash_params_path;

use crate::docker_tests::docker_tests_common::SIA_RPC_PARAMS;
use common::custom_futures::timeout::FutureTimerExt;
use common::executor::Timer;
use common::log::{LogLevel, UnifiedLoggerBuilder};
use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};
use mm2_rpc::data::legacy::CoinInitResponse;
use mm2_test_helpers::for_tests::MarketMakerIt;

use chrono::Local;
use core::pin::Pin;
use lazy_static::lazy_static;
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::collections::HashMap;
use std::io::Write;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;
use std::time::Duration;
use testcontainers::core::{ContainerAsync, Mount, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{GenericImage, RunnableImage};
use tokio::io::{AsyncBufRead, AsyncBufReadExt};
use tokio::sync::OnceCell;
use url::Url; // for read_line()

mod komodod_client;
pub use komodod_client::*;

const WALLETD_CONFIG: &str = r#"
http:
  address: :9980
  password: password
  publicEndpoints: true
index:
  mode: full
log:
  stdout:
    enabled: true
    level: debug
    format: human
"#;

// FIXME Alright - Nate provided a simplified version of this... use that after testing this works at all
const WALLETD_NETWORK_CONFIG: &str = r#"{
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
            "allowHeight": 40,
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

/// Filename for the log file for each test utilizing `init_test_dir()`
/// Each MarketMaker instance will log to <temp directory>/kdf.log generally.
const LOG_FILENAME: &str = "kdf.log";

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

pub const CHARLIE_KMD_KEY: TestKeyPair = TestKeyPair {
    address: "RHidEv1tYs7GxB2o6hYJcuruBcsPVSvutp",
    pubkey: "0363bee6428ce79a60ff905573e8358b3ba827aac455f3377b495a020035ce9d30",
    wif: "UtZxep1DqSk1UhizSmNktbZeoMqR3xkafRLXmgdwSKD7cVXE7TWP",
};

/// A single global walletd container that is shared between any test that uses init_global_walletd_container()
pub static DSIA_GLOBAL_CONTAINER: OnceCell<Arc<SiaTestnetContainer>> = OnceCell::const_new();

/// Used to ensure the mining thread is only started once globally
pub static DSIA_MINING_THREAD_INIT: OnceCell<()> = OnceCell::const_new();

/// A new temporary directory created by init_test_dir() each time a test or group of tests is ran.
/// eg, /tmp/kdf_tests_2025-02-18_11-36-21-802/ which might include subdirectories for each test.
pub static SHARED_TEMP_DIR: OnceCell<PathBuf> = OnceCell::const_new();

/// Atomic counter used to generate a unique netid per test.
static NEXT_NETID: AtomicU16 = AtomicU16::new(1);

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
            // init_alice and init_bob both rely on this being COINS[1] while setting 'confpath'
            {
                "coin": "DUTXO",
                "asset": "DUTXO",
                "fname": "DUTXO",
                "rpcport": 10001,
                "txversion": 4,
                "overwintered": 1,
                "mm2": 1,
                "sign_message_prefix": "Komodo Signed Message:\n",
                "is_testnet": true,
                "required_confirmations": 1,
                "requires_notarization": false,
                "avg_blocktime": 60,
                "protocol": {
                "type": "UTXO"
                },
                "derivation_path": "m/44'/141'",
                "trezor_coin": "Komodo"
            },
            {
                "coin": "DOC",
                "asset": "DOC",
                "fname": "DOC",
                "rpcport": 62415,
                "txversion": 4,
                "overwintered": 1,
                "mm2": 1,
                "sign_message_prefix": "Komodo Signed Message:\n",
                "is_testnet": true,
                "required_confirmations": 1,
                "requires_notarization": false,
                "avg_blocktime": 60,
                "protocol": {
                "type": "UTXO"
                },
                "derivation_path": "m/44'/141'",
                "trezor_coin": "Komodo"
            },
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

/// Used inconjunction with init_test_dir() to create a unique directory for each test
/// Not intended to be used otherwise due to hardcoded suffix value.
#[macro_export]
macro_rules! current_function_name {
    () => {{
        fn f() {}
        fn type_name_of<T>(_: T) -> &'static str {
            std::any::type_name::<T>()
        }
        let name = type_name_of(f);
        name.strip_suffix("::{{closure}}::f")
            .unwrap()
            .rsplit("::")
            .next()
            .unwrap()
    }};
}

pub(crate) use current_function_name;

/// Takes the output of container.stdout() or container.stderr() and pipes it to the host's stdout.
/// Should be spawned as a tokio task.
pub async fn pipe_buf_to_stdout(mut reader: Pin<Box<dyn AsyncBufRead + Send>>) {
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => break, // EOF
            Ok(_) => {
                print!("{}", line);
                let _ = std::io::stdout().flush();
            },
            Err(e) => {
                eprintln!("Error reading from stdout: {}", e);
                break;
            },
        }
    }
}

/// A container running a Sia walletd instance.
/// The container will run until the `Container` falls out of scope. It will then be stopped and removed.
/// It is sometimes useful while debugging to leave a container running after a test executes.
/// This can be done by leaking the `Container` or the `SiaTestnetContainer` itself.
/// eg,
/// let _leaked = Box::leak(Box::new(container));
pub struct SiaTestnetContainer {
    /// Docker container running walletd.
    // Todo: check why this field was added
    #[allow(dead_code)]
    pub container: ContainerAsync<GenericImage>,
    /// SiaClient to interact with the walletd API within the container
    pub client: SiaClient,
    /// Port on the host that walletd API is bound to
    pub host_port: u16,
}

/// Get a unique netid for each test.
pub fn get_unique_netid() -> u16 {
    let netid = NEXT_NETID.fetch_add(1, Ordering::Relaxed);
    if netid > MAX_NETID {
        panic!("get_unique_netid: Exceeded maximum netid value")
    }
    netid
}

/// Send coins from Charlie to the given address.
/// Assumes Charlie has enough coins to send.
pub async fn fund_address(client: &SiaClient, address: &Address, amount: Currency) {
    let tx = V2TransactionBuilder::new()
        .miner_fee(Currency::DEFAULT_FEE)
        .add_siacoin_output((address.clone(), amount).into())
        .fund_tx_single_source(client, &CHARLIE_SIA_KEYPAIR.public())
        .await
        .expect("fund_address helper failed at fund_tx_single_source")
        .add_change_output(&CHARLIE_SIA_KEYPAIR.public().address())
        .sign_simple(vec![&CHARLIE_SIA_KEYPAIR])
        .build();

    // Broadcast the transaction
    client.broadcast_transaction(&tx).await.unwrap();
}

/// Initialize the global walletd container and begin mining blocks every 10 seconds.
pub async fn init_global_walletd_container() -> Arc<SiaTestnetContainer> {
    let temp_dir = init_test_dir(current_function_name!(), true).await;

    let container = DSIA_GLOBAL_CONTAINER
        .get_or_init(|| async { Arc::new(init_walletd_container(&temp_dir).await) })
        .await
        .clone();

    // Start a task to mine a block every 10 seconds
    DSIA_MINING_THREAD_INIT
        .get_or_init(|| async {
            let client = container.client.clone();
            common::log::debug!("Starting global DSIA mining thread");
            tokio::spawn(async move {
                // Mine 155 blocks to begin because coinbase maturity is 150
                client.mine_blocks(155, &CHARLIE_SIA_ADDRESS).await.unwrap();
                loop {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    client.mine_blocks(1, &CHARLIE_SIA_ADDRESS).await.unwrap();
                    common::log::debug!("Mined 1 block on global DSIA container");
                }
            });
        })
        .await;

    container
}

pub struct TestKeyPair<'a> {
    pub address: &'a str,
    pub pubkey: &'a str,
    pub wif: &'a str,
}

/// Response from `get_directly_connected_peers` RPC endpoint.
/// eg, {"<PeerId>": ["<Multiaddr>", "<Multiaddr>"], "<PeerId>": ["<Multiaddr>"]}}
/// TODO: Should technically be HashMap<Peerid, Vec<Multiaddr>> but not needed for current use cases.
#[derive(Debug, Serialize, Deserialize)]
#[serde(transparent, rename = "result")]
pub struct GetDirectlyConnectedPeersResponse(pub HashMap<String, Vec<String>>);

pub async fn enable_dsia(mm: &MarketMakerIt, walletd_port: u16) -> CoinInitResponse {
    let url = format!("http://127.0.0.1:{}/", walletd_port);
    mm.rpc_typed::<CoinInitResponse>(&json!({
        "method": "enable",
        "coin": "DSIA",
        "tx_history": true,
        "client_conf": {
            "server_url": url,
            "password": "password"
        }
    }))
    .await
    .unwrap()
}

pub async fn enable_dutxo(mm: &MarketMakerIt) -> CoinInitResponse {
    mm.rpc_typed::<CoinInitResponse>(&json!({
        "method": "enable",
        "coin": "DUTXO",
        "tx_history": true
    }))
    .await
    .unwrap()
}

/// Create a temporary directory to be shared amongst all tests ran at the same time.
/// Utilizes `std::env::temp_dir()` so each OS will handle this differently.
/// We assume the OS will eventually prune these direcotories.
/// Note: Windows machines may never prune these directories so be cautious.
/// env var $TMPDIR can be set to change the location of the temp directory on most unix-like OSes.
/// This is async only to avoid an additional import of a non-async OnceCell implementation.
pub async fn init_test_dir(fn_path: &str, silent_console: bool) -> PathBuf {
    // initialize a shared temp directory and global logger if they haven't been already
    let shared_dir = SHARED_TEMP_DIR
        .get_or_init(|| async {
            let init_time = Local::now().format("%Y-%m-%d_%H-%M-%S-%3f").to_string();

            // Initialize env_logger that is shared amongst all KDF instances
            UnifiedLoggerBuilder::new().silent_console(silent_console).init();

            // eg, /tmp/kdf_tests_2025-02-18_11-36-21-802/
            let tests_group = format!("kdf_tests_{}", init_time);

            std::env::temp_dir().join(tests_group)
        })
        .await;

    // eg, /tmp/kdf_tests_2025-02-18_11-36-21-802/test_something/
    let test_dir = shared_dir.join(fn_path);
    common::log::debug!("Using temporary directory: {}", test_dir.display());

    std::fs::create_dir_all(&test_dir).unwrap();
    test_dir
}

/**
Initialize a MarketMaker instance with a configuration suitable for the taker aka Alice.

Intended to be used in conjunction with `init_bob` to create a taker/maker setup.

This node will not act as a seed node and will not listen on the p2p port.

This node will attempt to connect to a seed node on the host that is using the same
`netid` value. ie, `localhost:<p2p_port>` where <p2p_port> is influenced by the `netid` value.

`rpc_port` - The port the MarketMaker instance will listen on for RPC commands.
`netid` - The network id for the MarketMaker instance. This directly influences the p2p port
          used to comminucate with other MarketMaker instances. This is not the literal port number
          but rather the input to the function `mm2_main::lp_network::lp_ports`.
`utxo_rpc_port` - If set, enables the marketmaker instance to connect to a native UTXO node at the given
    port. This is only needed if multiple *native UTXO nodes for the same coin* are being used.
    Eg, Alice's personal DUTXO node http://test:test@127.0.0.1:10001/
        and Bob's http://test:test@127.0.0.1:10000/

Use unique values for `rpc_port`` and `netid`` for each test if they are intended to run concurrently
alongside other unrelated tests.

All configurations other than rpc_port and netid are hardcoded for simplicity.
**/
pub async fn init_alice(kdf_dir: &Path, netid: u16, utxo_rpc_port: Option<u16>) -> (MmArc, MarketMakerIt) {
    let alice_db_dir = kdf_dir.join("DB_alice");
    let test_case_string = kdf_dir.to_str().unwrap().to_string();
    let datetime = "init_alice".to_string();
    let ip = IpAddr::from([127, 0, 0, 1]);

    // `enable` method using native UTXO node is too stupid to allow setting the rpc credentials anywhere
    // other than a config file specified in the coins json. So using different UTXO nodes for Alice
    // and Bob means we need a unique coins json for each. If `utxo_rpc_port` is set, we create the
    // equivalent of `~/.komodo/DUTXO/DUTXO.conf` that would typically be created by Komodod and set
    // DUTXO['confpath'] to that file.
    let alice_coins = match utxo_rpc_port {
        Some(utxo_port) => {
            let mut coins = COINS.clone();
            let file_contents = format!("rpcuser=test\nrpcpassword=test\nrpcport={}\n", utxo_port);
            let utxo_conf_file_path = kdf_dir.join("ALICE_DUTXO.conf");
            let mut conf_file = std::fs::File::create(&utxo_conf_file_path).unwrap();
            conf_file.write_all(file_contents.as_bytes()).unwrap();
            coins[1]["confpath"] = json!(utxo_conf_file_path);
            coins
        },
        None => COINS.clone(),
    };

    let alice_conf = json!({
        "gui": format!("{}_alice", test_case_string),
        "netid": netid,
        "passphrase": "buyer buyer buyer buyer buyer buyer buyer buyer buyer buyer buyer cabin",
        "coins": alice_coins,
        "myipaddr": ip.to_string(),
        "rpc_password": "password",
        "rpcport": 0, // 0 value will assign an available port that can be read from ctx.rpc_started
        "i_am_seed": false,
        "enable_hd": false,
        "dbdir": alice_db_dir.to_str().unwrap(),
        "seednodes": [
            "127.0.0.1"
        ]
    });

    let ctx = MmCtxBuilder::new()
        .with_conf(alice_conf)
        .with_log_level(LogLevel::Debug)
        .with_version(test_case_string.clone())
        .with_datetime(datetime.clone())
        .into_mm_arc();
    let ctx_clone = ctx.clone();
    tokio::spawn(async move { lp_init(ctx, test_case_string, datetime).await.unwrap() });

    wait_for_rpc_started(ctx_clone.clone(), Duration::from_secs(20))
        .await
        .unwrap();
    let rpc_port = *ctx_clone.rpc_port.get().unwrap();

    let mm_alice = MarketMakerIt {
        folder: alice_db_dir,
        ip,
        rpc_port: Some(rpc_port),
        log_path: kdf_dir.join(LOG_FILENAME),
        pc: None,
        userpass: "password".to_string(),
    };

    (ctx_clone, mm_alice)
}

/**
Initialize a MarketMaker instance with a configuration suitable for the maker aka Bob.

Intended to be used in conjunction with `init_alice` to create a taker/maker setup.

This node will act as a seed node and will listen on the p2p port.

`rpc_port` - The port the MarketMaker instance will listen on for RPC commands.
`netid` - The network id for the MarketMaker instance. This directly influences the p2p port
          used to comminucate with other MarketMaker instances. This is not the literal port number
          but rather the input to the function `mm2_main::lp_network::lp_ports`.
`utxo_rpc_port` - If set, enables the marketmaker instance to connect to a native UTXO node at the given
    port. This is only needed if multiple *native UTXO nodes for the same coin* are being used.
    Eg, Alice's personal DUTXO node http://test:test@127.0.0.1:10001/
        and Bob's http://test:test@127.0.0.1:10000/

Use unique values for `rpc_port`` and `netid`` for each test if they are intended to run concurrently
alongside other unrelated tests.

All configurations other than rpc_port and netid are hardcoded for simplicity.
**/
pub async fn init_bob(kdf_dir: &Path, netid: u16, utxo_rpc_port: Option<u16>) -> (MmArc, MarketMakerIt) {
    let bob_db_dir = kdf_dir.join("DB_bob");
    let test_case_string = kdf_dir.to_str().unwrap().to_string();
    let datetime = "init_bob".to_string();
    let ip = IpAddr::from([127, 0, 0, 1]);

    // `enable` method using native UTXO node is too stupid to allow setting the rpc credentials anywhere
    // other than a config file specified in the coins json. So using different UTXO nodes for Alice
    // and Bob means we need a unique coins json for each. If `utxo_rpc_port` is set, we create the
    // equivalent of `~/.komodo/DUTXO/DUTXO.conf` that would typically be created by Komodod and set
    // DUTXO['confpath'] to that file.
    let coins = match utxo_rpc_port {
        Some(utxo_port) => {
            let mut coins = COINS.clone();
            let file_contents = format!("rpcuser=test\nrpcpassword=test\nrpcport={}\n", utxo_port);
            let utxo_conf_file_path = kdf_dir.join("BOB_DUTXO.conf");
            let mut conf_file = std::fs::File::create(&utxo_conf_file_path).unwrap();
            conf_file.write_all(file_contents.as_bytes()).unwrap();
            coins[1]["confpath"] = json!(utxo_conf_file_path);
            coins
        },
        None => COINS.clone(),
    };

    let bob_conf = json!({
        "gui": format!("{}_bob", test_case_string),
        "netid": netid,
        "passphrase": "sell sell sell sell sell sell sell sell sell sell sell sell",
        "coins": coins,
        "myipaddr": ip.to_string(),
        "rpc_password": "password",
        "rpcport": 0, // 0 value will assign an available port that can be read from ctx.rpc_started
        "i_am_seed": true,
        "is_bootstrap_node": true,
        "enable_hd": false,
        "dbdir": bob_db_dir.to_str().unwrap(),
    });

    let ctx = MmCtxBuilder::new()
        .with_conf(bob_conf)
        .with_log_level(LogLevel::Debug)
        .with_version(test_case_string.clone())
        .with_datetime(datetime.clone())
        .into_mm_arc();
    let ctx_clone = ctx.clone();
    tokio::spawn(async move { lp_init(ctx, test_case_string, datetime).await.unwrap() });

    wait_for_rpc_started(ctx_clone.clone(), Duration::from_secs(20))
        .await
        .unwrap();

    let rpc_port = *ctx_clone.rpc_port.get().unwrap();

    let mm_bob = MarketMakerIt {
        folder: bob_db_dir,
        ip,
        rpc_port: Some(rpc_port),
        log_path: kdf_dir.join(LOG_FILENAME),
        pc: None,
        userpass: "password".to_string(),
    };

    (ctx_clone, mm_bob)
}

/// Initialize a Sia standalone SiaClient.
/// This is useful to interact with a Sia testnet container for commands that are not from Alice or
/// Bob. Eg, mining blocks to progress the chain.
pub async fn init_sia_client(ip: &str, port: u16, password: &str) -> Result<SiaClient, String> {
    let conf = SiaClientConf {
        server_url: Url::parse(&format!("http://{}:{}/", ip, port)).unwrap(),
        password: Some(password.to_string()),
        timeout: Some(10),
    };
    SiaClient::new(conf).await.map_err(|e| e.to_string())
}

#[cfg(feature = "enable-sia")]
pub async fn wait_for_dsia_node_ready() {
    let (ip, port, password) = SIA_RPC_PARAMS;
    let mut attempts = 0;
    loop {
        if attempts >= 5 {
            panic!("Failed to connect to Dsia node after several attempts.");
        }

        match init_sia_client(ip, port, password).await {
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
}

/// Initialize a walletd docker container with walletd API bound to a random port on the host.
/// Returns the container and the host port it is bound to.
/// The container will run until it falls out of scope.
pub async fn init_walletd_container(temp_dir: &Path) -> SiaTestnetContainer {
    // Create a directory within the shared temp directory to mount as the /config within the container
    // eg, /tmp/kdf_tests_2025-02-18_11-36-21-802/walletd_config
    let config_dir = temp_dir.join("walletd_config");
    std::fs::create_dir_all(&config_dir).unwrap();

    // Write walletd.yml
    std::fs::write(config_dir.join("walletd.yml"), WALLETD_CONFIG).expect("failed to write walletd.yml");

    // Write ci_network.json
    std::fs::write(config_dir.join("ci_network.json"), WALLETD_NETWORK_CONFIG)
        .expect("failed to write ci_network.json");

    // Define the Docker image with a tag
    let image = GenericImage::new("ghcr.io/siafoundation/walletd", "master")
        .with_exposed_port(9980)
        .with_env_var("WALLETD_CONFIG_FILE", "/config/walletd.yml")
        .with_wait_for(WaitFor::message_on_stdout("node started"))
        .with_mount(Mount::bind_mount(
            config_dir.to_str().expect("config path is invalid"),
            "/config",
        ));
    let walletd_args = vec!["-network=/config/ci_network.json".to_string(), "-debug".to_string()];

    // Wrap the image in `RunnableImage` to allow custom port mapping to an available host port
    // 0 indicates that the host port will be automatically assigned to an available port
    let runnable_image = RunnableImage::from((image, walletd_args)).with_mapped_port((0, 9980));

    // Start the container. It will run until `Container` falls out of scope
    let container = runnable_image.start().await.unwrap();

    // Retrieve the host port that is mapped to the container's 9980 port
    let host_port = container.get_host_port_ipv4(9980).await.unwrap();

    // Initialize a SiaClient to interact with the walletd API
    while let Err(error) = (1..=5)
        .map(|_| init_sia_client("127.0.0.1", host_port, "password"))
        .next()
        .expect("couldn't connect to sia client within 5 attempts")
        .await
    {
        log!("Waiting for walletd SiaClient to be ready: {error}");
        Timer::sleep(1.).await;
    }
    let client = init_sia_client("127.0.0.1", host_port, "password").await.unwrap();
    SiaTestnetContainer {
        container,
        client,
        host_port,
    }
}

// Initialize a container with 2 komodod nodes.
// Binds "main" node(has address imported and mines blocks) to `port`
// Binds additional node to `port` - 1
// Auth for both nodes is "test:test"
pub async fn init_komodod_container() -> (ContainerAsync<GenericImage>, u16, u16) {
    // the ports komodod will listen on the container's network interface
    let mining_node_port = 10000;
    let nonmining_node_port = mining_node_port - 1;
    let image = GenericImage::new("docker.io/artempikulin/testblockchain", "multiarch")
        .with_env_var("CLIENTS", "2")
        .with_env_var("CHAIN", "ANYTHING")
        .with_env_var("TEST_ADDY", CHARLIE_KMD_KEY.address)
        .with_env_var("TEST_WIF", CHARLIE_KMD_KEY.wif)
        .with_env_var("TEST_PUBKEY", CHARLIE_KMD_KEY.pubkey)
        .with_env_var("DAEMON_URL", "http://test:test@127.0.0.1:7000")
        .with_env_var("COIN", "Komodo")
        .with_env_var("COIN_RPC_PORT", nonmining_node_port.to_string())
        //.with_wait_for(WaitFor::message_on_stdout("'name': 'ANYTHING'"))
        .with_wait_for(WaitFor::seconds(20))
        .with_exposed_port(mining_node_port)
        .with_exposed_port(nonmining_node_port)
        .with_mount(Mount::bind_mount(
            zcash_params_path().display().to_string(),
            "/root/.zcash-params",
        ));
    let container = image.start().await.unwrap();
    let mining_host_port = container.get_host_port_ipv4(mining_node_port).await.unwrap();
    let nonmining_host_port = container.get_host_port_ipv4(nonmining_node_port).await.unwrap();
    (container, mining_host_port, nonmining_host_port)
}

// this is imprecise because it relies on polling the block height every 2 seconds
// A docker container allowing the use of the `generate` RPC command was required from Decker as a
// solution.
#[allow(dead_code)]
pub async fn mine_n_blocks(client: &KomododClient, target_blocks: u64) {
    // Fetch current block height
    let result = client.rpc("getblockcount", json!([])).await;
    let start_height = result["result"]
        .as_u64()
        .expect("missing or invalid result field in initial getblockcount");
    let target_height = start_height + target_blocks;

    // Start mining
    let _ = client.rpc("setgenerate", json!([true, 1])).await;

    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let result = client.rpc("getblockcount", json!([])).await;
        let current_height = result["result"]
            .as_u64()
            .expect("missing or invalid result field in getblockcount");

        if current_height >= target_height {
            break;
        }
    }

    // Stop mining
    let _ = client.rpc("setgenerate", json!([false])).await;
}

/** Initialize a container with 2 komodod nodes and their respective clients.
Mines all blocks to CHARLIE_KMD_KEY including the premine amount of 10,000,000,000 coins
Imports CHARLIE_KMD_KEY.wif to miner node then funds funded_key.address with 1,000,000 coins
Imports funded_key.address to miner node and unfunded_key.address to nonminer node

Returns the container and both clients.
The docker container will run until this container falls out of scope.
**/
pub async fn init_komodod_clients(
    funded_key: TestKeyPair<'_>,
    unfunded_key: TestKeyPair<'_>,
) -> (ContainerAsync<GenericImage>, (KomododClient, KomododClient)) {
    let (container, funded_port, unfunded_port) = init_komodod_container().await;
    let miner_client_conf = KomododClientConf {
        ip: IpAddr::from([127, 0, 0, 1]),
        port: funded_port,
        rpcuser: "test".to_string(),
        rpcpassword: "test".to_string(),
        timeout: None,
    };

    let nonminer_client_conf = KomododClientConf {
        ip: IpAddr::from([127, 0, 0, 1]),
        port: unfunded_port,
        rpcuser: "test".to_string(),
        rpcpassword: "test".to_string(),
        timeout: None,
    };

    let miner = KomododClient::new(miner_client_conf).await;
    let nonminer = KomododClient::new(nonminer_client_conf).await;

    // import Charlie's private key to miner node to allow spending the premined coins
    let _ = miner.rpc("importprivkey", json!([CHARLIE_KMD_KEY.wif])).await;

    // Send 1,000,000 coins from Charlie to funded_key.address
    let _ = miner.rpc("sendtoaddress", json!([funded_key.address, 1000000])).await;

    // Import funded_key.address to miner node and unfunded_key.address to nonminer node
    let _ = miner.rpc("importaddress", json!([funded_key.address])).await;
    let _ = nonminer.rpc("importaddress", json!([unfunded_key.address])).await;

    (container, (miner, nonminer))
}

// Wait for `ctx.rpc_started.is_some()` or timeout
pub async fn wait_for_rpc_started(ctx: MmArc, timeout_duration: Duration) -> Result<(), String> {
    let start_time = tokio::time::Instant::now();
    common::log::debug!("Waiting for RPC to start");
    loop {
        {
            if ctx.rpc_port.get().is_some() {
                return Ok(());
            }
        }

        // Check if we've reached the timeout
        if start_time.elapsed() >= timeout_duration {
            common::log::debug!("Timed out waiting for RPC to start");
            return Err("Timed out waiting for RPC to start".to_string());
        }

        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
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
