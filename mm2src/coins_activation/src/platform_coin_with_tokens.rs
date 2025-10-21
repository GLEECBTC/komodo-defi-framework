use std::time::Duration;

use crate::context::CoinsActivationContext;
use crate::prelude::*;
use async_trait::async_trait;
use coins::my_tx_history_v2::TxHistoryStorage;
use coins::tx_history_storage::{CreateTxHistoryStorageError, TxHistoryStorageBuilder};
use coins::{
    lp_coinfind, lp_coinfind_any, CoinProtocol, CoinsContext, CustomTokenError, MmCoinEnum, PrivKeyPolicyNotAllowed,
    UnexpectedDerivationMethod,
};
use common::{log, HttpStatusCode, StatusCode, SuccessResponse};
use crypto::hw_rpc_task::{HwConnectStatuses, HwRpcTaskAwaitingStatus, HwRpcTaskUserAction};
use crypto::CryptoCtxError;
use derive_more::Display;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use mm2_number::BigDecimal;
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

pub type InitPlatformCoinWithTokensStatusError = RpcTaskStatusError;
pub type InitPlatformCoinWithTokensUserActionError = RpcTaskUserActionError;
pub type CancelInitPlatformCoinWithTokensError = CancelRpcTaskError;

pub type InitPlatformCoinWithTokensAwaitingStatus = HwRpcTaskAwaitingStatus;
pub type InitPlatformCoinWithTokensUserAction = HwRpcTaskUserAction;
pub type EnablePlatformCoinWithTokensResponse = InitRpcTaskResponse;
pub type EnablePlatformCoinWithTokensStatusRequest = RpcTaskStatusRequest;
pub type InitPlatformCoinWithTokensUserActionRequest<UserAction> = RpcTaskUserActionRequest<UserAction>;
pub type InitPlatformCoinWithTokensTaskManagerShared<Platform> =
    RpcTaskManagerShared<InitPlatformCoinWithTokensTask<Platform>>;

#[derive(Clone, Debug, Deserialize)]
pub struct TokenActivationRequest<Req> {
    ticker: String,
    protocol: Option<CoinProtocol>,
    #[serde(flatten)]
    request: Req,
}

pub trait TokenOf: Into<MmCoinEnum> {
    type PlatformCoin: TryPlatformCoinFromMmCoinEnum
        + PlatformCoinWithTokensActivationOps
        + RegisterTokenInfo<Self>
        + Clone;
}

pub struct TokenActivationParams<Req, Protocol> {
    pub(crate) ticker: String,
    pub(crate) conf: Json,
    pub(crate) activation_request: Req,
    pub(crate) protocol: Protocol,
    pub(crate) is_custom: bool,
}

#[async_trait]
pub trait TokenInitializer {
    type Token: TokenOf;
    type TokenActivationRequest: Send;
    type TokenProtocol: TryFromCoinProtocol + Send;
    type InitTokensError: NotMmError;

    fn tokens_requests_from_platform_request(
        platform_request: &<<Self::Token as TokenOf>::PlatformCoin as PlatformCoinWithTokensActivationOps>::ActivationRequest,
    ) -> Vec<TokenActivationRequest<Self::TokenActivationRequest>>;

    async fn enable_tokens(
        &self,
        params: Vec<TokenActivationParams<Self::TokenActivationRequest, Self::TokenProtocol>>,
    ) -> Result<Vec<Self::Token>, MmError<Self::InitTokensError>>;

    fn platform_coin(&self) -> &<Self::Token as TokenOf>::PlatformCoin;

    /// Currently ensures platform coin from the token protocol matches the platfrom coin ticker
    fn validate_token_params(
        &self,
        params: &[TokenActivationParams<Self::TokenActivationRequest, Self::TokenProtocol>],
    ) -> MmResult<(), Self::InitTokensError>;
}

#[async_trait]
pub trait TokenAsMmCoinInitializer: Send + Sync {
    type PlatformCoin;
    type ActivationRequest;

    async fn enable_tokens_as_mm_coins(
        &self,
        ctx: &MmArc,
        request: &Self::ActivationRequest,
    ) -> Result<Vec<MmCoinEnum>, MmError<InitTokensAsMmCoinsError>>;
}

