use super::{
    BalanceError, CoinBalance, CoinsContext, HistorySyncState, MarketCoinOps, MmCoin, RawTransactionError,
    RawTransactionFut, RawTransactionRequest, RawTransactionResult, SignRawTransactionRequest, SignatureError, SwapOps,
    SwapTxTypeWithSecretHash, TradeFee, TransactionData, TransactionDetails, TransactionEnum, TransactionErr,
    TransactionType, VerificationError,
};
use crate::hd_wallet::HDAddressSelector;
use crate::siacoin::sia_withdraw::SiaWithdrawBuilder;
use crate::{
    coin_errors::{AddressFromPubkeyError, MyAddressError},
    now_sec, BalanceFut, CanRefundHtlc, CheckIfMyPaymentSentArgs, ConfirmPaymentInput, DexFee, FeeApproxStage,
    FoundSwapTxSpend, NegotiateSwapContractAddrErr, PrivKeyBuildPolicy, PrivKeyPolicy, RawTransactionRes,
    RefundPaymentArgs, SearchForSwapTxSpendInput, SendPaymentArgs, SignatureResult, SpendPaymentArgs, TradePreimageFut,
    TradePreimageResult, TradePreimageValue, Transaction, TransactionResult, TxMarshalingErr,
    UnexpectedDerivationMethod, ValidateAddressResult, ValidateFeeArgs, ValidateOtherPubKeyErr, ValidatePaymentError,
    ValidatePaymentInput, ValidatePaymentResult, VerificationResult, WaitForHTLCTxSpendArgs, WatcherOps, WeakSpawner,
    WithdrawFut, WithdrawRequest,
};
use async_trait::async_trait;
use bitcrypto::sha256;
use common::executor::abortable_queue::AbortableQueue;
use common::executor::{AbortableSystem, AbortedError, Timer};
use common::log::{debug, info};
use common::DEX_FEE_PUBKEY_ED25519;
use derive_more::{Display, From, Into};
use rpc::v1::types::H264 as H264Json;
/*
TODO Alright — this is now the third type in our codebase representing BIP32 derivation paths.

We currently have:
- `ed25519_dalek_bip32::DerivationPath`
- `bip32::DerivationPath`
- Type aliases like `StandardHDPath`, `HDPathToCoin` and `HDPathToAccount` in `standard_hd_path.rs`
- `RpcDerivationPath`


This is named "DalekDerivationPath" to avoid confusion with bip32::DerivationPath, but they represent
the same thing conceptually.
 */
use ed25519_dalek_bip32::DerivationPath as DalekDerivationPath;
use futures::compat::Future01CompatExt;
use futures::{FutureExt, TryFutureExt};
use futures01::Future;
use hex;
use keys::KeyPair;
use mm2_core::mm_ctx::MmArc;
use mm2_number::num_bigint::ToBigInt;
use mm2_number::{BigDecimal, BigInt, MmNumber};
use num_traits::ToPrimitive;
use rpc::v1::types::{Bytes as BytesJson, H256 as H256Json};
use serde_json::Value as Json;
// expose all of sia-rust so mm2_main can use it via coins::siacoin::sia_rust
pub use sia_rust;
pub use sia_rust::transport::client::{ApiClient as SiaApiClient, ApiClientHelpers};
pub use sia_rust::transport::endpoints::{
    AddressesEventsRequest, ConsensusTipRequest, GetAddressUtxosRequest, GetEventRequest, TxpoolBroadcastRequest,
    TxpoolTransactionsRequest, TxpoolTransactionsResponse,
};
pub use sia_rust::types::{
    Address, Currency, Event, EventDataWrapper, EventPayout, EventType, Hash256, Hash256Error, Keypair as SiaKeypair,
    KeypairError, Preimage, PreimageError, PublicKey, PublicKeyError, SiacoinElement, SiacoinOutput, SiacoinOutputId,
    SpendPolicy, TransactionId, V1Transaction, V2Transaction,
};
pub use sia_rust::utils::{V2TransactionBuilder, V2TransactionBuilderError};
use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::convert::TryFrom;
use std::fmt;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

use mm2_err_handle::prelude::*;

pub mod error;
pub use error::SiaCoinNewError;
use error::*;

pub mod sia_hd_wallet;
mod sia_withdraw;

/*
The wasm and native modules should act identically *except* the ClientError associated type as it
wraps some transport specific error types.

Avoid doing any conditional logic on any of the client_error types.
*/
pub use sia_rust::transport::client::{error as client_error, Client as SiaClient};

pub type SiaCoin = SiaCoinGeneric<SiaClient>;
pub type SiaClientConf = <SiaClient as SiaApiClient>::Conf;

lazy_static! {
    pub static ref FEE_PUBLIC_KEY_BYTES: Vec<u8> =
        hex::decode(DEX_FEE_PUBKEY_ED25519).expect("DEX_FEE_PUBKEY_ED25510 is a valid hex string");
    pub static ref FEE_PUBLIC_KEY: PublicKey =
        PublicKey::from_bytes(&FEE_PUBLIC_KEY_BYTES).expect("DEX_FEE_PUBKEY_ED25510 is a valid PublicKey");
    pub static ref FEE_ADDR: Address = Address::from_public_key(&FEE_PUBLIC_KEY);
    pub static ref SINGLE_ADDRESS_MODE_PATH: DalekDerivationPath =
        DalekDerivationPath::from_str("m/44'/1991'/0'/0'/0'").expect("Valid single address mode path");
}

/// The index of the HTLC output in the transaction that locks the funds
/// u32 is used to because this is generally used as an index of a Vec or slice
/// Setting usize would result in a u64->u32 cast in some cases, and we want to avoid that.
const HTLC_VOUT_INDEX: u32 = 0;

// TODO see https://github.com/KomodoPlatform/komodo-defi-framework/pull/2086#discussion_r1521668313
// for additional fields needed
#[derive(Clone)]
pub struct SiaCoinGeneric<T: SiaApiClient + ApiClientHelpers> {
    /// SIA coin config
    pub conf: SiaCoinConf,
    pub priv_key_policy: Arc<PrivKeyPolicy<SiaKeypair>>,
    /// Client used to interact with the blockchain, most likely an HTTP(s) client
    pub client: Arc<T>,
    /// State of the transaction history loop (enabled, started, in progress, etc.)
    pub history_sync_state: Arc<Mutex<HistorySyncState>>,
    /// This abortable system is used to spawn coin's related futures that should be aborted on coin deactivation
    /// and on [`MmArc::stop`].
    pub abortable_system: Arc<AbortableQueue>,
    required_confirmations: Arc<AtomicU64>,
}

impl WatcherOps for SiaCoin {}

/// The JSON configuration loaded from `coins` file
#[derive(Clone, Debug, Deserialize)]
pub struct SiaCoinConf {
    #[serde(rename = "coin")]
    pub ticker: String,
    pub required_confirmations: u64,
}

// TODO see https://github.com/KomodoPlatform/komodo-defi-framework/pull/2086#discussion_r1521660384
// for additional fields needed
/// SiaCoinActivationRequest represents the deserialized JSON body from the `enable` RPC command
#[derive(Clone, Debug, Deserialize)]
pub struct SiaCoinActivationRequest {
    #[serde(default)]
    pub tx_history: bool,
    pub required_confirmations: Option<u64>,
    pub gap_limit: Option<u32>,
    pub client_conf: SiaClientConf,
}

#[derive(Debug, Display)]
pub enum SiaCoinFromLegacyReqErr {
    InvalidRequiredConfs(serde_json::Error),
    InvalidGapLimit(serde_json::Error),
    InvalidClientConf(serde_json::Error),
}

impl SiaCoinActivationRequest {
    pub fn from_legacy_req(req: &Json) -> Result<Self, MmError<SiaCoinFromLegacyReqErr>> {
        let tx_history = req["tx_history"].as_bool().unwrap_or_default();
        let required_confirmations = serde_json::from_value(req["required_confirmations"].clone())
            .map_to_mm(SiaCoinFromLegacyReqErr::InvalidRequiredConfs)?;
        let gap_limit =
            serde_json::from_value(req["gap_limit"].clone()).map_to_mm(SiaCoinFromLegacyReqErr::InvalidGapLimit)?;
        let client_conf =
            serde_json::from_value(req["client_conf"].clone()).map_to_mm(SiaCoinFromLegacyReqErr::InvalidClientConf)?;

        Ok(SiaCoinActivationRequest {
            tx_history,
            required_confirmations,
            gap_limit,
            client_conf,
        })
    }
}

impl SiaCoin {
    pub async fn new(
        ctx: &MmArc,
        json_conf: Json,
        request: &SiaCoinActivationRequest,
        priv_key_policy: PrivKeyBuildPolicy,
    ) -> Result<Self, MmError<SiaCoinNewError>> {
        let key_pair = match priv_key_policy {
            PrivKeyBuildPolicy::IguanaPrivKey(priv_key) => SiaKeypair::from_private_bytes(priv_key.as_slice())?,
            PrivKeyBuildPolicy::GlobalHDAccount(global_hd_account) => {
                // generate the keypair from SINGLE_ADDRESS_MODE_PATH to be used for a "single address mode" for now
                let extended_key = global_hd_account
                    .derive_ed25519_signing_key(&SINGLE_ADDRESS_MODE_PATH)
                    // TODO this map_err shouldn't be neccesary but From MmError<E1> for MmError<E2>
                    // impl is broken only for wasm targets, why?
                    .map_err(|e| e.into_inner())?;
                SiaKeypair::from_private_bytes(extended_key.signing_key.as_bytes())?
            },
            _ => return Err(SiaCoinNewError::UnsupportedPrivKeyPolicy.into()),
        };

        // parse the "coins" file JSON configuration
        let conf: SiaCoinConf = serde_json::from_value(json_conf)?;

        Ok(SiaCoinBuilder::new(ctx, conf, key_pair, request).build().await?)
    }
}

pub struct SiaCoinBuilder<'a> {
    ctx: &'a MmArc,
    conf: SiaCoinConf,
    key_pair: SiaKeypair,
    request: &'a SiaCoinActivationRequest,
}

impl<'a> SiaCoinBuilder<'a> {
    pub fn new(ctx: &'a MmArc, conf: SiaCoinConf, key_pair: SiaKeypair, request: &'a SiaCoinActivationRequest) -> Self {
        SiaCoinBuilder {
            ctx,
            conf,
            key_pair,
            request,
        }
    }

    // TODO Alright - update to follow the new error handling pattern
    async fn build(self) -> Result<SiaCoin, SiaCoinBuilderError> {
        let abortable_queue: AbortableQueue = self
            .ctx
            .abortable_system
            .create_subsystem()
            .map_err(SiaCoinBuilderError::AbortableSystem)?;
        let abortable_system = Arc::new(abortable_queue);
        let history_sync_state = if self.request.tx_history {
            HistorySyncState::NotStarted
        } else {
            HistorySyncState::NotEnabled
        };

        // Use required_confirmations from activation request if it's set, otherwise use the value from coins conf
        let required_confirmations: AtomicU64 = self
            .request
            .required_confirmations
            .unwrap_or(self.conf.required_confirmations)
            .into();

        Ok(SiaCoin {
            conf: self.conf,
            client: Arc::new(
                SiaClient::new(self.request.client_conf.clone())
                    .await
                    .map_err(SiaCoinBuilderError::Client)?,
            ),
            priv_key_policy: PrivKeyPolicy::Iguana(self.key_pair).into(),
            history_sync_state: Mutex::new(history_sync_state).into(),
            abortable_system,
            required_confirmations: required_confirmations.into(),
        })
    }
}

