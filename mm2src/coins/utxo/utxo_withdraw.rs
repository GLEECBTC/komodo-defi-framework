use crate::rpc_command::init_withdraw::{WithdrawInProgressStatus, WithdrawTaskHandleShared};
use crate::utxo::utxo_common::{big_decimal_from_sat, UtxoTxBuilder};
use crate::utxo::{
    output_script, sat_from_big_decimal, ActualFeeRate, Address, FeePolicy, GetUtxoListOps, PrivKeyPolicy,
    UtxoAddressFormat, UtxoCoinFields, UtxoCommonOps, UtxoFeeDetails, UtxoTx, UTXO_LOCK,
};
use crate::{
    CoinWithDerivationMethod, GetWithdrawSenderAddress, MarketCoinOps, TransactionData, TransactionDetails,
    UnexpectedDerivationMethod, WithdrawError, WithdrawFee, WithdrawRequest, WithdrawResult,
};
use async_trait::async_trait;
use chain::TransactionOutput;
use common::log::info;
use common::now_sec;
use crypto::hw_rpc_task::HwRpcTaskAwaitingStatus;
use crypto::trezor::trezor_rpc_task::{TrezorRequestStatuses, TrezorRpcTaskProcessor};
use crypto::trezor::{TrezorError, TrezorProcessingError};
use crypto::{from_hw_error, CryptoCtx, CryptoCtxError, DerivationPath, HwError, HwProcessingError, HwRpcError};
use keys::{AddressFormat, KeyPair, Private, Public as PublicKey};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use rpc::v1::types::ToTxHash;
use rpc_task::RpcTaskError;
use script::{SignatureVersion, TransactionInputSigner};
use serialization::{serialize, serialize_with_flags, SERIALIZE_TRANSACTION_WITNESS};
use std::iter::once;
use std::sync::Arc;
use utxo_signer::sign_params::{OutputDestination, SendingOutputInfo, SpendingInputInfo, UtxoSignTxParamsBuilder};
use utxo_signer::{with_key_pair, UtxoSignTxError};
use utxo_signer::{SignPolicy, UtxoSignerOps};

impl From<UtxoSignTxError> for WithdrawError {
    fn from(sign_err: UtxoSignTxError) -> Self {
        match sign_err {
            UtxoSignTxError::TrezorError(trezor) => WithdrawError::from(trezor),
            UtxoSignTxError::Transport(transport) => WithdrawError::Transport(transport),
            UtxoSignTxError::Internal(internal) => WithdrawError::InternalError(internal),
            sign_err => WithdrawError::InternalError(sign_err.to_string()),
        }
    }
}

impl From<HwProcessingError<RpcTaskError>> for WithdrawError {
    fn from(e: HwProcessingError<RpcTaskError>) -> Self {
        match e {
            HwProcessingError::HwError(hw) => WithdrawError::from(hw),
            HwProcessingError::ProcessorError(rpc_task) => WithdrawError::from(rpc_task),
            HwProcessingError::InternalError(err) => WithdrawError::InternalError(err),
        }
    }
}

impl From<TrezorProcessingError<RpcTaskError>> for WithdrawError {
    fn from(e: TrezorProcessingError<RpcTaskError>) -> Self {
        match e {
            TrezorProcessingError::TrezorError(trezor) => WithdrawError::from(trezor),
            TrezorProcessingError::ProcessorError(rpc_task) => WithdrawError::from(rpc_task),
        }
    }
}

impl From<HwError> for WithdrawError {
    fn from(e: HwError) -> Self {
        from_hw_error(e)
    }
}

impl From<TrezorError> for WithdrawError {
    fn from(e: TrezorError) -> Self {
        match e {
            TrezorError::DeviceDisconnected => WithdrawError::HwError(HwRpcError::NoTrezorDeviceAvailable),
            other => WithdrawError::InternalError(other.to_string()),
        }
    }
}

impl From<CryptoCtxError> for WithdrawError {
    fn from(e: CryptoCtxError) -> Self {
        WithdrawError::InternalError(e.to_string())
    }
}

