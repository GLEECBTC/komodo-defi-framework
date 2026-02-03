//! Shared ETH/ERC20 helper functions for docker tests.
//!
//! This module provides:
//! - Global state for Geth contracts and accounts
//! - Address getters and checksum helpers
//! - Funding utilities for ETH and ERC20 tokens
//! - Coin creation helpers
//! - Geth initialization with contract deployment

#[cfg(any(feature = "docker-tests-eth", feature = "docker-tests-watchers-eth"))]
use crate::docker_tests::helpers::env::random_secp256k1_secret;
use crate::docker_tests::helpers::env::DockerNode;
use coins::eth::addr_from_raw_pubkey;
#[cfg(any(feature = "docker-tests-eth", feature = "docker-tests-watchers-eth"))]
use coins::eth::EthCoin;
use coins::eth::{checksum_address, eth_coin_from_conf_and_request, ERC20_ABI};
#[cfg(any(feature = "docker-tests-eth", feature = "docker-tests-watchers-eth"))]
use coins::DerivationMethod;
use coins::{lp_coinfind, CoinProtocol, CoinWithDerivationMethod, CoinsContext, PrivKeyBuildPolicy};
use common::block_on;
use common::custom_futures::timeout::FutureTimerExt;
use crypto::privkey::key_pair_from_seed;
#[cfg(any(feature = "docker-tests-eth", feature = "docker-tests-integration"))]
use crypto::Secp256k1Secret;
use ethabi::Token;
use ethereum_types::{H160 as H160Eth, U256};
use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};
use mm2_test_helpers::for_tests::{erc20_dev_conf, eth_dev_conf, usdt_dev_conf};
use mm2_test_helpers::get_passphrase;
use serde_json::json;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use testcontainers::runners::SyncRunner;
use testcontainers::{GenericImage, RunnableImage};
use web3::contract::{Contract, Options};
use web3::types::{Address, BlockId, BlockNumber, TransactionRequest, H256};
use web3::{transports::Http, Web3};

// =============================================================================
// Global state - statics for Geth node
// =============================================================================

lazy_static! {
    /// Web3 instance connected to the Geth dev node
    pub static ref GETH_WEB3: Web3<Http> = Web3::new(Http::new(GETH_RPC_URL).unwrap());
    /// Mutex used to prevent nonce re-usage during funding addresses used in tests
    pub static ref GETH_NONCE_LOCK: Mutex<()> = Mutex::new(());

    /// Shared MmArc context for single-instance tests
    pub static ref MM_CTX: MmArc = MmCtxBuilder::new()
        .with_conf(json!({"coins":[eth_dev_conf()],"use_trading_proto_v2": true}))
        .into_mm_arc();

    /// Second MmCtx instance for Maker/Taker tests using same private keys.
    ///
    /// When enabling coins for both Maker and Taker, two distinct coin instances are created.
    /// Different instances of the same coin should have separate global nonce locks.
    /// Using different MmCtx instances assigns Maker and Taker coins to separate CoinsCtx,
    /// addressing the "replacement transaction" issue (same nonce for different transactions).
    pub static ref MM_CTX1: MmArc = MmCtxBuilder::new()
        .with_conf(json!({"use_trading_proto_v2": true}))
        .into_mm_arc();
}

// =============================================================================
// OnceLock contract addresses (initialized once in init_geth_node)
// =============================================================================