/// Convert hastings representation to "coin" amount
/// BigDecimal(1) == 1 SC == 10^24 hastings
/// 1 H == 0.000000000000000000000001 SC
fn hastings_to_siacoin(hastings: Currency) -> BigDecimal {
    let hastings: u128 = hastings.into();
    BigDecimal::new(BigInt::from(hastings), 24)
}

/// Convert "coin" representation to hastings amount
/// BigDecimal(1) == 1 SC == 10^24 hastings
// TODO Alright it's not ideal that we require these standalone helpers, but a newtype of Currency is even messier
fn siacoin_to_hastings(siacoin: BigDecimal) -> Result<Currency, SiacoinToHastingsError> {
    // Shift the decimal place to the right by 24 places (10^24)
    let decimals = BigInt::from(10u128.pow(24));
    let hastings = siacoin.clone() * BigDecimal::from(decimals);
    hastings
        .to_bigint()
        .ok_or(SiacoinToHastingsError::BigDecimalToBigInt(siacoin.clone()))?
        .to_u128()
        .ok_or(SiacoinToHastingsError::BigIntToU128(siacoin))
        .map(Currency)
}

// TODO Alright - refactor and move to siacoin::error
// #[derive(Debug, Error)]
// pub enum FrameworkErrorWga {
//     #[error(
//         "Sia select_outputs insufficent amount, available: {:?} required: {:?}",
//         available,
//         required
//     )]
//     SelectOutputsInsufficientAmount { available: Currency, required: Currency },
//     #[error("Sia TransactionErr {:?}", _0)]
//     MmTransactionErr(TransactionErr),
//     #[error("Sia MyAddressError: `{0}`")]
//     MyAddressError(MyAddressError),
// }

// impl From<TransactionErr> for FrameworkError {
//     fn from(e: TransactionErr) -> Self { FrameworkError::MmTransactionErr(e) }
// }

