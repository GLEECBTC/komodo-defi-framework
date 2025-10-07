use common::custom_futures::timeout::TimeoutError;
use derive_more::Display;
use futures::channel::oneshot;
use mm2_err_handle::prelude::*;
use serde::Serialize;
use std::sync::atomic::AtomicU64;
use std::time::Duration;

#[macro_use]
extern crate ser_error_derive;
#[macro_use]
extern crate serde_derive;

mod handle;
mod manager;
pub mod rpc_common;
mod task;

pub use handle::{RpcTaskHandle, RpcTaskHandleShared};
pub use manager::{RpcTaskManager, RpcTaskManagerShared};
pub use task::{RpcInitReq, RpcTask, RpcTaskTypes};

pub type RpcTaskResult<T> = Result<T, MmError<RpcTaskError>>;
pub type TaskId = u64;
pub type RpcTaskStatusAlias<Task> = RpcTaskStatus<
    <Task as RpcTaskTypes>::Item,
    <Task as RpcTaskTypes>::Error,
    <Task as RpcTaskTypes>::InProgressStatus,
    <Task as RpcTaskTypes>::AwaitingStatus,
>;

type AtomicTaskId = AtomicU64;
type TaskAbortHandle = oneshot::Sender<()>;
type TaskAbortHandler = oneshot::Receiver<()>;
type UserActionSender<UserAction> = oneshot::Sender<UserAction>;

#[derive(Clone, Display)]
pub enum RpcTaskError {
    #[display(fmt = "RPC task timeout '{_0:?}'")]
    Timeout(Duration),
    NoSuchTask(TaskId),
    #[display(fmt = "RPC '{task_id}' task is in unexpected status. Actual: '{actual}', expected: '{expected}'")]
    UnexpectedTaskStatus {
        task_id: TaskId,
        actual: TaskStatusError,
        expected: TaskStatusError,
    },
    #[display(fmt = "RPC 'task' is awaiting '{expected}' user action")]
    UnexpectedUserAction {
        expected: String,
    },
    Cancelled,
    Internal(String),
}

#[derive(Clone, Display)]
pub enum TaskStatusError {
    Idle,
    InProgress,
    AwaitingUserAction,
    Cancelled,
    Finished,
}

impl TaskStatusError {
    fn is_finished(&self) -> bool {
        matches!(self, TaskStatusError::Finished)
    }
}

impl From<TimeoutError> for RpcTaskError {
    fn from(e: TimeoutError) -> Self {
        RpcTaskError::Timeout(e.duration)
    }
}

/// We can't simplify the generic types because there are places where the [`RpcTaskStatus::map_err`] method is used.
#[derive(Debug, Serialize)]
#[serde(tag = "status", content = "details")]
pub enum RpcTaskStatus<Item, Error, InProgressStatus, AwaitingStatus>
where
    Item: Serialize,
    Error: SerMmErrorType,
{
    Ok(Item),
    Error(MmError<Error>),
    InProgress(InProgressStatus),
    UserActionRequired(AwaitingStatus),
}

impl<Item, Error, InProgressStatus, AwaitingStatus> RpcTaskStatus<Item, Error, InProgressStatus, AwaitingStatus>
where
    Item: Serialize,
    Error: SerMmErrorType,
{
    pub fn is_ready(&self) -> bool {
        matches!(self, RpcTaskStatus::Ok(_) | RpcTaskStatus::Error(_))
    }

    pub fn map_err<NewError, F>(self, f: F) -> RpcTaskStatus<Item, NewError, InProgressStatus, AwaitingStatus>
    where
        F: FnOnce(Error) -> NewError,
        NewError: SerMmErrorType,
    {
        match self {
            RpcTaskStatus::Ok(result) => RpcTaskStatus::Ok(result),
            RpcTaskStatus::Error(error) => RpcTaskStatus::Error(error.map(f)),
            RpcTaskStatus::InProgress(in_progress) => RpcTaskStatus::InProgress(in_progress),
            RpcTaskStatus::UserActionRequired(awaiting) => RpcTaskStatus::UserActionRequired(awaiting),
        }
    }
}

enum TaskStatus<Task: RpcTaskTypes> {
    Ok(Task::Item),
    Error(MmError<Task::Error>),
    InProgress(Task::InProgressStatus),
    UserActionRequired {
        awaiting_status: Task::AwaitingStatus,
        user_action_tx: UserActionSender<Task::UserAction>,
    },
}
