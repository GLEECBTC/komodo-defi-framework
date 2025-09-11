use super::{
    validate_amount, validate_from_to_addresses, EthPaymentType, PaymentMethod, PrepareTxDataError, ZERO_VALUE,
};
use crate::coin_errors::{ValidatePaymentError, ValidatePaymentResult};
use crate::eth::{
    decode_contract_call, get_function_input_data, u256_from_big_decimal, EthCoin, EthCoinType, SignedEthTx,
    MAKER_SWAP_V2,
};
use crate::{
    ParseCoinAssocTypes, RefundMakerPaymentSecretArgs, RefundMakerPaymentTimelockArgs, SendMakerPaymentArgs,
    SpendMakerPaymentArgs, SwapTxTypeWithSecretHash, TransactionErr, ValidateMakerPaymentArgs,
};
use ethabi::{Function, Token};
use ethcore_transaction::Action;
use ethereum_types::{Address, Public, U256};
use ethkey::public_to_address;
use futures::compat::Future01CompatExt;
use mm2_err_handle::mm_error::MmError;
use mm2_err_handle::prelude::{MapToMmResult, MmResultExt};
use std::convert::TryInto;
use web3::types::TransactionId;

const ETH_MAKER_PAYMENT: &str = "ethMakerPayment";
const ERC20_MAKER_PAYMENT: &str = "erc20MakerPayment";

struct MakerPaymentArgs<'a> {
    taker_address: Address,
    taker_secret_hash: &'a [u8; 32],
    maker_secret_hash: &'a [u8; 32],
    payment_time_lock: u64,
}

struct MakerValidationArgs<'a> {
    swap_id: Vec<u8>,
    amount: U256,
    taker: Address,
    taker_secret_hash: &'a [u8; 32],
    maker_secret_hash: &'a [u8; 32],
    payment_time_lock: u64,
}

struct MakerRefundTimelockArgs<'a> {
    payment_amount: U256,
    taker_address: Address,
    taker_secret_hash: &'a [u8; 32],
    maker_secret_hash: &'a [u8; 32],
    payment_time_lock: u64,
    token_address: Address,
}

struct MakerRefundSecretArgs<'a> {
    payment_amount: U256,
    taker_address: Address,
    taker_secret: &'a [u8; 32],
    maker_secret_hash: &'a [u8; 32],
    payment_time_lock: u64,
    token_address: Address,
}

impl EthCoin {
    pub(crate) async fn send_maker_payment_v2_impl(
        &self,
        args: SendMakerPaymentArgs<'_, Self>,
    ) -> Result<SignedEthTx, TransactionErr> {
        let maker_swap_v2_contract = self
            .swap_v2_contracts
            .ok_or_else(|| TransactionErr::Plain(ERRL!("Expected swap_v2_contracts to be Some, but found None")))?
            .maker_swap_v2_contract;
        let payment_amount = try_tx_s!(u256_from_big_decimal(&args.amount, self.decimals));
        let payment_args = {
            let taker_address = public_to_address(args.taker_pub);
            MakerPaymentArgs {
                taker_address,
                taker_secret_hash: try_tx_s!(args.taker_secret_hash.try_into()),
                maker_secret_hash: try_tx_s!(args.maker_secret_hash.try_into()),
                payment_time_lock: args.time_lock,
            }
        };
        match &self.coin_type {
            EthCoinType::Eth => {
                let data = try_tx_s!(self.prepare_maker_eth_payment_data(&payment_args).await);
                self.sign_and_send_transaction(
                    payment_amount,
                    Action::Call(maker_swap_v2_contract),
                    data,
                    Some(U256::from(self.gas_limit_v2.maker.eth_payment)),
                )
                .compat()
                .await
            },
            EthCoinType::Erc20 {
                platform: _,
                token_addr,
            } => {
                let data = try_tx_s!(
                    self.prepare_maker_erc20_payment_data(&payment_args, payment_amount, *token_addr)
                        .await
                );
                self.handle_allowance(maker_swap_v2_contract, payment_amount, args.time_lock)
                    .await?;
                self.sign_and_send_transaction(
                    U256::from(ZERO_VALUE),
                    Action::Call(maker_swap_v2_contract),
                    data,
                    Some(U256::from(self.gas_limit_v2.maker.erc20_payment)),
                )
                .compat()
                .await
            },
            EthCoinType::Nft { .. } => Err(TransactionErr::ProtocolNotSupported(ERRL!(
                "NFT protocol is not supported for ETH and ERC20 Swaps"
            ))),
        }
    }

