use crate::manager::{RpcTaskManager, RpcTaskManagerWeak};
use crate::{RpcTask, RpcTaskError, RpcTaskResult, TaskId, TaskStatus};
use common::custom_futures::timeout::FutureTimerExt;
use common::log::LogOnError;
use futures::channel::oneshot;
use mm2_err_handle::prelude::*;
use std::sync::{Arc, MutexGuard};
use std::time::Duration;

type TaskManagerLock<'a, Task> = MutexGuard<'a, RpcTaskManager<Task>>;
pub type RpcTaskHandleShared<Task> = Arc<RpcTaskHandle<Task>>;

pub struct RpcTaskHandle<Task: RpcTask> {
    pub(crate) task_manager: RpcTaskManagerWeak<Task>,
    pub(crate) task_id: TaskId,
}

impl<Task: RpcTask> RpcTaskHandle<Task> {
    fn lock_and_then<F, T>(&self, f: F) -> RpcTaskResult<T>
    where
        F: FnOnce(TaskManagerLock<Task>) -> RpcTaskResult<T>,
    {
        let arc = self
            .task_manager
            .upgrade()
            .or_mm_err(|| RpcTaskError::Internal("RpcTaskManager is not available".to_owned()))?;
        let lock = arc
            .lock()
            .map_to_mm(|e| RpcTaskError::Internal(format!("RpcTaskManager is not available: {e}")))?;
        f(lock)
    }

    fn update_task_status(&self, status: TaskStatus<Task>) -> RpcTaskResult<()> {
        self.lock_and_then(|mut task_manager| task_manager.update_task_status(self.task_id, status))
    }

    pub fn update_in_progress_status(&self, in_progress: Task::InProgressStatus) -> RpcTaskResult<()> {
        self.update_task_status(TaskStatus::InProgress(in_progress))
    }

    pub async fn wait_for_user_action(
        &self,
        timeout: Duration,
        awaiting_status: Task::AwaitingStatus,
    ) -> RpcTaskResult<Task::UserAction> {
        let (user_action_tx, user_action_rx) = oneshot::channel();
        // Set the status to 'UserActionRequired' to let the user know that we are waiting for an action.
        self.update_task_status(TaskStatus::UserActionRequired {
            awaiting_status,
            user_action_tx,
        })?;

        // Wait for the user action.
        user_action_rx
            .timeout(timeout)
            .await?
            .map_to_mm(|_canceled| RpcTaskError::Cancelled)
    }

    pub(crate) fn finish(&self, result: Result<Task::Item, MmError<Task::Error>>) {
        let task_status = Self::prepare_task_result(result);
        self.lock_and_then(|mut task_manager| task_manager.update_task_status(self.task_id, task_status))
            .warn_log();
    }

    pub(crate) fn on_cancelled(&self) {
        self.lock_and_then(|mut task_manager| task_manager.on_task_cancelling_finished(self.task_id))
            .warn_log();
    }

    fn prepare_task_result(result: Result<Task::Item, MmError<Task::Error>>) -> TaskStatus<Task> {
        match result {
            Ok(task_item) => TaskStatus::Ok(task_item),
            Err(task_error) => TaskStatus::Error(task_error),
        }
    }
}