pub enum InitTokensAsMmCoinsError {
    TokenConfigIsNotFound(String),
    CouldNotFetchBalance(String),
    UnexpectedDerivationMethod(UnexpectedDerivationMethod),
    Internal(String),
    TokenProtocolParseError { ticker: String, error: String },
    UnexpectedTokenProtocol { ticker: String, protocol: Json },
    Transport(String),
    InvalidPayload(String),
    CustomTokenError(CustomTokenError),
    PlatformCoinMismatch,
}

impl From<CoinConfWithProtocolError> for InitTokensAsMmCoinsError {
    fn from(err: CoinConfWithProtocolError) -> Self {
        match err {
            CoinConfWithProtocolError::ConfigIsNotFound(e) => InitTokensAsMmCoinsError::TokenConfigIsNotFound(e),
            CoinConfWithProtocolError::CoinProtocolParseError { ticker, err } => {
                InitTokensAsMmCoinsError::TokenProtocolParseError {
                    ticker,
                    error: err.to_string(),
                }
            },
            CoinConfWithProtocolError::UnexpectedProtocol { ticker, protocol } => {
                InitTokensAsMmCoinsError::UnexpectedTokenProtocol { ticker, protocol }
            },
            CoinConfWithProtocolError::CustomTokenError(e) => InitTokensAsMmCoinsError::CustomTokenError(e),
        }
    }
}

pub trait RegisterTokenInfo<T: TokenOf> {
    fn register_token_info(&self, token: &T);
}

#[async_trait]
impl<T> TokenAsMmCoinInitializer for T
where
    T: TokenInitializer + Send + Sync,
    InitTokensAsMmCoinsError: From<T::InitTokensError>,
{
    type PlatformCoin = <T::Token as TokenOf>::PlatformCoin;
    type ActivationRequest = <Self::PlatformCoin as PlatformCoinWithTokensActivationOps>::ActivationRequest;

    async fn enable_tokens_as_mm_coins(
        &self,
        ctx: &MmArc,
        request: &Self::ActivationRequest,
    ) -> Result<Vec<MmCoinEnum>, MmError<InitTokensAsMmCoinsError>> {
        let tokens_requests = T::tokens_requests_from_platform_request(request);
        let token_params = tokens_requests
            .into_iter()
            .map(|req| -> Result<_, MmError<CoinConfWithProtocolError>> {
                let (token_conf, protocol) =
                    coin_conf_with_protocol(ctx, &req.ticker, req.protocol.clone()).map_mm_err()?;
                Ok(TokenActivationParams {
                    ticker: req.ticker,
                    conf: token_conf,
                    activation_request: req.request,
                    protocol,
                    is_custom: req.protocol.is_some(),
                })
            })
            .collect::<Result<Vec<_>, _>>()
            .map_mm_err()?;

        self.validate_token_params(&token_params).map_mm_err()?;
        let tokens = self.enable_tokens(token_params).await.map_mm_err()?;
        for token in tokens.iter() {
            self.platform_coin().register_token_info(token);
        }
        Ok(tokens.into_iter().map(Into::into).collect())
    }
}

pub trait GetPlatformBalance {
    fn get_platform_balance(&self) -> Option<BigDecimal>;
}

