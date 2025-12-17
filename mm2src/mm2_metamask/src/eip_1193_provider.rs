use common::executor::{spawn_local_abortable, AbortOnDropHandle};
use common::log::error;
use futures::channel::{mpsc, oneshot};
use futures::future::BoxFuture;
use futures::StreamExt;
use jsonrpc_core::Call;
use serde::de::DeserializeOwned;
use serde_json::Value as Json;
use std::fmt;
use std::sync::Arc;
use web3::helpers::build_request;
use web3::transports::eip_1193::{Eip1193, Provider as RawProvider};
use web3::{Error, RequestId, Result, Transport};

type EipCommandSender = mpsc::UnboundedSender<EipCommand>;
type EipCommandReceiver = mpsc::UnboundedReceiver<EipCommand>;
type EipCommandResultSender<T> = oneshot::Sender<Result<T>>;

/// A wrapper over `Eip1193` transport, allowing it to be used cross-thread.
#[derive(Clone)]
pub struct Eip1193Provider {
    command_tx: EipCommandSender,
    /// This abort handle is needed to drop the spawned at [`WebUsbWrapper::new`] future immediately.
    _abort_handle: Arc<AbortOnDropHandle>,
}

impl Eip1193Provider {
    pub fn detect() -> Option<Eip1193Provider> {
        let raw_provider = RawProvider::default().ok()?.map(Eip1193::new)?;
        let (command_tx, command_rx) = mpsc::unbounded();
        let abort_handle = spawn_local_abortable(Self::command_loop(raw_provider, command_rx));

        Some(Eip1193Provider {
            command_tx,
            _abort_handle: Arc::new(abort_handle),
        })
    }

    pub async fn invoke_method<T>(&self, id: RequestId, request: Call) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let (result_tx, result_rx) = oneshot::channel();
        let command = EipCommand::InvokeMethod { id, request, result_tx };
        let result = send_command_recv_response(&self.command_tx, command, result_rx).await?;
        serde_json::from_value(result).map_err(|e| Error::InvalidResponse(e.to_string()))
    }

    async fn command_loop(transport: Eip1193, mut command_rx: EipCommandReceiver) {
        while let Some(command) = command_rx.next().await {
            match command {
                EipCommand::InvokeMethod { id, request, result_tx } => {
                    let res = transport.send(id, request).await;
                    result_tx.send(res).ok();
                },
            }
        }
    }
}

impl Transport for Eip1193Provider {
    type Out = BoxFuture<'static, Result<Json>>;

    fn prepare(&self, method: &str, params: Vec<Json>) -> (RequestId, Call) {
        // RequestId doesn't make sense for `MetamaskProvider`.
        const REQUEST_ID: RequestId = 0;

        let request = build_request(REQUEST_ID, method, params);
        (REQUEST_ID, request)
    }

    fn send(&self, id: RequestId, request: Call) -> Self::Out {
        let this = self.clone();
        let fut = async move { this.invoke_method(id, request).await };
        Box::pin(fut)
    }
}

impl fmt::Debug for Eip1193Provider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Eip1193Provider")
    }
}

async fn send_command_recv_response<Ok>(
    command_tx: &EipCommandSender,
    command: EipCommand,
    result_rx: oneshot::Receiver<Result<Ok>>,
) -> Result<Ok> {
    if let Err(e) = command_tx.unbounded_send(command) {
        error!("Error sending an EIP1193 command: {}", e);
        return Err(Error::Internal);
    }
    match result_rx.await {
        Ok(result) => result,
        Err(e) => {
            error!("Error receiving a an EIP1193 result: {}", e);
            Err(Error::Internal)
        },
    }
}

enum EipCommand {
    InvokeMethod {
        id: RequestId,
        request: Call,
        result_tx: EipCommandResultSender<Json>,
    },
}
