/******************************************************************************
 * Copyright © 2023 Pampex LTD and TillyHK LTD                                *
 *                                                                            *
 * See the CONTRIBUTOR-LICENSE-AGREEMENT, COPYING, LICENSE-COPYRIGHT-NOTICE   *
 * and DEVELOPER-CERTIFICATE-OF-ORIGIN files in the LEGAL directory in        *
 * the top-level directory of this distribution for the individual copyright  *
 * holder information and the developer policies on copyright and licensing.  *
 *                                                                            *
 * Unless otherwise agreed in a custom licensing agreement, no part of the    *
 * Komodo DeFi Framework software, including this file may be copied, modified, propagated*
 * or distributed except according to the terms contained in the              *
 * LICENSE-COPYRIGHT-NOTICE file.                                             *
 *                                                                            *
 * Removal or modification of this copyright notice is prohibited.            *
 *                                                                            *
 ******************************************************************************/
//
//  coins.rs
//  marketmaker
//

// `mockable` implementation uses these
#![allow(
    forgetting_references,
    forgetting_copy_types,
    clippy::swap_ptr_to_ref,
    clippy::forget_non_drop,
    clippy::doc_lazy_continuation,
    clippy::needless_lifetimes // mocktopus requires explicit lifetimes
)]
#![allow(uncommon_codepoints)]

#[macro_use]
extern crate common;
#[macro_use]
extern crate gstuff;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate mm2_metrics;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json;
#[macro_use]
extern crate ser_error_derive;

use async_trait::async_trait;
use bip32::ExtendedPrivateKey;
use common::custom_futures::timeout::TimeoutError;
use common::executor::{abortable_queue::WeakSpawner, AbortedError, SpawnFuture};
use common::log::{warn, LogOnError};
use common::{calc_total_pages, now_sec, ten, HttpStatusCode, DEX_BURN_ADDR_RAW_PUBKEY, DEX_FEE_ADDR_RAW_PUBKEY};
use crypto::{
    derive_secp256k1_secret, Bip32Error, Bip44Chain, CryptoCtx, CryptoCtxError, DerivationPath, GlobalHDAccountArc,
    HDPathToCoin, HwRpcError, KeyPairPolicy, RpcDerivationPath, Secp256k1ExtendedPublicKey, Secp256k1Secret,
    WithHwRpcError,
};
use derive_more::Display;
use enum_derives::{EnumFromStringify, EnumFromTrait};
use ethereum_types::{Address as EthAddress, H256, H264, H520, U256};
use futures::compat::Future01CompatExt;
use futures::lock::{Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard};
use futures::{FutureExt, TryFutureExt};
use futures01::Future;
use hex::FromHexError;
use http::{Response, StatusCode};
use keys::{AddressFormat as UtxoAddressFormat, KeyPair, NetworkPrefix as CashAddrPrefix, Public};
use mm2_core::mm_ctx::{from_ctx, MmArc};
use mm2_err_handle::prelude::*;
use mm2_metrics::MetricsWeak;
use mm2_number::BigRational;
use mm2_number::{
    bigdecimal::{BigDecimal, ParseBigDecimalError, Zero},
    BigUint, MmNumber, ParseBigIntError,
};
use mm2_rpc::data::legacy::{EnabledCoin, GetEnabledResponse, Mm2RpcResult};
#[cfg(any(test, feature = "for-tests"))]
use mocktopus::macros::*;
use parking_lot::Mutex as PaMutex;
use rpc::v1::types::{Bytes as BytesJson, H256 as H256Json, H264 as H264Json};
use rpc_command::tendermint::ibc::ChannelId;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{self as json, Value as Json};
use std::array::TryFromSliceError;
use std::cmp::Ordering;
use std::collections::hash_map::{Entry, HashMap};
use std::collections::HashSet;
use std::num::{NonZeroUsize, TryFromIntError};
use std::ops::{Add, AddAssign, Deref};
use std::str::FromStr;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering as AtomicOrdering;
use std::sync::Arc;
use std::time::Duration;
use std::{fmt, iter};
use utxo_signer::with_key_pair::UtxoSignWithKeyPairError;
use zcash_primitives::transaction::Transaction as ZTransaction;

cfg_native! {
    use crate::lightning::LightningCoin;
    use crate::lightning::ln_conf::PlatformCoinConfirmationTargets;
    use ::lightning::ln::PaymentHash as LightningPayment;
    use async_std::fs;
    use futures::AsyncWriteExt;
    use lightning_invoice::{Invoice, ParseOrSemanticError};
    use std::io;
    use std::path::PathBuf;
}

cfg_wasm32! {
    use ethereum_types::{H264 as EthH264};
    use hd_wallet::HDWalletDb;
    use mm2_db::indexed_db::{ConstructibleDb, DbLocked, SharedDb};
    use tx_history_storage::wasm::{clear_tx_history, load_tx_history, save_tx_history, TxHistoryDb};
    pub type TxHistoryDbLocked<'a> = DbLocked<'a, TxHistoryDb>;
}

// using custom copy of try_fus as futures crate was renamed to futures01
macro_rules! try_fus {
    ($e: expr) => {
        match $e {
            Ok(ok) => ok,
            Err(err) => return Box::new(futures01::future::err(ERRL!("{}", err))),
        }
    };
}

macro_rules! try_f {
    ($e: expr) => {
        match $e {
            Ok(ok) => ok,
            Err(e) => return Box::new(futures01::future::err(e.into())),
        }
    };
}

/// `TransactionErr` compatible `try_fus` macro.
macro_rules! try_tx_fus {
    ($e: expr) => {
        match $e {
            Ok(ok) => ok,
            Err(err) => return Box::new(futures01::future::err(crate::TransactionErr::Plain(ERRL!("{:?}", err)))),
        }
    };
    ($e: expr, $tx: expr) => {
        match $e {
            Ok(ok) => ok,
            Err(err) => {
                return Box::new(futures01::future::err(crate::TransactionErr::TxRecoverable(
                    TransactionEnum::from($tx),
                    ERRL!("{:?}", err),
                )))
            },
        }
    };
}

/// `TransactionErr` compatible `try_s` macro.
macro_rules! try_tx_s {
    ($e: expr) => {
        match $e {
            Ok(ok) => ok,
            Err(err) => {
                return Err(crate::TransactionErr::Plain(format!(
                    "{}:{}] {:?}",
                    file!(),
                    line!(),
                    err
                )))
            },
        }
    };
    ($e: expr, $tx: expr) => {
        match $e {
            Ok(ok) => ok,
            Err(err) => {
                return Err(crate::TransactionErr::TxRecoverable(
                    TransactionEnum::from($tx),
                    format!("{}:{}] {:?}", file!(), line!(), err),
                ))
            },
        }
    };
}

/// `TransactionErr:Plain` compatible `ERR` macro.
macro_rules! TX_PLAIN_ERR {
    ($format: expr, $($args: tt)+) => { Err(crate::TransactionErr::Plain((ERRL!($format, $($args)+)))) };
    ($format: expr) => { Err(crate::TransactionErr::Plain(ERRL!($format))) }
}

/// `TransactionErr:TxRecoverable` compatible `ERR` macro.
#[allow(unused_macros)]
macro_rules! TX_RECOVERABLE_ERR {
    ($tx: expr, $format: expr, $($args: tt)+) => {
        Err(crate::TransactionErr::TxRecoverable(TransactionEnum::from($tx), ERRL!($format, $($args)+)))
    };
    ($tx: expr, $format: expr) => {
        Err(crate::TransactionErr::TxRecoverable(TransactionEnum::from($tx), ERRL!($format)))
    };
}

macro_rules! ok_or_continue_after_sleep {
    ($e:expr, $delay: ident) => {
        match $e {
            Ok(res) => res,
            Err(e) => {
                error!("error {:?}", e);
                Timer::sleep($delay).await;
                continue;
            },
        }
    };
}

pub mod coin_balance;
use coin_balance::{AddressBalanceStatus, HDAddressBalance, HDWalletBalanceOps};

pub mod lp_price;
pub mod watcher_common;

pub mod coin_errors;
use coin_errors::{
    AddressFromPubkeyError, MyAddressError, ValidatePaymentError, ValidatePaymentFut, ValidatePaymentResult,
};
use crypto::secret_hash_algo::SecretHashAlgo;

pub mod eth;
use eth::erc20::get_erc20_ticker_by_contract_address;
use eth::eth_swap_v2::{PrepareTxDataError, ValidatePaymentV2Err};
use eth::{
    eth_coin_from_conf_and_request, get_eth_address, EthCoin, EthGasDetailsErr, EthTxFeeDetails, GetEthAddressError,
    GetValidEthWithdrawAddError, SignedEthTx,
};

pub mod hd_wallet;
use hd_wallet::{
    AccountUpdatingError, AddressDerivingError, HDAccountOps, HDAddressId, HDAddressOps, HDAddressSelector,
    HDCoinAddress, HDCoinHDAccount, HDExtractPubkeyError, HDPathAccountToAddressId, HDWalletAddress, HDWalletCoinOps,
    HDWalletOps, HDWithdrawError, HDXPubExtractor, WithdrawSenderAddress,
};

#[cfg(not(target_arch = "wasm32"))]
pub mod lightning;
#[cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]
pub mod my_tx_history_v2;

pub mod qrc20;
use qrc20::{qrc20_coin_with_policy, Qrc20ActivationParams, Qrc20Coin, Qrc20FeeDetails};

pub mod rpc_command;
use rpc_command::{
    get_new_address::{GetNewAddressTaskManager, GetNewAddressTaskManagerShared},
    init_account_balance::{AccountBalanceTaskManager, AccountBalanceTaskManagerShared},
    init_create_account::{CreateAccountTaskManager, CreateAccountTaskManagerShared},
    init_scan_for_new_addresses::{ScanAddressesTaskManager, ScanAddressesTaskManagerShared},
    init_withdraw::{WithdrawTaskManager, WithdrawTaskManagerShared},
};

pub mod tendermint;
use tendermint::htlc::CustomTendermintMsgType;
use tendermint::{
    CosmosTransaction, TendermintCoin, TendermintFeeDetails, TendermintProtocolInfo, TendermintToken,
    TendermintTokenProtocolInfo,
};

#[doc(hidden)]
#[allow(unused_variables)]
#[cfg(any(test, feature = "for-tests"))]
pub mod test_coin;
#[cfg(any(test, feature = "for-tests"))]
pub use test_coin::TestCoin;

pub mod tx_history_storage;

#[cfg(feature = "enable-sia")]
pub mod siacoin;
#[cfg(feature = "enable-sia")]
use siacoin::SiaCoin;

pub mod utxo;
use utxo::bch::{bch_coin_with_policy, BchActivationRequest, BchCoin};
use utxo::qtum::{
    self, qtum_coin_with_policy, Qrc20AddressError, QtumCoin, QtumDelegationOps, QtumDelegationRequest,
    QtumStakingInfosDetails, ScriptHashTypeNotSupported,
};
use utxo::rpc_clients::UtxoRpcError;
use utxo::slp::SlpToken;
use utxo::slp::{slp_addr_from_pubkey_str, SlpFeeDetails};
use utxo::utxo_common::{big_decimal_from_sat_unsigned, payment_script, WaitForOutputSpendErr};
use utxo::utxo_standard::{utxo_standard_coin_with_policy, UtxoStandardCoin};
use utxo::{swap_proto_v2_scripts, BlockchainNetwork, GenerateTxError, UtxoActivationParams, UtxoFeeDetails, UtxoTx};

pub mod nft;
use nft::nft_errors::GetNftInfoError;
use script::Script;

pub mod z_coin;
use crate::coin_balance::{BalanceObjectOps, HDWalletBalanceObject};
use crate::hd_wallet::{AddrToString, DisplayAddress};
use z_coin::{ZCoin, ZcoinProtocolInfo};

#[cfg(feature = "enable-solana")]
pub mod solana;

pub type TransactionFut = Box<dyn Future<Item = TransactionEnum, Error = TransactionErr> + Send>;
pub type TransactionResult = Result<TransactionEnum, TransactionErr>;
pub type BalanceResult<T> = Result<T, MmError<BalanceError>>;
pub type BalanceFut<T> = Box<dyn Future<Item = T, Error = MmError<BalanceError>> + Send>;
pub type NonZeroBalanceFut<T> = Box<dyn Future<Item = T, Error = MmError<GetNonZeroBalance>> + Send>;
pub type NumConversResult<T> = Result<T, MmError<NumConversError>>;
pub type StakingInfosFut = Box<dyn Future<Item = StakingInfos, Error = MmError<StakingInfoError>> + Send>;
pub type DelegationResult = Result<TransactionDetails, MmError<DelegationError>>;
pub type DelegationFut = Box<dyn Future<Item = TransactionDetails, Error = MmError<DelegationError>> + Send>;
pub type WithdrawResult = Result<TransactionDetails, MmError<WithdrawError>>;
pub type WithdrawFut = Box<dyn Future<Item = TransactionDetails, Error = MmError<WithdrawError>> + Send>;
pub type TradePreimageResult<T> = Result<T, MmError<TradePreimageError>>;
pub type TradePreimageFut<T> = Box<dyn Future<Item = T, Error = MmError<TradePreimageError>> + Send>;
pub type CoinFindResult<T> = Result<T, MmError<CoinFindError>>;
pub type TxHistoryFut<T> = Box<dyn Future<Item = T, Error = MmError<TxHistoryError>> + Send>;
pub type TxHistoryResult<T> = Result<T, MmError<TxHistoryError>>;
pub type RawTransactionResult = Result<RawTransactionRes, MmError<RawTransactionError>>;
pub type RawTransactionFut<'a> =
    Box<dyn Future<Item = RawTransactionRes, Error = MmError<RawTransactionError>> + Send + 'a>;
pub type RefundResult<T> = Result<T, MmError<RefundError>>;
/// Helper type used for swap transactions' spend preimage generation result
pub type GenPreimageResult<Coin> = MmResult<TxPreimageWithSig<Coin>, TxGenError>;
/// Helper type used for swap v2 tx validation result
pub type ValidateSwapV2TxResult = MmResult<(), ValidateSwapV2TxError>;
/// Helper type used for taker funding's spend preimage validation result
pub type ValidateTakerFundingSpendPreimageResult = MmResult<(), ValidateTakerFundingSpendPreimageError>;
/// Helper type used for taker payment's spend preimage validation result
pub type ValidateTakerPaymentSpendPreimageResult = MmResult<(), ValidateTakerPaymentSpendPreimageError>;

pub type IguanaPrivKey = Secp256k1Secret;
pub type Ticker = String;

// Constants for logs used in tests
pub const INVALID_SENDER_ERR_LOG: &str = "Invalid sender";
pub const EARLY_CONFIRMATION_ERR_LOG: &str = "Early confirmation";
pub const OLD_TRANSACTION_ERR_LOG: &str = "Old transaction";
pub const INVALID_RECEIVER_ERR_LOG: &str = "Invalid receiver";
pub const INVALID_CONTRACT_ADDRESS_ERR_LOG: &str = "Invalid contract address";
pub const INVALID_PAYMENT_STATE_ERR_LOG: &str = "Invalid payment state";
pub const INVALID_SWAP_ID_ERR_LOG: &str = "Invalid swap id";
pub const INVALID_SCRIPT_ERR_LOG: &str = "Invalid script";
pub const INVALID_REFUND_TX_ERR_LOG: &str = "Invalid refund transaction";

#[derive(Debug, Deserialize, Display, EnumFromStringify, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum RawTransactionError {
    #[display(fmt = "No such coin {coin}")]
    NoSuchCoin { coin: String },
    #[display(fmt = "Invalid  hash: {_0}")]
    InvalidHashError(String),
    #[from_stringify("web3::Error")]
    #[display(fmt = "Transport error: {_0}")]
    Transport(String),
    #[display(fmt = "Hash does not exist: {_0}")]
    HashNotExist(String),
    #[display(fmt = "Internal error: {_0}")]
    InternalError(String),
    #[display(fmt = "Transaction decode error: {_0}")]
    DecodeError(String),
    #[from_stringify("NumConversError", "FromHexError")]
    #[display(fmt = "Invalid param: {_0}")]
    InvalidParam(String),
    #[display(fmt = "Non-existent previous output: {_0}")]
    NonExistentPrevOutputError(String),
    #[display(fmt = "Signing error: {_0}")]
    SigningError(String),
    #[display(fmt = "Not implemented for this coin {coin}")]
    NotImplemented { coin: String },
    #[display(fmt = "Transaction error {_0}")]
    TransactionError(String),
}

impl HttpStatusCode for RawTransactionError {
    fn status_code(&self) -> StatusCode {
        match self {
            RawTransactionError::InternalError(_) | RawTransactionError::SigningError(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            },
            RawTransactionError::NoSuchCoin { .. }
            | RawTransactionError::InvalidHashError(_)
            | RawTransactionError::HashNotExist(_)
            | RawTransactionError::DecodeError(_)
            | RawTransactionError::InvalidParam(_)
            | RawTransactionError::NonExistentPrevOutputError(_)
            | RawTransactionError::TransactionError(_) => StatusCode::BAD_REQUEST,
            RawTransactionError::NotImplemented { .. } => StatusCode::NOT_IMPLEMENTED,
            RawTransactionError::Transport(_) => StatusCode::BAD_GATEWAY,
        }
    }
}

impl From<CoinFindError> for RawTransactionError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => RawTransactionError::NoSuchCoin { coin },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Display, EnumFromStringify, PartialEq, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum GetMyAddressError {
    CoinsConfCheckError(String),
    CoinIsNotSupported(String),
    #[from_stringify("CryptoCtxError")]
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
    #[from_stringify("serde_json::Error")]
    #[display(fmt = "Invalid request error error: {_0}")]
    InvalidRequest(String),
    #[display(fmt = "Get Eth address error: {_0}")]
    GetEthAddressError(GetEthAddressError),
}

impl From<GetEthAddressError> for GetMyAddressError {
    fn from(e: GetEthAddressError) -> Self {
        GetMyAddressError::GetEthAddressError(e)
    }
}

impl HttpStatusCode for GetMyAddressError {
    fn status_code(&self) -> StatusCode {
        match self {
            GetMyAddressError::CoinsConfCheckError(_)
            | GetMyAddressError::CoinIsNotSupported(_)
            | GetMyAddressError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            GetMyAddressError::Internal(_) | GetMyAddressError::GetEthAddressError(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            },
        }
    }
}

#[derive(Deserialize)]
pub struct RawTransactionRequest {
    pub coin: String,
    pub tx_hash: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct RawTransactionRes {
    /// Raw bytes of signed transaction in hexadecimal string, this should be return hexadecimal encoded signed transaction for get_raw_transaction
    pub tx_hex: BytesJson,
}

/// Previous utxo transaction data for signing
#[derive(Clone, Debug, Deserialize)]
pub struct PrevTxns {
    /// transaction hash
    tx_hash: String,
    /// transaction output index
    index: u32,
    /// transaction output script pub key
    script_pub_key: String,
    // TODO: implement if needed:
    // redeem script for P2SH script pubkey
    // pub redeem_script: Option<String>,
    /// transaction output amount
    amount: BigDecimal,
}

/// sign_raw_transaction RPC request's params for signing raw utxo transactions
#[derive(Clone, Debug, Deserialize)]
pub struct SignUtxoTransactionParams {
    /// unsigned utxo transaction in hex
    tx_hex: String,
    /// optional data of previous transactions referred by unsigned transaction inputs
    prev_txns: Option<Vec<PrevTxns>>,
    // TODO: add if needed for utxo:
    // pub sighash_type: Option<String>, optional signature hash type, one of values: NONE, SINGLE, ALL, NONE|ANYONECANPAY, SINGLE|ANYONECANPAY, ALL|ANYONECANPAY (if not set 'ALL' is used)
    // pub branch_id: Option<u32>, zcash or komodo optional consensus branch id, used for signing transactions ahead of current height
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "tx_type")]
pub enum GasPriceRpcParam {
    Legacy {
        /// Gas price in gwei
        gas_price: BigDecimal,
    },
    Eip1559 {
        /// Max fee per gas in gwei
        max_fee_per_gas: BigDecimal,
        /// Max priority fee per gas in gwei
        max_priority_fee_per_gas: BigDecimal,
    },
    GasPricePolicy(SwapGasFeePolicy),
}

/// sign_raw_transaction RPC request's params for signing raw eth transactions
#[derive(Clone, Debug, Deserialize)]
pub struct SignEthTransactionParams {
    /// Eth transfer value
    value: Option<BigDecimal>,
    /// Eth to address
    to: Option<String>,
    /// Eth contract data
    data: Option<String>,
    /// Eth gas use limit
    gas_limit: U256,
    /// Optional gas price or fee per gas params
    pub pay_for_gas: Option<GasPriceRpcParam>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", content = "tx")]
pub enum SignRawTransactionEnum {
    UTXO(SignUtxoTransactionParams),
    ETH(SignEthTransactionParams),
}

/// sign_raw_transaction RPC request
#[derive(Clone, Debug, Deserialize)]
pub struct SignRawTransactionRequest {
    coin: String,
    #[serde(flatten)]
    tx: SignRawTransactionEnum,
}

#[derive(Debug, Deserialize)]
pub struct MyAddressReq {
    coin: String,
    #[serde(default)]
    path_to_address: HDPathAccountToAddressId,
}

#[derive(Debug, Serialize)]
pub struct MyWalletAddress {
    coin: String,
    wallet_address: String,
}

pub type SignatureResult<T> = Result<T, MmError<SignatureError>>;
pub type VerificationResult<T> = Result<T, MmError<VerificationError>>;

#[derive(Debug, Display, EnumFromStringify)]
pub enum TxHistoryError {
    ErrorSerializing(String),
    ErrorDeserializing(String),
    ErrorSaving(String),
    ErrorLoading(String),
    ErrorClearing(String),
    #[display(fmt = "'internal_id' not found: {internal_id:?}")]
    FromIdNotFound {
        internal_id: BytesJson,
    },
    NotSupported(String),
    #[from_stringify("MyAddressError")]
    InternalError(String),
}

#[derive(Clone, Debug, Deserialize, Display, PartialEq)]
pub enum PrivKeyPolicyNotAllowed {
    #[display(fmt = "Hardware Wallet is not supported")]
    HardwareWalletNotSupported,
    #[display(fmt = "Unsupported method: {_0}")]
    UnsupportedMethod(String),
    #[display(fmt = "Internal error: {_0}")]
    InternalError(String),
}

impl Serialize for PrivKeyPolicyNotAllowed {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[derive(Clone, Debug, Deserialize, Display, PartialEq, Serialize)]
pub enum UnexpectedDerivationMethod {
    #[display(fmt = "Expected 'SingleAddress' derivation method")]
    ExpectedSingleAddress,
    #[display(fmt = "Expected 'HDWallet' derivationMethod")]
    ExpectedHDWallet,
    #[display(fmt = "Trezor derivation method is not supported yet!")]
    Trezor,
    #[display(fmt = "Unsupported error: {_0}")]
    UnsupportedError(String),
    #[display(fmt = "Internal error: {_0}")]
    InternalError(String),
}

impl From<PrivKeyPolicyNotAllowed> for UnexpectedDerivationMethod {
    fn from(e: PrivKeyPolicyNotAllowed) -> Self {
        match e {
            PrivKeyPolicyNotAllowed::HardwareWalletNotSupported => UnexpectedDerivationMethod::Trezor,
            PrivKeyPolicyNotAllowed::UnsupportedMethod(method) => UnexpectedDerivationMethod::UnsupportedError(method),
            PrivKeyPolicyNotAllowed::InternalError(e) => UnexpectedDerivationMethod::InternalError(e),
        }
    }
}

pub trait Transaction: fmt::Debug + 'static {
    /// Raw transaction bytes of the transaction
    fn tx_hex(&self) -> Vec<u8>;
    /// Serializable representation of tx hash for displaying purpose
    fn tx_hash_as_bytes(&self) -> BytesJson;
}

#[derive(Clone, Debug, PartialEq)]
pub enum TransactionEnum {
    UtxoTx(UtxoTx),
    SignedEthTx(SignedEthTx),
    ZTransaction(ZTransaction),
    CosmosTransaction(CosmosTransaction),
    #[cfg(not(target_arch = "wasm32"))]
    LightningPayment(LightningPayment),
}

ifrom!(TransactionEnum, UtxoTx);
ifrom!(TransactionEnum, SignedEthTx);
ifrom!(TransactionEnum, ZTransaction);
#[cfg(not(target_arch = "wasm32"))]
ifrom!(TransactionEnum, LightningPayment);

impl TransactionEnum {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn supports_tx_helper(&self) -> bool {
        !matches!(self, TransactionEnum::LightningPayment(_))
    }

    #[cfg(target_arch = "wasm32")]
    pub fn supports_tx_helper(&self) -> bool {
        true
    }
}

// NB: When stable and groked by IDEs, `enum_dispatch` can be used instead of `Deref` to speed things up.
impl Deref for TransactionEnum {
    type Target = dyn Transaction;
    fn deref(&self) -> &dyn Transaction {
        match self {
            TransactionEnum::UtxoTx(ref t) => t,
            TransactionEnum::SignedEthTx(ref t) => t,
            TransactionEnum::ZTransaction(ref t) => t,
            TransactionEnum::CosmosTransaction(ref t) => t,
            #[cfg(not(target_arch = "wasm32"))]
            TransactionEnum::LightningPayment(ref p) => p,
        }
    }
}

/// Error type for handling tx serialization/deserialization operations.
#[derive(Debug, Clone)]
pub enum TxMarshalingErr {
    InvalidInput(String),
    /// For cases where serialized and deserialized values doesn't verify each other.
    CrossCheckFailed(String),
    NotSupported(String),
    Internal(String),
}

#[derive(Clone, Debug, EnumFromStringify)]
#[allow(clippy::large_enum_variant)]
pub enum TransactionErr {
    /// Keeps transactions while throwing errors.
    TxRecoverable(TransactionEnum, String),
    /// Simply for plain error messages.
    #[from_stringify("keys::Error")]
    Plain(String),
    ProtocolNotSupported(String),
    InternalError(String),
}

impl TransactionErr {
    /// Returns transaction if the error includes it.
    #[inline]
    pub fn get_tx(&self) -> Option<TransactionEnum> {
        match self {
            TransactionErr::TxRecoverable(tx, _) => Some(tx.clone()),
            _ => None,
        }
    }

    #[inline]
    /// Returns plain text part of error.
    pub fn get_plain_text_format(&self) -> String {
        match self {
            TransactionErr::TxRecoverable(_, err) => err.to_string(),
            TransactionErr::Plain(err)
            | TransactionErr::ProtocolNotSupported(err)
            | TransactionErr::InternalError(err) => err.to_string(),
        }
    }
}

impl std::fmt::Display for TransactionErr {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}", self.get_plain_text_format())
    }
}

#[derive(Debug, PartialEq)]
pub enum FoundSwapTxSpend {
    Spent(TransactionEnum),
    Refunded(TransactionEnum),
}

pub enum CanRefundHtlc {
    CanRefundNow,
    // returns the number of seconds to sleep before HTLC becomes refundable
    HaveToWait(u64),
}

#[derive(Debug, Display, Eq, PartialEq)]
pub enum NegotiateSwapContractAddrErr {
    #[display(fmt = "InvalidOtherAddrLen, addr supplied {_0:?}")]
    InvalidOtherAddrLen(BytesJson),
    #[display(fmt = "UnexpectedOtherAddr, addr supplied {_0:?}")]
    UnexpectedOtherAddr(BytesJson),
    NoOtherAddrAndNoFallback,
}

#[derive(Debug, Display, Eq, PartialEq)]
pub enum ValidateOtherPubKeyErr {
    #[display(fmt = "InvalidPubKey: {_0:?}")]
    InvalidPubKey(String),
}

#[derive(Clone, Debug)]
pub struct ConfirmPaymentInput {
    pub payment_tx: Vec<u8>,
    pub confirmations: u64,
    pub requires_nota: bool,
    pub wait_until: u64,
    pub check_every: u64,
}

#[derive(Clone, Debug)]
pub struct WatcherValidateTakerFeeInput {
    pub taker_fee_hash: Vec<u8>,
    pub sender_pubkey: Vec<u8>,
    pub min_block_number: u64,
    pub lock_duration: u64,
}

/// Helper struct wrapping arguments for [WatcherOps::watcher_validate_taker_payment].
#[derive(Clone)]
pub struct WatcherValidatePaymentInput {
    /// Taker payment serialized to raw bytes.
    pub payment_tx: Vec<u8>,
    /// Payment refund preimage generated by taker.
    pub taker_payment_refund_preimage: Vec<u8>,
    /// Taker payment can be refunded after this timestamp.
    pub time_lock: u64,
    /// Taker's pubkey.
    pub taker_pub: Vec<u8>,
    /// Maker's pubkey.
    pub maker_pub: Vec<u8>,
    /// Hash of the secret generated by maker.
    pub secret_hash: Vec<u8>,
    /// Validation timeout.
    pub wait_until: u64,
    /// Required number of taker payment's on-chain confirmations.
    pub confirmations: u64,
    /// Maker coin.
    pub maker_coin: MmCoinEnum,
}

#[derive(Clone)]
pub enum WatcherSpendType {
    TakerPaymentRefund,
    MakerPaymentSpend,
}

#[derive(Clone)]
pub struct ValidateWatcherSpendInput {
    pub payment_tx: Vec<u8>,
    pub maker_pub: Vec<u8>,
    pub swap_contract_address: Option<BytesJson>,
    pub time_lock: u64,
    pub secret_hash: Vec<u8>,
    pub amount: BigDecimal,
    pub watcher_reward: Option<WatcherReward>,
    pub spend_type: WatcherSpendType,
}

/// Helper struct wrapping arguments for [SwapOps::validate_taker_payment] and [SwapOps::validate_maker_payment].
#[derive(Clone, Debug)]
pub struct ValidatePaymentInput {
    /// Payment transaction serialized to raw bytes.
    pub payment_tx: Vec<u8>,
    /// Time lock duration in seconds.
    pub time_lock_duration: u64,
    /// Payment can be refunded after this timestamp.
    pub time_lock: u64,
    /// Pubkey of other side of the swap.
    pub other_pub: Vec<u8>,
    /// Hash of the secret generated by maker.
    pub secret_hash: Vec<u8>,
    /// Expected payment amount.
    pub amount: BigDecimal,
    /// Swap contract address if applicable.
    pub swap_contract_address: Option<BytesJson>,
    /// SPV proof check timeout.
    pub try_spv_proof_until: u64,
    /// Required number of payment's on-chain confirmations.
    pub confirmations: u64,
    /// Unique data of specific swap.
    pub unique_swap_data: Vec<u8>,
    /// The reward assigned to watcher for providing help to complete the swap.
    pub watcher_reward: Option<WatcherReward>,
}

