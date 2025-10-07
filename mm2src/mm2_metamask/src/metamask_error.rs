use derive_more::Display;
use jsonrpc_core::{Error as RPCError, ErrorCode as RpcErrorCode};
use mm2_err_handle::prelude::*;
use serde_derive::{Deserialize, Serialize};
use web3::Error as Web3Error;

const USER_CANCELLED_ERROR_CODE: RpcErrorCode = RpcErrorCode::ServerError(4001);

pub type MetamaskResult<T> = MmResult<T, MetamaskError>;

#[derive(Debug, Display)]
pub enum MetamaskError {
    #[display(fmt = "ETH provider not found")]
    EthProviderNotFound,
    #[display(fmt = "Expected one ETH selected account")]
    ExpectedOneEthAccount,
    #[display(fmt = "Unexpected account selected")]
    UnexpectedAccountSelected,
    #[display(fmt = "Error serializing RPC arguments: {_0}")]
    ErrorSerializingArguments(String),
    #[display(fmt = "Error deserializing RPC result: {_0}")]
    ErrorDeserializingMethodResult(String),
    #[display(fmt = "User cancelled request")]
    UserCancelled,
    #[display(fmt = "RPC error: {_0:?}")]
    Rpc(RPCError),
    #[display(fmt = "Transport error: {_0:?}")]
    Transport(String),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl From<Web3Error> for MetamaskError {
    fn from(e: Web3Error) -> Self {
        match e {
            Web3Error::Decoder(de) | Web3Error::InvalidResponse(de) => {
                MetamaskError::ErrorDeserializingMethodResult(de)
            },
            Web3Error::Transport(tr) => MetamaskError::Transport(tr.to_string()),
            Web3Error::Rpc(rpc) => {
                if rpc.code == USER_CANCELLED_ERROR_CODE {
                    MetamaskError::UserCancelled
                } else {
                    MetamaskError::Rpc(rpc)
                }
            },
            Web3Error::Io(io) => MetamaskError::Transport(io.to_string()),
            other => MetamaskError::Internal(other.to_string()),
        }
    }
}

/// This error enumeration is involved to be used as a part of another RPC error.
/// This enum consists of error types that cli/GUI must handle correctly,
/// so please extend it if it's required **only**.
///
/// Please also note that this enum is fieldless.
#[derive(Clone, Debug, Deserialize, Display, Serialize, PartialEq)]
pub enum MetamaskRpcError {
    EthProviderNotFound,
    #[display(fmt = "User cancelled request")]
    UserCancelled,
    #[display(fmt = "An unexpected ETH account selected. Please select previous account or re-initialize MetaMask")]
    UnexpectedAccountSelected,
    #[display(fmt = "Metamask context is not initialized. Consider activating it via 'task::connect_metamask::init'")]
    MetamaskCtxNotInitialized,
}

pub trait WithMetamaskRpcError {
    fn metamask_rpc_error(metamask_rpc_error: MetamaskRpcError) -> Self;
}

/// Unfortunately, it's not possible to implementing `From<MetamaskError>` for every type
/// that implements `WithMetamaskRpcError`, `WithTimeout` and `WithInternal`.
/// So this function should be called from the `From<MetamaskError>` implementation.
pub fn from_metamask_error<T>(metamask_error: MetamaskError) -> T
where
    T: WithMetamaskRpcError + WithInternal,
{
    match metamask_error {
        MetamaskError::EthProviderNotFound => T::metamask_rpc_error(MetamaskRpcError::EthProviderNotFound),
        MetamaskError::UnexpectedAccountSelected => T::metamask_rpc_error(MetamaskRpcError::UnexpectedAccountSelected),
        other => T::internal(other.to_string()),
    }
}
