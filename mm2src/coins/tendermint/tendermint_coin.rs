use super::ethermint_account::EthermintAccount;
use super::htlc::{
    ClaimHtlcMsg, ClaimHtlcProto, CreateHtlcMsg, CreateHtlcProto, HtlcType, QueryHtlcRequestProto, QueryHtlcResponse,
    TendermintHtlc, HTLC_STATE_COMPLETED, HTLC_STATE_OPEN, HTLC_STATE_REFUNDED,
};
use super::ibc::transfer_v1::MsgTransfer;
use super::ibc::IBC_GAS_LIMIT_DEFAULT;
use super::rpc::*;
use crate::coin_errors::{AddressFromPubkeyError, MyAddressError, ValidatePaymentError, ValidatePaymentResult};
use crate::hd_wallet::{HDAddressSelector, HDPathAccountToAddressId};
use crate::rpc_command::tendermint::ibc::ChannelId;
use crate::rpc_command::tendermint::staking::{
    ClaimRewardsPayload, Delegation, DelegationPayload, DelegationsQueryResponse, Undelegation, UndelegationEntry,
    UndelegationsQueryResponse, ValidatorStatus,
};
use crate::utxo::sat_from_big_decimal;
use crate::utxo::utxo_common::big_decimal_from_sat;
use crate::{
    big_decimal_from_sat_unsigned, BalanceError, BalanceFut, BigDecimal, CheckIfMyPaymentSentArgs, CoinBalance,
    ConfirmPaymentInput, DelegationError, DexFee, FeeApproxStage, FoundSwapTxSpend, HistorySyncState, MarketCoinOps,
    MmCoin, NegotiateSwapContractAddrErr, PrivKeyBuildPolicy, PrivKeyPolicy, PrivKeyPolicyNotAllowed,
    RawTransactionError, RawTransactionFut, RawTransactionRequest, RawTransactionRes, RawTransactionResult,
    RefundPaymentArgs, RpcCommonOps, SearchForSwapTxSpendInput, SendPaymentArgs, SignRawTransactionRequest,
    SignatureError, SignatureResult, SpendPaymentArgs, SwapOps, ToBytes, TradeFee, TradePreimageError,
    TradePreimageFut, TradePreimageResult, TradePreimageValue, TransactionData, TransactionDetails, TransactionEnum,
    TransactionErr, TransactionFut, TransactionResult, TransactionType, TxFeeDetails, TxMarshalingErr,
    UnexpectedDerivationMethod, ValidateAddressResult, ValidateFeeArgs, ValidateOtherPubKeyErr, ValidatePaymentFut,
    ValidatePaymentInput, VerificationError, VerificationResult, WaitForHTLCTxSpendArgs, WatcherOps, WeakSpawner,
    WithdrawError, WithdrawFee, WithdrawFut, WithdrawRequest,
};
use async_std::prelude::FutureExt as AsyncStdFutureExt;
use async_trait::async_trait;
use bip32::DerivationPath;
use bitcrypto::{dhash160, sha256};
use common::executor::{abortable_queue::AbortableQueue, AbortableSystem};
use common::executor::{AbortedError, Timer};
use common::log::{debug, warn};
use common::{get_utc_timestamp, now_sec, Future01CompatExt, PagingOptions, DEX_FEE_ADDR_PUBKEY};
use compatible_time::Duration;
use cosmrs::bank::{MsgMultiSend, MsgSend, MultiSendIo};
use cosmrs::crypto::secp256k1::SigningKey;
use cosmrs::distribution::MsgWithdrawDelegatorReward;
use cosmrs::proto::cosmos::auth::v1beta1::{BaseAccount, QueryAccountRequest, QueryAccountResponse};
use cosmrs::proto::cosmos::bank::v1beta1::{
    MsgMultiSend as MsgMultiSendProto, MsgSend as MsgSendProto, QueryBalanceRequest, QueryBalanceResponse,
};
use cosmrs::proto::cosmos::base::query::v1beta1::PageRequest;
use cosmrs::proto::cosmos::base::tendermint::v1beta1::{
    GetBlockByHeightRequest, GetBlockByHeightResponse, GetLatestBlockRequest, GetLatestBlockResponse,
};
use cosmrs::proto::cosmos::base::v1beta1::{Coin as CoinProto, DecCoin};
use cosmrs::proto::cosmos::distribution::v1beta1::{QueryDelegationRewardsRequest, QueryDelegationRewardsResponse};
use cosmrs::proto::cosmos::staking::v1beta1::{
    QueryDelegationRequest, QueryDelegationResponse, QueryDelegatorDelegationsRequest,
    QueryDelegatorDelegationsResponse, QueryDelegatorUnbondingDelegationsRequest,
    QueryDelegatorUnbondingDelegationsResponse, QueryValidatorsRequest,
    QueryValidatorsResponse as QueryValidatorsResponseProto,
};
use cosmrs::proto::cosmos::tx::v1beta1::{
    GetTxRequest, GetTxResponse, SimulateRequest, SimulateResponse, Tx, TxBody, TxRaw,
};
use cosmrs::proto::ibc;
use cosmrs::proto::ibc::core::channel::v1::{QueryChannelRequest, QueryChannelResponse};
use cosmrs::proto::prost::{DecodeError, Message};
use cosmrs::staking::{MsgDelegate, MsgUndelegate, QueryValidatorsResponse, Validator};
use cosmrs::tendermint::block::Height;
use cosmrs::tendermint::chain::Id as ChainId;
use cosmrs::tendermint::PublicKey;
use cosmrs::tx::{self, Fee, Msg, Raw, SignDoc, SignerInfo};
use cosmrs::{AccountId, Any, Coin, Denom, ErrorReport};
use crypto::privkey::key_pair_from_secret;
use crypto::{HDPathToCoin, Secp256k1Secret};
use derive_more::Display;
use futures::future::try_join_all;
use futures::lock::Mutex as AsyncMutex;
use futures::{FutureExt, TryFutureExt};
use futures01::Future;
use hex::FromHexError;
use itertools::Itertools;
use kdf_walletconnect::{WalletConnectCtx, WalletConnectOps};
use keys::{KeyPair, Public};
use mm2_core::mm_ctx::{MmArc, MmWeak};
use mm2_err_handle::prelude::*;
use mm2_number::bigdecimal::ParseBigDecimalError;
use mm2_number::MmNumber;
use mm2_p2p::p2p_ctx::P2PContext;
use num_traits::Zero;
use parking_lot::Mutex as PaMutex;
use primitives::hash::H256;
use regex::Regex;
use rpc::v1::types::{Bytes as BytesJson, H264 as H264Json};
use serde_json::{self as json, Value as Json};
use std::collections::HashMap;
use std::convert::{TryFrom, TryInto};
use std::io;
use std::num::NonZeroU32;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[cfg(test)]
use mocktopus::macros::*;

// ABCI Request Paths
const ABCI_GET_LATEST_BLOCK_PATH: &str = "/cosmos.base.tendermint.v1beta1.Service/GetLatestBlock";
const ABCI_GET_BLOCK_BY_HEIGHT_PATH: &str = "/cosmos.base.tendermint.v1beta1.Service/GetBlockByHeight";
const ABCI_SIMULATE_TX_PATH: &str = "/cosmos.tx.v1beta1.Service/Simulate";
const ABCI_QUERY_ACCOUNT_PATH: &str = "/cosmos.auth.v1beta1.Query/Account";
const ABCI_QUERY_BALANCE_PATH: &str = "/cosmos.bank.v1beta1.Query/Balance";
const ABCI_GET_TX_PATH: &str = "/cosmos.tx.v1beta1.Service/GetTx";
const ABCI_VALIDATORS_PATH: &str = "/cosmos.staking.v1beta1.Query/Validators";
const ABCI_DELEGATION_PATH: &str = "/cosmos.staking.v1beta1.Query/Delegation";
const ABCI_DELEGATOR_DELEGATIONS_PATH: &str = "/cosmos.staking.v1beta1.Query/DelegatorDelegations";
const ABCI_DELEGATOR_UNDELEGATIONS_PATH: &str = "/cosmos.staking.v1beta1.Query/DelegatorUnbondingDelegations";
const ABCI_DELEGATION_REWARDS_PATH: &str = "/cosmos.distribution.v1beta1.Query/DelegationRewards";
const ABCI_IBC_CHANNEL_QUERY_PATH: &str = "/ibc.core.channel.v1.Query/Channel";

#[cfg(feature = "ibc-routing-for-swaps")]
const DEFAULT_MIN_BALANCE_FOR_IBC_ROUTING: f32 = 2.0;

pub(crate) const MIN_TX_SATOSHIS: i64 = 1;

// ABCI Request Defaults
const ABCI_REQUEST_HEIGHT: Option<Height> = None;
const ABCI_REQUEST_PROVE: bool = false;

/// 0.25 is good average gas price on atom and iris
const DEFAULT_GAS_PRICE: f64 = 0.25;
pub(super) const TIMEOUT_HEIGHT_DELTA: u64 = 100;
pub const GAS_LIMIT_DEFAULT: u64 = 125_000;
pub const GAS_WANTED_BASE_VALUE: f64 = 50_000.;
pub(crate) const TX_DEFAULT_MEMO: &str = "";

// https://github.com/irisnet/irismod/blob/5016c1be6fdbcffc319943f33713f4a057622f0a/modules/htlc/types/validation.go#L19-L22
const MAX_TIME_LOCK: i64 = 34560;
const MIN_TIME_LOCK: i64 = 50;

const ACCOUNT_SEQUENCE_ERR: &str = "account sequence mismatch";

pub(crate) const IRIS_PREFIX: &str = "iaa";
pub(crate) const NUCLEUS_PREFIX: &str = "nuc";

lazy_static! {
    static ref SEQUENCE_PARSER_REGEX: Regex = Regex::new(r"expected (\d+)").unwrap();
}

pub struct SerializedUnsignedTx {
    tx_json: Json,
    body_bytes: Vec<u8>,
}

type TendermintPrivKeyPolicy = PrivKeyPolicy<TendermintKeyPair>;

pub struct TendermintKeyPair {
    private_key_secret: Secp256k1Secret,
    public_key: Public,
}

impl TendermintKeyPair {
    fn new(private_key_secret: Secp256k1Secret, public_key: Public) -> Self {
        Self {
            private_key_secret,
            public_key,
        }
    }
}

#[derive(Clone, Deserialize)]
pub struct RpcNode {
    url: String,
    #[serde(default)]
    komodo_proxy: bool,
}

impl RpcNode {
    #[cfg(test)]
    fn for_test(url: &str) -> Self {
        Self {
            url: url.to_string(),
            komodo_proxy: false,
        }
    }
}

#[async_trait]
pub trait TendermintCommons {
    fn denom_to_ticker(&self, denom: &str) -> Option<String>;

    fn platform_denom(&self) -> &Denom;

    fn set_history_sync_state(&self, new_state: HistorySyncState);

    async fn get_block_timestamp(&self, block: i64) -> MmResult<Option<u64>, TendermintCoinRpcError>;

    async fn get_all_balances(&self) -> MmResult<AllBalancesResult, TendermintCoinRpcError>;

    async fn rpc_client(&self) -> MmResult<HttpClient, TendermintCoinRpcError>;
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TendermintFeeDetails {
    pub coin: String,
    pub amount: BigDecimal,
    #[serde(skip)]
    pub uamount: u64,
    pub gas_limit: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TendermintProtocolInfo {
    pub decimals: u8,
    pub(crate) denom: Denom,
    min_balance_for_ibc_routing: Option<f32>,
    pub account_prefix: String,
    pub chain_id: ChainId,
    gas_price: Option<f64>,
    /// Key represents the account prefix of the target chain and
    /// the value is the channel ID used for sending transactions.
    #[serde(default)]
    ibc_channels: HashMap<String, ChannelId>,
}

#[derive(Clone)]
pub struct ActivatedTokenInfo {
    pub(crate) decimals: u8,
    pub ticker: String,
}

pub struct TendermintConf {
    avg_blocktime: u8,
    /// Derivation path of the coin.
    /// This derivation path consists of `purpose` and `coin_type` only
    /// where the full `BIP44` address has the following structure:
    /// `m/purpose'/coin_type'/account'/change/address_index`.
    derivation_path: Option<HDPathToCoin>,
}

impl TendermintConf {
    pub fn try_from_json(ticker: &str, conf: &Json) -> MmResult<Self, TendermintInitError> {
        let avg_blocktime = conf.get("avg_blocktime").or_mm_err(|| TendermintInitError {
            ticker: ticker.to_string(),
            kind: TendermintInitErrorKind::AvgBlockTimeMissing,
        })?;

        let avg_blocktime = avg_blocktime.as_i64().or_mm_err(|| TendermintInitError {
            ticker: ticker.to_string(),
            kind: TendermintInitErrorKind::AvgBlockTimeInvalid,
        })?;

        let avg_blocktime = u8::try_from(avg_blocktime).map_to_mm(|_| TendermintInitError {
            ticker: ticker.to_string(),
            kind: TendermintInitErrorKind::AvgBlockTimeInvalid,
        })?;

        let derivation_path = json::from_value(conf["derivation_path"].clone()).map_to_mm(|e| TendermintInitError {
            ticker: ticker.to_string(),
            kind: TendermintInitErrorKind::ErrorDeserializingDerivationPath(e.to_string()),
        })?;

        Ok(TendermintConf {
            avg_blocktime,
            derivation_path,
        })
    }
}

pub enum TendermintActivationPolicy {
    PrivateKey(PrivKeyPolicy<TendermintKeyPair>),
    PublicKey(PublicKey),
}

impl TendermintActivationPolicy {
    pub fn with_private_key_policy(private_key_policy: PrivKeyPolicy<TendermintKeyPair>) -> Self {
        Self::PrivateKey(private_key_policy)
    }

    pub fn with_public_key(account_public_key: PublicKey) -> Self {
        Self::PublicKey(account_public_key)
    }

    fn generate_account_id(&self, account_prefix: &str) -> Result<AccountId, ErrorReport> {
        match self {
            Self::PrivateKey(priv_key_policy) => {
                let pk = priv_key_policy.activated_key().ok_or_else(|| {
                    ErrorReport::new(io::Error::new(io::ErrorKind::NotFound, "Activated key not found"))
                })?;

                Ok(
                    account_id_from_privkey(pk.private_key_secret.as_slice(), account_prefix)
                        .map_err(|e| ErrorReport::new(io::Error::new(io::ErrorKind::InvalidData, e.to_string())))?,
                )
            },

            Self::PublicKey(account_public_key) => {
                account_id_from_raw_pubkey(account_prefix, &account_public_key.to_bytes())
            },
        }
    }

    fn public_key(&self) -> Result<PublicKey, io::Error> {
        match self {
            Self::PrivateKey(private_key_policy) => match private_key_policy {
                PrivKeyPolicy::Iguana(pair) => PublicKey::from_raw_secp256k1(&pair.public_key.to_bytes())
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Couldn't generate public key")),

                PrivKeyPolicy::HDWallet { activated_key, .. } => {
                    PublicKey::from_raw_secp256k1(&activated_key.public_key.to_bytes())
                        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Couldn't generate public key"))
                },
                PrivKeyPolicy::Trezor => Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Trezor is not supported yet!",
                )),
                PrivKeyPolicy::WalletConnect { public_key, .. } => PublicKey::from_raw_secp256k1(public_key.as_bytes())
                    .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Couldn't generate public key")),
                #[cfg(target_arch = "wasm32")]
                PrivKeyPolicy::Metamask(_) => Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "Metamask is not supported yet!",
                )),
            },
            Self::PublicKey(account_public_key) => Ok(*account_public_key),
        }
    }

    pub(crate) fn activated_key_or_err(&self) -> Result<&Secp256k1Secret, MmError<PrivKeyPolicyNotAllowed>> {
        match self {
            Self::PrivateKey(private_key) => Ok(private_key.activated_key_or_err()?.private_key_secret.as_ref()),
            Self::PublicKey(_) => MmError::err(PrivKeyPolicyNotAllowed::UnsupportedMethod(
                "`activated_key_or_err` is not supported for pubkey-only activations".to_string(),
            )),
        }
    }

    pub(crate) fn activated_key(&self) -> Option<Secp256k1Secret> {
        match self {
            Self::PrivateKey(private_key) => Some(*private_key.activated_key()?.private_key_secret.as_ref()),
            Self::PublicKey(_) => None,
        }
    }

    pub(crate) fn path_to_coin_or_err(&self) -> Result<&HDPathToCoin, MmError<PrivKeyPolicyNotAllowed>> {
        match self {
            Self::PrivateKey(private_key) => Ok(private_key.path_to_coin_or_err()?),
            Self::PublicKey(_) => MmError::err(PrivKeyPolicyNotAllowed::UnsupportedMethod(
                "`path_to_coin_or_err` is not supported for pubkey-only activations".to_string(),
            )),
        }
    }

    pub(crate) fn hd_wallet_derived_priv_key_or_err(
        &self,
        path_to_address: &DerivationPath,
    ) -> Result<Secp256k1Secret, MmError<PrivKeyPolicyNotAllowed>> {
        match self {
            Self::PrivateKey(pair) => pair.hd_wallet_derived_priv_key_or_err(path_to_address),
            Self::PublicKey(_) => MmError::err(PrivKeyPolicyNotAllowed::UnsupportedMethod(
                "`hd_wallet_derived_priv_key_or_err` is not supported for pubkey-only activations".to_string(),
            )),
        }
    }
}

struct TendermintRpcClient(AsyncMutex<TendermintRpcClientImpl>);

struct TendermintRpcClientImpl {
    rpc_clients: Vec<HttpClient>,
}

#[async_trait]
impl RpcCommonOps for TendermintCoin {
    type RpcClient = HttpClient;
    type Error = TendermintCoinRpcError;

    async fn get_live_client(&self) -> Result<Self::RpcClient, Self::Error> {
        let mut client_impl = self.client.0.lock().await;
        // try to find first live client
        for (i, client) in client_impl.rpc_clients.clone().into_iter().enumerate() {
            match client.perform(HealthRequest).timeout(Duration::from_secs(15)).await {
                Ok(Ok(_)) => {
                    // Bring the live client to the front of rpc_clients
                    client_impl.rpc_clients.rotate_left(i);
                    return Ok(client);
                },
                Ok(Err(rpc_error)) => {
                    debug!("Could not perform healthcheck on: {:?}. Error: {}", &client, rpc_error);
                },
                Err(timeout_error) => {
                    debug!("Healthcheck timeout exceed on: {:?}. Error: {}", &client, timeout_error);
                },
            };
        }
        return Err(TendermintCoinRpcError::RpcClientError(
            "All the current rpc nodes are unavailable.".to_string(),
        ));
    }
}

#[derive(PartialEq)]
pub enum TendermintWalletConnectionType {
    Wc(kdf_walletconnect::WcTopic),
    WcLedger(kdf_walletconnect::WcTopic),
    KeplrLedger,
    Keplr,
    Native,
}

impl Default for TendermintWalletConnectionType {
    fn default() -> Self {
        Self::Native
    }
}

pub struct TendermintCoinImpl {
    ticker: String,
    /// As seconds
    avg_blocktime: u8,
    /// My address
    pub account_id: AccountId,
    pub activation_policy: TendermintActivationPolicy,
    pub tokens_info: PaMutex<HashMap<String, ActivatedTokenInfo>>,
    /// This spawner is used to spawn coin's related futures that should be aborted on coin deactivation
    /// or on [`MmArc::stop`].
    pub(super) abortable_system: AbortableQueue,
    pub(crate) history_sync_state: Mutex<HistorySyncState>,
    client: TendermintRpcClient,
    pub ctx: MmWeak,
    pub(crate) wallet_type: TendermintWalletConnectionType,
    pub(crate) protocol_info: TendermintProtocolInfo,
}

#[derive(Clone)]
pub struct TendermintCoin(Arc<TendermintCoinImpl>);

impl Deref for TendermintCoin {
    type Target = TendermintCoinImpl;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct TendermintInitError {
    pub ticker: String,
    pub kind: TendermintInitErrorKind,
}

#[derive(Display, Debug, Clone)]
pub enum TendermintInitErrorKind {
    Internal(String),
    InvalidPrivKey(String),
    CouldNotGenerateAccountId(String),
    EmptyRpcUrls,
    RpcClientInitError(String),
    InvalidChainId(String),
    InvalidProtocolData(String),
    InvalidPathToAddress(String),
    #[display(fmt = "'derivation_path' field is not found in config")]
    DerivationPathIsNotSet,
    #[display(fmt = "'account' field is not found in config")]
    AccountIsNotSet,
    #[display(fmt = "'address_index' field is not found in config")]
    AddressIndexIsNotSet,
    #[display(fmt = "Error deserializing 'derivation_path': {_0}")]
    ErrorDeserializingDerivationPath(String),
    #[display(fmt = "Error deserializing 'path_to_address': {_0}")]
    ErrorDeserializingPathToAddress(String),
    PrivKeyPolicyNotAllowed(PrivKeyPolicyNotAllowed),
    RpcError(String),
    #[display(fmt = "avg_blocktime is missing in coin configuration")]
    AvgBlockTimeMissing,
    #[display(fmt = "avg_blocktime must be in-between '0' and '255'.")]
    AvgBlockTimeInvalid,
    BalanceStreamInitError(String),
    #[display(fmt = "Watcher features can not be used with pubkey-only activation policy.")]
    CantUseWatchersWithPubkeyPolicy,
    #[display(fmt = "Unable to fetch account for chain: {_0}")]
    UnableToFetchChainAccount(String),
}

/// TODO: Rename this into `ClientRpcError` because this is very
/// confusing atm.
#[derive(Display, Debug, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum TendermintCoinRpcError {
    Prost(String),
    InvalidResponse(String),
    PerformError(String),
    RpcClientError(String),
    InternalError(String),
    #[display(fmt = "Account type '{prefix}' is not supported for HTLCs")]
    UnexpectedAccountType {
        prefix: String,
    },
    NotFound(String),
}

#[derive(Clone, Debug, Display, PartialEq, Serialize)]
pub enum IBCError {
    #[display(
        fmt = "IBC channel could not be found in coins file for '{address_prefix}' address prefix. Provide it manually by including `ibc_source_channel` in the request."
    )]
    IBCChannelCouldNotBeFound { address_prefix: String },
    #[display(
        fmt = "IBC channel '{channel_id}' is not healthy. Provide a healthy one manually by including `ibc_source_channel` in the request."
    )]
    IBCChannelNotHealthy { channel_id: ChannelId },
    #[display(fmt = "IBC channel '{channel_id}' is not present on the target node.")]
    IBCChannelMissingOnNode { channel_id: ChannelId },
    #[display(fmt = "Transport error: {reason}")]
    Transport { reason: String },
    #[display(fmt = "Internal error: {reason}")]
    InternalError { reason: String },
}

impl From<IBCError> for WithdrawError {
    fn from(err: IBCError) -> Self {
        WithdrawError::IBCError(err)
    }
}

impl From<DecodeError> for TendermintCoinRpcError {
    fn from(err: DecodeError) -> Self {
        TendermintCoinRpcError::Prost(err.to_string())
    }
}

impl From<PrivKeyPolicyNotAllowed> for TendermintCoinRpcError {
    fn from(err: PrivKeyPolicyNotAllowed) -> Self {
        TendermintCoinRpcError::InternalError(err.to_string())
    }
}

impl From<TendermintCoinRpcError> for WithdrawError {
    fn from(err: TendermintCoinRpcError) -> Self {
        WithdrawError::Transport(err.to_string())
    }
}

impl From<TendermintCoinRpcError> for DelegationError {
    fn from(err: TendermintCoinRpcError) -> Self {
        DelegationError::Transport(err.to_string())
    }
}

impl From<TendermintCoinRpcError> for BalanceError {
    fn from(err: TendermintCoinRpcError) -> Self {
        match err {
            TendermintCoinRpcError::InvalidResponse(e) => BalanceError::InvalidResponse(e),
            TendermintCoinRpcError::Prost(e) => BalanceError::InvalidResponse(e),
            TendermintCoinRpcError::PerformError(e)
            | TendermintCoinRpcError::RpcClientError(e)
            | TendermintCoinRpcError::NotFound(e) => BalanceError::Transport(e),
            TendermintCoinRpcError::InternalError(e) => BalanceError::Internal(e),
            TendermintCoinRpcError::UnexpectedAccountType { prefix } => {
                BalanceError::Internal(format!("Account type '{prefix}' is not supported for HTLCs"))
            },
        }
    }
}

impl From<TendermintCoinRpcError> for ValidatePaymentError {
    fn from(err: TendermintCoinRpcError) -> Self {
        match err {
            TendermintCoinRpcError::InvalidResponse(e) => ValidatePaymentError::InvalidRpcResponse(e),
            TendermintCoinRpcError::Prost(e) => ValidatePaymentError::InvalidRpcResponse(e),
            TendermintCoinRpcError::PerformError(e)
            | TendermintCoinRpcError::RpcClientError(e)
            | TendermintCoinRpcError::NotFound(e) => ValidatePaymentError::Transport(e),
            TendermintCoinRpcError::InternalError(e) => ValidatePaymentError::InternalError(e),
            TendermintCoinRpcError::UnexpectedAccountType { prefix } => {
                ValidatePaymentError::InvalidParameter(format!("Account type '{prefix}' is not supported for HTLCs"))
            },
        }
    }
}