#[derive(Clone, Debug)]
pub struct WatcherSearchForSwapTxSpendInput<'a> {
    pub time_lock: u32,
    pub taker_pub: &'a [u8],
    pub maker_pub: &'a [u8],
    pub secret_hash: &'a [u8],
    pub tx: &'a [u8],
    pub search_from_block: u64,
    pub watcher_reward: bool,
}

#[derive(Clone, Debug)]
pub struct SendMakerPaymentSpendPreimageInput<'a> {
    pub preimage: &'a [u8],
    pub secret_hash: &'a [u8],
    pub secret: &'a [u8],
    pub taker_pub: &'a [u8],
    pub watcher_reward: bool,
}

pub struct SearchForSwapTxSpendInput<'a> {
    pub time_lock: u64,
    pub other_pub: &'a [u8],
    pub secret_hash: &'a [u8],
    pub tx: &'a [u8],
    pub search_from_block: u64,
    pub swap_contract_address: &'a Option<BytesJson>,
    pub swap_unique_data: &'a [u8],
    pub watcher_reward: bool,
}

#[derive(Copy, Clone, Debug)]
pub enum RewardTarget {
    None,
    Contract,
    PaymentSender,
    PaymentSpender,
    PaymentReceiver,
}

#[derive(Clone, Debug)]
pub struct WatcherReward {
    pub amount: BigDecimal,
    pub is_exact_amount: bool,
    pub reward_target: RewardTarget,
    pub send_contract_reward_on_spend: bool,
}

/// Enum representing possible variants of swap transaction including secret hash(es)
#[derive(Debug)]
pub enum SwapTxTypeWithSecretHash<'a> {
    /// Legacy protocol transaction
    TakerOrMakerPayment { maker_secret_hash: &'a [u8] },
    /// Taker funding transaction
    TakerFunding { taker_secret_hash: &'a [u8] },
    /// Maker payment v2 (with immediate refund path)
    MakerPaymentV2 {
        maker_secret_hash: &'a [u8],
        taker_secret_hash: &'a [u8],
    },
    /// Taker payment v2
    TakerPaymentV2 {
        maker_secret_hash: &'a [u8],
        taker_secret_hash: &'a [u8],
    },
}

impl SwapTxTypeWithSecretHash<'_> {
    pub fn redeem_script(&self, time_lock: u32, my_public: &Public, other_public: &Public) -> Script {
        match self {
            SwapTxTypeWithSecretHash::TakerOrMakerPayment { maker_secret_hash } => {
                payment_script(time_lock, maker_secret_hash, my_public, other_public)
            },
            SwapTxTypeWithSecretHash::TakerFunding { taker_secret_hash } => {
                swap_proto_v2_scripts::taker_funding_script(time_lock, taker_secret_hash, my_public, other_public)
            },
            SwapTxTypeWithSecretHash::MakerPaymentV2 {
                maker_secret_hash,
                taker_secret_hash,
            } => swap_proto_v2_scripts::maker_payment_script(
                time_lock,
                maker_secret_hash,
                taker_secret_hash,
                my_public,
                other_public,
            ),
            SwapTxTypeWithSecretHash::TakerPaymentV2 { maker_secret_hash, .. } => {
                swap_proto_v2_scripts::taker_payment_script(time_lock, maker_secret_hash, my_public, other_public)
            },
        }
    }

    pub fn op_return_data(&self) -> Vec<u8> {
        match self {
            SwapTxTypeWithSecretHash::TakerOrMakerPayment { maker_secret_hash } => maker_secret_hash.to_vec(),
            SwapTxTypeWithSecretHash::TakerFunding { taker_secret_hash } => taker_secret_hash.to_vec(),
            SwapTxTypeWithSecretHash::MakerPaymentV2 {
                maker_secret_hash,
                taker_secret_hash,
            } => [*maker_secret_hash, *taker_secret_hash].concat(),
            SwapTxTypeWithSecretHash::TakerPaymentV2 { maker_secret_hash, .. } => maker_secret_hash.to_vec(),
        }
    }
}

/// Helper struct wrapping arguments for [SwapOps::send_taker_payment] and [SwapOps::send_maker_payment].
#[derive(Clone, Debug)]
pub struct SendPaymentArgs<'a> {
    /// Time lock duration in seconds.
    pub time_lock_duration: u64,
    /// Payment can be refunded after this timestamp.
    pub time_lock: u64,
    /// This is either:
    /// * Taker's pubkey if this structure is used in [`SwapOps::send_maker_payment`].
    /// * Maker's pubkey if this structure is used in [`SwapOps::send_taker_payment`].
    pub other_pubkey: &'a [u8],
    /// Hash of the secret generated by maker.
    pub secret_hash: &'a [u8],
    /// Payment amount
    pub amount: BigDecimal,
    /// Swap contract address if applicable.
    pub swap_contract_address: &'a Option<BytesJson>,
    /// Unique data of specific swap.
    pub swap_unique_data: &'a [u8],
    /// Instructions for the next step of the swap (e.g., Lightning invoice).
    pub payment_instructions: &'a Option<PaymentInstructions>,
    /// The reward assigned to watcher for providing help to complete the swap.
    pub watcher_reward: Option<WatcherReward>,
    /// As of now, this field is specifically used to wait for confirmations of ERC20 approval transaction.
    pub wait_for_confirmation_until: u64,
}

#[derive(Clone, Debug)]
pub struct SpendPaymentArgs<'a> {
    /// This is either:
    /// * Taker's payment tx if this structure is used in [`SwapOps::send_maker_spends_taker_payment`].
    /// * Maker's payment tx if this structure is used in [`SwapOps::send_taker_spends_maker_payment`].
    pub other_payment_tx: &'a [u8],
    pub time_lock: u64,
    /// This is either:
    /// * Taker's pubkey if this structure is used in [`SwapOps::send_maker_spends_taker_payment`].
    /// * Maker's pubkey if this structure is used in [`SwapOps::send_taker_spends_maker_payment`].
    pub other_pubkey: &'a [u8],
    pub secret: &'a [u8],
    pub secret_hash: &'a [u8],
    pub swap_contract_address: &'a Option<BytesJson>,
    pub swap_unique_data: &'a [u8],
    pub watcher_reward: bool,
}

#[derive(Debug)]
pub struct RefundPaymentArgs<'a> {
    pub payment_tx: &'a [u8],
    pub time_lock: u64,
    /// This is either:
    /// * Taker's pubkey if this structure is used in [`SwapOps::send_maker_refunds_payment`].
    /// * Maker's pubkey if this structure is used in [`SwapOps::send_taker_refunds_payment`].
    pub other_pubkey: &'a [u8],
    pub tx_type_with_secret_hash: SwapTxTypeWithSecretHash<'a>,
    pub swap_contract_address: &'a Option<BytesJson>,
    pub swap_unique_data: &'a [u8],
    pub watcher_reward: bool,
}

#[derive(Debug)]
pub struct RefundMakerPaymentTimelockArgs<'a> {
    pub payment_tx: &'a [u8],
    pub time_lock: u64,
    pub taker_pub: &'a [u8],
    pub tx_type_with_secret_hash: SwapTxTypeWithSecretHash<'a>,
    pub swap_unique_data: &'a [u8],
    pub watcher_reward: bool,
    pub amount: BigDecimal,
}

#[derive(Debug)]
pub struct RefundTakerPaymentArgs<'a> {
    pub payment_tx: &'a [u8],
    pub time_lock: u64,
    pub maker_pub: &'a [u8],
    pub tx_type_with_secret_hash: SwapTxTypeWithSecretHash<'a>,
    pub swap_unique_data: &'a [u8],
    pub watcher_reward: bool,
    pub dex_fee: &'a DexFee,
    /// Additional reward for maker (premium)
    pub premium_amount: BigDecimal,
    /// Actual volume of taker's payment
    pub trading_amount: BigDecimal,
}

/// Helper struct wrapping arguments for [SwapOps::check_if_my_payment_sent].
#[derive(Clone, Debug)]
pub struct CheckIfMyPaymentSentArgs<'a> {
    /// Payment can be refunded after this timestamp.
    pub time_lock: u64,
    /// Pubkey of other side of the swap.
    pub other_pub: &'a [u8],
    /// Hash of the secret generated by maker.
    pub secret_hash: &'a [u8],
    /// Search after specific block to avoid scanning entire blockchain.
    pub search_from_block: u64,
    /// Swap contract address if applicable.
    pub swap_contract_address: &'a Option<BytesJson>,
    /// Unique data of specific swap.
    pub swap_unique_data: &'a [u8],
    /// Payment amount.
    pub amount: &'a BigDecimal,
    /// Instructions for the next step of the swap (e.g., Lightning invoice).
    pub payment_instructions: &'a Option<PaymentInstructions>,
}

#[derive(Clone, Debug)]
pub struct ValidateFeeArgs<'a> {
    pub fee_tx: &'a TransactionEnum,
    pub expected_sender: &'a [u8],
    pub dex_fee: &'a DexFee,
    pub min_block_number: u64,
    pub uuid: &'a [u8],
}

pub struct EthValidateFeeArgs<'a> {
    pub fee_tx_hash: &'a H256,
    pub expected_sender: &'a [u8],
    pub amount: &'a BigDecimal,
    pub min_block_number: u64,
    pub uuid: &'a [u8],
}

#[derive(Clone, Debug)]
pub struct WaitForHTLCTxSpendArgs<'a> {
    pub tx_bytes: &'a [u8],
    pub secret_hash: &'a [u8],
    pub wait_until: u64,
    pub from_block: u64,
    pub swap_contract_address: &'a Option<BytesJson>,
    pub check_every: f64,
    pub watcher_reward: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum PaymentInstructions {
    #[cfg(not(target_arch = "wasm32"))]
    Lightning(Invoice),
    WatcherReward(BigDecimal),
}

#[derive(Clone, Debug, Default)]
pub struct PaymentInstructionArgs<'a> {
    pub secret_hash: &'a [u8],
    pub amount: BigDecimal,
    pub maker_lock_duration: u64,
    pub expires_in: u64,
    pub watcher_reward: bool,
    pub wait_until: u64,
}

#[derive(Display, EnumFromStringify)]
pub enum PaymentInstructionsErr {
    LightningInvoiceErr(String),
    WatcherRewardErr(String),
    #[from_stringify("NumConversError")]
    InternalError(String),
}

#[derive(Display)]
pub enum ValidateInstructionsErr {
    ValidateLightningInvoiceErr(String),
    UnsupportedCoin(String),
    DeserializationErr(String),
}

#[cfg(not(target_arch = "wasm32"))]
impl From<ParseOrSemanticError> for ValidateInstructionsErr {
    fn from(e: ParseOrSemanticError) -> Self {
        ValidateInstructionsErr::ValidateLightningInvoiceErr(e.to_string())
    }
}

#[derive(Display)]
pub enum RefundError {
    DecodeErr(String),
    DbError(String),
    Timeout(String),
    Internal(String),
}

#[derive(Debug, Display)]
pub enum WatcherRewardError {
    RPCError(String),
    InvalidCoinType(String),
    InternalError(String),
    #[display(fmt = "No such coin {}", coin)]
    NoSuchCoin {
        coin: String,
    },
}

impl From<CoinFindError> for WatcherRewardError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => WatcherRewardError::NoSuchCoin { coin },
        }
    }
}

/// Swap operations (mostly based on the Hash/Time locked transactions implemented by coin wallets).
#[async_trait]
#[cfg_attr(any(test, feature = "for-tests"), mockable)]
pub trait SwapOps {
    async fn send_taker_fee(&self, dex_fee: DexFee, uuid: &[u8], expire_at: u64) -> TransactionResult;

    async fn send_maker_payment(&self, maker_payment_args: SendPaymentArgs<'_>) -> TransactionResult;

    async fn send_taker_payment(&self, taker_payment_args: SendPaymentArgs<'_>) -> TransactionResult;

    async fn send_maker_spends_taker_payment(
        &self,
        maker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult;

    async fn send_taker_spends_maker_payment(
        &self,
        taker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult;

    async fn send_taker_refunds_payment(&self, taker_refunds_payment_args: RefundPaymentArgs<'_>) -> TransactionResult;

    async fn send_maker_refunds_payment(&self, maker_refunds_payment_args: RefundPaymentArgs<'_>) -> TransactionResult;

    async fn validate_fee(&self, validate_fee_args: ValidateFeeArgs<'_>) -> ValidatePaymentResult<()>;

    async fn validate_maker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()>;

    async fn validate_taker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()>;

    async fn check_if_my_payment_sent(
        &self,
        if_my_payment_sent_args: CheckIfMyPaymentSentArgs<'_>,
    ) -> Result<Option<TransactionEnum>, String>;

    async fn search_for_swap_tx_spend_my(
        &self,
        input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String>;

    async fn search_for_swap_tx_spend_other(
        &self,
        input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String>;

    async fn extract_secret(
        &self,
        secret_hash: &[u8],
        spend_tx: &[u8],
        watcher_reward: bool,
    ) -> Result<[u8; 32], String>;

    /// Whether the refund transaction can be sent now
    /// For example: there are no additional conditions for ETH, but for some UTXO coins we should wait for
    /// locktime < MTP
    async fn can_refund_htlc(&self, locktime: u64) -> Result<CanRefundHtlc, String> {
        let now = now_sec();
        if now > locktime {
            Ok(CanRefundHtlc::CanRefundNow)
        } else {
            Ok(CanRefundHtlc::HaveToWait(locktime - now + 1))
        }
    }

    /// Whether the swap payment is refunded automatically or not when the locktime expires, or the other side fails the HTLC.
    /// lightning specific
    fn is_auto_refundable(&self) -> bool {
        false
    }

    /// Waits for an htlc to be refunded automatically. - lightning specific
    async fn wait_for_htlc_refund(&self, _tx: &[u8], _locktime: u64) -> RefundResult<()> {
        MmError::err(RefundError::Internal(
            "wait_for_htlc_refund is not supported for this coin!".into(),
        ))
    }

    fn negotiate_swap_contract_addr(
        &self,
        other_side_address: Option<&[u8]>,
    ) -> Result<Option<BytesJson>, MmError<NegotiateSwapContractAddrErr>>;

    /// Consider using [`SwapOps::derive_htlc_pubkey`] if you need the public key only.
    /// Some coins may not have a private key.
    fn derive_htlc_key_pair(&self, swap_unique_data: &[u8]) -> KeyPair;

    /// Derives an HTLC key-pair and returns a public key corresponding to that key.
    fn derive_htlc_pubkey(&self, swap_unique_data: &[u8]) -> [u8; 33];

    fn validate_other_pubkey(&self, raw_pubkey: &[u8]) -> MmResult<(), ValidateOtherPubKeyErr>;

    /// Instructions from the taker on how the maker should send his payment. - lightning specific
    async fn maker_payment_instructions(
        &self,
        _args: PaymentInstructionArgs<'_>,
    ) -> Result<Option<Vec<u8>>, MmError<PaymentInstructionsErr>> {
        Ok(None)
    }

    /// Instructions from the maker on how the taker should send his payment. - lightning specific
    async fn taker_payment_instructions(
        &self,
        _args: PaymentInstructionArgs<'_>,
    ) -> Result<Option<Vec<u8>>, MmError<PaymentInstructionsErr>> {
        Ok(None)
    }

    /// lightning specific
    fn validate_maker_payment_instructions(
        &self,
        _instructions: &[u8],
        _args: PaymentInstructionArgs<'_>,
    ) -> Result<PaymentInstructions, MmError<ValidateInstructionsErr>> {
        MmError::err(ValidateInstructionsErr::UnsupportedCoin(
            "validate_maker_payment_instructions is not supported for this coin!".into(),
        ))
    }

    /// lightning specific
    fn validate_taker_payment_instructions(
        &self,
        _instructions: &[u8],
        _args: PaymentInstructionArgs<'_>,
    ) -> Result<PaymentInstructions, MmError<ValidateInstructionsErr>> {
        MmError::err(ValidateInstructionsErr::UnsupportedCoin(
            "validate_taker_payment_instructions is not supported for this coin!".into(),
        ))
    }

    fn is_supported_by_watchers(&self) -> bool {
        false
    }

    // Do we also need a method for the fallback contract?
    fn contract_supports_watchers(&self) -> bool {
        true
    }

    fn maker_locktime_multiplier(&self) -> f64 {
        2.0
    }

    fn dex_pubkey(&self) -> &[u8] {
        &DEX_FEE_ADDR_RAW_PUBKEY
    }

    fn burn_pubkey(&self) -> &[u8] {
        #[cfg(feature = "for-tests")]
        {
            lazy_static! {
                static ref TEST_BURN_ADDR_RAW_PUBKEY: Option<Vec<u8>> = std::env::var("TEST_BURN_ADDR_RAW_PUBKEY")
                    .ok()
                    .map(|env_pubkey| hex::decode(env_pubkey).expect("valid hex"));
            }
            if let Some(test_pk) = TEST_BURN_ADDR_RAW_PUBKEY.as_ref() {
                return test_pk;
            }
        }
        &DEX_BURN_ADDR_RAW_PUBKEY
    }

    /// Performs an action on Maker coin payment just before the Taker Swap payment refund begins
    /// Operation on maker coin from taker swap side
    /// Currently lightning specific
    async fn on_taker_payment_refund_start(&self, _maker_payment: &[u8]) -> RefundResult<()> {
        Ok(())
    }

    /// Performs an action on Maker coin payment after the Taker Swap payment is refunded successfully
    /// Operation on maker coin from taker swap side
    /// Currently lightning specific
    async fn on_taker_payment_refund_success(&self, _maker_payment: &[u8]) -> RefundResult<()> {
        Ok(())
    }

    /// Performs an action on Taker coin payment just before the Maker Swap payment refund begins
    /// Operation on taker coin from maker swap side
    /// Currently lightning specific
    async fn on_maker_payment_refund_start(&self, _taker_payment: &[u8]) -> RefundResult<()> {
        Ok(())
    }

    /// Performs an action on Taker coin payment after the Maker Swap payment is refunded successfully
    /// Operation on taker coin from maker swap side
    /// Currently lightning specific
    async fn on_maker_payment_refund_success(&self, _taker_payment: &[u8]) -> RefundResult<()> {
        Ok(())
    }
}

#[async_trait]
pub trait WatcherOps {
    fn send_maker_payment_spend_preimage(&self, _input: SendMakerPaymentSpendPreimageInput) -> TransactionFut {
        Box::new(
            futures::future::ready(Err(TransactionErr::Plain(
                "send_maker_payment_spend_preimage is not implemented".to_string(),
            )))
            .compat(),
        )
    }

    fn send_taker_payment_refund_preimage(&self, _watcher_refunds_payment_args: RefundPaymentArgs) -> TransactionFut {
        Box::new(
            futures::future::ready(Err(TransactionErr::Plain(
                "send_taker_payment_refund_preimage is not implemented".to_string(),
            )))
            .compat(),
        )
    }

    fn create_taker_payment_refund_preimage(
        &self,
        _taker_payment_tx: &[u8],
        _time_lock: u64,
        _maker_pub: &[u8],
        _secret_hash: &[u8],
        _swap_contract_address: &Option<BytesJson>,
        _swap_unique_data: &[u8],
    ) -> TransactionFut {
        Box::new(
            futures::future::ready(Err(TransactionErr::Plain(
                "create_taker_payment_refund_preimage is not implemented".to_string(),
            )))
            .compat(),
        )
    }

    fn create_maker_payment_spend_preimage(
        &self,
        _maker_payment_tx: &[u8],
        _time_lock: u64,
        _maker_pub: &[u8],
        _secret_hash: &[u8],
        _swap_unique_data: &[u8],
    ) -> TransactionFut {
        Box::new(
            futures::future::ready(Err(TransactionErr::Plain(
                "create_maker_payment_spend_preimage is not implemented".to_string(),
            )))
            .compat(),
        )
    }

    fn watcher_validate_taker_fee(&self, _input: WatcherValidateTakerFeeInput) -> ValidatePaymentFut<()> {
        Box::new(
            futures::future::ready(MmError::err(ValidatePaymentError::InternalError(
                "watcher_validate_taker_fee is not implemented".to_string(),
            )))
            .compat(),
        )
    }

    fn watcher_validate_taker_payment(&self, _input: WatcherValidatePaymentInput) -> ValidatePaymentFut<()> {
        Box::new(
            futures::future::ready(MmError::err(ValidatePaymentError::InternalError(
                "watcher_validate_taker_payment is not implemented".to_string(),
            )))
            .compat(),
        )
    }

    fn taker_validates_payment_spend_or_refund(&self, _input: ValidateWatcherSpendInput) -> ValidatePaymentFut<()> {
        Box::new(
            futures::future::ready(MmError::err(ValidatePaymentError::InternalError(
                "taker_validates_payment_spend_or_refund is not implemented".to_string(),
            )))
            .compat(),
        )
    }

    async fn watcher_search_for_swap_tx_spend(
        &self,
        _input: WatcherSearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        Err("watcher_search_for_swap_tx_spend is not implemented".to_string())
    }

    async fn get_taker_watcher_reward(
        &self,
        _other_coin: &MmCoinEnum,
        _coin_amount: Option<BigDecimal>,
        _other_coin_amount: Option<BigDecimal>,
        _reward_amount: Option<BigDecimal>,
        _wait_until: u64,
    ) -> Result<WatcherReward, MmError<WatcherRewardError>> {
        Err(WatcherRewardError::InternalError(
            "get_taker_watcher_reward is not implemented".to_string(),
        ))?
    }

    async fn get_maker_watcher_reward(
        &self,
        _other_coin: &MmCoinEnum,
        _reward_amount: Option<BigDecimal>,
        _wait_until: u64,
    ) -> Result<Option<WatcherReward>, MmError<WatcherRewardError>> {
        Err(WatcherRewardError::InternalError(
            "get_maker_watcher_reward is not implemented".to_string(),
        ))?
    }
}

/// Helper struct wrapping arguments for [TakerCoinSwapOpsV2::send_taker_funding]
pub struct SendTakerFundingArgs<'a> {
    /// For UTXO-based coins, the taker can refund the funding after this timestamp if the maker hasn't claimed it.
    /// For smart contracts, the taker can refund the payment after this timestamp if the maker hasn't pre-approved the transaction.
    /// This field is additionally used to wait for confirmations of ERC20 approval transaction.
    pub funding_time_lock: u64,
    /// For smart contracts, the taker can refund the payment after this timestamp if the maker hasn't claimed it by revealing their secret.
    pub payment_time_lock: u64,
    /// The hash of the secret generated by the taker, needed for immediate refund
    pub taker_secret_hash: &'a [u8],
    /// The hash of the secret generated by the maker, needed to spend the payment
    pub maker_secret_hash: &'a [u8],
    /// Maker's pubkey
    pub maker_pub: &'a [u8],
    /// DEX fee
    pub dex_fee: &'a DexFee,
    /// Additional reward for maker (premium)
    pub premium_amount: BigDecimal,
    /// Actual volume of taker's payment
    pub trading_amount: BigDecimal,
    /// Unique data of specific swap
    pub swap_unique_data: &'a [u8],
}

/// Helper struct wrapping arguments for [TakerCoinSwapOpsV2::refund_taker_funding_secret]
pub struct RefundFundingSecretArgs<'a, Coin: ParseCoinAssocTypes + ?Sized> {
    pub funding_tx: &'a Coin::Tx,
    pub funding_time_lock: u64,
    pub payment_time_lock: u64,
    pub maker_pubkey: &'a Coin::Pubkey,
    pub taker_secret: &'a [u8; 32],
    pub taker_secret_hash: &'a [u8],
    pub maker_secret_hash: &'a [u8],
    pub dex_fee: &'a DexFee,
    /// Additional reward for maker (premium)
    pub premium_amount: BigDecimal,
    /// Actual volume of taker's payment
    pub trading_amount: BigDecimal,
    pub swap_unique_data: &'a [u8],
    pub watcher_reward: bool,
}

/// Helper struct wrapping arguments for [TakerCoinSwapOpsV2::gen_taker_funding_spend_preimage]
pub struct GenTakerFundingSpendArgs<'a, Coin: ParseCoinAssocTypes + ?Sized> {
    /// Taker payment transaction serialized to raw bytes
    pub funding_tx: &'a Coin::Tx,
    /// Maker's pubkey
    pub maker_pub: &'a Coin::Pubkey,
    /// Taker's pubkey
    pub taker_pub: &'a Coin::Pubkey,
    /// Timelock of the funding tx
    pub funding_time_lock: u64,
    /// The hash of the secret generated by taker
    pub taker_secret_hash: &'a [u8],
    /// Timelock of the taker payment
    pub taker_payment_time_lock: u64,
    /// The hash of the secret generated by maker
    pub maker_secret_hash: &'a [u8],
}

/// Helper struct wrapping arguments for [TakerCoinSwapOpsV2::validate_taker_funding]
pub struct ValidateTakerFundingArgs<'a, Coin: ParseCoinAssocTypes + ?Sized> {
    /// Taker funding transaction
    pub funding_tx: &'a Coin::Tx,
    /// In EVM case: The timestamp after which the taker can refund the funding transaction if the taker hasn't pre-approved the transaction
    /// In UTXO case: Taker will be able to refund the payment after this timestamp
    pub funding_time_lock: u64,
    /// In EVM case: The timestamp after which the taker can refund the payment transaction if the maker hasn't claimed it by revealing their secret.
    /// UTXO doesn't use it
    pub payment_time_lock: u64,
    /// The hash of the secret generated by taker
    pub taker_secret_hash: &'a [u8],
    /// The hash of the secret generated by maker
    pub maker_secret_hash: &'a [u8],
    /// Taker's pubkey
    pub taker_pub: &'a Coin::Pubkey,
    /// DEX fee amount
    pub dex_fee: &'a DexFee,
    /// Additional reward for maker (premium)
    pub premium_amount: BigDecimal,
    /// Actual volume of taker's payment
    pub trading_amount: BigDecimal,
    /// Unique data of specific swap
    pub swap_unique_data: &'a [u8],
}

/// Helper struct wrapping arguments for taker payment's spend generation, used in
/// [TakerCoinSwapOpsV2::gen_taker_payment_spend_preimage], [TakerCoinSwapOpsV2::validate_taker_payment_spend_preimage] and
/// [TakerCoinSwapOpsV2::sign_and_broadcast_taker_payment_spend]
pub struct GenTakerPaymentSpendArgs<'a, Coin: ParseCoinAssocTypes + ?Sized> {
    /// Taker payment transaction serialized to raw bytes
    pub taker_tx: &'a Coin::Tx,
    /// Taker will be able to refund the payment after this timestamp
    pub time_lock: u64,
    /// The hash of the secret generated by maker
    pub maker_secret_hash: &'a [u8],
    /// Maker's pubkey
    pub maker_pub: &'a Coin::Pubkey,
    /// Maker's address
    pub maker_address: &'a Coin::Address,
    /// Taker's pubkey
    pub taker_pub: &'a Coin::Pubkey,
    /// DEX fee
    pub dex_fee: &'a DexFee,
    /// Additional reward for maker (premium)
    pub premium_amount: BigDecimal,
    /// Actual volume of taker's payment
    pub trading_amount: BigDecimal,
}

/// Taker payment spend preimage with taker's signature
pub struct TxPreimageWithSig<Coin: ParseCoinAssocTypes + ?Sized> {
    /// The preimage, might be () for certain coin types (only signature might be used)
    pub preimage: Coin::Preimage,
    /// Taker's signature
    pub signature: Coin::Sig,
}

/// Enum covering error cases that can happen during transaction preimage generation.
#[derive(Debug, Display)]
pub enum TxGenError {
    /// RPC error
    Rpc(String),
    /// Error during conversion of BigDecimal amount to coin's specific monetary units (satoshis, wei, etc.).
    NumConversion(String),
    /// Problem with tx preimage signing.
    Signing(String),
    /// Legacy error produced by usage of try_s/try_fus and other similar macros.
    Legacy(String),
    /// Input payment timelock overflows the type used by specific coin.
    LocktimeOverflow(String),
    /// Transaction fee is too high
    TxFeeTooHigh(String),
    /// Previous tx is not valid
    PrevTxIsNotValid(String),
    /// Previous tx output value too low
    PrevOutputTooLow(String),
    /// Other errors, can be used to return an error that can happen only in specific coin protocol implementation
    Other(String),
}

impl From<UtxoRpcError> for TxGenError {
    fn from(err: UtxoRpcError) -> Self {
        TxGenError::Rpc(err.to_string())
    }
}

impl From<NumConversError> for TxGenError {
    fn from(err: NumConversError) -> Self {
        TxGenError::NumConversion(err.to_string())
    }
}

impl From<UtxoSignWithKeyPairError> for TxGenError {
    fn from(err: UtxoSignWithKeyPairError) -> Self {
        TxGenError::Signing(err.to_string())
    }
}

/// Enum covering error cases that can happen during swap v2 transaction validation.
#[derive(Debug, Display, EnumFromStringify)]
pub enum ValidateSwapV2TxError {
    /// Payment sent to wrong address or has invalid amount.
    InvalidDestinationOrAmount(String),
    /// Error during conversion of BigDecimal amount to coin's specific monetary units (satoshis, wei, etc.).
    NumConversion(String),
    /// RPC error.
    #[from_stringify("web3::Error")]
    Rpc(String),
    /// Serialized tx bytes don't match ones received from coin's RPC.
    #[display(fmt = "Tx bytes {actual:02x} don't match ones received from rpc {from_rpc:02x}")]
    TxBytesMismatch {
        from_rpc: BytesJson,
        actual: BytesJson,
    },
    /// Provided transaction doesn't have output with specific index
    TxLacksOfOutputs,
    /// Indicates that overflow occurred, either while calculating a total payment or converting the timelock.
    Overflow(String),
    /// Internal error
    #[from_stringify("ethabi::Error", "TryFromSliceError")]
    Internal(String),
    /// Payment transaction is in unexpected state. E.g., `Uninitialized` instead of `PaymentSent` for ETH payment.
    UnexpectedPaymentState(String),
    /// Payment transaction doesn't exist on-chain.
    TxDoesNotExist(String),
    /// Transaction has wrong properties, for example, it has been sent to a wrong address.
    WrongPaymentTx(String),
    ProtocolNotSupported(String),
    InvalidData(String),
}

impl From<NumConversError> for ValidateSwapV2TxError {
    fn from(err: NumConversError) -> Self {
        ValidateSwapV2TxError::NumConversion(err.to_string())
    }
}

impl From<UtxoRpcError> for ValidateSwapV2TxError {
    fn from(err: UtxoRpcError) -> Self {
        ValidateSwapV2TxError::Rpc(err.to_string())
    }
}

impl From<ValidatePaymentV2Err> for ValidateSwapV2TxError {
    fn from(err: ValidatePaymentV2Err) -> Self {
        match err {
            ValidatePaymentV2Err::WrongPaymentTx(e) => ValidateSwapV2TxError::WrongPaymentTx(e),
        }
    }
}

impl From<PrepareTxDataError> for ValidateSwapV2TxError {
    fn from(err: PrepareTxDataError) -> Self {
        match err {
            PrepareTxDataError::ABIError(e) | PrepareTxDataError::Internal(e) => ValidateSwapV2TxError::Internal(e),
            PrepareTxDataError::InvalidData(e) => ValidateSwapV2TxError::InvalidData(e),
        }
    }
}

/// Enum covering error cases that can happen during taker funding spend preimage validation.
#[derive(Debug, Display, EnumFromStringify)]
pub enum ValidateTakerFundingSpendPreimageError {
    /// Funding tx has no outputs
    FundingTxNoOutputs,
    /// Actual preimage fee is either too high or too small
    UnexpectedPreimageFee(String),
    /// Error during signature deserialization.
    InvalidMakerSignature,
    /// Error during preimage comparison to an expected one.
    InvalidPreimage(String),
    /// Error during taker's signature check.
    #[from_stringify("UtxoSignWithKeyPairError")]
    SignatureVerificationFailure(String),
    /// Error during generation of an expected preimage.
    TxGenError(String),
    /// Input payment timelock overflows the type used by specific coin.
    LocktimeOverflow(String),
    /// Coin's RPC error
    #[from_stringify("UtxoRpcError")]
    Rpc(String),
}

impl From<TxGenError> for ValidateTakerFundingSpendPreimageError {
    fn from(err: TxGenError) -> Self {
        ValidateTakerFundingSpendPreimageError::TxGenError(format!("{err:?}"))
    }
}

/// Enum covering error cases that can happen during taker payment spend preimage validation.
#[derive(Debug, Display, EnumFromStringify)]
pub enum ValidateTakerPaymentSpendPreimageError {
    /// Error during signature deserialization.
    InvalidTakerSignature,
    /// Error during preimage comparison to an expected one.
    InvalidPreimage(String),
    /// Error during taker's signature check.
    #[from_stringify("UtxoSignWithKeyPairError")]
    SignatureVerificationFailure(String),
    /// Error during generation of an expected preimage.
    TxGenError(String),
    /// Input payment timelock overflows the type used by specific coin.
    LocktimeOverflow(String),
}

impl From<TxGenError> for ValidateTakerPaymentSpendPreimageError {
    fn from(err: TxGenError) -> Self {
        ValidateTakerPaymentSpendPreimageError::TxGenError(format!("{err:?}"))
    }
}

/// Helper trait used for various types serialization to bytes
pub trait ToBytes {
    fn to_bytes(&self) -> Vec<u8>;
}

/// Defines associated types specific to each coin (Pubkey, Address, etc.)
#[async_trait]
pub trait ParseCoinAssocTypes {
    type Address: Send + Sync + fmt::Display + AddrToString;
    type AddressParseError: fmt::Debug + Send + fmt::Display;
    type Pubkey: ToBytes + Send + Sync;
    type PubkeyParseError: fmt::Debug + Send + fmt::Display;
    type Tx: Transaction + Send + Sync;
    type TxParseError: fmt::Debug + Send + fmt::Display;
    type Preimage: ToBytes + Send + Sync;
    type PreimageParseError: fmt::Debug + Send + fmt::Display;
    type Sig: ToBytes + Send + Sync;
    type SigParseError: fmt::Debug + Send + fmt::Display;

    async fn my_addr(&self) -> Self::Address;

    fn parse_address(&self, address: &str) -> Result<Self::Address, Self::AddressParseError>;

    fn parse_pubkey(&self, pubkey: &[u8]) -> Result<Self::Pubkey, Self::PubkeyParseError>;

    fn parse_tx(&self, tx: &[u8]) -> Result<Self::Tx, Self::TxParseError>;

    fn parse_preimage(&self, tx: &[u8]) -> Result<Self::Preimage, Self::PreimageParseError>;

    fn parse_signature(&self, sig: &[u8]) -> Result<Self::Sig, Self::SigParseError>;
}

/// Defines associated types specific to Non-Fungible Tokens (Token Address, Token Id, etc.)
pub trait ParseNftAssocTypes {
    type ContractAddress: Send + Sync + fmt::Display;
    type TokenId: ToBytes + Send + Sync;
    type ContractType: ToBytes + Send + Sync;
    type NftAssocTypesError: fmt::Debug + Send + fmt::Display;

    fn parse_contract_address(
        &self,
        contract_address: &[u8],
    ) -> Result<Self::ContractAddress, Self::NftAssocTypesError>;

    fn parse_token_id(&self, token_id: &[u8]) -> Result<Self::TokenId, Self::NftAssocTypesError>;

    fn parse_contract_type(&self, contract_type: &[u8]) -> Result<Self::ContractType, Self::NftAssocTypesError>;
}

pub struct SendMakerPaymentArgs<'a, Coin: ParseCoinAssocTypes + ?Sized> {
    /// Maker will be able to refund the payment after this timestamp
    pub time_lock: u64,
    /// The hash of the secret generated by taker, this is used for immediate refund
    pub taker_secret_hash: &'a [u8],
    /// The hash of the secret generated by maker, taker needs it to spend the payment
    pub maker_secret_hash: &'a [u8],
    /// Payment amount
    pub amount: BigDecimal,
    /// Taker's HTLC pubkey
    pub taker_pub: &'a Coin::Pubkey,
    /// Unique data of specific swap
    pub swap_unique_data: &'a [u8],
}

/// Structure representing necessary NFT info for Swap
pub struct NftSwapInfo<'a, Coin: ParseNftAssocTypes + ?Sized> {
    /// The address of the NFT token
    pub token_address: &'a Coin::ContractAddress,
    /// The ID of the NFT token.
    pub token_id: &'a [u8],
    /// The type of smart contract that governs this NFT
    pub contract_type: &'a Coin::ContractType,
}

pub struct SendNftMakerPaymentArgs<'a, Coin: ParseCoinAssocTypes + ParseNftAssocTypes + ?Sized> {
    /// Maker will be able to refund the payment after this timestamp
    pub time_lock: u64,
    /// The hash of the secret generated by taker, this is used for immediate refund
    pub taker_secret_hash: &'a [u8],
    /// The hash of the secret generated by maker, taker needs it to spend the payment
    pub maker_secret_hash: &'a [u8],
    /// Payment amount
    pub amount: BigDecimal,
    /// Taker's HTLC pubkey
    pub taker_pub: &'a Coin::Pubkey,
    /// Unique data of specific swap
    pub swap_unique_data: &'a [u8],
    /// Structure representing necessary NFT info for Swap
    pub nft_swap_info: &'a NftSwapInfo<'a, Coin>,
}

pub struct ValidateMakerPaymentArgs<'a, Coin: ParseCoinAssocTypes + ?Sized> {
    /// Maker payment tx
    pub maker_payment_tx: &'a Coin::Tx,
    /// Maker will be able to refund the payment after this timestamp
    pub time_lock: u64,
    /// The hash of the secret generated by taker, this is used for immediate refund
    pub taker_secret_hash: &'a [u8],
    /// The hash of the secret generated by maker, taker needs it to spend the payment
    pub maker_secret_hash: &'a [u8],
    /// Payment amount
    pub amount: BigDecimal,
    /// Maker's HTLC pubkey
    pub maker_pub: &'a Coin::Pubkey,
    /// Unique data of specific swap
    pub swap_unique_data: &'a [u8],
}