impl From<RpcTaskError> for WithdrawError {
    fn from(e: RpcTaskError) -> Self {
        let error = e.to_string();
        match e {
            RpcTaskError::Cancelled => WithdrawError::InternalError("Cancelled".to_owned()),
            RpcTaskError::Timeout(timeout) => WithdrawError::Timeout(timeout),
            RpcTaskError::NoSuchTask(_) | RpcTaskError::UnexpectedTaskStatus { .. } => {
                WithdrawError::InternalError(error)
            },
            RpcTaskError::UnexpectedUserAction { expected } => WithdrawError::UnexpectedUserAction { expected },
            RpcTaskError::Internal(internal) => WithdrawError::InternalError(internal),
        }
    }
}

impl From<keys::Error> for WithdrawError {
    fn from(e: keys::Error) -> Self {
        WithdrawError::InternalError(e.to_string())
    }
}

fn derive_hd_key_pair<Coin>(
    coin: &Coin,
    derivation_path: &DerivationPath,
) -> Result<KeyPair, MmError<UnexpectedDerivationMethod>>
where
    Coin: AsRef<UtxoCoinFields>,
{
    let secret = coin
        .as_ref()
        .priv_key_policy
        .hd_wallet_derived_priv_key_or_err(derivation_path)
        .map_mm_err()?;

    let private = Private {
        prefix: coin.as_ref().conf.wif_prefix,
        secret,
        compressed: true,
        checksum_type: coin.as_ref().conf.checksum_type,
    };

    KeyPair::from_private(private).map_to_mm(|err| UnexpectedDerivationMethod::InternalError(err.to_string()))
}

#[async_trait]
pub trait UtxoWithdraw<Coin>
where
    Self: Sized + Sync,
    Coin: UtxoCommonOps + GetUtxoListOps,
{
    fn coin(&self) -> &Coin;

    fn sender_address(&self) -> Address;

    fn sender_address_string(&self) -> String;

    fn request(&self) -> &WithdrawRequest;

    fn signature_version(&self) -> SignatureVersion {
        match self.sender_address().addr_format() {
            UtxoAddressFormat::Segwit => SignatureVersion::WitnessV0,
            UtxoAddressFormat::Standard | UtxoAddressFormat::CashAddress { .. } => {
                self.coin().as_ref().conf.signature_version
            },
        }
    }

    #[allow(clippy::result_large_err)]
    fn on_generating_transaction(&self) -> Result<(), MmError<WithdrawError>>;

    #[allow(clippy::result_large_err)]
    fn on_finishing(&self) -> Result<(), MmError<WithdrawError>>;

    async fn sign_tx(&self, unsigned_tx: TransactionInputSigner) -> Result<UtxoTx, MmError<WithdrawError>>;

    async fn build(self) -> WithdrawResult {
        let coin = self.coin();
        let ticker = coin.as_ref().conf.ticker.clone();
        let decimals = coin.as_ref().decimals;
        let req = self.request();

        let to = coin.address_from_str(&req.to).map_mm_err()?;

        // Generate unsigned transaction.
        self.on_generating_transaction()?;

        let script_pubkey = output_script(&to).map(|script| script.to_bytes())?;

        let _utxo_lock = UTXO_LOCK.lock().await;
        let (unspents, _) = coin
            .get_unspent_ordered_list(&self.sender_address())
            .await
            .map_mm_err()?;
        let (value, fee_policy) = if req.max {
            (
                unspents.iter().fold(0, |sum, unspent| sum + unspent.value),
                FeePolicy::DeductFromOutput(0),
            )
        } else {
            let value = sat_from_big_decimal(&req.amount, decimals).map_mm_err()?;
            (value, FeePolicy::SendExact)
        };
        let outputs = vec![TransactionOutput { value, script_pubkey }];

        let mut tx_builder = UtxoTxBuilder::new(coin)
            .await
            .with_from_address(self.sender_address())
            .add_available_inputs(unspents)
            .add_outputs(outputs)
            .with_fee_policy(fee_policy);

        match req.fee {
            Some(WithdrawFee::UtxoFixed { ref amount }) => {
                let fixed = sat_from_big_decimal(amount, decimals).map_mm_err()?;
                tx_builder = tx_builder.with_fee(ActualFeeRate::FixedPerKb(fixed));
            },
            Some(WithdrawFee::UtxoPerKbyte { ref amount }) => {
                let dynamic_fee_rate = sat_from_big_decimal(amount, decimals).map_mm_err()?;
                tx_builder = tx_builder.with_fee(ActualFeeRate::Dynamic(dynamic_fee_rate));
            },
            Some(ref fee_policy) => {
                let error = format!("Expected 'UtxoFixed' or 'UtxoPerKbyte' fee types, found {fee_policy:?}");
                return MmError::err(WithdrawError::InvalidFeePolicy(error));
            },
            None => (),
        };
        let (unsigned, data) = tx_builder
            .build()
            .await
            .mm_err(|gen_tx_error| WithdrawError::from_generate_tx_error(gen_tx_error, ticker.clone(), decimals))?;

        // Sign the `unsigned` transaction.
        let signed = self.sign_tx(unsigned).await?;

        // Finish by generating `TransactionDetails` from the signed transaction.
        self.on_finishing()?;

        let fee_details = UtxoFeeDetails {
            coin: Some(ticker.clone()),
            amount: big_decimal_from_sat(data.fee_amount as i64, decimals),
        };
        let tx_hex = match coin.addr_format() {
            UtxoAddressFormat::Segwit => serialize_with_flags(&signed, SERIALIZE_TRANSACTION_WITNESS).into(),
            _ => serialize(&signed).into(),
        };
        Ok(TransactionDetails {
            from: vec![self.sender_address_string()],
            to: vec![req.to.clone()],
            total_amount: big_decimal_from_sat(data.spent_by_me as i64, decimals),
            spent_by_me: big_decimal_from_sat(data.spent_by_me as i64, decimals),
            received_by_me: big_decimal_from_sat(data.received_by_me as i64, decimals),
            my_balance_change: big_decimal_from_sat(data.received_by_me as i64 - data.spent_by_me as i64, decimals),
            tx: TransactionData::new_signed(tx_hex, signed.hash().reversed().to_vec().to_tx_hash()),
            fee_details: Some(fee_details.into()),
            block_height: 0,
            coin: ticker,
            internal_id: vec![].into(),
            timestamp: now_sec(),
            kmd_rewards: data.kmd_rewards,
            transaction_type: Default::default(),
            memo: None,
        })
    }
}

