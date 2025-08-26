use crate::coin_errors::{AddressFromPubkeyError, MyAddressError, ValidatePaymentError, ValidatePaymentResult};
use crate::eth::{self, u256_to_big_decimal, wei_from_big_decimal, TryToAddress};
use crate::hd_wallet::HDAddressSelector;
use crate::qrc20::rpc_clients::{
    LogEntry, Qrc20ElectrumOps, Qrc20NativeOps, Qrc20RpcOps, TopicFilter, TxReceipt, ViewContractCallType,
};
use crate::utxo::qtum::QtumBasedCoin;
use crate::utxo::rpc_clients::{
    ElectrumClient, NativeClient, UnspentInfo, UtxoRpcClientEnum, UtxoRpcClientOps, UtxoRpcError, UtxoRpcFut,
    UtxoRpcResult,
};
#[cfg(not(target_arch = "wasm32"))]
use crate::utxo::tx_cache::{UtxoVerboseCacheOps, UtxoVerboseCacheShared};
use crate::utxo::utxo_builder::{
    build_utxo_fields_with_global_hd, build_utxo_fields_with_iguana_priv_key, UtxoCoinBuildError, UtxoCoinBuildResult,
    UtxoCoinBuilder, UtxoCoinBuilderCommonOps,
};
use crate::utxo::utxo_common::{self, big_decimal_from_sat, check_all_utxo_inputs_signed_by_pub, UtxoTxBuilder};
use crate::utxo::{
    qtum, ActualFeeRate, AddrFromStrError, BroadcastTxErr, FeePolicy, GenerateTxError, GetUtxoListOps, HistoryUtxoTx,
    HistoryUtxoTxMap, MatureUnspentList, RecentlySpentOutPointsGuard, UnsupportedAddr, UtxoActivationParams,
    UtxoAddressFormat, UtxoCoinFields, UtxoCommonOps, UtxoFromLegacyReqErr, UtxoTx, UtxoTxBroadcastOps,
    UtxoTxGenerationOps, VerboseTransactionFrom, UTXO_LOCK,
};
use crate::{
    BalanceError, BalanceFut, CheckIfMyPaymentSentArgs, CoinBalance, ConfirmPaymentInput, DexFee, Eip1559Ops,
    FeeApproxStage, FoundSwapTxSpend, HistorySyncState, IguanaPrivKey, MarketCoinOps, MmCoin,
    NegotiateSwapContractAddrErr, PrivKeyBuildPolicy, PrivKeyPolicyNotAllowed, RawTransactionFut,
    RawTransactionRequest, RawTransactionResult, RefundPaymentArgs, SearchForSwapTxSpendInput, SendPaymentArgs,
    SignRawTransactionRequest, SignatureResult, SpendPaymentArgs, SwapOps, SwapTxFeePolicy, TradeFee,
    TradePreimageError, TradePreimageFut, TradePreimageResult, TradePreimageValue, TransactionData, TransactionDetails,
    TransactionEnum, TransactionErr, TransactionResult, TransactionType, TxMarshalingErr, UnexpectedDerivationMethod,
    ValidateAddressResult, ValidateFeeArgs, ValidateOtherPubKeyErr, ValidatePaymentInput, VerificationResult,
    WaitForHTLCTxSpendArgs, WatcherOps, WeakSpawner, WithdrawError, WithdrawFee, WithdrawFut, WithdrawRequest,
    WithdrawResult,
};
use async_trait::async_trait;
use bitcrypto::{dhash160, sha256, sign_message_hash};
use chain::TransactionOutput;
use common::executor::{AbortableSystem, AbortedError, Timer};
use common::jsonrpc_client::{JsonRpcClient, JsonRpcRequest, RpcRes};
use common::log::{error, warn};
use common::{now_sec, now_sec_u32};
use derive_more::Display;
use ethabi::{Function, Token};
use ethereum_types::{H160, U256};
use futures::compat::Future01CompatExt;
use futures::{FutureExt, TryFutureExt};
use futures01::Future;
use keys::bytes::Bytes as ScriptBytes;
use keys::{Address as UtxoAddress, KeyPair, Public};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::{BigDecimal, MmNumber};
#[cfg(test)]
use mocktopus::macros::*;
use rpc::v1::types::{
    Bytes as BytesJson, ToTxHash, Transaction as RpcTransaction, H160 as H160Json, H256 as H256Json, H264 as H264Json,
};
use script::{Builder as ScriptBuilder, Opcode, Script, TransactionInputSigner};
use script_pubkey::generate_contract_call_script_pubkey;
use serde_json::{self as json, Value as Json};
use serialization::{deserialize, serialize, CoinVariant};
use std::collections::{HashMap, HashSet};
use std::convert::TryInto;
use std::num::TryFromIntError;
use std::ops::{Deref, Neg};
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;
use utxo_signer::with_key_pair::{sign_tx, UtxoSignWithKeyPairError};

mod history;
#[cfg(test)]
mod qrc20_tests;
pub mod rpc_clients;
pub mod script_pubkey;
mod swap;

/// Qtum amount is always 0 for the QRC20 UTXO outputs,
/// because we should pay only a fee in Qtum to send the QRC20 transaction.
pub const OUTPUT_QTUM_AMOUNT: u64 = 0;
pub const QRC20_GAS_LIMIT_DEFAULT: u64 = 100_000;
const QRC20_PAYMENT_GAS_LIMIT: u64 = 200_000;
pub const QRC20_GAS_PRICE_DEFAULT: u64 = 40;
pub const QRC20_DUST: u64 = 0;
// Keccak-256 hash of `Transfer` event
const QRC20_TRANSFER_TOPIC: &str = "ddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef";
const QRC20_PAYMENT_SENT_TOPIC: &str = "ccc9c05183599bd3135da606eaaf535daffe256e9de33c048014cffcccd4ad57";
const QRC20_RECEIVER_SPENT_TOPIC: &str = "36c177bcb01c6d568244f05261e2946c8c977fa50822f3fa098c470770ee1f3e";
const QRC20_SENDER_REFUNDED_TOPIC: &str = "1797d500133f8e427eb9da9523aa4a25cb40f50ebc7dbda3c7c81778973f35ba";

pub type Qrc20AbiResult<T> = Result<T, MmError<Qrc20AbiError>>;

#[derive(Display)]
pub enum Qrc20GenTxError {
    ErrorGeneratingUtxoTx(GenerateTxError),
    ErrorSigningTx(UtxoSignWithKeyPairError),
    PrivKeyPolicyNotAllowed(PrivKeyPolicyNotAllowed),
    UnexpectedDerivationMethod(UnexpectedDerivationMethod),
    InvalidAddress(String),
}

impl From<GenerateTxError> for Qrc20GenTxError {
    fn from(e: GenerateTxError) -> Self {
        Qrc20GenTxError::ErrorGeneratingUtxoTx(e)
    }
}

impl From<UtxoSignWithKeyPairError> for Qrc20GenTxError {
    fn from(e: UtxoSignWithKeyPairError) -> Self {
        Qrc20GenTxError::ErrorSigningTx(e)
    }
}

impl From<PrivKeyPolicyNotAllowed> for Qrc20GenTxError {
    fn from(e: PrivKeyPolicyNotAllowed) -> Self {
        Qrc20GenTxError::PrivKeyPolicyNotAllowed(e)
    }
}

impl From<UnexpectedDerivationMethod> for Qrc20GenTxError {
    fn from(e: UnexpectedDerivationMethod) -> Self {
        Qrc20GenTxError::UnexpectedDerivationMethod(e)
    }
}

impl From<UtxoRpcError> for Qrc20GenTxError {
    fn from(e: UtxoRpcError) -> Self {
        Qrc20GenTxError::ErrorGeneratingUtxoTx(GenerateTxError::from(e))
    }
}

