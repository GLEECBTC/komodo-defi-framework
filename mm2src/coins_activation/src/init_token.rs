use crate::context::CoinsActivationContext;
use crate::platform_coin_with_tokens::{RegisterTokenInfo, TokenOf};
use crate::prelude::{
    coin_conf_with_protocol, CoinConfWithProtocolError, CurrentBlock, TryFromCoinProtocol,
    TryPlatformCoinFromMmCoinEnum,
};
use crate::token::TokenProtocolParams;
use async_trait::async_trait;
use coins::coin_balance::CoinBalanceReport;
use coins::{
    lp_coinfind, lp_coinfind_or_err, CoinBalanceMap, CoinProtocol, CoinsContext, CustomTokenError, MmCoinEnum,
    RegisterCoinError,
};
use common::{log, HttpStatusCode, StatusCode, SuccessResponse};
use crypto::hw_rpc_task::{HwConnectStatuses, HwRpcTaskAwaitingStatus, HwRpcTaskUserAction};
use crypto::HwRpcError;
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::mm_error::{MmError, MmResult, NotMmError};
use mm2_err_handle::prelude::*;
use rpc_task::rpc_common::{
    CancelRpcTaskError, CancelRpcTaskRequest, InitRpcTaskResponse, RpcTaskStatusError, RpcTaskStatusRequest,
    RpcTaskUserActionError, RpcTaskUserActionRequest,
};
use rpc_task::{
    RpcInitReq, RpcTask, RpcTaskError, RpcTaskHandleShared, RpcTaskManager, RpcTaskManagerShared, RpcTaskStatus,
    RpcTaskTypes, TaskId,
};
use ser_error_derive::SerializeErrorType;
use serde_derive::{Deserialize, Serialize};
use serde_json::Value as Json;
use std::time::Duration;

pub type InitTokenResponse = InitRpcTaskResponse;
pub type InitTokenStatusRequest = RpcTaskStatusRequest;
pub type InitTokenUserActionRequest<UserAction> = RpcTaskUserActionRequest<UserAction>;
pub type InitTokenTaskManagerShared<Standalone> = RpcTaskManagerShared<InitTokenTask<Standalone>>;
pub type InitTokenTaskHandleShared<Standalone> = RpcTaskHandleShared<InitTokenTask<Standalone>>;

pub type InitTokenAwaitingStatus = HwRpcTaskAwaitingStatus;
pub type InitTokenUserAction = HwRpcTaskUserAction;
pub type InitTokenStatusError = RpcTaskStatusError;
pub type InitTokenUserActionError = RpcTaskUserActionError;
pub type CancelInitTokenError = CancelRpcTaskError;

/// Request for the `init_token` RPC command.
#[derive(Debug, Deserialize, Clone)]
pub struct InitTokenReq<T> {
    ticker: String,
    protocol: Option<CoinProtocol>,
    activation_params: T,
}

/// Trait for the initializing a token using the task manager.
#[async_trait]
pub trait InitTokenActivationOps: Into<MmCoinEnum> + TokenOf + Clone + Send + Sync + 'static {
    type ActivationRequest: Clone + Send + Sync;
    type ProtocolInfo: TokenProtocolParams + TryFromCoinProtocol + Clone + Send + Sync;
    type ActivationResult: CurrentBlock + serde::Serialize + Clone + Send + Sync;
    type ActivationError: From<RegisterCoinError> + Into<InitTokenError> + SerMmErrorType + Clone + Send + Sync;
    type InProgressStatus: InitTokenInitialStatus + serde::Serialize + Clone + Send + Sync;
    type AwaitingStatus: serde::Serialize + Clone + Send + Sync;
    type UserAction: NotMmError + Send + Sync;

    /// Getter for the token initialization task manager.
    fn rpc_task_manager(activation_ctx: &CoinsActivationContext) -> &InitTokenTaskManagerShared<Self>;

    /// Activates a token and returns the activated token instance.
    async fn init_token(
        ticker: String,
        platform_coin: Self::PlatformCoin,
        activation_request: &Self::ActivationRequest,
        token_conf: Json,
        protocol_conf: Self::ProtocolInfo,
        task_handle: InitTokenTaskHandleShared<Self>,
        is_custom: bool,
    ) -> Result<Self, MmError<Self::ActivationError>>;

    /// Returns the result of the token activation.
    async fn get_activation_result(
        &self,
        ctx: MmArc,
        token_protocol: Self::ProtocolInfo,
        task_handle: InitTokenTaskHandleShared<Self>,
        activation_request: &Self::ActivationRequest,
    ) -> Result<Self::ActivationResult, MmError<Self::ActivationError>>;
}

