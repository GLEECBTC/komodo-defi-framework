use super::*;
use crate::coin_balance::{EnableCoinBalanceError, HDAddressBalance, HDWalletBalance, HDWalletBalanceOps};
use crate::coin_errors::{AddressFromPubkeyError, MyAddressError, ValidatePaymentResult};
use crate::hd_wallet::{
    ExtractExtendedPubkey, HDAddressSelector, HDCoinAddress, HDCoinWithdrawOps, HDExtractPubkeyError, HDXPubExtractor,
    SettingEnabledAddressError, TrezorCoinError, WithdrawSenderAddress,
};
use crate::my_tx_history_v2::{
    CoinWithTxHistoryV2, MyTxHistoryErrorV2, MyTxHistoryTarget, TxDetailsBuilder, TxHistoryStorage,
};
use crate::tx_history_storage::{GetTxHistoryFilters, WalletId};
use crate::utxo::rpc_clients::UtxoRpcFut;
use crate::utxo::slp::{parse_slp_script, SlpGenesisParams, SlpTokenInfo, SlpTransaction, SlpUnspent};
use crate::utxo::utxo_builder::{UtxoArcBuilder, UtxoCoinBuilder};
use crate::utxo::utxo_common::{big_decimal_from_sat_unsigned, utxo_prepare_addresses_for_balance_stream_if_enabled};
use crate::utxo::utxo_hd_wallet::{UtxoHDAccount, UtxoHDAddress};
use crate::utxo::utxo_tx_history_v2::{
    UtxoMyAddressesHistoryError, UtxoTxDetailsError, UtxoTxDetailsParams, UtxoTxHistoryOps,
};
use crate::{
    coin_balance, BlockHeightAndTime, CanRefundHtlc, CheckIfMyPaymentSentArgs, CoinBalance, CoinBalanceMap,
    CoinProtocol, CoinWithDerivationMethod, CoinWithPrivKeyPolicy, ConfirmPaymentInput, DexFee,
    GetWithdrawSenderAddress, IguanaBalanceOps, IguanaPrivKey, MmCoinEnum, NegotiateSwapContractAddrErr,
    PrivKeyBuildPolicy, RawTransactionFut, RawTransactionRequest, RawTransactionResult, RefundPaymentArgs,
    SearchForSwapTxSpendInput, SendMakerPaymentSpendPreimageInput, SendPaymentArgs, SignRawTransactionRequest,
    SignatureResult, SpendPaymentArgs, SwapOps, TradePreimageValue, TransactionFut, TransactionResult, TransactionType,
    TxFeeDetails, TxMarshalingErr, UnexpectedDerivationMethod, ValidateAddressResult, ValidateFeeArgs,
    ValidateOtherPubKeyErr, ValidatePaymentError, ValidatePaymentFut, ValidatePaymentInput, ValidateWatcherSpendInput,
    VerificationResult, WaitForHTLCTxSpendArgs, WatcherOps, WatcherReward, WatcherRewardError,
    WatcherSearchForSwapTxSpendInput, WatcherValidatePaymentInput, WatcherValidateTakerFeeInput, WithdrawFut,
};
use bitcrypto::sign_message_hash;
use common::executor::{AbortableSystem, AbortedError};
use common::log::warn;
use derive_more::Display;
use futures::{FutureExt, TryFutureExt};
use itertools::Either as EitherIter;
use keys::hash::H256;
use keys::CashAddress;
pub use keys::NetworkPrefix as CashAddrPrefix;
use mm2_metrics::MetricsArc;
use mm2_number::MmNumber;
use rpc::v1::types::H264 as H264Json;
use serde_json::{self as json, Value as Json};
use serialization::{deserialize, CoinVariant};
use std::sync::MutexGuard;

pub type BchUnspentMap = HashMap<Address, BchUnspents>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct BchActivationRequest {
    #[serde(default)]
    pub allow_slp_unsafe_conf: bool,
    pub bchd_urls: Vec<String>,
    #[serde(flatten)]
    pub utxo_params: UtxoActivationParams,
}

#[derive(Debug, Display)]
pub enum BchFromLegacyReqErr {
    InvalidUtxoParams(UtxoFromLegacyReqErr),
    InvalidBchdUrls(json::Error),
}

impl From<UtxoFromLegacyReqErr> for BchFromLegacyReqErr {
    fn from(err: UtxoFromLegacyReqErr) -> Self {
        BchFromLegacyReqErr::InvalidUtxoParams(err)
    }
}

impl BchActivationRequest {
    pub fn from_legacy_req(req: &Json) -> Result<Self, MmError<BchFromLegacyReqErr>> {
        let bchd_urls = json::from_value(req["bchd_urls"].clone()).map_to_mm(BchFromLegacyReqErr::InvalidBchdUrls)?;
        let allow_slp_unsafe_conf = req["allow_slp_unsafe_conf"].as_bool().unwrap_or_default();
        let utxo_params = UtxoActivationParams::from_legacy_req(req).map_mm_err()?;

        Ok(BchActivationRequest {
            allow_slp_unsafe_conf,
            bchd_urls,
            utxo_params,
        })
    }
}

#[derive(Clone)]
pub struct BchCoin {
    utxo_arc: UtxoArc,
    slp_addr_prefix: CashAddrPrefix,
    bchd_urls: Vec<String>,
    slp_tokens_infos: Arc<Mutex<HashMap<String, SlpTokenInfo>>>,
}

impl From<BchCoin> for UtxoArc {
    fn from(coin: BchCoin) -> Self {
        coin.utxo_arc
    }
}