/// The account supplied with ETH on Geth dev node creation
static GETH_ACCOUNT: OnceLock<H160Eth> = OnceLock::new();
/// ERC20 token address on Geth dev node
static GETH_ERC20_CONTRACT: OnceLock<H160Eth> = OnceLock::new();
/// Swap contract address on Geth dev node
static GETH_SWAP_CONTRACT: OnceLock<H160Eth> = OnceLock::new();
/// Maker Swap V2 contract address on Geth dev node
static GETH_MAKER_SWAP_V2: OnceLock<H160Eth> = OnceLock::new();
/// Taker Swap V2 contract address on Geth dev node
static GETH_TAKER_SWAP_V2: OnceLock<H160Eth> = OnceLock::new();
/// Swap contract (with watchers support) address on Geth dev node
static GETH_WATCHERS_SWAP_CONTRACT: OnceLock<H160Eth> = OnceLock::new();
/// ERC721 token address on Geth dev node
static GETH_ERC721_CONTRACT: OnceLock<H160Eth> = OnceLock::new();
/// ERC1155 token address on Geth dev node
static GETH_ERC1155_CONTRACT: OnceLock<H160Eth> = OnceLock::new();
/// NFT Maker Swap V2 contract address on Geth dev node
static GETH_NFT_MAKER_SWAP_V2: OnceLock<H160Eth> = OnceLock::new();
/// USDT contract address on Geth dev node (non-standard ERC20 for SafeERC20 testing)
static GETH_USDT_CONTRACT: OnceLock<H160Eth> = OnceLock::new();

/// Geth RPC URL
pub static GETH_RPC_URL: &str = "http://127.0.0.1:8545";

// =============================================================================
// Docker image constants
// =============================================================================

/// Geth docker image
pub const GETH_DOCKER_IMAGE: &str = "docker.io/ethereum/client-go";
/// Geth docker image with tag
pub const GETH_DOCKER_IMAGE_WITH_TAG: &str = "docker.io/ethereum/client-go:stable";

// =============================================================================
// Contract bytecode constants
// =============================================================================

pub const ERC20_TOKEN_BYTES: &str = include_str!("../../../../mm2_test_helpers/contract_bytes/erc20_token_bytes");
pub const SWAP_CONTRACT_BYTES: &str = include_str!("../../../../mm2_test_helpers/contract_bytes/swap_contract_bytes");
pub const WATCHERS_SWAP_CONTRACT_BYTES: &str =
    include_str!("../../../../mm2_test_helpers/contract_bytes/watchers_swap_contract_bytes");
/// https://github.com/KomodoPlatform/etomic-swap/blob/7d4eafd4a408188a95aee78a41f0bf5f9116ffa2/contracts/Erc721Token.sol
pub const ERC721_TEST_TOKEN_BYTES: &str =
    include_str!("../../../../mm2_test_helpers/contract_bytes/erc721_test_token_bytes");
/// https://github.com/KomodoPlatform/etomic-swap/blob/7d4eafd4a408188a95aee78a41f0bf5f9116ffa2/contracts/Erc1155Token.sol
pub const ERC1155_TEST_TOKEN_BYTES: &str =
    include_str!("../../../../mm2_test_helpers/contract_bytes/erc1155_test_token_bytes");
/// https://github.com/KomodoPlatform/etomic-swap/blob/7d4eafd4a408188a95aee78a41f0bf5f9116ffa2/contracts/EtomicSwapMakerNftV2.sol
pub const NFT_MAKER_SWAP_V2_BYTES: &str =
    include_str!("../../../../mm2_test_helpers/contract_bytes/nft_maker_swap_v2_bytes");
/// https://github.com/KomodoPlatform/etomic-swap/blob/7d4eafd4a408188a95aee78a41f0bf5f9116ffa2/contracts/EtomicSwapMakerV2.sol
pub const MAKER_SWAP_V2_BYTES: &str = include_str!("../../../../mm2_test_helpers/contract_bytes/maker_swap_v2_bytes");
/// https://github.com/KomodoPlatform/etomic-swap/blob/7d4eafd4a408188a95aee78a41f0bf5f9116ffa2/contracts/EtomicSwapTakerV2.sol
pub const TAKER_SWAP_V2_BYTES: &str = include_str!("../../../../mm2_test_helpers/contract_bytes/taker_swap_v2_bytes");
/// Real USDT mainnet contract bytecode from Etherscan for SafeERC20 testing.
/// This is a non-standard ERC20 where transfer/transferFrom return void instead of bool.
pub const USDT_CONTRACT_BYTES: &str = include_str!("../../../../mm2_test_helpers/contract_bytes/usdt_contract_bytes");
/// USDT ABI from Etherscan - note the non-standard outputs:[] for transfer/transferFrom
pub const USDT_ABI: &str = include_str!("../../../../mm2_test_helpers/dummy_files/usdt_abi.json");

