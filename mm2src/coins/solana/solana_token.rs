#![allow(dead_code)]
#![allow(unused_variables)]

use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

use async_trait::async_trait;
use common::executor::abortable_queue::{AbortableQueue, WeakSpawner};
use common::executor::{AbortableSystem, AbortedError};
use common::Future01CompatExt;
use derive_more::Display;
use futures::{FutureExt, TryFutureExt};
use futures01::Future;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::{BigDecimal, MmNumber};
use num_traits::ToPrimitive;
use num_traits::Zero;
use rpc::v1::types::{Bytes as RpcBytes, H264 as RpcH264};
use serde::Deserialize;

use crate::coin_errors::{AddressFromPubkeyError, MyAddressError, ValidatePaymentResult};
use crate::hd_wallet::HDAddressSelector;
use crate::solana::SolanaFeeDetails;
use crate::{
    solana::SolanaCoin, BalanceFut, CoinBalance, RawTransactionFut, RawTransactionRequest, TxFeeDetails, WithdrawFut,
    WithdrawRequest,
};
use crate::{
    CheckIfMyPaymentSentArgs, ConfirmPaymentInput, DexFee, FeeApproxStage, FoundSwapTxSpend, HistorySyncState,
    MarketCoinOps, MmCoin, NegotiateSwapContractAddrErr, RawTransactionResult, RefundPaymentArgs,
    SearchForSwapTxSpendInput, SendPaymentArgs, SignRawTransactionRequest, SignatureResult, SpendPaymentArgs, SwapOps,
    TradeFee, TradePreimageFut, TradePreimageResult, TradePreimageValue, TransactionEnum, TransactionResult,
    TxMarshalingErr, UnexpectedDerivationMethod, ValidateAddressResult, ValidateFeeArgs, ValidateOtherPubKeyErr,
    ValidatePaymentInput, VerificationResult, WaitForHTLCTxSpendArgs, WatcherOps, WithdrawError,
};
use solana_pubkey::Pubkey as SolanaAddress;
use solana_transaction::Transaction;
use spl_associated_token_account_client::address::get_associated_token_address;
use spl_associated_token_account_client::instruction::create_associated_token_account;
use spl_token as spl_token_program;

pub struct SolanaTokenFields {
    pub ticker: String,
    address: SolanaAddress,
    pub platform_coin: SolanaCoin,
    pub protocol_info: SolanaTokenProtocolInfo,
    abortable_system: AbortableQueue,
}

impl Deref for SolanaToken {
    type Target = SolanaTokenFields;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone)]
pub struct SolanaToken(Arc<SolanaTokenFields>);

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SolanaTokenProtocolInfo {
    pub platform: String,
    pub decimals: u8,
    #[serde(serialize_with = "serialize_pubkey", deserialize_with = "deserialize_pubkey")]
    pub mint_address: SolanaAddress,
}

pub fn serialize_pubkey<S>(public_key: &SolanaAddress, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&public_key.to_string())
}

pub fn deserialize_pubkey<'de, D>(deserializer: D) -> Result<SolanaAddress, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    SolanaAddress::from_str(&s).map_err(serde::de::Error::custom)
}

#[derive(Clone, Debug)]
pub struct SolanaTokenInitError {
    pub ticker: String,
    pub kind: SolanaTokenInitErrorKind,
}

#[derive(Display, Debug, Clone)]
pub enum SolanaTokenInitErrorKind {
    QueryError {
        reason: String,
    },
    Internal {
        reason: String,
    },
    #[display(fmt = "None of the RPC servers are healthy.")]
    UnhealthyRPCs,
    #[display(
        fmt = "Expected platform coin is '{expected_platform_coin}' but requested one is '{actual_platform_coin}'."
    )]
    PlatformCoinMismatch {
        expected_platform_coin: String,
        actual_platform_coin: String,
    },
}