// impl From<MyAddressError> for FrameworkError {
//     fn from(e: MyAddressError) -> Self { FrameworkError::MyAddressError(e) }
// }

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SiaCoinProtocolInfo;

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum SiaFeePolicy {
    Fixed,
    HastingsPerByte(Currency),
    Unknown,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct SiaFeeDetails {
    pub coin: String,
    pub policy: SiaFeePolicy,
    pub total_amount: BigDecimal,
}

#[async_trait]
impl MmCoin for SiaCoin {
    fn spawner(&self) -> WeakSpawner {
        self.abortable_system.weak_spawner()
    }

    /*
    TODO: refactor MmCoin to remove or better generalize this method
    No Sia software ever presents the user with a hex representation of a transaction. Transactions
    are always presented or taken as user input as JSON.
    Ideally, we would use an associated type within the response to allow returning
    the transaction as a JSON. For now, we encode JSON to hex and return this hex string.
    */
    fn get_raw_transaction(&self, req: RawTransactionRequest) -> RawTransactionFut<'_> {
        let fut = async move {
            let txid = match Hash256::from_str(&req.tx_hash).map_err(|e| {
                RawTransactionError::InternalError(format!("SiaCoin::get_raw_transaction: failed to parse txid: {}", e))
            }) {
                Ok(tx_hash) => tx_hash,
                Err(e) => return Err(e.into()),
            };
            let tx = match self.client.get_transaction(&txid).await.map_err(|e| {
                RawTransactionError::InternalError(format!(
                    "SiaCoin::get_raw_transaction: failed to fetch txid:{} :{}",
                    txid, e
                ))
            }) {
                Ok(tx) => tx,
                Err(e) => return Err(e.into()),
            };
            let tx_hex = SiaTransaction(tx).tx_hex();
            Ok(RawTransactionRes { tx_hex: tx_hex.into() })
        };
        Box::new(fut.boxed().compat())
    }

    // TODO Alright - this is only applicable to Watcher logic and will be removed from MmCoin trait
    fn get_tx_hex_by_hash(&self, _tx_hash: Vec<u8>) -> RawTransactionFut<'_> {
        let fut = async move {
            Err(RawTransactionError::InternalError(
                "SiaCoin::get_tx_hex_by_hash: Unsupported".to_string(),
            ))?
        };
        Box::new(fut.boxed().compat())
    }

    fn withdraw(&self, req: WithdrawRequest) -> WithdrawFut {
        let coin = self.clone();
        let fut = async move {
            let builder = SiaWithdrawBuilder::new(&coin, req)?;
            builder.build().await
        };
        Box::new(fut.boxed().compat())
    }

    fn decimals(&self) -> u8 {
        24
    }

    fn convert_to_address(&self, _from: &str, _to_address_format: Json) -> Result<String, String> {
        Err("SiaCoin::convert_to_address: Unsupported".to_string())
    }

    fn validate_address(&self, address: &str) -> ValidateAddressResult {
        match Address::from_str(address) {
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

    // Todo: deprecate this due to the use of attempts once tx_history_v2 is implemented
    fn process_history_loop(&self, ctx: MmArc) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        if self.history_sync_status() == HistorySyncState::NotEnabled {
            return Box::new(futures01::future::ok(()));
        }

        let mut my_balance: Option<CoinBalance> = None;
        let coin = self.clone();

        let fut = async move {
            let history = match coin.load_history_from_file(&ctx).compat().await {
                Ok(history) => history,
                Err(e) => {
                    log_tag!(
                        ctx,
                        "",
                        "tx_history",
                        "coin" => coin.conf.ticker;
                        fmt = "Error {} on 'load_history_from_file', stop the history loop", e
                    );
                    return;
                },
            };

            let mut history_map: HashMap<H256Json, TransactionDetails> = history
                .into_iter()
                .filter_map(|tx| {
                    let tx_hash = H256Json::from_str(tx.tx.tx_hash()?).ok()?;
                    Some((tx_hash, tx))
                })
                .collect();

            let mut success_iteration = 0i32;
            let mut attempts = 0;
            loop {
                if ctx.is_stopping() {
                    break;
                };
                {
                    let coins_ctx = CoinsContext::from_ctx(&ctx).unwrap();
                    let coins = coins_ctx.coins.lock().await;
                    if !coins.contains_key(&coin.conf.ticker) {
                        log_tag!(ctx, "", "tx_history", "coin" => coin.conf.ticker; fmt = "Loop stopped");
                        attempts += 1;
                        if attempts > 6 {
                            log_tag!(
                                ctx,
                                "",
                                "tx_history",
                                "coin" => coin.conf.ticker;
                                fmt = "Loop stopped after 6 attempts to find coin in coins context"
                            );
                            break;
                        }
                        Timer::sleep(10.).await;
                        continue;
                    };
                }

                let actual_balance = match coin.my_balance().compat().await {
                    Ok(actual_balance) => Some(actual_balance),
                    Err(err) => {
                        log_tag!(
                            ctx,
                            "",
                            "tx_history",
                            "coin" => coin.conf.ticker;
                            fmt = "Error {:?} on getting balance", err
                        );
                        None
                    },
                };

                let need_update = history_map.iter().any(|(_, tx)| tx.should_update());
                match (&my_balance, &actual_balance) {
                    (Some(prev_balance), Some(actual_balance)) if prev_balance == actual_balance && !need_update => {
                        // my balance hasn't been changed, there is no need to reload tx_history
                        Timer::sleep(30.).await;
                        continue;
                    },
                    _ => (),
                }

                // Todo: get mempool transactions and update them once they have confirmations
                let filtered_events: Vec<Event> = match coin.request_events_history().await {
                    Ok(events) => events
                        .into_iter()
                        .filter(|event| {
                            event.event_type == EventType::V2Transaction
                                || event.event_type == EventType::V1Transaction
                                || event.event_type == EventType::Miner
                                || event.event_type == EventType::Foundation
                        })
                        .collect(),
                    Err(e) => {
                        log_tag!(
                            ctx,
                            "",
                            "tx_history",
                            "coin" => coin.conf.ticker;
                            fmt = "Error {} on 'request_events_history', stop the history loop", e
                        );

                        Timer::sleep(10.).await;
                        continue;
                    },
                };

                // Remove transactions in the history_map that are not in the requested transaction list anymore
                let history_length = history_map.len();
                let requested_ids: HashSet<H256Json> = filtered_events.iter().map(|x| H256Json(x.id.0)).collect();
                history_map.retain(|hash, _| requested_ids.contains(hash));

                if history_map.len() < history_length {
                    let to_write: Vec<TransactionDetails> = history_map.values().cloned().collect();
                    if let Err(e) = coin.save_history_to_file(&ctx, to_write).compat().await {
                        log_tag!(
                            ctx,
                            "",
                            "tx_history",
                            "coin" => coin.conf.ticker;
                            fmt = "Error {} on 'save_history_to_file', stop the history loop", e
                        );
                        return;
                    };
                }

                let mut transactions_left = if requested_ids.len() > history_map.len() {
                    *coin.history_sync_state.lock().unwrap() = HistorySyncState::InProgress(json!({
                        "transactions_left": requested_ids.len() - history_map.len()
                    }));
                    requested_ids.len() - history_map.len()
                } else {
                    *coin.history_sync_state.lock().unwrap() = HistorySyncState::InProgress(json!({
                        "transactions_left": 0
                    }));
                    0
                };

                for txid in requested_ids {
                    let mut updated = false;
                    match history_map.entry(txid) {
                        Entry::Vacant(e) => match filtered_events.iter().find(|event| H256Json(event.id.0) == txid) {
                            Some(event) => {
                                let tx_details = match coin.tx_details_from_event(event) {
                                    Ok(tx_details) => tx_details,
                                    Err(e) => {
                                        log_tag!(
                                            ctx,
                                            "",
                                            "tx_history",
                                            "coin" => coin.conf.ticker;
                                            fmt = "Error {} on 'tx_details_from_event', stop the history loop", e
                                        );
                                        return;
                                    },
                                };
                                e.insert(tx_details);
                                if transactions_left > 0 {
                                    transactions_left -= 1;
                                    *coin.history_sync_state.lock().unwrap() =
                                        HistorySyncState::InProgress(json!({ "transactions_left": transactions_left }));
                                }
                                updated = true;
                            },
                            None => log_tag!(
                                ctx,
                                "",
                                "tx_history",
                                "coin" => coin.conf.ticker;
                                fmt = "Transaction with id {} not found in the events list", txid
                            ),
                        },
                        Entry::Occupied(_) => {},
                    }
                    if updated {
                        let to_write: Vec<TransactionDetails> = history_map.values().cloned().collect();
                        if let Err(e) = coin.save_history_to_file(&ctx, to_write).compat().await {
                            log_tag!(
                                ctx,
                                "",
                                "tx_history",
                                "coin" => coin.conf.ticker;
                                fmt = "Error {} on 'save_history_to_file', stop the history loop", e
                            );
                            return;
                        };
                    }
                }
                *coin.history_sync_state.lock().unwrap() = HistorySyncState::Finished;

                if success_iteration == 0 {
                    log_tag!(
                        ctx,
                        "😅",
                        "tx_history",
                        "coin" => coin.conf.ticker;
                        fmt = "history has been loaded successfully"
                    );
                }

                my_balance = actual_balance;
                success_iteration += 1;
                Timer::sleep(30.).await;
            }
        };

        Box::new(fut.map(|_| Ok(())).boxed().compat())
    }

    fn history_sync_status(&self) -> HistorySyncState {
        self.history_sync_state.lock().unwrap().clone()
    }

    // This is only utilized by the now deprecated get_trade_fee RPC method and should be removed
    // from the MmCoin trait
    fn get_trade_fee(&self) -> Box<dyn Future<Item = TradeFee, Error = String> + Send> {
        Box::new(futures01::future::err("SiaCoin::get_trade_fee: Unsupported".into()))
    }

    // Todo: Modify this when not using `DEFAULT_FEE`
    async fn get_sender_trade_fee(
        &self,
        _value: TradePreimageValue,
        _stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        Ok(TradeFee {
            coin: self.conf.ticker.clone(),
            amount: hastings_to_siacoin(Currency::DEFAULT_FEE).into(),
            paid_from_trading_vol: false,
        })
    }

    /// Get the transaction fee required to spend the HTLC output
    // Todo: Modify this when not using `DEFAULT_FEE`
    fn get_receiver_trade_fee(&self, _stage: FeeApproxStage) -> TradePreimageFut<TradeFee> {
        let ticker = self.conf.ticker.clone();
        let fut = async move {
            Ok(TradeFee {
                coin: ticker,
                amount: hastings_to_siacoin(Currency::DEFAULT_FEE).into(),
                paid_from_trading_vol: true,
            })
        };
        Box::new(fut.boxed().compat())
    }

    // Todo: Modify this when not using `DEFAULT_FEE`
    async fn get_fee_to_send_taker_fee(
        &self,
        _dex_fee_amount: DexFee,
        _stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        Ok(TradeFee {
            coin: self.conf.ticker.clone(),
            amount: hastings_to_siacoin(Currency::DEFAULT_FEE).into(),
            paid_from_trading_vol: false,
        })
    }

    fn required_confirmations(&self) -> u64 {
        self.required_confirmations.load(AtomicOrdering::Relaxed)
    }

    fn requires_notarization(&self) -> bool {
        false
    }

    fn set_required_confirmations(&self, confirmations: u64) {
        self.required_confirmations
            .store(confirmations, AtomicOrdering::Relaxed);
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

#[async_trait]
impl MarketCoinOps for SiaCoin {
    fn ticker(&self) -> &str {
        &self.conf.ticker
    }

    fn my_address(&self) -> MmResult<String, MyAddressError> {
        let key_pair = match &*self.priv_key_policy {
            PrivKeyPolicy::Iguana(key_pair) => key_pair,
            PrivKeyPolicy::Trezor | PrivKeyPolicy::HDWallet { .. } => {
                return Err(MyAddressError::UnexpectedDerivationMethod(
                    "SiaCoin::my_address: Unexpected Key Derivation Method.".to_string(),
                )
                .into());
            },
            #[cfg(target_arch = "wasm32")]
            PrivKeyPolicy::Metamask(_) => {
                return Err(MyAddressError::UnexpectedDerivationMethod(
                    "SiaCoin::my_address: Unexpected Key Derivation Method."
                        .to_string()
                        .to_string(),
                )
                .into());
            },
            PrivKeyPolicy::WalletConnect { .. } => {
                return Err(MyAddressError::UnexpectedDerivationMethod(
                    "WalletConnect not yet supported. Must use iguana seed.".to_string(),
                )
                .into())
            },
        };
        let address = key_pair.public().address();
        Ok(address.to_string())
    }

    // Todo: Implement in this PR, this was added to dev while work in this code was being done
    fn address_from_pubkey(&self, pubkey: &H264Json) -> MmResult<String, AddressFromPubkeyError> {
        let pubkey_bytes = &pubkey.0[..32];
        let pubkey =
            PublicKey::from_bytes(pubkey_bytes).map_err(|e| AddressFromPubkeyError::InternalError(e.to_string()))?;
        Ok(pubkey.address().to_string())
    }

    async fn get_public_key(&self) -> Result<String, MmError<UnexpectedDerivationMethod>> {
        let public_key = match &*self.priv_key_policy {
            PrivKeyPolicy::Iguana(key_pair) => key_pair.public(),
            PrivKeyPolicy::Trezor => {
                return MmError::err(UnexpectedDerivationMethod::ExpectedSingleAddress);
            },
            PrivKeyPolicy::HDWallet { .. } => {
                return MmError::err(UnexpectedDerivationMethod::ExpectedSingleAddress);
            },
            #[cfg(target_arch = "wasm32")]
            PrivKeyPolicy::Metamask(_) => {
                return MmError::err(UnexpectedDerivationMethod::ExpectedSingleAddress);
            },
            PrivKeyPolicy::WalletConnect { .. } => {
                return MmError::err(UnexpectedDerivationMethod::ExpectedSingleAddress);
            },
        };
        Ok(public_key.to_string())
    }

    // TODO Alright - Unsupported and will be removed - see dev comments in trait declaration
    fn sign_message_hash(&self, _message: &str) -> Option<[u8; 32]> {
        None
    }

    // Todo: needed as part of feature completion
    fn sign_message(&self, _message: &str, _address: Option<HDAddressSelector>) -> SignatureResult<String> {
        MmError::err(SignatureError::InternalError(
            "SiaCoin::sign_message: Unsupported".to_string(),
        ))
    }

    fn verify_message(&self, _signature: &str, _message: &str, _address: &str) -> VerificationResult<bool> {
        MmError::err(VerificationError::InternalError(
            "SiaCoin::verify_message: Unsupported".to_string(),
        ))
    }

    fn my_balance(&self) -> BalanceFut<CoinBalance> {
        let coin = self.clone();
        let fut = async move {
            let my_address = match &*coin.priv_key_policy {
                PrivKeyPolicy::Iguana(key_pair) => key_pair.public().address(),
                _ => {
                    return MmError::err(BalanceError::UnexpectedDerivationMethod(
                        UnexpectedDerivationMethod::ExpectedSingleAddress,
                    ))
                },
            };
            let balance = coin
                .client
                .address_balance(my_address)
                .await
                .map_to_mm(|e| BalanceError::Transport(e.to_string()))?;
            Ok(CoinBalance {
                spendable: hastings_to_siacoin(balance.siacoins),
                unspendable: hastings_to_siacoin(balance.immature_siacoins),
            })
        };
        Box::new(fut.boxed().compat())
    }

    fn platform_coin_balance(&self) -> BalanceFut<BigDecimal> {
        Box::new(self.my_balance().map(|res| res.spendable))
    }

    fn platform_ticker(&self) -> &str {
        self.ticker()
    }

    /// Receives raw transaction bytes in hexadecimal format as input and returns tx hash in hexadecimal format
    fn send_raw_tx(&self, tx: &str) -> Box<dyn Future<Item = String, Error = String> + Send> {
        let client = self.client.clone();
        let tx = tx.to_owned();

        let fut = async move {
            let tx: Json = serde_json::from_str(&tx).map_err(|e| e.to_string())?;
            let transaction = serde_json::from_str::<V2Transaction>(&tx.to_string()).map_err(|e| e.to_string())?;
            let txid = transaction.txid().to_string();

            client
                .broadcast_transaction(&transaction)
                .await
                .map_err(|e| e.to_string())?;
            Ok(txid)
        };
        Box::new(fut.boxed().compat())
    }

    fn send_raw_tx_bytes(&self, tx: &[u8]) -> Box<dyn Future<Item = String, Error = String> + Send> {
        let tx: V2Transaction = try_fus!(serde_json::from_slice(tx).map_err(|e| e.to_string()));
        let str_tx = try_fus!(serde_json::to_string(&tx).map_err(|e| e.to_string()));
        self.send_raw_tx(&str_tx)
    }

    #[inline(always)]
    async fn sign_raw_tx(&self, _args: &SignRawTransactionRequest) -> RawTransactionResult {
        unimplemented!()
    }

    // TODO Alright - match the standard convention of Tryfrom<ConfirmPaymentInput> for SiaConfirmPaymentInput
    fn wait_for_confirmations(&self, input: ConfirmPaymentInput) -> Box<dyn Future<Item = (), Error = String> + Send> {
        let tx: SiaTransaction = try_fus!(serde_json::from_slice(&input.payment_tx)
            .map_err(|e| format!("siacoin wait_for_confirmations payment_tx deser failed: {}", e)));
        let txid = tx.txid();
        let client = self.client.clone();
        let tx_request = GetEventRequest { txid: txid.clone() };

        let fut = async move {
            loop {
                if now_sec() > input.wait_until {
                    return ERR!(
                        "Waited too long until {} for payment {} to be received",
                        input.wait_until,
                        tx.txid()
                    );
                }

                match client.dispatcher(tx_request.clone()).await {
                    Ok(event) => {
                        if event.confirmations >= input.confirmations {
                            return Ok(());
                        }
                    },
                    Err(e) => info!("Waiting for confirmation of Sia txid {}: {}", txid, e),
                }

                Timer::sleep(input.check_every as f64).await;
            }
        };

        Box::new(fut.boxed().compat())
    }

    async fn wait_for_htlc_tx_spend(&self, args: WaitForHTLCTxSpendArgs<'_>) -> TransactionResult {
        self.sia_wait_for_htlc_tx_spend(args)
            .await
            .map_err(|e| TransactionErr::Plain(e.to_string()))
    }

    fn tx_enum_from_bytes(&self, bytes: &[u8]) -> Result<TransactionEnum, MmError<TxMarshalingErr>> {
        let tx: V2Transaction =
            serde_json::from_slice(bytes).map_to_mm(|e| TxMarshalingErr::InvalidInput(e.to_string()))?;
        Ok(TransactionEnum::SiaTransaction(SiaTransaction(tx)))
    }

    fn current_block(&self) -> Box<dyn Future<Item = u64, Error = String> + Send> {
        let client = self.client.clone(); // Clone the client

        let height_fut = async move { client.current_height().await.map_err(|e| e.to_string()) }
            .boxed() // Make the future 'static by boxing
            .compat(); // Convert to a futures 0.1-compatible future

        Box::new(height_fut)
    }

    // This remains unimplemented because the response is meaningless to a typical sia user since
    // all Sia software only ever presents or accepts seed phrases, never raw private keys.
    // TODO Alright: provide useful import/export functionality prior to mainnet v2 activation
    fn display_priv_key(&self) -> Result<String, String> {
        Err("SiaCoin::display_priv_key: Unsupported".to_string())
    }

    fn min_tx_amount(&self) -> BigDecimal {
        hastings_to_siacoin(1u64.into())
    }

    fn min_trading_vol(&self) -> MmNumber {
        hastings_to_siacoin(1u64.into()).into()
    }

    fn should_burn_dex_fee(&self) -> bool {
        false
    }

    fn is_trezor(&self) -> bool {
        self.priv_key_policy.is_trezor()
    }
}

// contains various helpers to account for subpar error handling trait method signatures
impl SiaCoin {
    pub fn my_keypair(&self) -> Result<&SiaKeypair, SiaCoinMyKeypairError> {
        match &*self.priv_key_policy {
            PrivKeyPolicy::Iguana(keypair) => Ok(keypair),
            _ => Err(SiaCoinMyKeypairError::PrivKeyPolicy),
        }
    }
}

// contains imeplementations of the SwapOps trait methods with proper error handling
// Some of these methods are extremely verbose and can obviously be refactored to be more consise.
// However, the SwapOps trait is expected to be refactored to use associated types for types such as
// Address, PublicKey, Currency and Error types.
// TODO Alright : refactor error types of SwapOps methods to use associated types
impl SiaCoin {
    /// Create a new transaction to send the taker fee to the fee address
    async fn new_send_taker_fee(
        &self,
        dex_fee: DexFee,
        uuid: &[u8],
        _expire_at: u64,
    ) -> Result<TransactionEnum, SendTakerFeeError> {
        // Check the Uuid provided is valid v4 as we will encode it into the transaction
        let uuid_type_check = Uuid::from_slice(uuid)?;

        match uuid_type_check.get_version_num() {
            4 => (),
            version => return Err(SendTakerFeeError::UuidVersion(version)),
        }

        // Convert the DexFee to a Currency amount
        let trade_fee_amount = match dex_fee {
            DexFee::Standard(mm_num) => siacoin_to_hastings(BigDecimal::from(mm_num))?,
            wrong_variant => return Err(SendTakerFeeError::DexFeeVariant(wrong_variant)),
        };

        let my_keypair = self.my_keypair()?;

        // Create a new transaction builder
        let tx = V2TransactionBuilder::new()
            // FIXME Alright: Calculate the miner fee amount
            .miner_fee(Currency::DEFAULT_FEE)
            // Add the trade fee output
            .add_siacoin_output((FEE_ADDR.clone(), trade_fee_amount).into())
            // Fund the transaction
            .fund_tx_single_source(&self.client, &my_keypair.public())
            .await?
            // Embed swap uuid to provide better validation from maker
            .arbitrary_data(uuid.to_vec().into())
            .add_change_output(&my_keypair.public().address())
            // Sign inputs and finalize the transaction
            .sign_simple(vec![my_keypair])
            .build();

        // Broadcast the transaction
        self.client.broadcast_transaction(&tx).await?;

        Ok(TransactionEnum::SiaTransaction(tx.into()))
    }

    async fn new_send_maker_payment(
        &self,
        args: SendPaymentArgs<'_>,
    ) -> Result<TransactionEnum, SendMakerPaymentError> {
        let my_keypair = self.my_keypair()?;

        let maker_public_key = my_keypair.public();

        // TODO Alright - pubkey padding, see SiaCoin::derive_htlc_pubkey
        if args.other_pubkey.len() != 33 {
            return Err(SendMakerPaymentError::InvalidTakerPublicKeyLength(
                args.other_pubkey.to_vec(),
            ));
        }
        let taker_public_key = PublicKey::from_bytes(&args.other_pubkey[..32])?;

        let secret_hash = Hash256::try_from(args.secret_hash)?;

        // Generate HTLC SpendPolicy
        let htlc_spend_policy =
            SpendPolicy::atomic_swap(&taker_public_key, &maker_public_key, args.time_lock, &secret_hash);

        // Convert the trade amount to a Currency amount
        let trade_amount = siacoin_to_hastings(args.amount)?;

        // Create a new transaction builder
        let tx = V2TransactionBuilder::new()
            // FIXME Alright: Calculate the miner fee amount
            .miner_fee(Currency::DEFAULT_FEE)
            // Add the HTLC output
            .add_siacoin_output((htlc_spend_policy.address(), trade_amount).into())
            // Fund the transaction from my_keypair
            .fund_tx_single_source(&self.client, &my_keypair.public())
            .await?
            .add_change_output(&my_keypair.public().address())
            // Sign inputs
            .sign_simple(vec![my_keypair])
            .build();

        // Broadcast the transaction
        self.client.broadcast_transaction(&tx).await?;

        Ok(TransactionEnum::SiaTransaction(tx.into()))
    }

    async fn new_send_taker_payment(
        &self,
        args: SendPaymentArgs<'_>,
    ) -> Result<TransactionEnum, SendTakerPaymentError> {
        let my_keypair = self.my_keypair()?;

        let taker_public_key = my_keypair.public();

        // TODO Alright - pubkey padding, see SiaCoin::derive_htlc_pubkey
        if args.other_pubkey.len() != 33 {
            return Err(SendTakerPaymentError::InvalidMakerPublicKeyLength(
                args.other_pubkey.to_vec(),
            ));
        }
        let maker_public_key = PublicKey::from_bytes(&args.other_pubkey[..32])?;

        let secret_hash = Hash256::try_from(args.secret_hash)?;

        // Generate HTLC SpendPolicy
        let htlc_spend_policy =
            SpendPolicy::atomic_swap(&maker_public_key, &taker_public_key, args.time_lock, &secret_hash);

        // Convert the trade amount to a Currency amount
        let trade_amount = siacoin_to_hastings(args.amount)?;

        // Create a new transaction builder
        let tx = V2TransactionBuilder::new()
            // Set the miner fee amount
            .miner_fee(Currency::DEFAULT_FEE)
            // Add the HTLC output
            .add_siacoin_output((htlc_spend_policy.address(), trade_amount).into())
            // Fund(add enough inputs) the transaction
            .fund_tx_single_source(&self.client, &my_keypair.public())
            .await?
            .add_change_output(&my_keypair.public().address())
            // Sign inputs and finalize the transaction
            .sign_simple(vec![my_keypair])
            .build();

        // Broadcast the transaction
        self.client.broadcast_transaction(&tx).await?;

        Ok(TransactionEnum::SiaTransaction(tx.into()))
    }

    // TODO Alright - this is logically the same as new_send_taker_spends_maker_payment except
    // maker_public_key, taker_public being swapped. Refactor to reduce code duplication
    async fn new_send_maker_spends_taker_payment(
        &self,
        args: SpendPaymentArgs<'_>,
    ) -> Result<TransactionEnum, MakerSpendsTakerPaymentError> {
        let my_keypair = self.my_keypair()?;

        let maker_public_key = my_keypair.public();

        // TODO Alright - pubkey padding, see SiaCoin::derive_htlc_pubkey
        if args.other_pubkey.len() != 33 {
            return Err(MakerSpendsTakerPaymentError::InvalidTakerPublicKeyLength(
                args.other_pubkey.to_vec(),
            ));
        }
        let taker_public_key = PublicKey::from_bytes(&args.other_pubkey[..32])?;

        let taker_payment_tx = SiaTransaction::try_from(args.other_payment_tx.to_vec())?;
        let taker_payment_txid = taker_payment_tx.txid();

        let secret = Preimage::try_from(args.secret)?;
        let secret_hash = Hash256::try_from(args.secret_hash)?;
        // TODO Alright could do `sha256(secret) == secret_hash`` sanity check here

        // Generate HTLC SpendPolicy as it will appear in the SiacoinInputV2 that spends taker payment
        let input_spend_policy =
            SpendPolicy::atomic_swap_success(&maker_public_key, &taker_public_key, args.time_lock, &secret_hash);

        // Fetch the HTLC UTXO from the taker payment transaction
        let htlc_utxo = self
            .client
            .utxo_from_txid(&taker_payment_txid, 0)
            .await
            .map_err(Box::new)?;

        // FIXME Alright this transaction will have a fixed size, calculate the miner fee amount
        // after we have the actual transaction size
        let miner_fee = Currency::DEFAULT_FEE;
        let htlc_utxo_amount = htlc_utxo.output.siacoin_output.value;

        // Create a new transaction builder
        let tx = V2TransactionBuilder::new()
            // Set the miner fee amount
            .miner_fee(miner_fee)
            // Add output of maker spending to self
            .add_siacoin_output((maker_public_key.address(), htlc_utxo_amount - miner_fee).into())
            // Add input spending the HTLC output
            .add_siacoin_input(htlc_utxo.output, input_spend_policy)
            // Satisfy the HTLC by providing a signature and the secret
            .satisfy_atomic_swap_success(my_keypair, secret, 0u32)?
            .build();

        // Broadcast the transaction
        self.client.broadcast_transaction(&tx).await?;

        Ok(TransactionEnum::SiaTransaction(tx.into()))
    }

    async fn new_send_taker_spends_maker_payment(
        &self,
        args: SpendPaymentArgs<'_>,
    ) -> Result<TransactionEnum, TakerSpendsMakerPaymentError> {
        let my_keypair = self.my_keypair()?;

        let taker_public_key = my_keypair.public();

        // TODO Alright - pubkey padding, see SiaCoin::derive_htlc_pubkey
        if args.other_pubkey.len() != 33 {
            return Err(TakerSpendsMakerPaymentError::InvalidMakerPublicKeyLength(
                args.other_pubkey.to_vec(),
            ));
        };
        let maker_public_key = PublicKey::from_bytes(&args.other_pubkey[..32])?;

        let maker_payment_tx = SiaTransaction::try_from(args.other_payment_tx.to_vec())?;
        let maker_payment_txid = maker_payment_tx.txid();

        let secret = Preimage::try_from(args.secret)?;
        let secret_hash = Hash256::try_from(args.secret_hash)?;
        // TODO Alright could do `sha256(secret) == secret_hash`` sanity check here

        // Generate HTLC SpendPolicy as it will appear in the SiacoinInputV2 that spends taker payment
        let input_spend_policy =
            SpendPolicy::atomic_swap_success(&taker_public_key, &maker_public_key, args.time_lock, &secret_hash);

        // Fetch the HTLC UTXO from the taker payment transaction
        let htlc_utxo = self
            .client
            .utxo_from_txid(&maker_payment_txid, 0)
            .await
            .map_err(Box::new)?;

        let miner_fee = Currency::DEFAULT_FEE;
        let htlc_utxo_amount = htlc_utxo.output.siacoin_output.value;

        // Create a new transaction builder
        let tx = V2TransactionBuilder::new()
            // Set the miner fee amount
            .miner_fee(miner_fee)
            // Add output of taker spending to self
            .add_siacoin_output((taker_public_key.address(), htlc_utxo_amount - miner_fee).into())
            // Add input spending the HTLC output
            .add_siacoin_input_with_basis(htlc_utxo, input_spend_policy)
            // Satisfy the HTLC by providing a signature and the secret
            .satisfy_atomic_swap_success(my_keypair, secret, 0u32)?
            .build();

        // Broadcast the transaction
        self.client.broadcast_transaction(&tx).await?;

        Ok(TransactionEnum::SiaTransaction(tx.into()))
    }

    async fn new_validate_fee(&self, args: ValidateFeeArgs<'_>) -> Result<(), ValidateFeeError> {
        let args = SiaValidateFeeArgs::try_from(args)?;

        // Transaction provided by peer via p2p stack
        let peer_tx = args.fee_tx.0.clone();
        let fee_txid = peer_tx.txid();

        let found_in_block = self.client.get_event(&fee_txid).await;

        // TODO Alright - this creates a significant mess in the Error type and can be simplified
        // with a helper method for fetching transactions from chain or mempool
        let fee_tx = match found_in_block {
            Ok(event) => {
                // check fetched event is V2Transaction
                let tx = match event.data {
                    EventDataWrapper::V2Transaction(tx) => tx,
                    _ => return Err(ValidateFeeError::EventVariant(event)),
                };

                // check tx confirmed at or after min_block_number
                let confirmed_at_height = event.index.height;
                if confirmed_at_height < args.min_block_number {
                    return Err(ValidateFeeError::MininumConfirmedHeight {
                        txid: tx.txid(),
                        min_block_number: args.min_block_number,
                    });
                }
                tx
            },
            Err(e) => {
                // Log the error incase of actual error rather than just not finding the tx
                // TODO Alright - can be simplified, see get_transaction FIXME dev comment
                debug!(
                    "SiaCoin::new_validate_fee: fee_tx not found on chain {}, checking mempool",
                    e
                );
                match self.client.get_unconfirmed_transaction(&fee_txid).await? {
                    Some(tx) => {
                        let current_height = self.client.current_height().await?;
                        // if tx found in mempool, check that it would confirm at or after min_block_number
                        if current_height < args.min_block_number {
                            return Err(ValidateFeeError::MininumMempoolHeight {
                                txid: tx.txid(),
                                min_block_number: args.min_block_number,
                            });
                        }
                        tx
                    },
                    None => return Err(ValidateFeeError::TxNotFound(fee_txid.clone())),
                }
            },
        };

        // check that all inputs originate from taker address
        // This mimicks the behavior of KDF's utxo_standard protocol for consistency.
        // TODO Alright - Logically there seems no reason to enforce this? Why would maker care
        // where the fee comes from?
        if !fee_tx
            .siacoin_inputs
            .into_iter()
            .all(|input| input.satisfied_policy.policy.address() == args.taker_public_key.address())
        {
            return Err(ValidateFeeError::InputsOrigin(fee_txid.clone()));
        }

        // check that fee_tx has 1 or 2 outputs
        match fee_tx.siacoin_outputs.len() {
            1 | 2 => (),
            outputs_length => {
                return Err(ValidateFeeError::VoutLength {
                    txid: fee_txid.clone(),
                    outputs_length,
                })
            },
        }

        // check that output 0 pays the fee address
        if fee_tx.siacoin_outputs[0].address != *FEE_ADDR {
            return Err(ValidateFeeError::InvalidFeeAddress {
                txid: fee_txid.clone(),
                address: fee_tx.siacoin_outputs[0].address.clone(),
            });
        }

        // check that output 0 is the correct amount, trade_fee_amount
        if fee_tx.siacoin_outputs[0].value != args.dex_fee_amount {
            return Err(ValidateFeeError::InvalidFeeAmount {
                txid: fee_txid.clone(),
                expected: args.dex_fee_amount,
                actual: fee_tx.siacoin_outputs[0].value,
            });
        }

        // check that arbitrary_data is the same as the uuid
        let fee_tx_uuid = Uuid::from_slice(&fee_tx.arbitrary_data.0)?;
        if fee_tx_uuid != args.uuid {
            return Err(ValidateFeeError::InvalidUuid {
                txid: fee_txid.clone(),
                expected: args.uuid,
                actual: fee_tx_uuid,
            });
        }

        Ok(())
    }

    async fn send_refund_htlc(&self, args: RefundPaymentArgs<'_>) -> Result<TransactionEnum, SendRefundHltcError> {
        let my_keypair = self.my_keypair()?;
        let refund_public_key = my_keypair.public();

        // parse KDF provided data to Sia specific types
        let sia_args = SiaRefundPaymentArgs::try_from(args)?;

        // Generate HTLC SpendPolicy as it will appear in the SiacoinInputV2
        let input_spend_policy = SpendPolicy::atomic_swap_refund(
            &sia_args.success_public_key,
            &refund_public_key,
            sia_args.time_lock,
            &sia_args.secret_hash,
        );

        // Fetch the HTLC UTXO from the payment_tx transaction
        let htlc_utxo = self
            .client
            .utxo_from_txid(&sia_args.payment_tx.txid(), 0)
            .await
            .map_err(Box::new)?;

        let miner_fee = Currency::DEFAULT_FEE;
        let htlc_utxo_amount = htlc_utxo.output.siacoin_output.value;

        // Create a new transaction builder
        let tx = V2TransactionBuilder::new()
            // Set the miner fee amount
            .miner_fee(miner_fee)
            // Add output of taker spending to self
            .add_siacoin_output((my_keypair.public().address(), htlc_utxo_amount - miner_fee).into())
            // Add input spending the HTLC output
            .add_siacoin_input_with_basis(htlc_utxo, input_spend_policy)
            // Satisfy the HTLC by providing a signature and the secret
            .satisfy_atomic_swap_refund(my_keypair, 0u32)?
            .build();

        // Broadcast the transaction
        self.client.broadcast_transaction(&tx).await?;

        Ok(TransactionEnum::SiaTransaction(tx.into()))
    }

    async fn new_check_if_my_payment_sent(
        &self,
        args: CheckIfMyPaymentSentArgs<'_>,
    ) -> Result<Option<TransactionEnum>, SiaCheckIfMyPaymentSentError> {
        // parse arguments to Sia specific types
        let sia_args = SiaCheckIfMyPaymentSentArgs::try_from(args)?;

        // Get my_keypair.public() to use in HTLC SpendPolicy
        let my_keypair = self.my_keypair()?;
        let refund_public_key = my_keypair.public();

        // Generate HTLC SpendPolicy and corresponding address
        let spend_policy = SpendPolicy::atomic_swap(
            &sia_args.success_public_key,
            &refund_public_key,
            sia_args.time_lock,
            &sia_args.secret_hash,
        );
        let htlc_address = spend_policy.address();

        // Fetch all events for the HTLC address
        let events_result = self.client.get_address_events(htlc_address).await;
        let events = match events_result {
            Ok(events) => events,
            Err(_) => return Ok(None),
        };

        // return Ok(None) if no events found - This indicates the payment has not been sent.
        let event = match events.len() {
            0 => return Ok(None),
            _ => events[0].clone(),
        };

        let tx = match event.data {
            EventDataWrapper::V2Transaction(tx) => tx,
            wrong_variant => return Err(SiaCheckIfMyPaymentSentError::EventVariant(wrong_variant)),
        };

        // TODO Alright - check that vout index is correct, check amount is correct
        // Unclear what the consequence of selecting the wrong transaction might have
        // The current implementation matches the UtxoStandardCoin logic
        Ok(Some(SiaTransaction(tx).into()))
    }

    #[allow(clippy::result_large_err)]
    fn sia_extract_secret(
        &self,
        expected_hash_slice: &[u8],
        spend_tx: &[u8],
    ) -> Result<[u8; 32], SiaCoinSiaExtractSecretError> {
        // Parse arguments to Sia specific types
        let tx = SiaTransaction::try_from(spend_tx)?;
        let expected_hash = Hash256::try_from(expected_hash_slice)?;

        // iterate over all inputs and search for preimage that hashes to expected_hash
        let found_secret =
            tx.0.siacoin_inputs
                .iter()
                // flat_map to iterate over all preimages of all inputs
                .flat_map(|input| input.satisfied_policy.preimages.iter())
                // hash each included preimage and check if secret_hash==sha256(preimage)
                .find(|extracted_secret| {
                    let check_secret_hash = Hash256(sha256(&extracted_secret.0).take());
                    check_secret_hash == expected_hash
                });

        // Map Sia types to SwapOps expected types
        found_secret
            .map(|secret| secret.0)
            .ok_or(SiaCoinSiaExtractSecretError::FailedToExtract { tx, expected_hash })
    }

    /// Determines if the HTLC output can be spent via refund path or if additional time must pass
    async fn sia_can_refund_htlc(&self, locktime: u64) -> Result<CanRefundHtlc, SiaCoinSiaCanRefundHtlcError> {
        let median_timestamp = self.client.get_median_timestamp().await?;

        if locktime < median_timestamp {
            return Ok(CanRefundHtlc::CanRefundNow);
        }
        Ok(CanRefundHtlc::HaveToWait(locktime - median_timestamp))
    }

    async fn sia_wait_for_htlc_tx_spend(
        &self,
        args: WaitForHTLCTxSpendArgs<'_>,
    ) -> Result<TransactionEnum, SiaWaitForHTLCTxSpendError> {
        let sia_args = SiaWaitForHTLCTxSpendArgs::try_from(args)?;

        let htlc_lock_txid = sia_args.tx.txid();
        let output_id = SiacoinOutputId::new(htlc_lock_txid.clone(), HTLC_VOUT_INDEX);
        loop {
            // search the memory pool by txid first
            let found_in_mempool = self
                .client
                .dispatcher(TxpoolTransactionsRequest)
                .await
                .unwrap_or({
                    // log any client error here because we must continue to search for the tx
                    // in the blockchain regardless of the error
                    debug!("SiaCoin::sia_wait_for_htlc_tx_spend: failed to fetch mempool transactions");
                    TxpoolTransactionsResponse::default()
                })
                .v2transactions
                .into_iter()
                .find(|tx| tx.siacoin_inputs.iter().any(|input| input.parent.id == output_id));

            if let Some(tx) = found_in_mempool {
                return Ok(TransactionEnum::SiaTransaction(SiaTransaction(tx)));
            }

            // Search confirmed blocks
            let found_in_block = self.client.find_where_utxo_spent(&output_id).await;

            match found_in_block {
                Ok(Some(tx)) => return Ok(TransactionEnum::SiaTransaction(SiaTransaction(tx))),
                // An Err is expected if the UTXO is not found in the blockchain yet.
                // FIXME Alright - An error may also be thrown if the server has dropped the spent
                // UTXO from its index. Need to analyze when this may happen. The indexer node will
                // generally keep ~24 hours of UTXO history. If we hit this case, we need to blindly
                // attempt to spend the UTXO ourselves.
                Err(e) => debug!(
                    "SiaCoin::sia_wait_for_htlc_tx_spend: find_where_utxo_spent failed, continue searching: {}",
                    e
                ),
                _ => (),
            }

            // Check timeout
            if now_sec() >= sia_args.wait_until {
                return Err(SiaWaitForHTLCTxSpendError::Timeout { txid: htlc_lock_txid });
            }

            // Wait before trying again
            Timer::sleep(sia_args.check_every).await;
        }
    }

    /// Validates that a given transaction has the expected HTLC output at HTLC_VOUT_INDEX
    async fn validate_htlc_payment(&self, input: ValidatePaymentInput) -> Result<(), SiaValidateHtlcPaymentError> {
        let sia_args = SiaValidatePaymentInput::try_from(input)?;

        let my_keypair = self.my_keypair()?;
        let success_public_key = my_keypair.public();
        let refund_public_key = sia_args.other_pub;

        // Generate the expected HTLC address where funds should be locked
        let htlc_address = SpendPolicy::atomic_swap(
            &success_public_key,
            &refund_public_key,
            sia_args.time_lock,
            &sia_args.secret_hash,
        )
        .address();

        // Build the expected HTLC output
        let expected_htlc_output = SiacoinOutput {
            value: sia_args.amount,
            address: htlc_address,
        };

        // Check that the transaction has the expected output at HTLC_VOUT_INDEX
        let htlc_output = match sia_args.payment_tx.0.siacoin_outputs.get(HTLC_VOUT_INDEX as usize) {
            Some(output) => output,
            None => {
                return Err(SiaValidateHtlcPaymentError::InvalidOutputLength {
                    expected: HTLC_VOUT_INDEX + 1,
                    actual: sia_args.payment_tx.0.siacoin_outputs.len() as u32,
                    txid: sia_args.payment_tx.0.txid(),
                })
            },
        };

        if *htlc_output != expected_htlc_output {
            return Err(SiaValidateHtlcPaymentError::InvalidOutput {
                expected: expected_htlc_output,
                actual: htlc_output.clone(),
                txid: sia_args.payment_tx.0.txid(),
            });
        }

        Ok(())
    }

    async fn sia_validate_maker_payment(
        &self,
        input: ValidatePaymentInput,
    ) -> Result<(), SiaValidateMakerPaymentError> {
        Ok(self.validate_htlc_payment(input).await?)
    }

    async fn sia_validate_taker_payment(
        &self,
        input: ValidatePaymentInput,
    ) -> Result<(), SiaValidateTakerPaymentError> {
        Ok(self.validate_htlc_payment(input).await?)
    }
}

/// Sia typed equivalent of coins::ValidatePaymentInput
#[derive(Clone, Debug)]
struct SiaValidatePaymentInput {
    payment_tx: SiaTransaction,
    time_lock: u64,
    other_pub: PublicKey,
    secret_hash: Hash256,
    amount: Currency,
}

impl TryFrom<ValidatePaymentInput> for SiaValidatePaymentInput {
    type Error = SiaValidatePaymentInputError;

    fn try_from(args: ValidatePaymentInput) -> Result<Self, Self::Error> {
        let payment_tx = SiaTransaction::try_from(args.payment_tx.to_vec())?;

        // TODO Alright - pubkey padding, see SiaCoin::derive_htlc_pubkey
        if args.other_pub.len() != 33 {
            return Err(SiaValidatePaymentInputError::InvalidOtherPublicKeyLength(
                args.other_pub,
            ));
        }
        let other_pub = PublicKey::from_bytes(&args.other_pub[..32])?;

        let secret_hash = Hash256::try_from(args.secret_hash.as_slice())?;
        let amount = siacoin_to_hastings(args.amount)?;

        Ok(SiaValidatePaymentInput {
            payment_tx,
            time_lock: args.time_lock,
            other_pub,
            secret_hash,
            amount,
        })
    }
}
/// Sia typed equivalent of coins::RefundPaymentArgs
pub struct SiaRefundPaymentArgs {
    payment_tx: SiaTransaction,
    time_lock: u64,
    success_public_key: PublicKey,
    secret_hash: Hash256,
}

impl TryFrom<RefundPaymentArgs<'_>> for SiaRefundPaymentArgs {
    type Error = SiaRefundPaymentArgsError;

    fn try_from(args: RefundPaymentArgs<'_>) -> Result<Self, Self::Error> {
        let payment_tx = SiaTransaction::try_from(args.payment_tx.to_vec())?;

        let time_lock = args.time_lock;

        // TODO Alright - pubkey padding, see SiaCoin::derive_htlc_pubkey
        if args.other_pubkey.len() != 33 {
            return Err(SiaRefundPaymentArgsError::InvalidOtherPublicKeyLength(
                args.other_pubkey.to_vec(),
            ));
        }
        let success_public_key = PublicKey::from_bytes(&args.other_pubkey[..32])?;

        let secret_hash_slice = match args.tx_type_with_secret_hash {
            SwapTxTypeWithSecretHash::TakerOrMakerPayment { maker_secret_hash } => maker_secret_hash,
            wrong_variant => {
                return Err(SiaRefundPaymentArgsError::SwapTxTypeVariant(format!(
                    "{:?}",
                    wrong_variant
                )));
            },
        };

        let secret_hash = Hash256::try_from(secret_hash_slice)?;

        // TODO Alright - check watcher_reward=false, swap_unique_data and swap_contract_address are valid???
        // currently unclear what swap_unique_data and swap_contract_address are used for(if anything)
        // in the context of Sia

        Ok(SiaRefundPaymentArgs {
            payment_tx,
            time_lock,
            success_public_key,
            secret_hash,
        })
    }
}

//
/// Sia typed equivalent of coins::ValidateFeeArgs
/// fee_addr from ValidateFeeArgs is not relevant to Sia because it is a secp256k1 public key
/// Sia requires a ed25519 public key, so FEE_ADDR is used instead
#[derive(Clone, Debug)]
struct SiaValidateFeeArgs {
    fee_tx: SiaTransaction,
    taker_public_key: PublicKey,
    dex_fee_amount: Currency,
    min_block_number: u64,
    uuid: Uuid,
}

impl TryFrom<ValidateFeeArgs<'_>> for SiaValidateFeeArgs {
    type Error = SiaValidateFeeArgsError;

    fn try_from(args: ValidateFeeArgs<'_>) -> Result<Self, Self::Error> {
        // Extract the fee tx from TransactionEnum
        let fee_tx = match args.fee_tx {
            TransactionEnum::SiaTransaction(tx) => tx.clone(),
            wrong_variant => return Err(SiaValidateFeeArgsError::TxEnumVariant(wrong_variant.clone())),
        };

        // TODO Alright - pubkey padding, see SiaCoin::derive_htlc_pubkey
        if args.expected_sender.len() != 33 {
            return Err(SiaValidateFeeArgsError::InvalidTakerPublicKeyLength(
                args.expected_sender.to_vec(),
            ));
        }

        let expected_sender_public_key = PublicKey::from_bytes(&args.expected_sender[..32])?;

        // Convert the DexFee to a Currency amount
        let dex_fee_amount = match args.dex_fee {
            DexFee::Standard(mm_num) => siacoin_to_hastings(BigDecimal::from(mm_num.clone()))?,
            wrong_variant => return Err(SiaValidateFeeArgsError::DexFeeVariant(wrong_variant.clone())),
        };

        // Check the Uuid provided is valid v4
        let uuid = Uuid::from_slice(args.uuid)?;

        match uuid.get_version_num() {
            4 => (),
            version => return Err(SiaValidateFeeArgsError::UuidVersion(version)),
        }

        Ok(SiaValidateFeeArgs {
            fee_tx,
            taker_public_key: expected_sender_public_key,
            dex_fee_amount,
            min_block_number: args.min_block_number,
            uuid,
        })
    }
}

