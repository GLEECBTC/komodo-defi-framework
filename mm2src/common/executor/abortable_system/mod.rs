use crate::executor::spawn;
use crate::log::LogOnError;
use futures::channel::oneshot;
use parking_lot::Mutex as PaMutex;
use std::fmt;
use std::sync::{Arc, Weak};

pub mod abortable_queue;
pub mod graceful_shutdown;
pub mod simple_map;

pub type InnerShared<Inner> = Arc<PaMutex<Inner>>;
pub type InnerWeak<Inner> = Weak<PaMutex<Inner>>;

#[derive(Clone, Debug, PartialEq)]
pub struct AbortedError;

impl fmt::Display for AbortedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Abortable system has been aborted already")
    }
}

pub trait AbortableSystem: From<InnerShared<Self::Inner>> {
    type Inner: SystemInner;

    /// Aborts all spawned futures and subsystems if they present.
    /// The abortable system is considered not to be
    fn abort_all(&self) -> Result<(), AbortedError> {
        self.__inner().lock().abort_all()
    }

    /// Aborts all the spawned futures & subsystems if present, and resets the system
    /// to the initial state for further use.
    fn abort_all_and_reset(&self) -> Result<(), AbortedError> {
        let inner = self.__inner();
        let mut inner_locked = inner.lock();
        // Don't allow resetting the system state if the system is already aborted. If the system is
        // aborted this is because its parent was aborted as well. Resetting it will leave the system
        // dangling with no parent to abort it (could still be aborted manually of course).
        if inner_locked.is_aborted() {
            return Err(AbortedError);
        }
        let mut previous_inner = std::mem::take(&mut *inner_locked);
        previous_inner.abort_all().ok();
        Ok(())
    }

    /// Creates a new subsystem `S` linked to `Self` the way that
    /// if `Self` is aborted, the futures spawned by the subsystem will be aborted as well.
    /// For more info, look at the [`tests::test_abort_subsystem`].
    ///
    ///
    /// But in the same time the subsystem can be aborted independently from `Self` system.
    /// For more info, look at the [`tests::test_abort_supersystem`].
    fn create_subsystem<S>(&self) -> Result<S, AbortedError>
    where
        S: AbortableSystem,
    {
        let (abort_tx, abort_rx) = oneshot::channel();
        self.__push_subsystem_abort_tx(abort_tx)?;

        let inner_shared = Arc::new(PaMutex::new(S::Inner::default()));
        let inner_weak = Arc::downgrade(&inner_shared);

        let abort_fut = async move {
            // Once the `abort_rx` is invoked, we need to abort its all futures.
            abort_rx.await.ok();

            if let Some(inner_arc) = inner_weak.upgrade() {
                inner_arc.lock().abort_all().warn_log();
            }
        };

        spawn(abort_fut);
        Ok(S::from(inner_shared))
    }

    fn __inner(&self) -> InnerShared<Self::Inner>;

    fn __push_subsystem_abort_tx(&self, subsystem_abort_tx: oneshot::Sender<()>) -> Result<(), AbortedError>;
}

pub trait SystemInner: Default + Send + 'static {
    /// Aborts all spawned futures and subsystems if they present.
    fn abort_all(&mut self) -> Result<(), AbortedError>;

    /// Returns whether the system has already been aborted.
    fn is_aborted(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::block_on;
    use crate::executor::{SpawnFuture, Timer};
    use abortable_queue::AbortableQueue;

    #[test]
    fn test_abort_subsystem() {
        static SUPER_FINISHED: AtomicBool = AtomicBool::new(false);
        static SUB_FINISHED: AtomicBool = AtomicBool::new(false);

        let super_system = AbortableQueue::default();
        super_system.weak_spawner().spawn(async move {
            Timer::sleep(0.5).await;
            SUPER_FINISHED.store(true, Ordering::Relaxed);
        });

        let sub_system: AbortableQueue = super_system.create_subsystem().unwrap();
        sub_system.weak_spawner().spawn(async move {
            Timer::sleep(0.5).await;
            SUB_FINISHED.store(true, Ordering::Relaxed);
        });

        block_on(Timer::sleep(0.1));
        drop(sub_system);
        block_on(Timer::sleep(0.8));

        // Only the super system should finish as the sub system has been aborted.
        assert!(SUPER_FINISHED.load(Ordering::Relaxed));
        assert!(!SUB_FINISHED.load(Ordering::Relaxed));
    }

    #[test]
    fn test_abort_supersystem() {
        static SUPER_FINISHED: AtomicBool = AtomicBool::new(false);
        static SUB_FINISHED: AtomicBool = AtomicBool::new(false);

        let super_system = AbortableQueue::default();
        super_system.weak_spawner().spawn(async move {
            Timer::sleep(0.5).await;
            SUPER_FINISHED.store(true, Ordering::Relaxed);
        });

        let sub_system: AbortableQueue = super_system.create_subsystem().unwrap();
        sub_system.weak_spawner().spawn(async move {
            Timer::sleep(0.5).await;
            SUB_FINISHED.store(true, Ordering::Relaxed);
        });

        block_on(Timer::sleep(0.1));
        drop(super_system);
        block_on(Timer::sleep(0.8));

        // Check if the subsystem can't be aborted twice.
        sub_system.abort_all().unwrap_err();

        // Nothing should finish as the super system has been aborted.
        assert!(!SUPER_FINISHED.load(Ordering::Relaxed));
        assert!(!SUB_FINISHED.load(Ordering::Relaxed));
    }
}