impl From<TendermintCoinRpcError> for TradePreimageError {
    fn from(err: TendermintCoinRpcError) -> Self {
        TradePreimageError::Transport(err.to_string())
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl From<tendermint_rpc::Error> for TendermintCoinRpcError {
    fn from(err: tendermint_rpc::Error) -> Self {
        TendermintCoinRpcError::PerformError(err.to_string())
    }
}

#[cfg(target_arch = "wasm32")]
impl From<PerformError> for TendermintCoinRpcError {
    fn from(err: PerformError) -> Self {
        TendermintCoinRpcError::PerformError(err.to_string())
    }
}

impl From<TendermintCoinRpcError> for RawTransactionError {
    fn from(err: TendermintCoinRpcError) -> Self {
        RawTransactionError::Transport(err.to_string())
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CosmosTransaction {
    pub data: cosmrs::proto::cosmos::tx::v1beta1::TxRaw,
}

impl crate::Transaction for CosmosTransaction {
    fn tx_hex(&self) -> Vec<u8> {
        self.data.encode_to_vec()
    }

    fn tx_hash_as_bytes(&self) -> BytesJson {
        let bytes = self.data.encode_to_vec();
        let hash = sha256(&bytes);
        hash.to_vec().into()
    }
}

pub(crate) fn account_id_from_privkey(priv_key: &[u8], prefix: &str) -> MmResult<AccountId, TendermintInitErrorKind> {
    let signing_key =
        SigningKey::from_slice(priv_key).map_to_mm(|e| TendermintInitErrorKind::InvalidPrivKey(e.to_string()))?;

    signing_key
        .public_key()
        .account_id(prefix)
        .map_to_mm(|e| TendermintInitErrorKind::CouldNotGenerateAccountId(e.to_string()))
}

#[derive(Display, Debug)]
pub enum AccountIdFromPubkeyHexErr {
    InvalidHexString(FromHexError),
    CouldNotCreateAccountId(ErrorReport),
}

impl From<FromHexError> for AccountIdFromPubkeyHexErr {
    fn from(err: FromHexError) -> Self {
        AccountIdFromPubkeyHexErr::InvalidHexString(err)
    }
}

impl From<ErrorReport> for AccountIdFromPubkeyHexErr {
    fn from(err: ErrorReport) -> Self {
        AccountIdFromPubkeyHexErr::CouldNotCreateAccountId(err)
    }
}

pub fn account_id_from_pubkey_hex(prefix: &str, pubkey: &str) -> Result<AccountId, AccountIdFromPubkeyHexErr> {
    let pubkey_bytes = hex::decode(pubkey)?;
    Ok(account_id_from_raw_pubkey(prefix, &pubkey_bytes)?)
}

pub fn account_id_from_raw_pubkey(prefix: &str, pubkey: &[u8]) -> Result<AccountId, ErrorReport> {
    let pubkey_hash = dhash160(pubkey);
    AccountId::new(prefix, pubkey_hash.as_slice())
}

#[derive(Debug, Clone, PartialEq)]
pub struct AllBalancesResult {
    pub platform_balance: BigDecimal,
    pub tokens_balances: HashMap<String, BigDecimal>,
}

#[derive(Debug, Display)]
enum SearchForSwapTxSpendErr {
    Cosmrs(ErrorReport),
    Rpc(TendermintCoinRpcError),
    TxMessagesEmpty,
    ClaimHtlcTxNotFound,
    UnexpectedHtlcState(i32),
    #[display(fmt = "Account type '{prefix}' is not supported for HTLCs")]
    UnexpectedAccountType {
        prefix: String,
    },
    Proto(DecodeError),
}

impl From<ErrorReport> for SearchForSwapTxSpendErr {
    fn from(e: ErrorReport) -> Self {
        SearchForSwapTxSpendErr::Cosmrs(e)
    }
}

impl From<TendermintCoinRpcError> for SearchForSwapTxSpendErr {
    fn from(e: TendermintCoinRpcError) -> Self {
        SearchForSwapTxSpendErr::Rpc(e)
    }
}

impl From<DecodeError> for SearchForSwapTxSpendErr {
    fn from(e: DecodeError) -> Self {
        SearchForSwapTxSpendErr::Proto(e)
    }
}

#[async_trait]
impl TendermintCommons for TendermintCoin {
    fn platform_denom(&self) -> &Denom {
        &self.protocol_info.denom
    }

    fn set_history_sync_state(&self, new_state: HistorySyncState) {
        *self.history_sync_state.lock().unwrap() = new_state;
    }

    async fn get_block_timestamp(&self, block: i64) -> MmResult<Option<u64>, TendermintCoinRpcError> {
        let block_response = self.get_block_by_height(block).await?;
        let block_header = some_or_return_ok_none!(some_or_return_ok_none!(block_response.block).header);
        let timestamp = some_or_return_ok_none!(block_header.time);

        Ok(u64::try_from(timestamp.seconds).ok())
    }

    fn denom_to_ticker(&self, denom: &str) -> Option<String> {
        if self.protocol_info.denom.as_ref() == denom {
            return Some(self.ticker.clone());
        }

        let ctx = MmArc::from_weak(&self.ctx)?;

        ctx.conf["coins"].as_array()?.iter().find_map(|coin| {
            coin["protocol"]["protocol_data"]["denom"]
                .as_str()
                .filter(|&d| d.to_lowercase() == denom.to_lowercase())
                .and_then(|_| coin["coin"].as_str().map(|s| s.to_owned()))
        })
    }

    async fn get_all_balances(&self) -> MmResult<AllBalancesResult, TendermintCoinRpcError> {
        let platform_balance_denom = self
            .account_balance_for_denom(&self.account_id, self.protocol_info.denom.to_string())
            .await?;
        let platform_balance = big_decimal_from_sat_unsigned(platform_balance_denom, self.protocol_info.decimals);
        let ibc_assets_info = self.tokens_info.lock().clone();

        let mut requests = Vec::with_capacity(ibc_assets_info.len());
        for (denom, info) in ibc_assets_info {
            let fut = async move {
                let balance_denom = self
                    .account_balance_for_denom(&self.account_id, denom)
                    .await
                    .map_err(|e| e.into_inner())?;
                let balance_decimal = big_decimal_from_sat_unsigned(balance_denom, info.decimals);
                Ok::<_, TendermintCoinRpcError>((info.ticker, balance_decimal))
            };
            requests.push(fut);
        }
        let tokens_balances = try_join_all(requests).await?.into_iter().collect();

        Ok(AllBalancesResult {
            platform_balance,
            tokens_balances,
        })
    }

    #[inline(always)]
    async fn rpc_client(&self) -> MmResult<HttpClient, TendermintCoinRpcError> {
        self.get_live_client().await.map_to_mm(|e| e)
    }
}

#[cfg_attr(test, mockable)]
impl TendermintCoin {
    #[allow(clippy::too_many_arguments)]
    pub async fn init(
        ctx: &MmArc,
        ticker: String,
        conf: TendermintConf,
        protocol_info: TendermintProtocolInfo,
        nodes: Vec<RpcNode>,
        tx_history: bool,
        activation_policy: TendermintActivationPolicy,
        wallet_type: TendermintWalletConnectionType,
    ) -> MmResult<TendermintCoin, TendermintInitError> {
        if nodes.is_empty() {
            return MmError::err(TendermintInitError {
                ticker,
                kind: TendermintInitErrorKind::EmptyRpcUrls,
            });
        }

        let account_id = activation_policy
            .generate_account_id(&protocol_info.account_prefix)
            .map_to_mm(|e| TendermintInitError {
                ticker: ticker.clone(),
                kind: TendermintInitErrorKind::CouldNotGenerateAccountId(e.to_string()),
            })?;

        let rpc_clients = clients_from_urls(ctx, nodes).mm_err(|kind| TendermintInitError {
            ticker: ticker.clone(),
            kind,
        })?;

        let client_impl = TendermintRpcClientImpl { rpc_clients };

        let history_sync_state = if tx_history {
            HistorySyncState::NotStarted
        } else {
            HistorySyncState::NotEnabled
        };

        // Create an abortable system linked to the `MmCtx` so if the context is stopped via `MmArc::stop`,
        // all spawned futures related to `TendermintCoin` will be aborted as well.
        let abortable_system = ctx
            .abortable_system
            .create_subsystem()
            .map_to_mm(|e| TendermintInitError {
                ticker: ticker.clone(),
                kind: TendermintInitErrorKind::Internal(e.to_string()),
            })?;

        Ok(TendermintCoin(Arc::new(TendermintCoinImpl {
            ticker,
            account_id,
            activation_policy,
            avg_blocktime: conf.avg_blocktime,
            tokens_info: PaMutex::new(HashMap::new()),
            abortable_system,
            history_sync_state: Mutex::new(history_sync_state),
            client: TendermintRpcClient(AsyncMutex::new(client_impl)),
            protocol_info,
            ctx: ctx.weak(),
            wallet_type,
        })))
    }

    /// Finds the IBC channel by querying the given channel ID and port ID
    /// and returns its information.
    async fn query_ibc_channel(
        &self,
        channel_id: ChannelId,
        port_id: &str,
    ) -> Result<ibc::core::channel::v1::Channel, IBCError> {
        let payload = QueryChannelRequest {
            channel_id: channel_id.to_string(),
            port_id: port_id.to_string(),
        }
        .encode_to_vec();

        let request = AbciRequest::new(
            Some(ABCI_IBC_CHANNEL_QUERY_PATH.to_string()),
            payload,
            ABCI_REQUEST_HEIGHT,
            ABCI_REQUEST_PROVE,
        );

        let response = self
            .rpc_client()
            .await
            .map_err(|e| IBCError::Transport { reason: e.to_string() })?
            .perform(request)
            .await
            .map_err(|e| IBCError::Transport { reason: e.to_string() })?;

        let response = QueryChannelResponse::decode(response.response.value.as_slice())
            .map_err(|e| IBCError::InternalError { reason: e.to_string() })?;

        response.channel.ok_or(IBCError::IBCChannelMissingOnNode { channel_id })
    }

    /// Looks for a healthy IBC channel on a network that supports HTLC transactions.
    /// Right now it first tries to find a channel on IRIS network, if none is found, then falls
    /// back to NUCLEUS network.
    pub async fn get_healthy_ibc_channel_to_htlc_chain(&self) -> Result<ChannelId, MmError<IBCError>> {
        let channel_id = if let Ok(channel_id) = self.get_healthy_ibc_channel_for_address_prefix(IRIS_PREFIX).await {
            channel_id
        } else {
            self.get_healthy_ibc_channel_for_address_prefix(NUCLEUS_PREFIX).await?
        };

        Ok(channel_id)
    }

    /// Returns a **healthy** IBC channel ID for the given target address.
    pub async fn get_healthy_ibc_channel_for_address_prefix(
        &self,
        address_prefix: &str,
    ) -> Result<ChannelId, MmError<IBCError>> {
        // ref: https://github.com/cosmos/ibc-go/blob/7f34724b982581435441e0bb70598c3e3a77f061/proto/ibc/core/channel/v1/channel.proto#L51-L68
        const STATE_OPEN: i32 = 3;

        let channel_id = *self.protocol_info.ibc_channels.get(address_prefix).ok_or_else(|| {
            IBCError::IBCChannelCouldNotBeFound {
                address_prefix: address_prefix.to_owned(),
            }
        })?;

        let channel = self.query_ibc_channel(channel_id, "transfer").await?;

        // TODO: Extend the validation logic to also include:
        //
        //   - Checking the time of the last update on the channel
        //   - Verifying the total amount transferred since the channel was created
        //   - Check the channel creation time
        if channel.state != STATE_OPEN {
            return MmError::err(IBCError::IBCChannelNotHealthy { channel_id });
        }

        Ok(channel_id)
    }

    pub fn supports_htlc(&self) -> bool {
        matches!(self.protocol_info.account_prefix.as_str(), NUCLEUS_PREFIX | IRIS_PREFIX)
    }

    #[inline(always)]
    fn gas_price(&self) -> f64 {
        self.protocol_info.gas_price.unwrap_or(DEFAULT_GAS_PRICE)
    }

    #[allow(unused)]
    async fn get_latest_block(&self) -> MmResult<GetLatestBlockResponse, TendermintCoinRpcError> {
        let request = GetLatestBlockRequest {};
        let request = AbciRequest::new(
            Some(ABCI_GET_LATEST_BLOCK_PATH.to_string()),
            request.encode_to_vec(),
            ABCI_REQUEST_HEIGHT,
            ABCI_REQUEST_PROVE,
        );

        let response = self.rpc_client().await?.perform(request).await?;

        Ok(GetLatestBlockResponse::decode(response.response.value.as_slice())?)
    }

    #[allow(unused)]
    async fn get_block_by_height(&self, height: i64) -> MmResult<GetBlockByHeightResponse, TendermintCoinRpcError> {
        let request = GetBlockByHeightRequest { height };
        let request = AbciRequest::new(
            Some(ABCI_GET_BLOCK_BY_HEIGHT_PATH.to_string()),
            request.encode_to_vec(),
            ABCI_REQUEST_HEIGHT,
            ABCI_REQUEST_PROVE,
        );

        let response = self.rpc_client().await?.perform(request).await?;

        Ok(GetBlockByHeightResponse::decode(response.response.value.as_slice())?)
    }

    // We must simulate the tx on rpc nodes in order to calculate network fee.
    // Right now cosmos doesn't expose any of gas price and fee informations directly.
    // Therefore, we can call SimulateRequest or CheckTx(doesn't work with using Abci interface) to get used gas or fee itself.
    pub(super) fn gen_simulated_tx(
        &self,
        account_info: &BaseAccount,
        priv_key: &Secp256k1Secret,
        tx_payload: Any,
        timeout_height: u64,
        memo: &str,
    ) -> cosmrs::Result<Vec<u8>> {
        let fee_amount = Coin {
            denom: self.protocol_info.denom.clone(),
            amount: 0_u64.into(),
        };

        let fee = Fee::from_amount_and_gas(fee_amount, GAS_LIMIT_DEFAULT);

        let signkey = SigningKey::from_slice(priv_key.as_slice())?;
        let tx_body = tx::Body::new(vec![tx_payload], memo, timeout_height as u32);
        let auth_info = SignerInfo::single_direct(Some(signkey.public_key()), account_info.sequence).auth_info(fee);
        let sign_doc = SignDoc::new(
            &tx_body,
            &auth_info,
            &self.protocol_info.chain_id,
            account_info.account_number,
        )?;
        sign_doc.sign(&signkey)?.to_bytes()
    }

    /// This is converted from irismod and cosmos-sdk source codes written in golang.
    /// Refs:
    ///  - Main algorithm: https://github.com/irisnet/irismod/blob/main/modules/htlc/types/htlc.go#L157
    ///  - Coins string building https://github.com/cosmos/cosmos-sdk/blob/main/types/coin.go#L210-L225
    fn calculate_htlc_id(
        &self,
        from_address: &AccountId,
        to_address: &AccountId,
        amount: &[Coin],
        secret_hash: &[u8],
    ) -> String {
        // Needs to be sorted if contains multiple coins
        // let mut amount = amount;
        // amount.sort();

        let coins_string = amount
            .iter()
            .map(|t| format!("{}{}", t.amount, t.denom))
            .collect::<Vec<String>>()
            .join(",");

        let mut htlc_id = vec![];
        htlc_id.extend_from_slice(secret_hash);
        htlc_id.extend_from_slice(&from_address.to_bytes());
        htlc_id.extend_from_slice(&to_address.to_bytes());
        htlc_id.extend_from_slice(coins_string.as_bytes());
        sha256(&htlc_id).to_string().to_uppercase()
    }

    async fn common_send_raw_tx_bytes(
        &self,
        tx_payload: Any,
        fee: Fee,
        timeout_height: u64,
        memo: &str,
        timeout: Duration,
    ) -> Result<(String, Raw), TransactionErr> {
        // As there wouldn't be enough time to process the data, to mitigate potential edge problems (such as attempting to send transaction
        // bytes half a second before expiration, which may take longer to send and result in the transaction amount being wasted due to a timeout),
        // reduce the expiration time by 5 seconds.
        let expiration = timeout - Duration::from_secs(5);

        match self.activation_policy {
            TendermintActivationPolicy::PrivateKey(_) => {
                try_tx_s!(
                    self.seq_safe_send_raw_tx_bytes(tx_payload, fee, timeout_height, memo)
                        .timeout(expiration)
                        .await
                )
            },
            TendermintActivationPolicy::PublicKey(_) => {
                if self.is_wallet_connect() {
                    return try_tx_s!(
                        self.seq_safe_send_raw_tx_bytes(tx_payload, fee, timeout_height, memo)
                            .timeout(expiration)
                            .await
                    );
                };

                try_tx_s!(
                    self.send_unsigned_tx_externally(tx_payload, fee, timeout_height, memo, expiration)
                        .timeout(expiration)
                        .await
                )
            },
        }
    }

    async fn get_tx_raw(
        &self,
        account_info: &BaseAccount,
        tx_payload: Any,
        fee: Fee,
        timeout_height: u64,
        memo: &str,
    ) -> Result<Raw, TransactionErr> {
        if self.is_wallet_connect() {
            let ctx = try_tx_s!(MmArc::from_weak(&self.ctx).ok_or(ERRL!("ctx must be initialized already")));
            let wc = try_tx_s!(WalletConnectCtx::from_ctx(&ctx).map_err(|e| e.to_string()));
            let SerializedUnsignedTx { tx_json, .. } = if self.is_ledger_connection() {
                try_tx_s!(self.any_to_legacy_amino_json(account_info, tx_payload, fee, timeout_height, memo))
            } else {
                try_tx_s!(self.any_to_serialized_sign_doc(account_info, tx_payload, fee, timeout_height, memo))
            };

            return Ok(try_tx_s!(self.wc_sign_tx(&wc, tx_json).await.map_err(|err| err.to_string())).into());
        }

        let tx_raw = try_tx_s!(self.any_to_signed_raw_tx(
            try_tx_s!(self.activation_policy.activated_key_or_err()),
            account_info,
            tx_payload,
            fee,
            timeout_height,
            memo,
        ));

        Ok(tx_raw)
    }

    async fn seq_safe_send_raw_tx_bytes(
        &self,
        tx_payload: Any,
        fee: Fee,
        timeout_height: u64,
        memo: &str,
    ) -> Result<(String, Raw), TransactionErr> {
        let mut account_info = try_tx_s!(self.account_info(&self.account_id).await);
        loop {
            let tx_raw = try_tx_s!(
                self.get_tx_raw(&account_info, tx_payload.clone(), fee.clone(), timeout_height, memo,)
                    .await
            );

            // Attempt to send the transaction bytes
            match self.send_raw_tx_bytes(try_tx_s!(&tx_raw.to_bytes())).compat().await {
                Ok(tx_id) => {
                    return Ok((tx_id, tx_raw));
                },
                Err(e) => {
                    // Handle sequence number mismatch and retry
                    if e.contains(ACCOUNT_SEQUENCE_ERR) {
                        account_info.sequence = try_tx_s!(parse_expected_sequence_number(&e));
                        debug!("Account sequence mismatch, retrying...");
                        continue;
                    }

                    return Err(TransactionErr::Plain(ERRL!("Transaction failed: {}", e)));
                },
            }
        }
    }

    async fn send_unsigned_tx_externally(
        &self,
        tx_payload: Any,
        fee: Fee,
        timeout_height: u64,
        memo: &str,
        timeout: Duration,
    ) -> Result<(String, Raw), TransactionErr> {
        #[derive(Deserialize)]
        struct TxHashData {
            hash: String,
        }

        let ctx = try_tx_s!(MmArc::from_weak(&self.ctx).ok_or(ERRL!("ctx must be initialized already")));

        let account_info = try_tx_s!(self.account_info(&self.account_id).await);
        let SerializedUnsignedTx { tx_json, body_bytes } = if self.is_ledger_connection() {
            try_tx_s!(self.any_to_legacy_amino_json(&account_info, tx_payload, fee, timeout_height, memo))
        } else {
            try_tx_s!(self.any_to_serialized_sign_doc(&account_info, tx_payload, fee, timeout_height, memo))
        };

        let data: TxHashData = try_tx_s!(ctx
            .ask_for_data(&format!("TX_HASH:{}", self.ticker()), tx_json.clone(), timeout)
            .await
            .map_err(|e| ERRL!("{}", e)));

        let tx = try_tx_s!(self.request_tx(data.hash.clone()).await.map_err(|e| ERRL!("{}", e)));

        let tx_raw = TxRaw {
            body_bytes: tx.body.as_ref().map(Message::encode_to_vec).unwrap_or_default(),
            auth_info_bytes: tx.auth_info.as_ref().map(Message::encode_to_vec).unwrap_or_default(),
            signatures: tx.signatures,
        };

        if body_bytes != tx_raw.body_bytes {
            return Err(crate::TransactionErr::Plain(ERRL!(
                "Unsigned transaction don't match with the externally provided transaction."
            )));
        }

        Ok((data.hash, Raw::from(tx_raw)))
    }

    #[allow(deprecated)]
    pub(super) async fn calculate_fee(
        &self,
        msg: Any,
        timeout_height: u64,
        memo: &str,
        withdraw_fee: Option<WithdrawFee>,
    ) -> MmResult<Fee, TendermintCoinRpcError> {
        let activated_priv_key = if let Ok(activated_priv_key) = self.activation_policy.activated_key_or_err() {
            activated_priv_key
        } else {
            let (gas_price, gas_limit) = self.gas_info_for_withdraw(&withdraw_fee, GAS_LIMIT_DEFAULT);
            let amount = ((GAS_WANTED_BASE_VALUE * 1.5) * gas_price).ceil();

            let fee_amount = Coin {
                denom: self.platform_denom().clone(),
                amount: (amount as u64).into(),
            };

            return Ok(Fee::from_amount_and_gas(fee_amount, gas_limit));
        };

        let mut account_info = self.account_info(&self.account_id).await?;
        let (response, raw_response) = loop {
            let tx_bytes = self
                .gen_simulated_tx(&account_info, activated_priv_key, msg.clone(), timeout_height, memo)
                .map_to_mm(|e| TendermintCoinRpcError::InternalError(format!("{e}")))?;

            let request = AbciRequest::new(
                Some(ABCI_SIMULATE_TX_PATH.to_string()),
                SimulateRequest { tx_bytes, tx: None }.encode_to_vec(),
                ABCI_REQUEST_HEIGHT,
                ABCI_REQUEST_PROVE,
            );

            let raw_response = self.rpc_client().await?.perform(request).await?;

            let log = raw_response.response.log.to_string();
            if log.contains(ACCOUNT_SEQUENCE_ERR) {
                account_info.sequence = parse_expected_sequence_number(&log)?;
                debug!("Got wrong account sequence, trying again.");
                continue;
            }

            match raw_response.response.code {
                cosmrs::tendermint::abci::Code::Ok => {},
                cosmrs::tendermint::abci::Code::Err(ecode) => {
                    return MmError::err(TendermintCoinRpcError::InvalidResponse(format!(
                        "Could not read gas_info. Error code: {} Message: {}",
                        ecode, raw_response.response.log
                    )));
                },
            };

            break (
                SimulateResponse::decode(raw_response.response.value.as_slice())?,
                raw_response,
            );
        };

        let gas = response.gas_info.as_ref().ok_or_else(|| {
            TendermintCoinRpcError::InvalidResponse(format!(
                "Could not read gas_info. Invalid Response: {raw_response:?}"
            ))
        })?;

        let (gas_price, gas_limit) = self.gas_info_for_withdraw(&withdraw_fee, GAS_LIMIT_DEFAULT);

        let amount = ((gas.gas_used as f64 * 1.5) * gas_price).ceil();

        let fee_amount = Coin {
            denom: self.platform_denom().clone(),
            amount: (amount as u64).into(),
        };

        Ok(Fee::from_amount_and_gas(fee_amount, gas_limit))
    }

    #[allow(deprecated)]
    pub(super) async fn calculate_account_fee_amount_as_u64(
        &self,
        account_id: &AccountId,
        priv_key: Option<Secp256k1Secret>,
        msg: Any,
        timeout_height: u64,
        memo: &str,
        withdraw_fee: Option<WithdrawFee>,
    ) -> MmResult<u64, TendermintCoinRpcError> {
        let priv_key = if let Some(priv_key) = priv_key {
            priv_key
        } else {
            let (gas_price, _) = self.gas_info_for_withdraw(&withdraw_fee, 0);
            return Ok(((GAS_WANTED_BASE_VALUE * 1.5) * gas_price).ceil() as u64);
        };

        let mut account_info = self.account_info(account_id).await?;
        let (response, raw_response) = loop {
            let tx_bytes = self
                .gen_simulated_tx(&account_info, &priv_key, msg.clone(), timeout_height, memo)
                .map_to_mm(|e| TendermintCoinRpcError::InternalError(format!("{e}")))?;

            let request = AbciRequest::new(
                Some(ABCI_SIMULATE_TX_PATH.to_string()),
                SimulateRequest { tx_bytes, tx: None }.encode_to_vec(),
                ABCI_REQUEST_HEIGHT,
                ABCI_REQUEST_PROVE,
            );

            let raw_response = self.rpc_client().await?.perform(request).await?;

            let log = raw_response.response.log.to_string();
            if log.contains(ACCOUNT_SEQUENCE_ERR) {
                account_info.sequence = parse_expected_sequence_number(&log)?;
                debug!("Got wrong account sequence, trying again.");
                continue;
            }

            match raw_response.response.code {
                cosmrs::tendermint::abci::Code::Ok => {},
                cosmrs::tendermint::abci::Code::Err(ecode) => {
                    return MmError::err(TendermintCoinRpcError::InvalidResponse(format!(
                        "Could not read gas_info. Error code: {} Message: {}",
                        ecode, raw_response.response.log
                    )));
                },
            };

            break (
                SimulateResponse::decode(raw_response.response.value.as_slice())?,
                raw_response,
            );
        };

        let gas = response.gas_info.as_ref().ok_or_else(|| {
            TendermintCoinRpcError::InvalidResponse(format!(
                "Could not read gas_info. Invalid Response: {raw_response:?}"
            ))
        })?;

        let (gas_price, _) = self.gas_info_for_withdraw(&withdraw_fee, 0);

        Ok(((gas.gas_used as f64 * 1.5) * gas_price).ceil() as u64)
    }

    pub(super) async fn account_info(&self, account_id: &AccountId) -> MmResult<BaseAccount, TendermintCoinRpcError> {
        let request = QueryAccountRequest {
            address: account_id.to_string(),
        };
        let request = AbciRequest::new(
            Some(ABCI_QUERY_ACCOUNT_PATH.to_string()),
            request.encode_to_vec(),
            ABCI_REQUEST_HEIGHT,
            ABCI_REQUEST_PROVE,
        );

        let response = self.rpc_client().await?.perform(request).await?;
        let account_response = QueryAccountResponse::decode(response.response.value.as_slice())?;
        let account = account_response
            .account
            .or_mm_err(|| TendermintCoinRpcError::InvalidResponse("Account is None".into()))?;

        let account_prefix = self.protocol_info.account_prefix.clone();
        let base_account = match BaseAccount::decode(account.value.as_slice()) {
            Ok(account) => account,
            Err(err) if account_prefix.as_str() == IRIS_PREFIX => {
                let ethermint_account = EthermintAccount::decode(account.value.as_slice())?;

                ethermint_account
                    .base_account
                    .or_mm_err(|| TendermintCoinRpcError::Prost(err.to_string()))?
            },
            Err(err) => {
                return MmError::err(TendermintCoinRpcError::Prost(err.to_string()));
            },
        };

        Ok(base_account)
    }

    pub(super) async fn account_balance_for_denom(
        &self,
        account_id: &AccountId,
        denom: String,
    ) -> MmResult<u64, TendermintCoinRpcError> {
        let request = QueryBalanceRequest {
            address: account_id.to_string(),
            denom,
        };
        let request = AbciRequest::new(
            Some(ABCI_QUERY_BALANCE_PATH.to_string()),
            request.encode_to_vec(),
            ABCI_REQUEST_HEIGHT,
            ABCI_REQUEST_PROVE,
        );

        let response = self.rpc_client().await?.perform(request).await?;
        let response = QueryBalanceResponse::decode(response.response.value.as_slice())?;
        response
            .balance
            .or_mm_err(|| TendermintCoinRpcError::InvalidResponse("balance is None".into()))?
            .amount
            .parse()
            .map_to_mm(|e| TendermintCoinRpcError::InvalidResponse(format!("balance is not u64, err {e}")))
    }

    pub(super) fn extract_account_id_and_private_key(
        &self,
        withdraw_from: Option<HDAddressSelector>,
    ) -> Result<(AccountId, Option<H256>), io::Error> {
        if let TendermintActivationPolicy::PublicKey(_) = self.activation_policy {
            return Ok((self.account_id.clone(), None));
        }

        match withdraw_from {
            Some(from) => {
                let path_to_coin = self
                    .activation_policy
                    .path_to_coin_or_err()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

                let path_to_address = from
                    .to_address_path(path_to_coin.coin_type())
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?
                    .to_derivation_path(path_to_coin)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

                let priv_key = self
                    .activation_policy
                    .hd_wallet_derived_priv_key_or_err(&path_to_address)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

                let account_id = account_id_from_privkey(priv_key.as_slice(), &self.protocol_info.account_prefix)
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;
                Ok((account_id, Some(priv_key)))
            },
            None => {
                let activated_key = self
                    .activation_policy
                    .activated_key_or_err()
                    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

                Ok((self.account_id.clone(), Some(*activated_key)))
            },
        }
    }

    pub(super) async fn any_to_transaction_data(
        &self,
        maybe_priv_key: Option<H256>,
        message: Any,
        account_info: &BaseAccount,
        fee: Fee,
        timeout_height: u64,
        memo: &str,
    ) -> Result<TransactionData, ErrorReport> {
        if let Some(priv_key) = maybe_priv_key {
            let tx_raw = self.any_to_signed_raw_tx(&priv_key, account_info, message, fee, timeout_height, memo)?;
            let tx_bytes = tx_raw.to_bytes()?;
            let hash = sha256(&tx_bytes);

            return Ok(TransactionData::new_signed(
                tx_bytes.into(),
                hex::encode_upper(hash.as_slice()),
            ));
        };

        let SerializedUnsignedTx { tx_json, .. } = if self.is_ledger_connection() {
            self.any_to_legacy_amino_json(account_info, message, fee, timeout_height, memo)
        } else {
            self.any_to_serialized_sign_doc(account_info, message, fee, timeout_height, memo)
        }?;

        if self.is_wallet_connect() {
            let ctx = MmArc::from_weak(&self.ctx)
                .ok_or(MyAddressError::InternalError(ERRL!("ctx must be initialized already")))?;
            let wallet_connect = WalletConnectCtx::from_ctx(&ctx)?;

            let tx_raw: Raw = self.wc_sign_tx(&wallet_connect, tx_json).await?.into();
            let tx_bytes = tx_raw.to_bytes()?;
            let hash = sha256(&tx_bytes);

            return Ok(TransactionData::new_signed(
                tx_bytes.into(),
                hex::encode_upper(hash.as_slice()),
            ));
        };

        Ok(TransactionData::Unsigned(tx_json))
    }

    fn gen_create_htlc_tx(
        &self,
        denom: Denom,
        to: &AccountId,
        amount: cosmrs::Amount,
        secret_hash: &[u8],
        time_lock: u64,
    ) -> MmResult<TendermintHtlc, TxMarshalingErr> {
        let amount = vec![Coin { denom, amount }];
        let timestamp = 0_u64;

        let htlc_type = HtlcType::from_str(&self.protocol_info.account_prefix).map_err(|_| {
            TxMarshalingErr::NotSupported(format!(
                "Account type '{}' is not supported for HTLCs",
                self.protocol_info.account_prefix
            ))
        })?;

        let msg_payload = CreateHtlcMsg::new(
            htlc_type,
            self.account_id.clone(),
            to.clone(),
            amount.clone(),
            hex::encode(secret_hash),
            timestamp,
            time_lock,
        );

        let htlc_id = self.calculate_htlc_id(&self.account_id, to, &amount, secret_hash);

        Ok(TendermintHtlc {
            id: htlc_id,
            msg_payload: msg_payload
                .to_any()
                .map_err(|e| MmError::new(TxMarshalingErr::InvalidInput(e.to_string())))?,
        })
    }

    fn gen_claim_htlc_tx(&self, htlc_id: String, secret: &[u8]) -> MmResult<TendermintHtlc, TxMarshalingErr> {
        let htlc_type = HtlcType::from_str(&self.protocol_info.account_prefix).map_err(|_| {
            TxMarshalingErr::NotSupported(format!(
                "Account type '{}' is not supported for HTLCs",
                self.protocol_info.account_prefix
            ))
        })?;

        let msg_payload = ClaimHtlcMsg::new(htlc_type, htlc_id.clone(), self.account_id.clone(), hex::encode(secret));

        Ok(TendermintHtlc {
            id: htlc_id,
            msg_payload: msg_payload
                .to_any()
                .map_err(|e| MmError::new(TxMarshalingErr::InvalidInput(e.to_string())))?,
        })
    }

    pub(super) fn any_to_signed_raw_tx(
        &self,
        priv_key: &Secp256k1Secret,
        account_info: &BaseAccount,
        tx_payload: Any,
        fee: Fee,
        timeout_height: u64,
        memo: &str,
    ) -> cosmrs::Result<Raw> {
        let signkey = SigningKey::from_slice(priv_key.as_slice())?;
        let tx_body = tx::Body::new(vec![tx_payload], memo, timeout_height as u32);
        let auth_info = SignerInfo::single_direct(Some(signkey.public_key()), account_info.sequence).auth_info(fee);
        let sign_doc = SignDoc::new(
            &tx_body,
            &auth_info,
            &self.protocol_info.chain_id,
            account_info.account_number,
        )?;
        sign_doc.sign(&signkey)
    }

    pub(super) fn any_to_serialized_sign_doc(
        &self,
        account_info: &BaseAccount,
        tx_payload: Any,
        fee: Fee,
        timeout_height: u64,
        memo: &str,
    ) -> cosmrs::Result<SerializedUnsignedTx> {
        let tx_body = tx::Body::new(vec![tx_payload], memo, timeout_height as u32);
        let pubkey = self.activation_policy.public_key()?.into();
        let auth_info = SignerInfo::single_direct(Some(pubkey), account_info.sequence).auth_info(fee);
        let sign_doc = SignDoc::new(
            &tx_body,
            &auth_info,
            &self.protocol_info.chain_id,
            account_info.account_number,
        )?;

        let tx_json = if self.is_wallet_connect() {
            let ctx = MmArc::from_weak(&self.ctx).expect("No context");
            let wc = WalletConnectCtx::from_ctx(&ctx).expect("should never fail in this block");
            let session_topic = self
                .session_topic()
                .expect("session_topic can't be None inside this block");
            let encode = |data| wc.encode(session_topic, data);

            json!({
                "signerAddress":  self.my_address()?,
                "signDoc": {
                    "accountNumber": sign_doc.account_number.to_string(),
                    "chainId": sign_doc.chain_id,
                    "bodyBytes": encode(&sign_doc.body_bytes),
                    "authInfoBytes": encode(&sign_doc.auth_info_bytes)
                }
            })
        } else {
            json!({
                "sign_doc": {
                    "body_bytes": &sign_doc.body_bytes,
                    "auth_info_bytes": sign_doc.auth_info_bytes,
                    "chain_id": sign_doc.chain_id,
                    "account_number": sign_doc.account_number,
                }
            })
        };

        Ok(SerializedUnsignedTx {
            tx_json,
            body_bytes: sign_doc.body_bytes,
        })
    }

    /// This should only be used for Keplr/WalletConnect from Ledger!
    /// When using Keplr from Ledger, they don't accept `SING_MODE_DIRECT` transactions.
    ///
    /// Visit https://docs.cosmos.network/main/build/architecture/adr-050-sign-mode-textual#context for more context.
    pub(super) fn any_to_legacy_amino_json(
        &self,
        account_info: &BaseAccount,
        tx_payload: Any,
        fee: Fee,
        timeout_height: u64,
        memo: &str,
    ) -> cosmrs::Result<SerializedUnsignedTx> {
        const MSG_SEND_TYPE_URL: &str = "/cosmos.bank.v1beta1.MsgSend";
        const LEDGER_MSG_SEND_TYPE_URL: &str = "cosmos-sdk/MsgSend";

        // Ledger's keplr works as wallet-only, so `MsgSend` support is enough for now.
        if tx_payload.type_url != MSG_SEND_TYPE_URL {
            return Err(ErrorReport::new(io::Error::new(
                io::ErrorKind::Unsupported,
                format!(
                    "Signing mode `SIGN_MODE_LEGACY_AMINO_JSON` is not supported for '{}' transaction type.",
                    tx_payload.type_url
                ),
            )));
        }

        let msg_send = MsgSend::from_any(&tx_payload)?;
        let timeout_height = u32::try_from(timeout_height)?;

        let amount: Vec<Json> = msg_send
            .amount
            .into_iter()
            .map(|t| {
                json!( {
                    "denom": t.denom,
                    // Numbers needs to be converted into string type.
                    // Ref: https://github.com/cosmos/ledger-cosmos/blob/c707129e59f6e0f07ad67161a6b75e8951af063c/docs/TXSPEC.md#json-format
                    "amount": t.amount.to_string(),
                })
            })
            .collect();

        let msg = json!({
            "type": LEDGER_MSG_SEND_TYPE_URL,
            "value": json!({
                "from_address": msg_send.from_address.to_string(),
                "to_address": msg_send.to_address.to_string(),
                "amount": amount,
            })
        });

        let fee_amount: Vec<Json> = fee
            .amount
            .into_iter()
            .map(|t| {
                json!( {
                    "denom": t.denom,
                    // Numbers needs to be converted into string type.
                    // Ref: https://github.com/cosmos/ledger-cosmos/blob/c707129e59f6e0f07ad67161a6b75e8951af063c/docs/TXSPEC.md#json-format
                    "amount": t.amount.to_string(),
                })
            })
            .collect();

        let sign_doc = json!({
            "account_number": account_info.account_number.to_string(),
            "chain_id": self.protocol_info.chain_id.to_string(),
            "fee": {
                "amount": fee_amount,
                "gas": fee.gas_limit.to_string()
                },
            "memo": memo,
            "msgs": [msg],
            "sequence": account_info.sequence.to_string()
        });
        let (tx_json, body_bytes) = match self.wallet_type {
            TendermintWalletConnectionType::WcLedger(_) => {
                let signer_address = self
                    .my_address()
                    .map_err(|e| ErrorReport::new(io::Error::other(e.to_string())))?;
                let body_bytes = tx::Body::new(vec![tx_payload], memo, timeout_height).into_bytes()?;
                let json = serde_json::json!({
                    "signerAddress": signer_address,
                    "signDoc": sign_doc,
                });
                (json, body_bytes)
            },
            TendermintWalletConnectionType::KeplrLedger => {
                let original_tx_type_url = tx_payload.type_url.clone();
                let body_bytes = tx::Body::new(vec![tx_payload], memo, timeout_height).into_bytes()?;
                let json = serde_json::json!({
                    "legacy_amino_json": sign_doc,
                    "original_tx_type_url": original_tx_type_url,
                });
                (json, body_bytes)
            },
            _ => {
                return Err(ErrorReport::new(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Only WalletConnect activated with Ledger can call this function",
                )))
            },
        };

        Ok(SerializedUnsignedTx { tx_json, body_bytes })
    }

    #[allow(clippy::let_unit_value)] // for mockable
    pub fn add_activated_token_info(&self, ticker: String, decimals: u8, denom: Denom) {
        self.tokens_info
            .lock()
            .insert(denom.to_string(), ActivatedTokenInfo { decimals, ticker });
    }

    fn estimate_blocks_from_duration(&self, duration: u64) -> i64 {
        let estimated_time_lock = (duration / self.avg_blocktime as u64) as i64;

        estimated_time_lock.clamp(MIN_TIME_LOCK, MAX_TIME_LOCK)
    }

    pub(crate) fn check_if_my_payment_sent_for_denom(
        &self,
        decimals: u8,
        denom: Denom,
        other_pub: &[u8],
        secret_hash: &[u8],
        amount: &BigDecimal,
    ) -> Box<dyn Future<Item = Option<TransactionEnum>, Error = String> + Send> {
        let amount = try_fus!(sat_from_big_decimal(amount, decimals));
        let amount = vec![Coin {
            denom,
            amount: amount.into(),
        }];

        let pubkey_hash = dhash160(other_pub);
        let to_address = try_fus!(AccountId::new(
            &self.protocol_info.account_prefix,
            pubkey_hash.as_slice()
        ));

        let htlc_id = self.calculate_htlc_id(&self.account_id, &to_address, &amount, secret_hash);

        let coin = self.clone();
        let fut = async move {
            let htlc_response = try_s!(coin.query_htlc(htlc_id.clone()).await);

            let Some(htlc_state) = htlc_response.htlc_state() else {
                return Ok(None);
            };

            match htlc_state {
                HTLC_STATE_OPEN | HTLC_STATE_COMPLETED | HTLC_STATE_REFUNDED => {},
                unexpected_state => return Err(format!("Unexpected state for HTLC {unexpected_state}")),
            };

            let rpc_client = try_s!(coin.rpc_client().await);
            let q = format!("create_htlc.id = '{htlc_id}'");

            let response = try_s!(
                // Search single tx
                rpc_client
                    .perform(TxSearchRequest::new(
                        q,
                        false,
                        1,
                        1,
                        TendermintResultOrder::Descending.into()
                    ))
                    .await
            );

            if let Some(tx) = response.txs.first() {
                if let cosmrs::tendermint::abci::Code::Err(err_code) = tx.tx_result.code {
                    return Err(format!(
                        "Got {err_code} error code. Broadcasted HTLC likely isn't valid."
                    ));
                }

                let deserialized_tx = try_s!(cosmrs::Tx::from_bytes(&tx.tx));
                let msg = try_s!(deserialized_tx.body.messages.first().ok_or("Tx body couldn't be read."));
                let htlc = try_s!(CreateHtlcProto::decode(
                    try_s!(HtlcType::from_str(&coin.protocol_info.account_prefix)),
                    msg.value.as_slice()
                ));

                let Some(hash_lock) = htlc_response.hash_lock() else {
                    return Ok(None);
                };

                if htlc.hash_lock().to_uppercase() == hash_lock.to_uppercase() {
                    let htlc = TransactionEnum::CosmosTransaction(CosmosTransaction {
                        data: try_s!(TxRaw::decode(tx.tx.as_slice())),
                    });
                    return Ok(Some(htlc));
                }
            }

            Ok(None)
        };

        Box::new(fut.boxed().compat())
    }

    pub(super) fn send_htlc_for_denom(
        &self,
        time_lock_duration: u64,
        other_pub: &[u8],
        secret_hash: &[u8],
        amount: BigDecimal,
        denom: Denom,
        decimals: u8,
    ) -> TransactionFut {
        let pubkey_hash = dhash160(other_pub);
        let to = try_tx_fus!(AccountId::new(
            &self.protocol_info.account_prefix,
            pubkey_hash.as_slice()
        ));

        let amount_as_u64 = try_tx_fus!(sat_from_big_decimal(&amount, decimals));
        let amount = cosmrs::Amount::from(amount_as_u64);

        let secret_hash = secret_hash.to_vec();
        let coin = self.clone();
        let fut = async move {
            let time_lock = coin.estimate_blocks_from_duration(time_lock_duration);

            let create_htlc_tx = try_tx_s!(coin.gen_create_htlc_tx(denom, &to, amount, &secret_hash, time_lock as u64));

            let current_block = try_tx_s!(coin.current_block().compat().await);
            let timeout_height = current_block + TIMEOUT_HEIGHT_DELTA;

            let fee = try_tx_s!(
                coin.calculate_fee(
                    create_htlc_tx.msg_payload.clone(),
                    timeout_height,
                    TX_DEFAULT_MEMO,
                    None
                )
                .await
            );

            let (_tx_id, tx_raw) = try_tx_s!(
                coin.common_send_raw_tx_bytes(
                    create_htlc_tx.msg_payload.clone(),
                    fee.clone(),
                    timeout_height,
                    TX_DEFAULT_MEMO,
                    Duration::from_secs(time_lock_duration),
                )
                .await
            );

            Ok(TransactionEnum::CosmosTransaction(CosmosTransaction {
                data: tx_raw.into(),
            }))
        };

        Box::new(fut.boxed().compat())
    }

    pub(super) fn send_taker_fee_for_denom(
        &self,
        dex_fee: &DexFee,
        denom: Denom,
        decimals: u8,
        uuid: &[u8],
        expires_at: u64,
    ) -> TransactionFut {
        let memo = try_tx_fus!(Uuid::from_slice(uuid)).to_string();
        let from_address = self.account_id.clone();
        let dex_pubkey_hash = dhash160(self.dex_pubkey());
        let burn_pubkey_hash = dhash160(self.burn_pubkey());
        let dex_address = try_tx_fus!(AccountId::new(
            &self.protocol_info.account_prefix,
            dex_pubkey_hash.as_slice()
        ));
        let burn_address = try_tx_fus!(AccountId::new(
            &self.protocol_info.account_prefix,
            burn_pubkey_hash.as_slice()
        ));

        let fee_amount_as_u64 = try_tx_fus!(dex_fee.fee_amount_as_u64(decimals));
        let fee_amount = vec![Coin {
            denom: denom.clone(),
            amount: cosmrs::Amount::from(fee_amount_as_u64),
        }];

        let tx_result = match dex_fee {
            DexFee::NoFee => try_tx_fus!(Err("Unexpected DexFee::NoFee".to_owned())),
            DexFee::Standard(_) => MsgSend {
                from_address,
                to_address: dex_address,
                amount: fee_amount,
            }
            .to_any(),
            DexFee::WithBurn { .. } => {
                let burn_amount_as_u64 = try_tx_fus!(dex_fee.burn_amount_as_u64(decimals)).unwrap_or_default();
                let burn_amount = vec![Coin {
                    denom: denom.clone(),
                    amount: cosmrs::Amount::from(burn_amount_as_u64),
                }];
                let total_amount_as_u64 = fee_amount_as_u64 + burn_amount_as_u64;
                let total_amount = vec![Coin {
                    denom,
                    amount: cosmrs::Amount::from(total_amount_as_u64),
                }];
                MsgMultiSend {
                    inputs: vec![MultiSendIo {
                        address: from_address,
                        coins: total_amount,
                    }],
                    outputs: vec![
                        MultiSendIo {
                            address: dex_address,
                            coins: fee_amount,
                        },
                        MultiSendIo {
                            address: burn_address,
                            coins: burn_amount,
                        },
                    ],
                }
                .to_any()
            },
        };
        let tx_payload = try_tx_fus!(tx_result);

        let coin = self.clone();
        let fut = async move {
            let current_block = try_tx_s!(coin.current_block().compat().await.map_to_mm(WithdrawError::Transport));
            let timeout_height = current_block + TIMEOUT_HEIGHT_DELTA;

            let fee = try_tx_s!(
                coin.calculate_fee(tx_payload.clone(), timeout_height, TX_DEFAULT_MEMO, None)
                    .await
            );

            let timeout = expires_at.checked_sub(now_sec()).unwrap_or_default();
            let (_tx_id, tx_raw) = try_tx_s!(
                coin.common_send_raw_tx_bytes(
                    tx_payload.clone(),
                    fee.clone(),
                    timeout_height,
                    &memo,
                    Duration::from_secs(timeout)
                )
                .await
            );

            Ok(TransactionEnum::CosmosTransaction(CosmosTransaction {
                data: tx_raw.into(),
            }))
        };

        Box::new(fut.boxed().compat())
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) fn validate_fee_for_denom(
        &self,
        fee_tx: &TransactionEnum,
        expected_sender: &[u8],
        dex_fee: &DexFee,
        decimals: u8,
        uuid: &[u8],
        denom: String,
    ) -> ValidatePaymentFut<()> {
        let tx = match fee_tx {
            TransactionEnum::CosmosTransaction(tx) => tx.clone(),
            invalid_variant => {
                return Box::new(futures01::future::err(
                    ValidatePaymentError::WrongPaymentTx(format!("Unexpected tx variant {invalid_variant:?}")).into(),
                ))
            },
        };

        let uuid = try_f!(Uuid::from_slice(uuid).map_to_mm(|r| ValidatePaymentError::InvalidParameter(r.to_string())))
            .to_string();

        let sender_pubkey_hash = dhash160(expected_sender);
        let expected_sender_address = try_f!(AccountId::new(
            &self.protocol_info.account_prefix,
            sender_pubkey_hash.as_slice()
        )
        .map_to_mm(|r| ValidatePaymentError::InvalidParameter(r.to_string())));

        let coin = self.clone();
        let dex_fee = dex_fee.clone();
        let fut = async move {
            let tx_body = TxBody::decode(tx.data.body_bytes.as_slice())
                .map_to_mm(|e| ValidatePaymentError::TxDeserializationError(e.to_string()))?;

            match dex_fee {
                DexFee::NoFee => {
                    return MmError::err(ValidatePaymentError::InternalError(
                        "unexpected DexFee::NoFee".to_string(),
                    ))
                },
                DexFee::Standard(_) => coin.validate_standard_dex_fee(
                    &tx_body,
                    &expected_sender_address,
                    &dex_fee,
                    decimals,
                    denom.clone(),
                )?,
                DexFee::WithBurn { .. } => coin.validate_with_burn_dex_fee(
                    &tx_body,
                    &expected_sender_address,
                    &dex_fee,
                    decimals,
                    denom.clone(),
                )?,
            }

            if tx_body.memo != uuid {
                return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                    "Invalid memo: {}, expected {}",
                    tx_body.memo, uuid
                )));
            }

            let encoded_tx = tx.data.encode_to_vec();
            let hash = hex::encode_upper(sha256(&encoded_tx).as_slice());
            let encoded_from_rpc = coin
                .request_tx(hash)
                .await
                .map_err(|e| MmError::new(ValidatePaymentError::TxDeserializationError(e.into_inner().to_string())))?
                .encode_to_vec();
            if encoded_tx != encoded_from_rpc {
                return MmError::err(ValidatePaymentError::WrongPaymentTx(
                    "Transaction from RPC doesn't match the input".to_string(),
                ));
            }
            Ok(())
        };
        Box::new(fut.boxed().compat())
    }

