#![allow(clippy::all)]

use super::{
    CoinBalance, CommonSwapOpsV2, FindPaymentSpendError, FundingTxSpend, HistorySyncState, MarketCoinOps, MmCoin,
    RawTransactionFut, RawTransactionRequest, RefundTakerPaymentArgs, SearchForFundingSpendErr, SwapOps, TradeFee,
    TransactionEnum, TransactionFut,
};
use crate::coin_errors::{AddressFromPubkeyError, ValidatePaymentResult};
use crate::hd_wallet::{AddrToString, HDAddressSelector};
use crate::{
    coin_errors::MyAddressError, BalanceFut, CanRefundHtlc, CheckIfMyPaymentSentArgs, ConfirmPaymentInput,
    FeeApproxStage, FoundSwapTxSpend, GenPreimageResult, GenTakerFundingSpendArgs, GenTakerPaymentSpendArgs,
    MmCoinEnum, NegotiateSwapContractAddrErr, ParseCoinAssocTypes, PaymentInstructionArgs, PaymentInstructions,
    PaymentInstructionsErr, RawTransactionResult, RefundFundingSecretArgs, RefundPaymentArgs, RefundResult,
    SearchForSwapTxSpendInput, SendMakerPaymentSpendPreimageInput, SendPaymentArgs, SendTakerFundingArgs,
    SignRawTransactionRequest, SignatureResult, SpendPaymentArgs, TakerCoinSwapOpsV2, TradePreimageFut,
    TradePreimageResult, TradePreimageValue, Transaction, TransactionErr, TransactionResult, TxMarshalingErr,
    TxPreimageWithSig, UnexpectedDerivationMethod, ValidateAddressResult, ValidateFeeArgs, ValidateInstructionsErr,
    ValidateOtherPubKeyErr, ValidatePaymentFut, ValidatePaymentInput, ValidateSwapV2TxResult, ValidateTakerFundingArgs,
    ValidateTakerFundingSpendPreimageResult, ValidateTakerPaymentSpendPreimageResult, VerificationResult,
    WaitForHTLCTxSpendArgs, WatcherOps, WatcherReward, WatcherRewardError, WatcherSearchForSwapTxSpendInput,
    WatcherValidatePaymentInput, WatcherValidateTakerFeeInput, WeakSpawner, WithdrawFut, WithdrawRequest,
};
use crate::{DexFee, GetTakerFundingFeeArgs, ToBytes, ValidateWatcherSpendInput};
use async_trait::async_trait;
use common::executor::AbortedError;
use futures01::Future;
use keys::KeyPair;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::{BigDecimal, MmNumber};
#[cfg(any(test, feature = "for-tests"))]
use mocktopus::macros::*;
use rpc::v1::types::{Bytes as BytesJson, H264 as H264Json};
use serde_json::Value as Json;
use std::fmt::{Display, Formatter};
use std::ops::Deref;
use std::sync::Arc;

/// Dummy coin struct used in tests which functions are unimplemented but then mocked
/// in specific test to emulate the required behaviour
#[derive(Clone, Debug)]
pub struct TestCoin(Arc<TestCoinImpl>);

impl Deref for TestCoin {
    type Target = TestCoinImpl;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Debug)]
pub struct TestCoinImpl {
    ticker: String,
}

impl Default for TestCoin {
    fn default() -> Self {
        TestCoin(Arc::new(TestCoinImpl { ticker: "test".into() }))
    }
}

impl TestCoin {
    pub fn new(ticker: &str) -> TestCoin {
        TestCoin(Arc::new(TestCoinImpl { ticker: ticker.into() }))
    }
}

#[async_trait]
#[cfg_attr(any(test, feature = "for-tests"), mockable)]
impl MarketCoinOps for TestCoin {
    fn ticker(&self) -> &str {
        &self.ticker
    }

    fn my_address(&self) -> MmResult<String, MyAddressError> {
        unimplemented!()
    }

    fn address_from_pubkey(&self, _pubkey: &H264Json) -> MmResult<String, AddressFromPubkeyError> {
        unimplemented!()
    }

    async fn get_public_key(&self) -> Result<String, MmError<UnexpectedDerivationMethod>> {
        unimplemented!()
    }

    fn sign_message_hash(&self, _message: &str) -> Option<[u8; 32]> {
        unimplemented!()
    }

    fn sign_message(&self, _message: &str, _address: Option<HDAddressSelector>) -> SignatureResult<String> {
        unimplemented!()
    }

    fn verify_message(&self, _signature: &str, _message: &str, _address: &str) -> VerificationResult<bool> {
        unimplemented!()
    }