impl Qrc20GenTxError {
    fn into_withdraw_error(self, coin: String, decimals: u8) -> WithdrawError {
        match self {
            Qrc20GenTxError::ErrorGeneratingUtxoTx(gen_err) => {
                WithdrawError::from_generate_tx_error(gen_err, coin, decimals)
            },
            Qrc20GenTxError::ErrorSigningTx(sign_err) => WithdrawError::InternalError(sign_err.to_string()),
            Qrc20GenTxError::PrivKeyPolicyNotAllowed(priv_err) => WithdrawError::InternalError(priv_err.to_string()),
            Qrc20GenTxError::UnexpectedDerivationMethod(addr_err) => WithdrawError::InternalError(addr_err.to_string()),
            Qrc20GenTxError::InvalidAddress(addr_err) => WithdrawError::InvalidAddress(addr_err),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Qrc20ActivationParams {
    swap_contract_address: H160,
    fallback_swap_contract: Option<H160>,
    #[serde(flatten)]
    utxo_params: UtxoActivationParams,
}

#[derive(Debug, Display)]
pub enum Qrc20FromLegacyReqErr {
    InvalidSwapContractAddr(json::Error),
    InvalidFallbackSwapContract(json::Error),
    InvalidUtxoParams(UtxoFromLegacyReqErr),
}

impl From<UtxoFromLegacyReqErr> for Qrc20FromLegacyReqErr {
    fn from(err: UtxoFromLegacyReqErr) -> Self {
        Qrc20FromLegacyReqErr::InvalidUtxoParams(err)
    }
}

impl Qrc20ActivationParams {
    pub fn from_legacy_req(req: &Json) -> Result<Self, MmError<Qrc20FromLegacyReqErr>> {
        let swap_contract_address = json::from_value(req["swap_contract_address"].clone())
            .map_to_mm(Qrc20FromLegacyReqErr::InvalidSwapContractAddr)?;
        let fallback_swap_contract = json::from_value(req["fallback_swap_contract"].clone())
            .map_to_mm(Qrc20FromLegacyReqErr::InvalidFallbackSwapContract)?;
        let utxo_params = UtxoActivationParams::from_legacy_req(req).map_mm_err()?;
        Ok(Qrc20ActivationParams {
            swap_contract_address,
            fallback_swap_contract,
            utxo_params,
        })
    }
}

struct Qrc20CoinBuilder<'a> {
    ctx: &'a MmArc,
    ticker: &'a str,
    conf: &'a Json,
    activation_params: &'a Qrc20ActivationParams,
    priv_key_policy: PrivKeyBuildPolicy,
    platform: String,
    token_contract_address: H160,
}

impl<'a> Qrc20CoinBuilder<'a> {
    pub fn new(
        ctx: &'a MmArc,
        ticker: &'a str,
        conf: &'a Json,
        activation_params: &'a Qrc20ActivationParams,
        priv_key_policy: PrivKeyBuildPolicy,
        platform: String,
        token_contract_address: H160,
    ) -> Qrc20CoinBuilder<'a> {
        Qrc20CoinBuilder {
            ctx,
            ticker,
            conf,
            activation_params,
            priv_key_policy,
            platform,
            token_contract_address,
        }
    }
}

#[async_trait]
impl UtxoCoinBuilderCommonOps for Qrc20CoinBuilder<'_> {
    fn ctx(&self) -> &MmArc {
        self.ctx
    }

    fn conf(&self) -> &Json {
        self.conf
    }

    fn activation_params(&self) -> &UtxoActivationParams {
        &self.activation_params.utxo_params
    }

    fn ticker(&self) -> &str {
        self.ticker
    }

    async fn decimals(&self, rpc_client: &UtxoRpcClientEnum) -> UtxoCoinBuildResult<u8> {
        if let Some(d) = self.conf()["decimals"].as_u64() {
            return Ok(d as u8);
        }

        rpc_client
            .token_decimals(&self.token_contract_address)
            .compat()
            .await
            .map_to_mm(UtxoCoinBuildError::ErrorDetectingDecimals)
    }

    fn dust_amount(&self) -> u64 {
        QRC20_DUST
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn confpath(&self) -> UtxoCoinBuildResult<PathBuf> {
        use crate::utxo::coin_daemon_data_dir;

        // Documented at https://github.com/jl777/coins#bitcoin-protocol-specific-json
        // "USERHOME/" prefix should be replaced with the user's home folder.
        let declared_confpath = match self.conf()["confpath"].as_str() {
            Some(path) if !path.is_empty() => path.trim(),
            _ => {
                let is_asset_chain = false;
                let platform = self.platform.to_lowercase();
                let data_dir = coin_daemon_data_dir(&platform, is_asset_chain);

                let confname = format!("{platform}.conf");
                return Ok(data_dir.join(&confname[..]));
            },
        };

        let (confpath, rel_to_home) = match declared_confpath.strip_prefix("~/") {
            Some(stripped) => (stripped, true),
            None => match declared_confpath.strip_prefix("USERHOME/") {
                Some(stripped) => (stripped, true),
                None => (declared_confpath, false),
            },
        };

        if rel_to_home {
            let home = std::env::home_dir().or_mm_err(|| UtxoCoinBuildError::CantDetectUserHome)?;
            Ok(home.join(confpath))
        } else {
            Ok(confpath.into())
        }
    }

    fn check_utxo_maturity(&self) -> bool {
        if let Some(false) = self.activation_params.utxo_params.check_utxo_maturity {
            warn!("'check_utxo_maturity' is ignored because QRC20 gas refund is returned as a coinbase transaction");
        }
        true
    }

    /// Override [`UtxoCoinBuilderCommonOps::tx_cache`] to initialize TX cache with the platform ticker.
    /// Please note the method is overridden for Native mode only.
    #[inline]
    #[cfg(not(target_arch = "wasm32"))]
    fn tx_cache(&self) -> UtxoVerboseCacheShared {
        crate::utxo::tx_cache::fs_tx_cache::FsVerboseCache::new(self.platform.clone(), self.tx_cache_path())
            .into_shared()
    }
}

#[async_trait]
impl UtxoCoinBuilder for Qrc20CoinBuilder<'_> {
    type ResultCoin = Qrc20Coin;
    type Error = UtxoCoinBuildError;

    fn priv_key_policy(&self) -> PrivKeyBuildPolicy {
        self.priv_key_policy.clone()
    }

    async fn build_utxo_fields(&self) -> UtxoCoinBuildResult<UtxoCoinFields> {
        match self.priv_key_policy() {
            PrivKeyBuildPolicy::IguanaPrivKey(priv_key) => build_utxo_fields_with_iguana_priv_key(self, priv_key).await,
            PrivKeyBuildPolicy::GlobalHDAccount(global_hd_ctx) => {
                build_utxo_fields_with_global_hd(self, global_hd_ctx).await
            },
            PrivKeyBuildPolicy::Trezor => {
                let priv_key_err = PrivKeyPolicyNotAllowed::HardwareWalletNotSupported;
                MmError::err(UtxoCoinBuildError::PrivKeyPolicyNotAllowed(priv_key_err))
            },
            PrivKeyBuildPolicy::WalletConnect { .. } => {
                let priv_key_err = PrivKeyPolicyNotAllowed::UnsupportedMethod(
                    "WalletConnect is not available for QRC20 coin".to_string(),
                );
                MmError::err(UtxoCoinBuildError::PrivKeyPolicyNotAllowed(priv_key_err))
            },
        }
    }

    async fn build(self) -> MmResult<Self::ResultCoin, Self::Error> {
        let utxo = self.build_utxo_fields().await?;

        let inner = Qrc20CoinFields {
            utxo,
            platform: self.platform,
            contract_address: self.token_contract_address,
            swap_contract_address: self.activation_params.swap_contract_address,
            fallback_swap_contract: self.activation_params.fallback_swap_contract,
        };
        Ok(Qrc20Coin(Arc::new(inner)))
    }
}

pub async fn qrc20_coin_with_policy(
    ctx: &MmArc,
    ticker: &str,
    platform: &str,
    conf: &Json,
    params: &Qrc20ActivationParams,
    priv_key_policy: PrivKeyBuildPolicy,
    contract_address: H160,
) -> Result<Qrc20Coin, String> {
    let builder = Qrc20CoinBuilder::new(
        ctx,
        ticker,
        conf,
        params,
        priv_key_policy,
        platform.to_owned(),
        contract_address,
    );
    Ok(try_s!(builder.build().await))
}

