use crate::hw_client::{HwProcessingError, TrezorConnectProcessor};
use crate::trezor::TrezorPinMatrix3x3Response;
use async_trait::async_trait;
use mm2_err_handle::prelude::*;
use rpc_task::rpc_common::RpcTaskUserActionRequest;
use serde::Serialize;
use std::convert::TryFrom;
use std::sync::Arc;
use std::time::Duration;
use trezor::trezor_rpc_task::{
    RpcTask, RpcTaskError, RpcTaskHandleShared, TrezorRequestStatuses, TrezorRpcTaskProcessor, TryIntoUserAction,
};
use trezor::user_interaction::TrezorPassphraseResponse;
use trezor::{TrezorProcessingError, TrezorRequestProcessor};

const CONNECT_DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

pub type HwRpcTaskUserActionRequest = RpcTaskUserActionRequest<HwRpcTaskUserAction>;

/// When it comes to interacting with a HW device, this is a common awaiting RPC status.
/// The status says to the user that he should pass a Trezor PIN to continue the pending RPC task.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum HwRpcTaskAwaitingStatus {
    EnterTrezorPin,
    EnterTrezorPassphrase,
}

/// When it comes to interacting with a HW device, this is a common user action in answer to awaiting RPC task status.
#[derive(Deserialize, Serialize)]
#[serde(tag = "action_type")]
pub enum HwRpcTaskUserAction {
    TrezorPin(TrezorPinMatrix3x3Response),
    TrezorPassphrase(TrezorPassphraseResponse),
}

impl TryFrom<HwRpcTaskUserAction> for TrezorPinMatrix3x3Response {
    type Error = RpcTaskError;

    fn try_from(value: HwRpcTaskUserAction) -> Result<Self, Self::Error> {
        match value {
            HwRpcTaskUserAction::TrezorPin(pin) => Ok(pin),
            HwRpcTaskUserAction::TrezorPassphrase(_) => Err(RpcTaskError::UnexpectedUserAction {
                expected: "TrezorPin".to_string(),
            }),
        }
    }
}

impl TryFrom<HwRpcTaskUserAction> for TrezorPassphraseResponse {
    type Error = RpcTaskError;

    fn try_from(value: HwRpcTaskUserAction) -> Result<Self, Self::Error> {
        match value {
            HwRpcTaskUserAction::TrezorPin(_) => Err(RpcTaskError::UnexpectedUserAction {
                expected: "TrezorPassphrase".to_string(),
            }),
            HwRpcTaskUserAction::TrezorPassphrase(passphrase) => Ok(passphrase),
        }
    }
}

#[derive(Clone)]
pub struct HwConnectStatuses<InProgressStatus, AwaitingStatus> {
    pub on_connect: InProgressStatus,
    pub on_connected: InProgressStatus,
    pub on_connection_failed: InProgressStatus,
    pub on_button_request: InProgressStatus,
    pub on_pin_request: AwaitingStatus,
    pub on_passphrase_request: AwaitingStatus,
    pub on_ready: InProgressStatus,
}

impl<InProgressStatus, AwaitingStatus> HwConnectStatuses<InProgressStatus, AwaitingStatus>
where
    InProgressStatus: Clone,
    AwaitingStatus: Clone,
{
    pub fn to_trezor_request_statuses(&self) -> TrezorRequestStatuses<InProgressStatus, AwaitingStatus> {
        TrezorRequestStatuses {
            on_button_request: self.on_button_request.clone(),
            on_pin_request: self.on_pin_request.clone(),
            on_passphrase_request: self.on_passphrase_request.clone(),
            on_ready: self.on_ready.clone(),
        }
    }
}

pub struct TrezorRpcTaskConnectProcessor<Task: RpcTask> {
    request_processor: TrezorRpcTaskProcessor<Task>,
    on_connect: Task::InProgressStatus,
    on_connected: Task::InProgressStatus,
    on_connection_failed: Task::InProgressStatus,
    connect_timeout: Duration,
}

#[async_trait]
impl<Task> TrezorRequestProcessor for TrezorRpcTaskConnectProcessor<Task>
where
    Task: RpcTask,
    Task::UserAction: TryIntoUserAction + Send,
{
    type Error = RpcTaskError;

    async fn on_button_request(&self) -> MmResult<(), TrezorProcessingError<Self::Error>> {
        self.request_processor.on_button_request().await
    }

    async fn on_pin_request(&self) -> MmResult<TrezorPinMatrix3x3Response, TrezorProcessingError<Self::Error>> {
        self.request_processor.on_pin_request().await
    }

    async fn on_passphrase_request(&self) -> MmResult<TrezorPassphraseResponse, TrezorProcessingError<Self::Error>> {
        self.request_processor.on_passphrase_request().await
    }

    async fn on_ready(&self) -> MmResult<(), TrezorProcessingError<Self::Error>> {
        self.request_processor.on_ready().await
    }
}

#[async_trait]
impl<Task> TrezorConnectProcessor for TrezorRpcTaskConnectProcessor<Task>
where
    Task: RpcTask,
    Task::UserAction: TryIntoUserAction,
{
    async fn on_connect(&self) -> MmResult<Duration, HwProcessingError<RpcTaskError>> {
        self.request_processor
            .update_in_progress_status(self.on_connect.clone())
            .map_mm_err()?;
        Ok(self.connect_timeout)
    }

    async fn on_connected(&self) -> MmResult<(), HwProcessingError<RpcTaskError>> {
        Ok(self
            .request_processor
            .update_in_progress_status(self.on_connected.clone())
            .map_mm_err()?)
    }

    async fn on_connection_failed(&self) -> MmResult<(), HwProcessingError<RpcTaskError>> {
        Ok(self
            .request_processor
            .update_in_progress_status(self.on_connection_failed.clone())
            .map_mm_err()?)
    }

    fn as_base_shared(&self) -> Arc<dyn TrezorRequestProcessor<Error = RpcTaskError>> {
        Arc::new(self.request_processor.clone())
    }
}

impl<Task: RpcTask> TrezorRpcTaskConnectProcessor<Task> {
    pub fn new(
        task_handle: RpcTaskHandleShared<Task>,
        statuses: HwConnectStatuses<Task::InProgressStatus, Task::AwaitingStatus>,
    ) -> Self {
        let request_statuses = TrezorRequestStatuses {
            on_button_request: statuses.on_button_request,
            on_pin_request: statuses.on_pin_request,
            on_passphrase_request: statuses.on_passphrase_request,
            on_ready: statuses.on_ready,
        };
        let request_processor = TrezorRpcTaskProcessor::new(task_handle, request_statuses);
        TrezorRpcTaskConnectProcessor {
            request_processor,
            on_connect: statuses.on_connect,
            on_connected: statuses.on_connected,
            on_connection_failed: statuses.on_connection_failed,
            connect_timeout: CONNECT_DEFAULT_TIMEOUT,
        }
    }

    pub fn with_pin_timeout(mut self, pin_timeout: Duration) -> Self {
        self.request_processor = self.request_processor.with_user_action_timeout(pin_timeout);
        self
    }

    pub fn with_connect_timeout(mut self, connect_timeout: Duration) -> Self {
        self.connect_timeout = connect_timeout;
        self
    }
}
