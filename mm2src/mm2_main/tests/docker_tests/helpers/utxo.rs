//! UTXO coin helpers for docker tests.
//!
//! This module provides:
//! - UTXO asset docker node helpers (MYCOIN, MYCOIN1)
//! - BCH/SLP docker node helpers (FORSLP)
//! - Coin creation and funding utilities

// =============================================================================
// Common imports (used by multiple feature sets)
// =============================================================================

use crate::docker_tests::helpers::docker_ops::CoinDockerOps;
use crate::docker_tests::helpers::env::DockerNode;
use coins::utxo::rpc_clients::{UtxoRpcClientEnum, UtxoRpcClientOps};
use coins::utxo::{coin_daemon_data_dir, zcash_params_path, UtxoCoinFields};
use coins::{ConfirmPaymentInput, MarketCoinOps};
use common::executor::Timer;
use common::Future01CompatExt;
use common::{block_on, now_ms, now_sec, wait_until_ms, wait_until_sec};
use crypto::Secp256k1Secret;
use mm2_number::BigDecimal;
use std::process::Command;
use testcontainers::core::Mount;
use testcontainers::runners::SyncRunner;
use testcontainers::GenericImage;
use testcontainers::{core::WaitFor, RunnableImage};
use tokio::sync::Mutex as AsyncMutex;

// UtxoStandardCoin imports - only needed by features that create UTXO coins
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia",
    feature = "docker-tests-integration"
))]
use coins::utxo::utxo_standard::{utxo_standard_coin_with_priv_key, UtxoStandardCoin};
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia",
    feature = "docker-tests-integration"
))]
use coins::utxo::UtxoActivationParams;
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia",
    feature = "docker-tests-slp",
    feature = "docker-tests-integration"
))]
use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};

// UtxoCommonOps - needed for my_public_key() in SLP initialization
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
use coins::utxo::UtxoCommonOps;

// Transaction trait - needed for tx_hex() in SLP initialization
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
use coins::Transaction;

// SLP-specific imports
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
use chain::TransactionOutput;
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
use coins::utxo::bch::{bch_coin_with_priv_key, BchActivationRequest, BchCoin};
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
use coins::utxo::slp::{slp_genesis_output, SlpOutput, SlpToken};
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
use coins::utxo::utxo_common::send_outputs_from_my_address;
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
use common::block_on_f01;
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
use keys::{AddressBuilder, KeyPair, NetworkPrefix as CashAddrPrefix};
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
use primitives::hash::H256;
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
use script::Builder;
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
use std::convert::TryFrom;
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
use std::sync::Mutex;

// rmd160_from_priv imports
#[cfg(any(feature = "docker-tests-ordermatch", feature = "docker-tests-swaps-utxo"))]
use bitcrypto::dhash160;
#[cfg(any(feature = "docker-tests-ordermatch", feature = "docker-tests-swaps-utxo"))]
use primitives::hash::H160;

// random_secp256k1_secret import - only for features that use generate_utxo_coin_with_random_privkey
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers"
))]
use crate::docker_tests::helpers::env::random_secp256k1_secret;

// =============================================================================
// Funding Locks
// =============================================================================

lazy_static! {
    /// Lock for MYCOIN funding operations
    pub static ref MYCOIN_LOCK: AsyncMutex<()> = AsyncMutex::new(());

    /// Lock for MYCOIN1 funding operations
    pub static ref MYCOIN1_LOCK: AsyncMutex<()> = AsyncMutex::new(());

    /// Lock for FORSLP (BCH/SLP) funding operations
    pub static ref FORSLP_LOCK: AsyncMutex<()> = AsyncMutex::new(());

    /// Lock for Qtum/QRC20 funding operations.
    /// Shared by QTUM, QICK, and QORTY coins since they all run on the same Qtum node.
    pub static ref QTUM_LOCK: AsyncMutex<()> = AsyncMutex::new(());
}

/// Get the appropriate funding lock for a given ticker.
fn get_funding_lock(ticker: &str) -> &'static AsyncMutex<()> {
    match ticker {
        "MYCOIN" => &MYCOIN_LOCK,
        "MYCOIN1" => &MYCOIN1_LOCK,
        "FORSLP" => &FORSLP_LOCK,
        "QTUM" | "QICK" | "QORTY" => &QTUM_LOCK,
        _ => panic!("No funding lock defined for ticker: {}", ticker),
    }
}

// =============================================================================
// SLP token metadata (SLP-only)
// =============================================================================