pub struct InitUtxoWithdraw<Coin> {
    ctx: MmArc,
    coin: Coin,
    task_handle: WithdrawTaskHandleShared,
    req: WithdrawRequest,
    from_address: Address,
    /// Displayed [`InitUtxoWithdraw::from_address`].
    from_address_string: String,
    /// Derivation path from which [`InitUtxoWithdraw::from_address`] was derived.
    from_derivation_path: DerivationPath,
    /// Public key corresponding to [`InitUtxoWithdraw::from_address`].
    from_pubkey: PublicKey,
}

#[async_trait]
impl<Coin> UtxoWithdraw<Coin> for InitUtxoWithdraw<Coin>
where
    Coin: UtxoCommonOps + GetUtxoListOps + UtxoSignerOps,
{
    fn coin(&self) -> &Coin {
        &self.coin
    }

    fn sender_address(&self) -> Address {
        self.from_address.clone()
    }

    fn sender_address_string(&self) -> String {
        self.from_address_string.clone()
    }

    fn request(&self) -> &WithdrawRequest {
        &self.req
    }

    fn on_generating_transaction(&self) -> Result<(), MmError<WithdrawError>> {
        let amount_display = if self.req.max {
            "MAX".to_owned()
        } else {
            self.req.amount.to_string()
        };

        // Display the address from which we are trying to withdraw funds.
        info!(
            "Trying to withdraw {} {} from {} to {}",
            amount_display, self.req.coin, self.from_address_string, self.req.to,
        );

        self.task_handle
            .update_in_progress_status(WithdrawInProgressStatus::GeneratingTransaction)
            .map_mm_err()
    }

    fn on_finishing(&self) -> Result<(), MmError<WithdrawError>> {
        self.task_handle
            .update_in_progress_status(WithdrawInProgressStatus::Finishing)
            .map_mm_err()
    }

    async fn sign_tx(&self, unsigned_tx: TransactionInputSigner) -> Result<UtxoTx, MmError<WithdrawError>> {
        self.task_handle
            .update_in_progress_status(WithdrawInProgressStatus::SigningTransaction)
            .map_mm_err()?;

        let mut sign_params = UtxoSignTxParamsBuilder::new();

        // TODO refactor [`UtxoTxBuilder::build`] to return `SpendingInputInfo` and `SendingOutputInfo` within `AdditionalTxData`.
        sign_params.add_inputs_infos(
            unsigned_tx
                .inputs
                .iter()
                .map(|_input| match self.from_address.addr_format() {
                    AddressFormat::Segwit => SpendingInputInfo::P2WPKH {
                        address_derivation_path: self.from_derivation_path.clone(),
                        address_pubkey: self.from_pubkey,
                    },
                    AddressFormat::Standard | AddressFormat::CashAddress { .. } => SpendingInputInfo::P2PKH {
                        address_derivation_path: self.from_derivation_path.clone(),
                        address_pubkey: self.from_pubkey,
                    },
                }),
        );
        sign_params.add_outputs_infos(once(SendingOutputInfo {
            destination_address: OutputDestination::plain(self.req.to.clone()),
        }));
        match unsigned_tx.outputs.len() {
            // There is no change output.
            1 => (),
            // There is a change output.
            2 => {
                sign_params.add_outputs_infos(once(SendingOutputInfo {
                    destination_address: OutputDestination::change(
                        self.from_derivation_path.clone(),
                        self.from_address.addr_format().clone(),
                    ),
                }));
            },
            unexpected => {
                let error = format!("Unexpected number of outputs: {unexpected}");
                return MmError::err(WithdrawError::InternalError(error));
            },
        }

        sign_params
            .with_signature_version(self.signature_version())
            .with_unsigned_tx(unsigned_tx);
        let sign_params = sign_params.build().map_mm_err()?;

        let signed = match self.coin.as_ref().priv_key_policy {
            PrivKeyPolicy::Iguana(ref key_pair) => self
                .coin
                .sign_tx(sign_params, SignPolicy::WithKeyPair(key_pair))
                .await
                .map_mm_err()?,
            PrivKeyPolicy::HDWallet { .. } => {
                let from_key_pair = derive_hd_key_pair(self.coin(), &self.from_derivation_path).map_mm_err()?;
                self.coin()
                    .sign_tx(sign_params, SignPolicy::WithKeyPair(&from_key_pair))
                    .await
                    .map_mm_err()?
            },
            PrivKeyPolicy::Trezor => {
                let trezor_statuses = TrezorRequestStatuses {
                    on_button_request: WithdrawInProgressStatus::FollowHwDeviceInstructions,
                    on_pin_request: HwRpcTaskAwaitingStatus::EnterTrezorPin,
                    on_passphrase_request: HwRpcTaskAwaitingStatus::EnterTrezorPassphrase,
                    on_ready: WithdrawInProgressStatus::FollowHwDeviceInstructions,
                };
                let sign_processor = TrezorRpcTaskProcessor::new(self.task_handle.clone(), trezor_statuses);
                let sign_processor = Arc::new(sign_processor);
                let crypto_ctx = CryptoCtx::from_ctx(&self.ctx).map_mm_err()?;
                let hw_ctx = crypto_ctx
                    .hw_ctx()
                    .or_mm_err(|| WithdrawError::HwError(HwRpcError::NoTrezorDeviceAvailable))?;
                let trezor_session = hw_ctx.trezor(sign_processor).await.map_mm_err()?;
                self.task_handle
                    .update_in_progress_status(WithdrawInProgressStatus::WaitingForUserToConfirmSigning)
                    .map_mm_err()?;
                self.coin
                    .sign_tx(sign_params, SignPolicy::WithTrezor(trezor_session))
                    .await
                    .map_mm_err()?
            },
            #[cfg(target_arch = "wasm32")]
            PrivKeyPolicy::Metamask(_) => {
                return MmError::err(WithdrawError::UnsupportedError(
                    "`PrivKeyPolicy::Metamask` is not supported for UTXO coins!".to_string(),
                ))
            },
            PrivKeyPolicy::WalletConnect { .. } => {
                return MmError::err(WithdrawError::UnsupportedError(
                    "`PrivKeyPolicy::WalletConnect` is not supported for UTXO coins!".to_string(),
                ))
            },
        };

        Ok(signed)
    }
}

