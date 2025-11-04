#![allow(unused_variables)]
#![allow(dead_code)]

use std::str::FromStr;
use std::sync::Arc;
use std::{collections::HashMap, ops::Deref};

use async_trait::async_trait;
use common::executor::{
    abortable_queue::{AbortableQueue, WeakSpawner},
    AbortableSystem, AbortedError,
};
use common::now_sec;
use derive_more::Display;
use futures::lock::Mutex as AsyncMutex;
use futures::{FutureExt, TryFutureExt};
use futures01::Future;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::{BigDecimal, MmNumber};
use nom::AsBytes;
use num_traits::ToPrimitive;
use num_traits::Zero;
use parking_lot::Mutex as PaMutex;
use rpc::v1::types::Bytes as BytesJson;
use rpc::v1::types::{Bytes as RpcBytes, H264 as RpcH264};
use solana_bincode::limited_deserialize;
use solana_keypair::{keypair_from_seed, Keypair};
use solana_pubkey::Pubkey as SolanaAddress;
use solana_rpc_client::nonblocking::rpc_client::RpcClient;
use solana_rpc_client_types::request::TokenAccountsFilter;
use solana_signer::Signer;
use solana_transaction::Transaction;
use url::Url;

use crate::TxFeeDetails;
use crate::{
    coin_errors::{AddressFromPubkeyError, MyAddressError, ValidatePaymentResult},
    hd_wallet::HDAddressSelector,
    BalanceError, BalanceFut, CheckIfMyPaymentSentArgs, CoinBalance, ConfirmPaymentInput, DexFee, FeeApproxStage,
    FoundSwapTxSpend, HistorySyncState, MarketCoinOps, MmCoin, NegotiateSwapContractAddrErr, PrivKeyBuildPolicy,
    RawTransactionFut, RawTransactionRequest, RawTransactionResult, RefundPaymentArgs, SearchForSwapTxSpendInput,
    SendPaymentArgs, SignRawTransactionRequest, SignatureResult, SpendPaymentArgs, SwapOps, TradeFee, TradePreimageFut,
    TradePreimageResult, TradePreimageValue, TransactionData, TransactionDetails, TransactionEnum, TransactionResult,
    TransactionType, TxMarshalingErr, UnexpectedDerivationMethod, ValidateAddressResult, ValidateFeeArgs,
    ValidateOtherPubKeyErr, ValidatePaymentInput, VerificationResult, WaitForHTLCTxSpendArgs, WatcherOps,
    WithdrawError, WithdrawFut, WithdrawRequest,
};

pub const SOLANA_DECIMALS: u8 = 9;

/// Maximum over-the-wire size of a Transaction
///   1280 is IPv6 minimum MTU
///   40 bytes is the size of the IPv6 header
///   8 bytes is the size of the fragment header
///
/// Ported from: https://github.com/anza-xyz/solana-sdk/blob/ac902c4bdb8b0a1/packet/src/lib.rs#L28-L32
pub const PACKET_DATA_SIZE: usize = 1280 - 40 - 8;

#[derive(Clone, Deserialize)]
pub struct RpcNode {
    url: Url,
}

#[derive(Clone)]
pub struct SolanaCoin(Arc<SolanaCoinFields>);

pub struct SolanaCoinFields {
    ticker: String,
    pub(crate) address: SolanaAddress,
    pub(crate) keypair: Keypair,
    pub(crate) abortable_system: AbortableQueue,
    rpc_clients: AsyncMutex<Vec<Arc<RpcClient>>>,
    protocol_info: SolanaProtocolInfo,
    pub tokens_info: PaMutex<HashMap<String, super::SolanaTokenProtocolInfo>>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SolanaProtocolInfo {}

impl Deref for SolanaCoin {
    type Target = SolanaCoinFields;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Clone, Debug)]
pub struct SolanaInitError {
    pub ticker: String,
    pub kind: SolanaInitErrorKind,
}

#[derive(Display, Debug, Clone)]
pub enum SolanaInitErrorKind {
    EmptyRpcUrls,
    RpcClientInitError {
        reason: String,
    },
    Internal {
        reason: String,
    },
    #[display(fmt = "Unsupported private-key policy: {policy_type}")]
    UnsupportedPrivKeyPolicy {
        policy_type: &'static str,
    },
    QueryError {
        reason: String,
    },
}

