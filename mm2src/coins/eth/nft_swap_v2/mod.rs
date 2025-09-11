use ethabi::Token;
use ethcore_transaction::Action;
use ethereum_types::U256;
use ethkey::public_to_address;
use futures::compat::Future01CompatExt;
use mm2_err_handle::prelude::{MapToMmResult, MmError, MmResult, MmResultExt};
use mm2_number::BigDecimal;
use num_traits::Signed;
use web3::types::TransactionId;

use super::ContractType;
use crate::coin_errors::{ValidatePaymentError, ValidatePaymentResult};
use crate::eth::eth_swap_v2::{validate_from_to_addresses, PaymentMethod, PrepareTxDataError, ZERO_VALUE};
use crate::eth::{
    decode_contract_call, EthCoin, EthCoinType, SignedEthTx, ERC1155_CONTRACT, ERC721_CONTRACT, NFT_MAKER_SWAP_V2,
};
use crate::{
    ParseCoinAssocTypes, RefundNftMakerPaymentArgs, SendNftMakerPaymentArgs, SpendNftMakerPaymentArgs, TransactionErr,
    ValidateNftMakerPaymentArgs,
};

pub(crate) mod errors;
use errors::{Erc721FunctionError, HtlcParamsError};
mod structs;
use structs::{ExpectedHtlcParams, ValidationParams};

impl EthCoin {
    pub(crate) async fn send_nft_maker_payment_v2_impl(
        &self,
        args: SendNftMakerPaymentArgs<'_, Self>,
    ) -> Result<SignedEthTx, TransactionErr> {
        match &self.coin_type {
            EthCoinType::Nft { .. } => {
                try_tx_s!(validate_payment_args(
                    args.taker_secret_hash,
                    args.maker_secret_hash,
                    &args.amount,
                    args.nft_swap_info.contract_type
                ));
                let htlc_data = try_tx_s!(self.prepare_htlc_data(&args));

                let data = try_tx_s!(self.prepare_nft_maker_payment_v2_data(&args, htlc_data).await);
                let gas_limit = self
                    .gas_limit_v2
                    .nft_gas_limit(args.nft_swap_info.contract_type, PaymentMethod::Send);
                self.sign_and_send_transaction(
                    ZERO_VALUE.into(),
                    Action::Call(*args.nft_swap_info.token_address),
                    data,
                    Some(U256::from(gas_limit)),
                )
                .compat()
                .await
            },
            EthCoinType::Eth | EthCoinType::Erc20 { .. } => Err(TransactionErr::ProtocolNotSupported(ERRL!(
                "ETH and ERC20 protocols are not supported for NFT swaps."
            ))),
        }
    }

    pub(crate) async fn validate_nft_maker_payment_v2_impl(
        &self,
        args: ValidateNftMakerPaymentArgs<'_, Self>,
    ) -> ValidatePaymentResult<()> {
        match self.coin_type {
            EthCoinType::Nft { .. } => {
                let nft_maker_swap_v2_contract = self
                    .swap_v2_contracts
                    .ok_or_else(|| {
                        ValidatePaymentError::InternalError(
                            "Expected swap_v2_contracts to be Some, but found None".to_string(),
                        )
                    })?
                    .nft_maker_swap_v2_contract;
                let contract_type = args.nft_swap_info.contract_type;
                validate_payment_args(
                    args.taker_secret_hash,
                    args.maker_secret_hash,
                    &args.amount,
                    contract_type,
                )
                .map_err(ValidatePaymentError::InternalError)?;
                let token_address = args.nft_swap_info.token_address;
                let maker_address = public_to_address(args.maker_pub);
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
                validate_from_to_addresses(tx_from_rpc, maker_address, *token_address).map_mm_err()?;

                let (decoded, bytes_index) = get_decoded_tx_data_and_bytes_index(contract_type, &tx_from_rpc.input.0)?;

                let amount = if matches!(contract_type, &ContractType::Erc1155) {
                    Some(args.amount.to_string())
                } else {
                    None
                };

                let validation_params = ValidationParams {
                    maker_address,
                    nft_maker_swap_v2_contract,
                    token_id: args.nft_swap_info.token_id,
                    amount,
                };
                validate_decoded_data(&decoded, &validation_params)?;

                let taker_address = public_to_address(args.taker_pub);
                let htlc_params = ExpectedHtlcParams {
                    swap_id,
                    taker_address,
                    token_address: *token_address,
                    taker_secret_hash: args.taker_secret_hash.to_vec(),
                    maker_secret_hash: args.maker_secret_hash.to_vec(),
                    time_lock: U256::from(args.time_lock),
                };
                decode_and_validate_htlc_params(decoded, bytes_index, htlc_params).map_mm_err()?;
            },
            EthCoinType::Eth | EthCoinType::Erc20 { .. } => {
                return MmError::err(ValidatePaymentError::InternalError(
                    "EthCoinType must be Nft".to_string(),
                ))
            },
        }
        Ok(())
    }