impl SolanaToken {
    pub async fn init(
        ticker: String,
        platform_coin: SolanaCoin,
        protocol_info: SolanaTokenProtocolInfo,
    ) -> MmResult<Self, SolanaTokenInitError> {
        let abortable_system = platform_coin
            .abortable_system
            .create_subsystem()
            .map_err(|e| SolanaTokenInitError {
                ticker: ticker.clone(),
                kind: SolanaTokenInitErrorKind::Internal { reason: e.to_string() },
            })?;

        let address = spl_associated_token_account_client::address::get_associated_token_address(
            &platform_coin.address,
            &protocol_info.mint_address,
        );

        // TODO: Handle non-existent mint accounts, then uncomment this code.
        //
        // let rpc = platform_coin.rpc_client().await.map_err(|e| SolanaTokenInitError {
        //     ticker: ticker.clone(),
        //     kind: SolanaTokenInitErrorKind::UnhealthyRPCs,
        // })?;

        // let mint_account = rpc
        //     .get_account(&protocol_info.mint_address)
        //     .map_err(|e| SolanaTokenInitError {
        //         ticker: ticker.clone(),
        //         kind: SolanaTokenInitErrorKind::QueryError { reason: e.to_string() },
        //     })?;

        // if mint_account.owner != spl_token_program::id() {
        //     return MmError::err(SolanaTokenInitError {
        //         ticker: ticker.clone(),
        //         kind: SolanaTokenInitErrorKind::QueryError {
        //             reason: format!(
        //                 "Unsupported SPL program. Expected Program ID: '{}', Got: '{}'.",
        //                 spl_token_program::id(),
        //                 mint_account.owner
        //             ),
        //         },
        //     });
        // }

        let token_fields = SolanaTokenFields {
            ticker,
            address,
            platform_coin,
            protocol_info,
            abortable_system,
        };

        Ok(SolanaToken(Arc::new(token_fields)))
    }
}

#[async_trait]
impl MmCoin for SolanaToken {
    fn is_asset_chain(&self) -> bool {
        todo!()
    }

    fn wallet_only(&self, ctx: &MmArc) -> bool {
        todo!()
    }

    fn spawner(&self) -> WeakSpawner {
        self.abortable_system.weak_spawner()
    }

    fn withdraw(&self, req: WithdrawRequest) -> WithdrawFut {
        let token = self.clone();
        let coin = self.platform_coin.clone();

        let fut = async move {
            let rpc = coin
                .rpc_client()
                .await
                .map_err(|e| WithdrawError::Transport(e.into_inner()))?;

            // `to` can be either a Solana address, or a token address. We create
            // `to_token_account` regardless to support the both cases.
            let to = SolanaAddress::from_str(&req.to).map_err(|e| WithdrawError::InvalidAddress(e.to_string()))?;
            let to_token_account = get_associated_token_address(&to, &token.protocol_info.mint_address);

            let amount_u64 = if req.max {
                let balance = token
                    .my_balance()
                    .compat()
                    .await
                    .map_err(|e| WithdrawError::Transport(e.to_string()))?;

                balance.spendable.to_u64().ok_or_else(|| {
                    MmError::new(WithdrawError::InternalError(format!(
                        "Couldn't convert {} to u64.",
                        balance.spendable
                    )))
                })?
            } else {
                let scale = BigDecimal::from(10u64.pow(token.protocol_info.decimals as u32));
                let big_decimal = &req.amount * &scale;

                big_decimal.to_u64().ok_or_else(|| {
                    MmError::new(WithdrawError::InternalError(format!(
                        "Couldn't convert {big_decimal} to u64."
                    )))
                })?
            };

            if amount_u64 == 0 {
                return MmError::err(WithdrawError::AmountTooLow {
                    amount: req.amount,
                    threshold: token.min_tx_amount(),
                });
            }

            // Instructions:
            //  - Create recipient address if missing.
            //  - Transfer.
            let mut instructions = Vec::new();

            if let Err(e) = rpc.get_account(&to_token_account) {
                // TODO: This might be a different kind of error. Check it more
                // precisely (like how we do it in `solana_coin`) before sending
                // the create instruction.

                instructions.push(create_associated_token_account(
                    &coin.address,
                    &to,
                    &token.protocol_info.mint_address,
                    &spl_token_program::id(),
                ));
            };

            let transfer_ix = spl_token_program::instruction::transfer_checked(
                &spl_token_program::id(),
                &coin.address,
                &token.protocol_info.mint_address,
                &to_token_account,
                &coin.address,
                &[],
                amount_u64,
                token.protocol_info.decimals,
            )
            .map_err(|e| WithdrawError::InternalError(e.to_string()))?;
            instructions.push(transfer_ix);

            let recent_blockhash = rpc
                .get_latest_blockhash()
                .map_err(|e| WithdrawError::Transport(e.to_string()))?;

            let tx = Transaction::new_signed_with_payer(
                &instructions,
                Some(&coin.address),
                &[&coin.keypair],
                recent_blockhash,
            );

            // TX hash is the first signature (base58 String).
            let tx_hash = tx
                .signatures
                .first()
                .map(|s| s.to_string())
                .ok_or_else(|| WithdrawError::InternalError("Couldn't find the TX signature.".to_owned()))?;

            let tx_bytes =
                bincode::serialize(&tx).map_err(|e| MmError::new(WithdrawError::InternalError(e.to_string())))?;

            let tx_data = crate::TransactionData::new_signed(rpc::v1::types::Bytes(tx_bytes), tx_hash.clone());

            let amount_dec =
                BigDecimal::from(amount_u64) / BigDecimal::from(10u64.pow(token.protocol_info.decimals as u32));

            let fee_lamports = rpc
                .get_fee_for_message(tx.message())
                .map_err(|e| WithdrawError::Transport(e.to_string()))?;
            let fee_dec = BigDecimal::from(fee_lamports)
                / BigDecimal::from(10u64.pow(super::solana_coin::SOLANA_DECIMALS as u32));

            let received_by_me = if to == coin.address {
                amount_dec.clone()
            } else {
                BigDecimal::zero()
            };

            Ok(crate::TransactionDetails {
                tx: tx_data,
                from: vec![coin.address.to_string()],
                to: vec![to.to_string()],
                total_amount: amount_dec.clone(),
                spent_by_me: amount_dec.clone(),
                received_by_me,
                my_balance_change: -amount_dec,
                block_height: 0,
                timestamp: 0,
                fee_details: Some(TxFeeDetails::Solana(SolanaFeeDetails { amount: fee_dec })),
                coin: req.coin,
                internal_id: rpc::v1::types::Bytes(tx_hash.into_bytes()),
                kmd_rewards: None,
                transaction_type: crate::TransactionType::StandardTransfer,
                // TODO: Add memo instruction to the TX.
                memo: None,
            })
        };

        Box::new(fut.boxed().compat())
    }