    fn my_balance(&self) -> BalanceFut<CoinBalance> {
        unimplemented!()
    }

    fn platform_coin_balance(&self) -> BalanceFut<BigDecimal> {
        unimplemented!()
    }

    fn platform_ticker(&self) -> &str {
        &self.ticker
    }

    /// Receives raw transaction bytes in hexadecimal format as input and returns tx hash in hexadecimal format
    fn send_raw_tx(&self, tx: &str) -> Box<dyn Future<Item = String, Error = String> + Send> {
        unimplemented!()
    }

    fn send_raw_tx_bytes(&self, tx: &[u8]) -> Box<dyn Future<Item = String, Error = String> + Send> {
        unimplemented!()
    }

    #[inline(always)]
    async fn sign_raw_tx(&self, _args: &SignRawTransactionRequest) -> RawTransactionResult {
        unimplemented!()
    }

    fn wait_for_confirmations(&self, _input: ConfirmPaymentInput) -> Box<dyn Future<Item = (), Error = String> + Send> {
        unimplemented!()
    }

    async fn wait_for_htlc_tx_spend(&self, args: WaitForHTLCTxSpendArgs<'_>) -> TransactionResult {
        unimplemented!()
    }

    fn tx_enum_from_bytes(&self, _bytes: &[u8]) -> Result<TransactionEnum, MmError<TxMarshalingErr>> {
        MmError::err(TxMarshalingErr::NotSupported(
            "tx_enum_from_bytes is not supported for Test coin yet.".to_string(),
        ))
    }

    fn current_block(&self) -> Box<dyn Future<Item = u64, Error = String> + Send> {
        unimplemented!()
    }

    fn display_priv_key(&self) -> Result<String, String> {
        unimplemented!()
    }

    fn min_tx_amount(&self) -> BigDecimal {
        Default::default()
    }

    fn min_trading_vol(&self) -> MmNumber {
        MmNumber::from("0.00777")
    }

    fn should_burn_directly(&self) -> bool {
        &self.ticker == "KMD"
    }

    fn should_burn_dex_fee(&self) -> bool {
        false
    }

    fn is_trezor(&self) -> bool {
        unimplemented!()
    }
}

#[async_trait]
#[cfg_attr(any(test, feature = "for-tests"), mockable)]
impl SwapOps for TestCoin {
    async fn send_taker_fee(&self, dex_fee: DexFee, uuid: &[u8], expire_at: u64) -> TransactionResult {
        unimplemented!()
    }

    async fn send_maker_payment(&self, maker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        unimplemented!()
    }

    async fn send_taker_payment(&self, taker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        unimplemented!()
    }

    async fn send_maker_spends_taker_payment(
        &self,
        _maker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        unimplemented!()
    }

    async fn send_taker_spends_maker_payment(
        &self,
        _taker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        unimplemented!()
    }

    async fn send_taker_refunds_payment(
        &self,
        _taker_refunds_payment_args: RefundPaymentArgs<'_>,
    ) -> TransactionResult {
        unimplemented!()
    }

    async fn send_maker_refunds_payment(
        &self,
        _maker_refunds_payment_args: RefundPaymentArgs<'_>,
    ) -> TransactionResult {
        unimplemented!()
    }

    async fn validate_fee(&self, validate_fee_args: ValidateFeeArgs<'_>) -> ValidatePaymentResult<()> {
        unimplemented!()
    }

    async fn validate_maker_payment(&self, _input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        unimplemented!()
    }

    async fn validate_taker_payment(&self, _input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        unimplemented!()
    }

    async fn check_if_my_payment_sent(
        &self,
        if_my_payment_sent_args: CheckIfMyPaymentSentArgs<'_>,
    ) -> Result<Option<TransactionEnum>, String> {
        unimplemented!()
    }

    async fn search_for_swap_tx_spend_my(
        &self,
        _input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        unimplemented!()
    }

    async fn search_for_swap_tx_spend_other(
        &self,
        _input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        unimplemented!()
    }

    async fn extract_secret(
        &self,
        secret_hash: &[u8],
        spend_tx: &[u8],
        watcher_reward: bool,
    ) -> Result<[u8; 32], String> {
        unimplemented!()
    }

    fn is_auto_refundable(&self) -> bool {
        false
    }

    async fn wait_for_htlc_refund(&self, _tx: &[u8], _locktime: u64) -> RefundResult<()> {
        unimplemented!()
    }