    pub(super) async fn validate_payment_for_denom(
        &self,
        input: ValidatePaymentInput,
        denom: Denom,
        decimals: u8,
    ) -> ValidatePaymentResult<()> {
        let tx = cosmrs::Tx::from_bytes(&input.payment_tx)
            .map_to_mm(|e| ValidatePaymentError::TxDeserializationError(e.to_string()))?;

        if tx.body.messages.len() != 1 {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(
                "Payment tx must have exactly one message".into(),
            ));
        }
        let htlc_type = HtlcType::from_str(&self.protocol_info.account_prefix).map_err(|_| {
            ValidatePaymentError::InvalidParameter(format!(
                "Account type '{}' is not supported for HTLCs",
                self.protocol_info.account_prefix
            ))
        })?;

        let create_htlc_msg_proto = CreateHtlcProto::decode(htlc_type, tx.body.messages[0].value.as_slice())
            .map_to_mm(|e| ValidatePaymentError::WrongPaymentTx(e.to_string()))?;
        let create_htlc_msg = CreateHtlcMsg::try_from(create_htlc_msg_proto)
            .map_to_mm(|e| ValidatePaymentError::WrongPaymentTx(e.to_string()))?;

        let sender_pubkey_hash = dhash160(&input.other_pub);
        let sender = AccountId::new(&self.protocol_info.account_prefix, sender_pubkey_hash.as_slice())
            .map_to_mm(|e| ValidatePaymentError::InvalidParameter(e.to_string()))?;

        let amount = sat_from_big_decimal(&input.amount, decimals).map_mm_err()?;
        let amount = vec![Coin {
            denom,
            amount: amount.into(),
        }];

        let time_lock = self.estimate_blocks_from_duration(input.time_lock_duration);

        let expected_msg = CreateHtlcMsg::new(
            htlc_type,
            sender.clone(),
            self.account_id.clone(),
            amount.clone(),
            hex::encode(&input.secret_hash),
            0,
            time_lock as u64,
        );

