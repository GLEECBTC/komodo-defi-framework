//! Module containing implementation for Tendermint Tokens. They include native assets + IBC

use super::ibc::IBC_GAS_LIMIT_DEFAULT;
use super::{
    create_withdraw_msg_as_any, TendermintCoin, TendermintFeeDetails, GAS_LIMIT_DEFAULT, MIN_TX_SATOSHIS,
    TIMEOUT_HEIGHT_DELTA, TX_DEFAULT_MEMO,
};
use crate::coin_errors::{AddressFromPubkeyError, ValidatePaymentResult};
use crate::hd_wallet::HDAddressSelector;
use crate::utxo::utxo_common::big_decimal_from_sat;
use crate::{
    big_decimal_from_sat_unsigned, utxo::sat_from_big_decimal, BalanceFut, BigDecimal, CheckIfMyPaymentSentArgs,
    CoinBalance, ConfirmPaymentInput, DexFee, FeeApproxStage, FoundSwapTxSpend, HistorySyncState, MarketCoinOps,
    MmCoin, MyAddressError, NegotiateSwapContractAddrErr, RawTransactionError, RawTransactionFut,
    RawTransactionRequest, RawTransactionResult, RefundPaymentArgs, SearchForSwapTxSpendInput, SendPaymentArgs,
    SignRawTransactionRequest, SignatureResult, SpendPaymentArgs, SwapOps, TradeFee, TradePreimageFut,
    TradePreimageResult, TradePreimageValue, TransactionDetails, TransactionEnum, TransactionErr, TransactionResult,
    TransactionType, TxFeeDetails, TxMarshalingErr, UnexpectedDerivationMethod, ValidateAddressResult, ValidateFeeArgs,
    ValidateOtherPubKeyErr, ValidatePaymentInput, VerificationResult, WaitForHTLCTxSpendArgs, WatcherOps, WeakSpawner,
    WithdrawError, WithdrawFut, WithdrawRequest,
};
use async_trait::async_trait;
use bitcrypto::sha256;
use common::executor::abortable_queue::AbortableQueue;
use common::executor::{AbortableSystem, AbortedError};
use common::log::warn;
use common::Future01CompatExt;
use cosmrs::{tx::Fee, AccountId, Coin, Denom};
use futures::{FutureExt, TryFutureExt};
use futures01::Future;
use keys::KeyPair;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::MmNumber;
use primitives::hash::H256;
use rpc::v1::types::{Bytes as BytesJson, H264 as H264Json};
use serde_json::Value as Json;
use std::ops::Deref;
use std::str::FromStr;
use std::sync::Arc;

pub struct TendermintTokenImpl {
    pub ticker: String,
    pub platform_coin: TendermintCoin,
    pub decimals: u8,
    pub denom: Denom,
    /// This spawner is used to spawn coin's related futures that should be aborted on coin deactivation
    /// or on [`MmArc::stop`].
    abortable_system: AbortableQueue,
}

#[derive(Clone)]
pub struct TendermintToken(Arc<TendermintTokenImpl>);

impl Deref for TendermintToken {
    type Target = TendermintTokenImpl;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct TendermintTokenProtocolInfo {
    pub platform: String,
    pub decimals: u8,
    pub denom: Denom,
}

#[derive(Clone, Deserialize)]
pub struct TendermintTokenActivationParams {}

pub enum TendermintTokenInitError {
    Internal(String),
    MyAddressError(String),
    CouldNotFetchBalance(String),
    PlatformCoinMismatch,
}

impl From<MyAddressError> for TendermintTokenInitError {
    fn from(err: MyAddressError) -> Self {
        TendermintTokenInitError::MyAddressError(err.to_string())
    }
}

impl From<AbortedError> for TendermintTokenInitError {
    fn from(e: AbortedError) -> Self {
        TendermintTokenInitError::Internal(e.to_string())
    }
}

impl TendermintToken {
    pub fn new(
        ticker: String,
        platform_coin: TendermintCoin,
        decimals: u8,
        denom: Denom,
    ) -> MmResult<Self, TendermintTokenInitError> {
        let token_impl = TendermintTokenImpl {
            abortable_system: platform_coin.abortable_system.create_subsystem()?,
            ticker,
            platform_coin,
            decimals,
            denom,
        };
        Ok(TendermintToken(Arc::new(token_impl)))
    }

