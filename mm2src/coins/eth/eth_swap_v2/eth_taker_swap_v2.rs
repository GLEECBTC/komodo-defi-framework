use super::{
    check_decoded_length, extract_id_from_tx_data, validate_amount, validate_from_to_addresses, EthPaymentType,
    PaymentMethod, PrepareTxDataError, SpendTxSearchParams, ZERO_VALUE,
};
use crate::eth::{
    decode_contract_call, get_function_input_data, signed_tx_from_web3_tx, u256_from_big_decimal, EthCoin, EthCoinType,
    ParseCoinAssocTypes, RefundFundingSecretArgs, RefundTakerPaymentArgs, SendTakerFundingArgs, SignedEthTx,
    SwapTxTypeWithSecretHash, TakerPaymentStateV2, TransactionErr, ValidateSwapV2TxError, ValidateSwapV2TxResult,
    ValidateTakerFundingArgs, TAKER_SWAP_V2,
};
use crate::{
    FindPaymentSpendError, FundingTxSpend, GenTakerFundingSpendArgs, GenTakerPaymentSpendArgs, SearchForFundingSpendErr,
};
use derive_more::Display;
use enum_derives::EnumFromStringify;
use ethabi::{Contract, Function, Token};
use ethcore_transaction::Action;
use ethereum_types::{Address, Public, U256};
use ethkey::public_to_address;
use futures::compat::Future01CompatExt;
use mm2_err_handle::prelude::{MapToMmResult, MmError, MmResult, MmResultExt};
use std::convert::TryInto;
use web3::types::{BlockNumber, TransactionId};

const ETH_TAKER_PAYMENT: &str = "ethTakerPayment";
const ERC20_TAKER_PAYMENT: &str = "erc20TakerPayment";
const TAKER_PAYMENT_APPROVE: &str = "takerPaymentApprove";

/// state index for `TakerPayment` structure from `EtomicSwapTakerV2.sol`
///
///     struct TakerPayment {
///         bytes20 paymentHash;
///         uint32 preApproveLockTime;
///         uint32 paymentLockTime;
///         TakerPaymentState state;
///     }
const TAKER_PAYMENT_STATE_INDEX: usize = 3;

struct TakerFundingArgs<'a> {
    dex_fee: U256,
    payment_amount: U256,
    maker_address: Address,
    taker_secret_hash: &'a [u8; 32],
    maker_secret_hash: &'a [u8; 32],
    funding_time_lock: u64,
    payment_time_lock: u64,
}

struct TakerRefundTimelockArgs<'a> {
    dex_fee: U256,
    payment_amount: U256,
    maker_address: Address,
    taker_secret_hash: &'a [u8; 32],
    maker_secret_hash: &'a [u8; 32],
    payment_time_lock: u64,
    token_address: Address,
}

struct TakerRefundSecretArgs<'a> {
    dex_fee: U256,
    payment_amount: U256,
    maker_address: Address,
    taker_secret: &'a [u8; 32],
    maker_secret_hash: &'a [u8; 32],
    payment_time_lock: u64,
    token_address: Address,
}

struct TakerValidationArgs<'a> {
    swap_id: Vec<u8>,
    amount: U256,
    dex_fee: U256,
    receiver: Address,
    taker_secret_hash: &'a [u8; 32],
    maker_secret_hash: &'a [u8; 32],
    funding_time_lock: u64,
    payment_time_lock: u64,
}