        if create_htlc_msg != expected_msg {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "Incorrect CreateHtlc message {create_htlc_msg:?}, expected {expected_msg:?}"
            )));
        }

        let hash = hex::encode_upper(sha256(&input.payment_tx).as_slice());
        let tx_from_rpc = self.request_tx(hash).await.map_mm_err()?;
        if input.payment_tx != tx_from_rpc.encode_to_vec() {
            return MmError::err(ValidatePaymentError::InvalidRpcResponse(
                "Tx from RPC doesn't match the input".into(),
            ));
        }

        let htlc_id = self.calculate_htlc_id(&sender, &self.account_id, &amount, &input.secret_hash);

        let htlc_response = self.query_htlc(htlc_id.clone()).await.map_mm_err()?;
        let htlc_state = htlc_response
            .htlc_state()
            .or_mm_err(|| ValidatePaymentError::InvalidRpcResponse(format!("No HTLC data for {htlc_id}")))?;

        match htlc_state {
            HTLC_STATE_OPEN => Ok(()),
            unexpected_state => MmError::err(ValidatePaymentError::UnexpectedPaymentState(format!(
                "{unexpected_state}"
            ))),
        }
    }

    fn validate_standard_dex_fee(
        &self,
        tx_body: &TxBody,
        expected_sender_address: &AccountId,
        dex_fee: &DexFee,
        decimals: u8,
        denom: String,
    ) -> MmResult<(), ValidatePaymentError> {
        if tx_body.messages.len() != 1 {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(
                "Tx body must have exactly one message".to_string(),
            ));
        }

        let dex_pubkey_hash = dhash160(self.dex_pubkey());
        let expected_dex_address = AccountId::new(&self.protocol_info.account_prefix, dex_pubkey_hash.as_slice())
            .map_to_mm(|r| ValidatePaymentError::InvalidParameter(r.to_string()))?;

        let fee_amount_as_u64 = dex_fee.fee_amount_as_u64(decimals).map_mm_err()?;
        let expected_dex_amount = CoinProto {
            denom,
            amount: fee_amount_as_u64.to_string(),
        };

        let msg = MsgSendProto::decode(tx_body.messages[0].value.as_slice())
            .map_to_mm(|e| ValidatePaymentError::TxDeserializationError(e.to_string()))?;
        if msg.to_address != expected_dex_address.as_ref() {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "Dex fee is sent to wrong address: {}, expected {}",
                msg.to_address, expected_dex_address
            )));
        }
        if msg.amount.len() != 1 {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(
                "Msg must have exactly one Coin".to_string(),
            ));
        }
        if msg.amount[0] != expected_dex_amount {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "Invalid amount {:?}, expected {:?}",
                msg.amount[0], expected_dex_amount
            )));
        }
        if msg.from_address != expected_sender_address.as_ref() {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "Invalid sender: {}, expected {}",
                msg.from_address, expected_sender_address
            )));
        }
        Ok(())
    }

    fn validate_with_burn_dex_fee(
        &self,
        tx_body: &TxBody,
        expected_sender_address: &AccountId,
        dex_fee: &DexFee,
        decimals: u8,
        denom: String,
    ) -> MmResult<(), ValidatePaymentError> {
        if tx_body.messages.len() != 1 {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(
                "Tx body must have exactly one message".to_string(),
            ));
        }

        let dex_pubkey_hash = dhash160(self.dex_pubkey());
        let expected_dex_address = AccountId::new(&self.protocol_info.account_prefix, dex_pubkey_hash.as_slice())
            .map_to_mm(|r| ValidatePaymentError::InvalidParameter(r.to_string()))?;

        let burn_pubkey_hash = dhash160(self.burn_pubkey());
        let expected_burn_address = AccountId::new(&self.protocol_info.account_prefix, burn_pubkey_hash.as_slice())
            .map_to_mm(|r| ValidatePaymentError::InvalidParameter(r.to_string()))?;

        let fee_amount_as_u64 = dex_fee.fee_amount_as_u64(decimals).map_mm_err()?;
        let expected_dex_amount = CoinProto {
            denom: denom.clone(),
            amount: fee_amount_as_u64.to_string(),
        };
        let burn_amount_as_u64 = dex_fee.burn_amount_as_u64(decimals).map_mm_err()?.unwrap_or_default();
        let expected_burn_amount = CoinProto {
            denom,
            amount: burn_amount_as_u64.to_string(),
        };

        let msg = MsgMultiSendProto::decode(tx_body.messages[0].value.as_slice())
            .map_to_mm(|e| ValidatePaymentError::TxDeserializationError(e.to_string()))?;
        if msg.outputs.len() != 2 {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(
                "Msg must have exactly two outputs".to_string(),
            ));
        }

        // Validate dex fee output
        if msg.outputs[0].address != expected_dex_address.as_ref() {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "Dex fee is sent to wrong address: {}, expected {}",
                msg.outputs[0].address, expected_dex_address
            )));
        }
        if msg.outputs[0].coins.len() != 1 {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(
                "Dex fee output must have exactly one Coin".to_string(),
            ));
        }
        if msg.outputs[0].coins[0] != expected_dex_amount {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "Invalid dex fee amount {:?}, expected {:?}",
                msg.outputs[0].coins[0], expected_dex_amount
            )));
        }

        // Validate burn output
        if msg.outputs[1].address != expected_burn_address.as_ref() {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "Burn fee is sent to wrong address: {}, expected {}",
                msg.outputs[1].address, expected_burn_address
            )));
        }
        if msg.outputs[1].coins.len() != 1 {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(
                "Burn fee output must have exactly one Coin".to_string(),
            ));
        }
        if msg.outputs[1].coins[0] != expected_burn_amount {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "Invalid burn amount {:?}, expected {:?}",
                msg.outputs[1].coins[0], expected_burn_amount
            )));
        }
        if msg.inputs.len() != 1 {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(
                "Msg must have exactly one input".to_string(),
            ));
        }

        // validate input
        if msg.inputs[0].address != expected_sender_address.as_ref() {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "Invalid sender: {}, expected {}",
                msg.inputs[0].address, expected_sender_address
            )));
        }
        Ok(())
    }

    pub(super) async fn get_sender_trade_fee_for_denom(
        &self,
        ticker: String,
        denom: Denom,
        decimals: u8,
        amount: BigDecimal,
    ) -> TradePreimageResult<TradeFee> {
        const TIME_LOCK: u64 = 1750;

        let mut sec = [0u8; 32];
        common::os_rng(&mut sec).map_err(|e| MmError::new(TradePreimageError::InternalError(e.to_string())))?;
        drop_mutability!(sec);

        let to_address = account_id_from_pubkey_hex(&self.protocol_info.account_prefix, DEX_FEE_ADDR_PUBKEY)
            .map_err(|e| MmError::new(TradePreimageError::InternalError(e.to_string())))?;

        let amount = sat_from_big_decimal(&amount, decimals).map_mm_err()?;

        let create_htlc_tx = self
            .gen_create_htlc_tx(denom, &to_address, amount.into(), sha256(&sec).as_slice(), TIME_LOCK)
            .map_err(|e| {
                MmError::new(TradePreimageError::InternalError(format!(
                    "Could not create HTLC. {:?}",
                    e.into_inner()
                )))
            })?;

        let current_block = self.current_block().compat().await.map_err(|e| {
            MmError::new(TradePreimageError::InternalError(format!(
                "Could not get current_block. {e}"
            )))
        })?;

        let timeout_height = current_block + TIMEOUT_HEIGHT_DELTA;

        let fee_uamount = self
            .calculate_account_fee_amount_as_u64(
                &self.account_id,
                self.activation_policy.activated_key(),
                create_htlc_tx.msg_payload.clone(),
                timeout_height,
                TX_DEFAULT_MEMO,
                None,
            )
            .await
            .map_mm_err()?;

        let fee_amount = big_decimal_from_sat_unsigned(fee_uamount, self.protocol_info.decimals);

        Ok(TradeFee {
            coin: ticker,
            amount: fee_amount.into(),
            paid_from_trading_vol: false,
        })
    }

    pub(super) async fn get_fee_to_send_taker_fee_for_denom(
        &self,
        ticker: String,
        denom: Denom,
        decimals: u8,
        dex_fee_amount: DexFee,
    ) -> TradePreimageResult<TradeFee> {
        let to_address = account_id_from_pubkey_hex(&self.protocol_info.account_prefix, DEX_FEE_ADDR_PUBKEY)
            .map_err(|e| MmError::new(TradePreimageError::InternalError(e.to_string())))?;
        let amount = sat_from_big_decimal(&dex_fee_amount.fee_amount().into(), decimals).map_mm_err()?;

        let current_block = self.current_block().compat().await.map_err(|e| {
            MmError::new(TradePreimageError::InternalError(format!(
                "Could not get current_block. {e}"
            )))
        })?;

        let timeout_height = current_block + TIMEOUT_HEIGHT_DELTA;

        let msg_send = MsgSend {
            from_address: self.account_id.clone(),
            to_address: to_address.clone(),
            amount: vec![Coin {
                denom,
                amount: amount.into(),
            }],
        }
        .to_any()
        .map_err(|e| MmError::new(TradePreimageError::InternalError(e.to_string())))?;

        let fee_uamount = self
            .calculate_account_fee_amount_as_u64(
                &self.account_id,
                self.activation_policy.activated_key(),
                msg_send,
                timeout_height,
                TX_DEFAULT_MEMO,
                None,
            )
            .await
            .map_mm_err()?;
        let fee_amount = big_decimal_from_sat_unsigned(fee_uamount, decimals);

        Ok(TradeFee {
            coin: ticker,
            amount: fee_amount.into(),
            paid_from_trading_vol: false,
        })
    }

    pub(super) async fn get_balance_as_unsigned_and_decimal(
        &self,
        account_id: &AccountId,
        denom: &Denom,
        decimals: u8,
    ) -> MmResult<(u64, BigDecimal), TendermintCoinRpcError> {
        let denom_ubalance = self.account_balance_for_denom(account_id, denom.to_string()).await?;
        let denom_balance_dec = big_decimal_from_sat_unsigned(denom_ubalance, decimals);

        Ok((denom_ubalance, denom_balance_dec))
    }

    async fn request_tx(&self, hash: String) -> MmResult<Tx, TendermintCoinRpcError> {
        let request = GetTxRequest { hash };
        let response = self
            .rpc_client()
            .await?
            .abci_query(
                Some(ABCI_GET_TX_PATH.to_string()),
                request.encode_to_vec(),
                ABCI_REQUEST_HEIGHT,
                ABCI_REQUEST_PROVE,
            )
            .await?;

        let response = GetTxResponse::decode(response.value.as_slice())?;
        response
            .tx
            .or_mm_err(|| TendermintCoinRpcError::InvalidResponse(format!("Tx {} does not exist", request.hash)))
    }

    /// Returns status code of transaction.
    /// If tx doesn't exists on chain, then returns `None`.
    async fn get_tx_status_code_or_none(
        &self,
        hash: String,
    ) -> MmResult<Option<cosmrs::tendermint::abci::Code>, TendermintCoinRpcError> {
        let request = GetTxRequest { hash };
        let response = self
            .rpc_client()
            .await?
            .abci_query(
                Some(ABCI_GET_TX_PATH.to_string()),
                request.encode_to_vec(),
                ABCI_REQUEST_HEIGHT,
                ABCI_REQUEST_PROVE,
            )
            .await?;

        let tx = GetTxResponse::decode(response.value.as_slice())?;

        if let Some(tx_response) = tx.tx_response {
            // non-zero values are error.
            match tx_response.code {
                TX_SUCCESS_CODE => Ok(Some(cosmrs::tendermint::abci::Code::Ok)),
                err_code => Ok(Some(cosmrs::tendermint::abci::Code::Err(
                    // This will never panic, as `0` code goes the the success variant above.
                    NonZeroU32::new(err_code).unwrap(),
                ))),
            }
        } else {
            Ok(None)
        }
    }

    pub(crate) async fn query_htlc(&self, id: String) -> MmResult<QueryHtlcResponse, TendermintCoinRpcError> {
        let htlc_type = HtlcType::from_str(&self.protocol_info.account_prefix).map_err(|_| {
            TendermintCoinRpcError::UnexpectedAccountType {
                prefix: self.protocol_info.account_prefix.clone(),
            }
        })?;

        let request = QueryHtlcRequestProto { id };
        let response = self
            .rpc_client()
            .await?
            .abci_query(
                Some(htlc_type.get_htlc_abci_query_path()),
                request.encode_to_vec(),
                ABCI_REQUEST_HEIGHT,
                ABCI_REQUEST_PROVE,
            )
            .await?;

        Ok(QueryHtlcResponse::decode(htlc_type, response.value.as_slice())?)
    }

    #[inline]
    pub(crate) fn is_tx_amount_enough(&self, decimals: u8, amount: &BigDecimal) -> bool {
        let min_tx_amount = big_decimal_from_sat(MIN_TX_SATOSHIS, decimals);
        amount >= &min_tx_amount
    }

    async fn search_for_swap_tx_spend<'a>(
        &self,
        input: SearchForSwapTxSpendInput<'a>,
    ) -> MmResult<Option<FoundSwapTxSpend>, SearchForSwapTxSpendErr> {
        let tx = cosmrs::Tx::from_bytes(input.tx)?;
        let first_message = tx
            .body
            .messages
            .first()
            .or_mm_err(|| SearchForSwapTxSpendErr::TxMessagesEmpty)?;

        let htlc_type = HtlcType::from_str(&self.protocol_info.account_prefix).map_err(|_| {
            SearchForSwapTxSpendErr::UnexpectedAccountType {
                prefix: self.protocol_info.account_prefix.clone(),
            }
        })?;

        let htlc_proto = CreateHtlcProto::decode(htlc_type, first_message.value.as_slice())?;
        let htlc = CreateHtlcMsg::try_from(htlc_proto)?;
        let htlc_id = self.calculate_htlc_id(htlc.sender(), htlc.to(), htlc.amount(), input.secret_hash);

        let htlc_response = self.query_htlc(htlc_id.clone()).await.map_mm_err()?;

        let htlc_state = match htlc_response.htlc_state() {
            Some(htlc_state) => htlc_state,
            None => return Ok(None),
        };

        match htlc_state {
            HTLC_STATE_OPEN => Ok(None),
            HTLC_STATE_COMPLETED => {
                let query = format!("claim_htlc.id='{htlc_id}'");
                let request = TxSearchRequest {
                    query,
                    order_by: TendermintResultOrder::Ascending.into(),
                    page: 1,
                    per_page: 1,
                    prove: false,
                };

                let response = self
                    .rpc_client()
                    .await
                    .map_mm_err()?
                    .perform(request)
                    .await
                    .map_to_mm(TendermintCoinRpcError::from)
                    .map_mm_err()?;
                match response.txs.first() {
                    Some(raw_tx) => {
                        let tx = cosmrs::Tx::from_bytes(&raw_tx.tx)?;
                        let tx = TransactionEnum::CosmosTransaction(CosmosTransaction {
                            data: TxRaw {
                                body_bytes: tx.body.into_bytes()?,
                                auth_info_bytes: tx.auth_info.into_bytes()?,
                                signatures: tx.signatures,
                            },
                        });
                        Ok(Some(FoundSwapTxSpend::Spent(tx)))
                    },
                    None => MmError::err(SearchForSwapTxSpendErr::ClaimHtlcTxNotFound),
                }
            },
            HTLC_STATE_REFUNDED => {
                // HTLC is refunded automatically without transaction. We have to return dummy tx data
                Ok(Some(FoundSwapTxSpend::Refunded(TransactionEnum::CosmosTransaction(
                    CosmosTransaction { data: TxRaw::default() },
                ))))
            },
            unexpected_state => MmError::err(SearchForSwapTxSpendErr::UnexpectedHtlcState(unexpected_state)),
        }
    }

    pub(crate) fn gas_info_for_withdraw(
        &self,
        withdraw_fee: &Option<WithdrawFee>,
        fallback_gas_limit: u64,
    ) -> (f64, u64) {
        match withdraw_fee {
            Some(WithdrawFee::CosmosGas { gas_price, gas_limit }) => (*gas_price, *gas_limit),
            _ => (self.gas_price(), fallback_gas_limit),
        }
    }

    pub(crate) fn active_ticker_and_decimals_from_denom(&self, denom: &str) -> Option<(String, u8)> {
        if self.protocol_info.denom.as_ref() == denom {
            return Some((self.ticker.clone(), self.protocol_info.decimals));
        }

        let tokens = self.tokens_info.lock();

        if let Some(token_info) = tokens.get(denom) {
            return Some((token_info.ticker.to_owned(), token_info.decimals));
        }

        None
    }

    #[inline]
    pub fn is_ledger_connection(&self) -> bool {
        matches!(
            self.wallet_type,
            TendermintWalletConnectionType::WcLedger(_) | TendermintWalletConnectionType::KeplrLedger
        )
    }

    #[inline]
    pub fn is_wallet_connect(&self) -> bool {
        matches!(
            self.wallet_type,
            TendermintWalletConnectionType::WcLedger(_) | TendermintWalletConnectionType::Wc(_)
        )
    }

    pub(crate) async fn validators_list(
        &self,
        filter_status: ValidatorStatus,
        paging: PagingOptions,
    ) -> MmResult<Vec<Validator>, TendermintCoinRpcError> {
        let request = QueryValidatorsRequest {
            status: filter_status.to_string(),
            pagination: Some(PageRequest {
                key: vec![],
                offset: ((paging.page_number.get() - 1usize) * paging.limit) as u64,
                limit: paging.limit as u64,
                count_total: false,
                reverse: false,
            }),
        };

        let raw_response = self
            .rpc_client()
            .await?
            .abci_query(
                Some(ABCI_VALIDATORS_PATH.to_owned()),
                request.encode_to_vec(),
                ABCI_REQUEST_HEIGHT,
                ABCI_REQUEST_PROVE,
            )
            .await?;

        let decoded_proto = QueryValidatorsResponseProto::decode(raw_response.value.as_slice())?;
        let typed_response = QueryValidatorsResponse::try_from(decoded_proto)
            .map_err(|e| TendermintCoinRpcError::InternalError(e.to_string()))?;

        Ok(typed_response.validators)
    }

    pub(crate) async fn delegate(&self, req: DelegationPayload) -> MmResult<TransactionDetails, DelegationError> {
        fn generate_message(
            delegator_address: AccountId,
            validator_address: AccountId,
            denom: Denom,
            amount: u128,
        ) -> Result<Any, ErrorReport> {
            MsgDelegate {
                delegator_address,
                validator_address,
                amount: Coin { denom, amount },
            }
            .to_any()
        }

        /// Calculates the send and total amounts.
        ///
        /// The send amount is what the receiver receives, while the total amount is what sender
        /// pays including the transaction fee.
        fn calc_send_and_total_amount(
            coin: &TendermintCoin,
            balance_u64: u64,
            balance_decimal: BigDecimal,
            fee_u64: u64,
            fee_decimal: BigDecimal,
            request_amount: BigDecimal,
            is_max: bool,
        ) -> Result<(u64, BigDecimal), DelegationError> {
            let not_sufficient = |required| DelegationError::NotSufficientBalance {
                coin: coin.ticker.clone(),
                available: balance_decimal.clone(),
                required,
            };

            if is_max {
                if balance_u64 < fee_u64 {
                    return Err(not_sufficient(fee_decimal));
                }

                let amount_u64 = balance_u64 - fee_u64;
                return Ok((amount_u64, balance_decimal));
            }

            let total = &request_amount + &fee_decimal;
            if balance_decimal < total {
                return Err(not_sufficient(total));
            }

            let amount_u64 = sat_from_big_decimal(&request_amount, coin.protocol_info.decimals)
                .map_err(|e| DelegationError::InternalError(e.to_string()))?;

            Ok((amount_u64, total))
        }

        let validator_address =
            AccountId::from_str(&req.validator_address).map_to_mm(|e| DelegationError::AddressError(e.to_string()))?;

        let (delegator_address, maybe_priv_key) = self
            .extract_account_id_and_private_key(req.withdraw_from)
            .map_err(|e| DelegationError::InternalError(e.to_string()))?;

        let (balance_u64, balance_dec) = self
            .get_balance_as_unsigned_and_decimal(&delegator_address, &self.protocol_info.denom, self.decimals())
            .await
            .map_mm_err()?;

        let amount_u64 = if req.max {
            balance_u64
        } else {
            sat_from_big_decimal(&req.amount, self.protocol_info.decimals)
                .map_err(|e| DelegationError::InternalError(e.to_string()))?
        };

        // This is used for transaction simulation so we can predict the best possible fee amount.
        let msg_for_fee_prediction = generate_message(
            delegator_address.clone(),
            validator_address.clone(),
            self.protocol_info.denom.clone(),
            amount_u64.into(),
        )
        .map_err(|e| DelegationError::InternalError(e.to_string()))?;

        let timeout_height = self
            .current_block()
            .compat()
            .await
            .map_to_mm(DelegationError::Transport)?
            + TIMEOUT_HEIGHT_DELTA;

        // `delegate` uses more gas than the regular transactions
        let gas_limit_default = (GAS_LIMIT_DEFAULT * 3) / 2;
        let (_, gas_limit) = self.gas_info_for_withdraw(&req.fee, gas_limit_default);

        let fee_amount_u64 = self
            .calculate_account_fee_amount_as_u64(
                &delegator_address,
                maybe_priv_key,
                msg_for_fee_prediction,
                timeout_height,
                &req.memo,
                req.fee,
            )
            .await
            .map_mm_err()?;

        let fee_amount_dec = big_decimal_from_sat_unsigned(fee_amount_u64, self.decimals());

        let fee = Fee::from_amount_and_gas(
            Coin {
                denom: self.protocol_info.denom.clone(),
                amount: fee_amount_u64.into(),
            },
            gas_limit,
        );

        let (amount_u64, total_amount) = calc_send_and_total_amount(
            self,
            balance_u64,
            balance_dec,
            fee_amount_u64,
            fee_amount_dec.clone(),
            req.amount,
            req.max,
        )?;

        let msg_for_actual_tx = generate_message(
            delegator_address.clone(),
            validator_address.clone(),
            self.protocol_info.denom.clone(),
            amount_u64.into(),
        )
        .map_err(|e| DelegationError::InternalError(e.to_string()))?;

        let account_info = self.account_info(&delegator_address).await.map_mm_err()?;

        let tx = self
            .any_to_transaction_data(
                maybe_priv_key,
                msg_for_actual_tx,
                &account_info,
                fee,
                timeout_height,
                &req.memo,
            )
            .await
            .map_to_mm(|e| DelegationError::InternalError(e.to_string()))?;

        let internal_id = tendermint_tx_internal_id(tx.tx_hash().unwrap_or_default().as_bytes(), None);

        Ok(TransactionDetails {
            tx,
            from: vec![delegator_address.to_string()],
            to: vec![req.validator_address],
            my_balance_change: &BigDecimal::default() - &total_amount,
            spent_by_me: total_amount.clone(),
            total_amount,
            received_by_me: BigDecimal::default(),
            block_height: 0,
            timestamp: 0,
            fee_details: Some(TxFeeDetails::Tendermint(TendermintFeeDetails {
                coin: self.ticker.clone(),
                amount: fee_amount_dec,
                uamount: fee_amount_u64,
                gas_limit,
            })),
            coin: self.ticker.to_string(),
            internal_id,
            kmd_rewards: None,
            transaction_type: TransactionType::StakingDelegation,
            memo: Some(req.memo),
        })
    }

    pub(crate) async fn undelegate(&self, req: DelegationPayload) -> MmResult<TransactionDetails, DelegationError> {
        fn generate_message(
            delegator_address: AccountId,
            validator_address: AccountId,
            denom: Denom,
            amount: u128,
        ) -> Result<Any, ErrorReport> {
            MsgUndelegate {
                delegator_address,
                validator_address,
                amount: Coin { denom, amount },
            }
            .to_any()
        }

        let (delegator_address, maybe_priv_key) = self
            .extract_account_id_and_private_key(None)
            .map_err(|e| DelegationError::InternalError(e.to_string()))?;

        let validator_address =
            AccountId::from_str(&req.validator_address).map_to_mm(|e| DelegationError::AddressError(e.to_string()))?;

        let (total_delegated_amount, total_delegated_uamount) = self.get_delegated_amount(&validator_address).await?;

        let uamount_to_undelegate = if req.max {
            total_delegated_uamount
        } else {
            if req.amount > total_delegated_amount {
                return MmError::err(DelegationError::TooMuchToUndelegate {
                    available: total_delegated_amount,
                    requested: req.amount,
                });
            };

            sat_from_big_decimal(&req.amount, self.protocol_info.decimals)
                .map_err(|e| DelegationError::InternalError(e.to_string()))?
        };

        let undelegate_msg = generate_message(
            delegator_address.clone(),
            validator_address.clone(),
            self.protocol_info.denom.clone(),
            uamount_to_undelegate.into(),
        )
        .map_err(|e| DelegationError::InternalError(e.to_string()))?;

        let timeout_height = self
            .current_block()
            .compat()
            .await
            .map_to_mm(DelegationError::Transport)?
            + TIMEOUT_HEIGHT_DELTA;

        // This uses more gas than any other transactions
        let gas_limit_default = GAS_LIMIT_DEFAULT * 2;
        let (_, gas_limit) = self.gas_info_for_withdraw(&req.fee, gas_limit_default);

        let fee_amount_u64 = self
            .calculate_account_fee_amount_as_u64(
                &delegator_address,
                maybe_priv_key,
                undelegate_msg.clone(),
                timeout_height,
                &req.memo,
                req.fee,
            )
            .await
            .map_mm_err()?;

        let fee_amount_dec = big_decimal_from_sat_unsigned(fee_amount_u64, self.decimals());

        let my_balance = self.my_balance().compat().await.map_mm_err()?.spendable;

        if fee_amount_dec > my_balance {
            return MmError::err(DelegationError::NotSufficientBalance {
                coin: self.ticker.clone(),
                available: my_balance,
                required: fee_amount_dec,
            });
        }

        let fee = Fee::from_amount_and_gas(
            Coin {
                denom: self.protocol_info.denom.clone(),
                amount: fee_amount_u64.into(),
            },
            gas_limit,
        );

        let account_info = self.account_info(&delegator_address).await.map_mm_err()?;

        let tx = self
            .any_to_transaction_data(
                maybe_priv_key,
                undelegate_msg,
                &account_info,
                fee,
                timeout_height,
                &req.memo,
            )
            .await
            .map_to_mm(|e| DelegationError::InternalError(e.to_string()))?;

        let internal_id = tendermint_tx_internal_id(tx.tx_hash().unwrap_or_default().as_bytes(), None);

        Ok(TransactionDetails {
            tx,
            from: vec![delegator_address.to_string()],
            to: vec![], // We just pay the transaction fee for undelegation
            my_balance_change: &BigDecimal::default() - &fee_amount_dec,
            spent_by_me: fee_amount_dec.clone(),
            total_amount: fee_amount_dec.clone(),
            received_by_me: BigDecimal::default(),
            block_height: 0,
            timestamp: 0,
            fee_details: Some(TxFeeDetails::Tendermint(TendermintFeeDetails {
                coin: self.ticker.clone(),
                amount: fee_amount_dec,
                uamount: fee_amount_u64,
                gas_limit,
            })),
            coin: self.ticker.to_string(),
            internal_id,
            kmd_rewards: None,
            transaction_type: TransactionType::RemoveDelegation,
            memo: Some(req.memo),
        })
    }

    async fn get_delegated_amount(
        &self,
        validator_addr: &AccountId, // keep this as `AccountId` to make it pre-validated
    ) -> MmResult<(BigDecimal, u64), DelegationError> {
        let delegator_addr = self
            .my_address()
            .map_err(|e| DelegationError::InternalError(e.to_string()))?;
        let validator_addr = validator_addr.to_string();

        let request = QueryDelegationRequest {
            delegator_addr,
            validator_addr,
        };

        let raw_response = self
            .rpc_client()
            .await
            .map_mm_err()?
            .abci_query(
                Some(ABCI_DELEGATION_PATH.to_owned()),
                request.encode_to_vec(),
                ABCI_REQUEST_HEIGHT,
                ABCI_REQUEST_PROVE,
            )
            .map_err(|e| DelegationError::Transport(e.to_string()))
            .await?;

        let decoded_response = QueryDelegationResponse::decode(raw_response.value.as_slice())
            .map_err(|e| DelegationError::InternalError(e.to_string()))?;

        let Some(delegation_response) = decoded_response.delegation_response else {
            return MmError::err(DelegationError::CanNotUndelegate {
                delegator_addr: request.delegator_addr,
                validator_addr: request.validator_addr,
            });
        };

        let Some(balance) = delegation_response.balance else {
            return MmError::err(DelegationError::Transport(
                format!("Unexpected response from '{ABCI_DELEGATION_PATH}' with {request:?} request; balance field should not be empty.")
            ));
        };

        let uamount = u64::from_str(&balance.amount).map_err(|e| DelegationError::InternalError(e.to_string()))?;

        Ok((big_decimal_from_sat_unsigned(uamount, self.decimals()), uamount))
    }

    async fn get_delegation_reward_amount(
        &self,
        validator_addr: &AccountId, // keep this as `AccountId` to make it pre-validated
    ) -> MmResult<BigDecimal, DelegationError> {
        let delegator_address = self
            .my_address()
            .map_err(|e| DelegationError::InternalError(e.to_string()))?;
        let validator_address = validator_addr.to_string();

        let query_payload = QueryDelegationRewardsRequest {
            delegator_address,
            validator_address,
        };

        let raw_response = self
            .rpc_client()
            .await
            .map_mm_err()?
            .abci_query(
                Some(ABCI_DELEGATION_REWARDS_PATH.to_owned()),
                query_payload.encode_to_vec(),
                ABCI_REQUEST_HEIGHT,
                ABCI_REQUEST_PROVE,
            )
            .map_err(|e| DelegationError::Transport(e.to_string()))
            .await?;

        let decoded_response = QueryDelegationRewardsResponse::decode(raw_response.value.as_slice())
            .map_err(|e| DelegationError::InternalError(e.to_string()))?;

        match decoded_response
            .rewards
            .iter()
            .find(|t| t.denom == self.protocol_info.denom.to_string())
        {
            Some(dec_coin) => extract_big_decimal_from_dec_coin(dec_coin, self.protocol_info.decimals as u32)
                .map_to_mm(|e| DelegationError::InternalError(e.to_string())),
            None => MmError::err(DelegationError::NothingToClaim {
                coin: self.ticker.clone(),
            }),
        }
    }

    pub(crate) async fn claim_staking_rewards(
        &self,
        req: ClaimRewardsPayload,
    ) -> MmResult<TransactionDetails, DelegationError> {
        let (delegator_address, maybe_priv_key) = self
            .extract_account_id_and_private_key(None)
            .map_err(|e| DelegationError::InternalError(e.to_string()))?;

        let validator_address =
            AccountId::from_str(&req.validator_address).map_to_mm(|e| DelegationError::AddressError(e.to_string()))?;

        let msg = MsgWithdrawDelegatorReward {
            delegator_address: delegator_address.clone(),
            validator_address: validator_address.clone(),
        }
        .to_any()
        .map_err(|e| DelegationError::InternalError(e.to_string()))?;

        let reward_amount = self.get_delegation_reward_amount(&validator_address).await?;

        if reward_amount.is_zero() {
            return MmError::err(DelegationError::NothingToClaim {
                coin: self.ticker.clone(),
            });
        }

        let timeout_height = self
            .current_block()
            .compat()
            .await
            .map_to_mm(DelegationError::Transport)?
            + TIMEOUT_HEIGHT_DELTA;

        // This uses more gas than the regular transactions
        let gas_limit_default = (GAS_LIMIT_DEFAULT * 3) / 2;
        let (_, gas_limit) = self.gas_info_for_withdraw(&req.fee, gas_limit_default);

        let fee_amount_u64 = self
            .calculate_account_fee_amount_as_u64(
                &delegator_address,
                maybe_priv_key,
                msg.clone(),
                timeout_height,
                &req.memo,
                req.fee,
            )
            .await
            .map_mm_err()?;

        let fee_amount_dec = big_decimal_from_sat_unsigned(fee_amount_u64, self.decimals());

        let my_balance = self.my_balance().compat().await.map_mm_err()?.spendable;

        if fee_amount_dec > my_balance {
            return MmError::err(DelegationError::NotSufficientBalance {
                coin: self.ticker.clone(),
                available: my_balance,
                required: fee_amount_dec,
            });
        }

        if !req.force && fee_amount_dec > reward_amount {
            return MmError::err(DelegationError::UnprofitableReward {
                reward: reward_amount.clone(),
                fee: fee_amount_dec.clone(),
            });
        }

        let fee = Fee::from_amount_and_gas(
            Coin {
                denom: self.protocol_info.denom.clone(),
                amount: fee_amount_u64.into(),
            },
            gas_limit,
        );

        let account_info = self.account_info(&delegator_address).await.map_mm_err()?;

        let tx = self
            .any_to_transaction_data(maybe_priv_key, msg, &account_info, fee, timeout_height, &req.memo)
            .await
            .map_to_mm(|e| DelegationError::InternalError(e.to_string()))?;

        let internal_id = tendermint_tx_internal_id(tx.tx_hash().unwrap_or_default().as_bytes(), None);

        Ok(TransactionDetails {
            tx,
            from: vec![validator_address.to_string()],
            to: vec![delegator_address.to_string()],
            my_balance_change: &reward_amount - &fee_amount_dec,
            spent_by_me: fee_amount_dec.clone(),
            total_amount: reward_amount.clone(),
            received_by_me: reward_amount,
            block_height: 0,
            timestamp: 0,
            fee_details: Some(TxFeeDetails::Tendermint(TendermintFeeDetails {
                coin: self.ticker.clone(),
                amount: fee_amount_dec,
                uamount: fee_amount_u64,
                gas_limit,
            })),
            coin: self.ticker.to_string(),
            internal_id,
            kmd_rewards: None,
            transaction_type: TransactionType::ClaimDelegationRewards,
            memo: Some(req.memo),
        })
    }

    pub(crate) async fn delegations_list(
        &self,
        paging: PagingOptions,
    ) -> MmResult<DelegationsQueryResponse, TendermintCoinRpcError> {
        let request = QueryDelegatorDelegationsRequest {
            delegator_addr: self.account_id.to_string(),
            pagination: Some(PageRequest {
                key: vec![],
                offset: ((paging.page_number.get() - 1usize) * paging.limit) as u64,
                limit: paging.limit as u64,
                count_total: false,
                reverse: false,
            }),
        };

        let raw_response = self
            .rpc_client()
            .await?
            .abci_query(
                Some(ABCI_DELEGATOR_DELEGATIONS_PATH.to_owned()),
                request.encode_to_vec(),
                ABCI_REQUEST_HEIGHT,
                ABCI_REQUEST_PROVE,
            )
            .await?;

        let decoded_proto = QueryDelegatorDelegationsResponse::decode(raw_response.value.as_slice())?;

        let mut delegations = Vec::new();
        let selfi = self.clone();
        for response in decoded_proto.delegation_responses {
            let Some(delegation) = response.delegation else {
                continue;
            };
            let Some(balance) = response.balance else { continue };

            let account_id = AccountId::from_str(&delegation.validator_address)
                .map_err(|e| TendermintCoinRpcError::InternalError(e.to_string()))?;

            let reward_amount = match selfi.get_delegation_reward_amount(&account_id).await {
                Ok(reward) => reward,
                Err(e) => match e.get_inner() {
                    DelegationError::NothingToClaim { .. } => BigDecimal::zero(),
                    _ => return MmError::err(TendermintCoinRpcError::InvalidResponse(e.to_string())),
                },
            };

            let amount = balance
                .amount
                .parse::<u64>()
                .map_err(|e| TendermintCoinRpcError::InternalError(e.to_string()))?;

            delegations.push(Delegation {
                validator_address: delegation.validator_address,
                delegated_amount: big_decimal_from_sat_unsigned(amount, selfi.decimals()),
                reward_amount,
            });
        }

        Ok(DelegationsQueryResponse { delegations })
    }

    pub(crate) async fn ongoing_undelegations_list(
        &self,
        paging: PagingOptions,
    ) -> MmResult<UndelegationsQueryResponse, TendermintCoinRpcError> {
        let request = QueryDelegatorUnbondingDelegationsRequest {
            delegator_addr: self.account_id.to_string(),
            pagination: Some(PageRequest {
                key: vec![],
                offset: ((paging.page_number.get() - 1usize) * paging.limit) as u64,
                limit: paging.limit as u64,
                count_total: false,
                reverse: false,
            }),
        };

        let raw_response = self
            .rpc_client()
            .await?
            .abci_query(
                Some(ABCI_DELEGATOR_UNDELEGATIONS_PATH.to_owned()),
                request.encode_to_vec(),
                ABCI_REQUEST_HEIGHT,
                ABCI_REQUEST_PROVE,
            )
            .await?;

        let decoded_proto = QueryDelegatorUnbondingDelegationsResponse::decode(raw_response.value.as_slice())?;
        let ongoing_undelegations = decoded_proto
            .unbonding_responses
            .into_iter()
            .map(|r| {
                let entries = r
                    .entries
                    .into_iter()
                    .filter_map(|e| {
                        let balance: u64 = e.balance.parse().ok()?;

                        Some(UndelegationEntry {
                            creation_height: e.creation_height,
                            completion_datetime: e.completion_time?.to_string(),
                            balance: big_decimal_from_sat_unsigned(balance, self.decimals()),
                        })
                    })
                    .collect();

                Undelegation {
                    validator_address: r.validator_address,
                    entries,
                }
            })
            .collect();

        Ok(UndelegationsQueryResponse { ongoing_undelegations })
    }
}

fn clients_from_urls(ctx: &MmArc, nodes: Vec<RpcNode>) -> MmResult<Vec<HttpClient>, TendermintInitErrorKind> {
    if nodes.is_empty() {
        return MmError::err(TendermintInitErrorKind::EmptyRpcUrls);
    }

    let p2p_keypair = if nodes.iter().any(|n| n.komodo_proxy) {
        let p2p_ctx = P2PContext::fetch_from_mm_arc(ctx);
        Some(p2p_ctx.keypair().clone())
    } else {
        None
    };

    let mut clients = Vec::new();
    let mut errors = Vec::new();

    // check that all urls are valid
    // keep all invalid urls in one vector to show all of them in error
    for node in nodes.iter() {
        let proxy_sign_keypair = if node.komodo_proxy { p2p_keypair.clone() } else { None };
        match HttpClient::new(node.url.as_str(), proxy_sign_keypair) {
            Ok(client) => clients.push(client),
            Err(e) => errors.push(format!("Url {} is invalid, got error {}", node.url, e)),
        }
    }
    drop_mutability!(clients);
    drop_mutability!(errors);
    if !errors.is_empty() {
        let errors: String = errors.into_iter().join(", ");
        return MmError::err(TendermintInitErrorKind::RpcClientInitError(errors));
    }
    Ok(clients)
}

#[async_trait]
#[allow(unused_variables)]
impl MmCoin for TendermintCoin {
    fn is_asset_chain(&self) -> bool {
        false
    }

    #[cfg(feature = "ibc-routing-for-swaps")]
    fn wallet_only(&self, ctx: &MmArc) -> bool {
        // Keplr with Ledger does not support some transactions like HTLC due to
        // the transaction format they use. As HTLC is part of our swap system's DNA,
        // treat any Tendermint asset as wallet-only.
        //
        // TODO: Once `SIGN_MODE_DIRECT` is supported, we can remove this.
        if self.is_ledger_connection() {
            common::log::info!("Using Keplr with Ledger: operating in wallet only mode.");
            return true;
        }

        let coin_conf = crate::coin_conf(ctx, self.ticker());
        let wallet_only_conf = coin_conf
            .get("wallet_only")
            .unwrap_or(&json!(false))
            .as_bool()
            .unwrap_or(false);

        if wallet_only_conf {
            warn!("`wallet_only` option cannot be set to true for Tendermint assets. This setting will be ignored.");
        }

        false
    }