/// Geth dev chain ID used for testing
pub const GETH_DEV_CHAIN_ID: u64 = 1337;

// =============================================================================
// Address getters - safe OnceCell access
// =============================================================================

/// Get the Geth coinbase account address.
/// Panics if called before `init_geth_node()`.
pub fn geth_account() -> Address {
    *GETH_ACCOUNT
        .get()
        .expect("GETH_ACCOUNT not initialized - call init_geth_node() first")
}

/// Get the swap contract address.
/// Panics if called before `init_geth_node()`.
pub fn swap_contract() -> Address {
    *GETH_SWAP_CONTRACT
        .get()
        .expect("GETH_SWAP_CONTRACT not initialized - call init_geth_node() first")
}

/// Get the watchers swap contract address.
/// Panics if called before `init_geth_node()`.
pub fn watchers_swap_contract() -> Address {
    *GETH_WATCHERS_SWAP_CONTRACT
        .get()
        .expect("GETH_WATCHERS_SWAP_CONTRACT not initialized - call init_geth_node() first")
}

/// Get the ERC20 contract address.
/// Panics if called before `init_geth_node()`.
pub fn erc20_contract() -> Address {
    *GETH_ERC20_CONTRACT
        .get()
        .expect("GETH_ERC20_CONTRACT not initialized - call init_geth_node() first")
}

/// Get the Maker Swap V2 contract address.
/// Panics if called before `init_geth_node()`.
pub fn geth_maker_swap_v2() -> Address {
    *GETH_MAKER_SWAP_V2
        .get()
        .expect("GETH_MAKER_SWAP_V2 not initialized - call init_geth_node() first")
}

/// Get the Taker Swap V2 contract address.
/// Panics if called before `init_geth_node()`.
pub fn geth_taker_swap_v2() -> Address {
    *GETH_TAKER_SWAP_V2
        .get()
        .expect("GETH_TAKER_SWAP_V2 not initialized - call init_geth_node() first")
}

/// Get the ERC721 contract address.
/// Panics if called before `init_geth_node()`.
pub fn geth_erc721_contract() -> Address {
    *GETH_ERC721_CONTRACT
        .get()
        .expect("GETH_ERC721_CONTRACT not initialized - call init_geth_node() first")
}

/// Get the ERC1155 contract address.
/// Panics if called before `init_geth_node()`.
pub fn geth_erc1155_contract() -> Address {
    *GETH_ERC1155_CONTRACT
        .get()
        .expect("GETH_ERC1155_CONTRACT not initialized - call init_geth_node() first")
}

/// Get the NFT Maker Swap V2 contract address.
/// Panics if called before `init_geth_node()`.
pub fn geth_nft_maker_swap_v2() -> Address {
    *GETH_NFT_MAKER_SWAP_V2
        .get()
        .expect("GETH_NFT_MAKER_SWAP_V2 not initialized - call init_geth_node() first")
}

/// Get the USDT contract address.
/// Panics if called before `init_geth_node()`.
pub fn geth_usdt_contract() -> Address {
    *GETH_USDT_CONTRACT
        .get()
        .expect("GETH_USDT_CONTRACT not initialized - call init_geth_node() first")
}

/// Return ERC20 dev token contract address in checksum format
pub fn erc20_contract_checksum() -> String {
    checksum_address(&format!("{:02x}", erc20_contract()))
}