#[async_trait]
pub trait PlatformCoinWithTokensActivationOps: Into<MmCoinEnum> + Clone + Send + Sync + 'static {
    type ActivationRequest: Clone + Send + Sync + TxHistory + ActivationRequestInfo;
    type PlatformProtocolInfo: TryFromCoinProtocol + Send;
    type ActivationResult: GetPlatformBalance + CurrentBlock + serde::Serialize + Send + Clone + Sync + 'static;
    type ActivationError: NotMmError + std::fmt::Debug + Into<EnablePlatformCoinWithTokensError> + Clone + Send + Sync;

    type InProgressStatus: InitPlatformCoinWithTokensInitialStatus + serde::Serialize + Clone + Send + Sync;
    type AwaitingStatus: serde::Serialize + Clone + Send + Sync;
    type UserAction: NotMmError + Send + Sync;

    /// Initializes the platform coin itself
    async fn enable_platform_coin(
        ctx: MmArc,
        ticker: String,
        coin_conf: &Json,
        activation_request: Self::ActivationRequest,
        protocol_conf: Self::PlatformProtocolInfo,
    ) -> Result<Self, MmError<Self::ActivationError>>;

    async fn enable_global_nft(
        &self,
        activation_request: &Self::ActivationRequest,
    ) -> Result<Option<MmCoinEnum>, MmError<Self::ActivationError>>;

    fn try_from_mm_coin(coin: MmCoinEnum) -> Option<Self>
    where
        Self: Sized;

    fn token_initializers(
        &self,
    ) -> Vec<Box<dyn TokenAsMmCoinInitializer<PlatformCoin = Self, ActivationRequest = Self::ActivationRequest>>>;

    async fn get_activation_result(
        &self,
        task_handle: Option<RpcTaskHandleShared<InitPlatformCoinWithTokensTask<Self>>>,
        activation_request: &Self::ActivationRequest,
        nft_global: &Option<MmCoinEnum>,
    ) -> Result<Self::ActivationResult, MmError<Self::ActivationError>>
    where
        EnablePlatformCoinWithTokensError: From<Self::ActivationError>;

    fn start_history_background_fetching(
        &self,
        ctx: MmArc,
        storage: impl TxHistoryStorage,
        initial_balance: Option<BigDecimal>,
    );

    fn rpc_task_manager(activation_ctx: &CoinsActivationContext) -> &InitPlatformCoinWithTokensTaskManagerShared<Self>
    where
        EnablePlatformCoinWithTokensError: From<Self::ActivationError>;
}

#[derive(Debug, Deserialize, Clone)]
pub struct EnablePlatformCoinWithTokensReq<T: Clone> {
    ticker: String,
    #[serde(flatten)]
    request: T,
}

#[derive(Debug, Display, Serialize, SerializeErrorType, Clone)]
#[serde(tag = "error_type", content = "error_data")]
pub enum EnablePlatformCoinWithTokensError {
    PlatformIsAlreadyActivated(String),
    #[display(fmt = "Platform {_0} config is not found")]
    PlatformConfigIsNotFound(String),
    #[display(fmt = "Platform coin {ticker} protocol parsing failed: {error}")]
    CoinProtocolParseError {
        ticker: String,
        error: String,
    },
    #[display(fmt = "Unexpected platform protocol {protocol} for {ticker}")]
    UnexpectedPlatformProtocol {
        ticker: String,
        protocol: Json,
    },
    #[display(fmt = "Token {_0} config is not found")]
    TokenConfigIsNotFound(String),
    #[display(fmt = "Token {ticker} protocol parsing failed: {error}")]
    TokenProtocolParseError {
        ticker: String,
        error: String,
    },
    #[display(fmt = "Unexpected token protocol {protocol} for {ticker}")]
    UnexpectedTokenProtocol {
        ticker: String,
        protocol: Json,
    },
    #[display(fmt = "Error on platform coin {ticker} creation: {error}")]
    PlatformCoinCreationError {
        ticker: String,
        error: String,
    },
    #[display(fmt = "Private key is not allowed: {_0}")]
    PrivKeyPolicyNotAllowed(PrivKeyPolicyNotAllowed),
    #[display(fmt = "Unexpected derivation method: {_0}")]
    UnexpectedDerivationMethod(String),
    Transport(String),
    AtLeastOneNodeRequired(String),
    InvalidPayload(String),
    #[display(fmt = "Failed spawning balance events. Error: {_0}")]
    FailedSpawningBalanceEvents(String),
    Internal(String),
    #[display(fmt = "No such task '{_0}'")]
    NoSuchTask(TaskId),
    #[display(fmt = "Initialization task has timed out {duration:?}")]
    TaskTimedOut {
        duration: Duration,
    },
    #[display(fmt = "Hardware policy must be activated within task manager")]
    UnexpectedDeviceActivationPolicy,
    #[display(fmt = "Custom token error: {_0}")]
    CustomTokenError(CustomTokenError),
    #[display(fmt = "WalletConnect Error: {_0}")]
    WalletConnectError(String),
    PlatformCoinMismatch,
}

