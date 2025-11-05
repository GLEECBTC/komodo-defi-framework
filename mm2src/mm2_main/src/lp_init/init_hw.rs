use crate::lp_native_dex::init_context::MmInitContext;
use async_trait::async_trait;
use common::{HttpStatusCode, SuccessResponse};
use crypto::hw_rpc_task::{
    HwConnectStatuses, HwRpcTaskAwaitingStatus, HwRpcTaskUserAction, HwRpcTaskUserActionRequest,
    TrezorRpcTaskConnectProcessor,
};
use crypto::{
    from_hw_error, CryptoCtx, CryptoCtxError, HwCtxInitError, HwDeviceInfo, HwError, HwPubkey, HwRpcError,
    HwWalletType, WithHwRpcError,
};
use derive_more::Display;
use enum_derives::EnumFromTrait;
use http::StatusCode;
use mm2_core::mm_ctx::MmArc;
use mm2_err_handle::prelude::*;
use rpc_task::rpc_common::{
    CancelRpcTaskError, CancelRpcTaskRequest, InitRpcTaskResponse, RpcTaskStatusError, RpcTaskStatusRequest,
    RpcTaskUserActionError,
};
use rpc_task::{
    RpcInitReq, RpcTask, RpcTaskError, RpcTaskHandleShared, RpcTaskManager, RpcTaskManagerShared, RpcTaskStatus,
    RpcTaskTypes,
};
use std::sync::Arc;
use std::time::Duration;

const TREZOR_CONNECT_TIMEOUT: Duration = Duration::from_secs(300);
const TREZOR_PIN_TIMEOUT: Duration = Duration::from_secs(600);

pub type InitHwAwaitingStatus = HwRpcTaskAwaitingStatus;
pub type InitHwUserAction = HwRpcTaskUserAction;

pub type InitHwTaskManagerShared = RpcTaskManagerShared<InitHwTask>;
pub type InitHwStatus = RpcTaskStatus<InitHwResponse, InitHwError, InitHwInProgressStatus, InitHwAwaitingStatus>;
type InitHwTaskHandleShared = RpcTaskHandleShared<InitHwTask>;

#[derive(Clone, Display, EnumFromTrait, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum InitHwError {
    #[display(fmt = "Hardware Wallet context is initializing already")]
    HwContextInitializingAlready,
    #[from_trait(WithHwRpcError::hw_rpc_error)]
    #[display(fmt = "{_0}")]
    HwError(HwRpcError),
    #[display(fmt = "RPC 'task' is awaiting '{expected}' user action")]
    UnexpectedUserAction { expected: String },
    #[from_trait(WithTimeout::timeout)]
    #[display(fmt = "RPC timed out {_0:?}")]
    Timeout(Duration),
    #[from_trait(WithInternal::internal)]
    #[display(fmt = "Internal: {_0}")]
    Internal(String),
}

impl From<HwError> for InitHwError {
    fn from(hw_error: HwError) -> Self {
        from_hw_error(hw_error)
    }
}

impl From<CryptoCtxError> for InitHwError {
    fn from(e: CryptoCtxError) -> Self {
        InitHwError::Internal(e.to_string())
    }
}

impl From<HwCtxInitError<RpcTaskError>> for InitHwError {
    fn from(e: HwCtxInitError<RpcTaskError>) -> Self {
        match e {
            HwCtxInitError::InitializingAlready => InitHwError::HwContextInitializingAlready,
            HwCtxInitError::UnexpectedPubkey { .. } => InitHwError::HwError(HwRpcError::FoundUnexpectedDevice),
            HwCtxInitError::HwError(hw_error) => InitHwError::from(hw_error),
            HwCtxInitError::ProcessorError(rpc) => InitHwError::from(rpc),
            HwCtxInitError::InternalError(err) => InitHwError::Internal(err),
        }
    }
}

impl From<RpcTaskError> for InitHwError {
    fn from(e: RpcTaskError) -> Self {
        let error = e.to_string();
        match e {
            RpcTaskError::Cancelled => InitHwError::Internal("Cancelled".to_owned()),
            RpcTaskError::Timeout(timeout) => InitHwError::Timeout(timeout),
            RpcTaskError::NoSuchTask(_) | RpcTaskError::UnexpectedTaskStatus { .. } => InitHwError::Internal(error),
            RpcTaskError::UnexpectedUserAction { expected } => InitHwError::UnexpectedUserAction { expected },
            RpcTaskError::Internal(internal) => InitHwError::Internal(internal),
        }
    }
}