    fn token_id(&self) -> BytesJson {
        let denom_hash = sha256(self.denom.as_ref().to_lowercase().as_bytes());
        H256::from(denom_hash.take()).to_vec().into()
    }
}

#[async_trait]
#[allow(unused_variables)]
impl SwapOps for TendermintToken {
    async fn send_taker_fee(&self, dex_fee: DexFee, uuid: &[u8], expire_at: u64) -> TransactionResult {
        self.platform_coin
            .send_taker_fee_for_denom(&dex_fee, self.denom.clone(), self.decimals, uuid, expire_at)
            .compat()
            .await
    }

    async fn send_maker_payment(&self, maker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        self.platform_coin
            .send_htlc_for_denom(
                maker_payment_args.time_lock_duration,
                maker_payment_args.other_pubkey,
                maker_payment_args.secret_hash,
                maker_payment_args.amount,
                self.denom.clone(),
                self.decimals,
            )
            .compat()
            .await
    }

    async fn send_taker_payment(&self, taker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        self.platform_coin
            .send_htlc_for_denom(
                taker_payment_args.time_lock_duration,
                taker_payment_args.other_pubkey,
                taker_payment_args.secret_hash,
                taker_payment_args.amount,
                self.denom.clone(),
                self.decimals,
            )
            .compat()
            .await
    }

    async fn send_maker_spends_taker_payment(
        &self,
        maker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        self.platform_coin
            .send_maker_spends_taker_payment(maker_spends_payment_args)
            .await
    }

    async fn send_taker_spends_maker_payment(
        &self,
        taker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        self.platform_coin
            .send_taker_spends_maker_payment(taker_spends_payment_args)
            .await
    }

    async fn send_taker_refunds_payment(&self, taker_refunds_payment_args: RefundPaymentArgs<'_>) -> TransactionResult {
        Err(TransactionErr::Plain(
            "Doesn't need transaction broadcast to be refunded".into(),
        ))
    }

    async fn send_maker_refunds_payment(&self, maker_refunds_payment_args: RefundPaymentArgs<'_>) -> TransactionResult {
        Err(TransactionErr::Plain(
            "Doesn't need transaction broadcast to be refunded".into(),
        ))
    }

    async fn validate_fee(&self, validate_fee_args: ValidateFeeArgs<'_>) -> ValidatePaymentResult<()> {
        self.platform_coin
            .validate_fee_for_denom(
                validate_fee_args.fee_tx,
                validate_fee_args.expected_sender,
                validate_fee_args.dex_fee,
                self.decimals,
                validate_fee_args.uuid,
                self.denom.to_string(),
            )
            .compat()
            .await
    }

    async fn validate_maker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        self.platform_coin
            .validate_payment_for_denom(input, self.denom.clone(), self.decimals)
            .await
    }

    async fn validate_taker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        self.platform_coin
            .validate_payment_for_denom(input, self.denom.clone(), self.decimals)
            .await
    }

    async fn check_if_my_payment_sent(
        &self,
        if_my_payment_sent_args: CheckIfMyPaymentSentArgs<'_>,
    ) -> Result<Option<TransactionEnum>, String> {
        self.platform_coin
            .check_if_my_payment_sent_for_denom(
                self.decimals,
                self.denom.clone(),
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
        self.platform_coin.search_for_swap_tx_spend_my(input).await
    }

    async fn search_for_swap_tx_spend_other(
        &self,
        input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        self.platform_coin.search_for_swap_tx_spend_other(input).await
    }

    async fn extract_secret(
        &self,
        secret_hash: &[u8],
        spend_tx: &[u8],
        watcher_reward: bool,
    ) -> Result<[u8; 32], String> {
        self.platform_coin
            .extract_secret(secret_hash, spend_tx, watcher_reward)
            .await
    }

    fn negotiate_swap_contract_addr(
        &self,
        other_side_address: Option<&[u8]>,
    ) -> Result<Option<BytesJson>, MmError<NegotiateSwapContractAddrErr>> {
        self.platform_coin.negotiate_swap_contract_addr(other_side_address)
    }

    #[inline]
    fn derive_htlc_key_pair(&self, swap_unique_data: &[u8]) -> KeyPair {
        self.platform_coin.derive_htlc_key_pair(swap_unique_data)
    }

    #[inline]
    fn derive_htlc_pubkey(&self, swap_unique_data: &[u8]) -> [u8; 33] {
        self.platform_coin.derive_htlc_pubkey(swap_unique_data)
    }

    fn validate_other_pubkey(&self, raw_pubkey: &[u8]) -> MmResult<(), ValidateOtherPubKeyErr> {
        self.platform_coin.validate_other_pubkey(raw_pubkey)
    }
}