impl EthCoin {
    /// Calls `"ethTakerPayment"` or `"erc20TakerPayment"` swap contract methods.
    /// Returns taker sent payment transaction.
    pub(crate) async fn send_taker_funding_impl(
        &self,
        args: SendTakerFundingArgs<'_>,
    ) -> Result<SignedEthTx, TransactionErr> {
        let taker_swap_v2_contract = self
            .swap_v2_contracts
            .ok_or_else(|| TransactionErr::Plain(ERRL!("Expected swap_v2_contracts to be Some, but found None")))?
            .taker_swap_v2_contract;
        // TODO add burnFee support
        let dex_fee = try_tx_s!(u256_from_big_decimal(&args.dex_fee.fee_amount().into(), self.decimals));

        let payment_amount = try_tx_s!(u256_from_big_decimal(
            &(args.trading_amount.clone() + args.premium_amount.clone()),
            self.decimals
        ));
        let funding_args = {
            let maker_address = public_to_address(&Public::from_slice(args.maker_pub));
            TakerFundingArgs {
                dex_fee,
                payment_amount,
                maker_address,
                taker_secret_hash: try_tx_s!(args.taker_secret_hash.try_into()),
                maker_secret_hash: try_tx_s!(args.maker_secret_hash.try_into()),
                funding_time_lock: args.funding_time_lock,
                payment_time_lock: args.payment_time_lock,
            }
        };
        match &self.coin_type {
            EthCoinType::Eth => {
                let data = try_tx_s!(self.prepare_taker_eth_funding_data(&funding_args).await);
                let eth_total_payment = payment_amount.checked_add(dex_fee).ok_or_else(|| {
                    TransactionErr::Plain(ERRL!("Overflow occurred while calculating eth_total_payment"))
                })?;
                self.sign_and_send_transaction(
                    eth_total_payment,
                    Action::Call(taker_swap_v2_contract),
                    data,
                    Some(U256::from(self.gas_limit_v2.taker.eth_payment)),
                )
                .compat()
                .await
            },
            EthCoinType::Erc20 {
                platform: _,
                token_addr,
            } => {
                let data = try_tx_s!(self.prepare_taker_erc20_funding_data(&funding_args, *token_addr).await);
                self.handle_allowance(taker_swap_v2_contract, payment_amount, args.funding_time_lock)
                    .await?;
                self.sign_and_send_transaction(
                    U256::from(ZERO_VALUE),
                    Action::Call(taker_swap_v2_contract),
                    data,
                    Some(U256::from(self.gas_limit_v2.taker.erc20_payment)),
                )
                .compat()
                .await
            },
            EthCoinType::Nft { .. } => Err(TransactionErr::ProtocolNotSupported(ERRL!(
                "NFT protocol is not supported for ETH and ERC20 Swaps"
            ))),
        }
    }

    pub(crate) async fn validate_taker_funding_impl(
        &self,
        args: ValidateTakerFundingArgs<'_, Self>,
    ) -> ValidateSwapV2TxResult {
        if let EthCoinType::Nft { .. } = self.coin_type {
            return MmError::err(ValidateSwapV2TxError::ProtocolNotSupported(
                "NFT protocol is not supported for ETH and ERC20 Swaps".to_string(),
            ));
        }
        let taker_swap_v2_contract = self
            .swap_v2_contracts
            .ok_or_else(|| {
                ValidateSwapV2TxError::Internal("Expected swap_v2_contracts to be Some, but found None".to_string())
            })?
            .taker_swap_v2_contract;
        let taker_secret_hash = args.taker_secret_hash.try_into()?;
        let maker_secret_hash = args.maker_secret_hash.try_into()?;
        validate_amount(&args.trading_amount).map_err(ValidateSwapV2TxError::Internal)?;
        let swap_id = self.etomic_swap_id_v2(args.payment_time_lock, args.maker_secret_hash);

        let tx_from_rpc = self.transaction(TransactionId::Hash(args.funding_tx.tx_hash())).await?;
        let tx_from_rpc = tx_from_rpc.as_ref().ok_or_else(|| {
            ValidateSwapV2TxError::TxDoesNotExist(format!(
                "Didn't find provided tx {:?} on ETH node",
                args.funding_tx.tx_hash()
            ))
        })?;
        let taker_address = public_to_address(args.taker_pub);
        let signed_tx = signed_tx_from_web3_tx(tx_from_rpc.clone())
            .map_err(|err| ValidateSwapV2TxError::WrongPaymentTx(format!("Could not parse tx: {:?}", err)))?;
        validate_from_to_addresses(&signed_tx, taker_address, taker_swap_v2_contract).map_mm_err()?;

        let validation_args = {
            let dex_fee = u256_from_big_decimal(&args.dex_fee.fee_amount().into(), self.decimals).map_mm_err()?;
            let payment_amount =
                u256_from_big_decimal(&(args.trading_amount + args.premium_amount), self.decimals).map_mm_err()?;
            TakerValidationArgs {
                swap_id,
                amount: payment_amount,
                dex_fee,
                receiver: self.my_addr().await,
                taker_secret_hash,
                maker_secret_hash,
                funding_time_lock: args.funding_time_lock,
                payment_time_lock: args.payment_time_lock,
            }
        };
        match self.coin_type {
            EthCoinType::Eth => {
                let function = TAKER_SWAP_V2.function(ETH_TAKER_PAYMENT)?;
                let decoded = decode_contract_call(function, &tx_from_rpc.input.0)?;
                validate_eth_taker_payment_data(&decoded, &validation_args, function, tx_from_rpc.value)?;
            },
            EthCoinType::Erc20 { token_addr, .. } => {
                let function = TAKER_SWAP_V2.function(ERC20_TAKER_PAYMENT)?;
                let decoded = decode_contract_call(function, &tx_from_rpc.input.0)?;
                validate_erc20_taker_payment_data(&decoded, &validation_args, function, token_addr)?;
            },
            EthCoinType::Nft { .. } => {
                return MmError::err(ValidateSwapV2TxError::ProtocolNotSupported(
                    "NFT protocol is not supported for ETH and ERC20 Swaps".to_string(),
                ));
            },
        }
        Ok(())
    }

