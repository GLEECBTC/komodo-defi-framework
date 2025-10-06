#![allow(dead_code, unused_variables)]

//! The helper structs used in testing of RPC responses, these should be separated from actual MM2 code to ensure
//! backwards compatibility
//! Use `#[serde(deny_unknown_fields)]` for all structs for tests to fail in case of adding new fields to the response

use http::{HeaderMap, StatusCode};
use mm2_number::{BigDecimal, BigRational, Fraction, MmNumber};
use mm2_rpc::data::legacy::{MatchBy, OrderConfirmationsSettings, OrderType, RpcOrderbookEntry, TakerAction};
use rpc::v1::types::H256 as H256Json;
use serde::de::DeserializeOwned;
use serde_json::Value as Json;
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::num::NonZeroUsize;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcSuccessResponse<T> {
    pub mmrpc: String,
    pub result: T,
    pub id: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcErrorResponse<E> {
    pub mmrpc: String,
    /// The legacy error description
    pub error: String,
    pub error_path: String,
    pub error_trace: String,
    pub error_type: String,
    pub error_data: Option<E>,
    pub id: Option<usize>,
}

/// This RPC response helper is useful in those tests that handle `Success` and `Error` both responses.
#[derive(Debug)]
pub struct RpcResponse {
    rpc_name: String,
    status_code: StatusCode,
    payload: String,
}

impl RpcResponse {
    pub fn new(rpc_name: &str, rc: (StatusCode, String, HeaderMap)) -> RpcResponse {
        RpcResponse {
            rpc_name: rpc_name.to_string(),
            status_code: rc.0,
            payload: rc.1,
        }
    }

    #[track_caller]
    pub fn unwrap<T>(self) -> T
    where
        T: fmt::Debug + DeserializeOwned,
    {
        assert!(self.status_code.is_success(), "!{}: {}", self.rpc_name, self.payload);
        let res: RpcSuccessResponse<T> = serde_json::from_str(&self.payload).unwrap();
        res.result
    }

    #[track_caller]
    pub fn unwrap_err<E>(self) -> RpcErrorResponse<E>
    where
        E: fmt::Debug + DeserializeOwned,
    {
        assert!(
            !self.status_code.is_success(),
            "'{}' should have failed, but got: {}",
            self.rpc_name,
            self.payload
        );
        serde_json::from_str(&self.payload).unwrap()
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoricalOrder {
    max_base_vol: Option<MmNumber>,
    min_base_vol: Option<MmNumber>,
    price: Option<MmNumber>,
    updated_at: Option<u64>,
    conf_settings: Option<OrderConfirmationsSettings>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuyOrSellRpcRes {
    pub base: String,
    pub rel: String,
    pub base_amount: BigDecimal,
    pub base_amount_rat: BigRational,
    pub rel_amount: BigDecimal,
    pub rel_amount_rat: BigRational,
    pub min_volume: BigDecimal,
    pub min_volume_rat: BigRational,
    pub min_volume_fraction: Fraction,
    pub action: TakerAction,
    pub uuid: Uuid,
    pub method: String,
    pub sender_pubkey: H256Json,
    pub dest_pub_key: H256Json,
    pub match_by: MatchBy,
    pub conf_settings: OrderConfirmationsSettings,
    pub order_type: OrderType,
    pub base_orderbook_ticker: Option<String>,
    pub rel_orderbook_ticker: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuyOrSellRpcResult {
    pub result: BuyOrSellRpcRes,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakerRequest {
    base: String,
    rel: String,
    base_amount: BigDecimal,
    base_amount_rat: BigRational,
    rel_amount: BigDecimal,
    rel_amount_rat: BigRational,
    action: TakerAction,
    uuid: Uuid,
    method: String,
    sender_pubkey: H256Json,
    dest_pub_key: H256Json,
    match_by: MatchBy,
    conf_settings: OrderConfirmationsSettings,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakerReserved {
    base: String,
    rel: String,
    base_amount: BigDecimal,
    base_amount_rat: BigRational,
    rel_amount: BigDecimal,
    rel_amount_rat: BigRational,
    taker_order_uuid: Uuid,
    maker_order_uuid: Uuid,
    method: String,
    sender_pubkey: H256Json,
    dest_pub_key: H256Json,
    conf_settings: OrderConfirmationsSettings,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakerConnect {
    taker_order_uuid: Uuid,
    maker_order_uuid: Uuid,
    method: String,
    sender_pubkey: H256Json,
    dest_pub_key: H256Json,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakerConnected {
    taker_order_uuid: Uuid,
    maker_order_uuid: Uuid,
    method: String,
    sender_pubkey: H256Json,
    dest_pub_key: H256Json,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakerMatch {
    request: TakerRequest,
    reserved: MakerReserved,
    connect: Option<TakerConnect>,
    connected: Option<MakerConnected>,
    last_updated: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MakerOrderRpcResult {
    pub max_base_vol: BigDecimal,
    pub max_base_vol_rat: BigRational,
    pub min_base_vol: BigDecimal,
    pub min_base_vol_rat: BigRational,
    pub price: BigDecimal,
    pub price_rat: BigRational,
    pub created_at: u64,
    pub updated_at: Option<u64>,
    pub base: String,
    pub rel: String,
    pub matches: HashMap<Uuid, MakerMatch>,
    pub started_swaps: Vec<Uuid>,
    pub uuid: Uuid,
    pub conf_settings: Option<OrderConfirmationsSettings>,
    changes_history: Option<Vec<HistoricalOrder>>,
    pub cancellable: bool,
    pub available_amount: BigDecimal,
    pub base_orderbook_ticker: Option<String>,
    pub rel_orderbook_ticker: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetPriceResult {
    pub max_base_vol: BigDecimal,
    pub max_base_vol_rat: BigRational,
    pub min_base_vol: BigDecimal,
    pub min_base_vol_rat: BigRational,
    pub price: BigDecimal,
    pub price_rat: BigRational,
    pub created_at: u64,
    pub updated_at: Option<u64>,
    pub base: String,
    pub rel: String,
    pub matches: HashMap<Uuid, MakerMatch>,
    pub started_swaps: Vec<Uuid>,
    pub uuid: Uuid,
    pub conf_settings: Option<OrderConfirmationsSettings>,
    pub base_orderbook_ticker: Option<String>,
    pub rel_orderbook_ticker: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetPriceResponse {
    pub result: SetPriceResult,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakerMatch {
    reserved: MakerReserved,
    connect: TakerConnect,
    connected: Option<MakerConnected>,
    last_updated: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TakerOrderRpcResult {
    created_at: u64,
    request: TakerRequest,
    matches: HashMap<Uuid, TakerMatch>,
    order_type: OrderType,
    pub cancellable: bool,
    pub base_orderbook_ticker: Option<String>,
    pub rel_orderbook_ticker: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MyOrdersRpc {
    pub maker_orders: HashMap<Uuid, MakerOrderRpcResult>,
    pub taker_orders: HashMap<Uuid, TakerOrderRpcResult>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MyOrdersRpcResult {
    pub result: MyOrdersRpc,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BestOrdersResponse {
    pub result: HashMap<String, Vec<RpcOrderbookEntry>>,
    pub original_tickers: HashMap<String, HashSet<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairDepth {
    pub asks: usize,
    pub bids: usize,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PairWithDepth {
    pub pair: (String, String),
    pub depth: PairDepth,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderbookDepthResponse {
    pub result: Vec<PairWithDepth>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TradeFeeForTest {
    pub coin: String,
    pub amount: BigDecimal,
    pub amount_rat: BigRational,
    pub amount_fraction: Fraction,
    pub paid_from_trading_vol: bool,
}

impl TradeFeeForTest {
    pub fn new(coin: &str, amount: &'static str, paid_from_trading_vol: bool) -> TradeFeeForTest {
        let amount_mm = MmNumber::from(amount);
        TradeFeeForTest {
            coin: coin.into(),
            amount: amount_mm.to_decimal(),
            amount_rat: amount_mm.to_ratio(),
            amount_fraction: amount_mm.to_fraction(),
            paid_from_trading_vol,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TotalTradeFeeForTest {
    pub coin: String,
    pub amount: BigDecimal,
    pub amount_rat: BigRational,
    pub amount_fraction: Fraction,
    pub required_balance: BigDecimal,
    pub required_balance_rat: BigRational,
    pub required_balance_fraction: Fraction,
}

impl TotalTradeFeeForTest {
    pub fn new(coin: &str, amount: &'static str, required_balance: &'static str) -> TotalTradeFeeForTest {
        let amount_mm = MmNumber::from(amount);
        let required_mm = MmNumber::from(required_balance);
        TotalTradeFeeForTest {
            coin: coin.into(),
            amount: amount_mm.to_decimal(),
            amount_rat: amount_mm.to_ratio(),
            amount_fraction: amount_mm.to_fraction(),
            required_balance: required_mm.to_decimal(),
            required_balance_rat: required_mm.to_ratio(),
            required_balance_fraction: required_mm.to_fraction(),
        }
    }
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TakerPreimage {
    pub base_coin_fee: TradeFeeForTest,
    pub rel_coin_fee: TradeFeeForTest,
    pub taker_fee: TradeFeeForTest,
    pub fee_to_send_taker_fee: TradeFeeForTest,
    // the order of fees is not deterministic
    pub total_fees: Vec<TotalTradeFeeForTest>,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MakerPreimage {
    pub base_coin_fee: TradeFeeForTest,
    pub rel_coin_fee: TradeFeeForTest,
    pub volume: Option<BigDecimal>,
    pub volume_rat: Option<BigRational>,
    pub volume_fraction: Option<Fraction>,
    // the order of fees is not deterministic
    pub total_fees: Vec<TotalTradeFeeForTest>,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
#[allow(clippy::large_enum_variant)]
pub enum TradePreimageResult {
    TakerPreimage(TakerPreimage),
    MakerPreimage(MakerPreimage),
}

impl TradePreimageResult {
    pub fn sort_total_fees(&mut self) {
        match self {
            TradePreimageResult::MakerPreimage(ref mut preimage) => {
                preimage.total_fees.sort_by(|fee1, fee2| fee1.coin.cmp(&fee2.coin))
            },
            TradePreimageResult::TakerPreimage(ref mut preimage) => {
                preimage.total_fees.sort_by(|fee1, fee2| fee1.coin.cmp(&fee2.coin))
            },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TradePreimageResponse {
    pub result: TradePreimageResult,
}

impl TradePreimageResponse {
    pub fn sort_total_fees(&mut self) { self.result.sort_total_fees() }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaxTakerVolResponse {
    pub result: Fraction,
    pub coin: String,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MaxMakerVolResponse {
    pub coin: String,
    pub volume: MmNumberMultiRepr,
    pub balance: MmNumberMultiRepr,
    pub locked_by_swaps: MmNumberMultiRepr,
}

pub mod max_maker_vol_error {
    use mm2_number::BigDecimal;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct NotSufficientBalance {
        pub coin: String,
        pub available: BigDecimal,
        pub required: BigDecimal,
        pub locked_by_swaps: Option<BigDecimal>,
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawTransactionResult {
    /// Raw bytes of signed transaction in hexadecimal string, this should be sent as is to send_raw_transaction RPC to broadcast the transaction
    pub tx_hex: String,
}

pub mod raw_transaction_error {
    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct InvalidCoin {
        pub coin: String,
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum CustomTendermintMsgType {
    SendHtlcAmount,
    ClaimHtlcAmount,
    SignClaimHtlc,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum TransactionType {
    StakingDelegation,
    RemoveDelegation,
    StandardTransfer,
    TokenTransfer(String),
    FeeForTokenTx,
    CustomTendermintMsg {
        msg_type: CustomTendermintMsgType,
        token_id: Option<String>,
    },
    TendermintIBCTransfer {
        token_id: Option<String>,
    },
}

#[derive(Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TransactionDetails {
    pub tx_hex: String,
    pub tx_hash: String,
    pub from: Vec<String>,
    pub to: Vec<String>,
    pub total_amount: BigDecimal,
    pub spent_by_me: BigDecimal,
    pub received_by_me: BigDecimal,
    pub my_balance_change: BigDecimal,
    pub block_height: u64,
    pub timestamp: u64,
    pub fee_details: Json,
    pub coin: String,
    pub internal_id: String,
    pub transaction_type: TransactionType,
    pub memo: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IguanaWalletBalance {
    pub address: String,
    pub balance: CoinBalance,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IguanaWalletBalanceMap {
    pub address: String,
    pub balance: HashMap<String, CoinBalance>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub enum Bip44Chain {
    External = 0,
    Internal = 1,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HDWalletBalance {
    pub accounts: Vec<HDAccountBalance>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HDWalletBalanceMap {
    pub accounts: Vec<HDAccountBalanceMap>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HDAccountBalance {
    pub account_index: u32,
    pub derivation_path: String,
    pub total_balance: CoinBalance,
    pub addresses: Vec<HDAddressBalance>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HDAccountBalanceMap {
    pub account_index: u32,
    pub derivation_path: String,
    pub total_balance: HashMap<String, CoinBalance>,
    pub addresses: Vec<HDAddressBalanceMap>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HDAddressBalance {
    pub address: String,
    pub derivation_path: String,
    pub chain: Bip44Chain,
    pub balance: CoinBalance,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HDAddressBalanceMap {
    pub address: String,
    pub derivation_path: String,
    pub chain: Bip44Chain,
    pub balance: HashMap<String, CoinBalance>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct HDAccountAddressId {
    pub account_id: u32,
    pub chain: Bip44Chain,
    pub address_id: u32,
}

impl Default for HDAccountAddressId {
    fn default() -> Self {
        HDAccountAddressId {
            account_id: 0,
            chain: Bip44Chain::External,
            address_id: 0,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, tag = "wallet_type")]
pub enum EnableCoinBalance {
    Iguana(IguanaWalletBalance),
    HD(HDWalletBalance),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, tag = "wallet_type")]
pub enum EnableCoinBalanceMap {
    Iguana(IguanaWalletBalanceMap),
    HD(HDWalletBalanceMap),
}

/// The `FirstSyncBlock` struct contains details about the block block that is used to start the synchronization
/// process.
/// It includes information about the requested block height, whether it predates the Sapling activation, and the
/// actual starting block height used during synchronization.
///
/// - `requested`: The requested block height during synchronization.
/// - `is_pre_sapling`: Indicates whether the block predates the Sapling activation.
/// - `actual`: The actual block height used for synchronization(may be altered).
#[derive(Debug, Deserialize)]
pub struct FirstSyncBlock {
    pub requested: u64,
    pub is_pre_sapling: bool,
    pub actual: u64,
}

/// `ZCoinActivationResult` provides information/data for Zcoin activation. It includes
/// details such as the ticker, the current block height, the wallet balance, and the result
/// of the first synchronization block (if applicable).
///
/// - `ticker`: A string representing the ticker of the Zcoin.
/// - `current_block`: The current block height at the time of this activation result.
/// - `wallet_balance`: Information about the wallet's coin balance and status.
/// - `first_sync_block`: An optional field containing details about the first synchronization block
///   during the activation process. It may be `None` if no first synchronization block is available.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZCoinActivationResult {
    pub ticker: String,
    pub current_block: u64,
    pub wallet_balance: EnableCoinBalance,
    pub first_sync_block: Option<FirstSyncBlock>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetNewAddressResponse {
    pub new_address: HDAddressBalanceMap,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HDAccountBalanceResponse {
    pub account_index: u32,
    pub derivation_path: String,
    pub addresses: Vec<HDAddressBalanceMap>,
    pub page_balance: HashMap<String, CoinBalance>,
    pub limit: usize,
    pub skipped: u32,
    pub total: u32,
    pub total_pages: usize,
    pub paging_options: PagingOptionsEnum<u32>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoinActivationResult {
    pub ticker: String,
    pub current_block: u64,
    pub wallet_balance: EnableCoinBalance,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UtxoStandardActivationResult {
    pub ticker: String,
    pub current_block: u64,
    pub wallet_balance: EnableCoinBalanceMap,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightningActivationResult {
    pub platform_coin: String,
    pub address: String,
    pub balance: CoinBalance,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitTaskResult {
    pub task_id: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, untagged)]
pub enum MmRpcResult<T> {
    Ok { result: T },
    Err(Json),
}

/// `InitZcoinStatus` encapsulates different states that may occur during the initialization of Zcoin,
/// These states include successful initialization, error conditions, ongoing
/// progress, and situations where user action is required.
///
/// - `Ok(ZCoinActivationResult)`: Indicates that Zcoin initialization was successful, with an associated
///   `ZCoinActivationResult` containing activation and status information.
/// - `Error(Json)`: Represents an error state during initialization, with an associated JSON object (`Json`)
///   providing details about the error.
/// - `InProgress(Json)`: Indicates that initialization is in progress, with an associated JSON object (`Json`)
///   containing information about the ongoing process.
/// - `UserActionRequired(Json)`: Denotes a state where user action is required for initialization to proceed,
///   with an associated JSON object (`Json`) providing instructions or requirements.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, tag = "status", content = "details")]
pub enum InitZcoinStatus {
    Ok(ZCoinActivationResult),
    Error(Json),
    InProgress(Json),
    UserActionRequired(Json),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, tag = "status", content = "details")]
pub enum InitUtxoStatus {
    Ok(UtxoStandardActivationResult),
    Error(Json),
    InProgress(Json),
    UserActionRequired(Json),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, tag = "status", content = "details")]
pub enum InitEthWithTokensStatus {
    Ok(EthWithTokensActivationResult),
    Error(Json),
    InProgress(Json),
    UserActionRequired(Json),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, tag = "status", content = "details")]
pub enum InitErc20TokenStatus {
    Ok(InitTokenActivationResult),
    Error(Json),
    InProgress(Json),
    UserActionRequired(Json),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, tag = "status", content = "details")]
pub enum InitLightningStatus {
    Ok(LightningActivationResult),
    Error(Json),
    InProgress(Json),
    UserActionRequired(Json),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, tag = "status", content = "details")]
pub enum CreateNewAccountStatus {
    Ok(HDAccountBalanceMap),
    Error(Json),
    InProgress(Json),
    UserActionRequired(Json),
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
pub enum HDAddressSelector {
    AddressId(HDAccountAddressId),
    DerivationPath { derivation_path: String },
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, tag = "status", content = "details")]
pub enum WithdrawStatus {
    Ok(TransactionDetails),
    Error(Json),
    InProgress(Json),
    UserActionRequired(Json),
}

pub mod withdraw_error {
    use mm2_number::BigDecimal;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct NotSufficientBalance {
        pub coin: String,
        pub available: BigDecimal,
        pub required: BigDecimal,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct AmountTooLow {
        pub amount: BigDecimal,
        pub threshold: BigDecimal,
    }
}

pub mod trade_preimage_error {
    use mm2_number::BigDecimal;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct NotSufficientBalance {
        pub coin: String,
        pub available: BigDecimal,
        pub required: BigDecimal,
        pub locked_by_swaps: Option<BigDecimal>,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct VolumeTooLow {
        pub coin: String,
        pub volume: BigDecimal,
        pub threshold: BigDecimal,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct NoSuchCoin {
        pub coin: String,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct InvalidParam {
        pub param: String,
        pub reason: String,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct PriceTooLow {
        pub price: BigDecimal,
        pub threshold: BigDecimal,
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct GetPublicKeyResult {
    pub public_key: String,
}

#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct GetPublicKeyHashResult {
    pub public_key_hash: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetSharedDbIdResult {
    pub shared_db_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetWalletNamesResult {
    pub wallet_names: Vec<String>,
    pub activated_wallet: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcV2Response<T> {
    pub mmrpc: String,
    pub id: Option<Json>,
    pub result: T,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoinBalance {
    pub spendable: BigDecimal,
    pub unspendable: BigDecimal,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnableSlpResponse {
    pub balances: HashMap<String, CoinBalance>,
    pub token_id: H256Json,
    pub platform_coin: String,
    pub required_confirmations: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnableSplResponse {
    pub balances: HashMap<String, CoinBalance>,
    pub token_contract_address: String,
    pub platform_coin: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "type", content = "data")]
pub enum DerivationMethod {
    Iguana,
    HDWallet(String),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoinAddressInfo<Balance> {
    pub derivation_method: DerivationMethod,
    pub pubkey: String,
    pub balances: Option<Balance>,
    pub tickers: Option<HashSet<String>>,
}

pub type TokenBalances = HashMap<String, CoinBalance>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IguanaEthWithTokensActivationResult {
    pub current_block: u64,
    pub eth_addresses_infos: HashMap<String, CoinAddressInfo<CoinBalance>>,
    pub erc20_addresses_infos: HashMap<String, CoinAddressInfo<TokenBalances>>,
    pub nfts_infos: Json,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HDEthWithTokensActivationResult {
    pub current_block: u64,
    pub ticker: String,
    pub wallet_balance: EnableCoinBalanceMap,
    pub nfts_infos: Json,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(untagged)]
pub enum EthWithTokensActivationResult {
    Iguana(IguanaEthWithTokensActivationResult),
    HD(HDEthWithTokensActivationResult),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InitTokenActivationResult {
    pub ticker: String,
    pub platform_coin: String,
    pub token_contract_address: String,
    pub current_block: u64,
    pub required_confirmations: u64,
    pub wallet_balance: EnableCoinBalanceMap,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnableBchWithTokensResponse {
    pub current_block: u64,
    pub bch_addresses_infos: HashMap<String, CoinAddressInfo<CoinBalance>>,
    pub slp_addresses_infos: HashMap<String, CoinAddressInfo<TokenBalances>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryTransactionDetails {
    #[serde(flatten)]
    pub tx: TransactionDetails,
    pub confirmations: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ZcoinTransactionDetails {
    pub tx_hash: String,
    pub from: HashSet<String>,
    pub to: HashSet<String>,
    pub spent_by_me: BigDecimal,
    pub received_by_me: BigDecimal,
    pub my_balance_change: BigDecimal,
    pub block_height: u64,
    pub timestamp: u64,
    pub transaction_fee: BigDecimal,
    pub coin: String,
    pub internal_id: i64,
    pub confirmations: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub enum PagingOptionsEnum<T> {
    FromId(T),
    PageNumber(NonZeroUsize),
}

pub type StandardHistoryV2Res = MyTxHistoryV2Response<HistoryTransactionDetails, String>;
pub type ZcoinHistoryRes = MyTxHistoryV2Response<ZcoinTransactionDetails, i64>;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MyTxHistoryV2Response<Tx, Id> {
    pub coin: String,
    pub target: MyTxHistoryTarget,
    pub current_block: u64,
    pub transactions: Vec<Tx>,
    pub sync_status: Json,
    pub limit: usize,
    pub skipped: usize,
    pub total: usize,
    pub total_pages: usize,
    pub paging_options: PagingOptionsEnum<Id>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type")]
#[serde(rename_all = "snake_case")]
pub enum MyTxHistoryTarget {
    Iguana,
    AccountId { account_id: u32 },
    AddressId(HDAccountAddressId),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UtxoFeeDetails {
    pub r#type: String,
    pub coin: Option<String>,
    pub amount: BigDecimal,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields, tag = "address_type", content = "address_data")]
pub enum OrderbookAddress {
    Transparent(String),
    Shielded,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RpcOrderbookEntryV2 {
    pub coin: String,
    pub address: OrderbookAddress,
    pub price: MmNumberMultiRepr,
    pub pubkey: String,
    pub uuid: Uuid,
    pub is_mine: bool,
    pub base_max_volume: MmNumberMultiRepr,
    pub base_min_volume: MmNumberMultiRepr,
    pub rel_max_volume: MmNumberMultiRepr,
    pub rel_min_volume: MmNumberMultiRepr,
    pub conf_settings: Option<OrderConfirmationsSettings>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AggregatedOrderbookEntryV2 {
    #[serde(flatten)]
    pub entry: RpcOrderbookEntryV2,
    pub base_max_volume_aggr: MmNumberMultiRepr,
    pub rel_max_volume_aggr: MmNumberMultiRepr,
}

#[derive(Debug, Deserialize, Eq, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MmNumberMultiRepr {
    pub decimal: BigDecimal,
    pub rational: BigRational,
    pub fraction: Fraction,
}

impl<T> From<T> for MmNumberMultiRepr
where
    MmNumber: From<T>,
{
    fn from(number: T) -> Self {
        let mm_number = MmNumber::from(number);
        MmNumberMultiRepr {
            decimal: mm_number.to_decimal(),
            rational: mm_number.to_ratio(),
            fraction: mm_number.to_fraction(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrderbookV2Response {
    pub asks: Vec<AggregatedOrderbookEntryV2>,
    pub base: String,
    pub bids: Vec<AggregatedOrderbookEntryV2>,
    pub net_id: u16,
    pub num_asks: usize,
    pub num_bids: usize,
    pub rel: String,
    pub timestamp: u64,
    pub total_asks_base_vol: MmNumberMultiRepr,
    pub total_asks_rel_vol: MmNumberMultiRepr,
    pub total_bids_base_vol: MmNumberMultiRepr,
    pub total_bids_rel_vol: MmNumberMultiRepr,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct BestOrdersV2Response {
    pub orders: HashMap<String, Vec<RpcOrderbookEntryV2>>,
    pub original_tickers: HashMap<String, HashSet<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignatureResponse {
    pub signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VerificationResponse {
    pub is_valid: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WithdrawResult {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TendermintActivationResult {
    pub address: String,
    pub current_block: u64,
    pub balance: Option<CoinBalance>,
    pub tokens_balances: Option<HashMap<String, CoinBalance>>,
    pub ticker: String,
    pub tokens_tickers: Option<HashSet<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GetLockedAmountResponse {
    pub coin: String,
    pub locked_amount: MmNumberMultiRepr,
}

pub mod gui_storage {
    use mm2_number::BigDecimal;
    use std::collections::BTreeSet;

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(tag = "type")]
    #[serde(rename_all = "lowercase")]
    pub enum AccountId {
        Iguana,
        HD { account_idx: u32 },
        HW { device_pubkey: String },
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct AccountWithEnabledFlag {
        pub account_id: AccountId,
        pub name: String,
        pub description: String,
        pub balance_usd: BigDecimal,
        pub enabled: bool,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct AccountWithCoins {
        pub account_id: AccountId,
        pub name: String,
        pub description: String,
        pub balance_usd: BigDecimal,
        pub coins: BTreeSet<String>,
    }

    #[derive(Debug, Deserialize, PartialEq)]
    #[serde(deny_unknown_fields)]
    pub struct AccountCoins {
        pub account_id: AccountId,
        pub coins: BTreeSet<String>,
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WatcherConf {
    #[serde(default = "common::sixty_f64")]
    pub wait_taker_payment: f64,
    #[serde(default = "common::one_f64")]
    pub wait_maker_payment_spend_factor: f64,
    #[serde(default = "common::one_and_half_f64")]
    pub refund_start_factor: f64,
    #[serde(default = "common::three_hundred_f64")]
    pub search_interval: f64,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct DisableResult {
    pub coin: String,
    pub cancelled_orders: HashSet<String>,
    pub passivized: bool,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct DisableCoinError {
    pub error: String,
    pub orders: DisableCoinOrders,
    pub active_swaps: Vec<Uuid>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct DisableCoinOrders {
    matching: Vec<Uuid>,
    cancelled: Vec<Uuid>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct CoinsNeededForKickstartResponse {
    pub result: Vec<String>,
}

#[derive(Serialize, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ActiveSwapsResponse {
    pub uuids: Vec<Uuid>,
    pub statuses: Option<HashMap<Uuid, Json>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Erc20TokenInfo {
    pub symbol: String,
    pub decimals: u8,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "type", content = "info")]
pub enum TokenInfo {
    ERC20(Erc20TokenInfo),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenInfoResponse {
    pub config_ticker: Option<String>,
    #[serde(flatten)]
    pub info: TokenInfo,
}

#[derive(Debug, Deserialize)]
pub struct SpentUtxo {
    pub txid: String,
    pub vout: u32,
    pub value: BigDecimal,
}

#[derive(Debug, Deserialize)]
pub struct ConsolidateUtxoResponse {
    pub tx: TransactionDetails,
    pub consolidated_utxos: Vec<SpentUtxo>,
}

#[derive(Debug, Deserialize)]
pub struct AddressUtxos {
    pub address: String,
    pub count: usize,
    pub utxos: Vec<SpentUtxo>,
    pub derivation_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct FetchUtxosResponse {
    pub total_count: usize,
    pub addresses: Vec<AddressUtxos>,
}