pub struct ValidateNftMakerPaymentArgs<'a, Coin: ParseCoinAssocTypes + ParseNftAssocTypes + ?Sized> {
    /// Maker payment tx
    pub maker_payment_tx: &'a Coin::Tx,
    /// Maker will be able to refund the payment after this timestamp
    pub time_lock: u64,
    /// The hash of the secret generated by taker, this is used for immediate refund
    pub taker_secret_hash: &'a [u8],
    /// The hash of the secret generated by maker, taker needs it to spend the payment
    pub maker_secret_hash: &'a [u8],
    /// Payment amount
    pub amount: BigDecimal,
    /// Taker's HTLC pubkey
    pub taker_pub: &'a Coin::Pubkey,
    /// Maker's HTLC pubkey
    pub maker_pub: &'a Coin::Pubkey,
    /// Unique data of specific swap
    pub swap_unique_data: &'a [u8],
    /// Structure representing necessary NFT info for Swap
    pub nft_swap_info: &'a NftSwapInfo<'a, Coin>,
}

pub struct RefundMakerPaymentSecretArgs<'a, Coin: ParseCoinAssocTypes + ?Sized> {
    /// Maker payment tx
    pub maker_payment_tx: &'a Coin::Tx,
    /// Maker will be able to refund the payment after this timestamp
    pub time_lock: u64,
    /// The hash of the secret generated by taker, this is used for immediate refund
    pub taker_secret_hash: &'a [u8],
    /// The hash of the secret generated by maker, taker needs it to spend the payment
    pub maker_secret_hash: &'a [u8],
    /// Taker's secret
    pub taker_secret: &'a [u8; 32],
    /// Taker's HTLC pubkey
    pub taker_pub: &'a Coin::Pubkey,
    /// Unique data of specific swap
    pub swap_unique_data: &'a [u8],
    pub amount: BigDecimal,
}

/// Common refund NFT Maker Payment structure for [MakerNftSwapOpsV2::refund_nft_maker_payment_v2_timelock] and
/// [MakerNftSwapOpsV2::refund_nft_maker_payment_v2_secret] methods
pub struct RefundNftMakerPaymentArgs<'a, Coin: ParseCoinAssocTypes + ParseNftAssocTypes + ?Sized> {
    /// Maker payment tx
    pub maker_payment_tx: &'a Coin::Tx,
    /// The hash of the secret generated by taker, this is used for immediate refund
    pub taker_secret_hash: &'a [u8],
    /// The hash of the secret generated by maker, taker needs it to spend the payment
    pub maker_secret_hash: &'a [u8],
    /// Taker's secret
    pub taker_secret: &'a [u8],
    /// Unique data of specific swap
    pub swap_unique_data: &'a [u8],
    /// The type of smart contract that governs this NFT
    pub contract_type: &'a Coin::ContractType,
}

pub struct SpendMakerPaymentArgs<'a, Coin: ParseCoinAssocTypes + ?Sized> {
    /// Maker payment tx
    pub maker_payment_tx: &'a Coin::Tx,
    /// Maker will be able to refund the payment after this timestamp
    pub time_lock: u64,
    /// The hash of the secret generated by taker, this is used for immediate refund
    pub taker_secret_hash: &'a [u8],
    /// The hash of the secret generated by maker, taker needs it to spend the payment
    pub maker_secret_hash: &'a [u8],
    /// The secret generated by maker, revealed when maker spends taker's payment
    pub maker_secret: [u8; 32],
    /// Maker's HTLC pubkey
    pub maker_pub: &'a Coin::Pubkey,
    /// Unique data of specific swap
    pub swap_unique_data: &'a [u8],
    pub amount: BigDecimal,
}

pub struct SpendNftMakerPaymentArgs<'a, Coin: ParseCoinAssocTypes + ParseNftAssocTypes + ?Sized> {
    /// Maker payment tx
    pub maker_payment_tx: &'a Coin::Tx,
    /// The hash of the secret generated by taker, this is used for immediate refund
    pub taker_secret_hash: &'a [u8],
    /// The hash of the secret generated by maker, taker needs it to spend the payment
    pub maker_secret_hash: &'a [u8],
    /// The secret generated by maker, revealed when maker spends taker's payment
    pub maker_secret: &'a [u8],
    /// Maker's HTLC pubkey
    pub maker_pub: &'a Coin::Pubkey,
    /// Unique data of specific swap
    pub swap_unique_data: &'a [u8],
    /// The type of smart contract that governs this NFT
    pub contract_type: &'a Coin::ContractType,
}

/// Operations specific to maker coin in [Trading Protocol Upgrade implementation](https://github.com/KomodoPlatform/komodo-defi-framework/issues/1895)
#[async_trait]
pub trait MakerCoinSwapOpsV2: ParseCoinAssocTypes + CommonSwapOpsV2 + Send + Sync + 'static {
    /// Generate and broadcast maker payment transaction
    async fn send_maker_payment_v2(&self, args: SendMakerPaymentArgs<'_, Self>) -> Result<Self::Tx, TransactionErr>;

    /// Validate maker payment transaction
    async fn validate_maker_payment_v2(&self, args: ValidateMakerPaymentArgs<'_, Self>) -> ValidatePaymentResult<()>;

    /// Refund maker payment transaction using timelock path
    async fn refund_maker_payment_v2_timelock(
        &self,
        args: RefundMakerPaymentTimelockArgs<'_>,
    ) -> Result<Self::Tx, TransactionErr>;

    /// Refund maker payment transaction using immediate refund path
    async fn refund_maker_payment_v2_secret(
        &self,
        args: RefundMakerPaymentSecretArgs<'_, Self>,
    ) -> Result<Self::Tx, TransactionErr>;

    /// Spend maker payment transaction
    async fn spend_maker_payment_v2(&self, args: SpendMakerPaymentArgs<'_, Self>) -> Result<Self::Tx, TransactionErr>;
}

#[async_trait]
pub trait MakerNftSwapOpsV2: ParseCoinAssocTypes + ParseNftAssocTypes + Send + Sync + 'static {
    async fn send_nft_maker_payment_v2(
        &self,
        args: SendNftMakerPaymentArgs<'_, Self>,
    ) -> Result<Self::Tx, TransactionErr>;

    /// Validate NFT maker payment transaction
    async fn validate_nft_maker_payment_v2(
        &self,
        args: ValidateNftMakerPaymentArgs<'_, Self>,
    ) -> ValidatePaymentResult<()>;

    /// Spend NFT maker payment transaction
    async fn spend_nft_maker_payment_v2(
        &self,
        args: SpendNftMakerPaymentArgs<'_, Self>,
    ) -> Result<Self::Tx, TransactionErr>;

    /// Refund NFT maker payment transaction using timelock path
    async fn refund_nft_maker_payment_v2_timelock(
        &self,
        args: RefundNftMakerPaymentArgs<'_, Self>,
    ) -> Result<Self::Tx, TransactionErr>;

    /// Refund NFT maker payment transaction using immediate refund path
    async fn refund_nft_maker_payment_v2_secret(
        &self,
        args: RefundNftMakerPaymentArgs<'_, Self>,
    ) -> Result<Self::Tx, TransactionErr>;
}

/// Enum representing errors that can occur while waiting for taker payment spend.
#[derive(Display, Debug, EnumFromStringify)]
pub enum FindPaymentSpendError {
    /// Timeout error variant, indicating that the wait for taker payment spend has timed out.
    #[display(fmt = "Timed out waiting for taker payment spend, wait_until {wait_until}, now {now}")]
    Timeout {
        /// The timestamp until which the wait was expected to complete.
        wait_until: u64,
        /// The current timestamp when the timeout occurred.
        now: u64,
    },
    /// Invalid input transaction error variant, containing additional information about the error.
    InvalidInputTx(String),
    #[from_stringify("TryFromSliceError")]
    Internal(String),
    #[from_stringify("ethabi::Error")]
    #[display(fmt = "ABI error: {_0}")]
    ABIError(String),
    InvalidData(String),
    Transport(String),
}

impl From<WaitForOutputSpendErr> for FindPaymentSpendError {
    fn from(err: WaitForOutputSpendErr) -> Self {
        match err {
            WaitForOutputSpendErr::Timeout { wait_until, now } => FindPaymentSpendError::Timeout { wait_until, now },
            WaitForOutputSpendErr::NoOutputWithIndex(index) => {
                FindPaymentSpendError::InvalidInputTx(format!("Tx doesn't have output with index {index}"))
            },
        }
    }
}

impl From<PrepareTxDataError> for FindPaymentSpendError {
    fn from(e: PrepareTxDataError) -> Self {
        match e {
            PrepareTxDataError::ABIError(e) => Self::ABIError(e),
            PrepareTxDataError::Internal(e) => Self::Internal(e),
            PrepareTxDataError::InvalidData(e) => Self::InvalidData(e),
        }
    }
}

/// Enum representing different ways a funding transaction can be spent.
///
/// This enum is generic over types that implement the `ParseCoinAssocTypes` trait.
pub enum FundingTxSpend<T: ParseCoinAssocTypes + ?Sized> {
    /// Variant indicating that the funding transaction has been spent through a timelock path.
    RefundedTimelock(T::Tx),
    /// Variant indicating that the funding transaction has been spent by revealing a taker's secret (immediate refund path).
    RefundedSecret {
        /// The spending transaction.
        tx: T::Tx,
        /// The taker's secret value revealed in the spending transaction.
        secret: [u8; 32],
    },
    /// Variant indicating that the funds from the funding transaction have been transferred
    /// to the taker's payment transaction.
    TransferredToTakerPayment(T::Tx),
}

impl<T: ParseCoinAssocTypes + ?Sized> fmt::Debug for FundingTxSpend<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FundingTxSpend::RefundedTimelock(tx) => {
                write!(f, "RefundedTimelock({tx:?})")
            },
            FundingTxSpend::RefundedSecret { tx, secret: _ } => {
                write!(f, "RefundedSecret {{ tx: {tx:?} }}")
            },
            FundingTxSpend::TransferredToTakerPayment(tx) => {
                write!(f, "TransferredToTakerPayment({tx:?})")
            },
        }
    }
}

/// Enum representing errors that can occur during the search for funding spend.
#[derive(Debug, EnumFromStringify)]
pub enum SearchForFundingSpendErr {
    /// Variant indicating an invalid input transaction error with additional information.
    InvalidInputTx(String),
    /// Variant indicating a failure to process the spending transaction with additional details.
    FailedToProcessSpendTx(String),
    /// Variant indicating a coin's RPC error with additional information.
    Rpc(String),
    /// Variant indicating an error during conversion of the `from_block` argument with associated `TryFromIntError`.
    FromBlockConversionErr(TryFromIntError),
    #[from_stringify("ethabi::Error")]
    Internal(String),
}