/// Sia typed equivalent of coins::WaitForHTLCTxSpendArgs
struct SiaWaitForHTLCTxSpendArgs {
    pub tx: SiaTransaction,
    pub wait_until: u64,
    pub check_every: f64,
}

impl TryFrom<WaitForHTLCTxSpendArgs<'_>> for SiaWaitForHTLCTxSpendArgs {
    type Error = SiaWaitForHTLCTxSpendArgsError;

    fn try_from(args: WaitForHTLCTxSpendArgs<'_>) -> Result<Self, Self::Error> {
        // Convert tx_bytes to an owned type to prevent lifetime issues
        let tx = SiaTransaction::try_from(args.tx_bytes.to_owned())?;

        // verify secret_hash is valid, but we don't need it otherwise
        let secret_hash_slice: &[u8] = args.secret_hash;
        let _secret_hash = Hash256::try_from(secret_hash_slice)?;

        Ok(SiaWaitForHTLCTxSpendArgs {
            tx,
            wait_until: args.wait_until,
            check_every: args.check_every,
        })
    }
}

/// Sia typed equivalent of coins::CheckIfMyPaymentSentArgs
/// Does not include irrelevant fields swap_contract_address, swap_unique_data or payment_instructions
struct SiaCheckIfMyPaymentSentArgs {
    time_lock: u64,
    /// The PublicKey that appears in the HTLC SpendPolicy success branch
    /// aka "other_pub" in coins::CheckIfMyPaymentSentArgs
    success_public_key: PublicKey,
    secret_hash: Hash256,
    #[expect(dead_code)]
    search_from_block: u64,
    #[expect(dead_code)]
    amount: Currency,
}