#[allow(clippy::large_enum_variant)]
pub enum IsSlpUtxoError {
    Rpc(UtxoRpcError),
    TxDeserialization(serialization::Error),
}

#[derive(Debug, Default)]
pub struct BchUnspents {
    /// Standard BCH UTXOs
    standard: Vec<UnspentInfo>,
    /// SLP related UTXOs
    slp: HashMap<H256, Vec<SlpUnspent>>,
    /// SLP minting batons outputs, DO NOT use them as MM2 doesn't support SLP minting by default
    slp_batons: Vec<UnspentInfo>,
    /// The unspents of transaction with an undetermined protocol (OP_RETURN in 0 output but not SLP)
    /// DO NOT ever use them to avoid burning users funds
    undetermined: Vec<UnspentInfo>,
}

impl BchUnspents {
    fn add_standard(&mut self, utxo: UnspentInfo) {
        self.standard.push(utxo)
    }

    fn add_slp(&mut self, token_id: H256, bch_unspent: UnspentInfo, slp_amount: u64) {
        let slp_unspent = SlpUnspent {
            bch_unspent,
            slp_amount,
        };
        self.slp.entry(token_id).or_default().push(slp_unspent);
    }

    fn add_slp_baton(&mut self, utxo: UnspentInfo) {
        self.slp_batons.push(utxo)
    }

    fn add_undetermined(&mut self, utxo: UnspentInfo) {
        self.undetermined.push(utxo)
    }

    pub fn platform_balance(&self, decimals: u8) -> CoinBalance {
        let spendable_sat = total_unspent_value(&self.standard);

        let unspendable_slp = self.slp.iter().fold(0, |cur, (_, slp_unspents)| {
            let bch_value = total_unspent_value(slp_unspents.iter().map(|slp| &slp.bch_unspent));
            cur + bch_value
        });

        let unspendable_slp_batons = total_unspent_value(&self.slp_batons);
        let unspendable_undetermined = total_unspent_value(&self.undetermined);

        let total_unspendable = unspendable_slp + unspendable_slp_batons + unspendable_undetermined;
        CoinBalance {
            spendable: big_decimal_from_sat_unsigned(spendable_sat, decimals),
            unspendable: big_decimal_from_sat_unsigned(total_unspendable, decimals),
        }
    }

    pub fn slp_token_balance(&self, token_id: &H256, decimals: u8) -> CoinBalance {
        self.slp
            .get(token_id)
            .map(|unspents| {
                let total_sat = unspents.iter().fold(0, |cur, unspent| cur + unspent.slp_amount);
                CoinBalance {
                    spendable: big_decimal_from_sat_unsigned(total_sat, decimals),
                    unspendable: 0.into(),
                }
            })
            .unwrap_or_default()
    }
}

impl From<UtxoRpcError> for IsSlpUtxoError {
    fn from(err: UtxoRpcError) -> IsSlpUtxoError {
        IsSlpUtxoError::Rpc(err)
    }
}

impl From<serialization::Error> for IsSlpUtxoError {
    fn from(err: serialization::Error) -> IsSlpUtxoError {
        IsSlpUtxoError::TxDeserialization(err)
    }
}