    fn negotiate_swap_contract_addr(
        &self,
        other_side_address: Option<&[u8]>,
    ) -> Result<Option<BytesJson>, MmError<NegotiateSwapContractAddrErr>> {
        unimplemented!()
    }

    fn derive_htlc_key_pair(&self, _swap_unique_data: &[u8]) -> KeyPair {
        unimplemented!()
    }

    fn derive_htlc_pubkey(&self, _swap_unique_data: &[u8]) -> [u8; 33] {
        unimplemented!()
    }

    async fn can_refund_htlc(&self, locktime: u64) -> Result<CanRefundHtlc, String> {
        unimplemented!()
    }

    fn validate_other_pubkey(&self, raw_pubkey: &[u8]) -> MmResult<(), ValidateOtherPubKeyErr> {
        unimplemented!()
    }

    async fn maker_payment_instructions(
        &self,
        _args: PaymentInstructionArgs<'_>,
    ) -> Result<Option<Vec<u8>>, MmError<PaymentInstructionsErr>> {
        unimplemented!()
    }

    async fn taker_payment_instructions(
        &self,
        args: PaymentInstructionArgs<'_>,
    ) -> Result<Option<Vec<u8>>, MmError<PaymentInstructionsErr>> {
        unimplemented!()
    }

    fn validate_maker_payment_instructions(
        &self,
        _instructions: &[u8],
        args: PaymentInstructionArgs,
    ) -> Result<PaymentInstructions, MmError<ValidateInstructionsErr>> {
        unimplemented!()
    }

    fn validate_taker_payment_instructions(
        &self,
        _instructions: &[u8],
        args: PaymentInstructionArgs,
    ) -> Result<PaymentInstructions, MmError<ValidateInstructionsErr>> {
        unimplemented!()
    }

    async fn on_taker_payment_refund_start(&self, _maker_payment: &[u8]) -> RefundResult<()> {
        unimplemented!()
    }

    async fn on_taker_payment_refund_success(&self, _maker_payment: &[u8]) -> RefundResult<()> {
        unimplemented!()
    }

    async fn on_maker_payment_refund_start(&self, _taker_payment: &[u8]) -> RefundResult<()> {
        unimplemented!()
    }

    async fn on_maker_payment_refund_success(&self, _taker_payment: &[u8]) -> RefundResult<()> {
        unimplemented!()
    }
}

#[async_trait]
#[cfg_attr(any(test, feature = "for-tests"), mockable)]
impl WatcherOps for TestCoin {
    fn create_maker_payment_spend_preimage(
        &self,
        _maker_payment_tx: &[u8],
        _time_lock: u64,
        _maker_pub: &[u8],
        _secret_hash: &[u8],
        _swap_unique_data: &[u8],
    ) -> TransactionFut {
        unimplemented!();
    }

    fn send_maker_payment_spend_preimage(&self, _input: SendMakerPaymentSpendPreimageInput) -> TransactionFut {
        unimplemented!();
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
        unimplemented!();
    }

    fn send_taker_payment_refund_preimage(&self, _watcher_refunds_payment_args: RefundPaymentArgs) -> TransactionFut {
        unimplemented!();
    }

    fn watcher_validate_taker_fee(&self, input: WatcherValidateTakerFeeInput) -> ValidatePaymentFut<()> {
        unimplemented!();
    }

    fn watcher_validate_taker_payment(&self, _input: WatcherValidatePaymentInput) -> ValidatePaymentFut<()> {
        unimplemented!();
    }

    fn taker_validates_payment_spend_or_refund(&self, input: ValidateWatcherSpendInput) -> ValidatePaymentFut<()> {
        unimplemented!()
    }

    async fn watcher_search_for_swap_tx_spend(
        &self,
        input: WatcherSearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        unimplemented!();
    }

    async fn get_taker_watcher_reward(
        &self,
        other_coin: &MmCoinEnum,
        coin_amount: Option<BigDecimal>,
        other_coin_amount: Option<BigDecimal>,
        reward_amount: Option<BigDecimal>,
        wait_until: u64,
    ) -> Result<WatcherReward, MmError<WatcherRewardError>> {
        unimplemented!()
    }

    async fn get_maker_watcher_reward(
        &self,
        other_coin: &MmCoinEnum,
        reward_amount: Option<BigDecimal>,
        wait_until: u64,
    ) -> Result<Option<WatcherReward>, MmError<WatcherRewardError>> {
        unimplemented!()
    }
}

#[async_trait]
#[cfg_attr(any(test, feature = "for-tests"), mockable)]
impl MmCoin for TestCoin {
    fn is_asset_chain(&self) -> bool {
        unimplemented!()
    }

