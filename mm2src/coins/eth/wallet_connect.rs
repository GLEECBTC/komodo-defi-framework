/// https://docs.reown.com/advanced/multichain/rpc-reference/ethereum-rpc
use super::{ChainSpec, EthCoin, EthPrivKeyPolicy};

use crate::common::Future01CompatExt;
use crate::hd_wallet::AddrToString;
use crate::Eip1559Ops;
use crate::{BytesJson, MarketCoinOps, TransactionErr};

use common::log::info;
use common::u256_to_hex;
use derive_more::Display;
use enum_derives::EnumFromStringify;
use ethcore_transaction::{Action, SignedTransaction};
use ethereum_types::H256;
use ethereum_types::{Address, Public, H160, H520, U256};
use ethkey::{public_to_address, Message, Signature};
use kdf_walletconnect::chain::{WcChainId, WcRequestMethods};
use kdf_walletconnect::error::WalletConnectError;
use kdf_walletconnect::{WalletConnectCtx, WalletConnectOps, WcTopic};
use mm2_err_handle::prelude::*;
use secp256k1::recovery::{RecoverableSignature, RecoveryId};
use secp256k1::{PublicKey, Secp256k1};
use std::iter::FromIterator;
use std::str::FromStr;
use web3::signing::hash_message;

// Wait for 60 seconds for the transaction to appear on the RPC node.
const WAIT_RPC_TIMEOUT_SECS: u64 = 60;

#[derive(Display, Debug, EnumFromStringify)]
pub enum EthWalletConnectError {
    UnsupportedChainId(WcChainId),
    InvalidSignature(String),
    AccountMisMatch(String),
    #[from_stringify("rlp::DecoderError", "hex::FromHexError")]
    TxDecodingFailed(String),
    #[from_stringify("ethkey::Error")]
    InternalError(String),
    InvalidTxData(String),
    SessionError(String),
    WalletConnectError(WalletConnectError),
}

impl From<WalletConnectError> for EthWalletConnectError {
    fn from(value: WalletConnectError) -> Self {
        Self::WalletConnectError(value)
    }
}

/// Eth Params required for constructing WalletConnect transaction.
pub struct WcEthTxParams<'a> {
    pub(crate) gas: U256,
    pub(crate) nonce: U256,
    pub(crate) data: &'a [u8],
    pub(crate) my_address: H160,
    pub(crate) action: Action,
    pub(crate) value: U256,
    pub(crate) gas_price: Option<U256>,
    pub(crate) chain_id: u64,
    pub(crate) max_fee_per_gas: Option<U256>,
    pub(crate) max_priority_fee_per_gas: Option<U256>,
}

impl<'a> WcEthTxParams<'a> {
    /// Construct WalletConnect transaction json from from `WcEthTxParams`
    fn prepare_wc_tx_format(&self) -> MmResult<serde_json::Value, EthWalletConnectError> {
        let mut tx_object = serde_json::Map::from_iter([
            ("chainId".to_string(), json!(self.chain_id)),
            ("nonce".to_string(), json!(u256_to_hex(self.nonce))),
            ("from".to_string(), json!(self.my_address.addr_to_string())),
            ("gasLimit".to_string(), json!(u256_to_hex(self.gas))),
            ("value".to_string(), json!(u256_to_hex(self.value))),
            ("data".to_string(), json!(format!("0x{}", hex::encode(self.data)))),
        ]);

        if let Some(gas_price) = self.gas_price {
            tx_object.insert("gasPrice".to_string(), json!(u256_to_hex(gas_price)));
        }

        if let Some(max_fee_per_gas) = self.max_fee_per_gas {
            tx_object.insert("maxFeePerGas".to_string(), json!(u256_to_hex(max_fee_per_gas)));
        }

        if let Some(max_priority_fee_per_gas) = self.max_priority_fee_per_gas {
            tx_object.insert(
                "maxPriorityFeePerGas".to_string(),
                json!(u256_to_hex(max_priority_fee_per_gas)),
            );
        }

        if let Action::Call(addr) = self.action {
            tx_object.insert("to".to_string(), json!(format!("0x{}", hex::encode(addr.as_bytes()))));
        }

        Ok(json!(vec![serde_json::Value::Object(tx_object)]))
    }
}

