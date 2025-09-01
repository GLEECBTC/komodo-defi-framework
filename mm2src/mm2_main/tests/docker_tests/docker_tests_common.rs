use coins::z_coin::ZCoin;
pub use common::{block_on, block_on_f01, now_ms, now_sec, wait_until_ms, wait_until_sec};
pub use mm2_number::MmNumber;
use mm2_rpc::data::legacy::BalanceResponse;
pub use mm2_test_helpers::for_tests::{
    check_my_swap_status, check_recent_swaps, enable_eth_coin, enable_native, enable_native_bch, erc20_dev_conf,
    eth_dev_conf, mm_dump, wait_check_stats_swap_status, MarketMakerIt,
};

use super::eth_docker_tests::{erc20_contract_checksum, fill_eth, fill_eth_erc20_with_private_key, swap_contract};
use super::z_coin_docker_tests::z_coin_from_spending_key;
use bitcrypto::{dhash160, ChecksumType};
use chain::TransactionOutput;
use coins::eth::addr_from_raw_pubkey;
use coins::qrc20::rpc_clients::for_tests::Qrc20NativeWalletOps;
use coins::qrc20::{qrc20_coin_with_priv_key, Qrc20ActivationParams, Qrc20Coin};
use coins::utxo::bch::{bch_coin_with_priv_key, BchActivationRequest, BchCoin};
use coins::utxo::qtum::{qtum_coin_with_priv_key, QtumBasedCoin, QtumCoin};
use coins::utxo::rpc_clients::{NativeClient, UtxoRpcClientEnum, UtxoRpcClientOps};
use coins::utxo::slp::{slp_genesis_output, SlpOutput, SlpToken};
use coins::utxo::utxo_common::send_outputs_from_my_address;
use coins::utxo::utxo_standard::{utxo_standard_coin_with_priv_key, UtxoStandardCoin};
use coins::utxo::{
    coin_daemon_data_dir, sat_from_big_decimal, zcash_params_path, UtxoActivationParams, UtxoAddressFormat,
    UtxoCoinFields, UtxoCommonOps,
};
use coins::{ConfirmPaymentInput, MarketCoinOps, Transaction};
use crypto::privkey::{key_pair_from_secret, key_pair_from_seed};
use crypto::Secp256k1Secret;
use ethabi::Token;
use ethereum_types::{H160 as H160Eth, U256};
use futures::TryFutureExt;
use http::StatusCode;
use keys::{
    Address, AddressBuilder, AddressHashEnum, AddressPrefix, KeyPair, NetworkAddressPrefixes,
    NetworkPrefix as CashAddrPrefix,
};
use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};
use mm2_number::BigDecimal;
use mm2_test_helpers::get_passphrase;
use mm2_test_helpers::structs::TransactionDetails;
use primitives::hash::{H160, H256};
use script::Builder;
use secp256k1::Secp256k1;
pub use secp256k1::{PublicKey, SecretKey};
use serde_json::{self as json, Value as Json};
pub use std::cell::Cell;
use std::convert::TryFrom;
use std::process::{Command, Stdio};
#[cfg(any(feature = "sepolia-maker-swap-v2-tests", feature = "sepolia-taker-swap-v2-tests"))]
use std::str::FromStr;
pub use std::{env, thread};
use std::{path::PathBuf, sync::Mutex, time::Duration};
use testcontainers::{clients::Cli, core::WaitFor, Container, GenericImage, RunnableImage};
#[cfg(any(feature = "sepolia-maker-swap-v2-tests", feature = "sepolia-taker-swap-v2-tests"))]
use web3::types::Address as EthAddress;
use web3::types::{BlockId, BlockNumber, TransactionRequest};
use web3::{transports::Http, Web3};

lazy_static! {
    static ref MY_COIN_LOCK: Mutex<()> = Mutex::new(());
    static ref MY_COIN1_LOCK: Mutex<()> = Mutex::new(());
    static ref QTUM_LOCK: Mutex<()> = Mutex::new(());
    static ref FOR_SLP_LOCK: Mutex<()> = Mutex::new(());
    static ref ZOMBIE_LOCK: Mutex<()> = Mutex::new(());
    pub static ref SLP_TOKEN_ID: Mutex<H256> = Mutex::new(H256::default());
    // Private keys supplied with 1000 SLP tokens on tests initialization.
    // Due to the SLP protocol limitations only 19 outputs (18 + change) can be sent in one transaction, which is sufficient for now though.
    // Supply more privkeys when 18 will be not enough.
    pub static ref SLP_TOKEN_OWNERS: Mutex<Vec<[u8; 32]>> = Mutex::new(Vec::with_capacity(18));
    pub static ref MM_CTX: MmArc = MmCtxBuilder::new().with_conf(json!({"coins":[eth_dev_conf()],"use_trading_proto_v2": true})).into_mm_arc();
    /// We need a second `MmCtx` instance when we use the same private keys for Maker and Taker across various tests.
    /// When enabling coins for both Maker and Taker, two distinct coin instances are created.
    /// This means that different instances of the same coin should have separate global nonce locks.
    /// Utilizing different `MmCtx` instances allows us to assign Maker and Taker coins to separate `CoinsCtx`.
    /// This approach addresses the `replacement transaction` issue, which occurs when different transactions share the same nonce.
    pub static ref MM_CTX1: MmArc = MmCtxBuilder::new().with_conf(json!({"use_trading_proto_v2": true})).into_mm_arc();
    pub static ref GETH_WEB3: Web3<Http> = Web3::new(Http::new(GETH_RPC_URL).unwrap());
    // Mutex used to prevent nonce re-usage during funding addresses used in tests
    pub static ref GETH_NONCE_LOCK: Mutex<()> = Mutex::new(());
}

#[cfg(any(feature = "sepolia-maker-swap-v2-tests", feature = "sepolia-taker-swap-v2-tests"))]
lazy_static! {
    pub static ref SEPOLIA_WEB3: Web3<Http> = Web3::new(Http::new(SEPOLIA_RPC_URL).unwrap());
    pub static ref SEPOLIA_NONCE_LOCK: Mutex<()> = Mutex::new(());
    pub static ref SEPOLIA_TESTS_LOCK: Mutex<()> = Mutex::new(());
}

pub static mut QICK_TOKEN_ADDRESS: Option<H160Eth> = None;
pub static mut QORTY_TOKEN_ADDRESS: Option<H160Eth> = None;
pub static mut QRC20_SWAP_CONTRACT_ADDRESS: Option<H160Eth> = None;
pub static mut QTUM_CONF_PATH: Option<PathBuf> = None;
/// The account supplied with ETH on Geth dev node creation
pub static mut GETH_ACCOUNT: H160Eth = H160Eth::zero();
/// ERC20 token address on Geth dev node
pub static mut GETH_ERC20_CONTRACT: H160Eth = H160Eth::zero();
#[cfg(any(feature = "sepolia-maker-swap-v2-tests", feature = "sepolia-taker-swap-v2-tests"))]
pub static mut SEPOLIA_ERC20_CONTRACT: H160Eth = H160Eth::zero();
/// Swap contract address on Geth dev node
pub static mut GETH_SWAP_CONTRACT: H160Eth = H160Eth::zero();
/// Maker Swap V2 contract address on Geth dev node
pub static mut GETH_MAKER_SWAP_V2: H160Eth = H160Eth::zero();
/// Taker Swap V2 contract address on Geth dev node
pub static mut GETH_TAKER_SWAP_V2: H160Eth = H160Eth::zero();
#[cfg(any(feature = "sepolia-maker-swap-v2-tests", feature = "sepolia-taker-swap-v2-tests"))]
pub static mut SEPOLIA_TAKER_SWAP_V2: H160Eth = H160Eth::zero();
#[cfg(any(feature = "sepolia-maker-swap-v2-tests", feature = "sepolia-taker-swap-v2-tests"))]
pub static mut SEPOLIA_MAKER_SWAP_V2: H160Eth = H160Eth::zero();
/// Swap contract (with watchers support) address on Geth dev node
pub static mut GETH_WATCHERS_SWAP_CONTRACT: H160Eth = H160Eth::zero();
/// ERC721 token address on Geth dev node
pub static mut GETH_ERC721_CONTRACT: H160Eth = H160Eth::zero();
/// ERC1155 token address on Geth dev node
pub static mut GETH_ERC1155_CONTRACT: H160Eth = H160Eth::zero();
/// NFT Maker Swap V2 contract address on Geth dev node
pub static mut GETH_NFT_MAKER_SWAP_V2: H160Eth = H160Eth::zero();
#[cfg(any(feature = "sepolia-maker-swap-v2-tests", feature = "sepolia-taker-swap-v2-tests"))]
/// NFT Maker Swap V2 contract address on Sepolia testnet
pub static mut SEPOLIA_ETOMIC_MAKER_NFT_SWAP_V2: H160Eth = H160Eth::zero();
pub static GETH_RPC_URL: &str = "http://127.0.0.1:8545";
#[cfg(any(feature = "sepolia-maker-swap-v2-tests", feature = "sepolia-taker-swap-v2-tests"))]
pub static SEPOLIA_RPC_URL: &str = "https://ethereum-sepolia-rpc.publicnode.com";

