//! BCH/SLP helpers for docker tests.
//!
//! This module was extracted from `helpers::utxo`.
//! It provides:
//! - `BchDockerOps` wrapper for the FORSLP node (BCH-like UTXO chain with SLP enabled)
//! - `forslp_docker_node()` to start the FORSLP docker container
//! - `initialize_slp()` to mint/distribute test SLP tokens
//! - Accessors to retrieve a prefilled SLP private key and the token id

use super::docker_ops::CoinDockerOps;
use super::env::DockerNode;
use chain::TransactionOutput;
use coins::utxo::bch::{bch_coin_with_priv_key, BchActivationRequest, BchCoin};
use coins::utxo::rpc_clients::{UtxoRpcClientEnum, UtxoRpcClientOps};
use coins::utxo::slp::{slp_genesis_output, SlpOutput, SlpToken};
use coins::utxo::utxo_common::send_outputs_from_my_address;
use coins::utxo::{coin_daemon_data_dir, zcash_params_path, UtxoCoinFields, UtxoCommonOps};
use coins::Transaction;
use coins::{ConfirmPaymentInput, MarketCoinOps};
use common::executor::Timer;
use common::Future01CompatExt;
use common::{block_on, block_on_f01, now_ms, now_sec, wait_until_ms, wait_until_sec};
use crypto::Secp256k1Secret;
use keys::{AddressBuilder, KeyPair, NetworkPrefix as CashAddrPrefix};
use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};
use mm2_number::BigDecimal;
use primitives::hash::H256;
use script::Builder;
use serde_json::json;
use std::convert::TryFrom;
use std::process::Command;
use std::sync::Mutex;
use testcontainers::core::Mount;
use testcontainers::runners::SyncRunner;
use testcontainers::GenericImage;
use testcontainers::{core::WaitFor, RunnableImage};
use tokio::sync::Mutex as AsyncMutex;

// =============================================================================
// SLP token metadata
// =============================================================================

lazy_static! {
    /// SLP token ID (genesis tx hash).
    pub static ref SLP_TOKEN_ID: Mutex<H256> = Mutex::new(H256::default());

    /// Private keys supplied with 1000 SLP tokens on tests initialization.
    ///
    /// Due to the SLP protocol limitations only 19 outputs (18 + change) can be sent in one transaction.
    pub static ref SLP_TOKEN_OWNERS: Mutex<Vec<[u8; 32]>> = Mutex::new(Vec::with_capacity(18));

    /// Lock for FORSLP funding operations.
    static ref FORSLP_LOCK: AsyncMutex<()> = AsyncMutex::new(());
}

// =============================================================================
// Docker image constants
// =============================================================================

/// FORSLP docker image (same as UTXO testblockchain).
const FORSLP_DOCKER_IMAGE: &str = "docker.io/gleec/testblockchain";

/// FORSLP docker image with tag (used by runner::required_images).
pub const FORSLP_IMAGE_WITH_TAG: &str = "docker.io/gleec/testblockchain:multiarch";

// =============================================================================
// Docker node helpers
// =============================================================================

/// Start the FORSLP dockerized BCH/SLP node.
pub fn forslp_docker_node(port: u16) -> DockerNode {
    let ticker = "FORSLP";
    let image = GenericImage::new(FORSLP_DOCKER_IMAGE, "multiarch")
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
    let container = image.start().expect("Failed to start FORSLP docker node");

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
        assert!(now_ms() < timeout, "Test timed out waiting for config");
    }

    DockerNode {
        container,
        ticker: ticker.into(),
        port,
    }
}

// =============================================================================
// BCH/SLP funding utilities
// =============================================================================

/// Fill a BCH/SLP address with the specified amount.
fn fill_bch_address<T>(coin: &T, address: &str, amount: BigDecimal, timeout: u64)
where
    T: MarketCoinOps + AsRef<UtxoCoinFields>,
{
    block_on(fill_bch_address_async(coin, address, amount, timeout));
}

/// Fill a BCH/SLP address with the specified amount (async version).
async fn fill_bch_address_async<T>(coin: &T, address: &str, amount: BigDecimal, timeout: u64)
where
    T: MarketCoinOps + AsRef<UtxoCoinFields>,
{
    let _lock = FORSLP_LOCK.lock().await;
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

// =============================================================================
// BCH/SLP docker ops
// =============================================================================

/// Docker operations for BCH/SLP coins (FORSLP).
pub struct BchDockerOps {
    #[allow(dead_code)]
    ctx: MmArc,
    coin: BchCoin,
}

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

    /// Initialize SLP tokens:
    /// - Fund node wallet
    /// - Create SLP genesis
    /// - Distribute tokens to 18 new addresses
    /// - Store their privkeys into `SLP_TOKEN_OWNERS` and token id into `SLP_TOKEN_ID`
    pub fn initialize_slp(&self) {
        fill_bch_address(&self.coin, &self.coin.my_address().unwrap(), 100000.into(), 30);
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

// =============================================================================
// Public accessors used by tests
// =============================================================================

/// Get a prefilled SLP privkey from the pool.
///
/// Panics if initialization didn't happen (runner must call `setup_slp()`).
pub fn get_prefilled_slp_privkey() -> [u8; 32] {
    SLP_TOKEN_OWNERS.lock().unwrap().remove(0)
}

/// Get the SLP token ID as hex string.
pub fn get_slp_token_id() -> String {
    hex::encode(SLP_TOKEN_ID.lock().unwrap().as_slice())
}