impl TryFrom<CheckIfMyPaymentSentArgs<'_>> for SiaCheckIfMyPaymentSentArgs {
    type Error = SiaCheckIfMyPaymentSentArgsError;

    fn try_from(args: CheckIfMyPaymentSentArgs<'_>) -> Result<Self, Self::Error> {
        let time_lock = args.time_lock;

        // TODO Alright - pubkey padding, see SiaCoin::derive_htlc_pubkey
        if args.other_pub.len() != 33 {
            return Err(SiaCheckIfMyPaymentSentArgsError::InvalidOtherPublicKeyLength(
                args.other_pub.to_vec(),
            ));
        }
        let success_public_key = PublicKey::from_bytes(&args.other_pub[..32])?;
        let secret_hash = Hash256::try_from(args.secret_hash)?;
        let search_from_block = args.search_from_block;
        let amount = siacoin_to_hastings(args.amount.clone())?;

        Ok(SiaCheckIfMyPaymentSentArgs {
            time_lock,
            success_public_key,
            secret_hash,
            search_from_block,
            amount,
        })
    }
}

#[async_trait]
impl SwapOps for SiaCoin {
    /* TODO Alright - refactor SwapOps to use associated types for error handling
    TransactionErr is a very suboptimal structure for error handling, so we route to
    new_send_taker_fee to allow for cleaner code patterns. The error is then converted to a
    TransactionErr::Plain(String) for compatibility with the SwapOps trait
    This may lose verbosity such as the full error chain/trace. */
    async fn send_taker_fee(&self, dex_fee: DexFee, uuid: &[u8], expire_at: u64) -> TransactionResult {
        self.new_send_taker_fee(dex_fee, uuid, expire_at)
            .await
            .map_err(|e| e.to_string().into())
    }