/// Operations specific to taker coin in [Trading Protocol Upgrade implementation](https://github.com/KomodoPlatform/komodo-defi-framework/issues/1895)
#[async_trait]
pub trait TakerCoinSwapOpsV2: ParseCoinAssocTypes + CommonSwapOpsV2 + Send + Sync + 'static {
    /// Generate and broadcast taker funding transaction that includes dex fee, maker premium and actual trading volume.
    /// Funding tx can be reclaimed immediately if maker back-outs (doesn't send maker payment)
    async fn send_taker_funding(&self, args: SendTakerFundingArgs<'_>) -> Result<Self::Tx, TransactionErr>;

    /// Validates taker funding transaction.
    async fn validate_taker_funding(&self, args: ValidateTakerFundingArgs<'_, Self>) -> ValidateSwapV2TxResult;

    /// Refunds taker funding transaction using time-locked path without secret reveal.
    async fn refund_taker_funding_timelock(&self, args: RefundTakerPaymentArgs<'_>)
        -> Result<Self::Tx, TransactionErr>;

    /// Reclaims taker funding transaction using immediate refund path with secret reveal.
    async fn refund_taker_funding_secret(
        &self,
        args: RefundFundingSecretArgs<'_, Self>,
    ) -> Result<Self::Tx, TransactionErr>;

    /// Looks for taker funding transaction spend and detects path used
    async fn search_for_taker_funding_spend(
        &self,
        tx: &Self::Tx,
        from_block: u64,
        secret_hash: &[u8],
    ) -> Result<Option<FundingTxSpend<Self>>, SearchForFundingSpendErr>;

    /// Generates and signs a preimage spending funding tx to the combined taker payment
    async fn gen_taker_funding_spend_preimage(
        &self,
        args: &GenTakerFundingSpendArgs<'_, Self>,
        swap_unique_data: &[u8],
    ) -> GenPreimageResult<Self>;

    /// Validates taker funding spend preimage generated and signed by maker
    async fn validate_taker_funding_spend_preimage(
        &self,
        gen_args: &GenTakerFundingSpendArgs<'_, Self>,
        preimage: &TxPreimageWithSig<Self>,
    ) -> ValidateTakerFundingSpendPreimageResult;

    /// Sign and send a spending funding tx to the combined taker payment
    async fn sign_and_send_taker_funding_spend(
        &self,
        preimage: &TxPreimageWithSig<Self>,
        args: &GenTakerFundingSpendArgs<'_, Self>,
        swap_unique_data: &[u8],
    ) -> Result<Self::Tx, TransactionErr>;

    /// Refunds taker payment transaction.
    async fn refund_combined_taker_payment(&self, args: RefundTakerPaymentArgs<'_>)
        -> Result<Self::Tx, TransactionErr>;

    /// A bool flag that allows skipping the generation and P2P message broadcasting of `TakerPaymentSpendPreimage` on the Taker side,
    /// as well as its reception and validation on the Maker side.
    /// This is typically used for coins that rely on smart contracts.
    fn skip_taker_payment_spend_preimage(&self) -> bool {
        false
    }

    /// Generates and signs taker payment spend preimage. The preimage and signature should be
    /// shared with maker to proceed with protocol execution.
    async fn gen_taker_payment_spend_preimage(
        &self,
        args: &GenTakerPaymentSpendArgs<'_, Self>,
        swap_unique_data: &[u8],
    ) -> GenPreimageResult<Self>;

    /// Validate taker payment spend preimage on maker's side.
    async fn validate_taker_payment_spend_preimage(
        &self,
        gen_args: &GenTakerPaymentSpendArgs<'_, Self>,
        preimage: &TxPreimageWithSig<Self>,
    ) -> ValidateTakerPaymentSpendPreimageResult;

    /// Sign and broadcast taker payment spend on maker's side.
    async fn sign_and_broadcast_taker_payment_spend(
        &self,
        preimage: Option<&TxPreimageWithSig<Self>>,
        gen_args: &GenTakerPaymentSpendArgs<'_, Self>,
        secret: &[u8],
        swap_unique_data: &[u8],
    ) -> Result<Self::Tx, TransactionErr>;

    /// Wait until taker payment spend transaction is found on-chain
    async fn find_taker_payment_spend_tx(
        &self,
        taker_payment: &Self::Tx,
        from_block: u64,
        wait_until: u64,
    ) -> MmResult<Self::Tx, FindPaymentSpendError>;

    async fn extract_secret_v2(&self, secret_hash: &[u8], spend_tx: &Self::Tx) -> Result<[u8; 32], String>;
}

#[async_trait]
pub trait CommonSwapOpsV2: ParseCoinAssocTypes + Send + Sync + 'static {
    /// Derives an HTLC key-pair and returns a public key corresponding to that key.
    fn derive_htlc_pubkey_v2(&self, swap_unique_data: &[u8]) -> Self::Pubkey;

    fn derive_htlc_pubkey_v2_bytes(&self, swap_unique_data: &[u8]) -> Vec<u8>;

    /// Returns taker pubkey for non-private coins, for dex fee calculation
    fn taker_pubkey_bytes(&self) -> Option<Vec<u8>>;
}

/// Operations that coins have independently from the MarketMaker.
/// That is, things implemented by the coin wallets or public coin services.
#[async_trait]
pub trait MarketCoinOps {
    fn ticker(&self) -> &str;

    fn my_address(&self) -> MmResult<String, MyAddressError>;

    fn address_from_pubkey(&self, pubkey: &H264Json) -> MmResult<String, AddressFromPubkeyError>;

    async fn get_public_key(&self) -> Result<String, MmError<UnexpectedDerivationMethod>>;

    fn sign_message_hash(&self, _message: &str) -> Option<[u8; 32]>;

    fn sign_message(&self, _message: &str, _address: Option<HDAddressSelector>) -> SignatureResult<String>;

    fn verify_message(&self, _signature: &str, _message: &str, _address: &str) -> VerificationResult<bool>;

    fn get_non_zero_balance(&self) -> NonZeroBalanceFut<MmNumber> {
        let closure = |spendable: BigDecimal| {
            if spendable.is_zero() {
                return MmError::err(GetNonZeroBalance::BalanceIsZero);
            }
            Ok(MmNumber::from(spendable))
        };

        Box::new(
            self.my_spendable_balance()
                .map_err(|e| e.map(GetNonZeroBalance::from))
                .and_then(closure),
        )
    }

    fn my_balance(&self) -> BalanceFut<CoinBalance>;

    fn my_spendable_balance(&self) -> BalanceFut<BigDecimal> {
        Box::new(self.my_balance().map(|CoinBalance { spendable, .. }| spendable))
    }

    /// Platform coin balance for tokens, e.g. ETH balance in ERC20 case
    fn platform_coin_balance(&self) -> BalanceFut<BigDecimal>;

    fn platform_ticker(&self) -> &str;

    /// Receives raw transaction bytes in hexadecimal format as input and returns tx hash in hexadecimal format
    fn send_raw_tx(&self, tx: &str) -> Box<dyn Future<Item = String, Error = String> + Send>;

    /// Receives raw transaction bytes as input and returns tx hash in hexadecimal format
    fn send_raw_tx_bytes(&self, tx: &[u8]) -> Box<dyn Future<Item = String, Error = String> + Send>;

    /// Signs raw utxo transaction in hexadecimal format as input and returns signed transaction in hexadecimal format
    async fn sign_raw_tx(&self, args: &SignRawTransactionRequest) -> RawTransactionResult;

    fn wait_for_confirmations(&self, input: ConfirmPaymentInput) -> Box<dyn Future<Item = (), Error = String> + Send>;

    /// Waits for spending/unlocking of funds locked in a HTLC construction specific to the coin's
    /// chain. Implementation should monitor locked funds (UTXO/contract/etc.) until funds are
    /// spent/unlocked or timeout is reached.
    ///
    /// Returns spending tx/event from mempool/pending state to allow prompt extraction of preimage
    /// secret.
    async fn wait_for_htlc_tx_spend(&self, args: WaitForHTLCTxSpendArgs<'_>) -> TransactionResult;

    fn tx_enum_from_bytes(&self, bytes: &[u8]) -> Result<TransactionEnum, MmError<TxMarshalingErr>>;

    fn current_block(&self) -> Box<dyn Future<Item = u64, Error = String> + Send>;

    fn display_priv_key(&self) -> Result<String, String>;

    /// Get the minimum amount to send.
    fn min_tx_amount(&self) -> BigDecimal;

    /// Get the minimum amount to trade.
    fn min_trading_vol(&self) -> MmNumber;

    /// Is privacy coin like zcash or pirate
    fn is_privacy(&self) -> bool {
        false
    }

    /// Returns `true` for coins (like KMD) that should use direct DEX fee burning via OP_RETURN.
    fn should_burn_directly(&self) -> bool {
        false
    }

    /// Should burn part of dex fee coin
    fn should_burn_dex_fee(&self) -> bool;

    fn is_trezor(&self) -> bool;
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum EthGasLimitOption {
    /// Use this value as gas limit
    Set(u64),
    /// Make MM2 calculate gas limit
    Calc,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum WithdrawFee {
    UtxoFixed {
        amount: BigDecimal,
    },
    UtxoPerKbyte {
        amount: BigDecimal,
    },
    EthGas {
        /// in gwei
        gas_price: BigDecimal,
        gas: u64,
    },
    EthGasEip1559 {
        /// in gwei
        max_priority_fee_per_gas: BigDecimal,
        max_fee_per_gas: BigDecimal,
        gas_option: EthGasLimitOption,
    },
    Qrc20Gas {
        /// in satoshi
        gas_limit: u64,
        gas_price: u64,
    },
    CosmosGas {
        gas_limit: u64,
        gas_price: f64,
    },
}

/// Rename to `GetWithdrawSenderAddresses` when withdraw supports multiple `from` addresses.
#[async_trait]
pub trait GetWithdrawSenderAddress {
    type Address;
    type Pubkey;

    async fn get_withdraw_sender_address(
        &self,
        req: &WithdrawRequest,
    ) -> MmResult<WithdrawSenderAddress<Self::Address, Self::Pubkey>, WithdrawError>;
}

/// TODO: Avoid using a single request structure on every platform.
/// Instead, accept a generic type from withdraw implementations.
/// This way we won't have to update the payload for every platform when
/// one of them requires specific addition.
#[derive(Clone, Default, Deserialize)]
pub struct WithdrawRequest {
    coin: String,
    from: Option<HDAddressSelector>,
    to: String,
    #[serde(default)]
    amount: BigDecimal,
    #[serde(default)]
    max: bool,
    fee: Option<WithdrawFee>,
    memo: Option<String>,
    /// Tendermint specific field used for manually providing the IBC channel IDs.
    ibc_source_channel: Option<ChannelId>,
    /// Currently, this flag is used by ETH/ERC20 coins activated with MetaMask/WalletConnect(Some wallets e.g Metamask) **only**.
    #[serde(default)]
    broadcast: bool,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum StakingDetails {
    Qtum(QtumDelegationRequest),
    Cosmos(Box<rpc_command::tendermint::staking::DelegationPayload>),
}

#[derive(Deserialize)]
pub struct AddDelegateRequest {
    pub coin: String,
    pub staking_details: StakingDetails,
}

#[derive(Deserialize)]
pub struct RemoveDelegateRequest {
    pub coin: String,
    pub staking_details: Option<StakingDetails>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ClaimingDetails {
    Cosmos(rpc_command::tendermint::staking::ClaimRewardsPayload),
}

#[derive(Deserialize)]
pub struct ClaimStakingRewardsRequest {
    pub coin: String,
    pub claiming_details: ClaimingDetails,
}

#[derive(Deserialize)]
pub struct DelegationsInfo {
    pub coin: String,
    info_details: DelegationsInfoDetails,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum DelegationsInfoDetails {
    Qtum,
    Cosmos(rpc_command::tendermint::staking::SimpleListQuery),
}

#[derive(Deserialize)]
pub struct UndelegationsInfo {
    pub coin: String,
    info_details: UndelegationsInfoDetails,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum UndelegationsInfoDetails {
    Cosmos(rpc_command::tendermint::staking::SimpleListQuery),
}

#[derive(Deserialize)]
pub struct ValidatorsInfo {
    pub coin: String,
    info_details: ValidatorsInfoDetails,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
pub enum ValidatorsInfoDetails {
    Cosmos(rpc_command::tendermint::staking::ValidatorsQuery),
}

#[derive(Deserialize)]
pub struct SignatureRequest {
    coin: String,
    message: String,
    address: Option<HDAddressSelector>,
}

#[derive(Serialize, Deserialize)]
pub struct VerificationRequest {
    coin: String,
    message: String,
    signature: String,
    address: String,
}

impl WithdrawRequest {
    pub fn new_max(coin: String, to: String) -> WithdrawRequest {
        WithdrawRequest {
            coin,
            to,
            max: true,
            ..Default::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum StakingInfosDetails {
    Qtum(QtumStakingInfosDetails),
}

impl From<QtumStakingInfosDetails> for StakingInfosDetails {
    fn from(qtum_staking_infos: QtumStakingInfosDetails) -> Self {
        StakingInfosDetails::Qtum(qtum_staking_infos)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct StakingInfos {
    pub staking_infos_details: StakingInfosDetails,
}

#[derive(Serialize)]
pub struct SignatureResponse {
    signature: String,
}

#[derive(Serialize)]
pub struct VerificationResponse {
    is_valid: bool,
}

/// Please note that no type should have the same structure as another type,
/// because this enum has the `untagged` deserialization.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type")]
pub enum TxFeeDetails {
    Utxo(UtxoFeeDetails),
    Eth(EthTxFeeDetails),
    Qrc20(Qrc20FeeDetails),
    Slp(SlpFeeDetails),
    Tendermint(TendermintFeeDetails),
}

/// Deserialize the TxFeeDetails as an untagged enum.
impl<'de> Deserialize<'de> for TxFeeDetails {
    fn deserialize<D>(deserializer: D) -> Result<Self, <D as Deserializer<'de>>::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum TxFeeDetailsUnTagged {
            Utxo(UtxoFeeDetails),
            Eth(EthTxFeeDetails),
            Qrc20(Qrc20FeeDetails),
            Tendermint(TendermintFeeDetails),
        }

        match Deserialize::deserialize(deserializer)? {
            TxFeeDetailsUnTagged::Utxo(f) => Ok(TxFeeDetails::Utxo(f)),
            TxFeeDetailsUnTagged::Eth(f) => Ok(TxFeeDetails::Eth(f)),
            TxFeeDetailsUnTagged::Qrc20(f) => Ok(TxFeeDetails::Qrc20(f)),
            TxFeeDetailsUnTagged::Tendermint(f) => Ok(TxFeeDetails::Tendermint(f)),
        }
    }
}

impl From<EthTxFeeDetails> for TxFeeDetails {
    fn from(eth_details: EthTxFeeDetails) -> Self {
        TxFeeDetails::Eth(eth_details)
    }
}

impl From<UtxoFeeDetails> for TxFeeDetails {
    fn from(utxo_details: UtxoFeeDetails) -> Self {
        TxFeeDetails::Utxo(utxo_details)
    }
}

impl From<Qrc20FeeDetails> for TxFeeDetails {
    fn from(qrc20_details: Qrc20FeeDetails) -> Self {
        TxFeeDetails::Qrc20(qrc20_details)
    }
}

impl From<TendermintFeeDetails> for TxFeeDetails {
    fn from(tendermint_details: TendermintFeeDetails) -> Self {
        TxFeeDetails::Tendermint(tendermint_details)
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct KmdRewardsDetails {
    amount: BigDecimal,
    claimed_by_me: bool,
}

impl KmdRewardsDetails {
    pub fn claimed_by_me(amount: BigDecimal) -> KmdRewardsDetails {
        KmdRewardsDetails {
            amount,
            claimed_by_me: true,
        }
    }
}

#[derive(Default, Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum TransactionType {
    StakingDelegation,
    RemoveDelegation,
    ClaimDelegationRewards,
    #[default]
    StandardTransfer,
    TokenTransfer(BytesJson),
    FeeForTokenTx,
    CustomTendermintMsg {
        msg_type: CustomTendermintMsgType,
        token_id: Option<BytesJson>,
    },
    NftTransfer,
    TendermintIBCTransfer {
        token_id: Option<BytesJson>,
    },
}

/// Transaction details
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct TransactionDetails {
    #[serde(flatten)]
    pub tx: TransactionData,
    /// Coins are sent from these addresses
    from: Vec<String>,
    /// Coins are sent to these addresses
    to: Vec<String>,
    /// Total tx amount
    total_amount: BigDecimal,
    /// The amount spent from "my" address
    spent_by_me: BigDecimal,
    /// The amount received by "my" address
    received_by_me: BigDecimal,
    /// Resulting "my" balance change
    my_balance_change: BigDecimal,
    /// Block height
    block_height: u64,
    /// Transaction timestamp
    timestamp: u64,
    /// Every coin can has specific fee details:
    /// In UTXO tx fee is paid with the coin itself (e.g. 1 BTC and 0.0001 BTC fee).
    /// But for ERC20 token transfer fee is paid with another coin: ETH, because it's ETH smart contract function call that requires gas to be burnt.
    fee_details: Option<TxFeeDetails>,
    /// The coin transaction belongs to
    coin: String,
    /// Internal MM2 id used for internal transaction identification, for some coins it might be equal to transaction hash
    internal_id: BytesJson,
    /// Amount of accrued rewards.
    #[serde(skip_serializing_if = "Option::is_none")]
    kmd_rewards: Option<KmdRewardsDetails>,
    /// Type of transactions, default is StandardTransfer
    #[serde(default)]
    transaction_type: TransactionType,
    memo: Option<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum TransactionData {
    Signed {
        /// Raw bytes of signed transaction, this should be sent as is to `send_raw_transaction_bytes` RPC to broadcast the transaction
        tx_hex: BytesJson,
        /// Transaction hash in hexadecimal format
        tx_hash: String,
    },
    /// This can contain entirely different data depending on the platform.
    /// TODO: Perhaps using generics would be more suitable here?
    Unsigned(Json),
}

impl TransactionData {
    pub fn new_signed(tx_hex: BytesJson, tx_hash: String) -> Self {
        Self::Signed { tx_hex, tx_hash }
    }

    pub fn new_unsigned(unsigned_tx_data: Json) -> Self {
        Self::Unsigned(unsigned_tx_data)
    }

    pub fn tx_hex(&self) -> Option<&BytesJson> {
        match self {
            TransactionData::Signed { tx_hex, .. } => Some(tx_hex),
            TransactionData::Unsigned(_) => None,
        }
    }

    pub fn tx_hash(&self) -> Option<&str> {
        match self {
            TransactionData::Signed { tx_hash, .. } => Some(tx_hash),
            TransactionData::Unsigned(_) => None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct BlockHeightAndTime {
    height: u64,
    timestamp: u64,
}

impl TransactionDetails {
    /// Whether the transaction details block height should be updated (when tx is confirmed)
    pub fn should_update_block_height(&self) -> bool {
        // checking for std::u64::MAX because there was integer overflow
        // in case of electrum returned -1 so there could be records with MAX confirmations
        self.block_height == 0 || self.block_height == u64::MAX
    }

    /// Whether the transaction timestamp should be updated (when tx is confirmed)
    pub fn should_update_timestamp(&self) -> bool {
        // checking for std::u64::MAX because there was integer overflow
        // in case of electrum returned -1 so there could be records with MAX confirmations
        self.timestamp == 0
    }

    pub fn should_update_kmd_rewards(&self) -> bool {
        self.coin == "KMD" && self.kmd_rewards.is_none()
    }

    pub fn firo_negative_fee(&self) -> bool {
        match &self.fee_details {
            Some(TxFeeDetails::Utxo(utxo)) => utxo.amount < 0.into() && self.coin == "FIRO",
            _ => false,
        }
    }

    pub fn should_update(&self) -> bool {
        self.should_update_block_height()
            || self.should_update_timestamp()
            || self.should_update_kmd_rewards()
            || self.firo_negative_fee()
    }
}

/// Transaction fee to pay for swap transactions (could be total for two transactions: taker fee and payment fee txns)
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TradeFee {
    pub coin: String,
    pub amount: MmNumber,
    pub paid_from_trading_vol: bool,
}

/// A type alias for a HashMap where the key is a String representing the coin/token ticker,
/// and the value is a `CoinBalance` struct representing the balance of that coin/token.
/// This is used to represent the balance of a wallet or account for multiple coins/tokens.
pub type CoinBalanceMap = HashMap<String, CoinBalance>;

impl BalanceObjectOps for CoinBalanceMap {
    fn new() -> Self {
        HashMap::new()
    }

    fn add(&mut self, other: Self) {
        for (ticker, balance) in other {
            let total_balance = self.entry(ticker).or_default();
            *total_balance += balance;
        }
    }

    fn get_total_for_ticker(&self, ticker: &str) -> Option<BigDecimal> {
        self.get(ticker).map(|b| b.get_total())
    }
}

#[derive(Clone, Debug, Default, PartialEq, PartialOrd, Serialize)]
pub struct CoinBalance {
    pub spendable: BigDecimal,
    pub unspendable: BigDecimal,
}

impl BalanceObjectOps for CoinBalance {
    fn new() -> Self {
        CoinBalance::default()
    }

    fn add(&mut self, other: Self) {
        *self += other;
    }

    fn get_total_for_ticker(&self, _ticker: &str) -> Option<BigDecimal> {
        Some(self.get_total())
    }
}

impl CoinBalance {
    pub fn new(spendable: BigDecimal) -> CoinBalance {
        CoinBalance {
            spendable,
            unspendable: BigDecimal::from(0),
        }
    }

    pub fn into_total(self) -> BigDecimal {
        self.spendable + self.unspendable
    }

    pub fn get_total(&self) -> BigDecimal {
        &self.spendable + &self.unspendable
    }
}

impl Add for CoinBalance {
    type Output = CoinBalance;

    fn add(self, rhs: Self) -> Self::Output {
        CoinBalance {
            spendable: self.spendable + rhs.spendable,
            unspendable: self.unspendable + rhs.unspendable,
        }
    }
}

impl AddAssign for CoinBalance {
    fn add_assign(&mut self, rhs: Self) {
        self.spendable += rhs.spendable;
        self.unspendable += rhs.unspendable;
    }
}

/// The approximation is needed to cover the dynamic miner fee changing during a swap.
/// Also used to indicate refund fee is needed for eth
/// Also used to indicate utxo fee correction is needed due to a possible change output
#[derive(Clone, Copy, Debug)]
pub enum FeeApproxStage {
    /// Do not increase the trade fee.
    WithoutApprox,
    /// Increase the trade fee slightly.
    StartSwap,
    /// Increase the trade fee slightly
    WatcherPreimage,
    /// Increase the trade fee significantly.
    OrderIssue,
    /// Increase the trade fee significantly (used to calculate max volume).
    OrderIssueMax,
    /// Increase the trade fee largely in the trade_preimage rpc.
    TradePreimage,
    /// Increase the trade fee in the trade_preimage rpc (used to calculate max volume for trade preimage).
    TradePreimageMax,
}

#[derive(Debug)]
pub enum TradePreimageValue {
    Exact(BigDecimal),
    UpperBound(BigDecimal),
}

/// Gas fee policy for EVM-like swap
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub enum SwapGasFeePolicy {
    /// Use legacy gas price (before the EIP-1559 priority gas fee policy was implemented)
    #[default]
    Legacy,
    /// Use low EIP-1559 gas fee priority
    Low,
    /// Use medium EIP-1559 gas fee priority
    Medium,
    /// Use high EIP-1559 gas fee priority
    High,
}

#[derive(Debug, Deserialize)]
pub struct GetSwapGasFeePolicyRequest {
    coin: String,
}

#[derive(Debug, Deserialize)]
pub struct SetSwapGasFeePolicyRequest {
    coin: String,
    #[serde(default)]
    swap_gas_fee_policy: SwapGasFeePolicy,
}

#[derive(Debug, Display, EnumFromStringify, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum SwapGasFeePolicyError {
    #[from_stringify("CoinFindError")]
    NoSuchCoin(String),
    #[display(fmt = "eip-1559 policy is not supported for coin {_0}")]
    NotSupported(String),
}

impl HttpStatusCode for SwapGasFeePolicyError {
    fn status_code(&self) -> StatusCode {
        match self {
            SwapGasFeePolicyError::NoSuchCoin(_) | SwapGasFeePolicyError::NotSupported(_) => StatusCode::BAD_REQUEST,
        }
    }
}

pub type SwapGasFeePolicyResult = Result<SwapGasFeePolicy, MmError<SwapGasFeePolicyError>>;

#[derive(Debug, Display, EnumFromStringify, PartialEq)]
pub enum TradePreimageError {
    #[display(fmt = "Not enough {coin} to preimage the trade: available {available}, required at least {required}")]
    NotSufficientBalance {
        coin: String,
        available: BigDecimal,
        required: BigDecimal,
    },
    #[display(fmt = "The amount {amount} less than minimum transaction amount {threshold}")]
    AmountIsTooSmall { amount: BigDecimal, threshold: BigDecimal },
    #[display(fmt = "Transport error: {_0}")]
    Transport(String),
    #[from_stringify("NumConversError", "UnexpectedDerivationMethod")]
    #[display(fmt = "Internal error: {_0}")]
    InternalError(String),
    #[display(fmt = "Nft Protocol is not supported yet!")]
    NftProtocolNotSupported,
    #[display(fmt = "No such coin {}", coin)]
    NoSuchCoin { coin: String },
}

impl TradePreimageError {
    /// Construct [`TradePreimageError`] from [`GenerateTxError`] using additional `coin` and `decimals`.
    pub fn from_generate_tx_error(
        gen_tx_err: GenerateTxError,
        coin: String,
        decimals: u8,
        is_upper_bound: bool,
    ) -> TradePreimageError {
        match gen_tx_err {
            GenerateTxError::EmptyUtxoSet { required } => {
                let required = big_decimal_from_sat_unsigned(required, decimals);
                TradePreimageError::NotSufficientBalance {
                    coin,
                    available: BigDecimal::from(0),
                    required,
                }
            },
            GenerateTxError::EmptyOutputs => TradePreimageError::InternalError(gen_tx_err.to_string()),
            GenerateTxError::OutputValueLessThanDust { value, dust } => {
                if is_upper_bound {
                    // If the preimage value is [`TradePreimageValue::UpperBound`], then we had to pass the account balance as the output value.
                    if value == 0 {
                        let required = big_decimal_from_sat_unsigned(dust, decimals);
                        TradePreimageError::NotSufficientBalance {
                            coin,
                            available: big_decimal_from_sat_unsigned(value, decimals),
                            required,
                        }
                    } else {
                        let error = format!(
                            "Output value {value} (equal to the account balance) less than dust {dust}. Probably, dust is not set or outdated"
                        );
                        TradePreimageError::InternalError(error)
                    }
                } else {
                    let amount = big_decimal_from_sat_unsigned(value, decimals);
                    let threshold = big_decimal_from_sat_unsigned(dust, decimals);
                    TradePreimageError::AmountIsTooSmall { amount, threshold }
                }
            },
            GenerateTxError::DeductFeeFromOutputFailed {
                output_value, required, ..
            } => {
                let available = big_decimal_from_sat_unsigned(output_value, decimals);
                let required = big_decimal_from_sat_unsigned(required, decimals);
                TradePreimageError::NotSufficientBalance {
                    coin,
                    available,
                    required,
                }
            },
            GenerateTxError::NotEnoughUtxos { sum_utxos, required } => {
                let available = big_decimal_from_sat_unsigned(sum_utxos, decimals);
                let required = big_decimal_from_sat_unsigned(required, decimals);
                TradePreimageError::NotSufficientBalance {
                    coin,
                    available,
                    required,
                }
            },
            GenerateTxError::Transport(e) => TradePreimageError::Transport(e),
            GenerateTxError::Internal(e) => TradePreimageError::InternalError(e),
        }
    }
}

impl From<CoinFindError> for TradePreimageError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => TradePreimageError::NoSuchCoin { coin },
        }
    }
}

/// The reason of unsuccessful conversion of two internal numbers, e.g. `u64` from `BigNumber`.
#[derive(Clone, Debug, Display)]
pub struct NumConversError(String);

impl From<ParseBigDecimalError> for NumConversError {
    fn from(e: ParseBigDecimalError) -> Self {
        NumConversError::new(e.to_string())
    }
}

impl NumConversError {
    pub fn new(description: String) -> NumConversError {
        NumConversError(description)
    }

    pub fn description(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Display, EnumFromStringify, PartialEq, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum BalanceError {
    #[display(fmt = "Transport: {_0}")]
    Transport(String),
    #[display(fmt = "Invalid response: {_0}")]
    InvalidResponse(String),
    UnexpectedDerivationMethod(UnexpectedDerivationMethod),
    #[display(fmt = "Wallet storage error: {_0}")]
    WalletStorageError(String),
    #[from_stringify("Bip32Error", "NumConversError", "ParseBigIntError")]
    #[display(fmt = "Internal: {_0}")]
    Internal(String),
    #[display(fmt = "No such coin {}", coin)]
    NoSuchCoin {
        coin: String,
    },
}

#[derive(Debug, PartialEq, Display)]
pub enum GetNonZeroBalance {
    #[display(fmt = "Internal error when retrieving balance")]
    MyBalanceError(BalanceError),
    #[display(fmt = "Balance is zero")]
    BalanceIsZero,
}

impl From<AddressDerivingError> for BalanceError {
    fn from(e: AddressDerivingError) -> Self {
        BalanceError::Internal(e.to_string())
    }
}

impl From<AccountUpdatingError> for BalanceError {
    fn from(e: AccountUpdatingError) -> Self {
        let error = e.to_string();
        match e {
            AccountUpdatingError::AddressLimitReached { .. } | AccountUpdatingError::InvalidBip44Chain(_) => {
                // Account updating is expected to be called after `address_id` and `chain` validation.
                BalanceError::Internal(format!("Unexpected internal error: {error}"))
            },
            AccountUpdatingError::WalletStorageError(_) => BalanceError::WalletStorageError(error),
        }
    }
}

impl From<BalanceError> for GetNonZeroBalance {
    fn from(e: BalanceError) -> Self {
        GetNonZeroBalance::MyBalanceError(e)
    }
}

impl From<UnexpectedDerivationMethod> for BalanceError {
    fn from(e: UnexpectedDerivationMethod) -> Self {
        BalanceError::UnexpectedDerivationMethod(e)
    }
}

impl From<CoinFindError> for BalanceError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => BalanceError::NoSuchCoin { coin },
        }
    }
}

#[derive(Debug, Deserialize, Display, EnumFromStringify, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum StakingInfoError {
    #[display(fmt = "No such coin {coin}")]
    NoSuchCoin { coin: String },
    #[from_stringify("UnexpectedDerivationMethod")]
    #[display(fmt = "Derivation method is not supported: {_0}")]
    UnexpectedDerivationMethod(String),
    #[display(fmt = "Invalid payload: {reason}")]
    InvalidPayload { reason: String },
    #[display(fmt = "Transport error: {_0}")]
    Transport(String),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl From<UtxoRpcError> for StakingInfoError {
    fn from(e: UtxoRpcError) -> Self {
        match e {
            UtxoRpcError::Transport(rpc) | UtxoRpcError::ResponseParseError(rpc) => {
                StakingInfoError::Transport(rpc.to_string())
            },
            UtxoRpcError::InvalidResponse(error) => StakingInfoError::Transport(error),
            UtxoRpcError::Internal(error) => StakingInfoError::Internal(error),
        }
    }
}

impl From<Qrc20AddressError> for StakingInfoError {
    fn from(e: Qrc20AddressError) -> Self {
        match e {
            Qrc20AddressError::UnexpectedDerivationMethod(e) => StakingInfoError::UnexpectedDerivationMethod(e),
            Qrc20AddressError::ScriptHashTypeNotSupported { script_hash_type } => {
                StakingInfoError::Internal(format!("Script hash type '{script_hash_type}' is not supported"))
            },
        }
    }
}

impl HttpStatusCode for StakingInfoError {
    fn status_code(&self) -> StatusCode {
        match self {
            StakingInfoError::NoSuchCoin { .. }
            | StakingInfoError::InvalidPayload { .. }
            | StakingInfoError::UnexpectedDerivationMethod(_) => StatusCode::BAD_REQUEST,
            StakingInfoError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            StakingInfoError::Transport(_) => StatusCode::BAD_GATEWAY,
        }
    }
}

impl From<CoinFindError> for StakingInfoError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => StakingInfoError::NoSuchCoin { coin },
        }
    }
}

#[derive(Debug, Deserialize, Display, EnumFromStringify, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum DelegationError {
    #[display(fmt = "Not enough {coin} to delegate: available {available}, required at least {required}")]
    NotSufficientBalance {
        coin: String,
        available: BigDecimal,
        required: BigDecimal,
    },
    #[display(fmt = "The amount {amount} is too small, required at least {threshold}")]
    AmountTooLow { amount: BigDecimal, threshold: BigDecimal },
    #[display(fmt = "Delegation not available for: {coin}")]
    CoinDoesntSupportDelegation { coin: String },
    #[display(fmt = "No such coin {coin}")]
    NoSuchCoin { coin: String },
    #[display(fmt = "Delegator '{delegator_addr}' does not have any delegation on validator '{validator_addr}'.")]
    CanNotUndelegate {
        delegator_addr: String,
        validator_addr: String,
    },
    #[display(fmt = "Max available amount to undelegate is '{available}' but '{requested}' was requested.")]
    TooMuchToUndelegate {
        available: BigDecimal,
        requested: BigDecimal,
    },
    #[display(
        fmt = "Fee ({fee}) exceeds reward ({reward}) which makes this unprofitable. Set 'force' to true in the request to bypass this check."
    )]
    UnprofitableReward { reward: BigDecimal, fee: BigDecimal },
    #[display(fmt = "There is no reward for {coin} to claim.")]
    NothingToClaim { coin: String },
    #[display(fmt = "{_0}")]
    CannotInteractWithSmartContract(String),
    #[from_stringify("ScriptHashTypeNotSupported")]
    #[display(fmt = "{_0}")]
    AddressError(String),
    #[display(fmt = "Already delegating to: {_0}")]
    AlreadyDelegating(String),
    #[display(fmt = "Delegation is not supported, reason: {reason}")]
    DelegationOpsNotSupported { reason: String },
    #[display(fmt = "Transport error: {_0}")]
    Transport(String),
    #[display(fmt = "Invalid payload: {reason}")]
    InvalidPayload { reason: String },
    #[from_stringify("MyAddressError")]
    #[display(fmt = "Internal error: {_0}")]
    InternalError(String),
}

impl From<UtxoRpcError> for DelegationError {
    fn from(e: UtxoRpcError) -> Self {
        match e {
            UtxoRpcError::Transport(transport) | UtxoRpcError::ResponseParseError(transport) => {
                DelegationError::Transport(transport.to_string())
            },
            UtxoRpcError::InvalidResponse(resp) => DelegationError::Transport(resp),
            UtxoRpcError::Internal(internal) => DelegationError::InternalError(internal),
        }
    }
}

impl From<StakingInfoError> for DelegationError {
    fn from(e: StakingInfoError) -> Self {
        match e {
            StakingInfoError::NoSuchCoin { coin } => DelegationError::NoSuchCoin { coin },
            StakingInfoError::Transport(e) => DelegationError::Transport(e),
            StakingInfoError::UnexpectedDerivationMethod(reason) => {
                DelegationError::DelegationOpsNotSupported { reason }
            },
            StakingInfoError::Internal(e) => DelegationError::InternalError(e),
            StakingInfoError::InvalidPayload { reason } => DelegationError::InvalidPayload { reason },
        }
    }
}

impl From<CoinFindError> for DelegationError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => DelegationError::NoSuchCoin { coin },
        }
    }
}

impl From<BalanceError> for DelegationError {
    fn from(e: BalanceError) -> Self {
        match e {
            BalanceError::Transport(error) | BalanceError::InvalidResponse(error) => DelegationError::Transport(error),
            BalanceError::UnexpectedDerivationMethod(e) => {
                DelegationError::DelegationOpsNotSupported { reason: e.to_string() }
            },
            e @ BalanceError::WalletStorageError(_) => DelegationError::InternalError(e.to_string()),
            BalanceError::Internal(internal) => DelegationError::InternalError(internal),
            BalanceError::NoSuchCoin { coin } => DelegationError::NoSuchCoin { coin },
        }
    }
}

impl From<UtxoSignWithKeyPairError> for DelegationError {
    fn from(e: UtxoSignWithKeyPairError) -> Self {
        let error = format!("Error signing: {e}");
        DelegationError::InternalError(error)
    }
}

impl From<PrivKeyPolicyNotAllowed> for DelegationError {
    fn from(e: PrivKeyPolicyNotAllowed) -> Self {
        DelegationError::DelegationOpsNotSupported { reason: e.to_string() }
    }
}

impl From<UnexpectedDerivationMethod> for DelegationError {
    fn from(e: UnexpectedDerivationMethod) -> Self {
        DelegationError::DelegationOpsNotSupported { reason: e.to_string() }
    }
}

impl HttpStatusCode for DelegationError {
    fn status_code(&self) -> StatusCode {
        match self {
            DelegationError::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            DelegationError::Transport(_) => StatusCode::BAD_GATEWAY,
            _ => StatusCode::BAD_REQUEST,
        }
    }
}

impl DelegationError {
    pub fn from_generate_tx_error(gen_tx_err: GenerateTxError, coin: String, decimals: u8) -> DelegationError {
        match gen_tx_err {
            GenerateTxError::EmptyUtxoSet { required } => {
                let required = big_decimal_from_sat_unsigned(required, decimals);
                DelegationError::NotSufficientBalance {
                    coin,
                    available: BigDecimal::from(0),
                    required,
                }
            },
            GenerateTxError::EmptyOutputs => DelegationError::InternalError(gen_tx_err.to_string()),
            GenerateTxError::OutputValueLessThanDust { value, dust } => {
                let amount = big_decimal_from_sat_unsigned(value, decimals);
                let threshold = big_decimal_from_sat_unsigned(dust, decimals);
                DelegationError::AmountTooLow { amount, threshold }
            },
            GenerateTxError::DeductFeeFromOutputFailed {
                output_value, required, ..
            } => {
                let available = big_decimal_from_sat_unsigned(output_value, decimals);
                let required = big_decimal_from_sat_unsigned(required, decimals);
                DelegationError::NotSufficientBalance {
                    coin,
                    available,
                    required,
                }
            },
            GenerateTxError::NotEnoughUtxos { sum_utxos, required } => {
                let available = big_decimal_from_sat_unsigned(sum_utxos, decimals);
                let required = big_decimal_from_sat_unsigned(required, decimals);
                DelegationError::NotSufficientBalance {
                    coin,
                    available,
                    required,
                }
            },
            GenerateTxError::Transport(e) => DelegationError::Transport(e),
            GenerateTxError::Internal(e) => DelegationError::InternalError(e),
        }
    }
}

#[derive(Clone, Debug, Display, EnumFromStringify, EnumFromTrait, PartialEq, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum WithdrawError {
    #[display(fmt = "'{coin}' coin doesn't support 'init_withdraw' yet. Consider using 'withdraw' request instead")]
    CoinDoesntSupportInitWithdraw {
        coin: String,
    },
    #[display(fmt = "Not enough {coin} to withdraw: available {available}, required at least {required}")]
    NotSufficientBalance {
        coin: String,
        available: BigDecimal,
        required: BigDecimal,
    },
    #[display(fmt = "Not enough {coin} to afford fee. Available {available}, required at least {required}")]
    NotSufficientPlatformBalanceForFee {
        coin: String,
        available: BigDecimal,
        required: BigDecimal,
    },
    #[display(fmt = "Balance is zero")]
    ZeroBalanceToWithdrawMax,
    #[display(fmt = "The amount {amount} is too small, required at least {threshold}")]
    AmountTooLow {
        amount: BigDecimal,
        threshold: BigDecimal,
    },
    #[display(fmt = "Invalid address: {_0}")]
    InvalidAddress(String),
    #[display(fmt = "Invalid fee policy: {_0}")]
    InvalidFeePolicy(String),
    #[display(fmt = "Invalid fee parameters: {reason}")]
    InvalidFee {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        details: Option<Json>,
    },
    #[display(fmt = "Invalid memo field: {_0}")]
    InvalidMemo(String),
    #[display(fmt = "No such coin {coin}")]
    NoSuchCoin {
        coin: String,
    },
    #[from_trait(WithTimeout::timeout)]
    #[display(fmt = "Withdraw timed out {_0:?}")]
    Timeout(Duration),
    #[display(fmt = "Request should contain a 'from' address/account")]
    FromAddressNotFound,
    #[display(fmt = "Unexpected 'from' address: {_0}")]
    UnexpectedFromAddress(String),
    #[display(fmt = "Unknown '{account_id}' account")]
    UnknownAccount {
        account_id: u32,
    },
    #[display(fmt = "RPC 'task' is awaiting '{expected}' user action")]
    UnexpectedUserAction {
        expected: String,
    },
    #[from_trait(WithHwRpcError::hw_rpc_error)]
    HwError(HwRpcError),
    #[cfg(target_arch = "wasm32")]
    BroadcastExpected(String),
    #[display(fmt = "Transport error: {_0}")]
    Transport(String),
    #[from_trait(WithInternal::internal)]
    #[from_stringify(
        "MyAddressError",
        "NumConversError",
        "UnexpectedDerivationMethod",
        "PrivKeyPolicyNotAllowed"
    )]
    #[display(fmt = "Internal error: {_0}")]
    InternalError(String),
    #[display(fmt = "Unsupported error: {_0}")]
    UnsupportedError(String),
    #[display(fmt = "{coin} coin doesn't support NFT withdrawing")]
    CoinDoesntSupportNftWithdraw {
        coin: String,
    },
    #[display(fmt = "Contract type {_0} doesnt support 'withdraw_nft' yet")]
    ContractTypeDoesntSupportNftWithdrawing(String),
    #[display(fmt = "Action not allowed for coin: {_0}")]
    ActionNotAllowed(String),
    GetNftInfoError(GetNftInfoError),
    #[display(
        fmt = "Not enough NFTs amount with token_address: {token_address} and token_id {token_id}. Available {available}, required {required}"
    )]
    NotEnoughNftsAmount {
        token_address: String,
        token_id: String,
        available: BigUint,
        required: BigUint,
    },
    #[display(fmt = "DB error {_0}")]
    DbError(String),
    #[display(fmt = "My address is {my_address}, while current Nft owner is {token_owner}")]
    MyAddressNotNftOwner {
        my_address: String,
        token_owner: String,
    },
    #[display(fmt = "Nft Protocol is not supported yet!")]
    NftProtocolNotSupported,
    #[display(fmt = "Chain id must be set for typed transaction for coin {coin}")]
    NoChainIdSet {
        coin: String,
    },
    #[display(fmt = "Signing error {_0}")]
    SigningError(String),
    #[display(fmt = "Transaction type not supported")]
    TxTypeNotSupported,
    #[display(fmt = "Tendermint IBC error: {_0}")]
    IBCError(tendermint::IBCError),
}