#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
lazy_static! {
    /// SLP token ID (genesis tx hash)
    pub static ref SLP_TOKEN_ID: Mutex<H256> = Mutex::new(H256::default());
    /// Private keys supplied with 1000 SLP tokens on tests initialization.
    /// Due to the SLP protocol limitations only 19 outputs (18 + change) can be sent in one transaction.
    pub static ref SLP_TOKEN_OWNERS: Mutex<Vec<[u8; 32]>> = Mutex::new(Vec::with_capacity(18));
}

// =============================================================================
// Docker image constants
// =============================================================================

/// UTXO asset docker image
pub const UTXO_ASSET_DOCKER_IMAGE: &str = "docker.io/artempikulin/testblockchain";
/// UTXO asset docker image with tag
pub const UTXO_ASSET_DOCKER_IMAGE_WITH_TAG: &str = "docker.io/artempikulin/testblockchain:multiarch";

// =============================================================================
// Ticker constants (UTXO asset features only)
// =============================================================================

/// Ticker of MYCOIN dockerized blockchain.
#[cfg(feature = "docker-tests-swaps-utxo")]
pub const MYCOIN: &str = "MYCOIN";

/// Ticker of MYCOIN1 dockerized blockchain.
#[cfg(feature = "docker-tests-swaps-utxo")]
pub const MYCOIN1: &str = "MYCOIN1";

// =============================================================================
// UtxoAssetDockerOps (UTXO asset features only)
// =============================================================================

/// Docker operations for standard UTXO assets (MYCOIN, MYCOIN1).
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia",
    feature = "docker-tests-integration"
))]
pub struct UtxoAssetDockerOps {
    #[allow(dead_code)]
    ctx: MmArc,
    coin: UtxoStandardCoin,
}

#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia",
    feature = "docker-tests-integration"
))]
impl CoinDockerOps for UtxoAssetDockerOps {
    fn rpc_client(&self) -> &UtxoRpcClientEnum {
        &self.coin.as_ref().rpc_client
    }
}

#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-sia",
    feature = "docker-tests-integration"
))]
impl UtxoAssetDockerOps {
    /// Create UtxoAssetDockerOps from ticker.
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

// =============================================================================
// BchDockerOps (SLP features only)
// =============================================================================

/// Docker operations for BCH/SLP coins (FORSLP).
#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
pub struct BchDockerOps {
    #[allow(dead_code)]
    ctx: MmArc,
    coin: BchCoin,
}

#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
impl BchDockerOps {
    /// Create BchDockerOps from ticker.
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

    /// Initialize SLP tokens.
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

#[cfg(any(feature = "docker-tests-slp", feature = "docker-tests-integration"))]
impl CoinDockerOps for BchDockerOps {
    fn rpc_client(&self) -> &UtxoRpcClientEnum {
        &self.coin.as_ref().rpc_client
    }
}

// =============================================================================
// Docker node helpers
// =============================================================================

/// Start a UTXO asset docker node.
pub fn utxo_asset_docker_node(ticker: &'static str, port: u16) -> DockerNode {
    let image = GenericImage::new(UTXO_ASSET_DOCKER_IMAGE, "multiarch")
        .with_mount(Mount::bind_mount(
            zcash_params_path().display().to_string(),
            "/root/.zcash-params",
        ))
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
    let container = image.start().expect("Failed to start UTXO asset docker node");
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

// =============================================================================
// Coin creation and funding utilities
// =============================================================================

/// Compute RIPEMD160(SHA256(pubkey)) from a private key.
#[cfg(any(feature = "docker-tests-ordermatch", feature = "docker-tests-swaps-utxo"))]
pub fn rmd160_from_priv(privkey: Secp256k1Secret) -> H160 {
    use secp256k1::{PublicKey, Secp256k1, SecretKey};
    let secret = SecretKey::from_slice(privkey.as_slice()).unwrap();
    let public = PublicKey::from_secret_key(&Secp256k1::new(), &secret);
    dhash160(&public.serialize())
}

/// Get a prefilled SLP privkey from the pool.
#[cfg(feature = "docker-tests-slp")]
pub fn get_prefilled_slp_privkey() -> [u8; 32] {
    SLP_TOKEN_OWNERS.lock().unwrap().remove(0)
}

/// Get the SLP token ID as hex string.
#[cfg(feature = "docker-tests-slp")]
pub fn get_slp_token_id() -> String {
    hex::encode(SLP_TOKEN_ID.lock().unwrap().as_slice())
}

/// Import an address to the coin's wallet.
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-integration"
))]
pub async fn import_address<T>(coin: &T)
where
    T: MarketCoinOps + AsRef<UtxoCoinFields>,
{
    let mutex = get_funding_lock(coin.ticker());
    let _lock = mutex.lock().await;

    match coin.as_ref().rpc_client {
        UtxoRpcClientEnum::Native(ref native) => {
            let my_address = coin.my_address().unwrap();
            native
                .import_address(&my_address, &my_address, false)
                .compat()
                .await
                .unwrap();
        },
        UtxoRpcClientEnum::Electrum(_) => panic!("Expected NativeClient"),
    }
}