impl<Coin> InitUtxoWithdraw<Coin> {
    pub async fn new(
        ctx: MmArc,
        coin: Coin,
        req: WithdrawRequest,
        task_handle: WithdrawTaskHandleShared,
    ) -> Result<InitUtxoWithdraw<Coin>, MmError<WithdrawError>>
    where
        Coin: CoinWithDerivationMethod + GetWithdrawSenderAddress<Address = Address, Pubkey = PublicKey>,
    {
        let from = coin.get_withdraw_sender_address(&req).await?;
        let from_address_string = from.address.display_address().map_to_mm(WithdrawError::InternalError)?;

        let from_derivation_path = match from.derivation_path {
            Some(der_path) => der_path,
            // [`WithdrawSenderAddress::derivation_path`] is not set, but the coin is initialized with an HD wallet derivation method.
            None if coin.has_hd_wallet_derivation_method() => {
                let error = "Cannot determine 'from' address derivation path".to_owned();
                return MmError::err(WithdrawError::UnexpectedFromAddress(error));
            },
            // Temporary initialize the derivation path by default since this field is not used without Trezor.
            None => DerivationPath::default(),
        };

        Ok(InitUtxoWithdraw {
            ctx,
            coin,
            task_handle: task_handle.clone(),
            req,
            from_address: from.address,
            from_address_string,
            from_derivation_path,
            from_pubkey: from.pubkey,
        })
    }
}