// use thread local to affect only the current running test
thread_local! {
    /// Set test dex pubkey as Taker (to check DexFee::NoFee)
    pub static SET_BURN_PUBKEY_TO_ALICE: Cell<bool> = const { Cell::new(false) };
}

pub const UTXO_ASSET_DOCKER_IMAGE: &str = "docker.io/artempikulin/testblockchain";
pub const UTXO_ASSET_DOCKER_IMAGE_WITH_TAG: &str = "docker.io/artempikulin/testblockchain:multiarch";
pub const GETH_DOCKER_IMAGE: &str = "docker.io/ethereum/client-go";
pub const GETH_DOCKER_IMAGE_WITH_TAG: &str = "docker.io/ethereum/client-go:stable";
pub const ZOMBIE_ASSET_DOCKER_IMAGE: &str = "docker.io/borngraced/zombietestrunner";
pub const ZOMBIE_ASSET_DOCKER_IMAGE_WITH_TAG: &str = "docker.io/borngraced/zombietestrunner:multiarch";

#[allow(dead_code)]
pub const SIA_DOCKER_IMAGE: &str = "docker.io/alrighttt/walletd-komodo";
#[allow(dead_code)]
pub const SIA_DOCKER_IMAGE_WITH_TAG: &str = "docker.io/alrighttt/walletd-komodo:latest";

pub const NUCLEUS_IMAGE: &str = "docker.io/komodoofficial/nucleusd";
pub const ATOM_IMAGE_WITH_TAG: &str = "docker.io/komodoofficial/gaiad:kdf-ci";
pub const IBC_RELAYER_IMAGE_WITH_TAG: &str = "docker.io/komodoofficial/ibc-relayer:kdf-ci";

pub const QTUM_ADDRESS_LABEL: &str = "MM2_ADDRESS_LABEL";

/// ERC721_TEST_TOKEN has additional mint function
pub const ERC721_TEST_ABI: &str = include_str!("../../../mm2_test_helpers/dummy_files/erc721_test_abi.json");
/// ERC1155_TEST_TOKEN has additional mint function
pub const ERC1155_TEST_ABI: &str = include_str!("../../../mm2_test_helpers/dummy_files/erc1155_test_abi.json");

/// Ticker of MYCOIN dockerized blockchain.
pub const MYCOIN: &str = "MYCOIN";
/// Ticker of MYCOIN1 dockerized blockchain.
pub const MYCOIN1: &str = "MYCOIN1";

pub const ERC20_TOKEN_BYTES: &str = include_str!("../../../mm2_test_helpers/contract_bytes/erc20_token_bytes");
pub const SWAP_CONTRACT_BYTES: &str = include_str!("../../../mm2_test_helpers/contract_bytes/swap_contract_bytes");
pub const WATCHERS_SWAP_CONTRACT_BYTES: &str =
    include_str!("../../../mm2_test_helpers/contract_bytes/watchers_swap_contract_bytes");
pub const ERC721_TEST_TOKEN_BYTES: &str =
    include_str!("../../../mm2_test_helpers/contract_bytes/erc721_test_token_bytes");
pub const ERC1155_TEST_TOKEN_BYTES: &str =
    include_str!("../../../mm2_test_helpers/contract_bytes/erc1155_test_token_bytes");
/// https://github.com/KomodoPlatform/etomic-swap/blob/5e15641cbf41766cd5b37b4d71842c270773f788/contracts/EtomicSwapMakerNftV2.sol
pub const NFT_MAKER_SWAP_V2_BYTES: &str =
    include_str!("../../../mm2_test_helpers/contract_bytes/nft_maker_swap_v2_bytes");
/// https://github.com/KomodoPlatform/etomic-swap/blob/5e15641cbf41766cd5b37b4d71842c270773f788/contracts/EtomicSwapMakerNftV2.sol
pub const MAKER_SWAP_V2_BYTES: &str = include_str!("../../../mm2_test_helpers/contract_bytes/maker_swap_v2_bytes");
/// https://github.com/KomodoPlatform/etomic-swap/blob/5e15641cbf41766cd5b37b4d71842c270773f788/contracts/EtomicSwapTakerV2.sol
pub const TAKER_SWAP_V2_BYTES: &str = include_str!("../../../mm2_test_helpers/contract_bytes/taker_swap_v2_bytes");

pub trait CoinDockerOps {
    fn rpc_client(&self) -> &UtxoRpcClientEnum;

    fn native_client(&self) -> &NativeClient {
        match self.rpc_client() {
            UtxoRpcClientEnum::Native(native) => native,
            _ => panic!("UtxoRpcClientEnum::Native is expected"),
        }
    }

    fn wait_ready(&self, expected_tx_version: i32) {
        let timeout = wait_until_ms(120000);
        loop {
            match block_on_f01(self.rpc_client().get_block_count()) {
                Ok(n) => {
                    if n > 1 {
                        if let UtxoRpcClientEnum::Native(client) = self.rpc_client() {
                            let hash = block_on_f01(client.get_block_hash(n)).unwrap();
                            let block = block_on_f01(client.get_block(hash)).unwrap();
                            let coinbase = block_on_f01(client.get_verbose_transaction(&block.tx[0])).unwrap();
                            log!("Coinbase tx {:?} in block {}", coinbase, n);
                            if coinbase.version == expected_tx_version {
                                break;
                            }
                        }
                    }
                },
                Err(e) => log!("{:?}", e),
            }
            assert!(now_ms() < timeout, "Test timed out");
            thread::sleep(Duration::from_secs(1));
        }
    }
}

pub struct UtxoAssetDockerOps {
    #[allow(dead_code)]
    ctx: MmArc,
    coin: UtxoStandardCoin,
}

impl CoinDockerOps for UtxoAssetDockerOps {
    fn rpc_client(&self) -> &UtxoRpcClientEnum {
        &self.coin.as_ref().rpc_client
    }
}

impl UtxoAssetDockerOps {
    pub fn from_ticker(ticker: &str) -> UtxoAssetDockerOps {
        let conf = json!({"coin": ticker, "asset": ticker, "txfee": 1000, "network": "regtest"});
        let req = json!({"method":"enable"});
        let priv_key = Secp256k1Secret::from("809465b17d0a4ddb3e4c69e8f23c2cabad868f51f8bed5c765ad1d6516c3306f");
        let ctx = MmCtxBuilder::new().into_mm_arc();
        let params = UtxoActivationParams::from_legacy_req(&req).unwrap();

        let coin = block_on(utxo_standard_coin_with_priv_key(&ctx, ticker, &conf, &params, priv_key)).unwrap();
        UtxoAssetDockerOps { ctx, coin }
    }
}

pub struct ZCoinAssetDockerOps {
    #[allow(dead_code)]
    ctx: MmArc,
    coin: ZCoin,
}

impl CoinDockerOps for ZCoinAssetDockerOps {
    fn rpc_client(&self) -> &UtxoRpcClientEnum {
        &self.coin.as_ref().rpc_client
    }
}