/// Implementation of the init token RPC command.
pub async fn init_token<Token>(
    ctx: MmArc,
    request: RpcInitReq<InitTokenReq<Token::ActivationRequest>>,
) -> MmResult<InitTokenResponse, InitTokenError>
where
    Token: InitTokenActivationOps + Send + Sync + 'static,
    Token::InProgressStatus: InitTokenInitialStatus,
    InitTokenError: From<Token::ActivationError>,
{
    let (client_id, request) = (request.client_id, request.inner);
    if let Ok(Some(_)) = lp_coinfind(&ctx, &request.ticker).await {
        return MmError::err(InitTokenError::TokenIsAlreadyActivated { ticker: request.ticker });
    }

    let (token_conf, token_protocol): (_, Token::ProtocolInfo) =
        coin_conf_with_protocol(&ctx, &request.ticker, request.protocol.clone()).map_mm_err()?;

    let platform_coin = lp_coinfind_or_err(&ctx, token_protocol.platform_coin_ticker())
        .await
        .mm_err(|_| InitTokenError::PlatformCoinIsNotActivated(token_protocol.platform_coin_ticker().to_owned()))?;

    let platform_coin =
        Token::PlatformCoin::try_from_mm_coin(platform_coin).or_mm_err(|| InitTokenError::UnsupportedPlatformCoin {
            platform_coin_ticker: token_protocol.platform_coin_ticker().into(),
            token_ticker: request.ticker.clone(),
        })?;

    let coins_act_ctx = CoinsActivationContext::from_ctx(&ctx)
        .map_to_mm(InitTokenError::Internal)
        .map_mm_err()?;
    let spawner = ctx.spawner();
    let task = InitTokenTask::<Token> {
        ctx,
        request,
        token_conf,
        token_protocol,
        platform_coin,
    };
    let task_manager = Token::rpc_task_manager(&coins_act_ctx);

    let task_id = RpcTaskManager::spawn_rpc_task(task_manager, &spawner, task, client_id)
        .mm_err(|e| InitTokenError::Internal(e.to_string()))?;

    Ok(InitTokenResponse { task_id })
}

/// Implementation of the init token status RPC command.
pub async fn init_token_status<Token: InitTokenActivationOps>(
    ctx: MmArc,
    req: InitTokenStatusRequest,
) -> MmResult<
    RpcTaskStatus<Token::ActivationResult, InitTokenError, Token::InProgressStatus, Token::AwaitingStatus>,
    InitTokenStatusError,
>
where
    InitTokenError: From<Token::ActivationError>,
{
    let coins_act_ctx = CoinsActivationContext::from_ctx(&ctx).map_to_mm(InitTokenStatusError::Internal)?;
    let mut task_manager = Token::rpc_task_manager(&coins_act_ctx)
        .lock()
        .map_to_mm(|poison| InitTokenStatusError::Internal(poison.to_string()))?;
    task_manager
        .task_status(req.task_id, req.forget_if_finished)
        .or_mm_err(|| InitTokenStatusError::NoSuchTask(req.task_id))
        .map(|rpc_task| rpc_task.map_err(InitTokenError::from))
}

/// Implementation of the init token user action RPC command.
pub async fn init_token_user_action<Token: InitTokenActivationOps>(
    ctx: MmArc,
    req: InitTokenUserActionRequest<Token::UserAction>,
) -> MmResult<SuccessResponse, InitTokenUserActionError> {
    let coins_act_ctx = CoinsActivationContext::from_ctx(&ctx).map_to_mm(InitTokenUserActionError::Internal)?;
    let mut task_manager = Token::rpc_task_manager(&coins_act_ctx)
        .lock()
        .map_to_mm(|poison| InitTokenUserActionError::Internal(poison.to_string()))?;
    task_manager.on_user_action(req.task_id, req.user_action).map_mm_err()?;
    Ok(SuccessResponse::new())
}

