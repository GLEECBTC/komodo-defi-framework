use crate::coin_errors::{AddressFromPubkeyError, MyAddressError};
use crate::hd_wallet::HDAddressSelector;
use crate::mintlayer::address::validate_mintlayer_address;
use crate::mintlayer::{
    build_mintlayer_htlc_output, canonical_transaction_id_from_signed_bytes,
    mintlayer_address_from_compressed_public_key, mintlayer_derivation_path, plan_signed_htlc_refund_offline,
    plan_signed_htlc_spend_offline, plan_signed_output_offline, plan_signed_transaction_offline,
    sdk_private_key_from_kdf_key_pair, MintlayerActivationRequest, MintlayerAddressInfo, MintlayerApiClient,
    MintlayerApiError, MintlayerChainTip, MintlayerCoinConf, MintlayerNetwork, MintlayerNodeClientConfig,
    MintlayerSignedTransactionPlan, MintlayerTransaction, MintlayerUtxo,
};
use crate::utxo::UtxoFeeDetails;
use crate::{
    BalanceError, BalanceFut, CoinBalance, ConfirmPaymentInput, DerivationMethodResponse, MarketCoinOps,
    PrivKeyBuildPolicy, SignatureError, SignatureResult, TransactionData, TransactionDetails, TransactionEnum,
    TransactionErr, TransactionResult, TxFeeDetails, TxMarshalingErr, UnexpectedDerivationMethod, VerificationError,
    VerificationResult, WaitForHTLCTxSpendArgs,
};
use crate::{
    CheckIfMyPaymentSentArgs, DexFee, FeeApproxStage, FoundSwapTxSpend, HistorySyncState, MmCoin,
    NegotiateSwapContractAddrErr, RawTransactionError, RawTransactionFut, RawTransactionRequest, RefundPaymentArgs,
    SearchForSwapTxSpendInput, SendPaymentArgs, SpendPaymentArgs, SwapOps, TradeFee, TradePreimageError,
    TradePreimageFut, TradePreimageResult, TradePreimageValue, ValidateAddressResult, ValidateFeeArgs,
    ValidateOtherPubKeyErr, ValidatePaymentError, ValidatePaymentInput, ValidatePaymentResult, WatcherOps, WeakSpawner,
    WithdrawError, WithdrawFut, WithdrawRequest,
};
use async_trait::async_trait;
use common::executor::AbortedError;
use common::log::info;
use common::now_sec;
use rpc::v1::types::Bytes as BytesJson;
use serde_json::Value as Json;

use common::executor::abortable_queue::AbortableQueue;
use common::executor::{AbortableSystem, Timer};
use crypto::privkey::key_pair_from_secret;
use crypto::Bip44Chain;
use derive_more::Display;
use futures::compat::Future01CompatExt;
use futures::{FutureExt, TryFutureExt};
use futures01::Future;
use keys::KeyPair;
use mintlayer_sdk::crypto::Network as SdkNetwork;
use mintlayer_sdk::node::Client as MintlayerNodeClient;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::{BigDecimal, BigInt, MmNumber};
use rpc::v1::types::H264 as H264Json;
use std::collections::HashSet;
use std::fmt;
use std::ops::Deref;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use url::{Host, Url};

pub const MINTLAYER_DECIMALS: u8 = 11;

const MINTLAYER_FEE_BPS_DENOMINATOR: u128 = 10_000;
const MINTLAYER_BASE_FEE_VOLATILITY_BPS: u128 = 50;

fn mintlayer_fee_rate_for_stage(
    fee_rate: crate::mintlayer::MintlayerFeeRate,
    stage: FeeApproxStage,
) -> Result<crate::mintlayer::MintlayerFeeRate, String> {
    let extra_bps = match stage {
        FeeApproxStage::WithoutApprox => 0,
        FeeApproxStage::StartSwap | FeeApproxStage::WatcherPreimage => MINTLAYER_BASE_FEE_VOLATILITY_BPS,
        FeeApproxStage::OrderIssue | FeeApproxStage::OrderIssueMax => MINTLAYER_BASE_FEE_VOLATILITY_BPS * 2,
        FeeApproxStage::TradePreimage | FeeApproxStage::TradePreimageMax => MINTLAYER_BASE_FEE_VOLATILITY_BPS * 5 / 2,
    };

    if extra_bps == 0 {
        return Ok(fee_rate);
    }

    let rate = fee_rate.atoms_per_kb();
    let whole = rate / MINTLAYER_FEE_BPS_DENOMINATOR;
    let remainder = rate % MINTLAYER_FEE_BPS_DENOMINATOR;

    let whole_extra = whole
        .checked_mul(extra_bps)
        .ok_or_else(|| "Mintlayer fee-rate approximation overflow".to_owned())?;
    let remainder_scaled = remainder
        .checked_mul(extra_bps)
        .ok_or_else(|| "Mintlayer fee-rate approximation overflow".to_owned())?;
    let remainder_extra = remainder_scaled / MINTLAYER_FEE_BPS_DENOMINATOR
        + if remainder_scaled % MINTLAYER_FEE_BPS_DENOMINATOR == 0 {
            0
        } else {
            1
        };

    let extra = whole_extra
        .checked_add(remainder_extra)
        .ok_or_else(|| "Mintlayer fee-rate approximation overflow".to_owned())?;
    let adjusted = rate
        .checked_add(extra)
        .ok_or_else(|| "Mintlayer fee-rate approximation overflow".to_owned())?;

    Ok(crate::mintlayer::MintlayerFeeRate::from_atoms_per_kb(adjusted))
}

#[derive(Debug, Display, PartialEq)]
pub enum MintlayerCoinBuildError {
    EmptyTicker,
    #[display(fmt = "Invalid Mintlayer decimals: expected {}, found {}", expected, actual)]
    InvalidDecimals {
        expected: u8,
        actual: u8,
    },
    InvalidGenesisBlockId,
    MissingApiUrls,
    #[display(fmt = "Invalid Mintlayer API URL at index {}", index)]
    InvalidApiUrl {
        index: usize,
    },
    #[display(fmt = "Unsupported Mintlayer API URL scheme at index {}", index)]
    UnsupportedApiUrlScheme {
        index: usize,
    },
    #[display(fmt = "Credentials are not allowed in Mintlayer API URL at index {}", index)]
    ApiUrlCredentialsNotAllowed {
        index: usize,
    },
    #[display(fmt = "Plain HTTP is allowed only for loopback Mintlayer API URLs; index {}", index)]
    InsecureRemoteApiUrl {
        index: usize,
    },
    #[display(fmt = "Duplicate Mintlayer API URL at index {}", index)]
    DuplicateApiUrl {
        index: usize,
    },
    InvalidNodeRpcUrl,
    NodeRpcUrlCredentialsNotAllowed,
    UnsupportedNodeRpcUrlScheme,
    RemoteNodeRpcUrlNotAllowed,
    InvalidRequiredConfirmations,
    #[display(fmt = "Failed to derive Mintlayer key: {}", _0)]
    KeyDerivation(String),
    #[display(fmt = "Failed to derive Mintlayer address: {}", _0)]
    AddressDerivation(String),
    #[display(fmt = "Unsupported Mintlayer private key policy: {}", policy)]
    UnsupportedPrivKeyPolicy {
        policy: &'static str,
    },
    #[display(fmt = "Failed to create Mintlayer abortable subsystem: {}", _0)]
    AbortableSystem(String),
}

#[derive(Debug, Display)]
pub enum MintlayerNetworkValidationError {
    #[display(fmt = "Mintlayer API request failed: {}", _0)]
    Api(MintlayerApiError),
    #[display(fmt = "Mintlayer API returned invalid genesis block ID '{}'", actual)]
    InvalidGenesisBlockId { actual: String },
    #[display(
        fmt = "Unexpected Mintlayer genesis block ID: expected '{}', found '{}'",
        expected,
        actual
    )]
    UnexpectedGenesisBlockId { expected: String, actual: String },
}

impl From<MintlayerApiError> for MintlayerNetworkValidationError {
    fn from(error: MintlayerApiError) -> Self {
        MintlayerNetworkValidationError::Api(error)
    }
}

#[derive(Clone, Debug)]
pub struct MintlayerCoin(Arc<MintlayerCoinImpl>);

impl Deref for MintlayerCoin {
    type Target = MintlayerCoinImpl;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

pub struct MintlayerCoinImpl {
    conf: MintlayerCoinConf,
    api_urls: Vec<Url>,
    api_client: MintlayerApiClient,
    node_conf: Option<MintlayerNodeClientConfig>,
    tx_history: bool,
    required_confirmations: AtomicU64,
    key_pair: KeyPair,
    address: String,
    derivation_method: DerivationMethodResponse,
    pub abortable_system: Arc<AbortableQueue>,
}

impl fmt::Debug for MintlayerCoinImpl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MintlayerCoinImpl")
            .field("conf", &self.conf)
            .field("api_urls", &self.api_urls)
            .field("node_conf", &self.node_conf)
            .field("tx_history", &self.tx_history)
            .field(
                "required_confirmations",
                &self.required_confirmations.load(Ordering::Relaxed),
            )
            .field("key_pair", &"<redacted>")
            .field("public_key", &hex::encode(self.key_pair.public_slice()))
            .field("address", &self.address)
            .field("derivation_method", &self.derivation_method)
            .finish()
    }
}

impl MintlayerCoin {
    pub fn new(
        ctx: &MmArc,
        conf: MintlayerCoinConf,
        request: MintlayerActivationRequest,
        priv_key_build_policy: PrivKeyBuildPolicy,
    ) -> Result<Self, MintlayerCoinBuildError> {
        if conf.ticker.trim().is_empty() {
            return Err(MintlayerCoinBuildError::EmptyTicker);
        }

        if conf.decimals != MINTLAYER_DECIMALS {
            return Err(MintlayerCoinBuildError::InvalidDecimals {
                expected: MINTLAYER_DECIMALS,
                actual: conf.decimals,
            });
        }

        validate_genesis_block_id(&conf.genesis_block_id)?;

        let api_urls = validate_api_urls(&request.client_conf.api_urls)?;
        let api_client = MintlayerApiClient::new(api_urls.clone());
        let node_conf = request.node_conf.as_ref().map(validate_node_rpc_config).transpose()?;

        let required_confirmations = request.required_confirmations.unwrap_or(conf.required_confirmations);

        if required_confirmations == 0 {
            return Err(MintlayerCoinBuildError::InvalidRequiredConfirmations);
        }

        let (key_pair, address, derivation_method) = build_mintlayer_identity(conf.network, priv_key_build_policy)?;
        let abortable_system = ctx
            .abortable_system
            .create_subsystem()
            .map_err(|error| MintlayerCoinBuildError::AbortableSystem(error.to_string()))?;

        Ok(MintlayerCoin(Arc::new(MintlayerCoinImpl {
            conf,
            api_urls,
            api_client,
            node_conf,
            tx_history: request.tx_history,
            required_confirmations: AtomicU64::new(required_confirmations),
            key_pair,
            address,
            derivation_method,
            abortable_system: Arc::new(abortable_system),
        })))
    }

    pub fn ticker(&self) -> &str {
        &self.conf.ticker
    }

    pub fn network(&self) -> MintlayerNetwork {
        self.conf.network
    }

    pub fn decimals(&self) -> u8 {
        self.conf.decimals
    }

    pub fn genesis_block_id(&self) -> &str {
        &self.conf.genesis_block_id
    }

    pub fn api_urls(&self) -> &[Url] {
        &self.api_urls
    }

    pub fn api_client(&self) -> &MintlayerApiClient {
        &self.api_client
    }

    pub fn node_rpc_url(&self) -> Option<&str> {
        self.node_conf.as_ref().map(|config| config.rpc_url.as_str())
    }

    pub fn node_client(&self) -> Result<Option<MintlayerNodeClient>, String> {
        let Some(config) = self.node_conf.as_ref() else {
            return Ok(None);
        };

        let client = if let Some(cookie_file) = config.rpc_cookie_file.as_deref() {
            let cookie = std::fs::read_to_string(cookie_file)
                .map_err(|error| format!("Failed to read Mintlayer node RPC cookie file: {error}"))?;
            let cookie = cookie.trim_end_matches(['\r', '\n']);
            let (username, password) = cookie
                .split_once(':')
                .ok_or_else(|| "Invalid Mintlayer node RPC cookie format".to_owned())?;
            if username.is_empty() || password.is_empty() {
                return Err("Invalid Mintlayer node RPC cookie format".to_owned());
            }
            MintlayerNodeClient::builder(config.rpc_url.clone())
                .basic_auth(username, password)
                .build()
                .map_err(|error| format!("Failed to build authenticated Mintlayer node RPC client: {error}"))?
        } else {
            MintlayerNodeClient::new(config.rpc_url.clone())
        };
        Ok(Some(client))
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub fn public_key(&self) -> &[u8] {
        self.key_pair.public_slice()
    }

    pub fn derivation_method(&self) -> &DerivationMethodResponse {
        &self.derivation_method
    }

    /// Verifies that the configured genesis belongs to the network served by the API.
    pub async fn validate_network(&self) -> Result<(), MintlayerNetworkValidationError> {
        let genesis = self.api_client.genesis().await?;
        validate_api_genesis_block_id(self.genesis_block_id(), &genesis.block_id)
    }

    pub async fn chain_tip(&self) -> Result<MintlayerChainTip, MintlayerApiError> {
        self.api_client.chain_tip().await
    }

    pub async fn address_info(&self, address: &str) -> Result<MintlayerAddressInfo, MintlayerApiError> {
        self.api_client.address_info(address).await
    }

    /// Returns the native coin balance of the locally derived Mintlayer address.
    pub async fn balance(&self) -> Result<CoinBalance, BalanceError> {
        let address_info = self
            .address_info(self.address())
            .await
            .map_err(|error| BalanceError::Transport(error.to_string()))?;

        coin_balance_from_address_info(address_info)
    }

    pub async fn spendable_utxos(&self, address: &str) -> Result<Vec<MintlayerUtxo>, MintlayerApiError> {
        self.api_client.spendable_utxos(address).await
    }

    pub fn tx_history_enabled(&self) -> bool {
        self.tx_history
    }

    pub fn required_confirmations(&self) -> u64 {
        self.required_confirmations.load(Ordering::Relaxed)
    }

    pub fn set_required_confirmations(&self, confirmations: u64) -> Result<(), MintlayerCoinBuildError> {
        if confirmations == 0 {
            return Err(MintlayerCoinBuildError::InvalidRequiredConfirmations);
        }

        self.required_confirmations.store(confirmations, Ordering::Relaxed);
        Ok(())
    }
}

fn mintlayer_sdk_network(network: MintlayerNetwork) -> SdkNetwork {
    match network {
        MintlayerNetwork::Mainnet => SdkNetwork::Mainnet,
        MintlayerNetwork::Testnet => SdkNetwork::Testnet,
        MintlayerNetwork::Regtest => SdkNetwork::Regtest,
        MintlayerNetwork::Signet => SdkNetwork::Signet,
    }
}

fn mintlayer_atoms_from_decimal(amount: &BigDecimal) -> Result<u128, String> {
    let rendered = amount.to_string();
    let rendered = rendered.strip_prefix('+').unwrap_or(&rendered);
    if rendered.starts_with('-') {
        return Err("Mintlayer withdrawal amount cannot be negative".into());
    }

    let mut parts = rendered.split('.');
    let integer = parts.next().unwrap_or_default();
    let fractional = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || !fractional.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(format!("Invalid Mintlayer decimal amount: {rendered}"));
    }

    let decimals = MINTLAYER_DECIMALS as usize;
    let (kept_fractional, excess_fractional) = if fractional.len() > decimals {
        fractional.split_at(decimals)
    } else {
        (fractional, "")
    };
    if !excess_fractional.bytes().all(|byte| byte == b'0') {
        return Err(format!(
            "Mintlayer amount has more than {MINTLAYER_DECIMALS} decimal places: {rendered}"
        ));
    }

    let scale = 10_u128.pow(MINTLAYER_DECIMALS as u32);
    let integer_atoms = integer
        .parse::<u128>()
        .map_err(|error| format!("Invalid Mintlayer amount '{rendered}': {error}"))?
        .checked_mul(scale)
        .ok_or_else(|| format!("Mintlayer amount overflow: {rendered}"))?;

    let mut padded_fractional = kept_fractional.to_owned();
    padded_fractional.push_str(&"0".repeat(decimals - kept_fractional.len()));
    let fractional_atoms = if padded_fractional.is_empty() {
        0
    } else {
        padded_fractional
            .parse::<u128>()
            .map_err(|error| format!("Invalid Mintlayer amount '{rendered}': {error}"))?
    };

    integer_atoms
        .checked_add(fractional_atoms)
        .ok_or_else(|| format!("Mintlayer amount overflow: {rendered}"))
}

fn mintlayer_decimal_from_atoms(atoms: u128) -> BigDecimal {
    BigDecimal::new(BigInt::from(atoms), MINTLAYER_DECIMALS as i64)
}

fn mintlayer_withdraw_details(
    ticker: &str,
    sender: String,
    recipient: String,
    plan: MintlayerSignedTransactionPlan,
) -> Result<TransactionDetails, WithdrawError> {
    let transaction_id = plan.transaction_id.clone();
    let internal_id = hex::decode(&transaction_id)
        .map_err(|error| WithdrawError::InternalError(format!("Invalid Mintlayer transaction ID: {error}")))?;
    let spent_by_me = mintlayer_decimal_from_atoms(plan.selected_atoms);
    let received_by_me = mintlayer_decimal_from_atoms(plan.change_atoms);
    let fee_amount = mintlayer_decimal_from_atoms(plan.fee_atoms);

    Ok(TransactionDetails {
        tx: TransactionData::new_signed(plan.signed_bytes.into(), transaction_id),
        from: vec![sender],
        to: vec![recipient],
        total_amount: spent_by_me.clone(),
        spent_by_me: spent_by_me.clone(),
        received_by_me: received_by_me.clone(),
        my_balance_change: received_by_me - spent_by_me,
        block_height: 0,
        timestamp: now_sec(),
        fee_details: Some(TxFeeDetails::Utxo(UtxoFeeDetails {
            coin: Some(ticker.to_owned()),
            amount: fee_amount,
        })),
        coin: ticker.to_owned(),
        internal_id: internal_id.into(),
        kmd_rewards: None,
        transaction_type: Default::default(),
        memo: None,
    })
}

async fn broadcast_mintlayer_transaction_data(coin: &MintlayerCoin, tx: &TransactionData) -> Result<String, String> {
    let tx_hex = tx
        .tx_hex()
        .ok_or_else(|| "Mintlayer broadcast requires signed transaction data".to_owned())?;
    let tx_hash = tx
        .tx_hash()
        .ok_or_else(|| "Mintlayer broadcast requires a canonical transaction ID".to_owned())?
        .to_owned();

    let canonical_id = canonical_transaction_id_from_signed_bytes(&tx_hex.0).map_err(|error| error.to_string())?;

    if canonical_id != tx_hash {
        return Err(format!(
            "Mintlayer TransactionData txid mismatch: canonical {}, stored {}",
            canonical_id, tx_hash
        ));
    }

    let submitted_txid = coin.send_raw_tx_bytes(&tx_hex.0).compat().await?;

    if submitted_txid != canonical_id {
        return Err(format!(
            "Mintlayer HTTPS broadcast returned txid {} but expected {}",
            submitted_txid, canonical_id
        ));
    }

    Ok(canonical_id)
}

async fn build_mintlayer_withdraw(
    coin: MintlayerCoin,
    req: WithdrawRequest,
) -> Result<TransactionDetails, WithdrawError> {
    if req.coin != coin.ticker() {
        return Err(WithdrawError::UnsupportedError(format!(
            "Mintlayer withdraw request coin '{}' does not match '{}'",
            req.coin,
            coin.ticker()
        )));
    }
    if req.from.is_some() {
        return Err(WithdrawError::UnsupportedError(
            "Mintlayer withdraw does not support an alternate from address yet".into(),
        ));
    }
    if req.max {
        return Err(WithdrawError::UnsupportedError(
            "Mintlayer withdraw max mode is not implemented yet".into(),
        ));
    }
    if req.fee.is_some() {
        return Err(WithdrawError::InvalidFeePolicy(
            "Mintlayer withdraw currently uses the canonical API fee rate only".into(),
        ));
    }
    if req.memo.is_some() {
        return Err(WithdrawError::UnsupportedError(
            "Mintlayer withdraw memo is not supported".into(),
        ));
    }
    if req.ibc_source_channel.is_some() || req.expiration_seconds.is_some() {
        return Err(WithdrawError::UnsupportedError(
            "Unsupported protocol-specific Mintlayer withdraw fields".into(),
        ));
    }
    validate_mintlayer_address(coin.network(), &req.to)
        .map_err(|error| WithdrawError::InvalidAddress(error.to_string()))?;
    let send_atoms = mintlayer_atoms_from_decimal(&req.amount).map_err(WithdrawError::InternalError)?;
    if send_atoms == 0 {
        return Err(WithdrawError::UnsupportedError(
            "Mintlayer withdrawal amount must be greater than zero".into(),
        ));
    }

    let sender = coin.address().to_owned();
    let utxos = coin
        .spendable_utxos(&sender)
        .await
        .map_err(|error| WithdrawError::Transport(error.to_string()))?;
    let fee_rate = coin
        .api_client()
        .fee_rate()
        .await
        .map_err(|error| WithdrawError::Transport(error.to_string()))?;
    let chain_tip = coin
        .chain_tip()
        .await
        .map_err(|error| WithdrawError::Transport(error.to_string()))?;

    let sdk_private_key = sdk_private_key_from_kdf_key_pair(&coin.key_pair)
        .map_err(|error| WithdrawError::InternalError(error.to_string()))?;
    let plan = plan_signed_transaction_offline(
        &utxos,
        &sender,
        &req.to,
        send_atoms,
        fee_rate,
        &sdk_private_key,
        chain_tip.block_height,
        mintlayer_sdk_network(coin.network()),
    )
    .map_err(|error| WithdrawError::InternalError(error.to_string()))?;
    drop(sdk_private_key);

    mintlayer_withdraw_details(coin.ticker(), sender, req.to, plan)
}

async fn build_mintlayer_swap_payment_with_fee(
    coin: &MintlayerCoin,
    amount: &BigDecimal,
    other_pubkey: &[u8],
    secret_hash: &[u8],
    time_lock: u64,
    swap_unique_data: &[u8],
) -> Result<(MintlayerTransaction, u128), String> {
    let fee_rate = coin.api_client().fee_rate().await.map_err(|error| error.to_string())?;

    build_mintlayer_swap_payment_with_fee_rate(
        coin,
        amount,
        other_pubkey,
        secret_hash,
        time_lock,
        swap_unique_data,
        fee_rate,
    )
    .await
}

async fn build_mintlayer_swap_payment_with_fee_rate(
    coin: &MintlayerCoin,
    amount: &BigDecimal,
    other_pubkey: &[u8],
    secret_hash: &[u8],
    time_lock: u64,
    swap_unique_data: &[u8],
    fee_rate: crate::mintlayer::MintlayerFeeRate,
) -> Result<(MintlayerTransaction, u128), String> {
    let send_atoms = mintlayer_atoms_from_decimal(amount)?;
    if send_atoms == 0 {
        return Err("Mintlayer swap payment amount must be greater than zero".into());
    }

    coin.validate_other_pubkey(other_pubkey)
        .map_err(|error| error.to_string())?;

    let spend_address = mintlayer_address_from_compressed_public_key(coin.network(), other_pubkey)
        .map_err(|error| error.to_string())?;

    let htlc_key_pair = coin.derive_htlc_key_pair(swap_unique_data);
    let refund_address = mintlayer_address_from_compressed_public_key(coin.network(), htlc_key_pair.public_slice())
        .map_err(|error| error.to_string())?;

    let network = mintlayer_sdk_network(coin.network());

    let payment_output = build_mintlayer_htlc_output(
        send_atoms,
        secret_hash,
        &spend_address,
        &refund_address,
        time_lock,
        network,
    )
    .map_err(|error| error.to_string())?;

    let sender = coin.address().to_owned();
    let utxos = coin.spendable_utxos(&sender).await.map_err(|error| error.to_string())?;

    let chain_tip = coin.chain_tip().await.map_err(|error| error.to_string())?;

    let sdk_private_key = sdk_private_key_from_kdf_key_pair(&coin.key_pair).map_err(|error| error.to_string())?;

    let plan = plan_signed_output_offline(
        &utxos,
        &sender,
        payment_output,
        send_atoms,
        fee_rate,
        &sdk_private_key,
        chain_tip.block_height,
        network,
    )
    .map_err(|error| error.to_string())?;

    drop(sdk_private_key);

    let fee_atoms = plan.fee_atoms;

    Ok((
        MintlayerTransaction {
            signed_bytes: plan.signed_bytes,
            transaction_id: plan.transaction_id,
        },
        fee_atoms,
    ))
}

async fn build_mintlayer_swap_payment(
    coin: &MintlayerCoin,
    args: &SendPaymentArgs<'_>,
) -> Result<MintlayerTransaction, String> {
    let (transaction, _fee_atoms) = build_mintlayer_swap_payment_with_fee(
        coin,
        &args.amount,
        args.other_pubkey,
        args.secret_hash,
        args.time_lock,
        args.swap_unique_data,
    )
    .await?;

    Ok(transaction)
}

async fn mintlayer_sender_trade_fee(coin: &MintlayerCoin, value: TradePreimageValue) -> Result<TradeFee, String> {
    mintlayer_sender_trade_fee_for_stage(coin, value, FeeApproxStage::WithoutApprox).await
}