impl ZCoinAssetDockerOps {
    pub fn new() -> ZCoinAssetDockerOps {
        let (ctx, coin) = block_on(z_coin_from_spending_key("secret-extended-key-main1q0k2ga2cqqqqpq8m8j6yl0say83cagrqp53zqz54w38ezs8ly9ly5ptamqwfpq85u87w0df4k8t2lwyde3n9v0gcr69nu4ryv60t0kfcsvkr8h83skwqex2nf0vr32794fmzk89cpmjptzc22lgu5wfhhp8lgf3f5vn2l3sge0udvxnm95k6dtxj2jwlfyccnum7nz297ecyhmd5ph526pxndww0rqq0qly84l635mec0x4yedf95hzn6kcgq8yxts26k98j9g32kjc8y83fe", "fe"));

        ZCoinAssetDockerOps { ctx, coin }
    }
}

pub struct BchDockerOps {
    #[allow(dead_code)]
    ctx: MmArc,
    coin: BchCoin,
}

impl BchDockerOps {
    pub fn from_ticker(ticker: &str) -> BchDockerOps {
        let conf =
            json!({"coin": ticker,"asset": ticker,"txfee":1000,"network": "regtest","txversion":4,"overwintered":1});
        let req = json!({"method":"enable", "bchd_urls": [], "allow_slp_unsafe_conf": true});
        let priv_key = Secp256k1Secret::from("809465b17d0a4ddb3e4c69e8f23c2cabad868f51f8bed5c765ad1d6516c3306f");
        let ctx = MmCtxBuilder::new().into_mm_arc();
        let params = BchActivationRequest::from_legacy_req(&req).unwrap();

        let coin = block_on(bch_coin_with_priv_key(
            &ctx,
            ticker,
            &conf,
            params,
            CashAddrPrefix::SlpTest,
            priv_key,
        ))
        .unwrap();
        BchDockerOps { ctx, coin }
    }

    pub fn initialize_slp(&self) {
        fill_address(&self.coin, &self.coin.my_address().unwrap(), 100000.into(), 30);
        let mut slp_privkeys = vec![];

        let slp_genesis_op_ret = slp_genesis_output("ADEXSLP", "ADEXSLP", None, None, 8, None, 1000000_00000000);
        let slp_genesis = TransactionOutput {
            value: self.coin.as_ref().dust_amount,
            script_pubkey: Builder::build_p2pkh(&self.coin.my_public_key().unwrap().address_hash().into()).to_bytes(),
        };

        let mut bch_outputs = vec![slp_genesis_op_ret, slp_genesis];
        let mut slp_outputs = vec![];

        for _ in 0..18 {
            let key_pair = KeyPair::random_compressed();
            let address = AddressBuilder::new(
                Default::default(),
                Default::default(),
                self.coin.as_ref().conf.address_prefixes.clone(),
                None,
            )
            .as_pkh_from_pk(*key_pair.public())
            .build()
            .expect("valid address props");

            block_on_f01(
                self.native_client()
                    .import_address(&address.to_string(), &address.to_string(), false),
            )
            .unwrap();

            let script_pubkey = Builder::build_p2pkh(&key_pair.public().address_hash().into());

            bch_outputs.push(TransactionOutput {
                value: 1000_00000000,
                script_pubkey: script_pubkey.to_bytes(),
            });

            slp_outputs.push(SlpOutput {
                amount: 1000_00000000,
                script_pubkey: script_pubkey.to_bytes(),
            });
            slp_privkeys.push(*key_pair.private_ref());
        }

        let slp_genesis_tx = block_on_f01(send_outputs_from_my_address(self.coin.clone(), bch_outputs)).unwrap();
        let confirm_payment_input = ConfirmPaymentInput {
            payment_tx: slp_genesis_tx.tx_hex(),
            confirmations: 1,
            requires_nota: false,
            wait_until: wait_until_sec(30),
            check_every: 1,
        };
        block_on_f01(self.coin.wait_for_confirmations(confirm_payment_input)).unwrap();

        let adex_slp = SlpToken::new(
            8,
            "ADEXSLP".into(),
            <&[u8; 32]>::try_from(slp_genesis_tx.tx_hash_as_bytes().as_slice())
                .unwrap()
                .into(),
            self.coin.clone(),
            1,
        )
        .unwrap();

        let tx = block_on(adex_slp.send_slp_outputs(slp_outputs)).unwrap();
        let confirm_payment_input = ConfirmPaymentInput {
            payment_tx: tx.tx_hex(),
            confirmations: 1,
            requires_nota: false,
            wait_until: wait_until_sec(30),
            check_every: 1,
        };
        block_on_f01(self.coin.wait_for_confirmations(confirm_payment_input)).unwrap();
        *SLP_TOKEN_OWNERS.lock().unwrap() = slp_privkeys;
        *SLP_TOKEN_ID.lock().unwrap() = <[u8; 32]>::try_from(slp_genesis_tx.tx_hash_as_bytes().as_slice())
            .unwrap()
            .into();
    }
}

impl CoinDockerOps for BchDockerOps {
    fn rpc_client(&self) -> &UtxoRpcClientEnum {
        &self.coin.as_ref().rpc_client
    }
}

pub struct DockerNode<'a> {
    #[allow(dead_code)]
    pub container: Container<'a, GenericImage>,
    #[allow(dead_code)]
    pub ticker: String,
    #[allow(dead_code)]
    pub port: u16,
}

pub fn random_secp256k1_secret() -> Secp256k1Secret {
    let priv_key = SecretKey::new(&mut rand6::thread_rng());
    Secp256k1Secret::from(*priv_key.as_ref())
}

pub fn utxo_asset_docker_node<'a>(docker: &'a Cli, ticker: &'static str, port: u16) -> DockerNode<'a> {
    let image = GenericImage::new(UTXO_ASSET_DOCKER_IMAGE, "multiarch")
        .with_volume(zcash_params_path().display().to_string(), "/root/.zcash-params")
        .with_env_var("CLIENTS", "2")
        .with_env_var("CHAIN", ticker)
        .with_env_var("TEST_ADDY", "R9imXLs1hEcU9KbFDQq2hJEEJ1P5UoekaF")
        .with_env_var("TEST_WIF", "UqqW7f766rADem9heD8vSBvvrdfJb3zg5r8du9rJxPtccjWf7RG9")
        .with_env_var(
            "TEST_PUBKEY",
            "021607076d7a2cb148d542fb9644c04ffc22d2cca752f80755a0402a24c567b17a",
        )
        .with_env_var("DAEMON_URL", "http://test:test@127.0.0.1:7000")
        .with_env_var("COIN", "Komodo")
        .with_env_var("COIN_RPC_PORT", port.to_string())
        .with_wait_for(WaitFor::message_on_stdout("config is ready"));
    let image = RunnableImage::from(image).with_mapped_port((port, port));
    let container = docker.run(image);
    let mut conf_path = coin_daemon_data_dir(ticker, true);
    std::fs::create_dir_all(&conf_path).unwrap();
    conf_path.push(format!("{ticker}.conf"));
    Command::new("docker")
        .arg("cp")
        .arg(format!("{}:/data/node_0/{}.conf", container.id(), ticker))
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
    DockerNode {
        container,
        ticker: ticker.into(),
        port,
    }
}

pub fn geth_docker_node<'a>(docker: &'a Cli, ticker: &'static str, port: u16) -> DockerNode<'a> {
    let image = GenericImage::new(GETH_DOCKER_IMAGE, "stable");
    let args = vec!["--dev".into(), "--http".into(), "--http.addr=0.0.0.0".into()];
    let image = RunnableImage::from((image, args)).with_mapped_port((port, port));
    let container = docker.run(image);
    DockerNode {
        container,
        ticker: ticker.into(),
        port,
    }
}