/// Implementation of the cancel init token RPC command.
pub async fn cancel_init_token<Standalone: InitTokenActivationOps>(
    ctx: MmArc,
    req: CancelRpcTaskRequest,
) -> MmResult<SuccessResponse, CancelInitTokenError> {
    let coins_act_ctx = CoinsActivationContext::from_ctx(&ctx).map_to_mm(CancelInitTokenError::Internal)?;
    let mut task_manager = Standalone::rpc_task_manager(&coins_act_ctx)
        .lock()
        .map_to_mm(|poison| CancelInitTokenError::Internal(poison.to_string()))?;
    task_manager.cancel_task(req.task_id).map_mm_err()?;
    Ok(SuccessResponse::new())
}

/// A struct that contains the info needed by the task that initializes the token.
#[derive(Clone)]
pub struct InitTokenTask<Token: InitTokenActivationOps> {
    ctx: MmArc,
    request: InitTokenReq<Token::ActivationRequest>,
    token_conf: Json,
    token_protocol: Token::ProtocolInfo,
    platform_coin: Token::PlatformCoin,
}

impl<Token: InitTokenActivationOps> RpcTaskTypes for InitTokenTask<Token> {
    type Item = Token::ActivationResult;
    type Error = Token::ActivationError;
    type InProgressStatus = Token::InProgressStatus;
    type AwaitingStatus = Token::AwaitingStatus;
    type UserAction = Token::UserAction;
}

#[async_trait]
impl<Token> RpcTask for InitTokenTask<Token>
where
    Token: InitTokenActivationOps,
{
    fn initial_status(&self) -> Self::InProgressStatus {
        <Token::InProgressStatus as InitTokenInitialStatus>::initial_status()
    }

    /// Try to disable the coin in case if we managed to register it already.
    async fn cancel(self) {
        if let Ok(c_ctx) = CoinsContext::from_ctx(&self.ctx) {
            if let Ok(Some(coin)) = lp_coinfind(&self.ctx, &self.request.ticker).await {
                c_ctx.remove_coin(coin).await;
            };
        };
    }

    async fn run(&mut self, task_handle: RpcTaskHandleShared<Self>) -> Result<Self::Item, MmError<Self::Error>> {
        let ticker = self.request.ticker.clone();
        let token = Token::init_token(
            ticker.clone(),
            self.platform_coin.clone(),
            &self.request.activation_params,
            self.token_conf.clone(),
            self.token_protocol.clone(),
            task_handle.clone(),
            self.request.protocol.is_some(),
        )
        .await?;

        let activation_result = token
            .get_activation_result(
                self.ctx.clone(),
                self.token_protocol.clone(),
                task_handle,
                &self.request.activation_params,
            )
            .await?;
        log::info!("{} current block {}", ticker, activation_result.current_block());

        let coins_ctx = CoinsContext::from_ctx(&self.ctx).unwrap();
        coins_ctx.add_token(token.clone().into()).await.map_mm_err()?;

        self.platform_coin.register_token_info(&token);

        Ok(activation_result)
    }
}

/// Response for the init token RPC command.
#[derive(Clone, Serialize)]
pub struct InitTokenActivationResult {
    pub ticker: String,
    pub platform_coin: String,
    pub token_contract_address: String,
    pub current_block: u64,
    pub required_confirmations: u64,
    pub wallet_balance: CoinBalanceReport<CoinBalanceMap>,
}

impl CurrentBlock for InitTokenActivationResult {
    fn current_block(&self) -> u64 {
        self.current_block
    }
}

/// Trait for the initial status of the token initialization task.
pub trait InitTokenInitialStatus {
    fn initial_status() -> Self;
}

/// Status of the token initialization task.
#[derive(Clone, Serialize)]
pub enum InitTokenInProgressStatus {
    ActivatingCoin,
    TemporaryError(String),
    RequestingWalletBalance,
    Finishing,
    /// This status doesn't require the user to send `UserAction`,
    /// but it tells the user that he should confirm/decline an address on his device.
    WaitingForTrezorToConnect,
    FollowHwDeviceInstructions,
}

impl InitTokenInitialStatus for InitTokenInProgressStatus {
    fn initial_status() -> Self {
        InitTokenInProgressStatus::ActivatingCoin
    }
}