    pub(crate) async fn spend_nft_maker_payment_v2_impl(
        &self,
        args: SpendNftMakerPaymentArgs<'_, Self>,
    ) -> Result<SignedEthTx, TransactionErr> {
        match self.coin_type {
            EthCoinType::Nft { .. } => {
                let nft_maker_swap_v2_contract = self
                    .swap_v2_contracts
                    .ok_or_else(|| {
                        TransactionErr::Plain(ERRL!("Expected swap_v2_contracts to be Some, but found None"))
                    })?
                    .nft_maker_swap_v2_contract;
                if args.maker_secret.len() != 32 {
                    return Err(TransactionErr::Plain(ERRL!("maker_secret must be 32 bytes")));
                }
                let (decoded, bytes_index) = try_tx_s!(get_decoded_tx_data_and_bytes_index(
                    args.contract_type,
                    args.maker_payment_tx.unsigned().data()
                ));

                let htlc_params = try_tx_s!(self.htlc_params_from_tx_data(&decoded, bytes_index,).await);
                let data = try_tx_s!(self.prepare_spend_nft_maker_v2_data(&args, decoded, htlc_params));
                let gas_limit = self
                    .gas_limit_v2
                    .nft_gas_limit(args.contract_type, PaymentMethod::Spend);
                self.sign_and_send_transaction(
                    ZERO_VALUE.into(),
                    Action::Call(nft_maker_swap_v2_contract),
                    data,
                    Some(U256::from(gas_limit)),
                )
                .compat()
                .await
            },
            EthCoinType::Eth | EthCoinType::Erc20 { .. } => Err(TransactionErr::ProtocolNotSupported(ERRL!(
                "ETH and ERC20 protocols are not supported for NFT swaps."
            ))),
        }
    }

    pub(crate) async fn refund_nft_maker_payment_v2_timelock_impl(
        &self,
        args: RefundNftMakerPaymentArgs<'_, Self>,
    ) -> Result<SignedEthTx, TransactionErr> {
        match self.coin_type {
            EthCoinType::Nft { .. } => {
                let nft_maker_swap_v2_contract = self
                    .swap_v2_contracts
                    .ok_or_else(|| {
                        TransactionErr::Plain(ERRL!("Expected swap_v2_contracts to be Some, but found None"))
                    })?
                    .nft_maker_swap_v2_contract;
                let (decoded, bytes_index) = try_tx_s!(get_decoded_tx_data_and_bytes_index(
                    args.contract_type,
                    args.maker_payment_tx.unsigned().data()
                ));

                let htlc_params = try_tx_s!(self.htlc_params_from_tx_data(&decoded, bytes_index,).await);
                let data = try_tx_s!(self.prepare_refund_nft_maker_payment_v2_timelock(&args, decoded, htlc_params));
                let gas_limit = self
                    .gas_limit_v2
                    .nft_gas_limit(args.contract_type, PaymentMethod::RefundTimelock);
                self.sign_and_send_transaction(
                    ZERO_VALUE.into(),
                    Action::Call(nft_maker_swap_v2_contract),
                    data,
                    Some(U256::from(gas_limit)),
                )
                .compat()
                .await
            },
            EthCoinType::Eth | EthCoinType::Erc20 { .. } => Err(TransactionErr::ProtocolNotSupported(ERRL!(
                "ETH and ERC20 protocols are not supported for NFT swaps."
            ))),
        }
    }