/// Return USDT contract address in checksum format
#[cfg(any(feature = "docker-tests-eth", feature = "docker-tests-watchers-eth"))]
pub fn usdt_contract_checksum() -> String {
    checksum_address(&format!("{:02x}", geth_usdt_contract()))
}

/// Return swap contract address in checksum format (with 0x prefix)
#[cfg(any(
    feature = "docker-tests-eth",
    feature = "docker-tests-tendermint",
    feature = "docker-tests-integration"
))]
pub fn swap_contract_checksum() -> String {
    checksum_address(&format!("{:02x}", swap_contract()))
}

/// Return watchers swap contract address in checksum format (with 0x prefix)
#[cfg(feature = "docker-tests-watchers")]
pub fn watchers_swap_contract_checksum() -> String {
    checksum_address(&format!("{:02x}", watchers_swap_contract()))
}

// =============================================================================
// Docker node helpers
// =============================================================================

/// Start a Geth docker node for testing.
pub fn geth_docker_node(ticker: &'static str, port: u16) -> DockerNode {
    let image = GenericImage::new(GETH_DOCKER_IMAGE, "stable");
    let args = vec!["--dev".into(), "--http".into(), "--http.addr=0.0.0.0".into()];
    let image = RunnableImage::from((image, args)).with_mapped_port((port, port));
    let container = image.start().expect("Failed to start Geth docker node");
    DockerNode {
        container,
        ticker: ticker.into(),
        port,
    }
}

/// Wait for the Geth node to be ready to accept connections.
///
/// Polls the node's block number endpoint until it responds successfully.
/// Used in compose mode where the node may still be starting up.
pub fn wait_for_geth_node_ready() {
    let mut attempts = 0;
    loop {
        if attempts >= 5 {
            panic!("Failed to connect to Geth node after several attempts.");
        }
        match block_on(GETH_WEB3.eth().block_number().timeout(Duration::from_secs(6))) {
            Ok(Ok(block_number)) => {
                log!("Geth node is ready, latest block number: {:?}", block_number);
                break;
            },
            Ok(Err(e)) => {
                log!("Failed to connect to Geth node: {:?}, retrying...", e);
            },
            Err(_) => {
                log!("Connection to Geth node timed out, retrying...");
            },
        }
        attempts += 1;
        thread::sleep(Duration::from_secs(1));
    }
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

/// Fill an address with USDT tokens from the Geth coinbase account.
/// Note: USDT's transfer() doesn't return a value, so we use the USDT ABI.
pub fn fill_usdt(to_addr: Address, amount: U256) {
    let _guard = GETH_NONCE_LOCK.lock().unwrap();
    let usdt = Contract::from_json(GETH_WEB3.eth(), geth_usdt_contract(), USDT_ABI.as_bytes()).unwrap();

    let tx_hash = block_on(usdt.call(
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
// Only used by docker-tests-eth and docker-tests-watchers-eth (not integration)
// =============================================================================

/// Creates ETH protocol coin supplied with 100 ETH
#[cfg(any(feature = "docker-tests-eth", feature = "docker-tests-watchers-eth"))]
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
        DerivationMethod::SingleAddress(addr) => addr.inner(),
        _ => panic!("Expected single address"),
    };

    // 100 ETH
    fill_eth(my_address, U256::from(10).pow(U256::from(20)));

    eth_coin
}

/// Creates ETH protocol coin supplied with 100 ETH, using the default GETH_RPC_URL
#[cfg(any(feature = "docker-tests-eth", feature = "docker-tests-watchers-eth"))]
pub fn eth_coin_with_random_privkey(swap_contract_address: Address) -> EthCoin {
    eth_coin_with_random_privkey_using_urls(swap_contract_address, &[GETH_RPC_URL])
}

/// Creates ERC20 protocol coin supplied with 1 ETH and 100 tokens
#[cfg(any(feature = "docker-tests-eth", feature = "docker-tests-watchers-eth"))]
pub fn erc20_coin_with_random_privkey(swap_contract_address: Address) -> EthCoin {
    let secret = random_secp256k1_secret();

    // Register platform ETH coin if not already registered by another parallel test, so platform_coin() lookups work.
    if block_on(lp_coinfind(&MM_CTX, "ETH")).ok().flatten().is_none() {
        let eth_conf = eth_dev_conf();
        let eth_req = json!({
            "method": "enable",
            "coin": "ETH",
            "swap_contract_address": swap_contract_address,
            "urls": [GETH_RPC_URL],
        });
        let platform_coin = block_on(eth_coin_from_conf_and_request(
            &MM_CTX,
            "ETH",
            &eth_conf,
            &eth_req,
            CoinProtocol::ETH {
                chain_id: GETH_DEV_CHAIN_ID,
            },
            PrivKeyBuildPolicy::IguanaPrivKey(secret),
        ))
        .unwrap();
        let coins_ctx = CoinsContext::from_ctx(&MM_CTX).unwrap();
        // Ignore error if another parallel test already registered the platform
        let _ = block_on(coins_ctx.add_platform_with_tokens(platform_coin.into(), vec![], None));
    }

    // Now create the ERC20 token
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
        PrivKeyBuildPolicy::IguanaPrivKey(secret),
    ))
    .unwrap();

    let my_address = match erc20_coin.derivation_method() {
        DerivationMethod::SingleAddress(addr) => addr.inner(),
        _ => panic!("Expected single address"),
    };

    // 1 ETH
    fill_eth(my_address, U256::from(10).pow(U256::from(18)));
    // 100 tokens (it has 8 decimals)
    fill_erc20(my_address, U256::from(10000000000u64));

    erc20_coin
}