impl From<CoinConfWithProtocolError> for EnablePlatformCoinWithTokensError {
    fn from(err: CoinConfWithProtocolError) -> Self {
        match err {
            CoinConfWithProtocolError::ConfigIsNotFound(ticker) => {
                EnablePlatformCoinWithTokensError::PlatformConfigIsNotFound(ticker)
            },
            CoinConfWithProtocolError::UnexpectedProtocol { ticker, protocol } => {
                EnablePlatformCoinWithTokensError::UnexpectedPlatformProtocol { ticker, protocol }
            },
            CoinConfWithProtocolError::CoinProtocolParseError { ticker, err } => {
                EnablePlatformCoinWithTokensError::CoinProtocolParseError {
                    ticker,
                    error: err.to_string(),
                }
            },
            CoinConfWithProtocolError::CustomTokenError(e) => EnablePlatformCoinWithTokensError::CustomTokenError(e),
        }
    }
}

impl From<InitTokensAsMmCoinsError> for EnablePlatformCoinWithTokensError {
    fn from(err: InitTokensAsMmCoinsError) -> Self {
        match err {
            InitTokensAsMmCoinsError::TokenConfigIsNotFound(ticker) => {
                EnablePlatformCoinWithTokensError::TokenConfigIsNotFound(ticker)
            },
            InitTokensAsMmCoinsError::TokenProtocolParseError { ticker, error } => {
                EnablePlatformCoinWithTokensError::TokenProtocolParseError { ticker, error }
            },
            InitTokensAsMmCoinsError::UnexpectedTokenProtocol { ticker, protocol } => {
                EnablePlatformCoinWithTokensError::UnexpectedTokenProtocol { ticker, protocol }
            },
            InitTokensAsMmCoinsError::Internal(e) => EnablePlatformCoinWithTokensError::Internal(e),
            InitTokensAsMmCoinsError::CouldNotFetchBalance(e) | InitTokensAsMmCoinsError::Transport(e) => {
                EnablePlatformCoinWithTokensError::Transport(e)
            },
            InitTokensAsMmCoinsError::InvalidPayload(e) => EnablePlatformCoinWithTokensError::InvalidPayload(e),
            InitTokensAsMmCoinsError::UnexpectedDerivationMethod(e) => {
                EnablePlatformCoinWithTokensError::UnexpectedDerivationMethod(e.to_string())
            },
            InitTokensAsMmCoinsError::CustomTokenError(e) => EnablePlatformCoinWithTokensError::CustomTokenError(e),
            InitTokensAsMmCoinsError::PlatformCoinMismatch => EnablePlatformCoinWithTokensError::PlatformCoinMismatch,
        }
    }
}

impl From<CreateTxHistoryStorageError> for EnablePlatformCoinWithTokensError {
    fn from(e: CreateTxHistoryStorageError) -> Self {
        match e {
            CreateTxHistoryStorageError::Internal(internal) => EnablePlatformCoinWithTokensError::Internal(internal),
        }
    }
}

impl From<CryptoCtxError> for EnablePlatformCoinWithTokensError {
    fn from(e: CryptoCtxError) -> Self {
        EnablePlatformCoinWithTokensError::Internal(e.to_string())
    }
}

impl From<RpcTaskError> for EnablePlatformCoinWithTokensError {
    fn from(e: RpcTaskError) -> Self {
        match e {
            RpcTaskError::NoSuchTask(task_id) => EnablePlatformCoinWithTokensError::NoSuchTask(task_id),
            RpcTaskError::Timeout(duration) => EnablePlatformCoinWithTokensError::TaskTimedOut { duration },
            rpc_internal => EnablePlatformCoinWithTokensError::Internal(rpc_internal.to_string()),
        }
    }
}