pub struct StandardUtxoWithdraw<Coin> {
    coin: Coin,
    req: WithdrawRequest,
    key_pair: KeyPair,
    from_address: Address,
    from_address_string: String,
}

#[async_trait]
impl<Coin> UtxoWithdraw<Coin> for StandardUtxoWithdraw<Coin>
where
    Coin: UtxoCommonOps + GetUtxoListOps,
{
    fn coin(&self) -> &Coin {
        &self.coin
    }

    fn sender_address(&self) -> Address {
        self.from_address.clone()
    }

    fn sender_address_string(&self) -> String {
        self.from_address_string.clone()
    }

    fn request(&self) -> &WithdrawRequest {
        &self.req
    }

    fn on_generating_transaction(&self) -> Result<(), MmError<WithdrawError>> {
        Ok(())
    }

    fn on_finishing(&self) -> Result<(), MmError<WithdrawError>> {
        Ok(())
    }

    async fn sign_tx(&self, unsigned_tx: TransactionInputSigner) -> Result<UtxoTx, MmError<WithdrawError>> {
        Ok(with_key_pair::sign_tx(
            unsigned_tx,
            &self.key_pair,
            self.signature_version(),
            self.coin.as_ref().conf.fork_id,
        )
        .map_mm_err()?)
    }
}

impl<Coin> StandardUtxoWithdraw<Coin>
where
    Coin: AsRef<UtxoCoinFields>
        + MarketCoinOps
        + CoinWithDerivationMethod
        + GetWithdrawSenderAddress<Address = Address, Pubkey = PublicKey>,
{
    #[allow(clippy::result_large_err)]
    pub async fn new(coin: Coin, req: WithdrawRequest) -> Result<Self, MmError<WithdrawError>> {
        let from = coin.get_withdraw_sender_address(&req).await?;
        let from_address_string = from.address.display_address().map_to_mm(WithdrawError::InternalError)?;

        let key_pair = match from.derivation_path {
            Some(der_path) => derive_hd_key_pair(&coin, &der_path).map_mm_err()?,
            // [`WithdrawSenderAddress::derivation_path`] is not set, but the coin is initialized with an HD wallet derivation method.
            None if coin.has_hd_wallet_derivation_method() => {
                let error = "Cannot determine 'from' address derivation path".to_owned();
                return MmError::err(WithdrawError::UnexpectedFromAddress(error));
            },
            None => *coin.as_ref().priv_key_policy.activated_key_or_err().map_mm_err()?,
        };

        Ok(StandardUtxoWithdraw {
            coin,
            req,
            key_pair,
            from_address: from.address,
            from_address_string,
        })
    }
}
