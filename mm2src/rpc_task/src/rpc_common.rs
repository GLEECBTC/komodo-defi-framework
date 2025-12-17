use super::{RpcTaskError, TaskId};
use common::{true_f, HttpStatusCode, StatusCode};
use derive_more::Display;

/// In most cases, the RPC task status request may fail with either [`RpcTaskStatusError::NoSuchTask`] or [`RpcTaskStatusError::Internal`].
/// Please do not add new error variants unless they are used in most cases.
#[derive(Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum RpcTaskStatusError {
    #[display(fmt = "No such task '{_0}'")]
    NoSuchTask(TaskId),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl HttpStatusCode for RpcTaskStatusError {
    fn status_code(&self) -> StatusCode {
        match self {
            RpcTaskStatusError::NoSuchTask(_) => StatusCode::BAD_REQUEST,
            RpcTaskStatusError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// In most cases, the RPC task action may fail with either [`RpcTaskStatusError::NoSuchTask`] or [`RpcTaskStatusError::Internal`].
/// Please do not add new error variants unless they are used in most cases.
#[derive(Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum RpcTaskUserActionError {
    #[display(fmt = "No such task '{_0}'")]
    NoSuchTask(TaskId),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl From<RpcTaskError> for RpcTaskUserActionError {
    fn from(rpc_err: RpcTaskError) -> Self {
        match rpc_err {
            RpcTaskError::NoSuchTask(task_id) => RpcTaskUserActionError::NoSuchTask(task_id),
            rpc_err => RpcTaskUserActionError::Internal(rpc_err.to_string()),
        }
    }
}

impl HttpStatusCode for RpcTaskUserActionError {
    fn status_code(&self) -> StatusCode {
        match self {
            RpcTaskUserActionError::NoSuchTask(_) => StatusCode::BAD_REQUEST,
            RpcTaskUserActionError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// In most cases, the RPC task status request may fail with the following errors.
/// Please do not add new error variants unless they are used in most cases.
#[derive(Display, Serialize, SerializeErrorType)]
#[serde(tag = "error_type", content = "error_data")]
pub enum CancelRpcTaskError {
    #[display(fmt = "No such task '{_0}'")]
    NoSuchTask(TaskId),
    #[display(fmt = "Task is finished already")]
    TaskFinished(TaskId),
    #[display(fmt = "Internal error: {_0}")]
    Internal(String),
}

impl From<RpcTaskError> for CancelRpcTaskError {
    fn from(e: RpcTaskError) -> Self {
        match e {
            RpcTaskError::NoSuchTask(task_id) => CancelRpcTaskError::NoSuchTask(task_id),
            RpcTaskError::UnexpectedTaskStatus {
                task_id,
                actual,
                expected,
                ..
            } if actual.is_finished() && !expected.is_finished() => CancelRpcTaskError::TaskFinished(task_id),
            rpc_err => CancelRpcTaskError::Internal(rpc_err.to_string()),
        }
    }
}

impl HttpStatusCode for CancelRpcTaskError {
    fn status_code(&self) -> StatusCode {
        match self {
            CancelRpcTaskError::NoSuchTask(_) => StatusCode::NOT_FOUND,
            CancelRpcTaskError::TaskFinished(_) => StatusCode::CONFLICT,
            CancelRpcTaskError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// In most cases, the RPC task status request consists of `task_id` and `forget_if_finished` fields only.
/// Please do not add new fields unless they are used in most cases.
#[derive(Deserialize)]
pub struct RpcTaskStatusRequest {
    pub task_id: TaskId,
    #[serde(default = "true_f")]
    pub forget_if_finished: bool,
}

/// Please do not add new fields unless they are used in most cases.
#[derive(Deserialize)]
pub struct RpcTaskUserActionRequest<UserAction> {
    pub task_id: TaskId,
    pub user_action: UserAction,
}

/// In most cases, the RPC task status request consists of `task_id` field only.
/// Please do not add new fields unless they are used in most cases.
#[derive(Deserialize)]
pub struct CancelRpcTaskRequest {
    pub task_id: TaskId,
}

/// In most cases, the response to the RPC task initialization consists of `task_id` only.
/// Please do not add new fields unless they are used in most cases.
#[derive(Serialize)]
pub struct InitRpcTaskResponse {
    pub task_id: TaskId,
}