    fn spawner(&self) -> WeakSpawner {
        unimplemented!()
    }

    fn get_raw_transaction(&self, _req: RawTransactionRequest) -> RawTransactionFut<'_> {
        unimplemented!()
    }

    fn get_tx_hex_by_hash(&self, tx_hash: Vec<u8>) -> RawTransactionFut<'_> {
        unimplemented!()
    }

    fn withdraw(&self, req: WithdrawRequest) -> WithdrawFut {
        unimplemented!()
    }

    fn decimals(&self) -> u8 {
        unimplemented!()
    }

    fn convert_to_address(&self, from: &str, to_address_format: Json) -> Result<String, String> {
        unimplemented!()
    }

    fn validate_address(&self, address: &str) -> ValidateAddressResult {
        unimplemented!()
    }

    fn process_history_loop(&self, ctx: MmArc) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        unimplemented!()
    }

    fn history_sync_status(&self) -> HistorySyncState {
        unimplemented!()
    }

    /// Get fee to be paid per 1 swap transaction
    fn get_trade_fee(&self) -> Box<dyn Future<Item = TradeFee, Error = String> + Send> {
        unimplemented!()
    }

    async fn get_sender_trade_fee(
        &self,
        _value: TradePreimageValue,
        _stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        unimplemented!()
    }

    fn get_receiver_trade_fee(&self, _stage: FeeApproxStage) -> TradePreimageFut<TradeFee> {
        unimplemented!()
    }

    async fn get_fee_to_send_taker_fee(
        &self,
        _dex_fee_amount: DexFee,
        _stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        unimplemented!()
    }

    fn required_confirmations(&self) -> u64 {
        1
    }

    fn requires_notarization(&self) -> bool {
        false
    }

    fn set_required_confirmations(&self, _confirmations: u64) {
        unimplemented!()
    }

    fn set_requires_notarization(&self, _requires_nota: bool) {
        unimplemented!()
    }

    fn swap_contract_address(&self) -> Option<BytesJson> {
        unimplemented!()
    }

    fn fallback_swap_contract(&self) -> Option<BytesJson> {
        unimplemented!()
    }

    fn mature_confirmations(&self) -> Option<u32> {
        unimplemented!()
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
        Ok(())
    }

    fn on_token_deactivated(&self, _ticker: &str) {
        ()
    }
}

pub struct TestPubkey {}

impl ToBytes for TestPubkey {
    fn to_bytes(&self) -> Vec<u8> {
        vec![]
    }
}

#[derive(Debug)]
pub struct TestTx {}

impl Transaction for TestTx {
    fn tx_hex(&self) -> Vec<u8> {
        todo!()
    }

    fn tx_hash_as_bytes(&self) -> BytesJson {
        todo!()
    }
}

pub struct TestPreimage {}

impl ToBytes for TestPreimage {
    fn to_bytes(&self) -> Vec<u8> {
        vec![]
    }
}

pub struct TestSig {}

impl ToBytes for TestSig {
    fn to_bytes(&self) -> Vec<u8> {
        vec![]
    }
}

pub struct TestAddress {}

impl AddrToString for TestAddress {
    fn addr_to_string(&self) -> String {
        unimplemented!()
    }
}

impl Display for TestAddress {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        unimplemented!()
    }
}

#[async_trait]
impl ParseCoinAssocTypes for TestCoin {
    type Address = TestAddress;
    type AddressParseError = String;
    type Pubkey = TestPubkey;
    type PubkeyParseError = String;
    type Tx = TestTx;
    type TxParseError = String;
    type Preimage = TestPreimage;
    type PreimageParseError = String;
    type Sig = TestSig;
    type SigParseError = String;

    async fn my_addr(&self) -> Self::Address {
        todo!()
    }

    fn parse_address(&self, address: &str) -> Result<Self::Address, Self::AddressParseError> {
        todo!()
    }

    fn parse_pubkey(&self, pubkey: &[u8]) -> Result<Self::Pubkey, Self::PubkeyParseError> {
        unimplemented!()
    }

    fn parse_tx(&self, tx: &[u8]) -> Result<Self::Tx, Self::TxParseError> {
        unimplemented!()
    }

    fn parse_preimage(&self, preimage: &[u8]) -> Result<Self::Preimage, Self::PreimageParseError> {
        todo!()
    }

    fn parse_signature(&self, sig: &[u8]) -> Result<Self::Sig, Self::SigParseError> {
        todo!()
    }
}