    /// Taker approves payment calling `takerPaymentApprove` for EVM based chains.
    /// Function accepts taker payment transaction, returns taker approve payment transaction.
    pub(crate) async fn taker_payment_approve(
        &self,
        args: &GenTakerFundingSpendArgs<'_, Self>,
    ) -> Result<SignedEthTx, TransactionErr> {
        let gas_limit = match self.coin_type {
            EthCoinType::Eth | EthCoinType::Erc20 { .. } => U256::from(self.gas_limit_v2.taker.approve_payment),
            EthCoinType::Nft { .. } => {
                return Err(TransactionErr::ProtocolNotSupported(ERRL!(
                    "NFT protocol is not supported for ETH and ERC20 Swaps"
                )))
            },
        };
        let (taker_swap_v2_contract, send_func, token_address) = self
            .taker_swap_v2_details(ETH_TAKER_PAYMENT, ERC20_TAKER_PAYMENT)
            .await?;
        let decoded = try_tx_s!(decode_contract_call(send_func, args.funding_tx.unsigned().data()));
        let data = try_tx_s!(
            self.prepare_taker_payment_approve_data(args, decoded, token_address)
                .await
        );
        let approve_tx = self
            .sign_and_send_transaction(
                U256::from(ZERO_VALUE),
                Action::Call(taker_swap_v2_contract),
                data,
                Some(gas_limit),
            )
            .compat()
            .await?;
        Ok(approve_tx)
    }

    pub(crate) async fn refund_taker_payment_with_timelock_impl(
        &self,
        args: RefundTakerPaymentArgs<'_>,
    ) -> Result<SignedEthTx, TransactionErr> {
        let token_address = self
            .get_token_address()
            .map_err(|e| TransactionErr::Plain(ERRL!("{}", e)))?;
        let taker_swap_v2_contract = self
            .swap_v2_contracts
            .ok_or_else(|| TransactionErr::Plain(ERRL!("Expected swap_v2_contracts to be Some, but found None")))?
            .taker_swap_v2_contract;
        let gas_limit = self
            .gas_limit_v2
            .gas_limit(
                &self.coin_type,
                EthPaymentType::TakerPayments,
                PaymentMethod::RefundTimelock,
            )
            .map_err(|e| TransactionErr::Plain(ERRL!("{}", e)))?;

        let (maker_secret_hash, taker_secret_hash) = match args.tx_type_with_secret_hash {
            SwapTxTypeWithSecretHash::TakerPaymentV2 {
                maker_secret_hash,
                taker_secret_hash,
            } => (maker_secret_hash, taker_secret_hash),
            _ => {
                return Err(TransactionErr::Plain(ERRL!(
                    "Unsupported swap tx type for timelock refund"
                )))
            },
        };
        let dex_fee = try_tx_s!(u256_from_big_decimal(
            &args.dex_fee.fee_amount().to_decimal(),
            self.decimals
        ));
        let payment_amount = try_tx_s!(u256_from_big_decimal(
            &(args.trading_amount + args.premium_amount),
            self.decimals
        ));

        let args = {
            let maker_address = public_to_address(&Public::from_slice(args.maker_pub));
            TakerRefundTimelockArgs {
                dex_fee,
                payment_amount,
                maker_address,
                taker_secret_hash: try_tx_s!(taker_secret_hash.try_into()),
                maker_secret_hash: try_tx_s!(maker_secret_hash.try_into()),
                payment_time_lock: args.time_lock,
                token_address,
            }
        };
        let data = try_tx_s!(self.prepare_taker_refund_payment_timelock_data(args).await);

        self.sign_and_send_transaction(
            U256::from(ZERO_VALUE),
            Action::Call(taker_swap_v2_contract),
            data,
            Some(U256::from(gas_limit)),
        )
        .compat()
        .await
    }

    pub(crate) async fn refund_taker_funding_secret_impl(
        &self,
        args: RefundFundingSecretArgs<'_, Self>,
    ) -> Result<SignedEthTx, TransactionErr> {
        let token_address = self
            .get_token_address()
            .map_err(|e| TransactionErr::Plain(ERRL!("{}", e)))?;
        let taker_swap_v2_contract = self
            .swap_v2_contracts
            .ok_or_else(|| TransactionErr::Plain(ERRL!("Expected swap_v2_contracts to be Some, but found None")))?
            .taker_swap_v2_contract;
        let gas_limit = self
            .gas_limit_v2
            .gas_limit(
                &self.coin_type,
                EthPaymentType::TakerPayments,
                PaymentMethod::RefundSecret,
            )
            .map_err(|e| TransactionErr::Plain(ERRL!("{}", e)))?;

        let maker_secret_hash = try_tx_s!(args.maker_secret_hash.try_into());
        let dex_fee = try_tx_s!(u256_from_big_decimal(
            &args.dex_fee.fee_amount().to_decimal(),
            self.decimals
        ));
        let payment_amount = try_tx_s!(u256_from_big_decimal(
            &(args.trading_amount + args.premium_amount),
            self.decimals
        ));

        let refund_args = {
            let maker_address = public_to_address(args.maker_pubkey);
            TakerRefundSecretArgs {
                dex_fee,
                payment_amount,
                maker_address,
                taker_secret: args.taker_secret,
                maker_secret_hash,
                payment_time_lock: args.payment_time_lock,
                token_address,
            }
        };
        let data = try_tx_s!(self.prepare_taker_refund_payment_secret_data(&refund_args).await);

        self.sign_and_send_transaction(
            U256::from(ZERO_VALUE),
            Action::Call(taker_swap_v2_contract),
            data,
            Some(U256::from(gas_limit)),
        )
        .compat()
        .await
    }

