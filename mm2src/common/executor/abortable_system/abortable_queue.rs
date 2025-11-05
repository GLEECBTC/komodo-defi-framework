use crate::executor::abortable_system::{AbortableSystem, AbortedError, InnerShared, InnerWeak, SystemInner};
use crate::executor::spawner::{SpawnAbortable, SpawnFuture};
use crate::executor::{spawn, AbortSettings, Timer};
use crate::log::{error, LogOnError};
use futures::channel::oneshot;
use futures::future::{abortable, select, Either};
use futures::FutureExt;
use std::future::Future as Future03;
use std::sync::Arc;

const CAPACITY: usize = 1024;

type FutureId = usize;

/// This is an `AbortableSystem` that ensures that the spawned futures will be aborted
/// once the `AbortableQueue` instance is dropped.
///
/// `AbortableQueue` is responsible for storing future handles in `QueueInnerState`
/// and deleting them as soon as they complete.
#[derive(Debug, Default)]
pub struct AbortableQueue {
    inner: InnerShared<QueueInnerState>,
}

impl AbortableQueue {
    /// Returns `WeakSpawner` that will not prevent the spawned futures from being aborted.
    /// This is the only way to create a `'static` instance pointing to the same `QueueInnerState`
    /// that can be passed into spawned futures, since `AbortableQueue` doesn't implement `Clone`.
    pub fn weak_spawner(&self) -> WeakSpawner {
        WeakSpawner {
            inner: Arc::downgrade(&self.inner),
        }
    }
}

impl From<InnerShared<QueueInnerState>> for AbortableQueue {
    fn from(inner: InnerShared<QueueInnerState>) -> Self {
        AbortableQueue { inner }
    }
}

impl AbortableSystem for AbortableQueue {
    type Inner = QueueInnerState;

    fn __inner(&self) -> InnerShared<Self::Inner> {
        self.inner.clone()
    }

    fn __push_subsystem_abort_tx(&self, subsystem_abort_tx: oneshot::Sender<()>) -> Result<(), AbortedError> {
        self.inner.lock().insert_handle(subsystem_abort_tx).map(|_| ())
    }
}

/// `WeakSpawner` doesn't prevent the spawned futures from being aborted.
/// An instance of `WeakSpawner` can be safely passed into spawned futures.
///
/// # Important
///
/// If corresponding `AbortableQueue` instance is dropped, [`WeakSpawner::spawn`] won't
/// actually spawn the future as it's more likely that the program, or part of the program,
/// ends its work, and there is no need to execute tasks that are no longer relevant.
#[derive(Clone)]
pub struct WeakSpawner {
    inner: InnerWeak<QueueInnerState>,
}

impl WeakSpawner {
    /// Spawns the `fut` future with the specified abort `settings`.
    /// The future won't be executed if `AbortableQueue` is dropped.
    fn spawn_with_settings_impl<F>(&self, fut: F, settings: AbortSettings) -> Result<(), AbortedError>
    where
        F: Future03<Output = ()> + Send + 'static,
    {
        let (abort_tx, abort_rx) = oneshot::channel();
        let future_id = match self.inner.upgrade() {
            Some(inner_arc) => {
                let mut inner_guard = inner_arc.lock();
                inner_guard.insert_handle(abort_tx)?
            },
            None => return Err(AbortedError),
        };

        let inner_weak = self.inner.clone();

        let (abortable_fut, abort_handle) = abortable(fut);

        let final_fut = async move {
            let critical_timeout_s = settings.critical_timeout_s;

            let wait_till_abort = async move {
                // First, wait for the `abort_tx` sender (i.e. corresponding [`QueueInnerState::abort_handlers`] item) is dropped.
                abort_rx.await.ok();

                // If the `critical_timeout_s` is set, give the `fut` future to try
                // to complete in `critical_timeout_s` seconds.
                if let Some(critical_timeout_s) = critical_timeout_s {
                    Timer::sleep(critical_timeout_s).await;
                }
            };

            match select(abortable_fut.boxed(), wait_till_abort.boxed()).await {
                // The future has finished normally.
                Either::Left((_, wait_till_abort_fut)) => {
                    if let Some(on_finish) = settings.on_finish {
                        log::log!(on_finish.level, "{}", on_finish.msg);
                    }

                    if let Some(queue_inner) = inner_weak.upgrade() {
                        // Drop the `wait_till_abort_fut` so to render the corresponding `abort_tx` sender canceled.
                        // This way we can query the `abort_tx` sender to check if it's canceled, thus safe to mark as finished.
                        drop(wait_till_abort_fut);
                        queue_inner.lock().on_future_finished(future_id);
                    }
                },
                // `abort_tx` has been removed from `QueueInnerState::abort_handlers`,
                // *and* the `critical_timeout_s` timeout has expired (if was specified).
                Either::Right(_) => {
                    if let Some(on_abort) = settings.on_abort {
                        log::log!(on_abort.level, "{}", on_abort.msg);
                    }

                    // Abort the input `fut`.
                    abort_handle.abort();
                },
            }
        };

        spawn(final_fut);
        Ok(())
    }
}

