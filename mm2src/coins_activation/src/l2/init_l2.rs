/// Contains L2 activation traits and their implementations for various coins
///
use crate::context::CoinsActivationContext;
use crate::l2::init_l2_error::{CancelInitL2Error, InitL2StatusError, InitL2UserActionError};
use crate::l2::InitL2Error;
use crate::prelude::*;
use async_trait::async_trait;
use coins::{lp_coinfind, lp_coinfind_or_err, CoinsContext, MmCoinEnum, RegisterCoinError};
use common::SuccessResponse;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use rpc_task::rpc_common::{CancelRpcTaskRequest, InitRpcTaskResponse, RpcTaskStatusRequest, RpcTaskUserActionRequest};
use rpc_task::{
    RpcInitReq, RpcTask, RpcTaskHandleShared, RpcTaskManager, RpcTaskManagerShared, RpcTaskStatus, RpcTaskTypes,
};
use serde_derive::Deserialize;
use serde_json::Value as Json;

pub type InitL2Response = InitRpcTaskResponse;
pub type InitL2StatusRequest = RpcTaskStatusRequest;
pub type InitL2UserActionRequest<UserAction> = RpcTaskUserActionRequest<UserAction>;
pub type InitL2TaskManagerShared<L2> = RpcTaskManagerShared<InitL2Task<L2>>;
pub type InitL2TaskHandleShared<L2> = RpcTaskHandleShared<InitL2Task<L2>>;

#[derive(Debug, Deserialize)]
pub struct InitL2Req<T> {
    ticker: String,
    activation_params: T,
}

pub trait L2ProtocolParams {
    fn platform_coin_ticker(&self) -> &str;
}

#[async_trait]
pub trait InitL2ActivationOps: Into<MmCoinEnum> + Send + Sync + 'static {
    type PlatformCoin: TryPlatformCoinFromMmCoinEnum + Clone + Send + Sync;
    type ActivationParams: Clone;
    type ProtocolInfo: L2ProtocolParams + TryFromCoinProtocol + Clone + Send + Sync;
    type ValidatedParams: Clone + Send + Sync;
    type CoinConf: Clone + Send + Sync;
    type ActivationResult: serde::Serialize + Clone + Send + Sync;
    type ActivationError: From<RegisterCoinError> + SerMmErrorType + Clone + Send + Sync;
    type InProgressStatus: InitL2InitialStatus + serde::Serialize + Clone + Send + Sync;
    type AwaitingStatus: serde::Serialize + Clone + Send + Sync;
    type UserAction: NotMmError + Send + Sync;

    fn rpc_task_manager(activation_ctx: &CoinsActivationContext) -> &InitL2TaskManagerShared<Self>;

    fn coin_conf_from_json(json: Json) -> Result<Self::CoinConf, MmError<Self::ActivationError>>;

    fn validate_platform_configuration(
        platform_coin: &Self::PlatformCoin,
    ) -> Result<(), MmError<Self::ActivationError>>;

    fn validate_activation_params(
        activation_params: Self::ActivationParams,
    ) -> Result<Self::ValidatedParams, MmError<Self::ActivationError>>;

    async fn init_l2(
        ctx: &MmArc,
        platform_coin: Self::PlatformCoin,
        validated_params: Self::ValidatedParams,
        protocol_conf: Self::ProtocolInfo,
        coin_conf: Self::CoinConf,
        task_handle: InitL2TaskHandleShared<Self>,
    ) -> Result<(Self, Self::ActivationResult), MmError<Self::ActivationError>>;
}

pub async fn init_l2<L2>(
    ctx: MmArc,
    req: RpcInitReq<InitL2Req<L2::ActivationParams>>,
) -> Result<InitL2Response, MmError<InitL2Error>>
where
    L2: InitL2ActivationOps,
    InitL2Error: From<L2::ActivationError>,
{
    let (client_id, req) = (req.client_id, req.inner);
    let ticker = req.ticker.clone();
    if let Ok(Some(_)) = lp_coinfind(&ctx, &ticker).await {
        return MmError::err(InitL2Error::L2IsAlreadyActivated(ticker));
    }

    let (coin_conf_json, protocol_conf): (Json, L2::ProtocolInfo) =
        coin_conf_with_protocol(&ctx, &ticker, None).map_mm_err()?;
    let coin_conf = L2::coin_conf_from_json(coin_conf_json).map_mm_err()?;

    let platform_coin = lp_coinfind_or_err(&ctx, protocol_conf.platform_coin_ticker())
        .await
        .mm_err(|_| InitL2Error::PlatformCoinIsNotActivated(ticker.clone()))?;

    let platform_coin =
        L2::PlatformCoin::try_from_mm_coin(platform_coin).or_mm_err(|| InitL2Error::UnsupportedPlatformCoin {
            platform_coin_ticker: protocol_conf.platform_coin_ticker().into(),
            l2_ticker: ticker.clone(),
        })?;

    L2::validate_platform_configuration(&platform_coin).map_mm_err()?;

    let validated_params = L2::validate_activation_params(req.activation_params.clone()).map_mm_err()?;

    let coins_act_ctx = CoinsActivationContext::from_ctx(&ctx)
        .map_to_mm(InitL2Error::Internal)
        .map_mm_err()?;
    let spawner = ctx.spawner();
    let task = InitL2Task::<L2> {
        ctx,
        ticker,
        platform_coin,
        validated_params,
        protocol_conf,
        coin_conf,
    };
    let task_manager = L2::rpc_task_manager(&coins_act_ctx);

    let task_id = RpcTaskManager::spawn_rpc_task(task_manager, &spawner, task, client_id)
        .mm_err(|e| InitL2Error::Internal(e.to_string()))?;

    Ok(InitL2Response { task_id })
}

