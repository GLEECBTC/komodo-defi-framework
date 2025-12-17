use crate::context::CoinsActivationContext;
use crate::init_token::{
    token_xpub_extractor_rpc_statuses, InitTokenActivationOps, InitTokenActivationResult, InitTokenAwaitingStatus,
    InitTokenError, InitTokenInProgressStatus, InitTokenTaskHandleShared, InitTokenTaskManagerShared,
    InitTokenUserAction,
};
use async_trait::async_trait;
use coins::coin_balance::{EnableCoinBalanceError, EnableCoinBalanceOps};
use coins::eth::v2_activation::{Erc20Protocol, EthTokenActivationError, InitErc20TokenActivationRequest};
use coins::eth::EthCoin;
use coins::hd_wallet::RpcTaskXPubExtractor;
use coins::{CustomTokenError, MarketCoinOps, MmCoin, RegisterCoinError};
use common::Future01CompatExt;
use crypto::HwRpcError;
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::mm_error::MmError;
use mm2_err_handle::prelude::*;
use rpc_task::RpcTaskError;
use ser_error_derive::SerializeErrorType;
use serde_derive::Serialize;
use serde_json::Value as Json;
use std::time::Duration;

pub type Erc20TokenTaskManagerShared = InitTokenTaskManagerShared<EthCoin>;

#[derive(Clone, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum InitErc20Error {
    #[display(fmt = "{_0}")]
    HwError(HwRpcError),
    #[display(fmt = "Initialization task has timed out {duration:?}")]
    TaskTimedOut {
        duration: Duration,
    },
    #[display(fmt = "Token {ticker} is activated already")]
    TokenIsAlreadyActivated {
        ticker: String,
    },
    #[display(fmt = "Error on  token {ticker} creation: {error}")]
    TokenCreationError {
        ticker: String,
        error: String,
    },
    #[display(fmt = "Could not fetch balance: {_0}")]
    CouldNotFetchBalance(String),
    #[display(fmt = "Transport error: {_0}")]
    Transport(String),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
    #[display(fmt = "Custom token error: {_0}")]
    CustomTokenError(CustomTokenError),
    PlatformCoinMismatch,
}

impl From<InitErc20Error> for InitTokenError {
    fn from(e: InitErc20Error) -> Self {
        match e {
            InitErc20Error::HwError(hw) => InitTokenError::HwError(hw),
            InitErc20Error::TaskTimedOut { duration } => InitTokenError::TaskTimedOut { duration },
            InitErc20Error::TokenIsAlreadyActivated { ticker, .. } => {
                InitTokenError::TokenIsAlreadyActivated { ticker }
            },
            InitErc20Error::TokenCreationError { ticker, error } => {
                InitTokenError::TokenCreationError { ticker, error }
            },
            InitErc20Error::CouldNotFetchBalance(error) => InitTokenError::CouldNotFetchBalance(error),
            InitErc20Error::Transport(transport) => InitTokenError::Transport(transport),
            InitErc20Error::Internal(internal) => InitTokenError::Internal(internal),
            InitErc20Error::CustomTokenError(error) => InitTokenError::CustomTokenError(error),
            InitErc20Error::PlatformCoinMismatch => InitTokenError::PlatformCoinMismatch,
        }
    }
}

impl From<EthTokenActivationError> for InitErc20Error {
    fn from(e: EthTokenActivationError) -> Self {
        match e {
            EthTokenActivationError::InternalError(_)
            | EthTokenActivationError::UnexpectedDerivationMethod(_)
            | EthTokenActivationError::PrivKeyPolicyNotAllowed(_) => InitErc20Error::Internal(e.to_string()),
            EthTokenActivationError::ClientConnectionFailed(_)
            | EthTokenActivationError::CouldNotFetchBalance(_)
            | EthTokenActivationError::InvalidPayload(_)
            | EthTokenActivationError::Transport(_) => InitErc20Error::Transport(e.to_string()),
            EthTokenActivationError::CustomTokenError(e) => InitErc20Error::CustomTokenError(e),
            EthTokenActivationError::PlatformCoinMismatch => InitErc20Error::PlatformCoinMismatch,
        }
    }
}

