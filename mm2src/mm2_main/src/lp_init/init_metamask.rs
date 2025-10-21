use crate::lp_native_dex::init_context::MmInitContext;
use async_trait::async_trait;
use common::{HttpStatusCode, SerdeInfallible, SuccessResponse};
use crypto::metamask::{from_metamask_error, MetamaskError, MetamaskRpcError, WithMetamaskRpcError};
use crypto::{CryptoCtx, CryptoCtxError, MetamaskCtxInitError};
use derive_more::Display;
use enum_derives::EnumFromTrait;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::common_errors::WithInternal;
use mm2_err_handle::prelude::*;
use rpc_task::rpc_common::{CancelRpcTaskError, CancelRpcTaskRequest, InitRpcTaskResponse, RpcTaskStatusError,
                           RpcTaskStatusRequest};
use rpc_task::{RpcInitReq, RpcTask, RpcTaskError, RpcTaskHandleShared, RpcTaskManager, RpcTaskManagerShared,
               RpcTaskStatus, RpcTaskTypes};
use std::sync::Arc;
use std::time::Duration;

pub type InitMetamaskManagerShared = RpcTaskManagerShared<InitMetamaskTask>;
pub type InitMetamaskStatus =
    RpcTaskStatus<InitMetamaskResponse, InitMetamaskError, InitMetamaskInProgressStatus, InitMetamaskAwaitingStatus>;

type InitMetamaskUserAction = SerdeInfallible;
type InitMetamaskAwaitingStatus = SerdeInfallible;
type InitMetamaskTaskHandleShared = RpcTaskHandleShared<InitMetamaskTask>;

#[derive(Clone, Display, EnumFromTrait, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum InitMetamaskError {
    #[display(fmt = "MetaMask is initializing already")]
    MetamaskInitializingAlready,
    #[from_trait(WithMetamaskRpcError::metamask_rpc_error)]
    MetamaskError(MetamaskRpcError),
    #[display(fmt = "RPC timed out {_0:?}")]
    Timeout(Duration),
    #[display(fmt = "Internal: {_0}")]
    #[from_trait(WithInternal::internal)]
    Internal(String),
}

impl From<CryptoCtxError> for InitMetamaskError {
    fn from(value: CryptoCtxError) -> Self { InitMetamaskError::Internal(value.to_string()) }
}

impl From<MetamaskCtxInitError> for InitMetamaskError {
    fn from(value: MetamaskCtxInitError) -> Self {
        match value {
            MetamaskCtxInitError::InitializingAlready => InitMetamaskError::MetamaskInitializingAlready,
            MetamaskCtxInitError::MetamaskError(metamask) => InitMetamaskError::from(metamask),
        }
    }
}

impl From<MetamaskError> for InitMetamaskError {
    fn from(metamask: MetamaskError) -> Self { from_metamask_error(metamask) }
}

impl From<RpcTaskError> for InitMetamaskError {
    fn from(e: RpcTaskError) -> Self {
        let error = e.to_string();
        match e {
            RpcTaskError::Cancelled => InitMetamaskError::Internal("Cancelled".to_owned()),
            RpcTaskError::Timeout(timeout) => InitMetamaskError::Timeout(timeout),
            RpcTaskError::NoSuchTask(_) | RpcTaskError::UnexpectedTaskStatus { .. } => {
                InitMetamaskError::Internal(error)
            },
            RpcTaskError::UnexpectedUserAction { .. } => {
                InitMetamaskError::Internal("Unexpected user action".to_string())
            },
            RpcTaskError::Internal(internal) => InitMetamaskError::Internal(internal),
        }
    }
}

impl HttpStatusCode for InitMetamaskError {
    fn status_code(&self) -> StatusCode {
        match self {
            InitMetamaskError::MetamaskInitializingAlready => StatusCode::BAD_REQUEST,
            InitMetamaskError::Timeout(_) => StatusCode::REQUEST_TIMEOUT,
            InitMetamaskError::MetamaskError(_) | InitMetamaskError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub enum InitMetamaskInProgressStatus {
    Initializing,
    SigningLoginMetadata,
}

#[derive(Deserialize)]
pub struct InitMetamaskRequest {
    project: String,
}

#[derive(Clone, Serialize)]
pub struct InitMetamaskResponse {
    eth_address: String,
}

pub struct InitMetamaskTask {
    ctx: MmArc,
    req: InitMetamaskRequest,
}

impl RpcTaskTypes for InitMetamaskTask {
    type Item = InitMetamaskResponse;
    type Error = InitMetamaskError;
    type InProgressStatus = InitMetamaskInProgressStatus;
    type AwaitingStatus = SerdeInfallible;
    type UserAction = SerdeInfallible;
}

#[async_trait]
impl RpcTask for InitMetamaskTask {
    fn initial_status(&self) -> Self::InProgressStatus { InitMetamaskInProgressStatus::Initializing }

    async fn cancel(self) {
        if let Ok(crypto_ctx) = CryptoCtx::from_ctx(&self.ctx) {
            crypto_ctx.reset_metamask_ctx();
        }
    }

    async fn run(&mut self, _task_handle: InitMetamaskTaskHandleShared) -> Result<Self::Item, MmError<Self::Error>> {
        let crypto_ctx = CryptoCtx::from_ctx(&self.ctx).map_mm_err()?;

        let metamask = crypto_ctx.init_metamask_ctx(self.req.project.clone()).await.map_mm_err()?;
        Ok(InitMetamaskResponse {
            eth_address: metamask.eth_account_str().to_string(),
        })
    }
}

pub async fn connect_metamask(
    ctx: MmArc,
    req: RpcInitReq<InitMetamaskRequest>,
) -> MmResult<InitRpcTaskResponse, InitMetamaskError> {
    let (client_id, req) = (req.client_id, req.inner);
    let init_ctx = MmInitContext::from_ctx(&ctx).map_to_mm(InitMetamaskError::Internal)?;
    let spawner = ctx.spawner();
    let task = InitMetamaskTask { ctx, req };
    let task_id = RpcTaskManager::spawn_rpc_task(&init_ctx.init_metamask_manager, &spawner, task, client_id).map_mm_err()?;
    Ok(InitRpcTaskResponse { task_id })
}

pub async fn connect_metamask_status(
    ctx: MmArc,
    req: RpcTaskStatusRequest,
) -> MmResult<InitMetamaskStatus, RpcTaskStatusError> {
    let init_ctx = MmInitContext::from_ctx(&ctx).map_to_mm(RpcTaskStatusError::Internal)?;
    let mut task_manager = init_ctx
        .init_metamask_manager
        .lock()
        .map_to_mm(|e| RpcTaskStatusError::Internal(e.to_string()))?;
    task_manager
        .task_status(req.task_id, req.forget_if_finished)
        .or_mm_err(|| RpcTaskStatusError::NoSuchTask(req.task_id))
}

pub async fn cancel_connect_metamask(
    ctx: MmArc,
    req: CancelRpcTaskRequest,
) -> MmResult<SuccessResponse, CancelRpcTaskError> {
    let init_ctx = MmInitContext::from_ctx(&ctx).map_to_mm(CancelRpcTaskError::Internal)?;
    let mut task_manager = init_ctx
        .init_metamask_manager
        .lock()
        .map_to_mm(|e| CancelRpcTaskError::Internal(e.to_string()))?;
    task_manager.cancel_task(req.task_id).map_mm_err()?;
    Ok(SuccessResponse::new())
}