    fn get_raw_transaction(&self, req: RawTransactionRequest) -> RawTransactionFut<'_> {
        todo!()
    }

    fn get_tx_hex_by_hash(&self, tx_hash: Vec<u8>) -> RawTransactionFut<'_> {
        todo!()
    }

    fn decimals(&self) -> u8 {
        self.protocol_info.decimals
    }

    fn convert_to_address(&self, from: &str, to_address_format: serde_json::Value) -> Result<String, String> {
        todo!()
    }

    fn validate_address(&self, address: &str) -> ValidateAddressResult {
        todo!()
    }

    fn process_history_loop(&self, ctx: MmArc) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        todo!()
    }

    fn history_sync_status(&self) -> HistorySyncState {
        todo!()
    }

    fn get_trade_fee(&self) -> Box<dyn Future<Item = TradeFee, Error = String> + Send> {
        todo!()
    }

    async fn get_sender_trade_fee(
        &self,
        value: TradePreimageValue,
        _stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        todo!()
    }

    fn get_receiver_trade_fee(&self, stage: FeeApproxStage) -> TradePreimageFut<TradeFee> {
        todo!()
    }

    async fn get_fee_to_send_taker_fee(
        &self,
        dex_fee_amount: DexFee,
        _stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        todo!()
    }

    fn required_confirmations(&self) -> u64 {
        todo!()
    }

    fn requires_notarization(&self) -> bool {
        todo!()
    }

    fn set_required_confirmations(&self, confirmations: u64) {
        todo!()
    }

    fn set_requires_notarization(&self, requires_nota: bool) {
        todo!()
    }

    fn swap_contract_address(&self) -> Option<RpcBytes> {
        todo!()
    }

    fn fallback_swap_contract(&self) -> Option<RpcBytes> {
        todo!()
    }

    fn mature_confirmations(&self) -> Option<u32> {
        todo!()
    }

    fn coin_protocol_info(&self, amount_to_receive: Option<MmNumber>) -> Vec<u8> {
        todo!()
    }

    fn is_coin_protocol_supported(
        &self,
        info: &Option<Vec<u8>>,
        amount_to_send: Option<MmNumber>,
        locktime: u64,
        is_maker: bool,
    ) -> bool {
        todo!()
    }

    fn on_disabled(&self) -> Result<(), AbortedError> {
        todo!()
    }

    fn on_token_deactivated(&self, ticker: &str) {
        todo!()
    }
}

#[async_trait]
impl MarketCoinOps for SolanaToken {
    fn ticker(&self) -> &str {
        &self.ticker
    }

    fn my_address(&self) -> MmResult<String, MyAddressError> {
        Ok(self.address.to_string())
    }

    fn address_from_pubkey(&self, pubkey: &RpcH264) -> MmResult<String, AddressFromPubkeyError> {
        todo!()
    }

    async fn get_public_key(&self) -> Result<String, MmError<UnexpectedDerivationMethod>> {
        todo!()
    }

    fn sign_message_hash(&self, _message: &str) -> Option<[u8; 32]> {
        todo!()
    }

    fn sign_message(&self, _message: &str, _address: Option<HDAddressSelector>) -> SignatureResult<String> {
        todo!()
    }

    fn verify_message(&self, _signature: &str, _message: &str, _address: &str) -> VerificationResult<bool> {
        todo!()
    }

