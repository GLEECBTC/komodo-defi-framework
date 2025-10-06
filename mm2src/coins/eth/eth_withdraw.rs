use super::{
    checksum_address, u256_from_big_decimal, u256_to_big_decimal, ChainSpec, EthCoinType, EthDerivationMethod,
    EthPrivKeyPolicy, Public, WithdrawError, WithdrawRequest, WithdrawResult, ERC20_CONTRACT, H160, H256,
};
use crate::eth::wallet_connect::WcEthTxParams;
use crate::eth::{
    calc_total_fee, get_eth_gas_details_from_withdraw_fee, tx_builder_with_pay_for_gas_option,
    tx_type_from_pay_for_gas_option, Action, Address, EthTxFeeDetails, KeyPair, PayForGasOption, SignedEthTx,
    TransactionWrapper, UnSignedEthTxBuilder, ETH_RPC_REQUEST_TIMEOUT_S,
};
use crate::hd_wallet::{HDAddressSelector, HDCoinWithdrawOps, HDWalletOps, WithdrawSenderAddress};
use crate::rpc_command::init_withdraw::{WithdrawInProgressStatus, WithdrawTaskHandleShared};
use crate::{
    BytesJson, CoinWithDerivationMethod, EthCoin, GetWithdrawSenderAddress, PrivKeyPolicy, TransactionData,
    TransactionDetails,
};
use async_trait::async_trait;
use bip32::DerivationPath;
use common::custom_futures::timeout::FutureTimerExt;
use common::now_sec;
use crypto::hw_rpc_task::HwRpcTaskAwaitingStatus;
use crypto::trezor::trezor_rpc_task::{TrezorRequestStatuses, TrezorRpcTaskProcessor};
use crypto::{CryptoCtx, HwRpcError};
use ethabi::Token;
use futures::compat::Future01CompatExt;
use kdf_walletconnect::{WalletConnectCtx, WalletConnectOps};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::map_mm_error::MapMmError;
use mm2_err_handle::mm_error::MmResult;
use mm2_err_handle::prelude::{MapToMmResult, MmError, MmResultExt, OrMmError};
use std::ops::Deref;
use std::sync::Arc;
#[cfg(target_arch = "wasm32")]
use web3::types::TransactionRequest;