impl HttpStatusCode for EnablePlatformCoinWithTokensError {
    fn status_code(&self) -> StatusCode {
        match self {
            EnablePlatformCoinWithTokensError::CoinProtocolParseError { .. }
            | EnablePlatformCoinWithTokensError::TokenProtocolParseError { .. }
            | EnablePlatformCoinWithTokensError::PlatformCoinCreationError { .. }
            | EnablePlatformCoinWithTokensError::PrivKeyPolicyNotAllowed(_)
            | EnablePlatformCoinWithTokensError::UnexpectedDerivationMethod(_)
            | EnablePlatformCoinWithTokensError::Internal(_)
            | EnablePlatformCoinWithTokensError::TaskTimedOut { .. }
            | EnablePlatformCoinWithTokensError::CustomTokenError(_) => StatusCode::INTERNAL_SERVER_ERROR,
            EnablePlatformCoinWithTokensError::PlatformIsAlreadyActivated(_)
            | EnablePlatformCoinWithTokensError::PlatformConfigIsNotFound(_)
            | EnablePlatformCoinWithTokensError::TokenConfigIsNotFound(_)
            | EnablePlatformCoinWithTokensError::UnexpectedPlatformProtocol { .. }
            | EnablePlatformCoinWithTokensError::InvalidPayload { .. }
            | EnablePlatformCoinWithTokensError::AtLeastOneNodeRequired(_)
            | EnablePlatformCoinWithTokensError::NoSuchTask(_)
            | EnablePlatformCoinWithTokensError::UnexpectedDeviceActivationPolicy
            | EnablePlatformCoinWithTokensError::FailedSpawningBalanceEvents(_)
            | EnablePlatformCoinWithTokensError::WalletConnectError(_)
            | EnablePlatformCoinWithTokensError::PlatformCoinMismatch
            | EnablePlatformCoinWithTokensError::UnexpectedTokenProtocol { .. } => StatusCode::BAD_REQUEST,
            EnablePlatformCoinWithTokensError::Transport(_) => StatusCode::BAD_GATEWAY,
        }
    }
}

pub async fn re_enable_passive_platform_coin_with_tokens<Platform>(
    ctx: MmArc,
    platform_coin: Platform,
    task_handle: Option<RpcTaskHandleShared<InitPlatformCoinWithTokensTask<Platform>>>,
    req: EnablePlatformCoinWithTokensReq<Platform::ActivationRequest>,
) -> Result<Platform::ActivationResult, MmError<EnablePlatformCoinWithTokensError>>
where
    Platform: PlatformCoinWithTokensActivationOps + Clone,
    EnablePlatformCoinWithTokensError: From<Platform::ActivationError>,
{
    let mut mm_tokens = Vec::new();
    for initializer in platform_coin.token_initializers() {
        let tokens = initializer
            .enable_tokens_as_mm_coins(&ctx, &req.request)
            .await
            .map_mm_err()?;

        mm_tokens.extend(tokens);
    }

    let nft_global = platform_coin.enable_global_nft(&req.request).await.map_mm_err()?;

    let activation_result = platform_coin
        .get_activation_result(task_handle, &req.request, &nft_global)
        .await
        .map_mm_err()?;
    log::info!("{} current block {}", req.ticker, activation_result.current_block());

    let coins_ctx = CoinsContext::from_ctx(&ctx).unwrap();
    coins_ctx
        .add_platform_with_tokens(platform_coin.clone().into(), mm_tokens, nft_global)
        .await
        .mm_err(|e| EnablePlatformCoinWithTokensError::PlatformIsAlreadyActivated(e.ticker))?;

    Ok(activation_result)
}

pub async fn enable_platform_coin_with_tokens<Platform>(
    ctx: MmArc,
    req: EnablePlatformCoinWithTokensReq<Platform::ActivationRequest>,
) -> Result<Platform::ActivationResult, MmError<EnablePlatformCoinWithTokensError>>
where
    Platform: PlatformCoinWithTokensActivationOps,
    EnablePlatformCoinWithTokensError: From<Platform::ActivationError>,
{
    if req.request.is_hw_policy() {
        return MmError::err(EnablePlatformCoinWithTokensError::UnexpectedDeviceActivationPolicy);
    }
    enable_platform_coin_with_tokens_impl::<Platform>(ctx, None, req).await
}