pub async fn qrc20_coin_with_priv_key(
    ctx: &MmArc,
    ticker: &str,
    platform: &str,
    conf: &Json,
    params: &Qrc20ActivationParams,
    priv_key: IguanaPrivKey,
    contract_address: H160,
) -> Result<Qrc20Coin, String> {
    let priv_key_policy = PrivKeyBuildPolicy::IguanaPrivKey(priv_key);
    qrc20_coin_with_policy(ctx, ticker, platform, conf, params, priv_key_policy, contract_address).await
}

pub struct Qrc20CoinFields {
    pub utxo: UtxoCoinFields,
    pub platform: String,
    pub contract_address: H160,
    pub swap_contract_address: H160,
    pub fallback_swap_contract: Option<H160>,
}

#[derive(Clone)]
pub struct Qrc20Coin(Arc<Qrc20CoinFields>);

impl Deref for Qrc20Coin {
    type Target = Qrc20CoinFields;
    fn deref(&self) -> &Qrc20CoinFields {
        &self.0
    }
}

impl AsRef<UtxoCoinFields> for Qrc20Coin {
    fn as_ref(&self) -> &UtxoCoinFields {
        &self.utxo
    }
}

impl qtum::QtumBasedCoin for Qrc20Coin {}

#[derive(Clone, Debug, PartialEq)]
pub struct ContractCallOutput {
    pub value: u64,
    pub script_pubkey: ScriptBytes,
    pub gas_limit: u64,
    pub gas_price: u64,
}

impl From<ContractCallOutput> for TransactionOutput {
    fn from(out: ContractCallOutput) -> Self {
        TransactionOutput {
            value: out.value,
            script_pubkey: out.script_pubkey,
        }
    }
}

/// Functions of ERC20/EtomicSwap smart contracts that may change the blockchain state.
#[derive(Debug, Eq, PartialEq)]
pub enum MutContractCallType {
    Transfer,
    Erc20Payment,
    ReceiverSpend,
    SenderRefund,
}

impl MutContractCallType {
    fn as_function_name(&self) -> &'static str {
        match self {
            MutContractCallType::Transfer => "transfer",
            MutContractCallType::Erc20Payment => "erc20Payment",
            MutContractCallType::ReceiverSpend => "receiverSpend",
            MutContractCallType::SenderRefund => "senderRefund",
        }
    }

    fn as_function(&self) -> &'static Function {
        match self {
            MutContractCallType::Transfer => eth::ERC20_CONTRACT.function(self.as_function_name()).unwrap(),
            MutContractCallType::Erc20Payment
            | MutContractCallType::ReceiverSpend
            | MutContractCallType::SenderRefund => eth::SWAP_CONTRACT.function(self.as_function_name()).unwrap(),
        }
    }

    pub fn from_script_pubkey(script: &[u8]) -> Result<Option<MutContractCallType>, String> {
        lazy_static! {
            static ref TRANSFER_SHORT_SIGN: [u8; 4] =
                eth::ERC20_CONTRACT.function("transfer").unwrap().short_signature();
            static ref ERC20_PAYMENT_SHORT_SIGN: [u8; 4] =
                eth::SWAP_CONTRACT.function("erc20Payment").unwrap().short_signature();
            static ref RECEIVER_SPEND_SHORT_SIGN: [u8; 4] =
                eth::SWAP_CONTRACT.function("receiverSpend").unwrap().short_signature();
            static ref SENDER_REFUND_SHORT_SIGN: [u8; 4] =
                eth::SWAP_CONTRACT.function("senderRefund").unwrap().short_signature();
        }

        if script.len() < 4 {
            return ERR!("Length of the script pubkey less than 4: {:?}", script);
        }

        if script.starts_with(TRANSFER_SHORT_SIGN.as_ref()) {
            return Ok(Some(MutContractCallType::Transfer));
        }
        if script.starts_with(ERC20_PAYMENT_SHORT_SIGN.as_ref()) {
            return Ok(Some(MutContractCallType::Erc20Payment));
        }
        if script.starts_with(RECEIVER_SPEND_SHORT_SIGN.as_ref()) {
            return Ok(Some(MutContractCallType::ReceiverSpend));
        }
        if script.starts_with(SENDER_REFUND_SHORT_SIGN.as_ref()) {
            return Ok(Some(MutContractCallType::SenderRefund));
        }
        Ok(None)
    }

    #[allow(dead_code)]
    fn short_signature(&self) -> [u8; 4] {
        self.as_function().short_signature()
    }
}

pub struct GenerateQrc20TxResult {
    pub signed: UtxoTx,
    pub miner_fee: u64,
    pub gas_fee: u64,
}

#[derive(Debug, Display)]
pub enum Qrc20AbiError {
    #[display(fmt = "Invalid QRC20 ABI params: {_0}")]
    InvalidParams(String),
    #[display(fmt = "QRC20 ABI error: {_0}")]
    ABIError(String),
}

impl From<ethabi::Error> for Qrc20AbiError {
    fn from(e: ethabi::Error) -> Qrc20AbiError {
        Qrc20AbiError::ABIError(e.to_string())
    }
}

impl From<Qrc20AbiError> for ValidatePaymentError {
    fn from(e: Qrc20AbiError) -> ValidatePaymentError {
        ValidatePaymentError::TxDeserializationError(e.to_string())
    }
}

impl From<Qrc20AbiError> for GenerateTxError {
    fn from(e: Qrc20AbiError) -> Self {
        GenerateTxError::Internal(e.to_string())
    }
}

impl From<Qrc20AbiError> for TradePreimageError {
    fn from(e: Qrc20AbiError) -> Self {
        // `Qrc20ABIError` is always an internal error
        TradePreimageError::InternalError(e.to_string())
    }
}

impl From<Qrc20AbiError> for WithdrawError {
    fn from(e: Qrc20AbiError) -> Self {
        // `Qrc20ABIError` is always an internal error
        WithdrawError::InternalError(e.to_string())
    }
}

impl From<Qrc20AbiError> for UtxoRpcError {
    fn from(e: Qrc20AbiError) -> Self {
        // `Qrc20ABIError` is always an internal error
        UtxoRpcError::Internal(e.to_string())
    }
}

impl Qrc20Coin {
    /// `gas_fee` should be calculated by: gas_limit * gas_price * (count of contract calls),
    /// or should be sum of gas fee of all contract calls.
    pub async fn get_qrc20_tx_fee(&self, gas_fee: u64) -> Result<u64, String> {
        match try_s!(self.get_fee_rate().await) {
            ActualFeeRate::Dynamic(amount)
            | ActualFeeRate::FixedPerKb(amount)
            | ActualFeeRate::FixedPerKbDingo(amount) => Ok(amount + gas_fee),
        }
    }

    /// Generate and send a transaction with the specified UTXO outputs.
    /// Note this function locks the `UTXO_LOCK`.
    pub async fn send_contract_calls(
        &self,
        outputs: Vec<ContractCallOutput>,
    ) -> Result<TransactionEnum, TransactionErr> {
        // TODO: we need to somehow refactor it using RecentlySpentOutpoints cache
        // Move over all QRC20 tokens should share the same cache with each other and base QTUM coin
        let _utxo_lock = UTXO_LOCK.lock().await;

        let GenerateQrc20TxResult { signed, .. } = self
            .generate_qrc20_transaction(outputs)
            .await
            .map_err(|e| TransactionErr::Plain(ERRL!("{}", e)))?;
        try_tx_s!(self.utxo.rpc_client.send_transaction(&signed).compat().await, signed);
        Ok(signed.into())
    }

