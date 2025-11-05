use crate::context::CoinsActivationContext;
use crate::prelude::*;
use crate::standalone_coin::{
    InitStandaloneCoinActivationOps, InitStandaloneCoinError, InitStandaloneCoinInitialStatus,
    InitStandaloneCoinTaskHandleShared, InitStandaloneCoinTaskManagerShared,
};
use async_trait::async_trait;
use coins::coin_balance::{CoinBalanceReport, IguanaWalletBalance};
use coins::coin_errors::MyAddressError;
use coins::my_tx_history_v2::TxHistoryStorage;
use coins::siacoin::{
    sia_coin_from_conf_and_params, SiaCoin, SiaCoinActivationParams, SiaCoinBuildError, SiaCoinProtocolInfo,
};
use coins::tx_history_storage::CreateTxHistoryStorageError;
use coins::{BalanceError, CoinBalance, CoinProtocol, MarketCoinOps, PrivKeyBuildPolicy, RegisterCoinError};
use crypto::hw_rpc_task::{HwRpcTaskAwaitingStatus, HwRpcTaskUserAction};
use crypto::CryptoCtxError;
use derive_more::Display;
use futures::compat::Future01CompatExt;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_event_stream::StreamingManager;
use mm2_metrics::MetricsArc;
use mm2_number::BigDecimal;
use rpc_task::RpcTaskError;
use ser_error_derive::SerializeErrorType;
use serde_derive::Serialize;
use serde_json::Value as Json;
use std::collections::HashMap;
use std::time::Duration;

pub type SiaCoinTaskManagerShared = InitStandaloneCoinTaskManagerShared<SiaCoin>;
pub type SiaCoinRpcTaskHandleShared = InitStandaloneCoinTaskHandleShared<SiaCoin>;
pub type SiaCoinAwaitingStatus = HwRpcTaskAwaitingStatus;
pub type SiaCoinUserAction = HwRpcTaskUserAction;

/// `SiaCoinActivationResult` provides information/data for Sia activation.
#[derive(Clone, Serialize)]
pub struct SiaCoinActivationResult {
    /// A string representing the ticker of the SiaCoin.
    pub ticker: String,
    pub current_block: u64,
    pub wallet_balance: CoinBalanceReport<CoinBalance>,
}

impl CurrentBlock for SiaCoinActivationResult {
    fn current_block(&self) -> u64 {
        self.current_block
    }
}

impl GetAddressesBalances for SiaCoinActivationResult {
    fn get_addresses_balances(&self) -> HashMap<String, BigDecimal> {
        self.wallet_balance
            .to_addresses_total_balances(&self.ticker)
            .into_iter()
            .map(|(address, balance)| (address, balance.unwrap_or_default()))
            .collect()
    }
}

/// `SiaCoinInProgressStatus` enumerates different states that may occur during the execution of
/// SiaCoin-related operations during coin activation.
#[derive(Clone, Serialize)]
#[non_exhaustive]
pub enum SiaCoinInProgressStatus {
    /// Indicates that SiaCoin is in the process of activating.
    ActivatingCoin,
    RequestingWalletBalance,
    /// Represents the finishing state of an operation.
    Finishing,
}

impl InitStandaloneCoinInitialStatus for SiaCoinInProgressStatus {
    fn initial_status() -> Self {
        SiaCoinInProgressStatus::ActivatingCoin
    }
}

#[derive(Clone, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
#[non_exhaustive]
pub enum SiaCoinInitError {
    #[display(fmt = "Error on coin {ticker} creation: {error}")]
    CoinCreationError {
        ticker: String,
        error: String,
    },
    CoinIsAlreadyActivated {
        ticker: String,
    },
    HardwareWalletsAreNotSupportedYet,
    #[display(fmt = "Initialization task has timed out {duration:?}")]
    TaskTimedOut {
        duration: Duration,
    },
    CouldNotGetBalance(String),
    CouldNotGetBlockCount(String),
    Internal(String),
}

impl SiaCoinInitError {
    pub fn from_build_err(build_err: SiaCoinBuildError, ticker: String) -> Self {
        SiaCoinInitError::CoinCreationError {
            ticker,
            error: build_err.to_string(),
        }
    }
}

impl From<BalanceError> for SiaCoinInitError {
    fn from(err: BalanceError) -> Self {
        SiaCoinInitError::CouldNotGetBalance(err.to_string())
    }
}

impl From<RegisterCoinError> for SiaCoinInitError {
    fn from(reg_err: RegisterCoinError) -> SiaCoinInitError {
        match reg_err {
            RegisterCoinError::CoinIsInitializedAlready { coin } => {
                SiaCoinInitError::CoinIsAlreadyActivated { ticker: coin }
            },
            RegisterCoinError::Internal(internal) => SiaCoinInitError::Internal(internal),
        }
    }
}

impl From<RpcTaskError> for SiaCoinInitError {
    fn from(rpc_err: RpcTaskError) -> Self {
        match rpc_err {
            RpcTaskError::Timeout(duration) => SiaCoinInitError::TaskTimedOut { duration },
            internal_error => SiaCoinInitError::Internal(internal_error.to_string()),
        }
    }
}