impl HttpStatusCode for WithdrawError {
    fn status_code(&self) -> StatusCode {
        match self {
            WithdrawError::NoSuchCoin { .. } => StatusCode::NOT_FOUND,
            WithdrawError::Timeout(_) => StatusCode::REQUEST_TIMEOUT,
            WithdrawError::CoinDoesntSupportInitWithdraw { .. }
            | WithdrawError::NotSufficientBalance { .. }
            | WithdrawError::NotSufficientPlatformBalanceForFee { .. }
            | WithdrawError::ZeroBalanceToWithdrawMax
            | WithdrawError::AmountTooLow { .. }
            | WithdrawError::InvalidAddress(_)
            | WithdrawError::InvalidFeePolicy(_)
            | WithdrawError::InvalidFee { .. }
            | WithdrawError::InvalidMemo(_)
            | WithdrawError::FromAddressNotFound
            | WithdrawError::UnexpectedFromAddress(_)
            | WithdrawError::UnknownAccount { .. }
            | WithdrawError::UnexpectedUserAction { .. }
            | WithdrawError::UnsupportedError(_)
            | WithdrawError::ActionNotAllowed(_)
            | WithdrawError::GetNftInfoError(_)
            | WithdrawError::ContractTypeDoesntSupportNftWithdrawing(_)
            | WithdrawError::CoinDoesntSupportNftWithdraw { .. }
            | WithdrawError::NotEnoughNftsAmount { .. }
            | WithdrawError::NoChainIdSet { .. }
            | WithdrawError::TxTypeNotSupported
            | WithdrawError::SigningError(_)
            | WithdrawError::IBCError(_)
            | WithdrawError::MyAddressNotNftOwner { .. } => StatusCode::BAD_REQUEST,
            WithdrawError::HwError(_) => StatusCode::GONE,
            #[cfg(target_arch = "wasm32")]
            WithdrawError::BroadcastExpected(_) => StatusCode::BAD_REQUEST,
            WithdrawError::InternalError(_) | WithdrawError::DbError(_) | WithdrawError::NftProtocolNotSupported => {
                StatusCode::INTERNAL_SERVER_ERROR
            },
            WithdrawError::Transport(_) => StatusCode::BAD_GATEWAY,
        }
    }
}

impl From<AddressDerivingError> for WithdrawError {
    fn from(e: AddressDerivingError) -> Self {
        match e {
            AddressDerivingError::InvalidBip44Chain { .. } | AddressDerivingError::Bip32Error(_) => {
                WithdrawError::UnexpectedFromAddress(e.to_string())
            },
            AddressDerivingError::Internal(internal) => WithdrawError::InternalError(internal),
        }
    }
}

impl From<BalanceError> for WithdrawError {
    fn from(e: BalanceError) -> Self {
        match e {
            BalanceError::Transport(error) | BalanceError::InvalidResponse(error) => WithdrawError::Transport(error),
            BalanceError::UnexpectedDerivationMethod(e) => WithdrawError::from(e),
            e @ BalanceError::WalletStorageError(_) => WithdrawError::InternalError(e.to_string()),
            BalanceError::Internal(internal) => WithdrawError::InternalError(internal),
            BalanceError::NoSuchCoin { coin } => WithdrawError::NoSuchCoin { coin },
        }
    }
}

impl From<CoinFindError> for WithdrawError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => WithdrawError::NoSuchCoin { coin },
        }
    }
}

impl From<HDWithdrawError> for WithdrawError {
    fn from(e: HDWithdrawError) -> Self {
        match e {
            HDWithdrawError::UnexpectedFromAddress(e) => WithdrawError::UnexpectedFromAddress(e),
            HDWithdrawError::UnknownAccount { account_id } => WithdrawError::UnknownAccount { account_id },
            HDWithdrawError::AddressDerivingError(e) => e.into(),
            HDWithdrawError::InternalError(e) => WithdrawError::InternalError(e),
        }
    }
}

impl From<UtxoSignWithKeyPairError> for WithdrawError {
    fn from(e: UtxoSignWithKeyPairError) -> Self {
        let error = format!("Error signing: {e}");
        WithdrawError::InternalError(error)
    }
}

impl From<TimeoutError> for WithdrawError {
    fn from(e: TimeoutError) -> Self {
        WithdrawError::Timeout(e.duration)
    }
}

impl From<GetValidEthWithdrawAddError> for WithdrawError {
    fn from(e: GetValidEthWithdrawAddError) -> Self {
        match e {
            GetValidEthWithdrawAddError::CoinDoesntSupportNftWithdraw { coin } => {
                WithdrawError::CoinDoesntSupportNftWithdraw { coin }
            },
            GetValidEthWithdrawAddError::InvalidAddress(e) => WithdrawError::InvalidAddress(e),
        }
    }
}

impl From<EthGasDetailsErr> for WithdrawError {
    fn from(e: EthGasDetailsErr) -> Self {
        match e {
            EthGasDetailsErr::InvalidFeePolicy(e) => WithdrawError::InvalidFeePolicy(e),
            EthGasDetailsErr::AmountTooLow { amount, threshold } => WithdrawError::AmountTooLow { amount, threshold },
            EthGasDetailsErr::GasFeeCapTooLow {
                provided_fee_cap,
                required_base_fee,
            } => {
                let reason = "Provided gas fee cap is less than the required network base fee.".to_string();
                let details = json!({
                    "provided_fee_cap_gwei": provided_fee_cap.to_string(),
                    "required_base_fee_gwei": required_base_fee.to_string()
                });
                WithdrawError::InvalidFee {
                    reason,
                    details: Some(details),
                }
            },
            EthGasDetailsErr::GasFeeCapBelowBaseFee => {
                let reason = "The provided 'max fee per gas' is too low for current network conditions.".to_string();
                WithdrawError::InvalidFee { reason, details: None }
            },
            EthGasDetailsErr::Internal(e) => WithdrawError::InternalError(e),
            EthGasDetailsErr::Transport(e) => WithdrawError::Transport(e),
            EthGasDetailsErr::NftProtocolNotSupported => WithdrawError::NftProtocolNotSupported,
            EthGasDetailsErr::NoSuchCoin { coin } => WithdrawError::NoSuchCoin { coin },
        }
    }
}

impl From<Bip32Error> for WithdrawError {
    fn from(e: Bip32Error) -> Self {
        let error = format!("Error deriving key: {e}");
        WithdrawError::UnexpectedFromAddress(error)
    }
}

impl WithdrawError {
    /// Construct [`WithdrawError`] from [`GenerateTxError`] using additional `coin` and `decimals`.
    pub fn from_generate_tx_error(gen_tx_err: GenerateTxError, coin: String, decimals: u8) -> WithdrawError {
        match gen_tx_err {
            GenerateTxError::EmptyUtxoSet { required } => {
                let required = big_decimal_from_sat_unsigned(required, decimals);
                WithdrawError::NotSufficientBalance {
                    coin,
                    available: BigDecimal::from(0),
                    required,
                }
            },
            GenerateTxError::EmptyOutputs => WithdrawError::InternalError(gen_tx_err.to_string()),
            GenerateTxError::OutputValueLessThanDust { value, dust } => {
                let amount = big_decimal_from_sat_unsigned(value, decimals);
                let threshold = big_decimal_from_sat_unsigned(dust, decimals);
                WithdrawError::AmountTooLow { amount, threshold }
            },
            GenerateTxError::DeductFeeFromOutputFailed {
                output_value, required, ..
            } => {
                let available = big_decimal_from_sat_unsigned(output_value, decimals);
                let required = big_decimal_from_sat_unsigned(required, decimals);
                WithdrawError::NotSufficientBalance {
                    coin,
                    available,
                    required,
                }
            },
            GenerateTxError::NotEnoughUtxos { sum_utxos, required } => {
                let available = big_decimal_from_sat_unsigned(sum_utxos, decimals);
                let required = big_decimal_from_sat_unsigned(required, decimals);
                WithdrawError::NotSufficientBalance {
                    coin,
                    available,
                    required,
                }
            },
            GenerateTxError::Transport(e) => WithdrawError::Transport(e),
            GenerateTxError::Internal(e) => WithdrawError::InternalError(e),
        }
    }
}

#[derive(Debug, Display, EnumFromStringify, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum SignatureError {
    #[display(fmt = "Invalid request: {_0}")]
    InvalidRequest(String),
    #[from_stringify("CoinFindError", "ethkey::Error", "keys::Error", "PrivKeyPolicyNotAllowed")]
    #[display(fmt = "Internal error: {_0}")]
    InternalError(String),
    #[display(fmt = "Coin is not found: {_0}")]
    CoinIsNotFound(String),
    #[display(fmt = "sign_message_prefix is not set in coin config")]
    PrefixNotFound,
}