    pub(crate) async fn validate_maker_payment_v2_impl(
        &self,
        args: ValidateMakerPaymentArgs<'_, Self>,
    ) -> ValidatePaymentResult<()> {
        if let EthCoinType::Nft { .. } = self.coin_type {
            return MmError::err(ValidatePaymentError::ProtocolNotSupported(
                "NFT protocol is not supported for ETH and ERC20 Swaps".to_string(),
            ));
        }
        let maker_swap_v2_contract = self
            .swap_v2_contracts
            .ok_or_else(|| {
                ValidatePaymentError::InternalError("Expected swap_v2_contracts to be Some, but found None".to_string())
            })?
            .maker_swap_v2_contract;
        let taker_secret_hash = args.taker_secret_hash.try_into()?;
        let maker_secret_hash = args.maker_secret_hash.try_into()?;
        validate_amount(&args.amount).map_to_mm(ValidatePaymentError::InternalError)?;
        let swap_id = self.etomic_swap_id_v2(args.time_lock, args.maker_secret_hash);

        let tx_from_rpc = self
            .transaction(TransactionId::Hash(args.maker_payment_tx.tx_hash()))
            .await?;
        let tx_from_rpc = tx_from_rpc.as_ref().ok_or_else(|| {
            ValidatePaymentError::TxDoesNotExist(format!(
                "Didn't find provided tx {:?} on ETH node",
                args.maker_payment_tx.tx_hash()
            ))
        })?;
        let maker_address = public_to_address(args.maker_pub);
        validate_from_to_addresses(tx_from_rpc, maker_address, maker_swap_v2_contract).map_mm_err()?;

        let validation_args = {
            let amount = u256_from_big_decimal(&args.amount, self.decimals).map_mm_err()?;
            MakerValidationArgs {
                swap_id,
                amount,
                taker: self.my_addr().await,
                taker_secret_hash,
                maker_secret_hash,
                payment_time_lock: args.time_lock,
            }
        };
        match self.coin_type {
            EthCoinType::Eth => {
                let function = MAKER_SWAP_V2.function(ETH_MAKER_PAYMENT)?;
                let decoded = decode_contract_call(function, &tx_from_rpc.input.0)?;
                validate_eth_maker_payment_data(&decoded, &validation_args, function, tx_from_rpc.value)?;
            },
            EthCoinType::Erc20 { token_addr, .. } => {
                let function = MAKER_SWAP_V2.function(ERC20_MAKER_PAYMENT)?;
                let decoded = decode_contract_call(function, &tx_from_rpc.input.0)?;
                validate_erc20_maker_payment_data(&decoded, &validation_args, function, token_addr)?;
            },
            EthCoinType::Nft { .. } => {
                return MmError::err(ValidatePaymentError::ProtocolNotSupported(
                    "NFT protocol is not supported for ETH and ERC20 Swaps".to_string(),
                ));
            },
        }
        Ok(())
    }