/// `EthWithdraw` trait provides methods for withdrawing Ethereum and ERC20 tokens.
/// This allows different implementations of withdrawal logic for different types of wallets.
#[async_trait]
pub trait EthWithdraw
where
    Self: Sized + Sync,
{
    /// A getter for the coin that implements this trait.
    fn coin(&self) -> &EthCoin;

    /// A getter for the withdrawal request.
    fn request(&self) -> &WithdrawRequest;

    /// Executes the logic that should be performed just before generating a transaction.
    #[allow(clippy::result_large_err)]
    fn on_generating_transaction(&self) -> Result<(), MmError<WithdrawError>>;

    /// Executes the logic that should be performed just before finishing the withdrawal.
    #[allow(clippy::result_large_err)]
    fn on_finishing(&self) -> Result<(), MmError<WithdrawError>>;

    /// Signs the transaction with a Trezor hardware wallet.
    async fn sign_tx_with_trezor(
        &self,
        derivation_path: &DerivationPath,
        unsigned_tx: &TransactionWrapper,
    ) -> Result<SignedEthTx, MmError<WithdrawError>>;

    /// Transforms the `from` parameter of the withdrawal request into an address.
    async fn get_from_address(&self, req: &WithdrawRequest) -> Result<H160, MmError<WithdrawError>> {
        let coin = self.coin();
        match req.from {
            Some(_) => Ok(coin.get_withdraw_sender_address(req).await.map_mm_err()?.address),
            None => Ok(coin.derivation_method.single_addr_or_err().await.map_mm_err()?),
        }
    }

    /// Gets the key pair for the address from which the withdrawal is made.
    #[allow(clippy::result_large_err)]
    fn get_key_pair(&self, req: &WithdrawRequest) -> Result<KeyPair, MmError<WithdrawError>> {
        let coin = self.coin();
        if coin.priv_key_policy.is_trezor() {
            return MmError::err(WithdrawError::InternalError("no keypair for hw wallet".to_owned()));
        }

        match req.from {
            Some(ref from) => {
                let derivation_path = self.get_from_derivation_path(from)?;
                let raw_priv_key = coin
                    .priv_key_policy
                    .hd_wallet_derived_priv_key_or_err(&derivation_path)
                    .map_mm_err()?;
                KeyPair::from_secret_slice(raw_priv_key.as_slice())
                    .map_to_mm(|e| WithdrawError::InternalError(e.to_string()))
            },
            None => coin
                .priv_key_policy
                .activated_key_or_err()
                .mm_err(|e| WithdrawError::InternalError(e.to_string()))
                .cloned(),
        }
    }

    /// Gets the derivation path for the address from which the withdrawal is made using the `from` parameter.
    #[allow(clippy::result_large_err)]
    fn get_from_derivation_path(&self, from: &HDAddressSelector) -> Result<DerivationPath, MmError<WithdrawError>> {
        let coin = self.coin();
        let path_to_coin = &coin
            .deref()
            .derivation_method
            .hd_wallet_or_err()
            .map_mm_err()?
            .derivation_path;
        let path_to_address = from
            .to_address_path(path_to_coin.coin_type())
            .mm_err(|err| WithdrawError::UnexpectedFromAddress(err.to_string()))
            .map_mm_err()?;
        let derivation_path = path_to_address.to_derivation_path(path_to_coin).map_mm_err()?;
        Ok(derivation_path)
    }

    /// Gets the derivation path for the address from which the withdrawal is made using the withdrawal request.
    async fn get_withdraw_derivation_path(
        &self,
        req: &WithdrawRequest,
    ) -> Result<DerivationPath, MmError<WithdrawError>> {
        let coin = self.coin();
        match req.from {
            Some(ref from) => self.get_from_derivation_path(from),
            None => {
                let default_hd_address = &coin
                    .deref()
                    .derivation_method
                    .hd_wallet_or_err()
                    .map_mm_err()?
                    .get_enabled_address()
                    .await
                    .ok_or_else(|| WithdrawError::InternalError("no enabled address".to_owned()))?;
                Ok(default_hd_address.derivation_path.clone())
            },
        }
    }

    /// Signs the transaction and returns the transaction hash and the signed transaction.
    async fn sign_withdraw_tx(
        &self,
        req: &WithdrawRequest,
        unsigned_tx: TransactionWrapper,
    ) -> Result<(H256, BytesJson), MmError<WithdrawError>> {
        let coin = self.coin();
        match coin.priv_key_policy {
            EthPrivKeyPolicy::Iguana(_) | EthPrivKeyPolicy::HDWallet { .. } => {
                let key_pair = self.get_key_pair(req)?;
                let chain_id = match coin.chain_spec {
                    ChainSpec::Evm { chain_id } => chain_id,
                    // Todo: Tron have different transaction signing algorithm, we should probably have a trait abstracting both
                    ChainSpec::Tron { .. } => {
                        return MmError::err(WithdrawError::InternalError(
                            "Tron is not supported for withdraw yet".to_owned(),
                        ))
                    },
                };
                let signed = unsigned_tx.sign(key_pair.secret(), Some(chain_id))?;
                let bytes = rlp::encode(&signed);

                Ok((signed.tx_hash(), BytesJson::from(bytes.to_vec())))
            },
            EthPrivKeyPolicy::Trezor => {
                let derivation_path = self.get_withdraw_derivation_path(req).await?;
                let signed = self.sign_tx_with_trezor(&derivation_path, &unsigned_tx).await?;
                let bytes = rlp::encode(&signed);
                Ok((signed.tx_hash(), BytesJson::from(bytes.to_vec())))
            },
            EthPrivKeyPolicy::WalletConnect { .. } => {
                MmError::err(WithdrawError::InternalError("invalid policy".to_owned()))
            },
            #[cfg(target_arch = "wasm32")]
            EthPrivKeyPolicy::Metamask(_) => MmError::err(WithdrawError::InternalError("invalid policy".to_owned())),
        }
    }

    /// Sends the transaction and returns the transaction hash and the signed transaction.
    /// This method should only be used when withdrawing using an external wallet like MetaMask.
    #[cfg(target_arch = "wasm32")]
    async fn send_withdraw_tx(
        &self,
        req: &WithdrawRequest,
        tx_to_send: TransactionRequest,
    ) -> Result<(H256, BytesJson), MmError<WithdrawError>> {
        let coin = self.coin();
        match coin.priv_key_policy {
            EthPrivKeyPolicy::Metamask(_) => {
                if !req.broadcast {
                    let error =
                        "Set 'broadcast' to generate, sign and broadcast a transaction with MetaMask".to_string();
                    return MmError::err(WithdrawError::BroadcastExpected(error));
                }

                // Wait for 10 seconds for the transaction to appear on the RPC node.
                let wait_rpc_timeout = 10;
                let check_every = 1.;

                // Please note that this method may take a long time
                // due to `wallet_switchEthereumChain` and `eth_sendTransaction` requests.
                let tx_hash = coin.send_transaction(tx_to_send).await?;

                let signed_tx = coin
                    .wait_for_tx_appears_on_rpc(tx_hash, wait_rpc_timeout, check_every)
                    .await
                    .map_mm_err()?;
                let tx_hex = signed_tx
                    .map(|signed_tx| BytesJson::from(rlp::encode(&signed_tx).to_vec()))
                    // Return an empty `tx_hex` if the transaction is still not appeared on the RPC node.
                    .unwrap_or_default();
                Ok((tx_hash, tx_hex))
            },
            EthPrivKeyPolicy::Iguana(_)
            | EthPrivKeyPolicy::HDWallet { .. }
            | EthPrivKeyPolicy::Trezor
            | EthPrivKeyPolicy::WalletConnect { .. } => {
                MmError::err(WithdrawError::InternalError("invalid policy".to_owned()))
            },
        }
    }

    /// Builds the withdrawal transaction and returns the transaction details.
    async fn build(self) -> WithdrawResult {
        let coin = self.coin();
        let ticker = coin.deref().ticker.clone();
        let req = self.request().clone();

        let to_addr = coin
            .address_from_str(&req.to)
            .map_to_mm(WithdrawError::InvalidAddress)?;
        let my_address = self.get_from_address(&req).await?;

        self.on_generating_transaction()?;

        let my_balance = coin.address_balance(my_address).compat().await.map_mm_err()?;
        let my_balance_dec = u256_to_big_decimal(my_balance, coin.decimals).map_mm_err()?;

        let (mut wei_amount, dec_amount) = if req.max {
            (my_balance, my_balance_dec.clone())
        } else {
            let wei_amount = u256_from_big_decimal(&req.amount, coin.decimals).map_mm_err()?;
            (wei_amount, req.amount.clone())
        };
        if wei_amount > my_balance {
            return MmError::err(WithdrawError::NotSufficientBalance {
                coin: coin.ticker.clone(),
                available: my_balance_dec.clone(),
                required: dec_amount,
            });
        };
        let (mut eth_value, data, call_addr, fee_coin) = match &coin.coin_type {
            EthCoinType::Eth => (wei_amount, vec![], to_addr, ticker.as_str()),
            EthCoinType::Erc20 { platform, token_addr } => {
                let function = ERC20_CONTRACT.function("transfer")?;
                let data = function.encode_input(&[Token::Address(to_addr), Token::Uint(wei_amount)])?;
                (0.into(), data, *token_addr, platform.as_str())
            },
            EthCoinType::Nft { .. } => return MmError::err(WithdrawError::NftProtocolNotSupported),
        };
        let eth_value_dec = u256_to_big_decimal(eth_value, coin.decimals).map_mm_err()?;

        let (gas, pay_for_gas_option) = get_eth_gas_details_from_withdraw_fee(
            coin,
            req.fee.clone(),
            eth_value,
            data.clone().into(),
            my_address,
            call_addr,
            req.max,
        )
        .await
        .map_mm_err()?;
        let total_fee = calc_total_fee(gas, &pay_for_gas_option).map_mm_err()?;
        let total_fee_dec = u256_to_big_decimal(total_fee, coin.decimals).map_mm_err()?;

        if req.max && coin.coin_type == EthCoinType::Eth {
            if eth_value < total_fee || wei_amount < total_fee {
                return MmError::err(WithdrawError::AmountTooLow {
                    amount: eth_value_dec,
                    threshold: total_fee_dec,
                });
            }
            eth_value -= total_fee;
            wei_amount -= total_fee;
        };
        drop_mutability!(eth_value);
        drop_mutability!(wei_amount);

        let (tx_hash, tx_hex) = match coin.priv_key_policy {
            EthPrivKeyPolicy::Iguana(_) | EthPrivKeyPolicy::HDWallet { .. } | EthPrivKeyPolicy::Trezor => {
                let address_lock = coin.get_address_lock(my_address).await;
                let _nonce_lock = address_lock.lock().await;
                let (nonce, _) = coin
                    .clone()
                    .get_addr_nonce(my_address)
                    .compat()
                    .timeout(ETH_RPC_REQUEST_TIMEOUT_S)
                    .await?
                    .map_to_mm(WithdrawError::Transport)?;

                let tx_type = tx_type_from_pay_for_gas_option!(pay_for_gas_option);
                if !coin.is_tx_type_supported(&tx_type) {
                    return MmError::err(WithdrawError::TxTypeNotSupported);
                }
                let tx_builder =
                    UnSignedEthTxBuilder::new(tx_type, nonce, gas, Action::Call(call_addr), eth_value, data);
                let tx_builder = tx_builder_with_pay_for_gas_option(coin, tx_builder, &pay_for_gas_option)?;
                let unsigned_tx = tx_builder
                    .build()
                    .map_to_mm(|e| WithdrawError::InternalError(e.to_string()))?;
                self.sign_withdraw_tx(&req, unsigned_tx).await?
            },
            #[cfg(target_arch = "wasm32")]
            EthPrivKeyPolicy::Metamask(_) => {
                let gas_price = pay_for_gas_option.get_gas_price();
                let (max_fee_per_gas, max_priority_fee_per_gas) = pay_for_gas_option.get_fee_per_gas();
                let tx_to_send = TransactionRequest {
                    from: my_address,
                    to: Some(to_addr),
                    gas: Some(gas),
                    gas_price,
                    max_fee_per_gas,
                    max_priority_fee_per_gas,
                    value: Some(eth_value),
                    data: Some(data.into()),
                    nonce: None,
                    ..TransactionRequest::default()
                };
                self.send_withdraw_tx(&req, tx_to_send).await?
            },
            EthPrivKeyPolicy::WalletConnect { .. } => {
                let ctx = MmArc::from_weak(&coin.ctx).expect("No context");
                let wc = WalletConnectCtx::from_ctx(&ctx)
                    .expect("TODO: handle error when enable kdf initialization without key.");
                // Todo: Tron will have to be set with `ChainSpec::Evm` to work with walletconnect.
                // This means setting the protocol as `ETH` in coin config and having a different coin for this mode.
                let chain_id = coin.chain_spec.chain_id().ok_or(WithdrawError::UnsupportedError(
                    "WalletConnect needs chain_id to be set".to_owned(),
                ))?;
                let gas_price = pay_for_gas_option.get_gas_price();
                let (max_fee_per_gas, max_priority_fee_per_gas) = pay_for_gas_option.get_fee_per_gas();
                // TODO: we should get _nonce_lock here (when WalletConnect is supported for swaps)
                let (nonce, _) = coin
                    .clone()
                    .get_addr_nonce(my_address)
                    .compat()
                    .timeout(ETH_RPC_REQUEST_TIMEOUT_S)
                    .await?
                    .map_to_mm(WithdrawError::Transport)?;
                let params = WcEthTxParams {
                    gas,
                    nonce,
                    data: &data,
                    my_address,
                    action: Action::Call(call_addr),
                    value: eth_value,
                    gas_price,
                    chain_id,
                    max_fee_per_gas,
                    max_priority_fee_per_gas,
                };

                let (tx, bytes) = if req.broadcast {
                    self.coin()
                        .wc_send_tx(&wc, params)
                        .await
                        .mm_err(|err| WithdrawError::SigningError(err.to_string()))?
                } else {
                    self.coin()
                        .wc_sign_tx(&wc, params)
                        .await
                        .mm_err(|err| WithdrawError::SigningError(err.to_string()))?
                };

                (tx.tx_hash(), bytes)
            },
        };

        self.on_finishing()?;
        let tx_hash_bytes = BytesJson::from(tx_hash.0.to_vec());
        let tx_hash_str = format!("{tx_hash_bytes:02x}");

        let amount_decimal = u256_to_big_decimal(wei_amount, coin.decimals).map_mm_err()?;
        let mut spent_by_me = amount_decimal.clone();
        let received_by_me = if to_addr == my_address {
            amount_decimal.clone()
        } else {
            0.into()
        };
        let fee_details = EthTxFeeDetails::new(gas, pay_for_gas_option, fee_coin).map_mm_err()?;
        if coin.coin_type == EthCoinType::Eth {
            spent_by_me += &fee_details.total_fee;
        }
        Ok(TransactionDetails {
            to: vec![checksum_address(&format!("{to_addr:#02x}"))],
            from: vec![checksum_address(&format!("{my_address:#02x}"))],
            total_amount: amount_decimal,
            my_balance_change: &received_by_me - &spent_by_me,
            spent_by_me,
            received_by_me,
            tx: TransactionData::new_signed(tx_hex, tx_hash_str),
            block_height: 0,
            fee_details: Some(fee_details.into()),
            coin: coin.ticker.clone(),
            internal_id: vec![].into(),
            timestamp: now_sec(),
            kmd_rewards: None,
            transaction_type: Default::default(),
            memo: None,
        })
    }
}