    /// Checks that taker payment state is `TakerApproved`. Called by maker.
    /// Accepts a taker payment transaction and returns it if the state is correct.
    pub(crate) async fn search_for_taker_funding_spend_impl(
        &self,
        tx: &SignedEthTx,
    ) -> Result<Option<FundingTxSpend<Self>>, SearchForFundingSpendErr> {
        let (decoded, taker_swap_v2_contract) = self
            .get_funding_decoded_and_swap_contract(tx)
            .await
            .map_err(|e| SearchForFundingSpendErr::Internal(ERRL!("{}", e)))?;
        let taker_status = self
            .payment_status_v2(
                taker_swap_v2_contract,
                decoded[0].clone(), // id from ethTakerPayment or erc20TakerPayment
                &TAKER_SWAP_V2,
                EthPaymentType::TakerPayments,
                TAKER_PAYMENT_STATE_INDEX,
                // Use the latest confirmed block to ensure smart contract has the correct taker payment state (`TakerPaymentStateV2::TakerApproved`)
                // before the maker sends the spend transaction, which reveals the maker's secret.
                // TPU state machine waits confirmations only for send payment tx, not approve tx.
                BlockNumber::Latest,
            )
            .await
            .map_err(|e| SearchForFundingSpendErr::Internal(ERRL!("{}", e)))?;
        if taker_status == U256::from(TakerPaymentStateV2::TakerApproved as u8) {
            return Ok(Some(FundingTxSpend::TransferredToTakerPayment(tx.clone())));
        }
        Ok(None)
    }

    /// Returns maker spent taker payment transaction. Called by maker.
    /// Taker swap contract's `spendTakerPayment` method is called for EVM-based chains.
    pub(crate) async fn sign_and_broadcast_taker_payment_spend_impl(
        &self,
        gen_args: &GenTakerPaymentSpendArgs<'_, Self>,
        secret: &[u8],
    ) -> Result<SignedEthTx, TransactionErr> {
        let gas_limit = self
            .gas_limit_v2
            .gas_limit(&self.coin_type, EthPaymentType::TakerPayments, PaymentMethod::Spend)
            .map_err(|e| TransactionErr::Plain(ERRL!("{}", e)))?;

        let (taker_swap_v2_contract, taker_payment, token_address) = self
            .taker_swap_v2_details(ETH_TAKER_PAYMENT, ERC20_TAKER_PAYMENT)
            .await?;
        let decoded = try_tx_s!(decode_contract_call(taker_payment, gen_args.taker_tx.unsigned().data()));
        let data = try_tx_s!(
            self.prepare_spend_taker_payment_data(gen_args, secret, decoded, token_address)
                .await
        );
        let spend_payment_tx = self
            .sign_and_send_transaction(
                U256::from(ZERO_VALUE),
                Action::Call(taker_swap_v2_contract),
                data,
                Some(U256::from(gas_limit)),
            )
            .compat()
            .await?;
        Ok(spend_payment_tx)
    }

    pub(crate) async fn find_taker_payment_spend_tx_impl(
        &self,
        taker_payment: &SignedEthTx, // it's approve_tx in Eth case, as in sign_and_send_taker_funding_spend we return approve_tx tx for it
        from_block: u64,
        wait_until: u64,
        check_every: f64,
    ) -> MmResult<SignedEthTx, FindPaymentSpendError> {
        let taker_swap_v2_contract = self
            .swap_v2_contracts
            .ok_or_else(|| {
                FindPaymentSpendError::Internal("Expected swap_v2_contracts to be Some, but found None".to_string())
            })?
            .taker_swap_v2_contract;
        let id_array = extract_id_from_tx_data(taker_payment.unsigned().data(), &TAKER_SWAP_V2, TAKER_PAYMENT_APPROVE)
            .await?
            .as_slice()
            .try_into()?;

        let params = SpendTxSearchParams {
            swap_contract_address: taker_swap_v2_contract,
            event_name: "TakerPaymentSpent",
            abi_contract: &TAKER_SWAP_V2,
            swap_id: &id_array,
            from_block,
            wait_until,
            check_every,
        };
        let tx_hash = self.find_transaction_hash_by_event(params).await?;

        let spend_tx = self.wait_for_transaction(tx_hash, wait_until, check_every).await?;
        Ok(spend_tx)
    }