impl HttpStatusCode for SignatureError {
    fn status_code(&self) -> StatusCode {
        match self {
            SignatureError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            SignatureError::CoinIsNotFound(_) => StatusCode::BAD_REQUEST,
            SignatureError::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            SignatureError::PrefixNotFound => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[derive(Debug, Display, EnumFromStringify, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum VerificationError {
    #[display(fmt = "Invalid request: {_0}")]
    InvalidRequest(String),
    #[from_stringify("ethkey::Error", "keys::Error")]
    #[display(fmt = "Internal error: {_0}")]
    InternalError(String),
    #[from_stringify("base64::DecodeError")]
    #[display(fmt = "Signature decoding error: {_0}")]
    SignatureDecodingError(String),
    #[from_stringify("hex::FromHexError")]
    #[display(fmt = "Address decoding error: {_0}")]
    AddressDecodingError(String),
    #[from_stringify("CoinFindError")]
    #[display(fmt = "Coin is not found: {_0}")]
    CoinIsNotFound(String),
    #[display(fmt = "sign_message_prefix is not set in coin config")]
    PrefixNotFound,
}

impl HttpStatusCode for VerificationError {
    fn status_code(&self) -> StatusCode {
        match self {
            VerificationError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            VerificationError::SignatureDecodingError(_) => StatusCode::BAD_REQUEST,
            VerificationError::AddressDecodingError(_) => StatusCode::BAD_REQUEST,
            VerificationError::CoinIsNotFound(_) => StatusCode::BAD_REQUEST,
            VerificationError::InternalError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            VerificationError::PrefixNotFound => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Display, PartialEq, Serialize)]
pub enum OrderCreationPreCheckError {
    #[display(fmt = "'{ticker}' is a wallet only asset and can't be used in orders.")]
    IsWalletOnly { ticker: String },
    #[display(fmt = "Pre-Check failed due to this reason: {reason}")]
    PreCheckFailed { reason: String },
    #[display(fmt = "Internal error: {reason}")]
    InternalError { reason: String },
}

/// NB: Implementations are expected to follow the pImpl idiom, providing cheap reference-counted cloning and garbage collection.
#[async_trait]
pub trait MmCoin: SwapOps + WatcherOps + MarketCoinOps + Send + Sync + 'static {
    // `MmCoin` is an extension fulcrum for something that doesn't fit the `MarketCoinOps`. Practical examples:
    // name (might be required for some APIs, CoinMarketCap for instance);
    // coin statistics that we might want to share with UI;
    // state serialization, to get full rewind and debugging information about the coins participating in a SWAP operation.
    // status/availability check: https://github.com/artemii235/SuperNET/issues/156#issuecomment-446501816

    fn is_asset_chain(&self) -> bool;

    /// The coin can be initialized, but it cannot participate in the swaps.
    fn wallet_only(&self, ctx: &MmArc) -> bool {
        let coin_conf = coin_conf(ctx, self.ticker());
        // If coin is not in config, it means that it was added manually (a custom token) and should be treated as wallet only
        if coin_conf.is_null() {
            return true;
        }
        coin_conf["wallet_only"].as_bool().unwrap_or(false)
    }

    /// Returns a spawner pinned to the coin.
    ///
    /// # Note
    ///
    /// `WeakSpawner` doesn't prevent the spawned futures from being aborted.
    fn spawner(&self) -> WeakSpawner;

    fn withdraw(&self, req: WithdrawRequest) -> WithdrawFut;

    fn get_raw_transaction(&self, req: RawTransactionRequest) -> RawTransactionFut<'_>;

    fn get_tx_hex_by_hash(&self, tx_hash: Vec<u8>) -> RawTransactionFut<'_>;

    /// Maximum number of digits after decimal point used to denominate integer coin units (satoshis, wei, etc.)
    fn decimals(&self) -> u8;

    /// Convert input address to the specified address format.
    fn convert_to_address(&self, from: &str, to_address_format: Json) -> Result<String, String>;

    fn validate_address(&self, address: &str) -> ValidateAddressResult;

    /// Loop collecting coin transaction history and saving it to local DB
    fn process_history_loop(&self, ctx: MmArc) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    /// Path to tx history file
    #[cfg(not(target_arch = "wasm32"))]
    fn tx_history_path(&self, ctx: &MmArc) -> PathBuf {
        let my_address = self.my_address().unwrap_or_default();
        // BCH cash address format has colon after prefix, e.g. bitcoincash:
        // Colon can't be used in file names on Windows so it should be escaped
        let my_address = my_address.replace(':', "_");
        ctx.dbdir()
            .join("TRANSACTIONS")
            .join(format!("{}_{}.json", self.ticker(), my_address))
    }

    /// Path to tx history migration file
    #[cfg(not(target_arch = "wasm32"))]
    fn tx_migration_path(&self, ctx: &MmArc) -> PathBuf {
        let my_address = self.my_address().unwrap_or_default();
        // BCH cash address format has colon after prefix, e.g. bitcoincash:
        // Colon can't be used in file names on Windows so it should be escaped
        let my_address = my_address.replace(':', "_");
        ctx.dbdir()
            .join("TRANSACTIONS")
            .join(format!("{}_{}_migration", self.ticker(), my_address))
    }

    /// Loads existing tx history from file, returns empty vector if file is not found
    /// Cleans the existing file if deserialization fails
    fn load_history_from_file(&self, ctx: &MmArc) -> TxHistoryFut<Vec<TransactionDetails>> {
        load_history_from_file_impl(self, ctx)
    }

    fn save_history_to_file(&self, ctx: &MmArc, history: Vec<TransactionDetails>) -> TxHistoryFut<()> {
        save_history_to_file_impl(self, ctx, history)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn get_tx_history_migration(&self, ctx: &MmArc) -> TxHistoryFut<u64> {
        get_tx_history_migration_impl(self, ctx)
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn update_migration_file(&self, ctx: &MmArc, migration_number: u64) -> TxHistoryFut<()> {
        update_migration_file_impl(self, ctx, migration_number)
    }

    /// Transaction history background sync status
    fn history_sync_status(&self) -> HistorySyncState;

    /// Get fee to be paid per 1 swap transaction
    fn get_trade_fee(&self) -> Box<dyn Future<Item = TradeFee, Error = String> + Send>;

    /// Get fee to be paid by sender per whole swap (including possible refund) using the sending value and check if the wallet has sufficient balance to pay the fee.
    async fn get_sender_trade_fee(
        &self,
        value: TradePreimageValue,
        stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee>;

    /// Get fee to be paid by receiver per whole swap and check if the wallet has sufficient balance to pay the fee.
    fn get_receiver_trade_fee(&self, stage: FeeApproxStage) -> TradePreimageFut<TradeFee>;

    /// Get transaction fee the Taker has to pay to send a `TakerFee` transaction and check if the wallet has sufficient balance to pay the fee.
    async fn get_fee_to_send_taker_fee(
        &self,
        dex_fee_amount: DexFee,
        stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee>;

    /// TODO: It's weird that we implement this function on this trait.
    ///
    /// Move this into the `SwapOps` trait when possible (this function requires `MmCoins`
    /// trait to be implemented, but it's currently not possible to do `SwapOps: MmCoins`
    /// as `MmCoins` is already `MmCoins: SwapOps`.
    async fn pre_check_for_order_creation(
        &self,
        ctx: &MmArc,
        rel_coin: &MmCoinEnum,
    ) -> MmResult<(), OrderCreationPreCheckError> {
        if self.wallet_only(ctx) {
            return MmError::err(OrderCreationPreCheckError::IsWalletOnly {
                ticker: self.ticker().to_owned(),
            });
        }

        if rel_coin.wallet_only(ctx) {
            return MmError::err(OrderCreationPreCheckError::IsWalletOnly {
                ticker: rel_coin.ticker().to_owned(),
            });
        }

        Ok(())
    }

    /// required transaction confirmations number to ensure double-spend safety
    fn required_confirmations(&self) -> u64;

    /// whether coin requires notarization to ensure double-spend safety
    fn requires_notarization(&self) -> bool;

    /// set required transaction confirmations number
    fn set_required_confirmations(&self, confirmations: u64);

    /// set requires notarization
    fn set_requires_notarization(&self, requires_nota: bool);

    /// Get swap contract address if the coin uses it in Atomic Swaps.
    fn swap_contract_address(&self) -> Option<BytesJson>;

    /// Get fallback swap contract address if the coin uses it in Atomic Swaps.
    fn fallback_swap_contract(&self) -> Option<BytesJson>;

    /// The minimum number of confirmations at which a transaction is considered mature.
    fn mature_confirmations(&self) -> Option<u32>;

    /// Get some of the coin protocol related info in serialized format for p2p messaging.
    fn coin_protocol_info(&self, amount_to_receive: Option<MmNumber>) -> Vec<u8>;

    /// Check if serialized coin protocol info is supported by current version.
    /// Can also be used to check if orders can be matched or not.
    fn is_coin_protocol_supported(
        &self,
        info: &Option<Vec<u8>>,
        amount_to_send: Option<MmNumber>,
        locktime: u64,
        is_maker: bool,
    ) -> bool;

    /// Abort all coin related futures on coin deactivation.
    fn on_disabled(&self) -> Result<(), AbortedError>;

    /// For Handling the removal/deactivation of token on platform coin deactivation.
    fn on_token_deactivated(&self, ticker: &str);
}

#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
pub enum MmCoinEnum {
    UtxoCoin(UtxoStandardCoin),
    QtumCoin(QtumCoin),
    Qrc20Coin(Qrc20Coin),
    EthCoin(EthCoin),
    ZCoin(ZCoin),
    Bch(BchCoin),
    SlpToken(SlpToken),
    Tendermint(TendermintCoin),
    TendermintToken(TendermintToken),
    #[cfg(not(target_arch = "wasm32"))]
    LightningCoin(LightningCoin),
    #[cfg(feature = "enable-sia")]
    SiaCoin(SiaCoin),
    #[cfg(feature = "enable-solana")]
    Solana(solana::SolanaCoin),
    #[cfg(any(test, feature = "for-tests"))]
    Test(TestCoin),
}

impl From<UtxoStandardCoin> for MmCoinEnum {
    fn from(c: UtxoStandardCoin) -> MmCoinEnum {
        MmCoinEnum::UtxoCoin(c)
    }
}

impl From<EthCoin> for MmCoinEnum {
    fn from(c: EthCoin) -> MmCoinEnum {
        MmCoinEnum::EthCoin(c)
    }
}

#[cfg(any(test, feature = "for-tests"))]
impl From<TestCoin> for MmCoinEnum {
    fn from(c: TestCoin) -> MmCoinEnum {
        MmCoinEnum::Test(c)
    }
}

impl From<QtumCoin> for MmCoinEnum {
    fn from(coin: QtumCoin) -> Self {
        MmCoinEnum::QtumCoin(coin)
    }
}

impl From<Qrc20Coin> for MmCoinEnum {
    fn from(c: Qrc20Coin) -> MmCoinEnum {
        MmCoinEnum::Qrc20Coin(c)
    }
}

impl From<BchCoin> for MmCoinEnum {
    fn from(c: BchCoin) -> MmCoinEnum {
        MmCoinEnum::Bch(c)
    }
}

impl From<SlpToken> for MmCoinEnum {
    fn from(c: SlpToken) -> MmCoinEnum {
        MmCoinEnum::SlpToken(c)
    }
}

impl From<TendermintCoin> for MmCoinEnum {
    fn from(c: TendermintCoin) -> Self {
        MmCoinEnum::Tendermint(c)
    }
}

impl From<TendermintToken> for MmCoinEnum {
    fn from(c: TendermintToken) -> Self {
        MmCoinEnum::TendermintToken(c)
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<LightningCoin> for MmCoinEnum {
    fn from(c: LightningCoin) -> MmCoinEnum {
        MmCoinEnum::LightningCoin(c)
    }
}

#[cfg(feature = "enable-solana")]
impl From<solana::SolanaCoin> for MmCoinEnum {
    fn from(c: solana::SolanaCoin) -> MmCoinEnum {
        MmCoinEnum::Solana(c)
    }
}

impl From<ZCoin> for MmCoinEnum {
    fn from(c: ZCoin) -> MmCoinEnum {
        MmCoinEnum::ZCoin(c)
    }
}

#[cfg(feature = "enable-sia")]
impl From<SiaCoin> for MmCoinEnum {
    fn from(c: SiaCoin) -> MmCoinEnum {
        MmCoinEnum::SiaCoin(c)
    }
}

// NB: When stable and groked by IDEs, `enum_dispatch` can be used instead of `Deref` to speed things up.
impl Deref for MmCoinEnum {
    type Target = dyn MmCoin;
    fn deref(&self) -> &dyn MmCoin {
        match self {
            MmCoinEnum::UtxoCoin(ref c) => c,
            MmCoinEnum::QtumCoin(ref c) => c,
            MmCoinEnum::Qrc20Coin(ref c) => c,
            MmCoinEnum::EthCoin(ref c) => c,
            MmCoinEnum::Bch(ref c) => c,
            MmCoinEnum::SlpToken(ref c) => c,
            MmCoinEnum::Tendermint(ref c) => c,
            MmCoinEnum::TendermintToken(ref c) => c,
            #[cfg(not(target_arch = "wasm32"))]
            MmCoinEnum::LightningCoin(ref c) => c,
            MmCoinEnum::ZCoin(ref c) => c,
            #[cfg(feature = "enable-sia")]
            MmCoinEnum::SiaCoin(ref c) => c,
            #[cfg(feature = "enable-solana")]
            MmCoinEnum::Solana(ref c) => c,
            #[cfg(any(test, feature = "for-tests"))]
            MmCoinEnum::Test(ref c) => c,
        }
    }
}

impl MmCoinEnum {
    pub fn is_utxo_in_native_mode(&self) -> bool {
        match self {
            MmCoinEnum::UtxoCoin(ref c) => c.as_ref().rpc_client.is_native(),
            MmCoinEnum::QtumCoin(ref c) => c.as_ref().rpc_client.is_native(),
            MmCoinEnum::Qrc20Coin(ref c) => c.as_ref().rpc_client.is_native(),
            MmCoinEnum::Bch(ref c) => c.as_ref().rpc_client.is_native(),
            MmCoinEnum::SlpToken(ref c) => c.as_ref().rpc_client.is_native(),
            #[cfg(not(target_arch = "wasm32"))]
            MmCoinEnum::ZCoin(ref c) => c.as_ref().rpc_client.is_native(),
            _ => false,
        }
    }

    pub fn is_eth(&self) -> bool {
        matches!(self, MmCoinEnum::EthCoin(_))
    }

    fn is_platform_coin(&self) -> bool {
        self.ticker() == self.platform_ticker()
    }

    /// Determines the secret hash algorithm for a coin, prioritizing specific algorithms for certain protocols.
    /// # Attention
    /// When adding new coins, update this function to specify their appropriate secret hash algorithm.
    /// Otherwise, the function will default to `SecretHashAlgo::DHASH160`, which may not be correct for the new coin.
    pub fn secret_hash_algo_v2(&self) -> SecretHashAlgo {
        match self {
            MmCoinEnum::Tendermint(_) | MmCoinEnum::TendermintToken(_) | MmCoinEnum::EthCoin(_) => {
                SecretHashAlgo::SHA256
            },
            #[cfg(not(target_arch = "wasm32"))]
            MmCoinEnum::LightningCoin(_) => SecretHashAlgo::SHA256,
            _ => SecretHashAlgo::DHASH160,
        }
    }
}

#[async_trait]
pub trait BalanceTradeFeeUpdatedHandler {
    async fn balance_updated(&self, coin: &MmCoinEnum, new_balance: &BigDecimal);
}

#[derive(Clone)]
pub struct MmCoinStruct {
    pub inner: MmCoinEnum,
    is_available: Arc<AtomicBool>,
}

impl MmCoinStruct {
    pub fn new(coin: MmCoinEnum) -> Self {
        Self {
            inner: coin,
            is_available: AtomicBool::new(true).into(),
        }
    }

    /// Gets the current state of the parent coin whether
    /// it's available for the external requests or not.
    ///
    /// Always `true` for child tokens.
    pub fn is_available(&self) -> bool {
        !self.inner.is_platform_coin() // Tokens are always active or disabled
            || self.is_available.load(AtomicOrdering::SeqCst)
    }

    /// Makes the coin disabled to the external requests.
    /// Useful for executing `disable_coin` on parent coins
    /// that have child tokens enabled.
    ///
    /// Ineffective for child tokens.
    pub fn update_is_available(&self, to: bool) {
        if !self.inner.is_platform_coin() {
            warn!(
                "`update_is_available` is ineffective for tokens. Current token: {}",
                self.inner.ticker()
            );
            return;
        }

        self.is_available.store(to, AtomicOrdering::SeqCst);
    }
}

/// Represents how to burn part of dex fee.
#[derive(Clone, Debug, PartialEq)]
pub enum DexFeeBurnDestination {
    /// Burn by sending to utxo opreturn output
    KmdOpReturn,
    /// Send non-kmd coins to a dedicated account to exchange for kmd coins and burn them
    PreBurnAccount,
}

/// Represents the different types of DEX fees.
#[derive(Clone, Debug, PartialEq)]
pub enum DexFee {
    /// No dex fee is taken (if taker is dex pubkey)
    NoFee,
    /// Standard dex fee which will be sent to the dex fee address
    Standard(MmNumber),
    /// Dex fee with the burn amount
    WithBurn {
        /// Amount to go to the dex fee address
        fee_amount: MmNumber,
        /// Amount to be burned
        burn_amount: MmNumber,
        /// This indicates how to burn the burn_amount
        burn_destination: DexFeeBurnDestination,
    },
}

impl DexFee {
    const DEX_FEE_SHARE: &'static str = "0.75";

    /// Recreates a `DexFee` from separate fields (usually stored in db).
    #[cfg(any(test, feature = "for-tests"))]
    pub fn create_from_fields(fee_amount: MmNumber, burn_amount: MmNumber, ticker: &str) -> DexFee {
        if fee_amount == MmNumber::default() && burn_amount == MmNumber::default() {
            return DexFee::NoFee;
        }
        if burn_amount > MmNumber::default() {
            let burn_destination = match ticker {
                "KMD" => DexFeeBurnDestination::KmdOpReturn,
                _ => DexFeeBurnDestination::PreBurnAccount,
            };
            DexFee::WithBurn {
                fee_amount,
                burn_amount,
                burn_destination,
            }
        } else {
            DexFee::Standard(fee_amount)
        }
    }

    /// Calculates DEX fee with known taker_pubkey (for some takers dexfee may be zero).
    pub fn new_with_taker_pubkey(
        taker_coin: &dyn MmCoin,
        maker_ticker: &str,
        trade_amount: &MmNumber,
        taker_pubkey: &[u8],
    ) -> DexFee {
        if !taker_coin.is_privacy() && taker_coin.burn_pubkey() == taker_pubkey {
            return DexFee::NoFee; // no dex fee if the taker is the burn pubkey
        }
        Self::new_from_taker_coin(taker_coin, maker_ticker, trade_amount)
    }

    /// Calculates DEX fee with a threshold based on min tx amount of the taker coin.
    /// With this fn we may calculate the max dex fee amount, when taker_pubkey is not known yet.
    pub fn new_from_taker_coin(taker_coin: &dyn MmCoin, maker_ticker: &str, trade_amount: &MmNumber) -> DexFee {
        // calc dex fee
        let rate = Self::dex_fee_rate(taker_coin.ticker(), maker_ticker);
        let dex_fee = trade_amount * &rate;
        let min_tx_amount = MmNumber::from(taker_coin.min_tx_amount());

        if taker_coin.should_burn_directly() {
            // use a special dex fee option for kmd
            return Self::calc_dex_fee_for_op_return(dex_fee, min_tx_amount);
        }
        if taker_coin.should_burn_dex_fee() {
            // send part of dex fee to the 'pre-burn' account
            return Self::calc_dex_fee_for_burn_account(dex_fee, min_tx_amount);
        }
        if dex_fee <= min_tx_amount {
            return DexFee::Standard(min_tx_amount);
        }
        DexFee::Standard(dex_fee)
    }

    /// Returns dex fee discount if KMD is traded
    pub fn dex_fee_rate(base: &str, rel: &str) -> MmNumber {
        #[cfg(any(feature = "for-tests", test))]
        let fee_discount_tickers: &[&str] = match std::env::var("MYCOIN_FEE_DISCOUNT") {
            Ok(_) => &["KMD", "MYCOIN"],
            Err(_) => &["KMD"],
        };

        #[cfg(not(any(feature = "for-tests", test)))]
        let fee_discount_tickers: &[&str] = &["KMD"];

        if fee_discount_tickers.contains(&base) || fee_discount_tickers.contains(&rel) {
            // 1/777 - 10%
            BigRational::new(9.into(), 7770.into()).into()
        } else {
            BigRational::new(1.into(), 777.into()).into()
        }
    }

    /// Drops the dex fee in KMD by 25%. This cut will be burned during the taker fee payment.
    ///
    /// Also the cut can be decreased if the new dex fee amount is less than the minimum transaction amount.
    fn calc_dex_fee_for_op_return(dex_fee: MmNumber, min_tx_amount: MmNumber) -> DexFee {
        if dex_fee <= min_tx_amount {
            return DexFee::Standard(min_tx_amount);
        }
        // Dex fee with 25% burn amount cut
        let new_fee = &dex_fee * &MmNumber::from(Self::DEX_FEE_SHARE);
        if new_fee >= min_tx_amount {
            // Use the max burn value, which is 25%.
            DexFee::WithBurn {
                fee_amount: new_fee.clone(),
                burn_amount: dex_fee - new_fee,
                burn_destination: DexFeeBurnDestination::KmdOpReturn,
            }
        } else {
            // Burn only the exceeding amount because fee after 25% cut is less than `min_tx_amount`.
            DexFee::WithBurn {
                fee_amount: min_tx_amount.clone(),
                burn_amount: dex_fee - min_tx_amount,
                burn_destination: DexFeeBurnDestination::KmdOpReturn,
            }
        }
    }

    /// Drops the dex fee in non-KMD by 25%. This cut will be sent to an output designated as 'burn account' during the taker fee payment
    /// (so it cannot be dust).
    ///
    /// The cut can be set to zero if any of resulting amounts is less than the minimum transaction amount.
    fn calc_dex_fee_for_burn_account(dex_fee: MmNumber, min_tx_amount: MmNumber) -> DexFee {
        if dex_fee <= min_tx_amount {
            return DexFee::Standard(min_tx_amount);
        }
        // Dex fee with 25% burn amount cut
        let new_fee = &dex_fee * &MmNumber::from(Self::DEX_FEE_SHARE);
        let burn_amount = &dex_fee - &new_fee;
        if new_fee >= min_tx_amount && burn_amount >= min_tx_amount {
            // Use the max burn value, which is 25%. Ensure burn_amount is not dust
            return DexFee::WithBurn {
                fee_amount: new_fee,
                burn_amount,
                burn_destination: DexFeeBurnDestination::PreBurnAccount,
            };
        }
        // If the new dex fee is dust set it to min_tx_amount and check the updated burn_amount is not dust.
        let burn_amount = &dex_fee - &min_tx_amount;
        if new_fee < min_tx_amount && burn_amount >= min_tx_amount {
            DexFee::WithBurn {
                fee_amount: min_tx_amount,
                burn_amount,
                burn_destination: DexFeeBurnDestination::PreBurnAccount,
            }
        } else {
            DexFee::Standard(dex_fee)
        }
    }

    /// Gets the fee amount associated with the dex fee.
    pub fn fee_amount(&self) -> MmNumber {
        match self {
            DexFee::NoFee => 0.into(),
            DexFee::Standard(t) => t.clone(),
            DexFee::WithBurn { fee_amount, .. } => fee_amount.clone(),
        }
    }

    /// Gets the burn amount associated with the dex fee, if applicable.
    pub fn burn_amount(&self) -> Option<MmNumber> {
        match self {
            DexFee::Standard(_) | DexFee::NoFee => None,
            DexFee::WithBurn { burn_amount, .. } => Some(burn_amount.clone()),
        }
    }

    /// Calculates the total spend amount, considering both the fee and burn amounts.
    pub fn total_spend_amount(&self) -> MmNumber {
        match self {
            DexFee::NoFee => 0.into(),
            DexFee::Standard(t) => t.clone(),
            DexFee::WithBurn {
                fee_amount,
                burn_amount,
                ..
            } => fee_amount + burn_amount,
        }
    }

    /// Converts the fee amount to micro-units based on the specified decimal places.
    pub fn fee_amount_as_u64(&self, decimals: u8) -> NumConversResult<u64> {
        let fee_amount = self.fee_amount();
        utxo::sat_from_big_decimal(&fee_amount.into(), decimals)
    }

    /// Converts the burn amount to micro-units, if applicable, based on the specified decimal places.
    pub fn burn_amount_as_u64(&self, decimals: u8) -> NumConversResult<Option<u64>> {
        if let Some(burn_amount) = self.burn_amount() {
            Ok(Some(utxo::sat_from_big_decimal(&burn_amount.into(), decimals)?))
        } else {
            Ok(None)
        }
    }
}

pub struct CoinsContext {
    /// A map from a currency ticker symbol to the corresponding coin.
    /// Similar to `LP_coins`.
    coins: AsyncMutex<HashMap<String, MmCoinStruct>>,
    balance_update_handlers: AsyncMutex<Vec<Box<dyn BalanceTradeFeeUpdatedHandler + Send + Sync>>>,
    account_balance_task_manager: AccountBalanceTaskManagerShared,
    create_account_manager: CreateAccountTaskManagerShared,
    get_new_address_manager: GetNewAddressTaskManagerShared,
    platform_coin_tokens: PaMutex<HashMap<String, HashSet<String>>>,
    scan_addresses_manager: ScanAddressesTaskManagerShared,
    withdraw_task_manager: WithdrawTaskManagerShared,
    #[cfg(target_arch = "wasm32")]
    tx_history_db: SharedDb<TxHistoryDb>,
    #[cfg(target_arch = "wasm32")]
    hd_wallet_db: SharedDb<HDWalletDb>,
}

#[derive(Debug)]
pub struct PlatformIsAlreadyActivatedErr {
    pub ticker: String,
}

impl CoinsContext {
    /// Obtains a reference to this crate context, creating it if necessary.
    pub fn from_ctx(ctx: &MmArc) -> Result<Arc<CoinsContext>, String> {
        Ok(try_s!(from_ctx(&ctx.coins_ctx, move || {
            Ok(CoinsContext {
                platform_coin_tokens: PaMutex::new(HashMap::new()),
                coins: AsyncMutex::new(HashMap::new()),
                balance_update_handlers: AsyncMutex::new(vec![]),
                account_balance_task_manager: AccountBalanceTaskManager::new_shared(ctx.event_stream_manager.clone()),
                create_account_manager: CreateAccountTaskManager::new_shared(ctx.event_stream_manager.clone()),
                get_new_address_manager: GetNewAddressTaskManager::new_shared(ctx.event_stream_manager.clone()),
                scan_addresses_manager: ScanAddressesTaskManager::new_shared(ctx.event_stream_manager.clone()),
                withdraw_task_manager: WithdrawTaskManager::new_shared(ctx.event_stream_manager.clone()),
                #[cfg(target_arch = "wasm32")]
                tx_history_db: ConstructibleDb::new(ctx).into_shared(),
                #[cfg(target_arch = "wasm32")]
                hd_wallet_db: ConstructibleDb::new_shared_db(ctx).into_shared(),
            })
        })))
    }

    pub async fn add_token(&self, coin: MmCoinEnum) -> Result<(), MmError<RegisterCoinError>> {
        let mut coins = self.coins.lock().await;
        if coins.contains_key(coin.ticker()) {
            return MmError::err(RegisterCoinError::CoinIsInitializedAlready {
                coin: coin.ticker().into(),
            });
        }

        let ticker = coin.ticker();

        let mut platform_coin_tokens = self.platform_coin_tokens.lock();
        // Here, we try to add a token to platform_coin_tokens if the token belongs to a platform coin.
        if let Some(platform) = platform_coin_tokens.get_mut(coin.platform_ticker()) {
            platform.insert(ticker.to_owned());
        }

        coins.insert(ticker.into(), MmCoinStruct::new(coin));

        Ok(())
    }

    /// Adds a Layer 2 coin that depends on a standalone platform.
    /// The process of adding l2 coins is identical to that of adding tokens.
    pub async fn add_l2(&self, coin: MmCoinEnum) -> Result<(), MmError<RegisterCoinError>> {
        self.add_token(coin).await
    }

    /// Adds a platform coin and its associated tokens to the CoinsContext.
    ///
    /// Registers a platform coin alongside its associated ERC-20 tokens and optionally a global NFT.
    /// Regular tokens are added to the context without overwriting existing entries, preserving any previously activated tokens.
    /// In contrast, the global NFT, if provided, replaces any previously stored NFT data for the platform, ensuring the NFT info is up-to-date.
    /// An error is returned if the platform coin is already activated within the context, enforcing a single active instance for each platform.
    pub async fn add_platform_with_tokens(
        &self,
        platform: MmCoinEnum,
        tokens: Vec<MmCoinEnum>,
        global_nft: Option<MmCoinEnum>,
    ) -> Result<(), MmError<PlatformIsAlreadyActivatedErr>> {
        let mut coins = self.coins.lock().await;
        let mut platform_coin_tokens = self.platform_coin_tokens.lock();

        let platform_ticker = platform.ticker().to_owned();

        if let Some(coin) = coins.get(&platform_ticker) {
            if coin.is_available() {
                return MmError::err(PlatformIsAlreadyActivatedErr {
                    ticker: platform.ticker().into(),
                });
            }

            coin.update_is_available(true);
        } else {
            coins.insert(platform_ticker.clone(), MmCoinStruct::new(platform));
        }

        // Tokens can't be activated without platform coin so we can safely insert them without checking prior existence
        let mut token_tickers = HashSet::with_capacity(tokens.len());
        // TODO
        // Handling for these case:
        // USDT was activated via enable RPC
        // We try to activate ETH coin and USDT token via enable_eth_with_tokens
        for token in tokens {
            token_tickers.insert(token.ticker().to_string());
            coins
                .entry(token.ticker().into())
                .or_insert_with(|| MmCoinStruct::new(token));
        }
        if let Some(nft) = global_nft {
            token_tickers.insert(nft.ticker().to_string());
            // For NFT overwrite existing data
            coins.insert(nft.ticker().into(), MmCoinStruct::new(nft));
        }

        platform_coin_tokens
            .entry(platform_ticker)
            .or_default()
            .extend(token_tickers);
        Ok(())
    }

    /// If `ticker` is a platform coin, returns tokens dependent on it.
    pub async fn get_dependent_tokens(&self, ticker: &str) -> HashSet<String> {
        let coins = self.platform_coin_tokens.lock();
        coins.get(ticker).cloned().unwrap_or_default()
    }

    pub async fn remove_coin(&self, coin: MmCoinEnum) {
        let ticker = coin.ticker();
        let platform_ticker = coin.platform_ticker();
        let mut coins_storage = self.coins.lock().await;
        let mut platform_tokens_storage = self.platform_coin_tokens.lock();

        // Check if ticker is a platform coin and remove from it platform's token list
        if ticker == platform_ticker {
            if let Some(tokens_to_remove) = platform_tokens_storage.remove(ticker) {
                tokens_to_remove.iter().for_each(|token| {
                    if let Some(token) = coins_storage.remove(token) {
                        // Abort all token related futures on token deactivation
                        token
                            .inner
                            .on_disabled()
                            .error_log_with_msg(&format!("Error aborting coin({ticker}) futures"));
                    }
                });
            };
        } else {
            if let Some(tokens) = platform_tokens_storage.get_mut(platform_ticker) {
                tokens.remove(ticker);
            }
            if let Some(platform_coin) = coins_storage.get(platform_ticker) {
                platform_coin.inner.on_token_deactivated(ticker);
            }
        };

        //  Remove coin from coin list
        coins_storage
            .remove(ticker)
            .ok_or(format!("{ticker} is disabled already"))
            .error_log();

        // Abort all coin related futures on coin deactivation
        coin.on_disabled()
            .error_log_with_msg(&format!("Error aborting coin({ticker}) futures"));
    }

    #[cfg(target_arch = "wasm32")]
    async fn tx_history_db(&self) -> TxHistoryResult<TxHistoryDbLocked<'_>> {
        self.tx_history_db.get_or_initialize().await.map_mm_err()
    }

    #[inline(always)]
    pub async fn lock_coins(&self) -> AsyncMutexGuard<'_, HashMap<String, MmCoinStruct>> {
        self.coins.lock().await
    }
}

/// This enum is used in coin activation requests.
/// TODO: should we use #[serde(tag = "type", content = "params")] for this PrivKeyActivationPolicy like for the Eth policy,
/// to have them identical in activation requests
#[derive(Clone, Debug, Deserialize, Serialize, Default)]
pub enum PrivKeyActivationPolicy {
    #[default]
    ContextPrivKey,
    Trezor,
    WalletConnect {
        session_topic: kdf_walletconnect::WcTopic,
    },
}

impl PrivKeyActivationPolicy {
    pub fn is_hw_policy(&self) -> bool {
        matches!(self, PrivKeyActivationPolicy::Trezor)
    }
}

/// Enum representing various private key management policies.
///
/// This enum defines the various ways in which private keys can be managed
/// or sourced within the system, whether it's from a local software-based HD Wallet,
/// a hardware device like Trezor, or even external sources like Metamask.
#[derive(Clone, Debug)]
pub enum PrivKeyPolicy<T> {
    /// The legacy private key policy.
    ///
    /// This policy corresponds to a one-to-one mapping of private keys to addresses.
    /// In this scheme, only a single key and corresponding address is activated per coin,
    /// without any hierarchical deterministic derivation.
    Iguana(T),
    /// The HD Wallet private key policy.
    ///
    /// This variant uses a BIP44 derivation path up to the coin level
    /// and contains the necessary information to manage and derive
    /// keys using an HD Wallet scheme.
    HDWallet {
        /// Derivation path up to coin.
        ///
        /// Represents the first two segments of the BIP44 derivation path: `purpose` and `coin_type`.
        /// A full BIP44 address is structured as:
        /// `m/purpose'/coin_type'/account'/change/address_index`.
        path_to_coin: HDPathToCoin,
        /// The key that's currently activated and in use for this HD Wallet policy.
        activated_key: T,
        /// Extended private key based on the secp256k1 elliptic curve cryptography scheme.
        bip39_secp_priv_key: ExtendedPrivateKey<secp256k1::SecretKey>,
    },
    /// The Trezor hardware wallet private key policy.
    ///
    /// Details about how the keys are managed with the Trezor device
    /// are abstracted away and are not directly managed by this policy.
    Trezor,
    /// The Metamask private key policy, specific to the WASM target architecture.
    ///
    /// This variant encapsulates details about how keys are managed when interfacing
    /// with the Metamask extension, especially within web-based contexts.
    #[cfg(target_arch = "wasm32")]
    Metamask(EthMetamaskPolicy),
    /// WalletConnect private key policy.
    ///
    /// This variant represents the key management details for connections
    /// established via WalletConnect. It includes both compressed and uncompressed
    /// public keys.
    /// - `public_key`: Compressed public key, represented as [H264].
    /// - `public_key_uncompressed`: Uncompressed public key, represented as [H520].
    /// - `session_topic`: WalletConnect session that was used to activate this coin.
    // TODO: We want to have different variants of WalletConnect policy for different coin types:
    //       - ETH uses the structure found here.
    //       - Tendermint doesn't use this variant all together. Tendermint generalizes one level on top of PrivKeyPolicy by having a different activation policy
    //         structure that is either Priv(PrivKeyPolicy) or Pubkey(PublicKey) and when activated via wallet connect it uses the Pubkey(PublicKey) variant.
    //       - UTXO coins on the otherhand need to keep a list of all the addresses activated in the wallet and not just a single account.
    //            - Note: We need to have a way to select which account and address are the active ones (WalletConnect just spams us with all the addresses in every account).
    WalletConnect {
        public_key: H264,
        public_key_uncompressed: H520,
        session_topic: kdf_walletconnect::WcTopic,
    },
}

#[cfg(target_arch = "wasm32")]
#[derive(Clone, Debug)]
pub struct EthMetamaskPolicy {
    pub(crate) public_key: EthH264,
    pub(crate) public_key_uncompressed: H520,
}

impl<T> From<T> for PrivKeyPolicy<T> {
    fn from(key_pair: T) -> Self {
        PrivKeyPolicy::Iguana(key_pair)
    }
}

impl<T> PrivKeyPolicy<T> {
    fn activated_key(&self) -> Option<&T> {
        match self {
            PrivKeyPolicy::Iguana(key_pair) => Some(key_pair),
            PrivKeyPolicy::HDWallet {
                activated_key: activated_key_pair,
                ..
            } => Some(activated_key_pair),
            PrivKeyPolicy::WalletConnect { .. } | PrivKeyPolicy::Trezor => None,
            #[cfg(target_arch = "wasm32")]
            PrivKeyPolicy::Metamask(_) => None,
        }
    }

    fn activated_key_or_err(&self) -> Result<&T, MmError<PrivKeyPolicyNotAllowed>> {
        self.activated_key().or_mm_err(|| {
            PrivKeyPolicyNotAllowed::UnsupportedMethod(
                "`activated_key_or_err` is supported only for `PrivKeyPolicy::KeyPair` or `PrivKeyPolicy::HDWallet`"
                    .to_string(),
            )
        })
    }

    fn bip39_secp_priv_key(&self) -> Option<&ExtendedPrivateKey<secp256k1::SecretKey>> {
        match self {
            PrivKeyPolicy::HDWallet {
                bip39_secp_priv_key, ..
            } => Some(bip39_secp_priv_key),
            PrivKeyPolicy::Iguana(_) | PrivKeyPolicy::Trezor | PrivKeyPolicy::WalletConnect { .. } => None,
            #[cfg(target_arch = "wasm32")]
            PrivKeyPolicy::Metamask(_) => None,
        }
    }

    fn bip39_secp_priv_key_or_err(
        &self,
    ) -> Result<&ExtendedPrivateKey<secp256k1::SecretKey>, MmError<PrivKeyPolicyNotAllowed>> {
        self.bip39_secp_priv_key().or_mm_err(|| {
            PrivKeyPolicyNotAllowed::UnsupportedMethod(
                "`bip39_secp_priv_key_or_err` is supported only for `PrivKeyPolicy::HDWallet`".to_string(),
            )
        })
    }

    fn path_to_coin(&self) -> Option<&HDPathToCoin> {
        match self {
            PrivKeyPolicy::HDWallet {
                path_to_coin: derivation_path,
                ..
            } => Some(derivation_path),
            PrivKeyPolicy::Iguana(_) | PrivKeyPolicy::Trezor | PrivKeyPolicy::WalletConnect { .. } => None,
            #[cfg(target_arch = "wasm32")]
            PrivKeyPolicy::Metamask(_) => None,
        }
    }

    // Todo: this can be removed after the HDWallet is fully implemented for all protocols
    fn path_to_coin_or_err(&self) -> Result<&HDPathToCoin, MmError<PrivKeyPolicyNotAllowed>> {
        self.path_to_coin().or_mm_err(|| {
            PrivKeyPolicyNotAllowed::UnsupportedMethod(
                "`derivation_path_or_err` is supported only for `PrivKeyPolicy::HDWallet`".to_string(),
            )
        })
    }

    fn hd_wallet_derived_priv_key_or_err(
        &self,
        derivation_path: &DerivationPath,
    ) -> Result<Secp256k1Secret, MmError<PrivKeyPolicyNotAllowed>> {
        let bip39_secp_priv_key = self.bip39_secp_priv_key_or_err()?;
        derive_secp256k1_secret(bip39_secp_priv_key.clone(), derivation_path)
            .mm_err(|e| PrivKeyPolicyNotAllowed::InternalError(e.to_string()))
    }

    fn is_trezor(&self) -> bool {
        matches!(self, PrivKeyPolicy::Trezor)
    }
}

/// 'CoinWithPrivKeyPolicy' trait is used to get the private key policy of a coin.
pub trait CoinWithPrivKeyPolicy {
    /// The type of the key pair used by the coin.
    type KeyPair;

    /// Returns the private key policy of the coin.
    fn priv_key_policy(&self) -> &PrivKeyPolicy<Self::KeyPair>;
}

/// A common function to get the extended public key for a certain coin and derivation path.
pub async fn extract_extended_pubkey_impl<Coin, XPubExtractor>(
    coin: &Coin,
    xpub_extractor: Option<XPubExtractor>,
    derivation_path: DerivationPath,
) -> MmResult<Secp256k1ExtendedPublicKey, HDExtractPubkeyError>
where
    XPubExtractor: HDXPubExtractor + Send,
    Coin: HDWalletCoinOps + CoinWithPrivKeyPolicy,
{
    match xpub_extractor {
        Some(xpub_extractor) => {
            let trezor_coin = coin.trezor_coin().map_mm_err()?;
            let xpub = xpub_extractor.extract_xpub(trezor_coin, derivation_path).await?;
            Secp256k1ExtendedPublicKey::from_str(&xpub).map_to_mm(|e| HDExtractPubkeyError::InvalidXpub(e.to_string()))
        },
        None => {
            let mut priv_key = coin
                .priv_key_policy()
                .bip39_secp_priv_key_or_err()
                .mm_err(|e| HDExtractPubkeyError::Internal(e.to_string()))?
                .clone();
            for child in derivation_path {
                priv_key = priv_key
                    .derive_child(child)
                    .map_to_mm(|e| HDExtractPubkeyError::Internal(e.to_string()))?;
            }
            drop_mutability!(priv_key);
            Ok(priv_key.public_key())
        },
    }
}

#[derive(Clone)]
pub enum PrivKeyBuildPolicy {
    IguanaPrivKey(IguanaPrivKey),
    GlobalHDAccount(GlobalHDAccountArc),
    Trezor,
    WalletConnect { session_topic: kdf_walletconnect::WcTopic },
}

impl PrivKeyBuildPolicy {
    /// Detects the `PrivKeyBuildPolicy` with which the given `MmArc` is initialized.
    pub fn detect_priv_key_policy(ctx: &MmArc) -> MmResult<PrivKeyBuildPolicy, CryptoCtxError> {
        let crypto_ctx = CryptoCtx::from_ctx(ctx)?;

        match crypto_ctx.key_pair_policy() {
            // Use an internal private key as the coin secret.
            KeyPairPolicy::Iguana => Ok(PrivKeyBuildPolicy::IguanaPrivKey(
                crypto_ctx.mm2_internal_privkey_secret(),
            )),
            KeyPairPolicy::GlobalHDAccount(global_hd) => Ok(PrivKeyBuildPolicy::GlobalHDAccount(global_hd.clone())),
        }
    }
}

/// Serializable struct for compatibility with the discontinued DerivationMethod struct
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "data")]
pub enum DerivationMethodResponse {
    /// Legacy iguana's privkey derivation, used by default
    Iguana,
    /// HD wallet derivation path, String is temporary here
    HDWallet(String),
}

/// Enum representing methods for deriving cryptographic addresses.
///
/// This enum distinguishes between two primary strategies for address generation:
/// 1. A static, single address approach.
/// 2. A hierarchical deterministic (HD) wallet that can derive multiple addresses.
#[derive(Debug)]
pub enum DerivationMethod<Address, HDWallet>
where
    HDWallet: HDWalletOps,
    HDWalletAddress<HDWallet>: Into<Address>,
{
    /// Represents the use of a single, static address for transactions and operations.
    SingleAddress(Address),
    /// Represents the use of an HD wallet for deriving multiple addresses.
    ///
    /// The encapsulated HD wallet should be capable of operations like
    /// getting the globally enabled address, and more, as defined by the
    /// [`HDWalletOps`] trait.
    HDWallet(HDWallet),
}

impl<Address, HDWallet> DerivationMethod<Address, HDWallet>
where
    Address: Clone,
    HDWallet: HDWalletOps,
    HDWalletAddress<HDWallet>: Into<Address>,
{
    pub async fn single_addr(&self) -> Option<Address> {
        match self {
            DerivationMethod::SingleAddress(my_address) => Some(my_address.clone()),
            DerivationMethod::HDWallet(hd_wallet) => {
                hd_wallet.get_enabled_address().await.map(|addr| addr.address().into())
            },
        }
    }

    pub async fn single_addr_or_err(&self) -> MmResult<Address, UnexpectedDerivationMethod> {
        self.single_addr()
            .await
            .or_mm_err(|| UnexpectedDerivationMethod::ExpectedSingleAddress)
    }

    pub fn hd_wallet(&self) -> Option<&HDWallet> {
        match self {
            DerivationMethod::SingleAddress(_) => None,
            DerivationMethod::HDWallet(hd_wallet) => Some(hd_wallet),
        }
    }

    pub fn hd_wallet_or_err(&self) -> MmResult<&HDWallet, UnexpectedDerivationMethod> {
        self.hd_wallet()
            .or_mm_err(|| UnexpectedDerivationMethod::ExpectedHDWallet)
    }

    /// # Panic
    ///
    /// Panic if the address mode is [`DerivationMethod::HDWallet`].
    pub async fn unwrap_single_addr(&self) -> Address {
        self.single_addr_or_err().await.unwrap()
    }

    pub async fn to_response(&self) -> MmResult<DerivationMethodResponse, UnexpectedDerivationMethod> {
        match self {
            DerivationMethod::SingleAddress(_) => Ok(DerivationMethodResponse::Iguana),
            DerivationMethod::HDWallet(hd_wallet) => {
                let enabled_address = hd_wallet
                    .get_enabled_address()
                    .await
                    .or_mm_err(|| UnexpectedDerivationMethod::ExpectedHDWallet)?;
                Ok(DerivationMethodResponse::HDWallet(
                    enabled_address.derivation_path().to_string(),
                ))
            },
        }
    }
}

/// A trait representing coins with specific address derivation methods.
///
/// This trait is designed for coins that have a defined mechanism for address derivation,
/// be it a single address approach or a hierarchical deterministic (HD) wallet strategy.
/// Coins implementing this trait should be clear about their chosen derivation method and
/// offer utility functions to interact with that method.
///
/// Implementors of this trait will typically be coins or tokens that are either used within
/// a traditional single address scheme or leverage the power and flexibility of HD wallets.
#[async_trait]
pub trait CoinWithDerivationMethod: HDWalletCoinOps {
    /// Returns the address derivation method associated with the coin.
    ///
    /// Implementors should return the specific `DerivationMethod` that the coin utilizes,
    /// either `SingleAddress` for a static address approach or `HDWallet` for an HD wallet strategy.
    fn derivation_method(&self) -> &DerivationMethod<HDCoinAddress<Self>, Self::HDWallet>;

    /// Checks if the coin uses the HD wallet strategy for address derivation.
    ///
    /// This is a utility function that returns `true` if the coin's derivation method is `HDWallet` and
    /// `false` otherwise.
    ///
    /// # Returns
    ///
    /// - `true` if the coin uses an HD wallet for address derivation.
    /// - `false` if it uses any other method.
    fn has_hd_wallet_derivation_method(&self) -> bool {
        matches!(self.derivation_method(), DerivationMethod::HDWallet(_))
    }

    /// Retrieves all addresses associated with the coin.
    async fn all_addresses(&self) -> MmResult<HashSet<HDCoinAddress<Self>>, AddressDerivingError> {
        const ADDRESSES_CAPACITY: usize = 60;

        match self.derivation_method() {
            DerivationMethod::SingleAddress(ref my_address) => Ok(iter::once(my_address.clone()).collect()),
            DerivationMethod::HDWallet(ref hd_wallet) => {
                let hd_accounts = hd_wallet.get_accounts().await;

                // We pre-allocate a suitable capacity for the HashSet to try to avoid re-allocations.
                // If the capacity is exceeded, the HashSet will automatically resize itself by re-allocating,
                // but this will not happen in most use cases where addresses will be below the capacity.
                let mut all_addresses = HashSet::with_capacity(ADDRESSES_CAPACITY);
                for (_, hd_account) in hd_accounts {
                    let external_addresses = self.derive_known_addresses(&hd_account, Bip44Chain::External).await?;
                    let internal_addresses = self.derive_known_addresses(&hd_account, Bip44Chain::Internal).await?;

                    let addresses_it = external_addresses
                        .into_iter()
                        .chain(internal_addresses)
                        .map(|hd_address| hd_address.address());
                    all_addresses.extend(addresses_it);
                }

                Ok(all_addresses)
            },
        }
    }
}

/// The `IguanaBalanceOps` trait provides an interface for fetching the balance of a coin and its tokens.
/// This trait should be implemented by coins that use the iguana derivation method.
#[async_trait]
pub trait IguanaBalanceOps {
    /// The object that holds the balance/s of the coin.
    type BalanceObject: BalanceObjectOps;

    /// Fetches the balance of the coin and its tokens if the coin uses an iguana derivation method.
    async fn iguana_balances(&self) -> BalanceResult<Self::BalanceObject>;
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
/// Information about the UTXO protocol used by a coin.
pub struct UtxoProtocolInfo {
    /// A CAIP-2 compliant chain ID. Starts with `b122:`
    /// This is used to identify the blockchain when using WalletConnect.
    /// https://github.com/ChainAgnostic/CAIPs/blob/main/CAIPs/caip-4.md
    chain_id: String,
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", content = "protocol_data")]
pub enum CoinProtocol {
    // TODO: Nest this option deep into the innert struct fields when more fields are added to the UTXO protocol info.
    UTXO(Option<UtxoProtocolInfo>),
    QTUM,
    QRC20 {
        platform: String,
        contract_address: String,
    },
    // Todo: Document this
    /// # Breaking Changes
    ETH {
        chain_id: u64,
    },
    ERC20 {
        platform: String,
        contract_address: String,
    },
    TRX {
        network: eth::tron::Network,
    },
    // Todo: Add TRC20, Do we need to support TRC10?
    SLPTOKEN {
        platform: String,
        token_id: H256Json,
        decimals: u8,
        required_confirmations: Option<u64>,
    },
    BCH {
        slp_prefix: String,
    },
    TENDERMINT(TendermintProtocolInfo),
    TENDERMINTTOKEN(TendermintTokenProtocolInfo),
    #[cfg(not(target_arch = "wasm32"))]
    LIGHTNING {
        platform: String,
        network: BlockchainNetwork,
        confirmation_targets: PlatformCoinConfirmationTargets,
    },
    ZHTLC(ZcoinProtocolInfo),
    #[cfg(feature = "enable-sia")]
    SIA,
    NFT {
        platform: String,
    },
}

#[derive(Clone, Debug, Deserialize, Display, PartialEq, Serialize)]
pub enum CustomTokenError {
    #[display(
        fmt = "Token with the same ticker already exists in coins configs, ticker in config: {ticker_in_config}"
    )]
    DuplicateTickerInConfig {
        ticker_in_config: String,
    },
    #[display(
        fmt = "Token with the same contract address already exists in coins configs, ticker in config: {ticker_in_config}"
    )]
    DuplicateContractInConfig {
        ticker_in_config: String,
    },
    #[display(fmt = "Token is already activated, ticker: {ticker}, contract address: {contract_address}")]
    TokenWithSameContractAlreadyActivated {
        ticker: String,
        contract_address: String,
    },
    InvalidTokenAddress,
}

impl CoinProtocol {
    /// Returns the platform coin associated with the coin protocol, if any.
    pub fn platform(&self) -> Option<&str> {
        match self {
            CoinProtocol::QRC20 { platform, .. }
            | CoinProtocol::ERC20 { platform, .. }
            | CoinProtocol::SLPTOKEN { platform, .. }
            | CoinProtocol::NFT { platform, .. } => Some(platform),
            CoinProtocol::TENDERMINTTOKEN(info) => Some(&info.platform),
            #[cfg(not(target_arch = "wasm32"))]
            CoinProtocol::LIGHTNING { platform, .. } => Some(platform),
            CoinProtocol::UTXO { .. }
            | CoinProtocol::QTUM
            | CoinProtocol::ETH { .. }
            | CoinProtocol::TRX { .. }
            | CoinProtocol::BCH { .. }
            | CoinProtocol::TENDERMINT(_)
            | CoinProtocol::ZHTLC(_) => None,
            #[cfg(feature = "enable-sia")]
            CoinProtocol::SIA => None,
        }
    }

    /// Returns the contract address associated with the coin, if any.
    pub fn contract_address(&self) -> Option<&str> {
        match self {
            CoinProtocol::QRC20 { contract_address, .. } | CoinProtocol::ERC20 { contract_address, .. } => {
                Some(contract_address)
            },
            CoinProtocol::SLPTOKEN { .. }
            | CoinProtocol::UTXO { .. }
            | CoinProtocol::QTUM
            | CoinProtocol::ETH { .. }
            | CoinProtocol::TRX { .. }
            | CoinProtocol::BCH { .. }
            | CoinProtocol::TENDERMINT(_)
            | CoinProtocol::TENDERMINTTOKEN(_)
            | CoinProtocol::ZHTLC(_)
            | CoinProtocol::NFT { .. } => None,
            #[cfg(not(target_arch = "wasm32"))]
            CoinProtocol::LIGHTNING { .. } => None,
            #[cfg(feature = "enable-sia")]
            CoinProtocol::SIA => None,
        }
    }

    /// Several checks to be preformed when a custom token is being activated to check uniqueness among other things.
    #[allow(clippy::result_large_err)]
    pub fn custom_token_validations(&self, ctx: &MmArc) -> MmResult<(), CustomTokenError> {
        let CoinProtocol::ERC20 {
            platform,
            contract_address,
        } = self
        else {
            return Ok(());
        };

        // Check if there is a token with the same contract address in the config.
        // If there is, return an error as the user should use this token instead of activating a custom one.
        // This is necessary as we will create an orderbook for this custom token using the contract address,
        // if it is duplicated in config, we will have two orderbooks one using the ticker and one using the contract address.
        // Todo: We should use the contract address for orderbook topics instead of the ticker once we make custom tokens non-wallet only.
        // If a coin is added to the config later, users who added it as a custom token and did not update will not see the orderbook.
        if let Some(existing_ticker) = get_erc20_ticker_by_contract_address(
            ctx,
            platform,
            &EthAddress::from_str(contract_address).map_err(|_| MmError::new(CustomTokenError::InvalidTokenAddress))?,
        ) {
            return Err(MmError::new(CustomTokenError::DuplicateContractInConfig {
                ticker_in_config: existing_ticker,
            }));
        }

        Ok(())
    }
}

/// Common methods to handle the connection events.
///
/// Note that the handler methods are sync and shouldn't take long time executing, otherwise it will hurt the performance.
/// If a handler needs to do some heavy work, it should be spawned/done in a separate thread.
pub trait RpcTransportEventHandler {
    fn debug_info(&self) -> String;

    fn on_outgoing_request(&self, data: &[u8]);

    fn on_incoming_response(&self, data: &[u8]);

    fn on_connected(&self, address: &str) -> Result<(), String>;

    fn on_disconnected(&self, address: &str) -> Result<(), String>;
}

pub type SharableRpcTransportEventHandler = dyn RpcTransportEventHandler + Send + Sync;
pub type RpcTransportEventHandlerShared = Arc<SharableRpcTransportEventHandler>;

impl fmt::Debug for SharableRpcTransportEventHandler {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.debug_info())
    }
}

impl RpcTransportEventHandler for RpcTransportEventHandlerShared {
    fn debug_info(&self) -> String {
        self.deref().debug_info()
    }

    fn on_outgoing_request(&self, data: &[u8]) {
        self.as_ref().on_outgoing_request(data)
    }

    fn on_incoming_response(&self, data: &[u8]) {
        self.as_ref().on_incoming_response(data)
    }

    fn on_connected(&self, address: &str) -> Result<(), String> {
        self.as_ref().on_connected(address)
    }

    fn on_disconnected(&self, address: &str) -> Result<(), String> {
        self.as_ref().on_disconnected(address)
    }
}

impl RpcTransportEventHandler for Box<SharableRpcTransportEventHandler> {
    fn debug_info(&self) -> String {
        self.as_ref().debug_info()
    }

    fn on_outgoing_request(&self, data: &[u8]) {
        self.as_ref().on_outgoing_request(data)
    }

    fn on_incoming_response(&self, data: &[u8]) {
        self.as_ref().on_incoming_response(data)
    }

    fn on_connected(&self, address: &str) -> Result<(), String> {
        self.as_ref().on_connected(address)
    }

    fn on_disconnected(&self, address: &str) -> Result<(), String> {
        self.as_ref().on_disconnected(address)
    }
}

impl<T: RpcTransportEventHandler> RpcTransportEventHandler for Vec<T> {
    fn debug_info(&self) -> String {
        let selfi: Vec<String> = self.iter().map(|x| x.debug_info()).collect();
        format!("{selfi:?}")
    }

    fn on_outgoing_request(&self, data: &[u8]) {
        for handler in self {
            handler.on_outgoing_request(data)
        }
    }

    fn on_incoming_response(&self, data: &[u8]) {
        for handler in self {
            handler.on_incoming_response(data)
        }
    }

    fn on_connected(&self, address: &str) -> Result<(), String> {
        let mut errors = vec![];
        for handler in self {
            if let Err(e) = handler.on_connected(address) {
                errors.push((handler.debug_info(), e))
            }
        }
        if !errors.is_empty() {
            return Err(format!("Errors: {errors:?}"));
        }
        Ok(())
    }

    fn on_disconnected(&self, address: &str) -> Result<(), String> {
        let mut errors = vec![];
        for handler in self {
            if let Err(e) = handler.on_disconnected(address) {
                errors.push((handler.debug_info(), e))
            }
        }
        if !errors.is_empty() {
            return Err(format!("Errors: {errors:?}"));
        }
        Ok(())
    }
}

pub enum RpcClientType {
    Native,
    Electrum,
    Ethereum,
}

impl std::fmt::Display for RpcClientType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RpcClientType::Native => write!(f, "native"),
            RpcClientType::Electrum => write!(f, "electrum"),
            RpcClientType::Ethereum => write!(f, "ethereum"),
        }
    }
}