    fn my_balance(&self) -> BalanceFut<CoinBalance> {
        let token = self.clone();
        let platform_coin = self.platform_coin.clone();

        let fut = async move { platform_coin.token_balance(&token.protocol_info.mint_address).await };

        Box::new(fut.boxed().compat())
    }

    fn platform_coin_balance(&self) -> BalanceFut<BigDecimal> {
        todo!()
    }

    fn platform_ticker(&self) -> &str {
        self.platform_coin.ticker()
    }

    fn send_raw_tx(&self, tx: &str) -> Box<dyn Future<Item = String, Error = String> + Send> {
        self.platform_coin.send_raw_tx(tx)
    }

    fn send_raw_tx_bytes(&self, tx: &[u8]) -> Box<dyn Future<Item = String, Error = String> + Send> {
        self.platform_coin.send_raw_tx_bytes(tx)
    }

    #[inline(always)]
    async fn sign_raw_tx(&self, _args: &SignRawTransactionRequest) -> RawTransactionResult {
        todo!()
    }

    fn wait_for_confirmations(&self, input: ConfirmPaymentInput) -> Box<dyn Future<Item = (), Error = String> + Send> {
        todo!()
    }

    async fn wait_for_htlc_tx_spend(&self, args: WaitForHTLCTxSpendArgs<'_>) -> TransactionResult {
        todo!()
    }

    fn tx_enum_from_bytes(&self, bytes: &[u8]) -> Result<TransactionEnum, MmError<TxMarshalingErr>> {
        todo!()
    }

    fn current_block(&self) -> Box<dyn Future<Item = u64, Error = String> + Send> {
        self.platform_coin.current_block()
    }

    fn display_priv_key(&self) -> Result<String, String> {
        todo!()
    }

    #[inline]
    fn min_tx_amount(&self) -> BigDecimal {
        self.platform_coin.min_tx_amount()
    }

    #[inline]
    fn min_trading_vol(&self) -> MmNumber {
        todo!()
    }

    #[inline]
    fn should_burn_dex_fee(&self) -> bool {
        todo!()
    }

    fn is_trezor(&self) -> bool {
        todo!()
    }
}

#[async_trait]
impl SwapOps for SolanaToken {
    async fn send_taker_fee(&self, dex_fee: DexFee, uuid: &[u8], expire_at: u64) -> TransactionResult {
        todo!()
    }

    async fn send_maker_payment(&self, maker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        todo!()
    }

    async fn send_taker_payment(&self, taker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        todo!()
    }

    async fn send_maker_spends_taker_payment(
        &self,
        maker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        todo!()
    }

    async fn send_taker_spends_maker_payment(
        &self,
        taker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        todo!()
    }

    async fn send_taker_refunds_payment(&self, taker_refunds_payment_args: RefundPaymentArgs<'_>) -> TransactionResult {
        todo!()
    }

    async fn send_maker_refunds_payment(&self, maker_refunds_payment_args: RefundPaymentArgs<'_>) -> TransactionResult {
        todo!()
    }

    async fn validate_fee(&self, validate_fee_args: ValidateFeeArgs<'_>) -> ValidatePaymentResult<()> {
        todo!()
    }

    async fn validate_maker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        todo!()
    }

    async fn validate_taker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        todo!()
    }

    async fn check_if_my_payment_sent(
        &self,
        if_my_payment_sent_args: CheckIfMyPaymentSentArgs<'_>,
    ) -> Result<Option<TransactionEnum>, String> {
        todo!()
    }

    async fn search_for_swap_tx_spend_my(
        &self,
        input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        todo!()
    }

    async fn search_for_swap_tx_spend_other(
        &self,
        input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        todo!()
    }

    async fn extract_secret(
        &self,
        secret_hash: &[u8],
        spend_tx: &[u8],
        watcher_reward: bool,
    ) -> Result<[u8; 32], String> {
        todo!()
    }

    fn negotiate_swap_contract_addr(
        &self,
        other_side_address: Option<&[u8]>,
    ) -> Result<Option<RpcBytes>, MmError<NegotiateSwapContractAddrErr>> {
        todo!()
    }

    #[inline]
    fn derive_htlc_key_pair(&self, _swap_unique_data: &[u8]) -> keys::KeyPair {
        todo!()
    }

    #[inline]
    fn derive_htlc_pubkey(&self, _swap_unique_data: &[u8]) -> [u8; 33] {
        todo!()
    }

    fn validate_other_pubkey(&self, raw_pubkey: &[u8]) -> MmResult<(), ValidateOtherPubKeyErr> {
        todo!()
    }
}

#[async_trait]
impl WatcherOps for SolanaToken {}