    pub(crate) async fn refund_maker_payment_v2_timelock_impl(
        &self,
        args: RefundMakerPaymentTimelockArgs<'_>,
    ) -> Result<SignedEthTx, TransactionErr> {
        let token_address = self
            .get_token_address()
            .map_err(|e| TransactionErr::Plain(ERRL!("{}", e)))?;
        let maker_swap_v2_contract = self
            .swap_v2_contracts
            .ok_or_else(|| TransactionErr::Plain(ERRL!("Expected swap_v2_contracts to be Some, but found None")))?
            .maker_swap_v2_contract;
        let gas_limit = self
            .gas_limit_v2
            .gas_limit(
                &self.coin_type,
                EthPaymentType::MakerPayments,
                PaymentMethod::RefundTimelock,
            )
            .map_err(|e| TransactionErr::Plain(ERRL!("{}", e)))?;

        let (maker_secret_hash, taker_secret_hash) = match args.tx_type_with_secret_hash {
            SwapTxTypeWithSecretHash::MakerPaymentV2 {
                maker_secret_hash,
                taker_secret_hash,
            } => (maker_secret_hash, taker_secret_hash),
            _ => {
                return Err(TransactionErr::Plain(ERRL!(
                    "Unsupported swap tx type for timelock refund"
                )))
            },
        };
        let payment_amount = try_tx_s!(u256_from_big_decimal(&args.amount, self.decimals));

        let args = {
            let taker_address = public_to_address(&Public::from_slice(args.taker_pub));
            MakerRefundTimelockArgs {
                payment_amount,
                taker_address,
                taker_secret_hash: try_tx_s!(taker_secret_hash.try_into()),
                maker_secret_hash: try_tx_s!(maker_secret_hash.try_into()),
                payment_time_lock: args.time_lock,
                token_address,
            }
        };
        let data = try_tx_s!(self.prepare_refund_maker_payment_timelock_data(args).await);

        self.sign_and_send_transaction(
            U256::from(ZERO_VALUE),
            Action::Call(maker_swap_v2_contract),
            data,
            Some(U256::from(gas_limit)),
        )
        .compat()
        .await
    }

    pub(crate) async fn refund_maker_payment_v2_secret_impl(
        &self,
        args: RefundMakerPaymentSecretArgs<'_, Self>,
    ) -> Result<SignedEthTx, TransactionErr> {
        let token_address = self
            .get_token_address()
            .map_err(|e| TransactionErr::Plain(ERRL!("{}", e)))?;
        let maker_swap_v2_contract = self
            .swap_v2_contracts
            .ok_or_else(|| TransactionErr::Plain(ERRL!("Expected swap_v2_contracts to be Some, but found None")))?
            .maker_swap_v2_contract;
        let gas_limit = self
            .gas_limit_v2
            .gas_limit(
                &self.coin_type,
                EthPaymentType::MakerPayments,
                PaymentMethod::RefundSecret,
            )
            .map_err(|e| TransactionErr::Plain(ERRL!("{}", e)))?;

        let maker_secret_hash = try_tx_s!(args.maker_secret_hash.try_into());
        let payment_amount = try_tx_s!(u256_from_big_decimal(&args.amount, self.decimals));
        let args = {
            let taker_address = public_to_address(args.taker_pub);
            MakerRefundSecretArgs {
                payment_amount,
                taker_address,
                taker_secret: args.taker_secret,
                maker_secret_hash,
                payment_time_lock: args.time_lock,
                token_address,
            }
        };
        let data = try_tx_s!(self.prepare_refund_maker_payment_secret_data(args).await);

        self.sign_and_send_transaction(
            U256::from(ZERO_VALUE),
            Action::Call(maker_swap_v2_contract),
            data,
            Some(U256::from(gas_limit)),
        )
        .compat()
        .await
    }

    pub(crate) async fn spend_maker_payment_v2_impl(
        &self,
        args: SpendMakerPaymentArgs<'_, Self>,
    ) -> Result<SignedEthTx, TransactionErr> {
        let token_address = self
            .get_token_address()
            .map_err(|e| TransactionErr::Plain(ERRL!("{}", e)))?;
        let maker_swap_v2_contract = self
            .swap_v2_contracts
            .ok_or_else(|| TransactionErr::Plain(ERRL!("Expected swap_v2_contracts to be Some, but found None")))?
            .maker_swap_v2_contract;
        let gas_limit = self
            .gas_limit_v2
            .gas_limit(&self.coin_type, EthPaymentType::MakerPayments, PaymentMethod::Spend)
            .map_err(|e| TransactionErr::Plain(ERRL!("{}", e)))?;

        let data = try_tx_s!(self.prepare_spend_maker_payment_data(args, token_address).await);

        self.sign_and_send_transaction(
            U256::from(ZERO_VALUE),
            Action::Call(maker_swap_v2_contract),
            data,
            Some(U256::from(gas_limit)),
        )
        .compat()
        .await
    }

