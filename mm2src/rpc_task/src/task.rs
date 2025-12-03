use crate::handle::RpcTaskHandleShared;
use async_trait::async_trait;
use mm2_err_handle::prelude::*;
use serde::Serialize;

pub trait RpcTaskTypes {
    type Item: Serialize + Clone + Send + Sync + 'static;
    type Error: SerMmErrorType + Clone + Send + Sync + 'static;
    type InProgressStatus: Serialize + Clone + Send + Sync + 'static;
    type AwaitingStatus: Serialize + Clone + Send + Sync + 'static;
    type UserAction: NotMmError + Send + Sync + 'static;
}

#[async_trait]
pub trait RpcTask: RpcTaskTypes + Sized + Send + 'static {
    fn initial_status(&self) -> Self::InProgressStatus;

    /// The method is invoked when the task has been cancelled.
    async fn cancel(self);

    async fn run(&mut self, task_handle: RpcTaskHandleShared<Self>) -> Result<Self::Item, MmError<Self::Error>>;
}

/// The general request for initializing an RPC Task.
///
/// `client_id` is used to identify the client to which the task should stream out update events
/// to and is common in each request. Other data is request-specific.
#[derive(Default, Deserialize)]
pub struct RpcInitReq<T> {
    // If the client ID isn't included, assume it's 0.
    #[serde(default)]
    pub client_id: u64,
    #[serde(flatten)]
    pub inner: T,
}