impl BchCoin {
    pub fn new(utxo_arc: UtxoArc, slp_addr_prefix: CashAddrPrefix, bchd_urls: Vec<String>) -> Self {
        BchCoin {
            utxo_arc,
            slp_addr_prefix,
            bchd_urls,
            slp_tokens_infos: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn slp_prefix(&self) -> &CashAddrPrefix {
        &self.slp_addr_prefix
    }

    pub fn slp_address(&self, address: &Address) -> Result<CashAddress, String> {
        let conf = &self.as_ref().conf;
        address.to_cashaddress(&self.slp_prefix().to_string(), &conf.address_prefixes)
    }

    pub fn bchd_urls(&self) -> &[String] {
        &self.bchd_urls
    }

    async fn utxos_into_bch_unspents(&self, utxos: Vec<UnspentInfo>) -> UtxoRpcResult<BchUnspents> {
        let mut result = BchUnspents::default();
        let mut temporary_undetermined = Vec::new();

        let to_verbose: HashSet<H256Json> = utxos
            .into_iter()
            .filter_map(|unspent| {
                if unspent.outpoint.index == 0 {
                    // Zero output is reserved for OP_RETURN of specific protocols
                    // so if we get it we can safely consider this as standard BCH UTXO.
                    // There is no need to request verbose transaction for such UTXO.
                    result.add_standard(unspent);
                    None
                } else {
                    let hash = unspent.outpoint.hash.reversed().into();
                    temporary_undetermined.push(unspent);
                    Some(hash)
                }
            })
            .collect();

        let verbose_txs = self
            .get_verbose_transactions_from_cache_or_rpc(to_verbose)
            .compat()
            .await?;

        for unspent in temporary_undetermined {
            let prev_tx_hash = unspent.outpoint.hash.reversed().into();
            let prev_tx_bytes = verbose_txs
                .get(&prev_tx_hash)
                .or_mm_err(|| {
                    UtxoRpcError::Internal(format!(
                        "'get_verbose_transactions_from_cache_or_rpc' should have returned '{prev_tx_hash:?}'"
                    ))
                })?
                .to_inner();
            let prev_tx: UtxoTx = match deserialize(prev_tx_bytes.hex.as_slice()) {
                Ok(b) => b,
                Err(e) => {
                    warn!(
                        "Failed to deserialize prev_tx {:?} with error {:?}, considering {:?} as undetermined",
                        prev_tx_bytes, e, unspent
                    );
                    result.add_undetermined(unspent);
                    continue;
                },
            };

            if prev_tx.outputs.is_empty() {
                warn!(
                    "Prev_tx {:?} outputs are empty, considering {:?} as undetermined",
                    prev_tx_bytes, unspent
                );
                result.add_undetermined(unspent);
                continue;
            }

            let zero_out_script: Script = prev_tx.outputs[0].script_pubkey.clone().into();
            if zero_out_script.is_pay_to_public_key()
                || zero_out_script.is_pay_to_public_key_hash()
                || zero_out_script.is_pay_to_script_hash()
            {
                result.add_standard(unspent);
            } else {
                match parse_slp_script(&prev_tx.outputs[0].script_pubkey) {
                    Ok(slp_data) => match slp_data.transaction {
                        SlpTransaction::Send { token_id, amounts } => {
                            match amounts.get(unspent.outpoint.index as usize - 1) {
                                Some(slp_amount) => result.add_slp(token_id, unspent, *slp_amount),
                                None => result.add_standard(unspent),
                            }
                        },
                        SlpTransaction::Genesis(genesis) => {
                            if unspent.outpoint.index == 1 {
                                let token_id = prev_tx.hash().reversed();
                                result.add_slp(token_id, unspent, genesis.initial_token_mint_quantity);
                            } else if Some(unspent.outpoint.index) == genesis.mint_baton_vout.map(|u| u as u32) {
                                result.add_slp_baton(unspent);
                            } else {
                                result.add_standard(unspent);
                            }
                        },
                        SlpTransaction::Mint {
                            token_id,
                            additional_token_quantity,
                            mint_baton_vout,
                        } => {
                            if unspent.outpoint.index == 1 {
                                result.add_slp(token_id, unspent, additional_token_quantity);
                            } else if Some(unspent.outpoint.index) == mint_baton_vout.map(|u| u as u32) {
                                result.add_slp_baton(unspent);
                            } else {
                                result.add_standard(unspent);
                            }
                        },
                    },
                    Err(e) => {
                        warn!(
                            "Error {} parsing script {:?} as SLP, considering {:?} as undetermined",
                            e, prev_tx.outputs[0].script_pubkey, unspent
                        );
                        result.undetermined.push(unspent);
                    },
                };
            }
        }
        Ok(result)
    }

    /// Returns unspents to calculate balance, use for displaying purposes only!
    /// DO NOT USE to build transactions, it can lead to double spending attempt and also have other unpleasant consequences
    pub async fn bch_unspents_for_display(&self, address: &Address) -> UtxoRpcResult<BchUnspents> {
        // ordering is not required to display balance to we can simply call "normal" list_unspent
        let all_unspents = self
            .utxo_arc
            .rpc_client
            .list_unspent(address, self.utxo_arc.decimals)
            .compat()
            .await?;
        self.utxos_into_bch_unspents(all_unspents).await
    }

    /// Locks recently spent cache to safely return UTXOs for spending
    pub async fn bch_unspents_for_spend(
        &self,
        address: &Address,
    ) -> UtxoRpcResult<(BchUnspents, RecentlySpentOutPointsGuard<'_>)> {
        let (all_unspents, recently_spent) = utxo_common::get_unspent_ordered_list(self, address).await?;
        let result = self.utxos_into_bch_unspents(all_unspents).await?;

        Ok((result, recently_spent))
    }

    pub async fn get_token_utxos_for_spend(
        &self,
        token_id: &H256,
    ) -> UtxoRpcResult<(Vec<SlpUnspent>, Vec<UnspentInfo>, RecentlySpentOutPointsGuard<'_>)> {
        let my_address = self
            .as_ref()
            .derivation_method
            .single_addr_or_err()
            .await
            .mm_err(|e| UtxoRpcError::Internal(e.to_string()))?;
        let (mut bch_unspents, recently_spent) = self.bch_unspents_for_spend(&my_address).await?;
        let (mut slp_unspents, standard_utxos) = (
            bch_unspents.slp.remove(token_id).unwrap_or_default(),
            bch_unspents.standard,
        );

        slp_unspents.sort_by(|a, b| a.slp_amount.cmp(&b.slp_amount));
        Ok((slp_unspents, standard_utxos, recently_spent))
    }

    pub async fn get_token_utxos_for_display(
        &self,
        token_id: &H256,
    ) -> UtxoRpcResult<(Vec<SlpUnspent>, Vec<UnspentInfo>)> {
        let my_address = self
            .as_ref()
            .derivation_method
            .single_addr_or_err()
            .await
            .mm_err(|e| UtxoRpcError::Internal(e.to_string()))?;
        let mut bch_unspents = self.bch_unspents_for_display(&my_address).await?;
        let (mut slp_unspents, standard_utxos) = (
            bch_unspents.slp.remove(token_id).unwrap_or_default(),
            bch_unspents.standard,
        );

        slp_unspents.sort_by(|a, b| a.slp_amount.cmp(&b.slp_amount));
        Ok((slp_unspents, standard_utxos))
    }

    pub fn add_slp_token_info(&self, ticker: String, info: SlpTokenInfo) {
        self.slp_tokens_infos.lock().unwrap().insert(ticker, info);
    }

    pub fn get_slp_tokens_infos(&self) -> MutexGuard<'_, HashMap<String, SlpTokenInfo>> {
        self.slp_tokens_infos.lock().unwrap()
    }

    pub async fn get_my_slp_address(&self) -> Result<CashAddress, String> {
        let my_address = try_s!(self.as_ref().derivation_method.single_addr_or_err().await);
        let slp_address =
            my_address.to_cashaddress(&self.slp_prefix().to_string(), &self.as_ref().conf.address_prefixes)?;
        Ok(slp_address)
    }

    /// Returns multiple details by tx hash if token transfers also occurred in the transaction
    pub async fn transaction_details_with_token_transfers<T: TxHistoryStorage>(
        &self,
        params: UtxoTxDetailsParams<'_, T>,
    ) -> MmResult<Vec<TransactionDetails>, UtxoTxDetailsError> {
        let tx = self.tx_from_storage_or_rpc(params.hash, params.storage).await?;

        let bch_tx_details = self
            .bch_tx_details(
                params.hash,
                &tx,
                params.block_height_and_time,
                params.storage,
                params.my_addresses,
            )
            .await?;
        let maybe_op_return: Script = tx
            .outputs
            .first()
            .ok_or(UtxoTxDetailsError::Internal(format!(
                "Transaction {} has no outputs",
                params.hash
            )))?
            .script_pubkey
            .clone()
            .into();
        if !(maybe_op_return.is_pay_to_public_key_hash()
            || maybe_op_return.is_pay_to_public_key()
            || maybe_op_return.is_pay_to_script_hash())
        {
            if let Ok(slp_details) = parse_slp_script(&maybe_op_return) {
                let slp_tx_details = self
                    .slp_tx_details(
                        &tx,
                        slp_details.transaction,
                        params.block_height_and_time,
                        bch_tx_details.fee_details.clone(),
                        params.storage,
                        params.my_addresses,
                    )
                    .await?;
                return Ok(vec![bch_tx_details, slp_tx_details]);
            }
        }

        Ok(vec![bch_tx_details])
    }

    async fn bch_tx_details<T: TxHistoryStorage>(
        &self,
        tx_hash: &H256Json,
        tx: &UtxoTx,
        height_and_time: Option<BlockHeightAndTime>,
        storage: &T,
        my_addresses: &HashSet<Address>,
    ) -> MmResult<TransactionDetails, UtxoTxDetailsError> {
        let mut tx_builder = TxDetailsBuilder::new(self.ticker().to_owned(), tx, height_and_time, my_addresses.clone());
        for output in &tx.outputs {
            let addresses = match self.addresses_from_script(&output.script_pubkey.clone().into()) {
                Ok(a) => a,
                Err(_) => continue,
            };

            if addresses.is_empty() {
                continue;
            }

            if addresses.len() != 1 {
                let msg = format!(
                    "{} tx {:02x} output script resulted into unexpected number of addresses",
                    self.ticker(),
                    tx_hash,
                );
                return MmError::err(UtxoTxDetailsError::TxAddressDeserializationError(msg));
            }

            let amount = big_decimal_from_sat_unsigned(output.value, self.decimals());
            for address in addresses {
                tx_builder.transferred_to(address, &amount);
            }
        }

        let mut total_input = 0;
        for input in &tx.inputs {
            let index = input.previous_output.index;
            let prev_tx = self
                .tx_from_storage_or_rpc(&input.previous_output.hash.reversed().into(), storage)
                .await?;
            let prev_script = prev_tx.outputs[index as usize].script_pubkey.clone().into();
            let addresses = self
                .addresses_from_script(&prev_script)
                .map_to_mm(UtxoTxDetailsError::TxAddressDeserializationError)?;
            if addresses.len() != 1 {
                let msg = format!(
                    "{} tx {:02x} output script resulted into unexpected number of addresses",
                    self.ticker(),
                    tx_hash,
                );
                return MmError::err(UtxoTxDetailsError::TxAddressDeserializationError(msg));
            }

            let prev_value = prev_tx.outputs[index as usize].value;
            total_input += prev_value;
            let amount = big_decimal_from_sat_unsigned(prev_value, self.decimals());
            for address in addresses {
                tx_builder.transferred_from(address, &amount);
            }
        }

        let total_output = tx.outputs.iter().fold(0, |total, output| total + output.value);
        let fee = Some(TxFeeDetails::Utxo(UtxoFeeDetails {
            coin: Some(self.ticker().into()),
            amount: big_decimal_from_sat_unsigned(total_input - total_output, self.decimals()),
        }));
        tx_builder.set_tx_fee(fee);
        Ok(tx_builder.build())
    }

    async fn get_slp_genesis_params<T: TxHistoryStorage>(
        &self,
        token_id: H256,
        storage: &T,
    ) -> MmResult<SlpGenesisParams, UtxoTxDetailsError> {
        let token_genesis_tx = self.tx_from_storage_or_rpc(&token_id.into(), storage).await?;
        let maybe_genesis_script: Script = token_genesis_tx.outputs[0].script_pubkey.clone().into();
        let slp_details = parse_slp_script(&maybe_genesis_script).map_mm_err()?;
        match slp_details.transaction {
            SlpTransaction::Genesis(params) => Ok(params),
            _ => {
                let error = format!("SLP token ID '{token_id}' is not a genesis TX");
                MmError::err(UtxoTxDetailsError::InvalidTransaction(error))
            },
        }
    }

    async fn slp_transferred_amounts<T: TxHistoryStorage>(
        &self,
        utxo_tx: &UtxoTx,
        slp_tx: SlpTransaction,
        storage: &T,
    ) -> MmResult<HashMap<usize, (CashAddress, BigDecimal)>, UtxoTxDetailsError> {
        let slp_amounts = match slp_tx {
            SlpTransaction::Send { token_id, amounts } => {
                let genesis_params = self.get_slp_genesis_params(token_id, storage).await?;
                EitherIter::Left(
                    amounts
                        .into_iter()
                        .map(move |amount| big_decimal_from_sat_unsigned(amount, genesis_params.decimals[0])),
                )
            },
            SlpTransaction::Mint {
                token_id,
                additional_token_quantity,
                ..
            } => {
                let slp_genesis_params = self.get_slp_genesis_params(token_id, storage).await?;
                EitherIter::Right(std::iter::once(big_decimal_from_sat_unsigned(
                    additional_token_quantity,
                    slp_genesis_params.decimals[0],
                )))
            },
            SlpTransaction::Genesis(genesis_params) => EitherIter::Right(std::iter::once(
                big_decimal_from_sat_unsigned(genesis_params.initial_token_mint_quantity, genesis_params.decimals[0]),
            )),
        };

        let mut result = HashMap::new();
        for (i, amount) in slp_amounts.into_iter().enumerate() {
            let output_index = i + 1;
            match utxo_tx.outputs.get(output_index) {
                Some(output) => {
                    let addresses = self
                        .addresses_from_script(&output.script_pubkey.clone().into())
                        .map_to_mm(UtxoTxDetailsError::TxAddressDeserializationError)?;
                    if addresses.len() != 1 {
                        let msg = format!(
                            "{} tx {:?} output script resulted into unexpected number of addresses",
                            self.ticker(),
                            utxo_tx.hash().reversed(),
                        );
                        return MmError::err(UtxoTxDetailsError::TxAddressDeserializationError(msg));
                    }

                    let slp_address = self
                        .slp_address(&addresses[0])
                        .map_to_mm(UtxoTxDetailsError::InvalidTransaction)?;
                    result.insert(output_index, (slp_address, amount));
                },
                None => {
                    let error = format!(
                        "Unexpected '{}' output index at {} TX",
                        output_index,
                        utxo_tx.hash().reversed()
                    );
                    return MmError::err(UtxoTxDetailsError::InvalidTransaction(error));
                },
            }
        }
        Ok(result)
    }

    async fn slp_tx_details<Storage: TxHistoryStorage>(
        &self,
        tx: &UtxoTx,
        slp_tx: SlpTransaction,
        height_and_time: Option<BlockHeightAndTime>,
        tx_fee: Option<TxFeeDetails>,
        storage: &Storage,
        my_addresses: &HashSet<Address>,
    ) -> MmResult<TransactionDetails, UtxoTxDetailsError> {
        let token_id = match slp_tx.token_id() {
            Some(id) => id,
            None => tx.hash().reversed(),
        };

        let slp_addresses: Vec<_> = my_addresses
            .iter()
            .map(|addr| self.slp_address(addr))
            .collect::<Result<_, _>>()
            .map_to_mm(UtxoTxDetailsError::Internal)?;

        let mut slp_tx_details_builder =
            TxDetailsBuilder::new(self.ticker().to_owned(), tx, height_and_time, slp_addresses);
        let slp_transferred_amounts = self.slp_transferred_amounts(tx, slp_tx, storage).await?;
        for (_, (address, amount)) in slp_transferred_amounts {
            slp_tx_details_builder.transferred_to(address, &amount);
        }

        for input in &tx.inputs {
            let prev_tx = self
                .tx_from_storage_or_rpc(&input.previous_output.hash.reversed().into(), storage)
                .await?;
            if let Ok(slp_tx_details) = parse_slp_script(&prev_tx.outputs[0].script_pubkey) {
                let mut prev_slp_transferred = self
                    .slp_transferred_amounts(&prev_tx, slp_tx_details.transaction, storage)
                    .await?;
                let i = input.previous_output.index as usize;
                if let Some((address, amount)) = prev_slp_transferred.remove(&i) {
                    slp_tx_details_builder.transferred_from(address, &amount);
                }
            }
        }

        slp_tx_details_builder.set_transaction_type(TransactionType::TokenTransfer(token_id.take().to_vec().into()));
        slp_tx_details_builder.set_tx_fee(tx_fee);

        Ok(slp_tx_details_builder.build())
    }

    pub async fn get_block_timestamp(&self, height: u64) -> Result<u64, MmError<GetBlockHeaderError>> {
        self.as_ref().rpc_client.get_block_timestamp(height).await
    }
}

impl AsRef<UtxoCoinFields> for BchCoin {
    fn as_ref(&self) -> &UtxoCoinFields {
        &self.utxo_arc
    }
}

pub async fn bch_coin_with_policy(
    ctx: &MmArc,
    ticker: &str,
    conf: &Json,
    params: BchActivationRequest,
    slp_addr_prefix: CashAddrPrefix,
    priv_key_policy: PrivKeyBuildPolicy,
) -> Result<BchCoin, String> {
    if conf["coin"].as_str() != Some(ticker) {
        return ERR!("Failed to activate '{}': ticker does not match coins config", ticker);
    }
    if params.bchd_urls.is_empty() && !params.allow_slp_unsafe_conf {
        return Err("Using empty bchd_urls is unsafe for SLP users!".into());
    }

    let bchd_urls = params.bchd_urls;
    let constructor = { move |utxo_arc| BchCoin::new(utxo_arc, slp_addr_prefix.clone(), bchd_urls.clone()) };

    let coin = try_s!(
        UtxoArcBuilder::new(ctx, ticker, conf, &params.utxo_params, priv_key_policy, constructor)
            .build()
            .await
    );
    Ok(coin)
}

pub async fn bch_coin_with_priv_key(
    ctx: &MmArc,
    ticker: &str,
    conf: &Json,
    params: BchActivationRequest,
    slp_addr_prefix: CashAddrPrefix,
    priv_key: IguanaPrivKey,
) -> Result<BchCoin, String> {
    let priv_key_policy = PrivKeyBuildPolicy::IguanaPrivKey(priv_key);
    bch_coin_with_policy(ctx, ticker, conf, params, slp_addr_prefix, priv_key_policy).await
}

#[derive(Debug)]
pub enum BchActivationError {
    CoinInitError(String),
    TokenConfIsNotFound {
        token: String,
    },
    TokenCoinProtocolParseError {
        token: String,
        error: json::Error,
    },
    TokenCoinProtocolIsNotSlp {
        token: String,
        protocol: CoinProtocol,
    },
    TokenPlatformCoinIsInvalidInConf {
        token: String,
        expected_platform: String,
        actual_platform: String,
    },
    RpcError(UtxoRpcError),
    SlpPrefixParseError(String),
}

impl From<UtxoRpcError> for BchActivationError {
    fn from(e: UtxoRpcError) -> Self {
        BchActivationError::RpcError(e)
    }
}

// if mockable is placed before async_trait there is `munmap_chunk(): invalid pointer` error on async fn mocking attempt
#[async_trait]
#[cfg_attr(test, mockable)]
impl UtxoTxBroadcastOps for BchCoin {
    async fn broadcast_tx(&self, tx: &UtxoTx) -> Result<H256Json, MmError<BroadcastTxErr>> {
        utxo_common::broadcast_tx(self, tx).await
    }
}

// if mockable is placed before async_trait there is `munmap_chunk(): invalid pointer` error on async fn mocking attempt
#[async_trait]
#[cfg_attr(test, mockable)]
impl UtxoTxGenerationOps for BchCoin {
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
impl GetUtxoListOps for BchCoin {
    async fn get_unspent_ordered_list(
        &self,
        address: &Address,
    ) -> UtxoRpcResult<(Vec<UnspentInfo>, RecentlySpentOutPointsGuard<'_>)> {
        let (bch_unspents, recently_spent) = self.bch_unspents_for_spend(address).await?;
        Ok((bch_unspents.standard, recently_spent))
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
        let (unspents, recently_spent) = utxo_common::get_all_unspent_ordered_list(self, address).await?;
        Ok((MatureUnspentList::new_mature(unspents), recently_spent))
    }
}

#[async_trait]
#[cfg_attr(test, mockable)]
impl GetUtxoMapOps for BchCoin {
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
impl UtxoCommonOps for BchCoin {
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
        // BCH uses the same coin variant as BTC for block header deserialization
        utxo_common::get_current_mtp(&self.utxo_arc, CoinVariant::BTC).await
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
        let addr_format = self.addr_format().clone();
        utxo_common::address_from_pubkey(
            pubkey,
            conf.address_prefixes.clone(),
            conf.checksum_type,
            conf.bech32_hrp.clone(),
            addr_format,
        )
    }
}

#[async_trait]
impl SwapOps for BchCoin {
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

fn total_unspent_value<'a>(unspents: impl IntoIterator<Item = &'a UnspentInfo>) -> u64 {
    unspents.into_iter().fold(0, |cur, unspent| cur + unspent.value)
}

#[async_trait]
impl WatcherOps for BchCoin {
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

#[async_trait]
impl MarketCoinOps for BchCoin {
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
        let coin = self.clone();
        let fut = async move {
            let my_address = coin
                .as_ref()
                .derivation_method
                .single_addr_or_err()
                .await
                .map_mm_err()?;
            let bch_unspents = coin.bch_unspents_for_display(&my_address).await.map_mm_err()?;
            Ok(bch_unspents.platform_balance(coin.as_ref().decimals))
        };
        Box::new(fut.boxed().compat())
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