/// Fees associated with a Solana transaction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SolanaFeeDetails {
    /// Network fee in SOL.
    pub fee_amount: BigDecimal,
    /// Rent in SOL when an associated token account (ATA) is created.
    ///
    /// This is 0 if no ATA creation is needed.
    pub rent_amount: BigDecimal,
    /// Sum of the network fee and rent.
    pub total_amount: BigDecimal,
}

impl SolanaCoin {
    pub async fn init(
        ctx: &MmArc,
        ticker: String,
        protocol_info: SolanaProtocolInfo,
        nodes: Vec<RpcNode>,
        priv_key_policy: PrivKeyBuildPolicy,
    ) -> MmResult<SolanaCoin, SolanaInitError> {
        if nodes.is_empty() {
            return MmError::err(SolanaInitError {
                ticker,
                kind: SolanaInitErrorKind::EmptyRpcUrls,
            });
        }

        // TODO: This isn't fully right and needs to be fixed before prod.
        // ref: https://github.com/KomodoPlatform/komodo-defi-framework/pull/2598#discussion_r2311756777
        let priv_key = match priv_key_policy {
            PrivKeyBuildPolicy::IguanaPrivKey(priv_key) => priv_key,
            PrivKeyBuildPolicy::Trezor => {
                return MmError::err(SolanaInitError {
                    ticker,
                    kind: SolanaInitErrorKind::UnsupportedPrivKeyPolicy { policy_type: "Trezor" },
                })
            },
            PrivKeyBuildPolicy::GlobalHDAccount(_) => {
                return MmError::err(SolanaInitError {
                    ticker,
                    kind: SolanaInitErrorKind::UnsupportedPrivKeyPolicy {
                        policy_type: "GlobalHDAccount",
                    },
                })
            },
            PrivKeyBuildPolicy::WalletConnect { .. } => {
                return MmError::err(SolanaInitError {
                    ticker,
                    kind: SolanaInitErrorKind::UnsupportedPrivKeyPolicy {
                        policy_type: "WalletConnect",
                    },
                })
            },
        };

        let keypair = keypair_from_seed(priv_key.as_bytes()).map_to_mm(|e| SolanaInitError {
            ticker: ticker.clone(),
            kind: SolanaInitErrorKind::Internal { reason: e.to_string() },
        })?;

        let address = SolanaAddress::from_str(&keypair.pubkey().to_string()).map_to_mm(|e| SolanaInitError {
            ticker: ticker.clone(),
            kind: SolanaInitErrorKind::Internal { reason: e.to_string() },
        })?;

        let rpc_clients: Vec<Arc<RpcClient>> = nodes
            .iter()
            .map(|n| Arc::new(RpcClient::new(n.url.to_string())))
            .collect();

        let abortable_system = ctx.abortable_system.create_subsystem().map_to_mm(|e| SolanaInitError {
            ticker: ticker.clone(),
            kind: SolanaInitErrorKind::Internal { reason: e.to_string() },
        })?;

        let fields = SolanaCoinFields {
            ticker,
            address,
            keypair,
            abortable_system,
            rpc_clients: AsyncMutex::new(rpc_clients),
            protocol_info,
            tokens_info: PaMutex::new(HashMap::new()),
        };

        Ok(SolanaCoin(Arc::new(fields)))
    }

    pub(crate) async fn rpc_client(&self) -> MmResult<Arc<RpcClient>, String> {
        let mut rpcs = self.rpc_clients.lock().await;

        for (index, rpc) in rpcs.iter().enumerate() {
            if rpc.get_health().await.is_ok() {
                rpcs.rotate_left(index);
                return Ok(rpcs[0].clone());
            }
        }

        MmError::err("No healthy RPC client found.".to_owned())
    }

    pub fn add_activated_token(&self, ticker: String, info: super::SolanaTokenProtocolInfo) {
        self.tokens_info.lock().insert(ticker, info);
    }