#[derive(Clone)]
pub struct CoinTransportMetrics {
    /// Using a weak reference by default in order to avoid circular references and leaks.
    metrics: MetricsWeak,
    /// Name of coin the rpc client is intended to work with.
    ticker: String,
    /// RPC client type.
    client: String,
}

impl CoinTransportMetrics {
    fn new(metrics: MetricsWeak, ticker: String, client: RpcClientType) -> CoinTransportMetrics {
        CoinTransportMetrics {
            metrics,
            ticker,
            client: client.to_string(),
        }
    }

    fn into_shared(self) -> RpcTransportEventHandlerShared {
        Arc::new(self)
    }
}

impl RpcTransportEventHandler for CoinTransportMetrics {
    fn debug_info(&self) -> String {
        "CoinTransportMetrics".into()
    }

    fn on_outgoing_request(&self, data: &[u8]) {
        mm_counter!(self.metrics, "rpc_client.traffic.out", data.len() as u64,
            "coin" => self.ticker.to_owned(), "client" => self.client.to_owned());
        mm_counter!(self.metrics, "rpc_client.request.count", 1,
            "coin" => self.ticker.to_owned(), "client" => self.client.to_owned());
    }

    fn on_incoming_response(&self, data: &[u8]) {
        mm_counter!(self.metrics, "rpc_client.traffic.in", data.len() as u64,
            "coin" => self.ticker.to_owned(), "client" => self.client.to_owned());
        mm_counter!(self.metrics, "rpc_client.response.count", 1,
            "coin" => self.ticker.to_owned(), "client" => self.client.to_owned());
    }

    fn on_connected(&self, _address: &str) -> Result<(), String> {
        Ok(())
    }

    fn on_disconnected(&self, _address: &str) -> Result<(), String> {
        Ok(())
    }
}

#[async_trait]
impl BalanceTradeFeeUpdatedHandler for CoinsContext {
    async fn balance_updated(&self, coin: &MmCoinEnum, new_balance: &BigDecimal) {
        for sub in self.balance_update_handlers.lock().await.iter() {
            sub.balance_updated(coin, new_balance).await
        }
    }
}

pub fn coin_conf(ctx: &MmArc, ticker: &str) -> Json {
    match ctx.conf["coins"].as_array() {
        Some(coins) => coins
            .iter()
            .find(|coin| coin["coin"].as_str() == Some(ticker))
            .cloned()
            .unwrap_or(Json::Null),
        None => Json::Null,
    }
}

pub fn is_wallet_only_conf(conf: &Json) -> bool {
    // If coin is not in config, it means that it was added manually (a custom token) and should be treated as wallet only
    if conf.is_null() {
        return true;
    }
    conf["wallet_only"].as_bool().unwrap_or(false)
}

pub fn is_wallet_only_ticker(ctx: &MmArc, ticker: &str) -> bool {
    let coin_conf = coin_conf(ctx, ticker);
    // If coin is not in config, it means that it was added manually (a custom token) and should be treated as wallet only
    if coin_conf.is_null() {
        return true;
    }
    coin_conf["wallet_only"].as_bool().unwrap_or(false)
}

/// Adds a new currency into the list of currencies configured.
///
/// Returns an error if the currency already exists. Initializing the same currency twice is a bad habit
/// (might lead to misleading and confusing information during debugging and maintenance, see DRY)
/// and should be fixed on the call site.
///
/// * `req` - Payload of the corresponding "enable" or "electrum" RPC request.
pub async fn lp_coininit(ctx: &MmArc, ticker: &str, req: &Json) -> Result<MmCoinEnum, String> {
    let cctx = try_s!(CoinsContext::from_ctx(ctx));
    {
        let coins = cctx.coins.lock().await;
        if coins.get(ticker).is_some() {
            return ERR!("Coin {} already initialized", ticker);
        }
    }

    let coins_en = coin_conf(ctx, ticker);

    coins_conf_check(ctx, &coins_en, ticker, Some(req))?;

    // The legacy electrum/enable RPCs don't support Hardware Wallet policy.
    let priv_key_policy = try_s!(PrivKeyBuildPolicy::detect_priv_key_policy(ctx));

    let protocol: CoinProtocol = try_s!(json::from_value(coins_en["protocol"].clone()));

    let coin: MmCoinEnum = match &protocol {
        CoinProtocol::UTXO { .. } => {
            let params = try_s!(UtxoActivationParams::from_legacy_req(req));
            try_s!(utxo_standard_coin_with_policy(ctx, ticker, &coins_en, &params, priv_key_policy).await).into()
        },
        CoinProtocol::QTUM => {
            let params = try_s!(UtxoActivationParams::from_legacy_req(req));
            try_s!(qtum_coin_with_policy(ctx, ticker, &coins_en, &params, priv_key_policy).await).into()
        },
        CoinProtocol::ETH { .. } | CoinProtocol::ERC20 { .. } => {
            try_s!(eth_coin_from_conf_and_request(ctx, ticker, &coins_en, req, protocol, priv_key_policy).await).into()
        },
        CoinProtocol::QRC20 {
            platform,
            contract_address,
        } => {
            let params = try_s!(Qrc20ActivationParams::from_legacy_req(req));
            let contract_address = try_s!(qtum::contract_addr_from_str(contract_address));

            try_s!(
                qrc20_coin_with_policy(
                    ctx,
                    ticker,
                    platform,
                    &coins_en,
                    &params,
                    priv_key_policy,
                    contract_address
                )
                .await
            )
            .into()
        },
        CoinProtocol::BCH { slp_prefix } => {
            let prefix = try_s!(CashAddrPrefix::from_str(slp_prefix));
            let params = try_s!(BchActivationRequest::from_legacy_req(req));

            let bch = try_s!(bch_coin_with_policy(ctx, ticker, &coins_en, params, prefix, priv_key_policy).await);
            bch.into()
        },
        CoinProtocol::SLPTOKEN {
            platform,
            token_id,
            decimals,
            required_confirmations,
        } => {
            let platform_coin = try_s!(lp_coinfind(ctx, platform).await);
            let platform_coin = match platform_coin {
                Some(MmCoinEnum::Bch(coin)) => coin,
                Some(_) => return ERR!("Platform coin {} is not BCH", platform),
                None => return ERR!("Platform coin {} is not activated", platform),
            };

            let confs = required_confirmations.unwrap_or(platform_coin.required_confirmations());
            let token = try_s!(SlpToken::new(
                *decimals,
                ticker.into(),
                (*token_id).into(),
                platform_coin,
                confs
            ));
            token.into()
        },
        CoinProtocol::TENDERMINT { .. } => return ERR!("TENDERMINT protocol is not supported by lp_coininit"),
        CoinProtocol::TENDERMINTTOKEN(_) => return ERR!("TENDERMINTTOKEN protocol is not supported by lp_coininit"),
        CoinProtocol::ZHTLC { .. } => return ERR!("ZHTLC protocol is not supported by lp_coininit"),
        CoinProtocol::NFT { .. } => return ERR!("NFT protocol is not supported by lp_coininit"),
        CoinProtocol::TRX { .. } => return ERR!("TRX protocol is not supported by lp_coininit"),
        #[cfg(not(target_arch = "wasm32"))]
        CoinProtocol::LIGHTNING { .. } => return ERR!("Lightning protocol is not supported by lp_coininit"),
        #[cfg(feature = "enable-sia")]
        CoinProtocol::SIA => {
            return ERR!("SIA protocol is not supported by lp_coininit. Use task::enable_sia::init");
        },
    };

    let register_params = RegisterCoinParams {
        ticker: ticker.to_owned(),
    };
    try_s!(lp_register_coin(ctx, coin.clone(), register_params).await);

    let tx_history = req["tx_history"].as_bool().unwrap_or(false);
    if tx_history {
        try_s!(lp_spawn_tx_history(ctx.clone(), coin.clone()).map_to_mm(RegisterCoinError::Internal));
    }
    Ok(coin)
}

#[derive(Debug, Display)]
pub enum RegisterCoinError {
    #[display(fmt = "Coin '{coin}' is initialized already")]
    CoinIsInitializedAlready {
        coin: String,
    },
    Internal(String),
}

pub struct RegisterCoinParams {
    pub ticker: String,
}

pub async fn lp_register_coin(
    ctx: &MmArc,
    coin: MmCoinEnum,
    params: RegisterCoinParams,
) -> Result<(), MmError<RegisterCoinError>> {
    let RegisterCoinParams { ticker } = params;
    let cctx = CoinsContext::from_ctx(ctx).map_to_mm(RegisterCoinError::Internal)?;

    // TODO AP: locking the coins list during the entire initialization prevents different coins from being
    // activated concurrently which results in long activation time: https://github.com/KomodoPlatform/atomicDEX/issues/24
    // So I'm leaving the possibility of race condition intentionally in favor of faster concurrent activation.
    // Should consider refactoring: maybe extract the RPC client initialization part from coin init functions.
    {
        let mut coins = cctx.coins.lock().await;
        match coins.entry(ticker.clone()) {
            Entry::Occupied(_oe) => return MmError::err(RegisterCoinError::CoinIsInitializedAlready { coin: ticker }),
            Entry::Vacant(ve) => ve.insert(MmCoinStruct::new(coin.clone())),
        };
    };

    if coin.is_platform_coin() {
        cctx.platform_coin_tokens
            .lock()
            .entry(coin.ticker().to_string())
            .or_insert_with(HashSet::new);
    }

    Ok(())
}

fn lp_spawn_tx_history(ctx: MmArc, coin: MmCoinEnum) -> Result<(), String> {
    let spawner = coin.spawner();
    let fut = async move {
        let _res = coin.process_history_loop(ctx).compat().await;
    };
    spawner.spawn(fut);
    Ok(())
}

/// NB: Returns only the enabled (aka active) coins.
pub async fn lp_coinfind(ctx: &MmArc, ticker: &str) -> Result<Option<MmCoinEnum>, String> {
    let cctx = try_s!(CoinsContext::from_ctx(ctx));
    let coins = cctx.coins.lock().await;

    if let Some(coin) = coins.get(ticker) {
        if coin.is_available() {
            return Ok(Some(coin.inner.clone()));
        }
    };

    Ok(None)
}

/// Returns coins even if they are on the passive mode
pub async fn lp_coinfind_any(ctx: &MmArc, ticker: &str) -> Result<Option<MmCoinStruct>, String> {
    let cctx = try_s!(CoinsContext::from_ctx(ctx));
    let coins = cctx.coins.lock().await;

    Ok(coins.get(ticker).cloned())
}

/// Attempts to find a pair of active coins returning None if one is not enabled
pub async fn find_pair(ctx: &MmArc, base: &str, rel: &str) -> Result<Option<(MmCoinEnum, MmCoinEnum)>, String> {
    let fut_base = lp_coinfind(ctx, base);
    let fut_rel = lp_coinfind(ctx, rel);

    futures::future::try_join(fut_base, fut_rel)
        .map_ok(|(base, rel)| base.zip(rel))
        .await
}

#[derive(Debug, Display)]
pub enum CoinFindError {
    #[display(fmt = "No such coin: {coin}")]
    NoSuchCoin { coin: String },
}

pub async fn lp_coinfind_or_err(ctx: &MmArc, ticker: &str) -> CoinFindResult<MmCoinEnum> {
    match lp_coinfind(ctx, ticker).await {
        Ok(Some(coin)) => Ok(coin),
        Ok(None) => MmError::err(CoinFindError::NoSuchCoin {
            coin: ticker.to_owned(),
        }),
        Err(e) => panic!("Unexpected error: {}", e),
    }
}

#[derive(Deserialize)]
struct ConvertAddressReq {
    coin: String,
    from: String,
    /// format to that the input address should be converted
    to_address_format: Json,
}

pub async fn convert_address(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let req: ConvertAddressReq = try_s!(json::from_value(req));
    let coin = match lp_coinfind(&ctx, &req.coin).await {
        Ok(Some(t)) => t,
        Ok(None) => return ERR!("No such coin: {}", req.coin),
        Err(err) => return ERR!("!lp_coinfind({}): {}", req.coin, err),
    };
    let result = json!({
        "result": {
            "address": try_s!(coin.convert_to_address(&req.from, req.to_address_format)),
        },
    });
    let body = try_s!(json::to_vec(&result));
    Ok(try_s!(Response::builder().body(body)))
}

pub async fn kmd_rewards_info(ctx: MmArc) -> Result<Response<Vec<u8>>, String> {
    let coin = match lp_coinfind(&ctx, "KMD").await {
        Ok(Some(MmCoinEnum::UtxoCoin(t))) => t,
        Ok(Some(_)) => return ERR!("KMD was expected to be UTXO"),
        Ok(None) => return ERR!("KMD is not activated"),
        Err(err) => return ERR!("!lp_coinfind({}): KMD", err),
    };

    let res = json!({
        "result": try_s!(utxo::kmd_rewards_info(&coin).await),
    });
    let res = try_s!(json::to_vec(&res));
    Ok(try_s!(Response::builder().body(res)))
}

#[derive(Deserialize)]
struct ValidateAddressReq {
    coin: String,
    address: String,
}

#[derive(Serialize)]
pub struct ValidateAddressResult {
    pub is_valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

pub async fn validate_address(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let req: ValidateAddressReq = try_s!(json::from_value(req));
    let coin = match lp_coinfind(&ctx, &req.coin).await {
        Ok(Some(t)) => t,
        Ok(None) => return ERR!("No such coin: {}", req.coin),
        Err(err) => return ERR!("!lp_coinfind({}): {}", req.coin, err),
    };

    let res = json!({ "result": coin.validate_address(&req.address) });
    let body = try_s!(json::to_vec(&res));
    Ok(try_s!(Response::builder().body(body)))
}

pub async fn withdraw(ctx: MmArc, req: WithdrawRequest) -> WithdrawResult {
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;
    coin.withdraw(req).compat().await
}

pub async fn get_raw_transaction(ctx: MmArc, req: RawTransactionRequest) -> RawTransactionResult {
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;
    coin.get_raw_transaction(req).compat().await
}

pub async fn sign_message(ctx: MmArc, req: SignatureRequest) -> SignatureResult<SignatureResponse> {
    if req.address.is_some() && !ctx.enable_hd() {
        return MmError::err(SignatureError::InvalidRequest(
            "You need to enable kdf with enable_hd to sign messages with a specific account/address".to_string(),
        ));
    };
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;
    let signature = coin.sign_message(&req.message, req.address)?;

    Ok(SignatureResponse { signature })
}

pub async fn verify_message(ctx: MmArc, req: VerificationRequest) -> VerificationResult<VerificationResponse> {
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;

    let validate_address_result = coin.validate_address(&req.address);
    if !validate_address_result.is_valid {
        return MmError::err(VerificationError::InvalidRequest(
            validate_address_result.reason.unwrap_or_else(|| "Unknown".to_string()),
        ));
    }

    let is_valid = coin.verify_message(&req.signature, &req.message, &req.address)?;

    Ok(VerificationResponse { is_valid })
}

pub async fn sign_raw_transaction(ctx: MmArc, req: SignRawTransactionRequest) -> RawTransactionResult {
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;
    coin.sign_raw_tx(&req).await
}

pub async fn remove_delegation(ctx: MmArc, req: RemoveDelegateRequest) -> DelegationResult {
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;

    match req.staking_details {
        Some(StakingDetails::Cosmos(req)) => {
            if req.withdraw_from.is_some() {
                return MmError::err(DelegationError::InvalidPayload {
                    reason: "Can't use `withdraw_from` field on 'remove_delegation' RPC for Cosmos.".to_owned(),
                });
            }

            let MmCoinEnum::Tendermint(tendermint) = coin else {
                return MmError::err(DelegationError::CoinDoesntSupportDelegation {
                    coin: coin.ticker().to_string(),
                });
            };

            tendermint.undelegate(*req).await
        },

        Some(StakingDetails::Qtum(_)) => MmError::err(DelegationError::InvalidPayload {
            reason: "staking_details isn't supported for Qtum".into(),
        }),

        None => match coin {
            MmCoinEnum::QtumCoin(qtum) => qtum.remove_delegation().compat().await,
            _ => MmError::err(DelegationError::CoinDoesntSupportDelegation {
                coin: coin.ticker().to_string(),
            }),
        },
    }
}

pub async fn delegations_info(ctx: MmArc, req: DelegationsInfo) -> Result<Json, MmError<StakingInfoError>> {
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;

    match req.info_details {
        DelegationsInfoDetails::Qtum => {
            let MmCoinEnum::QtumCoin(qtum) = coin else {
                return MmError::err(StakingInfoError::InvalidPayload {
                    reason: format!("{} is not a Qtum coin", req.coin),
                });
            };

            qtum.get_delegation_infos().compat().await.map(|v| json!(v))
        },

        DelegationsInfoDetails::Cosmos(r) => match coin {
            MmCoinEnum::Tendermint(t) => Ok(t.delegations_list(r.paging).await.map(|v| json!(v)).map_mm_err()?),
            MmCoinEnum::TendermintToken(_) => MmError::err(StakingInfoError::InvalidPayload {
                reason: "Tokens are not supported for delegation".into(),
            }),
            _ => MmError::err(StakingInfoError::InvalidPayload {
                reason: format!("{} is not a Cosmos coin", req.coin),
            }),
        },
    }
}

pub async fn ongoing_undelegations_info(ctx: MmArc, req: UndelegationsInfo) -> Result<Json, MmError<StakingInfoError>> {
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;

    match req.info_details {
        UndelegationsInfoDetails::Cosmos(r) => match coin {
            MmCoinEnum::Tendermint(t) => Ok(t
                .ongoing_undelegations_list(r.paging)
                .await
                .map(|v| json!(v))
                .map_mm_err()?),
            MmCoinEnum::TendermintToken(_) => MmError::err(StakingInfoError::InvalidPayload {
                reason: "Tokens are not supported for delegation".into(),
            }),
            _ => MmError::err(StakingInfoError::InvalidPayload {
                reason: format!("{} is not a Cosmos coin", req.coin),
            }),
        },
    }
}

pub async fn validators_info(ctx: MmArc, req: ValidatorsInfo) -> Result<Json, MmError<StakingInfoError>> {
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;

    match req.info_details {
        ValidatorsInfoDetails::Cosmos(payload) => rpc_command::tendermint::staking::validators_rpc(coin, payload)
            .await
            .map(|v| json!(v)),
    }
}

pub async fn add_delegation(ctx: MmArc, req: AddDelegateRequest) -> DelegationResult {
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;

    match req.staking_details {
        StakingDetails::Qtum(req) => {
            let MmCoinEnum::QtumCoin(qtum) = coin else {
                return MmError::err(DelegationError::CoinDoesntSupportDelegation {
                    coin: coin.ticker().to_string(),
                });
            };

            qtum.add_delegation(req).compat().await
        },
        StakingDetails::Cosmos(req) => {
            let MmCoinEnum::Tendermint(tendermint) = coin else {
                return MmError::err(DelegationError::CoinDoesntSupportDelegation {
                    coin: coin.ticker().to_string(),
                });
            };

            tendermint.delegate(*req).await
        },
    }
}

pub async fn claim_staking_rewards(ctx: MmArc, req: ClaimStakingRewardsRequest) -> DelegationResult {
    match req.claiming_details {
        ClaimingDetails::Cosmos(r) => {
            let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;

            let MmCoinEnum::Tendermint(tendermint) = coin else {
                return MmError::err(DelegationError::InvalidPayload {
                    reason: format!("{} is not a Cosmos coin", req.coin),
                });
            };

            tendermint.claim_staking_rewards(r).await
        },
    }
}

pub async fn send_raw_transaction(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let ticker = try_s!(req["coin"].as_str().ok_or("No 'coin' field")).to_owned();
    let coin = match lp_coinfind(&ctx, &ticker).await {
        Ok(Some(t)) => t,
        Ok(None) => return ERR!("No such coin: {}", ticker),
        Err(err) => return ERR!("!lp_coinfind({}): {}", ticker, err),
    };
    let bytes_string = try_s!(req["tx_hex"].as_str().ok_or("No 'tx_hex' field"));
    let res = try_s!(coin.send_raw_tx(bytes_string).compat().await);
    let body = try_s!(json::to_vec(&json!({ "tx_hash": res })));
    Ok(try_s!(Response::builder().body(body)))
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", content = "additional_info")]
pub enum HistorySyncState {
    NotEnabled,
    NotStarted,
    InProgress(Json),
    Error(Json),
    Finished,
}

#[derive(Deserialize)]
struct MyTxHistoryRequest {
    coin: String,
    from_id: Option<BytesJson>,
    #[serde(default)]
    max: bool,
    #[serde(default = "ten")]
    limit: usize,
    page_number: Option<NonZeroUsize>,
}

/// Returns the transaction history of selected coin. Returns no more than `limit` records (default: 10).
/// Skips the first records up to from_id (skipping the from_id too).
/// Transactions are sorted by number of confirmations in ascending order.
pub async fn my_tx_history(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let request: MyTxHistoryRequest = try_s!(json::from_value(req));
    let coin = match lp_coinfind(&ctx, &request.coin).await {
        Ok(Some(t)) => t,
        Ok(None) => return ERR!("No such coin: {}", request.coin),
        Err(err) => return ERR!("!lp_coinfind({}): {}", request.coin, err),
    };

    let history = try_s!(coin.load_history_from_file(&ctx).compat().await);
    let total_records = history.len();
    let limit = if request.max { total_records } else { request.limit };

    let block_number = try_s!(coin.current_block().compat().await);
    let skip = match &request.from_id {
        Some(id) => {
            try_s!(history
                .iter()
                .position(|item| item.internal_id == *id)
                .ok_or(format!("from_id {id:02x} is not found")))
                + 1
        },
        None => match request.page_number {
            Some(page_n) => (page_n.get() - 1) * request.limit,
            None => 0,
        },
    };

    let history = history.into_iter().skip(skip).take(limit);
    let history: Vec<Json> = history
        .map(|item| {
            let tx_block = item.block_height;
            let mut json = json::to_value(item).unwrap();
            json["confirmations"] = if tx_block == 0 {
                Json::from(0)
            } else if block_number >= tx_block {
                Json::from((block_number - tx_block) + 1)
            } else {
                Json::from(0)
            };
            json
        })
        .collect();

    let response = json!({
        "result": {
            "transactions": history,
            "limit": limit,
            "skipped": skip,
            "from_id": request.from_id,
            "total": total_records,
            "current_block": block_number,
            "sync_status": coin.history_sync_status(),
            "page_number": request.page_number,
            "total_pages": calc_total_pages(total_records, request.limit),
        }
    });
    let body = try_s!(json::to_vec(&response));
    Ok(try_s!(Response::builder().body(body)))
}

/// `get_trade_fee` rpc implementation.
/// There is some consideration about this rpc:
/// for eth coin this rpc returns max possible trade fee (estimated for maximum possible gas limit for any kind of swap).
/// However for eth coin, as part of fixing this issue https://github.com/KomodoPlatform/komodo-defi-framework/issues/1848,
/// `max_taker_vol' and `trade_preimage` rpc now return more accurate required gas calculations.
/// So maybe it would be better to deprecate this `get_trade_fee` rpc
pub async fn get_trade_fee(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let ticker = try_s!(req["coin"].as_str().ok_or("No 'coin' field")).to_owned();
    let coin = match lp_coinfind(&ctx, &ticker).await {
        Ok(Some(t)) => t,
        Ok(None) => return ERR!("No such coin: {}", ticker),
        Err(err) => return ERR!("!lp_coinfind({}): {}", ticker, err),
    };
    let fee_info = try_s!(coin.get_trade_fee().compat().await);
    let res = try_s!(json::to_vec(&json!({
        "result": {
            "coin": fee_info.coin,
            "amount": fee_info.amount.to_decimal(),
            "amount_fraction": fee_info.amount.to_fraction(),
            "amount_rat": fee_info.amount.to_ratio(),
        }
    })));
    Ok(try_s!(Response::builder().body(res)))
}

pub async fn get_enabled_coins(ctx: MmArc) -> Result<Response<Vec<u8>>, String> {
    let coins_ctx: Arc<CoinsContext> = try_s!(CoinsContext::from_ctx(&ctx));
    let coins = coins_ctx.coins.lock().await;
    let enabled_coins: GetEnabledResponse = try_s!(coins
        .iter()
        .map(|(ticker, coin)| {
            let address = try_s!(coin.inner.my_address());
            Ok(EnabledCoin {
                ticker: ticker.clone(),
                address,
            })
        })
        .collect());
    let res = try_s!(json::to_vec(&Mm2RpcResult::new(enabled_coins)));
    Ok(try_s!(Response::builder().body(res)))
}

#[derive(Deserialize)]
pub struct ConfirmationsReq {
    coin: String,
    confirmations: u64,
}

pub async fn set_required_confirmations(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let req: ConfirmationsReq = try_s!(json::from_value(req));
    let coin = match lp_coinfind(&ctx, &req.coin).await {
        Ok(Some(t)) => t,
        Ok(None) => return ERR!("No such coin {}", req.coin),
        Err(err) => return ERR!("!lp_coinfind ({}): {}", req.coin, err),
    };
    coin.set_required_confirmations(req.confirmations);
    let res = try_s!(json::to_vec(&json!({
        "result": {
            "coin": req.coin,
            "confirmations": coin.required_confirmations(),
        }
    })));
    Ok(try_s!(Response::builder().body(res)))
}

#[derive(Deserialize)]
pub struct RequiresNotaReq {
    coin: String,
    requires_notarization: bool,
}

pub async fn set_requires_notarization(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let req: RequiresNotaReq = try_s!(json::from_value(req));
    let coin = match lp_coinfind(&ctx, &req.coin).await {
        Ok(Some(t)) => t,
        Ok(None) => return ERR!("No such coin {}", req.coin),
        Err(err) => return ERR!("!lp_coinfind ({}): {}", req.coin, err),
    };
    coin.set_requires_notarization(req.requires_notarization);
    let res = try_s!(json::to_vec(&json!({
        "result": {
            "coin": req.coin,
            "requires_notarization": coin.requires_notarization(),
        }
    })));
    Ok(try_s!(Response::builder().body(res)))
}

pub async fn show_priv_key(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let ticker = try_s!(req["coin"].as_str().ok_or("No 'coin' field")).to_owned();
    let coin = match lp_coinfind(&ctx, &ticker).await {
        Ok(Some(t)) => t,
        Ok(None) => return ERR!("No such coin: {}", ticker),
        Err(err) => return ERR!("!lp_coinfind({}): {}", ticker, err),
    };
    let res = try_s!(json::to_vec(&json!({
        "result": {
            "coin": ticker,
            "priv_key": try_s!(coin.display_priv_key()),
        }
    })));
    Ok(try_s!(Response::builder().body(res)))
}

pub async fn register_balance_update_handler(
    ctx: MmArc,
    handler: Box<dyn BalanceTradeFeeUpdatedHandler + Send + Sync>,
) {
    let coins_ctx = CoinsContext::from_ctx(&ctx).unwrap();
    coins_ctx.balance_update_handlers.lock().await.push(handler);
}

#[derive(Deserialize)]
struct ConvertUtxoAddressReq {
    address: String,
    to_coin: String,
}

pub async fn convert_utxo_address(ctx: MmArc, req: Json) -> Result<Response<Vec<u8>>, String> {
    let req: ConvertUtxoAddressReq = try_s!(json::from_value(req));
    let mut addr: utxo::LegacyAddress = try_s!(req.address.parse()); // Only legacy addresses supported as source
    let coin = match lp_coinfind(&ctx, &req.to_coin).await {
        Ok(Some(c)) => c,
        _ => return ERR!("Coin {} is not activated", req.to_coin),
    };
    let coin = match coin {
        MmCoinEnum::UtxoCoin(utxo) => utxo,
        _ => return ERR!("Coin {} is not utxo", req.to_coin),
    };
    addr.prefix = coin.as_ref().conf.address_prefixes.p2pkh.clone();
    addr.checksum_type = coin.as_ref().conf.checksum_type;

    let response = try_s!(json::to_vec(&json!({
        "result": addr.to_string(),
    })));
    Ok(try_s!(Response::builder().body(response)))
}

pub fn address_by_coin_conf_and_pubkey_str(
    ctx: &MmArc,
    coin: &str,
    conf: &Json,
    pubkey: &str,
    addr_format: UtxoAddressFormat,
) -> Result<String, String> {
    let protocol: CoinProtocol = try_s!(json::from_value(conf["protocol"].clone()));
    match protocol {
        CoinProtocol::ERC20 { .. } | CoinProtocol::ETH { .. } | CoinProtocol::NFT { .. } => {
            eth::addr_from_pubkey_str(pubkey)
        },
        // Todo: implement TRX address generation
        CoinProtocol::TRX { .. } => ERR!("TRX address generation is not implemented yet"),
        CoinProtocol::UTXO { .. } | CoinProtocol::QTUM | CoinProtocol::QRC20 { .. } | CoinProtocol::BCH { .. } => {
            utxo::address_by_conf_and_pubkey_str(coin, conf, pubkey, addr_format)
        },
        CoinProtocol::SLPTOKEN { platform, .. } => {
            let platform_conf = coin_conf(ctx, &platform);
            if platform_conf.is_null() {
                return ERR!("platform {} conf is null", platform);
            }
            // TODO is there any way to make it better without duplicating the prefix in the SLP conf?
            let platform_protocol: CoinProtocol = try_s!(json::from_value(platform_conf["protocol"].clone()));
            match platform_protocol {
                CoinProtocol::BCH { slp_prefix } => {
                    slp_addr_from_pubkey_str(pubkey, &slp_prefix).map_err(|e| ERRL!("{}", e))
                },
                _ => ERR!("Platform protocol {:?} is not BCH", platform_protocol),
            }
        },
        CoinProtocol::TENDERMINT(protocol) => tendermint::account_id_from_pubkey_hex(&protocol.account_prefix, pubkey)
            .map(|id| id.to_string())
            .map_err(|e| e.to_string()),
        CoinProtocol::TENDERMINTTOKEN(proto) => {
            let platform_conf = coin_conf(ctx, &proto.platform);
            if platform_conf.is_null() {
                return ERR!("platform {} conf is null", proto.platform);
            }
            // TODO is there any way to make it better without duplicating the prefix in the IBC conf?
            let platform_protocol: CoinProtocol = try_s!(json::from_value(platform_conf["protocol"].clone()));
            match platform_protocol {
                CoinProtocol::TENDERMINT(platform) => {
                    tendermint::account_id_from_pubkey_hex(&platform.account_prefix, pubkey)
                        .map(|id| id.to_string())
                        .map_err(|e| e.to_string())
                },
                _ => ERR!("Platform protocol {:?} is not TENDERMINT", platform_protocol),
            }
        },
        #[cfg(not(target_arch = "wasm32"))]
        CoinProtocol::LIGHTNING { .. } => {
            ERR!("address_by_coin_conf_and_pubkey_str is not implemented for lightning protocol yet!")
        },
        CoinProtocol::ZHTLC { .. } => ERR!("address_by_coin_conf_and_pubkey_str is not supported for ZHTLC protocol!"),
        #[cfg(feature = "enable-sia")]
        CoinProtocol::SIA => ERR!("address_by_coin_conf_and_pubkey_str is not supported for SIA protocol!"), // TODO Alright
    }
}

#[cfg(target_arch = "wasm32")]
fn load_history_from_file_impl<T>(coin: &T, ctx: &MmArc) -> TxHistoryFut<Vec<TransactionDetails>>
where
    T: MmCoin + ?Sized,
{
    let ctx = ctx.clone();
    let ticker = coin.ticker().to_owned();
    let my_address = try_f!(coin.my_address().map_mm_err());

    let fut = async move {
        let coins_ctx = CoinsContext::from_ctx(&ctx).unwrap();
        let db = coins_ctx.tx_history_db().await?;
        let err = match load_tx_history(&db, &ticker, &my_address).await {
            Ok(history) => return Ok(history),
            Err(e) => e,
        };

        if let TxHistoryError::ErrorDeserializing(e) = err.get_inner() {
            ctx.log.log(
                "🌋",
                &[&"tx_history", &ticker.to_owned()],
                &ERRL!("Error {} on history deserialization, resetting the cache.", e),
            );
            clear_tx_history(&db, &ticker, &my_address).await?;
            return Ok(Vec::new());
        }

        Err(err)
    };
    Box::new(fut.boxed().compat())
}

#[cfg(not(target_arch = "wasm32"))]
fn load_history_from_file_impl<T>(coin: &T, ctx: &MmArc) -> TxHistoryFut<Vec<TransactionDetails>>
where
    T: MmCoin + ?Sized,
{
    let ticker = coin.ticker().to_owned();
    let history_path = coin.tx_history_path(ctx);
    let ctx = ctx.clone();

    let fut = async move {
        let content = match fs::read(&history_path).await {
            Ok(content) => content,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Ok(Vec::new());
            },
            Err(err) => {
                let error = format!(
                    "Error '{}' reading from the history file {}",
                    err,
                    history_path.display()
                );
                return MmError::err(TxHistoryError::ErrorLoading(error));
            },
        };
        let serde_err = match json::from_slice(&content) {
            Ok(txs) => return Ok(txs),
            Err(e) => e,
        };

        ctx.log.log(
            "🌋",
            &[&"tx_history", &ticker],
            &ERRL!("Error {} on history deserialization, resetting the cache.", serde_err),
        );
        fs::remove_file(&history_path)
            .await
            .map_to_mm(|e| TxHistoryError::ErrorClearing(e.to_string()))?;
        Ok(Vec::new())
    };
    Box::new(fut.boxed().compat())
}

#[cfg(target_arch = "wasm32")]
fn save_history_to_file_impl<T>(coin: &T, ctx: &MmArc, mut history: Vec<TransactionDetails>) -> TxHistoryFut<()>
where
    T: MmCoin + MarketCoinOps + ?Sized,
{
    let ctx = ctx.clone();
    let ticker = coin.ticker().to_owned();
    let my_address = try_f!(coin.my_address().map_mm_err());

    history.sort_unstable_by(compare_transaction_details);

    let fut = async move {
        let coins_ctx = CoinsContext::from_ctx(&ctx).unwrap();
        let db = coins_ctx.tx_history_db().await?;
        save_tx_history(&db, &ticker, &my_address, history).await?;
        Ok(())
    };
    Box::new(fut.boxed().compat())
}

#[cfg(not(target_arch = "wasm32"))]
fn get_tx_history_migration_impl<T>(coin: &T, ctx: &MmArc) -> TxHistoryFut<u64>
where
    T: MmCoin + MarketCoinOps + ?Sized,
{
    let migration_path = coin.tx_migration_path(ctx);

    let fut = async move {
        let current_migration = match fs::read(&migration_path).await {
            Ok(bytes) => {
                let mut num_bytes = [0; 8];
                if bytes.len() == 8 {
                    num_bytes.clone_from_slice(&bytes);
                    u64::from_le_bytes(num_bytes)
                } else {
                    0
                }
            },
            Err(_) => 0,
        };

        Ok(current_migration)
    };

    Box::new(fut.boxed().compat())
}

#[cfg(not(target_arch = "wasm32"))]
fn update_migration_file_impl<T>(coin: &T, ctx: &MmArc, migration_number: u64) -> TxHistoryFut<()>
where
    T: MmCoin + MarketCoinOps + ?Sized,
{
    let migration_path = coin.tx_migration_path(ctx);
    let tmp_file = format!("{}.tmp", migration_path.display());

    let fut = async move {
        let fs_fut = async {
            let mut file = fs::File::create(&tmp_file).await?;
            file.write_all(&migration_number.to_le_bytes()).await?;
            file.flush().await?;
            fs::rename(&tmp_file, migration_path).await?;
            Ok(())
        };

        let res: io::Result<_> = fs_fut.await;
        if let Err(e) = res {
            let error = format!("Error '{e}' creating/writing/renaming the tmp file {tmp_file}");
            return MmError::err(TxHistoryError::ErrorSaving(error));
        }
        Ok(())
    };

    Box::new(fut.boxed().compat())
}

#[cfg(not(target_arch = "wasm32"))]
fn save_history_to_file_impl<T>(coin: &T, ctx: &MmArc, mut history: Vec<TransactionDetails>) -> TxHistoryFut<()>
where
    T: MmCoin + MarketCoinOps + ?Sized,
{
    let history_path = coin.tx_history_path(ctx);
    let tmp_file = format!("{}.tmp", history_path.display());

    history.sort_unstable_by(compare_transaction_details);

    let fut = async move {
        let content = json::to_vec(&history).map_to_mm(|e| TxHistoryError::ErrorSerializing(e.to_string()))?;

        let fs_fut = async {
            let mut file = fs::File::create(&tmp_file).await?;
            file.write_all(&content).await?;
            file.flush().await?;
            fs::rename(&tmp_file, &history_path).await?;
            Ok(())
        };

        let res: io::Result<_> = fs_fut.await;
        if let Err(e) = res {
            let error = format!("Error '{e}' creating/writing/renaming the tmp file {tmp_file}");
            return MmError::err(TxHistoryError::ErrorSaving(error));
        }
        Ok(())
    };
    Box::new(fut.boxed().compat())
}

pub(crate) fn compare_transaction_details(a: &TransactionDetails, b: &TransactionDetails) -> Ordering {
    let a = TxIdHeight::new(a.block_height, a.internal_id.deref());
    let b = TxIdHeight::new(b.block_height, b.internal_id.deref());
    compare_transactions(a, b)
}

pub(crate) struct TxIdHeight<Id> {
    block_height: u64,
    tx_id: Id,
}

impl<Id> TxIdHeight<Id> {
    pub(crate) fn new(block_height: u64, tx_id: Id) -> TxIdHeight<Id> {
        TxIdHeight { block_height, tx_id }
    }
}

pub(crate) fn compare_transactions<Id>(a: TxIdHeight<Id>, b: TxIdHeight<Id>) -> Ordering
where
    Id: Ord,
{
    // the transactions with block_height == 0 are the most recent so we need to separately handle them while sorting
    if a.block_height == b.block_height {
        a.tx_id.cmp(&b.tx_id)
    } else if a.block_height == 0 {
        Ordering::Less
    } else if b.block_height == 0 {
        Ordering::Greater
    } else {
        b.block_height.cmp(&a.block_height)
    }
}

/// Use trait in the case, when we have to send requests to rpc client.
#[async_trait]
pub trait RpcCommonOps {
    type RpcClient;
    type Error;