pub async fn enable_platform_coin_with_tokens_impl<Platform>(
    ctx: MmArc,
    task_handle: Option<RpcTaskHandleShared<InitPlatformCoinWithTokensTask<Platform>>>,
    req: EnablePlatformCoinWithTokensReq<Platform::ActivationRequest>,
) -> Result<Platform::ActivationResult, MmError<EnablePlatformCoinWithTokensError>>
where
    Platform: PlatformCoinWithTokensActivationOps + Clone,
    EnablePlatformCoinWithTokensError: From<Platform::ActivationError>,
{
    if let Ok(Some(coin)) = lp_coinfind_any(&ctx, &req.ticker).await {
        if !coin.is_available() {
            if let Some(platform_coin) = Platform::try_from_mm_coin(coin.inner) {
                return re_enable_passive_platform_coin_with_tokens(ctx, platform_coin, task_handle, req).await;
            }
        }

        return MmError::err(EnablePlatformCoinWithTokensError::PlatformIsAlreadyActivated(
            req.ticker,
        ));
    }

    let (platform_conf, platform_protocol) = coin_conf_with_protocol(&ctx, &req.ticker, None).map_mm_err()?;

    let platform_coin = Platform::enable_platform_coin(
        ctx.clone(),
        req.ticker.clone(),
        &platform_conf,
        req.request.clone(),
        platform_protocol,
    )
    .await
    .map_mm_err()?;

    let mut mm_tokens = Vec::new();
    for initializer in platform_coin.token_initializers() {
        let tokens = initializer
            .enable_tokens_as_mm_coins(&ctx, &req.request)
            .await
            .map_mm_err()?;
        mm_tokens.extend(tokens);
    }

    let nft_global = platform_coin.enable_global_nft(&req.request).await.map_mm_err()?;

    let activation_result = platform_coin
        .get_activation_result(task_handle, &req.request, &nft_global)
        .await
        .map_mm_err()?;
    log::info!("{} current block {}", req.ticker, activation_result.current_block());

    if req.request.tx_history() {
        platform_coin.start_history_background_fetching(
            ctx.clone(),
            TxHistoryStorageBuilder::new(&ctx).build().map_mm_err()?,
            activation_result.get_platform_balance(),
        );
    }

    let coins_ctx = CoinsContext::from_ctx(&ctx).unwrap();
    coins_ctx
        .add_platform_with_tokens(platform_coin.into(), mm_tokens, nft_global)
        .await
        .mm_err(|e| EnablePlatformCoinWithTokensError::PlatformIsAlreadyActivated(e.ticker))?;

    Ok(activation_result)
}

/// A struct that contains the info needed by the task that initializes a platform coin with tokens.
pub struct InitPlatformCoinWithTokensTask<Platform: PlatformCoinWithTokensActivationOps> {
    ctx: MmArc,
    request: EnablePlatformCoinWithTokensReq<Platform::ActivationRequest>,
}

impl<Platform: PlatformCoinWithTokensActivationOps> RpcTaskTypes for InitPlatformCoinWithTokensTask<Platform> {
    type Item = Platform::ActivationResult;
    // Using real type here because enable_platform_coin_with_tokens_impl fn, which implements RpcTask::run common logic, creates such errors
    type Error = EnablePlatformCoinWithTokensError;
    type InProgressStatus = Platform::InProgressStatus;
    type AwaitingStatus = Platform::AwaitingStatus;
    type UserAction = Platform::UserAction;
}

#[async_trait]
impl<Platform> RpcTask for InitPlatformCoinWithTokensTask<Platform>
where
    Platform: PlatformCoinWithTokensActivationOps + Clone,
    EnablePlatformCoinWithTokensError: From<<Platform as PlatformCoinWithTokensActivationOps>::ActivationError>,
{
    fn initial_status(&self) -> Self::InProgressStatus {
        <Platform::InProgressStatus as InitPlatformCoinWithTokensInitialStatus>::initial_status()
    }

    /// Try to disable the coin in case if we managed to register it already.
    async fn cancel(self) {}

    async fn run(&mut self, task_handle: RpcTaskHandleShared<Self>) -> Result<Self::Item, MmError<Self::Error>> {
        enable_platform_coin_with_tokens_impl::<Platform>(self.ctx.clone(), Some(task_handle), self.request.clone())
            .await
    }
}

/// Trait for the initial status of the task that initializes a platform coin with tokens.
pub trait InitPlatformCoinWithTokensInitialStatus {
    fn initial_status() -> Self;
}

/// The status of the task that initializes a platform coin with tokens.
#[derive(Clone, Serialize)]
pub enum InitPlatformCoinWithTokensInProgressStatus {
    ActivatingCoin,
    SyncingBlockHeaders {
        current_scanned_block: u64,
        last_block: u64,
    },
    TemporaryError(String),
    RequestingWalletBalance,
    Finishing,
    /// This status doesn't require the user to send `UserAction`,
    /// but it tells the user that he should confirm/decline an address on his device.
    WaitingForTrezorToConnect,
    FollowHwDeviceInstructions,
}