    fn should_burn_dex_fee(&self) -> bool {
        utxo_common::should_burn_dex_fee()
    }

    fn is_trezor(&self) -> bool {
        self.as_ref().priv_key_policy.is_trezor()
    }
}

#[async_trait]
impl MmCoin for BchCoin {
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

    fn process_history_loop(&self, _ctx: MmArc) -> Box<dyn Future<Item = (), Error = ()> + Send> {
        warn!("'process_history_loop' is not implemented for BchCoin! Consider using 'my_tx_history_v2'");
        Box::new(futures01::future::err(()))
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

    fn on_token_deactivated(&self, ticker: &str) {
        if let Ok(tokens) = self.slp_tokens_infos.lock().as_deref_mut() {
            tokens.remove(ticker);
        };
    }
}

#[async_trait]
impl GetWithdrawSenderAddress for BchCoin {
    type Address = Address;
    type Pubkey = Public;

    async fn get_withdraw_sender_address(
        &self,
        req: &WithdrawRequest,
    ) -> MmResult<WithdrawSenderAddress<Self::Address, Self::Pubkey>, WithdrawError> {
        utxo_common::get_withdraw_from_address(self, req).await
    }
}

impl CoinWithPrivKeyPolicy for BchCoin {
    type KeyPair = KeyPair;