    #[cfg(not(feature = "ibc-routing-for-swaps"))]
    fn wallet_only(&self, ctx: &MmArc) -> bool {
        let coin_conf = crate::coin_conf(ctx, self.ticker());
        // If coin is not in config, it means that it was added manually (a custom token) and should be treated as wallet only
        if coin_conf.is_null() {
            return true;
        }
        let wallet_only_conf = coin_conf["wallet_only"].as_bool().unwrap_or(false);

        wallet_only_conf || self.is_ledger_connection()
    }

    fn spawner(&self) -> WeakSpawner {
        self.abortable_system.weak_spawner()
    }

    fn withdraw(&self, req: WithdrawRequest) -> WithdrawFut {
        let coin = self.clone();
        let fut = async move {
            let to_address =
                AccountId::from_str(&req.to).map_to_mm(|e| WithdrawError::InvalidAddress(e.to_string()))?;

            let is_ibc_transfer =
                to_address.prefix() != coin.protocol_info.account_prefix || req.ibc_source_channel.is_some();

            let (account_id, maybe_priv_key) = coin
                .extract_account_id_and_private_key(req.from)
                .map_err(|e| WithdrawError::InternalError(e.to_string()))?;

            let (balance_denom, balance_dec) = coin
                .get_balance_as_unsigned_and_decimal(&account_id, &coin.protocol_info.denom, coin.decimals())
                .await
                .map_mm_err()?;

            let (amount_denom, amount_dec) = if req.max {
                let amount_denom = balance_denom;
                (
                    amount_denom,
                    big_decimal_from_sat_unsigned(amount_denom, coin.decimals()),
                )
            } else {
                (
                    sat_from_big_decimal(&req.amount, coin.decimals()).map_mm_err()?,
                    req.amount.clone(),
                )
            };

            if !coin.is_tx_amount_enough(coin.decimals(), &amount_dec) {
                return MmError::err(WithdrawError::AmountTooLow {
                    amount: amount_dec,
                    threshold: coin.min_tx_amount(),
                });
            }

            let received_by_me = if to_address == account_id {
                amount_dec
            } else {
                BigDecimal::default()
            };

            let channel_id = if is_ibc_transfer {
                match &req.ibc_source_channel {
                    Some(_) => req.ibc_source_channel,
                    None => Some(
                        coin.get_healthy_ibc_channel_for_address_prefix(to_address.prefix())
                            .await
                            .map_mm_err()?,
                    ),
                }
            } else {
                None
            };

            let msg_payload = create_withdraw_msg_as_any(
                account_id.clone(),
                to_address.clone(),
                &coin.protocol_info.denom,
                amount_denom,
                channel_id,
            )
            .await?;

            let memo = req.memo.unwrap_or_else(|| TX_DEFAULT_MEMO.into());

            let current_block = coin
                .current_block()
                .compat()
                .await
                .map_to_mm(WithdrawError::Transport)?;

            let timeout_height = current_block + TIMEOUT_HEIGHT_DELTA;

            let (_, gas_limit) = if is_ibc_transfer {
                coin.gas_info_for_withdraw(&req.fee, IBC_GAS_LIMIT_DEFAULT)
            } else {
                coin.gas_info_for_withdraw(&req.fee, GAS_LIMIT_DEFAULT)
            };

            let fee_amount_u64 = coin
                .calculate_account_fee_amount_as_u64(
                    &account_id,
                    maybe_priv_key,
                    msg_payload.clone(),
                    timeout_height,
                    &memo,
                    req.fee,
                )
                .await
                .map_mm_err()?;

            let fee_amount_u64 = if coin.is_ledger_connection() {
                // When using `SIGN_MODE_LEGACY_AMINO_JSON`, Keplr ignores the fee we calculated
                // and calculates another one which is usually double what we calculate.
                // To make sure the transaction doesn't fail on the Keplr side (because if Keplr
                // calculates a higher fee than us, the withdrawal might fail), we use three times
                // the actual fee.
                fee_amount_u64 * 3
            } else if is_ibc_transfer {
                fee_amount_u64 * 3 / 2
            } else {
                fee_amount_u64
            };

            let fee_amount_dec = big_decimal_from_sat_unsigned(fee_amount_u64, coin.decimals());

            let fee_amount = Coin {
                denom: coin.protocol_info.denom.clone(),
                amount: fee_amount_u64.into(),
            };

            let fee = Fee::from_amount_and_gas(fee_amount, gas_limit);

            let (amount_denom, total_amount) = if req.max {
                if balance_denom < fee_amount_u64 {
                    return MmError::err(WithdrawError::NotSufficientBalance {
                        coin: coin.ticker.clone(),
                        available: balance_dec,
                        required: fee_amount_dec,
                    });
                }
                let amount_denom = balance_denom - fee_amount_u64;
                (amount_denom, balance_dec)
            } else {
                let total = &req.amount + &fee_amount_dec;
                if balance_dec < total {
                    return MmError::err(WithdrawError::NotSufficientBalance {
                        coin: coin.ticker.clone(),
                        available: balance_dec,
                        required: total,
                    });
                }

                (sat_from_big_decimal(&req.amount, coin.decimals()).map_mm_err()?, total)
            };

            let msg_payload = create_withdraw_msg_as_any(
                account_id.clone(),
                to_address.clone(),
                &coin.protocol_info.denom,
                amount_denom,
                channel_id,
            )
            .await?;

            let account_info = coin.account_info(&account_id).await.map_mm_err()?;

            let tx = coin
                .any_to_transaction_data(maybe_priv_key, msg_payload, &account_info, fee, timeout_height, &memo)
                .await
                .map_to_mm(|e| WithdrawError::InternalError(e.to_string()))?;

            let internal_id = tendermint_tx_internal_id(tx.tx_hash().unwrap_or_default().as_bytes(), None);

            Ok(TransactionDetails {
                tx,
                from: vec![account_id.to_string()],
                to: vec![req.to],
                my_balance_change: &received_by_me - &total_amount,
                spent_by_me: total_amount.clone(),
                total_amount,
                received_by_me,
                block_height: 0,
                timestamp: 0,
                fee_details: Some(TxFeeDetails::Tendermint(TendermintFeeDetails {
                    coin: coin.ticker.clone(),
                    amount: fee_amount_dec,
                    uamount: fee_amount_u64,
                    gas_limit,
                })),
                coin: coin.ticker.to_string(),
                internal_id,
                kmd_rewards: None,
                transaction_type: if is_ibc_transfer {
                    TransactionType::TendermintIBCTransfer { token_id: None }
                } else {
                    TransactionType::StandardTransfer
                },
                memo: Some(memo),
            })
        };
        Box::new(fut.boxed().compat())
    }

    fn get_raw_transaction(&self, mut req: RawTransactionRequest) -> RawTransactionFut<'_> {
        let coin = self.clone();
        let fut = async move {
            req.tx_hash.make_ascii_uppercase();
            let tx_from_rpc = coin.request_tx(req.tx_hash).await.map_mm_err()?;
            Ok(RawTransactionRes {
                tx_hex: tx_from_rpc.encode_to_vec().into(),
            })
        };
        Box::new(fut.boxed().compat())
    }

    fn get_tx_hex_by_hash(&self, tx_hash: Vec<u8>) -> RawTransactionFut<'_> {
        let coin = self.clone();
        let fut = async move {
            let len = tx_hash.len();
            let hash: [u8; 32] = tx_hash.try_into().map_to_mm(|_| {
                RawTransactionError::InvalidHashError(format!("Invalid hash length: expected 32, got {len}"))
            })?;
            let hash = hex::encode_upper(H256::from(hash));
            let tx_from_rpc = coin.request_tx(hash).await.map_mm_err()?;
            Ok(RawTransactionRes {
                tx_hex: tx_from_rpc.encode_to_vec().into(),
            })
        };
        Box::new(fut.boxed().compat())
    }

    fn decimals(&self) -> u8 {
        self.protocol_info.decimals
    }

    fn convert_to_address(&self, from: &str, to_address_format: Json) -> Result<String, String> {
        // TODO
        Err("Not implemented".into())
    }

    fn validate_address(&self, address: &str) -> ValidateAddressResult {
        match AccountId::from_str(address) {
            Ok(_) => ValidateAddressResult {
                is_valid: true,
                reason: None,
            },
            Err(e) => ValidateAddressResult {
                is_valid: false,
                reason: Some(e.to_string()),
            },
        }
    }

    fn process_history_loop(&self, ctx: MmArc) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        warn!("process_history_loop is deprecated, tendermint uses tx_history_v2");
        Box::new(futures01::future::err(()))
    }

    fn history_sync_status(&self) -> HistorySyncState {
        self.history_sync_state.lock().unwrap().clone()
    }

    fn get_trade_fee(&self) -> Box<dyn Future<Item = TradeFee, Error = String> + Send> {
        Box::new(futures01::future::err("Not implemented".into()))
    }

    async fn get_sender_trade_fee(
        &self,
        value: TradePreimageValue,
        _stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        let amount = match value {
            TradePreimageValue::Exact(decimal) | TradePreimageValue::UpperBound(decimal) => decimal,
        };
        self.get_sender_trade_fee_for_denom(
            self.ticker.clone(),
            self.protocol_info.denom.clone(),
            self.protocol_info.decimals,
            amount,
        )
        .await
    }

    /// Overrides the default `pre_check_for_order_creation` implementation with
    /// additional IBC-related logic on top of the default behavior.
    #[cfg(feature = "ibc-routing-for-swaps")]
    async fn pre_check_for_order_creation(
        &self,
        ctx: &MmArc,
        rel_coin: &crate::MmCoinEnum,
    ) -> MmResult<(), crate::OrderCreationPreCheckError> {
        use crate::{lp_coinfind, MmCoinEnum, OrderCreationPreCheckError};

        /// Looks for a Tendermint platform coin by the given ticker.
        ///
        /// Returns `Ok(Some(...))` if the coin exists and is a Tendermint platform coin,
        /// `Ok(None)` if it's not active, or an error if somethings goes wrong or the ticker
        /// isn't belongs to a Tendermint platform coin.
        async fn find_tendermint_platform_coin(
            ctx: &MmArc,
            ticker: &str,
        ) -> Result<Option<TendermintCoin>, MmError<OrderCreationPreCheckError>> {
            match lp_coinfind(ctx, ticker).await {
                Ok(Some(MmCoinEnum::Tendermint(coin))) => Ok(Some(coin)),
                Ok(Some(other)) => MmError::err(OrderCreationPreCheckError::InternalError {
                    reason: format!(
                        "Expected a Tendermint coin for '{}', but found '{}'.",
                        ticker,
                        other.ticker()
                    ),
                }),
                Ok(None) => Ok(None),
                Err(reason) => MmError::err(OrderCreationPreCheckError::PreCheckFailed { reason }),
            }
        }

        /// Picks an HTLC coin (IRIS or NUCLEUS) based on which IBC channel is configured
        /// and is healthy.
        async fn get_htlc_coin(
            coin: &TendermintCoin,
            ctx: &MmArc,
        ) -> Result<Option<TendermintCoin>, MmError<OrderCreationPreCheckError>> {
            const IRIS_TICKER: &str = "IRIS";
            const NUCLEUS_TICKER: &str = "NUCLEUS";

            if coin
                .get_healthy_ibc_channel_for_address_prefix(IRIS_PREFIX)
                .await
                .is_ok()
            {
                return find_tendermint_platform_coin(ctx, IRIS_TICKER).await;
            }

            if coin
                .get_healthy_ibc_channel_for_address_prefix(NUCLEUS_PREFIX)
                .await
                .is_ok()
            {
                return find_tendermint_platform_coin(ctx, NUCLEUS_TICKER).await;
            }

            MmError::err(OrderCreationPreCheckError::PreCheckFailed {
                reason: format!("No healthy IBC channel found for {}.", coin.ticker()),
            })
        }

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

        if self.supports_htlc() {
            return Ok(());
        }

        // If `self` is not an HTLC-supported coin, we need to check a few things when creating the order:
        //  - Is there an HTLC coin enabled?
        //  - Does that HTLC network have an IBC channel configured to `self` network?
        //  - Does that HTLC coin have enough balance to handle IBC routing?

        let Some(htlc_coin) = get_htlc_coin(self, ctx).await? else {
            return MmError::err(OrderCreationPreCheckError::PreCheckFailed {
                reason: "No HTLC coin is currently enabled. Please enable either Iris or Nucleus.".into(),
            });
        };

        let my_balance = htlc_coin
            .my_balance()
            .compat()
            .await
            .map_err(|e| OrderCreationPreCheckError::InternalError { reason: e.to_string() })?
            .spendable;

        let min_balance_for_ibc_routing = htlc_coin
            .protocol_info
            .min_balance_for_ibc_routing
            .unwrap_or(DEFAULT_MIN_BALANCE_FOR_IBC_ROUTING);
        let min_balance_for_ibc_routing = BigDecimal::try_from(min_balance_for_ibc_routing)
            .map_err(|e| OrderCreationPreCheckError::InternalError { reason: e.to_string() })?;

        if min_balance_for_ibc_routing > my_balance {
            let htlc_ticker = htlc_coin.ticker();
            let self_ticker = self.ticker();
            let reason = format!(
                "Insufficient balance on HTLC coin ({htlc_ticker}) for making orders with {self_ticker}. Minimum required expected balance {min_balance_for_ibc_routing}, current balance {my_balance}.",
            );
            return MmError::err(OrderCreationPreCheckError::PreCheckFailed { reason });
        }

        Ok(())
    }

    fn get_receiver_trade_fee(&self, stage: FeeApproxStage) -> TradePreimageFut<TradeFee> {
        let coin = self.clone();
        let fut = async move {
            // We can't simulate Claim Htlc without having information about broadcasted htlc tx.
            // Since create and claim htlc fees are almost same, we can simply simulate create htlc tx.
            coin.get_sender_trade_fee_for_denom(
                coin.ticker.clone(),
                coin.protocol_info.denom.clone(),
                coin.decimals(),
                coin.min_tx_amount(),
            )
            .await
        };
        Box::new(fut.boxed().compat())
    }

    async fn get_fee_to_send_taker_fee(
        &self,
        dex_fee_amount: DexFee,
        _stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        self.get_fee_to_send_taker_fee_for_denom(
            self.ticker.clone(),
            self.protocol_info.denom.clone(),
            self.protocol_info.decimals,
            dex_fee_amount,
        )
        .await
    }

    fn required_confirmations(&self) -> u64 {
        0
    }

    fn requires_notarization(&self) -> bool {
        false
    }

    fn set_required_confirmations(&self, confirmations: u64) {
        warn!("set_required_confirmations is not supported for tendermint")
    }

    fn set_requires_notarization(&self, requires_nota: bool) {
        warn!("TendermintCoin doesn't support notarization")
    }

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
        AbortableSystem::abort_all(&self.abortable_system)
    }

    fn on_token_deactivated(&self, _ticker: &str) {}
}

#[async_trait]
impl MarketCoinOps for TendermintCoin {
    fn ticker(&self) -> &str {
        &self.ticker
    }

    fn my_address(&self) -> MmResult<String, MyAddressError> {
        Ok(self.account_id.to_string())
    }

    fn address_from_pubkey(&self, pubkey: &H264Json) -> MmResult<String, AddressFromPubkeyError> {
        let address = account_id_from_raw_pubkey(&self.protocol_info.account_prefix, &pubkey.0)
            .map_err(|e| AddressFromPubkeyError::InternalError(e.to_string()))?;
        Ok(address.to_string())
    }

    async fn get_public_key(&self) -> Result<String, MmError<UnexpectedDerivationMethod>> {
        let key = SigningKey::from_slice(self.activation_policy.activated_key_or_err().map_mm_err()?.as_slice())
            .expect("privkey validity is checked on coin creation");
        Ok(key.public_key().to_string())
    }

    fn sign_message_hash(&self, _message: &str) -> Option<[u8; 32]> {
        // TODO
        None
    }

    fn sign_message(&self, _message: &str, _address: Option<HDAddressSelector>) -> SignatureResult<String> {
        // TODO
        MmError::err(SignatureError::InternalError("Not implemented".into()))
    }

    fn verify_message(&self, _signature: &str, _message: &str, _address: &str) -> VerificationResult<bool> {
        // TODO
        MmError::err(VerificationError::InternalError("Not implemented".into()))
    }

    fn my_balance(&self) -> BalanceFut<CoinBalance> {
        let coin = self.clone();
        let fut = async move {
            let balance_denom = coin
                .account_balance_for_denom(&coin.account_id, coin.protocol_info.denom.to_string())
                .await
                .map_mm_err()?;
            Ok(CoinBalance {
                spendable: big_decimal_from_sat_unsigned(balance_denom, coin.decimals()),
                unspendable: BigDecimal::default(),
            })
        };
        Box::new(fut.boxed().compat())
    }

    fn platform_coin_balance(&self) -> BalanceFut<BigDecimal> {
        Box::new(self.my_balance().map(|coin_balance| coin_balance.spendable))
    }

    fn platform_ticker(&self) -> &str {
        &self.ticker
    }

    fn send_raw_tx(&self, tx: &str) -> Box<dyn Future<Item = String, Error = String> + Send> {
        let tx_bytes = try_fus!(hex::decode(tx));
        self.send_raw_tx_bytes(&tx_bytes)
    }

    /// Consider using `seq_safe_send_raw_tx_bytes` instead.
    /// This is considered as unsafe due to sequence mismatches.
    fn send_raw_tx_bytes(&self, tx: &[u8]) -> Box<dyn Future<Item = String, Error = String> + Send> {
        // as sanity check
        try_fus!(Raw::from_bytes(tx));

        let coin = self.clone();
        let tx_bytes = tx.to_owned();
        let fut = async move {
            let broadcast_res = try_s!(try_s!(coin.rpc_client().await).broadcast_tx_commit(tx_bytes).await);

            if broadcast_res.check_tx.log.contains(ACCOUNT_SEQUENCE_ERR)
                || broadcast_res.tx_result.log.contains(ACCOUNT_SEQUENCE_ERR)
            {
                return ERR!(
                    "{}. check_tx log: {}, deliver_tx log: {}",
                    ACCOUNT_SEQUENCE_ERR,
                    broadcast_res.check_tx.log,
                    broadcast_res.tx_result.log
                );
            }

            if !broadcast_res.check_tx.code.is_ok() {
                return ERR!("Tx check failed {:?}", broadcast_res.check_tx);
            }

            if !broadcast_res.tx_result.code.is_ok() {
                return ERR!("Tx deliver failed {:?}", broadcast_res.tx_result);
            }
            Ok(broadcast_res.hash.to_string())
        };
        Box::new(fut.boxed().compat())
    }

    #[inline(always)]
    async fn sign_raw_tx(&self, _args: &SignRawTransactionRequest) -> RawTransactionResult {
        MmError::err(RawTransactionError::NotImplemented {
            coin: self.ticker().to_string(),
        })
    }

    fn wait_for_confirmations(&self, input: ConfirmPaymentInput) -> Box<dyn Future<Item = (), Error = String> + Send> {
        // Sanity check
        let _: TxRaw = try_fus!(Message::decode(input.payment_tx.as_slice()));

        let tx_hash = hex::encode_upper(sha256(&input.payment_tx));

        let coin = self.clone();
        let fut = async move {
            loop {
                if now_sec() > input.wait_until {
                    return ERR!(
                        "Waited too long until {} for payment {} to be received",
                        input.wait_until,
                        tx_hash.clone()
                    );
                }

                let tx_status_code = try_s!(coin.get_tx_status_code_or_none(tx_hash.clone()).await);

                if let Some(tx_status_code) = tx_status_code {
                    return match tx_status_code {
                        cosmrs::tendermint::abci::Code::Ok => Ok(()),
                        cosmrs::tendermint::abci::Code::Err(err_code) => Err(format!(
                            "Got error code: '{err_code}' for tx: '{tx_hash}'. Broadcasted tx isn't valid."
                        )),
                    };
                };

                Timer::sleep(input.check_every as f64).await;
            }
        };

        Box::new(fut.boxed().compat())
    }

    async fn wait_for_htlc_tx_spend(&self, args: WaitForHTLCTxSpendArgs<'_>) -> TransactionResult {
        let tx = try_tx_s!(cosmrs::Tx::from_bytes(args.tx_bytes));
        let first_message = try_tx_s!(tx.body.messages.first().ok_or("Tx body couldn't be read."));
        let htlc_proto = try_tx_s!(CreateHtlcProto::decode(
            try_tx_s!(HtlcType::from_str(&self.protocol_info.account_prefix)),
            first_message.value.as_slice()
        ));
        let htlc = try_tx_s!(CreateHtlcMsg::try_from(htlc_proto));
        let htlc_id = self.calculate_htlc_id(htlc.sender(), htlc.to(), htlc.amount(), args.secret_hash);

        let query = format!("claim_htlc.id='{htlc_id}'");
        let request = TxSearchRequest {
            query,
            order_by: TendermintResultOrder::Ascending.into(),
            page: 1,
            per_page: 1,
            prove: false,
        };

        loop {
            let response = try_tx_s!(try_tx_s!(self.rpc_client().await).perform(request.clone()).await);

            if let Some(raw_tx) = response.txs.first() {
                let tx = try_tx_s!(cosmrs::Tx::from_bytes(&raw_tx.tx));

                return Ok(TransactionEnum::CosmosTransaction(CosmosTransaction {
                    data: TxRaw {
                        body_bytes: try_tx_s!(tx.body.into_bytes()),
                        auth_info_bytes: try_tx_s!(tx.auth_info.into_bytes()),
                        signatures: tx.signatures,
                    },
                }));
            }
            Timer::sleep(5.).await;
            if get_utc_timestamp() > args.wait_until as i64 {
                return Err(TransactionErr::Plain("Waited too long".into()));
            }
        }
    }

    fn tx_enum_from_bytes(&self, bytes: &[u8]) -> Result<TransactionEnum, MmError<TxMarshalingErr>> {
        let tx_raw: TxRaw = Message::decode(bytes).map_to_mm(|e| TxMarshalingErr::InvalidInput(e.to_string()))?;
        Ok(TransactionEnum::CosmosTransaction(CosmosTransaction { data: tx_raw }))
    }

    fn current_block(&self) -> Box<dyn Future<Item = u64, Error = String> + Send> {
        let coin = self.clone();
        let fut = async move {
            let info = try_s!(try_s!(coin.rpc_client().await).abci_info().await);
            Ok(info.response.last_block_height.into())
        };
        Box::new(fut.boxed().compat())
    }

    fn display_priv_key(&self) -> Result<String, String> {
        Ok(self
            .activation_policy
            .activated_key_or_err()
            .map_err(|e| e.to_string())?
            .to_string())
    }

    #[inline]
    fn min_tx_amount(&self) -> BigDecimal {
        big_decimal_from_sat(MIN_TX_SATOSHIS, self.protocol_info.decimals)
    }

    #[inline]
    fn min_trading_vol(&self) -> MmNumber {
        self.min_tx_amount().into()
    }

    #[inline]
    fn should_burn_dex_fee(&self) -> bool {
        false
    } // TODO: fix back to true when negotiation version added

    fn is_trezor(&self) -> bool {
        match &self.activation_policy {
            TendermintActivationPolicy::PrivateKey(pk) => pk.is_trezor(),
            TendermintActivationPolicy::PublicKey(_) => false,
        }
    }
}

#[async_trait]
#[allow(unused_variables)]
impl SwapOps for TendermintCoin {
    async fn send_taker_fee(&self, dex_fee: DexFee, uuid: &[u8], expire_at: u64) -> TransactionResult {
        self.send_taker_fee_for_denom(
            &dex_fee,
            self.protocol_info.denom.clone(),
            self.protocol_info.decimals,
            uuid,
            expire_at,
        )
        .compat()
        .await
    }

    async fn send_maker_payment(&self, maker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        self.send_htlc_for_denom(
            maker_payment_args.time_lock_duration,
            maker_payment_args.other_pubkey,
            maker_payment_args.secret_hash,
            maker_payment_args.amount,
            self.protocol_info.denom.clone(),
            self.protocol_info.decimals,
        )
        .compat()
        .await
    }

    async fn send_taker_payment(&self, taker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        self.send_htlc_for_denom(
            taker_payment_args.time_lock_duration,
            taker_payment_args.other_pubkey,
            taker_payment_args.secret_hash,
            taker_payment_args.amount,
            self.protocol_info.denom.clone(),
            self.protocol_info.decimals,
        )
        .compat()
        .await
    }

    // TODO: release this function once watchers are supported
    // fn is_supported_by_watchers(&self) -> bool {
    //     !matches!(self.activation_policy, TendermintActivationPolicy::PublicKey(_))
    // }

    async fn send_maker_spends_taker_payment(
        &self,
        maker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        let tx = try_tx_s!(cosmrs::Tx::from_bytes(maker_spends_payment_args.other_payment_tx));
        let msg = try_tx_s!(tx.body.messages.first().ok_or("Tx body couldn't be read."));

        let htlc_proto = try_tx_s!(CreateHtlcProto::decode(
            try_tx_s!(HtlcType::from_str(&self.protocol_info.account_prefix)),
            msg.value.as_slice()
        ));
        let htlc = try_tx_s!(CreateHtlcMsg::try_from(htlc_proto));

        let mut amount = htlc.amount().to_vec();
        amount.sort();
        drop_mutability!(amount);

        let coins_string = amount
            .iter()
            .map(|t| format!("{}{}", t.amount, t.denom))
            .collect::<Vec<String>>()
            .join(",");

        let htlc_id = self.calculate_htlc_id(htlc.sender(), htlc.to(), &amount, maker_spends_payment_args.secret_hash);

        let claim_htlc_tx = try_tx_s!(self.gen_claim_htlc_tx(htlc_id, maker_spends_payment_args.secret));
        let timeout = maker_spends_payment_args
            .time_lock
            .checked_sub(now_sec())
            .unwrap_or_default();
        let coin = self.clone();

        let current_block = try_tx_s!(self.current_block().compat().await);
        let timeout_height = current_block + TIMEOUT_HEIGHT_DELTA;

        let fee = try_tx_s!(
            self.calculate_fee(claim_htlc_tx.msg_payload.clone(), timeout_height, TX_DEFAULT_MEMO, None)
                .await
        );

        let (_tx_id, tx_raw) = try_tx_s!(
            coin.common_send_raw_tx_bytes(
                claim_htlc_tx.msg_payload.clone(),
                fee.clone(),
                timeout_height,
                TX_DEFAULT_MEMO,
                Duration::from_secs(timeout),
            )
            .await
        );

        Ok(TransactionEnum::CosmosTransaction(CosmosTransaction {
            data: tx_raw.into(),
        }))
    }

    async fn send_taker_spends_maker_payment(
        &self,
        taker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        let tx = try_tx_s!(cosmrs::Tx::from_bytes(taker_spends_payment_args.other_payment_tx));
        let msg = try_tx_s!(tx.body.messages.first().ok_or("Tx body couldn't be read."));

        let htlc_proto = try_tx_s!(CreateHtlcProto::decode(
            try_tx_s!(HtlcType::from_str(&self.protocol_info.account_prefix)),
            msg.value.as_slice()
        ));
        let htlc = try_tx_s!(CreateHtlcMsg::try_from(htlc_proto));

        let mut amount = htlc.amount().to_vec();
        amount.sort();
        drop_mutability!(amount);

        let coins_string = amount
            .iter()
            .map(|t| format!("{}{}", t.amount, t.denom))
            .collect::<Vec<String>>()
            .join(",");

        let htlc_id = self.calculate_htlc_id(htlc.sender(), htlc.to(), &amount, taker_spends_payment_args.secret_hash);

        let timeout = taker_spends_payment_args
            .time_lock
            .checked_sub(now_sec())
            .unwrap_or_default();
        let claim_htlc_tx = try_tx_s!(self.gen_claim_htlc_tx(htlc_id, taker_spends_payment_args.secret));
        let coin = self.clone();

        let current_block = try_tx_s!(self.current_block().compat().await);
        let timeout_height = current_block + TIMEOUT_HEIGHT_DELTA;

        let fee = try_tx_s!(
            self.calculate_fee(claim_htlc_tx.msg_payload.clone(), timeout_height, TX_DEFAULT_MEMO, None)
                .await
        );

        let (tx_id, tx_raw) = try_tx_s!(
            coin.common_send_raw_tx_bytes(
                claim_htlc_tx.msg_payload.clone(),
                fee.clone(),
                timeout_height,
                TX_DEFAULT_MEMO,
                Duration::from_secs(timeout),
            )
            .await
        );

        Ok(TransactionEnum::CosmosTransaction(CosmosTransaction {
            data: tx_raw.into(),
        }))
    }

    async fn send_taker_refunds_payment(&self, taker_refunds_payment_args: RefundPaymentArgs<'_>) -> TransactionResult {
        Err(TransactionErr::Plain(
            "Doesn't need transaction broadcast to refund IRIS HTLC".into(),
        ))
    }

    async fn send_maker_refunds_payment(&self, maker_refunds_payment_args: RefundPaymentArgs<'_>) -> TransactionResult {
        Err(TransactionErr::Plain(
            "Doesn't need transaction broadcast to refund IRIS HTLC".into(),
        ))
    }