    /// Generate Qtum UTXO transaction with contract calls.
    /// Note: lock the UTXO_LOCK mutex before this function will be called.
    async fn generate_qrc20_transaction(
        &self,
        contract_outputs: Vec<ContractCallOutput>,
    ) -> Result<GenerateQrc20TxResult, MmError<Qrc20GenTxError>> {
        let my_address = self.utxo.derivation_method.single_addr_or_err().await.map_mm_err()?;
        let (unspents, _) = self.get_unspent_ordered_list(&my_address).await.map_mm_err()?;

        let mut gas_fee = 0;
        let mut outputs = Vec::with_capacity(contract_outputs.len());
        for output in contract_outputs {
            gas_fee += output.gas_limit * output.gas_price;
            outputs.push(TransactionOutput::from(output));
        }

        let (unsigned, data) = UtxoTxBuilder::new(self)
            .await
            .add_available_inputs(unspents)
            .add_outputs(outputs)
            .with_gas_fee(gas_fee)
            .build()
            .await
            .map_mm_err()?;

        let key_pair = self.utxo.priv_key_policy.activated_key_or_err().map_mm_err()?;

        let signed = sign_tx(
            unsigned,
            key_pair,
            self.utxo.conf.signature_version,
            self.utxo.conf.fork_id,
        )
        .map_mm_err()?;

        Ok(GenerateQrc20TxResult {
            signed,
            miner_fee: data.fee_amount,
            gas_fee,
        })
    }

    fn transfer_output(
        &self,
        to_addr: H160,
        amount: U256,
        gas_limit: u64,
        gas_price: u64,
    ) -> Qrc20AbiResult<ContractCallOutput> {
        let function = eth::ERC20_CONTRACT.function("transfer")?;
        let params = function.encode_input(&[Token::Address(to_addr), Token::Uint(amount)])?;

        let script_pubkey =
            generate_contract_call_script_pubkey(&params, gas_limit, gas_price, self.contract_address.as_bytes())?
                .to_bytes();

        Ok(ContractCallOutput {
            value: OUTPUT_QTUM_AMOUNT,
            script_pubkey,
            gas_limit,
            gas_price,
        })
    }

    async fn preimage_trade_fee_required_to_send_outputs(
        &self,
        contract_outputs: Vec<ContractCallOutput>,
        stage: &FeeApproxStage,
    ) -> TradePreimageResult<BigDecimal> {
        let decimals = self.as_ref().decimals;
        let mut gas_fee = 0;
        let mut outputs = Vec::with_capacity(contract_outputs.len());
        for output in contract_outputs {
            gas_fee += output.gas_limit * output.gas_price;
            outputs.push(TransactionOutput::from(output));
        }
        let fee_policy = FeePolicy::SendExact;
        let miner_fee =
            UtxoCommonOps::preimage_trade_fee_required_to_send_outputs(self, outputs, fee_policy, Some(gas_fee), stage)
                .await?;
        let gas_fee = big_decimal_from_sat(gas_fee as i64, decimals);
        Ok(miner_fee + gas_fee)
    }
}

// if mockable is placed before async_trait there is `munmap_chunk(): invalid pointer` error on async fn mocking attempt
#[async_trait]
#[cfg_attr(test, mockable)]
impl UtxoTxBroadcastOps for Qrc20Coin {
    async fn broadcast_tx(&self, tx: &UtxoTx) -> Result<H256Json, MmError<BroadcastTxErr>> {
        utxo_common::broadcast_tx(self, tx).await
    }
}

#[async_trait]
#[cfg_attr(test, mockable)]
impl UtxoTxGenerationOps for Qrc20Coin {
    /// Get only QTUM transaction fee.
    async fn get_fee_rate(&self) -> UtxoRpcResult<ActualFeeRate> {
        utxo_common::get_fee_rate(&self.utxo).await
    }

    async fn calc_interest_if_required(&self, unsigned: &mut TransactionInputSigner) -> UtxoRpcResult<u64> {
        utxo_common::calc_interest_if_required(self, unsigned).await
    }

    fn supports_interest(&self) -> bool {
        utxo_common::is_kmd(self)
    }
}

#[async_trait]
#[cfg_attr(test, mockable)]
impl GetUtxoListOps for Qrc20Coin {
    async fn get_unspent_ordered_list(
        &self,
        address: &UtxoAddress,
    ) -> UtxoRpcResult<(Vec<UnspentInfo>, RecentlySpentOutPointsGuard<'_>)> {
        utxo_common::get_unspent_ordered_list(self, address).await
    }

    async fn get_all_unspent_ordered_list(
        &self,
        address: &UtxoAddress,
    ) -> UtxoRpcResult<(Vec<UnspentInfo>, RecentlySpentOutPointsGuard<'_>)> {
        utxo_common::get_all_unspent_ordered_list(self, address).await
    }

    async fn get_mature_unspent_ordered_list(
        &self,
        address: &UtxoAddress,
    ) -> UtxoRpcResult<(MatureUnspentList, RecentlySpentOutPointsGuard<'_>)> {
        utxo_common::get_mature_unspent_ordered_list(self, address).await
    }
}

#[async_trait]
#[cfg_attr(test, mockable)]
impl UtxoCommonOps for Qrc20Coin {
    async fn get_htlc_spend_fee(&self, tx_size: u64, stage: &FeeApproxStage) -> UtxoRpcResult<u64> {
        utxo_common::get_htlc_spend_fee(self, tx_size, stage).await
    }

    fn addresses_from_script(&self, script: &Script) -> Result<Vec<UtxoAddress>, String> {
        utxo_common::addresses_from_script(self, script)
    }

    fn denominate_satoshis(&self, satoshi: i64) -> f64 {
        utxo_common::denominate_satoshis(&self.utxo, satoshi)
    }

    fn my_public_key(&self) -> Result<&Public, MmError<UnexpectedDerivationMethod>> {
        utxo_common::my_public_key(self.as_ref())
    }

    fn address_from_str(&self, address: &str) -> MmResult<UtxoAddress, AddrFromStrError> {
        utxo_common::checked_address_from_str(self, address)
    }

    fn script_for_address(&self, address: &UtxoAddress) -> MmResult<Script, UnsupportedAddr> {
        utxo_common::output_script_checked(self.as_ref(), address)
    }

    async fn get_current_mtp(&self) -> UtxoRpcResult<u32> {
        utxo_common::get_current_mtp(&self.utxo, CoinVariant::Qtum).await
    }

    fn is_unspent_mature(&self, output: &RpcTransaction) -> bool {
        self.is_qtum_unspent_mature(output)
    }

    async fn calc_interest_of_tx(
        &self,
        _tx: &UtxoTx,
        _input_transactions: &mut HistoryUtxoTxMap,
    ) -> UtxoRpcResult<u64> {
        MmError::err(UtxoRpcError::Internal(
            "QRC20 coin doesn't support transaction rewards".to_owned(),
        ))
    }

    async fn get_mut_verbose_transaction_from_map_or_rpc<'a, 'b>(
        &'a self,
        tx_hash: H256Json,
        utxo_tx_map: &'b mut HistoryUtxoTxMap,
    ) -> UtxoRpcResult<&'b mut HistoryUtxoTx> {
        utxo_common::get_mut_verbose_transaction_from_map_or_rpc(self, tx_hash, utxo_tx_map).await
    }

    async fn p2sh_spending_tx(&self, input: utxo_common::P2SHSpendingTxInput<'_>) -> Result<UtxoTx, String> {
        utxo_common::p2sh_spending_tx(self, input).await
    }

    fn get_verbose_transactions_from_cache_or_rpc(
        &self,
        tx_ids: HashSet<H256Json>,
    ) -> UtxoRpcFut<HashMap<H256Json, VerboseTransactionFrom>> {
        let selfi = self.clone();
        let fut = async move { utxo_common::get_verbose_transactions_from_cache_or_rpc(&selfi.utxo, tx_ids).await };
        Box::new(fut.boxed().compat())
    }

    async fn preimage_trade_fee_required_to_send_outputs(
        &self,
        outputs: Vec<TransactionOutput>,
        fee_policy: FeePolicy,
        gas_fee: Option<u64>,
        stage: &FeeApproxStage,
    ) -> TradePreimageResult<BigDecimal> {
        utxo_common::preimage_trade_fee_required_to_send_outputs(
            self,
            self.platform_ticker(),
            outputs,
            fee_policy,
            gas_fee,
            stage,
        )
        .await
    }

    fn increase_dynamic_fee_by_stage(&self, dynamic_fee: u64, stage: &FeeApproxStage) -> u64 {
        utxo_common::increase_dynamic_fee_by_stage(self, dynamic_fee, stage)
    }

    async fn p2sh_tx_locktime(&self, htlc_locktime: u32) -> Result<u32, MmError<UtxoRpcError>> {
        utxo_common::p2sh_tx_locktime(self, &self.utxo.conf.ticker, htlc_locktime).await
    }

    fn addr_format(&self) -> &UtxoAddressFormat {
        utxo_common::addr_format(self)
    }

    fn addr_format_for_standard_scripts(&self) -> UtxoAddressFormat {
        utxo_common::addr_format_for_standard_scripts(self)
    }

    fn address_from_pubkey(&self, pubkey: &Public) -> UtxoAddress {
        let conf = &self.utxo.conf;
        utxo_common::address_from_pubkey(
            pubkey,
            conf.address_prefixes.clone(),
            conf.checksum_type,
            conf.bech32_hrp.clone(),
            self.addr_format().clone(),
        )
    }
}