    fn priv_key_policy(&self) -> &PrivKeyPolicy<Self::KeyPair> {
        &self.utxo_arc.priv_key_policy
    }
}

impl CoinWithDerivationMethod for BchCoin {
    fn derivation_method(&self) -> &DerivationMethod<HDCoinAddress<Self>, Self::HDWallet> {
        utxo_common::derivation_method(self.as_ref())
    }
}

#[async_trait]
impl IguanaBalanceOps for BchCoin {
    type BalanceObject = CoinBalanceMap;

    async fn iguana_balances(&self) -> BalanceResult<Self::BalanceObject> {
        let balance = self.my_balance().compat().await?;
        Ok(HashMap::from([(self.ticker().to_string(), balance)]))
    }
}

#[async_trait]
impl ExtractExtendedPubkey for BchCoin {
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
impl HDWalletCoinOps for BchCoin {
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

impl HDCoinWithdrawOps for BchCoin {}

#[async_trait]
impl HDWalletBalanceOps for BchCoin {
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
impl CoinWithTxHistoryV2 for BchCoin {
    fn history_wallet_id(&self) -> WalletId {
        WalletId::new(self.ticker().to_owned())
    }

    /// TODO consider using `utxo_common::utxo_tx_history_common::get_tx_history_filters`
    /// when `BchCoin` implements `CoinWithDerivationMethod`.
    async fn get_tx_history_filters(
        &self,
        target: MyTxHistoryTarget,
    ) -> MmResult<GetTxHistoryFilters, MyTxHistoryErrorV2> {
        match target {
            MyTxHistoryTarget::Iguana => (),
            target => {
                let error = format!("Expected 'Iguana' target, found {target:?}");
                return MmError::err(MyTxHistoryErrorV2::InvalidTarget(error));
            },
        }
        let my_address = self.my_address().map_mm_err()?;
        Ok(GetTxHistoryFilters::for_address(my_address))
    }
}

#[async_trait]
impl UtxoTxHistoryOps for BchCoin {
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
        Ok(self.transaction_details_with_token_transfers(params).await?)
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
        self.get_block_timestamp(height).await
    }