    /// Prepares data for EtomicSwapMakerV2 contract [ethMakerPayment](https://github.com/KomodoPlatform/etomic-swap/blob/5e15641cbf41766cd5b37b4d71842c270773f788/contracts/EtomicSwapMakerV2.sol#L30) method
    async fn prepare_maker_eth_payment_data(&self, args: &MakerPaymentArgs<'_>) -> Result<Vec<u8>, PrepareTxDataError> {
        let function = MAKER_SWAP_V2.function(ETH_MAKER_PAYMENT)?;
        let id = self.etomic_swap_id_v2(args.payment_time_lock, args.maker_secret_hash);
        let data = function.encode_input(&[
            Token::FixedBytes(id),
            Token::Address(args.taker_address),
            Token::FixedBytes(args.taker_secret_hash.to_vec()),
            Token::FixedBytes(args.maker_secret_hash.to_vec()),
            Token::Uint(args.payment_time_lock.into()),
        ])?;
        Ok(data)
    }

    /// Prepares data for EtomicSwapMakerV2 contract [erc20MakerPayment](https://github.com/KomodoPlatform/etomic-swap/blob/5e15641cbf41766cd5b37b4d71842c270773f788/contracts/EtomicSwapMakerV2.sol#L64) method
    async fn prepare_maker_erc20_payment_data(
        &self,
        args: &MakerPaymentArgs<'_>,
        payment_amount: U256,
        token_address: Address,
    ) -> Result<Vec<u8>, PrepareTxDataError> {
        let function = MAKER_SWAP_V2.function(ERC20_MAKER_PAYMENT)?;
        let id = self.etomic_swap_id_v2(args.payment_time_lock, args.maker_secret_hash);
        let data = function.encode_input(&[
            Token::FixedBytes(id),
            Token::Uint(payment_amount),
            Token::Address(token_address),
            Token::Address(args.taker_address),
            Token::FixedBytes(args.taker_secret_hash.to_vec()),
            Token::FixedBytes(args.maker_secret_hash.to_vec()),
            Token::Uint(args.payment_time_lock.into()),
        ])?;
        Ok(data)
    }

    /// Prepares data for EtomicSwapMakerV2 contract [refundMakerPaymentTimelock](https://github.com/KomodoPlatform/etomic-swap/blob/5e15641cbf41766cd5b37b4d71842c270773f788/contracts/EtomicSwapMakerV2.sol#L144) method
    async fn prepare_refund_maker_payment_timelock_data(
        &self,
        args: MakerRefundTimelockArgs<'_>,
    ) -> Result<Vec<u8>, PrepareTxDataError> {
        let function = MAKER_SWAP_V2.function("refundMakerPaymentTimelock")?;
        let id = self.etomic_swap_id_v2(args.payment_time_lock, args.maker_secret_hash);
        let data = function.encode_input(&[
            Token::FixedBytes(id),
            Token::Uint(args.payment_amount),
            Token::Address(args.taker_address),
            Token::FixedBytes(args.taker_secret_hash.to_vec()),
            Token::FixedBytes(args.maker_secret_hash.to_vec()),
            Token::Address(args.token_address),
        ])?;
        Ok(data)
    }

    /// Prepares data for EtomicSwapMakerV2 contract [refundMakerPaymentSecret](https://github.com/KomodoPlatform/etomic-swap/blob/5e15641cbf41766cd5b37b4d71842c270773f788/contracts/EtomicSwapMakerV2.sol#L190) method
    async fn prepare_refund_maker_payment_secret_data(
        &self,
        args: MakerRefundSecretArgs<'_>,
    ) -> Result<Vec<u8>, PrepareTxDataError> {
        let function = MAKER_SWAP_V2.function("refundMakerPaymentSecret")?;
        let id = self.etomic_swap_id_v2(args.payment_time_lock, args.maker_secret_hash);
        let data = function.encode_input(&[
            Token::FixedBytes(id),
            Token::Uint(args.payment_amount),
            Token::Address(args.taker_address),
            Token::FixedBytes(args.taker_secret.to_vec()),
            Token::FixedBytes(args.maker_secret_hash.to_vec()),
            Token::Address(args.token_address),
        ])?;
        Ok(data)
    }

