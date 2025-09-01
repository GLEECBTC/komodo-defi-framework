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
use crate::utxo::utxo_builder::{MergeUtxoArcOps, UtxoCoinBuildError, UtxoCoinBuilder, UtxoCoinBuilderCommonOps};
use crate::utxo::utxo_hd_wallet::{UtxoHDAccount, UtxoHDAddress};
use crate::utxo::utxo_tx_history_v2::{
    UtxoMyAddressesHistoryError, UtxoTxDetailsError, UtxoTxDetailsParams, UtxoTxHistoryOps,
};
use crate::{
    eth, CanRefundHtlc, CheckIfMyPaymentSentArgs, CoinBalance, CoinBalanceMap, CoinWithDerivationMethod,
    CoinWithPrivKeyPolicy, ConfirmPaymentInput, DelegationError, DelegationFut, DexFee, GetWithdrawSenderAddress,
    IguanaBalanceOps, IguanaPrivKey, MmCoinEnum, NegotiateSwapContractAddrErr, PrivKeyBuildPolicy,
    RawTransactionRequest, RawTransactionResult, RefundPaymentArgs, SearchForSwapTxSpendInput,
    SendMakerPaymentSpendPreimageInput, SendPaymentArgs, SignRawTransactionRequest, SignatureResult, SpendPaymentArgs,
    StakingInfosFut, SwapOps, TradePreimageValue, TransactionFut, TransactionResult, TxMarshalingErr,
    UnexpectedDerivationMethod, ValidateAddressResult, ValidateFeeArgs, ValidateOtherPubKeyErr, ValidatePaymentError,
    ValidatePaymentFut, ValidatePaymentInput, ValidateWatcherSpendInput, VerificationResult, WaitForHTLCTxSpendArgs,
    WatcherOps, WatcherReward, WatcherRewardError, WatcherSearchForSwapTxSpendInput, WatcherValidatePaymentInput,
    WatcherValidateTakerFeeInput, WithdrawFut,
};
use bitcrypto::sign_message_hash;
use common::executor::{AbortableSystem, AbortedError};
use ethereum_types::H160;
use futures::{FutureExt, TryFutureExt};
use keys::AddressHashEnum;
use mm2_metrics::MetricsArc;
use mm2_number::MmNumber;
use rpc::v1::types::H264 as H264Json;
use serde::Serialize;
use serialization::CoinVariant;
use utxo_signer::UtxoSignerOps;

#[derive(Debug, Display)]
pub enum Qrc20AddressError {
    UnexpectedDerivationMethod(String),
    ScriptHashTypeNotSupported { script_hash_type: String },
}

impl From<UnexpectedDerivationMethod> for Qrc20AddressError {
    fn from(e: UnexpectedDerivationMethod) -> Self {
        Qrc20AddressError::UnexpectedDerivationMethod(e.to_string())
    }
}

impl From<ScriptHashTypeNotSupported> for Qrc20AddressError {
    fn from(e: ScriptHashTypeNotSupported) -> Self {
        Qrc20AddressError::ScriptHashTypeNotSupported {
            script_hash_type: e.script_hash_type,
        }
    }
}

#[derive(Debug, Display)]
pub struct ScriptHashTypeNotSupported {
    pub script_hash_type: String,
}

impl From<ScriptHashTypeNotSupported> for WithdrawError {
    fn from(e: ScriptHashTypeNotSupported) -> Self {
        WithdrawError::InvalidAddress(e.to_string())
    }
}

#[path = "qtum_delegation.rs"]
mod qtum_delegation;
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "format")]
pub enum QtumAddressFormat {
    /// Standard Qtum/UTXO address format.
    #[serde(rename = "wallet")]
    Wallet,
    /// Contract address format. The same as used in ETH/ERC20.
    /// Note starts with "0x" prefix.
    #[serde(rename = "contract")]
    Contract,
}

pub trait QtumDelegationOps {
    fn add_delegation(&self, request: QtumDelegationRequest) -> DelegationFut;

    fn get_delegation_infos(&self) -> StakingInfosFut;

    fn remove_delegation(&self) -> DelegationFut;

    #[allow(clippy::result_large_err)]
    fn generate_pod(&self, addr_hash: AddressHashEnum) -> Result<keys::Signature, MmError<DelegationError>>;
}