/// Build asset `UtxoStandardCoin` from ticker and privkey without filling the balance.
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-integration"
))]
pub fn utxo_coin_from_privkey(ticker: &str, priv_key: Secp256k1Secret) -> (MmArc, UtxoStandardCoin) {
    let ctx = MmCtxBuilder::new().into_mm_arc();
    let conf = json!({"coin":ticker,"asset":ticker,"txversion":4,"overwintered":1,"txfee":1000,"network":"regtest"});
    let req = json!({"method":"enable"});
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let coin = block_on(utxo_standard_coin_with_priv_key(&ctx, ticker, &conf, &params, priv_key)).unwrap();
    block_on(import_address(&coin));
    (ctx, coin)
}

/// Create a UTXO coin for the given privkey and fill its address with the specified balance.
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-integration"
))]
pub fn generate_utxo_coin_with_privkey(ticker: &str, balance: BigDecimal, priv_key: Secp256k1Secret) {
    let (_, coin) = utxo_coin_from_privkey(ticker, priv_key);
    let timeout = 30; // timeout if test takes more than 30 seconds to run
    let my_address = coin.my_address().expect("!my_address");
    fill_address(&coin, &my_address, balance, timeout);
}

/// Fund a UTXO address with the specified balance (async version).
/// Only used by Sia tests which need async funding.
#[cfg(feature = "docker-tests-sia")]
pub async fn fund_privkey_utxo(ticker: &str, balance: BigDecimal, priv_key: &Secp256k1Secret) {
    let ctx = MmCtxBuilder::new().into_mm_arc();
    let conf = json!({"coin":ticker,"asset":ticker,"txversion":4,"overwintered":1,"txfee":1000,"network":"regtest"});
    let req = json!({"method":"enable"});
    let params = UtxoActivationParams::from_legacy_req(&req).unwrap();
    let coin = utxo_standard_coin_with_priv_key(&ctx, ticker, &conf, &params, *priv_key)
        .await
        .unwrap();
    let my_address = coin.my_address().expect("!my_address");
    fill_address_async(&coin, &my_address, balance, 30).await;
}

/// Generate random privkey, create a UTXO coin and fill its address with the specified balance.
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers"
))]
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

/// Fill address with the specified amount (synchronous wrapper).
#[cfg(any(
    feature = "docker-tests-swaps-utxo",
    feature = "docker-tests-ordermatch",
    feature = "docker-tests-watchers",
    feature = "docker-tests-qrc20",
    feature = "docker-tests-slp",
    feature = "docker-tests-integration"
))]
pub fn fill_address<T>(coin: &T, address: &str, amount: BigDecimal, timeout: u64)
where
    T: MarketCoinOps + AsRef<UtxoCoinFields>,
{
    block_on(fill_address_async(coin, address, amount, timeout));
}

/// Fill address with the specified amount (async version).
pub async fn fill_address_async<T>(coin: &T, address: &str, amount: BigDecimal, timeout: u64)
where
    T: MarketCoinOps + AsRef<UtxoCoinFields>,
{
    // prevent concurrent fill since daemon RPC returns errors if send_to_address
    // is called concurrently (insufficient funds) and it also may return other errors
    // if previous transaction is not confirmed yet
    let mutex = get_funding_lock(coin.ticker());
    let _lock = mutex.lock().await;
    let timeout = wait_until_sec(timeout);

    if let UtxoRpcClientEnum::Native(client) = &coin.as_ref().rpc_client {
        client.import_address(address, address, false).compat().await.unwrap();
        let hash = client.send_to_address(address, &amount).compat().await.unwrap();
        let tx_bytes = client.get_transaction_bytes(&hash).compat().await.unwrap();
        let confirm_payment_input = ConfirmPaymentInput {
            payment_tx: tx_bytes.clone().0,
            confirmations: 1,
            requires_nota: false,
            wait_until: timeout,
            check_every: 1,
        };
        coin.wait_for_confirmations(confirm_payment_input)
            .compat()
            .await
            .unwrap();
        log!("{:02x}", tx_bytes);
        loop {
            let unspents = client
                .list_unspent_impl(0, i32::MAX, vec![address.to_string()])
                .compat()
                .await
                .unwrap();
            if !unspents.is_empty() {
                break;
            }
            assert!(now_sec() < timeout, "Test timed out");
            Timer::sleep(1.0).await;
        }
    };
}