#[async_trait]
impl SwapOps for Qrc20Coin {
    async fn send_taker_fee(&self, dex_fee: DexFee, _uuid: &[u8], _expire_at: u64) -> TransactionResult {
        let to_address = try_tx_s!(self.contract_address_from_raw_pubkey(self.dex_pubkey()));
        let amount = try_tx_s!(wei_from_big_decimal(&dex_fee.fee_amount().into(), self.utxo.decimals));
        let transfer_output =
            try_tx_s!(self.transfer_output(to_address, amount, QRC20_GAS_LIMIT_DEFAULT, QRC20_GAS_PRICE_DEFAULT));
        self.send_contract_calls(vec![transfer_output]).await
    }

    async fn send_maker_payment(&self, maker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        let time_lock = try_tx_s!(maker_payment_args.time_lock.try_into());
        let taker_addr = try_tx_s!(self.contract_address_from_raw_pubkey(maker_payment_args.other_pubkey));
        let id = qrc20_swap_id(time_lock, maker_payment_args.secret_hash);
        let value = try_tx_s!(wei_from_big_decimal(&maker_payment_args.amount, self.utxo.decimals));
        let secret_hash = Vec::from(maker_payment_args.secret_hash);
        let swap_contract_address = try_tx_s!(maker_payment_args.swap_contract_address.try_to_address());

        self.send_hash_time_locked_payment(id, value, time_lock, secret_hash, taker_addr, swap_contract_address)
            .await
    }

    #[inline]
    async fn send_taker_payment(&self, taker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        let time_lock = try_tx_s!(taker_payment_args.time_lock.try_into());
        let maker_addr = try_tx_s!(self.contract_address_from_raw_pubkey(taker_payment_args.other_pubkey));
        let id = qrc20_swap_id(time_lock, taker_payment_args.secret_hash);
        let value = try_tx_s!(wei_from_big_decimal(&taker_payment_args.amount, self.utxo.decimals));
        let secret_hash = Vec::from(taker_payment_args.secret_hash);
        let swap_contract_address = try_tx_s!(taker_payment_args.swap_contract_address.try_to_address());
        self.send_hash_time_locked_payment(id, value, time_lock, secret_hash, maker_addr, swap_contract_address)
            .await
    }

    #[inline]
    async fn send_maker_spends_taker_payment(
        &self,
        maker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        let payment_tx: UtxoTx =
            try_tx_s!(deserialize(maker_spends_payment_args.other_payment_tx).map_err(|e| ERRL!("{:?}", e)));
        let swap_contract_address = try_tx_s!(maker_spends_payment_args.swap_contract_address.try_to_address());
        let secret = maker_spends_payment_args.secret.to_vec();

        self.spend_hash_time_locked_payment(payment_tx, swap_contract_address, secret)
            .await
    }

    #[inline]
    async fn send_taker_spends_maker_payment(
        &self,
        taker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        let payment_tx: UtxoTx =
            try_tx_s!(deserialize(taker_spends_payment_args.other_payment_tx).map_err(|e| ERRL!("{:?}", e)));
        let secret = taker_spends_payment_args.secret.to_vec();
        let swap_contract_address = try_tx_s!(taker_spends_payment_args.swap_contract_address.try_to_address());

        self.spend_hash_time_locked_payment(payment_tx, swap_contract_address, secret)
            .await
    }

    #[inline]
    async fn send_taker_refunds_payment(&self, taker_refunds_payment_args: RefundPaymentArgs<'_>) -> TransactionResult {
        let payment_tx: UtxoTx =
            try_tx_s!(deserialize(taker_refunds_payment_args.payment_tx).map_err(|e| ERRL!("{:?}", e)));
        let swap_contract_address = try_tx_s!(taker_refunds_payment_args.swap_contract_address.try_to_address());

        self.refund_hash_time_locked_payment(swap_contract_address, payment_tx)
            .await
    }

    #[inline]
    async fn send_maker_refunds_payment(&self, maker_refunds_payment_args: RefundPaymentArgs<'_>) -> TransactionResult {
        let payment_tx: UtxoTx =
            try_tx_s!(deserialize(maker_refunds_payment_args.payment_tx).map_err(|e| ERRL!("{:?}", e)));
        let swap_contract_address = try_tx_s!(maker_refunds_payment_args.swap_contract_address.try_to_address());

        self.refund_hash_time_locked_payment(swap_contract_address, payment_tx)
            .await
    }

    #[inline]
    async fn validate_fee(&self, validate_fee_args: ValidateFeeArgs<'_>) -> ValidatePaymentResult<()> {
        let fee_tx = match validate_fee_args.fee_tx {
            TransactionEnum::UtxoTx(tx) => tx,
            fee_tx => {
                return MmError::err(ValidatePaymentError::InternalError(format!(
                    "Invalid fee tx type. fee tx: {fee_tx:?}"
                )))
            },
        };
        let fee_tx_hash = fee_tx.hash().reversed().into();
        let inputs_signed_by_pub =
            check_all_utxo_inputs_signed_by_pub(self, fee_tx, validate_fee_args.expected_sender).await?;
        if !inputs_signed_by_pub {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(
                "The dex fee was sent from wrong address".to_string(),
            ));
        }
        let fee_addr = self
            .contract_address_from_raw_pubkey(self.dex_pubkey())
            .map_to_mm(ValidatePaymentError::WrongPaymentTx)?;
        let expected_value =
            wei_from_big_decimal(&validate_fee_args.dex_fee.fee_amount().into(), self.utxo.decimals).map_mm_err()?;