#[async_trait]
#[cfg_attr(any(test, feature = "for-tests"), mockable)]
impl TakerCoinSwapOpsV2 for TestCoin {
    async fn send_taker_funding(&self, args: SendTakerFundingArgs<'_>) -> Result<Self::Tx, TransactionErr> {
        todo!()
    }

    async fn get_fee_to_send_taker_funding(&self, args: GetTakerFundingFeeArgs) -> TradePreimageResult<TradeFee> {
        unimplemented!()
    }

    async fn get_fee_to_spend_taker_funding(&self, _stage: FeeApproxStage) -> TradePreimageResult<TradeFee> {
        unimplemented!()
    }

    async fn get_fee_to_spend_taker_payment(&self, _stage: FeeApproxStage) -> TradePreimageResult<TradeFee> {
        unimplemented!()
    }

    async fn get_fee_to_refund_taker_payment(&self, _stage: FeeApproxStage) -> TradePreimageResult<TradeFee> {
        unimplemented!()
    }

    async fn validate_taker_funding(&self, args: ValidateTakerFundingArgs<'_, Self>) -> ValidateSwapV2TxResult {
        unimplemented!()
    }

    async fn refund_taker_funding_timelock(
        &self,
        args: RefundTakerPaymentArgs<'_>,
    ) -> Result<Self::Tx, TransactionErr> {
        todo!()
    }

    async fn refund_taker_funding_secret(
        &self,
        args: RefundFundingSecretArgs<'_, Self>,
    ) -> Result<Self::Tx, TransactionErr> {
        todo!()
    }

    async fn search_for_taker_funding_spend(
        &self,
        tx: &Self::Tx,
        from_block: u64,
        secret_hash: &[u8],
    ) -> Result<Option<FundingTxSpend<Self>>, SearchForFundingSpendErr> {
        todo!()
    }

    async fn gen_taker_funding_spend_preimage(
        &self,
        args: &GenTakerFundingSpendArgs<'_, Self>,
        swap_unique_data: &[u8],
    ) -> GenPreimageResult<Self> {
        todo!()
    }

    async fn validate_taker_funding_spend_preimage(
        &self,
        gen_args: &GenTakerFundingSpendArgs<'_, Self>,
        preimage: &TxPreimageWithSig<Self>,
    ) -> ValidateTakerFundingSpendPreimageResult {
        todo!()
    }

    async fn sign_and_send_taker_funding_spend(
        &self,
        preimage: &TxPreimageWithSig<Self>,
        args: &GenTakerFundingSpendArgs<'_, Self>,
        swap_unique_data: &[u8],
    ) -> Result<Self::Tx, TransactionErr> {
        todo!()
    }

    async fn refund_combined_taker_payment(
        &self,
        args: RefundTakerPaymentArgs<'_>,
    ) -> Result<Self::Tx, TransactionErr> {
        unimplemented!()
    }

    async fn gen_taker_payment_spend_preimage(
        &self,
        args: &GenTakerPaymentSpendArgs<'_, Self>,
        swap_unique_data: &[u8],
    ) -> GenPreimageResult<Self> {
        unimplemented!()
    }

    async fn validate_taker_payment_spend_preimage(
        &self,
        gen_args: &GenTakerPaymentSpendArgs<'_, Self>,
        preimage: &TxPreimageWithSig<Self>,
    ) -> ValidateTakerPaymentSpendPreimageResult {
        unimplemented!()
    }

    async fn sign_and_broadcast_taker_payment_spend(
        &self,
        preimage: Option<&TxPreimageWithSig<Self>>,
        gen_args: &GenTakerPaymentSpendArgs<'_, Self>,
        secret: &[u8],
        swap_unique_data: &[u8],
    ) -> Result<Self::Tx, TransactionErr> {
        unimplemented!()
    }

    async fn find_taker_payment_spend_tx(
        &self,
        taker_payment: &Self::Tx,
        from_block: u64,
        wait_until: u64,
    ) -> MmResult<Self::Tx, FindPaymentSpendError> {
        unimplemented!()
    }

    async fn extract_secret_v2(&self, secret_hash: &[u8], spend_tx: &Self::Tx) -> Result<[u8; 32], String> {
        unimplemented!()
    }
}

impl CommonSwapOpsV2 for TestCoin {
    fn derive_htlc_pubkey_v2(&self, _swap_unique_data: &[u8]) -> Self::Pubkey {
        todo!()
    }

    fn derive_htlc_pubkey_v2_bytes(&self, _swap_unique_data: &[u8]) -> Vec<u8> {
        todo!()
    }

    #[inline(always)]
    fn taker_pubkey_bytes(&self) -> Option<Vec<u8>> {
        todo!()
    }
}