/// Eth withdraw version with user interaction support
pub struct InitEthWithdraw {
    ctx: MmArc,
    coin: EthCoin,
    task_handle: WithdrawTaskHandleShared,
    req: WithdrawRequest,
}

#[async_trait]
impl EthWithdraw for InitEthWithdraw {
    fn coin(&self) -> &EthCoin {
        &self.coin
    }

    fn request(&self) -> &WithdrawRequest {
        &self.req
    }

    fn on_generating_transaction(&self) -> Result<(), MmError<WithdrawError>> {
        self.task_handle
            .update_in_progress_status(WithdrawInProgressStatus::GeneratingTransaction)
            .map_mm_err()
    }

    fn on_finishing(&self) -> Result<(), MmError<WithdrawError>> {
        self.task_handle
            .update_in_progress_status(WithdrawInProgressStatus::Finishing)
            .map_mm_err()
    }

    async fn sign_tx_with_trezor(
        &self,
        derivation_path: &DerivationPath,
        unsigned_tx: &TransactionWrapper,
    ) -> Result<SignedEthTx, MmError<WithdrawError>> {
        let coin = self.coin();
        let crypto_ctx = CryptoCtx::from_ctx(&self.ctx).map_mm_err()?;
        let hw_ctx = crypto_ctx
            .hw_ctx()
            .or_mm_err(|| WithdrawError::HwError(HwRpcError::NoTrezorDeviceAvailable))?;
        let trezor_statuses = TrezorRequestStatuses {
            on_button_request: WithdrawInProgressStatus::FollowHwDeviceInstructions,
            on_pin_request: HwRpcTaskAwaitingStatus::EnterTrezorPin,
            on_passphrase_request: HwRpcTaskAwaitingStatus::EnterTrezorPassphrase,
            on_ready: WithdrawInProgressStatus::FollowHwDeviceInstructions,
        };
        let sign_processor = TrezorRpcTaskProcessor::new(self.task_handle.clone(), trezor_statuses);
        let sign_processor = Arc::new(sign_processor);
        let mut trezor_session = hw_ctx.trezor(sign_processor).await.map_mm_err()?;
        let chain_id = match coin.chain_spec {
            ChainSpec::Evm { chain_id } => chain_id,
            // Todo: Add support for Tron signing with Trezor
            ChainSpec::Tron { .. } => {
                return MmError::err(WithdrawError::InternalError(
                    "Tron is not supported for withdraw yet".to_owned(),
                ))
            },
        };
        let unverified_tx = trezor_session
            .sign_eth_tx(derivation_path, unsigned_tx, chain_id)
            .await
            .map_mm_err()?;

        Ok(SignedEthTx::new(unverified_tx).map_to_mm(|err| WithdrawError::InternalError(err.to_string()))?)
    }
}