        self.validate_fee_impl(
            fee_tx_hash,
            fee_addr,
            expected_value,
            validate_fee_args.min_block_number,
        )
        .await
        .map_to_mm(ValidatePaymentError::WrongPaymentTx)
    }

    #[inline]
    async fn validate_maker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        let payment_tx: UtxoTx = deserialize(input.payment_tx.as_slice())?;
        let sender = self
            .contract_address_from_raw_pubkey(&input.other_pub)
            .map_to_mm(ValidatePaymentError::InvalidParameter)?;
        let swap_contract_address = input
            .swap_contract_address
            .try_to_address()
            .map_to_mm(ValidatePaymentError::InvalidParameter)?;

        let time_lock = input
            .time_lock
            .try_into()
            .map_to_mm(ValidatePaymentError::TimelockOverflow)?;
        self.validate_payment(
            payment_tx,
            time_lock,
            sender,
            input.secret_hash,
            input.amount,
            swap_contract_address,
        )
        .await
    }

    #[inline]
    async fn validate_taker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        let swap_contract_address = input
            .swap_contract_address
            .try_to_address()
            .map_to_mm(ValidatePaymentError::InvalidParameter)?;
        let payment_tx: UtxoTx = deserialize(input.payment_tx.as_slice())?;
        let sender = self
            .contract_address_from_raw_pubkey(&input.other_pub)
            .map_to_mm(ValidatePaymentError::InvalidParameter)?;
        let time_lock = input
            .time_lock
            .try_into()
            .map_to_mm(ValidatePaymentError::TimelockOverflow)?;

        self.validate_payment(
            payment_tx,
            time_lock,
            sender,
            input.secret_hash,
            input.amount,
            swap_contract_address,
        )
        .await
    }

    #[inline]
    async fn check_if_my_payment_sent(
        &self,
        if_my_payment_sent_args: CheckIfMyPaymentSentArgs<'_>,
    ) -> Result<Option<TransactionEnum>, String> {
        let time_lock = if_my_payment_sent_args
            .time_lock
            .try_into()
            .map_err(|e: TryFromIntError| e.to_string())?;
        let swap_id = qrc20_swap_id(time_lock, if_my_payment_sent_args.secret_hash);
        let swap_contract_address = if_my_payment_sent_args.swap_contract_address.try_to_address()?;

        self.check_if_my_payment_sent_impl(
            swap_contract_address,
            swap_id,
            if_my_payment_sent_args.search_from_block,
        )
        .await
    }

    #[inline]
    async fn search_for_swap_tx_spend_my(
        &self,
        input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        let tx: UtxoTx = try_s!(deserialize(input.tx).map_err(|e| ERRL!("{:?}", e)));

        self.search_for_swap_tx_spend(
            try_s!(input.time_lock.try_into()),
            input.secret_hash,
            tx,
            input.search_from_block,
        )
        .await
    }

    async fn search_for_swap_tx_spend_other(
        &self,
        input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        let tx: UtxoTx = try_s!(deserialize(input.tx).map_err(|e| ERRL!("{:?}", e)));

        self.search_for_swap_tx_spend(
            try_s!(input.time_lock.try_into()),
            input.secret_hash,
            tx,
            input.search_from_block,
        )
        .await
    }

    #[inline]
    async fn extract_secret(
        &self,
        secret_hash: &[u8],
        spend_tx: &[u8],
        _watcher_reward: bool,
    ) -> Result<[u8; 32], String> {
        self.extract_secret_impl(secret_hash, spend_tx)
    }

    fn negotiate_swap_contract_addr(
        &self,
        other_side_address: Option<&[u8]>,
    ) -> Result<Option<BytesJson>, MmError<NegotiateSwapContractAddrErr>> {
        match other_side_address {
            Some(bytes) => {
                if bytes.len() != 20 {
                    return MmError::err(NegotiateSwapContractAddrErr::InvalidOtherAddrLen(bytes.into()));
                }
                let other_addr = H160::from_slice(bytes);
                if other_addr == self.swap_contract_address {
                    return Ok(Some(self.swap_contract_address.0.to_vec().into()));
                }

                if Some(other_addr) == self.fallback_swap_contract {
                    return Ok(self.fallback_swap_contract.map(|addr| addr.0.to_vec().into()));
                }
                MmError::err(NegotiateSwapContractAddrErr::UnexpectedOtherAddr(bytes.into()))
            },
            None => self
                .fallback_swap_contract
                .map(|addr| Some(addr.0.to_vec().into()))
                .ok_or_else(|| MmError::new(NegotiateSwapContractAddrErr::NoOtherAddrAndNoFallback)),
        }
    }

    fn derive_htlc_key_pair(&self, swap_unique_data: &[u8]) -> KeyPair {
        utxo_common::derive_htlc_key_pair(self.as_ref(), swap_unique_data)
    }

    fn derive_htlc_pubkey(&self, swap_unique_data: &[u8]) -> [u8; 33] {
        utxo_common::derive_htlc_pubkey(self, swap_unique_data)
    }

    #[inline]
    fn validate_other_pubkey(&self, raw_pubkey: &[u8]) -> MmResult<(), ValidateOtherPubKeyErr> {
        utxo_common::validate_other_pubkey(raw_pubkey)
    }
}

#[async_trait]
impl WatcherOps for Qrc20Coin {}

#[async_trait]
impl MarketCoinOps for Qrc20Coin {
    fn ticker(&self) -> &str {
        &self.utxo.conf.ticker
    }

    fn my_address(&self) -> MmResult<String, MyAddressError> {
        utxo_common::my_address(self)
    }

    fn address_from_pubkey(&self, pubkey: &H264Json) -> MmResult<String, AddressFromPubkeyError> {
        let pubkey = Public::Compressed((*pubkey).into());
        Ok(UtxoCommonOps::address_from_pubkey(self, &pubkey).to_string())
    }

    async fn get_public_key(&self) -> Result<String, MmError<UnexpectedDerivationMethod>> {
        let pubkey = utxo_common::my_public_key(self.as_ref())?;
        Ok(pubkey.to_string())
    }

    fn sign_message_hash(&self, message: &str) -> Option<[u8; 32]> {
        let prefix = self.as_ref().conf.sign_message_prefix.as_ref()?;
        Some(sign_message_hash(prefix, message))
    }

    fn sign_message(&self, message: &str, address: Option<HDAddressSelector>) -> SignatureResult<String> {
        utxo_common::sign_message(self.as_ref(), message, address)
    }

    fn verify_message(&self, signature_base64: &str, message: &str, address: &str) -> VerificationResult<bool> {
        utxo_common::verify_message(self, signature_base64, message, address)
    }

    fn my_balance(&self) -> BalanceFut<CoinBalance> {
        let decimals = self.utxo.decimals;

        let coin = self.clone();
        let fut = async move {
            let my_address = coin
                .my_addr_as_contract_addr()
                .await
                .mm_err(|e| BalanceError::Internal(e.to_string()))?;
            let params = [Token::Address(my_address)];
            let contract_address = coin.contract_address;
            let tokens = coin
                .utxo
                .rpc_client
                .rpc_contract_call(ViewContractCallType::BalanceOf, &contract_address, &params)
                .compat()
                .await
                .map_mm_err()?;
            let spendable = match tokens.first() {
                Some(Token::Uint(bal)) => u256_to_big_decimal(*bal, decimals).map_mm_err()?,
                _ => {
                    let error = format!("Expected U256 as balanceOf result but got {tokens:?}");
                    return MmError::err(BalanceError::InvalidResponse(error));
                },
            };
            Ok(CoinBalance {
                spendable,
                unspendable: BigDecimal::from(0),
            })
        };
        Box::new(fut.boxed().compat())
    }
    fn platform_coin_balance(&self) -> BalanceFut<BigDecimal> {
        // use standard UTXO my_balance implementation that returns Qtum balance instead of QRC20
        Box::new(utxo_common::my_balance(self.clone()).map(|CoinBalance { spendable, .. }| spendable))
    }

    fn platform_ticker(&self) -> &str {
        &self.0.platform
    }

    #[inline(always)]
    fn send_raw_tx(&self, tx: &str) -> Box<dyn Future<Item = String, Error = String> + Send> {
        utxo_common::send_raw_tx(&self.utxo, tx)
    }

    #[inline(always)]
    fn send_raw_tx_bytes(&self, tx: &[u8]) -> Box<dyn Future<Item = String, Error = String> + Send> {
        utxo_common::send_raw_tx_bytes(&self.utxo, tx)
    }

    #[inline(always)]
    async fn sign_raw_tx(&self, args: &SignRawTransactionRequest) -> RawTransactionResult {
        utxo_common::sign_raw_tx(self, args).await
    }

    fn wait_for_confirmations(&self, input: ConfirmPaymentInput) -> Box<dyn Future<Item = (), Error = String> + Send> {
        let tx: UtxoTx = try_fus!(deserialize(input.payment_tx.as_slice()).map_err(|e| ERRL!("{:?}", e)));
        let selfi = self.clone();
        let fut = async move {
            selfi
                .wait_for_confirmations_and_check_result(
                    tx,
                    input.confirmations,
                    input.requires_nota,
                    input.wait_until,
                    input.check_every,
                )
                .await
        };
        Box::new(fut.boxed().compat())
    }

    async fn wait_for_htlc_tx_spend(&self, args: WaitForHTLCTxSpendArgs<'_>) -> TransactionResult {
        let tx: UtxoTx = try_tx_s!(deserialize(args.tx_bytes).map_err(|e| ERRL!("{:?}", e)));
        self.wait_for_tx_spend_impl(tx, args.wait_until, args.from_block, args.check_every)
            .map_err(TransactionErr::Plain)
            .await
    }