pub(crate) fn token_xpub_extractor_rpc_statuses(
) -> HwConnectStatuses<InitTokenInProgressStatus, InitTokenAwaitingStatus> {
    HwConnectStatuses {
        on_connect: InitTokenInProgressStatus::WaitingForTrezorToConnect,
        on_connected: InitTokenInProgressStatus::ActivatingCoin,
        on_connection_failed: InitTokenInProgressStatus::Finishing,
        on_button_request: InitTokenInProgressStatus::FollowHwDeviceInstructions,
        on_pin_request: InitTokenAwaitingStatus::EnterTrezorPin,
        on_passphrase_request: InitTokenAwaitingStatus::EnterTrezorPassphrase,
        on_ready: InitTokenInProgressStatus::ActivatingCoin,
    }
}

#[derive(Clone, Debug, Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum InitTokenError {
    #[display(fmt = "No such task '{_0}'")]
    NoSuchTask(TaskId),
    #[display(fmt = "Initialization task has timed out {duration:?}")]
    TaskTimedOut {
        duration: Duration,
    },
    #[display(fmt = "Token {ticker} is activated already")]
    TokenIsAlreadyActivated {
        ticker: String,
    },
    #[display(fmt = "Token {_0} config is not found")]
    TokenConfigIsNotFound(String),
    #[display(fmt = "Token {ticker} protocol parsing failed: {error}")]
    TokenProtocolParseError {
        ticker: String,
        error: String,
    },
    #[display(fmt = "Unexpected platform protocol {protocol} for {ticker}")]
    UnexpectedTokenProtocol {
        ticker: String,
        protocol: Json,
    },
    #[display(fmt = "Error on platform coin {ticker} creation: {error}")]
    TokenCreationError {
        ticker: String,
        error: String,
    },
    #[display(fmt = "Could not fetch balance: {_0}")]
    CouldNotFetchBalance(String),
    #[display(fmt = "Platform coin {_0} is not activated")]
    PlatformCoinIsNotActivated(String),
    #[display(fmt = "{platform_coin_ticker} is not a platform coin for token {token_ticker}")]
    UnsupportedPlatformCoin {
        platform_coin_ticker: String,
        token_ticker: String,
    },
    #[display(fmt = "Custom token error: {_0}")]
    CustomTokenError(CustomTokenError),
    #[display(fmt = "{_0}")]
    HwError(HwRpcError),
    #[display(fmt = "Transport error: {_0}")]
    Transport(String),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
    PlatformCoinMismatch,
}

impl From<CoinConfWithProtocolError> for InitTokenError {
    fn from(e: CoinConfWithProtocolError) -> Self {
        match e {
            CoinConfWithProtocolError::ConfigIsNotFound(error) => InitTokenError::TokenConfigIsNotFound(error),
            CoinConfWithProtocolError::CoinProtocolParseError { ticker, err } => {
                InitTokenError::TokenProtocolParseError {
                    ticker,
                    error: err.to_string(),
                }
            },
            CoinConfWithProtocolError::UnexpectedProtocol { ticker, protocol } => {
                InitTokenError::UnexpectedTokenProtocol { ticker, protocol }
            },
            CoinConfWithProtocolError::CustomTokenError(e) => InitTokenError::CustomTokenError(e),
        }
    }
}

impl From<RpcTaskError> for InitTokenError {
    fn from(e: RpcTaskError) -> Self {
        match e {
            RpcTaskError::NoSuchTask(task_id) => InitTokenError::NoSuchTask(task_id),
            RpcTaskError::Timeout(duration) => InitTokenError::TaskTimedOut { duration },
            rpc_internal => InitTokenError::Internal(rpc_internal.to_string()),
        }
    }
}

impl HttpStatusCode for InitTokenError {
    fn status_code(&self) -> StatusCode {
        match self {
            InitTokenError::NoSuchTask(_)
            | InitTokenError::TokenIsAlreadyActivated { .. }
            | InitTokenError::TokenConfigIsNotFound { .. }
            | InitTokenError::TokenProtocolParseError { .. }
            | InitTokenError::UnexpectedTokenProtocol { .. }
            | InitTokenError::TokenCreationError { .. }
            | InitTokenError::PlatformCoinIsNotActivated(_)
            | InitTokenError::PlatformCoinMismatch
            | InitTokenError::CustomTokenError(_) => StatusCode::BAD_REQUEST,
            InitTokenError::TaskTimedOut { .. } => StatusCode::REQUEST_TIMEOUT,
            InitTokenError::HwError(_) => StatusCode::GONE,
            InitTokenError::CouldNotFetchBalance(_)
            | InitTokenError::UnsupportedPlatformCoin { .. }
            | InitTokenError::Transport(_)
            | InitTokenError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}