impl From<CryptoCtxError> for SiaCoinInitError {
    fn from(err: CryptoCtxError) -> Self {
        SiaCoinInitError::Internal(err.to_string())
    }
}

impl From<SiaCoinInitError> for InitStandaloneCoinError {
    fn from(err: SiaCoinInitError) -> Self {
        match err {
            SiaCoinInitError::CoinCreationError { ticker, error } => {
                InitStandaloneCoinError::CoinCreationError { ticker, error }
            },
            SiaCoinInitError::CoinIsAlreadyActivated { ticker } => {
                InitStandaloneCoinError::CoinIsAlreadyActivated { ticker }
            },
            SiaCoinInitError::HardwareWalletsAreNotSupportedYet => {
                InitStandaloneCoinError::Internal("Hardware wallets are not supported yet".into())
            },
            SiaCoinInitError::TaskTimedOut { duration } => InitStandaloneCoinError::TaskTimedOut { duration },
            SiaCoinInitError::CouldNotGetBalance(e) | SiaCoinInitError::CouldNotGetBlockCount(e) => {
                InitStandaloneCoinError::Transport(e)
            },
            SiaCoinInitError::Internal(e) => InitStandaloneCoinError::Internal(e),
        }
    }
}

impl From<CreateTxHistoryStorageError> for SiaCoinInitError {
    fn from(e: CreateTxHistoryStorageError) -> Self {
        match e {
            CreateTxHistoryStorageError::Internal(internal) => SiaCoinInitError::Internal(internal),
        }
    }
}

impl From<MyAddressError> for SiaCoinInitError {
    fn from(e: MyAddressError) -> Self {
        match e {
            MyAddressError::InternalError(internal) => SiaCoinInitError::Internal(internal),
            MyAddressError::UnexpectedDerivationMethod(internal) => SiaCoinInitError::Internal(internal),
        }
    }
}

impl TryFromCoinProtocol for SiaCoinProtocolInfo {
    fn try_from_coin_protocol(proto: CoinProtocol) -> Result<Self, MmError<CoinProtocol>>
    where
        Self: Sized,
    {
        match proto {
            CoinProtocol::SIA => Ok(SiaCoinProtocolInfo {}),
            protocol => MmError::err(protocol),
        }
    }
}

#[async_trait]
impl InitStandaloneCoinActivationOps for SiaCoin {
    type ActivationRequest = SiaCoinActivationParams;
    type StandaloneProtocol = SiaCoinProtocolInfo;
    type ActivationResult = SiaCoinActivationResult;
    type ActivationError = SiaCoinInitError;
    type InProgressStatus = SiaCoinInProgressStatus;
    type AwaitingStatus = SiaCoinAwaitingStatus;
    type UserAction = SiaCoinUserAction;

    fn rpc_task_manager(activation_ctx: &CoinsActivationContext) -> &SiaCoinTaskManagerShared {
        &activation_ctx.init_sia_task_manager
    }

    async fn init_standalone_coin(
        ctx: MmArc,
        ticker: String,
        coin_conf: Json,
        activation_request: &SiaCoinActivationParams,
        _protocol_info: SiaCoinProtocolInfo,
        _task_handle: SiaCoinRpcTaskHandleShared,
    ) -> MmResult<Self, SiaCoinInitError> {
        let priv_key_policy = PrivKeyBuildPolicy::detect_priv_key_policy(&ctx).map_mm_err()?;

        let coin = sia_coin_from_conf_and_params(&ctx, &ticker, &coin_conf, activation_request, priv_key_policy)
            .await
            .mm_err(|e| SiaCoinInitError::from_build_err(e, ticker))?;

        Ok(coin)
    }

    async fn get_activation_result(
        &self,
        _ctx: MmArc,
        task_handle: SiaCoinRpcTaskHandleShared,
        _activation_request: &Self::ActivationRequest,
    ) -> MmResult<Self::ActivationResult, SiaCoinInitError> {
        task_handle
            .update_in_progress_status(SiaCoinInProgressStatus::RequestingWalletBalance)
            .map_mm_err()?;
        let current_block = self
            .current_block()
            .compat()
            .await
            .map_to_mm(SiaCoinInitError::CouldNotGetBlockCount)
            .map_mm_err()?;

        let balance = self.my_balance().compat().await.map_mm_err()?;
        let address = self.my_address().map_mm_err()?;

        Ok(SiaCoinActivationResult {
            ticker: self.ticker().into(),
            current_block,
            wallet_balance: CoinBalanceReport::Iguana(IguanaWalletBalance { address, balance }),
        })
    }

    /// Transaction history is fetching from a wallet database for `SiaCoin`.
    fn start_history_background_fetching(
        &self,
        _metrics: MetricsArc,
        _storage: impl TxHistoryStorage,
        _streaming_manager: StreamingManager,
        _current_balances: HashMap<String, BigDecimal>,
    ) {
    }
}