async fn mintlayer_sender_trade_fee_for_stage(
    coin: &MintlayerCoin,
    value: TradePreimageValue,
    stage: FeeApproxStage,
) -> Result<TradeFee, String> {
    const DUMMY_SECRET_HASH: [u8; 20] = [0x42; 20];
    const DUMMY_TIME_LOCK: u64 = 1_800_000_000;
    const DUMMY_SWAP_UNIQUE_DATA: &[u8] = b"mintlayer-trade-fee";

    let other_secret = [0x73_u8; 32];
    let other_key_pair = key_pair_from_secret(&other_secret).map_err(|error| error.to_string())?;
    let other_pubkey = other_key_pair.public_slice();

    let fee_atoms = match value {
        TradePreimageValue::Exact(amount) => {
            let fee_rate = coin.api_client().fee_rate().await.map_err(|error| error.to_string())?;
            let fee_rate = mintlayer_fee_rate_for_stage(fee_rate, stage)?;
            let (_transaction, fee_atoms) = build_mintlayer_swap_payment_with_fee_rate(
                coin,
                &amount,
                other_pubkey,
                &DUMMY_SECRET_HASH,
                DUMMY_TIME_LOCK,
                DUMMY_SWAP_UNIQUE_DATA,
                fee_rate,
            )
            .await?;
            fee_atoms
        },
        TradePreimageValue::UpperBound(budget) => {
            let budget_atoms = mintlayer_atoms_from_decimal(&budget)?;
            if budget_atoms == 0 {
                return Err("Mintlayer sender trade fee budget must be greater than zero".into());
            }

            coin.validate_other_pubkey(other_pubkey)
                .map_err(|error| error.to_string())?;
            let spend_address = mintlayer_address_from_compressed_public_key(coin.network(), other_pubkey)
                .map_err(|error| error.to_string())?;
            let htlc_key_pair = coin.derive_htlc_key_pair(DUMMY_SWAP_UNIQUE_DATA);
            let refund_address =
                mintlayer_address_from_compressed_public_key(coin.network(), htlc_key_pair.public_slice())
                    .map_err(|error| error.to_string())?;
            let network = mintlayer_sdk_network(coin.network());

            let sender = coin.address().to_owned();
            let utxos = coin.spendable_utxos(&sender).await.map_err(|error| error.to_string())?;
            let fee_rate = coin.api_client().fee_rate().await.map_err(|error| error.to_string())?;
            let fee_rate = mintlayer_fee_rate_for_stage(fee_rate, stage)?;
            let chain_tip = coin.chain_tip().await.map_err(|error| error.to_string())?;
            let sdk_private_key =
                sdk_private_key_from_kdf_key_pair(&coin.key_pair).map_err(|error| error.to_string())?;

            // Start with a positive candidate inside the budget. Rebuild the actual
            // signed HTLC for every fee guess; output size and UTXO selection may
            // change as the payment amount changes.
            let mut fee_guess_atoms = budget_atoms / 2;
            let mut converged_fee = None;
            let mut visited_fee_guesses = HashSet::new();
            let mut largest_fee_guess_atoms = fee_guess_atoms;
            for _ in 0..16 {
                let send_atoms = budget_atoms
                    .checked_sub(fee_guess_atoms)
                    .filter(|amount| *amount > 0)
                    .ok_or_else(|| "Mintlayer sender trade fee exhausts the upper-bound budget".to_owned())?;
                let payment_output = build_mintlayer_htlc_output(
                    send_atoms,
                    &DUMMY_SECRET_HASH,
                    &spend_address,
                    &refund_address,
                    DUMMY_TIME_LOCK,
                    network,
                )
                .map_err(|error| error.to_string())?;
                let plan = plan_signed_output_offline(
                    &utxos,
                    &sender,
                    payment_output,
                    send_atoms,
                    fee_rate,
                    &sdk_private_key,
                    chain_tip.block_height,
                    network,
                )
                .map_err(|error| error.to_string())?;

                let repeated_guess = !visited_fee_guesses.insert(fee_guess_atoms);
                if repeated_guess && fee_guess_atoms != largest_fee_guess_atoms {
                    fee_guess_atoms = largest_fee_guess_atoms;
                    continue;
                }

                if plan.fee_atoms == fee_guess_atoms {
                    let total_spent = send_atoms
                        .checked_add(plan.fee_atoms)
                        .ok_or_else(|| "Mintlayer upper-bound amount overflow".to_owned())?;
                    if total_spent <= budget_atoms {
                        converged_fee = Some(plan.fee_atoms);
                        break;
                    }
                    return Err("Mintlayer sender trade fee exceeds the upper-bound budget".into());
                }
                if repeated_guess {
                    let total_spent = send_atoms
                        .checked_add(plan.fee_atoms)
                        .ok_or_else(|| "Mintlayer upper-bound amount overflow".to_owned())?;
                    if plan.fee_atoms > fee_guess_atoms || total_spent > budget_atoms {
                        return Err("Mintlayer sender trade fee cycle has no fundable upper bound".into());
                    }

                    // The current quote is safe. Search for a smaller safe quote
                    // without assuming that a fixed-point fee exists at the UTXO
                    // boundary. Every candidate is signed and checked offline.
                    let mut lower = 0_u128;
                    let mut upper = fee_guess_atoms;
                    while lower + 1 < upper {
                        let candidate_quote = lower + (upper - lower) / 2;
                        let candidate_send = budget_atoms - candidate_quote;
                        let candidate_output = build_mintlayer_htlc_output(
                            candidate_send,
                            &DUMMY_SECRET_HASH,
                            &spend_address,
                            &refund_address,
                            DUMMY_TIME_LOCK,
                            network,
                        )
                        .map_err(|error| error.to_string())?;
                        let candidate = plan_signed_output_offline(
                            &utxos,
                            &sender,
                            candidate_output,
                            candidate_send,
                            fee_rate,
                            &sdk_private_key,
                            chain_tip.block_height,
                            network,
                        );
                        let safe = match candidate {
                            Ok(candidate_plan) => {
                                candidate_plan.fee_atoms <= candidate_quote
                                    && candidate_plan
                                        .send_atoms
                                        .checked_add(candidate_plan.fee_atoms)
                                        .is_some_and(|spent| spent <= budget_atoms)
                            },
                            Err(_) => false,
                        };
                        if safe {
                            upper = candidate_quote;
                        } else {
                            lower = candidate_quote;
                        }
                    }
                    // `upper` always remains a verified fundable quote.
                    converged_fee = Some(upper);
                    break;
                }
                largest_fee_guess_atoms = largest_fee_guess_atoms.max(plan.fee_atoms);
                fee_guess_atoms = plan.fee_atoms;
            }
            drop(sdk_private_key);
            converged_fee
                .ok_or_else(|| "Mintlayer sender trade fee did not converge within 16 iterations".to_owned())?
        },
    };

    Ok(TradeFee {
        coin: coin.ticker().to_owned(),
        amount: mintlayer_decimal_from_atoms(fee_atoms).into(),
        paid_from_trading_vol: false,
    })
}

async fn validate_mintlayer_dex_fee(coin: &MintlayerCoin, args: ValidateFeeArgs<'_>) -> Result<(), String> {
    use mintlayer_sdk::crypto::types::{OutputValue, TxOutput};

    let transaction = match args.fee_tx {
        TransactionEnum::MintlayerTransaction(transaction) => transaction,
        other => {
            return Err(format!("Expected Mintlayer DEX fee transaction, got {:?}", other));
        },
    };

    let canonical_txid =
        canonical_transaction_id_from_signed_bytes(&transaction.signed_bytes).map_err(|error| error.to_string())?;

    if canonical_txid != transaction.transaction_id {
        return Err(format!(
            "Mintlayer DEX fee transaction ID '{}' does not match canonical transaction ID '{}'",
            transaction.transaction_id, canonical_txid
        ));
    }

    const VISIBILITY_TIMEOUT_SECS: u64 = 60;
    const VISIBILITY_POLL_SECS: f64 = 2.0;

    let visibility_deadline = now_sec().saturating_add(VISIBILITY_TIMEOUT_SECS);
    let scanner_transaction = loop {
        match coin.api_client().transaction(&canonical_txid).await {
            Ok(scanner_transaction) => {
                if scanner_transaction.id != canonical_txid {
                    return Err(format!(
                        "Mintlayer scanner returned transaction ID '{}' for requested DEX fee transaction '{}'",
                        scanner_transaction.id, canonical_txid
                    ));
                }

                if !scanner_transaction.block_id.is_empty() {
                    break scanner_transaction;
                }

                if now_sec() >= visibility_deadline {
                    return Err(format!(
                        "Mintlayer DEX fee transaction {} did not become confirmed within {} seconds",
                        canonical_txid, VISIBILITY_TIMEOUT_SECS
                    ));
                }
            },
            Err(error) if error.is_transaction_not_found() => {
                if now_sec() >= visibility_deadline {
                    return Err(format!(
                        "Mintlayer DEX fee transaction {} did not become visible within {} seconds",
                        canonical_txid, VISIBILITY_TIMEOUT_SECS
                    ));
                }
            },
            Err(error) => return Err(error.to_string()),
        }

        Timer::sleep(VISIBILITY_POLL_SECS).await;
    };

    let block_height = coin
        .api_client()
        .block_height_in_main_chain(&scanner_transaction.block_id)
        .await
        .map_err(|error| error.to_string())?
        .ok_or_else(|| {
            format!(
                "Mintlayer DEX fee transaction {} block {} is not in the main chain",
                canonical_txid, scanner_transaction.block_id
            )
        })?;

    if block_height < args.min_block_number {
        return Err(format!(
            "Mintlayer DEX fee transaction {} was included at block {}, below minimum block {}",
            canonical_txid, block_height, args.min_block_number
        ));
    }

    let expected_sender = mintlayer_address_from_compressed_public_key(coin.network(), args.expected_sender)
        .map_err(|error| error.to_string())?;

    if scanner_transaction.inputs.is_empty() {
        return Err(format!(
            "Mintlayer DEX fee transaction {} has no inputs",
            canonical_txid
        ));
    }

    for input in &scanner_transaction.inputs {
        let utxo = input.utxo.as_ref().ok_or_else(|| {
            format!(
                "Mintlayer DEX fee transaction {} input has no previous UTXO metadata",
                canonical_txid
            )
        })?;

        let destination = utxo.get("destination").and_then(Json::as_str).ok_or_else(|| {
            format!(
                "Mintlayer DEX fee transaction {} input UTXO has no destination",
                canonical_txid
            )
        })?;

        if destination != expected_sender {
            return Err(format!(
                "Mintlayer DEX fee transaction {} input belongs to {}, expected {}",
                canonical_txid, destination, expected_sender
            ));
        }
    }

    let expected_amount = match args.dex_fee {
        DexFee::Standard(amount) => amount.to_decimal(),
        DexFee::NoFee => {
            return Err("Mintlayer validate_fee received DexFee::NoFee".into());
        },
        DexFee::WithBurn { .. } => {
            return Err("Mintlayer direct DEX fee burn validation is not supported".into());
        },
    };

    let expected_atoms = mintlayer_atoms_from_decimal(&expected_amount)?;
    let dex_fee_address = mintlayer_dex_fee_address(coin)?;
    let expected_destination =
        mintlayer_sdk::crypto::encode_destination(&dex_fee_address, mintlayer_sdk_network(coin.network()))
            .map_err(|error| error.to_string())?;

    let decoded = mintlayer_sdk::crypto::decode_transaction_lenient(&transaction.signed_bytes)
        .map_err(|error| error.to_string())?;

    let paid_atoms = decoded
        .outputs()
        .iter()
        .filter_map(|output| match output {
            TxOutput::Transfer(OutputValue::Coin(amount), destination) if destination == &expected_destination => {
                Some(amount.into_atoms())
            },
            _ => None,
        })
        .try_fold(0_u128, |total, atoms| {
            total
                .checked_add(atoms)
                .ok_or_else(|| "Mintlayer DEX fee output amount overflow".to_owned())
        })?;

    if paid_atoms < expected_atoms {
        return Err(format!(
            "Mintlayer DEX fee transaction {} pays {} atoms to DEX address, expected at least {}",
            canonical_txid, paid_atoms, expected_atoms
        ));
    }

    Ok(())
}

fn mintlayer_dex_fee_address(coin: &MintlayerCoin) -> Result<String, String> {
    mintlayer_address_from_compressed_public_key(coin.network(), coin.dex_pubkey()).map_err(|error| error.to_string())
}

async fn build_mintlayer_taker_fee_with_fee(
    coin: &MintlayerCoin,
    dex_fee: DexFee,
) -> Result<Option<(MintlayerTransaction, u128)>, String> {
    let fee_rate = coin.api_client().fee_rate().await.map_err(|error| error.to_string())?;
    build_mintlayer_taker_fee_with_fee_rate(coin, dex_fee, fee_rate).await
}

async fn build_mintlayer_taker_fee_with_fee_rate(
    coin: &MintlayerCoin,
    dex_fee: DexFee,
    fee_rate: crate::mintlayer::MintlayerFeeRate,
) -> Result<Option<(MintlayerTransaction, u128)>, String> {
    let amount = match dex_fee {
        DexFee::NoFee => return Ok(None),
        DexFee::Standard(amount) => amount,
        DexFee::WithBurn { .. } => {
            return Err("Mintlayer direct DEX fee burn is not supported".into());
        },
    };

    let amount = amount.to_decimal();
    let send_atoms = mintlayer_atoms_from_decimal(&amount)?;

    if send_atoms == 0 {
        return Err("Mintlayer DEX fee amount must be greater than zero".into());
    }

    let sender = coin.address().to_owned();
    let dex_fee_address = mintlayer_dex_fee_address(coin)?;

    let utxos = coin.spendable_utxos(&sender).await.map_err(|error| error.to_string())?;

    let chain_tip = coin.chain_tip().await.map_err(|error| error.to_string())?;

    let sdk_private_key = sdk_private_key_from_kdf_key_pair(&coin.key_pair).map_err(|error| error.to_string())?;

    let plan = plan_signed_transaction_offline(
        &utxos,
        &sender,
        &dex_fee_address,
        send_atoms,
        fee_rate,
        &sdk_private_key,
        chain_tip.block_height,
        mintlayer_sdk_network(coin.network()),
    )
    .map_err(|error| error.to_string())?;

    drop(sdk_private_key);

    let fee_atoms = plan.fee_atoms;

    Ok(Some((
        MintlayerTransaction {
            signed_bytes: plan.signed_bytes,
            transaction_id: plan.transaction_id,
        },
        fee_atoms,
    )))
}

async fn build_mintlayer_taker_fee(
    coin: &MintlayerCoin,
    dex_fee: DexFee,
) -> Result<Option<MintlayerTransaction>, String> {
    Ok(build_mintlayer_taker_fee_with_fee(coin, dex_fee)
        .await?
        .map(|(transaction, _fee_atoms)| transaction))
}

async fn send_mintlayer_taker_fee(coin: &MintlayerCoin, dex_fee: DexFee) -> TransactionResult {
    let Some(transaction) = build_mintlayer_taker_fee(coin, dex_fee)
        .await
        .map_err(TransactionErr::Plain)?
    else {
        return Err(TransactionErr::Plain(
            "Mintlayer send_taker_fee received DexFee::NoFee".into(),
        ));
    };

    let broadcast_txid = coin
        .send_raw_tx_bytes(&transaction.signed_bytes)
        .compat()
        .await
        .map_err(TransactionErr::Plain)?;

    if broadcast_txid != transaction.transaction_id {
        return Err(TransactionErr::Plain(format!(
            "Mintlayer broadcaster returned transaction ID '{}' but built DEX fee transaction ID is '{}'",
            broadcast_txid, transaction.transaction_id
        )));
    }

    Ok(transaction.into())
}

async fn send_mintlayer_swap_payment(coin: &MintlayerCoin, args: &SendPaymentArgs<'_>) -> TransactionResult {
    let transaction = build_mintlayer_swap_payment(coin, args)
        .await
        .map_err(TransactionErr::Plain)?;

    let broadcast_txid = coin
        .send_raw_tx_bytes(&transaction.signed_bytes)
        .compat()
        .await
        .map_err(TransactionErr::Plain)?;

    if broadcast_txid != transaction.transaction_id {
        return Err(TransactionErr::Plain(format!(
            "Mintlayer broadcaster returned transaction ID '{}' but built transaction ID is '{}'",
            broadcast_txid, transaction.transaction_id
        )));
    }

    Ok(transaction.into())
}

fn mintlayer_htlc_payment_outpoint(
    payment_tx: &[u8],
) -> Result<(String, mintlayer_sdk::crypto::types::TxOutput), String> {
    let payment = mintlayer_sdk::crypto::decode_transaction_lenient(payment_tx).map_err(|error| error.to_string())?;

    let payment_txid = canonical_transaction_id_from_signed_bytes(payment_tx).map_err(|error| error.to_string())?;

    let htlc_output = payment
        .outputs()
        .first()
        .cloned()
        .ok_or_else(|| "Mintlayer swap payment has no outputs".to_string())?;

    if !matches!(
        htlc_output,
        mintlayer_sdk::crypto::types::TxOutput::Htlc(mintlayer_sdk::crypto::types::OutputValue::Coin(_), _)
    ) {
        return Err("Mintlayer swap payment output 0 is not a native coin HTLC".to_string());
    }

    Ok((payment_txid, htlc_output))
}

fn verify_mintlayer_swap_payment(
    coin: &MintlayerCoin,
    payment_tx: &[u8],
    spend_pub: &[u8],
    refund_pub: &[u8],
    secret_hash: &[u8],
    amount: &BigDecimal,
    time_lock: u64,
) -> Result<String, String> {
    coin.validate_other_pubkey(spend_pub)
        .map_err(|error| error.to_string())?;
    coin.validate_other_pubkey(refund_pub)
        .map_err(|error| error.to_string())?;

    let expected_atoms = mintlayer_atoms_from_decimal(amount)?;

    if expected_atoms == 0 {
        return Err("Mintlayer swap payment amount must be greater than zero".into());
    }

    let spend_address =
        mintlayer_address_from_compressed_public_key(coin.network(), spend_pub).map_err(|error| error.to_string())?;

    let refund_address =
        mintlayer_address_from_compressed_public_key(coin.network(), refund_pub).map_err(|error| error.to_string())?;

    let expected_output = build_mintlayer_htlc_output(
        expected_atoms,
        secret_hash,
        &spend_address,
        &refund_address,
        time_lock,
        mintlayer_sdk_network(coin.network()),
    )
    .map_err(|error| error.to_string())?;

    let (payment_txid, actual_output) = mintlayer_htlc_payment_outpoint(payment_tx)?;

    if actual_output != expected_output {
        return Err(
            "Mintlayer swap payment HTLC output does not match expected amount, secret hash, keys or timelock"
                .to_owned(),
        );
    }

    Ok(payment_txid)
}

async fn build_mintlayer_htlc_spend_with_fee(
    coin: &MintlayerCoin,
    payment_txid: &str,
    htlc_output: &mintlayer_sdk::crypto::types::TxOutput,
    secret: &[u8],
) -> Result<(MintlayerTransaction, u128), String> {
    let fee_rate = coin.api_client().fee_rate().await.map_err(|error| error.to_string())?;
    let chain_tip = coin.chain_tip().await.map_err(|error| error.to_string())?;

    let network = mintlayer_sdk_network(coin.network());
    let destination_address = coin.address().to_owned();

    let sdk_private_key = sdk_private_key_from_kdf_key_pair(&coin.key_pair).map_err(|error| error.to_string())?;

    let plan = plan_signed_htlc_spend_offline(
        payment_txid,
        0,
        htlc_output,
        &destination_address,
        &sdk_private_key,
        secret,
        fee_rate,
        chain_tip.block_height,
        network,
    )
    .map_err(|error| error.to_string())?;

    drop(sdk_private_key);

    let fee_atoms = plan.fee_atoms;

    Ok((
        MintlayerTransaction {
            signed_bytes: plan.signed_bytes,
            transaction_id: plan.transaction_id,
        },
        fee_atoms,
    ))
}

async fn build_mintlayer_htlc_spend(
    coin: &MintlayerCoin,
    args: &SpendPaymentArgs<'_>,
) -> Result<MintlayerTransaction, String> {
    let (payment_txid, htlc_output) = mintlayer_htlc_payment_outpoint(args.other_payment_tx)?;

    let (transaction, _fee_atoms) =
        build_mintlayer_htlc_spend_with_fee(coin, &payment_txid, &htlc_output, args.secret).await?;

    Ok(transaction)
}

async fn mintlayer_receiver_trade_fee(coin: &MintlayerCoin) -> Result<TradeFee, String> {
    const DUMMY_SECRET: [u8; 32] = [0x56; 32];
    const DUMMY_TIME_LOCK: u64 = 1_800_000_000;
    const DUMMY_PAYMENT_ATOMS: u128 = 100_000_000_000;

    let secret_hash = mintlayer_sdk::crypto::types::HtlcSecret::new(DUMMY_SECRET).hash();

    let spend_address = mintlayer_address_from_compressed_public_key(coin.network(), coin.public_key())
        .map_err(|error| error.to_string())?;

    let refund_address = spend_address.clone();

    let htlc_output = build_mintlayer_htlc_output(
        DUMMY_PAYMENT_ATOMS,
        secret_hash.as_ref(),
        &spend_address,
        &refund_address,
        DUMMY_TIME_LOCK,
        mintlayer_sdk_network(coin.network()),
    )
    .map_err(|error| error.to_string())?;

    let dummy_payment_txid = "11".repeat(32);

    let (_transaction, fee_atoms) =
        build_mintlayer_htlc_spend_with_fee(coin, &dummy_payment_txid, &htlc_output, &DUMMY_SECRET).await?;

    Ok(TradeFee {
        coin: coin.ticker().to_owned(),
        amount: mintlayer_decimal_from_atoms(fee_atoms).into(),
        paid_from_trading_vol: true,
    })
}

async fn build_mintlayer_htlc_refund(
    coin: &MintlayerCoin,
    args: &RefundPaymentArgs<'_>,
) -> Result<MintlayerTransaction, String> {
    let (payment_txid, htlc_output) = mintlayer_htlc_payment_outpoint(args.payment_tx)?;

    let fee_rate = coin.api_client().fee_rate().await.map_err(|error| error.to_string())?;

    let chain_tip = coin.chain_tip().await.map_err(|error| error.to_string())?;

    let network = mintlayer_sdk_network(coin.network());
    let destination_address = coin.address().to_owned();

    let htlc_key_pair = coin.derive_htlc_key_pair(args.swap_unique_data);

    let sdk_private_key = sdk_private_key_from_kdf_key_pair(&htlc_key_pair).map_err(|error| error.to_string())?;

    let plan = plan_signed_htlc_refund_offline(
        &payment_txid,
        0,
        &htlc_output,
        &destination_address,
        &sdk_private_key,
        fee_rate,
        chain_tip.block_height,
        network,
    )
    .map_err(|error| error.to_string())?;

    drop(sdk_private_key);

    Ok(MintlayerTransaction {
        signed_bytes: plan.signed_bytes,
        transaction_id: plan.transaction_id,
    })
}

async fn load_mintlayer_chain_transaction(
    coin: &MintlayerCoin,
    block_id: &str,
    transaction_id: &str,
) -> Result<MintlayerTransaction, String> {
    let transaction = coin
        .api_client()
        .transaction_with_tx_hex(transaction_id)
        .await
        .map_err(|error| error.to_string())?;

    if transaction.id != transaction_id {
        return Err(format!(
            "Mintlayer transaction ID mismatch: API returned '{}' for requested transaction '{}'",
            transaction.id, transaction_id
        ));
    }

    if transaction.block_id != block_id {
        return Err(format!(
            "Mintlayer transaction '{}' block mismatch: API returned '{}' but observer expected '{}'",
            transaction_id, transaction.block_id, block_id
        ));
    }

    let tx_hex = transaction.tx_hex.as_deref().ok_or_else(|| {
        format!(
            "Mintlayer transaction '{}' response has no tx_hex after capability-aware API selection",
            transaction_id
        )
    })?;

    let signed_bytes = hex::decode(tx_hex)
        .map_err(|error| format!("Invalid Mintlayer transaction hex '{}': {}", transaction_id, error))?;

    let canonical_id = canonical_transaction_id_from_signed_bytes(&signed_bytes).map_err(|error| error.to_string())?;

    if canonical_id != transaction_id {
        return Err(format!(
            "Mintlayer transaction ID mismatch: API returned '{}' but canonical ID is '{}'",
            transaction_id, canonical_id
        ));
    }

    Ok(MintlayerTransaction {
        signed_bytes,
        transaction_id: canonical_id,
    })
}

async fn find_mintlayer_htlc_resolution(
    coin: &MintlayerCoin,
    address_info: &MintlayerAddressInfo,
    payment_transaction_id: &str,
) -> Result<Option<MintlayerTransaction>, String> {
    for candidate_transaction_id in &address_info.transaction_history {
        if candidate_transaction_id == payment_transaction_id {
            continue;
        }

        let transaction = coin
            .api_client()
            .transaction(candidate_transaction_id)
            .await
            .map_err(|error| error.to_string())?;

        let spends_payment = transaction.inputs.iter().any(|input_info| {
            let input = &input_info.input;

            input.input_type == "UTXO"
                && input.source_type.as_deref() == Some("Transaction")
                && input.source_id.as_deref() == Some(payment_transaction_id)
                && input.index == Some(0)
        });

        if !spends_payment {
            continue;
        }

        let canonical_transaction =
            load_mintlayer_chain_transaction(coin, &transaction.block_id, candidate_transaction_id).await?;

        return Ok(Some(canonical_transaction));
    }

    Ok(None)
}

async fn observe_mintlayer_htlc_spend_from_payment(
    coin: &MintlayerCoin,
    payment_tx: &[u8],
    from_block: u64,
) -> Result<Option<MintlayerTransaction>, String> {
    let (payment_transaction_id, _) = mintlayer_htlc_payment_outpoint(payment_tx)?;

    let output = coin
        .api_client()
        .transaction_output(&payment_transaction_id, 0)
        .await
        .map_err(|error| error.to_string())?;

    if output.output_type != "Htlc" {
        return Err(format!(
            "Mintlayer payment output 0 has type '{}' instead of Htlc",
            output.output_type
        ));
    }

    let htlc = output
        .htlc
        .ok_or_else(|| "Mintlayer HTLC payment output is missing HTLC metadata".to_owned())?;

    let Some(spent_at_block_height) = output.spent_at_block_height else {
        return Ok(None);
    };

    if spent_at_block_height < from_block {
        return Ok(None);
    }

    let spend_info = coin
        .api_client()
        .address_info(&htlc.spend_key)
        .await
        .map_err(|error| error.to_string())?;

    if let Some(transaction) = find_mintlayer_htlc_resolution(coin, &spend_info, &payment_transaction_id).await? {
        return Ok(Some(transaction));
    }

    if htlc.refund_key != htlc.spend_key {
        let refund_info = coin
            .api_client()
            .address_info(&htlc.refund_key)
            .await
            .map_err(|error| error.to_string())?;

        if let Some(transaction) = find_mintlayer_htlc_resolution(coin, &refund_info, &payment_transaction_id).await? {
            return Ok(Some(transaction));
        }
    }

    Ok(None)
}

async fn observe_mintlayer_htlc_resolution(
    coin: &MintlayerCoin,
    input: &SearchForSwapTxSpendInput<'_>,
) -> Result<Option<FoundSwapTxSpend>, String> {
    coin.validate_other_pubkey(input.other_pub)
        .map_err(|error| error.to_string())?;

    let (payment_transaction_id, _) = mintlayer_htlc_payment_outpoint(input.tx)?;

    let spend_address = mintlayer_address_from_compressed_public_key(coin.network(), input.other_pub)
        .map_err(|error| error.to_string())?;

    let htlc_key_pair = coin.derive_htlc_key_pair(input.swap_unique_data);

    let refund_address = mintlayer_address_from_compressed_public_key(coin.network(), htlc_key_pair.public_slice())
        .map_err(|error| error.to_string())?;

    let spend_info = coin
        .api_client()
        .address_info(&spend_address)
        .await
        .map_err(|error| error.to_string())?;

    if let Some(transaction) = find_mintlayer_htlc_resolution(coin, &spend_info, &payment_transaction_id).await? {
        return Ok(Some(FoundSwapTxSpend::Spent(transaction.into())));
    }

    let refund_info = coin
        .api_client()
        .address_info(&refund_address)
        .await
        .map_err(|error| error.to_string())?;

    if let Some(transaction) = find_mintlayer_htlc_resolution(coin, &refund_info, &payment_transaction_id).await? {
        return Ok(Some(FoundSwapTxSpend::Refunded(transaction.into())));
    }

    Ok(None)
}