#[async_trait]
impl WatcherOps for TendermintToken {}

#[async_trait]
impl MarketCoinOps for TendermintToken {
    fn ticker(&self) -> &str {
        &self.ticker
    }

    fn my_address(&self) -> MmResult<String, MyAddressError> {
        self.platform_coin.my_address()
    }

    fn address_from_pubkey(&self, pubkey: &H264Json) -> MmResult<String, AddressFromPubkeyError> {
        self.platform_coin.address_from_pubkey(pubkey)
    }

    async fn get_public_key(&self) -> Result<String, MmError<UnexpectedDerivationMethod>> {
        self.platform_coin.get_public_key().await
    }

    fn sign_message_hash(&self, message: &str) -> Option<[u8; 32]> {
        self.platform_coin.sign_message_hash(message)
    }

    fn sign_message(&self, message: &str, address: Option<HDAddressSelector>) -> SignatureResult<String> {
        self.platform_coin.sign_message(message, address)
    }

    fn verify_message(&self, signature: &str, message: &str, address: &str) -> VerificationResult<bool> {
        self.platform_coin.verify_message(signature, message, address)
    }

    fn my_balance(&self) -> BalanceFut<CoinBalance> {
        let coin = self.clone();
        let fut = async move {
            let balance_denom = coin
                .platform_coin
                .account_balance_for_denom(&coin.platform_coin.account_id, coin.denom.to_string())
                .await
                .map_mm_err()?;
            Ok(CoinBalance {
                spendable: big_decimal_from_sat_unsigned(balance_denom, coin.decimals),
                unspendable: BigDecimal::default(),
            })
        };
        Box::new(fut.boxed().compat())
    }

    fn platform_coin_balance(&self) -> BalanceFut<BigDecimal> {
        self.platform_coin.my_spendable_balance()
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
        MmError::err(RawTransactionError::NotImplemented {
            coin: self.ticker().to_string(),
        })
    }

    fn wait_for_confirmations(&self, input: ConfirmPaymentInput) -> Box<dyn Future<Item = (), Error = String> + Send> {
        self.platform_coin.wait_for_confirmations(input)
    }

    async fn wait_for_htlc_tx_spend(&self, args: WaitForHTLCTxSpendArgs<'_>) -> TransactionResult {
        self.platform_coin
            .wait_for_htlc_tx_spend(WaitForHTLCTxSpendArgs {
                tx_bytes: args.tx_bytes,
                secret_hash: args.secret_hash,
                wait_until: args.wait_until,
                from_block: args.from_block,
                swap_contract_address: args.swap_contract_address,
                check_every: args.check_every,
                watcher_reward: false,
            })
            .await
    }

    fn tx_enum_from_bytes(&self, bytes: &[u8]) -> Result<TransactionEnum, MmError<TxMarshalingErr>> {
        self.platform_coin.tx_enum_from_bytes(bytes)
    }

    fn current_block(&self) -> Box<dyn Future<Item = u64, Error = String> + Send> {
        self.platform_coin.current_block()
    }

    fn display_priv_key(&self) -> Result<String, String> {
        self.platform_coin.display_priv_key()
    }

    #[inline]
    fn min_tx_amount(&self) -> BigDecimal {
        big_decimal_from_sat(MIN_TX_SATOSHIS, self.decimals)
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
        self.platform_coin.is_trezor()
    }
}