    pub(crate) async fn refund_nft_maker_payment_v2_secret_impl(
        &self,
        args: RefundNftMakerPaymentArgs<'_, Self>,
    ) -> Result<SignedEthTx, TransactionErr> {
        match self.coin_type {
            EthCoinType::Nft { .. } => {
                let nft_maker_swap_v2_contract = self
                    .swap_v2_contracts
                    .ok_or_else(|| {
                        TransactionErr::Plain(ERRL!("Expected swap_v2_contracts to be Some, but found None"))
                    })?
                    .nft_maker_swap_v2_contract;
                let (decoded, bytes_index) = try_tx_s!(get_decoded_tx_data_and_bytes_index(
                    args.contract_type,
                    args.maker_payment_tx.unsigned().data()
                ));

                let htlc_params = try_tx_s!(self.htlc_params_from_tx_data(&decoded, bytes_index,).await);

                let data = try_tx_s!(self.prepare_refund_nft_maker_payment_v2_secret(&args, decoded, htlc_params));
                let gas_limit = self
                    .gas_limit_v2
                    .nft_gas_limit(args.contract_type, PaymentMethod::RefundSecret);
                self.sign_and_send_transaction(
                    ZERO_VALUE.into(),
                    Action::Call(nft_maker_swap_v2_contract),
                    data,
                    Some(U256::from(gas_limit)),
                )
                .compat()
                .await
            },
            EthCoinType::Eth | EthCoinType::Erc20 { .. } => Err(TransactionErr::ProtocolNotSupported(ERRL!(
                "ETH and ERC20 protocols are not supported for NFT swaps."
            ))),
        }
    }

    async fn prepare_nft_maker_payment_v2_data(
        &self,
        args: &SendNftMakerPaymentArgs<'_, Self>,
        htlc_data: Vec<u8>,
    ) -> Result<Vec<u8>, PrepareTxDataError> {
        let nft_maker_swap_v2_contract = self
            .swap_v2_contracts
            .ok_or_else(|| {
                PrepareTxDataError::Internal("Expected swap_v2_contracts to be Some, but found None".to_string())
            })?
            .nft_maker_swap_v2_contract;
        match args.nft_swap_info.contract_type {
            ContractType::Erc1155 => {
                let function = ERC1155_CONTRACT.function("safeTransferFrom")?;
                let amount_u256 = U256::from_dec_str(&args.amount.to_string())
                    .map_err(|e| PrepareTxDataError::Internal(e.to_string()))?;
                let data = function.encode_input(&[
                    Token::Address(self.my_addr().await),
                    Token::Address(nft_maker_swap_v2_contract),
                    Token::Uint(U256::from(args.nft_swap_info.token_id)),
                    Token::Uint(amount_u256),
                    Token::Bytes(htlc_data),
                ])?;
                Ok(data)
            },
            ContractType::Erc721 => {
                let function = erc721_transfer_with_data()?;
                let data = function.encode_input(&[
                    Token::Address(self.my_addr().await),
                    Token::Address(nft_maker_swap_v2_contract),
                    Token::Uint(U256::from(args.nft_swap_info.token_id)),
                    Token::Bytes(htlc_data),
                ])?;
                Ok(data)
            },
        }
    }