pub async fn init_l2_status<L2: InitL2ActivationOps>(
    ctx: MmArc,
    req: InitL2StatusRequest,
) -> MmResult<
    RpcTaskStatus<L2::ActivationResult, InitL2Error, L2::InProgressStatus, L2::AwaitingStatus>,
    InitL2StatusError,
>
where
    InitL2Error: From<L2::ActivationError>,
{
    let coins_act_ctx = CoinsActivationContext::from_ctx(&ctx)
        .map_to_mm(InitL2StatusError::Internal)
        .map_mm_err()?;
    let mut task_manager = L2::rpc_task_manager(&coins_act_ctx)
        .lock()
        .map_to_mm(|poison| InitL2StatusError::Internal(poison.to_string()))?;
    task_manager
        .task_status(req.task_id, req.forget_if_finished)
        .or_mm_err(|| InitL2StatusError::NoSuchTask(req.task_id))
        .map(|rpc_task| rpc_task.map_err(InitL2Error::from))
}

pub async fn init_l2_user_action<L2: InitL2ActivationOps>(
    ctx: MmArc,
    req: InitL2UserActionRequest<L2::UserAction>,
) -> MmResult<SuccessResponse, InitL2UserActionError> {
    let coins_act_ctx = CoinsActivationContext::from_ctx(&ctx)
        .map_to_mm(InitL2UserActionError::Internal)
        .map_mm_err()?;
    let mut task_manager = L2::rpc_task_manager(&coins_act_ctx)
        .lock()
        .map_to_mm(|poison| InitL2UserActionError::Internal(poison.to_string()))?;
    task_manager.on_user_action(req.task_id, req.user_action).map_mm_err()?;
    Ok(SuccessResponse::new())
}

pub async fn cancel_init_l2<L2: InitL2ActivationOps>(
    ctx: MmArc,
    req: CancelRpcTaskRequest,
) -> MmResult<SuccessResponse, CancelInitL2Error> {
    let coins_act_ctx = CoinsActivationContext::from_ctx(&ctx)
        .map_to_mm(CancelInitL2Error::Internal)
        .map_mm_err()?;
    let mut task_manager = L2::rpc_task_manager(&coins_act_ctx)
        .lock()
        .map_to_mm(|poison| CancelInitL2Error::Internal(poison.to_string()))
        .map_mm_err()?;
    task_manager.cancel_task(req.task_id).map_mm_err()?;
    Ok(SuccessResponse::new())
}

pub struct InitL2Task<L2: InitL2ActivationOps> {
    ctx: MmArc,
    ticker: String,
    platform_coin: L2::PlatformCoin,
    validated_params: L2::ValidatedParams,
    protocol_conf: L2::ProtocolInfo,
    coin_conf: L2::CoinConf,
}

impl<L2: InitL2ActivationOps> RpcTaskTypes for InitL2Task<L2> {
    type Item = L2::ActivationResult;
    type Error = L2::ActivationError;
    type InProgressStatus = L2::InProgressStatus;
    type AwaitingStatus = L2::AwaitingStatus;
    type UserAction = L2::UserAction;
}

#[async_trait]
impl<L2> RpcTask for InitL2Task<L2>
where
    L2: InitL2ActivationOps,
{
    fn initial_status(&self) -> Self::InProgressStatus {
        <L2::InProgressStatus as InitL2InitialStatus>::initial_status()
    }

    /// Try to disable the coin in case if we managed to register it already.
    async fn cancel(self) {
        if let Ok(ctx) = CoinsContext::from_ctx(&self.ctx) {
            if let Ok(Some(t)) = lp_coinfind(&self.ctx, &self.ticker).await {
                ctx.remove_coin(t).await;
            };
        };
    }
    async fn run(&mut self, task_handle: RpcTaskHandleShared<Self>) -> Result<Self::Item, MmError<Self::Error>> {
        let (coin, result) = L2::init_l2(
            &self.ctx,
            self.platform_coin.clone(),
            self.validated_params.clone(),
            self.protocol_conf.clone(),
            self.coin_conf.clone(),
            task_handle,
        )
        .await?;

        let c_ctx = CoinsContext::from_ctx(&self.ctx)
            .map_to_mm(RegisterCoinError::Internal)
            .map_mm_err()?;
        c_ctx.add_l2(coin.into()).await.map_mm_err()?;

        Ok(result)
    }
}

pub trait InitL2InitialStatus {
    fn initial_status() -> Self;
}