async fn send_mintlayer_htlc_resolution(coin: &MintlayerCoin, transaction: MintlayerTransaction) -> TransactionResult {
    let broadcast_txid = coin
        .send_raw_tx_bytes(&transaction.signed_bytes)
        .compat()
        .await
        .map_err(TransactionErr::Plain)?;

    if broadcast_txid != transaction.transaction_id {
        return Err(TransactionErr::Plain(format!(
            "Mintlayer broadcaster returned transaction ID '{}' but built transaction ID is '{}'",
            broadcast_txid, transaction.transaction_id
        )));
    }

    Ok(transaction.into())
}

fn mintlayer_confirmation_count(inclusion_height: u64, best_height: u64) -> Result<u64, String> {
    if best_height < inclusion_height {
        return Err(format!(
            "Mintlayer best block height {} is below transaction inclusion height {}",
            best_height, inclusion_height
        ));
    }

    Ok(best_height - inclusion_height + 1)
}

async fn mintlayer_transaction_confirmations(
    coin: &MintlayerCoin,
    transaction_id: &str,
) -> Result<Option<u64>, String> {
    let transaction = match coin.api_client().transaction(transaction_id).await {
        Ok(transaction) => transaction,
        Err(error) => return Err(error.to_string()),
    };

    if transaction.block_id.is_empty() {
        return Ok(None);
    }

    let Some(inclusion_height) = coin
        .api_client()
        .block_height_in_main_chain(&transaction.block_id)
        .await
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };

    let best_height = coin
        .api_client()
        .chain_tip()
        .await
        .map_err(|error| error.to_string())?
        .block_height;

    mintlayer_confirmation_count(inclusion_height, best_height).map(Some)
}

#[async_trait]
impl MarketCoinOps for MintlayerCoin {
    fn ticker(&self) -> &str {
        &self.conf.ticker
    }

    fn my_address(&self) -> MmResult<String, MyAddressError> {
        Ok(self.address.clone())
    }

    fn address_from_pubkey(&self, pubkey: &H264Json) -> MmResult<String, AddressFromPubkeyError> {
        mintlayer_address_from_compressed_public_key(self.network(), &pubkey.0)
            .map_err(|error| AddressFromPubkeyError::InternalError(error.to_string()).into())
    }

    async fn get_public_key(&self) -> Result<String, MmError<UnexpectedDerivationMethod>> {
        Ok(hex::encode(self.public_key()))
    }

    fn sign_message_hash(&self, _message: &str) -> Option<[u8; 32]> {
        None
    }

    fn sign_message(&self, _message: &str, _address: Option<HDAddressSelector>) -> SignatureResult<String> {
        MmError::err(SignatureError::InternalError(
            "Mintlayer message signing is not implemented".to_string(),
        ))
    }

    fn verify_message(&self, _signature: &str, _message: &str, _address: &str) -> VerificationResult<bool> {
        MmError::err(VerificationError::InternalError(
            "Mintlayer message verification is not implemented".to_string(),
        ))
    }

    fn my_balance(&self) -> BalanceFut<CoinBalance> {
        let coin = self.clone();
        let future = async move {
            match coin.balance().await {
                Ok(balance) => Ok(balance),
                Err(error) => MmError::err(error),
            }
        };
        Box::new(future.boxed().compat())
    }

    fn platform_coin_balance(&self) -> BalanceFut<BigDecimal> {
        Box::new(self.my_balance().map(|balance| balance.spendable))
    }

    fn platform_ticker(&self) -> &str {
        self.ticker()
    }

    fn send_raw_tx(&self, tx: &str) -> Box<dyn Future<Item = String, Error = String> + Send> {
        let bytes = match hex::decode(tx) {
            Ok(bytes) => bytes,
            Err(error) => {
                return Box::new(futures01::future::err(format!(
                    "Invalid Mintlayer transaction hex: {error}"
                )));
            },
        };
        self.send_raw_tx_bytes(&bytes)
    }

    fn send_raw_tx_bytes(&self, tx: &[u8]) -> Box<dyn Future<Item = String, Error = String> + Send> {
        let tx_id = match canonical_transaction_id_from_signed_bytes(tx) {
            Ok(tx_id) => tx_id,
            Err(error) => return Box::new(futures01::future::err(error.to_string())),
        };
        let tx_hex = hex::encode(tx);
        let coin = self.clone();
        let future = async move {
            let submitted_txid = coin
                .api_client()
                .submit_transaction(&tx_hex, &tx_id)
                .await
                .map_err(|error| error.to_string())?;

            if submitted_txid != tx_id {
                return Err(format!(
                    "Mintlayer HTTPS broadcast returned txid {} but expected {}",
                    submitted_txid, tx_id
                ));
            }

            Ok(tx_id)
        };
        Box::new(future.boxed().compat())
    }

    fn wait_for_confirmations(&self, input: ConfirmPaymentInput) -> Box<dyn Future<Item = (), Error = String> + Send> {
        let coin = self.clone();

        let transaction_id = match canonical_transaction_id_from_signed_bytes(&input.payment_tx) {
            Ok(transaction_id) => transaction_id,
            Err(error) => {
                return Box::new(futures01::future::err(format!(
                    "Failed to decode Mintlayer payment transaction: {}",
                    error
                )));
            },
        };

        let future = async move {
            loop {
                if now_sec() > input.wait_until {
                    return Err(format!(
                        "Waited too long until {} for Mintlayer payment {} to reach {} confirmations",
                        input.wait_until, transaction_id, input.confirmations
                    ));
                }

                match mintlayer_transaction_confirmations(&coin, &transaction_id).await {
                    Ok(Some(confirmations)) if confirmations >= input.confirmations => return Ok(()),
                    Ok(_) => {},
                    Err(error) => {
                        info!(
                            "Waiting for confirmations of Mintlayer transaction {}: {}",
                            transaction_id, error
                        );
                    },
                }

                Timer::sleep(input.check_every as f64).await;
            }
        };

        Box::new(future.boxed().compat())
    }

    async fn wait_for_htlc_tx_spend(&self, args: WaitForHTLCTxSpendArgs<'_>) -> TransactionResult {
        let payment_transaction_id = canonical_transaction_id_from_signed_bytes(args.tx_bytes).map_err(|error| {
            TransactionErr::Plain(format!(
                "Failed to decode Mintlayer HTLC payment transaction: {}",
                error
            ))
        })?;

        loop {
            match observe_mintlayer_htlc_spend_from_payment(self, args.tx_bytes, args.from_block).await {
                Ok(Some(transaction)) => return Ok(transaction.into()),
                Ok(None) => {},
                Err(error) => {
                    info!(
                        "Waiting for Mintlayer HTLC payment {} to be spent: {}",
                        payment_transaction_id, error
                    );
                },
            }

            if now_sec() >= args.wait_until {
                return Err(TransactionErr::Plain(format!(
                    "Waited too long until {} for Mintlayer HTLC payment {} to be spent",
                    args.wait_until, payment_transaction_id
                )));
            }

            Timer::sleep(args.check_every).await;
        }
    }

    fn tx_enum_from_bytes(&self, bytes: &[u8]) -> Result<TransactionEnum, MmError<TxMarshalingErr>> {
        let transaction_id = canonical_transaction_id_from_signed_bytes(bytes).map_err(|error| {
            MmError::new(TxMarshalingErr::InvalidInput(format!(
                "Failed to decode Mintlayer transaction: {}",
                error
            )))
        })?;

        Ok(MintlayerTransaction {
            signed_bytes: bytes.to_vec(),
            transaction_id,
        }
        .into())
    }

    fn current_block(&self) -> Box<dyn Future<Item = u64, Error = String> + Send> {
        let coin = self.clone();
        let future = async move {
            coin.chain_tip()
                .await
                .map(|tip| tip.block_height)
                .map_err(|error| error.to_string())
        };
        Box::new(future.boxed().compat())
    }

    fn display_priv_key(&self) -> Result<String, String> {
        Err("Mintlayer private-key export is disabled".to_string())
    }

    fn min_tx_amount(&self) -> BigDecimal {
        BigDecimal::new(BigInt::from(1), i64::from(MINTLAYER_DECIMALS))
    }

    fn min_trading_vol(&self) -> MmNumber {
        self.min_tx_amount().into()
    }

    fn should_burn_dex_fee(&self) -> bool {
        false
    }

    fn is_trezor(&self) -> bool {
        false
    }
}

#[async_trait]
impl SwapOps for MintlayerCoin {
    async fn send_taker_fee(&self, dex_fee: DexFee, _uuid: &[u8], _expire_at: u64) -> TransactionResult {
        send_mintlayer_taker_fee(self, dex_fee).await
    }

    async fn send_maker_payment(&self, args: SendPaymentArgs<'_>) -> TransactionResult {
        send_mintlayer_swap_payment(self, &args).await
    }

    async fn send_taker_payment(&self, args: SendPaymentArgs<'_>) -> TransactionResult {
        send_mintlayer_swap_payment(self, &args).await
    }

    async fn send_maker_spends_taker_payment(&self, args: SpendPaymentArgs<'_>) -> TransactionResult {
        let transaction = build_mintlayer_htlc_spend(self, &args)
            .await
            .map_err(TransactionErr::Plain)?;
        send_mintlayer_htlc_resolution(self, transaction).await
    }

    async fn send_taker_spends_maker_payment(&self, args: SpendPaymentArgs<'_>) -> TransactionResult {
        let transaction = build_mintlayer_htlc_spend(self, &args)
            .await
            .map_err(TransactionErr::Plain)?;
        send_mintlayer_htlc_resolution(self, transaction).await
    }

    async fn send_taker_refunds_payment(&self, args: RefundPaymentArgs<'_>) -> TransactionResult {
        let transaction = build_mintlayer_htlc_refund(self, &args)
            .await
            .map_err(TransactionErr::Plain)?;
        send_mintlayer_htlc_resolution(self, transaction).await
    }

    async fn send_maker_refunds_payment(&self, args: RefundPaymentArgs<'_>) -> TransactionResult {
        let transaction = build_mintlayer_htlc_refund(self, &args)
            .await
            .map_err(TransactionErr::Plain)?;
        send_mintlayer_htlc_resolution(self, transaction).await
    }

    async fn validate_fee(&self, args: ValidateFeeArgs<'_>) -> ValidatePaymentResult<()> {
        validate_mintlayer_dex_fee(self, args)
            .await
            .map_err(|error| MmError::new(ValidatePaymentError::WrongPaymentTx(error)))
    }

    async fn validate_maker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        let local_htlc_key_pair = self.derive_htlc_key_pair(&input.unique_swap_data);

        verify_mintlayer_swap_payment(
            self,
            &input.payment_tx,
            local_htlc_key_pair.public_slice(),
            &input.other_pub,
            &input.secret_hash,
            &input.amount,
            input.time_lock,
        )
        .map(|_| ())
        .map_err(|error| MmError::new(ValidatePaymentError::WrongPaymentTx(error)))
    }

    async fn validate_taker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        let local_htlc_key_pair = self.derive_htlc_key_pair(&input.unique_swap_data);

        verify_mintlayer_swap_payment(
            self,
            &input.payment_tx,
            local_htlc_key_pair.public_slice(),
            &input.other_pub,
            &input.secret_hash,
            &input.amount,
            input.time_lock,
        )
        .map(|_| ())
        .map_err(|error| MmError::new(ValidatePaymentError::WrongPaymentTx(error)))
    }

    async fn check_if_my_payment_sent(
        &self,
        args: CheckIfMyPaymentSentArgs<'_>,
    ) -> Result<Option<TransactionEnum>, String> {
        self.validate_other_pubkey(args.other_pub)
            .map_err(|error| error.to_string())?;

        let htlc_key_pair = self.derive_htlc_key_pair(args.swap_unique_data);

        let refund_address = mintlayer_address_from_compressed_public_key(self.network(), htlc_key_pair.public_slice())
            .map_err(|error| error.to_string())?;

        let address_info = self
            .api_client()
            .address_info(&refund_address)
            .await
            .map_err(|error| error.to_string())?;

        for candidate_transaction_id in &address_info.transaction_history {
            let transaction = self
                .api_client()
                .transaction(candidate_transaction_id)
                .await
                .map_err(|error| error.to_string())?;

            if transaction.block_id.is_empty() {
                continue;
            }

            let Some(block_height) = self
                .api_client()
                .block_height_in_main_chain(&transaction.block_id)
                .await
                .map_err(|error| error.to_string())?
            else {
                continue;
            };

            if block_height < args.search_from_block {
                continue;
            }

            let canonical_transaction =
                load_mintlayer_chain_transaction(self, &transaction.block_id, candidate_transaction_id).await?;

            if verify_mintlayer_swap_payment(
                self,
                &canonical_transaction.signed_bytes,
                args.other_pub,
                htlc_key_pair.public_slice(),
                args.secret_hash,
                args.amount,
                args.time_lock,
            )
            .is_err()
            {
                continue;
            }

            return Ok(Some(canonical_transaction.into()));
        }

        Ok(None)
    }

    async fn search_for_swap_tx_spend_my(
        &self,
        input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        observe_mintlayer_htlc_resolution(self, &input).await
    }

    async fn search_for_swap_tx_spend_other(
        &self,
        input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        observe_mintlayer_htlc_resolution(self, &input).await
    }

    async fn extract_secret(&self, secret_hash: &[u8], spend_tx: &[u8]) -> Result<[u8; 32], String> {
        use mintlayer_sdk::crypto::types::{DecodeAll, Encode, SignedTransaction, TxInput};
        use std::convert::TryInto;

        let signed = SignedTransaction::decode_all(&mut &spend_tx[..])
            .map_err(|error| format!("Failed to decode Mintlayer HTLC spend transaction: {}", error))?;

        for input in signed.transaction().inputs() {
            let TxInput::Utxo(outpoint) = input else {
                continue;
            };

            let source_id = outpoint.source_id();
            let output_index = outpoint.output_index();

            let secret = match mintlayer_sdk::crypto::extract_htlc_secret(&signed, source_id, output_index) {
                Ok(secret) => secret,
                Err(_) => continue,
            };

            if secret.hash().as_bytes() != secret_hash {
                continue;
            }

            let secret_bytes = secret.encode();

            return secret_bytes
                .try_into()
                .map_err(|_| "Mintlayer HTLC secret must contain exactly 32 bytes".to_owned());
        }

        Err("Mintlayer HTLC spend does not reveal the requested secret".to_owned())
    }

    fn negotiate_swap_contract_addr(
        &self,
        _other_side_address: Option<&[u8]>,
    ) -> Result<Option<BytesJson>, MmError<NegotiateSwapContractAddrErr>> {
        Ok(None)
    }

    fn derive_htlc_key_pair(&self, _swap_unique_data: &[u8]) -> KeyPair {
        self.key_pair
    }

    fn derive_htlc_pubkey(&self, _swap_unique_data: &[u8]) -> [u8; 33] {
        let mut public_key = [0_u8; 33];
        public_key.copy_from_slice(self.public_key());
        public_key
    }

    fn validate_other_pubkey(&self, raw_pubkey: &[u8]) -> MmResult<(), ValidateOtherPubKeyErr> {
        secp256k1::PublicKey::from_slice(raw_pubkey)
            .map(|_| ())
            .map_err(|error| ValidateOtherPubKeyErr::InvalidPubKey(error.to_string()).into())
    }
}

impl WatcherOps for MintlayerCoin {}

#[async_trait]
impl MmCoin for MintlayerCoin {
    fn wallet_only(&self, ctx: &MmArc) -> bool {
        crate::is_wallet_only_ticker(ctx, self.ticker())
    }

    fn spawner(&self) -> WeakSpawner {
        self.abortable_system.weak_spawner()
    }

    fn withdraw(&self, req: WithdrawRequest) -> WithdrawFut {
        let coin = self.clone();
        let future = async move {
            let broadcast = req.broadcast;
            let details = build_mintlayer_withdraw(coin.clone(), req)
                .await
                .map_err(MmError::new)?;

            if broadcast {
                broadcast_mintlayer_transaction_data(&coin, &details.tx)
                    .await
                    .map_err(|error| MmError::new(WithdrawError::Transport(error)))?;
            }

            Ok(details)
        };
        Box::new(future.boxed().compat())
    }

    fn get_raw_transaction(&self, _req: RawTransactionRequest) -> RawTransactionFut<'_> {
        Box::new(futures01::future::err(MmError::new(
            RawTransactionError::NotImplemented {
                coin: self.ticker().to_string(),
            },
        )))
    }

    fn get_tx_hex_by_hash(&self, _tx_hash: Vec<u8>) -> RawTransactionFut<'_> {
        Box::new(futures01::future::err(MmError::new(
            RawTransactionError::NotImplemented {
                coin: self.ticker().to_string(),
            },
        )))
    }

    fn decimals(&self) -> u8 {
        self.decimals()
    }

    fn convert_to_address(&self, _from: &str, _to_address_format: Json) -> Result<String, String> {
        Err("Mintlayer address conversion is not implemented".into())
    }

    fn validate_address(&self, address: &str) -> ValidateAddressResult {
        match validate_mintlayer_address(self.network(), address) {
            Ok(()) => ValidateAddressResult {
                is_valid: true,
                reason: None,
            },
            Err(error) => ValidateAddressResult {
                is_valid: false,
                reason: Some(error.to_string()),
            },
        }
    }

    fn process_history_loop(&self, _ctx: MmArc) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        Box::new(futures01::future::ok(()))
    }

    fn history_sync_status(&self) -> HistorySyncState {
        HistorySyncState::NotEnabled
    }

    fn get_trade_fee(&self) -> Box<dyn Future<Item = TradeFee, Error = String> + Send> {
        let coin = self.clone();

        Box::new(
            async move {
                // Mintlayer legacy trade fee representative amount.
                // The modern sender/receiver fee APIs should be preferred.
                let representative_amount: BigDecimal = "0.5"
                    .parse()
                    .map_err(|error| format!("Invalid Mintlayer representative trade amount: {}", error))?;

                mintlayer_sender_trade_fee(&coin, TradePreimageValue::Exact(representative_amount)).await
            }
            .boxed()
            .compat(),
        )
    }

    async fn get_sender_trade_fee(
        &self,
        value: TradePreimageValue,
        stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        mintlayer_sender_trade_fee_for_stage(self, value, stage)
            .await
            .map_err(|error| MmError::new(TradePreimageError::InternalError(error)))
    }

    fn get_receiver_trade_fee(&self, _stage: FeeApproxStage) -> TradePreimageFut<TradeFee> {
        let coin = self.clone();

        Box::new(
            async move {
                mintlayer_receiver_trade_fee(&coin)
                    .await
                    .map_err(|error| MmError::new(TradePreimageError::InternalError(error)))
            }
            .boxed()
            .compat(),
        )
    }

    async fn get_fee_to_send_taker_fee(
        &self,
        dex_fee_amount: DexFee,
        stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        let fee_rate = self
            .api_client()
            .fee_rate()
            .await
            .map_err(|error| MmError::new(TradePreimageError::InternalError(error.to_string())))?;
        let fee_rate = mintlayer_fee_rate_for_stage(fee_rate, stage)
            .map_err(|error| MmError::new(TradePreimageError::InternalError(error)))?;

        let (_transaction, fee_atoms) = build_mintlayer_taker_fee_with_fee_rate(self, dex_fee_amount, fee_rate)
            .await
            .map_err(|error| MmError::new(TradePreimageError::InternalError(error)))?
            .ok_or_else(|| {
                MmError::new(TradePreimageError::InternalError(
                    "Mintlayer taker-fee network fee cannot be calculated for DexFee::NoFee".into(),
                ))
            })?;

        Ok(TradeFee {
            coin: self.ticker().to_owned(),
            amount: mintlayer_decimal_from_atoms(fee_atoms).into(),
            paid_from_trading_vol: false,
        })
    }

    fn required_confirmations(&self) -> u64 {
        self.required_confirmations()
    }

    fn requires_notarization(&self) -> bool {
        false
    }

    fn set_required_confirmations(&self, confirmations: u64) {
        if confirmations > 0 {
            self.required_confirmations.store(confirmations, Ordering::Relaxed);
        }
    }

    fn set_requires_notarization(&self, _requires_nota: bool) {}

    fn swap_contract_address(&self) -> Option<BytesJson> {
        None
    }

    fn fallback_swap_contract(&self) -> Option<BytesJson> {
        None
    }

    fn mature_confirmations(&self) -> Option<u32> {
        None
    }

    fn coin_protocol_info(&self, _amount_to_receive: Option<MmNumber>) -> Vec<u8> {
        Vec::new()
    }

    fn is_coin_protocol_supported(
        &self,
        _info: &Option<Vec<u8>>,
        _amount_to_send: Option<MmNumber>,
        _locktime: u64,
        _is_maker: bool,
    ) -> bool {
        true
    }

    fn on_disabled(&self) -> Result<(), AbortedError> {
        self.abortable_system.abort_all()
    }

    fn on_token_deactivated(&self, _ticker: &str) {}
}

fn coin_balance_from_address_info(address_info: MintlayerAddressInfo) -> Result<CoinBalance, BalanceError> {
    let spendable = address_info
        .coin_balance
        .to_big_decimal()
        .map_err(|error| BalanceError::InvalidResponse(error.to_string()))?;
    let unspendable = address_info
        .locked_coin_balance
        .to_big_decimal()
        .map_err(|error| BalanceError::InvalidResponse(error.to_string()))?;

    Ok(CoinBalance { spendable, unspendable })
}

fn build_mintlayer_identity(
    network: MintlayerNetwork,
    priv_key_build_policy: PrivKeyBuildPolicy,
) -> Result<(KeyPair, String, DerivationMethodResponse), MintlayerCoinBuildError> {
    let (secret, derivation_method) = match priv_key_build_policy {
        PrivKeyBuildPolicy::IguanaPrivKey(secret) => (secret, DerivationMethodResponse::Iguana),
        PrivKeyBuildPolicy::GlobalHDAccount(global_hd_account) => {
            let derivation_path = mintlayer_derivation_path(network, 0, Bip44Chain::External, 0)
                .map_err(|error| MintlayerCoinBuildError::KeyDerivation(error.to_string()))?;
            let secret = global_hd_account
                .derive_secp256k1_secret(&derivation_path)
                .map_err(|error| MintlayerCoinBuildError::KeyDerivation(error.to_string()))?;

            (secret, DerivationMethodResponse::HDWallet(derivation_path.to_string()))
        },
        PrivKeyBuildPolicy::Trezor => {
            return Err(MintlayerCoinBuildError::UnsupportedPrivKeyPolicy { policy: "Trezor" })
        },
        PrivKeyBuildPolicy::WalletConnect { .. } => {
            return Err(MintlayerCoinBuildError::UnsupportedPrivKeyPolicy {
                policy: "WalletConnect",
            })
        },
    };

    let key_pair = key_pair_from_secret(&secret.take())
        .map_err(|error| MintlayerCoinBuildError::KeyDerivation(error.to_string()))?;
    let address = mintlayer_address_from_compressed_public_key(network, key_pair.public_slice())
        .map_err(|error| MintlayerCoinBuildError::AddressDerivation(error.to_string()))?;

    Ok((key_pair, address, derivation_method))
}

fn is_valid_genesis_block_id(genesis_block_id: &str) -> bool {
    genesis_block_id.len() == 64
        && hex::decode(genesis_block_id)
            .map(|bytes| bytes.len() == 32)
            .unwrap_or(false)
}

fn validate_genesis_block_id(genesis_block_id: &str) -> Result<(), MintlayerCoinBuildError> {
    if !is_valid_genesis_block_id(genesis_block_id) {
        return Err(MintlayerCoinBuildError::InvalidGenesisBlockId);
    }

    Ok(())
}

fn validate_api_genesis_block_id(expected: &str, actual: &str) -> Result<(), MintlayerNetworkValidationError> {
    if !is_valid_genesis_block_id(actual) {
        return Err(MintlayerNetworkValidationError::InvalidGenesisBlockId {
            actual: actual.to_owned(),
        });
    }

    if !expected.eq_ignore_ascii_case(actual) {
        return Err(MintlayerNetworkValidationError::UnexpectedGenesisBlockId {
            expected: expected.to_owned(),
            actual: actual.to_owned(),
        });
    }

    Ok(())
}

fn validate_node_rpc_config(
    config: &MintlayerNodeClientConfig,
) -> Result<MintlayerNodeClientConfig, MintlayerCoinBuildError> {
    let url = Url::parse(&config.rpc_url).map_err(|_| MintlayerCoinBuildError::InvalidNodeRpcUrl)?;

    if url.username() != "" || url.password().is_some() {
        return Err(MintlayerCoinBuildError::NodeRpcUrlCredentialsNotAllowed);
    }

    if url.scheme() != "http" {
        return Err(MintlayerCoinBuildError::UnsupportedNodeRpcUrlScheme);
    }

    let is_loopback = match url.host_str() {
        Some("localhost") => true,
        Some(host) => host
            .parse::<std::net::IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false),
        None => false,
    };
    if !is_loopback {
        return Err(MintlayerCoinBuildError::RemoteNodeRpcUrlNotAllowed);
    }

    Ok(MintlayerNodeClientConfig {
        rpc_url: url.to_string().trim_end_matches('/').to_owned(),
        rpc_cookie_file: config.rpc_cookie_file.clone(),
    })
}

