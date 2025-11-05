use crate::executor::abortable_system::{AbortedError, InnerShared, SystemInner};
use crate::executor::AbortableSystem;
use futures::channel::oneshot;
use futures::FutureExt;
use std::future::Future;

/// This is an `AbortableSystem` that initiates listeners for graceful shutdown
/// once the `GracefulShutdownRegistry` instance is dropped.
///
/// `GracefulShutdownRegistry` can be used in conjunction with the `spawn` method.
/// In some cases, the use of `GracefulShutdownRegistry` and `spawn` is justified.
/// For example, [`hyper::Server::with_graceful_shutdown`].
#[derive(Default)]
pub struct GracefulShutdownRegistry {
    inner: InnerShared<ShutdownInnerState>,
}

impl GracefulShutdownRegistry {
    /// Registers a graceful shutdown listener and returns a future
    /// that acts as a signal for graceful shutdown.
    pub fn register_listener(&self) -> Result<impl Future<Output = ()> + Send + Sync + 'static, AbortedError> {
        let (tx, rx) = oneshot::channel();
        self.inner.lock().insert_handle(tx)?;
        Ok(rx.then(|_| futures::future::ready(())))
    }
}

impl From<InnerShared<ShutdownInnerState>> for GracefulShutdownRegistry {
    fn from(inner: InnerShared<ShutdownInnerState>) -> Self {
        GracefulShutdownRegistry { inner }
    }
}

impl AbortableSystem for GracefulShutdownRegistry {
    type Inner = ShutdownInnerState;

    fn __inner(&self) -> InnerShared<Self::Inner> {
        self.inner.clone()
    }

    fn __push_subsystem_abort_tx(&self, subsystem_abort_tx: oneshot::Sender<()>) -> Result<(), AbortedError> {
        self.inner.lock().insert_handle(subsystem_abort_tx)
    }
}

pub enum ShutdownInnerState {
    Ready { abort_handlers: Vec<oneshot::Sender<()>> },
    Aborted,
}

impl Default for ShutdownInnerState {
    fn default() -> Self {
        ShutdownInnerState::Ready {
            abort_handlers: Vec::new(),
        }
    }
}

impl ShutdownInnerState {
    fn insert_handle(&mut self, handle: oneshot::Sender<()>) -> Result<(), AbortedError> {
        match self {
            ShutdownInnerState::Ready { abort_handlers } => {
                abort_handlers.push(handle);
                Ok(())
            },
            ShutdownInnerState::Aborted => Err(AbortedError),
        }
    }
}

impl SystemInner for ShutdownInnerState {
    fn abort_all(&mut self) -> Result<(), AbortedError> {
        if matches!(self, ShutdownInnerState::Aborted) {
            return Err(AbortedError);
        }

        *self = ShutdownInnerState::Aborted;
        Ok(())
    }

    fn is_aborted(&self) -> bool {
        matches!(self, ShutdownInnerState::Aborted)
    }
}