impl InitPlatformCoinWithTokensInitialStatus for InitPlatformCoinWithTokensInProgressStatus {
    fn initial_status() -> Self {
        InitPlatformCoinWithTokensInProgressStatus::ActivatingCoin
    }
}

/// Implementation of the init platform coin with tokens RPC command.
pub async fn init_platform_coin_with_tokens<Platform>(
    ctx: MmArc,
    request: RpcInitReq<EnablePlatformCoinWithTokensReq<Platform::ActivationRequest>>,
) -> MmResult<EnablePlatformCoinWithTokensResponse, EnablePlatformCoinWithTokensError>
where
    Platform: PlatformCoinWithTokensActivationOps + Send + Sync + 'static + Clone,
    Platform::InProgressStatus: InitPlatformCoinWithTokensInitialStatus,
    EnablePlatformCoinWithTokensError: From<Platform::ActivationError>,
{
    let (client_id, request) = (request.client_id, request.inner);
    if let Ok(Some(_)) = lp_coinfind(&ctx, &request.ticker).await {
        return MmError::err(EnablePlatformCoinWithTokensError::PlatformIsAlreadyActivated(
            request.ticker,
        ));
    }

    let coins_act_ctx =
        CoinsActivationContext::from_ctx(&ctx).map_to_mm(EnablePlatformCoinWithTokensError::Internal)?;
    let spawner = ctx.spawner();
    let task = InitPlatformCoinWithTokensTask::<Platform> { ctx, request };
    let task_manager = Platform::rpc_task_manager(&coins_act_ctx);

    let task_id = RpcTaskManager::spawn_rpc_task(task_manager, &spawner, task, client_id)
        .mm_err(|e| EnablePlatformCoinWithTokensError::Internal(e.to_string()))?;

    Ok(EnablePlatformCoinWithTokensResponse { task_id })
}

/// Implementation of the init platform coin with tokens status RPC command.
pub async fn init_platform_coin_with_tokens_status<Platform: PlatformCoinWithTokensActivationOps>(
    ctx: MmArc,
    req: EnablePlatformCoinWithTokensStatusRequest,
) -> MmResult<
    RpcTaskStatus<
        Platform::ActivationResult,
        EnablePlatformCoinWithTokensError,
        Platform::InProgressStatus,
        Platform::AwaitingStatus,
    >,
    InitPlatformCoinWithTokensStatusError,
>
where
    EnablePlatformCoinWithTokensError: From<Platform::ActivationError>,
{
    let coins_act_ctx =
        CoinsActivationContext::from_ctx(&ctx).map_to_mm(InitPlatformCoinWithTokensStatusError::Internal)?;
    let mut task_manager = Platform::rpc_task_manager(&coins_act_ctx)
        .lock()
        .map_to_mm(|poison| InitPlatformCoinWithTokensStatusError::Internal(poison.to_string()))?;
    task_manager
        .task_status(req.task_id, req.forget_if_finished)
        .or_mm_err(|| InitPlatformCoinWithTokensStatusError::NoSuchTask(req.task_id))
        .map(|rpc_task| rpc_task.map_err(|e| e))
}

/// Implementation of the init platform coin with tokens user action RPC command.
pub async fn init_platform_coin_with_tokens_user_action<Platform: PlatformCoinWithTokensActivationOps>(
    ctx: MmArc,
    req: InitPlatformCoinWithTokensUserActionRequest<Platform::UserAction>,
) -> MmResult<SuccessResponse, InitPlatformCoinWithTokensUserActionError>
where
    EnablePlatformCoinWithTokensError: From<Platform::ActivationError>,
{
    let coins_act_ctx = CoinsActivationContext::from_ctx(&ctx)
        .map_to_mm(InitPlatformCoinWithTokensUserActionError::Internal)
        .map_mm_err()?;
    let mut task_manager = Platform::rpc_task_manager(&coins_act_ctx)
        .lock()
        .map_to_mm(|poison| InitPlatformCoinWithTokensUserActionError::Internal(poison.to_string()))
        .map_mm_err()?;
    task_manager.on_user_action(req.task_id, req.user_action).map_mm_err()?;
    Ok(SuccessResponse::new())
}

