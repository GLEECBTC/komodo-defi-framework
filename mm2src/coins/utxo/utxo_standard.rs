use super::utxo_common::utxo_prepare_addresses_for_balance_stream_if_enabled;
use super::*;
use crate::coin_balance::{
    self, EnableCoinBalanceError, EnabledCoinBalanceParams, HDAccountBalance, HDAddressBalance, HDWalletBalance,
    HDWalletBalanceOps,
};
use crate::coin_errors::{AddressFromPubkeyError, MyAddressError, ValidatePaymentResult};
use crate::hd_wallet::{
    ExtractExtendedPubkey, HDAddressSelector, HDCoinAddress, HDCoinWithdrawOps, HDConfirmAddress, HDExtractPubkeyError,
    HDXPubExtractor, SettingEnabledAddressError, TrezorCoinError, WithdrawSenderAddress,
};
use crate::my_tx_history_v2::{CoinWithTxHistoryV2, MyTxHistoryErrorV2, MyTxHistoryTarget, TxHistoryStorage};
use crate::rpc_command::account_balance::{self, AccountBalanceParams, AccountBalanceRpcOps, HDAccountBalanceResponse};
use crate::rpc_command::get_new_address::{
    self, GetNewAddressParams, GetNewAddressResponse, GetNewAddressRpcError, GetNewAddressRpcOps,
};
use crate::rpc_command::hd_account_balance_rpc_error::HDAccountBalanceRpcError;
use crate::rpc_command::init_account_balance::{self, InitAccountBalanceParams, InitAccountBalanceRpcOps};
use crate::rpc_command::init_create_account::{
    self, CreateAccountRpcError, CreateAccountState, CreateNewAccountParams, InitCreateAccountRpcOps,
};
use crate::rpc_command::init_scan_for_new_addresses::{
    self, InitScanAddressesRpcOps, ScanAddressesParams, ScanAddressesResponse,
};
use crate::rpc_command::init_withdraw::{InitWithdrawCoin, WithdrawTaskHandleShared};
use crate::tx_history_storage::{GetTxHistoryFilters, WalletId};
use crate::utxo::rpc_clients::BlockHashOrHeight;
use crate::utxo::utxo_builder::{UtxoArcBuilder, UtxoCoinBuilder};
use crate::utxo::utxo_hd_wallet::{UtxoHDAccount, UtxoHDAddress};
use crate::utxo::utxo_tx_history_v2::{
    UtxoMyAddressesHistoryError, UtxoTxDetailsError, UtxoTxDetailsParams, UtxoTxHistoryOps,
};
use crate::{
    CanRefundHtlc, CheckIfMyPaymentSentArgs, CoinBalance, CoinBalanceMap, CoinWithDerivationMethod,
    CoinWithPrivKeyPolicy, CommonSwapOpsV2, ConfirmPaymentInput, DexFee, FindPaymentSpendError, FundingTxSpend,
    GenPreimageResult, GenTakerFundingSpendArgs, GenTakerPaymentSpendArgs, GetWithdrawSenderAddress, IguanaBalanceOps,
    IguanaPrivKey, MakerCoinSwapOpsV2, MmCoinEnum, NegotiateSwapContractAddrErr, PrivKeyBuildPolicy,
    RawTransactionRequest, RawTransactionResult, RefundFundingSecretArgs, RefundMakerPaymentSecretArgs,
    RefundMakerPaymentTimelockArgs, RefundPaymentArgs, RefundTakerPaymentArgs, SearchForFundingSpendErr,
    SearchForSwapTxSpendInput, SendMakerPaymentArgs, SendMakerPaymentSpendPreimageInput, SendPaymentArgs,
    SendTakerFundingArgs, SignRawTransactionRequest, SignatureResult, SpendMakerPaymentArgs, SpendPaymentArgs, SwapOps,
    SwapTxTypeWithSecretHash, TakerCoinSwapOpsV2, ToBytes, TradePreimageValue, TransactionFut, TransactionResult,
    TxMarshalingErr, TxPreimageWithSig, ValidateAddressResult, ValidateFeeArgs, ValidateMakerPaymentArgs,
    ValidateOtherPubKeyErr, ValidatePaymentError, ValidatePaymentFut, ValidatePaymentInput, ValidateSwapV2TxResult,
    ValidateTakerFundingArgs, ValidateTakerFundingSpendPreimageResult, ValidateTakerPaymentSpendPreimageResult,
    ValidateWatcherSpendInput, VerificationResult, WaitForHTLCTxSpendArgs, WatcherOps, WatcherReward,
    WatcherRewardError, WatcherSearchForSwapTxSpendInput, WatcherValidatePaymentInput, WatcherValidateTakerFeeInput,
    WithdrawFut,
};
use bitcrypto::sign_message_hash;
use common::executor::{AbortableSystem, AbortedError};
use futures::{FutureExt, TryFutureExt};
use mm2_metrics::MetricsArc;
use mm2_number::MmNumber;
#[cfg(test)]
use mocktopus::macros::*;
use rpc::v1::types::H264 as H264Json;
use script::Opcode;
use utxo_signer::UtxoSignerOps;

#[derive(Clone)]
pub struct UtxoStandardCoin {
    utxo_arc: UtxoArc,
}

impl AsRef<UtxoCoinFields> for UtxoStandardCoin {
    fn as_ref(&self) -> &UtxoCoinFields {
        &self.utxo_arc
    }
}

impl From<UtxoArc> for UtxoStandardCoin {
    fn from(coin: UtxoArc) -> UtxoStandardCoin {
        UtxoStandardCoin { utxo_arc: coin }
    }
}

impl From<UtxoStandardCoin> for UtxoArc {
    fn from(coin: UtxoStandardCoin) -> Self {
        coin.utxo_arc
    }
}

pub async fn utxo_standard_coin_with_policy(
    ctx: &MmArc,
    ticker: &str,
    conf: &Json,
    activation_params: &UtxoActivationParams,
    priv_key_policy: PrivKeyBuildPolicy,
) -> Result<UtxoStandardCoin, String> {
    let coin = try_s!(
        UtxoArcBuilder::new(
            ctx,
            ticker,
            conf,
            activation_params,
            priv_key_policy,
            UtxoStandardCoin::from
        )
        .build()
        .await
    );

    Ok(coin)
}