#[async_trait]
pub trait QtumBasedCoin: UtxoCommonOps + MarketCoinOps {
    fn convert_to_address(&self, from: &str, to_address_format: Json) -> Result<String, String> {
        let to_address_format: QtumAddressFormat =
            json::from_value(to_address_format).map_err(|e| ERRL!("Error on parse Qtum address format {:?}", e))?;
        let from_address = try_s!(self.utxo_address_from_any_format(from));
        match to_address_format {
            QtumAddressFormat::Wallet => Ok(from_address.to_string()),
            QtumAddressFormat::Contract => Ok(try_s!(display_as_contract_address(from_address))),
        }
    }

    /// Try to parse address from either wallet (UTXO) format or contract format.
    fn utxo_address_from_any_format(&self, from: &str) -> Result<Address, String> {
        let utxo_err = match Address::from_legacyaddress(from, &self.as_ref().conf.address_prefixes) {
            Ok(addr) => {
                if addr.is_pubkey_hash() {
                    return Ok(addr);
                }
                "Address has invalid prefix".to_string()
            },
            Err(e) => e,
        };
        let utxo_segwit_err = match Address::from_segwitaddress(from, self.as_ref().conf.checksum_type) {
            Ok(addr) => {
                let is_segwit =
                    addr.hrp().is_some() && addr.hrp() == &self.as_ref().conf.bech32_hrp && self.as_ref().conf.segwit;
                if is_segwit {
                    return Ok(addr);
                }
                "Address has invalid hrp".to_string()
            },
            Err(e) => e,
        };
        let contract_err = match contract_addr_from_str(from) {
            Ok(contract_addr) => return Ok(self.utxo_addr_from_contract_addr(contract_addr)),
            Err(e) => e,
        };
        ERR!(
            "error on parse wallet address: {:?}, {:?}, error on parse contract address: {:?}",
            utxo_err,
            utxo_segwit_err,
            contract_err,
        )
    }

    fn utxo_addr_from_contract_addr(&self, address: H160) -> Address {
        let utxo = self.as_ref();
        AddressBuilder::new(
            self.addr_format().clone(),
            utxo.conf.checksum_type,
            utxo.conf.address_prefixes.clone(),
            utxo.conf.bech32_hrp.clone(),
        )
        .as_pkh(AddressHashEnum::AddressHash(address.0.into()))
        .build()
        .expect("valid address props")
    }

    async fn my_addr_as_contract_addr(&self) -> MmResult<H160, Qrc20AddressError> {
        let my_address = self
            .as_ref()
            .derivation_method
            .single_addr_or_err()
            .await
            .map_mm_err()?;
        contract_addr_from_utxo_addr(my_address).mm_err(Qrc20AddressError::from)
    }

    fn contract_address_from_raw_pubkey(&self, pubkey: &[u8]) -> Result<H160, String> {
        let utxo = self.as_ref();
        let qtum_address = try_s!(utxo_common::address_from_raw_pubkey(
            pubkey,
            utxo.conf.address_prefixes.clone(),
            utxo.conf.checksum_type,
            utxo.conf.bech32_hrp.clone(),
            self.addr_format().clone()
        ));
        let contract_addr = try_s!(contract_addr_from_utxo_addr(qtum_address));
        Ok(contract_addr)
    }

    fn is_qtum_unspent_mature(&self, output: &RpcTransaction) -> bool {
        let is_qrc20_coinbase = output.vout.iter().any(|x| x.is_empty());
        let is_coinbase = output.is_coinbase() || is_qrc20_coinbase;
        !is_coinbase || output.confirmations >= self.as_ref().conf.mature_confirmations
    }
}

pub struct QtumCoinBuilder<'a> {
    ctx: &'a MmArc,
    ticker: &'a str,
    conf: &'a Json,
    activation_params: &'a UtxoActivationParams,
    priv_key_policy: PrivKeyBuildPolicy,
}