#[allow(clippy::result_large_err)]
impl InitEthWithdraw {
    pub fn new(
        ctx: MmArc,
        coin: EthCoin,
        req: WithdrawRequest,
        task_handle: WithdrawTaskHandleShared,
    ) -> Result<InitEthWithdraw, MmError<WithdrawError>> {
        Ok(InitEthWithdraw {
            ctx,
            coin,
            task_handle,
            req,
        })
    }
}

/// Simple eth withdraw version without user interaction support
pub struct StandardEthWithdraw {
    coin: EthCoin,
    req: WithdrawRequest,
}

#[async_trait]
impl EthWithdraw for StandardEthWithdraw {
    fn coin(&self) -> &EthCoin {
        &self.coin
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

    async fn sign_tx_with_trezor(
        &self,
        _derivation_path: &DerivationPath,
        _unsigned_tx: &TransactionWrapper,
    ) -> Result<SignedEthTx, MmError<WithdrawError>> {
        async {
            Err(MmError::new(WithdrawError::UnsupportedError(String::from(
                "Trezor not supported for legacy RPC",
            ))))
        }
        .await
    }
}

#[allow(clippy::result_large_err)]
impl StandardEthWithdraw {
    pub fn new(coin: EthCoin, req: WithdrawRequest) -> Result<StandardEthWithdraw, MmError<WithdrawError>> {
        Ok(StandardEthWithdraw { coin, req })
    }
}

#[async_trait]
impl GetWithdrawSenderAddress for EthCoin {
    type Address = Address;
    type Pubkey = Public;

