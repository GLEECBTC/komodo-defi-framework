use crate::eth::eth_swap_v2::{PrepareTxDataError, ValidatePaymentV2Err};
use crate::eth::nft_swap_v2::errors::{Erc721FunctionError, HtlcParamsError};
use crate::eth::{EthAssocTypesError, EthNftAssocTypesError, Web3RpcError};
use crate::{utxo::rpc_clients::UtxoRpcError, NumConversError, UnexpectedDerivationMethod};
use derive_more::Display;
use enum_derives::EnumFromStringify;
use futures01::Future;
use mm2_err_handle::prelude::MmError;
use spv_validation::helpers_validation::SPVError;
use std::{array::TryFromSliceError, num::TryFromIntError};

/// Helper type used as result for swap payment validation function(s)
pub type ValidatePaymentFut<T> = Box<dyn Future<Item = T, Error = MmError<ValidatePaymentError>> + Send>;
/// Helper type used as result for swap payment validation function(s)
pub type ValidatePaymentResult<T> = Result<T, MmError<ValidatePaymentError>>;

/// Enum covering possible error cases of swap payment validation
#[derive(Debug, Display, EnumFromStringify)]
pub enum ValidatePaymentError {
    /// Should be used to indicate internal MM2 state problems (e.g., DB errors, etc.).
    #[from_stringify(
        "EthAssocTypesError",
        "Erc721FunctionError",
        "EthNftAssocTypesError",
        "NumConversError",
        "UnexpectedDerivationMethod",
        "keys::Error",
        "PrepareTxDataError",
        "ethabi::Error",
        "TryFromSliceError"
    )]
    InternalError(String),
    /// Problem with deserializing the transaction, or one of the transaction parts is invalid.
    #[from_stringify("rlp::DecoderError", "serialization::Error")]
    TxDeserializationError(String),
    /// One of the input parameters is invalid.
    InvalidParameter(String),
    /// Coin's RPC returned unexpected/invalid response during payment validation.
    InvalidRpcResponse(String),
    /// Payment transaction doesn't exist on-chain.
    TxDoesNotExist(String),
    /// SPV client error.
    SPVError(SPVError),
    /// Payment transaction is in unexpected state. E.g., `Uninitialized` instead of `Sent` for ETH payment.
    UnexpectedPaymentState(String),
    /// Transport (RPC) error.
    #[from_stringify("web3::Error")]
    Transport(String),
    /// Transaction has wrong properties, for example, it has been sent to a wrong address.
    WrongPaymentTx(String),
    /// Indicates error during watcher reward calculation.
    WatcherRewardError(String),
    /// Input payment timelock overflows the type used by specific coin.
    TimelockOverflow(TryFromIntError),
    ProtocolNotSupported(String),
    InvalidData(String),
    CheckSignatureError(String),
}

impl From<SPVError> for ValidatePaymentError {
    fn from(err: SPVError) -> Self {
        Self::SPVError(err)
    }
}

impl From<UtxoRpcError> for ValidatePaymentError {
    fn from(err: UtxoRpcError) -> Self {
        match err {
            UtxoRpcError::Transport(e) => Self::Transport(e.to_string()),
            UtxoRpcError::Internal(e) => Self::InternalError(e),
            _ => Self::InvalidRpcResponse(err.to_string()),
        }
    }
}

impl From<Web3RpcError> for ValidatePaymentError {
    fn from(e: Web3RpcError) -> Self {
        match e {
            Web3RpcError::Transport(tr) => ValidatePaymentError::Transport(tr),
            Web3RpcError::InvalidResponse(resp) => ValidatePaymentError::InvalidRpcResponse(resp),
            Web3RpcError::Internal(internal)
            | Web3RpcError::Timeout(internal)
            | Web3RpcError::NumConversError(internal)
            | Web3RpcError::InvalidGasApiConfig(internal) => ValidatePaymentError::InternalError(internal),
            Web3RpcError::NftProtocolNotSupported => {
                ValidatePaymentError::ProtocolNotSupported("Nft protocol is not supported".to_string())
            },
        }
    }
}

impl From<HtlcParamsError> for ValidatePaymentError {
    fn from(err: HtlcParamsError) -> Self {
        match err {
            HtlcParamsError::WrongPaymentTx(e) => ValidatePaymentError::WrongPaymentTx(e),
            HtlcParamsError::ABIError(e) | HtlcParamsError::InvalidData(e) => ValidatePaymentError::InvalidData(e),
        }
    }
}

impl From<ValidatePaymentV2Err> for ValidatePaymentError {
    fn from(err: ValidatePaymentV2Err) -> Self {
        match err {
            ValidatePaymentV2Err::WrongPaymentTx(e) => ValidatePaymentError::WrongPaymentTx(e),
        }
    }
}

#[derive(Debug, Display, EnumFromStringify)]
pub enum MyAddressError {
    #[from_stringify("UnexpectedDerivationMethod")]
    UnexpectedDerivationMethod(String),
    InternalError(String),
}

impl std::error::Error for MyAddressError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // This error doesn't wrap another error, so we return None
        None
    }
}

#[derive(Debug, Display)]
pub enum AddressFromPubkeyError {
    InternalError(String),
}