    async fn validate_fee(&self, validate_fee_args: ValidateFeeArgs<'_>) -> ValidatePaymentResult<()> {
        self.validate_fee_for_denom(
            validate_fee_args.fee_tx,
            validate_fee_args.expected_sender,
            validate_fee_args.dex_fee,
            self.protocol_info.decimals,
            validate_fee_args.uuid,
            self.protocol_info.denom.to_string(),
        )
        .compat()
        .await
    }

    async fn validate_maker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        self.validate_payment_for_denom(input, self.protocol_info.denom.clone(), self.protocol_info.decimals)
            .await
    }

    async fn validate_taker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        self.validate_payment_for_denom(input, self.protocol_info.denom.clone(), self.protocol_info.decimals)
            .await
    }

    async fn check_if_my_payment_sent(
        &self,
        if_my_payment_sent_args: CheckIfMyPaymentSentArgs<'_>,
    ) -> Result<Option<TransactionEnum>, String> {
        self.check_if_my_payment_sent_for_denom(
            self.protocol_info.decimals,
            self.protocol_info.denom.clone(),
            if_my_payment_sent_args.other_pub,
            if_my_payment_sent_args.secret_hash,
            if_my_payment_sent_args.amount,
        )
        .compat()
        .await
    }

    async fn search_for_swap_tx_spend_my(
        &self,
        input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        self.search_for_swap_tx_spend(input).await.map_err(|e| e.to_string())
    }

    async fn search_for_swap_tx_spend_other(
        &self,
        input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        self.search_for_swap_tx_spend(input).await.map_err(|e| e.to_string())
    }

    async fn extract_secret(
        &self,
        secret_hash: &[u8],
        spend_tx: &[u8],
        watcher_reward: bool,
    ) -> Result<[u8; 32], String> {
        let tx = try_s!(cosmrs::Tx::from_bytes(spend_tx));
        let msg = try_s!(tx.body.messages.first().ok_or("Tx body couldn't be read."));

        let htlc_proto = try_s!(ClaimHtlcProto::decode(
            try_s!(HtlcType::from_str(&self.protocol_info.account_prefix)),
            msg.value.as_slice()
        ));
        let htlc = try_s!(ClaimHtlcMsg::try_from(htlc_proto));

        Ok(try_s!(try_s!(hex::decode(htlc.secret())).as_slice().try_into()))
    }

    fn negotiate_swap_contract_addr(
        &self,
        other_side_address: Option<&[u8]>,
    ) -> Result<Option<BytesJson>, MmError<NegotiateSwapContractAddrErr>> {
        Ok(None)
    }

    #[inline]
    fn derive_htlc_key_pair(&self, _swap_unique_data: &[u8]) -> KeyPair {
        key_pair_from_secret(
            &self
                .activation_policy
                .activated_key_or_err()
                .expect("valid priv key")
                .take(),
        )
        .expect("valid priv key")
    }

    #[inline]
    fn derive_htlc_pubkey(&self, _swap_unique_data: &[u8]) -> [u8; 33] {
        let mut res = [0u8; 33];
        res.copy_from_slice(&self.activation_policy.public_key().expect("valid pubkey").to_bytes());
        res
    }

    fn validate_other_pubkey(&self, raw_pubkey: &[u8]) -> MmResult<(), ValidateOtherPubKeyErr> {
        PublicKey::from_raw_secp256k1(raw_pubkey)
            .or_mm_err(|| ValidateOtherPubKeyErr::InvalidPubKey(hex::encode(raw_pubkey)))?;
        Ok(())
    }
}

#[async_trait]
impl WatcherOps for TendermintCoin {}

/// Processes the given `priv_key_build_policy` and returns corresponding `TendermintPrivKeyPolicy`.
/// This function expects either [`PrivKeyBuildPolicy::IguanaPrivKey`]
/// or [`PrivKeyBuildPolicy::GlobalHDAccount`], otherwise returns `PrivKeyPolicyNotAllowed` error.
pub fn tendermint_priv_key_policy(
    conf: &TendermintConf,
    ticker: &str,
    priv_key_build_policy: PrivKeyBuildPolicy,
    path_to_address: HDPathAccountToAddressId,
) -> MmResult<TendermintPrivKeyPolicy, TendermintInitError> {
    match priv_key_build_policy {
        PrivKeyBuildPolicy::IguanaPrivKey(iguana) => {
            let mm2_internal_key_pair = key_pair_from_secret(&iguana.take()).mm_err(|e| TendermintInitError {
                ticker: ticker.to_string(),
                kind: TendermintInitErrorKind::Internal(e.to_string()),
            })?;

            let tendermint_pair = TendermintKeyPair::new(iguana, *mm2_internal_key_pair.public());

            Ok(TendermintPrivKeyPolicy::Iguana(tendermint_pair))
        },
        PrivKeyBuildPolicy::GlobalHDAccount(global_hd) => {
            let path_to_coin = conf.derivation_path.as_ref().or_mm_err(|| TendermintInitError {
                ticker: ticker.to_string(),
                kind: TendermintInitErrorKind::DerivationPathIsNotSet,
            })?;
            let activated_priv_key = global_hd
                .derive_secp256k1_secret(&path_to_address.to_derivation_path(path_to_coin).mm_err(|e| {
                    TendermintInitError {
                        ticker: ticker.to_string(),
                        kind: TendermintInitErrorKind::InvalidPathToAddress(e.to_string()),
                    }
                })?)
                .mm_err(|e| TendermintInitError {
                    ticker: ticker.to_string(),
                    kind: TendermintInitErrorKind::InvalidPrivKey(e.to_string()),
                })?;
            let bip39_secp_priv_key = global_hd.root_priv_key().clone();
            let pubkey = Public::from_slice(&bip39_secp_priv_key.public_key().to_bytes()).map_to_mm(|e| {
                TendermintInitError {
                    ticker: ticker.to_string(),
                    kind: TendermintInitErrorKind::Internal(e.to_string()),
                }
            })?;

            let tendermint_pair = TendermintKeyPair::new(activated_priv_key, pubkey);

            Ok(TendermintPrivKeyPolicy::HDWallet {
                path_to_coin: path_to_coin.clone(),
                activated_key: tendermint_pair,
                bip39_secp_priv_key,
            })
        },
        PrivKeyBuildPolicy::Trezor => {
            let kind =
                TendermintInitErrorKind::PrivKeyPolicyNotAllowed(PrivKeyPolicyNotAllowed::HardwareWalletNotSupported);
            MmError::err(TendermintInitError {
                ticker: ticker.to_string(),
                kind,
            })
        },
        PrivKeyBuildPolicy::WalletConnect { .. } => {
            let kind = TendermintInitErrorKind::PrivKeyPolicyNotAllowed(PrivKeyPolicyNotAllowed::UnsupportedMethod(
                "Cannot use WalletConnect to get TendermintPrivKeyPolicy".to_string(),
            ));
            MmError::err(TendermintInitError {
                ticker: ticker.to_string(),
                kind,
            })
        },
    }
}

pub(crate) async fn create_withdraw_msg_as_any(
    sender: AccountId,
    receiver: AccountId,
    denom: &Denom,
    amount: u64,
    ibc_source_channel: Option<ChannelId>,
) -> Result<Any, MmError<WithdrawError>> {
    if let Some(channel_id) = ibc_source_channel {
        MsgTransfer::new_with_default_timeout(
            channel_id.to_string(),
            sender,
            receiver,
            Coin {
                denom: denom.clone(),
                amount: amount.into(),
            },
        )
        .map_to_mm(|e| WithdrawError::InternalError(e.to_string()))?
        .to_any()
    } else {
        MsgSend {
            from_address: sender,
            to_address: receiver,
            amount: vec![Coin {
                denom: denom.clone(),
                amount: amount.into(),
            }],
        }
        .to_any()
    }
    .map_to_mm(|e| WithdrawError::InternalError(e.to_string()))
}

fn extract_big_decimal_from_dec_coin(dec_coin: &DecCoin, decimals: u32) -> Result<BigDecimal, ParseBigDecimalError> {
    let raw = BigDecimal::from_str(&dec_coin.amount)?;
    // `DecCoin` represents decimal numbers as integer-like strings where the last 18 digits are the decimal part.
    let scale = BigDecimal::from(1_000_000_000_000_000_000u64) * BigDecimal::from(10u64.pow(decimals));
    Ok(raw / scale)
}

fn parse_expected_sequence_number(e: &str) -> MmResult<u64, TendermintCoinRpcError> {
    if let Some(sequence) = SEQUENCE_PARSER_REGEX.captures(e).and_then(|c| c.get(1)) {
        let account_sequence =
            u64::from_str(sequence.as_str()).map_to_mm(|e| TendermintCoinRpcError::InternalError(e.to_string()))?;

        return Ok(account_sequence);
    }

    MmError::err(TendermintCoinRpcError::InternalError(format!(
        "Could not parse the expected sequence number from this error message: '{e}'"
    )))
}

pub(crate) fn tendermint_tx_internal_id(bytes: &[u8], token_id: Option<BytesJson>) -> BytesJson {
    let mut bytes = bytes.to_vec();

    if let Some(token_id) = token_id {
        bytes.extend_from_slice(&token_id);
    }
    sha256(&bytes).to_vec().into()
}

#[cfg(test)]
pub mod tendermint_falsecoin_tests {
    use super::*;
    use crate::DexFeeBurnDestination;

    use common::{block_on, wait_until_ms, DEX_FEE_ADDR_RAW_PUBKEY};
    use cosmrs::proto::cosmos::tx::v1beta1::{GetTxRequest, GetTxResponse};
    use crypto::privkey::key_pair_from_seed;
    use mocktopus::mocking::{MockResult, Mockable};
    use std::{mem::discriminant, num::NonZeroUsize};

    pub const IRIS_TESTNET_HTLC_PAIR1_SEED: &str = "iris test seed";
    // pub const IRIS_TESTNET_HTLC_PAIR1_PUB_KEY: &[u8] = &[
    //     2, 35, 133, 39, 114, 92, 150, 175, 252, 203, 124, 85, 243, 144, 11, 52, 91, 128, 236, 82, 104, 212, 131, 40,
    //     79, 22, 40, 7, 119, 93, 50, 179, 43,
    // ];
    // const IRIS_TESTNET_HTLC_PAIR1_ADDRESS: &str = "iaa1e0rx87mdj79zejewuc4jg7ql9ud2286g2us8f2";

    // const IRIS_TESTNET_HTLC_PAIR2_SEED: &str = "iris test2 seed";
    const IRIS_TESTNET_HTLC_PAIR2_PUB_KEY: &[u8] = &[
        2, 90, 55, 151, 92, 7, 154, 117, 67, 96, 63, 202, 178, 78, 37, 101, 164, 173, 238, 60, 249, 175, 137, 52, 105,
        14, 16, 50, 130, 250, 64, 37, 17,
    ];
    const IRIS_TESTNET_HTLC_PAIR2_ADDRESS: &str = "iaa1erfnkjsmalkwtvj44qnfr2drfzdt4n9ldh0kjv";

    pub const IRIS_TESTNET_RPC_URL: &str = "http://34.80.202.172:26657";

    const TAKER_PAYMENT_SPEND_SEARCH_INTERVAL: f64 = 1.;
    const AVG_BLOCKTIME: u8 = 5;

    const SUCCEED_TX_HASH_SAMPLES: &[&str] = &[
        // https://nyancat.iobscan.io/#/tx?txHash=A010FC0AA33FC6D597A8635F9D127C0A7B892FAAC72489F4DADD90048CFE9279
        "A010FC0AA33FC6D597A8635F9D127C0A7B892FAAC72489F4DADD90048CFE9279",
        // https://nyancat.iobscan.io/#/tx?txHash=54FD77054AE311C484CC2EADD4621428BB23D14A9BAAC128B0E7B47422F86EC8
        "54FD77054AE311C484CC2EADD4621428BB23D14A9BAAC128B0E7B47422F86EC8",
        // https://nyancat.iobscan.io/#/tx?txHash=7C00FAE7F70C36A316A4736025B08A6EAA2A0CC7919A2C4FC4CD14D9FFD166F9
        "7C00FAE7F70C36A316A4736025B08A6EAA2A0CC7919A2C4FC4CD14D9FFD166F9",
    ];

    const FAILED_TX_HASH_SAMPLES: &[&str] = &[
        // https://nyancat.iobscan.io/#/tx?txHash=57EE62B2DF7E311C98C24AE2A53EB0FF2C16D289CECE0826CA1FF1108C91B3F9
        "57EE62B2DF7E311C98C24AE2A53EB0FF2C16D289CECE0826CA1FF1108C91B3F9",
        // https://nyancat.iobscan.io/#/tx?txHash=F3181D69C580318DFD54282C656AC81113BC600BCFBAAA480E6D8A6469EE8786
        "F3181D69C580318DFD54282C656AC81113BC600BCFBAAA480E6D8A6469EE8786",
        // https://nyancat.iobscan.io/#/tx?txHash=FE6F9F395DA94A14FCFC04E0E8C496197077D5F4968DA5528D9064C464ADF522
        "FE6F9F395DA94A14FCFC04E0E8C496197077D5F4968DA5528D9064C464ADF522",
    ];

    fn get_iris_usdc_ibc_protocol() -> TendermintProtocolInfo {
        TendermintProtocolInfo {
            decimals: 6,
            denom: Denom::from_str("ibc/5C465997B4F582F602CD64E12031C6A6E18CAF1E6EDC9B5D808822DC0B5F850C").unwrap(),
            min_balance_for_ibc_routing: None,
            account_prefix: String::from(IRIS_PREFIX),
            chain_id: ChainId::from_str("nyancat-9").unwrap(),
            gas_price: None,
            ibc_channels: HashMap::new(),
        }
    }

    fn get_iris_protocol() -> TendermintProtocolInfo {
        let mut ibc_channels = HashMap::new();
        ibc_channels.insert("cosmos".into(), ChannelId::new(0));

        TendermintProtocolInfo {
            decimals: 6,
            denom: Denom::from_str("unyan").unwrap(),
            min_balance_for_ibc_routing: None,
            account_prefix: String::from(IRIS_PREFIX),
            chain_id: ChainId::from_str("nyancat-9").unwrap(),
            gas_price: None,
            ibc_channels,
        }
    }

    fn get_iris_ibc_nucleus_protocol() -> TendermintProtocolInfo {
        TendermintProtocolInfo {
            decimals: 6,
            denom: Denom::from_str("ibc/F7F28FF3C09024A0225EDBBDB207E5872D2B4EF2FB874FE47B05EF9C9A7D211C").unwrap(),
            min_balance_for_ibc_routing: None,
            account_prefix: String::from(NUCLEUS_PREFIX),
            chain_id: ChainId::from_str("nucleus-testnet").unwrap(),
            gas_price: None,
            ibc_channels: HashMap::new(),
        }
    }

    fn get_tx_signer_pubkey_unprefixed(tx: &Tx, i: usize) -> Vec<u8> {
        tx.auth_info.as_ref().unwrap().signer_infos[i]
            .public_key
            .as_ref()
            .unwrap()
            .value[2..]
            .to_vec()
    }

    #[test]
    fn test_tx_hash_str_from_bytes() {
        let tx_hex = "0a97010a8f010a1c2f636f736d6f732e62616e6b2e763162657461312e4d736753656e64126f0a2d636f736d6f7331737661773061716334353834783832356a753775613033673578747877643061686c3836687a122d636f736d6f7331737661773061716334353834783832356a753775613033673578747877643061686c3836687a1a0f0a057561746f6d120631303030303018d998bf0512670a500a460a1f2f636f736d6f732e63727970746f2e736563703235366b312e5075624b657912230a2102000eef4ab169e7b26a4a16c47420c4176ab702119ba57a8820fb3e53c8e7506212040a020801180312130a0d0a057561746f6d12043130303010a08d061a4093e5aec96f7d311d129f5ec8714b21ad06a75e483ba32afab86354400b2ac8350bfc98731bbb05934bf138282750d71aadbe08ceb6bb195f2b55e1bbfdddaaad";
        let expected_hash = "1C25ED7D17FCC5959409498D5423594666C4E84F15AF7B4AF17DF29B2AF9E7F5";

        let tx_bytes = hex::decode(tx_hex).unwrap();
        let hash = sha256(&tx_bytes);
        assert_eq!(hex::encode_upper(hash.as_slice()), expected_hash);
    }

    #[ignore = "insufficient balance"]
    #[test]
    fn test_htlc_create_and_claim() {
        let nodes = vec![RpcNode::for_test(IRIS_TESTNET_RPC_URL)];

        let protocol_conf = get_iris_protocol();

        let ctx = mm2_core::mm_ctx::MmCtxBuilder::default().into_mm_arc();

        let conf = TendermintConf {
            avg_blocktime: AVG_BLOCKTIME,
            derivation_path: None,
        };

        let key_pair = key_pair_from_seed(IRIS_TESTNET_HTLC_PAIR1_SEED).unwrap();
        let tendermint_pair = TendermintKeyPair::new(key_pair.private().secret, *key_pair.public());
        let activation_policy =
            TendermintActivationPolicy::with_private_key_policy(TendermintPrivKeyPolicy::Iguana(tendermint_pair));

        let coin = block_on(TendermintCoin::init(
            &ctx,
            "IRIS".to_string(),
            conf,
            protocol_conf,
            nodes,
            false,
            activation_policy,
            Default::default(),
        ))
        .unwrap();

        // << BEGIN HTLC CREATION
        let to: AccountId = IRIS_TESTNET_HTLC_PAIR2_ADDRESS.parse().unwrap();
        let amount = 1;
        let amount_dec = big_decimal_from_sat_unsigned(amount, coin.decimals());

        let mut sec = [0u8; 32];
        common::os_rng(&mut sec).unwrap();
        drop_mutability!(sec);

        let time_lock = 1000;

        let create_htlc_tx = coin
            .gen_create_htlc_tx(
                coin.protocol_info.denom.clone(),
                &to,
                amount.into(),
                sha256(&sec).as_slice(),
                time_lock,
            )
            .unwrap();

        let current_block_fut = coin.current_block().compat();
        let current_block = block_on(async { current_block_fut.await.unwrap() });
        let timeout_height = current_block + TIMEOUT_HEIGHT_DELTA;

        let fee = block_on(async {
            coin.calculate_fee(
                create_htlc_tx.msg_payload.clone(),
                timeout_height,
                TX_DEFAULT_MEMO,
                None,
            )
            .await
            .unwrap()
        });

        let send_tx_fut = coin.common_send_raw_tx_bytes(
            create_htlc_tx.msg_payload.clone(),
            fee,
            timeout_height,
            TX_DEFAULT_MEMO,
            Duration::from_secs(20),
        );
        block_on(async {
            send_tx_fut.await.unwrap();
        });
        // >> END HTLC CREATION

        let htlc_spent = block_on(coin.check_if_my_payment_sent(CheckIfMyPaymentSentArgs {
            time_lock: 0,
            other_pub: IRIS_TESTNET_HTLC_PAIR2_PUB_KEY,
            secret_hash: sha256(&sec).as_slice(),
            search_from_block: current_block,
            swap_contract_address: &None,
            swap_unique_data: &[],
            amount: &amount_dec,
            payment_instructions: &None,
        }))
        .unwrap();
        assert!(htlc_spent.is_some());

        // << BEGIN HTLC CLAIMING
        let claim_htlc_tx = coin.gen_claim_htlc_tx(create_htlc_tx.id, &sec).unwrap();

        let current_block_fut = coin.current_block().compat();
        let current_block = common::block_on(async { current_block_fut.await.unwrap() });
        let timeout_height = current_block + TIMEOUT_HEIGHT_DELTA;

        let fee = block_on(async {
            coin.calculate_fee(claim_htlc_tx.msg_payload.clone(), timeout_height, TX_DEFAULT_MEMO, None)
                .await
                .unwrap()
        });

        let send_tx_fut = coin.common_send_raw_tx_bytes(
            claim_htlc_tx.msg_payload,
            fee,
            timeout_height,
            TX_DEFAULT_MEMO,
            Duration::from_secs(30),
        );

        let (tx_id, _tx_raw) = block_on(async { send_tx_fut.await.unwrap() });

        println!("Claim HTLC tx hash {tx_id}");
        // >> END HTLC CLAIMING
    }

    #[test]
    fn try_query_claim_htlc_txs_and_get_secret() {
        let nodes = vec![RpcNode::for_test(IRIS_TESTNET_RPC_URL)];

        let protocol_conf = get_iris_usdc_ibc_protocol();

        let ctx = mm2_core::mm_ctx::MmCtxBuilder::default().into_mm_arc();

        let conf = TendermintConf {
            avg_blocktime: AVG_BLOCKTIME,
            derivation_path: None,
        };

        let key_pair = key_pair_from_seed(IRIS_TESTNET_HTLC_PAIR1_SEED).unwrap();
        let tendermint_pair = TendermintKeyPair::new(key_pair.private().secret, *key_pair.public());
        let activation_policy =
            TendermintActivationPolicy::with_private_key_policy(TendermintPrivKeyPolicy::Iguana(tendermint_pair));

        let coin = block_on(TendermintCoin::init(
            &ctx,
            "USDC-IBC".to_string(),
            conf,
            protocol_conf,
            nodes,
            false,
            activation_policy,
            Default::default(),
        ))
        .unwrap();

        let query = "claim_htlc.id='2B925FC83A106CC81590B3DB108AC2AE496FFA912F368FE5E29BC1ED2B754F2C'".to_owned();
        let request = TxSearchRequest {
            query,
            order_by: TendermintResultOrder::Ascending.into(),
            page: 1,
            per_page: 1,
            prove: false,
        };
        let response = block_on(block_on(coin.rpc_client()).unwrap().perform(request)).unwrap();
        println!("{response:?}");

        let tx = cosmrs::Tx::from_bytes(&response.txs.first().unwrap().tx).unwrap();
        println!("{tx:?}");

        let first_msg = tx.body.messages.first().unwrap();
        println!("{first_msg:?}");

        let claim_htlc = ClaimHtlcProto::decode(HtlcType::Iris, first_msg.value.as_slice()).unwrap();
        let expected_secret = [1; 32];
        let actual_secret = hex::decode(claim_htlc.secret()).unwrap();

        assert_eq!(actual_secret, expected_secret);
    }

    #[test]
    fn wait_for_tx_spend_test() {
        let nodes = vec![RpcNode::for_test(IRIS_TESTNET_RPC_URL)];

        let protocol_conf = get_iris_usdc_ibc_protocol();

        let ctx = mm2_core::mm_ctx::MmCtxBuilder::default().into_mm_arc();

        let conf = TendermintConf {
            avg_blocktime: AVG_BLOCKTIME,
            derivation_path: None,
        };

        let key_pair = key_pair_from_seed(IRIS_TESTNET_HTLC_PAIR1_SEED).unwrap();
        let tendermint_pair = TendermintKeyPair::new(key_pair.private().secret, *key_pair.public());
        let activation_policy =
            TendermintActivationPolicy::with_private_key_policy(TendermintPrivKeyPolicy::Iguana(tendermint_pair));

        let coin = block_on(TendermintCoin::init(
            &ctx,
            "USDC-IBC".to_string(),
            conf,
            protocol_conf,
            nodes,
            false,
            activation_policy,
            Default::default(),
        ))
        .unwrap();

        // https://nyancat.iobscan.io/#/tx?txHash=2DB382CE3D9953E4A94957B475B0E8A98F5B6DDB32D6BF0F6A765D949CF4A727
        let create_tx_hash = "2DB382CE3D9953E4A94957B475B0E8A98F5B6DDB32D6BF0F6A765D949CF4A727";

        let request = GetTxRequest {
            hash: create_tx_hash.into(),
        };

        let response = block_on(block_on(coin.rpc_client()).unwrap().abci_query(
            Some(ABCI_GET_TX_PATH.to_string()),
            request.encode_to_vec(),
            ABCI_REQUEST_HEIGHT,
            ABCI_REQUEST_PROVE,
        ))
        .unwrap();
        println!("{response:?}");

        let response = GetTxResponse::decode(response.value.as_slice()).unwrap();
        let tx = response.tx.unwrap();

        println!("{tx:?}");

        let encoded_tx = tx.encode_to_vec();

        let secret_hash = hex::decode("0C34C71EBA2A51738699F9F3D6DAFFB15BE576E8ED543203485791B5DA39D10D").unwrap();
        let spend_tx = block_on(coin.wait_for_htlc_tx_spend(WaitForHTLCTxSpendArgs {
            tx_bytes: &encoded_tx,
            secret_hash: &secret_hash,
            wait_until: get_utc_timestamp() as u64,
            from_block: 0,
            swap_contract_address: &None,
            check_every: TAKER_PAYMENT_SPEND_SEARCH_INTERVAL,
            watcher_reward: false,
        }))
        .unwrap();

        // https://nyancat.iobscan.io/#/tx?txHash=565C820C1F95556ADC251F16244AAD4E4274772F41BC13F958C9C2F89A14D137
        let expected_spend_hash = "565C820C1F95556ADC251F16244AAD4E4274772F41BC13F958C9C2F89A14D137";
        let hash = spend_tx.tx_hash_as_bytes();
        assert_eq!(hex::encode_upper(hash.0), expected_spend_hash);
    }

