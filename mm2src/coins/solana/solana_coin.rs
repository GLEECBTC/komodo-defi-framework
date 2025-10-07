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
use derive_more::Display;
use futures::lock::Mutex as AsyncMutex;
use futures::{FutureExt, TryFutureExt};
use futures01::Future;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::{BigDecimal, MmNumber};
use nom::AsBytes;
use num_traits::Zero;
use parking_lot::Mutex as PaMutex;
use rpc::v1::types::{Bytes as RpcBytes, H264 as RpcH264};
use solana_keypair::keypair_from_seed;
use solana_pubkey::Pubkey as SolanaAddress;
use solana_rpc_client::rpc_client::RpcClient;
use solana_rpc_client_types::request::TokenAccountsFilter;
use solana_signer::Signer;
use url::Url;

use crate::{
    coin_errors::{AddressFromPubkeyError, MyAddressError, ValidatePaymentResult},
    hd_wallet::HDAddressSelector,
    BalanceError, BalanceFut, CheckIfMyPaymentSentArgs, CoinBalance, ConfirmPaymentInput, DexFee, FeeApproxStage,
    FoundSwapTxSpend, HistorySyncState, MarketCoinOps, MmCoin, NegotiateSwapContractAddrErr, PrivKeyBuildPolicy,
    RawTransactionFut, RawTransactionRequest, RawTransactionResult, RefundPaymentArgs, SearchForSwapTxSpendInput,
    SendPaymentArgs, SignRawTransactionRequest, SignatureResult, SpendPaymentArgs, SwapOps, TradeFee, TradePreimageFut,
    TradePreimageResult, TradePreimageValue, TransactionEnum, TransactionResult, TxMarshalingErr,
    UnexpectedDerivationMethod, ValidateAddressResult, ValidateFeeArgs, ValidateOtherPubKeyErr, ValidatePaymentInput,
    VerificationResult, WaitForHTLCTxSpendArgs, WatcherOps, WithdrawFut, WithdrawRequest,
};

pub const SOLANA_DECIMALS: u8 = 9;

#[derive(Clone, Deserialize)]
pub struct RpcNode {
    url: Url,
}

#[derive(Clone)]
pub struct SolanaCoin(Arc<SolanaCoinFields>);

pub struct SolanaCoinFields {
    ticker: String,
    pub(crate) address: SolanaAddress,
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

        let rpc_clients: Vec<Arc<RpcClient>> = nodes.iter().map(|n| Arc::new(RpcClient::new(&n.url))).collect();

        let abortable_system = ctx.abortable_system.create_subsystem().map_to_mm(|e| SolanaInitError {
            ticker: ticker.clone(),
            kind: SolanaInitErrorKind::Internal { reason: e.to_string() },
        })?;

        let fields = SolanaCoinFields {
            ticker,
            address,
            abortable_system,
            rpc_clients: AsyncMutex::new(rpc_clients),
            protocol_info,
            tokens_info: PaMutex::new(HashMap::new()),
        };

        Ok(SolanaCoin(Arc::new(fields)))
    }

    pub(crate) async fn rpc_client(&self) -> MmResult<Arc<RpcClient>, String> {
        let mut rpcs = self.rpc_clients.lock().await;

        if let Some(index) = rpcs.iter().position(|rpc| rpc.get_health().is_ok()) {
            // Put healthy one to the front.
            rpcs.rotate_left(index);

            return Ok(rpcs[0].clone());
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

        if let Err(e) = rpc.get_token_accounts_by_owner(&self.address, TokenAccountsFilter::Mint(*mint_address)) {
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
            .map_err(|e| BalanceError::Transport(e.to_string()))?
            .ui_amount_string;

        let balance = BigDecimal::from_str(&balance_string).map_err(|e| BalanceError::Internal(e.to_string()))?;

        Ok(CoinBalance {
            spendable: balance,
            unspendable: Default::default(),
        })
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
        todo!()
    }

    fn withdraw(&self, req: WithdrawRequest) -> WithdrawFut {
        todo!()
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
                .map_err(|e| BalanceError::Transport(e.to_string()))?;

            let scale = BigDecimal::from(10u64.pow(SOLANA_DECIMALS as u32));
            let balance_decimal = BigDecimal::from(balance_u64) / scale;

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
        todo!()
    }

    fn send_raw_tx_bytes(&self, tx: &[u8]) -> Box<dyn Future<Item = String, Error = String> + Send> {
        todo!()
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

            rpc_client.get_block_height().map_err(|e| e.to_string())
        };

        Box::new(fut.boxed().compat())
    }

    fn display_priv_key(&self) -> Result<String, String> {
        todo!()
    }

    #[inline]
    fn min_tx_amount(&self) -> BigDecimal {
        todo!()
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