    pub async fn token_balance(&self, mint_address: &SolanaAddress) -> Result<CoinBalance, MmError<BalanceError>> {
        let rpc = self
            .rpc_client()
            .map_err(|e| BalanceError::Transport(e.into_inner()))
            .await?;

        if let Err(e) = rpc
            .get_token_accounts_by_owner(&self.address, TokenAccountsFilter::Mint(*mint_address))
            .await
        {
            if e.kind.to_string().contains("could not find mint") {
                return Ok(CoinBalance {
                    spendable: BigDecimal::zero(),
                    unspendable: BigDecimal::zero(),
                });
            }

            return MmError::err(BalanceError::Transport(e.to_string()));
        };

        let token_account =
            spl_associated_token_account_client::address::get_associated_token_address(&self.address, mint_address);

        let balance_string = rpc
            .get_token_account_balance(&token_account)
            .await
            .map_err(|e| BalanceError::Transport(e.to_string()))?
            .ui_amount_string;

        let balance = BigDecimal::from_str(&balance_string).map_err(|e| BalanceError::Internal(e.to_string()))?;

        Ok(CoinBalance {
            spendable: balance,
            unspendable: Default::default(),
        })
    }

    /// Calculates the withdraw amount (in lamports) that can be withdrawn based on the
    /// user's request along with the network fee (in lamports).
    ///
    /// Returns the amount to withdraw and network fee in lamports on success or
    /// [`WithdrawError`] if the request is invalid or cannot be processed.
    async fn calculate_withdraw_and_fee_amount(&self, req: &WithdrawRequest) -> MmResult<(u64, u64), WithdrawError> {
        let rpc = self
            .rpc_client()
            .await
            .map_err(|e| WithdrawError::Transport(e.into_inner()))?;

        let recent_blockhash = rpc
            .get_latest_blockhash()
            .await
            .map_err(|e| WithdrawError::Transport(e.to_string()))?;

        // Dummy TX to estimate the fee.
        let tx = solana_system_transaction::transfer(&self.keypair, &self.address, 0, recent_blockhash);
        let fee_u64 = rpc
            .get_fee_for_message(tx.message())
            .await
            .map_err(|e| WithdrawError::Transport(e.to_string()))?;

        let balance_u64 = rpc
            .get_balance(&self.address)
            .await
            .map_err(|e| WithdrawError::Transport(e.to_string()))?;

        if req.max {
            let amount = balance_u64.saturating_sub(fee_u64);
            let amount_big_decimal = u64_lamports_to_big_decimal(amount, SOLANA_DECIMALS);

            // Amount must be bigger than min_tx_amount.
            if amount_big_decimal < self.min_tx_amount() {
                return MmError::err(WithdrawError::AmountTooLow {
                    amount: amount_big_decimal,
                    threshold: self.min_tx_amount(),
                });
            }

            return Ok((amount, fee_u64));
        }

        let requested_amount = include_lamports_to_big_decimal(&req.amount, SOLANA_DECIMALS);

        // Amount must be bigger than min_tx_amount.
        if requested_amount < self.min_tx_amount() {
            return MmError::err(WithdrawError::AmountTooLow {
                amount: requested_amount,
                threshold: self.min_tx_amount(),
            });
        }

        let requested_amount_u64 = requested_amount.to_u64().ok_or_else(|| {
            MmError::new(WithdrawError::InternalError(format!(
                "Couldn't convert {requested_amount} to u64."
            )))
        })?;

        // User must have enough balance to cover both the send and fee amounts.
        if requested_amount_u64 + fee_u64 > balance_u64 {
            return MmError::err(WithdrawError::NotSufficientBalance {
                coin: self.ticker.to_owned(),
                available: u64_lamports_to_big_decimal(balance_u64, SOLANA_DECIMALS),
                required: u64_lamports_to_big_decimal(requested_amount_u64 + fee_u64, SOLANA_DECIMALS),
            });
        };

        Ok((requested_amount_u64, fee_u64))
    }
}