    #[test]
    fn validate_taker_fee_test() {
        let nodes = vec![RpcNode::for_test(IRIS_TESTNET_RPC_URL)];

        let protocol_conf = get_iris_protocol();

        let ctx = mm2_core::mm_ctx::MmCtxBuilder::default().into_mm_arc();

        let conf = TendermintConf {
            avg_blocktime: AVG_BLOCKTIME,
            derivation_path: None,
        };

        let key_pair = key_pair_from_seed(IRIS_TESTNET_HTLC_PAIR1_SEED).unwrap();
        let tendermint_pair = TendermintKeyPair::new(key_pair.private().secret, *key_pair.public());
        let activation_policy =
            TendermintActivationPolicy::with_private_key_policy(TendermintPrivKeyPolicy::Iguana(tendermint_pair));

        let coin = block_on(TendermintCoin::init(
            &ctx,
            "IRIS-TEST".to_string(),
            conf,
            protocol_conf,
            nodes,
            false,
            activation_policy,
            Default::default(),
        ))
        .unwrap();

        // CreateHtlc tx, validation should fail because first message of dex fee tx must be MsgSend
        // https://nyancat.iobscan.io/#/tx?txHash=2DB382CE3D9953E4A94957B475B0E8A98F5B6DDB32D6BF0F6A765D949CF4A727
        let create_htlc_tx_response = GetTxResponse::decode(hex::decode("0ac4030a96020a8e020a1b2f697269736d6f642e68746c632e4d736743726561746548544c4312ee010a2a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76122a696161316530727838376d646a37397a656a65777563346a6737716c39756432323836673275733866321a40623736353830316334303930363762623837396565326563666665363138623931643734346663343030303030303030303030303030303030303030303030302a0d0a036e696d120631303030303032403063333463373165626132613531373338363939663966336436646166666231356265353736653865643534333230333438353739316235646133396431306440ea3c18afaba80212670a510a460a1f2f636f736d6f732e63727970746f2e736563703235366b312e5075624b657912230a21025a37975c079a7543603fcab24e2565a4adee3cf9af8934690e103282fa40251112040a02080118a50312120a0c0a05756e79616e120332303010a08d061a4029dfbe5fc6ec9ed257e0f3a86542cb9da0d6047620274f22265c4fb8221ed45830236adef675f76962f74e4cfcc7a10e1390f4d2071bc7dd07838e300381952612882208ccaaa8021240324442333832434533443939353345344139343935374234373542304538413938463542364444423332443642463046364137363544393439434634413732372ac60130413631304131423246363937323639373336443646363432453638373436433633324534443733363734333732363536313734363534383534344334333132343230413430343634333339343433383433333033353336343233363339343233323436333433313331333734353332343134333433333533323337343133343339333933303435333734353434333234323336343533323432343634313334343333323334333533373335333034343339333333353434333833313332333434333330333832cc095b7b226576656e7473223a5b7b2274797065223a22636f696e5f7265636569766564222c2261747472696275746573223a5b7b226b6579223a227265636569766572222c2276616c7565223a2269616131613778796e6a3463656674386b67646a72366b637130733037793363637961366d65707a646d227d2c7b226b6579223a22616d6f756e74222c2276616c7565223a223130303030306e696d227d5d7d2c7b2274797065223a22636f696e5f7370656e74222c2261747472696275746573223a5b7b226b6579223a227370656e646572222c2276616c7565223a22696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76227d2c7b226b6579223a22616d6f756e74222c2276616c7565223a223130303030306e696d227d5d7d2c7b2274797065223a226372656174655f68746c63222c2261747472696275746573223a5b7b226b6579223a226964222c2276616c7565223a2246433944384330353642363942324634313137453241434335323741343939304537454432423645324246413443323435373530443933354438313234433038227d2c7b226b6579223a2273656e646572222c2276616c7565223a22696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76227d2c7b226b6579223a227265636569766572222c2276616c7565223a22696161316530727838376d646a37397a656a65777563346a6737716c3975643232383667327573386632227d2c7b226b6579223a2272656365697665725f6f6e5f6f746865725f636861696e222c2276616c7565223a2262373635383031633430393036376262383739656532656366666536313862393164373434666334303030303030303030303030303030303030303030303030227d2c7b226b6579223a2273656e6465725f6f6e5f6f746865725f636861696e227d2c7b226b6579223a227472616e73666572222c2276616c7565223a2266616c7365227d5d7d2c7b2274797065223a226d657373616765222c2261747472696275746573223a5b7b226b6579223a22616374696f6e222c2276616c7565223a222f697269736d6f642e68746c632e4d736743726561746548544c43227d2c7b226b6579223a2273656e646572222c2276616c7565223a22696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76227d2c7b226b6579223a226d6f64756c65222c2276616c7565223a2268746c63227d2c7b226b6579223a2273656e646572222c2276616c7565223a22696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76227d5d7d2c7b2274797065223a227472616e73666572222c2261747472696275746573223a5b7b226b6579223a22726563697069656e74222c2276616c7565223a2269616131613778796e6a3463656674386b67646a72366b637130733037793363637961366d65707a646d227d2c7b226b6579223a2273656e646572222c2276616c7565223a22696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76227d2c7b226b6579223a22616d6f756e74222c2276616c7565223a223130303030306e696d227d5d7d5d7d5d3ac7061a5c0a0d636f696e5f726563656976656412360a087265636569766572122a69616131613778796e6a3463656674386b67646a72366b637130733037793363637961366d65707a646d12130a06616d6f756e7412093130303030306e696d1a580a0a636f696e5f7370656e7412350a077370656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a7612130a06616d6f756e7412093130303030306e696d1acc020a0b6372656174655f68746c6312460a02696412404643394438433035364236394232463431313745324143433532374134393930453745443242364532424641344332343537353044393335443831323443303812340a0673656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a7612360a087265636569766572122a696161316530727838376d646a37397a656a65777563346a6737716c3975643232383667327573386632125b0a1772656365697665725f6f6e5f6f746865725f636861696e12406237363538303163343039303637626238373965653265636666653631386239316437343466633430303030303030303030303030303030303030303030303012170a1573656e6465725f6f6e5f6f746865725f636861696e12110a087472616e73666572120566616c73651aac010a076d65737361676512250a06616374696f6e121b2f697269736d6f642e68746c632e4d736743726561746548544c4312340a0673656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76120e0a066d6f64756c65120468746c6312340a0673656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a761a8e010a087472616e7366657212370a09726563697069656e74122a69616131613778796e6a3463656674386b67646a72366b637130733037793363637961366d65707a646d12340a0673656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a7612130a06616d6f756e7412093130303030306e696d48a08d06509bd3045ade030a152f636f736d6f732e74782e763162657461312e547812c4030a96020a8e020a1b2f697269736d6f642e68746c632e4d736743726561746548544c4312ee010a2a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76122a696161316530727838376d646a37397a656a65777563346a6737716c39756432323836673275733866321a40623736353830316334303930363762623837396565326563666665363138623931643734346663343030303030303030303030303030303030303030303030302a0d0a036e696d120631303030303032403063333463373165626132613531373338363939663966336436646166666231356265353736653865643534333230333438353739316235646133396431306440ea3c18afaba80212670a510a460a1f2f636f736d6f732e63727970746f2e736563703235366b312e5075624b657912230a21025a37975c079a7543603fcab24e2565a4adee3cf9af8934690e103282fa40251112040a02080118a50312120a0c0a05756e79616e120332303010a08d061a4029dfbe5fc6ec9ed257e0f3a86542cb9da0d6047620274f22265c4fb8221ed45830236adef675f76962f74e4cfcc7a10e1390f4d2071bc7dd07838e30038195266214323032322d30392d31355432333a30343a35355a6a410a027478123b0a076163635f736571122e696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a762f34323118016a6d0a02747812670a097369676e617475726512584b642b2b583862736e744a5834504f6f5a554c4c6e614457424859674a3038694a6c7850754349653146677749327265396e583361574c33546b7a387836454f45354430306763627839304867343477413447564a673d3d18016a5b0a0a636f696e5f7370656e7412370a077370656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76180112140a06616d6f756e741208323030756e79616e18016a5f0a0d636f696e5f726563656976656412380a087265636569766572122a696161313778706676616b6d32616d67393632796c73366638347a336b656c6c3863356c396d72336676180112140a06616d6f756e741208323030756e79616e18016a93010a087472616e7366657212390a09726563697069656e74122a696161313778706676616b6d32616d67393632796c73366638347a336b656c6c3863356c396d72336676180112360a0673656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76180112140a06616d6f756e741208323030756e79616e18016a410a076d65737361676512360a0673656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a7618016a170a02747812110a036665651208323030756e79616e18016a320a076d65737361676512270a06616374696f6e121b2f697269736d6f642e68746c632e4d736743726561746548544c4318016a5c0a0a636f696e5f7370656e7412370a077370656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76180112150a06616d6f756e7412093130303030306e696d18016a600a0d636f696e5f726563656976656412380a087265636569766572122a69616131613778796e6a3463656674386b67646a72366b637130733037793363637961366d65707a646d180112150a06616d6f756e7412093130303030306e696d18016a94010a087472616e7366657212390a09726563697069656e74122a69616131613778796e6a3463656674386b67646a72366b637130733037793363637961366d65707a646d180112360a0673656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76180112150a06616d6f756e7412093130303030306e696d18016a410a076d65737361676512360a0673656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a7618016ad8020a0b6372656174655f68746c6312480a026964124046433944384330353642363942324634313137453241434335323741343939304537454432423645324246413443323435373530443933354438313234433038180112360a0673656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76180112380a087265636569766572122a696161316530727838376d646a37397a656a65777563346a6737716c39756432323836673275733866321801125d0a1772656365697665725f6f6e5f6f746865725f636861696e124062373635383031633430393036376262383739656532656366666536313862393164373434666334303030303030303030303030303030303030303030303030180112190a1573656e6465725f6f6e5f6f746865725f636861696e180112130a087472616e73666572120566616c736518016a530a076d65737361676512100a066d6f64756c65120468746c63180112360a0673656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a761801").unwrap().as_slice()).unwrap();
        let mock_tx = create_htlc_tx_response.tx.as_ref().unwrap().clone();
        TendermintCoin::request_tx.mock_safe(move |_, _| {
            let mock_tx = mock_tx.clone();
            MockResult::Return(Box::pin(async move { Ok(mock_tx) }))
        });
        let create_htlc_tx = TransactionEnum::CosmosTransaction(CosmosTransaction {
            data: TxRaw::decode(create_htlc_tx_response.tx.as_ref().unwrap().encode_to_vec().as_slice()).unwrap(),
        });

        let invalid_amount: MmNumber = 1.into();
        let error = block_on(coin.validate_fee(ValidateFeeArgs {
            fee_tx: &create_htlc_tx,
            expected_sender: &[],
            dex_fee: &DexFee::Standard(invalid_amount.clone()),
            min_block_number: 0,
            uuid: &[1; 16],
        }))
        .unwrap_err()
        .into_inner();
        println!("{error}");
        match error {
            ValidatePaymentError::TxDeserializationError(err) => {
                assert!(err.contains("failed to decode Protobuf message: MsgSend.amount"))
            },
            _ => panic!(
                "Expected `WrongPaymentTx` MsgSend.amount decode failure, found {:?}",
                error
            ),
        }
        TendermintCoin::request_tx.clear_mock();

        // just a random transfer tx not related to AtomicDEX, should fail on recipient address check
        // https://nyancat.iobscan.io/#/tx?txHash=65815814E7D74832D87956144C1E84801DC94FE9A509D207A0ABC3F17775E5DF
        let random_transfer_tx_response = GetTxResponse::decode(hex::decode("0ac6020a95010a8c010a1c2f636f736d6f732e62616e6b2e763162657461312e4d736753656e64126c0a2a696161317039703230667468306c7665647634736d7733327339377079386e74657230716e7774727538122a696161316b36636d636b7875757732647a7a6b76747a7239776c7467356c3633747361746b6c71357a791a120a05756e79616e1209313030303030303030120474657374126a0a510a460a1f2f636f736d6f732e63727970746f2e736563703235366b312e5075624b657912230a2103327a4866304ead15d941dbbdf2d2563514fcc94d25e4af897a71681a02b637b212040a02080118880212150a0f0a05756e79616e120632303030303010c09a0c1a402d1c8c1e1a44bd56fe24947d6ed6cae27c6f8a46e3e9beaaad9798dc842ae4ea0c0a20f33144c8fad3490638455b65f63decdb74c347a7c97d0469f5de453fe312a41608febfba021240363538313538313445374437343833324438373935363134344331453834383031444339344645394135303944323037413041424333463137373735453544462a403041314530413143324636333646373336443646373332453632363136453642324537363331363236353734363133313245344437333637353336353645363432da055b7b226576656e7473223a5b7b2274797065223a22636f696e5f7265636569766564222c2261747472696275746573223a5b7b226b6579223a227265636569766572222c2276616c7565223a22696161316b36636d636b7875757732647a7a6b76747a7239776c7467356c3633747361746b6c71357a79227d2c7b226b6579223a22616d6f756e74222c2276616c7565223a22313030303030303030756e79616e227d5d7d2c7b2274797065223a22636f696e5f7370656e74222c2261747472696275746573223a5b7b226b6579223a227370656e646572222c2276616c7565223a22696161317039703230667468306c7665647634736d7733327339377079386e74657230716e7774727538227d2c7b226b6579223a22616d6f756e74222c2276616c7565223a22313030303030303030756e79616e227d5d7d2c7b2274797065223a226d657373616765222c2261747472696275746573223a5b7b226b6579223a22616374696f6e222c2276616c7565223a222f636f736d6f732e62616e6b2e763162657461312e4d736753656e64227d2c7b226b6579223a2273656e646572222c2276616c7565223a22696161317039703230667468306c7665647634736d7733327339377079386e74657230716e7774727538227d2c7b226b6579223a226d6f64756c65222c2276616c7565223a2262616e6b227d5d7d2c7b2274797065223a227472616e73666572222c2261747472696275746573223a5b7b226b6579223a22726563697069656e74222c2276616c7565223a22696161316b36636d636b7875757732647a7a6b76747a7239776c7467356c3633747361746b6c71357a79227d2c7b226b6579223a2273656e646572222c2276616c7565223a22696161317039703230667468306c7665647634736d7733327339377079386e74657230716e7774727538227d2c7b226b6579223a22616d6f756e74222c2276616c7565223a22313030303030303030756e79616e227d5d7d5d7d5d3ad1031a610a0d636f696e5f726563656976656412360a087265636569766572122a696161316b36636d636b7875757732647a7a6b76747a7239776c7467356c3633747361746b6c71357a7912180a06616d6f756e74120e313030303030303030756e79616e1a5d0a0a636f696e5f7370656e7412350a077370656e646572122a696161317039703230667468306c7665647634736d7733327339377079386e74657230716e777472753812180a06616d6f756e74120e313030303030303030756e79616e1a770a076d65737361676512260a06616374696f6e121c2f636f736d6f732e62616e6b2e763162657461312e4d736753656e6412340a0673656e646572122a696161317039703230667468306c7665647634736d7733327339377079386e74657230716e7774727538120e0a066d6f64756c65120462616e6b1a93010a087472616e7366657212370a09726563697069656e74122a696161316b36636d636b7875757732647a7a6b76747a7239776c7467356c3633747361746b6c71357a7912340a0673656e646572122a696161317039703230667468306c7665647634736d7733327339377079386e74657230716e777472753812180a06616d6f756e74120e313030303030303030756e79616e48c09a0c5092e5035ae0020a152f636f736d6f732e74782e763162657461312e547812c6020a95010a8c010a1c2f636f736d6f732e62616e6b2e763162657461312e4d736753656e64126c0a2a696161317039703230667468306c7665647634736d7733327339377079386e74657230716e7774727538122a696161316b36636d636b7875757732647a7a6b76747a7239776c7467356c3633747361746b6c71357a791a120a05756e79616e1209313030303030303030120474657374126a0a510a460a1f2f636f736d6f732e63727970746f2e736563703235366b312e5075624b657912230a2103327a4866304ead15d941dbbdf2d2563514fcc94d25e4af897a71681a02b637b212040a02080118880212150a0f0a05756e79616e120632303030303010c09a0c1a402d1c8c1e1a44bd56fe24947d6ed6cae27c6f8a46e3e9beaaad9798dc842ae4ea0c0a20f33144c8fad3490638455b65f63decdb74c347a7c97d0469f5de453fe36214323032322d31302d30335430363a35313a31375a6a410a027478123b0a076163635f736571122e696161317039703230667468306c7665647634736d7733327339377079386e74657230716e77747275382f32363418016a6d0a02747812670a097369676e617475726512584c52794d486870457656622b4a4a52396274624b346e7876696b626a36623671725a655933495171354f6f4d4369447a4d5554492b744e4a426a68465732583250657a62644d4e4870386c3942476e31336b552f34773d3d18016a5e0a0a636f696e5f7370656e7412370a077370656e646572122a696161317039703230667468306c7665647634736d7733327339377079386e74657230716e7774727538180112170a06616d6f756e74120b323030303030756e79616e18016a620a0d636f696e5f726563656976656412380a087265636569766572122a696161313778706676616b6d32616d67393632796c73366638347a336b656c6c3863356c396d72336676180112170a06616d6f756e74120b323030303030756e79616e18016a96010a087472616e7366657212390a09726563697069656e74122a696161313778706676616b6d32616d67393632796c73366638347a336b656c6c3863356c396d72336676180112360a0673656e646572122a696161317039703230667468306c7665647634736d7733327339377079386e74657230716e7774727538180112170a06616d6f756e74120b323030303030756e79616e18016a410a076d65737361676512360a0673656e646572122a696161317039703230667468306c7665647634736d7733327339377079386e74657230716e777472753818016a1a0a02747812140a03666565120b323030303030756e79616e18016a330a076d65737361676512280a06616374696f6e121c2f636f736d6f732e62616e6b2e763162657461312e4d736753656e6418016a610a0a636f696e5f7370656e7412370a077370656e646572122a696161317039703230667468306c7665647634736d7733327339377079386e74657230716e77747275381801121a0a06616d6f756e74120e313030303030303030756e79616e18016a650a0d636f696e5f726563656976656412380a087265636569766572122a696161316b36636d636b7875757732647a7a6b76747a7239776c7467356c3633747361746b6c71357a791801121a0a06616d6f756e74120e313030303030303030756e79616e18016a99010a087472616e7366657212390a09726563697069656e74122a696161316b36636d636b7875757732647a7a6b76747a7239776c7467356c3633747361746b6c71357a79180112360a0673656e646572122a696161317039703230667468306c7665647634736d7733327339377079386e74657230716e77747275381801121a0a06616d6f756e74120e313030303030303030756e79616e18016a410a076d65737361676512360a0673656e646572122a696161317039703230667468306c7665647634736d7733327339377079386e74657230716e777472753818016a1b0a076d65737361676512100a066d6f64756c65120462616e6b1801").unwrap().as_slice()).unwrap();
        let mock_tx = random_transfer_tx_response.tx.as_ref().unwrap().clone();
        TendermintCoin::request_tx.mock_safe(move |_, _| {
            let mock_tx = mock_tx.clone();
            MockResult::Return(Box::pin(async move { Ok(mock_tx) }))
        });
        let random_transfer_tx = TransactionEnum::CosmosTransaction(CosmosTransaction {
            data: TxRaw::decode(
                random_transfer_tx_response
                    .tx
                    .as_ref()
                    .unwrap()
                    .encode_to_vec()
                    .as_slice(),
            )
            .unwrap(),
        });

        let error = block_on(coin.validate_fee(ValidateFeeArgs {
            fee_tx: &random_transfer_tx,
            expected_sender: &[],
            dex_fee: &DexFee::Standard(invalid_amount.clone()),
            min_block_number: 0,
            uuid: &[1; 16],
        }))
        .unwrap_err()
        .into_inner();
        println!("{error}");
        match error {
            ValidatePaymentError::WrongPaymentTx(err) => assert!(err.contains("sent to wrong address")),
            _ => panic!("Expected `WrongPaymentTx` wrong address, found {:?}", error),
        }
        TendermintCoin::request_tx.clear_mock();

        // dex fee tx sent during real swap
        // https://nyancat.iobscan.io/#/tx?txHash=8AA6B9591FE1EE93C8B89DE4F2C59B2F5D3473BD9FB5F3CFF6A5442BEDC881D7
        let dex_fee_tx_response = GetTxResponse::decode(hex::decode("0abc020a8e010a86010a1c2f636f736d6f732e62616e6b2e763162657461312e4d736753656e6412660a2a69616131647863376c64676b336e666e356b373671706c75703967397868786e7966346d6570396b7038122a696161316567307167617a37336a737676727676747a713478383233686d7a387161706c64643078347a1a0c0a05756e79616e120331303018a89bb00212670a500a460a1f2f636f736d6f732e63727970746f2e736563703235366b312e5075624b657912230a2103d4f75874e5f2a51d9d22f747ebd94da63207b08c7b023b09865051f074eb7ea412040a020801180612130a0d0a05756e79616e12043130303010a08d061a40784831c62a96658e9b0c484bbf684465788701c4fbd46c744f20f4ade3dbba1152f279c8afb118ae500ed9dc1260a8125a0f173c91ea408a3a3e0bd42b226ae012da1508c59ab0021240384141364239353931464531454539334338423839444534463243353942324635443334373342443946423546334346463641353434324245444338383144372a403041314530413143324636333646373336443646373332453632363136453642324537363331363236353734363133313245344437333637353336353645363432c8055b7b226576656e7473223a5b7b2274797065223a22636f696e5f7265636569766564222c2261747472696275746573223a5b7b226b6579223a227265636569766572222c2276616c7565223a22696161316567307167617a37336a737676727676747a713478383233686d7a387161706c64643078347a227d2c7b226b6579223a22616d6f756e74222c2276616c7565223a22313030756e79616e227d5d7d2c7b2274797065223a22636f696e5f7370656e74222c2261747472696275746573223a5b7b226b6579223a227370656e646572222c2276616c7565223a2269616131647863376c64676b336e666e356b373671706c75703967397868786e7966346d6570396b7038227d2c7b226b6579223a22616d6f756e74222c2276616c7565223a22313030756e79616e227d5d7d2c7b2274797065223a226d657373616765222c2261747472696275746573223a5b7b226b6579223a22616374696f6e222c2276616c7565223a222f636f736d6f732e62616e6b2e763162657461312e4d736753656e64227d2c7b226b6579223a2273656e646572222c2276616c7565223a2269616131647863376c64676b336e666e356b373671706c75703967397868786e7966346d6570396b7038227d2c7b226b6579223a226d6f64756c65222c2276616c7565223a2262616e6b227d5d7d2c7b2274797065223a227472616e73666572222c2261747472696275746573223a5b7b226b6579223a22726563697069656e74222c2276616c7565223a22696161316567307167617a37336a737676727676747a713478383233686d7a387161706c64643078347a227d2c7b226b6579223a2273656e646572222c2276616c7565223a2269616131647863376c64676b336e666e356b373671706c75703967397868786e7966346d6570396b7038227d2c7b226b6579223a22616d6f756e74222c2276616c7565223a22313030756e79616e227d5d7d5d7d5d3abf031a5b0a0d636f696e5f726563656976656412360a087265636569766572122a696161316567307167617a37336a737676727676747a713478383233686d7a387161706c64643078347a12120a06616d6f756e741208313030756e79616e1a570a0a636f696e5f7370656e7412350a077370656e646572122a69616131647863376c64676b336e666e356b373671706c75703967397868786e7966346d6570396b703812120a06616d6f756e741208313030756e79616e1a770a076d65737361676512260a06616374696f6e121c2f636f736d6f732e62616e6b2e763162657461312e4d736753656e6412340a0673656e646572122a69616131647863376c64676b336e666e356b373671706c75703967397868786e7966346d6570396b7038120e0a066d6f64756c65120462616e6b1a8d010a087472616e7366657212370a09726563697069656e74122a696161316567307167617a37336a737676727676747a713478383233686d7a387161706c64643078347a12340a0673656e646572122a69616131647863376c64676b336e666e356b373671706c75703967397868786e7966346d6570396b703812120a06616d6f756e741208313030756e79616e48a08d0650acdf035ad6020a152f636f736d6f732e74782e763162657461312e547812bc020a8e010a86010a1c2f636f736d6f732e62616e6b2e763162657461312e4d736753656e6412660a2a69616131647863376c64676b336e666e356b373671706c75703967397868786e7966346d6570396b7038122a696161316567307167617a37336a737676727676747a713478383233686d7a387161706c64643078347a1a0c0a05756e79616e120331303018a89bb00212670a500a460a1f2f636f736d6f732e63727970746f2e736563703235366b312e5075624b657912230a2103d4f75874e5f2a51d9d22f747ebd94da63207b08c7b023b09865051f074eb7ea412040a020801180612130a0d0a05756e79616e12043130303010a08d061a40784831c62a96658e9b0c484bbf684465788701c4fbd46c744f20f4ade3dbba1152f279c8afb118ae500ed9dc1260a8125a0f173c91ea408a3a3e0bd42b226ae06214323032322d30392d32335431313a31313a35395a6a3f0a02747812390a076163635f736571122c69616131647863376c64676b336e666e356b373671706c75703967397868786e7966346d6570396b70382f3618016a6d0a02747812670a097369676e6174757265125865456778786971575a5936624445684c763268455a5869484163543731477830547944307265506275684653386e6e4972374559726c414f32647753594b675357673858504a487151496f36506776554b794a7134413d3d18016a5c0a0a636f696e5f7370656e7412370a077370656e646572122a69616131647863376c64676b336e666e356b373671706c75703967397868786e7966346d6570396b7038180112150a06616d6f756e74120931303030756e79616e18016a600a0d636f696e5f726563656976656412380a087265636569766572122a696161313778706676616b6d32616d67393632796c73366638347a336b656c6c3863356c396d72336676180112150a06616d6f756e74120931303030756e79616e18016a94010a087472616e7366657212390a09726563697069656e74122a696161313778706676616b6d32616d67393632796c73366638347a336b656c6c3863356c396d72336676180112360a0673656e646572122a69616131647863376c64676b336e666e356b373671706c75703967397868786e7966346d6570396b7038180112150a06616d6f756e74120931303030756e79616e18016a410a076d65737361676512360a0673656e646572122a69616131647863376c64676b336e666e356b373671706c75703967397868786e7966346d6570396b703818016a180a02747812120a03666565120931303030756e79616e18016a330a076d65737361676512280a06616374696f6e121c2f636f736d6f732e62616e6b2e763162657461312e4d736753656e6418016a5b0a0a636f696e5f7370656e7412370a077370656e646572122a69616131647863376c64676b336e666e356b373671706c75703967397868786e7966346d6570396b7038180112140a06616d6f756e741208313030756e79616e18016a5f0a0d636f696e5f726563656976656412380a087265636569766572122a696161316567307167617a37336a737676727676747a713478383233686d7a387161706c64643078347a180112140a06616d6f756e741208313030756e79616e18016a93010a087472616e7366657212390a09726563697069656e74122a696161316567307167617a37336a737676727676747a713478383233686d7a387161706c64643078347a180112360a0673656e646572122a69616131647863376c64676b336e666e356b373671706c75703967397868786e7966346d6570396b7038180112140a06616d6f756e741208313030756e79616e18016a410a076d65737361676512360a0673656e646572122a69616131647863376c64676b336e666e356b373671706c75703967397868786e7966346d6570396b703818016a1b0a076d65737361676512100a066d6f64756c65120462616e6b1801").unwrap().as_slice()).unwrap();
        let mock_tx = dex_fee_tx_response.tx.as_ref().unwrap().clone();
        TendermintCoin::request_tx.mock_safe(move |_, _| {
            let mock_tx = mock_tx.clone();
            MockResult::Return(Box::pin(async move { Ok(mock_tx) }))
        });

        let pubkey = get_tx_signer_pubkey_unprefixed(dex_fee_tx_response.tx.as_ref().unwrap(), 0);
        let dex_fee_tx = TransactionEnum::CosmosTransaction(CosmosTransaction {
            data: TxRaw::decode(dex_fee_tx_response.tx.as_ref().unwrap().encode_to_vec().as_slice()).unwrap(),
        });

        let error = block_on(coin.validate_fee(ValidateFeeArgs {
            fee_tx: &dex_fee_tx,
            expected_sender: &[],
            dex_fee: &DexFee::Standard(invalid_amount),
            min_block_number: 0,
            uuid: &[1; 16],
        }))
        .unwrap_err()
        .into_inner();
        println!("{error}");
        match error {
            ValidatePaymentError::WrongPaymentTx(err) => assert!(err.contains("Invalid amount")),
            _ => panic!("Expected `WrongPaymentTx` invalid amount, found {:?}", error),
        }

        let valid_amount: BigDecimal = "0.0001".parse().unwrap();
        // valid amount but invalid sender
        let error = block_on(coin.validate_fee(ValidateFeeArgs {
            fee_tx: &dex_fee_tx,
            expected_sender: &DEX_FEE_ADDR_RAW_PUBKEY,
            dex_fee: &DexFee::Standard(valid_amount.clone().into()),
            min_block_number: 0,
            uuid: &[1; 16],
        }))
        .unwrap_err()
        .into_inner();
        println!("{error}");
        match error {
            ValidatePaymentError::WrongPaymentTx(err) => assert!(err.contains("Invalid sender")),
            _ => panic!("Expected `WrongPaymentTx` invalid sender, found {:?}", error),
        }

        // invalid memo
        let error = block_on(coin.validate_fee(ValidateFeeArgs {
            fee_tx: &dex_fee_tx,
            expected_sender: &pubkey,
            dex_fee: &DexFee::Standard(valid_amount.into()),
            min_block_number: 0,
            uuid: &[1; 16],
        }))
        .unwrap_err()
        .into_inner();
        println!("{error}");
        match error {
            ValidatePaymentError::WrongPaymentTx(err) => assert!(err.contains("Invalid memo")),
            _ => panic!("Expected `WrongPaymentTx` invalid memo, found {:?}", error),
        }

        // https://nyancat.iobscan.io/#/tx?txHash=5939A9D1AF57BB828714E0C4C4D7F2AEE349BB719B0A1F25F8FBCC3BB227C5F9
        let fee_with_memo_tx_response = GetTxResponse::decode(hex::decode("0ae2020ab2010a84010a1c2f636f736d6f732e62616e6b2e763162657461312e4d736753656e6412640a2a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76122a696161316567307167617a37336a737676727676747a713478383233686d7a387161706c64643078347a1a0a0a036e696d1203313030122463616536303131622d393831302d343731302d623738342d31653564643062336130643018dbe0bb0212690a510a460a1f2f636f736d6f732e63727970746f2e736563703235366b312e5075624b657912230a21025a37975c079a7543603fcab24e2565a4adee3cf9af8934690e103282fa40251112040a02080118a50412140a0e0a05756e79616e1205353030303010a08d061a4078295295db2e305b7b53c6b7154f1d6b1c311fd10aaf56ad96840e59f403bae045f2ca5920e7bef679eacd200d6f30eca7d3571b93dcde38c8c130e1c1d9e4c712f41508f8dfbb021240353933394139443141463537424238323837313445304334433444374632414545333439424237313942304131463235463846424343334242323237433546392a403041314530413143324636333646373336443646373332453632363136453642324537363331363236353734363133313245344437333637353336353645363432c2055b7b226576656e7473223a5b7b2274797065223a22636f696e5f7265636569766564222c2261747472696275746573223a5b7b226b6579223a227265636569766572222c2276616c7565223a22696161316567307167617a37336a737676727676747a713478383233686d7a387161706c64643078347a227d2c7b226b6579223a22616d6f756e74222c2276616c7565223a223130306e696d227d5d7d2c7b2274797065223a22636f696e5f7370656e74222c2261747472696275746573223a5b7b226b6579223a227370656e646572222c2276616c7565223a22696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76227d2c7b226b6579223a22616d6f756e74222c2276616c7565223a223130306e696d227d5d7d2c7b2274797065223a226d657373616765222c2261747472696275746573223a5b7b226b6579223a22616374696f6e222c2276616c7565223a222f636f736d6f732e62616e6b2e763162657461312e4d736753656e64227d2c7b226b6579223a2273656e646572222c2276616c7565223a22696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76227d2c7b226b6579223a226d6f64756c65222c2276616c7565223a2262616e6b227d5d7d2c7b2274797065223a227472616e73666572222c2261747472696275746573223a5b7b226b6579223a22726563697069656e74222c2276616c7565223a22696161316567307167617a37336a737676727676747a713478383233686d7a387161706c64643078347a227d2c7b226b6579223a2273656e646572222c2276616c7565223a22696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76227d2c7b226b6579223a22616d6f756e74222c2276616c7565223a223130306e696d227d5d7d5d7d5d3ab9031a590a0d636f696e5f726563656976656412360a087265636569766572122a696161316567307167617a37336a737676727676747a713478383233686d7a387161706c64643078347a12100a06616d6f756e7412063130306e696d1a550a0a636f696e5f7370656e7412350a077370656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a7612100a06616d6f756e7412063130306e696d1a770a076d65737361676512260a06616374696f6e121c2f636f736d6f732e62616e6b2e763162657461312e4d736753656e6412340a0673656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76120e0a066d6f64756c65120462616e6b1a8b010a087472616e7366657212370a09726563697069656e74122a696161316567307167617a37336a737676727676747a713478383233686d7a387161706c64643078347a12340a0673656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a7612100a06616d6f756e7412063130306e696d48a08d0650d4e1035afc020a152f636f736d6f732e74782e763162657461312e547812e2020ab2010a84010a1c2f636f736d6f732e62616e6b2e763162657461312e4d736753656e6412640a2a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76122a696161316567307167617a37336a737676727676747a713478383233686d7a387161706c64643078347a1a0a0a036e696d1203313030122463616536303131622d393831302d343731302d623738342d31653564643062336130643018dbe0bb0212690a510a460a1f2f636f736d6f732e63727970746f2e736563703235366b312e5075624b657912230a21025a37975c079a7543603fcab24e2565a4adee3cf9af8934690e103282fa40251112040a02080118a50412140a0e0a05756e79616e1205353030303010a08d061a4078295295db2e305b7b53c6b7154f1d6b1c311fd10aaf56ad96840e59f403bae045f2ca5920e7bef679eacd200d6f30eca7d3571b93dcde38c8c130e1c1d9e4c76214323032322d31302d30345431313a33343a35355a6a410a027478123b0a076163635f736571122e696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a762f35343918016a6d0a02747812670a097369676e6174757265125865436c536c6473754d4674375538613346553864617877784839454b723161746c6f514f57665144757542463873705a494f652b396e6e717a53414e627a447370394e5847355063336a6a497754446877646e6b78773d3d18016a5d0a0a636f696e5f7370656e7412370a077370656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76180112160a06616d6f756e74120a3530303030756e79616e18016a610a0d636f696e5f726563656976656412380a087265636569766572122a696161313778706676616b6d32616d67393632796c73366638347a336b656c6c3863356c396d72336676180112160a06616d6f756e74120a3530303030756e79616e18016a95010a087472616e7366657212390a09726563697069656e74122a696161313778706676616b6d32616d67393632796c73366638347a336b656c6c3863356c396d72336676180112360a0673656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76180112160a06616d6f756e74120a3530303030756e79616e18016a410a076d65737361676512360a0673656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a7618016a190a02747812130a03666565120a3530303030756e79616e18016a330a076d65737361676512280a06616374696f6e121c2f636f736d6f732e62616e6b2e763162657461312e4d736753656e6418016a590a0a636f696e5f7370656e7412370a077370656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76180112120a06616d6f756e7412063130306e696d18016a5d0a0d636f696e5f726563656976656412380a087265636569766572122a696161316567307167617a37336a737676727676747a713478383233686d7a387161706c64643078347a180112120a06616d6f756e7412063130306e696d18016a91010a087472616e7366657212390a09726563697069656e74122a696161316567307167617a37336a737676727676747a713478383233686d7a387161706c64643078347a180112360a0673656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a76180112120a06616d6f756e7412063130306e696d18016a410a076d65737361676512360a0673656e646572122a696161316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c6468306b6a7618016a1b0a076d65737361676512100a066d6f64756c65120462616e6b1801").unwrap().as_slice()).unwrap();
        let mock_tx = fee_with_memo_tx_response.tx.as_ref().unwrap().clone();
        TendermintCoin::request_tx.mock_safe(move |_, _| {
            let mock_tx = mock_tx.clone();
            MockResult::Return(Box::pin(async move { Ok(mock_tx) }))
        });

        let pubkey = get_tx_signer_pubkey_unprefixed(fee_with_memo_tx_response.tx.as_ref().unwrap(), 0);
        let fee_with_memo_tx = TransactionEnum::CosmosTransaction(CosmosTransaction {
            data: TxRaw::decode(
                fee_with_memo_tx_response
                    .tx
                    .as_ref()
                    .unwrap()
                    .encode_to_vec()
                    .as_slice(),
            )
            .unwrap(),
        });

        let uuid: Uuid = "cae6011b-9810-4710-b784-1e5dd0b3a0d0".parse().unwrap();
        let dex_fee = DexFee::Standard(MmNumber::from("0.0001"));
        block_on(
            coin.validate_fee_for_denom(&fee_with_memo_tx, &pubkey, &dex_fee, 6, uuid.as_bytes(), "nim".into())
                .compat(),
        )
        .unwrap();
        TendermintCoin::request_tx.clear_mock();
    }