#[async_trait::async_trait]
impl WalletConnectOps for EthCoin {
    type Error = MmError<EthWalletConnectError>;
    type Params<'a> = WcEthTxParams<'a>;
    type SignTxData = (SignedTransaction, BytesJson);
    type SendTxData = (SignedTransaction, BytesJson);

    async fn wc_chain_id(&self, wc: &WalletConnectCtx) -> Result<WcChainId, Self::Error> {
        let session_topic = self.session_topic()?;
        let chain_id = match self.chain_spec {
            ChainSpec::Evm { chain_id } => chain_id,
            // Todo: Add Tron signing logic
            ChainSpec::Tron { .. } => {
                return Err(MmError::new(EthWalletConnectError::InternalError(
                    "Tron is not supported for this action yet".into(),
                )))
            },
        };
        let chain_id = WcChainId::new_eip155(chain_id.to_string());
        wc.validate_update_active_chain_id(session_topic, &chain_id)
            .await
            .map_mm_err()?;

        Ok(chain_id)
    }

    async fn wc_sign_tx<'a>(
        &self,
        wc: &WalletConnectCtx,
        params: Self::Params<'a>,
    ) -> Result<Self::SignTxData, Self::Error> {
        let bytes = {
            let chain_id = self.wc_chain_id(wc).await?;
            let tx_json = params.prepare_wc_tx_format()?;
            let session_topic = self.session_topic()?;
            let tx_hex: String = wc
                .send_session_request_and_wait(session_topic, &chain_id, WcRequestMethods::EthSignTransaction, tx_json)
                .await
                .map_mm_err()?;
            // if tx_hex.len() < 4 {
            //     return MmError::err(EthWalletConnectError::TxDecodingFailed(
            //         "invalid transaction hex returned from wallet".to_string(),
            //     ));
            // }
            // // First 4 bytes from WalletConnect represents Protoc info
            let normalized_tx_hex = tx_hex.strip_prefix("0x").unwrap_or(&tx_hex);
            hex::decode(normalized_tx_hex)?
        };

        let unverified = rlp::decode(&bytes)?;
        let signed = SignedTransaction::new(unverified)?;
        let bytes = rlp::encode(&signed);

        Ok((signed, BytesJson::from(bytes.to_vec())))
    }

    async fn wc_send_tx<'a>(
        &self,
        wc: &WalletConnectCtx,
        params: Self::Params<'a>,
    ) -> Result<Self::SignTxData, Self::Error> {
        let tx_hash: String = {
            let chain_id = self.wc_chain_id(wc).await?;
            let tx_json = params.prepare_wc_tx_format()?;
            let session_topic = self.session_topic()?;
            wc.send_session_request_and_wait(session_topic, &chain_id, WcRequestMethods::EthSendTransaction, tx_json)
                .await
                .map_mm_err()?
        };

        let tx_hash = tx_hash.strip_prefix("0x").unwrap_or(&tx_hash);
        let maybe_signed_tx = {
            self.wait_for_tx_appears_on_rpc(H256::from_slice(&hex::decode(tx_hash)?), WAIT_RPC_TIMEOUT_SECS, 1.)
                .await
                .mm_err(|err| EthWalletConnectError::InternalError(err.to_string()))?
        };
        let signed_tx = maybe_signed_tx.ok_or(MmError::new(EthWalletConnectError::InternalError(format!(
            "Waited too long until the transaction {tx_hash:?} appear on the RPC node"
        ))))?;
        let tx_hex = BytesJson::from(rlp::encode(&signed_tx).to_vec());

        Ok((signed_tx, tx_hex))
    }

    fn session_topic(&self) -> Result<&WcTopic, Self::Error> {
        if let EthPrivKeyPolicy::WalletConnect { ref session_topic, .. } = &self.priv_key_policy {
            return Ok(session_topic);
        }

        MmError::err(EthWalletConnectError::SessionError(format!(
            "{} is not activated via WalletConnect",
            self.ticker()
        )))
    }
}