    fn prepare_htlc_data(&self, args: &SendNftMakerPaymentArgs<'_, Self>) -> Result<Vec<u8>, PrepareTxDataError> {
        let taker_address = public_to_address(args.taker_pub);
        let id = self.etomic_swap_id_v2(args.time_lock, args.maker_secret_hash);
        let encoded = ethabi::encode(&[
            Token::FixedBytes(id),
            Token::Address(taker_address),
            Token::Address(*args.nft_swap_info.token_address),
            Token::FixedBytes(args.taker_secret_hash.to_vec()),
            Token::FixedBytes(args.maker_secret_hash.to_vec()),
            Token::Uint(U256::from(args.time_lock)),
        ]);
        Ok(encoded)
    }

    /// Prepares the encoded transaction data for spending a maker's NFT payment on the blockchain.
    ///
    /// This function selects the appropriate contract function based on the NFT's contract type (ERC1155 or ERC721)
    /// and encodes the input parameters required for the blockchain transaction.
    fn prepare_spend_nft_maker_v2_data(
        &self,
        args: &SpendNftMakerPaymentArgs<'_, Self>,
        decoded: Vec<Token>,
        htlc_params: Vec<Token>,
    ) -> Result<Vec<u8>, PrepareTxDataError> {
        let spend_func = match args.contract_type {
            ContractType::Erc1155 => NFT_MAKER_SWAP_V2.function("spendErc1155MakerPayment")?,
            ContractType::Erc721 => NFT_MAKER_SWAP_V2.function("spendErc721MakerPayment")?,
        };
        // Initialize tokens with common elements
        let mut input_tokens = vec![
            htlc_params[0].clone(), // swapId
            Token::Address(args.maker_payment_tx.sender()),
            Token::FixedBytes(args.taker_secret_hash.to_vec()),
            Token::FixedBytes(args.maker_secret.to_vec()),
            htlc_params[2].clone(), // tokenAddress
            decoded[2].clone(),     // tokenId
        ];
        // Add specific elements based on contract type
        if let ContractType::Erc1155 = args.contract_type {
            input_tokens.push(decoded[3].clone()); // amount
        }

        let data = spend_func.encode_input(&input_tokens)?;
        Ok(data)
    }

    fn prepare_refund_nft_maker_payment_v2_timelock(
        &self,
        args: &RefundNftMakerPaymentArgs<'_, Self>,
        decoded: Vec<Token>,
        htlc_params: Vec<Token>,
    ) -> Result<Vec<u8>, PrepareTxDataError> {
        let refund_func = match args.contract_type {
            ContractType::Erc1155 => NFT_MAKER_SWAP_V2.function("refundErc1155MakerPaymentTimelock")?,
            ContractType::Erc721 => NFT_MAKER_SWAP_V2.function("refundErc721MakerPaymentTimelock")?,
        };
        // Initialize tokens with common elements
        let mut input_tokens = vec![
            htlc_params[0].clone(), // swapId
            htlc_params[1].clone(), // takerAddress
            Token::FixedBytes(args.taker_secret_hash.to_vec()),
            Token::FixedBytes(args.maker_secret_hash.to_vec()),
            htlc_params[2].clone(), // tokenAddress
            decoded[2].clone(),     // tokenId
        ];
        // Add specific elements based on contract type
        if let ContractType::Erc1155 = args.contract_type {
            input_tokens.push(decoded[3].clone()); // amount
        }

        let data = refund_func.encode_input(&input_tokens)?;
        Ok(data)
    }