    async fn get_withdraw_sender_address(
        &self,
        req: &WithdrawRequest,
    ) -> MmResult<WithdrawSenderAddress<Self::Address, Self::Pubkey>, WithdrawError> {
        eth_get_withdraw_from_address(self, req).await
    }
}

async fn eth_get_withdraw_from_address(
    coin: &EthCoin,
    req: &WithdrawRequest,
) -> MmResult<WithdrawSenderAddress<Address, Public>, WithdrawError> {
    match coin.derivation_method() {
        EthDerivationMethod::SingleAddress(my_address) => eth_get_withdraw_iguana_sender(coin, req, my_address),
        EthDerivationMethod::HDWallet(hd_wallet) => {
            let from = req.from.clone().or_mm_err(|| WithdrawError::FromAddressNotFound)?;
            coin.get_withdraw_hd_sender(hd_wallet, &from)
                .await
                .mm_err(WithdrawError::from)
        },
    }
}

#[allow(clippy::result_large_err)]
fn eth_get_withdraw_iguana_sender(
    coin: &EthCoin,
    req: &WithdrawRequest,
    my_address: &Address,
) -> MmResult<WithdrawSenderAddress<Address, Public>, WithdrawError> {
    if req.from.is_some() {
        let error = "'from' is not supported if the coin is initialized with an Iguana private key";
        return MmError::err(WithdrawError::UnexpectedFromAddress(error.to_owned()));
    }

    let pubkey = match coin.priv_key_policy {
        PrivKeyPolicy::Iguana(ref key_pair) => key_pair.public(),
        _ => return MmError::err(WithdrawError::InternalError("not iguana private key policy".to_owned())),
    };

    Ok(WithdrawSenderAddress {
        address: *my_address,
        pubkey: *pubkey,
        derivation_path: None,
    })
}