    async fn my_addresses_balances(&self) -> BalanceResult<HashMap<String, BigDecimal>> {
        let my_address = self
            .my_address()
            .map_err(|err| BalanceError::Internal(err.to_string()))?;
        let my_balance = self.my_balance().compat().await?;
        Ok(std::iter::once((my_address, my_balance.into_total())).collect())
    }

    fn address_from_str(&self, address: &str) -> MmResult<Address, AddrFromStrError> {
        utxo_common::checked_address_from_str(self, address)
    }

    fn set_history_sync_state(&self, new_state: HistorySyncState) {
        *self.as_ref().history_sync_state.lock().unwrap() = new_state;
    }
}

// testnet
#[cfg(test)]
pub fn tbch_coin_for_test() -> (MmArc, BchCoin) {
    use common::block_on;
    use crypto::privkey::key_pair_from_seed;
    use mm2_core::mm_ctx::MmCtxBuilder;
    use mm2_test_helpers::for_tests::{electrum_servers_rpc, BCHD_TESTNET_URLS, T_BCH_ELECTRUMS};

    let ctx = MmCtxBuilder::default().into_mm_arc();
    let keypair = key_pair_from_seed("BCH SLP test").unwrap();

    let conf = json!({"coin":"BCH","pubtype":0,"p2shtype":5,"mm2":1,"fork_id":"0x40","protocol":{"type":"UTXO"}, "sign_message_prefix": "Bitcoin Signed Message:\n",
         "address_format":{"format":"cashaddress","network":"bchtest"}});
    let req = json!({
        "method": "electrum",
        "coin": "BCH",
        "servers": electrum_servers_rpc(T_BCH_ELECTRUMS),
        "bchd_urls": BCHD_TESTNET_URLS,
        "allow_slp_unsafe_conf": false,
    });

    let params = BchActivationRequest::from_legacy_req(&req).unwrap();
    let coin = block_on(bch_coin_with_priv_key(
        &ctx,
        "BCH",
        &conf,
        params,
        CashAddrPrefix::SlpTest,
        keypair.private().secret,
    ))
    .unwrap();
    (ctx, coin)
}

// mainnet
#[cfg(test)]
pub fn bch_coin_for_test() -> BchCoin {
    use common::block_on;
    use crypto::privkey::key_pair_from_seed;
    use mm2_core::mm_ctx::MmCtxBuilder;

    let ctx = MmCtxBuilder::default().into_mm_arc();
    let keypair = key_pair_from_seed("BCH SLP test").unwrap();

    let conf = json!({"coin":"BCH","pubtype":0,"p2shtype":5,"mm2":1,"fork_id":"0x40","protocol":{"type":"UTXO"},
         "address_format":{"format":"cashaddress","network":"bitcoincash"}});
    let req = json!({
        "method": "electrum",
        "coin": "BCH",
        "servers": [{"url":"electrum1.cipig.net:10055"},{"url":"electrum2.cipig.net:10055"},{"url":"electrum3.cipig.net:10055"}],
        "bchd_urls": [],
        "allow_slp_unsafe_conf": true,
    });

    let params = BchActivationRequest::from_legacy_req(&req).unwrap();
    block_on(bch_coin_with_priv_key(
        &ctx,
        "BCH",
        &conf,
        params,
        CashAddrPrefix::SimpleLedger,
        keypair.private().secret,
    ))
    .unwrap()
}

#[cfg(test)]
mod bch_tests {
    use super::*;
    use crate::my_tx_history_v2::for_tests::init_storage_for;
    use crate::{TransactionType, TxFeeDetails};
    use common::block_on;