    /// Prepares data for EtomicSwapTakerV2 contract [ethTakerPayment](https://github.com/KomodoPlatform/etomic-swap/blob/5e15641cbf41766cd5b37b4d71842c270773f788/contracts/EtomicSwapTakerV2.sol#L44) method
    async fn prepare_taker_eth_funding_data(&self, args: &TakerFundingArgs<'_>) -> Result<Vec<u8>, PrepareTxDataError> {
        let function = TAKER_SWAP_V2.function(ETH_TAKER_PAYMENT)?;
        let id = self.etomic_swap_id_v2(args.payment_time_lock, args.maker_secret_hash);
        let data = function.encode_input(&[
            Token::FixedBytes(id),
            Token::Uint(args.dex_fee),
            Token::Address(args.maker_address),
            Token::FixedBytes(args.taker_secret_hash.to_vec()),
            Token::FixedBytes(args.maker_secret_hash.to_vec()),
            Token::Uint(args.funding_time_lock.into()),
            Token::Uint(args.payment_time_lock.into()),
        ])?;
        Ok(data)
    }

    /// Prepares data for EtomicSwapTakerV2 contract [erc20TakerPayment](https://github.com/KomodoPlatform/etomic-swap/blob/5e15641cbf41766cd5b37b4d71842c270773f788/contracts/EtomicSwapTakerV2.sol#L83) method
    async fn prepare_taker_erc20_funding_data(
        &self,
        args: &TakerFundingArgs<'_>,
        token_address: Address,
    ) -> Result<Vec<u8>, PrepareTxDataError> {
        let function = TAKER_SWAP_V2.function(ERC20_TAKER_PAYMENT)?;
        let id = self.etomic_swap_id_v2(args.payment_time_lock, args.maker_secret_hash);
        let data = function.encode_input(&[
            Token::FixedBytes(id),
            Token::Uint(args.payment_amount),
            Token::Uint(args.dex_fee),
            Token::Address(token_address),
            Token::Address(args.maker_address),
            Token::FixedBytes(args.taker_secret_hash.to_vec()),
            Token::FixedBytes(args.maker_secret_hash.to_vec()),
            Token::Uint(args.funding_time_lock.into()),
            Token::Uint(args.payment_time_lock.into()),
        ])?;
        Ok(data)
    }

    /// Prepares data for EtomicSwapTakerV2 contract [refundTakerPaymentTimelock](https://github.com/KomodoPlatform/etomic-swap/blob/5e15641cbf41766cd5b37b4d71842c270773f788/contracts/EtomicSwapTakerV2.sol#L208) method
    async fn prepare_taker_refund_payment_timelock_data(
        &self,
        args: TakerRefundTimelockArgs<'_>,
    ) -> Result<Vec<u8>, PrepareTxDataError> {
        let function = TAKER_SWAP_V2.function("refundTakerPaymentTimelock")?;
        let id = self.etomic_swap_id_v2(args.payment_time_lock, args.maker_secret_hash);
        let data = function.encode_input(&[
            Token::FixedBytes(id),
            Token::Uint(args.payment_amount),
            Token::Uint(args.dex_fee),
            Token::Address(args.maker_address),
            Token::FixedBytes(args.taker_secret_hash.to_vec()),
            Token::FixedBytes(args.maker_secret_hash.to_vec()),
            Token::Address(args.token_address),
        ])?;
        Ok(data)
    }

    /// Prepares data for EtomicSwapTakerV2 contract [refundTakerPaymentSecret](https://github.com/KomodoPlatform/etomic-swap/blob/5e15641cbf41766cd5b37b4d71842c270773f788/contracts/EtomicSwapTakerV2.sol#L267) method
    async fn prepare_taker_refund_payment_secret_data(
        &self,
        args: &TakerRefundSecretArgs<'_>,
    ) -> Result<Vec<u8>, PrepareTxDataError> {
        let function = TAKER_SWAP_V2.function("refundTakerPaymentSecret")?;
        let id = self.etomic_swap_id_v2(args.payment_time_lock, args.maker_secret_hash);
        let data = function.encode_input(&[
            Token::FixedBytes(id),
            Token::Uint(args.payment_amount),
            Token::Uint(args.dex_fee),
            Token::Address(args.maker_address),
            Token::FixedBytes(args.taker_secret.to_vec()),
            Token::FixedBytes(args.maker_secret_hash.to_vec()),
            Token::Address(args.token_address),
        ])?;
        Ok(data)
    }