impl From<RegisterCoinError> for InitErc20Error {
    fn from(err: RegisterCoinError) -> InitErc20Error {
        match err {
            RegisterCoinError::CoinIsInitializedAlready { coin } => {
                InitErc20Error::TokenIsAlreadyActivated { ticker: coin }
            },
            RegisterCoinError::Internal(e) => InitErc20Error::Internal(e),
        }
    }
}

impl From<RpcTaskError> for InitErc20Error {
    fn from(rpc_err: RpcTaskError) -> Self {
        match rpc_err {
            RpcTaskError::Timeout(duration) => InitErc20Error::TaskTimedOut { duration },
            internal_error => InitErc20Error::Internal(internal_error.to_string()),
        }
    }
}

impl From<EnableCoinBalanceError> for InitErc20Error {
    fn from(e: EnableCoinBalanceError) -> Self {
        match e {
            EnableCoinBalanceError::NewAddressDerivingError(err) => InitErc20Error::Internal(err.to_string()),
            EnableCoinBalanceError::NewAccountCreationError(err) => InitErc20Error::Internal(err.to_string()),
            EnableCoinBalanceError::BalanceError(err) => InitErc20Error::CouldNotFetchBalance(err.to_string()),
        }
    }
}

#[async_trait]
impl InitTokenActivationOps for EthCoin {
    type ActivationRequest = InitErc20TokenActivationRequest;
    type ProtocolInfo = Erc20Protocol;
    type ActivationResult = InitTokenActivationResult;
    type ActivationError = InitErc20Error;
    type InProgressStatus = InitTokenInProgressStatus;
    type AwaitingStatus = InitTokenAwaitingStatus;
    type UserAction = InitTokenUserAction;

    fn rpc_task_manager(activation_ctx: &CoinsActivationContext) -> &Erc20TokenTaskManagerShared {
        &activation_ctx.init_erc20_token_task_manager
    }

    async fn init_token(
        ticker: String,
        platform_coin: Self::PlatformCoin,
        activation_request: &Self::ActivationRequest,
        token_conf: Json,
        protocol_conf: Self::ProtocolInfo,
        _task_handle: InitTokenTaskHandleShared<Self>,
        is_custom: bool,
    ) -> Result<Self, MmError<Self::ActivationError>> {
        let token = platform_coin
            .initialize_erc20_token(
                ticker,
                activation_request.clone().into(),
                token_conf,
                protocol_conf,
                is_custom,
            )
            .await
            .map_mm_err()?;

        Ok(token)
    }

    // Todo: similar to utxo_activation a common method for getting activation result can be made, needed when more protocols that have tokens are supported
    async fn get_activation_result(
        &self,
        ctx: MmArc,
        protocol_conf: Self::ProtocolInfo,
        task_handle: InitTokenTaskHandleShared<Self>,
        activation_request: &Self::ActivationRequest,
    ) -> Result<Self::ActivationResult, MmError<Self::ActivationError>> {
        let ticker = self.ticker().to_owned();
        let current_block = self
            .current_block()
            .compat()
            .await
            .map_to_mm(EthTokenActivationError::Transport)
            .map_mm_err()?;

        let xpub_extractor = if self.is_trezor() {
            Some(
                RpcTaskXPubExtractor::new_trezor_extractor(
                    &ctx,
                    task_handle.clone(),
                    token_xpub_extractor_rpc_statuses(),
                    protocol_conf.into(),
                )
                .mm_err(|_| InitErc20Error::HwError(HwRpcError::NotInitialized))?,
            )
        } else {
            None
        };

        task_handle
            .update_in_progress_status(InitTokenInProgressStatus::RequestingWalletBalance)
            .map_mm_err()?;
        let wallet_balance = self
            .enable_coin_balance(
                xpub_extractor,
                activation_request.enable_params.clone(),
                &activation_request.path_to_address,
            )
            .await
            .map_mm_err()?;
        task_handle
            .update_in_progress_status(InitTokenInProgressStatus::ActivatingCoin)
            .map_mm_err()?;

        let token_contract_address = self
            .erc20_token_address()
            .ok_or_else(|| EthTokenActivationError::InternalError("Token contract address is missing".to_string()))?;

        Ok(InitTokenActivationResult {
            ticker,
            platform_coin: self.platform_ticker().to_owned(),
            token_contract_address: format!("{token_contract_address:#02x}"),
            current_block,
            required_confirmations: self.required_confirmations(),
            wallet_balance,
        })
    }
}