fn validate_api_urls(configured_urls: &[String]) -> Result<Vec<Url>, MintlayerCoinBuildError> {
    if configured_urls.is_empty() {
        return Err(MintlayerCoinBuildError::MissingApiUrls);
    }

    let mut normalized_urls = Vec::with_capacity(configured_urls.len());
    let mut unique_urls = HashSet::with_capacity(configured_urls.len());

    for (index, configured_url) in configured_urls.iter().enumerate() {
        let mut url = Url::parse(configured_url).map_err(|_| MintlayerCoinBuildError::InvalidApiUrl { index })?;

        if url.username() != "" || url.password().is_some() {
            return Err(MintlayerCoinBuildError::ApiUrlCredentialsNotAllowed { index });
        }

        if url.query().is_some() || url.fragment().is_some() {
            return Err(MintlayerCoinBuildError::InvalidApiUrl { index });
        }

        match url.scheme() {
            "https" => {},
            "http" if is_loopback(&url) => {},
            "http" => return Err(MintlayerCoinBuildError::InsecureRemoteApiUrl { index }),
            _ => return Err(MintlayerCoinBuildError::UnsupportedApiUrlScheme { index }),
        }

        if url.path() == "/" {
            url.set_path("");
        }

        let normalized = url.as_str().to_owned();
        if !unique_urls.insert(normalized) {
            return Err(MintlayerCoinBuildError::DuplicateApiUrl { index });
        }

        normalized_urls.push(url);
    }

    Ok(normalized_urls)
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(domain)) => domain.eq_ignore_ascii_case("localhost"),
        Some(Host::Ipv4(address)) => address.is_loopback(),
        Some(Host::Ipv6(address)) => address.is_loopback(),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mintlayer::MintlayerApiClientConfig;
    use crypto::{CryptoCtx, KeyPairPolicy};
    use futures::compat::Future01CompatExt;
    use mm2_core::mm_ctx::{MmArc, MmCtxBuilder};
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::{Duration, Instant};

    const WITHDRAW_RECIPIENT: &str = "mtc1qxlpcx3rzm4nlqw2a2atsw9gtuv6lvaeasdrqxjz";
    const WITHDRAW_FEE_RATE_ATOMS_PER_KB: u128 = 100_000_000_000;

    fn decimal(value: &str) -> BigDecimal {
        value.parse().unwrap()
    }

    fn withdraw_request(to: &str) -> WithdrawRequest {
        WithdrawRequest {
            coin: "ML".into(),
            from: None,
            to: to.into(),
            amount: decimal("0.5"),
            max: false,
            fee: None,
            memo: None,
            ibc_source_channel: None,
            broadcast: false,
            expiration_seconds: None,
        }
    }

    fn bind_withdraw_mock_api() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let api_url = format!("http://{}", listener.local_addr().unwrap());
        (listener, api_url)
    }

    fn read_request_path(stream: &mut TcpStream) -> String {
        stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];

        loop {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if request.windows(4).any(|window| window == b"\r\n\r\n") {
                break;
            }
            assert!(request.len() <= 16 * 1024, "mock HTTP request too large");
        }

        let request = String::from_utf8(request).unwrap();
        request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .expect("HTTP request path")
            .to_owned()
    }

    fn respond_json(stream: &mut TcpStream, body: &str) {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
        stream.flush().unwrap();
    }

    fn spawn_withdraw_mock_api(listener: TcpListener, sender: String) -> thread::JoinHandle<Vec<String>> {
        thread::spawn(move || {
            let spendable_path = format!("/api/v2/address/{sender}/spendable-utxos");
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut requested_paths = Vec::new();

            while requested_paths.len() < 3 && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let path = read_request_path(&mut stream);
                        let body = match path.as_str() {
                            value if value == spendable_path => json!([{
                                "outpoint": {
                                    "index": 0,
                                    "source_id": "11".repeat(32),
                                    "source_type": "Transaction"
                                },
                                "utxo": {
                                    "destination": sender.clone(),
                                    "type": "Transfer",
                                    "value": {
                                        "amount": { "atoms": "100000000000", "decimal": "1" },
                                        "type": "Coin"
                                    }
                                }
                            }])
                            .to_string(),
                            "/api/v2/feerate" => format!("\"{WITHDRAW_FEE_RATE_ATOMS_PER_KB}\""),
                            "/api/v2/chain/tip" => json!({
                                "block_height": 700000,
                                "block_id": "22".repeat(32)
                            })
                            .to_string(),
                            other => panic!("unexpected mock API request: {}", other),
                        };

                        respond_json(&mut stream, &body);
                        requested_paths.push(path);
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    },
                    Err(error) => panic!("mock API accept failed: {}", error),
                }
            }

            assert_eq!(requested_paths.len(), 3, "mock API did not receive all requests");
            requested_paths
        })
    }

    fn spawn_split_withdraw_mock_api(listener: TcpListener, sender: String) -> thread::JoinHandle<Vec<String>> {
        thread::spawn(move || {
            let spendable_path = format!("/api/v2/address/{sender}/spendable-utxos");
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut requested_paths = Vec::new();
            while requested_paths.len() < 3 && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let path = read_request_path(&mut stream);
                        let body = match path.as_str() {
                            value if value == spendable_path => json!([
                                {
                                    "outpoint": { "index": 0, "source_id": "11".repeat(32), "source_type": "Transaction" },
                                    "utxo": { "destination": sender.clone(), "type": "Transfer",
                                              "value": { "amount": { "atoms": "60000000000", "decimal": "0.6" }, "type": "Coin" } }
                                },
                                {
                                    "outpoint": { "index": 0, "source_id": "33".repeat(32), "source_type": "Transaction" },
                                    "utxo": { "destination": sender.clone(), "type": "Transfer",
                                              "value": { "amount": { "atoms": "60000000000", "decimal": "0.6" }, "type": "Coin" } }
                                }
                            ]).to_string(),
                            "/api/v2/feerate" => format!("\"{WITHDRAW_FEE_RATE_ATOMS_PER_KB}\""),
                            "/api/v2/chain/tip" => json!({
                                "block_height": 700000,
                                "block_id": "22".repeat(32)
                            }).to_string(),
                            other => panic!("unexpected split mock API request: {}", other),
                        };
                        respond_json(&mut stream, &body);
                        requested_paths.push(path);
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    },
                    Err(error) => panic!("split mock API accept failed: {}", error),
                }
            }
            assert_eq!(requested_paths.len(), 3, "split mock API did not receive all requests");
            requested_paths
        })
    }

    fn spawn_resolution_mock_api(listener: TcpListener) -> thread::JoinHandle<Vec<String>> {
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut requested_paths = Vec::new();

            while requested_paths.len() < 2 && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let path = read_request_path(&mut stream);

                        let body = match path.as_str() {
                            "/api/v2/feerate" => {
                                format!("\"{WITHDRAW_FEE_RATE_ATOMS_PER_KB}\"")
                            },
                            "/api/v2/chain/tip" => json!({
                                "block_height": 700000,
                                "block_id": "22".repeat(32)
                            })
                            .to_string(),
                            other => panic!("unexpected resolution mock API request: {}", other),
                        };

                        respond_json(&mut stream, &body);
                        requested_paths.push(path);
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    },
                    Err(error) => {
                        panic!("resolution mock API accept failed: {}", error)
                    },
                }
            }

            assert_eq!(
                requested_paths.len(),
                2,
                "resolution mock API did not receive all requests"
            );

            assert!(
                requested_paths.iter().any(|path| path == "/api/v2/feerate"),
                "resolution mock API did not receive fee-rate request"
            );

            assert!(
                requested_paths.iter().any(|path| path == "/api/v2/chain/tip"),
                "resolution mock API did not receive chain-tip request"
            );

            requested_paths
        })
    }

    #[test]
    fn mmcoin_withdraw_builds_signed_transaction_from_loopback_api() {
        let (listener, api_url) = bind_withdraw_mock_api();
        let coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![api_url]),
            iguana_policy(),
        )
        .unwrap();
        let sender = coin.address().to_owned();
        let server = spawn_withdraw_mock_api(listener, sender.clone());

        let details = common::block_on_f01(MmCoin::withdraw(&coin, withdraw_request(WITHDRAW_RECIPIENT))).unwrap();
        let requested_paths = server.join().unwrap();

        assert_eq!(
            requested_paths,
            vec![
                format!("/api/v2/address/{sender}/spendable-utxos"),
                "/api/v2/feerate".to_owned(),
                "/api/v2/chain/tip".to_owned(),
            ]
        );
        assert_eq!(details.from, vec![sender]);
        assert_eq!(details.to, vec![WITHDRAW_RECIPIENT.to_owned()]);
        assert_eq!(details.total_amount, decimal("1"));
        assert_eq!(details.spent_by_me, decimal("1"));
        assert_eq!(details.received_by_me, decimal("0.296"));
        assert_eq!(details.my_balance_change, decimal("-0.704"));
        assert_eq!(details.block_height, 0);
        assert!(details.timestamp > 0);
        assert_eq!(details.coin, "ML");
        assert_eq!(details.internal_id.0.len(), 32);

        match &details.tx {
            TransactionData::Signed { tx_hex, tx_hash } => {
                assert_eq!(tx_hex.0.len(), 204);
                assert_eq!(hex::decode(tx_hash).unwrap().len(), 32);
            },
            other => panic!("expected signed Mintlayer transaction, got {:?}", other),
        }

        match details.fee_details.as_ref() {
            Some(TxFeeDetails::Utxo(fee)) => {
                assert_eq!(fee.coin.as_deref(), Some("ML"));
                assert_eq!(fee.amount, decimal("0.204"));
            },
            other => panic!("expected Mintlayer UTXO fee details, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn mmcoin_withdraw_broadcasts_exact_signed_transaction_when_requested() {
        let (api_listener, api_url) = bind_withdraw_mock_api();

        let activation = request_with_urls(vec![api_url]);
        let coin = MintlayerCoin::new(&test_ctx(), valid_conf(), activation, iguana_policy()).unwrap();

        assert_eq!(coin.node_rpc_url(), None);
        assert!(coin.node_client().unwrap().is_none());

        let sender = coin.address().to_owned();
        let api_server = spawn_withdraw_and_transaction_capture_api(api_listener, sender);

        let mut request = withdraw_request(WITHDRAW_RECIPIENT);
        request.broadcast = true;

        let details = MmCoin::withdraw(&coin, request).compat().await.unwrap();
        let submitted_bytes = api_server.join().unwrap();

        let (returned_bytes, returned_txid) = match &details.tx {
            TransactionData::Signed { tx_hex, tx_hash } => (tx_hex.0.clone(), tx_hash.clone()),
            other => panic!("expected signed Mintlayer transaction, got {:?}", other),
        };

        assert_eq!(submitted_bytes, returned_bytes);
        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&returned_bytes).unwrap(),
            returned_txid
        );

        assert_eq!(details.to, vec![WITHDRAW_RECIPIENT.to_owned()]);

        match details.fee_details.as_ref() {
            Some(TxFeeDetails::Utxo(fee)) => {
                assert_eq!(fee.coin.as_deref(), Some("ML"));
            },
            other => panic!("expected Mintlayer UTXO fee details, got {:?}", other),
        }
    }
    #[test]
    fn mmcoin_withdraw_rejects_unsupported_requests_before_network() {
        let coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec!["http://127.0.0.1:9/api/v2".into()]),
            iguana_policy(),
        )
        .unwrap();

        let mut max = withdraw_request(WITHDRAW_RECIPIENT);
        max.max = true;
        let max_error = common::block_on_f01(MmCoin::withdraw(&coin, max)).unwrap_err();
        assert!(format!("{max_error:?}").contains("max mode is not implemented"));

        let invalid_address = withdraw_request("not-a-mintlayer-address");
        let address_error = common::block_on_f01(MmCoin::withdraw(&coin, invalid_address)).unwrap_err();
        assert!(format!("{address_error:?}").contains("InvalidAddress"));
    }

    const TEST_MNEMONIC: &str =
        "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";

    fn valid_conf() -> MintlayerCoinConf {
        MintlayerCoinConf {
            ticker: "ML".into(),
            network: MintlayerNetwork::Mainnet,
            decimals: MINTLAYER_DECIMALS,
            required_confirmations: 2,
            genesis_block_id: "a".repeat(64),
        }
    }

    fn request_with_urls(api_urls: Vec<String>) -> MintlayerActivationRequest {
        MintlayerActivationRequest {
            tx_history: false,
            required_confirmations: None,
            client_conf: MintlayerApiClientConfig { api_urls },
            node_conf: None,
        }
    }

    fn iguana_policy() -> PrivKeyBuildPolicy {
        PrivKeyBuildPolicy::IguanaPrivKey([1_u8; 32].into())
    }

    fn test_ctx() -> MmArc {
        MmCtxBuilder::default().into_mm_arc()
    }

    #[tokio::test]
    async fn swap_payment_builder_produces_native_htlc_without_broadcast() {
        use mintlayer_sdk::crypto::types::TxOutput;

        const SECRET_HASH: [u8; 20] = [
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x10, 0x20,
            0x30, 0x40,
        ];
        const TIME_LOCK: u64 = 1_800_000_000;

        let (listener, api_url) = bind_withdraw_mock_api();
        let coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![api_url]),
            iguana_policy(),
        )
        .unwrap();

        let sender = coin.address().to_owned();
        let api_server = spawn_withdraw_mock_api(listener, sender.clone());

        let other_secret = [2_u8; 32];
        let other_key_pair = key_pair_from_secret(&other_secret).unwrap();
        let other_pubkey = other_key_pair.public_slice().to_vec();

        let swap_contract_address = None;
        let payment_instructions = None;
        let amount: BigDecimal = "0.5".parse().unwrap();
        let swap_unique_data = b"mintlayer-swap-payment-builder-test";

        let args = SendPaymentArgs {
            time_lock_duration: 3600,
            time_lock: TIME_LOCK,
            other_pubkey: &other_pubkey,
            secret_hash: &SECRET_HASH,
            amount,
            swap_contract_address: &swap_contract_address,
            swap_unique_data,
            payment_instructions: &payment_instructions,
            watcher_reward: None,
            wait_for_confirmation_until: 0,
        };

        let transaction = build_mintlayer_swap_payment(&coin, &args).await.unwrap();

        api_server.join().unwrap();

        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&transaction.signed_bytes).unwrap(),
            transaction.transaction_id
        );

        let decoded = mintlayer_sdk::crypto::decode_transaction_lenient(&transaction.signed_bytes).unwrap();

        assert!(
            matches!(decoded.outputs().first(), Some(TxOutput::Htlc(_, _))),
            "first swap-payment output must be a native Mintlayer HTLC"
        );

        let expected_spend_address =
            mintlayer_address_from_compressed_public_key(coin.network(), &other_pubkey).unwrap();

        let local_htlc_key = coin.derive_htlc_key_pair(swap_unique_data);
        let expected_refund_address =
            mintlayer_address_from_compressed_public_key(coin.network(), local_htlc_key.public_slice()).unwrap();

        let expected_output = build_mintlayer_htlc_output(
            50_000_000_000,
            &SECRET_HASH,
            &expected_spend_address,
            &expected_refund_address,
            TIME_LOCK,
            mintlayer_sdk_network(coin.network()),
        )
        .unwrap();

        assert_eq!(decoded.outputs().first(), Some(&expected_output));
    }

    #[test]
    fn mintlayer_confirmation_count_counts_from_inclusion_block() {
        assert_eq!(mintlayer_confirmation_count(700_000, 700_000).unwrap(), 1);
        assert_eq!(mintlayer_confirmation_count(700_000, 700_001).unwrap(), 2);
        assert_eq!(mintlayer_confirmation_count(700_000, 700_005).unwrap(), 6);

        let error = mintlayer_confirmation_count(700_000, 699_999)
            .expect_err("best block below inclusion height must be rejected");

        assert!(
            error.contains("below transaction inclusion height"),
            "unexpected Mintlayer confirmation count error: {}",
            error
        );
    }

    #[tokio::test]
    async fn tx_enum_from_bytes_roundtrips_and_rejects_invalid_bytes() {
        const SECRET_HASH: [u8; 20] = [
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x10, 0x20,
            0x30, 0x40,
        ];
        const TIME_LOCK: u64 = 1_800_000_000;

        let (api_listener, api_url) = bind_withdraw_mock_api();

        let coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![api_url]),
            iguana_policy(),
        )
        .unwrap();

        let sender = coin.address().to_owned();
        let api_server = spawn_withdraw_mock_api(api_listener, sender);

        let other_secret = [2_u8; 32];
        let other_key_pair = key_pair_from_secret(&other_secret).unwrap();
        let other_pubkey = other_key_pair.public_slice().to_vec();

        let swap_contract_address = None;
        let payment_instructions = None;
        let amount: BigDecimal = "0.5".parse().unwrap();
        let swap_unique_data = b"mintlayer-vd5b-tx-enum";

        let args = SendPaymentArgs {
            time_lock_duration: 3600,
            time_lock: TIME_LOCK,
            other_pubkey: &other_pubkey,
            secret_hash: &SECRET_HASH,
            amount,
            swap_contract_address: &swap_contract_address,
            swap_unique_data,
            payment_instructions: &payment_instructions,
            watcher_reward: None,
            wait_for_confirmation_until: 0,
        };

        let payment = build_mintlayer_swap_payment(&coin, &args).await.unwrap();

        api_server.join().unwrap();

        let expected_txid = canonical_transaction_id_from_signed_bytes(&payment.signed_bytes).unwrap();

        let decoded = coin.tx_enum_from_bytes(&payment.signed_bytes).unwrap();

        let decoded = match decoded {
            TransactionEnum::MintlayerTransaction(transaction) => transaction,
            other => panic!(
                "expected Mintlayer transaction from tx_enum_from_bytes, got {:?}",
                other
            ),
        };

        assert_eq!(decoded.signed_bytes, payment.signed_bytes);
        assert_eq!(decoded.transaction_id, payment.transaction_id);
        assert_eq!(decoded.transaction_id, expected_txid);

        let invalid = [0xde, 0xad, 0xbe, 0xef];

        let error = coin
            .tx_enum_from_bytes(&invalid)
            .expect_err("invalid Mintlayer transaction bytes must be rejected");

        assert!(
            matches!(error.get_inner(), TxMarshalingErr::InvalidInput(_)),
            "invalid Mintlayer bytes must map to TxMarshalingErr::InvalidInput: {:?}",
            error
        );
    }

    #[tokio::test]
    async fn swap_payment_validation_accepts_other_wallet() {
        const SECRET_HASH: [u8; 20] = [
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x10, 0x20,
            0x30, 0x40,
        ];
        const TIME_LOCK: u64 = 1_800_000_000;

        // Maker A uses the existing test identity [1; 32].
        let (api_listener, api_url) = bind_withdraw_mock_api();
        let maker_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![api_url]),
            iguana_policy(),
        )
        .unwrap();

        // Taker B is deliberately a different local Mintlayer wallet: [2; 32].
        let taker_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![concat!("https", "://api.example").into()]),
            PrivKeyBuildPolicy::IguanaPrivKey([2_u8; 32].into()),
        )
        .unwrap();

        assert_ne!(
            maker_coin.public_key(),
            taker_coin.public_key(),
            "AUDIT-B1 requires two distinct local wallet identities"
        );

        let sender = maker_coin.address().to_owned();
        let api_server = spawn_withdraw_mock_api(api_listener, sender);

        let amount: BigDecimal = "0.5".parse().unwrap();
        let swap_unique_data = b"mintlayer-audit-b1-cross-wallet";
        let swap_contract_address = None;
        let payment_instructions = None;

        // A creates a payment spendable by B and refundable by A.
        let args = SendPaymentArgs {
            time_lock_duration: 3600,
            time_lock: TIME_LOCK,
            other_pubkey: taker_coin.public_key(),
            secret_hash: &SECRET_HASH,
            amount: amount.clone(),
            swap_contract_address: &swap_contract_address,
            swap_unique_data,
            payment_instructions: &payment_instructions,
            watcher_reward: None,
            wait_for_confirmation_until: 0,
        };

        let payment = build_mintlayer_swap_payment(&maker_coin, &args).await.unwrap();
        api_server.join().unwrap();

        // KDF validates on B and passes A as `other_pub`.
        // Correct validation must therefore accept:
        // spend_key = B (local validator), refund_key = A (`other_pub`).
        let taker_htlc_key_pair = taker_coin.derive_htlc_key_pair(swap_unique_data);
        let maker_htlc_key_pair = maker_coin.derive_htlc_key_pair(swap_unique_data);

        let verified_txid = verify_mintlayer_swap_payment(
            &taker_coin,
            &payment.signed_bytes,
            taker_htlc_key_pair.public_slice(),
            maker_htlc_key_pair.public_slice(),
            &SECRET_HASH,
            &amount,
            TIME_LOCK,
        )
        .expect("AUDIT-B1: wallet B must accept maker A's correctly constructed Mintlayer HTLC");

        assert_eq!(verified_txid, payment.transaction_id);
    }

    #[tokio::test]
    async fn swap_payment_verifier_accepts_valid_and_rejects_mismatches() {
        const SECRET_HASH: [u8; 20] = [
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x10, 0x20,
            0x30, 0x40,
        ];
        const TIME_LOCK: u64 = 1_800_000_000;

        let (api_listener, api_url) = bind_withdraw_mock_api();

        // Validator/recipient B uses the standard test identity [1; 32].
        let coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![concat!("https", "://api.example").into()]),
            iguana_policy(),
        )
        .unwrap();

        // Sender A uses a distinct identity [2; 32] and owns the mocked UTXO.
        let other_secret = [2_u8; 32];
        let sender_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![api_url]),
            PrivKeyBuildPolicy::IguanaPrivKey(other_secret.into()),
        )
        .unwrap();

        let sender = sender_coin.address().to_owned();
        let api_server = spawn_withdraw_mock_api(api_listener, sender);

        let swap_contract_address = None;
        let payment_instructions = None;
        let amount: BigDecimal = "0.5".parse().unwrap();
        let swap_unique_data = b"mintlayer-payment-verifier";

        let args = SendPaymentArgs {
            time_lock_duration: 3600,
            time_lock: TIME_LOCK,
            other_pubkey: coin.public_key(),
            secret_hash: &SECRET_HASH,
            amount: amount.clone(),
            swap_contract_address: &swap_contract_address,
            swap_unique_data,
            payment_instructions: &payment_instructions,
            watcher_reward: None,
            wait_for_confirmation_until: 0,
        };

        let payment = build_mintlayer_swap_payment(&sender_coin, &args).await.unwrap();

        api_server.join().unwrap();

        let spend_htlc_key_pair = coin.derive_htlc_key_pair(swap_unique_data);
        let refund_htlc_key_pair = sender_coin.derive_htlc_key_pair(swap_unique_data);

        let verified_txid = verify_mintlayer_swap_payment(
            &coin,
            &payment.signed_bytes,
            spend_htlc_key_pair.public_slice(),
            refund_htlc_key_pair.public_slice(),
            &SECRET_HASH,
            &amount,
            TIME_LOCK,
        )
        .unwrap();

        assert_eq!(verified_txid, payment.transaction_id);

        let wrong_amount: BigDecimal = "0.6".parse().unwrap();

        assert!(
            verify_mintlayer_swap_payment(
                &coin,
                &payment.signed_bytes,
                spend_htlc_key_pair.public_slice(),
                refund_htlc_key_pair.public_slice(),
                &SECRET_HASH,
                &wrong_amount,
                TIME_LOCK,
            )
            .is_err(),
            "Mintlayer payment verifier must reject a wrong amount"
        );

        let wrong_secret_hash = [0xaa_u8; 20];

        assert!(
            verify_mintlayer_swap_payment(
                &coin,
                &payment.signed_bytes,
                spend_htlc_key_pair.public_slice(),
                refund_htlc_key_pair.public_slice(),
                &wrong_secret_hash,
                &amount,
                TIME_LOCK,
            )
            .is_err(),
            "Mintlayer payment verifier must reject a wrong secret hash"
        );

        assert!(
            verify_mintlayer_swap_payment(
                &coin,
                &payment.signed_bytes,
                spend_htlc_key_pair.public_slice(),
                refund_htlc_key_pair.public_slice(),
                &SECRET_HASH,
                &amount,
                TIME_LOCK + 1,
            )
            .is_err(),
            "Mintlayer payment verifier must reject a wrong timelock"
        );
    }

    #[tokio::test]
    async fn maker_payment_swapops_builds_broadcasts_and_returns_native_htlc() {
        use mintlayer_sdk::crypto::types::TxOutput;

        const SECRET_HASH: [u8; 20] = [
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x10, 0x20,
            0x30, 0x40,
        ];
        const TIME_LOCK: u64 = 1_800_000_000;

        let (api_listener, api_url) = bind_withdraw_mock_api();
        let request = request_with_urls(vec![api_url]);
        let coin = MintlayerCoin::new(&test_ctx(), valid_conf(), request, iguana_policy()).unwrap();

        assert_eq!(coin.node_rpc_url(), None);
        assert!(coin.node_client().unwrap().is_none());

        let sender = coin.address().to_owned();
        let api_server = spawn_withdraw_and_transaction_capture_api(api_listener, sender);

        let other_secret = [2_u8; 32];
        let other_key_pair = key_pair_from_secret(&other_secret).unwrap();
        let other_pubkey = other_key_pair.public_slice().to_vec();

        let swap_contract_address = None;
        let payment_instructions = None;
        let amount: BigDecimal = "0.5".parse().unwrap();
        let swap_unique_data = b"mintlayer-maker-payment-loopback";

        let args = SendPaymentArgs {
            time_lock_duration: 3600,
            time_lock: TIME_LOCK,
            other_pubkey: &other_pubkey,
            secret_hash: &SECRET_HASH,
            amount,
            swap_contract_address: &swap_contract_address,
            swap_unique_data,
            payment_instructions: &payment_instructions,
            watcher_reward: None,
            wait_for_confirmation_until: 0,
        };

        let result = SwapOps::send_maker_payment(&coin, args).await.unwrap();
        let submitted_bytes = api_server.join().unwrap();

        let transaction = match result {
            TransactionEnum::MintlayerTransaction(transaction) => transaction,
            other => panic!("expected Mintlayer transaction from maker payment, got {:?}", other),
        };

        assert_eq!(transaction.signed_bytes, submitted_bytes);
        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&transaction.signed_bytes).unwrap(),
            transaction.transaction_id
        );

        let decoded = mintlayer_sdk::crypto::decode_transaction_lenient(&transaction.signed_bytes).unwrap();

        assert!(
            matches!(decoded.outputs().first(), Some(TxOutput::Htlc(_, _))),
            "maker payment must return a native Mintlayer HTLC transaction"
        );

        let expected_spend_address =
            mintlayer_address_from_compressed_public_key(coin.network(), &other_pubkey).unwrap();

        let local_htlc_key = coin.derive_htlc_key_pair(swap_unique_data);
        let expected_refund_address =
            mintlayer_address_from_compressed_public_key(coin.network(), local_htlc_key.public_slice()).unwrap();

        let expected_output = build_mintlayer_htlc_output(
            50_000_000_000,
            &SECRET_HASH,
            &expected_spend_address,
            &expected_refund_address,
            TIME_LOCK,
            mintlayer_sdk_network(coin.network()),
        )
        .unwrap();

        assert_eq!(decoded.outputs().first(), Some(&expected_output));
    }
    #[tokio::test]
    async fn taker_payment_swapops_builds_broadcasts_and_returns_native_htlc() {
        use mintlayer_sdk::crypto::types::TxOutput;

        const SECRET_HASH: [u8; 20] = [
            0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x4b, 0x4c, 0x4d, 0x4e, 0x4f, 0x50, 0x51, 0x52,
            0x53, 0x54,
        ];
        const TIME_LOCK: u64 = 1_800_003_600;

        let (api_listener, api_url) = bind_withdraw_mock_api();
        let request = request_with_urls(vec![api_url]);
        let coin = MintlayerCoin::new(&test_ctx(), valid_conf(), request, iguana_policy()).unwrap();

        assert_eq!(coin.node_rpc_url(), None);
        assert!(coin.node_client().unwrap().is_none());

        let sender = coin.address().to_owned();
        let api_server = spawn_withdraw_and_transaction_capture_api(api_listener, sender);

        let other_secret = [3_u8; 32];
        let other_key_pair = key_pair_from_secret(&other_secret).unwrap();
        let other_pubkey = other_key_pair.public_slice().to_vec();

        let swap_contract_address = None;
        let payment_instructions = None;
        let amount: BigDecimal = "0.5".parse().unwrap();
        let swap_unique_data = b"mintlayer-taker-payment-loopback";

        let args = SendPaymentArgs {
            time_lock_duration: 7200,
            time_lock: TIME_LOCK,
            other_pubkey: &other_pubkey,
            secret_hash: &SECRET_HASH,
            amount,
            swap_contract_address: &swap_contract_address,
            swap_unique_data,
            payment_instructions: &payment_instructions,
            watcher_reward: None,
            wait_for_confirmation_until: 0,
        };

        let result = SwapOps::send_taker_payment(&coin, args).await.unwrap();
        let submitted_bytes = api_server.join().unwrap();

        let transaction = match result {
            TransactionEnum::MintlayerTransaction(transaction) => transaction,
            other => panic!("expected Mintlayer transaction from taker payment, got {:?}", other),
        };

        assert_eq!(transaction.signed_bytes, submitted_bytes);
        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&transaction.signed_bytes).unwrap(),
            transaction.transaction_id
        );

        let decoded = mintlayer_sdk::crypto::decode_transaction_lenient(&transaction.signed_bytes).unwrap();

        assert!(
            matches!(decoded.outputs().first(), Some(TxOutput::Htlc(_, _))),
            "taker payment must return a native Mintlayer HTLC transaction"
        );

        let expected_spend_address =
            mintlayer_address_from_compressed_public_key(coin.network(), &other_pubkey).unwrap();

        let local_htlc_key = coin.derive_htlc_key_pair(swap_unique_data);
        let expected_refund_address =
            mintlayer_address_from_compressed_public_key(coin.network(), local_htlc_key.public_slice()).unwrap();

        let expected_output = build_mintlayer_htlc_output(
            50_000_000_000,
            &SECRET_HASH,
            &expected_spend_address,
            &expected_refund_address,
            TIME_LOCK,
            mintlayer_sdk_network(coin.network()),
        )
        .unwrap();

        assert_eq!(decoded.outputs().first(), Some(&expected_output));
    }
    fn bind_transaction_capture_api() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        (listener, endpoint)
    }

    fn spawn_transaction_capture_api(listener: TcpListener) -> thread::JoinHandle<Vec<u8>> {
        thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();

            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];

            loop {
                let read = stream.read(&mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);

                if let Some(header_pos) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    let header_end = header_pos + 4;
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(str::to_owned)
                        })
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);

                    if request.len() >= header_end + content_length {
                        break;
                    }
                }

                assert!(request.len() <= 128 * 1024, "mock API POST request too large");
            }

            let header_pos = request
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .expect("Mintlayer API POST headers");
            let header_end = header_pos + 4;
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let mut lines = headers.lines();

            assert_eq!(lines.next().unwrap_or_default(), "POST /api/v2/transaction HTTP/1.1");
            assert!(
                headers
                    .lines()
                    .any(|line| { line.to_ascii_lowercase().starts_with("content-type: text/plain") }),
                "Mintlayer API POST must use Content-Type: text/plain"
            );

            let body = std::str::from_utf8(&request[header_end..])
                .expect("Mintlayer API POST body must be UTF-8 hex")
                .trim();
            assert!(!body.is_empty(), "Mintlayer API POST body must not be empty");

            let tx_bytes = hex::decode(body).expect("Mintlayer API POST body must contain signed transaction hex");
            let tx_id = canonical_transaction_id_from_signed_bytes(&tx_bytes)
                .expect("Mintlayer API POST body must contain a canonical signed transaction");

            respond_json(&mut stream, &json!({ "tx_id": tx_id }).to_string());
            tx_bytes
        })
    }

    fn spawn_withdraw_and_transaction_capture_api(
        listener: TcpListener,
        sender: String,
    ) -> thread::JoinHandle<Vec<u8>> {
        thread::spawn(move || {
            let spendable_path = format!("/api/v2/address/{sender}/spendable-utxos");
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut request_count = 0_usize;
            let mut submitted_bytes = None;

            while request_count < 4 && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_test_http_request(&mut stream);
                        let header_pos = request
                            .windows(4)
                            .position(|window| window == b"\r\n\r\n")
                            .expect("Mintlayer combined mock API headers");
                        let header_end = header_pos + 4;
                        let headers = String::from_utf8_lossy(&request[..header_end]);
                        let request_line = headers
                            .lines()
                            .next()
                            .expect("Mintlayer combined mock API request line");
                        let mut parts = request_line.split_whitespace();
                        let method = parts.next().expect("HTTP method");
                        let request_path = parts.next().expect("HTTP path");

                        match (method, request_path) {
                            ("GET", value) if value == spendable_path => {
                                let body = json!([{
                                    "outpoint": {
                                        "index": 0,
                                        "source_id": "11".repeat(32),
                                        "source_type": "Transaction"
                                    },
                                    "utxo": {
                                        "destination": sender.clone(),
                                        "type": "Transfer",
                                        "value": {
                                            "amount": { "atoms": "100000000000", "decimal": "1" },
                                            "type": "Coin"
                                        }
                                    }
                                }])
                                .to_string();
                                respond_json(&mut stream, &body);
                            },
                            ("GET", "/api/v2/feerate") => {
                                respond_json(&mut stream, &format!("\"{WITHDRAW_FEE_RATE_ATOMS_PER_KB}\""));
                            },
                            ("GET", "/api/v2/chain/tip") => {
                                respond_json(
                                    &mut stream,
                                    &json!({
                                        "block_height": 700000,
                                        "block_id": "22".repeat(32)
                                    })
                                    .to_string(),
                                );
                            },
                            ("POST", "/api/v2/transaction") => {
                                assert!(
                                    headers.lines().any(|line| {
                                        line.to_ascii_lowercase().starts_with("content-type: text/plain")
                                    }),
                                    "Mintlayer API POST must use Content-Type: text/plain"
                                );

                                let body = std::str::from_utf8(&request[header_end..])
                                    .expect("Mintlayer API POST body must be UTF-8 hex")
                                    .trim();
                                let tx_bytes = hex::decode(body)
                                    .expect("Mintlayer API POST body must contain signed transaction hex");
                                let tx_id = canonical_transaction_id_from_signed_bytes(&tx_bytes)
                                    .expect("Mintlayer API POST body must contain a canonical signed transaction");

                                assert!(
                                    submitted_bytes.replace(tx_bytes).is_none(),
                                    "combined mock API received more than one transaction POST"
                                );
                                respond_json(&mut stream, &json!({ "tx_id": tx_id }).to_string());
                            },
                            _ => panic!(
                                "unexpected combined Mintlayer mock API request: {} {}",
                                method, request_path
                            ),
                        }

                        request_count += 1;
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    },
                    Err(error) => panic!("combined Mintlayer mock API accept failed: {}", error),
                }
            }

            assert_eq!(request_count, 4, "combined mock API did not receive all four requests");
            submitted_bytes.expect("combined mock API did not receive transaction POST")
        })
    }

    fn spawn_resolution_and_transaction_capture_api(listener: TcpListener) -> thread::JoinHandle<Vec<u8>> {
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut request_count = 0_usize;
            let mut submitted_bytes = None;

            while request_count < 3 && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let request = read_test_http_request(&mut stream);
                        let header_pos = request
                            .windows(4)
                            .position(|window| window == b"\r\n\r\n")
                            .expect("Mintlayer resolution mock API headers");
                        let header_end = header_pos + 4;
                        let headers = String::from_utf8_lossy(&request[..header_end]);
                        let request_line = headers
                            .lines()
                            .next()
                            .expect("Mintlayer resolution mock API request line");
                        let mut parts = request_line.split_whitespace();
                        let method = parts.next().expect("HTTP method");
                        let request_path = parts.next().expect("HTTP path");

                        match (method, request_path) {
                            ("GET", "/api/v2/feerate") => {
                                respond_json(&mut stream, &format!("\"{WITHDRAW_FEE_RATE_ATOMS_PER_KB}\""))
                            },
                            ("GET", "/api/v2/chain/tip") => respond_json(
                                &mut stream,
                                &json!({
                                    "block_height": 700000,
                                    "block_id": "22".repeat(32)
                                })
                                .to_string(),
                            ),
                            ("POST", "/api/v2/transaction") => {
                                assert!(
                                    headers.lines().any(|line| {
                                        line.to_ascii_lowercase().starts_with("content-type: text/plain")
                                    }),
                                    "Mintlayer API POST must use Content-Type: text/plain"
                                );
                                let body = std::str::from_utf8(&request[header_end..])
                                    .expect("Mintlayer API POST body must be UTF-8 hex")
                                    .trim();
                                let tx_bytes = hex::decode(body)
                                    .expect("Mintlayer API POST body must contain signed transaction hex");
                                let tx_id = canonical_transaction_id_from_signed_bytes(&tx_bytes)
                                    .expect("Mintlayer API POST body must contain a canonical signed transaction");
                                assert!(
                                    submitted_bytes.replace(tx_bytes).is_none(),
                                    "resolution mock API received more than one transaction POST"
                                );
                                respond_json(&mut stream, &json!({ "tx_id": tx_id }).to_string());
                            },
                            _ => panic!(
                                "unexpected Mintlayer resolution mock API request: {} {}",
                                method, request_path
                            ),
                        }
                        request_count += 1;
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    },
                    Err(error) => panic!("Mintlayer resolution mock API accept failed: {}", error),
                }
            }

            assert_eq!(
                request_count, 3,
                "resolution mock API did not receive all three requests"
            );
            submitted_bytes.expect("resolution mock API did not receive transaction POST")
        })
    }

    #[tokio::test]
    async fn htlc_spend_swapops_builds_broadcasts_and_reveals_secret() {
        use mintlayer_sdk::crypto::types::HtlcSecret;
        use mintlayer_sdk::crypto::SourceId;
        use mintlayer_sdk::prelude::DecodeAll;

        const SECRET: [u8; 32] = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10, 0x11,
            0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
        ];
        const TIME_LOCK: u64 = 1_800_010_000;

        let secret_hash = mintlayer_sdk::crypto::types::HtlcSecret::new(SECRET).hash();

        let (api_listener, api_url) = bind_withdraw_mock_api();
        let request = request_with_urls(vec![api_url]);
        let coin = MintlayerCoin::new(&test_ctx(), valid_conf(), request, iguana_policy()).unwrap();
        assert_eq!(coin.node_rpc_url(), None);

        let sender = coin.address().to_owned();
        let api_server = spawn_withdraw_mock_api(api_listener, sender);

        let other_secret = [2_u8; 32];
        let other_key_pair = key_pair_from_secret(&other_secret).unwrap();
        let other_pubkey = other_key_pair.public_slice().to_vec();

        let swap_contract_address = None;
        let payment_instructions = None;
        let amount: BigDecimal = "0.5".parse().unwrap();
        let swap_unique_data = b"mintlayer-vc3-spend";

        let payment_args = SendPaymentArgs {
            time_lock_duration: 3600,
            time_lock: TIME_LOCK,
            other_pubkey: coin.public_key(),
            secret_hash: secret_hash.as_bytes(),
            amount,
            swap_contract_address: &swap_contract_address,
            swap_unique_data,
            payment_instructions: &payment_instructions,
            watcher_reward: None,
            wait_for_confirmation_until: 0,
        };

        let payment = build_mintlayer_swap_payment(&coin, &payment_args).await.unwrap();

        api_server.join().unwrap();

        let (resolution_api_listener, resolution_api_url) = bind_withdraw_mock_api();
        let resolution_request = request_with_urls(vec![resolution_api_url]);
        let spend_coin = MintlayerCoin::new(&test_ctx(), valid_conf(), resolution_request, iguana_policy()).unwrap();
        assert_eq!(spend_coin.node_rpc_url(), None);

        let resolution_api_server = spawn_resolution_and_transaction_capture_api(resolution_api_listener);

        let spend_args = SpendPaymentArgs {
            other_payment_tx: &payment.signed_bytes,
            time_lock: TIME_LOCK,
            other_pubkey: &other_pubkey,
            secret: &SECRET,
            secret_hash: secret_hash.as_bytes(),
            swap_contract_address: &None,
            swap_unique_data,
            watcher_reward: false,
        };

        let result = SwapOps::send_maker_spends_taker_payment(&spend_coin, spend_args)
            .await
            .unwrap();

        let submitted_bytes = resolution_api_server.join().unwrap();

        let transaction = match result {
            TransactionEnum::MintlayerTransaction(transaction) => transaction,
            other => panic!("expected Mintlayer HTLC spend transaction, got {:?}", other),
        };

        assert_eq!(transaction.signed_bytes, submitted_bytes);

        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&transaction.signed_bytes).unwrap(),
            transaction.transaction_id
        );

        let signed =
            mintlayer_sdk::crypto::types::SignedTransaction::decode_all(&mut &transaction.signed_bytes[..]).unwrap();

        let payment_txid = canonical_transaction_id_from_signed_bytes(&payment.signed_bytes).unwrap();

        let source_id = mintlayer_sdk::crypto::encode_outpoint_source_id(
            mintlayer_sdk::crypto::types::H256::from_slice(&hex::decode(payment_txid).unwrap()),
            SourceId::Transaction,
        );

        let extracted = mintlayer_sdk::crypto::extract_htlc_secret(&signed, source_id, 0).unwrap();

        assert_eq!(extracted, HtlcSecret::new(SECRET));

        let kdf_extracted = SwapOps::extract_secret(&spend_coin, secret_hash.as_bytes(), &transaction.signed_bytes)
            .await
            .unwrap();

        assert_eq!(kdf_extracted, SECRET);

        let wrong_secret_hash = HtlcSecret::new([0xaa; 32]).hash();

        assert!(
            SwapOps::extract_secret(&spend_coin, wrong_secret_hash.as_bytes(), &transaction.signed_bytes,)
                .await
                .is_err(),
            "Mintlayer KDF extract_secret must reject a mismatched secret hash"
        );
    }

    #[tokio::test]
    async fn htlc_refund_swapops_builds_broadcasts_without_revealing_secret() {
        use crate::SwapTxTypeWithSecretHash;
        use mintlayer_sdk::crypto::SourceId;
        use mintlayer_sdk::prelude::DecodeAll;

        const SECRET: [u8; 32] = [0x55; 32];
        const TIME_LOCK: u64 = 1_800_020_000;

        let secret_hash = mintlayer_sdk::crypto::types::HtlcSecret::new(SECRET).hash();

        let (api_listener, api_url) = bind_withdraw_mock_api();
        let request = request_with_urls(vec![api_url]);
        let coin = MintlayerCoin::new(&test_ctx(), valid_conf(), request, iguana_policy()).unwrap();
        assert_eq!(coin.node_rpc_url(), None);

        let sender = coin.address().to_owned();
        let api_server = spawn_withdraw_mock_api(api_listener, sender);

        let other_secret = [3_u8; 32];
        let other_key_pair = key_pair_from_secret(&other_secret).unwrap();
        let other_pubkey = other_key_pair.public_slice().to_vec();

        let swap_contract_address = None;
        let payment_instructions = None;
        let amount: BigDecimal = "0.5".parse().unwrap();
        let swap_unique_data = b"mintlayer-vc3-refund";

        let payment_args = SendPaymentArgs {
            time_lock_duration: 3600,
            time_lock: TIME_LOCK,
            other_pubkey: &other_pubkey,
            secret_hash: secret_hash.as_bytes(),
            amount,
            swap_contract_address: &swap_contract_address,
            swap_unique_data,
            payment_instructions: &payment_instructions,
            watcher_reward: None,
            wait_for_confirmation_until: 0,
        };

        let payment = build_mintlayer_swap_payment(&coin, &payment_args).await.unwrap();

        api_server.join().unwrap();

        let (resolution_api_listener, resolution_api_url) = bind_withdraw_mock_api();
        let resolution_request = request_with_urls(vec![resolution_api_url]);
        let refund_coin = MintlayerCoin::new(&test_ctx(), valid_conf(), resolution_request, iguana_policy()).unwrap();
        assert_eq!(refund_coin.node_rpc_url(), None);

        let resolution_api_server = spawn_resolution_and_transaction_capture_api(resolution_api_listener);

        let refund_args = RefundPaymentArgs {
            payment_tx: &payment.signed_bytes,
            time_lock: TIME_LOCK,
            other_pubkey: &other_pubkey,
            tx_type_with_secret_hash: SwapTxTypeWithSecretHash::TakerOrMakerPayment {
                maker_secret_hash: secret_hash.as_bytes(),
            },
            swap_contract_address: &None,
            swap_unique_data,
            watcher_reward: false,
        };

        let result = SwapOps::send_maker_refunds_payment(&refund_coin, refund_args)
            .await
            .unwrap();

        let submitted_bytes = resolution_api_server.join().unwrap();

        let transaction = match result {
            TransactionEnum::MintlayerTransaction(transaction) => transaction,
            other => panic!("expected Mintlayer HTLC refund transaction, got {:?}", other),
        };

        assert_eq!(transaction.signed_bytes, submitted_bytes);

        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&transaction.signed_bytes).unwrap(),
            transaction.transaction_id
        );

        let signed =
            mintlayer_sdk::crypto::types::SignedTransaction::decode_all(&mut &transaction.signed_bytes[..]).unwrap();

        let payment_txid = canonical_transaction_id_from_signed_bytes(&payment.signed_bytes).unwrap();

        let source_id = mintlayer_sdk::crypto::encode_outpoint_source_id(
            mintlayer_sdk::crypto::types::H256::from_slice(&hex::decode(payment_txid).unwrap()),
            SourceId::Transaction,
        );

        let extracted = mintlayer_sdk::crypto::extract_htlc_secret(&signed, source_id, 0);

        assert!(
            matches!(extracted, Err(mintlayer_sdk::crypto::Error::UnexpectedHtlcSpendType)),
            "refund must not reveal an HTLC secret: {:?}",
            extracted
        );

        assert!(
            SwapOps::extract_secret(&refund_coin, secret_hash.as_bytes(), &transaction.signed_bytes,)
                .await
                .is_err(),
            "Mintlayer KDF extract_secret must reject an HTLC refund"
        );
    }

    #[tokio::test]
    async fn send_raw_tx_bytes_uses_canonical_txid_and_https_broadcaster() {
        let (api_listener, api_url) = bind_withdraw_mock_api();
        let build_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![api_url]),
            iguana_policy(),
        )
        .unwrap();
        let sender = build_coin.address().to_owned();
        let api_server = spawn_withdraw_mock_api(api_listener, sender);

        let details = MmCoin::withdraw(&build_coin, withdraw_request(WITHDRAW_RECIPIENT))
            .compat()
            .await
            .unwrap();
        api_server.join().unwrap();

        let (signed_bytes, expected_txid) = match details.tx {
            TransactionData::Signed { tx_hex, tx_hash } => (tx_hex.0, tx_hash),
            other => panic!("expected signed Mintlayer transaction, got {:?}", other),
        };

        let (post_listener, post_url) = bind_transaction_capture_api();
        let post_server = spawn_transaction_capture_api(post_listener);

        let request = request_with_urls(vec![post_url]);
        let coin = MintlayerCoin::new(&test_ctx(), valid_conf(), request, iguana_policy()).unwrap();

        assert_eq!(coin.node_rpc_url(), None);
        assert!(coin.node_client().unwrap().is_none());

        let returned_txid = coin.send_raw_tx_bytes(&signed_bytes).compat().await.unwrap();
        assert_eq!(returned_txid, expected_txid);

        let submitted_bytes = post_server.join().unwrap();
        assert_eq!(submitted_bytes, signed_bytes);
        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&submitted_bytes).unwrap(),
            expected_txid
        );
    }

    #[tokio::test]
    async fn transaction_data_broadcast_orchestrator_preserves_bytes_and_canonical_txid() {
        let (api_listener, api_url) = bind_withdraw_mock_api();
        let build_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![api_url]),
            iguana_policy(),
        )
        .unwrap();
        let sender = build_coin.address().to_owned();
        let api_server = spawn_withdraw_mock_api(api_listener, sender);

        let details = MmCoin::withdraw(&build_coin, withdraw_request(WITHDRAW_RECIPIENT))
            .compat()
            .await
            .unwrap();
        api_server.join().unwrap();

        let (signed_bytes, expected_txid) = match details.tx {
            TransactionData::Signed { tx_hex, tx_hash } => (tx_hex.0, tx_hash),
            other => panic!("expected signed Mintlayer transaction, got {:?}", other),
        };

        let tx = TransactionData::new_signed(signed_bytes.clone().into(), expected_txid.clone());

        let (post_listener, post_url) = bind_transaction_capture_api();
        let post_server = spawn_transaction_capture_api(post_listener);

        let request = request_with_urls(vec![post_url]);
        let coin = MintlayerCoin::new(&test_ctx(), valid_conf(), request, iguana_policy()).unwrap();

        assert_eq!(coin.node_rpc_url(), None);
        assert!(coin.node_client().unwrap().is_none());

        let returned_txid = broadcast_mintlayer_transaction_data(&coin, &tx).await.unwrap();
        assert_eq!(returned_txid, expected_txid);

        let submitted_bytes = post_server.join().unwrap();
        assert_eq!(submitted_bytes, signed_bytes);
        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&submitted_bytes).unwrap(),
            expected_txid
        );
    }

    fn spawn_payment_recovery_rest_api(
        listener: TcpListener,
        expected_address_path: String,
        transaction_id: String,
        transaction_hex: String,
        block_id: String,
        block_height: u64,
        load_raw_transaction: bool,
    ) -> thread::JoinHandle<Vec<String>> {
        thread::spawn(move || {
            let expected_total = if load_raw_transaction { 5 } else { 4 };
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut requested_paths = Vec::new();
            let mut transaction_requests = 0_usize;

            while requested_paths.len() < expected_total && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let path = read_request_path(&mut stream);

                        let body = if path == expected_address_path {
                            json!({
                                "coin_balance": { "atoms": "0", "decimal": "0" },
                                "locked_coin_balance": { "atoms": "0", "decimal": "0" },
                                "transaction_history": [transaction_id.clone()],
                                "tokens": []
                            })
                            .to_string()
                        } else if path == format!("/api/v2/transaction/{transaction_id}") {
                            transaction_requests += 1;
                            json!({
                                "id": transaction_id.clone(),
                                "block_id": block_id.clone(),
                                "inputs": [],
                                "tx_hex": transaction_hex.clone()
                            })
                            .to_string()
                        } else if path == format!("/api/v2/block/{block_id}") {
                            json!({ "height": block_height }).to_string()
                        } else if path == format!("/api/v2/chain/{block_height}") {
                            json!(block_id.clone()).to_string()
                        } else {
                            panic!("unexpected payment-recovery REST request: {}", path);
                        };

                        respond_json(&mut stream, &body);
                        requested_paths.push(path);
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    },
                    Err(error) => panic!("payment-recovery REST accept failed: {}", error),
                }
            }

            assert_eq!(
                requested_paths.len(),
                expected_total,
                "payment-recovery REST fixture did not receive all expected requests"
            );
            assert_eq!(
                transaction_requests,
                if load_raw_transaction { 2 } else { 1 },
                "unexpected number of transaction endpoint requests"
            );
            requested_paths
        })
    }

    fn spawn_confirmation_rest_api(
        listener: TcpListener,
        transaction_id: String,
        block_id: String,
        inclusion_height: u64,
        best_height: u64,
    ) -> thread::JoinHandle<Vec<String>> {
        thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut requested_paths = Vec::new();

            while requested_paths.len() < 4 && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let path = read_request_path(&mut stream);

                        let body = if path == format!("/api/v2/transaction/{transaction_id}") {
                            json!({
                                "id": transaction_id.clone(),
                                "block_id": block_id.clone(),
                                "inputs": []
                            })
                            .to_string()
                        } else if path == format!("/api/v2/block/{block_id}") {
                            json!({ "height": inclusion_height }).to_string()
                        } else if path == format!("/api/v2/chain/{inclusion_height}") {
                            json!(block_id.clone()).to_string()
                        } else if path == "/api/v2/chain/tip" {
                            json!({
                                "block_height": best_height,
                                "block_id": block_id.clone()
                            })
                            .to_string()
                        } else {
                            panic!("unexpected confirmation REST request: {}", path);
                        };

                        respond_json(&mut stream, &body);
                        requested_paths.push(path);
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    },
                    Err(error) => panic!("confirmation REST accept failed: {}", error),
                }
            }

            assert_eq!(
                requested_paths.len(),
                4,
                "confirmation REST fixture did not receive all four requests"
            );
            requested_paths
        })
    }

    #[tokio::test]
    async fn check_if_my_payment_sent_recovers_canonical_payment() {
        const SECRET_HASH: [u8; 20] = [
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x10, 0x20,
            0x30, 0x40,
        ];
        const TIME_LOCK: u64 = 1_800_030_000;
        const BLOCK_HEIGHT: u64 = 700_001;

        let (build_api_listener, build_api_url) = bind_withdraw_mock_api();
        let build_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![build_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let sender = build_coin.address().to_owned();
        let build_api_server = spawn_withdraw_mock_api(build_api_listener, sender);

        let other_secret = [4_u8; 32];
        let other_key_pair = key_pair_from_secret(&other_secret).unwrap();
        let other_pubkey = other_key_pair.public_slice().to_vec();

        let swap_contract_address = None;
        let payment_instructions = None;
        let amount: BigDecimal = "0.5".parse().unwrap();
        let swap_unique_data = b"mintlayer-vd4c1-observation";

        let payment_args = SendPaymentArgs {
            time_lock_duration: 3600,
            time_lock: TIME_LOCK,
            other_pubkey: &other_pubkey,
            secret_hash: &SECRET_HASH,
            amount: amount.clone(),
            swap_contract_address: &swap_contract_address,
            swap_unique_data,
            payment_instructions: &payment_instructions,
            watcher_reward: None,
            wait_for_confirmation_until: 0,
        };

        let payment = build_mintlayer_swap_payment(&build_coin, &payment_args).await.unwrap();
        build_api_server.join().unwrap();

        let payment_txid = payment.transaction_id.clone();
        let payment_hex = hex::encode(&payment.signed_bytes);
        let block_id = "44".repeat(32);

        let refund_key_pair = build_coin.derive_htlc_key_pair(swap_unique_data);
        let refund_address =
            mintlayer_address_from_compressed_public_key(build_coin.network(), refund_key_pair.public_slice()).unwrap();

        let (scanner_listener, scanner_url) = bind_withdraw_mock_api();
        let expected_address_path = format!("/api/v2/address/{refund_address}");
        let scanner_server = spawn_payment_recovery_rest_api(
            scanner_listener,
            expected_address_path,
            payment_txid.clone(),
            payment_hex,
            block_id,
            BLOCK_HEIGHT,
            true,
        );

        let observation_request = request_with_urls(vec![scanner_url]);
        let observation_coin =
            MintlayerCoin::new(&test_ctx(), valid_conf(), observation_request, iguana_policy()).unwrap();

        assert_eq!(observation_coin.node_rpc_url(), None);
        assert!(observation_coin.node_client().unwrap().is_none());

        let args = CheckIfMyPaymentSentArgs {
            time_lock: TIME_LOCK,
            other_pub: &other_pubkey,
            secret_hash: &SECRET_HASH,
            search_from_block: BLOCK_HEIGHT,
            swap_contract_address: &None,
            swap_unique_data,
            amount: &amount,
            payment_instructions: &None,
        };

        let found = SwapOps::check_if_my_payment_sent(&observation_coin, args)
            .await
            .unwrap()
            .expect("Mintlayer payment must be recovered from REST chain observation");

        scanner_server.join().unwrap();

        let recovered = match found {
            TransactionEnum::MintlayerTransaction(transaction) => transaction,
            other => panic!("expected recovered Mintlayer transaction, got {:?}", other),
        };

        assert_eq!(recovered.transaction_id, payment.transaction_id);
        assert_eq!(recovered.signed_bytes, payment.signed_bytes);
        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&recovered.signed_bytes).unwrap(),
            recovered.transaction_id
        );
    }

    #[tokio::test]
    async fn check_if_my_payment_sent_respects_search_from_block() {
        const SECRET_HASH: [u8; 20] = [
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x10, 0x20,
            0x30, 0x40,
        ];
        const TIME_LOCK: u64 = 1_800_030_000;
        const BLOCK_HEIGHT: u64 = 700_001;
        const SEARCH_FROM_BLOCK: u64 = BLOCK_HEIGHT + 1;

        let (build_api_listener, build_api_url) = bind_withdraw_mock_api();
        let build_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![build_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let sender = build_coin.address().to_owned();
        let build_api_server = spawn_withdraw_mock_api(build_api_listener, sender);

        let other_secret = [4_u8; 32];
        let other_key_pair = key_pair_from_secret(&other_secret).unwrap();
        let other_pubkey = other_key_pair.public_slice().to_vec();

        let swap_contract_address = None;
        let payment_instructions = None;
        let amount: BigDecimal = "0.5".parse().unwrap();
        let swap_unique_data = b"mintlayer-vd4c2-cutoff";

        let payment_args = SendPaymentArgs {
            time_lock_duration: 3600,
            time_lock: TIME_LOCK,
            other_pubkey: &other_pubkey,
            secret_hash: &SECRET_HASH,
            amount: amount.clone(),
            swap_contract_address: &swap_contract_address,
            swap_unique_data,
            payment_instructions: &payment_instructions,
            watcher_reward: None,
            wait_for_confirmation_until: 0,
        };

        let payment = build_mintlayer_swap_payment(&build_coin, &payment_args).await.unwrap();
        build_api_server.join().unwrap();

        let payment_txid = payment.transaction_id.clone();
        let payment_hex = hex::encode(&payment.signed_bytes);
        let block_id = "55".repeat(32);

        let refund_key_pair = build_coin.derive_htlc_key_pair(swap_unique_data);
        let refund_address =
            mintlayer_address_from_compressed_public_key(build_coin.network(), refund_key_pair.public_slice()).unwrap();

        let (scanner_listener, scanner_url) = bind_withdraw_mock_api();
        let expected_address_path = format!("/api/v2/address/{refund_address}");
        let scanner_server = spawn_payment_recovery_rest_api(
            scanner_listener,
            expected_address_path,
            payment_txid,
            payment_hex,
            block_id,
            BLOCK_HEIGHT,
            false,
        );

        let observation_request = request_with_urls(vec![scanner_url]);
        let observation_coin =
            MintlayerCoin::new(&test_ctx(), valid_conf(), observation_request, iguana_policy()).unwrap();

        assert_eq!(observation_coin.node_rpc_url(), None);
        assert!(observation_coin.node_client().unwrap().is_none());

        let args = CheckIfMyPaymentSentArgs {
            time_lock: TIME_LOCK,
            other_pub: &other_pubkey,
            secret_hash: &SECRET_HASH,
            search_from_block: SEARCH_FROM_BLOCK,
            swap_contract_address: &None,
            swap_unique_data,
            amount: &amount,
            payment_instructions: &None,
        };

        let found = SwapOps::check_if_my_payment_sent(&observation_coin, args)
            .await
            .unwrap();

        assert!(found.is_none(), "payment below search_from_block must be ignored");
        scanner_server.join().unwrap();
    }

    #[tokio::test]
    async fn wait_for_confirmations_accepts_confirmed_payment_and_times_out() {
        use futures::compat::Future01CompatExt;

        const SECRET_HASH: [u8; 20] = [
            0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x00, 0x10, 0x20,
            0x30, 0x40,
        ];
        const TIME_LOCK: u64 = 1_800_040_000;
        const INCLUSION_HEIGHT: u64 = 700_000;
        const BEST_HEIGHT: u64 = 700_005;

        let (build_api_listener, build_api_url) = bind_withdraw_mock_api();
        let build_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![build_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let sender = build_coin.address().to_owned();
        let build_api_server = spawn_withdraw_mock_api(build_api_listener, sender);

        let other_secret = [5_u8; 32];
        let other_key_pair = key_pair_from_secret(&other_secret).unwrap();
        let other_pubkey = other_key_pair.public_slice().to_vec();

        let swap_contract_address = None;
        let payment_instructions = None;
        let amount: BigDecimal = "0.5".parse().unwrap();
        let swap_unique_data = b"mintlayer-vd6c-wait";

        let payment_args = SendPaymentArgs {
            time_lock_duration: 3600,
            time_lock: TIME_LOCK,
            other_pubkey: &other_pubkey,
            secret_hash: &SECRET_HASH,
            amount,
            swap_contract_address: &swap_contract_address,
            swap_unique_data,
            payment_instructions: &payment_instructions,
            watcher_reward: None,
            wait_for_confirmation_until: 0,
        };

        let payment = build_mintlayer_swap_payment(&build_coin, &payment_args).await.unwrap();
        build_api_server.join().unwrap();

        let payment_txid = payment.transaction_id.clone();
        let block_id = "88".repeat(32);

        let (scanner_listener, scanner_url) = bind_withdraw_mock_api();
        let scanner_server =
            spawn_confirmation_rest_api(scanner_listener, payment_txid, block_id, INCLUSION_HEIGHT, BEST_HEIGHT);

        let request = request_with_urls(vec![scanner_url]);
        let coin = MintlayerCoin::new(&test_ctx(), valid_conf(), request, iguana_policy()).unwrap();

        assert_eq!(coin.node_rpc_url(), None);
        assert!(coin.node_client().unwrap().is_none());

        let success_input = ConfirmPaymentInput {
            payment_tx: payment.signed_bytes.clone(),
            confirmations: 6,
            requires_nota: false,
            wait_until: now_sec() + 30,
            check_every: 1,
        };

        coin.wait_for_confirmations(success_input)
            .compat()
            .await
            .expect("confirmed Mintlayer payment must satisfy wait_for_confirmations");

        scanner_server.join().unwrap();

        let timeout_input = ConfirmPaymentInput {
            payment_tx: payment.signed_bytes,
            confirmations: 7,
            requires_nota: false,
            wait_until: now_sec() - 1,
            check_every: 0,
        };

        let timeout_error = coin
            .wait_for_confirmations(timeout_input)
            .compat()
            .await
            .expect_err("expired Mintlayer confirmation wait must fail");

        assert!(
            timeout_error.contains("Waited too long"),
            "unexpected Mintlayer confirmation timeout: {}",
            timeout_error
        );
    }

    #[tokio::test]
    async fn htlc_spend_observer_recovers_canonical_spender() {
        const SECRET: [u8; 32] = [0x55; 32];
        const TIME_LOCK: u64 = 1_800_030_000;
        const SPENT_HEIGHT: u64 = 700_123;

        let secret_hash = mintlayer_sdk::crypto::types::HtlcSecret::new(SECRET).hash();

        let (payment_api_listener, payment_api_url) = bind_withdraw_mock_api();

        let payment_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![payment_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let sender = payment_coin.address().to_owned();
        let payment_api_server = spawn_withdraw_mock_api(payment_api_listener, sender);

        let other_secret = [2_u8; 32];
        let other_key_pair = key_pair_from_secret(&other_secret).unwrap();
        let other_pubkey = other_key_pair.public_slice().to_vec();

        let swap_contract_address = None;
        let payment_instructions = None;
        let amount: BigDecimal = "0.5".parse().unwrap();
        let swap_unique_data = b"mintlayer-vd7b-observer";

        let payment_args = SendPaymentArgs {
            time_lock_duration: 3600,
            time_lock: TIME_LOCK,
            other_pubkey: &other_pubkey,
            secret_hash: secret_hash.as_bytes(),
            amount,
            swap_contract_address: &swap_contract_address,
            swap_unique_data,
            payment_instructions: &payment_instructions,
            watcher_reward: None,
            wait_for_confirmation_until: 0,
        };

        let payment = build_mintlayer_swap_payment(&payment_coin, &payment_args)
            .await
            .unwrap();

        payment_api_server.join().unwrap();

        let (spend_api_listener, spend_api_url) = bind_withdraw_mock_api();
        let spend_api_server = spawn_resolution_mock_api(spend_api_listener);

        let mut spend_request = request_with_urls(vec![spend_api_url]);
        spend_request.node_conf = None;

        let spend_coin = MintlayerCoin::new(&test_ctx(), valid_conf(), spend_request, iguana_policy()).unwrap();

        let spend_args = SpendPaymentArgs {
            other_payment_tx: &payment.signed_bytes,
            time_lock: TIME_LOCK,
            other_pubkey: &other_pubkey,
            secret: &SECRET,
            secret_hash: secret_hash.as_bytes(),
            swap_contract_address: &None,
            swap_unique_data,
            watcher_reward: false,
        };

        let spender = build_mintlayer_htlc_spend(&spend_coin, &spend_args).await.unwrap();

        spend_api_server.join().unwrap();

        let payment_txid = payment.transaction_id.clone();
        let spender_txid = spender.transaction_id.clone();
        let spender_hex = hex::encode(&spender.signed_bytes);
        let block_id = "77".repeat(32);

        let spend_address =
            mintlayer_address_from_compressed_public_key(payment_coin.network(), &other_pubkey).unwrap();

        let refund_key_pair = payment_coin.derive_htlc_key_pair(swap_unique_data);
        let refund_address =
            mintlayer_address_from_compressed_public_key(payment_coin.network(), refund_key_pair.public_slice())
                .unwrap();

        let (scanner_listener, scanner_url) = bind_withdraw_mock_api();

        let scanner_payment_txid = payment_txid.clone();
        let scanner_spender_txid = spender_txid.clone();
        let scanner_spender_hex = spender_hex.clone();
        let scanner_block_id = block_id.clone();
        let scanner_spend_address = spend_address.clone();
        let scanner_refund_address = refund_address.clone();

        let scanner_server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut request_count = 0;

            while request_count < 4 && Instant::now() < deadline {
                match scanner_listener.accept() {
                    Ok((mut stream, _)) => {
                        let path = read_request_path(&mut stream);

                        let body = if path == format!("/api/v2/transaction/{scanner_payment_txid}/output/0") {
                            json!({
                                "type": "Htlc",
                                "value": {
                                    "type": "Coin",
                                    "amount": {
                                        "atoms": "50000000000",
                                        "decimal": "0.5"
                                    }
                                },
                                "htlc": {
                                    "secret": null,
                                    "secret_hash": {
                                        "string": null,
                                        "hex": "00"
                                    },
                                    "spend_key": scanner_spend_address,
                                    "refund_timelock": {
                                        "UntilTime": TIME_LOCK
                                    },
                                    "refund_key": scanner_refund_address
                                },
                                "spent_at_block_height": SPENT_HEIGHT
                            })
                            .to_string()
                        } else if path == format!("/api/v2/address/{scanner_spend_address}") {
                            json!({
                                "coin_balance": {
                                    "atoms": "0",
                                    "decimal": "0"
                                },
                                "locked_coin_balance": {
                                    "atoms": "0",
                                    "decimal": "0"
                                },
                                "transaction_history": [
                                    scanner_payment_txid.clone(),
                                    scanner_spender_txid.clone()
                                ],
                                "tokens": []
                            })
                            .to_string()
                        } else if path == format!("/api/v2/transaction/{scanner_spender_txid}") {
                            json!({
                                "id": scanner_spender_txid.clone(),
                                "block_id": scanner_block_id.clone(),
                                "inputs": [{
                                    "input": {
                                        "input_type": "UTXO",
                                        "source_type": "Transaction",
                                        "source_id": scanner_payment_txid.clone(),
                                        "index": 0
                                    },
                                    "utxo": null
                                }],
                                "tx_hex": scanner_spender_hex.clone()
                            })
                            .to_string()
                        } else {
                            panic!("unexpected V-D7b.1 scanner request: {}", path);
                        };

                        respond_json(&mut stream, &body);
                        request_count += 1;
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    },
                    Err(error) => panic!("V-D7b.1 scanner accept failed: {}", error),
                }
            }

            assert_eq!(request_count, 4, "V-D7b.1 scanner did not receive all requests");
        });

        let observer_request = request_with_urls(vec![scanner_url]);
        let observer_coin = MintlayerCoin::new(&test_ctx(), valid_conf(), observer_request, iguana_policy()).unwrap();

        assert_eq!(observer_coin.node_rpc_url(), None);
        assert!(observer_coin.node_client().unwrap().is_none());

        let found = observe_mintlayer_htlc_spend_from_payment(&observer_coin, &payment.signed_bytes, SPENT_HEIGHT)
            .await
            .unwrap()
            .expect("Mintlayer HTLC spender must be found");

        scanner_server.join().unwrap();

        assert_eq!(found.transaction_id, spender.transaction_id);
        assert_eq!(found.signed_bytes, spender.signed_bytes);
        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&found.signed_bytes).unwrap(),
            found.transaction_id
        );
    }

    #[tokio::test]
    async fn wait_for_htlc_tx_spend_returns_canonical_spender() {
        const SECRET: [u8; 32] = [0x56; 32];
        const TIME_LOCK: u64 = 1_800_040_000;
        const SPENT_HEIGHT: u64 = 700_234;

        let secret_hash = mintlayer_sdk::crypto::types::HtlcSecret::new(SECRET).hash();

        let (payment_api_listener, payment_api_url) = bind_withdraw_mock_api();

        let payment_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![payment_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let sender = payment_coin.address().to_owned();
        let payment_api_server = spawn_withdraw_mock_api(payment_api_listener, sender);

        let other_secret = [3_u8; 32];
        let other_key_pair = key_pair_from_secret(&other_secret).unwrap();
        let other_pubkey = other_key_pair.public_slice().to_vec();

        let swap_contract_address = None;
        let payment_instructions = None;
        let amount: BigDecimal = "0.5".parse().unwrap();
        let swap_unique_data = b"mintlayer-vd7c-wait";

        let payment_args = SendPaymentArgs {
            time_lock_duration: 3600,
            time_lock: TIME_LOCK,
            other_pubkey: &other_pubkey,
            secret_hash: secret_hash.as_bytes(),
            amount,
            swap_contract_address: &swap_contract_address,
            swap_unique_data,
            payment_instructions: &payment_instructions,
            watcher_reward: None,
            wait_for_confirmation_until: 0,
        };

        let payment = build_mintlayer_swap_payment(&payment_coin, &payment_args)
            .await
            .unwrap();

        payment_api_server.join().unwrap();

        let (spend_api_listener, spend_api_url) = bind_withdraw_mock_api();
        let spend_api_server = spawn_resolution_mock_api(spend_api_listener);

        let mut spend_request = request_with_urls(vec![spend_api_url]);
        spend_request.node_conf = None;

        let spend_coin = MintlayerCoin::new(&test_ctx(), valid_conf(), spend_request, iguana_policy()).unwrap();

        let spend_args = SpendPaymentArgs {
            other_payment_tx: &payment.signed_bytes,
            time_lock: TIME_LOCK,
            other_pubkey: &other_pubkey,
            secret: &SECRET,
            secret_hash: secret_hash.as_bytes(),
            swap_contract_address: &None,
            swap_unique_data,
            watcher_reward: false,
        };

        let spender = build_mintlayer_htlc_spend(&spend_coin, &spend_args).await.unwrap();

        spend_api_server.join().unwrap();

        let payment_txid = payment.transaction_id.clone();
        let spender_txid = spender.transaction_id.clone();
        let spender_hex = hex::encode(&spender.signed_bytes);
        let block_id = "88".repeat(32);

        let spend_address =
            mintlayer_address_from_compressed_public_key(payment_coin.network(), &other_pubkey).unwrap();

        let refund_key_pair = payment_coin.derive_htlc_key_pair(swap_unique_data);
        let refund_address =
            mintlayer_address_from_compressed_public_key(payment_coin.network(), refund_key_pair.public_slice())
                .unwrap();

        let (scanner_listener, scanner_url) = bind_withdraw_mock_api();

        let scanner_payment_txid = payment_txid.clone();
        let scanner_spender_txid = spender_txid.clone();
        let scanner_spender_hex = spender_hex.clone();
        let scanner_block_id = block_id.clone();
        let scanner_spend_address = spend_address.clone();
        let scanner_refund_address = refund_address.clone();

        let scanner_server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut request_count = 0;

            while request_count < 4 && Instant::now() < deadline {
                match scanner_listener.accept() {
                    Ok((mut stream, _)) => {
                        let path = read_request_path(&mut stream);

                        let body = if path == format!("/api/v2/transaction/{scanner_payment_txid}/output/0") {
                            json!({
                                "type": "Htlc",
                                "value": {
                                    "type": "Coin",
                                    "amount": {
                                        "atoms": "50000000000",
                                        "decimal": "0.5"
                                    }
                                },
                                "htlc": {
                                    "secret": null,
                                    "secret_hash": {
                                        "string": null,
                                        "hex": "00"
                                    },
                                    "spend_key": scanner_spend_address,
                                    "refund_timelock": {
                                        "UntilTime": TIME_LOCK
                                    },
                                    "refund_key": scanner_refund_address
                                },
                                "spent_at_block_height": SPENT_HEIGHT
                            })
                            .to_string()
                        } else if path == format!("/api/v2/address/{scanner_spend_address}") {
                            json!({
                                "coin_balance": {
                                    "atoms": "0",
                                    "decimal": "0"
                                },
                                "locked_coin_balance": {
                                    "atoms": "0",
                                    "decimal": "0"
                                },
                                "transaction_history": [
                                    scanner_payment_txid.clone(),
                                    scanner_spender_txid.clone()
                                ],
                                "tokens": []
                            })
                            .to_string()
                        } else if path == format!("/api/v2/transaction/{scanner_spender_txid}") {
                            json!({
                                "id": scanner_spender_txid.clone(),
                                "block_id": scanner_block_id.clone(),
                                "inputs": [{
                                    "input": {
                                        "input_type": "UTXO",
                                        "source_type": "Transaction",
                                        "source_id": scanner_payment_txid.clone(),
                                        "index": 0
                                    },
                                    "utxo": null
                                }],
                                "tx_hex": scanner_spender_hex.clone()
                            })
                            .to_string()
                        } else {
                            panic!("unexpected V-D7c.3 scanner request: {}", path);
                        };

                        respond_json(&mut stream, &body);
                        request_count += 1;
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    },
                    Err(error) => panic!("V-D7c.3 scanner accept failed: {}", error),
                }
            }

            assert_eq!(request_count, 4, "V-D7c.3 scanner did not receive all requests");
        });

        let wait_request = request_with_urls(vec![scanner_url]);
        let wait_coin = MintlayerCoin::new(&test_ctx(), valid_conf(), wait_request, iguana_policy()).unwrap();

        assert_eq!(wait_coin.node_rpc_url(), None);
        assert!(wait_coin.node_client().unwrap().is_none());

        let no_swap_contract = None;

        let found = wait_coin
            .wait_for_htlc_tx_spend(WaitForHTLCTxSpendArgs {
                tx_bytes: &payment.signed_bytes,
                secret_hash: secret_hash.as_bytes(),
                wait_until: now_sec() + 10,
                from_block: SPENT_HEIGHT,
                swap_contract_address: &no_swap_contract,
                check_every: 0.01,
                watcher_reward: false,
            })
            .await
            .unwrap();

        scanner_server.join().unwrap();

        let found = match found {
            TransactionEnum::MintlayerTransaction(transaction) => transaction,
            other => panic!(
                "expected Mintlayer transaction from wait_for_htlc_tx_spend, got {:?}",
                other
            ),
        };

        assert_eq!(found.transaction_id, spender.transaction_id);
        assert_eq!(found.signed_bytes, spender.signed_bytes);
        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&found.signed_bytes).unwrap(),
            found.transaction_id
        );
    }

    #[tokio::test]
    async fn wait_for_htlc_tx_spend_times_out_when_unspent() {
        const SECRET_HASH: [u8; 20] = [0x22; 20];
        const TIME_LOCK: u64 = 1_800_050_000;

        let (payment_api_listener, payment_api_url) = bind_withdraw_mock_api();

        let payment_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![payment_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let sender = payment_coin.address().to_owned();
        let payment_api_server = spawn_withdraw_mock_api(payment_api_listener, sender);

        let other_secret = [4_u8; 32];
        let other_key_pair = key_pair_from_secret(&other_secret).unwrap();
        let other_pubkey = other_key_pair.public_slice().to_vec();

        let swap_contract_address = None;
        let payment_instructions = None;
        let amount: BigDecimal = "0.5".parse().unwrap();
        let swap_unique_data = b"mintlayer-vd7c-timeout";

        let payment_args = SendPaymentArgs {
            time_lock_duration: 3600,
            time_lock: TIME_LOCK,
            other_pubkey: &other_pubkey,
            secret_hash: &SECRET_HASH,
            amount,
            swap_contract_address: &swap_contract_address,
            swap_unique_data,
            payment_instructions: &payment_instructions,
            watcher_reward: None,
            wait_for_confirmation_until: 0,
        };

        let payment = build_mintlayer_swap_payment(&payment_coin, &payment_args)
            .await
            .unwrap();

        payment_api_server.join().unwrap();

        let payment_txid = payment.transaction_id.clone();

        let (scanner_listener, scanner_url) = bind_withdraw_mock_api();
        let scanner_payment_txid = payment_txid.clone();

        let scanner_server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);

            loop {
                match scanner_listener.accept() {
                    Ok((mut stream, _)) => {
                        let path = read_request_path(&mut stream);

                        assert_eq!(path, format!("/api/v2/transaction/{scanner_payment_txid}/output/0"));

                        let body = json!({
                            "type": "Htlc",
                            "value": {
                                "type": "Coin",
                                "amount": {
                                    "atoms": "50000000000",
                                    "decimal": "0.5"
                                }
                            },
                            "htlc": {
                                "secret": null,
                                "secret_hash": {
                                    "string": null,
                                    "hex": "00"
                                },
                                "spend_key": "mtc1qspend",
                                "refund_timelock": {
                                    "UntilTime": TIME_LOCK
                                },
                                "refund_key": "mtc1qrefund"
                            },
                            "spent_at_block_height": null
                        })
                        .to_string();

                        respond_json(&mut stream, &body);
                        break;
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            panic!("V-D7c.3 timeout scanner request timed out");
                        }
                        thread::sleep(Duration::from_millis(10));
                    },
                    Err(error) => panic!("V-D7c.3 timeout scanner accept failed: {}", error),
                }
            }
        });

        let wait_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![scanner_url]),
            iguana_policy(),
        )
        .unwrap();

        let no_swap_contract = None;

        let error = wait_coin
            .wait_for_htlc_tx_spend(WaitForHTLCTxSpendArgs {
                tx_bytes: &payment.signed_bytes,
                secret_hash: &SECRET_HASH,
                wait_until: now_sec() - 1,
                from_block: 0,
                swap_contract_address: &no_swap_contract,
                check_every: 0.0,
                watcher_reward: false,
            })
            .await
            .expect_err("unspent Mintlayer HTLC must time out");

        scanner_server.join().unwrap();

        assert!(
            error.get_plain_text_format().contains("Waited too long"),
            "unexpected Mintlayer HTLC wait timeout: {:?}",
            error
        );
    }

    #[tokio::test]
    async fn mintlayer_transaction_confirmations_uses_scanner_and_node_heights() {
        const INCLUSION_HEIGHT: u64 = 700_000;
        const BEST_HEIGHT: u64 = 700_005;

        let transaction_id = "66".repeat(32);
        let block_id = "77".repeat(32);

        let (scanner_listener, scanner_url) = bind_withdraw_mock_api();
        let scanner_server = spawn_confirmation_rest_api(
            scanner_listener,
            transaction_id.clone(),
            block_id,
            INCLUSION_HEIGHT,
            BEST_HEIGHT,
        );

        let request = request_with_urls(vec![scanner_url]);
        let coin = MintlayerCoin::new(&test_ctx(), valid_conf(), request, iguana_policy()).unwrap();

        assert_eq!(coin.node_rpc_url(), None);
        assert!(coin.node_client().unwrap().is_none());

        let confirmations = mintlayer_transaction_confirmations(&coin, &transaction_id)
            .await
            .unwrap();

        scanner_server.join().unwrap();
        assert_eq!(confirmations, Some(6));
    }

    fn read_test_http_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
        use std::io::Read;

        let mut request = Vec::new();
        let mut buffer = [0_u8; 4096];

        loop {
            let read = stream.read(&mut buffer).unwrap();

            if read == 0 {
                break;
            }

            request.extend_from_slice(&buffer[..read]);

            if let Some(pos) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                let header_end = pos + 4;
                let headers = String::from_utf8_lossy(&request[..header_end]);

                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        line.to_ascii_lowercase()
                            .strip_prefix("content-length: ")
                            .map(str::to_owned)
                    })
                    .and_then(|value| value.trim().parse::<usize>().ok())
                    .unwrap_or(0);

                if request.len() >= header_end + content_length {
                    break;
                }
            }
        }

        request
    }

    #[tokio::test]
    async fn node_rpc_cookie_auth_is_reloaded_and_sent_on_wire() {
        use std::fs;
        use std::io::Write;
        use std::net::TcpListener;
        use std::thread;
        use std::time::{SystemTime, UNIX_EPOCH};

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());

        let server = thread::spawn(move || {
            for expected_auth in [
                "Basic Y29va2llLXVzZXI6Y29va2llLXBhc3M=",
                "Basic cm90YXRlZC11c2VyOnJvdGF0ZWQtcGFzcw==",
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                let request = read_test_http_request(&mut stream);
                let header_end = request.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                let headers = String::from_utf8_lossy(&request[..header_end]);

                assert!(
                    headers
                        .lines()
                        .any(|line| line.eq_ignore_ascii_case(&format!("authorization: {expected_auth}"))),
                    "expected Authorization header was not sent"
                );

                let body: serde_json::Value = serde_json::from_slice(&request[header_end..]).unwrap();
                assert_eq!(body["method"], "node_version");
                let response =
                    serde_json::json!({"jsonrpc":"2.0","id":body["id"].clone(),"result":"1.4.0"}).to_string();
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    response.len(),
                    response
                )
                .unwrap();
            }
        });

        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let cookie_path = std::env::temp_dir().join(format!("kdf-ml-rpc-cookie-{}-{unique}", std::process::id()));
        fs::write(&cookie_path, "cookie-user:cookie-pass").unwrap();

        let mut request = request_with_urls(vec![concat!("https", "://api.example").into()]);
        request.node_conf = Some(MintlayerNodeClientConfig {
            rpc_url: endpoint,
            rpc_cookie_file: Some(cookie_path.to_string_lossy().into_owned()),
        });
        let coin = MintlayerCoin::new(&test_ctx(), valid_conf(), request, iguana_policy()).unwrap();

        let first_client = coin.node_client().unwrap().unwrap();
        assert_eq!(first_client.node_version().await.unwrap(), "1.4.0");

        fs::write(&cookie_path, "rotated-user:rotated-pass").unwrap();
        let rotated_client = coin.node_client().unwrap().unwrap();
        assert_eq!(rotated_client.node_version().await.unwrap(), "1.4.0");

        fs::write(&cookie_path, "invalid-cookie").unwrap();
        assert!(coin
            .node_client()
            .unwrap_err()
            .contains("Invalid Mintlayer node RPC cookie format"));

        fs::remove_file(cookie_path).unwrap();
        server.join().unwrap();
    }

    #[test]
    fn node_rpc_configuration_is_optional_and_loopback_only() {
        let without_node = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![concat!("https", "://api.example").into()]),
            iguana_policy(),
        )
        .unwrap();
        assert_eq!(without_node.node_rpc_url(), None);
        assert!(without_node.node_client().unwrap().is_none());

        let mut with_node = request_with_urls(vec![concat!("https", "://api.example").into()]);
        with_node.node_conf = Some(MintlayerNodeClientConfig {
            rpc_url: "http://127.0.0.1:3030/".into(),
            rpc_cookie_file: None,
        });
        let with_node = MintlayerCoin::new(&test_ctx(), valid_conf(), with_node, iguana_policy()).unwrap();
        assert_eq!(with_node.node_rpc_url(), Some("http://127.0.0.1:3030"));
        assert!(with_node.node_client().unwrap().is_some());

        for forbidden in [
            "https://127.0.0.1:3030",
            "http://example.com:3030",
            "http://user:pass@127.0.0.1:3030",
        ] {
            let mut request = request_with_urls(vec![concat!("https", "://api.example").into()]);
            request.node_conf = Some(MintlayerNodeClientConfig {
                rpc_url: forbidden.into(),
                rpc_cookie_file: None,
            });
            assert!(MintlayerCoin::new(&test_ctx(), valid_conf(), request, iguana_policy()).is_err());
        }
    }

    #[test]
    fn convert_address_info_to_coin_balance() {
        let address_info = MintlayerAddressInfo {
            coin_balance: crate::mintlayer::MintlayerAmount {
                atoms: "1250000000000".into(),
                decimal: "12.5".into(),
            },
            locked_coin_balance: crate::mintlayer::MintlayerAmount {
                atoms: "250000000000".into(),
                decimal: "2.5".into(),
            },
            transaction_history: Vec::new(),
            tokens: Vec::new(),
        };

        let balance = coin_balance_from_address_info(address_info).unwrap();

        assert_eq!(balance.spendable, "12.5".parse().unwrap());
        assert_eq!(balance.unspendable, "2.5".parse().unwrap());
    }

    #[test]
    fn reject_inconsistent_address_info_balance() {
        let address_info = MintlayerAddressInfo {
            coin_balance: crate::mintlayer::MintlayerAmount {
                atoms: "1250000000000".into(),
                decimal: "12.6".into(),
            },
            locked_coin_balance: crate::mintlayer::MintlayerAmount {
                atoms: "0".into(),
                decimal: "0".into(),
            },
            transaction_history: Vec::new(),
            tokens: Vec::new(),
        };

        assert!(matches!(
            coin_balance_from_address_info(address_info),
            Err(BalanceError::InvalidResponse(_))
        ));
    }

    #[tokio::test]
    async fn mintlayer_validate_fee_accepts_canonical_dex_fee() {
        const BLOCK_HEIGHT: u64 = 700_321;

        let (build_api_listener, build_api_url) = bind_withdraw_mock_api();

        let build_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![build_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let sender = build_coin.address().to_owned();
        let build_api_server = spawn_withdraw_mock_api(build_api_listener, sender.clone());

        let fee_transaction = build_mintlayer_taker_fee(&build_coin, DexFee::Standard("0.0001".into()))
            .await
            .unwrap()
            .expect("standard Mintlayer DEX fee must build a transaction");

        build_api_server.join().unwrap();

        let fee_txid = fee_transaction.transaction_id.clone();
        let block_id = "99".repeat(32);

        let (scanner_listener, scanner_url) = bind_withdraw_mock_api();

        let scanner_txid = fee_txid.clone();
        let scanner_block_id = block_id.clone();
        let scanner_sender = sender.clone();

        let scanner_server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut request_count = 0_usize;

            while request_count < 3 && Instant::now() < deadline {
                match scanner_listener.accept() {
                    Ok((mut stream, _)) => {
                        let path = read_request_path(&mut stream);

                        let body = if path == format!("/api/v2/transaction/{scanner_txid}") {
                            json!({
                                "id": scanner_txid.clone(),
                                "block_id": scanner_block_id.clone(),
                                "inputs": [{
                                    "input": {
                                        "input_type": "UTXO",
                                        "source_type": "Transaction",
                                        "source_id": "11".repeat(32),
                                        "index": 0
                                    },
                                    "utxo": {
                                        "destination": scanner_sender.clone(),
                                        "type": "Transfer",
                                        "value": {
                                            "amount": {
                                                "atoms": "100000000000",
                                                "decimal": "1"
                                            },
                                            "type": "Coin"
                                        }
                                    }
                                }]
                            })
                            .to_string()
                        } else if path == format!("/api/v2/block/{scanner_block_id}") {
                            json!({ "height": BLOCK_HEIGHT }).to_string()
                        } else if path == format!("/api/v2/chain/{BLOCK_HEIGHT}") {
                            json!(scanner_block_id.clone()).to_string()
                        } else {
                            panic!("unexpected DEX-fee happy-path REST request: {}", path);
                        };

                        respond_json(&mut stream, &body);
                        request_count += 1;
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    },
                    Err(error) => panic!("DEX-fee happy-path REST accept failed: {}", error),
                }
            }

            assert_eq!(
                request_count, 3,
                "DEX-fee happy-path REST fixture did not receive all three requests"
            );
        });

        let validate_request = request_with_urls(vec![scanner_url]);
        let validate_coin = MintlayerCoin::new(&test_ctx(), valid_conf(), validate_request, iguana_policy()).unwrap();

        assert_eq!(validate_coin.node_rpc_url(), None);
        assert!(validate_coin.node_client().unwrap().is_none());

        let fee_tx = TransactionEnum::MintlayerTransaction(fee_transaction.clone());
        let dex_fee = DexFee::Standard("0.0001".into());

        SwapOps::validate_fee(
            &validate_coin,
            ValidateFeeArgs {
                fee_tx: &fee_tx,
                expected_sender: build_coin.public_key(),
                dex_fee: &dex_fee,
                min_block_number: BLOCK_HEIGHT,
                uuid: &[0x24; 16],
            },
        )
        .await
        .unwrap();

        scanner_server.join().unwrap();

        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&fee_transaction.signed_bytes).unwrap(),
            fee_transaction.transaction_id
        );
    }

    #[tokio::test]
    async fn mintlayer_validate_fee_retries_transaction_not_found() {
        const BLOCK_HEIGHT: u64 = 700_321;

        let (build_api_listener, build_api_url) = bind_withdraw_mock_api();

        let build_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![build_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let sender = build_coin.address().to_owned();
        let build_api_server = spawn_withdraw_mock_api(build_api_listener, sender.clone());

        let fee_transaction = build_mintlayer_taker_fee(&build_coin, DexFee::Standard("0.0001".into()))
            .await
            .unwrap()
            .expect("standard Mintlayer DEX fee must build a transaction");

        build_api_server.join().unwrap();

        let fee_txid = fee_transaction.transaction_id.clone();
        let block_id = "98".repeat(32);

        let (scanner_listener, scanner_url) = bind_withdraw_mock_api();

        let scanner_txid = fee_txid.clone();
        let scanner_block_id = block_id.clone();
        let scanner_sender = sender.clone();

        let scanner_server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut request_count = 0_usize;
            let mut transaction_requests = 0_usize;

            while request_count < 4 && Instant::now() < deadline {
                match scanner_listener.accept() {
                    Ok((mut stream, _)) => {
                        let path = read_request_path(&mut stream);

                        if path == format!("/api/v2/transaction/{scanner_txid}") {
                            transaction_requests += 1;

                            if transaction_requests == 1 {
                                let body = r#"{"error":"Transaction not found"}"#;
                                let response = format!(
                                    "HTTP/1.1 404 Not Found\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                    body.len(),
                                    body
                                );
                                std::io::Write::write_all(&mut stream, response.as_bytes()).unwrap();
                                std::io::Write::flush(&mut stream).unwrap();
                            } else {
                                let body = json!({
                                    "id": scanner_txid.clone(),
                                    "block_id": scanner_block_id.clone(),
                                    "inputs": [{
                                        "input": {
                                            "input_type": "UTXO",
                                            "source_type": "Transaction",
                                            "source_id": "11".repeat(32),
                                            "index": 0
                                        },
                                        "utxo": {
                                            "destination": scanner_sender.clone(),
                                            "type": "Transfer",
                                            "value": {
                                                "amount": {
                                                    "atoms": "100000000000",
                                                    "decimal": "1"
                                                },
                                                "type": "Coin"
                                            }
                                        }
                                    }]
                                })
                                .to_string();
                                respond_json(&mut stream, &body);
                            }
                        } else if path == format!("/api/v2/block/{scanner_block_id}") {
                            respond_json(&mut stream, &json!({ "height": BLOCK_HEIGHT }).to_string());
                        } else if path == format!("/api/v2/chain/{BLOCK_HEIGHT}") {
                            respond_json(&mut stream, &json!(scanner_block_id.clone()).to_string());
                        } else {
                            panic!("unexpected DEX-fee eventual-visibility REST request: {}", path);
                        }

                        request_count += 1;
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    },
                    Err(error) => panic!("DEX-fee eventual-visibility REST accept failed: {}", error),
                }
            }

            assert_eq!(
                request_count, 4,
                "DEX-fee eventual-visibility fixture did not receive all four requests"
            );
            assert_eq!(
                transaction_requests, 2,
                "DEX-fee transaction lookup was not retried exactly once"
            );
        });

        let validate_request = request_with_urls(vec![scanner_url]);
        let validate_coin = MintlayerCoin::new(&test_ctx(), valid_conf(), validate_request, iguana_policy()).unwrap();

        let fee_tx = TransactionEnum::MintlayerTransaction(fee_transaction.clone());
        let dex_fee = DexFee::Standard("0.0001".into());

        SwapOps::validate_fee(
            &validate_coin,
            ValidateFeeArgs {
                fee_tx: &fee_tx,
                expected_sender: build_coin.public_key(),
                dex_fee: &dex_fee,
                min_block_number: BLOCK_HEIGHT,
                uuid: &[0x26; 16],
            },
        )
        .await
        .unwrap();

        scanner_server.join().unwrap();
    }

    #[tokio::test]
    async fn mintlayer_validate_fee_retries_unconfirmed_transaction() {
        const BLOCK_HEIGHT: u64 = 700_321;

        let (build_api_listener, build_api_url) = bind_withdraw_mock_api();

        let build_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![build_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let sender = build_coin.address().to_owned();
        let build_api_server = spawn_withdraw_mock_api(build_api_listener, sender.clone());

        let fee_transaction = build_mintlayer_taker_fee(&build_coin, DexFee::Standard("0.0001".into()))
            .await
            .unwrap()
            .expect("standard Mintlayer DEX fee must build a transaction");

        build_api_server.join().unwrap();

        let fee_txid = fee_transaction.transaction_id.clone();
        let block_id = "97".repeat(32);

        let (scanner_listener, scanner_url) = bind_withdraw_mock_api();

        let scanner_txid = fee_txid.clone();
        let scanner_block_id = block_id.clone();
        let scanner_sender = sender.clone();

        let scanner_server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut request_count = 0_usize;
            let mut transaction_requests = 0_usize;

            while request_count < 4 && Instant::now() < deadline {
                match scanner_listener.accept() {
                    Ok((mut stream, _)) => {
                        let path = read_request_path(&mut stream);

                        if path == format!("/api/v2/transaction/{scanner_txid}") {
                            transaction_requests += 1;

                            let returned_block_id = if transaction_requests == 1 {
                                String::new()
                            } else {
                                scanner_block_id.clone()
                            };

                            let body = json!({
                                "id": scanner_txid.clone(),
                                "block_id": returned_block_id,
                                "inputs": [{
                                    "input": {
                                        "input_type": "UTXO",
                                        "source_type": "Transaction",
                                        "source_id": "11".repeat(32),
                                        "index": 0
                                    },
                                    "utxo": {
                                        "destination": scanner_sender.clone(),
                                        "type": "Transfer",
                                        "value": {
                                            "amount": {
                                                "atoms": "100000000000",
                                                "decimal": "1"
                                            },
                                            "type": "Coin"
                                        }
                                    }
                                }]
                            })
                            .to_string();
                            respond_json(&mut stream, &body);
                        } else if path == format!("/api/v2/block/{scanner_block_id}") {
                            respond_json(&mut stream, &json!({ "height": BLOCK_HEIGHT }).to_string());
                        } else if path == format!("/api/v2/chain/{BLOCK_HEIGHT}") {
                            respond_json(&mut stream, &json!(scanner_block_id.clone()).to_string());
                        } else {
                            panic!("unexpected DEX-fee unconfirmed REST request: {}", path);
                        }

                        request_count += 1;
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    },
                    Err(error) => panic!("DEX-fee unconfirmed REST accept failed: {}", error),
                }
            }

            assert_eq!(
                request_count, 4,
                "DEX-fee unconfirmed fixture did not receive all four requests"
            );
            assert_eq!(
                transaction_requests, 2,
                "DEX-fee unconfirmed transaction lookup was not retried exactly once"
            );
        });

        let validate_request = request_with_urls(vec![scanner_url]);
        let validate_coin = MintlayerCoin::new(&test_ctx(), valid_conf(), validate_request, iguana_policy()).unwrap();

        let fee_tx = TransactionEnum::MintlayerTransaction(fee_transaction.clone());
        let dex_fee = DexFee::Standard("0.0001".into());

        SwapOps::validate_fee(
            &validate_coin,
            ValidateFeeArgs {
                fee_tx: &fee_tx,
                expected_sender: build_coin.public_key(),
                dex_fee: &dex_fee,
                min_block_number: BLOCK_HEIGHT,
                uuid: &[0x27; 16],
            },
        )
        .await
        .unwrap();

        scanner_server.join().unwrap();
    }

    async fn run_mintlayer_validate_fee_rejection_case(
        scanner_sender: String,
        expected_sender: Vec<u8>,
        expected_fee: DexFee,
        block_height: u64,
        min_block_number: u64,
    ) -> String {
        let (build_api_listener, build_api_url) = bind_withdraw_mock_api();

        let build_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![build_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let sender = build_coin.address().to_owned();
        let build_api_server = spawn_withdraw_mock_api(build_api_listener, sender);

        let fee_transaction = build_mintlayer_taker_fee(&build_coin, DexFee::Standard("0.0001".into()))
            .await
            .unwrap()
            .expect("standard Mintlayer DEX fee must build a transaction");

        build_api_server.join().unwrap();

        let fee_txid = fee_transaction.transaction_id.clone();
        let block_id = "aa".repeat(32);

        let (scanner_listener, scanner_url) = bind_withdraw_mock_api();

        let scanner_txid = fee_txid.clone();
        let scanner_block_id = block_id.clone();

        let scanner_server = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(10);
            let mut request_count = 0_usize;

            while request_count < 3 && Instant::now() < deadline {
                match scanner_listener.accept() {
                    Ok((mut stream, _)) => {
                        let path = read_request_path(&mut stream);

                        let body = if path == format!("/api/v2/transaction/{scanner_txid}") {
                            json!({
                                "id": scanner_txid.clone(),
                                "block_id": scanner_block_id.clone(),
                                "inputs": [{
                                    "input": {
                                        "input_type": "UTXO",
                                        "source_type": "Transaction",
                                        "source_id": "11".repeat(32),
                                        "index": 0
                                    },
                                    "utxo": {
                                        "destination": scanner_sender.clone(),
                                        "type": "Transfer",
                                        "value": {
                                            "amount": {
                                                "atoms": "100000000000",
                                                "decimal": "1"
                                            },
                                            "type": "Coin"
                                        }
                                    }
                                }]
                            })
                            .to_string()
                        } else if path == format!("/api/v2/block/{scanner_block_id}") {
                            json!({ "height": block_height }).to_string()
                        } else if path == format!("/api/v2/chain/{block_height}") {
                            json!(scanner_block_id.clone()).to_string()
                        } else {
                            panic!("unexpected DEX-fee rejection REST request: {}", path);
                        };

                        respond_json(&mut stream, &body);
                        request_count += 1;
                    },
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    },
                    Err(error) => panic!("DEX-fee rejection REST accept failed: {}", error),
                }
            }

            assert_eq!(
                request_count, 3,
                "DEX-fee rejection REST fixture did not receive all three requests"
            );
        });

        let validate_request = request_with_urls(vec![scanner_url]);
        let validate_coin = MintlayerCoin::new(&test_ctx(), valid_conf(), validate_request, iguana_policy()).unwrap();

        assert_eq!(validate_coin.node_rpc_url(), None);
        assert!(validate_coin.node_client().unwrap().is_none());

        let fee_tx = TransactionEnum::MintlayerTransaction(fee_transaction);

        let error = SwapOps::validate_fee(
            &validate_coin,
            ValidateFeeArgs {
                fee_tx: &fee_tx,
                expected_sender: &expected_sender,
                dex_fee: &expected_fee,
                min_block_number,
                uuid: &[0x25; 16],
            },
        )
        .await
        .expect_err("Mintlayer DEX fee validation must reject invalid data");

        scanner_server.join().unwrap();

        match error.into_inner() {
            ValidatePaymentError::WrongPaymentTx(message) => message,
            other => panic!("Mintlayer DEX fee rejection returned unexpected error: {:?}", other),
        }
    }

    #[tokio::test]
    async fn mintlayer_validate_fee_rejects_wrong_sender() {
        let sender_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![concat!("https", "://api.example").into()]),
            iguana_policy(),
        )
        .unwrap();

        let sender = sender_coin.address().to_owned();
        let wrong_secret = [0x73_u8; 32];
        let wrong_key_pair = key_pair_from_secret(&wrong_secret).unwrap();
        let wrong_pubkey = wrong_key_pair.public_slice().to_vec();

        let error = run_mintlayer_validate_fee_rejection_case(
            sender,
            wrong_pubkey,
            DexFee::Standard("0.0001".into()),
            700_321,
            700_321,
        )
        .await;

        assert!(
            error.contains("input belongs to"),
            "unexpected wrong-sender validation error: {}",
            error
        );
    }

    #[tokio::test]
    async fn mintlayer_validate_fee_rejects_insufficient_amount() {
        let sender_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![concat!("https", "://api.example").into()]),
            iguana_policy(),
        )
        .unwrap();

        let sender = sender_coin.address().to_owned();
        let sender_pubkey = sender_coin.public_key().to_vec();

        let error = run_mintlayer_validate_fee_rejection_case(
            sender,
            sender_pubkey,
            DexFee::Standard("0.0002".into()),
            700_321,
            700_321,
        )
        .await;

        assert!(
            error.contains("expected at least"),
            "unexpected insufficient-fee validation error: {}",
            error
        );
    }

    #[tokio::test]
    async fn mintlayer_validate_fee_rejects_old_block() {
        let sender_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![concat!("https", "://api.example").into()]),
            iguana_policy(),
        )
        .unwrap();

        let sender = sender_coin.address().to_owned();
        let sender_pubkey = sender_coin.public_key().to_vec();

        let error = run_mintlayer_validate_fee_rejection_case(
            sender,
            sender_pubkey,
            DexFee::Standard("0.0001".into()),
            700_320,
            700_321,
        )
        .await;

        assert!(
            error.contains("below minimum block"),
            "unexpected old-block validation error: {}",
            error
        );
    }

    #[tokio::test]
    async fn mintlayer_legacy_trade_fee_matches_sender_planner() {
        let representative_amount: BigDecimal = "0.5".parse().unwrap();

        let (expected_api_listener, expected_api_url) = bind_withdraw_mock_api();

        let expected_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![expected_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let expected_sender = expected_coin.address().to_owned();
        let expected_api_server = spawn_withdraw_mock_api(expected_api_listener, expected_sender);

        let expected_fee = mintlayer_sender_trade_fee(&expected_coin, TradePreimageValue::Exact(representative_amount))
            .await
            .unwrap();

        expected_api_server.join().unwrap();

        let (legacy_api_listener, legacy_api_url) = bind_withdraw_mock_api();

        let legacy_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![legacy_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let legacy_sender = legacy_coin.address().to_owned();
        let legacy_api_server = spawn_withdraw_mock_api(legacy_api_listener, legacy_sender);

        let legacy_fee = MmCoin::get_trade_fee(&legacy_coin).compat().await.unwrap();

        legacy_api_server.join().unwrap();

        assert_eq!(legacy_fee.coin, expected_fee.coin);
        assert_eq!(legacy_fee.amount.to_decimal(), expected_fee.amount.to_decimal());
        assert_eq!(legacy_fee.paid_from_trading_vol, expected_fee.paid_from_trading_vol);
        assert!(!legacy_fee.paid_from_trading_vol);
    }

    #[tokio::test]
    async fn mintlayer_receiver_trade_fee_matches_canonical_htlc_spend_fee() {
        const DUMMY_SECRET: [u8; 32] = [0x56; 32];
        const DUMMY_TIME_LOCK: u64 = 1_800_000_000;
        const DUMMY_PAYMENT_ATOMS: u128 = 100_000_000_000;

        let secret_hash = mintlayer_sdk::crypto::types::HtlcSecret::new(DUMMY_SECRET).hash();

        let (plan_api_listener, plan_api_url) = bind_withdraw_mock_api();

        let plan_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![plan_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let plan_api_server = spawn_resolution_mock_api(plan_api_listener);

        let spend_address =
            mintlayer_address_from_compressed_public_key(plan_coin.network(), plan_coin.public_key()).unwrap();

        let refund_address = spend_address.clone();

        let htlc_output = build_mintlayer_htlc_output(
            DUMMY_PAYMENT_ATOMS,
            secret_hash.as_ref(),
            &spend_address,
            &refund_address,
            DUMMY_TIME_LOCK,
            mintlayer_sdk_network(plan_coin.network()),
        )
        .unwrap();

        let dummy_payment_txid = "11".repeat(32);

        let (_transaction, expected_fee_atoms) =
            build_mintlayer_htlc_spend_with_fee(&plan_coin, &dummy_payment_txid, &htlc_output, &DUMMY_SECRET)
                .await
                .unwrap();

        plan_api_server.join().unwrap();

        let (trade_api_listener, trade_api_url) = bind_withdraw_mock_api();

        let trade_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![trade_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let trade_api_server = spawn_resolution_mock_api(trade_api_listener);

        let trade_fee = MmCoin::get_receiver_trade_fee(&trade_coin, FeeApproxStage::WithoutApprox)
            .compat()
            .await
            .unwrap();

        trade_api_server.join().unwrap();

        assert_eq!(trade_fee.coin, trade_coin.ticker());
        assert!(trade_fee.paid_from_trading_vol);

        assert_eq!(
            trade_fee.amount.to_decimal(),
            mintlayer_decimal_from_atoms(expected_fee_atoms)
        );
    }

    #[tokio::test]
    async fn mintlayer_sender_trade_fee_upper_bound_covers_full_balance() {
        // The existing mock provides exactly 1 ML. UpperBound includes the fee;
        // it does not request an exact 1 ML payment plus an additional fee.
        let budget: BigDecimal = "1".parse().unwrap();
        let (api_listener, api_url) = bind_withdraw_mock_api();
        let coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![api_url]),
            iguana_policy(),
        )
        .unwrap();

        let sender = coin.address().to_owned();
        let api_server = spawn_withdraw_mock_api(api_listener, sender);

        let result = MmCoin::get_sender_trade_fee(
            &coin,
            TradePreimageValue::UpperBound(budget.clone()),
            FeeApproxStage::WithoutApprox,
        )
        .await;

        // Join the mock before asserting the result, including on failure.
        api_server.join().expect("AUDIT A1: mock API failed");

        let trade_fee =
            result.expect("AUDIT A1: UpperBound(1 ML) must reserve the fee inside the available 1 ML budget");
        assert_eq!(trade_fee.coin, coin.ticker());
        assert!(!trade_fee.paid_from_trading_vol);

        let network_fee = trade_fee.amount.to_decimal();
        assert!(
            network_fee > BigDecimal::from(0) && network_fee < budget,
            "AUDIT A1: expected a positive fee below the budget; fee={} budget={}",
            network_fee,
            budget
        );

        // KDF will send budget - quoted fee. Its exact planner must reproduce
        // the same canonical fee with the same mock UTXO and fee rate.
        let payment_amount = budget - network_fee.clone();
        let (exact_listener, exact_url) = bind_withdraw_mock_api();
        let exact_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![exact_url]),
            iguana_policy(),
        )
        .unwrap();
        let exact_server = spawn_withdraw_mock_api(exact_listener, exact_coin.address().to_owned());
        let exact_result = MmCoin::get_sender_trade_fee(
            &exact_coin,
            TradePreimageValue::Exact(payment_amount),
            FeeApproxStage::WithoutApprox,
        )
        .await;
        exact_server.join().expect("AUDIT A1: exact mock API failed");
        let exact_fee = exact_result.expect("AUDIT A1: budget minus quoted fee must be fundable");
        assert_eq!(network_fee, exact_fee.amount.to_decimal());
    }

    #[tokio::test]
    async fn mintlayer_sender_trade_fee_upper_bound_split_utxos() {
        let mut previous_fee = BigDecimal::from(0);
        for budget_text in ["0.55", "0.65", "1.2"] {
            let budget: BigDecimal = budget_text.parse().unwrap();
            let (listener, url) = bind_withdraw_mock_api();
            let coin =
                MintlayerCoin::new(&test_ctx(), valid_conf(), request_with_urls(vec![url]), iguana_policy()).unwrap();
            let server = spawn_split_withdraw_mock_api(listener, coin.address().to_owned());
            let quote = MmCoin::get_sender_trade_fee(
                &coin,
                TradePreimageValue::UpperBound(budget.clone()),
                FeeApproxStage::WithoutApprox,
            )
            .await;
            server.join().expect("split quote mock failed");
            let quote = quote.expect("split UTXO budget should be feasible");
            let fee = quote.amount.to_decimal();
            if budget_text == "0.65" {
                assert!(fee <= decimal("0.301"), "split UTXO quote reserves too much: {}", fee);
            }
            assert!(
                fee > BigDecimal::from(0) && fee < budget,
                "budget={}, fee={}",
                budget,
                fee
            );
            assert!(
                previous_fee <= fee,
                "fee fell as budget rose: previous={}, current={}",
                previous_fee,
                fee
            );
            previous_fee = fee.clone();

            let (exact_listener, exact_url) = bind_withdraw_mock_api();
            let exact_coin = MintlayerCoin::new(
                &test_ctx(),
                valid_conf(),
                request_with_urls(vec![exact_url]),
                iguana_policy(),
            )
            .unwrap();
            let exact_server = spawn_split_withdraw_mock_api(exact_listener, exact_coin.address().to_owned());
            let exact = MmCoin::get_sender_trade_fee(
                &exact_coin,
                TradePreimageValue::Exact(budget - fee.clone()),
                FeeApproxStage::WithoutApprox,
            )
            .await;
            exact_server.join().expect("split exact mock failed");
            let exact = exact.expect("budget minus quote should be fundable");
            assert!(
                exact.amount.to_decimal() <= fee,
                "quoted fee must cover the exact fee for {}: quote={}, exact={}",
                budget_text,
                fee,
                exact.amount.to_decimal()
            );
        }
    }

    #[tokio::test]
    async fn mintlayer_sender_trade_fee_upper_bound_rejects_budget_below_fee() {
        let budget: BigDecimal = "0.00000000001".parse().unwrap();
        let (api_listener, api_url) = bind_withdraw_mock_api();
        let coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![api_url]),
            iguana_policy(),
        )
        .unwrap();
        let api_server = spawn_withdraw_mock_api(api_listener, coin.address().to_owned());
        let result = MmCoin::get_sender_trade_fee(
            &coin,
            TradePreimageValue::UpperBound(budget),
            FeeApproxStage::WithoutApprox,
        )
        .await;
        api_server.join().expect("AUDIT A1: small-budget mock API failed");
        assert!(result.is_err(), "fee larger than budget must be rejected");
    }

    #[tokio::test]
    async fn mintlayer_sender_trade_fee_upper_bound_rejects_budget_below_change_fee() {
        // The 1 ML mock UTXO requires a change output for a 0.25 ML budget.
        // Its canonical fee is 0.251 ML, leaving no positive HTLC amount.
        let budget: BigDecimal = "0.25".parse().unwrap();
        let (api_listener, api_url) = bind_withdraw_mock_api();
        let coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![api_url]),
            iguana_policy(),
        )
        .unwrap();
        let api_server = spawn_withdraw_mock_api(api_listener, coin.address().to_owned());
        let result = MmCoin::get_sender_trade_fee(
            &coin,
            TradePreimageValue::UpperBound(budget),
            FeeApproxStage::WithoutApprox,
        )
        .await;
        api_server.join().expect("AUDIT A1: change-fee mock API failed");
        assert!(result.is_err(), "0.25 ML cannot cover the 0.251 ML change-output fee");
    }

    #[tokio::test]
    async fn mintlayer_sender_trade_fee_upper_bound_is_monotonic() {
        let lower_amount: BigDecimal = "0.5".parse().unwrap();
        let upper_amount: BigDecimal = "1".parse().unwrap();

        let (lower_api_listener, lower_api_url) = bind_withdraw_mock_api();

        let lower_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![lower_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let lower_sender = lower_coin.address().to_owned();
        let lower_api_server = spawn_withdraw_mock_api(lower_api_listener, lower_sender);

        let lower_fee = MmCoin::get_sender_trade_fee(
            &lower_coin,
            TradePreimageValue::UpperBound(lower_amount),
            FeeApproxStage::WithoutApprox,
        )
        .await
        .unwrap();

        lower_api_server.join().unwrap();

        let (upper_api_listener, upper_api_url) = bind_withdraw_mock_api();

        let upper_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![upper_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let upper_sender = upper_coin.address().to_owned();
        let upper_api_server = spawn_withdraw_mock_api(upper_api_listener, upper_sender);

        let upper_fee = MmCoin::get_sender_trade_fee(
            &upper_coin,
            TradePreimageValue::UpperBound(upper_amount),
            FeeApproxStage::WithoutApprox,
        )
        .await
        .unwrap();

        upper_api_server.join().unwrap();

        assert_eq!(lower_fee.coin, lower_coin.ticker());
        assert_eq!(upper_fee.coin, upper_coin.ticker());
        assert!(!lower_fee.paid_from_trading_vol);
        assert!(!upper_fee.paid_from_trading_vol);

        assert!(
            lower_fee.amount.to_decimal() <= upper_fee.amount.to_decimal(),
            "Mintlayer sender trade fee must be monotonic: lower={} upper={}",
            lower_fee.amount.to_decimal(),
            upper_fee.amount.to_decimal()
        );
    }

    #[tokio::test]
    async fn mintlayer_sender_trade_fee_exact_matches_canonical_planner_fee() {
        const DUMMY_SECRET_HASH: [u8; 20] = [0x42; 20];
        const DUMMY_TIME_LOCK: u64 = 1_800_000_000;
        const DUMMY_SWAP_UNIQUE_DATA: &[u8] = b"mintlayer-trade-fee";

        let amount: BigDecimal = "0.5".parse().unwrap();

        let other_secret = [0x73_u8; 32];
        let other_key_pair = key_pair_from_secret(&other_secret).unwrap();
        let other_pubkey = other_key_pair.public_slice();

        let (plan_api_listener, plan_api_url) = bind_withdraw_mock_api();

        let plan_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![plan_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let plan_sender = plan_coin.address().to_owned();
        let plan_api_server = spawn_withdraw_mock_api(plan_api_listener, plan_sender);

        let (_transaction, expected_fee_atoms) = build_mintlayer_swap_payment_with_fee(
            &plan_coin,
            &amount,
            other_pubkey,
            &DUMMY_SECRET_HASH,
            DUMMY_TIME_LOCK,
            DUMMY_SWAP_UNIQUE_DATA,
        )
        .await
        .unwrap();

        plan_api_server.join().unwrap();

        let (trade_api_listener, trade_api_url) = bind_withdraw_mock_api();

        let trade_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![trade_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let trade_sender = trade_coin.address().to_owned();
        let trade_api_server = spawn_withdraw_mock_api(trade_api_listener, trade_sender);

        let trade_fee = MmCoin::get_sender_trade_fee(
            &trade_coin,
            TradePreimageValue::Exact(amount),
            FeeApproxStage::WithoutApprox,
        )
        .await
        .unwrap();

        trade_api_server.join().unwrap();

        assert_eq!(trade_fee.coin, trade_coin.ticker());
        assert!(!trade_fee.paid_from_trading_vol);

        assert_eq!(
            trade_fee.amount.to_decimal(),
            mintlayer_decimal_from_atoms(expected_fee_atoms)
        );
    }

    #[tokio::test]
    async fn mintlayer_sender_trade_fee_respects_fee_approx_stage() {
        async fn quote(stage: FeeApproxStage) -> BigDecimal {
            let amount: BigDecimal = "0.5".parse().unwrap();
            let (api_listener, api_url) = bind_withdraw_mock_api();

            let coin = MintlayerCoin::new(
                &test_ctx(),
                valid_conf(),
                request_with_urls(vec![api_url]),
                iguana_policy(),
            )
            .unwrap();

            let sender = coin.address().to_owned();
            let api_server = spawn_withdraw_mock_api(api_listener, sender);

            let trade_fee = MmCoin::get_sender_trade_fee(&coin, TradePreimageValue::Exact(amount), stage)
                .await
                .unwrap();

            api_server.join().unwrap();
            trade_fee.amount.to_decimal()
        }

        let without = quote(FeeApproxStage::WithoutApprox).await;
        let start = quote(FeeApproxStage::StartSwap).await;
        let order = quote(FeeApproxStage::OrderIssue).await;
        let preimage = quote(FeeApproxStage::TradePreimage).await;

        assert!(
            without < start,
            "AUDIT-C3 RED: StartSwap must reserve more than WithoutApprox: without={}, start={}",
            without,
            start
        );
        assert!(
            start < order,
            "AUDIT-C3 RED: OrderIssue must reserve more than StartSwap: start={}, order={}",
            start,
            order
        );
        assert!(
            order < preimage,
            "AUDIT-C3 RED: TradePreimage must reserve more than OrderIssue: order={}, preimage={}",
            order,
            preimage
        );
    }

    #[tokio::test]
    async fn mintlayer_taker_fee_trade_fee_respects_fee_approx_stage() {
        async fn quote(stage: FeeApproxStage) -> BigDecimal {
            let (api_listener, api_url) = bind_withdraw_mock_api();

            let coin = MintlayerCoin::new(
                &test_ctx(),
                valid_conf(),
                request_with_urls(vec![api_url]),
                iguana_policy(),
            )
            .unwrap();

            let sender = coin.address().to_owned();
            let api_server = spawn_withdraw_mock_api(api_listener, sender);

            let trade_fee = MmCoin::get_fee_to_send_taker_fee(&coin, DexFee::Standard("0.0001".into()), stage)
                .await
                .unwrap();

            api_server.join().unwrap();
            trade_fee.amount.to_decimal()
        }

        let without = quote(FeeApproxStage::WithoutApprox).await;
        let start = quote(FeeApproxStage::StartSwap).await;

        assert!(
            without < start,
            "AUDIT-C3 RED: StartSwap taker-fee network reserve must exceed WithoutApprox: without={}, start={}",
            without,
            start
        );
    }

    #[tokio::test]
    async fn mintlayer_taker_fee_trade_fee_matches_canonical_planner_fee() {
        let (plan_api_listener, plan_api_url) = bind_withdraw_mock_api();

        let plan_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![plan_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let plan_sender = plan_coin.address().to_owned();
        let plan_api_server = spawn_withdraw_mock_api(plan_api_listener, plan_sender);

        let (_transaction, expected_fee_atoms) =
            build_mintlayer_taker_fee_with_fee(&plan_coin, DexFee::Standard("0.0001".into()))
                .await
                .unwrap()
                .expect("standard Mintlayer DEX fee must produce a transaction");

        plan_api_server.join().unwrap();

        let (trade_api_listener, trade_api_url) = bind_withdraw_mock_api();

        let trade_coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![trade_api_url]),
            iguana_policy(),
        )
        .unwrap();

        let trade_sender = trade_coin.address().to_owned();
        let trade_api_server = spawn_withdraw_mock_api(trade_api_listener, trade_sender);

        let trade_fee = MmCoin::get_fee_to_send_taker_fee(
            &trade_coin,
            DexFee::Standard("0.0001".into()),
            FeeApproxStage::WithoutApprox,
        )
        .await
        .unwrap();

        trade_api_server.join().unwrap();

        assert_eq!(trade_fee.coin, trade_coin.ticker());
        assert!(!trade_fee.paid_from_trading_vol);

        assert_eq!(
            trade_fee.amount.to_decimal(),
            mintlayer_decimal_from_atoms(expected_fee_atoms)
        );
    }

    #[tokio::test]
    async fn mintlayer_send_taker_fee_builds_and_broadcasts_canonical_transaction() {
        let (api_listener, api_url) = bind_withdraw_mock_api();
        let request = request_with_urls(vec![api_url]);
        let coin = MintlayerCoin::new(&test_ctx(), valid_conf(), request, iguana_policy()).unwrap();
        assert_eq!(coin.node_rpc_url(), None);

        let sender = coin.address().to_owned();
        let api_server = spawn_withdraw_and_transaction_capture_api(api_listener, sender);

        let result = SwapOps::send_taker_fee(&coin, DexFee::Standard("0.0001".into()), &[0x42; 16], now_sec() + 60)
            .await
            .unwrap();

        let broadcast_bytes = api_server.join().unwrap();

        let transaction = match result {
            TransactionEnum::MintlayerTransaction(transaction) => transaction,
            other => panic!("expected Mintlayer transaction from send_taker_fee, got {:?}", other),
        };

        assert_eq!(broadcast_bytes, transaction.signed_bytes);

        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&broadcast_bytes).unwrap(),
            transaction.transaction_id
        );
    }

    #[tokio::test]
    async fn mintlayer_taker_fee_builder_creates_dex_transfer() {
        use mintlayer_sdk::crypto::types::{OutputValue, TxOutput};

        const FEE_ATOMS: u128 = 10_000_000;

        let (api_listener, api_url) = bind_withdraw_mock_api();

        let coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![api_url]),
            iguana_policy(),
        )
        .unwrap();

        let sender = coin.address().to_owned();
        let api_server = spawn_withdraw_mock_api(api_listener, sender);

        let transaction = build_mintlayer_taker_fee(&coin, DexFee::Standard("0.0001".into()))
            .await
            .unwrap()
            .expect("standard Mintlayer DEX fee must build a transaction");

        api_server.join().unwrap();

        assert_eq!(
            canonical_transaction_id_from_signed_bytes(&transaction.signed_bytes).unwrap(),
            transaction.transaction_id
        );

        let decoded = mintlayer_sdk::crypto::decode_transaction_lenient(&transaction.signed_bytes).unwrap();

        let dex_fee_address = mintlayer_dex_fee_address(&coin).unwrap();
        let expected_destination =
            mintlayer_sdk::crypto::encode_destination(&dex_fee_address, mintlayer_sdk_network(coin.network())).unwrap();

        match &decoded.outputs()[0] {
            TxOutput::Transfer(OutputValue::Coin(amount), destination) => {
                assert_eq!(amount.into_atoms(), FEE_ATOMS);
                assert_eq!(destination, &expected_destination);
            },
            other => panic!(
                "expected Mintlayer DEX fee output 0 to be a coin transfer, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn converts_mintlayer_decimal_amounts_to_atoms_exactly() {
        let one: BigDecimal = "1".parse().unwrap();
        let fractional: BigDecimal = "1.23456789012".parse().unwrap();
        let trailing_zero: BigDecimal = "0.100000000000".parse().unwrap();
        let too_precise: BigDecimal = "0.000000000001".parse().unwrap();

        assert_eq!(mintlayer_atoms_from_decimal(&one).unwrap(), 100_000_000_000);
        assert_eq!(mintlayer_atoms_from_decimal(&fractional).unwrap(), 123_456_789_012);
        assert_eq!(mintlayer_atoms_from_decimal(&trailing_zero).unwrap(), 10_000_000_000);
        assert!(mintlayer_atoms_from_decimal(&too_precise).is_err());
    }

    #[test]
    fn maps_every_kdf_mintlayer_network_to_sdk() {
        assert_eq!(mintlayer_sdk_network(MintlayerNetwork::Mainnet), SdkNetwork::Mainnet);
        assert_eq!(mintlayer_sdk_network(MintlayerNetwork::Testnet), SdkNetwork::Testnet);
        assert_eq!(mintlayer_sdk_network(MintlayerNetwork::Regtest), SdkNetwork::Regtest);
        assert_eq!(mintlayer_sdk_network(MintlayerNetwork::Signet), SdkNetwork::Signet);
    }

    #[test]
    fn mmcoin_wallet_only_follows_kdf_config() {
        // A coin missing from KDF's configured `coins` list must remain wallet-only.
        let missing_ctx = test_ctx();
        let missing_coin = MintlayerCoin::new(
            &missing_ctx,
            valid_conf(),
            request_with_urls(vec![concat!("https", "://api.example").into()]),
            iguana_policy(),
        )
        .unwrap();
        assert!(MmCoin::wallet_only(&missing_coin, &missing_ctx));

        // A configured ML coin without an explicit wallet_only flag is tradable by default.
        let trading_ctx = MmCtxBuilder::new()
            .with_conf(serde_json::json!({
                "coins": [{
                    "coin": "ML"
                }]
            }))
            .into_mm_arc();
        let trading_coin = MintlayerCoin::new(
            &trading_ctx,
            valid_conf(),
            request_with_urls(vec![concat!("https", "://api.example").into()]),
            iguana_policy(),
        )
        .unwrap();
        assert!(
            !MmCoin::wallet_only(&trading_coin, &trading_ctx),
            "GATE-1 RED: configured Mintlayer without wallet_only=true must be tradable"
        );

        // Explicit wallet_only=true must continue to disable swap participation.
        let wallet_only_ctx = MmCtxBuilder::new()
            .with_conf(serde_json::json!({
                "coins": [{
                    "coin": "ML",
                    "wallet_only": true
                }]
            }))
            .into_mm_arc();
        let wallet_only_coin = MintlayerCoin::new(
            &wallet_only_ctx,
            valid_conf(),
            request_with_urls(vec![concat!("https", "://api.example").into()]),
            iguana_policy(),
        )
        .unwrap();
        assert!(MmCoin::wallet_only(&wallet_only_coin, &wallet_only_ctx));
    }

    #[test]
    fn mmcoin_protocol_support_accepts_native_swap_negotiation() {
        let ctx = test_ctx();
        let coin = MintlayerCoin::new(
            &ctx,
            valid_conf(),
            request_with_urls(vec![concat!("https", "://api.example").into()]),
            iguana_policy(),
        )
        .unwrap();

        // Mintlayer currently has no versioned/optional protocol payload:
        // coin_protocol_info() is empty, like other KDF-native swap coins that
        // simply report protocol support as true.
        assert!(
            MmCoin::is_coin_protocol_supported(&coin, &None, None, 0, false),
            "GATE-2 RED: Mintlayer must accept native swap negotiation as taker"
        );

        let empty_info = Some(Vec::new());
        assert!(
            MmCoin::is_coin_protocol_supported(&coin, &empty_info, None, 3_600, true),
            "GATE-2 RED: Mintlayer must accept native swap negotiation as maker"
        );
    }

    #[test]
    fn mmcoin_validates_local_address() {
        let ctx = test_ctx();
        let coin = MintlayerCoin::new(
            &ctx,
            valid_conf(),
            request_with_urls(vec![concat!("https", "://api.example").into()]),
            iguana_policy(),
        )
        .unwrap();

        let valid = MmCoin::validate_address(&coin, coin.address());
        assert!(valid.is_valid);
        assert!(valid.reason.is_none());

        let invalid = MmCoin::validate_address(&coin, "not-a-mintlayer-address");
        assert!(!invalid.is_valid);
        assert!(invalid.reason.is_some());
    }

    #[test]
    fn htlc_public_key_matches_local_identity() {
        let ctx = test_ctx();
        let coin = MintlayerCoin::new(
            &ctx,
            valid_conf(),
            request_with_urls(vec![concat!("https", "://api.example").into()]),
            iguana_policy(),
        )
        .unwrap();

        let htlc_public_key = SwapOps::derive_htlc_pubkey(&coin, b"test-swap");
        assert_eq!(htlc_public_key.as_slice(), coin.public_key());
        assert!(SwapOps::validate_other_pubkey(&coin, &htlc_public_key).is_ok());
        assert!(SwapOps::validate_other_pubkey(&coin, &[0_u8; 32]).is_err());
    }

    #[test]
    fn build_valid_coin() {
        let request = request_with_urls(vec![
            concat!("https", "://api.example").into(),
            concat!("http", "://127.0.0.1:3000").into(),
        ]);

        let coin = MintlayerCoin::new(&test_ctx(), valid_conf(), request, iguana_policy()).unwrap();

        assert_eq!(coin.ticker(), "ML");
        assert_eq!(coin.network(), MintlayerNetwork::Mainnet);
        assert_eq!(coin.decimals(), MINTLAYER_DECIMALS);
        assert_eq!(coin.required_confirmations(), 2);
        assert_eq!(coin.api_urls().len(), 2);
        assert_eq!(coin.api_client().api_urls(), coin.api_urls());
        assert_eq!(
            hex::encode(coin.public_key()),
            "031b84c5567b126440995d3ed5aaba0565d71e1834604819ff9c17f5e9d5dd078f"
        );
        assert_eq!(
            coin.address(),
            mintlayer_address_from_compressed_public_key(MintlayerNetwork::Mainnet, coin.public_key()).unwrap()
        );
        assert!(matches!(coin.derivation_method(), DerivationMethodResponse::Iguana));
    }

    #[test]
    fn build_global_hd_identity_at_default_address_path() {
        let ctx = MmCtxBuilder::default().into_mm_arc();
        let crypto_ctx = CryptoCtx::init_with_global_hd_account(ctx.clone(), TEST_MNEMONIC).unwrap();
        let global_hd_account = match crypto_ctx.key_pair_policy() {
            KeyPairPolicy::GlobalHDAccount(account) => account.clone(),
            KeyPairPolicy::Iguana => panic!("expected GlobalHDAccount policy"),
        };

        let expected_path = mintlayer_derivation_path(MintlayerNetwork::Mainnet, 0, Bip44Chain::External, 0).unwrap();
        let expected_secret = global_hd_account.derive_secp256k1_secret(&expected_path).unwrap();
        let expected_key_pair = key_pair_from_secret(&expected_secret.take()).unwrap();
        let expected_address =
            mintlayer_address_from_compressed_public_key(MintlayerNetwork::Mainnet, expected_key_pair.public_slice())
                .unwrap();

        let coin = MintlayerCoin::new(
            &ctx,
            valid_conf(),
            request_with_urls(vec![concat!("https", "://api.example").into()]),
            PrivKeyBuildPolicy::GlobalHDAccount(global_hd_account),
        )
        .unwrap();

        assert_eq!(coin.public_key(), expected_key_pair.public_slice());
        assert_eq!(coin.address(), expected_address);
        assert!(matches!(
            coin.derivation_method(),
            DerivationMethodResponse::HDWallet(path) if path == &expected_path.to_string()
        ));
    }

    #[test]
    fn debug_redacts_private_key() {
        let coin = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![concat!("https", "://api.example").into()]),
            iguana_policy(),
        )
        .unwrap();

        let debug = format!("{coin:?}");

        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains(&"01".repeat(32)));
    }

    #[test]
    fn reject_trezor_private_key_policy() {
        let result = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![concat!("https", "://api.example").into()]),
            PrivKeyBuildPolicy::Trezor,
        );

        assert!(matches!(
            result,
            Err(MintlayerCoinBuildError::UnsupportedPrivKeyPolicy { policy: "Trezor" })
        ));
    }

    #[test]
    fn activation_confirmations_override_configuration() {
        let mut request = request_with_urls(vec![concat!("https", "://api.example").into()]);
        request.required_confirmations = Some(5);

        let coin = MintlayerCoin::new(&test_ctx(), valid_conf(), request, iguana_policy()).unwrap();

        assert_eq!(coin.required_confirmations(), 5);
    }

    #[test]
    fn reject_zero_confirmations_in_setter() {
        let request = request_with_urls(vec![concat!("https", "://api.example").into()]);
        let coin = MintlayerCoin::new(&test_ctx(), valid_conf(), request, iguana_policy()).unwrap();

        let result = coin.set_required_confirmations(0);

        assert_eq!(result, Err(MintlayerCoinBuildError::InvalidRequiredConfirmations));
        assert_eq!(coin.required_confirmations(), 2);
    }

    #[test]
    fn accept_matching_api_genesis_block_id_case_insensitively() {
        let expected = "abcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcdefabcd";
        let actual = expected.to_uppercase();

        assert!(validate_api_genesis_block_id(expected, &actual).is_ok());
    }

    #[test]
    fn reject_invalid_api_genesis_block_id() {
        let expected = "a".repeat(64);

        let error = validate_api_genesis_block_id(&expected, "not-a-block-id").unwrap_err();

        assert!(matches!(
            error,
            MintlayerNetworkValidationError::InvalidGenesisBlockId { .. }
        ));
    }

    #[test]
    fn reject_unexpected_api_genesis_block_id() {
        let expected = "a".repeat(64);
        let actual = "b".repeat(64);

        let error = validate_api_genesis_block_id(&expected, &actual).unwrap_err();

        assert!(matches!(
            error,
            MintlayerNetworkValidationError::UnexpectedGenesisBlockId {
                expected: error_expected,
                actual: error_actual,
            } if error_expected == expected && error_actual == actual
        ));
    }

    #[test]
    fn reject_invalid_genesis_block_id() {
        let mut conf = valid_conf();
        conf.genesis_block_id = "not-a-block-id".into();

        let result = MintlayerCoin::new(
            &test_ctx(),
            conf,
            request_with_urls(vec![concat!("https", "://api.example").into()]),
            iguana_policy(),
        );

        assert!(matches!(result, Err(MintlayerCoinBuildError::InvalidGenesisBlockId)));
    }

    #[test]
    fn reject_remote_plain_http() {
        let result = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![concat!("http", "://api.example").into()]),
            iguana_policy(),
        );

        assert!(matches!(
            result,
            Err(MintlayerCoinBuildError::InsecureRemoteApiUrl { index: 0 })
        ));
    }

    #[test]
    fn reject_url_credentials() {
        let result = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![concat!("https", "://user:password@api.example").into()]),
            iguana_policy(),
        );

        assert!(matches!(
            result,
            Err(MintlayerCoinBuildError::ApiUrlCredentialsNotAllowed { index: 0 })
        ));
    }

    #[test]
    fn reject_duplicate_urls_after_normalization() {
        let result = MintlayerCoin::new(
            &test_ctx(),
            valid_conf(),
            request_with_urls(vec![
                concat!("https", "://api.example").into(),
                concat!("https", "://api.example/").into(),
            ]),
            iguana_policy(),
        );

        assert!(matches!(
            result,
            Err(MintlayerCoinBuildError::DuplicateApiUrl { index: 1 })
        ));
    }
}