/// Creates USDT protocol coin supplied with 1 ETH and 100 USDT.
/// Uses the real USDT contract (non-standard ERC20) for SafeERC20 testing.
#[cfg(any(feature = "docker-tests-eth", feature = "docker-tests-watchers-eth"))]
pub fn usdt_coin_with_random_privkey(swap_contract_address: Address) -> EthCoin {
    let secret = random_secp256k1_secret();

    // Register platform ETH coin if not already registered by another parallel test, so platform_coin() lookups work.
    if block_on(lp_coinfind(&MM_CTX, "ETH")).ok().flatten().is_none() {
        let eth_conf = eth_dev_conf();
        let eth_req = json!({
            "method": "enable",
            "coin": "ETH",
            "swap_contract_address": swap_contract_address,
            "urls": [GETH_RPC_URL],
        });
        let platform_coin = block_on(eth_coin_from_conf_and_request(
            &MM_CTX,
            "ETH",
            &eth_conf,
            &eth_req,
            CoinProtocol::ETH {
                chain_id: GETH_DEV_CHAIN_ID,
            },
            PrivKeyBuildPolicy::IguanaPrivKey(secret),
        ))
        .unwrap();
        let coins_ctx = CoinsContext::from_ctx(&MM_CTX).unwrap();
        // Ignore error if another parallel test already registered the platform
        let _ = block_on(coins_ctx.add_platform_with_tokens(platform_coin.into(), vec![], None));
    }

    // Now create the USDT token
    let usdt_conf = usdt_dev_conf(&usdt_contract_checksum());
    let req = json!({
        "method": "enable",
        "coin": "USDT",
        "swap_contract_address": swap_contract_address,
        "urls": [GETH_RPC_URL],
    });

    let usdt_coin = block_on(eth_coin_from_conf_and_request(
        &MM_CTX,
        "USDT",
        &usdt_conf,
        &req,
        CoinProtocol::ERC20 {
            platform: "ETH".to_string(),
            contract_address: usdt_contract_checksum(),
        },
        PrivKeyBuildPolicy::IguanaPrivKey(secret),
    ))
    .unwrap();

    let my_address = match usdt_coin.derivation_method() {
        DerivationMethod::SingleAddress(addr) => addr.inner(),
        _ => panic!("Expected single address"),
    };

    // 1 ETH for gas
    fill_eth(my_address, U256::from(10).pow(U256::from(18)));
    // 100 USDT (6 decimals)
    fill_usdt(my_address, U256::from(100_000_000u64));

    usdt_coin
}

