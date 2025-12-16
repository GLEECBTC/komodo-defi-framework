//! BCH/SLP helpers for docker tests.
//!
//! This module was extracted from `helpers::utxo`.
//! It provides:
//! - `BchDockerOps` wrapper for the FORSLP node (BCH-like UTXO chain with SLP enabled)
//! - `initialize_slp()` to mint/distribute test SLP tokens
//! - Accessors to retrieve a prefilled SLP private key and the token id

use super::docker_ops::CoinDockerOps;
use super::utxo::fill_address;
use coins::utxo::bch::{bch_coin_with_priv_key, BchActivationRequest, BchCoin};
use coins::utxo::rpc_clients::UtxoRpcClientEnum;
use coins::utxo::slp::{slp_genesis_output, SlpOutput, SlpToken};
use coins::utxo::utxo_common::send_outputs_from_my_address;
use coins::utxo::UtxoCommonOps;
use coins::Transaction;
use coins::{ConfirmPaymentInput, MarketCoinOps};
use common::{block_on, block_on_f01, wait_until_sec};
use crypto::Secp256k1Secret;
use keys::{AddressBuilder, KeyPair, NetworkPrefix as CashAddrPrefix};
use mm2_core::mm_ctx::MmCtxBuilder;
use primitives::hash::H256;
use script::Builder;
use std::convert::TryFrom;
use std::sync::Mutex;

use chain::TransactionOutput;
use mm2_core::mm_ctx::MmArc;

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
