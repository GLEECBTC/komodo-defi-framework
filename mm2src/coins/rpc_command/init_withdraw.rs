use crate::{lp_coinfind_or_err, CoinsContext, MmCoinEnum, WithdrawError};
use crate::{TransactionDetails, WithdrawRequest};
use async_trait::async_trait;
use common::SuccessResponse;
use crypto::hw_rpc_task::{HwRpcTaskAwaitingStatus, HwRpcTaskUserAction, HwRpcTaskUserActionRequest};
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use rpc_task::rpc_common::{
    CancelRpcTaskError, CancelRpcTaskRequest, InitRpcTaskResponse, RpcTaskStatusError, RpcTaskStatusRequest,
    RpcTaskUserActionError,
};
use rpc_task::{
    RpcInitReq, RpcTask, RpcTaskHandleShared, RpcTaskManager, RpcTaskManagerShared, RpcTaskStatusAlias, RpcTaskTypes,
};

pub type WithdrawAwaitingStatus = HwRpcTaskAwaitingStatus;
pub type WithdrawUserAction = HwRpcTaskUserAction;
pub type WithdrawStatusError = RpcTaskStatusError;
pub type WithdrawUserActionError = RpcTaskUserActionError;
pub type InitWithdrawResponse = InitRpcTaskResponse;
pub type WithdrawStatusRequest = RpcTaskStatusRequest;
pub type WithdrawUserActionRequest = HwRpcTaskUserActionRequest;
pub type WithdrawTaskManager = RpcTaskManager<WithdrawTask>;
pub type WithdrawTaskManagerShared = RpcTaskManagerShared<WithdrawTask>;
pub type WithdrawTaskHandleShared = RpcTaskHandleShared<WithdrawTask>;
pub type WithdrawRpcStatus = RpcTaskStatusAlias<WithdrawTask>;
pub type WithdrawInitResult<T> = Result<T, MmError<WithdrawError>>;

#[async_trait]
pub trait CoinWithdrawInit {
    #[allow(clippy::result_large_err)]
    fn init_withdraw(
        ctx: MmArc,
        req: WithdrawRequest,
        rpc_task_handle: WithdrawTaskHandleShared,
    ) -> WithdrawInitResult<TransactionDetails>;
}

pub async fn init_withdraw(
    ctx: MmArc,
    request: RpcInitReq<WithdrawRequest>,
) -> WithdrawInitResult<InitWithdrawResponse> {
    let (client_id, request) = (request.client_id, request.inner);
    let coin = lp_coinfind_or_err(&ctx, &request.coin).await.map_mm_err()?;
    let spawner = coin.spawner();
    let task = WithdrawTask {
        ctx: ctx.clone(),
        coin,
        request,
    };
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(WithdrawError::InternalError)?;
    let task_id = WithdrawTaskManager::spawn_rpc_task(&coins_ctx.withdraw_task_manager, &spawner, task, client_id)
        .map_mm_err()?;
    Ok(InitWithdrawResponse { task_id })
}

pub async fn withdraw_status(
    ctx: MmArc,
    req: WithdrawStatusRequest,
) -> Result<WithdrawRpcStatus, MmError<WithdrawStatusError>> {
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(WithdrawStatusError::Internal)?;
    let mut task_manager = coins_ctx
        .withdraw_task_manager
        .lock()
        .map_to_mm(|e| WithdrawStatusError::Internal(e.to_string()))?;
    task_manager
        .task_status(req.task_id, req.forget_if_finished)
        .or_mm_err(|| WithdrawStatusError::NoSuchTask(req.task_id))
}

#[derive(Clone, Serialize)]
pub enum WithdrawInProgressStatus {
    Preparing,
    GeneratingTransaction,
    SigningTransaction,
    Finishing,
    /// The following statuses don't require the user to send `UserAction`,
    /// but they tell the user that he should confirm/decline the operation on his device.
    WaitingForTrezorToConnect,
    FollowHwDeviceInstructions,
    WaitingForUserToConfirmSigning,
}

pub async fn withdraw_user_action(
    ctx: MmArc,
    req: WithdrawUserActionRequest,
) -> Result<SuccessResponse, MmError<WithdrawUserActionError>> {
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(WithdrawUserActionError::Internal)?;
    let mut task_manager = coins_ctx
        .withdraw_task_manager
        .lock()
        .map_to_mm(|e| WithdrawUserActionError::Internal(e.to_string()))?;
    task_manager.on_user_action(req.task_id, req.user_action).map_mm_err()?;
    Ok(SuccessResponse::new())
}

pub async fn cancel_withdraw(ctx: MmArc, req: CancelRpcTaskRequest) -> MmResult<SuccessResponse, CancelRpcTaskError> {
    let coins_ctx = CoinsContext::from_ctx(&ctx).map_to_mm(CancelRpcTaskError::Internal)?;
    let mut task_manager = coins_ctx
        .withdraw_task_manager
        .lock()
        .map_to_mm(|e| CancelRpcTaskError::Internal(e.to_string()))?;
    task_manager.cancel_task(req.task_id).map_mm_err()?;
    Ok(SuccessResponse::new())
}

#[async_trait]
pub trait InitWithdrawCoin {
    async fn init_withdraw(
        &self,
        ctx: MmArc,
        req: WithdrawRequest,
        task_handle: WithdrawTaskHandleShared,
    ) -> Result<TransactionDetails, MmError<WithdrawError>>;
}

pub struct WithdrawTask {
    ctx: MmArc,
    coin: MmCoinEnum,
    request: WithdrawRequest,
}

impl RpcTaskTypes for WithdrawTask {
    type Item = TransactionDetails;
    type Error = WithdrawError;
    type InProgressStatus = WithdrawInProgressStatus;
    type AwaitingStatus = WithdrawAwaitingStatus;
    type UserAction = WithdrawUserAction;
}

#[async_trait]
impl RpcTask for WithdrawTask {
    fn initial_status(&self) -> Self::InProgressStatus {
        WithdrawInProgressStatus::Preparing
    }

    // Do nothing if the task has been cancelled.
    async fn cancel(self) {}

    async fn run(&mut self, task_handle: WithdrawTaskHandleShared) -> Result<Self::Item, MmError<Self::Error>> {
        let ctx = self.ctx.clone();
        let request = self.request.clone();
        match self.coin {
            MmCoinEnum::UtxoCoin(ref standard_utxo) => standard_utxo.init_withdraw(ctx, request, task_handle).await,
            MmCoinEnum::QtumCoin(ref qtum) => qtum.init_withdraw(ctx, request, task_handle).await,
            MmCoinEnum::ZCoin(ref z) => z.init_withdraw(ctx, request, task_handle).await,
            MmCoinEnum::EthCoin(ref eth) => eth.init_withdraw(ctx, request, task_handle).await,
            _ => MmError::err(WithdrawError::CoinDoesntSupportInitWithdraw {
                coin: self.coin.ticker().to_owned(),
            }),
        }
    }
}