#[async_trait]
impl UtxoCoinBuilderCommonOps for QtumCoinBuilder<'_> {
    fn ctx(&self) -> &MmArc {
        self.ctx
    }

    fn conf(&self) -> &Json {
        self.conf
    }

    fn activation_params(&self) -> &UtxoActivationParams {
        self.activation_params
    }

    fn ticker(&self) -> &str {
        self.ticker
    }

    fn check_utxo_maturity(&self) -> bool {
        self.activation_params().check_utxo_maturity.unwrap_or(true)
    }
}

#[async_trait]
impl UtxoCoinBuilder for QtumCoinBuilder<'_> {
    type ResultCoin = QtumCoin;
    type Error = UtxoCoinBuildError;

    fn priv_key_policy(&self) -> PrivKeyBuildPolicy {
        self.priv_key_policy.clone()
    }

    async fn build(self) -> MmResult<Self::ResultCoin, Self::Error> {
        let utxo = self.build_utxo_fields().await?;
        let utxo_arc = UtxoArc::new(utxo);

        self.spawn_merge_utxo_loop_if_required(&utxo_arc, QtumCoin::from);
        Ok(QtumCoin::from(utxo_arc))
    }
}

impl MergeUtxoArcOps<QtumCoin> for QtumCoinBuilder<'_> {}

impl<'a> QtumCoinBuilder<'a> {
    pub fn new(
        ctx: &'a MmArc,
        ticker: &'a str,
        conf: &'a Json,
        activation_params: &'a UtxoActivationParams,
        priv_key_policy: PrivKeyBuildPolicy,
    ) -> Self {
        QtumCoinBuilder {
            ctx,
            ticker,
            conf,
            activation_params,
            priv_key_policy,
        }
    }
}

#[derive(Clone)]
pub struct QtumCoin {
    utxo_arc: UtxoArc,
}

impl AsRef<UtxoCoinFields> for QtumCoin {
    fn as_ref(&self) -> &UtxoCoinFields {
        &self.utxo_arc
    }
}

impl From<UtxoArc> for QtumCoin {
    fn from(coin: UtxoArc) -> QtumCoin {
        QtumCoin { utxo_arc: coin }
    }
}

impl From<QtumCoin> for UtxoArc {
    fn from(coin: QtumCoin) -> Self {
        coin.utxo_arc
    }
}

pub async fn qtum_coin_with_policy(
    ctx: &MmArc,
    ticker: &str,
    conf: &Json,
    activation_params: &UtxoActivationParams,
    priv_key_policy: PrivKeyBuildPolicy,
) -> Result<QtumCoin, String> {
    if conf["coin"].as_str() != Some(ticker) {
        return ERR!("Failed to activate '{}': ticker does not match coins config", ticker);
    }
    let coin = try_s!(
        QtumCoinBuilder::new(ctx, ticker, conf, activation_params, priv_key_policy)
            .build()
            .await
    );
    Ok(coin)
}

pub async fn qtum_coin_with_priv_key(
    ctx: &MmArc,
    ticker: &str,
    conf: &Json,
    activation_params: &UtxoActivationParams,
    priv_key: IguanaPrivKey,
) -> Result<QtumCoin, String> {
    let priv_key_policy = PrivKeyBuildPolicy::IguanaPrivKey(priv_key);
    qtum_coin_with_policy(ctx, ticker, conf, activation_params, priv_key_policy).await
}

impl QtumBasedCoin for QtumCoin {}

#[derive(Clone, Debug, Deserialize)]
pub struct QtumDelegationRequest {
    pub validator_address: String,
    pub fee: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QtumStakingInfosDetails {
    pub amount: BigDecimal,
    pub staker: Option<String>,
    pub am_i_staking: bool,
    pub is_staking_supported: bool,
}

// if mockable is placed before async_trait there is `munmap_chunk(): invalid pointer` error on async fn mocking attempt
#[async_trait]
#[cfg_attr(test, mockable)]
impl UtxoTxBroadcastOps for QtumCoin {
    async fn broadcast_tx(&self, tx: &UtxoTx) -> Result<H256Json, MmError<BroadcastTxErr>> {
        utxo_common::broadcast_tx(self, tx).await
    }
}

#[async_trait]
#[cfg_attr(test, mockable)]
impl UtxoTxGenerationOps for QtumCoin {
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
impl GetUtxoListOps for QtumCoin {
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
impl GetUtxoMapOps for QtumCoin {
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

#[async_trait]
#[cfg_attr(test, mockable)]
impl UtxoCommonOps for QtumCoin {
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
        utxo_common::get_current_mtp(&self.utxo_arc, CoinVariant::Qtum).await
    }