    async fn send_maker_payment(&self, maker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        self.new_send_maker_payment(maker_payment_args)
            .await
            .map_err(|e| e.to_string().into())
    }

    async fn send_taker_payment(&self, taker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        self.new_send_taker_payment(taker_payment_args)
            .await
            .map_err(|e| e.to_string().into())
    }

    async fn send_maker_spends_taker_payment(
        &self,
        maker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        self.new_send_maker_spends_taker_payment(maker_spends_payment_args)
            .await
            .map_err(|e| e.to_string().into())
    }

    async fn send_taker_spends_maker_payment(
        &self,
        taker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        self.new_send_taker_spends_maker_payment(taker_spends_payment_args)
            .await
            .map_err(|e| e.to_string().into())
    }

    async fn send_taker_refunds_payment(&self, taker_refunds_payment_args: RefundPaymentArgs<'_>) -> TransactionResult {
        self.send_refund_htlc(taker_refunds_payment_args)
            .await
            .map_err(|e| SendRefundHltcMakerOrTakerError::Taker(e).to_string().into())
    }

    async fn send_maker_refunds_payment(&self, maker_refunds_payment_args: RefundPaymentArgs<'_>) -> TransactionResult {
        self.send_refund_htlc(maker_refunds_payment_args)
            .await
            .map_err(|e| SendRefundHltcMakerOrTakerError::Maker(e).to_string().into())
    }

    async fn validate_fee(&self, validate_fee_args: ValidateFeeArgs<'_>) -> ValidatePaymentResult<()> {
        self.new_validate_fee(validate_fee_args)
            .await
            .map_err(|e| MmError::new(ValidatePaymentError::InternalError(e.to_string())))
    }

