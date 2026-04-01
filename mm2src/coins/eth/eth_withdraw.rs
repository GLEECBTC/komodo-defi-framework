use super::{
    u256_from_big_decimal, u256_to_big_decimal, ChainSpec, ChainTaggedAddress, EthCoinType, EthDerivationMethod,
    EthPrivKeyPolicy, Public, WithdrawError, WithdrawRequest, WithdrawResult, ERC20_CONTRACT, H256,
};
use crate::eth::tron::fee::DestAccountState;
use crate::eth::tron::sign::sign_tron_transaction;
use crate::eth::tron::withdraw::{
    build_tron_trc20_withdraw, build_tron_trx_withdraw, validate_tron_fee_policy, TronWithdrawContext,
};
use crate::eth::tron::TronAddress;
use crate::eth::wallet_connect::WcEthTxParams;
use crate::eth::{
    calc_total_fee, get_eth_gas_details_from_withdraw_fee, tx_builder_with_pay_for_gas_option,
    tx_type_from_pay_for_gas_option, Action, EthTxFeeDetails, KeyPair, PayForGasOption, SignedEthTx,
    TransactionWrapper, UnSignedEthTxBuilder, ETH_RPC_REQUEST_TIMEOUT_S,
};
use crate::hd_wallet::{DisplayAddress, HDAddressSelector, HDCoinWithdrawOps, HDWalletOps, WithdrawSenderAddress};
use crate::rpc_command::init_withdraw::{WithdrawInProgressStatus, WithdrawTaskHandleShared};
use crate::BigDecimal;
use crate::{
    BytesJson, CoinWithDerivationMethod, EthCoin, GetWithdrawSenderAddress, PrivKeyPolicy, TransactionData,
    TransactionDetails, TxFeeDetails,
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
use prost::Message;
use std::ops::Deref;
use std::sync::Arc;
#[cfg(target_arch = "wasm32")]
use web3::types::TransactionRequest;

/// Format an `H256` tx hash as a lowercase hex string.
fn format_tx_hash(hash: H256) -> String {
    let bytes = BytesJson::from(hash.0.to_vec());
    format!("{bytes:02x}")
}

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

    /// Assembles the final `TransactionDetails` from a signed transaction.
    ///
    /// Shared by both EVM and TRON withdraw paths to avoid duplicating the
    /// spent/received calculation and struct construction.
    #[allow(clippy::result_large_err)]
    fn build_transaction_details(
        &self,
        from_tagged: &ChainTaggedAddress,
        to_tagged: &ChainTaggedAddress,
        tx: TransactionData,
        amount: ethabi::ethereum_types::U256,
        total_fee: &BigDecimal,
        fee_details: TxFeeDetails,
    ) -> WithdrawResult {
        let coin = self.coin();

        let amount_decimal = u256_to_big_decimal(amount, coin.decimals).map_mm_err()?;
        let mut spent_by_me = amount_decimal.clone();
        let received_by_me = if to_tagged.inner() == from_tagged.inner() {
            amount_decimal.clone()
        } else {
            0.into()
        };
        // For native coins (ETH / TRX), the fee is paid from the same balance.
        if coin.coin_type == EthCoinType::Eth {
            spent_by_me += total_fee;
        }

        Ok(TransactionDetails {
            to: vec![to_tagged.display_address()],
            from: vec![from_tagged.display_address()],
            total_amount: amount_decimal,
            my_balance_change: &received_by_me - &spent_by_me,
            spent_by_me,
            received_by_me,
            tx,
            block_height: 0,
            fee_details: Some(fee_details),
            coin: coin.ticker.clone(),
            internal_id: vec![].into(),
            timestamp: now_sec(),
            kmd_rewards: None,
            transaction_type: Default::default(),
            memo: None,
        })
    }

    /// - This returns `ChainTaggedAddress` so user-facing formatting is always chain-aware (EVM checksum vs TRON base58).
    /// - Convert to raw with `.inner()` when performing RPC calls / tx building.
    async fn get_from_address(&self, req: &WithdrawRequest) -> Result<ChainTaggedAddress, MmError<WithdrawError>> {
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
                let chain_id = coin.chain_spec.chain_id().ok_or_else(|| {
                    WithdrawError::InternalError(
                        "sign_withdraw_tx must not be called for TRON; use TRON branch in build()".to_owned(),
                    )
                })?;
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

    /// Builds a TRON withdrawal transaction.
    ///
    /// Handles the full TRON withdraw pipeline: fee policy validation, TRON RPC calls
    /// for TAPOS + resources + prices, tx building, signing, and fee details construction.
    async fn build_tron_withdraw(
        &self,
        from_tagged: ChainTaggedAddress,
        to_tagged: ChainTaggedAddress,
    ) -> WithdrawResult {
        let coin = self.coin();
        let ticker = &coin.ticker;
        let req = self.request();

        // 1. Validate TRON-specific request constraints
        validate_tron_fee_policy(&req.fee)?;
        if req.memo.is_some() {
            return MmError::err(WithdrawError::UnsupportedError(
                "Memo is not yet supported for TRON withdraw (TRON charges 1 TRX burn fee for memo)".to_owned(),
            ));
        }

        // 2. Validate key policy (only Iguana/HDWallet supported for TRON MVP)
        match coin.priv_key_policy {
            EthPrivKeyPolicy::Iguana(_) | EthPrivKeyPolicy::HDWallet { .. } => {},
            EthPrivKeyPolicy::Trezor => {
                return MmError::err(WithdrawError::UnsupportedError(
                    "Trezor is not supported for TRON withdraw".to_owned(),
                ))
            },
            EthPrivKeyPolicy::WalletConnect { .. } => {
                return MmError::err(WithdrawError::UnsupportedError(
                    "WalletConnect is not supported for TRON withdraw".to_owned(),
                ))
            },
            #[cfg(target_arch = "wasm32")]
            EthPrivKeyPolicy::Metamask(_) => {
                return MmError::err(WithdrawError::UnsupportedError(
                    "MetaMask is not supported for TRON withdraw".to_owned(),
                ))
            },
        }

        // 3. Get TRON RPC client and fetch balance
        let tron = coin
            .0
            .tron_rpc()
            .ok_or_else(|| WithdrawError::InternalError("TRON RPC client is not initialized".to_owned()))?;

        let my_balance = coin.address_balance(from_tagged).compat().await.map_mm_err()?;
        let my_balance_dec = u256_to_big_decimal(my_balance, coin.decimals).map_mm_err()?;

        if req.max && my_balance.is_zero() {
            return MmError::err(WithdrawError::ZeroBalanceToWithdrawMax);
        }

        let amount_base_units = if req.max {
            my_balance
        } else {
            let amount = u256_from_big_decimal(&req.amount, coin.decimals).map_mm_err()?;
            if amount > my_balance {
                let required_dec = u256_to_big_decimal(amount, coin.decimals).map_mm_err()?;
                return MmError::err(WithdrawError::NotSufficientBalance {
                    coin: ticker.to_owned(),
                    available: my_balance_dec,
                    required: required_dec,
                });
            }
            amount
        };

        // 4. Convert addresses to TRON format
        let from_tron = TronAddress::from(from_tagged.inner());
        let to_tron = TronAddress::from(to_tagged.inner());

        // 5. Fetch TAPOS block data (timestamp/expiration are derived inside the tx builders).
        let block_data = tron.get_block_for_tapos().await.map_mm_err()?;

        // 6. Fetch account resources and chain prices.
        let resources = tron.get_account_resource(&from_tron).await.map_mm_err()?;
        let prices = tron.get_chain_prices().await.map_mm_err()?;

        // 7. Compute destination state — only native TRX transfers pay system-contract
        //    account creation fees.
        let dest_state = match &coin.coin_type {
            EthCoinType::Eth => {
                let dest_account = tron.get_account(&to_tron).await.map_mm_err()?;
                if dest_account.exists_meaningfully() {
                    DestAccountState::Activated
                } else {
                    // Validate that the RPC node actually provided account creation fee params.
                    // A zero value here means the node omitted them (defaulted), which would
                    // silently underprice the transaction and cause broadcast failure.
                    if prices.create_new_account_fee_sun == 0 {
                        return MmError::err(WithdrawError::InternalError(
                            "TRON node did not provide CreateNewAccountFeeInSystemContract chain parameter; \
                             cannot estimate fees for transfer to unactivated address"
                                .to_owned(),
                        ));
                    }
                    DestAccountState::NewAccount {
                        creation_fee_sun: prices.create_new_account_fee_sun,
                        bandwidth_fallback_sun: prices.create_account_bandwidth_fee_sun,
                        bandwidth_rate: prices.create_new_account_bandwidth_rate,
                    }
                }
            },
            // No need for destination account to be activated in TRC20 transfers, so we just assume
            // it is activated and calculate the fees accordingly.
            EthCoinType::Erc20 { .. } => DestAccountState::Activated,
            EthCoinType::Nft { .. } => DestAccountState::Activated,
        };

        // 9. Build tx, estimate fees — branching on coin type
        let withdraw_ctx = TronWithdrawContext {
            from: &from_tron,
            to: &to_tron,
            block_data: &block_data,
            resources,
            prices,
            fee_coin: ticker,
            expiration_seconds: req.expiration_seconds,
            dest_state,
        };
        let (raw, tron_fee_details, final_amount) = match &coin.coin_type {
            EthCoinType::Eth => {
                build_tron_trx_withdraw(&withdraw_ctx, amount_base_units, my_balance, &my_balance_dec, req.max)?
            },
            EthCoinType::Erc20 {
                token_addr, platform, ..
            } => {
                let contract_tron = TronAddress::from(*token_addr);
                let trc20_ctx = TronWithdrawContext {
                    fee_coin: platform.as_str(),
                    ..withdraw_ctx
                };
                build_tron_trc20_withdraw(&trc20_ctx, tron, &contract_tron, amount_base_units).await?
            },
            EthCoinType::Nft { .. } => {
                return MmError::err(WithdrawError::ProtocolNotSupported(
                    "NFT withdraw is not supported for TRON".to_owned(),
                ))
            },
        };

        // 8. Sign the transaction
        let key_pair = self.get_key_pair(req)?;
        let (tx_hash, signed_tx) =
            sign_tron_transaction(&raw, key_pair.secret()).map_to_mm(|e| WithdrawError::SigningError(e.to_string()))?;
        let signed_tx_bytes = BytesJson::from(signed_tx.encode_to_vec());

        // 9. Build TransactionDetails
        self.on_finishing()?;
        let tx = TransactionData::new_signed(signed_tx_bytes, format_tx_hash(tx_hash));
        let total_fee = tron_fee_details.total_fee.clone();
        self.build_transaction_details(
            &from_tagged,
            &to_tagged,
            tx,
            final_amount,
            &total_fee,
            tron_fee_details.into(),
        )
    }

    /// Builds the withdrawal transaction and returns the transaction details.
    async fn build(self) -> WithdrawResult {
        let coin = self.coin();
        let ticker = coin.ticker.clone();
        let req = self.request().clone();

        let to_tagged = coin
            .address_from_str(&req.to)
            .map_to_mm(WithdrawError::InvalidAddress)?;
        let from_tagged = self.get_from_address(&req).await?;

        self.on_generating_transaction()?;

        // ── TRON withdraw: early-return branch ──
        // TRON uses TAPOS + protobuf + bandwidth/energy fees instead of nonce + RLP + gas.
        if let ChainSpec::Tron { .. } = coin.chain_spec {
            return self.build_tron_withdraw(from_tagged, to_tagged).await;
        }

        // ── EVM withdraw: existing path (unchanged) ──
        let my_balance = coin.address_balance(from_tagged).compat().await.map_mm_err()?;
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
            EthCoinType::Eth => (wei_amount, vec![], to_tagged.inner(), ticker.as_str()),
            EthCoinType::Erc20 { platform, token_addr } => {
                let function = ERC20_CONTRACT.function("transfer")?;
                let data = function.encode_input(&[Token::Address(to_tagged.inner()), Token::Uint(wei_amount)])?;
                (0.into(), data, *token_addr, platform.as_str())
            },
            EthCoinType::Nft { .. } => {
                return MmError::err(WithdrawError::ProtocolNotSupported(format!(
                    "{} protocol is not supported",
                    coin.coin_type
                )))
            },
        };
        let eth_value_dec = u256_to_big_decimal(eth_value, coin.decimals).map_mm_err()?;

        let (gas, pay_for_gas_option) = get_eth_gas_details_from_withdraw_fee(
            coin,
            req.fee.clone(),
            eth_value,
            data.clone().into(),
            from_tagged.inner(),
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
                let address_lock = coin.get_address_lock(from_tagged.inner()).await;
                let _nonce_lock = address_lock.lock().await;
                let (nonce, _) = coin
                    .clone()
                    .get_addr_nonce(from_tagged.inner())
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
                    from: from_tagged.inner(),
                    to: Some(to_tagged.inner()),
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
                    .get_addr_nonce(from_tagged.inner())
                    .compat()
                    .timeout(ETH_RPC_REQUEST_TIMEOUT_S)
                    .await?
                    .map_to_mm(WithdrawError::Transport)?;
                let params = WcEthTxParams {
                    gas,
                    nonce,
                    data: &data,
                    my_address: from_tagged.inner(),
                    action: Action::Call(call_addr),
                    value: eth_value,
                    gas_price,
                    chain_id,
                    max_fee_per_gas,
                    max_priority_fee_per_gas,
                };

                let (tx, bytes) = if req.broadcast {
                    coin.wc_send_tx(&wc, params)
                        .await
                        .mm_err(|err| WithdrawError::SigningError(err.to_string()))?
                } else {
                    coin.wc_sign_tx(&wc, params)
                        .await
                        .mm_err(|err| WithdrawError::SigningError(err.to_string()))?
                };

                (tx.tx_hash(), bytes)
            },
        };

        self.on_finishing()?;
        let fee_details = EthTxFeeDetails::new(gas, pay_for_gas_option, fee_coin).map_mm_err()?;
        let total_fee = fee_details.total_fee.clone();
        let tx = TransactionData::new_signed(tx_hex, format_tx_hash(tx_hash));
        self.build_transaction_details(&from_tagged, &to_tagged, tx, wei_amount, &total_fee, fee_details.into())
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
        // Todo: Add support for Tron signing with Trezor
        let chain_id = coin
            .chain_spec
            .chain_id()
            .ok_or_else(|| WithdrawError::InternalError("Tron is not supported for withdraw yet".to_owned()))?;
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
    type Address = ChainTaggedAddress;
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
) -> MmResult<WithdrawSenderAddress<ChainTaggedAddress, Public>, WithdrawError> {
    match coin.derivation_method() {
        EthDerivationMethod::SingleAddress(my_address) => eth_get_withdraw_iguana_sender(coin, req, my_address),
        EthDerivationMethod::HDWallet(hd_wallet) => {
            let from = req.from.clone().or_mm_err(|| WithdrawError::FromAddressNotFound)?;
            coin.get_withdraw_hd_sender(hd_wallet, &from).await.map_mm_err()
        },
    }
}

#[allow(clippy::result_large_err)]
fn eth_get_withdraw_iguana_sender(
    coin: &EthCoin,
    req: &WithdrawRequest,
    my_address: &ChainTaggedAddress,
) -> MmResult<WithdrawSenderAddress<ChainTaggedAddress, Public>, WithdrawError> {
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