    /// This function constructs the encoded transaction input data required to approve the taker payment ([takerPaymentApprove](https://github.com/KomodoPlatform/etomic-swap/blob/5e15641cbf41766cd5b37b4d71842c270773f788/contracts/EtomicSwapTakerV2.sol#L128)).
    /// The `decoded` parameter should contain the transaction input data from the `ethTakerPayment` or `erc20TakerPayment` function of the EtomicSwapTakerV2 contract.
    async fn prepare_taker_payment_approve_data(
        &self,
        args: &GenTakerFundingSpendArgs<'_, Self>,
        decoded: Vec<Token>,
        token_address: Address,
    ) -> Result<Vec<u8>, PrepareTxDataError> {
        let function = TAKER_SWAP_V2.function(TAKER_PAYMENT_APPROVE)?;
        let data = match self.coin_type {
            EthCoinType::Eth => {
                let (dex_fee, amount) =
                    get_dex_fee_and_amount_from_eth_payment_data(&decoded, args.funding_tx.unsigned().value())?;
                function.encode_input(&[
                    decoded[0].clone(),   // id from ethTakerPayment
                    Token::Uint(amount),  // calculated payment amount (tx value - dexFee)
                    Token::Uint(dex_fee), // dexFee from ethTakerPayment
                    decoded[2].clone(),   // receiver from ethTakerPayment
                    Token::FixedBytes(args.taker_secret_hash.to_vec()),
                    Token::FixedBytes(args.maker_secret_hash.to_vec()),
                    Token::Address(token_address), // should be zero address Address::default()
                ])?
            },
            EthCoinType::Erc20 { .. } => {
                check_decoded_length(&decoded, 9)?;
                function.encode_input(&[
                    decoded[0].clone(), // id from erc20TakerPayment
                    decoded[1].clone(), // amount from erc20TakerPayment
                    decoded[2].clone(), // dexFee from erc20TakerPayment
                    decoded[4].clone(), // receiver from erc20TakerPayment
                    Token::FixedBytes(args.taker_secret_hash.to_vec()),
                    Token::FixedBytes(args.maker_secret_hash.to_vec()),
                    Token::Address(token_address), // erc20 token address from EthCoinType::Erc20
                ])?
            },
            EthCoinType::Nft { .. } => {
                return Err(PrepareTxDataError::Internal(
                    "NFT protocol is not supported for ETH and ERC20 Swaps".into(),
                ))
            },
        };
        Ok(data)
    }

    /// Prepares data for EtomicSwapTakerV2 contract [spendTakerPayment](https://github.com/KomodoPlatform/etomic-swap/blob/5e15641cbf41766cd5b37b4d71842c270773f788/contracts/EtomicSwapTakerV2.sol#L164) method
    async fn prepare_spend_taker_payment_data(
        &self,
        args: &GenTakerPaymentSpendArgs<'_, Self>,
        secret: &[u8],
        decoded: Vec<Token>,
        token_address: Address,
    ) -> Result<Vec<u8>, PrepareTxDataError> {
        let function = TAKER_SWAP_V2.function("spendTakerPayment")?;
        let taker_address = public_to_address(args.taker_pub);
        match self.coin_type {
            EthCoinType::Eth => {
                let (dex_fee, amount) =
                    get_dex_fee_and_amount_from_eth_payment_data(&decoded, args.taker_tx.unsigned().value())?;
                let data = function.encode_input(&[
                    decoded[0].clone(),                 // id from ethTakerPayment
                    Token::Uint(amount),                // calculated payment amount (tx value - dexFee)
                    Token::Uint(dex_fee),               // dexFee from ethTakerPayment
                    Token::Address(taker_address),      // taker address
                    decoded[3].clone(),                 // takerSecretHash from ethTakerPayment
                    Token::FixedBytes(secret.to_vec()), // makerSecret
                    Token::Address(token_address),      // tokenAddress
                ])?;
                Ok(data)
            },
            EthCoinType::Erc20 { .. } => {
                check_decoded_length(&decoded, 9)?;
                let data = function.encode_input(&[
                    decoded[0].clone(),                 // id from erc20TakerPayment
                    decoded[1].clone(),                 // amount from erc20TakerPayment
                    decoded[2].clone(),                 // dexFee from erc20TakerPayment
                    Token::Address(taker_address),      // taker address
                    decoded[5].clone(),                 // takerSecretHash from erc20TakerPayment
                    Token::FixedBytes(secret.to_vec()), // makerSecret
                    Token::Address(token_address),      // tokenAddress
                ])?;
                Ok(data)
            },
            EthCoinType::Nft { .. } => Err(PrepareTxDataError::Internal(
                "NFT protocol is not supported for ETH and ERC20 Swaps".to_string(),
            )),
        }
    }