    fn prepare_refund_nft_maker_payment_v2_secret(
        &self,
        args: &RefundNftMakerPaymentArgs<'_, Self>,
        decoded: Vec<Token>,
        htlc_params: Vec<Token>,
    ) -> Result<Vec<u8>, PrepareTxDataError> {
        let refund_func = match args.contract_type {
            ContractType::Erc1155 => NFT_MAKER_SWAP_V2.function("refundErc1155MakerPaymentSecret")?,
            ContractType::Erc721 => NFT_MAKER_SWAP_V2.function("refundErc721MakerPaymentSecret")?,
        };
        // Initialize tokens with common elements
        let mut input_tokens = vec![
            htlc_params[0].clone(), // swapId
            htlc_params[1].clone(), // takerAddress
            Token::FixedBytes(args.taker_secret.to_vec()),
            Token::FixedBytes(args.maker_secret_hash.to_vec()),
            htlc_params[2].clone(), // tokenAddress
            decoded[2].clone(),     // tokenId
        ];
        // Add specific elements based on contract type
        if let ContractType::Erc1155 = args.contract_type {
            input_tokens.push(decoded[3].clone()); // amount
        }
        let data = refund_func.encode_input(&input_tokens)?;
        Ok(data)
    }

    async fn htlc_params_from_tx_data(
        &self,
        decoded_data: &[Token],
        index: usize,
    ) -> Result<Vec<Token>, PrepareTxDataError> {
        let data_bytes = match decoded_data.get(index) {
            Some(Token::Bytes(data_bytes)) => data_bytes,
            _ => {
                return Err(PrepareTxDataError::InvalidData(ERRL!(
                    "Failed to decode HTLCParams from data_bytes"
                )))
            },
        };
        let htlc_params =
            ethabi::decode(htlc_params(), data_bytes).map_err(|e| PrepareTxDataError::ABIError(ERRL!("{}", e)))?;
        Ok(htlc_params)
    }
}

/// Validates decoded data from tx input, related to `safeTransferFrom` contract call
fn validate_decoded_data(decoded: &[Token], params: &ValidationParams) -> Result<(), MmError<ValidatePaymentError>> {
    let checks = vec![
        (0, Token::Address(params.maker_address), "maker_address"),
        (
            1,
            Token::Address(params.nft_maker_swap_v2_contract),
            "nft_maker_swap_v2_contract",
        ),
        (2, Token::Uint(U256::from(params.token_id)), "token_id"),
    ];

    for (index, expected_token, field_name) in checks {
        if decoded.get(index) != Some(&expected_token) {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "NFT Maker Payment `{}` {:?} is invalid, expected {:?}",
                field_name,
                decoded.get(index),
                expected_token
            )));
        }
    }
    if let Some(amount) = &params.amount {
        let value = U256::from_dec_str(amount).map_to_mm(|e| ValidatePaymentError::InternalError(e.to_string()))?;
        if decoded[3] != Token::Uint(value) {
            return MmError::err(ValidatePaymentError::WrongPaymentTx(format!(
                "NFT Maker Payment `amount` {:?} is invalid, expected {:?}",
                decoded[3],
                Token::Uint(value)
            )));
        }
    }
    Ok(())
}

fn decode_and_validate_htlc_params(
    decoded: Vec<Token>,
    index: usize,
    expected_params: ExpectedHtlcParams,
) -> MmResult<(), HtlcParamsError> {
    let data_bytes = match decoded.get(index) {
        Some(Token::Bytes(bytes)) => bytes,
        _ => {
            return MmError::err(HtlcParamsError::InvalidData(
                "Expected Bytes for HTLCParams data".to_string(),
            ))
        },
    };

    let decoded_params = ethabi::decode(htlc_params(), data_bytes)?;

    let expected_taker_secret_hash = Token::FixedBytes(expected_params.taker_secret_hash.clone());
    let expected_maker_secret_hash = Token::FixedBytes(expected_params.maker_secret_hash.clone());

    let checks = vec![
        (0, Token::FixedBytes(expected_params.swap_id.clone()), "swap_id"),
        (1, Token::Address(expected_params.taker_address), "taker_address"),
        (2, Token::Address(expected_params.token_address), "token_address"),
        (3, expected_taker_secret_hash, "taker_secret_hash"),
        (4, expected_maker_secret_hash, "maker_secret_hash"),
        (5, Token::Uint(expected_params.time_lock), "time_lock"),
    ];

    for (index, expected_token, param_name) in checks.into_iter() {
        if decoded_params[index] != expected_token {
            return MmError::err(HtlcParamsError::WrongPaymentTx(format!(
                "Invalid '{}' {:?}, expected {:?}",
                param_name, decoded_params[index], expected_token
            )));
        }
    }

    Ok(())
}