#[async_trait]
impl MmCoin for SolanaCoin {
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
        let coin = self.clone();
        let fut = async move {
            let to = SolanaAddress::from_str(&req.to).map_err(|e| WithdrawError::InvalidAddress(e.to_string()))?;

            let rpc = coin
                .rpc_client()
                .await
                .map_err(|e| WithdrawError::Transport(e.into_inner()))?;

            let (withdraw_lamports, fee_lamports) = coin.calculate_withdraw_and_fee_amount(&req).await?;

            if withdraw_lamports == 0 {
                return MmError::err(WithdrawError::AmountTooLow {
                    amount: req.amount,
                    threshold: coin.min_tx_amount(),
                });
            }

            let recent_blockhash = rpc
                .get_latest_blockhash()
                .await
                .map_err(|e| WithdrawError::Transport(e.to_string()))?;

            // Actual TX
            let tx = solana_system_transaction::transfer(&coin.keypair, &to, withdraw_lamports, recent_blockhash);

            let tx_hash = tx
                .signatures
                .first()
                .map(|s| s.to_string())
                .ok_or_else(|| WithdrawError::InternalError("Couldn't find the TX signature.".to_owned()))?;

            let tx_bytes =
                bincode::serialize(&tx).map_err(|e| MmError::new(WithdrawError::InternalError(e.to_string())))?;

            let tx_data = TransactionData::new_signed(BytesJson(tx_bytes), tx_hash.clone());

            let amount_dec = u64_lamports_to_big_decimal(withdraw_lamports, SOLANA_DECIMALS);

            let fee = u64_lamports_to_big_decimal(fee_lamports, SOLANA_DECIMALS);

            let received_by_me = if to == coin.address {
                amount_dec.clone()
            } else {
                BigDecimal::zero()
            };

            let spent_by_me = &amount_dec + &fee;

            Ok(TransactionDetails {
                tx: tx_data,
                from: vec![coin.address.to_string()],
                to: vec![to.to_string()],
                my_balance_change: &received_by_me - &spent_by_me,
                spent_by_me,
                total_amount: amount_dec,
                received_by_me,
                block_height: 0,
                timestamp: now_sec(),
                fee_details: Some(TxFeeDetails::Solana(SolanaFeeDetails {
                    fee_amount: fee.clone(),
                    rent_amount: 0.into(),
                    total_amount: fee,
                })),
                coin: req.coin,
                internal_id: BytesJson(tx_hash.into_bytes()),
                kmd_rewards: None,
                transaction_type: TransactionType::StandardTransfer,
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
        SOLANA_DECIMALS
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
impl MarketCoinOps for SolanaCoin {
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
        let coin = self.clone();

        let fut = async move {
            let rpc_client = coin
                .rpc_client()
                .map_err(|e| BalanceError::Internal(e.into_inner()))
                .await?;

            let balance_u64 = rpc_client
                .get_balance(&coin.address)
                .await
                .map_err(|e| BalanceError::Transport(e.to_string()))?;

            let balance_decimal = u64_lamports_to_big_decimal(balance_u64, SOLANA_DECIMALS);

            Ok(CoinBalance {
                spendable: balance_decimal,
                unspendable: BigDecimal::zero(),
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
        let bytes = try_fus!(hex::decode(tx));
        self.send_raw_tx_bytes(&bytes)
    }

    fn send_raw_tx_bytes(&self, tx: &[u8]) -> Box<dyn Future<Item = String, Error = String> + Send> {
        let coin = self.clone();
        let bytes = tx.to_vec();
        let fut = async move {
            let rpc = coin.rpc_client().await.map_err(|e| e.into_inner())?;

            let tx: Transaction = limited_deserialize(&bytes, PACKET_DATA_SIZE as u64).map_err(|e| e.to_string())?;
            let signature = rpc.send_transaction(&tx).await.map_err(|e| e.to_string())?;

            // TX hash is just the base58 `String` form of the `Signature`.
            // ref: https://solana.com/docs/references/terminology#transaction-id
            Ok(signature.to_string())
        };
        Box::new(fut.boxed().compat())
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
        let coin = self.clone();

        let fut = async move {
            let rpc_client = try_s!(coin.rpc_client().await);

            rpc_client.get_block_height().await.map_err(|e| e.to_string())
        };

        Box::new(fut.boxed().compat())
    }

    fn display_priv_key(&self) -> Result<String, String> {
        todo!()
    }

    #[inline]
    fn min_tx_amount(&self) -> BigDecimal {
        u64_lamports_to_big_decimal(1, SOLANA_DECIMALS)
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
impl SwapOps for SolanaCoin {
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
impl WatcherOps for SolanaCoin {}

pub(crate) fn u64_lamports_to_big_decimal<T: Into<u32>>(lamports: u64, decimals: T) -> BigDecimal {
    BigDecimal::from(lamports) / BigDecimal::from(10u64.pow(decimals.into()))
}

pub(crate) fn include_lamports_to_big_decimal<T: Into<u32>>(amount: &BigDecimal, decimals: T) -> BigDecimal {
    amount * &BigDecimal::from(10u64.pow(decimals.into()))
}