    async fn validate_maker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        self.sia_validate_maker_payment(input)
            .await
            .map_err(|e| MmError::new(ValidatePaymentError::InternalError(e.to_string())))
    }

    async fn validate_taker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        self.sia_validate_taker_payment(input)
            .await
            .map_err(|e| MmError::new(ValidatePaymentError::InternalError(e.to_string())))
    }

    // return Ok(Some(tx)) if a transaction is found
    // return Ok(None) if no transaction is found
    async fn check_if_my_payment_sent(
        &self,
        if_my_payment_sent_args: CheckIfMyPaymentSentArgs<'_>,
    ) -> Result<Option<TransactionEnum>, String> {
        self.new_check_if_my_payment_sent(if_my_payment_sent_args)
            .await
            .map_err(|e| e.to_string())
    }

    async fn search_for_swap_tx_spend_my(
        &self,
        _: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        unimplemented!()
    }

    async fn search_for_swap_tx_spend_other(
        &self,
        _: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        unimplemented!()
    }

    async fn extract_secret(&self, secret_hash: &[u8], spend_tx: &[u8]) -> Result<[u8; 32], String> {
        self.sia_extract_secret(secret_hash, spend_tx)
            .map_err(|e| e.to_string())
    }

    fn negotiate_swap_contract_addr(
        &self,
        _other_side_address: Option<&[u8]>,
    ) -> Result<Option<BytesJson>, MmError<NegotiateSwapContractAddrErr>> {
        Ok(None)
    }

    // Todo: This is only used for watchers so it's ok to use a default implementation as watchers are not supported for SIA yet
    fn derive_htlc_key_pair(&self, _swap_unique_data: &[u8]) -> KeyPair {
        KeyPair::default()
    }

    /// Return the iguana ed25519 public key
    /// This is the public key that will be used inside the HTLC SpendPolicy
    // TODO Alright - MakerSwapData is badly designed and assumes this is a 33 byte array aka H264
    // we pad it then drop the last byte when we use it for now
    fn derive_htlc_pubkey(&self, _swap_unique_data: &[u8]) -> [u8; 33] {
        let my_keypair = self
            .my_keypair()
            .expect("SiaCoin::derive_htlc_pubkey: failed to get my_keypair");

        let mut pubkey_bytes_padded = [0u8; 33];
        let pubkey_bytes = my_keypair.public().to_bytes();
        pubkey_bytes_padded[..32].copy_from_slice(&pubkey_bytes);
        pubkey_bytes_padded
    }

    /// Determines "Whether the refund transaction can be sent now"
    /// /api/consensus/tipstate provides 11 timestamps, take the median
    /// medianTimestamp = prevTimestamps[5]
    /// SpendPolicy::After(time) evaluates to true when `time > medianTimestamp`
    async fn can_refund_htlc(&self, locktime: u64) -> Result<CanRefundHtlc, String> {
        self.sia_can_refund_htlc(locktime).await.map_err(|e| e.to_string())
    }

    /// Validate the PublicKey the other party provided
    /// The other party generates this PublicKey via SwapOps::derive_htlc_pubkey
    fn validate_other_pubkey(&self, raw_pubkey: &[u8]) -> MmResult<(), ValidateOtherPubKeyErr> {
        // TODO Alright - pubkey padding, see SiaCoin::derive_htlc_pubkey
        if raw_pubkey.len() != 33 {
            return Err(ValidateOtherPubKeyErr::InvalidPubKey(format!(
                "SiaCoin::validate_other_pubkey: invalid raw_pubkey, expected 33 bytes found: {:?}",
                raw_pubkey.to_vec()
            ))
            .into());
        }
        let _public_key = PublicKey::from_bytes(&raw_pubkey[..32]).map_err(|e| {
            ValidateOtherPubKeyErr::InvalidPubKey(format!(
                "SiaCoin::validate_other_pubkey: validate pubkey:{:?} failed: {}",
                raw_pubkey, e
            ))
        })?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, From, Into)]
#[serde(transparent)]
pub struct SiaTransaction(pub V2Transaction);

impl fmt::Display for SiaTransaction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match serde_json::to_string(self) {
            Ok(json) => write!(f, "{}", json),
            Err(err) => write!(f, "Failed to serialize SiaTransaction:{:?} to JSON: {}", self, err),
        }
    }
}

impl SiaTransaction {
    pub fn txid(&self) -> Hash256 {
        self.0.txid()
    }
}

impl TryFrom<SiaTransaction> for Vec<u8> {
    type Error = SiaTransactionError;

    fn try_from(tx: SiaTransaction) -> Result<Self, Self::Error> {
        serde_json::ser::to_vec(&tx).map_err(SiaTransactionError::ToVec)
    }
}

impl TryFrom<&[u8]> for SiaTransaction {
    type Error = SiaTransactionError;

    fn try_from(tx_slice: &[u8]) -> Result<Self, Self::Error> {
        serde_json::de::from_slice(tx_slice).map_err(SiaTransactionError::FromVec)
    }
}

impl TryFrom<Vec<u8>> for SiaTransaction {
    type Error = SiaTransactionError;

    fn try_from(tx: Vec<u8>) -> Result<Self, Self::Error> {
        serde_json::de::from_slice(&tx).map_err(SiaTransactionError::FromVec)
    }
}

impl Transaction for SiaTransaction {
    // serde should always be succesful but write an empty vec just in case.
    fn tx_hex(&self) -> Vec<u8> {
        serde_json::ser::to_vec(self).unwrap_or_default()
    }

    fn tx_hash_as_bytes(&self) -> BytesJson {
        BytesJson(self.txid().0.to_vec())
    }
}

/// Represents the different types of transactions that can be sent to a wallet.
/// This enum is generally only useful for displaying wallet history.
/// We do not support any operations for any type other than V2Transaction, but we want the ability
/// to display other event types within the wallet history.
/// Use SiaTransaction type instead.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(untagged)]
pub enum SiaTransactionTypes {
    V1Transaction(V1Transaction),
    V2Transaction(V2Transaction),
    EventPayout(EventPayout),
}

impl SiaCoin {
    pub async fn request_events_history(&self) -> Result<Vec<Event>, MmError<String>> {
        let my_address = match &*self.priv_key_policy {
            PrivKeyPolicy::Iguana(key_pair) => key_pair.public().address(),
            _ => {
                return MmError::err(ERRL!("Unexpected derivation method. Expected single address."));
            },
        };

        let address_events = self
            .client
            .get_address_events(my_address)
            .await
            .map_err(|e| e.to_string())?;

        Ok(address_events)
    }