pub async fn utxo_standard_coin_with_priv_key(
    ctx: &MmArc,
    ticker: &str,
    conf: &Json,
    activation_params: &UtxoActivationParams,
    priv_key: IguanaPrivKey,
) -> Result<UtxoStandardCoin, String> {
    let priv_key_policy = PrivKeyBuildPolicy::IguanaPrivKey(priv_key);
    utxo_standard_coin_with_policy(ctx, ticker, conf, activation_params, priv_key_policy).await
}

// if mockable is placed before async_trait there is `munmap_chunk(): invalid pointer` error on async fn mocking attempt
#[async_trait]
#[cfg_attr(test, mockable)]
impl UtxoTxBroadcastOps for UtxoStandardCoin {
    async fn broadcast_tx(&self, tx: &UtxoTx) -> Result<H256Json, MmError<BroadcastTxErr>> {
        utxo_common::broadcast_tx(self, tx).await
    }
}

// if mockable is placed before async_trait there is `munmap_chunk(): invalid pointer` error on async fn mocking attempt
#[async_trait]
#[cfg_attr(test, mockable)]
impl UtxoTxGenerationOps for UtxoStandardCoin {
    async fn get_fee_rate(&self) -> UtxoRpcResult<ActualFeeRate> {
        utxo_common::get_fee_rate(&self.utxo_arc).await
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
impl GetUtxoListOps for UtxoStandardCoin {
    async fn get_unspent_ordered_list(
        &self,
        address: &Address,
    ) -> UtxoRpcResult<(Vec<UnspentInfo>, RecentlySpentOutPointsGuard<'_>)> {
        utxo_common::get_unspent_ordered_list(self, address).await
    }

    async fn get_all_unspent_ordered_list(
        &self,
        address: &Address,
    ) -> UtxoRpcResult<(Vec<UnspentInfo>, RecentlySpentOutPointsGuard<'_>)> {
        utxo_common::get_all_unspent_ordered_list(self, address).await
    }

    async fn get_mature_unspent_ordered_list(
        &self,
        address: &Address,
    ) -> UtxoRpcResult<(MatureUnspentList, RecentlySpentOutPointsGuard<'_>)> {
        utxo_common::get_mature_unspent_ordered_list(self, address).await
    }
}

#[async_trait]
#[cfg_attr(test, mockable)]
impl GetUtxoMapOps for UtxoStandardCoin {
    async fn get_unspent_ordered_map(
        &self,
        addresses: Vec<Address>,
    ) -> UtxoRpcResult<(UnspentMap, RecentlySpentOutPointsGuard<'_>)> {
        utxo_common::get_unspent_ordered_map(self, addresses).await
    }

    async fn get_all_unspent_ordered_map(
        &self,
        addresses: Vec<Address>,
    ) -> UtxoRpcResult<(UnspentMap, RecentlySpentOutPointsGuard<'_>)> {
        utxo_common::get_all_unspent_ordered_map(self, addresses).await
    }

    async fn get_mature_unspent_ordered_map(
        &self,
        addresses: Vec<Address>,
    ) -> UtxoRpcResult<(MatureUnspentMap, RecentlySpentOutPointsGuard<'_>)> {
        utxo_common::get_mature_unspent_ordered_map(self, addresses).await
    }
}

// if mockable is placed before async_trait there is `munmap_chunk(): invalid pointer` error on async fn mocking attempt
#[async_trait]
#[cfg_attr(test, mockable)]
impl UtxoCommonOps for UtxoStandardCoin {
    async fn get_htlc_spend_fee(&self, tx_size: u64, stage: &FeeApproxStage) -> UtxoRpcResult<u64> {
        utxo_common::get_htlc_spend_fee(self, tx_size, stage).await
    }

    fn addresses_from_script(&self, script: &Script) -> Result<Vec<Address>, String> {
        utxo_common::addresses_from_script(self, script)
    }

    fn denominate_satoshis(&self, satoshi: i64) -> f64 {
        utxo_common::denominate_satoshis(&self.utxo_arc, satoshi)
    }

    fn my_public_key(&self) -> Result<&Public, MmError<UnexpectedDerivationMethod>> {
        utxo_common::my_public_key(self.as_ref())
    }

    fn address_from_str(&self, address: &str) -> MmResult<Address, AddrFromStrError> {
        utxo_common::checked_address_from_str(self, address)
    }

    fn script_for_address(&self, address: &Address) -> MmResult<Script, UnsupportedAddr> {
        utxo_common::output_script_checked(self.as_ref(), address)
    }

    async fn get_current_mtp(&self) -> UtxoRpcResult<u32> {
        utxo_common::get_current_mtp(&self.utxo_arc, self.ticker().into()).await
    }

    fn is_unspent_mature(&self, output: &RpcTransaction) -> bool {
        utxo_common::is_unspent_mature(self.utxo_arc.conf.mature_confirmations, output)
    }

    async fn calc_interest_of_tx(&self, tx: &UtxoTx, input_transactions: &mut HistoryUtxoTxMap) -> UtxoRpcResult<u64> {
        utxo_common::calc_interest_of_tx(self, tx, input_transactions).await
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
        let fut = async move { utxo_common::get_verbose_transactions_from_cache_or_rpc(&selfi.utxo_arc, tx_ids).await };
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
            self.ticker(),
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
        utxo_common::p2sh_tx_locktime(self, &self.utxo_arc.conf.ticker, htlc_locktime).await
    }

    fn addr_format(&self) -> &UtxoAddressFormat {
        utxo_common::addr_format(self)
    }

    fn addr_format_for_standard_scripts(&self) -> UtxoAddressFormat {
        utxo_common::addr_format_for_standard_scripts(self)
    }

    fn address_from_pubkey(&self, pubkey: &Public) -> Address {
        let conf = &self.utxo_arc.conf;
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
impl UtxoStandardOps for UtxoStandardCoin {
    async fn tx_details_by_hash(
        &self,
        hash: &H256Json,
        input_transactions: &mut HistoryUtxoTxMap,
    ) -> Result<TransactionDetails, String> {
        utxo_common::tx_details_by_hash(self, hash, input_transactions).await
    }

    async fn request_tx_history(&self, metrics: MetricsArc) -> RequestTxHistoryResult {
        utxo_common::request_tx_history(self, metrics).await
    }

    async fn update_kmd_rewards(
        &self,
        tx_details: &mut TransactionDetails,
        input_transactions: &mut HistoryUtxoTxMap,
    ) -> UtxoRpcResult<()> {
        utxo_common::update_kmd_rewards(self, tx_details, input_transactions).await
    }
}

#[async_trait]
#[cfg_attr(test, mockable)]
impl SwapOps for UtxoStandardCoin {
    #[inline]
    async fn send_taker_fee(&self, dex_fee: DexFee, _uuid: &[u8], _expire_at: u64) -> TransactionResult {
        utxo_common::send_taker_fee(self.clone(), dex_fee).compat().await
    }

    #[inline]
    async fn send_maker_payment(&self, maker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        utxo_common::send_maker_payment(self.clone(), maker_payment_args)
            .compat()
            .await
    }

    #[inline]
    async fn send_taker_payment(&self, taker_payment_args: SendPaymentArgs<'_>) -> TransactionResult {
        utxo_common::send_taker_payment(self.clone(), taker_payment_args)
            .compat()
            .await
    }

    #[inline]
    async fn send_maker_spends_taker_payment(
        &self,
        maker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        utxo_common::send_maker_spends_taker_payment(self.clone(), maker_spends_payment_args).await
    }

    #[inline]
    async fn send_taker_spends_maker_payment(
        &self,
        taker_spends_payment_args: SpendPaymentArgs<'_>,
    ) -> TransactionResult {
        utxo_common::send_taker_spends_maker_payment(self.clone(), taker_spends_payment_args).await
    }

    #[inline]
    async fn send_taker_refunds_payment(&self, taker_refunds_payment_args: RefundPaymentArgs<'_>) -> TransactionResult {
        utxo_common::send_taker_refunds_payment(self.clone(), taker_refunds_payment_args).await
    }

    #[inline]
    async fn send_maker_refunds_payment(&self, maker_refunds_payment_args: RefundPaymentArgs<'_>) -> TransactionResult {
        utxo_common::send_maker_refunds_payment(self.clone(), maker_refunds_payment_args).await
    }

    async fn validate_fee(&self, validate_fee_args: ValidateFeeArgs<'_>) -> ValidatePaymentResult<()> {
        let tx = match validate_fee_args.fee_tx {
            TransactionEnum::UtxoTx(tx) => tx.clone(),
            fee_tx => {
                return MmError::err(ValidatePaymentError::InternalError(format!(
                    "Invalid fee tx type. fee tx: {fee_tx:?}"
                )))
            },
        };
        utxo_common::validate_fee(
            self.clone(),
            tx,
            utxo_common::DEFAULT_FEE_VOUT,
            validate_fee_args.expected_sender,
            validate_fee_args.dex_fee.clone(),
            validate_fee_args.min_block_number,
        )
        .compat()
        .await
    }

    #[inline]
    async fn validate_maker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        utxo_common::validate_maker_payment(self, input).await
    }

    #[inline]
    async fn validate_taker_payment(&self, input: ValidatePaymentInput) -> ValidatePaymentResult<()> {
        utxo_common::validate_taker_payment(self, input).await
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
        utxo_common::check_if_my_payment_sent(
            self.clone(),
            time_lock,
            if_my_payment_sent_args.other_pub,
            if_my_payment_sent_args.secret_hash,
            if_my_payment_sent_args.swap_unique_data,
        )
        .compat()
        .await
    }

    #[inline]
    async fn search_for_swap_tx_spend_my(
        &self,
        input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        utxo_common::search_for_swap_tx_spend_my(self, input, utxo_common::DEFAULT_SWAP_VOUT).await
    }

    #[inline]
    async fn search_for_swap_tx_spend_other(
        &self,
        input: SearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        utxo_common::search_for_swap_tx_spend_other(self, input, utxo_common::DEFAULT_SWAP_VOUT).await
    }

    #[inline]
    async fn extract_secret(
        &self,
        secret_hash: &[u8],
        spend_tx: &[u8],
        _watcher_reward: bool,
    ) -> Result<[u8; 32], String> {
        utxo_common::extract_secret(secret_hash, spend_tx)
    }

    #[inline]
    async fn can_refund_htlc(&self, locktime: u64) -> Result<CanRefundHtlc, String> {
        utxo_common::can_refund_htlc(self, locktime)
            .await
            .map_err(|e| ERRL!("{}", e))
    }

    #[inline]
    fn negotiate_swap_contract_addr(
        &self,
        _other_side_address: Option<&[u8]>,
    ) -> Result<Option<BytesJson>, MmError<NegotiateSwapContractAddrErr>> {
        Ok(None)
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

    fn is_supported_by_watchers(&self) -> bool {
        true
    }
}

#[async_trait]
impl WatcherOps for UtxoStandardCoin {
    #[inline]
    fn create_taker_payment_refund_preimage(
        &self,
        taker_tx: &[u8],
        time_lock: u64,
        maker_pub: &[u8],
        secret_hash: &[u8],
        _swap_contract_address: &Option<BytesJson>,
        swap_unique_data: &[u8],
    ) -> TransactionFut {
        utxo_common::create_taker_payment_refund_preimage(
            self,
            taker_tx,
            try_tx_fus!(time_lock.try_into()),
            maker_pub,
            secret_hash,
            swap_unique_data,
        )
    }

    #[inline]
    fn create_maker_payment_spend_preimage(
        &self,
        maker_payment_tx: &[u8],
        time_lock: u64,
        maker_pub: &[u8],
        secret_hash: &[u8],
        swap_unique_data: &[u8],
    ) -> TransactionFut {
        utxo_common::create_maker_payment_spend_preimage(
            self,
            maker_payment_tx,
            try_tx_fus!(time_lock.try_into()),
            maker_pub,
            secret_hash,
            swap_unique_data,
        )
    }

    #[inline]
    fn send_taker_payment_refund_preimage(&self, refund_payment_args: RefundPaymentArgs) -> TransactionFut {
        utxo_common::send_taker_payment_refund_preimage(self, refund_payment_args)
    }

    #[inline]
    fn send_maker_payment_spend_preimage(&self, input: SendMakerPaymentSpendPreimageInput) -> TransactionFut {
        utxo_common::send_maker_payment_spend_preimage(self, input)
    }

    #[inline]
    fn watcher_validate_taker_fee(&self, input: WatcherValidateTakerFeeInput) -> ValidatePaymentFut<()> {
        utxo_common::watcher_validate_taker_fee(self, input, utxo_common::DEFAULT_FEE_VOUT)
    }

    #[inline]
    fn watcher_validate_taker_payment(&self, input: WatcherValidatePaymentInput) -> ValidatePaymentFut<()> {
        utxo_common::watcher_validate_taker_payment(self, input)
    }

    #[inline]
    fn taker_validates_payment_spend_or_refund(&self, input: ValidateWatcherSpendInput) -> ValidatePaymentFut<()> {
        utxo_common::validate_payment_spend_or_refund(self, input)
    }

    #[inline]
    async fn watcher_search_for_swap_tx_spend(
        &self,
        input: WatcherSearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        utxo_common::watcher_search_for_swap_tx_spend(self, input, utxo_common::DEFAULT_SWAP_VOUT).await
    }

    async fn get_taker_watcher_reward(
        &self,
        other_coin: &MmCoinEnum,
        coin_amount: Option<BigDecimal>,
        other_coin_amount: Option<BigDecimal>,
        reward_amount: Option<BigDecimal>,
        wait_until: u64,
    ) -> Result<WatcherReward, MmError<WatcherRewardError>> {
        utxo_common::get_taker_watcher_reward(
            self,
            other_coin,
            coin_amount,
            other_coin_amount,
            reward_amount,
            wait_until,
        )
        .await
    }

    async fn get_maker_watcher_reward(
        &self,
        _other_coin: &MmCoinEnum,
        _reward_amount: Option<BigDecimal>,
        _wait_until: u64,
    ) -> Result<Option<WatcherReward>, MmError<WatcherRewardError>> {
        Ok(None)
    }
}

impl ToBytes for Public {
    fn to_bytes(&self) -> Vec<u8> {
        self.to_vec()
    }
}

#[async_trait]
impl MakerCoinSwapOpsV2 for UtxoStandardCoin {
    async fn send_maker_payment_v2(&self, args: SendMakerPaymentArgs<'_, Self>) -> Result<Self::Tx, TransactionErr> {
        utxo_common::send_maker_payment_v2(self.clone(), args).await
    }

    async fn validate_maker_payment_v2(&self, args: ValidateMakerPaymentArgs<'_, Self>) -> ValidatePaymentResult<()> {
        let taker_pub = self.derive_htlc_pubkey_v2(args.swap_unique_data);
        let time_lock = args
            .time_lock
            .try_into()
            .map_to_mm(ValidatePaymentError::TimelockOverflow)?;
        utxo_common::validate_payment(
            self.clone(),
            args.maker_payment_tx,
            utxo_common::DEFAULT_SWAP_VOUT,
            args.maker_pub,
            &taker_pub,
            SwapTxTypeWithSecretHash::MakerPaymentV2 {
                maker_secret_hash: args.maker_secret_hash,
                taker_secret_hash: args.taker_secret_hash,
            },
            args.amount,
            None,
            time_lock,
            0,
            0,
        )
        .await
    }

    async fn refund_maker_payment_v2_timelock(
        &self,
        args: RefundMakerPaymentTimelockArgs<'_>,
    ) -> Result<Self::Tx, TransactionErr> {
        let args = RefundPaymentArgs {
            payment_tx: args.payment_tx,
            time_lock: args.time_lock,
            other_pubkey: args.taker_pub,
            tx_type_with_secret_hash: args.tx_type_with_secret_hash,
            swap_contract_address: &None,
            swap_unique_data: args.swap_unique_data,
            watcher_reward: args.watcher_reward,
        };
        utxo_common::refund_htlc_payment(self.clone(), args).await
    }

    async fn refund_maker_payment_v2_secret(
        &self,
        args: RefundMakerPaymentSecretArgs<'_, Self>,
    ) -> Result<Self::Tx, TransactionErr> {
        utxo_common::refund_maker_payment_v2_secret(self.clone(), args).await
    }

    async fn spend_maker_payment_v2(&self, args: SpendMakerPaymentArgs<'_, Self>) -> Result<Self::Tx, TransactionErr> {
        utxo_common::spend_maker_payment_v2(self, args).await
    }
}

#[async_trait]
impl TakerCoinSwapOpsV2 for UtxoStandardCoin {
    async fn send_taker_funding(&self, args: SendTakerFundingArgs<'_>) -> Result<Self::Tx, TransactionErr> {
        utxo_common::send_taker_funding(self.clone(), args).await
    }

    async fn validate_taker_funding(&self, args: ValidateTakerFundingArgs<'_, Self>) -> ValidateSwapV2TxResult {
        utxo_common::validate_taker_funding(self, args).await
    }

    async fn refund_taker_funding_timelock(
        &self,
        args: RefundTakerPaymentArgs<'_>,
    ) -> Result<Self::Tx, TransactionErr> {
        let args = RefundPaymentArgs {
            payment_tx: args.payment_tx,
            time_lock: args.time_lock,
            other_pubkey: args.maker_pub,
            tx_type_with_secret_hash: args.tx_type_with_secret_hash,
            swap_contract_address: &None,
            swap_unique_data: args.swap_unique_data,
            watcher_reward: args.watcher_reward,
        };
        utxo_common::refund_htlc_payment(self.clone(), args).await
    }

    async fn refund_taker_funding_secret(
        &self,
        args: RefundFundingSecretArgs<'_, Self>,
    ) -> Result<Self::Tx, TransactionErr> {
        utxo_common::refund_taker_funding_secret(self.clone(), args).await
    }

    async fn search_for_taker_funding_spend(
        &self,
        tx: &Self::Tx,
        from_block: u64,
        _secret_hash: &[u8],
    ) -> Result<Option<FundingTxSpend<Self>>, SearchForFundingSpendErr> {
        let script_pubkey = &tx
            .first_output()
            .map_err(|e| SearchForFundingSpendErr::InvalidInputTx(e.to_string()))?
            .script_pubkey;

        let from_block = from_block
            .try_into()
            .map_err(SearchForFundingSpendErr::FromBlockConversionErr)?;

        let output_spend = self
            .as_ref()
            .rpc_client
            .find_output_spend(
                tx.hash(),
                script_pubkey,
                utxo_common::DEFAULT_SWAP_VOUT,
                BlockHashOrHeight::Height(from_block),
                self.as_ref().tx_hash_algo,
            )
            .compat()
            .await
            .map_err(SearchForFundingSpendErr::Rpc)?;
        match output_spend {
            Some(found) => {
                let script_sig: Script = found.input.script_sig.into();
                let maybe_first_op_if = script_sig
                    .get_instruction(1)
                    .ok_or_else(|| {
                        SearchForFundingSpendErr::FailedToProcessSpendTx("No instruction at index 1".into())
                    })?
                    .map_err(|e| {
                        SearchForFundingSpendErr::FailedToProcessSpendTx(format!(
                            "Couldn't get instruction at index 1: {e}"
                        ))
                    })?;
                match maybe_first_op_if.opcode {
                    Opcode::OP_1 => Ok(Some(FundingTxSpend::RefundedTimelock(found.spending_tx))),
                    Opcode::OP_PUSHBYTES_32 => Ok(Some(FundingTxSpend::RefundedSecret {
                        tx: found.spending_tx,
                        secret: maybe_first_op_if
                            .data
                            .ok_or_else(|| {
                                SearchForFundingSpendErr::FailedToProcessSpendTx(
                                    "No data at instruction with index 1".into(),
                                )
                            })?
                            .try_into()
                            .map_err(|e| {
                                SearchForFundingSpendErr::FailedToProcessSpendTx(format!(
                                    "Failed to parse data at instruction with index 1 as [u8; 32]: {e}"
                                ))
                            })?,
                    })),
                    Opcode::OP_PUSHBYTES_70 | Opcode::OP_PUSHBYTES_71 | Opcode::OP_PUSHBYTES_72 => {
                        Ok(Some(FundingTxSpend::TransferredToTakerPayment(found.spending_tx)))
                    },
                    unexpected => Err(SearchForFundingSpendErr::FailedToProcessSpendTx(format!(
                        "Got unexpected opcode {unexpected:?} at instruction with index 1"
                    ))),
                }
            },
            None => Ok(None),
        }
    }

    async fn gen_taker_funding_spend_preimage(
        &self,
        args: &GenTakerFundingSpendArgs<'_, Self>,
        swap_unique_data: &[u8],
    ) -> GenPreimageResult<Self> {
        let htlc_keypair = self.derive_htlc_key_pair(swap_unique_data);
        utxo_common::gen_and_sign_taker_funding_spend_preimage(self, args, &htlc_keypair).await
    }

    async fn validate_taker_funding_spend_preimage(
        &self,
        gen_args: &GenTakerFundingSpendArgs<'_, Self>,
        preimage: &TxPreimageWithSig<Self>,
    ) -> ValidateTakerFundingSpendPreimageResult {
        utxo_common::validate_taker_funding_spend_preimage(self, gen_args, preimage).await
    }

    async fn sign_and_send_taker_funding_spend(
        &self,
        preimage: &TxPreimageWithSig<Self>,
        args: &GenTakerFundingSpendArgs<'_, Self>,
        swap_unique_data: &[u8],
    ) -> Result<Self::Tx, TransactionErr> {
        let htlc_keypair = self.derive_htlc_key_pair(swap_unique_data);
        utxo_common::sign_and_send_taker_funding_spend(self, preimage, args, &htlc_keypair).await
    }

    async fn refund_combined_taker_payment(
        &self,
        args: RefundTakerPaymentArgs<'_>,
    ) -> Result<Self::Tx, TransactionErr> {
        let args = RefundPaymentArgs {
            payment_tx: args.payment_tx,
            time_lock: args.time_lock,
            other_pubkey: args.maker_pub,
            tx_type_with_secret_hash: args.tx_type_with_secret_hash,
            swap_contract_address: &None,
            swap_unique_data: args.swap_unique_data,
            watcher_reward: args.watcher_reward,
        };
        utxo_common::refund_htlc_payment(self.clone(), args).await
    }

    async fn gen_taker_payment_spend_preimage(
        &self,
        args: &GenTakerPaymentSpendArgs<'_, Self>,
        swap_unique_data: &[u8],
    ) -> GenPreimageResult<Self> {
        let key_pair = self.derive_htlc_key_pair(swap_unique_data);
        utxo_common::gen_and_sign_taker_payment_spend_preimage(self, args, &key_pair).await
    }

    async fn validate_taker_payment_spend_preimage(
        &self,
        gen_args: &GenTakerPaymentSpendArgs<'_, Self>,
        preimage: &TxPreimageWithSig<Self>,
    ) -> ValidateTakerPaymentSpendPreimageResult {
        utxo_common::validate_taker_payment_spend_preimage(self, gen_args, preimage).await
    }

    async fn sign_and_broadcast_taker_payment_spend(
        &self,
        preimage: Option<&TxPreimageWithSig<Self>>,
        gen_args: &GenTakerPaymentSpendArgs<'_, Self>,
        secret: &[u8],
        swap_unique_data: &[u8],
    ) -> Result<Self::Tx, TransactionErr> {
        let preimage = preimage
            .ok_or_else(|| TransactionErr::Plain(ERRL!("taker_payment_spend_preimage must be Some for UTXO coin")))?;
        let htlc_keypair = self.derive_htlc_key_pair(swap_unique_data);
        utxo_common::sign_and_broadcast_taker_payment_spend(self, preimage, gen_args, secret, &htlc_keypair).await
    }

    async fn find_taker_payment_spend_tx(
        &self,
        taker_payment: &Self::Tx,
        from_block: u64,
        wait_until: u64,
    ) -> MmResult<Self::Tx, FindPaymentSpendError> {
        let res = utxo_common::wait_for_output_spend_impl(
            self.as_ref(),
            taker_payment,
            utxo_common::DEFAULT_SWAP_VOUT,
            from_block,
            wait_until,
            10.,
        )
        .await
        .map_mm_err()?;
        Ok(res)
    }

    async fn extract_secret_v2(&self, secret_hash: &[u8], spend_tx: &Self::Tx) -> Result<[u8; 32], String> {
        utxo_common::extract_secret_v2(secret_hash, spend_tx)
    }
}

impl CommonSwapOpsV2 for UtxoStandardCoin {
    fn derive_htlc_pubkey_v2(&self, swap_unique_data: &[u8]) -> Self::Pubkey {
        *self.derive_htlc_key_pair(swap_unique_data).public()
    }

    #[inline(always)]
    fn derive_htlc_pubkey_v2_bytes(&self, swap_unique_data: &[u8]) -> Vec<u8> {
        self.derive_htlc_pubkey_v2(swap_unique_data).to_bytes()
    }

    #[inline(always)]
    fn taker_pubkey_bytes(&self) -> Option<Vec<u8>> {
        Some(self.derive_htlc_pubkey_v2(&[]).to_bytes()) // unique_data not used for non-private coins
    }
}

#[async_trait]
impl MarketCoinOps for UtxoStandardCoin {
    fn ticker(&self) -> &str {
        &self.utxo_arc.conf.ticker
    }

    async fn get_public_key(&self) -> Result<String, MmError<UnexpectedDerivationMethod>> {
        let pubkey = utxo_common::my_public_key(&self.utxo_arc)?;
        Ok(pubkey.to_string())
    }

    fn my_address(&self) -> MmResult<String, MyAddressError> {
        utxo_common::my_address(self)
    }

    fn address_from_pubkey(&self, pubkey: &H264Json) -> MmResult<String, AddressFromPubkeyError> {
        let pubkey = Public::Compressed((*pubkey).into());
        Ok(UtxoCommonOps::address_from_pubkey(self, &pubkey).to_string())
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
        utxo_common::my_balance(self.clone())
    }

    fn platform_coin_balance(&self) -> BalanceFut<BigDecimal> {
        utxo_common::platform_coin_balance(self)
    }

    fn platform_ticker(&self) -> &str {
        self.ticker()
    }

    #[inline(always)]
    fn send_raw_tx(&self, tx: &str) -> Box<dyn Future<Item = String, Error = String> + Send> {
        utxo_common::send_raw_tx(&self.utxo_arc, tx)
    }

    #[inline(always)]
    fn send_raw_tx_bytes(&self, tx: &[u8]) -> Box<dyn Future<Item = String, Error = String> + Send> {
        utxo_common::send_raw_tx_bytes(&self.utxo_arc, tx)
    }

    #[inline(always)]
    async fn sign_raw_tx(&self, args: &SignRawTransactionRequest) -> RawTransactionResult {
        utxo_common::sign_raw_tx(self, args).await
    }

    fn wait_for_confirmations(&self, input: ConfirmPaymentInput) -> Box<dyn Future<Item = (), Error = String> + Send> {
        utxo_common::wait_for_confirmations(&self.utxo_arc, input)
    }

    async fn wait_for_htlc_tx_spend(&self, args: WaitForHTLCTxSpendArgs<'_>) -> TransactionResult {
        utxo_common::wait_for_output_spend(
            self.clone(),
            args.tx_bytes,
            utxo_common::DEFAULT_SWAP_VOUT,
            args.from_block,
            args.wait_until,
            args.check_every,
        )
        .await
    }

    fn tx_enum_from_bytes(&self, bytes: &[u8]) -> Result<TransactionEnum, MmError<TxMarshalingErr>> {
        utxo_common::tx_enum_from_bytes(self.as_ref(), bytes)
    }

    fn current_block(&self) -> Box<dyn Future<Item = u64, Error = String> + Send> {
        utxo_common::current_block(&self.utxo_arc)
    }

    fn display_priv_key(&self) -> Result<String, String> {
        utxo_common::display_priv_key(&self.utxo_arc)
    }

    fn min_tx_amount(&self) -> BigDecimal {
        utxo_common::min_tx_amount(self.as_ref())
    }

    fn min_trading_vol(&self) -> MmNumber {
        utxo_common::min_trading_vol(self.as_ref())
    }

    fn should_burn_directly(&self) -> bool {
        &self.utxo_arc.conf.ticker == "KMD"
    }

    fn should_burn_dex_fee(&self) -> bool {
        utxo_common::should_burn_dex_fee()
    }

    fn is_trezor(&self) -> bool {
        self.as_ref().priv_key_policy.is_trezor()
    }
}

#[async_trait]
impl MmCoin for UtxoStandardCoin {
    fn is_asset_chain(&self) -> bool {
        utxo_common::is_asset_chain(&self.utxo_arc)
    }

    fn spawner(&self) -> WeakSpawner {
        self.as_ref().abortable_system.weak_spawner()
    }

    fn get_raw_transaction(&self, req: RawTransactionRequest) -> RawTransactionFut<'_> {
        Box::new(utxo_common::get_raw_transaction(&self.utxo_arc, req).boxed().compat())
    }

    fn get_tx_hex_by_hash(&self, tx_hash: Vec<u8>) -> RawTransactionFut<'_> {
        Box::new(
            utxo_common::get_tx_hex_by_hash(&self.utxo_arc, tx_hash)
                .boxed()
                .compat(),
        )
    }

    fn withdraw(&self, req: WithdrawRequest) -> WithdrawFut {
        Box::new(utxo_common::withdraw(self.clone(), req).boxed().compat())
    }

    fn decimals(&self) -> u8 {
        utxo_common::decimals(&self.utxo_arc)
    }

    fn convert_to_address(&self, from: &str, to_address_format: Json) -> Result<String, String> {
        utxo_common::convert_to_address(self, from, to_address_format)
    }

    fn validate_address(&self, address: &str) -> ValidateAddressResult {
        utxo_common::validate_address(self, address)
    }

    fn process_history_loop(&self, ctx: MmArc) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        Box::new(
            utxo_common::process_history_loop(self.clone(), ctx)
                .map(|_| Ok(()))
                .boxed()
                .compat(),
        )
    }

    fn history_sync_status(&self) -> HistorySyncState {
        utxo_common::history_sync_status(&self.utxo_arc)
    }

    fn get_trade_fee(&self) -> Box<dyn Future<Item = TradeFee, Error = String> + Send> {
        utxo_common::get_trade_fee(self.clone())
    }

    async fn get_sender_trade_fee(
        &self,
        value: TradePreimageValue,
        stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        utxo_common::get_sender_trade_fee(self, value, stage).await
    }

    fn get_receiver_trade_fee(&self, _stage: FeeApproxStage) -> TradePreimageFut<TradeFee> {
        utxo_common::get_receiver_trade_fee(self.clone())
    }

    async fn get_fee_to_send_taker_fee(
        &self,
        dex_fee_amount: DexFee,
        stage: FeeApproxStage,
    ) -> TradePreimageResult<TradeFee> {
        utxo_common::get_fee_to_send_taker_fee(self, dex_fee_amount, stage).await
    }

    fn required_confirmations(&self) -> u64 {
        utxo_common::required_confirmations(&self.utxo_arc)
    }

    fn requires_notarization(&self) -> bool {
        utxo_common::requires_notarization(&self.utxo_arc)
    }

    fn set_required_confirmations(&self, confirmations: u64) {
        utxo_common::set_required_confirmations(&self.utxo_arc, confirmations)
    }

    fn set_requires_notarization(&self, requires_nota: bool) {
        utxo_common::set_requires_notarization(&self.utxo_arc, requires_nota)
    }

    fn swap_contract_address(&self) -> Option<BytesJson> {
        utxo_common::swap_contract_address()
    }

    fn fallback_swap_contract(&self) -> Option<BytesJson> {
        utxo_common::fallback_swap_contract()
    }

    fn mature_confirmations(&self) -> Option<u32> {
        Some(self.utxo_arc.conf.mature_confirmations)
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

#[async_trait]
impl GetWithdrawSenderAddress for UtxoStandardCoin {
    type Address = Address;
    type Pubkey = Public;

    async fn get_withdraw_sender_address(
        &self,
        req: &WithdrawRequest,
    ) -> MmResult<WithdrawSenderAddress<Self::Address, Self::Pubkey>, WithdrawError> {
        utxo_common::get_withdraw_from_address(self, req).await
    }
}

#[async_trait]
impl InitWithdrawCoin for UtxoStandardCoin {
    async fn init_withdraw(
        &self,
        ctx: MmArc,
        req: WithdrawRequest,
        task_handle: WithdrawTaskHandleShared,
    ) -> Result<TransactionDetails, MmError<WithdrawError>> {
        utxo_common::init_withdraw(ctx, self.clone(), req, task_handle).await
    }
}

impl UtxoSignerOps for UtxoStandardCoin {
    type TxGetter = UtxoRpcClientEnum;

    fn trezor_coin(&self) -> UtxoSignTxResult<String> {
        self.utxo_arc
            .conf
            .trezor_coin
            .clone()
            .or_mm_err(|| UtxoSignTxError::CoinNotSupportedWithTrezor {
                coin: self.utxo_arc.conf.ticker.clone(),
            })
    }

    fn fork_id(&self) -> u32 {
        self.utxo_arc.conf.fork_id
    }

    fn branch_id(&self) -> u32 {
        self.utxo_arc.conf.consensus_branch_id
    }

    fn tx_provider(&self) -> Self::TxGetter {
        self.utxo_arc.rpc_client.clone()
    }
}

impl CoinWithPrivKeyPolicy for UtxoStandardCoin {
    type KeyPair = KeyPair;

    fn priv_key_policy(&self) -> &PrivKeyPolicy<Self::KeyPair> {
        &self.utxo_arc.priv_key_policy
    }
}

impl CoinWithDerivationMethod for UtxoStandardCoin {
    fn derivation_method(&self) -> &DerivationMethod<HDCoinAddress<Self>, Self::HDWallet> {
        utxo_common::derivation_method(self.as_ref())
    }
}

#[async_trait]
impl IguanaBalanceOps for UtxoStandardCoin {
    type BalanceObject = CoinBalanceMap;

    async fn iguana_balances(&self) -> BalanceResult<Self::BalanceObject> {
        let balance = self.my_balance().compat().await?;
        Ok(HashMap::from([(self.ticker().to_string(), balance)]))
    }
}

#[async_trait]
impl ExtractExtendedPubkey for UtxoStandardCoin {
    type ExtendedPublicKey = Secp256k1ExtendedPublicKey;

    async fn extract_extended_pubkey<XPubExtractor>(
        &self,
        xpub_extractor: Option<XPubExtractor>,
        derivation_path: DerivationPath,
    ) -> MmResult<Self::ExtendedPublicKey, HDExtractPubkeyError>
    where
        XPubExtractor: HDXPubExtractor + Send,
    {
        crate::extract_extended_pubkey_impl(self, xpub_extractor, derivation_path).await
    }
}

#[async_trait]
impl HDWalletCoinOps for UtxoStandardCoin {
    type HDWallet = UtxoHDWallet;

    fn address_from_extended_pubkey(
        &self,
        extended_pubkey: &Secp256k1ExtendedPublicKey,
        derivation_path: DerivationPath,
    ) -> UtxoHDAddress {
        utxo_common::address_from_extended_pubkey(self, extended_pubkey, derivation_path)
    }

    fn trezor_coin(&self) -> MmResult<String, TrezorCoinError> {
        utxo_common::trezor_coin(self)
    }

    async fn received_enabled_address_from_hw_wallet(
        &self,
        enabled_address: UtxoHDAddress,
    ) -> MmResult<(), SettingEnabledAddressError> {
        utxo_common::received_enabled_address_from_hw_wallet(self, enabled_address.address)
            .await
            .mm_err(SettingEnabledAddressError::Internal)
    }
}

impl HDCoinWithdrawOps for UtxoStandardCoin {}

#[async_trait]
impl GetNewAddressRpcOps for UtxoStandardCoin {
    type BalanceObject = CoinBalanceMap;

    async fn get_new_address_rpc_without_conf(
        &self,
        params: GetNewAddressParams,
    ) -> MmResult<GetNewAddressResponse<Self::BalanceObject>, GetNewAddressRpcError> {
        get_new_address::common_impl::get_new_address_rpc_without_conf(self, params).await
    }

    async fn get_new_address_rpc<ConfirmAddress>(
        &self,
        params: GetNewAddressParams,
        confirm_address: &ConfirmAddress,
    ) -> MmResult<GetNewAddressResponse<Self::BalanceObject>, GetNewAddressRpcError>
    where
        ConfirmAddress: HDConfirmAddress,
    {
        get_new_address::common_impl::get_new_address_rpc(self, params, confirm_address).await
    }
}

#[async_trait]
impl HDWalletBalanceOps for UtxoStandardCoin {
    type HDAddressScanner = UtxoAddressScanner;
    type BalanceObject = CoinBalanceMap;

    async fn produce_hd_address_scanner(&self) -> BalanceResult<Self::HDAddressScanner> {
        utxo_common::produce_hd_address_scanner(self).await
    }

    async fn enable_hd_wallet<XPubExtractor>(
        &self,
        hd_wallet: &Self::HDWallet,
        xpub_extractor: Option<XPubExtractor>,
        params: EnabledCoinBalanceParams,
        path_to_address: &HDPathAccountToAddressId,
    ) -> MmResult<HDWalletBalance<Self::BalanceObject>, EnableCoinBalanceError>
    where
        XPubExtractor: HDXPubExtractor + Send,
    {
        coin_balance::common_impl::enable_hd_wallet(self, hd_wallet, xpub_extractor, params, path_to_address).await
    }

    async fn scan_for_new_addresses(
        &self,
        hd_wallet: &Self::HDWallet,
        hd_account: &mut UtxoHDAccount,
        address_scanner: &Self::HDAddressScanner,
        gap_limit: u32,
    ) -> BalanceResult<Vec<HDAddressBalance<Self::BalanceObject>>> {
        utxo_common::scan_for_new_addresses(self, hd_wallet, hd_account, address_scanner, gap_limit).await
    }

    async fn all_known_addresses_balances(
        &self,
        hd_account: &UtxoHDAccount,
    ) -> BalanceResult<Vec<HDAddressBalance<Self::BalanceObject>>> {
        utxo_common::all_known_addresses_balances(self, hd_account).await
    }

    async fn known_address_balance(&self, address: &Address) -> BalanceResult<Self::BalanceObject> {
        let balance = utxo_common::address_balance(self, address).await?;
        Ok(HashMap::from([(self.ticker().to_string(), balance)]))
    }

    async fn known_addresses_balances(
        &self,
        addresses: Vec<Address>,
    ) -> BalanceResult<Vec<(Address, Self::BalanceObject)>> {
        let ticker = self.ticker().to_string();
        let balances = utxo_common::addresses_balances(self, addresses).await?;

        balances
            .into_iter()
            .map(|(address, balance)| Ok((address, HashMap::from([(ticker.clone(), balance)]))))
            .collect()
    }

    async fn prepare_addresses_for_balance_stream_if_enabled(
        &self,
        addresses: HashSet<String>,
    ) -> MmResult<(), String> {
        utxo_prepare_addresses_for_balance_stream_if_enabled(self, addresses).await
    }
}

#[async_trait]
impl AccountBalanceRpcOps for UtxoStandardCoin {
    type BalanceObject = CoinBalanceMap;

    async fn account_balance_rpc(
        &self,
        params: AccountBalanceParams,
    ) -> MmResult<HDAccountBalanceResponse<Self::BalanceObject>, HDAccountBalanceRpcError> {
        account_balance::common_impl::account_balance_rpc(self, params).await
    }
}

#[async_trait]
impl InitAccountBalanceRpcOps for UtxoStandardCoin {
    type BalanceObject = CoinBalanceMap;

    async fn init_account_balance_rpc(
        &self,
        params: InitAccountBalanceParams,
    ) -> MmResult<HDAccountBalance<Self::BalanceObject>, HDAccountBalanceRpcError> {
        init_account_balance::common_impl::init_account_balance_rpc(self, params).await
    }
}

#[async_trait]
impl InitScanAddressesRpcOps for UtxoStandardCoin {
    type BalanceObject = CoinBalanceMap;

    async fn init_scan_for_new_addresses_rpc(
        &self,
        params: ScanAddressesParams,
    ) -> MmResult<ScanAddressesResponse<Self::BalanceObject>, HDAccountBalanceRpcError> {
        init_scan_for_new_addresses::common_impl::scan_for_new_addresses_rpc(self, params).await
    }
}

#[async_trait]
impl InitCreateAccountRpcOps for UtxoStandardCoin {
    type BalanceObject = CoinBalanceMap;

    async fn init_create_account_rpc<XPubExtractor>(
        &self,
        params: CreateNewAccountParams,
        state: CreateAccountState,
        xpub_extractor: Option<XPubExtractor>,
    ) -> MmResult<HDAccountBalance<Self::BalanceObject>, CreateAccountRpcError>
    where
        XPubExtractor: HDXPubExtractor + Send,
    {
        init_create_account::common_impl::init_create_new_account_rpc(self, params, state, xpub_extractor).await
    }

    async fn revert_creating_account(&self, account_id: u32) {
        init_create_account::common_impl::revert_creating_account(self, account_id).await
    }
}

#[async_trait]
impl CoinWithTxHistoryV2 for UtxoStandardCoin {
    fn history_wallet_id(&self) -> WalletId {
        utxo_common::utxo_tx_history_v2_common::history_wallet_id(self.as_ref())
    }

    async fn get_tx_history_filters(
        &self,
        target: MyTxHistoryTarget,
    ) -> MmResult<GetTxHistoryFilters, MyTxHistoryErrorV2> {
        utxo_common::utxo_tx_history_v2_common::get_tx_history_filters(self, target).await
    }
}

#[async_trait]
impl UtxoTxHistoryOps for UtxoStandardCoin {
    async fn my_addresses(&self) -> MmResult<HashSet<Address>, UtxoMyAddressesHistoryError> {
        let addresses = self.all_addresses().await.map_mm_err()?;
        Ok(addresses)
    }

    async fn tx_details_by_hash<Storage>(
        &self,
        params: UtxoTxDetailsParams<'_, Storage>,
    ) -> MmResult<Vec<TransactionDetails>, UtxoTxDetailsError>
    where
        Storage: TxHistoryStorage,
    {
        utxo_common::utxo_tx_history_v2_common::tx_details_by_hash(self, params).await
    }

    async fn tx_from_storage_or_rpc<Storage: TxHistoryStorage>(
        &self,
        tx_hash: &H256Json,
        storage: &Storage,
    ) -> MmResult<UtxoTx, UtxoTxDetailsError> {
        utxo_common::utxo_tx_history_v2_common::tx_from_storage_or_rpc(self, tx_hash, storage).await
    }

    async fn request_tx_history(
        &self,
        metrics: MetricsArc,
        for_addresses: &HashSet<Address>,
    ) -> RequestTxHistoryResult {
        utxo_common::utxo_tx_history_v2_common::request_tx_history(self, metrics, for_addresses).await
    }

    async fn get_block_timestamp(&self, height: u64) -> MmResult<u64, GetBlockHeaderError> {
        self.as_ref().rpc_client.get_block_timestamp(height).await
    }

    async fn my_addresses_balances(&self) -> BalanceResult<HashMap<String, BigDecimal>> {
        utxo_common::utxo_tx_history_v2_common::my_addresses_balances(self).await
    }

    fn address_from_str(&self, address: &str) -> MmResult<Address, AddrFromStrError> {
        utxo_common::checked_address_from_str(self, address)
    }

    fn set_history_sync_state(&self, new_state: HistorySyncState) {
        *self.as_ref().history_sync_state.lock().unwrap() = new_state;
    }
}