    fn tx_enum_from_bytes(&self, bytes: &[u8]) -> Result<TransactionEnum, MmError<TxMarshalingErr>> {
        utxo_common::tx_enum_from_bytes(self.as_ref(), bytes)
    }

    fn current_block(&self) -> Box<dyn Future<Item = u64, Error = String> + Send> {
        utxo_common::current_block(&self.utxo)
    }

    fn display_priv_key(&self) -> Result<String, String> {
        utxo_common::display_priv_key(&self.utxo)
    }

    #[inline]
    fn min_tx_amount(&self) -> BigDecimal {
        BigDecimal::from(0)
    }

    #[inline]
    fn min_trading_vol(&self) -> MmNumber {
        let pow = self.utxo.decimals as u32;
        MmNumber::from(1) / MmNumber::from(10u64.pow(pow))
    }

    #[inline]
    fn should_burn_dex_fee(&self) -> bool {
        false
    }

    fn is_trezor(&self) -> bool {
        self.as_ref().priv_key_policy.is_trezor()
    }
}

#[async_trait]
impl MmCoin for Qrc20Coin {
    fn is_asset_chain(&self) -> bool {
        utxo_common::is_asset_chain(&self.utxo)
    }

    fn spawner(&self) -> WeakSpawner {
        self.as_ref().abortable_system.weak_spawner()
    }

    fn withdraw(&self, req: WithdrawRequest) -> WithdrawFut {
        Box::new(qrc20_withdraw(self.clone(), req).boxed().compat())
    }

    fn get_raw_transaction(&self, req: RawTransactionRequest) -> RawTransactionFut<'_> {
        Box::new(utxo_common::get_raw_transaction(&self.utxo, req).boxed().compat())
    }

    fn get_tx_hex_by_hash(&self, tx_hash: Vec<u8>) -> RawTransactionFut<'_> {
        Box::new(utxo_common::get_tx_hex_by_hash(&self.utxo, tx_hash).boxed().compat())
    }

    fn decimals(&self) -> u8 {
        utxo_common::decimals(&self.utxo)
    }

    fn convert_to_address(&self, from: &str, to_address_format: Json) -> Result<String, String> {
        qtum::QtumBasedCoin::convert_to_address(self, from, to_address_format)
    }

    fn validate_address(&self, address: &str) -> ValidateAddressResult {
        utxo_common::validate_address(self, address)
    }

    fn process_history_loop(&self, ctx: MmArc) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        Box::new(self.clone().history_loop(ctx).map(|_| Ok(())).boxed().compat())
    }

    fn history_sync_status(&self) -> HistorySyncState {
        utxo_common::history_sync_status(&self.utxo)
    }

    /// This method is called to check our QTUM balance.
    fn get_trade_fee(&self) -> Box<dyn Future<Item = TradeFee, Error = String> + Send> {
        // `erc20Payment` may require two `approve` contract calls in worst case,
        let gas_fee = (2 * QRC20_GAS_LIMIT_DEFAULT + QRC20_PAYMENT_GAS_LIMIT) * QRC20_GAS_PRICE_DEFAULT;

        let selfi = self.clone();
        let fut = async move {
            let fee = try_s!(selfi.get_qrc20_tx_fee(gas_fee).await);
            Ok(TradeFee {
                coin: selfi.platform.clone(),
                amount: big_decimal_from_sat(fee as i64, selfi.utxo.decimals).into(),
                paid_from_trading_vol: false,
            })
        };
        Box::new(fut.boxed().compat())
    }

    async fn get_sender_trade_fee(
        &self,
        value: TradePreimageValue,
        stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        let decimals = self.utxo.decimals;
        // pass the dummy params
        let timelock = now_sec_u32();
        let secret_hash = vec![0; 20];
        let swap_id = qrc20_swap_id(timelock, &secret_hash);
        let receiver_addr = H160::default();
        // we can avoid the requesting balance, because it doesn't affect the total fee
        let my_balance = U256::max_value();
        let value = match value {
            TradePreimageValue::Exact(value) | TradePreimageValue::UpperBound(value) => {
                wei_from_big_decimal(&value, decimals).map_mm_err()?
            },
        };

        let erc20_payment_fee = {
            let erc20_payment_outputs = self
                .generate_swap_payment_outputs(
                    my_balance,
                    swap_id.clone(),
                    value,
                    timelock,
                    secret_hash.clone(),
                    receiver_addr,
                    self.swap_contract_address,
                )
                .await
                .map_mm_err()?;
            self.preimage_trade_fee_required_to_send_outputs(erc20_payment_outputs, &stage)
                .await?
        };

        // Optionally calculate refund fee.
        let sender_refund_fee = if matches!(stage, FeeApproxStage::TradePreimage | FeeApproxStage::TradePreimageMax) {
            let sender_refund_output = self
                .sender_refund_output(&self.swap_contract_address, swap_id, value, secret_hash, receiver_addr)
                .map_mm_err()?;
            self.preimage_trade_fee_required_to_send_outputs(vec![sender_refund_output], &stage)
                .await?
        } else {
            BigDecimal::from(0) // No refund fee if not included.
        };

        let total_fee = erc20_payment_fee + sender_refund_fee;

        Ok(TradeFee {
            coin: self.platform.clone(),
            amount: total_fee.into(),
            paid_from_trading_vol: false,
        })
    }

    fn get_receiver_trade_fee(&self, stage: FeeApproxStage) -> TradePreimageFut<TradeFee> {
        let selfi = self.clone();
        let fut = async move {
            // pass the dummy params
            let timelock = now_sec_u32();
            let secret = vec![0; 32];
            let swap_id = qrc20_swap_id(timelock, &secret[0..20]);
            let sender_addr = H160::default();
            // get the max available value that we can pass into the contract call params
            // see `generate_contract_call_script_pubkey`
            let value = u64::MAX.into();
            let output = selfi
                .receiver_spend_output(&selfi.swap_contract_address, swap_id, value, secret, sender_addr)
                .map_mm_err()?;

            let total_fee = selfi
                .preimage_trade_fee_required_to_send_outputs(vec![output], &stage)
                .await?;
            Ok(TradeFee {
                coin: selfi.platform.clone(),
                amount: total_fee.into(),
                paid_from_trading_vol: false,
            })
        };
        Box::new(fut.boxed().compat())
    }

    async fn get_fee_to_send_taker_fee(
        &self,
        dex_fee_amount: DexFee,
        stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        let amount = wei_from_big_decimal(&dex_fee_amount.fee_amount().into(), self.utxo.decimals).map_mm_err()?;

        // pass the dummy params
        let to_addr = H160::default();
        let transfer_output = self
            .transfer_output(to_addr, amount, QRC20_GAS_LIMIT_DEFAULT, QRC20_GAS_PRICE_DEFAULT)
            .map_mm_err()?;

        let total_fee = self
            .preimage_trade_fee_required_to_send_outputs(vec![transfer_output], &stage)
            .await?;

        Ok(TradeFee {
            coin: self.platform.clone(),
            amount: total_fee.into(),
            paid_from_trading_vol: false,
        })
    }

    fn required_confirmations(&self) -> u64 {
        utxo_common::required_confirmations(&self.utxo)
    }

    fn requires_notarization(&self) -> bool {
        utxo_common::requires_notarization(&self.utxo)
    }

    fn set_required_confirmations(&self, confirmations: u64) {
        utxo_common::set_required_confirmations(&self.utxo, confirmations)
    }

    fn set_requires_notarization(&self, requires_nota: bool) {
        utxo_common::set_requires_notarization(&self.utxo, requires_nota)
    }

    fn swap_contract_address(&self) -> Option<BytesJson> {
        Some(BytesJson::from(self.swap_contract_address.0.as_ref()))
    }

    fn fallback_swap_contract(&self) -> Option<BytesJson> {
        self.fallback_swap_contract.map(|a| BytesJson::from(a.0.as_ref()))
    }

    fn mature_confirmations(&self) -> Option<u32> {
        Some(self.utxo.conf.mature_confirmations)
    }

    fn coin_protocol_info(&self, _amount_to_receive: Option<MmNumber>) -> Vec<u8> {
        utxo_common::coin_protocol_info(self)
    }

    fn is_coin_protocol_supported(
        &self,
        info: &Option<Vec<u8>>,
        _amount_to_send: Option<MmNumber>,
        _locktime: u64,
        _is_maker: bool,
    ) -> bool {
        utxo_common::is_coin_protocol_supported(self, info)
    }

    fn on_disabled(&self) -> Result<(), AbortedError> {
        AbortableSystem::abort_all(&self.as_ref().abortable_system)
    }

    fn on_token_deactivated(&self, _ticker: &str) {}
}