    /// Retrieves the taker smart contract address, the corresponding function, and the token address.
    ///
    /// Depending on the coin type (ETH or ERC20), it fetches the appropriate function name  and token address.
    /// Returns an error if the coin type is NFT or if the `swap_v2_contracts` is None.
    async fn taker_swap_v2_details(
        &self,
        eth_func_name: &str,
        erc20_func_name: &str,
    ) -> Result<(Address, &Function, Address), TransactionErr> {
        let (func, token_address) = match self.coin_type {
            EthCoinType::Eth => (try_tx_s!(TAKER_SWAP_V2.function(eth_func_name)), Address::default()),
            EthCoinType::Erc20 { token_addr, .. } => (try_tx_s!(TAKER_SWAP_V2.function(erc20_func_name)), token_addr),
            EthCoinType::Nft { .. } => {
                return Err(TransactionErr::ProtocolNotSupported(ERRL!(
                    "NFT protocol is not supported for ETH and ERC20 Swaps"
                )))
            },
        };
        let taker_swap_v2_contract = self
            .swap_v2_contracts
            .as_ref()
            .map(|contracts| contracts.taker_swap_v2_contract)
            .ok_or_else(|| TransactionErr::Plain(ERRL!("Expected swap_v2_contracts to be Some, but found None")))?;
        Ok((taker_swap_v2_contract, func, token_address))
    }

    async fn get_funding_decoded_and_swap_contract(
        &self,
        tx: &SignedEthTx,
    ) -> Result<(Vec<Token>, Address), PrepareTxDataError> {
        let decoded = {
            let func = match self.coin_type {
                EthCoinType::Eth => TAKER_SWAP_V2.function(ETH_TAKER_PAYMENT)?,
                EthCoinType::Erc20 { .. } => TAKER_SWAP_V2.function(ERC20_TAKER_PAYMENT)?,
                EthCoinType::Nft { .. } => {
                    return Err(PrepareTxDataError::Internal(
                        "NFT protocol is not supported for ETH and ERC20 Swaps".to_string(),
                    ));
                },
            };
            decode_contract_call(func, tx.unsigned().data())?
        };
        let taker_swap_v2_contract = self
            .swap_v2_contracts
            .as_ref()
            .map(|contracts| contracts.taker_swap_v2_contract)
            .ok_or_else(|| {
                PrepareTxDataError::Internal("Expected swap_v2_contracts to be Some, but found None".to_string())
            })?;

        Ok((decoded, taker_swap_v2_contract))
    }

    /// Extracts the maker's secret from the input of transaction that calls the `spendTakerPayment` smart contract method.
    ///
    ///     function spendTakerPayment(
    ///         bytes32 id,
    ///         uint256 amount,
    ///         uint256 dexFee,
    ///         address taker,
    ///         bytes32 takerSecretHash,
    ///         bytes32 makerSecret,
    ///         address tokenAddress
    ///     )
    pub(crate) async fn extract_secret_v2_impl(&self, spend_tx: &SignedEthTx) -> Result<[u8; 32], String> {
        let function = try_s!(TAKER_SWAP_V2.function("spendTakerPayment"));
        // should be 0xcc90c199
        let expected_signature = function.short_signature();
        let signature = &spend_tx.unsigned().data()[0..4];
        if signature != expected_signature {
            return ERR!(
                "Expected 'spendTakerPayment' contract call signature: {:?}, found {:?}",
                expected_signature,
                signature
            );
        };
        let decoded = try_s!(decode_contract_call(function, spend_tx.unsigned().data()));
        if decoded.len() < 7 {
            return ERR!("Invalid arguments in 'spendTakerPayment' call: {:?}", decoded);
        }
        match &decoded[5] {
            Token::FixedBytes(secret) => Ok(try_s!(secret.as_slice().try_into())),
            _ => ERR!(
                "Expected secret to be fixed bytes, but decoded function data is {:?}",
                decoded
            ),
        }
    }

    /// Retrieves the payment status from a given smart contract address based on the swap ID and state type.
    async fn payment_status_v2(
        &self,
        swap_address: Address,
        swap_id: Token,
        contract_abi: &Contract,
        payment_type: EthPaymentType,
        state_index: usize,
        block_number: BlockNumber,
    ) -> Result<U256, PaymentStatusErr> {
        let function = contract_abi.function(payment_type.as_str())?;
        let data = function.encode_input(&[swap_id])?;
        let bytes = self
            .call_request(
                self.my_addr().await,
                swap_address,
                None,
                Some(data.into()),
                block_number,
            )
            .await?;
        let decoded_tokens = function.decode_output(&bytes.0)?;

        let state = decoded_tokens.get(state_index).ok_or_else(|| {
            PaymentStatusErr::Internal(format!(
                "Payment status must contain 'state' as the {state_index} token"
            ))
        })?;
        match state {
            Token::Uint(state) => Ok(*state),
            _ => Err(PaymentStatusErr::InvalidData(format!(
                "Payment status must be Uint, got {state:?}"
            ))),
        }
    }
}