    fn is_unspent_mature(&self, output: &RpcTransaction) -> bool {
        self.is_qtum_unspent_mature(output)
    }

    async fn calc_interest_of_tx(
        &self,
        _tx: &UtxoTx,
        _input_transactions: &mut HistoryUtxoTxMap,
    ) -> UtxoRpcResult<u64> {
        MmError::err(UtxoRpcError::Internal(
            "QTUM coin doesn't support transaction rewards".to_owned(),
        ))
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
impl UtxoStandardOps for QtumCoin {
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
impl SwapOps for QtumCoin {
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
impl WatcherOps for QtumCoin {
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
    fn send_maker_payment_spend_preimage(&self, input: SendMakerPaymentSpendPreimageInput) -> TransactionFut {
        utxo_common::send_maker_payment_spend_preimage(self, input)
    }

    #[inline]
    fn create_taker_payment_refund_preimage(
        &self,
        taker_payment_tx: &[u8],
        time_lock: u64,
        maker_pub: &[u8],
        secret_hash: &[u8],
        _swap_contract_address: &Option<BytesJson>,
        swap_unique_data: &[u8],
    ) -> TransactionFut {
        utxo_common::create_taker_payment_refund_preimage(
            self,
            taker_payment_tx,
            try_tx_fus!(time_lock.try_into()),
            maker_pub,
            secret_hash,
            swap_unique_data,
        )
    }

    #[inline]
    fn send_taker_payment_refund_preimage(&self, watcher_refunds_payment_args: RefundPaymentArgs) -> TransactionFut {
        utxo_common::send_taker_payment_refund_preimage(self, watcher_refunds_payment_args)
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

    async fn watcher_search_for_swap_tx_spend(
        &self,
        input: WatcherSearchForSwapTxSpendInput<'_>,
    ) -> Result<Option<FoundSwapTxSpend>, String> {
        utxo_common::watcher_search_for_swap_tx_spend(self, input, utxo_common::DEFAULT_SWAP_VOUT).await
    }

    async fn get_taker_watcher_reward(
        &self,
        _other_coin: &MmCoinEnum,
        _coin_amount: Option<BigDecimal>,
        _other_coin_amount: Option<BigDecimal>,
        _reward_amount: Option<BigDecimal>,
        _wait_until: u64,
    ) -> Result<WatcherReward, MmError<WatcherRewardError>> {
        unimplemented!()
    }

    async fn get_maker_watcher_reward(
        &self,
        _other_coin: &MmCoinEnum,
        _reward_amount: Option<BigDecimal>,
        _wait_until: u64,
    ) -> Result<Option<WatcherReward>, MmError<WatcherRewardError>> {
        unimplemented!()
    }
}

#[async_trait]
impl MarketCoinOps for QtumCoin {
    fn ticker(&self) -> &str {
        &self.utxo_arc.conf.ticker
    }

    fn my_address(&self) -> MmResult<String, MyAddressError> {
        utxo_common::my_address(self)
    }

    fn address_from_pubkey(&self, pubkey: &H264Json) -> MmResult<String, AddressFromPubkeyError> {
        let pubkey = Public::Compressed((*pubkey).into());
        Ok(UtxoCommonOps::address_from_pubkey(self, &pubkey).to_string())
    }

    async fn get_public_key(&self) -> Result<String, MmError<UnexpectedDerivationMethod>> {
        let pubkey = utxo_common::my_public_key(&self.utxo_arc)?;
        Ok(pubkey.to_string())
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

    fn is_trezor(&self) -> bool {
        self.as_ref().priv_key_policy.is_trezor()
    }

    fn should_burn_dex_fee(&self) -> bool {
        utxo_common::should_burn_dex_fee()
    }
}

#[async_trait]
impl MmCoin for QtumCoin {
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

    /// Check if the `to_address_format` is standard and if the `from` address is standard UTXO address.
    fn convert_to_address(&self, from: &str, to_address_format: Json) -> Result<String, String> {
        QtumBasedCoin::convert_to_address(self, from, to_address_format)
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
impl GetWithdrawSenderAddress for QtumCoin {
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
impl InitWithdrawCoin for QtumCoin {
    async fn init_withdraw(
        &self,
        ctx: MmArc,
        req: WithdrawRequest,
        task_handle: WithdrawTaskHandleShared,
    ) -> Result<TransactionDetails, MmError<WithdrawError>> {
        utxo_common::init_withdraw(ctx, self.clone(), req, task_handle).await
    }
}

impl UtxoSignerOps for QtumCoin {
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

impl CoinWithPrivKeyPolicy for QtumCoin {
    type KeyPair = KeyPair;

    fn priv_key_policy(&self) -> &PrivKeyPolicy<Self::KeyPair> {
        &self.utxo_arc.priv_key_policy
    }
}

impl CoinWithDerivationMethod for QtumCoin {
    fn derivation_method(&self) -> &DerivationMethod<HDCoinAddress<Self>, Self::HDWallet> {
        utxo_common::derivation_method(self.as_ref())
    }
}

#[async_trait]
impl IguanaBalanceOps for QtumCoin {
    type BalanceObject = CoinBalanceMap;

    async fn iguana_balances(&self) -> BalanceResult<Self::BalanceObject> {
        let balance = self.my_balance().compat().await?;
        Ok(HashMap::from([(self.ticker().to_string(), balance)]))
    }
}

#[async_trait]
impl ExtractExtendedPubkey for QtumCoin {
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
impl HDWalletCoinOps for QtumCoin {
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

impl HDCoinWithdrawOps for QtumCoin {}

#[async_trait]
impl HDWalletBalanceOps for QtumCoin {
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
impl GetNewAddressRpcOps for QtumCoin {
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
impl AccountBalanceRpcOps for QtumCoin {
    type BalanceObject = CoinBalanceMap;

    async fn account_balance_rpc(
        &self,
        params: AccountBalanceParams,
    ) -> MmResult<HDAccountBalanceResponse<Self::BalanceObject>, HDAccountBalanceRpcError> {
        account_balance::common_impl::account_balance_rpc(self, params).await
    }
}

#[async_trait]
impl InitAccountBalanceRpcOps for QtumCoin {
    type BalanceObject = CoinBalanceMap;

    async fn init_account_balance_rpc(
        &self,
        params: InitAccountBalanceParams,
    ) -> MmResult<HDAccountBalance<Self::BalanceObject>, HDAccountBalanceRpcError> {
        init_account_balance::common_impl::init_account_balance_rpc(self, params).await
    }
}

#[async_trait]
impl InitScanAddressesRpcOps for QtumCoin {
    type BalanceObject = CoinBalanceMap;

    async fn init_scan_for_new_addresses_rpc(
        &self,
        params: ScanAddressesParams,
    ) -> MmResult<ScanAddressesResponse<Self::BalanceObject>, HDAccountBalanceRpcError> {
        init_scan_for_new_addresses::common_impl::scan_for_new_addresses_rpc(self, params).await
    }
}

#[async_trait]
impl InitCreateAccountRpcOps for QtumCoin {
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
impl CoinWithTxHistoryV2 for QtumCoin {
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
impl UtxoTxHistoryOps for QtumCoin {
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

/// Parse contract address (H160) from string.
/// Qtum Contract addresses have another checksum verification algorithm, because of this do not use [`eth::valid_addr_from_str`].
pub fn contract_addr_from_str(addr: &str) -> Result<H160, String> {
    eth::addr_from_str(addr)
}

pub fn contract_addr_from_utxo_addr(address: Address) -> MmResult<H160, ScriptHashTypeNotSupported> {
    match address.hash() {
        AddressHashEnum::AddressHash(h) => Ok(h.take().into()),
        AddressHashEnum::WitnessScriptHash(_) => MmError::err(ScriptHashTypeNotSupported {
            script_hash_type: "Witness".to_owned(),
        }),
    }
}

pub fn display_as_contract_address(address: Address) -> MmResult<String, ScriptHashTypeNotSupported> {
    let address = qtum::contract_addr_from_utxo_addr(address)?;
    Ok(format!("{address:#02x}"))
}
