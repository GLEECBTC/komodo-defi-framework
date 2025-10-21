use crate::standalone_coin::InitStandaloneCoinError;
use coins::coin_balance::EnableCoinBalanceError;
use coins::hd_wallet::{NewAccountCreationError, NewAddressDerivingError};
use coins::tx_history_storage::CreateTxHistoryStorageError;
use coins::utxo::utxo_builder::UtxoCoinBuildError;
use coins::{BalanceError, RegisterCoinError};
use crypto::{CryptoCtxError, HwError, HwRpcError};
use derive_more::Display;
use rpc_task::RpcTaskError;
use ser_error_derive::SerializeErrorType;
use serde_derive::Serialize;
use std::time::Duration;

#[derive(Clone, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum InitUtxoStandardError {
    #[display(fmt = "{_0}")]
    HwError(HwRpcError),
    #[display(fmt = "Initialization task has timed out {duration:?}")]
    TaskTimedOut { duration: Duration },
    #[display(fmt = "Coin {ticker} is activated already")]
    CoinIsAlreadyActivated { ticker: String },
    #[display(fmt = "Error on platform coin {ticker} creation: {error}")]
    CoinCreationError { ticker: String, error: String },
    #[display(fmt = "Transport error: {_0}")]
    Transport(String),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl From<RpcTaskError> for InitUtxoStandardError {
    fn from(rpc_err: RpcTaskError) -> Self {
        match rpc_err {
            RpcTaskError::Timeout(duration) => InitUtxoStandardError::TaskTimedOut { duration },
            internal_error => InitUtxoStandardError::Internal(internal_error.to_string()),
        }
    }
}

impl From<CryptoCtxError> for InitUtxoStandardError {
    /// `CryptoCtx` is expected to be initialized already.
    fn from(crypto_err: CryptoCtxError) -> Self {
        InitUtxoStandardError::Internal(crypto_err.to_string())
    }
}

impl From<CreateTxHistoryStorageError> for InitUtxoStandardError {
    fn from(e: CreateTxHistoryStorageError) -> Self {
        match e {
            CreateTxHistoryStorageError::Internal(internal) => InitUtxoStandardError::Internal(internal),
        }
    }
}

impl From<InitUtxoStandardError> for InitStandaloneCoinError {
    fn from(e: InitUtxoStandardError) -> Self {
        match e {
            InitUtxoStandardError::HwError(hw) => InitStandaloneCoinError::HwError(hw),
            InitUtxoStandardError::TaskTimedOut { duration } => InitStandaloneCoinError::TaskTimedOut { duration },
            InitUtxoStandardError::CoinIsAlreadyActivated { ticker } => {
                InitStandaloneCoinError::CoinIsAlreadyActivated { ticker }
            },
            InitUtxoStandardError::CoinCreationError { ticker, error } => {
                InitStandaloneCoinError::CoinCreationError { ticker, error }
            },
            InitUtxoStandardError::Transport(transport) => InitStandaloneCoinError::Transport(transport),
            InitUtxoStandardError::Internal(internal) => InitStandaloneCoinError::Internal(internal),
        }
    }
}

impl InitUtxoStandardError {
    pub fn from_build_err(build_err: UtxoCoinBuildError, ticker: String) -> Self {
        match build_err {
            UtxoCoinBuildError::Internal(internal) => InitUtxoStandardError::Internal(internal),
            build_err => InitUtxoStandardError::CoinCreationError {
                ticker,
                error: build_err.to_string(),
            },
        }
    }

    pub fn from_enable_coin_balance_err(enable_coin_balance_err: EnableCoinBalanceError, ticker: String) -> Self {
        match enable_coin_balance_err {
            EnableCoinBalanceError::NewAddressDerivingError(addr) => {
                Self::from_new_address_deriving_error(addr, ticker)
            },
            EnableCoinBalanceError::NewAccountCreationError(acc) => Self::from_new_account_err(acc, ticker),
            EnableCoinBalanceError::BalanceError(balance) => Self::from_balance_err(balance, ticker),
        }
    }

    fn from_new_address_deriving_error(new_addr_err: NewAddressDerivingError, ticker: String) -> Self {
        InitUtxoStandardError::CoinCreationError {
            ticker,
            error: new_addr_err.to_string(),
        }
    }

    fn from_new_account_err(new_acc_err: NewAccountCreationError, ticker: String) -> Self {
        match new_acc_err {
            NewAccountCreationError::RpcTaskError(rpc) => Self::from(rpc),
            NewAccountCreationError::HardwareWalletError(hw_err) => Self::from_hw_err(hw_err, ticker),
            NewAccountCreationError::Internal(internal) => InitUtxoStandardError::Internal(internal),
            other => InitUtxoStandardError::CoinCreationError {
                ticker,
                error: other.to_string(),
            },
        }
    }

    fn from_hw_err(hw_error: HwError, ticker: String) -> Self {
        match hw_error {
            HwError::NoTrezorDeviceAvailable | HwError::DeviceDisconnected => {
                InitUtxoStandardError::HwError(HwRpcError::NoTrezorDeviceAvailable)
            },
            HwError::CannotChooseDevice { .. } => InitUtxoStandardError::HwError(HwRpcError::FoundMultipleDevices),
            HwError::ConnectionTimedOut { timeout } => InitUtxoStandardError::TaskTimedOut { duration: timeout },
            HwError::FoundUnexpectedDevice => InitUtxoStandardError::HwError(HwRpcError::FoundUnexpectedDevice),
            HwError::InvalidPin
            | HwError::UnexpectedMessage
            | HwError::ButtonExpected
            | HwError::DataError
            | HwError::PinExpected
            | HwError::InvalidSignature
            | HwError::ProcessError
            | HwError::NotEnoughFunds
            | HwError::NotInitialized
            | HwError::WipeCodeMismatch
            | HwError::InvalidSession
            | HwError::FirmwareError
            | HwError::FailureMessageNotFound
            | HwError::UserCancelled => InitUtxoStandardError::CoinCreationError {
                ticker,
                error: hw_error.to_string(),
            },
            other => InitUtxoStandardError::Internal(other.to_string()),
        }
    }

    fn from_balance_err(balance_err: BalanceError, ticker: String) -> Self {
        match balance_err {
            BalanceError::Transport(transport) | BalanceError::InvalidResponse(transport) => {
                InitUtxoStandardError::Transport(transport)
            },
            BalanceError::Internal(internal) => InitUtxoStandardError::Internal(internal),
            other => InitUtxoStandardError::CoinCreationError {
                ticker,
                error: other.to_string(),
            },
        }
    }
}

impl From<RegisterCoinError> for InitUtxoStandardError {
    fn from(reg_err: RegisterCoinError) -> InitUtxoStandardError {
        match reg_err {
            RegisterCoinError::CoinIsInitializedAlready { coin } => {
                InitUtxoStandardError::CoinIsAlreadyActivated { ticker: coin }
            },
            RegisterCoinError::Internal(internal) => InitUtxoStandardError::Internal(internal),
        }
    }
}