pub fn qrc20_swap_id(time_lock: u32, secret_hash: &[u8]) -> Vec<u8> {
    let timelock_bytes = time_lock.to_le_bytes();
    let mut input = Vec::with_capacity(timelock_bytes.len() + secret_hash.len());

    input.extend_from_slice(&timelock_bytes);
    input.extend_from_slice(secret_hash);
    sha256(&input).to_vec()
}

pub fn contract_addr_into_rpc_format(address: &H160) -> H160Json {
    H160Json::from(address.0)
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Qrc20FeeDetails {
    /// Coin name
    pub coin: String,
    /// Standard UTXO miner fee based on transaction size
    pub miner_fee: BigDecimal,
    /// Gas limit in satoshi.
    pub gas_limit: u64,
    /// Gas price in satoshi.
    pub gas_price: u64,
    /// Total used gas.
    pub total_gas_fee: BigDecimal,
}

async fn qrc20_withdraw(coin: Qrc20Coin, req: WithdrawRequest) -> WithdrawResult {
    let to_addr = UtxoAddress::from_legacyaddress(&req.to, &coin.as_ref().conf.address_prefixes)
        .map_to_mm(WithdrawError::InvalidAddress)?;
    let conf = &coin.utxo.conf;
    if !to_addr.is_pubkey_hash() {
        let error = "QRC20 can be sent to P2PKH addresses only".to_owned();
        return MmError::err(WithdrawError::InvalidAddress(error));
    }

    let _utxo_lock = UTXO_LOCK.lock().await;

    let qrc20_balance = coin.my_spendable_balance().compat().await.map_mm_err()?;

    // the qrc20_amount_sat is used only within smart contract calls
    let (qrc20_amount_sat, qrc20_amount) = if req.max {
        let amount = wei_from_big_decimal(&qrc20_balance, coin.utxo.decimals).map_mm_err()?;
        if amount.is_zero() {
            return MmError::err(WithdrawError::ZeroBalanceToWithdrawMax);
        }
        (amount, qrc20_balance.clone())
    } else {
        let amount_sat = wei_from_big_decimal(&req.amount, coin.utxo.decimals).map_mm_err()?;
        if req.amount > qrc20_balance {
            return MmError::err(WithdrawError::NotSufficientBalance {
                coin: coin.ticker().to_owned(),
                available: qrc20_balance,
                required: req.amount,
            });
        }
        (amount_sat, req.amount)
    };

    let (gas_limit, gas_price) = match req.fee {
        Some(WithdrawFee::Qrc20Gas { gas_limit, gas_price }) => (gas_limit, gas_price),
        Some(fee_policy) => {
            let error = format!("Expected 'Qrc20Gas' fee type, found {fee_policy:?}");
            return MmError::err(WithdrawError::InvalidFeePolicy(error));
        },
        None => (QRC20_GAS_LIMIT_DEFAULT, QRC20_GAS_PRICE_DEFAULT),
    };

    // [`Qrc20Coin::transfer_output`] shouldn't fail if the arguments are correct
    let contract_addr = qtum::contract_addr_from_utxo_addr(to_addr.clone()).map_mm_err()?;
    let transfer_output = coin
        .transfer_output(contract_addr, qrc20_amount_sat, gas_limit, gas_price)
        .map_mm_err()?;
    let outputs = vec![transfer_output];

    let GenerateQrc20TxResult {
        signed,
        miner_fee,
        gas_fee,
    } = coin
        .generate_qrc20_transaction(outputs)
        .await
        .mm_err(|gen_tx_error| gen_tx_error.into_withdraw_error(coin.platform.clone(), coin.utxo.decimals))?;

    let my_address = coin.utxo.derivation_method.single_addr_or_err().await.map_mm_err()?;
    let received_by_me = if to_addr == my_address {
        qrc20_amount.clone()
    } else {
        0.into()
    };
    let my_balance_change = &received_by_me - &qrc20_amount;

    // [`MarketCoinOps::my_address`] and [`UtxoCommonOps::display_address`] shouldn't fail
    let my_address_string = coin.my_address().map_mm_err()?;
    let to_address = to_addr.display_address().map_to_mm(WithdrawError::InternalError)?;

    let fee_details = Qrc20FeeDetails {
        // QRC20 fees are paid in base platform currency (in particular Qtum)
        coin: coin.platform.clone(),
        miner_fee: utxo_common::big_decimal_from_sat(miner_fee as i64, coin.utxo.decimals),
        gas_limit,
        gas_price,
        total_gas_fee: utxo_common::big_decimal_from_sat(gas_fee as i64, coin.utxo.decimals),
    };
    Ok(TransactionDetails {
        from: vec![my_address_string],
        to: vec![to_address],
        total_amount: qrc20_amount.clone(),
        spent_by_me: qrc20_amount,
        received_by_me,
        my_balance_change,
        tx: TransactionData::new_signed(
            serialize(&signed).into(),
            signed.hash().reversed().to_vec().to_tx_hash(),
        ),
        fee_details: Some(fee_details.into()),
        block_height: 0,
        coin: conf.ticker.clone(),
        internal_id: vec![].into(),
        timestamp: now_sec(),
        kmd_rewards: None,
        transaction_type: TransactionType::StandardTransfer,
        memo: None,
    })
}

/// Parse the given topic to `H160` address.
fn address_from_log_topic(topic: &str) -> Result<H160, String> {
    if topic.len() != 64 {
        return ERR!(
            "Topic {:?} is expected to be H256 encoded topic (with length of 64)",
            topic
        );
    }

    // skip the first 24 characters to parse the last 40 characters to H160.
    // https://github.com/qtumproject/qtum-electrum/blob/v4.0.2/electrum/wallet.py#L2112
    let hash = try_s!(H160Json::from_str(&topic[24..]));
    Ok(hash.0.into())
}

fn address_to_log_topic(address: &H160) -> String {
    let zeros = std::str::from_utf8(&[b'0'; 24]).expect("Expected a valid str from slice of '0' chars");
    let mut topic = format!("{address:02x}");
    topic.insert_str(0, zeros);
    topic
}

pub struct TransferEventDetails {
    contract_address: H160,
    amount: U256,
    sender: H160,
    receiver: H160,
}

fn transfer_event_from_log(log: &LogEntry) -> Result<TransferEventDetails, String> {
    let contract_address = if log.address.starts_with("0x") {
        try_s!(qtum::contract_addr_from_str(&log.address))
    } else {
        let address = format!("0x{}", log.address);
        try_s!(qtum::contract_addr_from_str(&address))
    };

    if log.topics.len() != 3 {
        return ERR!("'Transfer' event must have 3 topics, found, {}", log.topics.len());
    }

    // https://github.com/qtumproject/qtum-electrum/blob/v4.0.2/electrum/wallet.py#L2111
    let amount = try_s!(U256::from_str(&log.data));

    // https://github.com/qtumproject/qtum-electrum/blob/v4.0.2/electrum/wallet.py#L2112
    let sender = try_s!(address_from_log_topic(&log.topics[1]));
    // https://github.com/qtumproject/qtum-electrum/blob/v4.0.2/electrum/wallet.py#L2113
    let receiver = try_s!(address_from_log_topic(&log.topics[2]));
    Ok(TransferEventDetails {
        contract_address,
        amount,
        sender,
        receiver,
    })
}

impl Eip1559Ops for Qrc20Coin {
    fn get_swap_transaction_fee_policy(&self) -> SwapTxFeePolicy {
        SwapTxFeePolicy::Unsupported
    }

    fn set_swap_transaction_fee_policy(&self, _swap_txfee_policy: SwapTxFeePolicy) {}
}