    // TODO this was written prior to Currency arithmetic traits being added; refactor to use those
    fn tx_details_from_event(&self, event: &Event) -> Result<TransactionDetails, MmError<String>> {
        match &event.data {
            EventDataWrapper::V2Transaction(tx) => {
                let txid = tx.txid().to_string();
                let internal_id = hex::decode(&txid).map_to_mm(|e| e.to_string())?.into();

                let from: Vec<String> = tx
                    .siacoin_inputs
                    .iter()
                    .map(|input| input.parent.siacoin_output.address.to_string())
                    .collect();

                let to: Vec<String> = tx
                    .siacoin_outputs
                    .iter()
                    .map(|output| output.address.to_string())
                    .collect();

                let total_input: u128 = tx
                    .siacoin_inputs
                    .iter()
                    .map(|input| *input.parent.siacoin_output.value)
                    .sum();

                let total_output: u128 = tx.siacoin_outputs.iter().map(|output| *output.value).sum();

                let fee = total_input - total_output;

                let my_address = self.my_address().mm_err(|e| e.to_string())?;

                let spent_by_me: u128 = tx
                    .siacoin_inputs
                    .iter()
                    .filter(|input| input.parent.siacoin_output.address.to_string() == my_address)
                    .map(|input| *input.parent.siacoin_output.value)
                    .sum();

                let received_by_me: u128 = tx
                    .siacoin_outputs
                    .iter()
                    .filter(|output| output.address.to_string() == my_address)
                    .map(|output| *output.value)
                    .sum();

                let my_balance_change = hastings_to_siacoin(received_by_me.into()) - hastings_to_siacoin(spent_by_me.into());

                Ok(TransactionDetails {
                    tx: TransactionData::Sia {
                        tx_json: SiaTransactionTypes::V2Transaction(tx.clone()),
                        tx_hash: txid,
                    },
                    from,
                    to,
                    total_amount: hastings_to_siacoin(total_input.into()),
                    spent_by_me: hastings_to_siacoin(spent_by_me.into()),
                    received_by_me: hastings_to_siacoin(received_by_me.into()),
                    my_balance_change,
                    block_height: event.index.height,
                    timestamp: event.timestamp.timestamp() as u64,
                    fee_details: Some(
                        SiaFeeDetails {
                            coin: self.ticker().to_string(),
                            policy: SiaFeePolicy::Unknown,
                            total_amount: hastings_to_siacoin(fee.into()),
                        }
                        .into(),
                    ),
                    coin: self.ticker().to_string(),
                    internal_id,
                    kmd_rewards: None,
                    transaction_type: TransactionType::SiaV2Transaction,
                    memo: None,
                })
            },
            EventDataWrapper::V1Transaction(tx) => {
                let txid = tx.transaction.txid().to_string();
                let internal_id = hex::decode(&txid).map_to_mm(|e| e.to_string())?.into();

                let from: Vec<String> = tx
                    .spent_siacoin_elements
                    .iter()
                    .map(|element| element.siacoin_output.address.to_string())
                    .collect();

                let to: Vec<String> = tx
                    .transaction
                    .siacoin_outputs
                    .iter()
                    .map(|output| output.address.to_string())
                    .collect();

                let total_input: u128 = tx
                    .spent_siacoin_elements
                    .iter()
                    .map(|element| *element.siacoin_output.value)
                    .sum();

                let total_output: u128 = tx.transaction.siacoin_outputs.iter().map(|output| *output.value).sum();

                // This accounts for v1 coinbase transactions where this is expected to underflow
                let fee = match total_input.checked_sub(total_output) {
                    Some(value) => value,
                    None => {
                        // this should be a rare case, but logging will help if we somehow hit it unexpectedly
                        debug!(
                            "SiaCoin::tx_details_from_event: fee underflow: total_input < total_output for event: {:?}", tx
                        );
                        0
                    }
                };

                let my_address = self.my_address().mm_err(|e| e.to_string())?;

                let spent_by_me: u128 = tx
                    .spent_siacoin_elements
                    .iter()
                    .filter(|element| element.siacoin_output.address.to_string() == my_address)
                    .map(|element| *element.siacoin_output.value)
                    .sum();

                let received_by_me: u128 = tx
                    .transaction
                    .siacoin_outputs
                    .iter()
                    .filter(|output| output.address.to_string() == my_address)
                    .map(|output| *output.value)
                    .sum();

                let my_balance_change = hastings_to_siacoin(received_by_me.into()) - hastings_to_siacoin(spent_by_me.into());

                Ok(TransactionDetails {
                    tx: TransactionData::Sia {
                        tx_json: SiaTransactionTypes::V1Transaction(tx.transaction.clone()),
                        tx_hash: txid,
                    },
                    from,
                    to,
                    total_amount: hastings_to_siacoin(total_input.into()),
                    spent_by_me: hastings_to_siacoin(spent_by_me.into()),
                    received_by_me: hastings_to_siacoin(received_by_me.into()),
                    my_balance_change,
                    block_height: event.index.height,
                    timestamp: event.timestamp.timestamp() as u64,
                    fee_details: Some(
                        SiaFeeDetails {
                            coin: self.ticker().to_string(),
                            policy: SiaFeePolicy::Unknown,
                            total_amount: hastings_to_siacoin(fee.into()),
                        }
                        .into(),
                    ),
                    coin: self.ticker().to_string(),
                    internal_id,
                    kmd_rewards: None,
                    transaction_type: TransactionType::SiaV1Transaction,
                    memo: None,
                })
            },
            EventDataWrapper::MinerPayout(event_payout) | EventDataWrapper::FoundationPayout(event_payout) => {
                let txid = event_payout.siacoin_element.id.to_string();
                let internal_id = hex::decode(&txid).map_to_mm(|e| e.to_string())?.into();

                let from: Vec<String> = vec![];

                let to: Vec<String> = vec![event_payout.siacoin_element.siacoin_output.address.to_string()];

                let total_output: u128 = event_payout.siacoin_element.siacoin_output.value.0;

                let my_address = self.my_address().mm_err(|e| e.to_string())?;

                let received_by_me: u128 =
                    if event_payout.siacoin_element.siacoin_output.address.to_string() == my_address {
                        total_output
                    } else {
                        0
                    };

                let my_balance_change = hastings_to_siacoin(received_by_me.into());

                Ok(TransactionDetails {
                    tx: TransactionData::Sia {
                        tx_json: SiaTransactionTypes::EventPayout(event_payout.clone()),
                        tx_hash: txid,
                    },
                    from,
                    to,
                    total_amount: hastings_to_siacoin(total_output.into()),
                    spent_by_me: BigDecimal::from(0),
                    received_by_me: hastings_to_siacoin(received_by_me.into()),
                    my_balance_change,
                    block_height: event.index.height,
                    timestamp: event.timestamp.timestamp() as u64,
                    fee_details: None,
                    coin: self.ticker().to_string(),
                    internal_id,
                    kmd_rewards: None,
                    transaction_type: TransactionType::SiaMinerPayout,
                    memo: None,
                })
            },
            EventDataWrapper::ClaimPayout(_) // TODO this can be moved to the above case with Miner and Foundation payouts
            | EventDataWrapper::V2FileContractResolution(_)
            | EventDataWrapper::EventV1ContractResolution(_) => MmError::err(ERRL!("Unsupported event type")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm2_number::BigDecimal;
    use std::str::FromStr;

    fn valid_transaction() -> SiaTransaction {
        let j = json!(
            {"siacoinInputs":[{"parent":{"id":"0f088eddda5320f8453a55349063abe43ba5b282631d5d2b9e684548f083055a","stateElement":{"leafIndex":3,"merkleProof":["ff9ce7f558df52b35d40fda59a8ab6d5ffb3dfab029992d02d1e929e0a36b6eb","b37a5387883748f73c1475ca85c8f3200eef09126c44824d0f44574109dabedc","0f1d4ef5b1bf0e6eb45240e717a1d548326f8379878944c6536fc73989cf2e7a","f85d8f6578bc2db41e8a206a060a10386c18505b533f64a5ceff574d602b57bd","c21c0b980cd4184996558b49e8b901c70cadb17fd27c62f472a2707f0eb6b092","482e9402c0c5e43d599b9683ba25224f74499f89e9bc33249794ff5e9f55e337","a29aabab81d0cf20e1bd33bb3e1138f1628a1337eb0430e4de47cf695eb25897","aeb60668a7e0ee0232f81642626104a1d4eb9ce3d0d9cf81196de48112e3ea41"]},"siacoinOutput":{"value":"299999000000000000000000000000","address":"c34caa97740668de2bbdb7174572ed64c861342bf27e80313cbfa02e9251f52e30aad3892533"},"maturityHeight":11},"satisfiedPolicy":{"policy":{"type":"pk","policy":"ed25519:a729be53dae7b0ed812f2a123ce93556014bbad8516ba6b1b496a112b46bbd97"},"signatures":["160e79ac52e0eaab5e92bd1675604a94b56ec58fdd0be3f3a842a4ece07d794f7ee1e8cc8f29b596bf71b2dc594df53347b9a4bcbec46fe09244ce6d3f6a6708"]}}],"siacoinOutputs":[{"value":"50000000000000000000000","address":"71731d7efe821794742c72a8376f56355b3c8a1984b861ccd42eed77a779a26626ea26ced3e2"},{"value":"299998949990000000000000000000","address":"c34caa97740668de2bbdb7174572ed64c861342bf27e80313cbfa02e9251f52e30aad3892533"}],"minerFee":"10000000000000000000"}
        );
        let tx = serde_json::from_value::<V2Transaction>(j).unwrap();
        SiaTransaction(tx)
    }

    #[test]
    fn test_siacoin_from_hastings_u128_max() {
        let hastings = u128::MAX;
        let siacoin = hastings_to_siacoin(hastings.into());
        assert_eq!(
            siacoin,
            BigDecimal::from_str("340282366920938.463463374607431768211455").unwrap()
        );
    }

    #[test]
    fn test_siacoin_from_hastings_total_supply() {
        // Total supply of Siacoin
        let hastings = 57769875000000000000000000000000000u128;
        let siacoin = hastings_to_siacoin(hastings.into());
        assert_eq!(siacoin, BigDecimal::from_str("57769875000").unwrap());
    }

    #[test]
    fn test_siacoin_to_hastings_supply() {
        // Total supply of Siacoin
        let siacoin = BigDecimal::from_str("57769875000").unwrap();
        let hastings = siacoin_to_hastings(siacoin).unwrap();
        assert_eq!(hastings, Currency(57769875000000000000000000000000000));
    }

    #[test]
    fn test_sia_transaction_serde_roundtrip() {
        let tx = valid_transaction();

        let vec = serde_json::ser::to_vec(&tx).unwrap();
        let tx2: SiaTransaction = serde_json::from_slice(&vec).unwrap();

        assert_eq!(tx, tx2);
    }

    /// Test the .expect()s used during lazy_static initialization of FEE_PUBLIC_KEY
    #[test]
    fn test_sia_fee_pubkey_init() {
        let pubkey_bytes: Vec<u8> = hex::decode(DEX_FEE_PUBKEY_ED25519).unwrap();
        let pubkey = PublicKey::from_bytes(&FEE_PUBLIC_KEY_BYTES).unwrap();
        assert_eq!(pubkey_bytes, *FEE_PUBLIC_KEY_BYTES);
        assert_eq!(pubkey, *FEE_PUBLIC_KEY);
    }

    #[test]
    fn test_siacoin_from_hastings_coin() {
        let coin = hastings_to_siacoin(Currency::COIN);
        assert_eq!(coin, BigDecimal::from(1));
    }

    #[test]
    fn test_siacoin_from_hastings_zero() {
        let coin = hastings_to_siacoin(Currency::ZERO);
        assert_eq!(coin, BigDecimal::from(0));
    }

    #[test]
    fn test_siacoin_to_hastings_coin() {
        let coin = BigDecimal::from(1);
        let hastings = siacoin_to_hastings(coin).unwrap();
        assert_eq!(hastings, Currency::COIN);
    }

    #[test]
    fn test_siacoin_to_hastings_zero() {
        let coin = BigDecimal::from(0);
        let hastings = siacoin_to_hastings(coin).unwrap();
        assert_eq!(hastings, Currency::ZERO);
    }

    #[test]
    fn test_siacoin_to_hastings_one() {
        let coin = serde_json::from_str::<BigDecimal>("0.000000000000000000000001").unwrap();
        println!("coin {:?}", coin);
        let hastings = siacoin_to_hastings(coin).unwrap();
        assert_eq!(hastings, Currency(1));
    }
}

#[cfg(all(test, target_arch = "wasm32"))]
mod wasm_tests {
    use super::*;
    use common::log::wasm_log::register_wasm_log;
    use sia_rust::transport::client::{ApiClient, ApiClientHelpers};
    use wasm_bindgen_test::*;

    use url::Url;

    wasm_bindgen_test_configure!(run_in_browser);

    async fn init_client() -> SiaClient {
        let conf = SiaClientConf {
            server_url: Url::parse("https://api.siascan.com/wallet/api").unwrap(),
            headers: HashMap::new(),
        };
        SiaClient::new(conf).await.unwrap()
    }

    #[wasm_bindgen_test]
    async fn test_endpoint_txpool_broadcast() {
        register_wasm_log();

        let client = init_client().await;

        let tx = serde_json::from_str::<V2Transaction>(
            r#"
            {
                "siacoinInputs": [
                    {
                        "parent": {
                            "id": "h:27248ab562cbbee260e07ccae87c74aae71c9358d7f91eee25837e2011ce36d3",
                            "leafIndex": 21867,
                            "merkleProof": [
                                "h:ac2fdcbed40f103e54b0b1a37c20a865f6f1f765950bc6ac358ff3a0e769da50",
                                "h:b25570eb5c106619d4eef5ad62482023df7a1c7461e9559248cb82659ebab069",
                                "h:baa78ec23a169d4e9d7f801e5cf25926bf8c29e939e0e94ba065b43941eb0af8",
                                "h:239857343f2997462bed6c253806cf578d252dbbfd5b662c203e5f75d897886d",
                                "h:ad727ef2112dc738a72644703177f730c634a0a00e0b405bd240b0da6cdfbc1c",
                                "h:4cfe0579eabafa25e98d83c3b5d07ae3835ce3ea176072064ea2b3be689e99aa",
                                "h:736af73aa1338f3bc28d1d8d3cf4f4d0393f15c3b005670f762709b6231951fc"
                            ],
                            "siacoinOutput": {
                                "value": "772999980000000000000000000",
                                "address": "addr:1599ea80d9af168ce823e58448fad305eac2faf260f7f0b56481c5ef18f0961057bf17030fb3"
                            },
                            "maturityHeight": 0
                        },
                        "satisfiedPolicy": {
                            "policy": {
                                "type": "pk",
                                "policy": "ed25519:968e286ef5df3954b7189c53a0b4b3d827664357ebc85d590299b199af46abad"
                            },
                            "signatures": [
                                "sig:7a2c332fef3958a0486ef5e55b70d2a68514ff46d9307a85c3c0e40b76a19eebf4371ab3dd38a668cefe94dbedff2c50cc67856fbf42dce2194b380e536c1500"
                            ]
                        }
                    }
                ],
                "siacoinOutputs": [
                    {
                        "value": "2000000000000000000000000",
                        "address": "addr:1d9a926b1e14b54242375c7899a60de883c8cad0a45a49a7ca2fdb6eb52f0f01dfe678918204"
                    },
                    {
                        "value": "770999970000000000000000000",
                        "address": "addr:1599ea80d9af168ce823e58448fad305eac2faf260f7f0b56481c5ef18f0961057bf17030fb3"
                    }
                ],
                "minerFee": "10000000000000000000"
            }
            "#).unwrap();

        // Use the helper which handles getting the basis (chain tip) automatically
        client.broadcast_transaction(&tx).await.unwrap();
    }

    #[wasm_bindgen_test]
    async fn test_helper_address_balance() {
        register_wasm_log();

        let client = init_client().await;

        client
            .address_balance(
                Address::from_str("addr:1599ea80d9af168ce823e58448fad305eac2faf260f7f0b56481c5ef18f0961057bf17030fb3")
                    .unwrap(),
            )
            .await
            .unwrap();
    }
}