pub async fn eth_request_wc_personal_sign(
    wc: &WalletConnectCtx,
    session_topic: &WcTopic,
    chain_id: u64,
) -> MmResult<(H520, Address), EthWalletConnectError> {
    let chain_id = WcChainId::new_eip155(chain_id.to_string());
    wc.validate_update_active_chain_id(session_topic, &chain_id)
        .await
        .map_mm_err()?;

    let (account_str, _) = wc
        .get_account_and_properties_for_chain_id(session_topic, &chain_id)
        .map_mm_err()?;
    let message = "Authenticate with KDF";
    let params = {
        let message_hex = format!("0x{}", hex::encode(message));
        json!(&[&message_hex, &account_str])
    };
    let data = wc
        .send_session_request_and_wait::<String>(session_topic, &chain_id, WcRequestMethods::EthPersonalSign, params)
        .await
        .map_mm_err()?;

    extract_pubkey_from_signature(&data, message, &account_str)
        .mm_err(|err| WalletConnectError::SessionError(err.to_string()))
        .map_mm_err()
}

fn extract_pubkey_from_signature(
    signature_str: &str,
    message: &str,
    account: &str,
) -> MmResult<(H520, Address), EthWalletConnectError> {
    let account =
        H160::from_str(&account[2..]).map_to_mm(|err| EthWalletConnectError::InternalError(err.to_string()))?;
    let uncompressed: H520 = {
        let message_hash = hash_message(message);
        let signature = Signature::from_str(&signature_str[2..])
            .map_to_mm(|err| EthWalletConnectError::InvalidSignature(err.to_string()))?;
        let pubkey = recover(&signature, &message_hash).map_to_mm(|err| {
            EthWalletConnectError::InvalidSignature(format!(
                "Couldn't recover public key from the signature: '{signature:?}, error: {err:?}'"
            ))
        })?;
        pubkey.serialize_uncompressed().into()
    };

    let mut public = Public::default();
    public.as_mut().copy_from_slice(&uncompressed[1..65]);

    let recovered_address = public_to_address(&public);
    if account != recovered_address {
        return MmError::err(EthWalletConnectError::AccountMisMatch(format!(
            "Recovered address '{recovered_address:?}' should be the same as '{account:?}'"
        )));
    }

    Ok((uncompressed, recovered_address))
}

pub(crate) fn recover(signature: &Signature, message: &Message) -> Result<PublicKey, ethkey::Error> {
    let recovery_id = {
        let recovery_id = signature[64].checked_sub(27).ok_or(ethkey::Error::InvalidSignature)?;
        RecoveryId::from_i32(recovery_id as i32)?
    };
    let sig = RecoverableSignature::from_compact(&signature[0..64], recovery_id)?;
    let pubkey = Secp256k1::new().recover(&secp256k1::Message::from_slice(&message[..])?, &sig)?;

    Ok(pubkey)
}

/// Sign and send eth transaction with WalletConnect,
/// This fn is primarily for swap transactions so it uses swap tx fee policy
pub(crate) async fn send_transaction_with_walletconnect(
    coin: EthCoin,
    wc: &WalletConnectCtx,
    my_address: Address,
    value: U256,
    action: Action,
    data: &[u8],
    gas: U256,
) -> Result<SignedTransaction, TransactionErr> {
    info!("target: WalletConnect: sign-and-send, get_gas_price…");
    // Todo: Tron will have to use ETH protocol for walletconnect, it will be a different coin than the native one in coins config.
    let chain_id = coin
        .chain_spec
        .chain_id()
        .ok_or(TransactionErr::Plain("Tron is not supported for this action!".into()))?;
    let pay_for_gas_policy = try_tx_s!(coin.get_swap_gas_fee_policy().await);
    let pay_for_gas_option = try_tx_s!(coin.get_swap_pay_for_gas_option(pay_for_gas_policy).await);
    let (max_fee_per_gas, max_priority_fee_per_gas) = pay_for_gas_option.get_fee_per_gas();
    let (nonce, _) = try_tx_s!(coin.clone().get_addr_nonce(my_address).compat().await);

    let params = WcEthTxParams {
        gas,
        nonce,
        data,
        my_address,
        action,
        value,
        gas_price: pay_for_gas_option.get_gas_price(),
        chain_id,
        max_fee_per_gas,
        max_priority_fee_per_gas,
    };
    // Please note that this method may take a long time
    // due to `eth_sendTransaction` requests.
    info!("target: WalletConnect: sign-and-send, signing and sending tx");
    let (signed_tx, _) = try_tx_s!(coin.wc_send_tx(wc, params).await);

    Ok(signed_tx)
}