/// Representation of the Solidity HTLCParams struct.
///
/// struct HTLCParams {
///     bytes32 id;
///     address taker;
///     address tokenAddress;
///     bytes32 takerSecretHash;
///     bytes32 makerSecretHash;
///     uint32 paymentLockTime;
/// }
fn htlc_params() -> &'static [ethabi::ParamType] {
    &[
        ethabi::ParamType::FixedBytes(32),
        ethabi::ParamType::Address,
        ethabi::ParamType::Address,
        ethabi::ParamType::FixedBytes(32),
        ethabi::ParamType::FixedBytes(32),
        ethabi::ParamType::Uint(256),
    ]
}

/// function to check if BigDecimal is a positive integer
#[inline(always)]
fn is_positive_integer(amount: &BigDecimal) -> bool {
    amount == &amount.with_scale(0) && amount.is_positive()
}

fn validate_payment_args<'a>(
    taker_secret_hash: &'a [u8],
    maker_secret_hash: &'a [u8],
    amount: &BigDecimal,
    contract_type: &ContractType,
) -> Result<(), String> {
    match contract_type {
        ContractType::Erc1155 => {
            if !is_positive_integer(amount) {
                return Err("ERC-1155 amount must be a positive integer".to_string());
            }
        },
        ContractType::Erc721 => {
            if amount != &BigDecimal::from(1) {
                return Err("ERC-721 amount must be 1".to_string());
            }
        },
    }
    if taker_secret_hash.len() != 32 {
        return Err("taker_secret_hash must be 32 bytes".to_string());
    }
    if maker_secret_hash.len() != 32 {
        return Err("maker_secret_hash must be 32 bytes".to_string());
    }

    Ok(())
}

/// Identifies the correct `"safeTransferFrom"` function based on the contract type (either ERC1155 or ERC721)
/// and decodes the provided contract call bytes using the ABI of the identified function. Additionally, it returns
/// the index position of the "bytes" field within the function's parameters.
pub(crate) fn get_decoded_tx_data_and_bytes_index(
    contract_type: &ContractType,
    contract_call_bytes: &[u8],
) -> Result<(Vec<Token>, usize), PrepareTxDataError> {
    let (send_func, bytes_index) = match contract_type {
        ContractType::Erc1155 => (ERC1155_CONTRACT.function("safeTransferFrom")?, 4),
        ContractType::Erc721 => (erc721_transfer_with_data()?, 3),
    };
    let decoded = decode_contract_call(send_func, contract_call_bytes)?;
    Ok((decoded, bytes_index))
}

/// ERC721 contract has overloaded versions of the `safeTransferFrom` function,
/// but `Contract::function` method returns only the first if there are overloaded versions of the same function.
/// Provided function retrieves the `safeTransferFrom` variant that includes a `bytes` parameter.
/// This variant is specifically used for transferring ERC721 tokens with additional data.
fn erc721_transfer_with_data<'a>() -> Result<&'a ethabi::Function, Erc721FunctionError> {
    let functions = ERC721_CONTRACT
        .functions_by_name("safeTransferFrom")
        .map_err(|e| Erc721FunctionError::ABIError(ERRL!("{}", e)))?;

    // Find the correct function variant by inspecting the input parameters.
    let function = functions
        .iter()
        .find(|f| {
            f.inputs.len() == 4
                && matches!(
                    f.inputs.last().map(|input| &input.kind),
                    Some(&ethabi::ParamType::Bytes)
                )
        })
        .ok_or_else(|| {
            Erc721FunctionError::FunctionNotFound(
                "Failed to find the correct safeTransferFrom function variant".to_string(),
            )
        })?;
    Ok(function)
}