impl SpawnFuture for WeakSpawner {
    /// Records a warning to the log if the corresponding `AbortableQueue` system is aborted already.
    #[track_caller]
    fn spawn<F>(&self, f: F)
    where
        F: Future03<Output = ()> + Send + 'static,
    {
        self.spawn_with_settings_impl(f, AbortSettings::default()).warn_log()
    }
}

impl SpawnAbortable for WeakSpawner {
    /// Records a warning to the log if the corresponding `AbortableQueue` system is aborted already.
    #[track_caller]
    fn spawn_with_settings<F>(&self, fut: F, settings: AbortSettings)
    where
        F: Future03<Output = ()> + Send + 'static,
    {
        self.spawn_with_settings_impl(fut, settings).warn_log()
    }
}

/// `QueueInnerState` is the container of the spawned future handles [`oneshot::Sender<()>`].
/// It holds the future handles, gives every future its *unique* `FutureId` identifier
/// (unique between spawned and alive futures).
/// Once a future is finished, its `FutureId` can be reassign to another future.
/// This is necessary so that this container does not grow indefinitely.
#[derive(Debug)]
pub enum QueueInnerState {
    Ready {
        abort_handlers: Vec<oneshot::Sender<()>>,
        finished_futures: Vec<FutureId>,
    },
    Aborted,
}

impl Default for QueueInnerState {
    fn default() -> Self {
        QueueInnerState::Ready {
            abort_handlers: Vec::with_capacity(CAPACITY),
            finished_futures: Vec::with_capacity(CAPACITY),
        }
    }
}

impl QueueInnerState {
    /// Inserts the given future `handle`.
    fn insert_handle(&mut self, handle: oneshot::Sender<()>) -> Result<FutureId, AbortedError> {
        let (abort_handlers, finished_futures) = match self {
            QueueInnerState::Ready {
                abort_handlers,
                finished_futures,
            } => (abort_handlers, finished_futures),
            QueueInnerState::Aborted => return Err(AbortedError),
        };

        match finished_futures.pop() {
            // We can reuse the given `finished_id`.
            Some(finished_id) if finished_id < abort_handlers.len() => {
                abort_handlers[finished_id] = handle;
                // The freed future ID.
                return Ok(finished_id);
            },
            // An invalid `FutureId` has been popped from the `finished_futures` container.
            Some(invalid_finished_id) => {
                error!("'The finished future ID ({invalid_finished_id}) doesn't belong to any future. Number of futures = {}", abort_handlers.len());
            },
            // There are no finished future IDs.
            None => (),
        }

        abort_handlers.push(handle);
        // Return the last item ID.
        Ok(abort_handlers.len() - 1)
    }

    /// Releases the `finished_future_id` so it can be reused later on [`QueueInnerState::insert_handle`].
    fn on_future_finished(&mut self, finished_future_id: FutureId) {
        if let QueueInnerState::Ready {
            finished_futures,
            abort_handlers,
        } = self
        {
            // Only mark this ID as finished if a future existed for it and is canceled. We can get false
            // `on_future_finished` signals from futures that aren't in the `abort_handlers` anymore (abortable queue was reset).
            if let Some(handle) = abort_handlers.get(finished_future_id) {
                if handle.is_canceled() {
                    finished_futures.push(finished_future_id);
                }
            }
        }
    }

    #[cfg(test)]
    fn count_abort_handlers(&self) -> Result<usize, AbortedError> {
        match self {
            QueueInnerState::Ready { abort_handlers, .. } => Ok(abort_handlers.len()),
            QueueInnerState::Aborted => Err(AbortedError),
        }
    }