#[async_trait]
#[allow(unused_variables)]
impl MmCoin for TendermintToken {
    fn is_asset_chain(&self) -> bool {
        false
    }

    fn wallet_only(&self, ctx: &MmArc) -> bool {
        self.platform_coin.wallet_only(ctx)
    }

    fn spawner(&self) -> WeakSpawner {
        self.abortable_system.weak_spawner()
    }

    fn withdraw(&self, req: WithdrawRequest) -> WithdrawFut {
        let platform = self.platform_coin.clone();
        let token = self.clone();
        let fut = async move {
            let to_address =
                AccountId::from_str(&req.to).map_to_mm(|e| WithdrawError::InvalidAddress(e.to_string()))?;

            let is_ibc_transfer =
                to_address.prefix() != platform.protocol_info.account_prefix || req.ibc_source_channel.is_some();

            let (account_id, maybe_priv_key) = platform
                .extract_account_id_and_private_key(req.from)
                .map_err(|e| WithdrawError::InternalError(e.to_string()))?;

            let (base_denom_balance, base_denom_balance_dec) = platform
                .get_balance_as_unsigned_and_decimal(&account_id, &platform.protocol_info.denom, token.decimals())
                .await
                .map_mm_err()?;

            let (balance_denom, balance_dec) = platform
                .get_balance_as_unsigned_and_decimal(&account_id, &token.denom, token.decimals())
                .await
                .map_mm_err()?;

            let (amount_denom, amount_dec, total_amount) = if req.max {
                (
                    balance_denom,
                    big_decimal_from_sat_unsigned(balance_denom, token.decimals),
                    balance_dec,
                )
            } else {
                if balance_dec < req.amount {
                    return MmError::err(WithdrawError::NotSufficientBalance {
                        coin: token.ticker.clone(),
                        available: balance_dec,
                        required: req.amount,
                    });
                }

                (
                    sat_from_big_decimal(&req.amount, token.decimals()).map_mm_err()?,
                    req.amount.clone(),
                    req.amount,
                )
            };

            if !platform.is_tx_amount_enough(token.decimals, &amount_dec) {
                return MmError::err(WithdrawError::AmountTooLow {
                    amount: amount_dec,
                    threshold: token.min_tx_amount(),
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
                        platform
                            .get_healthy_ibc_channel_for_address_prefix(to_address.prefix())
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
                &token.denom,
                amount_denom,
                channel_id,
            )
            .await?;

            let memo = req.memo.unwrap_or_else(|| TX_DEFAULT_MEMO.into());
            let current_block = token
                .current_block()
                .compat()
                .await
                .map_to_mm(WithdrawError::Transport)?;

            let timeout_height = current_block + TIMEOUT_HEIGHT_DELTA;

            let (_, gas_limit) = if is_ibc_transfer {
                platform.gas_info_for_withdraw(&req.fee, IBC_GAS_LIMIT_DEFAULT)
            } else {
                platform.gas_info_for_withdraw(&req.fee, GAS_LIMIT_DEFAULT)
            };

            let fee_amount_u64 = platform
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

            let fee_amount_dec = big_decimal_from_sat_unsigned(fee_amount_u64, platform.decimals());

            if base_denom_balance < fee_amount_u64 {
                return MmError::err(WithdrawError::NotSufficientPlatformBalanceForFee {
                    coin: platform.ticker().to_string(),
                    available: base_denom_balance_dec,
                    required: fee_amount_dec,
                });
            }

            let fee_amount = Coin {
                denom: platform.protocol_info.denom.clone(),
                amount: fee_amount_u64.into(),
            };

            let fee = Fee::from_amount_and_gas(fee_amount, gas_limit);

            let account_info = platform.account_info(&account_id).await.map_mm_err()?;

            let tx = platform
                .any_to_transaction_data(maybe_priv_key, msg_payload, &account_info, fee, timeout_height, &memo)
                .await
                .map_to_mm(|e| WithdrawError::InternalError(e.to_string()))?;

            let internal_id =
                super::tendermint_tx_internal_id(tx.tx_hash().unwrap_or_default().as_bytes(), Some(token.token_id()));

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
                    coin: platform.ticker().to_string(),
                    amount: fee_amount_dec,
                    uamount: fee_amount_u64,
                    gas_limit,
                })),
                coin: token.ticker.clone(),
                internal_id,
                kmd_rewards: None,
                transaction_type: if is_ibc_transfer {
                    TransactionType::TendermintIBCTransfer {
                        token_id: Some(token.token_id()),
                    }
                } else {
                    TransactionType::TokenTransfer(token.token_id())
                },
                memo: Some(memo),
            })
        };
        Box::new(fut.boxed().compat())
    }

    fn get_raw_transaction(&self, req: RawTransactionRequest) -> RawTransactionFut<'_> {
        self.platform_coin.get_raw_transaction(req)
    }

    fn get_tx_hex_by_hash(&self, tx_hash: Vec<u8>) -> RawTransactionFut<'_> {
        unimplemented!()
    }

    fn decimals(&self) -> u8 {
        self.decimals
    }

    fn convert_to_address(&self, from: &str, to_address_format: Json) -> Result<String, String> {
        self.platform_coin.convert_to_address(from, to_address_format)
    }

    fn validate_address(&self, address: &str) -> ValidateAddressResult {
        self.platform_coin.validate_address(address)
    }

    fn process_history_loop(&self, ctx: MmArc) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        warn!("process_history_loop is deprecated, tendermint uses tx_history_v2");
        Box::new(futures01::future::err(()))
    }

    fn history_sync_status(&self) -> HistorySyncState {
        self.platform_coin.history_sync_status()
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

        self.platform_coin
            .get_sender_trade_fee_for_denom(self.ticker.clone(), self.denom.clone(), self.decimals, amount)
            .await
    }

    fn get_receiver_trade_fee(&self, stage: FeeApproxStage) -> TradePreimageFut<TradeFee> {
        // As makers may not have a balance in the coin they want to swap, we need to
        // calculate this fee in platform coin.
        //
        // p.s.: Same goes for ETH assets: https://github.com/KomodoPlatform/komodo-defi-framework/blob/b0fd99e8406e67ea06435dd028991caa5f522b5c/mm2src/coins/eth.rs#L4892-L4895
        self.platform_coin.get_receiver_trade_fee(stage)
    }

    async fn get_fee_to_send_taker_fee(
        &self,
        dex_fee_amount: DexFee,
        _stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        self.platform_coin
            .get_fee_to_send_taker_fee_for_denom(self.ticker.clone(), self.denom.clone(), self.decimals, dex_fee_amount)
            .await
    }

    fn required_confirmations(&self) -> u64 {
        self.platform_coin.required_confirmations()
    }

    fn requires_notarization(&self) -> bool {
        self.platform_coin.requires_notarization()
    }

    fn set_required_confirmations(&self, confirmations: u64) {
        warn!("set_required_confirmations is not supported for tendermint")
    }

    fn set_requires_notarization(&self, requires_nota: bool) {
        self.platform_coin.set_requires_notarization(requires_nota)
    }

    fn swap_contract_address(&self) -> Option<BytesJson> {
        self.platform_coin.swap_contract_address()
    }

    fn fallback_swap_contract(&self) -> Option<BytesJson> {
        self.platform_coin.fallback_swap_contract()
    }

    fn mature_confirmations(&self) -> Option<u32> {
        None
    }

    fn coin_protocol_info(&self, amount_to_receive: Option<MmNumber>) -> Vec<u8> {
        self.platform_coin.coin_protocol_info(amount_to_receive)
    }

    fn is_coin_protocol_supported(
        &self,
        info: &Option<Vec<u8>>,
        amount_to_send: Option<MmNumber>,
        locktime: u64,
        is_maker: bool,
    ) -> bool {
        self.platform_coin
            .is_coin_protocol_supported(info, amount_to_send, locktime, is_maker)
    }

    fn on_disabled(&self) -> Result<(), AbortedError> {
        self.abortable_system.abort_all()
    }

    fn on_token_deactivated(&self, _ticker: &str) {}
}