#[allow(dead_code)]
pub fn sia_docker_node<'a>(docker: &'a Cli, ticker: &'static str, port: u16) -> DockerNode<'a> {
    let image =
        GenericImage::new(SIA_DOCKER_IMAGE, "latest").with_env_var("WALLETD_API_PASSWORD", "password".to_string());
    let args = vec![];
    let image = RunnableImage::from((image, args))
        .with_mapped_port((port, port))
        .with_container_name("sia-docker");
    let container = docker.run(image);
    DockerNode {
        container,
        ticker: ticker.into(),
        port,
    }
}

pub fn nucleus_node(docker: &'_ Cli, runtime_dir: PathBuf) -> DockerNode<'_> {
    let nucleus_node_runtime_dir = runtime_dir.join("nucleus-testnet-data");
    assert!(nucleus_node_runtime_dir.exists());

    let image = GenericImage::new(NUCLEUS_IMAGE, "latest")
        .with_volume(nucleus_node_runtime_dir.to_str().unwrap(), "/root/.nucleus");
    let image = RunnableImage::from((image, vec![])).with_network("host");
    let container = docker.run(image);

    DockerNode {
        container,
        ticker: "NUCLEUS-TEST".to_owned(),
        port: Default::default(), // This doesn't need to be the correct value as we are using the host network.
    }
}

pub fn atom_node(docker: &'_ Cli, runtime_dir: PathBuf) -> DockerNode<'_> {
    let atom_node_runtime_dir = runtime_dir.join("atom-testnet-data");
    assert!(atom_node_runtime_dir.exists());

    let (image, tag) = ATOM_IMAGE_WITH_TAG.rsplit_once(':').unwrap();
    let image = GenericImage::new(image, tag).with_volume(atom_node_runtime_dir.to_str().unwrap(), "/root/.gaia");
    let image = RunnableImage::from((image, vec![])).with_network("host");
    let container = docker.run(image);

    DockerNode {
        container,
        ticker: "ATOM-TEST".to_owned(),
        port: Default::default(), // This doesn't need to be the correct value as we are using the host network.
    }
}

pub fn ibc_relayer_node(docker: &'_ Cli, runtime_dir: PathBuf) -> DockerNode<'_> {
    let relayer_node_runtime_dir = runtime_dir.join("ibc-relayer-data");
    assert!(relayer_node_runtime_dir.exists());

    let (image, tag) = IBC_RELAYER_IMAGE_WITH_TAG.rsplit_once(':').unwrap();
    let image = GenericImage::new(image, tag).with_volume(relayer_node_runtime_dir.to_str().unwrap(), "/root/.relayer");
    let image = RunnableImage::from((image, vec![])).with_network("host");
    let container = docker.run(image);

    DockerNode {
        container,
        ticker: Default::default(), // This isn't an asset node.
        port: Default::default(),   // This doesn't need to be the correct value as we are using the host network.
    }
}

pub fn zombie_asset_docker_node(docker: &Cli, port: u16) -> DockerNode<'_> {
    let image = GenericImage::new(ZOMBIE_ASSET_DOCKER_IMAGE, "multiarch")
        .with_volume(zcash_params_path().display().to_string(), "/root/.zcash-params")
        .with_env_var("COIN_RPC_PORT", port.to_string())
        .with_wait_for(WaitFor::message_on_stdout("config is ready"));

    let image = RunnableImage::from(image).with_mapped_port((port, port));
    let container = docker.run(image);
    let config_ticker = "ZOMBIE";
    let mut conf_path = coin_daemon_data_dir(config_ticker, true);

    std::fs::create_dir_all(&conf_path).unwrap();
    conf_path.push(format!("{config_ticker}.conf"));
    Command::new("docker")
        .arg("cp")
        .arg(format!("{}:/data/node_0/{}.conf", container.id(), config_ticker))
        .arg(&conf_path)
        .status()
        .expect("Failed to execute docker command");

    let timeout = wait_until_ms(3000);
    while !conf_path.exists() {
        assert!(now_ms() < timeout, "Test timed out");
    }

    DockerNode {
        container,
        ticker: config_ticker.into(),
        port,
    }
}

pub fn rmd160_from_priv(privkey: Secp256k1Secret) -> H160 {
    let secret = SecretKey::from_slice(privkey.as_slice()).unwrap();
    let public = PublicKey::from_secret_key(&Secp256k1::new(), &secret);
    dhash160(&public.serialize())
}

pub fn get_prefilled_slp_privkey() -> [u8; 32] {
    SLP_TOKEN_OWNERS.lock().unwrap().remove(0)
}

pub fn get_slp_token_id() -> String {
    hex::encode(SLP_TOKEN_ID.lock().unwrap().as_slice())
}

pub fn import_address<T>(coin: &T)
where
    T: MarketCoinOps + AsRef<UtxoCoinFields>,
{
    let mutex = match coin.ticker() {
        "MYCOIN" => &*MY_COIN_LOCK,
        "MYCOIN1" => &*MY_COIN1_LOCK,
        "QTUM" | "QICK" | "QORTY" => &*QTUM_LOCK,
        "FORSLP" => &*FOR_SLP_LOCK,
        ticker => panic!("Unknown ticker {}", ticker),
    };
    let _lock = mutex.lock().unwrap();

    match coin.as_ref().rpc_client {
        UtxoRpcClientEnum::Native(ref native) => {
            let my_address = coin.my_address().unwrap();
            block_on_f01(native.import_address(&my_address, &my_address, false)).unwrap()
        },
        UtxoRpcClientEnum::Electrum(_) => panic!("Expected NativeClient"),
    }
}

/// Build `Qrc20Coin` from ticker and privkey without filling the balance.
pub fn qrc20_coin_from_privkey(ticker: &str, priv_key: Secp256k1Secret) -> (MmArc, Qrc20Coin) {
    let (contract_address, swap_contract_address) = unsafe {
        let contract_address = match ticker {
            "QICK" => QICK_TOKEN_ADDRESS.expect("QICK_TOKEN_ADDRESS must be set already"),
            "QORTY" => QORTY_TOKEN_ADDRESS.expect("QORTY_TOKEN_ADDRESS must be set already"),
            _ => panic!("Expected QICK or QORTY ticker"),
        };
        (
            contract_address,
            QRC20_SWAP_CONTRACT_ADDRESS.expect("QRC20_SWAP_CONTRACT_ADDRESS must be set already"),
        )
    };
    let platform = "QTUM";
    let ctx = MmCtxBuilder::new().into_mm_arc();
    let confpath = unsafe { QTUM_CONF_PATH.as_ref().expect("Qtum config is not set yet") };
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

    import_address(&coin);
    (ctx, coin)
}

fn qrc20_coin_conf_item(ticker: &str) -> Json {
    let contract_address = unsafe {
        match ticker {
            "QICK" => QICK_TOKEN_ADDRESS.expect("QICK_TOKEN_ADDRESS must be set already"),
            "QORTY" => QORTY_TOKEN_ADDRESS.expect("QORTY_TOKEN_ADDRESS must be set already"),
            _ => panic!("Expected either QICK or QORTY ticker, found {}", ticker),
        }
    };
    let contract_address = format!("{contract_address:#02x}");

    let confpath = unsafe { QTUM_CONF_PATH.as_ref().expect("Qtum config is not set yet") };
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

/// Build asset `UtxoStandardCoin` from ticker and privkey without filling the balance.
pub fn utxo_coin_from_privkey(ticker: &str, priv_key: Secp256k1Secret) -> (MmArc, UtxoStandardCoin) {
    let ctx = MmCtxBuilder::new().into_mm_arc();
    let conf = json!({"coin":ticker,"asset":ticker,"txversion":4,"overwintered":1,"txfee":1000,"network":"regtest"});
    let req = json!({"method":"enable"});
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let coin = block_on(utxo_standard_coin_with_priv_key(&ctx, ticker, &conf, &params, priv_key)).unwrap();
    import_address(&coin);
    (ctx, coin)
}

/// Create a UTXO coin for the given privkey and fill it's address with the specified balance.
pub fn generate_utxo_coin_with_privkey(ticker: &str, balance: BigDecimal, priv_key: Secp256k1Secret) {
    let (_, coin) = utxo_coin_from_privkey(ticker, priv_key);
    let timeout = 30; // timeout if test takes more than 30 seconds to run
    let my_address = coin.my_address().expect("!my_address");
    fill_address(&coin, &my_address, balance, timeout);
}

/// Generate random privkey, create a UTXO coin and fill it's address with the specified balance.
pub fn generate_utxo_coin_with_random_privkey(
    ticker: &str,
    balance: BigDecimal,
) -> (MmArc, UtxoStandardCoin, Secp256k1Secret) {
    let priv_key = random_secp256k1_secret();
    let (ctx, coin) = utxo_coin_from_privkey(ticker, priv_key);
    let timeout = 30; // timeout if test takes more than 30 seconds to run
    let my_address = coin.my_address().expect("!my_address");
    fill_address(&coin, &my_address, balance, timeout);
    (ctx, coin, priv_key)
}

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

pub fn fill_qrc20_address(coin: &Qrc20Coin, amount: BigDecimal, timeout: u64) {
    // prevent concurrent fill since daemon RPC returns errors if send_to_address
    // is called concurrently (insufficient funds) and it also may return other errors
    // if previous transaction is not confirmed yet
    let _lock = QTUM_LOCK.lock().unwrap();
    let timeout = wait_until_sec(timeout);
    let client = match coin.as_ref().rpc_client {
        UtxoRpcClientEnum::Native(ref client) => client,
        UtxoRpcClientEnum::Electrum(_) => panic!("Expected NativeClient"),
    };

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

/// Generate random privkey, create a QRC20 coin and fill it's address with the specified balance.
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

pub fn generate_qtum_coin_with_random_privkey(
    ticker: &str,
    balance: BigDecimal,
    txfee: Option<u64>,
) -> (MmArc, QtumCoin, [u8; 32]) {
    let confpath = unsafe { QTUM_CONF_PATH.as_ref().expect("Qtum config is not set yet") };
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

pub fn generate_segwit_qtum_coin_with_random_privkey(
    ticker: &str,
    balance: BigDecimal,
    txfee: Option<u64>,
) -> (MmArc, QtumCoin, Secp256k1Secret) {
    let confpath = unsafe { QTUM_CONF_PATH.as_ref().expect("Qtum config is not set yet") };
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

pub fn fill_address<T>(coin: &T, address: &str, amount: BigDecimal, timeout: u64)
where
    T: MarketCoinOps + AsRef<UtxoCoinFields>,
{
    // prevent concurrent fill since daemon RPC returns errors if send_to_address
    // is called concurrently (insufficient funds) and it also may return other errors
    // if previous transaction is not confirmed yet
    let mutex = match coin.ticker() {
        "MYCOIN" => &*MY_COIN_LOCK,
        "MYCOIN1" => &*MY_COIN1_LOCK,
        "QTUM" | "QICK" | "QORTY" => &*QTUM_LOCK,
        "FORSLP" => &*FOR_SLP_LOCK,
        ticker => panic!("Unknown ticker {}", ticker),
    };
    let _lock = mutex.lock().unwrap();
    let timeout = wait_until_sec(timeout);

    if let UtxoRpcClientEnum::Native(client) = &coin.as_ref().rpc_client {
        block_on_f01(client.import_address(address, address, false)).unwrap();
        let hash = block_on_f01(client.send_to_address(address, &amount)).unwrap();
        let tx_bytes = block_on_f01(client.get_transaction_bytes(&hash)).unwrap();
        let confirm_payment_input = ConfirmPaymentInput {
            payment_tx: tx_bytes.clone().0,
            confirmations: 1,
            requires_nota: false,
            wait_until: timeout,
            check_every: 1,
        };
        block_on_f01(coin.wait_for_confirmations(confirm_payment_input)).unwrap();
        log!("{:02x}", tx_bytes);
        loop {
            let unspents = block_on_f01(client.list_unspent_impl(0, i32::MAX, vec![address.to_string()])).unwrap();
            if !unspents.is_empty() {
                break;
            }
            assert!(now_sec() < timeout, "Test timed out");
            thread::sleep(Duration::from_secs(1));
        }
    };
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

pub async fn enable_qrc20_native(mm: &MarketMakerIt, coin: &str) -> Json {
    let swap_contract_address =
        unsafe { QRC20_SWAP_CONTRACT_ADDRESS.expect("QRC20_SWAP_CONTRACT_ADDRESS must be set already") };

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

pub fn trade_base_rel((base, rel): (&str, &str)) {
    /// Generate a wallet with the random private key and fill the wallet with Qtum (required by gas_fee) and specified in `ticker` coin.
    fn generate_and_fill_priv_key(ticker: &str) -> Secp256k1Secret {
        let timeout = 30; // timeout if test takes more than 30 seconds to run

        match ticker {
            "QTUM" => {
                //Segwit QTUM
                wait_for_estimate_smart_fee(timeout).expect("!wait_for_estimate_smart_fee");
                let (_ctx, _coin, priv_key) = generate_segwit_qtum_coin_with_random_privkey("QTUM", 10.into(), Some(0));

                priv_key
            },
            "QICK" | "QORTY" => {
                let priv_key = random_secp256k1_secret();
                let (_ctx, coin) = qrc20_coin_from_privkey(ticker, priv_key);
                let my_address = coin.my_address().expect("!my_address");
                fill_address(&coin, &my_address, 10.into(), timeout);
                fill_qrc20_address(&coin, 10.into(), timeout);

                priv_key
            },
            "MYCOIN" | "MYCOIN1" => {
                let priv_key = random_secp256k1_secret();
                let (_ctx, coin) = utxo_coin_from_privkey(ticker, priv_key);
                let my_address = coin.my_address().expect("!my_address");
                fill_address(&coin, &my_address, 10.into(), timeout);
                // also fill the Qtum
                let (_ctx, coin) = qrc20_coin_from_privkey("QICK", priv_key);
                let my_address = coin.my_address().expect("!my_address");
                fill_address(&coin, &my_address, 10.into(), timeout);

                priv_key
            },
            "ADEXSLP" | "FORSLP" => Secp256k1Secret::from(get_prefilled_slp_privkey()),
            "ETH" | "ERC20DEV" => {
                let priv_key = random_secp256k1_secret();
                fill_eth_erc20_with_private_key(priv_key);
                priv_key
            },
            _ => panic!("Expected either QICK or QORTY or MYCOIN or MYCOIN1, found {}", ticker),
        }
    }

    let bob_priv_key = generate_and_fill_priv_key(base);
    let alice_priv_key = generate_and_fill_priv_key(rel);
    let alice_pubkey_str = hex::encode(
        key_pair_from_secret(&alice_priv_key)
            .expect("valid test key pair")
            .public()
            .to_vec(),
    );

    let mut envs = vec![];
    if SET_BURN_PUBKEY_TO_ALICE.get() {
        envs.push(("TEST_BURN_ADDR_RAW_PUBKEY", alice_pubkey_str.as_str()));
    }
    let confpath = unsafe { QTUM_CONF_PATH.as_ref().expect("Qtum config is not set yet") };
    let coins = json! ([
        eth_dev_conf(),
        erc20_dev_conf(&erc20_contract_checksum()),
        qrc20_coin_conf_item("QICK"),
        qrc20_coin_conf_item("QORTY"),
        {"coin":"MYCOIN","asset":"MYCOIN","required_confirmations":0,"txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"UTXO"}},
        {"coin":"MYCOIN1","asset":"MYCOIN1","required_confirmations":0,"txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"UTXO"}},
        // TODO: check if we should fix protocol "type":"UTXO" to "QTUM" for this and other QTUM coin tests.
        // Maybe we should use a different coin for "UTXO" protocol and make new tests for "QTUM" protocol
        {"coin":"QTUM","asset":"QTUM","required_confirmations":0,"decimals":8,"pubtype":120,"p2shtype":110,"wiftype":128,"segwit":true,"txfee":0,"txfee_volatility_percent":0.1, "dust":72800,
        "mm2":1,"network":"regtest","confpath":confpath,"protocol":{"type":"UTXO"},"bech32_hrp":"qcrt","address_format":{"format":"segwit"}},
        {"coin":"FORSLP","asset":"FORSLP","required_confirmations":0,"txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"BCH","protocol_data":{"slp_prefix":"slptest"}}},
        {"coin":"ADEXSLP","protocol":{"type":"SLPTOKEN","protocol_data":{"decimals":8,"token_id":get_slp_token_id(),"platform":"FORSLP"}}}
    ]);
    let mut mm_bob = block_on(MarketMakerIt::start_with_envs(
        json! ({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(bob_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
        envs.as_slice(),
    ))
    .unwrap();
    let (_bob_dump_log, _bob_dump_dashboard) = mm_dump(&mm_bob.log_path);
    block_on(mm_bob.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();

    let mut mm_alice = block_on(MarketMakerIt::start_with_envs(
        json! ({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(alice_priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "seednodes": vec![format!("{}", mm_bob.ip)],
        }),
        "pass".to_string(),
        None,
        envs.as_slice(),
    ))
    .unwrap();
    let (_alice_dump_log, _alice_dump_dashboard) = mm_dump(&mm_alice.log_path);
    block_on(mm_alice.wait_for_log(22., |log| log.contains(">>>>>>>>> DEX stats "))).unwrap();

    let swap_contract = format!("0x{}", hex::encode(swap_contract()));
    log!("{:?}", block_on(enable_qrc20_native(&mm_bob, "QICK")));
    log!("{:?}", block_on(enable_qrc20_native(&mm_bob, "QORTY")));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_bob, "QTUM", &[], None)));
    log!("{:?}", block_on(enable_native_bch(&mm_bob, "FORSLP", &[])));
    log!("{:?}", block_on(enable_native(&mm_bob, "ADEXSLP", &[], None)));
    log!(
        "{:?}",
        block_on(enable_eth_coin(
            &mm_bob,
            "ETH",
            &[GETH_RPC_URL],
            &swap_contract,
            None,
            false
        ))
    );
    log!(
        "{:?}",
        block_on(enable_eth_coin(
            &mm_bob,
            "ERC20DEV",
            &[GETH_RPC_URL],
            &swap_contract,
            None,
            false
        ))
    );

    log!("{:?}", block_on(enable_qrc20_native(&mm_alice, "QICK")));
    log!("{:?}", block_on(enable_qrc20_native(&mm_alice, "QORTY")));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "MYCOIN1", &[], None)));
    log!("{:?}", block_on(enable_native(&mm_alice, "QTUM", &[], None)));
    log!("{:?}", block_on(enable_native_bch(&mm_alice, "FORSLP", &[])));
    log!("{:?}", block_on(enable_native(&mm_alice, "ADEXSLP", &[], None)));
    log!(
        "{:?}",
        block_on(enable_eth_coin(
            &mm_alice,
            "ETH",
            &[GETH_RPC_URL],
            &swap_contract,
            None,
            false
        ))
    );
    log!(
        "{:?}",
        block_on(enable_eth_coin(
            &mm_alice,
            "ERC20DEV",
            &[GETH_RPC_URL],
            &swap_contract,
            None,
            false
        ))
    );

    let rc = block_on(mm_bob.rpc(&json! ({
        "userpass": mm_bob.userpass,
        "method": "setprice",
        "base": base,
        "rel": rel,
        "price": 1,
        "volume": "3",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!setprice: {}", rc.1);

    thread::sleep(Duration::from_secs(1));

    log!("Issue alice {}/{} buy request", base, rel);
    let rc = block_on(mm_alice.rpc(&json! ({
        "userpass": mm_alice.userpass,
        "method": "buy",
        "base": base,
        "rel": rel,
        "price": 1,
        "volume": "2",
    })))
    .unwrap();
    assert!(rc.0.is_success(), "!buy: {}", rc.1);
    let buy_json: Json = serde_json::from_str(&rc.1).unwrap();
    let uuid = buy_json["result"]["uuid"].as_str().unwrap().to_owned();

    // ensure the swaps are started
    block_on(mm_bob.wait_for_log(22., |log| {
        log.contains(&format!("Entering the maker_swap_loop {base}/{rel}"))
    }))
    .unwrap();
    block_on(mm_alice.wait_for_log(22., |log| {
        log.contains(&format!("Entering the taker_swap_loop {base}/{rel}"))
    }))
    .unwrap();

    // ensure the swaps are finished
    block_on(mm_bob.wait_for_log(600., |log| log.contains(&format!("[swap uuid={uuid}] Finished")))).unwrap();
    block_on(mm_alice.wait_for_log(600., |log| log.contains(&format!("[swap uuid={uuid}] Finished")))).unwrap();

    log!("Checking alice/taker status..");
    block_on(check_my_swap_status(
        &mm_alice,
        &uuid,
        "2".parse().unwrap(),
        "2".parse().unwrap(),
    ));

    log!("Checking bob/maker status..");
    block_on(check_my_swap_status(
        &mm_bob,
        &uuid,
        "2".parse().unwrap(),
        "2".parse().unwrap(),
    ));

    log!("Checking alice status..");
    block_on(wait_check_stats_swap_status(&mm_alice, &uuid, 240));

    log!("Checking bob status..");
    block_on(wait_check_stats_swap_status(&mm_bob, &uuid, 240));

    log!("Checking alice recent swaps..");
    block_on(check_recent_swaps(&mm_alice, 1));
    log!("Checking bob recent swaps..");
    block_on(check_recent_swaps(&mm_bob, 1));

    block_on(mm_bob.stop()).unwrap();
    block_on(mm_alice.stop()).unwrap();
}

pub fn slp_supplied_node() -> MarketMakerIt {
    let coins = json! ([
        {"coin":"FORSLP","asset":"FORSLP","required_confirmations":0,"txversion":4,"overwintered":1,"txfee":1000,"protocol":{"type":"BCH","protocol_data":{"slp_prefix":"slptest"}}},
        {"coin":"ADEXSLP","protocol":{"type":"SLPTOKEN","protocol_data":{"decimals":8,"token_id":get_slp_token_id(),"platform":"FORSLP"}}}
    ]);

    let priv_key = get_prefilled_slp_privkey();
    MarketMakerIt::start(
        json! ({
            "gui": "nogui",
            "netid": 9000,
            "dht": "on",  // Enable DHT without delay.
            "passphrase": format!("0x{}", hex::encode(priv_key)),
            "coins": coins,
            "rpc_password": "pass",
            "i_am_seed": true,
            "is_bootstrap_node": true
        }),
        "pass".to_string(),
        None,
    )
    .unwrap()
}

pub fn get_balance(mm: &MarketMakerIt, coin: &str) -> BalanceResponse {
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "my_balance",
        "coin": coin,
    })))
    .unwrap();
    assert_eq!(rc.0, StatusCode::OK, "my_balance request failed {}", rc.1);
    json::from_str(&rc.1).unwrap()
}

pub fn utxo_burn_address() -> Address {
    AddressBuilder::new(
        UtxoAddressFormat::Standard,
        ChecksumType::DSHA256,
        NetworkAddressPrefixes {
            p2pkh: [60].into(),
            p2sh: AddressPrefix::default(),
        },
        None,
    )
    .as_pkh(AddressHashEnum::default_address_hash())
    .build()
    .expect("valid address props")
}

pub fn withdraw_max_and_send_v1(mm: &MarketMakerIt, coin: &str, to: &str) -> TransactionDetails {
    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "withdraw",
        "coin": coin,
        "max": true,
        "to": to,
    })))
    .unwrap();
    assert_eq!(rc.0, StatusCode::OK, "withdraw request failed {}", rc.1);
    let tx_details: TransactionDetails = json::from_str(&rc.1).unwrap();

    let rc = block_on(mm.rpc(&json!({
        "userpass": mm.userpass,
        "method": "send_raw_transaction",
        "tx_hex": tx_details.tx_hex,
        "coin": coin,
    })))
    .unwrap();
    assert_eq!(rc.0, StatusCode::OK, "send_raw_transaction request failed {}", rc.1);

    tx_details
}

async fn get_current_gas_limit(web3: &Web3<Http>) {
    match web3.eth().block(BlockId::Number(BlockNumber::Latest)).await {
        Ok(Some(block)) => {
            log!("Current gas limit: {}", block.gas_limit);
        },
        Ok(None) => log!("Latest block information is not available."),
        Err(e) => log!("Failed to fetch the latest block: {}", e),
    }
}

pub fn prepare_ibc_channels(container_id: &str) {
    let exec = |args: &[&str]| {
        Command::new("docker")
            .args(["exec", container_id])
            .args(args)
            .output()
            .unwrap();
    };

    exec(&["rly", "transact", "clients", "nucleus-atom", "--override"]);
    // It takes a couple of seconds for nodes to get into the right state after updating clients.
    // Wait for 5 just to make sure.
    thread::sleep(Duration::from_secs(5));

    exec(&["rly", "transact", "link", "nucleus-atom"]);
}

pub fn wait_until_relayer_container_is_ready(container_id: &str) {
    const Q_RESULT: &str = "0: nucleus-atom         -> chns(✔) clnts(✔) conn(✔) (nucleus-testnet<>cosmoshub-testnet)";

    let mut attempts = 0;
    loop {
        let mut docker = Command::new("docker");
        docker.arg("exec").arg(container_id).args(["rly", "paths", "list"]);

        log!("Running <<{docker:?}>>.");

        let output = docker.stderr(Stdio::inherit()).output().unwrap();
        let output = String::from_utf8(output.stdout).unwrap();
        let output = output.trim();

        if output == Q_RESULT {
            break;
        }
        attempts += 1;

        log!("Expected output {Q_RESULT}, received {output}.");
        if attempts > 10 {
            panic!("Reached max attempts for <<{:?}>>.", docker);
        } else {
            log!("Asking for relayer node status again..");
        }

        thread::sleep(Duration::from_secs(2));
    }
}

pub fn init_geth_node() {
    unsafe {
        block_on(get_current_gas_limit(&GETH_WEB3));
        let gas_price = block_on(GETH_WEB3.eth().gas_price()).unwrap();
        log!("Current gas price: {:?}", gas_price);
        let accounts = block_on(GETH_WEB3.eth().accounts()).unwrap();
        GETH_ACCOUNT = accounts[0];
        log!("GETH ACCOUNT {:?}", GETH_ACCOUNT);

        let tx_request_deploy_erc20 = TransactionRequest {
            from: GETH_ACCOUNT,
            to: None,
            gas: None,
            gas_price: None,
            value: None,
            data: Some(hex::decode(ERC20_TOKEN_BYTES).unwrap().into()),
            nonce: None,
            condition: None,
            transaction_type: None,
            access_list: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        };

        let deploy_erc20_tx_hash = block_on(GETH_WEB3.eth().send_transaction(tx_request_deploy_erc20)).unwrap();
        log!("Sent ERC20 deploy transaction {:?}", deploy_erc20_tx_hash);

        loop {
            let deploy_tx_receipt = match block_on(GETH_WEB3.eth().transaction_receipt(deploy_erc20_tx_hash)) {
                Ok(receipt) => receipt,
                Err(_) => {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                },
            };

            if let Some(receipt) = deploy_tx_receipt {
                GETH_ERC20_CONTRACT = receipt.contract_address.unwrap();
                log!("GETH_ERC20_CONTRACT {:?}", GETH_ERC20_CONTRACT);
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        let tx_request_deploy_swap_contract = TransactionRequest {
            from: GETH_ACCOUNT,
            to: None,
            gas: None,
            gas_price: None,
            value: None,
            data: Some(hex::decode(SWAP_CONTRACT_BYTES).unwrap().into()),
            nonce: None,
            condition: None,
            transaction_type: None,
            access_list: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        };
        let deploy_swap_tx_hash = block_on(GETH_WEB3.eth().send_transaction(tx_request_deploy_swap_contract)).unwrap();
        log!("Sent deploy swap contract transaction {:?}", deploy_swap_tx_hash);

        loop {
            let deploy_swap_tx_receipt = match block_on(GETH_WEB3.eth().transaction_receipt(deploy_swap_tx_hash)) {
                Ok(receipt) => receipt,
                Err(_) => {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                },
            };

            if let Some(receipt) = deploy_swap_tx_receipt {
                GETH_SWAP_CONTRACT = receipt.contract_address.unwrap();
                log!("GETH_SWAP_CONTRACT {:?}", GETH_SWAP_CONTRACT);
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        let tx_request_deploy_maker_swap_contract_v2 = TransactionRequest {
            from: GETH_ACCOUNT,
            to: None,
            gas: None,
            gas_price: None,
            value: None,
            data: Some(hex::decode(MAKER_SWAP_V2_BYTES).unwrap().into()),
            nonce: None,
            condition: None,
            transaction_type: None,
            access_list: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        };
        let deploy_maker_swap_v2_tx_hash = block_on(
            GETH_WEB3
                .eth()
                .send_transaction(tx_request_deploy_maker_swap_contract_v2),
        )
        .unwrap();
        log!(
            "Sent deploy maker swap v2 contract transaction {:?}",
            deploy_maker_swap_v2_tx_hash
        );

        loop {
            let deploy_maker_swap_v2_tx_receipt =
                match block_on(GETH_WEB3.eth().transaction_receipt(deploy_maker_swap_v2_tx_hash)) {
                    Ok(receipt) => receipt,
                    Err(_) => {
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    },
                };

            if let Some(receipt) = deploy_maker_swap_v2_tx_receipt {
                GETH_MAKER_SWAP_V2 = receipt.contract_address.unwrap();
                log!(
                    "GETH_MAKER_SWAP_V2 contract address: {:?}, receipt.status: {:?}",
                    GETH_MAKER_SWAP_V2,
                    receipt.status
                );
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        let dex_fee_addr = Token::Address(GETH_ACCOUNT);
        let params = ethabi::encode(&[dex_fee_addr]);
        let taker_swap_v2_data = format!("{}{}", TAKER_SWAP_V2_BYTES, hex::encode(params));

        let tx_request_deploy_taker_swap_contract_v2 = TransactionRequest {
            from: GETH_ACCOUNT,
            to: None,
            gas: None,
            gas_price: None,
            value: None,
            data: Some(hex::decode(taker_swap_v2_data).unwrap().into()),
            nonce: None,
            condition: None,
            transaction_type: None,
            access_list: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        };
        let deploy_taker_swap_v2_tx_hash = block_on(
            GETH_WEB3
                .eth()
                .send_transaction(tx_request_deploy_taker_swap_contract_v2),
        )
        .unwrap();
        log!(
            "Sent deploy taker swap v2 contract transaction {:?}",
            deploy_taker_swap_v2_tx_hash
        );

        loop {
            let deploy_taker_swap_v2_tx_receipt =
                match block_on(GETH_WEB3.eth().transaction_receipt(deploy_taker_swap_v2_tx_hash)) {
                    Ok(receipt) => receipt,
                    Err(_) => {
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    },
                };

            if let Some(receipt) = deploy_taker_swap_v2_tx_receipt {
                GETH_TAKER_SWAP_V2 = receipt.contract_address.unwrap();
                log!(
                    "GETH_TAKER_SWAP_V2 contract address: {:?}, receipt.status: {:?}",
                    GETH_TAKER_SWAP_V2,
                    receipt.status
                );
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        let tx_request_deploy_watchers_swap_contract = TransactionRequest {
            from: GETH_ACCOUNT,
            to: None,
            gas: None,
            gas_price: None,
            value: None,
            data: Some(hex::decode(WATCHERS_SWAP_CONTRACT_BYTES).unwrap().into()),
            nonce: None,
            condition: None,
            transaction_type: None,
            access_list: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        };
        let deploy_watchers_swap_tx_hash = block_on(
            GETH_WEB3
                .eth()
                .send_transaction(tx_request_deploy_watchers_swap_contract),
        )
        .unwrap();
        log!(
            "Sent deploy watchers swap contract transaction {:?}",
            deploy_watchers_swap_tx_hash
        );

        loop {
            let deploy_watchers_swap_tx_receipt =
                match block_on(GETH_WEB3.eth().transaction_receipt(deploy_watchers_swap_tx_hash)) {
                    Ok(receipt) => receipt,
                    Err(_) => {
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    },
                };

            if let Some(receipt) = deploy_watchers_swap_tx_receipt {
                GETH_WATCHERS_SWAP_CONTRACT = receipt.contract_address.unwrap();
                log!("GETH_WATCHERS_SWAP_CONTRACT {:?}", GETH_WATCHERS_SWAP_CONTRACT);
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        let tx_request_deploy_nft_maker_swap_v2_contract = TransactionRequest {
            from: GETH_ACCOUNT,
            to: None,
            gas: None,
            gas_price: None,
            value: None,
            data: Some(hex::decode(NFT_MAKER_SWAP_V2_BYTES).unwrap().into()),
            nonce: None,
            condition: None,
            transaction_type: None,
            access_list: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        };
        let deploy_nft_maker_swap_v2_tx_hash = block_on(
            GETH_WEB3
                .eth()
                .send_transaction(tx_request_deploy_nft_maker_swap_v2_contract),
        )
        .unwrap();
        log!(
            "Sent deploy nft maker swap v2 contract transaction {:?}",
            deploy_nft_maker_swap_v2_tx_hash
        );

        loop {
            let deploy_nft_maker_swap_v2_tx_receipt =
                match block_on(GETH_WEB3.eth().transaction_receipt(deploy_nft_maker_swap_v2_tx_hash)) {
                    Ok(receipt) => receipt,
                    Err(_) => {
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    },
                };

            if let Some(receipt) = deploy_nft_maker_swap_v2_tx_receipt {
                GETH_NFT_MAKER_SWAP_V2 = receipt.contract_address.unwrap();
                log!(
                    "GETH_NFT_MAKER_SWAP_V2 contact address: {:?}, receipt.status: {:?}",
                    GETH_NFT_MAKER_SWAP_V2,
                    receipt.status
                );
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        let tx_request_deploy_nft_maker_swap_v2_contract = TransactionRequest {
            from: GETH_ACCOUNT,
            to: None,
            gas: None,
            gas_price: None,
            value: None,
            data: Some(hex::decode(NFT_MAKER_SWAP_V2_BYTES).unwrap().into()),
            nonce: None,
            condition: None,
            transaction_type: None,
            access_list: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        };
        let deploy_nft_maker_swap_v2_tx_hash = block_on(
            GETH_WEB3
                .eth()
                .send_transaction(tx_request_deploy_nft_maker_swap_v2_contract),
        )
        .unwrap();
        log!(
            "Sent deploy nft maker swap v2 contract transaction {:?}",
            deploy_nft_maker_swap_v2_tx_hash
        );

        loop {
            let deploy_nft_maker_swap_v2_tx_receipt =
                match block_on(GETH_WEB3.eth().transaction_receipt(deploy_nft_maker_swap_v2_tx_hash)) {
                    Ok(receipt) => receipt,
                    Err(_) => {
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    },
                };

            if let Some(receipt) = deploy_nft_maker_swap_v2_tx_receipt {
                GETH_NFT_MAKER_SWAP_V2 = receipt.contract_address.unwrap();
                log!(
                    "GETH_NFT_MAKER_SWAP_V2 {:?}, receipt.status {:?}",
                    GETH_NFT_MAKER_SWAP_V2,
                    receipt.status
                );
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        let name = Token::String("MyNFT".into());
        let symbol = Token::String("MNFT".into());
        let params = ethabi::encode(&[name, symbol]);
        let erc721_data = format!("{}{}", ERC721_TEST_TOKEN_BYTES, hex::encode(params));

        let tx_request_deploy_erc721 = TransactionRequest {
            from: GETH_ACCOUNT,
            to: None,
            gas: None,
            gas_price: None,
            value: None,
            data: Some(hex::decode(erc721_data).unwrap().into()),
            nonce: None,
            condition: None,
            transaction_type: None,
            access_list: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        };
        let deploy_erc721_tx_hash = block_on(GETH_WEB3.eth().send_transaction(tx_request_deploy_erc721)).unwrap();
        log!("Sent ERC721 deploy transaction {:?}", deploy_erc721_tx_hash);

        loop {
            let deploy_erc721_tx_receipt = match block_on(GETH_WEB3.eth().transaction_receipt(deploy_erc721_tx_hash)) {
                Ok(receipt) => receipt,
                Err(_) => {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                },
            };

            if let Some(receipt) = deploy_erc721_tx_receipt {
                GETH_ERC721_CONTRACT = receipt.contract_address.unwrap();
                log!("GETH_ERC721_CONTRACT {:?}", GETH_ERC721_CONTRACT);
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        let uri = Token::String("MyNFTUri".into());
        let params = ethabi::encode(&[uri]);
        let erc1155_data = format!("{}{}", ERC1155_TEST_TOKEN_BYTES, hex::encode(params));

        let tx_request_deploy_erc1155 = TransactionRequest {
            from: GETH_ACCOUNT,
            to: None,
            gas: None,
            gas_price: None,
            value: None,
            data: Some(hex::decode(erc1155_data).unwrap().into()),
            nonce: None,
            condition: None,
            transaction_type: None,
            access_list: None,
            max_fee_per_gas: None,
            max_priority_fee_per_gas: None,
        };
        let deploy_erc1155_tx_hash = block_on(GETH_WEB3.eth().send_transaction(tx_request_deploy_erc1155)).unwrap();
        log!("Sent ERC1155 deploy transaction {:?}", deploy_erc721_tx_hash);

        loop {
            let deploy_erc1155_tx_receipt = match block_on(GETH_WEB3.eth().transaction_receipt(deploy_erc1155_tx_hash))
            {
                Ok(receipt) => receipt,
                Err(_) => {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                },
            };

            if let Some(receipt) = deploy_erc1155_tx_receipt {
                GETH_ERC1155_CONTRACT = receipt.contract_address.unwrap();
                log!("GETH_ERC1155_CONTRACT {:?}", GETH_ERC1155_CONTRACT);
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }

        #[cfg(any(feature = "sepolia-maker-swap-v2-tests", feature = "sepolia-taker-swap-v2-tests"))]
        {
            SEPOLIA_ETOMIC_MAKER_NFT_SWAP_V2 =
                EthAddress::from_str("0x9eb88cd58605d8fb9b14652d6152727f7e95fb4d").unwrap();
            SEPOLIA_ERC20_CONTRACT = EthAddress::from_str("0xF7b5F8E8555EF7A743f24D3E974E23A3C6cB6638").unwrap();
            SEPOLIA_TAKER_SWAP_V2 = EthAddress::from_str("0x3B19873b81a6B426c8B2323955215F7e89CfF33F").unwrap();
            // deploy tx https://sepolia.etherscan.io/tx/0x6f743d79ecb806f5899a6a801083e33eba9e6f10726af0873af9f39883db7f11
            SEPOLIA_MAKER_SWAP_V2 = EthAddress::from_str("0xf9000589c66Df3573645B59c10aa87594Edc318F").unwrap();
        }
        let alice_passphrase = get_passphrase!(".env.client", "ALICE_PASSPHRASE").unwrap();
        let alice_keypair = key_pair_from_seed(&alice_passphrase).unwrap();
        let alice_eth_addr = addr_from_raw_pubkey(alice_keypair.public()).unwrap();
        // 100 ETH
        fill_eth(alice_eth_addr, U256::from(10).pow(U256::from(20)));

        let bob_passphrase = get_passphrase!(".env.seed", "BOB_PASSPHRASE").unwrap();
        let bob_keypair = key_pair_from_seed(&bob_passphrase).unwrap();
        let bob_eth_addr = addr_from_raw_pubkey(bob_keypair.public()).unwrap();
        // 100 ETH
        fill_eth(bob_eth_addr, U256::from(10).pow(U256::from(20)));
    }
}