    /// Returns an alive RPC client or returns an error if no RPC endpoint is currently available.
    async fn get_live_client(&self) -> Result<Self::RpcClient, Self::Error>;
}

/// `get_my_address` function returns wallet address for necessary coin without its activation.
/// Currently supports only coins with `ETH` protocol type.
pub async fn get_my_address(ctx: MmArc, req: MyAddressReq) -> MmResult<MyWalletAddress, GetMyAddressError> {
    let ticker = req.coin.as_str();
    let conf = coin_conf(&ctx, ticker);
    coins_conf_check(&ctx, &conf, ticker, None).map_to_mm(GetMyAddressError::CoinsConfCheckError)?;

    let protocol: CoinProtocol = json::from_value(conf["protocol"].clone())?;

    let my_address = match protocol {
        CoinProtocol::ETH { .. } => get_eth_address(&ctx, &conf, ticker, &req.path_to_address)
            .await
            .map_mm_err()?,
        _ => {
            return MmError::err(GetMyAddressError::CoinIsNotSupported(format!(
                "{} doesn't support get_my_address",
                req.coin
            )));
        },
    };

    Ok(my_address)
}

fn coins_conf_check(ctx: &MmArc, coins_en: &Json, ticker: &str, req: Option<&Json>) -> Result<(), String> {
    if coins_en.is_null() {
        let warning = format!("Warning, coin {ticker} is used without a corresponding configuration.");
        ctx.log.log(
            "😅",
            #[allow(clippy::unnecessary_cast)]
            &[&("coin" as &str), &ticker, &("no-conf" as &str)],
            &warning,
        );
    }

    if let Some(req) = req {
        if coins_en["mm2"].is_null() && req["mm2"].is_null() {
            return ERR!(concat!(
                "mm2 param is not set neither in coins config nor enable request, assuming that coin is not supported"
            ));
        }
    } else if coins_en["mm2"].is_null() {
        return ERR!(concat!(
            "mm2 param is not set in coins config, assuming that coin is not supported"
        ));
    }

    if coins_en["protocol"].is_null() {
        return ERR!(
            r#""protocol" field is missing in coins file. The file format is deprecated, please execute ./mm2 update_config command to convert it or download a new one"#
        );
    }
    Ok(())
}

#[async_trait]
pub trait Eip1559Ops {
    /// Return swap transaction fee policy
    async fn get_swap_gas_fee_policy(&self) -> CoinFindResult<SwapGasFeePolicy>;

    /// set swap transaction fee policy
    async fn set_swap_gas_fee_policy(&self, swap_txfee_policy: SwapGasFeePolicy) -> CoinFindResult<()>;
}

/// Get the current eip 1559 transaction fee per gas policy
pub async fn get_swap_gas_fee_policy(ctx: MmArc, req: GetSwapGasFeePolicyRequest) -> SwapGasFeePolicyResult {
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;
    match coin {
        MmCoinEnum::EthCoin(eth_coin) => Ok(eth_coin.get_swap_gas_fee_policy().await.map_mm_err()?),
        _ => MmError::err(SwapGasFeePolicyError::NotSupported(req.coin)),
    }
}

/// Set eip 1559 transaction fee per gas policy (low, medium or high)
pub async fn set_swap_gas_fee_policy(ctx: MmArc, req: SetSwapGasFeePolicyRequest) -> SwapGasFeePolicyResult {
    let coin = lp_coinfind_or_err(&ctx, &req.coin).await.map_mm_err()?;
    match coin {
        MmCoinEnum::EthCoin(eth_coin) => {
            eth_coin
                .set_swap_gas_fee_policy(req.swap_gas_fee_policy)
                .await
                .map_mm_err()?;
            Ok(eth_coin.get_swap_gas_fee_policy().await.map_mm_err()?)
        },
        _ => MmError::err(SwapGasFeePolicyError::NotSupported(req.coin)),
    }
}

/// Checks addresses that either had empty transaction history last time we checked or has not been checked before.
/// The checking stops at the moment when we find `gap_limit` consecutive empty addresses.
pub async fn scan_for_new_addresses_impl<T>(
    coin: &T,
    hd_wallet: &T::HDWallet,
    hd_account: &mut HDCoinHDAccount<T>,
    address_scanner: &T::HDAddressScanner,
    chain: Bip44Chain,
    gap_limit: u32,
) -> BalanceResult<Vec<HDAddressBalance<HDWalletBalanceObject<T>>>>
where
    T: HDWalletBalanceOps + Sync,
{
    let mut balances = Vec::with_capacity(gap_limit as usize);

    // Get the first unknown address id.
    let mut checking_address_id = hd_account
        .known_addresses_number(chain)
        // A UTXO coin should support both [`Bip44Chain::External`] and [`Bip44Chain::Internal`].
        .mm_err(|e| BalanceError::Internal(e.to_string()))?;

    let mut unused_addresses_counter = 0;
    let max_addresses_number = hd_account.address_limit();
    while checking_address_id < max_addresses_number && unused_addresses_counter <= gap_limit {
        let hd_address = coin
            .derive_address(hd_account, chain, checking_address_id)
            .await
            .map_mm_err()?;
        let checking_address = hd_address.address();
        let checking_address_der_path = hd_address.derivation_path();

        match coin.is_address_used(&checking_address, address_scanner).await? {
            // We found a non-empty address, so we have to fill up the balance list
            // with zeros starting from `last_non_empty_address_id = checking_address_id - unused_addresses_counter`.
            AddressBalanceStatus::Used(non_empty_balance) => {
                let last_non_empty_address_id = checking_address_id - unused_addresses_counter;

                // First, derive all empty addresses and put it into `balances` with default balance.
                let address_ids = (last_non_empty_address_id..checking_address_id)
                    .map(|address_id| HDAddressId { chain, address_id });
                let empty_addresses = coin
                    .derive_addresses(hd_account, address_ids)
                    .await
                    .map_mm_err()?
                    .into_iter()
                    .map(|empty_address| HDAddressBalance {
                        address: empty_address.address().display_address(),
                        derivation_path: RpcDerivationPath(empty_address.derivation_path().clone()),
                        chain,
                        balance: HDWalletBalanceObject::<T>::new(),
                    });
                balances.extend(empty_addresses);

                // Then push this non-empty address.
                balances.push(HDAddressBalance {
                    address: checking_address.display_address(),
                    derivation_path: RpcDerivationPath(checking_address_der_path.clone()),
                    chain,
                    balance: non_empty_balance,
                });
                // Reset the counter of unused addresses to zero since we found a non-empty address.
                unused_addresses_counter = 0;
            },
            AddressBalanceStatus::NotUsed => unused_addresses_counter += 1,
        }

        checking_address_id += 1;
    }

    coin.set_known_addresses_number(
        hd_wallet,
        hd_account,
        chain,
        checking_address_id - unused_addresses_counter,
    )
    .await
    .map_mm_err()?;

    Ok(balances)
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::block_on;
    use mm2_test_helpers::for_tests::RICK;
    use mocktopus::mocking::{MockResult, Mockable};

    #[test]
    fn test_lp_coinfind() {
        let ctx = mm2_core::mm_ctx::MmCtxBuilder::default().into_mm_arc();
        let coins_ctx = CoinsContext::from_ctx(&ctx).unwrap();
        let coin = MmCoinEnum::Test(TestCoin::new(RICK));

        // Add test coin to coins context
        common::block_on(coins_ctx.add_platform_with_tokens(coin.clone(), vec![], None)).unwrap();

        // Try to find RICK from coins context that was added above
        let _found = common::block_on(lp_coinfind(&ctx, RICK)).unwrap();

        assert!(matches!(Some(coin), _found));

        block_on(coins_ctx.coins.lock())
            .get(RICK)
            .unwrap()
            .update_is_available(false);

        // Try to find RICK from coins context after making it passive
        let found = common::block_on(lp_coinfind(&ctx, RICK)).unwrap();

        assert!(found.is_none());
    }

    #[test]
    fn test_lp_coinfind_any() {
        let ctx = mm2_core::mm_ctx::MmCtxBuilder::default().into_mm_arc();
        let coins_ctx = CoinsContext::from_ctx(&ctx).unwrap();
        let coin = MmCoinEnum::Test(TestCoin::new(RICK));

        // Add test coin to coins context
        common::block_on(coins_ctx.add_platform_with_tokens(coin.clone(), vec![], None)).unwrap();

        // Try to find RICK from coins context that was added above
        let _found = common::block_on(lp_coinfind_any(&ctx, RICK)).unwrap();

        assert!(matches!(Some(coin.clone()), _found));

        block_on(coins_ctx.coins.lock())
            .get(RICK)
            .unwrap()
            .update_is_available(false);

        // Try to find RICK from coins context after making it passive
        let _found = common::block_on(lp_coinfind_any(&ctx, RICK)).unwrap();

        assert!(matches!(Some(coin), _found));
    }

    #[test]
    fn test_dex_fee_amount() {
        let base = "BTC";
        let btc = TestCoin::new(base);
        TestCoin::should_burn_dex_fee.mock_safe(|_| MockResult::Return(true));
        TestCoin::min_tx_amount.mock_safe(|_| MockResult::Return(MmNumber::from("0.0001").into()));
        let rel = "ETH";
        let amount = 1.into();
        let actual_fee = DexFee::new_from_taker_coin(&btc, rel, &amount);
        let expected_fee = DexFee::WithBurn {
            fee_amount: amount.clone() / 777u64.into() * "0.75".into(),
            burn_amount: amount / 777u64.into() * "0.25".into(),
            burn_destination: DexFeeBurnDestination::PreBurnAccount,
        };
        assert_eq!(expected_fee, actual_fee);
        TestCoin::should_burn_dex_fee.clear_mock();

        let base = "KMD";
        let kmd = TestCoin::new(base);
        TestCoin::should_burn_dex_fee.mock_safe(|_| MockResult::Return(true));
        TestCoin::min_tx_amount.mock_safe(|_| MockResult::Return(MmNumber::from("0.0001").into()));
        let rel = "ETH";
        let amount = 1.into();
        let actual_fee = DexFee::new_from_taker_coin(&kmd, rel, &amount);
        let expected_fee = amount.clone() * (9, 7770).into() * MmNumber::from("0.75");
        let expected_burn_amount = amount * (9, 7770).into() * MmNumber::from("0.25");
        assert_eq!(
            DexFee::WithBurn {
                fee_amount: expected_fee,
                burn_amount: expected_burn_amount,
                burn_destination: DexFeeBurnDestination::KmdOpReturn,
            },
            actual_fee
        );
        TestCoin::should_burn_dex_fee.clear_mock();

        // check the case when KMD taker fee is close to dust (0.75 of fee < dust)
        let base = "KMD";
        let kmd = TestCoin::new(base);
        TestCoin::should_burn_dex_fee.mock_safe(|_| MockResult::Return(true));
        TestCoin::min_tx_amount.mock_safe(|_| MockResult::Return(MmNumber::from("0.00001").into()));
        let rel = "BTC";
        let amount = (1001 * 777, 90000000).into();
        let actual_fee = DexFee::new_from_taker_coin(&kmd, rel, &amount);
        assert_eq!(
            DexFee::WithBurn {
                fee_amount: "0.00001".into(), // equals to min_tx_amount
                burn_amount: "0.00000001".into(),
                burn_destination: DexFeeBurnDestination::KmdOpReturn,
            },
            actual_fee
        );
        TestCoin::should_burn_dex_fee.clear_mock();

        let base = "BTC";
        let btc = TestCoin::new(base);
        TestCoin::should_burn_dex_fee.mock_safe(|_| MockResult::Return(true));
        TestCoin::min_tx_amount.mock_safe(|_| MockResult::Return(MmNumber::from("0.00001").into()));
        let rel = "KMD";
        let amount = 1.into();
        let actual_fee = DexFee::new_from_taker_coin(&btc, rel, &amount);
        let expected_fee = DexFee::WithBurn {
            fee_amount: amount.clone() * (9, 7770).into() * "0.75".into(),
            burn_amount: amount * (9, 7770).into() * "0.25".into(),
            burn_destination: DexFeeBurnDestination::PreBurnAccount,
        };
        assert_eq!(expected_fee, actual_fee);
        TestCoin::should_burn_dex_fee.clear_mock();

        // whole dex fee (0.001 * 9 / 7770) less than min tx amount (0.00001)
        let base = "BTC";
        let btc = TestCoin::new(base);
        TestCoin::should_burn_dex_fee.mock_safe(|_| MockResult::Return(true));
        TestCoin::min_tx_amount.mock_safe(|_| MockResult::Return(MmNumber::from("0.00001").into()));
        let rel = "KMD";
        let amount: MmNumber = "0.001".parse::<BigDecimal>().unwrap().into();
        let actual_fee = DexFee::new_from_taker_coin(&btc, rel, &amount);
        assert_eq!(DexFee::Standard("0.00001".into()), actual_fee);
        TestCoin::should_burn_dex_fee.clear_mock();

        // 75% of dex fee (0.03 * 9/7770 * 0.75) is over the min tx amount (0.00001)
        // but non-kmd burn amount is less than the min tx amount
        let base = "BTC";
        let btc = TestCoin::new(base);
        TestCoin::should_burn_dex_fee.mock_safe(|_| MockResult::Return(true));
        TestCoin::min_tx_amount.mock_safe(|_| MockResult::Return(MmNumber::from("0.00001").into()));
        let rel = "KMD";
        let amount: MmNumber = "0.03".parse::<BigDecimal>().unwrap().into();
        let actual_fee = DexFee::new_from_taker_coin(&btc, rel, &amount);
        assert_eq!(DexFee::Standard(amount * (9, 7770).into()), actual_fee);
        TestCoin::should_burn_dex_fee.clear_mock();

        // burning from eth currently not supported
        let base = "USDT-ERC20";
        let btc = TestCoin::new(base);
        TestCoin::should_burn_dex_fee.mock_safe(|_| MockResult::Return(false));
        TestCoin::min_tx_amount.mock_safe(|_| MockResult::Return(MmNumber::from("0.00001").into()));
        let rel = "BTC";
        let amount: MmNumber = "1".parse::<BigDecimal>().unwrap().into();
        let actual_fee = DexFee::new_from_taker_coin(&btc, rel, &amount);
        assert_eq!(DexFee::Standard(amount / "777".into()), actual_fee);
        TestCoin::should_burn_dex_fee.clear_mock();

        let base = "NUCLEUS";
        let btc = TestCoin::new(base);
        TestCoin::should_burn_dex_fee.mock_safe(|_| MockResult::Return(true));
        TestCoin::min_tx_amount.mock_safe(|_| MockResult::Return(MmNumber::from("0.000001").into()));
        let rel = "IRIS";
        let amount: MmNumber = "0.008".parse::<BigDecimal>().unwrap().into();
        let actual_fee = DexFee::new_from_taker_coin(&btc, rel, &amount);
        let std_fee = amount / "777".into();
        let fee_amount = std_fee.clone() * "0.75".into();
        let burn_amount = std_fee - fee_amount.clone();
        assert_eq!(
            DexFee::WithBurn {
                fee_amount,
                burn_amount,
                burn_destination: DexFeeBurnDestination::PreBurnAccount,
            },
            actual_fee
        );
        TestCoin::should_burn_dex_fee.clear_mock();

        // test NoFee if taker is dex
        let base = "BTC";
        let btc = TestCoin::new(base);
        TestCoin::should_burn_dex_fee.mock_safe(|_| MockResult::Return(true));
        TestCoin::dex_pubkey.mock_safe(|_| MockResult::Return(DEX_BURN_ADDR_RAW_PUBKEY.as_slice()));
        TestCoin::min_tx_amount.mock_safe(|_| MockResult::Return(MmNumber::from("0.00001").into()));
        let rel = "KMD";
        let amount: MmNumber = "0.03".parse::<BigDecimal>().unwrap().into();
        let actual_fee = DexFee::new_with_taker_pubkey(&btc, rel, &amount, DEX_BURN_ADDR_RAW_PUBKEY.as_slice());
        assert_eq!(DexFee::NoFee, actual_fee);
        TestCoin::should_burn_dex_fee.clear_mock();
        TestCoin::dex_pubkey.clear_mock();
    }
}

#[cfg(all(feature = "for-tests", not(target_arch = "wasm32")))]
pub mod for_tests {
    use crate::rpc_command::init_withdraw::WithdrawStatusRequest;
    use crate::rpc_command::init_withdraw::{init_withdraw, withdraw_status};
    use crate::{HDAddressSelector, TransactionDetails, WithdrawError, WithdrawFee, WithdrawRequest};
    use common::executor::Timer;
    use common::{now_ms, wait_until_ms};
    use mm2_core::mm_ctx::MmArc;
    use mm2_err_handle::prelude::MmResult;
    use mm2_number::BigDecimal;
    use rpc_task::{RpcInitReq, RpcTaskStatus};
    use std::str::FromStr;

    /// Helper to call init_withdraw and wait for completion
    pub async fn test_withdraw_init_loop(
        ctx: MmArc,
        ticker: &str,
        to: &str,
        amount: &str,
        from_derivation_path: Option<&str>,
        fee: Option<WithdrawFee>,
    ) -> MmResult<TransactionDetails, WithdrawError> {
        let withdraw_req = RpcInitReq {
            client_id: 0,
            inner: WithdrawRequest {
                amount: BigDecimal::from_str(amount).unwrap(),
                from: from_derivation_path.map(|from_derivation_path| HDAddressSelector::DerivationPath {
                    derivation_path: from_derivation_path.to_owned(),
                }),
                to: to.to_owned(),
                coin: ticker.to_owned(),
                fee,
                ..Default::default()
            },
        };
        let init = init_withdraw(ctx.clone(), withdraw_req).await.unwrap();
        let timeout = wait_until_ms(150000);
        loop {
            if now_ms() > timeout {
                panic!("{} init_withdraw timed out", ticker);
            }
            let status = withdraw_status(
                ctx.clone(),
                WithdrawStatusRequest {
                    task_id: init.task_id,
                    forget_if_finished: true,
                },
            )
            .await;
            if let Ok(status) = status {
                match status {
                    RpcTaskStatus::Ok(tx_details) => break Ok(tx_details),
                    RpcTaskStatus::Error(e) => break Err(e),
                    _ => Timer::sleep(1.).await,
                }
            } else {
                panic!("{} could not get withdraw_status", ticker)
            }
        }
    }
}