    #[cfg(test)]
    fn count_finished_futures(&self) -> Result<usize, AbortedError> {
        match self {
            QueueInnerState::Ready { finished_futures, .. } => Ok(finished_futures.len()),
            QueueInnerState::Aborted => Err(AbortedError),
        }
    }
}

impl SystemInner for QueueInnerState {
    fn abort_all(&mut self) -> Result<(), AbortedError> {
        if matches!(self, QueueInnerState::Aborted) {
            return Err(AbortedError);
        }

        *self = QueueInnerState::Aborted;
        Ok(())
    }

    fn is_aborted(&self) -> bool {
        matches!(self, QueueInnerState::Aborted)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::block_on;

    fn test_future_finished_impl(settings: AbortSettings) {
        let abortable_system = AbortableQueue::default();
        let spawner = abortable_system.weak_spawner();

        spawner.spawn_with_settings(async {}, settings.clone());
        block_on(Timer::sleep(0.1));

        {
            let inner = abortable_system.inner.lock();
            assert_eq!(inner.count_abort_handlers().unwrap(), 1);
            // The future should have finished already.
            assert_eq!(inner.count_finished_futures().unwrap(), 1);
        }

        let fut1 = async { Timer::sleep(0.3).await };
        let fut2 = async { Timer::sleep(0.7).await };
        spawner.spawn_with_settings(fut1, settings.clone());
        spawner.spawn_with_settings(fut2, settings);

        {
            let inner = abortable_system.inner.lock();
            // `abort_handlers` should be extended once
            // because `finished_futures` contained only one freed `FutureId`.
            assert_eq!(inner.count_abort_handlers().unwrap(), 2);
            // `FutureId` should be used from `finished_futures` container.
            assert_eq!(inner.count_finished_futures().unwrap(), 0);
        }

        block_on(Timer::sleep(0.5));

        {
            let inner = abortable_system.inner.lock();
            assert_eq!(inner.count_abort_handlers().unwrap(), 2);
            assert_eq!(inner.count_finished_futures().unwrap(), 1);
        }

        block_on(Timer::sleep(0.4));

        {
            let inner = abortable_system.inner.lock();
            assert_eq!(inner.count_abort_handlers().unwrap(), 2);
            assert_eq!(inner.count_finished_futures().unwrap(), 2);
        }
    }

    #[test]
    fn test_critical_future_finished() {
        let settings = AbortSettings::default().critical_timout_s(1.);
        test_future_finished_impl(settings);
    }

    #[test]
    fn test_future_finished() {
        let settings = AbortSettings::default();
        test_future_finished_impl(settings);
    }

    #[test]
    fn test_spawn_critical() {
        static F1_FINISHED: AtomicBool = AtomicBool::new(false);
        static F2_FINISHED: AtomicBool = AtomicBool::new(false);

        let abortable_system = AbortableQueue::default();
        let spawner = abortable_system.weak_spawner();

        let settings = AbortSettings::default().critical_timout_s(0.4);

        let fut1 = async move {
            Timer::sleep(0.6).await;
            F1_FINISHED.store(true, Ordering::Relaxed);
        };
        spawner.spawn_with_settings(fut1, settings.clone());

        let fut2 = async move {
            Timer::sleep(0.2).await;
            F2_FINISHED.store(true, Ordering::Relaxed);
        };
        spawner.spawn_with_settings(fut2, settings);

        abortable_system.abort_all().unwrap();

        block_on(Timer::sleep(1.2));
        // `fut1` must not complete.
        assert!(!F1_FINISHED.load(Ordering::Relaxed));
        // `fut` must complete.
        assert!(F2_FINISHED.load(Ordering::Relaxed));
    }

    #[test]
    fn test_spawn_after_abort() {
        static F1_FINISHED: AtomicBool = AtomicBool::new(false);

        for _ in 0..50 {
            let abortable_system = AbortableQueue::default();
            let spawner = abortable_system.weak_spawner();

            spawner.spawn(futures::future::ready(()));
            abortable_system.abort_all().unwrap();

            // This sleep allows to poll the `select(abortable_fut.boxed(), wait_till_abort.boxed()).await` future.
            block_on(Timer::sleep(0.01));

            spawner.spawn(async move {
                F1_FINISHED.store(true, Ordering::Relaxed);
            });
        }

        // Futures spawned after `AbortableQueue::abort_all` must not complete.
        assert!(!F1_FINISHED.load(Ordering::Relaxed));
    }
}