/// Fills the private key's public address with ETH and ERC20 tokens
#[cfg(any(feature = "docker-tests-eth", feature = "docker-tests-integration"))]
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
    fill_eth(my_address.inner(), U256::from(10).pow(U256::from(20)));

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
    fill_erc20(my_address.inner(), U256::from(10000000000u64));
}

// =============================================================================
// Geth initialization
// =============================================================================

async fn get_current_gas_limit(web3: &Web3<Http>) {
    match web3.eth().block(BlockId::Number(BlockNumber::Latest)).await {
        Ok(Some(block)) => {
            log!("Current gas limit: {}", block.gas_limit);
        },
        Ok(None) => log!("Latest block information is not available."),
        Err(e) => log!("Failed to fetch the latest block: {}", e),
    }
}

/// Initialize the Geth node by deploying all test contracts.
///
/// This function deploys:
/// - ERC20 test token
/// - Swap contract
/// - Maker/Taker Swap V2 contracts
/// - Watchers swap contract
/// - NFT Maker Swap V2 contract
/// - ERC721 and ERC1155 test tokens
///
/// It also funds the Alice and Bob test accounts with ETH.
pub fn init_geth_node() {
    block_on(get_current_gas_limit(&GETH_WEB3));
    let gas_price = block_on(GETH_WEB3.eth().gas_price()).unwrap();
    log!("Current gas price: {:?}", gas_price);
    let accounts = block_on(GETH_WEB3.eth().accounts()).unwrap();
    let geth_account = accounts[0];
    GETH_ACCOUNT
        .set(geth_account)
        .expect("GETH_ACCOUNT already initialized");
    log!("GETH ACCOUNT {:?}", geth_account);

    let tx_request_deploy_erc20 = TransactionRequest {
        from: geth_account,
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

    let geth_erc20_contract = loop {
        let deploy_tx_receipt = match block_on(GETH_WEB3.eth().transaction_receipt(deploy_erc20_tx_hash)) {
            Ok(receipt) => receipt,
            Err(_) => {
                thread::sleep(Duration::from_millis(100));
                continue;
            },
        };

        if let Some(receipt) = deploy_tx_receipt {
            let addr = receipt.contract_address.unwrap();
            log!("GETH_ERC20_CONTRACT {:?}", addr);
            break addr;
        }
        thread::sleep(Duration::from_millis(100));
    };
    GETH_ERC20_CONTRACT
        .set(geth_erc20_contract)
        .expect("GETH_ERC20_CONTRACT already initialized");

    let tx_request_deploy_swap_contract = TransactionRequest {
        from: geth_account,
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

    let geth_swap_contract = loop {
        let deploy_swap_tx_receipt = match block_on(GETH_WEB3.eth().transaction_receipt(deploy_swap_tx_hash)) {
            Ok(receipt) => receipt,
            Err(_) => {
                thread::sleep(Duration::from_millis(100));
                continue;
            },
        };

        if let Some(receipt) = deploy_swap_tx_receipt {
            let addr = receipt.contract_address.unwrap();
            log!("GETH_SWAP_CONTRACT {:?}", addr);
            break addr;
        }
        thread::sleep(Duration::from_millis(100));
    };
    GETH_SWAP_CONTRACT
        .set(geth_swap_contract)
        .expect("GETH_SWAP_CONTRACT already initialized");

    let tx_request_deploy_maker_swap_contract_v2 = TransactionRequest {
        from: geth_account,
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

    let geth_maker_swap_v2 = loop {
        let deploy_maker_swap_v2_tx_receipt =
            match block_on(GETH_WEB3.eth().transaction_receipt(deploy_maker_swap_v2_tx_hash)) {
                Ok(receipt) => receipt,
                Err(_) => {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                },
            };

        if let Some(receipt) = deploy_maker_swap_v2_tx_receipt {
            let addr = receipt.contract_address.unwrap();
            log!(
                "GETH_MAKER_SWAP_V2 contract address: {:?}, receipt.status: {:?}",
                addr,
                receipt.status
            );
            break addr;
        }
        thread::sleep(Duration::from_millis(100));
    };
    GETH_MAKER_SWAP_V2
        .set(geth_maker_swap_v2)
        .expect("GETH_MAKER_SWAP_V2 already initialized");

    let dex_fee_addr = Token::Address(geth_account);
    let params = ethabi::encode(&[dex_fee_addr]);
    let taker_swap_v2_data = format!("{}{}", TAKER_SWAP_V2_BYTES, hex::encode(params));

    let tx_request_deploy_taker_swap_contract_v2 = TransactionRequest {
        from: geth_account,
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

    let geth_taker_swap_v2 = loop {
        let deploy_taker_swap_v2_tx_receipt =
            match block_on(GETH_WEB3.eth().transaction_receipt(deploy_taker_swap_v2_tx_hash)) {
                Ok(receipt) => receipt,
                Err(_) => {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                },
            };

        if let Some(receipt) = deploy_taker_swap_v2_tx_receipt {
            let addr = receipt.contract_address.unwrap();
            log!(
                "GETH_TAKER_SWAP_V2 contract address: {:?}, receipt.status: {:?}",
                addr,
                receipt.status
            );
            break addr;
        }
        thread::sleep(Duration::from_millis(100));
    };
    GETH_TAKER_SWAP_V2
        .set(geth_taker_swap_v2)
        .expect("GETH_TAKER_SWAP_V2 already initialized");

    let tx_request_deploy_watchers_swap_contract = TransactionRequest {
        from: geth_account,
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

    let geth_watchers_swap_contract = loop {
        let deploy_watchers_swap_tx_receipt =
            match block_on(GETH_WEB3.eth().transaction_receipt(deploy_watchers_swap_tx_hash)) {
                Ok(receipt) => receipt,
                Err(_) => {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                },
            };

        if let Some(receipt) = deploy_watchers_swap_tx_receipt {
            let addr = receipt.contract_address.unwrap();
            log!("GETH_WATCHERS_SWAP_CONTRACT {:?}", addr);
            break addr;
        }
        thread::sleep(Duration::from_millis(100));
    };
    GETH_WATCHERS_SWAP_CONTRACT
        .set(geth_watchers_swap_contract)
        .expect("GETH_WATCHERS_SWAP_CONTRACT already initialized");

    let tx_request_deploy_nft_maker_swap_v2_contract = TransactionRequest {
        from: geth_account,
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

    let geth_nft_maker_swap_v2 = loop {
        let deploy_nft_maker_swap_v2_tx_receipt =
            match block_on(GETH_WEB3.eth().transaction_receipt(deploy_nft_maker_swap_v2_tx_hash)) {
                Ok(receipt) => receipt,
                Err(_) => {
                    thread::sleep(Duration::from_millis(100));
                    continue;
                },
            };

        if let Some(receipt) = deploy_nft_maker_swap_v2_tx_receipt {
            let addr = receipt.contract_address.unwrap();
            log!(
                "GETH_NFT_MAKER_SWAP_V2 contact address: {:?}, receipt.status: {:?}",
                addr,
                receipt.status
            );
            break addr;
        }
        thread::sleep(Duration::from_millis(100));
    };
    GETH_NFT_MAKER_SWAP_V2
        .set(geth_nft_maker_swap_v2)
        .expect("GETH_NFT_MAKER_SWAP_V2 already initialized");

    let name = Token::String("MyNFT".into());
    let symbol = Token::String("MNFT".into());
    let params = ethabi::encode(&[name, symbol]);
    let erc721_data = format!("{}{}", ERC721_TEST_TOKEN_BYTES, hex::encode(params));

    let tx_request_deploy_erc721 = TransactionRequest {
        from: geth_account,
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

    let geth_erc721_contract = loop {
        let deploy_erc721_tx_receipt = match block_on(GETH_WEB3.eth().transaction_receipt(deploy_erc721_tx_hash)) {
            Ok(receipt) => receipt,
            Err(_) => {
                thread::sleep(Duration::from_millis(100));
                continue;
            },
        };

        if let Some(receipt) = deploy_erc721_tx_receipt {
            let addr = receipt.contract_address.unwrap();
            log!("GETH_ERC721_CONTRACT {:?}", addr);
            break addr;
        }
        thread::sleep(Duration::from_millis(100));
    };
    GETH_ERC721_CONTRACT
        .set(geth_erc721_contract)
        .expect("GETH_ERC721_CONTRACT already initialized");

    let uri = Token::String("MyNFTUri".into());
    let params = ethabi::encode(&[uri]);
    let erc1155_data = format!("{}{}", ERC1155_TEST_TOKEN_BYTES, hex::encode(params));

    let tx_request_deploy_erc1155 = TransactionRequest {
        from: geth_account,
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
    log!("Sent ERC1155 deploy transaction {:?}", deploy_erc1155_tx_hash);

    let geth_erc1155_contract = loop {
        let deploy_erc1155_tx_receipt = match block_on(GETH_WEB3.eth().transaction_receipt(deploy_erc1155_tx_hash)) {
            Ok(receipt) => receipt,
            Err(_) => {
                thread::sleep(Duration::from_millis(100));
                continue;
            },
        };

        if let Some(receipt) = deploy_erc1155_tx_receipt {
            let addr = receipt.contract_address.unwrap();
            log!("GETH_ERC1155_CONTRACT {:?}", addr);
            break addr;
        }
        thread::sleep(Duration::from_millis(100));
    };
    GETH_ERC1155_CONTRACT
        .set(geth_erc1155_contract)
        .expect("GETH_ERC1155_CONTRACT already initialized");

    // Deploy USDT contract (non-standard ERC20 for SafeERC20 testing).
    // Note: USDT bytecode already includes constructor args for initialSupply=100000 USDT,
    // name="Tether USD", symbol="USDT", decimals=6.
    let tx_request_deploy_usdt = TransactionRequest {
        from: geth_account,
        to: None,
        // Explicit gas limit for large contract deployment
        gas: Some(U256::from(8_000_000u64)),
        gas_price: None,
        value: None,
        data: Some(hex::decode(USDT_CONTRACT_BYTES).unwrap().into()),
        nonce: None,
        condition: None,
        transaction_type: None,
        access_list: None,
        max_fee_per_gas: None,
        max_priority_fee_per_gas: None,
    };
    let deploy_usdt_tx_hash = block_on(GETH_WEB3.eth().send_transaction(tx_request_deploy_usdt)).unwrap();
    log!("Sent USDT deploy transaction {:?}", deploy_usdt_tx_hash);

    let geth_usdt_contract = loop {
        let deploy_usdt_tx_receipt = match block_on(GETH_WEB3.eth().transaction_receipt(deploy_usdt_tx_hash)) {
            Ok(receipt) => receipt,
            Err(_) => {
                thread::sleep(Duration::from_millis(100));
                continue;
            },
        };

        if let Some(receipt) = deploy_usdt_tx_receipt {
            let addr = receipt.contract_address.unwrap();
            log!("GETH_USDT_CONTRACT {:?}", addr);
            break addr;
        }
        thread::sleep(Duration::from_millis(100));
    };
    GETH_USDT_CONTRACT
        .set(geth_usdt_contract)
        .expect("GETH_USDT_CONTRACT already initialized");

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