    /// Prepares data for EtomicSwapMakerV2 contract [spendMakerPayment](https://github.com/KomodoPlatform/etomic-swap/blob/5e15641cbf41766cd5b37b4d71842c270773f788/contracts/EtomicSwapMakerV2.sol#L104) method
    async fn prepare_spend_maker_payment_data(
        &self,
        args: SpendMakerPaymentArgs<'_, Self>,
        token_address: Address,
    ) -> Result<Vec<u8>, PrepareTxDataError> {
        let function = MAKER_SWAP_V2.function("spendMakerPayment")?;
        let id = self.etomic_swap_id_v2(args.time_lock, args.maker_secret_hash);
        let maker_address = public_to_address(args.maker_pub);
        let payment_amount = u256_from_big_decimal(&args.amount, self.decimals)
            .map_err(|e| PrepareTxDataError::Internal(e.to_string()))?;
        let data = function.encode_input(&[
            Token::FixedBytes(id),
            Token::Uint(payment_amount),
            Token::Address(maker_address),
            Token::FixedBytes(args.taker_secret_hash.to_vec()),
            Token::FixedBytes(args.maker_secret.to_vec()),
            Token::Address(token_address),
        ])?;
        Ok(data)
    }
}

/// Validation function for ETH maker payment data
fn validate_eth_maker_payment_data(
    decoded: &[Token],
    args: &MakerValidationArgs<'_>,
    func: &Function,
    tx_value: U256,
) -> Result<(), MmError<ValidatePaymentError>> {
    let checks = vec![
        (0, Token::FixedBytes(args.swap_id.clone()), "id"),
        (1, Token::Address(args.taker), "taker"),
        (2, Token::FixedBytes(args.taker_secret_hash.to_vec()), "takerSecretHash"),
        (3, Token::FixedBytes(args.maker_secret_hash.to_vec()), "makerSecretHash"),
        (4, Token::Uint(U256::from(args.payment_time_lock)), "paymentLockTime"),
    ];

    for (index, expected_token, field_name) in checks {
        let token = get_function_input_data(decoded, func, index).map_to_mm(ValidatePaymentError::InternalError)?;
        if token != expected_token {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "ETH Maker Payment `{}` {:?} is invalid, expected {:?}",
                field_name,
                decoded.get(index),
                expected_token
            )));
        }
    }
    if args.amount != tx_value {
        return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
            "ETH Maker Payment amount, is invalid, expected {:?}, got {:?}",
            args.amount, tx_value
        )));
    }
    Ok(())
}

/// Validation function for ERC20 maker payment data
fn validate_erc20_maker_payment_data(
    decoded: &[Token],
    args: &MakerValidationArgs,
    func: &Function,
    token_addr: Address,
) -> Result<(), MmError<ValidatePaymentError>> {
    let checks = vec![
        (0, Token::FixedBytes(args.swap_id.clone()), "id"),
        (1, Token::Uint(args.amount), "amount"),
        (2, Token::Address(token_addr), "tokenAddress"),
        (3, Token::Address(args.taker), "taker"),
        (4, Token::FixedBytes(args.taker_secret_hash.to_vec()), "takerSecretHash"),
        (5, Token::FixedBytes(args.maker_secret_hash.to_vec()), "makerSecretHash"),
        (6, Token::Uint(U256::from(args.payment_time_lock)), "paymentLockTime"),
    ];

    for (index, expected_token, field_name) in checks {
        let token = get_function_input_data(decoded, func, index).map_to_mm(ValidatePaymentError::InternalError)?;
        if token != expected_token {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "ERC20 Maker Payment `{}` {:?} is invalid, expected {:?}",
                field_name,
                decoded.get(index),
                expected_token
            )));
        }
    }
    Ok(())
}