    #[test]
    fn test_get_slp_genesis_params() {
        let (_ctx, coin) = tbch_coin_for_test();
        let token_id = "bb309e48930671582bea508f9a1d9b491e49b69be3d6f372dc08da2ac6e90eb7".into();
        let (_ctx, storage) = init_storage_for(&coin);

        let slp_params = block_on(coin.get_slp_genesis_params(token_id, &storage)).unwrap();
        assert_eq!("USDF", slp_params.token_ticker);
        assert_eq!(4, slp_params.decimals[0]);
    }

    #[test]
    fn test_plain_bch_tx_details() {
        let (_ctx, coin) = tbch_coin_for_test();
        let (_ctx, storage) = init_storage_for(&coin);

        let hash = "a8dcc3c6776e93e7bd21fb81551e853447c55e2d8ac141b418583bc8095ce390".into();
        let tx = block_on(coin.tx_from_storage_or_rpc(&hash, &storage)).unwrap();

        let my_addresses = block_on(coin.my_addresses()).unwrap();
        let details = block_on(coin.bch_tx_details(&hash, &tx, None, &storage, &my_addresses)).unwrap();
        let expected_total: BigDecimal = "0.11407782".parse().unwrap();
        assert_eq!(expected_total, details.total_amount);

        let expected_received: BigDecimal = "0.11405301".parse().unwrap();
        assert_eq!(expected_received, details.received_by_me);

        let expected_spent: BigDecimal = "0.11407782".parse().unwrap();
        assert_eq!(expected_spent, details.spent_by_me);

        let expected_balance_change: BigDecimal = "-0.00002481".parse().unwrap();
        assert_eq!(expected_balance_change, details.my_balance_change);

        let expected_from = vec!["bchtest:qzx0llpyp8gxxsmad25twksqnwd62xm3lsnnczzt66".to_owned()];
        assert_eq!(expected_from, details.from);

        let expected_to = vec![
            "bchtest:qrhdt5adye8lc68upfj9fctfdgcd3aq9hctf8ft6md".to_owned(),
            "bchtest:qzx0llpyp8gxxsmad25twksqnwd62xm3lsnnczzt66".to_owned(),
        ];
        assert_eq!(expected_to, details.to);

        let expected_internal_id = BytesJson::from("a8dcc3c6776e93e7bd21fb81551e853447c55e2d8ac141b418583bc8095ce390");
        assert_eq!(expected_internal_id, details.internal_id);

        let expected_fee = Some(TxFeeDetails::Utxo(UtxoFeeDetails {
            coin: Some("BCH".into()),
            amount: "0.00001481".parse().unwrap(),
        }));
        assert_eq!(expected_fee, details.fee_details);

        assert_eq!(coin.ticker(), details.coin);
    }