impl HttpStatusCode for InitHwError {
    fn status_code(&self) -> StatusCode {
        match self {
            InitHwError::HwContextInitializingAlready | InitHwError::UnexpectedUserAction { .. } => {
                StatusCode::BAD_REQUEST
            },
            InitHwError::HwError(_) => StatusCode::GONE,
            InitHwError::Timeout(_) => StatusCode::REQUEST_TIMEOUT,
            InitHwError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub enum InitHwInProgressStatus {
    Initializing,
    WaitingForTrezorToConnect,
    FollowHwDeviceInstructions,
}

#[derive(Default, Deserialize, Clone)]
pub struct InitHwRequest {
    device_pubkey: Option<HwPubkey>,
}

#[derive(Clone, Serialize, Debug, Deserialize)]
pub struct InitHwResponse {
    #[serde(flatten)]
    device_info: HwDeviceInfo,
    device_pubkey: HwPubkey,
}

#[derive(Clone)]
pub struct InitHwTask {
    ctx: MmArc,
    hw_wallet_type: HwWalletType,
    req: InitHwRequest,
}

impl RpcTaskTypes for InitHwTask {
    type Item = InitHwResponse;
    type Error = InitHwError;
    type InProgressStatus = InitHwInProgressStatus;
    type AwaitingStatus = InitHwAwaitingStatus;
    type UserAction = InitHwUserAction;
}

#[async_trait]
impl RpcTask for InitHwTask {
    fn initial_status(&self) -> Self::InProgressStatus {
        InitHwInProgressStatus::Initializing
    }

    async fn cancel(self) {
        if let Ok(crypto_ctx) = CryptoCtx::from_ctx(&self.ctx) {
            crypto_ctx.reset_hw_ctx()
        }
    }

    async fn run(&mut self, task_handle: InitHwTaskHandleShared) -> Result<Self::Item, MmError<Self::Error>> {
        let crypto_ctx = CryptoCtx::from_ctx(&self.ctx).map_mm_err()?;

        match self.hw_wallet_type {
            HwWalletType::Trezor => {
                let trezor_connect_processor = TrezorRpcTaskConnectProcessor::new(
                    task_handle,
                    HwConnectStatuses {
                        on_connect: InitHwInProgressStatus::WaitingForTrezorToConnect,
                        on_connected: InitHwInProgressStatus::Initializing,
                        on_connection_failed: InitHwInProgressStatus::Initializing,
                        on_button_request: InitHwInProgressStatus::FollowHwDeviceInstructions,
                        on_pin_request: InitHwAwaitingStatus::EnterTrezorPin,
                        on_passphrase_request: InitHwAwaitingStatus::EnterTrezorPassphrase,
                        on_ready: InitHwInProgressStatus::Initializing,
                    },
                )
                .with_connect_timeout(TREZOR_CONNECT_TIMEOUT)
                .with_pin_timeout(TREZOR_PIN_TIMEOUT);
                let trezor_connect_processor = Arc::new(trezor_connect_processor);
                let (device_info, hw_ctx) = crypto_ctx
                    .init_hw_ctx_with_trezor(trezor_connect_processor, self.req.device_pubkey)
                    .await
                    .map_mm_err()?;
                let device_pubkey = hw_ctx.hw_pubkey();
                Ok(InitHwResponse {
                    device_info,
                    device_pubkey,
                })
            },
        }
    }
}

pub async fn init_trezor(
    ctx: MmArc,
    req: Option<RpcInitReq<InitHwRequest>>,
) -> MmResult<InitRpcTaskResponse, InitHwError> {
    let req = req.unwrap_or_default();

    let (client_id, req) = (req.client_id, req.inner);
    let init_ctx = MmInitContext::from_ctx(&ctx).map_to_mm(InitHwError::Internal)?;
    let spawner = ctx.spawner();
    let task = InitHwTask {
        ctx,
        hw_wallet_type: HwWalletType::Trezor,
        req,
    };
    let task_id =
        RpcTaskManager::spawn_rpc_task(&init_ctx.init_hw_task_manager, &spawner, task, client_id).map_mm_err()?;
    Ok(InitRpcTaskResponse { task_id })
}

pub async fn init_trezor_status(ctx: MmArc, req: RpcTaskStatusRequest) -> MmResult<InitHwStatus, RpcTaskStatusError> {
    let coins_ctx = MmInitContext::from_ctx(&ctx).map_to_mm(RpcTaskStatusError::Internal)?;
    let mut task_manager = coins_ctx
        .init_hw_task_manager
        .lock()
        .map_to_mm(|e| RpcTaskStatusError::Internal(e.to_string()))?;
    task_manager
        .task_status(req.task_id, req.forget_if_finished)
        .or_mm_err(|| RpcTaskStatusError::NoSuchTask(req.task_id))
}

pub async fn init_trezor_user_action(
    ctx: MmArc,
    req: HwRpcTaskUserActionRequest,
) -> MmResult<SuccessResponse, RpcTaskUserActionError> {
    let coins_ctx = MmInitContext::from_ctx(&ctx).map_to_mm(RpcTaskUserActionError::Internal)?;
    let mut task_manager = coins_ctx
        .init_hw_task_manager
        .lock()
        .map_to_mm(|e| RpcTaskUserActionError::Internal(e.to_string()))?;
    task_manager.on_user_action(req.task_id, req.user_action).map_mm_err()?;
    Ok(SuccessResponse::new())
}

pub async fn cancel_init_trezor(
    ctx: MmArc,
    req: CancelRpcTaskRequest,
) -> MmResult<SuccessResponse, CancelRpcTaskError> {
    let coins_ctx = MmInitContext::from_ctx(&ctx).map_to_mm(CancelRpcTaskError::Internal)?;
    let mut task_manager = coins_ctx
        .init_hw_task_manager
        .lock()
        .map_to_mm(|e| CancelRpcTaskError::Internal(e.to_string()))?;
    task_manager.cancel_task(req.task_id).map_mm_err()?;
    Ok(SuccessResponse::new())
}