    #[test]
    fn validate_taker_fee_with_burn_test() {
        const NUCLEUS_TEST_SEED: &str = "nucleus test seed";

        let ctx = mm2_core::mm_ctx::MmCtxBuilder::default().into_mm_arc();
        let conf = TendermintConf {
            avg_blocktime: AVG_BLOCKTIME,
            derivation_path: None,
        };

        let key_pair = key_pair_from_seed(NUCLEUS_TEST_SEED).unwrap();
        let tendermint_pair = TendermintKeyPair::new(key_pair.private().secret, *key_pair.public());
        let activation_policy =
            TendermintActivationPolicy::with_private_key_policy(TendermintPrivKeyPolicy::Iguana(tendermint_pair));
        let nucleus_nodes = vec![RpcNode::for_test("http://localhost:26657")];
        let iris_ibc_nucleus_protocol = get_iris_ibc_nucleus_protocol();
        let iris_ibc_nucleus_denom =
            String::from("ibc/F7F28FF3C09024A0225EDBBDB207E5872D2B4EF2FB874FE47B05EF9C9A7D211C");
        let coin = block_on(TendermintCoin::init(
            &ctx,
            "NUCLEUS-TEST".to_string(),
            conf,
            iris_ibc_nucleus_protocol,
            nucleus_nodes,
            false,
            activation_policy,
            Default::default(),
        ))
        .unwrap();

        // tx from docker test (no real swaps yet)
        let fee_with_burn_tx = Tx::decode(hex::decode("0abd030a91030a212f636f736d6f732e62616e6b2e763162657461312e4d73674d756c746953656e6412eb020a770a2a6e7563316572666e6b6a736d616c6b7774766a3434716e6672326472667a6474346e396c65647736337912490a446962632f4637463238464633433039303234413032323545444242444232303745353837324432423445463246423837344645343742303545463943394137443231314312013912770a2a6e7563316567307167617a37336a737676727676747a713478383233686d7a387161706c656877326b3212490a446962632f4637463238464633433039303234413032323545444242444232303745353837324432423445463246423837344645343742303545463943394137443231314312013712770a2a6e756331797937346b393278707437367a616e6c3276363837636175393861666d70363071723564743712490a446962632f46374632384646334330393032344130323235454442424442323037453538373244324234454632464238373446453437423035454639433941374432313143120132122433656338646436352d313036342d346630362d626166332d66373265623563396230346418b50a12680a500a460a1f2f636f736d6f732e63727970746f2e736563703235366b312e5075624b657912230a21025a37975c079a7543603fcab24e2565a4adee3cf9af8934690e103282fa40251112040a020801180312140a0e0a05756e75636c1205333338383510c8d0071a40852793cb49aeaff1f895fa18a4fc0a63a5c54813fd57b3f5a2af9d0d849a04cb4abe81bc8feb4178603e1c9eed4e4464157f0bffb7cf51ef3beb80f48cd73b91").unwrap().as_slice()).unwrap();
        let mock_tx = fee_with_burn_tx.clone();
        TendermintCoin::request_tx.mock_safe(move |_, _| {
            let mock_tx = mock_tx.clone();
            MockResult::Return(Box::pin(async move { Ok(mock_tx) }))
        });

        let pubkey = get_tx_signer_pubkey_unprefixed(&fee_with_burn_tx, 0);
        let fee_with_burn_cosmos_tx = TransactionEnum::CosmosTransaction(CosmosTransaction {
            data: TxRaw::decode(fee_with_burn_tx.encode_to_vec().as_slice()).unwrap(),
        });

        let uuid: Uuid = "3ec8dd65-1064-4f06-baf3-f72eb5c9b04d".parse().unwrap();
        let dex_fee = DexFee::WithBurn {
            fee_amount: MmNumber::from("0.000007"), // Amount is 0.008, both dex and burn fees rounded down
            burn_amount: MmNumber::from("0.000002"),
            burn_destination: DexFeeBurnDestination::PreBurnAccount,
        };
        block_on(
            coin.validate_fee_for_denom(
                &fee_with_burn_cosmos_tx,
                &pubkey,
                &dex_fee,
                6,
                uuid.as_bytes(),
                iris_ibc_nucleus_denom,
            )
            .compat(),
        )
        .unwrap();
    }

    #[test]
    fn validate_payment_test() {
        let nodes = vec![RpcNode::for_test(IRIS_TESTNET_RPC_URL)];

        let protocol_conf = get_iris_protocol();

        let ctx = mm2_core::mm_ctx::MmCtxBuilder::default().into_mm_arc();

        let conf = TendermintConf {
            avg_blocktime: AVG_BLOCKTIME,
            derivation_path: None,
        };

        let key_pair = key_pair_from_seed(IRIS_TESTNET_HTLC_PAIR1_SEED).unwrap();
        let tendermint_pair = TendermintKeyPair::new(key_pair.private().secret, *key_pair.public());
        let activation_policy =
            TendermintActivationPolicy::with_private_key_policy(TendermintPrivKeyPolicy::Iguana(tendermint_pair));

        let coin = block_on(TendermintCoin::init(
            &ctx,
            "IRIS-TEST".to_string(),
            conf,
            protocol_conf,
            nodes,
            false,
            activation_policy,
            Default::default(),
        ))
        .unwrap();

        // just a random transfer tx not related to AtomicDEX, should fail because the message is not CreateHtlc
        // https://nyancat.iobscan.io/#/tx?txHash=65815814E7D74832D87956144C1E84801DC94FE9A509D207A0ABC3F17775E5DF
        let random_transfer_tx_hash = "65815814E7D74832D87956144C1E84801DC94FE9A509D207A0ABC3F17775E5DF";
        let random_transfer_tx_bytes = block_on(coin.request_tx(random_transfer_tx_hash.into()))
            .unwrap()
            .encode_to_vec();

        let input = ValidatePaymentInput {
            payment_tx: random_transfer_tx_bytes,
            time_lock_duration: 0,
            time_lock: 0,
            other_pub: Vec::new(),
            secret_hash: Vec::new(),
            amount: Default::default(),
            swap_contract_address: None,
            try_spv_proof_until: 0,
            confirmations: 0,
            unique_swap_data: Vec::new(),
            watcher_reward: None,
        };
        let validate_err = block_on(coin.validate_taker_payment(input)).unwrap_err();
        match validate_err.into_inner() {
            ValidatePaymentError::WrongPaymentTx(e) => assert!(e.contains("Incorrect CreateHtlc message")),
            unexpected => panic!("Unexpected error variant {:?}", unexpected),
        };

        // The HTLC that was already claimed or refunded should not pass the validation
        // https://nyancat.iobscan.io/#/tx?txHash=93CF377D470EB27BD6E2C5B95BFEFE99359F95B88C70D785B34D1D2C670201B9
        let claimed_htlc_tx_hash = "93CF377D470EB27BD6E2C5B95BFEFE99359F95B88C70D785B34D1D2C670201B9";
        let claimed_htlc_tx_bytes = block_on(coin.request_tx(claimed_htlc_tx_hash.into()))
            .unwrap()
            .encode_to_vec();

        let input = ValidatePaymentInput {
            payment_tx: claimed_htlc_tx_bytes,
            time_lock_duration: 20000,
            time_lock: 1664984893,
            other_pub: hex::decode("025a37975c079a7543603fcab24e2565a4adee3cf9af8934690e103282fa402511").unwrap(),
            secret_hash: hex::decode("441d0237e93677d3458e1e5a2e69f61e3622813521bf048dd56290306acdd134").unwrap(),
            amount: "0.01".parse().unwrap(),
            swap_contract_address: None,
            try_spv_proof_until: 0,
            confirmations: 0,
            unique_swap_data: Vec::new(),
            watcher_reward: None,
        };
        let validate_err = block_on(coin.validate_payment_for_denom(input, "nim".parse().unwrap(), 6)).unwrap_err();
        match validate_err.into_inner() {
            ValidatePaymentError::UnexpectedPaymentState(_) => (),
            unexpected => panic!("Unexpected error variant {:?}", unexpected),
        };
    }

    #[test]
    fn test_search_for_swap_tx_spend_spent() {
        let nodes = vec![RpcNode::for_test(IRIS_TESTNET_RPC_URL)];

        let protocol_conf = get_iris_protocol();

        let ctx = mm2_core::mm_ctx::MmCtxBuilder::default().into_mm_arc();

        let conf = TendermintConf {
            avg_blocktime: AVG_BLOCKTIME,
            derivation_path: None,
        };

        let key_pair = key_pair_from_seed(IRIS_TESTNET_HTLC_PAIR1_SEED).unwrap();
        let tendermint_pair = TendermintKeyPair::new(key_pair.private().secret, *key_pair.public());
        let activation_policy =
            TendermintActivationPolicy::with_private_key_policy(TendermintPrivKeyPolicy::Iguana(tendermint_pair));

        let coin = block_on(TendermintCoin::init(
            &ctx,
            "IRIS-TEST".to_string(),
            conf,
            protocol_conf,
            nodes,
            false,
            activation_policy,
            Default::default(),
        ))
        .unwrap();

        // https://nyancat.iobscan.io/#/tx?txHash=2DB382CE3D9953E4A94957B475B0E8A98F5B6DDB32D6BF0F6A765D949CF4A727
        let create_tx_hash = "2DB382CE3D9953E4A94957B475B0E8A98F5B6DDB32D6BF0F6A765D949CF4A727";

        let request = GetTxRequest {
            hash: create_tx_hash.into(),
        };

        let response = block_on(block_on(coin.rpc_client()).unwrap().abci_query(
            Some(ABCI_GET_TX_PATH.to_string()),
            request.encode_to_vec(),
            ABCI_REQUEST_HEIGHT,
            ABCI_REQUEST_PROVE,
        ))
        .unwrap();
        println!("{response:?}");

        let response = GetTxResponse::decode(response.value.as_slice()).unwrap();
        let tx = response.tx.unwrap();

        println!("{tx:?}");

        let encoded_tx = tx.encode_to_vec();

        let secret_hash = hex::decode("0C34C71EBA2A51738699F9F3D6DAFFB15BE576E8ED543203485791B5DA39D10D").unwrap();
        let input = SearchForSwapTxSpendInput {
            time_lock: 0,
            other_pub: &[],
            secret_hash: &secret_hash,
            tx: &encoded_tx,
            search_from_block: 0,
            swap_contract_address: &None,
            swap_unique_data: &[],
            watcher_reward: false,
        };

        let spend_tx = match block_on(coin.search_for_swap_tx_spend_my(input)).unwrap().unwrap() {
            FoundSwapTxSpend::Spent(tx) => tx,
            unexpected => panic!("Unexpected search_for_swap_tx_spend_my result {:?}", unexpected),
        };

        // https://nyancat.iobscan.io/#/tx?txHash=565C820C1F95556ADC251F16244AAD4E4274772F41BC13F958C9C2F89A14D137
        let expected_spend_hash = "565C820C1F95556ADC251F16244AAD4E4274772F41BC13F958C9C2F89A14D137";
        let hash = spend_tx.tx_hash_as_bytes();
        assert_eq!(hex::encode_upper(hash.0), expected_spend_hash);
    }

    #[test]
    fn test_search_for_swap_tx_spend_refunded() {
        let nodes = vec![RpcNode::for_test(IRIS_TESTNET_RPC_URL)];

        let protocol_conf = get_iris_protocol();

        let ctx = mm2_core::mm_ctx::MmCtxBuilder::default().into_mm_arc();

        let conf = TendermintConf {
            avg_blocktime: AVG_BLOCKTIME,
            derivation_path: None,
        };

        let key_pair = key_pair_from_seed(IRIS_TESTNET_HTLC_PAIR1_SEED).unwrap();
        let tendermint_pair = TendermintKeyPair::new(key_pair.private().secret, *key_pair.public());
        let activation_policy =
            TendermintActivationPolicy::with_private_key_policy(TendermintPrivKeyPolicy::Iguana(tendermint_pair));

        let coin = block_on(TendermintCoin::init(
            &ctx,
            "IRIS-TEST".to_string(),
            conf,
            protocol_conf,
            nodes,
            false,
            activation_policy,
            Default::default(),
        ))
        .unwrap();

        // https://nyancat.iobscan.io/#/tx?txHash=BD1A76F43E8E2C7A1104EE363D63455CD50C76F2BFE93B703235F0A973061297
        let create_tx_hash = "BD1A76F43E8E2C7A1104EE363D63455CD50C76F2BFE93B703235F0A973061297";

        let request = GetTxRequest {
            hash: create_tx_hash.into(),
        };

        let response = block_on(block_on(coin.rpc_client()).unwrap().abci_query(
            Some(ABCI_GET_TX_PATH.to_string()),
            request.encode_to_vec(),
            ABCI_REQUEST_HEIGHT,
            ABCI_REQUEST_PROVE,
        ))
        .unwrap();
        println!("{response:?}");

        let response = GetTxResponse::decode(response.value.as_slice()).unwrap();
        let tx = response.tx.unwrap();

        println!("{tx:?}");

        let encoded_tx = tx.encode_to_vec();

        let secret_hash = hex::decode("cb11cacffdfc82060aa4a9a1bb9cc094c4141b170994f7642cd54d7e7af6743e").unwrap();
        let input = SearchForSwapTxSpendInput {
            time_lock: 0,
            other_pub: &[],
            secret_hash: &secret_hash,
            tx: &encoded_tx,
            search_from_block: 0,
            swap_contract_address: &None,
            swap_unique_data: &[],
            watcher_reward: false,
        };

        match block_on(coin.search_for_swap_tx_spend_my(input)).unwrap().unwrap() {
            FoundSwapTxSpend::Refunded(tx) => {
                let expected = TransactionEnum::CosmosTransaction(CosmosTransaction { data: TxRaw::default() });
                assert_eq!(expected, tx);
            },
            unexpected => panic!("Unexpected search_for_swap_tx_spend_my result {:?}", unexpected),
        };
    }

    #[test]
    fn test_get_tx_status_code_or_none() {
        let nodes = vec![RpcNode::for_test(IRIS_TESTNET_RPC_URL)];
        let protocol_conf = get_iris_usdc_ibc_protocol();

        let conf = TendermintConf {
            avg_blocktime: AVG_BLOCKTIME,
            derivation_path: None,
        };

        let ctx = mm2_core::mm_ctx::MmCtxBuilder::default().into_mm_arc();
        let key_pair = key_pair_from_seed(IRIS_TESTNET_HTLC_PAIR1_SEED).unwrap();
        let tendermint_pair = TendermintKeyPair::new(key_pair.private().secret, *key_pair.public());
        let activation_policy =
            TendermintActivationPolicy::with_private_key_policy(TendermintPrivKeyPolicy::Iguana(tendermint_pair));

        let coin = common::block_on(TendermintCoin::init(
            &ctx,
            "USDC-IBC".to_string(),
            conf,
            protocol_conf,
            nodes,
            false,
            activation_policy,
            Default::default(),
        ))
        .unwrap();

        for succeed_tx_hash in SUCCEED_TX_HASH_SAMPLES {
            let status_code = common::block_on(coin.get_tx_status_code_or_none(succeed_tx_hash.to_string()))
                .unwrap()
                .expect("tx exists");

            assert_eq!(status_code, cosmrs::tendermint::abci::Code::Ok);
        }

        for failed_tx_hash in FAILED_TX_HASH_SAMPLES {
            let status_code = common::block_on(coin.get_tx_status_code_or_none(failed_tx_hash.to_string()))
                .unwrap()
                .expect("tx exists");

            assert_eq!(
                discriminant(&status_code),
                discriminant(&cosmrs::tendermint::abci::Code::Err(NonZeroU32::new(61).unwrap()))
            );
        }

        // Doesn't exists
        let tx_hash = "0000000000000000000000000000000000000000000000000000000000000000".to_string();
        let status_code = common::block_on(coin.get_tx_status_code_or_none(tx_hash)).unwrap();
        assert!(status_code.is_none());
    }

    #[test]
    fn test_wait_for_confirmations() {
        const CHECK_INTERVAL: u64 = 2;

        let nodes = vec![RpcNode::for_test(IRIS_TESTNET_RPC_URL)];
        let protocol_conf = get_iris_usdc_ibc_protocol();

        let conf = TendermintConf {
            avg_blocktime: AVG_BLOCKTIME,
            derivation_path: None,
        };

        let ctx = mm2_core::mm_ctx::MmCtxBuilder::default().into_mm_arc();
        let key_pair = key_pair_from_seed(IRIS_TESTNET_HTLC_PAIR1_SEED).unwrap();
        let tendermint_pair = TendermintKeyPair::new(key_pair.private().secret, *key_pair.public());
        let activation_policy =
            TendermintActivationPolicy::with_private_key_policy(TendermintPrivKeyPolicy::Iguana(tendermint_pair));

        let coin = common::block_on(TendermintCoin::init(
            &ctx,
            "USDC-IBC".to_string(),
            conf,
            protocol_conf,
            nodes,
            false,
            activation_policy,
            Default::default(),
        ))
        .unwrap();

        let wait_until = || wait_until_ms(45);

        for succeed_tx_hash in SUCCEED_TX_HASH_SAMPLES {
            let tx_bytes = block_on(coin.request_tx(succeed_tx_hash.to_string()))
                .unwrap()
                .encode_to_vec();

            let confirm_payment_input = ConfirmPaymentInput {
                payment_tx: tx_bytes,
                confirmations: 0,
                requires_nota: false,
                wait_until: wait_until(),
                check_every: CHECK_INTERVAL,
            };
            block_on(coin.wait_for_confirmations(confirm_payment_input).compat()).unwrap();
        }

        for failed_tx_hash in FAILED_TX_HASH_SAMPLES {
            let tx_bytes = block_on(coin.request_tx(failed_tx_hash.to_string()))
                .unwrap()
                .encode_to_vec();

            let confirm_payment_input = ConfirmPaymentInput {
                payment_tx: tx_bytes,
                confirmations: 0,
                requires_nota: false,
                wait_until: wait_until(),
                check_every: CHECK_INTERVAL,
            };
            block_on(coin.wait_for_confirmations(confirm_payment_input).compat()).unwrap_err();
        }
    }

    #[test]
    fn test_generate_account_id() {
        let key_pair = key_pair_from_seed("best seed").unwrap();

        let tendermint_pair = TendermintKeyPair::new(key_pair.private().secret, *key_pair.public());
        let pb = PublicKey::from_raw_secp256k1(&key_pair.public().to_bytes()).unwrap();

        let pk_activation_policy =
            TendermintActivationPolicy::with_private_key_policy(TendermintPrivKeyPolicy::Iguana(tendermint_pair));
        // Derive account id from the private key.
        let pk_account_id = pk_activation_policy.generate_account_id("cosmos").unwrap();
        assert_eq!(
            pk_account_id.to_string(),
            "cosmos1aghdjgt5gzntzqgdxdzhjfry90upmtfsy2wuwp"
        );

        let pb_activation_policy = TendermintActivationPolicy::with_public_key(pb);
        // Derive account id from the public key.
        let pb_account_id = pb_activation_policy.generate_account_id("cosmos").unwrap();
        // Public and private keys are from the same keypair, account ids must be equal.
        assert_eq!(pk_account_id, pb_account_id);
    }

    #[test]
    fn test_parse_expected_sequence_number() {
        assert_eq!(
            13,
            parse_expected_sequence_number("check_tx log: account sequence mismatch, expected 13").unwrap()
        );
        assert_eq!(
            5,
            parse_expected_sequence_number("check_tx log: account sequence mismatch, expected 5, got...").unwrap()
        );
        assert_eq!(17, parse_expected_sequence_number("account sequence mismatch, expected. check_tx log: account sequence mismatch, expected 17, got 16: incorrect account sequence, deliver_tx log...").unwrap());
        assert!(parse_expected_sequence_number("").is_err());
        assert!(parse_expected_sequence_number("check_tx log: account sequence mismatch, expected").is_err());
    }

    #[test]
    fn test_extract_big_decimal_from_dec_coin() {
        let dec_coin = DecCoin {
            denom: "".into(),
            amount: "232503485176823921544000".into(),
        };

        let expected = BigDecimal::from_str("0.232503485176823921544").unwrap();
        let actual = extract_big_decimal_from_dec_coin(&dec_coin, 6).unwrap();
        assert_eq!(expected, actual);

        let dec_coin = DecCoin {
            denom: "".into(),
            amount: "1000000000000000000000000".into(),
        };

        let expected = BigDecimal::from(1);
        let actual = extract_big_decimal_from_dec_coin(&dec_coin, 6).unwrap();
        assert_eq!(expected, actual);
    }

    #[ignore = "insufficient balance"]
    #[test]
    fn test_claim_staking_rewards() {
        let nodes = vec![RpcNode::for_test(IRIS_TESTNET_RPC_URL)];
        let protocol_conf = get_iris_protocol();
        let conf = TendermintConf {
            avg_blocktime: AVG_BLOCKTIME,
            derivation_path: None,
        };

        let ctx = mm2_core::mm_ctx::MmCtxBuilder::default().into_mm_arc();
        let key_pair = key_pair_from_seed(IRIS_TESTNET_HTLC_PAIR1_SEED).unwrap();
        let tendermint_pair = TendermintKeyPair::new(key_pair.private().secret, *key_pair.public());
        let activation_policy =
            TendermintActivationPolicy::with_private_key_policy(TendermintPrivKeyPolicy::Iguana(tendermint_pair));

        let coin = block_on(TendermintCoin::init(
            &ctx,
            "IRIS-TEST".to_string(),
            conf,
            protocol_conf,
            nodes,
            false,
            activation_policy,
            Default::default(),
        ))
        .unwrap();

        let validator_address = "iva1svannhv2zaxefq83m7treg078udfk37lpjufkw";
        let memo = "test".to_owned();
        let req = ClaimRewardsPayload {
            validator_address: validator_address.to_owned(),
            fee: None,
            memo: memo.clone(),
            force: false,
        };
        let reward_amount =
            block_on(coin.get_delegation_reward_amount(&AccountId::from_str(validator_address).unwrap())).unwrap();
        let res = block_on(coin.claim_staking_rewards(req)).unwrap();

        assert_eq!(vec![validator_address], res.from);
        assert_eq!(vec![coin.account_id.to_string()], res.to);
        assert_eq!(TransactionType::ClaimDelegationRewards, res.transaction_type);
        assert_eq!(Some(memo), res.memo);
        // Rewards can increase during our tests, so round the first 4 digits.
        assert_eq!(reward_amount.round(4), res.total_amount.round(4));
        assert_eq!(reward_amount.round(4), res.received_by_me.round(4));
        // tx fee must be taken into account
        assert!(reward_amount > res.my_balance_change);
    }

    #[test]
    fn test_delegations_list() {
        let nodes = vec![RpcNode::for_test(IRIS_TESTNET_RPC_URL)];
        let protocol_conf = get_iris_protocol();
        let conf = TendermintConf {
            avg_blocktime: AVG_BLOCKTIME,
            derivation_path: None,
        };

        let ctx = mm2_core::mm_ctx::MmCtxBuilder::default().into_mm_arc();
        let key_pair = key_pair_from_seed(IRIS_TESTNET_HTLC_PAIR1_SEED).unwrap();
        let tendermint_pair = TendermintKeyPair::new(key_pair.private().secret, *key_pair.public());
        let activation_policy =
            TendermintActivationPolicy::with_private_key_policy(TendermintPrivKeyPolicy::Iguana(tendermint_pair));

        let coin = block_on(TendermintCoin::init(
            &ctx,
            "IRIS-TEST".to_string(),
            conf,
            protocol_conf,
            nodes,
            false,
            activation_policy,
            Default::default(),
        ))
        .unwrap();

        let validator_address = "iva1svannhv2zaxefq83m7treg078udfk37lpjufkw";
        let reward_amount =
            block_on(coin.get_delegation_reward_amount(&AccountId::from_str(validator_address).unwrap())).unwrap();

        let expected_list = DelegationsQueryResponse {
            delegations: vec![Delegation {
                validator_address: validator_address.to_owned(),
                delegated_amount: BigDecimal::from_str("1.98").unwrap(),
                reward_amount: reward_amount.round(4),
            }],
        };

        let mut actual_list = block_on(coin.delegations_list(PagingOptions {
            limit: 0,
            page_number: NonZeroUsize::new(1).unwrap(),
            from_uuid: None,
        }))
        .unwrap();
        for delegation in &mut actual_list.delegations {
            delegation.reward_amount = delegation.reward_amount.round(4);
        }

        assert_eq!(expected_list, actual_list);
    }

    #[test]
    fn test_get_ibc_channel_for_target_address() {
        let nodes = vec![RpcNode::for_test(IRIS_TESTNET_RPC_URL)];
        let protocol_conf = get_iris_protocol();
        let ctx = mm2_core::mm_ctx::MmCtxBuilder::default().into_mm_arc();
        let conf = TendermintConf {
            avg_blocktime: AVG_BLOCKTIME,
            derivation_path: None,
        };

        let key_pair = key_pair_from_seed(IRIS_TESTNET_HTLC_PAIR1_SEED).unwrap();
        let tendermint_pair = TendermintKeyPair::new(key_pair.private().secret, *key_pair.public());
        let activation_policy =
            TendermintActivationPolicy::with_private_key_policy(TendermintPrivKeyPolicy::Iguana(tendermint_pair));

        let coin = block_on(TendermintCoin::init(
            &ctx,
            "IRIS-TEST".to_string(),
            conf,
            protocol_conf,
            nodes,
            false,
            activation_policy,
            Default::default(),
        ))
        .unwrap();

        let expected_channel = ChannelId::new(0);
        let expected_channel_str = "channel-0";

        let actual_channel = block_on(coin.get_healthy_ibc_channel_for_address_prefix("cosmos")).unwrap();
        let actual_channel_str = actual_channel.to_string();

        assert_eq!(expected_channel, actual_channel);
        assert_eq!(expected_channel_str, actual_channel_str);
    }
}