/// Implementation of the cancel init platform coin with tokens RPC command.
pub async fn cancel_init_platform_coin_with_tokens<Platform: PlatformCoinWithTokensActivationOps>(
    ctx: MmArc,
    req: CancelRpcTaskRequest,
) -> MmResult<SuccessResponse, CancelInitPlatformCoinWithTokensError>
where
    EnablePlatformCoinWithTokensError: From<Platform::ActivationError>,
{
    let coins_act_ctx = CoinsActivationContext::from_ctx(&ctx)
        .map_to_mm(CancelInitPlatformCoinWithTokensError::Internal)
        .map_mm_err()?;
    let mut task_manager = Platform::rpc_task_manager(&coins_act_ctx)
        .lock()
        .map_to_mm(|poison| CancelInitPlatformCoinWithTokensError::Internal(poison.to_string()))
        .map_mm_err()?;
    task_manager.cancel_task(req.task_id).map_mm_err()?;
    Ok(SuccessResponse::new())
}

pub(crate) fn platform_coin_xpub_extractor_rpc_statuses(
) -> HwConnectStatuses<InitPlatformCoinWithTokensInProgressStatus, InitPlatformCoinWithTokensAwaitingStatus> {
    HwConnectStatuses {
        on_connect: InitPlatformCoinWithTokensInProgressStatus::WaitingForTrezorToConnect,
        on_connected: InitPlatformCoinWithTokensInProgressStatus::ActivatingCoin,
        on_connection_failed: InitPlatformCoinWithTokensInProgressStatus::Finishing,
        on_button_request: InitPlatformCoinWithTokensInProgressStatus::FollowHwDeviceInstructions,
        on_pin_request: InitPlatformCoinWithTokensAwaitingStatus::EnterTrezorPin,
        on_passphrase_request: InitPlatformCoinWithTokensAwaitingStatus::EnterTrezorPassphrase,
        on_ready: InitPlatformCoinWithTokensInProgressStatus::ActivatingCoin,
    }
}

pub mod for_tests {
    use common::{executor::Timer, now_ms, wait_until_ms};
    use mm2_core::mm_ctx::MmArc;
    use mm2_err_handle::prelude::MmResult;
    use rpc_task::{RpcInitReq, RpcTaskStatus};

    use super::{
        init_platform_coin_with_tokens, init_platform_coin_with_tokens_status, EnablePlatformCoinWithTokensError,
        EnablePlatformCoinWithTokensReq, EnablePlatformCoinWithTokensStatusRequest,
        InitPlatformCoinWithTokensInitialStatus, PlatformCoinWithTokensActivationOps,
    };

    /// test helper to activate platform coin with waiting for the result
    pub async fn init_platform_coin_with_tokens_loop<Platform>(
        ctx: MmArc,
        request: EnablePlatformCoinWithTokensReq<Platform::ActivationRequest>,
    ) -> MmResult<Platform::ActivationResult, EnablePlatformCoinWithTokensError>
    where
        Platform: PlatformCoinWithTokensActivationOps + Clone + Send + Sync + 'static,
        Platform::InProgressStatus: InitPlatformCoinWithTokensInitialStatus,
        EnablePlatformCoinWithTokensError: From<Platform::ActivationError>,
    {
        let request = RpcInitReq {
            client_id: 0,
            inner: request,
        };
        let init_result = init_platform_coin_with_tokens::<Platform>(ctx.clone(), request)
            .await
            .unwrap();
        let timeout = wait_until_ms(150000);
        loop {
            if now_ms() > timeout {
                panic!("init_standalone_coin timed out");
            }
            let status_req = EnablePlatformCoinWithTokensStatusRequest {
                task_id: init_result.task_id,
                forget_if_finished: true,
            };
            let status_res = init_platform_coin_with_tokens_status::<Platform>(ctx.clone(), status_req).await;
            if let Ok(status) = status_res {
                match status {
                    RpcTaskStatus::Ok(result) => break Ok(result),
                    RpcTaskStatus::Error(e) => break Err(e),
                    _ => Timer::sleep(1.).await,
                }
            } else {
                panic!("could not get init_standalone_coin status");
            }
        }
    }
}