    #[test]
    fn test_slp_tx_details() {
        let (_ctx, coin) = tbch_coin_for_test();
        let (_ctx, storage) = init_storage_for(&coin);

        let hash = "a8dcc3c6776e93e7bd21fb81551e853447c55e2d8ac141b418583bc8095ce390".into();
        let tx = block_on(coin.tx_from_storage_or_rpc(&hash, &storage)).unwrap();

        let slp_details = parse_slp_script(&tx.outputs[0].script_pubkey).unwrap();

        let my_addresses = block_on(coin.my_addresses()).unwrap();
        let slp_tx_details =
            block_on(coin.slp_tx_details(&tx, slp_details.transaction, None, None, &storage, &my_addresses)).unwrap();

        let expected_total: BigDecimal = "6.2974".parse().unwrap();
        assert_eq!(expected_total, slp_tx_details.total_amount);

        let expected_spent: BigDecimal = "6.2974".parse().unwrap();
        assert_eq!(expected_spent, slp_tx_details.spent_by_me);

        let expected_received: BigDecimal = "5.2974".parse().unwrap();
        assert_eq!(expected_received, slp_tx_details.received_by_me);

        let expected_balance_change = BigDecimal::from(-1i32);
        assert_eq!(expected_balance_change, slp_tx_details.my_balance_change);

        let expected_from = vec!["slptest:qzx0llpyp8gxxsmad25twksqnwd62xm3lsg8lecug8".to_owned()];
        assert_eq!(expected_from, slp_tx_details.from);

        let expected_to = vec![
            "slptest:qrhdt5adye8lc68upfj9fctfdgcd3aq9hcsaqj3dfs".to_owned(),
            "slptest:qzx0llpyp8gxxsmad25twksqnwd62xm3lsg8lecug8".to_owned(),
        ];
        assert_eq!(expected_to, slp_tx_details.to);

        let expected_tx_type =
            TransactionType::TokenTransfer("bb309e48930671582bea508f9a1d9b491e49b69be3d6f372dc08da2ac6e90eb7".into());
        assert_eq!(expected_tx_type, slp_tx_details.transaction_type);

        assert_eq!(coin.ticker(), slp_tx_details.coin);
    }

    #[test]
    fn test_sign_message() {
        let (_ctx, coin) = tbch_coin_for_test();
        let signature = coin.sign_message("test", None).unwrap();
        assert_eq!(
            signature,
            "ILuePKMsycXwJiNDOT7Zb7TfIlUW7Iq+5ylKd15AK72vGVYXbnf7Gj9Lk9MFV+6Ub955j7MiAkp0wQjvuIoRPPA="
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn test_verify_message() {
        let (_ctx, coin) = tbch_coin_for_test();
        let is_valid = coin
            .verify_message(
                "ILuePKMsycXwJiNDOT7Zb7TfIlUW7Iq+5ylKd15AK72vGVYXbnf7Gj9Lk9MFV+6Ub955j7MiAkp0wQjvuIoRPPA=",
                "test",
                "bchtest:qzx0llpyp8gxxsmad25twksqnwd62xm3lsnnczzt66",
            )
            .unwrap();
        assert!(is_valid);
    }
}
