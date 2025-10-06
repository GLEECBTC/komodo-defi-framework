use crate::hd_wallet::{AddressDerivingError, InvalidBip44ChainError};
use crate::{BalanceError, CoinFindError, UnexpectedDerivationMethod};
use common::HttpStatusCode;
use crypto::Bip44Chain;
use derive_more::Display;
use http::StatusCode;
use rpc_task::RpcTaskError;
use std::time::Duration;

#[derive(Clone, Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum HDAccountBalanceRpcError {
    #[display(fmt = "No such coin {coin}")]
    NoSuchCoin { coin: String },
    #[display(fmt = "RPC timed out {_0:?}")]
    Timeout(Duration),
    #[display(fmt = "Coin is expected to be activated with the HD wallet derivation method")]
    CoinIsActivatedNotWithHDWallet,
    #[display(fmt = "HD account '{account_id}' is not activated")]
    UnknownAccount { account_id: u32 },
    #[display(fmt = "Coin doesn't support the given BIP44 chain: {chain:?}")]
    InvalidBip44Chain { chain: Bip44Chain },
    #[display(fmt = "Error deriving an address: {_0}")]
    ErrorDerivingAddress(String),
    #[display(fmt = "Wallet storage error: {_0}")]
    WalletStorageError(String),
    #[display(fmt = "Electrum/Native RPC invalid response: {_0}")]
    RpcInvalidResponse(String),
    #[display(fmt = "Failed scripthash subscription. Error: {_0}")]
    FailedScripthashSubscription(String),
    #[display(fmt = "Transport: {_0}")]
    Transport(String),
    #[display(fmt = "Internal: {_0}")]
    Internal(String),
}

impl HttpStatusCode for HDAccountBalanceRpcError {
    fn status_code(&self) -> StatusCode {
        match self {
            HDAccountBalanceRpcError::NoSuchCoin { .. }
            | HDAccountBalanceRpcError::CoinIsActivatedNotWithHDWallet
            | HDAccountBalanceRpcError::UnknownAccount { .. }
            | HDAccountBalanceRpcError::InvalidBip44Chain { .. }
            | HDAccountBalanceRpcError::ErrorDerivingAddress(_) => StatusCode::BAD_REQUEST,
            HDAccountBalanceRpcError::Timeout(_) => StatusCode::REQUEST_TIMEOUT,
            HDAccountBalanceRpcError::Transport(_)
            | HDAccountBalanceRpcError::WalletStorageError(_)
            | HDAccountBalanceRpcError::RpcInvalidResponse(_)
            | HDAccountBalanceRpcError::FailedScripthashSubscription(_)
            | HDAccountBalanceRpcError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<CoinFindError> for HDAccountBalanceRpcError {
    fn from(e: CoinFindError) -> Self {
        match e {
            CoinFindError::NoSuchCoin { coin } => HDAccountBalanceRpcError::NoSuchCoin { coin },
        }
    }
}

impl From<UnexpectedDerivationMethod> for HDAccountBalanceRpcError {
    fn from(e: UnexpectedDerivationMethod) -> Self {
        match e {
            UnexpectedDerivationMethod::ExpectedHDWallet => HDAccountBalanceRpcError::CoinIsActivatedNotWithHDWallet,
            unexpected_error => HDAccountBalanceRpcError::Internal(unexpected_error.to_string()),
        }
    }
}

impl From<BalanceError> for HDAccountBalanceRpcError {
    fn from(e: BalanceError) -> Self {
        match e {
            BalanceError::Transport(transport) => HDAccountBalanceRpcError::Transport(transport),
            BalanceError::InvalidResponse(rpc) => HDAccountBalanceRpcError::RpcInvalidResponse(rpc),
            BalanceError::UnexpectedDerivationMethod(der_method) => HDAccountBalanceRpcError::from(der_method),
            BalanceError::WalletStorageError(e) | BalanceError::Internal(e) => HDAccountBalanceRpcError::Internal(e),
            BalanceError::NoSuchCoin { coin } => HDAccountBalanceRpcError::NoSuchCoin { coin },
        }
    }
}

impl From<InvalidBip44ChainError> for HDAccountBalanceRpcError {
    fn from(e: InvalidBip44ChainError) -> Self {
        HDAccountBalanceRpcError::InvalidBip44Chain { chain: e.chain }
    }
}

impl From<AddressDerivingError> for HDAccountBalanceRpcError {
    fn from(e: AddressDerivingError) -> Self {
        match e {
            AddressDerivingError::InvalidBip44Chain { chain } => HDAccountBalanceRpcError::InvalidBip44Chain { chain },
            AddressDerivingError::Bip32Error(bip32) => HDAccountBalanceRpcError::ErrorDerivingAddress(bip32),
            AddressDerivingError::Internal(internal) => HDAccountBalanceRpcError::Internal(internal),
        }
    }
}

impl From<RpcTaskError> for HDAccountBalanceRpcError {
    fn from(e: RpcTaskError) -> Self {
        match e {
            RpcTaskError::Cancelled => HDAccountBalanceRpcError::Internal("Cancelled".to_owned()),
            RpcTaskError::Timeout(timeout) => HDAccountBalanceRpcError::Timeout(timeout),
            RpcTaskError::NoSuchTask(_)
            // `UnexpectedTaskStatus` and `UnexpectedUserAction` are not expected at the balance request.
            | RpcTaskError::UnexpectedTaskStatus { .. }
            | RpcTaskError::UnexpectedUserAction { .. } => HDAccountBalanceRpcError::Internal(e.to_string()),
            RpcTaskError::Internal(internal) => HDAccountBalanceRpcError::Internal(internal),
        }
    }
}