#[derive(Debug, Display, EnumFromStringify)]
enum PaymentStatusErr {
    #[from_stringify("ethabi::Error")]
    #[display(fmt = "ABI error: {_0}")]
    ABIError(String),
    #[from_stringify("web3::Error")]
    #[display(fmt = "Transport error: {_0}")]
    Transport(String),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
    #[display(fmt = "Invalid data error: {_0}")]
    InvalidData(String),
}

/// Validation function for ETH taker payment data
fn validate_eth_taker_payment_data(
    decoded: &[Token],
    args: &TakerValidationArgs,
    func: &Function,
    tx_value: U256,
) -> Result<(), MmError<ValidateSwapV2TxError>> {
    let checks = vec![
        (0, Token::FixedBytes(args.swap_id.clone()), "id"),
        (1, Token::Uint(args.dex_fee), "dexFee"),
        (2, Token::Address(args.receiver), "receiver"),
        (3, Token::FixedBytes(args.taker_secret_hash.to_vec()), "takerSecretHash"),
        (4, Token::FixedBytes(args.maker_secret_hash.to_vec()), "makerSecretHash"),
        (5, Token::Uint(U256::from(args.funding_time_lock)), "preApproveLockTime"),
        (6, Token::Uint(U256::from(args.payment_time_lock)), "paymentLockTime"),
    ];

    for (index, expected_token, field_name) in checks {
        let token = get_function_input_data(decoded, func, index).map_to_mm(ValidateSwapV2TxError::Internal)?;
        if token != expected_token {
            return MmError::err(ValidateSwapV2TxError::WrongPaymentTx(format!(
                "ETH Taker Payment `{}` {:?} is invalid, expected {:?}",
                field_name,
                decoded.get(index),
                expected_token
            )));
        }
    }
    let total = args.amount.checked_add(args.dex_fee).ok_or_else(|| {
        ValidateSwapV2TxError::Overflow("Overflow occurred while calculating total payment".to_string())
    })?;
    if total != tx_value {
        return MmError::err(ValidateSwapV2TxError::WrongPaymentTx(format!(
            "ETH Taker Payment amount, is invalid, expected {total:?}, got {tx_value:?}"
        )));
    }
    Ok(())
}

/// Validation function for ERC20 taker payment data
fn validate_erc20_taker_payment_data(
    decoded: &[Token],
    args: &TakerValidationArgs,
    func: &Function,
    token_addr: Address,
) -> Result<(), MmError<ValidateSwapV2TxError>> {
    let checks = vec![
        (0, Token::FixedBytes(args.swap_id.clone()), "id"),
        (1, Token::Uint(args.amount), "amount"),
        (2, Token::Uint(args.dex_fee), "dexFee"),
        (3, Token::Address(token_addr), "tokenAddress"),
        (4, Token::Address(args.receiver), "receiver"),
        (5, Token::FixedBytes(args.taker_secret_hash.to_vec()), "takerSecretHash"),
        (6, Token::FixedBytes(args.maker_secret_hash.to_vec()), "makerSecretHash"),
        (7, Token::Uint(U256::from(args.funding_time_lock)), "preApproveLockTime"),
        (8, Token::Uint(U256::from(args.payment_time_lock)), "paymentLockTime"),
    ];

    for (index, expected_token, field_name) in checks {
        let token = get_function_input_data(decoded, func, index).map_to_mm(ValidateSwapV2TxError::Internal)?;
        if token != expected_token {
            return MmError::err(ValidateSwapV2TxError::WrongPaymentTx(format!(
                "ERC20 Taker Payment `{}` {:?} is invalid, expected {:?}",
                field_name,
                decoded.get(index),
                expected_token
            )));
        }
    }
    Ok(())
}

fn get_dex_fee_and_amount_from_eth_payment_data(
    decoded: &Vec<Token>,
    tx_value: U256,
) -> Result<(U256, U256), PrepareTxDataError> {
    check_decoded_length(decoded, 7)?;
    let dex_fee = match decoded.get(1) {
        Some(Token::Uint(dex_fee)) => *dex_fee,
        _ => {
            return Err(PrepareTxDataError::Internal(format!(
                "Invalid token type for dex fee, got decoded function data: {decoded:?}"
            )))
        },
    };
    let amount = tx_value
        .checked_sub(dex_fee)
        .ok_or_else(|| PrepareTxDataError::Internal("Underflow occurred while calculating amount".into()))?;
    Ok((dex_fee, amount))
}
